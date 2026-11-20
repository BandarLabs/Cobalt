//! A reader's arrival: from the cable going in to the computer finding it
//! on Wi-Fi.
//!
//! `kobo setup` already does all of it in one run. It writes Cobalt, ejects
//! the reader, waits for it to come back on the network, saves it, and locks
//! its remote login to this computer. What it does not do is say where it
//! has got to in a way a window can draw: it prints a report for somebody
//! reading a terminal, and it ends a failed wait with a command to type.
//!
//! A first-time owner cannot use either. So this reads what setup has said
//! so far and answers the one question a front end needs answered to show
//! the right step: what is happening now. It also keeps the two things a
//! window has to know after it has been closed and opened again, which are
//! which readers this computer has saved and whether one that was set up
//! from here is still waiting to be found, and it is the second half of
//! setup on its own for when the wait ran out.
//!
//! Nothing here is a second way of setting a reader up. The stages are read
//! from setup's own lines, and looking again runs the function setup's
//! ending runs.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::{connect, readers, setup, steps};

/// How long setup waits for a reader to come back, as a front end is told.
const LIMIT: u64 = setup::WAIT_LIMIT.as_secs();

/// What setup prints as it begins to wait, up to the number of minutes.
const WAITING: &str = "Waiting up to ";
/// What it prints when the reader it was waiting for has answered.
const BACK: &str = "The reader is back on Wi-Fi at ";
/// What it prints when the wait ran out.
const NOT_BACK: &str = "The reader did not come back on Wi-Fi in time";
/// What it prints when the reader answered from more than one place.
const SEVERAL: &str = "The reader answered at more than one address";
/// What it prints when the reader could not be ejected.
const LEFT_MOUNTED: &str = "volume left mounted";
/// What it prints once the reader has been ejected.
const EJECTED: &str = "volume ejected";
/// What opens its report of an install that has been written.
const WRITTEN: &str = "\nSet up ";
/// What every line about closing password login starts with.
const LOGIN: &str = "SSH: ";
/// What [`find`] prints when this computer has no network to look on.
pub const NO_NETWORK: &str = "This computer is not on a network, so the reader was not looked for.";

/// Where an install or a search has got to, from what it has said.
///
/// The stages are a front end's steps, not setup's: several things setup
/// does one after another are one stage here, because an owner watching
/// has one thing to do or to wait for during all of them.
///
/// * `writing`: Cobalt is being copied to the reader and checked.
/// * `eject`: it is on the reader, which is still mounted and has to be
///   ejected before it will restart. The job has ended.
/// * `finding`: the reader was ejected and is being looked for on Wi-Fi.
/// * `securing`: it answered and is being saved and locked to this
///   computer.
/// * `ready`: finished, with the reader saved.
/// * `not_found`: the wait ran out.
/// * `several`: it answered from more than one address, so none was saved.
/// * `no_network`: this computer is on no network, so nothing was looked
///   for.
/// * `failed`: the command failed, and its text says why.
#[must_use]
pub fn progress(text: &str, running: bool, ok: Option<bool>) -> Value {
    let said = |marker: &str| text.contains(marker);
    let waiting = text.find(WAITING);
    let saved_as = text
        .split_once(BACK)
        .and_then(|(_, rest)| rest.split_once("Saved "))
        .and_then(|(_, rest)| rest.split_once(" as \""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(name, _)| name.to_owned());
    // Setup closes password login only over a connection that proves this
    // computer's key works, and says in one line how that went. The two
    // endings that leave it closed both say so in the same words.
    let login = text
        .rfind(LOGIN)
        .map(|at| text[at..].lines().next().unwrap_or_default());
    let secured = login.map(|line| line.contains("only this computer's key gets in"));

    let stage = if ok == Some(false) {
        "failed"
    } else if said(BACK) {
        if running {
            "securing"
        } else {
            "ready"
        }
    } else if said(NOT_BACK) {
        "not_found"
    } else if said(SEVERAL) {
        "several"
    } else if waiting.is_some() {
        "finding"
    } else if said(NO_NETWORK) {
        "no_network"
    } else if said(LEFT_MOUNTED) {
        "eject"
    } else if said(EJECTED) && !running {
        // Ejected, finished, and never waited: the only reason setup skips
        // the wait after a clean eject is having no network to wait on.
        "no_network"
    } else if said(WRITTEN) && !running {
        "ready"
    } else {
        "writing"
    };

    // One dot is printed after each pause between looks, so the dots are
    // the wait so far in units of that pause. It undercounts by however
    // long the looks themselves took, which is the right way to be wrong
    // for something drawn against a limit.
    let waited = waiting.map_or(0, |at| {
        let dots = text[at..]
            .lines()
            .skip(1)
            .take_while(|line| line.chars().all(|dot| dot == '.'))
            .map(str::len)
            .sum::<usize>();
        (dots as u64 * setup::WAIT_INTERVAL.as_secs()).min(LIMIT)
    });

    let mut notes = Vec::new();
    if said("no menu entry:") {
        notes.push(
            "Cobalt could not be added to the reader's menu, because the menu's files could \
             not be downloaded. Install again when this computer is online."
                .to_owned(),
        );
    }
    if said("It could not be saved on this computer") {
        notes.push(
            "The reader was found, and this computer could not save it. Look again to retry."
                .to_owned(),
        );
    }
    if secured == Some(false) && !running {
        notes.push(
            "The reader's remote login could not be confirmed as locked to this computer. \
             Installing again retries it."
                .to_owned(),
        );
    }
    json!({
        "stage": stage,
        "waited": waited,
        "limit": LIMIT,
        "saved_as": saved_as,
        "secured": secured,
        "notes": notes,
    })
}

/// Whether a job with this title is one [`progress`] can read.
#[must_use]
pub fn follows(title: &str) -> bool {
    [
        "Installing Cobalt",
        "Installing Cobalt again",
        "Updating Cobalt",
        FINDING,
    ]
    .contains(&title)
}

/// The title of the job that looks for a reader again.
pub const FINDING: &str = "Looking for the reader";

/// Which corner of a reader's home screen its menu is in, in words.
#[must_use]
pub fn menu_corner(firmware: &str) -> &'static str {
    setup::menu_corner(firmware)
}

