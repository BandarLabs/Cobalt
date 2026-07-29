//! Bluetooth control through the firmware-owned stack.
//!
//! Clara BW/Colour devices do not expose ordinary BlueZ directly. Their
//! MediaTek service owns controller bring-up and presents BlueZ-compatible
//! objects at the custom `com.kobo.mtk.bluedroid` D-Bus destination. Other
//! models may expose `org.bluez` or `bluetoothctl`, so the backend chooses
//! the strongest interface it can prove is present and never attaches HCI or
//! unloads the shared Wi-Fi/Bluetooth kernel modules itself.

use kobo_protocol::{
    BluetoothDevice, BluetoothDeviceKind, DeviceError, DeviceResult, MAX_RADIO_DEVICES,
    MAX_RADIO_NAME,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const MTK_BUS: &str = "com.kobo.mtk.bluedroid";
const BLUEZ_BUS: &str = "org.bluez";
const ADAPTER: &str = "/org/bluez/hci0";
const MANAGER: &str = "/";
const DBUS_TOOLS: [&str; 4] = [
    "/usr/bin/dbus-send",
    "/bin/dbus-send",
    "/usr/local/Kobo/dbus-send",
    "/sbin/dbus-send",
];
const BLUETOOTHCTL_TOOLS: [&str; 4] = [
    "/usr/bin/bluetoothctl",
    "/usr/local/Kobo/bluetoothctl",
    "/bin/bluetoothctl",
    "/sbin/bluetoothctl",
];
const MTK_MARKERS: [&str; 4] = [
    "/usr/share/dbus-1/system-services/com.kobo.mtk.bluedroid.service",
    "/usr/local/Kobo/mtkbtd-launcher.sh",
    "/usr/local/Kobo/launch-mtkbtd.sh",
    "/usr/local/Kobo/mtkbtd",
];
const COMMAND_TIMEOUT: Duration = Duration::from_secs(18);

/// The MTK driver is not safely re-initialised by Nickel in the same boot.
///
/// Once Cobalt changes or scans the MTK stack, panel hand-back must reboot
/// cleanly instead of starting Nickel in that already-initialised driver state.
static MTK_USED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
enum Backend {
    Dbus {
        tool: PathBuf,
        bus: &'static str,
        mtk: bool,
    },
    Bluetoothctl(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Bluetooth {
    backend: Backend,
    scanning: Arc<AtomicBool>,
}

impl Bluetooth {
    /// Opens a firmware Bluetooth control surface when one can be proven.
    #[must_use]
    pub fn open() -> Option<Self> {
        let dbus = DBUS_TOOLS
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file());
        if let Some(tool) = dbus {
            if MTK_MARKERS.iter().any(|marker| Path::new(marker).exists()) {
                return Some(Self {
                    backend: Backend::Dbus {
                        tool: tool.to_path_buf(),
                        bus: MTK_BUS,
                        mtk: true,
                    },
                    scanning: Arc::new(AtomicBool::new(false)),
                });
            }
            if Path::new("/sys/class/bluetooth").exists() {
                return Some(Self {
                    backend: Backend::Dbus {
                        tool: tool.to_path_buf(),
                        bus: BLUEZ_BUS,
                        mtk: false,
                    },
                    scanning: Arc::new(AtomicBool::new(false)),
                });
            }
        }
        BLUETOOTHCTL_TOOLS
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(|tool| Self {
                backend: Backend::Bluetoothctl(tool.to_path_buf()),
                scanning: Arc::new(AtomicBool::new(false)),
            })
    }

    /// Current controller state and remembered/discovered devices.
    #[must_use]
    pub fn state(&self) -> DeviceResult {
        match &self.backend {
            Backend::Dbus { bus, .. } if !self.bus_running(bus) => {
                bluetooth_result(false, Vec::new())
            }
            Backend::Dbus { mtk, .. } => {
                // If Nickel left the MTK stack running, merely stopping and
                // starting Nickel around this session can ask the vendor
                // driver to initialise twice. Treat observing that live stack
                // as use so hand-back takes the same safe reboot path.
                if *mtk {
                    MTK_USED.store(true, Ordering::SeqCst);
                }
                let enabled = self
                    .dbus_property(ADAPTER, "org.bluez.Adapter1", "Powered")
                    .is_ok_and(|value| value.contains("boolean true"));
                let devices = if enabled {
                    self.managed_devices().unwrap_or_default()
                } else {
                    Vec::new()
                };
                if self.scanning.swap(false, Ordering::SeqCst) {
                    let _ = self.dbus_call(ADAPTER, &["org.bluez.Adapter1.StopDiscovery"]);
                }
                bluetooth_result(enabled, devices)
            }
            Backend::Bluetoothctl(_) => {
                let show = match self.ctl(&["show"]) {
                    Ok(output) => output,
                    Err(error) => return DeviceResult::Failed(error),
                };
                let enabled = property(&show, "Powered").is_some_and(|value| value == "yes");
                let devices = self.ctl_devices();
                if self.scanning.swap(false, Ordering::SeqCst) {
                    let _ = self.ctl(&["scan", "off"]);
                }
                bluetooth_result(enabled, devices)
            }
        }
    }

    #[must_use]
    pub fn set_enabled(&self, enabled: bool) -> DeviceResult {
        match &self.backend {
            Backend::Dbus { mtk: true, .. } => {
                MTK_USED.store(true, Ordering::SeqCst);
                let result = if enabled {
                    self.dbus_call(MANAGER, &["com.kobo.bluetooth.BluedroidManager1.On"])
                        .and_then(|_| self.set_dbus_power(true))
                } else {
                    self.set_dbus_power(false).and_then(|_| {
                        self.dbus_call(MANAGER, &["com.kobo.bluetooth.BluedroidManager1.Off"])
                    })
                };
                result.map_or_else(DeviceResult::Failed, |_| self.state())
            }
            Backend::Dbus { .. } => self
                .set_dbus_power(enabled)
                .map_or_else(DeviceResult::Failed, |_| self.state()),
            Backend::Bluetoothctl(_) => self
                .ctl(&["power", if enabled { "on" } else { "off" }])
                .map_or_else(DeviceResult::Failed, |_| self.state()),
        }
    }

    /// Starts discovery and returns immediately. The next state read collects
    /// the discovered objects and stops the scan, so the panel loop never
    /// sleeps inside a device request.
    #[must_use]
    pub fn scan(&self) -> DeviceResult {
        let started = match &self.backend {
            Backend::Dbus { mtk, .. } => {
                if *mtk {
                    MTK_USED.store(true, Ordering::SeqCst);
                }
                self.dbus_call(ADAPTER, &["org.bluez.Adapter1.StartDiscovery"])
            }
            Backend::Bluetoothctl(_) => self.ctl(&["scan", "on"]),
        };
        if let Err(error) = started {
            return DeviceResult::Failed(error);
        }
        self.scanning.store(true, Ordering::SeqCst);
        bluetooth_result(true, Vec::new())
    }

    #[must_use]
    pub fn pair(&self, address: &str) -> DeviceResult {
        if !valid_address(address) {
            return DeviceResult::Failed(DeviceError::InvalidInput);
        }
        match &self.backend {
            Backend::Dbus { mtk, .. } => {
                if *mtk {
                    MTK_USED.store(true, Ordering::SeqCst);
                }
                let path = device_path(address);
                let paired = self
                    .dbus_property(&path, "org.bluez.Device1", "Paired")
                    .is_ok_and(|value| value.contains("boolean true"));
                let result = if paired {
                    Ok(String::new())
                } else {
                    self.dbus_call(&path, &["org.bluez.Device1.Pair"])
                }
                .and_then(|_| self.set_trusted(&path, true));
                result.map_or_else(DeviceResult::Failed, |_| self.state())
            }
            Backend::Bluetoothctl(_) => self
                .ctl(&["--agent", "NoInputNoOutput", "pair", address])
                .and_then(|_| self.ctl(&["trust", address]))
                .map_or_else(DeviceResult::Failed, |_| self.state()),
        }
    }

    #[must_use]
    pub fn connect(&self, address: &str) -> DeviceResult {
        self.device_action(address, "org.bluez.Device1.Connect", "connect")
    }

    #[must_use]
    pub fn disconnect(&self, address: &str) -> DeviceResult {
        self.device_action(address, "org.bluez.Device1.Disconnect", "disconnect")
    }

    #[must_use]
    pub fn forget(&self, address: &str) -> DeviceResult {
        if !valid_address(address) {
            return DeviceResult::Failed(DeviceError::InvalidInput);
        }
        match &self.backend {
            Backend::Dbus { mtk, .. } => {
                if *mtk {
                    MTK_USED.store(true, Ordering::SeqCst);
                }
                let path = device_path(address);
                let argument = format!("objpath:{path}");
                let _ = self.dbus_call(&path, &["org.bluez.Device1.Disconnect"]);
                self.dbus_call(
                    ADAPTER,
                    &["org.bluez.Adapter1.RemoveDevice", argument.as_str()],
                )
                .map_or_else(DeviceResult::Failed, |_| self.state())
            }
            Backend::Bluetoothctl(_) => self
                .ctl(&["remove", address])
                .map_or_else(DeviceResult::Failed, |_| self.state()),
        }
    }

    fn device_action(&self, address: &str, dbus_method: &str, ctl_method: &str) -> DeviceResult {
        if !valid_address(address) {
            return DeviceResult::Failed(DeviceError::InvalidInput);
        }
        match &self.backend {
            Backend::Dbus { mtk, .. } => {
                if *mtk {
                    MTK_USED.store(true, Ordering::SeqCst);
                }
                let path = device_path(address);
                let wanted_connected = ctl_method == "connect";
                let already_in_state = self
                    .dbus_property(&path, "org.bluez.Device1", "Connected")
                    .is_ok_and(|value| value.contains("boolean true") == wanted_connected);
                if already_in_state {
                    return self.state();
                }
                self.dbus_call(&path, &[dbus_method])
                    .map_or_else(DeviceResult::Failed, |_| self.state())
            }
            Backend::Bluetoothctl(_) => self
                .ctl(&[ctl_method, address])
                .map_or_else(DeviceResult::Failed, |_| self.state()),
        }
    }

    fn set_dbus_power(&self, enabled: bool) -> Result<String, DeviceError> {
        self.dbus_call(
            ADAPTER,
            &[
                "org.freedesktop.DBus.Properties.Set",
                "string:org.bluez.Adapter1",
                "string:Powered",
                if enabled {
                    "variant:boolean:true"
                } else {
                    "variant:boolean:false"
                },
            ],
        )
    }

    fn set_trusted(&self, path: &str, trusted: bool) -> Result<String, DeviceError> {
        self.dbus_call(
            path,
            &[
                "org.freedesktop.DBus.Properties.Set",
                "string:org.bluez.Device1",
                "string:Trusted",
                if trusted {
                    "variant:boolean:true"
                } else {
                    "variant:boolean:false"
                },
            ],
        )
    }

    fn dbus_property(
        &self,
        path: &str,
        interface: &str,
        name: &str,
    ) -> Result<String, DeviceError> {
        let interface = format!("string:{interface}");
        let name = format!("string:{name}");
        self.dbus_call(
            path,
            &[
                "org.freedesktop.DBus.Properties.Get",
                interface.as_str(),
                name.as_str(),
            ],
        )
    }

    fn bus_running(&self, wanted: &str) -> bool {
        let Backend::Dbus { tool, .. } = &self.backend else {
            return false;
        };
        let name = format!("string:{wanted}");
        let mut command = Command::new(tool);
        command.args([
            "--system",
            "--print-reply",
            "--type=method_call",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.NameHasOwner",
            name.as_str(),
        ]);
        run_output(command, Duration::from_secs(3))
            .and_then(output_text)
            .is_ok_and(|output| output.contains("boolean true"))
    }

    fn dbus_call(&self, path: &str, arguments: &[&str]) -> Result<String, DeviceError> {
        let Backend::Dbus { tool, bus, .. } = &self.backend else {
            return Err(DeviceError::Backend);
        };
        let destination = format!("--dest={bus}");
        let mut command = Command::new(tool);
        command
            .args(["--system", "--print-reply", "--type=method_call"])
            .arg(destination)
            .arg(path)
            .args(arguments);
        run_output(command, COMMAND_TIMEOUT).and_then(output_text)
    }

    fn managed_devices(&self) -> Result<Vec<BluetoothDevice>, DeviceError> {
        self.dbus_call(
            MANAGER,
            &["org.freedesktop.DBus.ObjectManager.GetManagedObjects"],
        )
        .map(|output| parse_managed_devices(&output))
    }

    fn ctl(&self, arguments: &[&str]) -> Result<String, DeviceError> {
        let Backend::Bluetoothctl(tool) = &self.backend else {
            return Err(DeviceError::Backend);
        };
        let mut command = Command::new(tool);
        command.args(arguments);
        run_output(command, COMMAND_TIMEOUT).and_then(output_text)
    }

    fn ctl_devices(&self) -> Vec<BluetoothDevice> {
        let Ok(list) = self.ctl(&["devices"]) else {
            return Vec::new();
        };
        parse_ctl_devices(&list)
            .into_iter()
            .take(MAX_RADIO_DEVICES)
            .map(|(address, listed_name)| {
                let info = self.ctl(&["info", &address]).unwrap_or_default();
                let name = property(&info, "Alias")
                    .or_else(|| property(&info, "Name"))
                    .unwrap_or(listed_name.as_str());
                let icon = property(&info, "Icon").unwrap_or_default();
                BluetoothDevice {
                    address,
                    name: clip(name, MAX_RADIO_NAME),
                    kind: classify(icon, name, None),
                    paired: property(&info, "Paired").is_some_and(|value| value == "yes"),
                    connected: property(&info, "Connected").is_some_and(|value| value == "yes"),
                }
            })
            .collect()
    }
}

/// True when safe hand-back to Nickel requires a clean reboot.
#[must_use]
pub fn requires_reboot_after_use() -> bool {
    MTK_USED.load(Ordering::SeqCst)
}

fn bluetooth_result(enabled: bool, devices: Vec<BluetoothDevice>) -> DeviceResult {
    DeviceResult::Bluetooth {
        available: true,
        enabled,
        devices,
        // Read here rather than remembered by the caller, because every path
        // that reaches this funnel has already had its chance to set the flag.
        restart_on_exit: requires_reboot_after_use(),
    }
}

fn run_output(mut command: Command, timeout: Duration) -> Result<Output, DeviceError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| DeviceError::Backend)?;
    let mut stdout = child.stdout.take().ok_or(DeviceError::Backend)?;
    let mut stderr = child.stderr.take().ok_or(DeviceError::Backend)?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_reader
                    .join()
                    .map_err(|_| DeviceError::Backend)?
                    .map_err(|_| DeviceError::Backend)?;
                let stderr = stderr_reader
                    .join()
                    .map_err(|_| DeviceError::Backend)?
                    .map_err(|_| DeviceError::Backend)?;
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(DeviceError::TimedOut);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(DeviceError::Backend);
            }
        }
    }
}

