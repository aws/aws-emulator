// SPDX-License-Identifier: CC0-1.0

//! What a DELETE leaves behind for a point-in-time read, the per-resource tag quota,
//! the procurement-portal validation flow, the `ProcurementPortalName` value set and
//! the duplicate-`Name` refusal — driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0
//! body, so what these tests see is what an SDK client sees. The one exception is the
//! portal-validation code, which the real service delivers OUT OF BAND through the portal
//! the preference configures and the modelled responses never carry: the tests below have
//! `SendProcurementPortalValidation` issue it and then read it out of this service's
//! state, because there is no wire to carry it.
//!
//! The tables span the class each rule generalises over rather than one recorded
//! request: a unit's whole lifetime at several version counts, tagged and untagged; the
//! tag quota approached in several batch sizes; the portal-validation refusals over
//! several well-formed ARNs and codes; every value of the portal-name enum; and the
//! duplicate-name refusal over several name shapes.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::{
    ProcurementPortalPreference, PROCUREMENT_PORTAL_NAME_VALUES,
};
use service_com_amazonaws_invoicing_invoicing::state::{
    self, ComAmazonawsInvoicingInvoicingState, StoredProcurementPortalPreference,
};

/// The per-RESOURCE tag quota this service enforces: the AWS-wide tagging quota,
/// "Maximum number of tags per resource – 50" (AWS General Reference, "Tagging your
/// Amazon Web Services resources"). It is strictly below [`TAGS_PER_CALL_MAX`], the
/// per-CALL array maximum, which is what makes it reachable by a sequence of calls.
const TAG_QUOTA: usize = 50;

/// The per-CALL maximum of `ResourceTagList`: modelled `length 0..200`
/// (`smithy_query(query="operation", operation="TagResource")`). NOT the quota.
const TAGS_PER_CALL_MAX: usize = 200;

/// The quota has to be strictly below the per-call maximum, or no sequence of legal
/// calls could ever reach it — which is exactly the defect the rows below span.
const _: () = assert!(TAG_QUOTA < TAGS_PER_CALL_MAX);

/// Every value `ProcurementPortalName` accepts, taken from the model's own value set:
/// `smithy_query(query="shape", shape="ProcurementPortalName")`.
const PORTAL_NAMES: &[&str] = PROCUREMENT_PORTAL_NAME_VALUES;

/// The portal the row at `index` of a table routes to, wrapping round the value set: a
/// table may carry more rows than the model declares portals.
fn portal_at(index: usize) -> &'static str {
    PORTAL_NAMES[index % PORTAL_NAMES.len()]
}

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services. The engine is handed back for
/// the one thing no wire carries: the validation code the portal flow issues.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
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

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `message` of a failure body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `InvoiceUnitArn` of a response that carries one.
fn arn_of(body: &Value) -> String {
    body.get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {body}"))
        .to_string()
}

