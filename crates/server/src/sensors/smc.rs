//! Fan speeds from the System Management Controller on macOS.
//!
//! The SMC is a key-value store behind the `AppleSMC` IOKit service, readable
//! without privileges. Each key is four characters and its value comes typed:
//! `FNum` is the fan count and `F{n}Ac`, `F{n}Mn` and `F{n}Mx` are the actual,
//! minimum and maximum speed of fan `n`. Apple Silicon stores speeds as
//! little-endian `flt `, Intel as big-endian `fpe2` fixed point. A Mac without
//! fans reports none, or has no `FNum` at all.
use super::Fan;
use std::{
    ffi::{c_char, c_void},
    mem::size_of,
    sync::{Mutex, OnceLock},
};

type MachPort = u32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> MachPort;
    fn IOServiceOpen(service: MachPort, task: MachPort, kind: u32, connect: *mut MachPort) -> i32;
    fn IOServiceClose(connect: MachPort) -> i32;
    fn IOObjectRelease(object: MachPort) -> i32;
    fn IOConnectCallStructMethod(
        connect: MachPort,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> i32;
}

unsafe extern "C" {
    static mach_task_self_: MachPort;
}

/// The one selector the SMC user client takes; the command rides in the
/// struct.
const HANDLE_YPC_EVENT: u32 = 2;
const READ_KEY: u8 = 5;
const GET_KEY_INFO: u8 = 9;

// `SMCKeyData_t` as the kernel lays it out.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Version {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct PowerLimits {
    version: u16,
    length: u16,
    cpu: u32,
    gpu: u32,
    memory: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct KeyInfo {
    size: u32,
    kind: u32,
    attributes: u8,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct KeyData {
    key: u32,
    version: Version,
    limits: PowerLimits,
    info: KeyInfo,
    result: u8,
    status: u8,
    command: u8,
    data32: u32,
    bytes: [u8; 32],
}

const _: () = assert!(size_of::<KeyData>() == 80);

struct Smc(MachPort);

impl Smc {
    fn open() -> Option<Self> {
        // SAFETY: plain IOKit calls; `IOServiceGetMatchingService` consumes the
        // matching dictionary, and the service is released once opened.
        unsafe {
            let matching = IOServiceMatching(c"AppleSMC".as_ptr());
            if matching.is_null() {
                return None;
            }
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return None;
            }
            let mut connect = 0;
            let opened = IOServiceOpen(service, mach_task_self_, 0, &mut connect);
            IOObjectRelease(service);
            (opened == 0).then_some(Self(connect))
        }
    }

    fn call(&self, input: &KeyData) -> Option<KeyData> {
        let mut output = KeyData::default();
        let mut output_size = size_of::<KeyData>();
        // SAFETY: both buffers are `KeyData`-sized and live for the call.
        let status = unsafe {
            IOConnectCallStructMethod(
                self.0,
                HANDLE_YPC_EVENT,
                (input as *const KeyData).cast(),
                size_of::<KeyData>(),
                (&mut output as *mut KeyData).cast(),
                &mut output_size,
            )
        };
        (status == 0 && output.result == 0).then_some(output)
    }

    fn number(&self, key: &str) -> Option<f32> {
        let key = u32::from_be_bytes(key.as_bytes().try_into().ok()?);
        let info = self
            .call(&KeyData {
                key,
                command: GET_KEY_INFO,
                ..Default::default()
            })?
            .info;
        let data = self.call(&KeyData {
            key,
            info: KeyInfo {
                size: info.size,
                ..Default::default()
            },
            command: READ_KEY,
            ..Default::default()
        })?;
        let size = (info.size as usize).min(data.bytes.len());
        decode(&info.kind.to_be_bytes(), &data.bytes[..size])
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        // SAFETY: closes the connection this value opened.
        unsafe { IOServiceClose(self.0) };
    }
}

/// A non-negative number from an SMC value of a type a speed or count uses.
fn decode(kind: &[u8; 4], bytes: &[u8]) -> Option<f32> {
    let value = match (kind, bytes) {
        (b"flt ", [a, b, c, d]) => f32::from_le_bytes([*a, *b, *c, *d]),
        (b"fpe2", [a, b]) => f32::from(u16::from_be_bytes([*a, *b])) / 4.0,
        (b"ui8 ", [a]) => f32::from(*a),
        (b"ui16", [a, b]) => f32::from(u16::from_be_bytes([*a, *b])),
        (b"ui32", [a, b, c, d]) => u32::from_be_bytes([*a, *b, *c, *d]) as f32,
        _ => return None,
    };
    (value.is_finite() && value >= 0.0).then_some(value)
}

/// More than any Mac has; guards against reading garbage as a count.
const MOST_FANS: u32 = 8;

/// Run `read` against the one SMC connection. Opened once and kept: a
/// connection is cheap to hold, and a Mac whose SMC cannot be opened is not
/// asked again every second.
fn with_smc<T>(read: impl FnOnce(&Smc) -> T) -> Option<T> {
    static SMC: OnceLock<Option<Mutex<Smc>>> = OnceLock::new();
    let smc = SMC.get_or_init(|| Smc::open().map(Mutex::new)).as_ref()?;
    Some(read(&smc.lock().expect("smc connection")))
}

/// The whole machine's draw in watts, from `PSTR`, which Apple Silicon and
/// recent Intel Macs keep. A reading outside any Mac's range is a key that
/// means something else on this model.
pub fn system_power() -> Option<f32> {
    with_smc(|smc| smc.number("PSTR"))
        .flatten()
        .filter(|watts| *watts > 0.0 && *watts < 2_000.0)
}

pub fn fans() -> Vec<Fan> {
    with_smc(read_fans).unwrap_or_default()
}

fn read_fans(smc: &Smc) -> Vec<Fan> {
    let count = smc.number("FNum").map_or(0, |n| n as u32).min(MOST_FANS);
    let rpm = |key: String| smc.number(&key).map(|value| value.round() as u32);
    (0..count)
        .filter_map(|n| {
            Some(Fan {
                label: format!("Fan {}", n + 1),
                rpm: rpm(format!("F{n}Ac"))?,
                min_rpm: rpm(format!("F{n}Mn")),
                max_rpm: rpm(format!("F{n}Mx")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_decode_from_both_architectures() {
        assert_eq!(decode(b"flt ", &1234.5f32.to_le_bytes()), Some(1234.5));
        // 2000 rpm in 14.2 fixed point.
        assert_eq!(decode(b"fpe2", &(2000u16 * 4).to_be_bytes()), Some(2000.0));
        assert_eq!(decode(b"ui8 ", &[2]), Some(2.0));
        assert_eq!(decode(b"ui16", &[0x01, 0x00]), Some(256.0));
        assert_eq!(decode(b"flt ", &f32::NAN.to_le_bytes()), None);
        assert_eq!(decode(b"flt ", &(-1.0f32).to_le_bytes()), None);
        assert_eq!(decode(b"flt ", &[0, 0]), None, "wrong size for the type");
        assert_eq!(decode(b"ch8*", b"abcd"), None);
    }
}
