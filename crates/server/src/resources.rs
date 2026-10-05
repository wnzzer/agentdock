//! Host and per-session CPU and memory, for the status bar, and the fuller
//! picture of the machine (disks, network, load, temperatures) for the
//! system page.
//!
//! Read-only and cheap: one sampler shared by every request, refreshed at most
//! once a second, so several open browsers polling it do not multiply the
//! work. CPU is a rate, so the first reading after start is only a baseline.
use crate::{AppState, Result};
use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use sysinfo::{Components, DiskKind, Disks, Networks, Pid, ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize)]
pub struct SessionUsage {
    pub session_id: String,
    /// Percent of one core, summed over the session's process tree.
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostUsage {
    /// Percent of the whole machine, 0–100.
    pub cpu_percent: f32,
    pub cpu_count: usize,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub sessions: Vec<SessionUsage>,
}

struct Sampler {
    system: System,
    sampled: Option<(Instant, HostUsage)>,
    disks: Disks,
    networks: Networks,
    components: Components,
    /// When the network counters were last read, to turn bytes into a rate.
    networks_at: Instant,
    detail: Option<(Instant, SystemDetail)>,
}

fn sampler() -> &'static Mutex<Sampler> {
    static SAMPLER: OnceLock<Mutex<Sampler>> = OnceLock::new();
    SAMPLER.get_or_init(|| {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Mutex::new(Sampler {
            system,
            sampled: None,
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            networks_at: Instant::now(),
            detail: None,
        })
    })
}

/// A process and everything started beneath it, which is where a session's
/// native client and its tools actually run.
fn tree_usage(system: &System, root: u32) -> (f32, u64) {
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let (mut cpu, mut memory) = (0.0f32, 0u64);
    let mut stack = vec![Pid::from_u32(root)];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(process) = system.process(pid) {
            cpu += process.cpu_usage();
            memory += process.memory();
        }
        if let Some(next) = children.get(&pid) {
            stack.extend(next.iter().copied());
        }
    }
    (cpu, memory)
}

pub fn sample(roots: Vec<(String, u32)>) -> HostUsage {
    let mut guard = sampler().lock().expect("resource sampler");
    if let Some((at, usage)) = &guard.sampled
        && at.elapsed() < Duration::from_secs(1)
    {
        return usage.clone();
    }
    let system = &mut guard.system;
    system.refresh_cpu_usage();
    system.refresh_memory();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let sessions = roots
        .into_iter()
        .map(|(session_id, pid)| {
            let (cpu_percent, memory_bytes) = tree_usage(system, pid);
            SessionUsage {
                session_id,
                cpu_percent,
                memory_bytes,
            }
        })
        .collect();
    let usage = HostUsage {
        cpu_percent: system.global_cpu_usage(),
        cpu_count: system.cpus().len(),
        memory_used_bytes: system.used_memory(),
        memory_total_bytes: system.total_memory(),
        sessions,
    };
    guard.sampled = Some((Instant::now(), usage.clone()));
    usage
}

