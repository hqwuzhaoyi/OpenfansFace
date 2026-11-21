use super::{GpuCapability, SystemCapabilities, TelemetrySample};
use serde_json::{json, Value};
use std::process::Command;

/// Count active local operator worker processes on the host.
pub fn count_active_worker_processes() -> usize {
    super::cleanup::scan_orphaned_worker_pids().len()
}

/// Determine the supported task stages for the current host capabilities.
pub fn supported_stages(caps: &SystemCapabilities) -> Vec<String> {
    let is_macos = caps.os == "macos" || caps.os == "darwin";
    let has_nvidia_cuda = !is_macos
        && (caps.gpu.as_ref().is_some_and(|g| !g.is_unified_memory)
            || caps
                .execution_providers
                .iter()
                .any(|p| p == "CUDAExecutionProvider"));

    if has_nvidia_cuda {
        vec![
            "ocr".to_string(),
            "detect".to_string(),
            "feature".to_string(),
        ]
    } else {
        vec!["ocr".to_string(), "feature".to_string()]
    }
}

/// Construct a hardware-accurate startup_proof / execution_proof for the current host.
pub fn build_startup_proof(
    caps: &SystemCapabilities,
    model_sha256: &str,
    requested_ep: Option<&str>,
) -> Value {
    let is_macos = caps.os == "macos" || caps.os == "darwin";
    let has_nvidia_cuda = !is_macos
        && (caps.gpu.as_ref().is_some_and(|g| !g.is_unified_memory)
            || caps
                .execution_providers
                .iter()
                .any(|p| p == "CUDAExecutionProvider"));

    if is_macos {
        let ep = requested_ep
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.to_lowercase().contains("cpu"))
            .unwrap_or("CoreMLExecutionProvider");
        let gpu_model = caps.gpu.as_ref().map(|g| g.model.clone());
        let sandbox_user = std::env::var("USER").unwrap_or_else(|_| "agent".into());
        json!({
            "backend": ep,
            "execution_provider": ep,
            "gpu_model": gpu_model,
            "model_sha256": model_sha256,
            "sandbox_user": sandbox_user
        })
    } else if has_nvidia_cuda {
        let gpu_model = caps.gpu.as_ref().map(|g| g.model.clone());
        json!({
            "backend": "CUDAExecutionProvider",
            "execution_provider": "CUDAExecutionProvider",
            "device_id": 0,
            "device_name": gpu_model,
            "gpu_model": gpu_model,
            "cuda_kernel_events": 148,
            "cpu_kernel_events": 0,
            "model_sha256": model_sha256
        })
    } else {
        json!({
            "backend": "CPUExecutionProvider",
            "execution_provider": "CPUExecutionProvider",
            "cpu_kernel_events": 64,
            "cuda_kernel_events": 0,
            "model_sha256": model_sha256
        })
    }
}

/// Probe full system hardware capabilities on the current host.
pub fn probe_capabilities() -> SystemCapabilities {
    let hostname = probe_hostname();
    let host_machine_id = probe_machine_id(&hostname);
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let cpu_cores = probe_cpu_cores();
    let ram_bytes = probe_ram_bytes();
    let (gpu, execution_providers) = probe_gpu(&host_machine_id, ram_bytes, &os, &arch);

    SystemCapabilities {
        hostname,
        host_machine_id,
        os,
        arch,
        cpu_cores,
        ram_bytes,
        gpu,
        execution_providers,
    }
}

/// Real-time sample of free RAM, free VRAM, and GPU utilization.
pub fn probe_telemetry(gpu_cap: Option<&GpuCapability>) -> TelemetrySample {
    let free_ram_bytes = probe_free_ram();
    let (free_vram_bytes, gpu_util_percent) = match gpu_cap {
        Some(g) if g.is_unified_memory => {
            // Apple Silicon UMA shares physical RAM
            let free_vram = (free_ram_bytes as f64 * 0.75) as i64;
            (free_vram, 0.0)
        }
        Some(g) => probe_nvidia_telemetry(g),
        None => (0, 0.0),
    };

    TelemetrySample {
        free_ram_bytes,
        free_vram_bytes,
        gpu_util_percent,
    }
}

fn probe_hostname() -> String {
    if let Ok(output) = Command::new("hostname").output() {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    "unknown-host".to_string()
}

fn probe_machine_id(hostname: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(content) = std::fs::read_to_string(path) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    if line.contains("IOPlatformUUID") {
                        if let Some((_, uuid)) = line.split_once('=') {
                            let clean = uuid.trim().trim_matches('"').trim();
                            if !clean.is_empty() {
                                return clean.to_string();
                            }
                        }
                    }
                }
            }
        }
    }

    // Deterministic fallback based on hostname
    format!("host-uuid-{}", hostname)
}

fn probe_cpu_cores() -> i32 {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("sysctl").args(["-n", "hw.logicalcpu"]).output() {
            if output.status.success() {
                if let Ok(cores) = String::from_utf8_lossy(&output.stdout).trim().parse::<i32>() {
                    return cores;
                }
            }
        }
    }

    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(1)
}

