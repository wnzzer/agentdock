//! Host and per-session CPU and memory for the status bar, and the fuller
//! picture of the machine (disks, network, load, temperatures, fans and CPU
//! clocks where they can be read) for the system page.
//!
//! Read-only and cheap: one sampler shared by every request, refreshed at most
//! once a second, so several open browsers polling it do not multiply the
//! work. CPU is a rate, so the first reading after start is only a baseline.
//!
//! A background recorder also samples on its own clock, whether or not a
//! browser is open, and keeps the history in a separate metrics database.
use crate::{
    ApiError, AppState, Result, preferences,
    sensors::{self, ChipSensors, CpuFrequency, Fan, PowerPart, Temperature},
};
use agentdock_persistence::metrics::{self, HostPoint, MetricsStore, SessionPoint};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
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
    /// Empty where the platform does not report them.
    pub cpu_frequencies: Vec<CpuFrequency>,
    /// The whole machine's draw, and the parts of it the chip counts; absent
    /// and empty where the platform does not say.
    pub power_watts: Option<f32>,
    pub power_parts: Vec<PowerPart>,
    pub fans: Vec<Fan>,
    pub temperatures: Vec<Temperature>,
    pub sessions: Vec<SessionUsage>,
}

struct Sampler {
    system: System,
    components: Components,
    chip: ChipSensors,
    sampled: Option<(Instant, HostUsage)>,
    disks: Disks,
    networks: Networks,
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
            components: Components::new_with_refreshed_list(),
            chip: ChipSensors::new(),
            sampled: None,
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
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

fn session_usage(system: &System, roots: Vec<(String, u32)>) -> Vec<SessionUsage> {
    roots
        .into_iter()
        .map(|(session_id, pid)| {
            let (cpu_percent, memory_bytes) = tree_usage(system, pid);
            SessionUsage {
                session_id,
                cpu_percent,
                memory_bytes,
            }
        })
        .collect()
}

pub fn sample(roots: Vec<(String, u32)>) -> HostUsage {
    let mut guard = sampler().lock().expect("resource sampler");
    // The machine is reused within a second, but the sessions are always the
    // caller's: two callers asking about different sessions in the same
    // second each get their own, counted from the process table already read.
    if let Some((at, usage)) = &guard.sampled
        && at.elapsed() < Duration::from_secs(1)
    {
        let mut usage = usage.clone();
        usage.sessions = session_usage(&guard.system, roots);
        return usage;
    }
    let Sampler {
        system,
        components,
        chip,
        ..
    } = &mut *guard;
    system.refresh_cpu_usage();
    system.refresh_memory();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let sessions = session_usage(system, roots);
    let reading = chip.sample(system);
    let usage = HostUsage {
        cpu_percent: system.global_cpu_usage(),
        cpu_count: system.cpus().len(),
        memory_used_bytes: system.used_memory(),
        memory_total_bytes: system.total_memory(),
        cpu_frequencies: reading.clocks,
        power_watts: reading.power_watts,
        power_parts: reading.power_parts,
        fans: sensors::fans(),
        temperatures: sensors::temperatures(components),
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
    /// The hottest sensors first.
    pub temperatures: Vec<Temperature>,
    pub fans: Vec<Fan>,
    pub cpu_frequencies: Vec<CpuFrequency>,
    pub power_watts: Option<f32>,
    pub power_parts: Vec<PowerPart>,
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

/// Temperatures, fans, clocks and sessions come from the status bar's sample,
/// taken first, so the hardware is read once however both are asked for.
pub fn detail(roots: Vec<(String, u32)>) -> SystemDetail {
    let usage = sample(roots);
    let sessions = usage.sessions;
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

    let mut temperatures = usage.temperatures;
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
        fans: usage.fans,
        cpu_frequencies: usage.cpu_frequencies,
        power_watts: usage.power_watts,
        power_parts: usage.power_parts,
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

/// Turned off, nothing is read from the machine: not by the recorder, and
/// not for the status bar or the system page either.
async fn ensure_monitoring(state: &AppState) -> Result<()> {
    if preferences::load(state).await?.resource_monitoring() {
        Ok(())
    } else {
        Err(ApiError::conflict("Resource monitoring is turned off"))
    }
}

async fn system(State(state): State<AppState>) -> Result<Json<SystemDetail>> {
    ensure_monitoring(&state).await?;
    let roots = session_roots(&state);
    let detail = tokio::task::spawn_blocking(move || detail(roots))
        .await
        .map_err(crate::ApiError::internal)?;
    Ok(Json(detail))
}

async fn usage(State(state): State<AppState>) -> Result<Json<HostUsage>> {
    ensure_monitoring(&state).await?;
    let roots = session_roots(&state);
    let usage = tokio::task::spawn_blocking(move || sample(roots))
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(usage))
}

/// Raw samples are buffered and written once a minute; the rollups run on the
/// first write and every fifth after, always straight after one, so no sample
/// still in the buffer belongs to a bucket being rolled up. The first write
/// catches up on whatever a short earlier run left unrolled.
const FLUSH_EVERY: u32 = 12;
const COMPACT_EVERY: u32 = 5;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Sample every [`metrics::RAW`] seconds into `store` for as long as the
/// server runs. Failures are logged and the recorder carries on: losing a
/// minute of charts is not worth stopping anything for.
pub fn spawn_recorder(state: AppState, store: Arc<MetricsStore>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(metrics::RAW as u64));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let (mut host, mut sessions) = (Vec::new(), Vec::new());
        let (mut samples, mut flushes) = (0u32, 0u32);
        loop {
            tick.tick().await;
            let roots = session_roots(&state);
            let preferences = state.clone();
            // Asked every tick, so turning monitoring off stops the sampling
            // at once, and what was buffered is dropped rather than written.
            let Ok(usage) = tokio::task::spawn_blocking(move || {
                preferences::load_blocking(&preferences)
                    .resource_monitoring()
                    .then(|| sample(roots))
            })
            .await
            else {
                continue;
            };
            let Some(usage) = usage else {
                host.clear();
                sessions.clear();
                samples = 0;
                continue;
            };
            let ts = now();
            let rpm = usage.fans.iter().map(|fan| fan.rpm).max();
            let celsius = usage
                .temperatures
                .iter()
                .map(|t| t.celsius)
                .reduce(f32::max);
            let mhz = usage.cpu_frequencies.iter().filter_map(|c| c.mhz).max();
            let watts = usage.power_watts;
            host.push(HostPoint {
                ts,
                cpu_avg: usage.cpu_percent,
                cpu_max: usage.cpu_percent,
                memory_used_avg: usage.memory_used_bytes,
                memory_used_max: usage.memory_used_bytes,
                memory_total: usage.memory_total_bytes,
                fan_rpm_avg: rpm,
                fan_rpm_max: rpm,
                temperature_avg: celsius,
                temperature_max: celsius,
                cpu_mhz_avg: mhz,
                cpu_mhz_max: mhz,
                power_avg: watts,
                power_max: watts,
            });
            sessions.extend(usage.sessions.into_iter().map(|session| SessionPoint {
                ts,
                session_id: session.session_id,
                cpu_avg: session.cpu_percent,
                cpu_max: session.cpu_percent,
                memory_avg: session.memory_bytes,
                memory_max: session.memory_bytes,
            }));
            samples += 1;
            if samples < FLUSH_EVERY {
                continue;
            }
            samples = 0;
            flushes += 1;
            let compact = flushes % COMPACT_EVERY == 1;
            let (batch_host, batch_sessions) =
                (std::mem::take(&mut host), std::mem::take(&mut sessions));
            let store = store.clone();
            let written = tokio::task::spawn_blocking(move || {
                store.record(&batch_host, &batch_sessions)?;
                if compact {
                    store.compact(now())?;
                }
                Ok::<_, agentdock_persistence::DbError>(())
            })
            .await;
            match written {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "could not record resource history"),
                Err(error) => tracing::warn!(%error, "resource history writer failed"),
            }
        }
    });
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    /// Unix seconds; defaults to an hour before `to`.
    from: Option<i64>,
    /// Unix seconds; defaults to now.
    to: Option<i64>,
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct HostHistoryPoint {
    ts: i64,
    cpu_avg: f32,
    cpu_max: f32,
    memory_used_avg: u64,
    memory_used_max: u64,
    memory_total: u64,
    /// The fastest fan and the hottest sensor; null where never read.
    fan_rpm_avg: Option<u32>,
    fan_rpm_max: Option<u32>,
    temperature_avg: Option<f32>,
    temperature_max: Option<f32>,
    /// The fastest CPU cluster's clock; null where never read.
    cpu_mhz_avg: Option<u32>,
    cpu_mhz_max: Option<u32>,
    /// The whole machine's draw in watts; null where never read.
    power_avg: Option<f32>,
    power_max: Option<f32>,
}

