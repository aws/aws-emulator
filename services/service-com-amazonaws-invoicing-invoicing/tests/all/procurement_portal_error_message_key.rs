// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS and KEYS the explanatory text of a refusal, driven end to end
//! over its wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body. Nothing
//! reaches into the state struct, so what these tests see is exactly what an SDK client
//! sees — including the exact JSON key the text arrives under, which is what three of
//! these rules are about.
//!
//! The rules pinned here, each over a table spanning its class rather than one recorded
//! request:
//!
//! 1. The WIRE KEY of a refusal's explanatory text is a property of the PLANE that
//!    answers: every refusal of the procurement-portal-preference plane carries it as
//!    `Message`, and every refusal of the invoice-unit, invoice-profile and tag planes
//!    carries it as `message` — the member name the model spells. The rows span the error
//!    shapes each plane answers with (not-found, validation, conflict, quota, access
//!    denied) and several operations per shape, so the rule is asserted over the plane
//!    rather than over one operation or one shape.
//! 2. A not-found of the procurement-portal-preference plane WORDS that text from the
//!    REQUEST: it names the kind of resource asked about and the exact ARN the request
//!    supplied, so two ARNs are reported differently. The invoice-unit and tag planes word
//!    theirs as the constant sentence the shape documents, which is the contrast that
//!    makes rule 1 a property of the plane.
//! 3. A `VerifyProcurementPortalValidation` refusal states WHICH attempt failed, out of
//!    the three-attempt budget, counting the failed submissions this service recorded
//!    against that preference — so the count advances with each refusal, is per
//!    preference, and never reproduces the submitted code.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// The portal whose create states a Marketplace PunchOut configuration requirement of its
/// own (`CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`:
/// "Required for Coupa when MarketplacePunchOutEnabled is true"), which one row provokes.
const PORTAL_REQUIRING_PUNCH_OUT_CONFIG: &str = "COUPA";

/// A portal a create needs no further configuration for, so a row's write turns on what
/// the row is about: the first value `ProcurementPortalName` admits other than the portal
/// above (`smithy_query(query="shape", shape="ProcurementPortalName")`).
fn plain_portal() -> &'static str {
    PROCUREMENT_PORTAL_NAME_VALUES
        .iter()
        .copied()
        .find(|portal| *portal != PORTAL_REQUIRING_PUNCH_OUT_CONFIG)
        .expect("the model declares a portal beside the one with a configuration requirement")
}

/// The key the PROCUREMENT-PORTAL-PREFERENCE plane's refusals carry their explanatory
/// text under, as every recorded body of that plane keys it.
const PORTAL_PLANE_KEY: &str = "Message";

/// The key every other plane of this service keys that text under: the member name the
/// model spells (`smithy_query(query="shape",
/// shape="ResourceNotFoundException,ValidationException")` — a member `message`, no
/// `@jsonName`), which is what the awsJson codecs write.
const MODEL_MEMBER_KEY: &str = "message";

/// The sentence a 404 of the INVOICE-UNIT and TAG planes carries, from the shape's own
/// documentation (`smithy_query(query="shape", shape="ResourceNotFoundException")`).
const RESOURCE_NOT_FOUND_MESSAGE: &str = "The resource could not be found.";

/// How many submissions of a validation code a refusal of this service counts against —
/// the budget AWS's recorded refusal states.
const VERIFICATION_ATTEMPT_BUDGET: u32 = 3;

/// An account id of the modelled `AccountIdString` form that no invoice unit of these
/// tests names, so `BatchGetInvoiceProfile` refuses it as a non-member.
const NON_MEMBER_ACCOUNT: &str = "111111111111";

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

/// The text under one key of a failure body, if the body carries that key at all.
fn text_under(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The one contact a write carries (`Contacts` is modelled `@length(min: 1, max: 1)`).
fn contacts(local_part: &str) -> Value {
    json!([{"Name": "Portal Administrator", "Email": format!("{local_part}@example.com")}])
}

/// A `CreateProcurementPortalPreference` request for one portal and buyer/supplier pair.
fn create_preference_request(portal: &str, identifier: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": format!("{identifier}-buyer"),
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": format!("{identifier}-supplier"),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "ProcurementPortalInstanceEndpoint": format!("https://{identifier}.portal.example.com"),
        "Contacts": contacts(identifier),
    })
}

