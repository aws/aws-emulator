// SPDX-License-Identifier: CC0-1.0

//! The URL requirement a write of a procurement portal preference applies to the delivery
//! endpoint it stores, driven end to end over this service's wire protocol: `POST /` with
//! `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body, through the same
//! `MultiServiceRouter` the production router uses, so what these tests see is what an SDK
//! client sees.
//!
//! RULE: an input member the model documents as having to be a valid HTTPS URL takes only a
//! URL that names the `https` scheme and a host; any other value is the modelled
//! `ValidationException` (HTTP 400), answered before the write stores anything.
//! `CreateProcurementPortalPreferenceRequest$ProcurementPortalInstanceEndpoint` is "the
//! endpoint URL where e-invoices will be delivered to the procurement portal. Must be a
//! valid HTTPS URL" and the update's own member states the same requirement for the value it
//! writes (`smithy_query(query="shape",
//! shape="CreateProcurementPortalPreferenceRequest,PutProcurementPortalPreferenceRequest")`),
//! while those members' only modelled constraints — `BasicStringWithoutSpace`'s
//! `@length(0..1024)` and `^\S+$` — admit any non-space string, so the boundary validator
//! cannot express the requirement.
//!
//! The table spans what the rule generalises over: the scheme (the required one, the
//! plaintext sibling, another transport, none at all), the spelling's case (a URL's scheme
//! is case-insensitive, RFC 3986 §3.1), the host form (a registered name and an IP
//! literal), what follows the host (nothing, a path, a port, a query) and the forms that
//! name no host (a bare host, a relative path, a scheme-relative reference, a single-slash
//! authority, a string that is no URL). Every row is driven through BOTH writes, each
//! accepted row read back through `GetProcurementPortalPreference` and each refused row
//! checked to have written nothing.
//!
//! The SCOPE is the documentation, not the name: `TestEnvPreferenceInput`'s own
//! `ProcurementPortalInstanceEndpoint` is "the endpoint URL where e-invoices will be
//! delivered in the test environment" (`smithy_query(query="shape",
//! shape="TestEnvPreferenceInput")`), stating no scheme requirement, so a write that
//! configures the test environment on any other scheme is served — pinned by the last test
//! here.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    arn_of, call, emulator_with_engine, error_code, minimal_create, ok,
    portal_without_punch_out_requirement,
};

/// The member both writes state the delivery endpoint on, which a refusal about it names.
const ENDPOINT_MEMBER: &str = "ProcurementPortalInstanceEndpoint";

/// The key every refusal of the procurement-portal-preference plane carries its
/// explanatory text under.
const PORTAL_PLANE_KEY: &str = "Message";

/// A portal the create needs no further configuration for, so each row's write turns only
/// on its endpoint: one of the values `ProcurementPortalName` admits, chosen from the
/// model's own value set.
fn portal() -> &'static str {
    portal_without_punch_out_requirement()
}

/// The explanatory text of a refusal of this plane, if the body carries one.
fn message(body: &Value) -> String {
    body.get(PORTAL_PLANE_KEY)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `PutProcurementPortalPreference` request for `arn`, stating every member the
/// operation marks required and nothing else.
fn put_request(arn: &str) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": [{"Name": "Portal Administrator", "Email": "portal.admin@example.com"}],
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    })
}

