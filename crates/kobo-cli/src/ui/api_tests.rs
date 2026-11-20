//! Each route against a computer made of folders: readers that are plugged
//! in and pulled out, a signed fixture catalog, and a stand-in for the kobo
//! command that records exactly how it was run.

use serde_json::json;

use super::super::server::Limits;
use super::super::testing::{
    make_desk, picture, picture_address, query_value, quick, Desk, Running, CLARA_BW,
    CLARA_HD_NEWER, FIRMWARE_FIVE,
};
use super::*;

fn running(name: &str) -> (Running, Desk) {
    let (machine, desk) = make_desk(name);
    (Running::start(machine, Limits::default(), quick()), desk)
}

fn volume_body(volume: &Path) -> String {
    json!({ "volume": volume.to_string_lossy() }).to_string()
}

#[test]
fn the_state_names_each_reader_and_how_its_cobalt_stands_without_its_serial() {
    let (running, desk) = running("state");
    let page = running.open();
    let empty = page.get("/api/state").json();
    assert_eq!(empty["readers"], json!([]));
    assert_eq!(empty["computer"]["release"], "0.3.37");
    assert_eq!(empty["computer"]["release_problem"], Value::Null);
    assert_eq!(empty["jobs"], json!([]));
    assert_eq!(empty["api"], 1);
    assert!(["macos", "linux", "windows"].contains(&empty["computer"]["os"].as_str().expect("os")));
    assert_eq!(empty["attached"], Value::Null);
    assert_eq!(empty["blocked"], json!([]));

    let fresh = desk.plug("fresh", CLARA_BW, None);
    let behind = desk.plug("behind", CLARA_BW, Some("0.3.30"));
    desk.plug("current", CLARA_BW, Some("0.3.37"));
    desk.plug("ahead", CLARA_BW, Some("0.4.0"));
    desk.plug("untested", CLARA_HD_NEWER, None);
    desk.plug("five", FIRMWARE_FIVE, None);
    desk.plug(
        "unknown",
        "N999000000000,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000-0000-0000-000000000999",
        None,
    );
    let reply = page.get("/api/state");
    assert!(
        !reply.body.contains("N365410043013"),
        "the serial is not shown"
    );
    assert!(!reply.body.contains("410043013"));
    let state = reply.json();
    let reader = |name: &str| {
        state["readers"]
            .as_array()
            .expect("readers")
            .iter()
            .find(|reader| {
                reader["volume"]
                    .as_str()
                    .is_some_and(|volume| volume.ends_with(name))
            })
            .unwrap_or_else(|| panic!("no reader called {name}"))
            .clone()
    };

    let found = reader("fresh");
    assert_eq!(found["volume"], fresh.to_string_lossy().as_ref());
    assert_eq!(found["model"], "Kobo Clara BW");
    assert_eq!(found["model_code"], "N365");
    assert_eq!(found["firmware"], "4.45.23697");
    assert_eq!(found["supported"], true);
    assert_eq!(found["refusal"], Value::Null);
    assert_eq!(found["untested"], Value::Null);
    assert_eq!(found["cobalt"], Value::Null);
    assert_eq!(found["standing"], "absent");
    assert_eq!(found["free_bytes"], 1_000_000_000_u64);
    assert_eq!(found["writable"], true);

    let found = reader("behind");
    assert_eq!(found["volume"], behind.to_string_lossy().as_ref());
    assert_eq!(found["cobalt"]["cobalt_version"], "0.3.30");
    assert_eq!(found["cobalt"]["channel"], "stable");
    assert_eq!(found["standing"], "behind");
    assert_eq!(reader("current")["standing"], "current");
    assert_eq!(reader("ahead")["standing"], "ahead");

    let found = reader("untested");
    assert_eq!(found["model"], "Kobo Clara HD");
    assert_eq!(found["supported"], true);
    assert!(found["untested"]
        .as_str()
        .expect("a note")
        .starts_with("untested firmware."));

    // Refused with the reason the command itself gives.
    let found = reader("five");
    assert_eq!(found["supported"], false);
    assert!(found["refusal"]
        .as_str()
        .expect("a reason")
        .starts_with("firmware 5.2.190625 cannot start Cobalt"));
    assert_eq!(found["model"], "Kobo Clara BW");

    let found = reader("unknown");
    assert_eq!(found["model"], "Kobo, model code N999");
    assert_eq!(found["supported"], true);
    assert!(found["untested"]
        .as_str()
        .expect("a note")
        .starts_with("untested model."));

    // A reader that is pulled out is gone from the next answer.
    desk.plugged.lock().expect("plugged").clear();
    assert_eq!(page.get("/api/state").json()["readers"], json!([]));
}

#[test]
fn a_computer_with_no_release_says_why_and_a_cobalt_that_cannot_name_itself_is_a_problem() {
    let (mut machine, desk) = make_desk("state-problems");
    machine.release = Err("There is no release here.".to_owned());
    desk.plug("installed", CLARA_BW, Some("0.3.30"));
    desk.plug("broken", CLARA_BW, Some("not a version"));
    let running = Running::start(machine, Limits::default(), quick());
    let page = running.open();
    let state = page.get("/api/state").json();
    assert_eq!(state["computer"]["release"], Value::Null);
    assert_eq!(
        state["computer"]["release_problem"],
        "There is no release here."
    );
    let readers = state["readers"].as_array().expect("readers");
    let installed = readers
        .iter()
        .find(|reader| reader["cobalt"].is_object())
        .expect("the installed one");
    assert_eq!(installed["standing"], "unknown");
    let broken = readers
        .iter()
        .find(|reader| reader["cobalt_problem"].is_string())
        .expect("the broken one");
    assert_eq!(broken["cobalt"], Value::Null);
    assert!(broken["cobalt_problem"]
        .as_str()
        .expect("a problem")
        .contains("does not say which release it is"));
}

