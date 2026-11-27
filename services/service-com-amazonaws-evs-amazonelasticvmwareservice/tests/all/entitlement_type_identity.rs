// SPDX-License-Identifier: CC0-1.0

//! `entitlementType` is part of an entitlement's identity, end to end over awsJson1_0.
//!
//! The rule under test: an entitlement is identified by the connector it was created
//! through, the virtual machine it entitles AND the type of entitlement it is — the
//! three required members every operation that addresses an entitlement takes
//! (`smithy_query(query="operation", operation="CreateEntitlement,DeleteEntitlement,
//! ListVmEntitlements")`: `connectorId` and `entitlementType` are required on all three,
//! `vmIds` on the two that name machines). So, for one (connector, VM) pair:
//!
//! * a `CreateEntitlement` for a type that pair holds no record of MINTS one and reports
//!   the record of the type the request named, never a record of another type;
//! * the `@idempotent` repeat of that create finds the record of its own type;
//! * a `DeleteEntitlement` removes the record of the type it names and leaves the
//!   records of every other type standing;
//! * a `ListVmEntitlements` reports the records of the type it names and no others.
//!
//! The identity itself is single-homed in `evs::state::entitlement_key`, and the
//! selection a listing reports in `evs::state::StoredEntitlement::selected_by`, so this
//! suite drives the key directly and then drives every operation that resolves a record
//! through it.
//!
//! `EntitlementType` declares a single value today
//! (`smithy_query(query="shape", shape="EntitlementType")`, the catalog this crate
//! generates from it: `evs::ops::types::ENTITLEMENT_TYPE_VALUES`), and a value outside
//! that set is refused on the wire with a `ValidationException` — which the last test
//! here pins for all three operations. A second type is therefore seeded through this
//! service's own store, exactly as `tests/all/entitlement_vm_id_identity.rs` seeds the
//! `EntitlementStatus` values no client can reach; everything the rule is about is then
//! driven over the wire.
//!
//! The harness (`emulator`, `call`) is the one `environment_lifecycle` already owns and
//! the environment-with-a-vCenter-connector setup is `entitlement_connector`'s, so every
//! test drives the service the way a client does: `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::environment_with_vcenter_connector;
use crate::environment_lifecycle::{call, emulator, error_type, Emulator};

/// Every `EntitlementType` the model declares, from the catalog this crate generates
/// from the model (`smithy_query(query="shape", shape="EntitlementType")`).
const ENTITLEMENT_TYPES: &[&str] = evs::ops::types::ENTITLEMENT_TYPE_VALUES;

/// The type of entitlement a client can ask for today — the one value the catalog above
/// carries.
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// Types that are NOT in the modelled value set, each standing for a second type the
/// store holds a record of: one that sorts before the modelled value, one the modelled
/// value is a prefix of (so it sorts after), and one that is a prefix of the modelled
/// value. They differ along what the identity rule generalises over — the type string
/// itself and where it falls in the store's order — and none of them can be asked for
/// over the wire, which is why the records are seeded.
const OTHER_TYPES: &[&str] = &["SQL_SERVER", "WINDOWS_SERVER_DATACENTER", "WIN"];

/// The virtual machine the two records of a pair entitle.
const SUBJECT_VM: &str = "vm-1";

/// A `createdAt` in the past, written onto the seeded record of the other type so a path
/// that replaced or re-stamped it is visibly distinguishable from one that left it
/// exactly as it stands — without any assertion depending on how fast the host runs.
const MARKER_CREATED_AT_MS: i64 = 1_700_000_000_000;

/// A `CreateEntitlement`/`DeleteEntitlement` request body: the two operations take the
/// same four members.
fn entitlement_request(
    environment_id: &str,
    connector_id: &str,
    entitlement_type: &str,
    vm_ids: &[&str],
) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": entitlement_type,
        "vmIds": vm_ids,
    })
}

/// A `ListVmEntitlements` request for one environment, connector and entitlement type.
fn list_request(environment_id: &str, connector_id: &str, entitlement_type: &str) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": entitlement_type,
    })
}

