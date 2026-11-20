use std::fs;

use super::*;

/// What setup prints once Cobalt is on a reader, from setup's own report.
fn written(ejected: bool, waiting: bool) -> String {
    let reader = setup::Mounted {
        volume: PathBuf::from("/Volumes/KOBOeReader"),
        serial: "N365410043013".to_owned(),
        firmware: "4.45.23697".to_owned(),
        product_id: Some(391),
    };
    let report = setup::Report {
        installed: 212,
        ssh: None,
        key: None,
        settings: Vec::new(),
        trust: Vec::new(),
        menu: None,
        ejected,
        waiting,
    };
    format!(
        "found Kobo Clara BW at /Volumes/KOBOeReader\n{}",
        report.describe_for(&reader)
    )
}

const WAIT: &str = "\nWaiting up to 5 minutes for the reader to come back on 192.168.1.0/24. \
                    Ctrl-C to stop.\n";
const FOUND: &str = "\nThe reader is back on Wi-Fi at 192.168.1.23.\n\
                     Saved Kobo Clara BW (N365, firmware 4.45.23697) as \"study\" at 192.168.1.23.\n\
                     Commands that act on a reader now use it, for example:\n  \
                     kobo frame push ~/Pictures/family\n";
const LOCKED: &str = "SSH: only this computer's key gets in. Password login for root is off, \
                      and a\nfresh key login confirmed the key still works.\n";

fn stage(text: &str, running: bool, ok: Option<bool>) -> String {
    progress(text, running, ok)["stage"]
        .as_str()
        .expect("a stage")
        .to_owned()
}

#[test]
fn an_install_is_followed_from_copying_to_the_reader_being_saved() {
    // Nothing but the line that names the reader: Cobalt is being copied.
    let begun = "found Kobo Clara BW at /Volumes/KOBOeReader\n";
    assert_eq!(stage(begun, true, None), "writing");
    assert_eq!(stage("", true, None), "writing");

    // Ejected, and setup has begun to wait.
    let waiting = written(true, true) + WAIT;
    let answer = progress(&waiting, true, None);
    assert_eq!(answer["stage"], "finding");
    assert_eq!(answer["waited"], 0);
    assert_eq!(answer["limit"], 300);
    assert_eq!(answer["saved_as"], Value::Null);
    assert_eq!(answer["secured"], Value::Null);

    // A dot for each pause between looks.
    let a_while = waiting.clone() + "....";
    assert_eq!(progress(&a_while, true, None)["waited"], 40);
    let too_long = waiting.clone() + &".".repeat(99);
    assert_eq!(progress(&too_long, true, None)["waited"], 300);

    // It answered: saved under its name, and the login is being locked.
    let found = a_while.clone() + FOUND;
    let answer = progress(&found, true, None);
    assert_eq!(answer["stage"], "securing");
    assert_eq!(answer["saved_as"], "study");
    assert_eq!(answer["waited"], 40);

    let done = found + LOCKED;
    let answer = progress(&done, false, Some(true));
    assert_eq!(answer["stage"], "ready");
    assert_eq!(answer["secured"], true);
    assert_eq!(answer["notes"], json!([]));
}

#[test]
fn each_way_an_install_can_end_short_of_ready_is_its_own_stage() {
    // Copied, and the reader could not be ejected: the owner has to.
    assert_eq!(stage(&written(false, false), false, Some(true)), "eject");
    // Ejected with no network to wait on.
    assert_eq!(
        stage(&written(true, false), false, Some(true)),
        "no_network"
    );
    // Still running after the report means the wait is about to begin.
    assert_eq!(stage(&written(true, true), true, None), "writing");

    let waited = written(true, true) + WAIT + &".".repeat(30);
    let timed_out = waited.clone()
        + "\nThe reader did not come back on Wi-Fi in time, so it is not saved yet. It is\n\
           set up either way. Wake it (press power). Then save it with:\n  kobo pair\n";
    assert_eq!(stage(&timed_out, false, Some(true)), "not_found");
    let twice = waited
        + "\nThe reader answered at more than one address (192.168.1.23, 192.168.1.40), so \
           this will not\nguess which to save. Once it has settled, save it with: kobo pair\n";
    assert_eq!(stage(&twice, false, Some(true)), "several");

    // A command that failed is failed wherever it had got to.
    assert_eq!(
        stage("kobo: the reader was unplugged\n", false, Some(false)),
        "failed"
    );
    assert_eq!(stage(&written(true, true), false, Some(false)), "failed");
    // Looking again from a computer with no network.
    assert_eq!(
        stage(&format!("{NO_NETWORK}\n"), false, Some(true)),
        "no_network"
    );
}

