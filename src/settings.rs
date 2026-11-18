use std::{
    fs,
    path::{Path, PathBuf},
};

const FILES: [&str; 7] = [
    "shortcut.txt",
    "lock-settings.txt",
    "launch-settings.txt",
    "theme.txt",
    "appearance.txt",
    "screen-protection.txt",
    "auto-update.txt",
];

pub fn load_theme(root: &Path) -> Result<i32, String> {
    match fs::read_to_string(root.join("theme.txt")) {
        Ok(text) => match text.trim() {
            "teal" => Ok(0),
            "blue" => Ok(1),
            "violet" => Ok(2),
            "graphite" => Ok(3),
            _ => Err("配色设置无效，请保留文件并检查。".into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(_) => Err("无法读取配色设置。".into()),
    }
}

pub fn save_theme(root: &Path, index: i32) -> Result<(), String> {
    let name = match index {
        0 => "teal",
        1 => "blue",
        2 => "violet",
        3 => "graphite",
        _ => return Err("配色选项无效。".into()),
    };
    let temp = root.join("theme.tmp");
    fs::write(&temp, format!("{name}\n")).map_err(|_| "无法写入配色设置。")?;
    fs::rename(temp, root.join("theme.txt")).map_err(|_| "无法保存配色设置。".into())
}

pub fn load_screen_protection(root: &Path) -> Result<bool, String> {
    match fs::read_to_string(root.join("screen-protection.txt")) {
        Ok(text) => match text.trim() {
            "0" | "off" => Ok(false),
            "1" | "on" => Ok(true),
            _ => Err("防截屏保护设置无效，请保留文件并检查。".into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("无法读取防截屏保护设置。".into()),
    }
}

pub fn save_screen_protection(root: &Path, enabled: bool) -> Result<(), String> {
    let value = if enabled { "1\n" } else { "0\n" };
    let temp = root.join("screen-protection.tmp");
    fs::write(&temp, value).map_err(|_| "无法写入防截屏保护设置。")?;
    fs::rename(temp, root.join("screen-protection.txt"))
        .map_err(|_| "无法保存防截屏保护设置。".into())
}

pub fn load_auto_update(root: &Path) -> Result<bool, String> {
    match fs::read_to_string(root.join("auto-update.txt")) {
        Ok(text) => match text.trim() {
            "0" | "off" => Ok(false),
            "1" | "on" => Ok(true),
            _ => Err("自动更新检查设置无效，请保留文件并检查。".into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(_) => Err("无法读取自动更新检查设置。".into()),
    }
}

pub fn save_auto_update(root: &Path, enabled: bool) -> Result<(), String> {
    let value = if enabled { "1\n" } else { "0\n" };
    let temp = root.join("auto-update.tmp");
    fs::write(&temp, value).map_err(|_| "无法写入自动更新检查设置。")?;
    fs::rename(temp, root.join("auto-update.txt")).map_err(|_| "无法保存自动更新检查设置。".into())
}

pub struct Snapshot {
    root: PathBuf,
    files: Vec<Option<Vec<u8>>>,
}
impl Snapshot {
    pub fn capture(root: &Path) -> Result<Self, String> {
        let mut files = Vec::new();
        for name in FILES {
            let path = root.join(name);
            match fs::metadata(&path) {
                Ok(meta) if !meta.is_file() || meta.len() > 32768 => {
                    return Err("设置文件无效，未进行修改。".into());
                }
                Ok(_) => files.push(Some(fs::read(path).map_err(|_| "无法读取原设置。")?)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => files.push(None),
                Err(_) => return Err("无法读取原设置。".into()),
            }
        }
        Ok(Self {
            root: root.to_owned(),
            files,
        })
    }
    pub fn restore(&self) -> Result<(), String> {
        let mut failed = false;
        for (index, (name, contents)) in FILES.iter().zip(&self.files).enumerate() {
            let target = self.root.join(name);
            let result = if let Some(bytes) = contents {
                let temp = self.root.join(format!("settings-rollback-{index}.tmp"));
                fs::write(&temp, bytes).and_then(|_| fs::rename(temp, target))
            } else {
                match fs::remove_file(target) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    other => other,
                }
            };
            failed |= result.is_err();
        }
        if failed {
            Err("部分设置未能恢复，请重新打开设置核对。".into())
        } else {
            Ok(())
        }
    }
}
#[test]
fn rollback_preserves_exact_files_and_absent_defaults() {
    let root = std::env::temp_dir().join(format!("settings-snapshot-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(FILES[0]), b"Ctrl+Alt+P\n").unwrap();
    let snapshot = Snapshot::capture(&root).unwrap();
    for name in FILES {
        fs::write(root.join(name), b"new").unwrap();
    }
    snapshot.restore().unwrap();
    assert_eq!(fs::read(root.join(FILES[0])).unwrap(), b"Ctrl+Alt+P\n");
    assert!(FILES[1..].iter().all(|name| !root.join(name).exists()));
    fs::create_dir(root.join(FILES[1])).unwrap();
    assert!(Snapshot::capture(&root).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn themes_and_screen_protection_round_trip_and_roll_back() {
    let root = std::env::temp_dir().join(format!("theme-settings-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    assert_eq!(load_theme(&root).unwrap(), 0);
    assert!(!load_screen_protection(&root).unwrap());
    assert!(load_auto_update(&root).unwrap());
    for index in 0..4 {
        save_theme(&root, index).unwrap();
        assert_eq!(load_theme(&root).unwrap(), index);
    }
    save_screen_protection(&root, true).unwrap();
    save_auto_update(&root, false).unwrap();
    assert!(load_screen_protection(&root).unwrap());
    assert!(!load_auto_update(&root).unwrap());
    let snapshot = Snapshot::capture(&root).unwrap();
    save_theme(&root, 1).unwrap();
    save_screen_protection(&root, false).unwrap();
    save_auto_update(&root, true).unwrap();
    snapshot.restore().unwrap();
    assert_eq!(load_theme(&root).unwrap(), 3);
    assert!(load_screen_protection(&root).unwrap());
    assert!(!load_auto_update(&root).unwrap());
    assert!(save_theme(&root, 4).is_err());
    assert_eq!(load_theme(&root).unwrap(), 3);
    fs::write(root.join("theme.txt"), "unknown").unwrap();
    assert!(load_theme(&root).is_err());
    fs::write(root.join("auto-update.txt"), "unknown").unwrap();
    assert!(load_auto_update(&root).is_err());
    fs::remove_dir_all(root).unwrap();
}

pub fn load_appearance(root: &Path) -> Result<i32, String> {
    match fs::read_to_string(root.join("appearance.txt")) {
        Ok(text) => match text.trim() {
            "system" => Ok(0),
            "light" => Ok(1),
            "dark" => Ok(2),
            _ => Err("外观模式设置无效。".into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(_) => Err("无法读取外观模式设置。".into()),
    }
}

pub fn save_appearance(root: &Path, index: i32) -> Result<(), String> {
    let value = match index {
        0 => "system",
        1 => "light",
        2 => "dark",
        _ => return Err("外观模式无效。".into()),
    };
    fs::write(root.join("appearance.tmp"), format!("{value}\n"))
        .and_then(|_| fs::rename(root.join("appearance.tmp"), root.join("appearance.txt")))
        .map_err(|_| "无法保存外观模式。".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowLayout {
    // Virtual-screen coordinates captured via GetWindowRect and restored via SetWindowPos.
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}
impl WindowLayout {
    fn valid(&self) -> bool {
        [self.left, self.top, self.right, self.bottom]
            .iter()
            .all(|v| v.unsigned_abs() <= 1_000_000)
            && self.right > self.left
            && self.bottom > self.top
            && (640..=16384).contains(&self.width)
            && (560..=16384).contains(&self.height)
    }
    pub fn load(root: &Path) -> Result<Option<Self>, String> {
        let text = match fs::read_to_string(root.join("window.txt")) {
            Ok(text) if text.len() <= 256 => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => return Err("无法读取窗口布局，已使用默认位置和大小。".into()),
        };
        let values = text
            .lines()
            .map(str::parse::<i32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "窗口布局无效，已使用默认位置和大小。")?;
        if values.len() != 7 || ![0, 1].contains(&values[6]) {
            return Err("窗口布局无效。".into());
        }
        let layout = Self {
            left: values[0],
            top: values[1],
            right: values[2],
            bottom: values[3],
            width: values[4] as u32,
            height: values[5] as u32,
            maximized: values[6] == 1,
        };
        if !layout.valid() {
            return Err("窗口布局超出支持范围，已使用默认布局。".into());
        }
        Ok(Some(layout))
    }
    pub fn save(&self, root: &Path) -> Result<(), String> {
        if !self.valid() {
            return Err("窗口布局无效，未保存。".into());
        }
        let data = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.left,
            self.top,
            self.right,
            self.bottom,
            self.width,
            self.height,
            self.maximized as u8
        );
        fs::write(root.join("window.tmp"), data)
            .and_then(|_| fs::rename(root.join("window.tmp"), root.join("window.txt")))
            .map_err(|_| "无法保存窗口位置和大小。".into())
    }
}

#[test]
fn appearance_and_window_layout_validate_defaults_and_round_trip() {
    let root = std::env::temp_dir().join(format!("appearance-layout-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    assert_eq!(load_appearance(&root).unwrap(), 0);
    assert_eq!(WindowLayout::load(&root).unwrap(), None);
    for index in 0..3 {
        save_appearance(&root, index).unwrap();
        assert_eq!(load_appearance(&root).unwrap(), index);
    }
    let layout = WindowLayout {
        left: -1200,
        top: 40,
        right: -100,
        bottom: 900,
        width: 733,
        height: 573,
        maximized: true,
    };
    layout.save(&root).unwrap();
    assert_eq!(WindowLayout::load(&root).unwrap(), Some(layout));
    assert!(save_appearance(&root, 3).is_err());
    fs::write(root.join("window.txt"), "0\n0\n20\n30\n-1\n200\n0\n").unwrap();
    assert!(WindowLayout::load(&root).is_err());
    fs::write(root.join("appearance.txt"), "unknown").unwrap();
    assert!(load_appearance(&root).is_err());
    fs::remove_dir_all(root).unwrap();
}
