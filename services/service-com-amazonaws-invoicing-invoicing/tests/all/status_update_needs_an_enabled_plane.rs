// SPDX-License-Identifier: CC0-1.0

//! The verification status a plane of a procurement portal preference can be MOVED to,
//! driven end to end over this service's wire protocol: every write goes through
//! `CreateProcurementPortalPreference` / `PutProcurementPortalPreference` /
//! `UpdateProcurementPortalPreferenceStatus` and is read back through
//! `GetProcurementPortalPreference` and `ListProcurementPortalPreferences`.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body.
//! Nothing reaches into the state struct, so what these tests see is exactly what an SDK
//! client sees.
//!
//! The rule under test, spanned by a table rather than one request: a plane's status is the
//! state of the portal connection its configuration gives it, so only a plane the stored
//! configuration ENABLES has a status a status update can move — stating one for a disabled
//! plane is refused with the modelled `ValidationException` and writes nothing, and a
//! status an enabled plane was moved to survives a configuration write that restates the
//! configuration it was written under.
//!
//! * every status the modelled enum admits, for each of the two planes, asked for while
//!   that plane is disabled — with and without a status reason, and with and without the
//!   other plane enabled and stated in the same request, so the refusal is asserted to
//!   write neither plane;
//! * the statuses a CALLER may carry a plane to, asked for once a configuration write has
//!   enabled the plane and its validation has been completed, read back and then re-read
//!   after a configuration write that changes nothing about it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    emulator_with_engine, validate_preference, PORTAL_INSTANCE_ENDPOINT, PORTAL_NAMES,
};

/// Every value `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const STATUS_VALUES: &[&str] = &[
    "PENDING_VERIFICATION",
    "VALIDATED",
    "TEST_INITIALIZED",
    "TEST_INITIALIZATION_FAILED",
    "TEST_FAILED",
    "ACTIVE",
    "SUSPENDED",
];

/// The statuses a CALLER may ask an enabled plane to take once its validation is complete,
/// in the order the lifecycle reaches them — two of the values above; the rest are the
/// ones the service assigns itself.
const CALLER_REQUESTED_STATUSES: &[&str] = &["TEST_INITIALIZED", "ACTIVE"];

/// The status an enabled plane carries until something moves it, one of the values above.
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The two planes of a preference, each as the members the model spells for it: a label,
/// the configuration member that enables the plane, its status and its status reason.
const PLANES: &[(&str, &str, &str, &str)] = &[
    (
        "e-invoice delivery",
        "EinvoiceDeliveryEnabled",
        "EinvoiceDeliveryPreferenceStatus",
        "EinvoiceDeliveryPreferenceStatusReason",
    ),
    (
        "purchase order retrieval",
        "PurchaseOrderRetrievalEnabled",
        "PurchaseOrderRetrievalPreferenceStatus",
        "PurchaseOrderRetrievalPreferenceStatusReason",
    ),
];

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without the
/// namespace a backend stamps in front of it.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The explanatory text of a failure body, under either case of the member's wire key.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `ProcurementPortalPreferenceArn` of a response that carries one.
fn arn_of(body: &Value) -> String {
    body.get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {body}"))
        .to_string()
}

/// One contact, the single element `Contacts` (`@length(min: 1, max: 1)`) admits.
fn contact(name: &str) -> Value {
    json!([{ "Name": name, "Email": format!("{}@example.com", name.to_lowercase().replace(' ', ".")) }])
}

/// A create request for one identity, stating the two enable flags under test — and the
/// endpoint the test e-invoice of a portal validation is delivered to, so the send that
/// issues a code can confirm that delivery.
fn create_request(
    portal: &str,
    buyer: &str,
    supplier: &str,
    einvoice: bool,
    purchase_order: bool,
) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": buyer,
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": supplier,
        "EinvoiceDeliveryEnabled": einvoice,
        "PurchaseOrderRetrievalEnabled": purchase_order,
        "ProcurementPortalInstanceEndpoint": PORTAL_INSTANCE_ENDPOINT,
        "Contacts": contact("Plane Contact"),
    })
}

/// A `PutProcurementPortalPreference` request stating the two enable flags under test.
fn put_request(arn: &str, einvoice: bool, purchase_order: bool) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": contact("Plane Contact"),
        "EinvoiceDeliveryEnabled": einvoice,
        "PurchaseOrderRetrievalEnabled": purchase_order,
    })
}