#[test]
fn what_did_not_go_to_plan_is_said_in_a_sentence_and_does_not_change_the_stage() {
    let found = written(true, true) + WAIT + "." + FOUND;
    let unlocked = found.clone()
        + "SSH: password login is off for now, but the confirming key login did not get\n\
           through, so the reader turns it back on by itself in 5 minutes.\n";
    let answer = progress(&unlocked, false, Some(true));
    assert_eq!(answer["stage"], "ready");
    assert_eq!(answer["secured"], false);
    assert_eq!(answer["notes"].as_array().expect("notes").len(), 1);
    // While it is still being done, nothing is said to have gone wrong.
    assert_eq!(progress(&unlocked, true, None)["notes"], json!([]));

    let no_menu = written(true, true).replace(
        "  · volume ejected",
        "  · no menu entry: the download failed\n  · volume ejected",
    );
    let notes = progress(&no_menu, true, None)["notes"].clone();
    assert!(
        notes[0].as_str().expect("a note").contains("menu"),
        "{notes}"
    );

    let unsaved = written(true, true)
        + WAIT
        + "\nThe reader is back on Wi-Fi at 192.168.1.23.\n\
           It could not be saved on this computer (the store is read-only). Save it with:\n  \
           kobo pair\n";
    let answer = progress(&unsaved, false, Some(true));
    assert_eq!(answer["saved_as"], Value::Null);
    assert!(answer["notes"][0]
        .as_str()
        .expect("a note")
        .contains("could not save it"));
}

#[test]
fn the_lines_these_stages_are_read_from_are_the_ones_setup_still_prints() {
    // The report is made by calling setup; the wait's lines are printed
    // from the command's own source and can only be checked by reading it.
    let main = include_str!("../main.rs");
    for line in [
        WAITING,
        BACK,
        NOT_BACK,
        SEVERAL,
        "only this computer's key gets in",
        "It could not be saved on this computer",
        r#"Saved {summary} as \"{name}\" at {address}."#,
        "print!(\".\");",
    ] {
        assert!(main.contains(line), "kobo setup no longer prints: {line}");
    }
    let setup = include_str!("../setup.rs");
    assert!(setup.contains("no menu entry:"));
    assert!(written(true, false).contains(EJECTED));
    assert!(written(false, false).contains(LEFT_MOUNTED));
    assert!(written(true, false).contains(WRITTEN));
    assert!(WAIT.contains(&format!("{WAITING}{} minutes", LIMIT / 60)));
}

#[test]
fn only_an_install_or_a_search_is_read_this_way() {
    for title in [
        "Installing Cobalt",
        "Installing Cobalt again",
        "Updating Cobalt",
        FINDING,
    ] {
        assert!(follows(title), "{title}");
    }
    for title in ["Removing Cobalt", "Installing an app", "Sending a file", ""] {
        assert!(!follows(title), "{title}");
    }
}

