use crate::FillMode;
use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub const LOCK_MINUTES: [u32; 5] = [1, 5, 15, 30, 60];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleSource {
    Computer,
    Vault,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockSettings {
    pub minutes: u32,
    pub source: IdleSource,
}
impl Default for LockSettings {
    fn default() -> Self {
        Self {
            minutes: 5,
            source: IdleSource::Computer,
        }
    }
}
impl LockSettings {
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("lock-settings.txt");
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|_| "无法读取锁定设置。")?;
        Self::parse(&text)
    }
    fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<_> = text.lines().collect();
        if parts.len() != 2 {
            return Err("锁定设置损坏，请移走 lock-settings.txt 后重试。".into());
        }
        let minutes = parts[0].parse::<u32>().map_err(|_| "锁定时间无效。")?;
        if !LOCK_MINUTES.contains(&minutes) {
            return Err("不支持的锁定时间。".into());
        }
        let source = match parts[1] {
            "computer" => IdleSource::Computer,
            "vault" => IdleSource::Vault,
            _ => return Err("锁定计时方式无效。".into()),
        };
        Ok(Self { minutes, source })
    }
    pub fn save(&self, root: &Path) -> Result<(), String> {
        let text = format!(
            "{}\n{}\n",
            self.minutes,
            if self.source == IdleSource::Computer {
                "computer"
            } else {
                "vault"
            }
        );
        Self::parse(&text)?;
        let temporary = root.join("lock-settings.tmp");
        fs::write(&temporary, text)
            .and_then(|_| fs::rename(temporary, root.join("lock-settings.txt")))
            .map_err(|_| "无法保存锁定设置，原设置保持有效。".into())
    }
    fn expired(self, computer_idle: Option<u32>, vault_idle: Duration) -> bool {
        let threshold = self.minutes * 60;
        match self.source {
            IdleSource::Computer => computer_idle.is_none_or(|seconds| seconds >= threshold),
            IdleSource::Vault => vault_idle >= Duration::from_secs(threshold.into()),
        }
    }
}

#[derive(Default)]
pub struct MasterAttempts {
    failures: u8,
    retry_at: Option<Instant>,
}

impl MasterAttempts {
    pub fn retry_message(&self, now: Instant) -> Option<String> {
        let delay = self.retry_at?.checked_duration_since(now)?;
        if delay.is_zero() {
            return None;
        }
        Some(format!(
            "多次验证失败，请在 {} 秒后重试。",
            delay.as_secs() + u64::from(delay.subsec_nanos() != 0)
        ))
    }

    pub fn rejected(&mut self, now: Instant) -> Option<String> {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 5 {
            self.retry_at = Some(now + Duration::from_secs(5));
        }
        self.retry_message(now)
    }

    pub fn accepted(&mut self) {
        *self = Self::default();
    }
}

pub fn master_strength_hint(value: &str) -> (i32, &'static str) {
    let length = value.chars().count();
    if value.is_empty() {
        (0, "")
    } else if length < 10 {
        (1, "主密码至少需要 10 个字符。")
    } else if value.bytes().all(|c| c.is_ascii_digit())
        || fewer_than_five_distinct_chars(value)
        || is_ascii_sequence(value)
        || [
            "password", "qwerty", "admin", "letmein", "keyisle", "123456",
        ]
        .iter()
        .any(|pattern| {
            value
                .as_bytes()
                .windows(pattern.len())
                .any(|slice| slice.eq_ignore_ascii_case(pattern.as_bytes()))
        })
    {
        (1, "这个主密码容易猜到，建议用多个互不相关的词。")
    } else if length < 16 {
        (2, "建议使用至少 16 个字符。")
    } else {
        (3, "主密码强度良好")
    }
}

fn fewer_than_five_distinct_chars(value: &str) -> bool {
    let mut seen = ['\0'; 5];
    let mut count = 0;
    for character in value.chars().map(|c| c.to_ascii_lowercase()) {
        if !seen[..count].contains(&character) {
            seen[count] = character;
            count += 1;
            if count == seen.len() {
                return false;
            }
        }
    }
    true
}

fn is_ascii_sequence(value: &str) -> bool {
    let bytes = value.as_bytes();
    let [first, second, ..] = bytes else {
        return false;
    };
    let step = i16::from(second.to_ascii_lowercase()) - i16::from(first.to_ascii_lowercase());
    step.abs() == 1
        && bytes.windows(2).all(|pair| {
            pair.iter().all(u8::is_ascii_alphabetic)
                && i16::from(pair[1].to_ascii_lowercase()) - i16::from(pair[0].to_ascii_lowercase())
                    == step
        })
}

