// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pinpointemail-amazonpinpointemailservice service.
//!
//! One process-wide store behind the engine's mutex, holding the three collections the
//! implemented slice of this service computes its answers from: verified email
//! identities, configuration sets, and the log of messages `SendEmail` accepted.
//!
//! Both maps are [`BTreeMap`]s, so iteration order is the stored keys' order rather
//! than a hash order that varies per process: `ListEmailIdentities` pages over a stable
//! sequence, which is what makes its `NextToken` mean the same thing on the next call.
//!
//! The identity table here is this surface's VIEW of one account-wide set of email
//! identities, which lives in the shared engine as
//! [`::service_common::ses_identities::SesIdentityDirectory`] and which the two Amazon
//! SES surfaces publish to and read as well: an identity belongs to the account rather
//! than to the API that created it. What a record here adds to that set is what THIS
//! API adds to an identity — its DKIM attributes, its tags, its custom MAIL FROM
//! configuration — and [`reconcile_identities`] is where the two are brought back into
//! agreement, on every operation this service serves.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DeliveryOptions, DkimAttributes, EventDestinationDefinition, MailFromAttributes,
    ReputationOptions, SendingOptions, Tag, TrackingOptions,
};

use sha2::{Digest, Sha256};

/// What a handler says when the engine it was constructed with does not carry this
/// service's store. The router always builds the emulator on `engine::InMemoryEngine`,
/// so this is unreachable there; a handler must still answer a modelled error rather
/// than construct a private engine, whose writes nobody else would ever see.
pub const STATE_UNAVAILABLE: &str =
    "the Pinpoint Email emulator's state store is not available on this engine";

/// `IdentityType` for an identity that is an email address (`IdentityType` enum,
/// `shared_types::IDENTITY_TYPE_VALUES`).
pub const IDENTITY_TYPE_EMAIL_ADDRESS: &str = "EMAIL_ADDRESS";
/// `IdentityType` for an identity that is a domain.
pub const IDENTITY_TYPE_DOMAIN: &str = "DOMAIN";

/// `DkimStatus` for a domain whose CNAME records have not been detected yet
/// (`DkimStatus` enum, `shared_types::DKIM_STATUS_VALUES`).
pub const DKIM_STATUS_PENDING: &str = "PENDING";
/// `DkimStatus` for a domain whose CNAME records have been detected and are correct.
pub const DKIM_STATUS_SUCCESS: &str = "SUCCESS";
/// `DkimStatus` for an identity nobody has started the DKIM process for.
pub const DKIM_STATUS_NOT_STARTED: &str = "NOT_STARTED";

/// How long after `CreateEmailIdentity` an identity's verification is due.
///
/// A new identity is reported in a non-terminal state — not verified for sending, DKIM
/// `PENDING` — and real verification finishes out of band: a domain when its DKIM CNAME
/// records appear in DNS (AWS documents "around 72 hours"), an address when its owner
/// follows the link in the verification email. Neither instant is one the emulator can
/// observe, so it owes the transition on its own clock, at a chosen span: long enough
/// that a caller reading straight after the create still sees the pending identity, short
/// enough that a client script never waits on it. The instant is stored per record and
/// materialised when a later call reads it; nothing sleeps.
pub const VERIFICATION_DUE_AFTER: Duration = Duration::from_millis(500);

/// How many DKIM tokens a domain identity gets. `DkimAttributes$Tokens` is documented as
/// "a set of unique strings that you use to create a set of CNAME records"; AWS hands out
/// three of them for a domain.
const DKIM_TOKEN_COUNT: usize = 3;

/// The most email identities this store holds. AWS caps how many identities an account
/// may have in a Region, and the emulator needs a cap of its own anyway: without one a
/// caller could grow the store until the process runs out of memory. A create past the cap
/// is refused with the modelled `LimitExceededException` ("There are too many instances of
/// the specified resource type"), never served.
pub const MAX_EMAIL_IDENTITIES: usize = 10_000;

/// The most configuration sets this store holds — see [`MAX_EMAIL_IDENTITIES`] for why the
/// cap exists and how a create past it is refused.
pub const MAX_CONFIGURATION_SETS: usize = 10_000;

