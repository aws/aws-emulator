// SPDX-License-Identifier: CC0-1.0

//! What the ACCOUNT does with a message THIS surface accepted for one of its own recipients:
//! Amazon SES email receiving, end to end, with the classic surface — whose store owns the
//! account's receipt rule sets — and the object store a rule's `S3Action` writes into mounted
//! beside this service on ONE engine, exactly as the assembled router mounts them.
//!
//! The rule these rows pin: a message Amazon SES accepts for a recipient the account holds a
//! verified identity for is also RECEIVED by that account, whichever API accepted it, so the
//! active receipt rule set's matching rule saves the message's RFC 5322 bytes as one object at
//! `<ObjectKeyPrefix><MessageId>` of the bucket it names. The message is the one Amazon SES
//! assembles from the parts a simple send carries ("you specify the sender, the recipient, and
//! the message body, and Amazon SES assembles the message for you") or the document a raw send
//! submitted, and it carries the `DKIM-Signature` Amazon SES signs a message from an Easy DKIM
//! identity with — `d=` the identity's domain and `s=` one of the tokens its owner publishes —
//! whose check is what a scanning rule reports as the message's `dkim=` result.
//!
//! The rows span what the rule generalises over: the form the message was sent in (a simple
//! message and a raw MIME document), the action's `ObjectKeyPrefix` (a directory-like prefix,
//! another one, and none at all) and whether the rule scans.
//!
//! Every identity here is created through this surface's own `CreateEmailIdentity`, every rule
//! through the classic surface's own `CreateReceiptRuleSet`/`CreateReceiptRule`/
//! `SetActiveReceiptRuleSet`, and every bucket through Amazon S3's own `CreateBucket` — so what
//! is asserted is what those services really hold. A message is received AFTER it is accepted,
//! so each row waits for the CONDITION — the object being there — rather than for a span of
//! time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The domain identity the account holds, which is therefore the domain it sends from and the
/// domain it receives mail for.
const DOMAIN: &str = "inbound.example.com";

/// The mailbox every send below is from.
const SENDER: &str = "sender@inbound.example.com";

/// The mailbox every send below is addressed to.
const RECIPIENT: &str = "rcpt@inbound.example.com";

/// The bucket every rule below saves into.
const BUCKET: &str = "inbound-mail";

