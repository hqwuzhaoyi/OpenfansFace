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
