// SPDX-License-Identifier: CC0-1.0

//! `UpdateFunctionConfiguration` end to end over HTTP.
//!
//! The rule under test: an update of a function's configuration replaces the stored
//! value of EVERY setting the request names, leaves every setting it omits exactly as it
//! was, mints a new `RevisionId` and a new `LastModified` for the unpublished (`$LATEST`)
//! configuration it leaves behind, and reports what it persisted — so a later
//! `GetFunctionConfiguration` answers the same thing. It touches only the unpublished
//! version, it refuses a name qualified with a published version or an alias (400), a
//! function it holds no record of (404), a `RevisionId` that is not the stored one (412),
//! and a function that cannot be modified yet because it is still being created or
//! because an update of it is still in flight (409).
//!
//! Every row builds its function through the real `CreateFunction` over HTTP and reads
//! the result back through the real `GetFunctionConfiguration`, so a hardcoded answer
//! fails. The clock-driven halves — the `Pending` -> `Active` move `CreateFunction`
//! documents and the `InProgress` -> `Successful` move this operation documents — are
//! exercised by moving the stored due instants into the past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::seed_image;

/// A SigV4 credential scope naming this service's signing name (`lambda`), which is what
/// routes a request here. Nothing in it is verified: the harness turns request
/// authentication off, exactly as the emulator runs unless started with it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A second well-formed execution role ARN, so a row that updates `Role` reads back a
/// value no default could have produced.
const OTHER_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec-two";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The members every update is expected to change, whatever else it names: a fresh
/// revision, a fresh `LastModified`, and the in-flight update status with the reason and
/// reason code that report the work that status covers.
const VOLATILE_MEMBERS: [&str; 5] = [
    "RevisionId",
    "LastModified",
    "LastUpdateStatus",
    "LastUpdateStatusReason",
    "LastUpdateStatusReasonCode",
];

/// The member DERIVED from `Runtime`, so a row that replaces the runtime moves it too
/// without naming it: `RuntimeVersionConfig` reports "the ARN of the runtime version you
/// want the function to use" (`smithy_query(query="shape",
/// shape="RuntimeVersionConfig")`), which identifies the managed runtime the function is
/// pinned to — another runtime is another runtime version. Its VALUE is asserted in
/// `policy_versions_and_resources.rs`; here only the invariant "a member the request did
/// not name is unchanged" is, and this one is not independent of `Runtime`.
const DERIVED_FROM_RUNTIME: &str = "RuntimeVersionConfig";

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request and answer its status and body.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answer = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, answer)
}

/// `UpdateFunctionConfiguration`
/// (`PUT /2015-03-31/functions/{FunctionName}/configuration`).
async fn update_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    patch: &Value,
) -> (u16, Value) {
    let uri = format!("{FUNCTIONS_URI}/{reference}/configuration");
    call(router, "PUT", &uri, patch).await
}

/// `GetFunctionConfiguration`
/// (`GET /2015-03-31/functions/{FunctionName}/configuration`).
async fn get_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> (u16, Value) {
    let uri = match qualifier {
        Some(qualifier) => {
            format!("{FUNCTIONS_URI}/{reference}/configuration?Qualifier={qualifier}")
        }
        None => format!("{FUNCTIONS_URI}/{reference}/configuration"),
    };
    call(router, "GET", &uri, &Value::Null).await
}

/// `PublishVersion` (`POST /2015-03-31/functions/{FunctionName}/versions`).
async fn publish_version(router: &MultiServiceRouter, reference: &str) -> (u16, Value) {
    let uri = format!("{FUNCTIONS_URI}/{reference}/versions");
    call(router, "POST", &uri, &Value::Null).await
}

/// Deployment package bytes of a given length; only their length and digest are ever
/// reported.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// The `Code.ZipFile` member of a create request.
fn zip_file() -> Value {
    json!(base64::engine::general_purpose::STANDARD.encode(package_bytes(64)))
}

/// The unqualified ARN a function of `name` must carry.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Move every in-flight update's settle instant into the past, so the next read
/// materialises the `InProgress` -> `Successful` transition the clock owes.
fn make_update_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// Create one `Zip`-package function, with every setting a row might leave untouched
/// already set to something no default would produce.
fn create_zip_request(name: &str) -> Value {
    json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.11",
        "Handler": "index.handler",
        "Description": "as created",
        "Timeout": 7,
        "MemorySize": 256,
        "Environment": { "Variables": { "CREATED": "yes" } },
        "Code": { "ZipFile": zip_file() },
    })
}

