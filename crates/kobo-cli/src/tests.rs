#[test]
fn owner_help_names_every_shipped_companion_and_never_advertises_compiled_out_panel_writes() {
    let source = include_str!("main.rs");
    let help_start = source.find("fn print_help()").unwrap();
    let other_start = source.find("fn print_other_names()").unwrap();
    let help = &source[help_start..other_start];
    for command in [
        "apps",
        "deck",
        "flashcards",
        "frame",
        "musicstand",
        "post",
        "readlater",
        "birds",
        "vault",
        "sync",
        "sidekick",
        "export",
        "feeds",
        "fieldbook",
        "needles",
        "nonograms",
        "parser",
        "panels",
    ] {
        assert!(help.contains(command), "public help omitted {command}");
    }
    assert!(!help.contains("present <app>"));
    assert!(!help.contains("stop --device IP"));
    #[cfg(feature = "device-write")]
    {
        let conditional = &source[other_start..];
        assert!(conditional.contains("present <app>"));
        assert!(conditional.contains("stop --device IP"));
    }
}

#[test]
fn stream_init_refuses_a_reader_it_cannot_reach_before_minting_anything() {
    // A typo used to be found after the certificate had been minted and
    // the pairing code printed, which leaves a half-done pairing behind.
    let error =
        super::stream_init(&["--device".to_owned(), "not a host".to_owned()]).expect_err("refused");
    assert!(error.contains("not an address"), "{error}");
}

#[test]
fn stream_init_names_its_arguments_and_nothing_else() {
    // --reader NAME is a real argument now: it names the pairing in the
    // saved-reader store. What stays refused is a flag missing its value.
    for arguments in [vec!["--device".to_owned()], vec!["--host".to_owned()]] {
        let error = super::stream_init(&arguments).expect_err("refused");
        assert!(
            error.starts_with("usage: kobo stream init"),
            "{arguments:?} gave {error}"
        );
    }
}

#[test]
fn the_stream_authority_says_which_file_is_missing() {
    // Under an empty root this names the file rather than failing later
    // inside an install with nothing to point at.
    let empty = std::env::temp_dir().join("kobo-stream-authority-test");
    let _ignored = std::fs::create_dir_all(&empty);
    let error = super::stream_authority_in(&empty).expect_err("nothing is there");
    assert!(error.contains("ca-cert.pem"), "{error}");
}

#[test]
fn companion_help_exits_successfully() {
    super::parser_command(&["--help".into()]).expect("parser help");
    assert!(super::PARSER_USAGE.contains("parser inspect FILE"));
    assert!(super::PARSER_USAGE.contains("--sim | --device IP"));
    super::stream_command(&["--help".into()]).expect("stream help");
    super::flashcards::command(&["--help".into()]).expect("flashcards help");
    super::deck::command(&["--help".into()]).expect("deck help");
    super::frame::command(&["--help".into()]).expect("frame help");
    super::sync::command(&["--help".into()]).expect("sync help");
    super::needles::command(&["--help".into()]).expect("needles help");
    super::nonograms::command(&["--help".into()]).expect("nonograms help");
    super::vault::command(&["--help".into()]).expect("vault help");
}

#[test]
fn parser_check_accepts_a_minimal_v5_header() {
    let mut bytes = vec![0_u8; 64];
    bytes[0] = 5;
    let path = std::env::temp_dir().join(format!("parser-check-{}.z5", std::process::id()));
    std::fs::write(&path, &bytes).expect("fixture");
    super::parser_command(&["check".into(), path.display().to_string()]).expect("check");
    std::fs::remove_file(path).expect("cleanup");
}

#[test]
fn parser_check_refuses_glulx() {
    let path = std::env::temp_dir().join(format!("parser-glulx-{}.ulx", std::process::id()));
    std::fs::write(&path, b"Glul\0\0\0\0").expect("fixture");
    let error =
        super::parser_command(&["check".into(), path.display().to_string()]).expect_err("glulx");
    std::fs::remove_file(path).expect("cleanup");
    assert!(error.contains("Glulx"));
}

/// Builds a recording the way the device writes one.
fn recording(width: u32, height: u32, frames: &[(u32, u8)]) -> Vec<u8> {
    let mut raw = b"KOBOCST1".to_vec();
    raw.extend_from_slice(&width.to_le_bytes());
    raw.extend_from_slice(&height.to_le_bytes());
    for (millis, fill) in frames {
        raw.extend_from_slice(&millis.to_le_bytes());
        raw.extend(std::iter::repeat_n(*fill, (width * height) as usize));
    }

    raw
}

#[test]
fn host_update_defaults_stable_and_requires_explicit_beta() {
    let arguments = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        super::parse_host_update(&arguments(&[])),
        Ok(super::HostUpdateChannel::Stable)
    );
    assert_eq!(
        super::parse_host_update(&arguments(&["--channel", "beta"])),
        Ok(super::HostUpdateChannel::Beta)
    );
    assert!(super::parse_host_update(&arguments(&["--beta"])).is_err());
    assert!(super::parse_host_update(&arguments(&["--channel", "nightly"])).is_err());
}

#[test]
fn release_seed_accepts_raw_or_lowercase_hex() {
    let root = std::env::temp_dir().join(format!("kobo-seed-test-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create fixture");
    let raw = root.join("raw");
    let hex = root.join("hex");
    fs::write(&raw, [7_u8; 32]).expect("write raw seed");
    fs::write(&hex, format!("{}\n", "07".repeat(32))).expect("write hex seed");
    assert_eq!(
        super::read_signing_seed(&raw).expect("raw seed"),
        [7_u8; 32]
    );
    assert_eq!(
        super::read_signing_seed(&hex).expect("hex seed"),
        [7_u8; 32]
    );
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn release_registry_accepts_website_setup_metadata() {
    let value = kobo_json::parse(
        r#"{
                "package":"kobo-library",
                "id":"library",
                "display_name":"Library",
                "short_label":"Library",
                "summary":"Read a personal library.",
                "version":"1.0.0",
                "minimum_cobalt_version":"0.2.4",
                "glyph":"book",
                "capabilities":["network"],
                "setup":{"steps":[{"text":"Create a read-only key."}]}
            }"#,
    )
    .expect("registry fixture");

    let app = super::parse_release_app(&value).expect("setup metadata is registry-only");
    assert_eq!(app.id, "library");
}

#[test]
#[allow(clippy::too_many_lines)] // One signed fixture exercises creation and tampering together.
fn app_bundle_and_catalog_commands_produce_verified_assets() {
    let root = std::env::temp_dir().join(format!("kobo-app-assets-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create fixture");
    let seed_path = root.join("seed");
    let manifest_path = root.join("manifest.json");
    let binary_path = root.join("kobo-word-count");
    let bundle_path = root.join("word-count.cobalt-app");
    let catalog_path = root.join("cobalt-app-catalog.json");
    let signature_path = root.join("cobalt-app-catalog.json.sig");
    let seed = [9_u8; 32];
    let mut binary = vec![0_u8; 84];
    binary[..6].copy_from_slice(b"\x7fELF\x01\x01");
    binary[16..18].copy_from_slice(&2_u16.to_le_bytes());
    binary[18..20].copy_from_slice(&40_u16.to_le_bytes());
    binary[24..28].copy_from_slice(&0x10_0040_u32.to_le_bytes());
    binary[28..32].copy_from_slice(&52_u32.to_le_bytes());
    binary[36..40].copy_from_slice(&0x400_u32.to_le_bytes());
    binary[42..44].copy_from_slice(&32_u16.to_le_bytes());
    binary[44..46].copy_from_slice(&1_u16.to_le_bytes());
    binary[52..56].copy_from_slice(&1_u32.to_le_bytes());
    binary[56..60].copy_from_slice(&0_u32.to_le_bytes());
    binary[60..64].copy_from_slice(&0x10_0000_u32.to_le_bytes());
    let binary_size = u32::try_from(binary.len()).expect("fixture fits in u32");
    binary[68..72].copy_from_slice(&binary_size.to_le_bytes());
    binary[72..76].copy_from_slice(&binary_size.to_le_bytes());
    binary[76..80].copy_from_slice(&5_u32.to_le_bytes());
    fs::write(&seed_path, seed).expect("write seed");
    fs::write(&binary_path, &binary).expect("write binary");
    let manifest = kobo_app_store::Manifest::new_public(kobo_app_store::ManifestInput {
        id: "word-count".to_owned(),
        display_name: "Word Count".to_owned(),
        short_label: "Words".to_owned(),
        summary: "Counts words in a note.".to_owned(),
        version: "1.0.0".to_owned(),
        minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
        glyph: "note".to_owned(),
        capabilities: Vec::new(),
        binary_sha256: kobo_net::sha256::hex_digest(&binary),
        binary_bytes: binary.len() as u64,
    })
    .expect("manifest");
    fs::write(&manifest_path, manifest.to_canonical_bytes()).expect("write manifest");

    let header_only = root.join("header-only");
    let mut header = vec![0_u8; 52];
    header[..6].copy_from_slice(b"\x7fELF\x01\x01");
    header[16..18].copy_from_slice(&2_u16.to_le_bytes());
    header[18..20].copy_from_slice(&40_u16.to_le_bytes());
    header[24..28].copy_from_slice(&0x10_0040_u32.to_le_bytes());
    header[36..40].copy_from_slice(&0x400_u32.to_le_bytes());
    header[42..44].copy_from_slice(&32_u16.to_le_bytes());
    fs::write(&header_only, header).expect("write header-only binary");
    assert!(super::verify_arm_elf(&header_only)
        .expect_err("an ELF header without a load segment was accepted")
        .contains("executable load segment"));

    super::app_bundle(&[
        "--manifest".to_owned(),
        manifest_path.display().to_string(),
        "--binary".to_owned(),
        binary_path.display().to_string(),
        "--seed".to_owned(),
        seed_path.display().to_string(),
        "--out".to_owned(),
        bundle_path.display().to_string(),
    ])
    .expect("build bundle");
    super::app_catalog(&[
        "--seed".to_owned(),
        seed_path.display().to_string(),
        "--out".to_owned(),
        catalog_path.display().to_string(),
        "--signature".to_owned(),
        signature_path.display().to_string(),
        "--entry".to_owned(),
        bundle_path.display().to_string(),
        "https://example.test/word-count.cobalt-app".to_owned(),
    ])
    .expect("build catalog");

    let public = kobo_app_store::derive_public_key(&seed).expect("public key");
    let bundle = fs::read(&bundle_path).expect("read bundle");
    assert_eq!(
        kobo_app_store::parse_public_bundle(&bundle, &public)
            .expect("verify bundle")
            .manifest()
            .id(),
        "word-count"
    );
    let catalog_bytes = fs::read(&catalog_path).expect("read catalog");
    let signature = fs::read_to_string(&signature_path).expect("read signature");
    let signature =
        kobo_app_store::DetachedSignature::from_hex(signature.trim()).expect("signature");
    kobo_app_store::verify(&catalog_bytes, &signature, &public).expect("verify catalog");
    let catalog = kobo_app_store::Catalog::parse_public(&catalog_bytes).expect("parse catalog");
    assert_eq!(catalog.entries().len(), 1);
    let public_path = root.join("public-key");
    fs::write(&public_path, public.to_string()).unwrap();
    let verify_package = vec![
        "--package".into(),
        bundle_path.display().to_string(),
        "--public-key".into(),
        public_path.display().to_string(),
        "--manifest".into(),
        manifest_path.display().to_string(),
        "--binary".into(),
        binary_path.display().to_string(),
    ];
    let verify_catalog = vec![
        "--catalog".into(),
        catalog_path.display().to_string(),
        "--signature".into(),
        signature_path.display().to_string(),
        "--public-key".into(),
        public_path.display().to_string(),
        "--package".into(),
        bundle_path.display().to_string(),
    ];
    super::app_verify(&verify_package).expect("verify matching package");
    super::app_catalog_verify(&verify_catalog).expect("verify matching catalog");
    fs::write(&binary_path, b"different binary").unwrap();
    assert!(super::app_verify(&verify_package).is_err());
    fs::write(&binary_path, &binary).unwrap();
    fs::write(
        &public_path,
        kobo_app_store::derive_public_key(&[8; 32])
            .unwrap()
            .to_string(),
    )
    .unwrap();
    assert!(super::app_verify(&verify_package).is_err());
    assert!(super::app_catalog_verify(&verify_catalog).is_err());
    fs::write(&public_path, public.to_string()).unwrap();
    let mut changed_catalog = catalog_bytes.clone();
    changed_catalog.push(b' ');
    fs::write(&catalog_path, changed_catalog).unwrap();
    assert!(super::app_catalog_verify(&verify_catalog).is_err());
    fs::write(&catalog_path, &catalog_bytes).unwrap();
    let wrong_entry = kobo_app_store::CatalogEntry::new(kobo_app_store::CatalogEntryInput {
        manifest: manifest.clone(),
        package_url: "https://example.test/wrong.cobalt-app".into(),
        package_sha256: kobo_net::sha256::hex_digest(&bundle),
        package_bytes: bundle.len() as u64 + 1,
    })
    .unwrap();
    let wrong_catalog = kobo_app_store::Catalog::new(vec![wrong_entry])
        .unwrap()
        .to_canonical_bytes();
    fs::write(&catalog_path, &wrong_catalog).unwrap();
    fs::write(
        &signature_path,
        kobo_app_store::sign(&wrong_catalog, &seed)
            .unwrap()
            .to_string(),
    )
    .unwrap();
    assert!(
        super::app_catalog_verify(&verify_catalog).is_err(),
        "valid signature must not hide incorrect package length"
    );
    fs::write(&catalog_path, &catalog_bytes).unwrap();
    fs::write(&signature_path, signature.to_string()).unwrap();
    let mut changed_package = bundle.clone();
    *changed_package.last_mut().unwrap() ^= 1;
    fs::write(&bundle_path, changed_package).unwrap();
    assert!(super::app_verify(&verify_package).is_err());
    assert!(super::app_catalog_verify(&verify_catalog).is_err());
    fs::remove_dir_all(root).expect("remove fixture");
}

/// A session is what somebody asked for by not asking for anything else.
#[test]
fn paperterm_presets_are_literal_commands_with_clear_titles() {
    let (command, title) = super::stream_preset("terminal", Some("/bin/sh")).unwrap();
    assert_eq!(command, ["/bin/sh", "-l"]);
    assert_eq!(title, "Terminal");
    assert!(super::stream_preset("terminal", Some("sh; echo unsafe")).is_err());
    let (command, title) = super::stream_preset("monitor", None).unwrap();
    assert_eq!(command, ["top"]);
    assert_eq!(title, "System monitor");
    assert!(super::stream_preset("unknown", None).is_err());
    assert!(super::STREAM_START.contains("kobo stream demo"));
    super::stream_command(&[]).unwrap();
}

#[test]
fn a_shell_with_no_command_is_a_request_for_a_session() {
    let arguments = ["--device".to_owned(), "192.168.1.2".to_owned()];
    let request =
        super::parse_shell(&arguments).expect("a host and nothing else is a valid request");
    assert_eq!(request.host, "192.168.1.2");
    assert!(request.command.is_none());
}

/// The words after the host are one line of shell, not a list of
/// arguments. Somebody typing a pipeline or a redirection means it.
#[test]
fn the_words_after_the_host_become_one_line_of_shell() {
    let arguments = [
        "-s".to_owned(),
        "192.168.1.2".to_owned(),
        "dmesg".to_owned(),
        "|".to_owned(),
        "tail".to_owned(),
        "-n".to_owned(),
        "5".to_owned(),
    ];
    let request = super::parse_shell(&arguments).expect("a command is a valid request");
    assert_eq!(request.command.as_deref(), Some("dmesg | tail -n 5"));
}

/// The host goes into an ssh argument, so anything that could be read as
/// something else has to be refused before it gets there.
#[test]
fn a_shell_refuses_a_host_that_is_not_one() {
    for host in ["a;rm -rf /", "1.2.3.4 -oProxyCommand=x", "$(whoami)"] {
        let arguments = ["--device".to_owned(), host.to_owned(), "uname".to_owned()];
        assert!(
            super::parse_shell(&arguments).is_err(),
            "{host} was accepted as a device"
        );
    }
}

/// Missing the device entirely is the usage message, not a panic on an
/// empty slice.
#[test]
fn a_shell_without_a_device_says_how_to_spell_it() {
    for arguments in [
        Vec::new(),
        vec!["uname".to_owned()],
        vec!["192.168.1.2".to_owned()],
    ] {
        let error = super::parse_shell(&arguments).expect_err("no device was named");
        assert!(error.starts_with("usage: kobo shell"), "{error}");
    }
}

/// `sh` is what a shell is called by the people most likely to want one.
#[test]
fn sh_is_another_name_for_shell() {
    assert_eq!(super::canonical("sh"), "shell");
}

#[test]
fn a_recording_decodes_to_the_frames_that_were_kept() {
    let raw = recording(2, 3, &[(0, 0xff), (500, 0x40)]);
    let (width, height, frames) = super::decode_recording(&raw).expect("decode");
    assert_eq!((width, height), (2, 3));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].millis, 0);
    assert_eq!(frames[1].millis, 500);
    assert_eq!(frames[1].grey, vec![0x40; 6]);
}

