use pretty_assertions::assert_eq;
use source_vmf::prelude::*;
use vmf_ledger::sidecar::{Journal, Sidecar};
use vmf_ledger::{LedgerError, LedgerOptions, RestoreOptions, TrackedVmf, restore};

const TOOL: &str = "test-tool";
const VERSION: &str = "0.1.0";

/// Two entities and a brush, enough to tell tracked changes from untracked ones.
const MAP: &str = r#"
versioninfo
{
	"editorversion" "400"
	"mapversion" "1"
}
world
{
	"id" "1"
	"classname" "worldspawn"
	solid
	{
		"id" "2"
		side
		{
			"id" "3"
			"plane" "(0 0 0) (1 0 0) (0 1 0)"
			"material" "DEV/DEV_MEASUREGENERIC01B"
		}
	}
}
entity
{
	"id" "10"
	"classname" "pe_door"
	"targetname" "door_a"
	"open_time" "2.5"
	connections
	{
		"OnOpen" "relay_a\x1bTrigger\x1b\x1b0\x1b-1"
	}
	editor
	{
		"color" "220 30 220"
	}
}
entity
{
	"id" "11"
	"classname" "logic_relay"
	"targetname" "relay_a"
	editor
	{
		"color" "220 30 220"
	}
}
"#;

fn map() -> VmfFile {
    VmfFile::parse(&MAP.replace("\\x1b", "\x1b")).expect("fixture parses")
}

fn opts() -> LedgerOptions {
    LedgerOptions::new(TOOL, VERSION)
}

/// Runs a tool over the map and returns the compiled map plus its journal.
fn compile(edit: impl FnOnce(&mut TrackedVmf)) -> (VmfFile, Journal) {
    compile_from(map(), &opts(), edit)
}

/// The same from a given state and under a given name: several tools compile
/// the same map here, one after another.
fn compile_from(
    map: VmfFile,
    opts: &LedgerOptions,
    edit: impl FnOnce(&mut TrackedVmf),
) -> (VmfFile, Journal) {
    let mut tracked = TrackedVmf::new(map);
    edit(&mut tracked);
    tracked.finish(opts).expect("only tracked changes")
}

fn rollback(compiled: &mut VmfFile, journal: &Journal) -> Result<(), LedgerError> {
    restore(compiled, journal, &opts(), &RestoreOptions::default())
}

/// Compares what a mapper would see, not the in-memory structs.
fn text(file: &VmfFile) -> String {
    file.to_vmf_string()
}

// --- the round trip ------------------------------------------------------

#[test]
fn a_compiled_map_rolls_back_to_the_original() {
    let (mut compiled, journal) = compile(|m| {
        let ent = &mut m.entities.0[0];
        ent.set("classname".into(), "func_instance".into());
        ent.set("file".into(), "instances/door.vmf".into());
        ent.key_values.shift_remove("open_time");
    });

    assert_ne!(
        text(&compiled),
        text(&map()),
        "the tool did change something"
    );
    rollback(&mut compiled, &journal).unwrap();
    assert_eq!(text(&compiled), text(&map()));
}

#[test]
fn a_removed_key_comes_back_in_its_original_position() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0[0].key_values.shift_remove("targetname");
    });
    rollback(&mut compiled, &journal).unwrap();

    let keys: Vec<&str> = compiled.entities.0[0]
        .key_values
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "classname", "targetname", "open_time"]);
}

#[test]
fn connections_roll_back() {
    let (mut compiled, journal) = compile(|m| {
        let ent = &mut m.entities.0[0];
        ent.connections = None;
        ent.add_connection("OnClose", "relay_a", "Kill", "", 0.0, -1);
    });
    rollback(&mut compiled, &journal).unwrap();
    assert_eq!(text(&compiled), text(&map()));
}

#[test]
fn an_entity_the_tool_created_is_removed_again() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0.push(Entity::new("info_target", 99));
    });
    assert_eq!(compiled.entities.0.len(), 3);

    rollback(&mut compiled, &journal).unwrap();
    assert_eq!(text(&compiled), text(&map()));
}