/// The preference `arn` names, as `GetProcurementPortalPreference` reports it.
async fn get_preference(router: &MultiServiceRouter, arn: &str) -> Value {
    let read = ok(
        router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    read.get("ProcurementPortalPreference")
        .cloned()
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {read}"))
}

/// One row of the endpoint table: the value a write states for the documented member, and
/// whether it is a URL the requirement admits.
#[derive(Debug)]
struct EndpointRow {
    case: &'static str,
    endpoint: &'static str,
    accepted: bool,
}

/// The endpoint values the rule reads, each paired with the answer the requirement gives
/// it.
fn endpoint_rows() -> Vec<EndpointRow> {
    vec![
        EndpointRow {
            case: "the required scheme, a host and a path",
            endpoint: "https://portal.example.com/cxml",
            accepted: true,
        },
        EndpointRow {
            case: "the required scheme and a host, nothing after it",
            endpoint: "https://portal.example.com",
            accepted: true,
        },
        EndpointRow {
            case: "a port and a query beside the path",
            endpoint: "https://portal.example.com:8443/cxml?v=1",
            accepted: true,
        },
        EndpointRow {
            // A URL's scheme is case-insensitive (RFC 3986 §3.1), so the requirement is
            // about the scheme, not about how it is spelled.
            case: "the required scheme spelled in upper case",
            endpoint: "HTTPS://portal.example.com/cxml",
            accepted: true,
        },
        EndpointRow {
            // An address literal from the documentation range 192.0.2.0/24 (RFC 5737).
            case: "an IP literal for a host",
            endpoint: "https://192.0.2.10/cxml",
            accepted: true,
        },
        EndpointRow {
            case: "the plaintext sibling of the required scheme",
            endpoint: "http://portal.example.com/cxml",
            accepted: false,
        },
        EndpointRow {
            case: "the plaintext scheme with a port",
            endpoint: "http://portal.example.com:8080/cxml",
            accepted: false,
        },
        EndpointRow {
            case: "another transport entirely",
            endpoint: "ftp://portal.example.com/cxml",
            accepted: false,
        },
        EndpointRow {
            case: "a bare host, naming no scheme",
            endpoint: "portal.example.com",
            accepted: false,
        },
        EndpointRow {
            case: "a host and a path, naming no scheme",
            endpoint: "portal.example.com/cxml",
            accepted: false,
        },
        EndpointRow {
            case: "a scheme-relative reference",
            endpoint: "//portal.example.com/cxml",
            accepted: false,
        },
        EndpointRow {
            case: "the required scheme with a single slash, so no host",
            endpoint: "https:/portal.example.com",
            accepted: false,
        },
        EndpointRow {
            case: "a path on its own",
            endpoint: "/cxml",
            accepted: false,
        },
        EndpointRow {
            case: "a string that is no URL at all",
            endpoint: "not-a-url",
            accepted: false,
        },
    ]
}

/// What a refused write must leave behind and say: the modelled 400, a `ValidationException`
/// naming the member and the requirement it words, and nothing of the value it rejected.
fn assert_refusal(case: &str, which: &str, status: u16, body: &Value, endpoint: &str) {
    assert_eq!(
        status, 400,
        "case {case:?} ({which}): expected HTTP 400 for {endpoint:?}, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} ({which}): expected a ValidationException for {endpoint:?}, got {body}"
    );
    let text = message(body);
    assert!(
        text.contains(ENDPOINT_MEMBER),
        "case {case:?} ({which}): the refusal must name the member it is about, got {text:?}"
    );
    assert!(
        text.contains("HTTPS URL"),
        "case {case:?} ({which}): the refusal must state the requirement the member \
         documents, got {text:?}"
    );
}

/// THE RULE through the CREATE: a stated endpoint that is not a valid HTTPS URL is the
/// modelled 400 with nothing created, and one that is is stored and read back verbatim.
#[tokio::test]
async fn a_create_stores_only_a_secure_delivery_endpoint() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in endpoint_rows().iter().enumerate() {
        let case = row.case;
        let buyer = format!("create-buyer-{index}");
        let supplier = format!("create-supplier-{index}");
        let mut request = minimal_create(portal(), &buyer, &supplier);
        request[ENDPOINT_MEMBER] = json!(row.endpoint);
        let (status, body) = call(&router, "CreateProcurementPortalPreference", &request).await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {:?}, got {status} {body}",
                row.endpoint
            );
            let reported = get_preference(&router, &arn_of(&body)).await;
            assert_eq!(
                reported.get(ENDPOINT_MEMBER).and_then(Value::as_str),
                Some(row.endpoint),
                "case {case:?}: the preference must report the endpoint its create stated, \
                 got {reported}"
            );
            continue;
        }

        assert_refusal(case, "the create", status, &body, row.endpoint);

        // A refused create persisted nothing, so the identity it named is still free: the
        // same portal and buyer/supplier pair creates, rather than conflicting with a
        // preference the refusal left behind.
        let mut retried = minimal_create(portal(), &buyer, &supplier);
        retried[ENDPOINT_MEMBER] = json!("https://portal.example.com/cxml");
        let (retry_status, retry_body) =
            call(&router, "CreateProcurementPortalPreference", &retried).await;
        assert_eq!(
            retry_status, 200,
            "case {case:?}: a refused create writes nothing, so the identity it named \
             creates, got {retry_status} {retry_body}"
        );
    }
}

