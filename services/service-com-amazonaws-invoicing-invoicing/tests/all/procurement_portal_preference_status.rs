// SPDX-License-Identifier: CC0-1.0

//! `UpdateProcurementPortalPreferenceStatus` driven end to end over this service's wire
//! protocol, every write proved by reading the preference back through
//! `GetProcurementPortalPreference` and `ListProcurementPortalPreferences`.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body.
//! Nothing reaches into the state struct, so what these tests see is exactly what an SDK
//! client sees.
//!
//! The rules under test, each spanned by a table rather than one request:
//!
//! * a plane whose status the request STATES takes that status, and its status reason
//!   becomes the reason stated alongside it — absent when the request states none;
//! * a plane whose status the request does NOT state keeps the status and reason the last
//!   write left;
//! * the write advances `Version` by one and moves `LastUpdateDate`, leaving the identity
//!   members, `CreateDate` and the configuration alone;
//! * a status outside the modelled enum, a request that states no status at all, and a
//!   status reason stated without its status are each refused, and a refused request
//!   writes nothing;
//! * an ARN naming no stored preference is the modelled not-found;
//! * a `ClientToken` replay applies the update once.
//!
//! Every status a row states is one a CALLER may ask a plane to move to from where that
//! plane stands: the statuses the service assigns itself are refused, which the table in
//! `procurement_portal_preference_status_machine` spans.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    emulator_with_engine, validate_preference, PORTAL_INSTANCE_ENDPOINT, PORTAL_NAMES,
};

/// The statuses the planes of a preference hold once its validation is complete and the
/// e-invoice plane has been carried into its connection test — the distinct starting point
/// [`preference_with_known_statuses`] leaves, and the moves a caller may ask for out of it
/// (`smithy_query(query="shape", shape="ProcurementPortalPreferenceStatus")` lists all
/// seven values; the other four are the service's to assign).
const VALIDATED: &str = "VALIDATED";
const TEST_INITIALIZED: &str = "TEST_INITIALIZED";
const ACTIVE: &str = "ACTIVE";

/// The members of a stored preference no status update can change: its identity, when it
/// was created, and the configuration members — none of them is a member of this
/// operation's input (`smithy_query(query="operation",
/// operation="UpdateProcurementPortalPreferenceStatus")`).
const UNTOUCHED_MEMBERS: &[&str] = &[
    "AwsAccountId",
    "ProcurementPortalPreferenceArn",
    "ProcurementPortalName",
    "BuyerDomain",
    "BuyerIdentifier",
    "SupplierDomain",
    "SupplierIdentifier",
    "CreateDate",
    "Contacts",
    "EinvoiceDeliveryEnabled",
    "PurchaseOrderRetrievalEnabled",
    "Selector",
    "ProcurementPortalInstanceEndpoint",
    "TestEnvPreference",
];