#[test]
fn an_entity_the_tool_deleted_comes_back_in_place() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0.remove(0);
    });
    assert_eq!(compiled.entities.0.len(), 1);

    rollback(&mut compiled, &journal).unwrap();
    assert_eq!(text(&compiled), text(&map()));
}

#[test]
fn an_untouched_map_gets_no_journal_and_no_marker() {
    let (compiled, journal) = compile(|_| {});
    assert!(journal.is_empty());
    assert!(!vmf_ledger::is_marked(&compiled, &opts()));
    assert_eq!(text(&compiled), text(&map()));
}

// --- living with a mapper ------------------------------------------------

#[test]
fn edits_outside_the_tools_changes_survive_the_rollback() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });

    // The mapper reopens the compiled map and works on things the tool never
    // touched: a texture, an unrelated keyvalue, a brand new entity.
    compiled.world.solids[0].sides[0].material = "DEV/DEV_BLENDMEASURE".into();
    compiled.entities.0[1].set("spawnflags".into(), "1".into());
    compiled.entities.0.push(Entity::new("light", 42));

    rollback(&mut compiled, &journal).unwrap();

    assert_eq!(
        compiled.world.solids[0].sides[0].material, "DEV/DEV_BLENDMEASURE",
        "a retexture the tool never saw must not be undone"
    );
    assert_eq!(compiled.entities.0[1].get("spawnflags").unwrap(), "1");
    assert!(
        compiled
            .entities
            .0
            .iter()
            .any(|e| e.classname() == Some("light"))
    );
    // ...while the tool's own change is gone.
    assert_eq!(compiled.entities.0[0].classname(), Some("pe_door"));
}

#[test]
fn hammer_renumbering_ids_does_not_break_the_rollback() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });

    // Hammer rewrites ids on save. The marker is what carries identity.
    for (n, ent) in compiled.entities.0.iter_mut().enumerate() {
        ent.set("id".into(), (500 + n).to_string());
    }

    rollback(&mut compiled, &journal).unwrap();
    assert_eq!(compiled.entities.0[0].classname(), Some("pe_door"));
}

// --- refusing to guess ---------------------------------------------------

#[test]
fn an_edit_to_what_the_tool_wrote_stops_the_rollback() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });

    // The mapper edited the exact key the tool had written.
    compiled.entities.0[0].set("classname".into(), "func_movelinear".into());

    let err = rollback(&mut compiled, &journal).unwrap_err();
    let message = err.to_string();
    assert!(matches!(err, LedgerError::Collisions(_)), "{message}");
    assert!(message.contains("func_movelinear"), "{message}");
    assert!(message.contains("func_instance"), "{message}");
    assert_eq!(
        compiled.entities.0[0].classname(),
        Some("func_movelinear"),
        "a refused rollback must not have written anything"
    );
}

#[test]
fn force_rolls_back_over_the_conflicting_edit_only() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });
    compiled.entities.0[0].set("classname".into(), "func_movelinear".into());
    compiled.entities.0[0].set("rendercolor".into(), "255 0 0".into());

    let forced = RestoreOptions { force: true };
    restore(&mut compiled, &journal, &opts(), &forced).unwrap();

    assert_eq!(compiled.entities.0[0].classname(), Some("pe_door"));
    assert_eq!(
        compiled.entities.0[0].get("rendercolor").unwrap(),
        "255 0 0",
        "force discards the conflicting edit, not every edit"
    );
}

#[test]
fn a_duplicated_entity_is_refused_rather_than_guessed() {
    let (mut compiled, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });

    // Copy-paste in Hammer duplicates the marker along with everything else.
    let copy = compiled.entities.0[0].clone();
    compiled.entities.0.push(copy);

    assert!(matches!(
        rollback(&mut compiled, &journal).unwrap_err(),
        LedgerError::DuplicateMarker(_)
    ));
}

#[test]
fn a_journal_from_another_run_is_refused() {
    let (mut compiled, _) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });
    let (_, other) = compile(|m| {
        m.entities.0[1].set("targetname".into(), "relay_b".into());
    });

    assert!(matches!(
        rollback(&mut compiled, &other).unwrap_err(),
        LedgerError::Mismatched { .. }
    ));
}

