//! Host-process runtime simulation with real launcher/apps and shared policy.
//! Device sandboxing, kernel suspend and physical hardware remain uncalibrated.
pub(super) mod power;
use crate::{AppServer, AppSession, CaptureSource, Message};
use kobo_protocol::Lifecycle;
use kobod::navigation::{self, BackRoute};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Program {
    pub executable: PathBuf,
    pub source: CaptureSource,
}

struct Hosted {
    id: u64,
    name: String,
    child: Child,
    session: AppSession,
    used: Instant,
}
impl Drop for Hosted {
    fn drop(&mut self) {
        self.session.writer.close();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl AppSession {
    pub(super) fn route_input(&self, message: &Message) -> io::Result<bool> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("app state unavailable"))?;
        if !state.runtime_navigation {
            return Ok(true);
        }
        if state.lifecycle == Lifecycle::Background {
            return Ok(false);
        }
        if state.power_state != kobod::power::State::Awake {
            state.power_request = Some(power::Input::Wake(kobod::power::WakeReason::Touch));
            return Ok(false);
        }
        let is_back =
            matches!(message, Message::Action { action } if *action == kobo_ui::ActionId::BACK);
        match navigation::route(is_back, state.screen.owns_back) {
            BackRoute::Deliver => Ok(true),
            BackRoute::Offer => {
                let millis = state.time.now()?.monotonic_millis;
                state.back_offer.offer(1, millis);
                Ok(true)
            }
            BackRoute::Leave => {
                state.back_offer.clear();
                state.navigation.clear();
                state.navigation.push_back("launcher".into());
                Ok(false)
            }
        }
    }

    fn next_launch(&self) -> io::Result<Option<String>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("app state unavailable"))?;
        if state.lifecycle == Lifecycle::Background {
            state.navigation.clear();
            state.back_offer.clear();
            return Ok(None);
        }
        let millis = state.time.now()?.monotonic_millis;
        if state.back_offer.take_expired(1, millis) {
            state.navigation.clear();
            state.navigation.push_back("launcher".into());
            state.record("Back deadline reached; returning to launcher".into());
        }
        Ok(state.navigation.pop_front())
    }
}

