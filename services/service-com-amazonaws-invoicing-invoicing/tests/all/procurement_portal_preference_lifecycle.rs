// SPDX-License-Identifier: CC0-1.0

//! The WRITE side of the procurement-portal-preference plane —
//! `PutProcurementPortalPreference`, `DeleteProcurementPortalPreference` and
//! `SendProcurementPortalValidation` — driven end to end over this service's wire
//! protocol, each write proved by reading the resource back through
//! `GetProcurementPortalPreference` and `ListProcurementPortalPreferences`.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body.
//! Nothing reaches into the state struct, so what these tests see is exactly what an SDK
//! client sees.
//!
//! The rules under test, each spanned by a table rather than one recorded request:
//!
//! * an update replaces every configuration member it states, leaves the identity and
//!   `CreateDate` alone, and advances `Version` by one with `LastUpdateDate` at the
//!   instant it changed — asserted over rows differing in which members they state and
//!   over several successive updates of one preference;
//! * a configuration member the update omits keeps its stored value, except
//!   `MarketplacePunchOutEnabled`, the one member the model documents a default for, and
//!   `MarketplacePunchOutPreference`, which the model documents as present only while that
//!   flag is true — asserted with one row per optional member, omitted in turn;
//! * once a delete succeeds the resource is gone: every reader answers the modelled
//!   not-found, the listing omits it, its tags go with it and the identity it held is
//!   free again — asserted over several portals and identities;
//! * `SendProcurementPortalValidation` ISSUES a validation code for the preference and
//!   answers with its ARN when the service can confirm the delivery of the test e-invoice
//!   carrying it, and the modelled 400 when it cannot, writing none of the preference's
//!   configuration either way — asserted over preferences differing in which portal
//!   endpoints they configure, and over the codes a caller that never received the issued
//!   one could submit;
//! * repeating an update lands the same configuration, a repeat under one `ClientToken` is
//!   replayed rather than applied again, and a delete's `ClientToken` replay answers with
//!   the ARN it removed.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

// The members every applied write of this resource advances, stripped in ONE place for
// every suite of this plane that compares a configuration across such a write.
use crate::refused_procurement_portal_writes::{
    error_text, portal_without_punch_out_requirement, without_write_markers, PORTAL_NAMES,
};

/// The status each enabled plane of a preference carries until a validation moves it: one
/// of the values `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The members of a stored preference that identify it, which
/// `PutProcurementPortalPreference`'s input does not carry — so no update can change
/// them (`smithy_query(query="operation", operation="PutProcurementPortalPreference")`).
const IDENTITY_MEMBERS: &[&str] = &[
    "AwsAccountId",
    "ProcurementPortalPreferenceArn",
    "ProcurementPortalName",
    "BuyerDomain",
    "BuyerIdentifier",
    "SupplierDomain",
    "SupplierIdentifier",
    "CreateDate",
];

/// Members an update states that the echo check compares by hand instead of by whole
/// value: `EinvoiceDeliveryPreference` carries a timestamp, whose request form (a JSON
/// number) and response form need not be the same JSON literal.
const NOT_COMPARED_WHOLE: &[&str] = &[
    "ProcurementPortalPreferenceArn",
    "ClientToken",
    "EinvoiceDeliveryPreference",
];

/// The configuration member the model documents as "present when
/// `MarketplacePunchOutEnabled` is `true`" (`smithy_query(query="shape",
/// shape="ProcurementPortalPreference")`), so it follows that flag rather than the
/// keep-what-the-update-omits rule the other optional members follow.
const PUNCH_OUT_CONFIG_MEMBER: &str = "MarketplacePunchOutPreference";

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

