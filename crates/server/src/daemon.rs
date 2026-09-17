//! Running the server as a background gateway.
//!
//! The foreground `serve` is still the only thing that listens; these commands
//! put it behind a process you do not have to keep a terminal open for, which
//! is what `nohup agentdock &` was standing in for. The daemon is the same
//! executable re-executed with `serve`, so there is one code path that serves
//! and no second implementation to keep in step.
//!
//! State lives beside the database: a pid file to find the process again and a
//! log file to read what it said. The port itself remains the real mutual
//! exclusion — `prepare_server` fails to bind when one is already running — so
//! a stale pid file can never let two servers share a state directory.
use std::{
    env, fs,
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn pid_file(state_dir: &Path) -> PathBuf {
    state_dir.join("agentdock.pid")
}
pub fn log_file(state_dir: &Path) -> PathBuf {
    state_dir.join("agentdock.log")
}

/// The pid recorded for this state directory, if that process is still alive.
///
/// A pid file outlives a crash, and pids are reused, so the number alone proves
/// nothing. `kill(pid, 0)` asks the kernel whether the process exists; it is
/// still a guess about identity, which is why the port remains what actually
/// prevents a second server.
pub fn running(state_dir: &Path) -> Option<i32> {
    let raw = fs::read_to_string(pid_file(state_dir)).ok()?;
    let pid: i32 = raw.trim().parse().ok()?;
    if pid <= 1 {
        return None;
    }
    // SAFETY: signal 0 performs the permission and existence checks only.
    (unsafe { libc::kill(pid, 0) } == 0).then_some(pid)
}

/// Re-execute this binary as a detached `serve`.
///
/// `setsid` is the part that matters: without a new session the child keeps the
/// terminal as its controlling terminal, and closing that terminal delivers
/// SIGHUP to everything in the session, taking the gateway and every agent it
/// is supervising with it.
pub fn start(state_dir: &Path, address: SocketAddr) -> Result<i32, Box<dyn std::error::Error>> {
    if let Some(pid) = running(state_dir) {
        return Err(format!("AgentDock is already running (pid {pid})").into());
    }
    let program = env::current_exe()?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file(state_dir))?;
    let errors = log.try_clone()?;
    let mut command = Command::new(program);
    command
        .arg("serve")
        .env("AGENTDOCK_ADDR", address.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and is the documented way to
        // detach a child from the controlling terminal between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
        };
    }
    let child = command.spawn()?;
    let pid = child.id() as i32;
    fs::write(pid_file(state_dir), pid.to_string())?;
    Ok(pid)
}

/// Ask the running gateway to stop, and wait until it has.
///
/// SIGTERM rather than SIGKILL: the server stops its agent processes and closes
/// the database on the way out, and killing it outright would leave both to be
/// reconciled on the next start.
pub fn stop(
    state_dir: &Path,
    timeout: Duration,
) -> Result<Option<i32>, Box<dyn std::error::Error>> {
    let Some(pid) = running(state_dir) else {
        let _ = fs::remove_file(pid_file(state_dir));
        return Ok(None);
    };
    // SAFETY: a signal to a pid this state directory recorded.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if running(state_dir).is_none() {
            let _ = fs::remove_file(pid_file(state_dir));
            return Ok(Some(pid));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("AgentDock (pid {pid}) did not stop within {timeout:?}").into())
}

/// An address the health check can actually connect to.
///
/// A wildcard bind is a statement about which interfaces to accept on, not a
/// destination: connecting to `0.0.0.0` is not the same as connecting to the
/// server, and a probe sent there reported a healthy gateway as a failed start.
/// Loopback is the interface a wildcard bind is certain to include.
fn probe_target(address: SocketAddr) -> SocketAddr {
    if address.ip().is_unspecified() {
        return match address {
            SocketAddr::V4(_) => ([127, 0, 0, 1], address.port()).into(),
            SocketAddr::V6(_) => (std::net::Ipv6Addr::LOCALHOST, address.port()).into(),
        };
    }
    address
}

/// Whether the gateway answers, not merely whether its process exists. A server
/// that is still opening its database is not yet somewhere to point a browser.
pub async fn healthy(address: SocketAddr) -> bool {
    let url = format!("http://{}/api/health", probe_target(address));
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    client
        .get(url)
        .header("X-AgentDock-Client", "web")
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

/// Wait for a freshly started gateway to answer, so `start` can report a URL
/// that works rather than one that might.
pub async fn wait_until_ready(address: SocketAddr, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if healthy(address).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// The tail of the gateway's log, for the case `start` reports it never became
/// ready: the reason is in the log the detached process wrote, and making
/// someone go find that file themselves is how a startup error stays invisible.
pub fn tail(state_dir: &Path, lines: usize) -> String {
    let Ok(content) = fs::read_to_string(log_file(state_dir)) else {
        return String::new();
    };
    let all: Vec<&str> = content.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

pub fn print_log(state_dir: &Path, lines: usize) -> io::Result<()> {
    let path = log_file(state_dir);
    if !path.exists() {
        println!("No log yet at {}", path.display());
        return Ok(());
    }
    println!("{}", tail(state_dir, lines));
    println!("\n-- {} --", path.display());
    io::stdout().flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary() -> PathBuf {
        let path = env::temp_dir().join(format!("agentdock-daemon-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_pid_file_naming_a_dead_process_does_not_report_a_running_gateway() {
        let state = temporary();
        // A crash leaves the file behind. Treating its number as proof would
        // refuse to start a gateway that is not there.
        fs::write(pid_file(&state), "999999").unwrap();
        assert_eq!(running(&state), None);
        // Nor do the values that name no process at all.
        for content in ["0", "1", "", "not-a-pid", "-5"] {
            fs::write(pid_file(&state), content).unwrap();
            assert_eq!(running(&state), None, "{content:?}");
        }
        fs::remove_dir_all(&state).ok();
    }

    #[test]
    fn this_process_counts_as_running_so_the_liveness_check_is_not_vacuous() {
        let state = temporary();
        // Without this the test above would pass even if `running` always
        // returned None, which would make the check worthless.
        fs::write(pid_file(&state), std::process::id().to_string()).unwrap();
        assert_eq!(running(&state), Some(std::process::id() as i32));
        fs::remove_dir_all(&state).ok();
    }

    #[test]
    fn stopping_what_is_not_running_clears_the_stale_file_instead_of_failing() {
        let state = temporary();
        fs::write(pid_file(&state), "999999").unwrap();
        assert_eq!(stop(&state, Duration::from_secs(1)).unwrap(), None);
        assert!(!pid_file(&state).exists(), "the stale file is cleaned up");
        fs::remove_dir_all(&state).ok();
    }

    #[test]
    fn the_tail_of_a_missing_log_is_empty_rather_than_an_error() {
        let state = temporary();
        assert_eq!(tail(&state, 10), "");
        fs::write(log_file(&state), "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail(&state, 2), "two\nthree");
        // Asking for more than exists returns what exists.
        assert_eq!(tail(&state, 99), "one\ntwo\nthree");
        fs::remove_dir_all(&state).ok();
    }
}
