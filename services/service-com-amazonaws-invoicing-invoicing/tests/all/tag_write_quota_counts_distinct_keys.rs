// SPDX-License-Identifier: CC0-1.0

//! The per-resource tag quota against the SHAPE of one `TagResource` call's array,
//! driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body,
//! so what these tests see is exactly what an SDK client sees.
//!
//! The rule under test, over a table of array shapes rather than one recorded request: a
//! write of tags is decided by the tag SET it would leave on the resource — a key the
//! resource already holds, and a key the call's array names more than once, replace a
//! value instead of adding a key — so two writes that would leave the same tags are
//! answered the same way, whichever call shape they arrived in, and the quota
//! (`crate::state::MAX_RESOURCE_TAGS`) is reached only by DISTINCT keys. `ResourceTagList`
//! carries no `@uniqueItems` (`smithy_query(query="shape", shape="ResourceTagList")` ->
//! `{"type": "list", "length": {"min": 0, "max": 200}, "member": "ResourceTag"}`), so a
//! repeated key is input the model allows.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::PROCUREMENT_PORTAL_NAME_VALUES;

/// How many tags ONE RESOURCE holds — the per-resource quota this service enforces
/// (`crate::state::MAX_RESOURCE_TAGS`, the AWS-wide "maximum number of tags per resource"
/// tagging quota), strictly below `ResourceTagList`'s per-CALL maximum.
const MAX_RESOURCE_TAGS: usize = 50;

/// The per-CALL maximum of `ResourceTagList` (`smithy_query(query="shape",
/// shape="ResourceTagList")`: `@length(0..200)`), which is NOT the per-resource quota: an
/// array may carry this many ENTRIES whatever the resource ends up holding.
const MAX_TAGS_PER_CALL: usize = 200;

/// The sentence the per-resource tag quota is refused with
/// (`crate::state::TAG_QUOTA_EXCEEDED_MESSAGE`): this service's canned wording, naming
/// neither a resource nor a count.
const TAG_QUOTA_MESSAGE: &str =
    "The request exceeds the maximum number of tags allowed per resource.";

/// The resource kinds of this service that carry tags, so the rule is asserted on the tag
/// plane rather than on one resource kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    InvoiceUnit,
    ProcurementPortalPreference,
}

const KINDS: &[Kind] = &[Kind::InvoiceUnit, Kind::ProcurementPortalPreference];

/// One row: how many distinct keys the resource already holds, and the `ResourceTags`
/// array of the write under test, as entries IN ORDER (so a repeated key's last value is
/// the one that must win).
#[derive(Debug, Clone)]
struct Case {
    what: &'static str,
    held: usize,
    write: Vec<(String, String)>,
}

/// A held key: the keys a resource is pre-loaded with, `held` of them.
fn held_key(index: usize) -> String {
    format!("key-{index}")
}

/// The rows the rule is asserted over: array shapes that differ in how many entries they
/// carry, how many DISTINCT keys those entries name, whether a repeated key is new or
/// already held, and where the resulting set falls relative to the quota.
fn cases() -> Vec<Case> {
    let entry = |key: &str, value: &str| (key.to_string(), value.to_string());
    vec![
        Case {
            what: "one new key fills the last free slot",
            held: MAX_RESOURCE_TAGS - 1,
            write: vec![entry("fresh", "v")],
        },
        Case {
            what: "the same new key twice fills the last free slot once",
            held: MAX_RESOURCE_TAGS - 1,
            write: vec![entry("fresh", "first"), entry("fresh", "last")],
        },
        Case {
            what: "two entries sharing a new key plus another new key is past the quota",
            held: MAX_RESOURCE_TAGS - 1,
            write: vec![
                entry("fresh", "first"),
                entry("fresh", "last"),
                entry("other", "v"),
            ],
        },
        Case {
            what: "an existing key repeated is a replacement, not a step",
            held: MAX_RESOURCE_TAGS - 1,
            write: vec![
                (held_key(0), "first".to_string()),
                (held_key(0), "last".to_string()),
            ],
        },
        Case {
            what: "a resource at the quota takes repeated rewrites of keys it holds",
            held: MAX_RESOURCE_TAGS,
            write: vec![
                (held_key(0), "first".to_string()),
                (held_key(0), "last".to_string()),
                (held_key(1), "v".to_string()),
            ],
        },
        Case {
            what: "a resource at the quota refuses a new key however often it is repeated",
            held: MAX_RESOURCE_TAGS,
            write: vec![entry("fresh", "first"), entry("fresh", "last")],
        },
        Case {
            what: "the per-call maximum of entries naming ONE key is one tag",
            held: 0,
            write: (0..MAX_TAGS_PER_CALL)
                .map(|index| entry("one", &format!("value-{index}")))
                .collect(),
        },
        Case {
            what: "twice the quota in entries naming the quota's worth of keys is accepted",
            held: 0,
            write: (0..2 * MAX_RESOURCE_TAGS)
                .map(|index| {
                    (
                        format!("dup-{}", index % MAX_RESOURCE_TAGS),
                        format!("value-{index}"),
                    )
                })
                .collect(),
        },
        Case {
            what: "one distinct key past the quota is refused however many entries carry it",
            held: 0,
            write: (0..2 * (MAX_RESOURCE_TAGS + 1))
                .map(|index| {
                    (
                        format!("dup-{}", index % (MAX_RESOURCE_TAGS + 1)),
                        format!("value-{index}"),
                    )
                })
                .collect(),
        },
        Case {
            what: "an empty array leaves the resource exactly as it was",
            held: 3,
            write: Vec::new(),
        },
        Case {
            what: "a mixed array of held and new keys, each repeated, lands on the quota",
            held: MAX_RESOURCE_TAGS - 2,
            write: vec![
                (held_key(0), "a".to_string()),
                entry("fresh-a", "b"),
                (held_key(0), "c".to_string()),
                entry("fresh-b", "d"),
                entry("fresh-a", "e"),
                entry("fresh-b", "f"),
            ],
        },
    ]
}

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