/// How many accepted messages the sent-message log retains.
///
/// The log is an emulator-side record rather than an API resource: no operation reads an
/// individual entry, so it is a ring buffer and the oldest entry is dropped once the cap
/// is reached, which bounds the memory a sequence of sends can occupy. `MessageId`
/// uniqueness does not depend on the log, because `minted_message_ids` is monotone and is
/// never reset.
pub const SENT_MESSAGE_LOG_LIMIT: usize = 1_000;

/// How much of a caller-supplied address or resource name a log entry retains: the longest
/// an email address can be — a 64-character local part, `@`, and a 255-character domain.
const RETAINED_ADDRESS_CHARS: usize = 320;

/// How much of a message's subject a log entry retains.
const RETAINED_SUBJECT_CHARS: usize = 256;

/// A log entry's copy of a caller-supplied address or resource name.
pub fn retained_address(value: &str) -> String {
    value.chars().take(RETAINED_ADDRESS_CHARS).collect()
}

/// A log entry's copy of a caller-supplied subject.
pub fn retained_subject(value: &str) -> String {
    value.chars().take(RETAINED_SUBJECT_CHARS).collect()
}

/// One stored email identity — what `CreateEmailIdentity` persists and
/// `GetEmailIdentity` / `ListEmailIdentities` report.
#[derive(Debug, Clone)]
pub struct EmailIdentityRecord {
    /// The address or domain this identity was created under; also its key in the store.
    pub identity_name: String,
    /// One of `IDENTITY_TYPE_EMAIL_ADDRESS` / `IDENTITY_TYPE_DOMAIN`, classified from the
    /// identity's own form by [`classify_identity_type`]. `MANAGED_DOMAIN` is a domain AWS
    /// itself manages, so no `CreateEmailIdentity` call produces it.
    pub identity_type: String,
    /// Whether the identity finished verification. A newly created identity has not: it
    /// becomes verified when the transition [`VERIFICATION_DUE_AFTER`] describes comes
    /// due.
    pub verified_for_sending: bool,
    /// Whether bounce and complaint feedback is forwarded by email. AWS enables feedback
    /// forwarding for a new identity; `PutEmailIdentityFeedbackAttributes` changes it.
    pub feedback_forwarding_enabled: bool,
    /// Whether email may be sent from the identity (`IdentityInfo$SendingEnabled`), which
    /// follows verification.
    pub sending_enabled: bool,
    /// DKIM configuration, present for a domain identity and absent for an address.
    pub dkim: Option<DkimAttributes>,
    /// A custom MAIL FROM domain, set by `PutEmailIdentityMailFromAttributes`.
    pub mail_from: Option<MailFromAttributes>,
    /// The tags the create call carried.
    pub tags: Vec<Tag>,
    /// When the identity was created.
    pub created_at: SystemTime,
    /// When this identity's verification comes due, or `None` once it has been applied.
    pub verification_due_at: Option<SystemTime>,
    /// Whether the account's identity registry
    /// ([`::service_common::ses_identities::SesIdentityDirectory`]) already carries this
    /// identity.
    ///
    /// The registry is the account's SET of email identities, which the two Amazon SES
    /// surfaces publish to and read as well, so this flag is what tells an identity
    /// DELETED through one of them — registered here, gone from the registry — from one
    /// this service has created but not published yet ([`reconcile_identities`]).
    pub registered: bool,
}

impl EmailIdentityRecord {
    /// A freshly created, not-yet-verified identity for `identity_name`.
    pub fn new(identity_name: &str, created_at: SystemTime) -> Self {
        let identity_type = classify_identity_type(identity_name);
        let dkim = (identity_type == IDENTITY_TYPE_DOMAIN).then(|| DkimAttributes {
            signing_enabled: true,
            status: Some(DKIM_STATUS_PENDING.to_string()),
            tokens: Some(dkim_tokens(identity_name)),
        });
        Self {
            identity_name: identity_name.to_string(),
            identity_type: identity_type.to_string(),
            verified_for_sending: false,
            feedback_forwarding_enabled: true,
            sending_enabled: false,
            dkim,
            mail_from: None,
            tags: Vec::new(),
            created_at,
            verification_due_at: Some(created_at + VERIFICATION_DUE_AFTER),
            // Published to the account registry by the operation that created the
            // identity, so the Amazon SES surfaces answer for it from the moment that
            // call returns.
            registered: false,
        }
    }

