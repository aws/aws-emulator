// SPDX-License-Identifier: CC0-1.0

//! The REQUEST/CONTENT validation stage of the SESv2 send path, end to end over HTTP.
//!
//! Amazon SES reads an outbound send in two stages: it validates the request and the message
//! content FIRST — the addresses the message names, the recipients it addresses, the headers,
//! the tags and the attachments it carries, the character sets its parts are written in and
//! the template it renders from — and only then weighs the message against the account's own
//! state: its sending switches, the sending identities the send asks to use, and the
//! resources the content names. So a malformed request is refused as the caller's mistake
//! (`BadRequestException`, 400) naming the element that is wrong, whatever the account holds,
//! and a caller is never pointed at an identity or a resource for a defect in its own
//! request.
//!
//! Every case below drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — and each refusal is asserted from a sender the account has NOT
//! verified AND with a configuration set the account does not hold, which is the ordering
//! half of the rule: a request that is malformed is refused for that, while the same request
//! made well formed reaches the configuration-set lookup (404) instead. The counter-cases
//! are what keep the stage from refusing calls SES serves.
//!
//! The per-entry half of the same rule lives in `SendBulkEmail`: a defect in ONE entry's
//! destination is reported as that entry's `BulkEmailEntryResult` inside a 200 response,
//! while a defect in what the request states for every message refuses the whole call.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const HOME: &str = "us-east-1";

/// A sender whose syntax is fine and whose identity the account has NOT verified, so a
/// request that passes the validation stage is refused by a LATER gate rather than here.
const UNVERIFIED: &str = "stranger@example.com";

/// One of the mailbox SIMULATOR's own mailboxes: it belongs to no account and needs no
/// identity, so a case that addresses it reads the sender's own gate alone.
const SIMULATOR: &str = "success@simulator.amazonses.com";

/// The verified sending domain the counter-cases send from, and an address inside it.
const VERIFIED_DOMAIN: &str = "senders.example.com";
const VERIFIED_SENDER: &str = "sender@senders.example.com";

/// A configuration set name the account holds nothing under: naming it is what makes the
/// NEXT stage observable, because a well-formed request reaches its lookup and a malformed
/// one does not.
const ABSENT_SET: &str = "no-such-set";

/// The stored template the templated cases render from, and its parts.
const TEMPLATE: &str = "receipt";
const TEMPLATE_SUBJECT: &str = "Your receipt, {{name}}";
const TEMPLATE_TEXT: &str = "Thanks {{name}}.";

/// The values that go with that template.
const DATA: &str = "{\"name\":\"Ada\"}";

/// What the configuration-set lookup answers for [`ABSENT_SET`], which is the refusal a
/// request that PASSED the validation stage gets.
fn absent_set_message() -> String {
    format!("Configuration set <{ABSENT_SET}> does not exist.")
}

/// One answered request: the status, the error shape when there was one, and the parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `(Status, MessageId, Error)` of every `BulkEmailEntryResults` entry, in the order
    /// the response reported them. An absent member reads as the empty string.
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

/// The emulated service behind its router, plus the shared engine so a case can settle the
/// verification transition the clock owes an identity.
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

    /// One call, signed for `HOME`: the credential scope is what the handler reads the Region
    /// of the call from.
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

    /// `SendEmail` with `body` as the whole request.
    async fn send(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    /// `SendBulkEmail` with `body` as the whole request.
    async fn send_bulk(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-bulk-emails", Some(body))
            .await
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

    /// `CreateEmailTemplate` for the stored template the templated cases render from.
    async fn create_template(&self) -> Reply {
        self.call(
            "POST",
            "/v2/email/templates",
            Some(json!({
                "TemplateName": TEMPLATE,
                "TemplateContent": {"Subject": TEMPLATE_SUBJECT, "Text": TEMPLATE_TEXT},
            })),
        )
        .await
    }

    /// Bring every identity's verification transition into the past, so a case that sends
    /// from an identity the account holds does not race the span the clock owes it.
    fn settle_identities(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .expect("an instant before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
        store.settle(SystemTime::now());
    }
}

/// A service holding the verified sending domain the counter-cases send from.
async fn verified() -> Ses {
    let ses = Ses::new();
    let created = ses.create_identity(VERIFIED_DOMAIN).await;
    assert_eq!(
        created.status, 200,
        "the sending domain must be created first, got {} {:?}",
        created.status, created.body
    );
    ses.settle_identities();
    ses
}

/// A simple message with one text part, which every case that is not about the content uses.
fn simple() -> Value {
    json!({"Simple": {"Subject": {"Data": "s"}, "Body": {"Text": {"Data": "t"}}}})
}

/// A `SendEmail` request from `from` naming [`ABSENT_SET`]: a well-formed one is refused by
/// the configuration-set lookup, so what a case asserts is whether the validation stage
/// refused it first.
fn request(from: &str) -> Value {
    json!({
        "FromEmailAddress": from,
        "ConfigurationSetName": ABSENT_SET,
        "Destination": {"ToAddresses": ["reader@example.net"]},
        "Content": simple(),
    })
}

/// Assert that `reply` is the 400 `BadRequestException` carrying `message`.
fn refused(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "BadRequestException"),
        "case {case:?}: expected a 400 BadRequestException, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected the message {message:?}, got {:?}",
        reply.message()
    );
}

