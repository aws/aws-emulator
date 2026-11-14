// SPDX-License-Identifier: CC0-1.0

//! The FORM of the `MessageId` an accepted send is answered with, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this service
//! registered — so routing, the handler and serialization are all exercised, and the id the
//! response carries is the one the store wrote.
//!
//! The rule these cases pin: **an accepted send is answered with a message id in the form
//! Amazon SES issues one in — a sixteen-hexadecimal-digit field opening with the version
//! prefix, then a hyphen-separated 8-4-4-4-12 hexadecimal field, then the trailing
//! `000000` field — unique per accepted message, equal to the id the outbound-message log
//! holds for it, and accepted by the operations that take an outbound message id.**
//!
//! `OutboundMessageId` carries no modelled constraint
//! (`smithy_query(query="shape",shape="OutboundMessageId")` reports a bare string), so the
//! form is the one the service answers with: the id spelling the SES documentation's
//! event-publishing examples show (`…-e9aedb9a-02f9-4d12-a87d-dd0099a07f8a-000000`), which
//! is also the form the classic SES surface over these same accounts mints
//! (`services/service-com-amazonaws-ses-simpleemailservice/src/state.rs`) — one id form for
//! one account's accepted messages, whichever surface accepted them. A consumer of the
//! events a send publishes reads the id out of the event document and matches it against the
//! one the send answered, so the two have to be one value in one form.
//!
//! The rows span what the rule generalises over: the content form of the message (a simple
//! message, a raw MIME document), how many recipients it is addressed to, whether it names a
//! configuration set, and a run of sends made in one breath — which is where two ids minted
//! inside one clock tick would collide.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The verified sending domain every case here sends from.
const SENDER_DOMAIN: &str = "ids.example.com";

/// The version prefix the first field of an id opens with.
const VERSION_PREFIX: &str = "0100";

/// The trailing field every id ends with.
const TRAILING_FIELD: &str = "000000";

/// The length of each hyphen-separated field of the id, in order.
const FIELD_LENGTHS: [usize; 7] = [16, 8, 4, 4, 4, 12, 6];

