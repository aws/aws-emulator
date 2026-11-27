// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tag read/write loop: `TagResources` and
//! `UntagResources` write the ARN -> tags store, and `GetResources`,
//! `GetTagKeys` and `GetTagValues` are pure projections of it.
//!
//! Every test drives the service over the real wire path — the multi-service
//! router, `POST /` with `X-Amz-Target: ResourceGroupsTaggingAPI_20170126.<Op>`
//! and an awsJson1_1 body — so what is asserted is what an SDK client sees.
//!
//! The last section drives that same loop over the OTHER framing the endpoint
//! serves, Smithy RPC v2 CBOR, which names its operation in the request path and
//! sends no `X-Amz-Target` at all: one store, read and written through either.
//!
//! The tables below span the class each rule generalises over (ARN form, filter
//! shape, page size, token provenance, request framing); the ARNs the issue cited
//! are one row.

use std::collections::BTreeMap;
use std::sync::Arc;

// The CBOR implementation the framework's own codec is built on: the last section
// writes its request bodies and reads its response bodies with it, so what is
// asserted is what a client of that framing sends and reads.
use aws_smithy_cbor::data::Type;
use aws_smithy_cbor::{Decoder, Encoder};
use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over its own fresh engine, so each test
/// starts from an empty store and observes the same state across requests.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization`
    // header below is present because it is what routes an awsJson request to
    // the `tagging` signing name, not to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        // The SigV4 credential scope is the routing signal that names the
        // `tagging` signing name this service claims.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/tagging/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// Tag `arns` with `tags`, asserting the call succeeded with no failed resource.
async fn tag(router: &MultiServiceRouter, arns: &[&str], tags: &[(&str, &str)]) {
    let tags: serde_json::Map<String, serde_json::Value> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), serde_json::json!(value)))
        .collect();
    let (status, body) = call(
        router,
        "TagResources",
        serde_json::json!({ "ResourceARNList": arns, "Tags": tags }),
    )
    .await;
    assert_eq!(status, 200, "TagResources({arns:?}): body {body}");
    assert_eq!(
        body["FailedResourcesMap"],
        serde_json::json!({}),
        "TagResources({arns:?}): expected an empty FailedResourcesMap, got {}",
        body["FailedResourcesMap"]
    );
}

/// The error code an awsJson error body carries (`__type` minus its namespace).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error body with no __type: {body}"));
    raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
}

