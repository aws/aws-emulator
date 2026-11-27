// SPDX-License-Identifier: CC0-1.0

//! `DeleteEntitlement`, end to end over awsJson1_0.
//!
//! The rules under test, and where each comes from:
//!
//! * A delete REMOVES the entitlement records the request names from the stored
//!   environment and reports exactly those records, stopped: "Deleting an entitlement
//!   stops usage tracking for the specified virtual machines" (`DeleteEntitlement` doc),
//!   with the response being "A list of the deleted entitlements"
//!   (`DeleteEntitlementResponse$entitlements` doc). `ListVmEntitlements` then reports the
//!   records the delete did not name and nothing else.
//! * A removed record is reported in the terminal `DELETED` status with a `stoppedAt`,
//!   "The date and time that the entitlement stopped" (`VmEntitlement$stoppedAt` doc).
//!   `EntitlementStatus` declares no state between tracking and stopped
//!   (`smithy_query(query="shape", shape="EntitlementStatus")`), so the stop is immediate
//!   — whether the entitlement had settled into `CREATED` or was still `CREATING` — and
//!   no later clock change revives it.
//! * The connector, the virtual machine and the entitlement type identify an entitlement
//!   (`tests/all/entitlement_type_identity.rs` owns the type part), and the connector and
//!   the type are the same for every entry of one request, so a `vmIds` list naming one VM
//!   twice names one record, and re-entitling that triple after a delete mints a fresh
//!   record rather than finding the deleted one.
//! * The operation is `@idempotent` (`smithy_query(query="operation",
//!   operation="DeleteEntitlement")`) and the end state is what the call asks for — the
//!   named virtual machines are not entitled — so a `vmId` the environment holds no such
//!   record for is a success that removes nothing, exactly as a repeat of
//!   `DisassociateEipFromVlan`'s removal is.
//! * The environment and the connector are resources the request ADDRESSES: an
//!   `environmentId` this service holds no live environment for in the request's Region,
//!   or a `connectorId` that environment holds no live connector for, is a
//!   `ResourceNotFoundException` naming the resource the request asked for ("The resource
//!   might not be specified correctly, or it may have a `state` of `DELETED`",
//!   `ResourceNotFoundException` doc), and nothing is removed for it. Every modelled
//!   client error of this awsJson1_0 service is reported with HTTP 400 —
//!   `tests/all/error_wire_status.rs` owns that rule.
//!
//! The harness (`emulator`, `call`) is the one `environment_lifecycle` already owns, and
//! the environment-with-a-vCenter-connector setup is `entitlement_connector`'s, so every
//! test drives the service the way a client does: `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::entitlement_connector::{connector_in, environment_with_vcenter_connector};
use crate::environment_lifecycle::{call, emulator, error_type, Emulator, PAST_THE_TRANSITION};

/// The only `EntitlementType` the model declares, from the catalog this crate generated
/// from the model (`smithy_query(query="shape", shape="EntitlementType")`).
const ENTITLEMENT_TYPES: &[&str] =
    service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::ENTITLEMENT_TYPE_VALUES;

/// Every `EntitlementStatus` the model declares, from the same generated catalog
/// (`smithy_query(query="shape", shape="EntitlementStatus")`).
const ENTITLEMENT_STATUSES: &[&str] =
    service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::ENTITLEMENT_STATUS_VALUES;

/// The `EntitlementType` every request here carries.
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The status a stopped entitlement is reported in.
const DELETED: &str = "DELETED";

/// The status a new entitlement is reported in, before the validation the clock owes it
/// comes due.
const CREATING: &str = "CREATING";

/// The status an entitlement whose validation has come due is reported in.
const CREATED: &str = "CREATED";

/// A `CreateEntitlement` request for `vm_ids`, through `connector_id`.
fn create_request(environment_id: &str, connector_id: &str, vm_ids: &[String]) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
        "vmIds": vm_ids,
    })
}

/// A `DeleteEntitlement` request for `vm_ids`, through `connector_id`.
pub(crate) fn delete_request(environment_id: &str, connector_id: &str, vm_ids: &[String]) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
        "vmIds": vm_ids,
    })
}

/// A `ListVmEntitlements` request for one environment and connector.
fn list_request(environment_id: &str, connector_id: &str) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
    })
}

