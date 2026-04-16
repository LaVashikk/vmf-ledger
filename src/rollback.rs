//! Replaying a journal backwards.
//!
//! Only the spots the journal names are touched, so unrelated edits made in the
//! meantime - a retextured brush, a new entity - are left alone.

use vmf_forge::prelude::*;

use crate::LedgerOptions;
use crate::error::LedgerError;
use crate::marker;
use crate::ops::{Bucket, Op, Token};
use crate::sidecar::Journal;

/// Undoes the tool's edits in place.
pub fn restore(
    map: &mut VmfFile,
    journal: &Journal,
    opts: &LedgerOptions,
) -> Result<(), LedgerError> {
    let mark = marker::read(&map.world.key_values, &opts.world_key)
        .ok_or_else(|| LedgerError::NotMarked(journal.tool.clone()))?;

    let in_journal = journal.fingerprint();
    if mark.fingerprint != in_journal {
        return Err(LedgerError::Mismatched {
            tool: journal.tool.clone(),
            in_map: mark.fingerprint,
            in_journal,
        });
    }

    let index = index_markers(map, &journal.marker_key)?;

    apply(map, journal, &index);
    strip_markers(map, &journal.marker_key);

    marker::write(&mut map.world.key_values, &opts.world_key, None);
    Ok(())
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

fn apply(map: &mut VmfFile, journal: &Journal, index: &Index) {
    // Keyvalue work first: it addresses entities by index, and adding or
    // removing entities invalidates every index after it.
    for op in &journal.ops {
        let Some(ent) = resolve(map, index, op.token()) else {
            continue;
        };
        match op {
            Op::Set { key, old, .. } | Op::Remove { key, old, .. } => {
                ent.key_values.insert(key.clone(), old.clone());
            }
            Op::Add { key, .. } => {
                ent.key_values.shift_remove(key);
            }
            Op::Connections { old, .. } => ent.connections = old.clone(),
            _ => {}
        }
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
