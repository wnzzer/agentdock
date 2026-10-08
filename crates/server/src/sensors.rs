//! Fan speeds, temperatures, CPU clocks and power, where the platform offers
//! them.
//!
//! Temperatures come from sysinfo on every platform. Fans have no portable
//! source: macOS reads them from the SMC, Linux from hwmon, and elsewhere the
//! list is empty. An empty list means "not known", never "stopped"; a machine
//! without fans reports the same. CPU clocks are live only where the platform
//! reports them live: per cluster from IOReport on Apple Silicon, and averaged
//! over the cores on Linux. Elsewhere sysinfo has only a nominal figure, which
//! is not reported.
//!
//! Power is the whole machine's draw where it is known: the SMC on a Mac, the
//! battery while it discharges on a Linux laptop. Its parts come from the
//! chip's energy counters: IOReport on Apple Silicon, RAPL for the CPU package
//! on Linux, which most distributions let only root read. A desktop running
//! Linux as a user, and Windows, report no power.
#[cfg(target_os = "macos")]
mod ioreport;
#[cfg(target_os = "macos")]
mod smc;

use serde::Serialize;
use std::{fs, path::Path, time::Instant};
use sysinfo::{Components, System};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fan {
    pub label: String,
    pub rpm: u32,
    pub min_rpm: Option<u32>,
    pub max_rpm: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Temperature {
    pub label: String,
    pub celsius: f32,
    pub critical_celsius: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuFrequency {
    /// The cluster as the platform names it, such as `PCPU` or `ECPU1`, or
    /// `CPU` where cores are not told apart.
    pub label: String,
    /// `efficiency` or `performance` on a machine with both; absent otherwise.
    pub kind: Option<&'static str>,
    /// The clock while running over the last interval; absent when the
    /// cluster sat idle throughout.
    pub mhz: Option<u32>,
    pub max_mhz: Option<u32>,
}

/// One part of the machine's draw: `cpu`, `gpu`, `ane` (the neural engine)
/// or `dram`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PowerPart {
    pub kind: &'static str,
    pub watts: f32,
}

/// What one sample read from the chip: clocks, and power where it is known.
#[derive(Debug, Default)]
pub struct ChipReading {
    pub clocks: Vec<CpuFrequency>,
    pub power_watts: Option<f32>,
    pub power_parts: Vec<PowerPart>,
}

/// Keeps what a live clock or power reading needs between samples: both are
/// counters, read as a rate over the time since the last one.
pub struct ChipSensors {
    #[cfg(target_os = "macos")]
    reader: Option<ioreport::Reader>,
    /// The CPU package's energy counter when last read, and when.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    rapl: Option<(u64, Instant)>,
}

impl ChipSensors {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            reader: ioreport::Reader::open(),
            rapl: None,
        }
    }

    pub fn sample(&mut self, system: &mut System) -> ChipReading {
        #[cfg(target_os = "macos")]
        {
            let _ = system;
            let chip = self
                .reader
                .as_mut()
                .map(ioreport::Reader::sample)
                .unwrap_or_default();
            ChipReading {
                clocks: chip.clusters,
                power_watts: smc::system_power(),
                power_parts: chip.power,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            if !cfg!(target_os = "linux") {
                return ChipReading::default();
            }
            system.refresh_cpu_frequency();
            let cpu = self.rapl_watts(Path::new("/sys/class/powercap/intel-rapl:0"));
            ChipReading {
                clocks: average_clock(system.cpus().iter().map(|cpu| cpu.frequency())),
                power_watts: battery_draw(Path::new("/sys/class/power_supply")),
                power_parts: cpu
                    .map(|watts| vec![PowerPart { kind: "cpu", watts }])
                    .unwrap_or_default(),
            }
        }
    }

    /// The CPU package's power from its RAPL counter, which wraps at
    /// `max_energy_range_uj`; nothing on the first read or where the counter
    /// cannot be read.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn rapl_watts(&mut self, domain: &Path) -> Option<f32> {
        let read = |name: &str| -> Option<u64> {
            fs::read_to_string(domain.join(name))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        let energy = read("energy_uj")?;
        let now = Instant::now();
        let previous = self.rapl.replace((energy, now));
        let (before, at) = previous?;
        let range = read("max_energy_range_uj").unwrap_or(u64::MAX);
        let used = if energy >= before {
            energy - before
        } else {
            range.saturating_sub(before) + energy
        };
        let seconds = now.duration_since(at).as_secs_f64();
        (seconds > 0.0).then(|| (used as f64 / 1e6 / seconds) as f32)
    }
}