/// Assert that `reply` is the configuration-set 404 — the refusal a request that PASSED the
/// validation stage gets, since the set it names is one the account does not hold.
fn passed_validation(case: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (404, "NotFoundException"),
        "case {case:?}: expected the request to pass validation and reach the \
         configuration-set lookup, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.message(),
        absent_set_message(),
        "case {case:?}: expected the configuration-set refusal, got {:?}",
        reply.message()
    );
}

/// The defect classes SES's address reader reports, each with the sentence it reports it
/// with: a value with no domain at all, each half missing, each class of character neither
/// half admits — including the non-ASCII character SES admits in neither, since it "does not
/// support the SMTPUTF8 extension" (`Destination`) — and a domain that is no domain name,
/// since an address member documents the form `UserName@[SubDomain.]Domain.TopLevelDomain`.
const MALFORMED_ADDRESSES: &[(&str, &str, &str)] = &[
    ("no @ at all", "no-domain-sender", "Missing final '@domain'"),
    ("only a domain", "@example.com", "Missing local name"),
    ("only a local part", "user@", "Missing domain"),
    (
        "a space in the local part",
        "us er@example.com",
        "Local address contains control or whitespace",
    ),
    (
        "a non-ASCII local part",
        "üser@example.com",
        "Local address contains control or whitespace",
    ),
    (
        "a special in the local part",
        "us(er)@example.com",
        "Local address contains illegal character",
    ),
    (
        "a local part starting with the atom separator",
        ".user@example.com",
        "Local address starts with dot",
    ),
    (
        "a local part ending with it",
        "user.@example.com",
        "Local address ends with dot",
    ),
    (
        "an empty atom inside the local part",
        "us..er@example.com",
        "Local address contains dot-dot",
    ),
    (
        "a space in the domain",
        "user@exa mple.com",
        "Domain contains control or whitespace",
    ),
    (
        "a domain that is not Punycode",
        "user@exämple.com",
        "Domain contains control or whitespace",
    ),
    (
        "a special in the domain",
        "user@exa(mple).com",
        "Domain contains illegal character",
    ),
    (
        "an empty label inside the domain",
        "user@example..com",
        "Domain contains dot-dot",
    ),
    (
        "an underscore in the domain",
        "user@exa_mple.com",
        "Domain contains illegal character",
    ),
    (
        "a domain with no top-level label",
        "user@example",
        "Illegal address",
    ),
    (
        "a domain ending with the label separator",
        "user@example.",
        "Illegal address",
    ),
    (
        "a domain starting with it",
        "user@.example.com",
        "Illegal address",
    ),
    (
        "an all-numeric top-level label",
        "user@example.123",
        "Illegal address",
    ),
];

/// Where one address sits in a `SendEmail` request. Every member below names a mailbox, so
/// the one address form is read over all of them.
const ADDRESS_MEMBERS: &[&str] = &[
    "FromEmailAddress",
    "ToAddresses",
    "CcAddresses",
    "BccAddresses",
    "ReplyToAddresses",
    "FeedbackForwardingEmailAddress",
];

/// `request(UNVERIFIED)` with `address` put in the member `member`.
fn addressed(member: &str, address: &str) -> Value {
    let mut body = request(UNVERIFIED);
    match member {
        "FromEmailAddress" => body["FromEmailAddress"] = json!(address),
        "ToAddresses" => body["Destination"]["ToAddresses"] = json!([address]),
        "CcAddresses" => body["Destination"]["CcAddresses"] = json!([address]),
        "BccAddresses" => body["Destination"]["BccAddresses"] = json!([address]),
        "ReplyToAddresses" => body["ReplyToAddresses"] = json!([address]),
        "FeedbackForwardingEmailAddress" => {
            body["FeedbackForwardingEmailAddress"] = json!(address);
        }
        other => panic!("no such address member: {other}"),
    }
    body
}

#[tokio::test]
async fn an_address_that_names_no_mailbox_is_refused_before_any_gate_or_lookup() {
    for (case, address, sentence) in MALFORMED_ADDRESSES {
        for member in ADDRESS_MEMBERS {
            let ses = Ses::new();
            let case = format!("{case} in {member}");
            let reply = ses.send(addressed(member, address)).await;
            refused(&case, &reply, sentence);
        }
    }
}

#[tokio::test]
async fn the_addresses_ses_reads_let_the_request_through_to_the_next_stage() {
    // The counter-case: the same members carrying values that DO name a mailbox, in the
    // forms a caller may write one — bare, behind a display name, with the atom punctuation
    // a local part admits, and on a Punycode domain.
    for (case, address) in [
        ("a bare address", "reader@example.net"),
        ("an address behind a display name", "Reader <r@example.net>"),
        ("atom punctuation in the local part", "a+b.c_d-e@example.net"),
        ("a Punycode domain", "reader@xn--exmple-cua.com"),
        ("a subdomain", "reader@mail.corp.example.net"),
    ] {
        for member in ADDRESS_MEMBERS {
            let ses = Ses::new();
            let case = format!("{case} in {member}");
            let reply = ses.send(addressed(member, address)).await;
            passed_validation(&case, &reply);
        }
    }
}