fn output_text(output: Output) -> Result<String, DeviceError> {
    let Output {
        status,
        stdout,
        stderr,
    } = output;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr);
    let combined = format!("{stdout}\n{stderr}");
    if status.success() && !failure_text(&combined) {
        Ok(stdout)
    } else {
        Err(classify_error(&combined))
    }
}

fn failure_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("error org.")
        || text.contains("failed")
        || text.contains("not available")
        || text.contains("no default controller")
}

fn classify_error(text: &str) -> DeviceError {
    let text = text.to_ascii_lowercase();
    if text.contains("authentication") || text.contains("rejected") {
        DeviceError::Authentication
    } else if text.contains("not available")
        || text.contains("not found")
        || text.contains("unknown object")
    {
        DeviceError::NotFound
    } else if text.contains("timeout") {
        DeviceError::TimedOut
    } else if text.contains("not ready") || text.contains("unreachable") {
        DeviceError::Unreachable
    } else {
        DeviceError::Backend
    }
}

#[derive(Default)]
struct ManagedDevice {
    path: String,
    address: String,
    name: String,
    alias: String,
    icon: String,
    class: Option<u32>,
    paired: bool,
    connected: bool,
    rssi: i16,
}

fn parse_managed_devices(output: &str) -> Vec<BluetoothDevice> {
    let mut parsed = Vec::new();
    let mut current: Option<ManagedDevice> = None;
    let mut wanted = "";
    for line in output.lines() {
        let line = line.trim();
        if let Some(path) = line
            .strip_prefix("object path \"")
            .and_then(|value| value.strip_suffix('"'))
        {
            if let Some(device) = current.take() {
                push_managed(&mut parsed, device);
            }
            current = path.contains("/dev_").then(|| ManagedDevice {
                path: path.to_owned(),
                rssi: i16::MIN,
                ..ManagedDevice::default()
            });
            wanted = "";
            continue;
        }
        let Some(device) = current.as_mut() else {
            continue;
        };
        for property_name in [
            "Address",
            "Name",
            "Alias",
            "Icon",
            "Class",
            "Paired",
            "Connected",
            "RSSI",
        ] {
            if line == format!("string \"{property_name}\"") {
                wanted = property_name;
                break;
            }
        }
        if let Some(value) = dbus_string(line) {
            match wanted {
                "Address" => value.clone_into(&mut device.address),
                "Name" => value.clone_into(&mut device.name),
                "Alias" => value.clone_into(&mut device.alias),
                "Icon" => value.clone_into(&mut device.icon),
                _ => {}
            }
            wanted = "";
        } else if let Some(value) = boolean_variant(line) {
            match wanted {
                "Paired" => device.paired = value,
                "Connected" => device.connected = value,
                _ => {}
            }
            wanted = "";
        } else if wanted == "Class" {
            if let Some(value) = numeric_variant(line) {
                device.class = u32::try_from(value).ok();
                wanted = "";
            }
        } else if wanted == "RSSI" {
            if let Some(value) = numeric_variant(line) {
                device.rssi = i16::try_from(value).unwrap_or(i16::MIN);
                wanted = "";
            }
        }
    }
    if let Some(device) = current {
        push_managed(&mut parsed, device);
    }
    parsed.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.name.cmp(&right.0.name))
    });
    parsed.truncate(MAX_RADIO_DEVICES);
    parsed.into_iter().map(|(device, _)| device).collect()
}

