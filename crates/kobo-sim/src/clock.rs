//! Simulator-owned time. Sleep completion follows manual time when selected;
//! real HTTP transport timeouts retain their production deadline.
use kobo_policy::clock::{Clock, ManualClock, Snapshot, SystemClock};
use std::{io, sync::Arc, time::Duration};
#[derive(Clone, Debug)]
pub enum Time {
    Real(Arc<SystemClock>),
    Manual(Arc<ManualClock>),
}
impl Default for Time {
    fn default() -> Self {
        Self::Real(Arc::new(SystemClock::new(0).expect("UTC offset")))
    }
}
impl Time {
    pub fn configured() -> io::Result<Self> {
        let offset = std::env::var_os("KOBO_SIM_UTC_OFFSET_MINUTES")
            .map(|v| {
                v.to_str()
                    .and_then(|s| s.parse::<i16>().ok())
                    .ok_or_else(|| io::Error::other("invalid simulator UTC offset"))
            })
            .transpose()?
            .unwrap_or(0);
        let Some(value) = std::env::var_os("KOBO_SIM_CLOCK_MILLIS") else {
            return SystemClock::new(offset)
                .map(|clock| Self::Real(Arc::new(clock)))
                .map_err(|_| io::Error::other("invalid simulator UTC offset"));
        };
        let unix_millis = value
            .to_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| io::Error::other("KOBO_SIM_CLOCK_MILLIS must be Unix milliseconds"))?;
        ManualClock::new(Snapshot {
            unix_millis,
            monotonic_millis: 0,
            utc_offset_minutes: offset,
        })
        .map(|clock| Self::Manual(Arc::new(clock)))
        .map_err(|_| io::Error::other("simulator clock is out of range"))
    }
    pub fn now(&self) -> io::Result<Snapshot> {
        match self {
            Self::Real(clock) => clock.now(),
            Self::Manual(clock) => clock.now(),
        }
        .map_err(|_| io::Error::other("simulator clock unavailable"))
    }
    pub fn manual(&self) -> Option<Arc<ManualClock>> {
        match self {
            Self::Manual(clock) => Some(Arc::clone(clock)),
            Self::Real(_) => None,
        }
    }
    pub fn change(&self, command: &str) -> io::Result<Snapshot> {
        let Some(clock) = self.manual() else {
            return Err(io::Error::other(
                "start with KOBO_SIM_CLOCK_MILLIS to control time",
            ));
        };
        let parts = command.split_whitespace().collect::<Vec<_>>();
        let result = match parts.as_slice() {
            ["advance", millis] => millis
                .parse::<u64>()
                .ok()
                .filter(|&ms| ms <= 7 * 86_400_000)
                .and_then(|ms| clock.advance(Duration::from_millis(ms)).ok()),
            ["set", millis, offset] => millis
                .parse()
                .ok()
                .zip(offset.parse().ok())
                .and_then(|(ms, zone)| clock.set_wall(ms, zone).ok()),
            _ => None,
        };
        result.ok_or_else(|| io::Error::other("clock expects advance MILLISECONDS (at most 7 days) or set UNIX_MILLISECONDS UTC_OFFSET_MINUTES"))
    }
    pub fn json(&self, snapshot: Snapshot) -> kobo_json::Value {
        kobo_json::ObjectBuilder::new()
            .set(
                "mode",
                if self.manual().is_some() {
                    "manual"
                } else {
                    "real"
                },
            )
            .set("unixMillis", snapshot.unix_millis.to_string())
            .set("monotonicMillis", snapshot.monotonic_millis.to_string())
            .set("utcOffsetMinutes", i32::from(snapshot.utc_offset_minutes))
            .set(
                "date",
                snapshot.date().map_or(kobo_json::Value::Null, |date| {
                    kobo_json::Value::from(date.to_string())
                }),
            )
            .build()
    }
}
