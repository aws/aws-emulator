// SPDX-License-Identifier: CC0-1.0

//! The rules an entitlement record is resolved by that this crate single-homes, each
//! driven through every operation that reaches it, end to end over awsJson1_0.
//!
//! The first is the identity of a record. The connector an entitlement was created
//! through, the virtual machine it entitles and the type it is are what identify it —
//! `crate::state::entitlement_key`, the key `CreateEntitlement`, `ListVmEntitlements`
//! and `DeleteEntitlement` all address entitlements by, and `tests/all/entitlement_type_identity.rs`
//! owns the entitlement-type part of it — so, with the connector and the type the same
//! for every entry of one request, a `vmIds` list naming the same virtual machine twice
//! names ONE entitlement, and each of the two operations that take a `vmIds` list
//! reports it once, in the order the request first named it.
//!
//! `CreateEntitlement` and `DeleteEntitlement` reach that rule through the single
//! `crate::state::distinct_vm_ids` it is stated in, so this suite drives ONE table of
//! `vmIds` lists through BOTH operations: the create that mints the records and the
//! delete that removes them must name exactly the same virtual machines, in the same
//! order, for every row.
//!
//! The second rule this suite owns is the other line an entitlement record is resolved
//! by, and it is single-homed the same way: whether a record still TRACKS a virtual
//! machine's usage (`evs::state::StoredEntitlement::tracks_usage`), which decides which
//! records an environment's deletion stops in each of its two halves and which records
//! stand in the way of deleting the connector they were created through. One table of
//! `EntitlementStatus` values is driven through every path that asks it, so a status
//! moved to either side of the line moves for all of them.
//!
//! The harness (`emulator`, `call`) is the one `environment_lifecycle` already owns and
//! the environment-with-a-vCenter-connector setup is `entitlement_connector`'s, so every
//! test drives the service the way a client does: `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::environment_with_vcenter_connector;
use crate::environment_lifecycle::{call, emulator, error_type, Emulator};

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// A `CreateEntitlement`/`DeleteEntitlement` request body for `vm_ids`, through
/// `connector_id`: the two operations take the same four members.
fn entitlement_request(environment_id: &str, connector_id: &str, vm_ids: &[String]) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
        "vmIds": vm_ids,
    })
}

/// The `vmId`s of a response's `entitlements`, in the order the response reported them.
fn reported_vm_ids(case: &str, operation: &str, body: &Value) -> Vec<String> {
    body["entitlements"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} reported no entitlements list in {body}"))
        .iter()
        .map(|entry| {
            entry["vmId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: {operation}: no vmId in {entry}"))
                .to_string()
        })
        .collect()
}

/// Call `operation` with `vm_ids` and return the `vmId`s it reported, in order.
async fn reported(
    router: &Emulator,
    case: &str,
    operation: &str,
    environment_id: &str,
    connector_id: &str,
    vm_ids: &[String],
) -> Vec<String> {
    let (status, body) = call(
        router,
        operation,
        &entitlement_request(environment_id, connector_id, vm_ids),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} {body}"
    );
    reported_vm_ids(case, operation, &body)
}

/// `vm_ids` as the owned strings a request body carries.
fn vms(vm_ids: &[&str]) -> Vec<String> {
    vm_ids.iter().map(|vm| (*vm).to_string()).collect()
}

