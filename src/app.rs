use crate::platform::{self, Event, Native, Target};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows_password_manager::{
    FillMode, InputUnit, fill_plan, maintenance,
    security::{Grant, IdleSource, LOCK_MINUTES, LockClock, LockSettings, ProtectedAction},
    session::Session,
    shortcut::Shortcut,
    startup,
    vault::{Credential, Vault},
};
use zeroize::Zeroizing;
slint::include_modules!();

struct Pending {
    target: Target,
    inputs: Zeroizing<Vec<InputUnit>>,
    password: Zeroizing<Vec<InputUnit>>,
    tab_sent: Option<Instant>,
    panel: usize,
    started: Instant,
    restored: Option<Instant>,
    record: i64,
    application: String,
}
struct State {
    session: Rc<RefCell<Session>>,
    panel: usize,
    minimized_at: Option<Instant>,
    clock: Arc<LockClock>,
    requested: Option<(ProtectedAction, u64, Instant)>,
    request_serial: u64,
    auth_cancel: Arc<AtomicBool>,
    grant: Option<Grant>,
    editor_verified: Option<(i64, u64)>,
    editing_id: i64,
    deleting_id: i64,
    target: Option<Target>,
    target_application: String,
    details_id: Option<i64>,
    matches: Vec<i64>,
    pending: Option<Pending>,
    backup_paths: Vec<PathBuf>,
    context_record: Option<(i64, u64)>,
}

fn window_id(ui: &AppWindow) -> Result<usize, String> {
    match ui
        .window()
        .window_handle()
        .window_handle()
        .map_err(|e| e.to_string())?
        .as_raw()
    {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as usize),
        _ => Err("无法获取 Windows 窗口句柄。".into()),
    }
}

fn clear_editor(ui: &AppWindow) {
    ui.set_editing(false);
    ui.set_edit_dirty(false);
    ui.set_discard_action(0);
    ui.set_edit_title("".into());
    ui.set_edit_user("".into());
    ui.set_edit_password("".into());
    ui.set_edit_category("".into());
    ui.set_edit_tags("".into());
    ui.set_edit_url("".into());
    ui.set_edit_notes("".into());
    ui.set_edit_sensitive(false);
}

fn clear_details(ui: &AppWindow) {
    ui.set_details_open(false);
    ui.set_details_title("".into());
    ui.set_details_user("".into());
    ui.set_details_password("".into());
    ui.set_details_category("".into());
    ui.set_details_tags("".into());
    ui.set_details_url("".into());
    ui.set_details_notes("".into());
}

fn clear_maintenance(ui: &AppWindow) {
    ui.set_context_open(false);
    clear_backup_list(ui);
    ui.set_maintenance_open(false);
    ui.set_backups_confirm(false);
    ui.set_current_master("".into());
    ui.set_new_master("".into());
    ui.set_new_confirmation("".into());
}

fn clear_backup_list(ui: &AppWindow) {
    ui.set_backups_open(false);
    ui.set_backup_master("".into());
    ui.set_backup_confirm_mode(0);
    ui.set_selected_backup(-1);
    ui.set_backup_rows(ModelRc::new(VecModel::from(Vec::<BackupRow>::new())));
}

fn refresh_backups(ui: &AppWindow, state: &mut State) -> Result<(), String> {
    let selected = usize::try_from(ui.get_selected_backup())
        .ok()
        .and_then(|i| state.backup_paths.get(i))
        .cloned();
    let session = state.session.borrow();
    let backups = maintenance::backup_list(&session.root, &session.path)?;
    let rows: Vec<_> = backups
        .iter()
        .map(|entry| BackupRow {
            title: format!("{} · {}", entry.kind, platform::backup_time(entry.nanos)).into(),
            detail: format!(
                "{:.1} KiB · {}",
                entry.bytes as f64 / 1024.0,
                entry.path.file_name().unwrap().to_string_lossy()
            )
            .into(),
        })
        .collect();
    state.backup_paths = backups.into_iter().map(|b| b.path).collect();
    ui.set_backup_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_backup_count(state.backup_paths.len() as i32);
    ui.set_selected_backup(if state.backup_paths.is_empty() {
        -1
    } else {
        selected
            .and_then(|path| state.backup_paths.iter().position(|p| p == &path))
            .unwrap_or(0) as i32
    });
    ui.set_backup_confirm_mode(0);
    ui.set_backup_master("".into());
    Ok(())
}

fn rich(ui: &AppWindow, query: &str, text: &str) -> slint::StyledText {
    let color = ui.get_match_color();
    let markup = windows_password_manager::search::highlights(query, text)
        .into_iter()
        .map(|(part, hit)| {
            let escaped = windows_password_manager::search::highlighted_markdown("", &part);
            if hit {
                format!("<font color=\"{color}\">**{escaped}**</font>")
            } else {
                escaped
            }
        })
        .collect::<String>();
    slint::StyledText::from_markdown(&markup)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(text))
}

fn success(ui: &AppWindow, message: String) {
    ui.set_status(message.clone().into());
    ui.set_success_status(message.clone().into());
    let serial = ui.get_status_serial().wrapping_add(1);
    ui.set_status_serial(serial);
    let weak = ui.as_weak();
    Timer::single_shot(Duration::from_secs(4), move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_status_serial() == serial
                && ui.get_status_success()
                && ui.get_status().as_str() == message
            {
                ui.set_status("".into());
            }
        }
    });
}

fn refresh(ui: &AppWindow, state: &mut State) {
    let previously_selected = selected_id(ui, state);
    let session = state.session.borrow();
    ui.set_blocked(session.working || session.auth_owner.is_some());
    let settings = state.clock.settings();
    ui.set_policy_label(
        format!(
            "{}空闲 {} 分钟自动锁定 · 不自动提交登录",
            if settings.source == IdleSource::Computer {
                "电脑"
            } else {
                "密码库"
            },
            settings.minutes
        )
        .into(),
    );
    ui.set_unlocked(session.vault.is_some());
    ui.set_working(session.working);
    ui.set_new_vault(!session.path.exists());
    ui.set_vault_path(session.path.to_string_lossy().as_ref().into());
    state.matches.clear();
    let records = match session.vault.as_ref().map(|v| v.list()).transpose() {
        Ok(records) => records.unwrap_or_default(),
        Err(message) => {
            ui.set_status(message.into());
            Vec::new()
        }
    };
    let mut categories = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut rows = Vec::new();
    for record in &records {
        if !record.category.is_empty() {
            categories.insert(record.category.clone());
        }
        for tag in record
            .tags
            .split([',', '，', ';', '；'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            tags.insert(tag.to_owned());
        }
    }
    if !categories.contains(ui.get_category().as_str()) {
        ui.set_category("".into());
    }
    if !tags.contains(ui.get_tag().as_str()) {
        ui.set_tag("".into());
    }
    let application_usage = session
        .vault
        .as_ref()
        .map(|vault| vault.recent_for_application(&state.target_application))
        .transpose()
        .unwrap_or_else(|message| {
            ui.set_status(message.into());
            None
        })
        .unwrap_or_default();
    let mut ranked: Vec<_> = records
        .into_iter()
        .filter_map(|record| {
            record
                .rank(
                    ui.get_query().as_str(),
                    ui.get_category().as_str(),
                    ui.get_tag().as_str(),
                )
                .map(|score| (score, record))
        })
        .collect();
    ranked.sort_by(|(a_score, a), (b_score, b)| {
        b_score
            .cmp(a_score)
            .then_with(|| {
                application_usage
                    .get(&b.id)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&application_usage.get(&a.id).copied().unwrap_or(0))
            })
            .then_with(|| b.last_used.cmp(&a.last_used))
            .then_with(|| a.title.cmp(&b.title))
            .then(a.id.cmp(&b.id))
    });
    for (_, record) in ranked {
        state.matches.push(record.id);
        let query = ui.get_query();
        let title = if record.sensitive {
            format!("敏感 · {}", record.title)
        } else {
            record.title.clone()
        };
        let username = if record.sensitive {
            "使用前需验证".to_string()
        } else {
            record.username.clone()
        };
        let matched = |value: &str| {
            windows_password_manager::search::highlights(query.as_str(), value)
                .iter()
                .any(|(_, hit)| *hit)
        };
        let mut detail = [&record.category, &record.tags]
            .into_iter()
            .filter(|value| !value.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        if matched(&record.url) {
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&format!("网址：{}", record.url));
        }
        let source = [
            ("分类", &record.category),
            ("标签", &record.tags),
            ("网址", &record.url),
        ]
        .into_iter()
        .filter(|(_, value)| matched(value))
        .map(|(label, value)| format!("{label}：{value}"))
        .collect::<Vec<_>>()
        .join(" · ");
        let hint = if source.is_empty() {
            username.clone()
        } else {
            format!("{username} · {source}")
        };
        rows.push(EntryRow {
            title_rich: rich(ui, query.as_str(), &title),
            user_rich: if record.sensitive {
                slint::StyledText::from_plain_text(&username)
            } else {
                rich(ui, query.as_str(), &username)
            },
            detail_rich: rich(ui, query.as_str(), &detail),
            hint_rich: rich(ui, query.as_str(), &hint),
            hint: hint.into(),
            title: title.into(),
            username: username.into(),
            detail: detail.into(),
        });
    }
    ui.set_categories(ModelRc::new(VecModel::from(
        std::iter::once("全部分类".into())
            .chain(categories.iter().map(|value| value.as_str().into()))
            .collect::<Vec<_>>(),
    )));
    ui.set_tags(ModelRc::new(VecModel::from(
        std::iter::once("全部标签".into())
            .chain(tags.iter().map(|value| value.as_str().into()))
            .collect::<Vec<_>>(),
    )));
    ui.set_category_index(
        categories
            .iter()
            .position(|s| s == ui.get_category().as_str())
            .map_or(0, |i| i as i32 + 1),
    );
    ui.set_tag_index(
        tags.iter()
            .position(|s| s == ui.get_tag().as_str())
            .map_or(0, |i| i as i32 + 1),
    );
    ui.set_entries(ModelRc::new(VecModel::from(rows)));
    ui.set_selected(
        previously_selected
            .and_then(|id| state.matches.iter().position(|&item| item == id))
            .map(|i| i as i32)
            .unwrap_or(if state.matches.is_empty() { -1 } else { 0 }),
    );
    ui.invoke_scroll_selected();
    ui.set_delete_confirm(false);
    if session.vault.is_none() {
        clear_editor(ui);
        clear_details(ui);
        state.details_id = None;
        ui.set_master("".into());
        ui.set_confirmation("".into());
        ui.set_can_fill(false);
    }
}

fn selected_id(ui: &AppWindow, state: &State) -> Option<i64> {
    usize::try_from(ui.get_selected())
        .ok()
        .and_then(|i| state.matches.get(i).copied())
}

enum JobResult {
    Reset(Result<(), String>),
    Unlock(u64, Result<Vault, String>),
    Maintenance(Result<String, String>),
    Backup {
        generation: u64,
        result: Result<Option<(Vault, PathBuf)>, String>,
    },
    Verify {
        panel: usize,
        serial: u64,
        generation: u64,
        result: Result<(), String>,
    },
}

