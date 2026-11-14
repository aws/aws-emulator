// SPDX-License-Identifier: CC0-1.0

//! What the account's receive path does with a message the CLASSIC `SendEmail` accepted — the
//! message Amazon SES assembles itself — and what a receipt rule whose `ScanEnabled` is set has
//! stamped on it, with Amazon S3 mounted beside this service on ONE engine exactly as the
//! assembled router mounts it.
//!
//! The rules these suites pin:
//!
//! * a message Amazon SES accepts for a recipient this account holds a verified identity for is
//!   RECEIVED by the account whichever send API accepted it, so a send that hands Amazon SES the
//!   message's PARTS — "you specify the sender, the recipient, and the message body, and Amazon
//!   SES assembles the message for you" — has the assembled RFC 5322 document saved by the
//!   matching rule's `S3Action`, carrying the sender, the recipients and the subject the request
//!   named and the body parts it sent;
//! * "if true, then messages that this receipt rule applies to are scanned for spam and viruses"
//!   (`ReceiptRule$ScanEnabled`), so a rule with the scan on has the results of the sender
//!   authentication and of each scan stamped on the message its actions then save, and a rule
//!   with the scan off has the message saved exactly as it arrived.
//!
//! Each table spans what its rule generalises over: the body parts a message is assembled from
//! (text, HTML, both), and — for the scan — whether the rule scans and what the message's own
//! content is, which is what each verdict is read off.
//!
//! Every bucket and bucket policy here is made through Amazon S3's OWN operations over the
//! in-process callback, and every object asserted on is read back through Amazon S3's own
//! `ListObjectsV2` and `GetObject`, so what is asserted is what that service really holds. A
//! message is received AFTER it is accepted, so every assertion about an object waits for the
//! CONDITION — the object being there — rather than for a span of time.

use crate::harness::{call, element, field, settle_verifications, Field, REGION};
use service_com_amazonaws_ses_simpleemailservice::state::received_scan;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The domain identity this account holds verified, and therefore the domain it receives mail
/// for.
const DOMAIN: &str = "scan.example.com";

/// A mailbox of that domain the messages below are sent from.
const SENDER: &str = "sender@scan.example.com";

/// A mailbox of that domain the messages below are addressed to.
const RECIPIENT: &str = "rcpt@scan.example.com";

/// Another mailbox of it, so a message can address more than one recipient.
const SECOND_RECIPIENT: &str = "copied@scan.example.com";

/// The bucket every rule here saves into.
const BUCKET: &str = "received-mail";

/// The `ObjectKeyPrefix` every rule here saves under.
const PREFIX: &str = "inbox/";

/// How long a row waits for an object before giving up on it.
///
/// Generously longer than an in-process write takes: what a row asserts is the object, never how
/// quickly it arrived, so a loaded host makes this wait more patient and never less.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How long a row waits between two listings while waiting for an object.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A router carrying this service beside the object store a receipt rule's `S3Action` writes
/// into, both over ONE shared engine, plus that engine.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; the awsQuery calls
    // below sign nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the ONE construction of
/// that hop this tree uses.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    service_common::s3_client::s3_client(&callback, REGION)
}

/// A bucket Amazon SES may write a received message into: created through Amazon S3's own
/// `CreateBucket`, carrying the inbound-mail bucket policy the Amazon SES Developer Guide
/// writes — the service principal granted `s3:PutObject`, scoped to this account.
async fn given_bucket(router: &MultiServiceRouter, case: &str) {
    s3(router)
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: creating {BUCKET}: {error}"));
    let policy = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"AllowSESPuts","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{BUCKET}/*","Condition":{{"StringEquals":{{"aws:Referer":"{account}"}}}}}}]}}"#,
        account = engine::EMULATOR_ACCOUNT_ID
    );
    s3(router)
        .put_bucket_policy()
        .bucket(BUCKET)
        .policy(policy)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: putting {BUCKET}'s policy: {error}"));
}

/// `VerifyDomainIdentity` for the domain this account receives mail for, and then the store
/// settled past the span that request leaves the record pending for — what these rows are about
/// is the receive path rather than the verification span.
async fn given_verified_domain(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", DOMAIN)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: verifying {DOMAIN}: expected 200, got {status} with {body}"
    );
    settle_verifications(engine);
}

