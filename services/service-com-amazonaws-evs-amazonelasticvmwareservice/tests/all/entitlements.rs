// SPDX-License-Identifier: CC0-1.0

//! Windows Server License entitlements, end to end over awsJson1_0.
//!
//! The rules under test, and where each comes from:
//!
//! * `CreateEntitlement` PERSISTS one entitlement per DISTINCT requested virtual
//!   machine in the named environment, and `ListVmEntitlements` reports exactly those
//!   records — whatever the number, size or ordering of the `vmIds` the request
//!   carried, and whether or not it named a VM twice.
//! * A new entitlement is `CREATING` and the clock owes it `CREATED`: "This is an
//!   asynchronous operation. Amazon EVS validates the specified virtual machines
//!   before starting usage tracking" (`CreateEntitlement` doc). `startedAt` appears
//!   with `CREATED` and does not move between reads.
//! * The operation is `@idempotent`
//!   (`smithy_query(query="operation",operation="CreateEntitlement")`): the connector,
//!   the virtual machine and the entitlement type identify an entitlement, so a repeat
//!   call for a VM already entitled of that type through the same connector reports the
//!   stored record and creates nothing. The entitlement-type part of that identity is
//!   owned by `tests/all/entitlement_type_identity.rs`.
//! * An entitlement belongs to the connector it was created through: "Returns existing
//!   entitlements for virtual machines associated with the specified environment and
//!   connector" (`ListVmEntitlements` doc). A `connectorId` the addressed environment
//!   holds no live connector for is refused instead of reported as an empty page —
//!   `tests/all/list_vm_entitlements_connector.rs` owns that rule.
//! * Every entitlement here is created through a connector this environment really
//!   holds, minted by `CreateEnvironmentConnector`: "using the provided vCenter Server
//!   connector" (`CreateEntitlement` doc). The refusal of a `connectorId` the
//!   environment holds no live vCenter connector for is owned by
//!   `tests/all/entitlement_connector.rs`.
//! * `maxResults` is a real page bound and `nextToken` resumes exactly where the page
//!   stopped (`nextToken` doc).
//! * An `environmentId` that names no live environment at this endpoint's Region is a
//!   `ResourceNotFoundException` and nothing is stored for it — an environment that
//!   has reached `DELETED` included ("The resource might not be specified correctly,
//!   or it may have a `state` of `DELETED`", `ResourceNotFoundException` doc). The
//!   well-formed-but-absent id table for every environment-addressed operation lives
//!   in `tests/all/error_wire_status.rs`, which owns that rule.
//!
//! The harness (`emulator`, `call`, `create`) is the one `environment_lifecycle`
//! already owns, so every test drives the service the way a client does: `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::entitlement_connector::{connector_in, environment_with_vcenter_connector};
use crate::environment_lifecycle::{call, emulator, error_type};

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// Long enough for the `CREATING` -> `CREATED` change the clock owes a new entitlement
/// to come due (its span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The store's bound on how many entitlements one environment holds
/// (`crate::state::MAX_ENTITLEMENTS_PER_ENVIRONMENT`).
const MAX_ENTITLEMENTS: usize =
    service_com_amazonaws_evs_amazonelasticvmwareservice::state::MAX_ENTITLEMENTS_PER_ENVIRONMENT;

/// A `CreateEntitlement` request for `vm_ids`, through `connector_id`.
fn create_request(environment_id: &str, connector_id: &str, vm_ids: &[String]) -> Value {
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
fn entitlements(case: &str, body: &Value) -> Vec<Value> {
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

/// Every entitlement of one environment and connector, following `nextToken` to the
/// end. `page` is the `maxResults` each call asks for, when the caller names one.
async fn list_every_page(
    router: &MultiServiceRouter,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    page: Option<i32>,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    // One more round than the records can need, so a token that never clears fails
    // here instead of looping forever.
    for _ in 0..=MAX_ENTITLEMENTS {
        let mut request = list_request(environment_id, connector_id);
        if let Some(page) = page {
            request["maxResults"] = json!(page);
        }
        if let Some(token) = next_token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListVmEntitlements", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: list expected 200, got {status} {body}"
        );
        collected.extend(entitlements(case, &body));
        match body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return collected,
        }
    }
    panic!("case {case:?}: nextToken never cleared after {MAX_ENTITLEMENTS} pages");
}

