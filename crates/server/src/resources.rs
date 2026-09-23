//! Host and per-session CPU and memory, for the status bar.
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
use sysinfo::{Pid, ProcessesToUpdate, System};

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
}

fn sampler() -> &'static Mutex<Sampler> {
    static SAMPLER: OnceLock<Mutex<Sampler>> = OnceLock::new();
    SAMPLER.get_or_init(|| {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Mutex::new(Sampler {
            system,
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

async fn usage(State(state): State<AppState>) -> Result<Json<HostUsage>> {
    let mut roots: Vec<(String, u32)> = state
        .chats
        .process_ids()
        .into_iter()
        .map(|(id, pid)| (id.to_string(), pid))
        .collect();
    roots.extend(state.runtime.process_ids());
    let usage = tokio::task::spawn_blocking(move || sample(roots))
        .await
        .map_err(crate::ApiError::internal)?;
    Ok(Json(usage))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/host/resources", get(usage))
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