/// The container image every `Image`-package function of this suite is created on. It is
/// pushed to the registry first ([`seed_image`]), because a create RESOLVES its reference
/// there.
const IMAGE_URI: &str = "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest";

/// Create one `Image`-package function: "You do not need to specify the handler and
/// runtime properties."
fn create_image_request(name: &str) -> Value {
    json!({
        "FunctionName": name,
        "Role": ROLE,
        "PackageType": "Image",
        "Description": "as created",
        "Code": { "ImageUri": IMAGE_URI },
    })
}

/// Which package a row's function is built from. The two admit different settings, which
/// is itself part of the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Package {
    Zip,
    Image,
}

/// One row: a patch to send, and the members it must change. Every member the table does
/// not name must come back exactly as the function was created.
struct Case {
    /// Names the row, and names its function (so rows never share state).
    label: &'static str,
    /// The package the function is built from.
    package: Package,
    /// The `UpdateFunctionConfiguration` request body.
    patch: Value,
    /// The members the patch must change, as a JSON object; a `null` value means the
    /// member must be ABSENT from the answer.
    expected: Value,
}

/// The table: one row per kind of setting `UpdateFunctionConfiguration` models, varying
/// the value, its type (string, integer, enum, map, list, structure), its size, and the
/// count of settings named at once. The recorded defect named `Description` and
/// `MemorySize`; every other row is the same rule on a different member.
///
/// The published bounds are the narrower of the model's range and AWS's published quota:
/// `Timeout` is `@range(min: 1, max: 5400)` under a 900-second quota and `MemorySize` is
/// `@range(min: 128, max: 32768)` under a 10,240 MB quota, so both are exercised at each
/// end of what the service admits (`function_configuration_quotas.rs` covers the refusals
/// past the quota).
/// The enum values come from the model's own enums (`Runtime`, `TracingMode`, `LogFormat`,
/// `ApplicationLogLevel`, `SystemLogLevel`, `SnapStartApplyOn`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "description-replaced",
            package: Package::Zip,
            patch: json!({ "Description": "as updated" }),
            expected: json!({ "Description": "as updated" }),
        },
        Case {
            label: "description-emptied",
            package: Package::Zip,
            patch: json!({ "Description": "" }),
            expected: json!({ "Description": "" }),
        },
        Case {
            label: "timeout-at-minimum",
            package: Package::Zip,
            patch: json!({ "Timeout": 1 }),
            expected: json!({ "Timeout": 1 }),
        },
        Case {
            label: "timeout-at-maximum",
            package: Package::Zip,
            patch: json!({ "Timeout": 900 }),
            expected: json!({ "Timeout": 900 }),
        },
        Case {
            label: "memory-at-minimum",
            package: Package::Zip,
            patch: json!({ "MemorySize": 128 }),
            expected: json!({ "MemorySize": 128 }),
        },
        Case {
            label: "memory-at-maximum",
            package: Package::Zip,
            patch: json!({ "MemorySize": 10240 }),
            expected: json!({ "MemorySize": 10240 }),
        },
        Case {
            label: "two-settings-at-once",
            package: Package::Zip,
            patch: json!({ "Timeout": 30, "MemorySize": 3008 }),
            expected: json!({ "Timeout": 30, "MemorySize": 3008 }),
        },
        Case {
            label: "runtime-replaced",
            package: Package::Zip,
            patch: json!({ "Runtime": "nodejs22.x" }),
            expected: json!({ "Runtime": "nodejs22.x" }),
        },
        Case {
            label: "handler-replaced",
            package: Package::Zip,
            patch: json!({ "Handler": "app.lambda_handler" }),
            expected: json!({ "Handler": "app.lambda_handler" }),
        },
        Case {
            label: "role-replaced",
            package: Package::Zip,
            patch: json!({ "Role": OTHER_ROLE }),
            expected: json!({ "Role": OTHER_ROLE }),
        },
        Case {
            label: "environment-replaced-wholly",
            package: Package::Zip,
            patch: json!({ "Environment": { "Variables": { "STAGE": "prod", "TIER": "2" } } }),
            expected: json!({
                "Environment": { "Variables": { "STAGE": "prod", "TIER": "2" } },
            }),
        },
        Case {
            label: "environment-emptied",
            package: Package::Zip,
            patch: json!({ "Environment": { "Variables": {} } }),
            // The member reports the variables the function HOLDS: an update that states an
            // environment naming none clears them, so the member is part of no answer
            // afterwards (the row `no-setting-named` leaves the member out instead, and the
            // function keeps what it had).
            expected: json!({ "Environment": null }),
        },
        Case {
            label: "tracing-mode-replaced",
            package: Package::Zip,
            patch: json!({ "TracingConfig": { "Mode": "Active" } }),
            expected: json!({ "TracingConfig": { "Mode": "Active" } }),
        },
        Case {
            label: "kms-key-set",
            package: Package::Zip,
            patch: json!({
                "KMSKeyArn": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
            }),
            expected: json!({
                "KMSKeyArn": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
            }),
        },
        Case {
            label: "ephemeral-storage-resized",
            package: Package::Zip,
            patch: json!({ "EphemeralStorage": { "Size": 1024 } }),
            expected: json!({ "EphemeralStorage": { "Size": 1024 } }),
        },
        Case {
            label: "vpc-attached",
            package: Package::Zip,
            patch: json!({
                "VpcConfig": {
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                },
            }),
            expected: json!({
                "VpcConfig": {
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                    // The attachment's dual-stack egress setting is a setting it HAS, so the
                    // response reports it as off rather than omitting it for a request that
                    // did not turn it on.
                    "Ipv6AllowedForDualStack": false,
                },
            }),
        },
        Case {
            label: "dead-letter-queue-set",
            package: Package::Zip,
            patch: json!({
                "DeadLetterConfig": { "TargetArn": "arn:aws:sqs:us-east-1:000000000000:failures" },
            }),
            expected: json!({
                "DeadLetterConfig": { "TargetArn": "arn:aws:sqs:us-east-1:000000000000:failures" },
            }),
        },
        Case {
            label: "snapstart-applied",
            package: Package::Zip,
            // SnapStart is supported on the Python 3.12+ runtimes and not on the one the
            // zip row is created with, so this row moves the runtime with it — the one
            // patch that states two members, because the two settings are not independent
            // (`crate::state::runtime_supports_snap_start`).
            patch: json!({
                "Runtime": "python3.12",
                "SnapStart": { "ApplyOn": "PublishedVersions" },
            }),
            // `OptimizationStatus` reports "whether SnapStart is activated for the
            // specified function version", and `$LATEST` is not a snapshotted version.
            expected: json!({
                "Runtime": "python3.12",
                "SnapStart": { "ApplyOn": "PublishedVersions", "OptimizationStatus": "Off" },
            }),
        },
        Case {
            label: "snapstart-turned-off",
            package: Package::Zip,
            patch: json!({ "SnapStart": { "ApplyOn": "None" } }),
            expected: json!({
                "SnapStart": { "ApplyOn": "None", "OptimizationStatus": "Off" },
            }),
        },
        Case {
            label: "logging-config-set",
            package: Package::Zip,
            patch: json!({
                "LoggingConfig": {
                    "LogFormat": "JSON",
                    "ApplicationLogLevel": "DEBUG",
                    "SystemLogLevel": "WARN",
                    "LogGroup": "/aws/lambda/custom-group",
                },
            }),
            expected: json!({
                "LoggingConfig": {
                    "LogFormat": "JSON",
                    "ApplicationLogLevel": "DEBUG",
                    "SystemLogLevel": "WARN",
                    "LogGroup": "/aws/lambda/custom-group",
                },
            }),
        },
        // The two LEVEL members are the filters the JSON log format carries, and AWS
        // documents each one's default as INFO — so naming the format alone reports both
        // levels in effect rather than neither.
        Case {
            label: "logging-config-json-levels-defaulted",
            package: Package::Zip,
            patch: json!({ "LoggingConfig": { "LogFormat": "JSON" } }),
            expected: json!({
                "LoggingConfig": {
                    "LogFormat": "JSON",
                    "ApplicationLogLevel": "INFO",
                    "SystemLogLevel": "INFO",
                    "LogGroup": "/aws/lambda/logging-config-json-levels-defaulted",
                },
            }),
        },
        // One level named, the other defaulted: the one the caller set is kept, and the
        // other still reports the documented default.
        Case {
            label: "logging-config-json-one-level-named",
            package: Package::Zip,
            patch: json!({
                "LoggingConfig": { "LogFormat": "JSON", "SystemLogLevel": "WARN" },
            }),
            expected: json!({
                "LoggingConfig": {
                    "LogFormat": "JSON",
                    "ApplicationLogLevel": "INFO",
                    "SystemLogLevel": "WARN",
                    "LogGroup": "/aws/lambda/logging-config-json-one-level-named",
                },
            }),
        },
        // The plain-text format carries no level filtering at all, so neither member is
        // reported for it.
        Case {
            label: "logging-config-text-has-no-levels",
            package: Package::Zip,
            patch: json!({ "LoggingConfig": { "LogFormat": "Text" } }),
            expected: json!({
                "LoggingConfig": {
                    "LogFormat": "Text",
                    "LogGroup": "/aws/lambda/logging-config-text-has-no-levels",
                },
            }),
        },
        Case {
            label: "file-system-mounted",
            package: Package::Zip,
            // An Amazon EFS access point is reached through a mount target inside a VPC, so
            // the function that mounts one is attached to that VPC in the same request.
            patch: json!({
                "VpcConfig": {
                    "SubnetIds": ["subnet-0123456789abcdef0"],
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                },
                "FileSystemConfigs": [{
                    "Arn": "arn:aws:elasticfilesystem:us-east-1:000000000000:access-point/fsap-0123456789abcdef0",
                    "LocalMountPath": "/mnt/shared",
                }],
            }),
            expected: json!({
                "FileSystemConfigs": [{
                    "Arn": "arn:aws:elasticfilesystem:us-east-1:000000000000:access-point/fsap-0123456789abcdef0",
                    "LocalMountPath": "/mnt/shared",
                }],
            }),
        },
        Case {
            label: "layer-attached",
            package: Package::Zip,
            // A layer Lambda itself provides belongs to no account, so it needs no
            // `PublishLayerVersion` behind it and this emulator holds no archive of it to
            // measure: both reported sizes are the member defaults.
            patch: json!({ "Layers": ["arn:aws:lambda:::awslayer:AWSLambdaInsightsExtension"] }),
            expected: json!({
                "Layers": [{
                    "Arn": "arn:aws:lambda:::awslayer:AWSLambdaInsightsExtension",
                    "CodeSize": 0,
                    "UncompressedCodeSize": 0,
                }],
            }),
        },
        Case {
            label: "durable-config-set",
            package: Package::Zip,
            patch: json!({
                "DurableConfig": { "RetentionPeriodInDays": 30, "ExecutionTimeout": 3600 },
            }),
            expected: json!({
                "DurableConfig": { "RetentionPeriodInDays": 30, "ExecutionTimeout": 3600 },
            }),
        },
        Case {
            label: "image-overrides-set",
            package: Package::Image,
            patch: json!({
                "ImageConfig": {
                    "EntryPoint": ["/usr/bin/python3"],
                    "Command": ["app.handler"],
                    "WorkingDirectory": "/var/task",
                },
            }),
            expected: json!({
                "ImageConfigResponse": {
                    "ImageConfig": {
                        "EntryPoint": ["/usr/bin/python3"],
                        "Command": ["app.handler"],
                        "WorkingDirectory": "/var/task",
                    },
                },
            }),
        },
        Case {
            label: "no-setting-named",
            package: Package::Zip,
            patch: json!({}),
            expected: json!({}),
        },
    ]
}

