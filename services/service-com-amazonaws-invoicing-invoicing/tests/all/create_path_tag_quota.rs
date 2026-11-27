// SPDX-License-Identifier: CC0-1.0

//! The per-resource tag quota on the CREATE paths of this service, driven end to end
//! over its wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0
//! body. Nothing reaches into the state struct, so what these tests see is exactly what
//! an SDK client sees.
//!
//! The rule under test, over a table of tag counts rather than one recorded request:
//! a resource holds at most [`MAX_RESOURCE_TAGS`] tags HOWEVER the tags got there, so a
//! create whose `ResourceTags` would leave the resource it mints past that quota is
//! refused — atomically, with nothing persisted — carrying the quota error that
//! operation models, while a create at or below the quota is accepted and the tags it
//! carried are exactly what `ListTagsForResource` reports. The quota is therefore the
//! same on the create paths as on `TagResource`, which the last test asserts directly.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// How many tags ONE RESOURCE holds — the per-resource quota this service enforces
/// (`crate::state::MAX_RESOURCE_TAGS`, the AWS-wide "maximum number of tags per
/// resource" tagging quota), strictly below `ResourceTagList`'s per-CALL maximum.
const MAX_RESOURCE_TAGS: usize = 50;

/// The per-CALL maximum of `ResourceTagList` (`smithy_query(query="operation",
/// operation="CreateProcurementPortalPreference,CreateInvoiceUnit,TagResource")`:
/// `ResourceTagList` is `@length(0..200)`), which is NOT the per-resource quota.
const MAX_TAGS_PER_CALL: usize = 200;

/// The sentence the per-resource tag quota is refused with: this service's own canned
/// wording (`crate::state::TAG_QUOTA_EXCEEDED_MESSAGE`), which names neither a resource
/// nor a count, so every refusal of this quota carries it.
const TAG_QUOTA_MESSAGE: &str = "The request exceeds the maximum number of tags allowed per resource.";

/// Every value `ProcurementPortalName` accepts, taken from the model's own value set
/// (`smithy_query(query="shape", shape="ProcurementPortalName")`), so the rows span the
/// portals the model declares rather than repeating one.
const PORTAL_NAMES: &[&str] = PROCUREMENT_PORTAL_NAME_VALUES;

/// How a create carrying `tags` distinct tag keys must be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Within the per-resource quota: accepted, and the tags are on the resource.
    Accepted,
    /// Past the per-resource quota: refused with the quota error the operation models.
    OverQuota,
    /// Past `ResourceTagList`'s per-call maximum: refused by the modelled constraint,
    /// before any operation of this service sees the request.
    OverCallMaximum,
}

/// One row of the table: a tag count and the verdict the rule gives it.
#[derive(Debug, Clone, Copy)]
struct Row {
    tags: usize,
    verdict: Verdict,
}

/// The tag counts the rule is asserted over: both ends of the accepted range, the quota
/// itself, one key past it, counts well past it up to the per-call maximum, and one past
/// that published per-call bound.
fn rows() -> Vec<Row> {
    vec![
        Row {
            tags: 0,
            verdict: Verdict::Accepted,
        },
        Row {
            tags: 1,
            verdict: Verdict::Accepted,
        },
        Row {
            tags: 7,
            verdict: Verdict::Accepted,
        },
        Row {
            tags: MAX_RESOURCE_TAGS - 1,
            verdict: Verdict::Accepted,
        },
        Row {
            tags: MAX_RESOURCE_TAGS,
            verdict: Verdict::Accepted,
        },
        Row {
            tags: MAX_RESOURCE_TAGS + 1,
            verdict: Verdict::OverQuota,
        },
        Row {
            tags: 60,
            verdict: Verdict::OverQuota,
        },
        Row {
            tags: 120,
            verdict: Verdict::OverQuota,
        },
        Row {
            tags: MAX_TAGS_PER_CALL,
            verdict: Verdict::OverQuota,
        },
        Row {
            tags: MAX_TAGS_PER_CALL + 1,
            verdict: Verdict::OverCallMaximum,
        },
    ]
}

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

/// The explanatory text of a failure body, under either case of the member's wire key.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `count` distinct tags, as a `ResourceTagList` on the wire.
fn tag_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
            .collect(),
    )
}

/// The `ResourceTags` a `ListTagsForResource` answer reports, as sorted pairs.
fn listed_tags(body: &Value) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = body["ResourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ResourceTags list, got {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    tags.sort();
    tags
}

