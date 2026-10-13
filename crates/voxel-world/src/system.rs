//! CPU hotplug management and lightweight telemetry read from sysfs/procfs.
//!
//! The third-party firmware keeps only cpu0-1 online to save power. The game
//! brings the remaining cores online while it runs and restores the recorded
//! state when the option is turned off, on exit and on unwinding. The launch
//! scripts restore the same state again after crashes or SIGKILL. CPU
//! frequency governors are left to the firmware's Fn scripts.
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const CPU_ROOT: &str = "/sys/devices/system/cpu";
pub const THERMAL_ROOT: &str = "/sys/class/thermal";
const CHECK_INTERVAL: Duration = Duration::from_secs(5);
const MAX_RETRIES: u32 = 3;

/// Parses kernel CPU lists such as `0-1,3`.
pub fn parse_cpu_list(text: &str) -> Vec<usize> {
    let mut cpus = Vec::new();
    for part in text.trim().split(',').filter(|p| !p.is_empty()) {
        let mut bounds = part.splitn(2, '-');
        let Some(Ok(start)) = bounds.next().map(|s| s.trim().parse::<usize>()) else {
            continue;
        };
        let end = match bounds.next() {
            Some(s) => match s.trim().parse::<usize>() {
                Ok(end) if end >= start && end - start < 1024 => end,
                _ => continue,
            },
            None => start,
        };
        cpus.extend(start..=end);
    }
    cpus.sort_unstable();
    cpus.dedup();
    cpus
}

/// Formats CPU numbers compactly, e.g. `[0, 1, 2, 3]` becomes `0-3`.
pub fn format_cpu_list(cpus: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < cpus.len() {
        let mut j = i;
        while j + 1 < cpus.len() && cpus[j + 1] == cpus[j] + 1 {
            j += 1;
        }
        parts.push(if i == j {
            cpus[i].to_string()
        } else {
            format!("{}-{}", cpus[i], cpus[j])
        });
        i = j + 1;
    }
    if parts.is_empty() {
        "?".into()
    } else {
        parts.join(",")
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuState {
    /// The option is off; cores are in their original state.
    Default,
    /// The option is on and every present core is online.
    AllOnline,
    /// The system took cores offline again and retries were exhausted.
    Blocked,
    /// Hotplug files are missing or not writable.
    Unsupported,
}
impl CpuState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "系统默认",
            Self::AllOnline => "全部在线",
            Self::Blocked => "被系统关闭",
            Self::Unsupported => "不支持",
        }
    }
}

pub struct CpuManager {
    root: PathBuf,
    present: Vec<usize>,
    /// Cores this process brought online; they were offline originally.
    changed: Vec<usize>,
    enabled: bool,
    retries: u32,
    state: CpuState,
    next_check: Instant,
}

impl CpuManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let present = read_trimmed(&root.join("present"))
            .map(|s| parse_cpu_list(&s))
            .unwrap_or_default();
        Self {
            root,
            present,
            changed: Vec::new(),
            enabled: false,
            retries: 0,
            state: CpuState::Default,
            next_check: Instant::now() + CHECK_INTERVAL,
        }
    }
    pub fn present(&self) -> &[usize] {
        &self.present
    }
    pub fn online(&self) -> Vec<usize> {
        read_trimmed(&self.root.join("online"))
            .map(|s| parse_cpu_list(&s))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec![0])
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn state(&self) -> CpuState {
        self.state
    }
    fn online_path(&self, cpu: usize) -> PathBuf {
        self.root.join(format!("cpu{cpu}")).join("online")
    }
    fn is_online(&self, cpu: usize) -> bool {
        // cpu0 commonly has no hotplug file and is always online.
        read_trimmed(&self.online_path(cpu)).is_none_or(|s| s != "0")
    }
    fn write_online(&self, cpu: usize, online: bool) -> bool {
        match fs::write(self.online_path(cpu), if online { "1" } else { "0" }) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("CPU hotplug: cpu{cpu} -> {}: {e}", u8::from(online));
                false
            }
        }
    }
    fn bring_online(&mut self) -> bool {
        let mut ok = true;
        for &cpu in &self.present.clone() {
            if cpu == 0 || !self.online_path(cpu).is_file() || self.is_online(cpu) {
                continue;
            }
            if self.write_online(cpu, true) {
                if !self.changed.contains(&cpu) {
                    self.changed.push(cpu);
                }
            } else {
                ok = false;
            }
        }
        ok
    }
    /// Turns the "all cores" option on or off. Safe to call repeatedly.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        self.retries = 0;
        if enabled {
            let hotplug = self
                .present
                .iter()
                .any(|&cpu| cpu != 0 && self.online_path(cpu).is_file());
            self.state = if !hotplug || !self.bring_online() {
                CpuState::Unsupported
            } else {
                CpuState::AllOnline
            };
        } else {
            self.restore();
            self.state = CpuState::Default;
        }
        self.next_check = Instant::now() + CHECK_INTERVAL;
        log::info!(
            "CPU cores: option={}, state={:?}, online {}",
            if enabled { "all" } else { "default" },
            self.state,
            format_cpu_list(&self.online())
        );
    }
    /// Periodically re-checks cores the system may take offline again.
    pub fn poll(&mut self, now: Instant) {
        if now < self.next_check {
            return;
        }
        self.next_check = now + CHECK_INTERVAL;
        if !self.enabled || self.state != CpuState::AllOnline {
            return;
        }
        let offline = self
            .present
            .iter()
            .any(|&cpu| cpu != 0 && self.online_path(cpu).is_file() && !self.is_online(cpu));
        if !offline {
            return;
        }
        if self.retries < MAX_RETRIES {
            self.retries += 1;
            log::warn!(
                "CPU cores were taken offline; retry {}/{MAX_RETRIES}",
                self.retries
            );
            self.bring_online();
        } else {
            self.state = CpuState::Blocked;
            log::warn!(
                "CPU cores kept going offline; running with {}",
                format_cpu_list(&self.online())
            );
        }
    }
    /// Restores every core this process brought online to offline.
    pub fn restore(&mut self) {
        for cpu in std::mem::take(&mut self.changed).into_iter().rev() {
            self.write_online(cpu, false);
        }
    }
    /// Background workers that may run concurrently with the main thread.
    pub fn worker_budget(&self) -> usize {
        self.online().len().saturating_sub(1).clamp(1, 3)
    }
}

