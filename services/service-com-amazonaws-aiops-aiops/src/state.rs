// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-aiops-aiops service.
//!
//! The service's primary resource is the **investigation group**. Every group is
//! stored exactly once, keyed by its 16-character id — the last segment of its ARN
//! — so each operation reads and writes the one record.
//!
//! A group is addressed by its ARN and by nothing else. The modelled
//! `InvestigationGroupIdentifier` admits a bare `[\-_A-Za-z0-9]{1,512}` label beside
//! the ARN, but that alternative is a WELL-FORMEDNESS rule, not an address: recorded
//! live responses resolve a group from its ARN alone and answer the modelled 404 for
//! every bare label — the label a stored group carries as its name included. So every
//! `{identifier}` member and the tagging operations' `resourceArn` alike resolve
//! through one ARN lookup ([`ComAmazonawsAiopsAiopsState::resolve_group_arn`]), and an
//! identifier that is not a stored group's ARN is a modelled 404.
//!
//! A group belongs to the Region its ARN names, and an account holds at most
//! [`MAX_INVESTIGATION_GROUPS_PER_REGION`] of them there — so the store is keyed by
//! id and read per region through
//! [`ComAmazonawsAiopsAiopsState::groups_in_region`] (or, for a page that resumes at
//! an id, [`ComAmazonawsAiopsAiopsState::groups_in_region_from`]).
//!
//! What an id looks like is likewise decided once, in [`is_group_id`], from the one
//! modelled pattern segment that states it, and an ARN is split into its fields once,
//! in [`arn_fields`] — so the identifier resolver and the cross-account rule read the
//! same grammar and cannot disagree about what an ARN is.
//!
//! This service is REGIONAL — its endpoints are `aiops.<region>.<dns-suffix>`
//! (`smithy_query(query="service_info")`) — so every regional endpoint serves only the
//! resources of its own Region. Identifier resolution is therefore scoped to the Region
//! the request was signed for ([`request_region`]): resolving, reading, mutating and
//! removing a group all take that Region and answer as if the group did not exist when
//! it belongs to another one.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use aws_smithy_schema_server::constraints::{
    path, validate, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait,
};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    CrossAccountConfiguration, EncryptionConfiguration, ResourceNotFoundException,
    ServiceQuotaExceededException,
};

/// Asking this emulator's own KMS about the key a write's `encryptionConfiguration`
/// names — the one rule of this module that cannot be decided from the request alone.
mod encryption_key;

pub use encryption_key::{encryption_key_rejection, KMS_KEY_NOT_USABLE};

/// Characters an investigation-group id is built from.
///
/// Taken from the modelled `InvestigationGroupArn` pattern, whose last segment is
/// `[A-Za-z0-9]{16}`:
/// `smithy_query(query="shape",shape="InvestigationGroupArn")`.
const GROUP_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Length of the id segment of an investigation-group ARN, from the same pattern.
pub const GROUP_ID_LEN: usize = 16;

/// The resource portion an investigation-group ARN carries before its id, from the
/// same modelled pattern.
pub const GROUP_ARN_RESOURCE_PREFIX: &str = "investigation-group/";

/// The ARN service namespace for this service (`arnNamespace`/sigv4 name `aiops`).
pub const ARN_SERVICE: &str = "aiops";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Retention applied when `CreateInvestigationGroup` omits `retentionInDays`.
///
/// From that member's own documentation: "If you omit this parameter, the default
/// of 90 days is used."
pub const DEFAULT_RETENTION_IN_DAYS: i64 = 90;

