// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sns-amazonsimplenotificationservice service.
//!
//! The records here are the SNS control plane: topics, their subscriptions, and the
//! messages `Publish` delivered to each subscription. They are deliberately named
//! `*Record` so a handler cannot confuse them with the wire shapes of the same name
//! in [`crate::ops::types`] (`Topic`, `Subscription`).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{MessageAttributeValue, Subscription};

pub mod filter_policy;
/// The signature a delivered notification carries, and the certificate it verifies
/// against.
pub mod notification_signature;
/// Which ACCOUNT the ARNs a request carries address, and what this endpoint answers a
/// request addressing an account it does not serve.
pub mod account_resolution;
/// Whether IAM lets a signed request perform the action it asks for on the topic it
/// addresses: the caller's identity-based policies read together with the topic's own
/// access policy.
pub mod authorization;
/// The rules one MESSAGE ATTRIBUTE of a published message is held to, shared by every
/// operation that publishes one.
pub mod message_attributes;
/// The address-matching condition of a filter policy (`cidr`), which
/// [`filter_policy`] reads beside the conditions that compare values.
pub mod message_filter;
/// The rules ONE attribute of a topic is held to, shared by the operations that write
/// one.
pub mod topic_attributes;
/// The customer managed KMS key a topic encrypts its messages with, and whether a
/// message may be sealed under it — the step every operation that writes a message to a
/// topic takes before it accepts one.
pub mod topic_encryption;
/// The documented bounds of the retry policy a delivery policy document carries, held
/// against every write of one.
pub mod retry_policy_bounds;
/// What a topic configured with delivery-status feedback roles LOGS about each delivery
/// it attempted.
pub mod delivery_status;
/// Whether a delivery-status feedback role a write of a topic attribute names is one this
/// service could assume at all, asked before the attribute lands on the topic.
pub mod feedback_role;
/// Whether this account holds an origination entity SNS could send an SMS message from,
/// which every operation whose own work is to send one asks before it accepts.
pub mod sms_origination;
/// The `AWS/SNS` counters a topic publishes about its own fan-out: the publishes it
/// accepted, the messages its subscriptions' filter policies withheld, and the
/// notifications the subscribed endpoints took or refused.
pub mod topic_metrics;
mod message_structure;
/// The rules a topic's TAG set is held to, shared by the operations that write one.
mod tags;
/// What an ARN of this service names, and how a value that is not one is refused.
mod arn_references;
/// The order SNS reports one subscription's attributes in, and the map key that carries
/// it.
mod subscription_attribute_order;
/// The document a subscription's `DeliveryPolicy` is STORED as: the caller's own document
/// expanded to the whole shape, with the members it did not supply as the nulls they are.
mod stored_delivery_policy;
pub use arn_references::*;
pub use stored_delivery_policy::*;
pub use subscription_attribute_order::*;
pub use tags::*;
/// How a message published under a `MessageStructure` is read, and which text of it one
/// delivery protocol receives.
pub use message_structure::*;

/// The service principal Amazon SNS performs a DELIVERY as.
///
/// A subscription's endpoint is another service's resource, and SNS reaches it as a service
/// principal rather than as that resource's owner — "you must ensure that the Amazon SQS
/// queue policy allows the Amazon SNS topic to send messages to the queue", and the same
/// `lambda:InvokeFunction` grant is what lets a subscribed function be invoked. It is also
/// the principal a topic's delivery-status feedback role has to trust for SNS to assume it
/// ([`delivery_status`]).
pub const SERVICE_PRINCIPAL: &str = "sns.amazonaws.com";

/// The region an ARN carries when the request names none.
///
/// Every emulator writes `region = us-east-1` into the shared AWS config it hands a
/// client (`vendor/server/src/lib.rs`), so that is the region an unsigned request —
/// one carrying no SigV4 credential scope to read a region from — is addressing.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many items one page of a paginated SNS list holds.
///
/// SNS documents a page of "up to 100" for `ListTopics` and
/// `ListSubscriptionsByTopic`; the token is opaque to the caller either way.
pub const PAGE_SIZE: usize = 100;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores (topics, subscriptions).
///
/// This is NOT AWS's service quota — SNS publishes 100,000 topics and 12.5M
/// subscriptions per account, and nothing here claims to reproduce them. It is one of
/// the two axes of the emulator's own memory ceiling: every successful `CreateTopic`
/// and `Subscribe` inserts a record that lives until a caller deletes it, so without a
/// ceiling a caller could grow the process until the host runs out of memory. A
/// request that would cross it is refused with `TopicLimitExceededException` /
/// `SubscriptionLimitExceededException`, the modelled errors AWS itself uses for
/// "the customer already owns the maximum allowed number", so the refusal is one a
/// real client already handles.
///
/// It bounds the record COUNT; [`MAX_ATTRIBUTE_BYTES_PER_RECORD`] and
/// [`MAX_TAG_BYTES_PER_TOPIC`] bound the bytes ONE record can accumulate, which is
/// the other axis — a record whose attribute map grew without limit would defeat this
/// count on its own.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The attribute names SNS defines for a topic, from the `CreateTopic` /
/// `SetTopicAttributes` / `GetTopicAttributes` docs (the delivery-status logging
/// attributes come from `SetTopicAttributes`). SNS refuses any other name, and so
/// does this emulator: the closed set is also what stops a caller inventing fresh
/// attribute names to grow one topic record for ever.
pub const TOPIC_ATTRIBUTE_NAMES: &[&str] = &[
    "ApplicationFailureFeedbackRoleArn",
    "ApplicationSuccessFeedbackRoleArn",
    "ApplicationSuccessFeedbackSampleRate",
    "ArchivePolicy",
    "ContentBasedDeduplication",
    "DeliveryPolicy",
    "DisplayName",
    "FifoThroughputScope",
    "FifoTopic",
    "FirehoseFailureFeedbackRoleArn",
    "FirehoseSuccessFeedbackRoleArn",
    "FirehoseSuccessFeedbackSampleRate",
    "HTTPFailureFeedbackRoleArn",
    "HTTPSuccessFeedbackRoleArn",
    "HTTPSuccessFeedbackSampleRate",
    "KmsMasterKeyId",
    "LambdaFailureFeedbackRoleArn",
    "LambdaSuccessFeedbackRoleArn",
    "LambdaSuccessFeedbackSampleRate",
    "MaximumMessageSize",
    "Policy",
    "SQSFailureFeedbackRoleArn",
    "SQSSuccessFeedbackRoleArn",
    "SQSSuccessFeedbackSampleRate",
    "SignatureVersion",
    "TracingConfig",
];

/// What `Subscribe` answers in place of the ARN of a subscription that is not
/// confirmed yet: "the string \"pending confirmation\" if the subscription requires
/// confirmation" (`SubscribeResponse$SubscriptionArn` doc), unless
/// `ReturnSubscriptionArn` asks for the ARN in all cases.
pub const PENDING_CONFIRMATION_REPORTED: &str = "pending confirmation";

/// What a subscription LISTING carries in `Subscription$SubscriptionArn` while the
/// subscription is not confirmed yet.
///
/// "A subscription becomes confirmed when the subscriber calls the
/// ConfirmSubscription action" (`SubscribeInput$ReturnSubscriptionArn` doc), and until
/// then SNS publishes no ARN for it: a listing carries this sentinel in the ARN's
/// place. It is the same state as [`PENDING_CONFIRMATION_REPORTED`] but not the same
/// spelling — `Subscribe`'s own doc fixes the wording of its response, and the
/// recorded wire responses of `ListSubscriptionsByTopic` fix this one.
pub const PENDING_CONFIRMATION_LISTED: &str = "PendingConfirmation";

/// Whether `value` is one of the sentinels SNS publishes in place of an unconfirmed
/// subscription's ARN ([`PENDING_CONFIRMATION_REPORTED`],
/// [`PENDING_CONFIRMATION_LISTED`]) rather than an ARN — so a caller that read one out
/// of a `Subscribe` or a listing response and handed it back names a subscription that
/// is still pending, not a subscription that does not exist.
pub fn is_pending_confirmation_sentinel(value: &str) -> bool {
    value == PENDING_CONFIRMATION_REPORTED || value == PENDING_CONFIRMATION_LISTED
}

/// How many hexadecimal characters a confirmation token this service mints carries —
/// one SHA-256 digest's worth, which is the most
/// [`ComAmazonawsSnsAmazonsimplenotificationserviceState::next_confirmation_token`]
/// encodes.
///
/// `smithy_query(query="shape", shape="token")` reports a plain string with no length
/// or pattern, and `ConfirmSubscriptionInput$Token` documents only "Short-lived token
/// sent to an endpoint during the Subscribe action", so the token is opaque: nothing
/// but equality against the minted value is read off it, and this width only has to
/// make a guess hopeless.
pub const CONFIRMATION_TOKEN_HEX_CHARS: usize = 64;

/// The sentence a `ConfirmSubscription` carrying a token that is not the one sent to
/// one of the topic's awaiting endpoints is refused with.
///
/// WHOLLY FIXED text: the token is opaque and secret, so the sentence reproduces
/// NEITHER the token the caller sent nor the one that was sent to the endpoint, and it
/// carries no member frame either — a token that confirms nothing is not a member whose
/// value broke a constraint, so SNS names the condition itself rather than the member it
/// arrived in. Recorded evidence: `ConfirmSubscription` with a token that is not a
/// confirmation token of the topic answers `400 InvalidParameter` with exactly this
/// sentence.
pub const INVALID_CONFIRMATION_TOKEN_REFUSAL: &str = "Invalid token";

/// The attribute names SNS defines for a subscription, from the `Subscribe` /
/// `SetSubscriptionAttributes` docs. Closed for the same two reasons as
/// [`TOPIC_ATTRIBUTE_NAMES`].
pub const SUBSCRIPTION_ATTRIBUTE_NAMES: &[&str] = &[
    "DeliveryPolicy",
    "FilterPolicy",
    "FilterPolicyScope",
    "RawMessageDelivery",
    "RedrivePolicy",
    "ReplayPolicy",
    "SubscriptionRoleArn",
];

/// The total attribute bytes (names plus values) ONE topic or subscription record may
/// hold.
///
/// SNS's own documented attribute values are policy documents of at most 30 KB, so a
/// record of the closed attribute set has no legitimate need of more than this; the
/// ceiling is what stops a caller growing a single record without limit by repeating
/// a create or subscribe with ever larger values. A request that would cross it is
/// refused with `InvalidParameterException`.
pub const MAX_ATTRIBUTE_BYTES_PER_RECORD: usize = 64 * 1024;

/// The total tag bytes (keys plus values) one topic may hold — the same ceiling as
/// [`MAX_ATTRIBUTE_BYTES_PER_RECORD`] applied to the other caller-sized map on a
/// topic record, whose entry COUNT is capped at 50 by SNS's own documented tag limit.
pub const MAX_TAG_BYTES_PER_TOPIC: usize = 8 * 1024;

/// "Length Constraints: Maximum length of 30,720"
/// (`CreateTopicInput$DataProtectionPolicy` doc).
pub const MAX_DATA_PROTECTION_POLICY_LEN: usize = 30_720;

/// The message a `DataProtectionPolicy` past [`MAX_DATA_PROTECTION_POLICY_LEN`] is
/// refused with, or `None` when the document fits.
///
/// Held by every operation that WRITES a topic's inline data protection policy, so the
/// one documented bound is the same whichever call carried the document. The sentence
/// names the member, the bound and the length sent, and never reproduces the document.
///
/// The length is counted in CHARACTERS, not bytes: "Maximum length of 30,720" is the
/// constraint on a string member, which Smithy measures in Unicode code points, so a
/// document of non-ASCII characters is held to the same count as an ASCII one.
pub fn data_protection_policy_refusal(policy: &str) -> Option<String> {
    let length = policy.chars().count();
    (length > MAX_DATA_PROTECTION_POLICY_LEN).then(|| {
        format!(
            "Invalid parameter: DataProtectionPolicy - at most \
             {MAX_DATA_PROTECTION_POLICY_LEN} characters: {length}"
        )
    })
}

/// The bytes an attribute (or tag) map costs: every name and every value, since a
/// caller supplies both.
pub fn attribute_bytes(attributes: &HashMap<String, String>) -> usize {
    attributes
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum()
}

/// The refusal reason for an attribute map that would cross
/// [`MAX_ATTRIBUTE_BYTES_PER_RECORD`], or `None` when it fits.
///
/// `holder` names the record the map belongs to ("a topic", "a subscription"), so ONE
/// ceiling check serves every record this service stores and each caller lifts the
/// reason into its own modelled error.
pub fn attribute_bytes_refusal(
    holder: &str,
    attributes: &HashMap<String, String>,
) -> Option<String> {
    let held = attribute_bytes(attributes);
    (held > MAX_ATTRIBUTE_BYTES_PER_RECORD).then(|| {
        format!(
            "Invalid parameter: Attributes - {holder} holds at most \
             {MAX_ATTRIBUTE_BYTES_PER_RECORD} bytes of Attributes: {held}"
        )
    })
}

/// The same check for a write of ONE attribute: measured on the map the write WOULD
/// produce (`value` of `None` clears the attribute), so a caller cannot grow one
/// record past the ceiling by repeating the call with ever larger values.
pub fn attribute_write_refusal(
    holder: &str,
    attributes: &HashMap<String, String>,
    name: &str,
    value: Option<&str>,
) -> Option<String> {
    let mut would_hold = attributes.clone();
    match value {
        Some(value) => {
            would_hold.insert(name.to_string(), value.to_string());
        }
        None => {
            would_hold.remove(name);
        }
    }
    attribute_bytes_refusal(holder, &would_hold)
}

/// The name a refusal gives a topic record.
pub const TOPIC_RECORD: &str = "a topic";

/// The name a refusal gives a subscription record.
pub const SUBSCRIPTION_RECORD: &str = "a subscription";

/// Whether `name` is an attribute SNS defines for a topic.
pub fn is_topic_attribute(name: &str) -> bool {
    TOPIC_ATTRIBUTE_NAMES.contains(&name)
}

/// Whether `name` is an attribute SNS defines for a subscription.
pub fn is_subscription_attribute(name: &str) -> bool {
    SUBSCRIPTION_ATTRIBUTE_NAMES.contains(&name)
}

/// How many delivered messages [`ComAmazonawsSnsAmazonsimplenotificationserviceState::delivered`]
/// retains per subscription before dropping the oldest.
///
/// The delivery log is an emulator-side record of what `Publish` fanned out, not a
/// queue any operation drains, so it is bounded rather than kept: a caller that
/// publishes forever sees the most recent messages and no unbounded growth.
pub const MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION: usize = 100;

/// The total bytes of message payload the delivery log retains across every
/// subscription, oldest evicted first.
///
/// This is what bounds the PRODUCT a publish multiplies out — message size x
/// confirmed subscriptions x publish count — which the per-subscription count alone
/// does not: 8 MiB of retained payload holds 32 messages of the documented 256 KiB
/// maximum, however many subscriptions or publishes produced them.
pub const MAX_RETAINED_DELIVERY_BYTES: usize = 8 * 1024 * 1024;

/// How many messages ONE topic's message archive retains before dropping the oldest.
///
/// This is NOT AWS's archive, which holds every message published inside the retention
/// period its policy states: it is the emulator's own memory ceiling on a store a caller
/// grows by publishing, the archive counterpart of
/// [`MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION`]. A replay therefore redelivers the most
/// recent messages of the window it names rather than an unbounded history.
pub const MAX_ARCHIVED_MESSAGES_PER_TOPIC: usize = 100;

/// The total bytes of message payload the archives retain across every topic, oldest
/// message of the fullest archive evicted first — the archive counterpart of
/// [`MAX_RETAINED_DELIVERY_BYTES`], which bounds the (payload x publishes x topics)
/// product a caller can make the store hold.
pub const MAX_ARCHIVED_BYTES: usize = 8 * 1024 * 1024;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with;
/// `store` names the records it holds, e.g. `"topics"`.
pub fn limit_exceeded_message(store: &str) -> String {
    format!(
        "The customer already owns the maximum allowed number of {store}: \
         this emulator holds at most {MAX_RECORDS_PER_STORE}"
    )
}

/// One topic, as `CreateTopic` minted it.
#[derive(Debug, Clone)]
pub struct TopicRecord {
    pub arn: String,
    pub name: String,
    /// The account id that owns the topic.
    pub owner: String,
    /// The caller-settable attributes (`DisplayName`, `DeliveryPolicy`, `Policy`, …).
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub data_protection_policy: Option<String>,
    /// A FIFO topic: its name ends with `.fifo`.
    pub is_fifo: bool,
    pub created_at: SystemTime,
    /// How many of this topic's subscriptions have been deleted LONG ENOUGH AGO for the
    /// count to have settled — the part of the `SubscriptionsDeleted` attribute that is
    /// already reported. It outlives the subscription records.
    pub subscriptions_deleted: u64,
    /// The deletions this topic's `SubscriptionsDeleted` count has not taken up yet, each
    /// held as the instant it is DUE to: SNS does not advance the deleted count as part of
    /// the `Unsubscribe` call, so a read taken straight after one still reports the count
    /// from before it ([`SUBSCRIPTION_DELETION_SETTLES_AFTER`]).
    pub subscriptions_deleting: Vec<SystemTime>,
    /// When this topic's message archive came into force: the instant the
    /// [`ARCHIVE_POLICY_ATTRIBUTE`] was last added to a topic that was not archiving,
    /// and `None` while it holds no archive policy. Nothing older than it is in the
    /// archive, so it is one half of the replayable window
    /// ([`TopicRecord::beginning_archive_time`]); kept in step with the attribute map
    /// by [`TopicRecord::refresh_archive_window`].
    pub archive_enabled_at: Option<SystemTime>,
}

// =============================================================================
// Topic attributes SNS gives a meaning of its own
// =============================================================================

/// The topic attribute a caller-set access control policy lives under.
pub const POLICY_ATTRIBUTE: &str = "Policy";

/// The topic attribute a caller-set delivery policy lives under.
pub const DELIVERY_POLICY_ATTRIBUTE: &str = "DeliveryPolicy";

/// The topic attribute a caller-set display name lives under.
pub const DISPLAY_NAME_ATTRIBUTE: &str = "DisplayName";

/// The topic attribute the server-side-encryption key id lives under:
/// "The following attribute applies only to server-side-encryption: KmsMasterKeyId"
/// (`SetTopicAttributesInput$AttributeName` doc).
pub const KMS_MASTER_KEY_ID_ATTRIBUTE: &str = "KmsMasterKeyId";

/// The attribute that asks `CreateTopic` for a FIFO topic. It is fixed for the life of
/// the topic: a topic's kind is decided when it is created, so `SetTopicAttributes`
/// refuses it.
pub const FIFO_TOPIC_ATTRIBUTE: &str = "FifoTopic";

/// The attributes SNS COMPUTES for a topic rather than storing: the account that owns
/// it, its ARN, its subscription counts and the delivery policy it is effectively run
/// under. None of them is settable, so none is in [`TOPIC_ATTRIBUTE_NAMES`].
pub const OWNER_ATTRIBUTE: &str = "Owner";
pub const TOPIC_ARN_ATTRIBUTE: &str = "TopicArn";
pub const SUBSCRIPTIONS_PENDING_ATTRIBUTE: &str = "SubscriptionsPending";
pub const SUBSCRIPTIONS_CONFIRMED_ATTRIBUTE: &str = "SubscriptionsConfirmed";
pub const SUBSCRIPTIONS_DELETED_ATTRIBUTE: &str = "SubscriptionsDeleted";
pub const EFFECTIVE_DELIVERY_POLICY_ATTRIBUTE: &str = "EffectiveDeliveryPolicy";

/// How long after an `Unsubscribe` the deletion it performed is taken up by the topic's
/// [`SUBSCRIPTIONS_DELETED_ATTRIBUTE`] count.
///
/// The deleted count is NOT part of the unsubscribe: SNS removes the subscription
/// synchronously — it stops being listed and stops being delivered to at once, and the
/// confirmed and pending counts follow it immediately — while the per-topic DELETED count
/// is taken up on a cycle of the service's own. Recorded evidence: a client that
/// unsubscribed the only subscription of a topic and then polled `GetTopicAttributes`
/// until the confirmed and pending counts had settled still read `0` deleted on every one
/// of those reads.
///
/// The span is this emulator's own — AWS documents no period for it — and is chosen
/// longer than such a convergence loop runs for, which is what the recording requires: a
/// span of a few seconds was measured against this same recorded sequence and the count
/// had already moved to 1 by the read the recording shows as 0, because the client's own
/// polling spans more than that. A count that moves inside a convergence loop reports a
/// number no real client reading those three counters together ever sees.
pub const SUBSCRIPTION_DELETION_SETTLES_AFTER: Duration = Duration::from_secs(15 * 60);

/// How many not-yet-counted deletions ONE topic holds the instants of.
///
/// This is NOT an AWS quota: it is this emulator's own memory ceiling on a store a caller
/// grows by unsubscribing, the deletion counterpart of [`MAX_RECORDS_PER_STORE`]. Each
/// `Unsubscribe` of a topic adds one instant until that instant arrives, so without a
/// ceiling a caller subscribing and unsubscribing in a loop would grow one topic record
/// for the whole span. Crossing it takes the OLDEST pending deletions up early — they are
/// the next to settle anyway — so the count a topic reports is never lost, only reported
/// sooner.
pub const MAX_SETTLING_DELETIONS_PER_TOPIC: usize = 64;

/// The topic attribute a FIFO topic's message archive is configured under:
/// "ArchivePolicy – The policy that sets the retention period for messages stored in the
/// message archive of an Amazon SNS FIFO topic"
/// (`GetTopicAttributesResponse$Attributes` doc).
pub const ARCHIVE_POLICY_ATTRIBUTE: &str = "ArchivePolicy";

/// The member of an archive policy that states how long the archive keeps a message, in
/// DAYS — the "message retention period set by the topic's message archiving policy" the
/// replayable window is measured by (`GetTopicAttributesResponse$Attributes` doc).
pub const MESSAGE_RETENTION_PERIOD_MEMBER: &str = "MessageRetentionPeriod";

/// The attribute SNS reports the start of a topic's replayable window under:
/// "BeginningArchiveTime – The earliest starting point at which a message in the topic's
/// archive can be replayed from" (`GetTopicAttributesResponse$Attributes` doc). It is
/// COMPUTED from the archive policy and the clock, so it is not in
/// [`TOPIC_ATTRIBUTE_NAMES`]: no caller sets it.
pub const BEGINNING_ARCHIVE_TIME_ATTRIBUTE: &str = "BeginningArchiveTime";

/// The order SNS reports a topic's attributes in, as recorded `GetTopicAttributes`
/// responses carry it: the access policy, the owner, the pending-subscription count,
/// the encryption key id of an encrypted topic, the ARN, the effective delivery
/// policy, the confirmed count, the display name, the deleted count. Every other
/// attribute a topic holds follows these, ordered by name.
///
/// SNS answers the same attribute set with the same entry order every time, so a
/// client that reads the map positionally, or diffs two reads of one topic, sees one
/// stable document — which is why the attribute map is keyed by
/// [`TopicAttributeName`], whose ordering IS this order, rather than by a `String` in
/// a `HashMap` (whose entries come out in a different order on every call).
pub const REPORTED_TOPIC_ATTRIBUTE_ORDER: &[&str] = &[
    POLICY_ATTRIBUTE,
    OWNER_ATTRIBUTE,
    SUBSCRIPTIONS_PENDING_ATTRIBUTE,
    KMS_MASTER_KEY_ID_ATTRIBUTE,
    TOPIC_ARN_ATTRIBUTE,
    EFFECTIVE_DELIVERY_POLICY_ATTRIBUTE,
    SUBSCRIPTIONS_CONFIRMED_ATTRIBUTE,
    DISPLAY_NAME_ATTRIBUTE,
    SUBSCRIPTIONS_DELETED_ATTRIBUTE,
];

/// A topic attribute name as the key of a reported attribute map: a string on the
/// wire, ordered by [`REPORTED_TOPIC_ATTRIBUTE_ORDER`] rather than alphabetically, so
/// a `BTreeMap` keyed by it serializes its entries in the order SNS reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicAttributeName(String);

impl TopicAttributeName {
    /// Where SNS reports this attribute: its place in
    /// [`REPORTED_TOPIC_ATTRIBUTE_ORDER`], or past the end for a name that order does
    /// not list (which then sorts by name).
    fn reported_position(&self) -> usize {
        REPORTED_TOPIC_ATTRIBUTE_ORDER
            .iter()
            .position(|name| *name == self.0)
            .unwrap_or(REPORTED_TOPIC_ATTRIBUTE_ORDER.len())
    }
}

impl Ord for TopicAttributeName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.reported_position()
            .cmp(&other.reported_position())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for TopicAttributeName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<str> for TopicAttributeName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TopicAttributeName {
    fn from(name: &str) -> Self {
        Self(name.to_string())
    }
}

impl From<String> for TopicAttributeName {
    fn from(name: String) -> Self {
        Self(name)
    }
}

impl std::str::FromStr for TopicAttributeName {
    type Err = std::convert::Infallible;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(Self(name.to_string()))
    }
}

/// The maximum-message-size bounds SNS documents: "Valid values are 1024 to 1048576
/// (1 MiB). The default is 262144 (256 KiB)"
/// (`SetTopicAttributesInput$AttributeName` doc).
pub const MAXIMUM_MESSAGE_SIZE_ATTRIBUTE: &str = "MaximumMessageSize";
pub const MIN_MAXIMUM_MESSAGE_SIZE: u64 = 1_024;
pub const MAX_MAXIMUM_MESSAGE_SIZE: u64 = 1_048_576;

// =============================================================================
// The topic access policy document
// =============================================================================

/// One statement of a topic's access control policy.
///
/// The document is MODELLED rather than carried as opaque JSON for two reasons:
/// `AddPermission` and `RemovePermission` edit it statement by statement, and the
/// members of a document SNS itself produces come back in SNS's own order (`Sid`,
/// `Effect`, `Principal`, `Action`, `Resource`, `Condition`), which a
/// [`serde_json::Value`] object — sorted by key — cannot reproduce. Members this
/// emulator gives no meaning to (`NotAction`, `NotPrincipal`, …) are kept in `other`
/// so editing a caller's policy never drops part of it, and every member but `Effect`
/// is optional: `Effect` is the one member a policy statement cannot be read without.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TopicPolicyStatement {
    #[serde(rename = "Sid", skip_serializing_if = "Option::is_none", default)]
    pub sid: Option<String>,
    #[serde(rename = "Effect")]
    pub effect: String,
    #[serde(rename = "Principal", skip_serializing_if = "Option::is_none", default)]
    pub principal: Option<serde_json::Value>,
    #[serde(rename = "Action", skip_serializing_if = "Option::is_none", default)]
    pub action: Option<serde_json::Value>,
    #[serde(rename = "Resource", skip_serializing_if = "Option::is_none", default)]
    pub resource: Option<serde_json::Value>,
    #[serde(rename = "Condition", skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<serde_json::Value>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