#[test]
fn an_unmarked_map_has_nothing_to_roll_back() {
    let mut clean = map();
    let (_, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });

    assert!(matches!(
        rollback(&mut clean, &journal).unwrap_err(),
        LedgerError::NotMarked(_)
    ));
}

#[test]
fn changes_the_ledger_cannot_undo_block_the_export() {
    let mut tracked = TrackedVmf::new(map());
    tracked.world.solids[0].sides[0].material = "DEV/DEV_BLENDMEASURE".into();

    let err = tracked.finish(&opts()).unwrap_err();
    let message = err.to_string();
    assert!(matches!(err, LedgerError::Untracked(_)), "{message}");
    assert!(message.contains("world"), "{message}");
}

// --- the journal file ----------------------------------------------------

#[test]
fn the_journal_survives_a_trip_through_disk() {
    let (_, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
        m.entities.0[0].key_values.shift_remove("open_time");
    });

    let dir = std::env::temp_dir().join("vmf-ledger-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = Sidecar::path_for(dir.join("map.vmf"));
    assert_eq!(path.extension().unwrap(), "vdif");

    journal.write(&path).unwrap();
    assert_eq!(Journal::read(&path, TOOL).unwrap().unwrap(), journal);
    std::fs::remove_file(&path).ok();
}

#[test]
fn the_original_stays_available_while_the_map_is_edited() {
    let mut tracked = TrackedVmf::new(map());
    tracked.entities.0[0].set("classname".into(), "func_instance".into());

    assert_eq!(
        tracked.original().entities.0[0].classname(),
        Some("pe_door")
    );
    assert_eq!(tracked.entities.0[0].classname(), Some("func_instance"));
}

/// The markers are the whole scheme: Hammer keeps world and entity keyvalues,
/// so a rollback finds its way back through those and nothing else. If they did
/// not reach the file, every other test here would still pass in memory.
#[test]
fn markers_reach_the_saved_file() {
    let (compiled, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });
    let saved = text(&compiled);

    assert!(saved.contains("\"_vl_test-tool\" \"1\""), "{saved}");
    assert!(
        saved.contains(&format!(
            "\"vmf_ledger\" \"{TOOL} {VERSION} {}\"",
            journal.fingerprint()
        )),
        "{saved}"
    );

    // ...and a rollback works off the reparsed file, not the in-memory one.
    let mut reparsed = VmfFile::parse(&saved).unwrap();
    rollback(&mut reparsed, &journal).unwrap();
    assert_eq!(text(&reparsed), text(&map()));
}

// --- the whole cycle, on real files --------------------------------------