    /// An identity of this account that the registry holds and this service does not, as
    /// this service's own record of it.
    ///
    /// The account holds the identity in the verification state the registry carries, so
    /// the record is adopted in THAT state rather than started afresh as unverified: the
    /// identity was created through another of the account's email APIs, and every one of
    /// them reports one state for it. Everything this service's own API adds — tags, a
    /// custom MAIL FROM domain — starts where `CreateEmailIdentity` starts it, since none
    /// of it has been asked for.
    pub fn adopted(
        record: &::service_common::ses_identities::SesIdentityRecord,
        now: SystemTime,
    ) -> Self {
        use ::service_common::ses_identities::SesVerificationState;

        let mut adopted = Self::new(&record.name, now);
        adopted.registered = true;
        // Easy DKIM is set up for the ACCOUNT's identity: when the account already
        // holds a setup for this one — recorded through another of its email APIs —
        // the record is adopted with THOSE tokens, the records the owner publishes,
        // rather than with a second set of this API's own making. An identity the
        // account holds NO setup for has had DKIM started for it by nobody, which is
        // what `NOT_STARTED` reports: this record must not fabricate a key pair, a token
        // set and a signing state its owner never asked for and no other API over these
        // identities reports.
        let has_dkim = adopted.dkim.is_some();
        adopted.dkim = match record.dkim.as_ref() {
            Some(shared) => Some(DkimAttributes {
                signing_enabled: shared.signing_enabled,
                status: Some(service_dkim_status(shared.status).to_string()),
                tokens: Some(shared.tokens.clone()),
            }),
            None if has_dkim => Some(DkimAttributes {
                signing_enabled: false,
                status: Some(DKIM_STATUS_NOT_STARTED.to_string()),
                tokens: None,
            }),
            // An address identity has no DKIM setup of its own on this surface either
            // way: DKIM signing is set up for a domain.
            None => None,
        };
        match record.verification {
            // Verified: the same three facts this service writes itself when a
            // verification falls due.
            SesVerificationState::Success => {
                adopted.verification_due_at = None;
                adopted.verified_for_sending = true;
                adopted.sending_enabled = true;
                adopted.settle_dkim_with_verification();
            }
            // Still being verified: the move is owed at the instant the registry
            // carries, so this service reports it exactly when the others do.
            SesVerificationState::Pending => adopted.verification_due_at = record.verification_due,
            // A verification that failed or was never started is owed no move at all.
            SesVerificationState::Failed
            | SesVerificationState::TemporaryFailure
            | SesVerificationState::NotStarted => adopted.verification_due_at = None,
        }
        adopted
    }

    /// Apply the verification the clock has already made due.
    ///
    /// Safe for a reporting operation to call before it answers: the transition was owed
    /// at `verification_due_at` whether or not anybody called, so materialising it on a
    /// read shows no caller anything it would not have seen anyway. It is the only write
    /// a read makes.
    pub fn materialize_due_verification(&mut self, now: SystemTime) {
        let Some(due) = self.verification_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.verification_due_at = None;
        self.verified_for_sending = true;
        self.sending_enabled = true;
        self.settle_dkim_with_verification();
    }

    /// An identity whose verification succeeded is one whose published records the
    /// service has read, so a DKIM setup still WAITING on them is no longer waiting.
    ///
    /// A setup nobody started stays not started: a verification that completed through
    /// some other route detects no records that were never published.
    fn settle_dkim_with_verification(&mut self) {
        if let Some(dkim) = &mut self.dkim {
            if dkim.status.as_deref() == Some(DKIM_STATUS_PENDING) {
                dkim.status = Some(DKIM_STATUS_SUCCESS.to_string());
            }
        }
    }
}

/// Classify an identity from its own form: an email address carries a local part and a
/// domain separated by `@`, anything else names a domain.
pub fn classify_identity_type(identity_name: &str) -> &'static str {
    if identity_name.contains('@') {
        IDENTITY_TYPE_EMAIL_ADDRESS
    } else {
        IDENTITY_TYPE_DOMAIN
    }
}

