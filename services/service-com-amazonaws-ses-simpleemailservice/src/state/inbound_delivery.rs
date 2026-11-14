// SPDX-License-Identifier: CC0-1.0

//! The RECEIVING half of Amazon SES: what the account's ACTIVE receipt rule set does with a
//! message this account has just accepted for one of its own recipients.
//!
//! A receipt rule set is not inert configuration — it is the program Amazon SES runs on every
//! message it receives. `SetActiveReceiptRuleSet` "sets the specified receipt rule set as the
//! active receipt rule set", `DescribeActiveReceiptRuleSet` reports "the metadata and receipt
//! rules for the receipt rule set that is currently active", and `ReceiptRule$Recipients` is
//! "the recipient domains and email addresses that the receipt rule applies to. If this field
//! is not specified, this rule matches all recipients on all verified domains". So a message
//! accepted for a recipient this account holds a verified identity for is also RECEIVED by
//! this account: the active set's rules are evaluated in the order they are held, and each
//! ENABLED rule whose recipient conditions match carries out its actions, until a `StopAction`
//! "terminates the evaluation of the receipt rule set".
//!
//! Of those actions this module carries out the two that act on the message itself:
//!
//! * `S3Action` "saves the received message to an Amazon Simple Storage Service (Amazon S3)
//!   bucket" — one object per matching rule, written at the action's `ObjectKeyPrefix`
//!   followed by the message's own id, carrying the message's RFC 5322 bytes UNALTERED, so
//!   the MIME structure, the part media types, the `Content-Disposition` filenames and the
//!   transfer encodings a client submitted are the ones a client reading the object back
//!   parses. `S3Action$KmsKeyArn` is "the customer managed key that Amazon SES should use to
//!   encrypt your emails before saving them to the Amazon S3 bucket", and the same member says
//!   HOW: "your mail is encrypted by Amazon SES using the Amazon S3 encryption client before
//!   the mail is submitted to Amazon S3 for storage. It is not encrypted using Amazon S3
//!   server-side encryption" — so a write for an action naming one saves that client's
//!   envelope ([`super::s3_encryption_envelope`]) rather than the message in the clear.
//! * `AddHeaderAction` "adds a header to the received email", which the actions after it
//!   therefore act on.
//!
//! And before the actions of a rule whose `ScanEnabled` is set — "if true, then messages that
//! this receipt rule applies to are scanned for spam and viruses" — the results of the sender
//! authentication and of those scans are stamped on the message
//! ([`super::received_scan`]), so the actions of that rule act on a message carrying them.
//!
//! Every other action addresses a resource this module does not act on (a bounce sent back to
//! the sender, a Lambda function, an Amazon SNS notification, a WorkMail organization, an
//! Amazon Connect instance); nothing here reports one of them as carried out.
//!
//! The write is made the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]): the AWS SDK driven over
//! the emulator's own in-process transport ([`::service_common::s3_client`]), so nothing here
//! opens a socket or leaves the host. The destination is a bucket NAME and an object key, both
//! resolved by this same emulator's own object store, and the only part of the client's
//! addressing a request contributes is the signing Region, which is a Region LABEL by the time
//! it reaches here ([`super::region_from_request`]) — so no text of a request can steer where
//! a received message goes. A delivery that cannot be made leaves a line for an operator and
//! nothing else: the message was already ACCEPTED, and Amazon SES does not un-accept a message
//! because an action of a receipt rule failed.

/// How many STEPS of the active rule set ONE received message is carried out through — its
/// actions, and the stamping of the authentication and scan results a rule asks for.
///
/// An EMULATOR ceiling, not an AWS quota: a rule's `Actions` is a list a caller writes and the
/// active set holds up to [`super::MAX_RECEIPT_RULES_PER_RULE_SET`] rules, so the work one
/// accepted message can drive is bounded here ([`past_ceiling`], consulted before every step a
/// rule contributes). A received message is processed by the handful of actions its recipient's
/// rules name, so no rule set a client really writes reaches it. What one STAMPING costs is
/// bounded further by its being read once per message rather than once per rule
/// ([`receipt_steps`]).
pub const MAX_RECEIPT_ACTIONS_PER_MESSAGE: usize = 20;

