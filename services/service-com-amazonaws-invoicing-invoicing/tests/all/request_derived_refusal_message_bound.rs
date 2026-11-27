// SPDX-License-Identifier: CC0-1.0

//! The SIZE of a refusal this service words from a request, driven end to end over this
//! service's wire protocol: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an
//! awsJson1_0 body through the same `MultiServiceRouter` the production router uses, so
//! what these tests read is what an SDK client reads.
//!
//! The rule under test: a message this service words from a request is held to the
//! modelled maximum of the member it travels in — every error shape of this model carries
//! its explanatory text in a `message` member targeting `BasicString`,
//! `@length(min: 0, max: 1024)` (`smithy_query(query="shape", shape="BasicString")`), and
//! `@length` on a string counts CHARACTERS — so no request can make a refusal carry a text
//! that member never holds, nor decide how large the refusal is.
//!
//! `message_fits_the_modelled_bound.rs` pins that bound over the number of violations a
//! refusal states and over the LENGTH of the one caller-supplied identifier of this model
//! with no maximum of its own (`ListProcurementPortalSuppliers`' `PortalIdentifier`,
//! which targets `ProcurementPortalIdString`, `@length(min: 1)` and no max). This suite
//! spans what a length alone does not reach:
//!
//! 1. the WIDTH of the characters that identifier is made of — one, two, three and four
//!    bytes each — so the bound is held in characters, the cut lands on a character
//!    boundary, and the refusal's size in BYTES stays the service's to decide however
//!    large the request is;
//! 2. the OTHER messages this service words from a request — the preference-ARN 404, the
//!    403 naming a refused account and the duplicate-`Name` 400 — each at the longest
//!    value its own member admits, where that message is longest.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// The longest text a `message` member of this model may carry: every one of them targets
/// `BasicString`, `@length(min: 0, max: 1024)` (`smithy_query(query="shape",
/// shape="BasicString")`), counted in CHARACTERS.
const MAX_MESSAGE_LENGTH: usize = 1024;

/// How many bytes a refusal body of this service may run to. The text is bounded in
/// CHARACTERS and UTF-8 spends at most four bytes on one character; the rest is the error
/// envelope (`__type`, the key the text travels under, the braces). No request can push
/// the body past this, however long the value it names.
const MAX_BODY_BYTES: usize = 4 * MAX_MESSAGE_LENGTH + 512;

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

/// One awsJson1_0 request, returning the HTTP status, the parsed body and how many BYTES
/// that body ran to on the wire — which is what makes "a small request cannot inflate the
/// refusal" an assertion rather than a claim.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value, usize) {
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
    let bytes = response.body().len();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed, bytes)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed, _) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200 from {operation}, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The explanatory text a refusal carries, under either of the keys this service's planes
/// write it under (`procurement_portal_error_message_key.rs` pins WHICH key; these tests
/// are about the text).
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), distinct
/// from the requester's own account, which is all zeroes.
fn account(index: u32) -> String {
    format!("{:012}", 700 + index)
}

/// `character` repeated to exactly `length` characters.
fn repeated(character: char, length: usize) -> String {
    character.to_string().repeat(length)
}

/// The sentence the procurement-portal plane's 404 states for an identifier no portal of
/// this service answers to.
fn no_such_portal_sentence(identifier: &str) -> String {
    format!("Procurement portal with identifier {identifier} does not exist")
}

/// RULE, over the WIDTH of the characters a caller-supplied identifier is made of: a 404
/// naming an identifier with no modelled maximum is held to the bound in CHARACTERS
/// whatever those characters cost in bytes, the text stays that sentence's own prefix (so
/// the cut is on a character boundary and the body is still readable JSON), and the
/// refusal's size in bytes is the service's rather than the request's.
#[tokio::test]
async fn a_portal_404_holds_the_bound_in_characters_whatever_they_cost_in_bytes() {
    let router = emulator();

    // Rows differing along the width of the identifier's characters — one, two, three and
    // four bytes each — and along its length: short enough to be stated whole, just past
    // what the sentence has room for, and far past it (a hundred thousand characters,
    // which is 400 KB of request for the widest row).
    let characters = [
        ("one-byte", 'z'),
        ("two-byte", 'ß'),
        ("three-byte", '✓'),
        ("four-byte", '𝛼'),
    ];
    // The longest identifier the whole sentence still fits: the bound less its fixed text.
    let fitting = MAX_MESSAGE_LENGTH - no_such_portal_sentence("").chars().count();
    let lengths = [1usize, fitting, fitting + 1, 100_000];

    for (width, character) in characters {
        for length in lengths {
            let case = format!("{width} characters, {length} of them");
            // A run of one character is none of the catalog's identifiers (which are
            // lowercase portal names) at any length, so every row is a 404.
            let identifier = repeated(character, length);
            let (status, body, body_bytes) = call(
                &router,
                "ListProcurementPortalSuppliers",
                &json!({"PortalIdentifier": identifier}),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case}: expected a ResourceNotFoundException, got {body}"
            );

            let message = error_message(&body);
            let stated = message.chars().count();
            assert!(
                stated <= MAX_MESSAGE_LENGTH,
                "case {case}: expected a text within {MAX_MESSAGE_LENGTH} characters, \
                 got {stated}"
            );
            assert!(
                body_bytes <= MAX_BODY_BYTES,
                "case {case}: expected a refusal body within {MAX_BODY_BYTES} bytes, \
                 got {body_bytes}"
            );

            let sentence = no_such_portal_sentence(&identifier);
            if sentence.chars().count() <= MAX_MESSAGE_LENGTH {
                assert_eq!(
                    message, sentence,
                    "case {case}: a sentence that fits is stated whole, got {stated} characters"
                );
            } else {
                let expected: String = sentence.chars().take(MAX_MESSAGE_LENGTH).collect();
                assert_eq!(
                    message, expected,
                    "case {case}: expected the sentence as far as the member reaches, \
                     got {stated} characters"
                );
            }
        }
    }

    // The control: holding the text to the bound changes nothing about WHICH identifiers
    // resolve — a portal this service serves is still listed rather than refused. The
    // identifier is the one the catalog derives from a modelled portal name: that name
    // lowercased with its underscores hyphenated.
    let served = PROCUREMENT_PORTAL_NAME_VALUES[0]
        .to_lowercase()
        .replace('_', "-");
    let listed = ok(
        &router,
        "a served portal",
        "ListProcurementPortalSuppliers",
        &json!({"PortalIdentifier": served}),
    )
    .await;
    assert!(
        listed.get("ProcurementPortalSuppliers").is_some(),
        "case a served portal: expected a supplier listing, got {listed}"
    );
}

