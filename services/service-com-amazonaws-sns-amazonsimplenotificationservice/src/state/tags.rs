// SPDX-License-Identifier: CC0-1.0

//! The rules a topic's TAG set is held to: how many tags and how many bytes one topic
//! holds, which keys a caller may write, and the sentence each refusal states.
//!
//! One place, because the three operations that write a topic's tags and the one that
//! reports them cannot disagree about what a topic may hold.

use super::*;

/// The most tags one topic holds: "Can't add more than 50 tags to a topic"
/// ([`TagLimitExceededException`](crate::ops::types::TagLimitExceededException) doc).
///
/// Shared by every operation that writes a topic's tags, so a topic cannot hold a
/// different number depending on which call put them there.
pub const MAX_TAGS_PER_TOPIC: usize = 50;

/// The sentence SNS refuses a write that would take a resource past
/// [`MAX_TAGS_PER_TOPIC`] with: the quota that is exhausted, with neither the count
/// held nor the count asked for in it. Shared by every operation that writes a
/// topic's tags, so one condition reads the same whichever call carried the write.
pub const TAG_QUOTA_EXCEEDED_REFUSAL: &str =
    "Could not complete request: tag quota of per resource exceeded";

/// The message a `ResourceArn` of one of SNS's TAG operations is refused with, or
/// `None` when the value is a topic ARN.
///
/// Two rules, in order, and both are ARGUMENT validation rather than a lookup — a
/// value that could not name a topic this endpoint serves is bad input, so it is judged
/// before the store is consulted:
///
/// 1. it has to be an ARN of this endpoint's own region ([`regional_arn_refusal`]);
/// 2. it has to be an ARN of an Amazon SNS TOPIC. Every tag operation documents its
///    `ResourceArn` as "The ARN of the topic …", and SNS holds tags on topics alone,
///    so an ARN of another service, or an SNS ARN whose resource part is not a topic
///    name (a platform endpoint's `endpoint/GCM/app/…`, a subscription's
///    `<topic>:<uuid>`), names nothing a tag operation could ever act on.
///
/// Neither clause reproduces the value the caller sent: both name the member and the
/// constraint it failed, as every other refusal of this service does.
pub fn tagged_topic_arn_refusal(member: &str, value: &str, region: &str) -> Option<String> {
    if let Some(message) = regional_arn_refusal(member, value, region) {
        return Some(message);
    }
    topic_name_from_arn(value).is_none().then(|| {
        format!(
            "Invalid parameter: {member} Reason: An SNS tag is held by a topic, so the \
             ARN must name an Amazon SNS topic"
        )
    })
}

/// The refusal a topic's tag map is held to: `None` when `tags` is a map the topic may
/// hold, a message when it is not.
///
/// Both ceilings are checked, in the order a caller fixes them: the documented
/// [`MAX_TAGS_PER_TOPIC`] entry COUNT, then this emulator's own
/// [`MAX_TAG_BYTES_PER_TOPIC`] byte ceiling on the one caller-sized map a topic record
/// carries. The first is the modelled `TagLimitExceededException`'s sentence; the
/// second is an `InvalidParameterException`'s — so the caller is told which it crossed.
pub enum TagRefusal {
    /// Too many entries: the documented per-topic tag limit.
    TooMany(String),
    /// Too many bytes: this emulator's per-record ceiling.
    TooLarge(String),
}

/// The refusal for the tag map a write WOULD leave a topic holding, or `None` when
/// the topic may hold it. Measured on the resulting map, so a caller cannot grow one
/// record past either ceiling by repeating a call with ever larger values.
pub fn tag_map_refusal(tags: &HashMap<String, String>) -> Option<TagRefusal> {
    if tags.len() > MAX_TAGS_PER_TOPIC {
        return Some(TagRefusal::TooMany(TAG_QUOTA_EXCEEDED_REFUSAL.to_string()));
    }
    let held = attribute_bytes(tags);
    (held > MAX_TAG_BYTES_PER_TOPIC).then(|| {
        TagRefusal::TooLarge(format!(
            "Invalid parameter: Tags - a topic holds at most {MAX_TAG_BYTES_PER_TOPIC} \
             bytes of Tags: {held}"
        ))
    })
}

/// The key prefix Amazon Web Services keeps for the tags IT puts on a resource, which a
/// caller's own tag key may therefore not begin with.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Compared without regard to case, and by BYTES rather than characters, so a key whose
/// first bytes are a multi-byte character is simply not reserved instead of splitting a
/// character boundary.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Who named the tag keys one request carries, which is what decides whether a key in the
/// namespace [`RESERVED_TAG_KEY_PREFIX`] reserves may be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKeyOrigin {
    /// A request a CLIENT made: the `aws:` namespace is reserved for Amazon Web Services
    /// use, so a key in it is refused.
    Caller,
    /// A request another emulated service made in-process while provisioning a resource
    /// on a caller's behalf — the path the tags AWS applies from inside itself arrive on
    /// (the stack provenance a provisioning service records on every resource it
    /// creates). A key in the reserved namespace travels on it.
    Service,
}

/// Where the tag keys this request names came from.
///
/// Told apart by where the request CAME FROM and by nothing in it, through the crate's one
/// reading of that ([`super::authorization::is_in_process_call`]): the framework stamps a
/// request a handler sent through its in-process callback with that chain's depth, and a
/// request that arrived from a client carries none. The stamp is a typed request extension of
/// this process, so nothing a client can send sets it.
pub fn tag_key_origin(ctx: &::smithy_server_runtime::OperationContext<'_>) -> TagKeyOrigin {
    if super::authorization::is_in_process_call(ctx) {
        TagKeyOrigin::Service
    } else {
        TagKeyOrigin::Caller
    }
}

/// The sentence a tag key under the reserved prefix is refused with, or `None` when every
/// key of `keys` is one a request of this `origin` may write.
///
/// `aws:`-prefixed keys are the namespace Amazon Web Services writes its own (system and
/// stack) tags in, so a CALLER cannot create, update or delete one: a client request
/// naming such a key is refused rather than applied beside the service's own, while the
/// in-process provisioning path that records them keeps writing them. Shared by every
/// operation that writes a topic's tag set — the two that add tags and the one that
/// removes them — so the reserved namespace is the same whichever call carried the
/// write. The sentence states the condition and names the member the keys arrived in,
/// and nothing of the request: the caller named the keys it sent.
pub fn reserved_tag_key_refusal<'a, I>(
    member: &str,
    keys: I,
    origin: TagKeyOrigin,
) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    if origin == TagKeyOrigin::Service {
        return None;
    }
    keys.into_iter().any(is_reserved_tag_key).then(|| {
        format!(
            "Invalid parameter: {member} Reason: Caller is an end user and not allowed \
             to mutate system tags"
        )
    })
}
