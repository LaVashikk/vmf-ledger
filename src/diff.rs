//! Working out what a tool changed, by comparing the map to its pristine copy.
//!
//! Only entities are tracked. That is not a shortcut around hard work but the
//! honest boundary of what can be rolled back: `Side` and friends are typed
//! structs rather than keyvalue maps, so a generic op cannot address a field
//! inside them. Anything else that changed is reported as untracked and the
//! caller is expected to refuse the export rather than promise a rollback it
//! cannot deliver.

use indexmap::IndexMap;
use vmf_forge::VmfBlock;
use vmf_forge::prelude::*;

use crate::matching::{MatchOptions, match_blocks};
use crate::ops::{Bucket, Op, Token, as_written};

/// Where a marker keyvalue has to be written for a rollback to find the entity
/// again.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub bucket: Bucket,
    /// Index into the working file's list.
    pub idx: usize,
    pub token: Token,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Diff {
    pub ops: Vec<Op>,
    pub marks: Vec<Mark>,
    /// Human-readable descriptions of changes outside the tracked scope.
    pub untracked: Vec<String>,
}

pub fn diff(original: &VmfFile, working: &VmfFile) -> Diff {
    let mut out = Diff::default();
    let mut tokens = Tokens::default();

    diff_bucket(
        Bucket::Entity,
        &original.entities,
        &working.entities,
        &mut tokens,
        &mut out,
    );
    diff_bucket(
        Bucket::Hidden,
        &original.hiddens,
        &working.hiddens,
        &mut tokens,
        &mut out,
    );
    note_untracked(original, working, &mut out.untracked);

    out
}

/// Sequential, so a sidecar reads the same way twice for the same input.
#[derive(Default)]
struct Tokens(u32);

impl Tokens {
    fn next(&mut self) -> Token {
        self.0 += 1;
        self.0.to_string()
    }
}

fn diff_bucket(
    bucket: Bucket,
    original: &[Entity],
    working: &[Entity],
    tokens: &mut Tokens,
    out: &mut Diff,
) {
    // Matching only reads keyvalues, so the children are left off: converting
    // whole entities here would clone every solid in the map for nothing.
    let old_blocks: Vec<VmfBlock> = original.iter().map(match_view).collect();
    let new_blocks: Vec<VmfBlock> = working.iter().map(match_view).collect();
    let matching = match_blocks(&old_blocks, &new_blocks, &MatchOptions::default());

    for pair in &matching.pairs {
        let (before, after) = (&original[pair.old], &working[pair.new]);
        let mut ops = Vec::new();

        diff_key_values(&before.key_values, &after.key_values, &mut ops);
        let (old, new) = (
            as_written(before.connections.as_ref()),
            as_written(after.connections.as_ref()),
        );
        if old != new {
            ops.push(Op::Connections {
                at: String::new(),
                old,
                new,
            });
        }
        note_untracked_entity(before, after, &mut out.untracked);

        if ops.is_empty() {
            continue;
        }

        let token = tokens.next();
        for op in &mut ops {
            set_token(op, &token);
        }
        out.ops.extend(ops);
        out.marks.push(Mark {
            bucket,
            idx: pair.new,
            token,
        });
    }

    for &idx in &matching.only_new {
        let token = tokens.next();
        out.ops.push(Op::AddEntity {
            at: token.clone(),
            bucket,
        });
        out.marks.push(Mark { bucket, idx, token });
    }

    for &idx in &matching.only_old {
        out.ops.push(Op::RemoveEntity {
            at: tokens.next(),
            bucket,
            idx,
            entity: original[idx].clone(),
        });
    }
}

fn match_view(ent: &Entity) -> VmfBlock {
    VmfBlock {
        name: "entity".to_string(),
        key_values: ent.key_values.clone(),
        blocks: Vec::new(),
    }
}

fn diff_key_values(
    before: &IndexMap<String, String>,
    after: &IndexMap<String, String>,
    ops: &mut Vec<Op>,
) {
    for (idx, (key, old)) in before.iter().enumerate() {
        match after.get(key) {
            Some(new) if new != old => ops.push(Op::Set {
                at: String::new(),
                key: key.clone(),
                old: old.clone(),
                new: new.clone(),
            }),
            Some(_) => {}
            None => ops.push(Op::Remove {
                at: String::new(),
                idx,
                key: key.clone(),
                old: old.clone(),
            }),
        }
    }

    for (key, new) in after {
        if !before.contains_key(key) {
            ops.push(Op::Add {
                at: String::new(),
                key: key.clone(),
                new: new.clone(),
            });
        }
    }
}

/// Tokens are only known once an entity is known to have changed, so ops are
/// built with an empty one and stamped afterwards.
fn set_token(op: &mut Op, token: &str) {
    match op {
        Op::Set { at, .. }
        | Op::Add { at, .. }
        | Op::Remove { at, .. }
        | Op::Connections { at, .. }
        | Op::AddEntity { at, .. }
        | Op::RemoveEntity { at, .. } => *at = token.to_string(),
    }
}

fn note_untracked_entity(before: &Entity, after: &Entity, out: &mut Vec<String>) {
    let id = before.id();
    if before.solids != after.solids {
        out.push(format!("entity {id}: solids changed"));
    }
    if before.editor != after.editor {
        out.push(format!("entity {id}: editor block changed"));
    }
    if before.extra != after.extra {
        out.push(format!("entity {id}: unmodelled blocks changed"));
    }
}

fn note_untracked(original: &VmfFile, working: &VmfFile, out: &mut Vec<String>) {
    let sections: [(&str, bool); 7] = [
        ("world", original.world != working.world),
        ("versioninfo", original.versioninfo != working.versioninfo),
        ("visgroups", original.visgroups != working.visgroups),
        (
            "viewsettings",
            original.viewsettings != working.viewsettings,
        ),
        ("cameras", original.cameras != working.cameras),
        ("cordons", original.cordons != working.cordons),
        (
            "extra blocks",
            original.extra_blocks != working.extra_blocks,
        ),
    ];
    for (name, changed) in sections {
        if changed {
            out.push(format!("{name} changed"));
        }
    }
}