#[derive(Debug, Serialize)]
struct SessionHistoryPoint {
    ts: i64,
    session_id: String,
    cpu_avg: f32,
    cpu_max: f32,
    memory_avg: u64,
    memory_max: u64,
}

#[derive(Debug, Serialize)]
struct History {
    /// Whether monitoring is on and new samples are being recorded; what is
    /// stored stays readable either way.
    monitoring: bool,
    /// Seconds each point covers.
    resolution: i64,
    host: Vec<HostHistoryPoint>,
    sessions: Vec<SessionHistoryPoint>,
}

/// Enough points for a chart across a wide screen without shipping every raw
/// sample of a long range.
const MAX_POINTS: i64 = 1_500;

async fn history(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<History>> {
    let store = state
        .metrics
        .clone()
        .ok_or_else(|| ApiError::missing("Resource history"))?;
    let now = now();
    let to = query.to.unwrap_or(now);
    let from = query.from.unwrap_or(to - 3_600);
    if from > to {
        return Err(ApiError::bad("from must not be after to"));
    }
    let resolution = metrics::resolution_for(now, from, to, MAX_POINTS);
    let (host, sessions) = tokio::task::spawn_blocking(move || {
        Ok::<_, agentdock_persistence::DbError>((
            store.host_history(resolution, from, to)?,
            store.session_history(resolution, from, to, query.session_id.as_deref())?,
        ))
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    let monitoring = preferences::load(&state).await?.resource_monitoring();
    Ok(Json(History {
        monitoring,
        resolution,
        host: host
            .into_iter()
            .map(|p| HostHistoryPoint {
                ts: p.ts,
                cpu_avg: p.cpu_avg,
                cpu_max: p.cpu_max,
                memory_used_avg: p.memory_used_avg,
                memory_used_max: p.memory_used_max,
                memory_total: p.memory_total,
                fan_rpm_avg: p.fan_rpm_avg,
                fan_rpm_max: p.fan_rpm_max,
                temperature_avg: p.temperature_avg,
                temperature_max: p.temperature_max,
                cpu_mhz_avg: p.cpu_mhz_avg,
                cpu_mhz_max: p.cpu_mhz_max,
                power_avg: p.power_avg,
                power_max: p.power_max,
            })
            .collect(),
        sessions: sessions
            .into_iter()
            .map(|p| SessionHistoryPoint {
                ts: p.ts,
                session_id: p.session_id,
                cpu_avg: p.cpu_avg,
                cpu_max: p.cpu_max,
                memory_avg: p.memory_avg,
                memory_max: p.memory_max,
            })
            .collect(),
    }))
}

async fn clear(State(state): State<AppState>) -> Result<StatusCode> {
    let store = state
        .metrics
        .clone()
        .ok_or_else(|| ApiError::missing("Resource history"))?;
    tokio::task::spawn_blocking(move || store.clear())
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/host/resources", get(usage))
        .route("/api/host/system", get(system))
        .route("/api/host/resources/history", get(history).delete(clear))
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
