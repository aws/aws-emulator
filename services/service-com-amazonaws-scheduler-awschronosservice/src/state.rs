// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-scheduler-awschronosservice service.
//!
//! EventBridge Scheduler's control plane is two nested resources: a
//! `ScheduleGroup` (named, holds tags) and the `Schedule`s inside it. Both live
//! here, keyed so the ordered maps enumerate deterministically — which is what
//! lets `ListSchedules` resume from an opaque page token.
//!
//! The rules a schedule's own definition has to satisfy — its expression, the
//! zone that expression is read in, and the window its instants bound — live in
//! [`schedule_definition`] and are re-exported here, so both operations that
//! write a schedule reach them the way they reach the rest of this module. The
//! FORM of the ARN its target names — the one ARN-valued member of a schedule
//! the model leaves without a `@pattern` — is [`target_arn`], and the values the
//! service FILLS IN for the optional members a write leaves out, so a stored
//! schedule is its effective configuration, are [`server_defaults`].
//!
//! What the control plane persists is only half of this service: a schedule is
//! also INVOKED. Which instants one runs at lives in [`occurrence`], and the clock
//! that performs them — and the hand-over that reaches each target — in
//! [`invocation`], which the operations that write a schedule arm. What becomes of
//! an invocation that did not reach its target — the attempts its
//! retry policy admits, and the dead-letter route the last of them takes — is
//! [`dead_letter`]. A target that names an API OPERATION rather than a resource — a
//! UNIVERSAL target — is [`universal_target`]. A target naming an EVENT BUS is a
//! templated EventBridge target, whose own kind and the parameters it carries are
//! [`event_bridge_target`]. A target naming an ECS CLUSTER is a templated ECS
//! target, whose parameters carry one requirement across two members:
//! [`ecs_target`]. The SIZE of the payload a target is handed —
//! `Target$Input`'s service maximum, which the model does not state — is
//! [`target_input`].
//!
//! What a client can see of the invocations a store has MADE, beside the effect each
//! had on its target — the counts EventBridge Scheduler publishes about its own
//! deliveries, their failures and their dead-letter route — is [`schedule_metrics`].
//!
//! The customer managed key a write states is a reference to a resource of ANOTHER
//! service, so it is resolved against that service as the write takes it: [`kms_key`].
//!
//! Whether the CALLER may hand this service the execution role its request names at
//! all — the `iam:PassRole` every operation that carries a `Target.RoleArn` charges
//! against the calling principal's own policies — is [`pass_role`]. Whether the caller may
//! make the request AT ALL — the `scheduler:<OperationName>` IAM charges every request
//! against those same policies — is [`caller_authorization`], and the one reading of the
//! principal both stand on is [`caller_identity`].

mod dead_letter;
mod ecs_target;
mod event_bridge_target;
mod kms_key;
mod occurrence;
mod schedule_definition;
mod server_defaults;
mod target_arn;
mod target_input;
mod universal_target;

pub mod caller_authorization;
pub mod caller_identity;
pub mod invocation;
pub mod pass_role;
pub mod schedule_metrics;

pub use caller_authorization::{caller_denial, ANY_RESOURCE};
pub use dead_letter::RETRY_INTERVAL;
pub(crate) use ecs_target::ecs_target_violation;
pub(crate) use event_bridge_target::event_bridge_target_violation;
pub use invocation::{drive_invocation_clock, Invocations};
pub use kms_key::{kms_key_refusal, kms_key_violation};
pub use occurrence::Occurrences;
pub use schedule_definition::*;
pub use schedule_metrics::ScheduleMetrics;
pub use server_defaults::{
    fill_server_defaults, DEFAULT_ACTION_AFTER_COMPLETION, DEFAULT_MAXIMUM_EVENT_AGE_IN_SECONDS,
    DEFAULT_MAXIMUM_RETRY_ATTEMPTS, DEFAULT_SCHEDULE_EXPRESSION_TIMEZONE,
};
pub use target_arn::target_arn_violation;
pub use target_input::{target_input_violation, MAX_TARGET_INPUT_BYTES};
pub use universal_target::universal_target_violation;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{FlexibleTimeWindow, Tag, Target};

/// The schedule group every schedule lands in when a request names none.
///
/// AWS creates this group with the account: it is never created by a client and
/// no client may delete it, so every operation that would create, modify or
/// delete the group of this name is refused by
/// [`reserved_group_refusal`] before any existence check runs.
pub const DEFAULT_SCHEDULE_GROUP: &str = "default";

