// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkmeetings-chimemeetingssdkservice service.
//!
//! The meeting is this service's primary resource and its attendees live INSIDE it, so
//! `CreateMeeting`, `GetMeeting`, `DeleteMeeting`, `CreateAttendee`, `GetAttendee`,
//! `ListAttendees` and `DeleteAttendee` all read and write the one [`StoredMeeting`] map
//! held here. Nesting the attendees under their meeting is what makes
//! `DeleteMeeting` remove them transitively and makes it impossible for
//! `ListAttendees`/`GetAttendee` to reach another meeting's attendees.
//!
//! The derivations beside the map — the GUID minting, the `MediaPlacement` endpoints (in
//! [`media_endpoints`]), the ARN, the documented capability defaults and the documented
//! attendee ceiling — live here rather than in a handler because several operation files
//! share them, and a second copy of any of them would let two operations disagree about one
//! meeting.
//!
//! # Every store here is bounded
//!
//! This process authenticates nobody, so a caller that reaches it must not be able to
//! grow its memory without limit:
//!
//! | store | ceiling | release |
//! |---|---|---|
//! | [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::meetings`] | [`MAX_MEETINGS`] | `DeleteMeeting`, or `CreateMeeting` is refused |
//! | [`StoredMeeting::attendees`] | [`attendee_ceiling`] per meeting | `DeleteAttendee`/`DeleteMeeting`, or `CreateAttendee` is refused |
//! | [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::meeting_by_client_request_token`] | one entry per live meeting | the entry is dropped with its meeting |
//! | the record of ended meetings | [`MAX_ENDED_MEETINGS`] | the oldest id is forgotten when the next meeting ends |
//!
//! One thing this service reads is NOT in the struct below, because the other Chime
//! services read and write it too and the engine keys state by type: the family's
//! ARN-keyed TAG directory ([`chime_tags`]), which holds the tags of every meeting —
//! including the ones a create was given.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use crate::ops::types::{
    Attendee, AttendeeCapabilities, BadRequestException, ForbiddenException,
    LimitExceededException, MediaPlacement, Meeting, MeetingFeaturesConfiguration,
    NotFoundException, NotificationsConfiguration, ResourceNotFoundException,
    ServiceFailureException, TooManyTagsException, TranscriptionConfiguration,
};

/// The addresses a meeting's `MediaPlacement` announces, which every projection of a
/// meeting composes for the request that reads it.
mod media_endpoints;
pub use media_endpoints::*;

/// The verdict the three generic tagging operations share: whether a `ResourceARN` may be
/// addressed at all, and which refusal it earns when it may not.
mod tagging;
pub use tagging::*;

/// The `arnNamespace` this service's ARNs carry (`OPERATIONS.md`, `## Wire protocol`).
pub const ARN_NAMESPACE: &str = "chime";

/// The resource TYPE a meeting's ARN carries (`…:chime:<region>:<account>:meeting/<id>`,
/// as [`meeting_arn`] derives it). It is the one resource type this service mints, so it
/// is the one an ARN has to carry for this store to say whether the resource exists.
pub const MEETING_RESOURCE_TYPE: &str = "meeting";

/// How many tags one resource of THIS service carries at once: the bound its own model
/// declares on `TagList` (`@length(min: 0, max: 50)`,
/// `smithy_query(query="shape",shape="TagList")`), which is the family's usual one — so
/// the shared directory's default bound is this service's bound too.
pub const MAX_TAGS_PER_RESOURCE: usize = ::service_common::chime_tags::MAX_TAGS_PER_RESOURCE;

// The values of the model's `MediaCapabilities` enum
// (`smithy_query(query="shape",shape="MediaCapabilities")` -> `SendReceive | Send |
// Receive | None`). Named once here so the capability rules below read as the
// documentation states them.
pub const CAPABILITY_SEND_RECEIVE: &str = "SendReceive";
pub const CAPABILITY_SEND: &str = "Send";
pub const CAPABILITY_RECEIVE: &str = "Receive";
pub const CAPABILITY_NONE: &str = "None";

/// The `None` member of both `VideoResolution` and `ContentResolution`.
pub const RESOLUTION_NONE: &str = "None";

/// The resolution a meeting's video features carry when the request names none.
///
/// From the model: `VideoFeatures$MaxResolution` documents "Defaults to `HD`."
pub const DEFAULT_VIDEO_MAX_RESOLUTION: &str = "HD";

/// The resolution a meeting's content features carry when the request names none.
///
/// From the model: `ContentFeatures$MaxResolution` documents "Defaults to `FHD`."
pub const DEFAULT_CONTENT_MAX_RESOLUTION: &str = "FHD";

/// The `VideoResolution` value that reduces a meeting's attendee ceiling.
pub const VIDEO_RESOLUTION_FHD: &str = "FHD";

/// The `ContentResolution` value that reduces a meeting's attendee ceiling.
pub const CONTENT_RESOLUTION_UHD: &str = "UHD";

/// How many attendees a meeting admits when it names no `MeetingFeatures.Attendee.MaxCount`
/// and asks for neither `Video.MaxResolution: FHD` nor `Content.MaxResolution: UHD`.
///
/// From the model: `AttendeeFeatures` documents "the maximum number of attendees changes
/// from the default of `250` to `25`" in those two cases, and `AttendeeMax` is
/// `@range(1..250)`.
pub const DEFAULT_MAX_ATTENDEES: usize = 250;

/// How many attendees a meeting admits when it asks for `Video.MaxResolution: FHD` or
/// `Content.MaxResolution: UHD` without naming its own `Attendee.MaxCount`
/// (`AttendeeFeatures`, same sentence as [`DEFAULT_MAX_ATTENDEES`]).
pub const REDUCED_MAX_ATTENDEES: usize = 25;