/// The value of `member` in `body`, with an absent member read as `null`.
fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// Every member of a configuration body, as a map.
fn members(body: &Value) -> Map<String, Value> {
    body.as_object().cloned().unwrap_or_default()
}

#[tokio::test]
async fn a_named_setting_is_replaced_and_every_other_setting_is_left_alone() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    seed_image(&engine, IMAGE_URI).await;

    for case in cases() {
        let name = case.label;
        let create = match case.package {
            Package::Zip => create_zip_request(name),
            Package::Image => create_image_request(name),
        };
        let (status, body) = call(&router, "POST", FUNCTIONS_URI, &create).await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected 201 from CreateFunction, got {status} {body}"
        );

        // A function is not modifiable until it has finished being created, so the row
        // starts from the settled record a caller that waited would see.
        make_activation_due(&engine);
        let (status, baseline) = get_configuration(&router, name, None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from the baseline GetFunctionConfiguration, got \
             {status} {baseline}"
        );

        let (status, updated) = update_configuration(&router, name, &case.patch).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from UpdateFunctionConfiguration, got {status} \
             {updated}"
        );

        // Every member the row names carries its new value...
        for (name_of, expected) in members(&case.expected) {
            let actual = member(&updated, &name_of);
            assert_eq!(
                actual, expected,
                "case {name:?}: member {name_of} expected {expected}, got {actual}"
            );
        }

        // ...and every member it does not name is exactly what it was, bar the ones an
        // update always moves.
        for (name_of, was) in members(&baseline) {
            if case.expected.get(&name_of).is_some()
                || VOLATILE_MEMBERS.contains(&name_of.as_str())
                || (name_of == DERIVED_FROM_RUNTIME && case.expected.get("Runtime").is_some())
            {
                continue;
            }
            let actual = member(&updated, &name_of);
            assert_eq!(
                actual, was, "case {name:?}: member {name_of} was not named by the request but \
                 changed from {was} to {actual}"
            );
        }

        // A new revision is minted on every change to the configuration, and
        // `LastModified` never moves backwards (ISO-8601 sorts lexically).
        let revision = member(&updated, "RevisionId");
        assert!(
            revision.is_string() && revision != member(&baseline, "RevisionId"),
            "case {name:?}: expected a fresh RevisionId, got {revision} after \
             {}",
            member(&baseline, "RevisionId")
        );
        let last_modified = member(&updated, "LastModified");
        assert!(
            last_modified.as_str().is_some_and(|updated_at| {
                member(&baseline, "LastModified")
                    .as_str()
                    .is_some_and(|created_at| updated_at >= created_at)
            }),
            "case {name:?}: expected LastModified at or after {}, got {last_modified}",
            member(&baseline, "LastModified")
        );

        // The update is applied to the unpublished version, and it is in flight.
        assert_eq!(
            member(&updated, "Version"),
            json!("$LATEST"),
            "case {name:?}: an update must answer for the unpublished version, got {}",
            member(&updated, "Version")
        );
        assert_eq!(
            member(&updated, "FunctionArn"),
            json!(function_arn(name)),
            "case {name:?}: expected the unqualified function ARN, got {}",
            member(&updated, "FunctionArn")
        );
        assert_eq!(
            member(&updated, "State"),
            json!("Active"),
            "case {name:?}: an update leaves the function invocable, got State {}",
            member(&updated, "State")
        );
        assert_eq!(
            member(&updated, "LastUpdateStatus"),
            json!("InProgress"),
            "case {name:?}: expected LastUpdateStatus InProgress, got {}",
            member(&updated, "LastUpdateStatus")
        );

        // What the update answered is what the store holds: an immediate read reports
        // the same configuration, member for member.
        let (status, read_back) = get_configuration(&router, name, None).await;
        assert_eq!(
            (status, &read_back),
            (200, &updated),
            "case {name:?}: GetFunctionConfiguration must report what the update \
             persisted, got {status} {read_back}"
        );

        // Once the clock has made the update due, the only members that move are the
        // status the model says indicates "when the update is complete" and the reason
        // pair that reports the work that status covers: a settled update has no work in
        // flight left to give a reason for.
        make_update_due(&engine);
        let (status, settled) = get_configuration(&router, name, None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 after the update settled, got {status} {settled}"
        );
        assert_eq!(
            member(&settled, "LastUpdateStatus"),
            json!("Successful"),
            "case {name:?}: expected LastUpdateStatus Successful once the update is due, got {}",
            member(&settled, "LastUpdateStatus")
        );
        let mut expected_settled = members(&updated);
        expected_settled.insert("LastUpdateStatus".to_string(), json!("Successful"));
        expected_settled.remove("LastUpdateStatusReason");
        expected_settled.remove("LastUpdateStatusReasonCode");
        assert_eq!(
            members(&settled),
            expected_settled,
            "case {name:?}: settling the update changed more than its status: {settled}"
        );
    }
}

