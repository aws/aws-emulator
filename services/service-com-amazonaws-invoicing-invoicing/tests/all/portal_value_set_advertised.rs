// SPDX-License-Identifier: CC0-1.0

//! The VALUE SET a refused `ProcurementPortalName` advertises, and that it is the set a
//! create accepts — driven end to end over this service's wire (`POST /` with
//! `X-Amz-Target: Invoicing.<OperationName>` and an awsJson body) through the same
//! `MultiServiceRouter` the production router mounts.
//!
//! RULE under test: **the enum value set this service holds a request to is the set it
//! ACCEPTS: the refusal of a value outside the set advertises every value a create takes,
//! and takes every value it advertises — so the set a caller is told about and the set the
//! service serves cannot differ. The refusal is HTTP 400 `ValidationException` in AWS's
//! constraint envelope: fixed text around the wire (lowerCamelCase) member name taken from
//! the request and the comma-separated accepted values.**
//!
//! Nothing here writes the portals out: the suite READS the advertised set out of the
//! refusal and then drives a create and a read-back for each value of it, so the assertion
//! is the correspondence between the two sets rather than a value list that has to be
//! maintained beside the model. That correspondence is what a change to the accepted set
//! has to keep — the recorded refusal of this operation advertises every portal it serves,
//! so a set that gains a value gains it on both sides at once.
//!
//! The rows span what the rule generalises over: the FORM of the rejected value (a modelled
//! value in another case, a name of a portal product nothing declares, a value with the
//! right shape and the wrong name, the empty string) and, for the accepted half, every
//! portal the set advertises.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// The member a refusal of this input names, as the wire spells it: the lowerCamelCase
/// form of the modelled member name, which is what AWS's constraint envelope states.
const WIRE_MEMBER: &str = "procurementPortalName";

/// The code a request refused for a value outside a modelled set carries
/// (`smithy_query(query="operation",operation="CreateProcurementPortalPreference")`:
/// `ValidationException`, `@httpError 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A router carrying only this service, over one shared engine.
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

/// One awsJson request, returning the HTTP status and the parsed body.
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

/// A `CreateProcurementPortalPreference` request for one portal and identity: every member
/// the model marks required, well formed, so what a row turns on is the portal name.
fn create_request(portal: &str, label: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": format!("{label}-buyer"),
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": format!("{label}-supplier"),
        "EinvoiceDeliveryEnabled": false,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": json!([{
            "Name": "Portal Administrator",
            "Email": format!("{label}@example.com"),
        }]),
    })
}

/// The text a refusal of this plane carries.
fn refusal_text(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The envelope a request carrying one value outside a modelled set is refused with, with
/// the accepted values left for the caller to read: fixed text, then the member, then the
/// set in brackets.
fn one_value_outside_a_set(member: &str) -> (String, String) {
    (
        format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must satisfy enum value set: ["
        ),
        "]".to_string(),
    )
}

/// The values a refusal advertises, in the order it advertises them — read out of the
/// message, which is the only place a caller learns them.
fn advertised(case: &str, message: &str) -> Vec<String> {
    let (opening, closing) = one_value_outside_a_set(WIRE_MEMBER);
    let listed = message
        .strip_prefix(opening.as_str())
        .and_then(|rest| rest.strip_suffix(closing.as_str()))
        .unwrap_or_else(|| {
            panic!("case {case}: expected this service's value-set envelope, got {message:?}")
        });
    listed.split(", ").map(str::to_string).collect()
}

#[tokio::test]
async fn the_set_a_refusal_advertises_is_the_set_a_create_accepts() {
    let router = emulator();

    // Rows: the forms a value outside the set takes — a modelled value in another case, a
    // product name the service declares no portal for, a name shaped like a modelled one,
    // and the empty string. Every one of them is refused with the SAME advertised set, so
    // what the message states is the service's set rather than anything about the value.
    let mut sets: Vec<Vec<String>> = Vec::new();
    for (index, rejected) in ["coupa", "NOT_A_REAL_PORTAL", "SAP_BUSINESS_NETWORK_TEST", ""]
        .into_iter()
        .enumerate()
    {
        let case = format!("ProcurementPortalName={rejected:?}");
        let (status, body) = call(
            &router,
            "CreateProcurementPortalPreference",
            &create_request(rejected, &format!("outside-{index}")),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a value outside the set, got {status} {body}"
        );
        assert_eq!(
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {body}"
        );
        sets.push(advertised(&case, &refusal_text(&body)));
    }
    let first = sets.first().cloned().unwrap_or_default();
    for (index, set) in sets.iter().enumerate() {
        assert_eq!(
            set, &first,
            "case {index}: every refusal of this member advertises the same set, got {set:?}"
        );
    }
    assert!(
        !first.is_empty(),
        "expected the refusal to advertise the values it accepts, got none"
    );

    // Every value the refusal advertises is one a create ACCEPTS, and the created
    // preference reports the portal it was created for — so the advertised set is the
    // served set and not a superset of it.
    for (index, portal) in first.iter().enumerate() {
        let label = format!("advertised-{index}");
        let (status, created) = call(
            &router,
            "CreateProcurementPortalPreference",
            &create_request(portal, &label),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {portal}: a portal the refusal advertises is one a create accepts, got \
             {status} {created}"
        );
        let arn = created
            .get("ProcurementPortalPreferenceArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {portal}: expected a minted ARN in {created}"))
            .to_string();
        let (status, read) = call(
            &router,
            "GetProcurementPortalPreference",
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {portal}: expected the created preference to read back, got {status} {read}"
        );
        assert_eq!(
            read["ProcurementPortalPreference"]
                .get("ProcurementPortalName")
                .and_then(Value::as_str),
            Some(portal.as_str()),
            "case {portal}: expected the preference to report the portal it was created \
             for, got {read}"
        );
    }

    // And the set the generated value-set constant holds — what every other suite of this
    // crate reads its rows from — is the SAME set, so no layer of this service admits a
    // portal the refusal does not advertise or refuses one it does.
    let mut advertised_sorted = first.clone();
    advertised_sorted.sort_unstable();
    let mut declared: Vec<String> = PROCUREMENT_PORTAL_NAME_VALUES
        .iter()
        .map(|portal| (*portal).to_string())
        .collect();
    declared.sort_unstable();
    assert_eq!(
        declared, advertised_sorted,
        "expected the declared value set to be the advertised one, got {declared:?}"
    );
}
