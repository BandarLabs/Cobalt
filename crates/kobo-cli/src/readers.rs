//! The readers this computer knows by name.
//!
//! A reader's stable identity is its serial, burned in at the factory and
//! answered by the identity script. The owner nickname is what a command
//! line says. The store keeps both beside every address the reader has
//! answered from and whether pairing finished there, so a new DHCP lease is
//! a lookup and a re-identification, never a new pairing and never a silent
//! guess at the first reader on the network.
//!
//! The file is plain `key = value` blocks separated by blank lines, in the
//! same style as the identity answers the reader itself gives, so it stays
//! readable and editable without any tooling.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One saved reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedReader {
    /// The owner's name for it, as in `--reader clara`.
    pub nickname: String,
    /// The full serial, its stable identity.
    pub serial: String,
    /// Addresses it has answered from, most recently confirmed first.
    pub addresses: Vec<String>,
    /// Whether pairing completed with this serial.
    pub paired: bool,
}

/// The whole store: every saved reader.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Store {
    /// Saved readers, in the order they were first remembered.
    pub readers: Vec<SavedReader>,
}

impl Store {
    /// Reads a store file. Unknown lines are ignored, so the file can grow
    /// fields without stranding older builds.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut store = Self::default();
        let mut reader = SavedReader::empty();
        let mut started = false;
        let flush = |store: &mut Store, reader: &mut SavedReader, started: &mut bool| {
            if *started && !reader.serial.is_empty() {
                store.readers.push(reader.clone());
            }
            *reader = SavedReader::empty();
            *started = false;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                flush(&mut store, &mut reader, &mut started);
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key == "reader" {
                flush(&mut store, &mut reader, &mut started);
                value.clone_into(&mut reader.nickname);
                started = true;
                continue;
            }
            if !started {
                continue;
            }
            match key {
                "serial" => value.clone_into(&mut reader.serial),
                "address" => {
                    if !value.is_empty() && !reader.addresses.iter().any(|a| a == value) {
                        reader.addresses.push(value.to_owned());
                    }
                }
                "paired" => reader.paired = value == "yes",
                _ => {}
            }
        }
        flush(&mut store, &mut reader, &mut started);
        store
    }

    /// Writes the store back out, in the same shape [`Store::parse`] reads.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for reader in &self.readers {
            let _ = writeln!(out, "reader = {}", reader.nickname);
            let _ = writeln!(out, "serial = {}", reader.serial);
            for address in &reader.addresses {
                let _ = writeln!(out, "address = {address}");
            }
            out.push_str(if reader.paired {
                "paired = yes\n"
            } else {
                "paired = no\n"
            });
            out.push('\n');
        }
        out
    }

    /// Loads the store at `path`; a missing file is an empty store.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
        }
    }

    /// Saves the store at `path`, making its folder when needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{} cannot be made: {error}", parent.display()))?;
        }
        std::fs::write(path, self.render())
            .map_err(|error| format!("{} cannot be written: {error}", path.display()))
    }

    /// The reader saved under `nickname`, when there is one.
    #[must_use]
    pub fn find(&self, nickname: &str) -> Option<&SavedReader> {
        self.readers
            .iter()
            .find(|reader| reader.nickname == nickname)
    }

    /// Remembers a sighting: this serial answered at this address. A serial
    /// already known keeps its nickname and pairing, gains the address, and
    /// drops the address that no longer answers; a new serial starts a
    /// record under `nickname` when one is offered.
    pub fn remember(&mut self, serial: &str, address: &str, nickname: Option<&str>) {
        if let Some(reader) = self
            .readers
            .iter_mut()
            .find(|reader| reader.serial == serial)
        {
            reader.addresses.retain(|a| a != address);
            reader.addresses.insert(0, address.to_owned());
            reader.addresses.truncate(4);
            return;
        }
        let Some(nickname) = nickname else {
            return;
        };
        self.readers.push(SavedReader {
            nickname: nickname.to_owned(),
            serial: serial.to_owned(),
            addresses: vec![address.to_owned()],
            paired: false,
        });
    }

    /// Marks this serial paired, remembering where it answered.
    pub fn record_pairing(&mut self, serial: &str, address: &str, nickname: Option<&str>) {
        self.remember(serial, address, nickname);
        if let Some(reader) = self
            .readers
            .iter_mut()
            .find(|reader| reader.serial == serial)
        {
            reader.paired = true;
        }
    }
}

impl SavedReader {
    fn empty() -> Self {
        Self {
            nickname: String::new(),
            serial: String::new(),
            addresses: Vec::new(),
            paired: false,
        }
    }
}

/// Where the store lives: `$KOBO_CONFIG_DIR/readers`, else
/// `~/.config/kobo/readers`, beside the stream authority.
#[must_use]
pub fn store_path() -> PathBuf {
    if let Some(value) = std::env::var_os("KOBO_CONFIG_DIR") {
        return PathBuf::from(value).join("readers");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".config")
        .join("kobo")
        .join("readers")
}

