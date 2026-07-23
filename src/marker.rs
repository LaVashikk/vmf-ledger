//! The tool's mark, stamped into `world`.
//!
//! Hammer preserves world keyvalues the same way it preserves `comment`, so one
//! line there survives the map being opened, edited and saved again:
//!
//! ```text
//! "vmf_ledger" "pseudo-ents 0.1.0 a1b2c3d4e5f60718"
//! ```
//!
//! Who compiled the map, which build, and which journal undoes it. `world` is a
//! list a mapper actually opens in Hammer, so one line of tooling noise there is
//! the whole budget.

use indexmap::IndexMap;

/// The tool's record in `world`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    /// Informational only: what wrote this. Never part of the tool's identity,
    /// or a version bump would orphan every map compiled by the previous one.
    pub version: String,
    /// Ties the record to one exact journal.
    pub fingerprint: String,
}

impl std::fmt::Display for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.version.is_empty() {
            write!(f, "{} {}", self.name, self.fingerprint)
        } else {
            write!(f, "{} {} {}", self.name, self.version, self.fingerprint)
        }
    }
}

pub fn read(world: &IndexMap<String, String>, key: &str) -> Option<Record> {
    parse_record(world.get(key)?)
}

/// Writes the record back, dropping the key entirely once there is none.
pub fn write(world: &mut IndexMap<String, String>, key: &str, mark: Option<&Record>) {
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

/// A record is `name version fingerprint`, and the fingerprint is always last.
///
/// Read from the right, so a name with spaces in it - written by an earlier
/// build, or by hand - still resolves to the right fingerprint.
fn parse_record(text: &str) -> Option<Record> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    match fields.len() {
        0 | 1 => None,
        2 => Some(Record {
            name: fields[0].to_string(),
            version: String::new(),
            fingerprint: fields[1].to_string(),
        }),
        n => Some(Record {
            name: fields[..n - 2].join(" "),
            version: fields[n - 2].to_string(),
            fingerprint: fields[n - 1].to_string(),
        }),
    }
}
