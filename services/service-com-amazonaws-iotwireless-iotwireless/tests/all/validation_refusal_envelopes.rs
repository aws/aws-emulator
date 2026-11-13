// SPDX-License-Identifier: CC0-1.0

//! How this service answers a request its front door refused before any handler ran, end to
//! end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch` with a
//! signed `http::Request` — so what is asserted is what an SDK client sees, not a handler
//! return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded request:
//!
//! * a refused value is reported in AWS's constraint envelope, `1 validation error detected:
//!   Value['<value>'] at '<member path>' failed to satisfy constraint: <requirement>`, whose
//!   only request-derived parts are the member path and — on the stack that quotes it — the
//!   value; the requirement (the accepted value set and its order, the numeric limit, the
//!   expression) is the service's own fixed text;
//! * two stacks front this service, and a refusal is answered in the envelope of the one that
//!   fronts the operation it addressed: a multicast-group operation in the generic envelope
//!   (the value quoted, the text in `message`), every other operation in the modelled
//!   `ValidationException` (the submitted value withheld, the text in the `Message` member the
//!   model declares);
//! * an enumerated type's admissible values are listed in the order the service's own front
//!   door lists them, which is neither the model's declaration order nor sorted;
//! * only a refusal the framework rendered is restated: a success and an error a handler
//!   returned reach the caller as their renderer wrote them.
//!
//! The wire harness is the one the destination/device suite already has: `tests/all/*.rs` are
//! modules of ONE test target, so `router`, `call` and `error_code` are shared from there
//! rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// The member a refusal the framework rendered reaches the caller in on the generic stack.
const RENDERED_MEMBER: &str = "message";

/// The member the modelled error shape declares, which the other stack reports its text in.
/// Source: `smithy_query(query="shape", shape="ValidationException")` — one member,
/// `Message`, with no `@jsonName`.
const MODELLED_MEMBER: &str = "Message";

/// The error code both stacks answer a constraint refusal with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `ExpressionType`'s admissible values as the service states them, in the order it states
/// them — recorded behaviour, and a wider set than the shipped model's enum declares.
const STATED_EXPRESSION_TYPES: &str = "[RuleName, MqttTopic, SnsTopic]";

/// `LogLevel`'s admissible values as the service states them, in the order it states them.
const STATED_LOG_LEVELS: &str = "[ERROR, INFO, DISABLED]";

/// `SupportedRfRegion`'s admissible values as the service states them, in the order it states
/// them.
const STATED_RF_REGIONS: &str = "[CN470, EU433, AU915, EU868, AS923-1, AS923-2, CN779, AS923-3, \
                                 AS923-4, KR920, IN865, RU864, US915]";

/// `DlClass`'s admissible values as the service states them, in the order it states them.
const STATED_DL_CLASSES: &str = "[ClassB, ClassC]";

/// `LoRaWANGateway$GatewayEui`'s expression, as the model writes it.
/// Source: `smithy_query(query="shape", shape="LoRaWANGateway")`.
const GATEWAY_EUI_PATTERN: &str =
    r"^(([0-9A-Fa-f]{2}-){7}|([0-9A-Fa-f]{2}:){7}|([0-9A-Fa-f]{2}\s){7}|([0-9A-Fa-f]{2}){7})([0-9A-Fa-f]{2})$";

/// `LoRaWANDeviceProfile$MaxEirp`'s published ceiling.
/// Source: `smithy_query(query="shape", shape="LoRaWANDeviceProfile")` — `@range` 0..15.
const DEVICE_PROFILE_MAX_EIRP: i64 = 15;
const DEVICE_PROFILE_MIN_EIRP: i64 = 0;

/// `LoRaWANGateway$MaxEirp`'s published ceiling, a FLOAT member.
/// Source: `smithy_query(query="shape", shape="LoRaWANGateway")` — `@range` 0..30.
const GATEWAY_MAX_EIRP: i64 = 30;

/// `MaxResults`' published ceiling on `ListMulticastGroups`.
/// Source: `smithy_query(query="operation", operation="ListMulticastGroups")` — `@range`
/// 0..250.
const LIST_MAX_RESULTS: i64 = 250;

/// AWS's envelope over the one clause of a refusal that broke one constraint.
fn envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause the stack that WITHHOLDS the submitted value states.
fn withheld(member_path: &str, requirement: &str) -> String {
    envelope(&format!(
        "Value at '{member_path}' failed to satisfy constraint: {requirement}"
    ))
}

