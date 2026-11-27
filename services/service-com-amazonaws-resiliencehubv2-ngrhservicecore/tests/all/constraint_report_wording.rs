// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request the framework refused before any handler ran.
//!
//! A request value one of the rules its schema declares refused is reported as the
//! validator's own REPORT — the bracketed list a JSON-schema layer states, one finding per
//! rule the request broke, worded for the CLASS of rule (a regular expression, a length
//! bound, a numeric range, an enumerated set, a choice of alternatives), each naming the
//! requirement from the MODEL and the value from the REQUEST — and not the common AWS
//! framework form that counts the violations and names the member path.
//!
//! The suites are TABLES: a row is one input of the class the rule covers — the operation,
//! the member, the rule it broke, the form of the value, how many rules one request broke —
//! and the recorded `CreateSystem`, `CreateAssertion`, `CreatePolicy` and
//! `CreateInputSource` cases are each ONE of them. The last suite is the other half of the
//! rule: the two violations this layer states no finding about keep the framework's own
//! wording, so one refusal is never reported half in one voice and half in the other.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch`), so the refusal
//! is rendered by the same seam a client reaches.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`). Signature verification is off for these tests, so the value
/// only has to parse.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY00000000/20260101/us-east-1/resiliencehub/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// `EntityName`'s `@pattern`, transcribed from the model
/// (`smithy_query(query="operation",operation="CreateSystem")`, whose `name` targets it;
/// `CreateService$name` and `CreatePolicy$name` target the same shape).
const ENTITY_NAME_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$";

/// `EntityLabel`'s `@pattern`, from
/// `smithy_query(query="operation",operation="CreateServiceFunction")`. Close to
/// `EntityName`'s and NOT the same: this one admits a space inside the label.
const ENTITY_LABEL_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9 _\-]{1,59}$";

/// `ClientToken`'s `@pattern`, from the same two operations' `clientToken`.
const CLIENT_TOKEN_PATTERN: &str = r"^[A-Za-z0-9_.-]{1,64}$";

/// `AccountId`'s `@pattern`, from
/// `smithy_query(query="operation",operation="ListServices")`.
const ACCOUNT_ID_PATTERN: &str = r"^[0-9]{12}$";

/// `OuId`'s `@pattern`, from the same operation.
const OU_ID_PATTERN: &str = r"^ou-[a-z0-9]{4,32}-[a-z0-9]{8,32}$";

/// `Arn`'s `@pattern`, from the same operation (`ListServices$policyArn`); every member of
/// this model that carries an ARN targets this one shape.
const ARN_PATTERN: &str = r"^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):[A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:([a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]):[0-9]{12}:[A-Za-z0-9/][A-Za-z0-9:_/+.-]{0,1023}$";

/// An ARN-shaped value whose Region segment is not a Region, so it breaks `Arn`'s
/// `@pattern` while still clearing that shape's `@length` minimum of 31 — the refusal then
/// holds the pattern violation alone.
const MALFORMED_ARN: &str = "arn:aws:resiliencehub:not_a_region:000000000000:service/x";