#[tokio::test]
async fn an_entitlement_is_created_per_distinct_vm_and_read_back_by_list() {
    let router = emulator();

    // Each row: the case, and the `vmIds` the request carries. The rows differ along
    // what the rule generalises over — the number of VMs, the length of a vmId (both
    // modelled bounds of `VmId`, len 4..1024, pattern ^vm-[0-9]+$), the ordering, and
    // whether a VM is named twice.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one VM", vec!["vm-1".to_string()]),
        (
            "several VMs",
            vec![
                "vm-1".to_string(),
                "vm-22".to_string(),
                "vm-333".to_string(),
            ],
        ),
        (
            "the same VMs in the reverse order",
            vec![
                "vm-333".to_string(),
                "vm-22".to_string(),
                "vm-1".to_string(),
            ],
        ),
        (
            "the same VM named twice",
            vec!["vm-7".to_string(), "vm-7".to_string(), "vm-8".to_string()],
        ),
        (
            "a vmId at the modelled 4-character minimum",
            vec!["vm-0".to_string()],
        ),
        (
            "a vmId at the modelled 1024-character maximum",
            vec![format!("vm-{}", "1".repeat(1021))],
        ),
        (
            "the modelled maximum of 100 vmIds",
            (1..=100).map(|ordinal| format!("vm-{ordinal}")).collect(),
        ),
    ];

    for (case, requested) in rows {
        // A fresh environment, with its own vCenter connector, per row — so each row
        // reads back only its own records and creates through a connector that exists.
        let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
        let distinct: BTreeSet<String> = requested.iter().cloned().collect();

        let (status, body) = call(
            &router,
            "CreateEntitlement",
            &create_request(&environment, &connector, &requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} {body}"
        );
        let created = entitlements(case, &body);
        assert_eq!(
            created.len(),
            distinct.len(),
            "case {case:?}: expected one entitlement per distinct vmId ({}), got {}",
            distinct.len(),
            created.len()
        );
        assert_eq!(
            reported_vm_ids(case, &created),
            distinct,
            "case {case:?}: the created entitlements must cover exactly the requested VMs, got {body}"
        );
        for entry in &created {
            for (member, expected) in [
                ("environmentId", json!(environment)),
                ("connectorId", json!(connector)),
                ("type", json!(WINDOWS_SERVER)),
                // Validation of the VM has not finished yet, so usage tracking has
                // not started: the status is CREATING and there is no startedAt.
                ("status", json!("CREATING")),
                ("startedAt", Value::Null),
            ] {
                assert_eq!(
                    entry[member], expected,
                    "case {case:?}: {member}: expected {expected}, got {}",
                    entry[member]
                );
            }
        }

        // The read-back: ListVmEntitlements must report the records the create stored.
        let listed = list_every_page(&router, case, &environment, &connector, None).await;
        assert_eq!(
            reported_vm_ids(case, &listed),
            distinct,
            "case {case:?}: the list must report exactly the stored entitlements, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_create_reports_the_stored_entitlement_and_adds_nothing() {
    let router = emulator();
    let (environment, connector) =
        environment_with_vcenter_connector(&router, "the repeat create").await;
    let first = vec!["vm-1".to_string(), "vm-2".to_string()];
    let overlapping = vec!["vm-2".to_string(), "vm-3".to_string()];

    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &first),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the first create: expected 200, got {status} {body}"
    );

    // An overlapping request creates only the VM that had no entitlement, and reports
    // the stored record for the one that did.
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &overlapping),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the overlapping create: expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_vm_ids("the overlapping create", &entitlements("the overlapping create", &body)),
        overlapping.iter().cloned().collect::<BTreeSet<String>>(),
        "case the overlapping create: it must report an entitlement for every vmId it named, got {body}"
    );
    let listed = list_every_page(&router, "after the overlap", &environment, &connector, None).await;
    assert_eq!(
        reported_vm_ids("after the overlap", &listed),
        ["vm-1", "vm-2", "vm-3"]
            .iter()
            .map(|vm| (*vm).to_string())
            .collect::<BTreeSet<String>>(),
        "case after the overlap: the store must hold one entitlement per VM, got {listed:?}"
    );

    // Once the entitlements have settled, a repeat create must not restart them: the
    // stored record keeps its status and its startedAt.
    std::thread::sleep(PAST_THE_TRANSITION);
    let settled = list_every_page(&router, "settled", &environment, &connector, None).await;
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &first),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the repeat create: expected 200, got {status} {body}"
    );
    for entry in entitlements("the repeat create", &body) {
        let vm_id = entry["vmId"].as_str().unwrap_or_default().to_string();
        let stored = settled
            .iter()
            .find(|candidate| candidate["vmId"].as_str() == Some(vm_id.as_str()))
            .unwrap_or_else(|| panic!("case the repeat create: {vm_id} was not stored"));
        assert_eq!(
            entry["status"], stored["status"],
            "case the repeat create: {vm_id}: expected the stored status {}, got {}",
            stored["status"], entry["status"]
        );
        assert_eq!(
            entry["startedAt"], stored["startedAt"],
            "case the repeat create: {vm_id}: expected the stored startedAt {}, got {}",
            stored["startedAt"], entry["startedAt"]
        );
    }
    let after = list_every_page(&router, "after the repeat", &environment, &connector, None).await;
    assert_eq!(
        after.len(),
        settled.len(),
        "case after the repeat: the repeat create must add no record, expected {} entitlements, got {}",
        settled.len(),
        after.len()
    );
}

