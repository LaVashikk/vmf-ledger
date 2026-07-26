//! The registry of tools stamped into `world`.
//!
//! Several tools can compile the same map, so the mark is a list and not a
//! single record: each tool owns one entry, reads the others without touching
//! them, and drops only its own on rollback.
//!
//! One `world` keyvalue holds the lot:
//!
//! ```text
//! "vmf_ledger" "pseudo-ents 0.1.0 a1b2c3d4e5f60718; cube-init 1.0 0011223344556677"
//! ```
//!
//! A key per tool would work as well, but `world` is a list a mapper actually
//! opens in Hammer, and one line of tooling noise there is enough. The order of
//! the records is the order the tools stamped the map in.

use indexmap::IndexMap;

const RECORD_SEP: char = ';';

/// One tool's record in `world`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    /// Informational only: what wrote this. Never part of the tool's identity,
    /// or a version bump would orphan every map compiled by the previous one.
    pub version: String,
    /// Ties the record to one exact journal section.
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

/// A name that survives a trip through the registry and a keyvalue name.
///
/// Whitespace separates the fields of a record and `;` separates the records,
/// so a tool called `My Tool` would silently corrupt both. Folding them into
/// `_` costs nothing and keeps the failure impossible rather than rare.
pub fn slug(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == RECORD_SEP || c == '"' {
                '_'
            } else {
                c
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("unnamed");
    }
    out
}

/// Reads the registry. An unparsable record is dropped rather than fought over:
/// what matters is finding our own, and a record we cannot read is not ours.
pub fn read(world: &IndexMap<String, String>, key: &str) -> Vec<Record> {
    let Some(value) = world.get(key) else {
        return Vec::new();
    };
    value.split(RECORD_SEP).filter_map(parse_record).collect()
}

/// Writes the registry back, dropping the key entirely once nobody is left.
pub fn write(world: &mut IndexMap<String, String>, key: &str, marks: &[Record]) {
    if marks.is_empty() {
        world.shift_remove(key);
        return;
    }
    let text = marks
        .iter()
        .map(Record::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    // `insert` keeps an existing key where it is, so re-stamping does not move
    // the line around in `world` and show up as an edit.
    world.insert(key.to_string(), text);
}

pub fn find<'a>(marks: &'a [Record], name: &str) -> Option<&'a Record> {
    marks.iter().find(|mark| mark.name == name)
}

/// Adds the record, or replaces the one this tool left last time.
pub fn upsert(marks: &mut Vec<Record>, mark: Record) {
    match marks.iter_mut().find(|it| it.name == mark.name) {
        Some(slot) => *slot = mark,
        None => marks.push(mark),
    }
}

pub fn remove(marks: &mut Vec<Record>, name: &str) {
    marks.retain(|mark| mark.name != name);
}

/// A record is `name version fingerprint`, and the fingerprint is always last.
///
/// Read from the right, so a name with spaces in it - written by a build before
/// [`slug`] existed, or by hand - still resolves to the right fingerprint.
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
