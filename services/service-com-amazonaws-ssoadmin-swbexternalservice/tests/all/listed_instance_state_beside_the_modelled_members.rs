// SPDX-License-Identifier: CC0-1.0

//! The MEMBER SET an instance view carries on the `awsJson1_1` wire: the members the model
//! declares for `InstanceMetadata`, plus the one unmodelled `State` structure the service
//! serves beside them.
//!
//! THE RULE: an instance view is one projection of one stored instance record, so every
//! entry `ListInstances` serves — and the `Instance` envelope `DescribeInstance` repeats —
//! carries a `State` structure whose single `Name` is that record's own `Status`, and
//! carries no member name beyond the ones the model declares for the shape plus that one.
//!
//! The sibling suites pin the VALUE of the view: `instance_state_view.rs` that `State` is
//! served and mirrors the status through the lifecycle, `listed_instance_state_source.rs`
//! the record it is read from, `listed_instance_state_generated_accounts.rs` the same rule
//! over drawn populations, and `listed_instance_projection_matches_describe.rs` the two
//! reads' projections against each other. The axis this suite adds is the member set
//! against the MODEL: `State` is not a member the upstream snapshot declares, so the shape
//! that carries it is generated from this crate's own overlay model
//! (`smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`), and a
//! member list stated outside the snapshot is the one place a modelled member can be
//! dropped or a wire name misspelled without any single value looking wrong. So each row
//! checks both directions at once — the unmodelled member is there, and every other name a
//! served view carries is a member the model declares.
//!
//! Two of the tests here read the MODELS rather than the wire: one checks that the overlay
//! restates the snapshot exactly but for the two members it adds, and the other that every
//! member name a served view carries is one the overlay declares for the view — so the
//! served member set is pinned against the model it is generated from rather than against a
//! second copy of it.
//!
//! The member names are the model's own, not this crate's shapes:
//! `smithy_query(query="shape",shape="com.amazonaws.ssoadmin#InstanceMetadata",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`
//! declares `InstanceArn`, `IdentityStoreId`, `IdentityStoreArn`, `OwnerAccountId`, `Name`,
//! `CreatedDate`, `Status`, `StatusReason`, `PrimaryRegion` and `Regions`. The lifecycle
//! values are the model-generated `ops::types::INSTANCE_STATUS_VALUES`
//! (`InstanceStatus`: `CREATE_IN_PROGRESS | CREATE_FAILED | DELETE_IN_PROGRESS | ACTIVE`),
//! so no row's status is written out by hand, and the rows span the instance KIND, a view
//! whose optional `Name` is absent, a `Name` spanning `NameType`'s `^[\w+=,.@-]+$`, how
//! many instances the account holds and which page the entry arrives on.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, the enumeration and the `State` assertion the ones
//! `instance_state_view.rs` owns, and the empty account and the held activation instant the
//! ones `instance_create.rs` owns. Where a phase lives on the far side of a due instant the
//! test moves the STORED instant rather than sleeping — the transition is still applied by
//! the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::hold_activation;
use crate::instance_envelope_view::SHARED_MEMBERS;
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{call_in_region, empty_account, router_over, with_store};

/// The one member `com.amazonaws.ssoadmin#InstanceMetadata` declares that
/// `instance_envelope_view.rs`'s list of them leaves out, so the two together are the
/// model's whole member set: `StatusReason`, which that suite omits because a served view
/// never carries it. The source is the `smithy_query` call in this suite's header.
const STATUS_REASON: &str = "StatusReason";

/// The member the model does NOT declare and the service serves anyway: the `State`
/// structure whose `Name` repeats the instance's lifecycle.
const UNMODELLED_MEMBER: &str = "State";

/// Whether `name` is a member the model declares for the instance-metadata shape.
///
/// Read off the list `instance_envelope_view.rs` owns rather than a second copy of it, so the
/// model's member set is maintained in one place in this crate's tests.
fn is_modelled(name: &str) -> bool {
    SHARED_MEMBERS.contains(&name) || name == STATUS_REASON
}

/// The members an instance view always carries, whatever lifecycle the record is in: the
/// instance's identity and ownership, the Regions it is enabled in, its lifecycle under
/// both views.
///
/// `Name` is not among them — the member is optional and no part of an instance's identity,
/// so a view of a record that holds none serves none — and neither is `StatusReason`, which
/// the model documents as additional context the service has none of to give.
const ALWAYS_SERVED: [&str; 9] = [
    "InstanceArn",
    "IdentityStoreId",
    "IdentityStoreArn",
    "OwnerAccountId",
    "CreatedDate",
    "Status",
    "PrimaryRegion",
    "Regions",
    "State",
];