#[tokio::test]
async fn an_update_leaves_a_published_version_frozen() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "frozen-version";

    let (status, body) = call(&router, "POST", FUNCTIONS_URI, &create_zip_request(name)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateFunction, got {status} {body}"
    );
    make_activation_due(&engine);

    let (status, published) = publish_version(&router, name).await;
    assert_eq!(
        status, 201,
        "expected 201 from PublishVersion, got {status} {published}"
    );
    let version = member(&published, "Version");
    assert_eq!(version, json!("1"), "expected version 1, got {version}");

    let patch = json!({ "Description": "only the unpublished version moves", "MemorySize": 512 });
    let (status, updated) = update_configuration(&router, name, &patch).await;
    assert_eq!(
        status, 200,
        "expected 200 from UpdateFunctionConfiguration, got {status} {updated}"
    );

    // "These settings [...] are locked when you publish a version."
    let (status, frozen) = get_configuration(&router, name, Some("1")).await;
    assert_eq!(
        (status, &frozen),
        (200, &published),
        "the published version must report exactly what it froze, got {status} {frozen}"
    );

    // ...while the unpublished version carries the new values.
    let (status, latest) = get_configuration(&router, name, None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetFunctionConfiguration, got {status} {latest}"
    );
    assert_eq!(
        (
            member(&latest, "Description"),
            member(&latest, "MemorySize")
        ),
        (json!("only the unpublished version moves"), json!(512)),
        "the unpublished version must carry the update, got {latest}"
    );

    // A request that qualifies the name with the published version names a configuration
    // no update can touch.
    let (status, refused) =
        update_configuration(&router, &format!("{name}:1"), &json!({ "Timeout": 11 })).await;
    assert_eq!(
        status, 400,
        "expected 400 for an update of a published version, got {status} {refused}"
    );
    let (status, still_frozen) = get_configuration(&router, name, Some("1")).await;
    assert_eq!(
        (status, &still_frozen),
        (200, &published),
        "the refused update must have changed nothing, got {status} {still_frozen}"
    );
}

