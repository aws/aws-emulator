// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sqs-amazonsqs service.
//!
//! The queue is this service's primary resource: `CreateQueue` allocates one,
//! `GetQueueUrl` resolves it by name, `GetQueueAttributes` reports it,
//! `SetQueueAttributes` changes it and `DeleteQueue` removes it, all against the SAME
//! [`Queue`] record in [`ComAmazonawsSqsAmazonsqsState::queues`], so a create is
//! observable by every later read and an attribute change by every later report.
//!
//! # One namespace per region
//!
//! A queue belongs to the region the call that created it addressed ([`Queue::region`],
//! taken from the request's SigV4 credential scope by [`request_region`]), and the store
//! is keyed by that region together with the queue's name: a queue name "is unique within
//! the scope of your queues" of one region, so the same name created against a second
//! region is a second, independent queue and each delivers only its own messages.
//! Everything that resolves a queue takes that region —
//! [`ComAmazonawsSqsAmazonsqsState::queue`], [`ComAmazonawsSqsAmazonsqsState::queue_mut`],
//! [`ComAmazonawsSqsAmazonsqsState::queue_by_url`] and
//! [`ComAmazonawsSqsAmazonsqsState::queues_in`] — so a call signed for one region resolves
//! nothing another region holds, and the region the queue lives in is the one its ARN
//! carries ([`queue_arn`]). An ARN a request supplies is read the same way
//! ([`parse_queue_arn`]): one naming another region names no queue here, which is why
//! neither a dead-letter target nor a message movement task can span two regions.
//!
//! The record is the stored half; the wire half (the `Attributes` map
//! `GetQueueAttributes` returns) is PROJECTED from it by
//! [`Queue::reported_attributes`], so no handler assembles a response member by hand.
//! Server-side encryption is part of that projection: which of the two mutually
//! exclusive options a queue is on, and what it therefore reports, lives in
//! [`encryption`], as does the KMS permission a call that uses the queue's key needs.
//!
//! # The message plane
//!
//! A queue also HOLDS its messages: [`Queue::messages`] is the arrival-order deque
//! `SendMessage` appends to and `ReceiveMessage` hands out of, and [`Queue::handles`]
//! maps the receipt handle a receive minted to the message it names, which is what
//! `DeleteMessage` resolves — and [`Queue::deleted_handles`] remembers the handles a
//! delete has already consumed, so repeating a delete that succeeded is idempotent
//! rather than refused. HANDING a message over is [`Queue::hand_over`], the one path
//! every consumer of a queue takes — this service's own `ReceiveMessage` and a poller
//! another service runs over the same queue — so what one delivery does to a message's
//! counts, its visibility and its receipt handle is stated once. A handle is not an opaque
//! value with no provenance: it carries
//! the queue incarnation that minted it ([`Queue::mint_receipt_handle`]), so
//! [`ComAmazonawsSqsAmazonsqsState::receipt_handle_origin`] can tell this queue's own
//! handle from one another queue issued and from one a deleted incarnation issued, which
//! are three refusals a caller recovers from differently. Visibility and retention are
//! evaluated against the clock
//! by [`Queue::materialise_due`], so the three `ApproximateNumberOfMessages*`
//! attributes are counts over real state rather than made-up values. A message carries
//! the visibility timeout it was RECEIVED under, so a later change to the queue's
//! attribute spares it, while a FIFO queue's delay is re-derived from the queue's
//! current value by [`Queue::reapply_delay_seconds`], which is where the
//! FIFO-versus-standard retroactivity of `DelaySeconds` lives.
//!
//! Which of those messages a receive may hand over is [`Queue::delivery_order`]: on a
//! standard queue whatever is visible, and on a FIFO queue each message group in strict
//! send order, one group member in flight at a time. A FIFO send also remembers the
//! deduplication token it accepted a message under, in [`Queue::deduplication`], so a
//! resend inside the interval is accepted without delivering a second copy, and
//! sequences the message from [`Queue::sequenced`].
//!
//! A FIFO RECEIVE is idempotent the same way: the batch it handed over under a
//! `ReceiveRequestAttemptId` is remembered in [`Queue::receive_attempts`], so a retry
//! carrying that same token hands the identical messages and receipt handles back
//! instead of a second, different batch.
//!
//! `PurgeQueue` empties that plane through [`Queue::purge`]: the messages, the receipt
//! handles naming them, the remembered receive attempts that named them and the
//! deduplication tokens that would suppress a resend all go, and the instant of the
//! purge is remembered, so a further purge inside [`PURGE_QUEUE_INTERVAL`] of it is
//! refused ([`Queue::purge_in_progress`]).
//!
//! # Dead-letter redrive
//!
//! A queue whose `RedrivePolicy` names a dead-letter target only lets a message be
//! delivered as many times as that policy's count allows: once a message has had them
//! all, the next receive on that queue moves it to the target queue instead of handing it
//! over again ([`ComAmazonawsSqsAmazonsqsState::dead_letter_due`], over
//! [`Queue::dead_letter_target`]). The message keeps its identity and its receive count —
//! it is the same message in another queue — and is visible there at once, so the source
//! queue's `ApproximateNumberOfMessages` falls as the target's rises and the move task
//! below has something to drive back.
//!
//! That configuration is judged when it is WRITTEN, so no queue is left carrying one no
//! redrive could ever act on: the document itself against what the attribute documents —
//! both of its members, a count inside the range that member publishes and a target of this
//! account and region ([`redrive_policy_refusal`]) — and the PAIR it links against the
//! queue the target names and its type
//! ([`ComAmazonawsSqsAmazonsqsState::dead_letter_target_refusal`]). What
//! is stored and reported is the canonical rendering of the document
//! ([`canonical_redrive_document`]), not the caller's own spelling of it.
//!
//! # Dead-letter redrive permission
//!
//! WHICH queues may drive their messages to a dead-letter queue is that queue's own
//! `RedriveAllowPolicy` to say ([`Queue::admits_dead_letter_source`]), and both paths that
//! would make a queue a source of it ask: the write that associates the pair
//! ([`ComAmazonawsSqsAmazonsqsState::redrive_allow_policy_refusal`], from `CreateQueue` and
//! `SetQueueAttributes`) and every automatic move out of an already associated source queue
//! ([`ComAmazonawsSqsAmazonsqsState::dead_letter_due`]) — so a permission tightened after
//! the wiring was made governs the next move as well. The permission DOCUMENT is judged when
//! it is written, like the redrive document above: a `sourceQueueArns` entry naming another
//! region's queue names a source this queue could never have, so it is refused rather than
//! stored ([`redrive_allow_policy_document_refusal`]).
//!
//! # Message movement tasks
//!
//! `StartMessageMoveTask` records a [`MessageMoveTask`] in
//! [`ComAmazonawsSqsAmazonsqsState::message_move_tasks`] and the TASK does the moving, over
//! its own schedule: [`ComAmazonawsSqsAmazonsqsState::advance_message_move_tasks`] takes
//! what the source queue can hand out as each message's slot comes round, counts it on the
//! record and marks the task completed once that queue holds nothing more. Every reader
//! applies what was already due before it reports, so a `COMPLETED` status is only ever
//! reported over a drain that really happened and no two readers disagree about a task's
//! progress. `ListMessageMoveTasks` reports those records through
//! [`ComAmazonawsSqsAmazonsqsState::reported_message_move_tasks`], which projects the
//! wire entry from the record exactly as
//! [`Queue::reported_attributes`] projects a queue's attributes — including which of that
//! entry's members are populated at all, each one only in the condition the member is
//! documented for.
//!
//! `CancelMessageMoveTask` stops such a task through
//! [`ComAmazonawsSqsAmazonsqsState::cancel_message_move_task`]: what the task has not moved
//! is still the source queue's own, so the cancellation instant recorded on the task is all
//! every later reader needs to report its status and its progress.
//!
//! # Queue permissions
//!
//! A permission is a statement of the queue's own `Policy` attribute rather than a
//! record beside it: `AddPermission` "generates a policy", so
//! [`Queue::granted_policy`] assembles the document the grant leaves the queue
//! carrying and [`Queue::revoke_permission`] takes the statement a label identifies
//! back out of it. Every grant is therefore reported by `GetQueueAttributes` and
//! replaced by an uploaded `SetQueueAttributes` policy, because there is only ever one
//! policy.
//!
//! # The backlog a client watches
//!
//! Those same three counts are also a published measurement: [`queue_depth_metrics`] samples
//! each queue's `(visible, in flight, delayed)` split, and the age of the oldest message it
//! can deliver, into the CloudWatch metric store as the `AWS/SQS` series dimensioned by the
//! queue's name, so a queue-depth dashboard and a backlog alarm read the depth this store
//! really holds. Beside them it counts what the data plane DID — the messages sent, received
//! and deleted, and the polls that got nothing ([`Queue::count_traffic`]). Both are published
//! off the answer by the one seam every request passes through, and the readings are taken by
//! the clock as well, because a delay or a visibility timeout lapsing moves a message between
//! two of those series with nobody calling.
//!
//! Everything a caller can make this in-memory store hold is bounded — see the
//! "Store bounds" section — and a bound is REFUSED rather than enforced by eviction,
//! because dropping a message a client was told had been accepted would lose it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use md5::{Digest, Md5};
use sha2::Sha256;

pub mod encryption;

use crate::ops::types::{
    InvalidAddress, InvalidAttributeName, InvalidAttributeValue, ListMessageMoveTasksResultEntry,
    MessageAttributeValue, MessageSystemAttributeValue, QueueDoesNotExist, ReceiptHandleIsInvalid,
    ResourceNotFoundException, UnsupportedOperation,
    MESSAGE_SYSTEM_ATTRIBUTE_NAME_FOR_SENDS_VALUES, QUEUE_ATTRIBUTE_NAME_VALUES,
};

/// Whether the policies in force let a request act on the queue it addresses — the gate
/// every operation that acts on a queue runs before it reports or changes anything — and
/// what a `Policy` attribute value has to look like to be stored at all.
pub mod authorization;
/// What a message attribute a send supplies has to look like, name and typed value alike.
mod message_attributes;
/// The bounds a queue policy is held to, and the wording of every refusal a grant or a
/// revoke answers with.
mod permissions;
/// The `AWS/SQS` series a queue publishes about itself: the backlog gauges it is sampled into
/// and the counters its data-plane calls are counted into, with the clock that keeps taking
/// those readings.
pub mod queue_depth_metrics;
/// The two redrive attributes a queue carries, and what each document has to look like to
/// be stored at all.
mod redrive;
pub use message_attributes::*;
pub use permissions::*;
pub use queue_depth_metrics::*;
pub use redrive::*;

/// The region an ARN carries when the request names none (an unsigned call, which
/// carries no SigV4 credential scope). Matches the region the emulator's own
/// generated config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, as `smithy_query(query =
/// "service_info")` reports it (`arnNamespace: sqs`, `sigv4Name: sqs`) — the
/// `service` segment of every queue ARN.
pub const ARN_SERVICE: &str = "sqs";

/// The longest queue name `CreateQueue` accepts: "A queue name can have up to 80
/// characters" (`CreateQueueRequest$QueueName` docs).
pub const MAX_QUEUE_NAME_LENGTH: usize = 80;

/// The first character of `body` that a message body may not hold, if any.
///
/// The accepted set is `SendMessage`'s own: "The following Unicode characters are
/// allowed ... #x9 | #xA | #xD | #x20 to #xD7FF | #xE000 to #xFFFD | #x10000 to
/// #x10FFFF". A Rust `char` can never be a surrogate, so the #xD800–#xDFFF gap between
/// the third and fourth range needs no arm of its own. A body holding anything else is
/// what `SendMessage` answers with the modelled `InvalidMessageContents`.
pub fn invalid_body_character(body: &str) -> Option<char> {
    body.chars().find(|character| {
        !matches!(*character,
            '\u{9}' | '\u{A}' | '\u{D}'
            | '\u{20}'..='\u{D7FF}'
            | '\u{E000}'..='\u{FFFD}'
            | '\u{10000}'..='\u{10FFFF}')
    })
}

/// How a refusal states that the body holds `character`, which is outside the allowed
/// set: the character that was found, as the code point the published set is written
/// in, and that set.
///
/// One wording for every call that takes a message body, so `SendMessage` and a
/// `SendMessageBatch` entry cannot drift on how they state the same defect.
pub fn invalid_body_character_message(character: char) -> String {
    format!(
        "Invalid binary character '#x{:X}' was found in the message body, the set of allowed \
         characters is #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF].",
        character as u32
    )
}

/// The suffix a FIFO queue's name must carry: "A FIFO queue name must end with the
/// .fifo suffix" (`CreateQueueRequest$QueueName` docs).
pub const FIFO_QUEUE_NAME_SUFFIX: &str = ".fifo";

/// The attribute that designates a queue as FIFO.
pub const FIFO_QUEUE_ATTRIBUTE: &str = "FifoQueue";

/// The one value [`FIFO_QUEUE_ATTRIBUTE`] designates a queue with: "Designates a queue
/// as FIFO" is all the attribute does, and this service publishes no attribute that
/// designates a queue as a standard one — a create that names none makes one. A value
/// that designates nothing is therefore not this attribute at all; see
/// [`AttributeKind::Designator`].
pub const DESIGNATES_FIFO_QUEUE: &str = "true";

/// The attribute that makes a FIFO queue derive a message's deduplication token from
/// its body: "Enables content-based deduplication."
pub const CONTENT_BASED_DEDUPLICATION: &str = "ContentBasedDeduplication";

/// The attribute that says whether deduplication is per message group or per queue:
/// "Specifies whether message deduplication occurs at the message group or queue
/// level. Valid values are messageGroup and queue."
pub const DEDUPLICATION_SCOPE: &str = "DeduplicationScope";

/// The [`DEDUPLICATION_SCOPE`] value that scopes a deduplication token to its message
/// group; any other value (including none) scopes it to the whole queue, which is the
/// level the attribute's own documentation names as the alternative.
pub const DEDUPLICATION_SCOPE_MESSAGE_GROUP: &str = "messageGroup";

/// The attribute that says whether the FIFO throughput quota "applies to the entire
/// queue or per message group": "Valid values are perQueue and perMessageGroupId."
pub const FIFO_THROUGHPUT_LIMIT: &str = "FifoThroughputLimit";

/// The [`FIFO_THROUGHPUT_LIMIT`] value that scopes the quota to each message group. It
/// "is allowed only when the value for DeduplicationScope is messageGroup", which is
/// what [`attribute_combination_refusal`] enforces.
pub const FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP: &str = "perMessageGroupId";

/// How long a deduplication token suppresses a resend: a message carrying a
/// `MessageDeduplicationId` already seen is "accepted successfully but aren't delivered
/// during the 5-minute deduplication interval"
/// (`SendMessageRequest$MessageDeduplicationId`).
pub const DEDUPLICATION_INTERVAL: Duration = Duration::from_secs(300);

/// How long a `ReceiveRequestAttemptId` can be retried under: "You can use
/// ReceiveRequestAttemptId only for 5 minutes after a ReceiveMessage action"
/// (`ReceiveMessageRequest$ReceiveRequestAttemptId`).
pub const RECEIVE_REQUEST_ATTEMPT_INTERVAL: Duration = Duration::from_secs(300);

/// The wait the `QueueDeletedRecently` refusal STATES, which is Amazon SQS's own figure:
/// "If you delete a queue, you must wait at least 60 seconds before creating a queue with
/// the same name" (`CreateQueue`'s own documentation), worded in the refusal as "You must
/// wait 60 seconds after deleting a queue before you can create another with the same
/// name." — the model's documentation of that error shape with its second "queue"
/// dropped, as the recorded refusal words it. It is part of the fixed text of the class
/// through [`queue_deleted_recently_message`], so the sentence says what AWS says however
/// long this emulator actually defers the name.
pub const QUEUE_DELETED_RECENTLY_DOCUMENTED_WAIT: Duration = Duration::from_secs(60);

/// How long a deleted queue's NAME actually stays unavailable here: the delete makes the
/// name available again only once the deletion has propagated, so a re-create straight
/// after a delete is refused and one a moment later is served. The span is this
/// emulator's own — long enough that a caller re-creating a name it has just deleted sees
/// the refusal, short enough that a caller backing off and retrying gets its queue — and
/// is not the figure AWS's sentence quotes
/// ([`QUEUE_DELETED_RECENTLY_DOCUMENTED_WAIT`]).
pub const QUEUE_DELETED_RECENTLY_INTERVAL: Duration = Duration::from_millis(500);

/// How long an accepted `PurgeQueue` keeps the next one on the same queue refused:
/// "the specified queue previously received a PurgeQueue request within the last 60
/// seconds (the time it can take to delete the messages in the queue)"
/// (`PurgeQueueInProgress`), while "the message deletion process takes up to 60 seconds"
/// (`PurgeQueue`). The interval is part of the fixed text of that refusal too, through
/// [`purge_queue_in_progress_message`].
pub const PURGE_QUEUE_INTERVAL: Duration = Duration::from_secs(60);

/// The longest `MessageGroupId` / `MessageDeduplicationId` a message may carry: "The
/// length of MessageGroupId is 128 characters" and "The maximum length of
/// MessageDeduplicationId is 128 characters" (their own member documentation), which is
/// also "the maximum length of ReceiveRequestAttemptId".
pub const MAX_MESSAGE_TOKEN_LENGTH: usize = 128;

/// The value the first sequence number a FIFO queue assigns counts up from.
///
/// `SendMessageResult$SequenceNumber` documents the value as "the large,
/// non-consecutive number that Amazon SQS assigns to each message. The length of
/// SequenceNumber is 128 bits. SequenceNumber continues to increase for a particular
/// MessageGroupId" — so what a client may rely on is a decimal integer inside 128 bits
/// that increases. Counting up from here makes them all the same width, which is what a
/// client comparing them as text needs.
pub const SEQUENCE_NUMBER_BASE: u128 = 10_u128.pow(19);

/// Why the FIFO token called `member` — a `MessageGroupId`, a `MessageDeduplicationId`
/// or a `ReceiveRequestAttemptId` — cannot be accepted, as the text of the refusal a
/// caller has to fix the request from, or `None` when it can.
///
/// The rule is the one all three members publish: at most [`MAX_MESSAGE_TOKEN_LENGTH`]
/// characters, "alphanumeric characters and punctuation
/// (!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~)" — which is exactly the ASCII punctuation set.
/// An empty token names no group, no send and no receive attempt, so it is not one
/// either.
pub fn message_token_refusal(member: &str, token: &str) -> Option<String> {
    let length = token.chars().count();
    let within_length = (1..=MAX_MESSAGE_TOKEN_LENGTH).contains(&length);
    let printable = token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c.is_ascii_punctuation());
    if within_length && printable {
        return None;
    }
    // One wording for both halves of the one rule the member publishes, because that is
    // how the rule is published: the accepted characters and the accepted length in a
    // single sentence naming the member.
    Some(invalid_parameter_message(
        token,
        member,
        &format!(
            "{member} can only include alphanumeric and punctuation characters. 1 to \
             {MAX_MESSAGE_TOKEN_LENGTH} in length."
        ),
    ))
}

/// How many cost allocation tags one queue may carry.
///
/// The `TagQueue`/`CreateQueueRequest$tags` documentation makes 50 the guideline
/// ("Adding more than 50 tags to a queue isn't recommended") and points at "Quotas
/// related to queues" for "a full list of tag restrictions", which publishes 50 tags
/// per queue. It is enforced here as a bound, over the tag set a call would LEAVE the
/// queue with, so no sequence of tagging calls grows one queue's map without limit.
pub const MAX_TAGS_PER_QUEUE: usize = 50;

/// The longest tag key a queue accepts: 128 characters, per the same "Quotas related
/// to queues" tag restrictions.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value a queue accepts: 256 characters, per the same "Quotas related
/// to queues" tag restrictions.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The tag-key prefix reserved for the provider's own tags, which a caller may not
/// supply: the same "Quotas related to queues" tag restrictions state "Do not use the
/// `aws:` prefix in your tag names or values because it is reserved for Amazon Web
/// Services use", and a reserved key cannot be created, edited or deleted by a caller.
///
/// Matched case-insensitively, because the reservation is on the prefix itself rather
/// than on one spelling of it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, compared
/// without allocating and without assuming a spelling: the bytes of an ASCII prefix are
/// a char boundary of whatever follows them, so the comparison is safe on any key.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Attributes `GetQueueAttributes` reports but no caller sets: they are properties of
/// the queue record itself, so [`Queue::reported_attributes`] derives each one.
pub const DERIVED_ATTRIBUTES: &[&str] = &["QueueArn", "CreatedTimestamp", "LastModifiedTimestamp"];

/// Attributes that count MESSAGES ("the approximate number of messages available for
/// retrieval", "... that are delayed", "... that are in flight").
///
/// [`Queue::reported_attributes`] derives each from the queue's own message deque, so
/// they are reported but never settable — which is why they appear here and not in
/// [`SETTABLE_ATTRIBUTES`].
pub const MESSAGE_PLANE_ATTRIBUTES: &[&str] = &[
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesDelayed",
    "ApproximateNumberOfMessagesNotVisible",
];

/// The `AttributeNames` value that asks for every attribute the queue has.
pub const ALL_ATTRIBUTES: &str = "All";

/// What values a queue attribute accepts, from that attribute's own entry in the
/// `CreateQueue`/`SetQueueAttributes` `Attributes` documentation.
#[derive(Debug, Clone, Copy)]
pub enum AttributeKind {
    /// "An integer from <min> to <max>", inclusive on both bounds.
    Integer { min: i64, max: i64 },
    /// "Valid values are `true` and `false`."
    Boolean,
    /// A documented value set.
    OneOf(&'static [&'static str]),
    /// A JSON document ("a valid Amazon Web Services policy", "... as a JSON object").
    Json,
    /// An opaque string the service does not constrain.
    Text,
    /// An attribute that exists only to DESIGNATE what the queue is, carrying the one
    /// value that designates it. The service publishes no attribute designating the
    /// opposite, so a request supplying any other value asks for an attribute that does
    /// not exist rather than for a value outside a published set — which is why such a
    /// value is refused as an UNKNOWN NAME ([`unknown_attribute_name`]) and not as a bad
    /// value.
    Designator(&'static str),
}

/// One settable queue attribute: what it accepts, when it may be supplied, and the
/// value a queue carries when a create names none.
#[derive(Debug)]
pub struct AttributeSpec {
    /// The attribute name, as the model's `QueueAttributeName` enum spells it.
    pub name: &'static str,
    /// The values it accepts.
    pub kind: AttributeKind,
    /// True when only `CreateQueue` may supply it: "You can provide this attribute
    /// only during queue creation. You can't change it for an existing queue."
    pub create_only: bool,
    /// True when it "applies only to FIFO (first-in-first-out) queues".
    pub fifo_only: bool,
    /// The documented default ("Default: 30."), applied by `CreateQueue` when the
    /// request names no value, and only where the attribute APPLIES to the queue being
    /// created: a `fifo_only` default is materialised for a FIFO queue and for no other,
    /// since a standard queue has no such attribute at all. `None` for an attribute the
    /// docs give no default.
    pub default: Option<&'static str>,
}

/// Every attribute a caller may supply, with the bounds, value sets and defaults the
/// `Attributes` member of `CreateQueue`/`SetQueueAttributes` documents for it.
///
/// `SetQueueAttributes` lists exactly these minus `FifoQueue`, which is why that one
/// entry carries `create_only`. Name order, so a request carrying two bad attributes
/// is always refused for the same one.
pub const SETTABLE_ATTRIBUTES: &[AttributeSpec] = &[
    AttributeSpec {
        // "Enables content-based deduplication. Valid values are true and false." A FIFO
        // queue always carries the attribute: one whose create enabled nothing reads back
        // as deduplication that is not content-based, which is `false` — "absent" is not a
        // value this attribute has.
        name: CONTENT_BASED_DEDUPLICATION,
        kind: AttributeKind::Boolean,
        create_only: false,
        fifo_only: true,
        default: Some("false"),
    },
    AttributeSpec {
        // "Specifies whether message deduplication occurs at the message group or
        // queue level. Valid values are messageGroup and queue."
        name: DEDUPLICATION_SCOPE,
        kind: AttributeKind::OneOf(&[DEDUPLICATION_SCOPE_MESSAGE_GROUP, "queue"]),
        create_only: false,
        fifo_only: true,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 0 to 900 seconds (15 minutes). Default: 0."
        name: DELAY_SECONDS,
        kind: AttributeKind::Integer { min: 0, max: 900 },
        create_only: false,
        fifo_only: false,
        default: Some("0"),
    },
    AttributeSpec {
        // "Designates a queue as FIFO. ... You can provide this attribute only during
        // queue creation." Designating is all it does, so the only value it carries is
        // the one that designates — see [`AttributeKind::Designator`].
        name: FIFO_QUEUE_ATTRIBUTE,
        kind: AttributeKind::Designator(DESIGNATES_FIFO_QUEUE),
        create_only: true,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values are perQueue and perMessageGroupId."
        name: FIFO_THROUGHPUT_LIMIT,
        kind: AttributeKind::OneOf(&["perQueue", FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP]),
        create_only: false,
        fifo_only: true,
        default: None,
    },
    AttributeSpec {
        // "An integer representing seconds, between 60 seconds (1 minute) and 86,400
        // seconds (24 hours). Default: 300 (5 minutes)." The default is NOT stored at
        // create: it is the period a KMS key is used under, so it is reported for a
        // queue that is ON a KMS key — see [`encryption::report`].
        name: encryption::KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
        kind: AttributeKind::Integer {
            min: 60,
            max: 86_400,
        },
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "The ID of an Amazon Web Services managed customer master key (CMK) for
        // Amazon SQS or a custom CMK" — an opaque key id, alias or ARN. An EMPTY value
        // takes the queue off its key, which is why the form accepts any text; which
        // option the queue is then on is [`encryption::apply_encryption_choice`].
        name: encryption::KMS_MASTER_KEY_ID,
        kind: AttributeKind::Text,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 1,024 bytes (1 KiB) to 1,048,576 bytes
        // (1 MiB). Default: 1,048,576 bytes (1 MiB)."
        name: MAXIMUM_MESSAGE_SIZE,
        kind: AttributeKind::Integer {
            min: 1_024,
            max: 1_048_576,
        },
        create_only: false,
        fifo_only: false,
        default: Some("1048576"),
    },
    AttributeSpec {
        // "Valid values: An integer from 60 seconds (1 minute) to 1,209,600 seconds
        // (14 days). Default: 345,600 (4 days)."
        name: MESSAGE_RETENTION_PERIOD,
        kind: AttributeKind::Integer {
            min: 60,
            max: 1_209_600,
        },
        create_only: false,
        fifo_only: false,
        default: Some("345600"),
    },
    AttributeSpec {
        // "The queue's policy. A valid Amazon Web Services policy."
        name: POLICY,
        kind: AttributeKind::Json,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 0 to 20 (seconds). Default: 0."
        name: RECEIVE_MESSAGE_WAIT_TIME_SECONDS,
        kind: AttributeKind::Integer { min: 0, max: 20 },
        create_only: false,
        fifo_only: false,
        default: Some("0"),
    },
    AttributeSpec {
        // "The string that includes the parameters for the permissions for the
        // dead-letter queue redrive permission ... as a JSON object."
        name: REDRIVE_ALLOW_POLICY,
        kind: AttributeKind::Json,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "The string that includes the parameters for the dead-letter queue
        // functionality of the source queue as a JSON object."
        name: REDRIVE_POLICY,
        kind: AttributeKind::Json,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Enables server-side queue encryption using SQS owned encryption keys."
        // No value is STORED for a create that names none — a queue carries only what
        // its create supplied — and which option the queue is on, SQS-owned key or KMS
        // key, is what [`encryption::report`] answers from.
        name: encryption::SQS_MANAGED_SSE_ENABLED,
        kind: AttributeKind::Boolean,
        create_only: false,
        fifo_only: false,
        default: None,
    },
    AttributeSpec {
        // "Valid values: An integer from 0 to 43,200 (12 hours). Default: 30."
        name: VISIBILITY_TIMEOUT,
        kind: AttributeKind::Integer { min: 0, max: 43_200 },
        create_only: false,
        fifo_only: false,
        default: Some("30"),
    },
];