/// Create one procurement portal preference and answer with its ARN.
async fn create_preference(router: &MultiServiceRouter, portal: &str, identifier: &str) -> String {
    let created = ok(
        router,
        "CreateProcurementPortalPreference",
        &create_preference_request(portal, identifier),
    )
    .await;
    created
        .get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {created}"))
        .to_string()
}

/// Create one invoice unit and answer with its ARN.
async fn create_unit(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateInvoiceUnit",
        &json!({
            "Name": name,
            "InvoiceReceiver": "123456789012",
            "Rule": {"LinkedAccounts": ["210987654321"]},
        }),
    )
    .await;
    created
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {created}"))
        .to_string()
}

/// A well-formed procurement-portal-preference ARN no create produced
/// (`smithy_query(query="operation", operation="GetProcurementPortalPreference")`:
/// `@pattern ^arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+$`).
fn absent_preference_arn(id: &str) -> String {
    format!("arn:aws:invoicing::{}:procurement-portal-preference/{id}", engine::emulator_account_id())
}

/// A well-formed invoice-unit ARN no create produced.
fn absent_unit_arn(id: &str) -> String {
    format!(
        "arn:aws:invoicing::{}:invoice-unit/{id}",
        engine::emulator_account_id()
    )
}

/// One refusal the rule is asserted over: which plane answers it, the call that provokes
/// it, and the status and error code it must answer with.
#[derive(Debug)]
struct RefusalCase {
    case: String,
    operation: String,
    request: Value,
    status: u16,
    error_code: &'static str,
}