/// THE SAME RULE through the UPDATE: it applies to the value the request states, a refused
/// update leaves the preference exactly as it was, and an accepted one replaces the stored
/// endpoint.
#[tokio::test]
async fn an_update_stores_only_a_secure_delivery_endpoint() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in endpoint_rows().iter().enumerate() {
        let case = row.case;
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    portal(),
                    &format!("update-buyer-{index}"),
                    &format!("update-supplier-{index}"),
                ),
            )
            .await,
        );
        let before = get_preference(&router, &arn).await;

        let mut update = put_request(&arn);
        update[ENDPOINT_MEMBER] = json!(row.endpoint);
        let (status, body) = call(&router, "PutProcurementPortalPreference", &update).await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the update expected HTTP 200 for {:?}, got {status} {body}",
                row.endpoint
            );
            let after = get_preference(&router, &arn).await;
            assert_eq!(
                after.get(ENDPOINT_MEMBER).and_then(Value::as_str),
                Some(row.endpoint),
                "case {case:?}: the preference must report the endpoint its update stated, \
                 got {after}"
            );
            continue;
        }

        assert_refusal(case, "the update", status, &body, row.endpoint);
        let after = get_preference(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, so the preference must still \
             report what its create left, got {after}"
        );
    }
}

/// An update that states NO endpoint keeps the one an earlier write already stored: the
/// rule is about the value a write states, so a request that states none has nothing for
/// it to be about.
#[tokio::test]
async fn an_update_that_states_no_endpoint_keeps_the_stored_one() {
    let (router, _engine) = emulator_with_engine();

    let stored = "https://portal.example.com/cxml";
    let mut request = minimal_create(portal(), "kept-buyer", "kept-supplier");
    request[ENDPOINT_MEMBER] = json!(stored);
    let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &request).await);

    let (status, body) =
        call(&router, "PutProcurementPortalPreference", &put_request(&arn)).await;
    assert_eq!(
        status, 200,
        "an update stating no endpoint is served, got {status} {body}"
    );
    let after = get_preference(&router, &arn).await;
    assert_eq!(
        after.get(ENDPOINT_MEMBER).and_then(Value::as_str),
        Some(stored),
        "an update stating no endpoint keeps the stored one, got {after}"
    );
}

/// THE SCOPE: the requirement belongs to the members that DOCUMENT it.
/// `TestEnvPreferenceInput$ProcurementPortalInstanceEndpoint` documents none, so a write
/// that configures the test environment on another scheme is served and reports it back —
/// and the production member of the same write still answers the rule.
#[tokio::test]
async fn the_requirement_reaches_the_members_that_document_it() {
    let (router, _engine) = emulator_with_engine();

    // (label, the test environment's endpoint)
    let test_env_endpoints = [
        ("the required scheme", "https://test.portal.example.com/cxml"),
        ("the plaintext scheme", "http://test.portal.example.com/cxml"),
        ("no scheme at all", "test.portal.example.com"),
    ];

    for (index, (case, endpoint)) in test_env_endpoints.iter().enumerate() {
        let mut request = minimal_create(
            portal(),
            &format!("scope-buyer-{index}"),
            &format!("scope-supplier-{index}"),
        );
        request[ENDPOINT_MEMBER] = json!("https://portal.example.com/cxml");
        request["TestEnvPreference"] = json!({
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": format!("scope-test-buyer-{index}"),
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": format!("scope-test-supplier-{index}"),
            "ProcurementPortalInstanceEndpoint": endpoint,
        });
        let (status, body) = call(&router, "CreateProcurementPortalPreference", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a test-environment endpoint documents no scheme requirement, so \
             {endpoint:?} is served, got {status} {body}"
        );
        let reported = get_preference(&router, &arn_of(&body)).await;
        assert_eq!(
            reported
                .get("TestEnvPreference")
                .and_then(|test| test.get(ENDPOINT_MEMBER))
                .and_then(Value::as_str),
            Some(*endpoint),
            "case {case:?}: the preference must report the test-environment endpoint it \
             stated, got {reported}"
        );
    }
}