/// The spec for `name`, if a caller may supply that attribute at all.
pub fn settable_attribute(name: &str) -> Option<&'static AttributeSpec> {
    SETTABLE_ATTRIBUTES.iter().find(|spec| spec.name == name)
}

/// Whether `spec` is an attribute a queue of this kind HAS at all — `fifo` says whether that
/// queue is a FIFO queue.
///
/// Two ways of belonging to the FIFO kind of queue and to no other: an attribute that
/// "applies only to FIFO (first-in-first-out) queues" (`fifo_only`), and the attribute that
/// DESIGNATES a queue as FIFO ([`AttributeKind::Designator`]) — a standard queue carries no
/// attribute saying it is FIFO, which is why a read of one reports none. Stated once, because
/// it decides both which defaults a create materialises and whether a call naming the
/// attribute named one the queue it addresses does not have
/// ([`validate_attributes`]).
pub fn applies_to_queue(spec: &AttributeSpec, fifo: bool) -> bool {
    let designates_the_fifo_kind =
        matches!(spec.kind, AttributeKind::Designator(designating) if designating == DESIGNATES_FIFO_QUEUE);
    fifo || !(spec.fifo_only || designates_the_fifo_kind)
}

// The message-plane attributes the message handlers read, named once so a handler
// never spells one as a literal.
pub const DELAY_SECONDS: &str = "DelaySeconds";
pub const MAXIMUM_MESSAGE_SIZE: &str = "MaximumMessageSize";
pub const MESSAGE_RETENTION_PERIOD: &str = "MessageRetentionPeriod";
pub const RECEIVE_MESSAGE_WAIT_TIME_SECONDS: &str = "ReceiveMessageWaitTimeSeconds";
pub const VISIBILITY_TIMEOUT: &str = "VisibilityTimeout";
/// The attribute carrying the queue's access policy ("The queue's policy. A valid
/// Amazon Web Services policy.").
pub const POLICY: &str = "Policy";

/// The attribute that makes a queue a dead-letter SOURCE queue: it names the queue its
/// messages are driven to.
pub const REDRIVE_POLICY: &str = "RedrivePolicy";

/// The member of a `RedrivePolicy` naming the queue the messages go to: "the Amazon
/// Resource Name (ARN) of the dead-letter queue to which Amazon SQS moves messages after
/// the value of `maxReceiveCount` is exceeded" (the attribute's own documentation). One of
/// the two the attribute documents, and one a policy has to carry.
pub const DEAD_LETTER_TARGET_ARN_MEMBER: &str = "deadLetterTargetArn";

/// The member of a `RedrivePolicy` bounding the deliveries: "the number of times a message
/// is delivered to the source queue before being moved to the dead-letter queue" (the
/// attribute's own documentation). The other member a policy has to carry, and the one
/// documented as a NUMBER.
pub const MAX_RECEIVE_COUNT_MEMBER: &str = "maxReceiveCount";

/// The inclusive range a `maxReceiveCount` may carry, as Amazon SQS publishes it for that
/// member: a count below the lower bound admits no delivery for the queue to make, and the
/// upper bound is the most deliveries a dead-letter configuration can ask for. The refusal
/// of a count outside it states these bounds ([`max_receive_count_reason`]).
pub const MAX_RECEIVE_COUNT_RANGE: (i64, i64) = (1, 1_000);

/// The attribute a dead-letter queue carries to say WHICH queues may drive their messages
/// to it: "the parameters for the permissions for the dead-letter queue redrive permission
/// and which source queues can specify dead-letter queues as a JSON object" (the
/// `Attributes` member's own documentation).
pub const REDRIVE_ALLOW_POLICY: &str = "RedriveAllowPolicy";

/// The member of a `RedriveAllowPolicy` carrying its permission type: "the permission type
/// that defines which source queues can specify the current queue as the dead-letter
/// queue".
const REDRIVE_PERMISSION_MEMBER: &str = "redrivePermission";

/// The member of a `RedriveAllowPolicy` listing the queues `byQueue` admits: "the Amazon
/// Resource Names (ARN)s of the source queues that can specify this queue as the
/// dead-letter queue and redrive messages".
const SOURCE_QUEUE_ARNS_MEMBER: &str = "sourceQueueArns";

/// "`allowAll` – (Default) Any source queues in this Amazon Web Services account in the
/// same Region can specify this queue as the dead-letter queue."
pub const REDRIVE_PERMISSION_ALLOW_ALL: &str = "allowAll";

/// "`denyAll` – No source queues can specify this queue as the dead-letter queue."
pub const REDRIVE_PERMISSION_DENY_ALL: &str = "denyAll";

/// "`byQueue` – Only queues specified by the `sourceQueueArns` parameter can specify this
/// queue as the dead-letter queue."
pub const REDRIVE_PERMISSION_BY_QUEUE: &str = "byQueue";

/// The inclusive `(min, max)` the documentation gives integer attribute `name`, from
/// [`SETTABLE_ATTRIBUTES`] — the one place those bounds are written.
///
/// A name that is not an integer attribute is unbounded, so no caller has to unwrap.
pub fn integer_bounds(name: &str) -> (i64, i64) {
    match settable_attribute(name).map(|spec| spec.kind) {
        Some(AttributeKind::Integer { min, max }) => (min, max),
        _ => (0, i64::MAX),
    }
}

// ---------------------------------------------------------------------------
// Store bounds
// ---------------------------------------------------------------------------
//
// The store is in-memory and the emulator authenticates nobody, so everything a
// caller can make it hold is bounded. NOT fidelity claims about any published quota:
// each is the emulator's own ceiling, generous enough that no realistic workflow meets
// it, so a caller looping on a request cannot grow this process's memory without
// bound.

/// How many queues this emulator will hold; `CreateQueue` refuses past it.
pub const MAX_QUEUES: usize = 1_000;

/// How this store states that it holds as many queues as it will, naming the queue that
/// could not be made.
///
/// The emulator's own ceiling rather than a published Amazon SQS quota, so the sentence
/// says what it is. One wording for every API that provisions a queue — `CreateQueue`, and
/// the Cloud Control API resource handler that stands behind `AWS::SQS::Queue` — so two
/// paths cannot drift on how they state the same ceiling.
pub fn queues_quota_message(name: &str) -> String {
    format!("Cannot create queue {name}: this emulator already holds {MAX_QUEUES} queues.")
}

/// How many recently-deleted queue names the store remembers (see
/// [`ComAmazonawsSqsAmazonsqsState::recently_deleted`]).
///
/// An entry lapses with [`QUEUE_DELETED_RECENTLY_INTERVAL`] and is forgotten as soon as
/// anything touches the ledger, so this bound is only reached by a caller deleting
/// names faster than they lapse. Past it the oldest records are forgotten first: unlike
/// a message, a deletion record is derived data whose only effect is deferring a
/// re-create for a minute, so forgetting one loses nothing a client was told had been
/// accepted.
pub const MAX_RECENTLY_DELETED_NAMES: usize = MAX_QUEUES;

/// How many messages one queue holds. `MessageRetentionPeriod` is an AWS duration
/// (4 days by default, up to 14), far longer than an emulator session, so it does not
/// bound memory on its own: `SendMessage` refuses past this.
pub const MAX_MESSAGES_PER_QUEUE: usize = 1_000;

/// How many message bytes one queue holds, so a few large messages are bounded as well
/// as many small ones.
pub const MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE: usize = 8 * 1024 * 1024;

/// How many message bytes the whole store holds, across every queue: the per-queue
/// caps alone would still scale with the number of queues.
pub const MAX_RETAINED_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// How many bytes one attribute VALUE whose form admits text of any length may carry:
/// the `Json` attributes (`Policy`, `RedrivePolicy`, `RedriveAllowPolicy`) and the
/// `Text` ones (`KmsMasterKeyId`).
///
/// Every other kind is bounded by its own form (an integer, a boolean, one of a list).
/// These are not, and a queue keeps its attributes for as long as it exists, so without
/// this a caller looping on `CreateQueue`/`SetQueueAttributes` could grow the store
/// without end — the one thing the bounds above exist to prevent. The figure is the
/// ceiling AWS publishes for the largest of them: "The policy size is limited to 8,192
/// bytes" (the `Policy` attribute's own documentation).
pub const MAX_ATTRIBUTE_VALUE_BYTES: usize = 8 * 1024;

/// The page size `ListQueues` uses when the caller gives no `MaxResults`, so one
/// unparameterised request cannot materialise a response that grows with the stored
/// queue count. A client walks past it with `NextToken`.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// The inclusive range `ListQueues` accepts for `MaxResults`: "Maximum number of results
/// to include in the response. Value range is 1 to 1000"
/// (`ListQueuesRequest$MaxResults`). A value outside it is refused rather than clamped,
/// so a paginating client is told its request was wrong.
pub const LIST_QUEUES_MAX_RESULTS_RANGE: (i32, i32) = (1, 1_000);

/// The inclusive range `ListDeadLetterSourceQueues` accepts for `MaxResults`: "Maximum
/// number of results to include in the response. Value range is 1 to 1000"
/// (`ListDeadLetterSourceQueuesRequest$MaxResults`) — the same range `ListQueues`
/// publishes for its own page size, so the two name ONE value rather than two that have
/// to be kept equal. Refused rather than clamped, as every page size this service takes
/// is.
pub const LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE: (i32, i32) =
    LIST_QUEUES_MAX_RESULTS_RANGE;

/// How many source queues `ListDeadLetterSourceQueues` reports when the caller names no
/// `MaxResults`: "If you do not set MaxResults, the response includes a maximum of 1,000
/// results" (`ListDeadLetterSourceQueues`), which is the upper end of the range that
/// parameter publishes. It is at least [`MAX_QUEUES`], so an unparameterised request
/// cannot leave a source queue behind.
pub const LIST_DEAD_LETTER_SOURCE_QUEUES_DEFAULT_RESULTS: usize =
    LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE.1 as usize;

/// How every paged listing of this service refuses a pagination token it did not issue,
/// or one that no longer continues anything.
///
/// Amazon SQS states this refusal as WHOLLY FIXED text: it names neither the parameter
/// that carried the token nor the token itself, so a client (or an SDK paginator) that
/// recognises the sentence to restart a listing from its first page matches it exactly,
/// and nothing a caller supplied comes back in the message. It is therefore a constant
/// rather than one of the `Value … for parameter …` templates below, which exist for the
/// refusals that DO report the value they rejected.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid or expired next token.";

/// Where in a listing the `NextToken` a request carried continues it, or the text of the
/// refusal a token this listing never issued is answered with.
///
/// Every paged listing of this service carries its cursor the same way: the COUNT of
/// entries already handed over. `total` is how many entries the listing holds now, and a
/// token that is not such a count — unparseable, or past the end of the listing it claims
/// to continue — is refused with [`INVALID_NEXT_TOKEN_MESSAGE`] rather than silently
/// treated as "start again", which would loop a paging client forever. A request carrying
/// no token starts at the beginning.
///
/// One place for the rule and its wording, so two listings cannot drift on either.
pub fn count_cursor(token: Option<&str>, total: usize) -> Result<usize, String> {
    match token {
        None => Ok(0),
        Some(token) => match token.parse() {
            Ok(start) if start <= total => Ok(start),
            _ => Err(INVALID_NEXT_TOKEN_MESSAGE.to_string()),
        },
    }
}

/// The `NextToken` a page that began at `start` and handed over `handed` of `total`
/// entries answers with: the count the NEXT page continues from, and nothing at all for a
/// page that reached the end of the listing, which is how a paging client stops.
pub fn next_count_cursor(start: usize, handed: usize, total: usize) -> Option<String> {
    let next = start + handed;
    (next < total).then(|| next.to_string())
}

/// How one listing states the reason it will not take the page size it was given.
///
/// The sentence is the listing's OWN: Amazon SQS words the same refusal differently per
/// operation, so the call site names the wording it answers with rather than every
/// listing sharing one. Neither form carries the rejected value — only the bounds that
/// listing's `MaxResults` member publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxResultsWording {
    /// "MaxResults must be an integer between <min> and <max>." — how the queue listings
    /// state the range their `MaxResults` publishes ("Value range is 1 to 1000").
    IntegerBetween,
    /// "You must enter a number that's between <min> and <max>." — how the message-move
    /// task listing states its own ("The upper limit is 10").
    NumberBetween,
}

/// Why the value a listing's page-size parameter carried was refused, as the text of the
/// refusal a caller has to fix the request from, or `None` when `requested` is inside the
/// inclusive `(min, max)` that parameter publishes.
///
/// One place for the RULE — a page size outside the range its member publishes is refused
/// rather than clamped — with `wording` choosing the sentence the listing answers with
/// (see [`MaxResultsWording`]). The bounds in that sentence are the ones passed in, which
/// are each parameter's documented range.
pub fn max_results_refusal(
    parameter: &str,
    (min, max): (i32, i32),
    requested: i32,
    wording: MaxResultsWording,
) -> Option<String> {
    (requested < min || requested > max).then(|| {
        let reason = match wording {
            MaxResultsWording::IntegerBetween => {
                format!("{parameter} must be an integer between {min} and {max}.")
            }
            MaxResultsWording::NumberBetween => {
                format!("You must enter a number that's between {min} and {max}.")
            }
        };
        invalid_parameter_message_without_value(parameter, &reason)
    })
}

// The per-queue tag bounds live with the other tag quotas above:
// [`MAX_TAGS_PER_QUEUE`], [`MAX_TAG_KEY_LENGTH`] and [`MAX_TAG_VALUE_LENGTH`].

/// How many attributes one message may carry. Amazon SQS Developer Guide: "Each message
/// can have up to 10 attributes."
pub const MAX_MESSAGE_ATTRIBUTES: usize = 10;

/// How many superseded receipt handles one queue remembers (see
/// [`Queue::superseded_handles`]).
///
/// One message can be received any number of times, so the handles a queue has superseded
/// are bounded by this emulator rather than by what it holds, exactly as the deleted ones
/// are: past the bound the handles superseded earliest are forgotten, and a caller
/// presenting one of those is answered for a handle this queue no longer recognises.
pub const MAX_SUPERSEDED_HANDLES_PER_QUEUE: usize = MAX_MESSAGES_PER_QUEUE;

/// How many receipt handles one queue remembers as deleted (see
/// [`Queue::deleted_handles`]).
///
/// A delete is idempotent, so the handle a delete consumed outlives the message it named;
/// past this bound the queue forgets the handles it retired first. Like a deduplication
/// token, a retired handle is derived data whose only effect is answering a repeat of a
/// delete that already succeeded, so forgetting one loses nothing a client was told had
/// been accepted.
pub const MAX_DELETED_HANDLES_PER_QUEUE: usize = MAX_MESSAGES_PER_QUEUE;

/// How many deduplication tokens one queue remembers. A token outlives the message it
/// suppressed ("Amazon SQS continues to keep track of the message deduplication ID even
/// after the message is received and deleted"), so the message count does not bound it:
/// past this, the queue forgets the oldest tokens first. Unlike a message, a token is
/// derived data whose only effect is suppressing a resend inside
/// [`DEDUPLICATION_INTERVAL`], so forgetting one loses nothing a client was told had
/// been accepted — which is why this bound is enforced by eviction and the message
/// bounds are not.
pub const MAX_DEDUPLICATION_TOKENS_PER_QUEUE: usize = 1_000;

/// How many `ReceiveRequestAttemptId` batches one queue remembers. Each one lapses with
/// [`RECEIVE_REQUEST_ATTEMPT_INTERVAL`] and names at most the messages one receive can
/// hand over, and its only effect is replaying that batch to a retry — derived data, so
/// past this bound the queue forgets the oldest attempts first rather than refusing the
/// receive, exactly as it forgets the oldest deduplication tokens.
pub const MAX_RECEIVE_ATTEMPTS_PER_QUEUE: usize = 1_000;

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Where a stored message stands relative to the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Its delay has not elapsed yet, so it is not available for retrieval.
    Delayed,
    /// It was handed to a consumer and its visibility timeout has not lapsed.
    InFlight,
    /// It is available for retrieval now.
    Visible,
}

/// Why a visibility change was refused. Each handler renders it as its own modelled error,
/// so the wire code is the one that operation declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityRefusal {
    /// The value is not formed as a receipt handle at all, so no receive can have minted
    /// it and it names no message of any queue.
    MalformedHandle,
    /// It IS a receipt handle, but this queue holds no message under it: the message was
    /// deleted, purged or expired, or the handle is one this queue never minted.
    NoSuchMessage,
    /// This queue minted it for a message it still holds, and a later receive of that
    /// message took the lease it carried (FIFO only).
    ExpiredHandle,
    /// The span asked for reaches past what ONE receive may hide the message for, counted
    /// from that receive.
    BeyondLimit,
}

/// One message a queue holds.
#[derive(Debug, Clone)]
pub struct StoredMessage {
    /// The identifier minted when the message was sent.
    pub message_id: String,
    /// The body exactly as sent.
    pub body: String,
    /// MD5 hex digest of [`body`](Self::body).
    pub md5_of_body: String,
    /// The message attributes the sender supplied.
    pub message_attributes: BTreeMap<String, MessageAttributeValue>,
    /// The message system attributes the sender supplied — "currently, the only
    /// supported message system attribute is AWSTraceHeader", which a receive reports
    /// under that name when the request selects it.
    pub message_system_attributes: BTreeMap<String, MessageSystemAttributeValue>,
    /// The `MessageGroupId` the send carried, if any: the group whose messages a FIFO
    /// queue hands over "one at a time, in strict order". A standard queue stores it
    /// too — the send may carry it there — but does not order by it.
    pub message_group_id: Option<String>,
    /// The `MessageDeduplicationId` this message was accepted under, either the one the
    /// send supplied or the one `ContentBasedDeduplication` derived from the body. Only
    /// a FIFO queue has one, and it "is available to the consumer of the message".
    pub message_deduplication_id: Option<String>,
    /// The sequence number a FIFO queue assigned this message; `None` on a standard
    /// queue, which assigns none.
    pub sequence_number: Option<String>,
    /// When `SendMessage` accepted it. What a receive reports under `SentTimestamp`, and
    /// the instant the message was SENT whatever queue it is in now: moving a message
    /// between queues — to a dead-letter queue, or back out of one — does not re-send it.
    pub sent_at: SystemTime,
    /// When the queue holding this message ENQUEUED it, which is where its
    /// `MessageRetentionPeriod` is counted from.
    ///
    /// The same instant as [`sent_at`](Self::sent_at) for a message in the queue it was
    /// sent to, where "the expiration of a message is always based on its original
    /// enqueue timestamp" — and the two part company only where a queue enqueues a
    /// message it was handed rather than one a sender gave it (see
    /// [`ComAmazonawsSqsAmazonsqsState::dead_letter_due`]).
    pub retention_from: SystemTime,
    /// The principal id of the caller whose send this message arrived on — what
    /// `ReceiveMessage` reports under `SenderId`: "the Amazon Web Services account number
    /// (or the IP address, if anonymous access is allowed) of the sender", which for an
    /// IAM identity is the id that identifies it (an IAM user's `UserId`, or
    /// `<role id>:<session name>` for an assumed-role session — the same value
    /// `sts:GetCallerIdentity` answers that credential with). Stamped at enqueue time, so
    /// every later read reports the sender the message really arrived from, and it travels
    /// with the message when it changes queue.
    pub sender_id: String,
    /// `sent_at` + the effective `DelaySeconds`: before this instant the message is
    /// [`Visibility::Delayed`].
    pub available_at: SystemTime,
    /// Set by a receive to `now + effective visibility timeout`; cleared once the clock
    /// passes it.
    pub invisible_until: Option<SystemTime>,
    /// How many times the message has been handed to a consumer without being deleted.
    /// Counted across every queue that has held it — "the number of times a message has
    /// been received across all queues but not deleted" (`ApproximateReceiveCount`) — so
    /// a message that changes queue keeps it.
    pub receive_count: u32,
    /// How many times the queue HOLDING this message has handed it to a consumer, which
    /// is what its `RedrivePolicy` bounds: `maxReceiveCount` is "the number of times a
    /// message is delivered to the source queue before being moved to the dead-letter
    /// queue". A message that enters another queue starts that queue's count afresh,
    /// because the deliveries it had elsewhere were not deliveries of this queue — which
    /// is what lets a message driven back out of a dead-letter queue be processed again
    /// rather than dead-lettered on sight.
    pub queue_receive_count: u32,
    /// When it was first handed to a consumer.
    pub first_received_at: Option<SystemTime>,
    /// When it was LAST handed to a consumer, which is the instant the receipt handle
    /// that receive minted was minted at. The visibility a caller may still ask for is
    /// counted from here, so a change of this message's visibility timeout cannot keep
    /// it hidden for longer than the span one receive may hide it for.
    pub last_received_at: Option<SystemTime>,
    /// The ARN of the queue this message was MOVED OUT OF when a `RedrivePolicy` drove it
    /// to a dead-letter queue — what a receive reports under `DeadLetterQueueSourceArn`,
    /// and `None` for a message no redrive has moved. Stamped at the move
    /// ([`ComAmazonawsSqsAmazonsqsState::dead_letter_due`]) and kept with the message
    /// afterwards, so a dead-letter queue several queues redrive into can still be
    /// triaged by where each message came from; a later redrive re-stamps it with the
    /// queue that one moved the message out of.
    pub dead_letter_source_arn: Option<String>,
}

impl StoredMessage {
    /// Everything this message retains, in bytes: the body plus both attribute maps,
    /// which hold caller-supplied names, data types, strings and blobs. Amazon SQS
    /// counts message attributes toward a message's size and system attributes not at
    /// all, but this emulator's byte budgets exist to bound the store, so they count
    /// every byte a sender can make it hold.
    pub fn retained_bytes(&self) -> usize {
        self.body
            .len()
            .saturating_add(message_attribute_bytes(&self.message_attributes))
            .saturating_add(message_system_attribute_bytes(&self.message_system_attributes))
    }

    /// Where this message stands at `now`.
    pub fn visibility(&self, now: SystemTime) -> Visibility {
        if now < self.available_at {
            return Visibility::Delayed;
        }
        match self.invisible_until {
            Some(until) if now < until => Visibility::InFlight,
            _ => Visibility::Visible,
        }
    }

    /// The message group this message belongs to, as the empty group when it carries no
    /// `MessageGroupId` — which is every message of a standard queue that named none.
    pub fn group(&self) -> &str {
        self.message_group_id.as_deref().unwrap_or_default()
    }
}

/// What a queue remembers about a deduplication token it has already accepted: the
/// message that token admitted, so a resend inside [`DEDUPLICATION_INTERVAL`] is
/// "accepted successfully but aren't delivered" and still answers for that message.
#[derive(Debug, Clone)]
pub struct Deduplication {
    /// The `MessageId` of the message the token first admitted.
    pub message_id: String,
    /// The sequence number that message was assigned.
    pub sequence_number: String,
    /// When the token was accepted; the interval runs from here.
    pub accepted_at: SystemTime,
}

/// One message a receive handed over under a `ReceiveRequestAttemptId`, as the retry
/// carrying that token has to see it again.
#[derive(Debug, Clone)]
pub struct DeliveredMessage {
    /// The `MessageId` handed over.
    pub message_id: String,
    /// The receipt handle minted for it: "subsequent calls with the same
    /// ReceiveRequestAttemptId return the same messages and receipt handles".
    pub receipt_handle: String,
    /// The instant that receive made the message invisible until. The clock may lapse
    /// it, but only a caller can move it elsewhere, which is what makes a visibility
    /// change recognisable — see [`Queue::receive_attempt_was_modified`].
    pub invisible_until: SystemTime,
}

/// The batch one `ReceiveRequestAttemptId` handed over, so a retry carrying that token
/// "retrieve[s] the same set of messages, even if their visibility timeout has not yet
/// expired".
#[derive(Debug, Clone)]
pub struct ReceiveAttempt {
    /// The messages handed over, in the order they were handed over.
    pub delivered: Vec<DeliveredMessage>,
    /// When the receive was served; [`RECEIVE_REQUEST_ATTEMPT_INTERVAL`] runs from here.
    pub served_at: SystemTime,
}