#[test]
fn every_grey_level_survives_the_round_trip() {
    // The panel is greyscale and the text on it is anti-aliased. A
    // recording that flattened the greys would look harsher than the
    // device and would be read as a rendering bug that is not there.
    let mut raw = b"KOBOCST1".to_vec();
    raw.extend_from_slice(&16_u32.to_le_bytes());
    raw.extend_from_slice(&16_u32.to_le_bytes());
    raw.extend_from_slice(&0_u32.to_le_bytes());
    let ramp: Vec<u8> = (0..=255_u8).step_by(1).take(256).collect();
    raw.extend_from_slice(&ramp);
    let (_, _, frames) = super::decode_recording(&raw).expect("decode");
    assert_eq!(frames[0].grey, ramp);
}

#[test]
fn half_a_frame_is_dropped_rather_than_shown_as_a_whole_one() {
    // The device can be unplugged or fill up mid-write. Half a frame
    // decoded as a whole one is a picture of nothing that looks exactly
    // like a rendering failure.
    let mut raw = recording(2, 3, &[(0, 0xff)]);
    raw.extend_from_slice(&99_u32.to_le_bytes());
    raw.extend_from_slice(&[0x10, 0x10]);
    let (_, _, frames) = super::decode_recording(&raw).expect("decode");
    assert_eq!(frames.len(), 1, "a torn frame was kept");
}

#[test]
fn something_that_is_not_a_recording_is_refused() {
    assert!(super::decode_recording(b"not a recording at all").is_err());
    assert!(super::decode_recording(&recording(2, 3, &[])).is_err());
}

/// The frames are not evenly spaced, because only the ones that moved were
/// kept. Handing them to ffmpeg at a fixed rate would play a screen that
/// was up for ten seconds in the same time as one that was up for a tenth
/// of one, which is a recording of a run that never happened.
#[test]
fn each_frame_is_played_for_as_long_as_it_was_really_on_the_panel() {
    let raw = recording(2, 3, &[(0, 0xff), (2_000, 0x40), (2_150, 0x10)]);
    let (_, _, frames) = super::decode_recording(&raw).expect("decode");
    assert_eq!(
        super::concat_list(&frames),
        "file 'frame-0000.png'\nduration 2.000\n\
             file 'frame-0001.png'\nduration 0.150\n\
             file 'frame-0002.png'\nduration 1.000\n\
             file 'frame-0002.png'\n",
        "the last frame is named twice because concat ignores the last duration"
    );
}

/// A frame that was replaced within a tenth of a second is still a frame
/// somebody has to be able to see. Played for its real duration it is one
/// or two hundredths of a second, which at any sane frame rate is a frame
/// the encoder drops entirely.
#[test]
fn a_screen_that_barely_appeared_is_still_held_long_enough_to_see() {
    let raw = recording(2, 3, &[(0, 0xff), (20, 0x40)]);
    let (_, _, frames) = super::decode_recording(&raw).expect("decode");
    assert!(
        super::concat_list(&frames).contains("duration 0.100"),
        "a frame was given less time than the encoder will keep"
    );
}

use super::package;
use super::{
    build_executables, canonical, configured_target_directory, dev_session_root, is_device_flag,
    manifest_uses_sdk, normalise_secret_value, parse_deploy, parse_devices, parse_logs,
    parse_touch_probe, parser_shelf_summary, unreachable_device, valid_device_host, valid_slug,
    verify_arm_elf, wait_for_remote_child, workspace_doctor_binary, DevSessionGuard,
    RemoteArtifact, SimulationGuard, ALIASES, DEFAULT_TRACE_LINES, DEPLOY_TIMEOUT, DEVICE_PACKAGES,
    TOUCH_PROBE_DEFAULT_SECONDS, TOUCH_PROBE_MAXIMUM_SECONDS,
};
#[cfg(feature = "device-write")]
use super::{
    parse_guard_test, parse_smoke_display, run, workspace_smoke_binary, RemoteArtifactSession,
    RemoteProgram, SmokeStage, GUARD_TEST_CHILD, GUARD_TEST_CONFIRMATION, REMOTE_CLEANUP_TIMEOUT,
    REMOTE_COMMAND_TIMEOUT, REMOTE_CONNECT_TIMEOUT_SECONDS, REMOTE_SMOKE_TIMEOUT_SECONDS,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[test]
fn secret_remote_publish_preserves_old_value_on_failed_or_occupied_stage() {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!(
        "cobalt-secret-script-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let script = super::secret_install_script("fixture", "new-value")
        .replace(super::DEVICE_SECRETS_DIRECTORY, directory.to_str().unwrap());
    let destination = directory.join("fixture");
    let partial = directory.join(".fixture.writing");
    fs::write(&destination, "old-value").unwrap();
    fs::write(&partial, "other-attempt").unwrap();
    let run = || Command::new("sh").arg("-c").arg(&script).output().unwrap();
    assert!(!run().status.success());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old-value");
    assert_eq!(fs::read_to_string(&partial).unwrap(), "other-attempt");
    fs::remove_file(&partial).unwrap();
    let bin = directory.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("cat"), "#!/bin/sh\nprintf partial\nexit 42\n").unwrap();
    fs::set_permissions(bin.join("cat"), fs::Permissions::from_mode(0o700)).unwrap();
    let failed = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old-value");
    assert!(!partial.exists());
    assert!(run().status.success());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "new-value\n");
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::remove_dir_all(directory).unwrap();
}

fn typed_copies_fixture(label: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "cobalt-secret-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (path, value) in [
        ("apps/lichess/lichess", "lip_rejected"),
        ("apps/chat/openai", "chat-key"),
        ("apps/calibre-web/servers/lichess", "server-bound"),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    directory
}

