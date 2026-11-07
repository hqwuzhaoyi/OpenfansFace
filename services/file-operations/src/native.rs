//! Closed native action dispatch. Feature modules wire only known application actions here.
//! No client supplied SQL, executors, callbacks, or serialized plugins.
use crate::{
    config::Config,
    model::{Error, Plan, Request, Result},
};
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DomainSnapshot {
    pub metadata: crate::metadata::Snapshot,
    pub ledger: Option<crate::ledger::Delta>,
}

pub fn validate(_request: &Request) -> Result<()> {
    Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用"))
}
pub fn plan(_config: &Config, _request: Request) -> Result<Plan> {
    Err(Error::new(409, "unsupported_scope", "此原生操作尚未启用"))
}
pub fn snapshot(_config: &Config, _plan: &Plan, _successful: &[usize]) -> Result<DomainSnapshot> {
    Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用"))
}
pub fn revalidate(_config: &Config, _plan: &Plan) -> Result<()> {
    Ok(())
}

pub fn result(
    plan: &Plan,
    successful: &[usize],
    failures: &[serde_json::Value],
) -> serde_json::Value {
    let mut result = plan
        .execution
        .as_ref()
        .map(|e| e.result.clone())
        .unwrap_or_else(|| serde_json::json!({}));
    result["id"] = serde_json::json!(plan.id);
    result["ok"] = serde_json::json!(true);
    result["changed"] = serde_json::json!(successful.len());
    result["successful_file_indexes"] = serde_json::json!(successful);
    result["failures"] = serde_json::json!(failures);
    result
}
