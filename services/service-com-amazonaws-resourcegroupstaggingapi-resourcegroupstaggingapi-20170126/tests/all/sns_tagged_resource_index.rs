// SPDX-License-Identifier: CC0-1.0

//! Amazon SNS as an OWNING service of this API: a topic holds its own tag set, so
//! that set — not a second copy here — is what `GetResources` reports, what
//! `TagResources` writes and what `UntagResources` removes from, and a well-formed
//! topic ARN that service holds no topic for is this ARN's own
//! `FailedResourcesMap` entry carrying THAT service's refusal rather than a
//! silently created resource.
//!
//! Every test drives the real wire path over ONE engine carrying both services,
//! which is how the emulator runs: `POST /` with an `X-Amz-Target` and an awsJson
//! body for this API, and an awsQuery form body for Amazon SNS.
//!
//! The tables span the class the rule generalises over: a topic that was never
//! created, one whose name only resembles a live one, a FIFO topic, and one the
//! owning service DELETED after it had been tagged — and, on the writing side, how
//! many tags the request carries, a tag with an empty value, and which of the two
//! APIs wrote them. On the READING side they span the filter forms `TagFilters`
//! admits — a key with one value, with several, with none at all, a value or a key
//! no topic carries, and two filters AND-ed — over an estate of topics that share a
//! key with different values, so a filter selects a topic by its OWN value and a
//! filter nothing satisfies selects nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use service_com_amazonaws_sns_amazonsimplenotificationservice as owner_sns;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML readers this crate's suites share, so an
// owning service's answer is scraped one way for every suite.
use crate::elb_tagged_resource_index::{element, encode, require, text};

/// The Region every request below is signed for, and therefore the one both
/// endpoints serve and the one the ARNs Amazon SNS mints belong to.
const REGION: &str = "us-east-1";

/// The SigV4 signing name this API is routed on.
const TAGGING: &str = "tagging";

/// The SigV4 signing name Amazon SNS is routed on.
const SNS: &str = "sns";

/// The API version every awsQuery envelope below names: the version the SNS model
/// declares, which that service's own test harness posts too.
const SNS_VERSION: &str = "2010-03-31";

