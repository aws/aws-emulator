// SPDX-License-Identifier: CC0-1.0

//! AWS HealthScribe's `health-agent` namespace (connecthealth) as an OWNING service of
//! this API: a Domain, and a live Subscription under one, holds its own tag set, so that
//! set — not a second copy here — is what `GetResources` reports, what `TagResources`
//! writes and what `UntagResources` removes from.
//!
//! The rule under test, over a TABLE of resources and tag writes rather than the one case
//! the issue recorded: a tag the OWNING service wrote is in this API's index at once (the
//! resource is returned by `GetResources` under its own ARN, carrying that tag, and is
//! selected by `TagFilters` and by the `health-agent` `ResourceTypeFilters`); a tag the
//! owning service REMOVED is out of it at once; a tag written through THIS API is visible
//! through the owning service's own `ListTagsForResource`; and a well-formed ARN that
//! service holds no resource for is that ARN's own `FailedResourcesMap` entry carrying
//! THAT service's refusal rather than a silently created resource.
//!
//! Every test drives the real wire path over ONE engine carrying both services, which is
//! how the emulator runs: `POST /` with an `X-Amz-Target` and an awsJson body for this
//! API, and the restJson1 routes connecthealth publishes for the owning service.
//!
//! The rows span what the rule generalises over: which resource kind carries the tags (a
//! Domain and a Subscription under it), how many tags and how many resources are
//! involved, which API wrote the tag, and which of the two spellings of a resource-type
//! filter selects it (the bare service namespace and the qualified
//! `health-agent:domain`).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth as owner_connecthealth;
use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and therefore the one both endpoints
/// serve and the one the ARNs connecthealth mints belong to.
const REGION: &str = "eu-west-1";

/// The SigV4 signing name this API is routed on.
const TAGGING: &str = "tagging";

/// The SigV4 signing name connecthealth is routed on — its `endpointPrefix` /
/// `arnNamespace`, which is also the service field of every ARN it mints
/// (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).
const HEALTH_AGENT: &str = "health-agent";

/// A router carrying this API and connecthealth over ONE engine — the arrangement
/// `core/router` builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization` header below is
    // present because it is what routes a request to a signing name, not to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_connecthealth::EmulatedService::new(engine));
    router
}

/// The `Authorization` header that routes a request to `signing_name` in [`REGION`].
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0"
    )
}

/// One tagging-API operation, over the awsJson wire this API publishes.
async fn tagging(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let target = format!("ResourceGroupsTaggingAPI_20170126.{operation}");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", &target)
        .header("authorization", authorization(TAGGING))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One connecthealth operation, over the restJson1 routes that model publishes.
async fn connecthealth(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|body| body.to_string().into_bytes())
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", authorization(HEALTH_AGENT))
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `/tags/{resourceArn}`, connecthealth's tagging route, with the ARN percent-encoded
/// into its single non-greedy label exactly as an SDK encodes it.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", arn.replace(':', "%3A").replace('/', "%2F"))
}

/// A Domain created through connecthealth's own `CreateDomain`, with `tags` as given:
/// `(domainId, domain ARN)`.
async fn domain(router: &MultiServiceRouter, name: &str, tags: Value) -> (String, String) {
    let mut body = json!({ "name": name });
    if !tags.is_null() {
        body["tags"] = tags;
    }
    let (status, created) = connecthealth(router, "POST", "/domain", Some(body)).await;
    assert_eq!(status, 201, "CreateDomain must answer 201, got {created}");
    (
        created["domainId"]
            .as_str()
            .unwrap_or_else(|| panic!("the create reports a domainId, got {created}"))
            .to_string(),
        created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("the create reports an arn, got {created}"))
            .to_string(),
    )
}

