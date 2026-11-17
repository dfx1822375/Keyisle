use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authentication::Identity::*, Credentials::*, *},
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
        Threading::{GetCurrentProcess, GetCurrentThreadId, OpenProcessToken},
    },
    UI::WindowsAndMessaging::*,
};
use zeroize::Zeroize;
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct Buffer {
    pointer: *mut std::ffi::c_void,
    length: u32,
}
impl Drop for Buffer {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(self.pointer.cast::<u8>(), self.length as usize)
                    .zeroize();
                CoTaskMemFree(self.pointer);
            }
        }
    }
}
pub struct Credentials {
    buffer: Buffer,
    package: u32,
    expected: Vec<usize>,
}
struct Lsa(HANDLE);
impl Drop for Lsa {
    fn drop(&mut self) {
        unsafe {
            LsaDeregisterLogonProcess(self.0);
        }
    }
}
struct Token(HANDLE);
impl Drop for Token {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn token_user(token: HANDLE) -> Result<Vec<usize>, String> {
    unsafe {
        let mut length = 0;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut length);
        if GetLastError() != ERROR_INSUFFICIENT_BUFFER
            || !(size_of::<TOKEN_USER>() as u32..=65536).contains(&length)
        {
            return Err("无法读取 Windows 用户身份。".into());
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        ) == 0
        {
            return Err("无法读取 Windows 用户身份。".into());
        }
        Ok(buffer)
    }
}
fn same_user(left: &[usize], right: &[usize]) -> bool {
    if left.len() * size_of::<usize>() < size_of::<TOKEN_USER>()
        || right.len() * size_of::<usize>() < size_of::<TOKEN_USER>()
    {
        return false;
    }
    unsafe {
        let left = &*left.as_ptr().cast::<TOKEN_USER>();
        let right = &*right.as_ptr().cast::<TOKEN_USER>();
        !left.User.Sid.is_null()
            && !right.User.Sid.is_null()
            && EqualSid(left.User.Sid, right.User.Sid) != 0
    }
}
fn current_user() -> Result<Vec<usize>, String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("无法读取当前 Windows 账户。".into());
    }
    let token = Token(token);
    token_user(token.0)
}
pub fn prompt(
    owner: usize,
    title: &str,
    cancel: Arc<AtomicBool>,
    locked: Arc<AtomicBool>,
) -> Result<Credentials, String> {
    if cancel.load(Ordering::SeqCst) || locked.load(Ordering::SeqCst) {
        return Err("验证已取消。".into());
    }
    if unsafe { GetWindowThreadProcessId(owner as HWND, null_mut()) != GetCurrentThreadId() } {
        return Err("系统验证必须由窗口线程打开。".into());
    }
    if unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) } < 0 {
        return Err("无法初始化 Windows 验证窗口，请使用主密码。".into());
    }
    let _apartment = Apartment;
    let expected = current_user()?;
    let caption: Vec<u16> = "Keyisle · 验证当前 Windows 账户"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let message: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    let mut buffer = Buffer {
        pointer: null_mut(),
        length: 0,
    };
    let mut package = 0;
    let info = CREDUI_INFOW {
        cbSize: size_of::<CREDUI_INFOW>() as u32,
        hwndParent: owner as HWND,
        pszCaptionText: caption.as_ptr(),
        pszMessageText: message.as_ptr(),
        hbmBanner: null_mut(),
    };
    let result = unsafe {
        CredUIPromptForWindowsCredentialsW(
            &info,
            0,
            &mut package,
            null(),
            0,
            &mut buffer.pointer,
            &mut buffer.length,
            null_mut(),
            CREDUIWIN_ENUMERATE_CURRENT_USER,
        )
    };
    if cancel.load(Ordering::SeqCst) || locked.load(Ordering::SeqCst) {
        return Err("系统验证已取消或过期。".into());
    }
    if result != ERROR_SUCCESS || buffer.pointer.is_null() || buffer.length == 0 {
        return Err(format!(
            "Windows 验证已取消或失败（{result}），未执行操作。"
        ));
    }
    Ok(Credentials {
        buffer,
        package,
        expected,
    })
}
impl Credentials {
    pub fn verify(self, cancel: Arc<AtomicBool>, locked: Arc<AtomicBool>) -> Result<(), String> {
        if cancel.load(Ordering::SeqCst) || locked.load(Ordering::SeqCst) {
            return Err("验证已取消。".into());
        }
        let Self {
            buffer,
            package,
            expected,
        } = self;
        unsafe {
            let mut handle = null_mut();
            let status = LsaConnectUntrusted(&mut handle);
            if status != 0 {
                return Err("无法连接 Windows 验证服务。".into());
            }
            let lsa = Lsa(handle);
            let name = b"PasswordHelper\0";
            let origin = LSA_STRING {
                Length: 14,
                MaximumLength: 15,
                Buffer: name.as_ptr().cast_mut(),
            };
            let mut source: TOKEN_SOURCE = zeroed();
            for (dst, src) in source.SourceName.iter_mut().zip(b"PwdMgr") {
                *dst = *src as i8;
            }
            if AllocateLocallyUniqueId(&mut source.SourceIdentifier) == 0 {
                return Err("无法准备系统验证。".into());
            }
            let mut profile = null_mut();
            let mut profile_len = 0;
            let mut luid = zeroed();
            let mut token = null_mut();
            let mut quota = zeroed();
            let mut substatus = 0;
            let status = LsaLogonUser(
                lsa.0,
                &origin,
                SECURITY_LOGON_TYPE::Interactive,
                package,
                buffer.pointer,
                buffer.length,
                null(),
                &source,
                &mut profile,
                &mut profile_len,
                &mut luid,
                &mut token,
                &mut quota,
                &mut substatus,
            );
            if !profile.is_null() {
                LsaFreeReturnBuffer(profile);
            }
            let token = if token.is_null() {
                None
            } else {
                Some(Token(token))
            };
            if status != 0 {
                return Err(format!(
                    "Windows 验证未通过（{}），请重试或改用主密码。",
                    LsaNtStatusToWinError(status)
                ));
            }
            let token = token.ok_or("Windows 未返回验证身份。")?;
            if !same_user(&expected, &token_user(token.0)?) {
                return Err("仅允许当前 Windows 账户授权。".into());
            }
        }
        if cancel.load(Ordering::SeqCst) || locked.load(Ordering::SeqCst) {
            return Err("验证已取消。".into());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verified_identity_must_be_current_user() {
        let current = current_user().unwrap();
        let another_copy = current_user().unwrap();
        assert!(same_user(&current, &another_copy));
        assert!(!same_user(&[], &current));
        let mut sid = [0usize; 16];
        let mut length = std::mem::size_of_val(&sid) as u32;
        assert_ne!(
            unsafe {
                CreateWellKnownSid(
                    WinBuiltinAdministratorsSid,
                    null_mut(),
                    sid.as_mut_ptr().cast(),
                    &mut length,
                )
            },
            0
        );
        let fake = TOKEN_USER {
            User: SID_AND_ATTRIBUTES {
                Sid: sid.as_mut_ptr().cast(),
                Attributes: 0,
            },
        };
        let fake = unsafe {
            std::slice::from_raw_parts(
                (&fake as *const TOKEN_USER).cast::<usize>(),
                size_of::<TOKEN_USER>() / size_of::<usize>(),
            )
        };
        assert!(!same_user(&current, fake));
        assert!(
            prompt(
                0,
                "test",
                Arc::new(AtomicBool::new(true)),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
    }
}

pub fn request(
    owner: usize,
    title: &str,
    cancel: Arc<AtomicBool>,
    locked: Arc<AtomicBool>,
) -> Result<(), String> {
    use std::{
        io::{Read, Write},
        os::windows::{io::AsRawHandle, process::CommandExt},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::System::{JobObjects::*, Threading::CREATE_NO_WINDOW};
    if cancel.load(Ordering::SeqCst) || locked.load(Ordering::SeqCst) {
        return Err("验证已取消。".into());
    }
    let mut child = Command::new(std::env::current_exe().map_err(|_| "无法定位系统验证程序。")?)
        .arg("--windows-auth-helper")
        .arg(owner.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "无法启动 Windows 验证。")?;
    let result = (|| {
        let job = unsafe { CreateJobObjectW(null(), null()) };
        if job.is_null() {
            return Err("无法创建验证进程保护。".into());
        }
        let _job = Token(job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0
        {
            return Err("无法保护 Windows 验证进程，请改用主密码。".into());
        }
        unsafe {
            AllowSetForegroundWindow(child.id());
        }
        let text: String = title.chars().take(512).collect();
        let bytes = text.as_bytes();
        let mut input = child.stdin.take().ok_or("验证通信不可用。")?;
        input
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .and_then(|_| input.write_all(bytes))
            .map_err(|_| "无法发送验证请求。")?;
        drop(input);
        let started = Instant::now();
        loop {
            if cancel.load(Ordering::SeqCst)
                || locked.load(Ordering::SeqCst)
                || started.elapsed() >= Duration::from_secs(60)
            {
                return Err("系统验证已取消或过期。".into());
            }
            if let Some(status) = child.try_wait().map_err(|_| "无法读取系统验证结果。")?
            {
                if !status.success() {
                    return Err("Windows 验证失败或已取消，可重试或使用主密码。".into());
                }
                let mut result = [0u8; 1];
                child
                    .stdout
                    .take()
                    .ok_or("验证通信已中断。")?
                    .read_exact(&mut result)
                    .map_err(|_| "验证结果不完整。")?;
                return if result == [1] {
                    Ok(())
                } else {
                    Err("Windows 未授权当前操作。".into())
                };
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    })();
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}
pub fn helper() -> i32 {
    use std::io::{Read, Write};
    let result = (|| {
        let owner = std::env::args()
            .nth(2)
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or("missing owner")?;
        let mut length = [0u8; 4];
        std::io::stdin()
            .read_exact(&mut length)
            .map_err(|_| "missing request")?;
        let length = u32::from_le_bytes(length) as usize;
        if length > 4096 {
            return Err("request too long");
        }
        let mut text = vec![0u8; length];
        std::io::stdin()
            .read_exact(&mut text)
            .map_err(|_| "missing request")?;
        let title = String::from_utf8(text).map_err(|_| "invalid request")?;
        let mut bounds: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(owner as HWND, &mut bounds) } == 0 {
            return Err("owner closed");
        }
        let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
        let proxy = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class.as_ptr(),
                null(),
                WS_POPUP | WS_VISIBLE,
                (bounds.left + bounds.right) / 2,
                (bounds.top + bounds.bottom) / 2,
                0,
                0,
                owner as HWND,
                null_mut(),
                null_mut(),
                null(),
            )
        };
        if proxy.is_null() {
            return Err("cannot create owner");
        }
        unsafe {
            SetForegroundWindow(proxy);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let locked = Arc::new(AtomicBool::new(false));
        let verified = prompt(proxy as usize, &title, cancelled.clone(), locked.clone())
            .and_then(|credentials| credentials.verify(cancelled, locked));
        unsafe {
            DestroyWindow(proxy);
        }
        verified.map_err(|_| "verification denied")?;
        let mut output = std::io::stdout();
        output
            .write_all(&[1])
            .and_then(|_| output.flush())
            .map_err(|_| "cannot return result")?;
        Ok::<_, &str>(())
    })();
    if result.is_ok() { 0 } else { 1 }
}
