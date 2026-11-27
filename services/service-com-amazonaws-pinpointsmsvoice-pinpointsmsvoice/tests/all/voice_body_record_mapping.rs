// SPDX-License-Identifier: CC0-1.0

//! The members `SendVoiceMessage` records for the spoken message body it was given.
//!
//! RULE: a stored body record is the member-for-member image of the wire shape it was
//! built from, and the two shapes that declare the same members are stored by the same
//! mapping — so a member one body kind carries is carried by the other, and none of them
//! can be dropped for one kind only.
//!
//! The members come from
//! `smithy_query(query="shape",shape="CallInstructionsMessageType,PlainTextMessageType,SSMLMessageType",model_path="services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json")`:
//! `PlainTextMessageType` and `SSMLMessageType` each declare `Text`, `LanguageCode` and
//! `VoiceId`, `CallInstructionsMessageType` declares only `Text`, and every one of them is
//! optional.
//!
//! The rows span every subset of those three members, over two different sets of values,
//! and each row is driven through BOTH spoken body kinds; the two records are compared with
//! each other as well as with what was sent, so a member carried for one body kind and
//! dropped for the other fails the row. One row sends the one-member body kind, whose
//! record is a different type and must stay so.
//!
//! The suite drives the real `restJson1` router, so the requests here are the bytes an AWS
//! SDK `pinpoint-sms-voice` client sends. The account holds the entitlement this API's
//! resource-scoped actions need, which `legacy_surface_authorization.rs` owns.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::state::{
    VoiceContentRecord, VoiceTextRecord,
};
use service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::{
    ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState, EmulatedService,
};

/// The members both spoken-message shapes declare, in the order the suite fills them.
const SPOKEN_MEMBERS: [&str; 3] = ["Text", "LanguageCode", "VoiceId"];

/// The emulator under test, over the one shared engine the suite also reads state from
/// (what a send records is observable only there).
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite drives unsigned requests; signature verification is the framework's
        // own concern and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // The account holds the entitlement this API's resource-scoped actions need, so
        // the rows below exercise what an ACCEPTED send records; the entitlement itself is
        // pinned in `legacy_surface_authorization.rs`.
        engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>()
            .lock()
            .legacy_surface_entitled = true;
        Self { router, engine }
    }

    /// Send one `SendVoiceMessage` whose `Content` is `content`: the status and the parsed
    /// JSON body (`Value::Null` when empty).
    async fn send_voice(&self, content: Value) -> (u16, Value) {
        let payload = json!({
            "DestinationPhoneNumber": "+12065550100",
            "Content": content,
        })
        .to_string();
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sms-voice.us-east-1.amazonaws.com/v1/sms-voice/voice/message")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/sms-voice/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("the test request must build");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// The content of the most recently recorded message.
    fn last_content(&self) -> VoiceContentRecord {
        let store = self
            .engine
            .state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>();
        let recorded = store.lock();
        recorded
            .sent_messages
            .last()
            .expect("an accepted send must be recorded")
            .content
            .clone()
    }
}

/// The members a spoken record holds, keyed by the wire member name; an absent member is
/// left out, exactly as it was left out of the request.
fn recorded_members(body: &VoiceTextRecord) -> Map<String, Value> {
    [
        ("Text", &body.text),
        ("LanguageCode", &body.language_code),
        ("VoiceId", &body.voice_id),
    ]
    .into_iter()
    .filter_map(|(member, value)| {
        value
            .clone()
            .map(|text| (member.to_string(), Value::String(text)))
    })
    .collect()
}

/// RULE: every member a spoken body carries is recorded, and the two spoken body kinds
/// record the same members the same way.
#[tokio::test]
async fn both_spoken_body_kinds_record_every_member_they_carried() {
    // Two value sets, so no row can pass by a value the mapping happens to hold on to:
    // (Text, LanguageCode, VoiceId) read in the order of `SPOKEN_MEMBERS`.
    let value_sets: [[&str; 3]; 2] = [
        ["hello there", "en-US", "Matthew"],
        ["<speak>hi</speak>", "en-GB", "Amy"],
    ];

    let harness = Harness::new();

    // Every subset of the three members, including the empty one: a body supplied with no
    // members is still a body the caller supplied.
    for values in value_sets {
        for mask in 0u8..(1 << SPOKEN_MEMBERS.len()) {
            let sent: Map<String, Value> = SPOKEN_MEMBERS
                .iter()
                .zip(values)
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, (member, value))| ((*member).to_string(), json!(value)))
                .collect();
            let case = format!("{} / {:?}", Value::Object(sent.clone()), values);

            let mut per_kind: Vec<Map<String, Value>> = Vec::new();
            for kind in ["PlainTextMessage", "SSMLMessage"] {
                let (status, body) = harness
                    .send_voice(json!({ kind: Value::Object(sent.clone()) }))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?} as {kind}: expected 200, got {status} {body}"
                );

                let content = harness.last_content();
                let recorded = match kind {
                    "PlainTextMessage" => content.plain_text.as_ref(),
                    _ => content.ssml.as_ref(),
                };
                let recorded = recorded.unwrap_or_else(|| {
                    panic!("case {case:?} as {kind}: the supplied body must be recorded present, got {content:?}")
                });
                let recorded = recorded_members(recorded);
                assert_eq!(
                    Value::Object(recorded.clone()),
                    Value::Object(sent.clone()),
                    "case {case:?} as {kind}: expected the members the request carried, got {}",
                    Value::Object(recorded.clone())
                );
                assert!(
                    content.call_instructions.is_none(),
                    "case {case:?} as {kind}: no other body may be recorded, got {content:?}"
                );
                per_kind.push(recorded);
            }

            assert_eq!(
                Value::Object(per_kind[0].clone()),
                Value::Object(per_kind[1].clone()),
                "case {case:?}: the two spoken body kinds disagree: plain text recorded {}, SSML recorded {}",
                Value::Object(per_kind[0].clone()),
                Value::Object(per_kind[1].clone())
            );
        }
    }
}

/// The one-member body kind keeps its own record: it shares no member mapping with the
/// spoken bodies, so the member it declares is recorded and no spoken body is.
#[tokio::test]
async fn the_call_instructions_body_records_the_only_member_it_declares() {
    let harness = Harness::new();

    for text in [Some("press 1 to be connected"), None] {
        let case = format!("call instructions with Text={text:?}");
        let body = text.map_or_else(|| json!({}), |value| json!({ "Text": value }));
        let (status, answer) = harness
            .send_voice(json!({ "CallInstructionsMessage": body }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answer}"
        );

        let content = harness.last_content();
        let recorded = content.call_instructions.as_ref().unwrap_or_else(|| {
            panic!("case {case:?}: the supplied body must be recorded present, got {content:?}")
        });
        assert_eq!(
            recorded.text.as_deref(),
            text,
            "case {case:?}: expected the Text the request carried, got {:?}",
            recorded.text
        );
        assert!(
            content.plain_text.is_none() && content.ssml.is_none(),
            "case {case:?}: no spoken body may be recorded, got {content:?}"
        );
    }
}
