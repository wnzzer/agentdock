//! Fan speeds and temperatures, where the platform offers them.
//!
//! Temperatures come from sysinfo on every platform. Fans have no portable
//! source: macOS reads them from the SMC, Linux from hwmon, and elsewhere the
//! list is empty. An empty list means "not known", never "stopped"; a machine
//! without fans reports the same.
#[cfg(target_os = "macos")]
mod smc;

use serde::Serialize;
use std::{fs, path::Path};
use sysinfo::Components;

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
            let celsius = component.temperature().filter(|t| t.is_finite())?;
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
