// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED call of the procurement-portal-preference plane leaves behind, driven
//! end to end over this service's wire protocol (`POST /` with
//! `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body, through the same
//! `MultiServiceRouter` the production router uses).
//!
//! The rule under test: `ProcurementPortalPreference$Version` is "the version number of
//! the procurement portal preference configuration" and `LastUpdateDate` is "the date and
//! time when the procurement portal preference was last updated"
//! (`smithy_query(query="shape", shape="ProcurementPortalPreference")`), so a call the
//! service refuses BEFORE it writes anything leaves BOTH exactly as the last write left
//! them — however many times it is repeated. The version a reader sees is therefore one
//! plus the number of calls that wrote the resource, and a refusal that wrote nothing never
//! shifts it.
//!
//! Each test is a table rather than one request: the rows span the refusal classes this
//! plane answers (the configuration update, the status update — its empty request, its
//! unmodelled status and the transitions a caller may not ask for — and a conflicting
//! create: 400 and 409 alike), the portals a preference can route to, the ORDER refusals
//! and successful calls arrive in, and — for the validation flow, whose refusal keeps a
//! failed-attempt count of its own — whether a validation code is OUTSTANDING when the
//! submitted one is refused. A code is made outstanding through the call that issues one,
//! `SendProcurementPortalValidation`, and then read from state, because the modelled
//! response carries only the ARN.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;
use service_com_amazonaws_invoicing_invoicing::state::ComAmazonawsInvoicingInvoicingState;

/// Every value `ProcurementPortalName` accepts, taken from the model's own value set
/// (`smithy_query(query="shape", shape="ProcurementPortalName")`) rather than restated
/// here, so a table over the portals follows the model.
pub(crate) const PORTAL_NAMES: &[&str] = PROCUREMENT_PORTAL_NAME_VALUES;

/// The portal the row at `index` of a table routes to. A table may carry more rows than
/// the model declares portals, so a row past the end wraps round the value set: the rows
/// of these tables are told apart by the identity each one creates, not by the portal it
/// names.
pub(crate) fn portal_at(index: usize) -> &'static str {
    PORTAL_NAMES[index % PORTAL_NAMES.len()]
}

/// The portal the CREATE's own member states its Marketplace PunchOut configuration
/// requirement for (`CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`:
/// "Required for Coupa when MarketplacePunchOutEnabled is true"); the UPDATE states it for
/// every portal, which is what makes one row's update refusable.
pub(crate) const PORTAL_REQUIRING_PUNCH_OUT_CONFIG: &str = "COUPA";

/// A portal whose create states no Marketplace PunchOut configuration requirement: the
/// first value `ProcurementPortalName` admits other than
/// [`PORTAL_REQUIRING_PUNCH_OUT_CONFIG`], so a row that enables Marketplace PunchOut
/// without stating its configuration is accepted.
pub(crate) fn portal_without_punch_out_requirement() -> &'static str {
    PORTAL_NAMES
        .iter()
        .copied()
        .find(|portal| *portal != PORTAL_REQUIRING_PUNCH_OUT_CONFIG)
        .expect("the model declares a portal beside the one its create requires a configuration for")
}

/// A status every plane of a preference may hold: one of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const STATUS_ACTIVE: &str = "ACTIVE";

/// The status a plane reports once the connection test its configuration names has been
/// started — the move a caller may ask for out of the validated status.
pub(crate) const STATUS_TEST_INITIALIZED: &str = "TEST_INITIALIZED";

/// How many submissions of a validation code a refusal of this service counts against —
/// the budget AWS's recorded refusal states.
pub(crate) const VERIFICATION_ATTEMPT_BUDGET: u32 = 3;

/// How many times a row repeats its refused submission: past that budget, so a row
/// crosses it and the rule under test still holds.
const REPEATED_SUBMISSIONS: u32 = 4;

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services. The engine is handed back for
/// the one thing no wire carries: the validation code the portal flow issues.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// The same router, when the test has no reason to reach state.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
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
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without the
/// namespace a backend stamps in front of it.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `ProcurementPortalPreferenceArn` of a response that carries one.
pub(crate) fn arn_of(body: &Value) -> String {
    body.get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {body}"))
        .to_string()
}