/// How long a row waits for the object before giving up on it.
///
/// Generously longer than an in-process write takes: what a row asserts is the object, never how
/// quickly it arrived, so a loaded host makes this wait more patient and never less.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How long a row waits between two listings while waiting for the object.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The three services one engine carries here: this one, the classic surface that owns the
/// account's receipt rule sets, and the object store a rule writes into.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These rows exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One call of this surface's own REST API: `(status, body)`.
    async fn call(&self, case: &str, method: &str, path: &str, body: Option<Value>) -> Value {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
            .header("host", format!("email.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization())
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let answered: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "case {case:?}: {method} {path}: expected 200, got {status} with {answered}"
        );
        answered
    }

    /// One call of the CLASSIC surface's query API, which is where the account's receipt rule
    /// sets live.
    ///
    /// Each field is percent-encoded by the crate this crate's suites declare for it
    /// ([`crate::json_wire::query_component`]), so no suite writes the escaping itself.
    async fn classic(&self, case: &str, action: &str, params: &[(&str, &str)]) {
        let mut body = format!("Action={action}&Version=2010-12-01");
        for (name, value) in params {
            body.push('&');
            body.push_str(&crate::json_wire::query_component(name));
            body.push('=');
            body.push_str(&crate::json_wire::query_component(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://email.{REGION}.amazonaws.com/"))
            .header("host", format!("email.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization())
            .body(bytes::Bytes::from(body))
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).into_owned();
        assert_eq!(
            status, 200,
            "case {case:?}: {action}: expected 200, got {status} with {answered}"
        );
    }

    /// An Amazon S3 client over this emulator's own in-process callback — the ONE construction
    /// of that hop this tree uses.
    fn s3(&self) -> aws_sdk_s3::Client {
        let callback = smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone());
        service_common::s3_client::s3_client(&callback, REGION)
    }

    /// The account with [`DOMAIN`] as a verified domain identity, with the Easy DKIM setup a
    /// domain identity is created with — waited for until the verification the clock owes it has
    /// been made, which is what puts that setup in place — and the Easy DKIM tokens its owner
    /// published, read back through this surface's own `GetEmailIdentity`: the selectors a
    /// signature for the domain may name.
    ///
    /// The wait is for the CONDITION — the identity reporting itself verified — looked for again
    /// every [`POLL_INTERVAL`] until [`DELIVERY_DEADLINE`] passes, never for a span of time.
    async fn given_verified_domain(&self, case: &str) -> Vec<String> {
        self.call(
            case,
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": DOMAIN})),
        )
        .await;
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let identity = self
                .call(case, "GET", &format!("/v2/email/identities/{DOMAIN}"), None)
                .await;
            let verified = identity.get("VerifiedForSendingStatus") == Some(&Value::Bool(true));
            let dkim = identity
                .pointer("/DkimAttributes/Status")
                .and_then(Value::as_str)
                == Some("SUCCESS");
            if verified && dkim {
                return identity
                    .pointer("/DkimAttributes/Tokens")
                    .and_then(Value::as_array)
                    .map(|tokens| {
                        tokens
                            .iter()
                            .filter_map(|token| token.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: {DOMAIN} did not report itself verified with its Easy DKIM setup \
                 in place within {deadline_secs}s, last read {identity}",
                deadline_secs = DELIVERY_DEADLINE.as_secs()
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// A bucket Amazon SES may write a received message into, carrying the inbound-mail bucket
    /// policy the Amazon SES Developer Guide writes.
    async fn given_bucket(&self, case: &str) {
        self.s3()
            .create_bucket()
            .bucket(BUCKET)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: creating {BUCKET}: {error}"));
        let policy = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"AllowSESPuts","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{BUCKET}/*","Condition":{{"StringEquals":{{"aws:Referer":"{account}"}}}}}}]}}"#,
            account = engine::EMULATOR_ACCOUNT_ID
        );
        self.s3()
            .put_bucket_policy()
            .bucket(BUCKET)
            .policy(policy)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: putting {BUCKET}'s policy: {error}"));
    }

    /// The account's active receipt rule set: one rule applying to [`RECIPIENT`], saving into
    /// [`BUCKET`] under `prefix`, scanning or not as `scan_enabled` says.
    async fn given_active_rule(&self, case: &str, rule: &str, prefix: &str, scan_enabled: bool) {
        let rule_set = "received";
        self.classic(case, "CreateReceiptRuleSet", &[("RuleSetName", rule_set)])
            .await;
        self.classic(
            case,
            "CreateReceiptRule",
            &[
                ("RuleSetName", rule_set),
                ("Rule.Name", rule),
                ("Rule.Enabled", "true"),
                (
                    "Rule.ScanEnabled",
                    if scan_enabled { "true" } else { "false" },
                ),
                ("Rule.Recipients.member.1", RECIPIENT),
                ("Rule.Actions.member.1.S3Action.BucketName", BUCKET),
                ("Rule.Actions.member.1.S3Action.ObjectKeyPrefix", prefix),
            ],
        )
        .await;
        self.classic(case, "SetActiveReceiptRuleSet", &[("RuleSetName", rule_set)])
            .await;
    }

    /// `SendEmail`, answering the `MessageId` the message was accepted under.
    async fn send(&self, case: &str, content: Value) -> String {
        let answered = self
            .call(
                case,
                "POST",
                "/v2/email/outbound-emails",
                Some(json!({
                    "FromEmailAddress": SENDER,
                    "Destination": {"ToAddresses": [RECIPIENT]},
                    "Content": content,
                })),
            )
            .await;
        answered
            .get("MessageId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a MessageId, got {answered}"))
            .to_string()
    }

    /// The keys of [`BUCKET`] under `prefix`.
    async fn keys(&self, case: &str, prefix: &str) -> Vec<String> {
        let listed = self
            .s3()
            .list_objects_v2()
            .bucket(BUCKET)
            .prefix(prefix)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: listing {BUCKET}/{prefix}: {error}"));
        listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect()
    }

    /// The bytes Amazon S3 holds at `key` of [`BUCKET`], waited for until the object is there.
    ///
    /// The wait is for the CONDITION — the object the matching rule saves — looked for again
    /// every [`POLL_INTERVAL`] until [`DELIVERY_DEADLINE`] passes, because a message is received
    /// after it is accepted and nothing a sender is told says when.
    async fn settled_object(&self, case: &str, prefix: &str, key: &str) -> Vec<u8> {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let found = self.keys(case, prefix).await;
            if found.iter().any(|listed| listed == key) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: no object appeared at {BUCKET}/{key} within {deadline_secs}s, \
                 found {found:?}",
                deadline_secs = DELIVERY_DEADLINE.as_secs()
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let read = self
            .s3()
            .get_object()
            .bucket(BUCKET)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: reading {BUCKET}/{key}: {error}"));
        read.body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: reading {key}'s body: {error}"))
            .to_vec()
    }
}

/// The credential scope every call here carries, whose signing name is what the router resolves
/// a request's service by.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{REGION}/ses/aws4_request, \
         SignedHeaders=host, Signature=00"
    )
}

