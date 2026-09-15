//! Inverse operations recorded for rollback execution.
//!
//! Operations record previous and replacement values to enable compare-and-swap
//! verification during rollback, preventing unintended overwrites of concurrent edits.

use serde::{Deserialize, Serialize};
use source_vmf::prelude::*;

/// Target entity collection within a VMF file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    Entity,
    Hidden,
}

/// Unique token stored as an entity keyvalue to identify entities across Hammer saves.
pub type Token = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Set {
        at: Token,
        key: String,
        old: String,
        new: String,
    },
    Add {
        at: Token,
        key: String,
        new: String,
    },
    /// Re-inserts a deleted keyvalue at its original index to preserve file ordering.
    Remove {
        at: Token,
        idx: usize,
        key: String,
        old: String,
    },
    /// Replaces the connections block in full to avoid ordering ambiguities.
    Connections {
        at: Token,
        old: Option<Vec<Connection>>,
        new: Option<Vec<Connection>>,
    },
    AddEntity {
        at: Token,
        bucket: Bucket,
    },
    RemoveEntity {
        at: Token,
        bucket: Bucket,
        idx: usize,
        /// Boxed to avoid enum size inflation.
        entity: Box<Entity>,
    },
}

/// Map-level operations not scoped to a specific entity token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum VisgroupOp {
    Add { id: i32, name: String },
}

/// Normalizes connections to match KeyValues serialization order.
///
/// KeyValues serialization groups connections by output name. Normalization ensures
/// compare-and-swap equality checks operate on identical representations.
pub fn as_written(connections: Option<&Vec<Connection>>) -> Option<Vec<Connection>> {
    let list = connections.map(Vec::as_slice).unwrap_or_default();
    Connection::from_key_values(&Connection::to_key_values(list))
}

impl Op {
    pub fn token(&self) -> &str {
        match self {
            Op::Set { at, .. }
            | Op::Add { at, .. }
            | Op::Remove { at, .. }
            | Op::Connections { at, .. }
            | Op::AddEntity { at, .. }
            | Op::RemoveEntity { at, .. } => at,
        }
    }
}

/// Describes a compare-and-swap mismatch encountered during rollback validation.
#[derive(Debug, Clone, PartialEq)]
pub struct Collision {
    pub token: Token,
    pub what: String,
    pub expected: String,
    pub found: String,
}

impl std::fmt::Display for Collision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} is {:?}, expected {:?}",
            self.token, self.what, self.found, self.expected
        )
    }
}
