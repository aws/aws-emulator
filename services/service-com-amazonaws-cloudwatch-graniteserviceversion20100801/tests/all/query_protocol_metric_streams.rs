// SPDX-License-Identifier: CC0-1.0

//! Every call of this service is read AND answered in the protocol the CALLER used, and the
//! metric-stream operations are the ones this suite drives through both of them.
//!
//! The rule under test: **the protocol one request is served in follows from its content type**
//! — a `application/x-www-form-urlencoded` body is an `awsQuery` call, whose operation is named
//! by the `Action` field of the form and whose answer is `text/xml` in that protocol's
//! envelope (`<OperationResponse><OperationResult>…` for a success,
//! `<ErrorResponse><Error><Type>/<Code>/<Message></Error><RequestId>` for a failure, with the
//! status the error class carries), whatever OTHER dispatch header the client sends beside it;
//! an `application/x-amz-json-1.0` body is an `awsJson1_0` call and is answered in JSON. The
//! service's model declares both protocols side by side, so neither answer is a translation of
//! the other: the same stored stream is reported by each in its own form.
//!
//! The rows differ along what the rule generalises over — the protocol the call is made in,
//! how the client spells the media type (bare, with a `charset` parameter, in upper case),
//! whether it also sends the RPC target header of the OTHER protocol, which operation is
//! called, and whether the call succeeds or is refused (an absent stream, a name no stream
//! could carry, a page size outside its modelled range, an `Action` the model does not
//! declare) — so the recorded form POST is one row among many.
//!
//! The second half of the rule is what a FAILURE of a query call names the request by: the
//! `<RequestId>` element beside `<Error>`, the failure's counterpart of the
//! `ResponseMetadata/RequestId` a success carries. Every refused row asserts one.

use serde_json::{json, Value};
use service_common::query_error_envelope::is_request_id;

use crate::harness::{build_router, call, error_code, ok, query_call};

/// The media type a query client sends its form body under, bare.
const FORM: &str = "application/x-www-form-urlencoded";

/// The same media type with the `charset` parameter a client's own encoder adds.
const FORM_WITH_CHARSET: &str = "application/x-www-form-urlencoded; charset=utf-8";

/// The same media type in upper case: a media type is case-insensitive.
const FORM_IN_UPPER_CASE: &str = "Application/X-WWW-Form-Urlencoded";

/// The RPC target header an `awsJson1_0` client of this service sends — carried by the rows
/// that send it BESIDE a form body, as a client migrating between the two protocols does.
fn json_target(operation: &str) -> (&'static str, String) {
    ("x-amz-target", format!("{}.{operation}", crate::harness::TARGET_PREFIX))
}

/// The stream every row below reads: created over the JSON wire, so what a query call reports
/// comes from the store rather than from the request it arrived on.
const STREAM: &str = "query-protocol-stream";

/// A `PutMetricStream` request for `name`, with only the members the model makes required.
fn put_request(name: &str) -> Value {
    crate::metric_streams::put_request(name, "json")
}

/// The name of the document's root element, read with the XML reader this crate's tests
/// already read an XML wire with.
fn root_element(body: &str) -> String {
    let mut reader = quick_xml::Reader::from_str(body);
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the answer must be well-formed XML ({error}): {body}"))
        {
            quick_xml::events::Event::Start(start) => {
                return String::from_utf8_lossy(start.name().as_ref()).to_string();
            }
            quick_xml::events::Event::Eof => return String::new(),
            _ => continue,
        }
    }
}