/// A topic's access control policy: the JSON document the `Policy` attribute carries.
///
/// Top-level members this emulator gives no meaning to are kept in `other`, so editing
/// a caller's document (a grant, a revoke) rewrites it without dropping part of it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TopicPolicyDocument {
    #[serde(rename = "Version", skip_serializing_if = "Option::is_none", default)]
    pub version: Option<String>,
    #[serde(rename = "Id", skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    #[serde(rename = "Statement", deserialize_with = "statements")]
    pub statement: Vec<TopicPolicyStatement>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Read a document's `Statement`: ONE statement or a list of them, both of which the
/// policy grammar accepts. It is written back as a list, the form SNS answers with.
fn statements<'de, D>(deserializer: D) -> Result<Vec<TopicPolicyStatement>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        // Boxed: one statement is a far larger value than the vector of them, and an
        // enum is as big as its largest variant.
        One(Box<TopicPolicyStatement>),
        Many(Vec<TopicPolicyStatement>),
    }

    Ok(match <OneOrMany as serde::Deserialize>::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![*one],
        OneOrMany::Many(many) => many,
    })
}

impl TopicPolicyDocument {
    /// This document as the `Policy` attribute carries it.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The statement labelled `label`, if the document holds one. A statement's `Sid`
    /// IS the `Label` `AddPermission` gave it, which is what makes a label removable.
    pub fn position_of(&self, label: &str) -> Option<usize> {
        self.statement
            .iter()
            .position(|statement| statement.sid.as_deref() == Some(label))
    }
}

/// One value as a bare string, several as an array — the shape SNS writes a policy
/// statement's `Principal` and `Action` in.
pub fn one_or_many(values: Vec<String>) -> serde_json::Value {
    match <[String; 1]>::try_from(values) {
        Ok([only]) => serde_json::Value::String(only),
        Err(values) => serde_json::Value::Array(
            values.into_iter().map(serde_json::Value::String).collect(),
        ),
    }
}

/// The access policy of a topic whose owner has set none: the one SNS generates, which
/// allows the topic's own account every topic action.
pub fn default_topic_policy(arn: &str, owner: &str) -> TopicPolicyDocument {
    TopicPolicyDocument {
        version: Some("2008-10-17".to_string()),
        id: Some("__default_policy_ID".to_string()),
        statement: vec![TopicPolicyStatement {
            sid: Some("__default_statement_ID".to_string()),
            effect: "Allow".to_string(),
            principal: Some(serde_json::json!({ "AWS": "*" })),
            action: Some(serde_json::json!([
                "SNS:GetTopicAttributes",
                "SNS:SetTopicAttributes",
                "SNS:AddPermission",
                "SNS:RemovePermission",
                "SNS:DeleteTopic",
                "SNS:Subscribe",
                "SNS:ListSubscriptionsByTopic",
                "SNS:Publish"
            ])),
            resource: Some(serde_json::Value::String(arn.to_string())),
            condition: Some(serde_json::json!({
                "StringEquals": { "AWS:SourceOwner": owner }
            })),
            other: std::collections::BTreeMap::new(),
        }],
        other: std::collections::BTreeMap::new(),
    }
}

/// Every action name Amazon SNS defines, which is the SERVICE SCOPE a policy statement
/// this service writes may name: "Valid values: Any Amazon SNS action name, for example
/// `Publish`" (`AddPermissionInput$ActionName` doc). The list is the operations this
/// service's model binds, as
/// `smithy_query(query="list_operations")` reports them.
pub const ACTION_NAMES: &[&str] = &[
    "AddPermission",
    "CheckIfPhoneNumberIsOptedOut",
    "ConfirmSubscription",
    "CreatePlatformApplication",
    "CreatePlatformEndpoint",
    "CreateSMSSandboxPhoneNumber",
    "CreateTopic",
    "DeleteEndpoint",
    "DeletePlatformApplication",
    "DeleteSMSSandboxPhoneNumber",
    "DeleteTopic",
    "GetDataProtectionPolicy",
    "GetEndpointAttributes",
    "GetPlatformApplicationAttributes",
    "GetSMSAttributes",
    "GetSMSSandboxAccountStatus",
    "GetSubscriptionAttributes",
    "GetTopicAttributes",
    "ListEndpointsByPlatformApplication",
    "ListOriginationNumbers",
    "ListPhoneNumbersOptedOut",
    "ListPlatformApplications",
    "ListSMSSandboxPhoneNumbers",
    "ListSubscriptions",
    "ListSubscriptionsByTopic",
    "ListTagsForResource",
    "ListTopics",
    "OptInPhoneNumber",
    "Publish",
    "PublishBatch",
    "PutDataProtectionPolicy",
    "RemovePermission",
    "SetEndpointAttributes",
    "SetPlatformApplicationAttributes",
    "SetSMSAttributes",
    "SetSubscriptionAttributes",
    "SetTopicAttributes",
    "Subscribe",
    "TagResource",
    "Unsubscribe",
    "UntagResource",
    "VerifySMSSandboxPhoneNumber",
];

/// The sentence SNS refuses a policy statement naming an action that is not one of its
/// own with: the statement's action lies outside the service the policy governs, and
/// which value it was is not what the sentence carries. Recorded evidence:
/// `AddPermission` with an `ActionName` that is no Amazon SNS action answers `400
/// InvalidParameter` with exactly this.
pub const ACTION_OUT_OF_SCOPE_REFUSAL: &str =
    "Invalid parameter: Policy statement action out of service scope!";

/// The sentence SNS refuses a grant under a `Label` the topic's policy already carries a
/// DIFFERENT statement for with: a label identifies ONE statement, so a second grant under
/// it would have to replace that statement, which this call does not do. Recorded evidence:
/// a second `AddPermission` of one label answers `400 InvalidParameter` with exactly this.
///
/// A repeat that would add the statement the policy ALREADY carries, identically, asks for
/// the state the topic is already in and is served instead
/// ([`TopicPolicyDocument::position_of`] and the grant comparison in the handler).
pub const STATEMENT_ALREADY_EXISTS_REFUSAL: &str = "Invalid parameter: Statement already exists";

/// Whether `name` is an action Amazon SNS defines ([`ACTION_NAMES`]) — the question a
/// policy statement's action is judged by, whichever call carried it.
pub fn is_action_name(name: &str) -> bool {
    ACTION_NAMES.contains(&name)
}

/// The statement `AddPermission` adds: an `Allow` of `SNS:<action>` for each named
/// action, to the root principal of each named account, on this topic alone.
///
/// An account id is written as its root user's ARN, the principal form SNS stores a
/// delegated account as.
pub fn permission_statement(
    label: &str,
    account_ids: &[String],
    action_names: &[String],
    topic_arn: &str,
) -> TopicPolicyStatement {
    TopicPolicyStatement {
        sid: Some(label.to_string()),
        effect: "Allow".to_string(),
        principal: Some(serde_json::json!({
            "AWS": one_or_many(
                account_ids
                    .iter()
                    .map(|account| root_principal_arn(account))
                    .collect(),
            )
        })),
        action: Some(one_or_many(
            action_names
                .iter()
                .map(|action| format!("SNS:{action}"))
                .collect(),
        )),
        resource: Some(serde_json::Value::String(topic_arn.to_string())),
        condition: None,
        other: std::collections::BTreeMap::new(),
    }
}

impl TopicRecord {
    /// What [`SUBSCRIPTIONS_DELETED_ATTRIBUTE`] reports for this topic now: the deletions
    /// that have settled, which is every one the clock has made due.
    ///
    /// COMPUTED from the record and the clock rather than stored, so reading it moves
    /// nothing: a deletion becomes part of the count when its instant arrives, whether or
    /// not anybody reads the topic in between.
    pub fn reported_subscriptions_deleted(&self) -> u64 {
        let now = SystemTime::now();
        let settled = self
            .subscriptions_deleting
            .iter()
            .filter(|due| **due <= now)
            .count();
        self.subscriptions_deleted
            .saturating_add(settled.try_into().unwrap_or(u64::MAX))
    }

    /// Record one more deleted subscription, due to be counted once
    /// [`SUBSCRIPTION_DELETION_SETTLES_AFTER`] has passed.
    ///
    /// The deletions whose instant has already arrived are folded into the settled count
    /// as they are passed, so the pending list holds only the deletions of the last span
    /// rather than growing with every `Unsubscribe` the topic ever sees — and no more than
    /// [`MAX_SETTLING_DELETIONS_PER_TOPIC`] of those, past which the oldest are taken up
    /// early. The total is never lost either way: a deletion only ever reaches the count
    /// sooner.
    pub fn count_deleted_subscription(&mut self) {
        let now = SystemTime::now();
        let settled = self
            .subscriptions_deleting
            .iter()
            .filter(|due| **due <= now)
            .count();
        self.subscriptions_deleted = self
            .subscriptions_deleted
            .saturating_add(settled.try_into().unwrap_or(u64::MAX));
        self.subscriptions_deleting.retain(|due| *due > now);
        self.subscriptions_deleting
            .push(now + SUBSCRIPTION_DELETION_SETTLES_AFTER);
        // The list is a caller-grown store — one entry per `Unsubscribe` of this topic —
        // so it is bounded here as well as pruned above: the entries past the ceiling are
        // the oldest, which are the next to settle anyway.
        let over = self
            .subscriptions_deleting
            .len()
            .saturating_sub(MAX_SETTLING_DELETIONS_PER_TOPIC);
        if over > 0 {
            self.subscriptions_deleting.drain(..over);
            self.subscriptions_deleted = self
                .subscriptions_deleted
                .saturating_add(over.try_into().unwrap_or(u64::MAX));
        }
    }

    /// This topic's access control policy document: the one its owner set, or the
    /// default SNS generates for a topic with none. `None` when the stored value is not
    /// a policy document at all, which the write paths refuse to create.
    pub fn policy_document(&self) -> Option<TopicPolicyDocument> {
        match self.attributes.get(POLICY_ATTRIBUTE) {
            Some(stored) => serde_json::from_str(stored).ok(),
            None => Some(default_topic_policy(&self.arn, &self.owner)),
        }
    }

    /// The `Policy` attribute value this topic reports: what its owner set, verbatim,
    /// or the default document.
    pub fn policy_attribute(&self) -> String {
        match self.attributes.get(POLICY_ATTRIBUTE) {
            Some(stored) => stored.clone(),
            None => default_topic_policy(&self.arn, &self.owner).to_json(),
        }
    }

    /// Write `policy` back as this topic's access control policy.
    pub fn store_policy(&mut self, policy: &TopicPolicyDocument) {
        self.attributes
            .insert(POLICY_ATTRIBUTE.to_string(), policy.to_json());
    }

    /// Whether this topic archives the messages published to it: it holds an
    /// [`ARCHIVE_POLICY_ATTRIBUTE`].
    pub fn is_archiving(&self) -> bool {
        self.attributes.contains_key(ARCHIVE_POLICY_ATTRIBUTE)
    }

    /// Bring [`TopicRecord::archive_enabled_at`] in step with the attribute map, after a
    /// write that may have added or removed the archive policy.
    ///
    /// Called by every operation that writes a topic's attributes, so the instant the
    /// archive came into force does not depend on which call configured it. A write that
    /// leaves an already-archiving topic archiving keeps the instant it has: the archive
    /// never stopped, so its window did not restart.
    pub fn refresh_archive_window(&mut self) {
        match (self.is_archiving(), self.archive_enabled_at) {
            (true, None) => self.archive_enabled_at = Some(SystemTime::now()),
            (false, Some(_)) => self.archive_enabled_at = None,
            _ => {}
        }
    }

    /// How long this topic's archive keeps a message, or `None` when it archives nothing
    /// or its policy states no retention period.
    pub fn archive_retention(&self) -> Option<Duration> {
        let days = json_object(self.attributes.get(ARCHIVE_POLICY_ATTRIBUTE)?)?
            .get(MESSAGE_RETENTION_PERIOD_MEMBER)
            .and_then(retention_days)?;
        Some(ONE_DAY * days)
    }

    /// The earliest instant a message in this topic's archive can be replayed from, or
    /// `None` for a topic that archives nothing.
    ///
    /// "This point in time is based on the configured message retention period set by the
    /// topic's message archiving policy" (`GetTopicAttributesResponse$Attributes` doc):
    /// the archive holds nothing published before archiving came into force, and nothing
    /// older than the retention period, so the window starts at the LATER of the two.
    pub fn beginning_archive_time(&self) -> Option<SystemTime> {
        let enabled_at = self.archive_enabled_at?;
        let retained_from = SystemTime::now().checked_sub(self.archive_retention()?);
        Some(match retained_from {
            Some(horizon) if horizon > enabled_at => horizon,
            _ => enabled_at,
        })
    }
}

/// One day, the unit an archive policy's retention period is stated in.
const ONE_DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The number of days an archive policy's retention member states, or `None` when the
/// member is not a count of days at all.
///
/// SNS writes the member as a STRING (`{"MessageRetentionPeriod":"30"}`), which is the
/// form a caller's document carries; a JSON number says the same thing and is read too.
fn retention_days(value: &serde_json::Value) -> Option<u32> {
    match value {
        serde_json::Value::String(text) => text.parse().ok(),
        serde_json::Value::Number(number) => {
            number.as_u64().and_then(|days| u32::try_from(days).ok())
        }
        _ => None,
    }
}