/// The value of the header `name` of the stored message, and `None` when it carries none.
fn header(stored: &[u8], case: &str, name: &str) -> Option<String> {
    let parsed = mailparse::parse_mail(stored).unwrap_or_else(|error| {
        panic!("case {case:?}: the stored object is a MIME message: {error}")
    });
    mailparse::MailHeaderMap::get_first_value(parsed.headers.as_slice(), name)
}

/// A BULK send accepts one message per destination, and each one the account receives is saved
/// by the matching rule just as a single send's is — the same rule, reached from the other send
/// of this surface.
///
/// The entries differ along what the rule has to decide: a destination the account receives mail
/// for, and one on a domain it holds no identity for, which the account does not receive at all.
#[tokio::test]
async fn a_bulk_send_has_every_message_the_account_receives_saved_too() {
    let case = "a bulk send with one received and one foreign destination";
    let prefix = "bulk/";
    let stranger = "somebody@elsewhere.example.org";
    let emulator = Emulator::new();
    emulator.given_verified_domain(case).await;
    emulator.given_bucket(case).await;
    emulator
        .given_active_rule(case, "to-bulk", prefix, true)
        .await;

    let subject = format!("the subject of {case}");
    let body = format!("the body of {case}");
    emulator
        .call(
            case,
            "POST",
            "/v2/email/templates",
            Some(json!({
                "TemplateName": "bulk-template",
                "TemplateContent": {"Subject": subject, "Text": body},
            })),
        )
        .await;
    let answered = emulator
        .call(
            case,
            "POST",
            "/v2/email/outbound-bulk-emails",
            Some(json!({
                "FromEmailAddress": SENDER,
                "DefaultContent": {"Template": {
                    "TemplateName": "bulk-template",
                    "TemplateData": "{}",
                }},
                "BulkEmailEntries": [
                    {"Destination": {"ToAddresses": [RECIPIENT]}},
                    {"Destination": {"ToAddresses": [stranger]}},
                ],
            })),
        )
        .await;
    let results = answered
        .get("BulkEmailEntryResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let accepted: Vec<String> = results
        .iter()
        .map(|result| {
            result
                .get("MessageId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: every entry is accepted, got {answered}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        accepted.len(),
        2,
        "case {case:?}: expected one accepted message per destination, got {answered}"
    );

    // The destination the account receives mail for: its message is saved, carrying the rendered
    // template and the results the scanning rule stamped on it.
    let stored = emulator
        .settled_object(case, prefix, &format!("{prefix}{}", accepted[0]))
        .await;
    let read = |name: &str| header(&stored, case, name);
    assert_eq!(
        read("Subject").as_deref(),
        Some(subject.as_str()),
        "case {case:?}: expected the rendered subject, got {:?}",
        read("Subject")
    );
    assert!(
        read("Authentication-Results")
            .unwrap_or_default()
            .contains("dkim=pass"),
        "case {case:?}: expected the signature of the verified domain to pass, got {:?}",
        read("Authentication-Results")
    );
    // The foreign destination: the account receives no mail for it, so the rule applies to
    // nothing and the only object under the prefix is the one above. Asserted once the object
    // that proves the first message's steps are finished has appeared.
    assert_eq!(
        emulator.keys(case, prefix).await,
        vec![format!("{prefix}{}", accepted[0])],
        "case {case:?}: expected only the message the account receives to be saved"
    );
}

struct Row {
    /// What this row is called in every assertion it makes.
    case: &'static str,
    /// The rule's name, which is also what a scanning rule's results are attributed to.
    rule: &'static str,
    /// `S3Action$ObjectKeyPrefix`.
    prefix: &'static str,
    /// `ReceiptRule$ScanEnabled`.
    scan_enabled: bool,
    /// Whether the message is sent as a raw MIME document rather than as parts.
    raw: bool,
}

/// A message this surface accepts for a recipient the account holds a verified identity for is
/// received into the matching rule's bucket, signed for the sending identity.
#[tokio::test]
async fn a_message_this_surface_accepts_is_received_into_the_rules_bucket() {
    let rows = [
        Row {
            case: "a simple message under a directory-like prefix",
            rule: "to-inbox",
            prefix: "inbox/",
            scan_enabled: false,
            raw: false,
        },
        Row {
            case: "a simple message under another prefix, scanned",
            rule: "to-scan",
            prefix: "scan/2026/",
            scan_enabled: true,
            raw: false,
        },
        Row {
            case: "a simple message under no prefix at all",
            rule: "to-the-bucket-root",
            prefix: "",
            scan_enabled: false,
            raw: false,
        },
        Row {
            case: "a raw MIME document",
            rule: "to-raw",
            prefix: "raw/",
            scan_enabled: false,
            raw: true,
        },
    ];
    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let tokens = emulator.given_verified_domain(case).await;
        emulator.given_bucket(case).await;
        emulator
            .given_active_rule(case, row.rule, row.prefix, row.scan_enabled)
            .await;

        let subject = format!("the subject of {case}");
        let body = format!("the body of {case}");
        let content = if row.raw {
            let document = format!(
                "From: {SENDER}\r\nTo: {RECIPIENT}\r\nSubject: {subject}\r\n\
                 MIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{body}\r\n"
            );
            json!({"Raw": {"Data": aws_smithy_types::base64::encode(document.as_bytes())}})
        } else {
            json!({
                "Simple": {
                    "Subject": {"Data": subject},
                    "Body": {"Text": {"Data": body}},
                }
            })
        };
        let message_id = emulator.send(case, content).await;
        let key = format!("{}{message_id}", row.prefix);
        let stored = emulator.settled_object(case, row.prefix, &key).await;
        let read = |name: &str| header(&stored, case, name);

        assert_eq!(
            emulator.keys(case, row.prefix).await,
            vec![key.clone()],
            "case {case:?}: expected the matching rule to save exactly one object, keyed by the \
             id the send was accepted under"
        );
        assert_eq!(
            read("Subject").as_deref(),
            Some(subject.as_str()),
            "case {case:?}: expected the stored message to carry the subject that was sent, got \
             {:?}",
            read("Subject")
        );
        assert_eq!(
            read("To").as_deref(),
            Some(RECIPIENT),
            "case {case:?}: expected the stored message to carry the recipient it was addressed \
             to, got {:?}",
            read("To")
        );
        assert_eq!(
            read("From").as_deref(),
            Some(SENDER),
            "case {case:?}: expected the stored message to carry its sender, got {:?}",
            read("From")
        );
        let text = String::from_utf8_lossy(&stored).into_owned();
        assert!(
            text.contains(&body),
            "case {case:?}: expected the stored message to carry the body that was sent, got \
             {text:?}"
        );

        // The signature Amazon SES puts on a message from an Easy DKIM identity: the domain it
        // claims is the identity's, and the selector it names is one of the tokens that
        // identity's owner published.
        let signature = read("DKIM-Signature").unwrap_or_else(|| {
            panic!("case {case:?}: expected the stored message to carry a DKIM-Signature")
        });
        assert!(
            signature.contains(&format!("d={DOMAIN}")),
            "case {case:?}: expected the signature to claim the verified domain, got \
             {signature:?}"
        );
        assert!(
            tokens
                .iter()
                .any(|token| signature.contains(&format!("s={token}"))),
            "case {case:?}: expected the signature's selector to be one of the published tokens \
             {tokens:?}, got {signature:?}"
        );

        let results = read("Authentication-Results");
        if !row.scan_enabled {
            assert_eq!(
                results, None,
                "case {case:?}: expected a rule that does not scan to stamp no authentication \
                 results, got {results:?}"
            );
            continue;
        }
        let results = results.unwrap_or_else(|| {
            panic!("case {case:?}: expected a scanning rule to stamp an Authentication-Results")
        });
        // The signature CHECKS OUT against the key the signing identity holds, which is what
        // `dkim=pass` reports.
        assert!(
            results.contains(&format!("dkim=pass header.i=@{DOMAIN}")),
            "case {case:?}: expected the signature of the verified domain to pass, got \
             {results:?}"
        );
        for verdict in ["X-SES-Spam-Verdict", "X-SES-Virus-Verdict"] {
            assert_eq!(
                read(verdict).as_deref(),
                Some("PASS"),
                "case {case:?}: expected {verdict} to report what the scan found, got {:?}",
                read(verdict)
            );
        }
    }
}
