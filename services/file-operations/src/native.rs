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
    crate::model::validate_id(&request.id)?;
    if ![
        "archive",
        "queue-confirm",
        "queue-undo",
        "claim-decide",
        "queue-rule",
        "queue-restore",
    ]
    .contains(&request.action.as_str())
    {
        return Err(Error::new(400, "unsupported_scope", "操作未启用"));
    }
    if !request.files.is_empty() {
        return Err(Error::new(400, "invalid_request", "范围必须由原生计划确定"));
    }
    Ok(())
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    if request.action == "archive" {
        crate::archive::plan(config, request)
    } else if [
        "queue-confirm",
        "queue-undo",
        "claim-decide",
        "queue-rule",
        "queue-restore",
    ]
    .contains(&request.action.as_str())
    {
        crate::review::plan(config, request)
    } else {
        Err(Error::new(409, "unsupported_scope", "此原生操作尚未启用"))
    }
}
pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    if plan.action == "archive" {
        crate::archive::snapshot(config, plan, successful)
    } else {
        crate::review::snapshot(config, plan, successful)
    }
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    if plan.action == "archive" {
        crate::archive::revalidate(config, plan)
    } else {
        crate::review::revalidate(config, plan)
    }
}
pub fn result(
    plan: &Plan,
    successful: &[usize],
    failures: &[serde_json::Value],
) -> serde_json::Value {
    if plan.action == "archive" {
        crate::archive::result(plan, successful, failures)
    } else {
        let mut result = plan.execution.as_ref().unwrap().result.clone();
        result["id"] = serde_json::json!(plan.id);
        result["ok"] = serde_json::json!(failures.is_empty());
        result["changed"] = serde_json::json!(successful.len());
        result["successful_file_indexes"] = serde_json::json!(successful);
        result["failures"] = serde_json::json!(failures);
        result
    }
}