/// What the batteries supply while discharging, in watts: the machine's whole
/// draw on a laptop off its charger. Charging, or with no battery, unknown.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn battery_draw(root: &Path) -> Option<f32> {
    let read = |path: &Path| {
        fs::read_to_string(path)
            .ok()
            .map(|text| text.trim().to_owned())
    };
    let mut total = None;
    for supply in fs::read_dir(root).ok()?.flatten() {
        let supply = supply.path();
        if read(&supply.join("type")).as_deref() != Some("Battery")
            || read(&supply.join("status")).as_deref() != Some("Discharging")
        {
            continue;
        }
        let number = |name: &str| read(&supply.join(name))?.parse::<f64>().ok();
        // Microwatts, or microamps times microvolts.
        let watts = number("power_now")
            .map(|microwatts| microwatts / 1e6)
            .or_else(|| Some(number("current_now")? * number("voltage_now")? / 1e12));
        if let Some(watts) = watts.filter(|watts| *watts > 0.0) {
            *total.get_or_insert(0.0) += watts as f32;
        }
    }
    total
}

/// One figure for the whole machine, from the cores that report a clock.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn average_clock(cores: impl Iterator<Item = u64>) -> Vec<CpuFrequency> {
    let (sum, count) = cores
        .filter(|&mhz| mhz > 0)
        .fold((0u64, 0u64), |(sum, count), mhz| (sum + mhz, count + 1));
    if count == 0 {
        return Vec::new();
    }
    vec![CpuFrequency {
        label: "CPU".into(),
        kind: None,
        mhz: u32::try_from(sum / count).ok(),
        max_mhz: None,
    }]
}

#[cfg(target_os = "macos")]
pub use smc::fans;

#[cfg(not(target_os = "macos"))]
pub fn fans() -> Vec<Fan> {
    if cfg!(target_os = "linux") {
        hwmon_fans(Path::new("/sys/class/hwmon"))
    } else {
        Vec::new()
    }
}