/// Per-service state for com-amazonaws-sqs-amazonsqs. A handler reaches it through [`service_state`].
///
/// Every resource here belongs to ONE region — see "One namespace per region" in this
/// module's documentation — so each map is keyed by that region and every read takes it.
#[derive(Debug, Default)]
pub struct ComAmazonawsSqsAmazonsqsState {
    /// Every queue that exists, keyed by the `(region, QueueName)` pair that identifies
    /// one: a name is unique within one region's queues, so the same name in a second
    /// region is a second queue and both coexist here. Reached through
    /// [`Self::queue`] / [`Self::queue_mut`] / [`Self::queues_in`] rather than by
    /// indexing, so no caller can read one region's queue for another's.
    ///
    /// A `BTreeMap` rather than a `HashMap` so iteration is in region-and-name order:
    /// the order a listing pages over has to be the same on every call.
    pub queues: BTreeMap<(String, String), Queue>,
    /// The message movement tasks `StartMessageMoveTask` has started, oldest first.
    /// Bounded per source queue by [`MAX_MESSAGE_MOVE_TASKS_PER_QUEUE`].
    pub message_move_tasks: Vec<MessageMoveTask>,
    /// The queues `DeleteQueue` has removed, each keyed by the `(region, QueueName)` pair
    /// it was held under and carrying the instant it was removed: a name stays unavailable
    /// to `CreateQueue` in THAT region for [`QUEUE_DELETED_RECENTLY_INTERVAL`] after its
    /// queue was deleted. Entries lapse with that interval and are bounded by
    /// [`MAX_RECENTLY_DELETED_NAMES`].
    pub recently_deleted: BTreeMap<(String, String), SystemTime>,
    /// When each queue was last READ for its gauges, keyed by the same `(region, QueueName)`
    /// pair the queue itself is held under: one reading per queue per minute, which is what
    /// decides whether it owes another ([`queue_depth_metrics::publish_queue_metrics`]). An
    /// entry goes with its queue, so this map never holds more than [`queues`](Self::queues)
    /// does.
    pub last_sampled_at: BTreeMap<(String, String), SystemTime>,
    /// Until when the one queue-depth clock running over this store holds its claim, or `None`
    /// when no clock holds one — see [`queue_depth_metrics::drive_queue_metric_clock`].
    pub depth_clock_until: Option<SystemTime>,
    /// The task of that clock, so it can be stopped: a clock that outlives its claim is
    /// aborted when the next one takes it over.
    pub depth_clock: Option<::tokio::task::JoinHandle<()>>,
}

/// A stored queue: the state the reported `Attributes` map is rebuilt from, plus the
/// messages it holds.
#[derive(Debug, Clone)]
pub struct Queue {
    pub name: String,
    /// The region the `CreateQueue` call that made this queue addressed, as its SigV4
    /// credential scope named it: the region whose namespace holds the queue, the one its
    /// [`arn`](Self::arn) carries, and the only one a later call can reach it through.
    pub region: String,
    /// What identifies THIS incarnation of the queue: minted when the queue was created,
    /// so a queue created again under a name a delete removed is a different incarnation
    /// of that name. Every receipt handle this queue mints carries it
    /// ([`mint_receipt_handle`](Self::mint_receipt_handle)), which is what lets a handle
    /// be traced back to the queue that issued it after the queue is gone.
    pub incarnation: String,
    /// The url minted when the queue was created, and the one every later reference
    /// to this queue reports.
    pub url: String,
    pub arn: String,
    /// The configuration attributes the queue carries: what the create supplied,
    /// overlaid on the documented defaults, plus whatever a later
    /// `SetQueueAttributes` changed. Name-ordered so the reported map is stable.
    pub attributes: BTreeMap<String, String>,
    /// The queue's cost allocation tags: what the create carried, plus every tag a
    /// later `TagQueue` added or overwrote, bounded by [`MAX_TAGS_PER_QUEUE`].
    /// Key-ordered so the map `ListQueueTags` reports is stable, and case-sensitive
    /// (keys differing only in case are different tags), as the `TagQueue`
    /// documentation states.
    pub tags: BTreeMap<String, String>,
    /// The messages this queue holds, in arrival order. Bounded: at most
    /// [`MAX_MESSAGES_PER_QUEUE`] messages and [`MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE`]
    /// bytes, with the store-wide [`MAX_RETAINED_MESSAGE_BYTES`] budget bounding every
    /// queue together.
    pub messages: VecDeque<StoredMessage>,
    /// Receipt handle -> message id, written only through
    /// [`record_receipt_handle`](Self::record_receipt_handle). At most ONE live handle
    /// per message: a later receive replaces the handle an earlier one minted, and a
    /// handle is dropped with its message — so this map never holds more entries than
    /// [`messages`](Self::messages), whatever the receive count.
    pub handles: HashMap<String, String>,
    /// The receipt handles this queue minted and a delete has since CONSUMED, in the
    /// order they were retired. A delete is idempotent — "the request succeeds, but the
    /// message might not be deleted" — so a handle whose message a delete already removed
    /// still names a deletion this queue has performed, which is what makes a repeat of
    /// that delete succeed instead of being refused. Bounded by
    /// [`MAX_DELETED_HANDLES_PER_QUEUE`]; a purge empties it with the message plane.
    pub deleted_handles: VecDeque<String>,
    /// The receipt handles this queue minted that a LATER receive of the same message
    /// superseded, as `(receipt handle, message id)` in the order they were superseded —
    /// "if you receive a message more than once, each time you receive it, you get a
    /// different receipt handle". The message is still here, under the newer handle, so
    /// what a caller presenting the older one holds is a lease that has run out rather
    /// than a value that names nothing ([`ReceiptHandleRefusal::Expired`]). An entry goes
    /// when its message does, and the whole deque is bounded by
    /// [`MAX_SUPERSEDED_HANDLES_PER_QUEUE`]: past the bound the handles superseded
    /// earliest are forgotten first.
    pub superseded_handles: VecDeque<(String, String)>,
    /// The deduplication tokens this queue has accepted, keyed by
    /// `(deduplication scope, token)` — see [`Queue::deduplication_key`]. Only a FIFO
    /// queue writes here; entries lapse with [`DEDUPLICATION_INTERVAL`] and are bounded
    /// by [`MAX_DEDUPLICATION_TOKENS_PER_QUEUE`].
    pub deduplication: BTreeMap<(String, String), Deduplication>,
    /// The batch each `ReceiveRequestAttemptId` this queue has served handed over, keyed
    /// by that token. Only a FIFO receive writes here, and only when it handed something
    /// over; entries lapse with [`RECEIVE_REQUEST_ATTEMPT_INTERVAL`] and are bounded by
    /// [`MAX_RECEIVE_ATTEMPTS_PER_QUEUE`].
    pub receive_attempts: BTreeMap<String, ReceiveAttempt>,
    /// How many messages this queue has sequenced. The sequence number of the next one
    /// is derived from it, so "SequenceNumber continues to increase" across the queue
    /// and therefore across each of its message groups.
    pub sequenced: u128,
    pub created_at: SystemTime,
    /// When an attribute of this queue last changed; equals `created_at` until a
    /// `SetQueueAttributes` changes something.
    pub last_modified_at: SystemTime,
    /// When this queue last had a `PurgeQueue` accepted, if it ever has: a further
    /// purge inside [`PURGE_QUEUE_INTERVAL`] of that instant is what
    /// `PurgeQueueInProgress` reports.
    pub purged_at: Option<SystemTime>,
    /// The message group this queue last handed a message over FROM, on a FIFO queue:
    /// where the next receive's walk over the eligible groups starts (see
    /// [`Queue::delivery_order`]). `None` until a receive has handed anything over.
    pub group_cursor: Option<String>,
    /// What this queue's data-plane calls have contributed to the `AWS/SQS` counters and
    /// this service has not published yet — see [`Queue::count_traffic`]. Drained by the
    /// seam every request passes through, and bounded there.
    pub counted_traffic: Vec<CountedTraffic>,
}