#[test]
fn setting_a_secret_from_a_computer_replaces_the_copy_typed_on_the_reader() {
    let directory = typed_copies_fixture("set");
    let script = super::secret_install_script("lichess", "lip_new")
        .replace(super::DEVICE_SECRETS_DIRECTORY, directory.to_str().unwrap());
    let run = || Command::new("sh").arg("-c").arg(&script).output().unwrap();

    let first = run();
    assert!(first.status.success(), "{first:?}");
    assert_eq!(super::typed_copies_removed(&first.stdout), 1);
    assert_eq!(
        fs::read_to_string(directory.join("lichess")).unwrap(),
        "lip_new\n"
    );
    assert!(!directory.join("apps/lichess/lichess").exists());
    assert!(directory.join("apps/chat/openai").exists());
    assert!(directory.join("apps/calibre-web/servers/lichess").exists());

    let again = run();
    assert!(again.status.success(), "{again:?}");
    assert_eq!(super::typed_copies_removed(&again.stdout), 0);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn removing_a_secret_from_a_computer_also_removes_the_copy_typed_on_the_reader() {
    let directory = typed_copies_fixture("remove");
    fs::write(directory.join("lichess"), "lip_owner").unwrap();
    let script = super::secret_remove_script("lichess")
        .replace(super::DEVICE_SECRETS_DIRECTORY, directory.to_str().unwrap());

    let output = Command::new("sh").arg("-c").arg(&script).output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(super::typed_copies_removed(&output.stdout), 1);
    assert!(!directory.join("lichess").exists());
    assert!(!directory.join("apps/lichess/lichess").exists());
    assert!(directory.join("apps/chat/openai").exists());
    assert!(directory.join("apps/calibre-web/servers/lichess").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_mounted_reader_loses_only_typed_copies_of_the_named_secret() {
    let directory = typed_copies_fixture("volume");

    assert_eq!(super::forget_typed_copies(&directory, "lichess"), Ok(1));
    assert_eq!(super::forget_typed_copies(&directory, "lichess"), Ok(0));
    assert_eq!(super::forget_typed_copies(&directory, "servers"), Ok(0));
    assert!(directory.join("apps/chat/openai").exists());
    assert!(directory.join("apps/calibre-web/servers/lichess").exists());
    let empty = directory.join("no-apps-yet");
    assert_eq!(super::forget_typed_copies(&empty, "lichess"), Ok(0));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn secret_files_accept_raw_and_assignment_forms() {
    assert_eq!(normalise_secret_value("sk-secret"), "sk-secret");
    assert_eq!(
        normalise_secret_value("EXA_API_KEY='exa-secret'"),
        "exa-secret"
    );
    assert_eq!(normalise_secret_value("token=still=raw"), "token=still=raw");
}

#[test]
fn a_log_request_reads_the_way_adb_logcat_does() {
    let arguments = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    let parse = |parts: &[&str]| {
        parse_logs(&arguments(parts)).map(|request| (request.follow, request.lines, request.clear))
    };
    // Watching is the point, so it is what asking for nothing gets.
    assert_eq!(
        parse(&["--device", "192.168.1.15"]),
        Ok((true, DEFAULT_TRACE_LINES, false))
    );
    // -s is adb's spelling of the same flag and has to reach the same code.
    assert_eq!(
        parse(&["-s", "192.168.1.15", "-d"]),
        Ok((false, DEFAULT_TRACE_LINES, false))
    );
    assert_eq!(
        parse(&["--device", "host", "-t", "40"]),
        Ok((true, 40, false))
    );
    assert_eq!(
        parse(&["--device", "host", "--lines", "40", "--dump"]),
        Ok((false, 40, false))
    );
    // Clearing alone clears and stops; asked for with --follow it clears
    // and then watches, whichever order the two were written in.
    assert_eq!(
        parse(&["--device", "host", "-c"]),
        Ok((false, DEFAULT_TRACE_LINES, true))
    );
    assert_eq!(
        parse(&["--device", "host", "-c", "-f"]),
        Ok((true, DEFAULT_TRACE_LINES, true))
    );
    assert_eq!(
        parse(&["--device", "host", "-f", "-c"]),
        Ok((true, DEFAULT_TRACE_LINES, true))
    );
    for rejected in [
        // A host that could carry a second command into the remote shell.
        vec!["--device", "192.168.1.15; reboot"],
        vec!["--device"],
        vec!["--device", "host", "--lines"],
        vec!["--device", "host", "--lines", "0"],
        vec!["--device", "host", "--lines", "10001"],
        vec!["--device", "host", "--nonsense"],
        vec!["host"],
    ] {
        assert!(
            parse(&rejected).is_err(),
            "{rejected:?} should not be accepted"
        );
    }
}

#[test]
fn names_from_other_mobile_toolchains_reach_the_command_they_mean() {
    // Somebody who has shipped for Android should not have to learn a new
    // word for the same idea before they can read a log.
    assert_eq!(canonical("logcat"), "logs");
    assert_eq!(canonical("install"), "deploy");
    assert_eq!(canonical("wait-for-device"), "wait");
    assert_eq!(canonical("sim"), "dev");
    assert_eq!(canonical("init"), "new");
    // A canonical name is left exactly as it is, and so is a name nobody
    // knows, so an unknown command still reports itself rather than an
    // alias it was silently turned into.
    assert_eq!(canonical("logs"), "logs");
    assert_eq!(canonical("nonsense"), "nonsense");
    for (alias, name) in ALIASES {
        assert_ne!(alias, name, "an alias for itself is dead weight");
        assert_eq!(canonical(name), *name, "aliases must not chain");
    }
    assert!(is_device_flag("--device"));
    assert!(is_device_flag("-s"));
    assert!(!is_device_flag("--devices"));
}

#[test]
fn a_touch_probe_window_is_bounded_and_the_host_waits_longer_than_the_device() {
    let arguments = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        parse_touch_probe(&arguments(&["--device", "192.168.1.15"])),
        Ok(("192.168.1.15", TOUCH_PROBE_DEFAULT_SECONDS))
    );
    for rejected in [
        vec!["--device", "192.168.1.15", "--seconds", "0"],
        vec!["--device", "192.168.1.15", "--seconds", "121"],
        vec!["--device", "192.168.1.15", "--seconds", "ten"],
        vec!["--device", "192.168.1.15; reboot"],
        vec!["--device"],
    ] {
        assert!(
            parse_touch_probe(&arguments(&rejected)).is_err(),
            "{rejected:?} must be refused"
        );
    }
    // The device enforces its own bound, so the host must outlast it.
    let artifact = RemoteArtifact::touch_probe(TOUCH_PROBE_MAXIMUM_SECONDS);
    assert!(artifact.timeout().as_secs() > TOUCH_PROBE_MAXIMUM_SECONDS + 15);
}

/// A sweep builds addresses by appending a host part, so anything that is
/// not exactly three octets would produce addresses nobody asked for.
#[test]
fn a_sweep_is_confined_to_one_named_subnet() {
    let arguments = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        parse_devices(&arguments(&["--subnet", "192.168.1"])),
        Ok(("192.168.1".to_owned(), false))
    );
    assert_eq!(
        parse_devices(&arguments(&["--json", "--subnet", "192.168.1"])),
        Ok(("192.168.1".to_owned(), true))
    );
    for rejected in [
        vec!["--subnet", "192.168.1.10"],
        vec!["--subnet", "192.168"],
        vec!["--subnet", "192.168.1; reboot"],
        vec!["--subnet", "$(hostname)"],
        vec!["--subnet"],
        vec!["--subnet", "192.168.1", "--extra"],
        vec!["192.168.1"],
    ] {
        assert!(
            parse_devices(&arguments(&rejected)).is_err(),
            "{rejected:?} must be refused"
        );
    }
    // With no argument the subnet comes from this machine's own route, and
    // a machine with no route has nothing to scan rather than a default.
    assert_eq!(
        parse_devices(&[]).is_ok(),
        super::connect::local_subnet().is_some()
    );
}

#[test]
fn a_deploy_names_one_host_and_at_most_one_package() {
    let arguments = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        parse_deploy(&arguments(&["--device", "192.168.1.15"])),
        Ok(("192.168.1.15", None))
    );
    assert_eq!(
        parse_deploy(&arguments(&[
            "--device",
            "192.168.1.15",
            "--package",
            "target/KoboRoot.tgz"
        ])),
        Ok(("192.168.1.15", Some(PathBuf::from("target/KoboRoot.tgz"))))
    );
    for rejected in [
        vec!["--device", "192.168.1.15; reboot"],
        vec!["--device", ""],
        vec!["--device"],
        vec!["--device", "192.168.1.15", "--package"],
        vec!["--device", "192.168.1.15", "--out", "somewhere"],
        vec!["--package", "target/KoboRoot.tgz"],
        vec![],
    ] {
        assert!(
            parse_deploy(&arguments(&rejected)).is_err(),
            "{rejected:?} must be refused"
        );
    }
    // Six and a half megabytes of base64 through one stdin pipe took about
    // ten seconds on the device, so the budget has to be far larger.
    assert!(DEPLOY_TIMEOUT.as_secs() >= 180);
}

/// The checklist is the whole value of these messages, so it has to
/// survive being attached to an error rather than replacing one.
#[test]
fn an_unreachable_device_keeps_its_error_and_gains_the_checklist() {
    let reported = unreachable_device("device 192.168.1.15 did not answer".to_owned());
    assert_eq!(
        crate::console::category_of(&reported),
        crate::console::EXIT_TARGET
    );
    let shown = crate::console::display(&reported);
    assert!(shown.starts_with("device 192.168.1.15 did not answer"));
    assert!(shown.contains("kobo devices"));
    assert!(shown.contains("asleep"));
}

#[test]
fn exit_categories_hold_for_the_families_of_failure() {
    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }
    // A misspelling, a missing value and an unknown command are usage.
    assert_eq!(
        crate::console::category_of(&super::run(&arguments(&["nonsense"])).expect_err("unknown")),
        crate::console::EXIT_USAGE
    );
    for args in [
        vec!["wait", "--device"],
        vec!["wait", "--sim"],
        vec!["doctor", "--sim"],
        vec!["devices", "--subnet"],
        vec!["devices", "--bogus"],
    ] {
        let error = super::run(&arguments(&args)).expect_err("refused");
        assert_eq!(
            crate::console::category_of(&error),
            crate::console::EXIT_USAGE,
            "{args:?} gave {error}"
        );
    }
    // A saved name that has no reader behind it is a target problem.
    let error = super::run(&arguments(&[
        "wait",
        "--reader",
        "nobody",
        "--timeout",
        "1",
    ]))
    .expect_err("no such reader");
    assert_eq!(
        crate::console::category_of(&error),
        crate::console::EXIT_TARGET
    );
    assert!(
        crate::console::display(&error).contains("nobody"),
        "{error}"
    );
    // A command this build lacks is unsupported, not a generic failure.
    #[cfg(not(feature = "device-write"))]
    {
        let error = super::run(&arguments(&["guard-test"])).expect_err("compiled out");
        assert_eq!(
            crate::console::category_of(&error),
            crate::console::EXIT_UNSUPPORTED
        );
        assert!(crate::console::display(&error).contains("not compiled in"));
    }
}

#[test]
fn wait_and_doctor_take_the_shared_target_flags() {
    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }
    assert_eq!(
        super::parse_wait(&arguments(&["--device", "192.0.2.10", "--timeout", "5"])).unwrap(),
        ("192.0.2.10".to_owned(), Duration::from_secs(5))
    );
    // The adb spelling still works, wherever it appears.
    assert_eq!(
        super::parse_wait(&arguments(&["--timeout", "5", "-s", "192.0.2.11"])).unwrap(),
        ("192.0.2.11".to_owned(), Duration::from_secs(5))
    );
    let error = super::parse_wait(&arguments(&["--device", "192.0.2.10", "--sim"]))
        .expect_err("two targets");
    assert!(error.starts_with("usage: kobo wait"), "{error}");
    assert!(super::parse_doctor(&arguments(&["--reader", "clara"])).is_err());
    assert_eq!(
        super::parse_doctor(&arguments(&["--device", "192.0.2.1"])).unwrap(),
        (Some("192.0.2.1".to_owned()), false)
    );
}

