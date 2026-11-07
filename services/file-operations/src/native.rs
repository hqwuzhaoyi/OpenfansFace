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
fn review_action(action: &str) -> bool {
    [
        "archive-resolve",
        "queue-confirm",
        "queue-undo",
        "claim-decide",
        "queue-rule",
        "queue-restore",
    ]
    .contains(&action)
}
pub fn validate(request: &Request) -> Result<()> {
    crate::model::validate_id(&request.id)?;
    match request.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::validate(request),
        action if action == "archive" || action == "metadata-write" || review_action(action) => {
            if !request.files.is_empty() {
                return Err(Error::new(400, "invalid_request", "范围必须由原生计划确定"));
            }
            Ok(())
        }
        "delete-files" | "restore-files" => crate::staging::validate(request),
        "rename-person" => crate::person_rename::validate(request),
        "merge-person" => crate::person_merge::validate(request),
        "delete-person" => crate::person_delete::validate(request),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
    }
}
pub fn plan(config: &Config, request: Request) -> Result<Plan> {
    match request.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::plan(config, request),
        "archive" => crate::archive::plan(config, request),
        "metadata-write" => crate::shared_metadata::plan(config, request),
        action if review_action(action) => crate::review::plan(config, request),
        "delete-files" | "restore-files" => crate::staging::plan(config, request),
        "rename-person" => crate::person_rename::plan(config, request),
        "merge-person" => crate::person_merge::plan(config, request),
        "delete-person" => crate::person_delete::plan(config, request),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
    }
}
pub fn snapshot(config: &Config, plan: &Plan, successful: &[usize]) -> Result<DomainSnapshot> {
    match plan.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::snapshot(config, plan, successful),
        "archive" => crate::archive::snapshot(config, plan, successful),
        "metadata-write" => crate::shared_metadata::snapshot(config, plan, successful),
        action if review_action(action) => crate::review::snapshot(config, plan, successful),
        "delete-files" | "restore-files" => crate::staging::snapshot(config, plan, successful),
        "rename-person" => crate::person_rename::snapshot(config, plan, successful),
        "merge-person" => crate::person_merge::snapshot(config, plan, successful),
        "delete-person" => crate::person_delete::snapshot(config, plan, successful),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
    }
}
pub fn revalidate(config: &Config, plan: &Plan) -> Result<()> {
    match plan.action.as_str() {
        "move-files" | "rename-file" => crate::person_move::revalidate(config, plan),
        "archive" => crate::archive::revalidate(config, plan),
        "metadata-write" => crate::shared_metadata::revalidate(config, plan),
        action if review_action(action) => crate::review::revalidate(config, plan),
        "delete-files" | "restore-files" => crate::staging::revalidate(config, plan),
        "rename-person" => crate::person_rename::revalidate(config, plan),
        "merge-person" => crate::person_merge::revalidate(config, plan),
        "delete-person" => crate::person_delete::revalidate(config, plan),
        _ => Err(Error::new(400, "unsupported_scope", "此原生操作尚未启用")),
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
        let mut result = plan
            .execution
            .as_ref()
            .map(|e| e.result.clone())
            .unwrap_or_else(|| serde_json::json!({}));
        result["id"] = serde_json::json!(plan.id);
        result["ok"] = serde_json::json!(failures.is_empty());
        result["changed"] = serde_json::json!(successful.len());
        result["successful_file_indexes"] = serde_json::json!(successful);
        result["failures"] = serde_json::json!(failures);
        result
    }
}