/// The status members of each plane, as the model spells them: the status, its reason, and
/// a label for the case.
const PLANES: &[(&str, &str, &str)] = &[
    (
        "e-invoice delivery",
        "EinvoiceDeliveryPreferenceStatus",
        "EinvoiceDeliveryPreferenceStatusReason",
    ),
    (
        "purchase order retrieval",
        "PurchaseOrderRetrievalPreferenceStatus",
        "PurchaseOrderRetrievalPreferenceStatusReason",
    ),
];

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services. The engine is handed back for the
/// one thing no wire carries: the validation code the portal flow issues.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    emulator_with_engine()
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
async fn listed_summary(router: &MultiServiceRouter, arn: &str) -> Value {
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

/// Create a preference with both planes enabled, and bring each plane to a DISTINCT status
/// through the moves a caller may make, so "the plane the request did not state is
/// unchanged" is distinguishable from "it happens to hold the same status".
///
/// The validation flow is what carries the planes off the unverified status, and the
/// e-invoice plane is then carried one move further; both reasons are stated so a later
/// update is shown to replace them.
async fn preference_with_known_statuses(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    index: usize,
    tag: &str,
) -> String {
    let create = json!({
        "ProcurementPortalName": PORTAL_NAMES[index % PORTAL_NAMES.len()],
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": format!("BUYER-{tag}-{index}"),
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": format!("SUPPLIER-{tag}-{index}"),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "ProcurementPortalInstanceEndpoint": PORTAL_INSTANCE_ENDPOINT,
        "Contacts": contact("Status Contact"),
    });
    let arn = arn_of(&ok(router, "CreateProcurementPortalPreference", &create).await);
    validate_preference(router, engine, &arn).await;
    ok(
        router,
        "UpdateProcurementPortalPreferenceStatus",
        &json!({
            "ProcurementPortalPreferenceArn": arn,
            "EinvoiceDeliveryPreferenceStatus": TEST_INITIALIZED,
            "EinvoiceDeliveryPreferenceStatusReason": "the starting e-invoice reason",
            "PurchaseOrderRetrievalPreferenceStatus": VALIDATED,
            "PurchaseOrderRetrievalPreferenceStatusReason": "the starting purchase-order reason",
        }),
    )
    .await;
    arn
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

/// `LastUpdateDate` of a reported preference, as the epoch seconds the wire carries.
fn last_update(preference: &Value) -> f64 {
    preference
        .get("LastUpdateDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a LastUpdateDate in {preference}"))
}

/// RULE: a status update writes the status of each plane whose status the request states,
/// together with the reason stated alongside it (absent when none is stated), leaves a
/// plane it does not state exactly as it was, and advances the resource's `Version` by one
/// while leaving its identity, `CreateDate` and configuration alone.
///
/// The rows differ along which plane the request states, whether it states a reason, and
/// which status it moves to — the status drawn from the moves a caller may ask for out of
/// the status each plane holds, including restating the status it already holds, so the
/// rule is asserted over that whole set rather than one recorded value. The reasons span
/// `BasicString`'s own bounds: the empty string and 1024 characters, its
/// `@length(min: 0, max: 1024)`.
#[tokio::test]
async fn a_status_update_writes_the_planes_it_states_and_only_those() {
    let (router, engine) = emulator();
    let longest_reason = "r".repeat(1024);

    // (label, the e-invoice status/reason stated, the purchase-order status/reason stated)
    type Stated = Option<(&'static str, Option<String>)>;
    let rows: Vec<(&str, Stated, Stated)> = vec![
        (
            "the e-invoice plane only, with a reason",
            Some((ACTIVE, Some("activated by the portal".to_string()))),
            None,
        ),
        (
            "the e-invoice plane only, with no reason",
            Some((TEST_INITIALIZED, None)),
            None,
        ),
        (
            "the purchase-order plane only, with a reason",
            None,
            Some((
                TEST_INITIALIZED,
                Some("the test was initialized".to_string()),
            )),
        ),
        (
            "the purchase-order plane only, with no reason",
            None,
            Some((VALIDATED, None)),
        ),
        (
            "both planes, both reasons",
            Some((ACTIVE, Some("activated by the portal".to_string()))),
            Some((
                TEST_INITIALIZED,
                Some("the test was initialized".to_string()),
            )),
        ),
        (
            "both planes, only the purchase-order reason",
            Some((TEST_INITIALIZED, None)),
            Some((VALIDATED, Some("validated".to_string()))),
        ),
        (
            "both planes, no reason at all",
            Some((ACTIVE, None)),
            Some((TEST_INITIALIZED, None)),
        ),
        (
            "the empty reason, BasicString's minimum length",
            Some((ACTIVE, Some(String::new()))),
            None,
        ),
        (
            "the longest reason BasicString admits",
            None,
            Some((TEST_INITIALIZED, Some(longest_reason.clone()))),
        ),
    ];

    for (index, (label, einvoice, purchase_order)) in rows.iter().enumerate() {
        let arn = preference_with_known_statuses(&router, &engine, index, "WRITE").await;
        let before = get_preference(&router, &arn).await;

        let mut request = json!({"ProcurementPortalPreferenceArn": arn});
        for (stated, (_, status_member, reason_member)) in
            [einvoice, purchase_order].into_iter().zip(PLANES)
        {
            if let Some((status, reason)) = stated {
                request[*status_member] = json!(status);
                if let Some(reason) = reason {
                    request[*reason_member] = json!(reason);
                }
            }
        }

        let answered = ok(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &request,
        )
        .await;
        assert_eq!(
            arn_of(&answered),
            arn,
            "case {label}: the update answers with the ARN it acted on, got {answered}"
        );

        let after = get_preference(&router, &arn).await;
        for (stated, (plane, status_member, reason_member)) in
            [einvoice, purchase_order].into_iter().zip(PLANES)
        {
            match stated {
                Some((status, reason)) => {
                    assert_eq!(
                        member(&after, status_member),
                        json!(status),
                        "case {label}: the {plane} plane takes the stated status, got {after}"
                    );
                    let expected_reason = reason
                        .as_ref()
                        .map_or(Value::Null, |reason| json!(reason));
                    assert_eq!(
                        member(&after, reason_member),
                        expected_reason,
                        "case {label}: the {plane} plane reports the reason stated with the \
                         update, got {after}"
                    );
                }
                None => {
                    for name in [status_member, reason_member] {
                        assert_eq!(
                            member(&after, name),
                            member(&before, name),
                            "case {label}: an update that does not state the {plane} plane \
                             leaves {name} alone, got {after}"
                        );
                    }
                }
            }
        }

        assert_eq!(
            version(&after),
            version(&before) + 1,
            "case {label}: the write advances Version by one, got {after}"
        );
        assert!(
            last_update(&after) >= last_update(&before),
            "case {label}: the write stamps LastUpdateDate at or after the previous write, \
             got {after}"
        );
        for name in UNTOUCHED_MEMBERS {
            assert_eq!(
                member(&after, name),
                member(&before, name),
                "case {label}: a status update must not change {name}, got {after}"
            );
        }

        // The listing reports the same one copy of the resource the reader does.
        let summary = listed_summary(&router, &arn).await;
        for (_, status_member, reason_member) in PLANES {
            for name in [status_member, reason_member] {
                assert_eq!(
                    member(&summary, name),
                    member(&after, name),
                    "case {label}: the listing and the read agree on {name}, got {summary}"
                );
            }
        }
        assert_eq!(
            summary.get("Version").and_then(Value::as_i64),
            Some(version(&after)),
            "case {label}: the listing reports the version the read does, got {summary}"
        );
    }
}

/// RULE: a plane's status reason is the context stated with the update that set its
/// current status, so an update that states the status without a reason leaves the plane
/// with no reason rather than with the previous status's one.
///
/// One row per plane crossed with the statuses that plane may be asked to take from where
/// it stands — the move the lifecycle provides and the status it already holds — each
/// applied twice: first with a reason, then without, so the clearing is asserted across
/// that set and on both planes.
#[tokio::test]
async fn a_status_update_without_a_reason_leaves_the_plane_without_one() {
    let (router, engine) = emulator();

    // Each plane with the statuses a caller may state for it out of the status
    // `preference_with_known_statuses` leaves it holding.
    let rows: &[(usize, &str)] = &[
        (0, TEST_INITIALIZED),
        (0, ACTIVE),
        (1, VALIDATED),
        (1, TEST_INITIALIZED),
    ];

    for (index, (plane_index, status)) in rows.iter().enumerate() {
        let arn = preference_with_known_statuses(&router, &engine, index, "CLEAR").await;
        let (plane, status_member, reason_member) = PLANES[*plane_index];

        let with_reason = json!({
            "ProcurementPortalPreferenceArn": arn,
            status_member: status,
            reason_member: format!("moved to {status}"),
        });
        ok(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &with_reason,
        )
        .await;
        let reported = get_preference(&router, &arn).await;
        assert_eq!(
            member(&reported, reason_member),
            json!(format!("moved to {status}")),
            "case {plane} {status}: the stated reason is reported, got {reported}"
        );

        let without_reason = json!({
            "ProcurementPortalPreferenceArn": arn,
            status_member: status,
        });
        ok(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &without_reason,
        )
        .await;
        let reported = get_preference(&router, &arn).await;
        assert_eq!(
            member(&reported, reason_member),
            Value::Null,
            "case {plane} {status}: an update with no reason leaves the plane without \
             one, got {reported}"
        );
        assert_eq!(
            member(&reported, status_member),
            json!(status),
            "case {plane} {status}: the plane still holds the stated status, got {reported}"
        );
    }
}

/// RULE: a request that states no plane's status asks for no status update, and a status
/// reason stated without that plane's status gives context for an update that is not
/// happening: each is refused with the modelled `ValidationException`, and a refused
/// request writes nothing.
#[tokio::test]
async fn a_request_that_states_no_status_update_is_refused() {
    let (router, engine) = emulator();

    let rows: Vec<(&str, Value)> = vec![
        ("no member but the ARN", json!({})),
        ("a client token only", json!({"ClientToken": "no-status"})),
        (
            "the e-invoice reason without its status",
            json!({"EinvoiceDeliveryPreferenceStatusReason": "a reason for nothing"}),
        ),
        (
            "the purchase-order reason without its status",
            json!({"PurchaseOrderRetrievalPreferenceStatusReason": "a reason for nothing"}),
        ),
        (
            "both reasons, neither status",
            json!({
                "EinvoiceDeliveryPreferenceStatusReason": "a reason for nothing",
                "PurchaseOrderRetrievalPreferenceStatusReason": "a reason for nothing",
            }),
        ),
        (
            "one plane stated, the OTHER plane's reason without its status",
            json!({
                "EinvoiceDeliveryPreferenceStatus": ACTIVE,
                "PurchaseOrderRetrievalPreferenceStatusReason": "a reason for nothing",
            }),
        ),
    ];

    for (index, (label, body)) in rows.iter().enumerate() {
        let arn = preference_with_known_statuses(&router, &engine, index, "REFUSE").await;
        let before = get_preference(&router, &arn).await;

        let mut request = body.clone();
        request["ProcurementPortalPreferenceArn"] = json!(arn);
        let (status, response) = call(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &request,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label}: expected the modelled validation error, got {response}"
        );
        let message = error_message(&response);
        assert!(
            !message.is_empty(),
            "case {label}: the refusal states why, got {response}"
        );

        let after = get_preference(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {label}: a refused request writes nothing, got {after}"
        );
    }
}

/// RULE: which statuses a plane may hold is the modelled enum's value set, so a value
/// outside it is refused and writes nothing.
///
/// The rows are values the enum does not admit, including the right value in the wrong
/// case and the empty string, tested on both planes.
#[tokio::test]
async fn a_status_outside_the_modelled_enum_is_refused() {
    let (router, engine) = emulator();

    for (index, status) in ["active", "Validated", "NOT_A_STATUS", ""]
        .iter()
        .enumerate()
    {
        let arn = preference_with_known_statuses(&router, &engine, index, "ENUM").await;
        let before = get_preference(&router, &arn).await;

        for (plane, status_member, _) in PLANES {
            let request = json!({
                "ProcurementPortalPreferenceArn": arn,
                *status_member: status,
            });
            let (http_status, response) = call(
                &router,
                "UpdateProcurementPortalPreferenceStatus",
                &request,
            )
            .await;
            assert_eq!(
                http_status, 400,
                "case {plane} {status:?}: expected HTTP 400, got {http_status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {plane} {status:?}: expected the modelled validation error, got {response}"
            );
            let after = get_preference(&router, &arn).await;
            assert_eq!(
                after, before,
                "case {plane} {status:?}: a refused request writes nothing, got {after}"
            );
        }
    }
}

/// RULE: an ARN naming no stored preference is the modelled `ResourceNotFoundException`,
/// whatever the request states.
///
/// The rows differ along the resource id the ARN names — its shortest and longest forms
/// under `ProcurementPortalPreferenceArnString`'s `@pattern` — and along which planes the
/// request states.
#[tokio::test]
async fn an_unknown_arn_is_the_modelled_not_found() {
    let (router, _engine) = emulator();

    for id in [
        "does-not-exist",
        "a",
        "0123456789ab",
        "A-Very-Long-Preference-Id-0123456789",
    ] {
        let arn = engine::emulator_arn(
            "invoicing",
            "",
            &format!("procurement-portal-preference/{id}"),
        );
        for update in [
            json!({"EinvoiceDeliveryPreferenceStatus": "ACTIVE"}),
            json!({"PurchaseOrderRetrievalPreferenceStatus": "SUSPENDED",
                   "PurchaseOrderRetrievalPreferenceStatusReason": "gone"}),
            json!({"EinvoiceDeliveryPreferenceStatus": "VALIDATED",
                   "PurchaseOrderRetrievalPreferenceStatus": "VALIDATED"}),
            // A request that would be refused for stating no status is still reported
            // against a resource that does not exist.
            json!({}),
        ] {
            let mut request = update.clone();
            request["ProcurementPortalPreferenceArn"] = json!(arn);
            let (status, body) = call(
                &router,
                "UpdateProcurementPortalPreferenceStatus",
                &request,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {id} {update}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {id} {update}: expected the modelled not-found error, got {body}"
            );
        }
    }
}

/// RULE: this operation is `@idempotent` — repeating a request under the same
/// `ClientToken` against the same preference applies the update ONCE and answers with the
/// ARN the first call returned; without a token each call is its own write.
///
/// The rows span the token forms `BasicStringWithoutSpace` admits (`@length(min: 0, max:
/// 1024)`, `@pattern ^\S+$`), including none at all and the longest one.
#[tokio::test]
async fn a_client_token_replay_applies_the_update_once() {
    let (router, engine) = emulator();
    let longest = "t".repeat(1024);

    for (index, token) in [
        None,
        Some("t"),
        Some("a-client-token"),
        Some("9f2b-4c1d"),
        Some(longest.as_str()),
    ]
    .into_iter()
    .enumerate()
    {
        let arn = preference_with_known_statuses(&router, &engine, index, "IDEM").await;
        let before = get_preference(&router, &arn).await;

        let mut request = json!({
            "ProcurementPortalPreferenceArn": arn,
            "EinvoiceDeliveryPreferenceStatus": ACTIVE,
            "EinvoiceDeliveryPreferenceStatusReason": "replayed",
        });
        if let Some(token) = token {
            request["ClientToken"] = json!(token);
        }

        let rounds = 3;
        for round in 1..=rounds {
            let answered = ok(
                &router,
                "UpdateProcurementPortalPreferenceStatus",
                &request,
            )
            .await;
            assert_eq!(
                arn_of(&answered),
                arn,
                "case token {index} round {round}: the update answers with the same ARN, got \
                 {answered}"
            );
            let reported = get_preference(&router, &arn).await;
            assert_eq!(
                member(&reported, "EinvoiceDeliveryPreferenceStatus"),
                json!(ACTIVE),
                "case token {index} round {round}: the stated status is applied, got {reported}"
            );
            // A token makes every repeat a REPLAY of the first write, so the resource took
            // exactly one write; without one each call writes again.
            let expected = match token {
                Some(_) => version(&before) + 1,
                None => version(&before) + round,
            };
            assert_eq!(
                version(&reported),
                expected,
                "case token {index} round {round}: expected Version {expected}, got {reported}"
            );
        }

        // A DIFFERENT token is a different request, which writes again — stating the move
        // the purchase-order plane may make from where it stands.
        let mut other = request.clone();
        other["ClientToken"] = json!(format!("other-{index}"));
        other["PurchaseOrderRetrievalPreferenceStatus"] = json!(TEST_INITIALIZED);
        ok(&router, "UpdateProcurementPortalPreferenceStatus", &other).await;
        let reported = get_preference(&router, &arn).await;
        assert_eq!(
            member(&reported, "PurchaseOrderRetrievalPreferenceStatus"),
            json!(TEST_INITIALIZED),
            "case token {index}: a request under a new token is applied, got {reported}"
        );
    }
}