#[tokio::test]
async fn an_update_applies_only_at_the_revision_the_caller_read() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "revision-precondition";

    let (status, body) = call(&router, "POST", FUNCTIONS_URI, &create_zip_request(name)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateFunction, got {status} {body}"
    );
    make_activation_due(&engine);

    let (_, baseline) = get_configuration(&router, name, None).await;
    let stale = member(&baseline, "RevisionId")
        .as_str()
        .map(str::to_string)
        .expect("a created function must report a RevisionId");

    // The matching revision is accepted...
    let (status, accepted) = update_configuration(
        &router,
        name,
        &json!({ "Description": "at the read revision", "RevisionId": stale }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 for an update at the stored revision, got {status} {accepted}"
    );
    let fresh = member(&accepted, "RevisionId");
    assert_ne!(
        fresh,
        json!(stale),
        "an accepted update must mint a new revision, got {fresh}"
    );
    make_update_due(&engine);

    // ...and the one it replaced is no longer the stored one, so re-using it is the 412
    // `PreconditionFailedException` is modelled for.
    let (status, refused) = update_configuration(
        &router,
        name,
        &json!({ "Description": "at a stale revision", "RevisionId": stale }),
    )
    .await;
    assert_eq!(
        status, 412,
        "expected 412 for an update at a stale revision, got {status} {refused}"
    );

    let (status, unchanged) = get_configuration(&router, name, None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetFunctionConfiguration, got {status} {unchanged}"
    );
    assert_eq!(
        (
            member(&unchanged, "Description"),
            member(&unchanged, "RevisionId")
        ),
        (json!("at the read revision"), fresh),
        "the refused update must have changed nothing, got {unchanged}"
    );
}

