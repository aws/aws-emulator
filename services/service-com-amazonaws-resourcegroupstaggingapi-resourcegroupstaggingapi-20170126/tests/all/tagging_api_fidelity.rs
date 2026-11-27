// SPDX-License-Identifier: CC0-1.0

//! The fidelity rules this API is held to beyond the read/write loop in
//! `tag_read_write_loop.rs`: WHICH ARNs it will tag, WHERE the tags it writes
//! live, and which refusal each kind of bad request gets.
//!
//! Every test drives the real wire path — the multi-service router, `POST /`
//! with an `X-Amz-Target` and an awsJson body — so what is asserted is what an
//! SDK client sees. The router carries the OWNING services too, over ONE shared
//! engine, because that is how the emulator runs: the tagging API resolves an
//! ARN against the service that owns it, so a resource has to be created
//! through that service's own API before it can be tagged through this one.
//!
//! The tables span the class each rule generalises over (owning service and
//! resource kind, ARN form, Region, partition, tag-key namespace, page size,
//! token provenance); the ARNs and values the issue cited are one row each.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// The owning services, aliased to the role they play here, exactly as
// `src/state/owners.rs` aliases them.
use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as owner_autoscaling;
use service_com_amazonaws_ecs_amazonec2containerservicev20141113 as owner_ecs;
use service_com_amazonaws_eventbridge_awsevents as owner_events;
use service_com_amazonaws_kms_trentservice as owner_kms;
use service_com_amazonaws_secretsmanager_secretsmanager as owner_secretsmanager;
use service_com_amazonaws_sfn_awsstepfunctions as owner_sfn;
use service_com_amazonaws_sqs_amazonsqs as owner_sqs;

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::state as tagging_state;
use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and therefore the one the
/// tagging endpoint serves.
const REGION: &str = "us-east-1";

/// The SigV4 signing name of each service the tests call, which is the routing
/// signal that picks it out of the router.
const TAGGING: &str = "tagging";
const SQS: &str = "sqs";
const ECS: &str = "ecs";
const SECRETS: &str = "secretsmanager";

/// The SigV4 signing name of Amazon EC2 Auto Scaling, whose awsQuery endpoint creates the
/// Auto Scaling group an ECS capacity provider binds onto.
const AUTO_SCALING: &str = "autoscaling";

/// The launch template the Auto Scaling group below launches from, put in the account's own
/// Amazon EC2 records by [`emulator`]: a group launches from a launch template the account
/// holds, and this router mounts no Amazon EC2 endpoint to create one through.
const CAPACITY_LAUNCH_TEMPLATE: &str = "tagging-capacity";

/// Put that launch template in the account's own Amazon EC2 store, through Amazon EC2's own
/// writer.
///
/// It states the image its instances run, because a group is refused a launch-template version
/// no instance could be launched from: "An AMI ID is required to launch an instance"
/// (`smithy_query(query="doc",shape="RunInstancesRequest",member="ImageId")`), and the instance
/// type they run as, because a group that is not configured as a mixed instances group takes
/// that type from the version it launches from.
fn seed_capacity_launch_template(engine: &Arc<engine::InMemoryEngine>) {
    let ec2 = service_com_amazonaws_ec2_amazonec2::state::service_state(&**engine)
        .expect("the shared engine carries the Amazon EC2 store");
    let mut store = ec2.lock();
    store.insert_launch_template(
        service_com_amazonaws_ec2_amazonec2::state::LaunchTemplateSpec {
            name: CAPACITY_LAUNCH_TEMPLATE.to_string(),
            created_by: engine::emulator_arn("iam", "", "root"),
            create_time: SystemTime::now(),
            tags: Vec::new(),
            operator: None,
            data: service_com_amazonaws_ec2_amazonec2::ops::types::RequestLaunchTemplateData {
                image_id: Some(CAPACITY_IMAGE_ID.to_string()),
                instance_type: Some(CAPACITY_INSTANCE_TYPE.to_string()),
                ..Default::default()
            },
            version_description: None,
        },
    );
}

/// The image the instances of that group would run, in the printed form Amazon EC2 issues an
/// AMI id in: the group this suite creates holds no instance, so what matters is that its
/// template states an image at all.
const CAPACITY_IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type those instances would run as: the group this suite creates holds no
/// instance, so what matters is that its template states a type at all.
const CAPACITY_INSTANCE_TYPE: &str = "t3.micro";

