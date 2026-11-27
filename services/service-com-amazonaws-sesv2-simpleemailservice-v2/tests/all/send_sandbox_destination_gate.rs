// SPDX-License-Identifier: CC0-1.0

//! WHICH of a message's addresses a send's verification check weighs, end to end over
//! HTTP.
//!
//! `GetAccount$ProductionAccessEnabled` is "if the value is false, then your account is in
//! the sandbox. When your account is in the sandbox, you can only send email to verified
//! identities", and `SendEmailRequest$FromEmailAddress` is "the email address to use as
//! the 'From' address for the email. The address that you specify has to be verified." So
//! one rule decides every send of this surface:
//!
//! * the identity a message is SENT FROM is weighed always;
//! * while the account is in the SANDBOX the addresses the message would be MAILED to are
//!   weighed too — every recipient of `Destination` (To:, Cc: and Bcc:) and the address
//!   `FeedbackForwardingEmailAddress` nominates;
//! * a mailbox of the mailbox simulator belongs to no account, so it needs no identity and
//!   fails no check;
//! * an address the check rejects makes the send fail with `MessageRejected` (HTTP 400),
//!   whose message names the Region the check ran in and EVERY address that failed it, in
//!   the order the request states them;
//! * once the account has production access the recipients are weighed by nothing, since
//!   such an account may address a message to anybody — while its sending identity is
//!   still weighed.
//!
//! A bulk send is weighed the same way, over the identities of the whole CALL: its one
//! sending identity and every mailbox any of its entries addresses, so one mailbox the check
//! rejects refuses the request rather than being reported as that entry's own
//! `BulkEmailEntryResult`.
//!
//! The rows below differ along what the rule generalises over: which recipient list carries
//! the address, how many addresses fail, whether an address is verified by its own identity
//! or by a domain above it, whether its identity is verified yet, the form the address
//! arrives in, the mailbox simulator, and the account's production access.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call of this suite is signed for, which is the Region whose identity
/// list the check runs against and the one its refusal names.
const HOME: &str = "us-east-1";

/// The domain identity the sends below are sent from, which authorises every mailbox in it
/// and in its subdomains.
const SENDER_DOMAIN: &str = "sender.example.com";

/// A mailbox of that domain.
const SENDER: &str = "campaigns@sender.example.com";

/// A recipient the account verifies as an address identity of its own.
const VERIFIED_RECIPIENT: &str = "friend@partner.example.net";

/// A domain the account holds no identity for at all, which is where every address the
/// check rejects below lives.
const STRANGER_DOMAIN: &str = "elsewhere.example.org";

/// The mailbox simulator's accepting mailbox: "a recipient that accepts whatever is
/// addressed to it", which belongs to no account.
const SIMULATOR_SUCCESS: &str = "success@simulator.amazonses.com";

/// Another of its mailboxes, so the rule is asserted over the simulator rather than over
/// one of its addresses.
const SIMULATOR_BOUNCE: &str = "bounce@simulator.amazonses.com";