/// Resolves a saved nickname to an address that provably is that reader.
///
/// Stored addresses are tried first, but an answer is accepted only when the
/// serial behind it matches - an address is a lease, not an identity. When
/// none of them answers as this reader, the local network is swept and the
/// serial is looked up fresh, which is how a reader is found after its
/// address changed. Nothing here ever falls back to the first reader found.
///
/// `probe` reads the serial answering at one address; `sweep` reads every
/// serial answering on the local network beside its address.
pub fn resolve_saved(
    store: &mut Store,
    nickname: &str,
    mut probe: impl FnMut(&str) -> Option<String>,
    mut sweep: impl FnMut() -> Vec<(String, String)>,
) -> Result<String, String> {
    let Some(reader) = store.find(nickname) else {
        let known = store
            .readers
            .iter()
            .map(|reader| reader.nickname.as_str())
            .collect::<Vec<_>>();
        let hint = if known.is_empty() {
            "no readers are saved on this computer yet".to_owned()
        } else {
            format!("saved readers: {}", known.join(", "))
        };
        return Err(crate::console::target(format!(
            "no reader is saved as \"{nickname}\" ({hint})"
        )));
    };
    let serial = reader.serial.clone();
    for address in reader.addresses.clone() {
        if probe(&address).as_deref() == Some(serial.as_str()) {
            return Ok(address);
        }
    }
    for (address, found) in sweep() {
        if found == serial {
            store.remember(&serial, &address, None);
            return Ok(address);
        }
    }
    Err(crate::console::target(format!(
        "\"{nickname}\" (serial {}…) is not answering at its saved address or anywhere on this network",
        serial.get(..4).unwrap_or(&serial)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::parse(
            "reader = clara\nserial = N365410043013\naddress = 192.168.1.23\npaired = yes\n\n\
             reader = beckett\nserial = N418999999999\naddress = 192.168.1.40\npaired = no\n",
        )
    }

    #[test]
    fn a_store_round_trips() {
        let parsed = store();
        assert_eq!(parsed.readers.len(), 2);
        assert_eq!(parsed.find("clara").unwrap().serial, "N365410043013");
        assert!(parsed.find("clara").unwrap().paired);
        assert_eq!(Store::parse(&parsed.render()), parsed);
    }

    #[test]
    fn remembering_moves_the_fresh_address_first_and_keeps_the_record() {
        let mut store = store();
        store.remember("N365410043013", "192.168.1.99", None);
        let clara = store.find("clara").unwrap();
        assert_eq!(clara.addresses[0], "192.168.1.99");
        assert_eq!(clara.addresses.len(), 2);
        assert!(clara.paired, "a new lease must not forget the pairing");
        assert_eq!(store.readers.len(), 2, "a new lease is not a new reader");
    }

    #[test]
    fn a_new_serial_needs_a_nickname_to_be_remembered() {
        let mut store = Store::default();
        store.remember("N365410043013", "192.168.1.23", None);
        assert!(store.readers.is_empty());
        store.record_pairing("N365410043013", "192.168.1.23", Some("clara"));
        assert!(store.find("clara").unwrap().paired);
    }

    #[test]
    fn resolution_accepts_only_the_right_serial_at_a_saved_address() {
        let mut store = store();
        let wrong = resolve_saved(
            &mut store,
            "clara",
            |_| Some("N418999999999".to_owned()),
            Vec::new,
        );
        assert!(
            wrong.is_err(),
            "somebody else's reader at the old lease is not clara"
        );
        let right = resolve_saved(
            &mut store,
            "clara",
            |a| (a == "192.168.1.23").then(|| "N365410043013".to_owned()),
            Vec::new,
        );
        assert_eq!(right.unwrap(), "192.168.1.23");
    }

    #[test]
    fn resolution_finds_the_serial_after_its_address_changed() {
        let mut store = store();
        let found = resolve_saved(
            &mut store,
            "clara",
            |_| None,
            || {
                vec![
                    ("192.168.1.40".to_owned(), "N418999999999".to_owned()),
                    ("192.168.1.71".to_owned(), "N365410043013".to_owned()),
                ]
            },
        );
        assert_eq!(found.unwrap(), "192.168.1.71");
        assert_eq!(store.find("clara").unwrap().addresses[0], "192.168.1.71");
    }

    #[test]
    fn an_unknown_name_is_a_target_error_that_names_the_known_readers() {
        let mut store = store();
        let error = resolve_saved(&mut store, "nobody", |_| None, Vec::new).unwrap_err();
        assert!(error.starts_with("target: "), "{error}");
        assert!(error.contains("clara, beckett"), "{error}");
    }

    #[test]
    fn a_missing_reader_is_a_target_error_not_a_guess() {
        let mut store = store();
        let error = resolve_saved(&mut store, "clara", |_| None, Vec::new).unwrap_err();
        assert!(error.starts_with("target: "), "{error}");
        assert!(error.contains("not answering"), "{error}");
    }
}
