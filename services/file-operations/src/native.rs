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

pub fn validate(request: &Request) -> Result<()> {
    match request.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::validate(request),
        "rename-person" => crate::person_rename::validate(request),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
    }
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    match request.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::plan(config, request),
        "rename-person" => crate::person_rename::plan(config, request),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
    }
}
pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    match plan.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::snapshot(config, plan, successful),
        "rename-person" => crate::person_rename::snapshot(config, plan, successful),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
    }
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    match plan.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::revalidate(config, plan),
        "rename-person" => crate::person_rename::revalidate(config, plan),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
    }
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