/// The Region the seeded instances are enabled in and their calls are signed for: a
/// `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites drives.
const SEEDED_REGION: &str = "ca-central-1";

/// The Region the wire row's instance is enabled in — the three-prefix spelling of the same
/// pattern.
const WIRE_REGION: &str = "us-gov-west-1";

/// The page sizes the rows ask for: the published minimum of the operation's `MaxResults`
/// (`range 1..100`), the size that holds the whole seeded account, the published maximum,
/// and the request that names none.
const PAGE_SIZES: [Option<i64>; 4] = [Some(1), Some(3), Some(100), None];

/// THE RULE on one served instance view: it carries the unmodelled `State` structure
/// mirroring its own `Status`, every member an instance view always carries, and no member
/// name the model does not declare for the shape.
fn assert_instance_view_members(case: &str, view: &Value) {
    let members = view
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: an instance view is a structure, got {view}"));

    for name in members.keys() {
        assert!(
            is_modelled(name) || name == UNMODELLED_MEMBER,
            "case {case:?}: served member {name:?} is neither a member the model declares \
             for InstanceMetadata ({SHARED_MEMBERS:?} and {STATUS_REASON:?}) nor the \
             unmodelled {UNMODELLED_MEMBER:?} view, got {view}"
        );
    }

    for name in ALWAYS_SERVED {
        assert!(
            members.get(name).is_some_and(|value| !value.is_null()),
            "case {case:?}: expected the view to carry {name:?}, got {view}"
        );
    }

    assert_state_mirrors_status(case, view);
}