/// One thing the active receipt rule set does with a received message, in evaluation order.
///
/// Read off the rule set while this service's store is held, and carried out afterwards
/// ([`deliver`]): the decision is about state, the delivery is a call to another service, and
/// holding a store across that call would make one message's delivery block every other
/// request this service serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptStep {
    /// `AddHeaderAction`: the header to add to the received message, which every step after
    /// this one acts on.
    AddHeader {
        /// `AddHeaderAction$HeaderName`.
        name: String,
        /// `AddHeaderAction$HeaderValue`.
        value: String,
    },
    /// The authentication and scan results Amazon SES stamps on the message because the rule
    /// whose actions follow has `ScanEnabled` ([`super::received_scan`]), which every step
    /// after this one acts on.
    Stamp {
        /// The header fields to add, in the order they are added.
        headers: Vec<(String, String)>,
    },
    /// `S3Action`: save the received message into the object store.
    Save {
        /// The name of the rule set the saving rule belongs to, which with the rule's own name
        /// identifies the rule a saved message is attributed to.
        rule_set: String,
        /// `ReceiptRule$Name` of the rule whose action this is.
        rule: String,
        /// `S3Action$BucketName`.
        bucket: String,
        /// `S3Action$ObjectKeyPrefix`, empty when the action names none.
        object_key_prefix: String,
        /// `S3Action$KmsKeyArn`, when the action names a key to encrypt the message under.
        kms_key_arn: Option<String>,
    },
}

/// The message this account has just ACCEPTED, as the receive path reads it.
///
/// One value for every send path of either Amazon SES surface, so what a receipt rule is
/// evaluated against does not depend on which API accepted the message.
#[derive(Debug, Clone, Copy)]
pub struct AcceptedMessage<'a> {
    /// The id the send was accepted under, which is what the object a saving action writes is
    /// keyed by.
    pub message_id: &'a str,
    /// The message's ENVELOPE recipients, which is what a receipt rule's conditions are
    /// compared against.
    pub recipients: &'a [String],
    /// The message's RFC 5322 bytes, as they were accepted.
    pub raw: &'a [u8],
    /// The envelope sender Amazon SES relays the message under, which an SPF result is about.
    pub envelope_from: &'a str,
    /// The domain the message's `From` field claims, which DMARC alignment is about.
    pub from_domain: &'a str,
}

/// A message this account has accepted, as Amazon SES SENDS it, together with what the account's
/// own active receipt rule set does with it.
///
/// What [`accepted`] produces while this service's store is held and [`deliver_accepted`]
/// carries out once it has been released — one value, so every send path of either Amazon SES
/// surface reaches the receive path the same way.
#[derive(Debug, Clone)]
pub struct Accepted {
    /// The id the send was accepted under.
    message_id: String,
    /// The message's RFC 5322 bytes AS SENT: the bytes that were accepted, plus the
    /// `DKIM-Signature` Amazon SES signs them with when the sending identity's Easy DKIM setup
    /// signs ([`super::dkim`]).
    message: Vec<u8>,
    /// The envelope recipients the rules were evaluated against.
    recipients: Vec<String>,
    /// The envelope sender Amazon SES relays the message under.
    envelope_from: String,
    /// The domain the message's `From` field claims.
    from_domain: String,
    /// What the active rule set does with the message, in evaluation order.
    steps: Vec<ReceiptStep>,
}

impl Accepted {
    /// The message's RFC 5322 bytes as Amazon SES sends them — what a receipt rule's actions
    /// act on, and what a send log that keeps the message keeps.
    #[must_use]
    pub fn message(&self) -> &[u8] {
        &self.message
    }
}

