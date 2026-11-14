// SPDX-License-Identifier: CC0-1.0

//! The SERVICE EXECUTION ROLE a channel's create names, judged against the IAM records of
//! the account the create was made in, end to end over the wire with Kinesis and IAM mounted
//! on ONE engine — the shape `core/router/src/lib.rs` mounts them in, so the trust document
//! the decision reads is one IAM's own handlers wrote.
//!
//! RULE: a channel delivers AS the role its `ServiceExecutionRoleARN` names, so
//! `CreateChannel` resolves that role against IAM before it provisions anything and refuses
//! the create HTTP 400 `ValidationException`, with the fixed sentence the service publishes
//! for it, when IAM ANSWERED that the role is one this service cannot assume: this account's
//! IAM holds no role under the name the ARN carries, or the trust policy of the role it does
//! hold does not admit the Kinesis service principal with `sts:AssumeRole`. A refused create
//! leaves NO channel behind. Where nothing is KNOWN about the role the create is SERVED — an
//! emulator with no IAM mounted to ask is the complement the last test pins.
//!
//! SECOND RULE, ahead of the first: what the request says ON ITS OWN is judged before the
//! role is resolved and before anything the request says about the STREAMS it names, so a
//! create naming no destination configuration or both of them is refused for that, and a
//! create whose role this service cannot assume is refused for the role whatever state the
//! streams it names are in — the role is resolved before the source streams are, not after.
//!
//! The rows differ along what the rules generalise over: whether this account's IAM holds the
//! role at all, what its trust policy says (this principal admitted, admitted beside another,
//! admitted through an action wildcard, another principal only, this principal denied, this
//! principal granted some other action), which destination members the request carries, and
//! what state the source stream the request names is in.
//!
//! Every Kinesis case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>`), IAM its awsQuery one, through
//! `MultiServiceRouter::dispatch`, so the statuses, bodies and modelled errors asserted here
//! are the ones a client sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    CHANNEL_ACTIVE, STREAM_ACTIVE, STREAM_MODE_ON_DEMAND,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{
    dead_letter_queue, description, member, storage_configuration, stream_configuration,
    table_bucket_arn,
};
use crate::stream_tags::{error_code, error_message, poll_until, stream_arn, REGION};

/// The `X-Amz-Target` prefix this service's awsJson1_1 operations are addressed by.
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The sentence the service states for a service execution role it cannot assume, written
/// out here rather than borrowed from the crate so the wording itself is what the cases pin.
const UNASSUMABLE_ROLE_SENTENCE: &str =
    "Unable to assume the service execution role. Verify that the role exists and the trust \
     policy allows Kinesis service principal and sts:AssumeRole permissions.";

/// The sentence the service states for a create that does not name exactly one destination
/// configuration, written out here for the same reason.
const DESTINATION_SENTENCE: &str =
    "Exactly one of S3DestinationConfiguration or S3TablesDestinationConfiguration is \
     required.";

/// The service principal a channel's service execution role has to trust.
const KINESIS_PRINCIPAL: &str = "kinesis.amazonaws.com";

/// Another service's principal, for the rows whose trust policy names the wrong one.
const OTHER_PRINCIPAL: &str = "firehose.amazonaws.com";