fn cancel_auth(ui: &AppWindow, state: &mut State) {
    state.auth_cancel.store(true, Ordering::SeqCst);
    state.request_serial = state.request_serial.wrapping_add(1);
    state.requested = None;
    state.grant = None;
    if state.session.borrow().auth_owner == Some(state.panel) {
        state.session.borrow_mut().auth_owner = None;
    }
    ui.set_verification_active(false);
    ui.set_verify_master("".into());
    ui.set_verification_label("".into());
}

fn available(state: &State) -> bool {
    let session = state.session.borrow();
    session.vault.is_some() && !session.working && session.auth_owner.is_none()
}

fn authorize(ui: &AppWindow, state: &mut State, action: ProtectedAction) -> bool {
    if !available(state) {
        ui.set_status("请先完成或取消当前验证。".into());
        return false;
    }
    let result = {
        let session = state.session.borrow();
        let vault = session.vault.as_ref().unwrap();
        vault.is_sensitive(action.record()).and_then(|sensitive| {
            vault
                .title(action.record())
                .map(|title| (sensitive, title, session.generation))
        })
    };
    let (sensitive, title, generation) = match result {
        Ok(value) => value,
        Err(message) => {
            ui.set_status(message.into());
            return false;
        }
    };
    if !sensitive {
        state.grant = None;
        return true;
    }
    if Grant::consume(&mut state.grant, action, generation) {
        return true;
    }
    state.clock.touch();
    state.request_serial = state.request_serial.wrapping_add(1);
    state.auth_cancel = Arc::new(AtomicBool::new(false));
    state.requested = Some((action, generation, Instant::now()));
    state.session.borrow_mut().auth_owner = Some(state.panel);
    ui.set_verification_active(true);
    ui.set_verify_source(0);
    ui.set_verify_master("".into());
    let operation = match action {
        ProtectedAction::View(_) => "查看账号详情",
        ProtectedAction::Edit(_) => "查看或编辑账号",
        ProtectedAction::Delete(_) => "删除账号",
        ProtectedAction::Fill(_, FillMode::Password) => "填入密码",
        ProtectedAction::Fill(_, FillMode::Username) => "填入用户名",
        ProtectedAction::Fill(_, FillMode::Both) => "连填用户名和密码",
    };
    ui.set_verification_label(format!("{operation} · {title}").into());
    ui.set_status("".into());
    false
}

fn guard_editor(ui: &AppWindow, action: i32) -> bool {
    if ui.get_editing() && ui.get_edit_dirty() && ui.get_unlocked() {
        ui.set_discard_action(action);
        ui.invoke_focus_discard();
        true
    } else {
        false
    }
}

fn hide_window(ui: &AppWindow) -> Result<(), String> {
    if ui.get_compact() {
        let _ = platform::watch_popup(None);
    }
    ui.hide().map_err(|error| error.to_string())
}
fn show_window(ui: &AppWindow) -> Result<(), String> {
    ui.show().map_err(|error| error.to_string())?;
    if ui.get_compact() {
        platform::watch_popup(Some(window_id(ui)?))?;
    }
    Ok(())
}

fn cancel(ui: &AppWindow, state: &mut State, timer: &Timer) {
    state.clock.touch();
    timer.stop();
    cancel_auth(ui, state);
    state.editor_verified = None;
    ui.set_settings_open(false);
    ui.set_about_open(false);
    ui.set_context_open(false);
    ui.set_app_menu_open(false);
    clear_maintenance(ui);
    state.pending = None;
    state.target = None;
    state.target_application.clear();
    state.details_id = None;
    clear_details(ui);
    ui.set_busy(false);
    ui.set_can_fill(false);
    ui.set_target_label("".into());
    ui.set_target_detail("".into());
    ui.set_master("".into());
    ui.set_confirmation("".into());
    clear_editor(ui);
    let _ = hide_window(ui);
}

fn report(ui: &AppWindow, state: &mut State, message: String) {
    state.pending = None;
    state.target = None;
    ui.set_busy(false);
    ui.set_can_fill(false);
    ui.set_status(message.into());
    ui.set_target_label("本次填充已取消；请重新点击输入框并按呼出快捷键。".into());
    ui.set_target_detail("".into());
    let _ = show_window(ui);
    if let Ok(window) = window_id(ui) {
        platform::activate(window);
    }
    ui.invoke_focus_search();
}

