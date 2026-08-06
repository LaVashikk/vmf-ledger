//! Replaying a journal backwards.
//!
//! Only the spots the journal names are touched, so unrelated edits made in the
//! meantime - a retextured brush, a new entity - are left alone.

use vmf_forge::prelude::*;

use crate::error::LedgerError;
use crate::marker;
use crate::ops::{self, Bucket, Collision, Op, Token, VisgroupOp};
use crate::sidecar::{Journal, Sidecar};
use crate::{LedgerOptions, RestoreOptions};

/// True when this tool has compiled the map before.
pub fn is_marked(map: &VmfFile, opts: &LedgerOptions) -> bool {
    let marks = marker::read(&map.world.key_values, &opts.world_key);
    marker::find(&marks, &opts.name).is_some()
}

/// Undoes the tool's edits in place.
pub fn restore(
    map: &mut VmfFile,
    journal: &Journal,
    opts: &LedgerOptions,
    restore_opts: &RestoreOptions,
) -> Result<(), LedgerError> {
    let mut marks = marker::read(&map.world.key_values, &opts.world_key);
    let in_map = marker::find(&marks, &journal.name)
        .ok_or_else(|| LedgerError::NotMarked(journal.name.clone()))?
        .fingerprint
        .clone();

    let in_journal = journal.fingerprint();
    if in_map != in_journal {
        return Err(LedgerError::Mismatched {
            name: journal.name.clone(),
            in_map,
            in_journal,
        });
    }

    let index = index_markers(map, &journal.marker_key)?;

    // Checked in full before anything is written: a rollback that bailed out
    // halfway would leave the map in a state no journal describes.
    let collisions = check(map, journal, &index);
    if !collisions.is_empty() && !restore_opts.force {
        return Err(LedgerError::Collisions(collisions));
    }

    apply(map, journal, &index);
    undo_visgroups(map, journal);
    strip_markers(map, &journal.marker_key);

    marker::remove(&mut marks, &journal.name);
    marker::write(&mut map.world.key_values, &opts.world_key, &marks);
    Ok(())
}

/// Rolls a map back to the state it had before the tool last touched it.
///
/// The common entry point: a compiler calls this on load so it always works
/// from a map without its own previous output in it, whether or not it has
/// compiled this one before.
pub fn rewind(
    map: &mut VmfFile,
    map_path: impl AsRef<std::path::Path>,
    opts: &LedgerOptions,
    restore_opts: &RestoreOptions,
) -> Result<bool, LedgerError> {
    let marks = marker::read(&map.world.key_values, &opts.world_key);
    let Some(mark) = marker::find(&marks, &opts.name) else {
        return Ok(false);
    };
    let mark = mark.to_string();

    // The map says we compiled it, so compiling again without rolling back
    // first would build on top of the previous output. Worth its own error:
    // "no such file" tells a mapper nothing about a file they never heard of.
    let path = Sidecar::path_for(map_path);
    if !path.exists() {
        return Err(LedgerError::JournalMissing { path, marker: mark });
    }

    let Some(journal) = Journal::read(&path, &opts.name)? else {
        return Err(LedgerError::SectionMissing {
            path,
            name: opts.name.clone(),
        });
    };
    restore(map, &journal, opts, restore_opts)?;
    Ok(true)
}

type Index = std::collections::HashMap<Token, (Bucket, usize)>;

fn index_markers(map: &VmfFile, marker_key: &str) -> Result<Index, LedgerError> {
    let mut index = Index::new();
    for (bucket, list) in [
        (Bucket::Entity, &map.entities.0),
        (Bucket::Hidden, &map.hiddens.0),
    ] {
        for (idx, ent) in list.iter().enumerate() {
            let Some(token) = ent.key_values.get(marker_key) else {
                continue;
            };
            if index.insert(token.clone(), (bucket, idx)).is_some() {
                return Err(LedgerError::DuplicateMarker(token.clone()));
            }
        }
    }
    Ok(index)
}

fn resolve<'a>(map: &'a mut VmfFile, index: &Index, token: &str) -> Option<&'a mut Entity> {
    let (bucket, idx) = *index.get(token)?;
    entity_at(map, bucket, idx)
}

pub(crate) fn entity_at(map: &mut VmfFile, bucket: Bucket, idx: usize) -> Option<&mut Entity> {
    match bucket {
        Bucket::Entity => map.entities.0.get_mut(idx),
        Bucket::Hidden => map.hiddens.0.get_mut(idx),
    }
}

fn find<'a>(map: &'a VmfFile, index: &Index, token: &str) -> Option<&'a Entity> {
    let (bucket, idx) = *index.get(token)?;
    match bucket {
        Bucket::Entity => map.entities.0.get(idx),
        Bucket::Hidden => map.hiddens.0.get(idx),
    }
}

fn collision(token: &str, what: &str, expected: &str, found: &str) -> Collision {
    Collision {
        token: token.to_string(),
        what: what.to_string(),
        expected: expected.to_string(),
        found: found.to_string(),
    }
}