/// A router carrying this API and Amazon SNS over ONE engine — the arrangement
/// `core/router` builds.
///
/// `pub(crate)` because a sibling suite that needs a topic Amazon SNS really holds
/// stands this arrangement up rather than carrying a second copy of it
/// ([`crate::tag_batch_partial_failure`]).
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization` header
    // below is present because it is what routes a request to a signing name, not
    // to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_sns::EmulatedService::new(engine));
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
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{target} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// One Amazon SNS operation, over the awsQuery wire that model publishes. The form
/// encoder is this crate's ONE copy ([`encode`]), so an awsQuery body built here is
/// escaped exactly as every other suite's.
async fn sns(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version={SNS_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&encode(name));
        body.push('=');
        body.push_str(&encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://sns.amazonaws.com/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(SNS))
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One Amazon SNS operation that must succeed.
async fn sns_ok(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = sns(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {action}{params:?}: expected 200 from Amazon SNS, got {status} with body {body}"
    );
    body
}

/// A topic created through Amazon SNS's own API, with the ARN that service minted.
///
/// A name ending in `.fifo` names a FIFO topic, and Amazon SNS creates one only for a
/// request that ASKS for that kind — "the fifo suffix belongs to a FIFO topic and to no
/// other", so the name and the `FifoTopic` attribute have to agree (SNS refuses the
/// pair that disagrees with `Invalid parameter: Topic Name`). The attribute is therefore
/// read off the name rather than passed by every caller.
///
/// `pub(crate)` for the same reason [`emulator`] is.
pub(crate) async fn topic(router: &MultiServiceRouter, name: &str) -> String {
    let mut params: Vec<(&str, &str)> = vec![("Name", name)];
    if name.ends_with(".fifo") {
        params.push(("Attributes.entry.1.key", "FifoTopic"));
        params.push(("Attributes.entry.1.value", "true"));
    }
    require(&sns_ok(router, "CreateTopic", &params).await, "TopicArn")
}

/// The tag map Amazon SNS's own `ListTagsForResource` reports for one ARN.
///
/// A list member's open tag carries the envelope's default namespace
/// (`<member xmlns="http://sns.amazonaws.com/doc/2010-03-31/">`), so the opening is
/// matched up to its first `>` rather than as a bare `<member>`.
///
/// `pub(crate)` for the same reason [`emulator`] is: a sibling suite reads the tag set a
/// topic holds through the OWNING service's own reader, not through a second scraper.
pub(crate) async fn sns_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = sns_ok(router, "ListTagsForResource", &[("ResourceArn", arn)]).await;
    let Some(block) = element(&body, "Tags") else {
        return BTreeMap::new();
    };
    let mut found = BTreeMap::new();
    let mut rest = block.as_str();
    while let Some(start) = rest.find("<member") {
        rest = &rest[start..];
        let Some(open) = rest.find('>') else { break };
        rest = &rest[open + 1..];
        let Some(end) = rest.find("</member>") else {
            break;
        };
        let member = &rest[..end];
        if let Some(key) = text(member, "Key") {
            found.insert(key, text(member, "Value").unwrap_or_default());
        }
        rest = &rest[end + "</member>".len()..];
    }
    found
}

/// The ARNs of a `GetResources` page, in the order the service returned them.
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

/// Everything this API reports for the serving Region.
async fn whole_index(router: &MultiServiceRouter) -> Value {
    let (status, body) = tagging(router, "GetResources", json!({})).await;
    assert_eq!(status, 200, "GetResources: body {body}");
    body
}

/// A topic ARN of the form Amazon SNS mints, for a topic that was never created.
fn ghost_arn(name: &str) -> String {
    format!(
        "arn:aws:sns:{REGION}:{}:{name}",
        emulator_server::emulator_account_id()
    )
}

/// A well-formed topic ARN naming a topic Amazon SNS holds no record of is answered
/// per RESOURCE with that service's own refusal — its `ResourceNotFoundException`
/// as the wire carries it, code `ResourceNotFound` with `StatusCode` 404 and that
/// service's own sentence — while the other ARNs of the same batch are tagged.
///
/// And nothing is created for it: the ARN is absent from this API's index
/// afterwards, which is what tells a per-resource failure from a resource the
/// tagging store brought into being.
#[tokio::test]
async fn an_absent_sns_topic_fails_with_amazon_sns_own_refusal_and_creates_nothing() {
    let router = emulator();
    let live = topic(&router, "sns-owner-live").await;

    let ghosts: &[(&str, String)] = &[
        (
            "a topic that was never created",
            ghost_arn("sns-owner-ghost"),
        ),
        (
            "a name that only resembles a live one",
            ghost_arn("sns-owner-live-2"),
        ),
        (
            "a name differing from a live one in case alone",
            ghost_arn("SNS-OWNER-LIVE"),
        ),
        (
            "a FIFO topic that was never created",
            ghost_arn("sns-owner-ghost.fifo"),
        ),
    ];

    for (case, arn) in ghosts {
        let (status, body) = tagging(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [arn, &live], "Tags": { "Project": "project1" } }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResources body {body}");
        let failure = &body["FailedResourcesMap"][arn.as_str()];
        assert_eq!(
            failure["ErrorCode"],
            json!("ResourceNotFound"),
            "case {case}: expected ResourceNotFound for {arn}, got {}",
            body["FailedResourcesMap"]
        );
        assert_eq!(
            failure["StatusCode"],
            json!(404),
            "case {case}: expected StatusCode 404 for {arn}, got {failure}"
        );
        assert_eq!(
            body["FailedResourcesMap"]
                .as_object()
                .map(serde_json::Map::len),
            Some(1),
            "case {case}: the live ARN of the batch must be tagged, got {}",
            body["FailedResourcesMap"]
        );

        // The SAME refusal Amazon SNS's own tag write answers for that ARN: one
        // code and one sentence, composed by the owning service.
        let (own_status, own) = sns(
            &router,
            "TagResource",
            &[
                ("ResourceArn", arn),
                ("Tags.member.1.Key", "Project"),
                ("Tags.member.1.Value", "project1"),
            ],
        )
        .await;
        assert_eq!(
            own_status, 404,
            "case {case}: TagResource({arn}) body {own}"
        );
        assert_eq!(
            require(&own, "Code"),
            "ResourceNotFound",
            "case {case}: expected TagResource({arn}) to answer ResourceNotFound, got {own}"
        );
        assert_eq!(
            failure["ErrorMessage"],
            json!(require(&own, "Message")),
            "case {case}: expected the per-resource failure to carry Amazon SNS's own \
             sentence, got {failure}"
        );

        // Nothing was created for the absent ARN.
        let index = whole_index(&router).await;
        assert!(
            !page_arns(&index).contains(arn),
            "case {case}: a failed resource must not appear in the index, got {:?}",
            page_arns(&index)
        );
    }
}

/// A topic Amazon SNS holds is tagged through EITHER API into the SAME tag set: a
/// tag written here is reported by that service's own `ListTagsForResource`, a tag
/// written there is reported by `GetResources`, and `UntagResources` removes from
/// that one set.
#[tokio::test]
async fn a_live_topic_is_tagged_through_either_api_into_one_tag_set() {
    let router = emulator();

    // Columns: the case, the topic's name, and the tags this API writes to it.
    /// One row of the writing table: the case, the topic's name, and the tags.
    type Row = (&'static str, &'static str, &'static [(&'static str, &'static str)]);
    let rows: &[Row] = &[
        ("one tag", "sns-owner-one", &[("Env", "prod")]),
        (
            "three tags",
            "sns-owner-three",
            &[("Env", "prod"), ("Team", "payments"), ("Tier", "gold")],
        ),
        ("a tag with an empty value", "sns-owner-empty", &[("Env", "")]),
        ("a FIFO topic", "sns-owner-fifo.fifo", &[("Env", "prod")]),
    ];

    for (case, name, tags) in rows {
        let arn = topic(&router, name).await;
        let written: Value = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<serde_json::Map<String, Value>>()
            .into();

        let (status, body) = tagging(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [&arn], "Tags": written }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResources body {body}");
        assert_eq!(
            body["FailedResourcesMap"],
            json!({}),
            "case {case}: expected no failed resource, got {body}"
        );

        // The owning service's own read: the tags this API wrote are the topic's.
        let expected: BTreeMap<String, String> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let held = sns_tags(&router, &arn).await;
        assert_eq!(
            held, expected,
            "case {case}: expected Amazon SNS to hold {expected:?}, got {held:?}"
        );

        // A tag written through the OWNING service is in this API's answers at once.
        let _ = sns_ok(
            &router,
            "TagResource",
            &[
                ("ResourceArn", arn.as_str()),
                ("Tags.member.1.Key", "Owner"),
                ("Tags.member.1.Value", "platform"),
            ],
        )
        .await;
        let index = whole_index(&router).await;
        let reported = page_tags(&index, &arn);
        let mut both = expected.clone();
        both.insert("Owner".to_string(), "platform".to_string());
        assert_eq!(
            reported, both,
            "case {case}: expected GetResources to report {both:?}, got {reported:?}"
        );

        // And an untag through this API removes from that same one set.
        let (status, body) = tagging(
            &router,
            "UntagResources",
            json!({ "ResourceARNList": [&arn], "TagKeys": ["Owner"] }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: UntagResources body {body}");
        let left = sns_tags(&router, &arn).await;
        assert_eq!(
            left, expected,
            "case {case}: expected the untag to leave {expected:?}, got {left:?}"
        );
    }
}

/// A topic created with `Tags`, through Amazon SNS's own `CreateTopic`, with the ARN
/// that service minted for it.
///
/// The awsQuery form carries a tag list as `Tags.member.<n>.Key`/`.Value`, which is
/// the spelling the SNS model's `TagList` is serialized under.
async fn tagged_topic(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut fields: Vec<(String, String)> = vec![("Name".to_string(), name.to_string())];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        fields.push((format!("Tags.member.{position}.Key"), (*key).to_string()));
        fields.push((format!("Tags.member.{position}.Value"), (*value).to_string()));
    }
    let borrowed: Vec<(&str, &str)> = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    require(&sns_ok(router, "CreateTopic", &borrowed).await, "TopicArn")
}

/// The ARNs one `TagFilters` request selects, sorted, with the answer asserted to be
/// the modelled success — so a filter that selects nothing is read as the empty
/// `ResourceTagMappingList` it is and not as a refusal.
async fn selected(router: &MultiServiceRouter, case: &str, filters: Value) -> Vec<String> {
    let (status, body) = tagging(
        router,
        "GetResources",
        json!({ "TagFilters": filters.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetResources with TagFilters {filters} must answer 200, got {status} \
         with body {body}"
    );
    let mut arns = page_arns(&body);
    arns.sort();
    arns
}

/// `TagFilters` selects a topic when, for EVERY filter, the topic's tag set holds that
/// filter's key with one of the values the filter lists — any value when it lists none
/// ("If you specify more than one filter in a single request, the response returns only
/// those resources that satisfy all filters"; "if you specify a filter that contains more
/// than one value for a key, the response returns resources that match any of the
/// specified values for that key"; "if you don't specify a value for a key, the response
/// returns all resources that are tagged with that key, with any or no value" —
/// `GetResourcesInput$TagFilters`). So each of several topics sharing one key is selected
/// by its OWN value and by no other, and a key or value nothing carries selects nothing:
/// a 200 whose `ResourceTagMappingList` is empty.
///
/// The estate spans what the rule generalises over: how the tag reached the topic (a
/// create's `Tags`, a later `TagResource`), how many keys the topic carries, a topic
/// sharing the key with another value, a topic carrying the same VALUE under another key,
/// and a topic nothing ever tagged ("GetResources does not return untagged resources").
#[tokio::test]
async fn tag_filters_select_the_topics_whose_own_tag_set_satisfies_every_filter() {
    let router = emulator();

    // One key the estate shares, so one query reaches several topics, and the two
    // values that tell them apart.
    let key = "Run";
    let one = "run-one";
    let two = "run-two";

    let created = tagged_topic(&router, "sns-filter-created", &[(key, one)]).await;
    let different = tagged_topic(&router, "sns-filter-different", &[(key, two)]).await;
    let both = tagged_topic(&router, "sns-filter-both", &[(key, one), ("Stage", one)]).await;
    let other_key = tagged_topic(&router, "sns-filter-other-key", &[("Stage", one)]).await;
    let untagged = topic(&router, "sns-filter-untagged").await;

    // Tagged through the OWNING service AFTER its create, which is the same one tag
    // set read from the other side.
    let written = topic(&router, "sns-filter-written").await;
    let _ = sns_ok(
        &router,
        "TagResource",
        &[
            ("ResourceArn", written.as_str()),
            ("Tags.member.1.Key", key),
            ("Tags.member.1.Value", one),
        ],
    )
    .await;

    let mut key_is_one = vec![created.clone(), both.clone(), written.clone()];
    key_is_one.sort();
    let mut key_any_value = vec![
        created.clone(),
        both.clone(),
        written.clone(),
        different.clone(),
    ];
    key_any_value.sort();

    // Columns: the case, the `TagFilters` value, and the ARNs it selects.
    let rows: &[(&str, Value, Vec<String>)] = &[
        (
            "the key and the value one topic set",
            json!([{ "Key": key, "Values": [one] }]),
            key_is_one.clone(),
        ),
        (
            "the key and the OTHER value",
            json!([{ "Key": key, "Values": [two] }]),
            vec![different.clone()],
        ),
        (
            "both values of the key, which are OR-ed",
            json!([{ "Key": key, "Values": [one, two] }]),
            key_any_value.clone(),
        ),
        (
            "the key with no values, which matches any value",
            json!([{ "Key": key }]),
            key_any_value.clone(),
        ),
        (
            "the key with an empty value list, which matches any value",
            json!([{ "Key": key, "Values": [] }]),
            key_any_value,
        ),
        (
            "a value no topic was ever tagged with",
            json!([{ "Key": key, "Values": ["run-three"] }]),
            Vec::new(),
        ),
        (
            "a key no topic carries, with a value one of them does",
            json!([{ "Key": "Owner", "Values": [one] }]),
            Vec::new(),
        ),
        (
            "two filters, which are AND-ed",
            json!([{ "Key": key, "Values": [one] }, { "Key": "Stage", "Values": [one] }]),
            vec![both.clone()],
        ),
        (
            "two filters the same topic cannot satisfy",
            json!([{ "Key": key, "Values": [two] }, { "Key": "Stage", "Values": [one] }]),
            Vec::new(),
        ),
        (
            "the other key alone",
            json!([{ "Key": "Stage" }]),
            {
                let mut both_keys = vec![both.clone(), other_key.clone()];
                both_keys.sort();
                both_keys
            },
        ),
    ];

    for (case, filters, expected) in rows {
        let answer = selected(&router, case, filters.clone()).await;
        assert_eq!(
            &answer, expected,
            "case {case:?}: expected TagFilters {filters} to select {expected:?}, got {answer:?}"
        );
        assert!(
            !answer.contains(&untagged),
            "case {case:?}: a topic nothing ever tagged is in no answer, got {answer:?}"
        );
    }
}

/// An untag through the OWNING service withdraws exactly that topic from the queries
/// its tag answered: the tag set the index reports is the topic's own, so removing the
/// key there removes the topic from every filter naming it, leaves the siblings that
/// still carry it selected, and leaves a filter nothing satisfies answering 200 with an
/// empty `ResourceTagMappingList`.
#[tokio::test]
async fn an_untag_through_amazon_sns_withdraws_only_that_topic_from_its_tag_query() {
    let router = emulator();
    let key = "Run";
    let value = "run-one";

    let withdrawn = tagged_topic(&router, "sns-untag-withdrawn", &[(key, value)]).await;
    let sibling = tagged_topic(&router, "sns-untag-sibling", &[(key, value)]).await;
    let mut both = vec![withdrawn.clone(), sibling.clone()];
    both.sort();

    let filter = json!([{ "Key": key, "Values": [value] }]);
    let answer = selected(&router, "both topics tagged", filter.clone()).await;
    assert_eq!(
        answer, both,
        "case both topics tagged: expected {both:?}, got {answer:?}"
    );

    // Columns: the case, the topic untagged through Amazon SNS, and what the filter
    // selects afterwards.
    let rows: &[(&str, &String, Vec<String>)] = &[
        (
            "one of the two untagged",
            &withdrawn,
            vec![sibling.clone()],
        ),
        ("both untagged", &sibling, Vec::new()),
    ];

    for (case, arn, expected) in rows {
        let _ = sns_ok(
            &router,
            "UntagResource",
            &[("ResourceArn", arn.as_str()), ("TagKeys.member.1", key)],
        )
        .await;
        let answer = selected(&router, case, filter.clone()).await;
        assert_eq!(
            &answer, expected,
            "case {case:?}: expected the filter to select {expected:?} after untagging \
             {arn}, got {answer:?}"
        );
        // The owning service's own reader agrees: there is ONE tag set, and the untag
        // emptied it.
        let held = sns_tags(&router, arn).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected Amazon SNS to hold no tag for {arn}, got {held:?}"
        );
    }
}

/// A topic DELETED through Amazon SNS after it was tagged leaves this API's index
/// with no untag, and tagging it again is the per-resource not-found failure — the
/// same answer an ARN that never existed earns, because existence is the owning
/// service's answer and not a record of this API's own.
#[tokio::test]
async fn a_topic_deleted_through_amazon_sns_leaves_the_index_and_cannot_be_tagged() {
    let router = emulator();
    let arn = topic(&router, "sns-owner-transient").await;

    let (status, body) = tagging(
        &router,
        "TagResources",
        json!({ "ResourceARNList": [&arn], "Tags": { "Env": "prod" } }),
    )
    .await;
    assert_eq!(status, 200, "case tag the live topic: body {body}");
    let index = whole_index(&router).await;
    assert!(
        page_arns(&index).contains(&arn),
        "case tag the live topic: expected {arn} in the index, got {:?}",
        page_arns(&index)
    );

    let _ = sns_ok(&router, "DeleteTopic", &[("TopicArn", &arn)]).await;

    let index = whole_index(&router).await;
    assert!(
        !page_arns(&index).contains(&arn),
        "case the deleted topic: expected {arn} to leave the index, got {:?}",
        page_arns(&index)
    );

    let (status, body) = tagging(
        &router,
        "TagResources",
        json!({ "ResourceARNList": [&arn], "Tags": { "Env": "prod" } }),
    )
    .await;
    assert_eq!(status, 200, "case tag the deleted topic: body {body}");
    assert_eq!(
        body["FailedResourcesMap"][arn.as_str()]["ErrorCode"],
        json!("ResourceNotFound"),
        "case tag the deleted topic: expected the per-resource not-found failure, got {body}"
    );
    assert_eq!(
        body["FailedResourcesMap"][arn.as_str()]["StatusCode"],
        json!(404),
        "case tag the deleted topic: expected StatusCode 404, got {body}"
    );
}
