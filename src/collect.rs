/// collect.rs — System metrics collection
///
/// CPU and memory are read directly from /proc/stat and /proc/meminfo,
/// matching btop's approach of single-syscall reads instead of library overhead.
///
/// Disk space uses sysinfo. Disk IO comes from /proc/diskstats directly.
/// Processes use sysinfo with a minimal refresh (no cmd, no env, no open files).
///
/// Security note: All /proc reads are read-only. No writes, no spawning.

use anyhow::{Context, Result};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, BufRead};
use std::sync::Arc;
use sysinfo::Disks;  // kept only for disk-space stats (statvfs)
use tracing::debug;

// ── Public data types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Snapshot {
    pub cpu: CpuSnapshot,
    pub memory: MemSnapshot,
    pub disks: Arc<Vec<DiskSnapshot>>,
    pub network: NetworkSnapshot,
    pub processes: Vec<ProcessSnapshot>,
    pub temps: Arc<Vec<TempSnapshot>>,
    pub timestamp: std::time::Instant,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            cpu: CpuSnapshot::default(),
            memory: MemSnapshot::default(),
            disks: Arc::new(Vec::new()),
            network: NetworkSnapshot::default(),
            processes: Vec::new(),
            temps: Arc::new(Vec::new()),
            timestamp: std::time::Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CpuSnapshot {
    pub total_usage: f32,
    pub core_usage: Vec<f32>,
    pub brand: String,
    pub freq_mhz: Option<u64>,
    pub history: VecDeque<f32>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct MemSnapshot {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub history: VecDeque<f32>,
}

impl MemSnapshot {
    pub fn used_pct(&self) -> f32 {
        if self.total_bytes == 0 { return 0.0; }
        (self.used_bytes as f32 / self.total_bytes as f32) * 100.0
    }
    pub fn swap_pct(&self) -> f32 {
        if self.swap_total == 0 { return 0.0; }
        (self.swap_used as f32 / self.swap_total as f32) * 100.0
    }
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct DiskSnapshot {
    pub name: String,
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub is_removable: bool,
    pub read_bytes_per_sec: u64,
    pub write_bytes_per_sec: u64,
}

impl DiskSnapshot {
    pub fn used_pct(&self) -> f32 {
        if self.total_bytes == 0 { return 0.0; }
        (self.used_bytes as f32 / self.total_bytes as f32) * 100.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct NetworkSnapshot {
    pub interfaces: Vec<NetworkInterface>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct NetworkInterface {
    pub name: String,
    pub rx_bytes_per_sec: u64,
    pub tx_bytes_per_sec: u64,
    pub rx_total: u64,
    pub tx_total: u64,
    pub rx_history: VecDeque<u64>,
    pub tx_history: VecDeque<u64>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f32,
    pub mem_bytes: u64,
    pub status: String,
    pub user: String,
    pub threads: u32,
}


#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct TempSnapshot {
    pub label: String,
    pub celsius: f32,
    pub high: Option<f32>,   // warning threshold from hwmon
    pub crit: Option<f32>,   // critical threshold from hwmon
}

impl TempSnapshot {
    /// True if above the critical threshold, or above 90°C if no threshold known.
    pub fn is_critical(&self) -> bool {
        self.crit.map(|c| self.celsius >= c).unwrap_or(self.celsius >= 90.0)
    }
    /// True if above the high threshold, or above 75°C if no threshold known.
    pub fn is_high(&self) -> bool {
        self.high.map(|h| self.celsius >= h).unwrap_or(self.celsius >= 75.0)
    }
}

// ── Internal CPU state ───────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct CpuRaw {
    user: u64, nice: u64, system: u64, idle: u64,
    iowait: u64, irq: u64, softirq: u64, steal: u64,
}

impl CpuRaw {
    fn total(&self) -> u64 {
        self.user.saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }
    fn idle_total(&self) -> u64 { self.idle.saturating_add(self.iowait) }
    fn usage_since(&self, prev: &CpuRaw) -> f32 {
        let dt = self.total().saturating_sub(prev.total());
        if dt == 0 { return 0.0; }
        let d_idle = self.idle_total().saturating_sub(prev.idle_total());
        ((dt - d_idle) as f32 / dt as f32 * 100.0).clamp(0.0, 100.0)
    }
}

// ── Disk IO state ────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct DiskIoRaw {
    read_sectors: u64,
    write_sectors: u64,
}

// ── Collector ────────────────────────────────────────────────────────────────

pub struct Collector {
    // sysinfo System removed — processes now read from /proc directly.
    // Disks::new_with_refreshed_list() is still used for statvfs space stats.
    history_len: usize,
    prev_cpu: Vec<CpuRaw>,
    cpu_history: VecDeque<f32>,
    mem_history: VecDeque<f32>,
    cpu_brand: String,
    prev_net: HashMap<String, (u64, u64)>,
    net_history: HashMap<String, (VecDeque<u64>, VecDeque<u64>)>,
    prev_disk_io: HashMap<String, DiskIoRaw>,
    // Arc<Vec<>> — clone in collect() is a refcount bump, not a heap copy
    cached_disks: Arc<Vec<DiskSnapshot>>,
    cached_temps: Arc<Vec<TempSnapshot>>,
    tick: u64,
    disk_refresh_ticks: u64,
    last_tick_time: std::time::Instant,
    // Direct /proc process state
    prev_proc_jiffies: HashMap<u32, u64>,  // pid → utime+stime last tick
    uid_name_cache: HashMap<u32, String>,   // uid → username (loaded once)
    clock_ticks: u64,                       // USER_HZ from sysconf
}

impl Collector {
    pub fn new(history_len: usize) -> Self {
        let cpu_brand = Self::read_cpu_brand();
        // USER_HZ — typically 100 on all modern Linux. Used for /proc CPU jiffie math.
        let clock_ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        Self {
            history_len,
            prev_cpu: Vec::new(),
            cpu_history: VecDeque::with_capacity(history_len),
            mem_history: VecDeque::with_capacity(history_len),
            cpu_brand,
            prev_net: HashMap::new(),
            net_history: HashMap::new(),
            prev_disk_io: HashMap::new(),
            cached_disks: Arc::new(Vec::new()),
            cached_temps: Arc::new(Vec::new()),
            tick: 0,
            disk_refresh_ticks: 10,
            last_tick_time: std::time::Instant::now(),
            prev_proc_jiffies: HashMap::new(),
            uid_name_cache: load_uid_name_cache(),
            clock_ticks,
        }
    }

    pub fn collect(&mut self) -> Result<Snapshot> {
        self.tick = self.tick.wrapping_add(1);
        let now = std::time::Instant::now();
        let elapsed_secs = now.duration_since(self.last_tick_time).as_secs_f64().max(0.001);
        self.last_tick_time = now;

        let cpu      = self.collect_cpu()?;
        let memory   = self.collect_memory()?;
        let network  = self.collect_network(elapsed_secs)?;
        let processes = self.collect_processes(elapsed_secs);

        // Disk space refreshed every N ticks; IO rates updated every tick.
        // Arc::clone is a refcount bump — no heap allocation.
        if self.tick % self.disk_refresh_ticks == 1 || self.cached_disks.is_empty() {
            self.cached_disks = Arc::new(self.collect_disks(elapsed_secs));
        } else {
            // Clone the Vec out of the Arc, update IO rates in place, re-wrap
            let mut d = (*self.cached_disks).clone();
            let disk_io = read_proc_diskstats().unwrap_or_default();
            for disk in d.iter_mut() {
                if let (Some(cur), Some(prev)) =
                    (disk_io.get(&disk.name), self.prev_disk_io.get(&disk.name))
                {
                    let dr = cur.read_sectors.saturating_sub(prev.read_sectors);
                    let dw = cur.write_sectors.saturating_sub(prev.write_sectors);
                    disk.read_bytes_per_sec  = (dr as f64 * 512.0 / elapsed_secs) as u64;
                    disk.write_bytes_per_sec = (dw as f64 * 512.0 / elapsed_secs) as u64;
                }
            }
            self.prev_disk_io = disk_io;
            self.cached_disks = Arc::new(d);
        }
        let disks = Arc::clone(&self.cached_disks);

        debug!(
            "tick={} cpu={:.1}% mem={:.1}% procs={} disks={}",
            self.tick, cpu.total_usage, memory.used_pct(), processes.len(), disks.len()
        );

        // Temps refresh every 5 ticks — sensors change slowly.
        // Arc::clone is a refcount bump — no heap allocation.
        if self.tick % 5 == 1 || self.cached_temps.is_empty() {
            self.cached_temps = Arc::new(read_hwmon_temps());
        }
        let temps = Arc::clone(&self.cached_temps);

        Ok(Snapshot { cpu, memory, disks, network, processes, temps, timestamp: now })
    }

    // ── CPU from /proc/stat ─────────────────────────────────────────────────

    fn collect_cpu(&mut self) -> Result<CpuSnapshot> {
        let raw = read_proc_stat().context("reading /proc/stat")?;

        if self.prev_cpu.is_empty() {
            self.prev_cpu = raw;
            push_deque(&mut self.cpu_history, 0.0, self.history_len);
            return Ok(CpuSnapshot {
                total_usage: 0.0,
                core_usage: vec![0.0; self.prev_cpu.len().saturating_sub(1)],
                brand: self.cpu_brand.clone(),
                freq_mhz: read_cpu_freq_mhz(),
                history: self.cpu_history.clone(),
            });
        }

        // Guard against empty parse (transient /proc/stat read error) —
        // raw[0] would panic; instead return previous values unchanged.
        let (Some(raw_total), Some(prev_total)) = (raw.first(), self.prev_cpu.first()) else {
            tracing::warn!("Empty /proc/stat parse — skipping CPU tick");
            return Ok(CpuSnapshot {
                total_usage: self.cpu_history.back().copied().unwrap_or(0.0),
                core_usage: vec![0.0; self.prev_cpu.len().saturating_sub(1)],
                brand: self.cpu_brand.clone(),
                freq_mhz: read_cpu_freq_mhz(),
                history: self.cpu_history.clone(),
            });
        };

        let total_usage = raw_total.usage_since(prev_total);
        let core_usage: Vec<f32> = raw.iter().skip(1)
            .zip(self.prev_cpu.iter().skip(1))
            .map(|(cur, prev)| cur.usage_since(prev))
            .collect();

        push_deque(&mut self.cpu_history, total_usage, self.history_len);
        self.prev_cpu = raw;

        Ok(CpuSnapshot {
            total_usage,
            core_usage,
            brand: self.cpu_brand.clone(),
            freq_mhz: read_cpu_freq_mhz(),
            history: self.cpu_history.clone(),
        })
    }

    // ── Memory from /proc/meminfo ───────────────────────────────────────────

    fn collect_memory(&mut self) -> Result<MemSnapshot> {
        let m = read_proc_meminfo().context("reading /proc/meminfo")?;
        let used = m.total.saturating_sub(m.available);
        let pct = if m.total > 0 { used as f32 / m.total as f32 * 100.0 } else { 0.0 };
        push_deque(&mut self.mem_history, pct, self.history_len);
        Ok(MemSnapshot {
            total_bytes:    m.total,
            used_bytes:     used,
            available_bytes: m.available,
            swap_total:     m.swap_total,
            swap_used:      m.swap_total.saturating_sub(m.swap_free),
            history:        self.mem_history.clone(),
        })
    }

    // ── Disks: space via sysinfo + IO via /proc/diskstats ───────────────────

    fn collect_disks(&mut self, elapsed_secs: f64) -> Vec<DiskSnapshot> {
        let disk_io = read_proc_diskstats().unwrap_or_default();
        let sysinfo_disks = Disks::new_with_refreshed_list();
        let mut result = Vec::new();

        for d in sysinfo_disks.iter() {
            let dev_raw = d.name().to_string_lossy().into_owned();
            let io_key = dev_raw.trim_start_matches("/dev/").to_string();

            let (read_bps, write_bps) = if let (Some(cur), Some(prev)) =
                (disk_io.get(&io_key), self.prev_disk_io.get(&io_key))
            {
                let dr = cur.read_sectors.saturating_sub(prev.read_sectors);
                let dw = cur.write_sectors.saturating_sub(prev.write_sectors);
                ((dr as f64 * 512.0 / elapsed_secs) as u64,
                 (dw as f64 * 512.0 / elapsed_secs) as u64)
            } else { (0, 0) };

            result.push(DiskSnapshot {
                name:               io_key,
                mount:              d.mount_point().to_string_lossy().into_owned(),
                total_bytes:        d.total_space(),
                used_bytes:         d.total_space().saturating_sub(d.available_space()),
                is_removable:       d.is_removable(),
                read_bytes_per_sec: read_bps,
                write_bytes_per_sec: write_bps,
            });
        }
        self.prev_disk_io = disk_io;
        result
    }



    // ── Network from /proc/net/dev ──────────────────────────────────────────

    fn collect_network(&mut self, elapsed_secs: f64) -> Result<NetworkSnapshot> {
        let counters = read_proc_net_dev().context("reading /proc/net/dev")?;
        let mut interfaces = Vec::new();

        for (name, (rx_total, tx_total)) in &counters {
            let (prev_rx, prev_tx) = self.prev_net.get(name).copied().unwrap_or((0, 0));
            let rx_bps = (rx_total.saturating_sub(prev_rx) as f64 / elapsed_secs) as u64;
            let tx_bps = (tx_total.saturating_sub(prev_tx) as f64 / elapsed_secs) as u64;

            let entry = self.net_history
                .entry(name.clone())
                .or_insert_with(|| (VecDeque::new(), VecDeque::new()));
            push_deque(&mut entry.0, rx_bps, self.history_len);
            push_deque(&mut entry.1, tx_bps, self.history_len);

            if name == "lo" { continue; }  // skip loopback
            interfaces.push(NetworkInterface {
                name: name.clone(),
                rx_bytes_per_sec: rx_bps,
                tx_bytes_per_sec: tx_bps,
                rx_total: *rx_total,
                tx_total: *tx_total,
                rx_history: entry.0.clone(),
                tx_history: entry.1.clone(),
            });
        }
        self.prev_net = counters;
        Ok(NetworkSnapshot { interfaces })
    }

    // ── Processes ───────────────────────────────────────────────────────────

    // ── Processes — direct /proc reads, no sysinfo ──────────────────────
    //
    // Reads /proc/[pid]/stat for CPU jiffies and thread count.
    // Reads /proc/[pid]/status for memory (VmRSS), uid, and Tgid.
    // Tgid != Pid means this entry is a thread — skip it.
    // CPU% = delta_jiffies / (elapsed_secs * clock_ticks) / num_cores * 100

    fn collect_processes(&mut self, elapsed_secs: f64) -> Vec<ProcessSnapshot> {
        let num_cores = (self.prev_cpu.len().saturating_sub(1)).max(1) as f32;
        let elapsed_ticks = (elapsed_secs * self.clock_ticks as f64).max(1.0);
        let mut new_jiffies: HashMap<u32, u64> = HashMap::new();
        let mut procs = Vec::new();

        let dir = match fs::read_dir("/proc") { Ok(d) => d, Err(_) => return procs };

        for entry in dir.flatten() {
            let fname = entry.file_name();
            let pid: u32 = match fname.to_str().and_then(|s| s.parse().ok()) {
                Some(p) => p,
                None    => continue,
            };

            let base = format!("/proc/{}", pid);

            // /proc/[pid]/status — thread filter, memory, uid
            let status = match read_proc_pid_status(&base) {
                Some(s) => s,
                None    => continue,
            };

            // Skip threads: if Tgid != Pid this is a worker thread, not a process
            if status.tgid != pid { continue; }

            // /proc/[pid]/stat — name, CPU jiffies, state
            let stat = match read_proc_pid_stat(&base) {
                Some(s) => s,
                None    => continue,
            };

            let total_jiffies = stat.utime.saturating_add(stat.stime);
            new_jiffies.insert(pid, total_jiffies);

            // CPU% relative to all cores combined (matches system total display)
            let cpu_usage = if let Some(&prev) = self.prev_proc_jiffies.get(&pid) {
                let delta = total_jiffies.saturating_sub(prev) as f64;
                ((delta / elapsed_ticks / num_cores as f64) * 100.0)
                    .clamp(0.0, 100.0) as f32
            } else {
                0.0
            };

            let user = self.uid_name_cache
                .get(&status.uid)
                .cloned()
                .unwrap_or_else(|| status.uid.to_string());

            procs.push(ProcessSnapshot {
                pid,
                name:      stat.name,
                cpu_usage,
                mem_bytes: status.vm_rss_bytes,
                status:    stat.state.to_string(),
                user,
                threads:   status.threads,
            });
        }

        self.prev_proc_jiffies = new_jiffies;
        procs
    }

    fn read_cpu_brand() -> String {
        if let Ok(f) = fs::File::open("/proc/cpuinfo") {
            for line in io::BufReader::new(f).lines().map_while(Result::ok) {
                if line.starts_with("model name") {
                    if let Some(val) = line.splitn(2, ':').nth(1) {
                        return val.trim().to_string();
                    }
                }
            }
        }
        "Unknown CPU".to_string()
    }
}

// ── /proc readers ────────────────────────────────────────────────────────────

fn read_proc_stat() -> Result<Vec<CpuRaw>> {
    let content = fs::read_to_string("/proc/stat").context("/proc/stat")?;
    let mut result = Vec::new();
    for line in content.lines() {
        if !line.starts_with("cpu") { break; }
        let mut fields = line.split_ascii_whitespace();
        let label = fields.next().unwrap_or("");
        // Accept "cpu" (total) and "cpu0", "cpu1", etc. (per-core)
        if label != "cpu" && label.len() > 3 && !label[3..].bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let nums: Vec<u64> = fields.take(8).filter_map(|s| s.parse().ok()).collect();
        if nums.len() >= 4 {
            result.push(CpuRaw {
                user: nums[0], nice: nums[1], system: nums[2], idle: nums[3],
                iowait:   nums.get(4).copied().unwrap_or(0),
                irq:      nums.get(5).copied().unwrap_or(0),
                softirq:  nums.get(6).copied().unwrap_or(0),
                steal:    nums.get(7).copied().unwrap_or(0),
            });
        }
    }
    Ok(result)
}

struct MemInfo { total: u64, available: u64, swap_total: u64, swap_free: u64 }

fn read_proc_meminfo() -> Result<MemInfo> {
    let content = fs::read_to_string("/proc/meminfo").context("/proc/meminfo")?;
    let mut m = MemInfo { total: 0, available: 0, swap_total: 0, swap_free: 0 };
    for line in content.lines() {
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("MemTotal:")     => m.total      = it.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).saturating_mul(1024),
            Some("MemAvailable:") => m.available  = it.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).saturating_mul(1024),
            Some("SwapTotal:")    => m.swap_total  = it.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).saturating_mul(1024),
            Some("SwapFree:")     => m.swap_free   = it.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).saturating_mul(1024),
            _ => {}
        }
    }
    Ok(m)
}

