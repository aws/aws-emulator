// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's inference-deployment lifecycle:
//! model -> endpoint config -> endpoint -> inference component, plus `ListModels` and
//! the two deletes.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every resource kind, region and name shape; the create/describe round trip
//! for every optional member; the not-found refusal for every reader; and `ListModels`
//! for every documented sort, filter and page size.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::ENDPOINT_IN_SERVICE_AFTER` (300 ms).
pub(crate) const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// Spacing between creates, so `CreationTime` ordering is unambiguous and a filter
/// bound can be placed strictly between two models.
pub(crate) const CREATE_SPACING: Duration = Duration::from_millis(5);

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
///
/// Shared with this crate's other suites (`use super::inference_deployment_lifecycle::*`)
/// so every wire test drives the same router the production one assembles, rather than
/// each suite growing its own copy of the dispatch path.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The ONE shared engine the service is mounted on. A suite that exercises an
    /// operation whose writer is still a scaffold stub seeds that operation's records
    /// here — the same store the handler reads through `state::service_state`.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    pub(crate) async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the request routes by
    /// `X-Amz-Target` alone and the ARN region falls back to the emulator default.
    pub(crate) async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    pub(crate) async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    pub(crate) async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Fetch one URL this emulator published, the way a browser fetches it: a plain `GET`
    /// of the path and query the URL carries, nothing signed beyond what the URL itself
    /// holds, and no redirect followed.
    ///
    /// Returns `(status, body)`. Whatever host the URL names, the request is dispatched to
    /// this router — what is under test is the address the service published, not name
    /// resolution. Shared with the suite that follows a published URL
    /// (`tests/all/presigned_session.rs`).
    pub(crate) async fn fetch(&self, url: &str) -> (u16, String) {
        let target = match url.split_once("://") {
            Some((_, rest)) => match rest.split_once('/') {
                Some((_, path_and_query)) => format!("/{path_and_query}"),
                None => "/".to_string(),
            },
            None => url.to_string(),
        };
        let request = http::Request::builder()
            .method("GET")
            .uri(target)
            .body(bytes::Bytes::new())
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// A numeric member as `f64`, which is how the awsJson protocols put numbers and
/// timestamps on the wire. Named for the type it answers, because
/// `batch_transform_jobs` has its own `num_field` reading integer members.
pub(crate) fn f64_field(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric member {member} in {body}"))
}

/// An instant as the epoch-seconds number a wire timestamp carries, so a suite can
/// compare a reported timestamp against the instant the store holds.
pub(crate) fn epoch_seconds(at: std::time::SystemTime) -> f64 {
    let since_epoch = at
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch");
    since_epoch.as_secs_f64()
}

/// Materialise the lineage group `name` in `region` at `at` on `emulator`'s shared engine,
/// exactly as a lineage-entity create does, and answer the ARN when THIS call is what
/// materialised it.
///
/// Shared by the lineage-group suites (`lineage_groups`, `lineage_group_reporting`) so the
/// one write path is named once: it reaches the store directly because that is the only way
/// to put a group there with a chosen name, Region and instant — SageMaker models no
/// `CreateLineageGroup`, so `ensure_lineage_group` is the service's own write path.
pub(crate) fn materialise_lineage_group(
    emulator: &Emulator,
    region: &str,
    name: &str,
    at: std::time::SystemTime,
) -> Option<String> {
    let state = emulator.engine.state::<
        service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState,
    >();
    let mut store = state.lock();
    store.ensure_lineage_group(region, name, at)
}

pub(crate) fn primary_container(image: &str) -> Value {
    json!({ "Image": image })
}

/// `member` as a list of container definitions: a `PrimaryContainer` is one container, a
/// `Containers` member is already a list, and an absent member is no container at all.
fn container_list(member: Option<&Value>) -> Vec<Value> {
    match member {
        None => Vec::new(),
        Some(Value::Array(listed)) => listed.clone(),
        Some(single) => vec![single.clone()],
    }
}

/// A minimal one-variant configuration body naming `model`.
pub(crate) fn one_variant_config(name: &str, model: &str) -> Value {
    json!({
        "EndpointConfigName": name,
        "ProductionVariants": [{
            "VariantName": "AllTraffic",
            "ModelName": model,
            "InitialInstanceCount": 1,
            "InstanceType": "ml.m5.large",
        }],
    })
}

/// RULE: every ARN this service mints is
/// `arn:aws:sagemaker:<signed region>:<account>:<kind>/<name>`, for each of the three
/// resource kinds, whatever the name's shape and whatever region the caller signed for
/// (and the emulator default when the request is unsigned).
#[tokio::test]
async fn minted_arns_name_the_resource_kind_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: resource kind, region, and
    // the name's shape (short, digits, hyphens, and the 63-character modelled maximum).
    let names = [
        "m",
        "model1",
        "demo-model-with-hyphens",
        // `ModelName` / `EndpointConfigName` / `EndpointName` are all `len 0..63`
        // (smithy_query operation=CreateModel), so this row sits on the upper bound.
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for (index, name) in names.iter().enumerate() {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");

            let created = emulator
                .call_in(
                    region,
                    "CreateModel",
                    &json!({ "ModelName": name, "PrimaryContainer": primary_container("demo:1") }),
                )
                .await;
            assert_eq!(created.0, 200, "case {case}: CreateModel {:?}", created.1);
            assert_eq!(
                str_field(&created.1, "ModelArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model/{name}"),
                "case {case}: ModelArn, got {:?}",
                created.1
            );

            let config_name = format!("cfg{index}");
            let config = emulator
                .call_in(
                    region,
                    "CreateEndpointConfig",
                    &one_variant_config(&config_name, name),
                )
                .await;
            assert_eq!(config.0, 200, "case {case}: CreateEndpointConfig {:?}", config.1);
            assert_eq!(
                str_field(&config.1, "EndpointConfigArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint-config/{config_name}"),
                "case {case}: EndpointConfigArn, got {:?}",
                config.1
            );

            let endpoint_name = format!("ep{index}");
            let endpoint = emulator
                .call_in(
                    region,
                    "CreateEndpoint",
                    &json!({ "EndpointName": endpoint_name, "EndpointConfigName": config_name }),
                )
                .await;
            assert_eq!(endpoint.0, 200, "case {case}: CreateEndpoint {:?}", endpoint.1);
            assert_eq!(
                str_field(&endpoint.1, "EndpointArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint/{endpoint_name}"),
                "case {case}: EndpointArn, got {:?}",
                endpoint.1
            );
        }
    }

    // A credential scope whose region is not a region token cannot enter an ARN (the
    // region component is positional), so the emulator default stands in.
    for malformed in ["us east 1", "US-EAST-1", "us:east/1"] {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call_in(
                malformed,
                "CreateModel",
                &json!({ "ModelName": "odd-region", "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case malformed-region={malformed}: CreateModel, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "ModelArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/odd-region"),
            "case malformed-region={malformed}: ModelArn must fall back to the default \
             region, got {body:?}"
        );
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateModel",
            &json!({ "ModelName": "unsigned-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    assert_eq!(status, 200, "case unsigned: CreateModel, got {status} {body}");
    assert_eq!(
        str_field(&body, "ModelArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/unsigned-model"),
        "case unsigned: ModelArn, got {body:?}"
    );
}

/// RULE: `DescribeModel` replays every member `CreateModel` stored, present members and
/// absent ones alike, and the `CreationTime` it reports is the one recorded at create.
#[tokio::test]
async fn describe_model_replays_exactly_what_create_model_stored() {
    // Rows differ along container count, VPC configuration, network isolation, the
    // multi-container execution mode, the role, and tag count.
    let cases: Vec<(&str, Value)> = vec![
        (
            "primary-container-only",
            json!({ "PrimaryContainer": primary_container("demo:1") }),
        ),
        (
            "single-container-pipeline",
            json!({ "Containers": [primary_container("first:1")] }),
        ),
        (
            "three-container-pipeline",
            json!({
                "Containers": [
                    primary_container("first:1"),
                    { "Image": "second:2", "Environment": { "K": "V" } },
                    { "Image": "third:3", "ContainerHostname": "third" },
                ],
                "InferenceExecutionConfig": { "Mode": "Direct" },
            }),
        ),
        (
            "vpc-and-isolation",
            json!({
                "PrimaryContainer": primary_container("demo:1"),
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/SMRole",
                "VpcConfig": {
                    "SecurityGroupIds": ["sg-1", "sg-2"],
                    "Subnets": ["subnet-1"],
                },
                "EnableNetworkIsolation": true,
            }),
        ),
        (
            "isolation-explicitly-off-with-tags",
            json!({
                "PrimaryContainer": primary_container("demo:1"),
                "EnableNetworkIsolation": false,
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let mut body = json!({ "ModelName": case });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateModel", &body).await;
        let described = emulator
            .ok("DescribeModel", &json!({ "ModelName": case }))
            .await;

        assert_eq!(
            str_field(&described, "ModelName"),
            case,
            "case {case}: ModelName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelArn"),
            str_field(&created, "ModelArn"),
            "case {case}: DescribeModel must replay the ARN CreateModel minted"
        );
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be a stored timestamp, got {described:?}"
        );

        // Every member the request carried must come back unchanged; `Tags` is stored
        // but not part of DescribeModel's modelled output, so it is excluded here, and so
        // are the members the SERVICE supplies (each container's `Mode`,
        // `EnableNetworkIsolation`), which `service_resolved_members` pins.
        for member in ["InferenceExecutionConfig", "ExecutionRoleArn", "VpcConfig"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must round-trip, got {described:?}"
            );
        }
        for container in ["PrimaryContainer", "Containers"] {
            let sent = body.get(container);
            let reported = described.get(container);
            assert_eq!(
                sent.is_some(),
                reported.is_some(),
                "case {case}: {container} must round-trip, got {described:?}"
            );
            // Every member of a container the request carried is replayed; `Mode` is
            // reported beside them whether or not the request stated it.
            for (sent, reported) in container_list(sent).iter().zip(container_list(reported)) {
                for (member, value) in sent.as_object().expect("a container is an object") {
                    assert_eq!(
                        reported.get(member),
                        Some(value),
                        "case {case}: {container}.{member} must round-trip, got {reported:?}"
                    );
                }
                assert_eq!(
                    reported.get("Mode").and_then(Value::as_str),
                    Some("SingleModel"),
                    "case {case}: {container}.Mode, got {reported:?}"
                );
            }
        }
        assert_eq!(
            described.get("EnableNetworkIsolation").and_then(Value::as_bool),
            Some(
                body.get("EnableNetworkIsolation")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            ),
            "case {case}: EnableNetworkIsolation, got {described:?}"
        );

        // A second read sees the same values: a read changes nothing.
        let again = emulator
            .ok("DescribeModel", &json!({ "ModelName": case }))
            .await;
        assert_eq!(
            again, described,
            "case {case}: a second DescribeModel must answer identically"
        );
    }
}

/// RULE: `DescribeEndpointConfig` replays every member `CreateEndpointConfig` stored,
/// for one variant and for the modelled maximum of ten, instance-backed or serverless.
#[tokio::test]
async fn describe_endpoint_config_replays_exactly_what_create_stored() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "shared-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;

    // `ProductionVariants` is `len 1..10` (smithy_query operation=CreateEndpointConfig),
    // so the rows sit on both published bounds plus a serverless variant in between.
    let ten_variants: Vec<Value> = (0..10)
        .map(|index| {
            json!({
                "VariantName": format!("v{index}"),
                "ModelName": "shared-model",
                "InitialInstanceCount": index + 1,
                "InstanceType": "ml.m5.large",
                "InitialVariantWeight": 0.5,
            })
        })
        .collect();

    let cases: Vec<(&str, Value)> = vec![
        (
            "one-variant",
            json!({
                "ProductionVariants": [{
                    "VariantName": "AllTraffic",
                    "ModelName": "shared-model",
                    "InitialInstanceCount": 2,
                    "InstanceType": "ml.c5.xlarge",
                }],
            }),
        ),
        (
            "ten-variants-with-kms-and-capture",
            json!({
                "ProductionVariants": ten_variants,
                "KmsKeyId": "alias/sagemaker",
                "DataCaptureConfig": {
                    "EnableCapture": true,
                    "InitialSamplingPercentage": 25,
                    "DestinationS3Uri": "s3://bucket/capture",
                    "CaptureOptions": [{ "CaptureMode": "Input" }],
                },
            }),
        ),
        (
            "serverless-variant",
            json!({
                "ProductionVariants": [{
                    "VariantName": "Serverless",
                    "ModelName": "shared-model",
                    "ServerlessConfig": { "MemorySizeInMB": 2048, "MaxConcurrency": 5 },
                }],
                "EnableNetworkIsolation": true,
            }),
        ),
    ];

    for (case, extra) in cases {
        let mut body = json!({ "EndpointConfigName": case });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateEndpointConfig", &body).await;
        let described = emulator
            .ok(
                "DescribeEndpointConfig",
                &json!({ "EndpointConfigName": case }),
            )
            .await;

        assert_eq!(
            str_field(&described, "EndpointConfigArn"),
            str_field(&created, "EndpointConfigArn"),
            "case {case}: DescribeEndpointConfig must replay the ARN create minted"
        );
        for member in ["ProductionVariants", "KmsKeyId", "DataCaptureConfig"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must round-trip, got {described:?}"
            );
        }
        // `EnableNetworkIsolation` is resolved by the SERVICE: the submitted value where
        // the request named one, and no isolation where it named none — so the read
        // reports what the endpoint would run under rather than only what was sent
        // (`service_resolved_members` and `read_responses_report_what_a_resource_holds`
        // pin that rule).
        assert_eq!(
            described.get("EnableNetworkIsolation"),
            Some(body.get("EnableNetworkIsolation").unwrap_or(&json!(false))),
            "case {case}: EnableNetworkIsolation must report the effective value, got \
             {described:?}"
        );
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be a stored timestamp, got {described:?}"
        );
    }
}

/// RULE: a reader or a delete naming a resource that is not in state is refused with
/// HTTP 400 `ValidationException`, quoting the IDENTIFIER THE REQUEST CARRIED — never a
/// 200 with empty members, never a 501, and never an ARN the service synthesised for a
/// name the caller never sent.
#[tokio::test]
async fn an_absent_resource_is_refused_with_a_400_validation_exception() {
    let emulator = Emulator::new();

    // Rows: every implemented operation that resolves a name, each with a name nothing
    // created, so the rule is asserted over the whole class rather than one reader. The
    // third column is the kind label the service words that resource by.
    let cases: Vec<(&str, Value, &str)> = vec![
        ("DescribeModel", json!({ "ModelName": "absent" }), "model"),
        ("DeleteModel", json!({ "ModelName": "absent" }), "model"),
        (
            "DescribeEndpointConfig",
            json!({ "EndpointConfigName": "absent" }),
            "endpoint configuration",
        ),
        (
            "DescribeEndpoint",
            json!({ "EndpointName": "absent" }),
            "endpoint",
        ),
        (
            "DeleteEndpoint",
            json!({ "EndpointName": "absent" }),
            "endpoint",
        ),
        (
            "DeleteEndpointConfig",
            json!({ "EndpointConfigName": "absent" }),
            "endpoint configuration",
        ),
        (
            "CreateEndpoint",
            json!({ "EndpointName": "fresh", "EndpointConfigName": "absent" }),
            "endpoint configuration",
        ),
        (
            "CreateEndpointConfig",
            one_variant_config("fresh-cfg", "absent"),
            "model",
        ),
    ];

    for (target, body, kind) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {target}: expected HTTP 400 for an absent resource, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {target}: error code, got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            format!("Could not find {kind} \"absent\"."),
            "case {target}: the refusal must quote the {kind} identifier the request \
             carried, got {parsed:?}"
        );
    }
}

