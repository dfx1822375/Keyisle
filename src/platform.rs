use std::{
    cell::{Cell, RefCell},
    mem::{size_of, zeroed},
    ptr::null_mut,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    },
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard,
            RegisterClipboardFormatW, SetClipboardData,
        },
        Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
        },
        LibraryLoader::{GetModuleHandleW, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        RemoteDesktop::*,
        SystemInformation::GetTickCount,
        Threading::{AttachThreadInput, CreateMutexW, GetCurrentProcessId, GetCurrentThreadId},
    },
    UI::{
        HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
        Input::KeyboardAndMouse::*,
        Shell::*,
        WindowsAndMessaging::*,
    },
};
use zeroize::Zeroize;

use windows_password_manager::shortcut::Shortcut;

const POPUP_ESCAPE: u32 = WM_APP + 7;
const WATCH_POPUP: u32 = WM_APP + 8;
const SET_UNLOCKED: u32 = WM_APP + 9;
static NATIVE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static MAX_HOVER_HWND: AtomicUsize = AtomicUsize::new(0);
static MAX_HOVER_CALLBACK: Mutex<Option<Box<dyn Fn(usize, bool) + Send>>> = Mutex::new(None);

pub fn harden_process() {
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
}

pub fn set_screen_protection(window: usize, enabled: bool) {
    unsafe {
        let affinity = if enabled {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };
        let window = window as HWND;
        let mut current = WDA_NONE;
        if GetWindowDisplayAffinity(window, &mut current) == 0 || current != affinity {
            let _ = SetWindowDisplayAffinity(window, affinity);
        }
    }
}

pub fn set_max_hover_callback(callback: impl Fn(usize, bool) + Send + 'static) {
    *MAX_HOVER_CALLBACK.lock().unwrap() = Some(Box::new(callback));
}

fn set_dword_clipboard(format: u32, value: u32) -> bool {
    if format == 0 {
        return false;
    }
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, size_of::<u32>());
        if memory.is_null() {
            return false;
        }
        let ptr = GlobalLock(memory);
        if ptr.is_null() {
            let _ = GlobalFree(memory);
            return false;
        }
        *ptr.cast::<u32>() = value;
        let _ = GlobalUnlock(memory);
        if SetClipboardData(format, memory).is_null() {
            let _ = GlobalFree(memory);
            return false;
        }
        true
    }
}

/// Writes text to the clipboard excluded from Windows Clipboard History and cloud sync.
pub fn copy_protected_text(owner: usize, text: &str) -> Result<u32, String> {
    const CF_UNICODETEXT_ID: u32 = 13;
    if text.is_empty() || text.len() > 65536 {
        return Err("无可复制的有效内容。".into());
    }
    unsafe {
        let mut opened = false;
        for _ in 0..4 {
            if OpenClipboard(owner as HWND) != 0 {
                opened = true;
                break;
            }
            thread::sleep(Duration::from_millis(15));
        }
        if !opened {
            return Err("剪贴板正被其他程序占用，请稍后重试。".into());
        }
        let result = (|| {
            if EmptyClipboard() == 0 {
                return Err("无法清空系统剪贴板。".into());
            }
            let exclude = RegisterClipboardFormatW(
                wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr(),
            );
            let history = RegisterClipboardFormatW(wide("CanIncludeInClipboardHistory").as_ptr());
            let cloud = RegisterClipboardFormatW(wide("CanUploadToCloudClipboard").as_ptr());
            if !set_dword_clipboard(exclude, 1)
                || !set_dword_clipboard(history, 0)
                || !set_dword_clipboard(cloud, 0)
            {
                return Err("无法阻止剪贴板历史或云同步，已取消复制。".into());
            }
            let mut units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let byte_len = units.len() * size_of::<u16>();
            let memory = GlobalAlloc(GMEM_MOVEABLE, byte_len);
            if memory.is_null() {
                units.zeroize();
                return Err("无法分配剪贴板内存。".into());
            }
            let ptr = GlobalLock(memory);
            if ptr.is_null() {
                units.zeroize();
                let _ = GlobalFree(memory);
                return Err("无法写入剪贴板内存。".into());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), ptr.cast::<u16>(), units.len());
            let _ = GlobalUnlock(memory);
            units.zeroize();
            if SetClipboardData(CF_UNICODETEXT_ID, memory).is_null() {
                let _ = GlobalFree(memory);
                return Err("写入系统剪贴板失败。".into());
            }
            Ok(GetClipboardSequenceNumber())
        })();
        let _ = CloseClipboard();
        result
    }
}

pub fn clear_clipboard_if_matches(expected_sequence: u32) -> bool {
    unsafe {
        if expected_sequence == 0 || GetClipboardSequenceNumber() != expected_sequence {
            return true;
        }
        if OpenClipboard(null_mut()) == 0 {
            return false;
        }
        let cleared = GetClipboardSequenceNumber() != expected_sequence || EmptyClipboard() != 0;
        let _ = CloseClipboard();
        cleared
    }
}
thread_local! {
    static ESCAPE_WINDOW: Cell<usize> = const { Cell::new(0) };
    static ESCAPE_POPUP: Cell<usize> = const { Cell::new(0) };
    static ESCAPE_HELD: Cell<bool> = const { Cell::new(false) };
}
unsafe extern "system" fn popup_escape_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && (*(lparam as *const KBDLLHOOKSTRUCT)).vkCode == VK_ESCAPE as u32 {
            let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
            let up = wparam as u32 == WM_KEYUP || wparam as u32 == WM_SYSKEYUP;
            let consumed = ESCAPE_HELD.with(Cell::get);
            let popup = ESCAPE_POPUP.with(Cell::get) as HWND;
            if down && (consumed || (!popup.is_null() && IsWindowVisible(popup) != 0)) {
                if !consumed {
                    ESCAPE_HELD.with(|held| held.set(true));
                    ESCAPE_WINDOW.with(|window| {
                        PostMessageW(window.get() as HWND, POPUP_ESCAPE, 0, 0);
                    });
                }
                return 1;
            }
            if up && consumed {
                ESCAPE_HELD.with(|held| held.set(false));
                if popup.is_null() {
                    ESCAPE_WINDOW.with(|window| {
                        PostMessageW(window.get() as HWND, WATCH_POPUP, 0, 0);
                    });
                }
                return 1;
            }
        }
        CallNextHookEx(null_mut(), code, wparam, lparam)
    }
}

