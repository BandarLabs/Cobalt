use kobo_sdk::{
    ActionId, AppRunner, Client, ClientEvent, Command, Context, DeviceRequest, DeviceResult,
    KoboApp, Node, NodeId, Screen,
};
use std::env;
use std::process::ExitCode;

const INCREMENT: ActionId = ActionId(1);

struct Counter {
    value: u32,
    battery: Option<String>,
}

impl Counter {
    fn show(&self, context: &mut Context) {
        context.set_screen(Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "Counter".into(),
                },
                Node::Text {
                    id: NodeId(2),
                    text: format!("Value: {}", self.value),
                },
                Node::Text {
                    id: NodeId(3),
                    text: self
                        .battery
                        .clone()
                        .unwrap_or_else(|| "Battery: asking...".into()),
                },
                Node::Button {
                    id: NodeId(4),
                    action: INCREMENT,
                    label: "Increment".into(),
                },
            ],
        ));
    }
}

impl KoboApp for Counter {
    fn on_start(&mut self, context: &mut Context) {
        // Hardware is asked for, never touched. The answer arrives below.
        context.device().read_battery();
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == INCREMENT {
            self.value = self.value.saturating_add(1);
            self.show(context);
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if request != DeviceRequest::ReadBattery {
            return;
        }
        self.battery = Some(match result {
            DeviceResult::Battery { percent, charging } => {
                let state = if charging { ", charging" } else { "" };
                format!("Battery: {percent}%{state}")
            }
            DeviceResult::Denied(reason) => format!("Battery unavailable: {reason}"),
            _ => "Battery: unexpected answer".into(),
        });
        self.show(context);
    }
}

fn main() -> ExitCode {
    let mut app = AppRunner::new(Counter {
        value: 0,
        battery: None,
    });
    let initial_screen = app.start();
    let Some(socket) = env::var_os("KOBO_SOCKET") else {
        let _incremented_screen = app.action(INCREMENT);
        return ExitCode::SUCCESS;
    };
    let mut client = match Client::connect(socket, "dev.example.counter") {
        Ok(client) => client,
        Err(error) => {
            eprintln!("counter: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = client.send_commands(initial_screen) {
        eprintln!("counter: {error}");
        return ExitCode::FAILURE;
    }
    if env::var_os("KOBO_SIM_ONESHOT").is_some() {
        // Every device request is answered exactly once, so a clean exit means
        // collecting the answers rather than abandoning them mid-flight.
        while app.outstanding_requests() > 0 {
            match client.next_event() {
                Ok(ClientEvent::Task { .. }) => {
                    // This example starts no tasks, so an outcome here would mean
                    // the runtime invented one. Ignoring it keeps the loop honest
                    // without pretending to handle work that was never submitted.
                }
                Ok(ClientEvent::Device(result)) => {
                    if let Err(error) = client.send_commands(app.device_result(result)) {
                        eprintln!("counter: {error}");
                        return ExitCode::FAILURE;
                    }
                }
                Ok(_) => break,
                Err(error) => {
                    eprintln!("counter: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        if let Err(error) = client.send_commands([Command::Exit]) {
            eprintln!("counter: {error}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }
    loop {
        match client.next_event() {
            Ok(ClientEvent::Action(action)) => {
                let commands = app.action(action);
                let exiting = commands
                    .iter()
                    .any(|command| matches!(command, Command::Exit));
                if let Err(error) = client.send_commands(commands) {
                    eprintln!("counter: {error}");
                    return ExitCode::FAILURE;
                }
                if exiting {
                    return ExitCode::SUCCESS;
                }
            }
            Ok(ClientEvent::Task { .. }) => {
                // This example starts no tasks, so an outcome here would mean
                // the runtime invented one. Ignoring it keeps the loop honest
                // without pretending to handle work that was never submitted.
            }
            Ok(ClientEvent::Device(result)) => {
                let commands = app.device_result(result);
                if let Err(error) = client.send_commands(commands) {
                    eprintln!("counter: {error}");
                    return ExitCode::FAILURE;
                }
            }
            Ok(ClientEvent::Exit) => {
                let _ = client.send_commands(app.exit());
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("counter: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}
