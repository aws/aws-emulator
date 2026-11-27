// SPDX-License-Identifier: CC0-1.0

//! The asynchronous namespace operations: what a caller sees WHILE one is still running,
//! and what it sees once the clock has settled it.
//!
//! Cloud Map answers `DeleteNamespace` and `CreatePrivateDnsNamespace` with an
//! `OperationId` rather than with the resource, and the work belongs to the operation:
//!
//! * a delete's removal is the change its `SUCCESS` carries, so `GetNamespace` still
//!   answers with the namespace while the operation is non-terminal and
//!   `NamespaceNotFound` only once it has succeeded;
//! * a private namespace is "visible only inside a specified Amazon VPC", so a `Vpc` the
//!   account holds no VPC for leaves the association impossible: the operation lands
//!   `FAIL` with the `ErrorCode`/`ErrorMessage` saying so, and the namespace it staged is
//!   rolled back with it.
//!
//! The rows below drive the due instant from the STORE rather than from the wall clock —
//! the service records when a transition is owed and materialises it on read — so what is
//! asserted is the rule and not how fast the host runs.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, the same path the emulator
//! serves in production; the harness lives in `cloud_map_core`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{call, emulator_with_engine, failure, ok, refusal, REGION};

/// How long a poll for a clock-owed transition may take before the test gives up.
///
/// Generous on purpose: the span the service records is sub-second, and this only keeps a
/// test that will never see the transition from hanging.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// The statuses an `Operation` record can no longer move from.
const TERMINAL_STATUSES: [&str; 2] = ["SUCCESS", "FAIL"];