/// The one rule set this account evaluates: one rule matching every recipient of the account,
/// saving into [`BUCKET`] under [`PREFIX`], scanning or not as `scan_enabled` says.
async fn given_active_rule(router: &MultiServiceRouter, case: &str, scan_enabled: bool) {
    let rule_set = "received";
    let (status, body) = call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", rule_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the rule set: expected 200, got {status} with {body}"
    );
    let params: Vec<Field> = vec![
        field("RuleSetName", rule_set),
        field("Rule.Name", "save-every-recipient"),
        field("Rule.Enabled", "true"),
        field("Rule.ScanEnabled", if scan_enabled { "true" } else { "false" }),
        field("Rule.Actions.member.1.S3Action.BucketName", BUCKET),
        field("Rule.Actions.member.1.S3Action.ObjectKeyPrefix", PREFIX),
    ];
    let (status, body) = call(router, "CreateReceiptRule", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the rule: expected 200, got {status} with {body}"
    );
    let (status, body) = call(
        router,
        "SetActiveReceiptRuleSet",
        &[field("RuleSetName", rule_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: activating the rule set: expected 200, got {status} with {body}"
    );
}

/// One `SendEmail` — the send that hands Amazon SES the message's parts — answering the
/// `MessageId` it was accepted under.
async fn send_email(
    router: &MultiServiceRouter,
    case: &str,
    recipients: &[&str],
    copied: &[&str],
    subject: &str,
    text: Option<&str>,
    html: Option<&str>,
) -> String {
    let mut params = vec![
        field("Source", SENDER),
        field("Message.Subject.Data", subject),
    ];
    for (index, recipient) in recipients.iter().enumerate() {
        params.push(field(
            format!("Destination.ToAddresses.member.{}", index + 1),
            *recipient,
        ));
    }
    for (index, recipient) in copied.iter().enumerate() {
        params.push(field(
            format!("Destination.CcAddresses.member.{}", index + 1),
            *recipient,
        ));
    }
    if let Some(text) = text {
        params.push(field("Message.Body.Text.Data", text));
    }
    if let Some(html) = html {
        params.push(field("Message.Body.Html.Data", html));
    }
    let (status, body) = call(router, "SendEmail", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: sending: expected 200, got {status} with {body}"
    );
    element(&body, "MessageId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a MessageId, got {body}"))
}

