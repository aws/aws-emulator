// SPDX-License-Identifier: CC0-1.0

//! The Marketplace PunchOut rule a write of a procurement portal preference applies,
//! driven end to end over this service's wire protocol through BOTH writes that apply it.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body. Nothing
//! reaches into the state struct, so what these tests see is exactly what an SDK client
//! sees.
//!
//! The rule: `MarketplacePunchOutEnabled` "defaults to false if not provided", and
//! `MarketplacePunchOutPreference` is REQUIRED while that flag is true — so a write that
//! enables Marketplace PunchOut without stating that configuration is refused with the
//! modelled `ValidationException`, and every other combination is accepted with the enable
//! flag the request states or the default it omits.
//!
//! How far that requirement reaches is the one part of the rule the two writes state
//! differently, each on its own input member: the CREATE states it for one portal
//! (`CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`: "Required
//! for Coupa when MarketplacePunchOutEnabled is true") and the UPDATE states it with no
//! portal qualification at all
//! (`PutProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`: "Required when
//! MarketplacePunchOutEnabled is true").
//!
//! `CreateProcurementPortalPreference` and `PutProcurementPortalPreference` are the two
//! writes that set a preference's Marketplace PunchOut configuration, so this table drives
//! EVERY row through both: the status, the error code and the worded text for a refusal,
//! and the flag a reader reports for an acceptance. The rows span the whole portal catalog
//! (not only the portal the create's requirement is stated for), the enable flag stated
//! true, stated false and omitted, and the configuration present and absent.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// The key every refusal of the procurement-portal-preference plane carries its
/// explanatory text under.
const PORTAL_PLANE_KEY: &str = "Message";

/// Every portal a preference can name, taken from the model's own value set
/// (`smithy_query(query="shape", shape="ProcurementPortalName")`).
const PORTALS: &[&str] = PROCUREMENT_PORTAL_NAME_VALUES;

/// The one portal the CREATE's own member states its configuration requirement for
/// (`CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`: "Required
/// for Coupa when MarketplacePunchOutEnabled is true").
const PORTAL_REQUIRING_PUNCH_OUT_CONFIG: &str = "COUPA";

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

/// The explanatory text of a refusal of this plane, if the body carries it.
fn message(body: &Value) -> Option<String> {
    body.get(PORTAL_PLANE_KEY)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The one contact a write carries (`Contacts` is modelled `@length(min: 1, max: 1)`).
fn contacts() -> Value {
    json!([{"Name": "Portal Administrator", "Email": "portal.admin@example.com"}])
}

/// A `MarketplacePunchOutPreference`, as its one member
/// (`MarketplacePunchOutPreference$ApprovalRequestRedirectUrl`, a `^\S+$` string) states it.
fn punch_out_config() -> Value {
    json!({"ApprovalRequestRedirectUrl": "https://example.com/approve"})
}

/// A `CreateProcurementPortalPreference` request for one portal and one identity, with
/// nothing optional.
fn create_request(portal: &str, identifier: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": format!("{identifier}-buyer"),
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": format!("{identifier}-supplier"),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": contacts(),
    })
}

/// A `PutProcurementPortalPreference` request for one preference, stating every member the
/// operation marks required and nothing of the Marketplace PunchOut configuration.
fn put_request(arn: &str) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": contacts(),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    })
}

/// `request` with the row's Marketplace PunchOut members stated on it.
fn with_punch_out(mut request: Value, enabled: Option<bool>, config: bool) -> Value {
    if let Some(enabled) = enabled {
        request["MarketplacePunchOutEnabled"] = json!(enabled);
    }
    if config {
        request["MarketplacePunchOutPreference"] = punch_out_config();
    }
    request
}

/// The `ProcurementPortalPreferenceArn` of a response that carries one.
fn arn_of(body: &Value) -> String {
    body.get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {body}"))
        .to_string()
}

/// The `MarketplacePunchOutEnabled` `GetProcurementPortalPreference` reports for `arn`.
async fn reported_punch_out_enabled(router: &MultiServiceRouter, arn: &str) -> Option<bool> {
    let read = ok(
        router,
        "GetProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    read.get("ProcurementPortalPreference")
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreference in {read}"))
        .get("MarketplacePunchOutEnabled")
        .and_then(Value::as_bool)
}

/// What one write answered, reduced to what the rule decides.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: u16,
    error_code: String,
    message: Option<String>,
}

/// One combination of the members the rule reads.
#[derive(Debug)]
struct Row {
    case: String,
    portal: &'static str,
    /// `MarketplacePunchOutEnabled` as the request states it, or `None` for a request that
    /// omits it — the case the documented default covers.
    enabled: Option<bool>,
    /// Whether the request states `MarketplacePunchOutPreference`.
    config: bool,
}