#[tokio::test]
async fn a_header_the_service_writes_itself_is_refused_naming_that_header() {
    // The rows span the fields the request already states through its own members and the
    // MIME fields SES composes the message with, in several spellings — a header field name
    // is case-insensitive — and the recorded case (`From`) is one row of it.
    for (case, name) in [
        ("the originator field", "From"),
        ("the same, lower-cased", "from"),
        ("a recipient field", "To"),
        ("another recipient field", "Bcc"),
        ("the subject", "Subject"),
        ("the reply field", "Reply-To"),
        ("the bounce field", "Return-Path"),
        ("a MIME field", "Content-Type"),
        ("another MIME field", "MIME-Version"),
    ] {
        // On a simple message's own `Headers`, and on the `Headers` of a templated one: both
        // are "the list of message headers that will be added to the email message".
        let mut simple_content = simple();
        simple_content["Simple"]["Headers"] = json!([{"Name": name, "Value": "v"}]);
        let templated = json!({"Template": {
            "TemplateName": TEMPLATE,
            "TemplateData": DATA,
            "Headers": [{"Name": name, "Value": "v"}],
        }});
        for (form, content) in [("a simple message", simple_content), ("a templated message", templated)]
        {
            let ses = Ses::new();
            let created = ses.create_template().await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the template must be saved first, got {} {:?}",
                created.status, created.body
            );
            let mut body = request(UNVERIFIED);
            body["Content"] = content;
            let reply = ses.send(body).await;
            refused(
                &format!("{case} on {form}"),
                &reply,
                &format!("Header <{name}> is not supported"),
            );
        }
    }
}

#[tokio::test]
async fn a_header_of_the_callers_own_lets_the_request_through() {
    for (case, name) in [
        ("a custom field", "X-Campaign"),
        ("another custom field", "X-Audience"),
        ("a correlation field", "In-Reply-To"),
        ("a field whose name only starts like a reserved one", "From-Us"),
    ] {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        body["Content"]["Simple"]["Headers"] = json!([{"Name": name, "Value": "v"}]);
        let reply = ses.send(body).await;
        passed_validation(case, &reply);
    }
}

#[tokio::test]
async fn a_message_tag_outside_the_form_its_member_states_is_refused() {
    // `MessageTag$Name`/`MessageTag$Value`: "It can only contain ASCII letters (a–z, A–Z),
    // numbers (0–9), underscores (_), or dashes (-)" and "It can contain no more than 256
    // characters". The name names ONE tag of the message, so a second tag of the same name
    // is no tag set either.
    let long = "a".repeat(257);
    let cases: Vec<(String, Value, String)> = vec![
        (
            "a space in the name".to_string(),
            json!([{"Name": "bad name!", "Value": "ok"}]),
            "Invalid tag name <bad name!>: only alphanumeric ASCII characters, '_',  '-' are \
             allowed."
                .to_string(),
        ),
        (
            "a non-ASCII name".to_string(),
            json!([{"Name": "naïve", "Value": "ok"}]),
            "Invalid tag name <naïve>: only alphanumeric ASCII characters, '_',  '-' are \
             allowed."
                .to_string(),
        ),
        (
            "an empty name".to_string(),
            json!([{"Name": "", "Value": "ok"}]),
            "Invalid tag name <>: only alphanumeric ASCII characters, '_',  '-' are allowed."
                .to_string(),
        ),
        (
            "a name past 256 characters".to_string(),
            json!([{"Name": long, "Value": "ok"}]),
            format!(
                "Invalid tag name <{long}>: only alphanumeric ASCII characters, '_',  '-' are \
                 allowed."
            ),
        ),
        (
            "a dot in the value".to_string(),
            json!([{"Name": "campaign", "Value": "spring.2026"}]),
            "Invalid tag value <spring.2026>: only alphanumeric ASCII characters, '_',  '-' \
             are allowed."
                .to_string(),
        ),
        (
            "a value past 256 characters".to_string(),
            json!([{"Name": "campaign", "Value": long}]),
            format!(
                "Invalid tag value <{long}>: only alphanumeric ASCII characters, '_',  '-' are \
                 allowed."
            ),
        ),
        (
            "two tags of one name".to_string(),
            json!([{"Name": "probe", "Value": "a"}, {"Name": "probe", "Value": "b"}]),
            "Duplicate tag <probe>".to_string(),
        ),
    ];

    for (case, tags, message) in cases {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        body["EmailTags"] = tags;
        let reply = ses.send(body).await;
        refused(&case, &reply, &message);
    }
}

#[tokio::test]
async fn the_message_tags_the_form_admits_let_the_request_through() {
    for (case, tags) in [
        ("letters, digits and the two punctuation marks", json!([{"Name": "Campaign-1_x", "Value": "Spring-2026_v2"}])),
        ("an empty value", json!([{"Name": "campaign", "Value": ""}])),
        ("two tags of different names", json!([{"Name": "a", "Value": "1"}, {"Name": "b", "Value": "2"}])),
        ("a name of exactly 256 characters", json!([{"Name": "x".repeat(256), "Value": "ok"}])),
    ] {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        body["EmailTags"] = tags;
        let reply = ses.send(body).await;
        passed_validation(case, &reply);
    }
}

