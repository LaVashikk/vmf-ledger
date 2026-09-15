//! Computes differences between pristine and modified VMF maps to produce rollback operations.
//!
//! Only entity keyvalues and connections are tracked. Changes to typed structures
//! (`Side`, `Solid`, top-level map properties) cannot be expressed as generic inverse
//! operations and are reported in [`Diff::untracked`].

use indexmap::IndexMap;
use source_vmf::VmfBlock;
use source_vmf::prelude::*;

use crate::matching::{Confidence, MatchOptions, match_blocks};
use crate::ops::{Bucket, Op, Token, as_written};

/// Target entity location and token for stamping marker keyvalues.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub bucket: Bucket,
    pub idx: usize,
    pub token: Token,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Diff {
    pub ops: Vec<Op>,
    pub marks: Vec<Mark>,
    /// Descriptions of changes outside the tracked scope.
    pub untracked: Vec<String>,
}

/// Computes operations to transform `working` back into `original`.
///
/// Returns operations, entity marker targets, and descriptions of unsupported changes.
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

// Deterministic sequential token generator.
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
    // Extract keyvalues only; avoids cloning geometry solids during block matching.
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

        // Rollback requires deterministic pairing. Matches below Signature confidence
        // are rejected to avoid applying operations to unrelated entities.
        if pair.confidence < Confidence::Signature {
            out.untracked.push(format!(
                "entity {}: matched only by similarity ({:.2}), too weak to roll back",
                before.id(),
                pair.score
            ));
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
            entity: Box::new(original[idx].clone()),
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

// Operations are initialized with empty tokens and stamped once entity assignment is determined.
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
