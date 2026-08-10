//! Persistent global sleep settings and per-application overrides.

use kobo_protocol::DeviceError;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const DEFAULT_SLEEP_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const MINIMUM_SLEEP_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAXIMUM_SLEEP_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const STATE_DIRECTORY: &str = "state";
const DIRECTORY: &str = "power";
const FILE: &str = "sleep-after-seconds";

/// The owner-selected timeout shared by applications that do not override it.
pub struct GlobalSleep {
    directory: PathBuf,
    path: PathBuf,
    timeout: Duration,
}

impl GlobalSleep {
    /// Loads the saved value, falling back when this reader has never saved one.
    #[must_use]
    pub fn load(root: &Path, fallback: Duration) -> Self {
        // `state` is carried into every in-place platform update, so the
        // owner's choice survives replacement of the runtime binaries.
        let directory = root.join(STATE_DIRECTORY).join(DIRECTORY);
        let path = directory.join(FILE);
        let timeout = fs::read_to_string(&path)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .filter(|value| valid_timeout(*value))
            .unwrap_or_else(|| clamp_timeout(fallback));
        Self {
            directory,
            path,
            timeout,
        }
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Atomically publishes a validated timeout on the user-visible partition.
    pub fn set_seconds(&mut self, seconds: u32) -> Result<Duration, DeviceError> {
        let timeout = Duration::from_secs(u64::from(seconds));
        if !valid_timeout(timeout) {
            return Err(DeviceError::InvalidInput);
        }
        fs::create_dir_all(&self.directory).map_err(|_| DeviceError::Backend)?;
        let temporary = self.directory.join(format!(".{FILE}.writing"));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| DeviceError::Backend)?;
        writeln!(file, "{}", timeout.as_secs()).map_err(|_| DeviceError::Backend)?;
        file.sync_all().map_err(|_| DeviceError::Backend)?;
        fs::rename(&temporary, &self.path).map_err(|_| DeviceError::Backend)?;
        OpenOptions::new()
            .read(true)
            .open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| DeviceError::Backend)?;
        self.timeout = timeout;
        Ok(timeout)
    }
}

/// A foreground application's departure from the global timeout.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AppSleep {
    timeout: Option<Duration>,
    awake_until: Option<Instant>,
}

impl AppSleep {
    pub fn override_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(clamp_timeout(timeout));
    }

    pub fn use_global_timeout(&mut self) {
        self.timeout = None;
    }

    pub fn keep_awake(&mut self, now: Instant, duration: Duration) {
        self.awake_until = now.checked_add(duration);
    }

    pub fn allow_sleep(&mut self) {
        self.awake_until = None;
    }

    #[must_use]
    pub fn deadline(&self, last_activity: Instant, global: Duration, now: Instant) -> Instant {
        let idle = last_activity + self.timeout.unwrap_or(global);
        self.awake_until
            .filter(|until| *until > now)
            .map_or(idle, |until| idle.max(until))
    }
}

#[must_use]
pub fn clamp_timeout(timeout: Duration) -> Duration {
    timeout.clamp(MINIMUM_SLEEP_TIMEOUT, MAXIMUM_SLEEP_TIMEOUT)
}

#[must_use]
pub fn valid_timeout(timeout: Duration) -> bool {
    (MINIMUM_SLEEP_TIMEOUT..=MAXIMUM_SLEEP_TIMEOUT).contains(&timeout)
}

#[cfg(test)]
mod tests {
    use super::{
        AppSleep, GlobalSleep, DEFAULT_SLEEP_TIMEOUT, MAXIMUM_SLEEP_TIMEOUT, MINIMUM_SLEEP_TIMEOUT,
    };
    use kobo_protocol::DeviceError;
    use std::fs;
    use std::time::{Duration, Instant};

    fn root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("cobalt-power-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn a_reader_with_no_setting_uses_the_global_default() {
        let root = root("default");
        assert_eq!(
            GlobalSleep::load(&root, DEFAULT_SLEEP_TIMEOUT).timeout(),
            DEFAULT_SLEEP_TIMEOUT
        );
    }

    #[test]
    fn a_saved_timeout_survives_a_new_runtime() {
        let root = root("saved");
        let mut settings = GlobalSleep::load(&root, DEFAULT_SLEEP_TIMEOUT);
        settings.set_seconds(30 * 60).expect("save timeout");
        assert!(root.join("state/power/sleep-after-seconds").is_file());
        assert_eq!(
            GlobalSleep::load(&root, DEFAULT_SLEEP_TIMEOUT).timeout(),
            Duration::from_secs(30 * 60)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_global_timeout_outside_the_safe_range_is_refused() {
        let root = root("range");
        let mut settings = GlobalSleep::load(&root, DEFAULT_SLEEP_TIMEOUT);
        assert_eq!(settings.set_seconds(0), Err(DeviceError::InvalidInput));
        assert_eq!(
            settings.set_seconds(u32::try_from(MAXIMUM_SLEEP_TIMEOUT.as_secs() + 1).unwrap()),
            Err(DeviceError::InvalidInput)
        );
        assert_eq!(settings.timeout(), DEFAULT_SLEEP_TIMEOUT);
    }

    #[test]
    fn an_app_uses_global_until_it_explicitly_overrides() {
        let now = Instant::now();
        let mut app = AppSleep::default();
        assert_eq!(
            app.deadline(now, Duration::from_secs(15 * 60), now),
            now + Duration::from_secs(15 * 60)
        );
        app.override_timeout(Duration::from_secs(5 * 60));
        assert_eq!(
            app.deadline(now, Duration::from_secs(15 * 60), now),
            now + Duration::from_secs(5 * 60)
        );
        app.use_global_timeout();
        assert_eq!(
            app.deadline(now, Duration::from_secs(15 * 60), now),
            now + Duration::from_secs(15 * 60)
        );
    }

    #[test]
    fn a_wake_hold_delays_but_does_not_replace_the_timeout() {
        let now = Instant::now();
        let mut app = AppSleep::default();
        app.keep_awake(now, Duration::from_secs(20 * 60));
        assert_eq!(
            app.deadline(now, Duration::from_secs(15 * 60), now),
            now + Duration::from_secs(20 * 60)
        );
        app.allow_sleep();
        assert_eq!(
            app.deadline(now, Duration::from_secs(15 * 60), now),
            now + Duration::from_secs(15 * 60)
        );
    }

    #[test]
    fn app_overrides_are_clamped_to_the_same_global_bounds() {
        let now = Instant::now();
        let mut app = AppSleep::default();
        app.override_timeout(Duration::ZERO);
        assert_eq!(
            app.deadline(now, DEFAULT_SLEEP_TIMEOUT, now),
            now + MINIMUM_SLEEP_TIMEOUT
        );
    }
}