/// Applied when `CreateInvestigationGroup` omits `isCloudTrailEventHistoryEnabled`.
///
/// From that member's own documentation: "The default is true."
pub const DEFAULT_CLOUD_TRAIL_EVENT_HISTORY_ENABLED: bool = true;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteInvestigationGroup`, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. A cap is this service's own published
// quota where it publishes one; where it publishes none, the constant says so and
// the value is this emulator's resource guard, set well above any realistic use.
//
// The product is therefore finite: at most MAX_INVESTIGATION_GROUPS groups, each
// carrying at most MAX_TAGS tags, MAX_COLLECTION_ENTRIES tag key boundaries and
// MAX_COLLECTION_ENTRIES notification channels. Every remaining caller-sized member
// is bounded by the model: `crossAccountConfigurations` (0..25), each channel's
// `ChatConfigurationArns` list (1..5) and its SNS topic key (20..2048), each tag key
// (1..128) and value (1..256), each `tagKeyBoundaries` entry (1..128), `roleArn`
// (20..2048), `name` (1..512) and the policy document (1..32768). The centralized
// walk refuses every one of those BUT a map's KEY, which it never reaches; those are
// applied by [`map_key_rejection`] instead, so no caller-sized member reaches the
// store unchecked.
// ---------------------------------------------------------------------------

/// The most investigation groups one account may hold in one Region.
///
/// An AWS quota, from `CreateInvestigationGroup`'s own documentation: "Currently, you
/// can have one investigation group in each Region in your account." A create in a
/// Region that already holds this many is refused with the modelled
/// `ServiceQuotaExceededException` ([`region_quota_exceeded`]).
pub const MAX_INVESTIGATION_GROUPS_PER_REGION: usize = 1;

/// The most investigation groups this emulator will hold ACROSS every Region.
///
/// Emulator resource guard, NOT an AWS quota: the AWS quota is the per-region
/// [`MAX_INVESTIGATION_GROUPS_PER_REGION`] above, but the region a request is signed
/// for is caller-supplied text, so an unbounded number of regions could each hold
/// their one group. This cap is what keeps the store finite in that direction; a
/// create that would exceed it is refused with the modelled
/// `ServiceQuotaExceededException` too ([`store_capacity_exceeded`]).
pub const MAX_INVESTIGATION_GROUPS: usize = 500;

/// The most tags one group may carry.
///
/// From the `tags` member's own documentation: "You can associate as many as 50 tags
/// with an investigation group."
pub const MAX_TAGS: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator will
/// store for a caller-sized list or map whose size the model publishes no length
/// for — `tagKeyBoundaries` and `chatbotNotificationChannel`.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// One stored investigation group: every member `GetInvestigationGroup` reports,
/// plus the resource policy `Put/GetInvestigationGroupPolicy` attach to it and the
/// tags the tagging operations will key off its ARN.
#[derive(Debug, Clone, Default)]
pub struct InvestigationGroup {
    /// The 16-character id: the ARN's last segment and this record's store key.
    pub id: String,
    pub arn: String,
    /// The Region this group belongs to — the region field of its own ARN, which is
    /// also what the per-region quota and `ListInvestigationGroups` are scoped by.
    pub region: String,
    pub name: String,
    pub role_arn: String,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub retention_in_days: Option<i64>,
    pub tag_key_boundaries: Vec<String>,
    pub chatbot_notification_channel: BTreeMap<String, Vec<String>>,
    pub is_cloud_trail_event_history_enabled: Option<bool>,
    pub cross_account_configurations: Vec<CrossAccountConfiguration>,
    /// The IAM resource policy document, present only once a Put stored one.
    pub policy: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub created_by: String,
    pub created_at: i64,
    pub last_modified_by: String,
    pub last_modified_at: i64,
}

impl InvestigationGroup {
    /// `chatbotNotificationChannel` as the wire shape wants it: omitted when the
    /// group carries no channel at all.
    pub fn chatbot_notification_channel_output(&self) -> Option<HashMap<String, Vec<String>>> {
        (!self.chatbot_notification_channel.is_empty()).then(|| {
            self.chatbot_notification_channel
                .iter()
                .map(|(topic, configs)| (topic.clone(), configs.clone()))
                .collect()
        })
    }

    /// `tagKeyBoundaries`, omitted when none were ever set.
    pub fn tag_key_boundaries_output(&self) -> Option<Vec<String>> {
        (!self.tag_key_boundaries.is_empty()).then(|| self.tag_key_boundaries.clone())
    }

    /// `crossAccountConfigurations`, omitted when none were ever set.
    pub fn cross_account_configurations_output(&self) -> Option<Vec<CrossAccountConfiguration>> {
        (!self.cross_account_configurations.is_empty())
            .then(|| self.cross_account_configurations.clone())
    }

    /// Record that `principal` just changed this group.
    ///
    /// A successful mutation ALWAYS advances `lastModifiedAt`, which is what lets a
    /// caller notice a configuration change and order two writers by it. The stamp
    /// is whole seconds (the unit the modelled long carries), so a second write
    /// inside the same second would otherwise report the instant the first one did:
    /// `at` is therefore taken as a floor and the stamp still moves by its smallest
    /// representable step when the clock has not.
    pub fn touch(&mut self, principal: &str, at: i64) {
        self.last_modified_by = principal.to_string();
        self.last_modified_at = at.max(self.last_modified_at.saturating_add(1));
    }
}

/// Per-service state for com-amazonaws-aiops-aiops. A handler reaches it through
/// [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAiopsAiopsState {
    /// Every investigation group, keyed by its 16-character id. A `BTreeMap` so
    /// `ListInvestigationGroups` walks a stable, total order and a page token can
    /// name the next id.
    pub investigation_groups: BTreeMap<String, InvestigationGroup>,
    /// Mixed into each generated id so two creates in the same nanosecond differ.
    id_counter: u64,
}

impl ComAmazonawsAiopsAiopsState {
    /// The id of the group an identifier names: the group whose ARN is exactly that
    /// identifier, in `request_region`.
    ///
    /// The ONE resolver this service addresses a group through, for the `{identifier}`
    /// members and the tagging operations' `resourceArn` alike. The modelled
    /// `InvestigationGroupIdentifier` admits a bare `[\-_A-Za-z0-9]{1,512}` label beside
    /// the ARN, but recorded live responses resolve a group from its ARN alone: a bare
    /// label is well-formed and addresses nothing, whether or not a stored group carries
    /// it as its name or as the id segment of its ARN. So anything that is not a stored
    /// group's ARN resolves to `None` and the caller reports the modelled 404.
    ///
    /// An ARN must match the stored ARN in EVERY field, so an ARN naming another
    /// partition, region or account resolves to nothing rather than to the group that
    /// happens to share its id segment.
    ///
    /// Resolution is also scoped to `request_region`, the Region the request was signed
    /// for: this service is reached through per-Region endpoints, so an endpoint holds
    /// only its own Region's groups and resolves nothing for a group that belongs to
    /// another one — whatever ARN the request carried. That keeps a single reader,
    /// writer and remover consistent with the regional `ListInvestigationGroups` and
    /// with the per-region create quota.
    pub fn resolve_group_arn(&self, resource_arn: &str, request_region: &str) -> Option<String> {
        let id = group_id_from_arn(resource_arn)?;
        self.investigation_groups
            .get(id)
            .filter(|group| group.arn == resource_arn && group.region == request_region)
            .map(|group| group.id.clone())
    }

    /// The group `identifier` names in `request_region`, for a reporting operation.
    pub fn group(&self, identifier: &str, request_region: &str) -> Option<&InvestigationGroup> {
        let id = self.resolve_group_arn(identifier, request_region)?;
        self.investigation_groups.get(&id)
    }

    /// The group `identifier` names in `request_region`, for an operation the spec says
    /// mutates it.
    ///
    /// `id` is part of this record's identity — the store is keyed on it — so it may
    /// not be reassigned through this handle. No operation needs to; the model binds
    /// no `name` member on `UpdateInvestigationGroup` either, so a group keeps the
    /// name it was created with.
    pub fn group_mut(
        &mut self,
        identifier: &str,
        request_region: &str,
    ) -> Option<&mut InvestigationGroup> {
        let id = self.resolve_group_arn(identifier, request_region)?;
        self.investigation_groups.get_mut(&id)
    }

    /// The group `identifier` names in `request_region` for a POLICY operation that
    /// needs a stored document, or the not-found answer the absent one of the two
    /// resources is reported through — the two-step every policy operation shares,
    /// through its one home ([`resolved_policy_group`]).
    ///
    /// For a reporting operation; the group it answers with carries a policy.
    pub fn policy_group(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<&InvestigationGroup, ResourceNotFoundException> {
        resolved_policy_group(self.group(identifier, request_region))
    }

    /// [`Self::policy_group`] for an operation the spec says mutates the stored policy:
    /// the same two-step, through the same one home.
    pub fn policy_group_mut(
        &mut self,
        identifier: &str,
        request_region: &str,
    ) -> Result<&mut InvestigationGroup, ResourceNotFoundException> {
        resolved_policy_group(self.group_mut(identifier, request_region))
    }

    /// Every group this account holds in `region`, in the store's stable id order.
    ///
    /// A group belongs to exactly one Region — the one its ARN names — so this is
    /// what a regional reader (`ListInvestigationGroups`) reports and what the
    /// per-region quota counts.
    pub fn groups_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a InvestigationGroup> + 'a {
        self.groups_in_region_from(region, None)
    }

    /// The same per-region read as [`Self::groups_in_region`], resumed at `start_id`:
    /// the groups of `region` whose id is not before it, still in the store's stable
    /// id order.
    ///
    /// This is the reader a PAGINATED regional reader needs — the store is keyed by
    /// id, so a page token naming an id is where the next page starts — and it is the
    /// one place "which groups belong to this Region" is decided, for the quota and
    /// for a listing alike. The walk itself asks nothing of `start_id`: it simply
    /// begins at the first id that is not before it, so a reader resuming at a
    /// CALLER-SUPPLIED position must first resolve that position against this store
    /// ([`Self::holds_group_in_region`]) — a position the Region never held names no
    /// place in its listing, and walking from it would answer a page the store cannot
    /// back.
    pub fn groups_in_region_from<'a>(
        &'a self,
        region: &'a str,
        start_id: Option<&'a str>,
    ) -> impl Iterator<Item = &'a InvestigationGroup> + 'a {
        let from = start_id.map_or(Bound::Unbounded, Bound::Included);
        self.investigation_groups
            .range::<str, _>((from, Bound::Unbounded))
            .map(|(_, group)| group)
            .filter(move |group| group.region == region)
    }

    /// How many groups this account already holds in `region`, for the quota check.
    pub fn group_count_in_region(&self, region: &str) -> usize {
        self.groups_in_region(region).count()
    }

    /// Whether `region` holds a group whose id is exactly `id`.
    ///
    /// What a caller-supplied resume position has to satisfy before
    /// [`Self::groups_in_region_from`] is walked from it: a position is a place in
    /// THIS Region's listing, which only a group this Region holds is. Judged in one
    /// place so every reader that resumes a walk at a position a caller handed back
    /// judges it the same way, and from the stored record rather than from the shape
    /// of the text.
    pub fn holds_group_in_region(&self, id: &str, region: &str) -> bool {
        self.investigation_groups
            .get(id)
            .is_some_and(|group| group.region == region)
    }

    /// Store a newly created group.
    pub fn insert_group(&mut self, group: InvestigationGroup) {
        self.investigation_groups.insert(group.id.clone(), group);
    }

    /// Remove the group `identifier` names in `request_region`, with its policy and
    /// tags. `None` when nothing matched — including a group that exists in another
    /// Region, which this Region's endpoint may not reach, let alone delete.
    pub fn remove_group(
        &mut self,
        identifier: &str,
        request_region: &str,
    ) -> Option<InvestigationGroup> {
        let id = self.resolve_group_arn(identifier, request_region)?;
        self.investigation_groups.remove(&id)
    }

    /// A 16-character alphanumeric id no stored group is using.
    pub fn fresh_group_id(&mut self) -> String {
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let seed = now_nanos() ^ self.id_counter.rotate_left(17);
            let candidate = group_id_from_seed(seed);
            if !self.investigation_groups.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[A-Za-z0-9]{16}`, and there are always
        // more of them than there are stored groups.
        (0u64..=self.investigation_groups.len() as u64)
            .map(|n| format!("{n:016}"))
            .find(|id| !self.investigation_groups.contains_key(id))
            .unwrap_or_else(|| format!("{:016}", self.investigation_groups.len()))
    }
}

// ---------------------------------------------------------------------------
// How this service words a refusal
//
// Two wordings, each single-homed here so every operation states the same failure
// the same way: the `ValidationException` message a bad input is refused with
// ([`validation_exception_message`], [`validation_message`]) and the
// `ResourceNotFoundException` message an absent resource is reported with
// ([`investigation_group_not_found`], [`investigation_group_policy_not_found`]).
//
// A refusal the REQUEST BOUNDARY raises carries nothing the request sent: a
// `ValidationException` for a modelled constraint names the member and the
// requirement it violated and withholds the value that violated it, and a
// `ResourceNotFoundException` names the RESOURCE TYPE that is absent and withholds
// the identifier that named it. Recorded 4xx responses from this service are built
// that way across every operation, and both messages are the whole machine-readable
// detail a client gets, so a caller's own bytes must not be copied into them.
//
// A refusal the SERVICE ITSELF raises — a rule no modelled constraint expresses — is
// worded as the service words that error class, which is NOT always the clause
// grammar above and is not always value-free: what each sentence states, fixed text
// and variable parts alike, is recorded per error class on the helper that owns it
// ([`ROLE_NOT_ASSUMABLE`], [`INVALID_POLICY_DOCUMENT`],
// [`source_role_arn_in_investigation_account`], [`region_quota_exceeded`]). Where
// that sentence draws a value from the request, it is the value the request sent,
// spelled as it sent it.
//
// The TAGGING operations are served by the tagging frontend and have their own two
// wordings, single-homed below for the same reason: [`tag_capacity_rejection`] (400
// `BadRequestException`) and [`TAGGING_RESOURCE_NOT_FOUND`] (404
// `NotFoundException`). Neither carries anything the request sent either.
// ---------------------------------------------------------------------------

/// The modelled name of this service's primary resource
/// (`com.amazonaws.aiops#InvestigationGroup`, the resource every `{identifier}`
/// member carries a `@references` entry for), which is how a not-found answer names
/// it.
pub const INVESTIGATION_GROUP_RESOURCE: &str = "InvestigationGroup";

/// The modelled name of the resource policy attached to a group
/// (`com.amazonaws.aiops#InvestigationGroupPolicy`, the second `@references` entry on
/// the policy operations' inputs).
pub const INVESTIGATION_GROUP_POLICY_RESOURCE: &str = "InvestigationGroupPolicy";

