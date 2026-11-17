//! Managed local backups and a restartable, file-by-file master-password switch.
use crate::vault::Vault;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

type Result<T> = std::result::Result<T, String>;
const STAGING: &str = "master-change";

fn regular(path: &Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|_| "无法读取本地文件信息。")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("本地备份目录含有链接，未进行操作。".into());
        }
    }
    if meta.file_type().is_symlink()
        || meta.is_dir() != directory
        || (!directory && !meta.is_file())
    {
        return Err("本地备份含有链接或非普通文件，未进行操作。".into());
    }
    Ok(())
}

fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

fn numbered(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(".db"))
        .and_then(|s| s.split_once('-'))
        .is_some_and(|(time, pid)| digits(time) && digits(pid))
}

fn vault_name(value: &str) -> bool {
    value == "vault.db" || numbered(value, "restored-")
}

fn upgrade_name(value: &str) -> bool {
    value
        .strip_suffix(".db")
        .and_then(|s| s.rsplit_once(".pre-v04-"))
        .is_some_and(|(base, time)| vault_name(&format!("{base}.db")) && digits(time))
}

fn allowed(relative: &Path) -> bool {
    let parts: Vec<_> = relative.components().collect();
    match parts.as_slice() {
        [Component::Normal(name)] => name
            .to_str()
            .is_some_and(|s| vault_name(s) || upgrade_name(s)),
        [Component::Normal(folder), Component::Normal(name)] if *folder == "backups" => {
            name.to_str().is_some_and(|s| numbered(s, "auto-"))
        }
        _ => false,
    }
}

pub fn local_backups(root: &Path, active: &Path) -> Result<Vec<PathBuf>> {
    regular(root, true)?;
    let mut paths = Vec::new();
    let folder = root.join("backups");
    if folder.try_exists().map_err(|_| "无法检查本地备份目录。")? {
        regular(&folder, true)?;
        for entry in fs::read_dir(&folder).map_err(|_| "无法读取本地备份目录。")? {
            let entry = entry.map_err(|_| "无法读取本地备份。")?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|s| numbered(s, "auto-"))
            {
                regular(&entry.path(), false)?;
                paths.push(entry.path());
            }
        }
    }
    for entry in fs::read_dir(root).map_err(|_| "无法读取数据目录。")? {
        let entry = entry.map_err(|_| "无法读取数据文件。")?;
        if entry.file_name().to_str().is_some_and(upgrade_name) {
            regular(&entry.path(), false)?;
            paths.push(entry.path());
        }
    }
    paths.retain(|p| p != active);
    paths.sort();
    Ok(paths)
}

pub struct BackupInfo {
    pub path: PathBuf,
    pub nanos: u128,
    pub bytes: u64,
    pub kind: &'static str,
}

pub fn backup_list(root: &Path, active: &Path) -> Result<Vec<BackupInfo>> {
    let mut result = Vec::new();
    for path in local_backups(root, active)? {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("备份名称无效。")?;
        let (time, kind) = if let Some(rest) = name.strip_prefix("auto-") {
            (rest.split('-').next().unwrap(), "自动备份")
        } else {
            (
                name.rsplit_once(".pre-v04-")
                    .unwrap()
                    .1
                    .trim_end_matches(".db"),
                "升级前备份",
            )
        };
        let nanos = time.parse().map_err(|_| "备份名称中的时间无效。")?;
        let bytes = fs::metadata(&path).map_err(|_| "无法读取备份大小。")?.len();
        result.push(BackupInfo {
            path,
            nanos,
            bytes,
            kind,
        });
    }
    result.sort_by(|a, b| b.nanos.cmp(&a.nanos).then_with(|| a.path.cmp(&b.path)));
    Ok(result)
}

fn managed_backup(root: &Path, active: &Path, source: &Path) -> Result<()> {
    if !local_backups(root, active)?.iter().any(|p| p == source) {
        return Err("备份已变化或不在本地备份清单中，请刷新列表。".into());
    }
    regular(source, false)
}

