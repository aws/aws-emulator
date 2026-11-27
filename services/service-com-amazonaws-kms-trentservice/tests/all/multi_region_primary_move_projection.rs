// SPDX-License-Identifier: CC0-1.0

//! One instant's `KeyMetadata` projection of a multi-Region key set whose primary-key
//! designation is moving: the transient key state the move puts its two keys in and the
//! designation that move carries are HALVES OF ONE CHANGE, so no single projection ever
//! shows one without the other.
//!
//! The rule under test: "the old and new primary keys have a transient key state of
//! Updating. The original key state is restored when the update is complete"
//! (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`) — the restoration and the
//! arrival of the designation are the same instant, so a projection taken at ANY instant
//! either has the move still in flight (the designation where it was, both keys
//! `Updating`) or over (the designation arrived, neither key `Updating`), and never a
//! mixture of the two. Every transient state of a key is derived from the clock rather
//! than written when it falls due, so what makes this hold is that the whole document is
//! projected as of ONE instant
//! ([`service_com_amazonaws_kms_trentservice::state::Cmk::metadata_at`]) rather than
//! reading the clock once per member.
//!
//! The instants are SUPPLIED rather than waited for, so the suite asserts the rule on
//! both sides of the due instant — and on the due instant itself — without depending on
//! how fast the host runs. What a client polling the endpoints observes is asserted by
//! `tests/all/multi_region_primary_settlement.rs`.

use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_kms_trentservice::state::{service_state, PRIMARY_REGION_UPDATE_SPAN};

use super::harness::{create_key, ok_in, replicate_into, router_with_engine, text, REGION};

/// The Region the designation moves to: a replica key's own Region, since "this
/// operation changes the replica key in the specified Region to a primary key"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`).
const TARGET: &str = "eu-west-2";

/// How many instants the sweep takes across the window the move falls due inside.
///
/// A count, not a duration: nothing waits for these instants, each is handed to the
/// projection, so the sweep is as fast as the projection is.
const INSTANTS: u32 = 200;

/// One projected key, as the members this rule is about.
struct Projected {
    /// The Region whose endpoint serves this key.
    region: &'static str,
    /// Its `KeyState` at the instant.
    key_state: String,
    /// `MultiRegionKeyType` — which member of the set this key is at that instant.
    key_type: String,
    /// `PrimaryKey.Region` — where the set's designation is at that instant.
    designation: String,
}

/// No instant's projection splits the move of the designation from the transient key
/// state that carries it, whichever key of the set is projected.
#[tokio::test]
async fn one_instants_projection_never_splits_the_move_from_its_transient_state() {
    let case = format!("a move to {TARGET}");
    let (router, engine) = router_with_engine();
    let key_id = text(
        &case,
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    replicate_into(&router, &case, &key_id, &[TARGET]).await;

    // The move falls due one span after the call that started it, so the due instant
    // lies between these two readings: what is asserted below is that EVERY instant
    // either side of it, and the window it lies in, reports one consistent set.
    let started = SystemTime::now();
    ok_in(
        &router,
        REGION,
        "UpdatePrimaryRegion",
        &json!({"KeyId": key_id, "PrimaryRegion": TARGET}),
    )
    .await;
    let accepted = SystemTime::now();

    let state = service_state(&*engine).expect("the KMS state store is reachable");
    let held = state.lock();
    let keys = [
        (
            REGION,
            held.key_in(REGION, &key_id)
                .expect("the set's created key is held"),
        ),
        (
            TARGET,
            held.key_in(TARGET, &key_id)
                .expect("the set's replica key is held"),
        ),
    ];

    // From a second before the call to a second past the latest instant the move can
    // fall due, so the sweep covers the move in flight, the instant it ends, and the
    // set it leaves.
    let window = PRIMARY_REGION_UPDATE_SPAN + Duration::from_secs(2);
    let from = started - Duration::from_secs(1);
    for step in 0..=INSTANTS {
        let at = from + window * step / INSTANTS;
        let projected: Vec<Projected> = keys
            .iter()
            .map(|(region, key)| {
                let metadata = key.metadata_at(at);
                let configuration = metadata
                    .multi_region_configuration
                    .unwrap_or_else(|| panic!("case {case}: a key of a set reports its set"));
                Projected {
                    region,
                    key_state: metadata.key_state.unwrap_or_default(),
                    key_type: configuration.multi_region_key_type.unwrap_or_default(),
                    designation: configuration
                        .primary_key
                        .and_then(|primary| primary.region)
                        .unwrap_or_default(),
                }
            })
            .collect();

        // One set has one designation, whichever of its keys is asked.
        let designations: Vec<&str> = projected
            .iter()
            .map(|key| key.designation.as_str())
            .collect();
        assert!(
            designations.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case}: step {step} must report one designation for the set, got \
             {designations:?}"
        );
        let designation = designations.first().copied().unwrap_or_default();
        let moved = designation == TARGET;
        for key in &projected {
            let updating = key.key_state == "Updating";
            // A key still in the transient state is in a move that has NOT happened
            // yet, and a designation that has arrived leaves no key in it.
            assert!(
                !(updating && moved),
                "case {case}: step {step} has the {} key Updating while the designation \
                 already moved to {designation}, got state {:?} and type {:?}",
                key.region,
                key.key_state,
                key.key_type
            );
            // The key the designation rests with is the set's primary key, at the same
            // instant.
            let expected_type = if key.region == designation {
                "PRIMARY"
            } else {
                "REPLICA"
            };
            assert_eq!(
                key.key_type, expected_type,
                "case {case}: step {step} reports the {} key as {:?} while the \
                 designation is in {designation}",
                key.region, key.key_state
            );
        }
        // What the clock owes: before the call's own span has run the designation has
        // not moved, and past the latest instant it can be due it has, with no key left
        // in the transient state.
        if at < started + PRIMARY_REGION_UPDATE_SPAN {
            assert!(
                !moved,
                "case {case}: step {step} is inside the move's span, so the designation \
                 must still be in {REGION}, got {designation}"
            );
        }
        if at > accepted + PRIMARY_REGION_UPDATE_SPAN {
            assert!(
                moved,
                "case {case}: step {step} is past the move's span, so the designation \
                 must have arrived in {TARGET}, got {designation}"
            );
            assert!(
                projected.iter().all(|key| key.key_state != "Updating"),
                "case {case}: step {step} is past the move's span, so no key is still \
                 Updating, got {:?}",
                projected
                    .iter()
                    .map(|key| (key.region, key.key_state.clone()))
                    .collect::<Vec<_>>()
            );
        }
    }
}