/// The `ResourceNotFoundException` for an absent `resource`.
///
/// The message states the resource TYPE that does not exist and nothing else: the
/// recorded 404 for a group that was never created reads
/// `InvestigationGroup does not exist.` in every operation and every fixture, with
/// nothing drawn from the request in it. So the message varies only with WHICH
/// modelled resource is missing, never with the identifier that named it.
fn resource_not_found(resource: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("{resource} does not exist.")),
    }
}

/// The 404 every operation answers when the investigation group `{identifier}` names
/// does not exist — including the policy operations, where the GROUP is what is
/// missing and so is what gets reported.
pub fn investigation_group_not_found() -> ResourceNotFoundException {
    resource_not_found(INVESTIGATION_GROUP_RESOURCE)
}

/// The 404 `GetInvestigationGroupPolicy` answers when the group EXISTS and carries no
/// resource policy: a different condition from an absent group, and so a different
/// resource named, which is what lets a client tell the two apart.
pub fn investigation_group_policy_not_found() -> ResourceNotFoundException {
    resource_not_found(INVESTIGATION_GROUP_POLICY_RESOURCE)
}

/// Which of the two 404s a policy operation answers for `resolved`, what resolving its
/// `{identifier}` handed back — the WHOLE two-step a policy operation takes to reach a
/// stored document, in one place.
///
/// The individual wordings are single-homed above; so is this composition, because the
/// order is itself part of the contract. The identifier is resolved FIRST, and nothing
/// resolving is the absent GROUP ([`investigation_group_not_found`]) — a policy
/// operation reports the group it could not find, not a policy on a group that does not
/// exist. Only then is the stored document looked at, and a group that exists while
/// carrying none is the absent POLICY ([`investigation_group_policy_not_found`]), so a
/// client can tell the two conditions apart. A change to which exception either step
/// answers, or to their order, is therefore made once and every policy operation
/// follows it.
///
/// Generic over how the caller holds the resolved record, so the reporting and the
/// mutating resolver ([`ComAmazonawsAiopsAiopsState::policy_group`],
/// [`ComAmazonawsAiopsAiopsState::policy_group_mut`]) share this one body. The group it
/// answers with is guaranteed to carry a policy.
fn resolved_policy_group<G: std::ops::Deref<Target = InvestigationGroup>>(
    resolved: Option<G>,
) -> Result<G, ResourceNotFoundException> {
    let group = resolved.ok_or_else(investigation_group_not_found)?;
    if group.policy.is_none() {
        return Err(investigation_group_policy_not_found());
    }
    Ok(group)
}

/// The text separating a clause's subject from the requirement it states.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// The prefix the requirement half carries when the constraint violated is a
/// `@pattern`; the expression follows it.
const PATTERN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: ";

/// The `@pattern` expressions this service APPLIES, as
/// `(modelled expression, applied expression)`, for the members whose live constraint
/// differs from the one the model publishes.
///
/// `name` is the one such member: recorded 400 `ValidationException` responses quote
/// `[\-_A-Za-z0-9\[\]\(\)\{\}\.:!?@; ]+`, which is the modelled class
/// (`smithy_query(query="shape",shape="StringWithPatternAndLengthLimits")`:
/// `^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$`) plus `!`, `?`, `@` and `;`, and without the
/// anchors.
///
/// REPORTED, not ENFORCED: the constraint walk at the request boundary applies the
/// member's own modelled trait, read off the operation's generated input schema, which
/// no service crate can widen — so for `name` the expression reported here is the
/// WIDER of the two and the emulator still refuses a name this expression admits and
/// the model's does not. The reported form is what a caller is told the service
/// enforces; do not read it as what was checked.
///
/// The lookup is keyed on the model's EXACT expression, so a model that changes one
/// misses here and is reported through its own expression rather than through a stale
/// override.
const APPLIED_PATTERNS: &[(&str, &str)] = &[(
    r"^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$",
    r"[\-_A-Za-z0-9\[\]\(\)\{\}\.:!?@; ]+",
)];

/// The expression this service reports for the modelled `@pattern` `modelled`: its
/// [`APPLIED_PATTERNS`] entry when it has one, else the modelled expression with its
/// anchors dropped.
pub fn applied_pattern(modelled: &str) -> &str {
    APPLIED_PATTERNS
        .iter()
        .find(|(declared, _)| *declared == modelled)
        .map_or_else(|| unanchored(modelled), |(_, applied)| *applied)
}

/// A `@pattern` expression without the anchors a model spells it with: Smithy's
/// `@pattern` is an unanchored find, so `^`/`$` are how a model demands a whole-string
/// match rather than part of the expression a value has to satisfy.
///
/// A trailing `$` that is ESCAPED is a literal dollar sign inside the expression, not
/// the end anchor, so it stays: the run of backslashes before it decides.
///
/// (Sibling service crates carry their own copies of this helper — `grep -rln 'fn
/// unanchored' services/*/src` — because none is reachable from another;
/// `services/common/src/validation.rs` is the staged home for the one shared copy.)
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) if trimmed.chars().rev().take_while(|c| *c == '\\').count() % 2 == 0 => {
            trimmed
        }
        _ => body,
    }
}

/// The requirement half of a clause: what the value had to satisfy.
///
/// Read off the FRAMEWORK's own rendering of `kind` — with an empty path and no value,
/// so every word of the sentence is the framework's or the model's and none of it is
/// anything a caller sent — and then cut at the clause separator. A constraint class
/// the framework learns to check therefore cannot drift from this wording, and
/// [`ViolationKind`] being `#[non_exhaustive]` costs nothing here.
///
/// A `@pattern` requirement is restated through [`applied_pattern`].
pub fn requirement_text(kind: ViolationKind) -> String {
    let rendered = Violation {
        path: String::new(),
        kind,
        value: None,
        sensitive: false,
    }
    .message();
    let requirement = rendered
        .split_once(CONSTRAINT_SEPARATOR)
        .map_or(rendered.as_str(), |(_, requirement)| requirement);
    match requirement.strip_prefix(PATTERN_REQUIREMENT) {
        Some(pattern) => format!("{PATTERN_REQUIREMENT}{}", applied_pattern(pattern)),
        None => requirement.to_string(),
    }
}

/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated — and NOTHING of the value itself.
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}'{CONSTRAINT_SEPARATOR}{requirement}")
}

/// [`constraint_clause`] for a member that was ABSENT from the request.
///
/// The subject is `Value null`, because there was no value: the request carried
/// nothing at that path, which is a fact about the request's SHAPE rather than a value
/// echoed back from it.
pub fn absent_member_clause(member_path: &str, requirement: &str) -> String {
    format!("Value null at '{member_path}'{CONSTRAINT_SEPARATOR}{requirement}")
}

/// The AWS `ValidationException` envelope over one or more clauses: how many
/// constraints were violated, then the clauses.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [clause] => format!("1 validation error detected: {clause}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The whole `ValidationException` message for ONE member that failed ONE
/// requirement, for a rule a handler enforces itself (one the model does not express,
/// so the boundary walk passed it through). Worded through the same builders as a
/// boundary refusal, so the two cannot drift.
///
/// The path it is handed is the one the boundary walk would have built for that member,
/// which for a member inside a list element is [`list_element_member_path`]. A rule
/// broken at more than one path states one clause per path instead
/// ([`validation_envelope`] over [`constraint_clause`]), which is what the walk does.
pub fn validation_message(member_path: &str, requirement: &str) -> String {
    validation_envelope(&[constraint_clause(member_path, requirement)])
}

/// The path a clause names a member INSIDE a LIST ELEMENT at: the list member, the
/// element's own position and the step into the collection, then the member of the
/// element that failed (`<member>.<n>.member.<element member>`).
///
/// `element_index` is the element's 0-based index in the list the request carried; the
/// path states the 1-based position, spelled through the framework's own path builders
/// ([`path::element`], [`path::member`], [`path::join`]) — the very ones the boundary
/// walk builds a violation's path with. So a rule a handler enforces on a member of an
/// element names exactly the element a modelled constraint on that same member would be
/// refused at, and the two path grammars cannot drift.
fn list_element_member_path(member: &str, element_index: usize, element_member: &str) -> String {
    path::join(&[
        path::member(member),
        path::element(element_index + 1),
        path::member(element_member),
    ])
}

// ---------------------------------------------------------------------------
// Where a member path ends
//
// A clause's subject is a MEMBER PATH, and the path the constraint walk builds for a
// violation inside a MAP steps through that entry's own key — the caller's bytes,
// spelled exactly as sent. The wording rule above admits nothing the request sent, so
// the path a clause states stops at the map MEMBER: a map entry is reported at the
// member that holds it, with the requirement its value shape states, and never at a
// key. A LIST element keeps its position (`<member>.<n>.member`), which is a fact about
// the request's shape rather than anything it sent.
//
// Where the path stops is read off the MODEL, not off the text: a segment is a map step
// when it names a member this service models as a map, so no key can be mistaken for a
// member name or the other way round.
// ---------------------------------------------------------------------------