#[test]
fn the_plan_is_the_dry_run_one_change_to_an_entry_for_installing_and_for_removing() {
    let (running, desk) = running("plan");
    let page = running.open();
    let volume = desk.plug("reader", CLARA_HD_NEWER, None);
    let asked = query_value(&volume.to_string_lossy());

    let plan = page.get(&format!("/api/plan?volume={asked}")).json();
    assert_eq!(plan["undo"], false);
    assert_eq!(plan["release"], "0.3.37");
    let lines: Vec<&str> = plan["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(lines[0].starts_with("would install Cobalt into "));
    assert!(lines[0].ends_with(".adds/cobalt"));
    assert!(lines
        .iter()
        .any(|line| line.starts_with("would enable the firmware's root SSH server")));
    assert!(lines
        .last()
        .expect("a last line")
        .starts_with("nothing outside the book partition"));
    assert!(lines
        .iter()
        .all(|line| !line.starts_with(' ') && !line.is_empty()));
    assert!(plan["untested"]
        .as_str()
        .expect("the note setup would give")
        .starts_with("untested firmware."));

    let undo = page.get(&format!("/api/plan?volume={asked}&undo=1")).json();
    assert_eq!(undo["undo"], true);
    assert_eq!(undo["untested"], Value::Null);
    let lines: Vec<&str> = undo["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(lines[0].starts_with("would remove managed Cobalt trees under "));
    // A line the terminal wrapped is one entry again.
    assert!(lines[0].contains("after moving any owner folders into a free"));
    assert_eq!(
        lines.last().copied(),
        Some("nothing else on the reader is touched")
    );

    // A reader the command would refuse has no install plan, and says why.
    let five = desk.plug("five", FIRMWARE_FIVE, None);
    let asked = query_value(&five.to_string_lossy());
    let refused = page.get(&format!("/api/plan?volume={asked}"));
    assert_eq!(refused.status, 409);
    assert!(refused.error().contains("cannot start Cobalt"));
    // It can still be asked what removing would do.
    assert_eq!(
        page.get(&format!("/api/plan?volume={asked}&undo=1")).status,
        200
    );
    assert_eq!(page.get("/api/plan").status, 400);
}

#[test]
fn a_reader_the_server_did_not_find_for_itself_is_never_acted_on() {
    let (running, desk) = running("named-reader");
    let page = running.open();
    desk.plug("plugged", CLARA_BW, Some("0.3.30"));
    // A folder that is a reader in every way except being one this server
    // found: made the same way, then unplugged.
    let elsewhere = desk.plug("elsewhere", CLARA_BW, Some("0.3.30"));
    desk.plugged.lock().expect("plugged").pop();
    let before = Desk::listing(&desk.root);
    let body = json!({
        "volume": elsewhere.to_string_lossy(),
        "ids": ["almanac"],
        "id": "almanac",
    })
    .to_string();
    for path in [
        "/api/reader/install",
        "/api/reader/remove",
        "/api/reader/eject",
        "/api/apps/catalog",
        "/api/apps/install",
        "/api/apps/remove",
    ] {
        let reply = page.post(path, &body);
        assert_eq!(reply.status, 404, "{path}: {}", reply.body);
        assert!(reply.error().contains("no longer connected"));
    }
    let asked = query_value(&elsewhere.to_string_lossy());
    assert_eq!(page.get(&format!("/api/apps?volume={asked}")).status, 404);
    assert_eq!(page.get(&format!("/api/plan?volume={asked}")).status, 404);
    // Nor is one named by a path that only resembles a found one.
    let near = format!("{}/../plugged", elsewhere.to_string_lossy());
    assert_eq!(
        page.post("/api/reader/eject", &json!({ "volume": near }).to_string())
            .status,
        404
    );
    assert!(desk.commands().is_empty());
    assert!(desk.ejected.lock().expect("ejected").is_empty());
    assert!(desk.fetched.lock().expect("fetched").is_empty());
    assert_eq!(Desk::listing(&desk.root), before);
}

#[cfg(unix)]
#[test]
fn installing_updating_and_removing_cobalt_run_setup_with_its_confirmation_given() {
    let (running, desk) = running("setup-jobs");
    let page = running.open();
    let fresh = desk.plug("fresh", CLARA_BW, None);
    let reply = page.post("/api/reader/install", &volume_body(&fresh));
    assert_eq!(reply.status, 200, "{}", reply.body);
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["title"], "Installing Cobalt");
    assert_eq!(report["kind"], "reader");
    assert_eq!(report["ok"], true);
    let said = report["text"].as_str().expect("text");
    assert!(said.contains("ran: setup --yes --non-interactive --volume "));
    assert!(said.contains("kobo: said on the error stream"));
    assert_eq!(
        desk.commands(),
        [format!(
            "setup --yes --non-interactive --volume {}",
            fresh.display()
        )]
    );

    let installed = desk.plug("installed", CLARA_BW, Some("0.3.30"));
    let reply = page.post("/api/reader/install", &volume_body(&installed));
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["title"], "Updating Cobalt");
    let same = desk.plug("same", CLARA_BW, Some("0.3.37"));
    let reply = page.post("/api/reader/install", &volume_body(&same));
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["title"], "Installing Cobalt again");

    let reply = page.post("/api/reader/remove", &volume_body(&installed));
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["title"], "Removing Cobalt");
    assert_eq!(
        desk.commands().last().expect("a run"),
        &format!(
            "setup --undo --yes --non-interactive --volume {}",
            installed.display()
        )
    );

    // The jobs are in the state a reloaded page starts from.
    let jobs = page.get("/api/state").json()["jobs"].clone();
    assert_eq!(jobs.as_array().expect("jobs").len(), 4);
    assert_eq!(jobs[3]["title"], "Removing Cobalt");
    assert_eq!(jobs[3]["running"], false);
}

