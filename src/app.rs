use crate::platform::{self, BackupDialog, Event, Native, Target};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, VecDeque},
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
    security::{
        self, Grant, IdleSource, LOCK_MINUTES, LockClock, LockSettings, ProtectedAction,
        generate_password,
    },
    session::Session,
    settings,
    shortcut::Shortcut,
    startup,
    update::{self, CheckOutcome, PreparedUpdate, UpdateManifest},
    vault::{Credential, Vault},
};
use zeroize::Zeroizing;
slint::include_modules!();

#[derive(Clone)]
struct JobDispatcher {
    sender: mpsc::Sender<JobResult>,
    ui: slint::Weak<AppWindow>,
}

impl JobDispatcher {
    fn send(&self, job: JobResult) {
        if self.sender.send(job).is_ok() {
            let _ = self.ui.upgrade_in_event_loop(|ui| {
                ui.invoke_process_jobs();
            });
        }
    }
}

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
    delete_request: u64,
    deleting_ids: Vec<i64>,
    account_checks: BTreeSet<i64>,
    target: Option<Target>,
    target_application: String,
    details_id: Option<i64>,
    matches: Vec<i64>,
    pending: Option<Pending>,
    backup_paths: Vec<PathBuf>,
    backup_checks: BTreeSet<PathBuf>,
    backup_sources: Vec<PathBuf>,
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

fn reset_secret_masks(ui: &AppWindow) {
    ui.set_details_password("".into());
    ui.set_secret_serial(ui.get_secret_serial().wrapping_add(1));
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
    reset_secret_masks(ui);
}

fn clear_details(ui: &AppWindow) {
    ui.set_details_open(false);
    ui.set_details_title("".into());
    ui.set_details_user("".into());
    ui.set_details_has_password(false);
    ui.set_details_category("".into());
    ui.set_details_tags("".into());
    ui.set_details_url("".into());
    ui.set_details_notes("".into());
    reset_secret_masks(ui);
}

fn clear_maintenance(ui: &AppWindow) {
    ui.set_context_open(false);
    clear_backup_list(ui);
    ui.set_maintenance_open(false);
    ui.set_backups_confirm(false);
    ui.set_current_master("".into());
    ui.set_new_master("".into());
    ui.set_new_confirmation("".into());
    reset_secret_masks(ui);
}

fn clear_backup_list(ui: &AppWindow) {
    ui.set_backups_open(false);
    ui.set_backup_batch(false);
    ui.set_backup_checked(0);
    ui.set_backup_master("".into());
    ui.set_backup_confirm_mode(0);
    ui.set_selected_backup(-1);
    ui.set_backup_rows(ModelRc::new(VecModel::from(Vec::<BackupRow>::new())));
    reset_secret_masks(ui);
}

fn refresh_backups(ui: &AppWindow, state: &mut State) -> Result<(), String> {
    let selected = usize::try_from(ui.get_selected_backup())
        .ok()
        .and_then(|i| state.backup_paths.get(i))
        .cloned();
    let session = state.session.borrow();
    let backups = maintenance::backup_list(&session.root, &session.path)?;
    let directory = settings::backup_directory(&session.root)?;
    ui.set_backup_directory_label(
        if directory != session.root.join("backups")
            && !settings::backup_folder_available(&directory)
        {
            format!("默认目录不可用：{}", directory.display())
        } else {
            format!("默认目录：{}", directory.display())
        }
        .into(),
    );
    let inactive = maintenance::inactive_vault_count(&session.root, &session.path);
    state
        .backup_checks
        .retain(|path| ui.get_backup_batch() && backups.iter().any(|backup| backup.path == *path));
    let rows: Vec<_> = backups
        .iter()
        .map(|entry| BackupRow {
            title: entry.kind.into(),
            date: platform::backup_time(entry.nanos).into(),
            detail: format!(
                "{:.1} KiB · {}",
                entry.bytes as f64 / 1024.0,
                entry.path.file_name().unwrap().to_string_lossy()
            )
            .into(),
            checked: state.backup_checks.contains(&entry.path),
        })
        .collect();
    state.backup_paths = backups.into_iter().map(|b| b.path).collect();
    state.backup_sources.clear();
    ui.set_backup_checked(state.backup_checks.len() as i32);
    ui.set_backup_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_backup_count(state.backup_paths.len() as i32);
    ui.set_inactive_vault_count(inactive as i32);
    ui.set_selected_backup(if state.backup_paths.is_empty() {
        -1
    } else {
        selected
            .and_then(|path| state.backup_paths.iter().position(|p| p == &path))
            .unwrap_or(0) as i32
    });
    ui.set_backup_confirm_mode(0);
    ui.set_backup_master("".into());
    reset_secret_masks(ui);
    Ok(())
}

fn rich(color: &str, text: &str, spans: &[(&str, bool)]) -> slint::StyledText {
    if !spans.iter().any(|(_, hit)| *hit) {
        return slint::StyledText::from_plain_text(text);
    }
    let markup: String = spans
        .iter()
        .map(|(part, hit)| {
            let escaped = windows_password_manager::search::highlighted_markdown("", part);
            if *hit {
                format!("<font color=\"{color}\">**{escaped}**</font>")
            } else {
                escaped
            }
        })
        .collect();
    slint::StyledText::from_markdown(&markup)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(text))
}