/// Three instances an account can hold at once, each in `status` with nothing the clock
/// owes, spanning the instance KIND and the `Name` value space: a named organization
/// instance, an account instance with no `Name` at all, and one whose `Name` spans
/// `NameType`'s `^[\w+=,.@-]+$`.
///
/// An account enables ONE instance through this API, so a multi-instance account is seeded
/// through the service's own `state::Instance` constructor; everything asserted then goes
/// over the wire.
fn seeded_account(status: &str) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let created_date = DateTime::from(SystemTime::now());
    let records = [
        (
            "ssoins-4c1d9e7f0a2b3c4d",
            "d-4c1d9e7f0a",
            state::InstanceType::Organization,
            Some("member-set-idc-9f3a1c20".to_string()),
        ),
        (
            "ssoins-5d2e0f8a1b3c4d5e",
            "d-5d2e0f8a1b",
            state::InstanceType::Account,
            None,
        ),
        (
            "ssoins-6e3f1a9b2c4d5e6f",
            "d-6e3f1a9b2c",
            state::InstanceType::Account,
            Some("A_b+c=d,e.f@g-1".to_string()),
        ),
    ];
    with_store(&engine, |store| {
        store.instances.clear();
        for (instance_id, identity_store_id, instance_type, name) in records {
            let mut instance = state::Instance::active(
                instance_id,
                identity_store_id,
                instance_type,
                name,
                created_date,
                SEEDED_REGION,
            );
            // The lifecycle this row reads the view in. Nothing is owed: the status seeded
            // here IS the instance's status, so no transition the clock owes can move it
            // out from under the read.
            instance.status = status.to_string();
            instance.active_due = None;
            instance.deleted_due = None;
            store.instances.insert(instance.instance_arn.clone(), instance);
        }
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// THE RULE over every lifecycle value the model declares, on both instance read paths and
/// on whichever page an entry arrives on.
#[tokio::test]
async fn an_instance_view_carries_the_modelled_members_and_the_state_view() {
    for status in INSTANCE_STATUS_VALUES {
        let (router, _engine) = seeded_account(status);

        // The enumeration the single-instance reads below are addressed from, read once:
        // which instances the account holds does not depend on the page size the
        // enumeration asked for, and that axis is driven on the listing itself.
        let enumerated = listed(
            &format!("status {status} / the account's instances"),
            &router,
            SEEDED_REGION,
            None,
        )
        .await;

        for max_results in PAGE_SIZES {
            let case = format!("status {status} / MaxResults {max_results:?} / ListInstances");
            let entries = listed(&case, &router, SEEDED_REGION, max_results).await;
            assert_eq!(
                entries.len(),
                3,
                "case {case:?}: every instance the account holds is enumerated, got {entries:?}"
            );
            for entry in &entries {
                assert_instance_view_members(&case, entry);
                assert_eq!(
                    entry["Status"].as_str(),
                    Some(*status),
                    "case {case:?}: expected the stored status, got {entry}"
                );
            }
        }

        // The same projection through the single-instance read, whose `Instance` envelope
        // repeats it: a member set that is right in one read and wrong in the other fails
        // here. Made once per lifecycle value, since a single-instance read takes no page
        // size for the enumeration's bounds to vary.
        for entry in &enumerated {
            let instance_arn = entry["InstanceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("status {status}: no InstanceArn in {entry}"));
            let case = format!("status {status} / DescribeInstance {instance_arn}");
            let (http, body) = call_in_region(
                &router,
                SEEDED_REGION,
                "DescribeInstance",
                &json!({"InstanceArn": instance_arn}),
            )
            .await;
            assert_eq!(
                http, 200,
                "case {case:?}: DescribeInstance expected 200, got {http} {body}"
            );
            assert_instance_view_members(&case, &body["Instance"]);
        }
    }
}

/// THE RULE on an instance enabled over the WIRE rather than seeded, read in both phases
/// its own creation takes it through: the member set a `CreateInstance` leaves behind is the
/// same one, `State` included, before and after the activation the clock owes falls due.
#[tokio::test]
async fn an_instance_enabled_over_the_wire_serves_the_same_member_set() {
    let (router, engine) = empty_account();
    let (http, created) = call_in_region(
        &router,
        WIRE_REGION,
        "CreateInstance",
        &json!({"Name": "member-set-wire-idc-0b74e2f1"}),
    )
    .await;
    assert_eq!(
        http, 200,
        "CreateInstance expected 200, got {http} {created}"
    );

    for in_flight in [true, false] {
        let (phase, due_at, expected_status) = if in_flight {
            (
                "creation in flight",
                SystemTime::now() + Duration::from_secs(3_600),
                state::STATUS_CREATE_IN_PROGRESS,
            )
        } else {
            (
                "activation due",
                SystemTime::now() - Duration::from_secs(1),
                state::STATUS_ACTIVE,
            )
        };
        hold_activation(&engine, due_at);

        for max_results in PAGE_SIZES {
            let case = format!("{phase} / MaxResults {max_results:?}");
            let entries = listed(&case, &router, WIRE_REGION, max_results).await;
            assert_eq!(
                entries.len(),
                1,
                "case {case:?}: the account's one instance is enumerated, got {entries:?}"
            );
            for entry in &entries {
                assert_instance_view_members(&case, entry);
                assert_eq!(
                    entry["Status"].as_str(),
                    Some(expected_status),
                    "case {case:?}: expected Status {expected_status:?}, got {entry}"
                );
            }
        }
    }
}

/// The upstream model snapshot this crate's shapes are generated from, embedded at compile
/// time so the declarations below are checked against the model rather than against a copy
/// of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json"
));

/// The overlay model the two unmodelled declarations are generated from
/// (`smithy-model/LOCAL-ADDITIONS.md`), embedded for the same reason: what it restates of
/// the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shapes a local type is declared from, each with the one member the overlay adds to
/// the snapshot's own: the instance VIEW's lifecycle structure, the envelope the
/// single-instance read repeats that view in, and the catalog entry the provider read
/// repeats beside its flat members.
const RESTATED_WITH_LOCAL_MEMBERS: &[(&str, &str)] = &[
    ("com.amazonaws.ssoadmin#InstanceMetadata", "State"),
    ("com.amazonaws.ssoadmin#DescribeInstanceResponse", "Instance"),
    (
        "com.amazonaws.ssoadmin#DescribeApplicationProviderResponse",
        "ApplicationProvider",
    ),
];

/// The shapes the overlay ADDS outright, which the snapshot declares nowhere: the
/// structure the local member carries.
const LOCAL_SHAPES: &[&str] = &["com.amazonaws.ssoadmin#InstanceState"];

/// The `shapes` object of one model document.
fn model_shapes(document: &str) -> Value {
    serde_json::from_str::<Value>(document)
        .expect("the model is JSON")
        .get("shapes")
        .expect("the model carries a shapes object")
        .clone()
}

/// The traits one declaration states that codegen reads, as a map — empty where it states
/// none of them.
///
/// What is kept is the `smithy.api` namespace: the `@required`, `@default`, `@length`,
/// `@pattern`, `@enumValue` and naming traits the validating walk and the codec read. What
/// is dropped is documentation (prose) and the endpoint-rules annotations, which are
/// resolved from the snapshot's own service and operation shapes — the overlay declares
/// neither, so nothing reads them from it.
fn read_traits(declaration: &Value) -> serde_json::Map<String, Value> {
    declaration["traits"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(trait_id, _)| trait_id.starts_with("smithy.api#"))
        .filter(|(trait_id, _)| trait_id.as_str() != "smithy.api#documentation")
        .map(|(trait_id, value)| (trait_id.clone(), value.clone()))
        .collect()
}

/// The declaration of one member or one list slot, as codegen reads it: its target and the
/// traits above.
fn as_read(declaration: &Value) -> Value {
    match declaration {
        Value::Null => Value::Null,
        _ => {
            let mut read = serde_json::Map::new();
            read.insert("target".to_string(), declaration["target"].clone());
            read.insert(
                "traits".to_string(),
                Value::Object(read_traits(declaration)),
            );
            Value::Object(read)
        }
    }
}

/// The traits one declaration states, `Null` where it states none but documentation.
fn traits_of(declaration: &Value) -> Value {
    match Value::Object(read_traits(declaration)) {
        Value::Object(traits) if traits.is_empty() => Value::Null,
        other => other,
    }
}

/// The local member the overlay adds to `shape_id`, or `None` for a shape it restates only
/// so another one can target it.
fn local_member_of(shape_id: &str) -> Option<&'static str> {
    RESTATED_WITH_LOCAL_MEMBERS
        .iter()
        .find(|(shape, _)| *shape == shape_id)
        .map(|(_, member)| *member)
}

