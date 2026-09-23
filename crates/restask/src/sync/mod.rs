//! Reconciliation (§11): the pure planner plus the I/O orchestrating engine.

pub mod planner;

pub use planner::{
    plan, AdoptOp, DeferReason, DeleteOp, InsertTarget, MarkdownOp, MoveOp, Plan, Snapshots,
};
