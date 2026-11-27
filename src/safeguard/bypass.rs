use crate::state::{State, storage::DurabilityUncertain};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

const FILE: &str = "bypass.json";
const MAX_UNIX: f64 = 253_402_300_799.0; // 9999-12-31T23:59:59Z

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BypassState {
    Off,
    Timed { expires_at_unix: f64, owner_id: i64 },
    Indefinite { owner_id: i64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BypassPreset {
    OneHour,
    SixHours,
    OneDay,
    SevenDays,
    ThirtyDays,
    Indefinite,
    Custom,
    Off,
    Duration(u64),
}

impl BypassPreset {
    pub const MENU: [(&'static str, Self); 8] = [
        ("1h", Self::OneHour),
        ("6h", Self::SixHours),
        ("1d", Self::OneDay),
        ("7d", Self::SevenDays),
        ("30d", Self::ThirtyDays),
        ("inf", Self::Indefinite),
        ("custom", Self::Custom),
        ("off", Self::Off),
    ];

    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        ensure!(raw.len() <= 64, "Invalid bypass duration");
        let input = raw.to_ascii_lowercase();
        if let Some((_, preset)) = Self::MENU.iter().find(|(key, _)| *key == input) {
            return Ok(*preset);
        }
        if input == "forever" {
            return Ok(Self::Indefinite);
        }
        let mut rest = input.as_str();
        let mut seconds = 0_u64;
        let mut previous_unit = u64::MAX;
        while !rest.is_empty() {
            let end = rest.bytes().take_while(u8::is_ascii_digit).count();
            let (digits, suffix) = rest.split_at(end);
            let amount: u64 = digits
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid bypass duration"))?;
            let unit = match suffix.as_bytes().first() {
                Some(b'd') => 86_400,
                Some(b'h') => 3_600,
                Some(b'm') => 60,
                _ => bail!("Invalid bypass duration"),
            };
            ensure!(unit < previous_unit, "Invalid bypass duration");
            seconds = amount
                .checked_mul(unit)
                .and_then(|part| seconds.checked_add(part))
                .filter(|total| *total <= MAX_UNIX as u64)
                .ok_or_else(|| anyhow::anyhow!("Bypass duration is too large"))?;
            previous_unit = unit;
            rest = &suffix[1..];
        }
        ensure!(seconds > 0, "Bypass duration must be positive");
        Ok(Self::Duration(seconds))
    }

    pub fn seconds(self) -> Option<u64> {
        match self {
            Self::OneHour => Some(3_600),
            Self::SixHours => Some(21_600),
            Self::OneDay => Some(86_400),
            Self::SevenDays => Some(604_800),
            Self::ThirtyDays => Some(2_592_000),
            Self::Duration(seconds) => Some(seconds),
            Self::Off | Self::Indefinite | Self::Custom => None,
        }
    }

    /// `custom` only opens the instructions; it does not change state.
    pub fn state(self, now: f64, owner_id: i64) -> Result<Option<BypassState>> {
        ensure!(owner_id > 0, "Invalid bypass owner");
        let state = match self {
            Self::Off => BypassState::Off,
            Self::Indefinite => BypassState::Indefinite { owner_id },
            Self::Custom => return Ok(None),
            timed => {
                let seconds = timed
                    .seconds()
                    .filter(|&x| x > 0 && x <= MAX_UNIX as u64)
                    .ok_or_else(|| anyhow::anyhow!("Invalid bypass duration"))?;
                let expires_at_unix = now + seconds as f64;
                ensure!(
                    valid_clock(now) && valid_clock(expires_at_unix),
                    "Invalid bypass expiry"
                );
                BypassState::Timed {
                    expires_at_unix,
                    owner_id,
                }
            }
        };
        Ok(Some(state))
    }
}

fn valid_clock(value: f64) -> bool {
    (0.0..=MAX_UNIX).contains(&value)
}

impl BypassState {
    fn decode(record: &Value) -> Self {
        match (
            record.get("on"),
            record.get("by").and_then(Value::as_i64),
            record.get("until"),
        ) {
            (Some(Value::Bool(true)), Some(owner_id), Some(until)) if owner_id > 0 => {
                if until.is_null() {
                    Self::Indefinite { owner_id }
                } else if let Some(expires_at_unix) =
                    until.as_f64().filter(|&x| x > 0.0 && valid_clock(x))
                {
                    Self::Timed {
                        expires_at_unix,
                        owner_id,
                    }
                } else {
                    Self::Off
                }
            }
            _ => Self::Off,
        }
    }

    /// Return the effective state at the supplied UTC clock reading.
    pub fn read(state: &State, now: f64) -> Self {
        state
            .read_json(FILE, json!({"on": false}))
            .map(|value| Self::decode(&value).effective(now))
            .unwrap_or(Self::Off)
    }

    /// Call only after verifying the owner and their approval bot update.
    pub fn write(self, state: &State) -> Result<()> {
        let record = match self {
            Self::Off => json!({"on": false}),
            Self::Indefinite { owner_id } if owner_id > 0 => {
                json!({"on": true, "until": null, "by": owner_id})
            }
            Self::Timed {
                expires_at_unix,
                owner_id,
            } if owner_id > 0 && expires_at_unix > 0.0 && valid_clock(expires_at_unix) => {
                json!({"on": true, "until": expires_at_unix, "by": owner_id})
            }
            _ => bail!("Invalid bypass state"),
        };
        let _lock = state.state_lock()?;
        let previous = state.read_json_unlocked(FILE, json!({"on": false}))?;
        if let Err(error) = state.write_json_unlocked(FILE, &record) {
            if error.downcast_ref::<DurabilityUncertain>().is_some() {
                state.write_json_unlocked(FILE, &previous)?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn effective(self, now: f64) -> Self {
        if !valid_clock(now) {
            return Self::Off;
        }
        match self {
            Self::Timed {
                expires_at_unix,
                owner_id,
            } if owner_id > 0 && valid_clock(expires_at_unix) && expires_at_unix > now => self,
            Self::Indefinite { owner_id } if owner_id > 0 => self,
            _ => Self::Off,
        }
    }

    pub fn is_active(self, now: f64) -> bool {
        self.effective(now) != Self::Off
    }

    pub fn describe(self, now: f64) -> String {
        match self.effective(now) {
            Self::Off => "off".to_owned(),
            Self::Indefinite { .. } => "ON until you turn it off".to_owned(),
            Self::Timed {
                expires_at_unix, ..
            } => {
                format!("ON for {} more", span((expires_at_unix - now) as u64))
            }
        }
    }
}

/// Remaining time as shown to owners, e.g. `88 h 17 min` or `30 d 0 h 0 min`.
pub fn span(left: u64) -> String {
    // Preserve the legacy hours-and-minutes format through the 7-day preset.
    if left >= 8 * 86_400 {
        format!(
            "{} d {} h {} min",
            left / 86_400,
            left % 86_400 / 3_600,
            left % 3_600 / 60
        )
    } else {
        format!("{} h {} min", left / 3_600, left % 3_600 / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_preset_and_custom_duration() -> Result<()> {
        for (key, seconds) in [
            ("1h", 3600),
            ("6h", 21600),
            ("1d", 86400),
            ("7d", 604800),
            ("30d", 2592000),
            ("1d12h", 129600),
            ("2h30m", 9000),
            ("90m", 5400),
        ] {
            assert_eq!(BypassPreset::parse(key)?.seconds(), Some(seconds), "{key}");
        }
        for key in ["inf", "forever"] {
            assert_eq!(BypassPreset::parse(key)?, BypassPreset::Indefinite);
        }
        assert_eq!(BypassPreset::parse("off")?, BypassPreset::Off);
        assert_eq!(BypassPreset::parse("custom")?.state(10.0, 7)?, None);
        Ok(())
    }

    #[test]
    fn rejects_invalid_duration_and_unrepresentable_expiry() {
        for bad in [
            "",
            "-1h",
            "0m",
            "0d0h",
            "1 h",
            "1h1h",
            "1m1h",
            "1.5h",
            "1s",
            "2h30",
            "18446744073709551615d",
            "99999999d",
            "1hé",
        ] {
            assert!(BypassPreset::parse(bad).is_err(), "{bad}");
        }
        assert!(BypassPreset::OneHour.state(MAX_UNIX, 7).is_err());
        assert!(BypassPreset::OneHour.state(f64::NAN, 7).is_err());
        assert!(BypassPreset::Indefinite.state(1.0, 0).is_err());
    }

    #[test]
    fn legacy_state_expiry_and_description() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = State::at(root.path().to_owned(), vec![])?;
        let mode = BypassPreset::ThirtyDays.state(10.0, 7)?.unwrap();
        mode.write(&state)?;
        assert_eq!(
            state.read_json(FILE, Value::Null)?,
            json!({"on":true,"until":2592010.0,"by":7})
        );
        assert_eq!(
            BypassState::read(&state, 10.0).describe(10.0),
            "ON for 30 d 0 h 0 min more"
        );
        assert!(BypassState::read(&state, 2_592_009.5).is_active(2_592_009.5));
        assert_eq!(BypassState::read(&state, 2_592_010.0), BypassState::Off);
        assert_eq!(mode.describe(2_588_349.0), "ON for 1 h 1 min more");
        state.write_json(FILE, &json!({"on":true,"until":null,"by":7}))?;
        assert_eq!(
            BypassState::read(&state, 20.0).describe(20.0),
            "ON until you turn it off"
        );
        BypassState::Off.write(&state)?;
        assert_eq!(state.read_json(FILE, Value::Null)?, json!({"on":false}));
        Ok(())
    }

    #[test]
    fn corrupt_or_invalid_records_read_off() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = State::at(root.path().to_owned(), vec![])?;
        for invalid in [
            json!({"on":true,"by":7}),
            json!({"on":true,"until":null}),
            json!({"on":true,"until":1.0,"by":-1}),
            json!({"on":true,"until":"10","by":7}),
            json!({"on":1,"until":null,"by":7}),
            json!({"on":true,"until":0,"by":7}),
        ] {
            state.write_json(FILE, &invalid)?;
            assert_eq!(BypassState::read(&state, 0.0), BypassState::Off);
        }
        std::fs::write(state.root().join(FILE), b"broken")?;
        assert_eq!(BypassState::read(&state, 0.0), BypassState::Off);
        Ok(())
    }

    #[test]
    fn rejected_write_preserves_the_prior_setting() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = State::at(root.path().to_owned(), vec![])?;
        let prior = BypassState::Indefinite { owner_id: 42 };
        prior.write(&state)?;
        std::fs::create_dir(state.root().join("account-rename-transaction.json"))?;
        assert!(BypassState::Off.write(&state).is_err());
        assert_eq!(BypassState::read(&state, 8.0), prior);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn failed_directory_sync_restores_the_prior_setting() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = State::at(root.path().to_owned(), vec![])?;
        let prior = BypassState::Indefinite { owner_id: 42 };
        prior.write(&state)?;
        state.fail_next_directory_sync_for_test();
        assert!(BypassState::Off.write(&state).is_err());
        assert_eq!(BypassState::read(&state, 8.0), prior);
        Ok(())
    }
}
