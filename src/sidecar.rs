//! Serialization of `.vdif` journal sidecar files.
//!
//! Stored alongside VMF maps because Hammer discards unrecognised blocks on save.
//!
//! Uses JSON instead of KeyValues because `source-kv` does not escape quotes or newlines
//! in values. Multiple tools share a single sidecar using section-based read-modify-write.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::LedgerError;
use crate::ops::{Op, VisgroupOp};

pub const EXTENSION: &str = "vdif";
const FORMAT_VERSION: u32 = 2;

/// Rollback operations and metadata for a single tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    /// The tool's stable identity, matching its record in the map's marker.
    pub name: String,
    /// Informational: which build wrote this.
    pub version: String,
    /// The per-entity marker key, so a rollback needs nothing but this section.
    pub marker_key: String,
    pub ops: Vec<Op>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visgroups: Vec<VisgroupOp>,

    /// Set only when this section was lifted out of a v1 file. See [`V1::lift`].
    #[serde(skip)]
    legacy_fingerprint: Option<String>,
}

impl Journal {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        marker_key: impl Into<String>,
        ops: Vec<Op>,
        visgroups: Vec<VisgroupOp>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            marker_key: marker_key.into(),
            ops,
            visgroups,
            legacy_fingerprint: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.visgroups.is_empty()
    }

    /// Computes a hash identifying this journal section.
    ///
    /// Calculated over this tool's section alone so concurrent tool records in the
    /// shared sidecar do not invalidate map markers.
    pub fn fingerprint(&self) -> String {
        match &self.legacy_fingerprint {
            Some(carried) => carried.clone(),
            None => format!("{:016x}", fnv1a(&to_json(self))),
        }
    }

    /// Reads the journal section for `name` from the sidecar at `path`.
    pub fn read(path: impl AsRef<Path>, name: &str) -> Result<Option<Self>, LedgerError> {
        Ok(Sidecar::read(path)?.take(name))
    }

    /// Writes this journal section into the sidecar at `path`, preserving other tool sections.
    pub fn write(&self, path: impl AsRef<Path>) -> Result<(), LedgerError> {
        let path = path.as_ref();
        let mut sidecar = match Sidecar::read(path) {
            Ok(sidecar) => sidecar,
            Err(LedgerError::SidecarIo { source, .. }) if source.kind() == ErrorKind::NotFound => {
                Sidecar::default()
            }
            // Refuse to overwrite corrupt or unreadable sidecar to prevent data loss.
            Err(source) => {
                return Err(LedgerError::SidecarClobber {
                    path: path.to_path_buf(),
                    reason: source.to_string(),
                });
            }
        };
        sidecar.upsert(self.clone());
        sidecar.write(path)
    }
}

/// Top-level `.vdif` sidecar holding version and tool sections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    /// Tool journal sections stored in sequential insertion order.
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
    /// Returns the companion `.vdif` path for the given VMF file path.
    pub fn path_for(vmf_path: impl AsRef<Path>) -> PathBuf {
        vmf_path.as_ref().with_extension(EXTENSION)
    }

    pub fn get(&self, name: &str) -> Option<&Journal> {
        self.tools.iter().find(|journal| journal.name == name)
    }

    fn take(self, name: &str) -> Option<Journal> {
        self.tools.into_iter().find(|journal| journal.name == name)
    }

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
        let format = |source| LedgerError::SidecarFormat {
            path: path.to_path_buf(),
            source,
        };

        // Inspect format version before deserializing payload.
        #[derive(Deserialize)]
        struct Probe {
            version: u32,
        }
        let probe: Probe = serde_json::from_str(&text).map_err(format)?;

        match probe.version {
            1 => Ok(Self {
                version: FORMAT_VERSION,
                tools: vec![serde_json::from_str::<V1>(&text).map_err(format)?.lift()],
            }),
            FORMAT_VERSION => serde_json::from_str(&text).map_err(format),
            found => Err(LedgerError::SidecarVersion {
                found,
                expected: FORMAT_VERSION,
            }),
        }
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

// Legacy single-tool journal format (v1).
#[derive(Serialize, Deserialize)]
struct V1 {
    version: u32,
    tool: String,
    marker_key: String,
    ops: Vec<Op>,
}

impl V1 {
    fn lift(self) -> Journal {
        // Preserve original v1 fingerprint so existing map markers remain valid.
        let carried = format!("{:016x}", fnv1a(&to_json(&self)));

        // v1 combined name and version into a single space-separated string.
        let (name, version) = match self.tool.rsplit_once(' ') {
            Some((name, version)) => (name.to_string(), version.to_string()),
            None => (self.tool.clone(), String::new()),
        };

        Journal {
            name,
            version,
            marker_key: self.marker_key,
            ops: self.ops,
            visgroups: Vec::new(),
            legacy_fingerprint: Some(carried),
        }
    }
}

// Formatted with indentation for readability on disk.
fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("ops are plain data")
}

// 64-bit FNV-1a hash; provides deterministic hashes across Rust compiler releases.
fn fnv1a(data: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