fn read_proc_diskstats() -> Result<HashMap<String, DiskIoRaw>> {
    let content = fs::read_to_string("/proc/diskstats").context("/proc/diskstats")?;
    let mut map = HashMap::new();
    for line in content.lines() {
        let f: Vec<&str> = line.split_ascii_whitespace().collect();
        if f.len() < 10 { continue; }
        let name = f[2].to_string();
        let read_sectors:  u64 = f[5].parse().unwrap_or(0);
        let write_sectors: u64 = f[9].parse().unwrap_or(0);
        map.insert(name, DiskIoRaw { read_sectors, write_sectors });
    }
    Ok(map)
}

fn read_proc_net_dev() -> Result<HashMap<String, (u64, u64)>> {
    let content = fs::read_to_string("/proc/net/dev").context("/proc/net/dev")?;
    let mut map = HashMap::new();
    for line in content.lines().skip(2) {
        let line = line.trim();
        let colon = match line.find(':') { Some(i) => i, None => continue };
        let name = line[..colon].trim().to_string();
        let fields: Vec<u64> = line[colon+1..]
            .split_ascii_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if fields.len() >= 9 {
            map.insert(name, (fields[0], fields[8]));
        }
    }
    Ok(map)
}

fn read_cpu_freq_mhz() -> Option<u64> {
    let khz: u64 = fs::read_to_string(
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"
    ).ok()?.trim().parse().ok()?;
    Some(khz / 1000)
}

