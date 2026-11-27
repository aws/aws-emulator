// SPDX-License-Identifier: CC0-1.0

//! The VERIFICATION STATE each plane of a procurement portal preference reports, driven
//! end to end over this service's wire protocol: every configuration write goes through
//! `CreateProcurementPortalPreference` / `PutProcurementPortalPreference` and is read back
//! through `GetProcurementPortalPreference` and `ListProcurementPortalPreferences`.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body.
//! Nothing reaches into the state struct, so what these tests see is exactly what an SDK
//! client sees.
//!
//! The rule under test, spanned by a table rather than one recorded request: a plane's
//! status and status reason describe the portal connection of an ENABLED plane, so a
//! configuration write reports them for a plane it enables (at `PENDING_VERIFICATION`
//! while nothing has verified the connection) and reports neither for a plane it leaves
//! disabled — whichever write and whatever order of writes the configuration was reached
//! by.
//!
//! * every combination of the two enable flags, reached by a create and then by an update
//!   from every other combination, so both directions of each plane are covered;
//! * a status (and reason) a caller moved a plane to survives an update that leaves
//!   the plane enabled, goes when an update disables that plane, and does not come back
//!   when a later update enables it again;
//! * two preferences holding one configuration report the same plane state however they
//!   reached it, in the `Get` and in the listing's summary alike.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    emulator_with_engine, validate_preference, PORTAL_INSTANCE_ENDPOINT, PORTAL_NAMES,
};

/// The status an enabled plane carries until something moves it: one of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The statuses a CALLER can carry a plane to, in the order the lifecycle reaches them:
/// the validated status a completed validation leaves, then the connection test a caller
/// asks for and the activation that follows it. The other values
/// `ProcurementPortalPreferenceStatus` admits are the ones the service assigns itself, so
/// no request can move a plane to them.
const MOVED_STATUSES: &[&str] = &["VALIDATED", "TEST_INITIALIZED", "ACTIVE"];

/// The two planes of a preference, each as the members the model spells for it: a label,
/// the flag that enables the plane, its status and its status reason.
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

/// Every configuration the two enable flags can state.
const FLAG_COMBINATIONS: &[(bool, bool)] = &[
    (true, true),
    (true, false),
    (false, true),
    (false, false),
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
        "Contacts": contact("Initial Contact"),
    })
}

/// A `PutProcurementPortalPreference` request stating the two enable flags under test.
fn put_request(arn: &str, einvoice: bool, purchase_order: bool) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": contact("Updated Contact"),
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

/// Assert the rule for one plane of one reported preference: a status exactly when the
/// plane is enabled, `expected` while it is, and no reason while it is not.
fn assert_plane_state(case: &str, reported: &Value, plane: usize, expected_status: Option<&str>) {
    let (label, flag, status, reason) = PLANES[plane];
    let enabled = reported.get(flag).and_then(Value::as_bool);
    let actual_status = reported.get(status).and_then(Value::as_str);
    let actual_reason = reported.get(reason).and_then(Value::as_str);
    assert_eq!(
        actual_status, expected_status,
        "case {case:?}: the {label} plane reports {flag} {enabled:?}, so expected \
         {status} {expected_status:?}, got {actual_status:?}"
    );
    if expected_status.is_none() {
        assert_eq!(
            actual_reason, None,
            "case {case:?}: a plane with no {status} reports no {reason}, got {actual_reason:?}"
        );
    }
}