/// One answered request: the status, the error shape when there was one, and the body.
pub(crate) struct Reply {
    pub(crate) status: u16,
    error_type: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    pub(crate) fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    pub(crate) fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, or `""` when it carries none.
    pub(crate) fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `(Status, MessageId, Error)` of every `BulkEmailEntryResults` entry, in the
    /// order the response reported them.
    fn results(&self) -> Vec<(String, String, String)> {
        self.body
            .get("BulkEmailEntryResults")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        let member = |name: &str| {
                            row.get(name)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        (member("Status"), member("MessageId"), member("Error"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Give the account PRODUCTION access on `engine`: the state that takes it out of the
/// sandbox.
///
/// A SANDBOX account may only send to identities it has verified, or to the mailbox
/// simulator — the rule this suite is about. A suite about anything ELSE a send does gives
/// the account production access first, so its sends are weighed on their sending identity
/// alone and may be addressed to any mailbox at all. Written on the account record because
/// a caller is not a request: `PutAccountDetails` asks for production access and the
/// account's review grants it, which is a span of its own and not what those suites are
/// about.
pub(crate) fn given_production_access(engine: &engine::InMemoryEngine) {
    engine
        .state::<ComAmazonawsSesv2SimpleemailserviceV2State>()
        .lock()
        .account
        .production_access_enabled = true;
}

/// The emulated service behind its router, plus the shared engine, so a case can settle the
/// verification transition the clock owes an identity and read the outbound log back.
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

    /// One call, signed for [`HOME`]: the credential scope is what the handler reads the
    /// Region of the call from.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{HOME}.amazonaws.com{path}"))
            .header("host", format!("email.{HOME}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// `CreateEmailIdentity`.
    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    /// `GetEmailIdentity`.
    async fn get_identity(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// Record `identity` and carry its verification through, which is what makes it an
    /// identity a send may name: the handler stores the instant the transition falls due
    /// rather than sleeping, so the instant is moved into the past and the identity read
    /// back to prove the store settled it.
    async fn given_verified_identity(&self, identity: &str) {
        let reply = self.create_identity(identity).await;
        assert_eq!(
            reply.status, 200,
            "case {identity:?}: expected CreateEmailIdentity to answer 200, got {} {:?}",
            reply.status, reply.body
        );
        self.set_verification_due(
            identity,
            SystemTime::now()
                .checked_sub(Duration::from_millis(1))
                .expect("an instant just before now"),
        );
        let reply = self.get_identity(identity).await;
        assert_eq!(
            reply.text("VerificationStatus"),
            "SUCCESS",
            "case {identity:?}: expected the verification to have gone through, got {:?}",
            reply.body
        );
    }

    /// Record `identity` and HOLD its verification off, so a case about an identity whose
    /// verification has not gone through cannot race the span the clock owes it.
    async fn given_pending_identity(&self, identity: &str) {
        let reply = self.create_identity(identity).await;
        assert_eq!(
            reply.status, 200,
            "case {identity:?}: expected CreateEmailIdentity to answer 200, got {} {:?}",
            reply.status, reply.body
        );
        self.set_verification_due(identity, SystemTime::now() + Duration::from_secs(3600));
        let reply = self.get_identity(identity).await;
        assert_eq!(
            reply.text("VerificationStatus"),
            "PENDING",
            "case {identity:?}: expected the verification to still be pending, got {:?}",
            reply.body
        );
    }

    /// Move the instant at which `identity`'s verification transition falls due.
    fn set_verification_due(&self, identity: &str, due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must be in the store"));
        record.verification_due_at = due_at;
    }

    /// `SendEmail` with the destination lists and feedback address a row states.
    async fn send(
        &self,
        to: &[&str],
        cc: &[&str],
        bcc: &[&str],
        feedback: Option<&str>,
        from: &str,
    ) -> Reply {
        let mut body = json!({
            "FromEmailAddress": from,
            "Destination": {"ToAddresses": to, "CcAddresses": cc, "BccAddresses": bcc},
            "Content": {"Simple": {
                "Subject": {"Data": "Recipient check"},
                "Body": {"Text": {"Data": "this send is weighed on its addresses"}},
            }},
        });
        if let Some(feedback) = feedback {
            body["FeedbackForwardingEmailAddress"] = json!(feedback);
        }
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    /// `SendBulkEmail` with one entry per recipient list.
    async fn send_bulk(&self, entries: &[Vec<&str>]) -> Reply {
        let body = json!({
            "FromEmailAddress": SENDER,
            "DefaultContent": {"Template": {
                "TemplateContent": {
                    "Subject": "Recipient check",
                    "Text": "this send is weighed on its addresses",
                },
                "TemplateData": "{}",
            }},
            "BulkEmailEntries": entries
                .iter()
                .map(|to| json!({"Destination": {"ToAddresses": to}}))
                .collect::<Vec<Value>>(),
        });
        self.call("POST", "/v2/email/outbound-bulk-emails", Some(body))
            .await
    }

    /// How many messages the outbound log holds, which is what proves a refused send
    /// recorded nothing.
    fn logged(&self) -> usize {
        self.engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>()
            .lock()
            .sent_messages
            .len()
    }
}

/// The sentence a send whose addresses failed the verification check is refused with: the
/// Region the check ran in, upper-cased, then every address that failed it.
fn refusal(identities: &[&str]) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {}",
        HOME.to_uppercase(),
        identities.join(", ")
    )
}

/// A mailbox of the domain no account holds an identity for.
fn stranger(local: &str) -> String {
    format!("{local}@{STRANGER_DOMAIN}")
}

/// One refused row: the addresses the message names, and the ones the refusal has to name.
struct Refused {
    case: &'static str,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    feedback: Option<String>,
    /// The identities the refusal names, in order.
    named: Vec<String>,
}

#[tokio::test]
async fn a_destination_the_sandbox_account_has_not_verified_refuses_the_send() {
    let rows = vec![
        Refused {
            case: "an unverified To: address",
            to: vec![stranger("stranger")],
            cc: Vec::new(),
            bcc: Vec::new(),
            feedback: None,
            named: vec![stranger("stranger")],
        },
        Refused {
            case: "an unverified Cc: beside a verified To:",
            to: vec![VERIFIED_RECIPIENT.to_string()],
            cc: vec![stranger("carbon")],
            bcc: Vec::new(),
            feedback: None,
            named: vec![stranger("carbon")],
        },
        Refused {
            case: "an unverified Bcc: beside a verified To:",
            to: vec![VERIFIED_RECIPIENT.to_string()],
            cc: Vec::new(),
            bcc: vec![stranger("blind")],
            feedback: None,
            named: vec![stranger("blind")],
        },
        Refused {
            case: "two unverified addresses, one To: and one Cc:",
            to: vec![stranger("first")],
            cc: vec![stranger("second")],
            bcc: Vec::new(),
            feedback: None,
            named: vec![stranger("first"), stranger("second")],
        },
        Refused {
            case: "the same unverified mailbox in two lists, which is one identity",
            to: vec![stranger("twice")],
            cc: vec![stranger("twice")],
            bcc: Vec::new(),
            feedback: None,
            named: vec![stranger("twice")],
        },
        Refused {
            case: "an unverified mailbox behind a display name",
            to: vec![format!("Stranger <{}>", stranger("wrapped"))],
            cc: Vec::new(),
            bcc: Vec::new(),
            feedback: None,
            named: vec![stranger("wrapped")],
        },
        Refused {
            case: "an unverified feedback-forwarding address, every recipient verified",
            to: vec![VERIFIED_RECIPIENT.to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            feedback: Some(stranger("bounces")),
            named: vec![stranger("bounces")],
        },
        Refused {
            case: "a recipient of the simulator beside an unverified one",
            to: vec![SIMULATOR_SUCCESS.to_string(), stranger("beside")],
            cc: Vec::new(),
            bcc: Vec::new(),
            feedback: None,
            named: vec![stranger("beside")],
        },
    ];

    for row in &rows {
        let ses = Ses::new();
        ses.given_verified_identity(SENDER_DOMAIN).await;
        ses.given_verified_identity(VERIFIED_RECIPIENT).await;

        let to: Vec<&str> = row.to.iter().map(String::as_str).collect();
        let cc: Vec<&str> = row.cc.iter().map(String::as_str).collect();
        let bcc: Vec<&str> = row.bcc.iter().map(String::as_str).collect();
        let reply = ses
            .send(&to, &cc, &bcc, row.feedback.as_deref(), SENDER)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected HTTP 400, got {} {:?}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "MessageRejected",
            "case {:?}: expected the modelled MessageRejected, got {:?}",
            row.case,
            reply.body
        );
        let named: Vec<&str> = row.named.iter().map(String::as_str).collect();
        let expected = refusal(&named);
        assert_eq!(
            reply.message(),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            row.case,
            reply.message()
        );
        assert_eq!(
            ses.logged(),
            0,
            "case {:?}: a refused send records no message, got {} logged",
            row.case,
            ses.logged()
        );
    }
}

#[tokio::test]
async fn an_identity_whose_verification_has_not_gone_through_is_no_destination_yet() {
    // The other side of the same reading: a recipient the account has ASKED SES about is
    // not one it may send to until the verification has gone through, and then it is.
    let ses = Ses::new();
    ses.given_verified_identity(SENDER_DOMAIN).await;
    ses.given_pending_identity(VERIFIED_RECIPIENT).await;

    let reply = ses.send(&[VERIFIED_RECIPIENT], &[], &[], None, SENDER).await;
    let expected = refusal(&[VERIFIED_RECIPIENT]);
    assert_eq!(
        reply.status, 400,
        "case a pending recipient: expected HTTP 400, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.message(),
        expected,
        "case a pending recipient: expected {expected:?}, got {:?}",
        reply.message()
    );

    ses.set_verification_due(
        VERIFIED_RECIPIENT,
        SystemTime::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant just before now"),
    );
    let reply = ses.send(&[VERIFIED_RECIPIENT], &[], &[], None, SENDER).await;
    assert_eq!(
        reply.status, 200,
        "case the same recipient once verified: expected HTTP 200, got {} {:?}",
        reply.status, reply.body
    );
    assert!(
        !reply.text("MessageId").is_empty(),
        "case the same recipient once verified: expected a MessageId, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn a_verified_destination_or_the_mailbox_simulator_is_served_in_the_sandbox() {
    // (case, To:, Cc:) — every address is either an identity this account has verified, a
    // mailbox a verified domain identity covers, or the mailbox simulator's.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "an address identity of its own",
            vec![VERIFIED_RECIPIENT],
            vec![],
        ),
        (
            "a mailbox the verified domain identity covers",
            vec![SENDER],
            vec![],
        ),
        (
            "a mailbox in a subdomain of that domain",
            vec!["bot@mail.sender.example.com"],
            vec![],
        ),
        (
            "the simulator's accepting mailbox",
            vec![SIMULATOR_SUCCESS],
            vec![],
        ),
        (
            "another mailbox of the simulator",
            vec![SIMULATOR_BOUNCE],
            vec![],
        ),
        (
            "the simulator in any spelling of its domain",
            vec!["success@SIMULATOR.AmazonSES.com"],
            vec![],
        ),
        (
            "a verified address and the simulator together",
            vec![VERIFIED_RECIPIENT],
            vec![SIMULATOR_SUCCESS],
        ),
    ];

    let ses = Ses::new();
    ses.given_verified_identity(SENDER_DOMAIN).await;
    ses.given_verified_identity(VERIFIED_RECIPIENT).await;

    let mut ids: Vec<String> = Vec::new();
    for (case, to, cc) in &rows {
        let reply = ses.send(to, cc, &[], None, SENDER).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            reply.status, reply.body
        );
        let message_id = reply.text("MessageId");
        assert!(
            !message_id.is_empty(),
            "case {case:?}: expected a MessageId, got {:?}",
            reply.body
        );
        assert!(
            !ids.contains(&message_id),
            "case {case:?}: expected an id of its own, but {message_id} was already issued \
             ({ids:?})"
        );
        ids.push(message_id);
    }
    assert_eq!(
        ses.logged(),
        rows.len(),
        "case the whole table: expected one logged message per served send, got {}",
        ses.logged()
    );
}

#[tokio::test]
async fn production_access_weighs_the_sending_identity_and_no_recipient() {
    // The rows the sandbox refuses are served once the account has production access,
    // because such an account may address a message to anybody.
    let served: Vec<(&str, Vec<&str>, Option<&str>)> = vec![
        ("an address no account verified", vec!["stranger@elsewhere.example.org"], None),
        (
            "several unverified addresses at once",
            vec!["first@elsewhere.example.org", "second@elsewhere.example.org"],
            None,
        ),
        (
            "an unverified feedback-forwarding address",
            vec!["stranger@elsewhere.example.org"],
            Some("bounces@elsewhere.example.org"),
        ),
    ];

    let ses = Ses::new();
    ses.given_verified_identity(SENDER_DOMAIN).await;
    given_production_access(&ses.engine);

    for (case, to, feedback) in &served {
        let reply = ses.send(to, &[], &[], *feedback, SENDER).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 once the account has production access, got {} {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.text("MessageId").is_empty(),
            "case {case:?}: expected a MessageId, got {:?}",
            reply.body
        );
    }

    // The sending identity is still weighed, and it is the ONLY address the refusal names:
    // the recipients of this send were weighed by nothing.
    let reply = ses
        .send(
            &["stranger@elsewhere.example.org"],
            &[],
            &[],
            None,
            "nobody@unverified.example.org",
        )
        .await;
    let expected = refusal(&["nobody@unverified.example.org"]);
    assert_eq!(
        reply.status, 400,
        "case an unverified sender with production access: expected HTTP 400, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "MessageRejected",
        "case an unverified sender with production access: expected MessageRejected, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.message(),
        expected,
        "case an unverified sender with production access: expected {expected:?}, got {:?}",
        reply.message()
    );
}

#[tokio::test]
async fn a_bulk_destination_the_sandbox_account_may_not_send_to_refuses_the_whole_call() {
    // The check weighs the identities of the CALL, whichever entry addresses them, so one
    // mailbox nobody verified refuses the request and nothing of it is sent. The rows differ
    // along which entry carries the unverified mailbox, how many entries do, how many
    // mailboxes fail, and whether a verified mailbox or the simulator stands beside one.
    let ses = Ses::new();
    ses.given_verified_identity(SENDER_DOMAIN).await;
    ses.given_verified_identity(VERIFIED_RECIPIENT).await;

    let first = stranger("bulk-first");
    let second = stranger("bulk-second");
    /// One row: the entries the call states, and the identities its refusal must name.
    struct Row<'a> {
        case: &'a str,
        entries: Vec<Vec<&'a str>>,
        failed: Vec<&'a str>,
    }
    let rows: Vec<Row<'_>> = vec![
        Row {
            case: "one entry, addressing a mailbox nobody verified",
            entries: vec![vec![first.as_str()]],
            failed: vec![first.as_str()],
        },
        Row {
            case: "the unverified mailbox in the LAST entry, behind served ones",
            entries: vec![
                vec![VERIFIED_RECIPIENT],
                vec![SIMULATOR_SUCCESS],
                vec![first.as_str()],
            ],
            failed: vec![first.as_str()],
        },
        Row {
            case: "an unverified mailbox beside a verified one in the same entry",
            entries: vec![vec![VERIFIED_RECIPIENT, first.as_str()]],
            failed: vec![first.as_str()],
        },
        Row {
            case: "two entries failing on two different mailboxes",
            entries: vec![
                vec![first.as_str()],
                vec![VERIFIED_RECIPIENT],
                vec![second.as_str()],
            ],
            failed: vec![first.as_str(), second.as_str()],
        },
        Row {
            case: "the same unverified mailbox in two entries, named once",
            entries: vec![vec![first.as_str()], vec![first.as_str()]],
            failed: vec![first.as_str()],
        },
    ];

    for Row {
        case,
        entries,
        failed,
    } in rows
    {
        let reply = ses.send_bulk(&entries).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected the call refused with HTTP 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "MessageRejected",
            "case {case:?}: expected the modelled MessageRejected, got {:?}",
            reply.error_code()
        );
        let expected = refusal(&failed);
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
        assert!(
            reply.results().is_empty(),
            "case {case:?}: a refused call reports no entry result, got {:?}",
            reply.results()
        );
        assert_eq!(
            ses.logged(),
            0,
            "case {case:?}: a refused call sends nothing, got {} logged",
            ses.logged()
        );
    }
}

#[tokio::test]
async fn a_bulk_call_every_destination_of_which_passes_the_check_is_served() {
    // The other side of the same rule: a call none of whose addresses fails the check is
    // served in the sandbox, one message per entry.
    let ses = Ses::new();
    ses.given_verified_identity(SENDER_DOMAIN).await;
    ses.given_verified_identity(VERIFIED_RECIPIENT).await;

    let entries: Vec<Vec<&str>> = vec![
        vec![VERIFIED_RECIPIENT],
        vec![SIMULATOR_SUCCESS],
        vec![SIMULATOR_BOUNCE, VERIFIED_RECIPIENT],
    ];
    let reply = ses.send_bulk(&entries).await;
    assert_eq!(
        reply.status, 200,
        "case a served bulk send: expected HTTP 200 with one result per entry, got {} {:?}",
        reply.status, reply.body
    );
    let results = reply.results();
    assert_eq!(
        results.len(),
        entries.len(),
        "case a served bulk send: expected one result per entry, got {results:?}"
    );
    for (index, (status, message_id, error)) in results.iter().enumerate() {
        assert_eq!(
            status, "SUCCESS",
            "case entry {index}: expected SUCCESS, got {results:?}"
        );
        assert!(
            error.is_empty() && !message_id.is_empty(),
            "case entry {index}: a served entry reports the id it minted and no error, got \
             {results:?}"
        );
    }
    assert_eq!(
        ses.logged(),
        entries.len(),
        "case a served bulk send: expected one logged message per entry, got {}",
        ses.logged()
    );
}