/// One contact, the single element `Contacts` (`@length(min: 1, max: 1)`) admits.
fn contact(name: &str) -> Value {
    json!([{ "Name": name, "Email": format!("{}@example.com", name.to_lowercase().replace(' ', ".")) }])
}

/// A create request for the portal and buyer/supplier pair that identify a preference,
/// carrying only the members the model marks required.
pub(crate) fn minimal_create(portal: &str, buyer: &str, supplier: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": buyer,
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": supplier,
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contact("Initial Contact"),
    })
}

/// The endpoint a preference configures as "the endpoint URL where e-invoices are delivered
/// to the procurement portal" (`ProcurementPortalInstanceEndpoint`,
/// `smithy_query(query="shape", shape="ProcurementPortalPreference")`), inside that
/// member's `^\S+$`.
pub(crate) const PORTAL_INSTANCE_ENDPOINT: &str = "https://portal.example.com/cxml";

/// A create request like [`minimal_create`], configuring in addition the endpoint the test
/// e-invoice of a portal validation is delivered to — so the send that issues a code has a
/// delivery TARGET, and the refusal it answers names that target rather than reporting that
/// there was nothing to deliver to.
pub(crate) fn create_with_delivery_endpoint(portal: &str, buyer: &str, supplier: &str) -> Value {
    let mut request = minimal_create(portal, buyer, supplier);
    request["ProcurementPortalInstanceEndpoint"] = json!(PORTAL_INSTANCE_ENDPOINT);
    request
}

/// The sentence a send is refused with when there is no endpoint for the test e-invoice to
/// be delivered to at all: recorded behaviour, and FIXED text for this error class — it
/// carries nothing of the request and nothing of the preference's state.
pub(crate) const DELIVERY_NOT_CONFIRMED: &str = "We could not confirm delivery of the test \
     e-invoice. The OTP may have been sent but rejected by your procurement portal's \
     business-rule validation. Check your portal's rejected-invoice log for the OTP (for \
     example, in Coupa open Setup then Integrations then cXML Errors; in SAP Business Network \
     review your Invoice Rules) and submit it to complete verification. If the OTP is not \
     there, verify your portal configuration or contact support.";

/// The fixed path under a configured endpoint that the test e-invoice is delivered to: the
/// portal's cXML invoice intake, which the refusal below names under the endpoint.
pub(crate) const DELIVERY_PATH: &str = "/cxml/invoices";

/// The whole sentence a send is refused with when the test e-invoice could not be delivered
/// to the endpoint the preference configures for it: the delivery TARGET is the only
/// substituted part, and every other word is fixed text for this error class.
pub(crate) fn undeliverable_sentence(endpoint: &str) -> String {
    format!(
        "We were unable to deliver the test e-invoice to the endpoint {}{DELIVERY_PATH}. Please \
         confirm the endpoint URL is correct and reachable, then try again.",
        endpoint.trim_end_matches('/')
    )
}

/// Carry every enabled plane of `arn` to [`STATUS_VALIDATED`] by completing its portal
/// validation — the path a caller has to the statuses a status update may then ask for,
/// because the statuses the service assigns itself are not a caller's to state.
pub(crate) async fn validate_preference(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) {
    let code = issue_code(router, engine, arn).await;
    ok(
        router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": arn, "Code": code}),
    )
    .await;
}