#[tokio::test]
async fn one_vm_ids_list_names_one_entitlement_per_distinct_vm_in_both_operations() {
    let router = emulator();

    // The longest `vmId` the model admits (`VmId`, len 4..1024, pattern ^vm-[0-9]+$).
    let longest = format!("vm-{}", "1".repeat(1021));
    // The modelled maximum of 100 `vmIds` (`VmIdList`, len 1..100), every entry the same
    // virtual machine.
    let hundred_of_one = vec!["vm-9".to_string(); 100];
    // The same bound, with 50 distinct machines each named twice, interleaved.
    let fifty_pairs: Vec<String> = (1..=50)
        .flat_map(|ordinal| [format!("vm-{ordinal}"), format!("vm-{ordinal}")])
        .collect();
    let fifty_distinct: Vec<String> = (1..=50).map(|ordinal| format!("vm-{ordinal}")).collect();

    // Each row: the case, the `vmIds` the request carries, and the distinct machines it
    // names in first-seen order. The rows differ along what the rule generalises over —
    // how many entries repeat, how far apart the repeats are, how many times one machine
    // is named, the length of a repeated id, ids that only look alike, and the modelled
    // maximum number of entries.
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "no repeat at all",
            vms(&["vm-1", "vm-22", "vm-333"]),
            vms(&["vm-1", "vm-22", "vm-333"]),
        ),
        (
            "adjacent repeat",
            vms(&["vm-7", "vm-7", "vm-8"]),
            vms(&["vm-7", "vm-8"]),
        ),
        (
            "repeat several entries apart",
            vms(&["vm-1", "vm-2", "vm-3", "vm-1"]),
            vms(&["vm-1", "vm-2", "vm-3"]),
        ),
        (
            "a repeat before a machine first seen later",
            vms(&["vm-4", "vm-4", "vm-5", "vm-4", "vm-6"]),
            vms(&["vm-4", "vm-5", "vm-6"]),
        ),
        (
            "one machine named five times",
            vms(&["vm-5", "vm-5", "vm-5", "vm-5", "vm-5"]),
            vms(&["vm-5"]),
        ),
        (
            "ids that only look alike are distinct machines",
            vms(&["vm-01", "vm-1", "vm-01", "vm-10"]),
            vms(&["vm-01", "vm-1", "vm-10"]),
        ),
        (
            "a vmId at the modelled 1024-character maximum, named twice",
            vec![longest.clone(), "vm-1".to_string(), longest.clone()],
            vec![longest.clone(), "vm-1".to_string()],
        ),
        (
            "the modelled maximum of 100 entries, all one machine",
            hundred_of_one.clone(),
            vms(&["vm-9"]),
        ),
        (
            "the modelled maximum of 100 entries, 50 machines named twice",
            fifty_pairs.clone(),
            fifty_distinct.clone(),
        ),
    ];

    for (case, requested, distinct) in rows {
        // A fresh environment with its own vCenter connector per row, so each row
        // creates and deletes only its own records.
        let (environment, connector) = environment_with_vcenter_connector(&router, case).await;

        // The create: one record per distinct machine, reported in first-seen order.
        let created = reported(
            &router,
            case,
            "CreateEntitlement",
            &environment,
            &connector,
            &requested,
        )
        .await;
        assert_eq!(
            created, distinct,
            "case {case:?}: CreateEntitlement must report one entitlement per distinct \
             virtual machine, expected {distinct:?}, got {created:?}"
        );

        // The delete: the SAME machines the create named, from the same list, so the two
        // operations cannot disagree about which machines one request names.
        let deleted = reported(
            &router,
            case,
            "DeleteEntitlement",
            &environment,
            &connector,
            &requested,
        )
        .await;
        assert_eq!(
            deleted, distinct,
            "case {case:?}: DeleteEntitlement must report one entitlement per distinct \
             virtual machine, expected {distinct:?}, got {deleted:?}"
        );
        assert_eq!(
            deleted, created,
            "case {case:?}: both operations fold one vmIds list the same way, expected \
             {created:?}, got {deleted:?}"
        );

        // And the store is empty afterwards: the delete removed exactly the records the
        // create minted, so nothing is left for the same list to name.
        let left = reported(
            &router,
            case,
            "DeleteEntitlement",
            &environment,
            &connector,
            &requested,
        )
        .await;
        assert!(
            left.is_empty(),
            "case {case:?}: every record the create minted was removed, so a repeat \
             delete removes nothing, got {left:?}"
        );
    }
}