#[tokio::test]
async fn an_attachment_the_service_will_not_carry_is_refused_by_its_file_name() {
    // "Amazon SES restricts certain file extensions" (`Attachment$FileName`): the rows span
    // the classes of name SES will not attach — the executable, script and installer types,
    // whatever case the extension is written in, and a name that names no file at all.
    for (case, file_name) in [
        ("an executable", "probe.exe"),
        ("a batch script", "payroll.bat"),
        ("a script", "setup.js"),
        ("a screen saver", "holiday.scr"),
        ("an installer", "tool.msi"),
        ("the same extension upper-cased", "PROBE.EXE"),
        ("no file name at all", ""),
        ("a blank file name", "   "),
    ] {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        body["Content"]["Simple"]["Attachments"] = json!([{
            "FileName": file_name,
            "RawContent": "aGVsbG8=",
        }]);
        let reply = ses.send(body).await;
        refused(case, &reply, &format!("Illegal filename '{file_name}'."));
    }
}

#[tokio::test]
async fn an_attachments_content_type_has_to_be_a_media_type() {
    for (case, content_type, message) in [
        (
            "no separator at all",
            "textplain",
            "In Content-Type string <textplain>, expected '/', got null",
        ),
        (
            "another type with no separator",
            "application",
            "In Content-Type string <application>, expected '/', got null",
        ),
        (
            "no subtype after the separator",
            "text/",
            "In Content-Type string <text/>, expected MIME subtype, got null",
        ),
    ] {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        body["Content"]["Simple"]["Attachments"] = json!([{
            "FileName": "note.txt",
            "RawContent": "aGVsbG8=",
            "ContentType": content_type,
        }]);
        let reply = ses.send(body).await;
        refused(case, &reply, message);
    }

    // The counter-case: the media types the member documents ("Example: application/pdf,
    // image/jpeg") and a file type SES carries.
    for (case, file_name, content_type) in [
        ("a document", "receipt.pdf", "application/pdf"),
        ("an image", "logo.jpeg", "image/jpeg"),
        ("text", "note.txt", "text/plain"),
        ("a name with no extension", "README", "text/plain"),
    ] {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        body["Content"]["Simple"]["Attachments"] = json!([{
            "FileName": file_name,
            "RawContent": "aGVsbG8=",
            "ContentType": content_type,
        }]);
        let reply = ses.send(body).await;
        passed_validation(case, &reply);
    }
}

#[tokio::test]
async fn a_part_naming_a_character_set_the_service_cannot_read_is_refused() {
    for (case, charset) in [
        ("a label naming no character set", "NOPE-9"),
        ("text that is no label at all", "not a charset"),
        ("a code page that does not exist", "windows-9999"),
    ] {
        // On each part of a simple message that carries its own charset: the subject, the
        // text body and the HTML body.
        for part in ["Subject", "Text", "Html"] {
            let ses = Ses::new();
            let mut body = request(UNVERIFIED);
            let content = json!({"Data": "x", "Charset": charset});
            match part {
                "Subject" => body["Content"]["Simple"]["Subject"] = content,
                part => body["Content"]["Simple"]["Body"][part] = content,
            }
            let reply = ses.send(body).await;
            refused(
                &format!("{case} on {part}"),
                &reply,
                &format!("Unsupported charset '{charset}'."),
            );
        }
    }

    // The counter-case: the three the member names, the alias of one of them, and the
    // default 7-bit set.
    for (case, charset) in [
        ("the member's first example", "UTF-8"),
        ("its alias", "utf8"),
        ("the member's second example", "ISO-8859-1"),
        ("the member's third example", "Shift_JIS"),
        ("the 7-bit default", "us-ascii"),
    ] {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        body["Content"]["Simple"]["Body"]["Text"] = json!({"Data": "x", "Charset": charset});
        let reply = ses.send(body).await;
        passed_validation(case, &reply);
    }
}

#[tokio::test]
async fn one_message_addresses_at_most_fifty_recipients() {
    // The ceiling is on the message's whole recipient set, across its three recipient lines,
    // and it is weighed AFTER the configuration set the message names is resolved: a request
    // past the ceiling that also names a set the account does not hold is refused for the
    // set.
    let address = |index: usize| format!("reader-{index}@example.net");
    let rows: &[(&str, usize, usize, usize)] = &[
        ("fifty on one line", 50, 0, 0),
        ("fifty across two lines", 25, 25, 0),
        ("fifty across three", 20, 20, 10),
        ("fifty-one on one line", 51, 0, 0),
        ("fifty-one across three", 20, 20, 11),
        ("one", 1, 0, 0),
    ];

    for (case, to, cc, bcc) in rows {
        let ses = verified().await;
        // About the recipient CEILING, so the account is given production access and no row
        // is also weighed on which mailboxes it addresses
        // (`send_sandbox_destination_gate.rs`).
        crate::send_sandbox_destination_gate::given_production_access(&ses.engine);
        let mut index = 0;
        let mut line = |count: usize| {
            let values: Vec<String> = (0..count)
                .map(|_| {
                    index += 1;
                    address(index)
                })
                .collect();
            values
        };
        let body = json!({
            "FromEmailAddress": VERIFIED_SENDER,
            "Destination": {
                "ToAddresses": line(*to),
                "CcAddresses": line(*cc),
                "BccAddresses": line(*bcc),
            },
            "Content": simple(),
        });
        let reply = ses.send(body).await;
        let total = to + cc + bcc;
        if total > 50 {
            refused(case, &reply, "Recipient count exceeds 50.");
        } else {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected {total} recipients to be accepted, got {} {:?}",
                reply.status, reply.body
            );
        }
    }
}