/// The `InvoiceUnitArn`s a `ListInvoiceUnits` page reports.
fn listed_arns(body: &Value) -> Vec<String> {
    body.get("InvoiceUnits")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an InvoiceUnits list in {body}"))
        .iter()
        .map(|unit| {
            unit.get("InvoiceUnitArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The tag keys `ListTagsForResource` reports.
fn listed_tag_keys(body: &Value) -> Vec<String> {
    body.get("ResourceTags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ResourceTags list in {body}"))
        .iter()
        .map(|tag| {
            tag.get("Key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Now, as the epoch-seconds number an awsJson1_0 timestamp is carried as.
fn epoch_seconds_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// The `LastModified` a get or a listing entry reports, as that same number.
fn last_modified(body: &Value) -> f64 {
    body.get("LastModified")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a LastModified in {body}"))
}

/// The `Description` a read reports, as the JSON it reports it as (absent reads `null`).
fn description_of(body: &Value) -> Value {
    body.get("Description").cloned().unwrap_or(Value::Null)
}

/// A step of the wall clock, so two writes of this service land on distinct instants and
/// an `AsOf` can name a point strictly between them.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

/// One invoice-unit lifetime: how the unit is created, how many times its configuration
/// is replaced before it is deleted, and whether it carried tags.
#[derive(Debug, Clone)]
struct Lifetime {
    name: &'static str,
    description: Option<&'static str>,
    tax_inheritance_disabled: bool,
    linked_accounts: &'static [&'static str],
    tags: &'static [(&'static str, &'static str)],
    updates: usize,
}

impl Lifetime {
    fn create_request(&self, index: usize) -> Value {
        let mut request = json!({
            "Name": self.name,
            "InvoiceReceiver": "123456789012",
            "TaxInheritanceDisabled": self.tax_inheritance_disabled,
            "Rule": {"LinkedAccounts": self.linked_accounts},
            "ClientToken": format!("create-{index}"),
        });
        if let Some(description) = self.description {
            request["Description"] = json!(description);
        }
        if !self.tags.is_empty() {
            request["ResourceTags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        request
    }
}

/// Rows differing along what a point-in-time read generalises over: how many
/// configurations the unit held before it was deleted (one, two, four), whether it
/// carried a `Description` at all, both values of `TaxInheritanceDisabled`, the rule's
/// account count, and tagged vs untagged.
const LIFETIMES: &[Lifetime] = &[
    Lifetime {
        name: "never-updated",
        description: Some("as created"),
        tax_inheritance_disabled: false,
        linked_accounts: &["210987654321"],
        tags: &[("env", "test")],
        updates: 0,
    },
    Lifetime {
        name: "updated once",
        description: None,
        tax_inheritance_disabled: true,
        linked_accounts: &["111111111111", "222222222222"],
        tags: &[],
        updates: 1,
    },
    Lifetime {
        name: "Ünïcödé_Einheit-3",
        description: Some(""),
        tax_inheritance_disabled: false,
        linked_accounts: &["333333333333"],
        tags: &[("owner", "finance"), ("env", "prod")],
        updates: 3,
    },
];

/// RULE: a read at an instant answers with the state at THAT instant — so a unit that
/// existed then is reported with the configuration it held then even after it has been
/// deleted, while a read of the state now (no `AsOf`) holds only the units that still
/// exist.
///
/// The table walks a whole lifetime per row and checks every instant of it: before the
/// create, at each configuration's own `LastModified`, between the last write and the
/// delete, and after the delete.
#[tokio::test]
async fn a_point_in_time_read_still_reports_a_deleted_unit() {
    let router = emulator();

    for (index, case) in LIFETIMES.iter().enumerate() {
        let before_creation = epoch_seconds_now();
        tick().await;

        let arn = arn_of(&ok(&router, "CreateInvoiceUnit", &case.create_request(index)).await);
        let created = ok(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
        // (the instant that configuration was written at, the Description it held)
        let mut versions = vec![(last_modified(&created), description_of(&created))];

        for revision in 0..case.updates {
            tick().await;
            ok(
                &router,
                "UpdateInvoiceUnit",
                &json!({
                    "InvoiceUnitArn": arn,
                    "Description": format!("revision {revision}"),
                    "ClientToken": format!("update-{index}-{revision}"),
                }),
            )
            .await;
            let read = ok(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
            versions.push((last_modified(&read), description_of(&read)));
        }

        tick().await;
        let mid_lifetime = epoch_seconds_now();
        tick().await;

        let deleted = ok(
            &router,
            "DeleteInvoiceUnit",
            &json!({"InvoiceUnitArn": arn, "ClientToken": format!("delete-{index}")}),
        )
        .await;
        assert_eq!(
            deleted.get("InvoiceUnitArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the delete must answer with the ARN it retired, got {deleted}"
        );
        tick().await;
        let after_deletion = epoch_seconds_now();

        // The state NOW no longer holds it: no `AsOf` at all, and an `AsOf` after the
        // delete, both report it gone.
        let (status, body) = call(&router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a get with no AsOf must report the deleted unit missing, got {status} {body}"
        );
        for (label, request) in [
            ("no AsOf", json!({"Filters": {"Names": [case.name]}})),
            (
                "AsOf after the delete",
                json!({"Filters": {"Names": [case.name]}, "AsOf": after_deletion}),
            ),
        ] {
            let listed = listed_arns(&ok(&router, "ListInvoiceUnits", &request).await);
            assert!(
                !listed.contains(&arn),
                "case {case:?} {label}: a deleted unit is not part of the state then, got {listed:?}"
            );
        }

        // Every instant the unit DID exist at still answers with it, and with the
        // configuration that was in effect at that instant.
        for (index, (instant, expected_description)) in versions.iter().enumerate() {
            let listed = ok(
                &router,
                "ListInvoiceUnits",
                &json!({"Filters": {"Names": [case.name]}, "AsOf": instant}),
            )
            .await;
            assert!(
                listed_arns(&listed).contains(&arn),
                "case {case:?} AsOf version {index}: a deleted unit must still appear in a \
                 pre-delete AsOf snapshot, got {listed}"
            );
            let reported = listed
                .get("InvoiceUnits")
                .and_then(Value::as_array)
                .and_then(|units| units.first())
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                description_of(&reported),
                *expected_description,
                "case {case:?} AsOf version {index}: the configuration in effect then is \
                 reported, got {reported}"
            );

            let got = ok(
                &router,
                "GetInvoiceUnit",
                &json!({"InvoiceUnitArn": arn, "AsOf": instant}),
            )
            .await;
            assert_eq!(
                description_of(&got),
                *expected_description,
                "case {case:?} get AsOf version {index}: the configuration in effect then is \
                 reported, got {got}"
            );
        }
        let between = ok(
            &router,
            "ListInvoiceUnits",
            &json!({"Filters": {"Names": [case.name]}, "AsOf": mid_lifetime}),
        )
        .await;
        assert!(
            listed_arns(&between).contains(&arn),
            "case {case:?}: a historical AsOf view between the last write and the delete must \
             still show the unit, got {between}"
        );

        // And the instant before it was created still answers with nothing: a retired
        // unit is history, not a unit that always existed.
        let (status, body) = call(
            &router,
            "GetInvoiceUnit",
            &json!({"InvoiceUnitArn": arn, "AsOf": before_creation}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: AsOf before the create must report nothing, got {status} {body}"
        );
        let listed = listed_arns(
            &ok(
                &router,
                "ListInvoiceUnits",
                &json!({"Filters": {"Names": [case.name]}, "AsOf": before_creation}),
            )
            .await,
        );
        assert!(
            listed.is_empty(),
            "case {case:?}: AsOf before the create must list nothing, got {listed:?}"
        );

        // The live surface treats it as gone: its tags went with it, a second delete is
        // a 404, and the `Name` it held is free again.
        for (operation, request) in [
            ("ListTagsForResource", json!({"ResourceArn": arn})),
            (
                "TagResource",
                json!({"ResourceArn": arn, "ResourceTags": [{"Key": "k", "Value": "v"}]}),
            ),
            (
                "UntagResource",
                json!({"ResourceArn": arn, "ResourceTagKeys": ["k"]}),
            ),
            (
                "UpdateInvoiceUnit",
                json!({"InvoiceUnitArn": arn, "Description": "after the delete"}),
            ),
            ("DeleteInvoiceUnit", json!({"InvoiceUnitArn": arn})),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case:?} {operation} on a retired unit: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} {operation} on a retired unit: expected the modelled 404, got {body}"
            );
        }

        // Recreating the name is allowed, and does not rewrite history: the pre-delete
        // snapshot still names the RETIRED unit, not the new one.
        let recreated = arn_of(
            &ok(
                &router,
                "CreateInvoiceUnit",
                &json!({
                    "Name": case.name,
                    "InvoiceReceiver": "123456789012",
                    "TaxInheritanceDisabled": false,
                    "Rule": {"LinkedAccounts": ["999999999999"]},
                    "ClientToken": format!("recreate-{index}"),
                }),
            )
            .await,
        );
        assert_ne!(
            recreated, arn,
            "case {case:?}: recreating a deleted name mints a new unit, got {recreated}"
        );
        let historical = listed_arns(
            &ok(
                &router,
                "ListInvoiceUnits",
                &json!({"Filters": {"Names": [case.name]}, "AsOf": versions[0].0}),
            )
            .await,
        );
        assert_eq!(
            historical,
            vec![arn.clone()],
            "case {case:?}: the pre-delete snapshot names the retired unit only, got {historical:?}"
        );

        // Leave the name free for nothing in particular: the row is done with it.
        ok(
            &router,
            "DeleteInvoiceUnit",
            &json!({"InvoiceUnitArn": recreated, "ClientToken": format!("cleanup-{index}")}),
        )
        .await;
    }
}

/// RULE: the per-resource tag quota is a limit on the tags ONE RESOURCE holds,
/// independent of and strictly below the per-call array maximum, and a write that would
/// push a resource past it is refused atomically with the modelled
/// `ServiceQuotaExceededException` — the identical write succeeding once an
/// `UntagResource` has freed the headroom.
///
/// The rows are the batch size the caller tags in, which is what decides where the
/// quota is met: one key at a time reaches it exactly, and the larger batches overshoot
/// it from different offsets.
#[tokio::test]
async fn the_per_resource_tag_quota_is_reachable_and_atomic() {
    let router = emulator();

    for batch in [1_usize, 7, 10, TAG_QUOTA] {
        let arn = arn_of(
            &ok(
                &router,
                "CreateInvoiceUnit",
                &json!({
                    "Name": format!("quota-batch-{batch}"),
                    "InvoiceReceiver": "123456789012",
                    "TaxInheritanceDisabled": false,
                    // An account is linked into at most one invoice unit, and each row
                    // leaves its unit in state: every row links its own account.
                    "Rule": {"LinkedAccounts": [format!("{:012}", 700 + batch)]},
                    "ClientToken": format!("quota-{batch}"),
                }),
            )
            .await,
        );

        let mut written = 0_usize;
        let mut refused = None;
        // The loop is bounded by the per-call maximum, as the recorded sequence is: a
        // quota that is never met inside it is the defect this row is here to catch.
        while written + batch <= TAGS_PER_CALL_MAX {
            let keys: Vec<String> = (written..written + batch)
                .map(|index| format!("key-{index}"))
                .collect();
            let request = json!({
                "ResourceArn": arn,
                "ResourceTags": keys
                    .iter()
                    .map(|key| json!({"Key": key, "Value": "v"}))
                    .collect::<Vec<Value>>(),
            });
            let (status, body) = call(&router, "TagResource", &request).await;
            if status == 200 {
                written += batch;
                continue;
            }
            refused = Some((status, body, keys, request));
            break;
        }

        let (status, body, keys, request) = refused.unwrap_or_else(|| {
            panic!(
                "case batch {batch}: expected to hit the per-resource tag quota within \
                 {TAGS_PER_CALL_MAX} tags, but never did ({written} accepted)"
            )
        });
        assert_eq!(
            status, 402,
            "case batch {batch}: the over-quota write is the modelled 402, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case batch {batch}: expected the modelled quota error, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "The request exceeds the maximum number of tags allowed per resource.",
            "case batch {batch}: expected the service's per-resource tag sentence, got {body}"
        );
        // The quota is where it is whatever the batch size: every accepted tag is within
        // it, and the refused batch is the first one that would pass it.
        assert!(
            written <= TAG_QUOTA && written + batch > TAG_QUOTA,
            "case batch {batch}: the refusal must mark the quota of {TAG_QUOTA}, got {written} \
             tags accepted"
        );

        // Atomically refused: not one key of the rejected batch persisted.
        let held = listed_tag_keys(&ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await);
        assert_eq!(
            held.len(),
            written,
            "case batch {batch}: the refused write applied nothing, got {} keys",
            held.len()
        );
        for key in &keys {
            assert!(
                !held.contains(key),
                "case batch {batch}: the refused key {key} must not be stored, got {held:?}"
            );
        }

        // Freeing headroom makes the identical call succeed.
        let freed: Vec<String> = (written - batch..written)
            .map(|index| format!("key-{index}"))
            .collect();
        ok(
            &router,
            "UntagResource",
            &json!({"ResourceArn": arn, "ResourceTagKeys": freed}),
        )
        .await;
        ok(&router, "TagResource", &request).await;
        let after = listed_tag_keys(&ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await);
        for key in &keys {
            assert!(
                after.contains(key),
                "case batch {batch}: the re-issued key {key} must be stored once there is \
                 headroom, got {after:?}"
            );
        }
        assert!(
            after.len() <= TAG_QUOTA,
            "case batch {batch}: the resource never holds more than {TAG_QUOTA} tags, got {}",
            after.len()
        );
    }
}

/// RULE: a point-in-time read answers for ANY instant a caller can name, including one
/// no millisecond count can hold — an instant beyond every stored one reads as "at or
/// after everything", one before every stored instant reads as "before everything", and
/// neither faults.
///
/// The rows are the extremes of what a timestamp can carry on the wire (it travels as a
/// JSON number and is read as a double, so these are the magnitudes a caller reaches),
/// plus the ordinary instants on either side of a write.
#[tokio::test]
async fn an_out_of_range_instant_is_answered_rather_than_faulting() {
    let router = emulator();
    let arn = arn_of(
        &ok(
            &router,
            "CreateInvoiceUnit",
            &json!({
                "Name": "extreme-instants",
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
                "ClientToken": "extreme-1",
            }),
        )
        .await,
    );

    // (the AsOf a caller names, whether the unit existed at it)
    for (as_of, exists) in [
        (json!(1e16), true),
        (json!(1e300), true),
        (json!(9.3e18), true),
        (json!(-1e16), false),
        (json!(-1e300), false),
        (json!(0), false),
    ] {
        let (status, body) = call(
            &router,
            "GetInvoiceUnit",
            &json!({"InvoiceUnitArn": arn, "AsOf": as_of}),
        )
        .await;
        assert_eq!(
            status,
            if exists { 200 } else { 404 },
            "case AsOf {as_of}: expected the unit to be {}, got {status} {body}",
            if exists { "reported" } else { "absent" }
        );

        let listed = listed_arns(&ok(&router, "ListInvoiceUnits", &json!({"AsOf": as_of})).await);
        assert_eq!(
            listed.contains(&arn),
            exists,
            "case list AsOf {as_of}: expected the unit to be {}, got {listed:?}",
            if exists { "reported" } else { "absent" }
        );

        // The same instants reach `ListInvoiceSummaries`' interval filter.
        let summaries = ok(
            &router,
            "ListInvoiceSummaries",
            &json!({
                "Selector": {"ResourceType": "ACCOUNT_ID", "Value": "123456789012"},
                "Filter": {"TimeInterval": {"StartDate": as_of, "EndDate": as_of}},
            }),
        )
        .await;
        assert!(
            summaries.get("InvoiceSummaries").is_some(),
            "case summaries over the interval [{as_of}, {as_of}]: expected a reported list, got {summaries}"
        );
    }
}

/// A well-formed `ProcurementPortalPreferenceArnString`
/// (`^arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+$`) for
/// `id`, minted the way state mints one.
fn preference_arn(id: &str) -> String {
    engine::emulator_arn(
        "invoicing",
        "",
        &format!("procurement-portal-preference/{id}"),
    )
}

/// Put a procurement portal preference in state, as a create would.
///
/// A test that needs a preference to EXIST at a chosen ARN seeds the registry the
/// validation flow reads — the same one copy `CreateProcurementPortalPreference` and
/// `PutProcurementPortalPreference` write — so the ARN is the test's to name.
fn seed_preference(engine: &Arc<engine::InMemoryEngine>, arn: &str, portal: &str) {
    let at = state::now();
    let stored = StoredProcurementPortalPreference {
        preference: ProcurementPortalPreference {
            aws_account_id: engine::emulator_account_id().to_string(),
            procurement_portal_preference_arn: arn.to_string(),
            procurement_portal_name: portal.to_string(),
            buyer_domain: "NetworkID".to_string(),
            buyer_identifier: "AN01234567890".to_string(),
            supplier_domain: "NetworkID".to_string(),
            supplier_identifier: "AN09876543210".to_string(),
            selector: None,
            procurement_portal_shared_secret: None,
            // The endpoint the test e-invoice of a portal validation is delivered to, so
            // the send that issues a code has a delivery TARGET to name in the refusal it
            // answers (see `crate::refused_procurement_portal_writes`).
            procurement_portal_instance_endpoint: Some(
                "https://portal.example.com/cxml".to_string(),
            ),
            purchase_order_retrieval_endpoint: None,
            test_env_preference: None,
            einvoice_delivery_enabled: false,
            einvoice_delivery_preference: None,
            purchase_order_retrieval_enabled: true,
            marketplace_punch_out_enabled: None,
            marketplace_punch_out_preference: None,
            contacts: None,
            einvoice_delivery_preference_status: Some("PENDING_VERIFICATION".to_string()),
            einvoice_delivery_preference_status_reason: None,
            purchase_order_retrieval_preference_status: Some("PENDING_VERIFICATION".to_string()),
            purchase_order_retrieval_preference_status_reason: None,
            version: 1,
            create_date: at,
            last_update_date: at,
        },
        validation: None,
        // No validation code has been submitted for a freshly seeded preference, so its
        // failed-attempt count is at zero.
        failed_verification_attempts: 0,
    };
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .insert(arn.to_string(), stored);
}

/// Make a validation code outstanding for `arn`, through the call that ISSUES one.
///
/// `SendProcurementPortalValidation` is that call — the model documents it as initiating
/// the validation by issuing a validation code — so the code is issued over the wire and
/// then READ from state: the modelled response carries only the ARN, because the real
/// service delivers the code through the configured procurement portal. The call itself
/// REFUSES, reporting the delivery of the test e-invoice this emulator could not make,
/// while the code it issued stays outstanding.
async fn issue_code(
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
    outstanding_code(engine, arn).unwrap_or_else(|| {
        panic!("case issue a code: the send must leave a code outstanding for {arn}")
    })
}

/// The validation code this service issued for `arn` and has not seen back yet, read
/// from state because no modelled response carries it.
fn outstanding_code(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<String> {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .get(arn)
        .and_then(|stored| stored.validation.as_ref())
        .filter(|validation| validation.verified_at.is_none())
        .map(|validation| validation.code.clone())
}

/// Whether the validation of `arn` has been completed, per state.
fn is_verified(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> bool {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .get(arn)
        .and_then(|stored| stored.validation.as_ref())
        .is_some_and(|validation| validation.verified_at.is_some())
}

/// How many preferences state holds.
fn stored_preferences(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .len()
}

/// RULE: an operation that names a resource by ARN resolves it against state, and an
/// ARN that names nothing is the modelled `ResourceNotFoundException` (HTTP 404) — it
/// neither succeeds nor creates state for a resource that does not exist.
///
/// The rows are well-formed preference ARNs of different id shapes, each tried with a
/// different `Code`, so nothing here is keyed to one recorded ARN.
#[tokio::test]
async fn portal_validation_refuses_an_arn_that_names_no_preference() {
    let (router, engine) = emulator_with_engine();

    for (id, code) in [
        ("does-not-exist", "000000"),
        ("a", "123456"),
        ("0123456789ab", "abcdef"),
        ("A-Very-Long-Preference-Id-0123456789", "9"),
    ] {
        let arn = preference_arn(id);
        for (operation, request) in [
            (
                "SendProcurementPortalValidation",
                json!({"ProcurementPortalPreferenceArn": arn}),
            ),
            (
                "VerifyProcurementPortalValidation",
                json!({"ProcurementPortalPreferenceArn": arn, "Code": code}),
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
        assert_eq!(
            stored_preferences(&engine),
            0,
            "case {arn}: a refused validation must not create a preference"
        );
    }
}

/// RULE: a validation completes only on the code the service issued for THAT resource
/// and has not seen back yet; every other submitted code is refused with the modelled
/// `ValidationException`, and a refusal changes nothing.
#[tokio::test]
async fn portal_validation_accepts_only_the_code_it_issued() {
    let (router, engine) = emulator_with_engine();
    let first = preference_arn("pref-0000000000a1");
    let second = preference_arn("pref-0000000000b2");
    seed_preference(&engine, &first, portal_at(0));
    seed_preference(&engine, &second, portal_at(2));

    // No code is outstanding yet, so there is no code any submission could match.
    let (status, body) = call(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": first, "Code": "000000"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case verify before any send: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case verify before any send: expected the modelled validation error, got {body}"
    );

    // The send is the call that ISSUES a code: it reports the delivery of the test
    // e-invoice this emulator could not make and leaves that preference's own code
    // outstanding, which is what the flow runs on.
    for arn in [&first, &second] {
        let _ = issue_code(&router, &engine, arn).await;
    }

    let issued = outstanding_code(&engine, &first)
        .unwrap_or_else(|| panic!("case send {first}: the send leaves a code outstanding"));
    let other = outstanding_code(&engine, &second)
        .unwrap_or_else(|| panic!("case send {second}: the send leaves a code outstanding"));
    assert_ne!(
        issued, other,
        "case send: each preference gets its own code, got {issued:?} for both"
    );
    assert_eq!(
        issued.len(),
        6,
        "case issue a code: the issued code is six digits, got {issued:?}"
    );
    assert!(
        issued.chars().all(|digit| digit.is_ascii_digit()),
        "case issue a code: the issued code is six digits, got {issued:?}"
    );

    // Rows differing along how a submitted code can be wrong: every digit off, a longer
    // code, a non-digit code, and the code issued for ANOTHER preference.
    let shifted: String = issued
        .chars()
        .map(|digit| {
            char::from_digit((digit.to_digit(10).unwrap_or_default() + 1) % 10, 10)
                .unwrap_or('0')
        })
        .collect();
    for wrong in [shifted.as_str(), "1234567", "not-a-code", other.as_str()] {
        let (status, body) = call(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": first, "Code": wrong}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case verify with {wrong:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case verify with {wrong:?}: expected the modelled validation error, got {body}"
        );
        assert!(
            !error_message(&body).contains(wrong),
            "case verify with {wrong:?}: the refusal must not reproduce the submitted code, got {body}"
        );
        assert!(
            !is_verified(&engine, &first),
            "case verify with {wrong:?}: a refused submission must not complete the validation"
        );
        assert_eq!(
            outstanding_code(&engine, &first).as_deref(),
            Some(issued.as_str()),
            "case verify with {wrong:?}: the issued code stays outstanding"
        );
    }

    // The issued code completes the validation, exactly once.
    let verified = ok(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": first, "Code": issued, "ClientToken": "verify-1"}),
    )
    .await;
    assert_eq!(
        verified
            .get("ProcurementPortalPreferenceArn")
            .and_then(Value::as_str),
        Some(first.as_str()),
        "case verify with the issued code: answers with the ARN it validated, got {verified}"
    );
    assert!(
        is_verified(&engine, &first),
        "case verify with the issued code: the validation is recorded as completed"
    );
    // `@idempotent`: the same token answers the same way without a second code.
    ok(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": first, "Code": issued, "ClientToken": "verify-1"}),
    )
    .await;
    let (status, body) = call(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": first, "Code": issued}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case verify a consumed code: expected HTTP 400, got {status} {body}"
    );

    // The other preference was untouched by all of that, and a newly issued code is
    // outstanding for it.
    assert!(
        !is_verified(&engine, &second),
        "case verify: one preference's validation must not complete another's"
    );
    let reissued = issue_code(&router, &engine, &second).await;
    ok(
        &router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": second, "Code": reissued}),
    )
    .await;
    assert!(
        is_verified(&engine, &second),
        "case re-issue then verify: the latest code issued completes the validation"
    );
}

/// The `CreateProcurementPortalPreference` request for `portal`, with every member the
/// model marks required (`smithy_query(query="operation",
/// operation="CreateProcurementPortalPreference")`).
fn create_preference_request(portal: &str) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": "AN01234567890",
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": "AN09876543210",
        "EinvoiceDeliveryEnabled": false,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": [{"Name": "Finance Ops", "Email": "finance@example.com"}],
    })
}

/// RULE: a member typed as an enum accepts exactly the values the service's enum
/// declares and refuses anything else with a `ValidationException` whose clause
/// enumerates that same set — so the set the validator enforces and the set it reports
/// are one set.
#[tokio::test]
async fn every_declared_procurement_portal_name_is_accepted_and_enumerated() {
    let router = emulator();

    // A value the enum does not declare is refused, and the clause lists the whole set.
    let (status, body) = call(
        &router,
        "CreateProcurementPortalPreference",
        &create_preference_request("NOT_A_REAL_PORTAL"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case NOT_A_REAL_PORTAL: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case NOT_A_REAL_PORTAL: expected the modelled validation error, got {body}"
    );
    let message = error_message(&body);
    let marker = "Member must satisfy enum value set: [";
    let enumerated = message
        .split_once(marker)
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(set, _)| set.to_string())
        .unwrap_or_else(|| panic!("case NOT_A_REAL_PORTAL: expected an enumerated set in {message:?}"));
    let mut reported: Vec<&str> = enumerated.split(", ").collect();
    reported.sort_unstable();
    let mut expected: Vec<&str> = PORTAL_NAMES.to_vec();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "case NOT_A_REAL_PORTAL: the clause must enumerate every declared portal, got {message:?}"
    );

    // And every declared value passes the enum constraint, so what matters here is that
    // the request is NOT refused for the value it carries.
    for portal in PORTAL_NAMES {
        let (status, body) = call(
            &router,
            "CreateProcurementPortalPreference",
            &create_preference_request(portal),
        )
        .await;
        assert_ne!(
            status, 400,
            "case {portal}: a declared portal name must not be refused, got {status} {body}"
        );
        assert_ne!(
            error_code(&body),
            "ValidationException",
            "case {portal}: a declared portal name must not be refused, got {body}"
        );
    }
}

/// RULE: a create whose `Name` is already taken by an invoice unit that EXISTS is
/// refused with the service's duplicate-name `ValidationException`, whose message
/// states the service's template over the requested name — the name enclosed in angle
/// brackets — so the name is recoverable from the message by those delimiters whatever
/// characters it contains.
#[tokio::test]
async fn a_duplicate_name_is_refused_with_the_services_template() {
    let router = emulator();

    // Rows differing along the name's shape, inside `InvoiceUnitName`'s `@length(1..50)`
    // and `@pattern` (`smithy_query(query="operation", operation="CreateInvoiceUnit")`).
    let longest = "n".repeat(50);
    for (index, name) in ["x", "Finance EU 2026", "a-b_c", longest.as_str()]
        .into_iter()
        .enumerate()
    {
        let request = json!({
            "Name": name,
            "InvoiceReceiver": "123456789012",
            "TaxInheritanceDisabled": false,
            // An account is linked into at most one invoice unit, and each row leaves a
            // unit of its own in state: every row links its own account.
            "Rule": {"LinkedAccounts": [format!("{:012}", 700 + index)]},
        });
        let arn = arn_of(&ok(&router, "CreateInvoiceUnit", &request).await);

        let (status, body) = call(&router, "CreateInvoiceUnit", &request).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a second create of one name is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: expected the modelled validation error, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Invoice Unit already exists with name: <{name}>"),
            "case {name:?}: expected the service's duplicate-name template, got {body}"
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            Some("duplicateInvoiceUnit"),
            "case {name:?}: expected the modelled duplicateInvoiceUnit reason, got {body}"
        );
        // The name is recoverable from the message by the delimiters that built it.
        let message = error_message(&body);
        let delimited = message
            .split_once('<')
            .and_then(|(_, tail)| tail.rsplit_once('>'))
            .map(|(name, _)| name.to_string());
        assert_eq!(
            delimited.as_deref(),
            Some(name),
            "case {name:?}: the requested name must be readable out of the message, got {body}"
        );

        // Uniqueness is over the units that exist: once this one is deleted, the name is
        // free and the identical create succeeds.
        ok(
            &router,
            "DeleteInvoiceUnit",
            &json!({"InvoiceUnitArn": arn, "ClientToken": format!("dup-delete-{index}")}),
        )
        .await;
        let again = ok(&router, "CreateInvoiceUnit", &request).await;
        assert_ne!(
            arn_of(&again),
            arn,
            "case {name:?}: recreating a deleted name mints a new unit, got {again}"
        );
    }
}