/// A `CreateProcurementPortalPreference` body for one identity, carrying `tags` tags.
fn preference_create(portal: &str, identity: &str, tags: usize) -> Value {
    let mut request = json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": format!("buyer-{identity}"),
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": format!("supplier-{identity}"),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
        "Contacts": [{"Name": "Portal Contact", "Email": "portal.contact@example.com"}],
    });
    if tags > 0 {
        request["ResourceTags"] = tag_list(tags);
    }
    request
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), derived
/// from a row index: an account is linked into at most one invoice unit, so each row's
/// unit links its own.
fn account(index: usize) -> String {
    format!("{:012}", 700 + index)
}

/// A `CreateInvoiceUnit` body for one row, carrying `tags` tags.
fn invoice_unit_create(index: usize, name: &str, tags: usize) -> Value {
    let mut request = json!({
        "Name": name,
        "InvoiceReceiver": "123456789012",
        "TaxInheritanceDisabled": false,
        "Rule": {"LinkedAccounts": [account(index)]},
    });
    if tags > 0 {
        request["ResourceTags"] = tag_list(tags);
    }
    request
}

/// THE RULE on `CreateProcurementPortalPreference`: the tag count decides whether the
/// create is accepted, the refusal is the `ServiceQuotaExceededException` (HTTP 402) this
/// operation models, and a refused create persists nothing — the identity it asked for is
/// still free afterwards.
#[tokio::test]
async fn the_tag_quota_decides_a_preference_create() {
    let router = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let identity = format!("{index}");
        let (status, body) = call(
            &router,
            "CreateProcurementPortalPreference",
            &preference_create(portal, &identity, row.tags),
        )
        .await;

        match row.verdict {
            Verdict::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {row:?} on {portal}: expected HTTP 200, got {status} {body}"
                );
                let arn = body["ProcurementPortalPreferenceArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {row:?}: expected an ARN, got {body}"))
                    .to_string();
                // The tags the accepted create carried are what the resource holds.
                let listed = ok(
                    &router,
                    "ListTagsForResource",
                    &json!({"ResourceArn": arn.clone()}),
                )
                .await;
                let mut want: Vec<(String, String)> = (0..row.tags)
                    .map(|key| (format!("key-{key}"), format!("value-{key}")))
                    .collect();
                want.sort();
                assert_eq!(
                    listed_tags(&listed),
                    want,
                    "case {row:?}: expected exactly the tags the create carried, got {listed}"
                );
                // And the create really persisted the preference it answered with.
                let read = ok(
                    &router,
                    "GetProcurementPortalPreference",
                    &json!({"ProcurementPortalPreferenceArn": arn.clone()}),
                )
                .await;
                assert_eq!(
                    read["ProcurementPortalPreference"]["ProcurementPortalPreferenceArn"]
                        .as_str()
                        .unwrap_or_default(),
                    arn,
                    "case {row:?}: expected the preference to read back, got {read}"
                );
            }
            Verdict::OverQuota => {
                assert_eq!(
                    status, 402,
                    "case {row:?} on {portal}: expected HTTP 402, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ServiceQuotaExceededException",
                    "case {row:?}: expected the modelled quota error, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    TAG_QUOTA_MESSAGE,
                    "case {row:?}: expected the quota sentence, got {body}"
                );
            }
            Verdict::OverCallMaximum => {
                assert_eq!(
                    status, 400,
                    "case {row:?} on {portal}: expected HTTP 400 from the modelled \
                     per-call maximum, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {row:?}: expected the modelled constraint refusal, got {body}"
                );
            }
        }

        if row.verdict != Verdict::Accepted {
            // Nothing of a refused create persisted: the identity is still free, so the
            // same identity creates cleanly with a legal tag list (a conflict here would
            // mean the refused create had stored a preference).
            let retried = ok(
                &router,
                "CreateProcurementPortalPreference",
                &preference_create(portal, &identity, 2),
            )
            .await;
            let arn = retried["ProcurementPortalPreferenceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {row:?} retry: expected an ARN, got {retried}"))
                .to_string();
            let listed = ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
            assert_eq!(
                listed_tags(&listed).len(),
                2,
                "case {row:?} retry: the refused create must have left no tags behind, \
                 got {listed}"
            );
        }
    }
}

