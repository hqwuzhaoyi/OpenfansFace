pub mod admission;
pub mod agent;
pub mod agent_deployment;
pub mod agent_registry;
pub mod archive;
pub mod archive_api;
pub mod archive_cancel;
pub mod archive_ledger;
pub mod archive_resolution;
pub mod auto_archive;
pub mod auto_watermark;
pub mod config;
pub mod control;
pub mod creator_delta;
pub mod db;
pub mod delta;
pub mod engine;
pub mod fault;
pub mod files;
pub mod ledger;
pub mod metadata;
pub mod model;
pub mod native;
pub mod permissions;
pub mod person_batch;
pub mod person_delete;
pub mod person_directory;
pub mod person_ledger;
pub mod person_library;
pub mod person_merge;
pub mod person_merge_ledger;
pub mod person_merge_metadata;
pub mod person_metadata;
pub mod person_move;
pub mod person_registration;
pub mod person_rename;
pub mod processing_result;
pub mod processing_scheduler;
pub mod reference_relocation;
pub mod review;
pub mod review_sessions;
pub mod shared_metadata;
pub mod staging;
pub mod staging_delete;
pub mod store;
pub mod unarchive;
pub mod video_health;

pub use config::Config;
pub use engine::Engine;
pub use model::{Envelope, Error, Result};
use serde_json::Value;
use tokio::sync::oneshot;

pub enum Command {
    Files(Box<Envelope>),
    Batch(Value),
    Metadata { command: String, body: Value },
    Archive { command: String, body: Value },
}

pub struct Work {
    pub command: Command,
    pub reply: oneshot::Sender<Result<Value>>,
}
