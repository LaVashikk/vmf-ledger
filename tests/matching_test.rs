use pretty_assertions::assert_eq;
use source_vmf::VmfBlock;
use vmf_ledger::matching::*;

fn block(name: &str, kvs: &[(&str, &str)]) -> VmfBlock {
    VmfBlock {
        name: name.to_string(),
        key_values: kvs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        blocks: Vec::new(),
    }
}

fn pair_of(m: &Matching, old: usize) -> Option<Pair> {
    m.pairs.iter().copied().find(|p| p.old == old)
}

#[test]
fn marker_survives_every_other_key_changing() {
    let old = [block(
        "entity",
        &[("_vlid", "7"), ("classname", "pe_door"), ("id", "1")],
    )];
    let new = [block(
        "entity",
        &[
            ("_vlid", "7"),
            ("classname", "func_instance"),
            ("id", "990"),
        ],
    )];
    let opts = MatchOptions {
        marker_key: Some("_vlid"),
        ..Default::default()
    };

    let m = match_blocks(&old, &new, &opts);
    let p = pair_of(&m, 0).unwrap();
    assert_eq!(p.new, 0);
    assert_eq!(p.confidence, Confidence::Marker);
}

#[test]
fn id_pairs_when_no_marker_is_present() {
    let old = [block(
        "entity",
        &[("id", "4"), ("classname", "logic_relay")],
    )];
    let new = [block("entity", &[("id", "4"), ("classname", "logic_auto")])];

    let m = match_blocks(&old, &new, &MatchOptions::default());
    assert_eq!(pair_of(&m, 0).unwrap().confidence, Confidence::Id);
}

#[test]
fn signature_pairs_after_hammer_renumbers_ids() {
    let old = [block(
        "entity",
        &[
            ("id", "4"),
            ("classname", "prop_static"),
            ("origin", "0 0 8"),
        ],
    )];
    let new = [block(
        "entity",
        &[
            ("id", "81"),
            ("classname", "prop_static"),
            ("origin", "0 0 8"),
        ],
    )];

    let m = match_blocks(&old, &new, &MatchOptions::default());
    assert_eq!(pair_of(&m, 0).unwrap().confidence, Confidence::Signature);
}

#[test]
fn ambiguous_token_is_deferred_instead_of_guessed() {
    // Two blocks share id 4 after a copy-paste. Picking the first hit here is
    // how a rollback writes into the wrong entity.
    let old = [
        block("entity", &[("id", "4"), ("origin", "0 0 0")]),
        block("entity", &[("id", "4"), ("origin", "64 0 0")]),
    ];
    let new = [
        block("entity", &[("id", "4"), ("origin", "64 0 0")]),
        block("entity", &[("id", "4"), ("origin", "0 0 0")]),
    ];

    let m = match_blocks(&old, &new, &MatchOptions::default());
    // Nothing was paired on id; similarity sorted it out by origin instead.
    assert!(m.pairs.iter().all(|p| p.confidence != Confidence::Id));
    assert_eq!(pair_of(&m, 0).unwrap().new, 1);
    assert_eq!(pair_of(&m, 1).unwrap().new, 0);
}

#[test]
fn similarity_pairs_renumbered_blocks_without_signature_keys() {
    // Sides carry no classname/targetname/origin, so only stage 4 can place them.
    let mk = |id: &str, plane: &str, material: &str| {
        block(
            "side",
            &[
                ("id", id),
                ("plane", plane),
                ("material", material),
                ("uaxis", "[1 0 0 0] 0.25"),
                ("vaxis", "[0 -1 0 0] 0.25"),
                ("rotation", "0"),
                ("lightmapscale", "16"),
                ("smoothing_groups", "0"),
            ],
        )
    };
    let old = [
        mk("1", "(0 0 0) (1 0 0) (0 1 0)", "DEV/DEV_A"),
        mk("2", "(0 0 8) (1 0 8) (0 1 8)", "DEV/DEV_A"),
        mk("3", "(0 0 16) (1 0 16) (0 1 16)", "DEV/DEV_A"),
    ];
    let new = [
        mk("13", "(0 0 16) (1 0 16) (0 1 16)", "DEV/DEV_A"),
        mk("11", "(0 0 0) (1 0 0) (0 1 0)", "DEV/DEV_B"), // retextured
        mk("12", "(0 0 8) (1 0 8) (0 1 8)", "DEV/DEV_A"),
    ];

    let m = match_blocks(&old, &new, &MatchOptions::default());
    assert_eq!(m.pairs.len(), 3);
    assert!(m.only_old.is_empty() && m.only_new.is_empty());
    // Paired by the rare `plane`, not by the material every side shares.
    assert_eq!(pair_of(&m, 0).unwrap().new, 1);
    assert_eq!(pair_of(&m, 1).unwrap().new, 2);
    assert_eq!(pair_of(&m, 2).unwrap().new, 0);
}

#[test]
fn blocks_of_different_kinds_never_pair() {
    let old = [block("entity", &[("id", "1")])];
    let new = [block("solid", &[("id", "1")])];

    let m = match_blocks(&old, &new, &MatchOptions::default());
    assert!(m.pairs.is_empty());
    assert_eq!(m.only_old, [0]);
    assert_eq!(m.only_new, [0]);
}

#[test]
fn additions_and_removals_are_reported() {
    let old = [
        block(
            "entity",
            &[("id", "1"), ("classname", "a"), ("origin", "0 0 0")],
        ),
        block(
            "entity",
            &[("id", "2"), ("classname", "gone"), ("origin", "9 9 9")],
        ),
    ];
    let new = [
        block(
            "entity",
            &[("id", "1"), ("classname", "a"), ("origin", "0 0 0")],
        ),
        block(
            "entity",
            &[("id", "3"), ("classname", "fresh"), ("origin", "5 5 5")],
        ),
    ];

    let m = match_blocks(&old, &new, &MatchOptions::default());
    assert_eq!(m.pairs.len(), 1);
    assert_eq!(m.only_old, [1]);
    assert_eq!(m.only_new, [1]);
}

#[test]
fn min_score_keeps_unrelated_blocks_apart() {
    let old = [block("entity", &[("id", "1"), ("a", "1"), ("b", "2")])];
    let new = [block("entity", &[("id", "9"), ("x", "8"), ("y", "7")])];

    let m = match_blocks(&old, &new, &MatchOptions::default());
    assert!(m.pairs.is_empty());
}