/// A router carrying the tagging API and the owning services it resolves ARNs
/// against, all over ONE engine — exactly the arrangement `core/router` builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_capacity_launch_template(&engine);
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization`
    // header below is present because it is what routes a request to a signing
    // name, not to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_sqs::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_ecs::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_secretsmanager::EmulatedService::new(Arc::clone(
        &engine,
    )));
    router.register_service(owner_kms::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_events::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_sfn::EmulatedService::new(engine));
    router
}

/// Create one Auto Scaling group named `name` in [`REGION`], through Amazon EC2 Auto
/// Scaling's OWN awsQuery API.
///
/// An ECS capacity provider over `autoScalingGroupProvider` binds onto a group of the
/// caller's, and ECS resolves that reference against this service's records, so the group
/// has to exist before the provider that binds onto it can be created. The smallest group
/// that API accepts: a name, a zone of the Region to place instances in, a launch template
/// to launch them from, and a size range that holds none.
async fn create_auto_scaling_group(router: &MultiServiceRouter, name: &str) {
    let zone = format!("{REGION}a");
    let body = [
        ("Action", "CreateAutoScalingGroup"),
        ("Version", "2011-01-01"),
        ("AutoScalingGroupName", name),
        ("LaunchTemplate.LaunchTemplateName", CAPACITY_LAUNCH_TEMPLATE),
        ("MinSize", "0"),
        ("MaxSize", "0"),
        ("AvailabilityZones.member.1", zone.as_str()),
    ]
    .iter()
    .map(|(key, value)| {
        // The form encoding is the ONE this crate's tests carry
        // ([`crate::elb_tagged_resource_index::encode`]), so an awsQuery body built here
        // and one built by the load-balancer suite cannot drift.
        format!(
            "{}={}",
            crate::elb_tagged_resource_index::encode(key),
            crate::elb_tagged_resource_index::encode(value)
        )
    })
    .collect::<Vec<String>>()
    .join("&");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://autoscaling.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{AUTO_SCALING}/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup for {name}: body {payload}"
    );
}

/// Call one operation of one service and return its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call_in_region(router, REGION, signing_name, target, body).await
}

/// [`call`] with the Region the request is SIGNED for, which is the Region the
/// endpoint serves.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    target: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value = if response.body().is_empty() {
        serde_json::Value::Null
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

/// One tagging-API operation.
async fn tagging(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call(
        router,
        TAGGING,
        &format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        body,
    )
    .await
}

/// One tagging-API operation in a named Region.
async fn tagging_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call_in_region(
        router,
        region,
        TAGGING,
        &format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        body,
    )
    .await
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

/// The ARNs `GetResources` reports for one `ResourceTypeFilters` value.
async fn arns_of_type(router: &MultiServiceRouter, filters: &[&str]) -> Vec<String> {
    let (status, body) = tagging(
        router,
        "GetResources",
        serde_json::json!({ "ResourceTypeFilters": filters }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ResourceTypeFilters={filters:?}: body {body}"
    );
    page_arns(&body)
}

/// An ARN of a service no owning crate is linked against, so it needs no live
/// resource behind it: the tagging store is its tag home.
const UNOWNED: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm/fidelity-control";

/// A well-formed ARN naming a resource that was never created is NOT a request
/// fault: the call is a 200 whose `FailedResourcesMap` names that ARN alone,
/// with the OWNING service's own not-found code and `StatusCode` 400, and the
/// other ARNs of the same batch are tagged.
#[tokio::test]
async fn an_absent_resource_fails_with_the_owning_services_not_found_code() {
    let router = emulator();

    // One row per owning service, plus a second resource KIND of one of them
    // (`ecs:task-definition`, which the issue did not cite) to show the rule is
    // per-resource and not per-service. Each code is the one that service's own
    // tagging operation declares — `smithy_query(query="operation",
    // operation="TagResource")` on its model — and for SQS the `awsQueryError`
    // code of `QueueDoesNotExist`.
    let rows: &[(&str, &str, &str)] = &[
        (
            "sqs queue",
            "arn:aws:sqs:us-east-1:000000000000:ghost-queue",
            "AWS.SimpleQueueService.NonExistentQueue",
        ),
        (
            "ecs cluster",
            "arn:aws:ecs:us-east-1:000000000000:cluster/ghost",
            "ClusterNotFoundException",
        ),
        (
            "ecs task definition",
            "arn:aws:ecs:us-east-1:000000000000:task-definition/ghost:1",
            "ResourceNotFoundException",
        ),
        (
            "eventbridge rule",
            "arn:aws:events:us-east-1:000000000000:rule/ghost",
            "ResourceNotFoundException",
        ),
        (
            "kms key",
            "arn:aws:kms:us-east-1:000000000000:key/4e8ec784-0000-4000-8000-000000000000",
            "NotFoundException",
        ),
        (
            "secretsmanager secret",
            "arn:aws:secretsmanager:us-east-1:000000000000:secret:ghost-AbCdEf",
            "ResourceNotFoundException",
        ),
        (
            "step functions state machine",
            "arn:aws:states:us-east-1:000000000000:stateMachine:ghost",
            "ResourceNotFound",
        ),
    ];

    for (case, ghost, expected) in rows {
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({
                "ResourceARNList": [UNOWNED, ghost],
                "Tags": { "Env": "partial" }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected a 200 with a partial failure, got {status} with {body}"
        );
        let failed: Vec<String> = body["FailedResourcesMap"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: no FailedResourcesMap in {body}"))
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            failed,
            vec![(*ghost).to_string()],
            "case {case}: expected exactly the absent ARN to fail, got {failed:?}"
        );
        let failure = &body["FailedResourcesMap"][ghost];
        assert_eq!(
            failure["StatusCode"],
            serde_json::json!(400),
            "case {case}: expected StatusCode 400, got {body}"
        );
        assert_eq!(
            failure["ErrorCode"],
            serde_json::json!(expected),
            "case {case}: expected ErrorCode {expected}, got {body}"
        );

        // Nothing was written for the absent ARN, and the live one was tagged.
        let (_, read) = tagging(
            &router,
            "GetResources",
            serde_json::json!({ "ResourceARNList": [ghost, UNOWNED] }),
        )
        .await;
        assert_eq!(
            page_arns(&read),
            vec![UNOWNED.to_string()],
            "case {case}: only the live ARN may be in the index, got {read}"
        );
    }

    // `UntagResources` answers the same way, per resource.
    let (status, body) = tagging(
        &router,
        "UntagResources",
        serde_json::json!({
            "ResourceARNList": [UNOWNED, rows[0].1],
            "TagKeys": ["Env"]
        }),
    )
    .await;
    assert_eq!(status, 200, "UntagResources: body {body}");
    assert_eq!(
        body["FailedResourcesMap"][rows[0].1]["ErrorCode"],
        serde_json::json!(rows[0].2),
        "UntagResources must report the absent ARN with the owning service's code, got {body}"
    );
}

/// The SigV4 signing name of EventBridge, the service that owns an `events` ARN.
const EVENTS: &str = "events";

/// `arn:aws:events:<region>:<account>:rule/<bus-if-not-default>/<name>` — the ARN
/// form EventBridge mints for a rule, built here rather than read from that
/// crate.
fn events_rule_arn(bus: &str, rule: &str) -> String {
    let resource = if bus == "default" {
        format!("rule/{rule}")
    } else {
        format!("rule/{bus}/{rule}")
    };
    format!(
        "arn:aws:events:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>`, built here for the same
/// reason.
fn events_bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The tag map EventBridge's own `ListTagsForResource` reports for an ARN.
async fn events_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        EVENTS,
        "AWSEvents.ListTagsForResource",
        serde_json::json!({ "ResourceARN": arn }),
    )
    .await;
    assert_eq!(status, 200, "ListTagsForResource({arn}): body {body}");
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a ListTagsForResource body with no Tags: {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().expect("Tag.Key").to_string(),
                tag["Value"].as_str().expect("Tag.Value").to_string(),
            )
        })
        .collect()
}