#[test]
fn app_names_are_shell_safe() {
    assert!(valid_slug("weather"));
    assert!(valid_slug("home-panel-2"));
    assert!(!valid_slug("../bad"));
    assert!(!valid_slug("Bad"));
    assert!(!valid_slug("bad;rm"));
}

#[test]
fn rejects_non_elf_binary() {
    let path = std::env::temp_dir().join(format!("kobo-cli-not-elf-{}", std::process::id()));
    fs::write(&path, b"not an elf").expect("write fixture");
    assert!(verify_arm_elf(&path).is_err());
    fs::remove_file(path).expect("remove fixture");
}

#[test]
fn every_uploaded_artifact_is_built_from_this_workspace() {
    let command =
        super::device_build_command("kobo-doctor", None).expect("create doctor build command");
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        [
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            &super::workspace_manifest().display().to_string(),
            "--target",
            "armv7-unknown-linux-musleabihf",
            "-p",
            "kobo-doctor",
            "--bin",
            "kobo-doctor",
        ]
    );
}

#[test]
fn every_installed_package_is_a_member_of_this_workspace() {
    let manifest = fs::read_to_string(super::workspace_manifest()).expect("read the workspace");
    for (name, _) in super::INSTALLED_PACKAGES {
        let directory = if *name == "kobod" {
            "crates/kobod".to_owned()
        } else {
            format!("examples/{}", name.trim_start_matches("kobo-"))
        };
        assert!(
            manifest.contains(&format!("\"{directory}\"")),
            "{name} is packaged but {directory} is not a workspace member"
        );
    }
}

/// The daemon shipped in the package was built without `device-write` for
/// as long as the packager existed, so `--present` was not compiled in and
/// `start.sh` answered the owner with a usage message. Everything else
/// about that binary was correct, which is why nothing else caught it.
#[test]
fn every_packaged_binary_is_built_with_what_it_needs() {
    let features = super::INSTALLED_PACKAGES
        .iter()
        .find(|(name, _)| *name == "kobod")
        .map(|(_, features)| *features)
        .expect("kobod is packaged");
    assert_eq!(
        features,
        Some("device-write"),
        "a kobod without device-write cannot take the panel, and start.sh is \
             the only thing in the package an owner runs"
    );
    let command = super::device_build_command("kobod", features).expect("build command");
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair[0] == "--features"
                && pair[1].split(',').any(|one| one == "device-write")),
        "the build command dropped the feature: {arguments:?}"
    );
}

#[test]
fn a_daemon_without_the_panel_session_is_refused() {
    let path = std::path::Path::new("target/kobod");
    super::verify_present_is_compiled_in(b"nothing useful in here", path)
        .expect_err("a binary without the unlock phrase is not shippable");
    let mut bytes = b"padding".to_vec();
    bytes.extend_from_slice(super::PRESENT_UNLOCK_PHRASE);
    bytes.extend_from_slice(b"more padding");
    super::verify_present_is_compiled_in(&bytes, path)
        .expect("a binary carrying the unlock phrase is shippable");
}

#[test]
fn a_package_writes_nothing_outside_the_install_root() {
    // The archive is extracted as root by the device's own boot script, so
    // the check that matters is what it is *able* to write.
    let members = vec![
        package::Member {
            path: package::LAUNCH_BOOTSTRAP.to_owned(),
            bytes: super::bootstrap::CONTENT.as_bytes().to_vec(),
            program: true,
        },
        super::text_member("start.sh", super::START_SCRIPT, true),
        super::text_member("README.txt", super::INSTALL_README, false),
    ];
    let archive = package::tar(&members).expect("build the archive");
    let (readback, listed) =
        super::validated_release_archive(&archive).expect("high-level validation");
    assert_eq!(readback, members);
    assert_eq!(listed[0].path, package::LAUNCH_BOOTSTRAP);
}

#[test]
fn high_level_readback_requires_exact_first_unique_bootstrap() {
    let reviewed = super::bootstrap::CONTENT.as_bytes().to_vec();
    let version = (
        format!("{}/VERSION", package::INSTALL_ROOT),
        b"0.1.0\n".to_vec(),
        0o644,
    );
    let cases = [
        package::archive(
            &[],
            &[
                (
                    package::LAUNCH_BOOTSTRAP.to_owned(),
                    reviewed.clone(),
                    0o700,
                ),
                version.clone(),
            ],
        ),
        package::archive(
            &[],
            &[
                version.clone(),
                (
                    package::LAUNCH_BOOTSTRAP.to_owned(),
                    reviewed.clone(),
                    0o755,
                ),
            ],
        ),
        package::archive(
            &[],
            &[
                (
                    package::LAUNCH_BOOTSTRAP.to_owned(),
                    reviewed.clone(),
                    0o755,
                ),
                (
                    package::LAUNCH_BOOTSTRAP.to_owned(),
                    reviewed.clone(),
                    0o755,
                ),
                version.clone(),
            ],
        ),
        package::archive(
            &[],
            &[
                (
                    package::LAUNCH_BOOTSTRAP.to_owned(),
                    b"changed".to_vec(),
                    0o755,
                ),
                version.clone(),
            ],
        ),
        package::archive(
            &[(package::LAUNCH_BOOTSTRAP, 0o755)],
            &[(
                format!("{}/VERSION", package::INSTALL_ROOT),
                b"0.1.0\n".to_vec(),
                0o644,
            )],
        ),
    ];
    for archive in cases {
        assert!(
            super::validated_release_archive(&archive).is_err(),
            "malformed standalone bootstrap passed high-level readback"
        );
    }
}

#[test]
fn deploy_validation_accepts_then_omits_reviewed_bootstrap() {
    let members = vec![
        package::Member {
            path: package::LAUNCH_BOOTSTRAP.to_owned(),
            bytes: super::bootstrap::CONTENT.as_bytes().to_vec(),
            program: true,
        },
        super::text_member("VERSION", "0.1.0\n", false),
    ];
    let compressed = super::gzip(&package::tar(&members).expect("archive")).expect("gzip");
    let folder = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!("deploy-validation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("scratch");
    let path = folder.join("package.tgz");
    std::fs::write(&path, compressed).expect("package");

    let (uploaded, count) = super::validated_package(&path).expect("validated deploy package");
    let uploaded = super::gunzip(&uploaded).expect("uploaded archive");
    let listed = package::list(&uploaded).expect("uploaded listing");
    assert_eq!(count, 1);
    assert!(listed
        .iter()
        .all(|entry| entry.path != package::LAUNCH_BOOTSTRAP));
    assert!(listed.iter().all(|entry| {
        let path = std::path::Path::new(entry.path.trim_end_matches('/'));
        let root = std::path::Path::new(package::INSTALL_ROOT);
        path.starts_with(root) || root.starts_with(path)
    }));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_package_survives_the_check_the_device_runs_first() {
    let members = vec![super::text_member("VERSION", "0.1.0\n", false)];
    let archive = package::tar(&members).expect("build the archive");
    let compressed = super::gzip(&archive).expect("compress");
    super::gzip_test(&compressed).expect("the device would accept this");
    assert_eq!(
        super::gunzip(&compressed).expect("decompress"),
        archive,
        "compression must round-trip exactly"
    );
    assert_eq!(
        super::gzip(&archive).expect("compress again"),
        compressed,
        "the same input must produce the same file, or a checksum means nothing"
    );
}

#[test]
fn the_start_script_points_at_the_folder_the_package_writes() {
    assert!(super::START_SCRIPT.contains(&format!("/{}", package::INSTALL_ROOT)));
    assert!(super::INSTALL_README.contains(&format!("/{}", package::INSTALL_ROOT)));
}

#[test]
fn the_start_script_installs_the_staged_public_key_once() {
    let script = super::START_SCRIPT;
    // The home directory is read from /etc/passwd rather than assumed to
    // be /root: the i.MX6 firmware gives root a home of `/`, and a key
    // installed under /root never authenticates there.
    assert!(script.contains("awk -F: '$1 == \"root\" { print $6 }' /etc/passwd"));
    assert!(script.contains("keys=\"$home/.ssh/authorized_keys\""));
    assert!(script.contains("while IFS= read -r known"));
    assert!(script.contains("if [ \"$known\" = \"$key\" ]"));
    assert!(script.contains("printf '%s\\n' \"$key\" >> \"$keys\""));
    assert!(script.contains("rm -f \"$staged_key\""));
}

#[test]
fn package_options_are_parsed_and_unknown_ones_refused() {
    let (tarball, folder) = super::parse_package(&[]).expect("defaults");
    assert_eq!(tarball, PathBuf::from("target/KoboRoot.tgz"));
    assert!(folder.is_none());
    let (tarball, folder) = super::parse_package(&[
        "--out".to_owned(),
        "/tmp/a.tgz".to_owned(),
        "--folder".to_owned(),
        "/tmp/b".to_owned(),
    ])
    .expect("explicit paths");
    assert_eq!(tarball, PathBuf::from("/tmp/a.tgz"));
    assert_eq!(folder, Some(PathBuf::from("/tmp/b")));
    assert!(super::parse_package(&["--onto".to_owned()]).is_err());
}

/// That the template compiles is settled by `examples/hello` being a
/// workspace member, which is the whole reason it is one. What is left to
/// check here is that it still teaches the right things and that the
/// front matter naming it a template does not follow it out the door.
#[test]
fn the_generated_app_teaches_the_contract_it_should() {
    let source = super::generated_app_source();

    // The loop belongs to the SDK. An application that hand-rolls one
    // breaks the next time the event enum grows, which is how the
    // previous template died.
    assert!(source.contains("kobo_sdk::run("));
    assert!(!source.contains("next_event"));

    // Hardware is asked for, and every answer including a refusal is
    // shown. Both halves are the point of the example.
    assert!(source.contains("context.device().read_battery()"));
    assert!(source.contains("fn on_device_result"));
    assert!(source.contains("DeviceResult::Denied(reason)"));

    // It must never reach hardware itself.
    assert!(!source.contains("/dev/"));
    assert!(!source.contains("/sys/"));

    // It arrives as somebody's own application, not as a copy of a file
    // that describes itself as a template.
    assert!(
        source.starts_with("use kobo_sdk::prelude::*;"),
        "{}",
        &source[..80]
    );
    assert!(!source.contains("//!"));
    assert!(source.ends_with('\n'));
}

#[test]
fn detects_sdk_application_manifests() {
    assert!(manifest_uses_sdk(
        "[dependencies]\nkobo-sdk = { path = \"../kobo-sdk\" }"
    ));
    assert!(manifest_uses_sdk(
        "[dependencies]\nkobo-sdk.workspace = true"
    ));
    assert!(!manifest_uses_sdk("[dependencies]\nkobo-ui = \"0.1\""));
}

#[test]
fn finds_executable_from_cargo_build_output() {
    let output = concat!(
        r#"{"reason":"compiler-artifact","target":{"kind":["lib"]},"executable":null}"#,
        "\n",
        r#"{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"/apps/hello/target/debug/hello"}"#
    );
    assert_eq!(
        build_executables(output),
        vec![std::path::PathBuf::from("/apps/hello/target/debug/hello")]
    );
}

#[test]
fn simulation_guard_removes_private_artifacts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!(".simulation-cleanup-{}", std::process::id()));
    let guard = SimulationGuard::new_at(root.clone()).expect("create simulation guard");
    fs::write(&guard.socket, b"socket").expect("write socket fixture");
    fs::write(&guard.frame, b"frame").expect("write frame fixture");
    drop(guard);
    assert!(!root.exists());
}

#[test]
fn dev_session_guard_removes_private_artifacts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!(".dev-cleanup-{}", std::process::id()));
    let guard = DevSessionGuard::new_at(root.clone()).expect("create development session");
    fs::write(&guard.socket, b"socket").expect("write socket fixture");
    drop(guard);
    assert!(!root.exists());
}

