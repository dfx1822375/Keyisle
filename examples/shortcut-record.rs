#[cfg(windows)]
#[allow(dead_code)]
#[path = "../src/recording.rs"]
mod recording;
#[cfg(windows)]
fn main() {
    let owner = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    std::process::exit(recording::hook_helper(owner));
}
#[cfg(not(windows))]
fn main() {}
