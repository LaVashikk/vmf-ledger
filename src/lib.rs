//! Reversible VMF edits.
//!
//! Modify a map freely, then work out what changed and write enough beside the
//! file to undo exactly those changes later - and only those, so edits made by
//! anyone else in the meantime survive the rollback.
//!
//! ```no_run
//! # use source_vmf::prelude::*;
//! # use vmf_ledger::{LedgerOptions, TrackedVmf, sidecar::Sidecar};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let opts = LedgerOptions::new("my-tool", "1.0");
//! let mut map = TrackedVmf::new(VmfFile::open("map.vmf")?);
//!
//! for ent in map.entities.iter_mut() {
//!     ent.set("classname".into(), "func_detail".into());
//! }
//!
//! let (map, journal) = map.finish(&opts)?;
//! map.save("map.vmf")?;
//! journal.write(Sidecar::path_for("map.vmf"))?;
//! # Ok(()) }
//! ```
//!
//! # More than one tool
//!
//! Several tools can compile the same map, and none of them has to know about
//! the others. The name given to [`LedgerOptions::new`] is what keeps them
//! apart: it picks the tool's section in the shared `.vdif`, its record in the
//! map's marker, and the keyvalue that marks the entities it touched. Rolling
//! one tool back leaves the rest exactly where they were.
//!
//! The tool name acts as a persistent identifier across builds; version
//! information should be passed separately in `version`.

use std::ops::{Deref, DerefMut};

use source_vmf::prelude::*;

pub mod diff;
pub mod error;
pub mod marker;
pub mod matching;
pub mod ops;
pub mod rollback;
pub mod sidecar;

use diff::{Diff, Mark};
use marker::Record;
use ops::{Op, VisgroupOp};
use sidecar::Journal;

pub use error::LedgerError;
pub use rollback::{is_marked, restore, rewind};

/// Default key name for the tool registry in the `world` entity.
pub const DEFAULT_WORLD_KEY: &str = "vmf_ledger";

/// Key prefix for entity tracking markers.
pub const MARKER_PREFIX: &str = "_vl_";

#[derive(Debug, Clone)]
pub struct LedgerOptions {
    /// Unique tool identifier.
    pub name: String,
    /// Tool build version string.
    pub version: String,
    /// Key name for the registry record in the `world` entity.
    pub world_key: String,
    /// Key name stamped into modified entities.
    pub marker_key: String,
    /// Optional visgroup name for newly created entities.
    pub visgroup: Option<String>,
}

impl LedgerOptions {
    pub fn new(name: impl AsRef<str>, version: impl Into<String>) -> Self {
        let name = marker::slug(name.as_ref());
        Self {
            marker_key: format!("{MARKER_PREFIX}{name}"),
            world_key: DEFAULT_WORLD_KEY.to_string(),
            visgroup: None,
            name,
            version: version.into(),
        }
    }

    /// Overrides the entity marker key derived from the tool name.
    pub fn with_marker_key(mut self, key: impl Into<String>) -> Self {
        self.marker_key = key.into();
        self
    }

    /// Overrides the registry key name in the `world` entity.
    pub fn with_world_key(mut self, key: impl Into<String>) -> Self {
        self.world_key = key.into();
        self
    }

    /// Configures newly created entities to be placed into a visgroup.
    pub fn with_visgroup(mut self, name: impl Into<String>) -> Self {
        self.visgroup = Some(name.into());
        self
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RestoreOptions {
    /// Forces rollback even if guarded keyvalues or connections were modified concurrently.
    pub force: bool,
}

/// Tracks mutations to a VMF map by maintaining both original and working copies.
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

    /// Returns a reference to the unedited map state.
    pub fn original(&self) -> &VmfFile {
        &self.original
    }

    /// Borrows both the original and working copies simultaneously.
    pub fn split(&mut self) -> (&VmfFile, &mut VmfFile) {
        (&self.original, &mut self.working)
    }

    pub fn diff(&self) -> Diff {
        diff::diff(&self.original, &self.working)
    }

    /// Computes diffs, stamps marker keys, updates the world registry, and returns the modified map and journal.
    ///
    /// Returns [`LedgerError::Untracked`] if uninvertible changes outside entity scopes were detected.
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

        // Executed after diff: visgroup assignment applies only to new entities,
        // which are deleted entirely during rollback.
        let visgroups = self.stamp_visgroup(opts, &ops, &marks);

        let journal = Journal::new(&opts.name, &opts.version, &opts.marker_key, ops, visgroups);
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

    // Places newly created entities into the configured visgroup, creating it if absent.
    fn stamp_visgroup(
        &mut self,
        opts: &LedgerOptions,
        ops: &[Op],
        marks: &[Mark],
    ) -> Vec<VisgroupOp> {
        let Some(name) = &opts.visgroup else {
            return Vec::new();
        };
        let added: std::collections::HashSet<&str> = ops
            .iter()
            .filter_map(|op| match op {
                Op::AddEntity { at, .. } => Some(at.as_str()),
                _ => None,
            })
            .collect();
        if added.is_empty() {
            return Vec::new();
        }

        let existing = self
            .working
            .visgroups
            .find_by_name(name)
            .map(|group| group.id);
        let (id, created) = match existing {
            Some(id) => (id, false),
            None => (self.working.visgroups.create(name.clone()), true),
        };

        for mark in marks.iter().filter(|m| added.contains(m.token.as_str())) {
            if let Some(ent) = rollback::entity_at(&mut self.working, mark.bucket, mark.idx) {
                ent.editor.join_visgroup(id);
            }
        }

        // Record visgroup deletion op only if the visgroup was created during this pass.
        match created {
            true => vec![VisgroupOp::Add {
                id,
                name: name.clone(),
            }],
            false => Vec::new(),
        }
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
