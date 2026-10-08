//! The GPU on Apple Silicon, from its driver's own statistics.
//!
//! The AGX accelerator publishes `PerformanceStatistics` in the IO registry,
//! readable without privileges: how busy the GPU is (what Activity Monitor
//! shows) and how much of the shared memory it holds. The clock comes from
//! IOReport and the power from the chip's energy counters, both passed in.
use super::{
    Gpu,
    cf::{dictionary_number, is_dictionary, number, rust_string, service_property},
};
use std::sync::OnceLock;

/// The chip's name and the GPU's core count, which do not change while running.
fn identity() -> &'static (String, Option<u32>) {
    static IDENTITY: OnceLock<(String, Option<u32>)> = OnceLock::new();
    IDENTITY.get_or_init(|| {
        let model =
            service_property(c"AGXAccelerator", c"model").and_then(|value| rust_string(value.0));
        let cores = service_property(c"AGXAccelerator", c"gpu-core-count")
            .and_then(|value| number(value.0))
            .map(|cores| cores as u32);
        (
            model.map_or_else(|| "GPU".into(), |model| format!("{model} GPU")),
            cores,
        )
    })
}

pub fn gpu(mhz: Option<u32>, max_mhz: Option<u32>, watts: Option<f32>) -> Option<Gpu> {
    let statistics = service_property(c"AGXAccelerator", c"PerformanceStatistics")?;
    if !is_dictionary(statistics.0) {
        return None;
    }
    let (name, cores) = identity();
    Some(Gpu {
        name: name.clone(),
        cores: *cores,
        utilization_percent: dictionary_number(statistics.0, c"Device Utilization %")
            .map(|percent| percent.clamp(0.0, 100.0) as f32),
        memory_used_bytes: dictionary_number(statistics.0, c"In use system memory")
            .map(|bytes| bytes.max(0.0) as u64),
        // Its memory is the machine's: there is no separate total.
        memory_total_bytes: None,
        mhz,
        max_mhz,
        watts,
        celsius: None,
    })
}