/// The page size `ListAttendees` serves when the request names no `MaxResults`: the
/// maximum the model allows that member (`ListAttendeesRequest$MaxResults`, range 1..100).
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The most meetings this emulator holds at once.
///
/// `meetings` grows by one record per `CreateMeeting` with a fresh `ClientRequestToken`
/// and shrinks only on `DeleteMeeting`, so without a ceiling its size is set by how many
/// requests a caller makes. AWS bounds the same thing with an adjustable per-account
/// service quota that is not in the model, so this number is the EMULATOR's own bound
/// rather than a claim about AWS's — while the refusal it produces is the error the model
/// declares for exactly this case: `LimitExceededException` (HTTP 400, "The request
/// exceeds the resource limit.").
pub const MAX_MEETINGS: usize = 250;

/// How many ENDED meetings this emulator remembers having ended.
///
/// A meeting that was deleted is reported differently from an id that never named one
/// (see [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::meeting_absent`]), so
/// the ids of deleted meetings have to be remembered — and that record, unlike the
/// meetings themselves, is never released by a client call. It is therefore a bounded
/// FIFO: the oldest id is forgotten when the next one is recorded, after which that id
/// reads as one that never named a meeting again. The bound is the EMULATOR's own, chosen
/// to match [`MAX_MEETINGS`] so a caller that deletes every meeting it created still sees
/// every one of them reported as ended.
pub const MAX_ENDED_MEETINGS: usize = MAX_MEETINGS;

// The failures these operations share. Each handler raises them through its OWN
// operation's error enum (the variants a model declares differ per operation), but the
// payload inside is the same refusal, and a payload built inline in seven places drifts
// in wording. These constructors are the one place each payload's `Code` and message are
// chosen. `RequestId` is deliberately left unset: it identifies one AWS request, and this
// emulator mints no request ids.

/// `BadRequestException`'s payload: the request broke a rule the service enforces but the
/// model cannot express, so the boundary validator passed it.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: Some("BadRequest".to_string()),
        message: Some(message.into()),
        request_id: None,
    }
}

/// The prefix the model RESERVES in the two caller-supplied identifiers of this service.
///
/// From the model: `CreateAttendeeRequest$ExternalUserId` and
/// `CreateMeetingRequest$ExternalMeetingId` both document "Values that begin with `aws:`
/// are reserved. You can't configure a value that uses this prefix. Case insensitive."
/// (`smithy_query(query="doc",shape="CreateAttendeeRequest",member="ExternalUserId")`) —
/// which is why [`reserved_prefix_refusal`] compares it without regard to case.
pub const RESERVED_IDENTIFIER_PREFIX: &str = "aws:";

/// How a request naming the reserved prefix in `member` is refused.
///
/// The sentence is FIXED text for this error class apart from the INPUT MEMBER it names,
/// which is what tells a caller which of the identifiers it supplied was the offending
/// one; no value of the request is in it — the prefix it quotes is the reserved one as the
/// documentation writes it, not the casing the request happened to use.
pub fn reserved_prefix_problem(member: &str) -> String {
    format!("Validation error for {member}, prefix '{RESERVED_IDENTIFIER_PREFIX}' is reserved.")
}

/// `None` when `value` may be stored under `member`, or the refusal when it begins with
/// the prefix the model reserves.
///
/// The comparison is CASE-INSENSITIVE, as the documentation of both members states ("Case
/// insensitive."), so the reserved namespace cannot be entered by changing the casing of
/// the prefix. Only the beginning of the value is compared: one that merely CONTAINS the
/// prefix is admitted, because the rule is about what a value BEGINS with.
///
/// One function for every caller-supplied identifier the rule is documented on, so the
/// attendee's and the meeting's external ids cannot enforce it differently.
pub fn reserved_prefix_refusal(member: &str, value: &str) -> Option<BadRequestException> {
    value
        .get(..RESERVED_IDENTIFIER_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_IDENTIFIER_PREFIX))
        .then(|| bad_request(reserved_prefix_problem(member)))
}

/// The input member of `CreateAttendee` the external-identifier rules are documented on.
pub const EXTERNAL_USER_ID_MEMBER: &str = "ExternalUserId";

/// The input member of `CreateMeeting` the same rules are documented on.
pub const EXTERNAL_MEETING_ID_MEMBER: &str = "ExternalMeetingId";

/// The characters the model ADMITS in the two caller-supplied identifiers of this
/// service, written as the documentation writes them.
///
/// From the model: `CreateAttendeeRequest$ExternalUserId`, `Attendee$ExternalUserId` and
/// `CreateMeetingRequest$ExternalMeetingId` all document "Pattern:
/// `[-_&@+=,(){}\[\]\/«».:|'"#a-zA-Z0-9À-ÿ\s]*`" in the same paragraph as the reserved
/// prefix above
/// (`smithy_query(query="doc",shape="CreateAttendeeRequest",member="ExternalUserId")`).
/// NEITHER shape carries a Smithy `@pattern` trait (both are `string, length 2..64,
/// sensitive`), so nothing enforces the pattern before a handler and
/// [`external_identifier_pattern_refusal`] is what applies it.
pub const EXTERNAL_IDENTIFIER_PATTERN: &str = r##"[-_&@+=,(){}\[\]\/«».:|'"#a-zA-Z0-9À-ÿ\s]*"##;

/// Whether `candidate` is one of the characters [`EXTERNAL_IDENTIFIER_PATTERN`] admits.
///
/// The arms are that character class read left to right: the punctuation it lists
/// literally (its escapes `\[`, `\]` and `\/` stand for the brackets and the slash), the
/// two guillemets, the ASCII letters and digits, the `À-ÿ` Latin-1 range, and `\s`, whose
/// Unicode definition is [`char::is_whitespace`]. The class is CLOSED: a character it
/// does not list is not admitted.
fn admitted_identifier_char(candidate: char) -> bool {
    matches!(candidate,
        '-' | '_' | '&' | '@' | '+' | '=' | ',' | '(' | ')' | '{' | '}' | '[' | ']' | '/'
            | '«' | '»' | '.' | ':' | '|' | '\'' | '"' | '#'
            | 'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | 'À'..='ÿ'
    ) || candidate.is_whitespace()
}