fn push_managed(parsed: &mut Vec<(BluetoothDevice, i16)>, mut device: ManagedDevice) {
    if device.address.is_empty() {
        device.address = address_from_path(&device.path).unwrap_or_default();
    }
    if !valid_address(&device.address) {
        return;
    }
    let name = if device.alias.is_empty() {
        if device.name.is_empty() {
            device.address.clone()
        } else {
            device.name
        }
    } else {
        device.alias
    };
    parsed.push((
        BluetoothDevice {
            address: device.address,
            name: clip(&name, MAX_RADIO_NAME),
            kind: classify(&device.icon, &name, device.class),
            paired: device.paired,
            connected: device.connected,
        },
        device.rssi,
    ));
}

/// The string inside a `variant`, however deeply `dbus-send` indented it.
///
/// The gap between `variant` and the value it contains is not fixed.
/// `dbus-send` prints a variant by printing the word and then recursing one
/// indent level deeper, so the spacing grows with nesting. This matched a
/// literal ten spaces, which is the depth a device property sits at in a
/// single-property reply and *not* the depth it sits at inside
/// `GetManagedObjects`.
///
/// The symptom on hardware was that every string property silently failed to
/// parse. `Address` fell back to the one recoverable from the object path, so
/// the list still had the right devices, and `Name` and `Alias` had no
/// fallback at all, so every pair of headphones was listed by its MAC address.
/// It looked like the reader could not resolve names. It had never read them.
///
/// `boolean_variant` splits on whitespace and was never affected, which is why
/// paired and connected were right while the names beside them were not.
fn dbus_string(line: &str) -> Option<&str> {
    let value = line
        .strip_prefix("variant")?
        .trim_start()
        .strip_prefix("string \"")?;
    value.strip_suffix('"')
}