#[cfg(unix)]
#[test]
fn a_setup_that_fails_is_a_failed_job_that_keeps_what_the_command_said() {
    let (running, desk) = running("setup-fails");
    let page = running.open();
    std::fs::write(desk.root.join("kobo.status"), "3").expect("status");
    let volume = desk.plug("reader", CLARA_BW, None);
    let reply = page.post("/api/reader/install", &volume_body(&volume));
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["ok"], false);
    assert_eq!(report["code"], 3);
    assert!(report["text"]
        .as_str()
        .expect("text")
        .contains("kobo: said on the error stream"));
}

#[cfg(unix)]
#[test]
fn a_release_named_on_the_command_line_is_the_one_setup_is_told_to_install() {
    let (mut machine, desk) = make_desk("setup-release-dir");
    machine.release_dir = Some(PathBuf::from("/releases/0.3.37-stable"));
    let running = Running::start(machine, Limits::default(), quick());
    let page = running.open();
    let volume = desk.plug("reader", CLARA_BW, None);
    let reply = page.post("/api/reader/install", &volume_body(&volume));
    page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(
        desk.commands(),
        [format!(
            "setup --yes --non-interactive --volume {} --release-dir /releases/0.3.37-stable",
            volume.display()
        )]
    );
}

#[test]
fn cobalt_is_not_installed_on_a_reader_it_cannot_start_on_or_from_a_computer_with_no_release() {
    let (running, desk) = running("install-refused");
    let page = running.open();
    let five = desk.plug("five", FIRMWARE_FIVE, None);
    let reply = page.post("/api/reader/install", &volume_body(&five));
    assert_eq!(reply.status, 409);
    assert!(reply.error().contains("cannot start Cobalt"));

    let (mut machine, other) = make_desk("install-no-release");
    machine.release = Err("There is no release here.".to_owned());
    let volume = other.plug("reader", CLARA_BW, None);
    let without = Running::start(machine, Limits::default(), quick());
    let reply = without
        .open()
        .post("/api/reader/install", &volume_body(&volume));
    assert_eq!(reply.status, 409);
    assert_eq!(reply.error(), "There is no release here.");
    assert!(desk.commands().is_empty());
    assert!(other.commands().is_empty());
}

#[test]
fn ejecting_ejects_the_named_reader_and_a_failure_is_answered_with_its_reason() {
    let (running, desk) = running("eject");
    let volume = desk.plug("reader", CLARA_BW, None);
    desk.plug("another", CLARA_BW, None);
    let reply = running
        .open()
        .post("/api/reader/eject", &volume_body(&volume));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.json()["ejected"], true);
    assert_eq!(*desk.ejected.lock().expect("ejected"), [volume]);

    let (mut machine, desk) = make_desk("eject-fails");
    machine.eject = Box::new(|_| Err(console::target("a window is still using the volume")));
    let volume = desk.plug("reader", CLARA_BW, None);
    let failing = Running::start(machine, Limits::default(), quick());
    let reply = failing
        .open()
        .post("/api/reader/eject", &volume_body(&volume));
    assert_eq!(reply.status, 500);
    // Said as the owner should read it, without the category the command
    // line tags its errors with.
    assert_eq!(reply.error(), "a window is still using the volume");
}

#[test]
fn apps_are_listed_installed_updated_and_removed_through_the_signed_catalog() {
    let (running, desk) = running("apps");
    let page = running.open();
    let volume = desk.plug("reader", CLARA_BW, Some("0.3.37"));
    let asked = query_value(&volume.to_string_lossy());

    let installed = page.get(&format!("/api/apps?volume={asked}")).json();
    assert_eq!(installed["apps"], json!([]));
    assert_eq!(installed["reader"]["cobalt_version"], "0.3.37");
    assert!(
        desk.fetched.lock().expect("fetched").is_empty(),
        "no network"
    );

    let catalog = page.post("/api/apps/catalog", &volume_body(&volume)).json();
    let titles: Vec<&str> = catalog["apps"]
        .as_array()
        .expect("apps")
        .iter()
        .filter_map(|app| app["title"].as_str())
        .collect();
    assert_eq!(titles, ["Almanac", "Orrery", "Tide Table"]);
    assert_eq!(catalog["apps"][0]["installed_version"], Value::Null);
    assert_eq!(catalog["apps"][0]["runs_on_this_reader"], true);
    assert_eq!(catalog["apps"][1]["runs_on_this_reader"], false);
    assert_eq!(catalog["apps"][1]["minimum_cobalt_version"], "999.0.0");

    let body = json!({
        "volume": volume.to_string_lossy(),
        "ids": ["almanac", "orrery", "sundial"],
    })
    .to_string();
    let reply = page.post("/api/apps/install", &body);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    let said = report["text"].as_str().expect("text");
    assert!(said.contains("Installed Almanac 1.0.0."));
    assert!(
        said.contains("Orrery 2.0.0 needs Cobalt 999.0.0 or newer, and this reader has 0.3.37.")
    );
    assert!(said.contains("The catalog has no app with the id sundial."));
    assert_eq!(report["ok"], false, "two of the three failed");
    assert_eq!(report["title"], "Installing apps");

    let installed = page.get(&format!("/api/apps?volume={asked}")).json();
    assert_eq!(installed["apps"][0]["id"], "almanac");
    assert_eq!(installed["apps"][0]["installed_version"], "1.0.0");

    let body = json!({ "volume": volume.to_string_lossy(), "id": "almanac" }).to_string();
    let removed = page.post("/api/apps/remove", &body);
    assert_eq!(removed.status, 200, "{}", removed.body);
    let removed = removed.json();
    assert_eq!(removed["removed"]["title"], "Almanac");
    assert_eq!(removed["apps"], json!([]));
    let again = page.post("/api/apps/remove", &body);
    assert_eq!(again.status, 404);
    assert_eq!(again.kind(), "not_found");
    assert_eq!(again.error(), "almanac is not installed on this reader.");
}

