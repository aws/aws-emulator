// SPDX-License-Identifier: CC0-1.0

//! The overlay declaration the Storage Lens group's two filter CONDITIONS are
//! generated from, pinned against the published model snapshot.
//!
//! `MatchObjectSize` and `MatchObjectAge` are declared from
//! `smithy-model/local-additions.json` rather than from the snapshot, because a
//! bound neither condition was given must stay absent from what a read reports
//! and the snapshot's `@default` makes every bound a member the codec always
//! writes (see `smithy-model/LOCAL-ADDITIONS.md`). What the overlay restates of
//! the snapshot is therefore compared against it here: a refreshed snapshot that
//! renames a member, reorders one, retargets one, or stops defaulting them at all
//! fails here rather than silently changing what this service reports.
//!
//! The rule the declaration serves is pinned ON THE WIRE by the sibling suite
//! `storage_lens_group_absent_bounds`, which drives the documents a group can be
//! written with through create, update and read.

use serde_json::{Map, Value};

/// The two shapes the overlay declares, and the members each one carries — read
/// off the models below rather than copied here, so this suite states only WHICH
/// shapes are local.
const LOCAL_SHAPES: [&str; 2] = [
    "com.amazonaws.s3control#MatchObjectAge",
    "com.amazonaws.s3control#MatchObjectSize",
];

/// The trait the overlay exists to drop: a member carrying it is one the codec
/// always writes, which is what would materialise a bound nobody set.
const DEFAULT_TRAIT: &str = "smithy.api#default";

/// The `shapes` object of one model file of this crate, keyed by shape id.
fn shapes_of(path: &str) -> Map<String, Value> {
    let text = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|error| panic!("case {path}: the model file reads back: {error}"));
    let parsed: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {path}: the model file parses: {error}"));
    parsed["shapes"]
        .as_object()
        .unwrap_or_else(|| panic!("case {path}: a Smithy model declares shapes"))
        .clone()
}

/// The published snapshot this crate's wire shapes come from.
fn snapshot() -> Map<String, Value> {
    shapes_of("smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json")
}

/// The overlay model the two local declarations are generated from.
fn overlay() -> Map<String, Value> {
    shapes_of("smithy-model/local-additions.json")
}

/// The members one shape declares, in declaration order: `(name, target)` pairs.
/// The ORDER is part of the wire — it is the member index the codec reads — so it
/// is compared rather than a set.
fn members_of(case: &str, shapes: &Map<String, Value>, shape_id: &str) -> Vec<(String, String)> {
    shapes
        .get(shape_id)
        .and_then(|shape| shape.get("members"))
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: {shape_id} declares members"))
        .iter()
        .map(|(name, member)| {
            let target = member["target"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: {shape_id}${name} names a target"))
                .to_string();
            (name.clone(), target)
        })
        .collect()
}

/// Whether `declaration` carries the default trait, either on a member or on the
/// shape a member targets.
fn carries_default(declaration: &Value) -> bool {
    declaration
        .get("traits")
        .and_then(Value::as_object)
        .is_some_and(|traits| traits.contains_key(DEFAULT_TRAIT))
}

#[test]
fn the_overlay_restates_the_snapshots_own_member_lists() {
    let snapshot = snapshot();
    let overlay = overlay();

    for shape_id in LOCAL_SHAPES {
        let case = shape_id;
        let published = members_of(case, &snapshot, shape_id);
        let restated = members_of(case, &overlay, shape_id);
        assert_eq!(
            restated, published,
            "case {case:?}: expected the overlay to restate the snapshot's members, in order \
             and against the same targets, got {restated:?} against {published:?}"
        );

        for (name, target) in &restated {
            // The snapshot defaults these members; the overlay must not, or the
            // local declaration would admit no absent bound at all.
            let published_target = snapshot
                .get(target)
                .unwrap_or_else(|| panic!("case {case:?}: the snapshot declares {target}"));
            assert!(
                carries_default(published_target)
                    || carries_default(&snapshot[shape_id]["members"][name]),
                "case {case:?}: expected the snapshot to default {name}, which is why this \
                 overlay exists — if it no longer does, delete the overlay entry and let the \
                 generated shape declare it again"
            );
            let restated_target = overlay
                .get(target)
                .unwrap_or_else(|| panic!("case {case:?}: the overlay declares {target}"));
            assert!(
                !carries_default(restated_target)
                    && !carries_default(&overlay[shape_id]["members"][name]),
                "case {case:?}: expected the overlay to restate {name} with no default, got \
                 {restated_target:?}"
            );
            // The TYPE of the value a bound carries is the snapshot's, so the
            // overlay neither widens nor narrows it.
            assert_eq!(
                restated_target["type"], published_target["type"],
                "case {case:?}: expected {target} to keep the snapshot's type, got {} against {}",
                restated_target["type"], published_target["type"]
            );
        }
    }
}