/// The DKIM tokens for a domain identity.
///
/// The tokens are opaque strings a customer copies into DNS; nothing in the API derives
/// meaning from them, so the emulator derives them from the domain itself with SHA-256
/// (truncated to 16 bytes and hex-encoded, giving the 32-character lowercase
/// alphanumeric token shape AWS hands out). That makes them unique per domain and stable
/// for as long as the record lives, without any per-call randomness.
fn dkim_tokens(domain: &str) -> Vec<String> {
    (0..DKIM_TOKEN_COUNT)
        .map(|index| {
            let mut hasher = Sha256::new();
            hasher.update(domain.as_bytes());
            hasher.update(b"#dkim#");
            hasher.update(index.to_string().as_bytes());
            hex::encode(&hasher.finalize()[..16])
        })
        .collect()
}

/// One stored configuration set — what `CreateConfigurationSet` persists and
/// `GetConfigurationSet` reports.
#[derive(Debug, Clone, Default)]
pub struct ConfigurationSetRecord {
    /// The set's name; also its key in the store.
    pub name: String,
    pub tracking_options: Option<TrackingOptions>,
    pub delivery_options: Option<DeliveryOptions>,
    pub reputation_options: Option<ReputationOptions>,
    pub sending_options: Option<SendingOptions>,
    pub tags: Vec<Tag>,
    /// Event destinations attached to this set, keyed by `EventDestinationName`. The
    /// `*ConfigurationSetEventDestination` operations that fill it are still scaffolded.
    pub event_destinations: BTreeMap<String, EventDestinationDefinition>,
    /// Whether the ACCOUNT's configuration-set registry
    /// ([`::service_common::ses_configuration_sets`]) already carries this set.
    ///
    /// A set this service recorded is published there, so the two Amazon SES surfaces answer
    /// for it too; this flag is what tells a set deleted through one of them — registered
    /// here, gone from the registry — from one this service has recorded but not published
    /// yet.
    pub registered: bool,
}

/// One message `SendEmail` accepted. The log is what makes a minted `MessageId` the id of
/// something that exists, and backs the account-level send accounting `GetAccount` will
/// report once it is implemented.
///
/// Every field is either emulator-minted or a BOUNDED summary of the request: the
/// recipients, tags and body are kept as counts and sizes, and the addresses and subject
/// as a bounded prefix ([`retained_address`], [`retained_subject`]), so one request cannot
/// decide how much memory its entry occupies. Nothing delivers mail here, so no caller can
/// read a message back and the full payload has no reader.
#[derive(Debug, Clone)]
pub struct SentMessageRecord {
    pub message_id: String,
    pub from_email_address: Option<String>,
    /// How many `ToAddresses` + `CcAddresses` + `BccAddresses` recipients the send named.
    pub recipient_count: usize,
    /// Which `EmailContent` member the send carried: `SIMPLE_TEXT`, `SIMPLE_HTML`,
    /// `SIMPLE` (a subject with no body part), `RAW` or `TEMPLATE`.
    pub content_form: String,
    pub content_subject: Option<String>,
    /// Size in bytes of the body the request carried; the body itself is not retained.
    pub content_body_bytes: usize,
    pub reply_to_address_count: usize,
    pub email_tag_count: usize,
    pub configuration_set_name: Option<String>,
    pub sent_at: SystemTime,
}

/// Per-service state for com-amazonaws-pinpointemail-amazonpinpointemailservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPinpointemailAmazonpinpointemailserviceState {
    /// Email identities, keyed by `EmailIdentity`. At most [`MAX_EMAIL_IDENTITIES`].
    pub identities: BTreeMap<String, EmailIdentityRecord>,
    /// Configuration sets, keyed by `ConfigurationSetName`. At most
    /// [`MAX_CONFIGURATION_SETS`].
    pub configuration_sets: BTreeMap<String, ConfigurationSetRecord>,
    /// The most recent accepted messages, oldest first, at most
    /// [`SENT_MESSAGE_LOG_LIMIT`] of them.
    pub sent_messages: VecDeque<SentMessageRecord>,
    /// How many message ids have been minted. Monotone and never reset, so two sends can
    /// never be handed the same `MessageId` even though the log drops its oldest entries.
    pub minted_message_ids: u64,
}

impl ComAmazonawsPinpointemailAmazonpinpointemailserviceState {
    /// Apply every identity verification the clock has made due. A reporting operation
    /// over the whole collection calls this before it reads — see
    /// [`EmailIdentityRecord::materialize_due_verification`].
    pub fn materialize_due_verifications(&mut self, now: SystemTime) {
        for record in self.identities.values_mut() {
            record.materialize_due_verification(now);
        }
    }