#[test]
fn an_install_names_apps_by_valid_ids_and_a_reader_without_cobalt_has_no_apps() {
    let (running, desk) = running("apps-refused");
    let page = running.open();
    let volume = desk.plug("reader", CLARA_BW, Some("0.3.37"));
    for ids in [
        json!([]),
        json!(["../almanac"]),
        json!(["Almanac"]),
        json!(["almanac", 7]),
        json!("almanac"),
        json!(vec!["almanac"; 129]),
    ] {
        let body = json!({ "volume": volume.to_string_lossy(), "ids": ids }).to_string();
        assert_eq!(page.post("/api/apps/install", &body).status, 400, "{ids}");
    }
    let body = json!({ "volume": volume.to_string_lossy(), "id": "../../etc" }).to_string();
    assert_eq!(page.post("/api/apps/remove", &body).status, 400);
    assert!(desk.fetched.lock().expect("fetched").is_empty());

    let bare = desk.plug("bare", CLARA_BW, None);
    let asked = query_value(&bare.to_string_lossy());
    let reply = page.get(&format!("/api/apps?volume={asked}"));
    assert_eq!(reply.status, 409);
    assert!(reply
        .error()
        .starts_with("Cobalt is not installed on the reader"));
    let body = json!({ "volume": bare.to_string_lossy(), "ids": ["almanac"] }).to_string();
    assert_eq!(page.post("/api/apps/install", &body).status, 409);
}

#[test]
fn the_store_is_whole_with_no_reader_and_no_network_and_asks_nothing_of_either() {
    let (running, desk) = running("store");
    let page = running.open();
    desk.offline
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let reply = page.get("/api/store");

    assert_eq!(reply.status, 200, "{}", reply.body);
    let store = reply.json();
    assert_eq!(
        store["groups"],
        json!([
            { "id": "reading", "title": "Reading and listening", "apps": ["almanac"] },
            { "id": "everyday", "title": "Everyday tools", "apps": ["tide-table"] },
        ])
    );
    assert_eq!(store["apps"]["almanac"]["title"], "Almanac");
    assert_eq!(
        store["apps"]["almanac"]["shots"][1]["name"],
        "almanac/moon.png"
    );
    assert!(desk.fetched.lock().expect("fetched").is_empty());
    // The Mac app is answered the same.
    assert_eq!(running.app("GET", "/api/store", "").json(), store);
}

#[test]
fn a_picture_is_fetched_by_this_server_from_the_published_site_and_served_as_an_image() {
    let (running, desk) = running("shot");
    let page = running.open();
    let moon = picture(1072, 1448, 2);

    let reply = page.get("/api/shot?name=almanac%2Fmoon.png");

    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.header("content-type"), Some("image/png"));
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        reply.header("content-length"),
        Some(moon.len().to_string().as_str())
    );
    // The one address asked for is the listed name under the published
    // folder, and the copy kept is in the cache folder and nowhere else.
    assert_eq!(
        *desk.fetched.lock().expect("fetched"),
        [picture_address("almanac/moon.png")]
    );
    assert_eq!(
        fs::read(desk.root.join("cache/almanac_moon.png")).expect("kept"),
        moon
    );

    // Asked for again with the network gone, by either front end, it comes
    // from this computer.
    desk.offline
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(page.get("/api/shot?name=almanac/moon.png").status, 200);
    assert_eq!(
        running
            .app("GET", "/api/shot?name=almanac/moon.png", "")
            .status,
        200
    );
    assert_eq!(desk.fetched.lock().expect("fetched").len(), 1);

    // One that was never fetched cannot be, and says which kind of failure
    // that is so a front end can draw the frame empty.
    let reply = page.get("/api/shot?name=almanac.png");
    assert_eq!((reply.status, reply.kind().as_str()), (502, "unreachable"));
    assert!(!desk.root.join("cache/almanac.png").exists());
}

#[test]
fn a_picture_is_only_ever_one_the_store_lists_by_exactly_that_name() {
    let (running, desk) = running("shot-names");
    let page = running.open();
    // Published, so that only the lookup stands between these and a fetch.
    for name in ["secret.png", "almanac/sun.png"] {
        desk.pictures
            .lock()
            .expect("pictures")
            .insert(picture_address(name), picture(10, 10, 0));
    }
    let before = Desk::listing(&desk.root);

    for name in [
        "secret.png",
        "almanac/sun.png",
        "almanac/../almanac.png",
        "../almanac.png",
        "..%2F..%2F..%2Fetc%2Fpasswd",
        "%2Fetc%2Fpasswd",
        "ALMANAC.PNG",
        "almanac.png%00",
        "almanac.png%3Fx",
        "https:%2F%2Fexample.org%2Falmanac.png",
        "almanac_moon.png",
        "",
    ] {
        let reply = page.get(&format!("/api/shot?name={name}"));
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (404, "not_found"),
            "{name}"
        );
    }
    assert_eq!(page.get("/api/shot").status, 404);
    assert!(desk.fetched.lock().expect("fetched").is_empty());
    assert_eq!(Desk::listing(&desk.root), before);
}

#[test]
fn what_the_bus_and_the_system_say_about_readers_that_are_not_mounted_is_passed_on() {
    let (running, desk) = running("surroundings");
    let page = running.open();
    desk.plug("reader", CLARA_BW, None);
    // Two Kobos on the bus and one mounted: the other is waiting for
    // Connect. One drive the system is keeping from us.
    *desk.attached.lock().expect("attached") = Some(2);
    desk.blocked
        .lock()
        .expect("blocked")
        .push(PathBuf::from("/Volumes/KOBOeReader 1"));
    let state = page.get("/api/state").json();
    assert_eq!(state["attached"], 2);
    assert_eq!(state["readers"].as_array().expect("readers").len(), 1);
    assert_eq!(state["blocked"], json!(["/Volumes/KOBOeReader 1"]));
}