#[derive(Debug, Clone, Serialize)]
pub struct HostInfo {
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub arch: String,
    pub uptime_seconds: u64,
    pub agentdock_version: &'static str,
    pub agentdock_uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuDetail {
    pub percent: f32,
    pub count: usize,
    pub brand: Option<String>,
    pub frequency_mhz: Option<u64>,
    /// Each logical core, 0–100.
    pub cores: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryDetail {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskDetail {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub kind: &'static str,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub removable: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkDetail {
    pub name: String,
    /// Bytes per second since the previous reading.
    pub received_per_second: u64,
    pub transmitted_per_second: u64,
    pub total_received_bytes: u64,
    pub total_transmitted_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Temperature {
    pub label: String,
    pub celsius: f32,
    pub critical_celsius: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessDetail {
    pub pid: u32,
    pub name: String,
    /// Percent of one core.
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemDetail {
    pub host: HostInfo,
    /// Absent where the platform has no load average (Windows).
    pub load: Option<LoadAverage>,
    pub cpu: CpuDetail,
    pub memory: MemoryDetail,
    pub disks: Vec<DiskDetail>,
    pub networks: Vec<NetworkDetail>,
    pub temperatures: Vec<Temperature>,
    /// The busiest processes on the machine, by CPU then memory.
    pub processes: Vec<ProcessDetail>,
    pub sessions: Vec<SessionUsage>,
}

/// How long this server process has run, from the operating system's record
/// of when it started rather than from the first request.
fn agentdock_uptime(system: &System) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    system
        .process(Pid::from_u32(std::process::id()))
        .map(|process| now.saturating_sub(process.start_time()))
        .unwrap_or_default()
}

/// Filesystems that are views of something else, or of nothing at all: they
/// would list the same space several times, or space that is not storage.
fn real_disk(disk: &sysinfo::Disk) -> bool {
    let fs = disk.file_system().to_string_lossy().to_ascii_lowercase();
    let mount = disk.mount_point().to_string_lossy();
    disk.total_space() > 0
        && !matches!(
            fs.as_str(),
            "overlay" | "squashfs" | "tmpfs" | "devtmpfs" | "ramfs" | "autofs" | "nsfs"
        )
        && !mount.starts_with("/snap/")
        && !mount.starts_with("/var/lib/docker/")
        && !mount.starts_with("/run/")
        && !mount.starts_with("/boot/efi")
}

pub fn detail(roots: Vec<(String, u32)>) -> SystemDetail {
    let sessions = sample(roots).sessions;
    let mut guard = sampler().lock().expect("resource sampler");
    if let Some((at, detail)) = &guard.detail
        && at.elapsed() < Duration::from_secs(1)
    {
        let mut detail = detail.clone();
        detail.sessions = sessions;
        return detail;
    }
    let guard = &mut *guard;
    let system = &guard.system;
    guard.disks.refresh(true);
    guard.components.refresh(true);
    let elapsed = guard.networks_at.elapsed().as_secs_f64().max(0.001);
    guard.networks.refresh(true);
    guard.networks_at = Instant::now();

    let mut seen = std::collections::HashSet::new();
    let mut disks: Vec<DiskDetail> = guard
        .disks
        .list()
        .iter()
        .filter(|disk| real_disk(disk))
        // One device mounted in several places (bind mounts) is one disk.
        .filter(|disk| seen.insert(disk.name().to_os_string()))
        .map(|disk| DiskDetail {
            name: disk.name().to_string_lossy().into_owned(),
            mount_point: disk.mount_point().to_string_lossy().into_owned(),
            file_system: disk.file_system().to_string_lossy().into_owned(),
            kind: match disk.kind() {
                DiskKind::SSD => "ssd",
                DiskKind::HDD => "hdd",
                _ => "unknown",
            },
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
            removable: disk.is_removable(),
            read_only: disk.is_read_only(),
        })
        .collect();
    disks.sort_by(|a, b| {
        a.mount_point
            .len()
            .cmp(&b.mount_point.len())
            .then(a.mount_point.cmp(&b.mount_point))
    });

    let mut networks: Vec<NetworkDetail> = guard
        .networks
        .iter()
        .filter(|(name, data)| {
            name.as_str() != "lo"
                && !name.starts_with("veth")
                && (data.total_received() > 0 || data.total_transmitted() > 0)
        })
        .map(|(name, data)| NetworkDetail {
            name: name.clone(),
            received_per_second: (data.received() as f64 / elapsed) as u64,
            transmitted_per_second: (data.transmitted() as f64 / elapsed) as u64,
            total_received_bytes: data.total_received(),
            total_transmitted_bytes: data.total_transmitted(),
        })
        .collect();
    networks.sort_by_key(|network| {
        std::cmp::Reverse(network.total_received_bytes + network.total_transmitted_bytes)
    });

    let mut temperatures: Vec<Temperature> = guard
        .components
        .list()
        .iter()
        .filter_map(|component| {
            let celsius = component.temperature()?;
            (celsius.is_finite() && celsius > 0.0).then(|| Temperature {
                label: component.label().to_string(),
                celsius,
                critical_celsius: component.critical(),
            })
        })
        .collect();
    temperatures.sort_by(|a, b| b.celsius.total_cmp(&a.celsius));
    temperatures.truncate(8);

    let mut processes: Vec<ProcessDetail> = system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessDetail {
            pid: pid.as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            cpu_percent: process.cpu_usage(),
            memory_bytes: process.memory(),
        })
        .collect();
    processes.sort_by(|a, b| {
        b.cpu_percent
            .total_cmp(&a.cpu_percent)
            .then(b.memory_bytes.cmp(&a.memory_bytes))
    });
    processes.truncate(8);

    let cpus = system.cpus();
    let load = System::load_average();
    let detail = SystemDetail {
        host: HostInfo {
            hostname: System::host_name(),
            os: System::long_os_version(),
            kernel: System::kernel_version(),
            arch: System::cpu_arch(),
            uptime_seconds: System::uptime(),
            agentdock_version: env!("CARGO_PKG_VERSION"),
            agentdock_uptime_seconds: agentdock_uptime(system),
        },
        load: (!cfg!(windows)).then_some(LoadAverage {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        }),
        cpu: CpuDetail {
            percent: system.global_cpu_usage(),
            count: cpus.len(),
            brand: cpus
                .first()
                .map(|cpu| cpu.brand().trim().to_string())
                .filter(|brand| !brand.is_empty()),
            frequency_mhz: cpus
                .first()
                .map(|cpu| cpu.frequency())
                .filter(|mhz| *mhz > 0),
            cores: cpus.iter().map(|cpu| cpu.cpu_usage()).collect(),
        },
        memory: MemoryDetail {
            used_bytes: system.used_memory(),
            total_bytes: system.total_memory(),
            available_bytes: system.available_memory(),
            swap_used_bytes: system.used_swap(),
            swap_total_bytes: system.total_swap(),
        },
        disks,
        networks,
        temperatures,
        processes,
        sessions,
    };
    guard.detail = Some((Instant::now(), detail.clone()));
    detail
}

fn session_roots(state: &AppState) -> Vec<(String, u32)> {
    let mut roots: Vec<(String, u32)> = state
        .chats
        .process_ids()
        .into_iter()
        .map(|(id, pid)| (id.to_string(), pid))
        .collect();
    roots.extend(state.runtime.process_ids());
    roots
}

async fn system(State(state): State<AppState>) -> Result<Json<SystemDetail>> {
    let roots = session_roots(&state);
    let detail = tokio::task::spawn_blocking(move || detail(roots))
        .await
        .map_err(crate::ApiError::internal)?;
    Ok(Json(detail))
}

async fn usage(State(state): State<AppState>) -> Result<Json<HostUsage>> {
    let roots = session_roots(&state);
    let usage = tokio::task::spawn_blocking(move || sample(roots))
        .await
        .map_err(crate::ApiError::internal)?;
    Ok(Json(usage))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/host/resources", get(usage))
        .route("/api/host/system", get(system))
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_process_is_measured_and_the_machine_has_memory() {
        let usage = super::sample(vec![("self".into(), std::process::id())]);
        assert!(usage.memory_total_bytes > 0);
        assert!(usage.memory_used_bytes <= usage.memory_total_bytes);
        assert!(usage.cpu_count > 0);
        assert_eq!(usage.sessions.len(), 1);
        assert!(
            usage.sessions[0].memory_bytes > 0,
            "the test process uses memory"
        );
    }

    #[test]
    fn the_system_page_describes_the_machine() {
        let detail = super::detail(vec![("self".into(), std::process::id())]);
        assert!(detail.cpu.count > 0);
        assert_eq!(detail.cpu.cores.len(), detail.cpu.count);
        assert!(detail.memory.total_bytes > 0);
        assert!(!detail.host.arch.is_empty());
        assert!(!detail.processes.is_empty(), "the test process at least");
        assert_eq!(detail.sessions.len(), 1);
        for disk in &detail.disks {
            assert!(disk.available_bytes <= disk.total_bytes, "{disk:?}");
        }
        if !cfg!(windows) {
            assert!(detail.load.is_some());
        }
    }
}
