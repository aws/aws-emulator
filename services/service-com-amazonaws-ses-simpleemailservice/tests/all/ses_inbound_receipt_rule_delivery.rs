// SPDX-License-Identifier: CC0-1.0

//! The RECEIVING half of Amazon SES, end to end: what the account's ACTIVE receipt rule set
//! does with a message `SendRawEmail` has just accepted for one of the account's own
//! recipients — with Amazon S3 and Amazon Web Services KMS mounted beside this service on ONE
//! engine exactly as the assembled router mounts them.
//!
//! The rule these suites pin: a message Amazon SES accepts for an envelope recipient this
//! account holds a verified identity for is also RECEIVED by the account, so the active rule
//! set's rules are evaluated in the order they are held and each ENABLED rule whose
//! `Recipients` conditions match carries out its actions — an `S3Action` saving the message's
//! RFC 5322 bytes UNALTERED as one object at `<ObjectKeyPrefix><MessageId>` of the bucket it
//! names, an `AddHeaderAction` adding a header the later actions act on, and a `StopAction`
//! terminating the evaluation of the rule set.
//!
//! Every bucket, bucket policy and KMS key here is made through Amazon S3's and KMS's OWN
//! operations over the in-process callback, and every object asserted on is read back through
//! Amazon S3's own `ListObjectsV2` and `GetObject`, so what is asserted is what that service
//! really holds.
//!
//! The table spans what the rule generalises over: the form of the recipient condition (an
//! address, a domain, a parent domain of the recipient's domain, none at all, one naming
//! somebody else), whether the rule is enabled, whether its set is the active one, whether the
//! recipient is one this account receives mail for at all, how many rules match, and whether a
//! `StopAction` ends the evaluation. The object's placement under the action's
//! `ObjectKeyPrefix`, the bytes it carries, the header an `AddHeaderAction` adds and the
//! envelope an `S3Action$KmsKeyArn` has the message saved in are asserted beside it.
//!
//! A message is received AFTER it is accepted, so every assertion about an object waits for
//! the CONDITION — the object the message's last step writes being there — rather than for a
//! span of time: [`settled_key`] looks again until the object appears or [`DELIVERY_DEADLINE`]
//! passes, and a row that expects NO object asserts the listing only once the object proving
//! that message's steps are finished has appeared.

use crate::harness::{call, element, field, settle_verifications, Field, REGION};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The domain identity this account holds verified, and therefore the domain it receives mail
/// for.
const DOMAIN: &str = "inbound.example.com";

/// A mailbox of that domain the messages below are sent from.
const SENDER: &str = "sender@inbound.example.com";

/// A mailbox of that domain the messages below are addressed to.
const RECIPIENT: &str = "rcpt@inbound.example.com";

/// A mailbox of a SUBDOMAIN of the verified domain, which a verified domain covers.
const SUBDOMAIN_RECIPIENT: &str = "user@mail.inbound.example.com";

/// A mailbox of a domain this account holds no identity for at all.
const STRANGER: &str = "stranger@elsewhere.example.org";

/// The mailbox the control rule of a row is matched by, so a row can tell "not delivered" from
/// "not delivered YET": the control rule is the LAST rule of the active set, its object is
/// written by the last step of that message's delivery, and the steps of one message are
/// carried out in order.
const CONTROL_RECIPIENT: &str = "control@inbound.example.com";

/// The key prefix the control rule saves under.
const CONTROL_PREFIX: &str = "control/";

