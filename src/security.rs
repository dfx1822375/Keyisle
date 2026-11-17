use crate::FillMode;
use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

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
    Delete(i64),
    Fill(i64, FillMode),
}
impl ProtectedAction {
    pub fn record(self) -> i64 {
        match self {
            Self::View(id) | Self::Edit(id) | Self::Delete(id) | Self::Fill(id, _) => id,
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