/// How a request carrying a character the documented pattern excludes in `member` is
/// refused.
///
/// The sentence is FIXED text for this error class apart from the INPUT MEMBER it names —
/// which tells the caller which of the identifiers it supplied was the offending one —
/// and the pattern, quoted as the documentation writes it, which tells it what is
/// admitted. No value of the request is in it: both members are `@sensitive`, so a
/// refusal never echoes one back.
pub fn pattern_problem(member: &str) -> String {
    format!(
        "Validation error for {member}, value must match the pattern \
         '{EXTERNAL_IDENTIFIER_PATTERN}'."
    )
}

/// `None` when every character of `value` is one [`EXTERNAL_IDENTIFIER_PATTERN`] admits,
/// or the refusal when any character is outside it.
///
/// The whole value is compared, so one out-of-pattern character anywhere in it — first,
/// inside or last — refuses the request.
pub fn external_identifier_pattern_refusal(
    member: &str,
    value: &str,
) -> Option<BadRequestException> {
    (!value.chars().all(admitted_identifier_char)).then(|| bad_request(pattern_problem(member)))
}

/// `None` when `value` may be stored under `member`, or the refusal for the first rule of
/// the documented paragraph it breaks.
///
/// Both sentences of that paragraph constrain the value — the character pattern and the
/// reserved prefix — and both are the same modelled refusal, so every operation that
/// takes one of these identifiers applies them through this ONE function rather than
/// picking a sentence. The pattern is read first, in the order the documentation states
/// the two rules; the reserved prefix is itself inside the pattern, so a value that
/// enters the reserved namespace with admitted characters is still reported as reserved.
pub fn external_identifier_refusal(member: &str, value: &str) -> Option<BadRequestException> {
    external_identifier_pattern_refusal(member, value)
        .or_else(|| reserved_prefix_refusal(member, value))
}

/// How a tagging call naming a `MeetingId` this service holds no meeting under is refused.
///
/// A meeting's tags live exactly as long as the meeting, so an ARN whose meeting this
/// store does not hold — one that never named a meeting here, or one that has ended —
/// addresses no resource and the refusal reports the id in it as invalid. Entirely fixed
/// text: it interpolates nothing from the request.
pub const INVALID_MEETING_ID_MESSAGE: &str = "Invalid meeting id";

/// `BadRequestException`'s payload for a tagging call whose `ResourceARN` carries a
/// `MeetingId` this store holds no meeting under.
pub fn invalid_meeting_id() -> BadRequestException {
    bad_request(INVALID_MEETING_ID_MESSAGE)
}

/// How this service words every `NotFoundException`: the kind of resource the request
/// named, then the id it supplied for that resource in angle brackets, then `not found`.
///
/// The message is built from the REQUEST — the id in it is the one the caller sent, never
/// a value from this store — so a client that logs or surfaces the message can see which
/// of the ids it passed was the absent one. One function so every reader of a missing
/// meeting (`GetMeeting`, `ListAttendees`, `CreateAttendee`, `GetAttendee`,
/// `DeleteAttendee`) words the refusal identically. An `AttendeeId` absent from a meeting
/// that EXISTS is not this refusal: see [`attendee_not_in_meeting`].
fn not_found(kind: &str, id: &str) -> NotFoundException {
    NotFoundException {
        code: Some("NotFound".to_string()),
        message: Some(format!("{kind} <{id}> not found")),
        request_id: None,
    }
}

/// `NotFoundException`'s payload for a well-formed id that names no stored meeting.
pub fn meeting_not_found(meeting_id: &str) -> NotFoundException {
    not_found("Meeting", meeting_id)
}

/// `NotFoundException`'s payload for a meeting that EXISTED and has since been deleted.
///
/// `DeleteMeeting` is documented as disconnecting every client and preventing new ones
/// from joining — it ENDS the meeting — so a later caller is told that outcome rather
/// than that its id is unknown: the id did name a meeting, and no retry or correction of
/// it will ever succeed. The message names no id because it reports a fact about the
/// meeting's lifecycle, not a value from the request.
pub fn meeting_ended() -> NotFoundException {
    NotFoundException {
        code: Some("NotFound".to_string()),
        message: Some("The meeting has ended".to_string()),
        request_id: None,
    }
}

/// `ForbiddenException`'s payload wording `message`: the refusal for access that is
/// revoked for good rather than for a resource the request could not name
/// (`ForbiddenException` is documented "The client is permanently forbidden from making
/// the request").
pub fn forbidden(message: impl Into<String>) -> ForbiddenException {
    ForbiddenException {
        code: Some("Forbidden".to_string()),
        message: Some(message.into()),
        request_id: None,
    }
}

/// `ForbiddenException`'s payload for an `AttendeeId` that names no attendee of a LIVE
/// meeting — one that was never created there, or one `DeleteAttendee` has removed.
///
/// The refusal is the modelled `ForbiddenException` (HTTP 403, "The client is permanently
/// forbidden from making the request") rather than a 404: the meeting the attendee was
/// looked for in EXISTS, so access to that attendee's record is revoked rather than the
/// request naming an unknown resource, and a client reading 403 stops retrying the id and
/// re-joins instead. Every operation that resolves an `AttendeeId` inside a live meeting
/// (`GetAttendee`, `DeleteAttendee`, `UpdateAttendeeCapabilities`) raises THIS payload, so
/// the three cannot word one refusal three ways.
///
/// Both ids come from the REQUEST — never from this store — so a client that logs the
/// message sees the pair it asked about.
pub fn attendee_not_in_meeting(attendee_id: &str, meeting_id: &str) -> ForbiddenException {
    forbidden(format!(
        "Attendee <{attendee_id}> is not in the meeting <{meeting_id}>"
    ))
}