/// The preference `arn` names, as `GetProcurementPortalPreference` reports it.
async fn get_preference(router: &MultiServiceRouter, arn: &str) -> Value {
    let body = ok(
        router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    body.get("ProcurementPortalPreference")
        .cloned()
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {body}"))
}

/// The summary `ListProcurementPortalPreferences` reports for `arn`.
async fn summary_of(router: &MultiServiceRouter, arn: &str) -> Value {
    let body = ok(router, "ListProcurementPortalPreferences", &json!({})).await;
    body.get("ProcurementPortalPreferences")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ProcurementPortalPreferences in {body}"))
        .iter()
        .find(|summary| {
            summary
                .get("ProcurementPortalPreferenceArn")
                .and_then(Value::as_str)
                == Some(arn)
        })
        .cloned()
        .unwrap_or_else(|| panic!("expected a summary for {arn} in {body}"))
}

/// A member of a reported preference, as JSON (`Value::Null` when absent).
fn member(preference: &Value, name: &str) -> Value {
    preference.get(name).cloned().unwrap_or(Value::Null)
}

/// `Version` of a reported preference.
fn version(preference: &Value) -> i64 {
    preference
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a Version in {preference}"))
}

/// RULE: the status of an optional plane of a resource is the state of the connection that
/// plane's configuration gives it, so only a plane the stored configuration ENABLES has a
/// status to move — a request asking to move the status of a plane the configuration
/// leaves disabled is refused with the modelled validation failure and writes nothing at
/// all.
///
/// The rows are each plane crossed with every status the modelled enum admits, varying
/// along whether a status reason is stated and whether the OTHER plane is enabled and
/// stated in the same request, so a refusal is asserted to leave both planes — and the
/// resource's `Version` and `LastUpdateDate` — exactly as they were.
#[tokio::test]
async fn a_status_stated_for_a_disabled_plane_is_refused_and_writes_nothing() {
    let router = emulator();

    for (index, (plane, status)) in (0..PLANES.len())
        .flat_map(|plane| STATUS_VALUES.iter().map(move |status| (plane, *status)))
        .enumerate()
    {
        let (label, enabled_member, status_member, reason_member) = PLANES[plane];
        let (other_label, _, other_status_member, _) = PLANES[1 - plane];
        let other_enabled = index % 2 == 0;
        let states_reason = index % 3 == 0;
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let case = format!(
            "{label} disabled on {portal}, asked for {status} (reason stated: \
             {states_reason}, {other_label} enabled and stated: {other_enabled})"
        );

        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_request(
                    portal,
                    &format!("BUYER-DISABLED-{index}"),
                    &format!("SUPPLIER-DISABLED-{index}"),
                    plane != 0 && other_enabled,
                    plane != 1 && other_enabled,
                ),
            )
            .await,
        );

        let before = get_preference(&router, &arn).await;
        assert_eq!(
            member(&before, status_member),
            Value::Null,
            "case {case:?}: a disabled plane reports no {status_member}, got {before}"
        );

        let mut request = json!({"ProcurementPortalPreferenceArn": arn});
        request[status_member] = json!(status);
        if states_reason {
            request[reason_member] = json!(format!("{label} was asked to move to {status}"));
        }
        if other_enabled {
            request[other_status_member] = json!(STATUS_VALUES[(index + 1) % STATUS_VALUES.len()]);
        }

        let (http_status, body) =
            call(&router, "UpdateProcurementPortalPreferenceStatus", &request).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected the modelled validation failure's HTTP 400, got \
             {http_status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("{status_member} can only be updated when {enabled_member} is true"),
            "case {case:?}: the refusal names the status member and the flag that leaves \
             its plane disabled, got {body}"
        );

        let after = get_preference(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused status update writes nothing, got {after} where the \
             preference was {before}"
        );
        let summary = summary_of(&router, &arn).await;
        assert_eq!(
            member(&summary, status_member),
            Value::Null,
            "case {case:?}: the listing reports no {status_member} for a disabled plane, \
             got {summary}"
        );
    }
}