impl Queue {
    /// A queue as a create makes one: held in `region`'s namespace under `name`, carrying
    /// `attributes` and `tags`, with nothing in its message plane yet.
    ///
    /// The one place a queue record comes into being, so every API that provisions one —
    /// `CreateQueue`, and the Cloud Control API resource handler that stands behind
    /// `AWS::SQS::Queue` — makes the SAME record: the same url and ARN for the name, a
    /// fresh incarnation, and a queue nothing has sent to, purged or received from.
    pub fn created(
        region: &str,
        name: &str,
        attributes: BTreeMap<String, String>,
        tags: BTreeMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            region: region.to_string(),
            // A create makes a NEW incarnation of the name, whether or not a queue has
            // carried it before: the handles the previous one minted name that one, so
            // presenting one to this queue is presenting a handle of a queue that is gone.
            incarnation: new_identifier(),
            url: queue_url(region, name),
            arn: queue_arn(region, name),
            attributes,
            tags,
            messages: VecDeque::new(),
            handles: HashMap::new(),
            // A queue nobody has deleted a message from yet has retired no receipt handle.
            deleted_handles: VecDeque::new(),
            // Nor has any receive of it been superseded by a later one.
            superseded_handles: VecDeque::new(),
            deduplication: BTreeMap::new(),
            receive_attempts: BTreeMap::new(),
            // Nothing has been sequenced yet, so the first message this queue sequences
            // gets the first sequence number.
            sequenced: 0,
            created_at: now,
            // Nothing has changed the queue yet, so it was last modified when it was
            // created.
            last_modified_at: now,
            // A queue nobody has purged holds no purge instant, so the first `PurgeQueue`
            // on it is accepted.
            purged_at: None,
            // No receive has handed anything over yet, so the first FIFO receive starts
            // its walk at the oldest message group.
            group_cursor: None,
            // Nothing has been sent to, received from or deleted out of this queue yet, so
            // its counters have nothing to publish.
            counted_traffic: Vec::new(),
        }
    }

    /// Every attribute this queue has a value for: its stored configuration, the three
    /// the record itself derives (`QueueArn`, `CreatedTimestamp`,
    /// `LastModifiedTimestamp`), the three that count the messages it holds and the
    /// encryption the queue is under ([`encryption::report`], since a queue is always
    /// on one of the two server-side encryption options whether or not its create named
    /// one).
    ///
    /// The two timestamps are reported "in seconds (epoch time)", per their own
    /// entries in the `AttributeNames` documentation.
    pub fn reported_attributes(&self, now: SystemTime) -> BTreeMap<String, String> {
        let mut reported = self.attributes.clone();
        encryption::report(&self.attributes, &mut reported);
        reported.insert("QueueArn".to_string(), self.arn.clone());
        reported.insert(
            "CreatedTimestamp".to_string(),
            epoch_seconds(self.created_at),
        );
        reported.insert(
            "LastModifiedTimestamp".to_string(),
            epoch_seconds(self.last_modified_at),
        );
        let (visible, in_flight, delayed) = self.counts(now);
        reported.insert(
            "ApproximateNumberOfMessages".to_string(),
            visible.to_string(),
        );
        reported.insert(
            "ApproximateNumberOfMessagesNotVisible".to_string(),
            in_flight.to_string(),
        );
        reported.insert(
            "ApproximateNumberOfMessagesDelayed".to_string(),
            delayed.to_string(),
        );
        reported
    }

    /// Whether this queue is a FIFO queue, which is what makes the FIFO-only
    /// attributes applicable to it.
    pub fn is_fifo(&self) -> bool {
        self.boolean(FIFO_QUEUE_ATTRIBUTE)
    }

    /// This queue's access policy — the document its `Policy` attribute carries — or
    /// `None` when it carries none, which is a queue no resource-based statement is in
    /// force for.
    pub fn policy(&self) -> Option<&str> {
        self.attributes.get(POLICY).map(String::as_str)
    }

    /// Where this queue's messages are dead-lettered and after how many deliveries: the
    /// `QueueName` its `RedrivePolicy` names in `deadLetterTargetArn` and that policy's
    /// `maxReceiveCount`, or `None` for a queue carrying no such policy.
    ///
    /// The count is "the number of times a message is delivered to the source queue
    /// before being moved to the dead-letter queue", so a policy that names a target but
    /// no count configures no dead-lettering: there is no number of deliveries to pass.
    /// The policy is parsed once for both members, because a receive asks this of its
    /// queue on every poll.
    ///
    /// The target ARN is read in THIS queue's own region, exactly as
    /// [`ComAmazonawsSqsAmazonsqsState::dead_letter_source_queue_names`] reads it: a
    /// redrive link never crosses a region, so an ARN naming another region's queue names
    /// no dead-letter target of this one.
    pub fn dead_letter_target(&self) -> Option<(String, u32)> {
        let policy = parsed_redrive_policy(self.attributes.get(REDRIVE_POLICY)?)?;
        let target = queue_name_from_arn(redrive_target_arn(&policy)?, &self.region)?.to_string();
        Some((target, redrive_max_receive_count(&policy)?))
    }

    /// Whether this queue, AS A DEAD-LETTER TARGET, admits the queue whose ARN is
    /// `source_arn` as one of its dead-letter source queues.
    ///
    /// The decision is this queue's own `RedriveAllowPolicy`, whose `redrivePermission`
    /// "defines which source queues can specify the current queue as the dead-letter
    /// queue": `allowAll` — the documented DEFAULT, which is what a queue carrying no such
    /// policy is on — admits every source queue of the account and region, `denyAll` admits
    /// none, and `byQueue` admits "only queues specified by the `sourceQueueArns`
    /// parameter". A value that is not a JSON object, or that names a permission outside
    /// that published set, restricts nothing: it leaves the queue on the documented
    /// default rather than inventing a fourth permission type.
    ///
    /// Every path that would make this queue the dead-letter target of another asks this:
    /// the association a `RedrivePolicy` write creates, and each automatic redrive of a
    /// message out of a queue already associated — the permission is the dead-letter
    /// queue's current one, so one tightened after the wiring was made governs the next
    /// move as well.
    pub fn admits_dead_letter_source(&self, source_arn: &str) -> bool {
        let Some(policy) = self.attributes.get(REDRIVE_ALLOW_POLICY) else {
            return true;
        };
        let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
            return true;
        };
        match document
            .get(REDRIVE_PERMISSION_MEMBER)
            .and_then(::serde_json::Value::as_str)
            .map(str::trim)
        {
            Some(REDRIVE_PERMISSION_DENY_ALL) => false,
            Some(REDRIVE_PERMISSION_BY_QUEUE) => document
                .get(SOURCE_QUEUE_ARNS_MEMBER)
                .and_then(::serde_json::Value::as_array)
                .is_some_and(|listed| {
                    listed
                        .iter()
                        .filter_map(::serde_json::Value::as_str)
                        .any(|arn| arn.trim() == source_arn)
                }),
            _ => true,
        }
    }

    /// The customer master key this queue's messages are encrypted under, or `None` for
    /// a queue on the Amazon SQS owned key — see
    /// [`encryption::configured_master_key`]. A call that has to use the key asks
    /// [`encryption::authorize_master_key`] about it first.
    pub fn master_key_id(&self) -> Option<&str> {
        encryption::configured_master_key(&self.attributes)
    }

    /// Whether boolean attribute `name` is set on this queue. "Valid values are true
    /// and false", so anything else — including no value at all — is false.
    pub fn boolean(&self, name: &str) -> bool {
        self.attributes
            .get(name)
            .is_some_and(|value| value == "true")
    }

    /// The configured value of integer attribute `name`, falling back to the
    /// documented default when the queue carries no value (or an unparseable one).
    pub fn integer(&self, name: &str) -> i64 {
        let default = settable_attribute(name)
            .and_then(|spec| spec.default)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        self.attributes
            .get(name)
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .unwrap_or(default)
    }

    /// (visible, in flight, delayed) message counts at `now`.
    pub fn counts(&self, now: SystemTime) -> (usize, usize, usize) {
        let mut counts = (0usize, 0usize, 0usize);
        for message in &self.messages {
            match message.visibility(now) {
                Visibility::Visible => counts.0 += 1,
                Visibility::InFlight => counts.1 += 1,
                Visibility::Delayed => counts.2 += 1,
            }
        }
        counts
    }

    /// Whether this queue derives a message's deduplication token from its body rather
    /// than requiring the sender to supply one.
    pub fn content_based_deduplication(&self) -> bool {
        self.boolean(CONTENT_BASED_DEDUPLICATION)
    }

    /// The key the deduplication token `token` of a message in group `group` is
    /// remembered under.
    ///
    /// `DeduplicationScope` "specifies whether message deduplication occurs at the
    /// message group or queue level": under `messageGroup` the same token in two groups
    /// is two tokens, so the group is part of the key; at queue level — the default —
    /// the token alone is the key, whatever group it arrived in.
    pub fn deduplication_key(&self, group: &str, token: &str) -> (String, String) {
        let per_group = self
            .attributes
            .get(DEDUPLICATION_SCOPE)
            .is_some_and(|value| value == DEDUPLICATION_SCOPE_MESSAGE_GROUP);
        let scope = if per_group {
            group.to_string()
        } else {
            String::new()
        };
        (scope, token.to_string())
    }

    /// Remember that `key` admitted the message `record` names, forgetting the oldest
    /// tokens if that takes the queue past
    /// [`MAX_DEDUPLICATION_TOKENS_PER_QUEUE`](crate::state::MAX_DEDUPLICATION_TOKENS_PER_QUEUE).
    pub fn record_deduplication(&mut self, key: (String, String), record: Deduplication) {
        self.deduplication.insert(key, record);
        while self.deduplication.len() > MAX_DEDUPLICATION_TOKENS_PER_QUEUE {
            let Some(oldest) = self
                .deduplication
                .iter()
                .min_by_key(|(_, remembered)| remembered.accepted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.deduplication.remove(&oldest);
        }
    }

    /// Remember that the receive served under `attempt_id` handed `attempt` over,
    /// forgetting the oldest attempts if that takes the queue past
    /// [`MAX_RECEIVE_ATTEMPTS_PER_QUEUE`](crate::state::MAX_RECEIVE_ATTEMPTS_PER_QUEUE).
    pub fn record_receive_attempt(&mut self, attempt_id: String, attempt: ReceiveAttempt) {
        self.receive_attempts.insert(attempt_id, attempt);
        while self.receive_attempts.len() > MAX_RECEIVE_ATTEMPTS_PER_QUEUE {
            let Some(oldest) = self
                .receive_attempts
                .iter()
                .min_by_key(|(_, remembered)| remembered.served_at)
                .map(|(attempt_id, _)| attempt_id.clone())
            else {
                break;
            };
            self.receive_attempts.remove(&oldest);
        }
    }

    /// Whether the batch `attempt` handed over has been modified since, so it can no
    /// longer be handed over again.
    ///
    /// A retry works "if none of the messages have been modified (deleted or had their
    /// visibility changes)": a message the queue no longer holds was deleted, and one
    /// whose receipt handle or invisible-until instant is no longer the one the receive
    /// left had its visibility changed. An instant the CLOCK lapsed is not a
    /// modification — nobody touched the message, so the retry still owes the same batch.
    pub fn receive_attempt_was_modified(&self, attempt: &ReceiveAttempt) -> bool {
        attempt.delivered.iter().any(|delivered| {
            let held = self
                .messages
                .iter()
                .find(|message| message.message_id == delivered.message_id);
            let Some(message) = held else {
                return true;
            };
            self.handles.get(&delivered.receipt_handle) != Some(&delivered.message_id)
                || message
                    .invisible_until
                    .is_some_and(|until| until != delivered.invisible_until)
        })
    }

    /// Mint the next sequence number for this queue: a decimal integer inside 128 bits
    /// that is larger than every one it has already assigned.
    pub fn next_sequence_number(&mut self) -> String {
        self.sequenced = self.sequenced.saturating_add(1);
        SEQUENCE_NUMBER_BASE
            .saturating_add(self.sequenced)
            .to_string()
    }

    /// The indexes, in the order they are to be handed over, of at most `max` messages
    /// this queue may deliver at `now`.
    ///
    /// A standard queue hands over whatever is visible, oldest first. A FIFO queue
    /// hands over each message group's messages "one at a time, in strict order,
    /// ensuring that no two messages from the same group are processed simultaneously":
    /// the oldest message of a group that is not visible — because it is in flight with
    /// another consumer, or because its delay has not elapsed — stops that group, so
    /// nothing behind it is handed over until it is deleted or becomes visible again.
    /// Other groups are unaffected, which is what lets several ordered streams share
    /// one queue.
    ///
    /// Which group a FIFO receive serves FIRST rotates: the walk starts at the group
    /// after the one the last delivery came from ([`group_cursor`](Self::group_cursor)),
    /// so consecutive receives take their first message from different groups and every
    /// eligible group is reached within as many receives as the queue has groups. Order
    /// is defined WITHIN a message group only, so the cross-group interleaving is this
    /// queue's to choose — but a group that is eligible has to be reachable, which serving
    /// the globally oldest visible message every time does not make it: a consumer that
    /// hands each message it is not ready for straight back would be handed the same one
    /// again forever and never see the groups behind it.
    pub fn delivery_order(&self, now: SystemTime, max: usize) -> Vec<usize> {
        if !self.is_fifo() {
            let mut taking: Vec<usize> = Vec::new();
            for (index, message) in self.messages.iter().enumerate() {
                if taking.len() >= max {
                    break;
                }
                if message.visibility(now) == Visibility::Visible {
                    taking.push(index);
                }
            }
            return taking;
        }

        // Each eligible group's deliverable run, in the arrival order of the group's
        // oldest message: the messages from its head on while they are visible, stopping
        // at the first that is not — `(group, the run, whether the group has stopped)`. A
        // group whose head is not visible is blocked and contributes nothing.
        let mut runs: Vec<(&str, Vec<usize>, bool)> = Vec::new();
        for (index, message) in self.messages.iter().enumerate() {
            let group = message.group();
            let visible = message.visibility(now) == Visibility::Visible;
            match runs.iter_mut().find(|(named, _, _)| *named == group) {
                // A group already stopped hands over nothing more, whatever lies behind
                // the message that stopped it.
                Some((_, _, true)) => continue,
                Some((_, run, stopped)) => {
                    if visible {
                        run.push(index);
                    } else {
                        *stopped = true;
                    }
                }
                None => runs.push((group, if visible { vec![index] } else { Vec::new() }, !visible)),
            }
        }
        // Start after the group the last delivery came from, wrapping around: the groups
        // keep their arrival order, only where the walk ENTERS that order rotates.
        let start = self
            .group_cursor
            .as_deref()
            .and_then(|served| runs.iter().position(|(group, _, _)| *group == served))
            .map_or(0, |position| position + 1);
        let mut taking: Vec<usize> = Vec::new();
        for offset in 0..runs.len() {
            if taking.len() >= max {
                break;
            }
            let Some((_, run, _)) = runs.get((start + offset) % runs.len()) else {
                continue;
            };
            for index in run {
                if taking.len() >= max {
                    break;
                }
                taking.push(*index);
            }
        }
        taking
    }

    /// Remember which message group the delivery this receive is making came from, so the
    /// next one starts at the group after it (see [`delivery_order`](Self::delivery_order)).
    ///
    /// Only a FIFO queue rotates over groups; on a standard queue there is no group order
    /// to keep, so nothing is remembered.
    pub fn record_delivered_group(&mut self, group: &str) {
        if self.is_fifo() {
            self.group_cursor = Some(group.to_string());
        }
    }

    /// Hand the next batch this queue may deliver at `now` — at most `max` messages, in
    /// [`delivery_order`](Self::delivery_order) — to a consumer, and answer what was handed
    /// over.
    ///
    /// This is the ONE place a delivery is recorded, for every consumer of a queue: the
    /// caller's own `ReceiveMessage` and a poller another service runs over the same queue
    /// both count a delivery the same way, because `maxReceiveCount` is "the number of
    /// times a message is delivered to the source queue before being moved to the
    /// dead-letter queue" whoever the consumer was. Each message handed over has both its
    /// delivery counts raised, is hidden until `invisible_until` ("The parameter is applied
    /// to the messages that Amazon SQS returns in the response"), carries the instant it
    /// was handed over, and gets a freshly minted receipt handle — "if you receive a
    /// message more than once, each time you receive it, you get a different receipt
    /// handle" — which supersedes the handle an earlier delivery of it minted. On a FIFO
    /// queue the group this batch came from is remembered, so the next delivery starts at
    /// the group after it.
    ///
    /// The messages are left IN the queue: only a delete removes one, and a delivery whose
    /// consumer never deletes it is exactly the case the queue's `RedrivePolicy` bounds. Each
    /// entry answered therefore carries the POSITION in [`messages`](Self::messages) the
    /// message was handed over from, so a caller that has to read the stored message again —
    /// to project the wire message a receive answers with — walks to it instead of searching
    /// the queue for it. The position is this hand-over's: nothing here removes or reorders a
    /// message, so it holds until something else does.
    pub fn hand_over(
        &mut self,
        now: SystemTime,
        max: usize,
        invisible_until: SystemTime,
    ) -> Vec<(usize, DeliveredMessage)> {
        let taking = self.delivery_order(now, max);
        let mut handed: Vec<(usize, DeliveredMessage)> = Vec::with_capacity(taking.len());
        // The message group the last of these deliveries came from, which is where the next
        // delivery's walk over the groups starts.
        let mut served_group: Option<String> = None;
        for index in taking {
            // The handle is minted for a message this queue really holds, so an index it does
            // not hold costs nothing.
            if self.messages.get(index).is_none() {
                continue;
            }
            let receipt_handle = self.mint_receipt_handle();
            let Some(message) = self.messages.get_mut(index) else {
                continue;
            };
            message.receive_count = message.receive_count.saturating_add(1);
            // This queue's own count of the deliveries it has made, which is what its
            // `RedrivePolicy` bounds: this delivery is one of them.
            message.queue_receive_count = message.queue_receive_count.saturating_add(1);
            if message.first_received_at.is_none() {
                message.first_received_at = Some(now);
            }
            // The instant this handle was minted at, which is where the span a later
            // visibility change may still ask for is counted from.
            message.last_received_at = Some(now);
            message.invisible_until = Some(invisible_until);
            served_group = Some(message.group().to_string());
            handed.push((
                index,
                DeliveredMessage {
                    message_id: message.message_id.clone(),
                    receipt_handle,
                    invisible_until,
                },
            ));
        }
        if let Some(group) = served_group {
            self.record_delivered_group(&group);
        }
        for (_, delivered) in &handed {
            // Replaces the handle an earlier delivery minted for this message, so
            // re-receiving cannot accumulate handles.
            self.record_receipt_handle(delivered.receipt_handle.clone(), &delivered.message_id);
        }
        // Every one of them is a message this queue's consumers were handed, which is what the
        // `AWS/SQS` received counter counts. Counted HERE, the one path every delivery takes,
        // so a poller another service runs over this queue is counted like this service's own
        // receive.
        let delivered = handed.len();
        self.count_traffic(QueueTraffic::MessagesReceived, count_as_f64(delivered), now);
        handed
    }

    /// Count, against the messages this queue was about to hand over at `now`, a delivery
    /// it could not make.
    ///
    /// A receive that found messages to hand over and was then refused the data key they
    /// are sealed under made a delivery ATTEMPT: the queue took the messages out to hand
    /// them over and the hand-over failed. `maxReceiveCount` is "the number of times a
    /// message is delivered to the source queue before being moved to the dead-letter
    /// queue", and it counts that attempt — which is why a consumer that cannot decrypt a
    /// queue's messages drives them to the queue's dead-letter target instead of being
    /// handed the same undecryptable message forever.
    ///
    /// Nothing is handed over, so no receipt handle is minted and no message is hidden: a
    /// message no consumer received is in flight with nobody, and the next receive sees it
    /// at once.
    pub fn count_refused_delivery(&mut self, now: SystemTime, max: usize) {
        for index in self.delivery_order(now, max) {
            let Some(message) = self.messages.get_mut(index) else {
                continue;
            };
            message.receive_count = message.receive_count.saturating_add(1);
            message.queue_receive_count = message.queue_receive_count.saturating_add(1);
        }
    }

    /// Materialise everything the CLOCK already made due on this queue: drop messages
    /// whose retention period has elapsed, return messages whose visibility timeout has
    /// lapsed to the visible set, and forget deduplication tokens whose interval has
    /// run out. All three were owed at `now` whether or not anybody called, which is
    /// why a reporting operation may apply them.
    pub fn materialise_due(&mut self, now: SystemTime) {
        let retention = seconds(self.integer(MESSAGE_RETENTION_PERIOD));
        let mut expired: Vec<String> = Vec::new();
        self.messages.retain(|message| {
            let due = lapsed(message.retention_from, retention, now);
            if due {
                expired.push(message.message_id.clone());
            }
            !due
        });
        if !expired.is_empty() {
            self.handles.retain(|_, id| !expired.contains(id));
            self.forget_superseded_handles(&expired);
        }
        for message in &mut self.messages {
            if message.invisible_until.is_some_and(|until| now >= until) {
                message.invisible_until = None;
            }
        }
        // A token suppresses a resend only "during the 5-minute deduplication
        // interval"; past it, "Amazon SQS can't detect duplicate messages".
        self.deduplication.retain(|_, remembered| {
            !lapsed(remembered.accepted_at, DEDUPLICATION_INTERVAL, now)
        });
        // "You can use ReceiveRequestAttemptId only for 5 minutes after a ReceiveMessage
        // action", so past that a token names no batch and a receive carrying it is a
        // fresh one.
        self.receive_attempts.retain(|_, remembered| {
            remembered
                .served_at
                .checked_add(RECEIVE_REQUEST_ATTEMPT_INTERVAL)
                .is_none_or(|lapses| now < lapses)
        });
    }

    /// Re-derive, from this queue's CURRENT `DelaySeconds`, when each message it
    /// already holds becomes available — which is what a `DelaySeconds` change does to
    /// a FIFO queue and does not do to a standard one.
    ///
    /// "For standard queues, the per-queue delay setting is not retroactive — changing
    /// the setting doesn't affect the delay of messages already in the queue. For FIFO
    /// queues, the per-queue delay setting is retroactive — changing the setting affects
    /// the delay of messages already in the queue" (the `DelaySeconds` attribute's own
    /// documentation). A FIFO send carries no per-message delay either ("you can't set
    /// DelaySeconds per message ... only on a queue level"), so the queue's value is the
    /// only delay a stored FIFO message has and re-deriving it from the instant the
    /// message was sent loses nothing.
    ///
    /// A delay governs a message's FIRST delivery, so a message already handed to a
    /// consumer keeps the availability it was delivered under.
    pub fn reapply_delay_seconds(&mut self) {
        if !self.is_fifo() {
            return;
        }
        let delay = self.integer(DELAY_SECONDS);
        for message in &mut self.messages {
            if message.first_received_at.is_some() {
                continue;
            }
            message.available_at = after(message.sent_at, delay);
        }
    }

    /// Mint a receipt handle for a receive of one of this queue's messages: one opaque
    /// token, encoding this queue's incarnation, a fresh token identifying the receive and
    /// the queue's ARN, so the handle names the queue that issued it.
    ///
    /// A handle is "associated with a specific instance of receiving a message" and Amazon
    /// SQS hands it over as a single long encoded token, so what this service puts in a
    /// handle travels INSIDE the encoding ([`encoded_opaque_token`]) rather than in the
    /// token's own text: a consumer reads the value as opaque, which is what it is.
    ///
    /// The handle a consumer holds therefore carries its own provenance: a later call that
    /// presents it to another queue, or to a queue created again under the same name, is
    /// answered for what the handle IS rather than for what some live table happens to
    /// hold ([`receipt_handle_incarnation`]).
    pub fn mint_receipt_handle(&self) -> String {
        encoded_opaque_token(&format!(
            "{}{RECEIPT_HANDLE_FIELD_SEPARATOR}{}{RECEIPT_HANDLE_FIELD_SEPARATOR}{}",
            self.incarnation,
            new_identifier(),
            self.arn,
        ))
    }

    /// Record the receipt handle a receive just minted for `message_id`, remembering the
    /// handle an earlier receive minted for it as superseded.
    ///
    /// Amazon SQS documents exactly this behaviour: "If you receive a message more than
    /// once, each time you receive it, you get a different receipt handle. You must
    /// provide the most recently received receipt handle when you request to delete the
    /// message." Holding one live handle per message rather than accumulating them is also
    /// what bounds [`handles`](Self::handles) by the message count.
    pub fn record_receipt_handle(&mut self, receipt_handle: String, message_id: &str) {
        let superseded: Vec<String> = self
            .handles
            .iter()
            .filter(|(handle, id)| id.as_str() == message_id && *handle != &receipt_handle)
            .map(|(handle, _)| handle.clone())
            .collect();
        self.handles.retain(|_, id| id != message_id);
        for handle in superseded {
            self.remember_superseded_handle(handle, message_id);
        }
        self.handles.insert(receipt_handle, message_id.to_string());
    }

    /// Remember `receipt_handle` as one a later receive of `message_id` replaced, bounded
    /// by [`MAX_SUPERSEDED_HANDLES_PER_QUEUE`].
    fn remember_superseded_handle(&mut self, receipt_handle: String, message_id: &str) {
        if self
            .superseded_handles
            .iter()
            .any(|(handle, _)| *handle == receipt_handle)
        {
            return;
        }
        self.superseded_handles
            .push_back((receipt_handle, message_id.to_string()));
        while self.superseded_handles.len() > MAX_SUPERSEDED_HANDLES_PER_QUEUE {
            self.superseded_handles.pop_front();
        }
    }

    /// The message `receipt_handle` was minted for, where a LATER receive of that message
    /// has since replaced it — or `None` for any other value.
    ///
    /// An entry is dropped with its message, so one that is here names a message this
    /// queue still holds.
    fn superseded_message_id(&self, receipt_handle: &str) -> Option<&str> {
        self.superseded_handles
            .iter()
            .find(|(handle, _)| handle == receipt_handle)
            .map(|(_, message_id)| message_id.as_str())
    }

    /// Whether `receipt_handle` is one this queue minted that a later receive superseded
    /// and that no longer acts on its message: the handle's lease is over, the message is
    /// not. Only a FIFO queue has such handles — see
    /// [`message_id_for_handle`](Self::message_id_for_handle).
    pub fn expired_handle(&self, receipt_handle: &str) -> bool {
        self.is_fifo() && self.superseded_message_id(receipt_handle).is_some()
    }

    /// Forget every superseded handle of the messages `message_ids` names: a handle lives
    /// no longer than the message it was minted for.
    fn forget_superseded_handles(&mut self, message_ids: &[String]) {
        self.superseded_handles
            .retain(|(_, id)| !message_ids.contains(id));
    }

    /// The id of the message `receipt_handle` names, or why it names none.
    ///
    /// A handle this queue holds today names its message, and so does one a later receive
    /// of that message superseded: "the receipt handle is associated with a specific
    /// instance of receiving a message" and the message is still here, so a consumer that
    /// kept the handle it was first handed still acts on it.
    ///
    /// A FIFO queue is the exception. There a message is "processed one at a time, in
    /// strict order", so the receive that takes a message takes the LEASE on it with it:
    /// once a later receive has handed the message to a consumer, the handle the earlier
    /// one minted no longer acts on it and is [`Expired`](ReceiptHandleRefusal::Expired) —
    /// otherwise two consumers of one message group could both act on the one message
    /// either of them is meant to be processing alone.
    ///
    /// A handle whose message has been deleted, purged or expired names nothing, and a
    /// value that is not formed as a handle at all names nothing either — which is the
    /// distinction a caller recovers from differently.
    pub fn message_id_for_handle(
        &self,
        receipt_handle: &str,
    ) -> Result<String, ReceiptHandleRefusal> {
        if let Some(message_id) = self.handles.get(receipt_handle) {
            return Ok(message_id.clone());
        }
        if let Some(message_id) = self.superseded_message_id(receipt_handle) {
            if self.is_fifo() {
                return Err(ReceiptHandleRefusal::Expired);
            }
            return Ok(message_id.to_string());
        }
        Err(if is_receipt_handle(receipt_handle) {
            ReceiptHandleRefusal::NoSuchMessage
        } else {
            ReceiptHandleRefusal::Malformed
        })
    }

    /// The message `receipt_handle` names, to be changed in place, or why it names none.
    pub fn message_for_handle_mut(
        &mut self,
        receipt_handle: &str,
    ) -> Result<&mut StoredMessage, ReceiptHandleRefusal> {
        let message_id = self.message_id_for_handle(receipt_handle)?;
        self.messages
            .iter_mut()
            .find(|message| message.message_id == message_id)
            // A handle is dropped with its message, so a handle this queue holds always
            // names a message it holds; this arm is the same answer either way.
            .ok_or(ReceiptHandleRefusal::NoSuchMessage)
    }

    /// Discard everything this queue holds, and remember that it was purged at `at`.
    ///
    /// `PurgeQueue` "deletes available messages in a queue (including in-flight
    /// messages)", so no message survives whatever its visibility, and every receipt
    /// handle an earlier receive minted names a message that is gone — as does every
    /// batch a `ReceiveRequestAttemptId` remembers, so neither can be replayed against
    /// messages the purge discarded. The deduplication tokens go with them: a token's
    /// only effect is suppressing a resend of a message this queue holds, and "you
    /// can't retrieve any messages deleted from a queue", so a resend after a purge is a
    /// message the queue never accepted and is delivered rather than suppressed.
    ///
    /// The queue's configuration — its attributes, its tags and the sequence counter
    /// whose numbers "continue to increase" — is untouched: a purge empties a queue, it
    /// does not re-create it.
    pub fn purge(&mut self, at: SystemTime) {
        self.messages.clear();
        self.handles.clear();
        self.deleted_handles.clear();
        self.superseded_handles.clear();
        self.receive_attempts.clear();
        self.deduplication.clear();
        self.purged_at = Some(at);
    }

    /// Whether a `PurgeQueue` on this queue is refused at `now` because the purge it
    /// accepted inside [`PURGE_QUEUE_INTERVAL`] is still deleting.
    pub fn purge_in_progress(&self, now: SystemTime) -> bool {
        self.purged_at
            .is_some_and(|at| !lapsed(at, PURGE_QUEUE_INTERVAL, now))
    }

    /// Drop a message and every receipt handle that referred to it — the live one and
    /// every one a later receive superseded — remembering those handles as handles this
    /// queue has already deleted under, which is what makes a repeat of the same delete
    /// idempotent whichever of them it carries.
    pub fn remove_message(&mut self, message_id: &str) {
        self.messages
            .retain(|message| message.message_id != message_id);
        let mut retired: Vec<String> = self
            .handles
            .iter()
            .filter(|(_, id)| id.as_str() == message_id)
            .map(|(handle, _)| handle.clone())
            .collect();
        retired.extend(
            self.superseded_handles
                .iter()
                .filter(|(_, id)| id.as_str() == message_id)
                .map(|(handle, _)| handle.clone()),
        );
        self.handles.retain(|_, id| id != message_id);
        self.forget_superseded_handles(&[message_id.to_string()]);
        for handle in retired {
            self.remember_deleted_handle(handle);
        }
    }

    /// Remember `receipt_handle` as one whose message is gone, bounded by
    /// [`MAX_DELETED_HANDLES_PER_QUEUE`]: past the bound the handles retired earliest are
    /// forgotten first.
    fn remember_deleted_handle(&mut self, receipt_handle: String) {
        if self.deleted_handle(&receipt_handle) {
            return;
        }
        self.deleted_handles.push_back(receipt_handle);
        while self.deleted_handles.len() > MAX_DELETED_HANDLES_PER_QUEUE {
            self.deleted_handles.pop_front();
        }
    }

    /// Whether `receipt_handle` is one this queue minted and a delete has already
    /// consumed: it names a message this queue no longer holds, so there is nothing left
    /// for a further delete to do.
    pub fn deleted_handle(&self, receipt_handle: &str) -> bool {
        self.deleted_handles
            .iter()
            .any(|handle| handle == receipt_handle)
    }

    /// Give the message a receipt handle names a new visibility timeout, counted from `now`:
    /// "if you send a ChangeMessageVisibility call with VisibilityTimeout set to 10 seconds,
    /// the 10 seconds begin to count from the time that you make the
    /// ChangeMessageVisibility call."
    ///
    /// `Err` is why the change was refused, which each handler renders as its own modelled
    /// error (or, in a batch, as that entry's own result). The handle decides the first two:
    /// a value that is not a receipt handle at all is a different fault from one that is,
    /// but names no message this queue still holds — see
    /// [`message_for_handle_mut`](Self::message_for_handle_mut). That is the whole of the
    /// resolution: a handle names its message for as long as the queue HOLDS that message,
    /// not only while the message is hidden, so a consumer whose visibility window has
    /// already lapsed re-hides the message it is still working on rather than being
    /// refused. Past the handle, the span is bounded by what ONE receive may hide a message
    /// for, since "if you attempt to set the VisibilityTimeout to a value greater than the
    /// maximum time left, Amazon SQS returns an error. Amazon SQS doesn't automatically
    /// recalculate and increase the timeout to the maximum remaining time."
    pub fn change_visibility(
        &mut self,
        receipt_handle: &str,
        timeout: i64,
        now: SystemTime,
    ) -> Result<(), VisibilityRefusal> {
        let message = self
            .message_for_handle_mut(receipt_handle)
            .map_err(|refusal| match refusal {
                ReceiptHandleRefusal::Malformed => VisibilityRefusal::MalformedHandle,
                ReceiptHandleRefusal::NoSuchMessage => VisibilityRefusal::NoSuchMessage,
                ReceiptHandleRefusal::Expired => VisibilityRefusal::ExpiredHandle,
            })?;
        // The ceiling of the span one receive may hide a message for is the ceiling of the
        // range the timeout itself takes, and it is counted from the receipt handle's own
        // receive — the LAST one, since that is the receive whose handle this call carries
        // and an earlier handle names no message any more, so counting from the first
        // receive would charge this consumer for a delivery it does not hold. Counted in
        // whole seconds, the unit the timeout is in.
        let (_, limit) = integer_bounds(VISIBILITY_TIMEOUT);
        let held = message.last_received_at.map_or(0, |received_at| {
            now.duration_since(received_at)
                .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        });
        if held.saturating_add(timeout) > limit {
            return Err(VisibilityRefusal::BeyondLimit);
        }
        // A timeout of zero makes the message visible at once, which is how a consumer hands
        // one back; anything else holds it for that long from now.
        message.invisible_until = Some(after(now, timeout));
        Ok(())
    }

    /// Total bytes this queue holds, counting everything each message retains.
    pub fn retained_message_bytes(&self) -> usize {
        self.messages
            .iter()
            .map(StoredMessage::retained_bytes)
            .sum()
    }

    /// Why this queue cannot take one more message retaining `message_bytes`, given the
    /// store already holds `store_bytes` — or `None` when it can.
    ///
    /// The bounds are refused rather than enforced by eviction: silently dropping the
    /// oldest message would lose a message a client was told had been accepted.
    pub fn message_rejection(&self, message_bytes: usize, store_bytes: usize) -> Option<String> {
        self.batch_rejection(1, message_bytes, store_bytes)
    }

    /// Why this queue cannot take `count` more messages retaining `message_bytes`
    /// together, given the store already holds `store_bytes` — or `None` when it can.
    ///
    /// The whole batch is judged at once, so a call that adds several messages (a
    /// batch send, or the messages a message movement task hands over) is either
    /// accepted entirely or refused entirely, never half-applied.
    pub fn batch_rejection(
        &self,
        count: usize,
        message_bytes: usize,
        store_bytes: usize,
    ) -> Option<String> {
        if self.messages.len().saturating_add(count) > MAX_MESSAGES_PER_QUEUE {
            return Some(format!(
                "This emulator holds at most {MAX_MESSAGES_PER_QUEUE} messages per queue; \
                 queue {} cannot take {count} more. Delete messages from it first.",
                self.name
            ));
        }
        if self.retained_message_bytes().saturating_add(message_bytes)
            > MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE
        {
            return Some(format!(
                "This emulator holds at most {MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE} message \
                 bytes per queue; queue {} cannot take {message_bytes} more.",
                self.name
            ));
        }
        if store_bytes.saturating_add(message_bytes) > MAX_RETAINED_MESSAGE_BYTES {
            return Some(format!(
                "This emulator holds at most {MAX_RETAINED_MESSAGE_BYTES} message bytes in \
                 total; it cannot take {message_bytes} more."
            ));
        }
        None
    }

    // -----------------------------------------------------------------------
    // The queue's access policy
    // -----------------------------------------------------------------------

    /// The access policy document this queue carries, as the JSON object a policy is,
    /// with the members a generated one declares filled in where the stored value has
    /// none.
    ///
    /// A queue that carries no `Policy` attribute, or one whose value is not a JSON
    /// object, grants nothing: the document a grant is added to is then the generated
    /// one, whose `Id` is this queue's ARN and the suffix a generated queue policy
    /// carries.
    fn policy_document(&self) -> ::serde_json::Map<String, ::serde_json::Value> {
        let stored = self
            .attributes
            .get(POLICY)
            .and_then(|policy| ::serde_json::from_str::<::serde_json::Value>(policy).ok());
        let mut document = match stored {
            Some(::serde_json::Value::Object(members)) => members,
            _ => ::serde_json::Map::new(),
        };
        document
            .entry(POLICY_VERSION_MEMBER)
            .or_insert_with(|| POLICY_VERSION.into());
        document
            .entry(POLICY_ID_MEMBER)
            .or_insert_with(|| format!("{}{DEFAULT_POLICY_ID_SUFFIX}", self.arn).into());
        document
    }

    /// The statements this queue's policy carries, in policy order.
    ///
    /// `Statement` is "an array of individual statements", and the policy language also
    /// takes a lone statement object, so both forms answer the same list here and
    /// neither is lost by a grant that appends to it.
    fn policy_statements(&self) -> Vec<::serde_json::Value> {
        match self.policy_document().get(POLICY_STATEMENT_MEMBER) {
            Some(::serde_json::Value::Array(statements)) => statements.clone(),
            Some(statement @ ::serde_json::Value::Object(_)) => vec![statement.clone()],
            _ => Vec::new(),
        }
    }

    /// The label of every permission this queue's policy grants, in policy order: a
    /// permission's label is the `Sid` of the statement that carries it, which is what
    /// makes a label the handle `RemovePermission` revokes one by.
    pub fn permission_labels(&self) -> Vec<String> {
        self.policy_statements()
            .iter()
            .filter_map(|statement| {
                statement
                    .get(POLICY_SID_MEMBER)
                    .and_then(::serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
            .collect()
    }

    /// The policy document this queue would carry once `label` granted `actions` on it
    /// to each account in `account_numbers`, or why that grant cannot be served.
    ///
    /// The statement is added to whatever the queue's policy already carries, so a
    /// policy a caller uploaded keeps every statement it had and a second grant does not
    /// displace the first.
    pub fn granted_policy(
        &self,
        label: &str,
        account_numbers: &[String],
        actions: &[String],
    ) -> Result<String, PermissionRefusal> {
        if let Some(reason) = permission_label_refusal(label) {
            return Err(PermissionRefusal::Parameter(reason));
        }
        if let Some(reason) = principals_refusal(account_numbers) {
            return Err(reason);
        }
        if let Some(reason) = actions_refusal(actions) {
            return Err(reason);
        }
        let labels = self.permission_labels();
        if labels.iter().any(|existing| existing == label) {
            return Err(PermissionRefusal::Parameter(invalid_parameter_message(
                label,
                PERMISSION_LABEL_PARAMETER,
                LABEL_ALREADY_EXISTS,
            )));
        }
        let mut statements = self.policy_statements();
        if statements.len() >= MAX_POLICY_STATEMENTS {
            // The count the refusal reports is the one the REQUEST asked for — the
            // statements the policy already holds plus the one this grant would add —
            // measured against the published ceiling.
            return Err(PermissionRefusal::Limit(policy_statement_limit_message(
                statements.len().saturating_add(1),
            )));
        }
        statements.push(permission_statement(
            label,
            account_numbers,
            actions,
            &self.arn,
        ));
        let document = self.policy_with_statements(statements);
        // The policy is stored for the queue's lifetime, so the document a grant leaves
        // behind is held to the ceiling AWS publishes for it — "The policy size is
        // limited to 8,192 bytes" (the `Policy` attribute's own documentation) — which
        // is the same bound an uploaded policy is held to.
        if document.len() > MAX_ATTRIBUTE_VALUE_BYTES {
            return Err(PermissionRefusal::Limit(format!(
                "The policy size is limited to {MAX_ATTRIBUTE_VALUE_BYTES} bytes, and the policy \
                 of queue {} would be {} bytes.",
                self.name,
                document.len()
            )));
        }
        Ok(document)
    }

    /// This queue's policy document carrying `statements`, as the string the `Policy`
    /// attribute holds — or no document at all when nothing is left to grant, which is
    /// what makes a queue whose last permission was revoked carry no policy again.
    fn policy_with_statements(&self, statements: Vec<::serde_json::Value>) -> String {
        let mut document = self.policy_document();
        document.insert(
            POLICY_STATEMENT_MEMBER.to_string(),
            ::serde_json::Value::Array(statements),
        );
        ::serde_json::Value::Object(document).to_string()
    }

    /// Record `policy` as this queue's access policy as of `now`; `None` leaves the
    /// queue carrying no policy at all.
    fn set_policy(&mut self, policy: Option<String>, now: SystemTime) {
        match policy {
            Some(policy) => {
                self.attributes.insert(POLICY.to_string(), policy);
            }
            None => {
                self.attributes.remove(POLICY);
            }
        }
        self.last_modified_at = now;
    }

    /// Grant the permission `policy` describes, as of `now`.
    pub fn grant_permission(&mut self, policy: String, now: SystemTime) {
        self.set_policy(Some(policy), now);
    }

    /// Revoke the permission labelled `label`, as of `now`, answering whether the policy
    /// carried such a permission at all.
    ///
    /// Only the statement that label identifies goes; every other statement stays, and a
    /// policy left with no statement is dropped rather than stored empty, so a queue
    /// whose last permission was revoked reports no `Policy` exactly as one that was
    /// never granted a permission does.
    pub fn revoke_permission(&mut self, label: &str, now: SystemTime) -> bool {
        let statements = self.policy_statements();
        let remaining: Vec<::serde_json::Value> = statements
            .iter()
            .filter(|statement| {
                statement
                    .get(POLICY_SID_MEMBER)
                    .and_then(::serde_json::Value::as_str)
                    != Some(label)
            })
            .cloned()
            .collect();
        if remaining.len() == statements.len() {
            return false;
        }
        let policy = (!remaining.is_empty()).then(|| self.policy_with_statements(remaining));
        self.set_policy(policy, now);
        true
    }
}

impl ComAmazonawsSqsAmazonsqsState {
    /// Total message bytes the whole store holds, across every queue.
    pub fn retained_message_bytes(&self) -> usize {
        self.queues.values().map(Queue::retained_message_bytes).sum()
    }

    /// Materialise everything the CLOCK already made due across every queue, and carry
    /// every message movement task as far as it has made it due; see
    /// [`Queue::materialise_due`] and [`Self::advance_message_move_tasks`].
    pub fn materialise_due(&mut self, now: SystemTime) {
        for queue in self.queues.values_mut() {
            queue.materialise_due(now);
        }
        self.advance_message_move_tasks(now);
    }

    /// The queue called `name` in `region`, for reading: the one record every call signed
    /// for that region resolves, and nothing another region holds.
    pub fn queue(&self, region: &str, name: &str) -> Option<&Queue> {
        self.queues.get(&queue_key(region, name))
    }

    /// The same queue, for writing.
    pub fn queue_mut(&mut self, region: &str, name: &str) -> Option<&mut Queue> {
        self.queues.get_mut(&queue_key(region, name))
    }

    /// Whether `region` holds a queue called `name`.
    pub fn holds_queue(&self, region: &str, name: &str) -> bool {
        self.queues.contains_key(&queue_key(region, name))
    }

    /// Every queue of ONE region, in name order: the set a listing of that region pages
    /// over, since a queue belongs to the region its create addressed and nothing outside
    /// it is addressable.
    pub fn queues_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Queue> {
        self.queues
            .range(queue_key(region, "")..)
            .take_while(move |((held, _), _)| held == region)
            .map(|(_, queue)| queue)
    }

    /// Hold `queue` under the region and name it carries.
    pub fn insert_queue(&mut self, queue: Queue) {
        self.queues
            .insert(queue_key(&queue.region, &queue.name), queue);
    }

    /// The queue a queue ARN names, resolved in the region the ARN itself carries.
    ///
    /// For a reader that holds an ARN and no request region of its own — another service
    /// resolving the queue a configuration of its own names. A request-borne reference is
    /// resolved through [`Self::queue`] / [`Self::queue_by_url`] in the region the request
    /// addressed instead.
    pub fn queue_by_arn(&self, arn: &str) -> Option<&Queue> {
        self.queues.get(&queue_key_from_arn(arn)?)
    }

    /// The same queue, for writing: the one reader that CHANGES a queue it holds only an
    /// ARN for (another service's tagging API) resolves it the same way.
    pub fn queue_by_arn_mut(&mut self, arn: &str) -> Option<&mut Queue> {
        let key = queue_key_from_arn(arn)?;
        self.queues.get_mut(&key)
    }

    /// The queue a `QueueUrl` addresses in `region`, for reading.
    ///
    /// The url names the account and the queue ([`queue_name_from_url`]); which region's
    /// queue it resolves is the region the REQUEST addressed, exactly as the real service
    /// serves a url against the endpoint the call reached — so a url minted in another
    /// region resolves only where that region holds a queue of the same name too.
    pub fn queue_by_url(&self, region: &str, queue_url: &str) -> Option<&Queue> {
        self.queue(region, queue_name_from_url(queue_url)?)
    }

    /// The queue a `QueueUrl` addresses in `region`, for writing.
    pub fn queue_by_url_mut(&mut self, region: &str, queue_url: &str) -> Option<&mut Queue> {
        let name = queue_name_from_url(queue_url)?.to_string();
        self.queue_mut(region, &name)
    }

    /// Which queue minted `receipt_handle`, as the queue at `queue_url` that cannot
    /// resolve it sees it: a handle names the queue INCARNATION that minted it
    /// ([`receipt_handle_incarnation`]), so the three cases a caller recovers from
    /// differently are told apart by that name rather than by searching the live handle
    /// maps.
    ///
    /// A handle a delete has already consumed, or a later receive superseded, still names
    /// the incarnation that issued it, and a value this service never minted names none.
    pub fn receipt_handle_origin<'a>(
        &self,
        region: &str,
        queue_url: &str,
        receipt_handle: &'a str,
    ) -> ReceiptHandleOrigin<'a> {
        let Some(minted_by) = receipt_handle_incarnation(receipt_handle) else {
            return ReceiptHandleOrigin::Unrecognised;
        };
        if self
            .queue_by_url(region, queue_url)
            .is_some_and(|queue| queue.incarnation == minted_by)
        {
            // The queue addressed is the one that minted it, so what the refusal is about
            // is the handle's MESSAGE rather than which queue it belongs to.
            return ReceiptHandleOrigin::Unrecognised;
        }
        if self
            .queues
            .values()
            .any(|queue| queue.incarnation == minted_by)
        {
            ReceiptHandleOrigin::AnotherQueue(receipt_handle)
        } else {
            ReceiptHandleOrigin::PreviousIncarnation
        }
    }

    /// Remove the queue a `QueueUrl` addresses in `region`, with every message and handle
    /// it held, and remember that its NAME was deleted at `at` — which is what makes a
    /// re-create of the same name in that region inside
    /// [`QUEUE_DELETED_RECENTLY_INTERVAL`] refusable.
    pub fn remove_queue_by_url(
        &mut self,
        region: &str,
        queue_url: &str,
        at: SystemTime,
    ) -> Option<Queue> {
        let key = queue_key(region, queue_name_from_url(queue_url)?);
        let removed = self.queues.remove(&key)?;
        self.forget_lapsed_deletions(at);
        self.recently_deleted.insert(key, at);
        // The ledger is bounded; the oldest record goes first, so the names deleted most
        // recently — the ones a re-create is most likely to name — are the ones kept.
        while self.recently_deleted.len() > MAX_RECENTLY_DELETED_NAMES {
            if let Some(oldest) = self
                .recently_deleted
                .iter()
                .min_by_key(|(_, deleted_at)| **deleted_at)
                .map(|(key, _)| key.clone())
            {
                self.recently_deleted.remove(&oldest);
            }
        }
        Some(removed)
    }

    /// Forget every deletion record whose [`QUEUE_DELETED_RECENTLY_INTERVAL`] has
    /// elapsed by `now`: the name became available again at that instant whether or not
    /// anybody called, so materialising it is owed by the clock rather than by this call.
    pub fn forget_lapsed_deletions(&mut self, now: SystemTime) {
        self.recently_deleted
            .retain(|_, deleted_at| !lapsed(*deleted_at, QUEUE_DELETED_RECENTLY_INTERVAL, now));
    }

    /// When the name `name` was deleted in `region`, if a delete removed it there inside
    /// [`QUEUE_DELETED_RECENTLY_INTERVAL`] of `now`.
    ///
    /// The ledger is keyed by region and name and says nothing about whether a queue
    /// carries that name again, so a caller that needs both — `CreateQueue` — resolves the
    /// live queue first and reaches here only for a name no queue of that region answers
    /// to.
    pub fn deleted_recently(
        &self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<SystemTime> {
        self.recently_deleted
            .get(&queue_key(region, name))
            .copied()
            .filter(|deleted_at| !lapsed(*deleted_at, QUEUE_DELETED_RECENTLY_INTERVAL, now))
    }

    /// The names of the queues of `region` whose `RedrivePolicy` names the queue called
    /// `dead_letter_queue_name` as their dead-letter target, in queue-name order.
    ///
    /// These are that queue's dead-letter SOURCE queues, and having at least one is
    /// what makes it "configured as a dead-letter queue (DLQ) of other Amazon SQS
    /// queues" — the only kind of source `StartMessageMoveTask` supports. A redrive link
    /// never crosses a region: a target ARN is read in the region the policy's own queue
    /// lives in, so only that region's queues can be sources of it.
    pub fn dead_letter_source_queue_names(
        &self,
        region: &str,
        dead_letter_queue_name: &str,
    ) -> Vec<String> {
        self.queues_in(region)
            .filter(|queue| {
                queue
                    .attributes
                    .get(REDRIVE_POLICY)
                    .and_then(|policy| dead_letter_target_arn(policy))
                    .and_then(|arn| queue_name_from_arn(&arn, region).map(str::to_string))
                    .is_some_and(|target| target == dead_letter_queue_name)
            })
            .map(|queue| queue.name.clone())
            .collect()
    }

    /// Why the queue whose ARN is `source_arn` may not take the `RedrivePolicy` value
    /// `policy` — the dead-letter queue it names does not admit it as a source queue — as
    /// the text of the refusal a caller has to fix the request from, or `None` when it may.
    ///
    /// A dead-letter queue decides which queues may drive their messages to it
    /// ([`Queue::admits_dead_letter_source`]), and that decision is taken at the moment a
    /// source queue associates itself with the target: a `RedrivePolicy` the target's
    /// `RedriveAllowPolicy` does not admit is refused rather than stored, so a queue the
    /// permission denies never ends up wired to it. The target is resolved in `region`,
    /// where a redrive link lives (see [`Queue::dead_letter_target`]); a policy naming a
    /// target this region holds no queue for, or carrying no delivery count to pass,
    /// configures no association for any permission to refuse.
    pub fn redrive_allow_policy_refusal(
        &self,
        region: &str,
        source_arn: &str,
        policy: &str,
    ) -> Option<String> {
        let parsed = parsed_redrive_policy(policy)?;
        let target_arn = redrive_target_arn(&parsed)?;
        let count = redrive_max_receive_count_text(&parsed)?;
        redrive_max_receive_count(&parsed)?;
        let target = self.queue(region, queue_name_from_arn(target_arn, region)?)?;
        if target.admits_dead_letter_source(source_arn) {
            return None;
        }
        Some(invalid_parameter_message(
            &refused_redrive_policy_value(&count, target_arn),
            REDRIVE_POLICY,
            REDRIVE_ALLOW_POLICY_REASON,
        ))
    }

    /// Why the queue `policy` would be written on may not drive its messages to the
    /// dead-letter queue that `RedrivePolicy` value names, as the text of the refusal a
    /// caller has to fix the request from, or `None` when it may.
    ///
    /// Two rules about the PAIR of queues, both of which need the store:
    ///
    /// * The target has to be a queue there IS. A redrive moves a message from the source
    ///   queue to the target queue, so a policy naming a queue nothing holds configures a
    ///   move that could never be made, and is refused rather than stored
    ///   ([`DEAD_LETTER_TARGET_MISSING_REASON`]).
    /// * It has to be a queue of the source's own TYPE: "the dead-letter queue of a FIFO
    ///   queue must also be a FIFO queue. Similarly, the dead-letter queue of a standard
    ///   queue must also be a standard queue" ([`DEAD_LETTER_TARGET_TYPE_REASON`]).
    ///
    /// `source_is_fifo` says which type the queue taking the policy is, which at create
    /// time is what the request asks for and afterwards what the queue is. The target is
    /// resolved in `region`, where a redrive link lives (see
    /// [`Queue::dead_letter_target`]); a target of another namespace is refused by the
    /// document's own rules ([`redrive_policy_refusal`]) before this is reached, and a
    /// policy carrying no target at all is refused there too, so there is nothing here to
    /// judge for one.
    pub fn dead_letter_target_refusal(
        &self,
        region: &str,
        source_is_fifo: bool,
        policy: &str,
    ) -> Option<String> {
        let parsed = parsed_redrive_policy(policy)?;
        let target_arn = redrive_target_arn(&parsed)?;
        let refused =
            |reason: &str| Some(invalid_parameter_message(policy, REDRIVE_POLICY, reason));
        let Some(target) = queue_name_from_arn(target_arn, region)
            .and_then(|name| self.queue(region, name))
        else {
            return refused(DEAD_LETTER_TARGET_MISSING_REASON);
        };
        (target.is_fifo() != source_is_fifo).then(|| {
            invalid_parameter_message(policy, REDRIVE_POLICY, DEAD_LETTER_TARGET_TYPE_REASON)
        })
    }

    /// Move every message of the queue called `source_queue_name` that has had all the
    /// deliveries its `RedrivePolicy` allows onto the dead-letter queue that policy
    /// names, and answer how many moved.
    ///
    /// `maxReceiveCount` is "the number of times a message is delivered to the source
    /// queue before being moved to the dead-letter queue" (the `RedrivePolicy`
    /// attribute's own documentation), so a message the source queue has already handed
    /// over that many times is not handed over again: it is moved to
    /// `deadLetterTargetArn` and is visible there at once, which is what makes the source
    /// queue's depth fall as the dead-letter queue's rises. A message currently IN FLIGHT
    /// with a consumer is left alone — its consumer may still delete it, and its delivery
    /// is the one being counted — as is one whose delay has not elapsed, which has never
    /// been delivered at all.
    ///
    /// The message keeps its id, body, attributes and receive count: it is the same
    /// message in another queue, and the count is of receives "across all queues". The
    /// deliveries the queue it LEFT made are not deliveries of the queue it arrives in,
    /// so the per-queue count its new queue's own `RedrivePolicy` bounds starts afresh.
    /// On a STANDARD dead-letter queue its RETENTION keeps running from the instant it was
    /// enqueued originally, because "the expiration of a message is always based on its
    /// original enqueue timestamp" — being dead-lettered does not buy a message a fresh
    /// retention period there. A FIFO queue enqueues what it is handed, so a message
    /// arriving in one is enqueued AT the move and the retention period runs from there.
    /// Neither re-sends it: `SentTimestamp` is the instant the message was sent, whichever
    /// queue holds it. The receipt handles the source queue minted
    /// for it go with the message leaving that queue, exactly as they do when a message
    /// movement task takes it: a handle is only ever valid for the queue that minted it.
    ///
    /// Putting messages into the dead-letter queue is bounded like every other path that
    /// adds messages to a queue (see [`Queue::batch_rejection`]); a message the target
    /// queue cannot take stays where it is rather than being dropped, so no message a
    /// client was told had been accepted is lost. Each message is judged on its own here
    /// rather than as the all-or-nothing batch that helper's other callers pass it,
    /// because these messages are moved one delivery at a time and nothing ties them
    /// together: a message the target cannot take is simply handed over again later.
    ///
    /// Both queues are the ones `region` holds: a redrive link never crosses a region
    /// (see [`Queue::dead_letter_target`]).
    ///
    /// The target queue's `RedriveAllowPolicy` is consulted HERE as well as when the
    /// association was made ([`Self::redrive_allow_policy_refusal`]): the permission that
    /// governs a move is the one the dead-letter queue carries at the instant of the move,
    /// so a target that no longer admits this source takes none of its messages and the
    /// message stays visible on the source queue, to be handed over again.
    ///
    /// Each moved message is stamped with the ARN of the queue it was moved out of
    /// ([`StoredMessage::dead_letter_source_arn`]), which a receive on the dead-letter
    /// queue reports under `DeadLetterQueueSourceArn`.
    pub fn dead_letter_due(
        &mut self,
        region: &str,
        source_queue_name: &str,
        now: SystemTime,
    ) -> usize {
        let source_key = queue_key(region, source_queue_name);
        let Some((target_name, max_receive_count)) = self
            .queues
            .get(&source_key)
            .and_then(Queue::dead_letter_target)
        else {
            return 0;
        };
        let target_key = queue_key(region, &target_name);
        // A queue that drives its messages to itself, or to a queue this store does not
        // hold, has nowhere to move them to.
        if target_key == source_key || !self.queues.contains_key(&target_key) {
            return 0;
        }
        let Some(source) = self.queues.get(&source_key) else {
            return 0;
        };
        // Whether the dead-letter queue takes this source queue's messages at all is its
        // own permission to give, re-read on every move.
        let source_arn = source.arn.clone();
        if !self
            .queues
            .get(&target_key)
            .is_some_and(|target| target.admits_dead_letter_source(&source_arn))
        {
            return 0;
        }
        let exceeded: Vec<(String, usize)> = source
            .messages
            .iter()
            .filter(|message| {
                message.visibility(now) == Visibility::Visible
                    && message.queue_receive_count >= max_receive_count
            })
            .map(|message| (message.message_id.clone(), message.retained_bytes()))
            .collect();
        if exceeded.is_empty() {
            return 0;
        }

        // The bytes leave the source queue as they reach the dead-letter one, so the
        // store-wide budget is judged on what the move would leave behind.
        let store_bytes = self.retained_message_bytes();
        let mut moving: BTreeSet<String> = BTreeSet::new();
        if let Some(target) = self.queues.get(&target_key) {
            let mut bytes = 0usize;
            for (message_id, message_bytes) in &exceeded {
                let total = bytes.saturating_add(*message_bytes);
                if target
                    .batch_rejection(
                        moving.len().saturating_add(1),
                        total,
                        store_bytes.saturating_sub(total),
                    )
                    .is_none()
                {
                    bytes = total;
                    moving.insert(message_id.clone());
                }
            }
        }

        let mut moved: Vec<StoredMessage> = Vec::with_capacity(moving.len());
        if let Some(source) = self.queues.get_mut(&source_key) {
            let mut staying: VecDeque<StoredMessage> = VecDeque::new();
            for message in std::mem::take(&mut source.messages) {
                if moving.contains(&message.message_id) {
                    source.handles.retain(|_, id| *id != message.message_id);
                    moved.push(message);
                } else {
                    staying.push_back(message);
                }
            }
            source.messages = staying;
        }
        let count = moved.len();
        if let Some(target) = self.queues.get_mut(&target_key) {
            // A FIFO queue ENQUEUES the message it is handed: the arrival is a send into
            // that queue's message group, so the retention period the dead-letter queue
            // keeps the message for runs from the move. A standard queue takes the
            // message as the one it already was, where "the expiration of a message is
            // always based on its original enqueue timestamp", so being dead-lettered
            // buys it no fresh retention there. Neither re-SENDS it: the message was sent
            // once, and `SentTimestamp` is the instant its sender was answered under
            // whichever queue holds it now.
            let enqueued_at = target.is_fifo().then_some(now);
            for message in moved {
                target.messages.push_back(StoredMessage {
                    // "SQS moves the message to the dead-letter queue", where a consumer
                    // of that queue can take it: it is deliverable from the move.
                    available_at: now,
                    invisible_until: None,
                    // The deliveries it had from the queue it has left are not deliveries
                    // of this one; its cross-queue `receive_count` is untouched.
                    queue_receive_count: 0,
                    retention_from: enqueued_at.unwrap_or(message.retention_from),
                    // Where the message came FROM: a dead-letter queue is routinely shared
                    // by several source queues, so the moved message carries the ARN of
                    // the queue the move took it out of and a consumer of the dead-letter
                    // queue can tell the two apart.
                    dead_letter_source_arn: Some(source_arn.clone()),
                    ..message
                });
            }
        }
        count
    }

    /// The message movement task that is still running for the queue called
    /// `source_queue_name` in `region` at `now`, if any: "Only one active message movement
    /// task is supported per queue at any given time" (`StartMessageMoveTask`) — and the
    /// queue a task is per is one region's queue, so two regions' same-named queues each
    /// have an allowance of their own.
    pub fn active_message_move_task(
        &self,
        region: &str,
        source_queue_name: &str,
        now: SystemTime,
    ) -> Option<&MessageMoveTask> {
        self.message_move_tasks.iter().find(|task| {
            task.region == region
                && task.source_queue_name == source_queue_name
                && task.status(now) == MESSAGE_MOVE_RUNNING
        })
    }

    /// The message movement task the handle a start call returned names, if `region` holds
    /// its record: a task moves one region's messages between that region's queues, so a
    /// handle minted in another region names no task here.
    pub fn message_move_task(&self, region: &str, task_handle: &str) -> Option<&MessageMoveTask> {
        self.message_move_tasks
            .iter()
            .find(|task| task.region == region && task.task_handle == task_handle)
    }

    /// Stop the message movement task called `task_handle` at `now`, and answer how many
    /// of its messages it had moved by then.
    ///
    /// "Cancelling a message movement task does not revert the messages that have already
    /// been moved. It can only stop the messages that have not been moved yet"
    /// (`CancelMessageMoveTask`): a task takes a message out of its source queue only when
    /// its own schedule has reached that message ([`Self::advance_message_move_tasks`]), so
    /// the messages it has not moved are still the source queue's own and stopping the task
    /// is simply recording that it moves nothing more. The cancellation instant is kept on
    /// the task record, which is what every later reader projects its status and its
    /// progress from.
    ///
    /// `None` when no task answers to the handle.
    pub fn cancel_message_move_task(
        &mut self,
        region: &str,
        task_handle: &str,
        now: SystemTime,
    ) -> Option<u64> {
        let task = self
            .message_move_tasks
            .iter_mut()
            .find(|task| task.region == region && task.task_handle == task_handle)?;
        task.cancelled_at = Some(now);
        Some(task.messages_moved())
    }

    /// Carry every message movement task still in progress as far as `now` has made it
    /// due, moving the messages each task's schedule has reached out of its source queue
    /// and into its destination queue, and completing a task whose source queue holds no
    /// more messages.
    ///
    /// `StartMessageMoveTask` "starts an ASYNCHRONOUS task": the movement is work the task
    /// owes over time rather than work its start call does, so what has been moved is a
    /// function of the task record and the clock. Every reader applies what was already
    /// due — the same way [`Queue::materialise_due`] applies a lapsed visibility timeout —
    /// so no reader can see a task that is further on than the messages are.
    ///
    /// What a task may take is what its source queue can hand out at `now`: a message in
    /// flight with a consumer is not the queue's to give away, and a delayed one has never
    /// been delivered at all, so each is left and taken by a later turn once the clock
    /// makes it available. That is also why a task is COMPLETED only once the source queue
    /// holds nothing more: a `COMPLETED` status is the client's only statement that the
    /// drain finished, so it is reported over a source queue that really is empty.
    ///
    /// Handing messages to the destination queue is bounded like every other path that adds
    /// messages to a queue ([`Queue::batch_rejection`]): what the destination cannot take
    /// stays in the source queue and the task keeps owing it, so no message a client was
    /// told had been accepted is lost.
    pub fn advance_message_move_tasks(&mut self, now: SystemTime) {
        let in_progress: Vec<String> = self
            .message_move_tasks
            .iter()
            .filter(|task| task.in_progress())
            .map(|task| task.task_handle.clone())
            .collect();
        for handle in in_progress {
            self.advance_message_move_task(&handle, now);
        }
    }

    /// [`Self::advance_message_move_tasks`] for the one task `task_handle` names.
    fn advance_message_move_task(&mut self, task_handle: &str, now: SystemTime) {
        let Some(task) = self
            .message_move_tasks
            .iter()
            .find(|task| task.task_handle == task_handle)
        else {
            return;
        };
        // Nothing is owed before the task's first delivery slot: a consumer polling the
        // destination queue the moment the start call answered sees none of the messages
        // yet, and the task is still running.
        let Some(allowance) = task.deliverable_by(now) else {
            return;
        };
        let region = task.region.clone();
        let source_key = queue_key(&region, &task.source_queue_name);
        let destination_key = queue_key(&region, &task.destination_queue_name);
        // A task with nowhere to move messages to — a queue of the pair deleted since the
        // start — has nothing left to do, so it is finished rather than left running for
        // ever against a source it cannot drain. A task's source and destination are never
        // the same queue (`StartMessageMoveTask` refuses that pair), so that arm only
        // guards a record whose queues have gone.
        if source_key == destination_key
            || !self.queues.contains_key(&source_key)
            || !self.queues.contains_key(&destination_key)
        {
            self.complete_message_move_task(task_handle, now);
            return;
        }

        // The messages the source queue can hand out, oldest first, as far as this turn's
        // allowance and what the destination queue may hold reach.
        let store_bytes = self.retained_message_bytes();
        let available: Vec<(String, usize)> = self
            .queues
            .get(&source_key)
            .map(|source| {
                source
                    .messages
                    .iter()
                    .filter(|message| message.visibility(now) == Visibility::Visible)
                    .take(usize::try_from(allowance).unwrap_or(usize::MAX))
                    .map(|message| (message.message_id.clone(), message.retained_bytes()))
                    .collect()
            })
            .unwrap_or_default();
        let mut moving: BTreeSet<String> = BTreeSet::new();
        if let Some(destination) = self.queues.get(&destination_key) {
            let mut bytes = 0usize;
            for (message_id, message_bytes) in &available {
                let total = bytes.saturating_add(*message_bytes);
                // The bytes leave the source queue as they reach the destination, so the
                // store-wide budget is judged on what the move would leave behind.
                if destination
                    .batch_rejection(
                        moving.len().saturating_add(1),
                        total,
                        store_bytes.saturating_sub(total),
                    )
                    .is_none()
                {
                    bytes = total;
                    moving.insert(message_id.clone());
                }
            }
        }

        let mut taken: Vec<StoredMessage> = Vec::with_capacity(moving.len());
        if let Some(source) = self.queues.get_mut(&source_key) {
            let mut staying: VecDeque<StoredMessage> = VecDeque::new();
            for message in std::mem::take(&mut source.messages) {
                if moving.contains(&message.message_id) {
                    // The receipt handles the source queue minted for it go with the
                    // message leaving that queue: a handle is only ever valid for the queue
                    // that minted it.
                    source.handles.retain(|_, id| *id != message.message_id);
                    taken.push(message);
                } else {
                    staying.push_back(message);
                }
            }
            source.messages = staying;
        }
        let moved = u64::try_from(taken.len()).unwrap_or(u64::MAX);
        if let Some(destination) = self.queues.get_mut(&destination_key) {
            for message in taken {
                destination.messages.push_back(StoredMessage {
                    // The message is deliverable from the move: the destination queue has
                    // it, and a consumer of that queue can take it.
                    available_at: now,
                    invisible_until: None,
                    // Retention in the destination runs from the move, which is when that
                    // queue enqueued it, and so does the per-queue delivery count its own
                    // `RedrivePolicy` bounds: the deliveries the source queue made were
                    // not deliveries of this one. Neither re-SENDS it — `SentTimestamp` is
                    // the instant its sender was answered, whichever queue holds it now.
                    retention_from: now,
                    queue_receive_count: 0,
                    // A task moves the message it is handed: what it already recorded
                    // about an earlier redrive travels with it, so a message driven back
                    // out of a dead-letter queue still names the queue it was
                    // dead-lettered from.
                    ..message
                });
            }
        }
        let drained = self
            .queues
            .get(&source_key)
            .is_some_and(|source| source.messages.is_empty());
        if let Some(task) = self
            .message_move_tasks
            .iter_mut()
            .find(|task| task.task_handle == task_handle)
        {
            task.moved_count = task.moved_count.saturating_add(moved);
            if drained {
                task.completed_at = Some(now);
            }
        }
    }

    /// Record that the task `task_handle` names finished at `now`, whatever it still had
    /// to move: a task that cannot reach its source or its destination queue any more.
    fn complete_message_move_task(&mut self, task_handle: &str, now: SystemTime) {
        if let Some(task) = self
            .message_move_tasks
            .iter_mut()
            .find(|task| task.task_handle == task_handle)
        {
            task.completed_at = Some(now);
        }
    }

    /// Record a started message movement task, keeping at most
    /// [`MAX_MESSAGE_MOVE_TASKS_PER_QUEUE`] records per source queue and
    /// [`MAX_MESSAGE_MOVE_TASKS`] in all.
    ///
    /// Beyond either bound the OLDEST record is dropped, not refused:
    /// `ListMessageMoveTasks` reports "the most recent message movement tasks (up to 10)
    /// under a specific source queue", so nothing a caller can still list is lost, and a
    /// caller looping on `StartMessageMoveTask` cannot grow this store without end.
    ///
    /// The per-queue bound is counted on the task's region and
    /// [`MessageMoveTask::source_queue_name`] — the queue the start call RESOLVED, never a
    /// string the request supplied — so every request naming one queue shares one
    /// allowance. The store-wide bound is the backstop for the names of queues that no
    /// longer exist, and for the regions a caller can invent by signing for them, neither
    /// of which a per-queue count can limit.
    pub fn record_message_move_task(&mut self, task: MessageMoveTask) {
        let source = queue_key(&task.region, &task.source_queue_name);
        self.message_move_tasks.push(task);
        while self
            .message_move_tasks
            .iter()
            .filter(|recorded| recorded.is_of(&source))
            .count()
            > MAX_MESSAGE_MOVE_TASKS_PER_QUEUE
        {
            let Some(oldest) = self
                .message_move_tasks
                .iter()
                .position(|recorded| recorded.is_of(&source))
            else {
                break;
            };
            self.message_move_tasks.remove(oldest);
        }
        while self.message_move_tasks.len() > MAX_MESSAGE_MOVE_TASKS {
            self.message_move_tasks.remove(0);
        }
    }

    /// The message movement tasks to report for the queue called `source_queue_name` in
    /// `region`: "the most recent message movement tasks (up to 10) under a specific source
    /// queue" (`ListMessageMoveTasks`), most recent FIRST and at most `max_results` of
    /// them.
    ///
    /// Every member is PROJECTED from the task record and `now`, the way
    /// [`Queue::reported_attributes`] projects a queue's reported attributes: the status
    /// and the moved count are functions of the record and the clock, the to-move count
    /// is the size the task was started over, and each ARN is the one the queue record
    /// itself reports, so two readers cannot disagree about a task and nothing stores a
    /// progress value it would have to keep up to date.
    ///
    /// A member the entry populates only in a stated condition is populated in THAT
    /// condition and absent otherwise, so a reader can tell the condition from the reply:
    /// `TaskHandle` "is only populated for tasks that are in RUNNING status",
    /// `DestinationArn` is "the ARN of the destination queue IF IT HAS BEEN SPECIFIED in
    /// the StartMessageMoveTask request" and is NULL when the request specified none,
    /// `MaxNumberOfMessagesPerSecond` likewise carries only a rate the request fixed, and
    /// `FailureReason` is "only included if the task status is FAILED" — which no task this
    /// store records reaches, because a task here keeps moving what its source queue can
    /// give until there is nothing left, so a status it never reaches owes no reason.
    pub fn reported_message_move_tasks(
        &self,
        region: &str,
        source_queue_name: &str,
        now: SystemTime,
        max_results: usize,
    ) -> Vec<ListMessageMoveTasksResultEntry> {
        // `message_move_tasks` is append-ordered, so reversing it is start-time order,
        // newest first.
        self.message_move_tasks
            .iter()
            .rev()
            .filter(|task| task.region == region && task.source_queue_name == source_queue_name)
            .take(max_results)
            .map(|task| {
                let status = task.status(now);
                ListMessageMoveTasksResultEntry {
                    task_handle: (status == MESSAGE_MOVE_RUNNING)
                        .then(|| task.task_handle.clone()),
                    status: Some(status.to_string()),
                    source_arn: Some(self.reported_queue_arn(region, &task.source_queue_name)),
                    destination_arn: task.destination_specified.then(|| {
                        self.reported_queue_arn(region, &task.destination_queue_name)
                    }),
                    max_number_of_messages_per_second: task.max_number_of_messages_per_second,
                    approximate_number_of_messages_moved: count_as_i64(task.messages_moved()),
                    approximate_number_of_messages_to_move: Some(count_as_i64(
                        task.messages_to_move(),
                    )),
                    failure_reason: None,
                    started_timestamp: epoch_millis_number(task.started_at),
                }
            })
            .collect()
    }

    /// The ARN to report for the queue called `name` in `region`: the one the queue record
    /// carries, or the ARN that name resolves to in `region` when no queue answers to it
    /// any more — a task record outlives the queues it names.
    fn reported_queue_arn(&self, region: &str, name: &str) -> String {
        self.queue(region, name)
            .map(|queue| queue.arn.clone())
            .unwrap_or_else(|| queue_arn(region, name))
    }
}

/// A message count as the `long` a response carries it in. Every count here is bounded by
/// the messages one queue may hold, so the saturation is unreachable.
fn count_as_i64(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------------
// Message movement tasks
// ---------------------------------------------------------------------------

/// The largest message movement rate `StartMessageMoveTask` accepts: "The maximum value
/// for messages per second is 500"
/// (`StartMessageMoveTaskRequest$MaxNumberOfMessagesPerSecond`).
pub const MAX_MESSAGES_PER_SECOND: i32 = 500;

/// The smallest message movement rate it accepts: a rate below one would move no message
/// at all, so one is the lowest rate a task can be started at.
pub const MIN_MESSAGES_PER_SECOND: i32 = 1;

/// The parameter that rate is supplied for, as the request carries it.
pub const MESSAGES_PER_SECOND_PARAMETER: &str = "MaxNumberOfMessagesPerSecond";

/// How a refusal states that the rate a message movement task was asked to move messages
/// at is outside the range the parameter takes.
///
/// It states the RANGE and not the value supplied: the bounds are what the caller has to
/// fix the request against, and both ends of the range are refused with the one sentence.
pub fn message_move_rate_out_of_range_message() -> String {
    invalid_parameter_message_without_value(
        MESSAGES_PER_SECOND_PARAMETER,
        &format!(
            "You must enter a number that's between {MIN_MESSAGES_PER_SECOND} and \
             {MAX_MESSAGES_PER_SECOND}."
        ),
    )
}

/// How long after the start call a message movement task may move its first message.
///
/// `StartMessageMoveTask` "starts an asynchronous task": it answers with a task handle
/// before the task has moved anything, so this emulator owes the first move a short span
/// after the call rather than at it — a consumer polling the destination queue straight
/// away still sees the task running and none of its messages yet.
pub const MESSAGE_MOVE_TASK_START_DELAY: Duration = Duration::from_millis(500);

/// How many message movement task records one source queue keeps, and the most
/// `ListMessageMoveTasks` reports for it: "Gets the most recent message movement tasks
/// (up to 10) under a specific source queue" / "The upper limit is 10"
/// (`ListMessageMoveTasksRequest$MaxResults`).
pub const MAX_MESSAGE_MOVE_TASKS_PER_QUEUE: usize = 10;

/// How many message movement tasks `ListMessageMoveTasks` reports when the request names
/// no `MaxResults`: "The default is 1, which provides the most recent message movement
/// task" (`ListMessageMoveTasksRequest$MaxResults`).
pub const DEFAULT_MESSAGE_MOVE_TASK_RESULTS: usize = 1;

/// How many message movement task records the whole store holds. The per-queue bound
/// alone would still scale with the number of queue NAMES that have ever existed, which
/// a caller creating and deleting queues can grow; this bounds them all together.
pub const MAX_MESSAGE_MOVE_TASKS: usize = MAX_QUEUES * MAX_MESSAGE_MOVE_TASKS_PER_QUEUE;

/// Nanoseconds in one second, so a rate in messages per second becomes a span per
/// message without a floating-point division.
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A message movement task's status while it still has messages to deliver. The status
/// set is the member's own: "Possible values are: RUNNING, COMPLETED, CANCELLING,
/// CANCELLED, and FAILED" (`ListMessageMoveTasksResultEntry$Status`).
pub const MESSAGE_MOVE_RUNNING: &str = "RUNNING";

/// A message movement task's status once every message it took has been delivered.
pub const MESSAGE_MOVE_COMPLETED: &str = "COMPLETED";

/// A message movement task's status while its cancellation is being applied.
pub const MESSAGE_MOVE_CANCELLING: &str = "CANCELLING";

/// A message movement task's status once its cancellation has been applied.
pub const MESSAGE_MOVE_CANCELLED: &str = "CANCELLED";

/// How long a cancelled task reports [`MESSAGE_MOVE_CANCELLING`] before it reports
/// [`MESSAGE_MOVE_CANCELLED`].
///
/// `CancelMessageMoveTask` answers while the task is still winding down — the status set
/// has a distinct `CANCELLING` state before `CANCELLED`
/// (`ListMessageMoveTasksResultEntry$Status`) — so a caller listing the task straight
/// after the cancel call sees it cancelling and only later sees it cancelled. The span
/// itself is this emulator's choice, not a documented duration.
pub const MESSAGE_MOVE_CANCELLATION_SPAN: Duration = Duration::from_millis(500);

/// The `TaskHandle` a started message movement task is reported under: "An identifier
/// associated with a message movement task", as the one opaque encoded token the service
/// hands out ([`encoded_opaque_token`]), carrying the task's own identity — the token this
/// service minted for it and the source queue it moves messages out of.
///
/// `CancelMessageMoveTask` takes that handle back verbatim, so the value a caller holds is
/// what resolves the task; nothing is read out of its text.
pub fn message_move_task_handle(task_id: &str, source_arn: &str) -> String {
    encoded_opaque_token(
        &::serde_json::json!({
            "taskId": task_id,
            "sourceArn": source_arn,
        })
        .to_string(),
    )
}

/// One message movement task `StartMessageMoveTask` started.
///
/// The record holds only what the start call RESOLVED — the queues it found, the rate and
/// the depth it sized the move over — and nothing a request supplied verbatim: each queue
/// is held as the `QueueName` its ARN resolved to, which is the identity every reader and
/// every bound keys on, and the ARNs a reader reports are projected from those queue
/// records rather than stored a second time.
///
/// A task's PROGRESS is the moving itself: the messages it has taken out of its source
/// queue, counted in [`moved_count`](Self::moved_count) as
/// [`ComAmazonawsSqsAmazonsqsState::advance_message_move_tasks`] takes them, and the
/// instant it ran out of messages to take, in [`completed_at`](Self::completed_at). So a
/// status is never a function of a timer alone: `COMPLETED` is reported over a drain that
/// really happened, and every reader of the record agrees about it.
#[derive(Debug, Clone)]
pub struct MessageMoveTask {
    /// The opaque handle the start call returned, which `CancelMessageMoveTask` names.
    pub task_handle: String,
    /// The region the start call addressed, which is the region both its queues live in: a
    /// task moves messages between two queues of one region, so every reader resolves a
    /// task in the region it was started in and nowhere else.
    pub region: String,
    /// The name of the dead-letter queue the messages are taken from.
    pub source_queue_name: String,
    /// The name of the queue they are being delivered to.
    pub destination_queue_name: String,
    /// Whether the start call SPECIFIED that destination, rather than leaving the field
    /// blank for the messages to be "redriven back to their respective original source
    /// queues": a reader reports `DestinationArn` only for a task whose request named one.
    pub destination_specified: bool,
    /// The message movement rate the caller fixed, or `None` when it left the rate to
    /// the service ("the system will optimize the rate based on the queue message
    /// backlog size").
    pub max_number_of_messages_per_second: Option<i32>,
    /// When the start call accepted the task.
    pub started_at: SystemTime,
    /// How many messages the source queue held when this task was started, which is what
    /// `ApproximateNumberOfMessagesToMove` reports: "this number is obtained at the time of
    /// starting the message movement task", so it stays as it was sized.
    pub sized_count: u64,
    /// How many messages this task has moved to the destination queue so far, which is what
    /// `ApproximateNumberOfMessagesMoved` reports: "the approximate number of messages
    /// already moved to the destination queue".
    pub moved_count: u64,
    /// When this task ran out of messages to move, if it has: from that instant it reports
    /// `COMPLETED`, and a completed task moves nothing more.
    pub completed_at: Option<SystemTime>,
    /// When `CancelMessageMoveTask` stopped the task, if it was cancelled: from that
    /// instant the task moves nothing more, so its progress stops there too.
    pub cancelled_at: Option<SystemTime>,
}

impl MessageMoveTask {
    /// Whether this task's source queue is the one `key` identifies — the `(region, name)`
    /// pair the store holds a queue under — which is what the per-queue bound counts on.
    fn is_of(&self, key: &(String, String)) -> bool {
        self.region == key.0 && self.source_queue_name == key.1
    }

    /// Whether this task still has moving to do: one neither cancelled nor finished.
    fn in_progress(&self) -> bool {
        self.cancelled_at.is_none() && self.completed_at.is_none()
    }

    /// When this task's `index`-th message may be moved: the start delay, then one message
    /// per rate slot. With no rate fixed, every message may be moved at the end of the
    /// start delay ("the system will optimize the rate").
    pub fn delivery_instant(&self, index: u64) -> SystemTime {
        let first = self
            .started_at
            .checked_add(MESSAGE_MOVE_TASK_START_DELAY)
            .unwrap_or(self.started_at);
        let Some(rate) = self.rate() else {
            return first;
        };
        let nanos =
            u64::try_from(u128::from(index) * NANOS_PER_SECOND / u128::from(rate))
                .unwrap_or(u64::MAX);
        first
            .checked_add(Duration::from_nanos(nanos))
            .unwrap_or(first)
    }

    /// How many messages BEYOND the ones it has already moved this task may move at `now`,
    /// `None` while its first delivery slot is still ahead and `Some(u64::MAX)` for a task
    /// whose rate the service optimizes, which is bounded only by what its source queue can
    /// hand out.
    ///
    /// A fixed rate is "the number of messages to be moved per second", so the slot of the
    /// `index`-th message is `index / rate` after the first: the number of slots `now` has
    /// reached, less what the task has moved, is what this turn may take.
    fn deliverable_by(&self, now: SystemTime) -> Option<u64> {
        let first = self.delivery_instant(0);
        let elapsed = now.duration_since(first).ok()?;
        let Some(rate) = self.rate() else {
            return Some(u64::MAX);
        };
        let slots = u64::try_from(elapsed.as_nanos() * u128::from(rate) / NANOS_PER_SECOND)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        Some(slots.saturating_sub(self.moved_count))
    }

    /// The fixed message movement rate of this task, or `None` when the request left the
    /// rate to the service. A rate of zero or less would move nothing, so it is no rate.
    fn rate(&self) -> Option<u64> {
        self.max_number_of_messages_per_second
            .and_then(|rate| u64::try_from(rate).ok())
            .filter(|rate| *rate > 0)
    }

    /// How many messages this task has moved to the destination queue.
    pub fn messages_moved(&self) -> u64 {
        self.moved_count
    }

    /// How many messages this task is moving, which is "obtained at the time of starting
    /// the message movement task" (the `ApproximateNumberOfMessagesToMove` member's own
    /// documentation): the depth the source queue had when the task was started, so it
    /// does not fall as the task moves them and a finished task still reports the size it
    /// was started over.
    pub fn messages_to_move(&self) -> u64 {
        self.sized_count
    }

    /// This task's status at `now`: it runs until it has moved everything its source queue
    /// had to give, and is completed from that instant on. A cancelled task is cancelling
    /// for [`MESSAGE_MOVE_CANCELLATION_SPAN`] after the cancel call and cancelled from then
    /// on, whatever it had still to move.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if let Some(cancelled_at) = self.cancelled_at {
            let settled = cancelled_at
                .checked_add(MESSAGE_MOVE_CANCELLATION_SPAN)
                .unwrap_or(cancelled_at);
            return if now >= settled {
                MESSAGE_MOVE_CANCELLED
            } else {
                MESSAGE_MOVE_CANCELLING
            };
        }
        if self
            .completed_at
            .is_some_and(|completed_at| now >= completed_at)
        {
            MESSAGE_MOVE_COMPLETED
        } else {
            MESSAGE_MOVE_RUNNING
        }
    }
}

/// A `SystemTime` as the whole seconds since the Unix epoch, which is how SQS reports
/// `CreatedTimestamp`/`LastModifiedTimestamp`.
fn epoch_seconds(at: SystemTime) -> String {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
        .to_string()
}

/// A `SystemTime` as whole milliseconds since the Unix epoch, which is how SQS reports
/// the `SentTimestamp`/`ApproximateFirstReceiveTimestamp` message system attributes
/// ("epoch time in milliseconds").
pub fn epoch_millis(at: SystemTime) -> String {
    epoch_millis_number(at).to_string()
}

/// The same instant as a NUMBER of milliseconds since the Unix epoch, for the members a
/// response carries as a `long` rather than as a string (`StartedTimestamp`).
///
/// An instant before the epoch, and one too far past it to count in an `i64`, are both
/// unreachable for a value this store derives from the clock.
pub fn epoch_millis_number(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default()
        .try_into()
        .unwrap_or(i64::MAX)
}

/// The key one queue is held under: the region whose namespace holds it and its
/// `QueueName`.
///
/// Every read and every write of [`ComAmazonawsSqsAmazonsqsState::queues`] and
/// [`ComAmazonawsSqsAmazonsqsState::recently_deleted`] goes through this, so no path can
/// key one of those maps by the name alone and reach another region's queue.
pub fn queue_key(region: &str, name: &str) -> (String, String) {
    (region.to_string(), name.to_string())
}

/// The ARN of the queue named `name` in `region`:
/// `arn:aws:sqs:<region>:<account>:<QueueName>`.
///
/// The account segment comes from the core's single source of truth, never a literal.
pub fn queue_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, name)
}

