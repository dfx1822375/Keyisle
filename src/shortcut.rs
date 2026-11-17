use std::{fmt, fs, path::Path, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: u32,
    pub key: u32,
}
impl Default for Shortcut {
    fn default() -> Self {
        Self {
            modifiers: 3,
            key: b'P' as u32,
        }
    }
}
impl FromStr for Shortcut {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        if text.trim().chars().any(char::is_control) {
            return Err("快捷键不能包含控制字符。".into());
        }
        let mut modifiers = 0;
        let mut key = None;
        for part in text.trim().split('+').map(str::trim) {
            let part = part.to_ascii_uppercase();
            let flag = match part.as_str() {
                "CTRL" => 2,
                "ALT" => 1,
                "SHIFT" => 4,
                _ => 0,
            };
            if flag != 0 {
                if modifiers & flag != 0 {
                    return Err("修饰键重复。".into());
                }
                modifiers |= flag;
            } else {
                let value = if part.len() == 1 && part.as_bytes()[0].is_ascii_alphanumeric() {
                    part.as_bytes()[0] as u32
                } else if let Some(number) = part
                    .strip_prefix('F')
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|n| (1..=24).contains(n))
                {
                    0x6f + number
                } else {
                    return Err(
                        "请使用 Ctrl、Alt、Shift 加字母、数字或 F1～F24，例如 Ctrl+Alt+P。".into(),
                    );
                };
                if key.replace(value).is_some() {
                    return Err("快捷键只能包含一个普通按键。".into());
                }
            }
        }
        let value = Self {
            modifiers,
            key: key.ok_or("请指定字母、数字或功能键。")?,
        };
        if !value.valid() {
            return Err("快捷键必须包含 Ctrl 或 Alt。".into());
        }
        Ok(value)
    }
}
impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [(2, "Ctrl+"), (1, "Alt+"), (4, "Shift+")] {
            if self.modifiers & flag != 0 {
                f.write_str(name)?;
            }
        }
        if self.key >= 0x70 {
            write!(f, "F{}", self.key - 0x6f)
        } else {
            write!(f, "{}", char::from_u32(self.key).ok_or(fmt::Error)?)
        }
    }
}
impl Shortcut {
    pub fn valid(self) -> bool {
        self.modifiers & !7 == 0
            && self.modifiers & 3 != 0
            && ((b'A' as u32..=b'Z' as u32).contains(&self.key)
                || (b'0' as u32..=b'9' as u32).contains(&self.key)
                || (0x70..=0x87).contains(&self.key))
    }
    pub fn load(root: &Path) -> Result<Self, String> {
        match fs::read_to_string(root.join("shortcut.txt")) {
            Ok(text) => text.parse(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("无法读取快捷键设置。".into()),
        }
    }
    pub fn save(self, root: &Path) -> Result<(), String> {
        if !self.valid() {
            return Err("快捷键无效。".into());
        }
        let temporary = root.join("shortcut.tmp");
        fs::write(&temporary, format!("{self}\n"))
            .and_then(|_| fs::rename(temporary, root.join("shortcut.txt")))
            .map_err(|_| "无法保存快捷键设置。".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcut_roundtrips_and_rejects_incomplete_or_unsafe_combinations() {
        assert_eq!(
            " ctrl + alt + p ".parse::<Shortcut>().unwrap(),
            Shortcut::default()
        );
        for input in ["Ctrl+Shift+F2", "Alt+7", "Ctrl+Alt+Shift+F24"] {
            let parsed: Shortcut = input.parse().unwrap();
            assert!(parsed.valid());
            assert_eq!(parsed.to_string(), input);
        }
        for input in [
            "P",
            "Shift+P",
            "Ctrl",
            "Ctrl+Ctrl+P",
            "Ctrl+P+Q",
            "Ctrl+",
            "Ctrl+F0",
            "Alt+F25",
            "Ctrl+\nP",
            "Win+L",
        ] {
            assert!(input.parse::<Shortcut>().is_err(), "{input}");
        }
        assert!(
            !Shortcut {
                modifiers: 8,
                key: 80
            }
            .valid()
        );
        let root = std::env::temp_dir().join(format!("pm-shortcut-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert_eq!(Shortcut::load(&root).unwrap(), Shortcut::default());
        let shortcut: Shortcut = "Ctrl+Shift+F2".parse().unwrap();
        shortcut.save(&root).unwrap();
        assert_eq!(Shortcut::load(&root).unwrap(), shortcut);
        fs::remove_dir_all(root).unwrap();
    }
}
