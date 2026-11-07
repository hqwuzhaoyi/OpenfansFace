use crate::model::{Error, Result};
use std::{env, path::PathBuf};

#[derive(Clone)]
pub struct Config {
    pub base: PathBuf,
    pub media: PathBuf,
    pub ledger: PathBuf,
    pub bind: String,
    pub token: Option<String>,
    pub fixture: Option<PathBuf>,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        fn required(name: &str) -> Result<PathBuf> {
            let value = env::var(name)
                .map_err(|_| Error::new(503, "configuration", format!("缺少配置 {name}")))?;
            Ok(std::fs::canonicalize(value)?)
        }
        let config = Self {
            base: required("FACES_BASE")?,
            media: required("MEDIA2_ROOT")?,
            ledger: required("PEACH_LEDGER_PATH")?,
            bind: env::var("FILE_OPERATIONS_BIND").unwrap_or("127.0.0.1:8930".into()),
            token: env::var("FILE_OPERATIONS_RUST_TOKEN").ok(),
            fixture: env::var("FILE_OPERATIONS_FIXTURE_ROOT")
                .ok()
                .map(std::fs::canonicalize)
                .transpose()?,
        };
        if !config.base.is_dir() || !config.media.is_dir() || !config.ledger.is_file() {
            return Err(Error::new(
                503,
                "configuration",
                "本地状态目录、媒体根目录和 Peach 数据库必须已存在",
            ));
        }
        // Remote binding is an explicit authenticated deployment decision.
        let address: std::net::SocketAddr = config
            .bind
            .parse()
            .map_err(|_| Error::new(503, "configuration", "监听地址无效"))?;
        if !address.ip().is_loopback() && config.token.as_deref().is_none_or(str::is_empty) {
            return Err(Error::new(
                503,
                "configuration",
                "非本机监听必须配置访问令牌",
            ));
        }
        if let Some(root) = &config.fixture
            && (root.parent() != Some(std::fs::canonicalize("/tmp")?.as_path())
                || !root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("face-admin-person-fileops-")
                || config.base != root.join("base")
                || config.media != root.join("media")
                || config.ledger != root.join("ledger.db")
                || crate::control::read(
                    &root.join(".fileops-fixture"),
                    crate::control::MARKER_LIMIT,
                )?
                .unwrap_or_default()
                    != "person file operations"
                || !address.ip().is_loopback())
        {
            return Err(Error::new(
                503,
                "configuration",
                "故障注入只允许完整位于 /tmp 的标记隔离夹具",
            ));
        }
        Ok(config)
    }
    pub fn creators(&self) -> PathBuf {
        self.media.join("pikpak/creators")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.base.join("file-operations-rust")
    }
    pub fn state_db(&self) -> PathBuf {
        self.state_dir().join("operations.sqlite3")
    }
}