#[test]
fn every_refusal_says_what_kind_it_is() {
    let (running, desk) = running("kinds");
    let page = running.open();
    let bare = desk.plug("bare", CLARA_BW, None);
    let five = desk.plug("five", FIRMWARE_FIVE, None);
    let with = desk.plug("with", CLARA_BW, Some("0.3.37"));
    let volume = |path: &Path| {
        json!({ "volume": path.to_string_lossy(), "ids": ["almanac"], "id": "almanac" }).to_string()
    };
    let gone =
        json!({ "volume": "/Volumes/Unplugged", "ids": ["almanac"], "id": "almanac" }).to_string();

    let cases = [
        (page.post("/api/reader/install", &gone), 404, "gone"),
        (
            page.post("/api/reader/install", &volume(&five)),
            409,
            "refused",
        ),
        (
            page.post("/api/apps/catalog", &volume(&bare)),
            409,
            "no_cobalt",
        ),
        (
            page.post("/api/apps/remove", &volume(&with)),
            404,
            "not_found",
        ),
        (page.post("/api/reader/eject", "{}"), 400, "bad_request"),
        (
            page.post("/api/reader/eject", "not json"),
            400,
            "bad_request",
        ),
        (page.get("/api/job?id=99"), 404, "not_found"),
        (page.get("/api/nothing"), 404, "not_found"),
        (page.get("/api/reader/eject"), 405, "bad_request"),
        (
            page.post_bytes("/api/send/file?name=notes.xyz", b"?"),
            422,
            "unsupported_file",
        ),
        (
            page.post("/api/send/start", r#"{"upload":999}"#),
            404,
            "not_found",
        ),
        (
            page.post("/api/send/address", r#"{"url":"nope"}"#),
            400,
            "bad_request",
        ),
        (
            running.send(&format!(
                "GET /api/state HTTP/1.1\r\nHost: {}\r\n\r\n",
                running.host()
            )),
            401,
            "session",
        ),
        (
            running.send("GET /api/state HTTP/1.1\r\nHost: attacker.example\r\n\r\n"),
            403,
            "session",
        ),
    ];
    for (reply, status, kind) in &cases {
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (*status, *kind),
            "{}",
            reply.body
        );
        assert!(!reply.error().is_empty());
    }
}

#[test]
fn a_catalog_out_of_reach_an_altered_one_and_a_busy_reader_are_each_their_own_kind() {
    let (running, desk) = running("kinds-store");
    let page = running.open();
    let with = desk.plug("with", CLARA_BW, Some("0.3.37"));
    let body = volume_body(&with);

    desk.offline
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let unreachable = page.post("/api/apps/catalog", &body);
    assert_eq!(
        (unreachable.status, unreachable.kind().as_str()),
        (502, "unreachable")
    );
    assert!(unreachable.error().contains("could not be reached"));
    desk.offline
        .store(false, std::sync::atomic::Ordering::SeqCst);
    desk.tampered
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let altered = page.post("/api/apps/catalog", &body);
    assert_eq!(
        (altered.status, altered.kind().as_str()),
        (502, "integrity")
    );
    assert!(altered.error().contains("Nothing was changed"));
    assert!(
        !with.join(".adds/cobalt/store").join("catalog").exists(),
        "an altered catalog is not kept"
    );

    let (release, held) = std::sync::mpsc::channel::<()>();
    running
        .shared
        .jobs
        .start(
            Kind::Reader,
            "Updating Cobalt",
            None,
            move |_| {
                let _ = held.recv();
                Finish {
                    ok: true,
                    code: None,
                }
            },
            || {},
        )
        .expect("started");
    let busy = page.post("/api/reader/remove", &body);
    assert_eq!((busy.status, busy.kind().as_str()), (409, "busy"));
    release.send(()).expect("let the job end");
}

#[test]
fn nothing_is_started_on_a_card_that_is_mounted_read_only() {
    let (running, desk) = running("read-only");
    let page = running.open();
    let volume = desk.plug("reader", CLARA_BW, Some("0.3.37"));
    desk.room.lock().expect("room").writable = Some(false);
    let state = page.get("/api/state").json();
    assert_eq!(state["readers"][0]["writable"], false);
    let body = json!({ "volume": volume.to_string_lossy(), "ids": ["almanac"], "id": "almanac" })
        .to_string();
    for path in [
        "/api/reader/install",
        "/api/reader/remove",
        "/api/apps/catalog",
        "/api/apps/install",
        "/api/apps/remove",
    ] {
        let reply = page.post(path, &body);
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (409, "read_only"),
            "{path}"
        );
        assert!(reply.error().contains("mounted read-only"));
    }
    assert!(desk.commands().is_empty());
    assert!(desk.fetched.lock().expect("fetched").is_empty());
    assert!(running.shared.jobs.summaries().is_empty());
    // Reading what is on it is still answered, and so is ejecting it,
    // which is the way out.
    let asked = query_value(&volume.to_string_lossy());
    assert_eq!(page.get(&format!("/api/apps?volume={asked}")).status, 200);
    assert_eq!(page.post("/api/reader/eject", &body).status, 200);
    // A system that cannot say whether the card takes writes is not
    // treated as one that says it does not.
    desk.room.lock().expect("room").writable = None;
    assert_eq!(page.post("/api/apps/catalog", &body).status, 200);
}

#[test]
fn an_app_is_listed_with_its_size_and_is_not_installed_where_it_does_not_fit() {
    let (running, desk) = running("fits");
    let page = running.open();
    let volume = desk.plug("reader", CLARA_BW, Some("0.3.37"));
    let body = volume_body(&volume);
    let asked = query_value(&volume.to_string_lossy());

    // Before any catalog has been fetched for this reader nothing is known
    // about sizes, and nothing is claimed.
    let catalog = page.post("/api/apps/catalog", &body).json();
    let almanac = &catalog["apps"][0];
    assert_eq!(almanac["bytes"], "almanac 1.0.0 binary".len());
    assert_eq!(almanac["fits"], true);

    desk.room.lock().expect("room").free = Some(100_000);
    let tight = page.post("/api/apps/catalog", &body).json();
    assert_eq!(tight["apps"][0]["fits"], false);
    let reply = page.post(
        "/api/apps/install",
        &json!({ "volume": volume.to_string_lossy(), "ids": ["almanac"] }).to_string(),
    );
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["ok"], false);
    assert_eq!(report["volume"], volume.to_string_lossy().as_ref());
    assert!(report["text"]
        .as_str()
        .expect("text")
        .contains("Almanac needs 0.0 MB and the reader has 0.1 MB free. Remove something from the reader and try again."));
    assert_eq!(
        page.get(&format!("/api/apps?volume={asked}")).json()["apps"],
        json!([])
    );

    // A system that cannot say how much is free leaves the question open.
    desk.room.lock().expect("room").free = None;
    let unknown = page.post("/api/apps/catalog", &body).json();
    assert_eq!(unknown["apps"][0]["fits"], Value::Null);
    assert_eq!(unknown["apps"][0]["bytes"], "almanac 1.0.0 binary".len());
}