/// The modelled inputs a boundary refusal can name a member of: every operation input
/// this service validates a request against (each operation's own `INPUT_SCHEMA`).
///
/// The refusal seam is handed a member path and no operation, so a path's segments are
/// resolved against all of them at once; a member name names one shape across this
/// service's inputs, so which input a path came from cannot change the answer. A map
/// member added to any input's own members is covered the moment the model declares it;
/// this model nests no map inside a structure or a list, and a member schema carries no
/// members of its own to descend into, so resolution reads each input's own members.
const REFUSABLE_INPUTS: &[&Schema] = &[
    &crate::ops::types::CreateInvestigationGroupInput::SCHEMA,
    &crate::ops::types::UpdateInvestigationGroupInput::SCHEMA,
    &crate::ops::types::GetInvestigationGroupInput::SCHEMA,
    &crate::ops::types::DeleteInvestigationGroupInput::SCHEMA,
    &crate::ops::types::ListInvestigationGroupsInput::SCHEMA,
    &crate::ops::types::GetInvestigationGroupPolicyInput::SCHEMA,
    &crate::ops::types::PutInvestigationGroupPolicyInput::SCHEMA,
    &crate::ops::types::DeleteInvestigationGroupPolicyInput::SCHEMA,
    &crate::ops::types::TagResourceInput::SCHEMA,
    &crate::ops::types::UntagResourceInput::SCHEMA,
    &crate::ops::types::ListTagsForResourceInput::SCHEMA,
];

/// The character separating one step of a member path from the next.
const PATH_SEPARATOR: char = '.';

/// How a clause states what a MAP ENTRY's value had to satisfy: the requirement is the
/// value shape's whole constraint set, bundled at the map member, because the member is
/// the subject of the clause and one map may hold many entries that break it.
const MAP_VALUE_REQUIREMENT_PREFIX: &str = "Map value must satisfy constraint: ";

/// The member schema that `segment` names, where this service models that member as a
/// MAP, and `None` for every other segment (a scalar or collection member's name, a
/// list position, the collection step, or an entry key a caller chose).
fn map_member(segment: &str) -> Option<&'static Schema> {
    REFUSABLE_INPUTS
        .iter()
        .flat_map(|input| input.members().iter().copied())
        .find(|member| {
            matches!(member.shape_type(), ShapeType::Map)
                // How a member is spelled in a path is the framework's own rule
                // (`path::member`), so the lookup cannot drift from the spelling the
                // constraint walk produced.
                && member.member_name().map(path::member).as_deref() == Some(segment)
        })
}

/// The map member a violation at `path` is INSIDE, as `(the path up to and including
/// that member, that member's schema)`.
///
/// `None` when `path` names nothing inside a map — including a violation of the map
/// member itself (its entry count), whose subject already is the member and whose
/// requirement is the map's own.
fn map_entry_step(path: &str) -> Option<(&str, &'static Schema)> {
    let mut end = 0;
    for segment in path.split(PATH_SEPARATOR) {
        end += segment.len();
        if let Some(member) = map_member(segment) {
            // Everything after the map member is the entry: the key the caller chose,
            // and any step from that key into the entry's value.
            let member_path = path.get(..end)?;
            return (end < path.len()).then_some((member_path, member));
        }
        end += PATH_SEPARATOR.len_utf8();
    }
    None
}

/// The value shape's own constraint traits, as the collection member `schema` carries
/// them: a map value (and a list element) shares its collection member's schema, so its
/// constraints travel beside that schema rather than on it.
fn value_constraints(schema: &Schema) -> Option<&TraitMap> {
    let wrapper = schema.traits()?.get(&ElementConstraintsTrait::TRAIT_ID)?;
    let constraints = wrapper.as_any().downcast_ref::<ElementConstraintsTrait>()?;
    Some(constraints.inner())
}

/// Every requirement the value shape of the map member `schema` states, bundled as a
/// clause lists them.
fn map_value_requirements(schema: &Schema) -> Vec<String> {
    requirement_bundle(value_constraints(schema))
}

/// Every requirement a constraint trait map states, each worded through
/// [`requirement_text`] so a bundled requirement cannot drift from the one a scalar
/// member's clause states.
///
/// The order is the one a bundle lists them in: each bound, maximum before minimum,
/// then the value's character class or value set.
fn requirement_bundle(traits: Option<&TraitMap>) -> Vec<String> {
    let Some(traits) = traits else {
        return Vec::new();
    };
    let length = traits
        .get(&LengthTrait::TRAIT_ID)
        .and_then(|t| t.as_any().downcast_ref::<LengthTrait>());
    let range = traits
        .get(&RangeTrait::TRAIT_ID)
        .and_then(|t| t.as_any().downcast_ref::<RangeTrait>());
    let pattern = traits
        .get(&PatternTrait::TRAIT_ID)
        .and_then(|t| t.as_any().downcast_ref::<PatternTrait>());
    let values = traits
        .get(&EnumTrait::TRAIT_ID)
        .and_then(|t| t.as_any().downcast_ref::<EnumTrait>());
    let kinds = [
        length.and_then(|l| l.max).map(ViolationKind::MaxLength),
        length.and_then(|l| l.min).map(ViolationKind::MinLength),
        range
            .and_then(|r| r.max)
            .map(|max| ViolationKind::MaxValue(render_bound(max))),
        range
            .and_then(|r| r.min)
            .map(|min| ViolationKind::MinValue(render_bound(min))),
        pattern.map(|p| ViolationKind::Pattern(p.regex.to_string())),
        values.map(|v| {
            ViolationKind::EnumValue(v.values.iter().map(|value| (*value).to_string()).collect())
        }),
    ];
    kinds
        .into_iter()
        .flatten()
        .map(requirement_text)
        .collect()
}

/// A `@range` bound as a requirement states it: an integral bound without decimals.
///
/// [`ViolationKind`] carries a bound as the text it displays with, so a bound read back
/// off a schema is rendered here the way the walk renders the same bound. Reached only
/// by a map whose value shape the model gives a `@range` — none in this model does, so
/// it is what keeps the bundle complete for a value shape of any kind rather than for
/// the two kinds this model happens to hold.
fn render_bound(bound: f64) -> String {
    if bound.is_finite() && bound.fract() == 0.0 {
        format!("{}", bound as i64)
    } else {
        format!("{bound}")
    }
}

/// The one clause a violation states.
fn violation_clause(violation: &Violation) -> String {
    // Inside a map: the subject is the map MEMBER and the requirement is its value
    // shape's whole constraint set, so neither the key the caller chose nor the value it
    // sent reaches the message. A member whose value constraints the model leaves
    // uncaptured keeps the requirement that was broken, still stated at the member.
    // The absent-member wording has nothing to say here either way: a map holds the
    // entries a request carried, so there is no absent position inside one.
    if let Some((member_path, member)) = map_entry_step(&violation.path) {
        let requirements = map_value_requirements(member);
        let requirement = if requirements.is_empty() {
            requirement_text(violation.kind.clone())
        } else {
            format!("{MAP_VALUE_REQUIREMENT_PREFIX}[{}]", requirements.join(", "))
        };
        return constraint_clause(member_path, &requirement);
    }
    let requirement = requirement_text(violation.kind.clone());
    match violation.value {
        // No value at that path: the member was absent from the request.
        None => absent_member_clause(&violation.path, &requirement),
        Some(_) => constraint_clause(&violation.path, &requirement),
    }
}

/// The `ValidationException` message for a refusal the constraint walk made at the
/// request boundary: one clause per violation, in the AWS envelope.
///
/// A clause is stated ONCE however many violations state it: two entries of one map
/// breaking the same rule are one fact about one member, and the envelope counts the
/// facts it states.
///
/// `None` when there is nothing to state, which keeps the framework's own wording.
pub fn validation_exception_message(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    // The clauses keep the order the walk found them in, and the set is what decides
    // whether one was already stated — a request chooses how many violations it carries,
    // so a scan per clause would cost it the square of that.
    let mut stated = std::collections::HashSet::with_capacity(violations.len());
    let mut clauses: Vec<String> = Vec::with_capacity(violations.len());
    for violation in violations {
        let clause = violation_clause(violation);
        if stated.insert(clause.clone()) {
            clauses.push(clause);
        }
    }
    Some(validation_envelope(&clauses))
}

// ---------------------------------------------------------------------------
// Map KEYS: the modelled constraints no request boundary applies
//
// The centralized constraint walk captures a map entry's KEY as the path segment of
// that entry's value and returns before any leaf check, so no `@length` and no
// `@pattern` a map's key shape declares is ever applied there — the walk's own comment
// in `vendor/smithy-schema-server/src/constraints.rs` says as much. Every modelled
// constraint on a key shape is therefore applied here, by each operation that accepts
// the map, before anything is written: a key the model does not admit is otherwise
// stored and reported back as if the service had accepted it.
//
// Nothing about constraint checking is reimplemented. Each key shape is re-declared in
// the framework's own form — a [`TraitMap`] of [`LengthTrait`]/[`PatternTrait`] hung on
// a member schema — and the decision is [`validate`], the same walk the request
// boundary runs, so the Unicode classes a modelled pattern names mean here exactly
// what they mean there and the requirement a refusal states is worded through
// [`requirement_text`] like every other clause.
//
// A map entry whose VALUE the walk refuses never reaches a handler, so a request
// breaking a key rule and a value rule at once is refused by the walk's clause alone.
// ---------------------------------------------------------------------------

