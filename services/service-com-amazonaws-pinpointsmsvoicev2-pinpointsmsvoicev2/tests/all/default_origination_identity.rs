// SPDX-License-Identifier: CC0-1.0

//! Sending text without naming an origination identity, end to end over the wire.
//!
//! Every request goes through [`MultiServiceRouter::dispatch`] as `awsJson1_0` — `POST /`
//! with `X-Amz-Target: PinpointSMSVoiceV2.<Op>` — so these tests exercise routing,
//! deserialization, constraint validation, the handler and serialization, exactly as a
//! real SDK client would reach them.
//!
//! The rule under test: a member the model does not make `@required` is one the service
//! resolves for itself when the request omits it, never a refusal — so `SendTextMessage`
//! with no `OriginationIdentity` (the only `@required` input member is
//! `DestinationPhoneNumber`:
//! `smithy_query(query="operation",operation="SendTextMessage")`) sends from the account's
//! own origination identity, minting a `MessageId` and recording the send; an identity
//! that IS named must still resolve to a resource the account holds, and the number the
//! account sends from is a lease like any other, reported by `DescribePhoneNumbers`.
//!
//! Its harness is this suite's own: the sibling suites' helpers are private to their
//! modules, and a test target shares nothing it does not export.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_pinpointsmsvoicev2_pinpointsmsvoicev2 as sms_voice;
use smithy_server_runtime::MultiServiceRouter;

/// The region these tests sign for, so the ARNs under assertion are not the default one.
const REGION: &str = "ap-southeast-2";

/// One emulator: the shared engine plus a router with only this service mounted.
///
/// The engine handle is kept beside the router so a test can read the persisted sends —
/// the same store the handler writes, reached the way the handler reaches it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests assert behaviour, not signatures; the emulator verifies signatures
        // itself and a service never checks them.
        router.set_request_authentication(false);
        router.register_service(sms_voice::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation and answer `(http status, parsed JSON body)`.
    async fn call(&self, operation: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("PinpointSMSVoiceV2.{operation}"))
            // The SigV4 credential scope is what routes the request to this service and
            // what names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sms-voice/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation and require HTTP 200, reporting the body when it is not.
    async fn ok(&self, operation: &str, body: serde_json::Value) -> serde_json::Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// The sends the store holds, as `(message_id, origination_identity, destination, dry_run)`.
    fn stored_sends(&self) -> Vec<(String, String, String, bool)> {
        let store = sms_voice::state::service_state(&*self.engine).expect("this service's store");
        let state = store.lock();
        state
            .messages()
            .map(|message| {
                (
                    message.message_id.clone(),
                    message.origination_identity.clone(),
                    message.destination_phone_number.clone(),
                    message.dry_run,
                )
            })
            .collect()
    }

    /// Every number the account leases, as `DescribePhoneNumbers` reports it.
    async fn leased_numbers(&self) -> Vec<serde_json::Value> {
        let described = self
            .ok("DescribePhoneNumbers", serde_json::json!({}))
            .await;
        described
            .get("PhoneNumbers")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("DescribePhoneNumbers: expected PhoneNumbers in {described}"))
            .clone()
    }

    /// Lease one number of the given shape and answer its `PhoneNumberId`.
    async fn request_number(&self, iso: &str, number_type: &str) -> String {
        let requested = self
            .ok(
                "RequestPhoneNumber",
                serde_json::json!({
                    "IsoCountryCode": iso,
                    "MessageType": "TRANSACTIONAL",
                    "NumberCapabilities": ["SMS"],
                    "NumberType": number_type,
                }),
            )
            .await;
        as_str(&requested, "PhoneNumberId")
    }
}

/// The wire error code of a failed call (`__type`, with any namespace prefix dropped).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

