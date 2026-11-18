use std::path::Path;
use windows_password_manager::vault::{Credential, Vault};
use zeroize::Zeroizing;

struct DemoRecord {
    title: &'static str,
    username: &'static str,
    password: &'static str,
    category: &'static str,
    tags: &'static [&'static str],
}

// Synthetic test fixtures used only by isolated verification scripts.
const DEMO: [DemoRecord; 5] = [
    DemoRecord {
        title: "Chrome 示例网站",
        username: "demo.web@example.invalid",
        password: "Demo-Web!2026",
        category: "工作 / 示例项目",
        tags: &["网页", "测试"],
    },
    DemoRecord {
        title: "aTrust 示例账号",
        username: "demo.atrust",
        password: "Demo-aTrust!2026",
        category: "工作 / 示例项目",
        tags: &["客户端", "测试"],
    },
    DemoRecord {
        title: "数据库示例",
        username: "demo.reader",
        password: "Demo-DB#2026",
        category: "工作 / 示例项目",
        tags: &["数据库", "只读"],
    },
    DemoRecord {
        title: "个人邮箱示例",
        username: "demo.mail@example.invalid",
        password: "Demo-Mail@2026",
        category: "个人",
        tags: &["网页", "邮箱"],
    },
    DemoRecord {
        title: "特殊字符测试",
        username: "demo.unicode",
        password: "示例!@#€🔒",
        category: "测试",
        tags: &["测试", "特殊字符"],
    },
];

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let started = std::time::Instant::now();
    if args.len() == 3 {
        let path = Path::new(&args[2]);
        let master = "Cross-Backend-Test!2026";
        match args[1].as_str() {
            "--read-v07-ui-fixture" => {
                let original = Vault::open(&path.join("vault.db"), "GUI-Master-Only!2026")?;
                assert_eq!(original.list()?.len(), 3);
                let preference = std::fs::read_to_string(path.join("last-vault.txt"))
                    .map_err(|_| "fixture preference missing")?;
                let name = preference.rsplit(['\\', '/']).next().unwrap();
                assert!(name.starts_with("restored-"));
                let restored = Vault::open(&path.join(name), "GUI-Master-Only!2026")?;
                let rows = restored.list()?;
                assert_eq!(rows.len(), 1);
                let row = restored.get(rows[0].id)?;
                assert_eq!(row.title, "Alpha");
                assert_eq!(row.username, "alpha.user");
                assert_eq!(row.password.as_str(), "Alpha-secret!2026");
                println!(
                    "PASS: GUI restore preserves original three-record vault and restores exact one-record snapshot"
                );
            }
            "--read-maintenance-fixture" => {
                let vault = Vault::open(path, "GUI-New-Master!2026")?;
                vault.check()?;
                let records = vault.list()?;
                assert_eq!(records.len(), 1);
                let record = vault.get(records[0].id)?;
                assert_eq!(record.username, "synthetic.user");
                assert_eq!(record.password.as_str(), "Synthetic-Account!2026");
                assert!(Vault::verify_master(path, "GUI-Master-Only!2026").is_err());
                println!("PASS: rotated GUI fixture preserves encrypted credential contents");
            }
            "--seed-demo" | "--seed-feedback" | "--seed-context-fixture" => {
                let mut vault = Vault::create(path, master)?;
                for record in DEMO {
                    vault.save(&Credential {
                        title: record.title.into(),
                        username: record.username.into(),
                        password: Zeroizing::new(record.password.into()),
                        category: record.category.into(),
                        tags: record.tags.join(","),
                        ..Default::default()
                    })?;
                }
                if args[1] == "--seed-context-fixture" {
                    let records = vault.list()?;
                    let atrust = records
                        .iter()
                        .find(|r| r.title.to_lowercase().contains("atrust"))
                        .ok_or("missing aTrust fixture")?
                        .id;
                    let chrome = records
                        .iter()
                        .find(|r| r.title.to_lowercase().contains("chrome"))
                        .ok_or("missing Chrome fixture")?
                        .id;
                    vault.record_used_in_application(atrust, "powershell.exe")?;
                    vault.record_used_in_application(chrome, "chrome.exe")?;
                }
                if args[1] == "--seed-feedback" {
                    for (title, username, password) in [
                        ("Feedback empty", "", ""),
                        ("Feedback control", "valid.user", "bad\npassword"),
                    ] {
                        vault.save(&Credential {
                            title: title.into(),
                            username: username.into(),
                            password: Zeroizing::new(password.into()),
                            category: "Feedback".into(),
                            tags: "invalid,test".into(),
                            ..Default::default()
                        })?;
                    }
                }
                vault.check()?;
                println!("PASS: isolated demo fixture created");
            }
            "--write-fixture" => {
                let mut vault = Vault::create(path, master)?;
                vault.save(&Credential {
                    title: "cross-backend-title".into(),
                    username: "跨平台-user".into(),
                    password: Zeroizing::new("cross-backend-密码🔒".into()),
                    ..Default::default()
                })?;
                vault.check()?;
                println!("{}", vault.engine_info()?);
            }
            "--read-fixture" => {
                let vault = Vault::open(path, master)?;
                vault.check()?;
                let records = vault.list()?;
                assert_eq!(records.len(), 1);
                let record = vault.get(records[0].id)?;
                assert_eq!(record.title, "cross-backend-title");
                assert_eq!(record.username, "跨平台-user");
                assert_eq!(record.password.as_str(), "cross-backend-密码🔒");
                println!("{}", vault.engine_info()?);
            }
            _ => return Err("Unknown fixture operation".into()),
        }
        println!("PASS: cross-backend fixture");
    } else {
        println!("{}", windows_password_manager::vault::storage_self_check()?);
    }
    println!("Elapsed: {:.2}s", started.elapsed().as_secs_f64());
    Ok(())
}
fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