/// The message a send has just accepted, as Amazon SES sends it, and what this account's active
/// receipt rule set does with it.
///
/// `raw` is the message as the send composed or received it, `source` the sending identity's
/// address, and `recipients` the envelope recipients. Called with this service's store HELD,
/// because every decision here is about that store: whether the sending identity signs, what
/// envelope sender its MAIL FROM setting gives the message, and which rules of the active set
/// apply to the recipients.
#[must_use]
pub fn accepted(
    state: &mut super::ComAmazonawsSesSimpleemailserviceState,
    region: &str,
    message_id: &str,
    source: &str,
    recipients: &[String],
    raw: &[u8],
    sent_at: ::std::time::SystemTime,
) -> Accepted {
    let envelope_from = envelope_from(state, region, message_id, source);
    let from_domain = super::identity_domain(super::addr_spec(source)).to_string();
    // A message past the bytes this service reads at all is RECEIVED by nothing: every reading
    // of a message here — the signature it is signed with, the scan a rule asks for, the bytes
    // an action saves — is work the message's own size pays for, and this is the ceiling that
    // bounds it ([`super::MAX_MESSAGE_BYTES`], which is also what the send that carries its own
    // message refuses past). Nothing is refused here: the message was ACCEPTED before it
    // reached this path, so the only thing to do with one past the ceiling is to leave a line
    // for an operator.
    if raw.len() > super::MAX_MESSAGE_BYTES {
        ::tracing::warn!(
            message_id,
            bytes = raw.len(),
            ceiling = super::MAX_MESSAGE_BYTES,
            "this message is larger than the one this emulator receives, so the account's \
             receipt rules are not evaluated against it"
        );
        return Accepted {
            message_id: message_id.to_string(),
            message: raw.to_vec(),
            recipients: recipients.to_vec(),
            envelope_from,
            from_domain,
            steps: Vec::new(),
        };
    }
    let message = match super::dkim::signer(state, source) {
        Some(signer) => super::dkim::signed(raw, &signer, sent_at),
        None => raw.to_vec(),
    };
    let steps = receipt_steps(
        state,
        &AcceptedMessage {
            message_id,
            recipients,
            raw: &message,
            envelope_from: &envelope_from,
            from_domain: &from_domain,
        },
    );
    Accepted {
        message_id: message_id.to_string(),
        message,
        recipients: recipients.to_vec(),
        envelope_from,
        from_domain,
        steps,
    }
}

/// Carry out what the active receipt rule set does with `accepted` ([`deliver`]).
///
/// Called once the accepting request has decided everything it answers with, which is after
/// this service's store has been read: a message is RECEIVED behind its acceptance.
pub fn deliver_accepted(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    accepted: &Accepted,
) {
    deliver(
        ctx,
        region,
        &AcceptedMessage {
            message_id: &accepted.message_id,
            recipients: &accepted.recipients,
            raw: &accepted.message,
            envelope_from: &accepted.envelope_from,
            from_domain: &accepted.from_domain,
        },
        &accepted.steps,
    );
}

/// The envelope sender Amazon SES relays a message from `source` under.
///
/// Amazon SES is the sender, so the envelope sender is in a domain AMAZON SES speaks for: the
/// identity's own custom MAIL FROM domain once that setting is in place —
/// `SetIdentityMailFromDomain` records it and
/// `GetIdentityMailFromDomainAttributes` reports whether Amazon SES has read its MX record —
/// and otherwise the Region's own Amazon SES mail-from domain, which is what an identity with no
/// custom MAIL FROM setting sends under. The local part is the id the message was accepted
/// under, so the address names this message.
fn envelope_from(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    region: &str,
    message_id: &str,
    source: &str,
) -> String {
    let sender = super::identity_key(source);
    let custom = super::sender_identity_keys(&sender)
        .filter_map(|name| state.identities.get(name))
        .filter_map(|identity| identity.mail_from.as_ref())
        .find(|mail_from| mail_from.status == super::CustomMailFromStatus::Success)
        .map(|mail_from| mail_from.domain.clone());
    let domain = custom.unwrap_or_else(|| format!("{region}.{SES_MAIL_FROM_DOMAIN}"));
    format!("{message_id}@{domain}")
}

