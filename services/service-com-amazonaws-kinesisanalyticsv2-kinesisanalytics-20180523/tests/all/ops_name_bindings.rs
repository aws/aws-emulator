// SPDX-License-Identifier: CC0-1.0

//! Every `crate::ops::<Name>` path of this service names exactly one item, and each
//! operation keeps its own handler.
//!
//! The rule: a name is published once under `crate::ops`, whichever generated glob
//! (`types::*` or `<snake_op>::*`) carries it, so no `crate::ops::<Name>` depends on the
//! order the ops-module generator emits those globs in. `crate::ops::<ShapeName>` names
//! the MODELLED SHAPE — the generated service declaration is expanded with
//! `types = "crate::ops"`, so that is the path the wire machinery resolves every
//! input/output shape through — and an operation's handler struct is reached at
//! `crate::ops::<snake_op>::<Op>` as well as through its glob.
//!
//! This model is one that could break the rule: `DeleteApplicationOutput` is an operation
//! AND the `<Op>Output` form of `DeleteApplication`'s output struct. It does not, because
//! that output struct carries the model's own output shape name
//! (`DeleteApplicationResponse`). The rows below span the class rather than that one
//! name: the operation whose name is at risk, the operation whose output struct yields
//! the name, two operations with no such tension (`AddApplicationOutput`,
//! `DeleteApplicationCloudWatchLoggingOption`), and every one of them driven over the
//! wire — `DeleteApplication` to a real 200 (it is implemented, so its success response
//! is serialized through its own output shape) and each of the two output operations to
//! the modelled refusal only its own handler words.

use std::any::{type_name, TypeId};
use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::{ops, EmulatedService};
use smithy_server_runtime::{MultiServiceRouter, Operation};

/// The engine the service and all of its operations are constructed from.
type Eng = engine::InMemoryEngine;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`kinesisanalytics`, per `OPERATIONS.md` "Wire protocol"): that scope is how the
/// router picks the service which claims a request. The signature itself is never
/// verified here — [`Emulator`] turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/kinesisanalytics/aws4_request, SignedHeaders=host, Signature=not-verified";

/// The `X-Amz-Target` prefix this awsJson1_1 service routes on (`OPERATIONS.md`
/// "Wire protocol": `X-Amz-Target: KinesisAnalytics_20180523.<OperationName>`).
const TARGET_PREFIX: &str = "KinesisAnalytics_20180523";

/// One expectation about which item a `crate::ops::…` path names.
struct TypeRow {
    case: &'static str,
    actual: TypeId,
    actual_name: &'static str,
    expected: TypeId,
    expected_name: &'static str,
}

/// `Actual` is the type a `crate::ops::…` path resolves to; `Expected` is the item it
/// must be.
fn row<Actual: ?Sized + 'static, Expected: ?Sized + 'static>(case: &'static str) -> TypeRow {
    TypeRow {
        case,
        actual: TypeId::of::<Actual>(),
        actual_name: type_name::<Actual>(),
        expected: TypeId::of::<Expected>(),
        expected_name: type_name::<Expected>(),
    }
}