/// How a clause states what a map entry's KEY had to satisfy: the key shape's whole
/// constraint set, bundled at the map member for the same reason a value bundle is
/// ([`MAP_VALUE_REQUIREMENT_PREFIX`]) — the member is the subject of the clause and one
/// map may hold many entries that break the rule.
const MAP_KEY_REQUIREMENT_PREFIX: &str = "Map keys must satisfy constraint: ";

/// This service's Smithy namespace, for the key member schemas below.
const NAMESPACE: &str = "com.amazonaws.aiops";

/// One map member's KEY shape: the member a clause names, and the key shape's own
/// modelled constraints as the framework's walk reads them.
struct MapKeyShape {
    /// The map member the keys arrived on, which is what a clause names: a key is no
    /// member of its own, so there is no path below the map to state it at — the same
    /// rule the value bundle follows.
    member: &'static str,
    /// A member schema carrying the key shape's modelled constraints. Its own name is
    /// never reported.
    key: &'static Schema,
}

/// `TagKey`'s modelled constraints, as the model declares them
/// (`smithy_query(query="shape",shape="Tags")`: the map's key targets `TagKey`,
/// `@length` 1..128, `@pattern` `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`).
static TAG_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(
        r"^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$",
    )));
    traits
});

/// The member schema carrying [`TAG_KEY_TRAITS`].
static M_TAG_KEY: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.aiops#CheckedMapKey$tagKey",
        NAMESPACE,
        "tagKey",
    ),
    ShapeType::String,
    "tagKey",
    0,
)
.with_traits(&TAG_KEY_TRAITS);

/// `SNSTopicArn`'s modelled constraints, as the model declares them
/// (`smithy_query(query="shape",shape="ChatbotNotificationChannel")`: the map's key
/// targets `SNSTopicArn`, `@length` 20..2048, `@pattern` `^arn:.*$`).
static SNS_TOPIC_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(20), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(r"^arn:.*$")));
    traits
});

/// The member schema carrying [`SNS_TOPIC_ARN_TRAITS`].
static M_SNS_TOPIC_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.aiops#CheckedMapKey$snsTopicArn",
        NAMESPACE,
        "snsTopicArn",
    ),
    ShapeType::String,
    "snsTopicArn",
    0,
)
.with_traits(&SNS_TOPIC_ARN_TRAITS);

/// The `tags` map, whose keys every operation that accepts tags judges here.
static TAGS_KEYS: MapKeyShape = MapKeyShape {
    member: "tags",
    key: &M_TAG_KEY,
};

/// The `chatbotNotificationChannel` map, whose keys a create and an update judge here.
static CHATBOT_NOTIFICATION_CHANNEL_KEYS: MapKeyShape = MapKeyShape {
    member: "chatbotNotificationChannel",
    key: &M_SNS_TOPIC_ARN,
};

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`CheckedKey`] WRITES, and a root struct schema
/// contributes no path segment. The shape id is deliberately one the model does not
/// declare, so it cannot collide with a modelled shape.
static CHECKED_MAP_KEY_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.aiops#CheckedMapKey",
        NAMESPACE,
        "CheckedMapKey",
    ),
    ShapeType::Structure,
    &[],
);

/// One key written for the validating walk the way the codec writes a string member,
/// against the member schema whose re-declared constraints the walk is to apply.
struct CheckedKey<'a> {
    member: &'static Schema,
    value: &'a str,
}

impl SerializableStruct for CheckedKey<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(self.member, self.value)
    }
}

/// Whether `key` satisfies every constraint the key shape `member` carries, decided by
/// the framework's own constraint walk rather than by character predicates.
fn satisfies_key_shape(member: &'static Schema, key: &str) -> bool {
    validate(
        &CheckedKey {
            member,
            value: key,
        },
        &CHECKED_MAP_KEY_SCHEMA,
    )
    .is_empty()
}

/// The clause a map member states when any of `keys` is outside its key shape, or
/// `None` when every one of them satisfies it.
fn map_keys_clause<'a>(
    shape: &MapKeyShape,
    keys: impl IntoIterator<Item = &'a String>,
) -> Option<String> {
    keys.into_iter()
        .any(|key| !satisfies_key_shape(shape.key, key))
        .then(|| {
            constraint_clause(
                shape.member,
                &format!(
                    "{MAP_KEY_REQUIREMENT_PREFIX}[{}]",
                    requirement_bundle(shape.key.traits()).join(", ")
                ),
            )
        })
}

/// Why the KEYS of the maps a request carries are unacceptable, or `None` when every
/// one of them satisfies its key shape's modelled constraints.
///
/// Every operation that accepts either map applies this one rule — before its own
/// unmodelled rules, because a modelled constraint is one the boundary would have
/// refused the request with first — each rendering the returned sentence as its own
/// modelled `ValidationException`. Pass `None` for a member the operation does not
/// accept.
///
/// The clauses are ordered as the inputs declare the members and counted in the
/// envelope ([`validation_envelope`]), so a request breaking both members' rules reads
/// exactly as a boundary refusal of two members does.
pub fn map_key_rejection(
    tags: Option<&HashMap<String, String>>,
    chatbot_notification_channel: Option<&HashMap<String, Vec<String>>>,
) -> Option<String> {
    let clauses: Vec<String> = [
        tags.and_then(|tags| map_keys_clause(&TAGS_KEYS, tags.keys())),
        chatbot_notification_channel.and_then(|channel| {
            map_keys_clause(&CHATBOT_NOTIFICATION_CHANNEL_KEYS, channel.keys())
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!clauses.is_empty()).then(|| validation_envelope(&clauses))
}

/// Why an `encryptionConfiguration` is unacceptable, or `None` when it is fine.
///
/// `EncryptionConfiguration.type` targets the modelled `EncryptionConfigurationType`
/// enum, so only that shape's own value set is accepted. The catalog is the
/// generated `ENCRYPTION_CONFIGURATION_TYPE_VALUES`, which carries exactly the
/// values `smithy_query(query="shape",shape="EncryptionConfigurationType")` reports.
/// Every operation that accepts the structure applies this one rule, each rendering
/// the refusal as its own modelled `ValidationException`.
///
/// The value-set refusal is the service's own `ValidationException` MESSAGE, worded
/// through [`validation_message`]: the member path and the value set it had to satisfy,
/// with the rejected value withheld, exactly as a boundary refusal for the same class of
/// violation reads.
///
/// `type` is NOT `@required` on the structure, so a configuration that omits it is a
/// request the model admits and the VALUE SET has nothing to say about — hence naming
/// no type is not refused here.
///
/// The structure's two members have to AGREE, which is the second half of this rule and
/// no modelled constraint states it: the structure is "Use this structure to specify a
/// customer managed KMS key to use to encrypt investigation data" and `kmsKeyId` is "if
/// the investigation group uses a customer managed key for encryption, this field
/// displays the ID of that key" (`smithy_query(query="shape",shape="EncryptionConfiguration")`),
/// so the customer managed key type IS the request to encrypt with the key the member
/// names and the Amazon Web Services owned key type is the request to use no customer
/// managed key at all. A configuration selecting a customer managed key while naming no
/// key names nothing to encrypt with, and one selecting the Amazon Web Services owned
/// key while naming a key asks for two different keys at once; neither is a
/// configuration the service can honour, so both are refused
/// ([`KMS_KEY_ID_REQUIRED`], [`KMS_KEY_ID_NOT_ACCEPTED`]) and nothing is stored.
///
/// A configuration naming a key and NO type asks for that key
/// ([`stored_encryption_configuration`]), so it agrees with itself and is accepted.
///
/// Whether the key a coherent configuration names is a key that EXISTS is a question
/// only KMS can answer, so it is asked separately and over the wire:
/// [`encryption_key_rejection`].
pub fn encryption_configuration_rejection(
    configuration: Option<&EncryptionConfiguration>,
) -> Option<String> {
    let configuration = configuration?;
    let names_a_key = configuration.kms_key_id.is_some();
    // A configuration naming NO type is one the model admits: `type` is not `@required`,
    // and a structure naming only a key asks for that key.
    let supplied = configuration.r#type.as_deref()?;
    if !crate::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&supplied) {
        let allowed = crate::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        return Some(validation_message(
            ENCRYPTION_CONFIGURATION_TYPE_MEMBER,
            &requirement_text(ViolationKind::EnumValue(allowed)),
        ));
    }
    if supplied == CUSTOMER_MANAGED_KMS_KEY_TYPE && !names_a_key {
        return Some(KMS_KEY_ID_REQUIRED.to_string());
    }
    if supplied == AWS_OWNED_KEY_TYPE && names_a_key {
        return Some(KMS_KEY_ID_NOT_ACCEPTED.to_string());
    }
    None
}

/// The `encryptionConfiguration.type` member, as a refusal about it spells the path.
const ENCRYPTION_CONFIGURATION_TYPE_MEMBER: &str = "encryptionConfiguration.type";

/// The `encryptionConfiguration.kmsKeyId` member, spelled the same way.
const KMS_KEY_ID_MEMBER: &str = "encryptionConfiguration.kmsKeyId";

/// The sentence a write selecting a customer managed key while naming no key is refused
/// with: the service's own vocabulary for a member another member's value makes
/// mandatory, naming the two members and the value that ties them and nothing the
/// request sent.
///
/// It is NOT worded through [`validation_message`]: that envelope states which member
/// violated which modelled CONSTRAINT, and no constraint is violated here — the model
/// marks neither member `@required` — what the request got wrong is the agreement
/// between them ([`encryption_configuration_rejection`]).
pub static KMS_KEY_ID_REQUIRED: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{KMS_KEY_ID_MEMBER} must be specified when {ENCRYPTION_CONFIGURATION_TYPE_MEMBER} is \
         {CUSTOMER_MANAGED_KMS_KEY_TYPE}."
    )
});