/// The DNS suffix AWS publishes its commercial-partition service endpoints under: the tail
/// of the authority a queue url names. Named rather than spelled inline, as the other
/// crates in this tree that report an endpoint host do.
const ENDPOINT_DNS_SUFFIX: &str = "amazonaws.com";

/// The url of the queue named `name` in `region`, as Amazon SQS names it:
/// `https://<endpoint prefix>.<region>.<dns suffix>/<accountId>/<QueueName>`.
///
/// The authority is the service's REGIONAL endpoint — the endpoint prefix the model
/// publishes ([`ARN_SERVICE`], which is also this service's signing name and
/// `arnNamespace`) under the region the call was made in — and never the socket a request
/// happened to arrive on, because this value is what a client stores, derives the queue
/// ARN from, embeds in a policy document and reads the region and the account back out of:
/// it has to be ONE string for one queue, whatever address its caller reached this
/// emulator at and across restarts. The account segment is this emulator's own
/// ([`::engine::emulator_account_id`]) — the account every resource it mints belongs to —
/// rather than anything the request carried.
///
/// What that authority is NOT is an address anything here connects to. This service opens
/// no client, no socket and no resolver: it answers the requests that arrive and makes
/// none of its own. Nor does the authority decide anything, even for the urls this service
/// is HANDED back: a `QueueUrl` resolves to its queue by its `/<accountId>/<QueueName>`
/// path alone ([`queue_name_from_url`]). So a caller holding one of these keeps reaching
/// its queue here — it is already talking to this emulator, which is how it was handed the
/// url — and one that instead took the authority for an endpoint would be asking a public
/// endpoint about a queue of the emulator's own account, which holds nothing of that
/// caller's to read or to change.
pub fn queue_url(region: &str, name: &str) -> String {
    format!(
        "https://{ARN_SERVICE}.{region}.{ENDPOINT_DNS_SUFFIX}/{}/{name}",
        ::engine::emulator_account_id()
    )
}

