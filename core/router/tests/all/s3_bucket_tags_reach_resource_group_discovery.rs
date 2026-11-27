// SPDX-License-Identifier: CC0-1.0

//! A bucket tagged through Amazon S3's own API is a resource of the account a tag query
//! selects over, so a `TAG_FILTERS_1_0` group really holds it and an application created
//! over that group onboards it.
//!
//! The rule under test: a resource whose tag set the service that OWNS it models is a
//! resource of the ACCOUNT's tagged-resource view — the view `GetResources` reports and
//! the one a Resource Groups `TAG_FILTERS_1_0` query is resolved against
//! ("resource-groups:ListGroupResources … tag:GetResources" among `ListGroupResources`'
//! required permissions) — under the ARN that service minted for it, with exactly the tags
//! it holds there, and only while it holds them. A general purpose bucket is such a
//! resource: it has ONE tag set however it is written, so the set `PutBucketTagging`
//! recorded is the set that view reports, and a set written through the tagging API is the
//! set `GetBucketTagging` reports.
//!
//! Everything downstream follows from that one view: the membership of a query-defined
//! group really holds the bucket, and `CreateApplication` over that group onboards each
//! resolved resource as a DEFAULT
//! (standalone) component named by that resource's own ARN, unmonitored, so
//! `DescribeComponentConfiguration` for that ARN answers 200 with `Monitor` false, `Tier`
//! DEFAULT and no configuration content; `UpdateComponentConfiguration` stores what it is
//! given; and a later describe reports exactly that back. A component the application
//! really does not hold is the modelled `ResourceNotFoundException`.
//!
//! Four services have to agree for this to hold — Amazon S3, the resource-tagging view,
//! Resource Groups and Application Insights — so the suite lives here rather than in any
//! one crate. The sibling `applicationinsights_component_discovery.rs` pins onboarding
//! from a group whose membership was recorded by hand.
//!
//! The rows span what the rule generalises over: how many pairs a tag set carries, a value
//! carrying a space, how many tag keys a query names and whether it names a value at all,
//! which resource types the query admits (every supported type, the bucket type, another
//! service's type), how many resources the group resolves to (none, one, several), the
//! Region a bucket lives in, a bucket nothing ever tagged, a bucket whose tags are removed
//! and written again, and what a configuration sets (monitoring on and off, a tier, an
//! escaped-JSON document).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. Nothing waits on the wall clock: the one state a
//! clock owes (an application leaving `CREATING`) is polled for.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::applicationinsights_wire::{
    application_insights, resource_groups, Answer, BOOT_STACK_SIZE,
};
use crate::cloudformation_provisions_stack_resources::xml_texts;

/// The Region every request below is signed for, and the Region the buckets, the groups
/// and the applications live in.
const REGION: &str = "us-west-2";

/// A second Region, for the row that pins the view being per-Region: a bucket lives in the
/// Region its create placed it in, and a query of another Region's group does not select
/// it.
const OTHER_REGION: &str = "eu-central-1";

/// The XML namespace S3's own model declares, which a client's payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `X-Amz-Target` prefix the resource-tagging API's awsJson wire names its operations
/// under.
const TAGGING_TARGET: &str = "ResourceGroupsTaggingAPI_20170126";

/// How long a poll keeps looking for a state the clock owes. Generous: it bounds nothing a
/// row asserts, it only stops a broken emulator from hanging.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// The tag key every query below selects on, and a second key one query narrows with.
const MARKER: &str = "ai-probe";
const STAGE: &str = "Stage";

/// The whole emulator over one engine, exactly as the binary assembles it, with signature
/// verification off: the `Authorization` header below is what routes a request and tells a
/// handler its Region.
fn router() -> MultiServiceRouter {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = build_router(engine);
    router.set_request_authentication(false);
    router
}

/// One response whose body is XML: the status and the body as text.
struct XmlAnswer {
    status: u16,
    body: String,
}

/// Dispatch one request to `signing_name` in `region`, the way a signed SDK call is: the
/// credential scope is what names the service and the Region.
fn s3(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    method: &str,
    path: &str,
    body: &str,
) -> XmlAnswer {
    let host = format!("s3.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/xml")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/s3/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    XmlAnswer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// Escape a value for an XML text node, the way a client's XML writer does.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The `<Tag>` elements of a tag set, the element name S3's model gives `TagSet`'s member.
fn tag_elements(pairs: &BTreeMap<String, String>) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect()
}