/// `at` as an RFC 3339 instant, the form this service writes every timestamp it reports
/// in a string-valued field (a notification envelope's `Timestamp`, a topic's
/// [`BEGINNING_ARCHIVE_TIME_ATTRIBUTE`]), or `None` for an instant the timestamp type
/// cannot render.
pub fn rfc3339(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// `at` as the modelled timestamp type, the form this service reports an instant in a
/// member the model TYPES as a timestamp with (`PhoneNumberInformation$CreatedAt`) —
/// the typed counterpart of [`rfc3339`], which is what a string-valued field carries.
///
/// The protocol, not this service, then decides how such a member reaches the wire, so
/// a stored instant is handed over as an instant rather than pre-rendered as text.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

// =============================================================================
// The delivery policy document
// =============================================================================

/// A delivery policy document: how SNS retries a failed delivery to an HTTP/S
/// endpoint.
///
/// Modelled as a struct rather than carried as a [`serde_json::Value`] so the
/// document's members serialize in SNS's own order — a `Value` object is sorted by
/// key, which no SNS response is. Members this emulator gives no meaning to (a
/// throttle policy, a protocol SNS has added since) are carried in the `other` map of
/// each level, so a document the topic holds is reported WITH them rather than
/// silently shortened; they serialize after the modelled members of their level.
///
/// It lives here, beside [`TopicPolicyDocument`], because both the read that reports
/// `EffectiveDeliveryPolicy` and the write that accepts a `DeliveryPolicy` are stated
/// by it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeliveryPolicy {
    pub http: HttpDeliveryPolicy,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpDeliveryPolicy {
    pub default_healthy_retry_policy: HealthyRetryPolicy,
    pub disable_subscription_overrides: bool,
    pub default_request_policy: RequestPolicy,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthyRetryPolicy {
    pub min_delay_target: i64,
    pub max_delay_target: i64,
    pub num_retries: i64,
    pub num_max_delay_retries: i64,
    pub num_no_delay_retries: i64,
    pub num_min_delay_retries: i64,
    pub backoff_function: String,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPolicy {
    pub header_content_type: String,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl RequestPolicy {
    /// The request policy SNS delivers an HTTP/S notification under by default: the
    /// plain-text content type recorded AWS responses carry for both documents a request
    /// policy appears in — a topic's `http.defaultRequestPolicy` and the `requestPolicy`
    /// of a subscription's effective delivery policy — so the two cannot drift.
    pub fn system_default() -> Self {
        Self {
            header_content_type: "text/plain; charset=UTF-8".to_string(),
            other: std::collections::BTreeMap::new(),
        }
    }
}

impl HealthyRetryPolicy {
    /// SNS's own retry policy — the "account system defaults" every effective delivery
    /// policy is taken over: "the default delivery policy ... numRetries 3 ...
    /// minDelayTarget 20 ... maxDelayTarget 20 ... backoffFunction linear" (SNS
    /// Developer Guide, "Amazon SNS message delivery retries").
    ///
    /// ONE definition of the defaults, because a topic's delivery policy and a
    /// subscription's are laid over the same ones.
    pub fn system_default() -> Self {
        Self {
            min_delay_target: 20,
            max_delay_target: 20,
            num_retries: 3,
            num_max_delay_retries: 0,
            num_no_delay_retries: 0,
            num_min_delay_retries: 0,
            backoff_function: "linear".to_string(),
            other: std::collections::BTreeMap::new(),
        }
    }
}

impl DeliveryPolicy {
    /// SNS's own delivery policy — the "system defaults" an `EffectiveDeliveryPolicy`
    /// is taken over (`GetTopicAttributesResponse$Attributes` doc), with the plain-text
    /// `defaultRequestPolicy` recorded AWS responses carry.
    pub fn system_default() -> Self {
        Self {
            http: HttpDeliveryPolicy {
                default_healthy_retry_policy: HealthyRetryPolicy::system_default(),
                disable_subscription_overrides: false,
                default_request_policy: RequestPolicy::system_default(),
                other: std::collections::BTreeMap::new(),
            },
            other: std::collections::BTreeMap::new(),
        }
    }
}

/// A delivery policy as a CALLER wrote it: every member optional, because a caller
/// sets only the ones it wants to override. Laying one of these over
/// [`DeliveryPolicy::system_default`] is what "taking system defaults into account"
/// means.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredDeliveryPolicy {
    pub http: Option<ConfiguredHttpPolicy>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredHttpPolicy {
    pub default_healthy_retry_policy: Option<ConfiguredRetryPolicy>,
    pub disable_subscription_overrides: Option<bool>,
    pub default_request_policy: Option<ConfiguredRequestPolicy>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

/// A retry policy as a CALLER wrote it: every member optional.
///
/// It is `Serialize` as well, because the document a subscription's `DeliveryPolicy` is
/// STORED as states exactly these members with exactly these names — what the caller
/// supplied, and `null` for what it did not ([`stored_delivery_policy`]).
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredRetryPolicy {
    pub min_delay_target: Option<i64>,
    pub max_delay_target: Option<i64>,
    pub num_retries: Option<i64>,
    pub num_max_delay_retries: Option<i64>,
    pub num_no_delay_retries: Option<i64>,
    pub num_min_delay_retries: Option<i64>,
    pub backoff_function: Option<String>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

/// A request policy as a CALLER wrote it, `Serialize` for the reason
/// [`ConfiguredRetryPolicy`] is.
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredRequestPolicy {
    pub header_content_type: Option<String>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ConfiguredRetryPolicy {
    /// Overlay the retry members the caller set onto `held`, member by member: a member
    /// the caller left out keeps the value `held` already carries.
    ///
    /// ONE overlay for every document a retry policy appears in — a topic's
    /// `http.defaultHealthyRetryPolicy` and a subscription's own `healthyRetryPolicy` —
    /// so the two cannot drift.
    pub fn overlay(self, held: &mut HealthyRetryPolicy) {
        if let Some(value) = self.min_delay_target {
            held.min_delay_target = value;
        }
        if let Some(value) = self.max_delay_target {
            held.max_delay_target = value;
        }
        if let Some(value) = self.num_retries {
            held.num_retries = value;
        }
        if let Some(value) = self.num_max_delay_retries {
            held.num_max_delay_retries = value;
        }
        if let Some(value) = self.num_no_delay_retries {
            held.num_no_delay_retries = value;
        }
        if let Some(value) = self.num_min_delay_retries {
            held.num_min_delay_retries = value;
        }
        if let Some(value) = self.backoff_function {
            held.backoff_function = value;
        }
        held.other.extend(self.other);
    }
}

impl ConfiguredDeliveryPolicy {
    /// Overlay what the caller set onto `defaults`, member by member: a member the
    /// caller left out keeps SNS's default.
    pub fn overlay(self, defaults: &mut DeliveryPolicy) {
        defaults.other.extend(self.other);
        let Some(http) = self.http else { return };
        defaults.http.other.extend(http.other);
        if let Some(disable) = http.disable_subscription_overrides {
            defaults.http.disable_subscription_overrides = disable;
        }
        if let Some(retry) = http.default_healthy_retry_policy {
            retry.overlay(&mut defaults.http.default_healthy_retry_policy);
        }
        if let Some(request) = http.default_request_policy {
            if let Some(value) = request.header_content_type {
                defaults.http.default_request_policy.header_content_type = value;
            }
            defaults
                .http
                .default_request_policy
                .other
                .extend(request.other);
        }
    }
}

/// The bounds a retry policy's members are documented at (the Amazon SNS Developer
/// Guide, "Amazon SNS message delivery retries": a delay target is a number of seconds
/// up to an hour, a retry count is at most 100, and the backoff function is one of the
/// four published curves).
pub const MIN_DELAY_SECONDS: i64 = 1;
pub const MAX_DELAY_SECONDS: i64 = 3_600;
pub const MAX_RETRIES: i64 = 100;
pub const BACKOFF_FUNCTIONS: &[&str] = &["arithmetic", "exponential", "geometric", "linear"];

impl ConfiguredRetryPolicy {
    /// Why the retry policy's members constrain EACH OTHER in a way this one breaks, or
    /// `None` when they hold together.
    ///
    /// A member's own documented bounds are judged in
    /// [`retry_policy_bounds`] — one catalog of bounds for both documents a retry policy
    /// is written in — and what is left here is what no single member decides: a minimum
    /// delay longer than the maximum names no delay at all, the three retry phases are
    /// PART of the retry count so together they cannot come to more than it, and the
    /// backoff curve is one of the published functions. `path` is the document's own path
    /// to the retry policy, which the reason names so a caller can find the member to
    /// fix.
    pub fn bounds_reason(&self, path: &str) -> Option<String> {
        if let (Some(min), Some(max)) = (self.min_delay_target, self.max_delay_target) {
            if min > max {
                return Some(format!(
                    "{path}minDelayTarget must be less than or equal to {path}maxDelayTarget"
                ));
            }
        }
        // Saturating, because each phase count is a caller's number taken verbatim from
        // the document and this sum is reached before any per-member bound has run: three
        // values near the type's ceiling would otherwise overflow the addition. A sum that
        // saturates is still greater than any retry count the rule compares it against, so
        // the comparison below answers the same thing.
        let phases = self
            .num_no_delay_retries
            .unwrap_or(0)
            .saturating_add(self.num_min_delay_retries.unwrap_or(0))
            .saturating_add(self.num_max_delay_retries.unwrap_or(0));
        if let Some(retries) = self.num_retries {
            if phases > retries {
                return Some(format!(
                    "{path}numNoDelayRetries, {path}numMinDelayRetries and \
                     {path}numMaxDelayRetries together must be less than or equal to \
                     {path}numRetries"
                ));
            }
        }
        self.backoff_function
            .as_deref()
            .filter(|function| !BACKOFF_FUNCTIONS.contains(function))
            .map(|_| {
                format!(
                    "{path}backoffFunction must be one of {}",
                    BACKOFF_FUNCTIONS.join(", ")
                )
            })
    }
}

/// The path a TOPIC's delivery policy document gives its retry policy's members.
pub const TOPIC_RETRY_POLICY_PATH: &str = "http.defaultHealthyRetryPolicy.";

/// The path a SUBSCRIPTION's delivery policy document gives its retry policy's members.
pub const SUBSCRIPTION_RETRY_POLICY_PATH: &str = "healthyRetryPolicy.";

/// Why `value` is not a delivery policy document this emulator can lay over the system
/// defaults, or `None` when it is one.
///
/// It is the SAME parse [`effective_delivery_policy`] performs, so the write path
/// refuses exactly what the read could not honour: a document whose members are of the
/// wrong type would otherwise be stored and then be left out of the effective policy.
///
/// The retry policy's own members are bounded in [`retry_policy_bounds`] and the rules
/// its members hold EACH OTHER to in [`ConfiguredRetryPolicy::bounds_reason`], both of
/// which the write path asks after this one.
pub fn delivery_policy_refusal(value: &str) -> Option<String> {
    match serde_json::from_str::<ConfiguredDeliveryPolicy>(value) {
        Ok(_) => None,
        Err(error) => Some(format!(
            "the value must be a delivery policy document: {error}"
        )),
    }
}

/// Why the retry policy a TOPIC's delivery policy document carries breaks a rule its
/// members hold EACH OTHER to, or `None` when they hold together (and for a value that
/// is no such document at all, which [`delivery_policy_refusal`] answers).
pub fn delivery_policy_member_agreement_reason(value: &str) -> Option<String> {
    serde_json::from_str::<ConfiguredDeliveryPolicy>(value)
        .ok()?
        .http
        .as_ref()?
        .default_healthy_retry_policy
        .as_ref()?
        .bounds_reason(TOPIC_RETRY_POLICY_PATH)
}

/// The effective delivery policy of a topic: the system defaults with the topic's own
/// `DeliveryPolicy` laid over them, as the `EffectiveDeliveryPolicy` attribute carries
/// it.
pub fn effective_delivery_policy(topic: &TopicRecord) -> String {
    let mut effective = DeliveryPolicy::system_default();
    if let Some(configured) = topic
        .attributes
        .get(DELIVERY_POLICY_ATTRIBUTE)
        .and_then(|raw| serde_json::from_str::<ConfiguredDeliveryPolicy>(raw).ok())
    {
        configured.overlay(&mut effective);
    }
    serde_json::to_string(&effective).unwrap_or_default()
}

// =============================================================================
// The delivery policy document of a SUBSCRIPTION
// =============================================================================

/// A SUBSCRIPTION's delivery policy document, as SNS serializes it: the retry policy at
/// the top level, rather than under the protocol key a TOPIC's policy groups its
/// defaults by.
///
/// The two documents are different shapes of the same idea — "the JSON serialization of
/// the subscription's delivery policy" (`GetSubscriptionAttributesResponse$Attributes`
/// doc) governs ONE subscription, so it names no protocol and no
/// `disableSubscriptionOverrides`, and its retry policy is `healthyRetryPolicy`.
///
/// Every member of the shape is serialized, including the ones a subscription has no
/// value for: the policy is a serialization of the whole document rather than of the
/// members a caller happened to name, so a retry policy for an unhealthy endpoint and a
/// throttle policy nobody set are reported as the nulls they are, and the delivery
/// guarantee as the flag it is. Members this emulator gives no meaning to ride in `other`
/// so a document is reported WITH them rather than silently shortened.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionDeliveryPolicy {
    pub healthy_retry_policy: HealthyRetryPolicy,
    pub sickly_retry_policy: Option<serde_json::Value>,
    pub throttle_policy: Option<serde_json::Value>,
    /// How SNS forms the REQUEST it delivers the notification in — the `Content-Type`
    /// header of the POST — reported in the same place and the same order the document's
    /// other sections are: present as the document's own section, and `null` for a
    /// policy that states none. (A subscription's STORED policy states one only where its
    /// owner wrote one; the EFFECTIVE policy of an HTTP/S subscription takes the topic's
    /// default, see [`effective_subscription_delivery_policy`].)
    pub request_policy: Option<RequestPolicy>,
    pub guaranteed: bool,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl SubscriptionDeliveryPolicy {
    /// The policy a subscription nobody has given one delivers under: the account system
    /// defaults ([`HealthyRetryPolicy::system_default`]), no policy of its own for an
    /// unhealthy endpoint, no throttling, no request policy of its own and no delivery
    /// guarantee.
    pub fn system_default() -> Self {
        Self {
            healthy_retry_policy: HealthyRetryPolicy::system_default(),
            sickly_retry_policy: None,
            throttle_policy: None,
            request_policy: None,
            guaranteed: false,
            other: std::collections::BTreeMap::new(),
        }
    }
}

/// A subscription's delivery policy as a CALLER wrote it: every member optional,
/// because a caller sets only the ones it wants to override.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfiguredSubscriptionDeliveryPolicy {
    pub healthy_retry_policy: Option<ConfiguredRetryPolicy>,
    pub sickly_retry_policy: Option<serde_json::Value>,
    pub throttle_policy: Option<serde_json::Value>,
    pub request_policy: Option<ConfiguredRequestPolicy>,
    pub guaranteed: Option<bool>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ConfiguredSubscriptionDeliveryPolicy {
    /// Overlay what the caller set onto `defaults`, member by member: a retry member the
    /// caller left out keeps the system default, which is what NORMALIZING a stored
    /// policy means.
    pub fn overlay(self, defaults: &mut SubscriptionDeliveryPolicy) {
        defaults.other.extend(self.other);
        if let Some(retry) = self.healthy_retry_policy {
            retry.overlay(&mut defaults.healthy_retry_policy);
        }
        if self.sickly_retry_policy.is_some() {
            defaults.sickly_retry_policy = self.sickly_retry_policy;
        }
        if self.throttle_policy.is_some() {
            defaults.throttle_policy = self.throttle_policy;
        }
        if let Some(request) = self.request_policy {
            let mut held = defaults
                .request_policy
                .take()
                .unwrap_or_else(RequestPolicy::system_default);
            if let Some(value) = request.header_content_type {
                held.header_content_type = value;
            }
            held.other.extend(request.other);
            defaults.request_policy = Some(held);
        }
        if let Some(guaranteed) = self.guaranteed {
            defaults.guaranteed = guaranteed;
        }
    }
}

/// Why `value` is not a delivery policy document a SUBSCRIPTION can hold, or `None`
/// when it is one.
///
/// The same parse [`normalized_subscription_delivery_policy`] performs, so the write
/// refuses exactly what the read could not honour, and the same documented bounds on the
/// retry policy a TOPIC's document is held to ([`delivery_policy_refusal`]): one retry
/// policy, one rule, whichever document carried it.
pub fn subscription_delivery_policy_refusal(value: &str) -> Option<String> {
    match serde_json::from_str::<ConfiguredSubscriptionDeliveryPolicy>(value) {
        Ok(configured) => configured
            .healthy_retry_policy
            .as_ref()
            .and_then(|retry| retry.bounds_reason(SUBSCRIPTION_RETRY_POLICY_PATH)),
        Err(error) => Some(format!(
            "the value must be a delivery policy document: {error}"
        )),
    }
}

/// One subscription of one topic, as `Subscribe` minted it.
#[derive(Debug, Clone)]
pub struct SubscriptionRecord {
    pub arn: String,
    pub topic_arn: String,
    pub protocol: String,
    pub endpoint: Option<String>,
    pub owner: String,
    /// The principal that created the subscription, as the
    /// [`SUBSCRIPTION_PRINCIPAL_ATTRIBUTE`] reports it.
    pub principal: String,
    /// The caller-settable attributes (`FilterPolicy`, `RawMessageDelivery`, …).
    pub attributes: HashMap<String, String>,
    /// The subscriber has not confirmed yet, so SNS delivers nothing to it.
    pub pending_confirmation: bool,
    /// The confirmation token `Subscribe` minted for this subscription, held for the
    /// subscriptions whose subscriber has to confirm and `None` for the ones SNS
    /// confirms itself.
    ///
    /// AWS sends it to the ENDPOINT — in the `SubscriptionConfirmation` message posted
    /// to an HTTP/S endpoint or mailed to an email address — so it never travels on the
    /// wire in either direction but the `ConfirmSubscription` caller's: this field is
    /// this emulator's stand-in for that carrier, the way
    /// [`SandboxPhoneNumberRecord::one_time_password`] stands in for the SMS one. No
    /// response carries it and no refusal names it.
    pub confirmation_token: Option<String>,
    pub created_at: SystemTime,
    /// The message replay the subscription's current [`REPLAY_POLICY_ATTRIBUTE`] asked
    /// for, or `None` for a subscription no replay was ever asked for on.
    ///
    /// What [`REPLAY_STATUS_ATTRIBUTE`] reports is computed from it and the clock, so a
    /// replay reports the state it is in rather than a value a caller wrote.
    pub replay: Option<ReplayRecord>,
}

/// The replay one write of a subscription's [`REPLAY_POLICY_ATTRIBUTE`] started.
///
/// A replay redelivers the messages already in the topic's archive, so the work itself
/// is done by the call that asked for it; what remains is the state the subscription
/// REPORTS while it settles, which is why only the instant it started and whether the
/// policy named a window at all are held.
#[derive(Debug, Clone, Copy)]
pub struct ReplayRecord {
    /// When the call that asked for this replay started it.
    pub started_at: SystemTime,
    /// Whether the policy named a window of the archive that could be read: one that
    /// names none redelivers nothing, which is a replay that could not complete.
    pub window_was_readable: bool,
}

/// The subscription attribute a message replay's state is reported under:
/// "ReplayStatus – Retrieves the status of the subscription message replay"
/// (`SubscribeInput$Attributes` doc). COMPUTED from the replay record and the clock, so
/// no caller sets it and it is not in [`SUBSCRIPTION_ATTRIBUTE_NAMES`].
pub const REPLAY_STATUS_ATTRIBUTE: &str = "ReplayStatus";

/// "Completed – The replay has successfully redelivered all messages, and is now
/// delivering newly published messages" (`SubscribeInput$Attributes` doc).
pub const REPLAY_STATUS_COMPLETED: &str = "Completed";

/// "Pending – The default state while the replay initiates" (`SubscribeInput$Attributes`
/// doc).
pub const REPLAY_STATUS_PENDING: &str = "Pending";

/// "Failed – The replay was unable to complete" (`SubscribeInput$Attributes` doc).
pub const REPLAY_STATUS_FAILED: &str = "Failed";

/// How long a replay reports itself [`REPLAY_STATUS_PENDING`] before it reports itself
/// [`REPLAY_STATUS_COMPLETED`].
///
/// A span this emulator chooses: AWS documents the statuses a replay moves through but no
/// duration for them, and the span is deliberately non-zero so a caller that reads the
/// subscription straight after asking for the replay still observes the state the replay
/// starts in rather than a replay that was never pending.
pub const REPLAY_SETTLES_AFTER: Duration = Duration::from_millis(500);

impl ReplayRecord {
    /// What [`REPLAY_STATUS_ATTRIBUTE`] reports for this replay now.
    ///
    /// A replay whose policy named no readable window redelivered nothing and stays
    /// [`REPLAY_STATUS_FAILED`]; every other replay has redelivered the archive's
    /// messages already, so it settles from [`REPLAY_STATUS_PENDING`] to
    /// [`REPLAY_STATUS_COMPLETED`] once [`REPLAY_SETTLES_AFTER`] has passed — a
    /// transition the clock owes, not one a read performs.
    pub fn status(&self) -> &'static str {
        if !self.window_was_readable {
            return REPLAY_STATUS_FAILED;
        }
        let settled = SystemTime::now()
            .duration_since(self.started_at)
            .is_ok_and(|elapsed| elapsed >= REPLAY_SETTLES_AFTER);
        if settled {
            REPLAY_STATUS_COMPLETED
        } else {
            REPLAY_STATUS_PENDING
        }
    }
}

/// One subscription as a LISTING reports it: the wire shape every paginated
/// subscription listing carries, built from the stored record.
///
/// Shared by every listing, because what a subscription looks like in a list does not
/// depend on which listing selected it — a topic's own subscriptions or the whole
/// account's. A subscription that is not confirmed yet has no ARN SNS publishes, so
/// the listing carries [`PENDING_CONFIRMATION_LISTED`] in its place until the
/// subscriber calls `ConfirmSubscription`.
pub fn listed_subscription(sub: &SubscriptionRecord) -> Subscription {
    Subscription {
        subscription_arn: Some(if sub.pending_confirmation {
            PENDING_CONFIRMATION_LISTED.to_string()
        } else {
            sub.arn.clone()
        }),
        owner: Some(sub.owner.clone()),
        protocol: Some(sub.protocol.clone()),
        endpoint: sub.endpoint.clone(),
        topic_arn: Some(sub.topic_arn.clone()),
    }
}

/// The subscription attribute a caller-set message filter lives under.
pub const FILTER_POLICY_ATTRIBUTE: &str = "FilterPolicy";

/// The subscription attribute that says WHICH part of a message the filter policy is
/// applied to.
pub const FILTER_POLICY_SCOPE_ATTRIBUTE: &str = "FilterPolicyScope";

/// The filtering scope a subscription with a filter policy and no explicit scope is
/// filtered under: "FilterPolicyScope – ... MessageAttributes (default) – The filter
/// is applied on the message attributes"
/// (`GetSubscriptionAttributesResponse$Attributes` doc).
pub const DEFAULT_FILTER_POLICY_SCOPE: &str = "MessageAttributes";

/// The subscription attribute that says whether SNS delivers the message body alone.
pub const RAW_MESSAGE_DELIVERY_ATTRIBUTE: &str = "RawMessageDelivery";

/// What `RawMessageDelivery` is for a subscription whose owner has not set it: SNS
/// wraps a notification in its JSON envelope unless raw delivery is asked for
/// ("RawMessageDelivery – true if raw message delivery is enabled for the
/// subscription", `GetSubscriptionAttributesResponse$Attributes` doc).
pub const RAW_MESSAGE_DELIVERY_DISABLED: &str = "false";

/// The attributes SNS COMPUTES for a subscription rather than storing: its own ARN,
/// the topic it is a subscription of, how and where it delivers, the account that owns
/// it, and its confirmation state. None of them is settable, so none is in
/// [`SUBSCRIPTION_ATTRIBUTE_NAMES`].
pub const SUBSCRIPTION_ARN_ATTRIBUTE: &str = "SubscriptionArn";
pub const PROTOCOL_ATTRIBUTE: &str = "Protocol";
pub const ENDPOINT_ATTRIBUTE: &str = "Endpoint";
pub const PENDING_CONFIRMATION_ATTRIBUTE: &str = "PendingConfirmation";
pub const CONFIRMATION_WAS_AUTHENTICATED_ATTRIBUTE: &str = "ConfirmationWasAuthenticated";

/// The subscription attribute naming WHO created the subscription — the principal whose
/// call to `Subscribe` minted it, which is what a client auditing a topic's subscribers
/// reads. Computed from the record rather than stored by a caller, so it is not in
/// [`SUBSCRIPTION_ATTRIBUTE_NAMES`] either.
pub const SUBSCRIPTION_PRINCIPAL_ATTRIBUTE: &str = "SubscriptionPrincipal";

/// The principal form an ACCOUNT is written as: its root user's ARN.
pub fn root_principal_arn(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:root")
}

/// The IAM user name the identity this emulator's own credentials present carries.
const EMULATOR_USER_NAME: &str = "emulator";

/// The principal that made this call, as a resource it creates records it.
///
/// Every request this emulator serves is made by the one identity its published
/// credentials present — the IAM user `sts:GetCallerIdentity` answers with — so that is
/// the principal a subscription minted here was created by, and a client can match the
/// two against each other.
pub fn calling_principal_arn() -> String {
    ::emulator_server::emulator_arn("iam", "", &format!("user/{EMULATOR_USER_NAME}"))
}

/// How a boolean-valued attribute is written on the wire: SNS carries every attribute
/// value as a string, so a flag is the word, not a JSON literal.
pub fn attribute_flag(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// The canonical form of a FLAG-valued attribute value, or `None` when the value is
/// neither of the two words a flag takes.
///
/// A flag is the WORD `true` or `false`, matched without regard to case — the value is
/// a boolean carried as a string, and a boolean is the same whichever case a client (or
/// the template of an infrastructure tool) spelled it in — and it is held, and reported,
/// in the lower-case form [`attribute_flag`] writes. The emulator already reads a
/// topic's own flags this way (`FifoTopic`, `ContentBasedDeduplication`), so one
/// definition now serves every attribute whose value is a flag.
pub fn flag_attribute_value(value: &str) -> Option<&'static str> {
    ["true", "false"]
        .into_iter()
        .find(|word| value.eq_ignore_ascii_case(word))
}

/// Whether a flag-valued attribute READS as set: the word `true` in any case, and
/// nothing else — the reading side of [`flag_attribute_value`], so every attribute whose
/// value is a flag is written, stored and read through the one definition of its two
/// words.
pub fn flag_attribute_enabled(value: &str) -> bool {
    flag_attribute_value(value) == Some("true")
}

/// The WHOLE sentence SNS refuses a FLAG-valued attribute's value with: the attribute,
/// the value it rejected in brackets, and the two words a flag takes.
///
/// The rejected value is echoed because SNS's own sentence echoes it — it is the only
/// place the service states WHICH value it would not take, and it is what the caller has
/// to change. Everything around it is fixed text.
///
/// The sentence is framed by the MEMBER the attribute arrived in, like every other
/// refusal of an attribute's value ([`AttributeMember::refusal`]): a request that carried
/// it in an `Attributes` MAP is told which member it has to change, and one that was given
/// the attribute BY NAME needs no such frame. Recorded evidence: a `RawMessageDelivery` of
/// `yes` answers `400 InvalidParameter` with `Invalid parameter: Attributes Reason:
/// RawMessageDelivery: Invalid value [yes]. Must be true or false.` through `Subscribe`,
/// and with `Invalid parameter: RawMessageDelivery: Invalid value [yes]. Must be true or
/// false.` through `SetSubscriptionAttributes`.
///
/// ONE sentence for every attribute whose value is a flag, so the refusal does not depend
/// on which flag, or which record kind, carried the value.
pub fn flag_attribute_refusal(name: &str, value: &str, member: AttributeMember) -> String {
    member.refusal(
        name,
        &format!(
            "Invalid value [{value}]. Must be {RAW_MESSAGE_DELIVERY_ENABLED} or \
             {RAW_MESSAGE_DELIVERY_DISABLED}."
        ),
    )
}

// =============================================================================
// What a subscription attribute may be SET to
// =============================================================================

/// The subscription attribute a dead-letter queue is named under: "RedrivePolicy –
/// When specified, sends undeliverable messages to the specified Amazon SQS
/// dead-letter queue" (`SetSubscriptionAttributesInput$AttributeName` doc).
pub const REDRIVE_POLICY_ATTRIBUTE: &str = "RedrivePolicy";

/// The subscription attribute a message replay is asked for under
/// (`SubscribeInput$Attributes` doc).
pub const REPLAY_POLICY_ATTRIBUTE: &str = "ReplayPolicy";

/// The subscription attribute the role a Firehose delivery is written under lives
/// under: "SubscriptionRoleArn – The ARN of the IAM role that has … Permission to write
/// to the Firehose delivery stream" (`SetSubscriptionAttributesInput$AttributeName`
/// doc).
pub const SUBSCRIPTION_ROLE_ARN_ATTRIBUTE: &str = "SubscriptionRoleArn";

/// The member of a replay policy that says where in the archive the replay starts.
pub const STARTING_POINT_MEMBER: &str = "StartingPoint";

/// The member of a replay policy that says where the replay stops: "If an ending point
/// was specified in the ReplayPolicy then the subscription will no longer receive newly
/// published messages" (`SubscribeInput$Attributes` doc), so it is optional — a policy
/// without one replays up to now and keeps delivering.
pub const ENDING_POINT_MEMBER: &str = "EndingPoint";

/// The window of a topic's message archive a replay policy names — `(from, until)`, with
/// `until` of `None` for a policy that names no ending point — or `None` when the
/// document names no window that can be read.
///
/// A replay "replay[s] messages stored in the specified Amazon SNS topic"
/// (`SubscribeInput$Attributes` doc) from a point in time, so the points are instants,
/// written in the same RFC 3339 form this service reports every timestamp in (see
/// [`rfc3339`], which is what a caller reads a topic's
/// [`BEGINNING_ARCHIVE_TIME_ATTRIBUTE`] as).
pub fn replay_window(policy: &str) -> Option<(SystemTime, Option<SystemTime>)> {
    let members = json_object(policy)?;
    let point = |name: &str| {
        members
            .get(name)
            .and_then(serde_json::Value::as_str)
            .and_then(instant_from_rfc3339)
    };
    Some((point(STARTING_POINT_MEMBER)?, point(ENDING_POINT_MEMBER)))
}

/// The instant `value` names when it is an RFC 3339 timestamp, or `None` when it is not
/// one — the reading counterpart of [`rfc3339`].
fn instant_from_rfc3339(value: &str) -> Option<SystemTime> {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .and_then(|at| SystemTime::try_from(at).ok())
}

/// The member of a redrive policy that names the dead-letter queue.
pub const DEAD_LETTER_TARGET_ARN: &str = "deadLetterTargetArn";

/// The dead-letter queue `attributes` name, or `None` when they ask for no redrive.
///
/// "RedrivePolicy – When specified, sends undeliverable messages to the specified
/// Amazon SQS dead-letter queue" (`SetSubscriptionAttributesInput$AttributeName` doc):
/// the queue is the document's [`DEAD_LETTER_TARGET_ARN`] member, which is the member
/// every stored redrive policy carries, since a document without it is refused
/// ([`missing_required_value_refusal`]).
pub fn dead_letter_target_arn(attributes: &HashMap<String, String>) -> Option<String> {
    json_object(attributes.get(REDRIVE_POLICY_ATTRIBUTE)?)?
        .get(DEAD_LETTER_TARGET_ARN)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// What `RawMessageDelivery` is when raw delivery is asked for — the counterpart of
/// [`RAW_MESSAGE_DELIVERY_DISABLED`], and the only other value the flag takes.
pub const RAW_MESSAGE_DELIVERY_ENABLED: &str = "true";

/// The filtering scopes SNS defines: "FilterPolicyScope – This attribute lets you
/// choose the filtering scope by using one of the following string value types:
/// MessageAttributes (default) … MessageBody"
/// (`SetSubscriptionAttributesInput$AttributeName` doc).
pub const FILTER_POLICY_SCOPES: &[&str] =
    &[DEFAULT_FILTER_POLICY_SCOPE, MESSAGE_BODY_FILTER_POLICY_SCOPE];

/// The scope whose filtering reads the message BODY: "MessageBody – The filter is applied
/// on the message body" (`GetSubscriptionAttributesResponse$Attributes` doc) — the other
/// value of [`FILTER_POLICY_SCOPES`], and the one
/// [`filter_policy::admits`] reads the body under.
pub const MESSAGE_BODY_FILTER_POLICY_SCOPE: &str = "MessageBody";

/// The `FilterPolicyScope` a subscription REPORTS, or `None` when it reports none.
///
/// The scope says which part of a message "the filter is applied on"
/// (`GetSubscriptionAttributesResponse$Attributes` doc), so it is in force only while
/// there is a policy to apply: a subscription that holds a `FilterPolicy` reports the
/// scope that policy is matched under — the documented default when its owner set none —
/// and one that holds no policy reports no scope at all, including when a scope was set
/// for a policy the subscription does not have yet. The scope is still HELD, so setting
/// the policy later reveals it.
pub fn reported_filter_policy_scope(attributes: &HashMap<String, String>) -> Option<&str> {
    attributes.get(FILTER_POLICY_ATTRIBUTE)?;
    Some(
        attributes
            .get(FILTER_POLICY_SCOPE_ATTRIBUTE)
            .map_or(DEFAULT_FILTER_POLICY_SCOPE, String::as_str),
    )
}

/// The protocols raw message delivery may be ENABLED for: the ones whose endpoint
/// receives the published body ITSELF — "RawMessageDelivery – When set to true, enables
/// raw message delivery to Amazon SQS or HTTP/S endpoints"
/// (`SetSubscriptionAttributesInput$AttributeName` doc), and a Firehose delivery stream,
/// which takes the bare message bodies when raw delivery is enabled ("Fanout to
/// Firehose delivery streams", SNS Developer Guide). A subscription over any other
/// protocol always carries SNS's JSON envelope — the body reaches a mailbox, a phone or
/// a function invocation rather than a stream of bodies — so there is no raw delivery to
/// switch on for it.
pub const RAW_MESSAGE_DELIVERY_PROTOCOLS: &[&str] = &["sqs", "http", "https", "firehose"];

/// The protocols a `SubscriptionRoleArn` applies to: "The following attribute applies
/// only to Amazon Data Firehose delivery stream subscriptions: SubscriptionRoleArn"
/// (`SetSubscriptionAttributesInput$AttributeName` doc).
pub const SUBSCRIPTION_ROLE_ARN_PROTOCOLS: &[&str] = &["firehose"];

/// The protocols an `EffectiveDeliveryPolicy` is REPORTED for: the endpoints a delivery
/// policy governs the delivery to — "DeliveryPolicy – The policy that defines how Amazon
/// SNS retries failed deliveries to HTTP/S endpoints" (`CreateTopicInput$Attributes`
/// doc). A subscription over any other protocol has no such retry policy, so SNS reports
/// no effective one for it. Recorded evidence: a confirmed `sqs` subscription reports
/// exactly its identity, routing, confirmation state, raw-delivery flag and subscribing
/// principal, and no delivery policy among them.
pub const DELIVERY_POLICY_PROTOCOLS: &[&str] = &["http", "https"];

/// The members of `value` when it is a JSON OBJECT, else `None` — the form every policy
/// document a subscription attribute carries has.
pub fn json_object(value: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// The clause SNS opens the refusal of the attribute `name` whose document it could not
/// parse with: `Invalid parameter:`, the attribute AS THE CALLER SPELLED IT, and the
/// parse that failed — which the parser's own diagnostic is appended to (see
/// [`attribute_parse_refusal`]).
///
/// The fault is in the named attribute's own document, so SNS names that attribute
/// DIRECTLY: there is no `<member> Reason: ` frame here, whichever member carried the
/// write. (That frame belongs to the refusals whose offending parameter IS the
/// `Attributes` map — see [`AttributeMember`].) Recorded evidence:
/// `SetSubscriptionAttributes` with a `FilterPolicy` value that is not JSON answers `400
/// InvalidParameter` with `Invalid parameter: FilterPolicy: failed to parse JSON. ` and
/// the parser's diagnostic, and nothing in front of the attribute name.
pub fn attribute_parse_refusal_opening(name: &str) -> String {
    format!("Invalid parameter: {name}: failed to parse JSON.")
}

/// The WHOLE sentence SNS refuses a value of the attribute `name` that is not JSON at
/// all with: the opening clause ([`attribute_parse_refusal_opening`]) followed by the
/// parser's own diagnostic of where the document stopped parsing.
///
/// One sentence per CONDITION rather than per attribute: every attribute of a topic or a
/// subscription whose value is a JSON document is parsed by the same parser, so a value
/// that is not JSON is refused the same way whichever attribute carried it — only the
/// name changes.
///
/// A document that stops on an unquoted word where a value belongs is reported by
/// naming that word, which is what lets a caller whose policy was built by templating
/// or string concatenation locate the fault; every other fault names the character the
/// parse stopped on, or the end of the input. Both readings — and the line and column
/// in the location clause — are the tree's shared one
/// ([`::service_common::json_parse_fault`]), taken from the parse that actually ran, so
/// the position is the parser's rather than a guess. Nothing in the sentence echoes the
/// document back: its source is reported as redacted.
pub fn attribute_parse_refusal(name: &str, value: &str, error: &serde_json::Error) -> String {
    use ::service_common::json_parse_fault as fault;

    let diagnosis = match fault::unrecognized_token(value, error) {
        Some((word, line, column)) => format!(
            "Unrecognized token '{word}': was expecting ({}){}",
            fault::VALUE_CLASSES,
            fault::source_location(line, column)
        ),
        None => format!(
            "{}{}",
            fault::unexpected_input(value, error),
            fault::source_location(error.line(), error.column())
        ),
    };
    format!("{} {diagnosis}", attribute_parse_refusal_opening(name))
}

/// The sentence a value of the attribute `name` is refused with when it is not JSON at
/// all, or `None` when it parses as JSON (whatever the document then turns out to be).
///
/// The gate every attribute whose value is a JSON document passes first: a value the
/// parser could not read is a parse failure ([`attribute_parse_refusal`]), and only a
/// document that DID parse is then judged against the shape that attribute requires.
pub fn attribute_json_parse_refusal(name: &str, value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .err()
        .map(|error| attribute_parse_refusal(name, value, &error))
}

/// The sentence SNS refuses a NESTED filter policy scoped to the message attributes
/// with: a message attribute is a flat name and value, so only a policy scoped to the
/// message BODY matches a property inside a property.
pub const NESTED_FILTER_POLICY_REFUSAL: &str =
    "Invalid parameter: Filter policy scope MessageAttributes does not support nested \
     filter policy";

/// Whether this filter policy NESTS: a member whose value is an object of further
/// property names, rather than the list of values a property is matched against.
fn filter_policy_is_nested(policy: &str) -> bool {
    json_object(policy).is_some_and(|members| members.values().any(serde_json::Value::is_object))
}

/// Why the subscription attributes `attributes` would leave a record SNS cannot match
/// messages against, or `None` when they would not.
///
/// The `FilterPolicy` and the `FilterPolicyScope` constrain EACH OTHER, so neither can
/// be judged on its own: "MessageAttributes (default) – The filter is applied on the
/// message attributes" (`SetSubscriptionAttributesInput$AttributeName` doc), and a
/// message attribute is a flat name and value — a policy that matches a property
/// INSIDE a property is only meaningful against the message body. The pair is
/// therefore judged on the map the write WOULD leave, whichever of the two members the
/// request carried.
pub fn filter_policy_pair_refusal(attributes: &HashMap<String, String>) -> Option<String> {
    let policy = attributes.get(FILTER_POLICY_ATTRIBUTE)?;
    let scope = attributes
        .get(FILTER_POLICY_SCOPE_ATTRIBUTE)
        .map_or(DEFAULT_FILTER_POLICY_SCOPE, String::as_str);
    (scope == DEFAULT_FILTER_POLICY_SCOPE && filter_policy_is_nested(policy))
        .then(|| NESTED_FILTER_POLICY_REFUSAL.to_string())
}

/// The attributes of a topic or a subscription whose value is a policy DOCUMENT: the
/// access, delivery, archive, filter, redrive and replay policies.
pub const POLICY_DOCUMENT_ATTRIBUTES: &[&str] = &[
    ARCHIVE_POLICY_ATTRIBUTE,
    DELIVERY_POLICY_ATTRIBUTE,
    FILTER_POLICY_ATTRIBUTE,
    POLICY_ATTRIBUTE,
    REDRIVE_POLICY_ATTRIBUTE,
    REPLAY_POLICY_ATTRIBUTE,
];

/// The suffix of every attribute naming the IAM role SNS writes a delivery's status
/// record as (`HTTPSuccessFeedbackRoleArn`, `SQSFailureFeedbackRoleArn`, …).
pub const FEEDBACK_ROLE_ARN_SUFFIX: &str = "FeedbackRoleArn";

/// Whether the topic attribute `name` NAMES ANOTHER RESOURCE rather than carrying a
/// value of its own: the encryption key a topic is encrypted under, and the IAM roles
/// its delivery-status logging is written as.
///
/// Such an attribute is removed by writing an EMPTY value to it, the way a policy
/// document is: the empty value names no resource, so it asks for a topic that is not
/// encrypted — or that logs no delivery status — rather than for one holding a key id or
/// a role ARN that is the empty string.
pub fn names_a_resource(name: &str) -> bool {
    name == KMS_MASTER_KEY_ID_ATTRIBUTE || name.ends_with(FEEDBACK_ROLE_ARN_SUFFIX)
}

/// Whether writing `value` to the attribute `name` REMOVES it rather than setting it.
///
/// An attribute whose value is a policy document — or which NAMES ANOTHER RESOURCE
/// ([`names_a_resource`]) — is deleted by writing an EMPTY value to it: the way AWS
/// documents removing one ("To delete a filter policy, set the `FilterPolicy` attribute
/// to an empty value", SNS Developer Guide), and the way a topic's encryption is turned
/// off again. An empty value asks for a record that holds no such policy or resource
/// rather than naming a document that failed to parse or a resource called "". The
/// record then reports what it reports for a value it was never given.
///
/// A policy whose document AWS REQUIRES a member of is the exception
/// ([`POLICY_ATTRIBUTES_WITH_A_REQUIRED_MEMBER`]): the empty value carries no such
/// member, so it is judged as the document it is and refused
/// ([`missing_required_value_refusal`]) rather than read as an instruction to remove the
/// policy — two different intents a caller would otherwise have no way to tell apart.
///
/// Shared by every operation that writes an attribute of either record kind, so a value
/// is removed the same way whichever call carried the write.
pub fn clears_attribute(name: &str, value: &str) -> bool {
    value.is_empty()
        && (POLICY_DOCUMENT_ATTRIBUTES.contains(&name) || names_a_resource(name))
        && !POLICY_ATTRIBUTES_WITH_A_REQUIRED_MEMBER.contains(&name)
}

/// The policy-valued attributes whose document AWS requires a member of, so that a value
/// carrying no document at all — an empty value included — names no policy those
/// attributes can be read as.
///
/// The redrive policy is one: "sends undeliverable messages to the specified Amazon SQS
/// dead-letter queue" (`SetSubscriptionAttributesInput$AttributeName` doc), and the queue
/// it specifies is the document's [`DEAD_LETTER_TARGET_ARN`] member, without which the
/// policy redrives nowhere. Recorded evidence: an empty `RedrivePolicy` is refused `400
/// InvalidParameter` with [`missing_required_value_refusal`]'s sentence rather than
/// clearing the policy the subscription holds.
pub const POLICY_ATTRIBUTES_WITH_A_REQUIRED_MEMBER: &[&str] = &[REDRIVE_POLICY_ATTRIBUTE];

/// The sentence SNS refuses a policy document that carries none of the members the
/// attribute requires with: the attribute the write named, and that a value it cannot do
/// without is missing.
///
/// The attribute name comes from the request, so one sentence serves every attribute of
/// [`POLICY_ATTRIBUTES_WITH_A_REQUIRED_MEMBER`].
pub fn missing_required_value_refusal(name: &str) -> String {
    format!("Invalid parameter: {name}: missing required value")
}

/// The sentence SNS refuses a `FilterPolicyScope` outside its value set with: the
/// attribute, the value the caller sent in brackets, and the two values it accepts.
fn filter_policy_scope_refusal(value: &str) -> String {
    format!(
        "Invalid parameter: {FILTER_POLICY_SCOPE_ATTRIBUTE}: Invalid value [{value}]. \
         Please use either MessageBody or MessageAttributes"
    )
}

/// The sentence SNS refuses a `RedrivePolicy` whose dead-letter target is not a queue
/// with: "sends undeliverable messages to the specified Amazon SQS dead-letter queue"
/// (`SetSubscriptionAttributesInput$AttributeName` doc), so only an SQS queue can be
/// one.
pub const REDRIVE_POLICY_NOT_A_QUEUE_REFUSAL: &str =
    "Invalid parameter: RedrivePolicy: deadLetterTargetArn must be a SQS queue";

/// How a refusal names the member a subscription attribute arrived in.
///
/// The two write paths carry an attribute in different members, and SNS's refusal names
/// the member it read: the `Attributes` MAP of `Subscribe` is named, and then the entry
/// of it that was refused; `SetSubscriptionAttributes` was given the attribute by name,
/// so its refusal names that attribute alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeMember {
    /// The `Attributes` map of `Subscribe`.
    Map,
    /// The `AttributeName`/`AttributeValue` pair of `SetSubscriptionAttributes`.
    Named,
}

impl AttributeMember {
    /// The sentence a refusal of the attribute `name` for `reason` opens with under this
    /// member.
    pub fn refusal(self, name: &str, reason: &str) -> String {
        match self {
            Self::Map => format!("Invalid parameter: {ATTRIBUTES_MEMBER} Reason: {name}: {reason}"),
            Self::Named => format!("Invalid parameter: {name}: {reason}"),
        }
    }

    /// The same, for a condition SNS states as a WHOLE sentence of its own rather than
    /// about one attribute's value: the sentence stands as SNS words it, and only the
    /// member it is reported under frames it.
    ///
    /// A request that carried the attribute in the `Attributes` MAP is told which member
    /// it has to change, because that member holds every attribute it sent; one that was
    /// given the attribute BY NAME needs no such frame, the name being the whole input.
    /// Recorded evidence: enabling raw message delivery in a `Subscribe` request's
    /// `Attributes` answers `400 InvalidParameter` with `Invalid parameter: Attributes
    /// Reason: Delivery protocol [lambda] does not support raw message delivery.`, where
    /// the same condition reached through `SetSubscriptionAttributes` answers that
    /// sentence with no member frame.
    pub fn whole_sentence_refusal(self, sentence: &str) -> String {
        match self {
            Self::Map => format!("Invalid parameter: {ATTRIBUTES_MEMBER} Reason: {sentence}"),
            Self::Named => format!("Invalid parameter: {sentence}"),
        }
    }
}

/// The message a write of the subscription attribute `name` to `value` is refused
/// with, or `None` when the value is one the attribute accepts — including every
/// `name` SNS documents no constraint on the value of.
///
/// The message is the whole sentence SNS states for that condition, not a clause a
/// caller wraps: the same condition reads the same whichever operation carried the
/// write, bar the member that operation read the attribute from ([`AttributeMember`]).
/// It never echoes a policy document back, and names the value only where the sentence
/// SNS states itself names it.
///
/// This is ARGUMENT validation — it judges the value alone — so it runs before any
/// record is looked up, and it is SHARED by every operation that writes a subscription
/// attribute: a value `Subscribe` accepts cannot be one `SetSubscriptionAttributes`
/// refuses.
pub fn subscription_attribute_refusal(
    name: &str,
    value: &str,
    member: AttributeMember,
) -> Option<String> {
    // The clause form the attributes SNS states no fixed sentence for keep: the
    // attribute and the constraint its value failed.
    let attribute_reason = |reason: String| format!("Invalid parameter: Attributes Reason: {reason}");
    match name {
        // A flag, carried as a word because SNS carries every attribute value as a
        // string (see [`attribute_flag`]), in either case.
        RAW_MESSAGE_DELIVERY_ATTRIBUTE => flag_attribute_value(value)
            .is_none()
            .then(|| flag_attribute_refusal(name, value, member)),
        FILTER_POLICY_SCOPE_ATTRIBUTE => {
            (!FILTER_POLICY_SCOPES.contains(&value)).then(|| filter_policy_scope_refusal(value))
        }
        // "The simple JSON object that lets your subscriber receive only a subset of
        // messages" (`SetSubscriptionAttributesInput$AttributeName` doc): the policy is
        // matched property by property, so it has to be an object to match anything.
        FILTER_POLICY_ATTRIBUTE => match serde_json::from_str::<serde_json::Value>(value) {
            // What SNS reports when its parse of the document failed, naming the
            // attribute, the parse and where in the document it stopped — never the
            // document the caller sent.
            Err(error) => Some(attribute_parse_refusal(name, value, &error)),
            // The document parses and is an object, so what is left to judge is its
            // GRAMMAR and its size, which the matching's own reading of a policy states.
            // The reader's own sentence stands as it is; a LIMIT on the policy as a
            // whole is framed under the member the document arrived in.
            Ok(serde_json::Value::Object(_)) => {
                filter_policy::document_fault(value).map(|fault| match fault {
                    filter_policy::DocumentFault::Grammar(reason) => {
                        AttributeMember::Named.refusal(name, &reason)
                    }
                    filter_policy::DocumentFault::Limit(reason) => member.refusal(name, &reason),
                })
            }
            Ok(_) => Some(attribute_reason(format!(
                "{name} - the value must be a filter policy document: a JSON object of \
                 the message properties to match"
            ))),
        },
        // The SAME parse the stored, normalized document is computed with, so a
        // document the read path could not honour is never stored — then each retry
        // member against its own documented bound, and last the rules those members
        // hold EACH OTHER to. A value that is not JSON at all never reaches the
        // document's own shape: it is the parse failure every attribute's document
        // shares ([`attribute_json_parse_refusal`]).
        DELIVERY_POLICY_ATTRIBUTE => {
            if let Some(refusal) = attribute_json_parse_refusal(name, value) {
                return Some(refusal);
            }
            match serde_json::from_str::<ConfiguredSubscriptionDeliveryPolicy>(value) {
                Err(error) => Some(attribute_reason(format!(
                    "{name} - the value must be a delivery policy document: {error}"
                ))),
                Ok(_) => retry_policy_bounds::subscription_delivery_policy_bounds_refusal(value)
                    .or_else(|| {
                        subscription_delivery_policy_refusal(value)
                            .map(|reason| attribute_reason(format!("{name} - {reason}")))
                    }),
            }
        }
        REDRIVE_POLICY_ATTRIBUTE => match json_object(value) {
            // A value that is not a policy document at all carries no dead-letter
            // target either, which is the member the document is refused for.
            None => Some(missing_required_value_refusal(name)),
            Some(members) => match members
                .get(DEAD_LETTER_TARGET_ARN)
                .and_then(serde_json::Value::as_str)
            {
                None => Some(missing_required_value_refusal(name)),
                // "sends undeliverable messages to the specified Amazon SQS dead-letter
                // queue" — a redrive policy that names no SQS queue redrives nowhere.
                Some(arn) if arn_resource(arn, "sqs").is_none() => {
                    Some(REDRIVE_POLICY_NOT_A_QUEUE_REFUSAL.to_string())
                }
                Some(_) => None,
            },
        },
        REPLAY_POLICY_ATTRIBUTE => json_object(value).is_none().then(|| {
            // Not JSON at all is the parse failure every attribute's document shares;
            // JSON that is no object is no replay policy.
            attribute_json_parse_refusal(name, value).unwrap_or_else(|| {
                attribute_reason(format!(
                    "{name} - the value must be a replay policy document: a JSON object"
                ))
            })
        }),
        // "The ARN of the IAM role that has …" — only the ARN's shape is judged here;
        // whether that role exists is not this service's record to check.
        SUBSCRIPTION_ROLE_ARN_ATTRIBUTE => arn_shape_reason(value)
            .map(|reason| attribute_reason(format!("{name} - {reason}"))),
        _ => None,
    }
}

/// The WHOLE `InvalidParameter` message SNS refuses a subscription attribute with when
/// the subscription's delivery protocol is not one SNS documents the attribute for, or
/// `None` when the attribute applies to that protocol.
///
/// SNS documents some subscription attributes for the delivery protocols they have a
/// meaning for alone, and refuses one set on a subscription of any other protocol:
/// storing it would leave the subscription reporting a setting nothing it does is
/// governed by. Shared by every operation that writes a subscription attribute, so the
/// scope does not depend on which call wrote it.
///
/// Only the attributes the model SCOPES are scoped: raw message delivery, which asks for
/// a form of delivery only an endpoint that takes bare bodies has, and the Firehose role
/// — "The following attribute applies only to Amazon Data Firehose delivery stream
/// subscriptions: SubscriptionRoleArn" (`SubscribeInput$Attributes` doc). The delivery
/// policy is NOT: `DeliveryPolicy` and `EffectiveDeliveryPolicy` are listed among the
/// attributes a subscription's map carries with no protocol qualifying them
/// (`GetSubscriptionAttributesResponse$Attributes` doc), so every subscription accepts
/// and reports one.
///
/// The message is the complete sentence SNS states for this condition — this refusal is
/// about the SUBSCRIPTION the attribute was set on rather than about the value of one
/// request member, so it is not nested in the `<member> Reason: <attribute>: ` frame this
/// crate frames a member-level constraint failure in — reported under the member the
/// attribute arrived in ([`AttributeMember::whole_sentence_refusal`]). Recorded evidence:
/// enabling raw message delivery on a `lambda` subscription answers `400 InvalidParameter`
/// with exactly `Invalid parameter: Delivery protocol [lambda] does not support raw
/// message delivery.` when the attribute was given by name, and that sentence under
/// `Invalid parameter: Attributes Reason: ` when it arrived in a `Subscribe` request's
/// attribute map.
pub fn subscription_attribute_protocol_refusal(
    name: &str,
    value: &str,
    protocol: &str,
    member: AttributeMember,
) -> Option<String> {
    let (scoped, feature): (&[&str], &str) = match name {
        // Raw delivery is a property of the envelope SNS puts a notification in, so
        // only switching it ON is scoped: every protocol carries the JSON envelope by
        // default, which is what the flag's other value asks for.
        RAW_MESSAGE_DELIVERY_ATTRIBUTE
            if flag_attribute_value(value) == Some(RAW_MESSAGE_DELIVERY_ENABLED) =>
        {
            (RAW_MESSAGE_DELIVERY_PROTOCOLS, "raw message delivery")
        }
        SUBSCRIPTION_ROLE_ARN_ATTRIBUTE => {
            (SUBSCRIPTION_ROLE_ARN_PROTOCOLS, "a subscription role")
        }
        _ => return None,
    };
    (!scoped.contains(&protocol)).then(|| {
        member.whole_sentence_refusal(&format!(
            "Delivery protocol [{protocol}] does not support {feature}."
        ))
    })
}

/// The sentence SNS refuses a `ReplayPolicy` on a subscription whose topic archives
/// nothing with: a replay reads messages back out of the topic's message archive, so
/// there is no window to replay until the topic holds an [`ARCHIVE_POLICY_ATTRIBUTE`].
pub const REPLAY_WITHOUT_ARCHIVE_REFUSAL: &str =
    "Invalid parameter: Attributes Reason: ReplayPolicy: Topic ArchivePolicy attribute \
     is missing";

/// The delivery protocols a FIFO topic can be subscribed over.
///
/// "The following attributes apply only to FIFO topics" (`SubscribeInput$Attributes`
/// doc) names the FIFO-only settings; the protocols are the other half of the same
/// division — a FIFO topic delivers in order to the endpoint kinds Amazon SNS publishes
/// FIFO delivery for (Amazon SQS, Lambda, Amazon Data Firehose and HTTP/S), and the
/// endpoint kinds that carry no ordering at all (an email address, a phone number, a
/// mobile push endpoint) are not among them.
pub const FIFO_TOPIC_PROTOCOLS: &[&str] = &["sqs", "lambda", "firehose", "http", "https"];

/// The suffix the name of a FIFO Amazon SQS queue carries, as a FIFO topic's name does.
pub const FIFO_QUEUE_SUFFIX: &str = FIFO_SUFFIX;

/// Whether `arn` names a FIFO Amazon SQS queue.
pub fn is_fifo_queue_arn(arn: &str) -> bool {
    arn_resource(arn, "sqs").is_some_and(|queue| queue.ends_with(FIFO_QUEUE_SUFFIX))
}

/// The WHOLE `InvalidParameter` message SNS refuses a subscription with when its
/// protocol and endpoint do not fit the TOPIC's kind, or `None` when they do.
///
/// A subscription SNS cannot deliver over is one it does not create: a FIFO topic
/// delivers in order, so it is subscribed over the protocols that carry ordering
/// ([`FIFO_TOPIC_PROTOCOLS`]) alone, and a FIFO queue — which only accepts an ordered
/// message — is an endpoint of a FIFO topic alone. Shared by every operation that
/// creates or re-applies a subscription, so the topology one call admits is the topology
/// every call admits.
pub fn subscription_topology_refusal(
    topic: &TopicRecord,
    protocol: &str,
    endpoint: Option<&str>,
) -> Option<String> {
    if topic.is_fifo && !FIFO_TOPIC_PROTOCOLS.contains(&protocol) {
        // SNS states the protocol itself rather than the member it arrived in: the
        // protocol is not one a FIFO topic has a delivery for at all.
        return Some(format!("Invalid parameter: Invalid protocol type: {protocol}"));
    }
    let fifo_endpoint = endpoint.is_some_and(is_fifo_queue_arn);
    (fifo_endpoint && !topic.is_fifo).then(|| {
        // SNS states this one UNDER its own parameter clause, which is how the recorded
        // response reads: `Invalid parameter: ` followed by the whole sentence, itself
        // naming the member and the reason.
        "Invalid parameter: Invalid parameter: Endpoint Reason: FIFO SQS Queues can not \
         be subscribed to standard SNS topics"
            .to_string()
    })
}

/// The WHOLE `InvalidParameter` message SNS refuses a subscription with when an
/// attribute its protocol REQUIRES is missing from the map the write would leave, or
/// `None` when every such attribute is there.
///
/// "Specifying a valid ARN for this attribute is required for Firehose delivery stream
/// subscriptions" (`SubscribeInput$Attributes`, of `SubscriptionRoleArn`): a Firehose
/// subscription with no role is one SNS has no identity to deliver as, so it is never
/// created.
pub fn subscription_required_attribute_refusal(
    protocol: &str,
    attributes: &HashMap<String, String>,
    member: AttributeMember,
) -> Option<String> {
    (SUBSCRIPTION_ROLE_ARN_PROTOCOLS.contains(&protocol)
        && !attributes.contains_key(SUBSCRIPTION_ROLE_ARN_ATTRIBUTE))
    .then(|| {
        // The attribute the subscription cannot be created without, and the protocol
        // that requires it, under the member the attributes arrived in.
        let reason =
            format!("{SUBSCRIPTION_ROLE_ARN_ATTRIBUTE} is required for the {protocol} protocol");
        match member {
            AttributeMember::Map => {
                format!("Invalid parameter: {ATTRIBUTES_MEMBER} Reason: {reason}")
            }
            AttributeMember::Named => format!("Invalid parameter: {reason}"),
        }
    })
}

/// The WHOLE `InvalidParameter` message SNS refuses a subscription attribute with when
/// the TOPIC the subscription belongs to gives it nothing to govern, or `None` when the
/// topic accepts it.
///
/// A `ReplayPolicy` names a window of the topic's message archive to redeliver from
/// (`SubscribeInput$Attributes` doc), so it can only be set where that archive exists: a
/// topic with no archive policy archives nothing. A `RedrivePolicy` names the queue the
/// subscription's undeliverable messages are held in, and a queue takes a message only
/// of its OWN kind — so the dead-letter queue of a subscription is a FIFO queue exactly
/// when its topic is a FIFO topic. Shared by every operation that writes a subscription
/// attribute, so the rule does not depend on which call carried the write.
pub fn subscription_attribute_topic_refusal(
    name: &str,
    value: &str,
    topic: Option<&TopicRecord>,
    member: AttributeMember,
) -> Option<String> {
    if name == REPLAY_POLICY_ATTRIBUTE && topic.is_none_or(|topic| !topic.is_archiving()) {
        return Some(REPLAY_WITHOUT_ARCHIVE_REFUSAL.to_string());
    }
    if name != REDRIVE_POLICY_ATTRIBUTE {
        return None;
    }
    let topic = topic?;
    let target = json_object(value)
        .and_then(|members| {
            members
                .get(DEAD_LETTER_TARGET_ARN)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    (is_fifo_queue_arn(&target) != topic.is_fifo).then(|| {
        let kind = if topic.is_fifo { "FIFO" } else { "standard" };
        member.refusal(
            REDRIVE_POLICY_ATTRIBUTE,
            &format!("{DEAD_LETTER_TARGET_ARN} must be a {kind} SQS queue"),
        )
    })
}

/// The form a subscription attribute is STORED in: what the caller wrote, in the shape
/// SNS reports it back in.
///
/// SNS holds the MEANING of an attribute value rather than the caller's bytes, so a read
/// reports the canonical form: a flag is the lower-case word whichever case it arrived
/// in, and a delivery policy is the whole retry policy with the members the caller left
/// out filled from the account system defaults. Every other attribute is a document or
/// an ARN SNS keeps verbatim.
///
/// Shared by every operation that writes a subscription attribute, so a value does not
/// read back differently for having arrived on `Subscribe` rather than
/// `SetSubscriptionAttributes`. The value is validated FIRST
/// ([`subscription_attribute_refusal`]), so nothing canonicalized here was refusable.
pub fn stored_subscription_attribute_value(name: &str, value: &str) -> String {
    match name {
        RAW_MESSAGE_DELIVERY_ATTRIBUTE => flag_attribute_value(value)
            .unwrap_or(value)
            .to_string(),
        DELIVERY_POLICY_ATTRIBUTE => normalized_subscription_delivery_policy(value),
        _ => value.to_string(),
    }
}

/// The effective delivery policy of one SUBSCRIPTION: the account system defaults, the
/// `DeliveryPolicy` of the topic laid over them, and then the subscription's own
/// `DeliveryPolicy` — "the effective delivery policy that takes into account the topic
/// delivery policy and account system defaults"
/// (`GetSubscriptionAttributesResponse$Attributes` doc).
///
/// It is reported in the SUBSCRIPTION's document shape
/// ([`SubscriptionDeliveryPolicy`]), because it is the policy ONE subscription's
/// deliveries run under: the topic's document contributes the defaults it states for the
/// protocol group (`http.defaultHealthyRetryPolicy`), and the subscription's own retry
/// policy is laid over them.
///
/// A topic whose policy sets `disableSubscriptionOverrides` keeps its own policy: that
/// is what the member disables, so the subscription's policy is not laid over it.
pub fn effective_subscription_delivery_policy(
    topic: Option<&TopicRecord>,
    subscription: &SubscriptionRecord,
) -> String {
    // Infallible for this shape: every member is a number, a string, a bool or a
    // `serde_json::Value`, and no map key is caller-controlled, so the default branch is
    // unreachable rather than an empty policy a client could read as one.
    serde_json::to_string(&effective_subscription_delivery(topic, subscription)).unwrap_or_default()
}

/// The same policy as the document [`effective_subscription_delivery_policy`] reports, as the
/// SHAPE of it: the budget one subscription's deliveries really run under.
///
/// The two are one rule, read two ways — a client reads the document, and a delivery to a
/// subscribed URL reads the retry budget and the request policy out of this
/// ([`crate::ops::publish::endpoint_delivery`]) — so what governs a delivery is exactly what
/// `EffectiveDeliveryPolicy` reports for it.
pub fn effective_subscription_delivery(
    topic: Option<&TopicRecord>,
    subscription: &SubscriptionRecord,
) -> SubscriptionDeliveryPolicy {
    let mut from_topic = DeliveryPolicy::system_default();
    if let Some(configured) = topic
        .and_then(|topic| topic.attributes.get(DELIVERY_POLICY_ATTRIBUTE))
        .and_then(|raw| serde_json::from_str::<ConfiguredDeliveryPolicy>(raw).ok())
    {
        configured.overlay(&mut from_topic);
    }
    let mut effective = SubscriptionDeliveryPolicy {
        healthy_retry_policy: from_topic.http.default_healthy_retry_policy,
        // "the effective delivery policy that takes into account the topic delivery
        // policy": the request the notification is delivered IN is the topic's default
        // one, so the effective policy of a subscription nobody gave a request policy
        // states that default rather than nothing.
        request_policy: Some(from_topic.http.default_request_policy),
        ..SubscriptionDeliveryPolicy::system_default()
    };
    if !from_topic.http.disable_subscription_overrides {
        if let Some(configured) = subscription
            .attributes
            .get(DELIVERY_POLICY_ATTRIBUTE)
            .and_then(|raw| serde_json::from_str::<ConfiguredSubscriptionDeliveryPolicy>(raw).ok())
        {
            configured.overlay(&mut effective);
        }
    }
    effective
}

/// The fields a SUBSCRIPTION's ARN holds: an ARN whose resource is a topic name
/// followed by the subscription's own id segment
/// (`arn:aws:sns:<region>:<account>:<topic>:<id>`, as `Subscribe` mints it).
pub const SUBSCRIPTION_ARN_FIELDS: usize = ARN_ELEMENTS + 1;

/// The sentence SNS refuses an ARN-shaped `SubscriptionArn` that does not name a
/// subscription with: `Invalid parameter:` plus the part of the ARN that is at fault, as
/// SNS names it (`SubscriptionId`, not the Smithy member `SubscriptionArn`), and nothing
/// else — no member frame, no reason clause, and no part of the value the caller sent.
/// Recorded evidence: `GetSubscriptionAttributes` handed a TOPIC ARN answers `400
/// InvalidParameter` with exactly this sentence.
pub const INVALID_SUBSCRIPTION_ID: &str = "Invalid parameter: SubscriptionId";

/// Whether `arn` names a subscription: an ARN whose resource is a topic name followed by
/// the subscription's own id segment ([`SUBSCRIPTION_ARN_FIELDS`] elements).
fn names_a_subscription(arn: &str) -> bool {
    let fields: Vec<&str> = arn.split(':').collect();
    fields.len() == SUBSCRIPTION_ARN_FIELDS
        && fields[2] == "sns"
        && fields[1..].iter().all(|field| !field.is_empty())
        && topic_name_is_valid(fields[5])
}

/// The message a `SubscriptionArn`-typed `member` holding `arn` is refused with, or
/// `None` when `arn` names a subscription.
///
/// Two faults, each stated the way SNS states it. A value that is not an ARN AT ALL is
/// the generic ARN-shape fault every ARN-typed member of this service shares
/// ([`arn_refusal`]): the member and the constraint it failed. An ARN-shaped value whose
/// SUBSCRIPTION ID is not one SNS could have minted — a topic ARN handed to a
/// subscription call, an empty or extra element, a resource that is no topic name — is a
/// fault in that one part of the ARN, so the refusal names that part and nothing else
/// ([`INVALID_SUBSCRIPTION_ID`]). This is the subscription counterpart of the
/// [`INVALID_TOPIC_NAME`] refusal a topic ARN's bad name takes.
///
/// Shared, because every operation that takes a `SubscriptionArn` asks the same
/// question of it — a subscription ARN one call accepts cannot be one another refuses.
pub fn subscription_arn_refusal(member: &str, arn: &str) -> Option<String> {
    if let Some(message) = arn_refusal(member, arn) {
        return Some(message);
    }
    (!names_a_subscription(arn)).then(|| INVALID_SUBSCRIPTION_ID.to_string())
}

// =============================================================================
// The per-message rules every publishing operation shares
// =============================================================================

/// The value of `MessageStructure` that structures a message body by delivery protocol:
/// "Valid value: json" (`PublishInput$MessageStructure` doc).
pub const JSON_MESSAGE_STRUCTURE: &str = "json";

/// Whether `structure` names the one message structure SNS reads a body under.
///
/// The member SELECTS a reading rather than carrying data, and the model constrains it to
/// no value set at all (`messageStructure` is a plain string with no enum or pattern), so
/// a value that is not this one names no structure: the body is then one message for
/// every protocol, not a document of branches. Recorded evidence: a publish carrying a
/// structure value SNS defines nothing for is SERVED, so the member's value is not a
/// closed set this emulator may refuse outside of.
///
/// ONE definition, shared by the message rules and the fan-out, so a body judged as
/// structured is the body delivered as structured.
pub fn is_json_message_structure(structure: &str) -> bool {
    structure == JSON_MESSAGE_STRUCTURE
}

/// The key a JSON-structured message must carry: "contain at least a top-level JSON
/// key of \"default\" with a value that is a string" (`PublishInput$MessageStructure`
/// doc) — the body every protocol no branch names receives. A value that is a scalar
/// rather than a string names that body as the text SNS writes it (see [`default_body`]);
/// one that names no body at all leaves the document with no default entry
/// ([`NO_DEFAULT_ENTRY_REFUSAL`]).
pub const DEFAULT_JSON_KEY: &str = "default";

/// "By default, a message can be at most 256 KiB in size (262,144 bytes)"
/// (`PublishInput$Message` doc); a topic's [`MAXIMUM_MESSAGE_SIZE_ATTRIBUTE`]
/// attribute overrides it.
pub const DEFAULT_MAXIMUM_MESSAGE_SIZE: usize = 262_144;

/// The FIFO attribute that makes SNS derive the `MessageDeduplicationId` from the
/// message body instead of requiring the caller to supply one
/// (`GetTopicAttributesResponse$Attributes` doc).
pub const CONTENT_BASED_DEDUPLICATION_ATTRIBUTE: &str = "ContentBasedDeduplication";

/// The longest `Subject` SNS accepts, in CHARACTERS: "Subjects must be UTF-8 text
/// with no line breaks or control characters, and less than 100 characters long"
/// (`PublishInput$Subject` doc). The limit counts the subject's CHARACTERS, not its
/// UTF-8 bytes, and the recorded service accepts a subject of exactly this many and
/// refuses the next one — so it is an inclusive bound here.
pub const MAX_SUBJECT_CHARS: usize = 100;

/// "The MessageGroupId can contain up to 128 alphanumeric characters (a-z, A-Z, 0-9)
/// and punctuation" — the same rule the doc states for `MessageDeduplicationId`
/// ("When provided, the same validation rules apply").
pub const MAX_ORDERING_ID_CHARS: usize = 128;

/// Whether an ordering id (`MessageGroupId`, `MessageDeduplicationId`) satisfies the
/// rule above: 1 to 128 characters of ASCII alphanumerics and ASCII punctuation,
/// which is exactly the set the doc lists.
fn ordering_id_is_valid(value: &str) -> bool {
    let length = value.chars().count();
    length > 0
        && length <= MAX_ORDERING_ID_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c.is_ascii_punctuation())
}

/// The Unicode LINE SEPARATOR and PARAGRAPH SEPARATOR: the two line breaks that are
/// not control characters, so a rule that reads "no line breaks or control
/// characters" has to name them beside the control category.
const LINE_SEPARATORS: [char; 2] = ['\u{2028}', '\u{2029}'];

/// The one control character a `Subject` may carry: the HORIZONTAL TAB, which the
/// recorded service accepts in a subject where it refuses every other control
/// character — a tab is text a subject line can carry, not a break in it.
const SUBJECT_TAB: char = '\t';

/// Whether `subject` satisfies the rule SNS documents for it: it is "UTF-8 text" —
/// so it carries at least one character that is not blank space — of at most
/// [`MAX_SUBJECT_CHARS`] characters (CHARACTERS, not UTF-8 bytes, so any script is
/// accepted), with "no line breaks or control characters" other than
/// [`SUBJECT_TAB`].
///
/// A subject that is accepted is carried as it was sent: the blank space AROUND the
/// text is part of the subject, so nothing here trims it.
fn subject_is_valid(subject: &str) -> bool {
    subject.chars().count() <= MAX_SUBJECT_CHARS
        // Text, rather than blank space: a subject of none — the empty one, and the
        // one that is only spaces — is no subject line at all.
        && subject.chars().any(|c| !c.is_whitespace())
        && !subject
            .chars()
            .any(|c| (c.is_control() && c != SUBJECT_TAB) || LINE_SEPARATORS.contains(&c))
}

/// The sentence SNS refuses a publish that names no destination with: the members one
/// of which a publish has to carry, and the reason it is refused — "If you don't
/// specify a value for the TopicArn parameter, you must specify a value for the
/// PhoneNumber or TargetArn parameters" (`PublishInput$TopicArn` doc).
pub const NO_DESTINATION_REFUSAL: &str =
    "Invalid parameter: TopicArn or TargetArn Reason: no value for required parameter";

/// The sentence SNS refuses a publish that names MORE than one destination with: a
/// message goes to one destination, so the destination members cannot be combined.
pub const DESTINATIONS_NOT_EXCLUSIVE_REFUSAL: &str =
    "Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn specified. Use \
     only one or the other.";

/// The sentence SNS refuses a publish with no message body with.
///
/// SNS answers one FIXED sentence per refused condition — the parameter as the
/// service names it, and the value or the bound only where that sentence itself
/// carries them — so a client that reads `Error.Message` sees the same text for the
/// same condition on every call. Recorded evidence: `Publish` with `Message=""`
/// answers `400 InvalidParameter` with exactly this.
pub const EMPTY_MESSAGE_REFUSAL: &str = "Invalid parameter: Empty message";

/// The sentence SNS refuses a `MessageStructure: json` body it cannot READ as a structured
/// message with: the condition is about the message STRUCTURE rather than the body's bytes,
/// and the structure is a JSON document of per-protocol bodies — so a body that is not one
/// did not parse as the structure the publish declared.
///
/// It answers both ways a declared structure fails to parse: a body that is not JSON at all,
/// and one that is JSON but not the object of protocol branches the structure names ("use a
/// JSON object for the Message parameter", `PublishInput$Message` doc). Recorded evidence: a
/// publish declaring the json structure over a body that is not a JSON object is refused with
/// exactly this.
pub const JSON_MESSAGE_PARSE_REFUSAL: &str =
    "Invalid parameter: Message Structure - JSON message body failed to parse";

/// The sentence SNS refuses a `MessageStructure: json` body that names no default
/// message with: the condition is about the message STRUCTURE rather than the body's
/// bytes, and the member the structure is missing is the default entry.
///
/// It answers both ways a body can name no default: a document with no `default` member
/// at all, and one whose `default` names no message SNS could deliver (see
/// [`default_body`]) — a composite value is not an entry SNS can deliver, so a document
/// carrying one has no default entry. Recorded evidence: a publish of
/// `{"default":{…}}` under the json structure is refused for having no default entry.
pub const NO_DEFAULT_ENTRY_REFUSAL: &str =
    "Invalid parameter: Message Structure - No default entry in JSON message body";

/// The sentence SNS refuses a body past the topic's ceiling with: the condition, with
/// neither the body's size nor the ceiling in it.
pub const MESSAGE_TOO_LONG_REFUSAL: &str = "Invalid parameter: Message too long";

/// The sentence SNS refuses an unacceptable `Subject` with: `Invalid parameter:` and
/// the parameter, whichever of the member's rules the value broke (its length, a
/// character it may not carry, or carrying no text at all).
pub const INVALID_SUBJECT_REFUSAL: &str = "Invalid parameter: Subject";

/// The sentence SNS refuses a `MessageDeduplicationId` on a STANDARD topic with:
/// deduplication is a FIFO property, so the parameter is not one this topic type
/// takes.
pub const DEDUPLICATION_ID_WRONG_TOPIC_TYPE_REFUSAL: &str =
    "Invalid parameter: MessageDeduplicationId Reason: The request includes \
     MessageDeduplicationId parameter that is not valid for this topic type";

/// The sentence SNS refuses an ordering id (`MessageGroupId`,
/// `MessageDeduplicationId`) that is not 1 to [`MAX_ORDERING_ID_CHARS`] alphanumeric
/// or punctuation characters with: it names the parameter and states the rule, and
/// never the value the caller sent.
pub fn ordering_id_refusal(member: &str) -> String {
    format!(
        "Invalid parameter: The {member} parameter can only include alphanumeric and \
         punctuation characters. 1 to {MAX_ORDERING_ID_CHARS} in length"
    )
}

/// One message as a caller supplied it: the members `PublishInput` and
/// `PublishBatchRequestEntry` have in common, which is every member either shape
/// carries a documented per-message rule for. Borrowed, so judging a message costs no
/// clone.
#[derive(Debug, Clone, Copy)]
pub struct MessageRequest<'a> {
    pub message: &'a str,
    pub subject: Option<&'a str>,
    pub message_structure: Option<&'a str>,
    pub message_group_id: Option<&'a str>,
    pub message_deduplication_id: Option<&'a str>,
    /// The attributes published WITH the message, which carry rules of their own and
    /// count toward the size the message is measured at ([`message_size`]).
    pub message_attributes: Option<&'a HashMap<String, MessageAttributeValue>>,
}

/// The SIZE SNS measures one message at: "All parts of the message attribute, including
/// name, type, and value, are included in the message size restriction"
/// (`MessageAttributeValue` doc), beside the body's own bytes — and nothing else of the
/// request, so the `Subject` an email endpoint is sent is not part of it.
pub fn message_size(request: MessageRequest<'_>) -> usize {
    request.message.len()
        + request
            .message_attributes
            .map_or(0, message_attribute_bytes)
}

/// Why SNS refuses this message on its own members alone, or `None` when every
/// documented per-member rule holds.
///
/// This is ARGUMENT validation — it judges the message without looking at any record —
/// so it runs before the topic is read, and it is SHARED by every operation that
/// publishes a message: a message `Publish` accepts cannot be one `PublishBatch`
/// refuses. The reason names the member and the constraint it failed, built from the
/// request.
pub fn message_refusal(request: MessageRequest<'_>) -> Option<String> {
    // The one structure SNS defines for a message body is `json`, and the model
    // constrains the member to no value set at all (`messageStructure` is a plain
    // string): a value that names no structure SNS reads leaves the message ONE body for
    // every protocol rather than failing the publish.
    if request
        .message_structure
        .is_some_and(is_json_message_structure)
    {
        if !is_json_message_document(request.message) {
            return Some(JSON_MESSAGE_PARSE_REFUSAL.to_string());
        }
        // "contain at least a top-level JSON key of "default"" that names a message:
        // the body every protocol no other member names receives. A document without
        // one has no default entry, whether the member is absent or names no message.
        if default_branch_body(request.message).is_none() {
            return Some(NO_DEFAULT_ENTRY_REFUSAL.to_string());
        }
    }

    if request.message.is_empty() {
        return Some(EMPTY_MESSAGE_REFUSAL.to_string());
    }

    if request.subject.is_some_and(|subject| !subject_is_valid(subject)) {
        return Some(INVALID_SUBJECT_REFUSAL.to_string());
    }

    // The ordering ids follow one rule, whatever the topic type, and it is also what
    // keeps a retained delivery's own size bounded.
    for (member, value) in [
        ("MessageGroupId", request.message_group_id),
        ("MessageDeduplicationId", request.message_deduplication_id),
    ] {
        if value.is_some_and(|value| !ordering_id_is_valid(value)) {
            return Some(ordering_id_refusal(member));
        }
    }

    None
}

/// The refusal a published message's ATTRIBUTES answer, or `None` when every one of them
/// is one SNS accepts.
///
/// Held apart from [`message_refusal`] because SNS answers a message attribute it will
/// not take with its own modelled errors
/// ([`message_attributes::MessageAttributeRefusal`]) rather than always with
/// `InvalidParameterException`, so each publishing operation lifts this refusal into the
/// error it names. The rules themselves are shared
/// ([`message_attributes::message_attributes_refusal`]).
pub fn message_attributes_refusal(
    request: MessageRequest<'_>,
) -> Option<message_attributes::MessageAttributeRefusal> {
    request
        .message_attributes
        .and_then(message_attributes::message_attributes_refusal)
}

/// The ceiling, in bytes, on what `topic` accepts: [`DEFAULT_MAXIMUM_MESSAGE_SIZE`],
/// which the topic's [`MAXIMUM_MESSAGE_SIZE_ATTRIBUTE`] may only LOWER.
///
/// The attribute reaches 1 MiB at AWS, but a caller-raised ceiling would let one
/// request buy more memory per subscription than the documented default, so the
/// emulator keeps the default as its hard limit.
pub fn max_message_size(topic: &TopicRecord) -> usize {
    topic
        .attributes
        .get(MAXIMUM_MESSAGE_SIZE_ATTRIBUTE)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(DEFAULT_MAXIMUM_MESSAGE_SIZE, |configured| {
            configured.min(DEFAULT_MAXIMUM_MESSAGE_SIZE)
        })
}

/// What `topic` makes of this message: the `MessageDeduplicationId` the stored
/// delivery carries (`None` for a standard topic, which SNS does not sequence or
/// deduplicate), or why this topic refuses the message.
///
/// These are the rules that need the TOPIC as well as the message — its size ceiling
/// and the FIFO ordering parameters — so they run after the topic record is read, and
/// they too are shared by every publishing operation.
pub fn topic_message_refusal(
    topic: &TopicRecord,
    request: MessageRequest<'_>,
) -> Result<Option<String>, String> {
    let ceiling = max_message_size(topic);
    if message_size(request) > ceiling {
        return Err(MESSAGE_TOO_LONG_REFUSAL.to_string());
    }

    if !topic.is_fifo {
        if request.message_deduplication_id.is_some() {
            return Err(DEDUPLICATION_ID_WRONG_TOPIC_TYPE_REFUSAL.to_string());
        }
        // A MessageGroupId is NOT refused here: for a standard topic "the
        // MessageGroupId is optional and is forwarded only to Amazon SQS standard
        // subscriptions to activate fair queues" (`PublishInput$MessageGroupId` doc),
        // so it is stored with the delivery rather than rejected.
        return Ok(None);
    }

    if request.message_group_id.is_none() {
        // The sentence SNS states for the ordering parameter a FIFO publish must
        // carry.
        return Err(
            "Invalid parameter: The MessageGroupId parameter is required for FIFO topics"
                .to_string(),
        );
    }

    let content_based = topic
        .attributes
        .get(CONTENT_BASED_DEDUPLICATION_ATTRIBUTE)
        .is_some_and(|value| flag_attribute_enabled(value));
    match request.message_deduplication_id {
        Some(id) => Ok(Some(id.to_string())),
        // "Amazon SNS uses a SHA-256 hash to generate the MessageDeduplicationId using
        // the body of the message (but not the attributes of the message)"
        // (`GetTopicAttributesResponse$Attributes` doc).
        None if content_based => Ok(Some(hex::encode(Sha256::digest(
            request.message.as_bytes(),
        )))),
        None => Err(
            "Invalid parameter: MessageDeduplicationId - required for a FIFO topic whose \
             ContentBasedDeduplication is not enabled"
                .to_string(),
        ),
    }
}

/// The bytes one message's attribute map holds: every name, type and value, which is
/// what SNS counts beside the body when it measures a message against a topic's
/// `MaximumMessageSize` ("including each message's body and attributes" — the
/// `PublishBatch` doc).
pub fn message_attribute_bytes(attributes: &HashMap<String, MessageAttributeValue>) -> usize {
    attributes
        .iter()
        .map(|(name, value)| {
            name.len()
                + value.data_type.len()
                + value.string_value.as_ref().map_or(0, String::len)
                + value.binary_value.as_ref().map_or(0, Vec::len)
        })
        .sum()
}

/// The data type of a numeric message attribute: "Number – Numbers are positive or
/// negative integers or floating-point numbers" (`MessageAttributeValue$DataType` doc).
pub const NUMBER_DATA_TYPE: &str = "Number";

/// The data type of a textual message attribute: "String – Strings are Unicode with UTF-8
/// binary encoding" (`MessageAttributeValue$DataType` doc).
pub const STRING_DATA_TYPE: &str = "String";

/// The data type of a message attribute carrying BYTES: "Binary – Binary type attributes
/// can store any binary data" (`MessageAttributeValue$DataType` doc).
pub const BINARY_DATA_TYPE: &str = "Binary";

/// The data type of a message attribute carrying a LIST of values: "String.Array –
/// Strings, numbers, true, false and null" (`MessageAttributeValue$DataType` doc), written
/// as a JSON array.
pub const STRING_ARRAY_DATA_TYPE: &str = "String.Array";

/// What separates a data type from the custom label a publisher may append to it
/// (`Number.int`): "You can also append custom labels"
/// (`MessageAttributeValue$DataType` doc).
pub const DATA_TYPE_LABEL_SEPARATOR: char = '.';

/// The data type `value` carries, without the custom label a publisher may have appended
/// to it — so `Number.int` is a `Number` wherever a type decides what a value MEANS.
pub fn attribute_data_type(value: &MessageAttributeValue) -> &str {
    value
        .data_type
        .split(DATA_TYPE_LABEL_SEPARATOR)
        .next()
        .unwrap_or(&value.data_type)
}

/// One message `Publish` FANNED OUT to one subscription.
///
/// What a publish handed to the subscription, not what the subscription's endpoint received:
/// the record is written for every confirmed subscription of the topic, including the
/// protocols this emulator has no delivery path for and the ones whose endpoint refuses or
/// never grants the delivery. What an endpoint really holds is observed on the endpoint.
///
/// The message body and its attributes are held behind an `Arc`, because one publish is
/// recorded against EVERY subscription of the topic: sharing them keeps one publish's cost
/// one body and N pointers rather than N copies of a body that may be up to the topic's
/// `MaximumMessageSize`, which is what bounds the (subscriptions x body bytes) product of a
/// single request. The body is immutable once published, so there is nothing to clone it for.
#[derive(Debug, Clone)]
pub struct PublishedMessage {
    pub message_id: String,
    pub topic_arn: String,
    pub message: ::std::sync::Arc<str>,
    pub subject: Option<String>,
    pub message_structure: Option<String>,
    pub message_attributes: ::std::sync::Arc<HashMap<String, MessageAttributeValue>>,
    pub message_group_id: Option<String>,
    pub message_deduplication_id: Option<String>,
    /// Set only for a FIFO topic, which is the only kind SNS sequences.
    pub sequence_number: Option<String>,
    pub published_at: SystemTime,
}

impl PublishedMessage {
    /// Every byte retaining this message costs, as [`MAX_RETAINED_DELIVERY_BYTES`]
    /// counts them: EVERY variable-length field of the stored record, since each one
    /// is either caller-supplied or derived from caller input, plus a flat allowance
    /// for the fixed-size parts. A field left out of this sum is a field a caller
    /// could grow without the ceiling noticing.
    ///
    /// The shared body is counted in FULL for every record that holds a pointer to it, which
    /// over-counts what is actually held: the ceiling is a bound, so counting the sharing
    /// would weaken it, and a bound that errs high evicts sooner rather than later.
    pub fn retained_bytes(&self) -> usize {
        /// The fixed-size parts of the record (the timestamp, the `Option`/`String`
        /// headers, the map's own allocation) that no caller varies.
        const FIXED_OVERHEAD: usize = 256;

        let attributes = message_attribute_bytes(&self.message_attributes);
        FIXED_OVERHEAD
            + self.message_id.len()
            + self.topic_arn.len()
            + self.message.len()
            + self.subject.as_ref().map_or(0, String::len)
            + self.message_structure.as_ref().map_or(0, String::len)
            + self.message_group_id.as_ref().map_or(0, String::len)
            + self.message_deduplication_id.as_ref().map_or(0, String::len)
            + self.sequence_number.as_ref().map_or(0, String::len)
            + attributes
    }
}

// =============================================================================
// FIFO deduplication
// =============================================================================

/// How long one accepted deduplication id keeps collapsing its duplicates: "a token used
/// for deduplication of sent messages within the 5 minute minimum deduplication interval"
/// (`PublishInput$MessageDeduplicationId` doc).
pub const DEDUPLICATION_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The topic attribute that decides how WIDE a FIFO topic's deduplication scope is
/// (`CreateTopicInput$Attributes` doc).
pub const FIFO_THROUGHPUT_SCOPE_ATTRIBUTE: &str = "FifoThroughputScope";

/// The throughput scope that deduplicates within ONE message group rather than across the
/// topic: "MessageGroup – The scope of deduplication is within each individual message
/// group" (`CreateTopicInput$Attributes` doc). The other value, `Topic`, "is the default
/// value", so a topic whose owner set no scope deduplicates across the whole topic.
pub const FIFO_THROUGHPUT_SCOPE_MESSAGE_GROUP: &str = "MessageGroup";

/// How many live deduplication records the store retains.
///
/// The records are the emulator's own memory of what each scope accepted, and a caller
/// can mint one per publish, so the set is bounded like every other caller-grown store
/// here: the oldest record is evicted when a new one would cross this count, rather than
/// the publish being refused — a publish AWS accepts is not one this emulator rejects.
/// An evicted record stops collapsing its duplicates, which is why the count is far above
/// the number of distinct ids any one caller sends inside one
/// [`DEDUPLICATION_INTERVAL`].
pub const MAX_DEDUPLICATION_RECORDS: usize = 10_000;

/// What one accepted publish left behind for its duplicates to answer with: the message
/// id and sequence number SNS minted for the FIRST copy, and when that copy was accepted.
#[derive(Debug, Clone)]
pub struct DeduplicationRecord {
    pub message_id: String,
    pub sequence_number: Option<String>,
    pub accepted_at: SystemTime,
}

/// The key one publish deduplicates under: the topic, the scope the topic deduplicates
/// in, and the message's deduplication id.
///
/// "The scope of deduplication depends on the FifoThroughputScope attribute, when set to
/// Topic the message deduplication scope is across the entire topic, when set to
/// MessageGroup the message deduplication scope is within each individual message group"
/// (`PublishInput$MessageDeduplicationId` doc).
///
/// Shared by every publishing operation, because a copy one of them accepted is a
/// duplicate for the other: `PublishBatch` deduplicates "within the batch and across
/// batches" (the `PublishBatch` doc) against the same records `Publish` writes.
pub fn deduplication_key(
    topic: &TopicRecord,
    message_group_id: Option<&str>,
    deduplication_id: &str,
) -> String {
    let per_group = topic
        .attributes
        .get(FIFO_THROUGHPUT_SCOPE_ATTRIBUTE)
        .is_some_and(|scope| scope == FIFO_THROUGHPUT_SCOPE_MESSAGE_GROUP);
    let group = if per_group {
        message_group_id.unwrap_or_default()
    } else {
        ""
    };
    // The separator is a character neither an ARN nor an ordering id may carry (an
    // ordering id is alphanumerics and punctuation), so two different triples cannot
    // spell one key.
    format!("{}\n{group}\n{deduplication_id}", topic.arn)
}

// =============================================================================
// The SMS sandbox
// =============================================================================

/// The verification status a destination phone number in the SMS sandbox carries
/// until its one-time password has been verified.
///
/// One of the two values the modelled `SMSSandboxPhoneNumberVerificationStatus` enum
/// accepts; the other is [`SANDBOX_NUMBER_VERIFIED`]. Both are spelled exactly as
/// [`crate::ops::types::S_M_S_SANDBOX_PHONE_NUMBER_VERIFICATION_STATUS_VALUES`]
/// lists them, because that is what the enum lets a listing report.
pub const SANDBOX_NUMBER_PENDING: &str = "Pending";

/// The verification status a destination phone number in the SMS sandbox carries
/// once its one-time password has been verified — the other value of the enum named
/// on [`SANDBOX_NUMBER_PENDING`].
pub const SANDBOX_NUMBER_VERIFIED: &str = "Verified";

/// The name a refusal gives a sandbox destination phone number record.
pub const SANDBOX_NUMBER_RECORD: &str = "SMS sandbox destination phone numbers";

/// How many decimal digits a one-time password this service delivers to a sandbox
/// destination number carries.
///
/// `smithy_query(query="shape", shape="OTPCode")` reports `length {min: 5, max: 8}`
/// and pattern `^[0-9]+$`, so a code must be five to eight digits and nothing else;
/// six is the length SNS's own sandbox verification page asks a customer to enter,
/// and it sits inside those bounds.
pub const SANDBOX_OTP_DIGITS: usize = 6;

/// The sentence a verification carrying a one-time password that is not the one
/// currently delivered to that destination number is refused with.
///
/// This service keeps ONE live code per destination number — a later
/// `CreateSMSSandboxPhoneNumber` delivers a new one and retires the last — so any
/// code but that one is a code that is no longer good, which is how the refusal is
/// worded. Fixed text with no variable part: it reproduces NEITHER the code the
/// caller sent nor the one that was delivered, so a refusal never leaks the secret
/// it is judging, and not the destination number either (the model marks that member
/// `@sensitive`). Recorded evidence: `VerifySMSSandboxPhoneNumber` presented with a
/// code that is not the delivered one answers `400 VerificationException` with
/// exactly this sentence.
pub const INVALID_OTP_MESSAGE: &str =
    "Verification code has expired, cannot verify the destination phone number.";

/// What a refused verification reports in the modelled `Status` member, "the status
/// of the verification error" (`VerificationException$Status` doc).
///
/// The recorded refusal pins the SENTENCE ([`INVALID_OTP_MESSAGE`]) and carries no
/// evidence about this member, so the value is not reproduced from a recording: it
/// names the condition that sentence states, so the member and the sentence cannot
/// disagree.
pub const INVALID_OTP_STATUS: &str = "Expired";

/// How many times [`ComAmazonawsSnsAmazonsimplenotificationserviceState::next_sandbox_one_time_password`]
/// re-mints a code one of this account's sandbox numbers already holds.
///
/// Reducing an id to [`SANDBOX_OTP_DIGITS`] digits can land on a code that is
/// already live, and a repeat would let a retired code keep verifying, so the mint
/// tries again — bounded, so a caller cannot make the loop run on. Eight attempts
/// against at most [`MAX_RECORDS_PER_STORE`] live codes out of the ten-to-the-six a
/// code can take leaves nothing for the fall-through to do.
const SANDBOX_OTP_MINT_ATTEMPTS: usize = 8;

/// One destination phone number in the calling account's SMS sandbox, as
/// `CreateSMSSandboxPhoneNumber` added it.
#[derive(Debug, Clone)]
pub struct SandboxPhoneNumberRecord {
    /// The destination phone number, as the request that added it spelled it. The
    /// model marks this member `@sensitive`, so it belongs in the modelled listing
    /// and in no error string.
    pub phone_number: String,
    /// [`SANDBOX_NUMBER_PENDING`] or [`SANDBOX_NUMBER_VERIFIED`].
    pub status: String,
    /// The one-time password last delivered to this destination number — the code
    /// `VerifySMSSandboxPhoneNumber` checks the caller's against.
    ///
    /// AWS delivers it by SMS, to the number itself, so it is never on the wire in
    /// either direction but the caller's: this field is this emulator's stand-in for
    /// that carrier. No response carries it and no refusal names it.
    pub one_time_password: String,
    pub created_at: SystemTime,
}

/// Whether an account that has just started using SNS for SMS is in the SMS sandbox:
/// "When you start using Amazon SNS to send SMS messages, your Amazon Web Services
/// account is in the SMS sandbox" (`GetSMSSandboxAccountStatus` doc), so a freshly
/// booted emulator serves an account that is in it.
pub const IN_SMS_SANDBOX_AT_ACCOUNT_START: bool = true;

/// The calling account's SMS sandbox membership, which
/// `GetSMSSandboxAccountStatus` reports.
///
/// Membership is ACCOUNT state, not a property of any one destination number: the
/// sandbox "provides a safe environment for you to try Amazon SNS features", and
/// while an account is in it SMS messages reach only verified destination numbers.
/// No SNS operation moves an account out of it — leaving the sandbox is a separate,
/// unmodelled request for production access — so nothing a caller does to the
/// destination numbers in [`ComAmazonawsSnsAmazonsimplenotificationserviceState::sandbox_phone_numbers`]
/// changes it, and the record an account starts with is
/// [`IN_SMS_SANDBOX_AT_ACCOUNT_START`].
#[derive(Debug, Clone, Copy)]
pub struct SmsSandboxAccountRecord {
    /// Whether the calling account is in the SMS sandbox.
    pub is_in_sandbox: bool,
}

impl Default for SmsSandboxAccountRecord {
    fn default() -> Self {
        Self {
            is_in_sandbox: IN_SMS_SANDBOX_AT_ACCOUNT_START,
        }
    }
}

// =============================================================================
// SMS: the calling account's message settings
// =============================================================================

/// The default type of SMS message the account sends, one of
/// [`DEFAULT_SMS_TYPES`].
pub const DEFAULT_SMS_TYPE_ATTRIBUTE: &str = "DefaultSMSType";

/// The string displayed as the sender on the receiving device.
pub const DEFAULT_SENDER_ID_ATTRIBUTE: &str = "DefaultSenderID";

/// The ARN of the IAM role that lets SNS write SMS delivery logs.
pub const DELIVERY_STATUS_IAM_ROLE_ATTRIBUTE: &str = "DeliveryStatusIAMRole";

/// The percentage of successful SMS deliveries SNS writes logs for, between
/// [`MIN_SAMPLING_RATE`] and [`MAX_SAMPLING_RATE`].
pub const DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE: &str =
    "DeliveryStatusSuccessSamplingRate";

/// The maximum amount in USD the account is willing to spend on SMS each month.
pub const MONTHLY_SPEND_LIMIT_ATTRIBUTE: &str = "MonthlySpendLimit";

/// The name of the S3 bucket that receives the daily SMS usage report.
pub const USAGE_REPORT_S3_BUCKET_ATTRIBUTE: &str = "UsageReportS3Bucket";

/// The attribute names SNS defines for the calling account's SMS settings: "You can
/// set values for the following attribute names: `MonthlySpendLimit` …
/// `DeliveryStatusIAMRole` … `DeliveryStatusSuccessSamplingRate` …
/// `DefaultSenderID` … `DefaultSMSType` … `UsageReportS3Bucket`"
/// (`SetSMSAttributesInput$attributes` doc — the full text is
/// `smithy_query(query="doc", shape="SetSMSAttributesInput", member="attributes")`).
///
/// A closed set, like [`TOPIC_ATTRIBUTE_NAMES`] and [`SUBSCRIPTION_ATTRIBUTE_NAMES`]:
/// SNS refuses any other name, which is also what keeps one account's settings record
/// from growing as a caller invents fresh names for it.
pub const SMS_ATTRIBUTE_NAMES: &[&str] = &[
    DEFAULT_SENDER_ID_ATTRIBUTE,
    DEFAULT_SMS_TYPE_ATTRIBUTE,
    DELIVERY_STATUS_IAM_ROLE_ATTRIBUTE,
    DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE,
    MONTHLY_SPEND_LIMIT_ATTRIBUTE,
    USAGE_REPORT_S3_BUCKET_ATTRIBUTE,
];

/// The values [`DEFAULT_SMS_TYPE_ATTRIBUTE`] accepts: "`Promotional` – (Default)
/// Noncritical messages … `Transactional` – Critical messages that support customer
/// transactions" (`SetSMSAttributesInput$attributes` doc), in that documented order.
pub const DEFAULT_SMS_TYPES: &[&str] = &["Promotional", "Transactional"];

/// The lowest [`DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE`]: "The value can be
/// an integer from 0 - 100" (`SetSMSAttributesInput$attributes` doc).
pub const MIN_SAMPLING_RATE: u32 = 0;

/// The highest [`DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE`], from the same
/// documented range as [`MIN_SAMPLING_RATE`].
pub const MAX_SAMPLING_RATE: u32 = 100;

/// The fewest characters a [`DEFAULT_SENDER_ID_ATTRIBUTE`] carries: "The sender ID can
/// be 1 - 11 alphanumeric characters, and it must contain at least one letter"
/// (`SetSMSAttributesInput$attributes` doc).
pub const MIN_SENDER_ID_CHARS: usize = 1;

/// The most characters a [`DEFAULT_SENDER_ID_ATTRIBUTE`] carries, from the same
/// documented range as [`MIN_SENDER_ID_CHARS`].
pub const MAX_SENDER_ID_CHARS: usize = 11;

/// The name a refusal gives the calling account's SMS settings record, in the place
/// [`TOPIC_RECORD`] and [`SUBSCRIPTION_RECORD`] take for the records they name.
pub const SMS_SETTINGS_RECORD: &str = "an account's SMS configuration";

/// The [`MONTHLY_SPEND_LIMIT_ATTRIBUTE`] an account has before anyone sets one: "By
/// default, the spend limit is set to the maximum allowed by Amazon SNS. If you want
/// to raise the limit, submit an SNS Limit Increase case"
/// (`SetSMSAttributesInput$attributes` doc) — the maximum an account is allowed before
/// such a case is granted is 1.00 USD per month, which the SNS Developer Guide
/// publishes as an account's SMS spend quota.
///
/// So a spend limit is a setting an account ALWAYS has, unlike the settings SNS
/// applies only once an account has been given them: a report of an account's SMS
/// settings carries this one whether or not a caller set it
/// ([`SmsSettingsRecord::reported_attributes`]).
pub const DEFAULT_MONTHLY_SPEND_LIMIT: &str = "1";

/// The calling account's settings for sending SMS messages: what `SetSMSAttributes`
/// writes and `GetSMSAttributes` reports.
///
/// NOT caller-grown — [`SMS_ATTRIBUTE_NAMES`] is a closed set, so the map holds at
/// most that many entries — and bounded in BYTES by [`MAX_ATTRIBUTE_BYTES_PER_RECORD`]
/// like every other attribute map here, so a caller cannot grow one setting without
/// limit either.
#[derive(Debug, Clone, Default)]
pub struct SmsSettingsRecord {
    /// The settings the account has been GIVEN, keyed by attribute name: a setting it
    /// has never been given, and one a write of an empty value cleared, is absent. The
    /// settings a report carries are [`Self::reported_attributes`], which is what a
    /// caller reads.
    pub attributes: HashMap<String, String>,
}

impl SmsSettingsRecord {
    /// The settings a report of this account carries: every setting the account has
    /// been given, and the spend limit whether or not it has been given one.
    ///
    /// A spend limit is a setting an account ALWAYS has — "By default, the spend limit
    /// is set to the maximum allowed by Amazon SNS" — so it is reported from
    /// [`DEFAULT_MONTHLY_SPEND_LIMIT`] until a caller sets one and again once a caller
    /// clears it, unlike every other SMS setting, which SNS applies only once the
    /// account has been given it and which a report therefore carries only then.
    pub fn reported_attributes(&self) -> HashMap<String, String> {
        let mut attributes = self.attributes.clone();
        attributes
            .entry(MONTHLY_SPEND_LIMIT_ATTRIBUTE.to_string())
            .or_insert_with(|| DEFAULT_MONTHLY_SPEND_LIMIT.to_string());
        attributes
    }
}

/// Whether `name` is an attribute SNS defines for an account's SMS settings.
pub fn is_sms_attribute(name: &str) -> bool {
    SMS_ATTRIBUTE_NAMES.contains(&name)
}

/// Whether writing `value` to an SMS setting CLEARS it rather than giving it a value.
///
/// A setting is cleared by writing an EMPTY value to it — the way AWS documents
/// removing an attribute whose value is a document ([`clears_attribute`]) — so the
/// account is left as it was before it was ever given that setting: SNS applies to it
/// what it applies to an account that has not set it, and a report of the account's
/// settings carries no entry for it.
///
/// The rule is here rather than in the handler so the read path and the write path
/// agree on what the account holds: a cleared setting is one the record does not hold,
/// never one it holds empty.
pub fn clears_sms_attribute(value: &str) -> bool {
    value.is_empty()
}

/// The sentence SNS refuses a name that is no setting of an account's SMS
/// configuration with: the name the caller sent, and that it is not one.
///
/// Recorded evidence: `SetSMSAttributes` carrying a name outside
/// [`SMS_ATTRIBUTE_NAMES`] answers `400 InvalidParameter` with exactly this sentence
/// for that name.
pub fn invalid_sms_attribute_name_message(name: &str) -> String {
    format!("{name} is not a valid attribute")
}

/// The sentence SNS refuses a value an SMS setting does not accept with: the setting
/// whose value failed, and that the value is not one of its own.
///
/// ONE sentence for the whole class — whichever setting, and whichever of the
/// constraints [`sms_attribute_value_accepted`] holds it to, the refusal names the
/// setting and nothing else. Recorded evidence: `SetSMSAttributes` carrying a
/// `DefaultSMSType` outside [`DEFAULT_SMS_TYPES`] answers `400 InvalidParameter` with
/// exactly this sentence for that setting.
pub fn invalid_sms_attribute_value_message(name: &str) -> String {
    format!("{name} is invalid")
}

/// Whether the SMS setting `name` accepts `value`, under the constraint SNS documents
/// for that setting — and so for every `name` it documents no constraint on the value
/// of.
///
/// ARGUMENT validation: it judges the value alone, so a whole request is judged before
/// any of it is written. An EMPTY value is accepted whatever the setting, because such
/// a write CLEARS the setting ([`clears_sms_attribute`]) rather than giving it a value
/// to judge.
pub fn sms_attribute_value_accepted(name: &str, value: &str) -> bool {
    if clears_sms_attribute(value) {
        return true;
    }
    match name {
        DEFAULT_SMS_TYPE_ATTRIBUTE => DEFAULT_SMS_TYPES.contains(&value),
        // "The value can be an integer from 0 - 100": a percentage, so a value that is
        // not a whole number in that range governs no sampling.
        DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE => value
            .parse::<u32>()
            .is_ok_and(|rate| (MIN_SAMPLING_RATE..=MAX_SAMPLING_RATE).contains(&rate)),
        // "The sender ID can be 1 - 11 alphanumeric characters, and it must contain at
        // least one letter."
        DEFAULT_SENDER_ID_ATTRIBUTE => is_sender_id(value),
        // "The maximum amount in USD that you are willing to spend each month": an
        // amount, so a value that is not a non-negative number is not a spend limit.
        MONTHLY_SPEND_LIMIT_ATTRIBUTE => is_usd_amount(value),
        // "The ARN of the IAM role that allows Amazon SNS to write logs …" — only the
        // ARN's shape is judged here; whether that role exists is not this service's
        // record to check.
        DELIVERY_STATUS_IAM_ROLE_ATTRIBUTE => arn_shape_reason(value).is_none(),
        // "The name of the Amazon S3 bucket to receive daily SMS usage reports": SNS
        // documents no constraint on the name beyond its being one, and whether the
        // bucket exists is not this service's record to check.
        _ => true,
    }
}

/// The message a write of the SMS setting `name` to `value` is refused with, or `None`
/// when the setting accepts the value.
pub fn sms_attribute_refusal(name: &str, value: &str) -> Option<String> {
    (!sms_attribute_value_accepted(name, value)).then(|| invalid_sms_attribute_value_message(name))
}

/// Whether `value` is a sender id: [`MIN_SENDER_ID_CHARS`] to [`MAX_SENDER_ID_CHARS`]
/// alphanumeric characters, at least one of them a letter.
fn is_sender_id(value: &str) -> bool {
    let characters = value.chars().count();
    (MIN_SENDER_ID_CHARS..=MAX_SENDER_ID_CHARS).contains(&characters)
        && value.chars().all(|character| character.is_alphanumeric())
        && value.chars().any(char::is_alphabetic)
}

/// Whether `value` is an amount in USD: digits, optionally followed by a decimal point
/// and digits — a spend limit is an amount of money, so it carries no sign and no
/// other text.
fn is_usd_amount(value: &str) -> bool {
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (value, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    digits(whole) && fraction.is_none_or(digits)
}

// =============================================================================
// Phone numbers: the E.164 rule and the SMS opt-out list
// =============================================================================

/// The parameter name SNS states in a refusal of a phone number, whatever case the
/// operation spells the member in on the wire (`phoneNumber` on the opt-out pair,
/// `PhoneNumber` on the sandbox and publish ones).
///
/// Recorded evidence: `OptInPhoneNumber` with a value that is not an E.164 number
/// answers `400 InvalidParameter` naming `PhoneNumber`.
pub const PHONE_NUMBER_PARAMETER: &str = "PhoneNumber";

/// The most digits a phone number in E.164 format carries, country code included:
/// ITU-T Recommendation E.164 caps an international public telecommunication number
/// at fifteen digits.
pub const MAX_E164_DIGITS: usize = 15;

/// The fewest digits this service accepts after the `+` of a phone number in E.164
/// format, taken from SNS's own published constraint on a destination phone number:
/// `smithy_query(query="shape", shape="PhoneNumberString")` reports the pattern
/// `^(\+[0-9]{8,}|[0-9]{0,9})$`, whose E.164 branch takes eight digits at least.
pub const MIN_E164_DIGITS: usize = 8;

/// Whether `value` is a phone number in E.164 format: a `+`, then a country code
/// that does not begin with `0`, then digits only — between [`MIN_E164_DIGITS`] and
/// [`MAX_E164_DIGITS`] digits in all.
///
/// Every SNS operation whose phone-number member the model leaves an unconstrained
/// string documents it as "Use E.164 format", so this is the rule those members are
/// held to, and it is ARGUMENT validation — the phone-number counterpart of
/// [`arn_shape_reason`]: a value that could not be a phone number anywhere is bad
/// input, judged BEFORE the store is consulted.
///
/// Deliberately STRICTER than the SMS subscription endpoint's rule
/// (`is_phone_number` in `crate::ops::subscribe`), which accepts a national form
/// written with separators and no `+`: `Subscribe` documents its `sms` endpoint as "a
/// phone number of an SMS-enabled device", while these members say E.164 and nothing
/// else. The two floors differ for the same reason — this one is the digit count SNS
/// publishes for an E.164 destination number.
pub fn is_e164(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('+') else {
        return false;
    };
    (MIN_E164_DIGITS..=MAX_E164_DIGITS).contains(&digits.len())
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// The opening every refusal of a phone-number parameter carries: `Invalid
/// parameter:`, the parameter as SNS names it, and the `Reason:` the clauses below
/// complete — so the parameter name has ONE source for every such sentence.
fn phone_number_refusal_opening() -> String {
    format!("Invalid parameter: {PHONE_NUMBER_PARAMETER} Reason: ")
}

/// The sentence SNS refuses a phone number that is not in E.164 format with.
///
/// Fixed, whichever clause of [`is_e164`] the value broke — SNS states the parameter
/// and that its input was formatted wrongly, and does NOT reproduce the value the
/// caller sent (the model marks every phone-number member `@sensitive`, so echoing it
/// would put a sensitive value in an error string). Recorded evidence:
/// `OptInPhoneNumber` with a value that is not an E.164 number answers `400
/// InvalidParameter` with exactly this sentence.
pub fn invalid_phone_number_message() -> String {
    format!("{}input incorrectly formatted", phone_number_refusal_opening())
}

/// The message a phone-number member holding `value` is refused with, or `None` when
/// `value` is a phone number in E.164 format — the phone-number counterpart of
/// [`arn_refusal`].
pub fn phone_number_refusal(value: &str) -> Option<String> {
    (!is_e164(value)).then(invalid_phone_number_message)
}

/// How long after an opt-in the next one on the same phone number may happen: "You
/// can opt in a phone number only once every 30 days" (`OptInPhoneNumber` doc).
pub const OPT_IN_INTERVAL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The sentence an opt-in is refused with while the number's previous opt-in is
/// younger than [`OPT_IN_INTERVAL`]: the parameter and the documented interval it
/// failed, with none of the (sensitive) number echoed back.
///
/// The interval is documented; the error SHAPE is this emulator's reading of the
/// operation's modelled error set — `InvalidParameterException` is the one client
/// error it declares for a parameter that does not comply, and no recorded response
/// of a too-soon opt-in is available to fix either the shape or this wording. A
/// recording that shows otherwise corrects both here.
pub fn opt_in_too_soon_message() -> String {
    let days = OPT_IN_INTERVAL.as_secs() / (24 * 60 * 60);
    format!(
        "{}A phone number can be opted in only once every {days} days",
        phone_number_refusal_opening()
    )
}

/// What this account knows about one phone number's SMS opt-out state.
///
/// A number opts OUT out of band — the recipient replies `STOP` to the message — and
/// no SNS API performs that, so nothing a caller sends creates a record here. What a
/// caller can do is opt a number back in, which is why the record outlives the
/// opt-out it started as: [`Self::last_opt_in`] is what the
/// once-every-[`OPT_IN_INTERVAL`] rule is measured from.
#[derive(Debug, Clone)]
pub struct PhoneNumberRecord {
    /// The phone number, as the opt-out that created the record spelled it. The model
    /// marks every phone-number member `@sensitive`, so it belongs in a modelled
    /// listing — `ListPhoneNumbersOptedOut` is what reads it — and in no error string.
    pub phone_number: String,
    /// When the number opted out of receiving SMS messages from this account —
    /// `None` once it has been opted back in.
    pub opted_out_at: Option<SystemTime>,
    /// When `OptInPhoneNumber` last opted this number back in, or `None` when it
    /// never has been.
    pub last_opt_in: Option<SystemTime>,
}

impl PhoneNumberRecord {
    /// A number that has opted out of receiving SMS messages from this account and
    /// has not been opted back in: what `CheckIfPhoneNumberIsOptedOut` reports and
    /// what `OptInPhoneNumber` has something to do about.
    pub fn is_opted_out(&self) -> bool {
        self.opted_out_at.is_some()
    }

    /// The instant from which this number may be opted in again, or `None` when it
    /// has never been opted in and so may be opted in now.
    pub fn opt_in_available_at(&self) -> Option<SystemTime> {
        self.last_opt_in.map(|at| at + OPT_IN_INTERVAL)
    }
}

// =============================================================================
// The account's dedicated origination numbers
// =============================================================================

/// How many origination numbers one page of `ListOriginationNumbers` holds when the
/// request names no page size.
///
/// This listing's page size is bounded by its OWN page-size member rather than by the
/// [`PAGE_SIZE`] of the notification-plane listings:
/// `smithy_query(query="shape", shape="MaxItemsListOriginationNumbers")` reports
/// `range {min: 1, max: 30}`, so thirty is the largest page a caller can ask this
/// listing for — and the page a caller that asks for no size is answered with.
pub const ORIGINATION_NUMBERS_PAGE_SIZE: usize = 30;

/// One dedicated origination number this account holds, as `ListOriginationNumbers`
/// reports it.
///
/// A dedicated origination number is acquired OUT OF BAND — a short code, a toll-free
/// number or a ten-digit long code is requested and registered through a separate AWS
/// request, and no SNS API performs that — so nothing a caller sends to this service
/// creates a record here: `ListOriginationNumbers` only REPORTS the inventory, the way
/// the opt-out store of [`PhoneNumberRecord`] is only read by the operations that
/// report it. Every member of a reported entry comes from the record, so the listing
/// states what the account holds rather than anything one call made up.
#[derive(Debug, Clone)]
pub struct OriginationNumberRecord {
    /// The origination number itself. The model marks the member it is reported in
    /// `@sensitive`, so it belongs in the modelled listing and in no error string.
    pub phone_number: String,
    /// "The status of the phone number" (`PhoneNumberInformation$Status` doc). The
    /// model leaves the member an unconstrained string, so the listing reports the
    /// value the record carries.
    pub status: String,
    /// The two-letter ISO country code of the number's country, as
    /// `PhoneNumberInformation$Iso2CountryCode` (pattern `^[A-Za-z]{2}$`) carries it.
    pub iso2_country_code: String,
    /// The number's route, one of [`crate::ops::types::ROUTE_TYPE_VALUES`] — the
    /// values the modelled `RouteType` enum accepts.
    pub route_type: String,
    /// What the number can send, each entry one of
    /// [`crate::ops::types::NUMBER_CAPABILITY_VALUES`] — the values the modelled
    /// `NumberCapability` enum accepts.
    pub number_capabilities: Vec<String>,
    /// "The date and time when the phone number was created"
    /// (`PhoneNumberInformation$CreatedAt` doc), which also ORDERS the listing.
    pub created_at: SystemTime,
}

// =============================================================================
// Mobile push: platform applications and their device endpoints
// =============================================================================

/// The prefix the resource part of a platform application's ARN carries:
/// `arn:aws:sns:<region>:<account>:app/<platform>/<name>`, from the
/// `CreatePlatformApplicationResponse$PlatformApplicationArn` doc.
pub const PLATFORM_APPLICATION_PREFIX: &str = "app/";

/// The prefix the resource part of a platform ENDPOINT's ARN carries:
/// `arn:aws:sns:<region>:<account>:endpoint/<platform>/<application>/<id>`, from the
/// `CreatePlatformEndpointResponse$EndpointArn` doc.
///
/// Shared, because "is this string the ARN of a platform endpoint?" is asked both by
/// `Subscribe`'s `application` protocol and by the mobile-push operations that act on
/// an endpoint directly.
pub const PLATFORM_ENDPOINT_PREFIX: &str = "endpoint/";

/// A platform application's ARN, for the mobile push platform `platform` (`ADM`,
/// `APNS`, `APNS_SANDBOX`, `GCM`) and the application's `name`.
pub fn platform_application_arn(region: &str, platform: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        "sns",
        region,
        &format!("{PLATFORM_APPLICATION_PREFIX}{platform}/{name}"),
    )
}

/// "The credential received from the notification service"
/// (`SetPlatformApplicationAttributesInput$Attributes` doc): the secret of the push
/// credential pair, which every platform's own doc clause names.
pub const PLATFORM_CREDENTIAL_ATTRIBUTE: &str = "PlatformCredential";

/// "The principal received from the notification service"
/// (`SetPlatformApplicationAttributesInput$Attributes` doc): the public half of the
/// push credential pair, which the GCM clause of that doc has none of.
pub const PLATFORM_PRINCIPAL_ATTRIBUTE: &str = "PlatformPrincipal";

/// The platform-application attributes a caller WRITES and SNS never reads back.
///
/// `PlatformCredential` is "the credential received from the notification service"
/// and `PlatformPrincipal` "the principal received from the notification service"
/// (`SetPlatformApplicationAttributesInput$Attributes` doc) — the secret half of the
/// push credential pair. Every reporting path goes through
/// [`PlatformApplicationRecord::reported_attributes`], so a stored credential is
/// never published back to a caller.
pub const PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES: &[&str] =
    &[PLATFORM_CREDENTIAL_ATTRIBUTE, PLATFORM_PRINCIPAL_ATTRIBUTE];

/// Whether `name` names one of the write-only credential attributes listed on
/// [`PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES`].
pub fn is_platform_credential_attribute(name: &str) -> bool {
    PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(&name)
}

/// The attribute names SNS defines for a platform application, from the
/// `SetPlatformApplicationAttributesInput$Attributes` doc: the credential pair, the
/// four `Event*` topics an endpoint lifecycle event is published to, the two feedback
/// role ARNs with their sample rate, and the two APNs token-based `ApplePlatform*`
/// names. SNS refuses any other name, and so does this emulator: the closed set is
/// also what stops a caller inventing fresh attribute names to grow one application
/// record for ever.
///
/// The names `GetPlatformApplicationAttributes` reports and no caller writes
/// (`AppleCertificateExpiryDate`, `AuthenticationMethod`) are deliberately NOT here:
/// the service derives those from the credential material it was given.
pub const PLATFORM_APPLICATION_ATTRIBUTE_NAMES: &[&str] = &[
    "ApplePlatformBundleID",
    "ApplePlatformTeamID",
    PLATFORM_APPLICATION_ENABLED_ATTRIBUTE,
    "EventDeliveryFailure",
    "EventEndpointCreated",
    "EventEndpointDeleted",
    "EventEndpointUpdated",
    "FailureFeedbackRoleArn",
    PLATFORM_CREDENTIAL_ATTRIBUTE,
    PLATFORM_PRINCIPAL_ATTRIBUTE,
    "SuccessFeedbackRoleArn",
    "SuccessFeedbackSampleRate",
];

/// Whether SNS sends notifications through this platform application: the
/// application's counterpart of [`ENDPOINT_ENABLED_ATTRIBUTE`], spelled the same way
/// and carrying the same two words. An application SNS has just registered is
/// ENABLED, so the create writes the flag and every report of the application carries
/// it.
pub const PLATFORM_APPLICATION_ENABLED_ATTRIBUTE: &str = ENDPOINT_ENABLED_ATTRIBUTE;

/// Whether `name` is an attribute SNS defines for a platform application.
pub fn is_platform_application_attribute(name: &str) -> bool {
    PLATFORM_APPLICATION_ATTRIBUTE_NAMES.contains(&name)
}

/// The platform-application attributes an attribute WRITE does not take from its
/// caller: the ones SNS reports for an application and maintains itself.
///
/// The delivery flag is one. `SetPlatformApplicationAttributesInput$Attributes`
/// enumerates what that call writes — the credential pair, the four event topics, the
/// two feedback roles with their sample rate and the two APNs identifiers — and the flag
/// is in none of those clauses: SNS is what moves it, clearing it when the notification
/// service rejects the application's credentials.
///
/// The REGISTRATION is the exception, and its own recorded behaviour says so: a create
/// carrying the flag persists the value it carried, because that is the INITIAL value of
/// a record SNS is minting rather than a change to one it already maintains. So this
/// list belongs to the write, not to the attribute catalog.
///
/// Recorded evidence: an attribute write carrying the delivery flag is SERVED, and the
/// application still reports the value the service holds — so the name is accepted (it
/// is one SNS defines for an application) and the caller's value is not written.
pub const PLATFORM_APPLICATION_ATTRIBUTES_A_WRITE_DOES_NOT_TAKE: &[&str] =
    &[PLATFORM_APPLICATION_ENABLED_ATTRIBUTE];

/// Whether `name` is a platform-application attribute an attribute write does not take
/// from its caller ([`PLATFORM_APPLICATION_ATTRIBUTES_A_WRITE_DOES_NOT_TAKE`]).
pub fn platform_application_attribute_is_the_services_own(name: &str) -> bool {
    PLATFORM_APPLICATION_ATTRIBUTES_A_WRITE_DOES_NOT_TAKE.contains(&name)
}

/// The entries of a caller's platform-application attribute map an attribute write
/// APPLIES: every one except the names the service maintains itself
/// ([`platform_application_attribute_is_the_services_own`]).
pub fn platform_application_attributes_a_write_applies(
    attributes: &HashMap<String, String>,
) -> impl Iterator<Item = (String, String)> + '_ {
    attributes
        .iter()
        .filter(|(name, _)| !platform_application_attribute_is_the_services_own(name))
        .map(|(name, value)| (name.clone(), value.clone()))
}

/// The name a refusal gives a platform application record, for the shared
/// attribute-map ceiling ([`attribute_bytes_refusal`]).
pub const PLATFORM_APPLICATION_RECORD: &str = "a platform application";

/// The push notification services a platform application may target, each spelled as
/// its documentation spells it.
///
/// `CreatePlatformApplicationInput$Platform` names ADM, APNS, APNS_SANDBOX and GCM,
/// and the operation's own doc adds the per-platform credential clauses for Baidu,
/// MPNS and WNS; a value that names none of them names no notification service SNS
/// registers an application with. A request's value is resolved against this list
/// without regard to case ([`canonical_platform_name`]), so a service is the same
/// service whichever case a client spelled its name in, and the entry here — not the
/// spelling the request used — is what the application's ARN and record carry.
pub const PLATFORM_NAMES: &[&str] = &[
    "ADM",
    "APNS",
    "APNS_SANDBOX",
    "Baidu",
    "GCM",
    "MPNS",
    "WNS",
];

/// The entry of [`PLATFORM_NAMES`] `platform` names, or `None` when it names none of
/// them.
///
/// One platform is one application namespace: resolving the request's value to the
/// documented spelling is what keeps two spellings of one service from addressing two
/// records under two ARNs.
pub fn canonical_platform_name(platform: &str) -> Option<&'static str> {
    PLATFORM_NAMES
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(platform))
}

/// Whether `platform` names one of the push notification services listed on
/// [`PLATFORM_NAMES`].
pub fn is_platform_name(platform: &str) -> bool {
    canonical_platform_name(platform).is_some()
}

/// The push services whose credential pair carries NO principal: "For GCM (Firebase
/// Cloud Messaging), there is no PlatformPrincipal"
/// (`SetPlatformApplicationAttributesInput$Attributes` doc), where every other
/// platform's clause names one.
pub const PLATFORMS_WITHOUT_A_PRINCIPAL: &[&str] = &["GCM"];

/// Whether the credential pair of `platform` carries a principal beside its
/// credential — true for every service except those on
/// [`PLATFORMS_WITHOUT_A_PRINCIPAL`].
pub fn platform_takes_a_principal(platform: &str) -> bool {
    !PLATFORMS_WITHOUT_A_PRINCIPAL
        .iter()
        .any(|known| known.eq_ignore_ascii_case(platform))
}

/// The sentence SNS refuses credential material a platform cannot authenticate with:
/// the member the material arrived in and the reason it was not usable, with none of
/// the (secret) material echoed back.
pub const INVALID_PLATFORM_CREDENTIALS_REFUSAL: &str =
    "Invalid parameter: Attributes Reason: Platform credentials are invalid";

/// The opening of a PEM-encoded block, the form the certificate and private-key
/// credential material of a push service is carried in (RFC 7468's `-----BEGIN
/// <label>-----` encapsulation boundary).
const PEM_BLOCK_OPENING: &str = "-----BEGIN ";

/// The push services whose `PlatformCredential` is a private key: "For APNS and
/// APNS_SANDBOX using certificate credentials, PlatformPrincipal is SSL certificate
/// and PlatformCredential is private key. For APNS and APNS_SANDBOX using token
/// credentials, PlatformPrincipal is signing key ID and PlatformCredential is signing
/// key. … For MPNS, PlatformPrincipal is TLS certificate and PlatformCredential is
/// private key" (`CreatePlatformApplication` doc) — a key, whichever credential type
/// the application uses, so the material is PEM-encoded.
pub const PLATFORMS_WITH_A_KEY_CREDENTIAL: &[&str] = &["APNS", "APNS_SANDBOX", "MPNS"];

/// The push service whose `PlatformCredential` is an API key or a key FILE: "For GCM
/// (Firebase Cloud Messaging) using key credentials … The PlatformCredential is API
/// key. For GCM (Firebase Cloud Messaging) using token credentials … The
/// PlatformCredential is a JSON formatted private key file"
/// (`CreatePlatformApplication` doc).
pub const PLATFORM_WITH_AN_API_KEY_CREDENTIAL: &str = "GCM";

/// The opening of the API key the notification service's console issues, and the
/// length it issues one with: Google publishes an API key as a 39-character URL-safe
/// string beginning `AIza` (the form every key in the Google Cloud console's
/// credentials page takes), which is the `API key` the GCM key-credential clause of
/// the `CreatePlatformApplication` doc names.
///
/// The form is what makes the clause checkable at all: the doc names TWO credential
/// types for that platform, an API key and "a JSON formatted private key file", and a
/// value that is neither is material the notification service has no key in — which is
/// why presence alone cannot be the rule here, where it is the rule for a platform
/// whose clause names an opaque secret.
const API_KEY_PREFIX: &str = "AIza";

/// The length of the API key described on [`API_KEY_PREFIX`].
const API_KEY_LEN: usize = 39;

/// Whether `value` has the form of the API key the notification service's console
/// issues ([`API_KEY_PREFIX`]).
fn is_api_key(value: &str) -> bool {
    value.len() == API_KEY_LEN
        && value.starts_with(API_KEY_PREFIX)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `credential` could be the artifact `platform`'s documented credential type
/// is, and so material that platform could authenticate a notification with.
///
/// Checked where the credential type has a PUBLISHED form — a PEM-encoded key for the
/// services on [`PLATFORMS_WITH_A_KEY_CREDENTIAL`], and an API key or a JSON key file
/// for [`PLATFORM_WITH_AN_API_KEY_CREDENTIAL`]. For a service whose clause names an
/// opaque secret instead (a client secret, a secret key), the material has no form to
/// read, so anything the caller holds is material only that service can judge.
fn platform_credential_is_usable(platform: &str, credential: &str) -> bool {
    if PLATFORMS_WITH_A_KEY_CREDENTIAL
        .iter()
        .any(|known| known.eq_ignore_ascii_case(platform))
    {
        return credential.contains(PEM_BLOCK_OPENING);
    }
    if PLATFORM_WITH_AN_API_KEY_CREDENTIAL.eq_ignore_ascii_case(platform) {
        return is_api_key(credential) || json_object(credential).is_some();
    }
    true
}

/// The refusal for the credential material `attributes` offers for `platform`, or
/// `None` when that platform could authenticate with it.
///
/// Two clauses, both from the per-platform credential clauses of the
/// `CreatePlatformApplication` doc. The pair has to be THERE: "You must specify
/// PlatformPrincipal and PlatformCredential attributes when using the
/// CreatePlatformApplication action", with the principal the exception those clauses
/// state ([`PLATFORMS_WITHOUT_A_PRINCIPAL`]) and a blank value offering nothing to
/// authenticate with, so it counts as absent. And the credential has to BE the
/// artifact the platform's credential type names ([`platform_credential_is_usable`]):
/// a value that could not be that artifact is credential material no push service
/// would accept, whoever holds it.
pub fn platform_credential_refusal(
    platform: &str,
    attributes: &HashMap<String, String>,
) -> Option<String> {
    let offered = |name: &str| {
        attributes
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    };
    let usable = offered(PLATFORM_CREDENTIAL_ATTRIBUTE)
        .is_some_and(|credential| platform_credential_is_usable(platform, credential))
        && (!platform_takes_a_principal(platform)
            || offered(PLATFORM_PRINCIPAL_ATTRIBUTE).is_some());
    (!usable).then(|| INVALID_PLATFORM_CREDENTIALS_REFUSAL.to_string())
}

/// The longest platform application name SNS accepts: "must be between 1 and 256
/// characters long" (`CreatePlatformApplicationInput$Name` doc).
pub const MAX_PLATFORM_APPLICATION_NAME_LEN: usize = 256;

/// Whether `name` satisfies the name rule SNS documents for a platform application:
/// "Application names must be made up of only uppercase and lowercase ASCII letters,
/// numbers, underscores, hyphens, and periods, and must be between 1 and 256
/// characters long" (`CreatePlatformApplicationInput$Name` doc) — the topic name rule
/// plus the period, which a topic name takes only as the FIFO suffix.
pub fn platform_application_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PLATFORM_APPLICATION_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// The sentence SNS refuses an unacceptable platform application name with: the member
/// the name arrived in and the documented rule it failed, with none of the name echoed
/// back — the same shape as every other `Reason:` clause of this service.
pub const INVALID_PLATFORM_APPLICATION_NAME_REFUSAL: &str =
    "Invalid parameter: Name Reason: Application names must be made up of only uppercase \
     and lowercase ASCII letters, numbers, underscores, hyphens, and periods, and must be \
     between 1 and 256 characters long";

/// The sentence SNS refuses a platform that names no push notification service it
/// registers an application with: the member, and a reason naming the value that was
/// not one of them — the value is the push service's own name, so a caller reading the
/// refusal is told which name it is that SNS does not serve.
///
/// Recorded evidence: `CreatePlatformApplication` for a platform that names no push
/// service answers `400 InvalidParameter` with exactly this sentence.
pub fn unsupported_platform_refusal(platform: &str) -> String {
    format!("Invalid parameter: Platform Reason: {platform} is not supported")
}

/// One platform application object of the calling account, as
/// `CreatePlatformApplication` registered it.
#[derive(Debug, Clone)]
pub struct PlatformApplicationRecord {
    /// The application's ARN, as [`platform_application_arn`] spells it: the key this
    /// record is held under, and what every mobile-push call names it by.
    pub arn: String,
    /// The push notification service this application targets (`ADM`, `APNS`,
    /// `APNS_SANDBOX`, `GCM`, …), as the request that created it spelled it.
    pub platform: String,
    /// The application name the request that created it carried.
    pub name: String,
    /// The application's attributes, as the create and the attribute write left them
    /// — including the write-only credential material, which
    /// [`Self::reported_attributes`] withholds from every report. Bounded by
    /// [`MAX_ATTRIBUTE_BYTES_PER_RECORD`], like every other attribute map here.
    pub attributes: HashMap<String, String>,
    pub created_at: SystemTime,
}

impl PlatformApplicationRecord {
    /// The attributes a report of this application carries: every attribute the
    /// record holds except the write-only credential material named on
    /// [`PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES`].
    pub fn reported_attributes(&self) -> HashMap<String, String> {
        self.attributes
            .iter()
            .filter(|(name, _)| !is_platform_credential_attribute(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}

/// A device endpoint's ARN: the endpoint resource of the platform application named by
/// `platform` and `application`, plus the id that tells one of its devices from
/// another.
pub fn platform_endpoint_arn(
    region: &str,
    platform: &str,
    application: &str,
    endpoint_id: &str,
) -> String {
    ::emulator_server::emulator_arn(
        "sns",
        region,
        &format!("{PLATFORM_ENDPOINT_PREFIX}{platform}/{application}/{endpoint_id}"),
    )
}

/// The attribute names SNS defines for a device endpoint, from the
/// `GetEndpointAttributesResponse$Attributes` doc: "CustomUserData – arbitrary user
/// data to associate with the endpoint … Enabled – flag that enables/disables delivery
/// to the endpoint … Token – device token, also referred to as a registration id".
pub const ENDPOINT_ATTRIBUTE_NAMES: &[&str] = &[
    ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE,
    ENDPOINT_ENABLED_ATTRIBUTE,
    ENDPOINT_TOKEN_ATTRIBUTE,
];

/// The arbitrary user data an endpoint's owner associated with it.
pub const ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE: &str = "CustomUserData";

/// Whether SNS delivers a notification to the endpoint. SNS sets it to `false` when
/// the notification service reports the endpoint invalid, so an endpoint nobody has
/// disabled is enabled.
pub const ENDPOINT_ENABLED_ATTRIBUTE: &str = "Enabled";

/// The device token the notification service issued for the endpoint's device: the
/// value [`PlatformEndpointRecord::token`] holds.
pub const ENDPOINT_TOKEN_ATTRIBUTE: &str = "Token";

/// The push service whose endpoints carry a channel and a user of their own: "When
/// using CreatePlatformEndpoint with Baidu, two attributes must be provided: ChannelId
/// and UserId" (`CreatePlatformEndpoint` doc), where every other platform's endpoint
/// takes only the names on [`ENDPOINT_ATTRIBUTE_NAMES`].
pub const PLATFORM_WITH_CHANNEL_ENDPOINTS: &str = "Baidu";

/// The two extra attribute names an endpoint of [`PLATFORM_WITH_CHANNEL_ENDPOINTS`]
/// carries, from the same doc sentence.
pub const CHANNEL_ENDPOINT_ATTRIBUTE_NAMES: &[&str] = &["ChannelId", "UserId"];

/// Whether `name` is an attribute SNS defines for a device endpoint of a platform
/// application targeting `platform`: the names every endpoint takes
/// ([`ENDPOINT_ATTRIBUTE_NAMES`]), and for [`PLATFORM_WITH_CHANNEL_ENDPOINTS`] the two
/// its own endpoints carry beside them.
///
/// SNS refuses any other name, and so does this emulator: the closed set is also what
/// stops a caller inventing fresh attribute names to grow one endpoint record for ever.
pub fn is_endpoint_attribute(platform: &str, name: &str) -> bool {
    ENDPOINT_ATTRIBUTE_NAMES.contains(&name)
        || (platform.eq_ignore_ascii_case(PLATFORM_WITH_CHANNEL_ENDPOINTS)
            && CHANNEL_ENDPOINT_ATTRIBUTE_NAMES.contains(&name))
}

/// The name a refusal gives a device endpoint record, for the shared attribute-map
/// ceiling ([`attribute_bytes_refusal`]).
pub const ENDPOINT_RECORD: &str = "an endpoint";

/// The most bytes of `CustomUserData` an endpoint may carry: "The data must be in UTF-8
/// format and less than 2KB" (`CreatePlatformEndpointInput$CustomUserData` doc) — LESS
/// than 2 KiB, so the most it holds is one byte under that.
pub const MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES: usize = 2 * 1024 - 1;

/// The refusal for user data that crosses [`MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES`], or
/// `None` when an endpoint may carry it. Worded like this service's other
/// size refusals: the member, the ceiling, and the size the request offered — never the
/// data itself.
pub fn custom_user_data_refusal(data: &str) -> Option<String> {
    let held = data.len();
    (held > MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES).then(|| {
        format!(
            "Invalid parameter: CustomUserData - at most \
             {MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES} bytes: {held}"
        )
    })
}

/// The sentence SNS refuses a registration that re-uses a device token under DIFFERENT
/// attributes with: the endpoint that already holds the token, and the reason the second
/// registration is not the idempotent one.
///
/// "The CreatePlatformEndpoint action is idempotent, so if the requester already owns an
/// endpoint with the same device token and attributes, that endpoint's ARN is returned
/// without creating a new endpoint" (`CreatePlatformEndpoint` doc) — the same token
/// under other attributes is therefore a conflict rather than a second endpoint, and the
/// sentence names the held endpoint so the caller can read or update it.
pub fn endpoint_token_conflict_refusal(endpoint_arn: &str) -> String {
    format!(
        "Invalid parameter: Token Reason: Endpoint {endpoint_arn} already exists with \
         the same Token, but different attributes."
    )
}

/// One device endpoint of a platform application: the device a mobile push
/// notification is addressed to, as a registration call recorded it.
#[derive(Debug, Clone)]
pub struct PlatformEndpointRecord {
    /// The endpoint's own ARN, in the form [`platform_endpoint_arn`] builds — also
    /// this record's key in
    /// [`ComAmazonawsSnsAmazonsimplenotificationserviceState::platform_endpoints`].
    pub arn: String,
    /// The ARN of the platform application the endpoint belongs to, in the form
    /// [`platform_application_arn`] builds.
    pub platform_application_arn: String,
    /// The device token the notification service issued for the device, which is what
    /// identifies the device to that service.
    pub token: String,
    /// The endpoint's attributes, as `GetEndpointAttributes` reports them. Bounded by
    /// [`MAX_ATTRIBUTE_BYTES_PER_RECORD`], like every other caller-sized attribute map
    /// here.
    pub attributes: HashMap<String, String>,
    pub created_at: SystemTime,
}

impl PlatformEndpointRecord {
    /// The attributes a report of this endpoint carries: the device token the record
    /// holds, the delivery flag, and whatever user data its owner associated with it.
    ///
    /// Shared by every report of an endpoint — the single read and the listing of an
    /// application's endpoints — because what an endpoint's attributes are does not
    /// depend on which call asked for them (`Endpoint$Attributes` is the same map
    /// `GetEndpointAttributes` answers with). The token comes from the record's own
    /// field rather than from the attribute map beside it, so one value has one
    /// source; an endpoint nobody has disabled reports delivery ENABLED, since "Amazon
    /// SNS will set this to false when a notification service indicates to Amazon SNS
    /// that the endpoint is invalid" (`GetEndpointAttributes` doc).
    pub fn reported_attributes(&self) -> HashMap<String, String> {
        let mut attributes = self.attributes.clone();
        attributes.insert(ENDPOINT_TOKEN_ATTRIBUTE.to_string(), self.token.clone());
        attributes
            .entry(ENDPOINT_ENABLED_ATTRIBUTE.to_string())
            .or_insert_with(|| attribute_flag(true));
        attributes
    }
}

/// Per-service state for com-amazonaws-sns-amazonsimplenotificationservice. A handler reaches it through [`service_state`].
///
/// Every CALLER-DRIVEN store here is therefore BOUNDED: `topics`,
/// `subscriptions`, `sandbox_phone_numbers`, `platform_applications` and
/// `platform_endpoints` at
/// [`MAX_RECORDS_PER_STORE`] records
/// each (a request that would cross it is refused with a modelled error), and
/// `delivered` at
/// [`MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION`] messages per subscription AND
/// [`MAX_RETAINED_DELIVERY_BYTES`] of payload overall, evicting oldest-first. So the
/// product a publish multiplies out — payload x subscriptions x publishes — is capped
/// at [`MAX_RETAINED_DELIVERY_BYTES`] however the caller shapes the traffic.
#[derive(Debug, Default)]
pub struct ComAmazonawsSnsAmazonsimplenotificationserviceState {
    /// Topics by ARN. At most [`MAX_RECORDS_PER_STORE`].
    pub topics: HashMap<String, TopicRecord>,
    /// Subscriptions by subscription ARN. At most [`MAX_RECORDS_PER_STORE`].
    pub subscriptions: HashMap<String, SubscriptionRecord>,
    /// The calling account's SMS sandbox destination phone numbers, keyed by the
    /// phone number itself: the sandbox holds one record per destination number. At
    /// most [`MAX_RECORDS_PER_STORE`].
    pub sandbox_phone_numbers: HashMap<String, SandboxPhoneNumberRecord>,
    /// Whether the calling account is in the SMS sandbox, which
    /// `GetSMSSandboxAccountStatus` reports. NOT caller-grown and not caller-moved:
    /// the account starts in the sandbox and no SNS operation takes it out, so this
    /// record is one value rather than a store.
    pub sms_sandbox_account: SmsSandboxAccountRecord,
    /// The calling account's settings for sending SMS messages: what
    /// `SetSMSAttributes` writes and `GetSMSAttributes` reports. NOT caller-grown and
    /// bounded in bytes — see [`SmsSettingsRecord`] — so it is one record rather than
    /// a store.
    pub sms_settings: SmsSettingsRecord,
    /// The calling account's mobile-push platform applications, keyed by their ARN.
    /// At most [`MAX_RECORDS_PER_STORE`]: an operation that registers one must refuse
    /// a request that would cross that ceiling, as the topic and subscription stores
    /// do.
    pub platform_applications: HashMap<String, PlatformApplicationRecord>,
    /// The SMS opt-out state this account knows per phone number, keyed by the number
    /// itself. NOT caller-grown, so it needs no ceiling of its own: a record appears
    /// when a number opts OUT, which the recipient does out of band and no SNS API
    /// performs — `OptInPhoneNumber` only clears and annotates a record already here,
    /// and `CheckIfPhoneNumberIsOptedOut` and `ListPhoneNumbersOptedOut` only read.
    pub phone_numbers: HashMap<String, PhoneNumberRecord>,
    /// The dedicated origination numbers this account holds, keyed by the number
    /// itself: what `ListOriginationNumbers` reports. NOT caller-grown, so it needs no
    /// ceiling of its own — a number is acquired out of band and no SNS API adds one,
    /// as [`OriginationNumberRecord`] sets out.
    pub origination_numbers: HashMap<String, OriginationNumberRecord>,
    /// The device endpoints of this account's platform applications, keyed by the
    /// endpoint's own ARN: what `DeleteEndpoint` removes one of. A caller-grown store,
    /// so the registration that adds a record is held to [`MAX_RECORDS_PER_STORE`]
    /// records like every other store here.
    pub platform_endpoints: HashMap<String, PlatformEndpointRecord>,
    /// Delivered messages by subscription ARN, oldest first. Bounded: write through
    /// [`Self::record_delivery`], never by hand.
    ///
    /// ONE published message is held ONCE however many subscriptions received it: the
    /// record is SHARED, so a publish to a topic with many subscriptions does not
    /// allocate its body per subscription (the same sharing the queue deliveries of
    /// `crate::ops::publish` use for the envelope they hand out).
    pub delivered: HashMap<String, Vec<::std::sync::Arc<PublishedMessage>>>,
    /// The messages each ARCHIVING topic's message archive holds, oldest first, keyed by
    /// the topic's ARN: what a replay redelivers. Bounded: write through
    /// [`Self::archive_message`], never by hand.
    ///
    /// The records are SHARED with the delivery log, so archiving a publish costs one
    /// more handle rather than a second copy of the body.
    archive: HashMap<String, Vec<::std::sync::Arc<PublishedMessage>>>,
    /// The payload bytes currently retained in `archive`.
    archived_bytes: usize,
    /// The subscription each retained delivery belongs to, oldest delivery first:
    /// the eviction order of the whole log.
    delivery_order: VecDeque<String>,
    /// The payload bytes currently retained in `delivered`.
    retained_delivery_bytes: usize,
    /// What each FIFO deduplication scope has accepted, keyed by
    /// [`deduplication_key`]: the message a duplicate publish answers with instead of
    /// being delivered again. Bounded at [`MAX_DEDUPLICATION_RECORDS`] live records, and
    /// written through [`Self::remember_deduplication`], never by hand.
    deduplication: HashMap<String, DeduplicationRecord>,
    /// Backs the `SequenceNumber` a FIFO topic's `Publish` returns.
    pub sequence_counter: u64,
    /// Backs [`Self::next_unique_id`]: the monotone part of every minted id.
    id_counter: u64,
}

impl ComAmazonawsSnsAmazonsimplenotificationserviceState {
    /// This topic's subscriptions, oldest first (ARN breaking a tie), so a list
    /// response and its pagination token are stable across calls.
    pub fn subscriptions_of(&self, topic_arn: &str) -> Vec<&SubscriptionRecord> {
        let mut found: Vec<&SubscriptionRecord> = self
            .subscriptions
            .values()
            .filter(|sub| sub.topic_arn == topic_arn)
            .collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        found
    }

    /// Every subscription this account holds IN `region`, oldest first (ARN breaking a
    /// tie), so a listing and its pagination token are stable across calls — see
    /// [`Self::subscriptions_of`], which is the one topic's.
    ///
    /// A subscription belongs to the region of the topic it extends, which is what
    /// [`belongs_to_region`] reads off that topic's ARN: the account-wide listing an
    /// endpoint of `region` serves reports those and no others.
    ///
    /// Selects on nothing else but the record's existence: a subscription whose
    /// endpoint names a resource that is not there — or one in another region — is
    /// still a subscription of its topic's region, and is listed like any other.
    pub fn subscriptions_in_region(&self, region: &str) -> Vec<&SubscriptionRecord> {
        let mut found: Vec<&SubscriptionRecord> = self
            .subscriptions
            .values()
            .filter(|sub| belongs_to_region(&sub.topic_arn, region))
            .collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        found
    }

    /// Every topic this account holds in `region`, oldest first (ARN breaking a tie) —
    /// selected by [`belongs_to_region`] off the topic's own ARN, like
    /// [`Self::subscriptions_in_region`].
    pub fn topics_in_region(&self, region: &str) -> Vec<&TopicRecord> {
        let mut found: Vec<&TopicRecord> = self
            .topics
            .values()
            .filter(|topic| belongs_to_region(&topic.arn, region))
            .collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        found
    }

    /// Every destination phone number in the SMS sandbox, oldest first (the number
    /// itself breaking a tie), so a listing and its pagination token are stable
    /// across calls — see [`Self::subscriptions_of`].
    pub fn sandbox_phone_numbers_in_order(&self) -> Vec<&SandboxPhoneNumberRecord> {
        let mut found: Vec<&SandboxPhoneNumberRecord> =
            self.sandbox_phone_numbers.values().collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.phone_number.cmp(&b.phone_number))
        });
        found
    }

    /// Every phone number this account currently holds an uncleared opt-out for,
    /// oldest opt-out first (the number itself breaking a tie), so a listing and its
    /// pagination token are stable across calls — see [`Self::subscriptions_of`].
    ///
    /// Selects on the record's opt-out state, not on the record's existence: a record
    /// that `OptInPhoneNumber` cleared stays in the store (it carries the instant the
    /// once-per-interval rule is measured from) but the number is no longer opted out,
    /// so it is not listed — the same state [`PhoneNumberRecord::is_opted_out`] is what
    /// a check of one number reports.
    pub fn opted_out_numbers_in_order(&self) -> Vec<&PhoneNumberRecord> {
        let mut found: Vec<&PhoneNumberRecord> = self
            .phone_numbers
            .values()
            .filter(|record| record.is_opted_out())
            .collect();
        found.sort_by(|a, b| {
            a.opted_out_at
                .cmp(&b.opted_out_at)
                .then_with(|| a.phone_number.cmp(&b.phone_number))
        });
        found
    }

    /// Every dedicated origination number this account holds, oldest first (the number
    /// itself breaking a tie), so a listing and its pagination token are stable across
    /// calls — see [`Self::subscriptions_of`].
    ///
    /// Selects on nothing but the record's existence: a number the account holds is a
    /// number the listing reports, whatever status its record carries.
    pub fn origination_numbers_in_order(&self) -> Vec<&OriginationNumberRecord> {
        let mut found: Vec<&OriginationNumberRecord> = self.origination_numbers.values().collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.phone_number.cmp(&b.phone_number))
        });
        found
    }

    /// Every platform application this account holds in `region`, oldest first (the
    /// ARN breaking a tie), so a listing and its pagination token are stable across
    /// calls — selected by [`belongs_to_region`] off the application's own ARN, like
    /// [`Self::subscriptions_in_region`].
    pub fn platform_applications_in_region(
        &self,
        region: &str,
    ) -> Vec<&PlatformApplicationRecord> {
        let mut found: Vec<&PlatformApplicationRecord> = self
            .platform_applications
            .values()
            .filter(|application| belongs_to_region(&application.arn, region))
            .collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        found
    }

    /// Every device endpoint of the platform application `application_arn`, oldest
    /// first (the endpoint's own ARN breaking a tie), so a listing and its pagination
    /// token are stable across calls — the endpoint counterpart of
    /// [`Self::platform_applications_in_region`].
    ///
    /// Selected on the application the record names rather than on the region, because
    /// an endpoint belongs to one application and that application's ARN carries the
    /// region: the listing is already scoped by the application the caller named.
    pub fn endpoints_of_application(
        &self,
        application_arn: &str,
    ) -> Vec<&PlatformEndpointRecord> {
        let mut found: Vec<&PlatformEndpointRecord> = self
            .platform_endpoints
            .values()
            .filter(|endpoint| endpoint.platform_application_arn == application_arn)
            .collect();
        found.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        found
    }

    /// `(SubscriptionsConfirmed, SubscriptionsPending)` for a topic, counted from the
    /// subscription records rather than from a stored counter.
    pub fn subscription_counts(&self, topic_arn: &str) -> (usize, usize) {
        let mut confirmed = 0;
        let mut pending = 0;
        for sub in self.subscriptions.values() {
            if sub.topic_arn != topic_arn {
                continue;
            }
            if sub.pending_confirmation {
                pending += 1;
            } else {
                confirmed += 1;
            }
        }
        (confirmed, pending)
    }

    /// The next `SequenceNumber`: strictly increasing, so a later publish to a FIFO
    /// topic always sorts after an earlier one.
    pub fn next_sequence_number(&mut self) -> String {
        self.sequence_counter = self.sequence_counter.saturating_add(1);
        // SNS documents a 128-bit, non-consecutive sequence number; a fixed-width
        // decimal keeps the string order the same as the numeric order.
        format!("{:020}", self.sequence_counter)
    }

    /// A fresh id in the 8-4-4-4-12 hexadecimal form SNS message ids and the last
    /// segment of a subscription ARN carry.
    ///
    /// Unique because this store's own monotone counter goes into it; unpredictable
    /// because the process's randomly seeded hasher does too; shaped like a version-4
    /// UUID because that is what SNS returns. A `uuid` crate would be the natural home
    /// for the formatting, and the first version of this change used one — but that
    /// dependency is the ONE edit that makes this change touch Cargo.lock, the single
    /// file every other fix also edits, and the resulting squash conflict kept this
    /// change from landing at all. The digest comes from `sha2`, which this crate
    /// already depends on.
    pub fn next_unique_id(&mut self) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        uuid_v4_shaped(self.id_counter)
    }

    /// A fresh one-time password to deliver to a sandbox destination number:
    /// [`SANDBOX_OTP_DIGITS`] decimal digits, and NOT a code any of this account's
    /// sandbox numbers currently holds — so delivering a new code to a number really
    /// does retire the one it had, rather than possibly re-delivering it.
    ///
    /// Minted from the store's own id counter ([`Self::next_unique_id`]), so
    /// successive deliveries get different codes rather than one fixed code for ever,
    /// and NOT derived from the request, because a code the caller's own request
    /// determines would leave the verification step deciding nothing. It is not key
    /// material and nothing is authenticated with it: it stands in for the code the
    /// real service texts to the destination number, and a test reads it from the
    /// record it was delivered to, the way the handler does.
    pub fn next_sandbox_one_time_password(&mut self) -> String {
        let mut code = self.sandbox_otp_candidate();
        for _ in 1..SANDBOX_OTP_MINT_ATTEMPTS {
            if !self
                .sandbox_phone_numbers
                .values()
                .any(|record| record.one_time_password == code)
            {
                break;
            }
            code = self.sandbox_otp_candidate();
        }
        code
    }

    /// One candidate one-time password: [`SANDBOX_OTP_DIGITS`] decimal digits of a
    /// fresh [`Self::next_unique_id`]'s digest, which [`SANDBOX_OTP_MINT_ATTEMPTS`]
    /// governs the retrying of.
    fn sandbox_otp_candidate(&mut self) -> String {
        let seed = self.next_unique_id();
        let digest = Sha256::digest(seed.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        let modulus = 10u64.pow(SANDBOX_OTP_DIGITS as u32);
        format!(
            "{value:0width$}",
            value = u64::from_be_bytes(bytes) % modulus,
            width = SANDBOX_OTP_DIGITS,
        )
    }

    /// A fresh confirmation token to send to the endpoint of a subscription awaiting
    /// its subscriber's confirmation: the value `ConfirmSubscription` checks the
    /// caller's `Token` against.
    ///
    /// [`CONFIRMATION_TOKEN_HEX_CHARS`] lowercase hexadecimal characters, minted from
    /// the store's own id counter ([`Self::next_unique_id`]) so two subscriptions never
    /// share a token, and NOT derived from the request, because a token the caller's own
    /// request determines would leave the confirmation step deciding nothing. Opaque to
    /// the caller: nothing but equality is read off it.
    pub fn next_confirmation_token(&mut self) -> String {
        let seed = self.next_unique_id();
        // The width comes from the constant, not from the digest: a token is as many
        // hexadecimal characters as the constant names, capped by the digest it is
        // encoded from, so raising or lowering the constant cannot leave the token
        // disagreeing with its own documented width.
        let bytes: Vec<u8> = Sha256::digest(seed.as_bytes())
            .into_iter()
            .take(CONFIRMATION_TOKEN_HEX_CHARS / 2)
            .collect();
        hex::encode(bytes)
    }

    /// Record one delivery of `message` to `subscription_arn`, then bring the log back
    /// within [`MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION`] and
    /// [`MAX_RETAINED_DELIVERY_BYTES`] by dropping the oldest deliveries. This is the
    /// ONLY way to write `delivered`: writing the map directly loses the bound.
    ///
    /// The message arrives SHARED, so recording one publish to N subscriptions costs
    /// N handles and one body. The retained-byte count still charges every recording
    /// its whole [`PublishedMessage::retained_bytes`], which over-states what a shared
    /// body costs and therefore evicts sooner than the ceiling demands — the bound
    /// stays an upper bound on the bytes one caller can make the store hold.
    pub fn record_delivery(
        &mut self,
        subscription_arn: &str,
        message: ::std::sync::Arc<PublishedMessage>,
    ) {
        self.retained_delivery_bytes = self
            .retained_delivery_bytes
            .saturating_add(message.retained_bytes());
        self.delivered
            .entry(subscription_arn.to_string())
            .or_default()
            .push(message);
        self.delivery_order
            .push_back(subscription_arn.to_string());

        while self
            .delivered
            .get(subscription_arn)
            .is_some_and(|held| held.len() > MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION)
        {
            self.drop_oldest_delivery(subscription_arn);
        }
        while self.retained_delivery_bytes > MAX_RETAINED_DELIVERY_BYTES {
            let Some(oldest) = self.delivery_order.front().cloned() else {
                break;
            };
            self.drop_oldest_delivery(&oldest);
        }
    }

    /// Forget every delivery to `subscription_arn` — the subscription is gone, so its
    /// log is too, and the bytes it held are released.
    pub fn forget_deliveries(&mut self, subscription_arn: &str) {
        if let Some(held) = self.delivered.remove(subscription_arn) {
            let released: usize = held.iter().map(|message| message.retained_bytes()).sum();
            self.retained_delivery_bytes =
                self.retained_delivery_bytes.saturating_sub(released);
        }
        self.delivery_order.retain(|arn| arn != subscription_arn);
    }

    /// Drop this subscription's oldest retained delivery, keeping the eviction order
    /// and the retained-byte count in step with it.
    ///
    /// It ALWAYS shortens `delivery_order` when that queue names this subscription,
    /// whatever shape `delivered` is in, so an eviction loop driven by the queue
    /// always terminates.
    fn drop_oldest_delivery(&mut self, subscription_arn: &str) {
        // The first occurrence of this subscription in the order IS its oldest
        // delivery: its own deliveries are queued oldest first.
        if let Some(position) = self
            .delivery_order
            .iter()
            .position(|arn| arn == subscription_arn)
        {
            self.delivery_order.remove(position);
        }
        let Some(held) = self.delivered.get_mut(subscription_arn) else {
            return;
        };
        if held.is_empty() {
            self.delivered.remove(subscription_arn);
            return;
        }
        let dropped = held.remove(0);
        if held.is_empty() {
            self.delivered.remove(subscription_arn);
        }
        self.retained_delivery_bytes = self
            .retained_delivery_bytes
            .saturating_sub(dropped.retained_bytes());
    }

    /// The payload bytes the delivery log currently retains, for a test or a diagnostic.
    pub fn retained_delivery_bytes(&self) -> usize {
        self.retained_delivery_bytes
    }

    /// Keep `message` in the message archive of `topic_arn`, after dropping what
    /// `retention` has run out on, then bring the archive back within
    /// [`MAX_ARCHIVED_MESSAGES_PER_TOPIC`] and [`MAX_ARCHIVED_BYTES`] by dropping its
    /// oldest messages. This is the ONLY way to write `archive`.
    ///
    /// Only a topic that holds an [`ARCHIVE_POLICY_ATTRIBUTE`] archives, so the caller
    /// decides whether to archive at all; `retention` is that policy's retention period
    /// ([`TopicRecord::archive_retention`]), and dropping what it has run out on is a
    /// write the CLOCK already owed — a message leaves the archive the moment its
    /// retention period ends, whether or not anyone publishes or replays.
    pub fn archive_message(
        &mut self,
        topic_arn: &str,
        retention: Option<Duration>,
        message: ::std::sync::Arc<PublishedMessage>,
    ) {
        if let Some(retention) = retention {
            self.expire_archive(topic_arn, retention);
        }
        self.archived_bytes = self.archived_bytes.saturating_add(message.retained_bytes());
        self.archive
            .entry(topic_arn.to_string())
            .or_default()
            .push(message);
        while self
            .archive
            .get(topic_arn)
            .is_some_and(|held| held.len() > MAX_ARCHIVED_MESSAGES_PER_TOPIC)
        {
            self.drop_oldest_archived(topic_arn);
        }
        while self.archived_bytes > MAX_ARCHIVED_BYTES {
            let Some(fullest) = self
                .archive
                .iter()
                .max_by_key(|(_, held)| held.len())
                .map(|(arn, _)| arn.clone())
            else {
                break;
            };
            self.drop_oldest_archived(&fullest);
        }
    }

    /// Drop `topic_arn`'s oldest archived message, keeping the retained-byte count in
    /// step with it.
    fn drop_oldest_archived(&mut self, topic_arn: &str) {
        let Some(held) = self.archive.get_mut(topic_arn) else {
            return;
        };
        if held.is_empty() {
            self.archive.remove(topic_arn);
            return;
        }
        let dropped = held.remove(0);
        if held.is_empty() {
            self.archive.remove(topic_arn);
        }
        self.archived_bytes = self.archived_bytes.saturating_sub(dropped.retained_bytes());
    }

    /// Forget every archived message of `topic_arn` whose `retention` has run out: the
    /// archive holds nothing older than the retention period its policy states.
    pub fn expire_archive(&mut self, topic_arn: &str, retention: Duration) {
        let Some(held) = self.archive.get_mut(topic_arn) else {
            return;
        };
        let now = SystemTime::now();
        let mut released = 0;
        held.retain(|message| {
            let kept = now
                .duration_since(message.published_at)
                .is_ok_and(|age| age < retention);
            if !kept {
                released += message.retained_bytes();
            }
            kept
        });
        if held.is_empty() {
            self.archive.remove(topic_arn);
        }
        self.archived_bytes = self.archived_bytes.saturating_sub(released);
    }

    /// The archived messages of `topic_arn` inside the window `from ..= until` (`until`
    /// of `None` being every message from `from` on), oldest publish first — which is
    /// the order a replay redelivers them in.
    pub fn archived_messages(
        &self,
        topic_arn: &str,
        from: SystemTime,
        until: Option<SystemTime>,
    ) -> Vec<::std::sync::Arc<PublishedMessage>> {
        self.archive
            .get(topic_arn)
            .into_iter()
            .flatten()
            .filter(|message| {
                message.published_at >= from
                    && until.is_none_or(|until| message.published_at <= until)
            })
            .map(::std::sync::Arc::clone)
            .collect()
    }

    /// The payload bytes the message archives currently retain, for a test or a
    /// diagnostic.
    pub fn archived_bytes(&self) -> usize {
        self.archived_bytes
    }

    /// Forget the whole message archive of `topic_arn` — the topic is gone, or it stopped
    /// archiving, so there is nothing left to replay and the bytes it held are released.
    pub fn forget_archive(&mut self, topic_arn: &str) {
        if let Some(held) = self.archive.remove(topic_arn) {
            let released: usize = held.iter().map(|message| message.retained_bytes()).sum();
            self.archived_bytes = self.archived_bytes.saturating_sub(released);
        }
    }

    /// The message an earlier publish under this deduplication key was accepted as, or
    /// `None` when the key names no message inside the deduplication interval.
    ///
    /// "If a message with a particular MessageDeduplicationId is sent successfully,
    /// subsequent messages within the deduplication scope and interval, with the same
    /// MessageDeduplicationId, are accepted successfully but aren't delivered"
    /// (`PublishInput$MessageDeduplicationId` doc) — so a caller that retries a publish
    /// is answered with the id and sequence number of the copy that WAS delivered.
    ///
    /// Forgetting the records the interval has run out on is the only write a lookup
    /// performs, and it is one the clock already owed: a record is dead
    /// [`DEDUPLICATION_INTERVAL`] after the publish that wrote it, whether or not anyone
    /// looks.
    pub fn deduplicated_publish(&mut self, key: &str) -> Option<DeduplicationRecord> {
        self.expire_deduplication();
        self.deduplication.get(key).cloned()
    }

    /// Remember that this deduplication key was accepted as `message_id`, so the copies
    /// that follow it inside the interval answer with that message rather than being
    /// delivered again. The ONLY way to write `deduplication`: writing the map directly
    /// loses the bound.
    pub fn remember_deduplication(
        &mut self,
        key: String,
        message_id: &str,
        sequence_number: Option<&str>,
    ) {
        self.expire_deduplication();
        self.deduplication.insert(
            key,
            DeduplicationRecord {
                message_id: message_id.to_string(),
                sequence_number: sequence_number.map(str::to_string),
                accepted_at: SystemTime::now(),
            },
        );
        // The oldest record is the first to stop collapsing duplicates, which is the
        // least a bounded memory can cost: it is the one closest to expiring anyway.
        while self.deduplication.len() > MAX_DEDUPLICATION_RECORDS {
            let Some(oldest) = self
                .deduplication
                .iter()
                .min_by_key(|(_, held)| held.accepted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.deduplication.remove(&oldest);
        }
    }

    /// Forget every deduplication record whose [`DEDUPLICATION_INTERVAL`] has run out:
    /// the id is free again, so the next publish carrying it is a new message.
    fn expire_deduplication(&mut self) {
        let now = SystemTime::now();
        self.deduplication.retain(|_, held| {
            now.duration_since(held.accepted_at).unwrap_or_default() < DEDUPLICATION_INTERVAL
        });
    }
}

/// An id in the 8-4-4-4-12 hexadecimal form of a version-4 UUID, derived from
/// `counter` plus the clock and the process's randomly seeded hasher.
///
/// The ONE place this crate shapes such an id, reached by
/// [`ComAmazonawsSnsAmazonsimplenotificationserviceState::next_unique_id`] (whose
/// counter is the store's) and by [`new_request_id`] (whose counter is this
/// module's). Unique because a monotone counter goes into it; unpredictable
/// because the hasher's random seed does too.
fn uuid_v4_shaped(counter: u64) -> String {
    use std::hash::{BuildHasher, Hasher};

    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let digest = Sha256::digest(format!("{counter}:{nanos}:{entropy}"));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // The version (4) and variant (RFC 4122) nibbles, so the id is well formed and
    // not mistaken for a name-based or time-based one.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The id one response reports as its own: the value of the `<RequestId>` element
/// every SNS response document carries.
///
/// A correlation id, not a security token, and per-response rather than
/// per-resource: two responses are distinguishable, which is what a client logs
/// and a proxy correlates on. It needs no state, so it is a free function rather
/// than a method on the store — a request that fails before any store is reached
/// still answers with one.
pub fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The monotone part of a request id, so two ids minted inside one clock tick
    /// still differ.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    uuid_v4_shaped(MINTED.fetch_add(1, Ordering::Relaxed))
}

/// The sentence SNS states for a topic no account of this emulator holds.
///
/// Fixed, and the SAME for every operation that looks a topic up: SNS names the
/// resource kind that is missing and does NOT echo the identifier the caller sent
/// back at it. Recorded evidence: `GetTopicAttributes` on a deleted topic answers
/// `404 NotFound` with exactly `Topic does not exist`.
pub const TOPIC_DOES_NOT_EXIST: &str = "Topic does not exist";

/// The sentence SNS states for a subscription this emulator holds no record of — the
/// subscription counterpart of [`TOPIC_DOES_NOT_EXIST`], worded the same way: the
/// resource kind that is missing, with none of the caller's identifier echoed back.
/// The HTTP 404 and the `NotFound` code are the requirement; this is SNS's wording of
/// the reason.
pub const SUBSCRIPTION_DOES_NOT_EXIST: &str = "Subscription does not exist";

/// The sentence SNS states for an ENDPOINT of a platform application this emulator
/// holds no record of — the endpoint counterpart of [`TOPIC_DOES_NOT_EXIST`], worded
/// the same way: the resource kind that is missing, with none of the caller's
/// identifier echoed back. The HTTP 404 and the `NotFound` code are the requirement;
/// this is SNS's wording of the reason.
pub const ENDPOINT_DOES_NOT_EXIST: &str = "Endpoint does not exist";

/// The sentence SNS states for a PLATFORM APPLICATION this emulator holds no record of
/// — the application counterpart of [`ENDPOINT_DOES_NOT_EXIST`], worded the same way:
/// the resource kind that is missing, named as the mobile-push plane names it, with
/// none of the caller's identifier echoed back. The HTTP 404 and the `NotFound` code
/// are the requirement; this is SNS's wording of the reason.
pub const PLATFORM_APPLICATION_DOES_NOT_EXIST: &str = "PlatformApplication does not exist";

/// The sentence SNS states for a RESOURCE a call names and the account does not hold:
/// the operations whose miss is the resource-scoped `ResourceNotFoundException` (the
/// tag operations, the SMS sandbox) say "resource", where the topic-scoped
/// `NotFoundException` names the topic ([`TOPIC_DOES_NOT_EXIST`]). Neither echoes the
/// caller's identifier back — the sandbox's is a phone number, which the model marks
/// `@sensitive`.
pub const RESOURCE_DOES_NOT_EXIST: &str = "Resource does not exist";

/// The sentence SNS refuses an attribute name it does not write with: `Invalid
/// parameter:` and the member the name was carried in, whether the name is none SNS
/// defines for that resource, one only the create call sets, or one the service
/// computes. Shared by the operations that write ONE attribute by name — whose input
/// member IS `AttributeName` — so the sentence does not depend on which of them
/// carried it; a call carrying an attribute MAP names its own member and the key it
/// did not recognise instead (see [`unknown_attribute_refusal`]).
pub const INVALID_ATTRIBUTE_NAME_REFUSAL: &str = "Invalid parameter: AttributeName";

/// The input member an attribute MAP reaches SNS in — the member `CreateTopic` and
/// `Subscribe` carry their attributes in, as against the `AttributeName` member of the
/// calls that write ONE attribute by name.
pub const ATTRIBUTES_MEMBER: &str = "Attributes";

/// The unrecognised key `attributes` carries, or `None` when every one of them is a
/// name `is_defined` admits.
///
/// A map has no order of its own, so the key reported is the first of the unrecognised
/// ones in the order a reader of the refusal can predict — ascending by name — and one
/// request therefore always names the same offending key.
pub fn unknown_attribute_name(
    attributes: &HashMap<String, String>,
    is_defined: impl Fn(&str) -> bool,
) -> Option<&str> {
    attributes
        .keys()
        .map(String::as_str)
        .filter(|name| !is_defined(name))
        .min()
}

/// The sentence SNS refuses an attribute MAP carrying a name it does not define with:
/// the input member the map arrived in, and the key that was not recognised.
///
/// Built from the REQUEST, so a caller reading it can find the typo it made: the
/// member names where to look and the echoed key names which entry of it.
///
/// This is the NOTIFICATION plane's wording; the mobile push plane states the same
/// refusal in its own words ([`mobile_push_unknown_attribute_refusal`]). Recorded
/// evidence: `Subscribe` carrying a misspelling of a subscription attribute answers
/// `400 InvalidParameter` with exactly this sentence.
pub fn unknown_attribute_refusal(member: &str, name: &str) -> String {
    format!("Invalid parameter: {member} Reason: Unknown attribute {name}")
}

/// The same refusal as the MOBILE PUSH plane states it: the input member the map
/// arrived in, and a `Reason:` clause saying the attribute NAME is not one the service
/// defines — still built from the request, so the key names which entry to fix.
///
/// The two spellings are the same split as the two spellings of the paging refusal
/// ([`MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL`] beside
/// [`INVALID_NEXT_TOKEN_REFUSAL`]): the mobile push plane words its own refusals.
/// Recorded evidence: `SetPlatformApplicationAttributes` carrying a name SNS defines
/// for no platform application answers `400 InvalidParameter` with exactly this
/// sentence, where the notification plane's calls answer
/// [`unknown_attribute_refusal`]'s.
pub fn mobile_push_unknown_attribute_refusal(member: &str, name: &str) -> String {
    format!("Invalid parameter: {member} Reason: Invalid attribute name: {name}")
}

/// The sentence SNS refuses a `NextToken` that is not one it minted with.
///
/// `Invalid parameter:` plus the member the rejected value was carried in, and
/// nothing else: the token is opaque, so the sentence states neither the value nor
/// why it could not be used. Shared by the listings of the notification plane —
/// topics, subscriptions and the SMS sandbox — because the sentence does not depend
/// on which listing the token was handed to. Recorded evidence: `ListTopics`,
/// `ListSubscriptions` and `ListSubscriptionsByTopic` with a token they never issued
/// each answer `400 InvalidParameter` with exactly this sentence.
pub const INVALID_NEXT_TOKEN_REFUSAL: &str = "Invalid parameter: NextToken";

/// The same refusal as the MOBILE PUSH listing states it: the member, and then a
/// `Reason:` clause saying the token cannot be used — still echoing none of the value
/// back. Recorded evidence: `ListPlatformApplications` with a token it never issued
/// answers `400 InvalidParameter` with exactly this sentence, where the notification
/// plane's listings answer the bare [`INVALID_NEXT_TOKEN_REFUSAL`].
pub const MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL: &str =
    "Invalid parameter: NextToken Reason: token is invalid or expired";

/// The sentence SNS refuses an unacceptable topic name with.
///
/// `Invalid parameter:` plus the parameter as SNS names it (`Topic Name`, not the
/// Smithy member `Name`), and nothing else: the sentence is the same whichever
/// name rule the value broke — the character set, the length, or the
/// [`FIFO_SUFFIX`] a FIFO topic's name needs — and it does not reproduce the name
/// the caller sent. Recorded evidence: `CreateTopic` with `FifoTopic=true` and a
/// name that does not end in `.fifo` answers `400 InvalidParameter` with exactly
/// `Invalid parameter: Topic Name`.
pub const INVALID_TOPIC_NAME: &str = "Invalid parameter: Topic Name";

/// The region this request addresses: the one in its SigV4 credential scope, or
/// [`DEFAULT_REGION`] when the request carries no scope to read.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The account id every ARN and `Owner` field this emulator emits carries.
pub fn owner_account_id() -> String {
    ::emulator_server::emulator_account_id().to_string()
}

/// A topic's ARN: `arn:aws:sns:<region>:<account>:<name>`.
pub fn topic_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("sns", region, name)
}

/// A subscription's ARN: the topic's ARN plus a fresh, unique id segment minted by
/// [`ComAmazonawsSnsAmazonsimplenotificationserviceState::next_unique_id`].
pub fn subscription_arn(topic_arn: &str, id: &str) -> String {
    format!("{topic_arn}:{id}")
}

/// The suffix SNS requires on a FIFO topic's name: "For a FIFO (first-in-first-out)
/// topic, the name must end with the .fifo suffix" (`CreateTopicInput$Name` doc).
pub const FIFO_SUFFIX: &str = ".fifo";

/// Whether `arn` addresses a FIFO QUEUE: "the name of a FIFO queue must end with the .fifo
/// suffix" (`CreateQueueInput$QueueName` doc), and the resource part of a queue's ARN is
/// its name, so the endpoint itself says which kind of queue it names.
///
/// ONE reading of a queue's kind, shared by the `Subscribe` that binds a queue to a topic
/// and the delivery that hands one a published message, so the two cannot disagree about
/// the same endpoint.
pub fn addresses_fifo_queue(arn: &str) -> bool {
    arn.ends_with(FIFO_SUFFIX)
}

/// One page of `items` starting at `token`, plus the token of the page after it.
///
/// `token` is the opaque `NextToken` a previous page returned — the index of the
/// first item of this page, spelled exactly as this function mints it. `None` is
/// returned for a token that is not one this function produced (another spelling of a
/// number included, since the token is opaque and only this spelling is handed out),
/// which each caller reports as its own modelled error.
pub fn paginate<T>(
    items: Vec<T>,
    token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<T>, Option<String>)> {
    let start: usize = match token {
        Some(token) => {
            let start: usize = token.parse().ok()?;
            // A token this function minted reads back as the same text: a signed,
            // zero-padded or otherwise respelled number is not one it handed out.
            if token != start.to_string() {
                return None;
            }
            start
        }
        None => 0,
    };
    if start > items.len() {
        return None;
    }
    let end = (start + page_size).min(items.len());
    let next = (end < items.len()).then(|| end.to_string());
    Some((items.into_iter().skip(start).take(end - start).collect(), next))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSnsAmazonsimplenotificationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSnsAmazonsimplenotificationserviceState>)
}