/// The `entitlements` of a response body.
pub(crate) fn entitlements(case: &str, body: &Value) -> Vec<Value> {
    body["entitlements"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no entitlements list in {body}"))
        .clone()
}

/// The `vmId`s of a list of reported entitlements.
fn reported_vm_ids(case: &str, entries: &[Value]) -> BTreeSet<String> {
    entries
        .iter()
        .map(|entry| {
            entry["vmId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no vmId in {entry}"))
                .to_string()
        })
        .collect()
}

/// `vm_ids` as the owned strings the request builders take.
pub(crate) fn vms(vm_ids: &[&str]) -> Vec<String> {
    vm_ids.iter().map(|vm| (*vm).to_string()).collect()
}

/// Create the entitlements for `vm_ids`, failing the test if the create is refused.
pub(crate) async fn entitle(
    router: &Emulator,
    case: &str,
    environment: &str,
    connector: &str,
    vm_ids: &[String],
) {
    let (status, body) = call(
        router,
        "CreateEntitlement",
        &create_request(environment, connector, vm_ids),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create expected 200, got {status} {body}"
    );
}

/// Every entitlement one environment and connector still holds, following `nextToken` to
/// the end.
pub(crate) async fn list_all(router: &Emulator, case: &str, environment: &str, connector: &str) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    // One round per possible page plus one, so a token that never clears fails here
    // instead of looping forever.
    for _ in 0..=64 {
        let mut request = list_request(environment, connector);
        if let Some(token) = next_token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListVmEntitlements", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read-back expected 200, got {status} {body}"
        );
        collected.extend(entitlements(case, &body));
        match body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return collected,
        }
    }
    panic!("case {case:?}: nextToken never cleared");
}

/// Assert one reported entitlement is the stopped form of a record that was removed.
pub(crate) fn assert_stopped(case: &str, entry: &Value, environment: &str, connector: &str) {
    for (member, expected) in [
        ("environmentId", json!(environment)),
        ("connectorId", json!(connector)),
        ("type", json!(WINDOWS_SERVER)),
        ("status", json!(DELETED)),
    ] {
        assert_eq!(
            entry[member], expected,
            "case {case:?}: {member}: expected {expected}, got {}",
            entry[member]
        );
    }
    assert!(
        entry["stoppedAt"].as_f64().is_some(),
        "case {case:?}: a stopped entitlement reports when tracking stopped, got {entry}"
    );
}

