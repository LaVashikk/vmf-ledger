//! The inverse operations a rollback replays.
//!
//! Every op carries both what the tool wrote and what stood there before, which
//! makes a rollback a compare-and-swap: if the current value is not what the
//! tool left behind, someone else has edited that exact spot and the rollback
//! refuses rather than overwriting their work.

use serde::{Deserialize, Serialize};
use vmf_forge::prelude::*;

/// Which list an entity lives in. Hidden entities still compile into the BSP,
/// so they are tracked the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    Entity,
    Hidden,
}

/// A tool-written token identifying one entity across arbitrary editing.
///
/// Entity `id` is not enough: Hammer renumbers on save. The token is written
/// into the entity as a keyvalue and removed again on rollback.
pub type Token = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// The tool changed a keyvalue.
    Set {
        at: Token,
        key: String,
        old: String,
        new: String,
    },
    /// The tool added a keyvalue; rolling back removes it.
    Add { at: Token, key: String, new: String },
    /// The tool removed a keyvalue; rolling back re-inserts it at `idx`.
    ///
    /// The index matters: keyvalue order is the order of lines in the file, and
    /// restoring a key at the end would leave a needlessly noisy diff.
    Remove {
        at: Token,
        idx: usize,
        key: String,
        old: String,
    },
    /// The tool touched the connections list. Stored whole rather than per
    /// connection: the lists are short and a whole-list swap has no ordering
    /// pitfalls.
    Connections {
        at: Token,
        old: Option<Vec<Connection>>,
        new: Option<Vec<Connection>>,
    },
    /// The tool created an entity; rolling back deletes it.
    AddEntity { at: Token, bucket: Bucket },
    /// The tool deleted an entity; rolling back re-inserts it.
    ///
    /// `idx` is where it sat in the original file. A rollback clamps to the end
    /// of the current list, so position is best-effort when the file has since
    /// grown or shrunk.
    RemoveEntity {
        at: Token,
        bucket: Bucket,
        idx: usize,
        entity: Entity,
    },
}

impl Op {
    /// The entity this op addresses.
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