/// THE SAME RULE on `CreateInvoiceUnit`, the other create of this service that carries
/// `ResourceTags`: the count decides the create, and the refusal is the error THAT
/// operation models — it declares no `ServiceQuotaExceededException`, so the quota is
/// stated in its `ValidationException` (HTTP 400).
#[tokio::test]
async fn the_tag_quota_decides_an_invoice_unit_create() {
    let router = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let name = format!("QuotaUnit{index}");
        let (status, body) = call(
            &router,
            "CreateInvoiceUnit",
            &invoice_unit_create(index, &name, row.tags),
        )
        .await;

        match row.verdict {
            Verdict::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {row:?} {name}: expected HTTP 200, got {status} {body}"
                );
                let arn = body["InvoiceUnitArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {row:?}: expected an ARN, got {body}"))
                    .to_string();
                let listed = ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
                assert_eq!(
                    listed_tags(&listed).len(),
                    row.tags,
                    "case {row:?}: expected {} tags on the unit, got {listed}",
                    row.tags
                );
            }
            Verdict::OverQuota => {
                assert_eq!(
                    status, 400,
                    "case {row:?} {name}: expected HTTP 400, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {row:?}: expected the error this operation models, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    TAG_QUOTA_MESSAGE,
                    "case {row:?}: expected the quota sentence, got {body}"
                );
            }
            Verdict::OverCallMaximum => {
                assert_eq!(
                    status, 400,
                    "case {row:?} {name}: expected HTTP 400 from the modelled per-call \
                     maximum, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {row:?}: expected the modelled constraint refusal, got {body}"
                );
            }
        }

        if row.verdict != Verdict::Accepted {
            // A refused create stored nothing, so the name it asked for is free: the
            // retry succeeds rather than colliding with a unit of that name.
            let retried = ok(
                &router,
                "CreateInvoiceUnit",
                &invoice_unit_create(index, &name, 1),
            )
            .await;
            assert!(
                retried["InvoiceUnitArn"].is_string(),
                "case {row:?} retry: expected an ARN, got {retried}"
            );
        }
    }
}

/// The quota is ONE quota: whatever a resource was created with counts against what
/// `TagResource` may still add to it, so a preference created at the quota has no
/// headroom left, and freeing a key with `UntagResource` gives exactly one key back.
#[tokio::test]
async fn the_create_path_and_the_tag_path_share_one_quota() {
    let router = emulator();

    for (index, portal) in PORTAL_NAMES.iter().enumerate() {
        // Created with the quota's worth of tags, from the create path.
        let created = ok(
            &router,
            "CreateProcurementPortalPreference",
            &preference_create(portal, &format!("full-{index}"), MAX_RESOURCE_TAGS),
        )
        .await;
        let arn = created["ProcurementPortalPreferenceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {portal}: expected an ARN, got {created}"))
            .to_string();

        // Re-writing a key the create already set is a replacement, not a step past the
        // quota.
        let (replaced_status, replaced) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn.clone(), "ResourceTags": [{"Key": "key-0", "Value": "other"}]}),
        )
        .await;
        assert_eq!(
            replaced_status, 200,
            "case {portal}: replacing a key set by the create is accepted, got \
             {replaced_status} {replaced}"
        );

        // One more DISTINCT key is past the quota, exactly as it would be had the tags
        // been written by `TagResource` instead of by the create.
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn.clone(), "ResourceTags": [{"Key": "one-too-many", "Value": "v"}]}),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {portal}: a tag write past the quota is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {portal}: expected the modelled quota error, got {body}"
        );
        assert_eq!(
            error_message(&body),
            TAG_QUOTA_MESSAGE,
            "case {portal}: the create path and the tag path refuse with one sentence, \
             got {body}"
        );

        // Freeing one key gives exactly one key of headroom back.
        ok(
            &router,
            "UntagResource",
            &json!({"ResourceArn": arn.clone(), "ResourceTagKeys": ["key-1"]}),
        )
        .await;
        let (accepted_status, accepted) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn.clone(), "ResourceTags": [{"Key": "one-too-many", "Value": "v"}]}),
        )
        .await;
        assert_eq!(
            accepted_status, 200,
            "case {portal}: the freed key is headroom for exactly one more, got \
             {accepted_status} {accepted}"
        );
        let listed = ok(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
        assert_eq!(
            listed_tags(&listed).len(),
            MAX_RESOURCE_TAGS,
            "case {portal}: the resource holds the quota and no more, got {listed}"
        );
    }
}