/// The domain Amazon SES's own MAIL FROM addresses are in, which an identity with no custom
/// MAIL FROM domain sends under.
const SES_MAIL_FROM_DOMAIN: &str = "amazonses.com";

/// What the account's active receipt rule set does with `accepted` — the steps [`deliver`] then
/// carries out, in evaluation order, and empty when this account receives no mail for any of its
/// recipients, when no rule set is active, or when no rule of the active set matches.
#[must_use]
pub fn receipt_steps(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    accepted: &AcceptedMessage<'_>,
) -> Vec<ReceiptStep> {
    let Some(rule_set) = state.active_rule_set() else {
        // "If you leave this parameter empty, the account's active rule set is disabled" —
        // an account with no active set receives nothing.
        return Vec::new();
    };
    // Only the recipients this account receives mail FOR are evaluated: a rule's conditions
    // match "all recipients on all verified domains", so a recipient on a domain this account
    // holds no verified identity for is not a recipient of this account at all.
    let received: Vec<&String> = accepted
        .recipients
        .iter()
        .filter(|recipient| receives_mail_for(state, recipient))
        .collect();
    if received.is_empty() {
        return Vec::new();
    }
    let mut steps: Vec<ReceiptStep> = Vec::new();
    // The results of the sender authentication and of the scans are the MESSAGE's, not a rule's:
    // every value in them is read off this message, this account's keys and this envelope
    // sender, so they are computed AT MOST ONCE per accepted message however many rules of the
    // active set ask for the scan, and that one reading is what each of those rules stamps.
    // Reading them per rule would multiply message-sized work by the size of the rule set.
    let mut scanned: Option<Vec<(String, String)>> = None;
    for rule in &rule_set.rules {
        // "If false, the receipt rule is inactive."
        if !rule.enabled {
            continue;
        }
        if !received
            .iter()
            .any(|recipient| matches_recipients(rule, recipient))
        {
            continue;
        }
        // "If true, then messages that this receipt rule applies to are scanned for spam and
        // viruses": the results of those checks are stamped on the message BEFORE the rule's
        // own actions, which therefore act on a message carrying them.
        if rule.scan_enabled {
            // Counted against the SAME per-message ceiling the actions are, and checked before
            // the step is built: a rule that names no action at all must not be able to add work
            // past it either.
            if past_ceiling(&steps, &rule.name) {
                return steps;
            }
            let headers = scanned.get_or_insert_with(|| {
                super::received_scan::result_headers(
                    state,
                    accepted.raw,
                    accepted.envelope_from,
                    accepted.from_domain,
                )
            });
            steps.push(ReceiptStep::Stamp {
                headers: headers.clone(),
            });
        }
        for action in rule.actions.as_deref().unwrap_or_default() {
            if past_ceiling(&steps, &rule.name) {
                return steps;
            }
            if let Some(add_header) = &action.add_header_action {
                steps.push(ReceiptStep::AddHeader {
                    name: add_header.header_name.clone(),
                    value: add_header.header_value.clone(),
                });
            }
            if let Some(s3_action) = &action.s3_action {
                steps.push(ReceiptStep::Save {
                    rule_set: rule_set.name.clone(),
                    rule: rule.name.clone(),
                    bucket: s3_action.bucket_name.clone(),
                    object_key_prefix: s3_action
                        .object_key_prefix
                        .clone()
                        .unwrap_or_default(),
                    kms_key_arn: s3_action.kms_key_arn.clone(),
                });
            }
            // "This action terminates the evaluation of the receipt rule set": the actions
            // before it in this rule have been taken, and no later rule is evaluated.
            if action.stop_action.is_some() {
                return steps;
            }
        }
    }
    steps
}