/// RULE: enabling a plane is what gives it a status to move, and the status a status update
/// moved an ENABLED plane to is state a later reader can rely on — a configuration write
/// that leaves that plane's configuration as it was reports the same status and the same
/// reason.
///
/// The rows are each plane crossed with the statuses a CALLER may carry it to: the plane
/// starts disabled (so the same request is refused), is enabled by a configuration write
/// (so it reports the unverified starting status), is carried off that status by a
/// completed validation, is then moved by the status updates the lifecycle provides, and is
/// read again after a configuration write that changes nothing about it.
#[tokio::test]
async fn enabling_a_plane_gives_it_a_status_the_update_moves_and_a_later_write_keeps() {
    let (router, engine) = emulator_with_engine();

    for (index, (plane, status)) in (0..PLANES.len())
        .flat_map(|plane| {
            CALLER_REQUESTED_STATUSES
                .iter()
                .map(move |status| (plane, *status))
        })
        .enumerate()
    {
        let (label, enabled_member, status_member, reason_member) = PLANES[plane];
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let case = format!("{label} enabled on {portal}, then moved to {status}");
        let reason = format!("{label} moved to {status}");

        // The plane starts DISABLED, so there is no status for the update to move.
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_request(
                    portal,
                    &format!("BUYER-ENABLED-{index}"),
                    &format!("SUPPLIER-ENABLED-{index}"),
                    plane != 0,
                    plane != 1,
                ),
            )
            .await,
        );
        let mut request = json!({"ProcurementPortalPreferenceArn": arn});
        request[status_member] = json!(status);
        let (refused, body) =
            call(&router, "UpdateProcurementPortalPreferenceStatus", &request).await;
        assert_eq!(
            refused, 400,
            "case {case:?}: a disabled plane has no status to move, got {refused} {body}"
        );

        // The configuration write that enables the plane gives it a connection, and so the
        // status of a connection nothing has verified yet.
        ok(
            &router,
            "PutProcurementPortalPreference",
            &put_request(&arn, true, true),
        )
        .await;
        let enabled = get_preference(&router, &arn).await;
        assert_eq!(
            member(&enabled, enabled_member),
            json!(true),
            "case {case:?}: the configuration write states {enabled_member} true, got {enabled}"
        );
        assert_eq!(
            member(&enabled, status_member),
            json!(PENDING_VERIFICATION),
            "case {case:?}: a plane nothing has verified reports {PENDING_VERIFICATION}, \
             got {enabled}"
        );

        // A completed validation is what carries a plane off the unverified status, and
        // only from there can a caller ask for the moves the lifecycle provides.
        validate_preference(&router, &engine, &arn).await;
        let walk = CALLER_REQUESTED_STATUSES
            .iter()
            .position(|reachable| *reachable == status)
            .map(|target| &CALLER_REQUESTED_STATUSES[..target + 1])
            .unwrap_or_else(|| panic!("case {case:?}: the row's status has to be reachable"));
        let version_before = version(&get_preference(&router, &arn).await);

        for asked_for in walk {
            request[status_member] = json!(asked_for);
            if *asked_for == status {
                request[reason_member] = json!(reason);
            }
            ok(&router, "UpdateProcurementPortalPreferenceStatus", &request).await;
        }
        let moved = get_preference(&router, &arn).await;
        assert_eq!(
            member(&moved, status_member),
            json!(status),
            "case {case:?}: an enabled plane takes the status the update states, got {moved}"
        );
        assert_eq!(
            member(&moved, reason_member),
            json!(reason),
            "case {case:?}: the plane reports the reason stated with that status, got {moved}"
        );
        let expected_version = version_before + walk.len() as i64;
        assert_eq!(
            version(&moved),
            expected_version,
            "case {case:?}: each applied write advances Version by one, so it is \
             {expected_version}, got {}",
            version(&moved)
        );

        // A configuration write that restates the configuration the status was written
        // under is not what moves a status, so both survive it — in the read and in the
        // listing's summary alike.
        ok(
            &router,
            "PutProcurementPortalPreference",
            &put_request(&arn, true, true),
        )
        .await;
        let kept = get_preference(&router, &arn).await;
        assert_eq!(
            member(&kept, status_member),
            json!(status),
            "case {case:?}: a later configuration write keeps the status, got {kept}"
        );
        assert_eq!(
            member(&kept, reason_member),
            json!(reason),
            "case {case:?}: a later configuration write keeps the reason, got {kept}"
        );
        let summary = summary_of(&router, &arn).await;
        assert_eq!(
            member(&summary, status_member),
            json!(status),
            "case {case:?}: the listing reports the same status the read does, got {summary}"
        );
    }
}
