//! Host-neutral migration lifecycle orchestration.
//!
//! This module owns migration generation, planning, application, rollback, and
//! applied-state tracking through caller-owned storage and execution adapters.

mod adapters;
mod catalog;
mod engine;

pub use adapters::{
    BoxFuture, DatabaseTrackingStore, Executor, ExecutorError, MigrationStore, StoreError,
    TRACKING_TABLE, TrackingError, TrackingStore,
};
pub use catalog::{EngineError, MigrationArtifact, MigrationCatalog, MigrationMovement};
pub use engine::MigrationEngine;