/// `ServiceFailureException`'s payload for a state store that could not be reached — the
/// one case a handler cannot serve from state at all.
pub fn store_unavailable() -> ServiceFailureException {
    ServiceFailureException {
        code: Some("ServiceFailure".to_string()),
        message: Some("the Chime SDK meeting store is unavailable".to_string()),
        request_id: None,
    }
}

/// `LimitExceededException`'s payload for a store already holding [`MAX_MEETINGS`].
pub fn meeting_limit_reached() -> LimitExceededException {
    LimitExceededException {
        code: Some("LimitExceeded".to_string()),
        message: Some(format!(
            "this account already holds the maximum of {MAX_MEETINGS} Amazon Chime SDK meetings"
        )),
        request_id: None,
    }
}

/// `LimitExceededException`'s payload for a meeting already at its attendee ceiling.
pub fn attendee_limit_reached(meeting_id: &str, ceiling: usize) -> LimitExceededException {
    LimitExceededException {
        code: Some("LimitExceeded".to_string()),
        message: Some(format!(
            "meeting {meeting_id} already holds its maximum of {ceiling} attendees"
        )),
        request_id: None,
    }
}

/// `ResourceNotFoundException`'s payload for a tagging call whose `ResourceARN` names no
/// resource this service holds. Read by all three generic tagging operations, so the
/// three cannot word one refusal three ways. `ResourceName` carries the ARN the call
/// named, which is what that member documents ("The name of the resource that couldn't be
/// found").
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        code: Some("NotFound".to_string()),
        message: Some(format!("resource {resource_arn} does not exist")),
        request_id: None,
        resource_name: Some(resource_arn.to_string()),
    }
}

/// `TooManyTagsException`'s payload for a tagging call that would take a resource past
/// [`MAX_TAGS_PER_RESOURCE`] tags; `requested` is how many tags the resource would have
/// ended up with.
pub fn too_many_tags(resource_arn: &str, requested: usize) -> TooManyTagsException {
    TooManyTagsException {
        code: Some("TooManyTags".to_string()),
        message: Some(format!(
            "{requested} tags were requested for {resource_arn}, which is more than the \
             maximum of {MAX_TAGS_PER_RESOURCE} tags per resource"
        )),
        request_id: None,
        resource_name: Some(resource_arn.to_string()),
    }
}

/// Which operation created a meeting, and so which operation's `ClientRequestToken`
/// record names it.
///
/// `ClientRequestToken` is `@idempotencyToken` on `CreateMeetingRequest` and, separately,
/// on `CreateMeetingWithAttendeesRequest`: a token is the retry marker of the call it was
/// sent to, and the two calls do not produce the same thing — `CreateMeeting` creates a
/// meeting "with no initial attendees", `CreateMeetingWithAttendees` a meeting together
/// with the 1..20 attendees its `@required` `Attendees` list carries. So one call's record
/// cannot stand for the other's: replaying it would answer a request for attendees with a
/// meeting that holds none, creating none of them and reporting no refusal for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeetingCreateCall {
    /// `CreateMeeting`.
    Meeting,
    /// `CreateMeetingWithAttendees`.
    MeetingWithAttendees,
}

/// One Amazon Chime SDK meeting as this service persists it: the modelled [`Meeting`] it
/// hands back, plus the request members the `Meeting` shape does not carry.
///
/// The `Meeting` record IS the stored value — not a parallel struct plus a conversion —
/// so a reader clones exactly what the writer stored and `GetMeeting` cannot drift from
/// what `CreateMeeting` returned.
///
/// `attendees` is a `BTreeMap` rather than a `HashMap` because `ListAttendees` is
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `NextToken` a page returns names the first attendee it did NOT return, so the next
/// call resumes there.
#[derive(Debug, Clone)]
pub struct StoredMeeting {
    /// The meeting record as stored, which every reader returns with its
    /// `MediaPlacement` composed for the reading request ([`Self::announced_meeting`]):
    /// the stored record carries no endpoints, because an address is only an address for
    /// the caller it is announced to.
    pub meeting: Meeting,
    /// The attendees of this meeting, keyed by `AttendeeId`.
    pub attendees: BTreeMap<String, Attendee>,
    /// `NotificationsConfiguration` as supplied: the targets AWS would publish meeting
    /// and attendee events to. Kept because it belongs to the meeting, and reported by no
    /// operation — the `Meeting` shape has no member for it.
    pub notifications_configuration: Option<NotificationsConfiguration>,
    /// The `MediaPlacementNetworkType` the meeting was created with. Kept for the same
    /// reason: it is part of the meeting, and the `Meeting` shape has no member for it.
    pub media_placement_network_type: Option<String>,
    /// The `ClientRequestToken` that created this meeting, so deleting the meeting can
    /// drop its idempotency record too.
    pub client_request_token: String,
    /// The create operation that minted this meeting. Together with
    /// [`Self::client_request_token`] it names this meeting's idempotency record, so
    /// deleting the meeting drops that record and nothing else's.
    pub created_by: MeetingCreateCall,
    /// The transcription currently running in this meeting, as the call that started it
    /// configured it, or `None` when none is running — which is what a meeting starts out
    /// with and what stopping leaves behind.
    ///
    /// It is nested in the meeting rather than kept in a map beside the meetings so that
    /// it cannot outlive the meeting it belongs to: deleting the meeting drops it, and no
    /// second record can disagree about whether this meeting is being transcribed.
    /// Reported by no operation — neither the `Meeting` shape nor any output shape of this
    /// model carries a transcription member.
    pub transcription: Option<TranscriptionConfiguration>,
}