pub fn watch_popup(popup: Option<usize>) -> Result<(), String> {
    unsafe {
        let mut window = NATIVE_WINDOW.load(Ordering::SeqCst) as HWND;
        if window.is_null() {
            window = FindWindowW(
                wide("PasswordManagerPrototypeEvents").as_ptr(),
                wide("PasswordManagerPrototypeEvents").as_ptr(),
            );
        }
        if window.is_null() {
            return Err("全局 Esc 服务不可用。".into());
        }
        let mut result = 0;
        if SendMessageTimeoutW(
            window,
            WATCH_POPUP,
            popup.unwrap_or(0),
            0,
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            250,
            &mut result,
        ) == 0
            || result == 0
        {
            return Err("无法启用悬浮框的全局 Esc，请使用关闭按钮，或回到悬浮框按 Esc。".into());
        }
        Ok(())
    }
}

pub fn open_web_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.len() > 16384
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
        || url
            .split_once("://")
            .is_none_or(|(_, host)| host.is_empty())
    {
        return Err("只支持打开完整的 HTTP 或 HTTPS 网址，请编辑此账号检查网址。".into());
    }
    unsafe {
        if ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            wide(url).as_ptr(),
            null_mut(),
            null_mut(),
            SW_SHOWNORMAL,
        ) as isize
            <= 32
        {
            return Err(failure("无法打开默认浏览器"));
        }
    }
    Ok(())
}

const HOTKEY: i32 = 1;
const CHANGE_SHORTCUT: u32 = WM_APP + 2;
const COMMIT_SHORTCUT: u32 = WM_APP + 3;
const ABORT_SHORTCUT: u32 = WM_APP + 4;
const RECORD_SHORTCUT: u32 = WM_APP + 5;
const STOP_NATIVE: u32 = WM_APP + 6;
const RECORD_TIMER: usize = 3;
const TRAY_MESSAGE: u32 = WM_APP + 1;
const IDLE_TIMER: usize = 2;
const OPEN: usize = 1;
const EXIT: usize = 2;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn failure(action: &str) -> String {
    format!("{action}（Windows 错误 {}）", unsafe { GetLastError() })
}

#[derive(Clone, Copy)]
pub struct Target {
    window: usize,
    focus: usize,
    process: u32,
    thread: u32,
    captured: Instant,
    location: Option<(i32, i32, bool)>,
}

impl Target {
    pub fn capture() -> Option<Self> {
        unsafe {
            let window = GetForegroundWindow();
            let mut process = 0;
            let thread = GetWindowThreadProcessId(window, &mut process);
            if window.is_null() || thread == 0 || process == GetCurrentProcessId() {
                return None;
            }
            let mut info: GUITHREADINFO = zeroed();
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            if GetGUIThreadInfo(thread, &mut info) == 0 || info.hwndFocus.is_null() {
                return None;
            }
            let mut point: POINT = zeroed();
            let caret = !info.hwndCaret.is_null() && info.rcCaret.bottom > info.rcCaret.top;
            let location = if caret {
                point.x = info.rcCaret.left;
                point.y = info.rcCaret.top;
                (ClientToScreen(info.hwndCaret, &mut point) != 0)
                    .then_some((point.x, point.y, true))
            } else {
                (GetCursorPos(&mut point) != 0).then_some((point.x, point.y, false))
            };
            Some(Self {
                location,
                window: window as usize,
                focus: info.hwndFocus as usize,
                process,
                thread,
                captured: Instant::now(),
            })
        }
    }

