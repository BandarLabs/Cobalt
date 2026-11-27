//! Apps a catalog publication keeps exactly as they were published.
//!
//! A change to the platform under the Store apps used to rebuild every one of
//! them, and a rebuilt binary differs from the published one even when the
//! app's own code did not move, if only because a panic message records the
//! line it came from. So every app had to publish a new version for a change
//! that was not its own. The runtime already reads every protocol back to the
//! oldest one a published app speaks, the way a phone runs an app built for an
//! older system, so the published build keeps working on the new platform.
//!
//! An app whose own code and listing are unchanged is therefore kept: its
//! published package goes back into the new catalog byte for byte, under the
//! same version and the same minimum Cobalt version it was published with. A
//! reader that has it is offered nothing, and a reader on an older runtime
//! keeps being offered what it could install before. The app is rebuilt, from
//! the platform as it then is, when it next publishes a version of its own.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use super::ReleaseApp;

/// A published package this publication keeps, with the catalog entry that
/// points readers at it.
pub(crate) struct KeptApp {
    pub(crate) package: String,
    pub(crate) entry: kobo_app_store::CatalogEntry,
}

/// Reads the published packages in `directory` and pairs each with the
/// registered app it belongs to.
///
/// Every file must be a package signed by the release key, named as the
/// release names it (`<id>-<sha256>.cobalt-app`), for a registered app, and
/// still that app as registered: the same version and public listing, and a
/// minimum Cobalt version no newer than a build from this tree would need.
/// Anything else is refused, because the catalog is signed again here and
/// would otherwise vouch for something nobody reviewed.
///
/// # Errors
///
/// Returns a message naming the package and what disagreed.
pub(crate) fn read_kept_apps(
    apps: &[ReleaseApp],
    directory: &Path,
    public: &kobo_app_store::Ed25519PublicKey,
    base_url: &str,
) -> Result<Vec<KeptApp>, String> {
    let names = super::directory_entries(directory)?;
    let mut kept = Vec::with_capacity(names.len());
    let mut seen = BTreeSet::new();
    for name in names {
        let path = directory.join(&name);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect {}: {error}", path.display()))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!("kept package '{name}' must be a regular file"));
        }
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        let app = kept_app(apps, &name, &bytes, public, base_url)?;
        if !seen.insert(app.package.clone()) {
            return Err(format!(
                "two kept packages belong to {}; keep exactly one",
                app.package
            ));
        }
        kept.push(app);
    }
    Ok(kept)
}

fn kept_app(
    apps: &[ReleaseApp],
    name: &str,
    bytes: &[u8],
    public: &kobo_app_store::Ed25519PublicKey,
    base_url: &str,
) -> Result<KeptApp, String> {
    let parsed = kobo_app_store::parse_public_bundle(bytes, public).map_err(|error| {
        format!(
            "kept package '{name}' does not verify ({error}); give the app a new version so \
             it is built again"
        )
    })?;
    let manifest = parsed.manifest();
    let (expected_name, package_sha256) = super::release_package_name(manifest.id(), bytes);
    if name != expected_name {
        return Err(format!(
            "kept package '{name}' is not named as the release names it, '{expected_name}'"
        ));
    }
    let app = apps
        .iter()
        .find(|app| app.id == manifest.id())
        .ok_or_else(|| {
            format!(
                "kept package '{name}' is for '{}', which is not registered",
                manifest.id()
            )
        })?;
    let disagreements = disagreements(app, manifest);
    if !disagreements.is_empty() {
        return Err(format!(
            "{} cannot keep its published package: {}. Give it a new version so it is built \
             again.",
            app.id,
            disagreements.join("; ")
        ));
    }
    let entry = kobo_app_store::CatalogEntry::new(kobo_app_store::CatalogEntryInput {
        manifest: manifest.clone(),
        package_url: format!("{base_url}/{expected_name}"),
        package_sha256,
        package_bytes: u64::try_from(bytes.len())
            .map_err(|_| format!("kept package '{name}' is too large"))?,
    })
    .map_err(|error| format!("catalog {}: {error}", app.id))?;
    Ok(KeptApp {
        package: app.package.clone(),
        entry,
    })
}