#[test]
fn a_parser_push_names_every_story_left_on_the_reader() {
    let shelf = [
        "zork.z3".to_owned(),
        ".lamplight.z5.12.writing".to_owned(),
        "lamplight.z5".to_owned(),
    ];
    assert_eq!(
        parser_shelf_summary(&shelf),
        "Parser now holds 2 stories: lamplight, zork."
    );
    assert_eq!(
        parser_shelf_summary(&shelf[..1]),
        "Parser now holds 1 story: zork."
    );
    assert_eq!(parser_shelf_summary(&[]), "Parser has no stories yet.");
}

#[test]
fn a_dev_session_under_a_deep_temporary_folder_keeps_its_socket_short_enough_to_bind() {
    let shallow = dev_session_root(std::path::Path::new("/tmp/cb"), "kobo-dev-1-2");
    assert_eq!(shallow, PathBuf::from("/tmp/cb/kobo-dev-1-2"));
    let deep = std::path::Path::new("/private/tmp").join("a-folder-name-long-enough".repeat(4));
    let root = dev_session_root(&deep, "kobo-dev-1-2");
    assert_eq!(root, PathBuf::from("/tmp/kobo-dev-1-2"));
    assert!(root.join("app.sock").as_os_str().len() <= 100);
}

#[test]
fn default_device_build_excludes_guard_and_smoke() {
    assert_eq!(
        DEVICE_PACKAGES,
        ["kobo-doctor", "kobod", "kobo-todo", "kobo-terminal"]
    );
    assert!(!DEVICE_PACKAGES.contains(&"kobo-guard"));
    assert!(!DEVICE_PACKAGES.contains(&"kobo-smoke"));
}

#[cfg(feature = "device-write")]
#[test]
fn the_guard_test_needs_the_exact_confirmation_and_a_clean_host() {
    let arguments = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        parse_guard_test(&arguments(&[
            "--device",
            "192.168.1.15",
            "--confirm",
            GUARD_TEST_CONFIRMATION
        ])),
        Ok("192.168.1.15")
    );
    for rejected in [
        vec!["--device", "192.168.1.15", "--confirm", "GUARD_RESTORE"],
        vec!["--device", "192.168.1.15", "--confirm", ""],
        vec!["--device", "192.168.1.15"],
        vec![
            "--device",
            "192.168.1.15; reboot",
            "--confirm",
            GUARD_TEST_CONFIRMATION,
        ],
    ] {
        assert!(
            parse_guard_test(&arguments(&rejected)).is_err(),
            "{rejected:?} must be refused"
        );
    }
}

#[cfg(feature = "device-write")]
#[test]
fn the_guard_artifact_is_built_from_this_workspace_and_never_the_default_device_set() {
    let artifact = RemoteArtifact::guard();
    assert_eq!(artifact.package, "kobo-guard");
    assert_eq!(artifact.features, Some("device-write"));
    assert!(artifact
        .local_binary
        .ends_with("armv7-unknown-linux-musleabihf/release/kobo-guard"));
    // The child is an exact absolute path, never resolved through PATH.
    assert!(GUARD_TEST_CHILD.starts_with('/'));
}

#[cfg(feature = "device-write")]
#[test]
fn existing_run_command_cannot_invoke_the_smoke_binary() {
    let error = run(&["run".to_owned(), "kobo-smoke".to_owned()]).expect_err("run is gated");
    assert!(error.contains("device execution is safety-gated"));
}

#[test]
fn remote_doctor_uses_strict_hosts_and_workspace_artifact() {
    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }
    assert_eq!(
        super::parse_doctor(&arguments(&["--json", "--device", "192.0.2.1"])),
        Ok((Some("192.0.2.1".to_owned()), true))
    );
    for invalid in [
        vec!["--json", "--json"],
        vec!["--device"],
        vec!["--device", "reader;reboot"],
        vec!["--surprise"],
    ] {
        assert!(super::parse_doctor(&arguments(&invalid)).is_err());
    }
    assert!(valid_device_host("192.0.2.1"));
    assert!(valid_device_host("kobo-reader_1"));
    assert!(!valid_device_host(""));
    assert!(!valid_device_host("--sim"));
    assert!(!valid_device_host("-reader"));
    assert!(!valid_device_host("reader;reboot"));
    assert!(!valid_device_host("reader name"));
    assert_eq!(
        workspace_doctor_binary(),
        super::workspace_target_directory()
            .join("armv7-unknown-linux-musleabihf/release/kobo-doctor")
    );
    #[cfg(feature = "device-write")]
    assert_eq!(
        workspace_smoke_binary(),
        super::workspace_target_directory()
            .join("armv7-unknown-linux-musleabihf/release/kobo-smoke")
    );
}

#[test]
fn cargo_target_dir_resolves_exactly_like_the_build_invocation() {
    let workspace = PathBuf::from("/source/cobalt");
    let invocation = PathBuf::from("/runner/jobs/package");
    let relative = std::ffi::OsStr::new("../../cobalt-targets");
    let resolved = configured_target_directory(&workspace, &invocation, Some(relative));
    assert_eq!(resolved, invocation.join(relative));
    assert_ne!(resolved, workspace.join(relative));
    assert_eq!(
        resolved.join("armv7-unknown-linux-musleabihf/release/kobod"),
        invocation
            .join(relative)
            .join("armv7-unknown-linux-musleabihf/release/kobod")
    );
    assert_eq!(
        configured_target_directory(&workspace, &invocation, None),
        workspace.join("target")
    );
    assert_eq!(
        configured_target_directory(
            &workspace,
            &invocation,
            Some(std::ffi::OsStr::new("/external/cobalt-targets"))
        ),
        PathBuf::from("/external/cobalt-targets")
    );
}

#[cfg(feature = "device-write")]
#[test]
fn smoke_confirmation_is_exact_and_has_no_arbitrary_arguments() {
    let exact = [
        "--device".to_owned(),
        "192.0.2.1".to_owned(),
        "--confirm".to_owned(),
        "DISPLAY_ONLY_GC16".to_owned(),
    ];
    assert_eq!(
        parse_smoke_display(&exact),
        Ok(("192.0.2.1", SmokeStage::DisplayOnly))
    );
    let reversible = [
        "--device".to_owned(),
        "192.0.2.1".to_owned(),
        "--confirm".to_owned(),
        "REVERSIBLE_PIXELS_GC16".to_owned(),
    ];
    assert_eq!(
        parse_smoke_display(&reversible),
        Ok(("192.0.2.1", SmokeStage::ReversiblePixels))
    );
    for invalid in [
        vec![],
        vec!["--device", "192.0.2.1", "--confirm", "display_only_gc16"],
        vec!["--device", "192.0.2.1", "--confirm", "FULL_SCREEN_GC16"],
        vec![
            "--device",
            "192.0.2.1",
            "--confirm",
            "DISPLAY_ONLY_GC16",
            "--extra",
        ],
        vec![
            "--device",
            "reader;reboot",
            "--confirm",
            "DISPLAY_ONLY_GC16",
        ],
    ] {
        let invalid = invalid.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(parse_smoke_display(&invalid).is_err());
    }
}

#[test]
fn dev_session_parsing_is_exact_and_host_checked() {
    use super::{devsession::Switch, DevSessionAction};
    let base = ["--device".to_owned(), "192.0.2.1".to_owned()];
    let parse = |extra: &[&str]| {
        let mut arguments = base.to_vec();
        arguments.extend(extra.iter().map(|value| (*value).to_owned()));
        super::parse_dev_session(&arguments).map(|(host, action)| (host.to_owned(), action))
    };
    assert_eq!(
        parse(&[]),
        Ok(("192.0.2.1".to_owned(), DevSessionAction::Status))
    );
    assert_eq!(
        parse(&["--status"]),
        Ok(("192.0.2.1".to_owned(), DevSessionAction::Status))
    );
    assert_eq!(
        parse(&["--keep-awake", "on"]),
        Ok((
            "192.0.2.1".to_owned(),
            DevSessionAction::KeepAwake(Switch::On)
        ))
    );
    assert_eq!(
        parse(&["--wifi-always-on", "off"]),
        Ok((
            "192.0.2.1".to_owned(),
            DevSessionAction::WifiAlwaysOn(Switch::Off)
        ))
    );
    assert_eq!(
        parse(&["--restore-reader-config"]),
        Ok(("192.0.2.1".to_owned(), DevSessionAction::RestoreConfig))
    );
    for invalid in [
        vec!["--keep-awake"],
        vec!["--keep-awake", "yes"],
        vec!["--wifi-always-on", "1"],
        vec!["--unknown"],
        vec!["--status", "--keep-awake", "on"],
    ] {
        assert!(parse(&invalid).is_err(), "{invalid:?} must be rejected");
    }
    let hostile = [
        "--device".to_owned(),
        "reader;reboot".to_owned(),
        "--status".to_owned(),
    ];
    assert!(super::parse_dev_session(&hostile).is_err());
    assert!(super::parse_dev_session(&[]).is_err());
}

#[cfg(feature = "device-write")]
#[test]
fn each_stage_maps_to_exactly_one_device_unlock() {
    assert_eq!(
        SmokeStage::DisplayOnly.device_unlock(),
        "OWNER_ATTENDED_DISPLAY_ONLY_GC16"
    );
    assert_eq!(
        SmokeStage::ReversiblePixels.device_unlock(),
        "OWNER_ATTENDED_REVERSIBLE_PIXELS_GC16"
    );
}

#[cfg(feature = "device-write")]
#[test]
fn smoke_build_is_pinned_to_this_workspace_and_feature_targeted() {
    let command = super::device_build_command("kobo-smoke", Some("device-write"))
        .expect("create smoke build command");
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        [
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            &super::workspace_manifest().display().to_string(),
            "--target",
            "armv7-unknown-linux-musleabihf",
            "-p",
            "kobo-smoke",
            "--bin",
            "kobo-smoke",
            "--features",
            "device-write",
        ]
    );
    assert!(super::workspace_manifest().is_file());
}