/// Every `EntitlementStatus` the model declares, from the catalog this crate generated
/// from the model (`smithy_query(query="shape", shape="EntitlementStatus")`), paired with
/// whether a record in that status still TRACKS a virtual machine's usage — the side of
/// the line this crate single-homes in `evs::state::StoredEntitlement::tracks_usage`,
/// which every reader and writer of an entitlement resolves records by.
const ENTITLEMENT_STATUS_SIDES: &[(&str, bool)] = &[
    // The record is finished with: the enum declares no state between tracking and
    // stopped, so these two are the statuses in which usage is tracked no longer.
    ("DELETED", false),
    ("ENTITLEMENT_REMOVED", false),
    // Everything else still tracks: the virtual machine is being validated (`CREATING`),
    // tracking has started (`CREATED`), the machine's licence state needs attention
    // (`AT_RISK`), or the validation concluded without tracking (`CREATE_FAILED`) — none
    // of them a record this service has finished with, so each is still an entitlement a
    // caller has to remove and one an environment's deletion stops.
    ("CREATING", true),
    ("CREATED", true),
    ("AT_RISK", true),
    ("CREATE_FAILED", true),
];

/// The virtual machine every row of the status table entitles.
const SUBJECT_VM: &str = "vm-1";

/// A `stoppedAt` in the past, written onto a record that is already finished with so a
/// path that re-stamps it is visibly distinguishable from one that must leave it exactly
/// as it stands — without either assertion depending on how fast the host runs.
const MARKER_STOPPED_AT_MS: i64 = 1_700_000_000_000;

/// The stored entitlement of `vm_id`, as this service's own store holds it.
///
/// The store rather than the wire, because `AT_RISK`, `CREATE_FAILED` and
/// `ENTITLEMENT_REMOVED` are reached by nothing a client can call, and because an
/// entitlement is listed THROUGH a connector that goes away with its environment: the
/// store is what shows what became of the record behind that refusal.
fn stored_entitlement(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    vm_id: &str,
) -> evs::state::StoredEntitlement {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds the environment {environment_id}"))
        .entitlements
        .values()
        .find(|entitlement| entitlement.vm_id == vm_id)
        .unwrap_or_else(|| panic!("case {case:?}: the environment holds an entitlement of {vm_id}"))
        .clone()
}

/// Put the stored entitlement of [`SUBJECT_VM`] into `status`, owing no change of its
/// own, and carrying the `stoppedAt` a record in that status carries: the
/// [`MARKER_STOPPED_AT_MS`] of a record already finished with, and none for one that
/// still tracks usage.
///
/// Every row is seeded through this service's own store, as
/// `tests/all/connector_deletion.rs` seeds the `ConnectorState` values, which keeps the
/// row set exactly the modelled catalog; everything the rule is about is then driven over
/// the wire.
fn force_entitlement_status(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    status: &str,
    tracks_usage: bool,
) {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = state.lock();
    let stored = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds the environment {environment_id}"))
        .entitlements
        .values_mut()
        .find(|entitlement| entitlement.vm_id == SUBJECT_VM)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the environment holds an entitlement of {SUBJECT_VM}")
        });
    stored.status = status.to_string();
    stored.pending = None;
    stored.stopped_at_ms = if tracks_usage {
        None
    } else {
        Some(MARKER_STOPPED_AT_MS)
    };
}

/// An environment with a vCenter connector and one entitlement of [`SUBJECT_VM`] through
/// it, forced into `status`.
async fn environment_with_entitlement_in_status(
    router: &Emulator,
    case: &str,
    status: &str,
    tracks_usage: bool,
) -> (String, String) {
    let (environment, connector) = environment_with_vcenter_connector(router, case).await;
    let (status_code, body) = call(
        router,
        "CreateEntitlement",
        &entitlement_request(&environment, &connector, &vms(&[SUBJECT_VM])),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: creating the entitlement expected 200, got {status_code} {body}"
    );
    force_entitlement_status(router, case, &environment, status, tracks_usage);
    (environment, connector)
}