#[test]
fn ops_paths_name_exactly_one_item_each() {
    let rows = [
        // The name at risk belongs to the operation, and to nothing else.
        row::<
            ops::DeleteApplicationOutput<Eng>,
            ops::delete_application_output::DeleteApplicationOutput<Eng>,
        >("ops::DeleteApplicationOutput is the DeleteApplicationOutput operation struct"),
        // `DeleteApplication`'s output shape is therefore published under the model's own
        // output shape name, and that is the binding the service declaration resolves.
        row::<
            <ops::delete_application::DeleteApplication<Eng> as Operation>::Output,
            ops::DeleteApplicationResponse,
        >("DeleteApplication's output shape is ops::DeleteApplicationResponse"),
        row::<
            <ops::delete_application::DeleteApplication<Eng> as Operation>::Input,
            ops::DeleteApplicationInput,
        >("DeleteApplication's input shape is ops::DeleteApplicationInput"),
        // Names with no such tension: the operation struct comes through the ops module
        // for them too.
        row::<ops::AddApplicationOutput<Eng>, ops::add_application_output::AddApplicationOutput<Eng>>(
            "ops::AddApplicationOutput is the AddApplicationOutput operation struct",
        ),
        row::<
            ops::DeleteApplicationCloudWatchLoggingOption<Eng>,
            ops::delete_application_cloud_watch_logging_option::DeleteApplicationCloudWatchLoggingOption<Eng>,
        >("ops::DeleteApplicationCloudWatchLoggingOption is that operation's struct"),
        // That operation's own input and output shapes keep their own bindings.
        row::<
            <ops::delete_application_output::DeleteApplicationOutput<Eng> as Operation>::Input,
            ops::DeleteApplicationOutputInput,
        >("DeleteApplicationOutput's input shape is ops::DeleteApplicationOutputInput"),
        row::<
            <ops::delete_application_output::DeleteApplicationOutput<Eng> as Operation>::Output,
            ops::DeleteApplicationOutputOutput,
        >("DeleteApplicationOutput's output shape is ops::DeleteApplicationOutputOutput"),
    ];

    for TypeRow {
        case,
        actual,
        actual_name,
        expected,
        expected_name,
    } in rows
    {
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected_name}, got {actual_name}"
        );
    }

    // The two operations whose names are one a prefix of the other answer with different
    // output shapes: neither glob shadowed the other's.
    assert_ne!(
        TypeId::of::<<ops::delete_application::DeleteApplication<Eng> as Operation>::Output>(),
        TypeId::of::<
            <ops::delete_application_output::DeleteApplicationOutput<Eng> as Operation>::Output,
        >(),
        "case {:?}: the two operations must keep distinct output shapes",
        "DeleteApplication vs DeleteApplicationOutput"
    );

    // Each operation reports its own wire name.
    for (case, actual, expected) in [
        (
            "DeleteApplication",
            <ops::delete_application::DeleteApplication<Eng> as Operation>::NAME,
            "DeleteApplication",
        ),
        (
            "DeleteApplicationOutput",
            <ops::delete_application_output::DeleteApplicationOutput<Eng> as Operation>::NAME,
            "DeleteApplicationOutput",
        ),
        (
            "AddApplicationOutput",
            <ops::add_application_output::AddApplicationOutput<Eng> as Operation>::NAME,
            "AddApplicationOutput",
        ),
    ] {
        assert_eq!(
            actual, expected,
            "case {case:?}: expected Operation::NAME {expected:?}, got {actual:?}"
        );
    }
}

/// A router with only this service mounted — one emulator "session".
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::new(Eng::new())));
        Self { router }
    }

    /// Invoke `operation` with a JSON body, the way an awsJson1_1 client does; returns
    /// the status, the wire error code and the parsed body.
    async fn call(&self, operation: &str, body: &str) -> (u16, String, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("http://localhost/")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let code = if header_code.is_empty() {
            parsed
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        (status, code, parsed)
    }
}

/// The application the dispatch rows below are driven against: a Managed Service for
/// Apache Flink application, so it carries no SQL configuration for either operation to
/// change. Members and constraints from `OPERATIONS.md`.
const DISPATCH_APPLICATION: &str = r#"{"ApplicationName":"collision-case","RuntimeEnvironment":"FLINK-1_20","ApplicationMode":"STREAMING","ServiceExecutionRole":"arn:aws:iam::000000000000:role/kda-service-role"}"#;