/// How long after `DeleteScheduleGroup` a group's removal becomes due.
///
/// `DeleteScheduleGroup`: "When you delete a group, it remains in a DELETING
/// state until all of its associated schedules are deleted. … This operation is
/// eventually consistent." AWS documents no duration for that window, so this
/// span is CHOSEN: long enough that a caller reading straight after the delete
/// still observes the `DELETING` group (and the state errors that go with it),
/// short enough for a test to wait it out. Nothing sleeps for it — the due
/// instant is stored and [`ComAmazonawsSchedulerAwschronosserviceState::settle`]
/// materialises the removal the clock already owed.
pub const GROUP_DELETION_DELAY: Duration = Duration::from_millis(500);

/// The region an ARN carries when the request is unsigned, so no SigV4
/// credential scope names one.
pub const FALLBACK_REGION: &str = "us-east-1";

/// `FlexibleTimeWindowMode` value that asks for a window.
pub(crate) const FLEXIBLE_TIME_WINDOW_MODE_FLEXIBLE: &str = "FLEXIBLE";

/// The modelled name of the `FlexibleTimeWindow` member a cross-member refusal
/// is about (`com.amazonaws.scheduler#FlexibleTimeWindow$MaximumWindowInMinutes`).
const MAXIMUM_WINDOW_MEMBER: &str = "MaximumWindowInMinutes";

/// The modelled name of the enum that carries the mode such a refusal conflicts
/// with (`com.amazonaws.scheduler#FlexibleTimeWindowMode`), which is what the
/// service reports rather than the `Mode` member that targets it.
const FLEXIBLE_TIME_WINDOW_MODE_SHAPE: &str = "FlexibleTimeWindowMode";

/// The `ScheduleGroupState` a usable group reports.
///
/// `ScheduleGroupState` is modelled `ACTIVE | DELETING`, and AWS returns a
/// newly created group ACTIVE, so only `DeleteScheduleGroup` ever moves one.
pub const SCHEDULE_GROUP_STATE_ACTIVE: &str = "ACTIVE";

/// The `ScheduleGroupState` of a group whose deletion the clock has not yet
/// finished: `DeleteScheduleGroup` moves a group here, and nothing else does.
pub const SCHEDULE_GROUP_STATE_DELETING: &str = "DELETING";

/// The `ScheduleState` a schedule takes when the request omits `State`.
pub const DEFAULT_SCHEDULE_STATE: &str = "ENABLED";

/// Largest number of schedule groups one account/Region holds.
///
/// The EventBridge Scheduler service quota "Schedule groups per Region"
/// (default 500). It is also what bounds the store: a `CreateScheduleGroup`
/// past it answers the modelled `ServiceQuotaExceededException` instead of
/// growing memory without limit.
pub const MAX_SCHEDULE_GROUPS: usize = 500;

/// Largest number of schedules one account/Region holds.
///
/// The EventBridge Scheduler service quota "Schedules per Region" (default
/// 1,000,000), and likewise the store's bound: a `CreateSchedule` past it
/// answers the modelled `ServiceQuotaExceededException`.
pub const MAX_SCHEDULES: usize = 1_000_000;

/// Largest page `ListSchedules` returns, and the page size it uses when the
/// request names none.
///
/// `ListSchedulesInput$MaxResults` is modelled `range 1..100`, so no client can
/// ask for a bigger page; applying the same maximum to a request that omits
/// `MaxResults` means one call can never serialize more than this many records.
pub const MAX_LIST_SCHEDULES_PAGE: usize = 100;

/// Largest page `ListScheduleGroups` returns, and the page size it uses when
/// the request names none.
///
/// `ListScheduleGroupsInput$MaxResults` is modelled `range 1..100`, exactly as
/// `ListSchedulesInput$MaxResults` is, so the same bound holds.
pub const MAX_LIST_SCHEDULE_GROUPS_PAGE: usize = MAX_LIST_SCHEDULES_PAGE;

/// The tag-key prefix AWS keeps for itself.
///
/// "You cannot edit or delete tag keys or values with this prefix. Tags with
/// this prefix do not count against your tags per resource limit" — the AWS
/// tagging restrictions every tagging API shares, so a request that supplies
/// such a key is refused.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What a request naming a tag key in the reserved namespace is refused with.
///
/// A key under [`RESERVED_TAG_KEY_PREFIX`] belongs to the service, not to the
/// caller, so the refusal is an authorization statement about that namespace —
/// the caller may not modify system tags — and NOT a complaint about the key's
/// syntax. It therefore names neither the key nor the prefix: the text is the
/// same for every reserved key, and for every entry point that takes tags.
pub const SYSTEM_TAG_REFUSAL_MESSAGE: &str =
    "Caller is an end user and not allowed to modify system tags.";