/// The clause the stack that QUOTES the submitted value states.
fn quoted(value: &str, member_path: &str, requirement: &str) -> String {
    envelope(&format!(
        "Value '{value}' at '{member_path}' failed to satisfy constraint: {requirement}"
    ))
}

/// The requirement a value outside an enumerated set broke.
fn enum_value_set(stated: &str) -> String {
    format!("Member must satisfy enum value set: {stated}")
}

/// The requirement a value above a numeric ceiling broke.
fn at_most(bound: i64) -> String {
    format!("Member must have value less than or equal to {bound}")
}

/// The requirement a value below a numeric floor broke.
fn at_least(bound: i64) -> String {
    format!("Member must have value greater than or equal to {bound}")
}

/// The requirement a value outside an expression broke.
fn matching(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The text of a refusal body under `member`, or `None` when the body carries no such member.
fn text_under(body: &Value, member: &str) -> Option<String> {
    body.get(member).and_then(Value::as_str).map(str::to_string)
}

/// A body the model's shapes accept but for the one member under test.
fn destination_body(expression_type: Value) -> Value {
    json!({
        "Name": "RefusalDestination",
        "ExpressionType": expression_type,
        "Expression": "iot-rule",
        "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
    })
}

/// One refused create per operation on the stack that answers the MODELLED shape: the text
/// lands in the member the model declares, and no clause echoes the value the request
/// submitted.
#[tokio::test]
async fn a_refusal_outside_the_multicast_stack_states_the_modelled_shape() {
    // (case, method, target, body, the expected message)
    let rows: Vec<(&str, Method, &str, Option<Value>, String)> = vec![
        (
            "an expression type outside the published set",
            Method::POST,
            "/destinations",
            Some(destination_body(json!("NotARealExpressionType"))),
            withheld("expressionType", &enum_value_set(STATED_EXPRESSION_TYPES)),
        ),
        (
            "an expression type in the wrong case",
            Method::POST,
            "/destinations",
            Some(destination_body(json!("rulename"))),
            withheld("expressionType", &enum_value_set(STATED_EXPRESSION_TYPES)),
        ),
        (
            "a device profile EIRP above its ceiling",
            Method::POST,
            "/device-profiles",
            Some(json!({ "Name": "RefusalProfile", "LoRaWAN": { "MaxEirp": DEVICE_PROFILE_MAX_EIRP + 1 } })),
            withheld("loRaWAN.maxEirp", &at_most(DEVICE_PROFILE_MAX_EIRP)),
        ),
        (
            "a device profile EIRP below its floor",
            Method::POST,
            "/device-profiles",
            Some(json!({ "Name": "RefusalProfile", "LoRaWAN": { "MaxEirp": DEVICE_PROFILE_MIN_EIRP - 1 } })),
            withheld("loRaWAN.maxEirp", &at_least(DEVICE_PROFILE_MIN_EIRP)),
        ),
        (
            "a trace log level outside its set",
            Method::POST,
            "/network-analyzer-configurations",
            Some(json!({ "Name": "RefusalConfig", "TraceContent": { "LogLevel": "NOT_A_LEVEL" } })),
            withheld("traceContent.logLevel", &enum_value_set(STATED_LOG_LEVELS)),
        ),
        (
            "a trace log level in the wrong case",
            Method::POST,
            "/network-analyzer-configurations",
            Some(json!({ "Name": "RefusalConfig", "TraceContent": { "LogLevel": "info" } })),
            withheld("traceContent.logLevel", &enum_value_set(STATED_LOG_LEVELS)),
        ),
        (
            "a gateway EUI outside its expression",
            Method::POST,
            "/wireless-gateways",
            Some(json!({ "Name": "RefusalGateway", "LoRaWAN": { "GatewayEui": "not-an-eui" } })),
            withheld("loRaWAN.gatewayEui", &matching(GATEWAY_EUI_PATTERN)),
        ),
        (
            "an empty gateway EUI",
            Method::POST,
            "/wireless-gateways",
            Some(json!({ "Name": "RefusalGateway", "LoRaWAN": { "GatewayEui": "" } })),
            withheld("loRaWAN.gatewayEui", &matching(GATEWAY_EUI_PATTERN)),
        ),
        (
            "a gateway EIRP above its ceiling, a float member",
            Method::POST,
            "/wireless-gateways",
            Some(json!({ "Name": "RefusalGateway", "LoRaWAN": { "MaxEirp": GATEWAY_MAX_EIRP + 1 } })),
            withheld("loRaWAN.maxEirp", &at_most(GATEWAY_MAX_EIRP)),
        ),
    ];

    for (case, method, target, body, expected) in rows {
        let router = router();
        let (status, answered) = call(&router, method, target, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {case:?}: expected 400, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected the refusal code, got {answered}"
        );
        assert_eq!(
            text_under(&answered, MODELLED_MEMBER).as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the text under {MODELLED_MEMBER:?}, got {answered}"
        );
        assert!(
            answered.get(RENDERED_MEMBER).is_none(),
            "case {case:?}: the text must be carried under ONE member, got {answered}"
        );
    }
}

/// One refused call per member on the multicast-group stack: the generic envelope, the text
/// in its own member, and the value the request submitted quoted back.
#[tokio::test]
async fn a_refusal_on_the_multicast_stack_states_the_generic_envelope() {
    // (case, method, target, body, the expected message)
    let rows: Vec<(&str, Method, &str, Option<Value>, String)> = vec![
        (
            "an RF region outside its set",
            Method::POST,
            "/multicast-groups",
            Some(json!({ "Name": "RefusalGroup", "LoRaWAN": { "RfRegion": "NOT_A_REGION" } })),
            quoted(
                "NOT_A_REGION",
                "loRaWAN.rfRegion",
                &enum_value_set(STATED_RF_REGIONS),
            ),
        ),
        (
            "an RF region in the wrong case",
            Method::POST,
            "/multicast-groups",
            Some(json!({ "Name": "RefusalGroup", "LoRaWAN": { "RfRegion": "eu868" } })),
            quoted(
                "eu868",
                "loRaWAN.rfRegion",
                &enum_value_set(STATED_RF_REGIONS),
            ),
        ),
        (
            "a downlink class outside its set",
            Method::POST,
            "/multicast-groups",
            Some(json!({ "Name": "RefusalGroup", "LoRaWAN": { "DlClass": "ClassA" } })),
            quoted(
                "ClassA",
                "loRaWAN.dlClass",
                &enum_value_set(STATED_DL_CLASSES),
            ),
        ),
        (
            "a page size above its ceiling",
            Method::GET,
            "/multicast-groups?maxResults=251",
            None,
            quoted(
                &(LIST_MAX_RESULTS + 1).to_string(),
                "maxResults",
                &at_most(LIST_MAX_RESULTS),
            ),
        ),
    ];

    for (case, method, target, body, expected) in rows {
        let router = router();
        let (status, answered) = call(&router, method, target, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {case:?}: expected 400, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected the refusal code, got {answered}"
        );
        assert_eq!(
            text_under(&answered, RENDERED_MEMBER).as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the text under {RENDERED_MEMBER:?}, got {answered}"
        );
        assert!(
            answered.get(MODELLED_MEMBER).is_none(),
            "case {case:?}: the text must be carried under ONE member, got {answered}"
        );
    }
}

/// A member the request did not carry at all keeps the value half the walk reports it with —
/// the same clause on both stacks, because there is no submitted value to withhold.
#[tokio::test]
async fn an_absent_required_member_states_the_null_the_walk_reports() {
    let router = router();

    // (case, target, body, the member the request left out)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a destination with no name",
            "/destinations",
            json!({
                "ExpressionType": "RuleName",
                "Expression": "iot-rule",
                "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
            }),
            "name",
        ),
        (
            "a wireless gateway with no LoRaWAN block",
            "/wireless-gateways",
            json!({ "Name": "RefusalGateway" }),
            "loRaWAN",
        ),
    ];

    for (case, target, body, member) in rows {
        let (status, answered) = call(&router, Method::POST, target, Some(body)).await;
        let expected = envelope(&format!(
            "Value null at '{member}' failed to satisfy constraint: Member must not be null"
        ));
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {case:?}: expected 400, got {status} {answered}"
        );
        assert_eq!(
            text_under(&answered, MODELLED_MEMBER).as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the absent member stated with its null, got {answered}"
        );
    }
}

