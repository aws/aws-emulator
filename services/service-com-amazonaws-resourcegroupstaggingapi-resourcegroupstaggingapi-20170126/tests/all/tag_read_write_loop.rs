// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tag read/write loop: `TagResources` and
//! `UntagResources` write the ARN -> tags store, and `GetResources`,
//! `GetTagKeys` and `GetTagValues` are pure projections of it.
//!
//! Every test drives the service over the real wire path — the multi-service
//! router, `POST /` with `X-Amz-Target: ResourceGroupsTaggingAPI_20170126.<Op>`
//! and an awsJson1_1 body — so what is asserted is what an SDK client sees.
//!
//! The tables below span the class each rule generalises over (ARN form, filter
//! shape, page size, token provenance); the ARNs the issue cited are one row.

use std::collections::BTreeMap;
use std::sync::Arc;

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
/// member="ResourceTypeFilters")`). The forms are the three AWS ARN resource
/// layouts: bare `resource-id`, `resource-type/resource-id` and
/// `resource-type:resource-id`; the bare ones take their type from
/// `crate::state::implicit_resource_type`.
///
/// Every ARN is in the partition and Region this endpoint serves — the test
/// signs for `us-east-1` — because an ARN outside either is refused; that rule
/// has its own rows in `tagging_api_fidelity.rs`. The services named here are
/// the ones `state::owners` does NOT resolve against, so the ARNs need no live
/// resource behind them.
///
/// Columns: the ARN, the filter that selects it by service, and the filter that
/// selects it by `service:resourceType`.
const ARN_FORMS: &[(&str, &str, Option<&str>)] = &[
    // Bare resource id, no region, no account (the issue's first row).
    ("arn:aws:s3:::bucket-a", "s3", Some("s3:bucket")),
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
        "arn:aws:dynamodb:us-east-1:123456789012:table/orders/stream/2026",
        "dynamodb",
        Some("dynamodb:table"),
    ),
    // `resource-type:resource-id`.
    (
        "arn:aws:logs:us-east-1:123456789012:log-group:packer",
        "logs",
        Some("logs:log-group"),
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
        serde_json::json!({ "ResourceTypeFilters": ["s3:bucket", "ec2:instance"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ResourceTypeFilters=[s3:bucket, ec2:instance]: body {body}"
    );
    assert_eq!(
        page_arns(&body),
        vec![
            "arn:aws:ec2:us-east-1:123456789012:instance/i-0abc".to_string(),
            "arn:aws:s3:::bucket-a".to_string(),
        ],
        "case ResourceTypeFilters=[s3:bucket, ec2:instance]: got {}",
        body["ResourceTagMappingList"]
    );
}

#[tokio::test]
async fn tag_filters_and_across_keys_and_or_within_one_filters_values() {
    let router = emulator();
    // Three resources whose tag sets differ in value, in key set, and in count.
    let alpha = "arn:aws:s3:::alpha";
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
    let arn = "arn:aws:s3:::bucket-a";
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
            "ResourceARNList": ["arn:aws:s3:::never-tagged"],
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
    let alpha = "arn:aws:s3:::alpha";
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
    let arns: Vec<String> = (0..7).map(|n| format!("arn:aws:s3:::bucket-{n}")).collect();
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
    let arns: Vec<String> = (0..11).map(|n| format!("arn:aws:s3:::bucket-{n:02}")).collect();
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
        let arn = format!("arn:aws:s3:::bucket-{chunk}");
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
    let arns: Vec<String> = (0..3).map(|n| format!("arn:aws:s3:::bucket-{n}")).collect();
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
                "PaginationToken": "deadbeefdeadbeef:4000000000:arn:aws:s3:::bucket-0"
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
    tag(&router, &["arn:aws:s3:::bucket-a"], &[("Env", "prod")]).await;

    // "You can't specify both ResourceARNList and ResourceTypeFilters /
    // TagFilters / ResourcesPerPage / TagsPerPage / PaginationToken."
    let exclusions: &[(&str, serde_json::Value)] = &[
        (
            "TagFilters",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:s3:::bucket-a"],
                "TagFilters": [{ "Key": "Env" }]
            }),
        ),
        (
            "ResourceTypeFilters",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:s3:::bucket-a"],
                "ResourceTypeFilters": ["s3"]
            }),
        ),
        (
            "ResourcesPerPage",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:s3:::bucket-a"],
                "ResourcesPerPage": 1
            }),
        ),
        (
            "TagsPerPage",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:s3:::bucket-a"],
                "TagsPerPage": 100
            }),
        ),
        (
            "PaginationToken",
            serde_json::json!({
                "ResourceARNList": ["arn:aws:s3:::bucket-a"],
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
    let arn = "arn:aws:s3:::bucket-a";
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
    let arn = "arn:aws:s3:::bucket-a";
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
    let filler: Vec<String> = (0..999).map(|n| format!("arn:aws:s3:::q{n:04}")).collect();
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
            "ResourceARNList": ["arn:aws:s3:::one-too-many", filler[1].clone()],
            "Tags": { "Env": "prod" }
        }),
    )
    .await;
    assert_eq!(status, 200, "case store at capacity: expected HTTP 200, got {body}");
    assert_eq!(
        body["FailedResourcesMap"]["arn:aws:s3:::one-too-many"]["ErrorCode"],
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
        serde_json::json!({ "ResourceARNList": ["arn:aws:s3:::one-too-many"] }),
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
