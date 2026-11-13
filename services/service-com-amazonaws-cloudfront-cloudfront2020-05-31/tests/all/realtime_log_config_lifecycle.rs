// SPDX-License-Identifier: CC0-1.0

//! The CloudFront REAL-TIME LOG CONFIGURATION control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's real-time log
//! configuration surface states:
//!
//! * a configuration is PERSISTED by the call that creates it, which MINTS the ARN it is
//!   addressed by, so a later `GetRealtimeLogConfig` answers from that record rather than
//!   from the request being served — down to the ORDER of the fields, which "are provided
//!   in the same order in which they are sent to the Amazon Kinesis data stream";
//! * a configuration is addressed by its `Name` or by its `ARN`, and "If you provide both,
//!   CloudFront uses the name to identify the real-time log configuration to get" — a
//!   request naming neither addresses nothing ("You must provide at least one") and is
//!   refused with `InvalidArgument`, and an identifier no configuration answers to is
//!   `NoSuchRealtimeLogConfig`;
//! * `SamplingRate` is "a whole number between 1 and 100 (inclusive)", and an endpoint's
//!   `StreamType` has "only valid value ... `Kinesis`" and names the Kinesis stream the
//!   records go to: a request breaking either is refused with `InvalidArgument` and stores
//!   nothing, as is one that names nothing where it has to name something (an empty `Name`,
//!   an empty list) or one past the per-record lengths this emulator retains;
//! * `RealtimeLogConfig$Name` is "The unique name of this real-time log configuration", so
//!   a repeated name is refused with `RealtimeLogConfigAlreadyExists` and leaves the
//!   configuration that holds it exactly as it was;
//! * "When you update a real-time log configuration, all the parameters are updated with
//!   the values provided in the request" — a parameter the request supplies replaces the
//!   stored one, one it leaves out keeps the stored value, and the identity never moves
//!   ("You cannot update a real-time log configuration's `Name` or `ARN`");
//! * a delete removes the configuration, so every later read of it is
//!   `NoSuchRealtimeLogConfig` — except that "You cannot delete a real-time log
//!   configuration if it's attached to a cache behavior", where the delete is refused with
//!   `RealtimeLogConfigInUse` and the configuration stands.
//!
//! The table rows differ along everything these rules generalise over: the sampling rate
//! (each published bound and values between them), the number of endpoints (one, many),
//! the number of fields (one, many) and their order, and whether the name and the field
//! names are plain ASCII, carry punctuation, are non-ASCII or carry XML metacharacters.
//!
//! The harness (the router, the restXml reader and the request helper) is the one in
//! [`crate::distribution_lifecycle`], so these rules go over the same wire path as the
//! rest of the suite.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    KINESIS_STREAM_TYPE, MAX_REALTIME_LOG_CONFIG_END_POINTS, MAX_REALTIME_LOG_CONFIG_FIELDS,
    MAX_SAMPLING_RATE, MIN_SAMPLING_RATE, NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE,
    REALTIME_LOG_CONFIG_ALREADY_EXISTS_MESSAGE, REALTIME_LOG_CONFIG_IDENTIFIER_REQUIRED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, Answer, Node, ACCOUNT_ID, API_VERSION,
};
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the real-time log configuration surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/realtime-log-config` with the request document `body`.
async fn create_config(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/realtime-log-config"),
        Some(body),
        None,
    )
    .await
}

/// `POST /2020-05-31/get-realtime-log-config` naming the configuration by `name`, by
/// `arn`, by both or by neither — each `None` leaves that element out of the document
/// entirely.
///
/// `pub(crate)` so a sibling suite reads a configuration back over the same wire path.
pub(crate) async fn get_config(router: &MultiServiceRouter, name: Option<&str>, arn: Option<&str>) -> Answer {
    let mut body = format!(
        "<GetRealtimeLogConfigRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(name) = name {
        body.push_str(&format!("<Name>{}</Name>", escape(name)));
    }
    if let Some(arn) = arn {
        body.push_str(&format!("<ARN>{}</ARN>", escape(arn)));
    }
    body.push_str("</GetRealtimeLogConfigRequest>");
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/get-realtime-log-config"),
        Some(&body),
        None,
    )
    .await
}

/// `POST /2020-05-31/delete-realtime-log-config`, addressing the configuration the same
/// way a read does.
///
/// `pub(crate)` so a sibling suite removes a configuration over the same wire path.
pub(crate) async fn delete_config(
    router: &MultiServiceRouter,
    name: Option<&str>,
    arn: Option<&str>,
) -> Answer {
    let mut body = format!(
        "<DeleteRealtimeLogConfigRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(name) = name {
        body.push_str(&format!("<Name>{}</Name>", escape(name)));
    }
    if let Some(arn) = arn {
        body.push_str(&format!("<ARN>{}</ARN>", escape(arn)));
    }
    body.push_str("</DeleteRealtimeLogConfigRequest>");
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/delete-realtime-log-config"),
        Some(&body),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/realtime-log-config`: each parameter the request supplies, in the