/// Entries as a `ResourceTagList` on the wire, in the order given.
fn tag_list(entries: &[(String, String)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The tags `ListTagsForResource` reports for a resource, as a map.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = ok(router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    body["ResourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ResourceTags list, got {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A distinct account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`)
/// per invoice unit this slice mints: an account is linked into at most one invoice unit,
/// so no two units here link the same one.
fn next_linked_account() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(700);
    format!(
        "{:012}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Mint a resource of `kind` and return its ARN.
async fn resource(router: &MultiServiceRouter, kind: Kind, label: &str) -> String {
    match kind {
        Kind::InvoiceUnit => {
            let created = ok(
                router,
                "CreateInvoiceUnit",
                &json!({
                    "Name": format!("TagShapeUnit{label}"),
                    "InvoiceReceiver": "123456789012",
                    "TaxInheritanceDisabled": false,
                    "Rule": {"LinkedAccounts": [next_linked_account()]},
                }),
            )
            .await;
            created["InvoiceUnitArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected an InvoiceUnitArn, got {created}"))
                .to_string()
        }
        Kind::ProcurementPortalPreference => {
            let created = ok(
                router,
                "CreateProcurementPortalPreference",
                &json!({
                    // One of the portals the model declares (`smithy_query(query="shape",
                    // shape="ProcurementPortalName")`); which one it is does not matter to
                    // the tag quota under test.
                    "ProcurementPortalName": PROCUREMENT_PORTAL_NAME_VALUES[0],
                    "BuyerDomain": "NetworkID",
                    "BuyerIdentifier": format!("buyer-{label}"),
                    "SupplierDomain": "NetworkID",
                    "SupplierIdentifier": format!("supplier-{label}"),
                    "EinvoiceDeliveryEnabled": true,
                    "PurchaseOrderRetrievalEnabled": true,
                    "Contacts": [{"Name": "Portal Contact", "Email": "portal.contact@example.com"}],
                }),
            )
            .await;
            created["ProcurementPortalPreferenceArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("expected a ProcurementPortalPreferenceArn, got {created}")
                })
                .to_string()
        }
    }
}

/// A resource of `kind` already holding `held` distinct tags, written through
/// `TagResource` so the pre-load itself goes over the wire.
async fn resource_holding(
    router: &MultiServiceRouter,
    kind: Kind,
    label: &str,
    held: usize,
) -> String {
    let arn = resource(router, kind, label).await;
    if held > 0 {
        let entries: Vec<(String, String)> = (0..held)
            .map(|index| (held_key(index), format!("held-{index}")))
            .collect();
        ok(
            router,
            "TagResource",
            &json!({"ResourceArn": arn.clone(), "ResourceTags": tag_list(&entries)}),
        )
        .await;
    }
    let listed = listed_tags(router, &arn).await;
    assert_eq!(
        listed.len(),
        held,
        "pre-load of {kind:?} {label}: expected {held} tags, got {listed:?}"
    );
    arn
}

/// THE RULE: a `TagResource` call is decided — and applied — by the tag SET it would
/// leave on the resource, so entries sharing a key cost one slot between them, and a
/// refusal happens only when the DISTINCT keys would pass the quota. A refused write
/// persists nothing.
#[tokio::test]
async fn the_tag_quota_counts_the_distinct_keys_the_write_would_leave() {
    let router = emulator();

    for kind in KINDS {
        for (index, case) in cases().into_iter().enumerate() {
            let label = format!("{index}-{}", usize::from(*kind == Kind::InvoiceUnit));
            let arn = resource_holding(&router, *kind, &label, case.held).await;
            let before = listed_tags(&router, &arn).await;

            // What the write would leave the resource holding: the held tags with the
            // array applied in order, one entry per key.
            let mut want = before.clone();
            for (key, value) in &case.write {
                want.insert(key.clone(), value.clone());
            }

            let (status, body) = call(
                &router,
                "TagResource",
                &json!({"ResourceArn": arn.clone(), "ResourceTags": tag_list(&case.write)}),
            )
            .await;

            if want.len() <= MAX_RESOURCE_TAGS {
                assert_eq!(
                    status, 200,
                    "case {kind:?} {what:?} ({entries} entries, {distinct} distinct keys \
                     stored): expected HTTP 200, got {status} {body}",
                    what = case.what,
                    entries = case.write.len(),
                    distinct = want.len()
                );
                let after = listed_tags(&router, &arn).await;
                assert_eq!(
                    after, want,
                    "case {kind:?} {what:?}: expected the held tags with the array applied \
                     in order, got {after:?}",
                    what = case.what
                );
            } else {
                assert_eq!(
                    status, 402,
                    "case {kind:?} {what:?} ({entries} entries, {distinct} distinct keys \
                     stored): expected HTTP 402, got {status} {body}",
                    what = case.what,
                    entries = case.write.len(),
                    distinct = want.len()
                );
                assert_eq!(
                    error_code(&body),
                    "ServiceQuotaExceededException",
                    "case {kind:?} {what:?}: expected the modelled quota error, got {body}",
                    what = case.what
                );
                assert_eq!(
                    error_message(&body),
                    TAG_QUOTA_MESSAGE,
                    "case {kind:?} {what:?}: expected the quota sentence, got {body}",
                    what = case.what
                );
                let after = listed_tags(&router, &arn).await;
                assert_eq!(
                    after, before,
                    "case {kind:?} {what:?}: a refused write persists nothing, got {after:?}",
                    what = case.what
                );
            }
        }
    }
}

/// The same rule as an INVARIANT on the call shape: one array and the same entries sent
/// one call at a time leave the same tags and are both accepted, so splitting or merging
/// a write never changes the verdict.
#[tokio::test]
async fn one_array_and_one_call_per_entry_are_the_same_write() {
    let router = emulator();

    // Entry sequences that repeat keys in different places, each landing exactly on the
    // quota from a resource two keys short of it.
    let sequences: Vec<Vec<(String, String)>> = vec![
        vec![
            ("fresh-a".to_string(), "1".to_string()),
            ("fresh-a".to_string(), "2".to_string()),
            ("fresh-b".to_string(), "3".to_string()),
        ],
        vec![
            ("fresh-b".to_string(), "3".to_string()),
            ("fresh-a".to_string(), "1".to_string()),
            ("fresh-a".to_string(), "2".to_string()),
        ],
        vec![
            (held_key(0), "rewritten".to_string()),
            ("fresh-a".to_string(), "2".to_string()),
            ("fresh-b".to_string(), "3".to_string()),
            ("fresh-b".to_string(), "3".to_string()),
        ],
    ];

    for (index, sequence) in sequences.into_iter().enumerate() {
        let batched = resource_holding(
            &router,
            Kind::InvoiceUnit,
            &format!("batched-{index}"),
            MAX_RESOURCE_TAGS - 2,
        )
        .await;
        let one_by_one = resource_holding(
            &router,
            Kind::ProcurementPortalPreference,
            &format!("single-{index}"),
            MAX_RESOURCE_TAGS - 2,
        )
        .await;

        let (batched_status, batched_body) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": batched.clone(), "ResourceTags": tag_list(&sequence)}),
        )
        .await;
        assert_eq!(
            batched_status, 200,
            "case {sequence:?} batched: expected HTTP 200, got {batched_status} {batched_body}"
        );

        for entry in &sequence {
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({
                    "ResourceArn": one_by_one.clone(),
                    "ResourceTags": tag_list(std::slice::from_ref(entry)),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {sequence:?} entry {entry:?}: expected HTTP 200, got {status} {body}"
            );
        }

        let batched_tags = listed_tags(&router, &batched).await;
        let single_tags = listed_tags(&router, &one_by_one).await;
        assert_eq!(
            batched_tags, single_tags,
            "case {sequence:?}: one array and one call per entry must leave the same tags, \
             got {batched_tags:?} and {single_tags:?}"
        );
        assert_eq!(
            batched_tags.len(),
            MAX_RESOURCE_TAGS,
            "case {sequence:?}: expected the resource to land on the quota, got {batched_tags:?}"
        );
    }
}