#[cfg(feature = "device-write")]
#[test]
fn remote_session_uses_stdin_only_and_fixed_safe_artifacts() {
    let ssh = super::remote_shell_command("root@192.0.2.1");
    let ssh_args = ssh
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        &ssh_args[..5],
        ["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]
    );
    assert!(matches!(ssh_args.len(), 6 | 10), "{ssh_args:?}");
    if ssh_args.len() == 10 {
        assert_eq!(&ssh_args[5..8], ["-o", "IdentitiesOnly=yes", "-i"]);
        assert_eq!(
            PathBuf::from(&ssh_args[8])
                .file_name()
                .and_then(|name| name.to_str()),
            Some(super::DEVICE_KEY_NAME)
        );
    }
    assert_eq!(ssh_args.last().expect("remote host"), "root@192.0.2.1");
    let checksum = "a".repeat(64);
    let encoded = super::base64_encode(b"fixed artifact");
    let session = RemoteArtifactSession {
        directory: "/tmp/kobo-smoke-123-456".to_owned(),
        binary: "/tmp/kobo-smoke-123-456/kobo-smoke".to_owned(),
        owner_file: "/tmp/kobo-smoke-123-456/.kobo-smoke-owner".to_owned(),
        owner_token: "0123456789abcdef0123456789abcdef".to_owned(),
    };
    let script = super::remote_fixed_artifact_script(
        &session,
        &RemoteProgram::Smoke(SmokeStage::DisplayOnly),
        &checksum,
        &encoded,
    );
    assert!(script.starts_with("set -eu\numask 077\n"));
    assert!(script.contains("mkdir -m 700 \"$dir\""));
    assert!(script.contains("trap cleanup EXIT HUP INT TERM"));
    assert!(script.contains("IFS= read -r actual < \"$owner\""));
    assert!(script.contains("[ \"$actual\" = \"$token\" ]"));
    assert!(
        script.find("mkdir -m 700").expect("mkdir") < script.find("trap cleanup").expect("trap")
    );
    assert!(script.contains("rm -f \"$bin\""));
    assert!(script.contains("rmdir \"$dir\""));
    assert!(script.contains("base64 -d > \"$bin\" <<'KOBO_ARTIFACT_BASE64'"));
    assert!(script.contains(&encoded));
    assert!(script.contains(&checksum));
    assert!(script.contains("KOBO_SMOKE_UNLOCK='OWNER_ATTENDED_DISPLAY_ONLY_GC16'"));
    assert!(script.contains("[ -x /usr/bin/timeout ]"));
    assert!(script.contains("/usr/bin/timeout 25 \"$bin\""));
    assert!(script.contains("refusing display smoke"));
    assert!(!script.contains("192.0.2.1"));
    assert!(!script.contains("scp"));
    assert!(!script.contains("reader;reboot"));
    assert!(script.ends_with("exit\n"));
    let cleanup = super::remote_cleanup_script(&session);
    assert!(cleanup.contains("IFS= read -r actual < \"$owner\""));
    assert!(cleanup.contains("[ \"$actual\" = \"$token\" ]"));
    assert!(cleanup.contains("rm -f \"$bin\" \"$owner\""));
    assert!(cleanup.contains("rmdir \"$dir\""));
    assert_eq!(REMOTE_CONNECT_TIMEOUT_SECONDS, 10);
    assert_eq!(REMOTE_COMMAND_TIMEOUT.as_secs(), 60);
    assert_eq!(REMOTE_CLEANUP_TIMEOUT.as_secs(), 5);
    #[cfg(feature = "device-write")]
    {
        assert_eq!(REMOTE_SMOKE_TIMEOUT_SECONDS, 25);
        assert!(REMOTE_COMMAND_TIMEOUT.as_secs() > REMOTE_SMOKE_TIMEOUT_SECONDS);
    }
    let token = super::remote_owner_token().expect("ownership token");
    assert_eq!(token.len(), 32);
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[cfg(feature = "device-write")]
#[test]
fn a_whole_tap_sequence_travels_in_one_upload_and_outlasts_its_own_waits() {
    // The point of the sequence. Every step must reach the device in one
    // script, and both timeouts must outlast the sleeping the device is
    // being asked to do, or the run is killed partway through a tour.
    let session = RemoteArtifactSession {
        directory: "/tmp/kobo-tap-123-456".to_owned(),
        binary: "/tmp/kobo-tap-123-456/kobo-tap".to_owned(),
        owner_file: "/tmp/kobo-tap-123-456/.kobo-tap-owner".to_owned(),
        owner_token: "0123456789abcdef0123456789abcdef".to_owned(),
    };
    let sequence = "1500:536,400 2000:80,80 2500:400,1380";
    let artifact = super::RemoteArtifact::tap(sequence.to_owned(), 6_000);
    let script = super::remote_fixed_artifact_script(
        &session,
        &artifact.program,
        &"a".repeat(64),
        &super::base64_encode(b"tap"),
    );
    assert!(script.contains(&format!("KOBO_TAP_POINT='{sequence}'")));
    assert!(script.contains("KOBO_TAP_UNLOCK='OWNER_ATTENDED_SYNTHETIC_TOUCH'"));
    // 6s of waiting, so the device-side bound is 36 and the host's is 66.
    assert!(script.contains("/usr/bin/timeout 36 \"$bin\""));
    assert!(artifact.timeout() > Duration::from_secs(6));
    assert!(artifact.timeout() > Duration::from_secs(36));
}

#[cfg(feature = "device-write")]
#[test]
fn a_step_is_checked_here_before_anything_is_built_or_uploaded() {
    assert_eq!(super::parse_tap_step("536,400"), Ok(0));
    assert_eq!(super::parse_tap_step("1500:536,400"), Ok(1500));
    assert!(super::parse_tap_step("536").is_err());
    assert!(super::parse_tap_step("soon:536,400").is_err());
    assert!(super::parse_tap_step("536,-4").is_err());
}

#[test]
fn base64_round_trips_artifact_bytes() {
    let bytes = (0_u8..58).collect::<Vec<_>>();
    let encoded = super::base64_encode(&bytes);
    assert!(encoded.contains('\n'));
    assert_eq!(decode_base64(&encoded), bytes);
}

#[test]
fn parser_push_validates_story_formats_before_contacting_a_reader() {
    for version in [3, 5, 8] {
        let mut story = vec![0; 64];
        story[0] = version;
        assert_eq!(super::validate_parser_story(&story), Ok(()));
    }
    assert!(super::validate_parser_story(b"Glul followed by bytes")
        .expect_err("Glulx must be refused")
        .contains("Glulx"));
    let mut unsupported = vec![0; 64];
    unsupported[0] = 6;
    assert!(super::validate_parser_story(&unsupported)
        .expect_err("v6 must be refused")
        .contains("version 6"));
}

#[test]
fn remote_session_error_includes_captured_output() {
    let message = super::remote_shell_error(
        "remote doctor failed".to_owned(),
        b"doctor stdout",
        b"doctor stderr",
    );
    assert!(message.contains("stdout: doctor stdout"));
    assert!(message.contains("stderr: doctor stderr"));
}

#[test]
fn remote_child_timeout_kills_the_local_process() {
    let mut child = Command::new("/bin/sleep")
        .arg("5")
        .spawn()
        .expect("start local sleep");
    let error = wait_for_remote_child(&mut child, "test remote command", Duration::from_millis(1))
        .expect_err("timeout");
    assert!(error.contains("timed out"));
    assert!(child.try_wait().expect("inspect child").is_some());
}

fn decode_base64(value: &str) -> Vec<u8> {
    fn digit(value: u8) -> u8 {
        match value {
            b'A'..=b'Z' => value - b'A',
            b'a'..=b'z' => value - b'a' + 26,
            b'0'..=b'9' => value - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("invalid base64 byte"),
        }
    }

    let compact = value
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    let mut decoded = Vec::new();
    for chunk in compact.chunks_exact(4) {
        let padding = usize::from(chunk[2] == b'=') + usize::from(chunk[3] == b'=');
        let value = (u32::from(digit(chunk[0])) << 18)
            | (u32::from(digit(chunk[1])) << 12)
            | (u32::from(if chunk[2] == b'=' { 0 } else { digit(chunk[2]) }) << 6)
            | u32::from(if chunk[3] == b'=' { 0 } else { digit(chunk[3]) });
        let bytes = value.to_be_bytes();
        decoded.push(bytes[1]);
        if padding < 2 {
            decoded.push(bytes[2]);
        }
        if padding == 0 {
            decoded.push(bytes[3]);
        }
    }
    decoded
}

mod holding {
    use super::super::{parse_dev_session, DevSessionAction, HOLD_MAXIMUM_MINUTES};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn defaults_to_thirty_minutes() {
        let given = arguments(&["--device", "192.0.2.1", "--hold"]);
        assert_eq!(
            parse_dev_session(&given).expect("parse").1,
            DevSessionAction::Hold(30)
        );
    }

    #[test]
    fn accepts_an_explicit_duration() {
        let given = arguments(&["--device", "192.0.2.1", "--hold", "90"]);
        assert_eq!(
            parse_dev_session(&given).expect("parse").1,
            DevSessionAction::Hold(90)
        );
    }

    #[test]
    fn refuses_a_zero_or_unbounded_hold() {
        // A hold must always end by itself, so it can never be forgotten.
        let zero = arguments(&["--device", "192.0.2.1", "--hold", "0"]);
        assert!(parse_dev_session(&zero).is_err());
        let too_long = (HOLD_MAXIMUM_MINUTES + 1).to_string();
        let over = arguments(&["--device", "192.0.2.1", "--hold", &too_long]);
        assert!(parse_dev_session(&over).is_err());
        let words = arguments(&["--device", "192.0.2.1", "--hold", "forever"]);
        assert!(parse_dev_session(&words).is_err());
    }
}

mod change_counting {
    use super::super::changed_lines;

    #[test]
    fn reads_the_reported_count() {
        assert_eq!(
            changed_lines(b"applied; changed_lines=3\nforce_wifi_on: true\n"),
            3
        );
        assert_eq!(changed_lines(b"applied; changed_lines=0\n"), 0);
    }

    #[test]
    fn treats_anything_unreadable_as_no_change() {
        // Advice may only be suppressed by this, never invented.
        assert_eq!(changed_lines(b""), 0);
        assert_eq!(changed_lines(b"applied; changed_lines=lots\n"), 0);
        assert_eq!(changed_lines(b"something else entirely\n"), 0);
        assert_eq!(changed_lines(&[0xff, 0xfe, 0x00]), 0);
    }
}

mod simulating {
    use super::super::simulated_package;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn without_an_app_it_runs_the_one_it_always_ran() {
        assert_eq!(simulated_package(&arguments(&[])), Ok("kobo-todo"));
    }

    #[test]
    fn an_app_can_be_named_the_way_the_launcher_names_it() {
        assert_eq!(
            simulated_package(&arguments(&["--app", "rss"])),
            Ok("kobo-rss")
        );
        assert_eq!(
            simulated_package(&arguments(&["-a", "gutenbird"])),
            Ok("kobo-gutenbird")
        );
        assert_eq!(
            simulated_package(&arguments(&["--app", "sudoku"])),
            Ok("kobo-sudoku")
        );
    }

    #[test]
    fn an_app_can_also_be_named_the_way_cargo_names_it() {
        assert_eq!(
            simulated_package(&arguments(&["--app", "kobo-hn"])),
            Ok("kobo-hn")
        );
    }

    #[test]
    fn the_runtime_is_not_an_application_to_run_against_itself() {
        // kobod is on the packages list and is the thing already being
        // started; asking for it would start two of them.
        assert!(simulated_package(&arguments(&["--app", "kobod"])).is_err());
    }

    #[test]
    fn a_name_that_is_not_an_app_is_refused_with_the_ones_that_are() {
        let error =
            simulated_package(&arguments(&["--app", "../../etc/passwd"])).expect_err("refused");
        assert!(error.contains("rss"), "{error}");
        assert!(error.contains("todo"), "{error}");
        let missing = simulated_package(&arguments(&["--app"])).expect_err("refused");
        assert!(missing.contains("needs a name"), "{missing}");
    }

    mod app_registry {
        use super::super::super::{
            contributed_store_packages, read_release_registry, workspace_manifest, STORE_PACKAGES,
        };
        use std::collections::BTreeSet;

        #[test]
        fn cobalt_owned_registry_entries_are_known_store_applications() {
            let registry = workspace_manifest()
                .parent()
                .expect("workspace root")
                .join("apps/catalog.json");
            let apps = read_release_registry(&registry).expect("registry");
            let registered = apps
                .iter()
                .map(|app| app.package.as_str())
                .collect::<BTreeSet<_>>();
            let known = STORE_PACKAGES.iter().copied().collect::<BTreeSet<_>>();
            assert!(
                    registered.is_subset(&known),
                    "Cobalt-owned base entries must remain presentable; third-party manifests are collected by tools/app-registry.mjs"
                );
            // Versions move with every release, so the check is that
            // each entry carries a version rather than which version it
            // carries. A pinned number here broke every routine catalog
            // bump while catching nothing the shape check misses.
            for app in &apps {
                let parts: Vec<&str> = app.version.split('.').collect();
                assert!(
                    parts.len() == 3
                        && parts.iter().all(|part| {
                            !part.is_empty() && part.chars().all(|digit| digit.is_ascii_digit())
                        }),
                    "{} version {:?} is not three dot-separated numbers",
                    app.id,
                    app.version
                );
            }
        }

        #[test]
        fn standalone_contributions_are_discovered_without_a_package_list_edit() {
            let contributed = contributed_store_packages()
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            for package in [
                "kobo-arxiv",
                "kobo-morse",
                "kobo-sudoku",
                "kobo-zotero-reader",
            ] {
                assert!(contributed.contains(package), "missing {package}");
            }
        }
    }
}

mod preparing {
    use super::super::{
        choose_reader_list, confirmation_answer, connect, dry_run_plan, gzip,
        load_release_package_from_manifest, parse_setup, setup, setup_device_with_confirmation,
        undo_setup, SetupMode, SetupPayload, SETUP_HELP,
    };
    use std::path::PathBuf;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// A reader that has never had NickelMenu on it.
    ///
    /// A real path rather than a made-up one, because the plan now asks
    /// the volume what is already installed. The first version of this
    /// pointed at /Volumes/KOBOeReader and quietly read whichever device
    /// happened to be plugged in, so the test passed or failed by what was
    /// on somebody's desk.
    fn fresh_reader() -> (setup::Mounted, TempVolume) {
        let volume = TempVolume::new("fresh");
        (mounted(volume.path.clone()), volume)
    }

    /// A reader that already has the plugin, which most do by the second
    /// run of this command.
    fn prepared_reader() -> (setup::Mounted, TempVolume) {
        let volume = TempVolume::new("prepared");
        let folder = volume.path.join(menu_config_folder());
        std::fs::create_dir_all(&folder).expect("the plugin folder");
        std::fs::write(folder.join("doc"), "nickelmenu").expect("the marker");
        (mounted(volume.path.clone()), volume)
    }

    fn menu_config_folder() -> &'static str {
        crate::menu::CONFIG_FOLDER
    }

    struct TempVolume {
        path: PathBuf,
    }

    impl TempVolume {
        fn new(name: &str) -> Self {
            let path = std::env::current_dir()
                .expect("working directory")
                .join("target")
                .join(format!(
                    "kobo-plan-{name}-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a volume");
            Self { path }
        }
    }

    impl Drop for TempVolume {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn mounted(volume: PathBuf) -> setup::Mounted {
        setup::Mounted {
            volume,
            serial: "N365410043013".to_owned(),
            firmware: "4.45.23697".to_owned(),
        }
    }

    #[test]
    fn a_bare_run_installs_ejects_and_waits() {
        let parsed = parse_setup(&arguments(&[])).expect("parse");
        assert_eq!(parsed.mode, SetupMode::Install);
        assert!(parsed.eject);
        assert!(parsed.wait);
        assert!(!parsed.dry_run);
        assert!(parsed.enable_ssh, "SSH is part of a normal setup now");
        assert!(parsed.authorize_key);
        assert!(!parsed.yes);
        assert!(!parsed.non_interactive);
    }

    #[test]
    fn automation_requires_an_explicit_confirmation() {
        let error =
            parse_setup(&arguments(&["--non-interactive"])).expect_err("confirmation required");
        assert!(error.contains("--yes"), "{error}");
        let parsed = parse_setup(&arguments(&["--non-interactive", "--yes"])).expect("confirmed");
        assert!(parsed.non_interactive);
        assert!(parsed.yes);
        for declined in ["", "n", "no", "later"] {
            assert!(!confirmation_answer(declined));
        }
        assert!(confirmation_answer("yes\n"));
    }

    #[test]
    fn declined_confirmation_writes_nothing() {
        let volume = TempVolume::new("declined");
        let system = volume.path.join(".kobo");
        std::fs::create_dir_all(&system).expect("system folder");
        std::fs::write(
            system.join("version"),
            "N365410043013,4.9.77,4.45.23697,4.9.77\n",
        )
        .expect("version");
        setup_device_with_confirmation(
            &arguments(&[
                "--volume",
                volume.path.to_str().expect("path"),
                "--source",
                "--no-eject",
            ]),
            |_, _, _, _| Ok(false),
        )
        .expect("declined cleanly");
        assert!(!volume.path.join(setup::INSTALL_FOLDER).exists());
        assert!(!volume.path.join(setup::SETTINGS).exists());
    }

    #[test]
    fn an_untested_model_reaches_the_confirmation_instead_of_a_refusal() {
        let volume = TempVolume::new("untested-model");
        let system = volume.path.join(".kobo");
        std::fs::create_dir_all(&system).expect("system folder");
        std::fs::write(
            system.join("version"),
            "N999000000000,4.1.15,4.38.23828,4.1.15\n",
        )
        .expect("version");
        let mut said = None;
        setup_device_with_confirmation(
            &arguments(&[
                "--volume",
                volume.path.to_str().expect("path"),
                "--source",
                "--no-eject",
            ]),
            |_, reader, identified, _| {
                said = identified.untested(reader);
                Ok(false)
            },
        )
        .expect("declined cleanly");
        let note = said.expect("the confirmation says what is untested");
        assert!(note.starts_with("untested model."), "{note}");
        assert!(!volume.path.join(setup::INSTALL_FOLDER).exists());
    }

    #[test]
    fn several_readers_are_refused_without_printing_full_serials() {
        let first = mounted(PathBuf::from("/Volumes/ONE"));
        let mut second = mounted(PathBuf::from("/Volumes/TWO"));
        second.serial = "N418999999999".to_owned();
        let error = choose_reader_list(vec![first, second]).expect_err("ambiguous");
        assert!(error.contains("2 readers"), "{error}");
        assert!(error.contains("/Volumes/ONE"), "{error}");
        assert!(!error.contains("N365410043013"), "{error}");
        assert!(!error.contains("N418999999999"), "{error}");
    }

    #[test]
    fn source_build_setup_remains_an_explicit_supported_path() {
        let parsed = parse_setup(&arguments(&["--source", "--dry-run"])).expect("source");
        assert!(parsed.source);
        assert!(parse_setup(&arguments(&["--source", "--release-dir", "release"])).is_err());
    }

    #[test]
    fn a_verified_prebuilt_device_package_becomes_direct_writer_members() {
        let release = TempVolume::new("prebuilt");
        std::fs::write(release.path.join("channel"), "stable\n").expect("channel");
        let members = vec![
            crate::package::Member {
                path: crate::package::LAUNCH_BOOTSTRAP.to_owned(),
                bytes: crate::bootstrap::CONTENT.as_bytes().to_vec(),
                program: true,
            },
            crate::package::Member {
                path: format!("{}/bin/kobod", crate::package::INSTALL_ROOT),
                bytes: b"prebuilt binary".to_vec(),
                program: true,
            },
            crate::package::Member {
                path: format!("{}/VERSION", crate::package::INSTALL_ROOT),
                bytes: format!("{}\n", env!("CARGO_PKG_VERSION")).into_bytes(),
                program: false,
            },
        ];
        let archive = crate::package::tar(&members).expect("archive");
        let compressed = gzip(&archive).expect("gzip");
        let asset_name = format!("cobalt-{}-KoboRoot.tgz", env!("CARGO_PKG_VERSION"));
        std::fs::write(release.path.join(&asset_name), &compressed).expect("package");
        let manifest = crate::host_release::Manifest {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            channels: vec!["stable".to_owned(), "beta".to_owned()],
            source: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            assets: vec![crate::host_release::Asset {
                kind: "device".to_owned(),
                platform: None,
                name: asset_name,
                bytes: compressed.len() as u64,
                sha256: crate::sha256::hex_digest(&compressed),
            }],
        };
        let mut damaged = manifest.clone();
        damaged.assets[0].sha256 = "0".repeat(64);
        let Err(error) = load_release_package_from_manifest(&release.path, damaged) else {
            panic!("bad checksum was accepted");
        };
        assert!(error.contains("checksum failed"), "{error}");
        let payload =
            load_release_package_from_manifest(&release.path, manifest.clone()).expect("prebuilt");
        match payload {
            SetupPayload::Prebuilt { built, channel, .. } => {
                assert_eq!(channel, "stable");
                assert_eq!(built.members, members);
            }
            SetupPayload::Source => panic!("prebuilt became source build"),
        }

        let invalid_archive = crate::package::archive(
            &[],
            &[
                (
                    crate::package::LAUNCH_BOOTSTRAP.to_owned(),
                    crate::bootstrap::CONTENT.as_bytes().to_vec(),
                    0o700,
                ),
                (
                    format!("{}/VERSION", crate::package::INSTALL_ROOT),
                    format!("{}\n", env!("CARGO_PKG_VERSION")).into_bytes(),
                    0o644,
                ),
            ],
        );
        let invalid_compressed = gzip(&invalid_archive).expect("invalid gzip");
        std::fs::write(
            release.path.join(&manifest.assets[0].name),
            &invalid_compressed,
        )
        .expect("invalid package");
        let mut invalid_manifest = manifest.clone();
        invalid_manifest.assets[0].bytes = invalid_compressed.len() as u64;
        invalid_manifest.assets[0].sha256 = crate::sha256::hex_digest(&invalid_compressed);
        let Err(error) = load_release_package_from_manifest(&release.path, invalid_manifest) else {
            panic!("wrong bootstrap mode passed prebuilt validation");
        };
        assert!(error.contains("first regular 0755"), "{error}");
        std::fs::write(release.path.join(&manifest.assets[0].name), &compressed)
            .expect("restore valid package");

        std::fs::write(release.path.join("channel"), "beta\n").expect("channel");
        let manifest = crate::host_release::Manifest {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            channels: vec!["stable".to_owned(), "beta".to_owned()],
            source: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            assets: vec![crate::host_release::Asset {
                kind: "device".to_owned(),
                platform: None,
                name: format!("cobalt-{}-KoboRoot.tgz", env!("CARGO_PKG_VERSION")),
                bytes: compressed.len() as u64,
                sha256: crate::sha256::hex_digest(&compressed),
            }],
        };
        let Err(error) = load_release_package_from_manifest(&release.path, manifest) else {
            panic!("beta USB package was accepted");
        };
        assert!(error.contains("stable releases only"), "{error}");
    }

    #[test]
    fn the_wait_can_be_declined() {
        let parsed = parse_setup(&arguments(&["--no-wait"])).expect("parse");
        assert!(!parsed.wait);
        assert!(
            parsed.eject,
            "declining the wait does not decline the eject"
        );
    }

    #[test]
    fn a_dry_run_of_an_undo_describes_the_undo_and_performs_nothing() {
        // This is the whole reason the two are one mode and not two flags.
        // Read as two booleans, '--undo --dry-run' took the undo branch
        // first and removed Cobalt from a reader nobody had agreed to.
        let parsed = parse_setup(&arguments(&["--undo", "--dry-run"])).expect("parse");
        assert_eq!(parsed.mode, SetupMode::Undo);
        assert!(parsed.dry_run);
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.starts_with("would "), "{plan}");
        assert!(plan.contains("would remove"), "{plan}");
        assert!(plan.contains(".adds/cobalt-launch.sh"), "{plan}");
        assert!(plan.contains(".adds/nm/menu"), "{plan}");
        assert!(plan.contains(".adds/cobalt.recovery.N"), "{plan}");
        assert!(plan.contains(".adds/cobalt.unusable[.N]"), "{plan}");
        assert!(plan.contains(setup::SSH_ENABLED), "{plan}");
        assert!(!plan.contains("would install"), "{plan}");
    }

    #[test]
    fn full_undo_preserves_mixed_nickelmenu_owner_entries() {
        let (reader, volume) = prepared_reader();
        std::fs::create_dir_all(volume.path.join(crate::setup::INSTALL_FOLDER))
            .expect("managed payload");
        crate::bootstrap::install(&volume.path).expect("bootstrap");
        let cobalt_owner = "menu_item :main :Owner tool :cmd_spawn :quiet:/mnt/onboard/owner.sh\n";
        let shared_owner = "menu_item :main :Other :cmd_spawn :quiet:/mnt/onboard/other.sh\n";
        std::fs::write(
            volume.path.join(crate::menu::CONFIG),
            format!(
                "{}{}",
                crate::menu::config(crate::setup::INSTALL_FOLDER),
                cobalt_owner
            ),
        )
        .expect("mixed dedicated config");
        std::fs::write(
            volume.path.join(".adds/nm/menu"),
            format!(
                "menu_item :main :Cobalt :cmd_spawn :quiet:{}\n{}",
                crate::bootstrap::DEVICE_PATH,
                shared_owner
            ),
        )
        .expect("mixed shared config");

        undo_setup(&reader, false).expect("full undo");

        assert_eq!(
            std::fs::read_to_string(volume.path.join(crate::menu::CONFIG))
                .expect("dedicated config"),
            cobalt_owner
        );
        assert_eq!(
            std::fs::read_to_string(volume.path.join(".adds/nm/menu")).expect("shared config"),
            shared_owner
        );
        assert!(!volume.path.join(crate::menu::UNINSTALL_FLAG).exists());
        assert!(volume.path.join(crate::menu::INSTALLED_MARKER).exists());
        assert!(!volume.path.join(crate::bootstrap::RELATIVE_PATH).exists());
        assert!(!volume.path.join(crate::setup::INSTALL_FOLDER).exists());
    }

    #[test]
    fn a_dry_run_names_every_change_it_would_make() {
        let parsed = parse_setup(&arguments(&["--dry-run"])).expect("parse");
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains("would install"));
        assert!(
            plan.contains("enable the firmware's root SSH server"),
            "{plan}"
        );
        assert!(plan.contains("save it the way"), "{plan}");
        assert!(plan.contains("kobo pair does"), "{plan}");
        assert!(plan.contains("close password login for root"), "{plan}");
        for (section, key, value) in setup::SETTINGS_APPLIED {
            assert!(plan.contains(&format!("{section}/{key}={value}")), "{plan}");
        }
    }

    #[test]
    fn a_dry_run_names_the_welcome_note_and_its_opt_out() {
        let parsed = parse_setup(&arguments(&["--dry-run"])).expect("parse");
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains(setup::SAMPLE_NAME), "{plan}");
        let parsed = parse_setup(&arguments(&["--dry-run", "--no-sample"])).expect("parse");
        assert!(!parsed.sample);
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains("--no-sample was given"), "{plan}");
    }

    #[test]
    fn noninteractive_change_without_yes_is_a_usage_error() {
        let error = parse_setup(&arguments(&["--non-interactive"])).expect_err("refused");
        assert!(error.starts_with("usage: "), "{error}");
    }

    #[test]
    fn ssh_is_on_by_default_and_the_old_opt_in_still_parses() {
        for words in [&["--dry-run"][..], &["--enable-ssh", "--dry-run"][..]] {
            let parsed = parse_setup(&arguments(words)).expect("parse");
            assert!(parsed.enable_ssh, "{words:?}");
            let plan = dry_run_plan(&parsed, &fresh_reader().0);
            assert!(plan.contains(setup::SSH_DISABLED), "{plan}");
            assert!(plan.contains("root SSH"), "{plan}");
            assert!(plan.contains("wait for the restarted reader"), "{plan}");
        }
    }

    #[test]
    fn no_ssh_leaves_the_server_off_and_says_why_nothing_waits() {
        let parsed = parse_setup(&arguments(&["--no-ssh", "--dry-run"])).expect("parse");
        assert!(!parsed.enable_ssh);
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains("--no-ssh was given"), "{plan}");
        assert!(plan.contains("stop after ejecting"), "{plan}");
        assert!(!plan.contains("authorized_keys"), "{plan}");
    }

    #[test]
    fn a_key_given_by_hand_leaves_password_login_alone_and_says_so() {
        let parsed = parse_setup(&arguments(&["--no-key", "--dry-run"])).expect("parse");
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains("leaving"), "{plan}");
        assert!(
            plan.contains("password login as the firmware has it"),
            "{plan}"
        );
        assert!(!plan.contains("close password login"), "{plan}");
    }

    #[test]
    fn setup_help_answers_wherever_it_is_asked_and_names_the_opt_out() {
        assert!(SETUP_HELP.contains("--no-ssh"));
        assert!(!SETUP_HELP.contains("--enable-ssh"));
        assert!(SETUP_HELP.contains("saves it"));
        super::super::setup_device(&arguments(&["--yes", "--help"])).expect("help, not a run");
    }

    #[test]
    fn a_reader_set_up_again_keeps_its_name_and_a_new_one_gets_a_free_one() {
        let mut store = crate::readers::Store::default();
        assert_eq!(
            super::super::setup_reader_name(&store, "N365410043013"),
            "kobo"
        );
        store
            .name("N365410043013", "192.0.2.4", "study")
            .expect("a free name");
        assert_eq!(
            super::super::setup_reader_name(&store, "N365410043013"),
            "study",
            "running setup again to update Cobalt must not rename the reader"
        );
        store
            .name("N418000000001", "192.0.2.5", "kobo")
            .expect("a free name");
        assert_eq!(
            super::super::setup_reader_name(&store, "N500000000002"),
            "kobo-2"
        );
    }

    #[test]
    fn only_the_reader_on_the_cable_is_taken_for_the_one_coming_back() {
        let ours = connect::Identity {
            serial: "N365410043013".into(),
            ..connect::Identity::default()
        };
        let neighbour = connect::Identity {
            serial: "N418999999999".into(),
            ..connect::Identity::default()
        };
        let laptop = connect::Identity::default();
        assert_eq!(
            setup::verdict_for(&ours, "N365410043013"),
            setup::Verdict::Reader
        );
        assert_eq!(
            setup::verdict_for(&neighbour, "N365410043013"),
            setup::Verdict::Other
        );
        assert_eq!(
            setup::verdict_for(&laptop, "N365410043013"),
            setup::Verdict::Other
        );
    }

    #[test]
    fn a_dry_run_that_will_not_wait_says_so() {
        let parsed =
            parse_setup(&arguments(&["--dry-run", "--enable-ssh", "--no-wait"])).expect("parse");
        assert!(dry_run_plan(&parsed, &fresh_reader().0).contains("--no-wait was given"));
    }

    #[test]
    fn enabling_ssh_installs_this_machines_key_by_default() {
        let parsed = parse_setup(&arguments(&["--enable-ssh", "--dry-run"])).expect("parse");
        assert!(parsed.authorize_key);
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains("this machine's public key"), "{plan}");
        assert!(plan.contains("kobo_cobalt"), "{plan}");
        // Both go into the one slot the firmware reads, so the plan has to
        // say so rather than describe two archives that cannot both exist.
        assert!(plan.contains("same .kobo/KoboRoot.tgz"), "{plan}");
        assert!(plan.contains("one authorized_keys"), "{plan}");
    }

    #[test]
    fn no_key_says_why_no_key() {
        let parsed =
            parse_setup(&arguments(&["--enable-ssh", "--no-key", "--dry-run"])).expect("parse");
        assert!(!parsed.authorize_key);
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains("--no-key was given"), "{plan}");
        assert!(!plan.contains("one authorized_keys"), "{plan}");
    }

    #[test]
    fn without_ssh_there_is_no_key_to_install() {
        let parsed = parse_setup(&arguments(&["--no-ssh", "--dry-run"])).expect("parse");
        let plan = dry_run_plan(&parsed, &fresh_reader().0);
        assert!(plan.contains("no SSH server to use it"), "{plan}");
    }

    #[test]
    fn a_fresh_reader_is_told_both_things_go_into_the_one_slot() {
        // The firmware extracts exactly one archive, so a plan that
        // described two would be describing something impossible.
        let (reader, _volume) = fresh_reader();
        let parsed = parse_setup(&arguments(&["--enable-ssh", "--dry-run"])).expect("parse");
        let plan = dry_run_plan(&parsed, &reader);
        assert!(plan.contains("stage NickelMenu"), "{plan}");
        assert!(plan.contains("same .kobo/KoboRoot.tgz"), "{plan}");
        assert!(
            plan.contains(
                "NickelMenu's own two files, one authorized_keys and the firmware's \
                 first-login marker"
            ),
            "{plan}"
        );
    }

    #[test]
    fn a_reader_that_already_has_the_plugin_is_not_promised_it_again() {
        // Found on a real reader: the plan said it would stage NickelMenu
        // and put the key in beside it, on a device that already had the
        // plugin and where the key would go in alone.
        let (reader, _volume) = prepared_reader();
        let parsed = parse_setup(&arguments(&["--enable-ssh", "--dry-run"])).expect("parse");
        let plan = dry_run_plan(&parsed, &reader);
        assert!(plan.contains("already installed on this reader"), "{plan}");
        assert!(plan.contains("would not write or reinstall"), "{plan}");
        assert!(plan.contains("any NickelMenu files"), "{plan}");
        assert!(plan.contains("or stage a KoboRoot.tgz"), "{plan}");
        assert!(!plan.contains("stage NickelMenu"), "{plan}");
        assert!(!plan.contains("same .kobo/KoboRoot.tgz"), "{plan}");
        assert!(
            plan.contains("nothing extracted as root but one authorized_keys"),
            "{plan}"
        );
    }

    #[test]
    fn an_unknown_option_is_refused_with_the_whole_usage() {
        let error = parse_setup(&arguments(&["--force"])).expect_err("refused");
        assert!(error.contains("--no-wait"), "{error}");
        assert!(error.contains("--undo"), "{error}");
        assert!(error.contains("--no-key"), "{error}");
    }
}