fn as_str(body: &serde_json::Value, member: &str) -> String {
    body.get(member)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// One send that names NO `OriginationIdentity`: what the request carries beside the one
/// required member.
#[derive(Debug, Clone)]
struct SendCase {
    case: &'static str,
    /// A `PhoneNumber` (len 1..20, `^\+?[1-9][0-9]{1,18}$`), so the rows span the forms
    /// that pattern admits: with and without the leading `+`, and both of its bounds.
    destination: &'static str,
    message_type: Option<&'static str>,
    message_body: Option<&'static str>,
    /// Whether the send names the configuration set these cases create.
    configuration_set: bool,
    /// `None` leaves the member out, which the model defaults to `false`.
    dry_run: Option<bool>,
}

/// Whether the send these cases record is a dry run: the member as sent, or the modelled
/// default of `DryRun` (`PrimitiveBoolean`, default `false`) when the request omits it.
fn recorded_dry_run(case: &SendCase) -> bool {
    case.dry_run.unwrap_or(false)
}

/// The sends under test: every row omits `OriginationIdentity` and differs in the members
/// the rule generalises over — the destination's form, the message type, whether a body or
/// a configuration set is named, and whether the send is a dry run.
///
/// The `MessageType` values are the model's own enum
/// (`smithy_query(query="shape",shape="MessageType")`: `TRANSACTIONAL`, `PROMOTIONAL`).
fn send_cases() -> Vec<SendCase> {
    let bare = SendCase {
        case: "only the required member",
        destination: "+14255550100",
        message_type: None,
        message_body: None,
        configuration_set: false,
        dry_run: None,
    };
    vec![
        bare.clone(),
        SendCase {
            case: "transactional send into a configuration set",
            destination: "+46790645100",
            message_type: Some("TRANSACTIONAL"),
            message_body: Some("Your test code is 8765"),
            configuration_set: true,
            dry_run: Some(false),
        },
        SendCase {
            case: "promotional dry run into a configuration set",
            destination: "+447700900123",
            message_type: Some("PROMOTIONAL"),
            message_body: Some("A promotional note"),
            configuration_set: true,
            dry_run: Some(true),
        },
        SendCase {
            case: "destination without the leading plus",
            destination: "14255550199",
            message_type: Some("TRANSACTIONAL"),
            message_body: Some("No plus sign here"),
            configuration_set: false,
            dry_run: Some(false),
        },
        SendCase {
            case: "shortest destination the pattern admits",
            destination: "+12",
            message_type: None,
            message_body: Some("s"),
            configuration_set: false,
            dry_run: Some(true),
        },
        SendCase {
            case: "longest destination the pattern admits",
            destination: "+1234567890123456789",
            message_type: Some("PROMOTIONAL"),
            message_body: Some("The longest destination"),
            configuration_set: true,
            dry_run: Some(false),
        },
    ]
}

/// The request body for one case, carrying only the members it names.
fn send_request(case: &SendCase, configuration_set_name: &str) -> serde_json::Value {
    let mut body = serde_json::json!({"DestinationPhoneNumber": case.destination});
    if let Some(message_type) = case.message_type {
        body["MessageType"] = serde_json::Value::String(message_type.to_string());
    }
    if let Some(message_body) = case.message_body {
        body["MessageBody"] = serde_json::Value::String(message_body.to_string());
    }
    if case.configuration_set {
        body["ConfigurationSetName"] = serde_json::Value::String(configuration_set_name.to_string());
    }
    if let Some(dry_run) = case.dry_run {
        body["DryRun"] = serde_json::Value::Bool(dry_run);
    }
    body
}

#[tokio::test]
async fn a_send_that_names_no_origination_identity_is_accepted_in_every_shape() {
    let emulator = Emulator::new();
    let configuration_set_name = "evsink";
    emulator
        .ok(
            "CreateConfigurationSet",
            serde_json::json!({"ConfigurationSetName": configuration_set_name}),
        )
        .await;

    let cases = send_cases();
    let mut accepted: Vec<(SendCase, String)> = Vec::new();
    for case in &cases {
        let (status, body) = emulator
            .call(
                "SendTextMessage",
                send_request(case, configuration_set_name),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?} ({case:?}): an omitted OriginationIdentity must not be refused, got \
             {status} with {body}",
            case.case
        );
        let message_id = as_str(&body, "MessageId");
        assert!(
            !message_id.is_empty(),
            "case {:?}: expected a non-empty MessageId, got {body}",
            case.case
        );
        accepted.push((case.clone(), message_id));
    }

    let distinct: BTreeSet<&String> = accepted.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        accepted.len(),
        "every accepted send must mint its own MessageId, got {:?}",
        accepted.iter().map(|(_, id)| id.clone()).collect::<Vec<String>>()
    );

    // Each send is RECORDED, from the identity the service resolved, with the destination
    // and the dry-run flag the request carried.
    let stored = emulator.stored_sends();
    assert_eq!(
        stored.len(),
        accepted.len(),
        "every accepted send must be recorded, got {stored:?}"
    );
    let resolved: BTreeSet<String> = stored
        .iter()
        .map(|(_, identity, _, _)| identity.clone())
        .collect();
    assert_eq!(
        resolved.len(),
        1,
        "every send that names no identity must resolve the same account default, got {resolved:?}"
    );
    for ((case, message_id), (stored_id, identity, destination, dry_run)) in
        accepted.iter().zip(stored.iter())
    {
        assert_eq!(
            (stored_id.as_str(), destination.as_str(), *dry_run),
            (
                message_id.as_str(),
                case.destination,
                recorded_dry_run(case)
            ),
            "case {:?}: expected the send recorded as (id, destination, dry run) \
             ({message_id}, {}, {}), got ({stored_id}, {destination}, {dry_run})",
            case.case,
            case.destination,
            recorded_dry_run(case)
        );
        assert!(
            !identity.is_empty(),
            "case {:?}: expected a resolved origination identity, got {identity:?}",
            case.case
        );
    }

    // The identity the service resolved is BACKED: exactly one number is leased, and it is
    // the one every send went out from.
    let leased = emulator.leased_numbers().await;
    assert_eq!(
        leased.len(),
        1,
        "the account default identity must be one leased number, got {leased:?}"
    );
    let leased_id = as_str(&leased[0], "PhoneNumberId");
    assert_eq!(
        resolved.iter().next().map(String::as_str),
        Some(leased_id.as_str()),
        "expected every send to come from the leased number {leased_id}, got {resolved:?}"
    );
}