/// The tag map connecthealth's own `ListTagsForResource` reports for one ARN.
async fn owner_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, listed) = connecthealth(router, "GET", &tags_path(arn), None).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource must answer 200 for {arn}, got {listed}"
    );
    listed["tags"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ARNs a `GetResources` page carries.
fn page_arns(body: &Value) -> Vec<String> {
    body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .map(|mapping| {
            mapping["ResourceARN"]
                .as_str()
                .unwrap_or_else(|| panic!("a mapping with no ResourceARN: {mapping}"))
                .to_string()
        })
        .collect()
}

/// The tag map a `GetResources` page reports for one ARN.
fn page_tags(body: &Value, arn: &str) -> BTreeMap<String, String> {
    let mapping = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == json!(arn))
        .unwrap_or_else(|| panic!("{arn} is not in the page: {body}"));
    mapping["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a mapping with no Tags: {mapping}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().expect("Tag.Key").to_string(),
                tag["Value"].as_str().expect("Tag.Value").to_string(),
            )
        })
        .collect()
}

/// `GetResources` with the filters given, asserting the modelled success.
async fn get_resources(router: &MultiServiceRouter, case: &str, filters: Value) -> Value {
    let (status, body) = tagging(router, "GetResources", filters).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetResources must answer 200, got {status} with {body}"
    );
    body
}

#[tokio::test]
async fn a_domain_tagged_through_connecthealth_is_in_this_apis_index() {
    let router = emulator();

    // Three resources of the owning service, differing in how they came to carry tags and
    // in how many they carry: a Domain tagged by its own CREATE, a Domain tagged after the
    // fact through connecthealth's `TagResource`, and a Subscription under the first one —
    // the other resource kind that service holds a tag set for.
    let (created_id, created_arn) = domain(
        &router,
        "clinic-tagged-at-create",
        json!({ "env": "prod", "team": "cardiology" }),
    )
    .await;
    let (_, tagged_arn) = domain(&router, "clinic-tagged-later", Value::Null).await;
    let (status, body) = connecthealth(
        &router,
        "POST",
        &tags_path(&tagged_arn),
        Some(json!({ "tags": { "env": "prod" } })),
    )
    .await;
    assert_eq!(status, 204, "connecthealth TagResource got {status}: {body}");

    let (status, subscription) = connecthealth(
        &router,
        "POST",
        &format!("/domains/{created_id}/subscriptions"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 201, "CreateSubscription got {status}: {subscription}");
    let subscription_arn = subscription["arn"]
        .as_str()
        .expect("the subscription reports an arn")
        .to_string();
    let (status, body) = connecthealth(
        &router,
        "POST",
        &tags_path(&subscription_arn),
        Some(json!({ "tags": { "env": "staging" } })),
    )
    .await;
    assert_eq!(status, 204, "tagging the subscription got {status}: {body}");

    // Every one of them is in the index, carrying exactly the tags the owning service
    // holds for it — which is the same map that service's own reader reports.
    let whole = get_resources(&router, "the whole index", json!({})).await;
    for (case, arn) in [
        ("a Domain tagged by its create", &created_arn),
        ("a Domain tagged after the fact", &tagged_arn),
        ("a Subscription under a Domain", &subscription_arn),
    ] {
        assert!(
            page_arns(&whole).contains(arn),
            "case {case:?}: {arn} must be in the index, got {whole}"
        );
        let indexed = page_tags(&whole, arn);
        let held = owner_tags(&router, arn).await;
        assert_eq!(
            indexed, held,
            "case {case:?}: the index must report the owning service's own tags, got \
             {indexed:?} against {held:?}"
        );
    }

    // The filters a tag-driven sweep uses select them: a tag filter by key and value, and
    // a resource-type filter in either of the two spellings `ResourceTypeFilters` accepts
    // ("The format of each resource type is `service[:resourceType]`" — the bare service
    // namespace selects every resource of that service, the qualified token only that one
    // type).
    for (case, filters, expected) in [
        (
            "a tag filter by key and value",
            json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] }),
            vec![&created_arn, &tagged_arn],
        ),
        (
            "a tag filter by key alone",
            json!({ "TagFilters": [{ "Key": "team" }] }),
            vec![&created_arn],
        ),
        (
            "the bare service namespace as the resource type",
            json!({ "ResourceTypeFilters": [HEALTH_AGENT] }),
            vec![&created_arn, &tagged_arn, &subscription_arn],
        ),
        (
            "the qualified resource type",
            json!({ "ResourceTypeFilters": [format!("{HEALTH_AGENT}:domain")] }),
            vec![&created_arn, &tagged_arn, &subscription_arn],
        ),
        (
            "both filters at once",
            json!({
                "TagFilters": [{ "Key": "env", "Values": ["prod"] }],
                "ResourceTypeFilters": [HEALTH_AGENT],
            }),
            vec![&created_arn, &tagged_arn],
        ),
        (
            "a tag value nothing carries",
            json!({ "TagFilters": [{ "Key": "env", "Values": ["nowhere"] }] }),
            vec![],
        ),
    ] {
        let page = get_resources(&router, case, filters).await;
        let arns = page_arns(&page);
        for arn in &expected {
            assert!(
                arns.contains(arn),
                "case {case:?}: {arn} must be selected, got {page}"
            );
        }
        if expected.is_empty() {
            assert!(
                arns.is_empty(),
                "case {case:?}: nothing must be selected, got {page}"
            );
        }
    }

    // The keys and values this API reports for the whole account include the ones the
    // owning service wrote, so a tag sweep that starts from the key list finds them too.
    let (status, keys) = tagging(&router, "GetTagKeys", json!({})).await;
    assert_eq!(status, 200, "GetTagKeys got {status}: {keys}");
    for key in ["env", "team"] {
        assert!(
            keys["TagKeys"]
                .as_array()
                .is_some_and(|held| held.contains(&json!(key))),
            "the key {key} the owning service holds must be reported, got {keys}"
        );
    }
}