/// The other direction of the same rule: the sentence a write selecting the Amazon Web
/// Services owned key while naming a customer managed key is refused with.
pub static KMS_KEY_ID_NOT_ACCEPTED: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{KMS_KEY_ID_MEMBER} cannot be specified when {ENCRYPTION_CONFIGURATION_TYPE_MEMBER} is \
         {AWS_OWNED_KEY_TYPE}."
    )
});

/// Why the caller-sized collections on a create or update request are unacceptable,
/// or `None` when every one of them is within the store's bounds.
///
/// The model publishes no length for the `chatbotNotificationChannel` map or the
/// `tagKeyBoundaries` list, so the centralized walk cannot refuse an oversized one
/// and this is where the bound lives. Every operation that stores any of them applies
/// this one rule, each rendering the refusal as its own modelled `ValidationException`.
/// Pass `None` for a member the operation does not accept. The `tags` map is bounded
/// by the service's published tag maximum instead, which is a different refusal:
/// [`tag_capacity_rejection`].
///
/// The refusal is worded through [`validation_message`] too: the member path and the
/// bound it exceeded, in the same clause a modelled `@length` maximum would be refused
/// with at the boundary — so a client reads one grammar for both, and the entry count
/// the request carried is not echoed back.
pub fn collection_bounds_rejection(
    tag_key_boundaries: Option<&[String]>,
    chatbot_notification_channel: Option<&std::collections::HashMap<String, Vec<String>>>,
) -> Option<String> {
    let too_many = |member: &str, actual: usize, cap: usize| {
        (actual > cap).then(|| {
            validation_message(
                member,
                &requirement_text(ViolationKind::MaxLength(cap as i64)),
            )
        })
    };
    too_many(
        "tagKeyBoundaries",
        tag_key_boundaries.map_or(0, <[String]>::len),
        MAX_COLLECTION_ENTRIES,
    )
    .or_else(|| {
        too_many(
            "chatbotNotificationChannel",
            chatbot_notification_channel.map_or(0, std::collections::HashMap::len),
            MAX_COLLECTION_ENTRIES,
        )
    })
}

/// Why a resource may not end up carrying `resulting_tags` tags, or `None` when that
/// many fit.
///
/// The cap applies to the tag set the resource ENDS UP with, so `TagResource` counts
/// the merge of the request's map with the tags the resource already carries, and a
/// create counts the map it was given. The message states that rule and carries
/// nothing of the request.
///
/// This refusal is NOT the service's own `ValidationException`: a tag write is served
/// by the tagging frontend, whose refusal is HTTP 400 with the error code
/// `BadRequestException` — the code recorded for a 51-tag call on an investigation
/// group, and the one a sibling service's tag map is refused with as well
/// (`services/service-com-amazonaws-batch-awsbatchv20160810/src/service.rs`). The
/// shape is not in this service's model, so it is answered through
/// [`crate::scaffolded::Scaffolded::bad_request`].
pub fn tag_capacity_rejection(resulting_tags: usize) -> Option<String> {
    (resulting_tags > MAX_TAGS)
        .then(|| format!("The resultant tag set must not have more than {MAX_TAGS} user tags."))
}

/// The tag-key prefix Amazon Web Services reserves for the tags IT applies.
///
/// A key carrying it names a system tag, which a caller may neither create, change nor
/// remove — the AWS-wide tagging rule every taggable service's tagging surface enforces.
/// No modelled constraint expresses it: the `TagKey` pattern
/// (`smithy_query(query="shape",shape="Tags")`:
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`, length 1..128) admits `:` and every letter, so the
/// boundary walk passes such a key through and the rule has to live here.
///
/// Tag keys are case sensitive, and what is reserved is this literal lowercase prefix —
/// the same reading the sibling crates that already enforce it take
/// (`services/service-com-amazonaws-xray-awsxray/src/state.rs`'s
/// `RESERVED_TAG_KEY_PREFIX`, `services/service-com-amazonaws-acmpca-acmprivateca`'s
/// `reserved_tag_rejection`). A key that merely CONTAINS it, or starts with `aws`
/// without the colon, is an ordinary caller-owned key.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why the tag keys a request names may not be written or removed, or `None` when every
/// one of them is a key the caller owns.
///
/// The rule is about who OWNS the key, not about which direction the write goes, so it
/// applies to every key a request would create, change or delete: the `tags` map of a
/// create and of `TagResource`, and the `tagKeys` list of `UntagResource`. It is checked
/// over the whole request before anything is written, so a request naming one reserved
/// key stores and removes nothing at all — not even its other keys.
///
/// This rule is about the REQUEST alone, so it is applied before the store is consulted;
/// [`tag_capacity_rejection`] bounds the set the resource ENDS UP with and so can only
/// be applied once the resource has been resolved.
///
/// Answered through the same channel as [`tag_capacity_rejection`] and worded the same
/// way: a tag write is served by the tagging frontend, whose refusal of a bad tag
/// request is HTTP 400 with the error code `BadRequestException`
/// ([`crate::scaffolded::Scaffolded::bad_request`]), and the sentence states the rule
/// while carrying nothing the request sent.
pub fn reserved_tag_key_rejection<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    keys.into_iter()
        .any(|key| key.starts_with(RESERVED_TAG_KEY_PREFIX))
        .then(|| {
            format!(
                "Tag keys starting with {RESERVED_TAG_KEY_PREFIX} are reserved for Amazon Web \
                 Services use and cannot be created, changed or removed."
            )
        })
}

/// What the TAGGING operations answer when `resourceArn` names no resource.
///
/// The same split as [`tag_capacity_rejection`], one status up: a tagging call is
/// served by the tagging frontend rather than by this service's own resource surface,
/// and the frontend reports an absent resource as HTTP 404 with the error code
/// `NotFoundException` — the code recorded for `ListTagsForResource`, `TagResource`
/// and `UntagResource` against an investigation group that was never created, where
/// the resource operations are recorded answering this service's own modelled
/// `ResourceNotFoundException` ([`investigation_group_not_found`]). That shape is not
/// in this service's model, so the answer goes out through
/// [`crate::scaffolded::Scaffolded::not_found`].
///
/// The frontend addresses every taggable resource type through one `resourceArn`, so
/// its wording names no resource type; like every other 4xx this service words, it
/// carries nothing the request sent. Only the status and the error code are
/// requirements — no rule published by the model or the recording produces this
/// sentence — so this is the frontend's recorded generic text, single-homed here.
pub const TAGGING_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The `EncryptionConfigurationType` value naming the Amazon Web Services owned key,
/// taken from the modelled value set
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`).
pub const AWS_OWNED_KEY_TYPE: &str = "AWS_OWNED_KEY";