pub fn delete_backup(
    root: &Path,
    active: &Path,
    source: &Path,
    master: &str,
    cancel: &AtomicBool,
) -> Result<()> {
    canceled(cancel)?;
    Vault::verify_master(active, master)?;
    managed_backup(root, active, source)?;
    canceled(cancel)?;
    fs::remove_file(source).map_err(|_| "无法删除选中的备份，请检查文件占用和权限。".into())
}

pub fn restore_backup(
    root: &Path,
    active: &Path,
    source: &Path,
    master: &str,
    cancel: &AtomicBool,
) -> Result<(Vault, PathBuf)> {
    canceled(cancel)?;
    managed_backup(root, active, source)?;
    Vault::verify_master(source, master)?;
    canceled(cancel)?;
    let destination = crate::session::copy_backup(root, source)?;
    let result = (|| {
        let vault = Vault::open(&destination, master)?;
        vault.check()?;
        canceled(cancel)?;
        Ok((vault, destination.clone()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&destination);
    }
    result
}

fn canceled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err("操作已取消，原主密码和原文件保持不变。".into())
    } else {
        Ok(())
    }
}

fn durable_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "无法创建主密码修改记录。")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "无法保存主密码修改记录。".to_string())
}

fn rename(source: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let from: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err("无法切换加密文件，请关闭占用文件的程序后重试。".into());
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, target).map_err(|_| "无法切换加密文件。")?;
        for directory in [source.parent().unwrap(), target.parent().unwrap()] {
            fs::File::open(directory)
                .and_then(|f| f.sync_all())
                .map_err(|_| "无法同步目录。")?;
        }
    }
    Ok(())
}

fn discard_stage(stage: &Path) -> Result<()> {
    regular(stage, true)?;
    // Never recurse into links or user-supplied subdirectories during recovery.
    for entry in fs::read_dir(stage).map_err(|_| "无法读取改密临时目录。")? {
        let entry = entry.map_err(|_| "无法读取改密临时文件。")?;
        regular(&entry.path(), false)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("改密临时文件名无效。")?;
        if !["manifest", "ready", "ready.tmp", "done"].contains(&name)
            && !name
                .strip_suffix(".new")
                .or_else(|| name.strip_suffix(".old"))
                .or_else(|| name.strip_suffix(".new-journal"))
                .is_some_and(digits)
        {
            return Err("改密目录含有未知文件，请保留目录并检查。".into());
        }
    }
    fs::remove_dir_all(stage).map_err(|_| "无法清理改密临时文件，请重启程序重试。".into())
}

/// Before `ready`, originals are untouched. After `ready`, restart always rolls forward.
pub fn recover(root: &Path) -> Result<bool> {
    let stage = root.join(STAGING);
    if !stage.try_exists().map_err(|_| "无法检查主密码修改状态。")? {
        return Ok(false);
    }
    regular(&stage, true)?;
    if stage
        .join("done")
        .try_exists()
        .map_err(|_| "无法检查改密完成记录。")?
    {
        regular(&stage.join("done"), false)?;
        return discard_stage(&stage).map(|_| true);
    }
    if !stage
        .join("ready")
        .try_exists()
        .map_err(|_| "无法检查改密记录。")?
    {
        return discard_stage(&stage).map(|_| false);
    }
    regular(&stage.join("ready"), false)?;
    regular(&stage.join("manifest"), false)?;
    if fs::metadata(stage.join("manifest"))
        .map_err(|_| "无法读取改密记录大小。")?
        .len()
        > 1024 * 1024
    {
        return Err("主密码修改记录过大。".into());
    }
    let manifest =
        fs::read_to_string(stage.join("manifest")).map_err(|_| "无法读取主密码修改记录。")?;
    if manifest.len() > 1024 * 1024 {
        return Err("主密码修改记录过大。".into());
    }
    let files: Vec<_> = manifest.lines().map(Path::new).collect();
    if files.is_empty()
        || files.iter().any(|p| !allowed(p))
        || files
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != files.len()
    {
        return Err("主密码修改记录无效，已保留全部文件。".into());
    }
    for (i, relative) in files.iter().enumerate() {
        let target = root.join(relative);
        regular(target.parent().unwrap(), true)?;
        let next = stage.join(format!("{i}.new"));
        let previous = stage.join(format!("{i}.old"));
        if next.try_exists().map_err(|_| "无法读取改密副本。")? {
            regular(&next, false)?;
            if !previous.try_exists().map_err(|_| "无法检查原副本。")? {
                regular(&target, false)?;
                rename(&target, &previous)?;
            } else if target.try_exists().map_err(|_| "无法检查密码库。")? {
                return Err("改密文件状态冲突，已保留全部文件。".into());
            }
            rename(&next, &target)?;
        } else {
            regular(&target, false)?;
        }
    }
    rename(&stage.join("ready"), &stage.join("done"))?;
    discard_stage(&stage).map(|_| true)
}