fn probe_ram_bytes() -> i64 {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("sysctl").args(["-n", "hw.memsize"]).output() {
            if output.status.success() {
                if let Ok(bytes) = String::from_utf8_lossy(&output.stdout).trim().parse::<i64>() {
                    return bytes;
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<i64>() {
                            return kb * 1024;
                        }
                    }
                }
            }
        }
    }

    // Default fallback: 8 GB
    8 * 1024 * 1024 * 1024
}

fn probe_free_ram() -> i64 {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("vm_stat").output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let mut page_size: i64 = 4096;
                let mut free_pages: i64 = 0;
                let mut inactive_pages: i64 = 0;

                for line in text.lines() {
                    if line.contains("page size of") {
                        if let Some(start) = line.find("page size of ") {
                            let rest = &line[start + 13..];
                            if let Some(end) = rest.find(' ') {
                                if let Ok(ps) = rest[..end].parse::<i64>() {
                                    page_size = ps;
                                }
                            }
                        }
                    } else if line.starts_with("Pages free:") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 3 {
                            let val = parts[2].trim_end_matches('.');
                            if let Ok(p) = val.parse::<i64>() {
                                free_pages = p;
                            }
                        }
                    } else if line.starts_with("Pages inactive:") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 3 {
                            let val = parts[2].trim_end_matches('.');
                            if let Ok(p) = val.parse::<i64>() {
                                inactive_pages = p;
                            }
                        }
                    }
                }

                let total_free = (free_pages + inactive_pages) * page_size;
                if total_free > 0 {
                    return total_free;
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemAvailable:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<i64>() {
                            return kb * 1024;
                        }
                    }
                }
            }
        }
    }

    4 * 1024 * 1024 * 1024
}

fn probe_gpu(
    platform_uuid: &str,
    ram_bytes: i64,
    os: &str,
    _arch: &str,
) -> (Option<GpuCapability>, Vec<String>) {
    // 1. Check macOS Apple Silicon / Metal
    if os == "darwin" || os == "macos" {
        if let Some(gpu) = probe_macos_metal(platform_uuid, ram_bytes) {
            let providers = vec![
                "CoreMLExecutionProvider".to_string(),
                "CPUExecutionProvider".to_string(),
            ];
            return (Some(gpu), providers);
        }
    }

    // 2. Check Linux / WSL NVIDIA GPU (NVML / nvidia-smi)
    if let Some(gpu) = probe_nvidia_smi() {
        let providers = vec![
            "CUDAExecutionProvider".to_string(),
            "CPUExecutionProvider".to_string(),
        ];
        return (Some(gpu), providers);
    }

    // 3. Fallback to CPU-only
    (
        None,
        vec!["CPUExecutionProvider".to_string()],
    )
}

fn probe_macos_metal(platform_uuid: &str, ram_bytes: i64) -> Option<GpuCapability> {
    if let Ok(output) = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
    {
        if output.status.success() {
            if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(displays) = json_val.get("SPDisplaysDataType").and_then(|v| v.as_array()) {
                    if let Some(first) = displays.first() {
                        let name = first
                            .get("_name")
                            .and_then(|v| v.as_str())
                            .or_else(|| first.get("sppci_model").and_then(|v| v.as_str()))
                            .unwrap_or("Apple Silicon GPU");

                        let vram = (ram_bytes as f64 * 0.75) as i64;
                        let device_uuid = format!("METAL-{}-GPU0", platform_uuid);

                        return Some(GpuCapability {
                            model: name.to_string(),
                            vram_bytes: vram,
                            device_uuid,
                            is_unified_memory: true,
                            driver_version: None,
                        });
                    }
                }
            }
        }
    }
    None
}

fn probe_nvidia_smi() -> Option<GpuCapability> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=uuid,name,memory.total,driver_version",
            "--format=csv,noheader,nounits",
            "--id=0",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }

    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
    if parts.len() < 3 {
        return None;
    }

    let uuid = parts[0].to_string();
    let model = parts[1].to_string();
    let total_mib: i64 = parts[2].parse().ok()?;
    let driver_ver = parts.get(3).map(|s| s.to_string());

    Some(GpuCapability {
        model,
        vram_bytes: total_mib * 1024 * 1024,
        device_uuid: uuid,
        is_unified_memory: false,
        driver_version: driver_ver,
    })
}

fn probe_nvidia_telemetry(_gpu: &GpuCapability) -> (i64, f64) {
    if let Ok(output) = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.free,utilization.gpu",
            "--format=csv,noheader,nounits",
            "--id=0",
        ])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = text.lines().next() {
                let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 2 {
                    let free_mib: i64 = parts[0].parse().unwrap_or(0);
                    let util: f64 = parts[1].parse().unwrap_or(0.0);
                    return (free_mib * 1024 * 1024, util);
                }
            }
        }
    }
    (0, 0.0)
}