/// Fresh directory per test: these write files and the suite runs in parallel.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vmf-ledger-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The scenario the whole design exists for: a mapper compiles, keeps working
/// on the compiled map, and compiles again. The second run has to start from
/// the map as it was authored, not from the first run's output.
#[test]
fn compiling_an_already_compiled_map_starts_from_the_original() {
    let dir = scratch("recompile");
    let path = dir.join("map.vmf");
    map().save(&path).unwrap();
    let pristine = text(&map());

    // First compile.
    let mut first = TrackedVmf::new(VmfFile::open(&path).unwrap());
    first.entities.0[0].set("classname".into(), "func_instance".into());
    let (compiled, journal) = first.finish(&opts()).unwrap();
    compiled.save(&path).unwrap();
    journal.write(Sidecar::path_for(&path)).unwrap();

    // The mapper reopens it and changes something of their own.
    let mut edited = VmfFile::open(&path).unwrap();
    edited.entities.0[1].set("targetname".into(), "relay_renamed".into());
    edited.save(&path).unwrap();

    // Second compile: rewind first, then work from what comes back.
    let mut second = VmfFile::open(&path).unwrap();
    let rewound = vmf_ledger::rewind(&mut second, &path, &opts(), &RestoreOptions::default())
        .expect("rollback succeeds");
    assert!(rewound, "the map was marked, so it had to be rewound");

    assert_eq!(
        second.entities.0[0].classname(),
        Some("pe_door"),
        "the first run's own change is gone"
    );
    assert_eq!(
        second.entities.0[1].get("targetname").unwrap(),
        "relay_renamed",
        "the mapper's change is not"
    );
    assert!(!vmf_ledger::is_marked(&second, &opts()));

    // Undo the mapper's edit too and we are exactly back to the authored map.
    second.entities.0[1].set("targetname".into(), "relay_a".into());
    assert_eq!(text(&second), pristine);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_map_this_tool_never_touched_is_left_alone() {
    let dir = scratch("untouched");
    let path = dir.join("map.vmf");
    map().save(&path).unwrap();

    let mut file = VmfFile::open(&path).unwrap();
    let rewound =
        vmf_ledger::rewind(&mut file, &path, &opts(), &RestoreOptions::default()).unwrap();

    assert!(!rewound, "no marker, so nothing to do and no .vdif needed");
    assert_eq!(text(&file), text(&map()));

    std::fs::remove_dir_all(&dir).ok();
}

/// A KeyValues map holds one entry per key, so writing connections to a file
/// groups everything sharing an output name. Interleave two outputs and the
/// list comes back in a different order than the tool left it in.
#[test]
fn interleaved_outputs_survive_the_file_round_trip() {
    let (compiled, journal) = compile(|m| {
        let ent = &mut m.entities.0[1];
        ent.add_connection("OnTrigger", "a", "Open", "", 0.0, -1);
        ent.add_connection("OnSpawn", "b", "Kill", "", 0.0, -1);
        ent.add_connection("OnTrigger", "c", "Close", "", 0.0, -1);
    });

    let mut reopened = VmfFile::parse(&text(&compiled)).unwrap();
    rollback(&mut reopened, &journal).unwrap();
    assert_eq!(text(&reopened), text(&map()));
}

/// The order-sensitive case: several keys removed from different depths while
/// others are added. Each `Op::Remove` carries the index the key had in the
/// original, so replaying them in journal order - before the added keys are
/// gone - lands every one of them a few slots off.
#[test]
fn keys_come_back_in_order_when_the_tool_also_added_some() {
    let (compiled, journal) = compile(|m| {
        let ent = &mut m.entities.0[0];
        // Drop the second and fourth keys...
        ent.key_values.shift_remove("classname");
        ent.key_values.shift_remove("open_time");
        // ...and add more than were taken, at the front and at the back.
        ent.key_values
            .shift_insert(0, "file".into(), "a.vmf".into());
        ent.set("prev_classname".into(), "pe_door".into());
        ent.set("spawnflags".into(), "1".into());
    });

    let mut reopened = VmfFile::parse(&text(&compiled)).unwrap();
    rollback(&mut reopened, &journal).unwrap();

    let keys: Vec<&str> = reopened.entities.0[0]
        .key_values
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "classname", "targetname", "open_time"]);
    assert_eq!(text(&reopened), text(&map()));
}

/// The map says it was compiled, but the journal is not next to it - moved,
/// renamed, or never copied along. Compiling again would build on top of the
/// previous output, so this has to stop rather than quietly proceed.
#[test]
fn a_marked_map_without_its_journal_is_an_error() {
    let dir = scratch("no-journal");
    let path = dir.join("map.vmf");
    map().save(&path).unwrap();

    let mut first = TrackedVmf::new(VmfFile::open(&path).unwrap());
    first.entities.0[0].set("classname".into(), "func_instance".into());
    let (compiled, journal) = first.finish(&opts()).unwrap();
    compiled.save(&path).unwrap();
    journal.write(Sidecar::path_for(&path)).unwrap();

    std::fs::remove_file(Sidecar::path_for(&path)).unwrap();

    let mut reopened = VmfFile::open(&path).unwrap();
    let err =
        vmf_ledger::rewind(&mut reopened, &path, &opts(), &RestoreOptions::default()).unwrap_err();

    let message = err.to_string();
    assert!(
        matches!(err, LedgerError::JournalMissing { .. }),
        "{message}"
    );
    assert!(
        message.contains(TOOL),
        "tool name should be included in error: {message}"
    );
    assert!(message.contains(".vdif"), "{message}");

    std::fs::remove_dir_all(&dir).ok();
}

