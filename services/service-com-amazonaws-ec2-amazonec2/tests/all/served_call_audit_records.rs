// SPDX-License-Identifier: CC0-1.0

//! Every EC2 management call this service SERVES is in the account's event history, with
//! the outcome its caller saw — the refusals as much as the successes.
//!
//! RULE: a service that serves a control-plane API call records it as a management event of
//! that service, whether it served the call or refused it, so a lookup over event history
//! reports the call under its own `eventName` with this service's `eventSource`, the
//! `readOnly` of the operation, the `requestID` the CALLER was answered under and — for a
//! refused call — the fault class of the answer plus the error code the caller saw, the
//! message it saw, and NO `responseElements`, there having been no response to record. A
//! call that was served carries no `errorCode` at all.
//!
//! The rows below span the kinds of outcome the rule generalises over rather than the one
//! refusal that filed it: a read refused for a resource that is not there, a read refused
//! for a value that is no id at all, a write refused as a dry run, a write refused for a
//! resource that is not there, a read that reports state, and a write that changes the
//! account. Each row states the code and the message the CALLER is answered with and the
//! `errorCode` the RECORD carries, so the fault-class prefix is asserted against a literal
//! rather than recomputed by the code under test.
//!
//! The records are read out of the shared log the recorder writes into
//! (`service_common::management_events::ManagementEventLog`, which is the store
//! CloudTrail's `LookupEvents` serves from) as the JSON document that lookup reports, so
//! what is asserted here is the document a client reads back. The cross-service half — the
//! same records served through `cloudtrail:LookupEvents` — is
//! `core/router/tests/all/ec2_management_events.rs`, which needs CloudTrail mounted beside
//! this service.

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_common::management_events::{ManagementEventLog, HIDDEN_SENSITIVE_VALUE};

use super::ec2_query::{call, error_code, error_message, member};

/// The Region every call here is signed for, and so the Region its records live in.
const REGION: &str = "us-west-2";

/// A SHORT-form instance id the account holds no instance for: a form EC2 reads and looks
/// up, so it is refused as a resource that is not there.
const ABSENT_INSTANCE: &str = "i-1a2b3c4d";

/// A value that is no instance id at all, so there is nothing to look up.
const UNREADABLE_INSTANCE: &str = "not-an-instance";

/// A well-formed VPC id the account holds no VPC for.
const ABSENT_VPC: &str = "vpc-0fedcba987654321f";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// An AMI of the published catalog a fresh account can already see, read out of the
/// service's own state rather than written here, so the launch below names an image this
/// account really has.
fn catalog_image() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// One row of the audit table: one call, what the CALLER is answered with, and what the
/// RECORD of it must carry.
struct AuditCase {
    /// What this row is about, named in every assertion.
    case: &'static str,
    /// The operation the call names.
    action: &'static str,
    /// The call's own parameters.
    params: Params,
    /// The HTTP status the caller is answered on.
    status: u16,
    /// The `<Code>` the caller reads off its own response, and the record's own
    /// `errorCode` — `None` on both for a call that was served.
    wire_code: Option<&'static str>,
    audited_code: Option<&'static str>,
    /// The `<Message>` the caller reads, which is also the record's `errorMessage`.
    message: Option<String>,
    /// Whether the operation only REPORTS state.
    read_only: bool,
    /// Whether the record carries `responseElements`: what a MUTATING call that was served
    /// answered with, and nothing for a read or for a refusal.
    records_response_elements: bool,
    /// The whole `requestParameters` document the record must carry: the parameters the
    /// call was dispatched with, each member named as a CloudTrail event document names it.
    recorded_parameters: Value,
}

/// The request id the CALLER was answered under, read out of the envelope it received: the
/// error envelope's `RequestID` or the success envelope's `requestId`.
fn answered_request_id(case: &str, body: &str) -> String {
    member(body, "RequestID")
        .or_else(|| member(body, "requestId"))
        .unwrap_or_else(|| panic!("case {case:?}: the response names no request: {body}"))
}

/// The records of the call answered under `request_id`, as the JSON documents a lookup over
/// event history reports.
fn records_of(engine: &engine::InMemoryEngine, request_id: &str) -> Vec<Value> {
    let state = engine.state::<ManagementEventLog>();
    let log = state.lock();
    log.iter()
        .filter(|event| event.request_id == request_id)
        .map(|event| event.to_cloudtrail_event_json())
        .collect()
}

/// The one record of the call answered under `request_id`.
fn record_of(case: &str, engine: &engine::InMemoryEngine, request_id: &str) -> Value {
    let found = records_of(engine, request_id);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one record of the call answered under \
         {request_id:?}, got {found:?}"
    );
    found.into_iter().next().unwrap_or(Value::Null)
}