pub fn validate_change(old: &str, new: &str) -> Result<()> {
    if new.chars().count() < 10 || new.len() > 4096 || new.contains('\0') {
        return Err("新主密码至少需要 10 个字符、最多 4096 字节，且不能包含空字符。".into());
    }
    if old == new {
        return Err("新主密码不能与当前主密码相同。".into());
    }
    Ok(())
}

pub fn change_master(
    root: &Path,
    active: &Path,
    old: &str,
    new: &str,
    cancel: &AtomicBool,
) -> Result<usize> {
    validate_change(old, new)?;
    canceled(cancel)?;
    Vault::verify_master(active, old)?;
    let mut files = local_backups(root, active)?;
    let count = files.len();
    files.push(active.to_owned());
    let relative: Vec<_> = files
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .ok()
                .filter(|p| allowed(p))
                .ok_or("当前密码库不在支持的本地数据目录中。")
        })
        .collect::<std::result::Result<_, _>>()?;
    let stage = root.join(STAGING);
    fs::create_dir(&stage).map_err(|_| "存在未完成的改密记录，请重启程序恢复后再试。")?;
    let prepare = (|| {
        let manifest = relative
            .iter()
            .map(|p| p.to_str().ok_or("文件名无法保存。"))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join("\n");
        durable_write(&stage.join("manifest"), manifest.as_bytes())?;
        for (i, path) in files.iter().enumerate() {
            canceled(cancel)?;
            regular(path, false)?;
            Vault::rekey_copy(path, &stage.join(format!("{i}.new")), old, new).map_err(|_| {
                format!(
                    "本地文件 {} 无法解锁、校验或重新加密；全部原文件未替换。",
                    relative[i].display()
                )
            })?;
        }
        canceled(cancel)?;
        durable_write(&stage.join("ready.tmp"), b"1")?;
        rename(&stage.join("ready.tmp"), &stage.join("ready"))
    })();
    if let Err(message) = prepare {
        if stage.join("ready").exists() {
            return Err("改密已进入切换阶段，请重启程序完成恢复，并使用新主密码。".into());
        }
        let cleanup = discard_stage(&stage);
        return Err(if cleanup.is_ok() {
            message
        } else {
            format!("{message} 临时副本未清理，请重启程序。")
        });
    }
    // Cancellation cannot interrupt a committed multi-file switch; the UI stays locked.
    recover(root).map_err(
        |_| "主密码切换或旧副本清理未完成，请关闭文件占用程序并重启；恢复后使用新主密码。",
    )?;
    Ok(count)
}