    pub fn application_key(&self) -> Option<String> {
        unsafe {
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
            };
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, self.process);
            if process.is_null() {
                return None;
            }
            let mut path = [0u16; 32768];
            let mut length = path.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length);
            CloseHandle(process);
            if ok == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&path[..length as usize]);
            std::path::Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
        }
    }
    pub fn app_name(&self) -> String {
        unsafe {
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
            };
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, self.process);
            if process.is_null() {
                return "目标应用".into();
            }
            let mut path = [0u16; 32768];
            let mut length = path.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length);
            CloseHandle(process);
            if ok == 0 {
                return "目标应用".into();
            }
            let path = String::from_utf16_lossy(&path[..length as usize]);
            let name = std::path::Path::new(&path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            match name.to_ascii_lowercase().as_str() {
                "chrome" => "Chrome".into(),
                "atrust" => "aTrust".into(),
                _ => name.chars().filter(|c| !c.is_control()).take(48).collect(),
            }
        }
    }

    pub fn description(&self) -> String {
        let mut title = [0u16; 1024];
        let len =
            unsafe { GetWindowTextW(self.window as HWND, title.as_mut_ptr(), title.len() as i32) };
        let title = String::from_utf16_lossy(&title[..len.max(0) as usize])
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(160)
            .collect::<String>();
        if title.trim().is_empty() {
            format!("填充至 {}", self.app_name())
        } else {
            format!("{} · {}", self.app_name(), title.trim())
        }
    }

    pub fn location_source(&self) -> &'static str {
        match self.location {
            Some((_, _, true)) => "原输入框",
            Some((_, _, false)) => "原窗口 · 鼠标位置",
            None => "原窗口 · 无光标位置",
        }
    }

    pub fn location_details(&self) -> (&'static str, String, &'static str) {
        match self.location {
            Some((x, y, true)) => ("输入光标位置 ", format!("({x}, {y})"), " · 原输入框"),
            Some((x, y, false)) => ("呼出时鼠标 ", format!("({x}, {y})"), " · 已记录原输入窗口"),
            None => ("已记录原输入窗口；当前位置不可读取", String::new(), ""),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.captured.elapsed() > Duration::from_secs(60) {
            return Err("目标已过期，请重新点击输入框并按快捷键。".into());
        }
        unsafe {
            let mut process = 0;
            let thread = GetWindowThreadProcessId(self.window as HWND, &mut process);
            if IsWindow(self.window as HWND) == 0
                || IsWindow(self.focus as HWND) == 0
                || process != self.process
                || thread != self.thread
            {
                return Err("目标窗口已关闭或发生变化，已取消填充。".into());
            }
        }
        Ok(())
    }

    pub fn restore(&self) -> Result<(), String> {
        self.validate()?;
        unsafe {
            if IsIconic(self.window as HWND) != 0 || SetForegroundWindow(self.window as HWND) == 0 {
                return Err("无法恢复原窗口，请重新定位输入框后再试。".into());
            }
            // Foreground activation across input queues is asynchronous; wait before checking focus.
            let mut response = 0;
            if SendMessageTimeoutW(
                self.window as HWND,
                WM_NULL,
                0,
                0,
                SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                250,
                &mut response,
            ) == 0
            {
                return Err("目标应用暂未响应，已取消填充。请重新点击输入框后重试。".into());
            }
            self.verify_foreground()?;
            let mut info: GUITHREADINFO = zeroed();
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            if GetGUIThreadInfo(self.thread, &mut info) != 0
                && info.hwndFocus as usize == self.focus
            {
                return Ok(());
            }
            let current = GetCurrentThreadId();
            if AttachThreadInput(current, self.thread, 1) == 0 {
                return Err(failure("无法恢复原输入框的焦点"));
            }
            // SetFocus requires attached input queues when the saved control belongs to another thread.
            SetFocus(self.focus as HWND);
            let focused = GetGUIThreadInfo(self.thread, &mut info) != 0
                && info.hwndFocus as usize == self.focus;
            let detached = AttachThreadInput(current, self.thread, 0) != 0;
            if !focused || !detached {
                return Err("无法恢复原输入框的焦点，已取消填充。".into());
            }
        }
        Ok(())
    }

    pub fn verify_foreground(&self) -> Result<(), String> {
        self.validate()?;
        if !is_foreground(self.window) {
            return Err("填充期间目标窗口已切换，已取消后续输入。".into());
        }
        Ok(())
    }

    pub fn after_tab(&self) -> Result<Option<Self>, String> {
        self.verify_foreground()?;
        let next = Self::capture().ok_or("无法确认切换后的输入框，已取消密码填充。")?;
        if next.window != self.window || next.process != self.process || next.thread != self.thread
        {
            return Err("切换后的输入框不属于原窗口，已取消密码填充。".into());
        }
        unsafe {
            if next.focus != self.window && IsChild(self.window as HWND, next.focus as HWND) == 0 {
                return Err("切换后的输入框不属于原窗口，已取消密码填充。".into());
            }
        }
        if next.focus == self.focus {
            unsafe {
                let mut class = [0u16; 256];
                let len = GetClassNameW(self.focus as HWND, class.as_mut_ptr(), class.len() as i32);
                if len == 0 {
                    return Err("无法识别输入框，已取消密码填充。".into());
                }
                // Native edit fields have separate HWNDs; wait for Tab before sending a password.
                if String::from_utf16_lossy(&class[..len as usize])
                    .to_lowercase()
                    .contains("edit")
                {
                    return Ok(None);
                }
            }
        }
        Ok(Some(Self {
            focus: next.focus,
            ..*self
        }))
    }

    pub fn fill(&self, units: &[windows_password_manager::InputUnit]) -> Result<(), String> {
        self.verify_foreground()?;
        unsafe {
            let mut info: GUITHREADINFO = zeroed();
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            if GetForegroundWindow() as usize != self.window
                || GetGUIThreadInfo(self.thread, &mut info) == 0
                || info.hwndFocus as usize != self.focus
            {
                return Err("输入焦点已变化，已取消填充。请重新点击目标输入框。".into());
            }
            if !trigger_keys_released() {
                return Err("检测到按键仍被按下，已取消填充。".into());
            }
            // shortcut: browser fields can share one HWND; require a new hotkey if the page changes.
            let mut inputs = Vec::with_capacity(units.len() * 2);
            for unit in units {
                let (virtual_key, scan, base_flags) = match unit {
                    windows_password_manager::InputUnit::Unicode(unit) => {
                        (0, *unit, KEYEVENTF_UNICODE)
                    }
                    windows_password_manager::InputUnit::Tab => (VK_TAB, 0, 0),
                };
                for flags in [base_flags, base_flags | KEYEVENTF_KEYUP] {
                    inputs.push(INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: virtual_key,
                                wScan: scan,
                                dwFlags: flags,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }
            }
            // Never retry a partial send; the caller separates fields around an intentional Tab.
            let sent = SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            );
            let expected = inputs.len() as u32;
            std::slice::from_raw_parts_mut(
                inputs.as_mut_ptr().cast::<u8>(),
                inputs.len() * size_of::<INPUT>(),
            )
            .zeroize();
            if sent != expected {
                return Err(
                    "输入未完整发送，可能被目标软件或权限阻止。请检查输入框；不会自动重试。".into(),
                );
            }
        }
        Ok(())
    }
}

pub fn trigger_keys_released() -> bool {
    [
        VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_RETURN, VK_TAB, VK_LBUTTON, VK_RBUTTON,
    ]
    .into_iter()
    .chain(0x30..=0x5a)
    .chain(0x70..=0x87)
    .all(|key| unsafe { GetAsyncKeyState(key as i32) >= 0 })
}

pub fn is_foreground(window: usize) -> bool {
    unsafe { GetForegroundWindow() as usize == window }
}

pub fn activate(window: usize) {
    unsafe {
        if IsIconic(window as HWND) != 0 {
            ShowWindow(window as HWND, SW_RESTORE);
        }
        SetForegroundWindow(window as HWND);
    }
}

pub fn present_popup(window: usize, target: Option<Target>) -> Result<(), String> {
    unsafe {
        let hwnd = window as HWND;
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ((style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW) as isize,
        );
        let monitor = MonitorFromWindow(
            target.map(|t| t.window as HWND).unwrap_or(hwnd),
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        let mut bounds: RECT = zeroed();
        if GetMonitorInfoW(monitor, &mut info) == 0 || GetWindowRect(hwnd, &mut bounds) == 0 {
            return Err(failure("无法定位悬浮搜索框"));
        }
        let mut monitor_dpi_x = 0u32;
        let mut monitor_dpi_y = 0u32;
        let window_dpi = GetDpiForWindow(hwnd);
        let (popup_w, popup_h) = if window_dpi > 0
            && GetDpiForMonitor(
                monitor,
                MDT_EFFECTIVE_DPI,
                &mut monitor_dpi_x,
                &mut monitor_dpi_y,
            ) == 0
            && monitor_dpi_x > 0
        {
            let ratio = monitor_dpi_x as f64 / window_dpi as f64;
            (
                ((bounds.right - bounds.left) as f64 * ratio).round() as i32,
                ((bounds.bottom - bounds.top) as f64 * ratio).round() as i32,
            )
        } else {
            (bounds.right - bounds.left, bounds.bottom - bounds.top)
        };
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left - popup_w) / 2).max(0);
        let y = work.top + ((work.bottom - work.top - popup_h) / 4).max(0);
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err(failure("无法定位悬浮搜索框"));
        }
    }
    activate(window);
    Ok(())
}

pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(message).as_ptr(),
            wide("Keyisle").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub enum Event {
    DismissPopup,
    Search(Option<Target>),
    Manage,
    Quit,
    Lock,
    ShortcutRecorded(Shortcut),
}
struct Context {
    notify: Box<dyn Fn(Event) + Send>,
    taskbar_message: u32,
    idle_notified: Cell<bool>,
    clock: Arc<windows_password_manager::security::LockClock>,
    shortcut: Cell<Shortcut>,
    hotkey_id: Cell<i32>,
    staged: Cell<Option<(Shortcut, i32)>>,
    recording: Cell<u8>,
    escape_hook: Cell<HHOOK>,
}