/// EventBridge models the tag set of BOTH kinds it tags, so each has ONE tag
/// home: that service's own record. A tag written through this API is the
/// resource's own tag, which EventBridge's `ListTagsForResource` reports, and a
/// tag written through EventBridge is reported by `GetResources` — one set, read
/// and written from both sides, with nothing to keep in step.
///
/// The rows differ along the resource kind and, with it, the ARN form: an event
/// bus, a rule on the default bus, and a rule on a custom bus (whose ARN carries
/// that bus).
#[tokio::test]
async fn eventbridge_tags_live_in_eventbridges_own_records() {
    let router = emulator();
    let (status, body) = call(
        &router,
        EVENTS,
        "AWSEvents.CreateEventBus",
        serde_json::json!({ "Name": "shared-bus" }),
    )
    .await;
    assert_eq!(status, 200, "CreateEventBus: body {body}");
    for (rule, bus) in [("default-rule", None), ("bus-rule", Some("shared-bus"))] {
        let mut request = serde_json::json!({
            "Name": rule,
            "EventPattern": r#"{"source":["com.demo"]}"#,
        });
        if let Some(bus) = bus {
            request["EventBusName"] = serde_json::json!(bus);
        }
        let (status, body) = call(&router, EVENTS, "AWSEvents.PutRule", request).await;
        assert_eq!(status, 200, "PutRule {rule}: body {body}");
    }

    let rows: Vec<(&str, String)> = vec![
        ("an event bus", events_bus_arn("shared-bus")),
        (
            "a rule on the default bus",
            events_rule_arn("default", "default-rule"),
        ),
        (
            "a rule on a custom bus",
            events_rule_arn("shared-bus", "bus-rule"),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({ "ResourceARNList": [&arn], "Tags": { "Owner": "tagging-api" } }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResources body {body}");
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case {case}: tagging a live resource must not fail, got {body}"
        );

        // The round trip through the OWNING service's own reader.
        let native = events_tags(&router, &arn).await;
        assert_eq!(
            native,
            BTreeMap::from([("Owner".to_string(), "tagging-api".to_string())]),
            "case {case}: the tag written here must be the resource's own, got {native:?}"
        );

        // And the other direction: a tag written through EventBridge is part of
        // the set this API reports, because it is the same set.
        let (status, body) = call(
            &router,
            EVENTS,
            "AWSEvents.TagResource",
            serde_json::json!({
                "ResourceARN": &arn,
                "Tags": [{ "Key": "Team", "Value": "platform" }]
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResource body {body}");
        let (status, read) = tagging(
            &router,
            "GetResources",
            serde_json::json!({ "ResourceARNList": [&arn] }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: GetResources body {read}");
        let reported = page_tags(&read, &arn);
        assert_eq!(
            reported,
            BTreeMap::from([
                ("Owner".to_string(), "tagging-api".to_string()),
                ("Team".to_string(), "platform".to_string()),
            ]),
            "case {case}: both writes must be in the one tag set, got {reported:?}"
        );

        // A removal through this API removes exactly that key from the same set.
        let (status, body) = tagging(
            &router,
            "UntagResources",
            serde_json::json!({ "ResourceARNList": [&arn], "TagKeys": ["Owner"] }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: UntagResources body {body}");
        let native = events_tags(&router, &arn).await;
        assert_eq!(
            native,
            BTreeMap::from([("Team".to_string(), "platform".to_string())]),
            "case {case}: an untag here must remove the resource's own tag, got {native:?}"
        );
    }
}

/// A per-resource failure carries the hosting service's own error — its MESSAGE
/// as much as its code — so for an absent EventBridge rule it reads the way
/// EventBridge itself words a missing rule: the rule NAMED, and the event bus it
/// was looked up on named too, with no ARN echoed back. The same ARN put to
/// EventBridge's own tagging operation must answer that identical message, which
/// is what "the error returned by the service that hosts the resource" means.
///
/// The rows differ along the bus the ghost rule is sought on and the form of its
/// name, and with the bus the ARN form differs too (`rule/<name>` on the default
/// bus, `rule/<bus>/<name>` on any other).
#[tokio::test]
async fn an_absent_eventbridge_rule_fails_with_eventbridges_own_wording() {
    let router = emulator();

    // A live bus and a live rule, so a row can name a stored rule under a bus it
    // was never created on: that names a resource nothing created.
    let (status, body) = call(
        &router,
        EVENTS,
        "AWSEvents.CreateEventBus",
        serde_json::json!({ "Name": "orders-bus" }),
    )
    .await;
    assert_eq!(status, 200, "CreateEventBus: body {body}");
    let (status, body) = call(
        &router,
        EVENTS,
        "AWSEvents.PutRule",
        serde_json::json!({
            "Name": "live-rule",
            "EventPattern": r#"{"source":["com.demo"]}"#,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "PutRule: body {body}");

    // (case, the bus the ARN names, whether that bus EXISTS, the rule name)
    let rows: &[(&str, &str, bool, &str)] = &[
        ("the default bus", "default", true, "ghost-rule"),
        ("the default bus, a dotted name", "default", true, "ghost.rule.v2"),
        ("a custom bus that exists", "orders-bus", true, "ghost_rule"),
        ("a custom bus that does not exist", "ghost-bus", false, "ghost-rule"),
        (
            "a stored rule's name under another bus",
            "orders-bus",
            true,
            "live-rule",
        ),
    ];

    for (case, bus, bus_exists, rule) in rows {
        let ghost = events_rule_arn(bus, rule);
        // The wording this test holds the emulator to, composed here from the
        // row's own rule and bus. `EventBusName` names the SCOPE a rule is sought
        // in, so a row whose bus does not exist reads that the BUS is missing:
        // the hosting service resolves the bus first, and this API carries
        // whichever of the two refusals it composed.
        let expected = if *bus_exists {
            format!("Rule {rule} does not exist on EventBus {bus}.")
        } else {
            format!("Event bus {bus} does not exist.")
        };

        for (operation, request) in [
            (
                "TagResources",
                serde_json::json!({
                    "ResourceARNList": [UNOWNED, &ghost],
                    "Tags": { "Env": "partial" }
                }),
            ),
            (
                "UntagResources",
                serde_json::json!({
                    "ResourceARNList": [UNOWNED, &ghost],
                    "TagKeys": ["Env"]
                }),
            ),
        ] {
            let (status, body) = tagging(&router, operation, request).await;
            assert_eq!(
                status, 200,
                "case {case}/{operation}: expected a 200 with a partial failure, \
                 got {status} with {body}"
            );
            let failed: Vec<String> = body["FailedResourcesMap"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case}/{operation}: no FailedResourcesMap: {body}"))
                .keys()
                .cloned()
                .collect();
            assert_eq!(
                failed,
                vec![ghost.clone()],
                "case {case}/{operation}: expected exactly the absent ARN to fail, got {failed:?}"
            );
            let failure = &body["FailedResourcesMap"][&ghost];
            assert_eq!(
                failure["StatusCode"],
                serde_json::json!(400),
                "case {case}/{operation}: expected StatusCode 400, got {body}"
            );
            assert_eq!(
                failure["ErrorCode"],
                serde_json::json!("ResourceNotFoundException"),
                "case {case}/{operation}: expected EventBridge's own not-found code, got {body}"
            );
            assert_eq!(
                failure["ErrorMessage"],
                serde_json::json!(&expected),
                "case {case}/{operation}: expected {expected:?}, got {body}"
            );
            let reported = failure["ErrorMessage"].as_str().unwrap_or_default();
            assert!(
                !reported.contains("arn:"),
                "case {case}/{operation}: the failure must name the rule and its bus, \
                 not echo the ARN, got {reported:?}"
            );
        }

        // The same ARN, put to the hosting service's OWN tagging operation: the
        // per-resource failure above carries that service's message verbatim.
        let (status, native) = call(
            &router,
            EVENTS,
            "AWSEvents.TagResource",
            serde_json::json!({
                "ResourceARN": &ghost,
                "Tags": [{ "Key": "Env", "Value": "partial" }]
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: EventBridge's own TagResource must refuse the ghost ARN, \
             got {status} {native}"
        );
        assert_eq!(
            native["message"],
            serde_json::json!(&expected),
            "case {case}: the hosting service's own message must be the one the \
             per-resource failure carried, got {native}"
        );
    }

    // An `events` ARN that names no taggable EventBridge resource KIND at all is
    // not a missing resource but a request that service refuses, so the
    // per-resource failure carries THAT refusal — its code, its status and its
    // own message, which the hosting service is asked for here rather than
    // spelled out. The rows differ along the resource kind the ARN names.
    for (case, ghost) in [
        (
            "an archive",
            events_rule_arn("default", "x").replace(":rule/", ":archive/"),
        ),
        (
            "an api destination",
            events_rule_arn("default", "x").replace(":rule/", ":api-destination/"),
        ),
    ] {
        let (status, native) = call(
            &router,
            EVENTS,
            "AWSEvents.TagResource",
            serde_json::json!({
                "ResourceARN": &ghost,
                "Tags": [{ "Key": "Env", "Value": "partial" }]
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: EventBridge's own TagResource must refuse the ARN, got {status} {native}"
        );
        let refused = error_code(&native);
        let refusal = native["message"].as_str().unwrap_or_default().to_string();
        assert!(
            refusal.contains("ResourceARN"),
            "case {case}: that refusal must name the member it refused, got {refusal:?}"
        );

        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({
                "ResourceARNList": [UNOWNED, &ghost],
                "Tags": { "Env": "partial" }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected a 200 with a partial failure, got {status} with {body}"
        );
        let failure = &body["FailedResourcesMap"][&ghost];
        assert_eq!(
            (
                failure["StatusCode"].clone(),
                failure["ErrorCode"].clone(),
                failure["ErrorMessage"].clone()
            ),
            (
                serde_json::json!(400),
                serde_json::json!(refused),
                serde_json::json!(refusal)
            ),
            "case {case}: the per-resource failure must carry the hosting service's own \
             refusal ({refused} {refusal:?}), got {body}"
        );
    }
}

/// A tag written through this API is written to the OWNING service's own tag
/// set — so that service reports it — and the resource is discoverable by both
/// the bare and the qualified `ResourceTypeFilters` form.
#[tokio::test]
async fn a_tag_write_reaches_the_owning_services_own_tags_and_is_discoverable() {
    let router = emulator();
    let name = "fidelity-queue";
    let arn = owner_sqs::state::queue_arn(REGION, name);

    let (status, created) = call(
        &router,
        SQS,
        "AmazonSQS.CreateQueue",
        serde_json::json!({ "QueueName": name }),
    )
    .await;
    assert_eq!(status, 200, "CreateQueue: body {created}");
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue returned no QueueUrl: {created}"))
        .to_string();

    // A queue carries no tags yet, so the tagging API does not report it:
    // "GetResources does not return untagged resources."
    assert!(
        arns_of_type(&router, &["sqs"]).await.is_empty(),
        "an untagged queue must not be reported"
    );

    let (status, body) = tagging(
        &router,
        "TagResources",
        serde_json::json!({ "ResourceARNList": [&arn], "Tags": { "Env": "prod" } }),
    )
    .await;
    assert_eq!(status, 200, "TagResources: body {body}");
    assert_eq!(
        body["FailedResourcesMap"],
        serde_json::json!({}),
        "tagging a live queue must not fail, got {body}"
    );

    // The round trip through the OWNING service's own reader.
    let (status, native) = call(
        &router,
        SQS,
        "AmazonSQS.ListQueueTags",
        serde_json::json!({ "QueueUrl": &url }),
    )
    .await;
    assert_eq!(status, 200, "ListQueueTags: body {native}");
    assert_eq!(
        native["Tags"],
        serde_json::json!({ "Env": "prod" }),
        "the tag written through the tagging API must be the queue's own tag, got {native}"
    );

    // And the discovery query, in every filter form. Rows: the bare service
    // token, the qualified token, a union, the wrong type under the right
    // service, and the right type under the wrong service.
    let rows: &[(&[&str], bool)] = &[
        (&["sqs"], true),
        (&["sqs:queue"], true),
        (&["sqs:queue", "ecs:cluster"], true),
        (&["sqs:topic"], false),
        (&["sns:queue"], false),
        (&["sns:topic"], false),
    ];
    for (filters, selects) in rows {
        let got = arns_of_type(&router, filters).await;
        let want: Vec<String> = if *selects { vec![arn.clone()] } else { vec![] };
        assert_eq!(
            got, want,
            "case ResourceTypeFilters={filters:?}: expected {want:?}, got {got:?}"
        );
    }

    // Untagging goes to the same place.
    let (status, body) = tagging(
        &router,
        "UntagResources",
        serde_json::json!({ "ResourceARNList": [&arn], "TagKeys": ["Env"] }),
    )
    .await;
    assert_eq!(status, 200, "UntagResources: body {body}");
    let (_, native) = call(
        &router,
        SQS,
        "AmazonSQS.ListQueueTags",
        serde_json::json!({ "QueueUrl": &url }),
    )
    .await;
    assert!(
        native["Tags"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "the untag must have reached the queue's own tags, got {native}"
    );
    assert!(
        arns_of_type(&router, &["sqs:queue"]).await.is_empty(),
        "an untagged queue must not be reported any more"
    );

    // The documented 50-tag limit applies to the owning service's tag set too,
    // and a refused write leaves it exactly as it was.
    let keys: Vec<String> = (0..50).map(|n| format!("Key{n:02}")).collect();
    for chunk in keys.chunks(25) {
        let tags: serde_json::Map<String, serde_json::Value> = chunk
            .iter()
            .map(|key| (key.clone(), serde_json::json!("v")))
            .collect();
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({ "ResourceARNList": [&arn], "Tags": tags }),
        )
        .await;
        assert_eq!(status, 200, "case filling the limit: body {body}");
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case filling the limit: got {body}"
        );
    }
    let (status, body) = tagging(
        &router,
        "TagResources",
        serde_json::json!({ "ResourceARNList": [&arn], "Tags": { "OneTooMany": "v" } }),
    )
    .await;
    assert_eq!(status, 200, "case the 51st tag: body {body}");
    assert_eq!(
        body["FailedResourcesMap"][&arn]["ErrorCode"],
        serde_json::json!("InvalidParameterException"),
        "case the 51st tag: expected a per-resource refusal, got {body}"
    );
    let (_, native) = call(
        &router,
        SQS,
        "AmazonSQS.ListQueueTags",
        serde_json::json!({ "QueueUrl": &url }),
    )
    .await;
    let held = native["Tags"].as_object().expect("a tag map").len();
    assert_eq!(
        held, 50,
        "case the 51st tag: a refused write must leave the queue's own tags alone, got {held}"
    );
    assert!(
        native["Tags"]["OneTooMany"].is_null(),
        "case the 51st tag: the refused key reached the queue anyway: {native}"
    );
}

/// A resource tagged through its OWN service's API is reported by this one, and
/// a resource deleted there leaves the index — because the index IS the owning
/// services' state, not a copy of it.
#[tokio::test]
async fn tags_written_by_the_owning_service_are_visible_and_deletion_removes_them() {
    let router = emulator();

    // An ECS cluster, tagged at create time through ECS itself.
    let (status, cluster) = call(
        &router,
        ECS,
        "AmazonEC2ContainerServiceV20141113.CreateCluster",
        serde_json::json!({
            "clusterName": "fidelity-cluster",
            "tags": [{ "key": "Env", "value": "prod" }]
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateCluster: body {cluster}");
    let cluster_arn = cluster["cluster"]["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCluster returned no clusterArn: {cluster}"))
        .to_string();

    // A Secrets Manager secret, likewise.
    let (status, secret) = call(
        &router,
        SECRETS,
        "secretsmanager.CreateSecret",
        serde_json::json!({
            "Name": "fidelity-secret",
            "SecretString": "s3cr3t",
            // A create that stores a value creates that version, and Secrets
            // Manager requires a raw request to name it
            // (`CreateSecretRequest$ClientRequestToken`, that service's Smithy
            // model, `@length` 32..64).
            "ClientRequestToken": "client-request-token-fidelity-secret",
            "Tags": [{ "Key": "Env", "Value": "dev" }]
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateSecret: body {secret}");
    let secret_arn = secret["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSecret returned no ARN: {secret}"))
        .to_string();

    // Each is discoverable by its own qualified type, with the tags its own
    // service was given.
    let rows: &[(&str, &str, &str)] = &[
        ("ecs:cluster", &cluster_arn, "prod"),
        ("secretsmanager:secret", &secret_arn, "dev"),
    ];
    for (filter, arn, value) in rows {
        let (status, body) = tagging(
            &router,
            "GetResources",
            serde_json::json!({ "ResourceTypeFilters": [filter] }),
        )
        .await;
        assert_eq!(status, 200, "case {filter}: body {body}");
        assert_eq!(
            page_arns(&body),
            vec![(*arn).to_string()],
            "case {filter}: expected only {arn}, got {body}"
        );
        let tags = page_tags(&body, arn);
        assert_eq!(
            tags.get("Env").map(String::as_str),
            Some(*value),
            "case {filter}: expected Env={value}, got {tags:?}"
        );
    }

    // A tag query across both of them reports both, and their keys and values
    // are in the key/value projections.
    let (_, body) = tagging(
        &router,
        "GetResources",
        serde_json::json!({ "TagFilters": [{ "Key": "Env" }] }),
    )
    .await;
    let mut want = vec![cluster_arn.clone(), secret_arn.clone()];
    want.sort();
    assert_eq!(
        page_arns(&body),
        want,
        "a tag query must span every owning service, got {body}"
    );
    let (_, keys) = tagging(&router, "GetTagKeys", serde_json::json!({})).await;
    assert_eq!(
        keys["TagKeys"],
        serde_json::json!(["Env"]),
        "GetTagKeys must project the owning services' keys, got {keys}"
    );
    let (_, values) = tagging(
        &router,
        "GetTagValues",
        serde_json::json!({ "Key": "Env" }),
    )
    .await;
    assert_eq!(
        values["TagValues"],
        serde_json::json!(["dev", "prod"]),
        "GetTagValues must project the owning services' values, got {values}"
    );

    // Deleting the secret in its own service removes it from this API's answers.
    let (status, deleted) = call(
        &router,
        SECRETS,
        "secretsmanager.DeleteSecret",
        serde_json::json!({ "SecretId": &secret_arn, "ForceDeleteWithoutRecovery": true }),
    )
    .await;
    assert_eq!(status, 200, "DeleteSecret: body {deleted}");
    // The forced deletion is not instantaneous in the owning service either:
    // "Secrets Manager performs the actual deletion with an asynchronous
    // background process, so there might be a short delay before the secret is
    // permanently deleted." The index drops the ARN once the span that owner
    // recorded falls due, so this polls for it rather than assuming it has
    // already elapsed.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut still_indexed = arns_of_type(&router, &["secretsmanager:secret"]).await;
    while !still_indexed.is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
        still_indexed = arns_of_type(&router, &["secretsmanager:secret"]).await;
    }
    assert!(
        still_indexed.is_empty(),
        "case force-deleted secret: expected the index to drop {secret_arn} once its \
         asynchronous purge fell due, still got {still_indexed:?}"
    );
    assert_eq!(
        arns_of_type(&router, &["ecs:cluster"]).await,
        vec![cluster_arn],
        "the other service's resource must be untouched"
    );
}

/// A request-level parameter fault refuses the WHOLE call with a 400
/// `InvalidParameterException` and writes nothing — as distinct from a
/// well-formed ARN that names nothing, which is a per-resource failure.
#[tokio::test]
async fn a_request_level_fault_refuses_the_whole_call_and_writes_nothing() {
    let router = emulator();

    // Rows span the three faults, each along its own axis: a value that is not
    // an ARN (no `arn` scheme, too few fields, an empty required field), an ARN
    // in another Region, and an ARN in another partition.
    let bad_arns: &[(&str, &str)] = &[
        ("not an ARN at all", "sim2real-behav-not-an-arn"),
        ("too few fields", "arn:aws:s3:us-east-1"),
        ("no resource", "arn:aws:s3:::"),
        ("no partition", "arn::s3:::bucket-a"),
        ("no service", "arn:aws::::bucket-a"),
        ("another scheme", "urn:aws:s3:::bucket-a"),
        (
            "another Region",
            "arn:aws:sqs:us-west-2:000000000000:other-region-queue",
        ),
        (
            "another Region, same resource name",
            "arn:aws:ec2:eu-west-1:000000000000:instance/i-0abc",
        ),
        ("another partition", "arn:aws-cn:s3:::bucket-a"),
        ("the GovCloud partition", "arn:aws-us-gov:s3:::bucket-a"),
    ];
    for (case, bad) in bad_arns {
        for (operation, extra) in [
            ("TagResources", serde_json::json!({ "Env": "prod" })),
            ("UntagResources", serde_json::json!(["Env"])),
        ] {
            let mut request = serde_json::json!({ "ResourceARNList": [UNOWNED, bad] });
            request[if operation == "TagResources" {
                "Tags"
            } else {
                "TagKeys"
            }] = extra;
            let (status, body) = tagging(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {operation} with {case} ({bad:?}): expected HTTP 400, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterException",
                "case {operation} with {case} ({bad:?}): got {body}"
            );
        }
    }

    // A reserved `aws:` tag key is the same kind of fault, whatever its case,
    // suffix or encoding: "The caller isn't allowed to modify system tags."
    let reserved: &[&str] = &[
        "aws:cloudformation:stack-name",
        "aws:",
        "AWS:Service",
        "Aws:anything",
        // The prefix test is over bytes of untrusted text, so a key whose
        // characters continue past it in a multi-byte encoding is still in the
        // namespace.
        "aws:\u{e9}t\u{e9}",
        "AWS:\u{65e5}\u{672c}",
    ];
    for key in reserved {
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({ "ResourceARNList": [UNOWNED], "Tags": { (*key): "v" } }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case Tags={{{key}}}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case Tags={{{key}}}: got {body}"
        );
        let (status, body) = tagging(
            &router,
            "UntagResources",
            serde_json::json!({ "ResourceARNList": [UNOWNED], "TagKeys": [key] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case TagKeys=[{key}]: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case TagKeys=[{key}]: got {body}"
        );
    }

    // A key that is not in the namespace is tagged normally — including keys
    // whose 4th BYTE falls inside a multi-byte character, which a byte-sliced
    // prefix test would abort the request on rather than answer.
    let accepted: &[&str] = &[
        "awsome",
        "AWS",
        "aws\u{e9}",
        "aw\u{20ac}s:x",
        "\u{65e5}\u{672c}\u{8a9e}",
    ];
    for key in accepted {
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({ "ResourceARNList": [UNOWNED], "Tags": { (*key): "v" } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case Tags={{{key}}}: expected HTTP 200, got {status} with {body}"
        );
        let (status, body) = tagging(
            &router,
            "UntagResources",
            serde_json::json!({ "ResourceARNList": [UNOWNED], "TagKeys": [key] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case TagKeys=[{key}]: expected HTTP 200, got {status} with {body}"
        );
        // Written, then removed again, so the final tag map below is the one
        // the last accepted write left.
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case TagKeys=[{key}]: got {body}"
        );
    }

    // One last accepted write, so there is something to prove the refusals
    // above left the batch's other ARN untouched.
    let (status, body) = tagging(
        &router,
        "TagResources",
        serde_json::json!({ "ResourceARNList": [UNOWNED], "Tags": { "Env": "prod" } }),
    )
    .await;
    assert_eq!(status, 200, "case Tags={{Env}}: got {status} with {body}");

    // Every refused call wrote nothing: the batch is atomic, so the well-formed
    // ARN beside the bad value carries only the tag of the call that succeeded.
    let (_, read) = tagging(
        &router,
        "GetResources",
        serde_json::json!({ "ResourceARNList": [UNOWNED] }),
    )
    .await;
    let tags = page_tags(&read, UNOWNED);
    assert_eq!(
        tags,
        [("Env".to_string(), "prod".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        "a refused request must leave the other ARNs of its batch untouched, got {tags:?}"
    );

    // And a malformed value in `GetResources`' own `ResourceARNList` is refused
    // the same way.
    let (status, body) = tagging(
        &router,
        "GetResources",
        serde_json::json!({ "ResourceARNList": ["not-an-arn"] }),
    )
    .await;
    assert_eq!(status, 400, "GetResources with a malformed ARN: got {body}");
    assert_eq!(error_code(&body), "InvalidParameterException", "got {body}");
}

/// The tags ECS reports for one of its resources, read through ECS's OWN reader.
async fn ecs_tags(router: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        ECS,
        "AmazonEC2ContainerServiceV20141113.ListTagsForResource",
        serde_json::json!({ "resourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({resource_arn}): expected 200, got {status} with {body}"
    );
    body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a ListTagsForResource body with no tags list: {body}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().expect("Tag.key").to_string(),
                tag["value"].as_str().expect("Tag.value").to_string(),
            )
        })
        .collect()
}

/// A tag this API writes to a LIVE resource of an owning service is that
/// resource's own tag, so the owning service's own reader reports it — here ECS,
/// whose `ListTagsForResource` is the half of the partial-failure loop that says
/// whether the write that succeeded really landed.
///
/// The rows span two ECS resource KINDS, each beside a well-formed but absent
/// ARN of the same kind, so the per-resource failure the batch reports is proved
/// not to have cost the live resource its tag.
#[tokio::test]
async fn ecs_reports_the_tags_this_api_writes_through_its_own_reader() {
    let router = emulator();

    // A cluster and a task-definition revision, each created through ECS with a
    // tag of its own, so a tag this API never touched is beside the one it writes.
    let (status, cluster) = call(
        &router,
        ECS,
        "AmazonEC2ContainerServiceV20141113.CreateCluster",
        serde_json::json!({
            "clusterName": "propagation-cluster",
            "tags": [{ "key": "Owner", "value": "ecs" }]
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateCluster: body {cluster}");
    let cluster_arn = cluster["cluster"]["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCluster returned no clusterArn: {cluster}"))
        .to_string();

    let (status, definition) = call(
        &router,
        ECS,
        "AmazonEC2ContainerServiceV20141113.RegisterTaskDefinition",
        serde_json::json!({
            "family": "propagation-family",
            "containerDefinitions": [{
                "name": "app",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
            }],
            "tags": [{ "key": "Owner", "value": "ecs" }]
        }),
    )
    .await;
    assert_eq!(status, 200, "RegisterTaskDefinition: body {definition}");
    let definition_arn = definition["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("RegisterTaskDefinition returned no taskDefinitionArn: {definition}")
        })
        .to_string();

    // A third kind, the capacity provider, which `TagResource$resourceArn` names too. Its
    // Auto Scaling group is created first, through Auto Scaling's own API: ECS resolves the
    // reference and refuses a group the account does not hold.
    create_auto_scaling_group(&router, "asg-propagation").await;
    let (status, provider) = call(
        &router,
        ECS,
        "AmazonEC2ContainerServiceV20141113.CreateCapacityProvider",
        serde_json::json!({
            "name": "propagation-pool",
            "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-propagation" },
            "tags": [{ "key": "Owner", "value": "ecs" }]
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateCapacityProvider: body {provider}");
    let provider_arn = provider["capacityProvider"]["capacityProviderArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("CreateCapacityProvider returned no capacityProviderArn: {provider}")
        })
        .to_string();

    let rows: &[(&str, &str, &str)] = &[
        (
            "ecs cluster",
            &cluster_arn,
            "arn:aws:ecs:us-east-1:000000000000:cluster/propagation-ghost",
        ),
        (
            "ecs task definition",
            &definition_arn,
            "arn:aws:ecs:us-east-1:000000000000:task-definition/propagation-ghost:1",
        ),
        (
            "ecs capacity provider",
            &provider_arn,
            "arn:aws:ecs:us-east-1:000000000000:capacity-provider/propagation-ghost",
        ),
    ];

    for (case, live, ghost) in rows {
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({
                "ResourceARNList": [live, ghost],
                "Tags": { "Batch": "partial" }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected a 200 with a partial failure, got {status} with {body}"
        );
        let failed: Vec<String> = body["FailedResourcesMap"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: no FailedResourcesMap in {body}"))
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            failed,
            vec![(*ghost).to_string()],
            "case {case}: expected exactly the absent ARN to fail, got {failed:?}"
        );

        // The half the loop turns on: ECS's own reader reports the tag the batch
        // wrote, beside the tag ECS itself was created with.
        let held = ecs_tags(&router, live).await;
        assert_eq!(
            held,
            [
                ("Batch".to_string(), "partial".to_string()),
                ("Owner".to_string(), "ecs".to_string()),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
            "case {case}: expected ECS to report both tags for {live}, got {held:?}"
        );

        // And an untag through this API is gone from that same reader, while the
        // resource's own tag stays.
        let (status, body) = tagging(
            &router,
            "UntagResources",
            serde_json::json!({ "ResourceARNList": [live], "TagKeys": ["Batch"] }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: UntagResources body {body}");
        let held = ecs_tags(&router, live).await;
        assert_eq!(
            held,
            [("Owner".to_string(), "ecs".to_string())]
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            "case {case}: expected the untag to reach ECS's own tags of {live}, got {held:?}"
        );
    }

    // Each kind is in this API's own index too, under its qualified resource
    // type, since the index IS the owning service's records: a resource that
    // carries a tag is reported, whichever ECS kind it is.
    let indexed: &[(&str, &str)] = &[
        ("ecs:cluster", &cluster_arn),
        ("ecs:task-definition", &definition_arn),
        ("ecs:capacity-provider", &provider_arn),
    ];
    for (filter, arn) in indexed {
        let got = arns_of_type(&router, &[filter]).await;
        assert_eq!(
            got,
            vec![(*arn).to_string()],
            "case ResourceTypeFilters=[{filter}]: expected only {arn}, got {got:?}"
        );
    }
}

/// A page-size member outside its documented window is refused with the
/// service's FIXED range text for that member: the same message for every value
/// outside it, with the submitted value not echoed.
#[tokio::test]
async fn an_out_of_range_page_size_is_refused_with_the_members_fixed_range_text() {
    let router = emulator();

    // Sources: "You can specify a minimum of 1 and a maximum value of 100"
    // (`ResourcesPerPage`) and "a minimum of 100 items up to a maximum of 500
    // items" (`TagsPerPage`). Rows: each published bound, one past each bound,
    // and one far outside.
    let rows: &[(&str, i64, Option<&str>)] = &[
        ("ResourcesPerPage", 1, None),
        ("ResourcesPerPage", 100, None),
        (
            "ResourcesPerPage",
            0,
            Some("ResourcesPerPage is not in the valid range of 1 and 100"),
        ),
        (
            "ResourcesPerPage",
            101,
            Some("ResourcesPerPage is not in the valid range of 1 and 100"),
        ),
        (
            "ResourcesPerPage",
            -7,
            Some("ResourcesPerPage is not in the valid range of 1 and 100"),
        ),
        ("TagsPerPage", 100, None),
        ("TagsPerPage", 500, None),
        (
            "TagsPerPage",
            99,
            Some("TagsPerPage is not in the valid range of 100 and 500"),
        ),
        (
            "TagsPerPage",
            501,
            Some("TagsPerPage is not in the valid range of 100 and 500"),
        ),
    ];
    for (member, value, refusal) in rows {
        let mut request = serde_json::json!({});
        request[*member] = serde_json::json!(value);
        let (status, body) = tagging(&router, "GetResources", request).await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {member}={value}: expected HTTP 200, got {status} with {body}"
            ),
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {member}={value}: expected HTTP 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterException",
                    "case {member}={value}: got {body}"
                );
                assert_eq!(
                    body["message"].as_str().or(body["Message"].as_str()),
                    Some(*message),
                    "case {member}={value}: expected the fixed range text, got {body}"
                );
            }
        }
    }
}

/// The two token refusals are the two errors the service models for them: a
/// token this service never minted is `InvalidParameterException` (the call will
/// never succeed), while one it DID mint, past its documented 15-minute window,
/// is `PaginationTokenExpiredException` (restart the scan).
#[tokio::test]
async fn an_expired_token_and_an_unminted_token_get_different_errors() {
    let router = emulator();
    tagging(
        &router,
        "TagResources",
        serde_json::json!({ "ResourceARNList": [UNOWNED], "Tags": { "Env": "prod" } }),
    )
    .await;

    // `GetTagKeys` scopes its token to the operation and the serving Region, so
    // a token for this query can be minted here: one inside the window and one
    // past it. Source of the window: "A PaginationToken is valid for a maximum
    // of 15 minutes" (`PaginationTokenExpiredException` documentation).
    let scope = ["GetTagKeys", REGION];
    let fresh = tagging_state::encode_pagination_token_at(&scope, "Env", SystemTime::now());
    let aged = tagging_state::encode_pagination_token_at(
        &scope,
        "Env",
        SystemTime::now() - (tagging_state::PAGINATION_TOKEN_TTL + Duration::from_secs(60)),
    );

    // Rows: a token of this query inside its window, the same token past it, and
    // four strings this service never minted.
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("a fresh token of this query", &fresh, None),
        (
            "the same token past its window",
            &aged,
            Some("PaginationTokenExpiredException"),
        ),
        ("not a token at all", "bogus", Some("InvalidParameterException")),
        (
            "the right shape, a made-up digest",
            "deadbeefdeadbeef:1800000000:Env",
            Some("InvalidParameterException"),
        ),
        (
            "a token of another query",
            &tagging_state::encode_pagination_token_at(
                &["GetTagKeys", "eu-west-1"],
                "Env",
                SystemTime::now(),
            ),
            Some("InvalidParameterException"),
        ),
        (
            "no cursor",
            "0123456789abcdef:1800000000:",
            Some("InvalidParameterException"),
        ),
    ];
    for (case, token, refusal) in rows {
        let (status, body) = tagging(
            &router,
            "GetTagKeys",
            serde_json::json!({ "PaginationToken": token }),
        )
        .await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {body}"
            ),
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected HTTP 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    *expected,
                    "case {case}: got {body}"
                );
            }
        }
    }
}

/// The projection is scoped to the Region the request is signed for: "Returns
/// all the tagged or previously tagged resources that are located in the
/// specified … Region for the account" (`GetResources` documentation). An ARN
/// with an empty Region segment names a global resource, which every regional
/// endpoint reports.
#[tokio::test]
async fn the_reads_report_only_the_region_the_request_is_signed_for() {
    let router = emulator();
    // Both are in namespaces no owner crate resolves, so neither needs a live
    // resource behind it; they differ exactly along what the rule generalises
    // over — one carries a Region segment, one does not.
    let regional = "arn:aws:qldb:us-east-1:000000000000:ledger/in-region";
    let global = "arn:aws:iam::000000000000:user/fidelity-global";
    for (arn, scope) in [(regional, "regional"), (global, "global")] {
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({ "ResourceARNList": [arn], "Tags": { "Scope": scope } }),
        )
        .await;
        assert_eq!(status, 200, "TagResources({arn}): body {body}");
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "TagResources({arn}): got {body}"
        );
    }

    // (case, Region read in, ARNs reported, values of the shared key)
    let rows: [(&str, &str, Vec<&str>, Vec<&str>); 3] = [
        (
            "the Region both were written in",
            "us-east-1",
            vec![regional, global],
            vec!["global", "regional"],
        ),
        ("another Region", "us-west-2", vec![global], vec!["global"]),
        (
            "a third Region, so this is not a two-way special case",
            "eu-west-1",
            vec![global],
            vec!["global"],
        ),
    ];
    for (case, region, arns, values) in rows {
        let (status, body) =
            tagging_in_region(&router, region, "GetResources", serde_json::json!({})).await;
        assert_eq!(status, 200, "case {case}: body {body}");
        let mut want: Vec<String> = arns.iter().map(|arn| (*arn).to_string()).collect();
        want.sort();
        assert_eq!(
            page_arns(&body),
            want,
            "case {case}: expected {want:?}, got {body}"
        );

        let (status, body) = tagging_in_region(
            &router,
            region,
            "GetTagValues",
            serde_json::json!({ "Key": "Scope" }),
        )
        .await;
        assert_eq!(status, 200, "case {case} (GetTagValues): body {body}");
        assert_eq!(
            body["TagValues"],
            serde_json::json!(values),
            "case {case}: expected {values:?}, got {body}"
        );

        // The key is in use in every Region here, through the global resource,
        // so `GetTagKeys` reports it; the values above are what narrows.
        let (status, body) =
            tagging_in_region(&router, region, "GetTagKeys", serde_json::json!({})).await;
        assert_eq!(status, 200, "case {case} (GetTagKeys): body {body}");
        assert_eq!(
            body["TagKeys"],
            serde_json::json!(["Scope"]),
            "case {case}: got {body}"
        );
    }
}
