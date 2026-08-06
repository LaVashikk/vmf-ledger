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

/// A change to the file itself rather than to one entity.
///
/// Kept out of [`Op`] on purpose: every `Op` is addressed by the token of the
/// entity it belongs to, and a visgroup belongs to no entity. Squeezing it in
/// would mean a variant with a token that addresses nothing and a special case
/// in every match that resolves one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum VisgroupOp {
    /// The tool created a visgroup; rolling back deletes it, along with the
    /// membership of anything still sitting in it.
    Add { id: i32, name: String },
}

/// Connections in the form the file will actually hold them.
///
/// A `connections` block is a KeyValues map, so writing it groups everything
/// sharing an output name: a list left interleaved comes back grouped. Both the
/// recorded value and the guard have to speak that form, or a rollback fires on
/// a difference the file cannot even represent.
pub fn as_written(connections: Option<&Vec<Connection>>) -> Option<Vec<Connection>> {
    let list = connections.map(Vec::as_slice).unwrap_or_default();
    Connection::from_key_values(&Connection::to_key_values(list))
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

/// A spot where the map no longer holds what the tool left behind.
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