impl Drop for CpuManager {
    fn drop(&mut self) {
        if !self.changed.is_empty() {
            self.restore();
            log::info!(
                "CPU cores restored: online {}",
                format_cpu_list(&self.online())
            );
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Telemetry {
    pub online: Vec<usize>,
    /// Current frequency per present core; `None` while offline or unknown.
    pub freqs_mhz: Vec<(usize, Option<u32>)>,
    pub governor: String,
    pub temps: Vec<(String, f32)>,
    pub rss_mb: f32,
}

impl Telemetry {
    pub fn sample(cpu_root: &Path, thermal_root: &Path, present: &[usize]) -> Self {
        let online = read_trimmed(&cpu_root.join("online"))
            .map(|s| parse_cpu_list(&s))
            .unwrap_or_default();
        let freqs_mhz = present
            .iter()
            .map(|&cpu| {
                let freq = online
                    .contains(&cpu)
                    .then(|| {
                        read_trimmed(&cpu_root.join(format!("cpu{cpu}/cpufreq/scaling_cur_freq")))
                    })
                    .flatten()
                    .and_then(|s| s.parse::<u32>().ok())
                    .map(|khz| khz / 1000);
                (cpu, freq)
            })
            .collect();
        let governor = read_trimmed(&cpu_root.join("cpu0/cpufreq/scaling_governor"))
            .unwrap_or_else(|| "?".into());
        let mut temps = Vec::new();
        if let Ok(entries) = fs::read_dir(thermal_root) {
            let mut zones: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("thermal_zone"))
                })
                .collect();
            zones.sort();
            for zone in zones {
                let kind = read_trimmed(&zone.join("type")).unwrap_or_default();
                let label = if kind.contains("cpu") {
                    "CPU"
                } else if kind.contains("gpu") {
                    "GPU"
                } else {
                    continue;
                };
                if let Some(t) =
                    read_trimmed(&zone.join("temp")).and_then(|s| s.parse::<f32>().ok())
                {
                    temps.push((label.to_owned(), t / 1000.0));
                }
            }
        }
        Self {
            online,
            freqs_mhz,
            governor,
            temps,
            rss_mb: rss_bytes() as f32 / 1048576.0,
        }
    }
    pub fn online_label(&self) -> String {
        format_cpu_list(&self.online)
    }
    pub fn freq_label(&self) -> String {
        self.freqs_mhz
            .iter()
            .map(|(_, f)| f.map_or("-".to_owned(), |f| f.to_string()))
            .collect::<Vec<_>>()
            .join("/")
    }
    pub fn temp_label(&self) -> String {
        if self.temps.is_empty() {
            return "-".into();
        }
        self.temps
            .iter()
            .map(|(_, t)| format!("{t:.1}"))
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// Resident set size of this process.
pub fn rss_bytes() -> u64 {
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * page)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_sysfs(name: &str, online: &[u8]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("voxel-cpu-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("present"), "0-3\n").unwrap();
        for (cpu, state) in online.iter().enumerate() {
            let dir = root.join(format!("cpu{cpu}"));
            fs::create_dir_all(dir.join("cpufreq")).unwrap();
            if cpu > 0 {
                fs::write(dir.join("online"), format!("{state}\n")).unwrap();
            }
            fs::write(dir.join("cpufreq/scaling_cur_freq"), "1800000\n").unwrap();
        }
        fs::write(root.join("cpu0/cpufreq/scaling_governor"), "ondemand\n").unwrap();
        sync_online(&root);
        root
    }
    /// Emulates the kernel's aggregate `online` file.
    fn sync_online(root: &Path) {
        let mut online = vec![0];
        for cpu in 1..4 {
            if read_trimmed(&root.join(format!("cpu{cpu}/online"))).as_deref() == Some("1") {
                online.push(cpu);
            }
        }
        fs::write(root.join("online"), format_cpu_list(&online)).unwrap();
    }

    #[test]
    fn cpu_lists_round_trip() {
        assert_eq!(parse_cpu_list("0-1\n"), vec![0, 1]);
        assert_eq!(parse_cpu_list("0,2-3"), vec![0, 2, 3]);
        assert_eq!(parse_cpu_list("bad,1"), vec![1]);
        assert_eq!(parse_cpu_list("3-1"), Vec::<usize>::new());
        assert_eq!(format_cpu_list(&[0, 1, 2, 3]), "0-3");
        assert_eq!(format_cpu_list(&[0, 2, 3]), "0,2-3");
        assert_eq!(format_cpu_list(&[]), "?");
    }

    #[test]
    fn enable_restore_and_only_touch_changed_cores() {
        // cpu1 is already online and must stay online after restore.
        let root = fake_sysfs("restore", &[1, 1, 0, 0]);
        let mut cpu = CpuManager::new(&root);
        assert_eq!(cpu.present(), &[0, 1, 2, 3]);
        cpu.set_enabled(true);
        sync_online(&root);
        assert_eq!(cpu.state(), CpuState::AllOnline);
        assert_eq!(cpu.online(), vec![0, 1, 2, 3]);
        assert_eq!(cpu.worker_budget(), 3);
        cpu.set_enabled(false);
        sync_online(&root);
        assert_eq!(cpu.online(), vec![0, 1]);
        assert_eq!(cpu.worker_budget(), 1);
        assert_eq!(read_trimmed(&root.join("cpu1/online")).unwrap(), "1");
        cpu.set_enabled(true);
        drop(cpu);
        sync_online(&root);
        assert_eq!(read_trimmed(&root.join("online")).unwrap(), "0-1");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retries_then_reports_blocked() {
        let root = fake_sysfs("blocked", &[1, 1, 0, 0]);
        let mut cpu = CpuManager::new(&root);
        cpu.set_enabled(true);
        let mut now = Instant::now();
        for attempt in 0..=MAX_RETRIES {
            // The "system" turns cpu3 off again after every retry.
            fs::write(root.join("cpu3/online"), "0").unwrap();
            now += CHECK_INTERVAL;
            cpu.poll(now);
            if attempt < MAX_RETRIES {
                assert_eq!(cpu.state(), CpuState::AllOnline);
                assert_eq!(read_trimmed(&root.join("cpu3/online")).unwrap(), "1");
            }
        }
        assert_eq!(cpu.state(), CpuState::Blocked);
        assert_eq!(cpu.state().label(), "被系统关闭");
        drop(cpu);
        assert_eq!(read_trimmed(&root.join("cpu2/online")).unwrap(), "0");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_hotplug_is_unsupported_and_telemetry_formats() {
        let root = std::env::temp_dir().join(format!("voxel-cpu-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cpu = CpuManager::new(&root);
        cpu.set_enabled(true);
        assert_eq!(cpu.state(), CpuState::Unsupported);
        assert_eq!(cpu.worker_budget(), 1);

        let root = fake_sysfs("telemetry", &[1, 1, 0, 0]);
        let thermal = root.join("thermal");
        for (i, (kind, temp)) in [
            ("cpu_thermal_zone", "60796"),
            ("ddr", "1"),
            ("gpu_thermal_zone", "58719"),
        ]
        .iter()
        .enumerate()
        {
            let zone = thermal.join(format!("thermal_zone{i}"));
            fs::create_dir_all(&zone).unwrap();
            fs::write(zone.join("type"), kind).unwrap();
            fs::write(zone.join("temp"), temp).unwrap();
        }
        let t = Telemetry::sample(&root, &thermal, &[0, 1, 2, 3]);
        assert_eq!(t.online_label(), "0-1");
        assert_eq!(t.freq_label(), "1800/1800/-/-");
        assert_eq!(t.governor, "ondemand");
        assert_eq!(t.temp_label(), "60.8/58.7");
        assert!(t.rss_mb > 0.0);
        fs::remove_dir_all(root).unwrap();
    }
}
