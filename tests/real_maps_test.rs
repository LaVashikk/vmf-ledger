//! The rollback property, checked against real maps instead of a fixture.
//!
//! `real_vmfs/` is gitignored, so this skips itself when the maps are not
//! there. Anything it catches that `ledger_test.rs` does not comes from what
//! hand-written fixtures never contain: thousands of entities, duplicate ids,
//! instances, connections written by half a dozen different tools.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use vmf_forge::prelude::*;
use vmf_ledger::{LedgerOptions, RestoreOptions, TrackedVmf, restore};

fn maps() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../real_vmfs");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "vmf"))
        .collect();
    found.sort();
    found
}

/// Roughly what a pseudo-entity pass does: rewrite some classnames, add the
/// keys an instance needs, drop the ones the tool consumes, wire up an output.
fn compile_like_a_pass(map: &mut TrackedVmf) -> usize {
    let mut touched = 0;
    for (n, ent) in map.entities.0.iter_mut().enumerate() {
        if !n.is_multiple_of(7) {
            continue;
        }
        touched += 1;
        let was = ent.classname().unwrap_or_default().to_string();
        ent.set("classname".into(), "func_instance".into());
        ent.set("prev_classname".into(), was);
        ent.set("file".into(), "instances/generated.vmf".into());
        ent.add_connection("OnUser1", "@generated", "Trigger", "", 0.0, -1);

        // Swap a keyvalue for an instance parameter in place, the way the
        // `instance_settings` pass does. Removing from the middle while adding
        // elsewhere is what makes the order of a rollback's inserts matter;
        // edits that only append never notice.
        for key in ["model", "targetname", "angles"] {
            let Some(at) = ent.key_values.get_index_of(key) else {
                continue;
            };
            let value = ent.key_values.shift_remove(key).unwrap_or_default();
            ent.key_values.shift_insert(
                at,
                format!("replace0{}", at % 9),
                format!("${key} {value}"),
            );
        }
    }
    touched
}

#[test]
fn the_rollback_holds_on_real_maps() {
    let maps = maps();
    if maps.is_empty() {
        eprintln!("skipped: real_vmfs/ has no maps");
        return;
    }

    let opts = LedgerOptions::new("real-map-test", "0.1.0");
    for path in maps {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(original) = VmfFile::open(&path) else {
            eprintln!("{name}: does not parse, skipping");
            continue;
        };
        let pristine = original.to_vmf_string();

        let mut tracked = TrackedVmf::new(original);
        let touched = compile_like_a_pass(&mut tracked);
        if touched == 0 {
            eprintln!("{name}: no entities, skipping");
            continue;
        }

        let (compiled, journal) = tracked
            .finish(&opts)
            .unwrap_or_else(|e| panic!("{name}: export refused: {e}"));
        assert!(!journal.ops.is_empty(), "{name}: nothing recorded");

        // Through text and back, the way it would reach a mapper.
        let mut reopened = VmfFile::parse(&compiled.to_vmf_string())
            .unwrap_or_else(|e| panic!("{name}: compiled map does not re-parse: {e}"));
        assert!(vmf_ledger::is_marked(&reopened, &opts), "{name}: no marker");

        restore(&mut reopened, &journal, &opts, &RestoreOptions::default())
            .unwrap_or_else(|e| panic!("{name}: rollback failed: {e}"));

        // Line by line: these files run to millions of characters, and a whole
        // dump of two of them says nothing about what went wrong.
        if let Some(report) = first_differences(&pristine, &reopened.to_vmf_string()) {
            panic!("{name}: rollback did not land on the original\n{report}");
        }
        eprintln!("{name}: {touched} entities, {} ops, ok", journal.ops.len());
    }
}

/// A second tool over the same map: it generates entities of its own and edits
/// the ones it can still identify.
///
/// Deliberately not the ones the first tool rewrote. That pass moves
/// `targetname` into an instance parameter, and an entity that has lost its
/// name on a map that also carries duplicate ids can only be paired by
/// similarity - which the ledger refuses to build a rollback on, and says so
/// rather than guessing.
fn init_like_a_pass(map: &mut TrackedVmf) -> usize {
    let mut names: HashMap<&str, usize> = HashMap::new();
    for ent in map.entities.0.iter() {
        if let Some(name) = ent.targetname().filter(|n| !n.is_empty()) {
            *names.entry(name).or_default() += 1;
        }
    }
    let identifiable: HashSet<String> = names
        .into_iter()
        .filter(|&(_, count)| count == 1)
        .map(|(name, _)| name.to_string())
        .collect();

    let mut touched = 0;
    for ent in map.entities.0.iter_mut() {
        if !ent.targetname().is_some_and(|n| identifiable.contains(n)) {
            continue;
        }
        touched += 1;
        ent.set("spawnflags".into(), "1".into());
    }

    for n in 0..3 {
        map.entities.0.push(Entity::new("info_target", 900_000 + n));
    }
    touched + 3
}