#[tokio::test]
async fn an_entitlement_reaches_created_when_the_clock_owes_it_and_its_timestamps_hold() {
    let router = emulator();
    let (environment, connector) =
        environment_with_vcenter_connector(&router, "the clock").await;
    let requested = vec!["vm-10".to_string(), "vm-20".to_string()];

    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &requested),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the create: expected 200, got {status} {body}"
    );
    // The transient state is observable: a caller reading straight after the call
    // still sees CREATING.
    for entry in entitlements("straight after the create", &body) {
        assert_eq!(
            entry["status"],
            json!("CREATING"),
            "case straight after the create: expected CREATING, got {}",
            entry["status"]
        );
    }
    for entry in
        list_every_page(&router, "straight after the create", &environment, &connector, None).await
    {
        assert_eq!(
            entry["status"],
            json!("CREATING"),
            "case straight after the create: the list must report CREATING too, got {}",
            entry["status"]
        );
    }

    std::thread::sleep(PAST_THE_TRANSITION);
    let settled = list_every_page(&router, "settled", &environment, &connector, None).await;
    for entry in &settled {
        assert_eq!(
            entry["status"],
            json!("CREATED"),
            "case settled: the owed change must have been applied, got {}",
            entry["status"]
        );
        assert!(
            entry["startedAt"].as_f64().is_some(),
            "case settled: a CREATED entitlement reports when usage tracking started, got {entry}"
        );
    }

    // A read never moves a stamp: the second read reports what the first did.
    let again = list_every_page(&router, "read twice", &environment, &connector, None).await;
    assert_eq!(
        again, settled,
        "case read twice: a read must not change what a later caller sees, got {again:?}"
    );
}

