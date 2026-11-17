//! Shortcut capture is isolated from the UI message loop and exists only while recording.
use std::{
    cell::Cell,
    io::{BufRead, BufReader, Read, Write},
    mem::zeroed,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows_password_manager::shortcut::Shortcut;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{JobObjects::*, LibraryLoader::GetModuleHandleW, Threading::CREATE_NO_WINDOW},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

struct Context {
    owner: usize,
    modifiers: Cell<u16>,
    captured: Cell<Option<Shortcut>>,
}
thread_local! { static CONTEXT: Cell<*const Context> = const { Cell::new(std::ptr::null()) }; }

fn modifier_bit(key: u32) -> u16 {
    match key as u16 {
        VK_CONTROL | VK_LCONTROL => 1,
        VK_RCONTROL => 2,
        VK_MENU | VK_LMENU => 4,
        VK_RMENU => 8,
        VK_SHIFT | VK_LSHIFT => 16,
        VK_RSHIFT => 32,
        VK_LWIN => 64,
        VK_RWIN => 128,
        _ => 0,
    }
}

unsafe extern "system" fn keyboard(code: i32, message: usize, data: isize) -> isize {
    unsafe {
        if code == HC_ACTION as i32 {
            let handled = CONTEXT.with(|slot| {
                let Some(context) = slot.get().as_ref() else {
                    return false;
                };
                let event = &*(data as *const KBDLLHOOKSTRUCT);
                let down = matches!(message as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
                let bit = modifier_bit(event.vkCode);
                let bits = if down {
                    context.modifiers.get() | bit
                } else {
                    context.modifiers.get() & !bit
                };
                context.modifiers.set(bits);
                if let Some(shortcut) = context.captured.get() {
                    return shortcut.key == event.vkCode;
                }
                if !down || GetForegroundWindow() as usize != context.owner {
                    return false;
                }
                // The current event precedes async state updates; preceding modifiers can still be sampled.
                let held = |key: u16| GetAsyncKeyState(key as i32) < 0;
                let shortcut = Shortcut {
                    key: event.vkCode,
                    modifiers: ((bits & 12 != 0
                        || held(VK_MENU)
                        || event.flags & LLKHF_ALTDOWN != 0)
                        as u32)
                        | (((bits & 3 != 0 || held(VK_CONTROL)) as u32) * 2)
                        | (((bits & 48 != 0 || held(VK_SHIFT)) as u32) * 4)
                        | (((bits & 192 != 0 || held(VK_LWIN) || held(VK_RWIN)) as u32) * 8),
                };
                if !shortcut.valid() {
                    return false;
                }
                context.captured.set(Some(shortcut));
                PostQuitMessage(0);
                true
            });
            if handled {
                return 1;
            }
        }
        CallNextHookEx(null_mut(), code, message, data)
    }
}

// The helper starts before Slint or its accessibility bridge initializes.
pub fn hook_helper(owner: usize) -> i32 {
    unsafe {
        if owner == 0 || IsWindow(owner as _) == 0 {
            return 2;
        }
        let context = Context {
            owner,
            modifiers: Cell::new(0),
            captured: Cell::new(None),
        };
        let mut message: MSG = zeroed();
        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
        CONTEXT.with(|slot| slot.set(&context));
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard),
            GetModuleHandleW(null_mut()),
            0,
        );
        if hook.is_null() {
            CONTEXT.with(|slot| slot.set(null()));
            return 2;
        }
        let timer = SetTimer(null_mut(), 0, 100, None);
        if timer == 0 {
            UnhookWindowsHookEx(hook);
            CONTEXT.with(|slot| slot.set(null()));
            return 2;
        }
        println!("READY");
        let _ = std::io::stdout().flush();
        let started = Instant::now();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            if message.message == WM_TIMER
                && (started.elapsed() >= Duration::from_secs(90)
                    || GetForegroundWindow() as usize != owner)
            {
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        KillTimer(null_mut(), timer);
        UnhookWindowsHookEx(hook);
        CONTEXT.with(|slot| slot.set(null()));
        if let Some(shortcut) = context.captured.get() {
            println!("CAPTURED {shortcut}");
            let _ = std::io::stdout().flush();
            0
        } else {
            1
        }
    }
}

struct Job(HANDLE);
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub struct Recorder {
    child: Child,
    _job: Job,
    worker: Option<thread::JoinHandle<()>>,
}
impl Recorder {
    pub fn start(
        owner: usize,
        notify: impl Fn(Result<Shortcut, String>) + Send + 'static,
    ) -> Result<Self, String> {
        let job = Job(unsafe { CreateJobObjectW(null(), null()) });
        if job.0.is_null() {
            return Err("无法创建录制进程保护。".into());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err("无法保护录制进程。".into());
        }
        let child = Command::new(std::env::current_exe().map_err(|_| "无法定位录制程序。")?)
            .arg("--shortcut-record-helper")
            .arg(owner.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|_| "无法启动按键录制。")?;
        let mut recorder = Self {
            child,
            _job: job,
            worker: None,
        };
        if unsafe { AssignProcessToJobObject(recorder._job.0, recorder.child.as_raw_handle()) } == 0
        {
            return Err("无法保护录制进程。".into());
        }
        let output = recorder.child.stdout.take().ok_or("录制通信不可用。")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        recorder.worker = Some(thread::spawn(move || {
            let mut output = BufReader::new(output.take(128));
            let mut line = String::new();
            let ready = output.read_line(&mut line).is_ok() && line.trim_end() == "READY";
            let _ = sender.send(ready);
            if !ready {
                return;
            }
            line.clear();
            let result = output
                .read_line(&mut line)
                .map_err(|_| "录制通信已中断。".to_string())
                .and_then(|_| {
                    line.trim_end()
                        .strip_prefix("CAPTURED ")
                        .ok_or_else(|| "录制已取消或超时，请重新录制。".to_string())
                })
                .and_then(str::parse::<Shortcut>);
            notify(result);
        }));
        if receiver.recv_timeout(Duration::from_secs(3)) != Ok(true) {
            return Err("录制未能就绪，请重试。".into());
        }
        Ok(recorder)
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