pub fn clear_backups(
    root: &Path,
    active: &Path,
    master: &str,
    expected: usize,
    cancel: &AtomicBool,
) -> Result<usize> {
    canceled(cancel)?;
    Vault::verify_master(active, master)?;
    let files = local_backups(root, active)?;
    if files.len() != expected {
        return Err("备份数量已变化，请重新确认清除范围。".into());
    }
    let mut removed = 0;
    for path in files {
        if cancel.load(Ordering::SeqCst) {
            return Err(format!(
                "已停止清理，已删除 {removed} 个本地备份；当前密码库未删除。"
            ));
        }
        regular(&path, false)?;
        if fs::remove_file(path).is_err() {
            return Err(format!(
                "已删除 {removed} 个备份，其余未删除；请检查文件占用和权限。"
            ));
        }
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Credential;
    struct Profile(PathBuf);
    impl Profile {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "maintenance-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(root.join("backups")).unwrap();
            Self(root)
        }
        fn seed(&self) -> (PathBuf, Vec<PathBuf>) {
            let active = self.0.join("vault.db");
            let mut vault = Vault::create(&active, OLD).unwrap();
            vault
                .save(&Credential {
                    title: "sensitive record".into(),
                    sensitive: true,
                    password: zeroize::Zeroizing::new("Synthetic-Secret!".into()),
                    ..Default::default()
                })
                .unwrap();
            let backups = vec![
                self.0.join("backups/auto-1-1.db"),
                self.0.join("vault.pre-v04-1.db"),
            ];
            for path in &backups {
                vault.backup(path).unwrap();
            }
            (active, backups)
        }
    }
    impl Drop for Profile {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    const OLD: &str = "Old-Synthetic-Master!2026";
    const NEW: &str = "New-Synthetic-Master!2026";

    #[test]
    fn rotates_managed_files_preserves_data_and_excludes_exports_and_independent_vaults() {
        let profile = Profile::new();
        let (active, backups) = profile.seed();
        let export = profile.0.join("manual-export.db");
        let independent = profile.0.join("restored-1-1.db");
        fs::copy(&active, &export).unwrap();
        fs::copy(&active, &independent).unwrap();
        assert_eq!(
            change_master(&profile.0, &active, OLD, NEW, &AtomicBool::new(false)).unwrap(),
            2
        );
        for path in backups.iter().chain([&active]) {
            assert!(Vault::verify_master(path, OLD).is_err());
            let vault = Vault::open(path, NEW).unwrap();
            let record = vault.get(vault.list().unwrap()[0].id).unwrap();
            assert!(record.sensitive);
            assert_eq!(record.password.as_str(), "Synthetic-Secret!");
        }
        for path in [export, independent] {
            Vault::verify_master(&path, OLD).unwrap();
        }
        assert!(!profile.0.join(STAGING).exists());
    }

    #[test]
    fn prepare_failure_wrong_password_and_cancellation_leave_originals_unchanged() {
        let profile = Profile::new();
        let (active, backups) = profile.seed();
        let bytes = fs::read(&active).unwrap();
        for (old, new, canceled) in [
            ("wrong", NEW, false),
            (OLD, "short", false),
            (OLD, OLD, false),
            (OLD, NEW, true),
        ] {
            assert!(
                change_master(&profile.0, &active, old, new, &AtomicBool::new(canceled)).is_err()
            );
            assert_eq!(fs::read(&active).unwrap(), bytes);
        }
        fs::write(&backups[1], b"corrupt synthetic backup").unwrap();
        assert!(change_master(&profile.0, &active, OLD, NEW, &AtomicBool::new(false)).is_err());
        assert_eq!(fs::read(&active).unwrap(), bytes);
        Vault::verify_master(&backups[0], OLD).unwrap();
        assert!(!profile.0.join(STAGING).exists());
    }

    #[test]
    fn cleanup_requires_correct_master_and_confirmed_count_and_never_removes_the_vault() {
        let profile = Profile::new();
        let (active, _) = profile.seed();
        let export = profile.0.join("manual.db");
        fs::copy(&active, &export).unwrap();
        for (master, expected, canceled) in [("wrong", 2, false), (OLD, 1, false), (OLD, 2, true)] {
            assert!(
                clear_backups(
                    &profile.0,
                    &active,
                    master,
                    expected,
                    &AtomicBool::new(canceled)
                )
                .is_err()
            );
            assert_eq!(local_backups(&profile.0, &active).unwrap().len(), 2);
        }
        assert_eq!(
            clear_backups(&profile.0, &active, OLD, 2, &AtomicBool::new(false)).unwrap(),
            2
        );
        assert!(local_backups(&profile.0, &active).unwrap().is_empty());
        Vault::verify_master(&active, OLD).unwrap();
        assert!(export.exists());
        assert_eq!(
            clear_backups(&profile.0, &active, OLD, 0, &AtomicBool::new(false)).unwrap(),
            0
        );
    }

    #[test]
    fn backup_list_single_delete_and_verified_restore_preserve_originals() {
        let profile = Profile::new();
        let (active, backups) = profile.seed();
        let list = backup_list(&profile.0, &active).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|b| b.nanos == 1 && b.bytes > 0));
        assert!(list.iter().any(|b| b.kind == "升级前备份"));
        let newer = profile.0.join("backups/auto-200-1.db");
        fs::copy(&active, &newer).unwrap();
        assert_eq!(backup_list(&profile.0, &active).unwrap()[0].nanos, 200);
        fs::remove_file(newer).unwrap();
        let mut vault = Vault::open(&active, OLD).unwrap();
        vault
            .save(&Credential {
                title: "later record".into(),
                ..Default::default()
            })
            .unwrap();
        drop(vault);
        let cancel = AtomicBool::new(false);
        assert!(restore_backup(&profile.0, &active, &backups[0], "wrong", &cancel).is_err());
        assert!(delete_backup(&profile.0, &active, &active, OLD, &cancel).is_err());
        assert!(delete_backup(&profile.0, &active, &backups[0], "wrong", &cancel).is_err());
        let (restored, path) =
            restore_backup(&profile.0, &active, &backups[0], OLD, &cancel).unwrap();
        assert_ne!(path, active);
        assert_eq!(restored.list().unwrap().len(), 1);
        assert_eq!(Vault::open(&active, OLD).unwrap().list().unwrap().len(), 2);
        drop(restored);
        assert!(
            delete_backup(
                &profile.0,
                &active,
                &backups[0],
                OLD,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        delete_backup(&profile.0, &active, &backups[0], OLD, &cancel).unwrap();
        assert!(backups[1].exists() && path.exists() && active.exists());
        assert_eq!(backup_list(&profile.0, &active).unwrap().len(), 1);
    }

    #[test]
    fn recovery_handles_each_rename_boundary_and_interrupted_cleanup() {
        for boundary in 0..6 {
            let profile = Profile::new();
            let (active, _) = profile.seed();
            let paths = [profile.0.join("backups/auto-1-1.db"), active];
            let stage = profile.0.join(STAGING);
            fs::create_dir(&stage).unwrap();
            durable_write(&stage.join("manifest"), b"backups/auto-1-1.db\nvault.db").unwrap();
            for (i, path) in paths.iter().enumerate() {
                Vault::rekey_copy(path, &stage.join(format!("{i}.new")), OLD, NEW).unwrap();
            }
            durable_write(&stage.join("ready"), b"1").unwrap();
            for (i, path) in paths.iter().enumerate() {
                if boundary > i * 2 {
                    rename(path, &stage.join(format!("{i}.old"))).unwrap();
                }
                if boundary > i * 2 + 1 {
                    rename(&stage.join(format!("{i}.new")), path).unwrap();
                }
            }
            if boundary == 5 {
                rename(&stage.join("ready"), &stage.join("done")).unwrap();
                fs::remove_file(stage.join("manifest")).unwrap();
                fs::remove_file(stage.join("0.old")).unwrap();
            }
            recover(&profile.0).unwrap();
            for path in paths {
                Vault::verify_master(&path, NEW).unwrap();
                assert!(Vault::verify_master(&path, OLD).is_err());
            }
            assert!(!stage.exists());
        }
    }

    #[test]
    fn abandoned_preparation_is_removed_and_recovery_rejects_path_traversal() {
        let profile = Profile::new();
        let (active, _) = profile.seed();
        let stage = profile.0.join(STAGING);
        fs::create_dir(&stage).unwrap();
        durable_write(&stage.join("0.new"), b"incomplete encrypted copy").unwrap();
        durable_write(
            &stage.join("0.new-journal"),
            b"incomplete encrypted journal",
        )
        .unwrap();
        recover(&profile.0).unwrap();
        Vault::verify_master(&active, OLD).unwrap();
        fs::create_dir(&stage).unwrap();
        durable_write(&stage.join("manifest"), b"../outside.db").unwrap();
        durable_write(&stage.join("ready"), b"1").unwrap();
        assert!(recover(&profile.0).is_err());
        assert!(active.exists());
        assert!(stage.exists());
    }

    #[cfg(unix)]
    #[test]
    fn backup_symlinks_are_rejected() {
        let profile = Profile::new();
        let (active, _) = profile.seed();
        std::os::unix::fs::symlink(&active, profile.0.join("backups/auto-2-1.db")).unwrap();
        assert!(local_backups(&profile.0, &active).is_err());
        assert!(clear_backups(&profile.0, &active, OLD, 3, &AtomicBool::new(false)).is_err());
        Vault::verify_master(&active, OLD).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_in_use_preserves_switch_for_restart_recovery() {
        use std::os::windows::fs::OpenOptionsExt;
        let profile = Profile::new();
        let (active, backups) = profile.seed();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&backups[0])
            .unwrap();
        assert!(change_master(&profile.0, &active, OLD, NEW, &AtomicBool::new(false)).is_err());
        assert!(profile.0.join(STAGING).join("ready").exists());
        Vault::verify_master(&active, OLD).unwrap();
        drop(held);
        assert!(recover(&profile.0).unwrap());
        for path in backups.iter().chain([&active]) {
            Vault::verify_master(path, NEW).unwrap();
        }
    }
}

const RESET_PENDING: &str = "reset-pending";
const RESET_SETTINGS: &[&str] = &[
    "shortcut.txt",
    "lock-settings.txt",
    "launch-settings.txt",
    "theme.txt",
    "appearance.txt",
    "window.txt",
    "last-vault.txt",
    "shortcut.tmp",
    "lock-settings.tmp",
    "launch-settings.tmp",
    "theme.tmp",
    "appearance.tmp",
    "window.tmp",
    "last-vault.tmp",
    "settings-rollback-0.tmp",
    "settings-rollback-1.tmp",
    "settings-rollback-2.tmp",
    "settings-rollback-3.tmp",
    "settings-rollback-4.tmp",
];

fn reset_owned(relative: &Path) -> bool {
    if relative.components().count() == 1
        && relative
            .to_str()
            .is_some_and(|s| RESET_SETTINGS.contains(&s))
    {
        return true;
    }
    if allowed(relative) {
        return true;
    }
    relative
        .to_str()
        .and_then(|s| {
            ["-journal", "-wal", "-shm"]
                .iter()
                .find_map(|suffix| s.strip_suffix(suffix))
        })
        .is_some_and(|base| allowed(Path::new(base)))
}

pub fn reset_pending(root: &Path) -> bool {
    fs::symlink_metadata(root.join(RESET_PENDING)).is_ok()
}

pub fn reset_inventory(root: &Path, active: &Path) -> Result<Vec<PathBuf>> {
    regular(root, true)?;
    let relative = active
        .strip_prefix(root)
        .map_err(|_| "当前库不在软件数据目录内，不能执行完整还原。")?;
    if relative.components().count() != 1 || !relative.to_str().is_some_and(vault_name) {
        return Err("当前库不属于软件管理的密码库，未进行还原。".into());
    }
    regular(active, false)?;
    if root
        .join(STAGING)
        .try_exists()
        .map_err(|_| "无法检查维护状态。")?
    {
        return Err("请先重新启动，完成上次主密码维护后再还原。".into());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| "无法读取软件数据目录。")? {
        let entry = entry.map_err(|_| "无法读取软件数据文件。")?;
        let relative = PathBuf::from(entry.file_name());
        if reset_owned(&relative) {
            regular(&entry.path(), false)?;
            files.push(relative);
        }
    }
    let backups = root.join("backups");
    if fs::symlink_metadata(&backups).is_ok() {
        regular(&backups, true)?;
        for entry in fs::read_dir(&backups).map_err(|_| "无法读取自动备份目录。")? {
            let entry = entry.map_err(|_| "无法读取自动备份文件。")?;
            let relative = PathBuf::from("backups").join(entry.file_name());
            if reset_owned(&relative) {
                regular(&entry.path(), false)?;
                files.push(relative);
            }
        }
    }
    files.sort();
    if files.is_empty() || files.len() > 4096 {
        return Err("还原文件清单无效。".into());
    }
    Ok(files)
}

pub fn begin_reset(root: &Path, active: &Path, master: &str, cancel: &AtomicBool) -> Result<()> {
    canceled(cancel)?;
    if reset_pending(root) {
        return Err("上次还原尚未完成，请继续完成还原。".into());
    }
    let files = reset_inventory(root, active)?;
    Vault::verify_master(active, master)?;
    canceled(cancel)?;
    let mut manifest = String::from("password-helper-reset-v1\n");
    for file in files {
        manifest.push_str(
            &file
                .components()
                .map(|part| part.as_os_str().to_str().unwrap())
                .collect::<Vec<_>>()
                .join("/"),
        );
        manifest.push('\n');
    }
    let temporary = root.join("reset-pending.tmp");
    if fs::symlink_metadata(&temporary).is_ok() {
        regular(&temporary, false)?;
        fs::remove_file(&temporary).map_err(|_| "无法清理上次还原准备文件。")?;
    }
    durable_write(&temporary, manifest.as_bytes()).map_err(|_| "无法保存还原清单，未删除数据。")?;
    if let Err(error) = canceled(cancel) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    rename(&temporary, &root.join(RESET_PENDING)).map_err(|_| "无法提交还原，未删除数据。".into())
}

fn pending_reset_files(root: &Path) -> Result<Vec<PathBuf>> {
    regular(root, true)?;
    let marker = root.join(RESET_PENDING);
    regular(&marker, false)?;
    if fs::metadata(&marker)
        .map_err(|_| "无法读取还原清单。")?
        .len()
        > 256 * 1024
    {
        return Err("还原清单过大，未继续删除。".into());
    }
    let text = fs::read_to_string(&marker).map_err(|_| "无法读取还原清单。")?;
    let mut lines = text.lines();
    if lines.next() != Some("password-helper-reset-v1") {
        return Err("还原清单无效，未继续删除。".into());
    }
    let mut files = std::collections::BTreeSet::new();
    for line in lines {
        let path = PathBuf::from(line);
        if line.is_empty() || !reset_owned(&path) || !files.insert(path) || files.len() > 4096 {
            return Err("还原清单包含无效或重复路径，未继续删除。".into());
        }
    }
    if files.is_empty() {
        return Err("还原清单为空，未继续删除。".into());
    }
    if fs::symlink_metadata(root.join("backups")).is_ok() {
        regular(&root.join("backups"), true)?;
    }
    for relative in &files {
        match fs::symlink_metadata(root.join(relative)) {
            Ok(_) => regular(&root.join(relative), false)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法检查还原文件，未继续删除。".into()),
        }
    }
    Ok(files.into_iter().collect())
}

pub fn validate_reset(root: &Path) -> Result<()> {
    pending_reset_files(root).map(|_| ())
}

pub fn finish_reset(root: &Path) -> Result<()> {
    let files = pending_reset_files(root)?;
    // The durable marker is the commit point; cancellation cannot revive deleted records.
    for relative in files {
        let path = root.join(&relative);
        if relative.components().count() > 1 && fs::symlink_metadata(root.join("backups")).is_ok() {
            regular(&root.join("backups"), true)?;
        }
        if fs::symlink_metadata(&path).is_ok() {
            regular(&path, false)?;
        }
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(
                    "还原尚未完成：文件被占用或无法删除。关闭占用后点击继续，或重启软件完成还原。"
                        .into(),
                );
            }
        }
    }
    let backups = root.join("backups");
    if fs::symlink_metadata(&backups).is_ok() {
        regular(&backups, true)?;
        if fs::read_dir(&backups)
            .map_err(|_| "无法检查备份目录。")?
            .next()
            .is_none()
        {
            fs::remove_dir(backups).map_err(|_| "无法清除空备份目录，请继续还原。")?;
        }
    }
    fs::remove_file(root.join(RESET_PENDING))
        .map_err(|_| "数据已清除，但还原记录尚未清理。请继续完成还原。".into())
}