/// A create request for the portal and buyer/supplier pair that identify a preference,
/// carrying only the members the model marks required.
fn minimal_create(portal: &str, buyer: &str, supplier: &str) -> Value {
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

/// Every ARN `ListProcurementPortalPreferences` reports, in one page.
async fn listed_arns(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListProcurementPortalPreferences", &json!({})).await;
    body.get("ProcurementPortalPreferences")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ProcurementPortalPreferences in {body}"))
        .iter()
        .filter_map(|summary| {
            summary
                .get("ProcurementPortalPreferenceArn")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// A `PutProcurementPortalPreference` request: `update` with the ARN it acts on.
fn put_request(arn: &str, update: &Value) -> Value {
    let mut request = update.clone();
    request["ProcurementPortalPreferenceArn"] = json!(arn);
    request
}

/// Assert that the preference now reports every member `stated` states.
fn assert_states_were_applied(label: &str, stated: &Value, reported: &Value) {
    let stated = stated
        .as_object()
        .unwrap_or_else(|| panic!("case {label}: the update is an object"));
    for (member, value) in stated {
        if NOT_COMPARED_WHOLE.contains(&member.as_str()) {
            continue;
        }
        assert_eq!(
            reported.get(member),
            Some(value),
            "case {label}: expected {member} to be the stated {value}, got {:?}",
            reported.get(member)
        );
    }
}

/// RULE: a successful update of a configuration resource replaces every configuration
/// member the request states, leaves the resource's identity and creation metadata
/// untouched, and advances the resource's version counter with the last-update instant —
/// so the next read reports the submitted configuration at the next version.
///
/// The rows differ along which members the update states (the required ones alone, each
/// optional structure, the endpoint and the shared secret, both enable flags in each
/// combination), and every row is applied TWICE in a row, so the version counter is
/// asserted advancing rather than merely differing from one.
#[tokio::test]
async fn a_put_applies_what_it_states_and_advances_the_version() {
    let router = emulator();

    let updates: Vec<(&str, Value)> = vec![
        (
            "the required members alone, both planes off",
            json!({
                "Contacts": contact("Updated Contact"),
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
            }),
        ),
        (
            "purchase-order retrieval only, with the portal endpoint and secret",
            json!({
                "Contacts": contact("Second Contact"),
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": true,
                "ProcurementPortalInstanceEndpoint": "https://portal.example.com/invoices",
                "ProcurementPortalSharedSecret": "s3cret-value",
            }),
        ),
        (
            "both planes on, with the selector and the test environment",
            json!({
                "Contacts": contact("Third Contact"),
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": true,
                "Selector": {"SellerOfRecords": ["AMAZON_EU_SARL"]},
                "TestEnvPreference": {
                    "BuyerDomain": "NetworkID",
                    "BuyerIdentifier": "AN0TEST000001",
                    "SupplierDomain": "NetworkID",
                    "SupplierIdentifier": "AN0TEST000002",
                    "ProcurementPortalInstanceEndpoint": "https://test.portal.example.com/invoices",
                },
            }),
        ),
        (
            "e-invoice delivery on, with its delivery preference",
            json!({
                "Contacts": contact("Fourth Contact"),
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": false,
                "EinvoiceDeliveryPreference": {
                    // The values each enum member admits: `smithy_query(query="shape",
                    // shape="EinvoiceDeliveryPreference")` and the enums it targets.
                    "EinvoiceDeliveryDocumentTypes": ["AWS_CLOUD_INVOICE"],
                    "Protocol": "CXML",
                    "PurchaseOrderDataSources": [
                        {"EinvoiceDeliveryDocumentType": "AWS_CLOUD_INVOICE",
                         "PurchaseOrderDataSourceType": "ASSOCIATED_PURCHASE_ORDER_REQUIRED"}
                    ],
                    "ConnectionTestingMethod": "PROD_ENV_DOLLAR_TEST",
                    "EinvoiceDeliveryActivationDate": 1_767_225_600,
                },
            }),
        ),
        (
            "Marketplace PunchOut enabled with the preference Coupa requires",
            json!({
                "Contacts": contact("Fifth Contact"),
                "EinvoiceDeliveryEnabled": true,
                "PurchaseOrderRetrievalEnabled": true,
                "MarketplacePunchOutEnabled": true,
                "MarketplacePunchOutPreference": {
                    "ApprovalRequestRedirectUrl": "https://example.com/approve"
                },
            }),
        ),
    ];

    for (index, (label, update)) in updates.iter().enumerate() {
        // One identity per row, so a conflicting create never stands in for an update.
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let created = ok(
            &router,
            "CreateProcurementPortalPreference",
            &minimal_create(
                portal,
                &format!("BUYER-PUT-{index}"),
                &format!("SUPPLIER-PUT-{index}"),
            ),
        )
        .await;
        let arn = arn_of(&created);
        let created_state = get_preference(&router, &arn).await;

        let mut previous = created_state.clone();
        for round in 1..=2 {
            let answered = ok(&router, "PutProcurementPortalPreference", &put_request(&arn, update))
                .await;
            assert_eq!(
                arn_of(&answered),
                arn,
                "case {label} round {round}: the update answers with the ARN it updated, got {answered}"
            );

            let reported = get_preference(&router, &arn).await;
            assert_states_were_applied(&format!("{label} round {round}"), update, &reported);
            if update.get("EinvoiceDeliveryPreference").is_some() {
                assert_eq!(
                    reported
                        .get("EinvoiceDeliveryPreference")
                        .and_then(|delivery| delivery.get("Protocol")),
                    update
                        .get("EinvoiceDeliveryPreference")
                        .and_then(|delivery| delivery.get("Protocol")),
                    "case {label} round {round}: expected the stated delivery protocol, got {reported}"
                );
            }

            let expected_version = previous
                .get("Version")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                + 1;
            assert_eq!(
                reported.get("Version").and_then(Value::as_i64),
                Some(expected_version),
                "case {label} round {round}: expected Version {expected_version}, got {:?}",
                reported.get("Version")
            );
            for member in IDENTITY_MEMBERS {
                assert_eq!(
                    reported.get(*member),
                    created_state.get(*member),
                    "case {label} round {round}: {member} must be what the create left, got {:?}",
                    reported.get(*member)
                );
            }
            let created_at = created_state
                .get("CreateDate")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let updated_at = reported
                .get("LastUpdateDate")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            assert!(
                updated_at >= created_at,
                "case {label} round {round}: expected LastUpdateDate at or after CreateDate \
                 {created_at}, got {updated_at}"
            );

            // A plane the update leaves enabled still carries the status the validation
            // calls move; a status is never invented for a plane that has none.
            for (enabled, status) in [
                (
                    "EinvoiceDeliveryEnabled",
                    "EinvoiceDeliveryPreferenceStatus",
                ),
                (
                    "PurchaseOrderRetrievalEnabled",
                    "PurchaseOrderRetrievalPreferenceStatus",
                ),
            ] {
                if reported.get(enabled).and_then(Value::as_bool) == Some(true) {
                    assert_eq!(
                        reported.get(status).and_then(Value::as_str),
                        Some(PENDING_VERIFICATION),
                        "case {label} round {round}: an enabled plane carries a verification \
                         status, got {reported}"
                    );
                }
            }

            previous = reported;
        }
    }
}

/// RULE: a configuration member an update does not state keeps the value the resource
/// already holds — except a member the model documents a default for, which takes that
/// default (`MarketplacePunchOutEnabled`: "Defaults to false if not provided",
/// `smithy_query(query="operation", operation="PutProcurementPortalPreference")`), and a
/// member the model documents as present only while a flag is true, which goes with that
/// flag ([`PUNCH_OUT_CONFIG_MEMBER`]).
///
/// One row per optional member, omitted in turn while every other optional member is
/// stated with a NEW value — so each row asserts every part of the rule at once.
#[tokio::test]
async fn a_put_leaves_the_members_it_does_not_state_alone() {
    let router = emulator();

    let optional = json!({
        "Selector": {"SellerOfRecords": ["AMAZON_US_LLC"]},
        "ProcurementPortalSharedSecret": "updated-secret",
        "ProcurementPortalInstanceEndpoint": "https://updated.portal.example.com/invoices",
        "TestEnvPreference": {
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": "AN0UPDATED001",
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": "AN0UPDATED002",
            "ProcurementPortalInstanceEndpoint": "https://updated.test.example.com/invoices",
        },
        // On the host of the instance endpoint this update states: a stored approval
        // redirect URL and the instance endpoint beside it name one host.
        "MarketplacePunchOutPreference": {
            "ApprovalRequestRedirectUrl": "https://updated.portal.example.com/updated",
        },
    });
    let omittable: Vec<String> = optional
        .as_object()
        .expect("the optional members are an object")
        .keys()
        .cloned()
        .collect();

    for (index, omitted) in omittable.iter().enumerate() {
        // A portal other than Coupa, so Marketplace PunchOut may be enabled without the
        // preference Coupa requires and this row can omit that preference.
        let mut create = minimal_create(
            portal_without_punch_out_requirement(),
            &format!("BUYER-KEEP-{index}"),
            &format!("SUPPLIER-KEEP-{index}"),
        );
        create["MarketplacePunchOutEnabled"] = json!(true);
        for (member, value) in optional.as_object().expect("an object") {
            create[member] = match member.as_str() {
                // The create's own values, so "unchanged" is distinguishable from
                // "restated".
                "Selector" => json!({"SellerOfRecords": ["AMAZON_EU_SARL"]}),
                "ProcurementPortalSharedSecret" => json!("created-secret"),
                "ProcurementPortalInstanceEndpoint" => {
                    json!("https://created.portal.example.com/invoices")
                }
                "TestEnvPreference" => json!({
                    "BuyerDomain": "NetworkID",
                    "BuyerIdentifier": "AN0CREATED001",
                    "SupplierDomain": "NetworkID",
                    "SupplierIdentifier": "AN0CREATED002",
                    "ProcurementPortalInstanceEndpoint": "https://created.test.example.com/invoices",
                }),
                // The create's own redirect URL, on the host of the endpoint this create
                // states: the two endpoints a preference holds name one host.
                "MarketplacePunchOutPreference" => json!({
                    "ApprovalRequestRedirectUrl": "https://created.portal.example.com/approve",
                }),
                _ => value.clone(),
            };
        }
        let created = ok(&router, "CreateProcurementPortalPreference", &create).await;
        let arn = arn_of(&created);
        let before = get_preference(&router, &arn).await;

        let mut update = json!({
            "Contacts": contact("Kept Contact"),
            "EinvoiceDeliveryEnabled": true,
            "PurchaseOrderRetrievalEnabled": true,
        });
        for (member, value) in optional.as_object().expect("an object") {
            if member == omitted {
                continue;
            }
            update[member] = value.clone();
        }
        ok(
            &router,
            "PutProcurementPortalPreference",
            &put_request(&arn, &update),
        )
        .await;
        let after = get_preference(&router, &arn).await;

        // The member this row omits keeps the value the create stored — unless it is the
        // one the model ties to a flag these updates leave false, which the assertions
        // below cover instead.
        if omitted != PUNCH_OUT_CONFIG_MEMBER {
            assert_eq!(
                after.get(omitted.as_str()),
                before.get(omitted.as_str()),
                "case omitting {omitted}: an unstated member keeps its stored value, got {:?}",
                after.get(omitted.as_str())
            );
        }
        // The flag-dependent member is asserted below rather than echoed back: a row that
        // STATES it states it for a feature these updates leave disabled.
        let mut echoed = update.clone();
        if let Some(members) = echoed.as_object_mut() {
            members.remove(PUNCH_OUT_CONFIG_MEMBER);
        }
        assert_states_were_applied(&format!("omitting {omitted}"), &echoed, &after);
        // `MarketplacePunchOutEnabled` is stated by none of these updates, and the model
        // documents it as defaulting to false when it is not provided.
        assert_eq!(
            after.get("MarketplacePunchOutEnabled").and_then(Value::as_bool),
            Some(false),
            "case omitting {omitted}: the documented default applies, got {:?}",
            after.get("MarketplacePunchOutEnabled")
        );
        // That default leaves Marketplace PunchOut DISABLED, and its configuration is
        // documented as present only while the feature is enabled — so every row reports
        // it alike, whether it stated one or omitted it: not at all.
        assert_eq!(
            after.get(PUNCH_OUT_CONFIG_MEMBER),
            None,
            "case omitting {omitted}: a disabled feature reports no configuration, got {:?}",
            after.get(PUNCH_OUT_CONFIG_MEMBER)
        );
        // `MarketplacePunchOutEnabled` is stated by none of these updates, and the model
        // documents it as defaulting to false when it is not provided.
        assert_eq!(
            after.get("MarketplacePunchOutEnabled").and_then(Value::as_bool),
            Some(false),
            "case omitting {omitted}: the documented default applies, got {:?}",
            after.get("MarketplacePunchOutEnabled")
        );
    }
}

/// RULE: a write that names a resource by ARN resolves it against state, and an ARN that
/// names nothing is the modelled `ResourceNotFoundException` (HTTP 404) — it neither
/// succeeds nor creates state for a resource that does not exist.
///
/// The rows are well-formed preference ARNs of different id shapes, so nothing here is
/// keyed to one recorded ARN.
#[tokio::test]
async fn a_write_against_an_arn_that_names_no_preference_is_the_modelled_404() {
    let router = emulator();

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
        let update = json!({
            "Contacts": contact("Nobody"),
            "EinvoiceDeliveryEnabled": true,
            "PurchaseOrderRetrievalEnabled": false,
        });
        for (operation, request) in [
            (
                "PutProcurementPortalPreference",
                put_request(&arn, &update),
            ),
            (
                "DeleteProcurementPortalPreference",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {operation} {arn}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation} {arn}: expected the modelled not-found error, got {body}"
            );
        }
        assert!(
            listed_arns(&router).await.is_empty(),
            "case {arn}: a refused write must not create a preference"
        );
    }
}

/// RULE: once a delete of a resource succeeds the resource is GONE — every reader answers
/// the modelled not-found for its ARN, the listing omits it, the tags that were on it go
/// with it, a second delete reports it missing, and the identity it held is free for a new
/// create.
///
/// The rows are one preference per declared portal, so the rule is asserted over the whole
/// value set of the member that identifies a portal rather than one recorded portal.
#[tokio::test]
async fn a_delete_removes_the_preference_from_every_reader() {
    let router = emulator();

    for (index, portal) in PORTAL_NAMES.iter().enumerate() {
        let buyer = format!("BUYER-DEL-{index}");
        let supplier = format!("SUPPLIER-DEL-{index}");
        let mut create = minimal_create(portal, &buyer, &supplier);
        create["ResourceTags"] = json!([{"Key": "team", "Value": "finance"}]);
        let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &create).await);

        // The resource exists before the delete: a reader reports it and its tags.
        let before = get_preference(&router, &arn).await;
        assert_eq!(
            before
                .get("ProcurementPortalPreferenceArn")
                .and_then(Value::as_str),
            Some(arn.as_str()),
            "case {portal}: the created preference reads back, got {before}"
        );
        let tags = ok(
            &router,
            "ListTagsForResource",
            &json!({"ResourceArn": arn}),
        )
        .await;
        assert!(
            tags.get("ResourceTags").is_some(),
            "case {portal}: the created preference carries its tags, got {tags}"
        );

        let deleted = ok(
            &router,
            "DeleteProcurementPortalPreference",
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        assert_eq!(
            arn_of(&deleted),
            arn,
            "case {portal}: the delete answers with the ARN it removed, got {deleted}"
        );

        // Every reader of the plane, plus the tag plane and a second delete.
        let update = json!({
            "Contacts": contact("After Delete"),
            "EinvoiceDeliveryEnabled": true,
            "PurchaseOrderRetrievalEnabled": true,
        });
        for (operation, request) in [
            (
                "GetProcurementPortalPreference",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
            (
                "SendProcurementPortalValidation",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
            (
                "VerifyProcurementPortalValidation",
                json!({"ProcurementPortalPreferenceArn": arn, "Code": "000000"}),
            ),
            (
                "DeleteProcurementPortalPreference",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
            (
                "PutProcurementPortalPreference",
                put_request(&arn, &update),
            ),
            ("ListTagsForResource", json!({"ResourceArn": arn})),
            (
                "TagResource",
                json!({"ResourceArn": arn, "ResourceTags": [{"Key": "team", "Value": "ops"}]}),
            ),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {portal} {operation} after the delete: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {portal} {operation} after the delete: expected the modelled not-found \
                 error, got {body}"
            );
        }

        let listed = listed_arns(&router).await;
        assert!(
            !listed.contains(&arn),
            "case {portal}: a deleted preference is absent from the listing, got {listed:?}"
        );

        // The identity it held is free: the same portal and buyer/supplier pair creates
        // again, as a NEW resource.
        let recreated = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(portal, &buyer, &supplier),
            )
            .await,
        );
        assert_ne!(
            recreated, arn,
            "case {portal}: a create after the delete mints a new ARN, got {recreated}"
        );
        let fresh = get_preference(&router, &recreated).await;
        assert_eq!(
            fresh.get("Version").and_then(Value::as_i64),
            Some(1),
            "case {portal}: the re-created preference starts at the first version, got {fresh}"
        );
    }
}

/// RULE: the call that starts a validation ISSUES the validation code the flow runs on —
/// the model documents it as initiating the validation process by issuing a validation code
/// — and then reports the DELIVERY of the test e-invoice carrying that code, which this
/// emulator makes to no procurement portal: the call is refused with the modelled
/// `ValidationException`, naming the delivery target it could not reach for a preference
/// that configures the endpoint the document goes to and stating that the delivery was not
/// confirmed for one that configures none. The code never travels on the wire (the modelled
/// output carries only the ARN, because the real service delivers it through the portal),
/// issuing it writes no CONFIGURATION — only the `Version` and `LastUpdateDate` every
/// applied write of the resource advances — and a code the caller did not receive still
/// completes nothing.
///
/// The rows differ along which portal endpoints the preference configures (none, the portal
/// instance endpoint, the test environment's endpoint, both), because the endpoint is where
/// the real service delivers the code: with no connection-testing method stated, the test
/// e-invoice goes to the portal's own delivery endpoint, so that is the one the refusal
/// names.
#[tokio::test]
async fn a_send_issues_a_validation_code_and_reports_the_delivery_it_could_not_make() {
    let router = emulator();

    // One row: what the preference configures, and the delivery target the refusal names.
    struct SendRow {
        label: &'static str,
        endpoint: Option<&'static str>,
        test_endpoint: Option<&'static str>,
        target: Option<&'static str>,
    }
    let rows: Vec<SendRow> = vec![
        SendRow {
            label: "no endpoint configured",
            endpoint: None,
            test_endpoint: None,
            target: None,
        },
        SendRow {
            label: "a portal instance endpoint",
            endpoint: Some("https://portal.example.com/invoices"),
            test_endpoint: None,
            target: Some("https://portal.example.com/invoices"),
        },
        SendRow {
            label: "a test environment endpoint",
            endpoint: None,
            test_endpoint: Some("https://test.portal.example.com/invoices"),
            target: None,
        },
        SendRow {
            label: "both endpoints",
            endpoint: Some("https://portal.example.com/invoices"),
            test_endpoint: Some("https://test.portal.example.com/invoices"),
            target: Some("https://portal.example.com/invoices"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (label, endpoint, test_endpoint, target) =
            (row.label, row.endpoint, row.test_endpoint, row.target);
        let mut create = minimal_create(
            PORTAL_NAMES[index % PORTAL_NAMES.len()],
            &format!("BUYER-SEND-{index}"),
            &format!("SUPPLIER-SEND-{index}"),
        );
        if let Some(endpoint) = endpoint {
            create["ProcurementPortalInstanceEndpoint"] = json!(endpoint);
        }
        if let Some(test_endpoint) = test_endpoint {
            create["TestEnvPreference"] = json!({
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "AN0TEST000001",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "AN0TEST000002",
                "ProcurementPortalInstanceEndpoint": test_endpoint,
            });
        }
        let arn = arn_of(&ok(&router, "CreateProcurementPortalPreference", &create).await);
        let before = get_preference(&router, &arn).await;

        // The answer is the same on a first call, on a repeat, and on a repeat of the
        // `ClientToken`: a refused call records no answer for a later one to replay, so each
        // of the three ISSUES a code of its own and each one writes the resource.
        let mut issued_codes = 0;
        for attempt in ["no token", "with a token", "the token repeated"] {
            let mut request = json!({"ProcurementPortalPreferenceArn": arn});
            if attempt != "no token" {
                request["ClientToken"] = json!("send-token-1");
            }
            let (status, body) = call(&router, "SendProcurementPortalValidation", &request).await;
            issued_codes += 1;
            assert_eq!(
                status, 400,
                "case {label} ({attempt}): the send reports the delivery it could not make, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label} ({attempt}): expected the modelled validation error, got {body}"
            );
            // The refusal states what this emulator could not deliver: the target under the
            // configured endpoint when there is one, and the delivery-unconfirmed sentence
            // when the preference configures none.
            let names_a_target = error_text(&body).contains("unable to deliver");
            assert_eq!(
                names_a_target,
                target.is_some(),
                "case {label} ({attempt}): a configured endpoint is named as the delivery \
                 target and nothing else is, got {body}"
            );
            if let Some(target) = target {
                assert!(
                    error_text(&body).contains(&format!("{target}/cxml/invoices")),
                    "case {label} ({attempt}): the refusal names the delivery target \
                     {target}/cxml/invoices, got {body}"
                );
            }
        }

        // Issuing a code wrote no configuration: every configuration member stays where the
        // last configuration write left it and no plane's status moved — nothing has
        // verified the connection until a code comes back. What the issues DID advance is
        // the version every applied write of this resource advances.
        let after = get_preference(&router, &arn).await;
        let expected_version = before
            .get("Version")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            + issued_codes;
        assert_eq!(
            after.get("Version").and_then(Value::as_i64),
            Some(expected_version),
            "case {label}: expected Version {expected_version}, one per issued code, got \
             {after}"
        );
        assert_eq!(
            without_write_markers(&after),
            without_write_markers(&before),
            "case {label}: issuing a code must leave every configuration member as it was, \
             got {after}"
        );

        // A caller that never received the issued code completes nothing: these rows are
        // outside the six-decimal-digit form the issued code takes, so none of them can be
        // the outstanding one.
        for code in ["not-a-code", "1234567", "7", "00000a"] {
            let (status, body) = call(
                &router,
                "VerifyProcurementPortalValidation",
                &json!({"ProcurementPortalPreferenceArn": arn, "Code": code}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {label} verify {code}: a code the service did not issue completes \
                 nothing: got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label} verify {code}: expected the modelled validation error, got {body}"
            );
        }
    }
}

/// RULE: an update is idempotent in its EFFECT — repeating the same request leaves the
/// resource with exactly the configuration that request states — and a request that
/// carries a `ClientToken` is replayed: the repeat answers what the first call answered
/// and is applied ONCE, so the version counter, which counts the WRITES the resource took,
/// advances once for the whole retried request. Without a token there is nothing to
/// replay, so each call is its own write. A delete's `ClientToken` replay answers with the
/// ARN it removed rather than reporting it missing, and a delete of a preference that is
/// gone under any other token reports it missing.
///
/// The rows differ along the token's shape, inside `BasicStringWithoutSpace`'s
/// `@length(0..1024)` and `^\S+$` (`smithy_query(query="operation",
/// operation="PutProcurementPortalPreference")`), and one row carries no token at all.
#[tokio::test]
async fn a_repeated_update_lands_the_same_configuration_and_a_deletes_replay_answers_the_same() {
    let router = emulator();

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
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    PORTAL_NAMES[index % PORTAL_NAMES.len()],
                    &format!("BUYER-IDEM-{index}"),
                    &format!("SUPPLIER-IDEM-{index}"),
                ),
            )
            .await,
        );

        let mut update = put_request(
            &arn,
            &json!({
                "Contacts": contact("Replayed Contact"),
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": true,
            }),
        );
        if let Some(token) = token {
            update["ClientToken"] = json!(token);
        }
        let rounds = 3;
        for round in 1..=rounds {
            let answered = ok(&router, "PutProcurementPortalPreference", &update).await;
            assert_eq!(
                arn_of(&answered),
                arn,
                "case token {index} round {round}: the update answers with the same ARN, got {answered}"
            );
            let reported = get_preference(&router, &arn).await;
            assert_states_were_applied(&format!("token {index} round {round}"), &update, &reported);
            // The create left version 1. A token makes every repeat a replay of ONE
            // request, so exactly one write lands for the whole round; without a token
            // each call is its own write.
            let expected_version = match token {
                Some(_) => 2,
                None => 1 + round,
            };
            assert_eq!(
                reported.get("Version").and_then(Value::as_i64),
                Some(expected_version),
                "case token {index} round {round}: expected Version {expected_version}, got \
                 {reported}"
            );
        }

        let mut delete = json!({"ProcurementPortalPreferenceArn": arn});
        if let Some(token) = token {
            delete["ClientToken"] = json!(token);
        }
        let first = ok(&router, "DeleteProcurementPortalPreference", &delete).await;
        assert_eq!(
            arn_of(&first),
            arn,
            "case token {index}: the delete answers with the ARN it removed, got {first}"
        );
        for round in 2..=3 {
            let (status, body) = call(&router, "DeleteProcurementPortalPreference", &delete).await;
            match token {
                // A token makes the repeat a REPLAY of the call that removed it, which
                // answers the way that call did.
                Some(_) => {
                    assert_eq!(
                        status, 200,
                        "case token {index} delete round {round}: the replay answers 200, got \
                         {status} {body}"
                    );
                    assert_eq!(
                        arn_of(&body),
                        arn,
                        "case token {index} delete round {round}: the replay answers with the ARN \
                         it removed, got {body}"
                    );
                }
                // Without a token there is nothing to replay: the preference is gone.
                None => assert_eq!(
                    status, 404,
                    "case token {index} delete round {round}: expected HTTP 404, got {status} {body}"
                ),
            }
        }
        // A delete under a DIFFERENT token is a different request, so it reports the
        // preference missing rather than replaying the removal.
        let (status, body) = call(
            &router,
            "DeleteProcurementPortalPreference",
            &json!({"ProcurementPortalPreferenceArn": arn, "ClientToken": format!("other-{index}")}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case token {index}: a delete of a preference that is gone is a 404, got {status} {body}"
        );
    }
}