/// The text of every `<tag>` element whose content is text, in document order, read with the
/// XML reader this crate's tests already read an XML wire with.
///
/// The reader rather than a substring search, because both parts matter here: an entity
/// reference the renderer escaped (`&apos;` for the quotes an AWS constraint message puts
/// round the offending value) is read back as the character it stands for, the way an SDK's
/// parser hands it to a client, and an element that CONTAINS other elements yields no text of
/// its own — so `<member>` names the leaf entries of a list rather than everything nested
/// under one. The reader breaks text at each entity reference and emits the reference on its
/// own, so both kinds of fragment are accumulated until the element closes.
fn element_texts(body: &str, tag: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(body);
    let mut found: Vec<String> = Vec::new();
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
                // XML's five predefined entities, the only ones an AWS error envelope's
                // renderer writes.
                text.push(match named.as_ref() {
                    "apos" => '\'',
                    "quot" => '"',
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    other => panic!("the answer names an entity &{other}; this reader cannot \
                                     resolve: {body}"),
                });
            }
            quick_xml::events::Event::End(_) => {
                if inside {
                    found.push(std::mem::take(&mut text));
                }
                inside = false;
            }
            quick_xml::events::Event::Eof => return found,
            _ => continue,
        }
    }
}

/// The text of the first `<tag>` element, or `None` when the document carries none.
fn first_element(body: &str, tag: &str) -> Option<String> {
    element_texts(body, tag).first().cloned()
}

/// Assert that `body` names the request it answers by a well-formed `<RequestId>`.
fn assert_names_the_request(case: &str, body: &str) {
    let stated = first_element(body, "RequestId");
    let reported = stated.as_deref().unwrap_or_default();
    assert!(
        stated.is_some() && is_request_id(reported),
        "case {case:?}: expected the envelope to name the request by a request id, got \
         {stated:?} in {body}"
    );
}