unsafe fn tray(window: HWND, action: u32) -> bool {
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = window;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        data.hIcon = LoadImageW(
            GetModuleHandleW(null_mut()),
            1usize as *const u16,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_SHARED,
        );
        if data.hIcon.is_null() {
            data.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        }
        let tip = wide("Keyisle · 点击管理，右键退出");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(action, &data) != 0
    }
}

// Cancel only our native modal dialogs; querying a Slint window title here can block its UI thread.
fn close_file_dialogs() {
    unsafe extern "system" fn visit(window: HWND, _: LPARAM) -> i32 {
        unsafe {
            let mut process = 0;
            GetWindowThreadProcessId(window, &mut process);
            if process == GetCurrentProcessId() {
                let mut class = [0u16; 32];
                let count = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
                if String::from_utf16_lossy(&class[..count.max(0) as usize]) == "#32770" {
                    PostMessageW(window, WM_CLOSE, 0, 0);
                }
            }
            1
        }
    }
    unsafe {
        EnumWindows(Some(visit), 0);
    }
}

fn notify_lock(context: &Context) {
    (context.notify)(Event::Lock);
    close_file_dialogs();
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let context = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Context;
        if !context.is_null() {
            let context = &*context;
            if message == context.taskbar_message {
                tray(window, NIM_ADD);
                return 0;
            }
            match message {
                WATCH_POPUP => {
                    ESCAPE_WINDOW.with(|value| value.set(window as usize));
                    ESCAPE_POPUP.with(|value| value.set(wparam));
                    if wparam != 0 && context.escape_hook.get().is_null() {
                        let hook = SetWindowsHookExW(
                            WH_KEYBOARD_LL,
                            Some(popup_escape_hook),
                            GetModuleHandleW(null_mut()),
                            0,
                        );
                        if hook.is_null() {
                            return 0;
                        }
                        context.escape_hook.set(hook);
                    } else if wparam == 0 && !ESCAPE_HELD.with(Cell::get) {
                        let hook = context.escape_hook.replace(null_mut());
                        if !hook.is_null() {
                            UnhookWindowsHookEx(hook);
                        }
                    }
                    return 1;
                }
                POPUP_ESCAPE => {
                    (context.notify)(Event::DismissPopup);
                    return 0;
                }
                RECORD_SHORTCUT => {
                    context.recording.set(if wparam != 0 { 1 } else { 2 });
                    if wparam != 0 {
                        KillTimer(window, RECORD_TIMER);
                    } else {
                        SetTimer(window, RECORD_TIMER, 20, None);
                    }
                    return 0;
                }
                WM_TIMER if wparam == RECORD_TIMER => {
                    if trigger_keys_released() {
                        context.recording.set(0);
                        KillTimer(window, RECORD_TIMER);
                    }
                    return 0;
                }
                COMMIT_SHORTCUT => {
                    if let Some((shortcut, id)) = context.staged.take() {
                        if context.hotkey_id.get() != 0 {
                            UnregisterHotKey(window, context.hotkey_id.get());
                        }
                        context.hotkey_id.set(id);
                        context.shortcut.set(shortcut);
                    }
                    return 0;
                }
                ABORT_SHORTCUT => {
                    if let Some((_, id)) = context.staged.take() {
                        UnregisterHotKey(window, id);
                    }
                    return 0;
                }
                SET_UNLOCKED => {
                    if wparam != 0 {
                        context.idle_notified.set(false);
                        SetTimer(window, IDLE_TIMER, 1000, None);
                    } else {
                        context.idle_notified.set(true);
                        KillTimer(window, IDLE_TIMER);
                    }
                    return 0;
                }
                WM_TIMER if wparam == IDLE_TIMER => {
                    if idle_expired(&context.clock) {
                        if !context.idle_notified.replace(true) {
                            notify_lock(context);
                        }
                    } else {
                        context.idle_notified.set(false);
                    }
                    return 0;
                }
                CHANGE_SHORTCUT => {
                    let shortcut = Shortcut {
                        modifiers: wparam as u32,
                        key: lparam as u32,
                    };
                    if !shortcut.valid() {
                        return 3;
                    }
                    if shortcut == context.shortcut.get() && context.hotkey_id.get() != 0 {
                        return 0;
                    }
                    let next_id = if context.hotkey_id.get() == HOTKEY {
                        HOTKEY + 1
                    } else {
                        HOTKEY
                    };
                    if RegisterHotKey(
                        window,
                        next_id,
                        shortcut.modifiers | MOD_NOREPEAT,
                        shortcut.key,
                    ) == 0
                    {
                        return 1;
                    }
                    context.staged.set(Some((shortcut, next_id)));
                    return 0;
                }
                WM_HOTKEY if wparam == context.hotkey_id.get() as usize => {
                    if context.recording.get() != 0 {
                        if context.recording.get() == 1 {
                            (context.notify)(Event::ShortcutRecorded(context.shortcut.get()));
                        }
                        return 0;
                    }
                    // Capture before the Slint window takes foreground focus.
                    (context.notify)(Event::Search(Target::capture()));
                    return 0;
                }
                TRAY_MESSAGE => {
                    match lparam as u32 {
                        WM_LBUTTONUP => (context.notify)(Event::Manage),
                        WM_RBUTTONUP => {
                            let menu = CreatePopupMenu();
                            AppendMenuW(menu, MF_STRING, OPEN, wide("打开主界面").as_ptr());
                            AppendMenuW(menu, MF_STRING, EXIT, wide("退出").as_ptr());
                            let mut point: POINT = zeroed();
                            GetCursorPos(&mut point);
                            SetForegroundWindow(window);
                            let choice = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_NONOTIFY,
                                point.x,
                                point.y,
                                0,
                                window,
                                null_mut(),
                            );
                            DestroyMenu(menu);
                            PostMessageW(window, WM_NULL, 0, 0);
                            if choice as usize == OPEN {
                                (context.notify)(Event::Manage);
                            }
                            if choice as usize == EXIT {
                                close_file_dialogs();
                                (context.notify)(Event::Quit);
                            }
                        }
                        _ => (),
                    }
                    return 0;
                }
                WM_WTSSESSION_CHANGE
                    if wparam as u32 == WTS_SESSION_LOCK
                        || wparam as u32 == WTS_SESSION_LOGOFF
                        || wparam as u32 == WTS_REMOTE_DISCONNECT
                        || wparam as u32 == WTS_CONSOLE_DISCONNECT =>
                {
                    notify_lock(context);
                    return 0;
                }
                WM_POWERBROADCAST if wparam as u32 == PBT_APMSUSPEND => {
                    notify_lock(context);
                    return 1;
                }
                STOP_NATIVE => {
                    close_file_dialogs();
                    PostQuitMessage(0);
                    return 0;
                }
                WM_CLOSE => {
                    close_file_dialogs();
                    (context.notify)(Event::Quit);
                    return 0;
                }
                _ => (),
            }
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}

