//! Developer-session control for a connected device.
//!
//! Two independent mechanisms keep a device reachable while you work:
//!
//! * A kernel wake lock stops the device suspending. It lives in RAM only,
//!   clears on reboot, and is released by name.
//! * The stock reader's `ForceWifiOn` developer setting stops the reader
//!   powering Wi-Fi down after its inactivity timer. This is a single line in
//!   the reader's own settings file, is backed up before the first change, and
//!   is fully reversible.
//!
//! Neither mechanism touches a partition, the bootloader, the kernel, firmware,
//! or any book, and neither is required for an application to run.

/// Absolute path of the stock reader settings file.
pub const READER_CONFIG: &str = "/mnt/onboard/.kobo/Kobo/Kobo eReader.conf";

/// Path of the pristine backup taken before the first change.
pub const READER_CONFIG_BACKUP: &str = "/mnt/onboard/.kobo/Kobo/Kobo eReader.conf.kobo-sdk-backup";

/// Name of the wake lock this tool holds.
pub const WAKE_LOCK_NAME: &str = "kobo-sdk-dev";

/// Reader binaries searched for evidence that a setting exists in this firmware.
///
/// Most reader logic lives in the shared library rather than the small
/// executable, so both are searched before a setting is declared unsupported.
pub const READER_BINARIES: [&str; 2] = [
    "/usr/local/Kobo/libnickel.so.1.0.0",
    "/usr/local/Kobo/nickel",
];

