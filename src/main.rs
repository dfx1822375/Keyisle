#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod recording;
#[cfg(windows)]
mod windows_auth;

#[cfg(windows)]
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--windows-auth-helper") {
        std::process::exit(windows_auth::helper());
    }
    if std::env::args().nth(1).as_deref() == Some("--shortcut-record-helper") {
        let owner = std::env::args()
            .nth(2)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        std::process::exit(recording::hook_helper(owner));
    }
    if let Err(error) = app::run() {
        eprintln!("{error}");
        platform::show_error(&error);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("此原型需要 Windows。WSL 中请运行 scripts/build-windows.sh 生成 Windows 程序。");
}