#[tokio::test]
async fn entitlements_belong_to_the_connector_they_were_created_through() {
    // An environment holds at most ONE live connector of each type ("You can create
    // only one connector of each type per environment",
    // `CreateEnvironmentConnector` doc), so two vCenter connectors mean two
    // environments: each row is one environment, its own vCenter connector, and the
    // VMs entitled through it.
    let router = emulator();
    let mut rows: Vec<(&str, String, String, [&str; 2])> = Vec::new();
    for (case, vms) in [("the first", ["vm-1", "vm-2"]), ("the second", ["vm-3", "vm-4"])] {
        let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
        rows.push((case, environment, connector, vms));
    }

    for (case, environment, connector, vms) in &rows {
        let requested: Vec<String> = vms.iter().map(|vm| (*vm).to_string()).collect();
        let (status, body) = call(
            &router,
            "CreateEntitlement",
            &create_request(environment, connector, &requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} {body}"
        );
    }

    for (case, environment, connector, vms) in &rows {
        let listed = list_every_page(&router, case, environment, connector, None).await;
        assert_eq!(
            reported_vm_ids(case, &listed),
            vms.iter().map(|vm| (*vm).to_string()).collect::<BTreeSet<String>>(),
            "case {case:?}: a list must report only this connector's entitlements, got {listed:?}"
        );
        for entry in &listed {
            assert_eq!(
                entry["connectorId"],
                json!(connector),
                "case {case:?}: connectorId: expected {connector}, got {}",
                entry["connectorId"]
            );
        }
    }

    // Each row: the case, and the connector id the first environment is listed with — a
    // connector that environment really holds, of another type, which holds no
    // entitlement. The pair selects which records are reported ("Returns existing
    // entitlements for virtual machines associated with the specified environment and
    // connector"), so this is an empty page and never somebody else's records. A
    // connector id the environment holds NO connector for is a different answer —
    // `ResourceNotFoundException`, owned by `tests/all/list_vm_entitlements_connector.rs`.
    let (_, first_environment, first_connector, _) = &rows[0];
    let (_, _, other_connector, _) = &rows[1];
    let unused_type_connector =
        connector_in(&router, "an unused type", first_environment, "SDDC_MANAGER").await;
    let empty_cases: [(&str, &str); 1] =
        [("a connector of another type", unused_type_connector.as_str())];
    for (case, connector) in empty_cases {
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &list_request(first_environment, connector),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert!(
            entitlements(case, &body).is_empty(),
            "case {case:?}: expected no entitlements, got {body}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {case:?}: an exhausted list carries no nextToken, got {body}"
        );
    }
    assert_ne!(
        first_connector, other_connector,
        "the two environments must hold distinct connectors, got {first_connector}"
    );
}

#[tokio::test]
async fn max_results_bounds_the_page_and_next_token_resumes_where_it_stopped() {
    let router = emulator();
    let (environment, connector) = environment_with_vcenter_connector(&router, "paging").await;
    let requested: Vec<String> = (1..=7).map(|ordinal| format!("vm-{ordinal}")).collect();
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &requested),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the create: expected 200, got {status} {body}"
    );
    let all: BTreeSet<String> = requested.iter().cloned().collect();

    // Each row: the page size asked for, and the page sizes it must produce over the
    // seven stored entitlements.
    let rows: [(i32, Vec<usize>); 4] = [
        (1, vec![1, 1, 1, 1, 1, 1, 1]),
        (3, vec![3, 3, 1]),
        (7, vec![7]),
        (100, vec![7]),
    ];
    for (page, expected_pages) in rows {
        let case = format!("maxResults {page}");
        let mut sizes = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = list_request(&environment, &connector);
            request["maxResults"] = json!(page);
            if let Some(token) = next_token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListVmEntitlements", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            let entries = entitlements(&case, &body);
            sizes.push(entries.len());
            seen.extend(reported_vm_ids(&case, &entries));
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            sizes, expected_pages,
            "case {case:?}: expected the pages {expected_pages:?}, got {sizes:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            all,
            "case {case:?}: the pages together must report every stored entitlement, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            all.len(),
            "case {case:?}: no entitlement may be reported twice, got {seen:?}"
        );
    }

    // A token this service never issued is refused, not silently resumed.
    let mut request = list_request(&environment, &connector);
    request["nextToken"] = json!("not-a-token-this-service-issued");
    let (status, body) = call(&router, "ListVmEntitlements", &request).await;
    assert_eq!(
        status, 400,
        "case a forged nextToken: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_type(&body).contains("ValidationException"),
        "case a forged nextToken: expected a ValidationException, got {}",
        error_type(&body)
    );
}