/// Pull the deletion the clock owes `environment_id` forward so it is due NOW, and answer
/// the epoch milliseconds it is then owed at.
///
/// The span a transition comes due in is never slept on by this service — the due instant
/// is stored and compared — so a test that needs the terminal half of a deletion applied
/// by an OPERATION moves that instant instead of waiting for it.
fn make_the_deletion_due_now(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let due_at = Instant::now();
    let due_at_ms = evs::state::now_ms();
    let mut store = state.lock();
    let pending = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds the environment {environment_id}"))
        .pending
        .as_mut()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the accepted deletion of {environment_id} is owed a transition")
        });
    pending.due_at = due_at;
    pending.due_at_ms = due_at_ms;
    store.note_transition_due(due_at);
    due_at_ms
}

/// Assert that a record the path under test had to leave exactly as it stands is
/// untouched: the status it was seeded in, and the stop stamp it carried.
fn assert_left_as_it_stands(case: &str, status: &str, record: &evs::state::StoredEntitlement) {
    assert_eq!(
        record.status, status,
        "case {case:?}: a record that tracks no usage any more must keep its status, got {record:?}"
    );
    assert_eq!(
        record.stopped_at_ms,
        Some(MARKER_STOPPED_AT_MS),
        "case {case:?}: a record that tracks no usage any more must keep the instant it \
         stopped at ({MARKER_STOPPED_AT_MS}), got {record:?}"
    );
}

/// Assert that a record the path under test had to stop carries the one stop this service
/// writes: the terminal status, and `stoppedAt` at the instant the stop was APPLIED, which
/// lies between `from_ms` and `to_ms` inclusive — the instant the call that applied it was
/// owed at, or the window it ran in, according to the path.
fn assert_stopped_between(
    case: &str,
    from_ms: i64,
    to_ms: i64,
    record: &evs::state::StoredEntitlement,
) {
    assert_eq!(
        record.status,
        evs::state::ENTITLEMENT_DELETED,
        "case {case:?}: a record that was tracking usage must be stopped, got {record:?}"
    );
    let stopped_at_ms = record.stopped_at_ms.unwrap_or_else(|| {
        panic!("case {case:?}: a stopped record carries the instant it stopped, got {record:?}")
    });
    assert!(
        (from_ms..=to_ms).contains(&stopped_at_ms),
        "case {case:?}: the stop must be dated at the instant it was applied, between \
         {from_ms} and {to_ms}, got {record:?}"
    );
    assert!(
        record.pending.is_none(),
        "case {case:?}: a stopped record owes no further change, got {record:?}"
    );
}

