use crate::vault::Vault;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Session {
    pub vault: Option<Vault>,
    pub path: PathBuf,
    pub root: PathBuf,
    pub generation: u64,
    pub working: bool,
    pub auth_owner: Option<usize>,
    pub notice: Option<String>,
}

fn unique_name(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}.db",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id()
    )
}

impl Session {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|_| "无法创建用户数据目录。")?;
        let preference = root.join("last-vault.txt");
        let path = if preference.exists() {
            let value = fs::read_to_string(preference).map_err(|_| "无法读取密码库路径设置。")?;
            if value.is_empty() || value.len() > 32768 {
                return Err("密码库路径设置损坏。".into());
            }
            PathBuf::from(value)
        } else {
            root.join("vault.db")
        };
        Ok(Self {
            vault: None,
            path,
            root,
            generation: 0,
            working: false,
            auth_owner: None,
            notice: None,
        })
    }

    pub fn accept(&mut self, vault: Vault) -> Result<(), String> {
        let path = self.path.to_str().ok_or("密码库路径不能保存为文本。")?;
        let temporary = self.root.join("last-vault.tmp");
        fs::write(&temporary, path)
            .and_then(|_| fs::rename(&temporary, self.root.join("last-vault.txt")))
            .map_err(|_| "无法保存密码库路径设置，文件仍保留在显示的路径。")?;
        self.vault = Some(vault);
        Ok(())
    }

    pub fn lock(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.vault = None;
        self.auth_owner = None;
    }

    pub fn backup_before_change(&mut self) -> Result<(), String> {
        let folder = self.root.join("backups");
        fs::create_dir_all(&folder).map_err(|_| "无法创建自动备份目录，已取消修改。")?;
        self.vault
            .as_mut()
            .ok_or("密码库已锁定。")?
            .backup(&folder.join(unique_name("auto")))?;
        let mut backups: Vec<_> = fs::read_dir(&folder)
            .map_err(|_| "无法读取备份目录。")?
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("auto-")
                    && e.path().extension().is_some_and(|e| e == "db")
            })
            .map(|e| e.path())
            .collect();
        backups.sort();
        let remove = backups.len().saturating_sub(10);
        for path in backups.into_iter().take(remove) {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }

    pub fn restore_copy(&mut self, source: &Path) -> Result<(), String> {
        if self.working {
            return Err("请等待当前解锁操作完成。".into());
        }
        let destination = copy_backup(&self.root, source)?;
        self.lock();
        self.path = destination;
        Ok(())
    }
}

pub(crate) fn copy_backup(root: &Path, source: &Path) -> Result<PathBuf, String> {
    let mut input = fs::File::open(source).map_err(|_| "无法读取备份文件。")?;
    if input.metadata().map_err(|_| "无法读取备份大小。")?.len() > 128 * 1024 * 1024 {
        return Err("备份超过当前支持的 128 MiB。".into());
    }
    let destination = root.join(unique_name("restored"));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|_| "无法创建恢复副本。")?;
    let result = std::io::copy(&mut input.by_ref().take(128 * 1024 * 1024 + 1), &mut output)
        .and_then(|bytes| {
            if bytes > 128 * 1024 * 1024 {
                Err(std::io::Error::other("file grew"))
            } else {
                output.sync_all()
            }
        });
    if result.is_err() {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err("复制备份失败，原密码库未被修改。".into());
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Credential;
    #[test]
    fn profile_restores_a_copy_and_remembers_only_accepted_vaults() {
        let root = std::env::temp_dir().join(unique_name("profile-test"));
        let mut session = Session::new(root.clone()).unwrap();
        let original = session.path.clone();
        let master = "Session-Test-Only!2026";
        session
            .accept(Vault::create(&original, master).unwrap())
            .unwrap();
        session
            .vault
            .as_mut()
            .unwrap()
            .save(&Credential {
                title: "before".into(),
                ..Default::default()
            })
            .unwrap();
        session.backup_before_change().unwrap();
        let backup = fs::read_dir(root.join("backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        session
            .vault
            .as_mut()
            .unwrap()
            .save(&Credential {
                title: "after".into(),
                ..Default::default()
            })
            .unwrap();
        session.restore_copy(&backup).unwrap();
        assert!(session.vault.is_none());
        assert_eq!(Session::new(root.clone()).unwrap().path, original);
        let restored = session.path.clone();
        let vault = Vault::open(&restored, master).unwrap();
        assert_eq!(vault.list().unwrap().len(), 1);
        session.accept(vault).unwrap();
        assert_eq!(Session::new(root.clone()).unwrap().path, restored);
        assert_eq!(
            Vault::open(&original, master)
                .unwrap()
                .list()
                .unwrap()
                .len(),
            2
        );
        session.lock();
        assert!(session.vault.is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