fn check(map: &VmfFile, journal: &Journal, index: &Index) -> Vec<Collision> {
    const GONE: &str = "<absent>";
    let mut out = Vec::new();

    for op in &journal.ops {
        // A re-inserted entity is not in the map yet, by definition.
        if matches!(op, Op::RemoveEntity { .. }) {
            continue;
        }
        let Some(ent) = find(map, index, op.token()) else {
            out.push(collision(op.token(), "entity", "present", GONE));
            continue;
        };

        match op {
            Op::Set { key, new, .. } | Op::Add { key, new, .. } => {
                let found = ent.key_values.get(key).map(String::as_str).unwrap_or(GONE);
                if found != new {
                    out.push(collision(op.token(), key, new, found));
                }
            }
            Op::Remove { key, .. } => {
                if let Some(found) = ent.key_values.get(key) {
                    out.push(collision(op.token(), key, GONE, found));
                }
            }
            Op::Connections { new, .. } => {
                // Compared as the file holds them, so the guard behaves the
                // same whether the map came off disk or straight from a pass.
                let found = ops::as_written(ent.connections.as_ref());
                if &found != new {
                    out.push(collision(
                        op.token(),
                        "connections",
                        &describe(new),
                        &describe(&found),
                    ));
                }
            }
            Op::AddEntity { .. } | Op::RemoveEntity { .. } => {}
        }
    }
    out
}

fn describe(connections: &Option<Vec<Connection>>) -> String {
    match connections {
        None => "none".to_string(),
        Some(list) => format!("{} connection(s)", list.len()),
    }
}

fn apply(map: &mut VmfFile, journal: &Journal, index: &Index) {
    // Keyvalue work first: it addresses entities by index, and adding or
    // removing entities invalidates every index after it.
    //
    // Order inside an entity is not free. `Op::Remove` carries the index the key
    // sat at in the original, and that index only means anything once the keys
    // the tool added are gone and the earlier keys are already back. So: values,
    // then deletions, then insertions in ascending order.
    for op in &journal.ops {
        let Some(ent) = resolve(map, index, op.token()) else {
            continue;
        };
        match op {
            Op::Set { key, old, .. } => {
                ent.key_values.insert(key.clone(), old.clone());
            }
            Op::Connections { old, .. } => ent.connections = old.clone(),
            _ => {}
        }
    }

    for op in &journal.ops {
        if let Op::Add { key, .. } = op
            && let Some(ent) = resolve(map, index, op.token())
        {
            ent.key_values.shift_remove(key);
        }
    }

    let mut restored: Vec<(&Token, usize, &String, &String)> = journal
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Remove { at, idx, key, old } => Some((at, *idx, key, old)),
            _ => None,
        })
        .collect();
    restored.sort_unstable_by_key(|&(_, idx, ..)| idx);
    for (token, idx, key, old) in restored {
        let Some(ent) = resolve(map, index, token) else {
            continue;
        };
        let at = idx.min(ent.key_values.len());
        ent.key_values.shift_insert(at, key.clone(), old.clone());
    }

    for op in &journal.ops {
        if let Op::AddEntity { .. } = op
            && let Some(&(bucket, idx)) = index.get(op.token())
        {
            let list = list_mut(map, bucket);
            if idx < list.len() {
                list.remove(idx);
            }
        }
    }

    for op in &journal.ops {
        if let Op::RemoveEntity {
            bucket,
            idx,
            entity,
            ..
        } = op
        {
            let list = list_mut(map, *bucket);
            let at = (*idx).min(list.len());
            list.insert(at, entity.clone());
        }
    }
}

/// Takes back the visgroups the tool made.
///
/// Runs after [`apply`], so the entities the tool created - and their
/// membership with them - are already gone by the time occupancy is counted.
fn undo_visgroups(map: &mut VmfFile, journal: &Journal) {
    for op in &journal.visgroups {
        match op {
            VisgroupOp::Add { id, .. } => {
                // Somebody moved their own work in here. The group was ours to
                // create, not to take away with someone else still in it, and
                // deleting it would strand them in `_orphaned hidden`.
                if !occupied(map, *id) {
                    map.visgroups.remove_by_id(*id);
                }
            }
        }
    }
}

fn occupied(map: &VmfFile, id: i32) -> bool {
    let entities = map.entities.0.iter().chain(map.hiddens.0.iter());
    let solids = map.world.solids.iter().chain(map.world.hidden.iter());
    entities
        .map(|ent| &ent.editor)
        .chain(solids.map(|s| &s.editor))
        .any(|editor| editor.visgroup_ids.contains(&id))
}

fn list_mut(map: &mut VmfFile, bucket: Bucket) -> &mut Vec<Entity> {
    match bucket {
        Bucket::Entity => &mut map.entities.0,
        Bucket::Hidden => &mut map.hiddens.0,
    }
}

fn strip_markers(map: &mut VmfFile, marker_key: &str) {
    for ent in map.entities.0.iter_mut().chain(map.hiddens.0.iter_mut()) {
        ent.key_values.shift_remove(marker_key);
    }
}