/// RULE: every call this service serves is recorded with the outcome its caller saw.
#[tokio::test]
async fn every_served_call_is_audited_with_the_outcome_its_caller_saw() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = catalog_image();

    let rows = vec![
        AuditCase {
            case: "a read refused for an instance the account does not hold",
            action: "DescribeInstances",
            params: vec![p("InstanceId.1", ABSENT_INSTANCE)],
            status: 400,
            wire_code: Some("InvalidInstanceID.NotFound"),
            audited_code: Some("Client.InvalidInstanceID.NotFound"),
            message: Some(format!("The instance ID '{ABSENT_INSTANCE}' does not exist")),
            read_only: true,
            records_response_elements: false,
            recorded_parameters: json!({"instanceId": [ABSENT_INSTANCE]}),
        },
        AuditCase {
            case: "a read refused for a value that is no instance id at all",
            action: "DescribeInstances",
            params: vec![p("InstanceId.1", UNREADABLE_INSTANCE)],
            status: 400,
            wire_code: Some("InvalidInstanceID.Malformed"),
            audited_code: Some("Client.InvalidInstanceID.Malformed"),
            message: Some(format!("Invalid id: \"{UNREADABLE_INSTANCE}\"")),
            read_only: true,
            records_response_elements: false,
            recorded_parameters: json!({"instanceId": [UNREADABLE_INSTANCE]}),
        },
        AuditCase {
            case: "a launch refused as a dry run",
            action: "RunInstances",
            params: vec![
                p("ImageId", image_id.clone()),
                p("InstanceType", "t3.micro"),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("DryRun", "true"),
            ],
            status: 412,
            wire_code: Some("DryRunOperation"),
            audited_code: Some("Client.DryRunOperation"),
            message: Some("Request would have succeeded, but DryRun flag is set.".to_string()),
            read_only: false,
            records_response_elements: false,
            recorded_parameters: json!({
                "imageId": image_id.clone(),
                "instanceType": "t3.micro",
                "minCount": "1",
                "maxCount": "1",
                "dryRun": "true",
            }),
        },
        AuditCase {
            case: "a delete refused for a VPC the account does not hold",
            action: "DeleteVpc",
            params: vec![p("VpcId", ABSENT_VPC)],
            status: 400,
            wire_code: Some("InvalidVpcID.NotFound"),
            audited_code: Some("Client.InvalidVpcID.NotFound"),
            message: Some(format!("The vpc ID '{ABSENT_VPC}' does not exist")),
            read_only: false,
            records_response_elements: false,
            recorded_parameters: json!({"vpcId": ABSENT_VPC}),
        },
        AuditCase {
            case: "a read that reports the account's instances",
            action: "DescribeInstances",
            params: Vec::new(),
            status: 200,
            wire_code: None,
            audited_code: None,
            message: None,
            read_only: true,
            records_response_elements: false,
            recorded_parameters: json!({}),
        },
        AuditCase {
            case: "a read narrowed by a filter, whose parameters nest",
            action: "DescribeInstances",
            params: vec![
                p("Filter.1.Name", "instance-state-name"),
                p("Filter.1.Value.1", "running"),
                p("Filter.1.Value.2", "pending"),
            ],
            status: 200,
            wire_code: None,
            audited_code: None,
            message: None,
            read_only: true,
            records_response_elements: false,
            recorded_parameters: json!({
                "filter": [{"name": "instance-state-name", "value": ["running", "pending"]}],
            }),
        },
        AuditCase {
            case: "a create that changes the account",
            action: "CreateVpc",
            params: vec![p("CidrBlock", "10.43.0.0/16")],
            status: 200,
            wire_code: None,
            audited_code: None,
            message: None,
            read_only: false,
            records_response_elements: true,
            recorded_parameters: json!({"cidrBlock": "10.43.0.0/16"}),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, body) = call(&router, REGION, row.action, &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        // What the CALLER saw, which is the half of the record that has to agree with it.
        if let Some(wire_code) = row.wire_code {
            let answered = error_code(&body);
            assert_eq!(
                answered, wire_code,
                "case {case:?}: expected the caller to see {wire_code:?}, got {answered:?} \
                 in {body}"
            );
        }
        if let Some(message) = &row.message {
            let answered = error_message(&body);
            assert_eq!(
                &answered, message,
                "case {case:?}: expected the caller to see {message:?}, got {answered:?} \
                 in {body}"
            );
        }

        let request_id = answered_request_id(case, &body);
        let record = record_of(case, &engine, &request_id);
        assert_eq!(
            record.get("eventSource"),
            Some(&json!(ec2::service::management_events::EVENT_SOURCE)),
            "case {case:?}: the record names the serving service, got {record}"
        );
        assert_eq!(
            record.get("eventName"),
            Some(&json!(row.action)),
            "case {case:?}: the record names the operation called, got {record}"
        );
        assert_eq!(
            record.get("awsRegion"),
            Some(&json!(REGION)),
            "case {case:?}: the record is in the Region the call was served in, got {record}"
        );
        assert_eq!(
            record.get("requestID"),
            Some(&json!(request_id)),
            "case {case:?}: the record names the request the caller was answered under, \
             got {record}"
        );
        assert_eq!(
            record.get("readOnly"),
            Some(&json!(row.read_only)),
            "case {case:?}: expected readOnly {}, got {record}",
            row.read_only
        );
        assert_eq!(
            record.get("eventType"),
            Some(&json!("AwsApiCall")),
            "case {case:?}: a management event of an API call, got {record}"
        );
        // The error half: the fault class plus the code the caller saw, the message it saw,
        // and no key at all for a call that was served.
        assert_eq!(
            record.get("errorCode"),
            row.audited_code.map(|code| json!(code)).as_ref(),
            "case {case:?}: expected the audited error code {:?}, got {record}",
            row.audited_code
        );
        assert_eq!(
            record.get("errorMessage"),
            row.message.as_ref().map(|message| json!(message)).as_ref(),
            "case {case:?}: expected the audited message {:?}, got {record}",
            row.message
        );
        // The response half: what a mutating call answered with, and nothing — JSON null,
        // which is how CloudTrail records "no response elements" — for a read or a refusal.
        let response_elements = record.get("responseElements").unwrap_or(&Value::Null);
        assert_eq!(
            response_elements.is_object(),
            row.records_response_elements,
            "case {case:?}: expected responseElements present = {}, got {record}",
            row.records_response_elements
        );
        // The parameters the call was dispatched with are in the record, each member named
        // as the event document names a modelled member.
        assert_eq!(
            record.get("requestParameters"),
            Some(&row.recorded_parameters),
            "case {case:?}: expected the parameters {}, got {record}",
            row.recorded_parameters
        );
    }
}