#[tokio::test]
async fn a_delete_removes_exactly_the_named_records_and_reports_them_stopped() {
    let router = emulator();

    // The modelled catalogs this suite's fixed values come from, asserted here so the
    // table and the model cannot drift apart.
    assert_eq!(
        ENTITLEMENT_TYPES,
        [WINDOWS_SERVER],
        "the modelled EntitlementType catalog must be the single value this suite sends, got \
         {ENTITLEMENT_TYPES:?}"
    );
    assert!(
        ENTITLEMENT_STATUSES.contains(&DELETED),
        "the modelled EntitlementStatus catalog must declare {DELETED}, got \
         {ENTITLEMENT_STATUSES:?}"
    );

    // The longest and the shortest `vmId` the model admits (`VmId`, len 4..1024, pattern
    // ^vm-[0-9]+$).
    let longest = format!("vm-{}", "1".repeat(1021));
    let hundred: Vec<String> = (1..=100).map(|ordinal| format!("vm-{ordinal}")).collect();

    // Each row: the case, the VMs entitled first, and the VMs the delete names. The rows
    // differ along what the rule generalises over — how many records exist, how many the
    // delete names, the order it names them in, a VM named twice, the modelled length
    // bounds of a vmId, VMs that were never entitled, and the modelled maximum of 100
    // entries in `vmIds`.
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("the only record", vms(&["vm-1"]), vms(&["vm-1"])),
        (
            "one of several records",
            vms(&["vm-1", "vm-22", "vm-333"]),
            vms(&["vm-22"]),
        ),
        (
            "every record, named in the reverse order",
            vms(&["vm-1", "vm-22", "vm-333"]),
            vms(&["vm-333", "vm-22", "vm-1"]),
        ),
        (
            "the same VM named twice",
            vms(&["vm-7", "vm-8"]),
            vms(&["vm-7", "vm-7"]),
        ),
        (
            "a vmId at the modelled 4-character minimum",
            vms(&["vm-0", "vm-1"]),
            vms(&["vm-0"]),
        ),
        (
            "a vmId at the modelled 1024-character maximum",
            vec![longest.clone(), "vm-1".to_string()],
            vec![longest.clone()],
        ),
        (
            "VMs the environment holds no entitlement for",
            vms(&["vm-1"]),
            vms(&["vm-2", "vm-3"]),
        ),
        (
            "a mix of entitled and never-entitled VMs",
            vms(&["vm-1", "vm-2"]),
            vms(&["vm-2", "vm-9"]),
        ),
        (
            "the modelled maximum of 100 vmIds",
            hundred.clone(),
            hundred.clone(),
        ),
    ];

    for (case, entitled, requested) in rows {
        // A fresh environment with its own vCenter connector per row, so each row reads
        // back only its own records.
        let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
        entitle(&router, case, &environment, &connector, &entitled).await;

        let held: BTreeSet<String> = entitled.iter().cloned().collect();
        let named: BTreeSet<String> = requested.iter().cloned().collect();
        let removed: BTreeSet<String> = held.intersection(&named).cloned().collect();
        let left: BTreeSet<String> = held.difference(&named).cloned().collect();

        let (status, body) = call(
            &router,
            "DeleteEntitlement",
            &delete_request(&environment, &connector, &requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected 200, got {status} {body}"
        );
        let reported = entitlements(case, &body);
        assert_eq!(
            reported.len(),
            removed.len(),
            "case {case:?}: expected one entitlement per record removed ({}), got {}",
            removed.len(),
            reported.len()
        );
        assert_eq!(
            reported_vm_ids(case, &reported),
            removed,
            "case {case:?}: the response must report exactly the records removed, got {body}"
        );
        for entry in &reported {
            assert_stopped(case, entry, &environment, &connector);
        }

        // The read-back: the store holds the records the delete did not name, and no
        // record of one it did.
        let listed = list_all(&router, case, &environment, &connector).await;
        assert_eq!(
            reported_vm_ids(case, &listed),
            left,
            "case {case:?}: the list must report exactly the records the delete left, got \
             {listed:?}"
        );
        for entry in &listed {
            assert_ne!(
                entry["status"],
                json!(DELETED),
                "case {case:?}: a removed record must be gone, not reported, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeat_delete_removes_nothing_and_the_pair_can_be_entitled_again() {
    let router = emulator();
    let case = "the repeat delete";
    let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
    let both = vms(&["vm-1", "vm-2"]);
    entitle(&router, case, &environment, &connector, &both).await;

    // Each row: the case, and how many records the call removes. The first call removes
    // both; every later call asks for the same end state, which already holds.
    let rows: [(&str, usize); 3] = [
        ("the first delete", 2),
        ("the repeat delete", 0),
        ("a third delete", 0),
    ];
    for (row, expected) in rows {
        let (status, body) = call(
            &router,
            "DeleteEntitlement",
            &delete_request(&environment, &connector, &both),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            entitlements(row, &body).len(),
            expected,
            "case {row:?}: expected {expected} deleted entitlement(s), got {body}"
        );
    }
    let listed = list_all(&router, case, &environment, &connector).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: every record was removed, so the list reports none, got {listed:?}"
    );

    // A delete addressed to an environment that holds no entitlement at all is the same
    // success that removes nothing.
    let (other_environment, other_connector) =
        environment_with_vcenter_connector(&router, "an environment with no entitlement").await;
    let (status, body) = call(
        &router,
        "DeleteEntitlement",
        &delete_request(&other_environment, &other_connector, &both),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an environment with no entitlement: expected 200, got {status} {body}"
    );
    assert!(
        entitlements("an environment with no entitlement", &body).is_empty(),
        "case an environment with no entitlement: nothing was removed, got {body}"
    );

    // The record is GONE, not kept as a stopped tombstone: the same (connector, VM) pair
    // is entitled again and the create mints a fresh record, `CREATING` with no
    // stoppedAt.
    entitle(&router, "the re-entitlement", &environment, &connector, &both).await;
    let again = list_all(&router, "the re-entitlement", &environment, &connector).await;
    assert_eq!(
        reported_vm_ids("the re-entitlement", &again),
        both.iter().cloned().collect::<BTreeSet<String>>(),
        "case the re-entitlement: the pair must be entitled again, got {again:?}"
    );
    for entry in &again {
        assert_eq!(
            entry["status"],
            json!(CREATING),
            "case the re-entitlement: a fresh record starts at {CREATING}, got {}",
            entry["status"]
        );
        assert_eq!(
            entry["stoppedAt"],
            Value::Null,
            "case the re-entitlement: a fresh record has not stopped, got {entry}"
        );
    }
}

#[tokio::test]
async fn the_stop_is_immediate_whether_or_not_the_entitlement_had_settled() {
    let router = emulator();

    // Each row: the case, the VM, and whether to wait for the CREATING -> CREATED change
    // the clock owes the new entitlement before deleting it.
    let rows: [(&str, &str, bool); 2] = [
        ("an entitlement that is still CREATING", "vm-11", false),
        ("an entitlement that has reached CREATED", "vm-10", true),
    ];
    for (case, vm_id, settle) in rows {
        let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
        let requested = vms(&[vm_id]);
        entitle(&router, case, &environment, &connector, &requested).await;

        let mut started_at: Option<f64> = None;
        if settle {
            std::thread::sleep(PAST_THE_TRANSITION);
            let listed = list_all(&router, case, &environment, &connector).await;
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: expected the one record, got {listed:?}"
            );
            assert_eq!(
                listed[0]["status"],
                json!(CREATED),
                "case {case:?}: the owed change must have come due, got {}",
                listed[0]["status"]
            );
            started_at = listed[0]["startedAt"].as_f64();
            assert!(
                started_at.is_some(),
                "case {case:?}: a {CREATED} entitlement reports when tracking started, got {}",
                listed[0]
            );
        }

        let (status, body) = call(
            &router,
            "DeleteEntitlement",
            &delete_request(&environment, &connector, &requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected 200, got {status} {body}"
        );
        let reported = entitlements(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the one record removed, got {body}"
        );
        assert_stopped(case, &reported[0], &environment, &connector);
        if let Some(started_at) = started_at {
            let stopped_at = reported[0]["stoppedAt"].as_f64().unwrap_or_default();
            assert!(
                stopped_at >= started_at,
                "case {case:?}: tracking cannot stop before it started, expected a stoppedAt \
                 at or after {started_at}, got {stopped_at}"
            );
        }

        // No change the clock still owed revives the record: past the span the create's
        // own transition would have come due in, the store still holds nothing.
        std::thread::sleep(PAST_THE_TRANSITION);
        let listed = list_all(&router, case, &environment, &connector).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: the removed record must stay removed, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_removal_clears_the_entitlements_in_the_way_of_the_connectors_deletion() {
    // "Before deleting a connector, you must remove all entitlements that are associated
    // with the same vCenter" (`DeleteEnvironmentConnector` doc, the rule
    // `tests/all/connector_deletion.rs` owns): `DeleteEntitlement` is the operation that
    // removes them, so a connector whose deletion those records blocked is deletable
    // once this call has removed them.
    let router = emulator();
    let case = "the connector's entitlements";
    let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
    let held = vms(&["vm-1", "vm-2", "vm-3"]);
    entitle(&router, case, &environment, &connector, &held).await;

    let connector_request = json!({
        "environmentId": environment,
        "connectorId": connector,
    });
    let (status, body) = call(&router, "DeleteEnvironmentConnector", &connector_request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the entitlements must block the connector's deletion, got \
         {status} {body}"
    );

    let (status, body) = call(
        &router,
        "DeleteEntitlement",
        &delete_request(&environment, &connector, &held),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete expected 200, got {status} {body}"
    );
    assert_eq!(
        entitlements(case, &body).len(),
        held.len(),
        "case {case:?}: every held record must be removed, got {body}"
    );

    let (status, body) = call(&router, "DeleteEnvironmentConnector", &connector_request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: nothing is in the way any more, so the connector is deleted, got \
         {status} {body}"
    );
}

#[tokio::test]
async fn the_environment_and_the_connector_are_resolved_before_anything_is_removed() {
    let router = emulator();
    let case = "the environment under test";
    let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
    let (other_environment, other_connector) =
        environment_with_vcenter_connector(&router, "another environment").await;
    let held = vms(&["vm-1", "vm-2"]);
    entitle(&router, case, &environment, &connector, &held).await;

    // An environment that has reached DELETED is gone for this call too, so one is
    // prepared here: the delete below leaves it DELETING and the wait settles it.
    let (gone_environment, gone_connector) =
        environment_with_vcenter_connector(&router, "a DELETED environment").await;
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": gone_environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a DELETED environment: the environment delete expected 200, got {status} {body}"
    );
    std::thread::sleep(PAST_THE_TRANSITION);

    // A connector of this environment that is not a vCenter one, to show the TYPE is not
    // what the refusal is about: it resolves, so the call is served and removes nothing.
    let other_type = connector_in(&router, "an unused type", &environment, "SDDC_MANAGER").await;

    // Each row: the case, the environmentId and connectorId the request carries, and the
    // resource the refusal must name (`resourceType`, `resourceId`). The rows differ
    // along which handle fails to resolve — a well-formed environment id no environment
    // holds (spanning the modelled `^(env-[a-zA-Z0-9]{10})$`: digits, lower case, upper
    // case, mixed), an environment that has reached DELETED, a never-minted connector id,
    // and a connector id another environment holds.
    let rows: Vec<(String, String, String, &str, String)> = vec![
        (
            "a never-minted environment id, all digits".to_string(),
            "env-0000000000".to_string(),
            connector.clone(),
            "environment",
            "env-0000000000".to_string(),
        ),
        (
            "a never-minted environment id, lower case".to_string(),
            "env-aaaaaaaaaa".to_string(),
            connector.clone(),
            "environment",
            "env-aaaaaaaaaa".to_string(),
        ),
        (
            "a never-minted environment id, upper case".to_string(),
            "env-ZZZZZZZZZZ".to_string(),
            connector.clone(),
            "environment",
            "env-ZZZZZZZZZZ".to_string(),
        ),
        (
            "a never-minted environment id, mixed".to_string(),
            "env-0A1b2C3d4E".to_string(),
            connector.clone(),
            "environment",
            "env-0A1b2C3d4E".to_string(),
        ),
        (
            "an environment that has reached DELETED".to_string(),
            gone_environment.clone(),
            gone_connector.clone(),
            "environment",
            gone_environment.clone(),
        ),
        (
            "a never-minted connector id".to_string(),
            environment.clone(),
            "cnctr-0123456789".to_string(),
            "connector",
            "cnctr-0123456789".to_string(),
        ),
        (
            "another environment's connector id".to_string(),
            environment.clone(),
            other_connector.clone(),
            "connector",
            other_connector.clone(),
        ),
    ];

    for (row, environment_id, connector_id, resource_type, resource_id) in &rows {
        let (status, body) = call(
            &router,
            "DeleteEntitlement",
            &delete_request(environment_id, connector_id, &held),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
            "ResourceNotFoundException",
            "case {row:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
        assert_eq!(
            body["resourceType"],
            json!(resource_type),
            "case {row:?}: unexpected resourceType, got {body}"
        );
        assert_eq!(
            body["resourceId"],
            json!(resource_id),
            "case {row:?}: resourceId must echo the handle the request named, got {body}"
        );
        assert!(
            body["entitlements"].is_null(),
            "case {row:?}: a refusal reports no entitlement, got {body}"
        );
    }

    // Nothing was removed by any refusal: the records this environment holds are intact.
    let listed = list_all(&router, case, &environment, &connector).await;
    assert_eq!(
        reported_vm_ids(case, &listed),
        held.iter().cloned().collect::<BTreeSet<String>>(),
        "case {case:?}: a refused delete must remove nothing, got {listed:?}"
    );

    // A connector of another type resolves, so that call is SERVED — it removes nothing,
    // because no entitlement was created through it.
    let (status, body) = call(
        &router,
        "DeleteEntitlement",
        &delete_request(&environment, &other_type, &held),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a connector of another type: expected 200, got {status} {body}"
    );
    assert!(
        entitlements("a connector of another type", &body).is_empty(),
        "case a connector of another type: it holds no entitlement, so nothing is removed, \
         got {body}"
    );

    // And the pair that does resolve is still served, so the refusals above are the
    // lookups' answer and not the operation's answer to everything.
    let (status, body) = call(
        &router,
        "DeleteEntitlement",
        &delete_request(&environment, &connector, &held),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the live pair: expected 200, got {status} {body}"
    );
    assert_eq!(
        entitlements("the live pair", &body).len(),
        held.len(),
        "case the live pair: expected every held record removed, got {body}"
    );
    assert_ne!(
        other_environment, environment,
        "the two environments must be distinct, got {environment}"
    );
}