/// A well-formed `Arn` for a resource nothing was ever stored under: enough to satisfy the
/// member's own constraints so a SIBLING member's pattern violation is the only one.
const WELL_FORMED_ARN: &str =
    "arn:aws:resiliencehub:us-east-1:000000000000:service/11111111-1111-4111-8111-111111111111";

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService::new(engine),
    );
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", SIGNED)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One entry of the report: the expression the value had to match and the value that did
/// not match it.
fn entry(pattern: &str, value: &str) -> String {
    format!("ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"")
}

/// The whole report: the entries inside ONE pair of brackets.
fn report(entries: &[String]) -> String {
    format!("[{}]", entries.join(", "))
}

/// One row of the one-member table: a request carrying one value outside the `@pattern`
/// of the member it was sent for.
struct PatternCase {
    label: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
    /// The expression that member's value had to satisfy.
    pattern: &'static str,
    /// The value that did not satisfy it.
    value: &'static str,
}

/// RULE: a value outside the modelled `@pattern` of the member it was sent for is refused
/// 400 `ValidationException` with this service's constraint report — `[ECMA 262 regex
/// "<the member's @pattern>" does not match input string "<the submitted value>"]` —
/// whichever operation and whichever member carried it.
#[tokio::test]
async fn a_value_outside_a_modelled_pattern_is_refused_with_the_constraint_report() {
    let router = emulator();

    let cases = vec![
        PatternCase {
            label: "CreateSystem name holding spaces and punctuation",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "bad name has spaces!"})),
            pattern: ENTITY_NAME_PATTERN,
            value: "bad name has spaces!",
        },
        PatternCase {
            label: "CreateSystem name whose first character is not alphanumeric",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "_leading-underscore"})),
            pattern: ENTITY_NAME_PATTERN,
            value: "_leading-underscore",
        },
        PatternCase {
            label: "CreateService name holding dots",
            method: "POST",
            uri: "/v2/create-service",
            body: Some(json!({
                "name": "name.with.dots",
                "regions": ["us-east-1"],
                "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
            })),
            pattern: ENTITY_NAME_PATTERN,
            value: "name.with.dots",
        },
        PatternCase {
            label: "CreatePolicy name holding a space",
            method: "POST",
            uri: "/v2/create-policy",
            body: Some(json!({"name": "policy name"})),
            pattern: ENTITY_NAME_PATTERN,
            value: "policy name",
        },
        PatternCase {
            // `EntityLabel` admits the space `EntityName` refuses, so the entry has to
            // state the expression of the member that was sent, not one of its siblings.
            label: "CreateServiceFunction name holding an asterisk",
            method: "POST",
            uri: "/v2/create-service-function",
            body: Some(json!({
                "name": "function*label",
                "serviceArn": WELL_FORMED_ARN,
                "criticality": "PRIMARY",
            })),
            pattern: ENTITY_LABEL_PATTERN,
            value: "function*label",
        },
        PatternCase {
            label: "CreateSystem clientToken holding a space",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "a-well-formed-name", "clientToken": "bad token"})),
            pattern: CLIENT_TOKEN_PATTERN,
            value: "bad token",
        },
        PatternCase {
            label: "ListServices accountId of twelve non-digits",
            method: "GET",
            uri: "/v2/list-services?accountId=notanaccount",
            body: None,
            pattern: ACCOUNT_ID_PATTERN,
            value: "notanaccount",
        },
        PatternCase {
            label: "ListServices ouId in upper case",
            method: "GET",
            uri: "/v2/list-services?ouId=ou-ABCD-EFGHIJKL",
            body: None,
            pattern: OU_ID_PATTERN,
            value: "ou-ABCD-EFGHIJKL",
        },
        PatternCase {
            label: "ListServices policyArn whose Region segment is not a Region",
            method: "GET",
            uri: "/v2/list-services?policyArn=arn:aws:resiliencehub:not_a_region:000000000000:service/x",
            body: None,
            pattern: ARN_PATTERN,
            value: MALFORMED_ARN,
        },
        PatternCase {
            label: "GetSystem systemArn whose Region segment is not a Region",
            method: "GET",
            uri: "/v2/get-system?systemArn=arn:aws:resiliencehub:not_a_region:000000000000:service/x",
            body: None,
            pattern: ARN_PATTERN,
            value: MALFORMED_ARN,
        },
    ];

    for case in cases {
        let (status, body) = call(&router, case.method, case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} and {body:?}",
            case.label
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {:?}: expected ValidationException, got {code:?}",
            case.label
        );
        let expected = report(&[entry(case.pattern, case.value)]);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            case.label
        );
    }
}

/// Characters `EntityName`'s `@pattern` does not admit after its first character — read
/// off the expression itself, whose tail class is `[A-Za-z0-9_\-]`, so everything outside
/// that class is a row. None of them is the character the recorded case carried.
const CHARACTERS_OUTSIDE_THE_NAME_PATTERN: [char; 10] =
    ['.', '/', '@', '*', '#', ':', ',', '+', '=', '\t'];

