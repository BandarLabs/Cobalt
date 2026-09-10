//! Local signed packages use the runtime's trust and transaction implementation.
//! The explicit fixture key is never installed as a device trust key.
use crate::Scenario;
use kobo_app_store::Ed25519PublicKey;
use kobo_protocol::{DeviceError, DeviceRequest, DeviceResult, UpdateChannel};
use kobod::app_store::{self as runtime, AppWriteFault};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(super) struct SignedStore {
    root: PathBuf,
    transport: PathBuf,
    key: Ed25519PublicKey,
}

impl SignedStore {
    pub fn open(directory: &Path) -> io::Result<Self> {
        crate::validate_socket_parent(&directory.join("fixture"))?;
        if read(&directory.join("format"), 64)? != b"cobalt.simulator-app-store.v1\n" {
            return Err(io::Error::other("unsupported signed Store fixture"));
        }
        let text = read(&directory.join("key.hex"), 65)?;
        let key = std::str::from_utf8(&text)
            .ok()
            .and_then(|text| Ed25519PublicKey::from_hex(text.trim_end()).ok())
            .filter(|key| key.to_hex() != kobo_app_store::PUBLIC_RELEASE_KEY_HEX)
            .ok_or_else(|| io::Error::other("Store fixture needs its own test public key"))?;
        let root = directory.join("installed");
        let transport = directory.join("transport");
        crate::validate_socket_parent(&root.join("fixture"))?;
        crate::validate_socket_parent(&transport.join("fixture"))?;
        Ok(Self {
            root,
            transport,
            key,
        })
    }

    pub fn metadata(&self) -> kobo_json::Value {
        kobo_json::ObjectBuilder::new()
            .set("mode", "signed-local")
            .set("key", self.key.to_hex())
            .set("channel", "beta")
            .set("deviceTrust", false)
            .build()
    }

    pub fn request(&self, request: &DeviceRequest, scenario: Scenario) -> DeviceResult {
        let fault = (scenario == Scenario::StorageFull).then_some(AppWriteFault::NoRoom);
        let fetch = |url: &str, maximum: u32| {
            match scenario {
                Scenario::Offline | Scenario::HostDown => return Err(DeviceError::Unreachable),
                Scenario::NetworkTimeout => return Err(DeviceError::TimedOut),
                _ => {}
            }
            // Hashing the complete requested URL makes every filename pathless.
            // No URL in a signed catalog can initiate an actual network request.
            let path = self.transport.join(format!(
                "{}.blob",
                kobo_net::sha256::hex_digest(url.as_bytes())
            ));
            read(&path, u64::from(maximum)).map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => DeviceError::NotFound,
                io::ErrorKind::InvalidData => DeviceError::Integrity,
                _ => DeviceError::Backend,
            })
        };
        let channel = UpdateChannel::Beta;
        let entries = match request {
            DeviceRequest::ListInstalledApps => runtime::installed_using(&self.root, &self.key),
            DeviceRequest::ReadAppCatalog => runtime::catalog_using(&self.root, channel, &self.key),
            DeviceRequest::RefreshAppCatalog => {
                runtime::refresh_using_fault(&self.root, channel, &self.key, fetch, fault)
            }
            DeviceRequest::InstallApp { id } => {
                return done(runtime::install_using_fault(
                    &self.root, id, channel, &self.key, fetch, fault,
                ))
            }
            DeviceRequest::UninstallApp { id } => {
                return done(runtime::uninstall_using(&self.root, id, &self.key))
            }
            _ => return DeviceResult::Failed(DeviceError::InvalidInput),
        };
        entries.map_or_else(DeviceResult::Failed, |entries| DeviceResult::Apps {
            entries,
        })
    }
}

fn done(result: Result<(), DeviceError>) -> DeviceResult {
    result.map_or_else(DeviceResult::Failed, |()| DeviceResult::Done)
}

fn read(path: &Path, maximum: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Store fixture file",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized Store fixture file",
        ));
    }
    Ok(bytes)
}