/// The preference `arn` names, as `GetProcurementPortalPreference` reports it.
pub(crate) async fn get_preference(router: &MultiServiceRouter, arn: &str) -> Value {
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

/// The summary `ListProcurementPortalPreferences` reports for `arn`, so a refusal is
/// checked against both readers of this plane rather than only the single-resource one.
pub(crate) async fn listed_summary(router: &MultiServiceRouter, arn: &str) -> Value {
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

/// The reported `Version` of a preference or its summary.
pub(crate) fn version(preference: &Value) -> i64 {
    preference
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a Version in {preference}"))
}

/// The reported `LastUpdateDate`, as the epoch-seconds number it travels as.
pub(crate) fn last_update(preference: &Value) -> f64 {
    preference
        .get("LastUpdateDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a LastUpdateDate in {preference}"))
}

/// A reported preference WITHOUT the two members a write of it advances, so what is left
/// is everything a write has to leave alone.
pub(crate) fn without_write_markers(preference: &Value) -> Value {
    let mut stripped = preference.clone();
    if let Some(members) = stripped.as_object_mut() {
        for advanced_by_every_write in ["Version", "LastUpdateDate"] {
            members.remove(advanced_by_every_write);
        }
    }
    stripped
}

/// The preference one row drives its calls against: its ARN and the identity it holds,
/// because a row's request can name either.
struct Subject {
    arn: String,
    portal: &'static str,
    buyer: String,
    supplier: String,
}

/// One class of refused request: which operation refuses it, the request built from the
/// subject preference, and the modelled answer it is refused with.
struct Refusal {
    label: &'static str,
    /// The portal the subject preference routes to, so the rows span the catalog and the
    /// update row reaches the portal whose configuration requirement refuses it.
    portal: &'static str,
    operation: &'static str,
    request: fn(&Subject) -> Value,
    http_status: u16,
    error_code: &'static str,
}

/// A valid configuration update: every member `PutProcurementPortalPreference` marks
/// required (`smithy_query(query="operation",
/// operation="PutProcurementPortalPreference")`), so this one SUCCEEDS.
fn accepted_update(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "Contacts": contact("Updated Contact"),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    })
}

/// A configuration update that enables Marketplace PunchOut without the configuration its
/// own input member documents as required while that flag is true.
fn update_missing_required_punch_out_config(subject: &Subject) -> Value {
    let mut request = accepted_update(subject);
    request["MarketplacePunchOutEnabled"] = json!(true);
    request
}

/// A validation start for the subject preference.
fn send_validation(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn})
}

/// The same start, carrying a `ClientToken` — so a replay of it answers with the ARN the
/// first call returned.
fn send_validation_with_token(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn, "ClientToken": "send-1"})
}

/// A code submitted for the subject preference while none is outstanding.
fn verify_unissued_code(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn, "Code": "123456"})
}

/// A status update that states no plane's status, so it asks for no update.
fn status_update_stating_nothing(subject: &Subject) -> Value {
    json!({"ProcurementPortalPreferenceArn": subject.arn})
}

/// A status update that states a plane's reason without that plane's status.
fn status_update_stating_a_reason_only(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "PurchaseOrderRetrievalPreferenceStatusReason": "a reason for nothing",
    })
}

/// A status update naming a status outside the modelled enum.
fn status_update_outside_the_enum(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "EinvoiceDeliveryPreferenceStatus": "NOT_A_STATUS",
    })
}

/// A status update asking a plane to take a status the SERVICE assigns itself, which is
/// not a caller's to state.
fn status_update_into_a_service_assigned_status(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "EinvoiceDeliveryPreferenceStatus": "SUSPENDED",
        "EinvoiceDeliveryPreferenceStatusReason": "a suspension the caller asked for",
    })
}

/// A status update asking for a move the lifecycle does not provide out of the status the
/// preference holds, to a status a caller does state for a plane that reached it.
fn status_update_into_an_unreachable_status(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "PurchaseOrderRetrievalPreferenceStatus": STATUS_ACTIVE,
    })
}

/// A create of the identity the subject preference already holds, which is the conflict a
/// second create of one resource is.
fn conflicting_create(subject: &Subject) -> Value {
    minimal_create(subject.portal, &subject.buyer, &subject.supplier)
}