#[tokio::test]
async fn an_environment_that_has_reached_deleted_holds_no_new_entitlement() {
    let router = emulator();
    let (environment, connector) =
        environment_with_vcenter_connector(&router, "a DELETED environment").await;
    let requested = vec!["vm-1".to_string()];

    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the delete: expected 200, got {status} {body}"
    );
    // Past the transition the environment is DELETED, and a create in it is a miss.
    std::thread::sleep(PAST_THE_TRANSITION);

    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &requested),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a DELETED environment: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "case a DELETED environment: expected a ResourceNotFoundException, got {}",
        error_type(&body)
    );
    assert_eq!(
        body["resourceId"],
        json!(environment),
        "case a DELETED environment: resourceId must echo the requested id, got {body}"
    );
    // And nothing was stored for it. The list cannot be the read-back here: the
    // environment's deletion took its connectors with it, so the handle this list
    // resolves through is gone and the list is refused — the store itself is what shows
    // the refused create persisted nothing.
    let (status, body) = call(
        &router,
        "ListVmEntitlements",
        &list_request(&environment, &connector),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a DELETED environment: a list through a connector that went away with the \
         environment is refused, got {status} {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "case a DELETED environment: expected a ResourceNotFoundException, got {}",
        error_type(&body)
    );
    let store = service_com_amazonaws_evs_amazonelasticvmwareservice::state::service_state(
        &*router.engine,
    )
    .expect("the in-memory engine carries this service's store");
    let stored = store
        .lock()
        .environments
        .get(&environment)
        .map(|stored| stored.entitlements.len())
        .unwrap_or_default();
    assert_eq!(
        stored, 0,
        "case a DELETED environment: the refused create must persist nothing, got \
         {stored} entitlement record(s)"
    );
}

#[tokio::test]
async fn an_environment_holds_at_most_the_stores_bound_of_entitlements() {
    let router = emulator();
    let (environment, connector) = environment_with_vcenter_connector(&router, "the bound").await;

    // Fill the environment to exactly the bound, 100 vmIds per call (the modelled
    // maximum of `vmIds`).
    let mut filled = 0usize;
    while filled < MAX_ENTITLEMENTS {
        let batch = (MAX_ENTITLEMENTS - filled).min(100);
        let requested: Vec<String> = (filled..filled + batch)
            .map(|ordinal| format!("vm-{ordinal}"))
            .collect();
        let (status, body) = call(
            &router,
            "CreateEntitlement",
            &create_request(&environment, &connector, &requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case filling to {}: expected 200, got {status} {body}",
            filled + batch
        );
        filled += batch;
    }

    // One more VM does not fit, and is refused with the operation's modelled
    // ValidationException naming the member whose entries cannot be admitted.
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(
            &environment,
            &connector,
            &[format!("vm-{MAX_ENTITLEMENTS}")],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the bound: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_type(&body).contains("ValidationException"),
        "case one past the bound: expected a ValidationException, got {}",
        error_type(&body)
    );
    assert!(
        body["message"].as_str().unwrap_or_default().contains("vmIds"),
        "case one past the bound: the message must name the refused member, got {body}"
    );

    // A VM that is already entitled still succeeds at the bound: it adds no record.
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &["vm-0".to_string()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an already-entitled VM at the bound: expected 200, got {status} {body}"
    );
}