/// One answered request: the status and the parsed body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// A string member of the body, absent reported as an empty string.
    fn string(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can read the log
/// the ids were written to.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// The account with [`SENDER_DOMAIN`] verified for sending.
    ///
    /// The handler stores the instant the verification transition falls due rather than
    /// sleeping for it, so this brings that instant into the past instead of waiting out the
    /// span.
    async fn with_verified_domain() -> Self {
        let ses = Self::new();
        let created = ses
            .call(
                "POST",
                "/v2/email/identities",
                Some(json!({"EmailIdentity": SENDER_DOMAIN})),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "the sending domain must be created first, got {} {:?}",
            created.status, created.body
        );
        let state = ses
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant just before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
        store.settle(SystemTime::now());
        drop(store);
        ses
    }

    async fn send(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    async fn insights(&self, message_id: &str) -> Reply {
        self.call("GET", &format!("/v2/email/insights/{message_id}"), None)
            .await
    }

    /// The ids the outbound-message log holds, oldest first.
    fn logged_message_ids(&self) -> Vec<String> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .sent_messages
            .iter()
            .map(|message| message.message_id.clone())
            .collect()
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

/// A simple message to `recipients`, optionally through the configuration set `set`.
fn simple_send(recipients: &[&str], set: Option<&str>) -> Value {
    let mut body = json!({
        "FromEmailAddress": format!("sender@{SENDER_DOMAIN}"),
        "Destination": {"ToAddresses": recipients},
        "Content": {"Simple": {
            "Subject": {"Data": "a message under test"},
            "Body": {"Text": {"Data": "body text"}},
        }},
    });
    if let Some(set) = set {
        body["ConfigurationSetName"] = json!(set);
    }
    body
}

/// A raw MIME message to `recipients`: `Content$Raw` carries the document as a blob, which
/// the restJson1 wire spells as base64.
fn raw_send(recipients: &[&str]) -> Value {
    let document = format!(
        "From: sender@{SENDER_DOMAIN}\r\nTo: {}\r\nSubject: a raw message\r\n\r\nbody text\r\n",
        recipients.join(", ")
    );
    json!({
        "FromEmailAddress": format!("sender@{SENDER_DOMAIN}"),
        "Destination": {"ToAddresses": recipients},
        "Content": {"Raw": {"Data": aws_smithy_types::base64::encode(document.as_bytes())}},
    })
}

#[tokio::test]
async fn every_accepted_send_is_answered_with_a_message_id_in_the_form_ses_issues() {
    // (case, the configuration set the message names, the request body)
    let rows: Vec<(&str, Option<&str>, Value)> = vec![
        (
            "a simple message to one recipient",
            None,
            simple_send(&["one@example.org"], None),
        ),
        (
            "a simple message to three recipients",
            None,
            simple_send(
                &["one@example.org", "two@example.net", "three@example.com"],
                None,
            ),
        ),
        (
            "a simple message through a configuration set",
            Some("minted-ids"),
            simple_send(&["one@example.org"], Some("minted-ids")),
        ),
        (
            "a raw MIME document to one recipient",
            None,
            raw_send(&["one@example.org"]),
        ),
        (
            "a raw MIME document to two recipients",
            None,
            raw_send(&["one@example.org", "two@example.net"]),
        ),
    ];

    for (case, set, body) in rows {
        let ses = Ses::with_verified_domain().await;
        if let Some(set) = set {
            let created = ses
                .call(
                    "POST",
                    "/v2/email/configuration-sets",
                    Some(json!({"ConfigurationSetName": set})),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the configuration set must be created first, got {} {:?}",
                created.status, created.body
            );
        }
        let answered = ses.send(body).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected the send to be accepted, got {} {} {:?}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        let message_id = answered.string("MessageId");
        assert_message_id_form(case, &message_id);

        // The id answered is the id written: the log holds exactly this message under it.
        assert_eq!(
            ses.logged_message_ids(),
            vec![message_id.clone()],
            "case {case:?}: the outbound log must hold the accepted message under the id the \
             send answered with"
        );

        // And it is an id the operations that take one accept: the insights report resolves
        // it to the message that was accepted rather than refusing its form.
        let report = ses.insights(&message_id).await;
        assert_eq!(
            report.status,
            200,
            "case {case:?}: expected the insights report for {message_id:?}, got {} {} {:?}",
            report.status,
            report.error_code(),
            report.body
        );
        assert_eq!(
            report.string("MessageId"),
            message_id,
            "case {case:?}: the report must name the message it was asked for, got {:?}",
            report.body
        );
    }
}

#[tokio::test]
async fn a_run_of_sends_in_one_breath_each_get_their_own_message_id() {
    // Sends made back to back land inside one clock tick, which is where ids built from the
    // clock alone collide: every one of them is still its own id, and every one is in the
    // form SES issues.
    let case = "ten sends in one breath";
    let ses = Ses::with_verified_domain().await;
    let mut minted: Vec<String> = Vec::new();
    for index in 0..10 {
        let answered = ses
            .send(simple_send(
                &[&format!("recipient-{index}@example.org")],
                None,
            ))
            .await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: send {index} must be accepted, got {} {:?}",
            answered.status,
            answered.body
        );
        let message_id = answered.string("MessageId");
        assert_message_id_form(&format!("{case}, send {index}"), &message_id);
        assert!(
            !minted.contains(&message_id),
            "case {case:?}: send {index} repeated the message id {message_id:?}"
        );
        minted.push(message_id);
    }
    assert_eq!(
        ses.logged_message_ids(),
        minted,
        "case {case:?}: the log must hold every accepted message under the id its send \
         answered with, in order"
    );
}
