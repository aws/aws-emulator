// SPDX-License-Identifier: CC0-1.0

//! The REQUEST a metric-math expression refusal of `PutMetricAlarm` names.
//!
//! An expression of a `Metrics` array is validated by the metrics query service CloudWatch calls
//! for it, and CloudWatch reports that call's own failure: the expression error followed by the
//! backend trailer that names the service, the status, the class — and the request the whole
//! refusal answers, which is the id the SAME response reports to an SDK
//! (`ResponseMetadata.RequestId`, the `<RequestId>` of the `awsQuery` error envelope and the
//! `x-amzn-requestid` header). So the rule asserted here is: the trailer of such a refusal states
//! the id of the request it answers, the very id that response names that request by — not a
//! blank, and not some other id.
//!
//! The rows differ along what the rule generalises over: which expression of the array is at
//! fault and what is wrong with it (an id no query of the request defines, with a different
//! undefined id and a different expression id per row, and an expression the parse cannot read at
//! all), whether the call CREATES an alarm or would have replaced one that already exists, and
//! which of this service's wires the refusal is answered on. The ids are per-response values, so
//! no row states one: each row reads the id off the response it is judging and requires the
//! trailer to carry THAT value, and the last assertion requires every answer to name a request of
//! its own.

use std::collections::HashSet;

use serde_json::{json, Value};
use service_common::query_error_envelope::is_request_id;

use crate::harness::{call_with_headers, element, error_code, error_message, ok, query_call};

/// The media type an `awsQuery` client of this service sends its form body under.
const FORM: &str = "application/x-www-form-urlencoded";

/// The namespace the metric every row retrieves lives in.
const NAMESPACE: &str = "Custom/MathRefusals";

/// The metric every row's retrieval query reads.
const METRIC: &str = "Errors";

/// One row: a `Metrics` array `PutMetricAlarm` refuses the expression of, and the sentence that
/// refusal states before its trailer.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// Whether an alarm of the row's name exists before the refused call: the refusal is the
    /// same on a create and on an update that would have replaced a stored alarm.
    existing: bool,
    /// The queries the refused call carries.
    queries: Value,
    /// The refusal's sentence up to the trailer: the expression at fault and what is wrong with
    /// it, both read off the request.
    stated: String,
}

/// The query that retrieves the series the rows' expressions refer to.
fn retrieval(id: &str) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": METRIC},
            "Period": 60,
            "Stat": "Maximum",
        },
        "ReturnData": false,
    })
}

/// A `PutMetricAlarm` request on `name` whose series is stated as the query array `queries`.
fn alarm_request(name: &str, queries: &Value) -> Value {
    json!({
        "AlarmName": name,
        "Metrics": queries,
        "Threshold": 50.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "EvaluationPeriods": 1,
        "ActionsEnabled": false,
        "TreatMissingData": "missing",
    })
}

/// The whole refusal AWS words for a metric-math expression of this operation: the sentence, and
/// the backend trailer closing it under the id the answering response names its request by.
fn expected_refusal(stated: &str, request_id: &str) -> String {
    format!(
        "{stated} (Service: AmazonCloudWatch; Status Code: 400; Error Code: \
         ValidationException; Request ID: {request_id}; Proxy: null)"
    )
}

/// The form fields a query call carries the array `queries` on, as the indexed members of
/// `Metrics` — the same request the JSON row sends, spelled the way the other wire spells it.
fn query_fields(name: &str, queries: &Value) -> Vec<(String, String)> {
    let mut fields = vec![
        ("AlarmName".to_string(), name.to_string()),
        ("Threshold".to_string(), "50.0".to_string()),
        (
            "ComparisonOperator".to_string(),
            "GreaterThanThreshold".to_string(),
        ),
        ("EvaluationPeriods".to_string(), "1".to_string()),
        ("ActionsEnabled".to_string(), "false".to_string()),
        ("TreatMissingData".to_string(), "missing".to_string()),
    ];
    for (index, query) in queries
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let at = index + 1;
        let id = query["Id"].as_str().unwrap_or_default().to_string();
        fields.push((format!("Metrics.member.{at}.Id"), id));
        if let Some(expression) = query.get("Expression").and_then(Value::as_str) {
            fields.push((
                format!("Metrics.member.{at}.Expression"),
                expression.to_string(),
            ));
        }
        if let Some(stat) = query.get("MetricStat") {
            fields.push((
                format!("Metrics.member.{at}.MetricStat.Metric.Namespace"),
                stat["Metric"]["Namespace"].as_str().unwrap_or_default().to_string(),
            ));
            fields.push((
                format!("Metrics.member.{at}.MetricStat.Metric.MetricName"),
                stat["Metric"]["MetricName"].as_str().unwrap_or_default().to_string(),
            ));
            fields.push((
                format!("Metrics.member.{at}.MetricStat.Period"),
                stat["Period"].to_string(),
            ));
            fields.push((
                format!("Metrics.member.{at}.MetricStat.Stat"),
                stat["Stat"].as_str().unwrap_or_default().to_string(),
            ));
        }
        if let Some(returns) = query.get("ReturnData").and_then(Value::as_bool) {
            fields.push((format!("Metrics.member.{at}.ReturnData"), returns.to_string()));
        }
    }
    fields
}