mod waiting {
    use super::super::{parse_wait, DEVICE_WAIT_MAXIMUM_SECONDS};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn defaults_to_five_minutes() {
        let given = arguments(&["--device", "192.0.2.1"]);
        let parsed = parse_wait(&given).expect("parse");
        assert_eq!(parsed.0, "192.0.2.1");
        assert_eq!(parsed.1.as_secs(), 300);
    }

    #[test]
    fn accepts_an_explicit_timeout() {
        let given = arguments(&["--device", "192.0.2.1", "--timeout", "90"]);
        let parsed = parse_wait(&given).expect("parse");
        assert_eq!(parsed.1.as_secs(), 90);
    }

    #[test]
    fn refuses_a_zero_or_unbounded_wait() {
        let zero = arguments(&["--device", "192.0.2.1", "--timeout", "0"]);
        assert!(parse_wait(&zero).is_err());
        let too_long = (DEVICE_WAIT_MAXIMUM_SECONDS + 1).to_string();
        let over = arguments(&["--device", "192.0.2.1", "--timeout", &too_long]);
        assert!(parse_wait(&over).is_err());
    }

    #[test]
    fn refuses_an_unsafe_host() {
        let given = arguments(&["--device", "192.0.2.1; rm -rf /"]);
        assert!(parse_wait(&given).is_err());
    }

    #[test]
    fn refuses_unknown_flags() {
        let given = arguments(&["--device", "192.0.2.1", "--forever"]);
        assert!(parse_wait(&given).is_err());
    }
}

#[test]
fn app_link_maintenance_accepts_only_fixed_actions_and_safe_hosts() {
    let status = vec![
        "status".to_owned(),
        "--device".to_owned(),
        "192.0.2.1".to_owned(),
    ];
    assert_eq!(super::parse_app_link(&status), Ok(("status", "192.0.2.1")));
    let unpair = vec![
        "unpair".to_owned(),
        "-s".to_owned(),
        "reader.local".to_owned(),
    ];
    assert_eq!(
        super::parse_app_link(&unpair),
        Ok(("unpair", "reader.local"))
    );
    for invalid in [
        vec![
            "delete".to_owned(),
            "--device".to_owned(),
            "reader".to_owned(),
        ],
        vec![
            "status".to_owned(),
            "--device".to_owned(),
            "reader;reboot".to_owned(),
        ],
        vec!["status".to_owned()],
    ] {
        assert!(super::parse_app_link(&invalid).is_err());
    }
}
