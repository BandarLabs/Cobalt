//! The USB app verbs against folders shaped like a mounted reader and a
//! catalog signed with a fixture key, so every refusal the reader would
//! make is seen being made here too.

use std::cell::Cell;
use std::collections::BTreeMap;

use kobo_app_store::{
    build_bundle, derive_public_key, sign, Catalog, CatalogEntry, CatalogEntryInput, Manifest,
    ManifestInput,
};

use super::*;

const SEED: [u8; 32] = [7; 32];

/// A folder laid out like a reader's book partition with Cobalt `version`
/// on it. Named for the test so two tests never share one.
fn volume(name: &str, version: &str) -> PathBuf {
    let volume = std::env::temp_dir().join(format!("kobo-usb-apps-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&volume);
    fill(&volume, version);
    volume
}

fn fill(volume: &Path, version: &str) {
    fs::create_dir_all(volume.join(".kobo")).expect("system folder");
    fs::write(
        volume.join(".kobo/version"),
        "N249123456789,4.9.77,4.45.23640,4.9.77,4.9.77,00000000-0000-0000-0000-000000000391\n",
    )
    .expect("version file");
    let root = volume.join(setup::INSTALL_FOLDER);
    fs::create_dir_all(&root).expect("install folder");
    fs::write(root.join("VERSION"), format!("{version}\n")).expect("VERSION");
}

/// One app a fixture catalog offers.
struct Offer {
    id: &'static str,
    title: &'static str,
    version: &'static str,
    minimum: &'static str,
}

const fn offer(id: &'static str, title: &'static str, version: &'static str) -> Offer {
    Offer {
        id,
        title,
        version,
        minimum: "0.3.0",
    }
}

/// A signed catalog and its packages, answering fetches the way the
/// published one does and counting the packages it hands out.
struct Published {
    answers: BTreeMap<String, Vec<u8>>,
    packages: Cell<usize>,
}

impl Published {
    fn stable(offers: &[Offer]) -> Self {
        Self::on(
            offers,
            kobod::app_store::CATALOG_URL,
            kobod::app_store::CATALOG_SIGNATURE_URL,
        )
    }

    fn on(offers: &[Offer], catalog_url: &str, signature_url: &str) -> Self {
        let mut answers = BTreeMap::new();
        let mut entries = Vec::new();
        for offer in offers {
            let binary = format!("{} {} binary", offer.id, offer.version).into_bytes();
            let manifest = Manifest::new(
                ManifestInput {
                    id: offer.id.to_owned(),
                    display_name: offer.title.to_owned(),
                    short_label: offer.title.to_owned(),
                    summary: format!("The {} fixture.", offer.title),
                    version: offer.version.to_owned(),
                    minimum_cobalt_version: offer.minimum.to_owned(),
                    glyph: "note".to_owned(),
                    capabilities: Vec::new(),
                    binary_sha256: crate::sha256::hex_digest(&binary),
                    binary_bytes: binary.len() as u64,
                },
                kobo_app_store::public_reserved_app_ids(),
            )
            .expect("manifest");
            let package = build_bundle(&manifest, &binary, &SEED).expect("bundle");
            let url = format!("https://fixtures.invalid/{}.cobalt-app", offer.id);
            entries.push(
                CatalogEntry::new(CatalogEntryInput {
                    manifest,
                    package_url: url.clone(),
                    package_sha256: crate::sha256::hex_digest(&package),
                    package_bytes: package.len() as u64,
                })
                .expect("entry"),
            );
            answers.insert(url, package);
        }
        let catalog = Catalog::new(entries).expect("catalog").to_canonical_bytes();
        let signature = format!("{}\n", sign(&catalog, &SEED).expect("signature")).into_bytes();
        answers.insert(catalog_url.to_owned(), catalog);
        answers.insert(signature_url.to_owned(), signature);
        Self {
            answers,
            packages: Cell::new(0),
        }
    }

    fn fetch(&self, url: &str, _maximum: u32) -> Result<Vec<u8>, DeviceError> {
        if url.ends_with(".cobalt-app") {
            self.packages.set(self.packages.get() + 1);
        }
        self.answers.get(url).cloned().ok_or(DeviceError::NotFound)
    }
}

fn key() -> Ed25519PublicKey {
    derive_public_key(&SEED).expect("fixture key")
}

fn never_ejects(_: &Path) -> Result<(), String> {
    panic!("nothing asked for an eject")
}

/// Runs one verb the way the command does, short of finding the reader.
fn verb(volume: &Path, published: &Published, line: &[&str]) -> Result<Answer, String> {
    let words: Vec<String> = line[1..].iter().map(|word| (*word).to_owned()).collect();
    let options = parse(line[0], &words)?;
    let installation = Installation::on(volume)?;
    run(
        line[0],
        &options,
        &installation,
        &key(),
        |url, maximum| published.fetch(url, maximum),
        never_ejects,
    )
}

#[test]
fn a_reader_without_cobalt_is_refused_and_told_what_installs_it() {
    let volume = volume("no-cobalt", "0.3.37");
    fs::remove_dir_all(volume.join(setup::INSTALL_FOLDER)).expect("remove the install");
    let error = Installation::on(&volume).expect_err("no Cobalt on the volume");
    assert_eq!(console::category_of(&error), console::EXIT_TARGET);
    assert!(error.contains("Cobalt is not installed on the reader"));
    assert!(error.contains("kobo setup"));
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn an_installation_that_cannot_name_its_release_is_refused_rather_than_guessed_at() {
    let volume = volume("no-release", "not a version");
    let error = Installation::on(&volume).expect_err("unreadable release");
    assert!(error.contains("does not say which release it is"));
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn the_release_and_the_channel_are_the_readers_own() {
    let volume = volume("identity", "0.3.12");
    let stable = Installation::on(&volume).expect("installation");
    assert_eq!(stable.version, "0.3.12");
    assert_eq!(stable.channel, UpdateChannel::Stable);
    assert_eq!(stable.root, volume.join(".adds/cobalt"));

    let state = volume.join(".adds/cobalt/state");
    fs::create_dir_all(&state).expect("state folder");
    fs::write(
        state.join("auto-update"),
        "cobalt=on\napps=off\nchannel=beta\n",
    )
    .expect("preferences");
    let beta = Installation::on(&volume).expect("installation");
    assert_eq!(beta.channel, UpdateChannel::Beta);
    assert_eq!(beta.channel_name(), "beta");
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn a_reader_on_beta_is_offered_the_beta_catalog_and_only_that() {
    let volume = volume("beta-catalog", "0.3.37");
    let state = volume.join(".adds/cobalt/state");
    fs::create_dir_all(&state).expect("state folder");
    fs::write(state.join("auto-update"), "channel=beta\n").expect("preferences");
    let beta = Published::on(
        &[offer("almanac", "Almanac", "2.0.0")],
        kobod::app_store::BETA_CATALOG_URL,
        kobod::app_store::BETA_CATALOG_SIGNATURE_URL,
    );
    let answer = verb(&volume, &beta, &["available"]).expect("beta listing");
    assert!(answer.text.contains("beta catalog"));
    assert!(answer.text.contains("Almanac (almanac) 2.0.0"));

    // The same reader is not answered from the stable catalog: the stable
    // fixture does not publish at the beta address, so the fetch finds
    // nothing and says which catalog it was looking for.
    let stable = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    let error = verb(&volume, &stable, &["available"]).expect_err("no beta catalog");
    assert!(error.contains("it is not where the beta catalog is published"));
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn install_then_list_then_update_then_remove_is_the_whole_life_of_an_app() {
    let volume = volume("lifecycle", "0.3.37");
    let first = Published::stable(&[
        offer("almanac", "Almanac", "1.0.0"),
        offer("tide-table", "Tide Table", "3.1.0"),
    ]);

    let listed = verb(&volume, &first, &["available"]).expect("available");
    assert!(listed.text.contains("Almanac (almanac) 1.0.0\n"));
    assert!(listed.text.contains("The Almanac fixture."));
    assert_eq!(listed.data["apps"][0]["installed_version"], Value::Null);
    assert_eq!(listed.data["reader"]["cobalt_version"], "0.3.37");
    assert_eq!(listed.data["reader"]["channel"], "stable");

    let installed = verb(&volume, &first, &["install", "almanac"]).expect("install");
    assert_eq!(installed.failed, 0);
    assert!(installed.text.contains("Installed Almanac 1.0.0."));
    assert!(installed
        .text
        .contains("Eject the reader before you unplug it"));
    assert_eq!(installed.data["results"][0]["outcome"], "installed");
    assert_eq!(installed.data["ejected"], false);
    assert_eq!(
        fs::read(volume.join(".adds/cobalt/apps/almanac/bin/kobo-almanac")).expect("binary"),
        b"almanac 1.0.0 binary"
    );

    let on_reader = verb(&volume, &first, &["installed"]).expect("installed");
    assert!(on_reader.text.contains("  Almanac (almanac) 1.0.0\n"));
    assert!(!on_reader.text.contains("Tide Table"));
    assert_eq!(on_reader.data["apps"][0]["id"], "almanac");

    let second = Published::stable(&[
        offer("almanac", "Almanac", "1.1.0"),
        offer("tide-table", "Tide Table", "3.1.0"),
    ]);
    let stale = verb(&volume, &second, &["available"]).expect("available with an update");
    assert!(stale
        .text
        .contains("Almanac (almanac) 1.1.0, update from 1.0.0"));
    assert_eq!(stale.data["apps"][0]["update_available"], true);
    let updated = verb(&volume, &second, &["install", "almanac"]).expect("update");
    assert!(updated
        .text
        .contains("Updated Almanac from 1.0.0 to 1.1.0."));
    assert_eq!(updated.data["results"][0]["outcome"], "updated");
    assert_eq!(updated.data["results"][0]["previous_version"], "1.0.0");

    // The app keeps its own files beside the package, not inside it, and
    // removing the package must not take them.
    let data = volume.join(".adds/cobalt/data/almanac");
    fs::create_dir_all(&data).expect("app data");
    fs::write(data.join("notes"), b"kept").expect("app data file");
    let removed = verb(&volume, &second, &["remove", "almanac"]).expect("remove");
    assert!(removed.text.contains("Removed Almanac from the reader."));
    assert_eq!(removed.data["removed"]["id"], "almanac");
    assert!(!volume.join(".adds/cobalt/apps/almanac").exists());
    assert_eq!(
        fs::read(data.join("notes")).expect("data survives"),
        b"kept"
    );
    let empty = verb(&volume, &second, &["installed"]).expect("installed after remove");
    assert!(empty.text.contains("No apps are installed."));
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn an_app_already_at_the_catalogs_version_is_verified_and_not_downloaded_again() {
    let volume = volume("current", "0.3.37");
    let published = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    verb(&volume, &published, &["install", "almanac"]).expect("first install");
    assert_eq!(published.packages.get(), 1);

    let again = verb(&volume, &published, &["install", "almanac"]).expect("second install");
    assert!(again
        .text
        .contains("Almanac 1.0.0 is already on the reader."));
    assert_eq!(again.data["results"][0]["outcome"], "current");
    assert_eq!(published.packages.get(), 1, "nothing was fetched again");
    assert!(
        !again.text.contains("Eject the reader"),
        "nothing was written, so there is nothing to flush"
    );
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn an_installed_binary_that_no_longer_matches_is_installed_over_rather_than_called_current() {
    let volume = volume("repair", "0.3.37");
    let published = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    verb(&volume, &published, &["install", "almanac"]).expect("first install");
    // Same length, different bytes: the listing's length check passes, which
    // is exactly the case that must not be reported as already installed.
    fs::write(
        volume.join(".adds/cobalt/apps/almanac/bin/kobo-almanac"),
        b"almanac 1.0.0 BINARY",
    )
    .expect("corrupt the binary");

    let repaired = verb(&volume, &published, &["install", "almanac"]).expect("repair");
    assert_eq!(repaired.data["results"][0]["outcome"], "repaired");
    assert!(repaired.text.contains("did not verify"));
    assert_eq!(published.packages.get(), 2);
    assert_eq!(
        fs::read(volume.join(".adds/cobalt/apps/almanac/bin/kobo-almanac")).expect("binary"),
        b"almanac 1.0.0 binary"
    );
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn the_minimum_cobalt_is_checked_against_the_reader_and_not_this_build() {
    let needs_future = Offer {
        id: "almanac",
        title: "Almanac",
        version: "1.0.0",
        minimum: "999.0.0",
    };
    let published = Published::stable(&[needs_future]);

    // A reader older than the package asks for is refused before anything is
    // downloaded, in words that name both releases and what to do.
    let old = volume("minimum-old", env!("CARGO_PKG_VERSION"));
    let listed = verb(&old, &published, &["available"]).expect("available");
    assert!(listed.text.contains("needs Cobalt 999.0.0 or newer"));
    assert_eq!(listed.data["apps"][0]["runs_on_this_reader"], false);
    let refused = verb(&old, &published, &["install", "almanac"]).expect("answered");
    assert_eq!(refused.failed, 1);
    assert!(refused.text.contains(&format!(
        "Almanac 1.0.0 needs Cobalt 999.0.0 or newer, and this reader has {}.",
        env!("CARGO_PKG_VERSION")
    )));
    assert_eq!(published.packages.get(), 0);
    assert!(!old.join(".adds/cobalt/apps/almanac").exists());

    // A reader that is new enough takes the same package from this same
    // build, which is itself far older than the package asks for.
    let new = volume("minimum-new", "999.0.1");
    let installed = verb(&new, &published, &["install", "almanac"]).expect("install");
    assert_eq!(installed.failed, 0);
    assert!(installed.text.contains("Installed Almanac 1.0.0."));
    let _ = fs::remove_dir_all(old);
    let _ = fs::remove_dir_all(new);
}

#[test]
fn a_package_that_is_not_the_one_the_catalog_signed_for_is_never_written() {
    let volume = volume("tampered", "0.3.37");
    let mut published = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    let package = published
        .answers
        .get_mut("https://fixtures.invalid/almanac.cobalt-app")
        .expect("package");
    let last = package.len() - 1;
    package[last] ^= 1;

    let refused = verb(&volume, &published, &["install", "almanac"]).expect("answered");
    assert_eq!(refused.failed, 1);
    assert!(refused
        .text
        .contains("does not match its published signature and digest"));
    assert_eq!(refused.data["results"][0]["outcome"], "failed");
    assert!(!volume.join(".adds/cobalt/apps/almanac").exists());
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn a_catalog_signed_with_another_key_is_refused_and_leaves_nothing_on_the_reader() {
    let volume = volume("wrong-key", "0.3.37");
    let published = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    let installation = Installation::on(&volume).expect("installation");
    let other = derive_public_key(&[8; 32]).expect("another key");
    let error = installation
        .available(&other, |url, maximum| published.fetch(url, maximum))
        .expect_err("a catalog this key did not sign");
    assert!(error.contains("does not match its published signature and digest"));
    assert!(!volume.join(".adds/cobalt/store").join("catalog").exists());
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn several_apps_are_each_answered_and_one_failure_does_not_stop_the_rest() {
    let volume = volume("several", "0.3.37");
    let published = Published::stable(&[
        offer("almanac", "Almanac", "1.0.0"),
        offer("tide-table", "Tide Table", "3.1.0"),
    ]);
    let answer = verb(
        &volume,
        &published,
        &["install", "almanac", "sundial", "Tide Table"],
    )
    .expect("answered");
    assert_eq!(answer.failed, 1);
    assert!(answer.text.contains("Installed Almanac 1.0.0."));
    assert!(answer.text.contains("No app is called \"sundial\"."));
    assert!(answer.text.contains("Installed Tide Table 3.1.0."));
    assert_eq!(answer.data["results"][1]["name"], "sundial");
    assert_eq!(answer.data["results"][2]["id"], "tide-table");
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn removing_an_app_that_is_not_there_says_so_and_names_the_listing() {
    let volume = volume("remove-missing", "0.3.37");
    let published = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    let error = verb(&volume, &published, &["remove", "almanac"]).expect_err("not installed");
    assert!(error.contains("No app is called \"almanac\"."));
    assert!(error.contains("kobo apps installed"));
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn an_eject_is_only_made_when_asked_for_and_its_failure_is_said_not_hidden() {
    let volume = volume("eject", "0.3.37");
    let published = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    let installation = Installation::on(&volume).expect("installation");
    let options = parse("install", &["almanac".to_owned(), "--eject".to_owned()]).expect("parse");
    let ejected = Cell::new(false);
    let answer = run(
        "install",
        &options,
        &installation,
        &key(),
        |url, maximum| published.fetch(url, maximum),
        |path| {
            assert_eq!(path, volume);
            ejected.set(true);
            Ok(())
        },
    )
    .expect("install and eject");
    assert!(ejected.get());
    assert_eq!(answer.data["ejected"], true);
    assert!(answer.text.contains("The reader is ejected"));
    assert!(!answer.text.contains("Eject the reader before"));

    let options = parse("installed", &["--eject".to_owned()]).expect("parse");
    let answer = run(
        "installed",
        &options,
        &installation,
        &key(),
        |url, maximum| published.fetch(url, maximum),
        |_| Err("the volume is busy".to_owned()),
    )
    .expect("listing");
    assert_eq!(answer.data["ejected"], false);
    assert!(answer
        .text
        .contains("The reader was not ejected: the volume is busy"));
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn a_macs_sidecar_files_are_swept_and_a_file_that_only_has_the_name_is_kept() {
    let volume = volume("sidecars", "0.3.37");
    let published = Published::stable(&[offer("almanac", "Almanac", "1.0.0")]);
    verb(&volume, &published, &["install", "almanac"]).expect("install");
    let apps = volume.join(".adds/cobalt/apps");
    let sidecar = [&APPLE_DOUBLE[..], &[0_u8; 60][..]].concat();
    let swept = [
        apps.join("._almanac"),
        apps.join("almanac/._manifest.json"),
        apps.join("almanac/bin/._kobo-almanac"),
        volume.join(".adds/cobalt/store/catalog/._catalog.json"),
    ];
    for path in &swept {
        fs::write(path, &sidecar).expect("sidecar");
    }
    // Not an AppleDouble file, whatever it is called, and one that is but
    // sits in a folder these verbs never write to.
    let kept = [
        apps.join("almanac/._notes"),
        volume.join(".adds/cobalt/._VERSION"),
    ];
    fs::write(&kept[0], b"an owner's own file").expect("lookalike");
    fs::write(&kept[1], &sidecar).expect("sidecar elsewhere");

    verb(&volume, &published, &["available"]).expect("any verb that writes");
    for path in &swept {
        assert!(!path.exists(), "{} was left", path.display());
    }
    for path in &kept {
        assert!(path.exists(), "{} was taken", path.display());
    }
    assert!(
        Installation::on(&volume)
            .expect("installation")
            .installed(&key())
            .expect("still verifies")
            .iter()
            .any(|app| app.id == "almanac"),
        "the sweep took nothing the package needs"
    );
    let _ = fs::remove_dir_all(volume);
}

#[test]
fn an_app_is_found_by_id_name_or_label_and_a_miss_suggests_the_nearest() {
    let app = |id: &str, title: &str| AppInfo {
        id: id.to_owned(),
        title: title.to_owned(),
        label: title.to_owned(),
        summary: String::new(),
        version: "1.0.0".to_owned(),
        minimum_cobalt_version: "0.3.0".to_owned(),
        glyph: kobo_ui::Glyph::App,
        capabilities: Vec::new(),
        installed_version: None,
    };
    let apps = [app("musicstand", "Music Stand"), app("rss", "Feeds")];
    assert_eq!(find(&apps, "rss").expect("by id").id, "rss");
    assert_eq!(
        find(&apps, "music stand").expect("by name").id,
        "musicstand"
    );
    assert_eq!(find(&apps, "FEEDS").expect("any case").id, "rss");
    let miss = find(&apps, "musicstnd").expect_err("a typing mistake");
    assert!(miss.contains("Did you mean Music Stand (musicstand)?"));
    assert_eq!(
        find(&apps, "chess").expect_err("nothing near"),
        "No app is called \"chess\"."
    );
}

#[test]
fn each_verb_refuses_a_line_it_cannot_mean() {
    let words =
        |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_owned()).collect() };
    for (verb, line) in [
        ("install", vec![]),
        ("remove", vec!["--json"]),
        ("installed", vec!["almanac"]),
        ("available", vec!["--channel", "beta"]),
        ("install", vec!["almanac", "--volume"]),
    ] {
        let error = parse(verb, &words(&line)).expect_err("a line that means nothing");
        assert_eq!(
            console::category_of(&error),
            console::EXIT_USAGE,
            "{verb} {line:?}"
        );
    }
    let parsed = parse(
        "install",
        &words(&["almanac", "--volume", "/Volumes/KOBOeReader", "--json"]),
    )
    .expect("a good line");
    assert_eq!(
        parsed.volume.as_deref(),
        Some(Path::new("/Volumes/KOBOeReader"))
    );
    assert!(parsed.json);
    assert_eq!(parsed.names, ["almanac"]);
}

#[test]
fn a_missing_reader_is_answered_with_this_commands_own_way_to_name_one() {
    let nowhere =
        std::env::temp_dir().join(format!("kobo-usb-apps-nowhere-{}", std::process::id()));
    let error = reader(Some(&nowhere)).expect_err("not a reader");
    assert_eq!(console::category_of(&error), console::EXIT_TARGET);
    assert!(error.contains("is not a mounted reader"));
    // The advice the chooser gives for a reader it cannot find names the
    // setup command; here it has to name this one.
    let advice = crate::NO_READER_FOUND.replace("kobo setup --volume", "kobo apps --volume");
    assert!(crate::NO_READER_FOUND.contains("kobo setup --volume"));
    assert!(advice.contains("kobo apps --volume /path"));
}

/// The whole transaction on a real FAT volume mounted on this computer.
///
/// Ignored because it needs one: no temporary folder on a developer's disk
/// is FAT, and FAT is where a directory flush, a rename over a just-moved
/// folder and a mode change are answered by a different driver on every
/// operating system. Point `KOBO_USB_TEST_VOLUME` at an empty folder on a
/// mounted FAT volume (a reader, a memory stick, or a disk image) and run
/// `cargo test -p kobo-cli -- --ignored a_fat_volume`.
#[test]
#[ignore = "needs KOBO_USB_TEST_VOLUME naming a folder on a mounted FAT volume"]
fn a_fat_volume_mounted_on_this_computer_takes_the_whole_transaction() {
    let Some(volume) = std::env::var_os("KOBO_USB_TEST_VOLUME").map(PathBuf::from) else {
        panic!("set KOBO_USB_TEST_VOLUME to a folder on a mounted FAT volume");
    };
    let _ = fs::remove_dir_all(volume.join(".adds"));
    fill(&volume, "0.3.37");
    let first = Published::stable(&[
        offer("almanac", "Almanac", "1.0.0"),
        offer("tide-table", "Tide Table", "3.1.0"),
    ]);
    let installed =
        verb(&volume, &first, &["install", "almanac", "tide-table"]).expect("install two");
    assert_eq!(installed.failed, 0, "{}", installed.text);

    let second = Published::stable(&[
        offer("almanac", "Almanac", "1.1.0"),
        offer("tide-table", "Tide Table", "3.1.0"),
    ]);
    let updated = verb(&volume, &second, &["install", "almanac", "tide-table"]).expect("update");
    assert_eq!(updated.failed, 0, "{}", updated.text);
    assert!(updated
        .text
        .contains("Updated Almanac from 1.0.0 to 1.1.0."));
    assert!(updated
        .text
        .contains("Tide Table 3.1.0 is already on the reader."));

    let listed = verb(&volume, &second, &["installed"]).expect("installed");
    assert!(listed.text.contains("Almanac (almanac) 1.1.0"));
    verb(&volume, &second, &["remove", "tide-table"]).expect("remove");
    let listed = verb(&volume, &second, &["installed"]).expect("installed after remove");
    assert!(!listed.text.contains("Tide Table"));

    // What is left is what the runtime itself leaves: the app, the copy it
    // replaced (kept until the next install, as the rollback), and the mark
    // that says the other was removed on purpose. No staging folder, no
    // removal in progress, and none of the sidecar files a Mac writes.
    let mut left: Vec<String> = fs::read_dir(volume.join(".adds/cobalt/apps"))
        .expect("apps folder")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    left.sort();
    assert_eq!(left, ["almanac", "almanac.prev", "tide-table.uninstalled"]);
    for folder in ["apps/almanac", "apps/almanac/bin", "store", "store/catalog"] {
        let sidecars: Vec<String> = fs::read_dir(volume.join(".adds/cobalt").join(folder))
            .expect("folder")
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("._"))
            .collect();
        assert!(sidecars.is_empty(), "{folder}: {sidecars:?}");
    }
}
