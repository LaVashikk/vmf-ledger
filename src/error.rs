//! Everything that can go wrong.
//!
//! These messages are read by whoever runs the tool, not by whoever wrote it,
//! so each one names the file and the tool that left the mark.

use std::path::PathBuf;

use crate::ops::{Collision, Token};

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("{path}: {source}")]
    SidecarIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: not a valid .vdif journal: {source}")]
    SidecarFormat {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("journal format v{found}, this build understands v{expected}")]
    SidecarVersion { found: u32, expected: u32 },
    #[error(
        "refusing to write {path}: it exists but does not read back as a journal.\n\
         Another tool may have its rollback data in there, and overwriting it would \
         destroy it.\n\
         Reason: {reason}"
    )]
    SidecarClobber { path: PathBuf, reason: String },

    #[error("the map carries no mark from {0}; nothing to roll back")]
    NotMarked(String),
    #[error(
        "the map was compiled by {marker}, but its journal {path} is gone.\n\
         Without it the compile cannot be undone, and compiling again would build on \
         top of the previous output"
    )]
    JournalMissing { path: PathBuf, marker: String },
    #[error("the map says {name} compiled it, but {path} holds no section for {name}")]
    SectionMissing { path: PathBuf, name: String },
    #[error(
        "the {name} journal does not belong to this map: the marker says {in_map}, the \
         journal is {in_journal}"
    )]
    Mismatched {
        name: String,
        in_map: String,
        in_journal: String,
    },
    #[error("marker {0} is on more than one entity; a compiled entity was probably duplicated")]
    DuplicateMarker(Token),

    #[error(
        "the map was edited where this tool had written:\n{}\n\
         re-run with force to roll back anyway, discarding those edits",
        .0.iter().map(|c| format!("  {c}")).collect::<Vec<_>>().join("\n")
    )]
    Collisions(Vec<Collision>),

    #[error(
        "these changes cannot be rolled back, so no journal was written:\n{}",
        .0.iter().map(|s| format!("  {s}")).collect::<Vec<_>>().join("\n")
    )]
    Untracked(Vec<String>),
}
