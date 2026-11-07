use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub error: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_outcome: Option<String>,
}
impl Error {
    pub fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            error: message.into(),
            path: None,
            step: None,
            mutation_outcome: None,
        }
    }
    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
    pub fn during(mut self, step: impl Into<String>) -> Self {
        self.step = Some(step.into());
        self
    }
    pub fn ambiguous_mutation(mut self) -> Self {
        self.mutation_outcome = Some("unknown".into());
        self
    }
    pub fn mutation(e: std::io::Error) -> Self {
        let ambiguous = [
            rustix::io::Errno::IO,
            rustix::io::Errno::STALE,
            rustix::io::Errno::TIMEDOUT,
        ]
        .iter()
        .any(|errno| e.raw_os_error() == Some(errno.raw_os_error()));
        let error = Self::from(e);
        if ambiguous {
            error.ambiguous_mutation()
        } else {
            error
        }
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(409, "stale_preview", message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        let (status, code) = match e.kind() {
            std::io::ErrorKind::PermissionDenied => (403, "permission_denied"),
            std::io::ErrorKind::AlreadyExists => (409, "target_exists"),
            std::io::ErrorKind::NotFound => (409, "source_changed"),
            _ => (503, "storage_unavailable"),
        };
        Self::new(status, code, format!("文件存储操作失败：{e}"))
    }
}
impl From<rustix::io::Errno> for Error {
    fn from(e: rustix::io::Errno) -> Self {
        if e == rustix::io::Errno::LOOP || e == rustix::io::Errno::NOTDIR {
            return Self::new(409, "unsafe_path", "路径包含符号链接或非目录组件");
        }
        std::io::Error::from_raw_os_error(e.raw_os_error()).into()
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::new(503, "database_unavailable", format!("数据库操作失败：{e}"))
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new(400, "invalid_request", format!("数据格式无效：{e}"))
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            Json(self),
        )
            .into_response()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SelectedFile {
    pub vid: String,
    pub path: String,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub action: String,
    pub person: String,
    #[serde(default)]
    pub files: Vec<SelectedFile>,
    #[serde(default)]
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(alias = "new_name")]
    pub target_person: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub mode: String,
    pub request: Request,
    pub version: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMove {
    pub vids: Vec<String>,
    pub src: String,
    pub dst: String,
    pub size: u64,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub request: Request,
    pub version: String,
    pub action: String,
    pub person: String,
    pub target_person: String,
    pub files: Vec<FileMove>,
    pub total_bytes: u64,
    pub directories: Vec<String>,
    pub ledger_version: String,
    pub metadata_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionIntent>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScopeRoot {
    Person(String),
    Unorganized,
    Staging,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveScope {
    pub source: ScopeRoot,
    pub destination: ScopeRoot,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FailurePolicy {
    Rollback,
    ContinueUnchanged,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectedStores {
    pub faces: bool,
    pub jobs: bool,
    pub ledger: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionIntent {
    pub scopes: Vec<MoveScope>,
    pub failure_policy: FailurePolicy,
    pub stores: AffectedStores,
    pub data: Value,
    pub result: Value,
}
impl Plan {
    pub fn file(&self) -> &FileMove {
        &self.files[0]
    }
}
pub fn hash(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable intent"))
    )
}
pub fn validate_id(id: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(id)
        .map_err(|_| Error::new(400, "invalid_request", "操作编号无效"))?;
    if parsed.get_version_num() != 4
        || parsed.get_variant() != uuid::Variant::RFC4122
        || id.len() != 36
    {
        return Err(Error::new(400, "invalid_request", "操作编号必须是 UUID v4"));
    }
    Ok(())
}
pub fn final_result(plan: &Plan) -> Value {
    json!({"ok":true,"id":plan.id,"person":plan.person,"changed":plan.files.len(),"staged":false})
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    #[test]
    fn only_actual_ambiguous_write_errors_have_unknown_outcome() {
        for errno in [
            rustix::io::Errno::IO,
            rustix::io::Errno::STALE,
            rustix::io::Errno::TIMEDOUT,
        ] {
            assert!(Error::from(errno).mutation_outcome.is_none());
            let error = Error::mutation(std::io::Error::from_raw_os_error(errno.raw_os_error()));
            assert_eq!(error.mutation_outcome.as_deref(), Some("unknown"));
            let restored: Error =
                serde_json::from_value(serde_json::to_value(error).unwrap()).unwrap();
            assert_eq!(restored.mutation_outcome.as_deref(), Some("unknown"));
        }
        for errno in [
            rustix::io::Errno::ACCESS,
            rustix::io::Errno::PERM,
            rustix::io::Errno::EXIST,
            rustix::io::Errno::NOENT,
        ] {
            assert!(
                Error::mutation(std::io::Error::from_raw_os_error(errno.raw_os_error()))
                    .mutation_outcome
                    .is_none()
            );
        }
        let old: Error = serde_json::from_value(
            json!({"code":"storage_unavailable","error":"old receipt","status":503,"path":null}),
        )
        .unwrap();
        assert!(old.mutation_outcome.is_none());
    }
}