#[tokio::test]
async fn the_account_default_identity_is_a_number_the_account_already_leases() {
    let emulator = Emulator::new();
    // Two leases, so the default is one the account holds rather than a third number: the
    // rows differ in the shape of the lease, which the resolution does not depend on.
    let first = emulator.request_number("US", "LONG_CODE").await;
    let second = emulator.request_number("GB", "TOLL_FREE").await;

    for case in ["first send", "second send"] {
        let sent = emulator
            .ok(
                "SendTextMessage",
                serde_json::json!({
                    "DestinationPhoneNumber": "+14255550133",
                    "MessageBody": "sent from the account default",
                }),
            )
            .await;
        assert!(
            !as_str(&sent, "MessageId").is_empty(),
            "case {case:?}: expected a MessageId, got {sent}"
        );
    }

    let stored = emulator.stored_sends();
    assert_eq!(stored.len(), 2, "both sends must be recorded, got {stored:?}");
    for (index, (_, identity, _, _)) in stored.iter().enumerate() {
        assert_eq!(
            *identity, first,
            "case \"recorded send {index}\": expected the first leased number {first} (the \
             account also leases {second}), got {identity}"
        );
    }

    let leased = emulator.leased_numbers().await;
    assert_eq!(
        leased.len(),
        2,
        "an account that already leases a number must not be given another, got {leased:?}"
    );
}