/// A config folder for one test, with these installs recorded and these
/// readers saved.
fn config(name: &str, set_up: &[(&str, u64)], saved: &[(&str, &str)]) -> PathBuf {
    let folder =
        std::env::temp_dir().join(format!("kobo-ui-arrival-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).expect("folder");
    let mut done = steps::Steps::default();
    for (serial, at) in set_up {
        done.complete(steps::SETUP, serial, *at);
    }
    done.complete("something-else", "N999", 99_999);
    done.save(&folder.join("setup-steps")).expect("steps");
    let mut store = readers::Store::default();
    for (serial, name) in saved {
        store.name(serial, "192.168.1.9", name).expect("a name");
    }
    store.save(&folder.join("readers")).expect("readers");
    folder
}

#[test]
fn a_reader_set_up_from_here_and_not_yet_saved_is_the_one_being_waited_for() {
    let nothing = config("none", &[], &[]);
    assert_eq!(awaiting(&nothing), None);
    assert_eq!(awaiting(&nothing.join("no-such-folder")), None);

    let fresh = config("fresh", &[("N365A", 100)], &[]);
    assert_eq!(awaiting(&fresh).as_deref(), Some("N365A"));

    let found = config("found", &[("N365A", 100)], &[("N365A", "kobo")]);
    assert_eq!(awaiting(&found), None);

    // Two set up and one saved; then two waiting, and the later one wins.
    let second = config(
        "second",
        &[("N365A", 100), ("N418B", 200)],
        &[("N418B", "kobo")],
    );
    assert_eq!(awaiting(&second).as_deref(), Some("N365A"));
    let both = config("both", &[("N365A", 300), ("N418B", 200)], &[]);
    assert_eq!(awaiting(&both).as_deref(), Some("N365A"));
    for folder in [nothing, fresh, found, second, both] {
        let _ = fs::remove_dir_all(folder);
    }
}

#[test]
fn saved_readers_are_listed_by_name_with_where_each_is_plugged_in() {
    let folder = config("saved", &[], &[("N365A", "study"), ("N418B", "kitchen")]);
    let store = readers::Store::load(&folder.join("readers")).expect("store");
    let plugged = [setup::Mounted {
        volume: PathBuf::from("/Volumes/KOBOeReader"),
        serial: "N418B".to_owned(),
        firmware: "4.38.23171".to_owned(),
        product_id: None,
    }];
    let listed = json!(saved(&store, &plugged));
    assert_eq!(
        listed,
        json!([
            { "name": "study", "volume": null },
            { "name": "kitchen", "volume": "/Volumes/KOBOeReader" },
        ])
    );
    assert!(!listed.to_string().contains("N365A"));
    assert_eq!(saved_as(&store, "N418B").as_deref(), Some("kitchen"));
    assert_eq!(saved_as(&store, "N000"), None);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_reader_is_renamed_and_forgotten_by_name_and_no_name_is_shared() {
    let folder = config("rename", &[], &[("N365A", "kobo"), ("N418B", "kitchen")]);
    let names = || -> Vec<String> {
        readers::Store::load(&folder.join("readers"))
            .expect("store")
            .readers
            .into_iter()
            .map(|reader| format!("{}={}", reader.serial, reader.nickname))
            .collect()
    };

    assert_eq!(rename(&folder, "kobo", "study"), Ok(()));
    assert_eq!(names(), ["N365A=study", "N418B=kitchen"]);
    // To its own name is nothing to refuse.
    assert_eq!(rename(&folder, "study", "study"), Ok(()));
    assert!(rename(&folder, "study", "kitchen")
        .expect_err("taken")
        .is_some_and(|reason| reason.contains("already called \"kitchen\"")));
    assert_eq!(rename(&folder, "bedroom", "attic"), Err(None));
    assert_eq!(names(), ["N365A=study", "N418B=kitchen"]);

    assert_eq!(forget(&folder, "study"), Ok(()));
    assert_eq!(forget(&folder, "study"), Err(None));
    assert_eq!(names(), ["N418B=kitchen"]);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_name_is_one_line_that_cannot_be_read_as_an_option_or_an_address() {
    for name in [
        "study",
        "Mum's Kobo",
        "kobo-2",
        "Clara (bedroom)",
        "書斎",
        "192.168 shelf",
    ] {
        assert_eq!(name_problem(name), None, "{name}");
    }
    for name in [
        "",
        " study",
        "study ",
        "two\nlines",
        "tab\there",
        "-reader",
        "--volume",
        "a=b",
        "192.168.1.5",
        "::1",
        &"x".repeat(41),
    ] {
        assert!(name_problem(name).is_some(), "{name:?}");
    }
}

#[test]
fn the_menu_is_looked_for_in_the_corner_the_firmware_puts_it_in() {
    assert_eq!(menu_corner("4.45.23697"), "bottom right");
    assert_eq!(menu_corner("4.23.15505"), "bottom right");
    assert_eq!(menu_corner("4.22.15268"), "top left");
    // The same corner setup's own report names.
    assert!(written(true, false).contains("bottom-right Kobo menu"));
}