/// Every refusal class this plane answers for a preference that EXISTS and that the
/// service refuses BEFORE writing anything, one row each.
fn refusal_classes() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "a status update stating no plane",
            portal: portal_at(1),
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_stating_nothing,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a status update stating a reason without its status",
            portal: portal_at(2),
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_stating_a_reason_only,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a status update naming a status outside the modelled enum",
            portal: portal_at(3),
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_outside_the_enum,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a status update into a status the service assigns itself",
            portal: portal_at(0),
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_into_a_service_assigned_status,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a status update into a status the lifecycle does not reach from here",
            portal: portal_at(1),
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_into_an_unreachable_status,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a configuration update missing the configuration its own member requires",
            portal: PORTAL_REQUIRING_PUNCH_OUT_CONFIG,
            operation: "PutProcurementPortalPreference",
            request: update_missing_required_punch_out_config,
            http_status: 400,
            error_code: "ValidationException",
        },
        Refusal {
            label: "a create of the identity this preference already holds",
            portal: portal_at(0),
            operation: "CreateProcurementPortalPreference",
            request: conflicting_create,
            http_status: 409,
            error_code: "ConflictException",
        },
    ]
}

/// A preference of this row's own, carrying one successful configuration update on top of
/// the create — so the version under test is not the one a create happens to leave.
async fn subject_at_version_two(
    router: &MultiServiceRouter,
    index: usize,
    portal: &'static str,
) -> Subject {
    let buyer = format!("BUYER-REFUSED-{index}");
    let supplier = format!("SUPPLIER-REFUSED-{index}");
    let arn = arn_of(
        &ok(
            router,
            "CreateProcurementPortalPreference",
            &minimal_create(portal, &buyer, &supplier),
        )
        .await,
    );
    let subject = Subject {
        arn,
        portal,
        buyer,
        supplier,
    };
    ok(
        router,
        "PutProcurementPortalPreference",
        &accepted_update(&subject),
    )
    .await;
    subject
}

/// RULE: a call the service refuses BEFORE it writes anything leaves every member a reader
/// reports — `Version` and `LastUpdateDate` included — exactly as the last write left them,
/// however many times that call is repeated.
///
/// The rows are the refusal classes this plane answers for a preference that exists: the
/// status update's five (its empty request, a reason without its status, a status outside
/// the modelled enum, and the two kinds of move a caller may not ask for), the
/// configuration update's, and a conflicting create (a 409, so the rule is not read as
/// being about 400 alone), spread across the portals `ProcurementPortalName` admits.
#[tokio::test]
async fn a_refused_call_leaves_the_preference_where_the_last_successful_write_left_it() {
    let router = emulator();

    for (index, refusal) in refusal_classes().iter().enumerate() {
        let subject = subject_at_version_two(&router, index, refusal.portal).await;
        let before = get_preference(&router, &subject.arn).await;
        let before_summary = listed_summary(&router, &subject.arn).await;
        assert_eq!(
            version(&before),
            2,
            "case {}: the create and one update leave the second version, got {before}",
            refusal.label
        );

        // Repeated, because the defect this pins showed up as a counter that drifted one
        // step per refused call: three refusals are three times nothing, not three writes.
        for round in 1..=3 {
            let request = (refusal.request)(&subject);
            let (status, body) = call(&router, refusal.operation, &request).await;
            assert_eq!(
                status, refusal.http_status,
                "case {} round {round}: expected HTTP {}, got {status} {body}",
                refusal.label, refusal.http_status
            );
            assert_eq!(
                error_code(&body),
                refusal.error_code,
                "case {} round {round}: expected the modelled {}, got {body}",
                refusal.label,
                refusal.error_code
            );

            let after = get_preference(&router, &subject.arn).await;
            assert_eq!(
                after, before,
                "case {} round {round}: a refused call must leave every reported member as \
                 it was, got {after}",
                refusal.label
            );
            let after_summary = listed_summary(&router, &subject.arn).await;
            assert_eq!(
                after_summary, before_summary,
                "case {} round {round}: the listing must report the same summary, got \
                 {after_summary}",
                refusal.label
            );
        }
    }
}

/// One step of a scripted sequence against one preference.
struct Step {
    label: &'static str,
    operation: &'static str,
    request: fn(&Subject) -> Value,
    /// Whether this call SUCCEEDS at all — which is not the same thing as writing the
    /// resource: a send that reports the delivery it could not make, and a refused
    /// submission, both write.
    succeeds: bool,
    /// Whether this call WRITES the preference — which is exactly when the reported
    /// version advances.
    writes: bool,
}