fn numeric_variant(line: &str) -> Option<i64> {
    line.split_ascii_whitespace().last()?.parse().ok()
}

fn boolean_variant(line: &str) -> Option<bool> {
    let mut words = line.split_ascii_whitespace();
    (words.next()? == "variant" && words.next()? == "boolean")
        .then(|| words.next()?.parse().ok())
        .flatten()
}

fn parse_ctl_devices(output: &str) -> Vec<(String, String)> {
    let mut devices = Vec::new();
    for line in output.lines() {
        let line = line.trim_start_matches(|character: char| character != 'D');
        let Some(rest) = line.strip_prefix("Device ") else {
            continue;
        };
        let mut fields = rest.splitn(2, ' ');
        let Some(address) = fields.next().filter(|address| valid_address(address)) else {
            continue;
        };
        let name = fields.next().unwrap_or(address).trim();
        if !devices.iter().any(|(known, _)| known == address) {
            devices.push((address.to_owned(), clip(name, MAX_RADIO_NAME)));
        }
    }
    devices
}

fn property<'a>(output: &'a str, wanted: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let line = line.trim();
        let (name, value) = line.split_once(':')?;
        (name == wanted).then_some(value.trim())
    })
}

fn classify(icon: &str, name: &str, class: Option<u32>) -> BluetoothDeviceKind {
    let words = format!("{icon} {name}").to_ascii_lowercase();
    if words.contains("audio")
        || words.contains("headset")
        || words.contains("headphone")
        || words.contains("speaker")
        || class.is_some_and(|value| (value >> 8) & 0x1f == 4)
    {
        BluetoothDeviceKind::Audio
    } else if words.contains("keyboard")
        || class.is_some_and(|value| (value >> 8) & 0x1f == 5 && value & 0x40 != 0)
    {
        BluetoothDeviceKind::Keyboard
    } else if words.contains("input")
        || words.contains("mouse")
        || words.contains("remote")
        || words.contains("game")
        || class.is_some_and(|value| (value >> 8) & 0x1f == 5)
    {
        BluetoothDeviceKind::Input
    } else {
        BluetoothDeviceKind::Other
    }
}

