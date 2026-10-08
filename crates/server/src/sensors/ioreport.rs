//! CPU cluster clocks and the chip's power on Apple Silicon, from IOReport.
//!
//! IOReport is a private but long-stable library (Stats, macmon and asitop
//! read the same channels) that needs no privileges. The "CPU Complex
//! Performance States" channels report, per cluster, how long it spent in
//! each performance state; the frequency of each state comes from the `pmgr`
//! device's voltage tables. A clock is therefore an average over the time
//! between two samples, weighted by how long the cluster ran at each step, and
//! the first sample after start is only a baseline.
//!
//! The "Energy Model" group counts the energy each part of the chip used;
//! over the time between two samples that is its power. Each channel states
//! its own unit (the CPU counts millijoules, the GPU nanojoules).
//!
//! Intel Macs have neither the channels nor the tables, so they report none,
//! and neither does a virtual machine: its CPUs have no clock of their own,
//! and IOReport is not probed there at all, since a guest's device tree is
//! not the hardware's.
use super::{CpuFrequency, PowerPart};
use std::{
    ffi::{CStr, c_char, c_void},
    mem::size_of,
    ptr, slice,
    time::Instant,
};

type CFTypeRef = *const c_void;
type CFIndex = isize;

const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFAllocatorDefault: CFTypeRef;
    fn CFRelease(object: CFTypeRef);
    fn CFGetTypeID(object: CFTypeRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        text: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    fn CFStringGetCString(
        text: CFTypeRef,
        buffer: *mut c_char,
        size: CFIndex,
        encoding: u32,
    ) -> bool;
    fn CFDictionaryGetCount(dictionary: CFTypeRef) -> CFIndex;
    fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFDictionaryCreateMutableCopy(
        allocator: CFTypeRef,
        capacity: CFIndex,
        dictionary: CFTypeRef,
    ) -> CFTypeRef;
    fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceNameMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(object: u32) -> i32;
}

#[link(name = "IOReport", kind = "dylib")]
unsafe extern "C" {
    fn IOReportCopyChannelsInGroup(
        group: CFTypeRef,
        subgroup: CFTypeRef,
        a: u64,
        b: u64,
        c: u64,
    ) -> CFTypeRef;
    fn IOReportCreateSubscription(
        a: *const c_void,
        desired: CFTypeRef,
        subscribed: *mut CFTypeRef,
        channel_id: u64,
        b: CFTypeRef,
    ) -> CFTypeRef;
    fn IOReportCreateSamples(
        subscription: CFTypeRef,
        channels: CFTypeRef,
        a: CFTypeRef,
    ) -> CFTypeRef;
    fn IOReportCreateSamplesDelta(
        previous: CFTypeRef,
        current: CFTypeRef,
        a: CFTypeRef,
    ) -> CFTypeRef;
    fn IOReportChannelGetChannelName(channel: CFTypeRef) -> CFTypeRef;
    fn IOReportChannelGetGroup(channel: CFTypeRef) -> CFTypeRef;
    fn IOReportChannelGetUnitLabel(channel: CFTypeRef) -> CFTypeRef;
    fn IOReportSimpleGetIntegerValue(channel: CFTypeRef, a: *mut i32) -> i64;
    fn IOReportMergeChannels(into: CFTypeRef, from: CFTypeRef, a: CFTypeRef);
    fn IOReportStateGetCount(channel: CFTypeRef) -> i32;
    fn IOReportStateGetNameForIndex(channel: CFTypeRef, index: i32) -> CFTypeRef;
    fn IOReportStateGetResidency(channel: CFTypeRef, index: i32) -> i64;
}

/// A Core Foundation object this code created, released when dropped.
struct Owned(CFTypeRef);

impl Owned {
    fn new(object: CFTypeRef) -> Option<Self> {
        (!object.is_null()).then_some(Self(object))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: only created from a Create/Copy call, so this holds a reference.
        unsafe { CFRelease(self.0) }
    }
}

fn cf_string(text: &CStr) -> Option<Owned> {
    // SAFETY: a NUL-terminated string in, an owned CFString or null out.
    Owned::new(unsafe { CFStringCreateWithCString(kCFAllocatorDefault, text.as_ptr(), UTF8) })
}

