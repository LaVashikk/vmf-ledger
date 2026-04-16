//! Reversible VMF edits.
//!
//! Modify a map freely, then work out what changed and write enough beside the
//! file to undo exactly those changes later - and only those, so edits made by
//! anyone else in the meantime survive the rollback.

use std::ops::{Deref, DerefMut};

use vmf_forge::prelude::*;

pub mod diff;
pub mod error;
pub mod marker;
pub mod matching;
pub mod ops;
pub mod rollback;
pub mod sidecar;

use diff::Diff;
use sidecar::Journal;

pub use error::LedgerError;
pub use rollback::restore;

/// Key written into `world`. Hammer preserves world keyvalues the same way it
/// preserves `comment`.
pub const DEFAULT_WORLD_KEY: &str = "vmf_ledger";

/// Key marking the entities the tool touched. Short, and prefixed so it reads
/// as tooling when a mapper stumbles over it in Hammer.
pub const DEFAULT_MARKER_KEY: &str = "_vlid";

#[derive(Debug, Clone)]
pub struct LedgerOptions {
    /// Tool and build in one string. Goes into the map's mark and the journal.
    pub tool: String,
    /// Key of the mark in `world`.
    pub world_key: String,
    /// Key marking the entities this tool touched.
    pub marker_key: String,
}

impl LedgerOptions {
    pub fn new(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            world_key: DEFAULT_WORLD_KEY.to_string(),
            marker_key: DEFAULT_MARKER_KEY.to_string(),
        }
    }

    /// Overrides the marker key. Two tools sharing a map would otherwise write
    /// their tokens into the same keyvalue and read each other's back.
    pub fn with_marker_key(mut self, key: impl Into<String>) -> Self {
        self.marker_key = key.into();
        self
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RestoreOptions {
    /// Roll back over edits made where the tool had written, losing them.
    /// Edits anywhere else survive either way.
    pub force: bool,
}

/// A map plus the pristine copy it started from.
///
/// Deref goes to the working copy, so a tool mutates it exactly as it would a
/// plain [`VmfFile`] and the bookkeeping only happens at [`TrackedVmf::finish`].
#[derive(Debug, Clone)]
pub struct TrackedVmf {
    original: VmfFile,
    working: VmfFile,
}

impl TrackedVmf {
    pub fn new(file: VmfFile) -> Self {
        Self {
            original: file.clone(),
            working: file,
        }
    }

    /// The map as it was before any edits.
    pub fn original(&self) -> &VmfFile {
        &self.original
    }

    pub fn diff(&self) -> Diff {
        diff::diff(&self.original, &self.working)
    }

    /// Stamps the map and produces the journal that undoes the edits.
    ///
    /// Fails rather than writing a journal that promises more than it can
    /// deliver: if anything outside the tracked scope changed, the caller finds
    /// out here instead of at rollback time.
    pub fn finish(mut self, opts: &LedgerOptions) -> Result<(VmfFile, Journal), LedgerError> {
        let diff = self.diff();
        if !diff.untracked.is_empty() {
            return Err(LedgerError::Untracked(diff.untracked));
        }

        let Diff { ops, marks, .. } = diff;
        for mark in &marks {
            if let Some(ent) = rollback::entity_at(&mut self.working, mark.bucket, mark.idx) {
                ent.key_values
                    .insert(opts.marker_key.clone(), mark.token.clone());
            }
        }

        let journal = Journal::new(&opts.tool, &opts.marker_key, ops);
        if !journal.is_empty() {
            marker::write(
                &mut self.working.world.key_values,
                &opts.world_key,
                Some(&marker::Mark {
                    tool: opts.tool.clone(),
                    fingerprint: journal.fingerprint(),
                }),
            );
        }
        Ok((self.working, journal))
    }
}

impl Deref for TrackedVmf {
    type Target = VmfFile;
    fn deref(&self) -> &Self::Target {
        &self.working
    }
}

impl DerefMut for TrackedVmf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.working
    }
}