/// How long a row waits for an object before giving up on it.
///
/// Generously longer than an in-process write takes: what a row asserts is the object, never
/// how quickly it arrived, so a loaded host makes this wait more patient and never less.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How long a row waits between two listings while waiting for an object.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A router carrying this service beside the two a receipt rule's `S3Action` addresses — the
/// object store the message is saved into and KMS, which owns the key it may be encrypted
/// under — all over ONE shared engine, plus that engine.
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
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// A KMS client over the same callback.
fn kms(router: &MultiServiceRouter) -> aws_sdk_kms::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_kms::Config::builder()
        .behavior_version(aws_sdk_kms::config::BehaviorVersion::latest())
        .region(aws_sdk_kms::config::Region::new(REGION))
        .endpoint_url(format!("https://kms.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_kms::Client::from_conf(config)
}

/// The account this emulator serves, which owns every bucket and key here.
fn account() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

/// A bucket Amazon SES may write a received message into: created through Amazon S3's own
/// `CreateBucket`, carrying the inbound-mail bucket policy the Amazon SES Developer Guide
/// writes — the service principal granted `s3:PutObject`, scoped to this account.
async fn given_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    s3(router)
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: creating {bucket}: {error}"));
    let policy = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"AllowSESPuts","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{bucket}/*","Condition":{{"StringEquals":{{"AWS:SourceAccount":"{}"}}}}}}]}}"#,
        account()
    );
    s3(router)
        .put_bucket_policy()
        .bucket(bucket)
        .policy(policy)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: putting {bucket}'s policy: {error}"));
}

/// `VerifyDomainIdentity` for the domain this account receives mail for, and then the store
/// settled past the span that request leaves the record pending for — what these rows are
/// about is the receive path rather than the verification span.
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

/// One action of a receipt rule, as the rows name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// `S3Action` saving the received message under this `ObjectKeyPrefix`.
    Save(&'static str),
    /// `AddHeaderAction` adding `(HeaderName, HeaderValue)` to the received message.
    AddHeader(&'static str, &'static str),
    /// `StopAction`, whose "only acceptable value" for `Scope` is `RuleSet`.
    Stop,
}

/// One rule of a row's rule set.
#[derive(Debug, Clone, Copy)]
struct Rule {
    /// `ReceiptRule$Name`.
    name: &'static str,
    /// `ReceiptRule$Enabled`.
    enabled: bool,
    /// `ReceiptRule$Recipients`, empty for a rule stating no condition.
    recipients: &'static [&'static str],
    /// `ReceiptRule$Actions`, in the order Amazon SES takes them.
    actions: &'static [Action],
}

impl Rule {
    /// A rule that is enabled and saves the message under `prefix` for `recipients`.
    const fn saving(
        name: &'static str,
        recipients: &'static [&'static str],
        actions: &'static [Action],
    ) -> Self {
        Self {
            name,
            enabled: true,
            recipients,
            actions,
        }
    }
}

/// The control rule: it states no condition, so it matches every recipient of the account, and
/// it is the LAST rule of the active set.
const CONTROL_RULE: Rule = Rule::saving("control", &[], &[Action::Save(CONTROL_PREFIX)]);

/// The `Rule.*` form fields of a `CreateReceiptRule` carrying `rule` against `bucket`, with
/// `kms_key_arn` on every `S3Action` when the case names a key.
fn rule_fields(rule: &Rule, bucket: &str, kms_key_arn: Option<&str>) -> Vec<Field> {
    let mut params = vec![
        field("Rule.Name", rule.name),
        field("Rule.Enabled", if rule.enabled { "true" } else { "false" }),
        field("Rule.ScanEnabled", "false"),
    ];
    for (index, recipient) in rule.recipients.iter().enumerate() {
        params.push(field(
            format!("Rule.Recipients.member.{}", index + 1),
            *recipient,
        ));
    }
    for (index, action) in rule.actions.iter().enumerate() {
        let at = format!("Rule.Actions.member.{}", index + 1);
        match action {
            Action::Save(prefix) => {
                params.push(field(format!("{at}.S3Action.BucketName"), bucket));
                params.push(field(format!("{at}.S3Action.ObjectKeyPrefix"), *prefix));
                if let Some(kms_key_arn) = kms_key_arn {
                    params.push(field(format!("{at}.S3Action.KmsKeyArn"), kms_key_arn));
                }
            }
            Action::AddHeader(name, value) => {
                params.push(field(format!("{at}.AddHeaderAction.HeaderName"), *name));
                params.push(field(format!("{at}.AddHeaderAction.HeaderValue"), *value));
            }
            Action::Stop => params.push(field(format!("{at}.StopAction.Scope"), "RuleSet")),
        }
    }
    params
}