/// The `entitlements` a 200 response reports, each as `(vmId, type, status)`.
async fn reported(
    router: &Emulator,
    case: &str,
    operation: &str,
    request: &Value,
) -> Vec<(String, String, String)> {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} {body}"
    );
    body["entitlements"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} reported no entitlements in {body}"))
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry[name]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: {operation}: no {name} in {entry}"))
                    .to_string()
            };
            (member("vmId"), member("type"), member("status"))
        })
        .collect()
}

/// Every entitlement this service's store holds for `environment_id`, as
/// `(vmId, type)`, in the order the store pages over them.
///
/// The status is deliberately not part of this view: a record the clock owes a change
/// moves on its own, so a comparison of two reads taken at different instants would turn
/// on how fast the host runs. The statuses this suite asserts are the ones a response
/// reports, and the one of the seeded record that owes no change at all.
fn stored_entitlements(
    router: &Emulator,
    case: &str,
    environment_id: &str,
) -> Vec<(String, String)> {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"))
        .entitlements
        .values()
        .map(|entitlement| (entitlement.vm_id.clone(), entitlement.entitlement_type.clone()))
        .collect()
}

/// The stored record of `vm_id` and `entitlement_type`, resolved by the key this crate
/// single-homes the identity in — so a record the key does not place under that triple
/// is not found here either.
fn stored_record(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    vm_id: &str,
    entitlement_type: &str,
) -> Option<evs::state::StoredEntitlement> {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"))
        .entitlements
        .get(&evs::state::entitlement_key(connector_id, vm_id, entitlement_type))
        .cloned()
}

/// Seed one entitlement of `entitlement_type` for [`SUBJECT_VM`], already `CREATED` and
/// owing no change, straight into this service's store at the key its identity places it
/// under — the stand-in for a second `EntitlementType` no client can ask for.
fn seed_entitlement_of_type(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    entitlement_type: &str,
) {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = state.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"));
    let mut record = evs::state::StoredEntitlement::creating(
        connector_id,
        SUBJECT_VM,
        entitlement_type,
        MARKER_CREATED_AT_MS,
        evs::state::PendingStatus::starting_now(evs::state::ENTITLEMENT_CREATED),
    );
    record.status = evs::state::ENTITLEMENT_CREATED.to_string();
    record.started_at_ms = Some(MARKER_CREATED_AT_MS);
    record.pending = None;
    environment.entitlements.insert(
        evs::state::entitlement_key(connector_id, SUBJECT_VM, entitlement_type),
        record,
    );
}

/// Assert the seeded record of the other type is exactly as it was seeded: nothing a call
/// addressed to another type did may have replaced, re-stamped or removed it.
fn assert_other_type_untouched(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    entitlement_type: &str,
) {
    let record = stored_record(
        router,
        case,
        environment_id,
        connector_id,
        SUBJECT_VM,
        entitlement_type,
    )
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the store must still hold the {entitlement_type} entitlement of \
             {SUBJECT_VM}"
        )
    });
    assert_eq!(
        (
            record.entitlement_type.as_str(),
            record.status.as_str(),
            record.created_at_ms,
            record.stopped_at_ms,
        ),
        (
            entitlement_type,
            evs::state::ENTITLEMENT_CREATED,
            MARKER_CREATED_AT_MS,
            None,
        ),
        "case {case:?}: a record of another type must be left exactly as it stands, got \
         {record:?}"
    );
}