/// model's member order, under the root element the API reference names.
async fn update_config(
    router: &MultiServiceRouter,
    name: Option<&str>,
    arn: Option<&str>,
    end_points: Option<&[Endpoint]>,
    fields: Option<&[String]>,
    sampling_rate: Option<i64>,
) -> Answer {
    let mut body = format!(
        "<UpdateRealtimeLogConfigRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(end_points) = end_points {
        body.push_str("<EndPoints>");
        for end_point in end_points {
            body.push_str(&end_point.xml());
        }
        body.push_str("</EndPoints>");
    }
    if let Some(fields) = fields {
        body.push_str("<Fields>");
        for field in fields {
            body.push_str(&format!("<Field>{}</Field>", escape(field)));
        }
        body.push_str("</Fields>");
    }
    if let Some(name) = name {
        body.push_str(&format!("<Name>{}</Name>", escape(name)));
    }
    if let Some(arn) = arn {
        body.push_str(&format!("<ARN>{}</ARN>", escape(arn)));
    }
    if let Some(sampling_rate) = sampling_rate {
        body.push_str(&format!("<SamplingRate>{sampling_rate}</SamplingRate>"));
    }
    body.push_str("</UpdateRealtimeLogConfigRequest>");
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/realtime-log-config"),
        Some(&body),
        None,
    )
    .await
}

/// One Kinesis endpoint a configuration sends records to.
#[derive(Debug, Clone)]
pub(crate) struct Endpoint {
    /// `EndPoint$StreamType`.
    stream_type: String,
    /// `KinesisStreamConfig`, or `None` for an endpoint that names no stream at all.
    stream: Option<(String, String)>,
}

impl Endpoint {
    /// A well-formed Kinesis endpoint, distinguished by `label`.
    pub(crate) fn kinesis(label: &str) -> Self {
        Self {
            stream_type: KINESIS_STREAM_TYPE.to_string(),
            stream: Some((
                format!("arn:aws:iam::{ACCOUNT_ID}:role/cf-realtime-log-{label}"),
                format!("arn:aws:kinesis:us-east-1:{ACCOUNT_ID}:stream/{label}"),
            )),
        }
    }

    /// This endpoint as the `<member>` element of an `EndPointList`.
    fn xml(&self) -> String {
        let mut out = format!(
            "<member><StreamType>{}</StreamType>",
            escape(&self.stream_type)
        );
        if let Some((role_arn, stream_arn)) = &self.stream {
            out.push_str(&format!(
                "<KinesisStreamConfig><RoleARN>{}</RoleARN><StreamARN>{}</StreamARN>\
                 </KinesisStreamConfig>",
                escape(role_arn),
                escape(stream_arn)
            ));
        }
        out.push_str("</member>");
        out
    }
}

/// One real-time log configuration a request submits.
#[derive(Debug, Clone)]
pub(crate) struct Submitted {
    /// What this row is about, named in every assertion it drives.
    pub(crate) case: &'static str,
    pub(crate) name: String,
    pub(crate) sampling_rate: i64,
    pub(crate) end_points: Vec<Endpoint>,
    pub(crate) fields: Vec<String>,
}

impl Submitted {
    /// The `CreateRealtimeLogConfigRequest` document a client sends: none of the four
    /// members is bound to the URI or to a header, so each one travels in the body under
    /// the root element the API reference names.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<CreateRealtimeLogConfigRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><EndPoints>"
        );
        for end_point in &self.end_points {
            out.push_str(&end_point.xml());
        }
        out.push_str("</EndPoints><Fields>");
        for field in &self.fields {
            out.push_str(&format!("<Field>{}</Field>", escape(field)));
        }
        out.push_str(&format!(
            "</Fields><Name>{}</Name><SamplingRate>{}</SamplingRate>\
             </CreateRealtimeLogConfigRequest>",
            escape(&self.name),
            self.sampling_rate
        ));
        out
    }
}