fn bind(
    ui: &AppWindow,
    session: Rc<RefCell<Session>>,
    jobs: mpsc::Sender<JobResult>,
    clock: Arc<LockClock>,
    panel: usize,
    lock_signal: Arc<AtomicBool>,
) -> (Rc<RefCell<State>>, Rc<Timer>) {
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    let state = Rc::new(RefCell::new(State {
        session: session.clone(),
        panel,
        minimized_at: None,
        clock: clock.clone(),
        requested: None,
        request_serial: 0,
        auth_cancel: Arc::new(AtomicBool::new(false)),
        grant: None,
        editor_verified: None,
        target: None,
        target_application: String::new(),
        details_id: None,
        matches: Vec::new(),
        pending: None,
        editing_id: 0,
        deleting_id: 0,
        backup_paths: Vec::new(),
        context_record: None,
    }));
    let timer = Rc::new(Timer::default());
    {
        let weak = ui.as_weak();
        ui.on_open_slint_site(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(message) = platform::open_web_url("https://slint.dev") {
                    ui.set_status(message.into());
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_about(move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_working() || ui.get_editing() || ui.get_verification_active() {
                    return;
                }
                state.borrow().clock.touch();
                clear_details(&ui);
                state.borrow_mut().details_id = None;
                ui.set_about_from_settings(ui.get_settings_open());
                ui.set_settings_open(false);
                ui.set_app_menu_open(false);
                ui.set_about_open(true);
                ui.invoke_focus_search();
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_close_about(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_about_open(false);
                ui.set_settings_open(ui.get_about_from_settings() && ui.get_unlocked());
                ui.invoke_focus_search();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_minimize_window(move || {
            let weak = weak.clone();
            let state = state.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    state.borrow_mut().minimized_at = Some(Instant::now());
                    ui.window().set_minimized(true);
                }
            });
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_search_changed(move || {
            if let Some(ui) = weak.upgrade() {
                state.borrow().clock.touch();
                state.borrow_mut().matches.clear();
                refresh(&ui, &mut state.borrow_mut());
                ui.invoke_scroll_selected();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let timer = timer.clone();
        ui.on_cancel(move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_working() && !ui.get_compact() {
                    return;
                }
                if ui.get_compact()
                    && ui.get_working()
                    && !ui.get_verification_active()
                    && !ui.get_unlocked()
                {
                    ui.invoke_lock_vault();
                    return;
                }
                if ui.get_reset_open() {
                    ui.invoke_close_reset();
                    return;
                }
                if guard_editor(&ui, 2) {
                    return;
                }
                cancel(&ui, &mut state.borrow_mut(), &timer);
                ui.invoke_session_changed();
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.window().on_close_requested(move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_cancel();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_exit_app(move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_working() {
                    return;
                }
                if guard_editor(&ui, 3) {
                    return;
                }
            }
            let _ = slint::quit_event_loop();
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let timer = timer.clone();
        ui.on_finish_discard(move |discard| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.get_discard_open() || !ui.get_unlocked() {
                return;
            }
            let action = ui.get_discard_action();
            ui.set_discard_action(0);
            if !discard {
                ui.invoke_focus_search();
                return;
            }
            clear_editor(&ui);
            state.borrow_mut().editor_verified = None;
            match action {
                2 => {
                    cancel(&ui, &mut state.borrow_mut(), &timer);
                    ui.invoke_session_changed();
                }
                3 => {
                    let _ = slint::quit_event_loop();
                }
                _ => {
                    ui.set_status("已放弃未保存的修改。".into());
                    ui.invoke_focus_search();
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let session = session.clone();
        let jobs = jobs.clone();
        ui.on_unlock(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut session = session.borrow_mut();
            if session.working || session.vault.is_some() {
                return;
            }
            let master = Zeroizing::new(ui.get_master().to_string());
            let creating = !session.path.exists();
            let confirmation = Zeroizing::new(ui.get_confirmation().to_string());
            ui.set_master("".into());
            ui.set_confirmation("".into());
            if creating && master.as_str() != confirmation.as_str() {
                ui.set_status("两次主密码不一致，请重新输入。".into());
                return;
            }
            if master.is_empty() {
                ui.set_status("请输入主密码。".into());
                return;
            }
            session.working = true;
            let generation = session.generation;
            let path = session.path.clone();
            let root = session.root.clone();
            drop(session);
            ui.set_status("".into());
            ui.invoke_session_changed();
            let jobs = jobs.clone();
            std::thread::spawn(move || {
                let result = maintenance::recover(&root)
                    .and_then(|_| {
                        if creating {
                            Vault::create(&path, &master)
                        } else {
                            Vault::open(&path, &master)
                        }
                    })
                    .and_then(|vault| {
                        vault.check()?;
                        Ok(vault)
                    });
                let _ = jobs.send(JobResult::Unlock(generation, result));
            });
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let clock = clock.clone();
        ui.on_new_entry(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) {
                return;
            }
            if guard_editor(&ui, 1) {
                return;
            }
            clear_editor(&ui);
            ui.set_edit_is_new(true);
            state.borrow_mut().editing_id = 0;
            state.borrow_mut().editor_verified = None;
            clock.touch();
            ui.set_status("".into());
            ui.set_editing(true);
            ui.invoke_focus_search();
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_view_entry(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.get_compact() || ui.get_editing() || ui.get_delete_confirm() {
                return;
            }
            let mut state = state.borrow_mut();
            let Some(id) = selected_id(&ui, &state) else {
                return;
            };
            if !authorize(&ui, &mut state, ProtectedAction::View(id)) {
                drop(state);
                ui.invoke_session_changed();
                ui.invoke_focus_search();
                return;
            }
            let record = state
                .session
                .borrow()
                .vault
                .as_ref()
                .ok_or("密码库已锁定。".to_string())
                .and_then(|v| v.get(id));
            match record {
                Ok(record) => {
                    clear_details(&ui);
                    state.details_id = Some(id);
                    state.clock.touch();
                    ui.set_details_title(record.title.into());
                    ui.set_details_user(record.username.into());
                    ui.set_details_password(record.password.as_str().into());
                    ui.set_details_category(record.category.into());
                    ui.set_details_tags(record.tags.into());
                    ui.set_details_url(record.url.into());
                    ui.set_details_notes(record.notes.into());
                    ui.set_status("".into());
                    ui.set_details_open(true);
                    ui.invoke_focus_search();
                }
                Err(message) => ui.set_status(message.into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_close_details(move || {
            if let Some(ui) = weak.upgrade() {
                clear_details(&ui);
                state.borrow_mut().details_id = None;
                ui.invoke_focus_search();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_entry_url(move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_details_open()
                    && available(&state.borrow())
                    && state.borrow().details_id.is_some()
                {
                    state.borrow().clock.touch();
                    if let Err(message) = platform::open_web_url(ui.get_details_url().as_str()) {
                        ui.set_status(message.into());
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_edit_entry(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            let Some(id) = selected_id(&ui, &state) else {
                return;
            };
            if !authorize(&ui, &mut state, ProtectedAction::Edit(id)) {
                drop(state);
                ui.invoke_session_changed();
                ui.invoke_focus_search();
                return;
            }
            clear_details(&ui);
            state.details_id = None;
            state.clock.touch();
            let generation = state.session.borrow().generation;
            state.editor_verified = Some((id, generation));
            let record = state
                .session
                .borrow()
                .vault
                .as_ref()
                .ok_or("密码库已锁定。".to_string())
                .and_then(|v| v.get(id));
            match record {
                Ok(record) => {
                    state.editing_id = record.id;
                    ui.set_edit_sensitive(record.sensitive);
                    ui.set_edit_title(record.title.into());
                    ui.set_edit_user(record.username.into());
                    ui.set_edit_password(record.password.as_str().into());
                    ui.set_edit_category(record.category.into());
                    ui.set_edit_tags(record.tags.into());
                    ui.set_edit_url(record.url.into());
                    ui.set_edit_notes(record.notes.into());
                    ui.set_status("".into());
                    ui.set_edit_is_new(false);
                    ui.set_edit_dirty(false);
                    ui.set_editing(true);
                    ui.invoke_focus_search();
                }
                Err(message) => ui.set_status(message.into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_cancel_edit(move || {
            if let Some(ui) = weak.upgrade() {
                if guard_editor(&ui, 1) {
                    return;
                }
                clear_editor(&ui);
                state.borrow_mut().editor_verified = None;
                ui.set_status("".into());
                ui.invoke_focus_search();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_save_entry(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.get_editing() || ui.get_discard_open() || !available(&state.borrow()) {
                return;
            }
            state.borrow().clock.touch();
            let record = Credential {
                id: state.borrow().editing_id,
                title: ui.get_edit_title().trim().to_owned(),
                username: ui.get_edit_user().to_string(),
                password: Zeroizing::new(ui.get_edit_password().to_string()),
                category: ui.get_edit_category().trim().to_owned(),
                tags: ui.get_edit_tags().to_string(),
                url: ui.get_edit_url().to_string(),
                notes: ui.get_edit_notes().to_string(),
                sensitive: ui.get_edit_sensitive(),
            };
            let result = (|| {
                let state = state.borrow();
                let mut session = state.session.borrow_mut();
                if record.id != 0 {
                    if session
                        .vault
                        .as_ref()
                        .ok_or("密码库已锁定。")?
                        .is_sensitive(record.id)?
                        && state.editor_verified != Some((record.id, session.generation))
                    {
                        return Err("请重新验证后再编辑敏感账号。".into());
                    }
                    session.backup_before_change()?;
                }
                let id = session
                    .vault
                    .as_mut()
                    .ok_or("密码库已锁定。".to_string())?
                    .save(&record)?;
                Ok::<_, String>(id)
            })();
            match result {
                Ok(id) => {
                    clear_editor(&ui);
                    state.borrow_mut().editor_verified = None;
                    ui.set_query("".into());
                    ui.set_category("".into());
                    ui.set_tag("".into());
                    success(&ui, "账号已保存。".into());
                    ui.invoke_session_changed();
                    if let Some(index) = state
                        .borrow()
                        .matches
                        .iter()
                        .position(|record| *record == id)
                    {
                        ui.set_selected(index as i32);
                    }
                    ui.invoke_scroll_selected();
                    ui.invoke_focus_search();
                }
                Err(message) => ui.set_status(message.into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_delete_entry(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            if !available(&state) {
                return;
            }
            state.clock.touch();
            if !ui.get_delete_confirm() {
                let Some(id) = selected_id(&ui, &state) else {
                    return;
                };
                state.deleting_id = id;
                let title = state
                    .session
                    .borrow()
                    .vault
                    .as_ref()
                    .and_then(|v| v.title(id).ok())
                    .unwrap_or_default();
                ui.set_delete_confirm(true);
                ui.set_status(
                    format!("再次点击确认删除「{title}」。删除前会创建加密备份。").into(),
                );
                return;
            }
            let id = state.deleting_id;
            if !authorize(&ui, &mut state, ProtectedAction::Delete(id)) {
                drop(state);
                ui.invoke_session_changed();
                ui.invoke_focus_search();
                return;
            }
            let result = (|| {
                let mut session = state.session.borrow_mut();
                session.backup_before_change()?;
                session
                    .vault
                    .as_mut()
                    .ok_or("密码库已锁定。".to_string())?
                    .delete(state.deleting_id)
            })();
            drop(state);
            match result {
                Ok(()) => {
                    success(&ui, "已删除，自动备份保留在用户数据目录。".into());
                    ui.invoke_session_changed();
                }
                Err(message) => ui.set_status(message.into()),
            }
        });
    }
    {
        let clock = clock.clone();
        ui.on_activity(move || clock.touch());
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_cancel_verification(move || {
            if let Some(ui) = weak.upgrade() {
                cancel_auth(&ui, &mut state.borrow_mut());
                ui.set_status("验证已取消，未执行操作。".into());
                ui.invoke_session_changed();
                ui.invoke_focus_search();
            }
        });
    }
    for method in 0..3 {
        let weak = ui.as_weak();
        let state = state.clone();
        let jobs = jobs.clone();
        let signal = lock_signal.clone();
        let handler = move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let state = state.borrow();
            let Some((_, generation, requested)) = state.requested else {
                return;
            };
            if state.session.borrow().working || !ui.get_verification_active() {
                return;
            }
            if requested.elapsed() >= Duration::from_secs(60)
                || generation != state.session.borrow().generation
            {
                ui.set_status("验证请求已过期，请取消后重新选择。".into());
                return;
            }
            let owner = match window_id(&ui) {
                Ok(id) if platform::is_foreground(id) => id,
                _ => {
                    ui.set_status("请回到Keyisle再验证。".into());
                    return;
                }
            };
            let master = Zeroizing::new(ui.get_verify_master().to_string());
            ui.set_verify_master("".into());
            if method == 0 && master.is_empty() {
                ui.set_status("请输入所选验证方式对应的密码。".into());
                return;
            }
            let serial = state.request_serial;
            let panel = state.panel;
            let path = state.session.borrow().path.clone();
            let title = ui.get_verification_label().to_string();
            if signal.load(Ordering::SeqCst) {
                return;
            }
            let cancellation = state.auth_cancel.clone();
            let session = state.session.clone();
            let jobs = jobs.clone();
            let signal = signal.clone();
            state.clock.touch();
            session.borrow_mut().working = true;
            drop(state);
            ui.set_status("正在验证…".into());
            ui.invoke_session_changed();
            let operation = if method == 1 {
                match platform::begin_hello(owner, &title) {
                    Ok(operation) => Some(operation),
                    Err(message) => {
                        session.borrow_mut().working = false;
                        ui.invoke_session_changed();
                        ui.set_status(message.into());
                        ui.invoke_focus_search();
                        return;
                    }
                }
            } else {
                None
            };
            let reply = move |result| {
                let _ = jobs.send(JobResult::Verify {
                    panel,
                    serial,
                    generation,
                    result,
                });
            };
            std::thread::spawn(move || {
                let result = if method == 2 {
                    crate::windows_auth::request(owner, &title, cancellation, signal)
                } else if let Some(operation) = operation {
                    platform::finish_hello(operation, cancellation, signal)
                } else {
                    Vault::verify_master(&path, &master)
                };
                reply(result);
            });
        };
        match method {
            0 => ui.on_verify_password(handler),
            1 => ui.on_verify_hello(handler),
            _ => ui.on_verify_windows(handler),
        }
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_settings(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) {
                return;
            }
            let shortcut = Shortcut::load(&state.borrow().session.borrow().root);
            match shortcut {
                Ok(shortcut) => ui.set_shortcut_input(shortcut.to_string().into()),
                Err(message) => {
                    ui.set_status(message.into());
                    return;
                }
            }
            let settings = state.borrow().clock.settings();
            let launch = {
                let state = state.borrow();
                startup::load_tray(&state.session.borrow().root)
                    .and_then(|tray| startup::enabled().map(|enabled| (tray, enabled)))
            };
            match launch {
                Ok((tray, enabled)) => {
                    ui.set_start_in_tray(tray);
                    ui.set_start_at_logon(enabled);
                }
                Err(message) => {
                    ui.set_status(message.into());
                    return;
                }
            }
            ui.set_timeout_index(
                LOCK_MINUTES
                    .iter()
                    .position(|&m| m == settings.minutes)
                    .unwrap_or(1) as i32,
            );
            ui.set_idle_source_index(if settings.source == IdleSource::Computer {
                0
            } else {
                1
            });
            ui.set_theme_choice(ui.get_theme_index());
            ui.set_appearance_choice(ui.get_appearance_mode());
            ui.set_settings_open(true);
            ui.set_status("".into());
            state.borrow().clock.touch();
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_backups(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) {
                return;
            }
            clear_maintenance(&ui);
            match refresh_backups(&ui, &mut state.borrow_mut()) {
                Ok(()) => {
                    ui.set_backups_open(true);
                    ui.set_status("".into());
                    ui.invoke_focus_search();
                }
                Err(message) => ui.set_status(message.into()),
            }
            state.borrow().clock.touch();
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_refresh_backups(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) {
                return;
            }
            let result = refresh_backups(&ui, &mut state.borrow_mut());
            ui.set_status(
                result
                    .map(|_| "备份列表已刷新。".to_string())
                    .unwrap_or_else(|e| e)
                    .into(),
            );
            state.borrow().clock.touch();
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_close_backups(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            state.borrow().auth_cancel.store(true, Ordering::SeqCst);
            clear_backup_list(&ui);
            ui.set_status("".into());
            state.borrow().clock.touch();
        });
    }
    for restore in [false, true] {
        let weak = ui.as_weak();
        let state = state.clone();
        let jobs = jobs.clone();
        let operation = move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) || !ui.get_backups_open() {
                return;
            }
            let source = usize::try_from(ui.get_selected_backup())
                .ok()
                .and_then(|index| state.borrow().backup_paths.get(index).cloned());
            let Some(source) = source else {
                ui.set_status("请先选择一份备份。".into());
                return;
            };
            let mode = if restore { 2 } else { 1 };
            if ui.get_backup_confirm_mode() != mode {
                ui.set_backup_confirm_mode(mode);
                ui.set_backup_master("".into());
                ui.set_status(
                    format!("选中：{}", source.file_name().unwrap().to_string_lossy()).into(),
                );
                ui.invoke_focus_search();
                state.borrow().clock.touch();
                return;
            }
            let master = Zeroizing::new(ui.get_backup_master().to_string());
            ui.set_backup_master("".into());
            if master.is_empty() {
                ui.set_status("请输入本次操作所需的主密码。".into());
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let (root, active, generation) = {
                let mut state = state.borrow_mut();
                state.auth_cancel = cancel.clone();
                let mut session = state.session.borrow_mut();
                session.working = true;
                (
                    session.root.clone(),
                    session.path.clone(),
                    session.generation,
                )
            };
            ui.invoke_session_changed();
            ui.set_status("正在验证并处理选中的备份…".into());
            let jobs = jobs.clone();
            std::thread::spawn(move || {
                let result = if restore {
                    maintenance::restore_backup(&root, &active, &source, &master, &cancel).map(Some)
                } else {
                    maintenance::delete_backup(&root, &active, &source, &master, &cancel)
                        .map(|_| None)
                };
                let _ = jobs.send(JobResult::Backup { generation, result });
            });
        };
        if restore {
            ui.on_restore_backup(operation);
        } else {
            ui.on_delete_backup(operation);
        }
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_maintenance(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) {
                return;
            }
            let state = state.borrow();
            let session = state.session.borrow();
            match maintenance::local_backups(&session.root, &session.path) {
                Ok(paths) => {
                    clear_maintenance(&ui);
                    ui.set_backup_count(paths.len() as i32);
                    ui.set_maintenance_open(true);
                    ui.invoke_focus_search();
                    ui.set_status("".into());
                    state.clock.touch();
                }
                Err(message) => ui.set_status(message.into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_close_maintenance(move || {
            if let Some(ui) = weak.upgrade() {
                clear_maintenance(&ui);
                ui.set_status("".into());
                state.borrow().clock.touch();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_reset(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.get_compact() || !available(&state.borrow()) {
                return;
            }
            let state = state.borrow();
            let session = state.session.borrow();
            ui.invoke_cancel_recording();
            match maintenance::reset_inventory(&session.root, &session.path) {
                Ok(files) => {
                    let count = session
                        .vault
                        .as_ref()
                        .and_then(|v| v.list().ok())
                        .map(|v| v.len())
                        .unwrap_or(0);
                    ui.set_reset_summary(
                        format!(
                            "共 {count} 个账号、{} 个软件管理文件。此操作无法撤销。",
                            files.len()
                        )
                        .into(),
                    );
                    ui.set_reset_master("".into());
                    ui.set_reset_confirmed(false);
                    ui.set_reset_incomplete(false);
                    ui.set_status("".into());
                    ui.set_context_open(false);
                    ui.set_app_menu_open(false);
                    ui.set_reset_open(true);
                    ui.invoke_focus_search();
                }
                Err(message) => ui.set_status(message.into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_close_reset(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.get_working() || ui.get_reset_incomplete() {
                return;
            }
            ui.set_reset_open(false);
            ui.set_reset_master("".into());
            ui.set_reset_confirmed(false);
            ui.set_status("".into());
            state.borrow().clock.touch();
            ui.invoke_focus_search();
        });
    }
    for clear in [false, true] {
        let weak = ui.as_weak();
        let state = state.clone();
        let jobs = jobs.clone();
        let operation = move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) || !ui.get_maintenance_open() {
                return;
            }
            if clear && !ui.get_backups_confirm() {
                ui.set_backups_confirm(true);
                ui.set_new_master("".into());
                ui.set_new_confirmation("".into());
                ui.set_status("请核对备份数量，输入当前主密码，再确认清除。".into());
                state.borrow().clock.touch();
                return;
            }
            let old = Zeroizing::new(ui.get_current_master().to_string());
            let new = Zeroizing::new(ui.get_new_master().to_string());
            let confirmation = Zeroizing::new(ui.get_new_confirmation().to_string());
            if old.is_empty() {
                ui.set_status("请输入当前主密码。".into());
                return;
            }
            if !clear {
                let validation = if new.as_str() != confirmation.as_str() {
                    Err("两次新主密码不一致，请重新输入。".to_string())
                } else {
                    maintenance::validate_change(&old, &new)
                };
                if let Err(message) = validation {
                    ui.set_status(message.into());
                    return;
                }
            }
            let count = ui.get_backup_count().max(0) as usize;
            let (root, path) = {
                let state = state.borrow();
                let session = state.session.borrow();
                (session.root.clone(), session.path.clone())
            };
            ui.invoke_lock_vault();
            let cancel = Arc::new(AtomicBool::new(false));
            {
                let mut state = state.borrow_mut();
                state.auth_cancel = cancel.clone();
                state.session.borrow_mut().working = true;
            }
            ui.invoke_session_changed();
            ui.set_status(
                if clear {
                    "正在验证主密码并清除本地备份…"
                } else {
                    "正在验证并重新加密密码库和全部本地备份；请勿关闭程序…"
                }
                .into(),
            );
            let jobs = jobs.clone();
            std::thread::spawn(move || {
                let result = if clear {
                    maintenance::clear_backups(&root, &path, &old, count, &cancel).map(|n| {
                        format!("已清除 {n} 个本地备份，当前密码库保留。请输入当前主密码解锁。")
                    })
                } else {
                    maintenance::change_master(&root, &path, &old, &new, &cancel).map(|n| {
                        format!("主密码已修改，已同步更新 {n} 个本地备份。请输入新主密码解锁。")
                    })
                };
                let _ = jobs.send(JobResult::Maintenance(result));
            });
        };
        if clear {
            ui.on_clear_backups(operation);
        } else {
            ui.on_change_master(operation);
        }
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        let timer = timer.clone();
        ui.on_fill(move |action| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut current = state.borrow_mut();
            if current.pending.is_some() {
                return;
            }
            let Some(target) = current.target else {
                return;
            };
            let Ok(index) = usize::try_from(ui.get_selected()) else {
                return;
            };
            let Some(&record) = current.matches.get(index) else {
                return;
            };
            let panel = match window_id(&ui) {
                Ok(panel) => panel,
                Err(error) => {
                    report(&ui, &mut current, error);
                    return;
                }
            };
            if !platform::is_foreground(panel) {
                report(&ui, &mut current, "搜索窗口已失去焦点，已取消填充。".into());
                return;
            }
            let mode = match action {
                FillAction::Password => FillMode::Password,
                FillAction::Username => FillMode::Username,
                FillAction::Both => FillMode::Both,
            };
            if let Err(message) = target.validate() { report(&ui, &mut current, message); return; }
            if !authorize(&ui, &mut current, ProtectedAction::Fill(record, mode)) {
                drop(current); ui.invoke_session_changed(); ui.invoke_focus_search(); return;
            }
            current.clock.touch();
            let credential = match current
                .session
                .borrow()
                .vault
                .as_ref()
                .ok_or("密码库已锁定。".to_string())
                .and_then(|v| v.get(record))
            {
                Ok(value) => value,
                Err(message) => {
                    ui.set_status(message.into());
                    return;
                }
            };
            if lock_signal.load(Ordering::SeqCst) {
                return;
            }
            let mut inputs = Zeroizing::new(
                match fill_plan(&credential.username, &credential.password, mode) {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        ui.set_status(format!("{error} 可选择其他账号或填充方式后重试。").into());
                        ui.invoke_focus_search();
                        return;
                    }
                },
            );
            let password = Zeroizing::new(
                if let Some(tab) = inputs
                    .iter()
                    .position(|unit| matches!(unit, InputUnit::Tab))
                {
                    inputs.split_off(tab + 1)
                } else {
                    Vec::new()
                },
            );
            current.pending = Some(Pending {
                target,
                inputs,
                password,
                tab_sent: None,
                panel,
                started: Instant::now(),
                restored: None,
                record,
                application: current.target_application.clone(),
            });
            ui.set_busy(true);
            ui.set_status("请松开快捷键、Enter 和鼠标按键…".into());
            drop(current);
            let weak = weak.clone();
            let state = state.clone();
            let lock_signal = lock_signal.clone();
            let session = current_session(&state);
            let timer_weak = Rc::downgrade(&timer);
            timer.start(TimerMode::Repeated, Duration::from_millis(20), move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                let mut state = state.borrow_mut();
                if lock_signal.load(Ordering::SeqCst) || session.borrow().vault.is_none() {
                    state.pending = None;
                    state.target = None;
                    ui.set_busy(false);
                    ui.set_can_fill(false);
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                }
                let Some(pending) = state.pending.as_mut() else {
                    return;
                };
                // shortcut: shared HWNDs expose no field readiness; use 400 ms, add UIA checks if slower clients require them.
                let outcome = if let Some(tab_sent) = pending.tab_sent {
                    match pending.target.verify_foreground() {
                        Err(error) => Some(Err(error)),
                        Ok(()) if tab_sent.elapsed() < Duration::from_millis(400) => None,
                        Ok(()) => match pending.target.after_tab() {
                            Ok(Some(target)) => Some(target.fill(&pending.password)),
                            Ok(None) if tab_sent.elapsed() < Duration::from_millis(1500) => None,
                            Ok(None) => Some(Err("Tab 未及时切换到下一输入框，已取消密码填充。请点击密码框后单独填入。".into())),
                            Err(message) => Some(Err(message)),
                        },
                    }
                } else if let Some(restored) = pending.restored {
                    if restored.elapsed() < Duration::from_millis(120) {
                        return;
                    }
                    match pending.target.fill(&pending.inputs) {
                        Ok(()) if !pending.password.is_empty() => {
                            // Tab focus changes can be asynchronous; aTrust drops an immediate password batch.
                            pending.tab_sent = Some(Instant::now());
                            None
                        }
                        result => Some(result),
                    }
                } else if !platform::is_foreground(pending.panel) {
                    Some(Err("等待期间窗口已切换，已取消填充。".into()))
                } else if pending.started.elapsed() > Duration::from_secs(2) {
                    Some(Err("按键未及时释放，已取消填充。".into()))
                } else if platform::trigger_keys_released() {
                    // Release Enter while the panel still owns focus, so the target cannot submit on key-up.
                    match hide_window(&ui).and_then(|_| pending.target.restore())
                    {
                        Ok(()) => {
                            pending.restored = Some(Instant::now());
                            None
                        }
                        Err(error) => Some(Err(error)),
                    }
                } else {
                    None
                };
                if let Some(result) = outcome {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    if result.is_ok() {
                        let record = state.pending.as_ref().unwrap().record;
                        let mut session = session.borrow_mut();
                        if session.vault.as_mut().is_some_and(|vault| vault.record_used_in_application(record, &state.pending.as_ref().unwrap().application).is_err()) {
                            session.notice=Some("上次填充已完成，但最近使用记录未保存，请检查磁盘空间或权限。".into());
                        }
                    }
                    state.pending = None;
                    state.target = None;
                    ui.set_busy(false);
                    ui.set_can_fill(false);
                    if let Err(error) = result {
                        report(&ui, &mut state, error);
                    }
                }
            });
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_request_context(move |x, y| {
            let Some(ui) = weak.upgrade() else {
                return false;
            };
            if !available(&state.borrow())
                || ui.get_editing()
                || ui.get_settings_open()
                || ui.get_maintenance_open()
                || ui.get_backups_open()
                || ui.get_verification_active()
            {
                ui.set_context_open(false);
                return false;
            }
            let left = ui.get_list_left();
            let top = ui.get_list_top();
            if x < left
                || x > left + ui.get_list_width()
                || y < top
                || y > top + ui.get_list_height()
            {
                ui.set_context_open(false);
                return false;
            }
            let relative = y - top - ui.get_list_scroll();
            let index = (relative / 64.0).floor() as usize;
            let mut state = state.borrow_mut();
            let record = if relative >= 0.0
                && relative % 64.0 < if ui.get_compact() { 56.0 } else { 60.0 }
            {
                state.matches.get(index).copied()
            } else {
                None
            };
            state.context_record = record.map(|id| (id, state.session.borrow().generation));
            if record.is_some() {
                ui.set_selected(index as i32);
            }
            ui.set_delete_confirm(false);
            ui.set_context_has_record(record.is_some());
            ui.set_context_x(x);
            ui.set_context_y(y);
            ui.set_context_index(if record.is_some() { 0 } else { 2 });
            ui.set_context_open(true);
            ui.invoke_focus_context();
            state.clock.touch();
            true
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_context_action(move |action| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.get_context_open() || !available(&state.borrow()) {
                return;
            }
            ui.set_context_open(false);
            ui.set_delete_confirm(false);
            if action == 2 {
                if ui.get_compact() {
                    ui.invoke_manage_context(action);
                    return;
                }
                ui.invoke_new_entry();
                return;
            }
            let current = state.borrow();
            let Some((id, generation)) = current.context_record else {
                return;
            };
            if generation != current.session.borrow().generation {
                return;
            }
            let Some(index) = current.matches.iter().position(|r| *r == id) else {
                return;
            };
            drop(current);
            ui.set_selected(index as i32);
            if ui.get_compact() {
                ui.invoke_manage_context(action);
                return;
            }
            if action == 0 {
                ui.invoke_edit_entry();
            } else if action == 1 {
                ui.invoke_delete_entry();
            } else if action == 3 {
                ui.invoke_view_entry();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_appearance_changed(move || {
            // Palette bindings can be evaluated while a list refresh holds the state.
            let weak = weak.clone();
            let state = state.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    refresh(&ui, &mut state.borrow_mut());
                }
            });
        });
    }
    refresh(ui, &mut state.borrow_mut());
    (state, timer)
}

fn show_panel(
    ui: &AppWindow,
    state: &RefCell<State>,
    timer: &Timer,
    target: Option<Target>,
) -> Result<(), String> {
    timer.stop();
    let mut state = state.borrow_mut();
    cancel_auth(ui, &mut state);
    let reopening = ui.get_compact() && ui.window().is_visible();
    let target = target.or_else(|| {
        if reopening {
            state.target.filter(|target| target.validate().is_ok())
        } else {
            None
        }
    });
    if !reopening {
        state.matches.clear();
    }
    state.clock.touch();
    state.pending = None;
    state.target = target;
    state.target_application = target
        .and_then(|target| target.application_key())
        .unwrap_or_default();
    if !ui.get_compact() {
        ui.set_about_open(false);
        clear_details(ui);
        state.details_id = None;
    }
    ui.set_busy(false);
    ui.set_can_fill(target.is_some() && state.session.borrow().vault.is_some());
    ui.set_target_label(
        target
            .map(|t| t.description())
            .unwrap_or_else(|| format!("先点击外部输入框，再按 {}。", ui.get_shortcut_label()))
            .into(),
    );
    ui.set_target_detail(
        target
            .map(|t| t.location_description())
            .unwrap_or_default()
            .into(),
    );
    ui.set_status("".into());
    if !reopening {
        ui.set_query("".into());
        if ui.get_compact() {
            ui.set_category("".into());
            ui.set_tag("".into());
        }
    }
    refresh(ui, &mut state);
    ui.invoke_scroll_selected();
    if let Some(notice) = state.session.borrow_mut().notice.take() {
        ui.set_status(notice.into());
    }
    if !ui.window().is_visible() {
        show_window(ui)?;
    }
    if !ui.get_compact() {
        ui.invoke_restore_layout();
    }
    if ui.get_compact() {
        ui.window().set_minimized(false);
        platform::present_popup(window_id(ui)?, target)?;
    } else {
        // A rapid restore can be overtaken by the pending native minimize command.
        let delay = if ui.window().is_minimized() {
            state
                .minimized_at
                .map(|at| Duration::from_millis(1500).saturating_sub(at.elapsed()))
                .unwrap_or_default()
        } else {
            Duration::ZERO
        };
        let weak = ui.as_weak();
        Timer::single_shot(delay, move || {
            if let Some(ui) = weak.upgrade() {
                if ui.window().is_visible() {
                    ui.window().with_winit_window(|window| {
                        window.set_minimized(false);
                    });
                    ui.window().set_minimized(false);
                    if ui.get_fixed_page() {
                        ui.invoke_resize_for_page();
                    }
                    if let Ok(window) = window_id(&ui) {
                        platform::activate(window);
                    }
                    ui.invoke_focus_search();
                }
            }
        });
    }
    ui.invoke_focus_search();
    Ok(())
}

fn state_index(state: &RefCell<State>, id: i64) -> Option<usize> {
    state.borrow().matches.iter().position(|&item| item == id)
}

fn current_session(state: &RefCell<State>) -> Rc<RefCell<Session>> {
    state.borrow().session.clone()
}

pub fn run() -> Result<(), String> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attributes| attributes.with_transparent(false))
        .select()
        .map_err(|e| e.to_string())?;
    let root = if let Some(path) = std::env::var_os("PASSWORD_MANAGER_DATA_DIR") {
        PathBuf::from(path)
    } else {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("无法定位当前用户的数据目录。")?)
            .join("PasswordHelper")
    };
    let reset_boot = maintenance::reset_pending(&root);
    let session = Rc::new(RefCell::new(Session::new(root.clone())?));
    let clock = Arc::new(LockClock::new(if reset_boot {
        LockSettings::default()
    } else {
        LockSettings::load(&root)?
    }));
    let geometry_cache = Rc::new(RefCell::new(if reset_boot {
        None
    } else {
        match windows_password_manager::settings::WindowLayout::load(&root) {
            Ok(layout) => layout,
            Err(message) => {
                session.borrow_mut().notice = Some(message);
                None
            }
        }
    }));
    let geometry_timer = Rc::new(Timer::default());
    let geometry_paused = Rc::new(Cell::new(false));
    let geometry_epoch = Rc::new(Cell::new(0u64));
    let geometry_mode = Rc::new(Cell::new(None));
    let resetting = Rc::new(Cell::new(reset_boot));
    let restart_after_reset = Rc::new(Cell::new(false));
    let main = AppWindow::new().map_err(|e| e.to_string())?;
    let popup = AppWindow::new().map_err(|e| e.to_string())?;
    popup.set_compact(true);
    let shortcut = if reset_boot {
        Shortcut::default()
    } else {
        Shortcut::load(&root)?
    };
    let theme = if reset_boot {
        0
    } else {
        windows_password_manager::settings::load_theme(&root)?
    };
    let appearance = if reset_boot {
        0
    } else {
        windows_password_manager::settings::load_appearance(&root)?
    };
    if let Some(layout) = *geometry_cache.borrow() {
        main.set_desktop_width(layout.width as f32);
        main.set_desktop_height(layout.height as f32);
    }
    for ui in [&main, &popup] {
        ui.set_theme_index(theme);
        ui.set_system_dark(platform::system_dark());
        ui.set_appearance_mode(appearance);
        ui.invoke_apply_appearance();
        let weak = ui.as_weak();
        let cache = geometry_cache.clone();
        let paused = geometry_paused.clone();
        let epoch = geometry_epoch.clone();
        let mode = geometry_mode.clone();
        let disabled = resetting.clone();
        ui.on_restore_layout(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.get_compact()
                || disabled.get()
                || !ui.window().is_visible()
                || ui.window().is_minimized()
            {
                return;
            }
            let fixed = ui.get_fixed_page();
            let old = mode.replace(Some(fixed));
            if old.is_some() && (fixed || old == Some(false)) {
                return;
            }
            paused.set(true);
            epoch.set(epoch.get().wrapping_add(1));
            let serial = epoch.get();
            let weak = ui.as_weak();
            let cache = cache.clone();
            let paused = paused.clone();
            let epoch = epoch.clone();
            let disabled = disabled.clone();
            let mode = mode.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    if epoch.get() != serial {
                        return;
                    }
                    if !ui.window().is_visible() || disabled.get() {
                        mode.set(old);
                        paused.set(false);
                        return;
                    }
                    if let Some(layout) = *cache.borrow() {
                        if let Ok(id) = window_id(&ui) {
                            if let Err(message) = platform::restore_window_layout(id, layout, fixed)
                            {
                                ui.set_status(message.into());
                            }
                            if !fixed {
                                ui.window().set_size(slint::LogicalSize::new(
                                    layout.width as f32,
                                    layout.height as f32,
                                ));
                                ui.window().set_maximized(layout.maximized);
                            }
                        }
                    }
                    let weak = ui.as_weak();
                    Timer::single_shot(Duration::from_millis(200), move || {
                        if epoch.get() == serial && !disabled.get() {
                            paused.set(false);
                            if let Some(ui) = weak.upgrade() {
                                if !ui.get_fixed_page() {
                                    if let Ok(id) = window_id(&ui) {
                                        if let Some(layout) = platform::window_layout(
                                            id,
                                            ui.window().scale_factor() as f64,
                                        ) {
                                            *cache.borrow_mut() = Some(layout);
                                        }
                                    }
                                }
                            }
                        }
                    });
                }
            });
        });
        let weak = ui.as_weak();
        ui.on_resize_for_page(move || {
            let weak = weak.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    if ui.window().is_minimized() {
                        return;
                    }
                    if ui.get_fixed_page() {
                        ui.window().set_maximized(false);
                    }
                    ui.invoke_restore_layout();
                    let size = ui.window().size().to_logical(ui.window().scale_factor());
                    let (width, height) = (ui.get_page_width(), ui.get_page_height());
                    if (size.width - width).abs() > 1.0 || (size.height - height).abs() > 1.0 {
                        ui.window().set_size(slint::LogicalSize::new(width, height));
                    }
                    if ui.window().is_visible()
                        && !ui.get_editing()
                        && !ui.get_settings_open()
                        && !ui.get_backups_open()
                        && !ui.get_maintenance_open()
                    {
                        if let Ok(window) = window_id(&ui) {
                            if platform::is_foreground(window) {
                                ui.invoke_focus_search();
                            }
                        }
                    }
                }
            });
        });
        let weak = ui.as_weak();
        ui.on_drag_window(move || {
            let weak = weak.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    ui.window().with_winit_window(|window| {
                        let _ = window.drag_window();
                    });
                }
            });
        });
        let weak = ui.as_weak();
        ui.on_maximize_window(move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().set_maximized(!ui.window().is_maximized());
            }
        });
        let weak = ui.as_weak();
        ui.on_update_frame(move || {
            if let Some(ui) = weak.upgrade() {
                if let Ok(id) = window_id(&ui) {
                    if let Err(message) = platform::configure_frameless_window(
                        id,
                        !ui.get_fixed_page() && !ui.get_discard_open(),
                        ui.window().scale_factor() as f64,
                    ) {
                        ui.set_status(message.into());
                    }
                }
            }
        });
        let weak = ui.as_weak();
        ui.on_system_menu(move || {
            let weak = weak.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    if !ui.get_discard_open() {
                        if let Ok(id) = window_id(&ui) {
                            platform::system_menu(id);
                        }
                    }
                }
            });
        });
        ui.set_shortcut_label(shortcut.to_string().into());
        let weak = ui.as_weak();
        let modifiers = Cell::new(slint::winit_030::winit::keyboard::ModifiersState::empty());
        let rounded_bounds = Cell::new(None);
        let layout_cache = geometry_cache.clone();
        let layout_timer = geometry_timer.clone();
        let layout_paused = geometry_paused.clone();
        let layout_mode = geometry_mode.clone();
        let layout_disabled = resetting.clone();
        let layout_root = root.clone();
        ui.window().on_winit_window_event(move |window, event| {
            use slint::winit_030::winit::{
                event::ElementState,
                keyboard::{KeyCode, PhysicalKey},
            };
            if let WindowEvent::ModifiersChanged(value) = event {
                modifiers.set(value.state());
            }
            if let Some(ui) = weak.upgrade() {
                if matches!(event, WindowEvent::ThemeChanged(_)) {
                    ui.set_system_dark(platform::system_dark());
                    ui.invoke_apply_appearance();
                }
                if !ui.get_compact()
                    && !ui.get_fixed_page()
                    && !layout_paused.get()
                    && !layout_disabled.get()
                    && layout_mode.get() == Some(false)
                    && matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_))
                {
                    if let Ok(id) = window_id(&ui) {
                        if let Some(layout) =
                            platform::window_layout(id, window.scale_factor() as f64)
                        {
                            *layout_cache.borrow_mut() = Some(layout);
                            let cache = layout_cache.clone();
                            let disabled = layout_disabled.clone();
                            let root = layout_root.clone();
                            let weak = ui.as_weak();
                            layout_timer.start(
                                TimerMode::SingleShot,
                                Duration::from_millis(300),
                                move || {
                                    if !disabled.get() && !maintenance::reset_pending(&root) {
                                        if let Some(layout) = *cache.borrow() {
                                            if let Err(message) = layout.save(&root) {
                                                if let Some(ui) = weak.upgrade() {
                                                    ui.set_status(message.into());
                                                }
                                            }
                                        }
                                    }
                                },
                            );
                        }
                    }
                }
                if ui.get_compact() && !ui.get_busy() && !ui.get_working() {
                    if let WindowEvent::KeyboardInput { event, .. } = event {
                        let m = modifiers.get();
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && event.physical_key == PhysicalKey::Code(KeyCode::KeyM)
                            && m.control_key()
                            && !m.alt_key()
                            && m.shift_key()
                            && !m.super_key()
                        {
                            let target = ui.as_weak();
                            Timer::single_shot(Duration::ZERO, move || {
                                if let Some(ui) = target.upgrade() {
                                    ui.invoke_open_management();
                                }
                            });
                            return EventResult::PreventDefault;
                        }
                    }
                }
                if ui.get_recording_shortcut() {
                    if matches!(event, WindowEvent::Focused(false)) {
                        ui.invoke_cancel_recording();
                    }
                    if let WindowEvent::KeyboardInput { event, .. } = event {
                        if event.state == ElementState::Pressed && !event.repeat {
                            if event.physical_key == PhysicalKey::Code(KeyCode::Escape) {
                                ui.invoke_cancel_recording();
                            } else if let PhysicalKey::Code(code) = event.physical_key {
                                if let Some(key) = platform::shortcut_key(code) {
                                    let m = modifiers.get();
                                    let flags = (m.alt_key() as i32)
                                        | ((m.control_key() as i32) * 2)
                                        | ((m.shift_key() as i32) * 4)
                                        | ((m.super_key() as i32) * 8);
                                    ui.invoke_captured_shortcut(key as i32, flags);
                                }
                            }
                        }
                        return EventResult::PreventDefault;
                    }
                }
            }

            if matches!(
                event,
                WindowEvent::Focused(true) | WindowEvent::Occluded(false) | WindowEvent::Resized(_)
            ) {
                if let Some(ui) = weak.upgrade() {
                    if matches!(
                        event,
                        WindowEvent::Focused(true) | WindowEvent::Occluded(false)
                    ) {
                        ui.set_system_dark(platform::system_dark());
                        ui.invoke_apply_appearance();
                    }
                    if let Ok(id) = window_id(&ui) {
                        if let Err(message) = platform::configure_frameless_window(
                            id,
                            !ui.get_fixed_page() && !ui.get_discard_open(),
                            window.scale_factor() as f64,
                        ) {
                            ui.set_status(message.into());
                        }
                        if !ui.get_compact() {
                            platform::fit_window_to_work_area(id);
                        }
                    }
                    if !ui.get_fixed_page() && matches!(event, WindowEvent::Resized(_)) {
                        if let Some((size, maximized, minimized)) =
                            window.with_winit_window(|native| {
                                (
                                    native.inner_size(),
                                    native.is_maximized(),
                                    native.is_minimized().unwrap_or(false),
                                )
                            })
                        {
                            if !maximized && !minimized && size.width > 0 && size.height > 0 {
                                let size = size.to_logical::<f32>(window.scale_factor() as f64);
                                ui.set_desktop_width(size.width.max(ui.get_desktop_min_width()));
                                ui.set_desktop_height(size.height.max(ui.get_desktop_min_height()));
                            }
                        }
                    }
                    if ui.get_fixed_page() && matches!(event, WindowEvent::Resized(_)) {
                        ui.invoke_resize_for_page();
                    }
                    ui.set_is_maximized(
                        window
                            .with_winit_window(|native| native.is_maximized())
                            .unwrap_or(false),
                    );
                    if let Some(bounds) = window.with_winit_window(|native| {
                        let size = native.inner_size();
                        (
                            size.width,
                            size.height,
                            native.is_maximized(),
                            native.scale_factor().to_bits(),
                        )
                    }) {
                        if bounds.0 > 0 && bounds.1 > 0 && rounded_bounds.get() != Some(bounds) {
                            rounded_bounds.set(Some(bounds));
                            if let Ok(id) = window_id(&ui) {
                                platform::round_window(
                                    id,
                                    ui.get_window_radius(),
                                    window.scale_factor() as f64,
                                );
                            }
                        }
                    }
                    ui.set_repaint_frame(!ui.get_repaint_frame());
                }
                window.request_redraw();
                if matches!(event, WindowEvent::Focused(true)) {
                    let weak = weak.clone();
                    Timer::single_shot(Duration::ZERO, move || {
                        if let Some(ui) = weak.upgrade() {
                            if ui.get_compact() && !ui.get_busy() {
                                ui.invoke_focus_search();
                            }
                        }
                    });
                }
            }
            EventResult::Propagate
        });
    }
    let lock_signal = Arc::new(AtomicBool::new(false));
    let (jobs, results) = mpsc::channel::<JobResult>();
    let (main_state, main_timer) = bind(
        &main,
        session.clone(),
        jobs.clone(),
        clock.clone(),
        0,
        lock_signal.clone(),
    );
    let (popup_state, popup_timer) = bind(
        &popup,
        session.clone(),
        jobs.clone(),
        clock.clone(),
        1,
        lock_signal.clone(),
    );
    for ui in [&main, &popup] {
        {
            let main_weak = main.as_weak();
            let popup_weak = popup.as_weak();
            let main_state = main_state.clone();
            let popup_state = popup_state.clone();
            ui.on_session_changed(move || {
                if let (Some(main), Some(popup)) = (main_weak.upgrade(), popup_weak.upgrade()) {
                    refresh(&main, &mut main_state.borrow_mut());
                    refresh(&popup, &mut popup_state.borrow_mut());
                    popup.set_can_fill(
                        popup_state.borrow().target.is_some() && popup.get_unlocked(),
                    );
                }
            });
        }
        {
            let session = session.clone();
            let main_weak = main.as_weak();
            let popup_weak = popup.as_weak();
            let main_state = main_state.clone();
            let popup_state = popup_state.clone();
            let main_timer = main_timer.clone();
            let popup_timer = popup_timer.clone();
            ui.on_lock_vault(move || {
                session.borrow_mut().lock();
                for (weak, state, timer) in [
                    (&main_weak, &main_state, &main_timer),
                    (&popup_weak, &popup_state, &popup_timer),
                ] {
                    if let Some(ui) = weak.upgrade() {
                        timer.stop();
                        let mut state = state.borrow_mut();
                        cancel_auth(&ui, &mut state);
                        state.editor_verified = None;
                        ui.set_settings_open(false);
                        ui.set_about_open(false);
                        ui.set_reset_master("".into());
                        if !ui.get_working() && !ui.get_reset_incomplete() {
                            ui.set_reset_open(false);
                            ui.set_reset_confirmed(false);
                        }
                        clear_maintenance(&ui);
                        state.pending = None;
                        state.target = None;
                        ui.set_query("".into());
                        ui.set_category("".into());
                        ui.set_tag("".into());
                        ui.set_master("".into());
                        ui.set_confirmation("".into());
                        ui.set_busy(false);
                        ui.set_can_fill(false);
                        ui.set_target_label("".into());
                        ui.set_target_detail("".into());
                        ui.set_status("密码库已锁定。".into());
                        clear_editor(&ui);
                        clear_details(&ui);
                        state.details_id = None;
                        state.target_application.clear();
                        refresh(&ui, &mut state);
                    }
                }
                if let Some(popup) = popup_weak.upgrade() {
                    let _ = hide_window(&popup);
                }
            });
        }
        {
            let weak = ui.as_weak();
            let main_weak = main.as_weak();
            let popup_weak = popup.as_weak();
            let session = session.clone();
            let popup_state = popup_state.clone();
            let popup_timer = popup_timer.clone();
            ui.on_restore(move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                if session.borrow().working || session.borrow().auth_owner.is_some() {
                    return;
                }
                if let Some(popup) = popup_weak.upgrade() {
                    cancel(&popup, &mut popup_state.borrow_mut(), &popup_timer);
                }
                let result = (|| {
                    let selected = platform::choose_file(window_id(&ui)?, false)?;
                    if let Some(source) = selected {
                        session.borrow_mut().restore_copy(&source)?;
                        ui.invoke_lock_vault();
                        ui.invoke_session_changed();
                        if let Some(main) = main_weak.upgrade() {
                            main.set_status(
                                "已创建恢复副本。请输入备份的主密码；原密码库保持不变。".into(),
                            );
                            main.show().map_err(|e| e.to_string())?;
                            platform::activate(window_id(&main)?);
                            main.invoke_focus_search();
                        }
                    }
                    Ok::<_, String>(())
                })();
                if let Err(message) = result {
                    ui.set_status(message.into());
                }
            });
        }
    }
    {
        let weak = main.as_weak();
        let session = session.clone();
        let popup_weak = popup.as_weak();
        let popup_state = popup_state.clone();
        let popup_timer = popup_timer.clone();
        main.on_backup(move || {
            if session.borrow().working || session.borrow().auth_owner.is_some() {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Some(popup) = popup_weak.upgrade() {
                cancel(&popup, &mut popup_state.borrow_mut(), &popup_timer);
            }
            let result = (|| {
                if let Some(path) = platform::choose_file(window_id(&ui)?, true)? {
                    session
                        .borrow_mut()
                        .vault
                        .as_mut()
                        .ok_or("密码库已锁定。".to_string())?
                        .backup(&path)?;
                    success(&ui, format!("已保存加密备份：{}", path.display()));
                }
                Ok::<_, String>(())
            })();
            if let Err(message) = result {
                ui.set_status(message.into());
            }
        });
    }
    let mailbox = Arc::new(Mutex::new(VecDeque::<Event>::new()));
    let weak = main.as_weak();
    let incoming = mailbox.clone();
    let signal = lock_signal.clone();
    let native = Native::start(clock.clone(), session.borrow().root.clone(), move |event| {
        if matches!(event, Event::Lock) {
            signal.store(true, Ordering::SeqCst);
        }
        incoming.lock().unwrap().push_back(event);
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.invoke_native_open();
        });
    })?;
    let Some(native) = native else {
        return Ok(());
    };
    let native = Rc::new(native);
    if !reset_boot {
        if let Err(message) = startup::relocate_registration() {
            session.borrow_mut().notice.get_or_insert(message);
        }
    }
    if reset_boot {
        maintenance::validate_reset(&root)?;
        startup::remove_registration()?;
        maintenance::finish_reset(&root)?;
        session.borrow_mut().path = root.join("vault.db");
        session.borrow_mut().notice = Some("已完成上次中断的还原。请创建新的密码库。".into());
        resetting.set(false);
    }
    {
        let weak = main.as_weak();
        let session = session.clone();
        let state = main_state.clone();
        let jobs = jobs.clone();
        let resetting = resetting.clone();
        let geometry_timer = geometry_timer.clone();
        main.on_reset_profile(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.get_reset_open()
                || session.borrow().working
                || session.borrow().auth_owner.is_some()
            {
                return;
            }
            let resume = ui.get_reset_incomplete();
            if !resume && (!ui.get_reset_confirmed() || ui.get_reset_master().is_empty()) {
                return;
            }
            let master = Zeroizing::new(ui.get_reset_master().to_string());
            ui.set_reset_master("".into());
            let root = session.borrow().root.clone();
            let active = session.borrow().path.clone();
            ui.invoke_lock_vault();
            let cancel = Arc::new(AtomicBool::new(false));
            state.borrow_mut().auth_cancel = cancel.clone();
            session.borrow_mut().working = true;
            resetting.set(true);
            geometry_timer.stop();
            ui.set_reset_open(true);
            ui.set_reset_incomplete(resume);
            ui.invoke_session_changed();
            ui.set_status("正在验证并还原；完成后软件将重新启动…".into());
            ui.invoke_focus_search();
            let jobs = jobs.clone();
            std::thread::spawn(move || {
                let result = (|| {
                    if !resume {
                        maintenance::begin_reset(&root, &active, &master, &cancel)?;
                    }
                    maintenance::validate_reset(&root)?;
                    startup::remove_registration()?;
                    maintenance::finish_reset(&root)
                })();
                let _ = jobs.send(JobResult::Reset(result));
            });
        });
    }

    let resumed_master_change = maintenance::recover(&session.borrow().root)?;
    let active_recording = Rc::new(RefCell::new(None::<crate::recording::Recorder>));
    let recording_generation = Arc::new(AtomicU64::new(0));
    for ui in [&main, &popup] {
        let weak = ui.as_weak();
        let recorder = native.clone();
        let active_capture = active_recording.clone();
        let capture_epoch = recording_generation.clone();
        ui.on_start_recording(move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_blocked() || !ui.get_settings_open() {
                    return;
                }
                let owner = match window_id(&ui) {
                    Ok(owner) => owner,
                    Err(message) => {
                        ui.set_status(message.into());
                        return;
                    }
                };
                let generation = capture_epoch.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
                let guard = capture_epoch.clone();
                let weak = ui.as_weak();
                ui.set_shortcut_before_record(ui.get_shortcut_input());
                ui.set_recording_shortcut(true);
                recorder.record_shortcut(true);
                match crate::recording::Recorder::start(owner, move |shortcut| {
                    let guard = guard.clone();
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        if guard.load(Ordering::SeqCst) == generation && ui.get_recording_shortcut()
                        {
                            match shortcut {
                                Ok(shortcut) => ui.invoke_captured_shortcut(
                                    shortcut.key as i32,
                                    shortcut.modifiers as i32,
                                ),
                                Err(message) => {
                                    ui.invoke_cancel_recording();
                                    ui.set_status(message.into());
                                }
                            }
                        }
                    });
                }) {
                    Ok(capture) => {
                        *active_capture.borrow_mut() = Some(capture);
                        ui.set_status("请按快捷键组合；Esc 取消。已占用的组合也可录入。".into());
                    }
                    Err(message) => {
                        ui.set_recording_shortcut(false);
                        recorder.record_shortcut(false);
                        ui.set_status(message.into());
                    }
                }
            }
        });
        let weak = ui.as_weak();
        let recorder = native.clone();
        let active_capture = active_recording.clone();
        let capture_epoch = recording_generation.clone();
        ui.on_cancel_recording(move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_recording_shortcut() {
                    ui.set_recording_shortcut(false);
                    capture_epoch.fetch_add(1, Ordering::SeqCst);
                    active_capture.borrow_mut().take();
                    ui.set_shortcut_input(ui.get_shortcut_before_record());
                    recorder.record_shortcut(false);
                    ui.set_status("录制已取消，原快捷键仍有效。".into());
                }
            }
        });
        let weak = ui.as_weak();
        let recorder = native.clone();
        let active_capture = active_recording.clone();
        let capture_epoch = recording_generation.clone();
        ui.on_captured_shortcut(move |key, modifiers| {
            if let Some(ui) = weak.upgrade() {
                if !ui.get_recording_shortcut() {
                    return;
                }
                let shortcut = Shortcut {
                    key: key as u32,
                    modifiers: modifiers as u32,
                };
                if !shortcut.valid() {
                    ui.set_status("请使用 Ctrl 或 Alt 加字母、数字或 F1～F24。".into());
                    return;
                }
                ui.set_shortcut_input(shortcut.to_string().into());
                ui.set_recording_shortcut(false);
                capture_epoch.fetch_add(1, Ordering::SeqCst);
                active_capture.borrow_mut().take();
                recorder.record_shortcut(false);
                ui.set_status("已识别组合键，点击保存设置后生效。".into());
            }
        });
        let weak = ui.as_weak();
        let recorder = native.clone();
        let session = session.clone();
        let clock = clock.clone();
        let main_weak = main.as_weak();
        let popup_weak = popup.as_weak();
        ui.on_save_settings(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if session.borrow().vault.is_none()
                || session.borrow().working
                || session.borrow().auth_owner.is_some()
                || ui.get_recording_shortcut()
            {
                return;
            }
            let root = session.borrow().root.clone();
            let result = (|| {
                let shortcut = ui.get_shortcut_input().as_str().parse::<Shortcut>()?;
                let minutes = *LOCK_MINUTES
                    .get(usize::try_from(ui.get_timeout_index()).map_err(|_| "锁定时间无效。")?)
                    .ok_or("锁定时间无效。")?;
                let source = match ui.get_idle_source_index() {
                    0 => IdleSource::Computer,
                    1 => IdleSource::Vault,
                    _ => return Err("计时方式无效。".into()),
                };
                let settings = LockSettings { minutes, source };
                let snapshot = windows_password_manager::settings::Snapshot::capture(&root)?;
                recorder.prepare_shortcut(shortcut)?;
                let write = (|| {
                    shortcut.save(&root)?;
                    settings.save(&root)?;
                    windows_password_manager::settings::save_theme(&root, ui.get_theme_choice())?;
                    windows_password_manager::settings::save_appearance(
                        &root,
                        ui.get_appearance_choice(),
                    )?;
                    startup::save(&root, ui.get_start_at_logon(), ui.get_start_in_tray())
                })();
                if let Err(message) = write {
                    recorder.abort_shortcut();
                    return Err(match snapshot.restore() {
                        Ok(()) => format!("{message} 原设置已恢复。"),
                        Err(rollback) => format!("{message} {rollback}"),
                    });
                }
                recorder.commit_shortcut();
                clock.update(settings);
                for weak in [&main_weak, &popup_weak] {
                    if let Some(window) = weak.upgrade() {
                        window.set_shortcut_label(shortcut.to_string().into());
                        window.set_theme_index(ui.get_theme_choice());
                        window.set_appearance_mode(ui.get_appearance_choice());
                        window.invoke_apply_appearance();
                    }
                }
                Ok::<_, String>(())
            })();
            ui.invoke_session_changed();
            match result {
                Ok(()) => success(
                    &ui,
                    "全部设置已保存，配色、快捷键和锁定规则立即生效。".into(),
                ),
                Err(message) => ui.set_status(message.into()),
            }
        });
    }

    {
        let main_weak = main.as_weak();
        let popup_weak = popup.as_weak();
        let main_state = main_state.clone();
        let main_timer = main_timer.clone();
        let popup_state = popup_state.clone();
        let popup_timer = popup_timer.clone();
        let signal = lock_signal.clone();
        let navigate_main = main.as_weak();
        let navigate_popup = popup.as_weak();
        let navigate_main_state = main_state.clone();
        let navigate_popup_state = popup_state.clone();
        let navigate_main_timer = main_timer.clone();
        let navigate_popup_timer = popup_timer.clone();
        let context_main = main.as_weak();
        let context_popup = popup.as_weak();
        let context_main_state = main_state.clone();
        let context_popup_state = popup_state.clone();
        let context_main_timer = main_timer.clone();
        let context_popup_timer = popup_timer.clone();
        popup.on_manage_context(move |action| {
            let (Some(main), Some(popup)) = (context_main.upgrade(), context_popup.upgrade())
            else {
                return;
            };
            let record = selected_id(&popup, &context_popup_state.borrow());
            cancel(
                &popup,
                &mut context_popup_state.borrow_mut(),
                &context_popup_timer,
            );
            if let Err(message) = show_panel(&main, &context_main_state, &context_main_timer, None)
            {
                main.set_status(message.into());
                return;
            }
            if main.get_editing() || main.get_verification_active() || main.get_blocked() {
                main.set_status("请先完成主界面当前的编辑或验证，再操作账号。".into());
                return;
            }
            main.set_settings_open(false);
            clear_maintenance(&main);
            clear_backup_list(&main);
            main.set_query("".into());
            main.set_category("".into());
            main.set_tag("".into());
            refresh(&main, &mut context_main_state.borrow_mut());
            if action == 2 {
                main.invoke_new_entry();
                return;
            }
            if let Some(index) = record.and_then(|id| state_index(&context_main_state, id)) {
                main.set_selected(index as i32);
                main.invoke_scroll_selected();
                if action == 0 {
                    main.invoke_edit_entry();
                } else if action == 1 {
                    main.invoke_delete_entry();
                } else if action == 3 {
                    main.invoke_view_entry();
                }
            }
        });
        popup.on_open_management(move || {
            if let (Some(main), Some(popup)) = (navigate_main.upgrade(), navigate_popup.upgrade()) {
                cancel(
                    &popup,
                    &mut navigate_popup_state.borrow_mut(),
                    &navigate_popup_timer,
                );
                if let Err(message) =
                    show_panel(&main, &navigate_main_state, &navigate_main_timer, None)
                {
                    main.set_status(message.into());
                }
            }
        });
        let reset_events = resetting.clone();
        main.on_native_open(move || {
            let (Some(main), Some(popup)) = (main_weak.upgrade(), popup_weak.upgrade()) else {
                return;
            };
            let events: Vec<_> = mailbox.lock().unwrap().drain(..).collect();
            for event in events {
                if (reset_events.get() || main.get_reset_open())
                    && matches!(
                        event,
                        Event::Search(_) | Event::Manage | Event::ShortcutRecorded(_)
                    )
                {
                    continue;
                }
                let result = match event {
                    Event::DismissPopup => {
                        if popup.window().is_visible() {
                            popup.invoke_cancel();
                        }
                        Ok(())
                    }
                    Event::Search(target) => {
                        if signal.load(Ordering::SeqCst) {
                            main.invoke_lock_vault();
                            signal.store(false, Ordering::SeqCst);
                        }
                        show_panel(&popup, &popup_state, &popup_timer, target)
                    }
                    Event::ShortcutRecorded(shortcut) => {
                        main.invoke_captured_shortcut(
                            shortcut.key as i32,
                            shortcut.modifiers as i32,
                        );
                        Ok(())
                    }
                    Event::Manage => {
                        cancel(&popup, &mut popup_state.borrow_mut(), &popup_timer);
                        show_panel(&main, &main_state, &main_timer, None)
                    }
                    Event::Lock => {
                        main.invoke_lock_vault();
                        signal.store(false, Ordering::SeqCst);
                        Ok(())
                    }
                    Event::Quit => {
                        main.invoke_exit_app();
                        Ok(())
                    }
                };
                if let Err(message) = result {
                    platform::show_error(&message);
                }
            }
        });
    }
    let housekeeping = Timer::default();
    {
        let session = session.clone();
        let main_weak = main.as_weak();
        let popup_weak = popup.as_weak();
        let signal = lock_signal.clone();
        let main_state = main_state.clone();
        let popup_state = popup_state.clone();
        let clock = clock.clone();
        let reset_state = resetting.clone();
        let reset_restart = restart_after_reset.clone();
        housekeeping.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            let (Some(main), Some(popup)) = (main_weak.upgrade(), popup_weak.upgrade()) else {
                return;
            };
            if signal.swap(false, Ordering::SeqCst) {
                main.invoke_lock_vault();
            }
            if popup.window().is_visible() && !popup.get_busy() && !popup.get_verification_active()
            {
                let mut state = popup_state.borrow_mut();
                if let Some(error) = state.target.and_then(|target| target.validate().err()) {
                    state.target = None;
                    popup.set_can_fill(false);
                    popup.set_target_label("目标已失效，请重新点击输入框并按呼出快捷键。".into());
                    popup.set_target_detail("".into());
                    popup.set_status(error.into());
                }
            }
            while let Ok(job) = results.try_recv() {
                match job {
                    JobResult::Reset(result) => {
                        session.borrow_mut().working = false;
                        match result {
                            Ok(()) => {
                                reset_restart.set(true);
                                let _ = slint::quit_event_loop();
                            }
                            Err(message) => {
                                let pending = maintenance::reset_pending(&session.borrow().root);
                                reset_state.set(pending);
                                main.invoke_session_changed();
                                main.set_reset_incomplete(pending);
                                main.set_reset_open(true);
                                main.set_reset_master("".into());
                                main.set_status(message.into());
                                main.invoke_focus_search();
                            }
                        }
                    }
                    JobResult::Backup { generation, result } => {
                        session.borrow_mut().working = false;
                        let current = session.borrow().generation == generation
                            && session.borrow().vault.is_some()
                            && !signal.load(Ordering::SeqCst)
                            && !main_state.borrow().auth_cancel.load(Ordering::SeqCst);
                        if !current {
                            if let Ok(Some((vault, path))) = result {
                                drop(vault);
                                let _ = std::fs::remove_file(path);
                            }
                            main.invoke_session_changed();
                            continue;
                        }
                        match result {
                            Ok(Some((vault, path))) => {
                                main.invoke_lock_vault();
                                let previous = session.borrow().path.clone();
                                session.borrow_mut().path = path;
                                let result = session.borrow_mut().accept(vault);
                                if result.is_err() {
                                    session.borrow_mut().path = previous;
                                }
                                main.invoke_session_changed();
                                main.set_status(
                                    result
                                        .map(|_| {
                                            "已验证并恢复到独立副本，原密码库与备份均保留。"
                                                .to_string()
                                        })
                                        .unwrap_or_else(|e| e)
                                        .into(),
                                );
                                clock.touch();
                            }
                            outcome => {
                                main.invoke_session_changed();
                                let refreshed = if main.get_backups_open() {
                                    refresh_backups(&main, &mut main_state.borrow_mut())
                                } else {
                                    Ok(())
                                };
                                let message = outcome
                                    .map(|_| {
                                        "已删除选中的备份，其他备份和当前密码库保留。".to_string()
                                    })
                                    .unwrap_or_else(|e| e);
                                main.set_status(
                                    refreshed.map(|_| message).unwrap_or_else(|e| e).into(),
                                );
                                if main.get_backups_open() {
                                    main.invoke_focus_search();
                                }
                            }
                        }
                    }
                    JobResult::Maintenance(result) => {
                        session.borrow_mut().working = false;
                        main.invoke_session_changed();
                        main.set_status(result.unwrap_or_else(|message| message).into());
                        main.invoke_focus_search();
                    }
                    JobResult::Unlock(generation, result) => {
                        let mut session = session.borrow_mut();
                        session.working = false;
                        let message = if generation != session.generation {
                            None
                        } else {
                            match result.and_then(|vault| session.accept(vault)) {
                                Ok(()) => {
                                    clock.touch();
                                    Some("已解锁。".to_string())
                                }
                                Err(message) => Some(message),
                            }
                        };
                        drop(session);
                        main.invoke_session_changed();
                        if let Some(message) = message {
                            main.set_status(message.clone().into());
                            popup.set_status(message.into());
                        }
                    }
                    JobResult::Verify {
                        panel,
                        serial,
                        generation,
                        result,
                    } => {
                        let (ui, state) = if panel == 0 {
                            (&main, &main_state)
                        } else {
                            (&popup, &popup_state)
                        };
                        session.borrow_mut().working = false;
                        let mut state = state.borrow_mut();
                        let current = session.borrow().generation == generation
                            && session.borrow().auth_owner == Some(panel)
                            && serial == state.request_serial
                            && ui.get_verification_active()
                            && !state.auth_cancel.load(Ordering::SeqCst);
                        if !current {
                            if session.borrow().generation == generation
                                && session.borrow().auth_owner == Some(panel)
                                && serial == state.request_serial
                                && state.requested.is_some()
                                && state.auth_cancel.load(Ordering::SeqCst)
                            {
                                cancel_auth(ui, &mut state);
                                ui.set_status("系统验证已取消或过期，请重新选择账号。".into());
                            }
                            drop(state);
                            main.invoke_session_changed();
                            continue;
                        }
                        let Some((action, _, started)) = state.requested else {
                            continue;
                        };
                        let result = result.and_then(|_| {
                            if started.elapsed() >= Duration::from_secs(60) {
                                return Err("验证请求已过期，请取消后重试。".into());
                            }
                            if signal.load(Ordering::SeqCst)
                                || !window_id(ui).is_ok_and(platform::is_foreground)
                            {
                                return Err(
                                    "验证后窗口已切换，本次操作未获授权。请回到Keyisle重试。"
                                        .into(),
                                );
                            }
                            if let ProtectedAction::Fill(_, _) = action {
                                state.target.ok_or("填充目标已失效。")?.validate()?;
                            }
                            Ok(())
                        });
                        match result {
                            Err(message) => {
                                drop(state);
                                main.invoke_session_changed();
                                ui.set_status(message.into());
                                ui.invoke_focus_search();
                            }
                            Ok(()) => {
                                state.grant = Some(Grant::new(action, generation));
                                state.requested = None;
                                session.borrow_mut().auth_owner = None;
                                ui.set_verification_active(false);
                                ui.set_verify_master("".into());
                                ui.set_status("".into());
                                drop(state);
                                clock.touch();
                                main.invoke_session_changed();
                                let owner_state = if panel == 0 {
                                    &main_state
                                } else {
                                    &popup_state
                                };
                                let index = state_index(owner_state, action.record());
                                let Some(index) = index else {
                                    owner_state.borrow_mut().grant = None;
                                    ui.set_status("账号列表已变化，请重新选择。".into());
                                    continue;
                                };
                                ui.set_selected(index as i32);
                                match action {
                                    ProtectedAction::View(_) => ui.invoke_view_entry(),
                                    ProtectedAction::Edit(_) => ui.invoke_edit_entry(),
                                    ProtectedAction::Delete(id) => {
                                        if panel == 0 {
                                            main_state.borrow_mut().deleting_id = id;
                                        } else {
                                            popup_state.borrow_mut().deleting_id = id;
                                        }
                                        ui.set_delete_confirm(true);
                                        ui.invoke_delete_entry();
                                    }
                                    ProtectedAction::Fill(_, mode) => ui.invoke_fill(match mode {
                                        FillMode::Password => FillAction::Password,
                                        FillMode::Username => FillAction::Username,
                                        FillMode::Both => FillAction::Both,
                                    }),
                                }
                                owner_state.borrow_mut().grant = None;
                            }
                        }
                    }
                }
            }
            if session.borrow().vault.is_some() && platform::idle_expired(&clock) {
                main.invoke_lock_vault();
            }
        });
    }
    if !std::env::args().any(|arg| arg == "--autostart")
        || !startup::load_tray(&session.borrow().root)?
        || resumed_master_change
        || !native.shortcut_registered
    {
        show_panel(&main, &main_state, &main_timer, None)?;
    }
    if resumed_master_change {
        main.set_status("已完成上次中断的主密码切换，请使用新主密码解锁。".into());
    }
    if !native.shortcut_registered {
        main.set_status("呼出快捷键被占用，请从托盘打开主界面，在设置中修改。".into());
    }
    let result = slint::run_event_loop_until_quit().map_err(|e| e.to_string());
    geometry_timer.stop();
    let layout_result = if !resetting.get() && !maintenance::reset_pending(&root) {
        geometry_cache
            .borrow()
            .map_or(Ok(()), |layout| layout.save(&root))
    } else {
        Ok(())
    };
    main_timer.stop();
    popup_timer.stop();
    housekeeping.stop();
    session.borrow_mut().lock();
    main_state
        .borrow()
        .auth_cancel
        .store(true, Ordering::SeqCst);
    popup_state
        .borrow()
        .auth_cancel
        .store(true, Ordering::SeqCst);
    recording_generation.fetch_add(1, Ordering::SeqCst);
    active_recording.borrow_mut().take();
    native.stop();
    drop(native);
    if restart_after_reset.get() {
        std::process::Command::new(
            std::env::current_exe().map_err(|_| "无法定位程序以重新启动。")?,
        )
        .spawn()
        .map_err(|_| "还原已完成，请手动重新启动Keyisle。")?;
    }
    result.and(layout_result)
}