/// RULE 1: the wire key of a refusal's explanatory text is the answering PLANE's.
///
/// The rows span, per plane, the error shapes that plane refuses with and several
/// operations per shape — a reader, two writes and the two validation calls on the portal
/// plane; the invoice-unit reader and writes, the tag plane, the invoice-profile plane and
/// a paginated walk on the other side — so what is asserted is the plane rather than one
/// operation, one shape or one recorded body.
#[tokio::test]
async fn a_refusals_message_key_is_the_answering_planes() {
    let router = emulator();

    // Resources these rows refuse AGAINST, so a refusal is about the request rather than
    // about an empty store.
    let held_preference = create_preference(&router, plain_portal(), "held").await;
    let coupa_preference =
        create_preference(&router, PORTAL_REQUIRING_PUNCH_OUT_CONFIG, "coupa-held").await;
    let held_unit = create_unit(&router, "HeldUnit").await;

    let absent_preference = absent_preference_arn("does-not-exist-000");
    let absent_unit = absent_unit_arn("does-not-exist-000");

    // The procurement-portal-preference plane: every refusal keys the text `Message`.
    let portal_plane: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "read an absent preference".to_string(),
            operation: "GetProcurementPortalPreference".to_string(),
            request: json!({"ProcurementPortalPreferenceArn": absent_preference}),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "update an absent preference".to_string(),
            operation: "PutProcurementPortalPreference".to_string(),
            request: json!({
                "ProcurementPortalPreferenceArn": absent_preference,
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
                "Contacts": contacts("absent"),
            }),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "delete an absent preference".to_string(),
            operation: "DeleteProcurementPortalPreference".to_string(),
            request: json!({"ProcurementPortalPreferenceArn": absent_preference}),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "start a validation for an absent preference".to_string(),
            operation: "SendProcurementPortalValidation".to_string(),
            request: json!({"ProcurementPortalPreferenceArn": absent_preference}),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "complete a validation for an absent preference".to_string(),
            operation: "VerifyProcurementPortalValidation".to_string(),
            request: json!({
                "ProcurementPortalPreferenceArn": absent_preference,
                "Code": "000000",
            }),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "submit a code that is not the outstanding one".to_string(),
            operation: "VerifyProcurementPortalValidation".to_string(),
            request: json!({
                "ProcurementPortalPreferenceArn": held_preference,
                "Code": "000000",
            }),
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "create a preference whose identity is already held".to_string(),
            operation: "CreateProcurementPortalPreference".to_string(),
            request: create_preference_request(plain_portal(), "held"),
            status: 409,
            error_code: "ConflictException",
        },
        RefusalCase {
            case: "create a preference missing a combination the portal requires"
                .to_string(),
            operation: "CreateProcurementPortalPreference".to_string(),
            request: json!({
                "ProcurementPortalName": PORTAL_REQUIRING_PUNCH_OUT_CONFIG,
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "punch-out-buyer",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "punch-out-supplier",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
                "MarketplacePunchOutEnabled": true,
                "Contacts": contacts("punch-out"),
            }),
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "walk the preferences with a token this service did not mint".to_string(),
            operation: "ListProcurementPortalPreferences".to_string(),
            request: json!({"NextToken": "not-a-token-this-service-minted"}),
            status: 400,
            error_code: "ValidationException",
        },
    ];

    // Every other plane: the text travels under the member name the model spells.
    let other_planes: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "read an absent invoice unit".to_string(),
            operation: "GetInvoiceUnit".to_string(),
            request: json!({"InvoiceUnitArn": absent_unit}),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "update an absent invoice unit".to_string(),
            operation: "UpdateInvoiceUnit".to_string(),
            request: json!({"InvoiceUnitArn": absent_unit, "Description": "nope"}),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "delete an absent invoice unit".to_string(),
            operation: "DeleteInvoiceUnit".to_string(),
            request: json!({"InvoiceUnitArn": absent_unit}),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "create an invoice unit whose name is already held".to_string(),
            operation: "CreateInvoiceUnit".to_string(),
            request: json!({
                "Name": "HeldUnit",
                "InvoiceReceiver": "123456789012",
                "Rule": {"LinkedAccounts": ["210987654321"]},
            }),
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "read the tags of an absent resource".to_string(),
            operation: "ListTagsForResource".to_string(),
            request: json!({"ResourceArn": absent_unit}),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "tag an absent resource".to_string(),
            operation: "TagResource".to_string(),
            request: json!({
                "ResourceArn": absent_unit,
                "ResourceTags": [{"Key": "k", "Value": "v"}],
            }),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "read the profile of an account outside the organization".to_string(),
            operation: "BatchGetInvoiceProfile".to_string(),
            request: json!({"AccountIds": [NON_MEMBER_ACCOUNT]}),
            status: 403,
            error_code: "AccessDeniedException",
        },
        RefusalCase {
            case: "walk the invoice units with a token this service did not mint".to_string(),
            operation: "ListInvoiceUnits".to_string(),
            request: json!({"NextToken": "not-a-token-this-service-minted"}),
            status: 400,
            error_code: "ValidationException",
        },
    ];

    // The invoice unit and the Coupa preference are set up for rows above; naming them
    // here keeps the setup from reading as unused.
    assert!(
        !held_unit.is_empty() && !coupa_preference.is_empty(),
        "the setup must have minted the resources the rows refuse against"
    );

    for (expected_key, forbidden_key, cases) in [
        (PORTAL_PLANE_KEY, MODEL_MEMBER_KEY, portal_plane),
        (MODEL_MEMBER_KEY, PORTAL_PLANE_KEY, other_planes),
    ] {
        for case in &cases {
            let label = &case.case;
            let operation = &case.operation;
            let (status, body) = call(&router, &case.operation, &case.request).await;
            assert_eq!(
                status, case.status,
                "case {label} ({operation}): expected HTTP {}, got {status} {body}",
                case.status
            );
            assert_eq!(
                error_code(&body),
                case.error_code,
                "case {label} ({operation}): expected a {}, got {body}",
                case.error_code
            );
            let text = text_under(&body, expected_key);
            assert!(
                text.as_deref().is_some_and(|text| !text.is_empty()),
                "case {label} ({operation}): expected explanatory text under \
                 {expected_key:?}, got {body}"
            );
            assert!(
                text_under(&body, forbidden_key).is_none(),
                "case {label} ({operation}): the text travels under {expected_key:?} \
                 only, got {body}"
            );
        }
    }
}

