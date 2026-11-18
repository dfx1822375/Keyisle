#[cfg(windows)]
extern crate password_manager_sqlcipher;
pub mod maintenance;
pub mod search;
pub mod security;
pub mod session;
pub mod settings;
pub mod shortcut;
pub mod startup;
pub mod update;
pub mod vault;

pub fn input_units(text: &str) -> Result<Vec<u16>, &'static str> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err("字段为空或包含控制字符，已取消填充。");
    }
    let mut units: Vec<_> = text.encode_utf16().collect();
    if units.len() > 1024 {
        zeroize::Zeroize::zeroize(&mut units);
        return Err("字段过长，已取消填充。");
    }
    Ok(units)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMode {
    Password,
    Username,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputUnit {
    Unicode(u16),
    Tab,
}

impl zeroize::Zeroize for InputUnit {
    fn zeroize(&mut self) {
        *self = Self::Unicode(0);
    }
}

pub fn fill_plan(
    username: &str,
    password: &str,
    mode: FillMode,
) -> Result<Vec<InputUnit>, &'static str> {
    let text = |value: &str| {
        input_units(value).map(|units| {
            let units = zeroize::Zeroizing::new(units);
            units
                .iter()
                .copied()
                .map(InputUnit::Unicode)
                .collect::<Vec<_>>()
        })
    };
    match mode {
        FillMode::Password => text(password),
        FillMode::Username => text(username),
        FillMode::Both => {
            let mut plan = text(username)?;
            let password = match text(password) {
                Ok(value) => zeroize::Zeroizing::new(value),
                Err(error) => {
                    zeroize::Zeroize::zeroize(&mut plan);
                    return Err(error);
                }
            };
            plan.push(InputUnit::Tab);
            plan.extend(password.iter().copied());
            Ok(plan)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_modes_keep_fields_separate_and_validate_before_sending() {
        assert_eq!(
            fill_plan("user", "密码", FillMode::Password).unwrap(),
            vec![
                InputUnit::Unicode('密' as u16),
                InputUnit::Unicode('码' as u16)
            ]
        );
        assert_eq!(
            fill_plan("u", "p", FillMode::Username).unwrap(),
            vec![InputUnit::Unicode('u' as u16)]
        );
        assert_eq!(
            fill_plan("u", "p", FillMode::Both).unwrap(),
            vec![
                InputUnit::Unicode('u' as u16),
                InputUnit::Tab,
                InputUnit::Unicode('p' as u16)
            ]
        );
        assert!(fill_plan("user", "bad\n", FillMode::Both).is_err());
        assert!(fill_plan("bad\t", "pass", FillMode::Both).is_err());
    }

    #[test]
    fn input_preserves_unicode_and_rejects_action_characters() {
        let units = input_units("中文€🔒").unwrap();
        assert_eq!(String::from_utf16(&units).unwrap(), "中文€🔒");
        assert_eq!(units.len(), 5);
        for text in ["", "abc\n", "abc\r", "abc\t", "abc\0"] {
            assert!(input_units(text).is_err());
        }
        assert!(input_units(&"x".repeat(1025)).is_err());
    }
}