/// `CreateReceiptRuleSet` for `rule_set`, then one `CreateReceiptRule` per rule in order.
async fn given_rule_set(
    router: &MultiServiceRouter,
    case: &str,
    rule_set: &str,
    rules: &[Rule],
    bucket: &str,
    kms_key_arn: Option<&str>,
) {
    let (status, body) = call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", rule_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating rule set {rule_set}: expected 200, got {status} with {body}"
    );
    for (index, rule) in rules.iter().enumerate() {
        let mut params = vec![field("RuleSetName", rule_set)];
        // "The name of an existing rule after which the new rule is placed. If this parameter
        // is null, the new rule is inserted at the beginning of the rule list"
        // (`CreateReceiptRuleRequest$After`), so each rule after the first names the one
        // before it and the set's evaluation order is the order these rows list.
        if let Some(previous) = index.checked_sub(1).map(|before| rules[before].name) {
            params.push(field("After", previous));
        }
        params.extend(rule_fields(rule, bucket, kms_key_arn));
        let (status, body) = call(router, "CreateReceiptRule", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating rule {}: expected 200, got {status} with {body}",
            rule.name
        );
    }
    // The evaluation order the rows reason about, read back from the service itself: a row's
    // barrier is the object its LAST rule writes, so an order other than this one would make
    // that barrier prove nothing.
    let (status, body) = call(
        router,
        "DescribeReceiptRuleSet",
        &[field("RuleSetName", rule_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing {rule_set}: expected 200, got {status} with {body}"
    );
    let order: Vec<String> = crate::harness::elements(&body, "Rules")
        .first()
        .map(|rules| crate::harness::elements(rules, "Name"))
        .unwrap_or_default();
    let intended: Vec<String> = rules.iter().map(|rule| rule.name.to_string()).collect();
    assert_eq!(
        order, intended,
        "case {case:?}: expected {rule_set} to evaluate its rules in this order, got {order:?}"
    );
}

/// `SetActiveReceiptRuleSet`, which selects the one rule set the account evaluates.
async fn set_active_rule_set(router: &MultiServiceRouter, case: &str, rule_set: &str) {
    let (status, body) = call(
        router,
        "SetActiveReceiptRuleSet",
        &[field("RuleSetName", rule_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: activating {rule_set}: expected 200, got {status} with {body}"
    );
}

/// `SendRawEmail` carrying `raw` for `recipients`, answering the `MessageId` it was accepted
/// under — the id the object a matching `S3Action` writes is keyed by.
///
/// `RawMessage$Data` is a blob, so the awsQuery wire carries it base64-encoded, through the
/// runtime's own encoder.
async fn send_raw(
    router: &MultiServiceRouter,
    case: &str,
    recipients: &[&str],
    raw: &[u8],
) -> String {
    let mut params = vec![
        field("Source", SENDER),
        field("RawMessage.Data", ::aws_smithy_types::base64::encode(raw)),
    ];
    for (index, recipient) in recipients.iter().enumerate() {
        params.push(field(
            format!("Destinations.member.{}", index + 1),
            *recipient,
        ));
    }
    let (status, body) = call(router, "SendRawEmail", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: sending: expected 200, got {status} with {body}"
    );
    element(&body, "MessageId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a MessageId, got {body}"))
}

/// The keys of `bucket` under `prefix`, in the order Amazon S3 lists them.
async fn keys(router: &MultiServiceRouter, case: &str, bucket: &str, prefix: &str) -> Vec<String> {
    let listed = s3(router)
        .list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: listing {bucket}/{prefix}: {error}"));
    listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// The ONE key under `prefix` of `bucket`, waited for until it is there.
///
/// The wait is for the CONDITION — an object under that prefix — looked for again every
/// [`POLL_INTERVAL`] until [`DELIVERY_DEADLINE`] passes, because a message is received after it
/// is accepted and nothing a sender is told says when.
async fn settled_key(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    prefix: &str,
) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let found = keys(router, case, bucket, prefix).await;
        if let Some(key) = found.first() {
            assert_eq!(
                found.len(),
                1,
                "case {case:?}: expected one object under {bucket}/{prefix}, got {found:?}"
            );
            return key.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no object appeared under {bucket}/{prefix} within \
             {deadline_secs}s",
            deadline_secs = DELIVERY_DEADLINE.as_secs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The bytes Amazon S3 holds at `key` of `bucket`.
async fn object_body(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str) -> Vec<u8> {
    let read = s3(router)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {bucket}/{key}: {error}"));
    read.body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {bucket}/{key}'s body: {error}"))
        .to_vec()
}

/// A small RFC 5322 message from `SENDER` to `to`, with `subject` — the message the matching
/// rows have delivered.
fn plain_message(to: &str, subject: &str) -> Vec<u8> {
    format!(
        "From: {SENDER}\r\nTo: {to}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=UTF-8\r\n\r\nthe body of the message\r\n"
    )
    .into_bytes()
}

/// One row of the table.
#[derive(Debug, Clone)]
struct Row {
    /// What this row is called in every assertion it makes.
    case: &'static str,
    /// The rules of the row's own rule set, in evaluation order.
    rules: Vec<Rule>,
    /// Whether the row's own rule set is the ACTIVE one. When it is not, the control rule
    /// lives in another set that is, so the row can still tell "not delivered" from "not
    /// delivered yet".
    active: bool,
    /// Whether the control rule is appended to the active set — `false` for a row whose
    /// `StopAction` would terminate the evaluation before it.
    control: bool,
    /// The envelope recipients of the message the row sends.
    recipients: &'static [&'static str],
    /// The key prefixes that must hold the message, each as ONE object keyed by the id the
    /// send was accepted under.
    stored: &'static [&'static str],
    /// The key prefixes that must hold nothing.
    absent: &'static [&'static str],
    /// The prefix whose object proves this message's steps are finished.
    barrier: &'static str,
}

/// The active receipt rule set decides what happens to every message this account accepts for
/// one of its own recipients.
#[tokio::test]
async fn an_accepted_message_is_delivered_by_the_active_receipt_rule_sets_matching_rules() {
    let rows = [
        Row {
            case: "a condition naming the recipient's own address",
            rules: vec![Rule::saving("exact", &[RECIPIENT], &[Action::Save("exact/")])],
            active: true,
            control: true,
            recipients: &[RECIPIENT],
            stored: &["exact/"],
            absent: &[],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a condition naming the recipient's domain",
            rules: vec![Rule::saving("domain", &[DOMAIN], &[Action::Save("domain/")])],
            active: true,
            control: true,
            recipients: &[RECIPIENT],
            stored: &["domain/"],
            absent: &[],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a condition naming a domain the recipient's domain sits under",
            rules: vec![Rule::saving("parent", &[DOMAIN], &[Action::Save("parent/")])],
            active: true,
            control: true,
            recipients: &[SUBDOMAIN_RECIPIENT],
            stored: &["parent/"],
            absent: &[],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a rule stating no condition at all",
            rules: vec![Rule::saving("all", &[], &[Action::Save("all/")])],
            active: true,
            control: true,
            recipients: &[RECIPIENT],
            stored: &["all/"],
            absent: &[],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a condition naming another mailbox of the same domain",
            rules: vec![Rule::saving(
                "elsewhere",
                &["someone-else@inbound.example.com"],
                &[Action::Save("elsewhere/")],
            )],
            active: true,
            control: true,
            recipients: &[RECIPIENT],
            stored: &[],
            absent: &["elsewhere/"],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a condition naming another domain",
            rules: vec![Rule::saving(
                "other-domain",
                &["elsewhere.example.org"],
                &[Action::Save("other-domain/")],
            )],
            active: true,
            control: true,
            recipients: &[RECIPIENT],
            stored: &[],
            absent: &["other-domain/"],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a matching rule that is disabled",
            rules: vec![Rule {
                name: "disabled",
                enabled: false,
                recipients: &[],
                actions: &[Action::Save("disabled/")],
            }],
            active: true,
            control: true,
            recipients: &[RECIPIENT],
            stored: &[],
            absent: &["disabled/"],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a matching rule whose rule set is not the active one",
            rules: vec![Rule::saving("inactive", &[], &[Action::Save("inactive/")])],
            active: false,
            control: true,
            recipients: &[RECIPIENT],
            stored: &[],
            absent: &["inactive/"],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a recipient this account holds no identity for",
            rules: vec![Rule::saving(
                "stranger",
                &["elsewhere.example.org"],
                &[Action::Save("stranger/")],
            )],
            active: true,
            control: true,
            recipients: &[STRANGER, CONTROL_RECIPIENT],
            stored: &[],
            absent: &["stranger/"],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "two rules of the active set matching the same recipient",
            rules: vec![
                Rule::saving("first-of-two", &[], &[Action::Save("first-of-two/")]),
                Rule::saving("second-of-two", &[DOMAIN], &[Action::Save("second-of-two/")]),
            ],
            active: true,
            control: true,
            recipients: &[RECIPIENT],
            stored: &["first-of-two/", "second-of-two/"],
            absent: &[],
            barrier: CONTROL_PREFIX,
        },
        Row {
            case: "a StopAction terminating the evaluation of the rule set",
            rules: vec![
                Rule::saving("stopping", &[], &[Action::Save("stopping/"), Action::Stop]),
                Rule::saving("after-the-stop", &[], &[Action::Save("after-the-stop/")]),
            ],
            active: true,
            control: false,
            recipients: &[RECIPIENT],
            stored: &["stopping/"],
            absent: &["after-the-stop/", CONTROL_PREFIX],
            barrier: "stopping/",
        },
    ];

    for row in rows {
        let (router, engine) = emulator();
        let bucket = "inbound-mail";
        given_verified_domain(&router, &engine, row.case).await;
        given_bucket(&router, row.case, bucket).await;
        let mut rules: Vec<Rule> = row.rules.clone();
        if row.active && row.control {
            rules.push(CONTROL_RULE);
        }
        given_rule_set(&router, row.case, "row-rules", &rules, bucket, None).await;
        if row.active {
            set_active_rule_set(&router, row.case, "row-rules").await;
        } else {
            given_rule_set(
                &router,
                row.case,
                "other-rules",
                &[CONTROL_RULE],
                bucket,
                None,
            )
            .await;
            set_active_rule_set(&router, row.case, "other-rules").await;
        }

        let raw = plain_message(row.recipients[0], row.case);
        let message_id = send_raw(&router, row.case, row.recipients, &raw).await;
        // The barrier: once the object the message's LAST step writes is there, every step of
        // that message has been carried out, so a prefix that is still empty is one no rule
        // wrote to.
        settled_key(&router, row.case, bucket, row.barrier).await;

        for prefix in row.stored {
            let key = settled_key(&router, row.case, bucket, prefix).await;
            assert_eq!(
                key,
                format!("{prefix}{message_id}"),
                "case {:?}: expected the object under {prefix} to be keyed by the accepted \
                 MessageId, got {key:?}",
                row.case
            );
            let stored = object_body(&router, row.case, bucket, &key).await;
            assert_eq!(
                stored,
                raw,
                "case {:?}: expected {key} to carry the message's own bytes, got {:?}",
                row.case,
                String::from_utf8_lossy(&stored)
            );
        }
        for prefix in row.absent {
            let found = keys(&router, row.case, bucket, prefix).await;
            assert!(
                found.is_empty(),
                "case {:?}: expected nothing under {bucket}/{prefix}, got {found:?}",
                row.case
            );
        }
    }
}

/// The attachment of a multipart message survives the receive path byte for byte: the part the
/// `Content-Disposition` filename names decodes to exactly the bytes that were attached.
#[tokio::test]
async fn a_multipart_messages_attachment_is_saved_byte_for_byte() {
    let case = "a multipart message with a binary attachment";
    let (router, engine) = emulator();
    let bucket = "attachment-mail";
    given_verified_domain(&router, &engine, case).await;
    given_bucket(&router, case, bucket).await;
    given_rule_set(
        &router,
        case,
        "attachments",
        &[Rule::saving("to-s3", &[RECIPIENT], &[Action::Save("raw/")])],
        bucket,
        None,
    )
    .await;
    set_active_rule_set(&router, case, "attachments").await;

    // Every byte value, four times over: a payload no text encoding survives, so the only way
    // the part can decode to it is for the message to have been stored unaltered.
    let payload: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
    let filename = "probe.bin";
    let subject = "attachment-probe-7f3a";
    let raw = format!(
        "From: {SENDER}\r\nTo: {RECIPIENT}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"the-boundary\"\r\n\r\n\
         --the-boundary\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nsee the attachment\r\n\
         --the-boundary\r\nContent-Type: application/octet-stream\r\n\
         Content-Transfer-Encoding: base64\r\n\
         Content-Disposition: attachment; filename=\"{filename}\"\r\n\r\n{}\r\n\
         --the-boundary--\r\n",
        ::aws_smithy_types::base64::encode(&payload)
    )
    .into_bytes();

    let message_id = send_raw(&router, case, &[RECIPIENT], &raw).await;
    let key = settled_key(&router, case, bucket, "raw/").await;
    assert_eq!(
        key,
        format!("raw/{message_id}"),
        "case {case:?}: expected the object to be keyed by the accepted MessageId, got {key:?}"
    );
    let stored = object_body(&router, case, bucket, &key).await;
    assert_eq!(
        stored, raw,
        "case {case:?}: expected the stored object to be the message's own bytes"
    );

    let parsed = mailparse::parse_mail(&stored)
        .unwrap_or_else(|error| panic!("case {case:?}: the stored object is a MIME message: {error}"));
    let read_subject =
        mailparse::MailHeaderMap::get_first_value(parsed.headers.as_slice(), "Subject");
    assert_eq!(
        read_subject.as_deref(),
        Some(subject),
        "case {case:?}: expected the Subject to survive, got {read_subject:?}"
    );
    let attachment = parsed
        .subparts
        .iter()
        .find(|part| {
            part.get_content_disposition()
                .params
                .get("filename")
                .is_some_and(|name| name == filename)
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected a part named {filename}"));
    assert_eq!(
        attachment.ctype.mimetype, "application/octet-stream",
        "case {case:?}: expected the attachment's media type to survive, got {:?}",
        attachment.ctype.mimetype
    );
    let decoded = attachment
        .get_body_raw()
        .unwrap_or_else(|error| panic!("case {case:?}: decoding the attachment: {error}"));
    assert_eq!(
        decoded.len(),
        payload.len(),
        "case {case:?}: expected {} bytes, got {}",
        payload.len(),
        decoded.len()
    );
    assert_eq!(
        digest(&decoded),
        digest(&payload),
        "case {case:?}: expected the attachment's bytes to be the ones that were attached"
    );
}

/// The SHA-256 of `bytes`, hex-encoded: what "the same bytes" is asserted by.
fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// An `AddHeaderAction` before an `S3Action` adds its header to the message the save then
/// stores, and leaves every byte that was submitted in place behind it.
#[tokio::test]
async fn an_add_header_action_adds_its_header_to_the_message_a_later_action_saves() {
    let case = "an AddHeaderAction before the save";
    let (router, engine) = emulator();
    let bucket = "header-mail";
    given_verified_domain(&router, &engine, case).await;
    given_bucket(&router, case, bucket).await;
    given_rule_set(
        &router,
        case,
        "headers",
        &[Rule::saving(
            "add-then-save",
            &[],
            &[
                Action::AddHeader("X-Inbound-Probe", "added-by-the-rule"),
                Action::Save("tagged/"),
            ],
        )],
        bucket,
        None,
    )
    .await;
    set_active_rule_set(&router, case, "headers").await;

    let raw = plain_message(RECIPIENT, "a message a rule adds a header to");
    send_raw(&router, case, &[RECIPIENT], &raw).await;
    let key = settled_key(&router, case, bucket, "tagged/").await;
    let stored = object_body(&router, case, bucket, &key).await;
    let expected = [b"X-Inbound-Probe: added-by-the-rule\r\n".as_slice(), &raw].concat();
    assert_eq!(
        stored,
        expected,
        "case {case:?}: expected the added header and then the submitted bytes, got {:?}",
        String::from_utf8_lossy(&stored)
    );
    let parsed = mailparse::parse_mail(&stored)
        .unwrap_or_else(|error| panic!("case {case:?}: the stored object is a MIME message: {error}"));
    let added =
        mailparse::MailHeaderMap::get_first_value(parsed.headers.as_slice(), "X-Inbound-Probe");
    assert_eq!(
        added.as_deref(),
        Some("added-by-the-rule"),
        "case {case:?}: expected the added header to be readable, got {added:?}"
    );
}

/// An `S3Action` naming a KMS key has the message saved in the Amazon S3 encryption client's
/// ENVELOPE under that key — "your mail is encrypted by Amazon SES using the Amazon S3
/// encryption client before the mail is submitted to Amazon S3 for storage. It is not encrypted
/// using Amazon S3 server-side encryption" (`S3Action$KmsKeyArn`).
///
/// What that makes the object: a body that is ciphertext — the message's own words are not in it
/// — and user metadata holding the envelope a reader opens it from, whose wrapped data key KMS's
/// OWN `Decrypt` opens under the material description the object carries, yielding the key that
/// decrypts the body to exactly the message that was sent.
#[tokio::test]
async fn an_s3_action_naming_a_kms_key_saves_the_message_in_the_encryption_envelope() {
    let case = "an S3Action naming a customer managed key";
    let (router, engine) = emulator();
    let bucket = "encrypted-mail";
    let rule = "to-s3-encrypted";
    given_verified_domain(&router, &engine, case).await;
    given_bucket(&router, case, bucket).await;
    // The key policy a bucket for inbound mail is given: the account root keeps `kms:*` — "the
    // key policy must allow the calling principal to make a subsequent PutKeyPolicy request"
    // — and the service principal is granted the encrypt operations `S3Action$KmsKeyArn` asks
    // for.
    let policy = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"Enable IAM User Permissions","Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{account}:root"}},"Action":"kms:*","Resource":"*"}},{{"Sid":"AllowSESEncryption","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":["kms:Encrypt","kms:GenerateDataKey*"],"Resource":"*"}}]}}"#,
        account = account()
    );
    let created = kms(&router)
        .create_key()
        .policy(policy)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: creating the key: {error}"));
    let key_arn = created
        .key_metadata()
        .and_then(|metadata| metadata.arn())
        .unwrap_or_else(|| panic!("case {case:?}: the key has an ARN"))
        .to_string();
    given_rule_set(
        &router,
        case,
        "encrypted",
        &[Rule::saving(rule, &[], &[Action::Save("sealed/")])],
        bucket,
        Some(&key_arn),
    )
    .await;
    set_active_rule_set(&router, case, "encrypted").await;

    // A token no part of the envelope can carry unless the body is the plaintext.
    let token = "the-plaintext-token-of-this-message";
    let raw = plain_message(RECIPIENT, token);
    send_raw(&router, case, &[RECIPIENT], &raw).await;
    let key = settled_key(&router, case, bucket, "sealed/").await;
    let read = s3(&router)
        .get_object()
        .bucket(bucket)
        .key(&key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {bucket}/{key}: {error}"));
    let metadata = read.metadata().cloned().unwrap_or_default();
    let entry = |name: &str| {
        metadata
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected {name} in {metadata:?}"))
    };
    assert_eq!(
        entry("x-amz-wrap-alg"),
        "kms",
        "case {case:?}: expected the data key to be wrapped by KMS, got {metadata:?}"
    );
    assert_eq!(
        entry("x-amz-cek-alg"),
        "AES/GCM/NoPadding",
        "case {case:?}: expected the content algorithm the envelope declares, got {metadata:?}"
    );
    let material: serde_json::Value = serde_json::from_str(&entry("x-amz-matdesc"))
        .unwrap_or_else(|error| panic!("case {case:?}: the material description is JSON: {error}"));
    assert_eq!(
        material.get("kms_cmk_id").and_then(serde_json::Value::as_str),
        Some(key_arn.as_str()),
        "case {case:?}: expected the material description to name the action's key, got {material}"
    );
    assert_eq!(
        material
            .get("aws:ses:rule-name")
            .and_then(serde_json::Value::as_str),
        Some(rule),
        "case {case:?}: expected the material description to name the triggering rule, got \
         {material}"
    );
    let body = read
        .body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading the object's body: {error}"))
        .to_vec();
    assert!(
        memchr::memmem::find(&body, token.as_bytes()).is_none(),
        "case {case:?}: expected the stored body to be ciphertext, got {:?}",
        String::from_utf8_lossy(&body)
    );

    // The envelope opens: KMS unwraps the data key under the material description the object
    // carries, and that key decrypts the body to the message that was sent.
    let wrapped = aws_smithy_types::base64::decode(entry("x-amz-key-v2"))
        .unwrap_or_else(|error| panic!("case {case:?}: the wrapped key is base64: {error}"));
    let context: std::collections::HashMap<String, String> = material
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the material description is an object"))
        .iter()
        .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect();
    let unwrapped = kms(&router)
        .decrypt()
        .ciphertext_blob(aws_smithy_types::Blob::new(wrapped))
        .set_encryption_context(Some(context))
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: unwrapping the data key: {error}"));
    let data_key = unwrapped
        .plaintext()
        .unwrap_or_else(|| panic!("case {case:?}: the unwrapped data key has plaintext"));
    let iv = aws_smithy_types::base64::decode(entry("x-amz-iv"))
        .unwrap_or_else(|error| panic!("case {case:?}: the IV is base64: {error}"));
    let cipher = ring::aead::LessSafeKey::new(
        ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, data_key.as_ref())
            .unwrap_or_else(|error| panic!("case {case:?}: the data key is an AES-256 key: {error}")),
    );
    let mut opened = body.clone();
    let plaintext = cipher
        .open_in_place(
            ring::aead::Nonce::try_assume_unique_for_key(&iv)
                .unwrap_or_else(|error| panic!("case {case:?}: the IV is a nonce: {error}")),
            ring::aead::Aad::empty(),
            &mut opened,
        )
        .unwrap_or_else(|error| panic!("case {case:?}: opening the envelope: {error}"));
    assert_eq!(
        plaintext,
        raw.as_slice(),
        "case {case:?}: expected the envelope to open to the message that was sent, got {:?}",
        String::from_utf8_lossy(plaintext)
    );
    assert_eq!(
        entry("x-amz-unencrypted-content-length"),
        raw.len().to_string(),
        "case {case:?}: expected the plaintext length the envelope declares to be the message's"
    );
}
