// SPDX-License-Identifier: CC0-1.0

//! The two refusals of the procurement-portal-preference plane this suite pins, driven end
//! to end over this service's wire protocol with the shared harness of
//! `refused_procurement_portal_writes` rather than a copy of it.
//!
//! 1. A create whose PORTAL IDENTIFIERS already identify a stored preference is refused
//!    with the modelled `ConflictException` (HTTP 409), whose message is FIXED text for
//!    that error class: it interpolates nothing from the request or from state, because
//!    the preference it conflicts with travels in the shape's own `resourceId` /
//!    `resourceType` members instead.
//! 2. `SendProcurementPortalValidation` reports the DELIVERY of the test e-invoice that
//!    carries the code it issued, which this emulator makes to no procurement portal: the
//!    call is refused with the modelled `ValidationException` (HTTP 400) naming the
//!    delivery target it could not reach — the endpoint the preference configures for that
//!    document, with the portal's cXML invoice path under it — and the code it issued stays
//!    outstanding, so the flow the refusal points at still completes. The refused delivery
//!    attempt is RECORDED: it advances the preference's `Version` by exactly one and moves
//!    no plane's status.
//!
//! Each rule is spanned by a table: the identifiers a conflicting create can repeat (every
//! portal of the enum and the identifier shapes the pair admits), and the endpoints a
//! preference can configure the delivery target from (paths, a trailing slash, a port, a
//! query, the test environment's own endpoint under the connection-testing method that
//! replays into it, and no endpoint at all).

use serde_json::{json, Value};

use crate::refused_procurement_portal_writes::{
    arn_of, call, emulator_with_engine, error_code, error_text, get_preference, minimal_create,
    invalid_code_sentence, ok, outstanding_code, portal_at, shifted, version,
    DELIVERY_NOT_CONFIRMED, DELIVERY_PATH, PORTAL_NAMES,
};

/// The status an enabled plane holds while nothing has verified its portal connection: one
/// of the values `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The sentence a duplicate-portal-identifiers create is refused with: recorded behaviour,
/// and FIXED text for this error class.
const DUPLICATE_PORTAL_IDENTIFIERS: &str =
    "There is a procurement portal preference already existed with same portal identifiers";

/// The status member of each plane of a preference (`smithy_query(query="shape",
/// shape="ProcurementPortalPreference")`).
const PLANE_STATUS_MEMBERS: &[&str] = &[
    "EinvoiceDeliveryPreferenceStatus",
    "PurchaseOrderRetrievalPreferenceStatus",
];

/// One row of the conflict table: the identity a create states, which a second create then
/// repeats.
struct IdentityRow {
    label: &'static str,
    portal: &'static str,
    buyer_domain: &'static str,
    buyer: &'static str,
    supplier_domain: &'static str,
    supplier: &'static str,
}

/// The rows: every portal the enum admits, with identifier and domain shapes differing
/// along what the identity is made of, so the conflict is not keyed to one portal or one
/// pair of identifiers.
fn identity_rows() -> Vec<IdentityRow> {
    vec![
        IdentityRow {
            label: "network ids on the first portal",
            portal: portal_at(0),
            buyer_domain: "NetworkID",
            buyer: "AN01234567890",
            supplier_domain: "NetworkID",
            supplier: "AN01010639538",
        },
        IdentityRow {
            label: "a long network identifier pair on the second portal",
            portal: portal_at(1),
            buyer_domain: "NetworkID",
            buyer: "AN01234567890123456789",
            supplier_domain: "NetworkID",
            supplier: "AN09999999999",
        },
        IdentityRow {
            label: "one-character identifiers",
            portal: portal_at(2),
            buyer_domain: "NetworkID",
            buyer: "A",
            supplier_domain: "NetworkID",
            supplier: "B",
        },
        IdentityRow {
            label: "the same identifier on both sides",
            portal: portal_at(3),
            buyer_domain: "NetworkID",
            buyer: "AN05555555555",
            supplier_domain: "NetworkID",
            supplier: "AN05555555555",
        },
    ]
}

/// The create request one identity row states.
fn create_for(row: &IdentityRow, token: &str) -> Value {
    let mut request = minimal_create(row.portal, row.buyer, row.supplier);
    request["BuyerDomain"] = json!(row.buyer_domain);
    request["SupplierDomain"] = json!(row.supplier_domain);
    request["ClientToken"] = json!(token);
    request
}