pub fn temperatures(components: &mut Components) -> Vec<Temperature> {
    components.refresh(true);
    components
        .list()
        .iter()
        .filter_map(|component| {
            // Zero or below is a sensor that is present but not reporting.
            let celsius = component
                .temperature()
                .filter(|t| t.is_finite() && *t > 0.0)?;
            Some(Temperature {
                label: component.label().to_owned(),
                celsius,
                critical_celsius: component.critical().filter(|t| t.is_finite()),
            })
        })
        .collect()
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn read_number(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Every `fanN_input` under each hwmon device. A fan without its own label is
/// named after its chip, so two chips' `fan1` stay apart.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn hwmon_fans(root: &Path) -> Vec<Fan> {
    let Ok(devices) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut fans = Vec::new();
    for device in devices.flatten() {
        let device = device.path();
        let chip = fs::read_to_string(device.join("name"))
            .map(|name| name.trim().to_owned())
            .unwrap_or_else(|_| {
                device
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            });
        let Ok(entries) = fs::read_dir(&device) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(fan) = name
                .to_str()
                .and_then(|name| name.strip_suffix("_input"))
                .filter(|fan| {
                    fan.strip_prefix("fan")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
            else {
                continue;
            };
            let Some(rpm) = read_number(&entry.path()) else {
                continue;
            };
            let label = fs::read_to_string(device.join(format!("{fan}_label")))
                .map(|label| label.trim().to_owned())
                .ok()
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| format!("{chip} {fan}"));
            fans.push(Fan {
                label,
                rpm,
                min_rpm: read_number(&device.join(format!("{fan}_min"))),
                max_rpm: read_number(&device.join(format!("{fan}_max"))),
            });
        }
    }
    fans.sort_by(|a, b| a.label.cmp(&b.label));
    fans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hwmon_fans_are_read_with_their_labels_and_limits() {
        let root = std::env::temp_dir().join(format!("agentdock-hwmon-{}", uuid::Uuid::new_v4()));
        let write = |path: &str, value: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        };
        write("hwmon0/name", "nct6775\n");
        write("hwmon0/fan1_input", "1200\n");
        write("hwmon0/fan1_label", "CPU Fan\n");
        write("hwmon0/fan1_min", "300\n");
        write("hwmon0/fan2_input", "800\n");
        // Not fans, or not readable as a speed.
        write("hwmon0/fan2_alarm", "0\n");
        write("hwmon0/temp1_input", "45000\n");
        write("hwmon0/fan3_input", "\n");
        write("hwmon1/name", "coretemp\n");
        write("hwmon1/temp1_input", "50000\n");
        write("hwmon2/fan1_input", "2000\n");
        write("hwmon2/fan1_max", "6000\n");

        let fans = hwmon_fans(&root);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            fans,
            [
                Fan {
                    label: "CPU Fan".into(),
                    rpm: 1200,
                    min_rpm: Some(300),
                    max_rpm: None,
                },
                Fan {
                    label: "hwmon2 fan1".into(),
                    rpm: 2000,
                    min_rpm: None,
                    max_rpm: Some(6000),
                },
                Fan {
                    label: "nct6775 fan2".into(),
                    rpm: 800,
                    min_rpm: None,
                    max_rpm: None,
                },
            ]
        );
    }

    #[test]
    fn the_machine_clock_averages_the_cores_that_report_one() {
        assert_eq!(
            average_clock([3000, 0, 1000].into_iter())[0].mhz,
            Some(2000)
        );
        assert!(average_clock([0, 0].into_iter()).is_empty());
        assert!(average_clock(std::iter::empty()).is_empty());
    }

    fn tree(files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("agentdock-sys-{}", uuid::Uuid::new_v4()));
        for (path, value) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        }
        root
    }

    #[test]
    fn a_discharging_battery_is_the_machines_draw() {
        let root = tree(&[
            ("BAT0/type", "Battery\n"),
            ("BAT0/status", "Discharging\n"),
            ("BAT0/power_now", "12500000\n"),
            // A second battery reports current and voltage instead.
            ("BAT1/type", "Battery\n"),
            ("BAT1/status", "Discharging\n"),
            ("BAT1/current_now", "500000\n"),
            ("BAT1/voltage_now", "12000000\n"),
            // The charger is not a battery.
            ("AC/type", "Mains\n"),
            ("AC/power_now", "65000000\n"),
        ]);
        assert_eq!(battery_draw(&root), Some(18.5));
        fs::write(root.join("BAT0/status"), "Charging\n").unwrap();
        fs::write(root.join("BAT1/status"), "Full\n").unwrap();
        assert_eq!(
            battery_draw(&root),
            None,
            "on the charger the draw is unknown"
        );
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            battery_draw(Path::new("/nonexistent/agentdock/power")),
            None
        );
    }

    #[test]
    fn the_cpu_package_counter_is_read_as_a_rate_across_its_wrap() {
        let root = tree(&[("energy_uj", "900\n"), ("max_energy_range_uj", "1000\n")]);
        let mut chip = ChipSensors {
            #[cfg(target_os = "macos")]
            reader: None,
            rapl: None,
        };
        assert_eq!(chip.rapl_watts(&root), None, "the first read is a baseline");
        // Pretend the first read was a second ago, then the counter wrapped.
        chip.rapl = Some((900, Instant::now() - std::time::Duration::from_secs(1)));
        fs::write(root.join("energy_uj"), "400\n").unwrap();
        let watts = chip.rapl_watts(&root).unwrap();
        // 100 to the wrap and 400 after: 500 microjoules in about a second.
        assert!((watts - 0.0005).abs() < 0.0001, "{watts}");
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            chip.rapl_watts(Path::new("/nonexistent/agentdock/rapl")),
            None
        );
    }

    #[test]
    fn a_missing_hwmon_tree_has_no_fans() {
        assert!(hwmon_fans(Path::new("/nonexistent/agentdock/hwmon")).is_empty());
    }

    /// Hosted CI runners are virtual machines without sensors, so this only
    /// runs when asked for, on real hardware:
    /// `cargo test -p agentdock-server -- --ignored sensors`. Set
    /// `AGENTDOCK_NO_FANS=1` on a machine that has none, such as a MacBook Air.
    #[test]
    #[ignore = "needs a machine with real sensors"]
    fn this_machine_reports_real_temperatures_and_fans() {
        let temperatures = temperatures(&mut Components::new_with_refreshed_list());
        assert!(!temperatures.is_empty(), "no temperature sensors read");
        for temperature in &temperatures {
            assert!(
                (-20.0..150.0).contains(&temperature.celsius),
                "{temperature:?}"
            );
        }
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let mut chip = ChipSensors::new();
            let mut system = System::new();
            chip.sample(&mut system);
            // Keep a core busy so at least one cluster runs over the interval.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
            let mut spin = 0u64;
            while std::time::Instant::now() < until {
                spin = std::hint::black_box(spin.wrapping_add(1));
            }
            let reading = chip.sample(&mut system);
            let clocks = reading.clocks;
            if cfg!(target_os = "macos") {
                // A Mac reports its whole draw and the chip's parts; a Linux
                // desktop without root may report neither.
                let watts = reading.power_watts.expect("whole-machine power");
                assert!((0.5..500.0).contains(&watts), "{watts} W");
                let cpu = reading.power_parts.iter().find(|part| part.kind == "cpu");
                assert!(
                    cpu.is_some_and(|part| part.watts > 0.0),
                    "{:?}",
                    reading.power_parts
                );
            }
            assert!(
                clocks.iter().any(|c| c.mhz.is_some()),
                "no live CPU clock: {clocks:?}"
            );
            for c in &clocks {
                if let (Some(mhz), Some(max)) = (c.mhz, c.max_mhz) {
                    assert!((100..=max).contains(&mhz), "{c:?}");
                }
            }
        }
        let fans = fans();
        if std::env::var_os("AGENTDOCK_NO_FANS").is_none() {
            assert!(
                !fans.is_empty(),
                "no fans read; set AGENTDOCK_NO_FANS=1 if there are none"
            );
        }
        for fan in &fans {
            // A stopped fan reads 0, which is a real speed.
            assert!(fan.rpm < 20_000, "{fan:?}");
            if let Some(max) = fan.max_rpm {
                assert!(fan.rpm <= max + max / 10, "{fan:?}");
                assert!(fan.min_rpm.is_none_or(|min| min <= max), "{fan:?}");
            }
        }
    }
}