#[tokio::test]
async fn a_tag_the_owning_service_removed_leaves_this_apis_index() {
    let router = emulator();
    let (_, arn) = domain(
        &router,
        "clinic-removal",
        json!({ "env": "prod", "probe": "value" }),
    )
    .await;

    // Removing ONE key leaves the resource indexed under the keys it still carries.
    let (status, body) = connecthealth(
        &router,
        "DELETE",
        &format!("{}?tagKeys=probe", tags_path(&arn)),
        None,
    )
    .await;
    assert_eq!(status, 204, "connecthealth UntagResource got {status}: {body}");
    let page = get_resources(
        &router,
        "after one key was removed",
        json!({ "TagFilters": [{ "Key": "probe" }] }),
    )
    .await;
    assert!(
        !page_arns(&page).contains(&arn),
        "the removed key must select nothing, got {page}"
    );
    let still = get_resources(
        &router,
        "the key it still carries",
        json!({ "TagFilters": [{ "Key": "env" }] }),
    )
    .await;
    assert!(
        page_arns(&still).contains(&arn),
        "the key it still carries must select it, got {still}"
    );

    // Removing the LAST key takes the resource out of the index entirely: "GetResources
    // does not return untagged resources."
    let (status, body) = connecthealth(
        &router,
        "DELETE",
        &format!("{}?tagKeys=env", tags_path(&arn)),
        None,
    )
    .await;
    assert_eq!(status, 204, "connecthealth UntagResource got {status}: {body}");
    // Asserted twice, so the absence is a property of the index rather than one read.
    for attempt in ["the first read", "the second read"] {
        let page = get_resources(&router, attempt, json!({})).await;
        assert!(
            !page_arns(&page).contains(&arn),
            "{attempt}: an untagged resource must not be in the index, got {page}"
        );
    }

    // And a DELETED Domain is out of the index with no untag at all.
    let (_, gone_arn) = domain(&router, "clinic-deleted", json!({ "env": "prod" })).await;
    let gone_id = gone_arn
        .rsplit('/')
        .next()
        .expect("the ARN names the domain")
        .to_string();
    let (status, body) = connecthealth(&router, "DELETE", &format!("/domain/{gone_id}"), None).await;
    assert_eq!(status, 200, "DeleteDomain got {status}: {body}");
    let page = get_resources(&router, "after the domain was deleted", json!({})).await;
    assert!(
        !page_arns(&page).contains(&gone_arn),
        "a deleted Domain must leave the index, got {page}"
    );
}