/// The name this computer saved a reader under, when it has saved it.
#[must_use]
pub fn saved_as(store: &readers::Store, serial: &str) -> Option<String> {
    store
        .readers
        .iter()
        .find(|reader| reader.serial == serial)
        .map(|reader| reader.nickname.clone())
}

/// The readers saved on this computer, each with where it is mounted when
/// it is plugged in now.
///
/// By name only. A serial is how the store tells readers apart, and it
/// stays in the store: the front ends are handed names to show and to hand
/// back, never the thing a name stands for.
#[must_use]
pub fn saved(store: &readers::Store, mounted: &[setup::Mounted]) -> Vec<Value> {
    store
        .readers
        .iter()
        .map(|reader| {
            let volume = mounted
                .iter()
                .find(|here| here.serial == reader.serial)
                .map(|here| here.volume.to_string_lossy().into_owned());
            json!({ "name": reader.nickname, "volume": volume })
        })
        .collect()
}

/// The serial of the reader that was last set up from this computer and
/// has not been saved, when there is one.
///
/// Setup records each install it finishes, and saving a reader is the last
/// thing it does, minutes later and only if the reader came back. A reader
/// in the first list and not the second is one whose owner got as far as
/// unplugging it. That is true however many times the window has been
/// closed since, which is what lets it open again on the step they were at.
#[must_use]
pub fn awaiting(config: &Path) -> Option<String> {
    let done = steps::Steps::load(&config.join("setup-steps")).ok()?;
    let store = readers::Store::load(&config.join("readers")).unwrap_or_default();
    done.steps
        .iter()
        .filter(|step| step.name == steps::SETUP)
        .filter(|step| {
            !store
                .readers
                .iter()
                .any(|saved| saved.serial == step.serial)
        })
        .max_by_key(|step| step.at)
        .map(|step| step.serial.clone())
}

/// Why a name cannot be given to a reader, or nothing when it can.
///
/// The store is lines of `key = value`, a name is passed to commands as
/// `--reader NAME`, and a name that reads as an address is taken for one.
/// So a name is one line of ordinary characters, does not start like an
/// option, and is not a run of digits and dots.
#[must_use]
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Give the reader a name.");
    }
    if name.chars().count() > 40 {
        return Some("Use a name of 40 characters or fewer.");
    }
    if name != name.trim() || name.chars().any(char::is_control) {
        return Some("Use a name with no line breaks or spaces at either end.");
    }
    if name.starts_with('-') || name.contains('=') {
        return Some("Use a name that does not start with a hyphen or contain an equals sign.");
    }
    if name.parse::<std::net::IpAddr>().is_ok() {
        return Some("Use a name that is not a network address.");
    }
    None
}

/// Gives the reader saved as `name` the name `to`.
///
/// # Errors
///
/// `None` when no reader is saved as `name`; otherwise why `to` cannot be
/// its name, or why the store could not be written.
pub fn rename(config: &Path, name: &str, to: &str) -> Result<(), Option<String>> {
    let path = config.join("readers");
    let mut store = readers::Store::load(&path).map_err(Some)?;
    if store.find(to).is_some_and(|other| other.nickname != name) {
        return Err(Some(format!("Another reader is already called \"{to}\".")));
    }
    let reader = store
        .readers
        .iter_mut()
        .find(|reader| reader.nickname == name)
        .ok_or(None)?;
    to.clone_into(&mut reader.nickname);
    store.save(&path).map_err(Some)
}

/// Forgets the reader saved as `name`. Nothing on the reader changes: it
/// keeps Cobalt and still accepts this computer's key, and plugging it in
/// and installing again saves it again.
///
/// # Errors
///
/// `None` when no reader is saved as `name`; otherwise why the store could
/// not be written.
pub fn forget(config: &Path, name: &str) -> Result<(), Option<String>> {
    let path = config.join("readers");
    let mut store = readers::Store::load(&path).map_err(Some)?;
    let before = store.readers.len();
    store.readers.retain(|reader| reader.nickname != name);
    if store.readers.len() == before {
        return Err(None);
    }
    store.save(&path).map_err(Some)
}

/// `kobo ui --find-reader`: the second half of setup, for the reader whose
/// first half was done from this computer.
///
/// Run as a child by the server, never typed. It looks for that reader by
/// its serial for as long as setup would, then saves it and closes its
/// password login exactly as setup does, by calling what setup calls. It
/// prints what setup prints, so [`progress`] reads it the same way.
///
/// # Errors
///
/// When no reader set up from this computer is waiting to be found.
pub fn find() -> Result<(), String> {
    let config = crate::clip_delivery::config_root();
    let Some(serial) = awaiting(&config) else {
        return Err(crate::console::target(
            "no reader set up from this computer is waiting to be found",
        ));
    };
    let Some(subnet) = connect::local_subnet() else {
        println!("{NO_NETWORK}");
        return Ok(());
    };
    // Only the serial is used: it is how the reader is told from any other
    // Kobo that answers.
    let reader = setup::Mounted {
        volume: PathBuf::new(),
        serial,
        firmware: String::new(),
        product_id: None,
    };
    crate::await_reader(&subnet, &reader, true);
    Ok(())
}

#[cfg(test)]
#[path = "arrival_tests.rs"]
mod tests;