fn rust_string(text: CFTypeRef) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut buffer = [0 as c_char; 128];
    // SAFETY: the buffer's length is passed, and the call NUL-terminates it on success.
    unsafe { CFStringGetCString(text, buffer.as_mut_ptr(), buffer.len() as CFIndex, UTF8) }.then(
        || {
            unsafe { CStr::from_ptr(buffer.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        },
    )
}

/// The frequency of each performance state, in MHz, from a `pmgr` table of
/// (frequency, voltage) pairs of little-endian `u32`. M1 to M3 give hertz and
/// later chips kilohertz; no clock is under 100 MHz or over 100 GHz, so the
/// largest entry tells which.
fn table_mhz(bytes: &[u8]) -> Vec<u32> {
    let (pairs, _) = bytes.as_chunks::<8>();
    let raw: Vec<u32> = pairs
        .iter()
        .map(|[a, b, c, d, ..]| u32::from_le_bytes([*a, *b, *c, *d]))
        .collect();
    let divisor = if raw.iter().any(|&value| value > 100_000_000) {
        1_000_000
    } else {
        1_000
    };
    raw.into_iter().map(|value| value / divisor).collect()
}

fn pmgr_table(key: &CStr) -> Option<Vec<u32>> {
    let key = cf_string(key)?;
    // SAFETY: IOKit lookups; the matching dictionary is consumed by the lookup
    // and the service released once its property is copied.
    let data = unsafe {
        let service = IOServiceGetMatchingService(0, IOServiceNameMatching(c"pmgr".as_ptr()));
        if service == 0 {
            return None;
        }
        let data = IORegistryEntryCreateCFProperty(service, key.0, kCFAllocatorDefault, 0);
        IOObjectRelease(service);
        Owned::new(data)?
    };
    // SAFETY: checked to be CFData before its bytes are read, and the slice
    // does not outlive `data`.
    let table = unsafe {
        if CFGetTypeID(data.0) != CFDataGetTypeID() {
            return None;
        }
        let length = usize::try_from(CFDataGetLength(data.0)).ok()?;
        let bytes = CFDataGetBytePtr(data.0);
        if length == 0 || bytes.is_null() {
            return None;
        }
        table_mhz(slice::from_raw_parts(bytes, length))
    };
    (!table.is_empty()).then_some(table)
}

/// Whether macOS runs as a guest, as hosted CI runners do.
fn in_virtual_machine() -> bool {
    let mut present: i32 = 0;
    let mut size = size_of::<i32>();
    // SAFETY: the output buffer is an `i32` and its size is passed with it.
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.hv_vmm_present".as_ptr(),
            (&mut present as *mut i32).cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    status == 0 && present != 0
}

/// The states a cluster is not running in. They come first, before the
/// performance states the table describes in order.
const RESTING: [&str; 3] = ["IDLE", "OFF", "DOWN"];

/// The clock a cluster ran at while it was running, weighted by time in each
/// state; none when it did not run at all over the interval.
fn running_mhz(states: &[(String, i64)], table: &[u32]) -> Option<u32> {
    let first = states
        .iter()
        .position(|(name, _)| !RESTING.contains(&name.as_str()))?;
    let (mut weighted, mut running) = (0f64, 0f64);
    for ((_, residency), mhz) in states[first..].iter().zip(table) {
        let residency = (*residency).max(0) as f64;
        weighted += residency * f64::from(*mhz);
        running += residency;
    }
    (running > 0.0).then(|| (weighted / running).round() as u32)
}

/// The parts of the chip whose energy is reported, by channel name. Many
/// more channels exist (each core, each cache); these are the totals.
fn power_kind(channel: &str) -> Option<&'static str> {
    if channel.ends_with("CPU Energy") {
        Some("cpu")
    } else if channel == "GPU Energy" {
        Some("gpu")
    } else if channel.starts_with("ANE") {
        Some("ane")
    } else if channel == "DRAM" {
        Some("dram")
    } else {
        None
    }
}

fn joules(unit: &str) -> Option<f64> {
    match unit {
        "mJ" => Some(1e-3),
        "uJ" | "\u{b5}J" => Some(1e-6),
        "nJ" => Some(1e-9),
        _ => None,
    }
}

/// Watts per part from each energy channel's (name, unit, count) over
/// `seconds`, parts in a fixed order and each summed over its channels.
fn power_parts(channels: &[(String, String, i64)], seconds: f64) -> Vec<PowerPart> {
    if seconds <= 0.0 {
        return Vec::new();
    }
    let mut parts: Vec<PowerPart> = Vec::new();
    for (name, unit, count) in channels {
        let (Some(kind), Some(scale)) = (power_kind(name), joules(unit)) else {
            continue;
        };
        let watts = ((*count).max(0) as f64 * scale / seconds) as f32;
        match parts.iter_mut().find(|part| part.kind == kind) {
            Some(part) => part.watts += watts,
            None => parts.push(PowerPart { kind, watts }),
        }
    }
    let order = |kind: &str| {
        ["cpu", "gpu", "ane", "dram"]
            .iter()
            .position(|k| *k == kind)
    };
    parts.sort_by_key(|part| order(part.kind));
    parts
}