/// Real maps do carry duplicate entity ids - a copy-paste in Hammer keeps the
/// id - so matching cannot lean on `id` alone. Here both duplicates are edited,
/// which also breaks the signature stage: the classname it hashes is exactly
/// what the tool changed.
#[test]
fn entities_sharing_an_id_still_roll_back() {
    let mut original = map();
    let mut twin = original.entities.0[0].clone();
    twin.set("id".into(), "10".into()); // Duplicate id of first entity
    twin.set("targetname".into(), "door_b".into());
    twin.set("origin".into(), "512 0 0".into());
    original.entities.0.push(twin);
    let pristine = text(&original);

    let mut tracked = TrackedVmf::new(original);
    for ent in tracked.entities.0.iter_mut() {
        if ent.classname() == Some("pe_door") {
            ent.set("classname".into(), "func_instance".into());
            ent.set(
                "file".into(),
                format!("i/{}.vmf", ent.targetname().unwrap_or("x")),
            );
        }
    }

    let (compiled, journal) = tracked.finish(&opts()).unwrap();
    let mut reopened = VmfFile::parse(&text(&compiled)).unwrap();
    rollback(&mut reopened, &journal).unwrap();

    assert_eq!(text(&reopened), pristine);
}

// Matches below Signature confidence cannot be used for rollback and must block export.
#[test]
fn a_pair_matched_only_by_similarity_blocks_the_export() {
    // Omit id and signature keys so matching falls through to the similarity stage.
    let faceless = |mark: &str| {
        let mut ent = Entity::default();
        ent.set("shared_a".into(), "1".into());
        ent.set("shared_b".into(), "2".into());
        ent.set("shared_c".into(), "3".into());
        ent.set("shared_d".into(), "4".into());
        ent.set("own".into(), mark.into());
        ent
    };

    let mut original = VmfFile::default();
    original.entities.0.push(faceless("first"));

    let mut tracked = TrackedVmf::new(original);
    tracked.entities.0[0].set("shared_a".into(), "modified".into());

    let err = tracked.finish(&opts()).unwrap_err();
    let message = err.to_string();
    assert!(matches!(err, LedgerError::Untracked(_)), "{message}");
    assert!(message.contains("similarity"), "{message}");
}

// --- more than one tool on one map ---------------------------------------

fn beta() -> LedgerOptions {
    LedgerOptions::new("other-tool", "2.0")
}

/// Compiles the map with both tools in turn, the second one working off what
/// the first one wrote, and returns the map plus a journal each.
fn compile_with_both(dir: &std::path::Path) -> (std::path::PathBuf, Journal, Journal) {
    let path = dir.join("map.vmf");
    let vdif = Sidecar::path_for(&path);

    let (compiled, first) = compile_from(map(), &opts(), |m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });
    first.write(&vdif).unwrap();
    compiled.save(&path).unwrap();

    // The second tool knows nothing about the first and reads the map as it
    // finds it, marker and all.
    let (compiled, second) = compile_from(VmfFile::open(&path).unwrap(), &beta(), |m| {
        m.entities.0[1].set("targetname".into(), "relay_renamed".into());
    });
    second.write(&vdif).unwrap();
    compiled.save(&path).unwrap();

    (path, first, second)
}

#[test]
fn two_tools_share_one_journal_without_erasing_each_other() {
    let dir = scratch("two-tools");
    let (path, first, second) = compile_with_both(&dir);

    let file = Sidecar::read(Sidecar::path_for(&path)).unwrap();
    assert_eq!(file.tools.len(), 2, "both sections are in the one .vdif");
    assert_eq!(
        file.get(TOOL).unwrap().fingerprint(),
        first.fingerprint(),
        "the second tool writing itself in did not disturb the first section"
    );
    assert_eq!(
        file.get("other-tool").unwrap().fingerprint(),
        second.fingerprint()
    );

    let both = VmfFile::open(&path).unwrap();
    assert!(vmf_ledger::is_marked(&both, &opts()));
    assert!(vmf_ledger::is_marked(&both, &beta()));
    assert!(both.entities.0[0].key_values.contains_key("_vl_test-tool"));
    assert!(both.entities.0[1].key_values.contains_key("_vl_other-tool"));

    std::fs::remove_dir_all(&dir).ok();
}

