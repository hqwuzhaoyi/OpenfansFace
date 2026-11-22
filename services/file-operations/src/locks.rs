//! In-memory active entity lock registry with RAII guards and SSE watch broadcast.
//!
//! All lock reads (`GET /v1/locks`, `GET /v1/locks/stream`) run completely off the single
//! executor lane via an in-memory `tokio::sync::watch` snapshot so UI clients stay responsive
//! even while heavy media/NFS file moves are in flight.
use crate::{
    config::Config,
    model::{Error, Plan, Request, Result, ScopeRoot},
    store::Store,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, watch};

pub const DEFAULT_SSE_KEEPALIVE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityLock {
    pub op_id: String,
    pub action: String,
    pub status: String,
    pub label: String,
    pub started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveOpSummary {
    pub op_id: String,
    pub action: String,
    pub status: String,
    pub label: String,
    pub video_count: usize,
    pub people: Vec<String>,
    pub started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettledSummary {
    pub op_id: String,
    pub action: String,
    pub outcome: String,
    pub people: Vec<String>,
    pub vids: Vec<String>,
    pub settled_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockSnapshot {
    pub revision: u64,
    pub at: u64,
    pub videos: BTreeMap<String, EntityLock>,
    pub people: BTreeMap<String, EntityLock>,
    pub active_ops: Vec<ActiveOpSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_settled: Option<SettledSummary>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64)
}

fn normalize_set(items: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    items
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn status_rank(status: &str) -> u8 {
    match status {
        "recovery_needed" => 3,
        "running" => 2,
        "queued" => 1,
        _ => 0,
    }
}

/// Localized Chinese status label for all write actions and recovery states.
pub fn label_for(action: &str, status: &str) -> &'static str {
    if status == "recovery_needed" {
        return "待恢复";
    }
    let queued = status == "queued";
    match action {
        "auto-archive" | "auto-merge" => {
            if queued {
                "排队合入"
            } else {
                "合入中"
            }
        }
        "archive" => {
            if queued {
                "排队归档"
            } else {
                "归档中"
            }
        }
        "archive-unarchive" => {
            if queued {
                "排队撤回"
            } else {
                "撤回中"
            }
        }
        "delete-person" | "batch-delete" | "delete-files" | "review-delete" => {
            if queued {
                "排队删除"
            } else {
                "删除中"
            }
        }
        "merge-person" | "batch-merge" => {
            if queued {
                "排队合并"
            } else {
                "合并中"
            }
        }
        "rename-person" | "rename-file" => {
            if queued {
                "排队改名"
            } else {
                "改名中"
            }
        }
        "move-files" => {
            if queued {
                "排队移动"
            } else {
                "移动中"
            }
        }
        "restore-files" | "staging-restore" | "review-restore" => {
            if queued {
                "排队恢复"
            } else {
                "恢复中"
            }
        }
        "delete-staged" | "clear-staging" | "staging-delete" => {
            if queued {
                "排队清理"
            } else {
                "清理中"
            }
        }
        "queue-confirm" | "claim-decide" => {
            if queued {
                "排队确认"
            } else {
                "确认中"
            }
        }
        "queue-undo" => {
            if queued {
                "排队撤销"
            } else {
                "撤销中"
            }
        }
        "review-defer" => {
            if queued {
                "排队延后"
            } else {
                "延后中"
            }
        }
        "review-note" => {
            if queued {
                "排队保存"
            } else {
                "保存中"
            }
        }
        "archive-resolve" => {
            if queued {
                "排队处理"
            } else {
                "处理中"
            }
        }
        _ => {
            if queued {
                "排队中"
            } else {
                "处理中"
            }
        }
    }
}

/// Whether an action should publish active entity locks during normal live execution.
/// Background analysis publication (`processing-result`), pure metadata writes, and optional
/// follow-up rule sub-ops are excluded so they don't churn user-facing UI lock badges.
pub fn tracks_action(action: &str) -> bool {
    !matches!(
        action,
        "processing-result" | "metadata-write" | "queue-rule" | "queue-restore"
    )
}

#[derive(Debug, Clone)]
struct ActiveOpEntry {
    op_id: String,
    action: String,
    status: String,
    vids: BTreeSet<String>,
    people: BTreeSet<String>,
    started_at: u64,
    order: u64,
}

#[derive(Debug, Default)]
struct RegistryState {
    revision: u64,
    next_order: u64,
    ops: BTreeMap<String, ActiveOpEntry>,
    last_settled: Option<SettledSummary>,
}

impl RegistryState {
    fn build_snapshot(&self, at: u64) -> LockSnapshot {
        let mut ordered: Vec<&ActiveOpEntry> = self.ops.values().collect();
        ordered.sort_by_key(|entry| (entry.order, entry.started_at, entry.op_id.as_str()));

        let mut videos: BTreeMap<String, (u8, u64, EntityLock)> = BTreeMap::new();
        let mut people: BTreeMap<String, (u8, u64, EntityLock)> = BTreeMap::new();
        let mut active_ops = Vec::with_capacity(ordered.len());

        for entry in ordered {
            let label = label_for(&entry.action, &entry.status).to_owned();
            let rank = status_rank(&entry.status);
            let lock = EntityLock {
                op_id: entry.op_id.clone(),
                action: entry.action.clone(),
                status: entry.status.clone(),
                label: label.clone(),
                started_at: entry.started_at,
            };

            for vid in &entry.vids {
                let replace = match videos.get(vid) {
                    None => true,
                    Some((prev_rank, prev_order, _)) => {
                        rank > *prev_rank || (rank == *prev_rank && entry.order < *prev_order)
                    }
                };
                if replace {
                    videos.insert(vid.clone(), (rank, entry.order, lock.clone()));
                }
            }

            for person in &entry.people {
                let replace = match people.get(person) {
                    None => true,
                    Some((prev_rank, prev_order, _)) => {
                        rank > *prev_rank || (rank == *prev_rank && entry.order < *prev_order)
                    }
                };
                if replace {
                    people.insert(person.clone(), (rank, entry.order, lock.clone()));
                }
            }

            active_ops.push(ActiveOpSummary {
                op_id: entry.op_id.clone(),
                action: entry.action.clone(),
                status: entry.status.clone(),
                label,
                video_count: entry.vids.len(),
                people: entry.people.iter().cloned().collect(),
                started_at: entry.started_at,
            });
        }

        LockSnapshot {
            revision: self.revision,
            at,
            videos: videos.into_iter().map(|(k, (_, _, v))| (k, v)).collect(),
            people: people.into_iter().map(|(k, (_, _, v))| (k, v)).collect(),
            active_ops,
            last_settled: self.last_settled.clone(),
        }
    }
}

pub struct ActiveLockRegistry {
    state: Mutex<RegistryState>,
    tx: watch::Sender<Arc<LockSnapshot>>,
    shutdown_tx: watch::Sender<bool>,
}

impl ActiveLockRegistry {
    pub fn new() -> Arc<Self> {
        let state = RegistryState::default();
        let initial = Arc::new(state.build_snapshot(now_ms()));
        let (tx, _rx) = watch::channel(initial);
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        Arc::new(Self {
            state: Mutex::new(state),
            tx,
            shutdown_tx,
        })
    }

    pub fn snapshot(&self) -> Arc<LockSnapshot> {
        self.tx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<LockSnapshot>> {
        self.tx.subscribe()
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Signal all active SSE stream tasks to close cleanly (used during graceful daemon shutdown).
    pub fn close_streams(&self) {
        let _ = self.shutdown_tx.send_replace(true);
    }

    fn publish_locked(&self, state: &mut RegistryState, at: u64) {
        state.revision = state.revision.saturating_add(1);
        let snap = Arc::new(state.build_snapshot(at));
        let _ = self.tx.send_replace(snap);
    }

    /// Register an admitted operation in `"queued"` state and return an RAII `LockGuard`.
    ///
    /// If the operation is already present in the registry (`queued`, `running`, or
    /// `recovery_needed`), the existing lock is left untouched and the returned guard is
    /// pre-disarmed so dropping or finishing it never clears the existing lock.
    pub fn register_queued(
        self: &Arc<Self>,
        op_id: impl Into<String>,
        action: impl Into<String>,
        vids: impl IntoIterator<Item = String>,
        people: impl IntoIterator<Item = String>,
    ) -> LockGuard {
        let op_id = op_id.into();
        let action = action.into();
        let vids = normalize_set(vids);
        let people = normalize_set(people);
        let at = now_ms();

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.ops.contains_key(&op_id) {
            return LockGuard {
                registry: Arc::clone(self),
                op_id,
                settled: true,
                drop_outcome: "cancelled",
            };
        }

        state.next_order = state.next_order.saturating_add(1);
        let order = state.next_order;
        state.ops.insert(
            op_id.clone(),
            ActiveOpEntry {
                op_id: op_id.clone(),
                action,
                status: "queued".to_owned(),
                vids,
                people,
                started_at: at,
                order,
            },
        );
        self.publish_locked(&mut state, at);
        drop(state);

        LockGuard {
            registry: Arc::clone(self),
            op_id,
            settled: false,
            drop_outcome: "cancelled",
        }
    }

    /// Upgrade an operation to `"running"` with the authoritative planned `vids` and `people`.
    /// Also creates an entry if the operation was invoked directly without prior HTTP queueing.
    pub fn mark_running(
        &self,
        op_id: impl Into<String>,
        action: impl Into<String>,
        vids: impl IntoIterator<Item = String>,
        people: impl IntoIterator<Item = String>,
    ) {
        let op_id = op_id.into();
        let action = action.into();
        let vids = normalize_set(vids);
        let people = normalize_set(people);
        let at = now_ms();

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (started_at, order, resolved_action) = if let Some(existing) = state.ops.get(&op_id) {
            let act = if action.is_empty() {
                existing.action.clone()
            } else {
                action
            };
            (existing.started_at, existing.order, act)
        } else {
            state.next_order = state.next_order.saturating_add(1);
            (at, state.next_order, action)
        };
        state.ops.insert(
            op_id.clone(),
            ActiveOpEntry {
                op_id,
                action: resolved_action,
                status: "running".to_owned(),
                vids,
                people,
                started_at,
                order,
            },
        );
        self.publish_locked(&mut state, at);
    }

    /// Explicitly mark an operation as `"recovery_needed"` (even if not previously active, e.g.
    /// a batch child that failed ambiguously).
    pub fn mark_recovery_needed(
        &self,
        op_id: impl Into<String>,
        action: impl Into<String>,
        vids: impl IntoIterator<Item = String>,
        people: impl IntoIterator<Item = String>,
    ) {
        let op_id = op_id.into();
        let action = action.into();
        let vids = normalize_set(vids);
        let people = normalize_set(people);
        let at = now_ms();

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (started_at, order, final_action, final_vids, final_people) =
            if let Some(existing) = state.ops.get(&op_id) {
                let act = if action.is_empty() {
                    existing.action.clone()
                } else {
                    action
                };
                let v = if vids.is_empty() {
                    existing.vids.clone()
                } else {
                    vids
                };
                let p = if people.is_empty() {
                    existing.people.clone()
                } else {
                    people
                };
                (existing.started_at, existing.order, act, v, p)
            } else {
                state.next_order = state.next_order.saturating_add(1);
                (at, state.next_order, action, vids, people)
            };

        state.ops.insert(
            op_id.clone(),
            ActiveOpEntry {
                op_id: op_id.clone(),
                action: final_action.clone(),
                status: "recovery_needed".to_owned(),
                vids: final_vids.clone(),
                people: final_people.clone(),
                started_at,
                order,
            },
        );
        state.last_settled = Some(SettledSummary {
            op_id,
            action: final_action,
            outcome: "recovery_needed".to_owned(),
            people: final_people.into_iter().collect(),
            vids: final_vids.into_iter().collect(),
            settled_at: at,
        });
        self.publish_locked(&mut state, at);
    }

    /// Settle an operation.
    ///
    /// - If `outcome == "recovery_needed"`, keeps locks active with `status: "recovery_needed"`,
    ///   `label: "待恢复"` until explicitly reconciled.
    /// - Otherwise removes the operation's locks and updates `last_settled`.
    pub fn settle(
        &self,
        op_id: &str,
        outcome: &str,
        vids: impl IntoIterator<Item = String>,
        people: impl IntoIterator<Item = String>,
    ) {
        let vids = normalize_set(vids);
        let people = normalize_set(people);
        let at = now_ms();

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if outcome == "recovery_needed" {
            let (started_at, order, action, final_vids, final_people) =
                if let Some(existing) = state.ops.get(op_id) {
                    let v = if vids.is_empty() {
                        existing.vids.clone()
                    } else {
                        vids
                    };
                    let p = if people.is_empty() {
                        existing.people.clone()
                    } else {
                        people
                    };
                    (existing.started_at, existing.order, existing.action.clone(), v, p)
                } else {
                    state.next_order = state.next_order.saturating_add(1);
                    (at, state.next_order, String::new(), vids, people)
                };
            state.ops.insert(
                op_id.to_owned(),
                ActiveOpEntry {
                    op_id: op_id.to_owned(),
                    action: action.clone(),
                    status: "recovery_needed".to_owned(),
                    vids: final_vids.clone(),
                    people: final_people.clone(),
                    started_at,
                    order,
                },
            );
            state.last_settled = Some(SettledSummary {
                op_id: op_id.to_owned(),
                action,
                outcome: "recovery_needed".to_owned(),
                people: final_people.into_iter().collect(),
                vids: final_vids.into_iter().collect(),
                settled_at: at,
            });
            self.publish_locked(&mut state, at);
            return;
        }

        let Some(existing) = state.ops.remove(op_id) else {
            return;
        };
        let final_vids = if vids.is_empty() {
            existing.vids
        } else {
            vids
        };
        let final_people = if people.is_empty() {
            existing.people
        } else {
            people
        };
        state.last_settled = Some(SettledSummary {
            op_id: op_id.to_owned(),
            action: existing.action,
            outcome: outcome.to_owned(),
            people: final_people.into_iter().collect(),
            vids: final_vids.into_iter().collect(),
            settled_at: at,
        });
        self.publish_locked(&mut state, at);
    }

    /// Quietly remove an op entry without overwriting `last_settled` (e.g. when a parent batch
    /// hands off recovery state to its specific stuck child op).
    pub fn release_op(&self, op_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.ops.remove(op_id).is_some() {
            self.publish_locked(&mut state, now_ms());
        }
    }

    /// Called by `LockGuard` drop / finish safety net: releases only if the operation is still
    /// live (`queued` or `running`) and was not already settled or transitioned to `recovery_needed`.
    fn release_if_live(&self, op_id: &str, outcome: &str) {
        let at = now_ms();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let is_live = state
            .ops
            .get(op_id)
            .is_some_and(|entry| entry.status != "recovery_needed");
        if !is_live {
            return;
        }
        if let Some(existing) = state.ops.remove(op_id) {
            state.last_settled = Some(SettledSummary {
                op_id: op_id.to_owned(),
                action: existing.action,
                outcome: outcome.to_owned(),
                people: existing.people.into_iter().collect(),
                vids: existing.vids.into_iter().collect(),
                settled_at: at,
            });
            self.publish_locked(&mut state, at);
        }
    }

    /// Seed unresolved / `recovery_needed` operations from `operations.sqlite3` on daemon startup
    /// (called after `recover_startup` and `person_batch::recover`).
    pub fn seed_from_store(&self, store: &Store) -> Result<usize> {
        let mut stmt = store.db.prepare(
            "SELECT id, plan, CAST(unixepoch(created_at) * 1000 AS INTEGER)
             FROM operations
             WHERE status IN ('accepted', 'running', 'recovery_needed', 'rolling_back')
             ORDER BY created_at, id",
        )?;
        let rows: Vec<(String, String, Option<i64>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut batch_stmt = store.db.prepare(
            "SELECT id, plan, CAST(unixepoch(created_at) * 1000 AS INTEGER)
             FROM operations
             WHERE json_extract(plan, '$.action') = 'person-batch'
               AND (status = 'batch_running' OR json_extract(result, '$.recovery_needed') = 1)
             ORDER BY created_at, id",
        )?;
        let batch_rows: Vec<(String, String, Option<i64>)> = batch_stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Parse plans before acquiring the registry mutex so neither SQLite I/O nor JSON
        // deserialization runs while holding `self.state`.
        let at = now_ms();
        let parsed_rows: Vec<(String, String, BTreeSet<String>, BTreeSet<String>, u64)> = rows
            .into_iter()
            .filter_map(|(id, plan_raw, created_ms)| {
                let plan = serde_json::from_str::<Plan>(&plan_raw).ok()?;
                let (action, vids, people) = entities_from_plan(&plan);
                let started_at = created_ms
                    .and_then(|ms| u64::try_from(ms).ok())
                    .filter(|ms| *ms > 0)
                    .unwrap_or(at);
                Some((id, action, normalize_set(vids), normalize_set(people), started_at))
            })
            .collect();
        let parsed_batches: Vec<(
            String,
            Vec<String>,
            String,
            BTreeSet<String>,
            BTreeSet<String>,
            u64,
        )> = batch_rows
            .into_iter()
            .filter_map(|(id, plan_raw, created_ms)| {
                let plan = serde_json::from_str::<Plan>(&plan_raw).ok()?;
                let child_ids: Vec<String> = plan
                    .execution
                    .as_ref()
                    .and_then(|exec| exec.data.get("plans"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|child| child.get("id").and_then(Value::as_str).map(str::to_owned))
                    .collect();
                let (action, vids, people) = entities_from_plan(&plan);
                let started_at = created_ms
                    .and_then(|ms| u64::try_from(ms).ok())
                    .filter(|ms| *ms > 0)
                    .unwrap_or(at);
                Some((
                    id,
                    child_ids,
                    action,
                    normalize_set(vids),
                    normalize_set(people),
                    started_at,
                ))
            })
            .collect();

        let mut seeded = 0usize;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for (id, action, vids, people, started_at) in parsed_rows {
            let order = if let Some(existing) = state.ops.get(&id) {
                existing.order
            } else {
                state.next_order = state.next_order.saturating_add(1);
                state.next_order
            };
            state.ops.insert(
                id.clone(),
                ActiveOpEntry {
                    op_id: id,
                    action,
                    status: "recovery_needed".to_owned(),
                    vids,
                    people,
                    started_at,
                    order,
                },
            );
            seeded += 1;
        }
        for (id, child_ids, action, vids, people, started_at) in parsed_batches {
            if state.ops.contains_key(&id)
                || child_ids.iter().any(|cid| state.ops.contains_key(cid))
            {
                continue;
            }
            state.next_order = state.next_order.saturating_add(1);
            let order = state.next_order;
            state.ops.insert(
                id.clone(),
                ActiveOpEntry {
                    op_id: id,
                    action,
                    status: "recovery_needed".to_owned(),
                    vids,
                    people,
                    started_at,
                    order,
                },
            );
            seeded += 1;
        }
        if seeded > 0 {
            self.publish_locked(&mut state, at);
        }
        Ok(seeded)
    }
}

/// RAII guard for a registered active lock.
///
/// Dropping the guard without explicitly settling (or when an operation returns/panics before
/// settling) automatically releases any live (`queued`/`running`) lock with `outcome: "cancelled"`
/// while never disturbing a lock that has already been settled or marked `recovery_needed`.
pub struct LockGuard {
    registry: Arc<ActiveLockRegistry>,
    op_id: String,
    settled: bool,
    drop_outcome: &'static str,
}

impl LockGuard {
    #[allow(dead_code)]
    pub fn op_id(&self) -> &str {
        &self.op_id
    }

    #[allow(dead_code)]
    pub fn mark_running(
        &self,
        action: impl Into<String>,
        vids: impl IntoIterator<Item = String>,
        people: impl IntoIterator<Item = String>,
    ) {
        self.registry.mark_running(self.op_id.clone(), action, vids, people);
    }

    #[allow(dead_code)]
    pub fn settle(
        mut self,
        outcome: &str,
        vids: impl IntoIterator<Item = String>,
        people: impl IntoIterator<Item = String>,
    ) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.registry.settle(&self.op_id, outcome, vids, people);
    }

    /// Executor completion hook: settles any still-live lock based on the command result without
    /// touching operations already settled or retained in `recovery_needed` by `Engine`.
    pub fn finish_with_result(mut self, result: &Result<Value>) {
        if self.settled {
            return;
        }
        self.settled = true;
        let outcome = if result.is_ok() { "complete" } else { "failed" };
        self.registry.release_if_live(&self.op_id, outcome);
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.registry.release_if_live(&self.op_id, self.drop_outcome);
        }
    }
}

fn push_opt_str(set: &mut BTreeSet<String>, value: Option<&str>) {
    if let Some(s) = value.map(str::trim).filter(|s| !s.is_empty()) {
        set.insert(s.to_owned());
    }
}

fn push_str_array(set: &mut BTreeSet<String>, value: Option<&Value>) {
    if let Some(arr) = value.and_then(Value::as_array) {
        for item in arr {
            push_opt_str(set, item.as_str());
        }
    }
}

/// Extract authoritative `(canonical_action, vids, people)` from a built `Plan`.
pub fn entities_from_plan(plan: &Plan) -> (String, Vec<String>, Vec<String>) {
    let mut vids = BTreeSet::new();
    let mut people = BTreeSet::new();

    if plan.action == "person-batch" {
        let sub_action = plan
            .request
            .payload
            .as_ref()
            .and_then(|p| p["action"].as_str())
            .unwrap_or("");
        let canonical_action = match sub_action {
            "merge" => "batch-merge".to_owned(),
            "delete" => "batch-delete".to_owned(),
            _ => "person-batch".to_owned(),
        };
        push_opt_str(&mut people, Some(&plan.target_person));
        if let Some(payload) = &plan.request.payload {
            push_str_array(&mut people, payload.get("people"));
            push_opt_str(&mut people, payload.get("target").and_then(Value::as_str));
        }
        if let Some(exec) = &plan.execution
            && let Some(child_plans) = exec.data.get("plans").and_then(Value::as_array)
        {
            for child_val in child_plans {
                if let Ok(child) = serde_json::from_value::<Plan>(child_val.clone()) {
                    let (_, child_vids, child_people) = entities_from_plan(&child);
                    vids.extend(child_vids);
                    people.extend(child_people);
                } else {
                    push_opt_str(&mut people, child_val.get("person").and_then(Value::as_str));
                    push_opt_str(
                        &mut people,
                        child_val.get("target_person").and_then(Value::as_str),
                    );
                    if let Some(files) = child_val.get("files").and_then(Value::as_array) {
                        for f in files {
                            push_str_array(&mut vids, f.get("vids"));
                        }
                    }
                }
            }
        }
        return (
            canonical_action,
            vids.into_iter().collect(),
            people.into_iter().collect(),
        );
    }

    for file in &plan.files {
        for vid in &file.vids {
            push_opt_str(&mut vids, Some(vid));
        }
    }

    if let Some(exec) = &plan.execution {
        for scope in &exec.scopes {
            if let ScopeRoot::Person(p) = &scope.source {
                push_opt_str(&mut people, Some(p));
            }
            if let ScopeRoot::Person(p) = &scope.destination {
                push_opt_str(&mut people, Some(p));
            }
        }
    }

    match plan.action.as_str() {
        "auto-archive" => {
            if let Some(exec) = &plan.execution {
                if let Some(rows) = exec.data.get("rows").and_then(Value::as_array) {
                    for row in rows {
                        push_opt_str(&mut vids, row.get("vid").and_then(Value::as_str));
                        push_opt_str(&mut people, row.get("creator").and_then(Value::as_str));
                    }
                }
                if let Some(fallbacks) = exec.data.get("fallback_items").and_then(Value::as_array) {
                    for item in fallbacks {
                        push_opt_str(&mut vids, item.get("vid").and_then(Value::as_str));
                        push_opt_str(&mut people, item.get("creator").and_then(Value::as_str));
                    }
                }
            }
        }
        "archive" => {
            if let Some(exec) = &plan.execution
                && let Some(rows) = exec.data.get("rows").and_then(Value::as_array)
            {
                for row in rows {
                    push_opt_str(&mut vids, row.get("vid").and_then(Value::as_str));
                    push_opt_str(&mut people, row.get("creator").and_then(Value::as_str));
                }
            }
        }
        "archive-unarchive" => {
            push_opt_str(&mut people, Some(&plan.person));
            if let Some(exec) = &plan.execution {
                push_opt_str(&mut vids, exec.data.get("vid").and_then(Value::as_str));
                push_opt_str(&mut people, exec.data.get("person").and_then(Value::as_str));
            }
            if let Some(payload) = &plan.request.payload {
                push_opt_str(&mut vids, payload.get("vid").and_then(Value::as_str));
            }
        }
        "review-delete" => {
            if let Some(payload) = &plan.request.payload {
                push_str_array(&mut vids, payload.get("vids"));
            }
        }
        "delete-person" | "merge-person" | "rename-person" => {
            push_opt_str(&mut people, Some(&plan.person));
            push_opt_str(&mut people, Some(&plan.target_person));
            push_opt_str(&mut people, Some(&plan.request.person));
            push_opt_str(&mut people, plan.request.target_person.as_deref());
            if let Some(exec) = &plan.execution {
                push_str_array(&mut people, exec.data.get("identity_names"));
                push_str_array(&mut people, exec.data.get("source_names"));
                push_str_array(&mut people, exec.data.get("target_names"));
            }
        }
        "delete-files" | "move-files" | "rename-file" | "restore-files" | "delete-staged"
        | "clear-staging" => {
            for f in &plan.request.files {
                push_opt_str(&mut vids, Some(&f.vid));
            }
            push_opt_str(&mut people, Some(&plan.person));
            push_opt_str(&mut people, Some(&plan.target_person));
            push_opt_str(&mut people, Some(&plan.request.person));
            push_opt_str(&mut people, plan.request.target_person.as_deref());
        }
        "processing-result" => {
            if let Some(payload) = &plan.request.payload {
                push_opt_str(
                    &mut vids,
                    payload
                        .get("task")
                        .and_then(|t| t.get("vid"))
                        .and_then(Value::as_str),
                );
            }
        }
        _ => {
            push_opt_str(&mut people, Some(&plan.person));
            push_opt_str(&mut people, Some(&plan.target_person));
            push_opt_str(&mut people, Some(&plan.request.person));
            push_opt_str(&mut people, plan.request.target_person.as_deref());
            for f in &plan.request.files {
                push_opt_str(&mut vids, Some(&f.vid));
            }
            if let Some(payload) = &plan.request.payload {
                push_opt_str(&mut vids, payload.get("vid").and_then(Value::as_str));
                push_str_array(&mut vids, payload.get("vids"));
                push_opt_str(&mut people, payload.get("creator").and_then(Value::as_str));
                push_opt_str(&mut people, payload.get("person").and_then(Value::as_str));
            }
        }
    }

    (
        plan.action.clone(),
        vids.into_iter().collect(),
        people.into_iter().collect(),
    )
}

/// Best-effort entity extraction from an incoming `/v1/person-files` `Request` at admission time.
#[allow(dead_code)]
pub fn extract_from_request(request: &Request) -> Option<(String, Vec<String>, Vec<String>)> {
    if !tracks_action(&request.action) {
        return None;
    }
    let mut vids = BTreeSet::new();
    let mut people = BTreeSet::new();
    for f in &request.files {
        push_opt_str(&mut vids, Some(&f.vid));
    }
    push_opt_str(&mut people, Some(&request.person));
    push_opt_str(&mut people, request.target_person.as_deref());
    if let Some(payload) = &request.payload {
        push_opt_str(&mut vids, payload.get("vid").and_then(Value::as_str));
        push_str_array(&mut vids, payload.get("vids"));
        push_opt_str(&mut people, payload.get("creator").and_then(Value::as_str));
        push_opt_str(&mut people, payload.get("person").and_then(Value::as_str));
    }
    Some((
        request.action.clone(),
        vids.into_iter().collect(),
        people.into_iter().collect(),
    ))
}

/// Canonical action & best-effort entity extraction for mutating `/v1/archive` commands.
#[allow(dead_code)]
pub fn extract_from_archive_body(
    command: &str,
    body: &Value,
) -> Option<(String, Vec<String>, Vec<String>)> {
    let action = match command {
        "archive-apply" => "archive",
        "auto-watermark-apply" => "auto-archive",
        "archive-unarchive" => "archive-unarchive",
        "archive-resolve" => "archive-resolve",
        "queue-confirm" => "queue-confirm",
        "review-note" => "review-note",
        "review-defer" => "review-defer",
        "review-restore" => "review-restore",
        "queue-undo" => "queue-undo",
        "claim-decide" => "claim-decide",
        _ => return None,
    };
    let mut vids = BTreeSet::new();
    let mut people = BTreeSet::new();
    if let Some(obj) = body.as_object() {
        push_opt_str(&mut vids, obj.get("vid").and_then(Value::as_str));
        push_str_array(&mut vids, obj.get("vids"));
        push_opt_str(&mut people, obj.get("creator").and_then(Value::as_str));
        push_opt_str(&mut people, obj.get("person").and_then(Value::as_str));
        push_str_array(&mut people, obj.get("creators"));
        if let Some(items) = obj.get("items").and_then(Value::as_array) {
            for item in items {
                push_opt_str(&mut vids, item.get("vid").and_then(Value::as_str));
                push_opt_str(&mut people, item.get("person").and_then(Value::as_str));
                push_opt_str(&mut people, item.get("creator").and_then(Value::as_str));
            }
        }
    }
    Some((
        action.to_owned(),
        vids.into_iter().collect(),
        people.into_iter().collect(),
    ))
}

/// Best-effort entity extraction for `/v1/person-batch` `mode == "apply"` using the persisted
/// preview plan in `operations.sqlite3` (read-only off the executor lane).
#[allow(dead_code)]
pub fn extract_from_batch_apply(
    config: &Config,
    body: &Value,
) -> Option<(String, String, Vec<String>, Vec<String>)> {
    let id = body.get("id").and_then(Value::as_str)?.to_owned();
    crate::model::validate_id(&id).ok()?;
    if let Ok(db) = crate::db::open_readonly(&config.state_db())
        && let Ok(Some(record)) = crate::store::record(&db, &id)
    {
        let (action, vids, people) = entities_from_plan(&record.plan);
        return Some((id, action, vids, people));
    }
    let req = body.get("request")?;
    let action = match req.get("action").and_then(Value::as_str) {
        Some("merge") => "batch-merge".to_owned(),
        Some("delete") => "batch-delete".to_owned(),
        _ => return None,
    };
    let mut people = BTreeSet::new();
    push_str_array(&mut people, req.get("people"));
    push_opt_str(&mut people, req.get("target").and_then(Value::as_str));
    Some((id, action, vec![], people.into_iter().collect()))
}

struct ReceiverStream {
    rx: mpsc::Receiver<std::result::Result<Bytes, Infallible>>,
}

impl Stream for ReceiverStream {
    type Item = std::result::Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

async fn send_or_shutdown(
    tx: &mpsc::Sender<std::result::Result<Bytes, Infallible>>,
    shutdown: &mut watch::Receiver<bool>,
    bytes: Bytes,
) -> bool {
    if *shutdown.borrow() {
        return false;
    }
    tokio::select! {
        biased;
        res = shutdown.changed() => {
            let _ = res;
            false
        }
        sent = tx.send(Ok(bytes)) => sent.is_ok(),
    }
}

async fn send_snapshot_frame(
    tx: &mpsc::Sender<std::result::Result<Bytes, Infallible>>,
    shutdown: &mut watch::Receiver<bool>,
    snapshot: &LockSnapshot,
) -> bool {
    let Ok(json) = serde_json::to_string(snapshot) else {
        return false;
    };
    let frame = format!("event: locks\ndata: {json}\n\n");
    send_or_shutdown(tx, shutdown, Bytes::from(frame)).await
}

/// Build an HTTP 200 JSON response for `GET /v1/locks` with `Cache-Control: no-store`.
pub fn snapshot_response(registry: &ActiveLockRegistry) -> Response {
    let snap = registry.snapshot();
    match serde_json::to_vec(&*snap) {
        Ok(body) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(body))
            .expect("valid JSON response"),
        Err(err) => Error::new(500, "serialization_error", format!("锁状态序列化失败：{err}"))
            .into_response(),
    }
}

/// Build an HTTP 200 SSE response for `GET /v1/locks/stream` using the standard 15s keepalive.
pub fn stream_response(registry: &ActiveLockRegistry) -> Response {
    stream_response_with_keepalive(registry, DEFAULT_SSE_KEEPALIVE)
}

/// Build an HTTP 200 SSE response with a configurable keepalive interval (used by tests & prod).
pub fn stream_response_with_keepalive(
    registry: &ActiveLockRegistry,
    keepalive_interval: Duration,
) -> Response {
    let mut rx = registry.subscribe();
    let mut shutdown = registry.subscribe_shutdown();
    let (tx, body_rx) = mpsc::channel::<std::result::Result<Bytes, Infallible>>(32);

    tokio::spawn(async move {
        let initial = rx.borrow_and_update().clone();
        if !send_snapshot_frame(&tx, &mut shutdown, &initial).await {
            return;
        }
        if *shutdown.borrow() {
            return;
        }
        let mut keepalive = tokio::time::interval(keepalive_interval);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the immediate first tick so the first keepalive fires after `keepalive_interval`.
        keepalive.tick().await;

        loop {
            tokio::select! {
                biased;
                _ = tx.closed() => break,
                res = shutdown.changed() => {
                    if res.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                changed = rx.changed() => {
                    match changed {
                        Ok(()) => {
                            let snap = rx.borrow_and_update().clone();
                            if !send_snapshot_frame(&tx, &mut shutdown, &snap).await {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = keepalive.tick() => {
                    if !send_or_shutdown(&tx, &mut shutdown, Bytes::from_static(b": keepalive\n\n")).await {
                        break;
                    }
                }
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(ReceiverStream { rx: body_rx }))
        .expect("valid SSE response")
}

#[derive(Clone)]
struct LockRouterState {
    config: Config,
    registry: Arc<ActiveLockRegistry>,
    keepalive: Duration,
}

fn authorize_headers(config: &Config, headers: &HeaderMap) -> Result<()> {
    if let Some(token) = &config.token
        && headers.get("authorization").and_then(|h| h.to_str().ok())
            != Some(format!("Bearer {token}").as_str())
    {
        return Err(Error::new(401, "unauthorized", "访问令牌无效"));
    }
    Ok(())
}

async fn handle_locks_get(
    State(state): State<LockRouterState>,
    headers: HeaderMap,
) -> Result<Response> {
    authorize_headers(&state.config, &headers)?;
    Ok(snapshot_response(&state.registry))
}

async fn handle_locks_stream(
    State(state): State<LockRouterState>,
    headers: HeaderMap,
) -> Result<Response> {
    authorize_headers(&state.config, &headers)?;
    Ok(stream_response_with_keepalive(&state.registry, state.keepalive))
}

/// Standalone authenticated Axum router for `/v1/locks` and `/v1/locks/stream` (useful for
/// integration tests exercising the exact HTTP/SSE wire protocol).
#[allow(dead_code)]
pub fn router(config: Config, registry: Arc<ActiveLockRegistry>) -> Router {
    router_with_keepalive(config, registry, DEFAULT_SSE_KEEPALIVE)
}

#[allow(dead_code)]
pub fn router_with_keepalive(
    config: Config,
    registry: Arc<ActiveLockRegistry>,
    keepalive: Duration,
) -> Router {
    let state = LockRouterState {
        config,
        registry,
        keepalive,
    };
    Router::new()
        .route("/v1/locks", get(handle_locks_get))
        .route("/v1/locks/stream", get(handle_locks_stream))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AffectedStores, ExecutionIntent, FailurePolicy, FileMove, MoveScope};
    use serde_json::json;

    #[test]
    fn label_mapping_covers_all_actions_and_recovery_needed() {
        let cases = [
            ("auto-archive", "合入中", "排队合入"),
            ("archive", "归档中", "排队归档"),
            ("archive-unarchive", "撤回中", "排队撤回"),
            ("delete-person", "删除中", "排队删除"),
            ("batch-delete", "删除中", "排队删除"),
            ("merge-person", "合并中", "排队合并"),
            ("batch-merge", "合并中", "排队合并"),
            ("rename-person", "改名中", "排队改名"),
            ("delete-files", "删除中", "排队删除"),
            ("review-delete", "删除中", "排队删除"),
            ("move-files", "移动中", "排队移动"),
            ("rename-file", "改名中", "排队改名"),
            ("staging-restore", "恢复中", "排队恢复"),
            ("restore-files", "恢复中", "排队恢复"),
            ("staging-delete", "清理中", "排队清理"),
            ("delete-staged", "清理中", "排队清理"),
            ("clear-staging", "清理中", "排队清理"),
        ];
        for (action, running, queued) in cases {
            assert_eq!(label_for(action, "running"), running, "running {action}");
            assert_eq!(label_for(action, "queued"), queued, "queued {action}");
            assert_eq!(
                label_for(action, "recovery_needed"),
                "待恢复",
                "recovery {action}"
            );
        }
    }

    #[test]
    fn queued_running_settled_lifecycle_updates_revision_and_watch() {
        let reg = ActiveLockRegistry::new();
        let mut rx = reg.subscribe();
        assert_eq!(reg.snapshot().revision, 0);
        assert!(reg.snapshot().videos.is_empty());
        assert!(reg.snapshot().people.is_empty());
        assert!(reg.snapshot().last_settled.is_none());

        let guard = reg.register_queued(
            "op-1",
            "auto-archive",
            vec!["v1".into(), "v2".into()],
            vec!["Alice".into()],
        );
        assert!(rx.has_changed().unwrap());
        let s1 = rx.borrow_and_update().clone();
        assert_eq!(s1.revision, 1);
        assert_eq!(s1.videos["v1"].status, "queued");
        assert_eq!(s1.videos["v1"].label, "排队合入");
        assert_eq!(s1.people["Alice"].label, "排队合入");
        assert_eq!(s1.active_ops.len(), 1);
        assert_eq!(s1.active_ops[0].video_count, 2);

        // Authoritative running set replaces best-effort set (v2 skipped, Bob added).
        reg.mark_running(
            "op-1",
            "auto-archive",
            vec!["v1".into()],
            vec!["Alice".into(), "Bob".into()],
        );
        let s2 = rx.borrow_and_update().clone();
        assert_eq!(s2.revision, 2);
        assert!(s2.videos.contains_key("v1"));
        assert!(!s2.videos.contains_key("v2"));
        assert_eq!(s2.videos["v1"].status, "running");
        assert_eq!(s2.videos["v1"].label, "合入中");
        assert_eq!(s2.people["Alice"].label, "合入中");
        assert_eq!(s2.people["Bob"].label, "合入中");
        assert_eq!(s2.active_ops[0].video_count, 1);

        guard.settle("complete", vec!["v1".into()], vec!["Alice".into(), "Bob".into()]);
        let s3 = rx.borrow_and_update().clone();
        assert_eq!(s3.revision, 3);
        assert!(s3.videos.is_empty());
        assert!(s3.people.is_empty());
        assert!(s3.active_ops.is_empty());
        let settled = s3.last_settled.as_ref().expect("last_settled");
        assert_eq!(settled.op_id, "op-1");
        assert_eq!(settled.action, "auto-archive");
        assert_eq!(settled.outcome, "complete");
        assert_eq!(settled.vids, vec!["v1"]);
        assert_eq!(settled.people, vec!["Alice", "Bob"]);
    }

    #[test]
    fn raii_drop_releases_live_lock_but_preserves_recovery_needed() {
        let reg = ActiveLockRegistry::new();
        {
            let _guard = reg.register_queued(
                "op-drop",
                "review-delete",
                vec!["v-drop".into()],
                Vec::<String>::new(),
            );
            assert_eq!(reg.snapshot().videos["v-drop"].label, "排队删除");
        }
        let after_drop = reg.snapshot();
        assert!(after_drop.videos.is_empty());
        assert!(after_drop.active_ops.is_empty());
        assert_eq!(
            after_drop.last_settled.as_ref().unwrap().outcome,
            "cancelled"
        );

        // Now verify recovery_needed survives guard drop.
        {
            let _guard = reg.register_queued(
                "op-rec",
                "delete-person",
                vec!["v-rec".into()],
                vec!["Carol".into()],
            );
            reg.mark_running(
                "op-rec",
                "delete-person",
                vec!["v-rec".into()],
                vec!["Carol".into()],
            );
            reg.settle(
                "op-rec",
                "recovery_needed",
                vec!["v-rec".into()],
                vec!["Carol".into()],
            );
        }
        let s_rec = reg.snapshot();
        assert_eq!(s_rec.videos["v-rec"].status, "recovery_needed");
        assert_eq!(s_rec.videos["v-rec"].label, "待恢复");
        assert_eq!(s_rec.people["Carol"].status, "recovery_needed");
        assert_eq!(s_rec.people["Carol"].label, "待恢复");

        // Retrying same op_id while in recovery_needed must not downgrade or clear it.
        {
            let _dup = reg.register_queued(
                "op-rec",
                "delete-person",
                vec!["v-rec".into()],
                vec!["Carol".into()],
            );
            assert_eq!(reg.snapshot().people["Carol"].status, "recovery_needed");
        }
        assert_eq!(reg.snapshot().people["Carol"].status, "recovery_needed");

        // Reconcile clears it.
        reg.settle("op-rec", "complete", vec!["v-rec".into()], vec!["Carol".into()]);
        assert!(reg.snapshot().videos.is_empty());
        assert!(reg.snapshot().people.is_empty());
        assert_eq!(reg.snapshot().last_settled.as_ref().unwrap().outcome, "complete");
    }

    #[test]
    fn precedence_prefers_recovery_then_running_then_queued() {
        let reg = ActiveLockRegistry::new();
        let g1 = reg.register_queued("op-1", "merge-person", vec!["v1".into()], vec!["Alice".into()]);
        reg.mark_running("op-1", "merge-person", vec!["v1".into()], vec!["Alice".into()]);
        let g2 = reg.register_queued("op-2", "delete-person", vec!["v1".into()], vec!["Alice".into()]);

        let snap = reg.snapshot();
        assert_eq!(snap.people["Alice"].op_id, "op-1");
        assert_eq!(snap.people["Alice"].status, "running");
        assert_eq!(snap.people["Alice"].label, "合并中");

        g1.settle("complete", vec!["v1".into()], vec!["Alice".into()]);
        let snap2 = reg.snapshot();
        assert_eq!(snap2.people["Alice"].op_id, "op-2");
        assert_eq!(snap2.people["Alice"].status, "queued");
        assert_eq!(snap2.people["Alice"].label, "排队删除");

        drop(g2);
        assert!(reg.snapshot().people.is_empty());
    }

    #[test]
    fn entities_from_plan_covers_batch_auto_archive_and_review_delete() {
        let child = Plan {
            id: "c1".into(),
            request: Request {
                id: "c1".into(),
                action: "merge-person".into(),
                person: "Bob".into(),
                files: vec![],
                filename: String::new(),
                target_person: Some("Alice".into()),
                target_folder: None,
                payload: None,
            },
            version: "v".into(),
            action: "merge-person".into(),
            person: "Bob".into(),
            target_person: "Alice".into(),
            files: vec![FileMove {
                vids: vec!["vid-b1".into()],
                src: "/src".into(),
                dst: "/dst".into(),
                size: 10,
                version: "ver".into(),
            }],
            total_bytes: 10,
            directories: vec![],
            ledger_version: String::new(),
            metadata_version: String::new(),
            execution: Some(ExecutionIntent {
                scopes: vec![MoveScope {
                    source: ScopeRoot::Person("Bob".into()),
                    destination: ScopeRoot::Person("Alice".into()),
                }],
                failure_policy: FailurePolicy::Rollback,
                stores: AffectedStores {
                    faces: true,
                    jobs: false,
                    ledger: false,
                },
                data: json!({"identity_names": ["Bob", "Alice"]}),
                result: json!({}),
            }),
        };
        let parent = Plan {
            id: "p1".into(),
            request: Request {
                id: "p1".into(),
                action: "person-batch".into(),
                person: String::new(),
                files: vec![],
                filename: String::new(),
                target_person: Some("Alice".into()),
                target_folder: None,
                payload: Some(json!({"action": "merge", "people": ["Bob", "Alice"], "target": "Alice"})),
            },
            version: "v".into(),
            action: "person-batch".into(),
            person: String::new(),
            target_person: "Alice".into(),
            files: vec![],
            total_bytes: 10,
            directories: vec![],
            ledger_version: String::new(),
            metadata_version: String::new(),
            execution: Some(ExecutionIntent {
                scopes: vec![],
                failure_policy: FailurePolicy::Rollback,
                stores: AffectedStores {
                    faces: false,
                    jobs: false,
                    ledger: false,
                },
                data: json!({"plans": [child]}),
                result: json!({}),
            }),
        };
        let (action, vids, people) = entities_from_plan(&parent);
        assert_eq!(action, "batch-merge");
        assert_eq!(vids, vec!["vid-b1"]);
        assert_eq!(people, vec!["Alice", "Bob"]);
    }

    #[test]
    fn duplicate_register_queued_while_queued_or_running_is_pre_disarmed() {
        let reg = ActiveLockRegistry::new();

        // Duplicate while original is still "queued" -> dropping or finishing duplicate never clears original.
        let primary = reg.register_queued(
            "op-dup",
            "archive",
            vec!["v1".into()],
            vec!["Alice".into()],
        );
        let rev_before = reg.snapshot().revision;
        {
            let dup_drop = reg.register_queued(
                "op-dup",
                "delete-person",
                vec!["v-other".into()],
                vec!["Bob".into()],
            );
            // Duplicate did not overwrite existing entry or bump revision.
            let snap = reg.snapshot();
            assert_eq!(snap.revision, rev_before);
            assert_eq!(snap.videos["v1"].action, "archive");
            assert!(!snap.people.contains_key("Bob"));
            drop(dup_drop);
        }
        let after_dup_drop = reg.snapshot();
        assert_eq!(after_dup_drop.revision, rev_before);
        assert_eq!(after_dup_drop.videos["v1"].status, "queued");
        assert!(after_dup_drop.last_settled.is_none());

        // Upgrade original to "running", then retry same op_id and call finish_with_result / settle on duplicate.
        reg.mark_running(
            "op-dup",
            "archive",
            vec!["v1".into()],
            vec!["Alice".into()],
        );
        let rev_running = reg.snapshot().revision;

        let dup_err = reg.register_queued(
            "op-dup",
            "archive",
            vec!["v1".into()],
            vec!["Alice".into()],
        );
        dup_err.finish_with_result(&Err(Error::new(503, "busy", "busy")));
        assert_eq!(reg.snapshot().revision, rev_running);
        assert_eq!(reg.snapshot().videos["v1"].status, "running");

        let dup_ok = reg.register_queued(
            "op-dup",
            "archive",
            vec!["v1".into()],
            vec!["Alice".into()],
        );
        dup_ok.finish_with_result(&Ok(json!({"ok": true})));
        assert_eq!(reg.snapshot().revision, rev_running);
        assert_eq!(reg.snapshot().videos["v1"].status, "running");

        let dup_settle = reg.register_queued(
            "op-dup",
            "archive",
            vec!["v1".into()],
            vec!["Alice".into()],
        );
        dup_settle.settle("cancelled", vec!["v1".into()], vec!["Alice".into()]);
        assert_eq!(reg.snapshot().revision, rev_running);
        assert_eq!(reg.snapshot().videos["v1"].status, "running");

        // Primary guard still settles normally.
        primary.finish_with_result(&Ok(json!({"ok": true})));
        let final_snap = reg.snapshot();
        assert!(final_snap.videos.is_empty());
        assert_eq!(final_snap.last_settled.as_ref().unwrap().outcome, "complete");
    }

    #[tokio::test]
    async fn sse_send_or_shutdown_unblocks_immediately_when_client_buffer_is_full() {
        let (tx, _rx_stalled) = mpsc::channel::<std::result::Result<Bytes, Infallible>>(1);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        // Fill the 1-slot channel completely so the next send would block indefinitely.
        tx.send(Ok(Bytes::from_static(b"first"))).await.unwrap();

        let send_task = tokio::spawn(async move {
            send_or_shutdown(&tx, &mut shutdown_rx, Bytes::from_static(b": keepalive\n\n")).await
        });

        // Yield once so send_task enters the blocked tx.send(...) branch, then trigger shutdown.
        tokio::task::yield_now().await;
        let _ = shutdown_tx.send_replace(true);

        let sent = tokio::time::timeout(Duration::from_secs(1), send_task)
            .await
            .expect("send_or_shutdown did not hang on full buffer")
            .unwrap();
        assert!(!sent);
    }
}
