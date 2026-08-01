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
use marker::Record;
use sidecar::Journal;

pub use error::LedgerError;
pub use rollback::{is_marked, restore, rewind};

/// Default key written into `world`. Hammer preserves world keyvalues the same
/// way it preserves `comment`.
pub const DEFAULT_WORLD_KEY: &str = "vmf_ledger";

/// What a derived marker key starts with. Short, and prefixed so it reads as
/// tooling when a mapper stumbles over it in Hammer.
pub const MARKER_PREFIX: &str = "_vl_";

#[derive(Debug, Clone)]
pub struct LedgerOptions {
    /// The tool's identity. Everything else is derived from it, so two tools
    /// with different names never write over each other.
    pub name: String,
    /// Which build. Recorded in the map and the journal for a human to read,
    /// and deliberately kept out of the identity.
    pub version: String,
    /// Key of the tool registry in `world`. Shared with every other tool, and
    /// the one thing that has no business being per-tool.
    pub world_key: String,
    /// Key marking the entities this tool touched.
    pub marker_key: String,
}

impl LedgerOptions {
    pub fn new(name: impl AsRef<str>, version: impl Into<String>) -> Self {
        let name = marker::slug(name.as_ref());
        Self {
            marker_key: format!("{MARKER_PREFIX}{name}"),
            world_key: DEFAULT_WORLD_KEY.to_string(),
            name,
            version: version.into(),
        }
    }

    /// Overrides the key derived from the name. For a tool that has already
    /// shipped under a different one and does not want to strand its maps.
    pub fn with_marker_key(mut self, key: impl Into<String>) -> Self {
        self.marker_key = key.into();
        self
    }

    /// Leaves the shared registry for one of its own. A tool that does this
    /// becomes invisible to every other tool's bookkeeping, which is only what
    /// you want if it is not sharing the map in the first place.
    pub fn with_world_key(mut self, key: impl Into<String>) -> Self {
        self.world_key = key.into();
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

        let journal = Journal::new(
            &opts.name,
            &opts.version,
            &opts.marker_key,
            ops,
            Vec::new(),
        );
        if !journal.is_empty() {
            let mut registry = marker::read(&self.working.world.key_values, &opts.world_key);
            marker::upsert(
                &mut registry,
                Record {
                    name: opts.name.clone(),
                    version: opts.version.clone(),
                    fingerprint: journal.fingerprint(),
                },
            );
            marker::write(
                &mut self.working.world.key_values,
                &opts.world_key,
                &registry,
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
