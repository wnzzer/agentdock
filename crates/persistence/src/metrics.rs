//! Host and session resource history, kept apart from the main database.
//!
//! A separate file because it is written every minute and is disposable: it
//! never contends with session records for the main connection, and deleting
//! it loses nothing but charts. Samples land at [`RAW`] seconds apart and are
//! rolled up into minute and hour buckets, each resolution kept for its own
//! retention, so the file stays a few megabytes however long it runs.
use rusqlite::{Connection, Result, params};
use std::{path::Path, sync::Mutex};

/// Seconds between raw samples.
pub const RAW: i64 = 5;
/// Each resolution in seconds and how long its buckets are kept.
pub const RESOLUTIONS: [(i64, i64); 3] = [(RAW, 86_400), (60, 7 * 86_400), (3_600, 90 * 86_400)];

#[derive(Debug, Clone, PartialEq)]
pub struct HostPoint {
    pub ts: i64,
    pub cpu_avg: f32,
    pub cpu_max: f32,
    pub memory_used_avg: u64,
    pub memory_used_max: u64,
    pub memory_total: u64,
    /// The fastest fan; absent where fans cannot be read.
    pub fan_rpm_avg: Option<u32>,
    pub fan_rpm_max: Option<u32>,
    /// The hottest sensor; absent where temperatures cannot be read.
    pub temperature_avg: Option<f32>,
    pub temperature_max: Option<f32>,
    /// The fastest CPU cluster's clock in MHz; absent where it cannot be read.
    pub cpu_mhz_avg: Option<u32>,
    pub cpu_mhz_max: Option<u32>,
    /// The whole machine's draw in watts; absent where it cannot be read.
    pub power_avg: Option<f32>,
    pub power_max: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionPoint {
    pub ts: i64,
    pub session_id: String,
    pub cpu_avg: f32,
    pub cpu_max: f32,
    pub memory_avg: u64,
    pub memory_max: u64,
}

pub struct MetricsStore {
    connection: Mutex<Connection>,
}

impl MetricsStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 4 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if version < 1 {
            connection.execute_batch(
                "BEGIN;
                CREATE TABLE host_samples(
                    resolution INTEGER NOT NULL, ts INTEGER NOT NULL,
                    cpu_avg REAL NOT NULL, cpu_max REAL NOT NULL,
                    memory_used_avg INTEGER NOT NULL, memory_used_max INTEGER NOT NULL,
                    memory_total INTEGER NOT NULL,
                    PRIMARY KEY(resolution, ts)) WITHOUT ROWID;
                CREATE TABLE session_samples(
                    resolution INTEGER NOT NULL, ts INTEGER NOT NULL, session_id TEXT NOT NULL,
                    cpu_avg REAL NOT NULL, cpu_max REAL NOT NULL,
                    memory_avg INTEGER NOT NULL, memory_max INTEGER NOT NULL,
                    PRIMARY KEY(resolution, ts, session_id)) WITHOUT ROWID;
                PRAGMA user_version = 1;
                COMMIT;",
            )?;
        }
        if version < 2 {
            connection.execute_batch(
                "BEGIN;
                ALTER TABLE host_samples ADD COLUMN fan_rpm_avg INTEGER;
                ALTER TABLE host_samples ADD COLUMN fan_rpm_max INTEGER;
                ALTER TABLE host_samples ADD COLUMN temperature_avg REAL;
                ALTER TABLE host_samples ADD COLUMN temperature_max REAL;
                PRAGMA user_version = 2;
                COMMIT;",
            )?;
        }
        if version < 3 {
            connection.execute_batch(
                "BEGIN;
                ALTER TABLE host_samples ADD COLUMN cpu_mhz_avg INTEGER;
                ALTER TABLE host_samples ADD COLUMN cpu_mhz_max INTEGER;
                PRAGMA user_version = 3;
                COMMIT;",
            )?;
        }
        if version < 4 {
            connection.execute_batch(
                "BEGIN;
                ALTER TABLE host_samples ADD COLUMN power_avg REAL;
                ALTER TABLE host_samples ADD COLUMN power_max REAL;
                PRAGMA user_version = 4;
                COMMIT;",
            )?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Store raw samples in one transaction. A raw point's average and maximum
    /// are the same reading.
    pub fn record(&self, host: &[HostPoint], sessions: &[SessionPoint]) -> Result<()> {
        let mut connection = self.connection.lock().expect("metrics connection");
        let tx = connection.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO host_samples (resolution, ts, cpu_avg, cpu_max,
                    memory_used_avg, memory_used_max, memory_total,
                    fan_rpm_avg, fan_rpm_max, temperature_avg, temperature_max,
                    cpu_mhz_avg, cpu_mhz_max, power_avg, power_max)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;
            for point in host {
                insert.execute(params![
                    RAW,
                    point.ts,
                    point.cpu_avg,
                    point.cpu_max,
                    point.memory_used_avg as i64,
                    point.memory_used_max as i64,
                    point.memory_total as i64,
                    point.fan_rpm_avg,
                    point.fan_rpm_max,
                    point.temperature_avg,
                    point.temperature_max,
                    point.cpu_mhz_avg,
                    point.cpu_mhz_max,
                    point.power_avg,
                    point.power_max
                ])?;
            }
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO session_samples (resolution, ts, session_id, cpu_avg,
                    cpu_max, memory_avg, memory_max)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for point in sessions {
                insert.execute(params![
                    RAW,
                    point.ts,
                    point.session_id,
                    point.cpu_avg,
                    point.cpu_max,
                    point.memory_avg as i64,
                    point.memory_max as i64
                ])?;
            }
        }
        tx.commit()
    }

