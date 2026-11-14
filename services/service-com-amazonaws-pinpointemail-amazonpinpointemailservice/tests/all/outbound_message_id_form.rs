// SPDX-License-Identifier: CC0-1.0

//! The FORM of the `MessageId` an accepted `SendEmail` is answered with, end to end over
//! HTTP.
//!
//! Every case drives the service the way a client does — through the multi-service router,
//! over `restJson1` requests signed for the `ses` signing name — so what is asserted is the
//! wire behaviour.
//!
//! The rule these cases pin: **an accepted send is answered with a message id in the form
//! Amazon SES issues one in — a sixteen-hexadecimal-digit field opening with the version
//! prefix, then a hyphen-separated 8-4-4-4-12 hexadecimal field, then the trailing `000000`
//! field — and no two accepted messages share one.**
//!
//! `OutboundMessageId` carries no modelled constraint
//! (`smithy_query(query="shape",shape="OutboundMessageId")` reports a bare string), so the
//! form is the one the service answers with: the spelling the SES documentation's
//! event-publishing examples show (`…-e9aedb9a-02f9-4d12-a87d-dd0099a07f8a-000000`). This
//! API and the two SES APIs accept mail for ONE account, so one accepted message is
//! identified in one form whichever of them accepted it; the sibling suites of
//! `service-com-amazonaws-sesv2-simpleemailservice-v2` and
//! `service-com-amazonaws-ses-simpleemailservice` pin the same form on theirs.
//!
//! The rows span what the rule generalises over: the content form of the message (simple
//! text, simple HTML, a raw MIME document), which recipient fields it is addressed in, and a
//! run of sends made in one breath — which is where two ids minted from the clock alone
//! would collide.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointemail_amazonpinpointemailservice::EmulatedService;

/// A SigV4 credential scope for signing name `ses` — the routing signal that makes the
/// router hand the request to this service. The signature itself is never checked: the test
/// router runs with authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ses/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The version prefix the first field of an id opens with.
const VERSION_PREFIX: &str = "0100";

/// The trailing field every id ends with.
const TRAILING_FIELD: &str = "000000";

/// The length of each hyphen-separated field of the id, in order.
const FIELD_LENGTHS: [usize; 7] = [16, 8, 4, 4, 4, 12, 6];

/// One emulator: a router with only this service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// `SendEmail`, answering the status and the parsed body.
    async fn send_email(&self, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://email.us-east-1.amazonaws.com/v1/email/outbound-emails")
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// Assert `id` is in the form SES issues a message id in.
fn assert_message_id_form(case: &str, id: &str) {
    let fields: Vec<&str> = id.split('-').collect();
    let lengths: Vec<usize> = fields.iter().map(|field| field.len()).collect();
    assert_eq!(
        lengths,
        FIELD_LENGTHS.to_vec(),
        "case {case:?}: expected the fields of {id:?} to be {FIELD_LENGTHS:?} characters long, \
         got {lengths:?}"
    );
    assert!(
        fields
            .iter()
            .all(|field| field.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())),
        "case {case:?}: every field of a message id is lower-case hexadecimal, got {id:?}"
    );
    assert!(
        id.starts_with(VERSION_PREFIX),
        "case {case:?}: a message id opens with the version prefix {VERSION_PREFIX:?}, got {id:?}"
    );
    assert!(
        id.ends_with(TRAILING_FIELD),
        "case {case:?}: a message id ends with the field {TRAILING_FIELD:?}, got {id:?}"
    );
}

#[tokio::test]
async fn every_accepted_send_is_answered_with_a_message_id_in_the_form_ses_issues() {
    let emulator = Emulator::new();
    let raw = aws_smithy_types::base64::encode(
        "Subject: raw\r\nFrom: sender@example.com\r\n\r\nbody\r\n".as_bytes(),
    );
    // (case, the request body)
    let rows: &[(&str, Value)] = &[
        (
            "simple text to one recipient",
            json!({
                "FromEmailAddress": "sender@example.com",
                "Destination": {"ToAddresses": ["dest@example.org"]},
                "Content": {"Simple": {
                    "Subject": {"Data": "hello"},
                    "Body": {"Text": {"Data": "a plain body"}},
                }},
            }),
        ),
        (
            "simple html to two recipients",
            json!({
                "FromEmailAddress": "sender@example.com",
                "Destination": {"ToAddresses": ["one@example.org", "two@example.org"]},
                "Content": {"Simple": {
                    "Subject": {"Data": "hello again"},
                    "Body": {"Html": {"Data": "<p>a body</p>"}},
                }},
            }),
        ),
        (
            "a copied recipient only",
            json!({
                "Destination": {"CcAddresses": ["copy@example.org"]},
                "Content": {"Simple": {
                    "Subject": {"Data": "cc"},
                    "Body": {"Text": {"Data": "body"}},
                }},
            }),
        ),
        (
            "a raw MIME document",
            json!({
                "Destination": {"ToAddresses": ["dest@example.org"]},
                "Content": {"Raw": {"Data": raw}},
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body) in rows {
        let (status, answered) = emulator.send_email(body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the send to be accepted, got {status} {answered:?}"
        );
        let message_id = answered["MessageId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_message_id_form(case, &message_id);
        assert!(
            !minted.contains(&message_id),
            "case {case:?}: the message id {message_id:?} was already answered to an earlier send"
        );
        minted.push(message_id);
    }

    // Sends made back to back land inside one clock tick, which is where ids built from the
    // clock alone collide: each is still its own id, in the same form.
    let repeated = rows[0].1.clone();
    for index in 0..5 {
        let case = format!("send {index} of a run in one breath");
        let (status, answered) = emulator.send_email(repeated.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the send to be accepted, got {status} {answered:?}"
        );
        let message_id = answered["MessageId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_message_id_form(&case, &message_id);
        assert!(
            !minted.contains(&message_id),
            "case {case:?}: the message id {message_id:?} was already answered to an earlier send"
        );
        minted.push(message_id);
    }
}