#[cfg(test)]
mod reset_tests {
    use super::*;
    use crate::vault::Credential;
    fn fixture(label: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "reset-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("backups")).unwrap();
        let active = root.join("vault.db");
        let mut vault = Vault::create(&active, "Reset-Only!2026").unwrap();
        vault
            .save(&Credential {
                title: "private".into(),
                ..Default::default()
            })
            .unwrap();
        for name in [
            "backups/auto-123-1.db",
            "restored-456-1.db",
            "vault.pre-v04-789.db",
            "manual-export.db",
        ] {
            vault.backup(&root.join(name)).unwrap();
        }
        drop(vault);
        for name in [
            "theme.txt",
            "appearance.txt",
            "window.txt",
            "shortcut.txt",
            "last-vault.txt",
        ] {
            fs::write(root.join(name), b"original").unwrap();
        }
        (root, active)
    }
    #[test]
    fn reset_authenticates_before_commit_and_clears_all_owned_files_only() {
        let (root, active) = fixture("scope");
        let cancel = AtomicBool::new(false);
        let files = reset_inventory(&root, &active).unwrap();
        assert!(files.iter().any(|p| p == Path::new("restored-456-1.db")));
        assert!(begin_reset(&root, &active, "wrong", &cancel).is_err());
        assert!(!reset_pending(&root));
        assert_eq!(fs::read(root.join("theme.txt")).unwrap(), b"original");
        assert!(active.exists());
        cancel.store(true, Ordering::SeqCst);
        assert!(begin_reset(&root, &active, "Reset-Only!2026", &cancel).is_err());
        assert!(!reset_pending(&root));
        cancel.store(false, Ordering::SeqCst);
        begin_reset(&root, &active, "Reset-Only!2026", &cancel).unwrap();
        assert!(reset_pending(&root));
        assert!(active.exists());
        fs::remove_file(root.join("theme.txt")).unwrap(); // Interrupted after one deletion.
        finish_reset(&root).unwrap();
        assert!(!reset_pending(&root));
        for file in files {
            assert!(!root.join(file).exists());
        }
        assert!(Vault::open(&root.join("manual-export.db"), "Reset-Only!2026").is_ok());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn reset_recovery_rejects_traversal_unknown_names_and_duplicate_entries() {
        let (root, active) = fixture("invalid");
        for line in [
            "../outside.db",
            "manual-export.db",
            "vault.db\nvault.db",
            "/tmp/other.db",
        ] {
            fs::write(
                root.join(RESET_PENDING),
                format!("password-helper-reset-v1\n{line}\n"),
            )
            .unwrap();
            assert!(finish_reset(&root).is_err());
            assert!(active.exists());
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn reset_refuses_links_and_preserves_the_target() {
        let (root, active) = fixture("links");
        let outside = root.with_extension("outside");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("restored-999-1.db")).unwrap();
        assert!(begin_reset(&root, &active, "Reset-Only!2026", &AtomicBool::new(false)).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");
        assert!(!reset_pending(&root));
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn reset_keeps_a_resume_marker_when_windows_holds_a_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let (root, active) = fixture("busy");
        begin_reset(&root, &active, "Reset-Only!2026", &AtomicBool::new(false)).unwrap();
        let blocker = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&active)
            .unwrap();
        assert!(finish_reset(&root).is_err());
        assert!(reset_pending(&root));
        drop(blocker);
        finish_reset(&root).unwrap();
        assert!(!reset_pending(&root));
        assert!(!active.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