/// RULE: the entry quotes the value the request carried, VERBATIM — the rule generalises
/// over the value, so one row per character the expression does not admit, each inside an
/// otherwise well-formed name.
#[tokio::test]
async fn the_report_quotes_the_value_the_request_carried() {
    let router = emulator();

    for character in CHARACTERS_OUTSIDE_THE_NAME_PATTERN {
        let name = format!("ab{character}cd");
        let (status, body) = call(
            &router,
            "POST",
            "/v2/create-system",
            Some(json!({"name": name})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected 400, got {status} and {body:?}"
        );
        let expected = report(&[entry(ENTITY_NAME_PATTERN, &name)]);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case name={name:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: two members outside their own patterns are reported as two entries inside the ONE
/// pair of brackets, each naming its own member's expression and value.
#[tokio::test]
async fn two_members_outside_their_patterns_are_reported_as_one_bracketed_list() {
    let router = emulator();

    let (status, body) = call(
        &router,
        "GET",
        "/v2/list-services?accountId=notanaccount&ouId=ou-ABCD-EFGHIJKL",
        None,
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status} and {body:?}");
    let actual = message(&body);
    let account = entry(ACCOUNT_ID_PATTERN, "notanaccount");
    let ou = entry(OU_ID_PATTERN, "ou-ABCD-EFGHIJKL");
    // The entries come out in the order the validating walk found the members, which is
    // the model's own member order; both orderings are one bracketed list of the same two
    // findings.
    let orderings = [
        report(&[account.clone(), ou.clone()]),
        report(&[ou, account]),
    ];
    assert!(
        orderings.contains(&actual),
        "case two members outside their patterns: expected one of {orderings:?}, got {actual:?}"
    );
}

/// One row of the declining table: a request breaking a constraint this layer states no
/// finding for.
struct OtherClassCase {
    label: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
}

/// RULE: a refusal holding one violation this layer states no finding ABOUT keeps the
/// framework's own wording, whole — a message half in one voice and half in the other is
/// worse than one consistent voice. Those are: a `@required` member the request left out
/// (a JSON Schema states the missing properties as the SCHEMA spells them, which the
/// refusal seam cannot build), and a MAP below its modelled entry minimum, which no
/// sentence of that layer states.
#[tokio::test]
async fn a_violation_this_layer_has_no_finding_for_keeps_the_framework_wording() {
    let router = emulator();

    let cases = vec![
        OtherClassCase {
            label: "a required member the request left out",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"description": "no name at all"})),
        },
        OtherClassCase {
            label: "a second required member the request left out",
            method: "POST",
            uri: "/v2/create-assertion",
            body: Some(json!({"serviceArn": WELL_FORMED_ARN})),
        },
        OtherClassCase {
            label: "a map below its modelled entry minimum",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "a-well-formed-name", "tags": {}})),
        },
    ];

    for case in cases {
        let (status, body) = call(&router, case.method, case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} and {body:?}",
            case.label
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {:?}: expected ValidationException, got {code:?}",
            case.label
        );
        let actual = message(&body);
        assert!(
            actual.contains("validation error") && !actual.starts_with('['),
            "case {:?}: expected the framework's validation wording, got {actual:?}",
            case.label
        );
    }
}

/// The finding for a string shorter than the `@length` minimum of the member it was sent
/// for: the string, its length, and that minimum.
fn too_short(value: &str, length: usize, min: i64) -> String {
    format!("string \"{value}\" is too short (length: {length}, required minimum: {min})")
}

/// The finding for a string longer than that member's `@length` maximum.
fn too_long(value: &str, length: usize, max: i64) -> String {
    format!("string \"{value}\" is too long (length: {length}, maximum allowed: {max})")
}