/// A list of owned strings from string literals, for the table below.
pub(crate) fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The configurations every rule below is asserted over.
///
/// The rows span the class each rule generalises over: both published bounds of
/// `SamplingRate` and a value between them, one endpoint and several, one field and many,
/// a field order that is not the sorted one, and names and field names that are plain
/// ASCII, carry punctuation, are non-ASCII or carry XML metacharacters.
///
/// `pub(crate)` so a sibling suite asserts its own rules over these same rows rather than
/// carrying a second table.
pub(crate) fn config_table() -> Vec<Submitted> {
    vec![
        Submitted {
            case: "one endpoint, one field, the smallest sampling rate",
            name: "minimal".to_string(),
            sampling_rate: MIN_SAMPLING_RATE,
            end_points: vec![Endpoint::kinesis("minimal")],
            fields: strings(&["timestamp"]),
        },
        Submitted {
            case: "many fields in an order that is not the sorted one, mid-range rate",
            name: "viewer-requests_2".to_string(),
            sampling_rate: 37,
            end_points: vec![Endpoint::kinesis("viewer-requests")],
            fields: strings(&[
                "timestamp",
                "c-ip",
                "sc-status",
                "cs-uri-stem",
                "x-edge-result-type",
            ]),
        },
        Submitted {
            case: "two endpoints and the largest sampling rate",
            name: "fan-out".to_string(),
            sampling_rate: MAX_SAMPLING_RATE,
            end_points: vec![Endpoint::kinesis("primary"), Endpoint::kinesis("secondary")],
            fields: strings(&["timestamp", "sc-bytes"]),
        },
        Submitted {
            case: "a non-ASCII name and a field name carrying XML metacharacters",
            name: "журнал-müller".to_string(),
            sampling_rate: 50,
            end_points: vec![Endpoint::kinesis("unicode")],
            fields: strings(&["timestamp", "x-field<&>\"'", "ünicode-field"]),
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// The `RealtimeLogConfig` element of an answer, wherever the operation's output wraps it.
pub(crate) fn config_node<'a>(answer: &'a Answer, case: &str, context: &str) -> &'a Node {
    answer
        .root()
        .descendant("RealtimeLogConfig")
        .unwrap_or_else(|| {
            panic!(
                "case {case} ({context}): the answer carries no RealtimeLogConfig: {}",
                answer.raw
            )
        })
}

/// Assert the `RealtimeLogConfig` document `node` is the configuration `row` submitted,
/// under the ARN the service mints for its name.
///
/// `pub(crate)` so a sibling suite asserts a configuration it reads another way is the same
/// record, by this one rule.
pub(crate) fn assert_reports(node: &Node, row: &Submitted, context: &str, raw: &str) {
    let case = row.case;
    let expected_arn = format!("arn:aws:cloudfront::{ACCOUNT_ID}:realtime-log-config/{}", row.name);
    let arn = node.text_at("ARN").unwrap_or_default();
    assert_eq!(
        arn, expected_arn,
        "case {case} ({context}): expected ARN {expected_arn}, got {arn:?} in {raw}"
    );
    let name = node.text_at("Name").unwrap_or_default();
    assert_eq!(
        name, row.name,
        "case {case} ({context}): expected Name {}, got {name:?} in {raw}",
        row.name
    );
    let rate = node.text_at("SamplingRate").unwrap_or_default();
    assert_eq!(
        rate,
        row.sampling_rate.to_string(),
        "case {case} ({context}): expected SamplingRate {}, got {rate:?} in {raw}",
        row.sampling_rate
    );

    // The fields come back in the order CloudFront SENDS them in — the catalog's order,
    // which the rows above are already written in — and not sorted, reversed or dropped.
    let reported_fields: Vec<&str> = node
        .at("Fields")
        .map(|fields| {
            fields
                .children
                .iter()
                .map(|field| field.text())
                .collect::<Vec<&str>>()
        })
        .unwrap_or_default();
    let expected_fields: Vec<&str> = row.fields.iter().map(String::as_str).collect();
    assert_eq!(
        reported_fields, expected_fields,
        "case {case} ({context}): expected Fields {expected_fields:?}, got \
         {reported_fields:?} in {raw}"
    );

    // Each endpoint comes back with the stream type and the stream it was submitted with,
    // in the submitted order.
    let reported_end_points: Vec<(String, String, String)> = node
        .at("EndPoints")
        .map(|end_points| {
            end_points
                .children
                .iter()
                .map(|member| {
                    (
                        member.text_at("StreamType").unwrap_or_default().to_string(),
                        member
                            .text_at("KinesisStreamConfig/RoleARN")
                            .unwrap_or_default()
                            .to_string(),
                        member
                            .text_at("KinesisStreamConfig/StreamARN")
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect::<Vec<(String, String, String)>>()
        })
        .unwrap_or_default();
    let expected_end_points: Vec<(String, String, String)> = row
        .end_points
        .iter()
        .map(|end_point| {
            let (role_arn, stream_arn) = end_point.stream.clone().unwrap_or_default();
            (end_point.stream_type.clone(), role_arn, stream_arn)
        })
        .collect();
    assert_eq!(
        reported_end_points, expected_end_points,
        "case {case} ({context}): expected EndPoints {expected_end_points:?}, got \
         {reported_end_points:?} in {raw}"
    );
}

/// Create `row` and report the answer, asserting it was accepted.
pub(crate) async fn create_accepted(router: &MultiServiceRouter, row: &Submitted) -> Answer {
    let created = create_config(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateRealtimeLogConfig, got {} with {}",
        row.case, created.status, created.raw
    );
    created
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create stores the configuration and reports the record it stored, and every way of
/// addressing it afterwards answers from that one record: by name, by ARN, and by both at
/// once (where the name identifies it).
#[tokio::test]
async fn a_create_persists_the_configuration_every_read_answers_from() {
    let router = emulator();
    for row in config_table() {
        let created = create_accepted(&router, &row).await;
        assert_reports(
            config_node(&created, row.case, "the create's answer"),
            &row,
            "the create's answer",
            &created.raw,
        );
        let arn = config_node(&created, row.case, "the create's answer")
            .text_at("ARN")
            .unwrap_or_default()
            .to_string();

        for (context, name, by_arn) in [
            ("read back by Name", Some(row.name.as_str()), None),
            ("read back by ARN", None, Some(arn.as_str())),
            (
                "read back by Name and ARN together",
                Some(row.name.as_str()),
                Some(arn.as_str()),
            ),
        ] {
            let got = get_config(&router, name, by_arn).await;
            assert_eq!(
                got.status, 200,
                "case {} ({context}): expected HTTP 200 from GetRealtimeLogConfig, got {} \
                 with {}",
                row.case, got.status, got.raw
            );
            assert_reports(
                config_node(&got, row.case, context),
                &row,
                context,
                &got.raw,
            );
        }
    }
}

/// Every configuration in the table is readable after the whole table has been created, so
/// one create neither replaces nor disturbs another's record.
#[tokio::test]
async fn configurations_are_independent_records() {
    let router = emulator();
    let table = config_table();
    for row in &table {
        create_accepted(&router, row).await;
    }
    for row in &table {
        let got = get_config(&router, Some(row.name.as_str()), None).await;
        assert_eq!(
            got.status, 200,
            "case {}: expected HTTP 200 reading one of {} configurations back, got {} with {}",
            row.case,
            table.len(),
            got.status,
            got.raw
        );
        assert_reports(
            config_node(&got, row.case, "read back after the whole table"),
            row,
            "read back after the whole table",
            &got.raw,
        );
    }
}

/// A name another configuration already holds names no second one: the create is refused
/// with `RealtimeLogConfigAlreadyExists` and the configuration holding the name is left
/// exactly as it was.
#[tokio::test]
async fn a_repeated_name_is_refused_and_changes_nothing() {
    let router = emulator();
    for row in config_table() {
        create_accepted(&router, &row).await;

        // The same name with everything else different: nothing of this request may reach
        // the stored record.
        let mut again = row.clone();
        again.sampling_rate = if row.sampling_rate == MAX_SAMPLING_RATE {
            MIN_SAMPLING_RATE
        } else {
            MAX_SAMPLING_RATE
        };
        again.fields = strings(&["cs-method"]);
        again.end_points = vec![Endpoint::kinesis("replacement")];
        let refused = create_config(&router, &again.xml()).await;
        assert_eq!(
            refused.status, 409,
            "case {}: expected HTTP 409 for a repeated name, got {} with {}",
            row.case, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "RealtimeLogConfigAlreadyExists",
            "case {}: expected RealtimeLogConfigAlreadyExists for a repeated name, got {}",
            row.case,
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, REALTIME_LOG_CONFIG_ALREADY_EXISTS_MESSAGE,
            "case {}: expected the served already-exists message, got {message:?} in {}",
            row.case, refused.raw
        );

        let got = get_config(&router, Some(row.name.as_str()), None).await;
        assert_reports(
            config_node(&got, row.case, "after a refused repeat"),
            &row,
            "after a refused repeat",
            &got.raw,
        );
    }
}

/// The sampling rates outside the documented range, each with what makes it no percentage
/// of viewer requests. The two bounds themselves are accepted, which the table above
/// covers.
const REJECTED_SAMPLING_RATES: [i64; 5] = [0, -1, -100, 101, 1_000];

/// A sampling rate outside `1..=100` is refused with `InvalidArgument` and stores nothing.
#[tokio::test]
async fn a_sampling_rate_outside_the_documented_range_is_refused() {
    let router = emulator();
    for row in config_table() {
        for (index, rate) in REJECTED_SAMPLING_RATES.into_iter().enumerate() {
            let mut submitted = row.clone();
            submitted.name = format!("{}-rate-{index}", row.name);
            submitted.sampling_rate = rate;
            let refused = create_config(&router, &submitted.xml()).await;
            assert_eq!(
                refused.status, 400,
                "case {} (SamplingRate {rate}): expected HTTP 400, got {} with {}",
                row.case, refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "InvalidArgument",
                "case {} (SamplingRate {rate}): expected InvalidArgument, got {}",
                row.case,
                refused.raw
            );

            // A refused create stores nothing, so the name it named is still free.
            let got = get_config(&router, Some(submitted.name.as_str()), None).await;
            assert_eq!(
                got.status, 404,
                "case {} (SamplingRate {rate}): expected the refused create to store \
                 nothing, got HTTP {} with {}",
                row.case, got.status, got.raw
            );
        }
    }
}

/// An endpoint that is no Kinesis destination is refused with `InvalidArgument` and stores
/// nothing: a stream type other than the one valid value, whatever its case, and a Kinesis
/// endpoint naming no stream at all.
#[tokio::test]
async fn an_endpoint_that_is_no_kinesis_destination_is_refused() {
    let router = emulator();
    let broken: [(&str, Endpoint); 4] = [
        (
            "a stream type no member admits",
            Endpoint {
                stream_type: "Firehose".to_string(),
                ..Endpoint::kinesis("firehose")
            },
        ),
        (
            "the valid value in the wrong case",
            Endpoint {
                stream_type: KINESIS_STREAM_TYPE.to_lowercase(),
                ..Endpoint::kinesis("lowercase")
            },
        ),
        (
            "an empty stream type",
            Endpoint {
                stream_type: String::new(),
                ..Endpoint::kinesis("empty")
            },
        ),
        (
            "a Kinesis endpoint naming no stream",
            Endpoint {
                stream_type: KINESIS_STREAM_TYPE.to_string(),
                stream: None,
            },
        ),
    ];

    for row in config_table() {
        for (index, (why, end_point)) in broken.iter().enumerate() {
            let mut submitted = row.clone();
            submitted.name = format!("{}-endpoint-{index}", row.name);
            // The broken endpoint LAST, so a row with several endpoints proves every one
            // of them is checked rather than only the first.
            submitted.end_points.push(end_point.clone());
            let refused = create_config(&router, &submitted.xml()).await;
            assert_eq!(
                refused.status, 400,
                "case {} ({why}): expected HTTP 400, got {} with {}",
                row.case, refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "InvalidArgument",
                "case {} ({why}): expected InvalidArgument, got {}",
                row.case,
                refused.raw
            );

            let got = get_config(&router, Some(submitted.name.as_str()), None).await;
            assert_eq!(
                got.status, 404,
                "case {} ({why}): expected the refused create to store nothing, got HTTP \
                 {} with {}",
                row.case, got.status, got.raw
            );
        }
    }
}

/// A read that names no identifier addresses no configuration at all, and is refused with
/// `InvalidArgument` whatever the store holds.
#[tokio::test]
async fn a_read_naming_no_identifier_is_refused() {
    let router = emulator();
    for (context, name, arn) in [
        ("neither element present", None, None),
        ("an empty Name element", Some(""), None),
        ("an empty ARN element", None, Some("")),
        ("both elements empty", Some(""), Some("")),
    ] {
        let refused = get_config(&router, name, arn).await;
        assert_eq!(
            refused.status, 400,
            "case {context}: expected HTTP 400 from a read naming no identifier, got {} \
             with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {context}: expected InvalidArgument, got {}",
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, REALTIME_LOG_CONFIG_IDENTIFIER_REQUIRED_MESSAGE,
            "case {context}: expected the served missing-identifier message, got \
             {message:?} in {}",
            refused.raw
        );
    }

    // The same refusal once the store holds configurations: it is a property of the
    // request, not of the store.
    for row in config_table() {
        create_accepted(&router, &row).await;
    }
    let refused = get_config(&router, None, None).await;
    assert_eq!(
        refused.error_code(),
        "InvalidArgument",
        "case a populated store: expected InvalidArgument from a read naming no \
         identifier, got {}",
        refused.raw
    );
}

/// An identifier no stored configuration answers to is `NoSuchRealtimeLogConfig`, whatever
/// form the identifier takes.
#[tokio::test]
async fn an_identifier_no_configuration_holds_is_not_found() {
    let router = emulator();
    for row in config_table() {
        create_accepted(&router, &row).await;
    }

    let unknown_arn = format!("arn:aws:cloudfront::{ACCOUNT_ID}:realtime-log-config/not-created");
    let unknown: [(&str, Option<&str>, Option<&str>); 4] = [
        ("a name no configuration holds", Some("not-created"), None),
        (
            "a name that differs from a stored one only in case",
            Some("MINIMAL"),
            None,
        ),
        (
            "an ARN of the right shape for a name nothing holds",
            None,
            Some(unknown_arn.as_str()),
        ),
        ("an ARN that is no ARN at all", None, Some("not-an-arn")),
    ];
    for (context, name, arn) in unknown {
        let answer = get_config(&router, name, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {context}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchRealtimeLogConfig",
            "case {context}: expected NoSuchRealtimeLogConfig, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE,
            "case {context}: expected the modelled message, got {message:?} in {}",
            answer.raw
        );
    }
}

/// A delete removes the configuration, whichever way the request addresses it: every later
/// read answers `NoSuchRealtimeLogConfig`, a second delete of the same configuration does
/// too, and the name it held is free for a new configuration.
#[tokio::test]
async fn a_delete_removes_the_configuration_however_it_is_addressed() {
    let router = emulator();
    // The three ways a request addresses one configuration, each driven over every row.
    let addressings: [&str; 3] = ["by Name", "by ARN", "by Name and ARN together"];
    for row in config_table() {
        for addressing in addressings {
            let created = create_accepted(&router, &row).await;
            let arn = config_node(&created, row.case, addressing)
                .text_at("ARN")
                .unwrap_or_default()
                .to_string();
            let (name, by_arn) = match addressing {
                "by Name" => (Some(row.name.as_str()), None),
                "by ARN" => (None, Some(arn.as_str())),
                _ => (Some(row.name.as_str()), Some(arn.as_str())),
            };

            let deleted = delete_config(&router, name, by_arn).await;
            assert_eq!(
                deleted.status, 204,
                "case {} ({addressing}): expected HTTP 204 from DeleteRealtimeLogConfig, \
                 got {} with {}",
                row.case, deleted.status, deleted.raw
            );

            // Gone from every read, by either identifier.
            for (context, read_name, read_arn) in [
                ("read by Name after the delete", Some(row.name.as_str()), None),
                ("read by ARN after the delete", None, Some(arn.as_str())),
            ] {
                let got = get_config(&router, read_name, read_arn).await;
                assert_eq!(
                    got.status, 404,
                    "case {} ({addressing}, {context}): expected HTTP 404, got {} with {}",
                    row.case, got.status, got.raw
                );
            }

            // And a second delete of the same configuration finds nothing to delete.
            let again = delete_config(&router, name, by_arn).await;
            assert_eq!(
                again.error_code(),
                "NoSuchRealtimeLogConfig",
                "case {} ({addressing}): expected NoSuchRealtimeLogConfig from a repeated \
                 delete, got {}",
                row.case,
                again.raw
            );
        }
        // The last iteration left the name free, which is what lets the loop above create
        // the same row again: assert it explicitly once.
        let recreated = create_config(&router, &row.xml()).await;
        assert_eq!(
            recreated.status, 201,
            "case {}: expected the deleted name to be free again, got {} with {}",
            row.case, recreated.status, recreated.raw
        );
    }
}

/// A delete that names no identifier addresses no configuration, and is refused with
/// `InvalidArgument` without removing anything.
#[tokio::test]
async fn a_delete_naming_no_identifier_is_refused_and_removes_nothing() {
    let router = emulator();
    let table = config_table();
    for row in &table {
        create_accepted(&router, row).await;
    }

    for (context, name, arn) in [
        ("neither element present", None, None),
        ("an empty Name element", Some(""), None),
        ("both elements empty", Some(""), Some("")),
    ] {
        let refused = delete_config(&router, name, arn).await;
        assert_eq!(
            refused.status, 400,
            "case {context}: expected HTTP 400 from a delete naming no identifier, got {} \
             with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {context}: expected InvalidArgument, got {}",
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, REALTIME_LOG_CONFIG_IDENTIFIER_REQUIRED_MESSAGE,
            "case {context}: expected the served missing-identifier message, got \
             {message:?} in {}",
            refused.raw
        );
    }

    // Nothing was removed by any of those refusals.
    for row in &table {
        let got = get_config(&router, Some(row.name.as_str()), None).await;
        assert_eq!(
            got.status, 200,
            "case {}: expected the refused deletes to remove nothing, got HTTP {} with {}",
            row.case, got.status, got.raw
        );
    }
}

/// The `DistributionConfig` of a distribution whose cache behaviours attach `arn`.
///
/// `attach_default` attaches it to the DEFAULT cache behaviour and `attach_ordered` to an
/// ordered one, so the rule can be asserted over each place a behaviour can carry it. The
/// element order is the model's member order for each behaviour.
fn distribution_with_realtime_log(
    reference: &str,
    arn: &str,
    attach_default: bool,
    attach_ordered: bool,
) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin><Id>primary</Id>\
         <DomainName>origin.example.com</DomainName><CustomOriginConfig><HTTPPort>80</HTTPPort>\
         <HTTPSPort>443</HTTPSPort><OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         </CustomOriginConfig></Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>"
    );
    if attach_default {
        out.push_str(&format!(
            "<RealtimeLogConfigArn>{}</RealtimeLogConfigArn>",
            escape(arn)
        ));
    }
    out.push_str("</DefaultCacheBehavior>");
    if attach_ordered {
        out.push_str(&format!(
            "<CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
             <PathPattern>/images/*</PathPattern><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy>\
             <RealtimeLogConfigArn>{}</RealtimeLogConfigArn>\
             </CacheBehavior></Items></CacheBehaviors>",
            escape(arn)
        ));
    }
    out.push_str(
        "<Comment>attaches a real-time log configuration</Comment>\
         <Enabled>true</Enabled></DistributionConfig>",
    );
    out
}

/// "You cannot delete a real-time log configuration if it's attached to a cache behavior":
/// a configuration a distribution still sends its logs through survives the delete, while
/// one no behaviour attaches is deleted — asserted for an attachment on the default cache
/// behaviour and on an ordered one.
#[tokio::test]
async fn a_configuration_attached_to_a_cache_behaviour_is_not_deleted() {
    let router = emulator();
    let attachments: [(&str, bool, bool); 3] = [
        ("attached to the default cache behaviour", true, false),
        ("attached to an ordered cache behaviour", false, true),
        ("attached to both", true, true),
    ];

    for (index, (context, attach_default, attach_ordered)) in attachments.into_iter().enumerate()
    {
        // The attached configuration, and a second one nothing attaches.
        let attached = Submitted {
            case: "the attached configuration",
            name: format!("attached-{index}"),
            sampling_rate: 100,
            end_points: vec![Endpoint::kinesis("attached")],
            fields: strings(&["timestamp", "c-ip"]),
        };
        let detached = Submitted {
            case: "the configuration nothing attaches",
            name: format!("detached-{index}"),
            ..attached.clone()
        };
        let created = create_accepted(&router, &attached).await;
        create_accepted(&router, &detached).await;
        let arn = config_node(&created, attached.case, context)
            .text_at("ARN")
            .unwrap_or_default()
            .to_string();

        let distribution = create_distribution(
            &router,
            &distribution_with_realtime_log(
                &format!("ref-rtlog-{index}"),
                &arn,
                attach_default,
                attach_ordered,
            ),
        )
        .await;
        assert_eq!(
            distribution.status, 201,
            "case {context}: expected HTTP 201 from CreateDistribution, got {} with {}",
            distribution.status, distribution.raw
        );

        let refused = delete_config(&router, Some(attached.name.as_str()), None).await;
        assert_eq!(
            refused.status, 400,
            "case {context}: expected HTTP 400 deleting an attached configuration, got {} \
             with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "RealtimeLogConfigInUse",
            "case {context}: expected RealtimeLogConfigInUse, got {}",
            refused.raw
        );
        // The refusal left the configuration exactly as it was.
        let got = get_config(&router, Some(attached.name.as_str()), None).await;
        assert_reports(
            config_node(&got, attached.case, context),
            &attached,
            context,
            &got.raw,
        );

        // The rule is about the ATTACHMENT, not about the store holding a distribution: the
        // configuration no behaviour names is deleted while that distribution stands.
        let deleted = delete_config(&router, Some(detached.name.as_str()), None).await;
        assert_eq!(
            deleted.status, 204,
            "case {context}: expected HTTP 204 deleting an unattached configuration, got \
             {} with {}",
            deleted.status, deleted.raw
        );
    }
}

/// A configuration that names nothing where it has to name something is refused with
/// `InvalidArgument` and stores nothing: an empty `Name` (which identifies no
/// configuration), an `EndPoints` list with no endpoint (no destination for the log data)
/// and a `Fields` list with no field (no content for a log record).
#[tokio::test]
async fn a_configuration_that_names_nothing_is_refused() {
    let router = emulator();
    let base = Submitted {
        case: "the named configuration",
        name: "named".to_string(),
        sampling_rate: 10,
        end_points: vec![Endpoint::kinesis("named")],
        fields: strings(&["timestamp"]),
    };

    for (context, submitted) in [
        (
            "an empty Name",
            Submitted {
                name: String::new(),
                ..base.clone()
            },
        ),
        (
            "an EndPoints list naming no endpoint",
            Submitted {
                name: "no-endpoints".to_string(),
                end_points: Vec::new(),
                ..base.clone()
            },
        ),
        (
            "a Fields list naming no field",
            Submitted {
                name: "no-fields".to_string(),
                fields: Vec::new(),
                ..base.clone()
            },
        ),
    ] {
        let refused = create_config(&router, &submitted.xml()).await;
        assert_eq!(
            refused.status, 400,
            "case {context}: expected HTTP 400, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {context}: expected InvalidArgument, got {}",
            refused.raw
        );
    }

    // The same two list rules on the update path, which shares them.
    create_accepted(&router, &base).await;
    for (context, end_points, fields) in [
        ("updating to no endpoint", Some(Vec::new()), None),
        ("updating to no field", None, Some(Vec::new())),
    ] {
        let refused = update_config(
            &router,
            Some(base.name.as_str()),
            None,
            end_points.as_deref(),
            fields.as_deref(),
            None,
        )
        .await;
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {context}: expected InvalidArgument, got {}",
            refused.raw
        );
    }
    // And the refused updates left the record as the create stored it.
    let got = get_config(&router, Some(base.name.as_str()), None).await;
    assert_reports(
        config_node(&got, base.case, "after the refused updates"),
        &base,
        "after the refused updates",
        &got.raw,
    );
}

/// A configuration larger than this emulator retains is refused with `InvalidArgument` and
/// stores nothing: more endpoints, or more fields, than the record is bounded by. The
/// bounds themselves are accepted, which each case below asserts beside its refusal.
#[tokio::test]
async fn a_configuration_past_the_retained_bounds_is_refused() {
    let router = emulator();
    let base = Submitted {
        case: "the bounded configuration",
        name: "bounds".to_string(),
        sampling_rate: 10,
        end_points: vec![Endpoint::kinesis("bounds")],
        fields: strings(&["timestamp"]),
    };

    // (what the case varies, the length that is accepted, the length that is refused)
    let cases: [(&str, usize, usize); 2] = [
        (
            "endpoints",
            MAX_REALTIME_LOG_CONFIG_END_POINTS,
            MAX_REALTIME_LOG_CONFIG_END_POINTS + 1,
        ),
        (
            "fields",
            MAX_REALTIME_LOG_CONFIG_FIELDS,
            MAX_REALTIME_LOG_CONFIG_FIELDS + 1,
        ),
    ];

    for (what, at_bound, past_bound) in cases {
        for (length, expect_accepted) in [(at_bound, true), (past_bound, false)] {
            let mut submitted = base.clone();
            submitted.name = format!("{what}-{length}");
            if what == "endpoints" {
                submitted.end_points = (0..length)
                    .map(|index| Endpoint::kinesis(&format!("stream-{index}")))
                    .collect();
            } else {
                submitted.fields = (0..length).map(|index| format!("field-{index}")).collect();
            }

            let answer = create_config(&router, &submitted.xml()).await;
            if expect_accepted {
                assert_eq!(
                    answer.status, 201,
                    "case {length} {what} (the bound itself): expected HTTP 201, got {} with {}",
                    answer.status, answer.raw
                );
                assert_reports(
                    config_node(&answer, "the bound itself", what),
                    &submitted,
                    what,
                    &answer.raw,
                );
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {length} {what} (one past the bound): expected HTTP 400, got {} \
                     with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidArgument",
                    "case {length} {what} (one past the bound): expected InvalidArgument, got {}",
                    answer.raw
                );
                let got = get_config(&router, Some(submitted.name.as_str()), None).await;
                assert_eq!(
                    got.status, 404,
                    "case {length} {what}: expected the refused create to store nothing, got \
                     HTTP {} with {}",
                    got.status, got.raw
                );
            }
        }
    }
}

/// An update replaces the parameters the request supplies, keeps the ones it leaves out,
/// never changes the identity, and is what every later read answers from.
#[tokio::test]
async fn an_update_replaces_the_parameters_it_supplies_and_keeps_the_rest() {
    let router = emulator();
    let table = config_table();
    // Each row is updated TOWARDS the next row's parameters, so the values written differ
    // along everything the rule generalises over rather than being one edit repeated.
    for (index, row) in table.iter().enumerate() {
        let target = &table[(index + 1) % table.len()];
        create_accepted(&router, row).await;

        for (context, rate, end_points, fields) in [
            ("the sampling rate alone", Some(target.sampling_rate), None, None),
            (
                "the endpoints alone",
                None,
                Some(target.end_points.clone()),
                None,
            ),
            ("the fields alone", None, None, Some(target.fields.clone())),
            (
                "every parameter at once",
                Some(target.sampling_rate),
                Some(target.end_points.clone()),
                Some(target.fields.clone()),
            ),
        ] {
            // What the record should hold afterwards: the supplied parameters, and the
            // stored ones for everything the request leaves out. The identity never moves.
            let expected = Submitted {
                case: row.case,
                name: row.name.clone(),
                sampling_rate: rate.unwrap_or(row.sampling_rate),
                end_points: end_points.clone().unwrap_or_else(|| row.end_points.clone()),
                fields: fields.clone().unwrap_or_else(|| row.fields.clone()),
            };

            let updated = update_config(
                &router,
                Some(row.name.as_str()),
                None,
                end_points.as_deref(),
                fields.as_deref(),
                rate,
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {} ({context}): expected HTTP 200 from UpdateRealtimeLogConfig, got \
                 {} with {}",
                row.case, updated.status, updated.raw
            );
            assert_reports(
                config_node(&updated, row.case, context),
                &expected,
                context,
                &updated.raw,
            );

            // And the read answers from the updated record.
            let got = get_config(&router, Some(row.name.as_str()), None).await;
            assert_reports(
                config_node(&got, row.case, context),
                &expected,
                context,
                &got.raw,
            );

            // Restore the row's own parameters, so each case above starts from them.
            update_config(
                &router,
                Some(row.name.as_str()),
                None,
                Some(&row.end_points),
                Some(&row.fields),
                Some(row.sampling_rate),
            )
            .await;
        }
    }
}

/// An update is refused exactly where a create is: a sampling rate outside the documented
/// range, an endpoint that is no Kinesis destination, an identifier no configuration
/// answers to, and a request naming no identifier — and a refused update leaves the record
/// exactly as it was.
#[tokio::test]
async fn an_update_is_refused_by_the_same_rules_as_a_create() {
    let router = emulator();
    for row in config_table() {
        create_accepted(&router, &row).await;

        for rate in REJECTED_SAMPLING_RATES {
            let refused =
                update_config(&router, Some(row.name.as_str()), None, None, None, Some(rate)).await;
            assert_eq!(
                refused.error_code(),
                "InvalidArgument",
                "case {} (SamplingRate {rate}): expected InvalidArgument from the update, \
                 got {}",
                row.case,
                refused.raw
            );
        }

        let broken = [Endpoint {
            stream_type: "Firehose".to_string(),
            ..Endpoint::kinesis("firehose")
        }];
        let refused = update_config(
            &router,
            Some(row.name.as_str()),
            None,
            Some(&broken),
            None,
            None,
        )
        .await;
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {}: expected InvalidArgument updating to a non-Kinesis endpoint, got {}",
            row.case,
            refused.raw
        );

        // Every refusal above left the record as the create stored it.
        let got = get_config(&router, Some(row.name.as_str()), None).await;
        assert_reports(
            config_node(&got, row.case, "after the refused updates"),
            &row,
            "after the refused updates",
            &got.raw,
        );
    }

    let unknown = update_config(&router, Some("never-created"), None, None, None, Some(10)).await;
    assert_eq!(
        unknown.error_code(),
        "NoSuchRealtimeLogConfig",
        "case an identifier nothing holds: expected NoSuchRealtimeLogConfig from the \
         update, got {}",
        unknown.raw
    );
    let unaddressed = update_config(&router, None, None, None, None, Some(10)).await;
    assert_eq!(
        unaddressed.error_code(),
        "InvalidArgument",
        "case no identifier: expected InvalidArgument from the update, got {}",
        unaddressed.raw
    );
    let message = unaddressed.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, REALTIME_LOG_CONFIG_IDENTIFIER_REQUIRED_MESSAGE,
        "case no identifier: expected the served missing-identifier message, got \
         {message:?} in {}",
        unaddressed.raw
    );
}