/// Largest number of tags one schedule group holds.
///
/// `TagList` (what `TagResource$Tags` sends and `ListTagsForResourceOutput$Tags`
/// answers) is modelled `length 0..200`, so a group that held more could not be
/// reported: a merge past the bound is refused instead.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The ARN infix that separates a schedule group's ARN prefix from its name.
const SCHEDULE_GROUP_ARN_INFIX: &str = ":schedule-group/";

/// The name of the schedule group `arn` identifies.
///
/// `TagResourceArn` is modelled `^arn:aws[a-z-]*:scheduler:[a-z0-9\-]+:\d{12}:schedule-group\/[0-9a-zA-Z-_.]+$`,
/// so the name is everything after the resource type — the boundary validator has
/// already refused an ARN of any other shape (a schedule's, say) by the time a
/// handler runs.
pub fn schedule_group_name_from_arn(arn: &str) -> Option<&str> {
    arn.split_once(SCHEDULE_GROUP_ARN_INFIX)
        .map(|(_, name)| name)
        .filter(|name| !name.is_empty() && !name.contains('/'))
}

/// Whether any of `keys` is in the reserved namespace, which is what makes a
/// request a modification of system tags. The comparison is case-insensitive on
/// the prefix: `AWS:` is reserved exactly as `aws:` is.
///
/// A predicate rather than the offending key, because the refusal
/// ([`SYSTEM_TAG_REFUSAL_MESSAGE`]) does not report which key it was.
///
/// See [`RESERVED_TAG_KEY_PREFIX`].
pub fn names_system_tag<'a, I: IntoIterator<Item = &'a str>>(keys: I) -> bool {
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    keys.into_iter().any(|key| {
        key.as_bytes()
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}

/// A `ScheduleGroup` as this emulator persists it.
///
/// A group's two instants are the ones a REQUEST made: `creation_date` is when
/// a create call minted the group and `last_modification_date` when a call last
/// changed it. The group the service provisions with the account was minted by
/// no call, so it holds neither, and a reader reports only the instants the
/// record actually holds — both modelled members are optional.
#[derive(Debug, Clone)]
pub struct ScheduleGroupRecord {
    pub name: String,
    pub arn: String,
    /// `ScheduleGroupState`: `ACTIVE` | `DELETING`.
    pub state: String,
    /// When a create call minted this group; `None` for a group no call minted.
    pub creation_date: Option<SystemTime>,
    /// When a call last changed this group; `None` for a group no call has
    /// created or changed.
    pub last_modification_date: Option<SystemTime>,
    /// Carried for `TagResource`/`UntagResource`/`ListTagsForResource`.
    pub tags: Vec<Tag>,
    /// When this group's removal becomes due, set by `DeleteScheduleGroup`.
    /// `None` for a group nobody has deleted — the only state in which the
    /// group accepts new schedules.
    pub deletion_due: Option<SystemTime>,
}

impl ScheduleGroupRecord {
    /// Merge `tags` into the group's tag set and answer whether anything changed.
    ///
    /// A key the group does not carry is appended; a key it does carry keeps its
    /// position and takes the new value — tagging a resource with a key already
    /// on it replaces that key's value rather than adding a second pair, which is
    /// what makes a tag set a map keyed by tag key.
    pub fn merge_tags(&mut self, tags: impl IntoIterator<Item = Tag>) -> bool {
        let mut changed = false;
        for tag in tags {
            match self.tags.iter_mut().find(|held| held.key == tag.key) {
                Some(held) => {
                    if held.value != tag.value {
                        held.value = tag.value;
                        changed = true;
                    }
                }
                None => {
                    self.tags.push(tag);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Drop every tag whose key is in `keys` and answer whether anything changed.
    ///
    /// A key the group does not carry is not an error: `UntagResource` is modelled
    /// `@idempotent`, so removing a key twice has the same effect as removing it
    /// once.
    pub fn remove_tags<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) -> bool {
        let keys: BTreeSet<&str> = keys.into_iter().collect();
        let before = self.tags.len();
        self.tags.retain(|tag| !keys.contains(tag.key.as_str()));
        self.tags.len() != before
    }
}

/// The distinct keys a resource holding `held` would carry once `incoming` were
/// merged in — what [`MAX_TAGS_PER_RESOURCE`] bounds.
pub fn tag_keys_after<'a>(held: &'a [Tag], incoming: &'a [Tag]) -> BTreeSet<&'a str> {
    held.iter()
        .chain(incoming.iter())
        .map(|tag| tag.key.as_str())
        .collect()
}

/// A `Schedule` as this emulator persists it.
///
/// `start_date`/`end_date` keep the instant the client sent, at the resolution the
/// service holds it ([`to_millisecond_precision`]), so a `GetSchedule` hands back
/// the instant `CreateSchedule` was given;
/// `creation_date`/`last_modification_date` are server clock readings, so they
/// are `SystemTime` and can be compared.
#[derive(Debug, Clone)]
pub struct ScheduleRecord {
    pub name: String,
    pub group_name: String,
    pub arn: String,
    pub schedule_expression: String,
    pub schedule_expression_timezone: Option<String>,
    pub start_date: Option<::aws_smithy_types::DateTime>,
    pub end_date: Option<::aws_smithy_types::DateTime>,
    pub description: Option<String>,
    /// `ScheduleState`: `ENABLED` | `DISABLED`.
    pub state: String,
    pub kms_key_arn: Option<String>,
    /// `ActionAfterCompletion`: `DELETE` | `NONE`.
    pub action_after_completion: Option<String>,
    pub target: Target,
    pub flexible_time_window: FlexibleTimeWindow,
    pub creation_date: SystemTime,
    pub last_modification_date: SystemTime,
}

/// Per-service state for com-amazonaws-scheduler-awschronosservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSchedulerAwschronosserviceState {
    /// Groups by name. The implicit `default` group is not seeded: it exists for
    /// every account, so membership is answered by [`Self::group_exists`] rather
    /// than by a row, and a row for it appears only once a request has to STORE
    /// something on it (its tags). An operation that enumerates groups therefore
    /// reports `default` whether or not this map holds a row for it.
    pub groups: BTreeMap<String, ScheduleGroupRecord>,
    /// Schedules by `(group name, schedule name)` — the pair that identifies a
    /// schedule, and an ordering `ListSchedules` pages through.
    pub schedules: BTreeMap<(String, String), ScheduleRecord>,
    /// The clock that invokes this account's schedules as their expressions make
    /// them due, and what it has already performed ([`invocation`]).
    pub invocations: Invocations,
}

impl Default for ComAmazonawsSchedulerAwschronosserviceState {
    fn default() -> Self {
        Self {
            groups: BTreeMap::new(),
            schedules: BTreeMap::new(),
            invocations: Invocations::default(),
        }
    }
}

impl ComAmazonawsSchedulerAwschronosserviceState {
    /// Whether `name` names a group this account holds — including one still in
    /// `DELETING`, which is addressable until its removal falls due.
    pub fn group_exists(&self, name: &str) -> bool {
        name == DEFAULT_SCHEDULE_GROUP || self.groups.contains_key(name)
    }

    /// Whether `name` names a group whose deletion is under way.
    pub fn group_is_deleting(&self, name: &str) -> bool {
        self.groups
            .get(name)
            .is_some_and(|group| group.deletion_due.is_some())
    }

    /// Materialise every group removal the clock has already made due, together
    /// with the schedules that group held (`DeleteScheduleGroup`: "Deleting a
    /// schedule group results in EventBridge Scheduler deleting all schedules
    /// associated with the group").
    ///
    /// Every operation calls this first, so a caller never reads a group whose
    /// `DELETING` window has elapsed, and nothing is removed before the instant
    /// the delete call made it due.
    pub fn settle(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .groups
            .values()
            .filter(|group| group.deletion_due.is_some_and(|due| due <= now))
            .map(|group| group.name.clone())
            .collect();
        for name in due {
            self.groups.remove(&name);
            self.schedules.retain(|(group, _), _| group != &name);
        }
    }

    /// The group `name` addresses, or `None` when no such group exists.
    ///
    /// Every account has the `default` group without having created it, so it is
    /// synthesised rather than read from a stored row — the same membership rule
    /// [`Self::group_exists`] applies, as one record a reader can serialize.
    /// `region` is the request's region, which the ARN of a group no create call
    /// minted carries, and no call minted it, so it carries neither instant.
    pub fn group_record(&self, region: &str, name: &str) -> Option<ScheduleGroupRecord> {
        if let Some(record) = self.groups.get(name) {
            return Some(record.clone());
        }
        if name == DEFAULT_SCHEDULE_GROUP {
            return Some(ScheduleGroupRecord {
                name: DEFAULT_SCHEDULE_GROUP.to_string(),
                arn: schedule_group_arn(region, DEFAULT_SCHEDULE_GROUP),
                state: SCHEDULE_GROUP_STATE_ACTIVE.to_string(),
                creation_date: None,
                last_modification_date: None,
                tags: Vec::new(),
                // The built-in group is never deleted: `DeleteScheduleGroup` on
                // the reserved name is refused, so no removal is ever due.
                deletion_due: None,
            });
        }
        None
    }

    /// Every schedule group this account holds, in `ScheduleGroupName` order:
    /// the stored rows plus the implicit `default` group in its sorted place,
    /// as the one record [`Self::group_record`] would hand a reader.
    ///
    /// One total order over the whole set is what lets a page token be a
    /// POSITION in it rather than an index into a list that may have changed
    /// between calls.
    pub fn groups_in_name_order(&self, region: &str) -> Vec<ScheduleGroupRecord> {
        let mut all: Vec<ScheduleGroupRecord> = self.groups.values().cloned().collect();
        // The built-in group is listed from its stored row once one exists — a
        // write such as tagging it materialises one — and synthesised otherwise,
        // so it appears exactly once either way.
        if !self.groups.contains_key(DEFAULT_SCHEDULE_GROUP) {
            all.extend(self.group_record(region, DEFAULT_SCHEDULE_GROUP));
        }
        all.sort_by(|left, right| left.name.cmp(&right.name));
        all
    }

    /// The name of the schedule group `arn` identifies in this account and
    /// Region, or `None` when no group there answers to that ARN.
    ///
    /// An ARN is a resource's identity, so it matches only the group whose own
    /// ARN it equals: a well-formed ARN naming another partition, Region or
    /// account addresses no group reachable from this endpoint. Resolution goes
    /// through [`Self::group_record`], so the account's built-in `default` group
    /// answers to its ARN with no row stored for it.
    pub fn group_name_for_arn(&self, arn: &str, region: &str) -> Option<String> {
        let name = schedule_group_name_from_arn(arn)?;
        let record = self.group_record(region, name)?;
        (record.arn == arn).then(|| name.to_string())
    }

    /// The tags held by `name`, which is empty for a group that carries none.
    pub fn group_tags(&self, name: &str) -> &[Tag] {
        self.groups
            .get(name)
            .map_or(&[][..], |record| record.tags.as_slice())
    }

    /// The record tags are stored on for the group `name`, materialising the row
    /// the account's built-in `default` group needs on its first write.
    ///
    /// Only a caller that already resolved `name` through [`Self::group_name_for_arn`]
    /// reaches this, so the group exists — and every group a create call minted
    /// already has a row, so the one row this can add is the `default` group's.
    /// That row holds no creation instant, the same as the record
    /// [`Self::group_record`] synthesises for it: materialising a row to hold
    /// tags is not a create call, so it mints no date a reader would see.
    pub fn group_for_write(&mut self, name: &str, region: &str) -> &mut ScheduleGroupRecord {
        self.groups
            .entry(name.to_string())
            .or_insert_with(|| ScheduleGroupRecord {
                name: name.to_string(),
                arn: schedule_group_arn(region, name),
                // The built-in group AWS creates with the account is ACTIVE; this
                // row only starts holding its tags.
                state: SCHEDULE_GROUP_STATE_ACTIVE.to_string(),
                creation_date: None,
                last_modification_date: None,
                tags: Vec::new(),
                // Nothing has deleted this group: the built-in one cannot be
                // deleted at all, and any other name already has its row.
                deletion_due: None,
            })
    }
}

/// The refusal for an operation addressed at a RESERVED schedule group — the
/// built-in group every account owns — or `None` for any other name.
///
/// EventBridge Scheduler owns the `default` group: a client can neither create
/// it (it already exists for every account) nor delete it, and the refusal is a
/// request-validation failure about the NAME, evaluated before any existence or
/// conflict check, rather than a collision with the resource. The message is the
/// one AWS is recorded answering; its code (`ValidationException`) and status
/// (400) are what a client branches on.
pub fn reserved_group_refusal(name: &str) -> Option<String> {
    (name == DEFAULT_SCHEDULE_GROUP)
        .then(|| format!("This operation cannot be performed on schedule group {name}."))
}

/// The `ConflictException` message for a request that would have MODIFIED the
/// schedule group `arn` while another writer held it.
///
/// A create that carries tags writes the group's tags as well as minting it, so a
/// name a group already holds is a collision with a group that is being modified
/// rather than a name that is merely taken: the refusal is the concurrent
/// modification of the group it collided with, and it names that group by its ARN —
/// the identity of the resource the request met, which the name alone does not
/// carry.
pub fn group_modified_concurrently(arn: &str) -> String {
    format!("Schedule group {arn} was modified concurrently.")
}

/// The `ConflictException` message for a create addressed at a name a schedule
/// group already holds, which carries nothing to modify on it.
pub fn group_already_exists(name: &str) -> String {
    format!("Schedule group {name} already exists.")
}

/// The `ResourceNotFoundException` message for a group this account does not
/// hold, worded as AWS words it.
pub fn group_does_not_exist(name: &str) -> String {
    format!("Schedule group {name} does not exist.")
}

/// The `ResourceNotFoundException` message for a request addressed by the ARN of
/// a schedule group this account does not hold.
///
/// The resource the request named is a GROUP, so the refusal is the group's own
/// ([`group_does_not_exist`]) and names the group rather than the ARN that
/// carried it: a tagging operation is addressed by ARN, but what is missing is
/// the group that ARN identifies, and the name is the part of the ARN a caller
/// knows the resource by. An ARN this service cannot read a group name out of —
/// one of another resource type, which the modelled `@pattern` on the member
/// already refuses before a handler runs — has no name to report, so the value
/// as submitted is what the refusal names.
pub fn group_arn_does_not_exist(arn: &str) -> String {
    group_does_not_exist(schedule_group_name_from_arn(arn).unwrap_or(arn))
}

/// The `ValidationException` message for a request addressed at a schedule group
/// whose deletion is under way.
///
/// "When you delete a group, it remains in a DELETING state until all of its
/// associated schedules are deleted" (`DeleteScheduleGroup`): the group is on its
/// way out, so it holds no schedule a write could address — which is a statement
/// about the request, not a conflict a client may retry.
///
/// The leading clause is part of the refusal AWS words: it is what tells a client
/// the group is on its way out and may be re-created, rather than that some other
/// input was wrong.
pub fn group_is_being_deleted(name: &str) -> String {
    format!("You cannot perform this operation because schedule group {name} is being deleted.")
}

/// The `ResourceNotFoundException` message for a schedule the addressed group
/// does not hold.
///
/// It names the schedule the request named and nothing else: the group is how
/// the schedule was addressed, not a second resource the refusal reports.
pub fn schedule_does_not_exist(name: &str) -> String {
    format!("Schedule {name} does not exist.")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSchedulerAwschronosserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSchedulerAwschronosserviceState>)
}

/// Whether `value` is shaped like a Region name: non-empty, and made only of
/// the characters every ARN in this service's model spells its Region segment
/// with (`[a-z0-9\-]+`).
///
/// The Region a request carries is interpolated into an ARN and into the host
/// of the endpoint an in-process call to another service is addressed to, so a
/// value outside that class is not used at all — nothing a request sends can
/// reshape either.
fn is_region_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region this request was signed for, which is the region its ARNs carry.
///
/// A credential scope that names no Region — or one that is not shaped like a
/// Region name — leaves [`FALLBACK_REGION`], so every caller of this gets a
/// value from [`is_region_name`]'s class.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_name(region))
        .unwrap_or_else(|| FALLBACK_REGION.to_string())
}

/// `arn:aws:scheduler:<region>:<account>:schedule-group/<name>`.
pub fn schedule_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("scheduler", region, &format!("schedule-group/{name}"))
}

/// `arn:aws:scheduler:<region>:<account>:schedule/<group>/<name>`.
pub fn schedule_arn(region: &str, group_name: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "scheduler",
        region,
        &format!("schedule/{group_name}/{name}"),
    )
}