/// Whether the steps read off the active rule set so far have reached the per-message ceiling
/// ([`MAX_RECEIPT_ACTIONS_PER_MESSAGE`]) — with the line for an operator that says so.
///
/// Consulted before EVERY step a rule contributes, the stamping of a scan's results as well as
/// each action, so what one accepted message can drive is bounded whatever the rules of the
/// active set carry.
fn past_ceiling(steps: &[ReceiptStep], rule: &str) -> bool {
    if steps.len() < MAX_RECEIPT_ACTIONS_PER_MESSAGE {
        return false;
    }
    ::tracing::warn!(
        rule = ?rule,
        ceiling = MAX_RECEIPT_ACTIONS_PER_MESSAGE,
        "the active receipt rule set names more work for one received message than this \
         emulator carries out"
    );
    true
}

/// Whether this account RECEIVES mail for `recipient`: the address itself is one of its
/// verified identities, or it sits in a domain one of them covers.
///
/// The same reading of a verified domain every other gate of this service applies — a
/// verified domain covers the mailboxes in it and in its subdomains
/// ([`super::ComAmazonawsSesSimpleemailserviceState::covers_verified_domain`]) — so a rule
/// matching "all recipients on all verified domains" matches exactly the recipients this
/// account holds a verified identity for.
fn receives_mail_for(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    recipient: &str,
) -> bool {
    let address = super::addr_spec(recipient);
    state.is_verified(address) || state.covers_verified_domain(super::identity_domain(address))
}

/// Whether `rule`'s recipient conditions apply to `recipient`.
///
/// `ReceiptRule$Recipients` holds "the recipient domains and email addresses that the receipt
/// rule applies to. If this field is not specified, this rule matches all recipients on all
/// verified domains" — so a rule stating no condition applies to every recipient of the
/// account, and a condition applies when it names the recipient's own address, the recipient's
/// domain, or a domain the recipient's domain sits under. Compared without regard to case,
/// which is how a domain name is compared and how a mailbox is addressed in practice.
fn matches_recipients(rule: &crate::ops::types::ReceiptRule, recipient: &str) -> bool {
    let conditions = rule.recipients.as_deref().unwrap_or_default();
    if conditions.is_empty() {
        return true;
    }
    let address = super::addr_spec(recipient);
    let domain = super::identity_domain(address);
    conditions.iter().any(|condition| {
        let condition = condition.trim();
        condition.eq_ignore_ascii_case(address)
            || condition.eq_ignore_ascii_case(domain)
            || super::is_subdomain_of(domain, condition)
    })
}

/// How many received-message delivery tasks this process may have in flight at once.
///
/// An EMULATOR ceiling, not an AWS quota: a caller looping on sends must not be able to start
/// delivery tasks without end, and each task holds a copy of the message it is saving, which
/// `super::MAX_MESSAGE_BYTES` bounds. A message that finds every permit taken is delivered
/// nowhere and says so in the log, which is the same outcome as a destination that cannot be
/// reached — the delivery is best effort either way, and what the sender is told about its send
/// does not depend on it.
pub const MAX_IN_FLIGHT_DELIVERY_TASKS: usize = 16;

/// How many delivery tasks are in flight right now; see [`MAX_IN_FLIGHT_DELIVERY_TASKS`].
static IN_FLIGHT_DELIVERY_TASKS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One task's claim on an in-flight permit, released when the task ends — including when it
/// ends by panicking or by being dropped at runtime shutdown, because the release is a `Drop`.
struct DeliveryPermit;

impl DeliveryPermit {
    /// Claim a permit, or `None` when [`MAX_IN_FLIGHT_DELIVERY_TASKS`] are already in flight.
    fn claim() -> Option<Self> {
        IN_FLIGHT_DELIVERY_TASKS
            .try_update(
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
                |in_flight| (in_flight < MAX_IN_FLIGHT_DELIVERY_TASKS).then_some(in_flight + 1),
            )
            .ok()
            .map(|_| Self)
    }
}

