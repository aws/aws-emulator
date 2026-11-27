// SPDX-License-Identifier: CC0-1.0

//! How a REFUSAL of this service identifies itself, in each of the protocols a client reaches it
//! in.
//!
//! Two rules, asserted over one table of refusals so every class is judged under both of them.
//!
//! 1. **A refusal answered on the `awsQuery` wire names its class by that class's QUERY code.**
//!    This service's model declares `awsQuery` beside `awsJson1_0` and carries the
//!    `awsQueryCompatible` modifier, and it binds an `awsQueryError` code to each error shape
//!    (`SetAlarmState`: `InvalidFormatFault` -> `InvalidFormat`, `ResourceNotFound` ->
//!    `ResourceNotFound`, `smithy_query(query="operation",operation="SetAlarmState")`), so the
//!    `<Code>` of the `<ErrorResponse>` envelope is the code a query-era client branches on
//!    rather than the shape name a JSON body carries — and the request-validation class, which no
//!    shape of the model declares, is `ValidationError` there while the same refusal names
//!    `ValidationException` in a JSON body.
//! 2. **Every refusal names the REQUEST it answers**: the `<RequestId>` element beside `<Error>`
//!    in the query envelope, and the `x-amzn-requestid` header an SDK reads
//!    `ResponseMetadata.RequestId` off whatever the protocol. The value is minted per call, so
//!    only its presence and its form are asserted (`service_common::query_error_envelope`) — and
//!    where a refusal states that id in its own SENTENCE, as the backend trailer of a metric-math
//!    expression refusal does, the two wires' sentences are compared with each answer's own id
//!    taken out of them ([`without_request_id`]).
//!
//! The rows differ along what the two rules generalise over: which operation is called, which
//! class refuses it (the request-validation class the constraint walk raises, the same class this
//! service words itself, and a modelled fault with an `awsQueryError` code of its own), the HTTP
//! status it carries, and whether the refusal's sentence states a grammar (no value of the
//! request) or quotes what the request carried.
//!
//! The second test is the other half of rule 1 for a refused WRITE: a request refused on either
//! wire leaves the resource it named exactly as it was.

use serde_json::{json, Value};
use service_common::query_error_envelope::is_request_id;

use crate::harness::{
    build_router, call_with_headers, described_alarm, error_code, error_message, merged,
    metric_alarm_request, ok, query_call, AlarmKind,
};

/// The media type an `awsQuery` client of this service sends its form body under.
const FORM: &str = "application/x-www-form-urlencoded";

/// The alarm every row of the table reads or writes, created before the table runs.
const ALARM: &str = "refusal-envelope-alarm";

/// The composite alarm one row reads, whose members are not contributors.
const COMPOSITE: &str = "refusal-envelope-composite";

/// The `Period` one row asks for: a cadence outside the grammar an alarm is evaluated on.
const BAD_PERIOD: &str = "45";

/// The sentence CloudWatch refuses a `Period` outside that grammar with: "Valid values are 10,
/// 20, 30, and any multiple of 60."
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — the whole of the
/// message states which periods ARE legal, so it carries no value of the request.
const PERIOD_REFUSAL: &str = "Period must be 10, 20, 30 or a multiple of 60";

/// The marker [`without_request_id`] puts in place of the id an answer names its own request by.
const THE_ANSWERS_OWN_REQUEST: &str = "<the answer's own request>";

/// `message` with `request_id` — the id the answering response named its own request by —
/// replaced by [`THE_ANSWERS_OWN_REQUEST`].
///
/// Two answers to two calls name two requests, so a sentence that states its own request id is
/// compared across the wires with that one per-call value taken out of it and every other word
/// left exactly as the service wrote it.
fn without_request_id(message: &str, request_id: &str) -> String {
    if request_id.is_empty() {
        return message.to_string();
    }
    message.replace(request_id, THE_ANSWERS_OWN_REQUEST)
}

