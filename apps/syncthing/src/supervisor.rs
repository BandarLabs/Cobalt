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
            Self::Manual => "No schedule",
            Self::Hourly => "Every hour",
            Self::FourHourly => "Every 4 hours",
            Self::Daily => "Once a day",
        }
    }
    pub const fn seconds(self) -> Option<u32> {
        match self {
            Self::Manual => None,
            Self::Hourly => Some(3_600),
            Self::FourHourly => Some(14_400),
            Self::Daily => Some(86_400),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub cadence: Cadence,
    pub enabled: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            cadence: Cadence::Manual,
            enabled: false,
        }
    }
}

/// The four fixed folders and which way their files go, in the owner's words
/// rather than Syncthing's folder types.
pub const FOLDERS: [(&str, &str); 4] = [
    ("sync/vault", "Notes, from the computer to Vault"),
    ("sync/frame", "Photos, from the computer to Frame"),
    ("sync/books", "Books, from the computer"),
    ("sync/out", "Files, from the reader to the computer"),
];

/// The config fragment kobod gives the unmodified binary. Its API key is
/// generated daemon-side and deliberately absent from this app-owned model.
#[cfg(test)]
pub fn generated_config(config: &Config) -> String {
    let enabled = config.enabled;
    format!("<configuration version=\"37\"><options><globalAnnounceEnabled>true</globalAnnounceEnabled><relaysEnabled>true</relaysEnabled><natEnabled>false</natEnabled></options><gui enabled=\"{enabled}\" tls=\"false\" address=\"127.0.0.1:8384\"/></configuration>")
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
    fn sync_starts_off_with_no_schedule() {
        let config = Config::default();
        assert!(!config.enabled);
        assert_eq!(config.cadence.seconds(), None);
    }
}