pub fn backup_time(nanos: u128) -> String {
    use windows_sys::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx},
    };
    let Some(ticks) = (nanos / 100)
        .checked_add(116_444_736_000_000_000)
        .and_then(|n| u64::try_from(n).ok())
    else {
        return "时间不可用".into();
    };
    let file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { FileTimeToSystemTime(&file, &mut utc) } == 0
        || unsafe { SystemTimeToTzSpecificLocalTimeEx(std::ptr::null(), &utc, &mut local) } == 0
    {
        return "时间不可用".into();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

pub fn shortcut_key(code: slint::winit_030::winit::keyboard::KeyCode) -> Option<u32> {
    use slint::winit_030::winit::keyboard::KeyCode::*;
    for (keys, base) in [
        (
            &[
                KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN,
                KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
            ][..],
            0x41,
        ),
        (
            &[
                Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
            ][..],
            0x30,
        ),
        (
            &[
                F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18,
                F19, F20, F21, F22, F23, F24,
            ][..],
            0x70,
        ),
    ] {
        if let Some(index) = keys.iter().position(|key| *key == code) {
            return Some(base + index as u32);
        }
    }
    None
}

pub struct Native {
    window: usize,
    worker: RefCell<Option<thread::JoinHandle<()>>>,
    pub shortcut_registered: bool,
}

impl Native {
    pub fn prepare_shortcut(&self, shortcut: Shortcut) -> Result<(), String> {
        if !shortcut.valid() {
            return Err("快捷键无效。".into());
        }
        match unsafe {
            SendMessageW(
                self.window as HWND,
                CHANGE_SHORTCUT,
                shortcut.modifiers as usize,
                shortcut.key as isize,
            )
        } {
            0 => Ok(()),
            1 => Err("快捷键已被占用或由系统保留，原快捷键保持有效。".into()),
            _ => Err("快捷键无效。".into()),
        }
    }
    pub fn commit_shortcut(&self) {
        unsafe {
            SendMessageW(self.window as HWND, COMMIT_SHORTCUT, 0, 0);
        }
    }
    pub fn abort_shortcut(&self) {
        unsafe {
            SendMessageW(self.window as HWND, ABORT_SHORTCUT, 0, 0);
        }
    }
    pub fn record_shortcut(&self, active: bool) {
        unsafe {
            SendMessageW(self.window as HWND, RECORD_SHORTCUT, active as usize, 0);
        }
    }
    pub fn set_unlocked(&self, unlocked: bool) {
        unsafe {
            PostMessageW(self.window as HWND, SET_UNLOCKED, unlocked as usize, 0);
        }
    }
    pub fn start(
        clock: Arc<windows_password_manager::security::LockClock>,
        root: std::path::PathBuf,
        notify: impl Fn(Event) + Send + 'static,
    ) -> Result<Option<Self>, String> {
        let shortcut = if windows_password_manager::maintenance::reset_pending(&root) {
            Shortcut::default()
        } else {
            Shortcut::load(&root)?
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || unsafe {
            let mutex = CreateMutexW(
                null_mut(),
                0,
                wide("Local\\WindowsPasswordManagerPrototype").as_ptr(),
            );
            if mutex.is_null() {
                let _ = sender.send(Err(failure("无法创建单实例锁")));
                return;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex);
                let mut opened = false;
                for _ in 0..40 {
                    let existing = FindWindowW(
                        wide("PasswordManagerPrototypeEvents").as_ptr(),
                        wide("PasswordManagerPrototypeEvents").as_ptr(),
                    );
                    if !existing.is_null() {
                        let mut process = 0;
                        GetWindowThreadProcessId(existing, &mut process);
                        AllowSetForegroundWindow(process);
                        opened =
                            PostMessageW(existing, TRAY_MESSAGE, 0, WM_LBUTTONUP as isize) != 0;
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                let _ = sender.send(if opened {
                    Ok(None)
                } else {
                    Err("已有实例正在启动，请稍后再打开。".into())
                });
                return;
            }
            let instance = GetModuleHandleW(null_mut());
            let class_name = wide("PasswordManagerPrototypeEvents");
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..zeroed()
            };
            if RegisterClassW(&class) == 0 {
                let _ = sender.send(Err(failure("无法注册窗口类")));
                CloseHandle(mutex);
                return;
            }
            let mut context = Box::new(Context {
                notify: Box::new(notify),
                taskbar_message: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
                idle_notified: Cell::new(false),
                clock,
                shortcut: Cell::new(shortcut),
                hotkey_id: Cell::new(HOTKEY),
                staged: Cell::new(None),
                recording: Cell::new(0),
                escape_hook: Cell::new(null_mut()),
            });
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                class_name.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                (&mut *context as *mut Context).cast(),
            );
            if window.is_null() {
                let _ = sender.send(Err(failure("无法创建事件窗口")));
                CloseHandle(mutex);
                return;
            }
            NATIVE_WINDOW.store(window as usize, Ordering::SeqCst);
            let registered = RegisterHotKey(
                window,
                HOTKEY,
                shortcut.modifiers | MOD_NOREPEAT,
                shortcut.key,
            ) != 0;
            if !registered {
                context.hotkey_id.set(0);
            }
            if !tray(window, NIM_ADD) {
                let _ = sender.send(Err(failure("无法创建托盘图标")));
                NATIVE_WINDOW.store(0, Ordering::SeqCst);
                UnregisterHotKey(window, context.hotkey_id.get());
                DestroyWindow(window);
                CloseHandle(mutex);
                return;
            }
            if WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) == 0 {
                let _ = sender.send(Err(failure("无法注册系统锁屏通知")));
                NATIVE_WINDOW.store(0, Ordering::SeqCst);
                tray(window, NIM_DELETE);
                UnregisterHotKey(window, context.hotkey_id.get());
                DestroyWindow(window);
                CloseHandle(mutex);
                return;
            }
            let _ = sender.send(Ok(Some((window as usize, registered))));
            let mut message: MSG = zeroed();
            loop {
                let result = GetMessageW(&mut message, null_mut(), 0, 0);
                if result <= 0 {
                    if result < 0 {
                        show_error(&failure("Windows 消息循环失败"));
                        (context.notify)(Event::Quit);
                    }
                    break;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if !context.escape_hook.get().is_null() {
                UnhookWindowsHookEx(context.escape_hook.get());
            }
            ESCAPE_POPUP.with(|popup| popup.set(0));
            NATIVE_WINDOW.store(0, Ordering::SeqCst);
            KillTimer(window, IDLE_TIMER);
            WTSUnRegisterSessionNotification(window);
            tray(window, NIM_DELETE);
            UnregisterHotKey(window, context.hotkey_id.get());
            DestroyWindow(window);
            UnregisterClassW(class_name.as_ptr(), instance);
            CloseHandle(mutex);
        });
        match receiver
            .recv()
            .map_err(|_| "Windows 事件线程启动失败。".to_string())?
        {
            Ok(Some((window, registered))) => Ok(Some(Self {
                window,
                worker: RefCell::new(Some(worker)),
                shortcut_registered: registered,
            })),
            Ok(None) => {
                let _ = worker.join();
                Ok(None)
            }
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
}

impl Native {
    pub fn stop(&self) {
        if let Some(worker) = self.worker.borrow_mut().take() {
            unsafe {
                PostMessageW(self.window as HWND, STOP_NATIVE, 0, 0);
            }
            let _ = worker.join();
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn idle_expired(clock: &windows_password_manager::security::LockClock) -> bool {
    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut info) == 0 {
            return clock.expired(None);
        }
        let mut process = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut process);
        clock.observe_input(info.dwTime, process == GetCurrentProcessId());
        clock.expired(Some(GetTickCount().wrapping_sub(info.dwTime) / 1000))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileDialog {
    Save,
    Restore,
    Directory,
    Wallpaper,
}

pub fn choose_path(
    owner: usize,
    initial: &std::path::Path,
    mode: FileDialog,
) -> Result<Option<std::path::PathBuf>, String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::{
        Win32::{
            Foundation::HWND,
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize,
            },
            UI::Shell::{
                Common::COMDLG_FILTERSPEC, FOS_DONTADDTORECENT, FOS_FORCEFILESYSTEM,
                FOS_NOCHANGEDIR, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileDialog,
                IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
            },
        },
        core::{HRESULT, PCWSTR, w},
    };
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|_| "无法初始化 Windows 文件选择器。")?;
        let result = (|| -> windows::core::Result<_> {
            let class = if mode == FileDialog::Save {
                &FileSaveDialog
            } else {
                &FileOpenDialog
            };
            let dialog: IFileDialog = CoCreateInstance(class, None, CLSCTX_INPROC_SERVER)?;
            let mut flags =
                dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR | FOS_DONTADDTORECENT;
            if mode == FileDialog::Directory {
                flags |= FOS_PICKFOLDERS;
            } else if mode == FileDialog::Wallpaper {
                dialog.SetFileTypes(&[COMDLG_FILTERSPEC {
                    pszName: w!("PNG / JPEG 图片"),
                    pszSpec: w!("*.png;*.jpg;*.jpeg"),
                }])?;
            } else {
                dialog.SetFileTypes(&[
                    COMDLG_FILTERSPEC {
                        pszName: w!("加密密码库 (*.db)"),
                        pszSpec: w!("*.db"),
                    },
                    COMDLG_FILTERSPEC {
                        pszName: w!("所有文件"),
                        pszSpec: w!("*.*"),
                    },
                ])?;
                dialog.SetDefaultExtension(w!("db"))?;
            }
            dialog.SetOptions(flags)?;
            dialog.SetTitle(match mode {
                FileDialog::Save => w!("保存加密备份（请选择新文件）"),
                FileDialog::Restore => w!("选择要恢复的加密备份"),
                FileDialog::Directory => w!("选择默认备份目录"),
                FileDialog::Wallpaper => w!("选择背景图片"),
            })?;
            let path: Vec<_> = initial.as_os_str().encode_wide().chain([0]).collect();
            let folder: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None)?;
            dialog.SetFolder(&folder)?;
            if let Err(error) = dialog.Show(Some(HWND(owner as _))) {
                return if error.code() == HRESULT::from_win32(1223) {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let selected = std::ffi::OsString::from_wide(name.as_wide()).into();
            CoTaskMemFree(Some(name.0.cast()));
            Ok(Some(selected))
        })();
        CoUninitialize();
        result.map_err(|error| format!("无法打开文件选择器（{}）。", error.code()))
    }
}

pub fn begin_hello(
    owner: usize,
    title: &str,
) -> Result<
    windows_future::IAsyncOperation<
        windows::Security::Credentials::UI::UserConsentVerificationResult,
    >,
    String,
> {
    use windows::{
        Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier},
        Win32::{
            Foundation::HWND,
            System::WinRT::{
                IUserConsentVerifierInterop, RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize,
            },
        },
        core::{HSTRING, factory},
    };
    // The system prompt must be requested on the thread that owns its parent window.
    if unsafe { GetWindowThreadProcessId(owner as _, null_mut()) != GetCurrentThreadId() }
        || !is_foreground(owner)
    {
        return Err("请回到Keyisle再启动系统验证。".into());
    }
    unsafe {
        RoInitialize(RO_INIT_SINGLETHREADED)
            .map_err(|_| "无法初始化 Windows 系统验证，请改用主密码。")?;
        let result = (|| {
            let interop: IUserConsentVerifierInterop =
                factory::<UserConsentVerifier, IUserConsentVerifierInterop>().map_err(|error| {
                    format!(
                        "Windows 验证接口不可用（0x{:08X}），请改用主密码。",
                        error.code().0 as u32
                    )
                })?;
            let operation: windows_future::IAsyncOperation<UserConsentVerificationResult> = interop
                .RequestVerificationForWindowAsync(HWND(owner as _), &HSTRING::from(title))
                .map_err(|error| {
                    format!(
                        "无法打开 Windows PIN / Hello（0x{:08X}）。请改用主密码。",
                        error.code().0 as u32
                    )
                })?;
            Ok(operation)
        })();
        RoUninitialize();
        result
    }
}

pub fn finish_hello(
    operation: windows_future::IAsyncOperation<
        windows::Security::Credentials::UI::UserConsentVerificationResult,
    >,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    locked: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    use windows::{
        Security::Credentials::UI::UserConsentVerificationResult,
        Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    };
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED).map_err(|_| "无法等待 Windows 验证，请改用主密码。")?;
        let result: Result<(), String> = (|| {
            let started = Instant::now();
            while operation
                .Status()
                .map_err(|_| "无法读取 Windows Hello 状态。")?
                == windows_future::AsyncStatus::Started
            {
                if cancel.load(std::sync::atomic::Ordering::SeqCst)
                    || locked.load(std::sync::atomic::Ordering::SeqCst)
                    || started.elapsed() >= Duration::from_secs(60)
                {
                    let _ = operation.Cancel();
                    return Err("验证已取消或超时。".into());
                }
                thread::sleep(Duration::from_millis(50));
            }
            let result = operation
                .join()
                .map_err(|_| "Windows Hello 验证失败，请重试或改用主密码。")?;
            if result == UserConsentVerificationResult::Verified {
                Ok(())
            } else {
                let reason = if result == UserConsentVerificationResult::DeviceNotPresent {
                    "系统未提供可用的 PIN / Hello 验证设备"
                } else if result == UserConsentVerificationResult::NotConfiguredForUser {
                    "当前 Windows 用户未配置 PIN / Hello"
                } else if result == UserConsentVerificationResult::DisabledByPolicy {
                    "PIN / Hello 验证被系统策略禁用"
                } else if result == UserConsentVerificationResult::DeviceBusy {
                    "系统验证设备正忙"
                } else if result == UserConsentVerificationResult::RetriesExhausted {
                    "系统验证重试次数已用尽"
                } else {
                    "系统验证未通过或已取消"
                };
                Err(format!(
                    "{reason}（结果 {}）；未执行操作，可输入主密码。",
                    result.0
                ))
            }
        })();
        RoUninitialize();
        result
    }
}