#[test]
fn the_apps_on_a_reader_are_left_alone_while_something_is_writing_to_it() {
    let (running, desk) = running("apps-busy");
    let page = running.open();
    let volume = desk.plug("reader", CLARA_BW, Some("0.3.37"));
    let (release, held) = std::sync::mpsc::channel::<()>();
    running
        .shared
        .jobs
        .start(
            Kind::Reader,
            "Updating Cobalt",
            None,
            move |_| {
                let _ = held.recv();
                Finish {
                    ok: true,
                    code: None,
                }
            },
            || {},
        )
        .expect("started");
    let asked = query_value(&volume.to_string_lossy());
    let body = json!({
        "volume": volume.to_string_lossy(),
        "ids": ["almanac"],
        "id": "almanac",
    })
    .to_string();
    let listing = page.get(&format!("/api/apps?volume={asked}"));
    assert_eq!(listing.status, 409);
    assert_eq!(
        listing.error(),
        "Updating Cobalt has not finished yet. Wait for it, then try again."
    );
    for path in [
        "/api/apps/catalog",
        "/api/apps/install",
        "/api/apps/remove",
        "/api/reader/install",
        "/api/reader/remove",
        "/api/reader/eject",
    ] {
        assert_eq!(page.post(path, &body).status, 409, "{path}");
    }
    assert!(desk.fetched.lock().expect("fetched").is_empty());
    assert!(desk.ejected.lock().expect("ejected").is_empty());
    // The state is still answered: it reads two small files and no store.
    assert_eq!(page.get("/api/state").status, 200);
    release.send(()).expect("let the job end");
}