/// The finding for a MAP carrying more entries than its `@length` maximum admits: the
/// entry count the request carried and that maximum. A map is the request's own JSON
/// OBJECT, so its ceiling is reported as a property count and no value is quoted.
fn too_many_properties(found: usize, max: i64) -> String {
    format!("object has too many properties (found {found} but schema requires at most {max})")
}

/// The finding for a number above the `@range` maximum of the member it was sent for.
fn above_maximum(value: &str, max: &str) -> String {
    format!("numeric instance is greater than the required maximum (maximum: {max}, found: {value})")
}

/// The finding for a number below that member's `@range` minimum.
fn below_minimum(value: &str, min: &str) -> String {
    format!("numeric instance is lower than the required minimum (minimum: {min}, found: {value})")
}

/// The finding for a value outside the set an enumerated type declares: the value, then
/// every value the type admits, in the order the model declares them.
fn not_in_enum(value: &str, permitted: &[&str]) -> String {
    let quoted: Vec<String> = permitted
        .iter()
        .map(|admitted| format!("\"{admitted}\""))
        .collect();
    format!(
        "instance value (\"{value}\") not found in enum (possible values: [{}])",
        quoted.join(",")
    )
}

/// The finding the validator states about ITSELF for a property whose declared type carries
/// a `format` attribute it implements no check for.
fn unsupported_format(attribute: &str) -> String {
    format!("format attribute \"{attribute}\" not supported")
}

/// The finding for an instance validated against a CHOICE of schemas that matched other
/// than exactly one of them.
fn matched_schemas(matched: usize, alternatives: usize) -> String {
    format!("instance failed to match exactly one schema (matched {matched} out of {alternatives})")
}

/// How many alternatives the service's published schema offers for `ResourceConfiguration`,
/// as the recorded refusal of a `resourceConfiguration` that provided no field reports it —
/// seven, where the model lists five members.
const RESOURCE_CONFIGURATION_ALTERNATIVES: usize = 7;

/// The values `AssessmentStatus` admits, in the order the model declares them.
///
/// SOURCE: `smithy_query(query="shape", shape="AssessmentStatus")`.
const ASSESSMENT_STATUS_VALUES: [&str; 5] = [
    "NOT_STARTED",
    "PENDING",
    "IN_PROGRESS",
    "FAILED",
    "SUCCESS",
];

/// One row of the report table: a request breaking one or more rules of its schema, and the
/// findings the report is to list, in order.
struct ReportCase {
    label: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
    findings: Vec<String>,
}

/// Drive every row of a report table and assert the whole report.
async fn assert_reports(router: &MultiServiceRouter, cases: Vec<ReportCase>) {
    for case in cases {
        let (status, body) = call(router, case.method, case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} and {body:?}",
            case.label
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {:?}: expected ValidationException, got {code:?}",
            case.label
        );
        let expected = report(&case.findings);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            case.label
        );
    }
}