fn push_deque<T>(v: &mut VecDeque<T>, val: T, max: usize) {
    if v.len() >= max { v.pop_front(); }
    v.push_back(val);
}
/// Map a raw hwmon device name + sensor label to a clear, human-readable string.
///
/// hwmon names come from the kernel driver and are often cryptic lowercase
/// identifiers. This function produces labels that describe what the sensor
/// actually is (CPU die, GPU, WiFi chip, SSD, etc.).
/// Map a raw hwmon driver name to a short plain-English label.
/// One label per physical component — duplicates are collapsed in read_hwmon_temps.
fn device_short_label(device: &str) -> &'static str {
    let dev = device.trim();
    // Match on common driver names (case-insensitive prefix/contains)
    let d = &dev.to_ascii_lowercase();
    if d.starts_with("coretemp")
        || d.starts_with("k10temp")
        || d.starts_with("zenpower")  { return "CPU"; }
    if d.starts_with("amdgpu")        { return "GPU"; }
    if d.starts_with("nouveau")
        || d.starts_with("nvidia")    { return "GPU"; }
    if d.starts_with("nvme")          { return "SSD"; }
    if d.starts_with("drivetemp")     { return "HDD"; }
    if d.starts_with("acpitz")        { return "System"; }
    if d.starts_with("thinkpad")      { return "ThinkPad"; }
    if d.contains("mt7921") || d.contains("mt7922")
        || d.contains("iwlwifi")
        || d.contains("phy")          { return "WiFi"; }
    if d.starts_with("it8") || d.starts_with("w83")
        || d.starts_with("nct")
        || d.starts_with("f718")      { return "Mobo"; }
    "Sensor"
}