#[tokio::test]
async fn this_api_writes_into_the_owning_services_own_tag_set() {
    let router = emulator();
    let (_, arn) = domain(&router, "clinic-written-through", Value::Null).await;

    // One row per tag write this API performs, each asserted through the OWNING service's
    // own reader: the set this API wrote is the set that service holds.
    let (status, written) = tagging(
        &router,
        "TagResources",
        json!({ "ResourceARNList": [arn], "Tags": { "env": "prod", "empty": "" } }),
    )
    .await;
    assert_eq!(status, 200, "TagResources got {status}: {written}");
    assert_eq!(
        written["FailedResourcesMap"].as_object().map(|map| map.len()),
        Some(0),
        "no resource may fail, got {written}"
    );
    assert_eq!(
        owner_tags(&router, &arn).await,
        BTreeMap::from([
            ("env".to_string(), "prod".to_string()),
            ("empty".to_string(), String::new()),
        ]),
        "the owning service must hold what this API wrote"
    );

    // And a removal through this API removes from that same set.
    let (status, removed) = tagging(
        &router,
        "UntagResources",
        json!({ "ResourceARNList": [arn], "TagKeys": ["empty"] }),
    )
    .await;
    assert_eq!(status, 200, "UntagResources got {status}: {removed}");
    assert_eq!(
        owner_tags(&router, &arn).await,
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        "the owning service must hold what this API left"
    );
}

#[tokio::test]
async fn an_arn_the_owning_service_holds_no_resource_for_fails_with_its_own_refusal() {
    let router = emulator();
    let (_, live) = domain(&router, "clinic-live", Value::Null).await;

    // One row per shape of `health-agent` ARN that names nothing: an id no Domain was ever
    // created under, and a Subscription under a Domain that does exist.
    let account = emulator_server::emulator_account_id();
    let ghosts: Vec<(&str, String)> = vec![
        (
            "a domain id nothing was created under",
            format!("arn:aws:{HEALTH_AGENT}:{REGION}:{account}:domain/dom-0123456789abcdef0123"),
            ),
        (
            "a subscription under a live domain that holds none",
            format!("{live}/subscription/sub-000000000000000000000"),
        ),
    ];

    for (case, ghost) in &ghosts {
        let (status, answered) = tagging(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [ghost, &live], "Tags": { "env": "prod" } }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: TagResources got {status}: {answered}");
        let failure = answered["FailedResourcesMap"]
            .get(ghost)
            .unwrap_or_else(|| panic!("case {case:?}: {ghost} must have failed, got {answered}"));
        assert_eq!(
            (
                failure["StatusCode"].as_i64(),
                failure["ErrorCode"].as_str()
            ),
            (Some(404), Some("ResourceNotFoundException")),
            "case {case:?}: the failure carries the owning service's own refusal, got \
             {failure}"
        );
        assert_eq!(
            failure["ErrorMessage"].as_str(),
            Some(
                owner_connecthealth::service::error_detail::resource_not_found_message(ghost)
                    .as_str()
            ),
            "case {case:?}: and that service's own sentence, got {failure}"
        );
        // Nothing was created for it, while the ARN beside it in the same batch was
        // tagged: that is what tells a per-resource failure from a resource this API
        // brought into being.
        let page = get_resources(&router, case, json!({})).await;
        assert!(
            !page_arns(&page).contains(ghost) && page_arns(&page).contains(&live),
            "case {case:?}: the absent ARN must not enter the index and the live one must \
             be in it, got {page}"
        );
    }
}
