use std::{fs, path::Path};
#[cfg(windows)]
use std::{fs::OpenOptions, io::Write};

pub fn load_tray(root: &Path) -> Result<bool, String> {
    match fs::read_to_string(root.join("launch-settings.txt")) {
        Ok(text) => match text.as_str() {
            "tray=0\n" => Ok(false),
            "tray=1\n" => Ok(true),
            _ => Err("启动设置损坏，请保留文件并检查。".into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("无法读取启动设置。".into()),
    }
}

#[cfg(windows)]
mod registry {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::*,
    };
    const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const NAME: &str = "WindowsPasswordManager";
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn read_at(path: &str) -> Result<Option<String>, String> {
        let mut buffer = [0u16; 2048];
        let mut size = (buffer.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                wide(NAME).as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS || size < 2 || size as usize > buffer.len() * 2 || size % 2 != 0
        {
            return Err("无法读取当前用户的开机启动项。".into());
        }
        let count = size as usize / 2;
        if buffer[count - 1] != 0 || buffer[..count - 1].contains(&0) {
            return Err("开机启动项内容无效。".into());
        }
        String::from_utf16(&buffer[..count - 1])
            .map(Some)
            .map_err(|_| "开机启动项编码无效。".into())
    }

    fn write_at(path: &str, command: Option<&str>) -> Result<(), String> {
        let mut handle = null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut handle,
                null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err("无法修改当前用户的开机启动项。".into());
        }
        let key = Key(handle);
        let status = if let Some(command) = command {
            let value = wide(command);
            unsafe {
                RegSetValueExW(
                    key.0,
                    wide(NAME).as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteValueW(key.0, wide(NAME).as_ptr()) }
        };
        if status != ERROR_SUCCESS && !(command.is_none() && status == ERROR_FILE_NOT_FOUND) {
            return Err("保存开机启动项失败，原设置未确认更改。".into());
        }
        Ok(())
    }

    fn command() -> Result<String, String> {
        let path = std::env::current_exe().map_err(|_| "无法定位程序路径。")?;
        let text = path.to_str().ok_or("程序路径编码不受支持。")?;
        let value = format!("\"{text}\" --autostart");
        if text.contains(['\0', '"']) || value.encode_utf16().count() > 260 || !path.is_file() {
            return Err("程序路径无法登记为启动项，请放到较短的本地目录。".into());
        }
        Ok(value)
    }

    fn run_path() -> Result<String, String> {
        match std::env::var("PASSWORD_MANAGER_TEST_RUN_KEY") {
            Err(std::env::VarError::NotPresent) => Ok(RUN.into()),
            Ok(path)
                if std::env::var_os("PASSWORD_MANAGER_DATA_DIR").is_some()
                    && path
                        .strip_prefix("Software\\PasswordHelperTests\\")
                        .is_some_and(|name| {
                            !name.is_empty()
                                && name.len() <= 80
                                && name
                                    .chars()
                                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
                        }) =>
            {
                Ok(path)
            }
            _ => Err("测试启动项路径无效。".into()),
        }
    }
    pub fn enabled() -> Result<bool, String> {
        Ok(read_at(&run_path()?)?.is_some())
    }

    fn relocate_at(path: &str) -> Result<(), String> {
        if let Some(previous) = read_at(path)? {
            let current = command()?;
            if previous != current {
                write_at(path, Some(&current))?;
            }
        }
        Ok(())
    }

    pub fn relocate_registration() -> Result<(), String> {
        // An isolated vault must never retarget the real user's startup entry.
        if std::env::var_os("PASSWORD_MANAGER_DATA_DIR").is_some()
            && std::env::var_os("PASSWORD_MANAGER_TEST_RUN_KEY").is_none()
        {
            return Ok(());
        }
        relocate_at(&run_path()?)
    }

    pub fn remove_registration() -> Result<(), String> {
        write_at(&run_path()?, None)
    }

    fn save_at(root: &Path, path: &str, enabled: bool, tray: bool) -> Result<(), String> {
        let previous = read_at(path)?;
        let desired = if enabled { Some(command()?) } else { None };
        let temporary = root.join("launch-settings.tmp");
        let mut output = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|_| "无法保存启动设置。")?;
        output
            .write_all(if tray { b"tray=1\n" } else { b"tray=0\n" })
            .and_then(|_| output.sync_all())
            .map_err(|_| "无法写入启动设置。")?;
        drop(output);
        if previous != desired {
            if let Err(error) = write_at(path, desired.as_deref()) {
                let _ = fs::remove_file(temporary);
                return Err(error);
            }
        }
        if fs::rename(&temporary, root.join("launch-settings.txt")).is_err() {
            let rollback = if previous != desired {
                write_at(path, previous.as_deref())
            } else {
                Ok(())
            };
            let _ = fs::remove_file(temporary);
            return Err(if rollback.is_ok() {
                "启动设置保存失败，原启动项已恢复。"
            } else {
                "启动设置保存失败，且启动项恢复失败；请重新打开设置核对。"
            }
            .into());
        }
        Ok(())
    }
    pub fn save(root: &Path, enabled: bool, tray: bool) -> Result<(), String> {
        save_at(root, &run_path()?, enabled, tray)
    }

    #[test]
    fn registry_and_settings_roundtrip_and_failed_file_commit_rolls_back() {
        let root = std::env::temp_dir().join(format!("launch-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let key = format!("Software\\PasswordHelperTests\\{}", std::process::id());
        assert!(read_at(&key).unwrap().is_none());
        save_at(&root, &key, true, true).unwrap();
        let original = read_at(&key).unwrap().unwrap();
        assert!(original.starts_with('"') && original.ends_with("\" --autostart"));
        write_at(&key, Some("\"C:\\old-location\\Keyisle.exe\" --autostart")).unwrap();
        relocate_at(&key).unwrap();
        assert_eq!(read_at(&key).unwrap(), Some(original.clone()));
        assert!(load_tray(&root).unwrap());
        fs::remove_file(root.join("launch-settings.txt")).unwrap();
        fs::create_dir(root.join("launch-settings.txt")).unwrap();
        assert!(save_at(&root, &key, false, false).is_err());
        assert_eq!(read_at(&key).unwrap(), Some(original));
        fs::remove_dir(root.join("launch-settings.txt")).unwrap();
        save_at(&root, &key, false, false).unwrap();
        assert!(read_at(&key).unwrap().is_none());
        assert!(!load_tray(&root).unwrap());
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(&key).as_ptr());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
#[cfg(windows)]
pub use registry::{enabled, relocate_registration, remove_registration, save};

#[test]
fn tray_settings_default_and_invalid_content() {
    let root = std::env::temp_dir().join(format!("tray-test-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    assert!(!load_tray(&root).unwrap());
    fs::write(root.join("launch-settings.txt"), "tray=1\n").unwrap();
    assert!(load_tray(&root).unwrap());
    fs::write(root.join("launch-settings.txt"), "unexpected").unwrap();
    assert!(load_tray(&root).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(not(windows))]
pub fn remove_registration() -> Result<(), String> {
    Ok(())
}