impl StoredMeeting {
    /// The attendee of this meeting whose `ExternalUserId` is `external_user_id`, or
    /// `None` when the meeting holds none.
    ///
    /// `CreateAttendeeRequest$ExternalUserId` is documented as "An idempotency token.
    /// Links the attendee to an identity managed by a builder application", so an external
    /// identity has at most ONE attendee in a meeting and a repeated create resolves to
    /// the attendee the first call minted instead of a second one. The scan is over this
    /// meeting's attendees alone, so one meeting's external identity never resolves
    /// through another's.
    ///
    /// Answers the FIRST attendee in `AttendeeId` order, which is a total order over a
    /// `BTreeMap`'s keys, so the answer does not depend on how the map was filled.
    pub fn attendee_with_external_user_id(&self, external_user_id: &str) -> Option<&Attendee> {
        self.attendees.values().find(|attendee| {
            attendee.external_user_id.as_deref() == Some(external_user_id)
        })
    }
}

/// Per-service state for com-amazonaws-chimesdkmeetings-chimemeetingssdkservice. A handler reaches it through [`service_state`].
///
/// The meetings map is NOT partitioned by region: a meeting records the `MediaRegion` it
/// was created in and its ARN the Region of the endpoint that created it,
/// but any later call reaches every meeting in the store whatever region it is signed for.
/// Partitioning the control plane by region is a separate change; nothing here claims it.
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState {
    /// Every meeting that currently exists, keyed by its minted `MeetingId`.
    pub meetings: HashMap<String, StoredMeeting>,
    /// (the create operation, the `ClientRequestToken` it was given) -> the `MeetingId`
    /// that call created, so a replayed create returns the meeting its own first call
    /// created instead of minting a second one. Keyed by the operation as well as the
    /// token ([`MeetingCreateCall`]), because a retry marker is a marker for ONE
    /// operation's answer. Read and written through [`Self::replayed_meeting`] and
    /// [`Self::remember_meeting_create`]. One entry per live meeting: `DeleteMeeting`
    /// drops the meeting's entry with it, so this map is bounded by [`MAX_MEETINGS`] too.
    pub meeting_by_client_request_token: HashMap<(MeetingCreateCall, String), String>,
    /// The `MeetingId`s of meetings that EXISTED and have since been deleted, oldest
    /// first, so a reader can tell a meeting that ended from an id that never named one.
    /// Bounded by [`MAX_ENDED_MEETINGS`]: recording one past the bound drops the oldest.
    ended_meetings: VecDeque<String>,
    /// Counter the minted ids and join tokens are derived from. Never an id itself.
    next_seq: u64,
}

impl ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState {
    /// The meeting `call` created with this `ClientRequestToken`, or `None` when that
    /// operation has created none with it — which is what makes the call a fresh create
    /// rather than a replay.
    ///
    /// The lookup is scoped to the operation that recorded the token: a create answers a
    /// replay only from its OWN record, never from another create operation's.
    pub fn replayed_meeting(
        &self,
        call: MeetingCreateCall,
        client_request_token: &str,
    ) -> Option<&StoredMeeting> {
        let meeting_id = self
            .meeting_by_client_request_token
            .get(&(call, client_request_token.to_string()))?;
        self.meetings.get(meeting_id)
    }

    /// Record that `call` created `meeting_id` with `client_request_token`, which is what
    /// a later call of the SAME operation carrying that token replays.
    pub fn remember_meeting_create(
        &mut self,
        call: MeetingCreateCall,
        client_request_token: &str,
        meeting_id: &str,
    ) {
        self.meeting_by_client_request_token.insert(
            (call, client_request_token.to_string()),
            meeting_id.to_string(),
        );
    }