/// Every combination of portal, enable flag and configuration the rule reads: the whole
/// portal catalog, the flag stated true, stated false and omitted, and the configuration
/// present and absent.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for &portal in PORTALS {
        for enabled in [Some(true), Some(false), None] {
            for config in [false, true] {
                rows.push(Row {
                    case: format!("{portal}, enabled {enabled:?}, config stated {config}"),
                    portal,
                    enabled,
                    config,
                });
            }
        }
    }
    rows
}

impl Row {
    /// Whether the CREATE refuses this combination: Marketplace PunchOut enabled, without
    /// the configuration, for the one portal the create's own member names.
    fn create_refused(&self) -> bool {
        self.enabled == Some(true)
            && self.portal == PORTAL_REQUIRING_PUNCH_OUT_CONFIG
            && !self.config
    }

    /// Whether the UPDATE refuses this combination: Marketplace PunchOut enabled without
    /// the configuration, for every portal — its own member qualifies the requirement by
    /// no portal.
    fn put_refused(&self) -> bool {
        self.enabled == Some(true) && !self.config
    }

    /// The flag a reader must report once this combination is accepted: what the request
    /// states, or the documented default for one that omits it.
    fn stored_flag(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
}

/// The create and the configuration update answer one Marketplace PunchOut rule, each
/// within the scope its own input member states it for: the same default, the same
/// refusal and the same stored flag, and the requirement reaching every portal on the
/// update and the one portal the create names.
#[tokio::test]
async fn the_punch_out_rule_answers_the_same_from_the_create_and_the_update() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let case = &row.case;
        let expected_create_status = if row.create_refused() { 400 } else { 200 };
        let expected_put_status = if row.put_refused() { 400 } else { 200 };

        // The CREATE path states the portal on its own input. One identity per row, so a
        // conflict never stands in for the rule's answer.
        let created = with_punch_out(
            create_request(row.portal, &format!("create-{index}")),
            row.enabled,
            row.config,
        );
        let (create_status, create_body) =
            call(&router, "CreateProcurementPortalPreference", &created).await;
        let create_answer = Answer {
            status: create_status,
            error_code: error_code(&create_body),
            message: message(&create_body),
        };

        // The UPDATE path reads the portal off the STORED preference, so it needs one:
        // this baseline create states no Marketplace PunchOut members at all, which the
        // rule accepts for every portal.
        let baseline = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_request(row.portal, &format!("update-{index}")),
            )
            .await,
        );
        let updated = with_punch_out(put_request(&baseline), row.enabled, row.config);
        let (put_status, put_body) =
            call(&router, "PutProcurementPortalPreference", &updated).await;
        let put_answer = Answer {
            status: put_status,
            error_code: error_code(&put_body),
            message: message(&put_body),
        };

        assert_eq!(
            create_answer.status, expected_create_status,
            "case {case}: the create expected HTTP {expected_create_status}, got {:?}",
            create_answer
        );
        assert_eq!(
            put_answer.status, expected_put_status,
            "case {case}: the update expected HTTP {expected_put_status}, got {put_answer:?}"
        );

        if row.create_refused() {
            assert_eq!(
                create_answer.error_code, "ValidationException",
                "case {case}: expected a ValidationException, got {create_answer:?}"
            );
            let text = create_answer.message.clone().unwrap_or_default();
            assert!(
                text.contains(row.portal),
                "case {case}: the create's refusal must name the portal its requirement is \
                 stated for, got {text:?}"
            );
        }

        if row.put_refused() {
            assert_eq!(
                put_answer.error_code, "ValidationException",
                "case {case}: expected a ValidationException, got {put_answer:?}"
            );
            let text = put_answer.message.clone().unwrap_or_default();
            assert!(
                text.contains("MarketplacePunchOutPreference")
                    && text.contains("MarketplacePunchOutEnabled"),
                "case {case}: the update's refusal must name the two members whose \
                 combination it refuses, got {text:?}"
            );
            // The rule runs before anything is written: the refused update left the
            // baseline preference with the flag its create stored.
            assert_eq!(
                reported_punch_out_enabled(&router, &baseline).await,
                Some(false),
                "case {case}: a refused update must write nothing"
            );
        }

        // Accepted: a write that was accepted stored the flag the request states, or the
        // documented default for one that omits it.
        let expected_flag = row.stored_flag();
        let accepted = [
            ("the create", row.create_refused(), &create_body),
            ("the update", row.put_refused(), &put_body),
        ];
        for (path, refused, body) in accepted {
            if refused {
                continue;
            }
            let reported = reported_punch_out_enabled(&router, &arn_of(body)).await;
            assert_eq!(
                reported,
                Some(expected_flag),
                "case {case}: {path} must report MarketplacePunchOutEnabled \
                 {expected_flag}, got {reported:?}"
            );
        }
    }
}