#[test]
fn the_store_key_of_an_entitlement_names_its_connector_its_vm_and_its_type() {
    // The identity itself: two entitlements are the same entitlement only when their
    // connector, their virtual machine AND their type all agree. The rows differ along
    // each of the three parts, including ids and types that only look alike and parts
    // that could run into one another if the key were a plain concatenation.
    let triples: &[(&str, &str, &str)] = &[
        ("cnctr-0123456789", "vm-1", WINDOWS_SERVER),
        // The same pair, a different type.
        ("cnctr-0123456789", "vm-1", "SQL_SERVER"),
        // The same pair, a type the first row's type is a prefix of.
        ("cnctr-0123456789", "vm-1", "WINDOWS_SERVER_DATACENTER"),
        // The same type, a different virtual machine — and one whose id only looks like
        // another's.
        ("cnctr-0123456789", "vm-10", WINDOWS_SERVER),
        ("cnctr-0123456789", "vm-01", WINDOWS_SERVER),
        // The same type and machine, a different connector.
        ("cnctr-9876543210", "vm-1", WINDOWS_SERVER),
        ("cnctr-9876543210", "vm-1", "SQL_SERVER"),
    ];

    for (left_index, left) in triples.iter().enumerate() {
        let left_key = evs::state::entitlement_key(left.0, left.1, left.2);
        // The key is a function of the triple: asking twice answers the same key, which
        // is what makes an idempotent repeat of a create find the record it stored.
        let repeat = evs::state::entitlement_key(left.0, left.1, left.2);
        assert_eq!(
            left_key, repeat,
            "case {left:?}: one triple has one key, got {left_key:?} and {repeat:?}"
        );
        for right in triples.iter().skip(left_index + 1) {
            let right_key = evs::state::entitlement_key(right.0, right.1, right.2);
            assert_ne!(
                left_key, right_key,
                "case {left:?} vs {right:?}: triples that differ identify different \
                 entitlements, got the shared key {left_key:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_entitlement_of_one_type_is_not_the_entitlement_of_another_on_the_same_pair() {
    let router = emulator();

    for other_type in OTHER_TYPES {
        let case = format!("a {other_type} entitlement beside a {WINDOWS_SERVER} one");
        // A fresh environment with its own vCenter connector per row, so each row only
        // ever sees its own records.
        let (environment, connector) = environment_with_vcenter_connector(&router, &case).await;
        seed_entitlement_of_type(&router, &case, &environment, &connector, other_type);

        // The create: the pair holds no record of the type the request names, so one is
        // minted and the record REPORTED is of that type — not the record of the other
        // type that already occupies the pair.
        let created = reported(
            &router,
            &case,
            "CreateEntitlement",
            &entitlement_request(&environment, &connector, WINDOWS_SERVER, &[SUBJECT_VM]),
        )
        .await;
        assert_eq!(
            created,
            vec![(
                SUBJECT_VM.to_string(),
                WINDOWS_SERVER.to_string(),
                evs::state::ENTITLEMENT_CREATING.to_string(),
            )],
            "case {case:?}: the create must mint and report the entitlement of the type it \
             names, got {created:?}"
        );
        // Both records exist: the pair holds one entitlement per type.
        let held = stored_entitlements(&router, &case, &environment);
        assert_eq!(
            held.len(),
            2,
            "case {case:?}: the pair holds one record per type, got {held:?}"
        );
        assert_other_type_untouched(&router, &case, &environment, &connector, other_type);

        // The listing reports the records of the type it names and no others.
        let listed = reported(
            &router,
            &case,
            "ListVmEntitlements",
            &list_request(&environment, &connector, WINDOWS_SERVER),
        )
        .await;
        assert_eq!(
            listed
                .iter()
                .map(|(vm, kind, _)| (vm.as_str(), kind.as_str()))
                .collect::<Vec<_>>(),
            vec![(SUBJECT_VM, WINDOWS_SERVER)],
            "case {case:?}: the listing must report only the type it names, got {listed:?}"
        );

        // The `@idempotent` repeat finds the record of ITS OWN type, and mints nothing.
        let repeated = reported(
            &router,
            &case,
            "CreateEntitlement",
            &entitlement_request(&environment, &connector, WINDOWS_SERVER, &[SUBJECT_VM]),
        )
        .await;
        assert_eq!(
            repeated
                .iter()
                .map(|(vm, kind, _)| (vm.as_str(), kind.as_str()))
                .collect::<Vec<_>>(),
            vec![(SUBJECT_VM, WINDOWS_SERVER)],
            "case {case:?}: the repeat must report the stored record of its own type, got \
             {repeated:?}"
        );
        let held = stored_entitlements(&router, &case, &environment);
        assert_eq!(
            held.len(),
            2,
            "case {case:?}: the repeat creates no second record, got {held:?}"
        );
        assert_other_type_untouched(&router, &case, &environment, &connector, other_type);

        // The delete removes the record of the type it names, and leaves the record of
        // the other type standing.
        let deleted = reported(
            &router,
            &case,
            "DeleteEntitlement",
            &entitlement_request(&environment, &connector, WINDOWS_SERVER, &[SUBJECT_VM]),
        )
        .await;
        assert_eq!(
            deleted
                .iter()
                .map(|(vm, kind, status)| (vm.as_str(), kind.as_str(), status.as_str()))
                .collect::<Vec<_>>(),
            vec![(SUBJECT_VM, WINDOWS_SERVER, evs::state::ENTITLEMENT_DELETED)],
            "case {case:?}: the delete must remove and report the entitlement of the type \
             it names, got {deleted:?}"
        );
        assert!(
            stored_record(
                &router,
                &case,
                &environment,
                &connector,
                SUBJECT_VM,
                WINDOWS_SERVER,
            )
            .is_none(),
            "case {case:?}: the record the delete named is gone, got {:?}",
            stored_entitlements(&router, &case, &environment)
        );
        assert_other_type_untouched(&router, &case, &environment, &connector, other_type);

        // And the listing of the deleted type is empty again, while the record of the
        // other type is still the one record the store holds.
        let listed = reported(
            &router,
            &case,
            "ListVmEntitlements",
            &list_request(&environment, &connector, WINDOWS_SERVER),
        )
        .await;
        assert!(
            listed.is_empty(),
            "case {case:?}: the listing must report no record of a type that has none, got \
             {listed:?}"
        );
        let held = stored_entitlements(&router, &case, &environment);
        assert_eq!(
            held,
            vec![(SUBJECT_VM.to_string(), (*other_type).to_string())],
            "case {case:?}: only the record of the other type is left, got {held:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_entitlement_type_is_served_and_anything_else_is_refused() {
    let router = emulator();

    // Each value the model declares is a type a client can create, list and delete
    // through, one record per type — so the day the model declares a second value, this
    // table drives it over the wire too.
    let case = "every modelled entitlement type";
    let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
    for entitlement_type in ENTITLEMENT_TYPES {
        let created = reported(
            &router,
            case,
            "CreateEntitlement",
            &entitlement_request(&environment, &connector, entitlement_type, &[SUBJECT_VM]),
        )
        .await;
        assert_eq!(
            created
                .iter()
                .map(|(vm, kind, _)| (vm.as_str(), kind.as_str()))
                .collect::<Vec<_>>(),
            vec![(SUBJECT_VM, *entitlement_type)],
            "case {case:?}: a create of {entitlement_type} must report a record of that \
             type, got {created:?}"
        );
    }
    let held = stored_entitlements(&router, case, &environment);
    assert_eq!(
        held.len(),
        ENTITLEMENT_TYPES.len(),
        "case {case:?}: one record per modelled type, got {held:?}"
    );
    for entitlement_type in ENTITLEMENT_TYPES {
        let listed = reported(
            &router,
            case,
            "ListVmEntitlements",
            &list_request(&environment, &connector, entitlement_type),
        )
        .await;
        assert_eq!(
            listed
                .iter()
                .map(|(vm, kind, _)| (vm.as_str(), kind.as_str()))
                .collect::<Vec<_>>(),
            vec![(SUBJECT_VM, *entitlement_type)],
            "case {case:?}: the listing of {entitlement_type} reports the record of that \
             type, got {listed:?}"
        );
    }

    // A value outside the modelled set is refused by every operation that takes the
    // member, which is why a second type can only be seeded through the store.
    for operation in ["CreateEntitlement", "DeleteEntitlement", "ListVmEntitlements"] {
        for entitlement_type in OTHER_TYPES {
            let case = format!("{operation} naming the unmodelled type {entitlement_type}");
            let request = if operation == "ListVmEntitlements" {
                list_request(&environment, &connector, entitlement_type)
            } else {
                entitlement_request(&environment, &connector, entitlement_type, &[SUBJECT_VM])
            };
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a type outside the modelled set is refused, got {status} {body}"
            );
            assert_eq!(
                error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {}",
                error_type(&body)
            );
        }
    }
    // The refusals changed nothing: the records of the modelled types are still the only
    // ones the environment holds.
    let after = stored_entitlements(&router, case, &environment);
    assert_eq!(
        after, held,
        "case {case:?}: a refused request stores nothing, expected {held:?}, got {after:?}"
    );
}