    /// Mint a `MeetingId` no stored meeting holds.
    ///
    /// The caller must insert the record under the returned id while still holding the
    /// same state lock, or the id it returned names nothing.
    pub fn mint_meeting_id(&mut self) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = guid_from(&format!(
                "chime-sdk-meetings/meeting/{seq}/{}",
                clock_nanos()
            ));
            // Neither a live meeting's id nor one recorded as ended, so the two records
            // stay disjoint and a fresh meeting is never read as an ended one.
            if !self.meetings.contains_key(&candidate) && !self.has_ended(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint an `AttendeeId` no attendee of `meeting_id` holds.
    pub fn mint_attendee_id(&mut self, meeting_id: &str) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = guid_from(&format!(
                "chime-sdk-meetings/attendee/{meeting_id}/{seq}/{}",
                clock_nanos()
            ));
            let taken = self
                .meetings
                .get(meeting_id)
                .is_some_and(|meeting| meeting.attendees.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// Mint the `JoinToken` an attendee authenticates with.
    ///
    /// `Attendee$JoinToken` is `@sensitive` and the shape's own documentation asks that it
    /// be transferred securely, so it must not be derivable from the ids a client already
    /// knows: it is a digest of those ids together with this store's counter and the
    /// wall clock, which a caller holding only the `MeetingId` and `AttendeeId` cannot
    /// reproduce. 64 hex characters, inside `JoinTokenString`'s modelled length 2..2048.
    pub fn mint_join_token(&mut self, meeting_id: &str, attendee_id: &str) -> String {
        let seq = self.next_seq();
        let digest = Sha256::digest(
            format!(
                "chime-sdk-meetings/join-token/{meeting_id}/{attendee_id}/{seq}/{}",
                clock_nanos()
            )
            .as_bytes(),
        );
        hex::encode(digest)
    }

    /// Drop a meeting and everything recorded against it, so nothing outlives the meeting
    /// it belongs to: its attendees (nested in the record) and its idempotency entry.
    ///
    /// Answers the removed record, or `None` when no meeting had that id.
    ///
    /// The id itself is remembered as ENDED, which is what lets every later reader of it
    /// report the meeting's outcome instead of an unknown id.
    pub fn forget_meeting(&mut self, meeting_id: &str) -> Option<StoredMeeting> {
        let removed = self.meetings.remove(meeting_id)?;
        self.meeting_by_client_request_token
            .remove(&(removed.created_by, removed.client_request_token.clone()));
        if !self.has_ended(meeting_id) {
            self.ended_meetings.push_back(meeting_id.to_string());
            while self.ended_meetings.len() > MAX_ENDED_MEETINGS {
                self.ended_meetings.pop_front();
            }
        }
        Some(removed)
    }

    /// Stop whatever transcription is running in `meeting_id`, leaving the meeting, its
    /// attendees and every other meeting exactly as they were.
    ///
    /// The configuration is DROPPED rather than kept beside a stopped flag: nothing
    /// reports a stopped transcription, and a later start supplies its own configuration.
    /// A meeting with nothing running, or an id this store holds no meeting under, is
    /// left untouched — stopping states the outcome "no transcription is running in this
    /// meeting", which already holds in both cases.
    pub fn stop_transcription(&mut self, meeting_id: &str) {
        if let Some(stored) = self.meetings.get_mut(meeting_id) {
            stored.transcription = None;
        }
    }

    /// Whether this id named a meeting this store has since deleted.
    pub fn has_ended(&self, meeting_id: &str) -> bool {
        self.ended_meetings.iter().any(|ended| ended == meeting_id)
    }

    /// The `NotFoundException` payload for a `MeetingId` this store holds no meeting for.
    ///
    /// Two situations reach it and a caller acts differently on each: a meeting that
    /// EXISTED and has ended (its media is gone, so the caller moves on to a new meeting),
    /// and an id that never named a meeting here (the caller has the wrong id). The
    /// distinction is the store's to make, so it is made once, here, for every operation
    /// that resolves a `MeetingId`.
    ///
    /// The caller must have established that `self.meetings` holds no record under
    /// `meeting_id`.
    pub fn meeting_absent(&self, meeting_id: &str) -> NotFoundException {
        if self.has_ended(meeting_id) {
            meeting_ended()
        } else {
            meeting_not_found(meeting_id)
        }
    }

    /// The next value of this store's monotonic id counter.
    fn next_seq(&mut self) -> u64 {
        self.next_seq = self.next_seq.wrapping_add(1);
        self.next_seq
    }

    /// Why a tagging call may NOT address `arn`, or `None` when it may.
    ///
    /// The family's one rule for which ARNs the shared tagging routes serve lives in
    /// [`::service_common::chime_tags::addresses_taggable_resource`]; this store answers
    /// the half of it only this service can — whether an ARN that names a resource THIS
    /// service mints (one of type [`MEETING_RESOURCE_TYPE`]) is one it still holds. The
    /// comparison is against the meeting's OWN ARN, so an ARN carrying a live meeting's id
    /// in another Region names no meeting here. An ARN of any other Chime resource type is
    /// another family member's, which this store cannot check.
    ///
    /// The two refusals differ because the caller's mistake does: an ARN of THIS service
    /// whose meeting the store does not hold carries an invalid `MeetingId`
    /// ([`TaggingRefusal::InvalidMeetingId`]), while a value that is no family resource ARN
    /// at all names nothing this service could resolve
    /// ([`TaggingRefusal::UnresolvedResource`]).
    pub fn tagging_refusal(&self, arn: &str) -> Option<TaggingRefusal> {
        // `Some(true)`/`Some(false)` when the ARN names a MEETING of this family, `None`
        // when it names another member's resource type or is no family ARN at all.
        let own_resource_exists = ::service_common::arn::is_resource_arn_of(
            arn,
            ::service_common::chime_tags::CHIME_ARN_NAMESPACE,
        )
        .then(|| ::service_common::arn::resource_id_of_type(arn, MEETING_RESOURCE_TYPE))
        .flatten()
        .map(|id| {
            self.meetings
                .get(id)
                .is_some_and(|stored| stored.meeting.meeting_arn.as_deref() == Some(arn))
        });
        if ::service_common::chime_tags::addresses_taggable_resource(arn, own_resource_exists) {
            return None;
        }
        if own_resource_exists == Some(false) {
            return Some(TaggingRefusal::InvalidMeetingId);
        }
        Some(TaggingRefusal::UnresolvedResource)
    }
}

/// Why a tagging call cannot address the `ResourceARN` it named, as
/// [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::tagging_refusal`] decides
/// it. The three tagging handlers map each variant to the error their own model declares
/// for it, so one ARN gets one verdict whichever of them read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggingRefusal {
    /// The ARN names a meeting of this service that the store holds no meeting under.
    InvalidMeetingId,
    /// The ARN names no resource of this family at all.
    UnresolvedResource,
}

/// The meeting's effective `MeetingFeatures.Video.MaxResolution`: what the request named,
/// or the documented default (`VideoFeatures$MaxResolution`, "Defaults to `HD`").
pub fn video_max_resolution(features: Option<&MeetingFeaturesConfiguration>) -> &str {
    features
        .and_then(|config| config.video.as_ref())
        .and_then(|video| video.max_resolution.as_deref())
        .unwrap_or(DEFAULT_VIDEO_MAX_RESOLUTION)
}

/// The meeting's effective `MeetingFeatures.Content.MaxResolution`: what the request
/// named, or the documented default (`ContentFeatures$MaxResolution`, "Defaults to
/// `FHD`").
pub fn content_max_resolution(features: Option<&MeetingFeaturesConfiguration>) -> &str {
    features
        .and_then(|config| config.content.as_ref())
        .and_then(|content| content.max_resolution.as_deref())
        .unwrap_or(DEFAULT_CONTENT_MAX_RESOLUTION)
}

/// How many attendees a meeting with these features admits.
///
/// From `AttendeeFeatures`: the ceiling is the meeting's own
/// `MeetingFeatures.Attendee.MaxCount` when it names one, else "the maximum number of
/// attendees changes from the default of `250` to `25`" if the meeting asked for
/// `Video.MaxResolution: FHD` or `Content.MaxResolution: UHD`, else 250.
pub fn attendee_ceiling(features: Option<&MeetingFeaturesConfiguration>) -> usize {
    let max_count = features
        .and_then(|config| config.attendee.as_ref())
        .and_then(|attendee| attendee.max_count);
    if let Some(max_count) = max_count {
        // `AttendeeMax` is `@range(1..250)`, so the boundary validator has already
        // refused anything outside it by the time a handler reads this.
        return usize::try_from(max_count).unwrap_or(DEFAULT_MAX_ATTENDEES);
    }
    if video_max_resolution(features) == VIDEO_RESOLUTION_FHD
        || content_max_resolution(features) == CONTENT_RESOLUTION_UHD
    {
        REDUCED_MAX_ATTENDEES
    } else {
        DEFAULT_MAX_ATTENDEES
    }
}

/// Whether a `MediaCapabilities` value asks to send or receive media at all.
fn sends_or_receives(capability: &str) -> bool {
    capability == CAPABILITY_SEND_RECEIVE
        || capability == CAPABILITY_SEND
        || capability == CAPABILITY_RECEIVE
}

/// Whether a `MediaCapabilities` value asks to RECEIVE media.
fn receives(capability: &str) -> bool {
    capability == CAPABILITY_SEND_RECEIVE || capability == CAPABILITY_RECEIVE
}

/// The capabilities an attendee created without any gets, for a meeting with `features`.
///
/// From `AttendeeCapabilities`: "If meeting features is defined as
/// `Video:MaxResolution:None` but `Content:MaxResolution` is defined as something other
/// than `None` and attendee capabilities are not defined in the API request, then the
/// default attendee video capability is set to `Receive` and attendee content capability
/// is set to `SendReceive`. This is because content `SendReceive` requires video to be at
/// least `Receive`." Every other meeting defaults all three to `SendReceive`.
///
/// A meeting whose CONTENT resolution is `None` is the case the documentation does not
/// name. Its default content capability is `None` here, because a default the service
/// picks has to be a value a request could have supplied — and
/// [`capability_problem`] refuses `Send`/`Receive`/`SendReceive` content for exactly
/// that meeting.
pub fn default_capabilities(features: Option<&MeetingFeaturesConfiguration>) -> AttendeeCapabilities {
    let video_none = video_max_resolution(features) == RESOLUTION_NONE;
    let content_none = content_max_resolution(features) == RESOLUTION_NONE;
    let content = if content_none {
        CAPABILITY_NONE
    } else {
        CAPABILITY_SEND_RECEIVE
    };
    let video = match (video_none, content_none) {
        // Documented: content `SendReceive` requires video to be at least `Receive`.
        (true, false) => CAPABILITY_RECEIVE,
        (true, true) => CAPABILITY_NONE,
        (false, _) => CAPABILITY_SEND_RECEIVE,
    };
    AttendeeCapabilities {
        audio: CAPABILITY_SEND_RECEIVE.to_string(),
        video: video.to_string(),
        content: content.to_string(),
    }
}

/// Why REQUEST-SUPPLIED `capabilities` cannot hold for a meeting with `features`, or
/// `None` when they can.
///
/// The three rules are the "corner cases" the model's `AttendeeCapabilities`
/// documentation lists, and each one is refused with HTTP 400 — the status both of its
/// sentences name:
///
/// 1. `Video:MaxResolution:None` — "all API requests that include `SendReceive`, `Send`,
///    or `Receive` for `AttendeeCapabilities:Video` will be rejected".
/// 2. `Content:MaxResolution:None` — the same for `AttendeeCapabilities:Content`.
/// 3. "You can't set `content` capabilities to `SendReceive` or `Receive` unless you also
///    set `video` capabilities to `SendReceive` or `Receive`."
///
/// Each of the three answers with the sentence the service serves for that error class —
/// fixed text naming the capability member and the rule, with nothing of the request
/// spliced into it, so one rule always reads the same however it was broken.
///
/// Every caller judges the set it was GIVEN before it resolves what the store already
/// holds for that identity: the documentation refuses every request that includes a
/// forbidden value ("all API requests that include …"), so the refusal follows from the
/// request and the meeting's features alone — never from whether the identity named
/// already has an attendee.
pub fn capability_problem(
    capabilities: &AttendeeCapabilities,
    features: Option<&MeetingFeaturesConfiguration>,
) -> Option<String> {
    if video_max_resolution(features) == RESOLUTION_NONE
        && sends_or_receives(&capabilities.video)
    {
        return Some(max_resolution_none_problem(
            VIDEO_CAPABILITY_MEMBER,
            VIDEO_FEATURE_MEMBER,
        ));
    }
    if content_max_resolution(features) == RESOLUTION_NONE
        && sends_or_receives(&capabilities.content)
    {
        return Some(max_resolution_none_problem(
            CONTENT_CAPABILITY_MEMBER,
            CONTENT_FEATURE_MEMBER,
        ));
    }
    if receives(&capabilities.content) && !receives(&capabilities.video) {
        return Some(content_needs_video_problem());
    }
    None
}

/// How the capability member one of the two `MaxResolution` rules is about is named in that
/// rule's refusal: the attendee's capability in lower case, then the meeting feature in the
/// `MeetingFeatures` member's own case.
const VIDEO_CAPABILITY_MEMBER: &str = "video";
const VIDEO_FEATURE_MEMBER: &str = "Video";
const CONTENT_CAPABILITY_MEMBER: &str = "content";
const CONTENT_FEATURE_MEMBER: &str = "Content";

/// How a meeting whose `MeetingFeatures` maximum resolution for one axis is `None` refuses
/// a request asking to send or receive that medium.
///
/// The sentence is FIXED text for this error class: nothing from the request is in it, not
/// the rejected capability value and not either id. Only which axis it is about varies
/// (the `Video` and `Content` forms of the same sentence), which is why both callers reach
/// it through the member names rather than composing prose of their own.
fn max_resolution_none_problem(capability_member: &str, feature_member: &str) -> String {
    format!(
        "Attendee's {capability_member} capability must be {CAPABILITY_NONE} if MeetingFeatures \
         is configured with {feature_member}:MaxResolution:{RESOLUTION_NONE}"
    )
}

/// How content reception without video reception is refused.
///
/// Fixed text as well: it names the two capability members and the two admissible values
/// generically, and interpolates nothing from the request.
fn content_needs_video_problem() -> String {
    format!(
        "Attendee's {VIDEO_CAPABILITY_MEMBER} capability must be set to either \
         {CAPABILITY_SEND_RECEIVE} or {CAPABILITY_RECEIVE} when setting \
         {CONTENT_CAPABILITY_MEMBER} capability to {CAPABILITY_SEND_RECEIVE} or \
         {CAPABILITY_RECEIVE}."
    )
}

/// The `MeetingArn` of a meeting: `arn:aws:chime:<region>:<account>:meeting/<id>`, which
/// satisfies `AmazonResourceName`'s modelled `^arn:` pattern and length 1..1011.
///
/// `region` is the Region of the ENDPOINT the call was signed for ([`request_region`]),
/// not the meeting's `MediaRegion`: the ARN names the control-plane resource, so a client
/// that builds a meeting's ARN from its own Region and account — to tag it, to write an
/// IAM resource condition, or to compare it against a stored ARN — builds the same string
/// this service hands back. `MediaRegion` is an independent request parameter, reported as
/// supplied and part of no other value this service derives.
pub fn meeting_arn(meeting_id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("meeting/{meeting_id}"))
}