/// The `EncryptionConfigurationType` value naming a customer managed KMS key, taken
/// from the modelled value set
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`).
pub const CUSTOMER_MANAGED_KMS_KEY_TYPE: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// The encryption configuration a group carries when its create omitted one.
///
/// From `encryptionConfiguration`'s own documentation: "If you omit this parameter,
/// CloudWatch investigations will use an Amazon Web Services key to encrypt the
/// data." That is the [`AWS_OWNED_KEY_TYPE`] member of the modelled value set, and it
/// carries no `kmsKeyId` because no customer managed key is involved — so the
/// default-key state is a stored configuration like any other and every later read
/// reports it, rather than the member going missing.
pub fn default_encryption_configuration() -> EncryptionConfiguration {
    EncryptionConfiguration {
        r#type: Some(AWS_OWNED_KEY_TYPE.to_string()),
        kms_key_id: None,
    }
}

/// The encryption configuration a write STORES for the configuration it was given,
/// which is what every later read of the group reports.
///
/// Neither member of the structure is `@required`
/// (`smithy_query(query="shape",shape="EncryptionConfiguration")`), so a request may
/// name the member while naming only a key type, only a key id, or neither. What a
/// write stores follows from what the request NAMES:
///
/// * a key the request names is the key the group uses — `kmsKeyId` is documented as
///   the id of the customer managed key the group's data is encrypted with, so a
///   configuration carrying one is stored with that id, and a configuration naming the
///   id but no type is stored as the customer managed key type
///   ([`CUSTOMER_MANAGED_KMS_KEY_TYPE`]) it asks for: the structure's own documentation
///   is "Use this structure to specify a customer managed KMS key to use to encrypt
///   investigation data";
/// * a request that names NO key at all — the member omitted, or a structure carrying
///   neither member — asks for no customer managed key and gets the documented Amazon
///   Web Services owned key ([`default_encryption_configuration`]).
///
/// Either way the stored member names how the group's data is encrypted, with the key
/// the caller named when they named one.
///
/// Single-homed here because every writer of the member — the create and the update —
/// stores what this returns, so the two cannot disagree about what a configuration
/// means.
///
/// Every writer applies [`encryption_configuration_rejection`] first, so a configuration
/// reaching this one AGREES with itself: a structure naming the customer managed key type
/// carries the key id, and one naming the Amazon Web Services owned key type carries
/// none.
pub fn stored_encryption_configuration(
    supplied: Option<EncryptionConfiguration>,
) -> EncryptionConfiguration {
    match supplied {
        Some(configuration) if configuration.r#type.is_some() => configuration,
        Some(configuration) if configuration.kms_key_id.is_some() => EncryptionConfiguration {
            r#type: Some(CUSTOMER_MANAGED_KMS_KEY_TYPE.to_string()),
            kms_key_id: configuration.kms_key_id,
        },
        _ => default_encryption_configuration(),
    }
}

/// Why a `crossAccountConfigurations` list is unacceptable, or `None` when every
/// entry names an account other than this one.
///
/// A cross-account configuration exists to let the investigating account reach
/// ANOTHER account's resources, so an entry has to NAME that other account, and an
/// entry that does not is refused: it could never do anything. Both halves of that are
/// enforced here, because both are things the model admits —
///
/// * `sourceRoleArn` is not `@required` on the structure
///   (`smithy_query(query="shape",shape="CrossAccountConfiguration")`), so an entry
///   carrying no source role at all is a request the boundary walk passes through; it
///   names no account, and is refused through [`absent_member_clause`] with the
///   framework's own absent-member requirement, exactly as the walk would refuse a
///   member the model did mark required;
/// * the member's modelled constraints (`@length 20..2048`, `^arn:.*$`) admit text that
///   is not a complete ARN, so a value whose account id field cannot be read
///   ([`arn_account_id`]) names no account either and is refused alongside a value whose
///   account id IS the investigation account's own.
///
/// So the list is accepted only when every entry names a readable account that is not
/// this one, and nothing the rule says cannot exist is ever stored — every operation
/// that stores the list applies this rule, each rendering the refusal as its own
/// modelled `ValidationException`.
///
/// The two conditions are worded differently, because the service words them
/// differently, and the ABSENT-member one is applied first — it is the condition the
/// boundary walk itself would have refused the request with, had the model marked the
/// member required, exactly as a modelled constraint is judged before an unmodelled
/// rule everywhere else in this service:
///
/// * an entry carrying no source role AT ALL is a member missing from the request, which
///   the clause grammar states: [`absent_member_clause`] inside [`validation_envelope`],
///   at the entry's own position ([`list_element_member_path`]), with the framework's own
///   absent-member requirement. Each such entry is a separate fact about the request's
///   shape, so a list missing the member in several places states one clause per
///   position, in list order, and the envelope counts them — again as the walk does;
/// * an entry carrying a source role that names no OTHER account is refused with the
///   service's own sentence for that error class
///   ([`source_role_arn_in_investigation_account`]), which names the investigation
///   account and ECHOES the ARN the request sent — a caller sending up to 25 entries
///   identifies the refused one by that ARN, and the sentence states no member path and
///   no clause grammar. That sentence names ONE source role, so the FIRST entry the rule
///   refuses, in list order, is the one reported, and both operations that store the list
///   report that same entry.
pub fn cross_account_configurations_rejection(
    configurations: Option<&[CrossAccountConfiguration]>,
) -> Option<String> {
    let configurations = configurations?;
    // Nothing at that path to judge: the entry names no account because it carries no
    // source role.
    let absent: Vec<String> = configurations
        .iter()
        .enumerate()
        .filter(|(_, configuration)| configuration.source_role_arn.is_none())
        .map(|(index, _)| {
            absent_member_clause(
                &list_element_member_path(
                    CROSS_ACCOUNT_CONFIGURATIONS_MEMBER,
                    index,
                    SOURCE_ROLE_ARN_MEMBER,
                ),
                &requirement_text(ViolationKind::MissingRequiredMember),
            )
        })
        .collect();
    if !absent.is_empty() {
        return Some(validation_envelope(&absent));
    }
    // A source role names another account only when an account id can be read from it
    // at all, and that id is not this account's.
    let own_account = ::engine::EMULATOR_ACCOUNT_ID;
    configurations
        .iter()
        .filter_map(|configuration| configuration.source_role_arn.as_deref())
        .find(|arn| arn_account_id(arn).is_none_or(|account| account == own_account))
        .map(|arn| source_role_arn_in_investigation_account(own_account, arn))
}

/// The sentence a write naming a cross-account source role that does not name an
/// account other than the investigation account is refused with.
///
/// Recorded behavior: the refusal is the operation's modelled `ValidationException`
/// (400) with `Source role ARN cannot belong to the investigation account <account>.
/// Found ARN: <sourceRoleArn>`. The fixed text is the whole sentence around the two
/// variable parts; `investigation_account` is the account the investigation group
/// belongs to, read from this emulator's own state (every group's ARN carries it), and
/// `source_role_arn` is the value the request sent, spelled exactly as it sent it. One
/// function for every operation that stores the member — the create and the update — so
/// the two cannot word this refusal differently.
pub fn source_role_arn_in_investigation_account(
    investigation_account: &str,
    source_role_arn: &str,
) -> String {
    format!(
        "Source role ARN cannot belong to the investigation account \
         {investigation_account}. Found ARN: {source_role_arn}"
    )
}

/// The list member a cross-account configuration arrives on, as a clause spells it.
const CROSS_ACCOUNT_CONFIGURATIONS_MEMBER: &str = "crossAccountConfigurations";

/// The member of a cross-account configuration this rule judges.
const SOURCE_ROLE_ARN_MEMBER: &str = "sourceRoleArn";

/// Why the configuration a write would store is unacceptable, or `None` when every
/// rule accepts it — the WHOLE chain a request that stores an investigation group's
/// configuration passes, in one place.
///
/// The individual rules are single-homed above; so is their composition, because the
/// chain is itself part of the contract: the rules are applied in the order
/// [`encryption_configuration_rejection`], [`cross_account_configurations_rejection`],
/// [`collection_bounds_rejection`], and the FIRST one with something to say is what
/// the request is refused with, so a request violating more than one of them is
/// refused identically whichever operation carried it. Every operation that stores
/// any of these members applies this one chain — each rendering the returned sentence
/// as its own modelled `ValidationException` — so a create and an update can neither
/// disagree about which input is refused nor about which rule reports it.
///
/// Pass `None` for a member the operation does not accept.
///
/// The modelled constraints on a map's KEYS are NOT part of this chain:
/// [`map_key_rejection`] states them, and every operation applies that rule before this
/// one, because a modelled constraint is what the request boundary would have refused
/// the request with first.
///
/// The rules that resolve a value against another emulated service are not part of it
/// either — [`role_arn_rejection`] and [`encryption_key_rejection`] are awaited, and
/// every operation applies them AFTER this chain, so a request the chain can refuse from
/// its own contents is refused without a hop.
pub fn write_configuration_rejection(
    encryption_configuration: Option<&EncryptionConfiguration>,
    cross_account_configurations: Option<&[CrossAccountConfiguration]>,
    tag_key_boundaries: Option<&[String]>,
    chatbot_notification_channel: Option<&std::collections::HashMap<String, Vec<String>>>,
) -> Option<String> {
    encryption_configuration_rejection(encryption_configuration)
        .or_else(|| cross_account_configurations_rejection(cross_account_configurations))
        .or_else(|| collection_bounds_rejection(tag_key_boundaries, chatbot_notification_channel))
}

/// The service principal a group's `roleArn` has to let assume the role.
///
/// This service's OWN principal, which the model states: the service shape's
/// `aws.api#service` trait carries `cloudTrailEventSource` `aiops.amazonaws.com`
/// (`smithy_query(query="service_info")`) — the identity this service acts under when it
/// assumes a caller-supplied role.
pub const SERVICE_PRINCIPAL: &str = "aiops.amazonaws.com";

/// The sentence a write naming a role this service could not act as is refused with.
///
/// Recorded behavior: a `CreateInvestigationGroup` whose `roleArn` names a role the
/// service cannot assume is answered 400 `ValidationException` with this text. It states
/// both halves of what the caller has to fix — the role has to exist and it has to be
/// configured for this service — and, like every other 4xx this service words, it carries
/// nothing the request sent.
///
/// It is NOT worded through [`validation_message`]: that envelope states which MEMBER
/// violated which CONSTRAINT, and this refusal is not a constraint violation — the value
/// satisfies every constraint the model declares for it, and what failed is the
/// RESOLUTION of the role it names.
pub const ROLE_NOT_ASSUMABLE: &str = "The InvestigationGroup could not assume the role \
     associated with the specified account. Please validate that the role exists and is \
     configured correctly.";