impl Drop for DeliveryPermit {
    fn drop(&mut self) {
        IN_FLIGHT_DELIVERY_TASKS.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// Carry out `steps` for the message `accepted` describes, on the runtime's own time.
///
/// Called with this service's store HELD, because nothing it does touches that store: a
/// message is RECEIVED after it was accepted — the acceptance is answered first, and the
/// actions of the matching rules are carried out behind it — so the steps run on a task of
/// their own rather than on the accepting request's path. With no async runtime to spawn onto
/// there is nothing to run them on, and nothing a caller is told depends on them.
///
/// Nothing about the call that accepted the message changes: the message was accepted before
/// this runs, so a step that could not be carried out leaves a line for an operator and the
/// accepted send stands (see the module docs).
///
/// What bounds the task: one accepted message spawns at most one task, at most
/// [`MAX_IN_FLIGHT_DELIVERY_TASKS`] of them run at once, each carries out at most
/// [`MAX_RECEIPT_ACTIONS_PER_MESSAGE`] actions with no retry loop behind any of them, and the
/// whole delivery is abandoned after [`DELIVERY_DEADLINE`].
///
/// `region` is the Region the accepting request was signed for, which is where this account's
/// receipt rule set and the destinations its actions name live; no text of a request chooses
/// the endpoint a write is made to.
pub fn deliver(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    accepted: &AcceptedMessage<'_>,
    steps: &[ReceiptStep],
) {
    if steps.is_empty() {
        return;
    }
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        ::tracing::warn!(
            message_id = accepted.message_id,
            "no async runtime is available to carry out the receipt rules of this received \
             message on"
        );
        return;
    };
    let Some(permit) = DeliveryPermit::claim() else {
        ::tracing::warn!(
            message_id = accepted.message_id,
            in_flight = MAX_IN_FLIGHT_DELIVERY_TASKS,
            "this emulator is already delivering as many received messages as it delivers at \
             once; this message is delivered nowhere"
        );
        return;
    };
    // The callback is an owned handle into this same emulator, so the task that carries the
    // actions out outlives the request that accepted the message.
    let callback = ctx.callback().clone();
    let region = region.to_string();
    let identifier = accepted.message_id.to_string();
    let from_domain = accepted.from_domain.to_string();
    let message = accepted.raw.to_vec();
    let steps = steps.to_vec();
    runtime.spawn(async move {
        // Moved into the task, so the permit is held for exactly as long as the task runs and
        // is released however it ends.
        let _permit = permit;
        if ::tokio::time::timeout(
            DELIVERY_DEADLINE,
            carry_out(&callback, &region, &identifier, &from_domain, message, &steps),
        )
        .await
        .is_err()
        {
            ::tracing::warn!(
                message_id = identifier,
                deadline_secs = DELIVERY_DEADLINE.as_secs(),
                "the receipt rules of this received message were not carried out inside the \
                 deadline and were abandoned"
            );
        }
    });
}

/// How long the actions of ONE received message are carried out for before the delivery is
/// abandoned.
///
/// An EMULATOR deadline, not an AWS one: every call behind a step is in-process, with retries
/// and the SDK's own timeouts disabled, so this is what keeps a task waiting on a slow handler
/// from holding its permit for the life of the process.
pub const DELIVERY_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// Carry out `steps` for one received message, in order, against the services their actions
/// address.
async fn carry_out(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    message_id: &str,
    from_domain: &str,
    raw: Vec<u8>,
    steps: &[ReceiptStep],
) {
    let client = ::service_common::s3_client::s3_client(callback, region);
    // The message as the steps taken so far leave it: the bytes a client submitted, plus the
    // headers an `AddHeaderAction` or a scan has added to it.
    let mut message = raw;
    for step in steps {
        match step {
            ReceiptStep::AddHeader { name, value } => with_header(&mut message, name, value),
            ReceiptStep::Stamp { headers } => with_headers(&mut message, headers),
            ReceiptStep::Save {
                rule_set,
                rule,
                bucket,
                object_key_prefix,
                kms_key_arn,
            } => {
                let key = format!("{object_key_prefix}{message_id}");
                let write = match kms_key_arn
                    .as_deref()
                    .map(str::trim)
                    .filter(|kms_key_arn| !kms_key_arn.is_empty())
                {
                    // "If you do not specify an Amazon Web Services KMS key, Amazon SES does
                    // not encrypt your emails": the object is the message itself.
                    None => client
                        .put_object()
                        .bucket(bucket)
                        .key(&key)
                        .body(::aws_sdk_s3::primitives::ByteStream::from(message.clone())),
                    // "Your mail is encrypted by Amazon SES using the Amazon S3 encryption
                    // client before the mail is submitted to Amazon S3 for storage": the object
                    // is that client's envelope, whose data key KMS itself seals under the key
                    // the action named ([`super::s3_encryption_envelope`]).
                    Some(kms_key_arn) => {
                        let material = saved_message_material(
                            region,
                            rule_set,
                            rule,
                            message_id,
                            from_domain,
                            kms_key_arn,
                        );
                        let Some(envelope) = super::s3_encryption_envelope::sealed(
                            callback,
                            region,
                            kms_key_arn,
                            &material,
                            &message,
                        )
                        .await
                        else {
                            continue;
                        };
                        let mut write = client
                            .put_object()
                            .bucket(bucket)
                            .key(&key)
                            .body(::aws_sdk_s3::primitives::ByteStream::from(envelope.body));
                        for (name, value) in envelope.metadata {
                            write = write.metadata(name, value);
                        }
                        write
                    }
                };
                if let Err(failure) = write.send().await {
                    ::tracing::warn!(
                        bucket = ?bucket,
                        key = ?key,
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                        "a received message could not be saved into the receipt rule's bucket"
                    );
                }
            }
        }
    }
}

/// The material description the data key of an encrypted saved message is sealed under: what
/// the message IS — the rule that saved it, the id it was accepted under, the account and rule
/// it was saved for, the domain it came from — and the key it is sealed under, which is how the
/// Amazon S3 encryption client records the material of an object.
///
/// An entry with no value is left out, because an encryption context binds non-empty pairs.
fn saved_message_material(
    region: &str,
    rule_set: &str,
    rule: &str,
    message_id: &str,
    from_domain: &str,
    kms_key_arn: &str,
) -> Vec<(String, String)> {
    [
        ("aws:ses:rule-name", rule.to_string()),
        ("aws:ses:message-id", message_id.to_string()),
        (
            "aws:ses:source-account",
            ::engine::EMULATOR_ACCOUNT_ID.to_string(),
        ),
        (
            "aws:ses:source-arn",
            super::receipt_destinations::receipt_rule_arn(region, rule_set, rule),
        ),
        ("aws:ses:from-domain", from_domain.to_string()),
        (
            super::s3_encryption_envelope::MATERIAL_KEY_ID,
            kms_key_arn.to_string(),
        ),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// Add the header `name: value` to the received message `message`.
///
/// Added at the TOP of the header block, which is where a message-transfer agent adds a header
/// of its own: the fields already in the block keep their order and their bytes, and the body
/// is untouched. The line ends `CRLF`, the line ending RFC 5322 states for a message's fields;
/// a name and a value outside the charsets `AddHeaderAction` documents for them were refused
/// when the rule was stored ([`super::is_header_name`], [`super::is_header_value`]), so no
/// value reaching here can carry a line ending of its own.
fn with_header(message: &mut Vec<u8>, name: &str, value: &str) {
    let field = format!("{name}: {value}\r\n");
    message.splice(..0, field.into_bytes());
}

/// Add `headers` to the received message, in the order given, at the top of its header block
/// ([`with_header`]).
fn with_headers(message: &mut Vec<u8>, headers: &[(String, String)]) {
    let block: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    message.splice(..0, block.into_bytes());
}