/// What a row expects of the answer: a success envelope named after the operation, or a
/// refusal naming a class and whatever sentence that class words.
enum Answered {
    /// `<{0}Response><{0}Result>` — the success envelope of the named operation.
    Success(&'static str),
    /// `<ErrorResponse>` with this `<Code>` and this `<Message>`.
    Refused(&'static str, Stated),
}

/// What the `<Message>` of a refusal states.
enum Stated {
    /// No `<Message>` at all: the class is answered by its code alone.
    Nothing,
    /// This exact sentence, the fixed text of the class with the request's own values
    /// interpolated.
    Exactly(&'static str),
    /// A sentence this suite does not pin: the routing failure's wording belongs to the
    /// protocol envelope rather than to this service, and no recording of this service shows
    /// it, so only the class and the status are asserted for it.
    Whatever,
}

/// One row of the table below: the request a client makes, and the answer rule 1 owes it.
///
/// A struct rather than a tuple, because seven positional fields (two of them collections) is
/// what `clippy::type_complexity` is for, and a row reads better with its parts named.
struct Row {
    /// The case name every assertion of the row names.
    case: &'static str,
    /// The media type the client spells its form body with.
    content_type: &'static str,
    /// Any other header it sends beside it — the other protocol's dispatch header, for the
    /// rows about a client migrating between the two.
    extra_headers: Vec<(&'static str, String)>,
    /// The operation its form body names in `Action`.
    action: &'static str,
    /// The form fields after `Action` and `Version`.
    params: Vec<(&'static str, &'static str)>,
    /// The HTTP status the answer carries.
    status: u16,
    /// What the answer's envelope states.
    answered: Answered,
}

#[tokio::test]
async fn a_form_encoded_call_is_served_in_the_query_protocol() {
    let router = build_router();
    ok(&router, "PutMetricStream", &put_request(STREAM)).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "a read of a stored stream",
            content_type: FORM,
            extra_headers: vec![],
            action: "GetMetricStream",
            params: vec![("Name", STREAM)],
            status: 200,
            answered: Answered::Success("GetMetricStream"),
        },
        Row {
            case: "the same read with the charset parameter the client's encoder adds",
            content_type: FORM_WITH_CHARSET,
            extra_headers: vec![],
            action: "GetMetricStream",
            params: vec![("Name", STREAM)],
            status: 200,
            answered: Answered::Success("GetMetricStream"),
        },
        Row {
            case: "the same read with the media type in upper case",
            content_type: FORM_IN_UPPER_CASE,
            extra_headers: vec![],
            action: "GetMetricStream",
            params: vec![("Name", STREAM)],
            status: 200,
            answered: Answered::Success("GetMetricStream"),
        },
        Row {
            case: "the same read from a client that also sends the other protocol's target \
                   header",
            content_type: FORM_WITH_CHARSET,
            extra_headers: vec![json_target("GetMetricStream")],
            action: "GetMetricStream",
            params: vec![("Name", STREAM)],
            status: 200,
            answered: Answered::Success("GetMetricStream"),
        },
        Row {
            case: "a listing of the streams of the Region",
            content_type: FORM,
            extra_headers: vec![],
            action: "ListMetricStreams",
            params: vec![],
            status: 200,
            answered: Answered::Success("ListMetricStreams"),
        },
        Row {
            case: "a read of a stream that does not exist",
            content_type: FORM,
            extra_headers: vec![],
            action: "GetMetricStream",
            params: vec![("Name", "no-such-stream")],
            status: 404,
            // "The named resource does not exist." is answered by its code alone, so the
            // envelope words no sentence.
            answered: Answered::Refused("ResourceNotFoundException", Stated::Nothing),
        },
        Row {
            case: "the same miss from a client that also sends the other protocol's target \
                   header",
            content_type: FORM,
            extra_headers: vec![json_target("GetMetricStream")],
            action: "GetMetricStream",
            params: vec![("Name", "no-such-stream")],
            status: 404,
            answered: Answered::Refused("ResourceNotFoundException", Stated::Nothing),
        },
        Row {
            // A metric stream's name is the resource's identity, so a name no stream could
            // carry is a parameter VALUE fault rather than a missing resource; the legacy
            // query code of `InvalidParameterValueException` is what the envelope names.
            case: "a read naming a name no stream could carry",
            content_type: FORM,
            extra_headers: vec![],
            action: "GetMetricStream",
            params: vec![("Name", "a name with spaces")],
            status: 400,
            answered: Answered::Refused("InvalidParameterValue", Stated::Exactly("Invalid Name")),
        },
        Row {
            // `ListMetricStreamsInput$MaxResults` targets `ListMetricStreamsMaxResults`,
            // @range min 1 max 500 (smithy_query(query="operation",
            // operation="ListMetricStreams")), so a page size under the minimum is refused by
            // the request-validation class with the range clause.
            case: "a listing asking for a page below the modelled minimum",
            content_type: FORM,
            extra_headers: vec![],
            action: "ListMetricStreams",
            params: vec![("MaxResults", "0")],
            status: 400,
            answered: Answered::Refused(
                "ValidationError",
                Stated::Exactly(
                    "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
                     constraint: Member must have value greater than or equal to 1",
                ),
            ),
        },
        Row {
            case: "a listing asking for a page above the modelled maximum",
            content_type: FORM,
            extra_headers: vec![],
            action: "ListMetricStreams",
            params: vec![("MaxResults", "501")],
            status: 400,
            answered: Answered::Refused(
                "ValidationError",
                Stated::Exactly(
                    "1 validation error detected: Value '501' at 'maxResults' failed to satisfy \
                     constraint: Member must have value less than or equal to 500",
                ),
            ),
        },
        Row {
            case: "a form body naming an Action the model does not declare",
            content_type: FORM,
            extra_headers: vec![],
            action: "GetMetricStreamPlease",
            params: vec![("Name", STREAM)],
            status: 400,
            answered: Answered::Refused("InvalidAction", Stated::Whatever),
        },
    ];

    for row in rows {
        let Row {
            case,
            content_type,
            extra_headers,
            action,
            params,
            status,
            answered: expected,
        } = row;
        let headers: Vec<(&str, &str)> = extra_headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let (answered, media_type, body) =
            query_call(&router, content_type, &headers, action, &params).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with {body}"
        );
        assert_eq!(
            media_type, "text/xml",
            "case {case:?}: expected a query call to be answered as text/xml, got \
             {media_type:?} with {body}"
        );
        match expected {
            Answered::Success(operation) => {
                assert_eq!(
                    root_element(&body),
                    format!("{operation}Response"),
                    "case {case:?}: expected the {operation}Response envelope, got {body}"
                );
                assert!(
                    body.contains(&format!("<{operation}Result>")),
                    "case {case:?}: expected the {operation}Result element, got {body}"
                );
                assert_names_the_request(case, &body);
            }
            Answered::Refused(code, message) => {
                assert_eq!(
                    root_element(&body),
                    "ErrorResponse",
                    "case {case:?}: expected the ErrorResponse envelope, got {body}"
                );
                // The fault side of a 4xx is the caller's, and the class is named by the code
                // a query client branches on.
                assert_eq!(
                    (
                        first_element(&body, "Type").as_deref(),
                        first_element(&body, "Code").as_deref(),
                    ),
                    (Some("Sender"), Some(code)),
                    "case {case:?}: expected a Sender fault of class {code}, got {body}"
                );
                let stated = first_element(&body, "Message");
                match message {
                    Stated::Nothing => assert_eq!(
                        stated, None,
                        "case {case:?}: this class is answered by its code alone, so the \
                         envelope must word no message, got {stated:?} in {body}"
                    ),
                    Stated::Exactly(expected) => assert_eq!(
                        stated.as_deref(),
                        Some(expected),
                        "case {case:?}: expected the message {expected:?}, got {stated:?} in \
                         {body}"
                    ),
                    Stated::Whatever => assert!(
                        stated.is_some_and(|message| !message.is_empty()),
                        "case {case:?}: expected the envelope to word the failure, got {body}"
                    ),
                }
                assert_names_the_request(case, &body);
            }
        }
    }
}

#[tokio::test]
async fn a_stream_configured_over_the_query_protocol_is_reported_by_both() {
    let router = build_router();
    // Every member of the configuration arrives on the form wire, including the two nested
    // lists: a namespace filter with one of its metric names, and a statistics configuration.
    let (status, media_type, body) = query_call(
        &router,
        FORM_WITH_CHARSET,
        &[],
        "PutMetricStream",
        &[
            ("Name", "form-configured-stream"),
            (
                "FirehoseArn",
                "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
            ),
            ("RoleArn", "arn:aws:iam::000000000000:role/MetricStreamRole"),
            ("OutputFormat", "json"),
            ("IncludeLinkedAccountsMetrics", "true"),
            ("ExcludeFilters.member.1.Namespace", "AWS/S3"),
            (
                "ExcludeFilters.member.1.MetricNames.member.1",
                "BucketSizeBytes",
            ),
            (
                "StatisticsConfigurations.member.1.IncludeMetrics.member.1.Namespace",
                "AWS/EC2",
            ),
            (
                "StatisticsConfigurations.member.1.IncludeMetrics.member.1.MetricName",
                "CPUUtilization",
            ),
            (
                "StatisticsConfigurations.member.1.AdditionalStatistics.member.1",
                "p99",
            ),
        ],
    )
    .await;
    assert_eq!(
        (status, media_type.as_str()),
        (200, "text/xml"),
        "the form-encoded write must be accepted and answered in its own protocol, got {body}"
    );
    assert!(
        first_element(&body, "Arn").is_some_and(|arn| !arn.is_empty()),
        "the write must report the ARN of the stream it configured, got {body}"
    );

    // The JSON wire reports what the form wire stored, member for member.
    let read = ok(
        &router,
        "GetMetricStream",
        &json!({"Name": "form-configured-stream"}),
    )
    .await;
    assert_eq!(
        (
            read.get("Name").and_then(Value::as_str),
            read.get("OutputFormat").and_then(Value::as_str),
            read.get("State").and_then(Value::as_str),
            read.get("IncludeLinkedAccountsMetrics").and_then(Value::as_bool),
        ),
        (
            Some("form-configured-stream"),
            Some("json"),
            Some("running"),
            Some(true),
        ),
        "the JSON read must report the configuration the form wire stored, got {read}"
    );
    assert_eq!(
        read["ExcludeFilters"],
        json!([{"Namespace": "AWS/S3", "MetricNames": ["BucketSizeBytes"]}]),
        "the JSON read must report the namespace filter the form wire carried, got {read}"
    );
    assert_eq!(
        read["StatisticsConfigurations"],
        json!([{
            "IncludeMetrics": [{"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"}],
            "AdditionalStatistics": ["p99"],
        }]),
        "the JSON read must report the statistics configuration the form wire carried, got \
         {read}"
    );

    // And the query wire reports the same stream in its own form: the scalars as text and the
    // list entries as `<member>` elements.
    let (status, media_type, body) = query_call(
        &router,
        FORM,
        &[],
        "GetMetricStream",
        &[("Name", "form-configured-stream")],
    )
    .await;
    assert_eq!(
        (status, media_type.as_str()),
        (200, "text/xml"),
        "the query read must be answered in its own protocol, got {body}"
    );
    for (member, value) in [
        ("Name", "form-configured-stream"),
        ("OutputFormat", "json"),
        ("State", "running"),
        ("IncludeLinkedAccountsMetrics", "true"),
        ("Namespace", "AWS/S3"),
        ("MetricName", "CPUUtilization"),
    ] {
        let stated = first_element(&body, member);
        assert_eq!(
            stated.as_deref(),
            Some(value),
            "case {member:?}: expected the query read to state {value:?}, got {stated:?} in \
             {body}"
        );
    }
    assert_eq!(
        element_texts(&body, "member"),
        vec!["BucketSizeBytes".to_string(), "p99".to_string()],
        "expected the leaf list entries as <member> elements, got {body}"
    );
}

#[tokio::test]
async fn a_json_call_is_still_read_and_answered_in_json() {
    let router = build_router();
    ok(&router, "PutMetricStream", &put_request(STREAM)).await;

    // (case, operation, request, expected status, expected error code — `None` for a success)
    let rows: Vec<(&str, &str, Value, u16, Option<&str>)> = vec![
        (
            "a read of a stored stream",
            "GetMetricStream",
            json!({"Name": STREAM}),
            200,
            None,
        ),
        (
            "a listing of the streams of the Region",
            "ListMetricStreams",
            json!({}),
            200,
            None,
        ),
        (
            "a read of a stream that does not exist",
            "GetMetricStream",
            json!({"Name": "no-such-stream"}),
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "a read naming a name no stream could carry",
            "GetMetricStream",
            json!({"Name": "a name with spaces"}),
            400,
            Some("InvalidParameterValueException"),
        ),
        (
            "a listing asking for a page below the modelled minimum",
            "ListMetricStreams",
            json!({"MaxResults": 0}),
            400,
            Some("ValidationException"),
        ),
    ];

    for (case, operation, request, status, code) in rows {
        let (answered, body) = call(&router, operation, &request).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with {body}"
        );
        match code {
            Some(code) => assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected the {code} class, got {body}"
            ),
            None => assert!(
                body.get("__type").is_none(),
                "case {case:?}: expected a successful JSON answer, got {body}"
            ),
        }
    }

    // The not-found is answered by its code alone: the resource is named by the two members
    // that carry it and NO message member is sent, so a client testing whether a reason came
    // with the refusal is not handed an empty one.
    let (status, body) = call(
        &router,
        "GetMetricStream",
        &json!({"Name": "no-such-stream"}),
    )
    .await;
    assert_eq!(
        (
            status,
            body.get("ResourceType").and_then(Value::as_str),
            body.get("ResourceId").and_then(Value::as_str),
            body.get("Message"),
        ),
        (404, Some("MetricStream"), Some("no-such-stream"), None),
        "expected the refusal to name the resource and carry no Message member, got {body}"
    );
}