/// The folder a browser keeps its profiles in on this system, under a home.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn chrome_profiles(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Google/Chrome")
    } else {
        home.join(".config/google-chrome")
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn each_browser_found_is_listed_with_the_one_step_it_still_needs() {
    let (running, desk) = running("browsers");
    let page = running.open();
    let none = page.get("/api/browsers").json();
    let found: Vec<&str> = none["browsers"]
        .as_array()
        .expect("browsers")
        .iter()
        .filter_map(|browser| browser["id"].as_str())
        .collect();
    // Safari is found by its application, not by anything under the home.
    assert!(found.iter().all(|id| *id == "safari"), "{found:?}");

    std::fs::create_dir_all(chrome_profiles(&desk.root.join("home"))).expect("profiles");
    let listed = page.get("/api/browsers").json();
    let chrome = listed["browsers"]
        .as_array()
        .expect("browsers")
        .iter()
        .find(|browser| browser["id"] == "chrome")
        .expect("Chrome is found")
        .clone();
    assert_eq!(chrome["title"], "Chrome");
    assert_eq!(chrome["set_up"], false);
    let step = chrome["step"].as_str().expect("a step");
    assert!(step.starts_with("Chrome: open chrome://extensions, turn on Developer mode"));
    assert!(step.contains("config/clip/extension"));

    // Set up is told from the file the browser reads, where setup puts it.
    let manifest = clip_install::registration(
        Browser::Chrome,
        Os::current(),
        &desk.root.join("home"),
        &desk.root.join("config"),
    );
    let Some(Registration::File(manifest)) = manifest else {
        panic!("Chrome reads a file on this system");
    };
    std::fs::create_dir_all(manifest.parent().expect("folder")).expect("folder");
    std::fs::write(&manifest, b"{}").expect("manifest");
    let listed = page.get("/api/browsers").json();
    let chrome = listed["browsers"]
        .as_array()
        .expect("browsers")
        .iter()
        .find(|browser| browser["id"] == "chrome")
        .expect("Chrome")
        .clone();
    assert_eq!(chrome["set_up"], true);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn setting_up_a_browser_runs_clip_install_for_that_browser_and_no_other_word() {
    let (running, desk) = running("browsers-setup");
    let page = running.open();
    std::fs::create_dir_all(chrome_profiles(&desk.root.join("home"))).expect("profiles");

    let reply = page.post("/api/browsers/setup", r#"{"browser":"chrome"}"#);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let answer = reply.json();
    assert!(answer["said"]
        .as_str()
        .expect("what the command said")
        .contains("ran: clip install --browser chrome"));
    assert!(answer["browsers"].is_array());
    let reply = page.post("/api/browsers/remove", r#"{"browser":"chrome"}"#);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        desk.commands(),
        [
            "clip install --browser chrome",
            "clip uninstall --browser chrome"
        ]
    );

    // A browser that is not here, a name that is not a browser, and a name
    // dressed as an option are all refused before anything is run.
    for browser in ["firefox", "netscape", "--all", "chrome --all", ""] {
        let body = json!({ "browser": browser }).to_string();
        assert_eq!(
            page.post("/api/browsers/setup", &body).status,
            400,
            "{browser}"
        );
        assert_eq!(
            page.post("/api/browsers/remove", &body).status,
            400,
            "{browser}"
        );
    }
    assert_eq!(desk.commands().len(), 2);

    // A command that fails is answered with what it said.
    std::fs::write(desk.root.join("kobo.status"), "1").expect("status");
    let reply = page.post("/api/browsers/setup", r#"{"browser":"chrome"}"#);
    assert_eq!(reply.status, 500);
    assert!(reply.error().contains("kobo: said on the error stream"));
}

#[test]
fn a_received_file_is_kept_under_a_safe_name_and_matched_to_the_app_that_opens_it() {
    let (running, desk) = running("receive");
    let page = running.open();
    let comic = page
        .post_bytes("/api/send/file?name=Winter+Vol.+1.cbz", b"comic bytes")
        .json();
    assert_eq!(comic["name"], "Winter Vol. 1.cbz");
    assert_eq!(comic["bytes"], 11);
    assert_eq!(comic["app"]["name"], "panels");
    assert_eq!(comic["app"]["title"], "Panels");
    assert_eq!(comic["app"]["replaces"], "comic");
    assert_eq!(comic["choices"], Value::Null);

    // A PDF is a score or a pattern, so the page is given the choice.
    let pdf = page
        .post_bytes("/api/send/file?name=Sonata.pdf", b"%PDF-1.4")
        .json();
    assert_eq!(pdf["app"], Value::Null);
    let names: Vec<&str> = pdf["choices"]
        .as_array()
        .expect("choices")
        .iter()
        .filter_map(|choice| choice["name"].as_str())
        .collect();
    assert_eq!(names, ["musicstand", "needles"]);
    assert!(pdf["choices"][0]["purpose"].is_string());
    assert_eq!(pdf["choices"][0]["replaces"], Value::Null);

    // A name that tries to leave its folder, hide, or pass as an option
    // arrives as a plain file name in the folder made for it.
    let scratch = desk.root.join("scratch");
    for (offered, kept) in [
        ("..%2F..%2F..%2Fetc%2Fnotes.md", "notes.md"),
        ("C%3A%5CUsers%5Cme%5Cnotes.md", "notes.md"),
        ("--replace.md", "replace.md"),
        (".hidden.md", "hidden.md"),
        ("a%00b%0Ac.md", "a_b_c.md"),
    ] {
        let reply = page.post_bytes(&format!("/api/send/file?name={offered}"), b"# note");
        assert_eq!(reply.status, 200, "{offered}: {}", reply.body);
        assert_eq!(reply.json()["name"], kept, "{offered}");
    }
    for entry in Desk::listing(&desk.root) {
        if entry.contains("notes.md") || entry.contains("replace.md") {
            assert!(entry.starts_with(&scratch.display().to_string()), "{entry}");
        }
    }

    // A kind of file no app opens is refused in the command's own words and
    // not kept.
    let unknown = page.post_bytes("/api/send/file?name=notes.xyz", b"?");
    assert_eq!(unknown.status, 422);
    assert!(unknown
        .error()
        .starts_with("No app opens this kind of file."));
    assert!(!Desk::listing(&scratch)
        .iter()
        .any(|entry| entry.contains("notes.xyz")));

    // Only the newest few are kept; the oldest were deleted as more came.
    let folders: Vec<String> = std::fs::read_dir(&scratch)
        .expect("scratch")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    assert_eq!(folders.len(), MOST_UPLOADS, "{folders:?}");
    let waiting = running.shared.machine.uploads.lock().expect("uploads");
    assert_eq!(waiting.waiting.len(), MOST_UPLOADS);
    assert!(waiting
        .waiting
        .iter()
        .all(|upload| Path::new(&upload.name).extension() == Some("md".as_ref())));
    assert!(!Desk::listing(&scratch)
        .iter()
        .any(|entry| entry.contains("Winter") || entry.contains("Sonata")));
}

#[cfg(unix)]
#[test]
fn a_received_file_is_sent_by_name_from_its_own_folder_and_then_deleted() {
    let (running, desk) = running("send");
    let page = running.open();
    std::fs::write(
        desk.root.join("config/readers"),
        "reader = clara\nserial = N365410043013\naddress = 192.168.1.20\npaired = yes\n",
    )
    .expect("saved readers");
    let upload = page
        .post_bytes("/api/send/file?name=Winter.cbz", b"comic bytes")
        .json()["upload"]
        .as_u64()
        .expect("an upload");
    let scratch = desk.root.join("scratch");
    assert!(Desk::listing(&scratch)
        .iter()
        .any(|entry| entry.ends_with("Winter.cbz 11")));

    let body = json!({ "upload": upload, "to": "panels", "replace": true, "reader": "clara" });
    let reply = page.post("/api/send/start", &body.to_string());
    assert_eq!(reply.status, 200, "{}", reply.body);
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["title"], "Sending Winter.cbz");
    assert_eq!(report["kind"], "send");
    let said = report["text"].as_str().expect("text");
    assert!(said.contains("ran: send Winter.cbz --to panels --replace --reader clara"));
    // The command ran where the file was, so the file was beside it.
    assert!(said.contains("beside: Winter.cbz"));
    assert!(
        !Desk::listing(&scratch)
            .iter()
            .any(|entry| entry.contains("Winter")),
        "the file is deleted once it has been sent"
    );
    // It was the page's to send once.
    assert_eq!(page.post("/api/send/start", &body.to_string()).status, 404);

    // With nothing chosen, nothing is added to the line.
    let upload = page
        .post_bytes("/api/send/file?name=note.md", b"# note")
        .json()["upload"]
        .clone();
    let reply = page.post("/api/send/start", &json!({ "upload": upload }).to_string());
    page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(desk.commands().last().expect("a run"), "send note.md");
}

#[test]
fn a_send_names_only_an_app_that_takes_files_and_a_reader_that_was_saved() {
    let (running, desk) = running("send-refused");
    let page = running.open();
    let upload = page
        .post_bytes("/api/send/file?name=note.md", b"# note")
        .json()["upload"]
        .clone();
    for body in [
        json!({ "upload": upload, "to": "--replace" }),
        json!({ "upload": upload, "to": "terminal" }),
        json!({ "upload": upload, "reader": "nobody" }),
        json!({ "upload": upload, "reader": "--device" }),
    ] {
        let reply = page.post("/api/send/start", &body.to_string());
        assert_eq!(reply.status, 400, "{body}");
    }
    assert_eq!(
        page.post("/api/send/start", &json!({ "upload": 999 }).to_string())
            .status,
        404
    );
    assert!(desk.commands().is_empty());
    // The refusals did not use the file up.
    assert!(Desk::listing(&desk.root.join("scratch"))
        .iter()
        .any(|entry| entry.ends_with("note.md 6")));
}

#[cfg(unix)]
#[test]
fn a_web_address_is_sent_with_clip_and_anything_that_is_not_one_is_refused() {
    let (running, desk) = running("address");
    let page = running.open();
    for address in [
        "",
        "example.org/article",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "--out=/tmp/x",
        "-https://example.org",
        "https://",
        "https://example.org/a b",
        "https://example.org/a\nsync",
        "ftp://example.org/file",
    ] {
        let reply = page.post("/api/send/address", &json!({ "url": address }).to_string());
        assert_eq!(reply.status, 400, "{address:?}");
    }
    assert_eq!(page.post("/api/send/address", "{}").status, 400);
    assert!(desk.commands().is_empty());

    let body = json!({ "url": "  https://example.org/a?b=c&d=e#f  " }).to_string();
    let reply = page.post("/api/send/address", &body);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let report = page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(report["title"], "Sending an article");
    assert_eq!(desk.commands(), ["clip https://example.org/a?b=c&d=e#f"]);

    let reply = page.post("/api/send/queue", "{}");
    page.finished(reply.json()["job"].as_u64().expect("a job"));
    assert_eq!(desk.commands().last().expect("a run"), "clip sync");
}

#[test]
fn the_send_place_lists_the_saved_readers_and_what_is_waiting_for_one() {
    let (running, desk) = running("sending");
    let page = running.open();
    let empty = page.get("/api/send").json();
    assert_eq!(empty["readers"], json!([]));
    assert_eq!(empty["waiting"], json!([]));

    std::fs::write(
        desk.root.join("config/readers"),
        "reader = clara\nserial = N365410043013\naddress = 192.168.1.20\npaired = yes\n",
    )
    .expect("saved readers");
    let queue = desk.root.join("config/clip/queue");
    std::fs::create_dir_all(&queue).expect("queue");
    std::fs::write(
        queue.join("1-1-1.json"),
        r#"{"title":"On Paper","file_name":"on-paper.epub","site":"example.org"}"#,
    )
    .expect("record");
    std::fs::write(queue.join("1-1-1.epub"), b"book").expect("book");
    let reply = page.get("/api/send");
    assert!(!reply.body.contains("N365410043013"));
    assert!(!reply.body.contains("192.168.1.20"));
    let listed = reply.json();
    assert_eq!(
        listed["readers"],
        json!([{ "name": "clara", "paired": true }])
    );
    assert_eq!(
        listed["waiting"],
        json!([{ "title": "On Paper", "site": "example.org" }])
    );
}

#[test]
fn a_job_that_is_not_remembered_is_not_found() {
    let (running, _desk) = running("job-missing");
    let page = running.open();
    for query in ["", "?id=", "?id=abc", "?id=7", "?id=-1"] {
        assert_eq!(page.get(&format!("/api/job{query}")).status, 404, "{query}");
    }
}

#[test]
fn a_file_name_keeps_its_extension_and_loses_everything_that_could_do_harm() {
    assert_eq!(safe_name("Winter.cbz"), "Winter.cbz");
    assert_eq!(safe_name("/home/me/Winter.cbz"), "Winter.cbz");
    assert_eq!(safe_name("..\\..\\Winter.cbz"), "Winter.cbz");
    assert_eq!(safe_name("-rf"), "rf");
    assert_eq!(safe_name("--to=panels.cbz"), "to=panels.cbz");
    assert_eq!(safe_name("...cbz"), "cbz");
    assert_eq!(safe_name(""), "file");
    assert_eq!(safe_name(" . "), "file");
    assert_eq!(safe_name("a/"), "file");
    assert_eq!(safe_name("what?*<>.md"), "what____.md");
    let long = format!("{}.epub", "n".repeat(400));
    let kept = safe_name(&long);
    assert_eq!(kept.chars().count(), 96 + ".epub".len());
    assert_eq!(Path::new(&kept).extension(), Some("epub".as_ref()));
    assert_eq!(safe_name("caf\u{e9}.md"), "caf\u{e9}.md");
}

#[test]
fn only_a_web_address_passes_as_one() {
    assert_eq!(
        web_address(" https://example.org/a "),
        Some("https://example.org/a")
    );
    assert_eq!(
        web_address("HTTP://example.org"),
        Some("HTTP://example.org")
    );
    for other in [
        "https://",
        "http:/example.org",
        "//example.org",
        "-x",
        "https://a b",
        "https://a\tb",
        "mailto:a@example.org",
    ] {
        assert_eq!(web_address(other), None, "{other:?}");
    }
    assert_eq!(
        web_address(&format!("https://e.org/{}", "a".repeat(2048))),
        None
    );
}

#[test]
fn a_reader_stands_by_how_its_cobalt_compares_with_this_computers_release() {
    let release = "0.3.37".to_owned();
    let none = "no release".to_owned();
    assert_eq!(standing(None, Ok(&release)), "absent");
    assert_eq!(standing(None, Err(&none)), "absent");
    assert_eq!(standing(Some("0.3.37"), Ok(&release)), "current");
    assert_eq!(standing(Some("0.3.9"), Ok(&release)), "behind");
    assert_eq!(standing(Some("0.3.38"), Ok(&release)), "ahead");
    assert_eq!(standing(Some("0.10.0"), Ok(&release)), "ahead");
    assert_eq!(standing(Some("0.3.37"), Err(&none)), "unknown");
}

#[test]
fn a_plan_wrapped_for_a_terminal_is_one_entry_per_change_again() {
    assert_eq!(
        joined("would do one thing\n  and carry on\n   further\nwould do another\n"),
        [
            "would do one thing and carry on further",
            "would do another"
        ]
    );
    assert!(joined("").is_empty());
}