/// What a published manifest says that the registry no longer does.
///
/// The minimum Cobalt version is the one field allowed to differ. It belongs
/// to the build: a package built under an older protocol needs an older
/// runtime than a build from this tree would. It may be older than the
/// registry's, never newer.
fn disagreements(app: &ReleaseApp, manifest: &kobo_app_store::Manifest) -> Vec<String> {
    let mut found = Vec::new();
    for (field, published, registered) in [
        ("version", manifest.version(), app.version.as_str()),
        (
            "display_name",
            manifest.display_name(),
            app.display_name.as_str(),
        ),
        (
            "short_label",
            manifest.short_label(),
            app.short_label.as_str(),
        ),
        ("summary", manifest.summary(), app.summary.as_str()),
        ("glyph", manifest.glyph(), app.glyph.as_str()),
    ] {
        if published != registered {
            found.push(format!(
                "published {field} {published:?} is not the registered {registered:?}"
            ));
        }
    }
    let published = manifest.capabilities().collect::<BTreeSet<_>>();
    let registered = app
        .capabilities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if published != registered {
        found.push("published capabilities are not the registered ones".to_owned());
    }
    if !kobo_app_store::cobalt_version_at_least(
        &app.minimum_cobalt_version,
        manifest.minimum_cobalt_version(),
    ) {
        found.push(format!(
            "published minimum Cobalt {} is newer than the {} a build from this tree needs",
            manifest.minimum_cobalt_version(),
            app.minimum_cobalt_version
        ));
    }
    found
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::super::{release_catalog, ReleaseApp, ReleaseBinaries};

    const BASE_URL: &str = "https://example.test/app-catalog-beta";
    const SEED: [u8; 32] = [9; 32];

    /// The smallest file the release tool accepts as a static ARM executable,
    /// with `marker` written past the headers so two apps differ.
    fn arm_binary(marker: u8) -> Vec<u8> {
        let mut binary = vec![0_u8; 85];
        binary[..6].copy_from_slice(b"\x7fELF\x01\x01");
        binary[16..18].copy_from_slice(&2_u16.to_le_bytes());
        binary[18..20].copy_from_slice(&40_u16.to_le_bytes());
        binary[24..28].copy_from_slice(&0x10_0040_u32.to_le_bytes());
        binary[28..32].copy_from_slice(&52_u32.to_le_bytes());
        binary[36..40].copy_from_slice(&0x400_u32.to_le_bytes());
        binary[42..44].copy_from_slice(&32_u16.to_le_bytes());
        binary[44..46].copy_from_slice(&1_u16.to_le_bytes());
        binary[52..56].copy_from_slice(&1_u32.to_le_bytes());
        binary[60..64].copy_from_slice(&0x10_0000_u32.to_le_bytes());
        let size = u32::try_from(binary.len()).expect("fixture fits in u32");
        binary[68..72].copy_from_slice(&size.to_le_bytes());
        binary[72..76].copy_from_slice(&size.to_le_bytes());
        binary[76..80].copy_from_slice(&5_u32.to_le_bytes());
        binary[84] = marker;
        binary
    }

    fn app(id: &str, version: &str, minimum: &str) -> ReleaseApp {
        ReleaseApp {
            package: format!("kobo-{id}"),
            id: id.to_owned(),
            display_name: id.to_uppercase(),
            short_label: id.to_uppercase(),
            summary: format!("Does {id}."),
            version: version.to_owned(),
            minimum_cobalt_version: minimum.to_owned(),
            glyph: "dice".to_owned(),
            capabilities: Vec::new(),
        }
    }

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("kobo-kept-apps-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("create fixture");
            Self { root }
        }

        fn directory(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::create_dir_all(&path).expect("create directory");
            path
        }

        /// Publishes `apps` from binaries marked by their position, as the
        /// release workflow does from freshly built ones.
        fn publish(
            &self,
            name: &str,
            apps: &[ReleaseApp],
            kept: Option<&Path>,
        ) -> Result<PathBuf, String> {
            let prebuilt = self.directory(&format!("{name}-prebuilt"));
            for (index, app) in apps.iter().enumerate() {
                let kept_here = kept.is_some_and(|directory| {
                    fs::read_dir(directory).expect("read kept").any(|entry| {
                        entry
                            .expect("entry")
                            .file_name()
                            .to_string_lossy()
                            .starts_with(&format!("{}-", app.id))
                    })
                });
                if !kept_here {
                    // Each publication builds different bytes, as a new
                    // build of a changed app would.
                    let marker = u8::try_from(index + name.len()).expect("few apps");
                    fs::write(prebuilt.join(&app.package), arm_binary(marker))
                        .expect("write binary");
                }
            }
            let output = self.directory(&format!("{name}-out"));
            release_catalog(
                apps,
                &ReleaseBinaries::Prebuilt(prebuilt),
                kept,
                &SEED,
                BASE_URL,
                &output,
            )?;
            Ok(output)
        }

        /// Copies the package `output` published for `id` into a directory of
        /// kept packages, as the workflow downloads it from the release.
        fn keep(&self, output: &Path, id: &str) -> PathBuf {
            let kept = self.directory("kept");
            for entry in fs::read_dir(output).expect("read output") {
                let entry = entry.expect("entry");
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{id}-")) && name.ends_with(".cobalt-app") {
                    fs::copy(entry.path(), kept.join(&name)).expect("copy package");
                }
            }
            kept
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn catalog(output: &Path) -> kobo_app_store::Catalog {
        let bytes = fs::read(output.join(kobo_app_store::CATALOG_FILE)).expect("read catalog");
        kobo_app_store::Catalog::parse_public(&bytes).expect("parse catalog")
    }

    fn entry<'a>(
        catalog: &'a kobo_app_store::Catalog,
        id: &str,
    ) -> &'a kobo_app_store::CatalogEntry {
        catalog
            .entries()
            .iter()
            .find(|entry| entry.manifest().id() == id)
            .expect("entry for app")
    }

    #[test]
    fn a_platform_change_keeps_the_published_package_and_its_minimum_beside_a_rebuilt_app() {
        let fixture = Fixture::new("kept");
        let first = fixture
            .publish(
                "first",
                &[
                    app("abacus", "1.0.0", "0.3.34"),
                    app("beacon", "2.0.0", "0.3.34"),
                ],
                None,
            )
            .expect("first publication");
        let kept = fixture.keep(&first, "abacus");

        // The platform moved to a protocol first read by 0.3.40, which every
        // build from the tree now needs. Beacon changed and was rebuilt;
        // Abacus did not and keeps what readers already have.
        let second = fixture
            .publish(
                "second",
                &[
                    app("abacus", "1.0.0", "0.3.40"),
                    app("beacon", "2.0.1", "0.3.40"),
                ],
                Some(&kept),
            )
            .expect("second publication");

        let before = catalog(&first);
        let after = catalog(&second);
        assert_eq!(entry(&after, "abacus"), entry(&before, "abacus"));
        assert_eq!(
            entry(&after, "abacus").manifest().minimum_cobalt_version(),
            "0.3.34"
        );
        assert_eq!(entry(&after, "beacon").manifest().version(), "2.0.1");
        assert_eq!(
            entry(&after, "beacon").manifest().minimum_cobalt_version(),
            "0.3.40"
        );
        let written = fs::read_dir(&second)
            .expect("read output")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".cobalt-app"))
            .collect::<Vec<_>>();
        assert_eq!(
            written.len(),
            1,
            "only the rebuilt app gets a new package: {written:?}"
        );
        assert!(written[0].starts_with("beacon-"));
    }

    #[test]
    fn an_app_whose_listing_changed_cannot_keep_its_published_package() {
        let fixture = Fixture::new("listing");
        let first = fixture
            .publish("first", &[app("abacus", "1.0.0", "0.3.34")], None)
            .expect("first publication");
        let kept = fixture.keep(&first, "abacus");
        let mut renamed = app("abacus", "1.0.0", "0.3.34");
        renamed.summary = "Counts beads.".to_owned();

        let error = fixture
            .publish("second", &[renamed], Some(&kept))
            .expect_err("a changed listing was kept under the old version");
        assert!(error.contains("summary"), "{error}");
        assert!(error.contains("new version"), "{error}");
    }

    #[test]
    fn an_app_given_a_new_version_is_built_again_rather_than_kept() {
        let fixture = Fixture::new("version");
        let first = fixture
            .publish("first", &[app("abacus", "1.0.0", "0.3.34")], None)
            .expect("first publication");
        let kept = fixture.keep(&first, "abacus");

        let error = fixture
            .publish("second", &[app("abacus", "1.0.1", "0.3.34")], Some(&kept))
            .expect_err("an old package was kept under a new version");
        assert!(error.contains("version"), "{error}");
    }

    #[test]
    fn a_kept_package_cannot_need_a_newer_cobalt_than_a_build_from_the_tree() {
        let fixture = Fixture::new("minimum");
        let first = fixture
            .publish("first", &[app("abacus", "1.0.0", "0.3.40")], None)
            .expect("first publication");
        let kept = fixture.keep(&first, "abacus");

        let error = fixture
            .publish("second", &[app("abacus", "1.0.0", "0.3.34")], Some(&kept))
            .expect_err("a kept package raised its own minimum past the tree's");
        assert!(error.contains("minimum Cobalt 0.3.40"), "{error}");
    }

    #[test]
    fn a_package_the_release_key_did_not_sign_is_never_kept() {
        let fixture = Fixture::new("forged");
        let kept = fixture.directory("kept");
        let binary = arm_binary(1);
        let manifest = kobo_app_store::Manifest::new_public(kobo_app_store::ManifestInput {
            id: "abacus".to_owned(),
            display_name: "ABACUS".to_owned(),
            short_label: "ABACUS".to_owned(),
            summary: "Does abacus.".to_owned(),
            version: "1.0.0".to_owned(),
            minimum_cobalt_version: "0.3.34".to_owned(),
            glyph: "dice".to_owned(),
            capabilities: Vec::new(),
            binary_sha256: kobo_net::sha256::hex_digest(&binary),
            binary_bytes: binary.len() as u64,
        })
        .expect("manifest");
        let bundle = kobo_app_store::build_bundle(&manifest, &binary, &[7; 32]).expect("bundle");
        let (name, _) = super::super::release_package_name("abacus", &bundle);
        fs::write(kept.join(name), bundle).expect("write package");

        let error = fixture
            .publish("second", &[app("abacus", "1.0.0", "0.3.34")], Some(&kept))
            .expect_err("a package signed by another key was kept");
        assert!(error.contains("does not verify"), "{error}");
    }

    #[test]
    fn an_app_neither_kept_nor_built_holds_the_catalog_back() {
        let fixture = Fixture::new("missing");
        let first = fixture
            .publish("first", &[app("abacus", "1.0.0", "0.3.34")], None)
            .expect("first publication");
        let kept = fixture.keep(&first, "abacus");
        let empty = fixture.directory("empty");
        let output = fixture.directory("out");

        let error = release_catalog(
            &[
                app("abacus", "1.0.0", "0.3.34"),
                app("beacon", "1.0.0", "0.3.34"),
            ],
            &ReleaseBinaries::Prebuilt(empty),
            Some(&kept),
            &SEED,
            BASE_URL,
            &output,
        )
        .expect_err("a catalog without beacon's binary was signed");
        assert!(error.contains("kobo-beacon"), "{error}");
    }
}
