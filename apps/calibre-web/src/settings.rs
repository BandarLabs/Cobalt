//! Preserve the previous account until a checked replacement is acknowledged.
use kobo_sdk::{Context, StoreResult};
#[derive(Default, PartialEq)]
pub enum State {
    #[default]
    Loading,
    Ready,
    Blocked,
    Saving,
    Failed,
}
#[derive(Default)]
pub struct Settings {
    pub state: State,
    candidate: Option<Vec<u8>>,
    writing: Option<Vec<u8>>,
}
pub fn decode(bytes: &[u8]) -> Option<(String, Option<String>)> {
    if bytes.len() > 2200 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let (url, account) = text.split_once('|').unwrap_or((text, ""));
    let url = super::catalog::endpoint(url)?;
    let account = account_name(account).ok()?;
    Some((url, account))
}
pub fn account_name(text: &str) -> Result<Option<String>, ()> {
    let text = text.trim();
    if text.len() > 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(());
    }
    Ok((!text.is_empty()).then(|| text.to_owned()))
}
impl Settings {
    pub fn can_edit(&self) -> bool {
        matches!(self.state, State::Ready | State::Failed)
    }
    pub fn prepare(&mut self, url: &str, account: Option<&str>) -> bool {
        if !self.can_edit() {
            return false;
        }
        let bytes = format!("{}|{}", url, account.unwrap_or_default()).into_bytes();
        if decode(&bytes).is_none() {
            return false;
        }
        self.candidate = Some(bytes);
        self.state = State::Ready;
        true
    }
    pub fn verified(&mut self, c: &mut Context) {
        if self.state != State::Ready || self.writing.is_some() {
            return;
        }
        if let Some(bytes) = &self.candidate {
            c.store().save(super::REGISTRY, bytes.clone());
            self.writing = Some(bytes.clone());
            self.state = State::Saving;
        }
    }
    pub fn finish(&mut self, result: &StoreResult) {
        if self.writing.is_none() {
            return;
        }
        match result {
            StoreResult::Saved { key } if key == super::REGISTRY => {
                self.writing = None;
                self.candidate = None;
                self.state = State::Ready;
            }
            StoreResult::Denied(_) => {
                self.writing = None;
                self.state = State::Failed;
            }
            _ => {}
        }
    }
    pub fn retry(&mut self, c: &mut Context) {
        if self.state == State::Failed {
            self.state = State::Ready;
            self.verified(c);
        }
    }
    pub fn cancel_check(&mut self) {
        if self.state == State::Ready {
            self.candidate = None;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_registry_is_validated_without_accepting_passwords_or_extra_fields() {
        assert_eq!(
            decode(b"https://library.test|calibre").unwrap().0,
            "https://library.test/opds"
        );
        for bytes in [
            b"https://u:p@library|account".as_slice(),
            b"https://library|a|b",
            b"",
            b"https://library|user:password",
        ] {
            assert!(decode(bytes).is_none());
        }
        assert_eq!(account_name(" "), Ok(None));
    }
    #[test]
    fn verification_and_exact_ack_are_required_before_settings_are_saved() {
        let mut settings = Settings {
            state: State::Ready,
            ..Settings::default()
        };
        let mut c = Context::default();
        assert!(settings.prepare("https://library.test/opds", Some("calibre")));
        assert!(c.commands().is_empty());
        settings.verified(&mut c);
        assert!(settings.state == State::Saving);
        assert!(!settings.prepare("https://other.test/opds", None));
        settings.finish(&StoreResult::Saved {
            key: "another-record".into(),
        });
        assert!(settings.state == State::Saving);
        settings.finish(&StoreResult::Denied(kobo_sdk::StoreError::TooFull));
        assert!(settings.state == State::Failed);
        settings.retry(&mut c);
        settings.finish(&StoreResult::Saved {
            key: super::super::REGISTRY.into(),
        });
        assert!(settings.state == State::Ready);
        assert!(settings.candidate.is_none());
    }
}