// Shared with the native timer so file dialogs cannot suspend idle locking.
pub struct LockClock(Mutex<(LockSettings, Instant, Option<u32>)>);
impl LockClock {
    pub fn new(settings: LockSettings) -> Self {
        Self(Mutex::new((settings, Instant::now(), None)))
    }
    pub fn touch(&self) {
        self.0.lock().unwrap().1 = Instant::now();
    }
    pub fn update(&self, settings: LockSettings) {
        let mut state = self.0.lock().unwrap();
        state.0 = settings;
        state.1 = Instant::now();
    }
    pub fn observe_input(&self, tick: u32, own_foreground: bool) {
        let mut state = self.0.lock().unwrap();
        if state.2 != Some(tick) {
            state.2 = Some(tick);
            if own_foreground {
                state.1 = Instant::now();
            }
        }
    }
    pub fn settings(&self) -> LockSettings {
        self.0.lock().unwrap().0
    }
    pub fn expired(&self, computer_idle: Option<u32>) -> bool {
        let (settings, used, _) = *self.0.lock().unwrap();
        settings.expired(computer_idle, used.elapsed())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedAction {
    View(i64),
    Edit(i64),
    Delete(u64),
    Fill(i64, FillMode),
}
impl ProtectedAction {
    pub fn record(self) -> Option<i64> {
        match self {
            Self::View(id) | Self::Edit(id) | Self::Fill(id, _) => Some(id),
            Self::Delete(_) => None,
        }
    }
}
pub struct Grant {
    action: ProtectedAction,
    generation: u64,
    verified: Instant,
}
impl Grant {
    pub fn new(action: ProtectedAction, generation: u64) -> Self {
        Self {
            action,
            generation,
            verified: Instant::now(),
        }
    }
    pub fn consume(slot: &mut Option<Self>, action: ProtectedAction, generation: u64) -> bool {
        slot.take().is_some_and(|grant| {
            grant.action == action
                && grant.generation == generation
                && grant.verified.elapsed() < Duration::from_secs(60)
        })
    }
}

fn fill_random_bytes(buf: &mut [u8]) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{
            BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
        };
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err("无法从系统安全随机源生成密码。".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::io::Read;
        fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(buf))
            .map_err(|_| "无法从系统安全随机源生成密码。".into())
    }
}