    /// Roll finished buckets up into the next resolution and drop what has
    /// outlived its retention. Only buckets that ended by `now` are written and
    /// none is written twice, so samples have to be recorded before a compact
    /// passes their bucket; a bucket is never rebuilt from rows already
    /// partly dropped.
    pub fn compact(&self, now: i64) -> Result<()> {
        let mut connection = self.connection.lock().expect("metrics connection");
        let tx = connection.transaction()?;
        for pair in RESOLUTIONS.windows(2) {
            let (from, to) = (pair[0].0, pair[1].0);
            let start: i64 = tx.query_row(
                "SELECT COALESCE(MAX(ts) + ?1, 0) FROM host_samples WHERE resolution = ?1",
                [to],
                |row| row.get(0),
            )?;
            let end = now.div_euclid(to) * to;
            tx.execute(
                "INSERT OR REPLACE INTO host_samples (resolution, ts, cpu_avg, cpu_max,
                    memory_used_avg, memory_used_max, memory_total,
                    fan_rpm_avg, fan_rpm_max, temperature_avg, temperature_max,
                    cpu_mhz_avg, cpu_mhz_max, power_avg, power_max)
                 SELECT ?2, (ts / ?2) * ?2, AVG(cpu_avg), MAX(cpu_max),
                        CAST(AVG(memory_used_avg) AS INTEGER), MAX(memory_used_max), MAX(memory_total),
                        CAST(AVG(fan_rpm_avg) AS INTEGER), MAX(fan_rpm_max),
                        AVG(temperature_avg), MAX(temperature_max),
                        CAST(AVG(cpu_mhz_avg) AS INTEGER), MAX(cpu_mhz_max),
                        AVG(power_avg), MAX(power_max)
                 FROM host_samples WHERE resolution = ?1 AND ts >= ?3 AND ts < ?4
                 GROUP BY ts / ?2",
                params![from, to, start, end],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO session_samples (resolution, ts, session_id, cpu_avg,
                    cpu_max, memory_avg, memory_max)
                 SELECT ?2, (ts / ?2) * ?2, session_id, AVG(cpu_avg), MAX(cpu_max),
                        CAST(AVG(memory_avg) AS INTEGER), MAX(memory_max)
                 FROM session_samples WHERE resolution = ?1 AND ts >= ?3 AND ts < ?4
                 GROUP BY ts / ?2, session_id",
                params![from, to, start, end],
            )?;
        }
        for (resolution, keep) in RESOLUTIONS {
            for table in ["host_samples", "session_samples"] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE resolution = ?1 AND ts < ?2"),
                    params![resolution, now - keep],
                )?;
            }
        }
        tx.commit()
    }

    /// Forget everything recorded, at every resolution.
    pub fn clear(&self) -> Result<()> {
        let connection = self.connection.lock().expect("metrics connection");
        connection.execute_batch("DELETE FROM host_samples; DELETE FROM session_samples;")
    }

    pub fn host_history(&self, resolution: i64, from: i64, to: i64) -> Result<Vec<HostPoint>> {
        let connection = self.connection.lock().expect("metrics connection");
        let mut statement = connection.prepare_cached(
            "SELECT ts, cpu_avg, cpu_max, memory_used_avg, memory_used_max, memory_total,
                    fan_rpm_avg, fan_rpm_max, temperature_avg, temperature_max,
                    cpu_mhz_avg, cpu_mhz_max, power_avg, power_max
             FROM host_samples WHERE resolution = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts",
        )?;
        statement
            .query_map(params![resolution, from, to], |row| {
                Ok(HostPoint {
                    ts: row.get(0)?,
                    cpu_avg: row.get(1)?,
                    cpu_max: row.get(2)?,
                    memory_used_avg: row.get::<_, i64>(3)? as u64,
                    memory_used_max: row.get::<_, i64>(4)? as u64,
                    memory_total: row.get::<_, i64>(5)? as u64,
                    fan_rpm_avg: row.get(6)?,
                    fan_rpm_max: row.get(7)?,
                    temperature_avg: row.get(8)?,
                    temperature_max: row.get(9)?,
                    cpu_mhz_avg: row.get(10)?,
                    cpu_mhz_max: row.get(11)?,
                    power_avg: row.get(12)?,
                    power_max: row.get(13)?,
                })
            })?
            .collect()
    }

    /// Every session's points in the range, or one session's when named.
    pub fn session_history(
        &self,
        resolution: i64,
        from: i64,
        to: i64,
        session_id: Option<&str>,
    ) -> Result<Vec<SessionPoint>> {
        let connection = self.connection.lock().expect("metrics connection");
        let mut statement = connection.prepare_cached(
            "SELECT ts, session_id, cpu_avg, cpu_max, memory_avg, memory_max
             FROM session_samples
             WHERE resolution = ?1 AND ts >= ?2 AND ts <= ?3 AND (?4 IS NULL OR session_id = ?4)
             ORDER BY session_id, ts",
        )?;
        statement
            .query_map(params![resolution, from, to, session_id], |row| {
                Ok(SessionPoint {
                    ts: row.get(0)?,
                    session_id: row.get(1)?,
                    cpu_avg: row.get(2)?,
                    cpu_max: row.get(3)?,
                    memory_avg: row.get::<_, i64>(4)? as u64,
                    memory_max: row.get::<_, i64>(5)? as u64,
                })
            })?
            .collect()
    }
}