/// RULE, the other half: event history is the record of the calls an account's APIs
/// actually SERVED, so a call this emulator has not implemented — answered 501 — is not in
/// it, and neither is a request naming no operation of this service.
#[tokio::test]
async fn a_call_nothing_served_is_not_in_event_history() {
    let (router, engine) = super::ec2_query::emulator();

    // A stub answers 501 `NotImplemented`: the call reached no implementation.
    let (status, body) = call(&router, REGION, "DescribeIdFormat", &[]).await;
    assert_eq!(
        status, 501,
        "case \"a stub\": expected HTTP 501, got {status} with body {body}"
    );
    // A name this service serves no operation for is refused by the routing failure.
    let (routing_status, routing_body) = call(&router, REGION, "NoSuchEc2Action", &[]).await;
    assert_eq!(
        routing_status, 400,
        "case \"an action nothing serves\": expected HTTP 400, got {routing_status} with \
         body {routing_body}"
    );

    let state = engine.state::<ManagementEventLog>();
    let recorded: Vec<Value> = {
        let log = state.lock();
        log.iter().map(|event| event.to_cloudtrail_event_json()).collect()
    };
    assert!(
        recorded.is_empty(),
        "neither a stub nor an unknown action is a served call: got {recorded:?}"
    );
}

/// RULE: a record never carries the value of a member this service's model marks
/// `@sensitive` — the private half of a generated key pair reaches no audit record.
#[tokio::test]
async fn a_sensitive_value_is_withheld_from_the_record() {
    let (router, engine) = super::ec2_query::emulator();
    let (status, body) = call(
        &router,
        REGION,
        "CreateKeyPair",
        &[p("KeyName", "audited-key")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the key pair create must be served, got {status} with body {body}"
    );
    let key_material = super::ec2_query::element(&body, "keyMaterial")
        .unwrap_or_else(|| panic!("the create must answer the private key: {body}"));
    assert!(
        key_material.contains("PRIVATE KEY"),
        "the create must answer a PEM private key, got {key_material:?}"
    );

    let request_id = answered_request_id("the key pair create", &body);
    let record = record_of("the key pair create", &engine, &request_id);
    assert_eq!(
        record.pointer("/responseElements/keyMaterial"),
        Some(&json!(HIDDEN_SENSITIVE_VALUE)),
        "the private key must be withheld from the record, got {record}"
    );
    assert!(
        !record.to_string().contains(&key_material),
        "no part of the record may carry the private key, got {record}"
    );
}