/// The group a request addresses: the one it names, else the implicit `default`.
pub fn effective_group_name(group_name: Option<&str>) -> String {
    group_name
        .filter(|name| !name.is_empty())
        .unwrap_or(DEFAULT_SCHEDULE_GROUP)
        .to_string()
}

/// A server clock reading as the wire `DateTime` the timestamp members carry.
pub fn to_wire_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The IAM service principal EventBridge Scheduler assumes a schedule's
/// execution role as (`Target$RoleArn`: "the IAM role that EventBridge
/// Scheduler will use for this target when the schedule is invoked").
pub const SCHEDULER_SERVICE_PRINCIPAL: &str = "scheduler.amazonaws.com";

/// The IAM store the calling principal's own policies are read from.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so this service reads the very records
/// `CreateUser`, `CreateRole`, `PutRolePolicy` and `AttachRolePolicy` wrote rather
/// than a second copy of them. What a schedule's own execution ROLE is resolved
/// against is a different question, asked of IAM over the in-process callback
/// ([`execution_role_violation`]), because a target's role is resolved exactly as any
/// client would resolve it.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's identities
/// and their policies live. `None` means the engine carries no such store — a
/// deployment with no IAM to ask, which is evidence of nothing.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// What AWS answers when a schedule's execution role cannot be assumed — the
/// role does not exist, or its trust policy does not admit Scheduler.
pub const EXECUTION_ROLE_REFUSAL: &str =
    "The execution role you provide must allow AWS EventBridge Scheduler to assume the role.";