/// RULE: a preference is identified by the portal and the buyer/supplier pair it routes
/// between, so a second create of that identity is refused with the modelled
/// `ConflictException` — HTTP 409, the fixed sentence of that error class, and the ARN of
/// the preference it conflicts with in the shape's own member rather than in the prose.
#[tokio::test]
async fn a_create_repeating_the_portal_identifiers_is_refused_with_the_conflict_sentence() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in identity_rows().iter().enumerate() {
        let created = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_for(row, &format!("conflict-first-{index}")),
            )
            .await,
        );

        // A second create of the SAME identity, under a fresh token so it is a new request
        // rather than an idempotent replay.
        let (status, body) = call(
            &router,
            "CreateProcurementPortalPreference",
            &create_for(row, &format!("conflict-again-{index}")),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {}: a second create of one identity is refused, got {status} {body}",
            row.label
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {}: expected the modelled ConflictException, got {body}",
            row.label
        );
        assert_eq!(
            error_text(&body),
            DUPLICATE_PORTAL_IDENTIFIERS,
            "case {}: expected this error class's fixed sentence, got {body}",
            row.label
        );
        // The sentence interpolates nothing: no identifier of the request and no value of
        // the stored preference appears in it.
        for value in [
            row.portal,
            row.buyer,
            row.supplier,
            row.buyer_domain,
            created.as_str(),
        ] {
            assert!(
                !error_text(&body).contains(value),
                "case {}: the sentence is fixed text, so it must not carry {value:?}, got \
                 {body}",
                row.label
            );
        }
        // What the refusal does state mechanically is the resource it conflicts with.
        assert_eq!(
            body.get("resourceId").and_then(Value::as_str),
            Some(created.as_str()),
            "case {}: the conflict names the preference it collided with, got {body}",
            row.label
        );

        // A create differing in ONE part of the identity is a different preference, so it
        // is not refused: the conflict is about the identity and nothing else.
        let mut elsewhere = create_for(row, &format!("conflict-other-{index}"));
        elsewhere["SupplierIdentifier"] = json!(format!("{}-OTHER", row.supplier));
        let other = arn_of(
            &ok(&router, "CreateProcurementPortalPreference", &elsewhere).await,
        );
        assert_ne!(
            other, created,
            "case {}: a create of another identity mints its own preference",
            row.label
        );
    }
}

/// One row of the delivery table: the endpoint the preference configures for the test
/// e-invoice of its connection test, and the delivery target the refusal must name.
struct DeliveryRow {
    label: &'static str,
    /// The `ProcurementPortalInstanceEndpoint` the create states, if any.
    endpoint: Option<&'static str>,
    /// The `TestEnvPreference$ProcurementPortalInstanceEndpoint` the create states, if any,
    /// stated together with the connection-testing method that replays into the test
    /// environment — which is what makes that endpoint the one the document goes to.
    test_env_endpoint: Option<&'static str>,
    /// The endpoint the delivery target is built from, or `None` when the configuration
    /// names none at all.
    target_endpoint: Option<&'static str>,
}

/// The rows: the endpoint shapes `ProcurementPortalInstanceEndpoint`'s own `^\S+$` admits
/// (a bare host, a path, a trailing slash, a port, a query), the test environment's
/// endpoint under the method that replays into it, and the configuration that names no
/// endpoint at all.
fn delivery_rows() -> Vec<DeliveryRow> {
    vec![
        DeliveryRow {
            label: "a portal endpoint with a path",
            endpoint: Some("https://sim2real-behav.example.com/procurement-portal"),
            test_env_endpoint: None,
            target_endpoint: Some("https://sim2real-behav.example.com/procurement-portal"),
        },
        DeliveryRow {
            label: "a bare host",
            endpoint: Some("https://portal.example.com"),
            test_env_endpoint: None,
            target_endpoint: Some("https://portal.example.com"),
        },
        DeliveryRow {
            label: "a path with a trailing slash, which names the same target",
            endpoint: Some("https://portal.example.com/invoices/"),
            test_env_endpoint: None,
            target_endpoint: Some("https://portal.example.com/invoices"),
        },
        DeliveryRow {
            label: "a port and a deep path",
            endpoint: Some("https://portal.example.com:8443/a/b/c"),
            test_env_endpoint: None,
            target_endpoint: Some("https://portal.example.com:8443/a/b/c"),
        },
        DeliveryRow {
            label: "a query the endpoint carries",
            endpoint: Some("https://portal.example.com/cxml?tenant=7"),
            test_env_endpoint: None,
            target_endpoint: Some("https://portal.example.com/cxml?tenant=7"),
        },
        DeliveryRow {
            label: "the test environment's endpoint, which the replay test delivers to",
            endpoint: Some("https://portal.example.com/cxml"),
            test_env_endpoint: Some("https://test.portal.example.com/cxml"),
            target_endpoint: Some("https://test.portal.example.com/cxml"),
        },
        DeliveryRow {
            label: "no endpoint at all",
            endpoint: None,
            test_env_endpoint: None,
            target_endpoint: None,
        },
    ]
}