/// RULE 2: a not-found of the procurement-portal-preference plane words its text from the
/// REQUEST — the kind of resource and the exact ARN — while the invoice-unit and tag
/// planes word theirs as the sentence the shape documents.
///
/// The ARN rows differ along what the rule generalises over: the id's characters and its
/// length, from one character to the longest this table states.
#[tokio::test]
async fn a_not_found_of_the_portal_plane_is_worded_from_the_request() {
    let router = emulator();
    // One preference and one unit exist, so each row is reported absent on its own
    // account rather than because the store is empty.
    create_preference(&router, plain_portal(), "held").await;
    create_unit(&router, "HeldUnit").await;

    let ids = [
        "A",
        "ffffffffffff",
        "does-not-exist-000",
        "a-very-long-procurement-portal-preference-identifier-0123456789",
    ];

    let mut seen: Vec<String> = Vec::new();
    for id in ids {
        let arn = absent_preference_arn(id);
        for operation in [
            "GetProcurementPortalPreference",
            "DeleteProcurementPortalPreference",
            "SendProcurementPortalValidation",
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({"ProcurementPortalPreferenceArn": arn}),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {operation} on {arn}: expected HTTP 404, got {status} {body}"
            );
            let text = text_under(&body, PORTAL_PLANE_KEY).unwrap_or_default();
            assert!(
                text.contains(&arn),
                "case {operation} on {arn}: the text must name the ARN asked about, got {body}"
            );
            assert!(
                text.to_lowercase().contains("procurement portal preference"),
                "case {operation} on {arn}: the text must name the kind of resource, got {body}"
            );
            assert_ne!(
                text, RESOURCE_NOT_FOUND_MESSAGE,
                "case {operation} on {arn}: this plane words the text from the request, got {body}"
            );
            // The ARN is still reported mechanically, so a client need not parse the prose.
            assert_eq!(
                text_under(&body, "resourceId").as_deref(),
                Some(arn.as_str()),
                "case {operation} on {arn}: the ARN travels in its own member too, got {body}"
            );
            seen.push(text);
        }
    }
    // Built from the request: no two ARNs are reported with the same sentence.
    let mut distinct = seen.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "the text must differ with the ARN asked about, got {seen:?}"
    );

    // The contrast: the invoice-unit and tag planes answer the documented constant.
    for (operation, request) in [
        (
            "GetInvoiceUnit",
            json!({"InvoiceUnitArn": absent_unit_arn("does-not-exist-000")}),
        ),
        (
            "DeleteInvoiceUnit",
            json!({"InvoiceUnitArn": absent_unit_arn("ffffffffffff")}),
        ),
        (
            "ListTagsForResource",
            json!({"ResourceArn": absent_unit_arn("A")}),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 404,
            "case {operation} {request}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            text_under(&body, MODEL_MEMBER_KEY).as_deref(),
            Some(RESOURCE_NOT_FOUND_MESSAGE),
            "case {operation} {request}: expected the documented constant, got {body}"
        );
    }
}

/// RULE 3: a refused submission of a validation code states WHICH attempt failed, out of
/// the budget, from the count this service recorded against that preference.
///
/// The rows differ along the form of the wrong code (a six-digit code, a longer one, a
/// non-digit one, the one issued for another preference is covered in
/// `retired_units_and_portal_validation.rs`) and along how many attempts have been made,
/// including past the budget — and the count is asserted to be PER PREFERENCE.
#[tokio::test]
async fn a_refused_validation_code_states_the_attempt_and_the_budget() {
    let router = emulator();
    let first = create_preference(&router, plain_portal(), "attempts-first").await;
    let second = create_preference(
        &router,
        PORTAL_REQUIRING_PUNCH_OUT_CONFIG,
        "attempts-second",
    )
    .await;

    // One row per attempt, so the reported attempt number has to advance with the count
    // this service keeps rather than being a constant.
    let wrong_codes = ["000000", "1234567", "not-a-code", "999999"];
    for (index, wrong) in wrong_codes.iter().enumerate() {
        let attempt = u32::try_from(index + 1).expect("the table is small");
        let (status, body) = call(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": first, "Code": wrong}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case attempt {attempt} with {wrong:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case attempt {attempt} with {wrong:?}: expected a ValidationException, got {body}"
        );
        let text = text_under(&body, PORTAL_PLANE_KEY).unwrap_or_default();
        assert!(
            text.contains(&format!("Attempt {attempt} of {VERIFICATION_ATTEMPT_BUDGET}")),
            "case attempt {attempt} with {wrong:?}: the text must state the attempt that \
             failed and the budget, got {body}"
        );
        assert!(
            !text.contains(wrong),
            "case attempt {attempt} with {wrong:?}: the refusal must not reproduce the \
             submitted code, got {body}"
        );
        assert_eq!(
            text_under(&body, MODEL_MEMBER_KEY),
            None,
            "case attempt {attempt} with {wrong:?}: the text travels under \
             {PORTAL_PLANE_KEY:?} only, got {body}"
        );
    }

    // The count belongs to the preference it was recorded against: another preference's
    // first refusal is its first attempt, not the fifth.
    let (status, body) = call(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": second, "Code": "000000"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case another preference's first attempt: expected HTTP 400, got {status} {body}"
    );
    let text = text_under(&body, PORTAL_PLANE_KEY).unwrap_or_default();
    assert!(
        text.contains(&format!("Attempt 1 of {VERIFICATION_ATTEMPT_BUDGET}")),
        "case another preference's first attempt: the count is per preference, got {body}"
    );
}