/// One VALID request per operation whose name is at risk, with the error code and the
/// message that identify the handler it reached. Valid so the boundary validator passes it
/// and the request reaches that operation's own handler; members and constraints from
/// `OPERATIONS.md`.
///
/// Both operations are implemented, so what identifies the handler is the modelled refusal
/// it words for [`DISPATCH_APPLICATION`]: the two sentences are each one handler's own.
const DISPATCH_TABLE: [(&str, &str, &str, &str); 2] = [
    (
        "DeleteApplicationOutput",
        r#"{"ApplicationName":"collision-case","CurrentApplicationVersionId":1,"OutputId":"out-1"}"#,
        "InvalidArgumentException",
        "There are no outputs with OutputId out-1 that you would like to delete",
    ),
    (
        "AddApplicationOutput",
        r#"{"ApplicationName":"collision-case","CurrentApplicationVersionId":1,"Output":{"Name":"in_app_stream","DestinationSchema":{"RecordFormatType":"JSON"}}}"#,
        "InvalidArgumentException",
        "You cannot add sql configuration Output to a Flink application.",
    ),
];

#[tokio::test]
async fn every_operation_dispatches_to_the_handler_that_bears_its_name() {
    let emulator = Emulator::new();
    let (status, _, created) = emulator
        .call("CreateApplication", DISPATCH_APPLICATION)
        .await;
    assert_eq!(
        status, 200,
        "the application the rows are driven against was not created: {status} {created:?}"
    );

    for (operation, body, expected_code, expected_message) in DISPATCH_TABLE {
        let (status, code, parsed) = emulator.call(operation, body).await;

        assert_eq!(
            status, 400,
            "case {operation:?}: expected the modelled client error, got {status} with {parsed:?}"
        );
        assert!(
            code.ends_with(expected_code),
            "case {operation:?}: expected the error code {expected_code}, got {code:?} with {parsed:?}"
        );

        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, expected_message,
            "case {operation:?}: expected the refusal only {operation}'s handler words, got \
             {message:?}"
        );
    }
}

/// `DeleteApplication` — the operation whose `<Op>Output` name is taken by an operation
/// of its own — is implemented here, so its success response is serialized through the
/// shape the ops module publishes as `crate::ops::DeleteApplicationResponse`. Driving it
/// to a 200 over the wire is the end-to-end proof that the two names did not get crossed.
#[tokio::test]
async fn the_operation_whose_output_shape_carries_a_colliding_name_still_answers() {
    let emulator = Emulator::new();

    let create = r#"{"ApplicationName":"collision-case","RuntimeEnvironment":"FLINK-1_20","ServiceExecutionRole":"arn:aws:iam::000000000000:role/kda-service-role"}"#;
    let (status, _, parsed) = emulator.call("CreateApplication", create).await;
    assert_eq!(
        status, 200,
        "case create: expected HTTP 200 from CreateApplication, got {status} with {parsed:?}"
    );
    let create_timestamp = parsed
        .pointer("/ApplicationDetail/CreateTimestamp")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case create: expected a CreateTimestamp, got {parsed:?}"));

    let (status, code, parsed) = emulator
        .call(
            "DeleteApplication",
            &format!(
                r#"{{"ApplicationName":"collision-case","CreateTimestamp":{create_timestamp}}}"#
            ),
        )
        .await;
    assert_eq!(
        status, 200,
        "case delete: expected HTTP 200 from DeleteApplication, got {status} with {parsed:?}"
    );
    assert!(
        code.is_empty(),
        "case delete: expected no error code on a success, got {code:?} with {parsed:?}"
    );

    // The modelled output of DeleteApplication declares no members (`OPERATIONS.md`), so
    // the shape serializes as an empty awsJson1_1 document.
    assert_eq!(
        parsed,
        serde_json::json!({}),
        "case delete: expected the empty DeleteApplicationResponse document, got {parsed:?}"
    );

    // And the record really is gone: the delete was served from state, not faked.
    let (status, code, parsed) = emulator
        .call(
            "DescribeApplication",
            r#"{"ApplicationName":"collision-case"}"#,
        )
        .await;
    assert_eq!(
        status, 400,
        "case describe-after-delete: expected HTTP 400, got {status} with {parsed:?}"
    );
    assert!(
        code.ends_with("ResourceNotFoundException"),
        "case describe-after-delete: expected ResourceNotFoundException, got {code:?} with {parsed:?}"
    );
}