/// RULE: the status and status reason of an optional plane of a resource describe that
/// plane while it is ENABLED, so a write of the resource's configuration reports them for
/// a plane the configuration enables — at the unverified starting status while nothing has
/// moved it — and reports neither for a plane the configuration leaves disabled, whichever
/// write stated that configuration.
///
/// The rows are every pair of (configuration the create states, configuration the update
/// states) over the four combinations of the two enable flags, so each plane is asserted
/// being enabled, left enabled, disabled and left disabled.
#[tokio::test]
async fn a_plane_reports_a_verification_status_only_while_it_is_enabled() {
    let router = emulator();

    for (index, ((created_einvoice, created_purchase_order), (put_einvoice, put_purchase_order))) in
        FLAG_COMBINATIONS
            .iter()
            .flat_map(|created| FLAG_COMBINATIONS.iter().map(move |put| (*created, *put)))
            .enumerate()
    {
        // One identity per row, so a conflicting create never stands in for an update.
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let created = ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_request(
                portal,
                &format!("BUYER-PLANE-{index}"),
                &format!("SUPPLIER-PLANE-{index}"),
                created_einvoice,
                created_purchase_order,
            ),
        )
        .await;
        let arn = arn_of(&created);

        let case = format!(
            "created ({created_einvoice}, {created_purchase_order}) on {portal}, then put \
             ({put_einvoice}, {put_purchase_order})"
        );
        let pending = |enabled: bool| enabled.then_some(PENDING_VERIFICATION);

        let after_create = get_preference(&router, &arn).await;
        assert_plane_state(&case, &after_create, 0, pending(created_einvoice));
        assert_plane_state(&case, &after_create, 1, pending(created_purchase_order));

        ok(
            &router,
            "PutProcurementPortalPreference",
            &put_request(&arn, put_einvoice, put_purchase_order),
        )
        .await;

        // Nothing has verified either plane, so an enabled plane is at the starting
        // status whether this update enabled it or left it enabled.
        let after_put = get_preference(&router, &arn).await;
        assert_plane_state(&case, &after_put, 0, pending(put_einvoice));
        assert_plane_state(&case, &after_put, 1, pending(put_purchase_order));

        // The listing reports the same plane state the read does.
        let summary = summary_of(&router, &arn).await;
        assert_plane_state(&case, &summary, 0, pending(put_einvoice));
        assert_plane_state(&case, &summary, 1, pending(put_purchase_order));
    }
}

/// RULE: the status a plane was moved to belongs to that plane while it stays ENABLED — a
/// configuration write that leaves the plane enabled keeps it and its reason, a write that
/// disables the plane reports neither, and a later write that enables the plane again
/// reports the unverified starting status rather than the status the plane last held.
///
/// The rows are each plane crossed with every status a CALLER can carry a plane to
/// ([`MOVED_STATUSES`]), so nothing here is keyed to one status value.
#[tokio::test]
async fn disabling_a_plane_drops_the_status_it_was_moved_to() {
    let (router, engine) = emulator_with_engine();

    for (index, (plane, moved_to)) in (0..PLANES.len())
        .flat_map(|plane| MOVED_STATUSES.iter().map(move |status| (plane, *status)))
        .enumerate()
    {
        let (label, flag, status_member, reason_member) = PLANES[plane];
        let (_, other_flag, other_status_member, _) = PLANES[1 - plane];
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let created = ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_request(
                portal,
                &format!("BUYER-MOVED-{index}"),
                &format!("SUPPLIER-MOVED-{index}"),
                true,
                true,
            ),
        )
        .await;
        let arn = arn_of(&created);
        let case = format!("{label} moved to {moved_to} on {portal}");
        let reason = format!("{label} was moved for this case");

        // A completed validation carries both planes off the unverified status; the moves
        // past it are the ones a caller asks for, one update each, with the reason stated
        // on the update that sets the status under test.
        validate_preference(&router, &engine, &arn).await;
        let target = MOVED_STATUSES
            .iter()
            .position(|reachable| *reachable == moved_to)
            .unwrap_or_else(|| panic!("case {case:?}: the row's status has to be reachable"));
        for asked_for in &MOVED_STATUSES[..target + 1] {
            let mut move_status = json!({"ProcurementPortalPreferenceArn": arn});
            move_status[status_member] = json!(asked_for);
            if *asked_for == moved_to {
                move_status[reason_member] = json!(reason);
            }
            ok(
                &router,
                "UpdateProcurementPortalPreferenceStatus",
                &move_status,
            )
            .await;
        }

        // An update that leaves the plane enabled is not what moves a status, so the
        // status and the reason the status update left are what the plane still reports.
        let mut both_on = put_request(&arn, true, true);
        both_on["Contacts"] = contact("Still Enabled");
        ok(&router, "PutProcurementPortalPreference", &both_on).await;
        let kept = get_preference(&router, &arn).await;
        assert_plane_state(&case, &kept, plane, Some(moved_to));
        assert_eq!(
            kept.get(reason_member).and_then(Value::as_str),
            Some(reason.as_str()),
            "case {case:?}: an enabled plane keeps the reason stated with its status, got {:?}",
            kept.get(reason_member)
        );

        // Disabling the plane leaves it with no status and no reason, and says nothing
        // about the other plane, which this request leaves enabled.
        let mut disable = put_request(&arn, plane != 0, plane != 1);
        disable["Contacts"] = contact("Now Disabled");
        ok(&router, "PutProcurementPortalPreference", &disable).await;
        let disabled = get_preference(&router, &arn).await;
        assert_eq!(
            disabled.get(flag).and_then(Value::as_bool),
            Some(false),
            "case {case:?}: the update states {flag} false, got {:?}",
            disabled.get(flag)
        );
        assert_plane_state(&case, &disabled, plane, None);
        // The other plane stayed enabled throughout, so it still reports the status the
        // completed validation left it at.
        assert_eq!(
            disabled.get(other_status_member).and_then(Value::as_str),
            Some(MOVED_STATUSES[0]),
            "case {case:?}: the plane this write left enabled keeps its status, got {:?}",
            disabled.get(other_status_member)
        );
        let listed = summary_of(&router, &arn).await;
        assert_plane_state(&case, &listed, plane, None);

        // Enabling it again is a connection nothing has verified, so it starts at the
        // unverified status rather than returning to the one it last held.
        let mut enable_again = put_request(&arn, true, true);
        enable_again["Contacts"] = contact("Enabled Again");
        ok(&router, "PutProcurementPortalPreference", &enable_again).await;
        let re_enabled = get_preference(&router, &arn).await;
        assert_plane_state(&case, &re_enabled, plane, Some(PENDING_VERIFICATION));
        assert_eq!(
            re_enabled.get(other_flag).and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the other plane stays enabled, got {:?}",
            re_enabled.get(other_flag)
        );
    }
}