/// The `QueueName` a `QueueUrl` addresses, or `None` for a value that is not a queue
/// url of this account at all.
///
/// A queue url is `<endpoint>/<accountId>/<QueueName>`, so the name is its last path
/// segment and the account the segment before it. Keying on that pair rather than on
/// the whole string is what lets a client reach its queue through any endpoint this
/// emulator answers on — the url it was handed, or one built for another host name —
/// while a url naming a different account resolves to nothing.
pub fn queue_name_from_url(queue_url: &str) -> Option<&str> {
    let (prefix, name) = queue_url.trim_end_matches('/').rsplit_once('/')?;
    if name.is_empty() {
        return None;
    }
    let (_, account) = prefix.rsplit_once('/')?;
    if account != ::engine::emulator_account_id() {
        return None;
    }
    Some(name)
}

/// The modelled refusal for a `SourceArn` or `DestinationArn` that is not an ARN naming
/// a queue at all (`InvalidAddress`: "The specified ID is invalid.").
///
/// Only the member NAME reaches the payload, never the value it carried.
pub fn invalid_address(member: &str) -> InvalidAddress {
    InvalidAddress {
        message: Some(format!(
            "The address supplied for the {member} parameter is not a valid Amazon SQS queue ARN."
        )),
    }
}

/// The modelled refusal for a `SourceArn` or `DestinationArn` no queue of this account
/// answers to, naming the member the caller has to fix.
///
/// Only the member NAME reaches the payload, never the value it carried, so a caller
/// probing for a queue it may not address is never handed its own input back.
pub fn resource_not_found(member: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "The resource that you specified for the {member} parameter doesn't exist."
        )),
    }
}

/// The modelled refusal for a `TaskHandle` no recorded message movement task answers to
/// (`ResourceNotFoundException`: "One or more specified resources don't exist.").
///
/// The service words this one for the task rather than for the parameter, and carries no
/// value the request supplied.
pub fn unknown_message_move_task() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Task does not exist.".to_string()),
    }
}

/// What a `SourceArn` or `DestinationArn` value names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueArn<'a> {
    /// An Amazon SQS queue ARN of this account in the region the request addressed,
    /// naming the queue called this.
    OfThisRegion(&'a str),
    /// An Amazon SQS queue ARN of this account naming ANOTHER region's namespace: a queue
    /// called this may exist there, but no call served in this region can reach it.
    OfAnotherRegion {
        /// The region the ARN names.
        region: &'a str,
        /// The queue name it names in that region.
        name: &'a str,
    },
    /// A well-formed ARN, but not one of this account's Amazon SQS queues: no resource
    /// this emulator holds can answer to it.
    Elsewhere,
    /// Not an ARN that names a queue at all.
    Malformed,
}

/// Classify the ARN a request supplied, as the region the request addressed sees it.
///
/// A queue ARN is `arn:<partition>:sqs:<region>:<accountId>:<QueueName>`, and a queue
/// name "can have up to 80 characters" (`CreateQueueRequest$QueueName`), so a value with
/// another segment count, an empty segment or a longer last segment names no queue in any
/// account. The service and account segments are what single out this account's queues,
/// and the REGION segment is what says which namespace they are in: a queue belongs to the
/// region its create addressed, so an ARN naming another region names a queue no call
/// served here can resolve — which is what keeps a dead-letter link and a message movement
/// task inside one region.
pub fn parse_queue_arn<'a>(arn: &'a str, region: &str) -> QueueArn<'a> {
    let Some((service, named_region, account, name)) = queue_arn_fields(arn) else {
        return QueueArn::Malformed;
    };
    if service != ARN_SERVICE || account != ::engine::emulator_account_id() {
        return QueueArn::Elsewhere;
    }
    if named_region != region {
        return QueueArn::OfAnotherRegion {
            region: named_region,
            name,
        };
    }
    QueueArn::OfThisRegion(name)
}

/// The `(service, region, account, resource)` fields of `arn` when it has the SHAPE of a
/// queue ARN, and `None` when it has not.
///
/// A queue ARN is `arn:<partition>:sqs:<region>:<accountId>:<QueueName>`, and a queue name
/// "can have up to 80 characters" (`CreateQueueRequest$QueueName`), so a value with another
/// segment count, an empty segment or a longer last segment names no queue in any account.
/// The one place those segments are read, so every reader of a queue ARN agrees on what
/// one is.
fn queue_arn_fields(arn: &str) -> Option<(&str, &str, &str, &str)> {
    let segments: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, name] = segments[..] else {
        return None;
    };
    if prefix != "arn"
        || partition.is_empty()
        || service.is_empty()
        || region.is_empty()
        || account.is_empty()
        || name.is_empty()
        || name.chars().count() > MAX_QUEUE_NAME_LENGTH
    {
        return None;
    }
    Some((service, region, account, name))
}

/// The key the store holds the queue a queue ARN of this account names under — the
/// `(region, QueueName)` pair the ARN itself carries — or `None` for any other value.
///
/// For a reader holding only an ARN and no request of its own (another service resolving a
/// queue a configuration names): an ARN names the region its queue lives in, so it is
/// resolved THERE rather than in some caller's region.
pub fn queue_key_from_arn(arn: &str) -> Option<(String, String)> {
    let (service, region, account, name) = queue_arn_fields(arn)?;
    (service == ARN_SERVICE && account == ::engine::emulator_account_id())
        .then(|| queue_key(region, name))
}

/// The `QueueName` a queue ARN of this account addresses in `region`, or `None` for any
/// other value — another account's, another service's, another region's, or one that is
/// no queue ARN at all.
pub fn queue_name_from_arn<'a>(arn: &'a str, region: &str) -> Option<&'a str> {
    match parse_queue_arn(arn, region) {
        QueueArn::OfThisRegion(name) => Some(name),
        QueueArn::OfAnotherRegion { .. } | QueueArn::Elsewhere | QueueArn::Malformed => None,
    }
}

/// The message every `QueueDoesNotExist` refusal carries, whatever the request named.
///
/// Amazon SQS answers this error with ONE fixed string — "The specified queue does not
/// exist." — for every operation and every queue: the error CODE
/// (`AWS.SimpleQueueService.NonExistentQueue` / `QueueDoesNotExist`, from the model's
/// `awsQueryError`) is what identifies the condition, and the message is a constant a
/// client can match on. It deliberately does not name the queue, so a caller probing for
/// a queue it may not address is never handed the name it probed back.
pub const QUEUE_DOES_NOT_EXIST_MESSAGE: &str = "The specified queue does not exist.";

/// The modelled refusal for a `QueueUrl` or `QueueName` no queue of this account answers
/// to, carrying [`QUEUE_DOES_NOT_EXIST_MESSAGE`] verbatim.
///
/// Takes no argument on purpose: nothing of the request may reach this payload, so no
/// caller can word a message of its own that echoes a url, a name or an owner.
pub fn queue_does_not_exist() -> QueueDoesNotExist {
    QueueDoesNotExist {
        message: Some(QUEUE_DOES_NOT_EXIST_MESSAGE.to_string()),
    }
}

/// What Amazon SQS answers a `CreateQueue` for a name whose queue was just deleted: one
/// fixed sentence quoting the wait its own documentation states
/// ([`QUEUE_DELETED_RECENTLY_DOCUMENTED_WAIT`]), with nothing of the request in it.
pub fn queue_deleted_recently_message() -> String {
    format!(
        "You must wait {} seconds after deleting a queue before you can create another with the same name.",
        QUEUE_DELETED_RECENTLY_DOCUMENTED_WAIT.as_secs(),
    )
}

/// What Amazon SQS answers a `PurgeQueue` on the queue called `name` with while the one
/// before it is still deleting: the interval is part of the sentence, so the caller is
/// told how long the deletion can take, and the queue the request named is the only
/// value filled in.
pub fn purge_queue_in_progress_message(name: &str) -> String {
    format!(
        "Only one PurgeQueue operation on {name} is allowed every {} seconds.",
        PURGE_QUEUE_INTERVAL.as_secs(),
    )
}

/// What Amazon SQS answers for a value supplied where a receipt handle was expected that
/// is not a receipt handle at all: ONE fixed sentence, with nothing of the request in it,
/// so a client can match on it and no caller is handed its own value back.
pub const RECEIPT_HANDLE_IS_INVALID_MESSAGE: &str = "The input receipt handle is invalid.";

/// How the same refusal is worded for a handle that IS a receipt handle, minted by
/// another queue: the handle is named, because what the caller has to fix is which queue
/// it sent it to rather than the handle itself.
pub fn receipt_handle_of_another_queue_message(receipt_handle: &str) -> String {
    format!("The receipt handle {receipt_handle:?} is not valid for this queue.")
}

/// Where a receipt handle a queue cannot resolve came from, which is what decides how the
/// refusal is worded — and, for a handle of a queue that is gone, WHICH refusal answers
/// it ([`unresolvable_receipt_handle`]).
#[derive(Debug, Clone, Copy)]
pub enum ReceiptHandleOrigin<'a> {
    /// The queue addressed minted it, or no queue of this store did and it is not a
    /// receipt handle at all: either way the refusal is about the handle itself, so it
    /// names the class of fault and no queue.
    Unrecognised,
    /// Another queue that EXISTS minted it: it is a receipt handle, just not one of this
    /// queue's.
    AnotherQueue(&'a str),
    /// An incarnation that no longer exists minted it: the queue the handle names was
    /// deleted, whether or not a queue carries its name again.
    PreviousIncarnation,
}

/// The refusal a receipt handle the queue addressed cannot resolve earns, as the two
/// modelled errors every operation taking a `ReceiptHandle` declares.
///
/// A handle names the queue incarnation that minted it, so a handle of an incarnation that
/// is gone addresses a queue that does not exist — which is a different fault from a
/// handle that is merely not this queue's, and recovered from differently: the caller has
/// to receive again from the queue it has now, not retry or re-address.
#[derive(Debug)]
pub enum UnresolvableReceiptHandle {
    /// The queue the handle names is gone.
    QueueIsGone(QueueDoesNotExist),
    /// The handle is not a valid one for the queue addressed.
    HandleIsInvalid(ReceiptHandleIsInvalid),
}

/// Which refusal answers a receipt handle the queue addressed cannot resolve, so every
/// operation that takes one classifies it the same way.
pub fn unresolvable_receipt_handle(origin: ReceiptHandleOrigin<'_>) -> UnresolvableReceiptHandle {
    match origin {
        ReceiptHandleOrigin::PreviousIncarnation => {
            UnresolvableReceiptHandle::QueueIsGone(queue_does_not_exist())
        }
        ReceiptHandleOrigin::Unrecognised | ReceiptHandleOrigin::AnotherQueue(_) => {
            UnresolvableReceiptHandle::HandleIsInvalid(receipt_handle_is_invalid(origin))
        }
    }
}

