//! Bounded Linux activity samples for the built-in Settings application.

use kobo_protocol::MAX_PROCESS_NAME_LEN;
#[cfg(feature = "device-write")]
use kobo_protocol::{ProcessActivity, SystemActivity, MAX_ACTIVITY_PROCESSES};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
#[cfg(feature = "device-write")]
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

#[derive(Debug)]
struct RawProcess {
    pid: u32,
    name: String,
    ticks: u64,
    memory_bytes: u64,
}

/// Samples procfs deltas while retaining only the previous process counters.
#[cfg(feature = "device-write")]
pub struct ActivityMonitor {
    proc_root: PathBuf,
    disk_root: PathBuf,
    previous_cpu: Option<CpuTimes>,
    previous_process_ticks: BTreeMap<u32, u64>,
}

#[cfg(feature = "device-write")]
impl ActivityMonitor {
    #[must_use]
    pub fn system(disk_root: &Path) -> Self {
        Self::new(Path::new("/proc"), disk_root)
    }

    #[must_use]
    fn new(proc_root: &Path, disk_root: &Path) -> Self {
        Self {
            proc_root: proc_root.to_path_buf(),
            disk_root: disk_root.to_path_buf(),
            previous_cpu: None,
            previous_process_ticks: BTreeMap::new(),
        }
    }

    /// Reads one system sample and computes CPU from the interval since the
    /// previous call.
    ///
    /// # Errors
    ///
    /// Returns an error when the aggregate CPU or memory readings are missing.
    /// Individual processes may disappear while `/proc` is being walked and
    /// are skipped rather than failing the complete sample.
    pub fn sample(&mut self) -> io::Result<SystemActivity> {
        let cpu = cpu_times(&fs::read_to_string(self.proc_root.join("stat"))?)?;
        let (memory_used_bytes, memory_total_bytes) =
            memory(&fs::read_to_string(self.proc_root.join("meminfo"))?)?;
        let processes = read_processes(&self.proc_root);
        let interval = self
            .previous_cpu
            .map_or(0, |previous| cpu.total.saturating_sub(previous.total));
        let cpu_tenths = self.previous_cpu.map_or(0, |previous| {
            let total = cpu.total.saturating_sub(previous.total);
            let idle = cpu.idle.saturating_sub(previous.idle);
            percent_tenths(total.saturating_sub(idle), total)
        });

        let mut next_ticks = BTreeMap::new();
        let mut reported = processes
            .into_iter()
            .map(|process| {
                next_ticks.insert(process.pid, process.ticks);
                let changed = self
                    .previous_process_ticks
                    .get(&process.pid)
                    .map_or(0, |previous| process.ticks.saturating_sub(*previous));
                ProcessActivity {
                    pid: process.pid,
                    name: process.name,
                    cpu_tenths: percent_tenths(changed, interval),
                    memory_bytes: process.memory_bytes,
                }
            })
            .collect::<Vec<_>>();
        reported.sort_by(|left, right| {
            right
                .cpu_tenths
                .cmp(&left.cpu_tenths)
                .then_with(|| right.memory_bytes.cmp(&left.memory_bytes))
                .then_with(|| left.pid.cmp(&right.pid))
        });
        reported.truncate(MAX_ACTIVITY_PROCESSES);

        self.previous_cpu = Some(cpu);
        self.previous_process_ticks = next_ticks;
        Ok(SystemActivity {
            cpu_tenths,
            memory_used_bytes,
            memory_total_bytes,
            disk_free_bytes: kobo_abi::free_space(&self.disk_root),
            processes: reported,
        })
    }
}

fn cpu_times(stat: &str) -> io::Result<CpuTimes> {
    let line = stat
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing aggregate CPU"))?;
    let values = line
        .split_ascii_whitespace()
        .skip(1)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid aggregate CPU"))?;
    if values.len() < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short aggregate CPU",
        ));
    }
    // Guest and guest_nice are already included in user and nice respectively.
    let total = values
        .iter()
        .take(8)
        .copied()
        .fold(0_u64, u64::saturating_add);
    let idle = values[3].saturating_add(values.get(4).copied().unwrap_or(0));
    Ok(CpuTimes { total, idle })
}