#[tokio::test]
async fn the_recipient_ceiling_is_weighed_after_the_configuration_set_is_resolved() {
    let ses = verified().await;
    let mut body = request(VERIFIED_SENDER);
    body["Destination"]["ToAddresses"] = Value::Array(
        (0..51)
            .map(|index| json!(format!("reader-{index}@example.net")))
            .collect(),
    );
    let reply = ses.send(body).await;
    passed_validation("fifty-one recipients and a set the account does not hold", &reply);
}

#[tokio::test]
async fn templated_content_states_which_template_and_the_values_for_it() {
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a stored template with no values at all",
            json!({"TemplateName": TEMPLATE}),
            "Template data cannot be empty",
        ),
        (
            "a stored template with empty values",
            json!({"TemplateName": TEMPLATE, "TemplateData": ""}),
            "Template data cannot be empty",
        ),
        (
            "neither a name nor an ARN nor content",
            json!({"TemplateData": DATA}),
            "TemplateName and TemplateArn cannot be both empty",
        ),
        (
            "inline content with no subject",
            json!({"TemplateContent": {"Text": "t"}, "TemplateData": DATA}),
            "Template is missing required fields.",
        ),
        (
            "inline content with an empty subject",
            json!({"TemplateContent": {"Subject": "", "Text": "t"}, "TemplateData": DATA}),
            "Template is missing required fields.",
        ),
        (
            "inline content with no body part",
            json!({"TemplateContent": {"Subject": "s"}, "TemplateData": DATA}),
            "Template is missing required fields.",
        ),
        (
            "inline content that is empty",
            json!({"TemplateContent": {}, "TemplateData": DATA}),
            "Template is missing required fields.",
        ),
    ];

    for (case, template, message) in rows {
        let ses = Ses::new();
        let created = ses.create_template().await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the template must be saved first, got {} {:?}",
            created.status, created.body
        );
        let mut body = request(UNVERIFIED);
        body["Content"] = json!({"Template": template});
        let reply = ses.send(body).await;
        refused(case, &reply, message);
    }

    // The counter-cases: content that states a template and values for it goes on to the
    // next stage, whether the template is the stored one or carried inline.
    for (case, template) in [
        (
            "the stored template",
            json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
        ),
        (
            "content carried inline, with a text part",
            json!({"TemplateContent": {"Subject": "s", "Text": "t"}, "TemplateData": DATA}),
        ),
        (
            "content carried inline, with an HTML part",
            json!({"TemplateContent": {"Subject": "s", "Html": "<p>t</p>"}, "TemplateData": DATA}),
        ),
    ] {
        let ses = Ses::new();
        let created = ses.create_template().await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the template must be saved first, got {} {:?}",
            created.status, created.body
        );
        let mut body = request(UNVERIFIED);
        body["Content"] = json!({"Template": template});
        let reply = ses.send(body).await;
        passed_validation(case, &reply);
    }
}

#[tokio::test]
async fn a_message_that_addresses_nobody_is_refused_as_the_recipient_header_it_has_not_got() {
    for (case, destination) in [
        ("no Destination member at all", None),
        ("a destination with no member", Some(json!({}))),
        (
            "a destination whose lines are all empty",
            Some(json!({"ToAddresses": [], "CcAddresses": [], "BccAddresses": []})),
        ),
    ] {
        let ses = Ses::new();
        let mut body = request(UNVERIFIED);
        match destination {
            None => {
                body.as_object_mut()
                    .expect("the request is an object")
                    .remove("Destination");
            }
            Some(value) => body["Destination"] = value,
        }
        let reply = ses.send(body).await;
        refused(case, &reply, "Missing required header 'To'.");
    }
}