/// The role name an IAM role ARN carries, when the ARN names a role of THIS
/// account: everything after the last `/`, so a role under a path
/// (`:role/service-role/<name>`) resolves to the name IAM holds it under.
///
/// `None` when the value is not an IAM role ARN of this account — a malformed
/// ARN (which the modelled `@pattern` on `Target$RoleArn` has already refused by
/// the time a handler runs) or one naming another account, whose roles this
/// account's Scheduler cannot assume ("the execution role must be in the same
/// account as the schedule", EventBridge Scheduler's execution-role reference).
fn iam_role_name(role_arn: &str) -> Option<&str> {
    let mut fields = role_arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(_partition),
        Some("iam"),
        Some(""),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if account != ::engine::emulator_account_id() {
        return None;
    }
    let name = resource.strip_prefix("role/")?.rsplit('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// Whether an IAM trust-policy document lets `principal` assume the role.
///
/// The rule is IAM's, so it is read through the ONE evaluator every service that resolves
/// a caller-supplied execution role shares (`service_common::iam_trust_policy`): a
/// statement is about the call when it names the principal and the `sts:AssumeRole` action
/// (either member being one string or a list, compared case-insensitively), an explicit
/// `Deny` wins over any `Allow`, and the document is percent-decoded before it is parsed.
///
/// A document this emulator cannot read as a policy is not one Scheduler can act on, so it
/// does not admit the assume — the refusal a resolved role with no readable grant earns.
fn trust_policy_admits(document: &str, principal: &str) -> bool {
    ::service_common::iam_trust_policy::admits_service_principal(document, principal)
        .unwrap_or(false)
}

/// Refuse a target whose execution role EventBridge Scheduler could not assume.
///
/// `Target$RoleArn` is "the IAM role that EventBridge Scheduler will use for this target
/// when the schedule is invoked", so the role is resolved WHERE IT IS TAKEN UP: when a
/// schedule is CREATED, which is refused with 400 `ValidationException` and changes
/// nothing unless the role exists in this account and its trust policy lets the Scheduler
/// service principal assume it, and again at each OCCURRENCE ([`invocation`]), where a
/// role that can no longer be assumed leaves the invocation undelivered. An UPDATE does
/// not resolve it: the schedule's new configuration is applied with the role it states,
/// which is what lets a client re-point a schedule off a role that has been deleted.
///
/// IAM is reached the way one emulated service reaches another's API: through the
/// in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so the
/// role is resolved by the very handlers a client's own `GetRole` would reach.
///
/// `None` means the schedule may be written. That includes the case where this
/// emulator has no IAM to ask (a router with only this service mounted, say):
/// nothing is then known about the role, and a refusal has to be grounded in
/// what IAM answered, never assumed.
///
/// It takes the in-process `callback` rather than the operation's whole context,
/// because the other caller is the invocation clock ([`invocation`]), which
/// resolves the same role when the schedule is INVOKED and holds a callback cloned
/// from the request that started it rather than a request of its own.
pub async fn execution_role_violation(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
) -> Option<String> {
    let Some(role_name) = iam_role_name(role_arn) else {
        return Some(EXECUTION_ROLE_REFUSAL.to_string());
    };
    // The Region names the HOST this call is addressed to, so only a value from
    // the Region class reaches it; anything else resolves to the fallback.
    let region = if is_region_name(region) {
        region
    } else {
        FALLBACK_REGION
    };
    let config = ::aws_sdk_iam::Config::builder()
        .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_iam::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://iam.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    let answer = ::aws_sdk_iam::Client::from_conf(config)
        .get_role()
        .role_name(role_name)
        .send()
        .await;
    match answer {
        Ok(output) => {
            let trusted = output
                .role()
                .and_then(::aws_sdk_iam::types::Role::assume_role_policy_document)
                .is_some_and(|document| {
                    trust_policy_admits(document, SCHEDULER_SERVICE_PRINCIPAL)
                });
            (!trusted).then(|| EXECUTION_ROLE_REFUSAL.to_string())
        }
        // IAM says this account holds no such role: the refusal is grounded.
        Err(error) if role_is_absent(&error) => Some(EXECUTION_ROLE_REFUSAL.to_string()),
        // Any other answer is this emulator failing to ask, not IAM reporting a
        // bad role, so it refuses nothing — and it is worth a warning, because a
        // deployment with no IAM to ask is one where this gate does not run.
        Err(error) => {
            ::tracing::warn!(
                %role_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the execution role could not be verified against IAM"
            );
            None
        }
    }
}

/// Whether an IAM `GetRole` failure is IAM reporting that the role does not
/// exist (`NoSuchEntity`), as opposed to the call not reaching IAM.
fn role_is_absent(
    error: &::aws_sdk_iam::error::SdkError<
        ::aws_sdk_iam::operation::get_role::GetRoleError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception)
}

/// Refuse a `FlexibleTimeWindow` whose `MaximumWindowInMinutes` does not match
/// its `Mode`.
///
/// EventBridge Scheduler requires the maximum window exactly when the mode is
/// `FLEXIBLE` ("If the flexible time window mode is `FLEXIBLE`, you must specify
/// a `MaximumWindowInMinutes`" — the `FlexibleTimeWindow` reference). The bound
/// on the value itself (1..1440) is modelled, so the boundary validator has
/// already refused anything outside it by the time a handler runs.
///
/// The refusal names the offending member and the mode it conflicts with by the
/// names the model gives them — the member `MaximumWindowInMinutes` of
/// `FlexibleTimeWindow` and the enum `FlexibleTimeWindowMode` carrying the mode
/// the request sent — and no emulator-invented field path.
pub fn flexible_time_window_violation(window: &FlexibleTimeWindow) -> Option<String> {
    let flexible = window.mode == FLEXIBLE_TIME_WINDOW_MODE_FLEXIBLE;
    let obligation = match (flexible, window.maximum_window_in_minutes) {
        (true, None) => "must be provided",
        (false, Some(_)) => "must not be provided",
        _ => return None,
    };
    Some(format!(
        "{MAXIMUM_WINDOW_MEMBER} {obligation} when {FLEXIBLE_TIME_WINDOW_MODE_SHAPE} is set to {}.",
        window.mode
    ))
}

/// Encode a `ListSchedules` page token for the last `(group, name)` returned.
///
/// The token is opaque to a client but decodable here, so the next page resumes
/// immediately after that key in the ordered map — no index into a list that may
/// have changed between calls. Hex, not base64, so the token is URL-safe: it
/// travels in the `NextToken` QUERY parameter, where `+` and `/` would have to be
/// escaped by every client that echoes it back.
pub fn encode_page_token(group_name: &str, name: &str) -> String {
    let payload = ::serde_json::json!({ "g": group_name, "n": name });
    ::hex::encode(payload.to_string())
}

/// Decode a `ListSchedules` page token. `None` when it is not one this service
/// minted, which the caller turns into a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<(String, String)> {
    let bytes = ::hex::decode(token).ok()?;
    let value: ::serde_json::Value = ::serde_json::from_slice(&bytes).ok()?;
    let group = value.get("g")?.as_str()?.to_string();
    let name = value.get("n")?.as_str()?.to_string();
    Some((group, name))
}

/// Encode a `ListScheduleGroups` page token for the last group name returned.
///
/// Same encoding as [`encode_page_token`] — hex, so it survives the `NextToken`
/// query parameter unescaped — under its own payload key, so a token minted for
/// one listing never decodes as a position in the other.
pub fn encode_group_page_token(name: &str) -> String {
    let payload = ::serde_json::json!({ "sg": name });
    ::hex::encode(payload.to_string())
}

/// Decode a `ListScheduleGroups` page token. `None` when it is not one this
/// service minted, which the caller turns into a `ValidationException`.
pub fn decode_group_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let value: ::serde_json::Value = ::serde_json::from_slice(&bytes).ok()?;
    Some(value.get("sg")?.as_str()?.to_string())
}