/// Read all temperature sensors from /sys/class/hwmon.
///
/// Each hwmon device exposes tempN_input (millidegrees C), tempN_label (name),
/// tempN_max (high threshold), tempN_crit (critical threshold).
/// We enumerate hwmon0..hwmon9 and up to 16 sensors each — cheap sysfs reads,
/// no spawning, no lm-sensors dependency.
pub fn read_hwmon_temps() -> Vec<TempSnapshot> {
    // We collect one entry per hwmon device — the hottest sensor on that device.
    // This prevents devices like the ThinkPad EC (which exposes 5+ sensors all
    // at the same temp) from flooding the panel with duplicate rows.
    use std::collections::HashMap;

    // label → best (hottest) TempSnapshot seen so far
    let mut best: HashMap<String, TempSnapshot> = HashMap::new();

    for hwmon_idx in 0..16u8 {
        let base = format!("/sys/class/hwmon/hwmon{}", hwmon_idx);
        if !std::path::Path::new(&base).exists() { continue; }

        let device_name = fs::read_to_string(format!("{}/name", base))
            .unwrap_or_default();
        let device_name = device_name.trim().to_string();

        // Short plain label for this device, e.g. "CPU", "SSD", "WiFi"
        let label = device_short_label(&device_name).to_string();

        for sensor_idx in 1..=24u8 {
            let input_path = format!("{}/temp{}_input", base, sensor_idx);
            let raw = match fs::read_to_string(&input_path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let millideg: i64 = match raw.trim().parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let celsius = millideg as f32 / 1000.0;
            if celsius < -40.0 || celsius > 150.0 { continue; }

            // Cap thresholds at 150°C — some drivers report sentinel values
            let high = fs::read_to_string(format!("{}/temp{}_max", base, sensor_idx))
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .map(|v| v as f32 / 1000.0)
                .filter(|&v| v > 0.0 && v <= 150.0);

            let crit = fs::read_to_string(format!("{}/temp{}_crit", base, sensor_idx))
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .map(|v| v as f32 / 1000.0)
                .filter(|&v| v > 0.0 && v <= 150.0);

            let candidate = TempSnapshot { label: label.clone(), celsius, high, crit };

            // Keep only the hottest reading per label
            best.entry(label.clone())
                .and_modify(|existing| {
                    if celsius > existing.celsius { *existing = candidate.clone(); }
                })
                .or_insert(candidate);
        }
    }

    let mut temps: Vec<TempSnapshot> = best.into_values().collect();

    // Sort: critical first, then hottest first
    temps.sort_by(|a, b| {
        b.is_critical().cmp(&a.is_critical())
            .then(b.celsius.partial_cmp(&a.celsius).unwrap_or(std::cmp::Ordering::Equal))
    });
    temps
}

// ── /proc per-process helpers ─────────────────────────────────────────────────

struct ProcStatFields {
    name:  String,
    state: char,
    utime: u64,
    stime: u64,
}

struct ProcStatusFields {
    tgid:         u32,
    uid:          u32,
    vm_rss_bytes: u64,
    threads:      u32,
}

/// Parse /proc/[pid]/stat.
///
/// The comm field (name) is wrapped in parens and may contain spaces and
/// parens itself. We find the first '(' and last ')' to extract it safely,
/// then parse the fixed-position fields from the remainder.
fn read_proc_pid_stat(base: &str) -> Option<ProcStatFields> {
    let content = fs::read_to_string(format!("{}/stat", base)).ok()?;
    let open  = content.find('(')?;
    let close = content.rfind(')')?;
    let name  = content[open + 1..close].to_string();

    // Fields after the closing paren (space-separated):
    // [0]=state [1]=ppid [2]=pgrp ... [11]=utime [12]=stime ... [17]=num_threads
    let rest: Vec<&str> = content[close + 2..].split_ascii_whitespace().collect();
    let state = rest.first()?.chars().next().unwrap_or('?');
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;

    Some(ProcStatFields { name, state, utime, stime })
}

/// Parse /proc/[pid]/status for the fields we need.
///
/// We read Tgid (thread-group id, equals Pid for main threads), the real Uid,
/// VmRSS (resident memory in kB), and Threads.
fn read_proc_pid_status(base: &str) -> Option<ProcStatusFields> {
    let content = fs::read_to_string(format!("{}/status", base)).ok()?;
    let mut tgid: Option<u32>   = None;
    let mut uid:  Option<u32>   = None;
    let mut rss:  Option<u64>   = None;
    let mut threads: Option<u32> = None;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Tgid:") {
            tgid = rest.split_whitespace().next()?.parse().ok();
        } else if let Some(rest) = line.strip_prefix("Uid:") {
            // Uid: real  effective  saved  fs  — we want real (first)
            uid = rest.split_whitespace().next()?.parse().ok();
        } else if let Some(rest) = line.strip_prefix("VmRSS:") {
            // Value is in kB
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            rss = Some(kb.saturating_mul(1024));
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            threads = rest.split_whitespace().next()?.parse().ok();
        }
        // Short-circuit once we have everything
        if tgid.is_some() && uid.is_some() && rss.is_some() && threads.is_some() { break; }
    }

    Some(ProcStatusFields {
        tgid:         tgid?,
        uid:          uid?,
        vm_rss_bytes: rss.unwrap_or(0),
        threads:      threads.unwrap_or(1),
    })
}

/// Build a uid→username map from /etc/passwd, loaded once at startup.
/// Falls back to an empty map if the file is unreadable (uid shown as number).
fn load_uid_name_cache() -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let content = match fs::read_to_string("/etc/passwd") {
        Ok(c)  => c,
        Err(_) => return map,
    };
    for line in content.lines() {
        // Format: username:password:uid:gid:...
        let mut fields = line.splitn(4, ':');
        if let (Some(name), Some(_), Some(uid_str)) =
            (fields.next(), fields.next(), fields.next())
        {
            if let Ok(uid) = uid_str.parse::<u32>() {
                map.insert(uid, name.to_string());
            }
        }
    }
    map
}