/// RULE: a string outside the `@length` bounds of the member it was sent for is reported as
/// that layer's finding for its class — the string, the length it had and the bound it
/// broke — and a value breaking its member's `@length` AND its `@pattern` is reported as
/// both findings inside the one bracketed list. A MAP outside its own `@length` is the
/// request's JSON object, so its ceiling is reported as a property count.
///
/// The rows span the bound that was broken (minimum, maximum), the member (a body member, a
/// query parameter, a map), and whether the value broke one rule or two.
#[tokio::test]
async fn a_string_outside_its_modelled_length_is_reported_as_the_schema_finding() {
    let router = emulator();
    // `TagMap` is `len 1..50` (`smithy_query(query="shape", shape="TagMap")`), so one entry
    // past its maximum is a map outside its length.
    let too_many_tags: Value = (0..51)
        .map(|index| (format!("key-{index}"), Value::from("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();

    let cases = vec![
        ReportCase {
            // `AssertionText` is `len 1..1000` and carries no `@pattern`, so an empty text
            // breaks exactly one rule.
            label: "CreateAssertion text at zero length",
            method: "POST",
            uri: "/v2/create-assertion",
            body: Some(json!({"serviceArn": WELL_FORMED_ARN, "text": ""})),
            findings: vec![too_short("", 0, 1)],
        },
        ReportCase {
            label: "CreateSystem name one character below EntityName's minimum",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "a"})),
            findings: vec![too_short("a", 1, 2), entry(ENTITY_NAME_PATTERN, "a")],
        },
        ReportCase {
            label: "ListServices accountId one digit past AccountId's maximum",
            method: "GET",
            uri: "/v2/list-services?accountId=0123456789012",
            body: None,
            findings: vec![
                too_long("0123456789012", 13, 12),
                entry(ACCOUNT_ID_PATTERN, "0123456789012"),
            ],
        },
        ReportCase {
            label: "ListServices ouId below OuId's minimum",
            method: "GET",
            uri: "/v2/list-services?ouId=ou-abcd",
            body: None,
            findings: vec![too_short("ou-abcd", 7, 16), entry(OU_ID_PATTERN, "ou-abcd")],
        },
        ReportCase {
            label: "CreateSystem tags one entry past TagMap's maximum",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "a-well-formed-name", "tags": too_many_tags})),
            findings: vec![too_many_properties(51, 50)],
        },
    ];

    assert_reports(&router, cases).await;
}

/// RULE: a number outside the `@range` of the member it was sent for is reported as the
/// finding for its class — the bound the model declares and the value the request carried —
/// beside, for a member of the request BODY, the finding the validator states about the
/// `format` attribute that member's declared type carries and it implements no check for: a
/// fractional member's ahead of the value's finding, an integral member's after it.
///
/// The rows span the bound (minimum, maximum), the member's declared type (a `double` and
/// an `integer`, which owe different attributes in different positions) and where the
/// request carries it (a body property, which owes a `format` finding, and a query
/// parameter, which is no property of the body schema and owes none).
#[tokio::test]
async fn a_number_outside_its_modelled_range_is_reported_as_the_schema_finding() {
    let router = emulator();

    let cases = vec![
        ReportCase {
            label: "CreatePolicy availabilitySlo.target above AvailabilitySlo's maximum",
            method: "POST",
            uri: "/v2/create-policy",
            body: Some(json!({"name": "slo-too-high", "availabilitySlo": {"target": 100.5}})),
            findings: vec![unsupported_format("double"), above_maximum("100.5", "100")],
        },
        ReportCase {
            label: "CreatePolicy availabilitySlo.target below AvailabilitySlo's minimum",
            method: "POST",
            uri: "/v2/create-policy",
            body: Some(json!({"name": "slo-too-low", "availabilitySlo": {"target": -0.5}})),
            findings: vec![unsupported_format("double"), below_minimum("-0.5", "0")],
        },
        ReportCase {
            label: "CreatePolicy multiAz.rtoInMinutes below its minimum",
            method: "POST",
            uri: "/v2/create-policy",
            body: Some(json!({"name": "rto-negative", "multiAz": {"rtoInMinutes": -1}})),
            findings: vec![below_minimum("-1", "0"), unsupported_format("int32")],
        },
        ReportCase {
            label: "CreatePolicy multiRegion.rpoInMinutes below its minimum",
            method: "POST",
            uri: "/v2/create-policy",
            body: Some(json!({"name": "rpo-negative", "multiRegion": {"rpoInMinutes": -5}})),
            findings: vec![below_minimum("-5", "0"), unsupported_format("int32")],
        },
        ReportCase {
            label: "CreatePolicy dataRecovery.timeBetweenBackupsInMinutes below its minimum",
            method: "POST",
            uri: "/v2/create-policy",
            body: Some(json!({
                "name": "backups-negative",
                "dataRecovery": {"timeBetweenBackupsInMinutes": -1},
            })),
            findings: vec![below_minimum("-1", "0"), unsupported_format("int32")],
        },
        ReportCase {
            label: "ListServices maxResults below MaxResults' minimum",
            method: "GET",
            uri: "/v2/list-services?maxResults=0",
            body: None,
            findings: vec![below_minimum("0", "1")],
        },
        ReportCase {
            label: "ListServices maxResults above MaxResults' maximum",
            method: "GET",
            uri: "/v2/list-services?maxResults=101",
            body: None,
            findings: vec![above_maximum("101", "100")],
        },
    ];

    assert_reports(&router, cases).await;

    // Each published bound is INSIDE the range, so a value AT it is served rather than
    // reported: the minute targets are `@range` 0.., and `MaxResults` is 1..100.
    // `AvailabilitySlo$target` is `@range` 0..100 as well, but the service offers only
    // discrete availability levels inside that range, so the values served at that member
    // are those levels (the lowest and the highest of them here; the whole set, and the
    // refusal of a value outside it, is pinned in `availability_slo_allowed_targets.rs`).
    for (case, method, uri, body) in [
        (
            "the lowest availability SLO target the service offers",
            "POST",
            "/v2/create-policy",
            Some(json!({"name": "slo-lowest-offered", "availabilitySlo": {"target": 99.9}})),
        ),
        (
            "the highest availability SLO target the service offers",
            "POST",
            "/v2/create-policy",
            Some(json!({"name": "slo-highest-offered", "availabilitySlo": {"target": 99.99}})),
        ),
        (
            "the minute targets at their minimum",
            "POST",
            "/v2/create-policy",
            Some(json!({
                "name": "minutes-at-minimum",
                "multiAz": {"rtoInMinutes": 0, "rpoInMinutes": 0},
                "dataRecovery": {"timeBetweenBackupsInMinutes": 0},
            })),
        ),
        (
            "maxResults at its minimum",
            "GET",
            "/v2/list-services?maxResults=1",
            None,
        ),
        (
            "maxResults at its maximum",
            "GET",
            "/v2/list-services?maxResults=100",
            None,
        ),
    ] {
        let (status, body) = call(&router, method, uri, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value at a published bound must be served, got {status} and \
             {body:?}"
        );
    }
}