/// The member names the overlay declares for one of its shapes, in the order it states
/// them.
fn overlay_members(shape_id: &str) -> Vec<String> {
    model_shapes(OVERLAY)[shape_id]["members"]
        .as_object()
        .unwrap_or_else(|| panic!("the overlay declares no members for {shape_id}"))
        .keys()
        .cloned()
        .collect()
}

/// THE RULE on the MODEL the unmodelled member is declared from: the overlay restates what
/// the snapshot declares EXACTLY — the same type, the same constraint traits, the same
/// member targets, the same enum values — and adds only the two members this service
/// answers with that the snapshot declares nowhere. Those restatements are what the
/// framework's validating walk enforces for the shapes declared from them, so a bound
/// dropped, mistyped or renamed there would silently loosen what the service admits or
/// change what it serves, with no single value on the wire looking wrong.
///
/// The rows span what the rule generalises over: WHICH shape the overlay restates (the
/// instance view, the single-instance read's whole response) and WHAT it restates of it (a
/// shape whose members a local type is declared from, and a shape restated only so those
/// members can target it, whose type and constraint traits are all codegen reads).
///
/// When a refreshed snapshot declares one of the local members itself, this fails and the
/// local declaration should give way to the modelled shape — which is the point: the
/// overlay exists only for what the snapshot does not carry.
#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_members_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        let case = shape_id.as_str();
        if LOCAL_SHAPES.contains(&case) {
            assert!(
                snapshot[case].is_null(),
                "case {case}: the snapshot now declares this shape, so the local \
                 declaration should give way to the modelled one"
            );
            continue;
        }
        let modelled = &snapshot[case];
        assert!(
            !modelled.is_null(),
            "case {case}: the overlay restates a shape the snapshot no longer declares"
        );
        assert_eq!(
            declared["type"], modelled["type"],
            "case {case}: expected the type the snapshot gives it, got {}",
            declared["type"]
        );
        // The traits of the SHAPE are where a string's length and pattern live, and they
        // are the bounds the validating walk enforces for every member that targets this
        // shape — so they must be the snapshot's own.
        assert_eq!(
            traits_of(declared),
            traits_of(modelled),
            "case {case}: expected the traits the snapshot states for it, got {}",
            declared["traits"]
        );
        // A list's element is read by codegen too.
        assert_eq!(
            as_read(&declared["member"]),
            as_read(&modelled["member"]),
            "case {case}: expected the member the snapshot declares, got {}",
            declared["member"]
        );

        let modelled_members = modelled["members"].as_object().cloned().unwrap_or_default();
        let declared_members = declared["members"].as_object().cloned().unwrap_or_default();
        for (member, restated_member) in &declared_members {
            if Some(member.as_str()) == local_member_of(case) {
                continue;
            }
            assert_eq!(
                as_read(restated_member),
                as_read(&modelled_members[member]),
                "case {case}: member {member} must be restated as the snapshot declares \
                 it, got {restated_member}"
            );
        }

        match local_member_of(case) {
            // A shape a local type is declared from restates EVERY member the snapshot
            // declares — a member left out would be dropped from the wire — and adds the
            // one local member and no other.
            Some(local) => {
                let dropped: Vec<&String> = modelled_members
                    .keys()
                    .filter(|member| !declared_members.contains_key(*member))
                    .collect();
                assert!(
                    dropped.is_empty(),
                    "case {case}: every modelled member must be restated, dropped \
                     {dropped:?}"
                );
                let added: Vec<&str> = declared_members
                    .keys()
                    .filter(|member| !modelled_members.contains_key(*member))
                    .map(String::as_str)
                    .collect();
                assert_eq!(
                    added,
                    vec![local],
                    "case {case}: expected the one local member, added {added:?}"
                );
            }
            // A shape restated only so the members above can target it is restated as far
            // as codegen reads it — its type and its own traits, which the assertions
            // above compared — and declares no member of its own beside the ones the
            // snapshot declares.
            None => {
                let added: Vec<&str> = declared_members
                    .keys()
                    .filter(|member| !modelled_members.contains_key(*member))
                    .map(String::as_str)
                    .collect();
                assert!(
                    added.is_empty(),
                    "case {case}: a shape restated to be targeted may add no member, \
                     added {added:?}"
                );
            }
        }
    }

    // Every shape a local type is declared from is restated by the overlay, and the
    // snapshot still declares none of the local members.
    for (shape_id, local) in RESTATED_WITH_LOCAL_MEMBERS {
        assert!(
            restated.contains_key(*shape_id),
            "case {shape_id}: the overlay must restate the shape its local type is \
             declared from"
        );
        let modelled_members = snapshot[*shape_id]["members"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            !modelled_members.contains_key(*local),
            "case {shape_id}: the snapshot now declares {local}, so the local declaration \
             should give way to the modelled member"
        );
    }
}

