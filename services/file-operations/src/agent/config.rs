use crate::model::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub schema_version: u32,
    pub center_url: String,
    pub node_id: String,
    pub node_token: String,
    pub device_group_id: String,
    pub enrolled_at: u64,
}

impl AgentConfig {
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("FACE_AGENT_CONFIG") {
            if !path.trim().is_empty() {
                return PathBuf::from(path.trim());
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".openfans/agent.json");
        }
        PathBuf::from("/etc/face-agent/agent.json")
    }

    /// Load existing config from disk with strict permission check.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::new(
                404,
                "config_not_found",
                format!("Agent 配置文件不存在: {}", path.display()),
            ));
        }

        // Verify strict 0600 permissions on Unix
        #[cfg(unix)]
        {
            let metadata = fs::metadata(path).map_err(|e| {
                Error::new(
                    500,
                    "io_error",
                    format!("读取配置文件元数据失败 {}: {e}", path.display()),
                )
            })?;
            let mode = metadata.permissions().mode();
            if mode & 0o077 != 0 {
                return Err(Error::new(
                    403,
                    "insecure_config_permissions",
                    format!(
                        "Agent 配置文件 {} 权限过宽 (mode {:o})，必须为 0600 (-rw-------)",
                        path.display(),
                        mode & 0o777
                    ),
                ));
            }
        }

        let content = fs::read_to_string(path).map_err(|e| {
            Error::new(
                500,
                "io_error",
                format!("读取配置文件失败 {}: {e}", path.display()),
            )
        })?;

        let parsed: Self = serde_json::from_str(&content).map_err(|e| {
            Error::new(
                500,
                "invalid_config",
                format!("解析 Agent 配置文件失败 {}: {e}", path.display()),
            )
        })?;

        if parsed.schema_version != 1 {
            return Err(Error::new(
                500,
                "unsupported_schema",
                format!(
                    "不支持的 Agent 配置文件版本: {}",
                    parsed.schema_version
                ),
            ));
        }

        Ok(parsed)
    }

    /// Save config to disk atomically with strict 0600 permissions.
    /// Enrollment tokens MUST NOT be included or stored.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));

        // Ensure parent directory exists with 0700 mode
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::new(
                    500,
                    "io_error",
                    format!("创建目录失败 {}: {e}", parent.display()),
                )
            })?;

            #[cfg(unix)]
            {
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }

        let json_bytes = serde_json::to_vec_pretty(self).map_err(|e| {
            Error::new(500, "json_error", format!("序列化配置失败: {e}"))
        })?;

        let temp_path = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4().simple()));

        #[cfg(unix)]
        let mut file = {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true).mode(0o600);
            opts.open(&temp_path).map_err(|e| {
                Error::new(
                    500,
                    "io_error",
                    format!("打开临时文件失败 {}: {e}", temp_path.display()),
                )
            })?
        };

        #[cfg(not(unix))]
        let mut file = fs::File::create(&temp_path).map_err(|e| {
            Error::new(
                500,
                "io_error",
                format!("打开临时文件失败 {}: {e}", temp_path.display()),
            )
        })?;

        file.write_all(&json_bytes).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            Error::new(500, "io_error", format!("写入配置临时文件失败: {e}"))
        })?;

        file.sync_all().map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            Error::new(500, "io_error", format!("同步配置临时文件失败: {e}"))
        })?;

        drop(file);

        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            Error::new(
                500,
                "io_error",
                format!("原子重命名失败 {} -> {}: {e}", temp_path.display(), path.display()),
            )
        })?;

        #[cfg(unix)]
        {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }
}