/// The ARNs of a `GetResources` page, in the order the service returned them.
fn page_arns(body: &serde_json::Value) -> Vec<String> {
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
fn page_tags(body: &serde_json::Value, arn: &str) -> BTreeMap<String, String> {
    let mapping = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == serde_json::json!(arn))
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

/// Every ARN shape the service has to carry, one row per resource-field form.
///
/// Source: the `ResourceTypeFilters` member documentation — "The format of each
/// resource type is `service[:resourceType]` … the same as that embedded in a
/// resource's ARN" (`smithy_query(query="doc", shape="GetResourcesInput",
/// member="ResourceTypeFilters")`). The forms are the AWS ARN resource layouts:
/// bare `resource-id`, `resource-type/resource-id` and `resource-type:resource-id`.
///
/// Every ARN is in the partition and Region this endpoint serves — the test
/// signs for `us-east-1` — because an ARN outside either is refused; that rule
/// has its own rows in `tagging_api_fidelity.rs`. The services named here are
/// the ones `state::owners` does NOT resolve against, so the ARNs need no live
/// resource behind them. A bare id whose type comes from
/// `crate::state::implicit_resource_type` instead of from the ARN belongs to a
/// service this crate IS linked against (a bucket, a queue, a topic), so such a
/// resource has to be standing in its owner before it can be tagged at all: the
/// cross-service suite
/// `core/router/tests/all/s3_bucket_tags_reach_resource_group_discovery.rs` pins
/// that form over a real bucket.
///
/// Columns: the ARN, the filter that selects it by service, and the filter that
/// selects it by `service:resourceType`.
const ARN_FORMS: &[(&str, &str, Option<&str>)] = &[
    // Bare resource id with region and account, under a service the table of
    // implicit resource types does not name — so its type key is the bare service
    // token, which is both its filters.
    (
        "arn:aws:codecommit:us-east-1:123456789012:repo-a",
        "codecommit",
        Some("codecommit"),
    ),
    // `resource-type/resource-id`.
    (
        "arn:aws:ec2:us-east-1:123456789012:instance/i-0abc",
        "ec2",
        Some("ec2:instance"),
    ),
    // `resource-type/resource-id` with a multi-segment id.
    (
        "arn:aws:ssm:us-east-1:123456789012:parameter/orders/dev/token",
        "ssm",
        Some("ssm:parameter"),
    ),
    // `resource-type:resource-id`.
    (
        "arn:aws:lex:us-east-1:123456789012:bot:packer",
        "lex",
        Some("lex:bot"),
    ),
    // A global resource: no region segment at all.
    (
        "arn:aws:iam::123456789012:user/zoe",
        "iam",
        Some("iam:user"),
    ),
    // A type segment the ARN DOES carry, under a service whose primary resource
    // has an implicit type: the ARN's own type wins over the implicit one.
    (
        "arn:aws:s3:us-east-1:123456789012:accesspoint/ap-a",
        "s3",
        Some("s3:accesspoint"),
    ),
];

#[tokio::test]
async fn get_resources_returns_exactly_what_tag_resources_wrote() {
    let router = emulator();
    let arns: Vec<&str> = ARN_FORMS.iter().map(|(arn, ..)| *arn).collect();

    // One tag call per ARN, each with its own value, so the read cannot pass by
    // echoing a single request.
    for (index, arn) in arns.iter().enumerate() {
        tag(
            &router,
            &[arn],
            &[("Env", "prod"), ("Index", &index.to_string())],
        )
        .await;
    }

    let (status, body) = call(&router, "GetResources", serde_json::json!({})).await;
    assert_eq!(status, 200, "GetResources: body {body}");

    let mut expected: Vec<String> = arns.iter().map(|arn| (*arn).to_string()).collect();
    expected.sort();
    assert_eq!(
        page_arns(&body),
        expected,
        "GetResources with no filters must report every tagged ARN, in ARN order; got {body}"
    );

    for (index, arn) in arns.iter().enumerate() {
        let tags = page_tags(&body, arn);
        let want: BTreeMap<String, String> = [
            ("Env".to_string(), "prod".to_string()),
            ("Index".to_string(), index.to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            tags, want,
            "case {arn}: expected {want:?}, got {tags:?}"
        );
    }

    // A read is a read: the same call twice answers the same thing.
    let (_, again) = call(&router, "GetResources", serde_json::json!({})).await;
    assert_eq!(
        again, body,
        "GetResources changed what the next caller sees: {again} vs {body}"
    );
}

#[tokio::test]
async fn resource_type_filters_match_the_service_and_resource_type_of_the_arn() {
    let router = emulator();
    let arns: Vec<&str> = ARN_FORMS.iter().map(|(arn, ..)| *arn).collect();
    tag(&router, &arns, &[("Env", "prod")]).await;

    // Every row of the catalog: the `service:resourceType` filter selects
    // exactly its own ARN, and the bare service filter selects every ARN of
    // that service.
    for (arn, service_filter, type_filter) in ARN_FORMS {
        let cases: Vec<(&str, Vec<String>)> = vec![
            (
                *service_filter,
                ARN_FORMS
                    .iter()
                    .filter(|(_, service, _)| service == service_filter)
                    .map(|(other, ..)| (*other).to_string())
                    .collect(),
            ),
            (
                type_filter.expect("every row names its qualified filter"),
                vec![(*arn).to_string()],
            ),
        ];
        for (filter, expected) in cases {
            let (status, body) = call(
                &router,
                "GetResources",
                serde_json::json!({ "ResourceTypeFilters": [filter] }),
            )
            .await;
            assert_eq!(status, 200, "case ResourceTypeFilters=[{filter}]: body {body}");
            let mut want = expected;
            want.sort();
            assert_eq!(
                page_arns(&body),
                want,
                "case ResourceTypeFilters=[{filter}]: expected {want:?}, got {}",
                body["ResourceTagMappingList"]
            );
        }
    }

    // A `service:resourceType` filter whose type no ARN carries matches nothing.
    // A filter naming no supported service at all is a REFUSAL rather than an
    // empty answer; its rows live in `resource_type_filter_spelling.rs`.
    for filter in ["s3:object", "sns:subscription", "ec2:volume"] {
        let (status, body) = call(
            &router,
            "GetResources",
            serde_json::json!({ "ResourceTypeFilters": [filter] }),
        )
        .await;
        assert_eq!(status, 200, "case ResourceTypeFilters=[{filter}]: body {body}");
        assert!(
            page_arns(&body).is_empty(),
            "case ResourceTypeFilters=[{filter}]: expected no resources, got {}",
            body["ResourceTagMappingList"]
        );
    }

    // Several filters select the union of what each selects.
    let (status, body) = call(
        &router,
        "GetResources",
        serde_json::json!({ "ResourceTypeFilters": ["s3:accesspoint", "ec2:instance"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ResourceTypeFilters=[s3:accesspoint, ec2:instance]: body {body}"
    );
    assert_eq!(
        page_arns(&body),
        vec![
            "arn:aws:ec2:us-east-1:123456789012:instance/i-0abc".to_string(),
            "arn:aws:s3:us-east-1:123456789012:accesspoint/ap-a".to_string(),
        ],
        "case ResourceTypeFilters=[s3:accesspoint, ec2:instance]: got {}",
        body["ResourceTagMappingList"]
    );
}

#[tokio::test]
async fn tag_filters_and_across_keys_and_or_within_one_filters_values() {
    let router = emulator();
    // Three resources whose tag sets differ in value, in key set, and in count.
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    let beta = "arn:aws:codecommit:us-east-1:123456789012:beta";
    let gamma = "arn:aws:ec2:us-east-1:123456789012:instance/gamma";
    tag(&router, &[alpha], &[("Env", "prod"), ("Team", "core")]).await;
    tag(&router, &[beta], &[("Env", "dev"), ("Team", "core")]).await;
    tag(&router, &[gamma], &[("Env", "prod"), ("Owner", "")]).await;

    // Rows differ along: value match, value miss, multi-value OR, key-only
    // (any-or-no value), several filters AND-ed, and a key nobody uses.
    let cases: &[(serde_json::Value, Vec<&str>)] = &[
        (serde_json::json!([{ "Key": "Env", "Values": ["prod"] }]), vec![gamma, alpha]),
        (serde_json::json!([{ "Key": "Env", "Values": ["dev"] }]), vec![beta]),
        (serde_json::json!([{ "Key": "Env", "Values": ["staging"] }]), vec![]),
        (
            serde_json::json!([{ "Key": "Env", "Values": ["prod", "dev"] }]),
            vec![beta, gamma, alpha],
        ),
        (serde_json::json!([{ "Key": "Env" }]), vec![beta, gamma, alpha]),
        (serde_json::json!([{ "Key": "Env", "Values": [] }]), vec![beta, gamma, alpha]),
        (serde_json::json!([{ "Key": "Owner", "Values": [""] }]), vec![gamma]),
        (
            serde_json::json!([
                { "Key": "Env", "Values": ["prod"] },
                { "Key": "Team", "Values": ["core"] }
            ]),
            vec![alpha],
        ),
        (
            serde_json::json!([
                { "Key": "Env", "Values": ["prod"] },
                { "Key": "Nothing" }
            ]),
            vec![],
        ),
        (serde_json::json!([{ "Key": "Nothing" }]), vec![]),
        (serde_json::json!([]), vec![beta, gamma, alpha]),
    ];

    for (filters, expected) in cases {
        let (status, body) = call(
            &router,
            "GetResources",
            serde_json::json!({ "TagFilters": filters }),
        )
        .await;
        assert_eq!(status, 200, "case TagFilters={filters}: body {body}");
        let mut want: Vec<String> = expected.iter().map(|arn| (*arn).to_string()).collect();
        want.sort();
        assert_eq!(
            page_arns(&body),
            want,
            "case TagFilters={filters}: expected {want:?}, got {:?}",
            page_arns(&body)
        );
    }
}

#[tokio::test]
async fn tag_resources_upserts_and_untag_resources_removes_only_the_named_keys() {
    let router = emulator();
    let arn = "arn:aws:lex:us-east-1:123456789012:bot:group-a";
    let other = "arn:aws:codecommit:us-east-1:123456789012:repo-a";
    tag(&router, &[arn, other], &[("Env", "prod"), ("Team", "core")]).await;

    // Rows differ in what the second write does to the stored map: overwrite one
    // key, add a key, and both at once.
    /// One upsert case: the tags written, and the tag map expected afterwards.
    type UpsertCase<'a> = (&'a [(&'a str, &'a str)], &'a [(&'a str, &'a str)]);
    let cases: &[UpsertCase<'_>] = &[
        (
            &[("Env", "staging")],
            &[("Env", "staging"), ("Team", "core")],
        ),
        (
            &[("Cost", "42")],
            &[("Cost", "42"), ("Env", "staging"), ("Team", "core")],
        ),
        (
            &[("Env", "prod"), ("New", "yes")],
            &[
                ("Cost", "42"),
                ("Env", "prod"),
                ("New", "yes"),
                ("Team", "core"),
            ],
        ),
    ];
    for (write, expected) in cases {
        tag(&router, &[arn], write).await;
        let (_, body) = call(
            &router,
            "GetResources",
            serde_json::json!({ "ResourceARNList": [arn] }),
        )
        .await;
        let want: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let got = page_tags(&body, arn);
        assert_eq!(got, want, "case upsert {write:?}: expected {want:?}, got {got:?}");
    }

    // The resource tagged alongside it was never touched by those upserts.
    let (_, body) = call(
        &router,
        "GetResources",
        serde_json::json!({ "ResourceARNList": [other] }),
    )
    .await;
    let got = page_tags(&body, other);
    assert_eq!(
        got,
        [
            ("Env".to_string(), "prod".to_string()),
            ("Team".to_string(), "core".to_string())
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>(),
        "case untouched {other}: got {got:?}"
    );

    // Untagging removes exactly the named keys, one row per key count.
    let removals: &[(&[&str], &[&str])] = &[
        (&["Env"], &["Cost", "New", "Team"]),
        (&["Cost", "New"], &["Team"]),
        (&["Nothing"], &["Team"]),
        (&["Team"], &[]),
    ];
    for (keys, expected) in removals {
        let (status, body) = call(
            &router,
            "UntagResources",
            serde_json::json!({ "ResourceARNList": [arn], "TagKeys": keys }),
        )
        .await;
        assert_eq!(status, 200, "case UntagResources({keys:?}): body {body}");
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case UntagResources({keys:?}): expected an empty FailedResourcesMap, got {}",
            body["FailedResourcesMap"]
        );
        let (_, read) = call(
            &router,
            "GetResources",
            serde_json::json!({ "ResourceARNList": [arn] }),
        )
        .await;
        let got: Vec<String> = page_tags(&read, arn).into_keys().collect();
        assert_eq!(
            got,
            expected.iter().map(|k| (*k).to_string()).collect::<Vec<_>>(),
            "case UntagResources({keys:?}): expected keys {expected:?}, got {got:?}"
        );
    }

    // A resource whose last key was removed is still reported, with no tags.
    let (_, read) = call(&router, "GetResources", serde_json::json!({})).await;
    assert!(
        page_tags(&read, arn).is_empty(),
        "an untagged-to-empty resource must report an empty tag set, got {read}"
    );

    // And a well-formed ARN nothing ever tagged is a no-op success.
    let (status, body) = call(
        &router,
        "UntagResources",
        serde_json::json!({
            "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:never-tagged"],
            "TagKeys": ["Env"]
        }),
    )
    .await;
    assert_eq!(status, 200, "case untag an untagged ARN: body {body}");
    assert_eq!(
        body["FailedResourcesMap"],
        serde_json::json!({}),
        "case untag an untagged ARN: expected an empty FailedResourcesMap, got {body}"
    );
}

#[tokio::test]
async fn get_tag_keys_and_values_are_projections_of_the_store() {
    let router = emulator();
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    let beta = "arn:aws:codecommit:us-east-1:123456789012:beta";
    tag(&router, &[alpha], &[("Env", "prod"), ("Team", "core")]).await;
    tag(&router, &[beta], &[("Env", "dev"), ("Zone", "a")]).await;

    let (status, body) = call(&router, "GetTagKeys", serde_json::json!({})).await;
    assert_eq!(status, 200, "GetTagKeys: body {body}");
    assert_eq!(
        body["TagKeys"],
        serde_json::json!(["Env", "Team", "Zone"]),
        "GetTagKeys must report the distinct keys of the store, sorted; got {body}"
    );

    // Rows differ in how many resources carry the key, how many distinct values
    // it has, and whether anything uses it at all.
    let cases: &[(&str, Vec<&str>)] = &[
        ("Env", vec!["dev", "prod"]),
        ("Team", vec!["core"]),
        ("Zone", vec!["a"]),
        ("Unused", vec![]),
    ];
    for (key, expected) in cases {
        let (status, body) = call(
            &router,
            "GetTagValues",
            serde_json::json!({ "Key": key }),
        )
        .await;
        assert_eq!(status, 200, "case GetTagValues(Key={key}): body {body}");
        assert_eq!(
            body["TagValues"],
            serde_json::json!(expected),
            "case GetTagValues(Key={key}): expected {expected:?}, got {}",
            body["TagValues"]
        );
    }

    // Removing a key from every resource that used it removes it from both reads.
    for arn in [alpha, beta] {
        let (status, _) = call(
            &router,
            "UntagResources",
            serde_json::json!({ "ResourceARNList": [arn], "TagKeys": ["Env"] }),
        )
        .await;
        assert_eq!(status, 200, "UntagResources({arn}, [Env])");
    }
    let (_, body) = call(&router, "GetTagKeys", serde_json::json!({})).await;
    assert_eq!(
        body["TagKeys"],
        serde_json::json!(["Team", "Zone"]),
        "a key no resource carries must disappear from GetTagKeys; got {body}"
    );
    let (_, body) = call(&router, "GetTagValues", serde_json::json!({ "Key": "Env" })).await;
    assert_eq!(
        body["TagValues"],
        serde_json::json!([]),
        "a key no resource carries must have no values; got {body}"
    );
}

#[tokio::test]
async fn resources_per_page_walks_every_resource_exactly_once() {
    let router = emulator();
    let arns: Vec<String> = (0..7)
        .map(|n| format!("arn:aws:lex:us-east-1:123456789012:bot:group-{n}"))
        .collect();
    let borrowed: Vec<&str> = arns.iter().map(String::as_str).collect();
    tag(&router, &borrowed, &[("Env", "prod")]).await;

    // Rows: a page smaller than, equal to, and larger than the result set, plus
    // one that divides it evenly and one that does not.
    for per_page in [1_usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token = String::new();
        let mut pages = 0;
        loop {
            let mut request = serde_json::json!({ "ResourcesPerPage": per_page });
            if !token.is_empty() {
                request["PaginationToken"] = serde_json::json!(token);
            }
            let (status, body) = call(&router, "GetResources", request).await;
            assert_eq!(status, 200, "case ResourcesPerPage={per_page}: body {body}");
            let page = page_arns(&body);
            assert!(
                page.len() <= per_page,
                "case ResourcesPerPage={per_page}: a page of {} exceeds the cap",
                page.len()
            );
            seen.extend(page);
            token = body["PaginationToken"].as_str().unwrap_or_default().to_string();
            pages += 1;
            assert!(
                pages <= arns.len() + 1,
                "case ResourcesPerPage={per_page}: pagination did not terminate"
            );
            if token.is_empty() {
                break;
            }
        }

        let mut expected = arns.clone();
        expected.sort();
        assert_eq!(
            seen, expected,
            "case ResourcesPerPage={per_page}: expected every ARN once, got {seen:?}"
        );
        // A full page is never the last one: the walk ends on the empty page the
        // full page's token serves, so an evenly dividing size costs one extra
        // call and an under-full page ends the walk where it is.
        let expected_pages = arns.len() / per_page + 1;
        assert_eq!(
            pages, expected_pages,
            "case ResourcesPerPage={per_page}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn tags_per_page_caps_a_page_without_splitting_a_resource() {
    let router = emulator();
    // 11 resources with 10 tags each: 110 tag pairs against the smallest
    // documented TagsPerPage budget of 100.
    let arns: Vec<String> = (0..11)
        .map(|n| format!("arn:aws:lex:us-east-1:123456789012:bot:group-{n:02}"))
        .collect();
    let borrowed: Vec<&str> = arns.iter().map(String::as_str).collect();
    let keys: Vec<String> = (0..10).map(|n| format!("Key{n}")).collect();
    let tags: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v")).collect();
    tag(&router, &borrowed, &tags).await;

    let (status, first) = call(
        &router,
        "GetResources",
        serde_json::json!({ "TagsPerPage": 100 }),
    )
    .await;
    assert_eq!(status, 200, "case TagsPerPage=100: body {first}");
    let first_page = page_arns(&first);
    assert_eq!(
        first_page.len(),
        10,
        "case TagsPerPage=100: expected 10 resources (100 tag pairs), got {first_page:?}"
    );
    for arn in &first_page {
        assert_eq!(
            page_tags(&first, arn).len(),
            10,
            "case TagsPerPage=100: {arn} must carry all its tags, never a split set"
        );
    }

    let token = first["PaginationToken"].as_str().unwrap_or_default();
    assert!(
        !token.is_empty(),
        "case TagsPerPage=100: a truncated page must carry a PaginationToken, got {first}"
    );
    let (status, second) = call(
        &router,
        "GetResources",
        serde_json::json!({ "TagsPerPage": 100, "PaginationToken": token }),
    )
    .await;
    assert_eq!(status, 200, "case TagsPerPage=100 page 2: body {second}");
    let second_page = page_arns(&second);
    assert_eq!(
        second_page,
        vec![arns[10].clone()],
        "case TagsPerPage=100 page 2: expected the remainder with no repeats, got {second_page:?}"
    );
    assert_eq!(
        second["PaginationToken"],
        serde_json::json!(""),
        "case TagsPerPage=100 page 2: the last page must carry no token, got {second}"
    );
}

#[tokio::test]
async fn get_tag_keys_and_values_resume_from_their_own_token() {
    let router = emulator();
    // More distinct keys and values than the page size, so both reads truncate.
    // Spread across three resources, since one resource holds at most 50 tags.
    for chunk in 0..3 {
        let arn = format!("arn:aws:lex:us-east-1:123456789012:bot:group-{chunk}");
        let keys: Vec<String> = (0..50).map(|n| format!("Key{:03}", chunk * 50 + n)).collect();
        let tags: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "shared")).collect();
        tag(&router, &[arn.as_str()], &tags).await;
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token = String::new();
    loop {
        let mut request = serde_json::json!({});
        if !token.is_empty() {
            request["PaginationToken"] = serde_json::json!(token);
        }
        let (status, body) = call(&router, "GetTagKeys", request).await;
        assert_eq!(status, 200, "GetTagKeys page: body {body}");
        let page: Vec<String> = body["TagKeys"]
            .as_array()
            .unwrap_or_else(|| panic!("no TagKeys: {body}"))
            .iter()
            .map(|key| key.as_str().expect("a TagKeys entry").to_string())
            .collect();
        assert!(
            page.len() <= 100,
            "GetTagKeys served {} keys in one page", page.len()
        );
        seen.extend(page);
        token = body["PaginationToken"].as_str().unwrap_or_default().to_string();
        if token.is_empty() {
            break;
        }
    }
    let expected: Vec<String> = (0..150).map(|n| format!("Key{n:03}")).collect();
    assert_eq!(
        seen, expected,
        "GetTagKeys must page through every distinct key exactly once"
    );

    // One key, one value: GetTagValues answers it in a single untruncated page.
    let (status, body) = call(
        &router,
        "GetTagValues",
        serde_json::json!({ "Key": "Key000" }),
    )
    .await;
    assert_eq!(status, 200, "GetTagValues: body {body}");
    assert_eq!(body["TagValues"], serde_json::json!(["shared"]));
    assert_eq!(
        body["PaginationToken"],
        serde_json::json!(""),
        "an untruncated GetTagValues must carry no token, got {body}"
    );
}

#[tokio::test]
async fn a_token_this_service_never_minted_is_a_malformed_parameter() {
    let router = emulator();
    let arns: Vec<String> = (0..3)
        .map(|n| format!("arn:aws:lex:us-east-1:123456789012:bot:group-{n}"))
        .collect();
    let borrowed: Vec<&str> = arns.iter().map(String::as_str).collect();
    tag(&router, &borrowed, &[("Env", "prod")]).await;

    // A real token, so the cross-projection rows below are tokens that are
    // well-formed but were cut from a different list.
    let (_, first) = call(
        &router,
        "GetResources",
        serde_json::json!({ "ResourcesPerPage": 1 }),
    )
    .await;
    let valid = first["PaginationToken"]
        .as_str()
        .expect("a token on a truncated page")
        .to_string();
    assert!(!valid.is_empty(), "expected a truncated first page: {first}");
    let (_, keys_page) = call(&router, "GetTagKeys", serde_json::json!({})).await;
    let keys_token = keys_page["PaginationToken"].as_str().unwrap_or_default();
    assert!(
        keys_token.is_empty(),
        "three keys fit one page; got a token: {keys_page}"
    );

    // Rows: unparseable, empty cursor, a made-up digest, and a valid token
    // replayed against a different filter set.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("bogus", serde_json::json!({ "PaginationToken": "bogus" })),
        (
            "no cursor",
            serde_json::json!({ "PaginationToken": "0123456789abcdef:0:" }),
        ),
        (
            "made-up digest",
            serde_json::json!({
                "PaginationToken": "deadbeefdeadbeef:4000000000:arn:aws:lex:us-east-1:123456789012:bot:group-0"
            }),
        ),
        (
            "replayed against other filters",
            serde_json::json!({
                "PaginationToken": valid,
                "TagFilters": [{ "Key": "Env", "Values": ["prod"] }]
            }),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = call(&router, "GetResources", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case}: got {body}"
        );
    }

    // The same refusal on the other two paginated reads.
    for operation in ["GetTagKeys", "GetTagValues"] {
        let mut request = serde_json::json!({ "PaginationToken": "bogus" });
        if operation == "GetTagValues" {
            request["Key"] = serde_json::json!("Env");
        }
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {operation} with a bogus token: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {operation} with a bogus token: got {body}"
        );
    }
}

#[tokio::test]
async fn get_resources_refuses_the_documented_parameter_combinations() {
    let router = emulator();
    tag(
        &router,
        &["arn:aws:lex:us-east-1:123456789012:bot:group-a"],
        &[("Env", "prod")],
    )
    .await;

    // "You can't specify both ResourceARNList and ResourceTypeFilters /
    // TagFilters / ResourcesPerPage / TagsPerPage / PaginationToken."
    let exclusions: &[(&str, serde_json::Value)] = &[
        (
            "TagFilters",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:group-a"],
                "TagFilters": [{ "Key": "Env" }]
            }),
        ),
        (
            "ResourceTypeFilters",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:group-a"],
                "ResourceTypeFilters": ["s3"]
            }),
        ),
        (
            "ResourcesPerPage",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:group-a"],
                "ResourcesPerPage": 1
            }),
        ),
        (
            "TagsPerPage",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:group-a"],
                "TagsPerPage": 100
            }),
        ),
        (
            "PaginationToken",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:group-a"],
                "PaginationToken": "anything"
            }),
        ),
        (
            "ExcludeCompliantResources without IncludeComplianceDetails",
            serde_json::json!({ "ExcludeCompliantResources": true }),
        ),
        (
            "a TagFilters entry with no Key",
            serde_json::json!({ "TagFilters": [{ "Values": ["prod"] }] }),
        ),
    ];
    for (case, request) in exclusions {
        let (status, body) = call(&router, "GetResources", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case}: got {body}"
        );
    }

    // The documented ranges: each published bound is accepted, one past it is
    // refused. Sources: "You can specify a minimum of 1 and a maximum value of
    // 100" (ResourcesPerPage) and "a minimum of 100 items up to a maximum of 500
    // items" (TagsPerPage).
    let ranges: &[(&str, i64, bool)] = &[
        ("ResourcesPerPage", 1, true),
        ("ResourcesPerPage", 100, true),
        ("ResourcesPerPage", 0, false),
        ("ResourcesPerPage", 101, false),
        ("TagsPerPage", 100, true),
        ("TagsPerPage", 500, true),
        ("TagsPerPage", 99, false),
        ("TagsPerPage", 501, false),
    ];
    for (member, value, accepted) in ranges {
        let mut request = serde_json::json!({});
        request[*member] = serde_json::json!(value);
        let (status, body) = call(&router, "GetResources", request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {member}={value}: expected HTTP 200, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {member}={value}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterException",
                "case {member}={value}: got {body}"
            );
        }
    }
}