/// The `Code` and `Message` a BATCH entry carrying a handle the queue addressed cannot
/// resolve fails with: a batch judges each entry on its own, so the entry carries the code
/// of the error the single-message operation answers for the same handle — the model's
/// `awsQueryError` code for that error shape.
pub fn unresolvable_receipt_handle_batch_failure(
    origin: ReceiptHandleOrigin<'_>,
) -> (&'static str, Option<String>) {
    match unresolvable_receipt_handle(origin) {
        UnresolvableReceiptHandle::QueueIsGone(error) => (QUEUE_DOES_NOT_EXIST_CODE, error.message),
        UnresolvableReceiptHandle::HandleIsInvalid(error) => {
            (RECEIPT_HANDLE_IS_INVALID_CODE, error.message)
        }
    }
}

/// The error code `QueueDoesNotExist` is reported under, from the model's `awsQueryError`
/// for that shape — which is what a batch result entry's `Code` carries.
pub const QUEUE_DOES_NOT_EXIST_CODE: &str = "AWS.SimpleQueueService.NonExistentQueue";

/// The error code `ReceiptHandleIsInvalid` is reported under, from the model's
/// `awsQueryError` for that shape.
pub const RECEIPT_HANDLE_IS_INVALID_CODE: &str = "ReceiptHandleIsInvalid";

/// The modelled refusal for a receipt handle this queue cannot resolve.
///
/// A handle is replaced by the next receive of the same message, so a superseded one is
/// invalid too: "You must provide the most recently received receipt handle when you
/// request to delete the message."
pub fn receipt_handle_is_invalid(origin: ReceiptHandleOrigin<'_>) -> ReceiptHandleIsInvalid {
    ReceiptHandleIsInvalid {
        message: Some(match origin {
            ReceiptHandleOrigin::AnotherQueue(receipt_handle) => {
                receipt_handle_of_another_queue_message(receipt_handle)
            }
            ReceiptHandleOrigin::Unrecognised | ReceiptHandleOrigin::PreviousIncarnation => {
                RECEIPT_HANDLE_IS_INVALID_MESSAGE.to_string()
            }
        }),
    }
}

/// The parameter a receipt handle is supplied for, as every operation that takes one
/// names it.
pub const RECEIPT_HANDLE_PARAMETER: &str = "ReceiptHandle";

/// Why a receipt handle a request carries names no message of the queue it was sent to.
///
/// The two are told apart because a caller recovers from them differently: a string that
/// is not a receipt handle at all is a request no retry can fix, while a handle this
/// queue no longer holds a message under means the message is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptHandleRefusal {
    /// The value is not formed as a receipt handle, so no receive can have minted it.
    Malformed,
    /// It is formed as one, but names no message this queue still holds: the message was
    /// deleted, purged or expired, or the handle is one this queue never minted.
    NoSuchMessage,
    /// This queue minted it for a message it STILL holds, and the lease it carried has
    /// moved on: a later receive of that message took it (FIFO only — see
    /// [`Queue::message_id_for_handle`]).
    Expired,
}

/// Whether `receipt_handle` is formed as a receipt handle at all — a handle is an opaque
/// token a receive handed a consumer, so what can be judged of a value is its FORM.
///
/// A handle this emulator minted is one [`Queue::mint_receipt_handle`] produced, and
/// another queue service's is a longer opaque token of the same alphabet, so both are
/// handles here: a consumer replaying one is told its MESSAGE is gone rather than that
/// what it holds is not a handle. A value shorter than one minted identifier, one holding
/// a character no opaque token does, or one longer than any handle a consumer can have
/// been handed is not one.
pub fn is_receipt_handle(receipt_handle: &str) -> bool {
    let length = receipt_handle.chars().count();
    (MINTED_IDENTIFIER_LENGTH..=MAX_RECEIPT_HANDLE_LENGTH).contains(&length)
        && receipt_handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '='))
}

/// The queue INCARNATION a receipt handle this service minted names, or `None` for a value
/// this service did not mint.
///
/// A minted handle is the opaque encoding of the incarnation that issued it, the token
/// identifying the receive and the queue's ARN ([`Queue::mint_receipt_handle`]) — the first
/// two both [`new_identifier`] values — so the queue a consumer's handle belongs to is read
/// out of the handle itself, and a value of any other form names no queue.
pub fn receipt_handle_incarnation(receipt_handle: &str) -> Option<String> {
    let decoded = decoded_opaque_token(receipt_handle)?;
    let mut fields = decoded.split(RECEIPT_HANDLE_FIELD_SEPARATOR);
    let incarnation = fields.next()?;
    let receive = fields.next()?;
    (is_minted_identifier(incarnation) && is_minted_identifier(receive))
        .then(|| incarnation.to_string())
}

/// Whether `value` is one of the opaque identifiers this service mints — the form
/// [`new_identifier`] produces, and nothing else.
fn is_minted_identifier(value: &str) -> bool {
    value.chars().count() == MINTED_IDENTIFIER_LENGTH && uuid::Uuid::parse_str(value).is_ok()
}

/// `payload` as one opaque token a caller is handed: the single encoded string Amazon SQS
/// hands out where a token identifies something it minted (a receipt handle, a message
/// movement task handle), rather than that payload in the clear.
///
/// The codec is the workspace's own base64 ([`aws_smithy_types::base64`]), whose alphabet
/// is inside the one [`is_receipt_handle`] accepts.
fn encoded_opaque_token(payload: &str) -> String {
    ::aws_smithy_types::base64::encode(payload)
}

/// The payload [`encoded_opaque_token`] put in `token`, or `None` for a value it did not
/// produce.
///
/// A request supplies the token, so the decode is bounded by the length a handle has
/// ([`MAX_RECEIPT_HANDLE_LENGTH`]) before anything is allocated from it.
fn decoded_opaque_token(token: &str) -> Option<String> {
    if token.len() > MAX_RECEIPT_HANDLE_LENGTH {
        return None;
    }
    String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()
}

/// What separates the fields a minted receipt handle's payload carries. It appears in none
/// of them, so the split is unambiguous.
const RECEIPT_HANDLE_FIELD_SEPARATOR: char = ' ';

/// The length of one opaque identifier this service mints: [`new_identifier`] produces the
/// hyphenated 36-character form, and nothing shorter is a receipt handle.
const MINTED_IDENTIFIER_LENGTH: usize = 36;

/// The longest value this service treats as a receipt handle. A handle is held by a
/// consumer and handed back, so this bounds what one refusal can be made to echo; it is
/// this emulator's bound, as the other `MAX_*` bounds here are.
const MAX_RECEIPT_HANDLE_LENGTH: usize = 1_024;

/// How a refusal states that the handle names no message whose visibility there is to
/// change. The value is echoed only because it is already known to be a handle, and so
/// bounded by the form one has.
pub fn no_message_for_receipt_handle_message(receipt_handle: &str) -> String {
    invalid_parameter_message(
        receipt_handle,
        RECEIPT_HANDLE_PARAMETER,
        "Message does not exist or is not available for visibility timeout change.",
    )
}

/// How a refusal states that the handle was valid and has been SUPERSEDED: the lease a
/// receive handed over moved on with a later receive of the same message, so the caller
/// has to act under the handle that receive minted rather than retry or re-address.
///
/// A parameter fault — the value supplied for `ReceiptHandle` is no longer one this queue
/// acts on — so it is worded in the one template this service refuses a parameter value
/// with, and the handle is the only value filled in.
pub fn receipt_handle_expired_message(receipt_handle: &str) -> String {
    invalid_parameter_message(
        receipt_handle,
        RECEIPT_HANDLE_PARAMETER,
        RECEIPT_HANDLE_EXPIRED_REASON,
    )
}

/// The reason clause [`receipt_handle_expired_message`] states.
pub const RECEIPT_HANDLE_EXPIRED_REASON: &str = "The receipt handle has expired.";

/// How a refusal states that the `VisibilityTimeout` a call supplied is outside the range
/// the parameter takes: the value the request carried, and the bounds the parameter
/// publishes ("Values range: 0 to 43200"), which are whole seconds.
///
/// One wording for both ends of the range, and for every operation that takes the
/// parameter as a value to apply to a message — the single-message call and one entry of
/// the batch form state the same fault about the same parameter.
pub fn visibility_timeout_out_of_range_message(requested: i64) -> String {
    let (min, max) = integer_bounds(VISIBILITY_TIMEOUT);
    invalid_parameter_message(
        &requested.to_string(),
        VISIBILITY_TIMEOUT,
        &format!("{VISIBILITY_TIMEOUT} must be an integer between {min} and {max}."),
    )
}

/// How a refusal states that the visibility asked for reaches past what the message has
/// left: "if you attempt to set the VisibilityTimeout to a value greater than the maximum
/// time left, Amazon SQS returns an error. Amazon SQS doesn't automatically recalculate
/// and increase the timeout to the maximum remaining time." The limit is the span one
/// receive may hide a message for, counted from that receive.
pub fn visibility_beyond_limit_message(requested: i64, limit: i64) -> String {
    invalid_parameter_message(
        &requested.to_string(),
        VISIBILITY_TIMEOUT,
        &format!("Total VisibilityTimeout for the message is beyond the limit [{limit} seconds]."),
    )
}

/// How many entries one batch request may carry.
///
/// "For Amazon SQS, the maximum number of entries you can include in a single
/// `SendMessageBatch`, `DeleteMessageBatch`, or `ChangeMessageVisibilityBatch` request is
/// 10" (`TooManyEntriesInBatchRequest`'s own documentation). The model puts no `@length`
/// on the entry lists, so every batch operation applies this bound itself.
pub const MAX_BATCH_ENTRIES: usize = 10;

/// How long a batch entry's `Id` may be: "This identifier can have up to 80 characters"
/// (`DeleteMessageBatchRequestEntry$Id`).
pub const MAX_BATCH_ENTRY_ID_LEN: usize = 80;

/// How a refusal of an empty batch states it: the operation's own request-entry shape
/// is what the request should have carried at least one of.
///
/// One wording for every batch operation, so two of them cannot drift on how they state
/// the same defect; `operation` is the operation's name, whose request entry shape is
/// `<operation>RequestEntry`.
pub fn empty_batch_message(operation: &str) -> String {
    format!("There should be at least one {operation}RequestEntry in the request.")
}

/// Whether `id` is a batch entry id the specification admits.
///
/// "The following characters are accepted: alphanumeric characters, hyphens(-), and
/// underscores (_)" and "this identifier can have up to 80 characters"
/// (`DeleteMessageBatchRequestEntry$Id`); an empty id names nothing, so it is not one
/// either. A value outside this is what `InvalidBatchEntryId` refuses.
pub fn is_valid_batch_entry_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_BATCH_ENTRY_ID_LEN
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

/// The per-entry error code for a batch entry that carries a required member with no
/// value at all: the member is there, so nothing is missing, but it holds nothing the
/// operation can act on.
pub const EMPTY_VALUE: &str = "EmptyValue";

/// How a batch result entry states that `member` of one entry carried no value: the
/// member is named by its PATH in the request — the operation's request-entry shape, the
/// entry's position in `Entries` counted from one, and the member — because a batch
/// carries several entries and the caller has to know which one to repair.
///
/// One wording for every batch operation, so two of them cannot drift on how they state
/// the same defect; `operation` is the operation's name, whose request entry shape is
/// `<operation>RequestEntry`, and `position` is one-based.
pub fn empty_batch_entry_value_message(operation: &str, position: usize, member: &str) -> String {
    format!("No Value Found for {operation}RequestEntry.{position}.{member}.")
}

/// The largest payload one batch request may carry, in bytes.
///
/// `SendMessageBatch`'s own documentation gives it as a FIXED service limit — "the
/// maximum allowed individual message size and the maximum total payload size (the sum
/// of the individual lengths of all of the batched messages)" — which is the largest
/// value [`MAXIMUM_MESSAGE_SIZE`] admits, so it is read from that attribute's documented
/// range rather than written twice. A queue's own (possibly lowered)
/// `MaximumMessageSize` bounds EACH message, not the sum of a batch's entries.
///
/// It is also the most message content ANY request of this service may legally carry,
/// batch or not, which is what the transport ceiling is built from
/// (`crate::service::max_request_body_bytes`): that ceiling sits above this figure, because
/// a request carrying MORE than it is one the operation measures and refuses with
/// `BatchRequestTooLong` rather than one the transport drops.
pub fn maximum_batch_payload_bytes() -> i64 {
    integer_bounds(MAXIMUM_MESSAGE_SIZE).1
}

// ---------------------------------------------------------------------------
// Parameter-level refusals
// ---------------------------------------------------------------------------
//
// This service's model shapes an error for an ATTRIBUTE fault — `InvalidAttributeName`,
// `InvalidAttributeValue` — and one for each batch fault, but a fault in a PARAMETER is
// carried by two codes it documents without shaping: one for a parameter the call
// requires and did not get, one for a parameter whose value it does not accept. The
// error channel owns those two codes and their variants
// ([`crate::scaffolded::Scaffolded::missing_parameter`] and
// [`crate::scaffolded::Scaffolded::invalid_parameter_value`]); the TEXT they carry is
// worded HERE, once, in the templates the service builds from the request, so no handler
// and no batch entry invents a second wording for the same class of refusal.

/// How a refusal states that `parameter` was absent. The one wording for that class:
/// [`crate::scaffolded::MissingParameter`] builds its payload from it, and a batch entry
/// reporting the condition per entry uses it too.
pub fn missing_parameter_message(parameter: &str) -> String {
    format!("The request must contain the parameter {parameter}.")
}

/// How a refusal states that `value`, supplied for `parameter`, is not one this service
/// accepts: the offending value, the parameter it was supplied for, and the rule it
/// broke. `reason` is a whole sentence and carries its own terminal period.
pub fn invalid_parameter_message(value: &str, parameter: &str, reason: &str) -> String {
    format!("Value {value} for parameter {parameter} is invalid. Reason: {reason}")
}

/// The same template for a refusal that does NOT name the value: the one to use where
/// the value is a string whose length is the very thing being refused, so echoing it
/// would hand an unbounded value back to the caller.
pub fn invalid_parameter_message_without_value(parameter: &str, reason: &str) -> String {
    format!("Value for parameter {parameter} is invalid. Reason: {reason}")
}

/// How a refusal states a rule the request broke as a whole rather than through one
/// named parameter. `reason` is a whole sentence and carries its own terminal period.
pub fn invalid_parameters_message(reason: &str) -> String {
    format!("One or more parameters are invalid. Reason: {reason}")
}

/// The same class of fault as [`invalid_parameters_message`], as a batch RESULT ENTRY
/// states it: the entry's own members could not be validated, so the sentence is about
/// the entry rather than about the request.
pub fn unvalidated_parameters_message(reason: &str) -> String {
    format!("One or more parameters cannot be validated. Reason: {reason}")
}

/// Why a message is larger than the queue will carry, as the reason of either sentence
/// above: the bound is the queue's own `MaximumMessageSize`, which applies to ONE message
/// whether it was sent on its own or as one entry of a batch.
pub fn message_too_long_reason(maximum: i64) -> String {
    format!("Message must be shorter than {maximum} bytes.")
}

/// The message system attribute names in the order the model DECLARES them, which is the
/// order a refusal lists the accepted set in.
///
/// Source: `smithy_query(query="shape", shape="MessageSystemAttributeName",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`.
/// It holds exactly the values the generated
/// `MESSAGE_SYSTEM_ATTRIBUTE_NAME_VALUES` carries — membership is still checked against
/// that generated set — but the generated one is sorted, and a published value set is
/// stated in the order it is published.
pub const MESSAGE_SYSTEM_ATTRIBUTE_NAMES: &[&str] = &[
    "All",
    "SenderId",
    "SentTimestamp",
    "ApproximateReceiveCount",
    "ApproximateFirstReceiveTimestamp",
    "SequenceNumber",
    "MessageDeduplicationId",
    "MessageGroupId",
    "AWSTraceHeader",
    "DeadLetterQueueSourceArn",
];

/// How a refusal states that the queue a message movement task names as its source is
/// not one it can move messages out of.
///
/// The sentence states the pre-condition the named source does not meet — the queue has
/// to be the dead-letter queue of another queue — and nothing else: the whole message is
/// fixed text, because the parameter whose value is refused is already named by the
/// parameter-refusal class this is answered as. The documentation sentence of the
/// service's `UnsupportedOperation` class ("This action is currently limited to
/// supporting message redrive from queues that are configured as dead-letter queues
/// (DLQs) ... only") belongs to that OTHER class and is not reused here.
pub const DEAD_LETTER_SOURCE_REQUIRED: &str =
    "Source queue must be configured as a Dead Letter Queue.";

/// How a refusal states that the source queue a message movement task names already has
/// one running: "only one active message movement task is supported per queue at any
/// given time", so the request is refused for the source arn it named rather than for the
/// kind of queue that arn is. Fixed text — the condition clears on its own, and nothing of
/// the request tells the caller anything the sentence does not.
pub const TASK_ALREADY_RUNNING: &str =
    "There is already a task running. Only one active task is allowed for a source queue \
     arn at a given time.";

/// How a refusal states that a message movement task was asked to move a queue's messages
/// onto the queue they are already in.
///
/// Fixed text: the two parameters that cannot be equal are the only thing the sentence is
/// about, and the ARN they both carry is the caller's own and tells it nothing it did not
/// send. The rule is a cross-member one the model does not carry — `SourceArn` and
/// `DestinationArn` are plain strings with no modelled relationship — so it is stated here
/// once, where every refusal wording of this service lives.
pub const SAME_SOURCE_AND_DESTINATION: &str =
    "Source queue arn and destination queue arn cannot be the same.";

/// How a refusal states that a message movement task was asked to move the messages of one
/// kind of queue onto the other kind.
///
/// A move carries a queue's messages across unchanged, so the destination has to be able to
/// hold them: only a FIFO queue carries the group and deduplication members a FIFO queue's
/// messages have, and only a standard queue takes a message carrying none. It is the same
/// rule a dead-letter association is held to — "the dead-letter queue of a FIFO queue must
/// also be a FIFO queue. Similarly, the dead-letter queue of a standard queue must also be a
/// standard queue" ([`DEAD_LETTER_TARGET_TYPE_REASON`]) — stated for the pair of queues this
/// call names. Fixed text: the rule is about the two KINDS and the ARNs the caller sent tell
/// it nothing it did not send.
pub const SAME_QUEUE_TYPE_REQUIRED: &str =
    "The source queue and destination queue must be of the same queue type.";

/// The reason a refusal gives for a parameter that does not apply to the KIND of queue
/// the call addresses: a FIFO-only member supplied to a standard queue, or a
/// per-message member a FIFO queue accepts only as a queue attribute.
pub const QUEUE_TYPE_PARAMETER_REASON: &str =
    "The request include parameter that is not valid for this queue type.";

/// How a refusal states that a FIFO send carried no deduplication token and the queue it
/// addresses derives none: "If you don't provide a MessageDeduplicationId and the queue
/// doesn't have ContentBasedDeduplication set, the action fails with an error"
/// (`SendMessageRequest$MessageDeduplicationId`).
pub const NO_DEDUPLICATION_ID: &str = "The queue should either have ContentBasedDeduplication \
                                       enabled or MessageDeduplicationId provided explicitly";

/// How a refusal states that a parameter's value is not one this service accepts, where
/// it names the parameter and nothing else: what that parameter accepts is published
/// with the parameter, not restated per call.
pub fn invalid_value_message(parameter: &str) -> String {
    format!("Invalid value for the parameter {parameter}.")
}

/// The same template with the rule the value broke stated after it, in the `Reason:`
/// clause every refusal of this service that HAS a rule to state carries.
///
/// A value refused for what ANOTHER attribute holds cannot be repaired from the refused
/// attribute's own published value set, so the rule that rejected the pair is the only
/// thing that tells the caller what to change. `reason` is a whole sentence and carries
/// its own terminal period.
pub fn invalid_value_message_with_reason(parameter: &str, reason: &str) -> String {
    format!("{} Reason: {reason}", invalid_value_message(parameter))
}

/// The rule the FIFO throughput limit and the deduplication scope have to satisfy
/// TOGETHER, as the reason a refusal of either half states: "The perMessageGroupId value
/// is allowed only when the value for DeduplicationScope is messageGroup"
/// ([`FIFO_THROUGHPUT_LIMIT`]'s own documentation), stated as the change the caller has
/// to make.
pub fn fifo_throughput_pair_reason() -> String {
    format!(
        "To set {FIFO_THROUGHPUT_LIMIT} to {FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP}, the \
         {DEDUPLICATION_SCOPE} must be {DEDUPLICATION_SCOPE_MESSAGE_GROUP}."
    )
}

/// Why a retry under a `ReceiveRequestAttemptId` can no longer be served, as the reason
/// of the refusal that turns it away: the token replays its batch only "if none of the
/// messages have been modified (deleted or had their visibility changes)"
/// (`ReceiveMessageRequest$ReceiveRequestAttemptId`), so once one of them was, the
/// receive that token names is not a retryable call any more.
pub const RECEIVE_ATTEMPT_NOT_RETRYABLE: &str = "The receiveRequestAttemptId is not valid \
                                                 anymore since the receive call is not \
                                                 retryable.";

/// The modelled refusal for an attribute value the call does not accept: it names the
/// attribute whose value was refused, which is all this refusal states.
pub fn invalid_attribute_value(attribute: &str) -> InvalidAttributeValue {
    InvalidAttributeValue {
        message: Some(invalid_value_message(attribute)),
    }
}

/// The modelled refusal for an attribute name the call does not accept: it names no
/// attribute of the value set that member carries, or none this call can set on the queue
/// it addresses.
///
/// One wording for every call that takes attribute names — a queue attribute map, a
/// queue attribute name list, or a message system attribute name list — so two
/// operations cannot drift on how they word the same defect. Amazon SQS reports every name
/// it does not HAVE for the call as an UNKNOWN one, whatever made it unacceptable: a name
/// outside the value set, one the service only ever reports and one the queue the call
/// addresses does not have at all — a FIFO-only attribute, or the designator of the FIFO
/// kind, on a standard queue — are all refused in this single sentence, with the name
/// exactly as the request spelled it.
///
/// An attribute the queue the call addresses DOES have and the call may not CHANGE is a
/// different class: see [`unmodifiable_attribute_value`].
pub fn unknown_attribute_name(name: &str) -> InvalidAttributeName {
    InvalidAttributeName {
        message: Some(format!("Unknown Attribute {name}.")),
    }
}

/// The rule a refusal states for a write that would change what KIND of queue an existing
/// queue is: "You can provide this attribute only during queue creation. You can't change
/// it for an existing queue" ([`FIFO_QUEUE_ATTRIBUTE`]'s own documentation), stated as the
/// change that is not supported, because that is what such a call asked for.
pub const QUEUE_TYPE_NOT_MODIFIABLE: &str = "Modifying queue type is not supported.";

/// The modelled refusal for an attribute the queue a call addresses HAS and only a create
/// may supply.
///
/// Such an attribute is one that queue carries and reports: a call naming it has not named
/// an attribute that does not exist, it has asked for a change the attribute's value cannot
/// carry — so the refusal is the one for an attribute VALUE ([`invalid_attribute_value`]'s
/// class), in the same template, with the rule it broke stated in the `Reason:` clause every
/// refusal of this service that has a rule to state carries.
///
/// A queue that does NOT have the attribute is the other case: there the call named a name
/// outside that queue's own vocabulary ([`unknown_attribute_name`]).
pub fn unmodifiable_attribute_value(attribute: &str) -> InvalidAttributeValue {
    InvalidAttributeValue {
        message: Some(invalid_value_message_with_reason(
            attribute,
            QUEUE_TYPE_NOT_MODIFIABLE,
        )),
    }
}

/// Why an `Attributes` map was refused. Each handler renders it as its own modelled
/// error variant, so the wire code is the one that operation declares.
#[derive(Debug)]
pub enum AttributeRefusal {
    /// The map named an attribute the call does not have for the queue it addresses: one
    /// that does not exist, one no call sets, or one that queue does not carry at all.
    Name(InvalidAttributeName),
    /// The map gave an attribute that queue DOES have a value outside what the attribute
    /// accepts, or asked an attribute only a create supplies to change on an existing queue.
    Value(InvalidAttributeValue),
}

/// Check an `Attributes` map a caller supplied.
///
/// `on_create` is true for `CreateQueue`, which is the only call that may supply a
/// `create_only` attribute; `fifo` says whether the queue the map applies to is a FIFO
/// queue, which is what makes the FIFO-only attributes applicable. Names are visited
/// in order, so a map with two bad attributes is always refused for the same one.
///
/// A name the queue this call addresses does not HAVE is an UNKNOWN attribute whichever rule
/// put it out of reach ([`unknown_attribute_name`]): an attribute only `GetQueueAttributes`
/// reports, one that applies only to the other kind of queue, the designator of a kind the
/// addressed queue is not ([`AttributeKind::Designator`] — a standard queue carries no
/// attribute saying it is FIFO), and a designator given a value that designates nothing are
/// each simply not an attribute THAT queue has.
///
/// A `create_only` attribute of a queue that DOES have it is the one case that is not about
/// the name: the queue carries the attribute and reports it, so a later call naming it is
/// refused for the change its value asks for ([`unmodifiable_attribute_value`]). Which of
/// the two a call draws therefore follows the kind of queue it addresses, and the order the
/// checks below are written in is that rule: whether the queue HAS the attribute is settled
/// before whether this call may change it.
pub fn validate_attributes(
    supplied: &HashMap<String, String>,
    on_create: bool,
    fifo: bool,
) -> Result<(), AttributeRefusal> {
    let mut names: Vec<&String> = supplied.keys().collect();
    names.sort();
    for name in names {
        let value = &supplied[name];
        if !QUEUE_ATTRIBUTE_NAME_VALUES.contains(&name.as_str()) {
            return Err(AttributeRefusal::Name(unknown_attribute_name(name)));
        }
        // Reported-but-not-settable: an attribute the queue record derives is one no call
        // has as a settable attribute at all.
        let Some(spec) = settable_attribute(name) else {
            return Err(AttributeRefusal::Name(unknown_attribute_name(name)));
        };
        // An attribute of the FIFO kind of queue on a queue that is not of that kind: a
        // modelled name the queue this call addresses does not have, whether it applies
        // only to FIFO queues or DESIGNATES one — a standard queue carries no attribute
        // saying it is FIFO, which is why a read of one reports none.
        if !applies_to_queue(spec, fifo) {
            return Err(AttributeRefusal::Name(unknown_attribute_name(name)));
        }
        // Create-only on a later call, for a queue that HAS the attribute: the queue
        // carries it and reports it, so this call has asked for a change the attribute's
        // value cannot carry, and the refusal is about the value rather than the name.
        if spec.create_only && !on_create {
            return Err(AttributeRefusal::Value(unmodifiable_attribute_value(name)));
        }
        check_attribute_value(spec, value)?;
    }
    Ok(())
}