fn memory(meminfo: &str) -> io::Result<(u64, u64)> {
    let values = meminfo
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let kib = rest.split_ascii_whitespace().next()?.parse::<u64>().ok()?;
            Some((name, kib.saturating_mul(1024)))
        })
        .collect::<BTreeMap<_, _>>();
    let total = values
        .get("MemTotal")
        .copied()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing total memory"))?;
    let available = values.get("MemAvailable").copied().unwrap_or_else(|| {
        ["MemFree", "Buffers", "Cached"]
            .into_iter()
            .filter_map(|name| values.get(name).copied())
            .fold(0_u64, u64::saturating_add)
    });
    Ok((total.saturating_sub(available.min(total)), total))
}

#[cfg(feature = "device-write")]
fn read_processes(proc_root: &Path) -> Vec<RawProcess> {
    let Ok(entries) = fs::read_dir(proc_root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            read_process(&entry.path(), pid)
        })
        .collect()
}

fn read_process(path: &Path, pid: u32) -> Option<RawProcess> {
    let stat = fs::read_to_string(path.join("stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    let fields = after_name.split_ascii_whitespace().collect::<Vec<_>>();
    let user = fields.get(11)?.parse::<u64>().ok()?;
    let system = fields.get(12)?.parse::<u64>().ok()?;
    let status = fs::read_to_string(path.join("status")).ok()?;
    let memory_bytes = status
        .lines()
        .find_map(|line| {
            let rest = line.strip_prefix("VmRSS:")?;
            rest.split_ascii_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kib| kib.saturating_mul(1024))
        })
        .unwrap_or(0);
    let name = fs::read_to_string(path.join("comm"))
        .ok()
        .map_or_else(|| format!("pid-{pid}"), |name| safe_name(&name));
    Some(RawProcess {
        pid,
        name,
        ticks: user.saturating_add(system),
        memory_bytes,
    })
}

fn safe_name(name: &str) -> String {
    let cleaned = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '?'
            }
        })
        .take(MAX_PROCESS_NAME_LEN)
        .collect::<String>();
    if cleaned.is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}

fn percent_tenths(part: u64, whole: u64) -> u16 {
    if whole == 0 {
        return 0;
    }
    let tenths = u128::from(part).saturating_mul(1000) / u128::from(whole);
    u16::try_from(tenths.min(1000)).unwrap_or(1000)
}

#[cfg(test)]
mod tests {
    use super::{cpu_times, memory, percent_tenths, read_process, safe_name, CpuTimes};
    use std::fs;

    #[test]
    fn aggregate_cpu_counts_idle_and_iowait_as_idle() {
        assert_eq!(
            cpu_times("cpu  100 2 30 400 8 3 4 1\n").expect("cpu"),
            CpuTimes {
                total: 548,
                idle: 408,
            }
        );
    }

    #[test]
    fn memory_prefers_the_kernels_available_estimate() {
        let (used, total) =
            memory("MemTotal: 512000 kB\nMemAvailable: 128000 kB\n").expect("memory");
        assert_eq!(total, 512_000 * 1024);
        assert_eq!(used, 384_000 * 1024);
    }

    #[test]
    fn percentages_are_bounded_and_names_are_safe_for_the_panel() {
        assert_eq!(percent_tenths(1, 4), 250);
        assert_eq!(percent_tenths(8, 4), 1000);
        assert_eq!(safe_name("worker\n"), "worker");
        assert_eq!(safe_name("\u{2603}"), "?");
    }

    #[test]
    fn a_process_stat_yields_cpu_memory_name_and_pid() {
        let root =
            std::env::temp_dir().join(format!("cobalt-activity-process-{}", std::process::id()));
        let process = root.join("42");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&process).expect("process directory");
        fs::write(
            process.join("stat"),
            "42 (worker name) S 1 1 1 0 0 0 0 0 0 0 25 5\n",
        )
        .expect("stat");
        fs::write(process.join("status"), "Name:\tworker\nVmRSS:\t123 kB\n").expect("status");
        fs::write(process.join("comm"), "worker name\n").expect("comm");
        let sampled = read_process(&process, 42).expect("process sample");
        assert_eq!(sampled.pid, 42);
        assert_eq!(sampled.name, "worker name");
        assert_eq!(sampled.ticks, 30);
        assert_eq!(sampled.memory_bytes, 123 * 1024);
        let _ = fs::remove_dir_all(root);
    }
}