/// The finest resolution still kept at `from` that answers `from..=to` in at
/// most `max_points` buckets, or the coarsest when none does.
pub fn resolution_for(now: i64, from: i64, to: i64, max_points: i64) -> i64 {
    RESOLUTIONS
        .iter()
        .find(|(resolution, keep)| from >= now - keep && (to - from) / resolution <= max_points)
        .map_or(RESOLUTIONS[RESOLUTIONS.len() - 1].0, |(resolution, _)| {
            *resolution
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(ts: i64, cpu: f32, memory: u64) -> HostPoint {
        HostPoint {
            ts,
            cpu_avg: cpu,
            cpu_max: cpu,
            memory_used_avg: memory,
            memory_used_max: memory,
            memory_total: 1000,
            fan_rpm_avg: None,
            fan_rpm_max: None,
            temperature_avg: None,
            temperature_max: None,
            cpu_mhz_avg: None,
            cpu_mhz_max: None,
            power_avg: None,
            power_max: None,
        }
    }
    fn session(ts: i64, id: &str, cpu: f32) -> SessionPoint {
        SessionPoint {
            ts,
            session_id: id.into(),
            cpu_avg: cpu,
            cpu_max: cpu,
            memory_avg: 10,
            memory_max: 10,
        }
    }

    #[test]
    fn finished_minutes_roll_up_and_the_current_one_waits() {
        let store = MetricsStore::open(":memory:").unwrap();
        let base = 1_800_000_000 / 3600 * 3600;
        let hosts: Vec<_> = (0..24)
            .map(|i| {
                host(
                    base + i * RAW,
                    if i < 12 { 10.0 } else { 50.0 },
                    100 + i as u64,
                )
            })
            .collect();
        let sessions: Vec<_> = (0..12)
            .flat_map(|i| {
                [
                    session(base + i * RAW, "a", i as f32),
                    session(base + i * RAW, "b", 1.0),
                ]
            })
            .collect();
        store.record(&hosts, &sessions).unwrap();

        // Partway into the second minute: only the first is complete.
        store.compact(base + 90).unwrap();
        let minutes = store.host_history(60, base, base + 3600).unwrap();
        assert_eq!(minutes.len(), 1);
        assert_eq!(minutes[0].ts, base);
        assert_eq!(minutes[0].cpu_avg, 10.0);
        assert_eq!(minutes[0].memory_used_max, 111);
        let a = store
            .session_history(60, base, base + 3600, Some("a"))
            .unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].cpu_avg, 5.5);
        assert_eq!(a[0].cpu_max, 11.0);
        assert_eq!(
            store
                .session_history(60, base, base + 3600, None)
                .unwrap()
                .len(),
            2
        );

        // Run again after the second minute ends, and once more to show a
        // repeat adds nothing.
        store.compact(base + 120).unwrap();
        store.compact(base + 120).unwrap();
        let minutes = store.host_history(60, base, base + 3600).unwrap();
        assert_eq!(
            minutes.iter().map(|p| p.cpu_avg).collect::<Vec<_>>(),
            [10.0, 50.0]
        );

        // Once the hour ends it rolls up from the minutes.
        store.compact(base + 3600).unwrap();
        let hours = store.host_history(3600, base, base + 3600).unwrap();
        assert_eq!(hours.len(), 1);
        assert_eq!(hours[0].cpu_avg, 30.0);
        assert_eq!(hours[0].cpu_max, 50.0);
    }

    #[test]
    fn each_resolution_is_dropped_after_its_retention() {
        let store = MetricsStore::open(":memory:").unwrap();
        let base = 1_800_000_000 / 3600 * 3600;
        store
            .record(&[host(base, 1.0, 1)], &[session(base, "a", 1.0)])
            .unwrap();
        store.compact(base + 3600).unwrap();
        store.compact(base + 86_400 + 1).unwrap();
        assert!(store.host_history(RAW, 0, i64::MAX).unwrap().is_empty());
        assert!(
            store
                .session_history(RAW, 0, i64::MAX, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.host_history(60, 0, i64::MAX).unwrap().len(), 1);
        store.compact(base + 7 * 86_400 + 1).unwrap();
        assert!(store.host_history(60, 0, i64::MAX).unwrap().is_empty());
        assert_eq!(store.host_history(3600, 0, i64::MAX).unwrap().len(), 1);
    }

    #[test]
    fn resolution_follows_span_and_retention() {
        let now = 1_800_000_000;
        assert_eq!(resolution_for(now, now - 3600, now, 1000), RAW);
        assert_eq!(resolution_for(now, now - 6 * 3600, now, 1000), 60);
        assert_eq!(resolution_for(now, now - 7 * 86_400, now, 1000), 3600);
        // A short span older than a day is only kept in minutes.
        assert_eq!(
            resolution_for(now, now - 2 * 86_400, now - 2 * 86_400 + 600, 1000),
            60
        );
    }

    #[test]
    fn sensors_roll_up_and_stay_absent_where_never_read() {
        let store = MetricsStore::open(":memory:").unwrap();
        let base = 1_800_000_000 / 3600 * 3600;
        let sensed = |ts, rpm: Option<u32>, celsius| HostPoint {
            fan_rpm_avg: rpm,
            fan_rpm_max: rpm,
            temperature_avg: celsius,
            temperature_max: celsius,
            cpu_mhz_avg: rpm.map(|rpm| rpm + 500),
            cpu_mhz_max: rpm.map(|rpm| rpm + 500),
            power_avg: celsius.map(|c| c / 2.0),
            power_max: celsius.map(|c| c / 2.0),
            ..host(ts, 1.0, 1)
        };
        store
            .record(
                &[
                    sensed(base, Some(1000), Some(40.0)),
                    // A reading that failed once does not drag the average to 0.
                    sensed(base + RAW, None, None),
                    sensed(base + 2 * RAW, Some(3000), Some(60.0)),
                    sensed(base + 60, None, None),
                ],
                &[],
            )
            .unwrap();
        store.compact(base + 120).unwrap();
        let minutes = store.host_history(60, base, base + 3600).unwrap();
        assert_eq!(minutes[0].fan_rpm_avg, Some(2000));
        assert_eq!(minutes[0].fan_rpm_max, Some(3000));
        assert_eq!(minutes[0].temperature_avg, Some(50.0));
        assert_eq!(minutes[0].temperature_max, Some(60.0));
        assert_eq!(minutes[0].cpu_mhz_avg, Some(2500));
        assert_eq!(minutes[0].cpu_mhz_max, Some(3500));
        assert_eq!(minutes[0].power_avg, Some(25.0));
        assert_eq!(minutes[0].power_max, Some(30.0));
        assert_eq!(minutes[1].fan_rpm_max, None);
        assert_eq!(minutes[1].temperature_max, None);
        assert_eq!(minutes[1].cpu_mhz_max, None);
        assert_eq!(minutes[1].power_max, None);
    }

    #[test]
    fn a_version_one_file_gains_the_sensor_columns_and_keeps_its_rows() {
        let path =
            std::env::temp_dir().join(format!("agentdock-metrics-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE host_samples(
                        resolution INTEGER NOT NULL, ts INTEGER NOT NULL,
                        cpu_avg REAL NOT NULL, cpu_max REAL NOT NULL,
                        memory_used_avg INTEGER NOT NULL, memory_used_max INTEGER NOT NULL,
                        memory_total INTEGER NOT NULL,
                        PRIMARY KEY(resolution, ts)) WITHOUT ROWID;
                    CREATE TABLE session_samples(
                        resolution INTEGER NOT NULL, ts INTEGER NOT NULL, session_id TEXT NOT NULL,
                        cpu_avg REAL NOT NULL, cpu_max REAL NOT NULL,
                        memory_avg INTEGER NOT NULL, memory_max INTEGER NOT NULL,
                        PRIMARY KEY(resolution, ts, session_id)) WITHOUT ROWID;
                    INSERT INTO host_samples VALUES (5, 100, 1.0, 2.0, 3, 4, 5);
                    PRAGMA user_version = 1;",
                )
                .unwrap();
        }
        let store = MetricsStore::open(&path).unwrap();
        let old = store.host_history(RAW, 0, 1000).unwrap();
        assert_eq!(old.len(), 1);
        assert_eq!(
            (old[0].cpu_max, old[0].fan_rpm_max, old[0].cpu_mhz_max),
            (2.0, None, None)
        );
        store.record(&[host(200, 1.0, 1)], &[]).unwrap();
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