/// The Region the caller signed this request for, from the SigV4 credential scope;
/// [`DEFAULT_REGION`] for a request carrying no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region an unsigned request is attributed to; the emulator's default.
const DEFAULT_REGION: &str = "us-east-1";

/// The tenant ids a stored meeting carries, given what the create asked for.
///
/// The member belongs to the `Meeting` projection itself, so a meeting created without
/// tenant ids holds the EMPTY list rather than nothing: every reader of a meeting reports
/// a list, and a client that iterates the member never meets an absent one. The modelled
/// `@length(1..5)` on `TenantIdList` bounds what a REQUEST may supply; it says nothing
/// about whether the projection carries the member.
pub fn tenant_ids(requested: Option<Vec<String>>) -> Vec<String> {
    requested.unwrap_or_default()
}

/// One id in the `GuidString` form this service's ids take:
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, with RFC 4122's version-4
/// and variant bits stamped in, so the value is also a well-formed random UUID — which is
/// what an SDK that parses a `MeetingId` or `AttendeeId` expects.
fn guid_from(label: &str) -> String {
    let digest = Sha256::digest(label.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
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

/// Nanoseconds since the Unix epoch: the entropy mixed into a minted id, so successive
/// ids do not expose this store's counter.
fn clock_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// The page size `ListAttendees` serves for a request whose `MaxResults` is `max_results`.
///
/// An absent `MaxResults` asks for as many attendees as the modelled range allows, not an
/// empty page. A caller-supplied bound is already inside `@range(1..100)` by the time a
/// handler runs; clamping again costs nothing and keeps this total.
pub fn page_size(max_results: Option<i32>) -> usize {
    let bound = max_results
        .unwrap_or(MAX_LIST_PAGE_SIZE)
        .clamp(1, MAX_LIST_PAGE_SIZE);
    usize::try_from(bound).unwrap_or(1)
}

/// Prefix carried inside a pagination token, so a token from somewhere else is refused
/// rather than read as an `AttendeeId`.
const PAGE_TOKEN_PREFIX: &str = "attendee:";

/// Encode the opaque `NextToken` that resumes a listing AT `next_id`.
///
/// Opaque to the caller (hex of a tagged byte string) because the model gives `NextToken`
/// no structure: a client must pass back what it was handed, not construct one. 2 hex
/// characters per byte keeps it inside the modelled length 0..4096 for any `AttendeeId`.
pub fn encode_page_token(next_id: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{next_id}"))
}

/// The `AttendeeId` a page token resumes at, or `None` when the token was not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// One page of an ordered `(key, record)` sequence, plus the `NextToken` that continues
/// it — `None` when the page reached the end.
///
/// The token names the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts at
/// the first key at or after it. Resuming is therefore well defined even when the record
/// the token names was deleted between the two calls: the scan simply continues at the
/// next key. Together with the stable key order of a `BTreeMap`, that is what makes a
/// paging client see each attendee exactly once.
pub fn page<'a, T: Clone + 'a>(
    ordered: impl IntoIterator<Item = (&'a String, &'a T)>,
    resume_at: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if resume_at.is_some_and(|from| key.as_str() < from) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(encode_page_token(key)));
        }
        records.push(record.clone());
    }
    (records, None)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState>)
}

/// The SHARED Chime TAG directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A resource's tags are keyed by ARN and live in ONE directory
/// ([`::service_common::chime_tags::ChimeTagDirectory`]) rather than on the
/// [`StoredMeeting`]s of this state struct, because the family's generic tagging routes
/// are served by one member for every member's resource: a copy inside
/// [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState`] would be invisible to the
/// sibling serving a tagging call, which would then report "no tags" for a meeting that
/// has them.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn chime_tags<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::chime_tags::ChimeTagDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::chime_tags::ChimeTagDirectory>)
}
