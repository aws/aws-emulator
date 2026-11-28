// SPDX-License-Identifier: CC0-1.0

//! The local declaration of `Workload` against the upstream model snapshot, and against
//! the wire.
//!
//! `Workload` is the one shape of this service whose Rust type is generated from the
//! overlay model `smithy-model/local-additions.json` rather than from the snapshot, because
//! AWS answers every workload read with a member the snapshot does not declare
//! (`FTRSupported` — see `smithy-model/LOCAL-ADDITIONS.md`). A local declaration can drift
//! from the snapshot in two directions, so both are pinned here:
//!
//! 1. The overlay declares EXACTLY the members the snapshot's `Workload` declares, each at
//!    the same target shape, plus the one the snapshot lacks. A member the snapshot adds,
//!    renames or re-targets in a later refresh fails this test rather than silently
//!    disappearing from the wire — and if the refresh brings `FTRSupported` itself, the
//!    local declaration should give way to the modelled shape.
//! 2. What the overlay declares is what goes ON THE WIRE: a workload read carries the
//!    members the request set, under the names the model spells them with, and carries no
//!    member neither model declares.
//!
//! Rule 1 reads both model files from disk — they are this crate's own data, consumed at
//! build time by `#[smithy_macros::smithy]` — so the comparison is against the model rather
//! than against a member list copied into a test. Rule 2 goes through the real
//! `MultiServiceRouter`, so routing, restJson1 serialization and the handler all run as
//! they do in production.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Client};

/// The shape both models declare, by its absolute id.
const WORKLOAD_SHAPE: &str = "com.amazonaws.wellarchitected#Workload";

/// The one member the overlay adds: the flag AWS answers and the snapshot does not
/// declare.
const LOCAL_MEMBER: &str = "FTRSupported";

/// `Workload`'s members in one model file, as `name -> target shape id`.
fn members(model: &str) -> BTreeMap<String, String> {
    let model: Value = serde_json::from_str(model).expect("a model file holds JSON");
    let shape = model
        .get("shapes")
        .and_then(|shapes| shapes.get(WORKLOAD_SHAPE))
        .unwrap_or_else(|| panic!("the model must declare {WORKLOAD_SHAPE}"));
    shape
        .get("members")
        .and_then(Value::as_object)
        .expect("a structure shape declares members")
        .iter()
        .map(|(name, member)| {
            (
                name.clone(),
                member
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The upstream snapshot, as the model update wrote it.
fn snapshot_members() -> BTreeMap<String, String> {
    members(include_str!(
        "../../smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json"
    ))
}

/// The overlay the local declaration is generated from.
fn overlay_members() -> BTreeMap<String, String> {
    members(include_str!("../../smithy-model/local-additions.json"))
}

/// Rule 1: the overlay covers the snapshot's member list in full, at the same targets,
/// and adds exactly one member of its own.
#[test]
fn the_local_declaration_restates_every_member_the_snapshot_declares() {
    let snapshot = snapshot_members();
    let overlay = overlay_members();

    assert!(
        !snapshot.is_empty(),
        "the snapshot must declare the members of {WORKLOAD_SHAPE}, got {snapshot:?}"
    );
    assert!(
        !snapshot.contains_key(LOCAL_MEMBER),
        "case \"snapshot-declares-the-local-member\": the snapshot now declares \
         {LOCAL_MEMBER}, so the local declaration should give way to the modelled shape, \
         got {snapshot:?}"
    );

    // Every snapshot member, at the target the snapshot gives it. The overlay restates the
    // target rather than widening it, so every length, pattern, range and enum constraint
    // the member carries is the snapshot's.
    for (name, target) in &snapshot {
        assert_eq!(
            overlay.get(name),
            Some(target),
            "case {name:?}: the overlay must restate the member at the target the \
             snapshot gives it, got {:?}",
            overlay.get(name)
        );
    }

    // And nothing else, apart from the one member AWS answers.
    let added: Vec<&String> = overlay
        .keys()
        .filter(|name| !snapshot.contains_key(*name))
        .collect();
    assert_eq!(
        added,
        vec![&LOCAL_MEMBER.to_string()],
        "case \"members-the-overlay-adds\": the overlay adds the one member AWS answers \
         and no other, got {added:?}"
    );
}

/// Rule 2: the members the local declaration spells are the members on the wire.
///
/// The rows differ in WHICH members the create sets — the minimum a create takes, a
/// workload carrying every optional member this read projects back, and one with the
/// collection members empty — because the risk a local declaration carries is a member
/// misspelled in the overlay, which shows up as a value the read drops.
#[tokio::test]
async fn a_workload_read_carries_the_members_the_declaration_spells() {
    let client = Client::new();
    let declared = overlay_members();

    let rows: [(&str, Value, Vec<&str>); 3] = [
        (
            "minimal",
            json!({}),
            vec!["WorkloadId", "WorkloadArn", "WorkloadName", "Description"],
        ),
        (
            "every-projected-member",
            json!({
                "AccountIds": ["111122223333"],
                "NonAwsRegions": ["on-premises-dc-1"],
                "ArchitecturalDesign": "https://example.com/design",
                "Industry": "Financial Services",
                "IndustryType": "Financial Services",
                "Notes": "notes the workload carries",
                "Tags": { "Env": "prod" },
                "DiscoveryConfig": { "TrustedAdvisorIntegrationStatus": "ENABLED" },
            }),
            vec![
                "AccountIds",
                "NonAwsRegions",
                "ArchitecturalDesign",
                "Industry",
                "IndustryType",
                "Notes",
                "Tags",
                "DiscoveryConfig",
                "PillarPriorities",
                "RiskCounts",
            ],
        ),
        (
            "empty-collections",
            json!({ "Lenses": [], "NonAwsRegions": [] }),
            vec!["Lenses", "RiskCounts"],
        ),
    ];

    for (index, (case, overrides, must_carry)) in rows.iter().enumerate() {
        let mut body = minimal_create(
            &format!("declared-workload-{index}"),
            &format!("tok-declared-{index}"),
        );
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            body[member.as_str()] = value.clone();
        }

        let created = client.create(body.clone()).await;
        let workload_id = created_id(&created, case);
        let fetched = client.get(&workload_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetWorkload 200, got {fetched:?}"
        );
        let projected = fetched
            .body
            .get("Workload")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: the read must project the workload, got {fetched:?}"));

        // Every key on the wire is a member the declaration spells: a member misspelled in
        // the overlay would travel under the wrong name, and a member of neither model
        // could not have come from the declaration at all.
        for member in projected.keys() {
            assert!(
                declared.contains_key(member),
                "case {case:?}: {member} is on the wire but the declaration spells no such \
                 member, got {projected:?}"
            );
        }
        // And the members this row populates come back.
        for member in must_carry {
            assert!(
                projected.contains_key(*member),
                "case {case:?}: the read must carry {member}, got {projected:?}"
            );
        }
        // Including the one the snapshot does not declare.
        assert_eq!(
            projected.get(LOCAL_MEMBER),
            Some(&Value::Bool(false)),
            "case {case:?}: the read must carry {LOCAL_MEMBER}, got {projected:?}"
        );
    }
}