#[tokio::test]
async fn the_usage_tracking_line_is_one_rule_for_every_path_that_resolves_an_entitlement() {
    // One predicate decides, for every `EntitlementStatus`, whether a record still tracks
    // a virtual machine's usage. Three paths ask it, and they must agree row by row: the
    // half of an environment's deletion that STARTS the deletion of what belongs to the
    // environment stops exactly the records on the tracking side of the line, the half
    // that FINISHES it stops exactly the same records, and the reader behind the
    // connector delete's precondition counts exactly those records as being in the way.
    let router = emulator();

    // Every modelled status has a row, so the table and the catalog cannot drift apart.
    for status in evs::ops::types::ENTITLEMENT_STATUS_VALUES {
        assert!(
            ENTITLEMENT_STATUS_SIDES.iter().any(|(row, _)| row == status),
            "the table must cover the modelled status {status:?}"
        );
    }
    assert_eq!(
        ENTITLEMENT_STATUS_SIDES.len(),
        evs::ops::types::ENTITLEMENT_STATUS_VALUES.len(),
        "the table must name only modelled statuses, got {ENTITLEMENT_STATUS_SIDES:?}"
    );

    for (status, tracks_usage) in ENTITLEMENT_STATUS_SIDES {
        // The half the accepted `DeleteEnvironment` applies: a record that still tracks
        // usage stops at the instant the deletion was accepted, and one that tracks none
        // is left exactly as it stands.
        let case = format!("DeleteEnvironment accepted over a {status} entitlement");
        let (environment, _) =
            environment_with_entitlement_in_status(&router, &case, status, *tracks_usage).await;

        // The window the delete ran in: the stop it applies is dated inside it, which is
        // what makes the stamp the instant the deletion was accepted rather than one the
        // record carried before or one the clock owes later.
        let arrived_at_ms = evs::state::now_ms();
        let (status_code, body) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": &environment }),
        )
        .await;
        let answered_at_ms = evs::state::now_ms();
        assert_eq!(
            status_code, 200,
            "case {case:?}: the delete expected 200, got {status_code} {body}"
        );
        assert_eq!(
            body["environment"]["environmentState"],
            json!("DELETING"),
            "case {case:?}: the delete reports the environment going away, got {body}"
        );
        let record = stored_entitlement(&router, &case, &environment, SUBJECT_VM);
        if *tracks_usage {
            assert_stopped_between(&case, arrived_at_ms, answered_at_ms, &record);
        } else {
            assert_left_as_it_stands(&case, status, &record);
        }

        // The terminal half, applied by the deletion coming DUE — reached here by a read,
        // which materialises a change the clock had already made due. The record is put
        // into the row's status after the deletion was accepted, so it is the record that
        // reached the environment while its deletion was in flight: the one the first
        // half never saw.
        let case =
            format!("the deletion of an environment holding a {status} entitlement coming due");
        let (environment, _) =
            environment_with_entitlement_in_status(&router, &case, status, *tracks_usage).await;
        let (status_code, body) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": &environment }),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the delete expected 200, got {status_code} {body}"
        );
        force_entitlement_status(&router, &case, &environment, status, *tracks_usage);
        let due_at_ms = make_the_deletion_due_now(&router, &case, &environment);

        let (status_code, body) = call(
            &router,
            "GetEnvironment",
            &json!({ "environmentId": &environment }),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the read expected 200, got {status_code} {body}"
        );
        assert_eq!(
            body["environment"]["environmentState"],
            json!("DELETED"),
            "case {case:?}: the read must report the deletion the clock made due, got {body}"
        );
        let record = stored_entitlement(&router, &case, &environment, SUBJECT_VM);
        if *tracks_usage {
            // The terminal half dates the stop at the instant the deletion was OWED at,
            // never at the instant of the read, so two reads report one `stoppedAt`.
            assert_stopped_between(&case, due_at_ms, due_at_ms, &record);
        } else {
            assert_left_as_it_stands(&case, status, &record);
        }

        // And the reader the same line single-homes, asked by the connector delete's
        // precondition: "Before deleting a connector, you must remove all entitlements
        // that are associated with the same vCenter" (`DeleteEnvironmentConnector` doc),
        // so a record on the tracking side of the line refuses that delete and a record
        // on the other side is in nobody's way.
        let case = format!("DeleteEnvironmentConnector against a {status} entitlement");
        let (environment, connector) =
            environment_with_entitlement_in_status(&router, &case, status, *tracks_usage).await;
        let (status_code, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &json!({ "environmentId": &environment, "connectorId": &connector }),
        )
        .await;
        if *tracks_usage {
            assert_eq!(
                status_code, 400,
                "case {case:?}: a connector whose vCenter still has an entitlement cannot \
                 be deleted, got {status_code} {body}"
            );
            assert_eq!(
                error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {}",
                error_type(&body)
            );
            assert_eq!(
                body["fieldList"][0]["name"],
                json!("connectorId"),
                "case {case:?}: the refusal must name the member it refuses, got {body}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains('1') && message.contains("DeleteEntitlement"),
                "case {case:?}: the message must say how many entitlements are in the way \
                 and how to remove them, got {message:?}"
            );
        } else {
            assert_eq!(
                status_code, 200,
                "case {case:?}: a connector whose vCenter has no entitlement left is \
                 deleted, got {status_code} {body}"
            );
            assert_eq!(
                body["connector"]["state"],
                json!("DELETING"),
                "case {case:?}: the delete must start the connector's teardown, got {body}"
            );
        }
    }
}