/// Kinesis and IAM over ONE engine, so the role a create names is the record IAM's own
/// handlers hold.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// Both services mounted, which is what lets a create resolve a role.
    fn new() -> Self {
        Self::assembled(true)
    }

    /// Kinesis alone, with no IAM to ask: nothing is known about any role, which is the
    /// complement the last test pins.
    fn without_iam() -> Self {
        Self::assembled(false)
    }

    fn assembled(with_iam: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated; the
        // router still routes by the SigV4 credential scope, which is where the request's
        // region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if with_iam {
            router.register_service(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                    engine,
                ),
            );
        }
        Self { router }
    }

    /// One Kinesis call, signed for `REGION`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("kinesis.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One Kinesis call that must be answered 200.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body {response}"
        );
        response
    }

    /// One awsQuery call to IAM, answering the XML body it served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from IAM {action}, got {status} with {text}"
        );
        text
    }

    /// A role of this account carrying `trust` as its trust policy, as the ARN IAM persisted.
    async fn role(&self, case: &str, name: &str, trust: &Value) -> String {
        let created = self
            .iam(
                case,
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        xml_element(&created, "Arn").unwrap_or_else(|| {
            panic!("case {case}: CreateRole answers the role Arn, got {created}")
        })
    }

    /// An `ACTIVE` on-demand stream named `name` — the only capacity mode a channel supports.
    /// Answers its ARN.
    async fn on_demand_stream(&self, case: &str, name: &str) -> String {
        self.creating_stream(case, name).await;
        poll_until(
            case,
            &format!("the stream {name} reports {STREAM_ACTIVE}"),
            || async {
                let (status, response) = self
                    .call("DescribeStreamSummary", &json!({ "StreamName": name }))
                    .await;
                status == 200
                    && response["StreamDescriptionSummary"]["StreamStatus"].as_str()
                        == Some(STREAM_ACTIVE)
            },
        )
        .await;
        stream_arn(REGION, name)
    }

    /// An on-demand stream named `name`, WITHOUT waiting for the transition its create armed:
    /// the stream has not reached `ACTIVE` when this returns. Answers its ARN.
    async fn creating_stream(&self, case: &str, name: &str) -> String {
        self.ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": name,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
        stream_arn(REGION, name)
    }

    /// Whether `ListChannels` shows a channel named `name`.
    async fn lists_channel(&self, case: &str, name: &str) -> bool {
        let listed = self.ok(case, "ListChannels", &json!({})).await;
        listed["ChannelSummaries"]
            .as_array()
            .is_some_and(|summaries| {
                summaries
                    .iter()
                    .any(|summary| summary["ChannelName"].as_str() == Some(name))
            })
    }
}

/// `value` as an `application/x-www-form-urlencoded` field: everything but the unreserved
/// characters percent-encoded, which is how an awsQuery client writes a policy document into
/// a form body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A trust policy with one statement: `effect` for the principals `principal` names,
/// over the actions `action` names.
fn trust(effect: &str, principal: Value, action: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }]
    })
}

/// The trust policy a role this service can deliver under carries.
fn trusted_by_kinesis() -> Value {
    trust("Allow", json!(KINESIS_PRINCIPAL), json!("sts:AssumeRole"))
}

/// A create body naming `role` as its service execution role and `source` as its one source
/// stream, with every other member the row that succeeds carries.
fn create_body(name: &str, role: &str, source: &str) -> Value {
    json!({
        "ChannelName": name,
        "ServiceExecutionRoleARN": role,
        "StreamConfigurationList": [stream_configuration(source, "JSON", None)],
        "S3DestinationConfiguration": {
            "StorageConfiguration": storage_configuration("channel-role-bucket"),
        },
    })
}

/// One row of the role table: the trust document IAM holds for the role, and whether the
/// create is served.
struct Row {
    case: &'static str,
    /// The trust policy the role is created with, `None` for a role IAM holds nothing under.
    trust: Option<Value>,
    /// Whether that document lets this service assume the role.
    served: bool,
}