/// RULE: a value outside the set an enumerated type declares is reported as that layer's
/// finding for its class: the value the request carried, then every value the type admits.
#[tokio::test]
async fn a_value_outside_an_enumerated_set_is_reported_as_the_schema_finding() {
    let router = emulator();
    // Rows differ along the form of the rejected value: a value of the right shape that is
    // not in the set, one differing only in case from a value that is, and the empty
    // string.
    let cases = vec![
        ReportCase {
            label: "ListServices assessmentStatus outside its modelled value set",
            method: "GET",
            uri: "/v2/list-services?assessmentStatus=NOT_A_STATUS",
            body: None,
            findings: vec![not_in_enum("NOT_A_STATUS", &ASSESSMENT_STATUS_VALUES)],
        },
        ReportCase {
            label: "ListServices assessmentStatus in the wrong case",
            method: "GET",
            uri: "/v2/list-services?assessmentStatus=success",
            body: None,
            findings: vec![not_in_enum("success", &ASSESSMENT_STATUS_VALUES)],
        },
        ReportCase {
            label: "ListServices assessmentStatus empty",
            method: "GET",
            uri: "/v2/list-services?assessmentStatus=",
            body: None,
            findings: vec![not_in_enum("", &ASSESSMENT_STATUS_VALUES)],
        },
    ];

    assert_reports(&router, cases).await;
}