/// Generates a high-entropy random password using OS CSPRNG without touching the clipboard.
pub fn generate_password(
    length: usize,
    include_symbols: bool,
) -> Result<Zeroizing<String>, String> {
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
    const DIGITS: &[u8] = b"23456789";
    const SYMBOLS: &[u8] = b"!@#$%^&*-_=+?";
    if !(12..=64).contains(&length) {
        return Err("生成密码长度需在 12～64 位之间。".into());
    }
    let mut pool = Zeroizing::new([0u8; 128]);
    let mut cursor = pool.len();
    let mut uniform = |bound: usize| -> Result<usize, String> {
        let limit = 256 - (256 % bound);
        loop {
            if cursor >= pool.len() {
                fill_random_bytes(&mut *pool)?;
                cursor = 0;
            }
            let byte = pool[cursor] as usize;
            cursor += 1;
            if byte < limit {
                return Ok(byte % bound);
            }
        }
    };
    let mut classes: Vec<&[u8]> = vec![UPPER, LOWER, DIGITS];
    if include_symbols {
        classes.push(SYMBOLS);
    }
    let mut all = Vec::with_capacity(72);
    for class in &classes {
        all.extend_from_slice(class);
    }
    let mut out = Zeroizing::new(Vec::with_capacity(length));
    for class in &classes {
        out.push(class[uniform(class.len())?]);
    }
    while out.len() < length {
        out.push(all[uniform(all.len())?]);
    }
    for i in (1..out.len()).rev() {
        let j = uniform(i + 1)?;
        out.swap(i, j);
    }
    let mut text = String::with_capacity(length);
    for &b in out.iter() {
        text.push(b as char);
    }
    all.zeroize();
    Ok(Zeroizing::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_attempts_cool_down_and_success_resets_the_count() {
        let now = Instant::now();
        let mut attempts = MasterAttempts::default();
        for _ in 0..4 {
            assert!(attempts.rejected(now).is_none());
        }
        assert!(attempts.rejected(now).unwrap().contains('5'));
        assert!(
            attempts
                .retry_message(now + Duration::from_secs(4))
                .is_some()
        );
        assert!(
            attempts
                .retry_message(now + Duration::from_secs(5))
                .is_none()
        );
        attempts.accepted();
        assert!(attempts.rejected(now).is_none());
        assert_eq!(master_strength_hint(""), (0, ""));
        assert_eq!(master_strength_hint("abc").0, 1);
        for weak in [
            "qWeRtY-Example",
            "中文开头-my-Password-7890",
            "my-Keyisle-example",
            "1234567890123456",
            "1111122222333333",
            "abcdefghij",
            "ZYXWVUTSRQ",
        ] {
            let (level, hint) = master_strength_hint(weak);
            assert_eq!(level, 1);
            assert!(hint.contains("容易猜到"));
        }
        assert_eq!(master_strength_hint("tree-delta-plum").0, 2);
        assert_eq!(master_strength_hint("tree-delta-comet-plum").0, 3);
    }

    #[test]
    fn settings_and_clock_distinguish_computer_from_vault_idle() {
        assert_eq!(
            LockSettings::default(),
            LockSettings::parse("5\ncomputer\n").unwrap()
        );
        for input in [
            "0\ncomputer",
            "999\nvault",
            "5\nnever",
            "5\ncomputer\nextra",
        ] {
            assert!(LockSettings::parse(input).is_err());
        }
        let computer = LockSettings::default();
        assert!(!computer.expired(Some(299), Duration::from_secs(999)));
        assert!(computer.expired(Some(300), Duration::ZERO));
        assert!(computer.expired(None, Duration::ZERO));
        let vault = LockSettings {
            minutes: 1,
            source: IdleSource::Vault,
        };
        assert!(!vault.expired(Some(999), Duration::from_secs(59)));
        assert!(vault.expired(Some(0), Duration::from_secs(60)));
        let clock = LockClock::new(computer);
        clock.update(vault);
        assert_eq!(clock.settings(), vault);
        assert!(!clock.expired(Some(999)));
    }
    #[test]
    fn only_new_input_in_our_foreground_window_resets_vault_idle() {
        let clock = LockClock::new(LockSettings {
            minutes: 1,
            source: IdleSource::Vault,
        });
        clock.0.lock().unwrap().1 = Instant::now() - Duration::from_secs(61);
        clock.observe_input(1, false);
        assert!(clock.expired(Some(0)));
        clock.observe_input(2, true);
        assert!(!clock.expired(Some(0)));
        clock.0.lock().unwrap().1 = Instant::now() - Duration::from_secs(61);
        clock.observe_input(2, true);
        assert!(clock.expired(Some(0)));
    }
    #[test]
    fn grants_are_single_use_and_bound_to_record_action_session_and_time() {
        let edit = ProtectedAction::Edit(7);
        let mut grant = Some(Grant::new(edit, 1));
        assert!(Grant::consume(&mut grant, edit, 1));
        assert!(!Grant::consume(&mut grant, edit, 1));
        for (action, generation) in [
            (ProtectedAction::Edit(8), 1),
            (ProtectedAction::View(7), 1),
            (ProtectedAction::Delete(7), 1),
            (edit, 2),
        ] {
            let mut grant = Some(Grant::new(edit, 1));
            assert!(!Grant::consume(&mut grant, action, generation));
            assert!(grant.is_none());
        }
        let mut expired = Some(Grant {
            action: edit,
            generation: 1,
            verified: Instant::now() - Duration::from_secs(61),
        });
        assert!(!Grant::consume(&mut expired, edit, 1));
        let mut fill = Some(Grant::new(ProtectedAction::Fill(7, FillMode::Password), 1));
        assert!(!Grant::consume(
            &mut fill,
            ProtectedAction::Fill(7, FillMode::Username),
            1
        ));
    }
    #[test]
    fn password_generator_enforces_length_character_classes_and_fill_compatibility() {
        assert!(generate_password(11, true).is_err());
        assert!(generate_password(65, true).is_err());
        let first = generate_password(20, true).unwrap();
        let second = generate_password(20, true).unwrap();
        assert_eq!(first.len(), 20);
        assert_ne!(first.as_str(), second.as_str());
        assert!(first.chars().any(|c| c.is_ascii_uppercase()));
        assert!(first.chars().any(|c| c.is_ascii_lowercase()));
        assert!(first.chars().any(|c| c.is_ascii_digit()));
        assert!(first.chars().any(|c| "!@#$%^&*-_=+?".contains(c)));
        assert!(crate::input_units(&first).is_ok());

        let alpha = generate_password(16, false).unwrap();
        assert_eq!(alpha.len(), 16);
        assert!(alpha.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
