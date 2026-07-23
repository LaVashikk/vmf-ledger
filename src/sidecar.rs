//! The `.vdif` journal that lives next to the map.
//!
//! Hammer rewrites a map from the blocks it knows and drops everything else, so
//! a journal kept inside the VMF would not survive the map being opened once.
//! Only markers stay in the file; the operations themselves live here.
//!
//! JSON rather than KeyValues: op values are arbitrary original keyvalues, and
//! `source-kv` writes a value as `"{}"` with no escaping, so a quote or a
//! newline in one would corrupt the file.
//!
//! One file, one section per tool. Two compilers on the same map would
//! otherwise take turns overwriting each other's only way back, and the second
//! one would not even notice. Writing is therefore read-modify-write: a tool
//! replaces its own section and leaves the rest of the file exactly as it
//! found it.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::LedgerError;
use crate::ops::Op;

pub const EXTENSION: &str = "vdif";
const FORMAT_VERSION: u32 = 2;

/// One tool's half of the journal: everything needed to undo that tool's last
/// run, and nothing about anyone else's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    /// The tool's stable identity, matching its record in the map's marker.
    pub name: String,
    /// Informational: which build wrote this.
    pub version: String,
    /// The per-entity marker key, so a rollback needs nothing but this section.
    pub marker_key: String,
    pub ops: Vec<Op>,
}

impl Journal {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        marker_key: impl Into<String>,
        ops: Vec<Op>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            marker_key: marker_key.into(),
            ops,
        }
    }

    /// Nothing to undo, so nothing worth writing or marking.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Ties the tool's record in the map to this exact section.
    ///
    /// Taken over the section alone, never the whole file: another tool
    /// compiling the same map appends its own section, and that must not
    /// invalidate a rollback that has nothing to do with it.
    pub fn fingerprint(&self) -> String {
        format!("{:016x}", fnv1a(&to_json(self)))
    }

    /// This tool's section of the journal beside the map, if it has one.
    pub fn read(path: impl AsRef<Path>, name: &str) -> Result<Option<Self>, LedgerError> {
        Ok(Sidecar::read(path)?.take(name))
    }

    /// Merges this section into the file, leaving every other tool's alone.
    pub fn write(&self, path: impl AsRef<Path>) -> Result<(), LedgerError> {
        let path = path.as_ref();
        let mut sidecar = match Sidecar::read(path) {
            Ok(sidecar) => sidecar,
            // No file yet is the normal first compile.
            Err(LedgerError::SidecarIo { source, .. }) if source.kind() == ErrorKind::NotFound => {
                Sidecar::default()
            }
            Err(other) => return Err(other),
        };
        sidecar.upsert(self.clone());
        sidecar.write(path)
    }
}

/// The `.vdif` file: a format version and the tools that have compiled the map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    /// A list rather than a map keyed by name: the name already lives in the
    /// section, and two of them would be one more thing that can disagree. The
    /// order is the order the tools first wrote themselves in.
    pub tools: Vec<Journal>,
}

impl Default for Sidecar {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            tools: Vec::new(),
        }
    }
}

impl Sidecar {
    /// `maps/foo.vmf` -> `maps/foo.vdif`.
    pub fn path_for(vmf_path: impl AsRef<Path>) -> PathBuf {
        vmf_path.as_ref().with_extension(EXTENSION)
    }

    pub fn get(&self, name: &str) -> Option<&Journal> {
        self.tools.iter().find(|journal| journal.name == name)
    }

    fn take(self, name: &str) -> Option<Journal> {
        self.tools.into_iter().find(|journal| journal.name == name)
    }

    /// Adds a section, or replaces the one that tool wrote last time.
    pub fn upsert(&mut self, journal: Journal) {
        match self.tools.iter_mut().find(|it| it.name == journal.name) {
            Some(slot) => *slot = journal,
            None => self.tools.push(journal),
        }
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| LedgerError::SidecarIo {
            path: path.to_path_buf(),
            source,
        })?;
        let sidecar: Self =
            serde_json::from_str(&text).map_err(|source| LedgerError::SidecarFormat {
                path: path.to_path_buf(),
                source,
            })?;
        if sidecar.version != FORMAT_VERSION {
            return Err(LedgerError::SidecarVersion {
                found: sidecar.version,
                expected: FORMAT_VERSION,
            });
        }
        Ok(sidecar)
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<(), LedgerError> {
        let path = path.as_ref();
        fs::write(path, self.to_json()).map_err(|source| LedgerError::SidecarIo {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn to_json(&self) -> String {
        to_json(self)
    }
}

/// Pretty-printed on purpose: the file is meant to be readable when a rollback
/// goes wrong, and nobody pays for the bytes of a sidecar.
fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("ops are plain data")
}

/// FNV-1a. `DefaultHasher` is explicitly not stable across Rust releases, and
/// this value is written to disk.
fn fnv1a(data: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