    /// Append one accepted message to the log, dropping the oldest entry once the log
    /// holds [`SENT_MESSAGE_LOG_LIMIT`] of them.
    pub fn record_sent_message(&mut self, record: SentMessageRecord) {
        self.sent_messages.push_back(record);
        while self.sent_messages.len() > SENT_MESSAGE_LOG_LIMIT {
            self.sent_messages.pop_front();
        }
    }

    /// Mint the next `MessageId`: an opaque, never-repeated id in the shape SES answers
    /// an accepted send with — the version prefix and a time field, sixteen hexadecimal
    /// digits together, then a hyphen-separated 8-4-4-4-12 hexadecimal field, then the
    /// trailing zero field.
    ///
    /// `OutboundMessageId` carries no modelled constraint, so the FORM is the one the
    /// service answers with (the same shape the two SES surfaces over these same accounts
    /// mint, since one accepted message is identified in one form whichever API accepted
    /// it) while the guarantee this makes is the documented one: the id is unique per
    /// accepted message. Uniqueness is STRUCTURAL rather than probabilistic: the LAST
    /// field is this store's own monotone counter, so two ids it minted always differ in
    /// it — two sends in the same instant included — and the fields before it only keep
    /// the rest of the id opaque.
    pub fn mint_message_id(&mut self, sent_at: SystemTime) -> String {
        self.minted_message_ids += 1;
        let since_epoch = sent_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let millis = since_epoch.as_millis();
        let mut digest = Sha256::new();
        digest.update(since_epoch.as_nanos().to_be_bytes());
        digest.update(self.minted_message_ids.to_be_bytes());
        let hex = hex::encode(digest.finalize());
        // The counter fills the last field, which is twelve hexadecimal digits wide: a
        // store that minted more ids than that field can hold would have to mint 2^48 of
        // them, far past the log this service keeps.
        let minted = self.minted_message_ids & 0xffff_ffff_ffff;
        format!(
            "0100{millis:012x}-{}-{}-{}-{}-{minted:012x}-000000",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
        )
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
///
/// Every call RECONCILES this service's identity table against the account's identity
/// registry first ([`reconcile_identities`]), so an operation served here answers for
/// every email identity the account holds — including the ones recorded through the
/// Amazon SES surfaces — and for none it has deleted.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>> {
    let engine = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let state = engine.state::<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>();
    let registry = engine.state::<::service_common::ses_identities::SesIdentityDirectory>();
    reconcile_identities(&state, &registry, SystemTime::now());
    let configuration_sets =
        engine.state::<::service_common::ses_configuration_sets::SesConfigurationSetDirectory>();
    reconcile_configuration_sets(&state, &configuration_sets, SystemTime::now());
    Some(state)
}

/// The account's configuration-set registry in the shared engine, which this service and the
/// two Amazon SES surfaces all publish to and read: call it with a handler's `&*self.engine`.
///
/// An operation that CREATES, CONFIGURES or DELETES a configuration set publishes the change
/// here, so the other surfaces answer for it from the moment the call returns. Take this lock
/// while holding this service's own store, never the other way round, so two callers on
/// different surfaces cannot wait on each other.
pub fn configuration_set_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::ses_configuration_sets::SesConfigurationSetDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<
            ::service_common::ses_configuration_sets::SesConfigurationSetDirectory,
        >)
}

/// Bring this service's configuration-set table and the account's configuration-set registry
/// back into agreement at `now`, so every API over the account's configuration sets answers
/// for ONE set of them.
///
/// The reconciliation itself is the registry's
/// ([`::service_common::ses_configuration_sets::SesConfigurationSetDirectory::reconcile`]),
/// written once there rather than per surface; what this service contributes is its
/// [`::service_common::ses_configuration_sets::AccountConfigurationSetRecord`] impl below.
fn reconcile_configuration_sets(
    state: &::engine::ServiceState<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>,
    registry: &::engine::ServiceState<
        ::service_common::ses_configuration_sets::SesConfigurationSetDirectory,
    >,
    now: SystemTime,
) {
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    registry.reconcile(&mut state.configuration_sets, now);
}