#[tokio::test]
async fn the_failed_check_names_every_identity_the_send_asked_to_use() {
    // "The address that you specify has to be verified": the check covers every identity
    // ONE message involves — the sending identity, each recipient the message addresses,
    // and the identity bounce and complaint notifications are forwarded to, whose use is
    // authorised by that identity's own policy
    // (`FeedbackForwardingEmailAddressIdentityArn`). The refusal names every one of them
    // that failed, comma-separated, in the order the request states them, so a caller
    // learns which identities to verify rather than only the first. The mailbox simulator's
    // own mailboxes belong to no account and need no identity, so they never fail it.
    //
    // The rows span which of those members carries an unverified identity, how many do, a
    // recipient inside the verified domain, a recipient on each recipient line, and one
    // mailbox named twice.
    struct Row {
        case: &'static str,
        from: &'static str,
        to: &'static [&'static str],
        cc: &'static [&'static str],
        bcc: &'static [&'static str],
        feedback: Option<&'static str>,
        /// The identities the refusal must name, in order.
        failed: &'static [&'static str],
    }
    let rows: &[Row] = &[
        Row {
            case: "an unverified sender, with the mailbox simulator as the recipient",
            from: UNVERIFIED,
            to: &["success@simulator.amazonses.com"],
            cc: &[],
            bcc: &[],
            feedback: None,
            failed: &[UNVERIFIED],
        },
        Row {
            case: "an unverified sender and an unverified recipient",
            from: UNVERIFIED,
            to: &["reader@example.net"],
            cc: &[],
            bcc: &[],
            feedback: None,
            failed: &[UNVERIFIED, "reader@example.net"],
        },
        Row {
            case: "a recipient on each line, in request order",
            from: UNVERIFIED,
            to: &["to@example.net"],
            cc: &["cc@example.net"],
            bcc: &["bcc@example.net"],
            feedback: None,
            failed: &[
                UNVERIFIED,
                "to@example.net",
                "cc@example.net",
                "bcc@example.net",
            ],
        },
        Row {
            case: "a recipient inside the verified domain, which passes the check",
            from: UNVERIFIED,
            to: &[VERIFIED_SENDER],
            cc: &[],
            bcc: &[],
            feedback: None,
            failed: &[UNVERIFIED],
        },
        Row {
            case: "an unverified sender and an unverified bounce address",
            from: UNVERIFIED,
            to: &["success@simulator.amazonses.com"],
            cc: &[],
            bcc: &[],
            feedback: Some("bounces@example.net"),
            failed: &[UNVERIFIED, "bounces@example.net"],
        },
        Row {
            case: "one mailbox named as the sender and as the bounce address",
            from: UNVERIFIED,
            to: &["success@simulator.amazonses.com"],
            cc: &[],
            bcc: &[],
            feedback: Some(UNVERIFIED),
            failed: &[UNVERIFIED],
        },
        Row {
            case: "the sender behind a display name",
            from: "Stranger <stranger@example.com>",
            to: &["success@simulator.amazonses.com"],
            cc: &[],
            bcc: &[],
            feedback: None,
            failed: &[UNVERIFIED],
        },
        Row {
            case: "every member unverified at once",
            from: UNVERIFIED,
            to: &["reader@example.net"],
            cc: &[],
            bcc: &[],
            feedback: Some("bounces@example.org"),
            failed: &[UNVERIFIED, "reader@example.net", "bounces@example.org"],
        },
    ];

    for row in rows {
        let ses = verified().await;
        let mut body = json!({
            "FromEmailAddress": row.from,
            "Destination": {
                "ToAddresses": row.to,
                "CcAddresses": row.cc,
                "BccAddresses": row.bcc,
            },
            "Content": simple(),
        });
        if let Some(feedback) = row.feedback {
            body["FeedbackForwardingEmailAddress"] = json!(feedback);
        }
        let reply = ses.send(body).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "MessageRejected"),
            "case {:?}: expected a 400 MessageRejected, got {} {:?}",
            row.case,
            reply.status,
            reply.body
        );
        let expected = format!(
            "Email address is not verified. The following identities failed the check in \
             region {}: {}",
            HOME.to_uppercase(),
            row.failed.join(", ")
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {:?}: expected the message {expected:?}, got {:?}",
            row.case,
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_send_whose_every_identity_passes_the_check_is_accepted() {
    // The counter-case to the refusal above: the check is passed by an address inside the
    // verified domain and by the mailbox simulator's own mailboxes, so a send naming only
    // those is accepted.
    for (case, to) in [
        ("the mailbox simulator", "success@simulator.amazonses.com"),
        ("another of its mailboxes", "bounce@simulator.amazonses.com"),
        ("an address inside the verified domain", "reader@senders.example.com"),
    ] {
        let ses = verified().await;
        let reply = ses
            .send(json!({
                "FromEmailAddress": VERIFIED_SENDER,
                "Destination": {"ToAddresses": [to]},
                "FeedbackForwardingEmailAddress": "bounces@senders.example.com",
                "Content": simple(),
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the send to be accepted, got {} {:?}",
            reply.status, reply.body
        );
    }
}

/// A `SendBulkEmail` request from `from` through `template`, with `entries` as its entries.
fn bulk(from: &str, template: Value, entries: Vec<Value>) -> Value {
    json!({
        "FromEmailAddress": from,
        "DefaultContent": {"Template": template},
        "BulkEmailEntries": entries,
    })
}

/// One `BulkEmailEntries` entry addressing `to`.
fn bulk_entry(to: &str) -> Value {
    json!({"Destination": {"ToAddresses": [to]}})
}

#[tokio::test]
async fn a_bulk_entry_that_addresses_nobody_is_reported_in_its_own_result() {
    // "A description of an error that prevented a message being sent": a defect in ONE
    // entry's destination is that entry's own, so it is reported as its `INVALID_PARAMETER`
    // result inside a 200 response — and a call whose every entry is such an entry renders no
    // message at all, so the TEMPLATE a message would be rendered from is never looked up,
    // which is why a row naming a template the account holds nothing under is answered the
    // same way.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a destination with no member, from a verified sender",
            json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
        ),
        (
            "the same through a template name the account holds nothing under",
            json!({"TemplateName": "no-such-template", "TemplateData": DATA}),
        ),
    ];

    for (case, template) in rows {
        let ses = verified().await;
        let created = ses.create_template().await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the template must be saved first, got {} {:?}",
            created.status, created.body
        );
        let reply = ses
            .send_bulk(bulk(
                VERIFIED_SENDER,
                template,
                vec![json!({"Destination": {}})],
            ))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: an entry's own defect is reported in the results, not as the \
             call's error, got {} {:?}",
            reply.status, reply.body
        );
        let results = reply.results();
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: expected one result per entry, got {results:?}"
        );
        assert_eq!(
            (results[0].0.as_str(), results[0].2.as_str(), results[0].1.as_str()),
            ("INVALID_PARAMETER", "Destination cannot be empty", ""),
            "case {case:?}: expected the entry to carry what stopped it and no MessageId, \
             got {:?}",
            results[0]
        );
    }
}

