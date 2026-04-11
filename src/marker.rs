//! The tool's mark, stamped into `world`.
//!
//! Hammer preserves world keyvalues the same way it preserves `comment`, so one
//! line there survives the map being opened, edited and saved again:
//!
//! ```text
//! "vmf_ledger" "pseudo-ents 0.1.0 a1b2c3d4e5f60718"
//! ```
//!
//! What compiled the map, and which journal undoes it. `world` is a list a
//! mapper actually opens in Hammer, so one line of tooling noise there is the
//! whole budget.

use indexmap::IndexMap;

/// The tool's mark in `world`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// Tool and build in one string, spelled the way the journal spells it.
    pub tool: String,
    /// Ties the mark to one exact journal.
    pub fingerprint: String,
}

impl std::fmt::Display for Mark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.tool, self.fingerprint)
    }
}

/// Reads the mark. The fingerprint is always last, so the split happens from
/// the right and a tool name with spaces in it still resolves.
pub fn read(world: &IndexMap<String, String>, key: &str) -> Option<Mark> {
    let (tool, fingerprint) = world.get(key)?.trim().rsplit_once(' ')?;
    Some(Mark {
        tool: tool.to_string(),
        fingerprint: fingerprint.to_string(),
    })
}

/// Writes the mark back, dropping the key entirely once there is none.
pub fn write(world: &mut IndexMap<String, String>, key: &str, mark: Option<&Mark>) {
    match mark {
        // `insert` keeps an existing key where it is, so re-stamping does not
        // move the line around in `world` and show up as an edit.
        Some(mark) => {
            world.insert(key.to_string(), mark.to_string());
        }
        None => {
            world.shift_remove(key);
        }
    }
}