/// The create one delivery row states.
fn create_for_delivery(row: &DeliveryRow, index: usize) -> Value {
    let mut request = minimal_create(
        PORTAL_NAMES[index % PORTAL_NAMES.len()],
        &format!("BUYER-DELIVERY-{index}"),
        &format!("SUPPLIER-DELIVERY-{index}"),
    );
    if let Some(endpoint) = row.endpoint {
        request["ProcurementPortalInstanceEndpoint"] = json!(endpoint);
    }
    if let Some(test_endpoint) = row.test_env_endpoint {
        request["TestEnvPreference"] = json!({
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": "AN0TEST000001",
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": "AN0TEST000002",
            "ProcurementPortalInstanceEndpoint": test_endpoint,
        });
        // The connection test whose document is replayed into the portal's TEST
        // environment, which is what makes that environment's endpoint the delivery
        // target (`smithy_query(query="shape", shape="EinvoiceDeliveryPreference")`).
        request["EinvoiceDeliveryPreference"] = json!({
            "EinvoiceDeliveryDocumentTypes": ["AWS_CLOUD_INVOICE"],
            "Protocol": "CXML",
            "PurchaseOrderDataSources": [{
                "EinvoiceDeliveryDocumentType": "AWS_CLOUD_INVOICE",
                "PurchaseOrderDataSourceType": "PURCHASE_ORDER_NOT_REQUIRED",
            }],
            "ConnectionTestingMethod": "TEST_ENV_REPLAY_TEST",
            "EinvoiceDeliveryActivationDate": 1_767_225_600,
        });
    }
    request
}

/// RULE: the call that starts a portal validation issues the code and then reports the
/// DELIVERY of the test e-invoice carrying it, which this emulator makes to no procurement
/// portal: it is refused with the modelled `ValidationException` naming the delivery
/// target — the endpoint the preference configures for that document with the portal's cXML
/// invoice path under it — and stating that the delivery was not confirmed when the
/// preference configures no such endpoint. The attempt is recorded: the preference's
/// `Version` advances by exactly one, no plane's status moves, and the code stays
/// outstanding.
#[tokio::test]
async fn a_send_names_the_delivery_target_it_could_not_reach() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in delivery_rows().iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_for_delivery(row, index),
            )
            .await,
        );
        let before = get_preference(&router, &arn).await;
        for member in PLANE_STATUS_MEMBERS {
            assert_eq!(
                before.get(*member).and_then(Value::as_str),
                Some(PENDING_VERIFICATION),
                "case {}: an enabled plane awaits verification before the send, got {before}",
                row.label
            );
        }

        let (status, body) = call(
            &router,
            "SendProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {}: the send reports the delivery it could not make, got {status} {body}",
            row.label
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {}: expected the modelled ValidationException, got {body}",
            row.label
        );
        let expected = match row.target_endpoint {
            Some(endpoint) => format!(
                "We were unable to deliver the test e-invoice to the endpoint \
                 {endpoint}{DELIVERY_PATH}. Please confirm the endpoint URL is correct and \
                 reachable, then try again."
            ),
            None => DELIVERY_NOT_CONFIRMED.to_string(),
        };
        assert_eq!(
            error_text(&body),
            expected,
            "case {}: expected the sentence this error class states, got {body}",
            row.label
        );

        // The refused delivery attempt is recorded: one version of the resource, no plane
        // moved, and the code it issued is outstanding.
        let after = get_preference(&router, &arn).await;
        assert_eq!(
            version(&after),
            version(&before) + 1,
            "case {}: the refused attempt is one version of the resource, got {after}",
            row.label
        );
        for member in PLANE_STATUS_MEMBERS {
            assert_eq!(
                after.get(*member).and_then(Value::as_str),
                Some(PENDING_VERIFICATION),
                "case {}: a refused send moves no plane's status, got {after}",
                row.label
            );
        }
        let issued = outstanding_code(&engine, &arn).unwrap_or_else(|| {
            panic!(
                "case {}: the send issues the code the flow runs on",
                row.label
            )
        });

        // A submission of a code the caller did not receive is refused with the attempt it
        // just made against that code, and recorded the same way: one version, no plane
        // moved.
        let (status, body) = call(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": shifted(&issued)}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {}: a code the service did not issue completes nothing, got {status} {body}",
            row.label
        );
        assert_eq!(
            error_text(&body),
            invalid_code_sentence(1),
            "case {}: the refusal reports the attempt it just made, got {body}",
            row.label
        );
        let refused = get_preference(&router, &arn).await;
        assert_eq!(
            version(&refused),
            version(&after) + 1,
            "case {}: the refused submission is one more version, got {refused}",
            row.label
        );
        for member in PLANE_STATUS_MEMBERS {
            assert_eq!(
                refused.get(*member).and_then(Value::as_str),
                Some(PENDING_VERIFICATION),
                "case {}: a refused submission moves no plane's status, got {refused}",
                row.label
            );
        }

        // The remedy the refusal points at works: the code the send issued completes the
        // verification, so the refused delivery left the flow usable.
        let completed = ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": issued}),
        )
        .await;
        assert_eq!(
            arn_of(&completed),
            arn,
            "case {}: the issued code completes the validation, got {completed}",
            row.label
        );
    }
}