/// RULE: a create whose name is already in use is refused, so the stored resource is
/// never silently overwritten.
#[tokio::test]
async fn creating_a_name_that_is_already_in_use_is_refused_and_keeps_the_stored_resource() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "dup", "PrimaryContainer": primary_container("first:1") }),
        )
        .await;
    emulator
        .ok("CreateEndpointConfig", &one_variant_config("dup-cfg", "dup"))
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "dup-ep", "EndpointConfigName": "dup-cfg" }),
        )
        .await;

    // Rows: each of the three create operations, re-run with the same name but a
    // DIFFERENT body, so a silent overwrite would be visible in the follow-up read.
    let cases: Vec<(&str, Value, &str, Value, &str)> = vec![
        (
            "CreateModel",
            json!({ "ModelName": "dup", "PrimaryContainer": primary_container("second:2") }),
            "DescribeModel",
            json!({ "ModelName": "dup" }),
            "model/dup",
        ),
        (
            "CreateEndpointConfig",
            json!({
                "EndpointConfigName": "dup-cfg",
                "ProductionVariants": [{
                    "VariantName": "Replacement",
                    "ModelName": "dup",
                    "InitialInstanceCount": 9,
                    "InstanceType": "ml.m5.24xlarge",
                }],
            }),
            "DescribeEndpointConfig",
            json!({ "EndpointConfigName": "dup-cfg" }),
            "endpoint-config/dup-cfg",
        ),
        (
            "CreateEndpoint",
            json!({ "EndpointName": "dup-ep", "EndpointConfigName": "dup-cfg" }),
            "DescribeEndpoint",
            json!({ "EndpointName": "dup-ep" }),
            "endpoint/dup-ep",
        ),
    ];

    for (target, body, reader, read_body, arn_tail) in cases {
        let before = emulator.ok(reader, &read_body).await;
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {target}: a name already in use must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {target}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(arn_tail),
            "case {target}: the refusal must name the existing resource, got {parsed:?}"
        );
        let after = emulator.ok(reader, &read_body).await;
        assert_eq!(
            after, before,
            "case {target}: the refused create must leave the stored resource untouched"
        );
    }
}