/// Two tools over one real map. The fixture tests prove the bookkeeping; this
/// proves it survives a matcher with thousands of entities to place, on maps
/// where both tools have marked some of the same ones.
///
/// Rolled back last-in-first-out, which is not a rule the ledger enforces but
/// the only order that means anything: the second tool recorded the first
/// tool's output as the values it found, so undoing the first one underneath it
/// would be undoing something the second one is still standing on. Out of
/// order, compare-and-swap says so rather than corrupting anything.
#[test]
fn two_tools_roll_back_independently_on_real_maps() {
    let maps = maps();
    if maps.is_empty() {
        eprintln!("skipped: real_vmfs/ has no maps");
        return;
    }

    let first_opts = LedgerOptions::new("first-tool", "1.0");
    let second_opts = LedgerOptions::new("second-tool", "2.0").with_visgroup("generated");
    let plain = RestoreOptions::default();

    for path in maps {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(original) = VmfFile::open(&path) else {
            continue;
        };
        let pristine = original.to_vmf_string();

        let mut tracked = TrackedVmf::new(original);
        if compile_like_a_pass(&mut tracked) == 0 {
            continue;
        }
        let (compiled, first) = tracked
            .finish(&first_opts)
            .unwrap_or_else(|e| panic!("{name}: first export refused: {e}"));

        // The second tool opens the map as it finds it, marker and all.
        let mut tracked = TrackedVmf::new(reparse(&name, &compiled));
        init_like_a_pass(&mut tracked);
        let (compiled, second) = tracked
            .finish(&second_opts)
            .unwrap_or_else(|e| panic!("{name}: second export refused: {e}"));

        let mut map = reparse(&name, &compiled);
        assert!(vmf_ledger::is_marked(&map, &first_opts), "{name}: no first");
        assert!(
            vmf_ledger::is_marked(&map, &second_opts),
            "{name}: the second tool erased the first one's mark"
        );
        let group = map
            .visgroups
            .find_by_name("generated")
            .unwrap_or_else(|| panic!("{name}: the generated visgroup is missing"))
            .id;
        assert_eq!(
            map.get_entities_in_visgroup(group, false).unwrap().count(),
            3,
            "{name}: only what the second tool made belongs in its visgroup"
        );

        restore(&mut map, &second, &second_opts, &plain)
            .unwrap_or_else(|e| panic!("{name}: second rollback failed: {e}"));
        assert!(
            vmf_ledger::is_marked(&map, &first_opts),
            "{name}: rolling the second tool back took the first one with it"
        );
        assert!(
            map.visgroups.find_by_name("generated").is_none(),
            "{name}: the visgroup outlived the entities it held"
        );

        restore(&mut map, &first, &first_opts, &plain)
            .unwrap_or_else(|e| panic!("{name}: first rollback failed: {e}"));

        if let Some(report) = first_differences(&pristine, &map.to_vmf_string()) {
            panic!("{name}: two rollbacks did not land on the original\n{report}");
        }
        eprintln!(
            "{name}: {} + {} ops from two tools, both undone, ok",
            first.ops.len(),
            second.ops.len()
        );
    }
}

/// Through text and back, the way the map would reach the next tool.
fn reparse(name: &str, map: &VmfFile) -> VmfFile {
    VmfFile::parse(&map.to_vmf_string())
        .unwrap_or_else(|e| panic!("{name}: compiled map does not re-parse: {e}"))
}

/// First few differing lines, with a little context on either side.
fn first_differences(want: &str, got: &str) -> Option<String> {
    const SHOW: usize = 4;

    let (want_lines, got_lines): (Vec<&str>, Vec<&str>) =
        (want.lines().collect(), got.lines().collect());
    let mut report = String::new();
    let mut shown = 0;

    for (n, (w, g)) in want_lines.iter().zip(&got_lines).enumerate() {
        if w == g {
            continue;
        }
        shown += 1;
        if shown > SHOW {
            report.push_str("  ...\n");
            break;
        }
        report.push_str(&format!("  line {}:\n", n + 1));
        for line in &want_lines[n.saturating_sub(2)..n] {
            report.push_str(&format!("      context {line:?}\n"));
        }
        report.push_str(&format!("    original {w:?}\n    restored {g:?}\n"));
    }

    if shown == 0 && want_lines.len() != got_lines.len() {
        report.push_str(&format!(
            "  same prefix, different length: {} lines vs {}\n",
            want_lines.len(),
            got_lines.len()
        ));
        shown = 1;
    }
    (shown > 0).then_some(report)
}