/// Publish the configuration set `name` as this service now holds it to the account's
/// configuration-set registry, so every other API over the account's configuration sets
/// reports what this one last wrote.
///
/// What an operation that CREATES or CHANGES a configuration set calls once it has written
/// it. Changes nothing when the account holds no such set, and writes nothing but the
/// registry's record of it. Reads this service's store WITHOUT reconciling it: the change
/// being published is newer than the registry's copy of it.
pub fn publish_configuration_set<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) {
    let Some(engine) = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
    else {
        return;
    };
    let state = engine.state::<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>();
    let registry =
        engine.state::<::service_common::ses_configuration_sets::SesConfigurationSetDirectory>();
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    let Some(held) = state.configuration_sets.get(name) else {
        return;
    };
    if registry
        .publish(
            ::service_common::ses_configuration_sets::AccountConfigurationSetRecord::published(
                held,
            ),
            SystemTime::now(),
        )
        .is_some()
    {
        if let Some(held) = state.configuration_sets.get_mut(name) {
            held.registered = true;
        }
    }
}

/// The account's identity registry in the shared engine, which this service and the two
/// Amazon SES surfaces all publish to and read: call it with a handler's `&*self.engine`.
///
/// An operation that CREATES or DELETES an email identity publishes the change here, so
/// the other surfaces answer for it from the moment the call returns. Take this lock
/// while holding this service's own store, never the other way round, so two callers on
/// different surfaces cannot wait on each other.
pub fn identity_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::ses_identities::SesIdentityDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<
            ::service_common::ses_identities::SesIdentityDirectory,
        >)
}

/// Bring this service's identity table and the account's identity registry back into
/// agreement at `now`, so every API over the account's email identities answers for ONE
/// set of them.
///
/// The reconciliation itself is the registry's
/// ([`::service_common::ses_identities::SesIdentityDirectory::reconcile`]), written once
/// there rather than per surface; what this service contributes is its
/// [`::service_common::ses_identities::AccountIdentityRecord`] impl below.
fn reconcile_identities(
    state: &::engine::ServiceState<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>,
    registry: &::engine::ServiceState<::service_common::ses_identities::SesIdentityDirectory>,
    now: SystemTime,
) {
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    registry.reconcile(&mut state.identities, now);
}

impl ::service_common::ses_identities::AccountIdentityRecord for EmailIdentityRecord {
    /// This service's table is keyed by the `EmailIdentity` as the request spelled it,
    /// the way its own operations look one up.
    fn table_key(name: &str) -> String {
        name.to_string()
    }

    fn identity_name(&self) -> &str {
        &self.identity_name
    }

    fn is_registered(&self) -> bool {
        self.registered
    }

    fn mark_registered(
        &mut self,
        published: &::service_common::ses_identities::SesIdentityRecord,
    ) {
        // The TXT verification token the registry holds for a domain identity is not
        // one this surface's model carries, so there is nothing of it to keep here.
        let _ = published;
        self.registered = true;
    }

    /// This API has no sending-authorization policy operations at all, so it reports
    /// none of the account's and takes none: a surface that cannot report a shared fact
    /// must not disturb it either. The Easy DKIM setup it DOES report, so a setup the
    /// account gained through another API over these identities after this record was
    /// written is taken here — its tokens are the records the identity's owner
    /// publishes. A record already carrying a token set keeps it: the first setup
    /// stands, because those records may already be in DNS.
    fn take_shared(
        &mut self,
        published: &::service_common::ses_identities::SesIdentityRecord,
    ) {
        let held_tokens = self
            .dkim
            .as_ref()
            .and_then(|dkim| dkim.tokens.as_ref())
            .is_some_and(|tokens| !tokens.is_empty());
        let gained = published.dkim.as_ref().filter(|_| !held_tokens);
        if let Some(shared) = gained {
            self.dkim = Some(DkimAttributes {
                signing_enabled: shared.signing_enabled,
                status: Some(service_dkim_status(shared.status).to_string()),
                tokens: Some(shared.tokens.clone()),
            });
        }
    }

    fn published(&self) -> ::service_common::ses_identities::SesIdentityRecord {
        registry_record(self)
    }

    fn adopt(
        published: &::service_common::ses_identities::SesIdentityRecord,
        now: SystemTime,
    ) -> Self {
        Self::adopted(published, now)
    }
}