#[tokio::test]
async fn compliance_details_are_refused_rather_than_invented() {
    let router = emulator();
    let arn = "arn:aws:lex:us-east-1:123456789012:bot:group-a";
    tag(&router, &[arn], &[("Env", "prod")]).await;

    // A normal read carries no ComplianceDetails at all.
    let (status, body) = call(&router, "GetResources", serde_json::json!({})).await;
    assert_eq!(status, 200, "GetResources: body {body}");
    assert!(
        body["ResourceTagMappingList"][0]["ComplianceDetails"].is_null(),
        "ComplianceDetails must be absent, got {body}"
    );

    // Compliance is judged against an effective tag policy, and none is
    // modelled: both members say so instead of inventing a status or serving an
    // empty "nothing is noncompliant" list.
    let cases: &[(&str, serde_json::Value)] = &[
        (
            "IncludeComplianceDetails",
            serde_json::json!({ "IncludeComplianceDetails": true }),
        ),
        (
            "IncludeComplianceDetails + ExcludeCompliantResources",
            serde_json::json!({
                "IncludeComplianceDetails": true,
                "ExcludeCompliantResources": true
            }),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = call(&router, "GetResources", request.clone()).await;
        assert_eq!(
            status, 501,
            "case {case}: expected HTTP 501, got {status} with {body}"
        );
        assert_eq!(error_code(&body), "NotImplemented", "case {case}: got {body}");
    }

    // `IncludeComplianceDetails: false` is not a request for compliance, so it
    // reads normally.
    let (status, body) = call(
        &router,
        "GetResources",
        serde_json::json!({ "IncludeComplianceDetails": false }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case IncludeComplianceDetails=false: expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        page_arns(&body),
        vec![arn.to_string()],
        "case IncludeComplianceDetails=false: got {body}"
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_without_bound() {
    let router = emulator();

    // AWS limit: "Each resource can have up to 50 tags" (TagResources doc).
    // Rows: exactly at the limit is accepted, one past it is refused, and the
    // refusal leaves the stored tag map untouched.
    let arn = "arn:aws:lex:us-east-1:123456789012:bot:group-a";
    let keys: Vec<String> = (0..50).map(|n| format!("Key{n:02}")).collect();
    for chunk in keys.chunks(25) {
        let tags: Vec<(&str, &str)> = chunk.iter().map(|key| (key.as_str(), "v")).collect();
        tag(&router, &[arn], &tags).await;
    }
    let (_, read) = call(
        &router,
        "GetResources",
        serde_json::json!({ "ResourceARNList": [arn] }),
    )
    .await;
    assert_eq!(
        page_tags(&read, arn).len(),
        50,
        "case 50 tags: expected the limit to be reachable, got {read}"
    );

    let (status, body) = call(
        &router,
        "TagResources",
        serde_json::json!({ "ResourceARNList": [arn], "Tags": { "OneTooMany": "v" } }),
    )
    .await;
    assert_eq!(status, 200, "case 51st tag: expected HTTP 200, got {body}");
    assert_eq!(
        body["FailedResourcesMap"][arn]["ErrorCode"],
        serde_json::json!("InvalidParameterException"),
        "case 51st tag: expected a per-resource refusal, got {body}"
    );
    let (_, read) = call(
        &router,
        "GetResources",
        serde_json::json!({ "ResourceARNList": [arn] }),
    )
    .await;
    let stored = page_tags(&read, arn);
    assert_eq!(
        stored.len(),
        50,
        "case 51st tag: a refused write must not change the store, got {stored:?}"
    );
    assert!(
        !stored.contains_key("OneTooMany"),
        "case 51st tag: the refused key was stored anyway: {stored:?}"
    );

    // Emulator bound: the store holds at most MAX_TAGGED_RESOURCES resources, so
    // a client looping with fresh ARNs is refused rather than growing it for ever.
    // 1 resource is already stored, so 999 more fill it.
    let filler: Vec<String> = (0..999).map(|n| format!("arn:aws:lex:us-east-1:123456789012:bot:q{n:04}")).collect();
    for chunk in filler.chunks(20) {
        let arns: Vec<&str> = chunk.iter().map(String::as_str).collect();
        tag(&router, &arns, &[("Env", "prod")]).await;
    }

    // Re-tagging a resource already in the store still succeeds at capacity.
    tag(&router, &[filler[0].as_str()], &[("Env", "staging")]).await;

    // A new ARN does not.
    let (status, body) = call(
        &router,
        "TagResources",
        serde_json::json!({
            "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:one-too-many", filler[1].clone()],
            "Tags": { "Env": "prod" }
        }),
    )
    .await;
    assert_eq!(status, 200, "case store at capacity: expected HTTP 200, got {body}");
    assert_eq!(
        body["FailedResourcesMap"]["arn:aws:lex:us-east-1:123456789012:bot:one-too-many"]["ErrorCode"],
        serde_json::json!("InvalidParameterException"),
        "case store at capacity: the new ARN must be refused, got {body}"
    );
    assert!(
        body["FailedResourcesMap"][&filler[1]].is_null(),
        "case store at capacity: an ARN already in the store must still be tagged, got {body}"
    );

    // And the refused ARN really is absent.
    let (_, read) = call(
        &router,
        "GetResources",
        serde_json::json!({ "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:one-too-many"] }),
    )
    .await;
    assert!(
        page_arns(&read).is_empty(),
        "case store at capacity: the refused ARN was stored anyway, got {read}"
    );
}

#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_not_implemented() {
    let router = emulator();
    // Each is out of scope for this slice; a 501 says "not written yet", which
    // is what must NOT quietly become an empty 200.
    let stubs: &[(&str, serde_json::Value)] = &[
        ("GetComplianceSummary", serde_json::json!({})),
        ("StartReportCreation", serde_json::json!({ "S3Bucket": "a-bucket" })),
        ("DescribeReportCreation", serde_json::json!({})),
        ("ListRequiredTags", serde_json::json!({})),
    ];
    for (operation, request) in stubs {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, 501,
            "case {operation}: expected HTTP 501, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The same loop over the OTHER framing this endpoint serves.
//
// RULE: a request is read, dispatched and answered in the framing THE REQUEST
// names — the `awsJson1_1` framing the model names, which posts to `/` and names
// its operation in `X-Amz-Target`, or Smithy RPC v2 CBOR, which names its
// operation in the path `/service/{Service}/operation/{Operation}`, declares
// itself with `smithy-protocol: rpc-v2-cbor`, carries its body as CBOR and sends
// no `X-Amz-Target`. Both decode to the same modelled input and are served by the
// same operation over the same store, so the two answer the same payload for the
// same query, a write in either is visible through the other, and a read in
// either changes nothing a later caller sees.
//
// The rows below differ along what that rule generalises over: which framing
// carries the request, which operation it names, which members the input holds
// (a nested filter, a list, a number, none at all), how the CBOR container
// lengths are written, and which class of answer the call earns (a success, a
// modelled refusal, an operation this endpoint does not have).
// ---------------------------------------------------------------------------

/// The `{Service}` label of this endpoint's RPC v2 CBOR paths: the local name of
/// the model's service shape (`smithy_query(query="service_info")` -> service
/// `ResourceGroupsTaggingAPI_20170126`), which is also the `X-Amz-Target` prefix
/// of its awsJson framing.
const SERVICE_LABEL: &str = "ResourceGroupsTaggingAPI_20170126";

/// The header the RPC v2 CBOR specification requires on every request and
/// response of that framing, and the value it carries.
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// The media type that framing carries its bodies under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// The framing one request is sent in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// The framing the model names: `POST /` with `X-Amz-Target` and a JSON body.
    AwsJson,
    /// Smithy RPC v2 CBOR: the operation in the path, the body as CBOR.
    RpcV2Cbor,
}

/// How a CBOR request writes its container headers. Both forms are CBOR and both
/// are what some client sends, so each framing row is sent in one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lengths {
    /// Every map and array declares its entry count up front.
    Declared,
    /// Every map and array is opened and closed by a break, as the framework's
    /// own serializer writes them.
    Indefinite,
}

/// `value` as the CBOR item carrying the same members, with container lengths
/// written as `lengths` says.
///
/// Encoded with the CBOR implementation the framework's own codec is built on, so
/// the bytes a case sends are the bytes a client of that framing sends.
fn cbor_bytes(value: &serde_json::Value, lengths: Lengths) -> Vec<u8> {
    fn write(encoder: &mut Encoder, value: &serde_json::Value, lengths: Lengths) {
        match value {
            serde_json::Value::Null => {
                encoder.null();
            }
            serde_json::Value::Bool(flag) => {
                encoder.boolean(*flag);
            }
            serde_json::Value::Number(number) => match number.as_i64() {
                Some(whole) => {
                    encoder.long(whole);
                }
                None => {
                    encoder.double(number.as_f64().unwrap_or_default());
                }
            },
            serde_json::Value::String(text) => {
                encoder.str(text);
            }
            serde_json::Value::Array(elements) => {
                match lengths {
                    Lengths::Declared => encoder.array(elements.len()),
                    Lengths::Indefinite => encoder.begin_array(),
                };
                for element in elements {
                    write(encoder, element, lengths);
                }
                if lengths == Lengths::Indefinite {
                    encoder.end();
                }
            }
            serde_json::Value::Object(members) => {
                match lengths {
                    Lengths::Declared => encoder.map(members.len()),
                    Lengths::Indefinite => encoder.begin_map(),
                };
                for (name, held) in members {
                    encoder.str(name);
                    write(encoder, held, lengths);
                }
                if lengths == Lengths::Indefinite {
                    encoder.end();
                }
            }
        }
    }
    let mut encoder = Encoder::new(Vec::new());
    write(&mut encoder, value, lengths);
    encoder.into_writer()
}

/// One value a response body carries, whichever framing carries it: the two
/// framings decode to the same shape, so every assertion below is written once.
///
/// A map's entries are held in KEY order rather than in the order the body wrote
/// them, because the order two codecs write one structure's members in is each
/// codec's own and is not part of the payload.
#[derive(Debug, Clone, PartialEq)]
enum Decoded {
    Text(String),
    Number(f64),
    Flag(bool),
    Bytes(Vec<u8>),
    List(Vec<Decoded>),
    Map(BTreeMap<String, Decoded>),
    Null,
}

impl Decoded {
    /// The value under `key`, for a map.
    fn get(&self, key: &str) -> Option<&Decoded> {
        match self {
            Self::Map(entries) => entries.get(key),
            _ => None,
        }
    }

    /// This value's text, for a text member.
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The text under `key`, or the empty string when the body carries none
    /// there — so an assertion reports what the body held rather than panicking
    /// on its absence.
    fn text_at(&self, key: &str) -> &str {
        self.get(key).and_then(Decoded::text).unwrap_or_default()
    }

    /// The elements of the list this body carries at `member`, each as the one
    /// string that names it: the `ResourceARN` of a resource-tag mapping, or the
    /// element itself where the list is one of strings (`TagKeys`, `TagValues`).
    fn listed_at(&self, member: &str) -> Vec<String> {
        let Some(Decoded::List(elements)) = self.get(member) else {
            return Vec::new();
        };
        elements
            .iter()
            .map(|element| match element {
                Decoded::Text(text) => text.clone(),
                Decoded::Map(_) => element.text_at("ResourceARN").to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    /// This body with the member `key` removed, for comparing two answers that
    /// agree on everything but a per-call value.
    fn without(&self, key: &str) -> Decoded {
        match self {
            Self::Map(entries) => Self::Map(
                entries
                    .iter()
                    .filter(|(name, _)| name.as_str() != key)
                    .map(|(name, held)| (name.clone(), held.clone()))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// The error code a refusal names, as a client branches on it: the `__type`
    /// without its namespace.
    fn refusal_code(&self) -> String {
        let raw = self.text_at("__type");
        raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
    }
}

/// A JSON response body as the shape above.
fn decoded_json(body: &[u8]) -> Decoded {
    fn convert(value: serde_json::Value) -> Decoded {
        match value {
            serde_json::Value::Null => Decoded::Null,
            serde_json::Value::Bool(flag) => Decoded::Flag(flag),
            serde_json::Value::Number(number) => {
                Decoded::Number(number.as_f64().unwrap_or_default())
            }
            serde_json::Value::String(text) => Decoded::Text(text),
            serde_json::Value::Array(elements) => {
                Decoded::List(elements.into_iter().map(convert).collect())
            }
            serde_json::Value::Object(members) => Decoded::Map(
                members
                    .into_iter()
                    .map(|(key, held)| (key, convert(held)))
                    .collect(),
            ),
        }
    }
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) => convert(value),
        Err(_) => Decoded::Null,
    }
}

/// A CBOR response body as the shape above, decoded with the CBOR implementation
/// the framework's own codec is built on rather than scanned as text, so what is
/// asserted is the WHOLE decoded value a client of that framing reads.
fn decoded_cbor(body: &[u8]) -> Decoded {
    fn read(decoder: &mut Decoder<'_>) -> Decoded {
        let datatype = decoder
            .datatype()
            .unwrap_or_else(|error| panic!("the body ends where an item was expected: {error}"));
        match datatype {
            Type::String | Type::StringIndef => Decoded::Text(
                decoder
                    .string()
                    .unwrap_or_else(|error| panic!("a text item does not read: {error}")),
            ),
            Type::Bytes | Type::BytesIndef => Decoded::Bytes(
                decoder
                    .blob()
                    .unwrap_or_else(|error| panic!("a byte string does not read: {error}"))
                    .into_inner(),
            ),
            Type::Bool => Decoded::Flag(
                decoder
                    .boolean()
                    .unwrap_or_else(|error| panic!("a boolean does not read: {error}")),
            ),
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::I8 | Type::I16 | Type::I32
            | Type::I64 | Type::Int => Decoded::Number(
                decoder
                    .long()
                    .unwrap_or_else(|error| panic!("a whole number does not read: {error}"))
                    as f64,
            ),
            Type::F16 | Type::F32 | Type::F64 => Decoded::Number(
                decoder
                    .double()
                    .unwrap_or_else(|error| panic!("a number does not read: {error}")),
            ),
            Type::Null => {
                decoder.null().expect("a null reads");
                Decoded::Null
            }
            Type::Array | Type::ArrayIndef => {
                let declared = decoder
                    .list()
                    .unwrap_or_else(|error| panic!("an array header does not read: {error}"));
                let mut elements = Vec::new();
                while !at_end(decoder, declared, elements.len()) {
                    elements.push(read(decoder));
                }
                end_container(decoder, declared);
                Decoded::List(elements)
            }
            Type::Map | Type::MapIndef => {
                let declared = decoder
                    .map()
                    .unwrap_or_else(|error| panic!("a map header does not read: {error}"));
                let mut entries = BTreeMap::new();
                while !at_end(decoder, declared, entries.len()) {
                    let key = decoder
                        .string()
                        .unwrap_or_else(|error| panic!("a map key is not text: {error}"));
                    entries.insert(key, read(decoder));
                }
                end_container(decoder, declared);
                Decoded::Map(entries)
            }
            other => panic!("a response body carries no item of type {other:?}"),
        }
    }

    /// Whether a container that declared `declared` entries (or none, for an
    /// indefinite one) ends here, `read` entries in.
    fn at_end(decoder: &mut Decoder<'_>, declared: Option<u64>, read: usize) -> bool {
        match declared {
            Some(entries) => read as u64 == entries,
            None => matches!(
                decoder
                    .datatype()
                    .unwrap_or_else(|error| panic!("a container ends mid-entry: {error}")),
                Type::Break
            ),
        }
    }

    /// Consume the break that terminates an indefinite-length container.
    fn end_container(decoder: &mut Decoder<'_>, declared: Option<u64>) {
        if declared.is_none() {
            decoder
                .skip()
                .unwrap_or_else(|error| panic!("the break of a container does not read: {error}"));
        }
    }

    let mut decoder = Decoder::new(body);
    read(&mut decoder)
}

/// One answer a call earns: the status, the framing markers the response carries,
/// and the decoded body.
#[derive(Debug, Clone, PartialEq)]
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Decoded,
}

/// Call `operation` in `framing` with the input `body` names, and answer what the
/// endpoint replied.
///
/// The awsJson request is the one the rest of this file sends; the CBOR one is
/// the same input, path-routed, CBOR-encoded and carrying NO `X-Amz-Target` —
/// which is what that framing sends, and the whole point of the case.
async fn framed_call(
    router: &MultiServiceRouter,
    framing: Framing,
    lengths: Lengths,
    operation: &str,
    body: serde_json::Value,
) -> Answer {
    // The SigV4 credential scope is the routing signal that names the `tagging`
    // signing name this service claims, in either framing.
    let authorization = "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/us-east-1/tagging/aws4_request, \
         SignedHeaders=host, Signature=0";
    let request = match framing {
        Framing::AwsJson => http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{SERVICE_LABEL}.{operation}"))
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the awsJson request"),
        Framing::RpcV2Cbor => http::Request::builder()
            .method("POST")
            .uri(format!("/service/{SERVICE_LABEL}/operation/{operation}"))
            .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1)
            .header("content-type", CBOR_MEDIA_TYPE)
            .header("accept", CBOR_MEDIA_TYPE)
            .header("authorization", authorization)
            .body(bytes::Bytes::from(cbor_bytes(&body, lengths)))
            .expect("build the rpc-v2-cbor request"),
    };

    let response = router.dispatch(request).await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let smithy_protocol = header(SMITHY_PROTOCOL.0);
    let body = match framing {
        Framing::AwsJson => decoded_json(response.body()),
        Framing::RpcV2Cbor if response.body().is_empty() => Decoded::Null,
        Framing::RpcV2Cbor => decoded_cbor(response.body()),
    };
    Answer {
        status: response.status().as_u16(),
        content_type,
        smithy_protocol,
        body,
    }
}

/// One query of the read loop, asked in both framings: the operation, the input,
/// the output member whose elements the answer is checked against, those
/// elements, whether the answer is one page of more than one, and how the CBOR
/// request writes its container lengths.
struct FramingCase {
    case: &'static str,
    operation: &'static str,
    input: serde_json::Value,
    member: &'static str,
    expected: Vec<&'static str>,
    truncated: bool,
    lengths: Lengths,
}

#[tokio::test]
async fn both_framings_of_this_endpoint_answer_one_query_the_same_way() {
    let router = emulator();
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    let beta = "arn:aws:codecommit:us-east-1:123456789012:beta";
    tag(&router, &[alpha], &[("Env", "prod"), ("Team", "core")]).await;
    tag(&router, &[beta], &[("Env", "dev")]).await;

    let cases = vec![
        // The query the recorded CBOR call asks: a nested filter structure.
        FramingCase {
            case: "GetResources by tag filter",
            operation: "GetResources",
            input: serde_json::json!({ "TagFilters": [{ "Key": "Env", "Values": ["prod"] }] }),
            member: "ResourceTagMappingList",
            expected: vec!["arn:aws:lex:us-east-1:123456789012:bot:alpha"],
            truncated: false,
            lengths: Lengths::Declared,
        },
        // A list of strings, and the same query written with indefinite-length
        // CBOR containers.
        FramingCase {
            case: "GetResources by ARN list, indefinite lengths",
            operation: "GetResources",
            input: serde_json::json!({ "ResourceARNList": [beta] }),
            member: "ResourceTagMappingList",
            expected: vec!["arn:aws:codecommit:us-east-1:123456789012:beta"],
            truncated: false,
            lengths: Lengths::Indefinite,
        },
        // A number member, which the two framings carry in different wire forms:
        // a page size of one, so the answer is the first page of two and carries
        // a token to ask for the next with.
        FramingCase {
            case: "GetResources with a page size",
            operation: "GetResources",
            input: serde_json::json!({ "ResourcesPerPage": 1 }),
            member: "ResourceTagMappingList",
            expected: vec!["arn:aws:codecommit:us-east-1:123456789012:beta"],
            truncated: true,
            lengths: Lengths::Declared,
        },
        // No member at all: an empty body, which each framing spells its own way.
        FramingCase {
            case: "GetResources with no member set",
            operation: "GetResources",
            input: serde_json::json!({}),
            member: "ResourceTagMappingList",
            expected: vec![
                "arn:aws:codecommit:us-east-1:123456789012:beta",
                "arn:aws:lex:us-east-1:123456789012:bot:alpha",
            ],
            truncated: false,
            lengths: Lengths::Indefinite,
        },
        FramingCase {
            case: "GetTagKeys",
            operation: "GetTagKeys",
            input: serde_json::json!({}),
            member: "TagKeys",
            expected: vec!["Env", "Team"],
            truncated: false,
            lengths: Lengths::Declared,
        },
        FramingCase {
            case: "GetTagValues of one key",
            operation: "GetTagValues",
            input: serde_json::json!({ "Key": "Env" }),
            member: "TagValues",
            expected: vec!["dev", "prod"],
            truncated: false,
            lengths: Lengths::Declared,
        },
    ];

    for FramingCase {
        case,
        operation,
        input,
        member,
        expected,
        truncated,
        lengths,
    } in cases
    {
        let as_json = framed_call(
            &router,
            Framing::AwsJson,
            lengths,
            operation,
            input.clone(),
        )
        .await;
        let as_cbor = framed_call(&router, Framing::RpcV2Cbor, lengths, operation, input).await;

        assert_eq!(
            as_cbor.status, 200,
            "case {case:?}: the CBOR framing must be served; got {} with {:?}",
            as_cbor.status, as_cbor.body
        );
        assert_eq!(
            as_cbor.smithy_protocol, SMITHY_PROTOCOL.1,
            "case {case:?}: expected the response to name the framing {:?}, got {:?}",
            SMITHY_PROTOCOL.1, as_cbor.smithy_protocol
        );
        assert_eq!(
            as_cbor.content_type, CBOR_MEDIA_TYPE,
            "case {case:?}: expected a {CBOR_MEDIA_TYPE} body, got {:?}",
            as_cbor.content_type
        );
        assert_eq!(
            as_json.status, 200,
            "case {case:?}: the awsJson framing must be served; got {} with {:?}",
            as_json.status, as_json.body
        );

        // The payload itself: each framing carries the resources the query
        // selects, and the two carry the SAME answer.
        for (framing, answer) in [("awsJson", &as_json), ("rpc-v2-cbor", &as_cbor)] {
            let listed = answer.body.listed_at(member);
            assert_eq!(
                listed, expected,
                "case {case:?} over {framing}: expected {member} {expected:?}, got {listed:?} \
                 (whole body {:?})",
                answer.body
            );
            let token = answer.body.text_at("PaginationToken");
            assert_eq!(
                !token.is_empty(),
                truncated,
                "case {case:?} over {framing}: expected a token to carry on from: {truncated}, \
                 got {token:?}"
            );
        }
        // A page's token is cut per call and carries the instant it was cut, so
        // it is the one member of an answer the two framings do not share.
        let (cbor_payload, json_payload) = (
            as_cbor.body.without("PaginationToken"),
            as_json.body.without("PaginationToken"),
        );
        assert_eq!(
            cbor_payload, json_payload,
            "case {case:?}: the two framings must decode to one payload; CBOR {cbor_payload:?} \
             vs JSON {json_payload:?}"
        );
    }
}

#[tokio::test]
async fn one_store_is_read_and_written_through_either_framing() {
    let router = emulator();
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    let gamma = "arn:aws:ec2:us-east-1:123456789012:instance/gamma";
    tag(&router, &[alpha], &[("Env", "prod")]).await;

    let query = serde_json::json!({ "TagFilters": [{ "Key": "Env", "Values": ["prod"] }] });
    let read = |framing| {
        framed_call(
            &router,
            framing,
            Lengths::Declared,
            "GetResources",
            query.clone(),
        )
    };

    // The read the store answers before any CBOR call reaches it.
    let before = read(Framing::AwsJson).await;
    assert_eq!(
        before.body.listed_at("ResourceTagMappingList"),
        vec![alpha.to_string()],
        "case the first read: expected the tagged resource, got {:?}",
        before.body
    );

    // Reading twice over CBOR answers the same thing, and the awsJson read is
    // unchanged afterwards: a read in either framing perturbs nothing.
    for look in 1..=2 {
        let as_cbor = read(Framing::RpcV2Cbor).await;
        assert_eq!(
            as_cbor.status, 200,
            "case CBOR read {look}: expected HTTP 200, got {} with {:?}",
            as_cbor.status, as_cbor.body
        );
        assert_eq!(
            as_cbor.body, before.body,
            "case CBOR read {look}: expected the same payload as the awsJson read, got {:?}",
            as_cbor.body
        );
    }
    let after = read(Framing::AwsJson).await;
    assert_eq!(
        after, before,
        "case the read after the CBOR reads: the CBOR reads must not have changed the store; \
         got {after:?}"
    );

    // A WRITE in the CBOR framing lands in that same store: the awsJson read
    // finds it, and so does the CBOR one.
    let written = framed_call(
        &router,
        Framing::RpcV2Cbor,
        Lengths::Declared,
        "TagResources",
        serde_json::json!({ "ResourceARNList": [gamma], "Tags": { "Env": "prod" } }),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case the CBOR write: expected HTTP 200, got {} with {:?}",
        written.status, written.body
    );
    assert_eq!(
        written.body.get("FailedResourcesMap"),
        Some(&Decoded::Map(BTreeMap::new())),
        "case the CBOR write: expected no failed resource, got {:?}",
        written.body
    );

    let expected = vec![gamma.to_string(), alpha.to_string()];
    for framing in [Framing::AwsJson, Framing::RpcV2Cbor] {
        let reread = read(framing).await;
        let listed = reread.body.listed_at("ResourceTagMappingList");
        assert_eq!(
            listed, expected,
            "case the read after the CBOR write over {framing:?}: expected {expected:?}, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_failure_of_a_cbor_framed_request_is_answered_in_that_framing() {
    let router = emulator();
    tag(&router, &["arn:aws:lex:us-east-1:123456789012:bot:alpha"], &[("Env", "prod")]).await;

    // A modelled refusal of the operation ("You can't specify both
    // ResourceARNList and ResourceTypeFilters / TagFilters / ResourcesPerPage /
    // TagsPerPage / PaginationToken"): InvalidParameterException is one of the
    // errors GetResources declares (`smithy_query(query="operation",
    // operation="GetResources")` -> errors InvalidParameterException 400).
    let refused = serde_json::json!({
        "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:alpha"],
        "TagFilters": [{ "Key": "Env" }]
    });
    let as_json = framed_call(
        &router,
        Framing::AwsJson,
        Lengths::Declared,
        "GetResources",
        refused.clone(),
    )
    .await;
    let as_cbor = framed_call(
        &router,
        Framing::RpcV2Cbor,
        Lengths::Declared,
        "GetResources",
        refused,
    )
    .await;
    assert_eq!(
        as_cbor.status, as_json.status,
        "case the refused combination: expected the status of the awsJson answer ({}), got {} \
         with {:?}",
        as_json.status, as_cbor.status, as_cbor.body
    );
    assert_eq!(
        as_cbor.status, 400,
        "case the refused combination: expected HTTP 400, got {} with {:?}",
        as_cbor.status, as_cbor.body
    );
    assert_eq!(
        as_cbor.body.refusal_code(),
        "InvalidParameterException",
        "case the refused combination: expected the modelled error code, got {:?}",
        as_cbor.body
    );
    assert_eq!(
        as_cbor.body, as_json.body,
        "case the refused combination: the two framings must state one refusal; CBOR {:?} vs \
         JSON {:?}",
        as_cbor.body, as_json.body
    );
    assert_eq!(
        (
            as_cbor.content_type.as_str(),
            as_cbor.smithy_protocol.as_str()
        ),
        (CBOR_MEDIA_TYPE, SMITHY_PROTOCOL.1),
        "case the refused combination: a refusal of a CBOR call is labelled as that framing, \
         got {as_cbor:?}"
    );

    // An operation this endpoint does not have, named in that framing's path, is
    // the framing's own not-found rather than a complaint about a header it does
    // not use.
    let unknown = framed_call(
        &router,
        Framing::RpcV2Cbor,
        Lengths::Declared,
        "GetNoSuchThing",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        unknown.status, 404,
        "case an operation the endpoint does not have: expected HTTP 404, got {} with {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        (
            unknown.content_type.as_str(),
            unknown.smithy_protocol.as_str()
        ),
        (CBOR_MEDIA_TYPE, SMITHY_PROTOCOL.1),
        "case an operation the endpoint does not have: the answer is labelled as that framing, \
         got {unknown:?}"
    );
    assert!(
        !unknown.body.refusal_code().is_empty(),
        "case an operation the endpoint does not have: the answer names no error, got {:?}",
        unknown.body
    );
}

// ---------------------------------------------------------------------------
// A request whose FRAMING CONTRADICTS ITSELF.
//
// RULE: the RPC v2 CBOR framing names its operation in the request path and its
// specification PROHIBITS the `awsJson` framing's dispatch headers
// (`X-Amz-Target`, `X-Amzn-Target`), so a request that names that framing and
// carries one of those headers anyway names its operation twice, in two
// framings, and is a request in neither: the front door refuses it with HTTP 400
// whose body names the class `MalformedHttpRequestException` and carries nothing
// else. The refusal is decided from the request's headers and path alone, so it
// is settled before any byte of the body is read, before any operation is named,
// and no operation of this endpoint runs for such a request.
//
// The rows below differ along what that rule generalises over: which spelling and
// casing of the prohibited header the request carries, how it names the CBOR
// framing (that framing's header, its path, or both), which operation the path
// names (a read, a write, one this endpoint does not have), whether the header
// agrees with the path, and what the body holds (a filter, no member, bytes that
// are not CBOR at all, nothing).
// ---------------------------------------------------------------------------

/// The class the front door reports a malformed HTTP request as. The whole body
/// of that refusal is this one member, with no content under it: the front door
/// read no operation and no member, so it words nothing.
const MALFORMED_HTTP_REQUEST: &str = "MalformedHttpRequestException";

/// One answer the front door gave: the status, the media type it labelled the
/// body with, the body's bytes as text (for an assertion to report), and that
/// body parsed as JSON (`Null` when it is empty or is not JSON).
#[derive(Debug, Clone, PartialEq)]
struct FrontDoorAnswer {
    status: u16,
    content_type: String,
    text: String,
    body: serde_json::Value,
}

/// `POST uri` carrying `headers` and `body`, and what the endpoint answered.
///
/// The headers are given per case rather than per framing, because the whole
/// point of a case is the combination of markers a request carries.
async fn asked_with(
    router: &MultiServiceRouter,
    uri: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> FrontDoorAnswer {
    // The SigV4 credential scope is the routing signal that names the `tagging`
    // signing name this service claims, whatever framing the request is in.
    let mut request = http::Request::builder().method("POST").uri(uri).header(
        "authorization",
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/tagging/aws4_request, \
         SignedHeaders=host, Signature=0",
    );
    for (name, value) in headers {
        request = request.header(*name, value.as_str());
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
    FrontDoorAnswer {
        status,
        content_type,
        text,
        body,
    }
}

/// The headers the CBOR framing itself carries: the protocol marker and the media
/// types of that framing's bodies.
fn cbor_framing_headers() -> Vec<(&'static str, String)> {
    vec![
        (SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1.to_string()),
        ("content-type", CBOR_MEDIA_TYPE.to_string()),
        ("accept", CBOR_MEDIA_TYPE.to_string()),
    ]
}

/// The path that framing routes `operation` on.
fn cbor_path(operation: &str) -> String {
    format!("/service/{SERVICE_LABEL}/operation/{operation}")
}

/// One row of the table below: the case, the uri asked for, the headers the
/// request carries beyond the credential scope, and its body bytes.
type ContradictoryCase = (
    &'static str,
    String,
    Vec<(&'static str, String)>,
    Vec<u8>,
);

#[tokio::test]
async fn a_cbor_framed_request_carrying_a_target_header_is_a_malformed_http_request() {
    let router = emulator();
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    tag(&router, &[alpha], &[("Env", "prod")]).await;

    // The ARN the contradictory WRITE row asks to tag. It must still be untagged
    // afterwards: the refusal runs no operation.
    let never_written = "arn:aws:lex:us-east-1:123456789012:bot:never-written";
    let filter = serde_json::json!({ "TagFilters": [{ "Key": "Env", "Values": ["prod"] }] });
    let target = |operation: &str| format!("{SERVICE_LABEL}.{operation}");
    let with = |extra: (&'static str, String)| {
        let mut headers = cbor_framing_headers();
        headers.push(extra);
        headers
    };

    let rows: Vec<ContradictoryCase> = vec![
        // The framing's own header and path, plus the header it prohibits.
        (
            "the framing's header and path, plus x-amz-target",
            cbor_path("GetResources"),
            with(("x-amz-target", target("GetResources"))),
            cbor_bytes(&filter, Lengths::Declared),
        ),
        // The other spelling of that header, on another operation, with the
        // request's containers written the other way.
        (
            "x-amzn-target, the other spelling of that header",
            cbor_path("GetTagKeys"),
            with(("x-amzn-target", target("GetTagKeys"))),
            cbor_bytes(&serde_json::json!({}), Lengths::Indefinite),
        ),
        // Header names are case-insensitive on the wire, so the upper-case
        // spelling an SDK writes is the same header.
        (
            "X-Amz-Target spelled in upper case",
            cbor_path("GetTagValues"),
            with(("X-Amz-Target", target("GetTagValues"))),
            cbor_bytes(&serde_json::json!({ "Key": "Env" }), Lengths::Declared),
        ),
        // The framing named by its PATH alone, with no protocol header.
        (
            "the framing's path alone, plus that header",
            cbor_path("GetResources"),
            vec![
                ("content-type", CBOR_MEDIA_TYPE.to_string()),
                ("x-amz-target", target("GetResources")),
            ],
            cbor_bytes(&filter, Lengths::Declared),
        ),
        // The framing named by its HEADER alone, posted to `/`.
        (
            "the framing's header alone, posted to /, plus that header",
            "/".to_string(),
            with(("x-amz-target", target("GetResources"))),
            cbor_bytes(&filter, Lengths::Indefinite),
        ),
        // A WRITE named in both framings at once.
        (
            "a write operation named in both framings",
            cbor_path("TagResources"),
            with(("x-amz-target", target("TagResources"))),
            cbor_bytes(
                &serde_json::json!({
                    "ResourceARNList": [never_written],
                    "Tags": { "Env": "prod" }
                }),
                Lengths::Declared,
            ),
        ),
        // The two namings disagreeing about which operation is called, which is
        // why the framing prohibits the header in the first place.
        (
            "a target header naming another operation than the path",
            cbor_path("GetResources"),
            with(("x-amz-target", target("GetTagKeys"))),
            cbor_bytes(&filter, Lengths::Declared),
        ),
        // An empty value still names the other framing's dispatch header.
        (
            "an empty target header value",
            cbor_path("GetResources"),
            with(("x-amz-target", String::new())),
            cbor_bytes(&filter, Lengths::Declared),
        ),
        // An operation this endpoint does not have: the framing is already
        // contradictory, so the request is refused before any operation is named.
        (
            "an operation this endpoint does not have, plus that header",
            cbor_path("GetNoSuchThing"),
            with(("x-amz-target", target("GetResources"))),
            cbor_bytes(&serde_json::json!({}), Lengths::Declared),
        ),
        // Bodies the framing could not read: the refusal is settled by the
        // headers and the path, before a byte of the body is read.
        (
            "a body that is not CBOR at all",
            cbor_path("GetResources"),
            with(("x-amz-target", target("GetResources"))),
            b"not cbor at all".to_vec(),
        ),
        (
            "no body at all",
            cbor_path("GetResources"),
            with(("x-amz-target", target("GetResources"))),
            Vec::new(),
        ),
    ];

    for (case, uri, headers, body) in rows {
        let answer = asked_with(&router, &uri, &headers, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for a self-contradictory framing, got {} with \
             {:?}",
            answer.status, answer.text
        );
        assert_eq!(
            answer.body,
            serde_json::json!({ MALFORMED_HTTP_REQUEST: "" }),
            "case {case:?}: expected the body to name {MALFORMED_HTTP_REQUEST} and nothing \
             else, got {:?}",
            answer.text
        );
    }

    // No operation ran for any of those requests: the write row tagged nothing,
    // and the store still answers exactly what was tagged before them.
    let (status, read) = call(
        &router,
        "GetResources",
        serde_json::json!({ "TagFilters": [{ "Key": "Env", "Values": ["prod"] }] }),
    )
    .await;
    assert_eq!(status, 200, "case the read afterwards: body {read}");
    assert_eq!(
        page_arns(&read),
        vec![alpha.to_string()],
        "case the read afterwards: the refused requests must have changed nothing, got {read}"
    );
}

#[tokio::test]
async fn each_framing_on_its_own_is_still_served() {
    let router = emulator();
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    tag(&router, &[alpha], &[("Env", "prod")]).await;

    let filter = serde_json::json!({ "TagFilters": [{ "Key": "Env", "Values": ["prod"] }] });
    // (case, uri, headers, body) — the same calls as the table above with the one
    // marker that makes them contradictory removed, plus the `awsJson` framing
    // whose dispatch header `X-Amz-Target` is.
    let rows: Vec<ContradictoryCase> = vec![
        (
            "the CBOR framing with no target header",
            cbor_path("GetResources"),
            cbor_framing_headers(),
            cbor_bytes(&filter, Lengths::Declared),
        ),
        (
            "the CBOR framing's path alone, no target header",
            cbor_path("GetResources"),
            vec![("content-type", CBOR_MEDIA_TYPE.to_string())],
            cbor_bytes(&filter, Lengths::Indefinite),
        ),
        (
            "the awsJson framing, whose dispatch header that is",
            "/".to_string(),
            vec![
                ("content-type", "application/x-amz-json-1.1".to_string()),
                (
                    "x-amz-target",
                    format!("{SERVICE_LABEL}.GetResources"),
                ),
            ],
            filter.to_string().into_bytes(),
        ),
    ];

    for (case, uri, headers, body) in rows {
        let answer = asked_with(&router, &uri, &headers, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a framing that names itself once must be served, got {} with {:?}",
            answer.status, answer.text
        );
        assert!(
            !answer.text.contains(MALFORMED_HTTP_REQUEST),
            "case {case:?}: expected no malformed-request refusal, got {:?} ({})",
            answer.text,
            answer.content_type
        );
    }
}

/// One row of the table below: the case, the operation the CBOR path names, the
/// input the request carries, and the status that operation answers for that
/// input when the request names ONE framing.
///
/// That last field is the answer the framing refusal takes precedence over: the
/// rows differ along WHICH answer that is — a served read, the modelled refusal
/// of an input the operation itself rejects, and the scaffold signal of an
/// operation this slice has not implemented.
type PrecedenceCase = (&'static str, &'static str, serde_json::Value, u16);

/// RULE (the half the table above does not reach): the refusal is settled by the
/// request's framing markers alone, so it precedes NAMING the operation — the
/// answer the named operation would itself have given, whatever class it is, is
/// never reached, and nothing of this service's state is read or written for such
/// a request.
#[tokio::test]
async fn the_framing_refusal_precedes_the_answer_the_named_operation_would_give() {
    let router = emulator();
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    tag(&router, &[alpha], &[("Env", "prod")]).await;

    let rows: Vec<PrecedenceCase> = vec![
        // A read this endpoint serves.
        (
            "an operation that answers the query",
            "GetResources",
            serde_json::json!({ "TagFilters": [{ "Key": "Env", "Values": ["prod"] }] }),
            200,
        ),
        // An input GetResources itself refuses: the documented combination of
        // `ResourceARNList` with `TagFilters`, which is `InvalidParameterException`
        // (`smithy_query(query="operation", operation="GetResources")` -> errors
        // InvalidParameterException 400).
        (
            "an input the operation itself refuses",
            "GetResources",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:lex:us-east-1:123456789012:bot:alpha"],
                "TagFilters": [{ "Key": "Env" }]
            }),
            400,
        ),
        // An operation this slice has not implemented, which answers the scaffold
        // signal on its own.
        (
            "an operation that answers the scaffold signal",
            "GetComplianceSummary",
            serde_json::json!({}),
            501,
        ),
    ];

    for (case, operation, input, own_status) in rows {
        let body = || cbor_bytes(&input, Lengths::Declared);

        // Naming ONE framing, the request reaches the operation and earns that
        // operation's own answer.
        let served = asked_with(
            &router,
            &cbor_path(operation),
            &cbor_framing_headers(),
            body(),
        )
        .await;
        assert_eq!(
            served.status, own_status,
            "case {case:?}: expected the operation's own answer ({own_status}), got {} with {:?}",
            served.status, served.text
        );
        assert!(
            !served.text.contains(MALFORMED_HTTP_REQUEST),
            "case {case:?}: a request naming one framing is not a malformed request, got {:?}",
            served.text
        );

        // The same call carrying the header that framing prohibits never reaches
        // it: the answer is the front door's, not the operation's.
        let mut headers = cbor_framing_headers();
        headers.push(("x-amz-target", format!("{SERVICE_LABEL}.{operation}")));
        let refused = asked_with(&router, &cbor_path(operation), &headers, body()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a self-contradictory framing, got {} with \
             {:?}",
            refused.status, refused.text
        );
        assert_eq!(
            refused.body,
            serde_json::json!({ MALFORMED_HTTP_REQUEST: "" }),
            "case {case:?}: expected the body to name {MALFORMED_HTTP_REQUEST} and nothing else, \
             got {:?}",
            refused.text
        );
    }

    // None of those refusals read or wrote anything: the store still answers
    // exactly what was tagged before them.
    let (status, read) = call(&router, "GetResources", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "case the read afterwards: expected HTTP 200, got {status} with {read}"
    );
    assert_eq!(
        page_arns(&read),
        vec![alpha.to_string()],
        "case the read afterwards: the refused requests must have changed nothing, got {read}"
    );
    assert_eq!(
        page_tags(&read, alpha),
        [("Env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case the read afterwards: the tags of the one stored resource changed, got {read}"
    );
}

// ---------------------------------------------------------------------------
// WHICH framings this endpoint serves at all.
//
// RULE: an endpoint serves a request in the framing the request names among the
// wire protocols its MODEL declares, and in no other
// (`smithy_query(query="service_info")` -> protocols `["awsJson1_1",
// "rpcv2Cbor"]`): a request naming one of them is read, dispatched and answered
// in that one, and a request naming a protocol the model does not declare — or no
// protocol at all — names no framing it could be read in, so the front door
// refuses it with the protocol-neutral `404` that carries no `content-type`,
// ahead of naming an operation, and no operation of this endpoint runs for it.
//
// The rows below differ along what that rule generalises over: which marker the
// request carries (that framing's dispatch header, its media type, its path, a
// marker of another protocol family, a protocol header naming a framing this
// endpoint does not serve, no marker at all), which operation it names, and
// whether that operation would READ or WRITE the store.
// ---------------------------------------------------------------------------

/// One row of the table below: the case, the uri asked for, the headers beyond
/// the credential scope, the body bytes, and whether the framing the request
/// names is one this endpoint serves.
type DeclaredFramingCase = (
    &'static str,
    String,
    Vec<(&'static str, String)>,
    Vec<u8>,
    bool,
);

#[tokio::test]
async fn only_the_framings_this_endpoints_model_declares_are_served() {
    let router = emulator();
    let alpha = "arn:aws:lex:us-east-1:123456789012:bot:alpha";
    tag(&router, &[alpha], &[("Env", "prod")]).await;

    // The ARN the refused WRITE rows ask to tag. It must still be untagged
    // afterwards: a request naming no framing this endpoint serves runs no
    // operation.
    let never_written = "arn:aws:lex:us-east-1:123456789012:bot:never-written";
    let filter = serde_json::json!({ "TagFilters": [{ "Key": "Env", "Values": ["prod"] }] });
    let write = serde_json::json!({
        "ResourceARNList": [never_written],
        "Tags": { "Env": "prod" }
    });
    let json_media_type = "application/x-amz-json-1.1";

    let rows: Vec<DeclaredFramingCase> = vec![
        // The `awsJson1_1` framing: `POST /`, the operation in `X-Amz-Target`,
        // a JSON body.
        (
            "the awsJson framing's dispatch header and media type",
            "/".to_string(),
            vec![
                ("content-type", json_media_type.to_string()),
                ("x-amz-target", format!("{SERVICE_LABEL}.GetResources")),
            ],
            filter.to_string().into_bytes(),
            true,
        ),
        // The same framing naming a WRITE, so a served row covers the mutating
        // half of the loop too. The write is the tag the store already holds, so
        // what it leaves behind is what the read at the end asserts.
        (
            "the awsJson framing naming a write",
            "/".to_string(),
            vec![
                ("content-type", json_media_type.to_string()),
                ("x-amz-target", format!("{SERVICE_LABEL}.TagResources")),
            ],
            serde_json::json!({ "ResourceARNList": [alpha], "Tags": { "Env": "prod" } })
                .to_string()
                .into_bytes(),
            true,
        ),
        // The RPC v2 CBOR framing, named by its own header and its path.
        (
            "the CBOR framing's header, media type and path",
            cbor_path("GetResources"),
            cbor_framing_headers(),
            cbor_bytes(&filter, Lengths::Declared),
            true,
        ),
        // The same framing named by its PATH alone, which is what routes it.
        (
            "the CBOR framing's path alone",
            cbor_path("GetTagKeys"),
            vec![("content-type", CBOR_MEDIA_TYPE.to_string())],
            cbor_bytes(&serde_json::json!({}), Lengths::Indefinite),
            true,
        ),
        // A request carrying NO protocol marker: `POST /` with a plain JSON media
        // type and no dispatch header names neither framing.
        (
            "no marker at all, with a plain JSON body",
            "/".to_string(),
            vec![("content-type", "application/json".to_string())],
            filter.to_string().into_bytes(),
            false,
        ),
        // The marker of another protocol FAMILY — the form media type the query
        // protocols carry — which this model does not declare.
        (
            "the form media type of the query protocols",
            "/".to_string(),
            vec![(
                "content-type",
                "application/x-www-form-urlencoded; charset=utf-8".to_string(),
            )],
            b"Action=GetResources&Version=2017-01-26".to_vec(),
            false,
        ),
        // A `smithy-protocol` header naming an RPC framing this endpoint does not
        // serve: the header is that family's marker, and this value is not the
        // CBOR one.
        (
            "a protocol header naming a framing this endpoint does not serve",
            "/".to_string(),
            vec![
                (SMITHY_PROTOCOL.0, "rpc-v2-json".to_string()),
                ("content-type", "application/json".to_string()),
            ],
            filter.to_string().into_bytes(),
            false,
        ),
        // The same, on a request that would WRITE: the refusal precedes the
        // operation, so nothing is tagged.
        (
            "no marker at all, on a request that would write",
            "/".to_string(),
            vec![("content-type", "application/json".to_string())],
            write.to_string().into_bytes(),
            false,
        ),
    ];

    for (case, uri, headers, body, served) in rows {
        let answer = asked_with(&router, &uri, &headers, body).await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a framing this model declares must be served, got {} with {:?}",
                answer.status, answer.text
            );
            assert!(
                !answer.content_type.is_empty(),
                "case {case:?}: a served answer carries the framing's media type, got {:?}",
                answer.text
            );
        } else {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: a framing this model does not declare must not be served, got \
                 {} with {:?}",
                answer.status, answer.text
            );
            assert_eq!(
                answer.content_type, "",
                "case {case:?}: the refusal of a request naming no declared framing is \
                 protocol-neutral, so it carries no content-type, got {:?}",
                answer.content_type
            );
            assert_eq!(
                answer.text,
                smithy_server_runtime::UNRECOGNISED_PROTOCOL_BODY,
                "case {case:?}: expected the front door's protocol-neutral refusal body, got \
                 {:?}",
                answer.text
            );
        }
    }

    // Nothing those refusals named ran: the store still holds exactly what the
    // served calls left in it — the one resource tagged above, and no trace of
    // the ARN the refused write asked for.
    let (status, read) = call(&router, "GetResources", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "case the read afterwards: expected HTTP 200, got {status} with {read}"
    );
    assert_eq!(
        page_arns(&read),
        vec![alpha.to_string()],
        "case the read afterwards: the refused requests must have written nothing, got {read}"
    );
    assert_eq!(
        page_tags(&read, alpha),
        [("Env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case the read afterwards: the tags of the one stored resource changed, got {read}"
    );
}

