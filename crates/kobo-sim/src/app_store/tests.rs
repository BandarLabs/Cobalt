use super::*;
use crate::{AppState, SimulatedApps};
use kobo_app_store::{
    build_bundle, derive_public_key, sign, Catalog, CatalogEntry, CatalogEntryInput, Manifest,
    ManifestInput,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = crate::tests::private_temp_dir();
        for name in ["installed", "transport"] {
            fs::create_dir(root.join(name)).unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(root.join("format"), b"cobalt.simulator-app-store.v1\n").unwrap();
        fs::write(
            root.join("key.hex"),
            derive_public_key(&[71; 32]).unwrap().to_hex(),
        )
        .unwrap();
        Self(root)
    }
    fn blob(&self, url: &str) -> PathBuf {
        self.0.join("transport").join(format!(
            "{}.blob",
            kobo_net::sha256::hex_digest(url.as_bytes())
        ))
    }
    fn release(&self, version: &str, minimum: &str, signer: &[u8; 32]) -> Vec<u8> {
        let binary = format!("Original inert test payload {version}").into_bytes();
        let manifest = Manifest::new_public(ManifestInput {
            id: "quality-fixture".into(),
            display_name: "Quality fixture".into(),
            short_label: "Fixture".into(),
            summary: "Original local transaction fixture".into(),
            version: version.into(),
            minimum_cobalt_version: minimum.into(),
            glyph: "note".into(),
            capabilities: vec![],
            binary_sha256: kobo_net::sha256::hex_digest(&binary),
            binary_bytes: binary.len() as u64,
        })
        .unwrap();
        let package = build_bundle(&manifest, &binary, signer).unwrap();
        let catalog = Catalog::new(vec![CatalogEntry::new(CatalogEntryInput {
            manifest,
            package_url: "https://fixture.invalid/app.cobalt-app".into(),
            package_sha256: kobo_net::sha256::hex_digest(&package),
            package_bytes: package.len() as u64,
        })
        .unwrap()])
        .unwrap()
        .to_canonical_bytes();
        fs::write(self.blob(runtime::BETA_CATALOG_URL), &catalog).unwrap();
        fs::write(
            self.blob(runtime::BETA_CATALOG_SIGNATURE_URL),
            sign(&catalog, &[71; 32]).unwrap().to_hex(),
        )
        .unwrap();
        fs::write(self.blob("https://fixture.invalid/app.cobalt-app"), package).unwrap();
        binary
    }
    fn state(&self) -> Arc<Mutex<AppState>> {
        Arc::new(Mutex::new(AppState::with_apps(Arc::new(Mutex::new(
            SimulatedApps {
                catalog: vec![],
                signed: Some(SignedStore::open(&self.0).unwrap()),
            },
        )))))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn request(
    state: &Arc<Mutex<AppState>>,
    request: &DeviceRequest,
    scenario: Scenario,
) -> DeviceResult {
    crate::simulated_app_request(state, "store", scenario, request)
        .unwrap()
        .unwrap()
}
fn install() -> DeviceRequest {
    DeviceRequest::InstallApp {
        id: "quality-fixture".into(),
    }
}
fn entries(state: &Arc<Mutex<AppState>>) -> Vec<kobo_protocol::AppInfo> {
    let DeviceResult::Apps { entries } =
        request(state, &DeviceRequest::ReadAppCatalog, Scenario::Normal)
    else {
        panic!("catalog")
    };
    entries
}

#[test]
fn real_transactions_install_update_reopen_remove_and_preserve_owner_data() {
    let fixture = Fixture::new();
    let initial = fixture.release("1.0.0", env!("CARGO_PKG_VERSION"), &[71; 32]);
    let state = fixture.state();
    assert!(matches!(
        request(&state, &DeviceRequest::RefreshAppCatalog, Scenario::Normal),
        DeviceResult::Apps { .. }
    ));
    assert_eq!(
        request(&state, &install(), Scenario::Normal),
        DeviceResult::Done
    );
    let binary = fixture
        .0
        .join("installed/apps/quality-fixture/bin/kobo-quality-fixture");
    assert_eq!(fs::read(&binary).unwrap(), initial);
    assert_eq!(
        entries(&state)[0].installed_version.as_deref(),
        Some("1.0.0")
    );
    let data = fixture.0.join("installed/data/quality-fixture/notes");
    fs::create_dir_all(data.parent().unwrap()).unwrap();
    fs::write(&data, b"Owner's notes").unwrap();
    let newer = fixture.release("1.1.0", env!("CARGO_PKG_VERSION"), &[71; 32]);
    request(&state, &DeviceRequest::RefreshAppCatalog, Scenario::Normal);
    assert!(entries(&state)[0].has_update());
    assert_eq!(
        request(&state, &install(), Scenario::StorageFull),
        DeviceResult::Failed(DeviceError::Backend)
    );
    assert_eq!(fs::read(&binary).unwrap(), initial);
    assert_eq!(
        request(&state, &install(), Scenario::Normal),
        DeviceResult::Done
    );
    assert_eq!(fs::read(&binary).unwrap(), newer);
    drop(state);
    let state = fixture.state();
    assert_eq!(
        entries(&state)[0].installed_version.as_deref(),
        Some("1.1.0")
    );
    assert_eq!(
        request(
            &state,
            &DeviceRequest::UninstallApp {
                id: "quality-fixture".into()
            },
            Scenario::Offline
        ),
        DeviceResult::Done
    );
    assert!(!binary.exists());
    assert_eq!(fs::read(&data).unwrap(), b"Owner's notes");
    assert_eq!(entries(&state)[0].installed_version, None);
    assert_eq!(
        request(&state, &install(), Scenario::Normal),
        DeviceResult::Done
    );
}

#[test]
fn bad_packages_and_newer_runtime_requirements_cannot_replace_an_install() {
    let fixture = Fixture::new();
    let original = fixture.release("1.0.0", env!("CARGO_PKG_VERSION"), &[71; 32]);
    let state = fixture.state();
    request(&state, &DeviceRequest::RefreshAppCatalog, Scenario::Normal);
    assert_eq!(
        request(&state, &install(), Scenario::Normal),
        DeviceResult::Done
    );
    let binary = fixture
        .0
        .join("installed/apps/quality-fixture/bin/kobo-quality-fixture");
    fixture.release("1.1.0", env!("CARGO_PKG_VERSION"), &[72; 32]);
    request(&state, &DeviceRequest::RefreshAppCatalog, Scenario::Normal);
    // Catalog hash matches, but the package itself has the wrong signer.
    assert_eq!(
        request(&state, &install(), Scenario::StorageFull),
        DeviceResult::Failed(DeviceError::Integrity)
    );
    fixture.release("1.2.0", "999.0.0", &[71; 32]);
    request(&state, &DeviceRequest::RefreshAppCatalog, Scenario::Normal);
    assert_eq!(
        request(&state, &install(), Scenario::Offline),
        DeviceResult::Failed(DeviceError::InvalidInput)
    );
    fixture.release("1.3.0", env!("CARGO_PKG_VERSION"), &[71; 32]);
    request(&state, &DeviceRequest::RefreshAppCatalog, Scenario::Normal);
    let package = fixture.blob("https://fixture.invalid/app.cobalt-app");
    let mut bytes = fs::read(&package).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&package, bytes).unwrap();
    assert_eq!(
        request(&state, &install(), Scenario::Normal),
        DeviceResult::Failed(DeviceError::Integrity)
    );
    assert_eq!(fs::read(&binary).unwrap(), original);
    assert_eq!(
        entries(&state)[0].installed_version.as_deref(),
        Some("1.0.0")
    );
}

#[test]
fn authority_and_validation_precede_faults_and_failed_refresh_keeps_cache() {
    let fixture = Fixture::new();
    fixture.release("1.0.0", env!("CARGO_PKG_VERSION"), &[71; 32]);
    let state = fixture.state();
    assert!(matches!(
        crate::simulated_app_request(&state, "todo", Scenario::Normal, &install()).unwrap(),
        Some(DeviceResult::Denied(_))
    ));
    request(&state, &DeviceRequest::RefreshAppCatalog, Scenario::Normal);
    for (scenario, error) in [
        (Scenario::Offline, DeviceError::Unreachable),
        (Scenario::NetworkTimeout, DeviceError::TimedOut),
    ] {
        assert_eq!(
            request(&state, &install(), scenario),
            DeviceResult::Failed(error)
        );
        assert_eq!(
            request(
                &state,
                &DeviceRequest::InstallApp {
                    id: "../escape".into()
                },
                scenario
            ),
            DeviceResult::Failed(DeviceError::InvalidInput)
        );
    }
    fixture.release("1.1.0", env!("CARGO_PKG_VERSION"), &[71; 32]);
    assert_eq!(
        request(
            &state,
            &DeviceRequest::RefreshAppCatalog,
            Scenario::StorageFull
        ),
        DeviceResult::Failed(DeviceError::Backend)
    );
    assert_eq!(entries(&state)[0].version, "1.0.0");
    fs::write(
        fixture.blob(runtime::BETA_CATALOG_SIGNATURE_URL),
        "0".repeat(128),
    )
    .unwrap();
    assert_eq!(
        request(
            &state,
            &DeviceRequest::RefreshAppCatalog,
            Scenario::StorageFull
        ),
        DeviceResult::Failed(DeviceError::Integrity)
    );
    assert_eq!(entries(&state)[0].version, "1.0.0");
    assert_eq!(
        request(
            &state,
            &DeviceRequest::UninstallApp {
                id: "settings".into()
            },
            Scenario::Normal
        ),
        DeviceResult::Failed(DeviceError::InvalidInput)
    );
}

#[test]
fn fixture_configuration_refuses_release_keys_oversized_files_and_links() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("key.hex"),
        kobo_app_store::PUBLIC_RELEASE_KEY_HEX,
    )
    .unwrap();
    assert!(SignedStore::open(&fixture.0).is_err());
    fs::write(fixture.0.join("key.hex"), "a".repeat(66)).unwrap();
    assert!(SignedStore::open(&fixture.0).is_err());
    fs::remove_file(fixture.0.join("key.hex")).unwrap();
    std::os::unix::fs::symlink("format", fixture.0.join("key.hex")).unwrap();
    assert!(SignedStore::open(&fixture.0).is_err());
}