/// RULE: a resource's reported state is a function of its configuration, not of the
/// sequence of writes that produced it — two resources holding one configuration report the
/// same members.
///
/// The rows are every combination of the two enable flags: one preference reaches it
/// through a create alone, the other through a create of the opposite configuration
/// followed by an update, and the status members of the two are compared in the read and in
/// the listing's summary.
#[tokio::test]
async fn one_configuration_reports_one_plane_state_whatever_history_reached_it() {
    let router = emulator();

    for (index, (einvoice, purchase_order)) in FLAG_COMBINATIONS.iter().enumerate() {
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let case = format!("({einvoice}, {purchase_order}) on {portal}");

        // Reached directly by a create.
        let created = ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_request(
                portal,
                &format!("BUYER-DIRECT-{index}"),
                &format!("SUPPLIER-DIRECT-{index}"),
                *einvoice,
                *purchase_order,
            ),
        )
        .await;
        let direct = arn_of(&created);

        // Reached by a create of the opposite configuration and an update onto this one.
        let created = ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_request(
                portal,
                &format!("BUYER-UPDATED-{index}"),
                &format!("SUPPLIER-UPDATED-{index}"),
                !*einvoice,
                !*purchase_order,
            ),
        )
        .await;
        let updated = arn_of(&created);
        ok(
            &router,
            "PutProcurementPortalPreference",
            &put_request(&updated, *einvoice, *purchase_order),
        )
        .await;

        let direct_read = get_preference(&router, &direct).await;
        let updated_read = get_preference(&router, &updated).await;
        let direct_summary = summary_of(&router, &direct).await;
        let updated_summary = summary_of(&router, &updated).await;
        for (label, _, status_member, reason_member) in PLANES {
            for member in [status_member, reason_member] {
                assert_eq!(
                    direct_read.get(member),
                    updated_read.get(member),
                    "case {case:?}: the {label} plane reports one {member} for one \
                     configuration, got {:?} after a create and {:?} after an update",
                    direct_read.get(member),
                    updated_read.get(member)
                );
                assert_eq!(
                    direct_summary.get(member),
                    updated_summary.get(member),
                    "case {case:?}: the listing reports one {member} for one configuration, \
                     got {:?} after a create and {:?} after an update",
                    direct_summary.get(member),
                    updated_summary.get(member)
                );
            }
        }
    }
}
