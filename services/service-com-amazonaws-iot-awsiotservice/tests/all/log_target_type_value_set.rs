// SPDX-License-Identifier: CC0-1.0

//! The value set a log target TYPE is held to, and the set a refusal of one advertises,
//! end to end over HTTP.
//!
//! The rule under test: every operation that carries a `LogTargetType` admits every value
//! of that type, and a string outside it is refused (400) with the WHOLE set listed in the
//! clause — stated in the order the front door walks its table of the admissible values,
//! which is derived from the values themselves
//! (`service_common::validation_message::hashed_enum_order`). The set is the type's, so it
//! is the same set at whichever operation carried it and at whatever depth: in the query
//! string of `DeleteV2LoggingLevel` and `ListV2LoggingLevels`, and inside the `logTarget`
//! structure of `SetV2LoggingLevel`.
//!
//! The value set is wider than the published model snapshot declares, so the shapes that
//! carry it are declared from this crate's overlay model (`smithy-model/local-additions.json`,
//! see `smithy-model/LOCAL-ADDITIONS.md`), and `LOG_TARGET_TYPE_VALUES` is its set.
//! `tests/all/v2_logging_options/logging_levels.rs` drives every value of it through the
//! writer, the reader and the remover; this suite is about the SET itself — that no value of
//! it is refused and that a refusal advertises all of it.
//!
//! The rows differ along what the rule generalises over: which value is carried (every one
//! the type admits), which operation carries it, and — for the refusals — the FORM a string
//! outside the set takes: another case of an admitted value, a near-miss spelling, a value
//! of another enum of this model, and a plainly absent type.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::LOG_TARGET_TYPE_VALUES;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::validation_message::hashed_enum_order;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. The levels are account-wide, so
/// no assertion depends on it.
const REGION: &str = "us-east-2";

/// A role ARN the account's V2 logging options may name: the levels OVERRIDE those options,
/// so the account has to have them before a level can be set.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/iot-logs";

/// The error this service refuses a request whose input breaks a modelled constraint with
/// (`@httpError 400`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The values the type admits, in the order its declaration states them.
///
/// SOURCE: `com.amazonaws.iot#LogTargetType` in this crate's overlay model
/// (`smithy-model/local-additions.json`). The ORDER matters here and only here: two of
/// these values share a bucket of the table a refusal lists the set from, and a chain is
/// read from the head each value was pushed onto, so which of the two is stated first
/// follows the declaration. [`the_rows_are_the_admitted_target_types`] holds this list to
/// the set this crate re-exports.
const DECLARED_TARGET_TYPES: [&str; 6] = [
    "DEFAULT",
    "THING_GROUP",
    "CLIENT_ID",
    "SOURCE_IP",
    "PRINCIPAL_ID",
    "DEVICE_DEFENDER",
];

/// Strings that name no admitted log target type, one per form such a string takes.
const UNADMITTED_TARGET_TYPES: [&str; 4] = [
    // Another case of an admitted value.
    "default",
    // A near-miss spelling of one.
    "THINGGROUP",
    // A value of another enum of this model (a `LogLevel`).
    "DISABLED",
    // A type the service has no notion of at all.
    "NOT_A_REAL_TYPE",
];

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn dispatch(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}{query}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The credential scope names the `iot` signing name the model's
            // `aws.auth#sigv4` trait declares, which is what routes the request.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the v2-logging-level request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}{query} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, body)
    }

    /// Give the account V2 logging options, which is what a per-target level overrides.
    async fn configure_logging(&self) {
        let (status, body) = self
            .dispatch(
                "POST",
                "/v2LoggingOptions",
                "",
                Some(&json!({ "roleArn": ROLE_ARN, "defaultLogLevel": "INFO" })),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: expected SetV2LoggingOptions to answer 200, got {status} body {body}"
        );
    }
}

/// A target name of the FORM a named target type addresses. The account-wide `DEFAULT`
/// target names no resource, so it carries none.
fn target_name_for(target_type: &str) -> Option<&'static str> {
    (target_type != "DEFAULT").then_some("suite-target")
}

/// The `message` of a refusal body, or a panic naming `case`.
fn message_of(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a refusal message, got {body}"))
        .to_string()
}

/// The error code a refusal carries, as this service's restJson1 envelope states it.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .or_else(|| body["code"].as_str())
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The clause a value outside the type's set is refused with, for the member it sat at.
fn expected_clause(member: &str, value: &str) -> String {
    let listed = hashed_enum_order(&DECLARED_TARGET_TYPES).join(", ");
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{listed}]"
    )
}