/// How the account registry carries the identity `record`: the identity as it was
/// created, its class, the verification this service holds for it, and the Easy DKIM
/// setup it holds for it.
pub fn registry_record(
    record: &EmailIdentityRecord,
) -> ::service_common::ses_identities::SesIdentityRecord {
    use ::service_common::ses_identities::{
        SesDkimSetup, SesIdentityKind, SesIdentityRecord, SesVerificationState,
    };

    SesIdentityRecord {
        name: record.identity_name.clone(),
        kind: if record.identity_type == IDENTITY_TYPE_DOMAIN {
            SesIdentityKind::Domain
        } else {
            SesIdentityKind::EmailAddress
        },
        verification: if record.verified_for_sending {
            SesVerificationState::Success
        } else {
            SesVerificationState::Pending
        },
        verification_due: record.verification_due_at,
        // The TXT value a domain identity's owner publishes is the registry's own: no
        // operation of this surface reports it, and the registry mints it once so the
        // surface whose model carries one answers a stable value for the identity.
        verification_token: None,
        // The Easy DKIM setup belongs to the account's identity rather than to this
        // API, so the setup generated here — the tokens the owner publishes as CNAME
        // records, where the search for them stands, and whether messages from the
        // identity are signed — is published for every API over these identities to
        // report. An identity this API generated no token set for carries no setup to
        // share: one whose DKIM process nobody started.
        dkim: record
            .dkim
            .as_ref()
            .filter(|dkim| {
                dkim.tokens
                    .as_ref()
                    .is_some_and(|tokens| !tokens.is_empty())
            })
            .map(|dkim| SesDkimSetup {
                signing_enabled: dkim.signing_enabled,
                status: registry_dkim_state(dkim.status.as_deref()),
                tokens: dkim.tokens.clone().unwrap_or_default(),
            }),
        // This API has no sending-authorization policy operations, so it publishes no
        // set of them: the policies another API over these identities attached stay as
        // that API left them rather than being cleared by a surface that cannot see
        // them.
        policies: None,
    }
}

/// The registry's spelling of a stored `DkimStatus`.
///
/// This surface writes three of the model's values (`DKIM_STATUS_PENDING`,
/// `DKIM_STATUS_SUCCESS` and `DKIM_STATUS_NOT_STARTED`): a setup is waiting for the
/// records it told the caller to publish until they have been found, and an identity
/// nobody started the process for has none to find.
fn registry_dkim_state(
    status: Option<&str>,
) -> ::service_common::ses_identities::SesVerificationState {
    use ::service_common::ses_identities::SesVerificationState;

    match status {
        Some(DKIM_STATUS_SUCCESS) => SesVerificationState::Success,
        Some(DKIM_STATUS_NOT_STARTED) => SesVerificationState::NotStarted,
        _ => SesVerificationState::Pending,
    }
}

/// This surface's spelling of a registry Easy DKIM status, out of the model's
/// `DkimStatus` value set (`shared_types::DKIM_STATUS_VALUES`).
fn service_dkim_status(
    status: ::service_common::ses_identities::SesVerificationState,
) -> &'static str {
    use ::service_common::ses_identities::SesVerificationState;

    match status {
        SesVerificationState::Pending => DKIM_STATUS_PENDING,
        SesVerificationState::Success => DKIM_STATUS_SUCCESS,
        SesVerificationState::Failed => "FAILED",
        SesVerificationState::TemporaryFailure => "TEMPORARY_FAILURE",
        SesVerificationState::NotStarted => DKIM_STATUS_NOT_STARTED,
    }
}