/// What one sample read from the chip.
#[derive(Default)]
pub struct Reading {
    pub clusters: Vec<CpuFrequency>,
    pub power: Vec<PowerPart>,
}

pub struct Reader {
    subscription: Owned,
    channels: Owned,
    _desired: Owned,
    previous: Option<(Owned, Instant)>,
    /// The clock of each performance state, efficiency then performance
    /// cluster; absent where the tables are, and then no clocks are read.
    tables: Option<(Vec<u32>, Vec<u32>)>,
}

// SAFETY: the Core Foundation objects are only ever used through `&mut self`,
// and the sampler that owns this reader keeps it behind a lock.
unsafe impl Send for Reader {}

/// The channels of one IOReport group (and subgroup), owned.
fn group(name: &CStr, subgroup: Option<&CStr>) -> Option<Owned> {
    let name = cf_string(name)?;
    let subgroup = match subgroup {
        Some(subgroup) => Some(cf_string(subgroup)?),
        None => None,
    };
    // SAFETY: CF strings live for the call; the result is owned.
    Owned::new(unsafe {
        IOReportCopyChannelsInGroup(
            name.0,
            subgroup.as_ref().map_or(ptr::null(), |s| s.0),
            0,
            0,
            0,
        )
    })
}

impl Reader {
    pub fn open() -> Option<Self> {
        if in_virtual_machine() {
            return None;
        }
        let tables = pmgr_table(c"voltage-states1-sram").zip(pmgr_table(c"voltage-states5-sram"));
        let clocks = tables
            .is_some()
            .then(|| group(c"CPU Stats", Some(c"CPU Complex Performance States")))
            .flatten();
        let energy = group(c"Energy Model", None);
        let channels = match (clocks, energy) {
            (Some(clocks), Some(energy)) => {
                // SAFETY: both are live channel dictionaries; the merge
                // copies `energy` into `clocks` and transfers nothing.
                unsafe { IOReportMergeChannels(clocks.0, energy.0, ptr::null()) };
                clocks
            }
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => return None,
        };
        // SAFETY: IOReport calls with live CF objects; every object created
        // here is owned by an `Owned` and released with the reader.
        unsafe {
            let desired = Owned::new(CFDictionaryCreateMutableCopy(
                kCFAllocatorDefault,
                CFDictionaryGetCount(channels.0),
                channels.0,
            ))?;
            let mut subscribed = ptr::null();
            let subscription = Owned::new(IOReportCreateSubscription(
                ptr::null(),
                desired.0,
                &mut subscribed,
                0,
                ptr::null(),
            ))?;
            Some(Self {
                subscription,
                channels: Owned::new(subscribed)?,
                _desired: desired,
                previous: None,
                tables,
            })
        }
    }