#[tokio::test]
async fn what_a_bulk_entry_states_for_its_own_message_is_read_before_the_identity_gate() {
    // What an ENTRY states for its own message — the headers and the tags of that message —
    // is part of the request's own FORM, so it is read before anything the account holds,
    // like every other reading of the request's form: a call stating a header the service
    // writes itself is told so even though the identity it would have sent from is not one
    // this account has verified. A call whose entries state nothing of the kind reaches the
    // gates on sending and is refused WHOLE for the identity instead, rather than reported
    // entry by entry. The rows span both: an entry carrying a header the service writes
    // itself, one carrying a tag name no tag may take, a well-formed entry, a well-formed
    // entry beside one that addresses nobody, and a template name the account holds nothing
    // under — so no row is answered by a lookup instead. Every recipient here is the mailbox
    // simulator's, which belongs to no account and needs no identity, so the one identity a
    // refusal names is the sender the call states. A call whose EVERY entry addresses nobody
    // has no message to compose at all and is settled on its entries instead
    // (`tests/all/bulk_send_settled_on_its_entries.rs`).
    let unverified_sender = format!(
        "Email address is not verified. The following identities failed the check in \
         region {}: {UNVERIFIED}",
        HOME.to_uppercase()
    );
    let rows: Vec<(&str, Value, Vec<Value>, &str, String)> = vec![
        (
            "an entry stating a header the service writes itself",
            json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
            vec![json!({
                "Destination": {"ToAddresses": [SIMULATOR]},
                "ReplacementHeaders": [{"Name": "From", "Value": "someone@example.net"}],
            })],
            "BadRequestException",
            "Header <From> is not supported".to_string(),
        ),
        (
            "an entry stating a tag name no tag may take",
            json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
            vec![json!({
                "Destination": {"ToAddresses": [SIMULATOR]},
                "ReplacementTags": [{"Name": "not a tag", "Value": "v"}],
            })],
            "BadRequestException",
            "Invalid tag name <not a tag>: only alphanumeric ASCII characters, '_',  '-' \
             are allowed."
                .to_string(),
        ),
        (
            "a well-formed entry",
            json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
            vec![json!({"Destination": {"ToAddresses": [SIMULATOR]}})],
            "MessageRejected",
            unverified_sender.clone(),
        ),
        (
            "a well-formed entry beside one that addresses nobody",
            json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
            vec![
                json!({"Destination": {}}),
                json!({"Destination": {"ToAddresses": [SIMULATOR]}}),
            ],
            "MessageRejected",
            unverified_sender.clone(),
        ),
        (
            "a well-formed entry through a template the account holds nothing under",
            json!({"TemplateName": "no-such-template", "TemplateData": DATA}),
            vec![json!({"Destination": {"ToAddresses": [SIMULATOR]}})],
            "MessageRejected",
            unverified_sender.clone(),
        ),
    ];

    for (case, template, entries, code, message) in rows {
        let ses = verified().await;
        let created = ses.create_template().await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the template must be saved first, got {} {:?}",
            created.status, created.body
        );
        let reply = ses.send_bulk(bulk(UNVERIFIED, template, entries)).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, code),
            "case {case:?}: expected the call to be refused {code}, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_bad_entry_beside_a_good_one_stops_only_its_own_message() {
    let ses = verified().await;
    // About an entry's OWN defect, so the account is given production access and no entry
    // is also weighed on which mailboxes it addresses
    // (`send_sandbox_destination_gate.rs`).
    crate::send_sandbox_destination_gate::given_production_access(&ses.engine);
    let created = ses.create_template().await;
    assert_eq!(
        created.status, 200,
        "the template must be saved first, got {} {:?}",
        created.status, created.body
    );
    let reply = ses
        .send_bulk(bulk(
            VERIFIED_SENDER,
            json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
            vec![
                bulk_entry("first@example.net"),
                json!({"Destination": {"ToAddresses": []}}),
                bulk_entry("last@example.net"),
            ],
        ))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected the partial success to be a 200, got {} {:?}",
        reply.status, reply.body
    );
    let results = reply.results();
    assert_eq!(
        results.len(),
        3,
        "expected one result per entry, got {results:?}"
    );
    assert_eq!(
        results[0].0, "SUCCESS",
        "expected the first entry to be sent, got {:?}",
        results[0]
    );
    assert_eq!(
        (results[1].0.as_str(), results[1].2.as_str()),
        ("INVALID_PARAMETER", "Destination cannot be empty"),
        "expected the middle entry to report its own defect, got {:?}",
        results[1]
    );
    assert_eq!(
        results[2].0, "SUCCESS",
        "expected the last entry to be sent, got {:?}",
        results[2]
    );
    assert!(
        !results[0].1.is_empty() && !results[2].1.is_empty() && results[1].1.is_empty(),
        "expected a MessageId for each sent entry and none for the refused one, got \
         {results:?}"
    );
}