/// One row of the request-derived table: the operation to drive, the request, the status it
/// is refused with and the whole sentence it must carry.
struct RequestDerivedCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    status: u16,
    expected_message: String,
}

/// RULE, over the OTHER messages this service words from a request: each is held to the
/// same bound, and because the member each of their values comes from carries a modelled
/// maximum of its own, each is stated WHOLE — the bound truncates nothing a client could
/// otherwise have read.
#[tokio::test]
async fn every_other_request_derived_refusal_states_its_bounded_value_whole() {
    let router = emulator();
    let own = engine::emulator_account_id().to_string();

    // One invoice unit exists, so the duplicate-name row is refused on the name rather
    // than on an empty registry, and the organization names a linked account, so the 403
    // row is refused on the account asked about.
    let longest_name = repeated('n', 50);
    ok(
        &router,
        "seed",
        "CreateInvoiceUnit",
        &json!({
            "Name": longest_name,
            "InvoiceReceiver": account(9),
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": [account(1)]},
            "ClientToken": "request-derived-seed",
        }),
    )
    .await;

    // Rows differing along the operation that answers and the member the value came from:
    // a preference ARN (`ProcurementPortalPreferenceArnString`, `@length(1..256)`), an
    // account id (`AccountIdString`, `@pattern ^\d{12}$`) and an invoice unit `Name`
    // (`InvoiceUnitName`, `@length(1..50)`), each at the longest value it admits.
    let arn = format!(
        "arn:aws:invoicing::000000000000:procurement-portal-preference/{}",
        repeated('a', 60)
    );
    let refused_account = account(31);
    let cases = [
        RequestDerivedCase {
            case: "a preference ARN no create produced",
            operation: "GetProcurementPortalPreference",
            request: json!({"ProcurementPortalPreferenceArn": arn}),
            status: 404,
            expected_message: format!("Procurement portal preference with arn {arn} does not exist"),
        },
        RequestDerivedCase {
            case: "an account outside the organization",
            operation: "BatchGetInvoiceProfile",
            request: json!({"AccountIds": [refused_account]}),
            status: 403,
            expected_message: format!(
                "Account {own} is not authorized to access resources for account {refused_account}"
            ),
        },
        RequestDerivedCase {
            case: "the longest duplicate invoice unit name",
            operation: "CreateInvoiceUnit",
            request: json!({
                "Name": longest_name,
                "InvoiceReceiver": account(9),
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": [account(1)]},
                "ClientToken": "request-derived-duplicate",
            }),
            status: 400,
            expected_message: format!("Invoice Unit already exists with name: <{longest_name}>"),
        },
    ];

    for case in cases {
        let (status, body, body_bytes) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, case.status,
            "case {}: expected HTTP {} from {}, got {status} {body}",
            case.case, case.status, case.operation
        );
        let message = error_message(&body);
        assert_eq!(
            message, case.expected_message,
            "case {}: expected the sentence {:?}, got {message:?}",
            case.case, case.expected_message
        );
        let stated = message.chars().count();
        assert!(
            stated <= MAX_MESSAGE_LENGTH,
            "case {}: expected a text within {MAX_MESSAGE_LENGTH} characters, got {stated}",
            case.case
        );
        assert!(
            body_bytes <= MAX_BODY_BYTES,
            "case {}: expected a refusal body within {MAX_BODY_BYTES} bytes, got {body_bytes}",
            case.case
        );
    }
}