    /// Each cluster's clock and each part's power since the previous call;
    /// nothing on the first.
    pub fn sample(&mut self) -> Reading {
        // SAFETY: the subscription and channels live as long as `self`.
        let Some(current) = Owned::new(unsafe {
            IOReportCreateSamples(self.subscription.0, self.channels.0, ptr::null())
        }) else {
            return Reading::default();
        };
        let now = Instant::now();
        let Some((previous, at)) = self.previous.replace((current, now)) else {
            return Reading::default();
        };
        let seconds = now.duration_since(at).as_secs_f64();
        let current = self
            .previous
            .as_ref()
            .map_or(ptr::null(), |(sample, _)| sample.0);
        // SAFETY: both samples are live; the delta is owned and released below.
        let Some(delta) =
            Owned::new(unsafe { IOReportCreateSamplesDelta(previous.0, current, ptr::null()) })
        else {
            return Reading::default();
        };
        let Some(key) = cf_string(c"IOReportChannels") else {
            return Reading::default();
        };
        let mut reading = Reading::default();
        let mut energy = Vec::new();
        // SAFETY: the channel array and its items are borrowed from `delta`,
        // which outlives the loop; the IOReport getters do not transfer ownership.
        unsafe {
            let items = CFDictionaryGetValue(delta.0, key.0);
            if items.is_null() {
                return reading;
            }
            for index in 0..CFArrayGetCount(items) {
                let item = CFArrayGetValueAtIndex(items, index);
                let Some(name) = rust_string(IOReportChannelGetChannelName(item)) else {
                    continue;
                };
                if rust_string(IOReportChannelGetGroup(item)).as_deref() == Some("Energy Model") {
                    if power_kind(&name).is_some() {
                        let unit =
                            rust_string(IOReportChannelGetUnitLabel(item)).unwrap_or_default();
                        energy.push((
                            name,
                            unit,
                            IOReportSimpleGetIntegerValue(item, ptr::null_mut()),
                        ));
                    }
                    continue;
                }
                let Some((efficiency, performance)) = &self.tables else {
                    continue;
                };
                let (kind, table) = if name.starts_with("ECPU") {
                    ("efficiency", efficiency)
                } else if name.starts_with("PCPU") {
                    ("performance", performance)
                } else {
                    continue;
                };
                let states: Vec<(String, i64)> = (0..IOReportStateGetCount(item))
                    .map(|state| {
                        (
                            rust_string(IOReportStateGetNameForIndex(item, state))
                                .unwrap_or_default(),
                            IOReportStateGetResidency(item, state),
                        )
                    })
                    .collect();
                reading.clusters.push(CpuFrequency {
                    label: name,
                    kind: Some(kind),
                    mhz: running_mhz(&states, table),
                    max_mhz: table.last().copied(),
                });
            }
        }
        reading.power = power_parts(&energy, seconds);
        reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| [value.to_le_bytes(), 800_000u32.to_le_bytes()].concat())
            .collect()
    }

    #[test]
    fn energy_over_time_is_power_in_each_channels_own_unit() {
        let channel = |name: &str, unit: &str, count| (name.to_owned(), unit.to_owned(), count);
        let parts = power_parts(
            &[
                channel("DRAM", "mJ", 500),
                // The GPU counts in nanojoules.
                channel("GPU Energy", "nJ", 2_000_000_000),
                channel("CPU Energy", "mJ", 3_000),
                // A die's CPU total adds to the CPU.
                channel("DIE_1_CPU Energy", "mJ", 1_000),
                channel("ANE0", "uJ", 0),
                // Not a total, and an unknown unit: both left out.
                channel("PCPU0", "mJ", 9_999),
                channel("CPU Energy", "kWh", 1),
            ],
            2.0,
        );
        let watts: Vec<_> = parts.iter().map(|part| (part.kind, part.watts)).collect();
        assert_eq!(
            watts,
            [("cpu", 2.0), ("gpu", 1.0), ("ane", 0.0), ("dram", 0.25)]
        );
        assert!(power_parts(&[channel("CPU Energy", "mJ", 1)], 0.0).is_empty());
    }

    #[test]
    fn voltage_tables_read_in_hertz_or_kilohertz() {
        // An M2's efficiency cluster, in hertz.
        assert_eq!(
            table_mhz(&pairs(&[600_000_000, 2_424_000_000])),
            [600, 2424]
        );
        // Later chips give kilohertz.
        assert_eq!(table_mhz(&pairs(&[1_020_000, 4_512_000])), [1020, 4512]);
        // A trailing half pair is ignored.
        assert_eq!(
            table_mhz(&[pairs(&[600_000_000]), vec![1, 2, 3]].concat()),
            [600]
        );
        assert!(table_mhz(&[]).is_empty());
    }

    #[test]
    fn a_cluster_clock_is_weighted_by_time_running_at_each_step() {
        let table = [600, 1200, 2400];
        let states = |idle, low, mid, high| {
            vec![
                ("IDLE".to_owned(), idle),
                ("DOWN".to_owned(), 0),
                ("V0P0".to_owned(), low),
                ("V1P1".to_owned(), mid),
                ("V2P2".to_owned(), high),
            ]
        };
        // Idle time does not pull the clock down.
        assert_eq!(running_mhz(&states(900, 0, 0, 100), &table), Some(2400));
        assert_eq!(running_mhz(&states(0, 1, 0, 1), &table), Some(1500));
        assert_eq!(running_mhz(&states(10, 1, 2, 1), &table), Some(1350));
        // A cluster that never ran has no clock rather than a made-up one.
        assert_eq!(running_mhz(&states(100, 0, 0, 0), &table), None);
        // States beyond the table are not guessed at.
        let mut extra = states(0, 0, 0, 1);
        extra.push(("V3P3".to_owned(), 1_000));
        assert_eq!(running_mhz(&extra, &table), Some(2400));
        assert_eq!(running_mhz(&[("IDLE".to_owned(), 5)], &table), None);
    }
}