/// THE RULE tied back to the WIRE: the member set a served instance view carries is read
/// from the model it is declared from, so every member name an entry carries — and the
/// `Instance` envelope repeats — is one the overlay declares for the view, and the
/// unmodelled lifecycle structure is among them.
///
/// The rows span every lifecycle value the model declares and both instance read paths,
/// over an account holding three instances that differ in kind and name.
#[tokio::test]
async fn a_served_instance_view_carries_only_members_the_overlay_declares() {
    let declared = overlay_members("com.amazonaws.ssoadmin#InstanceMetadata");
    assert!(
        declared.iter().any(|member| member == UNMODELLED_MEMBER),
        "the overlay must declare the unmodelled {UNMODELLED_MEMBER:?} member, got \
         {declared:?}"
    );

    for status in INSTANCE_STATUS_VALUES {
        let (router, _engine) = seeded_account(status);
        let case = format!("status {status} / ListInstances");
        let entries = listed(&case, &router, SEEDED_REGION, None).await;
        assert_eq!(
            entries.len(),
            3,
            "case {case:?}: every instance the account holds is enumerated, got {entries:?}"
        );

        for entry in &entries {
            for member in entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is a structure: {entry}"))
                .keys()
            {
                assert!(
                    declared.contains(member),
                    "case {case:?}: served member {member:?} is not one the overlay \
                     declares for the instance view ({declared:?}), got {entry}"
                );
            }

            let instance_arn = entry["InstanceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no InstanceArn in {entry}"));
            let (http, body) = call_in_region(
                &router,
                SEEDED_REGION,
                "DescribeInstance",
                &json!({"InstanceArn": instance_arn}),
            )
            .await;
            assert_eq!(
                http, 200,
                "case {case:?}: DescribeInstance expected 200, got {http} {body}"
            );
            let envelope = body["Instance"]
                .as_object()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the response carries no Instance envelope: {body}")
                })
                .clone();
            for member in envelope.keys() {
                assert!(
                    declared.contains(member),
                    "case {case:?}: envelope member {member:?} is not one the overlay \
                     declares for the instance view ({declared:?}), got {body}"
                );
            }
        }
    }
}