#[tokio::test]
async fn a_bulk_send_addresses_between_one_and_fifty_destinations() {
    let ceiling = "You must specify at least 1 destination, and no more than 50 destinations";
    for (case, count) in [("no destination at all", 0usize), ("fifty-one", 51)] {
        let ses = verified().await;
        // About how many destinations one call may carry, so the account is given
        // production access (`send_sandbox_destination_gate.rs`).
        crate::send_sandbox_destination_gate::given_production_access(&ses.engine);
        let created = ses.create_template().await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the template must be saved first, got {} {:?}",
            created.status, created.body
        );
        let entries: Vec<Value> = (0..count)
            .map(|index| bulk_entry(&format!("reader-{index}@example.net")))
            .collect();
        let reply = ses
            .send_bulk(bulk(
                VERIFIED_SENDER,
                json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
                entries,
            ))
            .await;
        refused(case, &reply, ceiling);
    }

    // The counter-cases: the two ends of the range the sentence states.
    for (case, count) in [("one destination", 1usize), ("fifty", 50)] {
        let ses = verified().await;
        // About how many destinations one call may carry, so the account is given
        // production access here too (`send_sandbox_destination_gate.rs`): a sandbox
        // account weighs every mailbox the call addresses, which is a different rule.
        crate::send_sandbox_destination_gate::given_production_access(&ses.engine);
        let created = ses.create_template().await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the template must be saved first, got {} {:?}",
            created.status, created.body
        );
        let entries: Vec<Value> = (0..count)
            .map(|index| bulk_entry(&format!("reader-{index}@example.net")))
            .collect();
        let reply = ses
            .send_bulk(bulk(
                VERIFIED_SENDER,
                json!({"TemplateName": TEMPLATE, "TemplateData": DATA}),
                entries,
            ))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected {count} destinations to be accepted, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.results().len(),
            count,
            "case {case:?}: expected one result per entry, got {:?}",
            reply.results()
        );
    }
}

#[tokio::test]
async fn a_bulk_send_states_its_content_and_its_sender_before_anything_is_resolved() {
    // Each row is a request that has not said what to send or who sends it, answered before
    // any template of the account is looked up — which is why a row naming a template the
    // account holds nothing under is still refused for the member it left out.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "default content carrying no template at all",
            json!({
                "FromEmailAddress": VERIFIED_SENDER,
                "DefaultContent": {},
                "BulkEmailEntries": [bulk_entry("reader@example.net")],
            }),
            "Template content must be specified",
        ),
        (
            "a template stating neither a name nor an ARN nor content",
            bulk(
                VERIFIED_SENDER,
                json!({"TemplateData": DATA}),
                vec![bulk_entry("reader@example.net")],
            ),
            "TemplateName and TemplateArn both can not be empty",
        ),
        (
            "no sending address at all",
            json!({
                "DefaultContent": {"Template": {"TemplateName": "no-such-template",
                                                "TemplateData": DATA}},
                "BulkEmailEntries": [bulk_entry("reader@example.net")],
            }),
            "Email address not specified.",
        ),
        (
            "a sending address that names no mailbox",
            bulk(
                "no-domain-sender",
                json!({"TemplateName": "no-such-template", "TemplateData": DATA}),
                vec![bulk_entry("reader@example.net")],
            ),
            "Invalid email address<no-domain-sender>.",
        ),
        (
            "a header the service writes itself, stated by one entry",
            bulk(
                VERIFIED_SENDER,
                json!({"TemplateName": "no-such-template", "TemplateData": DATA}),
                vec![json!({
                    "Destination": {"ToAddresses": ["reader@example.net"]},
                    "ReplacementHeaders": [{"Name": "From", "Value": "someone@example.net"}],
                })],
            ),
            "Header <From> is not supported",
        ),
    ];

    for (case, body, message) in rows {
        let ses = verified().await;
        let reply = ses.send_bulk(body).await;
        refused(case, &reply, message);
    }
}