/// The point of the whole arrangement: one tool runs over and over while
/// another ran once to set things up, and neither undoes the other.
#[test]
fn rolling_one_tool_back_leaves_the_other_compiled() {
    let dir = scratch("one-back");
    let (path, first, second) = compile_with_both(&dir);
    let mut both = VmfFile::open(&path).unwrap();

    restore(&mut both, &first, &opts(), &RestoreOptions::default()).unwrap();

    assert_eq!(
        both.entities.0[0].classname(),
        Some("pe_door"),
        "the first tool's change is gone"
    );
    assert_eq!(
        both.entities.0[1].get("targetname").unwrap(),
        "relay_renamed",
        "the second tool's is not"
    );
    assert!(!vmf_ledger::is_marked(&both, &opts()));
    assert!(vmf_ledger::is_marked(&both, &beta()));
    assert!(
        both.entities.0[1].key_values.contains_key("_vl_other-tool"),
        "and neither is its marker"
    );

    // ...and the one left behind still rolls back on its own afterwards.
    restore(&mut both, &second, &beta(), &RestoreOptions::default()).unwrap();
    assert_eq!(text(&both), text(&map()));

    std::fs::remove_dir_all(&dir).ok();
}

/// A `.vdif` that does not read back may hold another tool's only way home.
#[test]
fn a_journal_that_does_not_read_is_never_overwritten() {
    let dir = scratch("clobber");
    let vdif = Sidecar::path_for(dir.join("map.vmf"));
    std::fs::write(&vdif, "definitely not json").unwrap();

    let (_, journal) = compile(|m| {
        m.entities.0[0].set("classname".into(), "func_instance".into());
    });
    let err = journal.write(&vdif).unwrap_err();

    assert!(matches!(err, LedgerError::SidecarClobber { .. }), "{err}");
    assert_eq!(
        std::fs::read_to_string(&vdif).unwrap(),
        "definitely not json"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Maps compiled before the registry existed carry a single-tool journal, and
/// their marker holds a fingerprint taken over that exact file. Reproducing it
/// byte for byte is the only thing standing between those maps and a rollback
/// they can never run, so the v1 hash is spelled out here on purpose: if the
/// crate ever computes it differently, this test says so.
#[test]
fn a_journal_from_the_single_tool_format_still_rolls_back() {
    #[derive(serde::Serialize)]
    struct V1<'a> {
        version: u32,
        tool: &'a str,
        marker_key: &'a str,
        ops: &'a [vmf_ledger::ops::Op],
    }

    fn fnv1a(data: &str) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in data.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    let dir = scratch("v1");
    let path = dir.join("map.vmf");
    let vdif = Sidecar::path_for(&path);

    // v1 marked entities with one shared key, whoever the tool was.
    let legacy = opts().with_marker_key("_vlid");
    let (compiled, journal) = compile_from(map(), &legacy, |m| {
        let ent = &mut m.entities.0[0];
        ent.set("classname".into(), "func_instance".into());
        ent.key_values.shift_remove("open_time");
    });

    let text_v1 = serde_json::to_string_pretty(&V1 {
        version: 1,
        tool: "test-tool 0.1.0",
        marker_key: "_vlid",
        ops: &journal.ops,
    })
    .unwrap();
    std::fs::write(&vdif, &text_v1).unwrap();

    // The map as v1 left it: one record, tool and version glued together.
    let mut old_map = compiled;
    old_map.world.key_values.insert(
        "vmf_ledger".to_string(),
        format!("test-tool 0.1.0 {:016x}", fnv1a(&text_v1)),
    );
    old_map.save(&path).unwrap();

    let lifted = Journal::read(&vdif, TOOL).unwrap().expect("section found");
    assert_eq!(
        lifted.marker_key, "_vlid",
        "the old key comes from the file"
    );
    assert_eq!(lifted.version, "0.1.0");

    let mut reopened = VmfFile::open(&path).unwrap();
    restore(&mut reopened, &lifted, &opts(), &RestoreOptions::default())
        .expect("an old journal still undoes an old compile");
    assert_eq!(text(&reopened), text(&map()));

    std::fs::remove_dir_all(&dir).ok();
}

// --- visgroups -----------------------------------------------------------

/// One switch in Hammer for everything a tool generated.
#[test]
fn entities_the_tool_creates_land_in_its_visgroup() {
    let opts = opts().with_visgroup("generated");
    let (compiled, journal) = compile_from(map(), &opts, |m| {
        m.entities.0.push(Entity::new("info_target", 99));
    });

    let group = compiled
        .visgroups
        .find_by_name("generated")
        .expect("the group was made");
    let members: Vec<&Entity> = compiled
        .get_entities_in_visgroup(group.id, false)
        .unwrap()
        .collect();

    assert_eq!(members.len(), 1, "only what the tool made");
    assert_eq!(members[0].classname(), Some("info_target"));
    assert!(!journal.visgroups.is_empty(), "and it is undoable");
}

#[test]
fn the_visgroup_goes_away_with_the_entities_it_held() {
    let opts = opts().with_visgroup("generated");
    let (mut compiled, journal) = compile_from(map(), &opts, |m| {
        m.entities.0.push(Entity::new("info_target", 99));
    });

    restore(&mut compiled, &journal, &opts, &RestoreOptions::default()).unwrap();

    assert!(compiled.visgroups.find_by_name("generated").is_none());
    assert_eq!(text(&compiled), text(&map()));
}

/// A group is deleted only while it is ours alone. Hammer sweeps anything
/// pointing at a group that is gone into `_orphaned hidden`, and doing that to
/// a mapper's own work would be a poor trade for tidiness.
#[test]
fn a_visgroup_the_mapper_moved_into_outlives_the_rollback() {
    let opts = opts().with_visgroup("generated");
    let (mut compiled, journal) = compile_from(map(), &opts, |m| {
        m.entities.0.push(Entity::new("info_target", 99));
    });
    let id = compiled.visgroups.find_by_name("generated").unwrap().id;
    compiled.entities.0[1].editor.join_visgroup(id);

    restore(&mut compiled, &journal, &opts, &RestoreOptions::default()).unwrap();

    assert!(
        compiled.visgroups.find_by_name("generated").is_some(),
        "somebody else is still filed under it"
    );
    assert!(compiled.entities.0[1].editor.visgroup_ids.contains(&id));
}

/// `apply` sorts the re-insertions by index rather than trusting the order they
/// appear in. Our own `diff` happens to emit them ascending already, so the sort
/// looks redundant - until a journal comes from a different build, or from a
/// hand edit. Feeding the ops back in reverse is the cheapest way to hold that
/// contract.
#[test]
fn insertions_do_not_depend_on_the_order_ops_appear_in() {
    let (compiled, journal) = compile(|m| {
        let ent = &mut m.entities.0[0];
        ent.key_values.shift_remove("classname");
        ent.key_values.shift_remove("targetname");
        ent.key_values.shift_remove("open_time");
        ent.set("file".into(), "a.vmf".into());
    });

    let shuffled = Journal::new(
        TOOL,
        VERSION,
        journal.marker_key.clone(),
        journal.ops.iter().rev().cloned().collect(),
        Vec::new(),
    );
    // The fingerprint is taken over the content and the order of the ops is
    // part of that content, so the map's record has to be moved to the new one.
    let mut reopened = VmfFile::parse(&text(&compiled)).unwrap();
    reopened.world.key_values.insert(
        "vmf_ledger".to_string(),
        format!("{TOOL} {VERSION} {}", shuffled.fingerprint()),
    );

    rollback(&mut reopened, &shuffled).unwrap();

    let keys: Vec<&str> = reopened.entities.0[0]
        .key_values
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "classname", "targetname", "open_time"]);
}