/// The text of the first `<tag>` element of `body`, read with the XML reader this crate's tests
/// read an XML wire with.
fn first_element(body: &str, tag: &str) -> Option<String> {
    let mut reader = quick_xml::Reader::from_str(body);
    let mut text = String::new();
    let mut inside = false;
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the answer must be well-formed XML ({error}): {body}"))
        {
            quick_xml::events::Event::Start(start) => {
                inside = String::from_utf8_lossy(start.name().as_ref()) == tag;
                text.clear();
            }
            quick_xml::events::Event::Text(chunk) if inside => {
                let decoded = chunk
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the answer must decode ({error}): {body}"));
                text.push_str(&decoded);
            }
            quick_xml::events::Event::GeneralRef(reference) if inside => {
                let named = reference
                    .decode()
                    .unwrap_or_else(|error| panic!("the answer must decode ({error}): {body}"));
                // XML's five predefined entities, the only ones an error envelope's renderer
                // writes.
                text.push(match named.as_ref() {
                    "apos" => '\'',
                    "quot" => '"',
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    other => panic!("the answer names an entity &{other}: {body}"),
                });
            }
            quick_xml::events::Event::End(_) if inside => return Some(std::mem::take(&mut text)),
            quick_xml::events::Event::Eof => return None,
            _ => continue,
        }
    }
}