/// Apply the attribute map a call supplied to a queue's stored attributes, with the two
/// rules that are about the VALUE rather than about the attribute.
///
/// * An attribute whose form admits text of any length is REMOVED by a value holding no
///   text ("You can remove the ... attribute by setting it to an empty string"), so a
///   cleared policy leaves the queue carrying no `Policy` at all rather than one that is
///   not a document.
/// * A `Policy` is stored in its normalised form
///   ([`authorization::normalised_policy`]), so every later read reports the principals
///   in the spelling AWS canonicalises them to.
/// * A redrive policy attribute is stored in its canonical rendering
///   ([`canonical_redrive_document`]), so every later read reports one document whatever
///   spacing, member order or quoting the caller wrote it with.
///
/// Both `CreateQueue` and `SetQueueAttributes` store through this, so the two cannot
/// disagree about what a supplied value means.
pub fn apply_attributes<'a>(
    attributes: &mut BTreeMap<String, String>,
    supplied: impl IntoIterator<Item = (&'a String, &'a String)>,
) {
    for (name, value) in supplied {
        match stored_attribute(name, value) {
            Some(stored) => attributes.insert(name.clone(), stored),
            None => attributes.remove(name),
        };
    }
}

/// One value as an attribute's wire text carries it: a string stays itself, and every other
/// JSON form is its own compact JSON (`30`, `true`, `{"maxReceiveCount":3}`).
///
/// An `Attributes` value is TEXT on this service's wire, while the same value is typed
/// wherever a resource model declares it — an `AWS::SQS::Queue`'s `VisibilityTimeout` is a
/// number and its `RedrivePolicy` an object. Several services need that conversion, so the
/// rule itself is stated once in the shared crate
/// ([`::service_common::json_wire_values::value_text`]) and named here, beside the attribute
/// vocabulary it belongs to, for the callers that hand this service an attribute map.
pub fn attribute_text(value: &::serde_json::Value) -> String {
    ::service_common::json_wire_values::value_text(value)
}

/// The form attribute `name` is STORED in when a call supplies `value` for it, or `None`
/// when that value removes the attribute. See [`apply_attributes`].
pub fn stored_attribute(name: &str, value: &str) -> Option<String> {
    let removable = settable_attribute(name)
        .is_some_and(|spec| matches!(spec.kind, AttributeKind::Json | AttributeKind::Text));
    if value.is_empty() && removable {
        return None;
    }
    if name == POLICY {
        return Some(authorization::normalised_policy(value));
    }
    if let Some((_, numeric_members)) = CANONICAL_JSON_ATTRIBUTES
        .iter()
        .find(|(attribute, _)| *attribute == name)
    {
        return Some(canonical_redrive_document(value, numeric_members));
    }
    Some(value.to_string())
}

/// Why the attribute set a call would LEAVE a queue carrying cannot hold together, or
/// `None` when it can.
///
/// Some attributes are each individually valid yet constrain one another, so the pair
/// can only be judged over the resulting set rather than over the map one call supplies:
/// the per-message-group FIFO throughput limit "is allowed only when the value for
/// DeduplicationScope is messageGroup".
pub fn attribute_combination_refusal(
    attributes: &BTreeMap<String, String>,
) -> Option<InvalidAttributeValue> {
    let per_message_group = attributes
        .get(FIFO_THROUGHPUT_LIMIT)
        .is_some_and(|value| value == FIFO_THROUGHPUT_LIMIT_PER_MESSAGE_GROUP);
    let message_group_scope = attributes
        .get(DEDUPLICATION_SCOPE)
        .is_some_and(|value| value == DEDUPLICATION_SCOPE_MESSAGE_GROUP);
    (per_message_group && !message_group_scope).then(|| InvalidAttributeValue {
        // The fixed text Amazon SQS words a refused attribute value in, instantiated with
        // the attribute whose value the pair rule rejects, and carrying the `Reason:`
        // clause stating that rule — without it the refusal names no way to repair the
        // request, because the value is outside nothing the named attribute itself
        // publishes. The attribute named is the one the rule is stated ON, whichever half
        // of the pair the call supplied: it is the value that is only conditionally
        // allowed, and the scope is what has to change for it.
        message: Some(invalid_value_message_with_reason(
            FIFO_THROUGHPUT_LIMIT,
            &fifo_throughput_pair_reason(),
        )),
    })
}

/// Check one attribute value against what its own documentation says it accepts.
fn check_attribute_value(spec: &AttributeSpec, value: &str) -> Result<(), AttributeRefusal> {
    // An attribute whose form admits text of any length is also the kind a caller REMOVES
    // by supplying no text for it: "You can remove the ... attribute by setting it to an
    // empty string", so the empty value is accepted and stored as the attribute's absence
    // ([`stored_attribute`]) rather than refused as a malformed document.
    if value.is_empty() && matches!(spec.kind, AttributeKind::Json | AttributeKind::Text) {
        return Ok(());
    }
    // Every attribute value is stored for the queue's lifetime, so the two kinds whose
    // form admits text of any length are bounded FIRST: a caller looping on
    // `CreateQueue`/`SetQueueAttributes` must not be able to grow this store without
    // end (see "Store bounds" above). The ceiling is the one AWS publishes for the
    // largest of them — "The policy size is limited to 8,192 bytes" (the `Policy`
    // attribute's own documentation) — applied to each of them.
    let bounded = match spec.kind {
        AttributeKind::Json | AttributeKind::Text => value.len() <= MAX_ATTRIBUTE_VALUE_BYTES,
        _ => true,
    };
    if !bounded {
        return Err(AttributeRefusal::Value(invalid_attribute_value(spec.name)));
    }
    let accepted = match spec.kind {
        AttributeKind::Integer { min, max } => match value.parse::<i64>() {
            Ok(parsed) => (min..=max).contains(&parsed),
            Err(_) => false,
        },
        AttributeKind::Boolean => value == "true" || value == "false",
        AttributeKind::OneOf(allowed) => allowed.contains(&value),
        AttributeKind::Json => ::serde_json::from_str::<::serde_json::Value>(value).is_ok(),
        AttributeKind::Text => true,
        AttributeKind::Designator(designating) => value == designating,
    };
    if accepted {
        return Ok(());
    }
    // The refusal names the attribute and nothing else: what that attribute accepts is
    // published with the attribute, not restated per call.
    Err(match spec.kind {
        // A designator carries the one value that designates and nothing else, so a value
        // that designates nothing is not a value of this attribute to repair: the
        // attribute the request named is one the service does not have for it, and that is
        // what the refusal states.
        AttributeKind::Designator(_) => AttributeRefusal::Name(unknown_attribute_name(spec.name)),
        _ => AttributeRefusal::Value(invalid_attribute_value(spec.name)),
    })
}

/// Why a `QueueName` was refused, as the text of the refusal a caller has to fix the
/// request from, or `None` when the name is one this service accepts.
///
/// The rule is the member's own: "A queue name can have up to 80 characters. Valid
/// values: alphanumeric characters, hyphens (-), and underscores (_). A FIFO queue
/// name must end with the .fifo suffix." `fifo` says whether the request asks for a
/// FIFO queue, which is what makes the suffix part of the rule — and makes the refusal
/// state the whole rule a FIFO name has to satisfy, suffix included.
pub fn queue_name_refusal(name: &str, fifo: bool) -> Option<String> {
    // The suffix is part of a FIFO queue's name and of nothing else: a standard queue's
    // name may hold only the characters the member lists, so a `.fifo` name is accepted
    // for a FIFO queue and refused for a standard one.
    let body = if fifo {
        name.strip_suffix(FIFO_QUEUE_NAME_SUFFIX).unwrap_or(name)
    } else {
        name
    };
    let well_formed = (1..=MAX_QUEUE_NAME_LENGTH).contains(&name.chars().count())
        && !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let suffixed = !fifo || name.ends_with(FIFO_QUEUE_NAME_SUFFIX);
    if well_formed && suffixed {
        return None;
    }
    // The refusal states the rule, not which half of it this name broke: a caller fixes
    // the name against the whole rule either way.
    Some(if fifo {
        format!(
            "The name of a FIFO queue can only include alphanumeric characters, hyphens, or \
             underscores, must end with {FIFO_QUEUE_NAME_SUFFIX} suffix and be 1 to \
             {MAX_QUEUE_NAME_LENGTH} in length."
        )
    } else {
        format!(
            "Can only include alphanumeric characters, hyphens, or underscores. 1 to \
             {MAX_QUEUE_NAME_LENGTH} in length"
        )
    })
}

/// How this service refuses a call that would create, change or remove a tag in the
/// reserved [`RESERVED_TAG_KEY_PREFIX`] namespace: those tags belong to the provider, so
/// the refusal is about who the caller is rather than about the key's form, and it names
/// neither the key nor the operation.
pub const SYSTEM_TAG_REFUSAL: &str = "Caller is an end user and not allowed to mutate system tags";

/// Why a tag KEY a caller named is not one this service takes, as the text of the refusal
/// a caller has to fix the request from, or `None` when the key is acceptable.
///
/// The rule holds wherever a request NAMES a tag key — the `tags` of a create, the `Tags`
/// of a `TagQueue`, the `TagKeys` of an `UntagQueue` — so all of them refuse the same key
/// the same way: a key of 1 to [`MAX_TAG_KEY_LENGTH`] characters, and no key in the
/// reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, which only the provider writes.
///
/// Each refusal STATES THE RULE and carries nothing of the request: the rejected key is
/// not echoed back, and the bounds are the published ones rather than a measurement of
/// what was supplied.
pub fn tag_key_refusal(key: &str) -> Option<String> {
    let length = key.chars().count();
    if length == 0 || length > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "Tag keys must be between 1 and {MAX_TAG_KEY_LENGTH} characters in length."
        ));
    }
    if is_reserved_tag_key(key) {
        return Some(SYSTEM_TAG_REFUSAL.to_string());
    }
    None
}

/// Why a tag VALUE a caller supplied is not one this service takes, or `None` when the
/// value is acceptable: a value carries up to [`MAX_TAG_VALUE_LENGTH`] characters and may
/// be empty, so the rule the refusal states spans zero to that bound. Like
/// [`tag_key_refusal`], it carries nothing of the request.
pub fn tag_value_refusal(value: &str) -> Option<String> {
    (value.chars().count() > MAX_TAG_VALUE_LENGTH).then(|| {
        format!("Tag values must be between 0 and {MAX_TAG_VALUE_LENGTH} characters in length.")
    })
}

/// Why a `tags` map a caller supplied was refused, as the text of the refusal a caller
/// has to fix the request from, or `None` when the map is acceptable.
///
/// `existing` is the tag map the queue carries today, so the count is checked against
/// what the call would LEAVE the queue with: a key the request repeats overwrites the
/// existing tag rather than adding one. `queue` names the queue the call addresses, the
/// way a refusal of its tag count states it. Keys are visited in order, so a map with
/// two bad tags is always refused for the same one.
///
/// The rules are the tag restrictions the `tags` member's own documentation points at:
/// at most [`MAX_TAGS_PER_QUEUE`] tags per queue, a key of 1 to
/// [`MAX_TAG_KEY_LENGTH`] characters, a value of up to [`MAX_TAG_VALUE_LENGTH`], and no
/// key in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
pub fn tag_refusal(
    queue: &str,
    existing: &BTreeMap<String, String>,
    supplied: &HashMap<String, String>,
) -> Option<String> {
    let mut keys: Vec<&String> = supplied.keys().collect();
    keys.sort();
    for key in &keys {
        if let Some(why) = tag_key_refusal(key) {
            return Some(why);
        }
        if let Some(why) = tag_value_refusal(&supplied[*key]) {
            return Some(why);
        }
    }
    let merged: BTreeSet<&String> = existing.keys().chain(supplied.keys()).collect();
    if merged.len() > MAX_TAGS_PER_QUEUE {
        return Some(format!("Too many tags added for queue {queue}."));
    }
    None
}

/// Why `CreateQueue` must refuse a request that names an EXISTING queue, as the text of
/// the `QueueNameExists` refusal, or `None` when the request asks for nothing different
/// about that queue and is therefore idempotent.
///
/// "If you specify the name of an existing queue and provide the exact same names and
/// values for all its attributes, the CreateQueue action will return the URL of the
/// existing queue instead of creating a new one. If you attempt to create a queue with a
/// name that already exists but with different attribute names or values, the CreateQueue
/// action will return an error" — and `QueueNameExists` itself: "Amazon SQS returns this
/// error only if the request includes attributes whose values differ from those of the
/// existing queue."
///
/// `requested` is the attribute map the request would leave the queue with (the
/// documented defaults overlaid with what it supplied), compared attribute by attribute
/// in name order so a request differing in two is always refused for the same one. EVERY
/// settable attribute is part of that comparison, including the queue's access policy —
/// see [`attribute_value_unchanged`] for how each one is compared. `tags` is the tag map
/// the request supplied, and `None` when it supplied no `tags` member at all: tags carry
/// no default, so a request that supplies none includes nothing about them, while one
/// that supplies them has to name the set the queue already holds.
///
/// The two refusals are worded apart because a tag is not an attribute: an attribute
/// conflict is repaired with `SetQueueAttributes` and a tag conflict with `TagQueue`, so a
/// caller can tell them apart from the sentence alone.
pub fn recreate_refusal(
    existing: &Queue,
    requested: &BTreeMap<String, String>,
    tags: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    let compared: BTreeSet<&String> = existing
        .attributes
        .keys()
        .chain(requested.keys())
        .collect();
    for name in compared {
        if !attribute_value_unchanged(
            name,
            existing.attributes.get(name).map(String::as_str),
            requested.get(name).map(String::as_str),
        ) {
            return Some(format!(
                "A queue already exists with the same name and a different value for attribute {name}"
            ));
        }
    }
    if tags.is_some_and(|supplied| *supplied != existing.tags) {
        return Some(RECREATE_TAGS_CONFLICT_MESSAGE.to_string());
    }
    None
}

/// How `CreateQueue` states that a re-create of an existing name supplied a different TAG
/// SET. Fixed text that names no attribute, because tags are not one, and carries nothing
/// of the request or of the queue.
pub const RECREATE_TAGS_CONFLICT_MESSAGE: &str =
    "A queue already exists with the same name and a different value for tags ";

/// Whether the value a re-create supplies for attribute `name` is the one the queue
/// already carries, each side `None` where that side carries the attribute at all.
///
/// Every attribute is compared on its VALUE, which for all but one of them is the string
/// it is stored as. The access policy is a JSON document, so it is compared on what it
/// GRANTS: two documents saying the same thing about the same principals, actions and
/// resources are the same policy however they are spelled, and a statement's `Sid` is not
/// part of it, since an id names a statement without changing who may do what. A `Policy`
/// value that is not a JSON document at all cannot be read that way and is compared
/// verbatim.
fn attribute_value_unchanged(name: &str, existing: Option<&str>, requested: Option<&str>) -> bool {
    match (name, existing, requested) {
        (POLICY, Some(existing), Some(requested)) => {
            match (comparable_policy(existing), comparable_policy(requested)) {
                (Some(existing), Some(requested)) => existing == requested,
                _ => existing == requested,
            }
        }
        _ => existing == requested,
    }
}

/// A policy document as the content a comparison is about: its members with `Statement`
/// normalised to a list — "an array of individual statements", and the policy language
/// takes a lone statement object too — and each statement's `Sid` dropped. `None` for a
/// value that is not a JSON object, which is not a policy document and carries no
/// statements to compare.
fn comparable_policy(policy: &str) -> Option<::serde_json::Value> {
    let mut document = match ::serde_json::from_str(policy) {
        Ok(::serde_json::Value::Object(members)) => members,
        _ => return None,
    };
    let statements = match document.get(POLICY_STATEMENT_MEMBER) {
        Some(::serde_json::Value::Array(statements)) => statements.clone(),
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement.clone()],
        _ => Vec::new(),
    };
    let statements: Vec<::serde_json::Value> = statements
        .into_iter()
        .map(|mut statement| {
            if let ::serde_json::Value::Object(members) = &mut statement {
                members.remove(POLICY_SID_MEMBER);
            }
            statement
        })
        .collect();
    document.insert(
        POLICY_STATEMENT_MEMBER.to_string(),
        ::serde_json::Value::Array(statements),
    );
    Some(::serde_json::Value::Object(document))
}

/// The region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope (an unsigned call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The modelled refusal every handler answers with when [`service_state`] finds no
/// store (this service mounted on an engine that holds none).
///
/// `UnsupportedOperation` ("Error code 400. Unsupported operation.") is the only
/// refusal every one of these operations models that says "this emulator will not
/// serve the call" rather than "your request was malformed"; the payload is shared and
/// each operation wraps it in its OWN generated error variant.
pub fn store_unavailable() -> UnsupportedOperation {
    UnsupportedOperation {
        message: Some("the Amazon SQS queue store is unavailable".to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSqsAmazonsqsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSqsAmazonsqsState>)
}

// ---------------------------------------------------------------------------
// Message content: digests, sizes and identifiers
// ---------------------------------------------------------------------------

/// A non-negative second count as a [`Duration`]; a negative count is zero.
pub fn seconds(value: i64) -> Duration {
    Duration::from_secs(u64::try_from(value).unwrap_or(0))
}

/// `at` plus `value` seconds. Total: `SystemTime`'s `Add` PANICS on overflow and every
/// instant computed here derives from a request value, so overflow yields `at` (the
/// unreachable case — every span this is called with is a validated attribute bounded
/// at 43 200 seconds).
pub fn after(at: SystemTime, value: i64) -> SystemTime {
    at.checked_add(seconds(value)).unwrap_or(at)
}

/// Whether `interval` has run out, measured from `since`, by `now`.
///
/// An instant so far in the future that adding the interval overflows has not lapsed —
/// the same reading the message and deduplication spans take of an unrepresentable
/// deadline.
pub fn lapsed(since: SystemTime, interval: Duration, now: SystemTime) -> bool {
    since
        .checked_add(interval)
        .is_some_and(|lapses| now >= lapses)
}

/// A fresh opaque identifier: a version-4 UUID, so two calls never collide. Used for a
/// `MessageId` and for the receipt handle each receive mints.
pub fn new_identifier() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// MD5 hex digest of `body` — the value SQS returns as `MD5OfMessageBody`/`MD5OfBody`.
pub fn md5_hex(body: &[u8]) -> String {
    hex::encode(Md5::digest(body))
}

/// The deduplication token a queue with `ContentBasedDeduplication` derives for a
/// message: "Amazon SQS uses a SHA-256 hash to generate the MessageDeduplicationId
/// using the body of the message (but not the attributes of the message)", so two sends
/// of identical content inside the interval are "treated as duplicates and only one
/// copy of the message is delivered".
pub fn content_deduplication_token(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}

/// The parts of ONE attribute the validation and the digest below read.
///
/// `MessageAttributeValue` (what a sender puts in `MessageAttributes`) and
/// `MessageSystemAttributeValue` (what it puts in `MessageSystemAttributes`) declare
/// exactly the same members, so both are measured, judged and digested through this one
/// borrowed view: the rules are written once and a send cannot apply them to one map and
/// not the other.
#[derive(Debug, Clone, Copy)]
pub struct AttributeParts<'a> {
    /// The attribute's `DataType`, custom label included.
    pub data_type: &'a str,
    /// Its `StringValue`, when it carries one.
    pub string_value: Option<&'a str>,
    /// Its `BinaryValue`, when it carries one.
    pub binary_value: Option<&'a [u8]>,
    /// Which of the two list members it carries, when it carries one.
    pub list_member: Option<&'static str>,
}

/// Which list member an attribute carries, for the two shapes that declare both.
fn list_member(strings: bool, binaries: bool) -> Option<&'static str> {
    [
        strings.then_some("StringListValues"),
        binaries.then_some("BinaryListValues"),
    ]
    .into_iter()
    .flatten()
    .next()
}

impl<'a> From<&'a MessageAttributeValue> for AttributeParts<'a> {
    fn from(value: &'a MessageAttributeValue) -> Self {
        Self {
            data_type: value.data_type.as_str(),
            string_value: value.string_value.as_deref(),
            binary_value: value.binary_value.as_deref(),
            list_member: list_member(
                value.string_list_values.is_some(),
                value.binary_list_values.is_some(),
            ),
        }
    }
}

impl<'a> From<&'a MessageSystemAttributeValue> for AttributeParts<'a> {
    fn from(value: &'a MessageSystemAttributeValue) -> Self {
        Self {
            data_type: value.data_type.as_str(),
            string_value: value.string_value.as_deref(),
            binary_value: value.binary_value.as_deref(),
            list_member: list_member(
                value.string_list_values.is_some(),
                value.binary_list_values.is_some(),
            ),
        }
    }
}

/// MD5 hex digest of the message-attribute encoding SQS defines, or `None` when there
/// are no attributes (SQS omits `MD5OfMessageAttributes` then).
pub fn md5_of_message_attributes(
    attributes: &BTreeMap<String, MessageAttributeValue>,
) -> Option<String> {
    md5_of_attribute_encoding(attributes)
}

/// The same digest over the message SYSTEM attributes, which is what
/// `MD5OfMessageSystemAttributes` reports: the encoding is defined on an attribute's
/// name, type and value, and a system attribute has all three.
pub fn md5_of_message_system_attributes(
    attributes: &BTreeMap<String, MessageSystemAttributeValue>,
) -> Option<String> {
    md5_of_attribute_encoding(attributes)
}

/// MD5 hex digest of the attribute encoding SQS defines, or `None` for an empty map.
///
/// The encoding is Amazon SQS's own, documented as "Calculating the MD5 message digest
/// for message attributes" in the Amazon SQS Developer Guide, and no crate implements
/// it, so it is written here over the `md-5` crate's digest: attributes are taken in
/// name order, and each contributes
///
/// 1. its name, length-prefixed with a 4-byte big-endian length,
/// 2. its `DataType`, length-prefixed the same way,
/// 3. one transport-type byte — `1` for a `StringValue`, `2` for a `BinaryValue`,
/// 4. the value itself, length-prefixed the same way.
///
/// `StringListValues`/`BinaryListValues` contribute NOTHING: the model documents both as
/// "Not implemented. Reserved for future use.", so SQS defines no transport type for
/// them and a send refuses a message that carries one rather than guessing a byte.
fn md5_of_attribute_encoding<'a, V>(attributes: &'a BTreeMap<String, V>) -> Option<String>
where
    AttributeParts<'a>: From<&'a V>,
{
    if attributes.is_empty() {
        return None;
    }
    let mut encoded = Vec::new();
    for (name, value) in attributes {
        let parts = AttributeParts::from(value);
        append_length_prefixed(&mut encoded, name.as_bytes());
        append_length_prefixed(&mut encoded, parts.data_type.as_bytes());
        if let Some(string_value) = parts.string_value {
            encoded.push(1);
            append_length_prefixed(&mut encoded, string_value.as_bytes());
        } else if let Some(binary_value) = parts.binary_value {
            encoded.push(2);
            append_length_prefixed(&mut encoded, binary_value);
        }
    }
    Some(md5_hex(&encoded))
}

/// Append `bytes` preceded by its length as a 4-byte big-endian integer.
fn append_length_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The bytes a message-attribute map holds: each attribute's name, data type and value.
///
/// Amazon SQS counts message attributes toward a message's size ("All parts of the
/// message attribute, including Name, Type, and Value, are part of the message size
/// restriction"), so they are counted here too, both against the queue's
/// `MaximumMessageSize` and against this emulator's retained-byte budgets.
pub fn message_attribute_bytes(attributes: &BTreeMap<String, MessageAttributeValue>) -> usize {
    attribute_bytes(attributes)
}

/// The bytes a message-SYSTEM-attribute map holds, measured the same way.
///
/// These do NOT count toward a message's size — "the size of a message system attribute
/// doesn't count towards the total size of a message"
/// (`SendMessageRequest$MessageSystemAttributes`) — so this is used only for this
/// emulator's retained-byte budgets, which bound every byte a sender can make the store
/// hold.
pub fn message_system_attribute_bytes(
    attributes: &BTreeMap<String, MessageSystemAttributeValue>,
) -> usize {
    attribute_bytes(attributes)
}

/// The bytes an attribute map holds: each attribute's name, data type and value.
fn attribute_bytes<'a, V>(attributes: &'a BTreeMap<String, V>) -> usize
where
    AttributeParts<'a>: From<&'a V>,
{
    attributes
        .iter()
        .map(|(name, value)| {
            let parts = AttributeParts::from(value);
            name.len()
                .saturating_add(parts.data_type.len())
                .saturating_add(parts.string_value.map_or(0, str::len))
                .saturating_add(parts.binary_value.map_or(0, <[u8]>::len))
        })
        .fold(0usize, usize::saturating_add)
}

/// The longest a message attribute name may be. Amazon SQS Developer Guide, message
/// attribute components: the name "can contain up to 256 characters" — so 256 is taken
/// and 257 refused, even though the refusal Amazon SQS words for it says "shorter than
/// 256 bytes".
pub const MAX_MESSAGE_ATTRIBUTE_NAME_LEN: usize = 256;

/// The prefixes a message attribute name may not start with, in any casing: they are
/// "reserved for use by Amazon Web Services" (Amazon SQS Developer Guide, message
/// attribute components). Held lower-cased, because the rule ignores casing.
const RESERVED_MESSAGE_ATTRIBUTE_PREFIXES: [&str; 2] = ["aws.", "amazon."];

/// The logical data types a message attribute may carry: "Amazon SQS supports the
/// following logical data types: String, Number, and Binary"
/// (`MessageAttributeValue$DataType`), each optionally followed by a custom label ("You
/// can also append custom labels").
pub const MESSAGE_ATTRIBUTE_DATA_TYPES: [&str; 3] = ["String", "Number", "Binary"];

/// How many digits of precision the `Number` data type holds: "a number can have up to
/// 38 digits of precision" (Amazon SQS Developer Guide, message attribute data types).
pub const MAX_NUMBER_PRECISION_DIGITS: usize = 38;

/// Where a `Number` value's decimal point may fall: the type "must be within the range
/// -10^128 to 10^126" (the same entry), so the point stands at most 127 digits past the
/// first one and at most 128 before it. It also bounds how many zeros writing the value
/// out plainly can take.
pub const NUMBER_DECIMAL_POINT_RANGE: (i64, i64) = (-128, 127);