/// RULE: an endpoint reports the variant summary its configuration's variants imply —
/// `InitialVariantWeight` (defaulting to 1.0, per the member's model doc) as current and
/// desired weight, `InitialInstanceCount` as current and desired count, and a serverless
/// variant's config as the current and desired serverless config.
#[tokio::test]
async fn endpoint_production_variants_are_derived_from_the_stored_configuration() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "pv-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;

    // Rows differ along: weight given vs defaulted, instance count, and instance-backed
    // vs serverless.
    let variants = json!([
        {
            "VariantName": "weighted",
            "ModelName": "pv-model",
            "InitialInstanceCount": 3,
            "InstanceType": "ml.m5.large",
            "InitialVariantWeight": 0.25,
        },
        {
            "VariantName": "defaulted-weight",
            "ModelName": "pv-model",
            "InitialInstanceCount": 1,
            "InstanceType": "ml.m5.large",
        },
        {
            "VariantName": "serverless",
            "ModelName": "pv-model",
            "ServerlessConfig": { "MemorySizeInMB": 1024, "MaxConcurrency": 2 },
        },
    ]);

    emulator
        .ok(
            "CreateEndpointConfig",
            &json!({
                "EndpointConfigName": "pv-cfg",
                "ProductionVariants": variants,
                "ShadowProductionVariants": [{
                    "VariantName": "shadow",
                    "ModelName": "pv-model",
                    "InitialInstanceCount": 2,
                    "InstanceType": "ml.m5.large",
                    "InitialVariantWeight": 0.1,
                }],
            }),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "pv-ep", "EndpointConfigName": "pv-cfg" }),
        )
        .await;

    let described = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "pv-ep" }))
        .await;
    let summaries = described
        .get("ProductionVariants")
        .and_then(Value::as_array)
        .expect("DescribeEndpoint must report ProductionVariants");

    let expected: Vec<(&str, f64, Option<i64>, Option<i64>)> = vec![
        ("weighted", 0.25, Some(3), None),
        ("defaulted-weight", 1.0, Some(1), None),
        ("serverless", 1.0, None, Some(1024)),
    ];
    assert_eq!(
        summaries.len(),
        expected.len(),
        "one summary per configured variant, got {summaries:?}"
    );

    for (index, (name, weight, count, memory)) in expected.into_iter().enumerate() {
        let summary = &summaries[index];
        let case = format!("variant {name}");
        assert_eq!(
            str_field(summary, "VariantName"),
            name,
            "case {case}: VariantName, got {summary:?}"
        );
        for member in ["CurrentWeight", "DesiredWeight"] {
            let actual = summary.get(member).and_then(Value::as_f64);
            assert_eq!(
                actual,
                Some(weight),
                "case {case}: {member}, got {summary:?}"
            );
        }
        for member in ["CurrentInstanceCount", "DesiredInstanceCount"] {
            assert_eq!(
                summary.get(member).and_then(Value::as_i64),
                count,
                "case {case}: {member}, got {summary:?}"
            );
        }
        for member in ["CurrentServerlessConfig", "DesiredServerlessConfig"] {
            assert_eq!(
                summary
                    .get(member)
                    .and_then(|config| config.get("MemorySizeInMB"))
                    .and_then(Value::as_i64),
                memory,
                "case {case}: {member}, got {summary:?}"
            );
        }
    }

    // The configuration's shadow variants are summarised the same way, and an endpoint
    // whose configuration carried none omits the member entirely.
    let shadow = described
        .get("ShadowProductionVariants")
        .and_then(Value::as_array)
        .expect("DescribeEndpoint must report the configured ShadowProductionVariants");
    assert_eq!(
        shadow.len(),
        1,
        "case shadow-variants: one summary per configured shadow variant, got {shadow:?}"
    );
    assert_eq!(
        str_field(&shadow[0], "VariantName"),
        "shadow",
        "case shadow-variants: VariantName, got {shadow:?}"
    );
    assert_eq!(
        shadow[0].get("DesiredInstanceCount").and_then(Value::as_i64),
        Some(2),
        "case shadow-variants: DesiredInstanceCount, got {shadow:?}"
    );

    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("no-shadow-cfg", "pv-model"),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "no-shadow-ep", "EndpointConfigName": "no-shadow-cfg" }),
        )
        .await;
    let without_shadow = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "no-shadow-ep" }))
        .await;
    assert!(
        without_shadow.get("ShadowProductionVariants").is_none(),
        "case no-shadow-variants: the member must be omitted, got {without_shadow:?}"
    );
}

/// RULE: a newly created endpoint is reported `Creating`, and the `InService` transition
/// the clock owes becomes visible once its span has elapsed — without any call having to
/// make it happen, and stably thereafter.
#[tokio::test]
async fn an_endpoint_advances_from_creating_to_in_service_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "wait-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("wait-cfg", "wait-model"),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "wait-ep", "EndpointConfigName": "wait-cfg" }),
        )
        .await;

    let creating = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "wait-ep" }))
        .await;
    assert_eq!(
        str_field(&creating, "EndpointStatus"),
        "Creating",
        "case immediately-after-create: EndpointStatus, got {creating:?}"
    );
    assert_eq!(
        str_field(&creating, "EndpointConfigName"),
        "wait-cfg",
        "case immediately-after-create: EndpointConfigName, got {creating:?}"
    );

    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    let settled = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "wait-ep" }))
        .await;
    assert_eq!(
        str_field(&settled, "EndpointStatus"),
        "InService",
        "case after-the-due-span: EndpointStatus, got {settled:?}"
    );
    let created_at = settled
        .get("CreationTime")
        .and_then(Value::as_f64)
        .expect("CreationTime");
    let modified_at = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .expect("LastModifiedTime");
    assert!(
        modified_at > created_at,
        "case after-the-due-span: LastModifiedTime {modified_at} must move past \
         CreationTime {created_at} when the status changes"
    );
    assert_eq!(
        settled.get("CreationTime"),
        creating.get("CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {settled:?}"
    );

    let again = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "wait-ep" }))
        .await;
    assert_eq!(
        again, settled,
        "case repeated-read: a settled endpoint must read back identically"
    );
}

/// RULE: a delete removes the entry, so the reader refuses it afterwards and `ListModels`
/// stops listing it, while every sibling resource is untouched.
#[tokio::test]
async fn deleting_a_resource_removes_only_that_entry() {
    let emulator = Emulator::new();
    for name in ["keep-model", "drop-model"] {
        emulator
            .ok(
                "CreateModel",
                &json!({ "ModelName": name, "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
    }
    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("del-cfg", "keep-model"),
        )
        .await;
    for name in ["keep-ep", "drop-ep"] {
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": name, "EndpointConfigName": "del-cfg" }),
            )
            .await;
    }

    // Rows: both deletes, each with the sibling that must survive it.
    let cases: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            "DeleteEndpoint",
            json!({ "EndpointName": "drop-ep" }),
            "DescribeEndpoint",
            json!({ "EndpointName": "drop-ep" }),
            json!({ "EndpointName": "keep-ep" }),
        ),
        (
            "DeleteModel",
            json!({ "ModelName": "drop-model" }),
            "DescribeModel",
            json!({ "ModelName": "drop-model" }),
            json!({ "ModelName": "keep-model" }),
        ),
    ];

    for (target, body, reader, gone_body, survivor_body) in cases {
        let deleted = emulator.ok(target, &body).await;
        assert_eq!(
            deleted,
            json!({}),
            "case {target}: the modelled output has no members, got {deleted:?}"
        );

        let (status, parsed) = emulator.call(reader, &gone_body).await;
        assert_eq!(
            status, 400,
            "case {target}: the deleted resource must be gone, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {target}: error code after delete, got {parsed:?}"
        );

        let survivor = emulator.ok(reader, &survivor_body).await;
        assert!(
            survivor.get("CreationTime").is_some(),
            "case {target}: the sibling resource must survive, got {survivor:?}"
        );

        // A second delete of the same name is refused: the entry is really gone.
        let (repeat_status, repeat) = emulator.call(target, &body).await;
        assert_eq!(
            repeat_status, 400,
            "case {target}: deleting twice must be refused, got {repeat_status} {repeat}"
        );
    }

    let listed = emulator.ok("ListModels", &json!({})).await;
    let names: Vec<String> = listed
        .get("Models")
        .and_then(Value::as_array)
        .expect("Models")
        .iter()
        .map(|model| str_field(model, "ModelName"))
        .collect();
    assert_eq!(
        names,
        vec!["keep-model".to_string()],
        "a deleted model must not be listed, got {listed:?}"
    );
}