#[tokio::test]
async fn the_identity_the_service_provides_is_reported_like_any_other_lease() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.leased_numbers().await,
        Vec::<serde_json::Value>::new(),
        "the account leases nothing before the first send"
    );

    emulator
        .ok(
            "SendTextMessage",
            serde_json::json!({
                "DestinationPhoneNumber": "+14255550144",
                "MessageBody": "the first send of this account",
            }),
        )
        .await;

    let leased = emulator.leased_numbers().await;
    assert_eq!(
        leased.len(),
        1,
        "an account that leases nothing must be given one origination identity, got {leased:?}"
    );
    let number = &leased[0];
    let phone_number = as_str(number, "PhoneNumber");
    assert!(
        phone_number.starts_with('+')
            && phone_number[1..].chars().all(|digit| digit.is_ascii_digit())
            && (3..=20).contains(&phone_number.len()),
        "expected an E.164 PhoneNumber (len 1..20, ^\\+?[1-9][0-9]{{1,18}}$), got {phone_number:?}"
    );
    // The members a lease reports, each a value of its own modelled enum: `NumberStatus`,
    // `NumberCapability` and `RequestableNumberType`
    // (`smithy_query(query="shape",shape="NumberStatus,NumberCapability,RequestableNumberType")`).
    assert_eq!(
        as_str(number, "Status"),
        "ACTIVE",
        "expected the lease in its terminal status, got {number}"
    );
    assert_eq!(
        number.get("NumberCapabilities"),
        Some(&serde_json::json!(["SMS"])),
        "a text message goes out over the SMS capability, got {number}"
    );
    assert_eq!(
        as_str(number, "NumberType"),
        "LONG_CODE",
        "expected a requestable number type, got {number}"
    );
    let identity = as_str(number, "PhoneNumberId");
    assert_eq!(
        emulator
            .stored_sends()
            .iter()
            .map(|(_, recorded, _, _)| recorded.clone())
            .collect::<Vec<String>>(),
        vec![identity.clone()],
        "expected the send recorded against the identity the service provided ({identity})"
    );

    // ROUND TRIP: the identity the service resolved is one a caller can name explicitly,
    // in every form that names the number.
    let arn = as_str(number, "PhoneNumberArn");
    for named in [identity.clone(), arn, phone_number.clone()] {
        let sent = emulator
            .ok(
                "SendTextMessage",
                serde_json::json!({
                    "DestinationPhoneNumber": "+14255550155",
                    "OriginationIdentity": named,
                    "MessageBody": "named explicitly",
                }),
            )
            .await;
        assert!(
            !as_str(&sent, "MessageId").is_empty(),
            "case {named:?}: expected a MessageId, got {sent}"
        );
    }
    assert_eq!(
        emulator.leased_numbers().await.len(),
        1,
        "no further send may lease another number"
    );
}

#[tokio::test]
async fn a_refused_send_resolves_no_identity_and_records_nothing() {
    let emulator = Emulator::new();
    // Each of these IS present and names no resource this account holds: an id, an ARN, an
    // E.164 number and a pool id, which are the forms `TextMessageOriginationIdentity`
    // documents (`smithy_query(query="doc",shape="SendTextMessageRequest",member="OriginationIdentity")`).
    let cases = [
        ("an id nobody leases", "phone-does-not-exist"),
        (
            "an ARN nobody leases",
            "arn:aws:sms-voice:ap-southeast-2:000000000000:phone-number/phone-0001",
        ),
        ("a number nobody leases", "+14255559999"),
        ("a pool this account does not hold", "pool-0001"),
    ];
    for (case, identity) in cases {
        let (status, body) = emulator
            .call(
                "SendTextMessage",
                serde_json::json!({
                    "DestinationPhoneNumber": "+14255550166",
                    "OriginationIdentity": identity,
                    "MessageBody": "names an identity that resolves to nothing",
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected a not-found refusal, got {status} with {body}"
        );
        assert_eq!(
            as_str(&body, "Message"),
            format!(
                "Resource Not Found - ResourceType=\"phone-number\" ResourceId=\"{identity}\""
            ),
            "case {case:?}: expected this service's not-found message, got {body}"
        );
    }

    // A send that names NO identity but a configuration set this account does not hold is
    // refused too, and the refusal provisions nothing: the account default is resolved
    // only once every reference the send carries has been checked.
    let (status, body) = emulator
        .call(
            "SendTextMessage",
            serde_json::json!({
                "DestinationPhoneNumber": "+14255550177",
                "ConfigurationSetName": "no-such-set",
                "MessageBody": "names a configuration set that resolves to nothing",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case \"unknown configuration set\": expected a not-found refusal, got {status} with {body}"
    );
    assert_eq!(
        as_str(&body, "Message"),
        "Resource Not Found - ResourceType=\"configuration-set\" ResourceId=\"no-such-set\"",
        "case \"unknown configuration set\": expected this service's not-found message, got {body}"
    );

    // A refused send records nothing, and refusing one never provides the account with an
    // origination identity.
    assert!(
        emulator.stored_sends().is_empty(),
        "a refused send must record nothing, got {:?}",
        emulator.stored_sends()
    );
    assert_eq!(
        emulator.leased_numbers().await,
        Vec::<serde_json::Value>::new(),
        "a refused send must lease nothing"
    );
}
