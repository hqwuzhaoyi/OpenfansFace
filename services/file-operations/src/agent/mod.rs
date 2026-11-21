pub mod cleanup;
pub mod config;
pub mod outbox;
pub mod probe;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuCapability {
    pub model: String,
    pub vram_bytes: i64,
    pub device_uuid: String,
    pub is_unified_memory: bool,
    pub driver_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemCapabilities {
    pub hostname: String,
    pub host_machine_id: String,
    pub os: String,
    pub arch: String,
    pub cpu_cores: i32,
    pub ram_bytes: i64,
    pub gpu: Option<GpuCapability>,
    pub execution_providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySample {
    pub free_ram_bytes: i64,
    pub free_vram_bytes: i64,
    pub gpu_util_percent: f64,
}