/// Create four models, spaced so `CreationTime` ordering is unambiguous.
async fn seed_models(emulator: &Emulator) -> Vec<(String, f64)> {
    let mut seeded = Vec::new();
    for name in ["alpha-one", "beta-two", "demo-three", "demo-four"] {
        emulator
            .ok(
                "CreateModel",
                &json!({ "ModelName": name, "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
        let described = emulator
            .ok("DescribeModel", &json!({ "ModelName": name }))
            .await;
        seeded.push((
            name.to_string(),
            described
                .get("CreationTime")
                .and_then(Value::as_f64)
                .expect("CreationTime"),
        ));
        tokio::time::sleep(CREATE_SPACING).await;
    }
    seeded
}

fn listed_names(body: &Value) -> Vec<String> {
    body.get("Models")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Models list in {body}"))
        .iter()
        .map(|model| str_field(model, "ModelName"))
        .collect()
}

/// RULE: `ListModels` returns the stored models filtered by `NameContains`, ordered by
/// `SortBy`/`SortOrder` (defaulting to `CreationTime` / `Descending`, as both members'
/// model docs state), and each entry carries the stored name, ARN and creation time.
#[tokio::test]
async fn list_models_honours_every_documented_sort_and_filter() {
    let emulator = Emulator::new();
    let seeded = seed_models(&emulator).await;

    // Rows differ along sort field, sort order, and the filter's selectivity.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "documented-defaults",
            json!({}),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            vec!["demo-three", "demo-four", "beta-two", "alpha-one"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-contains-two-matches",
            json!({ "NameContains": "demo", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["demo-four", "demo-three"],
        ),
        (
            "name-contains-one-match",
            json!({ "NameContains": "alpha", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "nothing-here" }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListModels", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListModels order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // Each summary replays the stored name, ARN and creation time.
    let listed = emulator
        .ok(
            "ListModels",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let models = listed.get("Models").and_then(Value::as_array).expect("Models");
    for (index, (name, created_at)) in seeded.iter().enumerate() {
        let summary = &models[index];
        assert_eq!(
            str_field(summary, "ModelArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model/{name}"),
            "case {name}: ModelArn in the summary, got {summary:?}"
        );
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_f64),
            Some(*created_at),
            "case {name}: CreationTime in the summary must be the stored one"
        );
    }
}

/// RULE: a `CreationTimeBefore` / `CreationTimeAfter` bound selects exactly the models on
/// that side of it.
#[tokio::test]
async fn list_models_filters_on_the_creation_time_bounds() {
    let emulator = Emulator::new();
    let seeded = seed_models(&emulator).await;
    // A bound strictly between the second and third model, so the split is unambiguous.
    let split = (seeded[1].1 + seeded[2].1) / 2.0;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "before-the-split",
            json!({ "CreationTimeBefore": split, "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "after-the-split",
            json!({ "CreationTimeAfter": split, "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "both-bounds-around-one-model",
            json!({
                "CreationTimeAfter": split,
                "CreationTimeBefore": (seeded[2].1 + seeded[3].1) / 2.0,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["demo-three"],
        ),
        (
            "bounds-that-exclude-everything",
            json!({ "CreationTimeAfter": seeded[3].1 + 3600.0 }),
            vec![],
        ),
        (
            "bound-combined-with-a-name-filter",
            json!({ "CreationTimeAfter": split, "NameContains": "demo", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["demo-four", "demo-three"],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListModels", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListModels selection, got {listed:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered result exactly once — each page
/// holds at most `MaxResults` entries, a `NextToken` is present only while more remain,
/// and feeding it back resumes immediately after the previous page.
#[tokio::test]
async fn paging_with_max_results_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    seed_models(&emulator).await;
    let ordered = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];

    // Rows: every page size from one entry up to past the end of the result set.
    // `MaxResults` is `range 1..100` (smithy_query operation=ListModels), so 1 and 100
    // are the published bounds.
    for page_size in [1_i64, 2, 3, 4, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut body = json!({
                "SortBy": "Name",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListModels", &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() as i64 <= page_size,
                "case {case}: a page must hold at most {page_size} entries, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= ordered.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, ordered,
            "case {case}: paging must yield the whole ordered result exactly once"
        );
        let expected_pages = (ordered.len() as i64 + page_size - 1) / page_size;
        assert_eq!(
            pages as i64, expected_pages,
            "case {case}: page count, saw {seen:?}"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the listing.
    let (status, parsed) = emulator
        .call("ListModels", &json!({ "NextToken": "not-a-token" }))
        .await;
    assert_eq!(
        status, 400,
        "case bogus-NextToken: expected HTTP 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case bogus-NextToken: error code, got {parsed:?}"
    );
}

/// RULE: the page is capped server-side, so no single `ListModels` call serialises the
/// whole store however large `MaxResults` is or whether the caller sends it at all — and
/// the resume token still walks the full set.
#[tokio::test]
async fn the_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    // `state::MAX_PAGE_SIZE` is this service's own ceiling; the store is seeded past it
    // so a capped page and an uncapped one are distinguishable.
    const CEILING: usize = 50;
    const STORED: usize = CEILING + 7;

    let emulator = Emulator::new();
    for index in 0..STORED {
        emulator
            .ok(
                "CreateModel",
                &json!({
                    "ModelName": format!("m{index:03}"),
                    "PrimaryContainer": primary_container("demo:1"),
                }),
            )
            .await;
    }

    // Rows: no `MaxResults` at all, the modelled maximum (above the ceiling), and a
    // value below the ceiling, which still wins because it is the smaller cap.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("max-results-absent", json!({}), CEILING),
        ("max-results-at-the-modelled-maximum", json!({ "MaxResults": 100 }), CEILING),
        ("max-results-below-the-ceiling", json!({ "MaxResults": 10 }), 10),
    ];

    for (case, extra, expected_page) in cases {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        let listed = emulator.ok("ListModels", &body).await;
        assert_eq!(
            listed_names(&listed).len(),
            expected_page,
            "case {case}: page size, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_some(),
            "case {case}: more models remain, so a NextToken is required, got {listed:?}"
        );
    }

    // And the capped pages still walk the whole store exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListModels", &body).await;
        let names = listed_names(&listed);
        assert!(
            names.len() <= CEILING,
            "case capped-walk: a page must never exceed the ceiling, got {} entries",
            names.len()
        );
        seen.extend(names);
        assert!(
            seen.len() <= STORED,
            "case capped-walk: paging did not terminate, saw {} entries",
            seen.len()
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    let expected: Vec<String> = (0..STORED).map(|index| format!("m{index:03}")).collect();
    assert_eq!(
        seen, expected,
        "case capped-walk: the capped pages must cover the whole store exactly once"
    );
}

/// RULE: an endpoint's `DataCaptureConfig` summary is derived from the configuration's
/// stored capture settings — `CaptureStatus` follows `EnableCapture` (which the model doc
/// says "defaults to enabled"), the sampling percentage and destination are replayed, and
/// an absent `KmsKeyId` reports the empty string the required summary member allows.
#[tokio::test]
async fn the_endpoint_data_capture_summary_is_derived_from_the_stored_capture_config() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "dc-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;

    // Rows differ along: capture enabled explicitly, disabled explicitly, left to the
    // documented default, and with vs without a KMS key.
    let cases: Vec<(&str, Value, bool, &str, i64, &str)> = vec![
        (
            "enabled-with-kms",
            json!({
                "EnableCapture": true,
                "InitialSamplingPercentage": 25,
                "DestinationS3Uri": "s3://bucket/one",
                "CaptureOptions": [{ "CaptureMode": "Input" }],
                "KmsKeyId": "alias/capture",
            }),
            true,
            "Started",
            25,
            "alias/capture",
        ),
        (
            "disabled-without-kms",
            json!({
                "EnableCapture": false,
                "InitialSamplingPercentage": 10,
                "DestinationS3Uri": "s3://bucket/two",
                "CaptureOptions": [{ "CaptureMode": "Output" }],
            }),
            false,
            "Stopped",
            10,
            "",
        ),
        (
            "enable-capture-left-to-the-documented-default",
            json!({
                "InitialSamplingPercentage": 100,
                "DestinationS3Uri": "s3://bucket/three",
                "CaptureOptions": [
                    { "CaptureMode": "Input" },
                    { "CaptureMode": "Output" },
                ],
            }),
            true,
            "Started",
            100,
            "",
        ),
    ];

    for (index, (case, capture, enabled, status, sampling, key)) in cases.into_iter().enumerate() {
        let config_name = format!("dc-cfg{index}");
        let endpoint_name = format!("dc-ep{index}");
        let mut config = one_variant_config(&config_name, "dc-model");
        config["DataCaptureConfig"] = capture;
        emulator.ok("CreateEndpointConfig", &config).await;
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": endpoint_name, "EndpointConfigName": config_name }),
            )
            .await;

        let described = emulator
            .ok("DescribeEndpoint", &json!({ "EndpointName": endpoint_name }))
            .await;
        let summary = described
            .get("DataCaptureConfig")
            .unwrap_or_else(|| panic!("case {case}: DescribeEndpoint must report the summary"));
        assert_eq!(
            summary.get("EnableCapture").and_then(Value::as_bool),
            Some(enabled),
            "case {case}: EnableCapture, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "CaptureStatus"),
            status,
            "case {case}: CaptureStatus, got {summary:?}"
        );
        assert_eq!(
            summary
                .get("CurrentSamplingPercentage")
                .and_then(Value::as_i64),
            Some(sampling),
            "case {case}: CurrentSamplingPercentage, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "KmsKeyId"),
            key,
            "case {case}: KmsKeyId, got {summary:?}"
        );
    }

    // An endpoint whose configuration carried no capture settings omits the member.
    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("dc-none-cfg", "dc-model"),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "dc-none-ep", "EndpointConfigName": "dc-none-cfg" }),
        )
        .await;
    let described = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "dc-none-ep" }))
        .await;
    assert!(
        described.get("DataCaptureConfig").is_none(),
        "case no-capture-config: the member must be omitted, got {described:?}"
    );
}

/// RULE: each store holds at most its quota of records; the create past it is refused
/// with the modelled `ResourceLimitExceeded` and stores nothing, so no caller can grow
/// the emulator's state without end by varying the resource name.
#[tokio::test]
async fn a_create_past_a_store_quota_is_refused_with_the_modelled_limit_failure() {
    // `state::MODEL_QUOTA` / `ENDPOINT_CONFIG_QUOTA` / `ENDPOINT_QUOTA`, all 1_000.
    const QUOTA: usize = 1_000;

    // Rows: all three bounded stores, each filled to its quota and then pushed one past
    // it, with the reader that proves the refused record was not stored.
    for store in ["models", "endpoint-configs", "endpoints"] {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateModel",
                &json!({ "ModelName": "quota-model", "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
        emulator
            .ok(
                "CreateEndpointConfig",
                &one_variant_config("quota-cfg", "quota-model"),
            )
            .await;

        // Fill the store under test up to its quota (one record of it already exists).
        let (target, name_member, reader, fill_body): (&str, &str, &str, fn(&str) -> Value) =
            match store {
                "models" => (
                    "CreateModel",
                    "ModelName",
                    "DescribeModel",
                    |name| json!({ "ModelName": name, "PrimaryContainer": { "Image": "demo:1" } }),
                ),
                "endpoint-configs" => (
                    "CreateEndpointConfig",
                    "EndpointConfigName",
                    "DescribeEndpointConfig",
                    |name| one_variant_config(name, "quota-model"),
                ),
                _ => (
                    "CreateEndpoint",
                    "EndpointName",
                    "DescribeEndpoint",
                    |name| json!({ "EndpointName": name, "EndpointConfigName": "quota-cfg" }),
                ),
            };
        let already_stored = usize::from(store != "endpoints");
        for index in already_stored..QUOTA {
            emulator.ok(target, &fill_body(&format!("q{index:04}"))).await;
        }

        let overflow = format!("q{QUOTA:04}");
        let (status, parsed) = emulator.call(target, &fill_body(&overflow)).await;
        assert_eq!(
            status, 400,
            "case {store}: a create past the quota must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceLimitExceeded",
            "case {store}: the refusal must use the operation's modelled limit error, \
             got {parsed:?}"
        );

        // Nothing was stored for the refused request.
        let (read_status, read_body) = emulator
            .call(reader, &json!({ name_member: overflow.clone() }))
            .await;
        assert_eq!(
            read_status, 400,
            "case {store}: the refused record must not be stored, got {read_status} {read_body}"
        );

        // And a delete frees a slot, so the same create then succeeds: the bound is a
        // live count, not a one-way counter.
        let freed = format!("q{:04}", QUOTA - 1);
        let delete_target = match store {
            "models" => "DeleteModel",
            "endpoint-configs" => "DeleteEndpointConfig",
            _ => "DeleteEndpoint",
        };
        emulator
            .ok(delete_target, &json!({ name_member: freed }))
            .await;
        emulator.ok(target, &fill_body(&overflow)).await;
    }
}

/// RULE: a request outside a member's modelled constraint is refused at the boundary with
/// HTTP 400, before any state is written.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound the model states for these operations —
    // `ModelName` len 0..63, `ProductionVariants` len 1..10, `MaxResults` range 1..100.
    let too_long = "a".repeat(64);
    let eleven_variants: Vec<Value> = (0..11)
        .map(|index| json!({ "VariantName": format!("v{index}") }))
        .collect();
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "model-name-past-63",
            "CreateModel",
            json!({ "ModelName": too_long }),
        ),
        (
            "model-name-with-an-illegal-character",
            "CreateModel",
            json!({ "ModelName": "not a model name" }),
        ),
        (
            "zero-production-variants",
            "CreateEndpointConfig",
            json!({ "EndpointConfigName": "empty", "ProductionVariants": [] }),
        ),
        (
            "eleven-production-variants",
            "CreateEndpointConfig",
            json!({ "EndpointConfigName": "eleven", "ProductionVariants": eleven_variants }),
        ),
        (
            "max-results-below-1",
            "ListModels",
            json!({ "MaxResults": 0 }),
        ),
        (
            "max-results-above-100",
            "ListModels",
            json!({ "MaxResults": 101 }),
        ),
    ];

    for (case, target, body) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {target} must be refused, got {status} {parsed}"
        );
    }

    // Nothing above reached state: the store is still empty.
    let listed = emulator.ok("ListModels", &json!({})).await;
    assert_eq!(
        listed_names(&listed),
        Vec::<String>::new(),
        "a refused request must write nothing, got {listed:?}"
    );
}

/// Create `model -> endpoint config -> endpoint` named after `prefix` and let the
/// endpoint's clock-owed `Creating` -> `InService` transition fall due, which is what
/// `CreateInferenceComponent` requires of the endpoint it deploys onto.
///
/// Deliberately does NOT call `DescribeEndpoint` afterwards: the transition is owed by
/// the clock, so `CreateInferenceComponent` has to see it without a reader having
/// materialised it first.
async fn in_service_endpoint(emulator: &Emulator, prefix: &str) -> (String, String) {
    let model = format!("{prefix}-model");
    let config = format!("{prefix}-cfg");
    let endpoint = format!("{prefix}-ep");
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": model, "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    emulator
        .ok("CreateEndpointConfig", &one_variant_config(&config, &model))
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": endpoint, "EndpointConfigName": config }),
        )
        .await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    (endpoint, model)
}

/// One `InferenceComponentSpecification` naming `model`, with the compute requirement
/// whose `MinMemoryRequiredInMb` member the shape makes required.
fn model_specification(model: &str, instance_type: &str) -> Value {
    json!({
        "ModelName": model,
        "InstanceType": instance_type,
        "ComputeResourceRequirements": {
            "MinMemoryRequiredInMb": 1024,
            "NumberOfCpuCoresRequired": 1.5,
        },
    })
}

/// RULE: a specification summary replays every member of the specification that was
/// stored, with the one shape difference the summary makes — the container's `Image` is
/// reported as its `DeployedImage`'s `SpecifiedImage`.
fn assert_specification_summary(case: &str, sent: &Value, summary: &Value) {
    for member in [
        "InstanceType",
        "ModelName",
        "BaseInferenceComponentName",
        "StartupParameters",
        "ComputeResourceRequirements",
        "SchedulingConfig",
        "DataCacheConfig",
    ] {
        assert_eq!(
            summary.get(member),
            sent.get(member),
            "case {case}: {member} must be replayed as stored, got {summary:?}"
        );
    }
    match sent.get("Container") {
        None => assert!(
            summary.get("Container").is_none(),
            "case {case}: no container was sent, so none may be reported, got {summary:?}"
        ),
        Some(container) => {
            let reported = summary
                .get("Container")
                .unwrap_or_else(|| panic!("case {case}: Container must be reported in {summary:?}"));
            assert_eq!(
                reported.pointer("/DeployedImage/SpecifiedImage"),
                container.get("Image"),
                "case {case}: the container image must be reported as the deployed image's \
                 SpecifiedImage, got {reported:?}"
            );
            assert_eq!(
                reported.get("ArtifactUrl"),
                container.get("ArtifactUrl"),
                "case {case}: ArtifactUrl, got {reported:?}"
            );
            assert_eq!(
                reported.get("Environment"),
                container.get("Environment"),
                "case {case}: Environment, got {reported:?}"
            );
        }
    }
}

/// RULE: `CreateInferenceComponent` mints
/// `arn:aws:sagemaker:<signed region>:<account>:inference-component/<name>`, stores the
/// component against its hosting endpoint in `Creating` with no copies placed, and
/// `DescribeInferenceComponent` replays exactly what was stored for every shape of
/// specification the model allows; once the clock-owed placement falls due the component
/// reports `InService` with every requested copy current.
#[tokio::test]
async fn an_inference_component_round_trips_through_describe_for_every_specification_shape() {
    let emulator = Emulator::new();
    let (endpoint, model) = in_service_endpoint(&emulator, "ic").await;
    let endpoint_arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:endpoint/{endpoint}");

    // A second model, so a multi-specification row names more than one.
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "ic-model-two", "PrimaryContainer": primary_container("demo:2") }),
        )
        .await;

    // Rows differ along the axes the rule generalises over: which of the two
    // specification members carries the request, the list's size at both published
    // bounds (`Specifications` is len 1..5), whether a production variant is named,
    // the copy count (including the `CopyCount` range minimum, 0) and its absence, and
    // which optional sub-structures the specification carries.
    // `Specifications` is documented as "one per instance type", so the five-row case
    // names five DIFFERENT instance types. Every value is a member of the modelled
    // `ProductionVariantInstanceType` enum (smithy_query shape=ProductionVariantInstanceType),
    // and none of them is the one the recorded case used.
    let five_instance_types = [
        "ml.m5.large",
        "ml.c5.xlarge",
        "ml.g5.2xlarge",
        "ml.r5.large",
        "ml.m6i.large",
    ];
    let five_specifications: Vec<Value> = five_instance_types
        .iter()
        .enumerate()
        .map(|(index, instance_type)| {
            let mut specification = model_specification(&model, instance_type);
            specification["StartupParameters"] =
                json!({ "ModelDataDownloadTimeoutInSeconds": 60 + index });
            specification
        })
        .collect();
    let cases: Vec<(&str, Value)> = vec![
        (
            "singular-model-specification-with-copies",
            json!({
                "Specification": model_specification(&model, "ml.m5.large"),
                "RuntimeConfig": { "CopyCount": 2 },
            }),
        ),
        (
            "container-specification-on-a-named-variant",
            json!({
                "VariantName": "AllTraffic",
                "Specification": {
                    "Container": {
                        "Image": "demo-image:3",
                        "ArtifactUrl": "s3://bucket/model.tar.gz",
                        "Environment": { "MODEL_ID": "tiny", "WORKERS": "2" },
                    },
                    "StartupParameters": {
                        "ModelDataDownloadTimeoutInSeconds": 60,
                        "ContainerStartupHealthCheckTimeoutInSeconds": 3600,
                    },
                    "ComputeResourceRequirements": {
                        "MinMemoryRequiredInMb": 128,
                        "MaxMemoryRequiredInMb": 256,
                        "NumberOfAcceleratorDevicesRequired": 1.0,
                    },
                },
                "RuntimeConfig": { "CopyCount": 1 },
            }),
        ),
        (
            "zero-copies-at-the-range-minimum",
            json!({
                "Specification": model_specification(&model, "ml.c5.large"),
                "RuntimeConfig": { "CopyCount": 0 },
            }),
        ),
        (
            "no-runtime-config",
            json!({ "Specification": model_specification(&model, "ml.m5.xlarge") }),
        ),
        (
            "data-cache-and-scheduling-config",
            json!({
                "Specification": {
                    "ModelName": model,
                    "ComputeResourceRequirements": { "MinMemoryRequiredInMb": 2048 },
                    "DataCacheConfig": { "EnableCaching": true },
                    "SchedulingConfig": { "PlacementStrategy": "BINPACK" },
                },
                "RuntimeConfig": { "CopyCount": 4 },
                "Tags": [{ "Key": "team", "Value": "inference" }],
            }),
        ),
        (
            "specifications-list-of-one",
            json!({
                "Specifications": [model_specification("ic-model-two", "ml.g5.xlarge")],
                "RuntimeConfig": { "CopyCount": 1 },
            }),
        ),
        (
            "specifications-list-of-five",
            json!({
                "Specifications": five_specifications,
                "RuntimeConfig": { "CopyCount": 3 },
            }),
        ),
    ];

    // Create every row, then read them all back before and after the due span, so the
    // transition is observed once for the whole table.
    let mut created: Vec<(&str, String, Value)> = Vec::new();
    for (index, (case, extra)) in cases.into_iter().enumerate() {
        let name = format!("ic-component{index}");
        let mut body = json!({
            "InferenceComponentName": name,
            "EndpointName": endpoint,
        });
        for (member, value) in extra.as_object().expect("the row is an object") {
            body[member] = value.clone();
        }
        let output = emulator.ok("CreateInferenceComponent", &body).await;
        assert_eq!(
            str_field(&output, "InferenceComponentArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:inference-component/{name}"),
            "case {case}: the minted ARN, got {output:?}"
        );
        created.push((case, name, body));
    }

    for (case, name, body) in &created {
        let described = emulator
            .ok(
                "DescribeInferenceComponent",
                &json!({ "InferenceComponentName": name }),
            )
            .await;

        assert_eq!(
            str_field(&described, "InferenceComponentName"),
            *name,
            "case {case}: InferenceComponentName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "InferenceComponentArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:inference-component/{name}"),
            "case {case}: InferenceComponentArn, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "EndpointName"),
            endpoint,
            "case {case}: EndpointName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "EndpointArn"),
            endpoint_arn,
            "case {case}: EndpointArn, got {described:?}"
        );
        assert_eq!(
            described.get("VariantName"),
            body.get("VariantName"),
            "case {case}: VariantName must be replayed as sent, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "InferenceComponentStatus"),
            "Creating",
            "case {case}: a component whose placement is not yet due reports Creating, \
             got {described:?}"
        );
        assert_eq!(
            described.get("CreationTime"),
            described.get("LastModifiedTime"),
            "case {case}: nothing has changed the component yet, got {described:?}"
        );
        assert!(
            described.get("FailureReason").is_none(),
            "case {case}: a component that did not fail reports no reason, got {described:?}"
        );
        assert!(
            described.get("LastDeploymentConfig").is_none(),
            "case {case}: no update has run, so no deployment config is reported, \
             got {described:?}"
        );

        // Exactly one of the two specification members comes back, matching the one that
        // was sent, and each summary replays its specification.
        match (body.get("Specification"), body.get("Specifications")) {
            (Some(sent), None) => {
                assert!(
                    described.get("Specifications").is_none(),
                    "case {case}: the plural member must not be reported beside the \
                     singular one, got {described:?}"
                );
                let summary = described.get("Specification").unwrap_or_else(|| {
                    panic!("case {case}: Specification must be reported in {described:?}")
                });
                assert_specification_summary(case, sent, summary);
            }
            (None, Some(sent)) => {
                assert!(
                    described.get("Specification").is_none(),
                    "case {case}: the singular member must not be reported beside the \
                     plural one, got {described:?}"
                );
                let sent = sent.as_array().expect("the row sent a list");
                let summaries = described
                    .get("Specifications")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| {
                        panic!("case {case}: Specifications must be reported in {described:?}")
                    });
                assert_eq!(
                    summaries.len(),
                    sent.len(),
                    "case {case}: one summary per specification, got {summaries:?}"
                );
                for (index, (sent, summary)) in sent.iter().zip(summaries).enumerate() {
                    assert_specification_summary(&format!("{case}[{index}]"), sent, summary);
                }
            }
            other => panic!("case {case}: the row must send exactly one member, sent {other:?}"),
        }

        // No copy is running while the placement is still owed.
        match body.pointer("/RuntimeConfig/CopyCount") {
            None => assert!(
                described.get("RuntimeConfig").is_none(),
                "case {case}: no runtime config was sent, so none may be reported, \
                 got {described:?}"
            ),
            Some(desired) => {
                let runtime = described.get("RuntimeConfig").unwrap_or_else(|| {
                    panic!("case {case}: RuntimeConfig must be reported in {described:?}")
                });
                assert_eq!(
                    runtime.get("DesiredCopyCount"),
                    Some(desired),
                    "case {case}: DesiredCopyCount, got {runtime:?}"
                );
                assert_eq!(
                    runtime.get("CurrentCopyCount").and_then(Value::as_i64),
                    Some(0),
                    "case {case}: nothing is placed before the due span, got {runtime:?}"
                );
            }
        }
    }

    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    for (case, name, body) in &created {
        let settled = emulator
            .ok(
                "DescribeInferenceComponent",
                &json!({ "InferenceComponentName": name }),
            )
            .await;
        assert_eq!(
            str_field(&settled, "InferenceComponentStatus"),
            "InService",
            "case {case}: after the due span the component is placed, got {settled:?}"
        );
        let created_at = settled
            .get("CreationTime")
            .and_then(Value::as_f64)
            .expect("CreationTime");
        let modified_at = settled
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .expect("LastModifiedTime");
        assert!(
            modified_at > created_at,
            "case {case}: LastModifiedTime {modified_at} must move past CreationTime \
             {created_at} when the status changes"
        );

        if let Some(desired) = body.pointer("/RuntimeConfig/CopyCount") {
            let runtime = settled
                .get("RuntimeConfig")
                .unwrap_or_else(|| panic!("case {case}: RuntimeConfig in {settled:?}"));
            assert_eq!(
                runtime.get("CurrentCopyCount"),
                Some(desired),
                "case {case}: every requested copy is current once placed, got {runtime:?}"
            );
            // `PlacementStatus` is keyed by instance type, so it is reported exactly
            // when the specifications name ONE distinct type — then every current copy
            // sits on it. Several distinct types, or none, leave the member out.
            let mut instance_types: Vec<String> = match (
                body.get("Specification"),
                body.get("Specifications").and_then(Value::as_array),
            ) {
                (Some(single), None) => single
                    .get("InstanceType")
                    .and_then(Value::as_str)
                    .map(|instance_type| vec![instance_type.to_string()])
                    .unwrap_or_default(),
                (None, Some(list)) => list
                    .iter()
                    .filter_map(|specification| specification.get("InstanceType"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                _ => Vec::new(),
            };
            instance_types.sort();
            instance_types.dedup();
            match runtime.get("PlacementStatus").and_then(Value::as_array) {
                None => assert_ne!(
                    instance_types.len(),
                    1,
                    "case {case}: {instance_types:?} is one instance type, so a placement \
                     status is owed, got {runtime:?}"
                ),
                Some(placements) => {
                    let reported: Vec<String> = placements
                        .iter()
                        .map(|placement| str_field(placement, "InstanceType"))
                        .collect();
                    assert_eq!(
                        reported, instance_types,
                        "case {case}: the placement status names the one instance type the \
                         specifications named, got {runtime:?}"
                    );
                    for placement in placements {
                        assert_eq!(
                            placement.get("CurrentCopyCount"),
                            Some(desired),
                            "case {case}: placed copies per instance type, got {placement:?}"
                        );
                    }
                }
            }
        }
    }
}

/// RULE: a component is only created from a request whose members it can satisfy — an
/// endpoint or model that does not exist, a production variant the endpoint does not
/// carry, a name already taken, and a request carrying both or neither specification
/// member are all refused with HTTP 400 `ValidationException` and store nothing.
#[tokio::test]
async fn creating_an_inference_component_is_refused_when_the_request_cannot_be_satisfied() {
    let emulator = Emulator::new();
    let (endpoint, model) = in_service_endpoint(&emulator, "bad").await;

    // One component that exists, so a duplicate name has something to collide with.
    emulator
        .ok(
            "CreateInferenceComponent",
            &json!({
                "InferenceComponentName": "bad-taken",
                "EndpointName": endpoint,
                "Specification": model_specification(&model, "ml.m5.large"),
                "RuntimeConfig": { "CopyCount": 1 },
            }),
        )
        .await;

    // Rows differ along which part of the request cannot be satisfied.
    let cases: Vec<(&str, Value)> = vec![
        (
            "unknown-endpoint",
            json!({
                "InferenceComponentName": "bad-c1",
                "EndpointName": "no-such-endpoint",
                "Specification": model_specification(&model, "ml.m5.large"),
            }),
        ),
        (
            "unknown-model",
            json!({
                "InferenceComponentName": "bad-c3",
                "EndpointName": endpoint,
                "Specification": model_specification("no-such-model", "ml.m5.large"),
            }),
        ),
        (
            "unknown-production-variant",
            json!({
                "InferenceComponentName": "bad-c4",
                "EndpointName": endpoint,
                "VariantName": "NoSuchVariant",
                "Specification": model_specification(&model, "ml.m5.large"),
            }),
        ),
        (
            "both-specification-members",
            json!({
                "InferenceComponentName": "bad-c5",
                "EndpointName": endpoint,
                "Specification": model_specification(&model, "ml.m5.large"),
                "Specifications": [model_specification(&model, "ml.m5.large")],
            }),
        ),
        (
            "neither-specification-member",
            json!({
                "InferenceComponentName": "bad-c6",
                "EndpointName": endpoint,
            }),
        ),
        (
            "name-already-taken",
            json!({
                "InferenceComponentName": "bad-taken",
                "EndpointName": endpoint,
                "Specification": model_specification(&model, "ml.m5.large"),
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("CreateInferenceComponent", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: the create must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: the refusal's error code, got {parsed:?}"
        );
        assert!(
            !message(&parsed).is_empty(),
            "case {case}: the refusal must say what was wrong, got {parsed:?}"
        );

        // Nothing was stored for a refused request.
        let name = str_field(&body, "InferenceComponentName");
        let (read_status, read_body) = emulator
            .call(
                "DescribeInferenceComponent",
                &json!({ "InferenceComponentName": name }),
            )
            .await;
        if case == "name-already-taken" {
            assert_eq!(
                read_status, 200,
                "case {case}: the component that was already there must survive, \
                 got {read_status} {read_body}"
            );
        } else {
            assert_eq!(
                read_status, 400,
                "case {case}: the refused component must not be stored, \
                 got {read_status} {read_body}"
            );
        }
    }

    // And a reader for a component nobody created is refused, not answered emptily.
    let (status, parsed) = emulator
        .call(
            "DescribeInferenceComponent",
            &json!({ "InferenceComponentName": "never-created" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case describe-unknown-component: got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case describe-unknown-component: the refusal's error code, got {parsed:?}"
    );
}

/// RULE: a component is accepted on an endpoint that is still being created, and its
/// copies are placed only once that endpoint is serving: it reports `Creating` with no
/// current copy until then, and the instant its placement falls due is after the instant
/// the endpoint went `InService`.
#[tokio::test]
async fn a_component_on_a_still_creating_endpoint_is_placed_only_once_the_endpoint_serves() {
    // The component's span runs from the endpoint's, and both are 300 ms
    // (`state::ENDPOINT_IN_SERVICE_AFTER`, `state::INFERENCE_COMPONENT_IN_SERVICE_AFTER`),
    // so this waits past the two of them with margin.
    const PAST_BOTH_DUE: Duration = Duration::from_millis(1_200);

    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({
                "ModelName": "wait-ic-model",
                "PrimaryContainer": primary_container("demo:1"),
            }),
        )
        .await;
    emulator
        .ok(
            "CreateEndpointConfig",
            &one_variant_config("wait-ic-cfg", "wait-ic-model"),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "wait-ic-ep", "EndpointConfigName": "wait-ic-cfg" }),
        )
        .await;

    // No wait at all: the endpoint is still `Creating` when the component is created.
    let created = emulator
        .ok(
            "CreateInferenceComponent",
            &json!({
                "InferenceComponentName": "wait-ic",
                "EndpointName": "wait-ic-ep",
                "Specification": model_specification("wait-ic-model", "ml.m5.large"),
                "RuntimeConfig": { "CopyCount": 2 },
            }),
        )
        .await;
    assert_eq!(
        str_field(&created, "InferenceComponentArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:inference-component/wait-ic"),
        "case created-on-a-creating-endpoint: the minted ARN, got {created:?}"
    );

    let creating = emulator
        .ok(
            "DescribeInferenceComponent",
            &json!({ "InferenceComponentName": "wait-ic" }),
        )
        .await;
    assert_eq!(
        str_field(&creating, "InferenceComponentStatus"),
        "Creating",
        "case created-on-a-creating-endpoint: got {creating:?}"
    );
    assert_eq!(
        creating
            .pointer("/RuntimeConfig/CurrentCopyCount")
            .and_then(Value::as_i64),
        Some(0),
        "case created-on-a-creating-endpoint: no copy runs yet, got {creating:?}"
    );

    tokio::time::sleep(PAST_BOTH_DUE).await;

    let endpoint = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": "wait-ic-ep" }))
        .await;
    assert_eq!(
        str_field(&endpoint, "EndpointStatus"),
        "InService",
        "case after-both-due-spans: the endpoint serves, got {endpoint:?}"
    );
    let settled = emulator
        .ok(
            "DescribeInferenceComponent",
            &json!({ "InferenceComponentName": "wait-ic" }),
        )
        .await;
    assert_eq!(
        str_field(&settled, "InferenceComponentStatus"),
        "InService",
        "case after-both-due-spans: the component is placed, got {settled:?}"
    );
    assert_eq!(
        settled
            .pointer("/RuntimeConfig/CurrentCopyCount")
            .and_then(Value::as_i64),
        Some(2),
        "case after-both-due-spans: every requested copy runs, got {settled:?}"
    );

    // The placement is not due before the endpoint is serving: each `LastModifiedTime`
    // is the instant its own transition fell due, so the component's is the later one.
    let endpoint_in_service_at = endpoint
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .expect("the endpoint's LastModifiedTime");
    let placed_at = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .expect("the component's LastModifiedTime");
    assert!(
        placed_at > endpoint_in_service_at,
        "case after-both-due-spans: the component was placed at {placed_at}, which must be \
         after the endpoint started serving at {endpoint_in_service_at}"
    );
}

/// RULE: `DeleteInferenceComponent` releases the component it names — the reader no
/// longer finds it, a second delete is refused, the name is free again, and no other
/// component is touched.
#[tokio::test]
async fn deleting_an_inference_component_releases_only_that_component() {
    let emulator = Emulator::new();
    let (endpoint, model) = in_service_endpoint(&emulator, "del").await;

    // Rows differ along the shape of the component being deleted: a plain one, one
    // pinned to a production variant, and one created from the plural member.
    let bodies = [
        json!({
            "InferenceComponentName": "del-plain",
            "EndpointName": endpoint,
            "Specification": model_specification(&model, "ml.m5.large"),
            "RuntimeConfig": { "CopyCount": 1 },
        }),
        json!({
            "InferenceComponentName": "del-on-variant",
            "EndpointName": endpoint,
            "VariantName": "AllTraffic",
            "Specification": model_specification(&model, "ml.m5.large"),
        }),
        json!({
            "InferenceComponentName": "del-plural",
            "EndpointName": endpoint,
            "Specifications": [model_specification(&model, "ml.c5.large")],
        }),
    ];
    for body in &bodies {
        emulator.ok("CreateInferenceComponent", body).await;
    }

    for body in &bodies {
        let name = str_field(body, "InferenceComponentName");
        emulator
            .ok(
                "DeleteInferenceComponent",
                &json!({ "InferenceComponentName": name }),
            )
            .await;

        // Gone for the reader, and gone for a second delete.
        for target in ["DescribeInferenceComponent", "DeleteInferenceComponent"] {
            let (status, parsed) = emulator
                .call(target, &json!({ "InferenceComponentName": name }))
                .await;
            assert_eq!(
                status, 400,
                "case {name}/{target}: a deleted component is gone, got {status} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {name}/{target}: the refusal's error code, got {parsed:?}"
            );
        }

        // The name is free again: the same create succeeds, and the record it leaves is
        // readable.
        emulator.ok("CreateInferenceComponent", body).await;
        let described = emulator
            .ok(
                "DescribeInferenceComponent",
                &json!({ "InferenceComponentName": name }),
            )
            .await;
        assert_eq!(
            str_field(&described, "InferenceComponentName"),
            name,
            "case {name}: the recreated component reads back, got {described:?}"
        );
    }

    // Nothing else was released along the way: every component is still readable.
    for body in &bodies {
        let name = str_field(body, "InferenceComponentName");
        emulator
            .ok(
                "DescribeInferenceComponent",
                &json!({ "InferenceComponentName": name }),
            )
            .await;
    }
}

/// RULE: an inference component's ARN names the region the caller signed for (the
/// emulator default when the request is unsigned), the `inference-component` resource
/// kind and the stored name, whatever the name's shape — and the component reports the
/// ARN of the endpoint it was deployed onto in that same region.
#[tokio::test]
async fn an_inference_component_arn_names_the_signed_region_the_kind_and_the_stored_name() {
    // Rows differ along the name's shape, including the modelled 63-character maximum
    // (`InferenceComponentName` is len 0..63, smithy_query
    // operation=CreateInferenceComponent).
    let names = [
        "c",
        "component1",
        "demo-component-with-hyphens",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    // Rows differ along the signed region, plus the unsigned request, whose ARN falls
    // back to `state::DEFAULT_REGION`.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        let emulator = Emulator::new();
        let model = "arn-model";
        let config = "arn-cfg";
        let endpoint = "arn-ep";
        emulator
            .call_in(
                region,
                "CreateModel",
                &json!({ "ModelName": model, "PrimaryContainer": primary_container("demo:1") }),
            )
            .await;
        emulator
            .call_in(region, "CreateEndpointConfig", &one_variant_config(config, model))
            .await;
        emulator
            .call_in(
                region,
                "CreateEndpoint",
                &json!({ "EndpointName": endpoint, "EndpointConfigName": config }),
            )
            .await;
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

        for name in names {
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateInferenceComponent",
                    &json!({
                        "InferenceComponentName": name,
                        "EndpointName": endpoint,
                        "Specification": model_specification(model, "ml.m5.large"),
                        "RuntimeConfig": { "CopyCount": 1 },
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {region}/{name}: the create must succeed, got {status} {created}"
            );
            assert_eq!(
                str_field(&created, "InferenceComponentArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:inference-component/{name}"),
                "case {region}/{name}: the minted ARN, got {created:?}"
            );

            let described = emulator
                .call_in(
                    region,
                    "DescribeInferenceComponent",
                    &json!({ "InferenceComponentName": name }),
                )
                .await
                .1;
            assert_eq!(
                str_field(&described, "InferenceComponentArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:inference-component/{name}"),
                "case {region}/{name}: the replayed ARN, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "EndpointArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint/{endpoint}"),
                "case {region}/{name}: the hosting endpoint's ARN, got {described:?}"
            );
        }
    }

    // The unsigned request: no credential scope, so every ARN falls back to the
    // emulator's default region.
    let emulator = Emulator::new();
    emulator
        .call_unsigned(
            "CreateModel",
            &json!({ "ModelName": "u-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    emulator
        .call_unsigned("CreateEndpointConfig", &one_variant_config("u-cfg", "u-model"))
        .await;
    emulator
        .call_unsigned(
            "CreateEndpoint",
            &json!({ "EndpointName": "u-ep", "EndpointConfigName": "u-cfg" }),
        )
        .await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    let (status, created) = emulator
        .call_unsigned(
            "CreateInferenceComponent",
            &json!({
                "InferenceComponentName": "u-component",
                "EndpointName": "u-ep",
                "Specification": model_specification("u-model", "ml.m5.large"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: the create must succeed, got {status} {created}"
    );
    assert_eq!(
        str_field(&created, "InferenceComponentArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:inference-component/u-component"),
        "case unsigned: the ARN must name the emulator's default region, got {created:?}"
    );
}
