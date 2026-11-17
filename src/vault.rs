//! Local SQLCipher storage. No UI strings or Windows paths belong in this module.
use rusqlite::{Connection, OpenFlags, params};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, String>;
const APP_ID: i32 = 0x504D4752;

#[derive(Default)]
pub struct Credential {
    pub id: i64,
    pub title: String,
    pub username: String,
    pub password: Zeroizing<String>,
    pub category: String,
    pub tags: String,
    pub url: String,
    pub notes: String,
    pub sensitive: bool,
}

pub struct Summary {
    pub id: i64,
    pub title: String,
    pub username: String,
    pub category: String,
    pub tags: String,
    pub sensitive: bool,
    pub url: String,
    pub last_used: i64,
}

impl Summary {
    pub fn matches(&self, query: &str, category: &str, tag: &str) -> bool {
        self.rank(query, category, tag).is_some()
    }
    pub fn rank(&self, query: &str, category: &str, tag: &str) -> Option<i32> {
        let tags: Vec<_> = self
            .tags
            .split([',', '，', ';', '；'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if !category.is_empty() && self.category != category {
            return None;
        }
        if !tag.is_empty() && !tags.contains(&tag) {
            return None;
        }
        self.score(query)
    }
    pub fn score(&self, query: &str) -> Option<i32> {
        crate::search::score(
            query,
            &[
                (&self.title, 120),
                (&self.username, 100),
                (&self.tags, 110),
                (&self.category, 80),
                (&self.url, 60),
            ],
        )
    }
}

pub struct Vault {
    conn: Connection,
    path: PathBuf,
}

fn db<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(|_| "密码库操作失败；请检查主密码、文件完整性和磁盘权限。".into())
}

impl Vault {
    fn connect(path: &Path, master: &str, create: bool, read_only: bool) -> Result<Self> {
        if master.is_empty() || master.len() > 4096 || master.contains('\0') {
            return Err("主密码为空、过长或含有空字符。".into());
        }
        let flags = (if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        }) | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = db(Connection::open_with_flags(path, flags))?;
        let version: String = db(conn.query_row("PRAGMA cipher_version", [], |r| r.get(0)))?;
        if version.is_empty() {
            return Err("当前构建未启用 SQLCipher，拒绝打开密码库。".into());
        }
        // Use the codec API so the master password never becomes part of SQL text.
        let key_len = i32::try_from(master.len()).map_err(|_| "主密码过长。")?;
        let rc =
            unsafe { rusqlite::ffi::sqlite3_key(conn.handle(), master.as_ptr().cast(), key_len) };
        if rc != rusqlite::ffi::SQLITE_OK {
            return Err("无法设置密码库密钥。".into());
        }
        db(conn.execute_batch(
            "PRAGMA cipher_memory_security=ON; PRAGMA temp_store=MEMORY; PRAGMA foreign_keys=ON;",
        ))?;
        db(conn.busy_timeout(Duration::from_secs(2)))?;
        // Reading schema authenticates the password before any persistent settings change.
        db(
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            }),
        )?;
        if !create {
            let app_id: i32 = db(conn.query_row("PRAGMA application_id", [], |r| r.get(0)))?;
            let schema: i32 = db(conn.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
            if app_id != APP_ID || !(schema == 2 || schema == 1) {
                return Err("不是受支持的密码库版本，未进行修改。".into());
            }
        }
        if !read_only {
            let journal: String =
                db(conn.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0)))?;
            if !journal.eq_ignore_ascii_case("delete") {
                return Err("无法设置密码库日志模式。".into());
            }
            db(conn.execute_batch("PRAGMA synchronous=FULL;"))?;
        }
        Ok(Self {
            conn,
            path: path.to_owned(),
        })
    }

    pub fn create(path: &Path, master: &str) -> Result<Self> {
        if master.chars().count() < 10 {
            return Err("主密码至少需要 10 个字符。".into());
        }
        // Reserve without truncation: an existing vault must never be overwritten.
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| "无法创建密码库：文件已存在或目录不可写。")?;
        drop(file);
        let result = (|| {
            let vault = Self::connect(path, master, true, false)?;
            db(vault.conn.execute_batch(&format!(
                "BEGIN IMMEDIATE; PRAGMA application_id={APP_ID}; PRAGMA user_version=2;
                CREATE TABLE credentials (
                    id INTEGER PRIMARY KEY, title TEXT NOT NULL, username TEXT NOT NULL,
                    password TEXT NOT NULL, category TEXT NOT NULL, tags TEXT NOT NULL,
                    url TEXT NOT NULL, notes TEXT NOT NULL, sensitive INTEGER NOT NULL DEFAULT 0 CHECK(sensitive IN (0,1)));
                COMMIT;"
            )))?;
            vault.ensure_usage()?;
            Ok(vault)
        })();
        if result.is_err() {
            let _ = fs::remove_file(path);
        }
        result
    }

    pub fn open(path: &Path, master: &str) -> Result<Self> {
        let mut vault = Self::connect(path, master, false, false)?;
        let version: i32 = db(vault
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0)))?;
        if version == 1 {
            let backup = path.with_extension(format!(
                "pre-v04-{}.db",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            vault.backup(&backup)?;
            db(vault.conn.execute_batch("BEGIN IMMEDIATE; ALTER TABLE credentials ADD COLUMN sensitive INTEGER NOT NULL DEFAULT 0 CHECK(sensitive IN (0,1)); PRAGMA user_version=2; COMMIT;"))?;
        }
        vault.ensure_usage()?;
        Ok(vault)
    }

    fn ensure_usage(&self) -> Result<()> {
        db(self.conn.execute_batch("CREATE TABLE IF NOT EXISTS usage (credential_id INTEGER PRIMARY KEY REFERENCES credentials(id) ON DELETE CASCADE, last_used INTEGER NOT NULL CHECK(last_used >= 0)); CREATE TABLE IF NOT EXISTS application_usage (application TEXT NOT NULL, credential_id INTEGER NOT NULL REFERENCES credentials(id) ON DELETE CASCADE, last_used INTEGER NOT NULL CHECK(last_used >= 0), PRIMARY KEY(application,credential_id));"))
    }

    pub fn record_used(&mut self, id: i64) -> Result<()> {
        self.record_used_in_application(id, "")
    }

    pub fn recent_for_application(&self, application: &str) -> Result<HashMap<i64, i64>> {
        if application.is_empty() {
            return Ok(HashMap::new());
        }
        let mut statement = db(self.conn.prepare(
            "SELECT credential_id,last_used FROM application_usage WHERE application=?1",
        ))?;
        let rows = db(statement.query_map([application], |row| Ok((row.get(0)?, row.get(1)?))))?;
        db(rows.collect())
    }

    pub fn record_used_in_application(&mut self, id: i64, application: &str) -> Result<()> {
        if application.len() > 1024 || application.contains('\0') {
            return Err("应用标识无效。".into());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
            .min(i64::MAX as u128 - 1) as i64;
        let transaction = db(self.conn.transaction())?;
        db(transaction.execute("INSERT INTO usage(credential_id,last_used) VALUES(?1,max(?2,coalesce((SELECT max(last_used) FROM usage),0)+1)) ON CONFLICT(credential_id) DO UPDATE SET last_used=excluded.last_used", params![id,now]))?;
        if !application.is_empty() {
            db(transaction.execute("INSERT INTO application_usage(application,credential_id,last_used) SELECT ?1,credential_id,last_used FROM usage WHERE credential_id=?2 ON CONFLICT(application,credential_id) DO UPDATE SET last_used=excluded.last_used", params![application,id]))?;
        }
        db(transaction.commit())
    }

    pub fn verify_master(path: &Path, master: &str) -> Result<()> {
        Self::connect(path, master, false, true)?.check()
    }

    pub(crate) fn rekey_copy(
        source: &Path,
        destination: &Path,
        old: &str,
        new: &str,
    ) -> Result<()> {
        let mut original = Self::connect(source, old, false, false)?;
        original.backup(destination)?;
        drop(original);
        let copy = Self::connect(destination, old, false, false)?;
        // The codec API keeps both passphrases out of SQL text and preserves old schemas.
        let rc = unsafe {
            rusqlite::ffi::sqlite3_rekey(copy.conn.handle(), new.as_ptr().cast(), new.len() as i32)
        };
        if rc != rusqlite::ffi::SQLITE_OK {
            return Err("备份重新加密失败，原文件未被替换。".into());
        }
        copy.check()?;
        drop(copy);
        Self::verify_master(destination, new)?;
        OpenOptions::new()
            .write(true)
            .open(destination)
            .and_then(|file| file.sync_all())
            .map_err(|_| "无法同步重新加密的文件。".to_string())
    }

    pub fn is_sensitive(&self, id: i64) -> Result<bool> {
        db(self
            .conn
            .query_row("SELECT sensitive FROM credentials WHERE id=?1", [id], |r| {
                r.get(0)
            }))
    }

    pub fn title(&self, id: i64) -> Result<String> {
        db(self
            .conn
            .query_row("SELECT title FROM credentials WHERE id=?1", [id], |r| {
                r.get(0)
            }))
    }

    pub fn engine_info(&self) -> Result<String> {
        let version: String = db(self
            .conn
            .query_row("PRAGMA cipher_version", [], |r| r.get(0)))?;
        let provider: String = db(self
            .conn
            .query_row("PRAGMA cipher_provider", [], |r| r.get(0)))?;
        let temp: i32 = db(self.conn.query_row("PRAGMA temp_store", [], |r| r.get(0)))?;
        let kdf: String = db(self.conn.query_row("PRAGMA kdf_iter", [], |r| r.get(0)))?;
        if temp != 2 {
            return Err("未启用内存临时存储。".into());
        }
        Ok(format!(
            "SQLCipher={version}, provider={provider}, temp_store=MEMORY, kdf_iter={kdf}"
        ))
    }

    pub fn list(&self) -> Result<Vec<Summary>> {
        let mut statement = db(self.conn.prepare(
            "SELECT c.id,c.title,c.username,c.category,c.tags,c.sensitive,c.url,coalesce(u.last_used,0) FROM credentials c LEFT JOIN usage u ON u.credential_id=c.id ORDER BY c.title,c.id",
        ))?;
        let rows = db(statement.query_map([], |r| {
            Ok(Summary {
                id: r.get(0)?,
                title: r.get(1)?,
                username: r.get(2)?,
                category: r.get(3)?,
                tags: r.get(4)?,
                sensitive: r.get(5)?,
                url: r.get(6)?,
                last_used: r.get(7)?,
            })
        }))?;
        db(rows.collect())
    }

    pub fn get(&self, id: i64) -> Result<Credential> {
        db(self.conn.query_row("SELECT id,title,username,password,category,tags,url,notes,sensitive FROM credentials WHERE id=?1", [id], |r| Ok(Credential {
            id: r.get(0)?, title: r.get(1)?, username: r.get(2)?, password: Zeroizing::new(r.get(3)?),
            category: r.get(4)?, tags: r.get(5)?, url: r.get(6)?, notes: r.get(7)?, sensitive: r.get(8)?,
        })))
    }

    pub fn save(&mut self, record: &Credential) -> Result<i64> {
        if record.title.trim().is_empty() {
            return Err("名称不能为空。".into());
        }
        if [
            &record.title,
            &record.username,
            &record.password,
            &record.category,
            &record.tags,
            &record.url,
            &record.notes,
        ]
        .iter()
        .any(|s| s.len() > 65536 || s.contains('\0'))
        {
            return Err("字段过长或包含空字符。".into());
        }
        let tx = db(self.conn.transaction())?;
        let values = params![
            record.title,
            record.username,
            record.password.as_str(),
            record.category,
            record.tags,
            record.url,
            record.notes,
            record.sensitive,
            record.id
        ];
        let id = if record.id == 0 {
            db(tx.execute("INSERT INTO credentials(title,username,password,category,tags,url,notes,sensitive) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![record.title, record.username, record.password.as_str(), record.category, record.tags, record.url, record.notes, record.sensitive]))?;
            tx.last_insert_rowid()
        } else {
            let changed = db(tx.execute("UPDATE credentials SET title=?1,username=?2,password=?3,category=?4,tags=?5,url=?6,notes=?7,sensitive=?8 WHERE id=?9", values))?;
            if changed != 1 {
                return Err("账号已不存在，请刷新后重试。".into());
            }
            record.id
        };
        db(tx.commit())?;
        Ok(id)
    }

    pub fn delete(&mut self, id: i64) -> Result<()> {
        let tx = db(self.conn.transaction())?;
        if db(tx.execute("DELETE FROM credentials WHERE id=?1", [id]))? != 1 {
            return Err("账号已不存在。".into());
        }
        db(tx.commit())
    }

    pub fn check(&self) -> Result<()> {
        let mut stmt = db(self.conn.prepare("PRAGMA cipher_integrity_check"))?;
        let mut rows = db(stmt.query([]))?;
        if db(rows.next())?.is_some() {
            return Err("密码库加密完整性检查失败。".into());
        }
        let check: String = db(self
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0)))?;
        if check != "ok" {
            return Err("密码库结构完整性检查失败。".into());
        }
        Ok(())
    }

    pub fn backup(&mut self, destination: &Path) -> Result<()> {
        self.check()?;
        // DELETE journal mode + a writer reservation gives a committed encrypted snapshot.
        let tx = db(self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate))?;
        let mut source = fs::File::open(&self.path).map_err(|_| "无法读取密码库。")?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(|_| "无法创建备份；请选择不存在的新文件。")?;
        let result = std::io::copy(&mut source, &mut output).and_then(|_| output.sync_all());
        if result.is_err() {
            drop(output);
            let _ = fs::remove_file(destination);
            return Err("备份失败，可能磁盘空间不足。".into());
        }
        db(tx.commit())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn application_usage_is_scoped_persistent_and_cascades_on_delete() {
        let root =
            std::env::temp_dir().join(format!("application-usage-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("vault.db");
        let mut vault = super::Vault::create(&path, "Application-Only!2026").unwrap();
        let id = vault
            .save(&super::Credential {
                title: "VPN".into(),
                ..Default::default()
            })
            .unwrap();
        vault.record_used_in_application(id, "atrust.exe").unwrap();
        assert!(
            vault
                .recent_for_application("atrust.exe")
                .unwrap()
                .contains_key(&id)
        );
        assert!(
            vault
                .recent_for_application("chrome.exe")
                .unwrap()
                .is_empty()
        );
        assert!(vault.record_used_in_application(id, "bad\0name").is_err());
        assert!(
            vault
                .record_used_in_application(i64::MAX, "atrust.exe")
                .is_err()
        );
        drop(vault);
        let mut vault = super::Vault::open(&path, "Application-Only!2026").unwrap();
        assert!(
            vault
                .recent_for_application("atrust.exe")
                .unwrap()
                .contains_key(&id)
        );
        vault.delete(id).unwrap();
        assert!(
            vault
                .recent_for_application("atrust.exe")
                .unwrap()
                .is_empty()
        );
        drop(vault);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_is_monotonic_encrypted_persistent_and_cascades_on_delete() {
        use super::*;
        let root = std::env::temp_dir().join(format!("usage-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("vault.db");
        let mut vault = Vault::create(&path, "Usage-Test-Only!2026").unwrap();
        let first = vault
            .save(&Credential {
                title: "邮箱".into(),
                ..Default::default()
            })
            .unwrap();
        let second = vault
            .save(&Credential {
                title: "aTrust".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(vault.list().unwrap().iter().all(|r| r.last_used == 0));
        vault.record_used(first).unwrap();
        let stamp = vault
            .list()
            .unwrap()
            .into_iter()
            .find(|r| r.id == first)
            .unwrap()
            .last_used;
        vault.record_used(second).unwrap();
        vault.record_used(first).unwrap();
        let rows = vault.list().unwrap();
        assert!(rows.iter().find(|r| r.id == first).unwrap().last_used > stamp);
        assert!(vault.record_used(i64::MAX).is_err());
        drop(vault);
        let mut vault = Vault::open(&path, "Usage-Test-Only!2026").unwrap();
        assert!(
            vault
                .list()
                .unwrap()
                .into_iter()
                .find(|r| r.id == second)
                .unwrap()
                .last_used
                > stamp
        );
        vault.delete(first).unwrap();
        assert_eq!(
            vault
                .conn
                .query_row("SELECT count(*) FROM usage", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            1
        );
        drop(vault);
        assert_ne!(&std::fs::read(&path).unwrap()[..16], b"SQLite format 3\0");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn summary_search_filters_without_loading_passwords() {
        let row = super::Summary {
            id: 1,
            title: "aTrust".into(),
            username: "demo".into(),
            category: "工作".into(),
            tags: "客户端， 测试; VPN".into(),
            sensitive: false,
            url: String::new(),
            last_used: 0,
        };
        assert!(row.matches(" ATRUST 客户端 ", "工作", "VPN"));
        assert!(!row.matches("", "个人", ""));
        assert!(!row.matches("", "", "VP"));
        assert!(!row.matches("secret-password", "", ""));
    }
    #[test]
    fn legacy_upgrade_preserves_records_and_creates_backup() {
        use super::*;
        let path = std::env::temp_dir().join(format!(
            "legacy-vault-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let master = "Legacy-Test-Only!2026";
        let old = Vault::create(&path, master).unwrap();
        old.conn.execute_batch("ALTER TABLE credentials DROP COLUMN sensitive; PRAGMA user_version=1; INSERT INTO credentials(title,username,password,category,tags,url,notes) VALUES('legacy','user','secret','','','','');").unwrap();
        drop(old);
        let mut migrated = Vault::open(&path, master).unwrap();
        let row = migrated.list().unwrap().remove(0);
        assert!(!row.sensitive);
        let mut record = migrated.get(row.id).unwrap();
        assert_eq!(record.password.as_str(), "secret");
        record.sensitive = true;
        migrated.save(&record).unwrap();
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(path.file_stem().unwrap().to_str().unwrap())
                    && p.extension().is_some_and(|e| e == "db")
                    && p != &path
            })
            .collect();
        assert_eq!(backups.len(), 1);
        let backup = Vault::connect(&backups[0], master, false, false).unwrap();
        let version: i32 = backup
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        drop(backup);
        let changed = path.with_extension("rekey-test.db");
        Vault::rekey_copy(&backups[0], &changed, master, "New-Legacy-Master!2026").unwrap();
        let rekeyed = Vault::connect(&changed, "New-Legacy-Master!2026", false, true).unwrap();
        assert_eq!(
            rekeyed
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            1
        );
        drop(rekeyed);
        std::fs::remove_file(changed).unwrap();
        drop(migrated);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&backups[0]).unwrap();
    }
    #[test]
    fn encrypted_lifecycle() {
        super::storage_self_check().unwrap();
    }
}

/// Runs with synthetic data in an isolated directory; also executable on Windows.
pub fn storage_self_check() -> Result<String> {
    let root = std::env::temp_dir().join(format!(
        "password-manager-storage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).map_err(|_| "无法创建测试目录。")?;
    let result = (|| {
        let master = "Storage-Test-Only!2026";
        let path = root.join("密码库.db");
        let backup = root.join("备份.db");
        assert!(Vault::open(&path, master).is_err());
        assert!(!path.exists());
        let mut vault = Vault::create(&path, master)?;
        let info = vault.engine_info()?;
        let record = Credential {
            title: "unique-title-测试".into(),
            username: "unique-user@example.invalid".into(),
            password: Zeroizing::new("unique-password-🔒€".into()),
            category: "工作".into(),
            tags: "客户端,测试".into(),
            notes: "多行\n备注".into(),
            ..Default::default()
        };
        assert!(vault.save(&Credential::default()).is_err());
        let id = vault.save(&record)?;
        assert!(Vault::create(&path, master).is_err());
        vault.check()?;
        vault.backup(&backup)?;
        assert!(vault.backup(&backup).is_err());
        let bytes = fs::read(&path).unwrap();
        for needle in [
            record.title.as_bytes(),
            record.username.as_bytes(),
            record.password.as_bytes(),
            record.category.as_bytes(),
        ] {
            assert!(!bytes.windows(needle.len()).any(|w| w == needle));
        }
        assert_ne!(&bytes[..16], b"SQLite format 3\0");
        db(vault.conn.execute_batch("CREATE TEMP TRIGGER fail_update BEFORE UPDATE ON credentials BEGIN SELECT RAISE(ABORT, 'simulated write failure'); END;"))?;
        let mut rejected = vault.get(id)?;
        rejected.title = "must-not-be-committed".into();
        assert!(vault.save(&rejected).is_err());
        assert_eq!(vault.get(id)?.title, record.title);
        db(vault.conn.execute_batch("DROP TRIGGER fail_update"))?;
        let mut edited = vault.get(id)?;
        edited.password = Zeroizing::new("changed-password".into());
        edited.sensitive = true;
        vault.save(&edited)?;
        drop(vault);
        assert!(Vault::open(&path, "wrong-password").is_err());
        let mut vault = Vault::open(&path, master)?;
        assert_eq!(vault.get(id)?.password.as_str(), "changed-password");
        assert!(vault.is_sensitive(id)?);
        assert!(vault.list()?[0].sensitive);
        Vault::verify_master(&path, master)?;
        assert!(Vault::verify_master(&path, "wrong-password").is_err());
        assert_eq!(vault.list()?.len(), 1);
        vault.delete(id)?;
        assert!(vault.list()?.is_empty());
        assert!(vault.save(&edited).is_err());
        drop(vault);
        let restored = Vault::open(&backup, master)?;
        restored.check()?;
        assert_eq!(
            restored.get(id)?.password.as_str(),
            record.password.as_str()
        );
        drop(restored);
        let mut damaged = fs::read(&backup).unwrap();
        damaged[100] ^= 0x80;
        fs::write(root.join("damaged.db"), damaged).unwrap();
        assert!(
            Vault::open(&root.join("damaged.db"), master)
                .and_then(|v| v.check())
                .is_err()
        );
        Ok(format!(
            "{info}\nPASS: create, CRUD, reopen, wrong key, encrypted bytes, backup/restore, corruption, overwrite protection"
        ))
    })();
    let _ = fs::remove_dir_all(root);
    result
}
