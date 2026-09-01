//! Deterministic inputs for the kobod-owned Syncthing supervisor.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cadence {
    Manual,
    Hourly,
    FourHourly,
    Daily,
}
impl Cadence {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual only",
            Self::Hourly => "Hourly",
            Self::FourHourly => "Every 4 hours",
            Self::Daily => "Daily",
        }
    }
    pub const fn radio_minutes(self) -> u8 {
        match self {
            Self::Manual => 0,
            Self::Hourly => 120,
            Self::FourHourly => 30,
            Self::Daily => 5,
        }
    }
    pub const fn next(self) -> Self {
        match self {
            Self::Manual => Self::Hourly,
            Self::Hourly => Self::FourHourly,
            Self::FourHourly => Self::Daily,
            Self::Daily => Self::Manual,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub device_id: String,
    pub cadence: Cadence,
    pub enabled: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            device_id: "KOBOSYNC-DEVICE-ID".into(),
            cadence: Cadence::Manual,
            enabled: false,
        }
    }
}

pub const FOLDERS: [(&str, &str); 4] = [
    ("sync/vault", "Receive only"),
    ("sync/frame", "Receive only"),
    ("sync/books", "Receive only"),
    ("sync/out", "Send only"),
];

/// The config fragment kobod gives the unmodified binary. Its API key is
/// generated daemon-side and deliberately absent from this app-owned model.
#[cfg(test)]
pub fn generated_config(config: &Config) -> String {
    format!("<configuration version=\"37\"><options><globalAnnounceEnabled>true</globalAnnounceEnabled><relaysEnabled>true</relaysEnabled><natEnabled>false</natEnabled></options><gui enabled=\"true\" tls=\"false\" address=\"127.0.0.1:8384\"/><device id=\"{}\"/></configuration>", config.device_id)
}

#[cfg(test)]
pub fn should_open_window(config: &Config, event: &str) -> bool {
    config.enabled && matches!(event, "settings-open" | "network-tail" | "scheduled")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_config_binds_the_rest_api_to_loopback() {
        let config = generated_config(&Config::default());
        assert!(config.contains("127.0.0.1:8384"));
        assert!(config.contains("<natEnabled>false</natEnabled>"));
        assert!(!config.to_lowercase().contains("api_key"));
    }
    #[test]
    fn disabled_sync_never_opens_a_window() {
        let config = Config::default();
        assert!(!should_open_window(&config, "scheduled"));
        assert_eq!(Cadence::Hourly.radio_minutes(), 120);
    }
}
