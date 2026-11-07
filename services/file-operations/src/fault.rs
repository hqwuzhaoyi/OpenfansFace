//! Process-level acceptance hooks; unavailable unless all data roots pass the marked fixture guard.
use crate::{
    config::Config,
    files::read_optional,
    model::{Error, Result},
};
pub fn checkpoint(config: &Config, phase: &str) -> Result<()> {
    let Some(root) = &config.fixture else {
        return Ok(());
    };
    let Some(raw) = read_optional(&root.join("rust-fault.json"))? else {
        return Ok(());
    };
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    if value["phase"] != phase {
        return Ok(());
    }
    match value["mode"].as_str() {
        Some("exit") => std::process::exit(86),
        Some("error") => Err(Error::new(
            503,
            "fixture_fault",
            format!("隔离夹具故障：{phase}"),
        )),
        Some("io_error") => {
            let errno = match value["errno"].as_str() {
                Some("EIO") => rustix::io::Errno::IO,
                Some("ESTALE") => rustix::io::Errno::STALE,
                Some("ETIMEDOUT") => rustix::io::Errno::TIMEDOUT,
                _ => return Err(Error::new(400, "fixture_fault", "未知 I/O 故障")),
            };
            Err(Error::from(errno).during(phase))
        }
        Some("pause") => {
            crate::files::atomic_write(
                &root.join("rust-fault-reached.json"),
                serde_json::to_string(&serde_json::json!({"phase":phase}))?.as_bytes(),
            )?;
            std::thread::sleep(std::time::Duration::from_millis(
                value["milliseconds"].as_u64().unwrap_or(500).min(10_000),
            ));
            Ok(())
        }
        _ => Err(Error::new(400, "fixture_fault", "未知故障注入模式")),
    }
}

/// These modes model a syscall response, not an HTTP waiter timeout. Only marked
/// fixture configurations can supply a root. Delayed effects are released by the
/// acceptance harness after the service has returned the ambiguous response.
pub fn mutation_error(root: Option<&std::path::Path>, step: &str, after: bool) -> Result<()> {
    let Some(root) = root else {
        return Ok(());
    };
    let Some(raw) = read_optional(&root.join("rust-fault.json"))? else {
        return Ok(());
    };
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    if value["phase"] != format!("mutation:{step}") {
        return Ok(());
    }
    let expected = if after {
        "mutation_error_after"
    } else {
        "mutation_error_delayed"
    };
    if value["mode"] != expected {
        return Ok(());
    }
    let errno = match value["errno"].as_str() {
        Some("EIO") => rustix::io::Errno::IO,
        Some("ESTALE") => rustix::io::Errno::STALE,
        Some("ETIMEDOUT") => rustix::io::Errno::TIMEDOUT,
        _ => return Err(Error::new(400, "fixture_fault", "未知变更 I/O 故障")),
    };
    Err(Error::mutation(std::io::Error::from_raw_os_error(errno.raw_os_error())).during(step))
}

/// Exercise the real bounded-copy/recovery path on same-device isolated fixtures.
/// This switch is ignored outside the full Config fixture guard.
pub fn copy_required(config: &Config) -> Result<bool> {
    let Some(root) = &config.fixture else {
        return Ok(false);
    };
    let Some(raw) = read_optional(&root.join("rust-fault.json"))? else {
        return Ok(false);
    };
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    Ok(value["force_copy"] == true)
}