#[test]
fn the_rows_are_the_admitted_target_types() {
    let declared: std::collections::BTreeSet<&str> =
        DECLARED_TARGET_TYPES.iter().copied().collect();
    let admitted: std::collections::BTreeSet<&str> =
        LOG_TARGET_TYPE_VALUES.iter().copied().collect();
    assert_eq!(
        declared, admitted,
        "case \"the admitted LogTargetType values\": expected {admitted:?}, got {declared:?}"
    );
    for value in UNADMITTED_TARGET_TYPES {
        assert!(
            !admitted.contains(value),
            "case {value:?}: a negative row must name no admitted value, got {admitted:?}"
        );
    }
}

#[tokio::test]
async fn every_admitted_target_type_is_served_by_every_operation_that_carries_one() {
    // Rows: every value the type admits, through each of the three operations that carry
    // it — the writer, the reader filtered by that type, and the remover.
    for target_type in LOG_TARGET_TYPE_VALUES {
        let emulator = Emulator::new();
        emulator.configure_logging().await;
        let target_name = target_name_for(target_type);

        let mut log_target = json!({ "targetType": target_type });
        if let Some(target_name) = target_name {
            log_target["targetName"] = json!(target_name);
        }
        let (status, body) = emulator
            .dispatch(
                "POST",
                "/v2LoggingLevel",
                "",
                Some(&json!({ "logTarget": log_target, "logLevel": "INFO" })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {target_type:?}: SetV2LoggingLevel must admit the type, got {status} {body}"
        );

        // The reader filtered by that type reports what the writer persisted for it.
        let (status, body) = emulator
            .dispatch(
                "GET",
                "/v2LoggingLevel",
                &format!("?targetType={target_type}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {target_type:?}: ListV2LoggingLevels must admit the type, got {status} {body}"
        );
        let reported = body["logTargetConfigurations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {target_type:?}: expected logTargetConfigurations, got {body}")
            })
            .clone();
        assert_eq!(
            reported,
            vec![json!({ "logTarget": log_target, "logLevel": "INFO" })],
            "case {target_type:?}: the listing must report the level written for the type, got \
             {body}"
        );

        let query = match target_name {
            Some(target_name) => {
                format!("?targetType={target_type}&targetName={target_name}")
            }
            None => format!("?targetType={target_type}&targetName="),
        };
        let (status, body) = emulator
            .dispatch("DELETE", "/v2LoggingLevel", &query, None)
            .await;
        assert_eq!(
            status, 200,
            "case {target_type:?}: DeleteV2LoggingLevel must admit the type, got {status} {body}"
        );
        let (status, body) = emulator
            .dispatch(
                "GET",
                "/v2LoggingLevel",
                &format!("?targetType={target_type}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {target_type:?}: expected 200 from the read-back, got {status} {body}"
        );
        let left = body["logTargetConfigurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            left, 0,
            "case {target_type:?}: the delete must take the one level away, got {body}"
        );
    }
}

#[tokio::test]
async fn a_type_outside_the_set_is_refused_with_the_whole_set_listed() {
    let emulator = Emulator::new();
    emulator.configure_logging().await;

    // Rows: every form a string outside the set takes, at each of the three members the
    // type is carried at. The member named is the one the request spelled it at, and the
    // set listed is the type's own, whichever member carried it.
    for value in UNADMITTED_TARGET_TYPES {
        let carriers: [(&str, &str, &str, Option<Value>); 3] = [
            (
                "DeleteV2LoggingLevel",
                "targetType",
                "delete",
                None,
            ),
            ("ListV2LoggingLevels", "targetType", "list", None),
            (
                "SetV2LoggingLevel",
                "logTarget.targetType",
                "set",
                Some(json!({
                    "logTarget": { "targetType": value, "targetName": "suite-target" },
                    "logLevel": "INFO",
                })),
            ),
        ];
        for (operation, member, kind, body) in carriers {
            let case = format!("{value:?} at {operation}");
            let (status, answer) = match kind {
                "delete" => {
                    emulator
                        .dispatch(
                            "DELETE",
                            "/v2LoggingLevel",
                            &format!("?targetType={value}&targetName=suite-target"),
                            None,
                        )
                        .await
                }
                "list" => {
                    emulator
                        .dispatch(
                            "GET",
                            "/v2LoggingLevel",
                            &format!("?targetType={value}"),
                            None,
                        )
                        .await
                }
                _ => {
                    emulator
                        .dispatch("POST", "/v2LoggingLevel", "", body.as_ref())
                        .await
                }
            };
            assert_eq!(
                status, 400,
                "case {case:?}: a value outside the type's set must be refused, got {status} \
                 {answer}"
            );
            assert_eq!(
                error_code(&answer),
                INVALID_REQUEST_EXCEPTION,
                "case {case:?}: expected {INVALID_REQUEST_EXCEPTION}, got {answer}"
            );
            let message = message_of(&case, &answer);
            assert_eq!(
                message,
                expected_clause(member, value),
                "case {case:?}: expected the whole set listed, got {message:?}"
            );
        }
    }
}