/// A status update that asks the e-invoice plane for the move a caller may make out of the
/// validated status, so it SUCCEEDS.
fn accepted_status_update(subject: &Subject) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": subject.arn,
        "EinvoiceDeliveryPreferenceStatus": STATUS_TEST_INITIALIZED,
    })
}

/// The steps a sequence is built from, each named once here so a row is just an order.
fn step(label: &'static str) -> Step {
    match label {
        "refused send" => Step {
            label,
            operation: "SendProcurementPortalValidation",
            request: send_validation,
            // The send reports the delivery it could not make, and the code it ISSUED on
            // the way is a write of the preference.
            succeeds: false,
            writes: true,
        },
        "refused send with a token" => Step {
            label,
            operation: "SendProcurementPortalValidation",
            request: send_validation_with_token,
            succeeds: false,
            writes: true,
        },
        "refused verify" => Step {
            label,
            operation: "VerifyProcurementPortalValidation",
            request: verify_unissued_code,
            succeeds: false,
            // The refusal RECORDS the attempt it reports against the preference, which is
            // a write of it.
            writes: true,
        },
        "refused status update" => Step {
            label,
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: status_update_stating_nothing,
            succeeds: false,
            writes: false,
        },
        "accepted update" => Step {
            label,
            operation: "PutProcurementPortalPreference",
            request: accepted_update,
            succeeds: true,
            writes: true,
        },
        "accepted status update" => Step {
            label,
            operation: "UpdateProcurementPortalPreferenceStatus",
            request: accepted_status_update,
            succeeds: true,
            writes: true,
        },
        other => panic!("case {other}: no such step"),
    }
}

/// RULE: the reported `Version` counts the calls that WROTE the preference — one per
/// applied write, whatever that write touched — and `LastUpdateDate` moves only with them,
/// so interleaving calls the service refuses before writing anything, in any order, cannot
/// shift either.
///
/// The rows are orders in which those kinds of call arrive: refusals before the
/// configuration writes, refusals between them, and a sequence with no configuration write
/// at all — the validation calls among them, which write the preference's validation state
/// without writing its configuration.
#[tokio::test]
async fn the_version_counts_the_applied_writes_only() {
    let (router, engine) = emulator_with_engine();

    let sequences: Vec<(&str, Vec<&str>)> = vec![
        (
            "refusals before the writes",
            vec![
                "refused send",
                "refused verify",
                "refused status update",
                "accepted update",
                "accepted status update",
            ],
        ),
        (
            "refusals between the writes",
            vec![
                "accepted update",
                "refused send",
                "accepted status update",
                "refused verify",
                "refused send with a token",
                "accepted update",
            ],
        ),
        (
            "nothing but refusals",
            vec![
                "refused send",
                "refused send",
                "refused verify",
                "refused status update",
            ],
        ),
    ];

    for (index, (label, sequence)) in sequences.iter().enumerate() {
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let buyer = format!("BUYER-SEQUENCE-{index}");
        let supplier = format!("SUPPLIER-SEQUENCE-{index}");
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(portal, &buyer, &supplier),
            )
            .await,
        );
        let subject = Subject {
            arn,
            portal,
            buyer,
            supplier,
        };
        // A completed validation first, because the move a scripted status update asks for
        // is one a caller may only make out of the validated status. Those two calls write
        // the preference as well, which is what the baseline below reads.
        validate_preference(&router, &engine, &subject.arn).await;

        let mut reported = get_preference(&router, &subject.arn).await;
        let mut writes = version(&reported);
        assert_eq!(
            writes, 3,
            "case {label}: the create, the issued code and the completed validation are \
             three writes, got {reported}"
        );

        for step_label in sequence {
            let step = step(step_label);
            let previous_last_update = last_update(&reported);
            let request = (step.request)(&subject);
            let (status, body) = call(&router, step.operation, &request).await;
            if step.writes {
                writes += 1;
            }
            if step.succeeds {
                assert_eq!(
                    status, 200,
                    "case {label} / {}: expected HTTP 200, got {status} {body}",
                    step.label
                );
            } else {
                assert!(
                    status >= 400,
                    "case {label} / {}: expected a refusal, got {status} {body}",
                    step.label
                );
            }

            reported = get_preference(&router, &subject.arn).await;
            assert_eq!(
                version(&reported),
                writes,
                "case {label} / {}: expected Version {writes} after {writes} applied \
                 writes, got {reported}",
                step.label
            );
            if step.writes {
                assert!(
                    last_update(&reported) >= previous_last_update,
                    "case {label} / {}: an applied write stamps the instant it changed \
                     at, got {reported}",
                    step.label
                );
            } else {
                assert_eq!(
                    last_update(&reported),
                    previous_last_update,
                    "case {label} / {}: a call that wrote nothing must not restamp \
                     LastUpdateDate, got {reported}",
                    step.label
                );
            }
            // The listing reports the same resource, so it reports the same version.
            let summary = listed_summary(&router, &subject.arn).await;
            assert_eq!(
                version(&summary),
                writes,
                "case {label} / {}: the listing reports the version the read does, got \
                 {summary}",
                step.label
            );
        }
    }
}