/// Read `GetOperation` again until the record reports a terminal status, and answer it.
///
/// Waits for the CONDITION rather than for a duration: the service stores the instant a
/// transition is due and materialises it on read, never sleeping for it.
pub(crate) async fn settled_operation(router: &MultiServiceRouter, operation_id: &str) -> Value {
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let read = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
        let status = read["Operation"]["Status"].as_str().unwrap_or_default();
        if TERMINAL_STATUSES.contains(&status) {
            return read;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "operation {operation_id} never reached a terminal status, last read {read}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Read `GetNamespace` again until it answers `NamespaceNotFound`, which is what the
/// namespace's delete operation carries when it succeeds.
pub(crate) async fn deleted_namespace(
    router: &MultiServiceRouter,
    case: &str,
    namespace_id: &str,
) {
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let (status, answer) = call(router, "GetNamespace", json!({ "Id": namespace_id })).await;
        if status != 200 {
            let code = answer["__type"]
                .as_str()
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default();
            assert_eq!(
                (status, code),
                (400, "NamespaceNotFound"),
                "case {case:?}: the deleted namespace must answer NamespaceNotFound, got \
                 {status} {answer}"
            );
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the namespace {namespace_id} was never removed, last read {answer}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Move the due instant of one operation record, so a test decides WHEN the clock owes the
/// transition instead of waiting for it.
///
/// `owed = true` moves it into the past (the transition is due) and `false` far enough into
/// the future that no test run reaches it.
pub(crate) fn set_operation_due(
    engine: &Arc<engine::InMemoryEngine>,
    operation_id: &str,
    owed: bool,
) {
    let state =
        engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    let mut store = state.lock();
    let record = store
        .operations
        .get_mut(operation_id)
        .unwrap_or_else(|| panic!("the operation {operation_id} must be on file"));
    record.success_due_at = if owed {
        SystemTime::now() - Duration::from_secs(1)
    } else {
        SystemTime::now() + Duration::from_secs(3_600)
    };
}

/// Create a namespace through `operation` and answer `(operation id, namespace id)`.
async fn created_namespace(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (String, String) {
    let created = ok(router, operation, body).await;
    let operation_id = created["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("{operation} answers an OperationId, got {created}"))
        .to_string();
    let filed = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    let namespace_id = filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .unwrap_or_else(|| panic!("the CREATE_NAMESPACE operation targets its namespace, got {filed}"))
        .to_string();
    (operation_id, namespace_id)
}

/// The namespace ids one `ListNamespaces` page carries.
async fn listed_namespace_ids(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(router, "ListNamespaces", json!({})).await;
    page["Namespaces"]
        .as_array()
        .unwrap_or_else(|| panic!("ListNamespaces answers a Namespaces list, got {page}"))
        .iter()
        .filter_map(|summary| summary["Id"].as_str())
        .map(str::to_string)
        .collect()
}

/// While a delete's operation is still non-terminal the namespace reads back; once that
/// operation reports `SUCCESS` it is gone from every read and its name is free again.
#[tokio::test]
async fn a_namespace_stays_readable_until_its_delete_operation_succeeds() {
    let (router, engine) = emulator_with_engine();

    // Rows differ along the kind of namespace and along the form the delete names it by —
    // `DeleteNamespaceRequest$Id` is modelled as an `Arn`, so an id and an ARN both reach
    // the one record.
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "an HTTP namespace named by id",
            "CreateHttpNamespace",
            json!({ "Name": "async-del-http" }),
            false,
        ),
        (
            "an HTTP namespace named by ARN",
            "CreateHttpNamespace",
            json!({ "Name": "async-del-http-arn" }),
            true,
        ),
        (
            "a public DNS namespace named by id",
            "CreatePublicDnsNamespace",
            json!({ "Name": "async-del-public.example.com" }),
            false,
        ),
    ];

    for (case, create, body, by_arn) in cases {
        let (create_operation, namespace_id) = created_namespace(&router, create, body).await;
        // The create's own operation is settled first, so only the delete's record is
        // still owed a transition below.
        settled_operation(&router, &create_operation).await;
        let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
        let namespace_arn = read["Namespace"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a stored namespace carries its ARN"))
            .to_string();
        let name = read["Namespace"]["Name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a stored namespace carries its Name"))
            .to_string();

        let named = if by_arn {
            namespace_arn.clone()
        } else {
            namespace_id.clone()
        };
        let deleted = ok(&router, "DeleteNamespace", json!({ "Id": named })).await;
        let delete_operation = deleted["OperationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: DeleteNamespace answers an OperationId"))
            .to_string();

        // The clock does not owe the transition yet, so the delete is still running — and
        // the namespace is still readable, by BOTH the forms that reach it.
        set_operation_due(&engine, &delete_operation, false);
        let polled = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": delete_operation }),
        )
        .await;
        assert_eq!(
            polled["Operation"]["Status"],
            json!("SUBMITTED"),
            "case {case:?}: the delete is still running, got {polled}"
        );
        for form in [&namespace_id, &namespace_arn] {
            let read = ok(&router, "GetNamespace", json!({ "Id": form })).await;
            assert_eq!(
                read["Namespace"]["Id"],
                json!(namespace_id),
                "case {case:?}: the namespace must still read back by {form} while its delete \
                 is running, got {read}"
            );
        }
        assert!(
            listed_namespace_ids(&router).await.contains(&namespace_id),
            "case {case:?}: a namespace whose delete is still running is still listed"
        );
        // A second delete of the same namespace is the duplicate of the one in flight.
        let (status, code) = failure(&router, "DeleteNamespace", json!({ "Id": namespace_id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "DuplicateRequest"),
            "case {case:?}: a delete already in progress is a duplicate request"
        );

        // The clock now owes it: the operation reports SUCCESS and the namespace is gone.
        set_operation_due(&engine, &delete_operation, true);
        let settled = settled_operation(&router, &delete_operation).await;
        assert_eq!(
            settled["Operation"]["Status"],
            json!("SUCCESS"),
            "case {case:?}: a delete that was owed its terminal status succeeds, got {settled}"
        );
        for form in [&namespace_id, &namespace_arn] {
            let (status, code) = failure(&router, "GetNamespace", json!({ "Id": form })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NamespaceNotFound"),
                "case {case:?}: the namespace must be gone under {form} once the delete \
                 succeeded"
            );
        }
        assert!(
            !listed_namespace_ids(&router).await.contains(&namespace_id),
            "case {case:?}: a deleted namespace is listed no more"
        );

        // The name is free again, so the identical create is a create and not a conflict.
        let (_, recreated) = created_namespace(&router, create, json!({ "Name": name })).await;
        assert_ne!(
            recreated, namespace_id,
            "case {case:?}: re-creating the freed name stores a new namespace"
        );
    }
}

/// Nobody has to poll: the removal is owed at the instant the delete's operation is due,
/// so a namespace deleted and left alone is gone on its own.
#[tokio::test]
async fn a_delete_is_applied_without_anybody_polling_its_operation() {
    let (router, _engine) = emulator_with_engine();
    let (_, namespace_id) = created_namespace(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "async-del-unpolled" }),
    )
    .await;
    ok(
        &router,
        "DeleteNamespace",
        json!({ "Id": namespace_id.clone() }),
    )
    .await;
    deleted_namespace(&router, "a delete nobody polled", &namespace_id).await;
}

/// One emulator with this service, Route 53 and Amazon EC2 on the one shared engine — the
/// three the assembled emulator mounts for a private DNS namespace, whose hosted zone is a
/// zone of that Route 53 and whose `Vpc` is a VPC of that EC2.
///
/// `pub(crate)` because the sibling suite that asserts the failure REASON of an absent VPC
/// drives the same three services rather than carrying a second copy of this mounting.
pub(crate) fn emulator_with_ec2() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator_with_engine();
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// A private namespace whose `Vpc` no VPC answers to cannot be created: the synchronous
/// call still answers an `OperationId`, and that operation lands `FAIL` with the documented
/// `CANNOT_CREATE_HOSTED_ZONE` code, leaving no namespace behind. The same create against a
/// VPC the account holds succeeds.
#[tokio::test]
async fn a_private_namespace_fails_its_operation_when_its_vpc_does_not_exist() {
    let (router, engine) = emulator_with_ec2();

    // The account's own VPC in the Region these requests are signed for, read from the EC2
    // store the resolution asks — never an id this test invented.
    let held_vpc = {
        let ec2 = engine
            .state::<service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State>();
        let held = ec2
            .lock()
            .ensure_default_network(REGION)
            .expect("the account holds a default VPC in this Region");
        held
    };

    // Rows differ along the `Vpc` the create names: one the account holds, and ids of the
    // documented form that name no VPC at all.
    let cases: Vec<(&str, String, bool)> = vec![
        ("a VPC the account holds", held_vpc.clone(), true),
        (
            "a VPC id of the documented form that names nothing",
            "vpc-0123456789abcdef0".to_string(),
            false,
        ),
        (
            "another id that names nothing",
            "vpc-00000000000000000".to_string(),
            false,
        ),
    ];

    for (index, (case, vpc, exists)) in cases.into_iter().enumerate() {
        let name = format!("async-vpc-{index}.example");
        let created = ok(
            &router,
            "CreatePrivateDnsNamespace",
            json!({ "Name": name, "Vpc": vpc }),
        )
        .await;
        let operation_id = created["OperationId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the synchronous call answers an OperationId, got {created}")
            })
            .to_string();
        let namespace_id = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": operation_id.clone() }),
        )
        .await["Operation"]["Targets"]["NAMESPACE"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the operation targets its namespace"))
            .to_string();

        set_operation_due(&engine, &operation_id, true);
        let settled = settled_operation(&router, &operation_id).await;
        if exists {
            assert_eq!(
                settled["Operation"]["Status"],
                json!("SUCCESS"),
                "case {case:?}: the association has a VPC to make, got {settled}"
            );
            assert!(
                settled["Operation"]["ErrorCode"].is_null()
                    && settled["Operation"]["ErrorMessage"].is_null(),
                "case {case:?}: a successful operation carries no error, got {settled}"
            );
            let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
            assert_eq!(
                read["Namespace"]["Name"],
                json!(name),
                "case {case:?}: the created namespace reads back, got {read}"
            );
        } else {
            assert_eq!(
                (
                    &settled["Operation"]["Status"],
                    &settled["Operation"]["ErrorCode"]
                ),
                (&json!("FAIL"), &json!("CANNOT_CREATE_HOSTED_ZONE")),
                "case {case:?}: the operation must fail with the documented code, got {settled}"
            );
            // The reason is the one the association of the namespace's hosted zone with
            // that VPC is refused with, which names the VPC that could not be resolved and
            // the Region it was asked in. Its wording is pinned by the sibling suite that
            // owns the rule.
            let message = settled["Operation"]["ErrorMessage"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                message.contains(&vpc) && message.contains(REGION),
                "case {case:?}: the failure names the VPC and the Region it was asked in, got \
                 {message:?}"
            );
            let (status, code, _) =
                refusal(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NamespaceNotFound"),
                "case {case:?}: a create that failed leaves no namespace behind"
            );
            // The name it staged is free again, so it can be created once the VPC exists.
            let retried = ok(
                &router,
                "CreatePrivateDnsNamespace",
                json!({ "Name": name, "Vpc": held_vpc }),
            )
            .await;
            assert!(
                retried["OperationId"].is_string(),
                "case {case:?}: the name of a rolled-back create is free, got {retried}"
            );
        }
    }
}