pub fn round_window(window: usize, radius: f32, scale: f64) {
    unsafe {
        let window = window as HWND;
        if IsIconic(window) != 0 {
            return;
        }
        if IsZoomed(window) != 0 {
            windows_sys::Win32::Graphics::Gdi::SetWindowRgn(window, null_mut(), 1);
            return;
        }
        let mut bounds: RECT = zeroed();
        if GetWindowRect(window, &mut bounds) == 0 {
            return;
        }
        let diameter = (radius as f64 * scale * 2.0).round() as i32;
        let region = windows_sys::Win32::Graphics::Gdi::CreateRoundRectRgn(
            0,
            0,
            bounds.right - bounds.left + 1,
            bounds.bottom - bounds.top + 1,
            diameter,
            diameter,
        );
        if !region.is_null()
            && windows_sys::Win32::Graphics::Gdi::SetWindowRgn(window, region, 1) == 0
        {
            windows_sys::Win32::Graphics::Gdi::DeleteObject(region);
        }
    }
}

unsafe extern "system" fn frameless_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    frame: usize,
) -> LRESULT {
    unsafe {
        if message == WM_STYLECHANGING && wparam == GWL_STYLE as usize {
            let style = &mut *(lparam as *mut STYLESTRUCT);
            style.styleNew = (style.styleNew & !(WS_CAPTION | WS_BORDER)) | WS_POPUP;
        }
        if frame != 0 && message == WM_NCLBUTTONDBLCLK && wparam == HTCAPTION as usize {
            PostMessageW(
                window,
                WM_SYSCOMMAND,
                if IsZoomed(window) == 0 {
                    SC_MAXIMIZE
                } else {
                    SC_RESTORE
                } as usize,
                0,
            );
            return 0;
        }
        // Suppress native nonclient drawing and input around our own controls.
        if IsIconic(window) == 0 {
            if message == WM_NCPAINT || (message == WM_NCCALCSIZE && wparam == 0) {
                return 0;
            }
            if message == WM_NCHITTEST {
                if frame != 0 {
                    let scale = frame as f64 / 1000.0;
                    let mut bounds: RECT = zeroed();
                    if GetWindowRect(window, &mut bounds) != 0 {
                        // Signed screen coordinates also work on monitors left/above the primary.
                        let x = (lparam as u16 as i16) as i32 - bounds.left;
                        let y = ((lparam as u32 >> 16) as u16 as i16) as i32 - bounds.top;
                        let width = bounds.right - bounds.left;
                        let height = bounds.bottom - bounds.top;
                        if IsZoomed(window) == 0 {
                            let border = (6.0 * scale).ceil() as i32;
                            let left = x < border;
                            let right = x >= width - border;
                            let top = y < border;
                            let bottom = y >= height - border;
                            let hit = match (left, right, top, bottom) {
                                (true, _, true, _) => HTTOPLEFT,
                                (_, true, true, _) => HTTOPRIGHT,
                                (true, _, _, true) => HTBOTTOMLEFT,
                                (_, true, _, true) => HTBOTTOMRIGHT,
                                (true, _, _, _) => HTLEFT,
                                (_, true, _, _) => HTRIGHT,
                                (_, _, true, _) => HTTOP,
                                (_, _, _, true) => HTBOTTOM,
                                _ => HTCLIENT,
                            };
                            if hit != HTCLIENT {
                                return hit as LRESULT;
                            }
                        }
                        if x >= width - (80.0 * scale) as i32
                            && x < width - (44.0 * scale) as i32
                            && y >= (16.0 * scale) as i32
                            && y < (46.0 * scale) as i32
                        {
                            return HTMAXBUTTON as LRESULT;
                        }
                        if x < width - (236.0 * scale) as i32 && y < (52.0 * scale) as i32 {
                            return HTCAPTION as LRESULT;
                        }
                    }
                }
                if MAX_HOVER_HWND.swap(0, Ordering::SeqCst) == window as usize {
                    if let Some(cb) = MAX_HOVER_CALLBACK.lock().unwrap().as_ref() {
                        cb(window as usize, false);
                    }
                }
                return HTCLIENT as LRESULT;
            }
            if message == WM_NCMOUSEMOVE {
                let on_max = frame != 0 && wparam == HTMAXBUTTON as usize;
                let target = if on_max { window as usize } else { 0 };
                let prev = MAX_HOVER_HWND.swap(target, Ordering::SeqCst);
                if on_max && prev != window as usize {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_NONCLIENT | TME_LEAVE,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                    if let Some(cb) = MAX_HOVER_CALLBACK.lock().unwrap().as_ref() {
                        cb(window as usize, true);
                    }
                } else if !on_max && prev == window as usize {
                    if let Some(cb) = MAX_HOVER_CALLBACK.lock().unwrap().as_ref() {
                        cb(window as usize, false);
                    }
                }
            }
            if message == 0x02A2 /* WM_NCMOUSELEAVE */ || message == WM_MOUSEMOVE {
                if MAX_HOVER_HWND.swap(0, Ordering::SeqCst) == window as usize {
                    if let Some(cb) = MAX_HOVER_CALLBACK.lock().unwrap().as_ref() {
                        cb(window as usize, false);
                    }
                }
            }
            if frame != 0 && wparam == HTMAXBUTTON as usize {
                if message == WM_NCLBUTTONDOWN {
                    return 0;
                }
                if message == WM_NCLBUTTONUP {
                    PostMessageW(
                        window,
                        WM_SYSCOMMAND,
                        if IsZoomed(window) == 0 {
                            SC_MAXIMIZE
                        } else {
                            SC_RESTORE
                        } as usize,
                        0,
                    );
                    return 0;
                }
            }
            if message == WM_NCACTIVATE {
                return DefSubclassProc(window, message, wparam, -1);
            }
        }
        if message == WM_NCDESTROY {
            RemoveWindowSubclass(window, Some(frameless_proc), id);
        }
        DefSubclassProc(window, message, wparam, lparam)
    }
}