/// RULE: a union body that provides other than exactly one of the alternatives its schema
/// publishes is reported as the CHOICE's own finding — how many alternatives the submitted
/// object matched, out of how many the schema offers — and never as an internal shape name.
///
/// The rows span how many alternatives the body matched: none (an empty object, and one
/// carrying a field the union does not declare) and two.
#[tokio::test]
async fn a_union_outside_exactly_one_alternative_is_reported_as_the_choice_finding() {
    let router = emulator();

    let cases = vec![
        ReportCase {
            label: "CreateInputSource resourceConfiguration with no field",
            method: "POST",
            uri: "/v2/create-input-source",
            body: Some(json!({
                "serviceArn": WELL_FORMED_ARN,
                "resourceConfiguration": {},
            })),
            findings: vec![matched_schemas(0, RESOURCE_CONFIGURATION_ALTERNATIVES)],
        },
        ReportCase {
            label: "CreateInputSource resourceConfiguration with an undeclared field",
            method: "POST",
            uri: "/v2/create-input-source",
            body: Some(json!({
                "serviceArn": WELL_FORMED_ARN,
                "resourceConfiguration": {"notAnAlternative": "x"},
            })),
            findings: vec![matched_schemas(0, RESOURCE_CONFIGURATION_ALTERNATIVES)],
        },
        ReportCase {
            label: "CreateInputSource resourceConfiguration with two fields",
            method: "POST",
            uri: "/v2/create-input-source",
            body: Some(json!({
                "serviceArn": WELL_FORMED_ARN,
                "resourceConfiguration": {
                    "cfnStackArn": WELL_FORMED_ARN,
                    "designFileS3Url": "s3://bucket/design.json",
                },
            })),
            findings: vec![matched_schemas(2, RESOURCE_CONFIGURATION_ALTERNATIVES)],
        },
    ];

    assert_reports(&router, cases).await;

    // A body providing EXACTLY one alternative satisfies the choice and reaches the
    // handler, which refuses the `serviceArn` nothing is stored under — so the finding
    // above reports the choice rather than refusing every configuration.
    let (status, body) = call(
        &router,
        "POST",
        "/v2/create-input-source",
        Some(json!({
            "serviceArn": WELL_FORMED_ARN,
            "resourceConfiguration": {"cfnStackArn": WELL_FORMED_ARN},
        })),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"one alternative, for a service that is not stored\": expected the handler's \
         404, got {status} and {body:?}"
    );
}

/// RULE: wording a refusal changes nothing about the store — a create refused for a value
/// outside its member's pattern leaves no resource behind, so a later read sees exactly
/// what it saw before.
#[tokio::test]
async fn a_refused_create_leaves_the_store_untouched() {
    let router = emulator();

    // A service that WAS accepted, so the read after the refusals has something to still
    // see: a count that was zero both times would also hold for a store nothing reaches.
    let (status, created) = call(
        &router,
        "POST",
        "/v2/create-service",
        Some(json!({
            "name": "a-well-formed-name",
            "regions": ["us-east-1"],
            "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
        })),
    )
    .await;
    assert_eq!(status, 200, "creating the service to read back: got {created:?}");
    let (status, before) = call(&router, "GET", "/v2/list-services", None).await;
    assert_eq!(status, 200, "listing before the refusals: got {before:?}");
    let names_before = summary_names(&before);
    assert_eq!(
        names_before,
        vec!["a-well-formed-name".to_string()],
        "listing before the refusals: expected the one created service, got {names_before:?}"
    );

    for name in ["bad name has spaces!", "_leading-underscore", "name.with.dots"] {
        let (status, body) = call(
            &router,
            "POST",
            "/v2/create-service",
            Some(json!({
                "name": name,
                "regions": ["us-east-1"],
                "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected 400, got {status} and {body:?}"
        );
    }

    let (status, after) = call(&router, "GET", "/v2/list-services", None).await;
    assert_eq!(status, 200, "listing after the refusals: got {after:?}");
    let names_after = summary_names(&after);
    assert_eq!(
        names_after, names_before,
        "case a refused create: expected {names_before:?} still stored, got {names_after:?}"
    );
}

/// The `name` of every service `ListServices` reported, in the order it reported them.
fn summary_names(body: &Value) -> Vec<String> {
    body["serviceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no serviceSummaries array in {body:?}"))
        .iter()
        .map(|summary| {
            summary["name"]
                .as_str()
                .unwrap_or_else(|| panic!("no name in {summary:?}"))
                .to_string()
        })
        .collect()
}