fn start(
    server: &AppServer,
    socket: &Path,
    name: &str,
    program: &Program,
    id: u64,
) -> io::Result<Hosted> {
    let mut child = Command::new(&program.executable)
        .env("KOBO_SOCKET", socket)
        .env("KOBO_SIM_CALLBACKS", "1")
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let accepted = (|| loop {
        if let Some(session) = server.try_accept_app()? {
            {
                let mut state = session
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("app state unavailable"))?;
                if state.app_name != name {
                    session.writer.close();
                    return Err(io::Error::other(
                        "launched app reported a different identity",
                    ));
                }
                state.capture_source = program.source.clone();
                state.process_id = Some(child.id());
            }
            return Ok(session);
        }
        if child.try_wait()?.is_some() || Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "{name} did not connect within 10 seconds"
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    })();
    match accepted {
        Ok(session) => Ok(Hosted {
            id,
            name: name.into(),
            child,
            session,
            used: Instant::now(),
        }),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

fn switch(apps: &mut [Hosted], front: u64, wanted: u64) -> io::Result<u64> {
    let Some(next) = apps.iter().position(|app| app.id == wanted) else {
        return Ok(front);
    };
    if front == wanted {
        return Ok(front);
    }
    if let Some(previous) = apps.iter().position(|app| app.id == front) {
        if apps[previous].session.writer.connected() {
            apps[previous]
                .session
                .send_lifecycle(Lifecycle::Background)?;
        }
        // Move the one panel's history into the foreground app. Background
        // screens are retained without performing physical/panel submissions.
        let mut old = apps[previous]
            .session
            .state
            .lock()
            .map_err(|_| io::Error::other("app state unavailable"))?;
        let mut target = apps[next]
            .session
            .state
            .lock()
            .map_err(|_| io::Error::other("app state unavailable"))?;
        target.panel = std::mem::replace(&mut old.panel, crate::panel::PanelPreview::new());
        target.hardware = old.hardware;
        target.scenario = old.scenario;
        target.observe_hardware();
        old.back_offer.clear();
        old.navigation.clear();
    }
    apps[next].used = Instant::now();
    apps[next].session.send_lifecycle(Lifecycle::Foreground)?;
    {
        let mut state = apps[next]
            .session
            .state
            .lock()
            .map_err(|_| io::Error::other("app state unavailable"))?;
        state.update_chrome();
        state.commit_frame();
        state.record(format!("foreground: {}", apps[next].name));
    }
    Ok(wanted)
}

fn make_room(apps: &mut Vec<Hosted>, front: u64) -> io::Result<()> {
    if apps.len() < navigation::MAX_HOSTED {
        return Ok(());
    }
    let seen = apps
        .iter()
        .map(|app| {
            let tasks = app
                .session
                .state
                .lock()
                .ok()
                .and_then(|state| state.tasks.clone());
            let busy = tasks
                .and_then(|tasks| tasks.lock().ok().map(|tasks| tasks.in_flight() > 0))
                .unwrap_or(true);
            (app.id, app.used, busy)
        })
        .collect::<Vec<_>>();
    let index = navigation::eviction(&seen, front, Some(1))
        .ok_or_else(|| io::Error::other("all process slots are protected"))?;
    apps.remove(index);
    Ok(())
}

/// Run real SDK programs against the simulated HAL/services. Programs are
/// explicitly selected host binaries; their existence does not grant device
/// trust or bypass SDK capability declarations. Exit ends only these children.
///
/// # Errors
/// Refuses missing launcher, invalid identities, failed starts or lost HTTP IPC.
pub fn run(
    server: AppServer,
    socket: &Path,
    programs: &BTreeMap<String, Program>,
) -> io::Result<()> {
    let server = server.with_runtime_navigation();
    let home_program = programs
        .get("launcher")
        .ok_or_else(|| io::Error::other("runtime simulation needs a launcher"))?;
    if programs.keys().any(|name| !crate::valid_app_name(name)) {
        return Err(io::Error::other("invalid simulator program identity"));
    }
    server.set_nonblocking(true)?;
    prepare_catalog(&server, programs)?;
    let mut apps = vec![start(&server, socket, "launcher", home_program, 1)?];
    let mut front = switch(&mut apps, 0, 1)?;
    let mut next_id = 2_u64;
    let mut power = power::Controller::default();
    println!("Kobo runtime simulator: http://{}", server.local_addr()?);
    loop {
        let hosted = apps.iter().map(|app| app.name.clone()).collect::<Vec<_>>();
        for app in &apps {
            app.session
                .state
                .lock()
                .map_err(|_| io::Error::other("app state unavailable"))?
                .hosted
                .clone_from(&hosted);
        }
        let Some(index) = apps.iter().position(|app| app.id == front) else {
            return Err(io::Error::other("foreground app lost"));
        };
        server.try_serve_one(&apps[index].session)?;
        power.step(&mut apps, front)?;
        if power.awake() {
            if let Some(name) = apps[index].session.next_launch()? {
                let available = {
                    let catalog = server
                        .apps
                        .lock()
                        .map_err(|_| io::Error::other("catalog unavailable"))?;
                    catalog
                        .catalog
                        .iter()
                        .find(|entry| entry.id == name)
                        .is_none_or(kobo_protocol::AppInfo::is_installed)
                };
                let launched = if !available {
                    Err(io::Error::other(format!(
                        "{name} is not installed in this simulation"
                    )))
                } else if let Some(id) = apps.iter().find(|app| app.name == name).map(|app| app.id)
                {
                    Ok(id)
                } else if let Some(program) = programs.get(&name) {
                    let id = next_id;
                    next_id = next_id
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("session identities exhausted"))?;
                    make_room(&mut apps, front)
                        .and_then(|()| start(&server, socket, &name, program, id))
                        .map(|app| {
                            apps.push(app);
                            id
                        })
                } else {
                    Err(io::Error::other(format!(
                        "{name} is not included in this simulation"
                    )))
                };
                match launched {
                    Ok(wanted) => front = switch(&mut apps, front, wanted)?,
                    Err(error) => {
                        eprintln!("Could not open {name}: {error}");
                        if let Some(app) = apps.iter().find(|app| app.id == front) {
                            app.session
                                .state
                                .lock()
                                .map_err(|_| io::Error::other("app state unavailable"))?
                                .record(format!("Could not open {name}: {error}"));
                        }
                    }
                }
            }
        }
        let mut gone = Vec::new();
        for app in &mut apps {
            if app.child.try_wait()?.is_some() || !app.session.writer.connected() {
                gone.push(app.id);
            }
        }
        if gone.contains(&1) {
            return Ok(());
        }
        if !gone.is_empty() {
            power.cancel(&mut apps)?;
        }
        if gone.contains(&front) {
            front = switch(&mut apps, front, 1)?;
        }
        apps.retain(|app| !gone.contains(&app.id));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn prepare_catalog(server: &AppServer, programs: &BTreeMap<String, Program>) -> io::Result<()> {
    {
        let mut apps = server
            .apps
            .lock()
            .map_err(|_| io::Error::other("catalog unavailable"))?;
        if apps.signed.is_some() {
            return Err(io::Error::other("signed Store fixtures run in single-app mode; runtime mode launches selected local builds"));
        }
        {
            for entry in &mut apps.catalog {
                entry.installed_version = programs
                    .contains_key(&entry.id)
                    .then(|| entry.version.clone());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    #[test]
    fn owned_back_is_delivered_and_only_its_deadline_or_answer_ends_the_offer() {
        let (mut peer, stream) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let clock = Arc::new(
            kobo_policy::clock::ManualClock::new(kobo_policy::clock::Snapshot {
                unix_millis: 0,
                monotonic_millis: 0,
                utc_offset_minutes: 0,
            })
            .unwrap(),
        );
        let state = crate::AppState {
            runtime_navigation: true,
            time: crate::clock::Time::Manual(clock),
            ..crate::AppState::default()
        };
        let session = AppSession {
            state: Arc::new(Mutex::new(state)),
            writer: crate::AppWriter::spawn_for(stream, kobo_protocol::VERSION),
        };
        session.send_action(kobo_ui::ActionId::BACK).unwrap();
        assert_eq!(session.next_launch().unwrap().as_deref(), Some("launcher"));
        assert!(kobo_protocol::read_from(&mut peer).is_err());
        session.state.lock().unwrap().screen.owns_back = true;
        session.send_action(kobo_ui::ActionId::BACK).unwrap();
        assert!(
            matches!(kobo_protocol::read_from(&mut peer).unwrap().message, Message::Action { action } if action == kobo_ui::ActionId::BACK)
        );
        session
            .state
            .lock()
            .unwrap()
            .time
            .change("advance 1999")
            .unwrap();
        session.send_action(kobo_ui::ActionId::BACK).unwrap();
        kobo_protocol::read_from(&mut peer).unwrap();
        assert_eq!(session.next_launch().unwrap(), None);
        session
            .state
            .lock()
            .unwrap()
            .time
            .change("advance 1")
            .unwrap();
        assert_eq!(session.next_launch().unwrap().as_deref(), Some("launcher"));
        assert_eq!(session.next_launch().unwrap(), None);
        session.send_action(kobo_ui::ActionId::BACK).unwrap();
        kobo_protocol::read_from(&mut peer).unwrap();
        {
            let mut state = session.state.lock().unwrap();
            state.set_screen(kobo_ui::Screen::new(2, vec![]));
            state.time.change("advance 2000").unwrap();
        }
        assert_eq!(session.next_launch().unwrap(), None);
        session.writer.close();
    }

    #[test]
    fn background_screens_do_not_submit_panel_updates() {
        let mut state = crate::AppState {
            runtime_navigation: true,
            lifecycle: Lifecycle::Background,
            ..crate::AppState::default()
        };
        let before = state.panel.state_json();
        state.set_screen(kobo_ui::Screen::new(
            200,
            vec![kobo_ui::Node::Heading {
                id: kobo_ui::NodeId(1),
                text: "Background work".into(),
                level: 1,
            }],
        ));
        assert_eq!(state.panel.state_json(), before);
        assert_eq!(state.screen.id, 200);
        state.lifecycle = Lifecycle::Foreground;
        state.commit_frame();
        assert_ne!(state.panel.state_json(), before);
    }
}