/// An S3 bucket's ARN: `arn:aws:s3:::<bucket>`, whose Region and account fields are empty
/// because a bucket name is globally unique.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// `PUT /<bucket>` — S3's `CreateBucket`, placing the bucket in `region`: a create sent to
/// a regional endpoint names its location constraint.
fn create_bucket(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    bucket: &str,
) {
    let answer = s3(
        router,
        runtime,
        region,
        "PUT",
        &format!("/{bucket}"),
        &format!(
            "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
             <LocationConstraint>{region}</LocationConstraint></CreateBucketConfiguration>"
        ),
    );
    assert_eq!(
        answer.status, 200,
        "setup: CreateBucket({bucket:?}) in {region} must succeed, got {} / {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>?tagging` — S3's `PutBucketTagging`, which REPLACES the whole set.
fn put_bucket_tagging(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    bucket: &str,
    pairs: &BTreeMap<String, String>,
) {
    let answer = s3(
        router,
        runtime,
        region,
        "PUT",
        &format!("/{bucket}?tagging"),
        &format!(
            "<Tagging xmlns=\"{S3_XMLNS}\"><TagSet>{}</TagSet></Tagging>",
            tag_elements(pairs)
        ),
    );
    assert_eq!(
        answer.status, 204,
        "setup: PutBucketTagging({bucket:?}) must succeed, got {} / {}",
        answer.status, answer.body
    );
}

/// `DELETE /<bucket>?tagging` — S3's `DeleteBucketTagging`, which leaves the bucket with
/// no tag set at all.
fn delete_bucket_tagging(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    bucket: &str,
) {
    let answer = s3(
        router,
        runtime,
        region,
        "DELETE",
        &format!("/{bucket}?tagging"),
        "",
    );
    assert_eq!(
        answer.status, 204,
        "setup: DeleteBucketTagging({bucket:?}) must succeed, got {} / {}",
        answer.status, answer.body
    );
}

/// `GET /<bucket>?tagging` — S3's `GetBucketTagging`, as the `(Key, Value)` pairs it
/// reports in key order (a tag set is a set, so no member order is part of the contract)
/// beside the `<Code>` of a failure.
fn bucket_tagging(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    bucket: &str,
) -> (BTreeMap<String, String>, String) {
    let answer = s3(
        router,
        runtime,
        region,
        "GET",
        &format!("/{bucket}?tagging"),
        "",
    );
    let keys = xml_texts(&answer.body, "Key");
    let values = xml_texts(&answer.body, "Value");
    let code = xml_texts(&answer.body, "Code")
        .into_iter()
        .next()
        .unwrap_or_default();
    (keys.into_iter().zip(values).collect(), code)
}

/// One resource-tagging call over its awsJson1_1 wire, in `region`.
fn tagging(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    operation: &str,
    body: Value,
) -> Answer {
    let host = format!("tagging.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TAGGING_TARGET}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/tagging/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let payload = response.into_body();
    let body = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// One resource-tagging call that must answer the modelled 200.
fn tagging_ok(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let answer = tagging(router, runtime, region, operation, body);
    assert_eq!(
        answer.status, 200,
        "case {operation} in {region}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The `(ARN -> tags)` map one `GetResources` answer reports.
fn reported(answer: &Value) -> BTreeMap<String, BTreeMap<String, String>> {
    answer["ResourceTagMappingList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|mapping| {
            let arn = mapping["ResourceARN"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let tags = mapping["Tags"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            (arn, tags)
        })
        .collect()
}

/// The ARNs one `GetResources` request selects in `region`, in ARN order.
fn selected(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    request: Value,
) -> Vec<String> {
    let answer = tagging_ok(router, runtime, region, "GetResources", request);
    let mut arns: Vec<String> = reported(&answer).into_keys().collect();
    arns.sort();
    arns
}

/// The tag pairs a row names, as the one ordered map this suite handles tags in.
fn pairs(of: &[(&str, &str)]) -> BTreeMap<String, String> {
    of.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// A bucket of `region` carrying `tags`, as the ARN S3 names it by.
fn tagged_bucket(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    bucket: &str,
    tags: &[(&str, &str)],
) -> String {
    create_bucket(router, runtime, region, bucket);
    let tags = pairs(tags);
    if !tags.is_empty() {
        put_bucket_tagging(router, runtime, region, bucket, &tags);
    }
    bucket_arn(bucket)
}

/// Create the Resource Groups group `name` in `region`, defined by the tag query
/// `document`. The model carries a `ResourceQuery`'s `Query` as a JSON STRING inside the
/// structure (`smithy_query(query="shape",shape="ResourceQuery")`).
fn tag_query_group(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    name: &str,
    document: &Value,
) {
    let answer = resource_groups(
        router,
        runtime,
        region,
        "/groups",
        json!({
            "Name": name,
            "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": document.to_string() }
        }),
    );
    assert_eq!(
        answer.status, 200,
        "setup: CreateGroup({name:?}) in {region} must succeed, got {} / {:?}",
        answer.status, answer.body
    );
}

/// The `(ARN, ResourceType)` pairs `ListGroupResources` reports for `group` in `region`, in
/// ARN order.
fn group_members(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    group: &str,
) -> Vec<(String, Option<String>)> {
    let answer = resource_groups(
        router,
        runtime,
        region,
        "/list-group-resources",
        json!({ "Group": group }),
    );
    assert_eq!(
        answer.status, 200,
        "ListGroupResources({group:?}) must be served, got {} / {:?}",
        answer.status, answer.body
    );
    let mut members: Vec<(String, Option<String>)> = answer.body["Resources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                item["Identifier"]["ResourceArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                item["Identifier"]["ResourceType"].as_str().map(str::to_string),
            )
        })
        .collect();
    members.sort();
    members
}

/// The ARNs `ListGroupResources` reports for `group` in [`REGION`], in ARN order.
fn group_membership(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
) -> Vec<String> {
    group_members(router, runtime, REGION, group)
        .into_iter()
        .map(|(arn, _)| arn)
        .collect()
}

/// Create the application over `group`, which must succeed.
fn create_application(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
) {
    let answer = application_insights(
        router,
        runtime,
        REGION,
        "CreateApplication",
        json!({ "ResourceGroupName": group }),
    );
    assert_eq!(
        answer.status, 200,
        "setup: CreateApplication({group:?}) must succeed, got {} / {:?}",
        answer.status, answer.body
    );
}

/// `DescribeComponentConfiguration` for one component of one application.
fn describe_configuration(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
    component: &str,
) -> Answer {
    application_insights(
        router,
        runtime,
        REGION,
        "DescribeComponentConfiguration",
        json!({ "ResourceGroupName": group, "ComponentName": component }),
    )
}

/// `UpdateComponentConfiguration` for one component of one application.
fn update_configuration(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
    component: &str,
    monitor: bool,
    tier: &str,
    configuration: &str,
) -> Answer {
    application_insights(
        router,
        runtime,
        REGION,
        "UpdateComponentConfiguration",
        json!({
            "ResourceGroupName": group,
            "ComponentName": component,
            "Monitor": monitor,
            "Tier": tier,
            "ComponentConfiguration": configuration,
        }),
    )
}

/// The `ComponentName`s `ListComponents` reports for `group`.
fn component_names(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
) -> BTreeSet<String> {
    let answer = application_insights(
        router,
        runtime,
        REGION,
        "ListComponents",
        json!({ "ResourceGroupName": group }),
    );
    assert_eq!(
        answer.status, 200,
        "ListComponents({group:?}) must be served, got {} / {:?}",
        answer.status, answer.body
    );
    answer.body["ApplicationComponentList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|component| component["ComponentName"].as_str().map(str::to_string))
        .collect()
}

/// The `LifeCycle` `DescribeApplication` reports for `group`.
fn life_cycle(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
) -> String {
    let answer = application_insights(
        router,
        runtime,
        REGION,
        "DescribeApplication",
        json!({ "ResourceGroupName": group }),
    );
    assert_eq!(
        answer.status, 200,
        "DescribeApplication({group:?}) must be served, got {} / {:?}",
        answer.status, answer.body
    );
    answer.body["ApplicationInfo"]["LifeCycle"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Look again until `ready` holds, or until [`POLL_DEADLINE`] passes — a transition the
/// CLOCK owes is waited for by looking for the condition, never by sleeping a fixed time.
fn poll_until(case: &str, what: &str, mut ready: impl FnMut() -> bool) {
    let since = Instant::now();
    loop {
        if ready() {
            return;
        }
        assert!(
            since.elapsed() < POLL_DEADLINE,
            "case {case:?}: {what} did not hold while polling"
        );
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// Run `body` on a thread with the stack the whole router's assembly needs.
fn on_a_sized_thread(name: &str, body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(body)
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}

/// The CloudFormation-style type a bucket is reported under, from the AWS CloudFormation
/// resource and property types reference — the vocabulary `ResourceIdentifier$ResourceType`
/// is documented in (`smithy_query(query="shape",shape="ResourceIdentifier")`) — beside a
/// supported type nothing in these accounts is.
const BUCKET_TYPE: &str = "AWS::S3::Bucket";
const QUEUE_TYPE: &str = "AWS::SQS::Queue";

/// One bucket of the account every row of the view test is matched against: its name, the
/// Region its create placed it in, and the tag set S3's own API was given for it.
struct Held {
    bucket: &'static str,
    region: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// One write through the tagging API: the case, the operation it is made with, the request
/// body, and the whole tag set the bucket must hold afterwards.
struct TagWrite {
    case: &'static str,
    operation: &'static str,
    body: Value,
    expected: &'static [(&'static str, &'static str)],
}

/// A bucket has ONE tag set, whichever API writes it, and that set is what the account's
/// tagged-resource view reports: `GetResources` selects the bucket by its own type and tag
/// filters, a `TagResources` write is what `GetBucketTagging` then reports, an
/// `UntagResources` removes it there too, and an ARN that service holds no bucket for is
/// reported with ITS own not-found identity rather than tagged into a store of its own.
#[test]
fn a_buckets_tag_set_is_the_one_the_accounts_tagged_resource_view_reads_and_writes() {
    on_a_sized_thread("s3-bucket-tags-view", || {
        let router = router();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");

        // (the bucket, its Region, the tag set S3's own API was given)
        let rows: Vec<Held> = vec![
            Held {
                bucket: "aitag-view-one",
                region: REGION,
                tags: &[(MARKER, "1"), (STAGE, "Test")],
            },
            Held {
                bucket: "aitag-view-two",
                region: REGION,
                tags: &[(MARKER, "1")],
            },
            // A value carrying a space, under a key the selections below do not name.
            Held {
                bucket: "aitag-view-three",
                region: REGION,
                tags: &[("Owner", "platform team")],
            },
            // Never tagged: "GetResources does not return untagged resources".
            Held {
                bucket: "aitag-view-untagged",
                region: REGION,
                tags: &[],
            },
            // Another Region's bucket, reported by that Region's endpoint and not by this
            // one.
            Held {
                bucket: "aitag-view-elsewhere",
                region: OTHER_REGION,
                tags: &[(MARKER, "1")],
            },
        ];
        let mut arns: BTreeMap<&str, String> = BTreeMap::new();
        for row in &rows {
            arns.insert(
                row.bucket,
                tagged_bucket(&router, &runtime, row.region, row.bucket, row.tags),
            );
        }

        // Every tagged bucket of this Region, with exactly the tags S3 holds for it.
        let answer = tagging_ok(&router, &runtime, REGION, "GetResources", json!({}));
        let view = reported(&answer);
        for row in &rows {
            let arn = &arns[row.bucket];
            let expected: Option<BTreeMap<String, String>> =
                (!row.tags.is_empty() && row.region == REGION).then(|| pairs(row.tags));
            assert_eq!(
                view.get(arn),
                expected.as_ref(),
                "case {:?} in {}: expected the account's view to report {expected:?} for \
                 {arn}, got {:?}",
                row.bucket,
                row.region,
                view.get(arn)
            );
        }

        // The other Region's endpoint reports its own bucket, and not this Region's.
        let elsewhere = selected(&router, &runtime, OTHER_REGION, json!({}));
        assert_eq!(
            elsewhere,
            vec![arns["aitag-view-elsewhere"].clone()],
            "case the other Region's endpoint: expected only its own bucket, got \
             {elsewhere:?}"
        );

        // (the case, the request, the ARNs it must select)
        let selections: Vec<(&str, Value, Vec<String>)> = vec![
            (
                "the bare service token",
                json!({ "ResourceTypeFilters": ["s3"] }),
                vec![
                    arns["aitag-view-one"].clone(),
                    arns["aitag-view-three"].clone(),
                    arns["aitag-view-two"].clone(),
                ],
            ),
            (
                "the qualified bucket type",
                json!({ "ResourceTypeFilters": ["s3:bucket"] }),
                vec![
                    arns["aitag-view-one"].clone(),
                    arns["aitag-view-three"].clone(),
                    arns["aitag-view-two"].clone(),
                ],
            ),
            (
                "another type of the same service",
                json!({ "ResourceTypeFilters": ["s3:accesspoint"] }),
                Vec::new(),
            ),
            (
                "one tag key and value",
                json!({ "TagFilters": [{ "Key": MARKER, "Values": ["1"] }] }),
                vec![
                    arns["aitag-view-one"].clone(),
                    arns["aitag-view-two"].clone(),
                ],
            ),
            (
                "two tag keys",
                json!({
                    "TagFilters": [
                        { "Key": MARKER, "Values": ["1"] },
                        { "Key": STAGE, "Values": ["Test"] }
                    ]
                }),
                vec![arns["aitag-view-one"].clone()],
            ),
            (
                "a key with no value",
                json!({ "TagFilters": [{ "Key": "Owner" }] }),
                vec![arns["aitag-view-three"].clone()],
            ),
            (
                "a value no tag carries",
                json!({ "TagFilters": [{ "Key": MARKER, "Values": ["9"] }] }),
                Vec::new(),
            ),
        ];
        for (case, request, expected) in selections {
            let listed = selected(&router, &runtime, REGION, request.clone());
            assert_eq!(
                listed, expected,
                "case {case:?} ({request}): expected {expected:?}, got {listed:?}"
            );
        }

        // A write through this API edits the BUCKET's own tag set, which is what S3
        // reports; an untag removes exactly the named key from that same set.
        let bucket = "aitag-view-one";
        let arn = arns[bucket].clone();
        // (the case, the call, its body, the whole set the bucket must hold afterwards)
        let writes: Vec<TagWrite> = vec![
            TagWrite {
                case: "one pair",
                operation: "TagResources",
                body: json!({ "ResourceARNList": [&arn], "Tags": { "Owner": "platform" } }),
                expected: &[(MARKER, "1"), ("Owner", "platform"), (STAGE, "Test")],
            },
            TagWrite {
                case: "two pairs, one of them an overwrite",
                operation: "TagResources",
                body: json!({
                    "ResourceARNList": [&arn],
                    "Tags": { "Owner": "platform team", (MARKER): "2" }
                }),
                expected: &[(MARKER, "2"), ("Owner", "platform team"), (STAGE, "Test")],
            },
            TagWrite {
                case: "the keys removed again",
                operation: "UntagResources",
                body: json!({ "ResourceARNList": [&arn], "TagKeys": ["Owner", STAGE] }),
                expected: &[(MARKER, "2")],
            },
        ];
        for write in writes {
            let answered = tagging_ok(&router, &runtime, REGION, write.operation, write.body);
            assert_eq!(
                answered["FailedResourcesMap"],
                json!({}),
                "case {:?}: {} on {arn} must fail for nothing, got {answered}",
                write.case,
                write.operation
            );
            let expected = pairs(write.expected);
            let (held, _) = bucket_tagging(&router, &runtime, REGION, bucket);
            assert_eq!(
                held, expected,
                "case {:?}: expected S3 to report {expected:?}, got {held:?}",
                write.case
            );
        }

        // An ARN the owning service holds no bucket for: ITS code and status, per resource,
        // with the rest of the batch served and nothing brought into being for it.
        let ghost = bucket_arn("aitag-view-ghost");
        let answered = tagging_ok(
            &router,
            &runtime,
            REGION,
            "TagResources",
            json!({ "ResourceARNList": [&ghost, &arn], "Tags": { "Owner": "platform" } }),
        );
        assert_eq!(
            (
                answered["FailedResourcesMap"][&ghost]["ErrorCode"].clone(),
                answered["FailedResourcesMap"][&ghost]["StatusCode"].clone(),
            ),
            (json!("NoSuchBucket"), json!(404)),
            "case the absent bucket: expected S3's own code and status, got {answered}"
        );
        let (held, _) = bucket_tagging(&router, &runtime, REGION, bucket);
        assert_eq!(
            held.get("Owner").map(String::as_str),
            Some("platform"),
            "case the absent bucket: the served half of the batch must still be written, \
             got {held:?}"
        );
        let (_, code) = bucket_tagging(&router, &runtime, REGION, "aitag-view-ghost");
        assert_eq!(
            code, "NoSuchBucket",
            "case the absent bucket: nothing may have brought it into being, got {code:?}"
        );
    });
}

/// A group whose query selects on tags holds the buckets Amazon S3 holds those tags for:
/// every query form over one account, each member reported under the type its ARN names,
/// and only for the Region the bucket lives in.
#[test]
fn a_tag_query_group_holds_the_buckets_tagged_through_s3() {
    on_a_sized_thread("s3-bucket-tags-membership", || {
        let router = router();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");

        let one = tagged_bucket(
            &router,
            &runtime,
            REGION,
            "aitag-member-one",
            &[(MARKER, "1"), (STAGE, "Test")],
        );
        let two = tagged_bucket(&router, &runtime, REGION, "aitag-member-two", &[(MARKER, "1")]);
        // A value carrying a space, under a key no query below selects on.
        let three = tagged_bucket(
            &router,
            &runtime,
            REGION,
            "aitag-member-three",
            &[("Owner", "platform team")],
        );
        // Never tagged: no query selects it, whatever it names.
        tagged_bucket(&router, &runtime, REGION, "aitag-member-untagged", &[]);
        // Another Region's bucket, carrying the very tags the queries select on.
        tagged_bucket(
            &router,
            &runtime,
            OTHER_REGION,
            "aitag-member-elsewhere",
            &[(MARKER, "1")],
        );

        // (the case, the query document a group is defined by, the ARNs it must hold)
        let rows: Vec<(&str, Value, Vec<String>)> = vec![
            (
                "one key and value, every supported type",
                json!({
                    "ResourceTypeFilters": ["AWS::AllSupported"],
                    "TagFilters": [{ "Key": MARKER, "Values": ["1"] }]
                }),
                vec![one.clone(), two.clone()],
            ),
            (
                "the bucket type alone",
                json!({
                    "ResourceTypeFilters": [BUCKET_TYPE],
                    "TagFilters": [{ "Key": MARKER, "Values": ["1"] }]
                }),
                vec![one.clone(), two.clone()],
            ),
            (
                "another service's type",
                json!({
                    "ResourceTypeFilters": [QUEUE_TYPE],
                    "TagFilters": [{ "Key": MARKER, "Values": ["1"] }]
                }),
                Vec::new(),
            ),
            (
                "two keys narrow",
                json!({
                    "TagFilters": [
                        { "Key": MARKER, "Values": ["1"] },
                        { "Key": STAGE, "Values": ["Test"] }
                    ]
                }),
                vec![one.clone()],
            ),
            (
                "a key with no value",
                json!({ "TagFilters": [{ "Key": "Owner" }] }),
                vec![three.clone()],
            ),
            (
                "several values of one key widen",
                json!({ "TagFilters": [{ "Key": MARKER, "Values": ["1", "2"] }] }),
                vec![one.clone(), two.clone()],
            ),
            (
                "a value no tag carries",
                json!({ "TagFilters": [{ "Key": MARKER, "Values": ["9"] }] }),
                Vec::new(),
            ),
        ];

        for (at, (case, document, expected)) in rows.iter().enumerate() {
            let group = format!("aitag-member-{at}");
            tag_query_group(&router, &runtime, REGION, &group, document);
            let mut expected_members: Vec<(String, Option<String>)> = expected
                .iter()
                .map(|arn| (arn.clone(), Some(BUCKET_TYPE.to_string())))
                .collect();
            expected_members.sort();
            let listed = group_members(&router, &runtime, REGION, &group);
            assert_eq!(
                listed, expected_members,
                "case {case:?} ({document}): expected the membership {expected_members:?}, \
                 got {listed:?}"
            );
        }

        // The other Region's own group selects its own bucket and nothing of this Region.
        tag_query_group(
            &router,
            &runtime,
            OTHER_REGION,
            "aitag-member-elsewhere-group",
            &json!({ "TagFilters": [{ "Key": MARKER, "Values": ["1"] }] }),
        );
        let elsewhere = group_members(
            &router,
            &runtime,
            OTHER_REGION,
            "aitag-member-elsewhere-group",
        );
        assert_eq!(
            elsewhere,
            vec![(
                bucket_arn("aitag-member-elsewhere"),
                Some(BUCKET_TYPE.to_string())
            )],
            "case the other Region's group: expected only its own bucket, got {elsewhere:?}"
        );

        // The membership is DERIVED, so it follows the owning service's own tag set:
        // removing the tags there takes the bucket out of the group, writing them again
        // puts it back.
        let group = "aitag-member-following";
        tag_query_group(
            &router,
            &runtime,
            REGION,
            group,
            &json!({ "TagFilters": [{ "Key": MARKER, "Values": ["1"] }] }),
        );
        // What a step does to the bucket in S3 before it reads the membership again.
        enum Write {
            /// Nothing: the membership as the fixtures left it.
            Nothing,
            /// `DeleteBucketTagging`, which leaves the bucket with no tag set at all.
            DeleteTags,
            /// `PutBucketTagging`, which replaces the whole set with these pairs.
            PutTags(&'static [(&'static str, &'static str)]),
        }

        // (the case, what is done to the bucket in S3, the membership it leaves)
        let steps: Vec<(&str, Write, Vec<String>)> = vec![
            ("tagged", Write::Nothing, vec![one.clone(), two.clone()]),
            (
                "its whole tag set deleted",
                Write::DeleteTags,
                vec![two.clone()],
            ),
            (
                "tagged again",
                Write::PutTags(&[(MARKER, "1")]),
                vec![one.clone(), two.clone()],
            ),
            (
                "tagged with another value",
                Write::PutTags(&[(MARKER, "9")]),
                vec![two.clone()],
            ),
        ];
        for (case, write, expected) in steps {
            match write {
                Write::Nothing => {}
                Write::DeleteTags => {
                    delete_bucket_tagging(&router, &runtime, REGION, "aitag-member-one")
                }
                Write::PutTags(tags) => put_bucket_tagging(
                    &router,
                    &runtime,
                    REGION,
                    "aitag-member-one",
                    &pairs(tags),
                ),
            }
            let listed = group_membership(&router, &runtime, group);
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the derived membership {expected:?}, got {listed:?}"
            );
        }
    });
}

/// One row of the discovery table: the group's query, and the buckets it must resolve to.
struct Row {
    case: &'static str,
    group: &'static str,
    query: Value,
    members: Vec<String>,
}

/// An application created over a query-defined group onboards every resource that query
/// selects as a DEFAULT component named by the resource's own ARN, unmonitored — so its
/// configuration can be read, written and read back, while a component the application
/// does not hold is the modelled not-found refusal.
#[test]
fn a_tag_query_group_onboards_each_tagged_bucket_as_a_component() {
    on_a_sized_thread("s3-bucket-tags-components", || {
        let router = router();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");

        let one = tagged_bucket(
            &router,
            &runtime,
            REGION,
            "aitag-comp-one",
            &[(MARKER, "1"), (STAGE, "Test")],
        );
        let two = tagged_bucket(&router, &runtime, REGION, "aitag-comp-two", &[(MARKER, "1")]);
        // Tagged, but with a value no query below selects.
        tagged_bucket(&router, &runtime, REGION, "aitag-comp-other", &[(MARKER, "2")]);

        let rows: Vec<Row> = vec![
            Row {
                case: "every supported type, one key and value",
                group: "aitag-comp-all",
                query: json!({
                    "ResourceTypeFilters": ["AWS::AllSupported"],
                    "TagFilters": [{ "Key": MARKER, "Values": ["1"] }]
                }),
                members: vec![one.clone(), two.clone()],
            },
            Row {
                case: "the bucket type alone, two keys",
                group: "aitag-comp-typed",
                query: json!({
                    "ResourceTypeFilters": [BUCKET_TYPE],
                    "TagFilters": [
                        { "Key": MARKER, "Values": ["1"] },
                        { "Key": STAGE, "Values": ["Test"] }
                    ]
                }),
                members: vec![one.clone()],
            },
            Row {
                case: "a query nothing satisfies",
                group: "aitag-comp-none",
                query: json!({
                    "ResourceTypeFilters": ["AWS::AllSupported"],
                    "TagFilters": [{ "Key": MARKER, "Values": ["3"] }]
                }),
                members: Vec::new(),
            },
        ];

        for row in &rows {
            tag_query_group(&router, &runtime, REGION, row.group, &row.query);
            let mut expected = row.members.clone();
            expected.sort();

            // The group really holds what its query selects.
            let listed = group_membership(&router, &runtime, row.group);
            assert_eq!(
                listed, expected,
                "case {:?}: expected the group's membership {expected:?}, got {listed:?}",
                row.case
            );

            create_application(&router, &runtime, row.group);
            let components = component_names(&router, &runtime, row.group);
            assert_eq!(
                components,
                expected.iter().cloned().collect::<BTreeSet<String>>(),
                "case {:?}: expected one component per resolved resource, got \
                 {components:?}",
                row.case
            );

            for (at, arn) in expected.iter().enumerate() {
                // Unconfigured: unmonitored, in the DEFAULT tier, with no
                // configuration document stored for it.
                let described = describe_configuration(&router, &runtime, row.group, arn);
                assert_eq!(
                    described.status, 200,
                    "case {:?} / {arn}: expected HTTP 200, got {} / {:?}",
                    row.case, described.status, described.body
                );
                assert_eq!(
                    (
                        described.body["Monitor"].clone(),
                        described.body["Tier"].clone(),
                        described.body["ComponentConfiguration"].clone(),
                    ),
                    (json!(false), json!("DEFAULT"), json!("")),
                    "case {:?} / {arn}: an onboarded component is unmonitored, in the \
                     DEFAULT tier, with nothing configured, got {:?}",
                    row.case,
                    described.body
                );

                // What a configuration stores is what the next read reports: the rows
                // differ in the monitoring they ask for and in the document they carry.
                let monitor = at % 2 == 0;
                let configuration = json!({ "subComponents": [], "position": at }).to_string();
                let updated = update_configuration(
                    &router,
                    &runtime,
                    row.group,
                    arn,
                    monitor,
                    "DEFAULT",
                    &configuration,
                );
                assert_eq!(
                    (updated.status, updated.body.clone()),
                    (200, json!({})),
                    "case {:?} / {arn}: the output shape is empty, got {} / {:?}",
                    row.case,
                    updated.status,
                    updated.body
                );

                let described = describe_configuration(&router, &runtime, row.group, arn);
                assert_eq!(
                    described.status, 200,
                    "case {:?} / {arn}: expected HTTP 200, got {} / {:?}",
                    row.case, described.status, described.body
                );
                assert_eq!(
                    (
                        described.body["Monitor"].clone(),
                        described.body["Tier"].clone(),
                        described.body["ComponentConfiguration"].clone(),
                    ),
                    (json!(monitor), json!("DEFAULT"), json!(configuration)),
                    "case {:?} / {arn}: expected the stored configuration back, got {:?}",
                    row.case,
                    described.body
                );
            }

            // Monitoring one component is what moves the application to ACTIVE; the
            // onboarding the create started is owed by the clock, so this is polled for.
            let expected_life_cycle = if expected.is_empty() {
                "NOT_CONFIGURED"
            } else {
                "ACTIVE"
            };
            poll_until(row.case, "the application reached its LifeCycle", || {
                life_cycle(&router, &runtime, row.group) == expected_life_cycle
            });

            // A component the application does not hold is the modelled refusal. The
            // status is the one this service answers every refusal about what the caller
            // asked for with, the not-found one included; the error CODE is what tells
            // the classes apart.
            for component in ["does-not-exist", &bucket_arn("aitag-comp-never")] {
                let described = describe_configuration(&router, &runtime, row.group, component);
                assert_eq!(
                    (described.status, described.error_code()),
                    (400, "ResourceNotFoundException".to_string()),
                    "case {:?} / {component}: expected the modelled not-found refusal, got \
                     {} / {:?}",
                    row.case,
                    described.status,
                    described.body
                );
                let updated = update_configuration(
                    &router,
                    &runtime,
                    row.group,
                    component,
                    true,
                    "DEFAULT",
                    "{}",
                );
                assert_eq!(
                    (updated.status, updated.error_code()),
                    (400, "ResourceNotFoundException".to_string()),
                    "case {:?} / {component}: expected the modelled not-found refusal, got \
                     {} / {:?}",
                    row.case,
                    updated.status,
                    updated.body
                );
            }
        }
    });
}
