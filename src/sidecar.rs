//! The `.vdif` journal that lives next to the map.
//!
//! Hammer rewrites a map from the blocks it knows and drops everything else, so
//! a journal kept inside the VMF would not survive the map being opened once.
//! Only the marker stays in the file; the operations themselves live here.
//!
//! JSON rather than KeyValues: op values are arbitrary original keyvalues, and
//! `source-kv` writes a value as `"{}"` with no escaping, so a quote or a
//! newline in one would corrupt the file.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::LedgerError;
use crate::ops::Op;

pub const EXTENSION: &str = "vdif";
const FORMAT_VERSION: u32 = 1;

/// Everything needed to undo the tool's last run over one map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    /// Tool and build in one string, exactly as it goes into the map's marker.
    pub tool: String,
    /// The per-entity marker key, so a rollback needs nothing but this file.
    pub marker_key: String,
    pub ops: Vec<Op>,
}

impl Journal {
    pub fn new(tool: impl Into<String>, marker_key: impl Into<String>, ops: Vec<Op>) -> Self {
        Self {
            version: FORMAT_VERSION,
            tool: tool.into(),
            marker_key: marker_key.into(),
            ops,
        }
    }

    /// Nothing to undo, so nothing worth writing or marking.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// `maps/foo.vmf` -> `maps/foo.vdif`.
    pub fn path_for(vmf_path: impl AsRef<Path>) -> PathBuf {
        vmf_path.as_ref().with_extension(EXTENSION)
    }

    /// Ties the tool's mark in the map to this exact journal, so a rollback can
    /// tell its own journal from one left over by another run.
    pub fn fingerprint(&self) -> String {
        format!("{:016x}", fnv1a(&self.to_json()))
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| LedgerError::SidecarIo {
            path: path.to_path_buf(),
            source,
        })?;
        let journal: Self =
            serde_json::from_str(&text).map_err(|source| LedgerError::SidecarFormat {
                path: path.to_path_buf(),
                source,
            })?;
        if journal.version != FORMAT_VERSION {
            return Err(LedgerError::SidecarVersion {
                found: journal.version,
                expected: FORMAT_VERSION,
            });
        }
        Ok(journal)
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