/// The rows the rule is checked over. The trust-document readings they span are IAM's own
/// (`service_common::iam_trust_policy`): the principal named as a string or in a list, the
/// action named exactly or through a wildcard, and an explicit deny winning over any allow.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "channel-role-trusted-by-this-service",
            trust: Some(trusted_by_kinesis()),
            served: true,
        },
        Row {
            case: "channel-role-trusted-through-an-action-wildcard",
            trust: Some(trust("Allow", json!(KINESIS_PRINCIPAL), json!("sts:*"))),
            served: true,
        },
        Row {
            case: "channel-role-trusted-beside-another-principal",
            trust: Some(trust(
                "Allow",
                json!([OTHER_PRINCIPAL, KINESIS_PRINCIPAL]),
                json!("sts:AssumeRole"),
            )),
            served: true,
        },
        Row {
            case: "channel-role-trusting-another-service-only",
            trust: Some(trust(
                "Allow",
                json!(OTHER_PRINCIPAL),
                json!("sts:AssumeRole"),
            )),
            served: false,
        },
        Row {
            case: "channel-role-denying-this-service",
            trust: Some(trust(
                "Deny",
                json!(KINESIS_PRINCIPAL),
                json!("sts:AssumeRole"),
            )),
            served: false,
        },
        Row {
            case: "channel-role-granted-another-action-only",
            trust: Some(trust(
                "Allow",
                json!(KINESIS_PRINCIPAL),
                json!("sts:TagSession"),
            )),
            served: false,
        },
        Row {
            case: "channel-role-iam-holds-nothing-under",
            trust: None,
            served: false,
        },
    ]
}

/// The ARN a role named `name` of this account carries, for the row IAM holds no role for.
fn role_arn_of(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::EMULATOR_ACCOUNT_ID)
}