#[tokio::test]
async fn a_function_being_created_or_updated_cannot_be_modified() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "in-flight";

    let (status, body) = call(&router, "POST", FUNCTIONS_URI, &create_zip_request(name)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateFunction, got {status} {body}"
    );

    // "When the code is Creating, you can't invoke or modify the function."
    let (status, refused) = update_configuration(&router, name, &json!({ "Timeout": 11 })).await;
    assert_eq!(
        status, 409,
        "expected 409 while the function is still being created, got {status} {refused}"
    );
    make_activation_due(&engine);

    let (status, first) = update_configuration(&router, name, &json!({ "Timeout": 11 })).await;
    assert_eq!(
        status, 200,
        "expected 200 from the first update, got {status} {first}"
    );

    // "During this time, you can't modify the function, but you can still invoke it."
    let (status, conflict) = update_configuration(&router, name, &json!({ "Timeout": 12 })).await;
    assert_eq!(
        status, 409,
        "expected 409 while an update is in progress, got {status} {conflict}"
    );
    let (status, unchanged) = get_configuration(&router, name, None).await;
    assert_eq!(
        (status, member(&unchanged, "Timeout")),
        (200, json!(11)),
        "the refused update must have changed nothing, got {status} {unchanged}"
    );

    // Once the first update has settled, the next one is served.
    make_update_due(&engine);
    let (status, second) = update_configuration(&router, name, &json!({ "Timeout": 12 })).await;
    assert_eq!(
        status, 200,
        "expected 200 once the first update settled, got {status} {second}"
    );
    assert_eq!(
        member(&second, "Timeout"),
        json!(12),
        "expected the second update to apply, got {second}"
    );
}