#[tokio::test]
async fn an_expression_refusal_names_the_request_its_own_answer_names() {
    let router = crate::harness::build_router();
    let rows = [
        Row {
            case: "an expression dividing by an id no query defines",
            existing: false,
            queries: json!([
                retrieval("m1"),
                {"Id": "e1", "Expression": "(m1/m9)*100", "ReturnData": true},
            ]),
            stated: "Error in expression 'e1': ID 'm9' not found".to_string(),
        },
        Row {
            case: "another expression id and another undefined id",
            existing: false,
            queries: json!([
                retrieval("a2"),
                {"Id": "total", "Expression": "a2 + missing_one", "ReturnData": true},
            ]),
            stated: "Error in expression 'total': ID 'missing_one' not found".to_string(),
        },
        Row {
            case: "an update that would have replaced a stored alarm",
            existing: true,
            queries: json!([
                retrieval("m1"),
                {"Id": "e1", "Expression": "m1 + does_not_exist", "ReturnData": true},
            ]),
            stated: "Error in expression 'e1': ID 'does_not_exist' not found".to_string(),
        },
        Row {
            case: "an expression the parse cannot read at all",
            existing: false,
            queries: json!([
                retrieval("m1"),
                {"Id": "e1", "Expression": "m1 *", "ReturnData": true},
            ]),
            stated: "Error in expression 'e1': Invalid syntax".to_string(),
        },
    ];

    // Every id this test saw an answer name its request by: an id belongs to ONE response, so no
    // two of these answers may report the same one.
    let mut named: Vec<String> = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("math-refusal-request-id-{index}");
        if row.existing {
            // A well-formed alarm of the same name, so the refused call below is an update of a
            // stored alarm rather than a create.
            ok(
                &router,
                "PutMetricAlarm",
                &alarm_request(
                    &name,
                    &json!([
                        retrieval("m1"),
                        {"Id": "e1", "Expression": "m1 * 2", "ReturnData": true},
                    ]),
                ),
            )
            .await;
        }

        // The JSON wire: the id is reported in the header an SDK reads a failed call's
        // `ResponseMetadata.RequestId` off, and the trailer must carry that same id.
        let (status, headers, body) =
            call_with_headers(&router, "PutMetricAlarm", &alarm_request(&name, &row.queries)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the expression to be refused with HTTP 400, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the request-validation class, got {body}"
        );
        let reported = headers
            .get("x-amzn-requestid")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            is_request_id(&reported),
            "case {case:?}: expected the answer to name the request it answers, got \
             {reported:?} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            expected_refusal(&row.stated, &reported),
            "case {case:?}: expected the refusal to close with the trailer naming its own \
             request {reported:?}, got {message:?}"
        );
        named.push(reported);

        // The same refusal on the query wire, where the id is an element of the error envelope:
        // the trailer carries the id that envelope states, so a client reading either sees one
        // value. The sentence travels XML-escaped, so what is asserted of it is the trailer.
        let fields = query_fields(&name, &row.queries);
        let borrowed: Vec<(&str, &str)> = fields
            .iter()
            .map(|(member, value)| (member.as_str(), value.as_str()))
            .collect();
        let (status, _media_type, envelope) =
            query_call(&router, FORM, &[], "PutMetricAlarm", &borrowed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the same refusal on the query wire, got {status} with \
             {envelope}"
        );
        let stated_id = element(&envelope, "RequestId");
        assert!(
            is_request_id(&stated_id),
            "case {case:?}: expected the envelope to name the request, got {stated_id:?} in \
             {envelope}"
        );
        let trailer = format!("Request ID: {stated_id}; Proxy: null)");
        assert!(
            element(&envelope, "Message").ends_with(&trailer),
            "case {case:?}: expected the refusal to close with {trailer:?}, got {envelope}"
        );
        named.push(stated_id);
    }

    let distinct: HashSet<&String> = named.iter().collect();
    assert_eq!(
        distinct.len(),
        named.len(),
        "every answer names a request of its own, so no id may be reported twice, got {named:?}"
    );
}