/// The ARN a channel named `name` of this account and region is addressed by.
fn channel_arn_of(name: &str) -> String {
    format!(
        "arn:aws:kinesis:{REGION}:{}:channel/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The create is served exactly when IAM's records let this service assume the role, and a
/// refused create leaves no channel behind.
#[tokio::test]
async fn a_create_is_served_only_where_iam_lets_this_service_assume_the_role() {
    let emulator = Emulator::new();
    let source = emulator
        .on_demand_stream("channel-role-source", "channel-role-source")
        .await;

    for row in rows() {
        let case = row.case;
        let role = match row.trust.as_ref() {
            Some(trust) => emulator.role(case, case, trust).await,
            None => role_arn_of(case),
        };
        let body = create_body(case, &role, &source);
        let (status, response) = emulator.call("CreateChannel", &body).await;

        if row.served {
            assert_eq!(
                status, 200,
                "case {case}: expected the create to be served, got {status} with body \
                 {response}"
            );
            assert_eq!(
                member(case, &description(case, &response), "ChannelName"),
                case,
                "case {case}: expected the created channel reported back, got {response}"
            );
            // The channel the create minted is a real one: it settles and reads back under
            // the role the create named.
            poll_until(case, "the channel reports ACTIVE", || async {
                let (status, read) = emulator
                    .call(
                        "DescribeChannel",
                        &json!({ "ChannelARN": channel_arn_of(case) }),
                    )
                    .await;
                status == 200
                    && read["ChannelDescription"]["ChannelStatus"].as_str() == Some(CHANNEL_ACTIVE)
            })
            .await;
            let read = emulator
                .ok(
                    case,
                    "DescribeChannel",
                    &json!({ "ChannelARN": channel_arn_of(case) }),
                )
                .await;
            let reported = member(
                case,
                &description(case, &read),
                "ServiceExecutionRoleARN",
            );
            assert_eq!(
                reported, role,
                "case {case}: expected the role the create named reported back, got {reported}"
            );
            assert!(
                emulator.lists_channel(case, case).await,
                "case {case}: expected the created channel listed"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            UNASSUMABLE_ROLE_SENTENCE,
            "case {case}: expected the sentence the service states for a role it cannot \
             assume, got {}",
            error_message(&response)
        );
        assert!(
            !emulator.lists_channel(case, case).await,
            "case {case}: a refused create leaves no channel behind"
        );
    }
}

/// The role is resolved BEFORE anything the request says about its source streams: a create
/// whose role this service cannot assume is refused for the role whether the stream it names
/// has reached `ACTIVE` or is one this account holds at all.
#[tokio::test]
async fn the_role_is_resolved_before_the_source_streams_are() {
    let emulator = Emulator::new();
    let role = role_arn_of("channel-role-before-streams");
    // The stream this create names has only just been created, so its `CREATING` -> `ACTIVE`
    // transition is still owed; the other names a stream nothing created.
    let creating = emulator
        .creating_stream("channel-role-creating-source", "channel-role-creating-source")
        .await;
    let absent = stream_arn(REGION, "channel-role-no-such-source");

    for (case, source) in [
        ("source-stream-still-creating", creating),
        ("source-stream-nothing-holds", absent),
    ] {
        let (status, response) = emulator
            .call("CreateChannel", &create_body(case, &role, &source))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the role refusal ahead of any refusal about the source \
             stream, got {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            UNASSUMABLE_ROLE_SENTENCE,
            "case {case}: expected the role sentence rather than one about the stream, got {}",
            error_message(&response)
        );
    }

    // The complement, with the role IAM holds and trusts: the stream nothing holds is then
    // what refuses the identical create.
    let trusted = emulator
        .role(
            "source-stream-nothing-holds",
            "channel-role-trusted-before-streams",
            &trusted_by_kinesis(),
        )
        .await;
    let (status, response) = emulator
        .call(
            "CreateChannel",
            &create_body(
                "source-stream-nothing-holds",
                &trusted,
                &stream_arn(REGION, "channel-role-no-such-source"),
            ),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case \"source-stream-nothing-holds\": with an assumable role the source stream is \
         what refuses the create, got {status} with body {response}"
    );
}

/// What the request says ON ITS OWN is judged ahead of the role: a create that does not name
/// exactly one destination configuration is refused for that, with the sentence the service
/// states for the rule, even where the role it names is one this service cannot assume.
#[tokio::test]
async fn the_destination_rule_is_judged_before_the_role_is_resolved() {
    let emulator = Emulator::new();
    let source = emulator
        .on_demand_stream("channel-destination-source", "channel-destination-source")
        .await;
    let unassumable = role_arn_of("channel-destination-role");
    let tables_destination = json!({
        "DeadLetterQueueS3Configuration": dead_letter_queue("channel-role-dlq", "errors/"),
        "S3TablesConfigurationList": [{
            "TableBucketARN": table_bucket_arn("channel-role-tables"),
            "Namespace": "events",
            "TableName": "records",
            "CompressionType": "ZSTD",
        }],
    });

    // The rows differ along which destination members the request carries: neither of them,
    // and both at once.
    for (case, members) in [
        ("no-destination", json!({ "S3DestinationConfiguration": null })),
        (
            "both-destinations",
            json!({ "S3TablesDestinationConfiguration": tables_destination }),
        ),
    ] {
        let mut body = create_body(case, &unassumable, &source);
        if let (Some(body), Some(members)) = (body.as_object_mut(), members.as_object()) {
            for (name, value) in members {
                if value.is_null() {
                    body.remove(name);
                } else {
                    body.insert(name.clone(), value.clone());
                }
            }
        }

        let (status, response) = emulator.call("CreateChannel", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            DESTINATION_SENTENCE,
            "case {case}: expected the destination sentence rather than the role one, got {}",
            error_message(&response)
        );
        assert!(
            !emulator.lists_channel(case, case).await,
            "case {case}: a refused create leaves no channel behind"
        );
    }
}

/// The complement: with no IAM mounted nothing is KNOWN about the role, so the create that
/// the table above refuses for a role IAM holds nothing under is served here — a refusal is
/// only ever grounded in what IAM answered.
#[tokio::test]
async fn a_create_is_served_where_no_iam_answers_about_the_role() {
    let emulator = Emulator::without_iam();
    let case = "channel-role-without-iam";
    let source = emulator
        .on_demand_stream(case, "channel-role-unasked-source")
        .await;
    let created = emulator
        .ok(
            case,
            "CreateChannel",
            &create_body(case, &role_arn_of(case), &source),
        )
        .await;
    assert_eq!(
        member(case, &description(case, &created), "ChannelName"),
        case,
        "case {case}: expected the create served with no IAM to ask, got {created}"
    );
}