#[tokio::test]
async fn an_update_of_a_resource_that_does_not_exist_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    seed_image(&engine, IMAGE_URI).await;

    // Every documented `FunctionName` form names the same absent record, so each one is
    // the same 404 — and a request naming no stored function is refused for that before
    // any of its other members is judged, whatever else it carries.
    let oversized = "x".repeat(5_000);
    let absent: Vec<(String, Value)> = vec![
        ("no-such-function".to_string(), json!({ "Timeout": 11 })),
        (
            format!("{ACCOUNT}:function:no-such-function"),
            json!({ "Timeout": 11 }),
        ),
        (function_arn("no-such-function"), json!({ "Timeout": 11 })),
        (
            "no-such-function".to_string(),
            json!({ "Layers": ["arn:aws:lambda:us-east-1:000000000000:layer:never-published:1"] }),
        ),
        (
            "no-such-function".to_string(),
            json!({ "Environment": { "Variables": { "BIG": oversized } } }),
        ),
    ];
    for (reference, patch) in &absent {
        let (status, body) = update_configuration(&router, reference, patch).await;
        assert_eq!(
            status, 404,
            "case {reference:?} with {patch}: expected 404 for an absent function, got {status} \
             {body}"
        );
    }

    // A setting the function's package type does not admit is a 400, not a silent write.
    let zip = "package-type-zip";
    let (status, body) = call(&router, "POST", FUNCTIONS_URI, &create_zip_request(zip)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateFunction, got {status} {body}"
    );
    let image = "package-type-image";
    let (status, body) = call(&router, "POST", FUNCTIONS_URI, &create_image_request(image)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateFunction, got {status} {body}"
    );
    make_activation_due(&engine);

    let refusals = [
        (
            zip,
            json!({ "ImageConfig": { "WorkingDirectory": "/var/task" } }),
        ),
        (image, json!({ "Runtime": "python3.12" })),
        (image, json!({ "Handler": "index.handler" })),
        (
            zip,
            json!({ "Layers": ["arn:aws:lambda:us-east-1:000000000000:layer:never-published:1"] }),
        ),
    ];
    for (reference, patch) in &refusals {
        let (status, body) = update_configuration(&router, reference, patch).await;
        assert_eq!(
            status, 400,
            "case {patch}: expected 400 from UpdateFunctionConfiguration on {reference:?}, got \
             {status} {body}"
        );
    }

    // None of the refusals wrote anything: the store still holds the two functions it
    // was given, each at the revision it was created with.
    let held: Vec<String> = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .functions
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        held,
        vec![image.to_string(), zip.to_string()],
        "the refused updates changed the store, it now holds {held:?}"
    );
    for reference in [zip, image] {
        let (status, body) = get_configuration(&router, reference, None).await;
        assert_eq!(
            status, 200,
            "case {reference:?}: expected 200 from GetFunctionConfiguration, got {status} {body}"
        );
        assert_eq!(
            member(&body, "LastUpdateStatus"),
            json!("Successful"),
            "case {reference:?}: a refused update must leave no update in flight, got {body}"
        );
    }
}