fn highlighter(tokens: &[String], text: &str) -> Vec<(String, bool)> {
    if text.is_empty() {
        return Vec::new();
    }
    windows_password_manager::search::highlights_for_tokens(tokens, text)
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

fn changed_options(
    current: &ModelRc<slint::SharedString>,
    placeholder: &str,
    options: &[String],
) -> Option<ModelRc<slint::SharedString>> {
    let values = || std::iter::once(placeholder).chain(options.iter().map(String::as_str));
    if current.row_count() == options.len() + 1
        && current
            .iter()
            .zip(values())
            .all(|(old, new)| old.as_str() == new)
    {
        return None;
    }
    Some(ModelRc::new(VecModel::from(
        values().map(Into::into).collect::<Vec<_>>(),
    )))
}

fn refresh(ui: &AppWindow, state: &mut State) {
    let previously_selected = selected_id(ui, state);
    let mut session = state.session.borrow_mut();
    ui.set_blocked(session.working || session.auth_owner.is_some());
    let settings = state.clock.settings();
    ui.set_policy_label(
        format!(
            "{}空闲 {} 分钟锁定",
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

    if let Err(message) = session.ensure_cache() {
        ui.set_status(message.into());
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

    let query = ui.get_query();
    let tokens = windows_password_manager::search::query_tokens(query.as_str());
    let (categories, tags, mut ranked) = if let Some(cache) = session.cache.as_ref() {
        if !cache
            .categories
            .iter()
            .any(|c| c == ui.get_category().as_str())
        {
            ui.set_category("".into());
        }
        if !cache.tags.iter().any(|t| t == ui.get_tag().as_str()) {
            ui.set_tag("".into());
        }
        let cat_str = ui.get_category();
        let tag_str = ui.get_tag();
        let ranked: Vec<_> = cache
            .entries
            .iter()
            .filter_map(|(record, prepared)| {
                prepared
                    .rank_tokens(
                        record,
                        tokens.as_deref()?,
                        cat_str.as_str(),
                        tag_str.as_str(),
                    )
                    .map(|score| (score, record.clone()))
            })
            .collect();
        (cache.categories.clone(), cache.tags.clone(), ranked)
    } else {
        ui.set_category("".into());
        ui.set_tag("".into());
        (Vec::new(), Vec::new(), Vec::new())
    };
    let unlocked = session.vault.is_some();
    drop(session);

    if !unlocked {
        ui.set_account_batch(false);
        state.deleting_ids.clear();
        state.backup_paths.clear();
        state.backup_checks.clear();
        state.backup_sources.clear();
    }
    if !ui.get_account_batch() {
        state.account_checks.clear();
    }
    ui.set_account_checked(state.account_checks.len() as i32);

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

    let color = ui.get_match_color();
    let tokens = tokens.as_deref().unwrap_or_default();
    let compact = ui.get_compact();
    let mut rows = Vec::with_capacity(ranked.len());
    let mut shared_metadata = HashMap::new();
    let mut metadata_spans = |value: String| {
        Rc::clone(
            shared_metadata
                .entry(value.clone())
                .or_insert_with(|| Rc::new(highlighter(tokens, &value))),
        )
    };
    for (_, record) in ranked {
        state.matches.push(record.id);
        let accessible_title = if record.sensitive {
            format!("敏感 · {}", record.title)
        } else {
            record.title.clone()
        };
        let username = if record.sensitive {
            "使用前需验证".to_string()
        } else {
            record.username
        };
        let username_spans = highlighter(if record.sensitive { &[] } else { tokens }, &username);
        let category = metadata_spans(record.category);
        let tags = metadata_spans(record.tags);
        let url = metadata_spans(record.url);
        let mut metadata = Vec::new();
        if compact {
            metadata.extend(
                username_spans
                    .iter()
                    .map(|(part, hit)| (part.as_str(), *hit)),
            );
        }
        for (label, field) in [("分类：", &category), ("标签：", &tags), ("网址：", &url)]
        {
            let matched = field.iter().any(|(_, hit)| *hit);
            if field.is_empty() || (!matched && (compact || label == "网址：")) {
                continue;
            }
            if !metadata.is_empty() {
                metadata.push((" · ", false));
            }
            if compact || label == "网址：" {
                metadata.push((label, false));
            }
            metadata.extend(field.iter().map(|(part, hit)| (part.as_str(), *hit)));
        }
        let meta_text: String = metadata.iter().map(|(part, _)| *part).collect();
        let meta_rich = rich(color.as_str(), &meta_text, &metadata);
        let (hint, hint_rich, detail, detail_rich, user_rich) = if compact {
            (
                meta_text,
                meta_rich,
                String::new(),
                slint::StyledText::default(),
                slint::StyledText::default(),
            )
        } else {
            let user = username_spans
                .iter()
                .map(|(part, hit)| (part.as_str(), *hit))
                .collect::<Vec<_>>();
            (
                String::new(),
                slint::StyledText::default(),
                meta_text,
                meta_rich,
                rich(color.as_str(), &username, &user),
            )
        };
        let title = highlighter(tokens, &record.title);
        let title_spans = title
            .iter()
            .map(|(part, hit)| (part.as_str(), *hit))
            .collect::<Vec<_>>();
        rows.push(EntryRow {
            title_rich: rich(color.as_str(), &record.title, &title_spans),
            user_rich,
            detail_rich,
            hint_rich,
            hint: hint.into(),
            title: accessible_title.into(),
            username: username.into(),
            detail: detail.into(),
            sensitive: record.sensitive,
            checked: state.account_checks.contains(&record.id),
        });
    }
    if !compact {
        if let Some(model) = changed_options(&ui.get_edit_categories(), "选择已有分类", &categories)
        {
            ui.set_edit_categories(model);
        }
        if let Some(model) = changed_options(&ui.get_edit_tag_options(), "添加已有标签", &tags)
        {
            ui.set_edit_tag_options(model);
        }
        if let Some(model) = changed_options(&ui.get_categories(), "全部分类", &categories) {
            ui.set_categories(model);
        }
        if let Some(model) = changed_options(&ui.get_tags(), "全部标签", &tags) {
            ui.set_tags(model);
        }
    }
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
    if !unlocked {
        clear_editor(ui);
        clear_details(ui);
        state.details_id = None;
        ui.set_master("".into());
        ui.set_confirmation("".into());
        reset_secret_masks(ui);
        ui.set_can_fill(false);
    }
}

fn master_attempt_result<T>(session: &mut Session, result: Result<T, String>) -> Result<T, String> {
    match &result {
        Ok(_) => session.master_attempts.accepted(),
        Err(message) if message == windows_password_manager::vault::MASTER_REJECTED => {
            if let Some(wait) = session.master_attempts.rejected(Instant::now()) {
                return Err(format!("{message} {wait}"));
            }
        }
        Err(_) => {}
    }
    result
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
        result: Result<BackupOutcome, String>,
    },
    Verify {
        panel: usize,
        serial: u64,
        generation: u64,
        master_password: bool,
        result: Result<(), String>,
    },
    UpdateCheck {
        serial: u64,
        manual: bool,
        result: Result<CheckOutcome, String>,
    },
    UpdateProgress {
        serial: u64,
        received: u64,
        total: u64,
    },
    UpdatePrepared {
        serial: u64,
        result: Result<PreparedUpdate, String>,
    },
}

enum BackupOutcome {
    Restored(Vault, PathBuf),
    Deleted(usize),
}

struct UpdateController {
    available: Option<UpdateManifest>,
    busy: bool,
    cancel: Arc<AtomicBool>,
    serial: u64,
    pending_launch: Option<PreparedUpdate>,
}

fn schedule_clipboard_clear(seq: &Rc<Cell<u32>>, timer: &Rc<Timer>, delay: Duration) {
    timer.stop();
    if delay.is_zero() && platform::clear_clipboard_if_matches(seq.get()) {
        seq.set(0);
        return;
    }
    let seq = seq.clone();
    let timer_weak = Rc::downgrade(timer);
    let deadline = Instant::now() + delay;
    timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
        if Instant::now() >= deadline && platform::clear_clipboard_if_matches(seq.get()) {
            seq.set(0);
            if let Some(timer) = timer_weak.upgrade() {
                timer.stop();
            }
        }
    });
}

fn start_update_check(
    main: &AppWindow,
    ctrl: &RefCell<UpdateController>,
    jobs: &JobDispatcher,
    manual: bool,
) {
    let mut c = ctrl.borrow_mut();
    if c.busy {
        return;
    }
    c.serial = c.serial.wrapping_add(1);
    let serial = c.serial;
    let cancel = Arc::new(AtomicBool::new(false));
    c.cancel = cancel.clone();
    c.busy = true;
    drop(c);

    main.set_update_checking(true);
    if manual {
        main.set_update_error("".into());
        main.set_update_status("".into());
        if main.get_settings_open() {
            main.set_status("".into());
        }
    }
    let jobs = jobs.clone();
    std::thread::spawn(move || {
        let result = update::check_for_updates(&cancel);
        jobs.send(JobResult::UpdateCheck {
            serial,
            manual,
            result,
        });
    });
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
    reset_secret_masks(ui);
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
    let result = (|| {
        let session = state.session.borrow();
        let vault = session.vault.as_ref().unwrap();
        let (sensitive, title) = if let Some(id) = action.record() {
            (vault.is_sensitive(id)?, vault.title(id)?)
        } else {
            if action != ProtectedAction::Delete(state.delete_request)
                || state.deleting_ids.is_empty()
            {
                return Err("删除请求已失效，请重新选择。".into());
            }
            let sensitive = state
                .deleting_ids
                .iter()
                .try_fold(false, |sensitive, &id| {
                    vault.is_sensitive(id).map(|current| current || sensitive)
                })?;
            let title = if state.deleting_ids.len() == 1 {
                vault.title(state.deleting_ids[0])?
            } else {
                format!("已选 {} 个账号", state.deleting_ids.len())
            };
            (sensitive, title)
        };
        Ok::<_, String>((sensitive, title, session.generation))
    })();
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
    reset_secret_masks(ui);
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

fn guard_draft(ui: &AppWindow, action: i32) -> bool {
    let settings_route =
        ui.get_settings_open() || (ui.get_about_open() && ui.get_about_from_settings());
    if settings_route {
        ui.invoke_cancel_recording();
    }
    let settings_draft = settings_route && ui.get_settings_dirty();
    if ui.get_unlocked() && ((ui.get_editing() && ui.get_edit_dirty()) || settings_draft) {
        ui.set_discard_settings(settings_draft);
        ui.set_discard_action(action);
        ui.invoke_focus_discard();
        true
    } else {
        false
    }
}

fn hide_window(ui: &AppWindow, popup_watch: Option<&Timer>) -> Result<(), String> {
    if ui.get_compact() {
        if let Some(watch) = popup_watch {
            watch.stop();
        }
        let _ = platform::watch_popup(None);
    }
    ui.hide().map_err(|error| error.to_string())
}

fn show_window(ui: &AppWindow, popup_watch: Option<&Timer>) -> Result<(), String> {
    ui.show().map_err(|error| error.to_string())?;
    if ui.get_compact() {
        platform::watch_popup(Some(window_id(ui)?))?;
        if let Some(watch) = popup_watch {
            watch.restart();
        }
    }
    Ok(())
}

fn cancel(ui: &AppWindow, state: &mut State, timer: &Timer, popup_watch: Option<&Timer>) {
    state.clock.touch();
    timer.stop();
    cancel_auth(ui, state);
    state.editor_verified = None;
    ui.set_settings_open(false);
    ui.set_about_open(false);
    ui.set_context_open(false);
    ui.set_app_menu_open(false);
    clear_maintenance(ui);
    ui.set_account_batch(false);
    state.pending = None;
    state.deleting_ids.clear();
    state.backup_paths.clear();
    state.backup_checks.clear();
    state.backup_sources.clear();
    state.target = None;
    state.target_application.clear();
    state.details_id = None;
    clear_details(ui);
    ui.set_busy(false);
    ui.set_can_fill(false);
    ui.set_target_label("".into());
    ui.set_target_detail(TargetDetail::default());
    ui.set_master("".into());
    ui.set_confirmation("".into());
    clear_editor(ui);
    let _ = hide_window(ui, popup_watch);
}

fn report(ui: &AppWindow, state: &mut State, message: String, popup_watch: Option<&Timer>) {
    state.pending = None;
    state.target = None;
    ui.set_busy(false);
    ui.set_can_fill(false);
    ui.set_status(message.into());
    ui.set_target_label("本次填充已取消；请重新点击输入框并按呼出快捷键。".into());
    ui.set_target_detail(TargetDetail::default());
    let _ = show_window(ui, popup_watch);
    if let Ok(window) = window_id(ui) {
        platform::activate(window);
    }
    ui.invoke_focus_search();
}

fn bind(
    ui: &AppWindow,
    session: Rc<RefCell<Session>>,
    jobs: JobDispatcher,
    clock: Arc<LockClock>,
    panel: usize,
    lock_signal: Arc<AtomicBool>,
    popup_watch: Option<Rc<Timer>>,
    clipboard_seq: Rc<Cell<u32>>,
    clipboard_timer: Rc<Timer>,
) -> (Rc<RefCell<State>>, Rc<Timer>) {
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.set_install_mode_label(
        if update::is_installed_mode() {
            "安装版"
        } else {
            "便携版"
        }
        .into(),
    );
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
        delete_request: 0,
        deleting_ids: Vec::new(),
        account_checks: BTreeSet::new(),
        backup_paths: Vec::new(),
        backup_checks: BTreeSet::new(),
        backup_sources: Vec::new(),
        context_record: None,
    }));
    let timer = Rc::new(Timer::default());
    ui.on_master_strength_hint(|password| security::master_strength_hint(password.as_str()).into());
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
        ui.on_open_release_page(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(message) = platform::open_web_url(update::RELEASE_PAGE_URL) {
                    ui.set_update_error(message.into());
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
                ui.invoke_cancel_recording();
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
                if ui.get_update_installing() {
                    return;
                }
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
                let mut state = state.borrow_mut();
                state.clock.touch();
                state.matches.clear();
                state.account_checks.clear();
                refresh(&ui, &mut state);
                ui.invoke_scroll_selected();
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let timer = timer.clone();
        let watch = popup_watch.clone();
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
                if guard_draft(&ui, 2) {
                    return;
                }
                cancel(&ui, &mut state.borrow_mut(), &timer, watch.as_deref());
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
                let surface_blocked_window = |ui: &AppWindow| {
                    if !ui.get_compact() {
                        if !ui.window().is_visible() {
                            let _ = show_window(ui, None);
                        }
                        ui.window().with_winit_window(|window| {
                            window.set_minimized(false);
                        });
                        ui.window().set_minimized(false);
                        if let Ok(hwnd) = window_id(ui) {
                            platform::activate(hwnd);
                        }
                    }
                };
                if ui.get_working() {
                    surface_blocked_window(&ui);
                    return;
                }
                if guard_draft(&ui, 3) {
                    surface_blocked_window(&ui);
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
        let watch = popup_watch.clone();
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
            if ui.get_discard_settings() {
                ui.invoke_mark_settings_saved();
                ui.set_settings_open(false);
                ui.set_about_from_settings(false);
            } else {
                clear_editor(&ui);
                state.borrow_mut().editor_verified = None;
            }
            match action {
                2 => {
                    cancel(&ui, &mut state.borrow_mut(), &timer, watch.as_deref());
                    ui.invoke_session_changed();
                }
                3 => {
                    let _ = slint::quit_event_loop();
                }
                4 => {
                    ui.set_about_open(true);
                    ui.invoke_install_update();
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
            let creating = !session.path.exists();
            if !creating
                && let Some(message) = session.master_attempts.retry_message(Instant::now())
            {
                ui.set_status(message.into());
                return;
            }
            let master = Zeroizing::new(ui.get_master().to_string());
            let confirmation = Zeroizing::new(ui.get_confirmation().to_string());
            ui.set_master("".into());
            ui.set_confirmation("".into());
            reset_secret_masks(&ui);
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
                jobs.send(JobResult::Unlock(generation, result));
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
            if guard_draft(&ui, 1) {
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
        ui.on_generate_password(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.get_editing() || ui.get_ui_blocked() {
                return;
            }
            state.borrow().clock.touch();
            let length = match ui.get_generator_length_choice() {
                0 => 12,
                1 => 16,
                _ => 20,
            };
            match generate_password(length, ui.get_generator_symbols()) {
                Ok(generated) => {
                    ui.set_edit_password(generated.as_str().into());
                    ui.set_edit_dirty(true);
                    success(&ui, "已生成 20 位高强度随机密码，保存账号后生效。".into());
                }
                Err(message) => ui.set_status(message.into()),
            }
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
                .and_then(|v| v.details(id));
            match record {
                Ok((record, has_password)) => {
                    clear_details(&ui);
                    state.details_id = Some(id);
                    state.clock.touch();
                    ui.set_details_title(record.title.into());
                    ui.set_details_user(record.username.into());
                    ui.set_details_has_password(has_password);
                    ui.set_details_category(record.category.into());
                    ui.set_details_tags(record.tags.into());
                    ui.set_details_url(record.url.into());
                    ui.set_details_notes(record.notes.into());
                    reset_secret_masks(&ui);
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
        let clock = clock.clone();
        ui.on_append_edit_tag(move |tag| {
            let Some(ui) = weak.upgrade() else { return };
            if !ui.get_editing() || ui.get_ui_blocked() {
                return;
            }
            let raw = ui.get_edit_tags();
            let value = raw.trim();
            if value
                .split([',', '，', ';', '；'])
                .map(str::trim)
                .any(|part| part == tag.as_str())
            {
                return;
            }
            ui.set_edit_tags(if value.is_empty() {
                tag
            } else {
                format!("{value}, {tag}").into()
            });
            ui.set_edit_dirty(true);
            clock.touch();
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_reveal_details_password(move |reveal| {
            let Some(ui) = weak.upgrade() else { return };
            if !reveal {
                ui.set_details_password("".into());
                return;
            }
            let state = state.borrow();
            if !ui.get_details_open() || !available(&state) {
                return;
            }
            if let Some(id) = state.details_id {
                state.clock.touch();
                match state.session.borrow().vault.as_ref().unwrap().password(id) {
                    Ok(password) => ui.set_details_password(password.as_str().into()),
                    Err(error) => ui.set_status(error.into()),
                }
            }
        });
    }
    for copy_secret in [false, true] {
        let weak = ui.as_weak();
        let state = state.clone();
        let seq_cell = clipboard_seq.clone();
        let timer_ref = clipboard_timer.clone();
        let handler = move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.get_details_open()
                || !available(&state.borrow())
                || state.borrow().details_id.is_none()
            {
                return;
            }
            let state = state.borrow();
            state.clock.touch();
            let value = if copy_secret {
                state
                    .session
                    .borrow()
                    .vault
                    .as_ref()
                    .unwrap()
                    .password(state.details_id.unwrap())
            } else {
                Ok(Zeroizing::new(ui.get_details_user().to_string()))
            };
            let result = value.and_then(|value| {
                window_id(&ui).and_then(|owner| platform::copy_protected_text(owner, &value))
            });
            match result {
                Ok(seq) => {
                    seq_cell.set(seq);
                    schedule_clipboard_clear(&seq_cell, &timer_ref, Duration::from_secs(15));
                    let label = if copy_secret { "密码" } else { "用户名" };
                    success(
                        &ui,
                        format!("已临时复制{label}（已排除剪贴板历史，15 秒后或锁库自动清除）。"),
                    );
                }
                Err(message) => ui.set_status(message.into()),
            }
        };
        if copy_secret {
            ui.on_copy_details_password(handler);
        } else {
            ui.on_copy_details_user(handler);
        }
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_close_details(move || {
            if let Some(ui) = weak.upgrade() {
                clear_details(&ui);
                state.borrow_mut().details_id = None;
                ui.set_status("".into());
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
                    clear_editor(&ui);
                    state.editing_id = record.id;
                    ui.set_edit_sensitive(record.sensitive);
                    ui.set_edit_title(record.title.into());
                    ui.set_edit_user(record.username.into());
                    ui.set_edit_password(record.password.as_str().into());
                    ui.set_edit_category(record.category.into());
                    ui.set_edit_tags(record.tags.into());
                    ui.set_edit_url(record.url.into());
                    ui.set_edit_notes(record.notes.into());
                    reset_secret_masks(&ui);
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
                if guard_draft(&ui, 1) {
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
                url: ui.get_edit_url().trim().to_owned(),
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
                session.invalidate_cache();
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
        ui.on_toggle_account_batch(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            if ui.get_compact() || ui.get_delete_confirm() || !available(&state) {
                return;
            }
            state.account_checks.clear();
            ui.set_account_batch(!ui.get_account_batch());
            ui.set_status("".into());
            state.clock.touch();
            refresh(&ui, &mut state);
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_check_account(move |index, checked| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            if !ui.get_account_batch() || ui.get_delete_confirm() || !available(&state) {
                return;
            }
            if let Some(&id) = state.matches.get(index as usize) {
                if checked {
                    state.account_checks.insert(id);
                } else {
                    state.account_checks.remove(&id);
                }
                ui.set_account_checked(state.account_checks.len() as i32);
                state.clock.touch();
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
                let ids: Vec<_> = if ui.get_account_batch() {
                    state.account_checks.iter().copied().collect()
                } else {
                    selected_id(&ui, &state).into_iter().collect()
                };
                if ids.is_empty() {
                    return;
                }
                let subject = if ids.len() == 1 && !ui.get_account_batch() {
                    let session = state.session.borrow();
                    match session.vault.as_ref().unwrap().title(ids[0]) {
                        Ok(title) => format!("账号「{title}」"),
                        Err(message) => {
                            ui.set_status(message.into());
                            return;
                        }
                    }
                } else {
                    format!("选中的 {} 个账号", ids.len())
                };
                state.deleting_ids = ids;
                state.delete_request = state.delete_request.wrapping_add(1);
                ui.set_delete_confirm(true);
                ui.set_status(format!("确认删除{subject}？本次删除不会创建新的加密备份。").into());
                return;
            }
            let action = ProtectedAction::Delete(state.delete_request);
            if !authorize(&ui, &mut state, action) {
                drop(state);
                ui.invoke_session_changed();
                ui.invoke_focus_search();
                return;
            }
            let count = state.deleting_ids.len();
            let result = (|| {
                let mut session = state.session.borrow_mut();
                session
                    .vault
                    .as_mut()
                    .ok_or("密码库已锁定。".to_string())?
                    .delete_many(&state.deleting_ids)?;
                session.invalidate_cache();
                Ok::<_, String>(())
            })();
            match result {
                Ok(()) => {
                    state.deleting_ids.clear();
                    state.account_checks.clear();
                    drop(state);
                    success(&ui, format!("已删除 {count} 个账号。"));
                    ui.invoke_session_changed();
                    ui.invoke_focus_search();
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
            if method == 0
                && let Some(message) = state
                    .session
                    .borrow()
                    .master_attempts
                    .retry_message(Instant::now())
            {
                ui.set_status(message.into());
                return;
            }
            let master = Zeroizing::new(ui.get_verify_master().to_string());
            ui.set_verify_master("".into());
            reset_secret_masks(&ui);
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
                jobs.send(JobResult::Verify {
                    panel,
                    serial,
                    generation,
                    master_password: method == 0,
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
        ui.on_close_settings(move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_cancel_recording();
                if !guard_draft(&ui, 5) {
                    ui.set_settings_open(false);
                    ui.set_status("".into());
                    ui.invoke_focus_search();
                }
                state.borrow().clock.touch();
            }
        });
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
            let root = state.borrow().session.borrow().root.clone();
            let backup_directory = match settings::backup_directory(&root) {
                Ok(path) => path.to_string_lossy().into_owned(),
                Err(_) => String::new(),
            };
            ui.set_backup_directory_choice(backup_directory.into());
            let shortcut = Shortcut::load(&root);
            match shortcut {
                Ok(shortcut) => ui.set_shortcut_input(shortcut.to_string().into()),
                Err(message) => {
                    ui.set_status(message.into());
                    return;
                }
            }
            let screen_protection =
                match windows_password_manager::settings::load_screen_protection(&root) {
                    Ok(enabled) => enabled,
                    Err(message) => {
                        ui.set_status(message.into());
                        return;
                    }
                };
            let auto_update = match windows_password_manager::settings::load_auto_update(&root) {
                Ok(enabled) => enabled,
                Err(message) => {
                    ui.set_status(message.into());
                    return;
                }
            };
            let settings = state.borrow().clock.settings();
            let launch = startup::load_tray(&root)
                .and_then(|tray| startup::enabled().map(|enabled| (tray, enabled)));
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
            ui.set_screen_protection_choice(screen_protection);
            ui.set_auto_update_choice(auto_update);
            ui.invoke_mark_settings_saved();
            ui.set_settings_open(true);
            ui.invoke_scroll_settings_top();
            ui.set_status("".into());
            state.borrow().clock.touch();
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_choose_backup_directory(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !available(&state.borrow()) || !ui.get_settings_open() {
                return;
            }
            ui.invoke_cancel_recording();
            let root = state.borrow().session.borrow().root.clone();
            let draft = PathBuf::from(ui.get_backup_directory_choice().as_str());
            let result = (|| {
                let initial = if draft.is_absolute() && settings::backup_folder_available(&draft) {
                    draft
                } else {
                    settings::backup_dialog_directory(&root)?
                };
                if let Some(folder) = platform::choose_backup_path(
                    window_id(&ui)?,
                    &initial,
                    BackupDialog::Directory,
                )? && available(&state.borrow())
                    && ui.get_settings_open()
                {
                    ui.set_backup_directory_choice(folder.to_string_lossy().as_ref().into());
                    state.borrow().clock.touch();
                }
                Ok::<_, String>(())
            })();
            if let Err(message) = result {
                ui.set_status(message.into());
            }
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
            ui.invoke_cancel_recording();
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
        ui.on_toggle_backup_batch(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            if !available(&state) || !ui.get_backups_open() || ui.get_backup_confirm_mode() != 0 {
                return;
            }
            state.backup_checks.clear();
            ui.set_backup_batch(!ui.get_backup_batch());
            state.clock.touch();
            match refresh_backups(&ui, &mut state) {
                Ok(()) => ui.set_status("".into()),
                Err(message) => ui.set_status(message.into()),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_check_backup(move |index, checked| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            if !available(&state) || !ui.get_backup_batch() || ui.get_backup_confirm_mode() != 0 {
                return;
            }
            if let Some(path) = state.backup_paths.get(index as usize).cloned() {
                if checked {
                    state.backup_checks.insert(path);
                } else {
                    state.backup_checks.remove(&path);
                }
                ui.set_backup_checked(state.backup_checks.len() as i32);
                state.clock.touch();
            }
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
            match refresh_backups(&ui, &mut state.borrow_mut()) {
                Ok(()) => success(&ui, "备份列表已刷新。".into()),
                Err(message) => ui.set_status(message.into()),
            }
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
            if ui.get_working() {
                return;
            }
            let mut state = state.borrow_mut();
            state.backup_paths.clear();
            state.backup_checks.clear();
            state.backup_sources.clear();
            clear_backup_list(&ui);
            ui.set_status("".into());
            state.clock.touch();
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
            let batch = ui.get_backup_batch();
            if restore && batch {
                return;
            }
            let mode = if restore { 2 } else { 1 };
            if ui.get_backup_confirm_mode() != mode {
                let mut state = state.borrow_mut();
                let sources: Vec<_> = if batch {
                    state.backup_checks.iter().cloned().collect()
                } else {
                    state
                        .backup_paths
                        .get(ui.get_selected_backup() as usize)
                        .cloned()
                        .into_iter()
                        .collect()
                };
                if sources.is_empty() {
                    ui.set_status("请先选择要操作的备份。".into());
                    return;
                }
                if sources.len() == 1 && !batch {
                    ui.set_status(
                        format!(
                            "选中：{}",
                            sources[0].file_name().unwrap().to_string_lossy()
                        )
                        .into(),
                    );
                } else {
                    ui.set_status("".into());
                }
                ui.set_backup_delete_count(sources.len() as i32);
                state.backup_sources = sources;
                ui.set_backup_confirm_mode(mode);
                ui.set_backup_master("".into());
                reset_secret_masks(&ui);
                ui.invoke_focus_search();
                state.clock.touch();
                return;
            }
            let master = Zeroizing::new(ui.get_backup_master().to_string());
            ui.set_backup_master("".into());
            reset_secret_masks(&ui);
            if master.is_empty() {
                ui.set_status("请输入本次操作所需的主密码。".into());
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let (root, active, generation, sources) = {
                let mut state = state.borrow_mut();
                if state.backup_sources.is_empty() {
                    ui.set_backup_confirm_mode(0);
                    ui.set_status("所选备份已变化，请重新选择。".into());
                    return;
                }
                state.auth_cancel = cancel.clone();
                let sources = state.backup_sources.clone();
                let mut session = state.session.borrow_mut();
                session.working = true;
                (
                    session.root.clone(),
                    session.path.clone(),
                    session.generation,
                    sources,
                )
            };
            ui.invoke_session_changed();
            ui.set_status("正在验证并处理选中的备份…".into());
            let jobs = jobs.clone();
            std::thread::spawn(move || {
                let result = if restore {
                    maintenance::restore_backup(&root, &active, &sources[0], &master, &cancel)
                        .map(|(vault, path)| BackupOutcome::Restored(vault, path))
                } else {
                    maintenance::delete_backups(&root, &active, &sources, &master, &cancel)
                        .map(BackupOutcome::Deleted)
                };
                jobs.send(JobResult::Backup { generation, result });
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
            ui.invoke_cancel_recording();
            let state = state.borrow();
            let session = state.session.borrow();
            match maintenance::local_backups(&session.root, &session.path) {
                Ok(paths) => {
                    let inactive = maintenance::inactive_vault_count(&session.root, &session.path);
                    clear_maintenance(&ui);
                    ui.set_backup_count(paths.len() as i32);
                    ui.set_inactive_vault_count(inactive as i32);
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
                    reset_secret_masks(&ui);
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
            reset_secret_masks(&ui);
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
                reset_secret_masks(&ui);
                ui.set_status("请核对备份数量，输入当前主密码，再确认清除。".into());
                state.borrow().clock.touch();
                return;
            }
            let old = Zeroizing::new(ui.get_current_master().to_string());
            let new = Zeroizing::new(ui.get_new_master().to_string());
            let confirmation = Zeroizing::new(ui.get_new_confirmation().to_string());
            reset_secret_masks(&ui);
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
                    "正在并行验证并重新加密密码库和全部本地备份；请勿关闭程序…"
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
                jobs.send(JobResult::Maintenance(result));
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
        let watch = popup_watch.clone();
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
                    report(&ui, &mut current, error, watch.as_deref());
                    return;
                }
            };
            if !platform::is_foreground(panel) {
                report(
                    &ui,
                    &mut current,
                    "搜索窗口已失去焦点，已取消填充。".into(),
                    watch.as_deref(),
                );
                return;
            }
            let mode = match action {
                FillAction::Password => FillMode::Password,
                FillAction::Username => FillMode::Username,
                FillAction::Both => FillMode::Both,
            };
            if let Err(message) = target.validate() {
                report(&ui, &mut current, message, watch.as_deref());
                return;
            }
            if !authorize(&ui, &mut current, ProtectedAction::Fill(record, mode)) {
                drop(current);
                ui.invoke_session_changed();
                ui.invoke_focus_search();
                return;
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
            let watch_inner = watch.clone();
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
                let outcome = if let Some(tab_sent) = pending.tab_sent {
                    match pending.target.verify_foreground() {
                        Err(error) => Some(Err(error)),
                        Ok(()) if tab_sent.elapsed() < Duration::from_millis(400) => None,
                        Ok(()) => match pending.target.after_tab() {
                            Ok(Some(target)) => Some(target.fill(&pending.password)),
                            Ok(None) if tab_sent.elapsed() < Duration::from_millis(1500) => None,
                            Ok(None) => Some(Err(
                                "Tab 未及时切换到下一输入框，已取消密码填充。请点击密码框后单独填入。"
                                    .into(),
                            )),
                            Err(message) => Some(Err(message)),
                        },
                    }
                } else if let Some(restored) = pending.restored {
                    if restored.elapsed() < Duration::from_millis(120) {
                        return;
                    }
                    match pending.target.fill(&pending.inputs) {
                        Ok(()) if !pending.password.is_empty() => {
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
                    match hide_window(&ui, watch_inner.as_deref())
                        .and_then(|_| pending.target.restore())
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
                        let app_key = state.pending.as_ref().unwrap().application.clone();
                        let mut session = session.borrow_mut();
                        if session.vault.as_mut().is_some_and(|vault| {
                            vault.record_used_in_application(record, &app_key).is_err()
                        }) {
                            session.notice = Some(
                                "上次填充已完成，但最近使用记录未保存，请检查磁盘空间或权限。"
                                    .into(),
                            );
                        } else {
                            session.invalidate_cache();
                        }
                    }
                    state.pending = None;
                    state.target = None;
                    ui.set_busy(false);
                    ui.set_can_fill(false);
                    if let Err(error) = result {
                        report(&ui, &mut state, error, watch_inner.as_deref());
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
                || ui.get_account_batch()
                || ui.get_delete_confirm()
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
            let stride = ui.get_row_stride();
            let index = (relative / stride).floor() as usize;
            let mut state = state.borrow_mut();
            let record = if relative >= 0.0 && relative % stride < ui.get_row_height() {
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
            let weak = weak.clone();
            let state = state.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    if !ui.get_compact() || ui.window().is_visible() {
                        refresh(&ui, &mut state.borrow_mut());
                    }
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
    popup_watch: Option<&Timer>,
    screen_protection: bool,
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
        if ui.get_about_open() && ui.get_about_from_settings() && ui.get_unlocked() {
            ui.set_settings_open(true);
        }
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
    ui.set_target_location(
        target
            .map(|t| t.location_source())
            .unwrap_or_default()
            .into(),
    );
    let (prefix, coordinates, suffix) = target.map(|t| t.location_details()).unwrap_or_default();
    ui.set_target_detail(TargetDetail {
        prefix: prefix.into(),
        coordinates: coordinates.into(),
        suffix: suffix.into(),
    });
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
        show_window(ui, popup_watch)?;
    }
    if let Ok(hwnd) = window_id(ui) {
        platform::set_screen_protection(hwnd, screen_protection);
    }
    if !ui.get_compact() {
        ui.invoke_restore_layout();
    }
    if ui.get_compact() {
        ui.window().set_minimized(false);
        platform::present_popup(window_id(ui)?, target)?;
    } else {
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

fn capture_window_geometry(
    ui: &AppWindow,
    cache: &Cell<Option<windows_password_manager::settings::WindowLayout>>,
    paused: &Cell<bool>,
    mode: &Cell<Option<bool>>,
    disabled: &Cell<bool>,
) -> bool {
    if ui.get_compact()
        || disabled.get()
        || paused.get()
        || mode.get() != Some(ui.get_fixed_page())
        || !ui.window().is_visible()
        || !window_id(ui).is_ok()
        || ui.window().is_minimized()
    {
        return false;
    }
    let Ok(id) = window_id(ui) else {
        return false;
    };
    let scale = ui.window().scale_factor() as f64;
    if !ui.get_fixed_page() {
        if let Some(layout) = platform::window_layout(id, scale, cache.get()) {
            cache.set(Some(layout));
            return true;
        }
    } else if let Some((left, top)) = platform::window_position(id) {
        let next = if let Some(prev) = cache.get() {
            let span_w = (prev.right - prev.left).max(1);
            let span_h = (prev.bottom - prev.top).max(1);
            windows_password_manager::settings::WindowLayout {
                left,
                top,
                right: left + span_w,
                bottom: top + span_h,
                ..prev
            }
        } else {
            let width = ui.get_desktop_width().round().clamp(640.0, 16384.0) as u32;
            let height = ui.get_desktop_height().round().clamp(560.0, 16384.0) as u32;
            let span_w = (width as f64 * scale).round().max(640.0) as i32;
            let span_h = (height as f64 * scale).round().max(560.0) as i32;
            windows_password_manager::settings::WindowLayout {
                left,
                top,
                right: left + span_w,
                bottom: top + span_h,
                width,
                height,
                maximized: false,
            }
        };
        cache.set(Some(next));
        return true;
    }
    false
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
    let geometry_cache = Rc::new(Cell::new(if reset_boot {
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
    let screen_protection = Rc::new(Cell::new(if reset_boot {
        false
    } else {
        windows_password_manager::settings::load_screen_protection(&root)?
    }));
    let auto_update = Rc::new(Cell::new(if reset_boot {
        true
    } else {
        windows_password_manager::settings::load_auto_update(&root)?
    }));
    let update_ctrl = Rc::new(RefCell::new(UpdateController {
        available: None,
        busy: false,
        cancel: Arc::new(AtomicBool::new(false)),
        serial: 0,
        pending_launch: None,
    }));
    if let Some(layout) = geometry_cache.get() {
        main.set_desktop_width(layout.width as f32);
        main.set_desktop_height(layout.height as f32);
        main.window()
            .set_position(slint::PhysicalPosition::new(layout.left, layout.top));
    }
    {
        let main_weak = main.as_weak();
        platform::set_max_hover_callback(move |hwnd, hovered| {
            let _ = main_weak.upgrade_in_event_loop(move |ui| {
                if window_id(&ui).ok() == Some(hwnd) {
                    ui.set_max_hovered(hovered);
                }
            });
        });
    }
    for ui in [&main, &popup] {
        ui.set_theme_index(theme);
        ui.set_system_dark(platform::system_dark());
        ui.set_appearance_mode(appearance);
        ui.set_screen_protection_choice(screen_protection.get());
        ui.set_auto_update_choice(auto_update.get());
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
            if window_id(&ui).is_err() {
                if let Some(layout) = cache.get() {
                    ui.window()
                        .set_position(slint::PhysicalPosition::new(layout.left, layout.top));
                }
                return;
            }
            let fixed = ui.get_fixed_page();
            let old = mode.get();
            if old == Some(fixed) {
                return;
            }
            mode.set(Some(fixed));
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
                    if let Some(layout) = cache.get() {
                        if let Ok(id) = window_id(&ui) {
                            let scale = ui.window().scale_factor() as f64;
                            if let Err(message) =
                                platform::restore_window_layout(id, layout, fixed, scale)
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
                    Timer::single_shot(Duration::from_millis(120), move || {
                        if epoch.get() == serial && !disabled.get() {
                            paused.set(false);
                            if let Some(ui) = weak.upgrade() {
                                if !ui.get_fixed_page() {
                                    if let Ok(id) = window_id(&ui) {
                                        if let Some(layout) = platform::window_layout(
                                            id,
                                            ui.window().scale_factor() as f64,
                                            cache.get(),
                                        ) {
                                            cache.set(Some(layout));
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
        let capture_protection = screen_protection.clone();
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
                    && !layout_paused.get()
                    && !layout_disabled.get()
                    && layout_mode.get() == Some(ui.get_fixed_page())
                    && (matches!(event, WindowEvent::Moved(_))
                        || (!ui.get_fixed_page() && matches!(event, WindowEvent::Resized(_))))
                {
                    if capture_window_geometry(
                        &ui,
                        &layout_cache,
                        &layout_paused,
                        &layout_mode,
                        &layout_disabled,
                    ) {
                        let cache = layout_cache.clone();
                        let disabled = layout_disabled.clone();
                        let root = layout_root.clone();
                        let weak = ui.as_weak();
                        layout_timer.start(
                            TimerMode::SingleShot,
                            Duration::from_millis(250),
                            move || {
                                if !disabled.get() && !maintenance::reset_pending(&root) {
                                    if let Some(layout) = cache.get() {
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
                WindowEvent::Focused(true)
                    | WindowEvent::Occluded(false)
                    | WindowEvent::Resized(_)
                    | WindowEvent::ScaleFactorChanged { .. }
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
                            if layout_mode.get().is_none() {
                                ui.invoke_restore_layout();
                            }
                            platform::fit_window_to_work_area(id);
                        }
                    }
                    if !ui.get_compact()
                        && !ui.get_fixed_page()
                        && let Some(size) = window.with_winit_window(|native| {
                            native.inner_size().to_logical::<f32>(native.scale_factor())
                        })
                        && size.width >= ui.get_desktop_min_width()
                        && size.height >= ui.get_desktop_min_height()
                    {
                        ui.set_list_width_basis(size.width);
                    }
                    if !ui.get_fixed_page()
                        && !layout_paused.get()
                        && layout_mode.get() == Some(false)
                        && matches!(event, WindowEvent::Resized(_))
                    {
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
                    // Apply protection after winit has created and sized the visible native window.
                    if capture_protection.get()
                        && ui.window().is_visible()
                        && let Ok(id) = window_id(&ui)
                    {
                        platform::set_screen_protection(id, true);
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
    let (jobs_tx, results) = mpsc::channel::<JobResult>();
    let jobs = JobDispatcher {
        sender: jobs_tx,
        ui: main.as_weak(),
    };
    let popup_watch_timer = Rc::new(Timer::default());
    let clipboard_seq = Rc::new(Cell::new(0u32));
    let clipboard_timer = Rc::new(Timer::default());

    let (main_state, main_timer) = bind(
        &main,
        session.clone(),
        jobs.clone(),
        clock.clone(),
        0,
        lock_signal.clone(),
        None,
        clipboard_seq.clone(),
        clipboard_timer.clone(),
    );
    let (popup_state, popup_timer) = bind(
        &popup,
        session.clone(),
        jobs.clone(),
        clock.clone(),
        1,
        lock_signal.clone(),
        Some(popup_watch_timer.clone()),
        clipboard_seq.clone(),
        clipboard_timer.clone(),
    );

    // Target-validity watcher runs ONLY while the quick-fill popup is visible.
    {
        let popup_weak = popup.as_weak();
        let popup_state = popup_state.clone();
        let watch_weak = Rc::downgrade(&popup_watch_timer);
        popup_watch_timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            let Some(popup) = popup_weak.upgrade() else {
                return;
            };
            if !popup.window().is_visible() {
                if let Some(watch) = watch_weak.upgrade() {
                    watch.stop();
                }
                return;
            }
            if !popup.get_busy() && !popup.get_verification_active() {
                let mut state = popup_state.borrow_mut();
                if let Some(error) = state.target.and_then(|target| target.validate().err()) {
                    state.target = None;
                    popup.set_can_fill(false);
                    popup.set_target_label("目标已失效，请重新点击输入框并按呼出快捷键。".into());
                    popup.set_target_detail(TargetDetail::default());
                    popup.set_status(error.into());
                }
            }
        });
        popup_watch_timer.stop();
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
        update::cleanup_staging(&root);
    }

    for ui in [&main, &popup] {
        {
            let main_weak = main.as_weak();
            let popup_weak = popup.as_weak();
            let main_state = main_state.clone();
            let popup_state = popup_state.clone();
            let native_sync = native.clone();
            let session_sync = session.clone();
            ui.on_session_changed(move || {
                native_sync.set_unlocked(session_sync.borrow().vault.is_some());
                if let (Some(main), Some(popup)) = (main_weak.upgrade(), popup_weak.upgrade()) {
                    refresh(&main, &mut main_state.borrow_mut());
                    if popup.window().is_visible() {
                        refresh(&popup, &mut popup_state.borrow_mut());
                    } else {
                        let s = session_sync.borrow();
                        popup.set_unlocked(s.vault.is_some());
                        popup.set_working(s.working);
                        popup.set_blocked(s.working || s.auth_owner.is_some());
                        popup.set_new_vault(!s.path.exists());
                    }
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
            let popup_watch = popup_watch_timer.clone();
            let native_lock = native.clone();
            let clip_seq = clipboard_seq.clone();
            let clip_timer = clipboard_timer.clone();
            let lock_layout_cache = geometry_cache.clone();
            let lock_layout_paused = geometry_paused.clone();
            let lock_layout_mode = geometry_mode.clone();
            let lock_layout_disabled = resetting.clone();
            let lock_layout_root = root.clone();
            ui.on_lock_vault(move || {
                if let Some(main_win) = main_weak.upgrade() {
                    if capture_window_geometry(
                        &main_win,
                        &lock_layout_cache,
                        &lock_layout_paused,
                        &lock_layout_mode,
                        &lock_layout_disabled,
                    ) && !maintenance::reset_pending(&lock_layout_root)
                    {
                        if let Some(layout) = lock_layout_cache.get() {
                            let _ = layout.save(&lock_layout_root);
                        }
                    }
                }
                schedule_clipboard_clear(&clip_seq, &clip_timer, Duration::ZERO);
                session.borrow_mut().lock();
                native_lock.set_unlocked(false);
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
                        ui.set_target_detail(TargetDetail::default());
                        ui.set_status("密码库已锁定。".into());
                        clear_editor(&ui);
                        clear_details(&ui);
                        state.details_id = None;
                        state.target_application.clear();
                        refresh(&ui, &mut state);
                    }
                }
                if let Some(popup) = popup_weak.upgrade() {
                    let _ = hide_window(&popup, Some(&popup_watch));
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
            let popup_watch = popup_watch_timer.clone();
            let protection = screen_protection.clone();
            ui.on_restore(move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                if session.borrow().working || session.borrow().auth_owner.is_some() {
                    return;
                }
                if let Some(popup) = popup_weak.upgrade() {
                    cancel(
                        &popup,
                        &mut popup_state.borrow_mut(),
                        &popup_timer,
                        Some(&popup_watch),
                    );
                }
                let result = (|| {
                    let folder = settings::backup_dialog_directory(&session.borrow().root)?;
                    let selected = platform::choose_backup_path(
                        window_id(&ui)?,
                        &folder,
                        BackupDialog::Restore,
                    )?;
                    if let Some(source) = selected {
                        session.borrow_mut().restore_copy(&source)?;
                        ui.invoke_lock_vault();
                        ui.invoke_session_changed();
                        if let Some(main) = main_weak.upgrade() {
                            main.set_status(
                                "已创建恢复副本。请输入备份的主密码；原密码库保持不变。".into(),
                            );
                            main.show().map_err(|e| e.to_string())?;
                            let id = window_id(&main)?;
                            platform::set_screen_protection(id, protection.get());
                            platform::activate(id);
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
        let main_state = main_state.clone();
        let popup_weak = popup.as_weak();
        let popup_state = popup_state.clone();
        let popup_timer = popup_timer.clone();
        let popup_watch = popup_watch_timer.clone();
        main.on_backup(move || {
            if session.borrow().working || session.borrow().auth_owner.is_some() {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Some(popup) = popup_weak.upgrade() {
                cancel(
                    &popup,
                    &mut popup_state.borrow_mut(),
                    &popup_timer,
                    Some(&popup_watch),
                );
            }
            let result = (|| {
                let folder = settings::backup_dialog_directory(&session.borrow().root)?;
                if let Some(path) =
                    platform::choose_backup_path(window_id(&ui)?, &folder, BackupDialog::Save)?
                {
                    session
                        .borrow_mut()
                        .vault
                        .as_mut()
                        .ok_or("密码库已锁定。".to_string())?
                        .backup(&path)?;
                    if ui.get_backups_open() {
                        if let Err(error) = refresh_backups(&ui, &mut main_state.borrow_mut()) {
                            return Err(format!("已保存加密备份，但未能刷新列表。{error}"));
                        }
                    }
                    success(&ui, format!("已保存加密备份：{}", path.display()));
                }
                Ok::<_, String>(())
            })();
            if let Err(message) = result {
                ui.set_status(message.into());
            }
        });
    }

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
            if ui.get_update_downloading() {
                ui.set_status("请先等待更新下载结束，或取消下载后再还原。".into());
                return;
            }
            let resume = ui.get_reset_incomplete();
            if !resume && (!ui.get_reset_confirmed() || ui.get_reset_master().is_empty()) {
                return;
            }
            let master = Zeroizing::new(ui.get_reset_master().to_string());
            ui.set_reset_master("".into());
            reset_secret_masks(&ui);
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
                jobs.send(JobResult::Reset(result));
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
                if ui.get_ui_blocked() || ui.get_discard_open() || !ui.get_settings_visible() {
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
        let protection = screen_protection.clone();
        let auto_update_pref = auto_update.clone();
        let save_update_ctrl = update_ctrl.clone();
        let save_jobs = jobs.clone();
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
            let theme = ui.get_theme_choice();
            let appearance = ui.get_appearance_choice();
            let protect_screen = ui.get_screen_protection_choice();
            let check_updates = ui.get_auto_update_choice();
            let backup_directory = ui.get_backup_directory_choice();
            let was_auto_update = auto_update_pref.get();
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
                    windows_password_manager::settings::save_theme(&root, theme)?;
                    windows_password_manager::settings::save_appearance(&root, appearance)?;
                    windows_password_manager::settings::save_screen_protection(
                        &root,
                        protect_screen,
                    )?;
                    windows_password_manager::settings::save_auto_update(&root, check_updates)?;
                    settings::save_backup_directory(&root, backup_directory.trim())?;
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
                let protection_changed = protection.replace(protect_screen) != protect_screen;
                auto_update_pref.set(check_updates);
                for weak in [&main_weak, &popup_weak] {
                    if let Some(window) = weak.upgrade() {
                        let colors_changed = window.get_theme_index() != theme
                            || window.get_appearance_mode() != appearance;
                        window.set_shortcut_label(shortcut.to_string().into());
                        window.set_theme_index(theme);
                        window.set_appearance_mode(appearance);
                        window.set_screen_protection_choice(protect_screen);
                        window.set_auto_update_choice(check_updates);
                        window.invoke_apply_appearance();
                        // Hidden panels apply capture protection when shown.
                        if window.window().is_visible() {
                            if protection_changed && let Ok(hwnd) = window_id(&window) {
                                platform::set_screen_protection(hwnd, protect_screen);
                            }
                            if colors_changed || protection_changed {
                                window.set_repaint_frame(!window.get_repaint_frame());
                                window.window().request_redraw();
                            }
                        }
                    }
                }
                if !was_auto_update
                    && check_updates
                    && save_update_ctrl.borrow().available.is_none()
                    && (std::env::var_os("PASSWORD_MANAGER_DATA_DIR").is_none()
                        || std::env::var_os("KEYISLE_UPDATE_URL").is_some())
                {
                    if let Some(main_win) = main_weak.upgrade() {
                        start_update_check(&main_win, &save_update_ctrl, &save_jobs, false);
                    }
                }
                Ok::<_, String>(())
            })();
            ui.invoke_session_changed();
            match result {
                Ok(()) => {
                    ui.invoke_mark_settings_saved();
                    success(&ui, "设置已保存。".into());
                }
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
        let popup_watch = popup_watch_timer.clone();
        let signal = lock_signal.clone();
        let navigate_main = main.as_weak();
        let navigate_popup = popup.as_weak();
        let navigate_main_state = main_state.clone();
        let navigate_popup_state = popup_state.clone();
        let navigate_main_timer = main_timer.clone();
        let navigate_popup_timer = popup_timer.clone();
        let navigate_popup_watch = popup_watch_timer.clone();
        let navigate_protection = screen_protection.clone();
        let context_main = main.as_weak();
        let context_popup = popup.as_weak();
        let context_main_state = main_state.clone();
        let context_popup_state = popup_state.clone();
        let context_main_timer = main_timer.clone();
        let context_popup_timer = popup_timer.clone();
        let context_popup_watch = popup_watch_timer.clone();
        let context_protection = screen_protection.clone();
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
                Some(&context_popup_watch),
            );
            if let Err(message) = show_panel(
                &main,
                &context_main_state,
                &context_main_timer,
                None,
                context_protection.get(),
                None,
            ) {
                main.set_status(message.into());
                return;
            }
            if main.get_editing()
                || main.get_account_batch()
                || main.get_verification_active()
                || main.get_blocked()
                || (main.get_settings_open() && main.get_settings_dirty())
            {
                main.set_status("请先完成主界面的操作，再从悬浮窗管理账号。".into());
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
                    Some(&navigate_popup_watch),
                );
                if let Err(message) = show_panel(
                    &main,
                    &navigate_main_state,
                    &navigate_main_timer,
                    None,
                    navigate_protection.get(),
                    None,
                ) {
                    main.set_status(message.into());
                }
            }
        });
        let reset_events = resetting.clone();
        let native_protection = screen_protection.clone();
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
                        show_panel(
                            &popup,
                            &popup_state,
                            &popup_timer,
                            Some(&popup_watch),
                            native_protection.get(),
                            target,
                        )
                    }
                    Event::ShortcutRecorded(shortcut) => {
                        main.invoke_captured_shortcut(
                            shortcut.key as i32,
                            shortcut.modifiers as i32,
                        );
                        Ok(())
                    }
                    Event::Manage => {
                        cancel(
                            &popup,
                            &mut popup_state.borrow_mut(),
                            &popup_timer,
                            Some(&popup_watch),
                        );
                        show_panel(
                            &main,
                            &main_state,
                            &main_timer,
                            None,
                            native_protection.get(),
                            None,
                        )
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

    // Manual & one-click in-app update handlers + background auto-check timers.
    let auto_check_initial = Rc::new(Timer::default());
    let auto_check_periodic = Rc::new(Timer::default());
    {
        let main_weak = main.as_weak();
        let ctrl = update_ctrl.clone();
        let jobs_ref = jobs.clone();
        main.on_check_update(move || {
            if let Some(main) = main_weak.upgrade() {
                start_update_check(&main, &ctrl, &jobs_ref, true);
            }
        });
    }
    {
        let main_weak = main.as_weak();
        let ctrl = update_ctrl.clone();
        main.on_cancel_update(move || {
            if let Some(main) = main_weak.upgrade().filter(|ui| ui.get_update_downloading()) {
                ctrl.borrow().cancel.store(true, Ordering::SeqCst);
                main.set_update_canceling(true);
                main.set_update_progress_text("正在取消下载…".into());
            }
        });
    }
    {
        let main_weak = main.as_weak();
        let ctrl = update_ctrl.clone();
        let session_ref = session.clone();
        let reset_ref = resetting.clone();
        let jobs_ref = jobs.clone();
        main.on_install_update(move || {
            let Some(main) = main_weak.upgrade() else {
                return;
            };
            if session_ref.borrow().working
                || session_ref.borrow().auth_owner.is_some()
                || reset_ref.get()
            {
                main.set_update_error("请先完成当前验证或维护操作，再执行覆盖升级。".into());
                return;
            }
            if guard_draft(&main, 4) {
                main.set_about_open(false);
                return;
            }
            let mut c = ctrl.borrow_mut();
            if c.busy {
                return;
            }
            let Some(manifest) = c.available.clone() else {
                return;
            };
            c.serial = c.serial.wrapping_add(1);
            let serial = c.serial;
            let cancel = Arc::new(AtomicBool::new(false));
            c.cancel = cancel.clone();
            c.busy = true;
            drop(c);

            main.set_update_error("".into());
            main.set_update_status("".into());
            main.set_update_downloading(true);
            main.set_update_progress(0.0);
            main.set_update_progress_text("正在连接下载源…".into());

            let root_dir = session_ref.borrow().root.clone();
            let jobs_bg = jobs_ref.clone();
            std::thread::spawn(move || {
                let mut last_tick: Option<Instant> = None;
                let progress_jobs = jobs_bg.clone();
                let result = update::prepare_update_install(
                    &root_dir,
                    &manifest,
                    &cancel,
                    |received, total| {
                        let should_emit = received == 0
                            || received == total
                            || last_tick.is_none_or(|t| t.elapsed() >= Duration::from_millis(80));
                        if should_emit {
                            last_tick = Some(Instant::now());
                            progress_jobs.send(JobResult::UpdateProgress {
                                serial,
                                received,
                                total,
                            });
                        }
                    },
                );
                jobs_bg.send(JobResult::UpdatePrepared { serial, result });
            });
        });
    }
    if std::env::var_os("PASSWORD_MANAGER_DATA_DIR").is_none()
        || std::env::var_os("KEYISLE_UPDATE_URL").is_some()
    {
        for (timer_handle, interval, repeated) in [
            (&auto_check_initial, Duration::from_secs(4), false),
            (&auto_check_periodic, Duration::from_secs(6 * 3600), true),
        ] {
            let main_weak = main.as_weak();
            let popup_weak = popup.as_weak();
            let auto_pref = auto_update.clone();
            let reset_flag = resetting.clone();
            let ctrl = update_ctrl.clone();
            let jobs_ref = jobs.clone();
            timer_handle.start(
                if repeated {
                    TimerMode::Repeated
                } else {
                    TimerMode::SingleShot
                },
                interval,
                move || {
                    if !auto_pref.get() || reset_flag.get() {
                        return;
                    }
                    let (Some(main), Some(popup)) = (main_weak.upgrade(), popup_weak.upgrade())
                    else {
                        return;
                    };
                    if popup.window().is_visible()
                        || main.get_verification_active()
                        || popup.get_verification_active()
                        || main.get_busy()
                        || popup.get_busy()
                        || main.get_working()
                    {
                        return;
                    }
                    start_update_check(&main, &ctrl, &jobs_ref, false);
                },
            );
        }
    }

    // Event-driven async job completion handler (0ms latency, zero idle polling).
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
        let job_update_ctrl = update_ctrl.clone();
        main.on_process_jobs(move || {
            let (Some(main), Some(popup)) = (main_weak.upgrade(), popup_weak.upgrade()) else {
                return;
            };
            if signal.swap(false, Ordering::SeqCst) {
                main.invoke_lock_vault();
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
                                reset_secret_masks(&main);
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
                            if let Ok(BackupOutcome::Restored(vault, path)) = result {
                                drop(vault);
                                let _ = std::fs::remove_file(path);
                            }
                            main.invoke_session_changed();
                            continue;
                        }
                        match result {
                            Ok(BackupOutcome::Restored(vault, path)) => {
                                main.invoke_lock_vault();
                                let previous = session.borrow().path.clone();
                                session.borrow_mut().path = path;
                                let result = session.borrow_mut().accept(vault);
                                if result.is_err() {
                                    session.borrow_mut().path = previous;
                                }
                                main.invoke_session_changed();
                                match result {
                                    Ok(()) => success(
                                        &main,
                                        "已验证并恢复到独立副本，原密码库与备份均保留。".into(),
                                    ),
                                    Err(e) => main.set_status(e.into()),
                                }
                                clock.touch();
                            }
                            outcome => {
                                main.invoke_session_changed();
                                let refreshed = if main.get_backups_open() {
                                    refresh_backups(&main, &mut main_state.borrow_mut())
                                } else {
                                    Ok(())
                                };
                                match outcome {
                                    Ok(BackupOutcome::Deleted(count)) => match refreshed {
                                        Ok(()) => {
                                            success(&main, format!("已删除 {count} 份备份。"))
                                        }
                                        Err(e) => main.set_status(
                                            format!("已删除 {count} 份备份。{e}").into(),
                                        ),
                                    },
                                    Err(e) => main.set_status(e.into()),
                                    Ok(BackupOutcome::Restored(..)) => unreachable!(),
                                }
                                if main.get_backups_open() {
                                    main.invoke_focus_search();
                                }
                            }
                        }
                    }
                    JobResult::Maintenance(result) => {
                        session.borrow_mut().working = false;
                        main.invoke_session_changed();
                        match result {
                            Ok(message) => success(&main, message),
                            Err(message) => main.set_status(message.into()),
                        }
                        main.invoke_focus_search();
                    }
                    JobResult::Unlock(generation, result) => {
                        let mut session = session.borrow_mut();
                        session.working = false;
                        let outcome = if generation != session.generation {
                            None
                        } else {
                            let result = master_attempt_result(&mut session, result);
                            Some(result.and_then(|vault| session.accept(vault)))
                        };
                        drop(session);
                        main.invoke_session_changed();
                        match outcome {
                            Some(Ok(())) => {
                                clock.touch();
                                success(&main, "已解锁。".into());
                                if popup.window().is_visible() {
                                    success(&popup, "已解锁。".into());
                                } else {
                                    popup.set_status("".into());
                                }
                            }
                            Some(Err(message)) => {
                                main.set_status(message.clone().into());
                                popup.set_status(message.into());
                            }
                            None => {}
                        }
                    }
                    JobResult::Verify {
                        panel,
                        serial,
                        generation,
                        master_password,
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
                        let result = if master_password {
                            master_attempt_result(&mut session.borrow_mut(), result)
                        } else {
                            result
                        };
                        let result = result.and_then(|_| {
                            if started.elapsed() >= Duration::from_secs(60) {
                                return Err("验证请求已过期，请取消后重试。".into());
                            }
                            let mut fg_ok = window_id(ui).is_ok_and(platform::is_foreground);
                            if !fg_ok && !signal.load(Ordering::SeqCst) {
                                for _ in 0..4 {
                                    std::thread::sleep(Duration::from_millis(20));
                                    if window_id(ui).is_ok_and(platform::is_foreground) {
                                        fg_ok = true;
                                        break;
                                    }
                                }
                            }
                            if signal.load(Ordering::SeqCst) || !fg_ok {
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
                                reset_secret_masks(ui);
                                ui.set_status("".into());
                                drop(state);
                                clock.touch();
                                main.invoke_session_changed();
                                let owner_state = if panel == 0 {
                                    &main_state
                                } else {
                                    &popup_state
                                };
                                if let Some(id) = action.record() {
                                    let Some(index) = state_index(owner_state, id) else {
                                        owner_state.borrow_mut().grant = None;
                                        ui.set_status("账号列表已变化，请重新选择。".into());
                                        continue;
                                    };
                                    ui.set_selected(index as i32);
                                }
                                match action {
                                    ProtectedAction::View(_) => ui.invoke_view_entry(),
                                    ProtectedAction::Edit(_) => ui.invoke_edit_entry(),
                                    ProtectedAction::Delete(request) => {
                                        let unchanged = {
                                            let state = owner_state.borrow();
                                            state.delete_request == request
                                                && !state.deleting_ids.is_empty()
                                        };
                                        if unchanged {
                                            ui.set_delete_confirm(true);
                                            ui.invoke_delete_entry();
                                        }
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
                    JobResult::UpdateCheck {
                        serial,
                        manual,
                        result,
                    } => {
                        let mut ctrl = job_update_ctrl.borrow_mut();
                        if serial != ctrl.serial {
                            continue;
                        }
                        ctrl.busy = false;
                        main.set_update_checking(false);
                        let settings_visible =
                            main.get_settings_visible() && main.window().is_visible();
                        let report = manual
                            || settings_visible
                            || (main.get_about_open() && main.window().is_visible());
                        match result {
                            Ok(CheckOutcome::Available(manifest)) => {
                                let ver = manifest.version.clone();
                                main.set_update_available(true);
                                main.set_update_version(ver.clone().into());
                                main.set_update_notes(manifest.notes.clone().into());
                                main.set_update_error("".into());
                                main.set_update_status("".into());
                                ctrl.available = Some(manifest);
                                if manual && settings_visible {
                                    success(
                                        &main,
                                        format!(
                                            "发现新版本 v{ver}，点击“前往更新”即可直接覆盖安装。"
                                        ),
                                    );
                                }
                            }
                            Ok(CheckOutcome::UpToDate) => {
                                ctrl.available = None;
                                main.set_update_available(false);
                                if report {
                                    let message = format!(
                                        "当前已是最新版本（v{}）。",
                                        env!("CARGO_PKG_VERSION")
                                    );
                                    main.set_update_error("".into());
                                    main.set_update_status(message.clone().into());
                                    if settings_visible {
                                        success(&main, message);
                                    }
                                }
                            }
                            Err(message) => {
                                if report {
                                    main.set_update_error(message.clone().into());
                                    if settings_visible {
                                        main.set_status(message.into());
                                    }
                                }
                            }
                        }
                    }
                    JobResult::UpdateProgress {
                        serial,
                        received,
                        total,
                    } => {
                        let ctrl = job_update_ctrl.borrow();
                        if serial == ctrl.serial
                            && main.get_update_downloading()
                            && !main.get_update_canceling()
                        {
                            let pct = if total > 0 {
                                (received as f32 * 100.0 / total as f32).clamp(0.0, 100.0)
                            } else {
                                0.0
                            };
                            main.set_update_progress(pct);
                            main.set_update_progress_text(
                                format!(
                                    "{:.2} / {:.2} MiB（{:.0}%）",
                                    received as f64 / 1_048_576.0,
                                    total as f64 / 1_048_576.0,
                                    pct
                                )
                                .into(),
                            );
                        }
                    }
                    JobResult::UpdatePrepared { serial, result } => {
                        let mut ctrl = job_update_ctrl.borrow_mut();
                        if serial != ctrl.serial {
                            continue;
                        }
                        ctrl.busy = false;
                        let was_canceled = ctrl.cancel.load(Ordering::SeqCst);
                        drop(ctrl);
                        main.set_update_downloading(false);
                        main.set_update_canceling(false);
                        if was_canceled {
                            main.set_update_status("已取消下载。".into());
                            continue;
                        }
                        match result {
                            Err(message) => {
                                main.set_update_error(message.into());
                            }
                            Ok(prepared) => {
                                if session.borrow().working
                                    || session.borrow().auth_owner.is_some()
                                    || reset_state.get()
                                {
                                    main.set_update_error(
                                        "请先完成当前的密码库验证或维护操作，再点击覆盖安装。"
                                            .into(),
                                    );
                                    continue;
                                }
                                if guard_draft(&main, 4) {
                                    main.set_about_open(false);
                                    continue;
                                }
                                main.set_update_installing(true);
                                job_update_ctrl.borrow_mut().pending_launch = Some(prepared);
                                let _ = slint::quit_event_loop();
                            }
                        }
                    }
                }
            }
        });
    }

    if !std::env::args().any(|arg| arg == "--autostart")
        || !startup::load_tray(&session.borrow().root)?
        || resumed_master_change
        || !native.shortcut_registered
    {
        show_panel(
            &main,
            &main_state,
            &main_timer,
            None,
            screen_protection.get(),
            None,
        )?;
    }
    if resumed_master_change {
        main.set_status("已完成上次中断的主密码切换，请使用新主密码解锁。".into());
    }
    if !native.shortcut_registered {
        main.set_status("呼出快捷键被占用，请从托盘打开主界面，在设置中修改。".into());
    }
    let result = slint::run_event_loop_until_quit().map_err(|e| e.to_string());
    auto_check_initial.stop();
    auto_check_periodic.stop();
    update_ctrl.borrow().cancel.store(true, Ordering::SeqCst);
    geometry_timer.stop();
    let _ = capture_window_geometry(
        &main,
        &geometry_cache,
        &geometry_paused,
        &geometry_mode,
        &resetting,
    );
    let layout_result = if !resetting.get() && !maintenance::reset_pending(&root) {
        geometry_cache
            .get()
            .map_or(Ok(()), |layout| layout.save(&root))
    } else {
        Ok(())
    };
    clipboard_timer.stop();
    for _ in 0..20 {
        if platform::clear_clipboard_if_matches(clipboard_seq.get()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    main_timer.stop();
    popup_timer.stop();
    popup_watch_timer.stop();
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
    if let Some(prepared) = update_ctrl.borrow_mut().pending_launch.take() {
        update::launch_prepared_update(&prepared)?;
    } else if restart_after_reset.get() {
        std::process::Command::new(
            std::env::current_exe().map_err(|_| "无法定位程序以重新启动。")?,
        )
        .spawn()
        .map_err(|_| "还原已完成，请手动重新启动Keyisle。")?;
    }
    result.and(layout_result)
}