pub fn configure_frameless_window(
    window: usize,
    resizable: bool,
    scale: f64,
) -> Result<(), String> {
    unsafe {
        let window = window as HWND;
        if SetWindowSubclass(
            window,
            Some(frameless_proc),
            1,
            if resizable {
                (scale * 1000.0).round() as usize
            } else {
                0
            },
        ) == 0
        {
            return Err(failure("无法设置自绘窗口"));
        }
        if IsIconic(window) == 0 {
            let style = GetWindowLongPtrW(window, GWL_STYLE);
            let caption = (WS_CAPTION | WS_BORDER) as isize;
            let frameless = (style & !caption) | WS_POPUP as isize;
            if style != frameless {
                SetWindowLongPtrW(window, GWL_STYLE, frameless);
                SetWindowPos(
                    window,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        Ok(())
    }
}

pub fn system_menu(window: usize) {
    unsafe {
        let window = window as HWND;
        let mut bounds: RECT = zeroed();
        if GetWindowRect(window, &mut bounds) == 0 {
            return;
        }
        let menu = GetSystemMenu(window, 0);
        if menu.is_null() {
            return;
        }
        let action = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            bounds.left + 12,
            bounds.top + 40,
            0,
            window,
            std::ptr::null(),
        );
        if action != 0 {
            PostMessageW(window, WM_SYSCOMMAND, action as usize, 0);
        }
    }
}

pub fn fit_window_to_work_area(window: usize) {
    unsafe {
        let window = window as HWND;
        if IsIconic(window) != 0 || IsZoomed(window) != 0 {
            return;
        }
        let mut bounds: RECT = zeroed();
        let mut monitor: MONITORINFO = zeroed();
        monitor.cbSize = size_of::<MONITORINFO>() as u32;
        if GetWindowRect(window, &mut bounds) == 0
            || GetMonitorInfoW(
                MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            ) == 0
        {
            return;
        }
        let area = monitor.rcWork;
        let x = bounds.left.clamp(
            area.left,
            (area.right - (bounds.right - bounds.left)).max(area.left),
        );
        let y = bounds.top.clamp(
            area.top,
            (area.bottom - (bounds.bottom - bounds.top)).max(area.top),
        );
        if x != bounds.left || y != bounds.top {
            SetWindowPos(
                window,
                null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

pub fn system_dark() -> bool {
    use windows_sys::Win32::System::Registry::*;
    unsafe {
        let mut value = 1u32;
        let mut size = size_of::<u32>() as u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            wide("AppsUseLightTheme").as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        ) == 0
            && value == 0
    }
}

pub fn window_position(window: usize) -> Option<(i32, i32)> {
    unsafe {
        let window = window as HWND;
        if IsIconic(window) != 0 || IsZoomed(window) != 0 {
            return None;
        }
        let mut bounds: RECT = zeroed();
        if GetWindowRect(window, &mut bounds) == 0 {
            return None;
        }
        Some((bounds.left, bounds.top))
    }
}

pub fn window_layout(
    window: usize,
    scale: f64,
    previous: Option<windows_password_manager::settings::WindowLayout>,
) -> Option<windows_password_manager::settings::WindowLayout> {
    unsafe {
        let window = window as HWND;
        if IsIconic(window) != 0 {
            return None;
        }
        if IsZoomed(window) != 0 {
            if let Some(prev) = previous {
                return Some(windows_password_manager::settings::WindowLayout {
                    maximized: true,
                    ..prev
                });
            }
            let mut place: WINDOWPLACEMENT = zeroed();
            place.length = size_of::<WINDOWPLACEMENT>() as u32;
            if GetWindowPlacement(window, &mut place) == 0 {
                return None;
            }
            let bounds = place.rcNormalPosition;
            let phys_w = (bounds.right - bounds.left).max(1);
            let phys_h = (bounds.bottom - bounds.top).max(1);
            return Some(windows_password_manager::settings::WindowLayout {
                left: bounds.left,
                top: bounds.top,
                right: bounds.left + phys_w,
                bottom: bounds.top + phys_h,
                width: (phys_w as f64 / scale).round().clamp(640.0, 16384.0) as u32,
                height: (phys_h as f64 / scale).round().clamp(560.0, 16384.0) as u32,
                maximized: true,
            });
        }
        let mut bounds: RECT = zeroed();
        if GetWindowRect(window, &mut bounds) == 0 {
            return None;
        }
        let phys_w = bounds.right - bounds.left;
        let phys_h = bounds.bottom - bounds.top;
        if phys_w <= 0 || phys_h <= 0 {
            return None;
        }
        Some(windows_password_manager::settings::WindowLayout {
            left: bounds.left,
            top: bounds.top,
            right: bounds.right,
            bottom: bounds.bottom,
            width: (phys_w as f64 / scale).round().clamp(640.0, 16384.0) as u32,
            height: (phys_h as f64 / scale).round().clamp(560.0, 16384.0) as u32,
            maximized: false,
        })
    }
}

pub fn restore_window_layout(
    window: usize,
    layout: windows_password_manager::settings::WindowLayout,
    fixed: bool,
    scale: f64,
) -> Result<(), String> {
    unsafe {
        let window = window as HWND;
        if IsIconic(window) != 0 || (IsZoomed(window) != 0 && (fixed || !layout.maximized)) {
            ShowWindow(window, SW_RESTORE);
        }
        if fixed {
            if SetWindowPos(
                window,
                null_mut(),
                layout.left,
                layout.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            ) == 0
            {
                return Err(failure("无法恢复窗口位置"));
            }
            fit_window_to_work_area(window as usize);
        } else {
            let width = (layout.width as f64 * scale).round().max(1.0) as i32;
            let height = (layout.height as f64 * scale).round().max(1.0) as i32;
            if SetWindowPos(
                window,
                null_mut(),
                layout.left,
                layout.top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            ) == 0
            {
                return Err(failure("无法恢复窗口布局"));
            }
            fit_window_to_work_area(window as usize);
            if layout.maximized {
                ShowWindow(window, SW_MAXIMIZE);
            }
        }
        Ok(())
    }
}
