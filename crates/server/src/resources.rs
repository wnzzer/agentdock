//! Host and per-session CPU and memory, plus the host's fans and
//! temperatures where they can be read, for the status bar.
//!
//! Read-only and cheap: one sampler shared by every request, refreshed at most
//! once a second, so several open browsers polling it do not multiply the
//! work. CPU is a rate, so the first reading after start is only a baseline.
//!
//! A background recorder also samples on its own clock, whether or not a
//! browser is open, and keeps the history in a separate metrics database.
use crate::{
    ApiError, AppState, Result,
    sensors::{self, Fan, Temperature},
};
use agentdock_persistence::metrics::{self, HostPoint, MetricsStore, SessionPoint};
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use sysinfo::{Components, Pid, ProcessesToUpdate, System};

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
    pub fans: Vec<Fan>,
    pub temperatures: Vec<Temperature>,
    pub sessions: Vec<SessionUsage>,
}

struct Sampler {
    system: System,
    components: Components,
    sampled: Option<(Instant, HostUsage)>,
}

fn sampler() -> &'static Mutex<Sampler> {
    static SAMPLER: OnceLock<Mutex<Sampler>> = OnceLock::new();
    SAMPLER.get_or_init(|| {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Mutex::new(Sampler {
            system,
            components: Components::new_with_refreshed_list(),
            sampled: None,
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
    let Sampler {
        system, components, ..
    } = &mut *guard;
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
        fans: sensors::fans(),
        temperatures: sensors::temperatures(components),
        sessions,
    };
    guard.sampled = Some((Instant::now(), usage.clone()));
    usage
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

async fn usage(State(state): State<AppState>) -> Result<Json<HostUsage>> {
    let roots = session_roots(&state);
    let usage = tokio::task::spawn_blocking(move || sample(roots))
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(usage))
}

/// Raw samples are buffered and written once a minute; the rollups run every
/// fifth write, always straight after one, so no sample still in the buffer
/// belongs to a bucket being rolled up.
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
            let Ok(usage) = tokio::task::spawn_blocking(move || sample(roots)).await else {
                continue;
            };
            let ts = now();
            let rpm = usage.fans.iter().map(|fan| fan.rpm).max();
            let celsius = usage
                .temperatures
                .iter()
                .map(|t| t.celsius)
                .reduce(f32::max);
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
            let compact = flushes % COMPACT_EVERY == 0;
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
    Ok(Json(History {
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

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/host/resources", get(usage))
        .route("/api/host/resources/history", get(history))
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
}