/// Why the `roleArn` a write would store names a role this service could not act as, or
/// `None` when nothing shows it could not.
///
/// An investigation group gathers investigation data AS this role (`roleArn`: "Specify
/// the ARN of the IAM role that CloudWatch investigations will use when it gathers
/// investigation data"), so the role is resolved BEFORE anything is persisted and a role
/// the service could not assume is refused with the operation's modelled
/// `ValidationException` ([`ROLE_NOT_ASSUMABLE`]) rather than stored on a group that
/// could not work. ONE function for every operation that writes the member — the create
/// and the update — so the two cannot judge the same value differently.
///
/// The value is unusable in either of two ways:
///
/// * it names no IAM role of THIS account at all
///   ([`::service_common::iam_role::role_name`], the tree's one reading of an IAM ARN).
///   The modelled constraints on the member (`@length 20..2048`, `^arn:.*$`) admit any
///   ARN, so the boundary walk passes a bucket, a user or another account's role through,
///   and none of those is a role this account's service could assume for this account's
///   group — a fact about the VALUE, which needs no record to settle, exactly as
///   `::service_common::iam_role::role_pass_is_unauthorized` settles it;
/// * this account's IAM ANSWERED that the role is one this service cannot assume: it
///   holds no role under that name, or the trust policy the role carries does not grant
///   [`SERVICE_PRINCIPAL`] `sts:AssumeRole`. That whole question is the ONE reading every
///   emulated service that resolves a caller-supplied execution role shares
///   ([`::service_common::iam_role::role_is_unassumable`]), so this crate states only
///   WHICH principal has to be admitted and WHICH operations ask.
///
/// IAM is reached the way any emulated service reaches another: the AWS SDK over the
/// emulator's OWN in-process callback, so nothing here opens a socket. Everything that is
/// not one of the two answers above is accepted — an emulator with no IAM mounted to ask
/// included — because a refusal grounded in a record has to be grounded in what IAM
/// actually said. Awaited with NO store guard held.
pub async fn role_arn_rejection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<String> {
    let names_a_role_of_this_account = ::service_common::iam_role::role_name(role_arn).is_some();
    let unassumable = !names_a_role_of_this_account
        || ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL)
            .await;
    unassumable.then(|| ROLE_NOT_ASSUMABLE.to_string())
}

/// Why a resource policy document is unacceptable, or `None` when it is one this
/// service will attach.
///
/// The modelled constraints on the document bound only its length and its character
/// set (`@length 1..32768`, `^[\u0009\u000A\u000D\u0020-\u00FF]+$`), so the boundary
/// walk admits any printable text; the operation's own semantic check is that the
/// document IS a policy document — the `policy` member is documented as "The policy,
/// in JSON format", and a policy document is a JSON object. Parsed with `serde_json`,
/// so the judgement is a real parser's and never a match on the document's text.
///
/// A document that fails this is not stored at all, so a later
/// `GetInvestigationGroupPolicy` cannot hand back something unparseable.
///
/// The refusal is the service's own sentence for that error class
/// ([`INVALID_POLICY_DOCUMENT`]), not a clause about the member: the model expresses no
/// JSON requirement, so nothing the boundary walk applies was violated.
pub fn policy_document_rejection(policy: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(policy).ok();
    parsed
        .filter(serde_json::Value::is_object)
        .is_none()
        .then(|| INVALID_POLICY_DOCUMENT.to_string())
}

/// The sentence a `PutInvestigationGroupPolicy` carrying something that is not a policy
/// document is refused with.
///
/// Recorded behavior: the refusal is the operation's modelled `ValidationException`
/// (400) with this text. It is entirely FIXED — it draws nothing from the request: no
/// member path, no excerpt of the document, no count — so the sentence is the whole
/// requirement for the error class.
///
/// It is NOT worded through [`validation_message`]: that envelope states which MEMBER
/// violated which modelled CONSTRAINT, and the document satisfies every constraint the
/// model declares for it ([`policy_document_rejection`] states which).
pub const INVALID_POLICY_DOCUMENT: &str = "The specified resource policy document is invalid.";

/// The literal first field every ARN opens with.
const ARN_FIRST_FIELD: &str = "arn";

/// The six colon-separated fields of `candidate` —
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — or `None` when it is not
/// in that form at all: fewer than six fields, or a first field that is not literally
/// `arn`.
///
/// ONE definition of "split an ARN" for this module: every rule of this service that
/// reads a field out of an ARN — the service namespace and resource an identifier names
/// ([`group_id_from_arn`]) and the account a cross-account source role names
/// ([`arn_account_id`]) — splits it here and differs only in which of the fields it then
/// requires, so a change to what this service accepts as an ARN is made exactly once.
///
/// The resource portion may itself hold `:` (`…:type:id`), so only the leading fields
/// are split off and the last field keeps every colon it carries.
fn arn_fields(candidate: &str) -> Option<[&str; 6]> {
    let mut split = candidate.splitn(6, ':');
    let fields = [
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
        split.next()?,
    ];
    (fields[0] == ARN_FIRST_FIELD).then_some(fields)
}

/// The account id field of `arn`, or `None` when `arn` is not an ARN carrying one.
///
/// The account is the FIFTH field of a complete ARN ([`arn_fields`]), so all six have to
/// be present for one to be read: a string with fewer fields carries no account id — its
/// last field is a resource, a region or a service name — and answers `None` rather than
/// having whatever sits in the fourth position read as an account. An ARN whose account
/// field is empty, as a resource owned by no account spells it, carries none either.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    let [_arn, _partition, _service, _region, account, _resource] = arn_fields(arn)?;
    (!account.is_empty()).then_some(account)
}

/// The `ServiceQuotaExceededException` every quota refusal of this service is built
/// through: the quota that was exceeded, stated in `message`, and the resource type
/// and service it belongs to.
///
/// `resourceId` and `quotaCode` stay unset: neither is a value this emulator knows,
/// and the resource the refused create would have minted has no id.
fn quota_exceeded(message: String) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(message),
        resource_type: Some(INVESTIGATION_GROUP_RESOURCE.to_string()),
        service_code: Some(ARN_SERVICE.to_string()),
        ..Default::default()
    }
}

/// The 402 a `CreateInvestigationGroup` is refused with when the Region it is signed
/// for already holds this account's one investigation group.
///
/// The quota is published on the operation itself: "Currently, you can have one
/// investigation group in each Region in your account." The MESSAGE, though, states
/// none of that: recorded behavior is the fixed sentence
/// [`CANNOT_CREATE_ANOTHER_GROUP`], which names no count, no Region and nothing the
/// request carried — so the cap this emulator enforces
/// ([`MAX_INVESTIGATION_GROUPS_PER_REGION`]) stays out of it and the wording cannot
/// become a function of emulator configuration.
pub fn region_quota_exceeded() -> ServiceQuotaExceededException {
    quota_exceeded(CANNOT_CREATE_ANOTHER_GROUP.to_string())
}

/// The sentence a create refused by the per-account/Region investigation-group quota
/// carries, as the service words that error class: entirely fixed text.
pub const CANNOT_CREATE_ANOTHER_GROUP: &str = "Cannot create another investigation group.";

/// The 402 a create is refused with when the store has reached its cross-region
/// resource guard, [`MAX_INVESTIGATION_GROUPS`].
pub fn store_capacity_exceeded() -> ServiceQuotaExceededException {
    quota_exceeded(format!(
        "this account already holds the maximum of {MAX_INVESTIGATION_GROUPS} investigation groups"
    ))
}

/// Whether `text` has the shape of an investigation-group id.
///
/// One modelled pattern segment decides it: the last segment of
/// `InvestigationGroupArn` is `[A-Za-z0-9]{16}`
/// (`smithy_query(query="shape",shape="InvestigationGroupArn")`). Everything that has
/// to judge whether some text could name a stored group — the ARN resolver below and
/// the opaque page token a regional reader hands back — judges it here, so the rule is
/// read off that pattern in exactly one place.
pub fn is_group_id(text: &str) -> bool {
    text.len() == GROUP_ID_LEN && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The id segment of an investigation-group ARN, or `None` when `identifier` is not
/// an ARN of this service's investigation-group resource.
///
/// The service namespace is the THIRD field of a complete ARN and the resource the
/// SIXTH, both read off the one split ([`arn_fields`]).
pub fn group_id_from_arn(identifier: &str) -> Option<&str> {
    let [_arn, _partition, service, _region, _account, resource] = arn_fields(identifier)?;
    if service != ARN_SERVICE {
        return None;
    }
    let id = resource.strip_prefix(GROUP_ARN_RESOURCE_PREFIX)?;
    is_group_id(id).then_some(id)
}

/// The ARN of the investigation group `id` in `region`.
pub fn group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{GROUP_ARN_RESOURCE_PREFIX}{id}"))
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only `[a-zA-Z0-9-]*` in that field, so anything else is
/// refused here and the default is used instead: a generated ARN can never carry
/// text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The principal that made this request, for `createdBy`/`lastModifiedBy`.
///
/// Derived from the request's own SigV4 access key id, so the two members report
/// who called rather than a fabricated identity. That id is caller-controlled text,
/// so it is first reduced to the characters an access key id can hold; the rendering
/// then satisfies `IdentifierStringWithPatternAndLengthLimits`
/// (`^[\-_\/A-Za-z0-9:\.]+$`, 1..512) by construction, whatever the header said.
pub fn caller_principal(parts: &::http::request::Parts) -> String {
    let raw = ::engine::extract_akid(&parts.headers);
    let filtered: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let akid = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// Now, in whole seconds since the Unix epoch — the unit the modelled `createdAt` /
/// `lastModifiedAt` longs carry.
pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `[A-Za-z0-9]{16}` id from `seed`.
fn group_id_from_seed(seed: u64) -> String {
    let mut state = seed;
    (0..GROUP_ID_LEN)
        .map(|_| {
            let draw = splitmix64(&mut state);
            GROUP_ID_ALPHABET[(draw % GROUP_ID_ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAiopsAiopsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAiopsAiopsState>)
}