/// The keys of [`BUCKET`] under [`PREFIX`], in the order Amazon S3 lists them.
async fn keys(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let listed = s3(router)
        .list_objects_v2()
        .bucket(BUCKET)
        .prefix(PREFIX)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: listing {BUCKET}/{PREFIX}: {error}"));
    listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// The bytes Amazon S3 holds for the message `message_id` identifies, waited for until the
/// object is there.
///
/// The wait is for the CONDITION — the object under the action's prefix — looked for again every
/// [`POLL_INTERVAL`] until [`DELIVERY_DEADLINE`] passes, because a message is received after it
/// is accepted and nothing a sender is told says when.
async fn settled_message(router: &MultiServiceRouter, case: &str, message_id: &str) -> Vec<u8> {
    let key = format!("{PREFIX}{message_id}");
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let found = keys(router, case).await;
        if found.contains(&key) {
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
    let read = s3(router)
        .get_object()
        .bucket(BUCKET)
        .key(&key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {BUCKET}/{key}: {error}"));
    read.body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {key}'s body: {error}"))
        .to_vec()
}

/// The value of the header `name` of the stored message, and `None` when it carries none.
fn header(stored: &[u8], case: &str, name: &str) -> Option<String> {
    let parsed = mailparse::parse_mail(stored).unwrap_or_else(|error| {
        panic!("case {case:?}: the stored object is a MIME message: {error}")
    });
    mailparse::MailHeaderMap::get_first_value(parsed.headers.as_slice(), name)
}

/// One row of the assembly table: the body parts the request sends, and the media type the
/// assembled message declares for them.
#[derive(Debug, Clone, Copy)]
struct Assembled {
    /// What this row is called in every assertion it makes.
    case: &'static str,
    /// `Message.Body.Text.Data`.
    text: Option<&'static str>,
    /// `Message.Body.Html.Data`.
    html: Option<&'static str>,
    /// The media type the assembled message's own `Content-Type` declares.
    media_type: &'static str,
}

/// A message the classic `SendEmail` accepted is assembled by Amazon SES and received into the
/// matching rule's bucket, carrying the sender, the recipients, the subject and the body parts
/// the request named.
///
/// The rows differ along the body parts a message is assembled from, which is what decides the
/// document's own structure.
#[tokio::test]
async fn a_message_amazon_ses_assembles_is_received_into_the_matching_rules_bucket() {
    let rows = [
        Assembled {
            case: "a message with a text part",
            text: Some("the plain text of this message"),
            html: None,
            media_type: "text/plain",
        },
        Assembled {
            case: "a message with an HTML part",
            text: None,
            html: Some("<p>the markup of this message</p>"),
            media_type: "text/html",
        },
        Assembled {
            case: "a message with both parts",
            text: Some("the plain text of this message"),
            html: Some("<p>the markup of this message</p>"),
            media_type: "multipart/alternative",
        },
    ];
    for row in rows {
        let case = row.case;
        let (router, engine) = emulator();
        given_verified_domain(&router, &engine, case).await;
        given_bucket(&router, case).await;
        given_active_rule(&router, case, false).await;

        let subject = format!("the subject of {case}");
        let message_id = send_email(
            &router,
            case,
            &[RECIPIENT],
            &[SECOND_RECIPIENT],
            &subject,
            row.text,
            row.html,
        )
        .await;
        let stored = settled_message(&router, case, &message_id).await;
        let read = |name: &str| header(&stored, case, name);

        assert_eq!(
            read("Subject").as_deref(),
            Some(subject.as_str()),
            "case {case:?}: expected the subject the request named, got {:?}",
            read("Subject")
        );
        assert_eq!(
            read("From").as_deref(),
            Some(SENDER),
            "case {case:?}: expected the sender the request named, got {:?}",
            read("From")
        );
        assert_eq!(
            read("To").as_deref(),
            Some(RECIPIENT),
            "case {case:?}: expected the recipient the request named, got {:?}",
            read("To")
        );
        assert_eq!(
            read("Cc").as_deref(),
            Some(SECOND_RECIPIENT),
            "case {case:?}: expected the copied recipient the request named, got {:?}",
            read("Cc")
        );
        let message_id_header = read("Message-ID").unwrap_or_default();
        assert!(
            message_id_header.contains(&message_id),
            "case {case:?}: expected the Message-ID to carry the accepted id {message_id}, got \
             {message_id_header:?}"
        );
        assert!(
            read("Date").is_some(),
            "case {case:?}: expected the assembled message to carry a Date"
        );
        let media_type = mailparse::parse_mail(&stored)
            .unwrap_or_else(|error| panic!("case {case:?}: the object is a MIME message: {error}"))
            .ctype
            .mimetype;
        assert_eq!(
            media_type, row.media_type,
            "case {case:?}: expected the assembled message to declare {}, got {media_type}",
            row.media_type
        );
        let text = String::from_utf8_lossy(&stored).into_owned();
        for part in [row.text, row.html].into_iter().flatten() {
            assert!(
                text.contains(part),
                "case {case:?}: expected the stored message to carry the part the request sent \
                 ({part:?}), got {text:?}"
            );
        }
        // Nothing but the one object: a rule saves one object per matching message.
        let found = keys(&router, case).await;
        assert_eq!(
            found,
            vec![format!("{PREFIX}{message_id}")],
            "case {case:?}: expected exactly the one object the matching rule saves"
        );
    }
}

/// One row of the scan table: what the rule asks for, what the message carries, and the verdicts
/// that follow.
#[derive(Debug, Clone, Copy)]
struct Scanned {
    /// What this row is called in every assertion it makes.
    case: &'static str,
    /// `Rule.ScanEnabled`.
    scan_enabled: bool,
    /// The body of the message the row sends.
    body: &'static str,
    /// `X-SES-Spam-Verdict`, and `None` for a row whose rule does not scan.
    spam: Option<&'static str>,
    /// `X-SES-Virus-Verdict`, and `None` for a row whose rule does not scan.
    virus: Option<&'static str>,
}

/// A receipt rule whose `ScanEnabled` is set has the authentication and scan results stamped on
/// the message its action then saves, and one with the scan off has nothing stamped on it.
///
/// The rows differ along whether the rule scans and along what the message's own content is,
/// which is what each verdict is read off: a clean message, one carrying the published spam test
/// pattern, and one carrying the published virus test signature — each in the body a client
/// sends, which is where a scanner reads it.
#[tokio::test]
async fn a_scanning_rule_stamps_the_authentication_and_scan_results_on_the_message() {
    let rows = [
        Scanned {
            case: "a clean message, scanned",
            scan_enabled: true,
            body: "nothing either scan looks for",
            spam: Some(received_scan::VERDICT_PASS),
            virus: Some(received_scan::VERDICT_PASS),
        },
        Scanned {
            case: "a message carrying the published spam test pattern",
            scan_enabled: true,
            body: received_scan::SPAM_TEST_PATTERN,
            spam: Some(received_scan::VERDICT_FAIL),
            virus: Some(received_scan::VERDICT_PASS),
        },
        Scanned {
            case: "a message carrying the published virus test signature",
            scan_enabled: true,
            body: received_scan::VIRUS_TEST_SIGNATURE,
            spam: Some(received_scan::VERDICT_PASS),
            virus: Some(received_scan::VERDICT_FAIL),
        },
        Scanned {
            case: "a rule that does not scan",
            scan_enabled: false,
            body: "nothing either scan looks for",
            spam: None,
            virus: None,
        },
    ];
    for row in rows {
        let case = row.case;
        let (router, engine) = emulator();
        given_verified_domain(&router, &engine, case).await;
        given_bucket(&router, case).await;
        given_active_rule(&router, case, row.scan_enabled).await;

        let message_id = send_email(
            &router,
            case,
            &[RECIPIENT],
            &[],
            &format!("the subject of {case}"),
            Some(row.body),
            None,
        )
        .await;
        let stored = settled_message(&router, case, &message_id).await;
        let read = |name: &str| header(&stored, case, name);

        assert_eq!(
            read(received_scan::SPAM_VERDICT_HEADER).as_deref(),
            row.spam,
            "case {case:?}: expected the spam verdict {:?}, got {:?}",
            row.spam,
            read(received_scan::SPAM_VERDICT_HEADER)
        );
        assert_eq!(
            read(received_scan::VIRUS_VERDICT_HEADER).as_deref(),
            row.virus,
            "case {case:?}: expected the virus verdict {:?}, got {:?}",
            row.virus,
            read(received_scan::VIRUS_VERDICT_HEADER)
        );
        let spf = read(received_scan::RECEIVED_SPF_HEADER);
        let results = read(received_scan::AUTHENTICATION_RESULTS_HEADER);
        if !row.scan_enabled {
            assert_eq!(
                (spf.as_deref(), results.as_deref()),
                (None, None),
                "case {case:?}: expected a rule that does not scan to stamp no authentication \
                 results, got {spf:?} and {results:?}"
            );
            continue;
        }
        let spf = spf.unwrap_or_else(|| {
            panic!("case {case:?}: expected a scanning rule to stamp a Received-SPF")
        });
        assert!(
            spf.starts_with("pass "),
            "case {case:?}: expected the SPF result of a message this account itself sent to be \
             a pass, got {spf:?}"
        );
        assert!(
            spf.contains(&format!("envelope-from={message_id}@")),
            "case {case:?}: expected the SPF result to name the envelope sender the message was \
             relayed under, got {spf:?}"
        );
        let results = results.unwrap_or_else(|| {
            panic!("case {case:?}: expected a scanning rule to stamp an Authentication-Results")
        });
        assert!(
            results.starts_with(&format!("{}; ", received_scan::AUTHENTICATION_SERVICE_ID)),
            "case {case:?}: expected the results to be reported under Amazon SES's own \
             authentication-service identifier, got {results:?}"
        );
        assert!(
            results.contains("spf=pass"),
            "case {case:?}: expected the SPF result in the authentication results, got {results:?}"
        );
        // Nothing set Easy DKIM up for this identity, so the message carries no signature at all
        // and there is no DKIM result to report — which is what `dkim=none` says.
        assert!(
            results.contains("dkim=none"),
            "case {case:?}: expected an unsigned message's DKIM result, got {results:?}"
        );
        assert!(
            results.contains(&format!("header.from={DOMAIN}")),
            "case {case:?}: expected the DMARC result to name the domain the From field claims, \
             got {results:?}"
        );
    }
}