/// A response no refusal renderer wrote reaches the caller as its renderer wrote it: a create
/// still succeeds and reads back on both stacks, and an error a HANDLER returned keeps the
/// modelled member it was serialized with.
#[tokio::test]
async fn a_response_the_refusal_layer_does_not_own_is_answered_unchanged() {
    let router = router();

    // A create on the stack that answers the modelled shape, read back through its reader.
    let (status, created) = call(
        &router,
        Method::POST,
        "/destinations",
        Some(destination_body(json!("RuleName"))),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "expected 201 from CreateDestination, got {status} {created}"
    );
    let name = created["Name"]
        .as_str()
        .unwrap_or_else(|| panic!("the created destination must carry a Name: {created}"))
        .to_string();
    let (status, fetched) = call(&router, Method::GET, &format!("/destinations/{name}"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "expected the destination to read back, got {status} {fetched}"
    );
    assert_eq!(
        fetched["ExpressionType"],
        json!("RuleName"),
        "expected the stored expression type, got {fetched}"
    );

    // A create on the multicast stack, which the same wrapper serves, read back too.
    let (status, group) = call(
        &router,
        Method::POST,
        "/multicast-groups",
        Some(json!({ "Name": "KeptGroup", "LoRaWAN": { "RfRegion": "US915" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "expected 201 from CreateMulticastGroup, got {status} {group}"
    );
    let id = group["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("the created group must carry an Id: {group}"))
        .to_string();
    let (status, fetched) = call(
        &router,
        Method::GET,
        &format!("/multicast-groups/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "expected the group to read back, got {status} {fetched}"
    );

    // An error a handler returned: the modelled 404, serialized from the model's own shape.
    let (status, missing) = call(&router, Method::GET, "/destinations/NoSuchDestination", None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "expected 404 for an unknown destination, got {status} {missing}"
    );
    assert_eq!(
        error_code(&missing),
        "ResourceNotFoundException",
        "expected the modelled not-found code, got {missing}"
    );
    assert_eq!(
        text_under(&missing, MODELLED_MEMBER).as_deref(),
        Some(service_com_amazonaws_iotwireless_iotwireless::state::DESTINATION_NOT_FOUND_MESSAGE),
        "expected the handler's own sentence under {MODELLED_MEMBER:?}, got {missing}"
    );
}

/// The sets the refusals state hold exactly the values the model declares — plus the one
/// value this service publishes beyond its shipped enum — so a stated set is a reordering of
/// the model's and not a list that drifted from it.
#[tokio::test]
async fn a_stated_value_set_holds_the_values_the_model_declares() {
    // (case, the stated set, the values the model declares, the values the service adds)
    let rows: Vec<(&str, &str, &[&str], &[&str])> = vec![
        (
            "the destination expression types",
            STATED_EXPRESSION_TYPES,
            service_com_amazonaws_iotwireless_iotwireless::ops::types::EXPRESSION_TYPE_VALUES,
            &["SnsTopic"],
        ),
        (
            "the trace log levels",
            STATED_LOG_LEVELS,
            service_com_amazonaws_iotwireless_iotwireless::ops::types::LOG_LEVEL_VALUES,
            &[],
        ),
        (
            "the supported RF regions",
            STATED_RF_REGIONS,
            service_com_amazonaws_iotwireless_iotwireless::ops::types::SUPPORTED_RF_REGION_VALUES,
            &[],
        ),
        (
            "the downlink classes",
            STATED_DL_CLASSES,
            service_com_amazonaws_iotwireless_iotwireless::ops::types::DL_CLASS_VALUES,
            &[],
        ),
    ];

    for (case, stated, declared, published) in rows {
        let listed: Vec<&str> = stated
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(str::trim)
            .collect();
        let mut sorted = listed.clone();
        sorted.sort_unstable();
        let mut expected: Vec<&str> = declared.iter().chain(published.iter()).copied().collect();
        expected.sort_unstable();
        assert_eq!(
            sorted, expected,
            "case {case:?}: expected the stated set to hold the declared values, got {listed:?}"
        );
    }
}

/// The refusals are stated through the router this suite drives, so the wrapper that chooses
/// the envelope must be reachable for every operation the model binds.
#[tokio::test]
async fn an_unroutable_request_is_still_answered_by_the_framework() {
    let router: MultiServiceRouter = router();
    let (status, answered) = call(&router, Method::GET, "/not-a-modelled-resource", None).await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "expected a failure for a request naming no modelled operation, got {status} {answered}"
    );
    assert!(
        text_under(&answered, MODELLED_MEMBER).is_none(),
        "expected the framework's own answer for an unroutable request, got {answered}"
    );
}