impl ::service_common::ses_configuration_sets::AccountConfigurationSetRecord
    for ConfigurationSetRecord
{
    fn set_name(&self) -> &str {
        &self.name
    }

    fn is_registered(&self) -> bool {
        self.registered
    }

    fn mark_registered(&mut self) {
        self.registered = true;
    }

    /// The account's configuration for the set, onto the members this surface's own model
    /// spells them with. Everything this surface adds beside them — the set's tags and its
    /// event destinations — is untouched.
    ///
    /// Each of the four option structures exists exactly while the ACCOUNT's set carries a
    /// value this surface can report in it: a structure whose every member is the account's
    /// own is dropped where the account holds none, and the one member of a structure this
    /// surface has of its own (`DeliveryOptions$SendingPoolName`) is kept.
    fn take_shared(
        &mut self,
        published: &::service_common::ses_configuration_sets::SesConfigurationSetRecord,
    ) {
        self.tracking_options = published
            .custom_redirect_domain
            .clone()
            .map(|custom_redirect_domain| TrackingOptions {
                custom_redirect_domain,
            });
        // This surface spells the TLS policy with its own model's value set
        // (`shared_types::TLS_POLICY_VALUES`), which is the SES v2 spelling.
        let tls_policy = published.tls_policy.map(|policy| {
            match policy {
                ::service_common::ses_configuration_sets::SesTlsPolicy::Require => "REQUIRE",
                ::service_common::ses_configuration_sets::SesTlsPolicy::Optional => "OPTIONAL",
            }
            .to_string()
        });
        let sending_pool_name = self
            .delivery_options
            .as_ref()
            .and_then(|options| options.sending_pool_name.clone());
        self.delivery_options = (tls_policy.is_some() || sending_pool_name.is_some())
            .then_some(DeliveryOptions {
                tls_policy,
                sending_pool_name,
            });
        self.reputation_options = match self.reputation_options.take() {
            // A structure this surface already holds takes the account's value for the one
            // member both publish, and keeps the fresh start the account holds for the set.
            Some(held) => Some(ReputationOptions {
                reputation_metrics_enabled: published.reputation_metrics_enabled,
                last_fresh_start: published
                    .last_fresh_start
                    .map(::aws_smithy_types::DateTime::from)
                    .or(held.last_fresh_start),
            }),
            // One it holds none of is materialised only where the ACCOUNT holds something
            // this surface's own default does not say — metrics are on for a set nobody
            // configured, and such a set has had no fresh start — so a set created with no
            // options still reports no reputation options at all.
            None => (!published.reputation_metrics_enabled
                || published.last_fresh_start.is_some())
            .then_some(ReputationOptions {
                reputation_metrics_enabled: published.reputation_metrics_enabled,
                last_fresh_start: published
                    .last_fresh_start
                    .map(::aws_smithy_types::DateTime::from),
            }),
        };
        self.sending_options = match self.sending_options.take() {
            Some(_) => Some(SendingOptions {
                sending_enabled: published.sending_enabled,
            }),
            // The same rule: sending is on for a set nobody configured, so only a PAUSED
            // account set gives this surface something to report.
            None => (!published.sending_enabled).then_some(SendingOptions {
                sending_enabled: published.sending_enabled,
            }),
        };
    }

    /// How the registry carries this record: the set's name and the whole configuration both
    /// SES surfaces publish, read off the structures this surface holds it in. A structure
    /// this set carries none of is a set no caller configured that way, which this surface
    /// reports as its own model's default — reputation metrics and sending both on, as
    /// `CreateConfigurationSet` stores a set that states neither.
    fn published(
        &self,
    ) -> ::service_common::ses_configuration_sets::SesConfigurationSetRecord {
        ::service_common::ses_configuration_sets::SesConfigurationSetRecord {
            name: self.name.clone(),
            custom_redirect_domain: self
                .tracking_options
                .as_ref()
                .map(|options| options.custom_redirect_domain.clone()),
            tls_policy: self
                .delivery_options
                .as_ref()
                .and_then(|options| options.tls_policy.as_deref())
                .and_then(::service_common::ses_configuration_sets::SesTlsPolicy::of),
            reputation_metrics_enabled: self
                .reputation_options
                .as_ref()
                .is_none_or(|options| options.reputation_metrics_enabled),
            sending_enabled: self
                .sending_options
                .as_ref()
                .is_none_or(|options| options.sending_enabled),
            last_fresh_start: self
                .reputation_options
                .as_ref()
                .and_then(|options| options.last_fresh_start)
                .and_then(|instant| ::std::time::SystemTime::try_from(instant).ok()),
        }
    }

    /// This surface's record of a set the account holds that it does not: the set exists,
    /// carries the account's own configuration (applied by [`Self::take_shared`], which runs
    /// for every set both sides hold) and holds none of what this surface's own API adds,
    /// since none of it has been asked for.
    fn adopt(
        record: &::service_common::ses_configuration_sets::SesConfigurationSetRecord,
        _created_at: SystemTime,
    ) -> Self {
        Self {
            name: record.name.clone(),
            registered: true,
            ..Self::default()
        }
    }
}