/// Make a validation code outstanding for `arn`, through the call that ISSUES one.
///
/// `SendProcurementPortalValidation` is that call — the model documents it as initiating
/// the validation by issuing a validation code — so the code is issued over the wire and
/// then READ from state: the modelled response carries only the ARN, because the real
/// service delivers the code through the configured procurement portal.
///
/// The call itself REFUSES: this emulator delivers no test e-invoice to a procurement
/// portal, so the send reports the delivery it could not make (the sentence naming the
/// delivery target when the preference configures one, and [`DELIVERY_NOT_CONFIRMED`] when
/// it configures none) while the code it issued stays outstanding — which is what the
/// refusal's own remedy rests on, and what this helper hands back.
pub(crate) async fn issue_code(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> String {
    let (status, body) = call(
        router,
        "SendProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case issue a code for {arn}: the send reports the delivery it could not make, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case issue a code for {arn}: expected the modelled ValidationException, got {body}"
    );
    let text = error_text(&body);
    assert!(
        text == DELIVERY_NOT_CONFIRMED || text.starts_with("We were unable to deliver"),
        "case issue a code for {arn}: expected one of this error class's sentences, got {text:?}"
    );
    outstanding_code(engine, arn).unwrap_or_else(|| {
        panic!("case issue a code: the send must leave a code outstanding for {arn}")
    })
}

/// The validation code this service issued for `arn` and has not seen back yet, read from
/// state because no modelled response carries it.
pub(crate) fn outstanding_code(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<String> {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .get(arn)
        .and_then(|stored| stored.validation.as_ref())
        .filter(|validation| validation.verified_at.is_none())
        .map(|validation| validation.code.clone())
}

/// `code` with every digit shifted by one — a copy of an issued code that is wrong in
/// every position, so a refusal of it is not about the code's shape.
pub(crate) fn shifted(code: &str) -> String {
    code.chars()
        .map(|digit| {
            char::from_digit((digit.to_digit(10).unwrap_or_default() + 1) % 10, 10).unwrap_or('0')
        })
        .collect()
}

/// The whole sentence a refused submission of a validation code carries: it states that
/// the code is not valid and WHICH attempt failed out of the budget, and past the budget
/// it points at the call that issues a fresh code instead of inviting another try.
pub(crate) fn invalid_code_sentence(attempt: u32) -> String {
    let budget = VERIFICATION_ATTEMPT_BUDGET;
    let tail = if attempt < budget {
        "Please try again."
    } else {
        "Request a new validation code with SendProcurementPortalValidation."
    };
    format!("The code you entered is invalid. Attempt {attempt} of {budget} failed. {tail}")
}

/// A configuration update that SUCCEEDS, naming its contact after `round` so successive
/// writes are distinguishable.
pub(crate) fn accepted_update_round(arn: &str, round: i64) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": contact(&format!("Updated Contact {round}")),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    })
}

/// The text of a failure body, under either key an error of this service may travel with.
pub(crate) fn error_text(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the validation-flow table: a preference reached through a chosen number of
/// successful writes, with or without an outstanding code, and the wrong code submitted
/// against it.
struct RejectedCode {
    label: &'static str,
    portal: &'static str,
    /// How many SUCCESSFUL configuration writes follow the create, which is what the
    /// reported version counts on top of it.
    writes_after_create: i64,
    /// Whether those writes are status updates rather than configuration updates, so both
    /// writing operations are exercised as the thing the version counts.
    write_with_status_update: bool,
    /// Whether a code is outstanding when the wrong code is submitted.
    issue_a_code: bool,
    /// The wrong code, built from the issued one when there is one.
    wrong_code: fn(Option<&str>) -> String,
}

/// Each row's wrong code, named once here so a row is just a combination.
fn a_shifted_copy_of_the_issued_code(issued: Option<&str>) -> String {
    issued.map(shifted).unwrap_or_else(|| "000000".to_string())
}

fn a_longer_code(_issued: Option<&str>) -> String {
    "12345678".to_string()
}

fn a_non_digit_code(_issued: Option<&str>) -> String {
    "not-a-code".to_string()
}

fn a_one_character_code(_issued: Option<&str>) -> String {
    "7".to_string()
}

/// The rows: both outstanding-code states, at several version counts, across the portals
/// the enum admits and the forms a submitted code can be wrong in.
fn rejected_code_rows() -> Vec<RejectedCode> {
    vec![
        RejectedCode {
            label: "no code outstanding, straight after the create",
            portal: portal_at(0),
            writes_after_create: 0,
            write_with_status_update: false,
            issue_a_code: false,
            wrong_code: a_shifted_copy_of_the_issued_code,
        },
        RejectedCode {
            label: "no code outstanding, after one configuration update",
            portal: portal_at(1),
            writes_after_create: 1,
            write_with_status_update: false,
            issue_a_code: false,
            wrong_code: a_non_digit_code,
        },
        RejectedCode {
            label: "a code outstanding, straight after the create",
            portal: portal_at(2),
            writes_after_create: 0,
            write_with_status_update: false,
            issue_a_code: true,
            wrong_code: a_shifted_copy_of_the_issued_code,
        },
        RejectedCode {
            label: "a code outstanding, after one status update",
            portal: portal_at(3),
            writes_after_create: 1,
            write_with_status_update: true,
            issue_a_code: true,
            wrong_code: a_longer_code,
        },
        RejectedCode {
            label: "a code outstanding, after two configuration updates",
            portal: portal_at(0),
            writes_after_create: 2,
            write_with_status_update: false,
            issue_a_code: true,
            wrong_code: a_one_character_code,
        },
        RejectedCode {
            label: "no code outstanding, after two configuration updates",
            portal: portal_at(2),
            writes_after_create: 2,
            write_with_status_update: false,
            issue_a_code: false,
            wrong_code: a_longer_code,
        },
    ]
}

/// RULE (the same one, over the state the validation flow keeps): a submitted validation
/// code that is REFUSED is RECORDED as the attempt its refusal reports, which is a write of
/// the preference — so `Version` advances by exactly one per refused submission and
/// `LastUpdateDate` is restamped, while every CONFIGURATION member a reader reports stays
/// exactly where the last configuration write left it.
///
/// The rows add the dimension the table above cannot reach: whether a code really is
/// outstanding when the submitted one is wrong, which is the OTP retry a client makes.
#[tokio::test]
async fn a_rejected_validation_code_records_the_attempt_it_reports() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in rejected_code_rows().iter().enumerate() {
        let buyer = format!("BUYER-REJECTED-{index}");
        let supplier = format!("SUPPLIER-REJECTED-{index}");
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(row.portal, &buyer, &supplier),
            )
            .await,
        );
        // Every write this row performs on top of the create, counted as it is made,
        // because the version under test is one per applied write.
        let mut writes = 1;

        // Reach the version this row submits its wrong code at.
        for round in 0..row.writes_after_create {
            if row.write_with_status_update {
                // A status update may only ask for a move the lifecycle provides, so the
                // plane is carried to the validated status first — two writes of its own.
                validate_preference(&router, &engine, &arn).await;
                writes += 2;
                ok(
                    &router,
                    "UpdateProcurementPortalPreferenceStatus",
                    &json!({
                        "ProcurementPortalPreferenceArn": arn,
                        "EinvoiceDeliveryPreferenceStatus": STATUS_TEST_INITIALIZED,
                    }),
                )
                .await;
            } else {
                ok(
                    &router,
                    "PutProcurementPortalPreference",
                    &accepted_update_round(&arn, round),
                )
                .await;
            }
            writes += 1;
        }

        let issued = if row.issue_a_code {
            let code = issue_code(&router, &engine, &arn).await;
            writes += 1;
            Some(code)
        } else {
            None
        };
        let before = get_preference(&router, &arn).await;
        let before_summary = listed_summary(&router, &arn).await;
        assert_eq!(
            version(&before),
            writes,
            "case {}: the create and the {} writes after it leave Version {writes}, got \
             {before}",
            row.label,
            writes - 1
        );

        let wrong = (row.wrong_code)(issued.as_deref());
        assert_ne!(
            Some(wrong.as_str()),
            issued.as_deref(),
            "case {}: the row's code has to be the wrong one, got {wrong:?}",
            row.label
        );

        // Repeated past the attempt budget, so the rule holds across it: each refusal
        // records one attempt, so each is one version of the resource and no more.
        for attempt in 1..=REPEATED_SUBMISSIONS {
            let (status, body) = call(
                &router,
                "VerifyProcurementPortalValidation",
                &json!({"ProcurementPortalPreferenceArn": arn, "Code": wrong}),
            )
            .await;
            writes += 1;
            assert_eq!(
                status, 400,
                "case {} attempt {attempt} with {wrong:?}: expected HTTP 400, got {status} {body}"
                ,
                row.label
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {} attempt {attempt} with {wrong:?}: expected the modelled \
                 ValidationException, got {body}",
                row.label
            );
            // The count the refusal states is what the submission moved, and recording it
            // is the write a reader sees.
            assert_eq!(
                error_text(&body),
                invalid_code_sentence(attempt),
                "case {} attempt {attempt} with {wrong:?}: the refusal states which \
                 attempt failed out of the budget, got {body}",
                row.label
            );

            let after = get_preference(&router, &arn).await;
            assert_eq!(
                without_write_markers(&after),
                without_write_markers(&before),
                "case {} attempt {attempt} with {wrong:?}: a refused code must leave every \
                 configuration member as it was, got {after}",
                row.label
            );
            assert_eq!(
                version(&after),
                writes,
                "case {} attempt {attempt} with {wrong:?}: the recorded attempt is one \
                 version of the resource, so Version is {writes}, got {after}",
                row.label
            );
            assert!(
                last_update(&after) >= last_update(&before),
                "case {} attempt {attempt} with {wrong:?}: recording the attempt stamps \
                 the instant it happened at, got {after}",
                row.label
            );
            let after_summary = listed_summary(&router, &arn).await;
            assert_eq!(
                without_write_markers(&after_summary),
                without_write_markers(&before_summary),
                "case {} attempt {attempt} with {wrong:?}: the listing must report the same \
                 summary, got {after_summary}",
                row.label
            );
            assert_eq!(
                version(&after_summary),
                writes,
                "case {} attempt {attempt} with {wrong:?}: the listing reports the version \
                 the read does, got {after_summary}",
                row.label
            );
            assert_eq!(
                outstanding_code(&engine, &arn),
                issued,
                "case {} attempt {attempt} with {wrong:?}: a refused code neither issues \
                 nor retires one",
                row.label
            );
        }

        // The configuration the refusals left untouched is still writable, and that write
        // advances the version by exactly one more — so the rule is one version per write,
        // not "this resource stopped versioning".
        ok(
            &router,
            "PutProcurementPortalPreference",
            &accepted_update_round(&arn, row.writes_after_create + 1),
        )
        .await;
        writes += 1;
        let after_write = get_preference(&router, &arn).await;
        assert_eq!(
            version(&after_write),
            writes,
            "case {}: the configuration write after the refusals advances Version by \
             exactly one, got {after_write}",
            row.label
        );
        assert!(
            last_update(&after_write) >= last_update(&before),
            "case {}: that write stamps the instant it changed at, got {after_write}",
            row.label
        );
    }
}