/// Returns a shell fragment that sets `supported` to 1 when the running
/// firmware contains `setting`, and to 0 when it does not.
///
/// A settings file is only advice: the reader ignores keys it does not
/// implement. Writing one would look successful and change nothing, so every
/// settings change is preceded by this check and refuses when it fails.
fn setting_support_probe(setting: &str) -> String {
    let candidates = READER_BINARIES
        .iter()
        .map(|path| format!("'{path}'"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "supported=0\n\
         for reader in {candidates}; do\n\
         \x20 [ -f \"$reader\" ] || continue\n\
         \x20 if grep -qa '{setting}' \"$reader\"; then supported=1; break; fi\n\
         done\n"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Switch {
    On,
    Off,
}

impl Switch {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Returns the shell fragment reporting one setting's support, current value,
/// and whether the running reader has read the last change this tool made.
fn setting_report_fragment(setting: &Setting) -> String {
    let key = setting.key;
    let marker = setting.marker();
    let probe = setting_support_probe(key);
    format!(
        "{probe}\
         if [ \"$supported\" -eq 1 ]; then\n\
         \x20 echo '{key}_supported: yes'\n\
         else\n\
         \x20 echo '{key}_supported: no'\n\
         fi\n\
         printf '{key}: '\n\
         sed -n 's/^{key}[ \\t]*=[ \\t]*//p' '{READER_CONFIG}' 2>/dev/null | head -1 \
           | grep . || echo unset\n\
         if [ ! -f '{marker}' ] || [ -z \"$reader_pid\" ]; then\n\
         \x20 echo '{key}_pending_restart: unknown'\n\
         elif [ '{marker}' -nt \"/proc/$reader_pid\" ]; then\n\
         \x20 echo '{key}_pending_restart: yes'\n\
         else\n\
         \x20 echo '{key}_pending_restart: no'\n\
         fi\n"
    )
}

/// Returns a script that reports the current developer-session state.
#[must_use]
pub fn status_script() -> String {
    let settings_report = [Setting::force_wifi_on(), Setting::auto_sleep_minutes(0)]
        .iter()
        .map(setting_report_fragment)
        .collect::<String>();
    format!(
        "set -u\n\
         printf 'wake_lock: '\n\
         cat /sys/power/wake_lock 2>/dev/null || printf '<unreadable>'\n\
         printf '\\n'\n\
         printf 'wifi_operstate: '\n\
         cat /sys/class/net/wlan0/operstate 2>/dev/null || printf '<absent>'\n\
         printf '\\n'\n\
         echo \"suspend_events: $(dmesg | grep -c 'PM: suspend entry' || true)\"\n\
         uptime_seconds=$(cut -d' ' -f1 /proc/uptime | cut -d. -f1)\n\
         awake_seconds=$(dmesg | sed -n 's/^\\[ *\\([0-9][0-9]*\\)\\..*/\\1/p' | tail -1)\n\
         echo \"uptime_seconds: ${{uptime_seconds:-unknown}}\"\n\
         echo \"kernel_awake_seconds: ${{awake_seconds:-unknown}}\"\n\
         reader_pid=$(pidof nickel 2>/dev/null | awk '{{ print $1 }}')\n\
         {settings_report}\
         if [ -f '{READER_CONFIG_BACKUP}' ]; then\n\
           echo 'config_backup: present'\n\
         else\n\
           echo 'config_backup: absent'\n\
         fi\n\
         exit\n"
    )
}

/// Returns a script that holds or releases the developer wake lock.
///
/// The wake lock is RAM-only state in the running kernel. Rebooting always
/// clears it, so this can never leave a device permanently unable to sleep.
#[must_use]
pub fn wake_lock_script(switch: Switch) -> String {
    let action = match switch {
        Switch::On => format!("echo {WAKE_LOCK_NAME} > /sys/power/wake_lock"),
        Switch::Off => format!(
            "if grep -qw {WAKE_LOCK_NAME} /sys/power/wake_lock 2>/dev/null; then\n\
             \x20 echo {WAKE_LOCK_NAME} > /sys/power/wake_unlock\n\
             fi"
        ),
    };
    format!(
        "set -eu\n\
         if [ ! -w /sys/power/wake_lock ]; then\n\
         \x20 echo 'this kernel has no writable /sys/power/wake_lock' >&2\n\
         \x20 exit 1\n\
         fi\n\
         {action}\n\
         printf 'wake_lock: '\n\
         cat /sys/power/wake_lock\n\
         printf '\\n'\n\
         exit\n"
    )
}

/// Returns a script that re-applies the wake lock and reports whether it had
/// been lost since the last renewal.
///
/// Writing a name that is already held is a no-op in the kernel, so renewing is
/// safe to repeat. Something on this firmware clears the lock after a couple of
/// minutes, so a session that must stay reachable has to renew it.
#[must_use]
pub fn wake_lock_renew_script() -> String {
    format!(
        "set -eu\n\
         if grep -qw {WAKE_LOCK_NAME} /sys/power/wake_lock 2>/dev/null; then\n\
         \x20 echo 'renew: held'\n\
         else\n\
         \x20 echo {WAKE_LOCK_NAME} > /sys/power/wake_lock\n\
         \x20 echo 'renew: reacquired'\n\
         fi\n\
         exit\n"
    )
}

/// One reader setting this tool knows how to change.
///
/// Every setting is expressed this way so there is a single audited rewrite
/// path rather than one per setting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Setting {
    /// Settings file section, without brackets.
    pub section: &'static str,
    /// Key name inside that section.
    pub key: &'static str,
    /// Value to write.
    pub value: String,
}

impl Setting {
    /// Stops the reader powering Wi-Fi down on its own inactivity timer.
    #[must_use]
    pub fn force_wifi_on() -> Self {
        Self {
            section: "DeveloperSettings",
            key: "ForceWifiOn",
            value: "true".to_owned(),
        }
    }

    /// How long the reader waits before suspending the whole device.
    ///
    /// This is the setting that actually governs reachability: the suspend is
    /// requested by the reader process itself, so no kernel wake lock can
    /// prevent it.
    #[must_use]
    pub fn auto_sleep_minutes(minutes: u32) -> Self {
        Self {
            section: "PowerOptions",
            key: "AutoSleepMinutes",
            value: minutes.to_string(),
        }
    }

    /// Path of the marker recording that this tool changed the setting.
    #[must_use]
    pub fn marker(&self) -> String {
        format!("{APPLIED_MARKER_PREFIX}{}", self.key)
    }
}

/// Prefix of the per-setting markers this tool writes.
///
/// The reader rewrites its settings file during normal startup and preserves
/// keys it did not write, so the file's modification time cannot say whether a
/// change has been read yet. A marker written only when this tool actually
/// changes something can.
pub const APPLIED_MARKER_PREFIX: &str = "/mnt/onboard/.kobo/kobo-sdk-applied-";

/// Returns the awk program that rewrites just the target section.
///
/// Every other byte of the file is copied through unchanged. Blank lines inside
/// the section are held back so the key lands directly under the section's last
/// real line rather than after a gap.
fn section_transform(setting: &Setting, switch: Switch) -> String {
    let Setting {
        section,
        key,
        value,
    } = setting;
    match switch {
        Switch::On => format!(
            "awk '\n\
             \x20 BEGIN {{ inserted = 0; in_section = 0; held = 0 }}\n\
             \x20 /^\\[{section}\\][ \\t]*$/ {{\n\
             \x20   while (held > 0) {{ print \"\"; held-- }}\n\
             \x20   in_section = 1; print; next\n\
             \x20 }}\n\
             \x20 /^\\[/ {{\n\
             \x20   if (in_section && !inserted) {{ print \"{key}={value}\"; inserted = 1 }}\n\
             \x20   while (held > 0) {{ print \"\"; held-- }}\n\
             \x20   in_section = 0; print; next\n\
             \x20 }}\n\
             \x20 /^[ \\t]*$/ {{ if (in_section) {{ held++; next }} print; next }}\n\
             \x20 /^{key}[ \\t]*=/ {{\n\
             \x20   if (in_section) {{ print \"{key}={value}\"; inserted = 1; next }}\n\
             \x20 }}\n\
             \x20 {{ while (held > 0) {{ print \"\"; held-- }} print }}\n\
             \x20 END {{\n\
             \x20   if (!inserted) {{\n\
             \x20     if (!in_section) {{ print \"\"; print \"[{section}]\" }}\n\
             \x20     print \"{key}={value}\"\n\
             \x20   }}\n\
             \x20   while (held > 0) {{ print \"\"; held-- }}\n\
             \x20 }}\n\
             ' \"$conf\" > \"$tmp\"\n"
        ),
        Switch::Off => format!("awk '!/^{key}[ \\t]*=/ {{ print }}' \"$conf\" > \"$tmp\"\n"),
    }
}

/// Returns a script that writes or removes one reader setting.
///
/// The script refuses to run unless the settings file is a regular file, takes
/// a pristine backup before the first change, writes through a temporary file
/// in the same directory, and verifies that exactly the intended line changed
/// before replacing the original.
#[must_use]
pub fn setting_script(setting: &Setting, switch: Switch) -> String {
    let Setting { key, value, .. } = setting;
    let marker_path = setting.marker();
    let transform = section_transform(setting, switch);
    let (expectation, max_changed) = match switch {
        // Writing adds one line, replaces one line, or, on a device without the
        // section at all, adds a blank line, the section, and the key.
        Switch::On => (format!("! grep -q '^{key}={value}$' \"$tmp\""), 3),
        Switch::Off => (format!("grep -q '^{key}' \"$tmp\""), 1),
    };
    // A rewrite that changed nothing means the reader already holds the
    // intended value, so there is nothing for a restart to pick up.
    let marker = match switch {
        Switch::On => format!(
            "if [ \"$changed\" -gt 0 ]; then\n\
             \x20 touch '{marker_path}'\n\
             fi\n"
        ),
        Switch::Off => format!("rm -f '{marker_path}'\n"),
    };
    // Removing a key is always allowed so recovery never depends on a probe.
    let support_gate = match switch {
        Switch::On => format!(
            "{probe}\
             if [ \"$supported\" -ne 1 ]; then\n\
             \x20 echo 'this firmware has no {key} setting; refusing to write one that would do nothing' >&2\n\
             \x20 exit 1\n\
             fi\n",
            probe = setting_support_probe(key)
        ),
        Switch::Off => String::new(),
    };
    format!(
        "set -eu\n\
         conf='{READER_CONFIG}'\n\
         backup='{READER_CONFIG_BACKUP}'\n\
         tmp=\"$conf.kobo-sdk-new\"\n\
         {support_gate}\
         if [ ! -s \"$conf\" ]; then\n\
         \x20 echo 'reader settings file is missing or empty; refusing to create one' >&2\n\
         \x20 exit 1\n\
         fi\n\
         if [ ! -f \"$backup\" ]; then\n\
         \x20 cp \"$conf\" \"$backup\"\n\
         fi\n\
         if ! command -v diff >/dev/null 2>&1 || ! command -v cmp >/dev/null 2>&1; then\n\
         \x20 echo 'no diff or cmp command to bound the change; refusing' >&2\n\
         \x20 exit 1\n\
         fi\n\
         trap 'rm -f \"$tmp\"' EXIT HUP INT TERM\n\
         {transform}\
         if [ ! -s \"$tmp\" ]; then\n\
         \x20 echo 'rewritten settings file is empty; keeping the original' >&2\n\
         \x20 exit 1\n\
         fi\n\
         if {expectation}; then\n\
         \x20 echo 'rewritten settings file does not have the intended value; keeping the original' >&2\n\
         \x20 exit 1\n\
         fi\n\
         if cmp -s \"$conf\" \"$tmp\"; then\n\
         \x20 changed=0\n\
         else\n\
         \x20 changed=$(diff -U 0 \"$conf\" \"$tmp\" 2>/dev/null \\\n\
         \x20   | grep -cE '^[-+]($|[^-+])' || true)\n\
         \x20 if [ \"$changed\" -eq 0 ]; then\n\
         \x20   echo 'the files differ but no change could be counted; refusing' >&2\n\
         \x20   exit 1\n\
         \x20 fi\n\
         fi\n\
         if [ \"$changed\" -gt {max_changed} ]; then\n\
         \x20 echo \"refusing to apply $changed changed lines; expected at most {max_changed}\" >&2\n\
         \x20 exit 1\n\
         fi\n\
         cat \"$tmp\" > \"$conf\"\n\
         {marker}\
         sync\n\
         echo \"applied; changed_lines=$changed\"\n\
         printf '{key}: '\n\
         if grep -q '^{key}={value}$' \"$conf\"; then echo '{value}'; else echo absent; fi\n\
         exit\n"
    )
}

/// Returns a script that restores the pristine settings backup.
#[must_use]
pub fn restore_config_script() -> String {
    format!(
        "set -eu\n\
         conf='{READER_CONFIG}'\n\
         backup='{READER_CONFIG_BACKUP}'\n\
         if [ ! -f \"$backup\" ]; then\n\
         \x20 echo 'no backup to restore' >&2\n\
         \x20 exit 1\n\
         fi\n\
         cat \"$backup\" > \"$conf\"\n\
         rm -f '{APPLIED_MARKER_PREFIX}'*\n\
         sync\n\
         echo 'reader settings restored from backup'\n\
         exit\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        restore_config_script, setting_script, status_script, wake_lock_renew_script,
        wake_lock_script, Setting, Switch, APPLIED_MARKER_PREFIX, READER_BINARIES, READER_CONFIG,
        READER_CONFIG_BACKUP, WAKE_LOCK_NAME,
    };
    use std::env;
    use std::fs;
    use std::io::Write;
    use std::process::{self, Command, Stdio};
    use std::thread;

    #[test]
    fn switch_parsing_is_exact() {
        assert_eq!(Switch::parse("on"), Some(Switch::On));
        assert_eq!(Switch::parse("off"), Some(Switch::Off));
        for wrong in ["ON", "true", "1", "", "enable"] {
            assert_eq!(Switch::parse(wrong), None);
        }
    }

    #[test]
    fn wake_lock_scripts_are_ram_only_and_named() {
        let on = wake_lock_script(Switch::On);
        assert!(on.contains(&format!("echo {WAKE_LOCK_NAME} > /sys/power/wake_lock")));
        let off = wake_lock_script(Switch::Off);
        assert!(off.contains(&format!("echo {WAKE_LOCK_NAME} > /sys/power/wake_unlock")));
        for script in [&on, &off] {
            assert!(!script.contains("/mnt/onboard"));
            assert!(!script.contains("rm "));
            assert!(!script.contains("reboot"));
            assert!(script.ends_with("exit\n"));
        }
    }

    #[test]
    fn wifi_scripts_back_up_before_changing_and_bound_the_change() {
        for switch in [Switch::On, Switch::Off] {
            let script = setting_script(&Setting::force_wifi_on(), switch);
            assert!(script.contains(READER_CONFIG));
            assert!(script.contains(READER_CONFIG_BACKUP));
            // The backup must be taken before the temporary file is written.
            let backup_at = script.find("cp \"$conf\" \"$backup\"").expect("backup");
            let write_at = script.find("$tmp\"\n").expect("temporary write");
            assert!(backup_at < write_at);
            assert!(script.contains("if [ ! -s \"$tmp\" ]"));
            assert!(script.contains("expected at most"));
            assert!(script.contains("trap 'rm -f \"$tmp\"'"));
            // Nothing outside the reader settings file may be touched.
            assert!(!script.contains("mmcblk"));
            assert!(!script.contains("/dev/mmc"));
            assert!(!script.contains("/dev/fb"));
            assert!(!script.contains("mkfs"));
            assert!(!script.contains("dd "));
            assert!(!script.contains("reboot"));
            assert!(!script.contains("rm -rf"));
            assert!(script.ends_with("exit\n"));
        }
    }

    #[test]
    fn enabling_targets_the_developer_section_and_disabling_removes_the_key() {
        assert!(
            setting_script(&Setting::force_wifi_on(), Switch::On).contains("[DeveloperSettings]")
        );
        assert!(setting_script(&Setting::force_wifi_on(), Switch::On).contains("ForceWifiOn=true"));
        assert!(setting_script(&Setting::force_wifi_on(), Switch::Off).contains("!/^ForceWifiOn"));
        assert!(
            !setting_script(&Setting::force_wifi_on(), Switch::Off).contains("ForceWifiOn=true\"")
        );
    }

    /// Runs the real generated script against a throwaway copy of a settings
    /// file so the rewrite logic itself is proven, not just its text.
    fn apply(switch: Switch, original: &str) -> Result<String, String> {
        apply_with_firmware(switch, original, true)
    }

    fn apply_with_firmware(
        switch: Switch,
        original: &str,
        firmware_supports: bool,
    ) -> Result<String, String> {
        apply_setting(
            &Setting::force_wifi_on(),
            switch,
            original,
            firmware_supports,
        )
    }

    fn apply_setting(
        setting: &Setting,
        switch: Switch,
        original: &str,
        firmware_supports: bool,
    ) -> Result<String, String> {
        let directory = env::temp_dir().join(format!(
            "kobo-devsession-test-{}-{:?}",
            process::id(),
            thread::current().id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("test directory");
        let config = directory.join("Kobo eReader.conf");
        let backup = directory.join("Kobo eReader.conf.backup");
        fs::write(&config, original).expect("write settings");

        // Stand in for the reader library the support probe searches.
        let reader = directory.join("libnickel");
        let contents = if firmware_supports {
            format!("some symbols {} more symbols", setting.key)
        } else {
            "some symbols without the setting".to_owned()
        };
        fs::write(&reader, &contents).expect("write fake reader");

        // The backup constant starts with the config constant, so it has to be
        // substituted first or it would be rewritten twice.
        let marker = directory.join("wifi-applied");
        let script = setting_script(setting, switch)
            .replace(&setting.marker(), marker.to_str().expect("utf-8 path"))
            .replace(READER_BINARIES[0], reader.to_str().expect("utf-8 path"))
            .replace(
                READER_BINARIES[1],
                directory.join("absent").to_str().expect("utf-8 path"),
            )
            .replace(READER_CONFIG_BACKUP, backup.to_str().expect("utf-8 path"))
            .replace(READER_CONFIG, config.to_str().expect("utf-8 path"));
        let mut shell = Command::new("sh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn shell");
        shell
            .stdin
            .as_mut()
            .expect("shell stdin")
            .write_all(script.as_bytes())
            .expect("write script");
        let finished = shell.wait_with_output().expect("shell exit");
        let status = finished.status;
        let reported = String::from_utf8_lossy(&finished.stdout)
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("applied; changed_lines=")?
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(usize::MAX);

        let marker_present = marker.exists();
        let updated = fs::read_to_string(&config).expect("read settings");
        let changed_something = updated != original;
        if status.success() {
            // The count is what bounds the blast radius of every settings
            // write, so it has to be right, not merely present. An earlier
            // version counted zero on every device whose diff writes unified
            // output, which silently disabled the bound.
            let truly_changed = line_difference(original, &updated);
            assert_eq!(
                reported, truly_changed,
                "reported changed lines must match the real difference"
            );
        }
        let result = if status.success() {
            assert_eq!(
                marker_present,
                switch == Switch::On && changed_something,
                "the marker must record only a change the reader has not read yet"
            );
            assert_eq!(
                fs::read_to_string(&backup).expect("read backup"),
                original,
                "the backup must be a pristine copy of the original"
            );
            Ok(fs::read_to_string(&config).expect("read settings"))
        } else {
            let unchanged = fs::read_to_string(&config).expect("read settings");
            assert_eq!(unchanged, original, "a rejected change must not be applied");
            Err(format!("script exited with {status}"))
        };
        let _ = fs::remove_dir_all(&directory);
        result
    }

    /// Counts lines that differ between two files, independent of any diff
    /// program's output format.
    fn line_difference(before: &str, after: &str) -> usize {
        let mut removed: Vec<&str> = before.lines().collect();
        let mut added = Vec::new();
        for line in after.lines() {
            if let Some(at) = removed.iter().position(|candidate| *candidate == line) {
                removed.remove(at);
            } else {
                added.push(line);
            }
        }
        removed.len() + added.len()
    }

    #[test]
    fn the_change_count_is_reported_correctly() {
        // A plain insertion is exactly one changed line.
        let original = "[PowerOptions]\nFrontLightLevel=7\n";
        apply_setting(
            &Setting::auto_sleep_minutes(120),
            Switch::On,
            original,
            true,
        )
        .expect("apply");
        // Replacing an existing value is one removal plus one addition.
        let existing = "[PowerOptions]\nAutoSleepMinutes=5\nFrontLightLevel=7\n";
        apply_setting(
            &Setting::auto_sleep_minutes(120),
            Switch::On,
            existing,
            true,
        )
        .expect("apply");
        // Removing is one changed line.
        apply_setting(&Setting::auto_sleep_minutes(0), Switch::Off, existing, true).expect("apply");
    }

    #[test]
    fn the_sleep_delay_is_written_into_the_existing_power_section() {
        // The real device already has this section with an unrelated key in it.
        let original = "[General]\nx=1\n\n[PowerOptions]\nAutoColorEnabled=false\nFrontLightLevel=7\n\n[Reading]\ny=2\n";
        let setting = Setting::auto_sleep_minutes(120);
        let result = apply_setting(&setting, Switch::On, original, true).expect("apply");
        assert!(result.contains("AutoSleepMinutes=120"));
        // The unrelated keys and every other section must survive untouched.
        assert!(result.contains("FrontLightLevel=7"));
        assert!(result.contains("AutoColorEnabled=false"));
        assert!(result.contains("[General]"));
        assert!(result.contains("[Reading]"));
        // It must land inside PowerOptions, not in a later section.
        let section = result.split("[PowerOptions]").nth(1).expect("section");
        let body = section.split("[Reading]").next().expect("body");
        assert!(body.contains("AutoSleepMinutes=120"));
    }

    #[test]
    fn the_sleep_delay_can_be_put_back_to_the_reader_default() {
        // Removing the key is how the reader returns to its own default.
        let original = "[PowerOptions]\nAutoSleepMinutes=120\nFrontLightLevel=7\n";
        let setting = Setting::auto_sleep_minutes(0);
        let result = apply_setting(&setting, Switch::Off, original, true).expect("apply");
        assert!(!result.contains("AutoSleepMinutes"));
        assert!(result.contains("FrontLightLevel=7"));
    }

    #[test]
    fn each_setting_records_its_own_marker() {
        let wifi = Setting::force_wifi_on().marker();
        let sleep = Setting::auto_sleep_minutes(30).marker();
        assert_ne!(wifi, sleep);
        assert!(wifi.starts_with(APPLIED_MARKER_PREFIX));
        assert!(sleep.starts_with(APPLIED_MARKER_PREFIX));
        // The value must not affect which marker is used.
        assert_eq!(sleep, Setting::auto_sleep_minutes(90).marker());
    }

    #[test]
    fn renewing_reacquires_a_cleared_lock_and_leaves_a_held_one_alone() {
        let script = wake_lock_renew_script();
        assert!(script.contains(WAKE_LOCK_NAME));
        assert!(script.contains("/sys/power/wake_lock"));
        // Renewal must never release, and must touch nothing else.
        assert!(!script.contains("wake_unlock"));
        assert!(!script.contains("/sys/power/state"));
        assert!(!script.contains("autosleep"));
        assert!(script.contains("renew: held"));
        assert!(script.contains("renew: reacquired"));
        assert!(script.ends_with("exit\n"));
    }

    #[test]
    fn enabling_is_refused_when_the_firmware_has_no_such_setting() {
        // A settings file the reader does not implement would look like a
        // success and change nothing, so it must be refused outright.
        let original = "[Reading]\nfoo=bar\n";
        apply_with_firmware(Switch::On, original, false)
            .expect_err("unsupported firmware must be refused");
    }

    #[test]
    fn disabling_never_depends_on_the_firmware_probe() {
        // Recovery must work even on firmware the probe does not recognise.
        let original = "[DeveloperSettings]\nForceWifiOn=true\n";
        let result = apply_with_firmware(Switch::Off, original, false).expect("apply");
        assert!(!result.contains("ForceWifiOn"));
    }

    #[test]
    fn status_reports_support_and_whether_a_restart_is_still_pending() {
        let script = status_script();
        // Every setting this tool can change must be reported.
        for setting in [Setting::force_wifi_on(), Setting::auto_sleep_minutes(0)] {
            assert!(script.contains(&format!("{}_supported:", setting.key)));
            assert!(script.contains(&format!("{}_pending_restart:", setting.key)));
            assert!(script.contains(&setting.marker()));
        }
        // Suspend evidence, which is what the uptime clock cannot show.
        assert!(script.contains("suspend_events:"));
        assert!(script.contains("kernel_awake_seconds:"));
        // Pending restart must be decided from our own marker, never from the
        // settings file: the reader rewrites that file during normal operation,
        // so its modification time says nothing about what the reader has read.
        assert!(script.contains("/proc/$reader_pid"));
        assert!(script.contains(APPLIED_MARKER_PREFIX));
        assert!(
            !script.contains(&format!("'{READER_CONFIG}' -nt")),
            "pending restart must not be decided from the settings file"
        );
        assert!(script.contains(READER_BINARIES[0]));
        // With no marker the honest answer is that we do not know.
        assert!(script.contains("_pending_restart: unknown"));
        // Status must never modify anything.
        for forbidden in [
            "cp ",
            "cat \"$tmp\"",
            "rm ",
            "wake_unlock",
            "reboot",
            "touch ",
        ] {
            assert!(
                !script.contains(forbidden),
                "status wrote something: {forbidden}"
            );
        }
    }

    #[test]
    fn enabling_inserts_the_key_inside_an_existing_developer_section() {
        let original = "[Reading]\nfoo=bar\n\n[DeveloperSettings]\nEnableDebugServices=true\n\n[FeatureSettings]\nbaz=1\n";
        let result = apply(Switch::On, original).expect("apply");
        assert!(
            result.contains("[DeveloperSettings]\nEnableDebugServices=true\nForceWifiOn=true\n")
        );
        assert!(result.contains("[FeatureSettings]\nbaz=1\n"));
        assert!(result.contains("[Reading]\nfoo=bar\n"));
        assert_eq!(result.matches("ForceWifiOn").count(), 1);
    }

    #[test]
    fn enabling_replaces_an_existing_value_and_is_idempotent() {
        let original = "[DeveloperSettings]\nForceWifiOn=false\n";
        let once = apply(Switch::On, original).expect("apply");
        assert_eq!(once, "[DeveloperSettings]\nForceWifiOn=true\n");
        let twice = apply(Switch::On, &once).expect("apply again");
        assert_eq!(twice, once);
    }

    #[test]
    fn enabling_creates_the_section_when_the_device_has_none() {
        let original = "[Reading]\nfoo=bar\n";
        let result = apply(Switch::On, original).expect("apply");
        assert!(result.starts_with("[Reading]\nfoo=bar\n"));
        assert!(result.contains("[DeveloperSettings]\nForceWifiOn=true\n"));
    }

    #[test]
    fn disabling_removes_only_that_key() {
        let original = "[DeveloperSettings]\nEnableDebugServices=true\nForceWifiOn=true\n";
        let result = apply(Switch::Off, original).expect("apply");
        assert_eq!(result, "[DeveloperSettings]\nEnableDebugServices=true\n");
    }

    #[test]
    fn an_identically_named_key_in_another_section_is_left_alone() {
        let original = "[Powersave]\nForceWifiOn=false\n";
        let result = apply(Switch::On, original).expect("apply");
        assert!(result.starts_with("[Powersave]\nForceWifiOn=false\n"));
        assert!(result.contains("[DeveloperSettings]\nForceWifiOn=true\n"));
    }

    #[test]
    fn a_missing_or_empty_settings_file_is_refused() {
        assert!(apply(Switch::On, "").is_err());
        assert!(apply(Switch::Off, "").is_err());
    }

    #[test]
    fn status_and_restore_are_read_only_or_backup_bound() {
        let status = status_script();
        assert!(!status.contains('>') || status.contains("printf"));
        assert!(!status.contains("cat \"$tmp\""));
        assert!(status.contains("/sys/power/wake_lock"));
        assert!(status.contains("operstate"));

        let restore = restore_config_script();
        assert!(restore.contains("no backup to restore"));
        assert!(restore.contains(READER_CONFIG_BACKUP));
    }
}