/// What a row expects of the `<Message>` / `Message` member the refusal words.
enum Stated {
    /// No sentence at all: the class is answered by its code alone.
    Nothing,
    /// This exact sentence.
    Exactly(String),
    /// A sentence opening with this text — the form of a refusal whose wording states the
    /// request's own expression beside the envelope the metrics query service answered with.
    OpeningWith(&'static str),
}

/// One row: a refusal, and the two names its answer carries for the class that made it.
struct Row {
    /// The case name every assertion of the row names.
    case: &'static str,
    /// The operation the row calls, as both wires name it.
    action: &'static str,
    /// The form fields the query call carries after `Action` and `Version`.
    params: Vec<(&'static str, String)>,
    /// The same request as the JSON wire carries it.
    json: Value,
    /// The HTTP status both wires answer with.
    status: u16,
    /// The `<Code>` of the query envelope: the class's query code.
    query_code: &'static str,
    /// The `__type` of the JSON body: the class's shape name.
    json_code: &'static str,
    /// What the refusal's sentence states.
    stated: Stated,
}

/// One row of the refused-write table: the case, the operation it calls, and the form fields it
/// carries. Named so the row type stays simple enough to read.
type RefusedWrite = (&'static str, &'static str, Vec<(&'static str, String)>);

/// A `PutMetricAlarm` request whose `Metrics` expression refers to an id the array does not
/// define, as the JSON wire carries it.
fn undefined_reference_request(name: &str) -> Value {
    merged(
        metric_alarm_request(name),
        &json!({
            "Namespace": null,
            "MetricName": null,
            "Statistic": null,
            "Period": null,
            "Metrics": [{"Id": "watched", "Expression": "absent * 2", "ReturnData": true}],
        }),
    )
}

#[tokio::test]
async fn a_refusal_names_its_class_by_the_protocols_own_code_and_names_the_request() {
    let router = build_router();
    ok(&router, "PutMetricAlarm", &metric_alarm_request(ALARM)).await;
    ok(
        &router,
        "PutCompositeAlarm",
        &json!({"AlarmName": COMPOSITE, "AlarmRule": format!("ALARM(\"{ALARM}\")")}),
    )
    .await;

    let rows: Vec<Row> = vec![
        Row {
            // "StateValue" targets the `StateValue` enum, so a value outside it is refused by
            // the request-validation class — `ValidationError` on the query wire.
            case: "a state value outside the modelled enum",
            action: "SetAlarmState",
            params: vec![
                ("AlarmName", ALARM.to_string()),
                ("StateValue", "ALARMING".to_string()),
                ("StateReason", "testing".to_string()),
            ],
            json: json!({
                "AlarmName": ALARM,
                "StateValue": "ALARMING",
                "StateReason": "testing",
            }),
            status: 400,
            query_code: "ValidationError",
            json_code: "ValidationException",
            stated: Stated::OpeningWith("1 validation error detected"),

        },
        Row {
            // The same operation's own modelled fault, whose `awsQueryError` code the model
            // binds: `ResourceNotFound` / 404.
            case: "a state forced on an alarm no writer created",
            action: "SetAlarmState",
            params: vec![
                ("AlarmName", "refusal-envelope-absent".to_string()),
                ("StateValue", "ALARM".to_string()),
                ("StateReason", "testing".to_string()),
            ],
            json: json!({
                "AlarmName": "refusal-envelope-absent",
                "StateValue": "ALARM",
                "StateReason": "testing",
            }),
            status: 404,
            query_code: "ResourceNotFound",
            json_code: "ResourceNotFound",
            stated: Stated::Nothing,
        },
        Row {
            case: "a cadence outside the grammar an alarm is evaluated on",
            action: "PutMetricAlarm",
            params: vec![
                ("AlarmName", "refusal-envelope-period".to_string()),
                ("Namespace", "Custom/Refusals".to_string()),
                ("MetricName", "Latency".to_string()),
                ("Statistic", "Average".to_string()),
                ("Period", BAD_PERIOD.to_string()),
                ("EvaluationPeriods", "1".to_string()),
                ("Threshold", "1".to_string()),
                ("ComparisonOperator", "GreaterThanThreshold".to_string()),
            ],
            json: merged(
                metric_alarm_request("refusal-envelope-period"),
                &json!({"Period": 45}),
            ),
            status: 400,
            query_code: "ValidationError",
            json_code: "ValidationException",
            stated: Stated::Exactly(PERIOD_REFUSAL.to_string()),
        },
        Row {
            case: "an expression naming an id the array does not define",
            action: "PutMetricAlarm",
            params: vec![
                ("AlarmName", "refusal-envelope-expression".to_string()),
                ("EvaluationPeriods", "1".to_string()),
                ("Threshold", "1".to_string()),
                ("ComparisonOperator", "GreaterThanThreshold".to_string()),
                ("Metrics.member.1.Id", "watched".to_string()),
                ("Metrics.member.1.Expression", "absent * 2".to_string()),
                ("Metrics.member.1.ReturnData", "true".to_string()),
            ],
            json: undefined_reference_request("refusal-envelope-expression"),
            status: 400,
            query_code: "ValidationError",
            json_code: "ValidationException",
            stated: Stated::OpeningWith("Error in expression"),
        },
        Row {
            // An alarm that can never have contributors is refused for its TYPE rather than
            // answered an empty list: a composite alarm's members are not contributors.
            case: "a contributor read of an alarm that can have none",
            action: "DescribeAlarmContributors",
            params: vec![("AlarmName", COMPOSITE.to_string())],
            json: json!({"AlarmName": COMPOSITE}),
            status: 400,
            query_code: "ValidationError",
            json_code: "ValidationException",
            stated: Stated::Exactly(format!("Alarm {COMPOSITE} is not a multi time series alarm")),
        },
    ];

    for row in rows {
        let Row {
            case,
            action,
            params,
            json,
            status,
            query_code,
            json_code,
            stated,
        } = row;

        // Rule 1 on the query wire, and rule 2 in the element of its envelope.
        let fields: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let (answered, media_type, body) =
            query_call(&router, FORM, &[], action, &fields).await;
        assert_eq!(
            (answered, media_type.as_str()),
            (status, "text/xml"),
            "case {case:?}: expected HTTP {status} in the query envelope, got {answered} with \
             {body}"
        );
        assert_eq!(
            (
                first_element(&body, "Type").as_deref(),
                first_element(&body, "Code").as_deref(),
            ),
            (Some("Sender"), Some(query_code)),
            "case {case:?}: expected a Sender fault of class {query_code}, got {body}"
        );
        let named = first_element(&body, "RequestId");
        assert!(
            named.as_deref().is_some_and(is_request_id),
            "case {case:?}: expected the envelope to name the request, got {named:?} in {body}"
        );
        let query_message = first_element(&body, "Message").unwrap_or_default();

        // The same refusal on the JSON wire: the class's SHAPE name, the same sentence, and the
        // request named in the header an SDK reads its `ResponseMetadata` off.
        let (answered, headers, parsed) = call_with_headers(&router, action, &json).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status} on the JSON wire, got {answered} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            json_code,
            "case {case:?}: expected the {json_code} class, got {parsed}"
        );
        let reported = headers
            .get("x-amzn-requestid")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(
            is_request_id(reported),
            "case {case:?}: expected the response to name the request, got {reported:?} with \
             {parsed}"
        );
        let json_message = error_message(&parsed);
        // One class, one sentence on both wires — with the id each answer names its OWN request
        // by taken out of it: the backend trailer of a metric-math expression refusal states
        // that id, so two answers to two calls differ in exactly that value.
        assert_eq!(
            without_request_id(&json_message, reported),
            without_request_id(&query_message, named.as_deref().unwrap_or_default()),
            "case {case:?}: both wires word one class with one sentence, got {json_message:?} \
             and {query_message:?}"
        );
        match stated {
            Stated::Nothing => assert!(
                json_message.is_empty() && query_message.is_empty(),
                "case {case:?}: this class is answered by its code alone, so neither wire words \
                 a sentence, got {json_message:?} and {query_message:?}"
            ),
            Stated::Exactly(expected) => assert_eq!(
                json_message, expected,

                "case {case:?}: expected the sentence {expected:?}, got {json_message:?}"
            ),
            Stated::OpeningWith(opening) => assert!(
                json_message.starts_with(opening),
                "case {case:?}: expected a sentence opening with {opening:?}, got \
                 {json_message:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_refused_write_leaves_what_it_named_exactly_as_it_was() {
    let router = build_router();
    let name = "refused-write-keeps-its-configuration";
    ok(&router, "PutMetricAlarm", &metric_alarm_request(name)).await;
    let before = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));

    // Rows differ along the wire the refused call arrives on and along what is wrong with it:
    // a cadence outside the grammar, an expression naming an id nothing defines, and a state
    // value outside the modelled enum.
    let refused: Vec<RefusedWrite> = vec![
        (
            "a cadence outside the grammar",
            "PutMetricAlarm",
            vec![
                ("AlarmName", name.to_string()),
                ("Namespace", "Custom/Refusals".to_string()),
                ("MetricName", "Latency".to_string()),
                ("Statistic", "Average".to_string()),
                ("Period", BAD_PERIOD.to_string()),
                ("EvaluationPeriods", "1".to_string()),
                ("Threshold", "99".to_string()),
                ("ComparisonOperator", "LessThanThreshold".to_string()),
            ],
        ),
        (
            "an expression naming an id nothing defines",
            "PutMetricAlarm",
            vec![
                ("AlarmName", name.to_string()),
                ("EvaluationPeriods", "1".to_string()),
                ("Threshold", "99".to_string()),
                ("ComparisonOperator", "LessThanThreshold".to_string()),
                ("Metrics.member.1.Id", "watched".to_string()),
                ("Metrics.member.1.Expression", "absent * 2".to_string()),
                ("Metrics.member.1.ReturnData", "true".to_string()),
            ],
        ),
        (
            "a state value outside the modelled enum",
            "SetAlarmState",
            vec![
                ("AlarmName", name.to_string()),
                ("StateValue", "ALARMING".to_string()),
                ("StateReason", "testing".to_string()),
            ],
        ),
    ];

    for (case, action, params) in refused {
        let fields: Vec<(&str, &str)> = params
            .iter()
            .map(|(member, value)| (*member, value.as_str()))
            .collect();
        let (status, _media_type, body) = query_call(&router, FORM, &[], action, &fields).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the call to be refused, got {status} with {body}"
        );
        let after = described_alarm(&router, AlarmKind::Metric, name)
            .await
            .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
        assert_eq!(
            (
                after.get("Period"),
                after.get("Threshold"),
                after.get("ComparisonOperator"),
                after.get("StateValue"),
                after.get("Metrics"),
            ),
            (
                before.get("Period"),
                before.get("Threshold"),
                before.get("ComparisonOperator"),
                before.get("StateValue"),
                before.get("Metrics"),
            ),
            "case {case:?}: a refused call must leave the alarm as it was, got {after} beside \
             {before}"
        );
    }
}