fn device_path(address: &str) -> String {
    format!("{ADAPTER}/dev_{}", address.replace(':', "_"))
}

fn address_from_path(path: &str) -> Option<String> {
    let address = path.rsplit("/dev_").next()?.replace('_', ":");
    valid_address(&address).then_some(address)
}

fn valid_address(address: &str) -> bool {
    let fields = address.split(':').collect::<Vec<_>>();
    fields.len() == 6
        && fields
            .iter()
            .all(|field| field.len() == 2 && field.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn clip(value: &str, bytes: usize) -> String {
    if value.len() <= bytes {
        return value.to_owned();
    }
    let mut end = bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::{classify, device_path, parse_ctl_devices, parse_managed_devices, property};
    use kobo_protocol::BluetoothDeviceKind;

    #[test]
    fn bluez_device_lines_are_deduplicated() {
        let output = "[NEW] Device AA:BB:CC:DD:EE:FF QuietComfort\n\
                      Device AA:BB:CC:DD:EE:FF QuietComfort\n\
                      Device 01:23:45:67:89:AB Keyboard";
        assert_eq!(parse_ctl_devices(output).len(), 2);
    }

    #[test]
    fn bluez_properties_ignore_indentation() {
        let output = "Controller 00:11\n\tPowered: yes\n\tDiscovering: no\n";
        assert_eq!(property(output, "Powered"), Some("yes"));
    }

    #[test]
    fn useful_device_classes_survive_the_backend() {
        assert_eq!(
            classify("audio-headphones", "", None),
            BluetoothDeviceKind::Audio
        );
        assert_eq!(
            classify("input-keyboard", "", None),
            BluetoothDeviceKind::Keyboard
        );
        assert_eq!(
            classify("input-mouse", "", None),
            BluetoothDeviceKind::Input
        );
    }

    #[test]
    fn mtk_managed_objects_include_pairing_connection_and_class() {
        let output = r#"object path "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
   string "org.bluez.Device1"
      string "Address"
      variant          string "AA:BB:CC:DD:EE:FF"
      string "Alias"
      variant          string "Writing Keyboard"
      string "Class"
      variant          uint32 1344
      string "Paired"
      variant          boolean true
      string "Connected"
      variant          boolean false
      string "RSSI"
      variant          int16 -41
"#;
        let devices = parse_managed_devices(output);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Writing Keyboard");
        assert_eq!(devices[0].kind, BluetoothDeviceKind::Keyboard);
        assert!(devices[0].paired);
        assert!(!devices[0].connected);
    }

    /// The indentation is the bug, so the fixture has to carry the real one.
    ///
    /// This is the shape `dbus-send` actually prints for
    /// `GetManagedObjects`, where a device property sits several levels deeper
    /// than in a single-property reply. Parsed with a fixed ten-space gap it
    /// yielded no name at all, and the reader listed a pair of headphones as
    /// `AA:BB:CC:DD:EE:FF`.
    #[test]
    fn a_name_survives_however_deeply_dbus_send_indented_it() {
        let output = "object path \"/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF\"\n\
            array [\n\
            dict entry(\n\
            string \"org.bluez.Device1\"\n\
            array [\n\
            dict entry(\n\
            string \"Address\"\n\
            variant                            string \"AA:BB:CC:DD:EE:FF\"\n\
            )\n\
            dict entry(\n\
            string \"Name\"\n\
            variant                            string \"AirPods Pro\"\n\
            )\n\
            dict entry(\n\
            string \"Icon\"\n\
            variant                            string \"audio-headphones\"\n\
            )\n\
            dict entry(\n\
            string \"Connected\"\n\
            variant                            boolean true\n\
            )\n";
        let devices = parse_managed_devices(output);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "AirPods Pro", "not the MAC address");
        assert_eq!(devices[0].address, "AA:BB:CC:DD:EE:FF");
        assert_eq!(devices[0].kind, BluetoothDeviceKind::Audio);
        assert!(devices[0].connected);
    }

    #[test]
    fn addresses_become_dbus_object_paths_without_a_shell() {
        assert_eq!(
            device_path("AA:BB:CC:DD:EE:FF"),
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
        );
    }
}
