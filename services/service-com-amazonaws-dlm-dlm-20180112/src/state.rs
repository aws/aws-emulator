// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dlm-dlm-20180112 service.
//!
//! The lifecycle policy is the primary resource of the Amazon Data Lifecycle
//! Manager control plane: `CreateLifecyclePolicy` mints one,
//! `GetLifecyclePolicy` / `GetLifecyclePolicies` report it,
//! `UpdateLifecyclePolicy` mutates it, `DeleteLifecyclePolicy` removes it and the
//! three tag operations read and write its tags. [`LifecyclePolicyRecord`] below
//! is the ONE copy of a policy every one of those handlers reads and writes, plus
//! the two projections the model asks for (`LifecyclePolicy` and
//! `LifecyclePolicySummary`).

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    CreateRule, CrossRegionCopyTarget, DeprecateRule, Exclusions, FastRestoreRule,
    InternalServerException, InvalidRequestException, LifecyclePolicy, LifecyclePolicySummary,
    LimitExceededException, PolicyDetails, ResourceNotFoundException, RetainRule,
    RetentionArchiveTier, Schedule, Tag,
};

/// The documented floor on the creation frequency of a schedule that archives
/// the snapshots it creates — the second bullet of `CreateRule`'s note — which
/// [`schedule_rule_violation`] judges beside the form rules.
mod archive_creation_frequency;

pub use archive_creation_frequency::{
    archive_creation_frequency_violation, ARCHIVE_CREATION_FREQUENCY_MIN_DAYS,
};

/// The region a request belongs to — the policies it reaches, and the region a
/// policy ARN it mints names — when it carries no SigV4 credential scope to read
/// one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / sigv4 signing name this service's ARNs carry
/// (`smithy_query(query="service_info")`: `arnNamespace: dlm`).
pub const ARN_NAMESPACE: &str = "dlm";

/// The `ResourceType` a `ResourceNotFoundException` / `LimitExceededException`
/// reports for a lifecycle policy — "the type of resource that was not found".
///
/// `ResourceNotFoundException$ResourceType` is a WIRE token naming a kind of
/// Amazon Data Lifecycle Manager resource, which is what lets a client tell a
/// missing policy from another missing DLM resource; it is not prose and not an
/// internal name. The lifecycle policy's token is `LIFECYCLE_POLICY`.
pub const POLICY_RESOURCE_TYPE: &str = "LIFECYCLE_POLICY";

/// The `Code` every `InvalidRequestException` this service answers carries.
///
/// `InvalidRequestException$Code` is the service's own error code for the
/// exception class — the member a client branches on beside the shape name —
/// and Amazon Data Lifecycle Manager answers `InvalidParameter` for a request
/// whose parameters it refuses.
pub const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The `Code` every `ResourceNotFoundException` this service answers carries,
/// by the same rule as [`INVALID_PARAMETER_CODE`].
pub const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";

/// The `Code` every `LimitExceededException` this service answers carries, by
/// the same rule as [`INVALID_PARAMETER_CODE`]: the code names the CONDITION the
/// exception class is about, and the class is "the request failed because a limit
/// was exceeded" (`smithy_query(query="shape",
/// shape="LimitExceededException")`).
pub const LIMIT_EXCEEDED_CODE: &str = "LimitExceeded";

/// The `Code` every `InternalServerException` this service answers carries, by
/// the same rule: the class is "the service failed in an unexpected way"
/// (`smithy_query(query="shape", shape="InternalServerException")`), the
/// condition AWS spells `InternalError` wherever it reports a server-side
/// failure in an error code.
pub const INTERNAL_ERROR_CODE: &str = "InternalError";

/// The tag-key prefix AWS reserves for itself: a caller may not SET a tag whose
/// key begins with it, in any case.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// How Amazon Data Lifecycle Manager states the refusal of a reserved tag key.
pub const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// The policy type a policy carries when `PolicyDetails$PolicyType` names none:
/// "The default is `EBS_SNAPSHOT_MANAGEMENT`."
pub const DEFAULT_POLICY_TYPE: &str = "EBS_SNAPSHOT_MANAGEMENT";

/// The `PolicyTypeValues` member that "manages the lifecycle of EBS-backed
/// AMIs".
pub const IMAGE_POLICY_TYPE: &str = "IMAGE_MANAGEMENT";

/// The `DefaultPolicyTypeValues` member a DEFAULT policy names when what it
/// manages is EBS-backed AMIs: `DefaultPolicy` documents "Specify `INSTANCE` to
/// create a default policy that creates EBS-backed AMIs", against `VOLUME`,
/// which creates snapshots.
///
/// This is the type of a DEFAULT policy only. The identically spelled
/// `ResourceTypeValues` member a CUSTOM policy's `PolicyDetails$ResourceTypes`
/// may name says nothing about the policy's type: it is documented "[Custom
/// snapshot policies only] ... use `INSTANCE` to create multi-volume snapshots
/// from the volumes for an instance".
pub const INSTANCE_DEFAULT_POLICY_TYPE: &str = "INSTANCE";

/// The `DefaultPolicyTypeValues` member a DEFAULT policy names when what it
/// manages is EBS snapshots: `DefaultPolicy` documents "To create a default
/// policy for EBS snapshots, that creates snapshots of all volumes in the Region
/// that do not have recent backups, specify `VOLUME`".
pub const VOLUME_DEFAULT_POLICY_TYPE: &str = "VOLUME";

/// The `PolicyTypeValues` a DEFAULT policy can manage, in the order
/// `DefaultPolicyTypeValues` pairs them with: the snapshot management a `VOLUME`
/// default policy performs and the AMI management an `INSTANCE` one performs.
///
/// `EVENT_BASED_POLICY` is not among them: an event-based policy "performs
/// specific actions when a defined event occurs", which is neither of the two
/// kinds of default policy `DefaultPolicy` offers, so only a custom policy
/// manages it.
pub const DEFAULT_POLICY_MANAGED_TYPES: [&str; 2] = [DEFAULT_POLICY_TYPE, IMAGE_POLICY_TYPE];

/// The `PolicyLanguage` value that marks a policy as a DEFAULT policy:
/// `PolicyDetails$PolicyLanguage` documents `SIMPLIFIED` as "To create a default
/// policy" and `STANDARD` as "To create a custom policy".
pub const SIMPLIFIED_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The `PolicyLanguage` value that marks a policy as a CUSTOM one:
/// `PolicyDetails$PolicyLanguage` documents `STANDARD` as "To create a custom
/// policy".
pub const STANDARD_POLICY_LANGUAGE: &str = "STANDARD";

/// The policy type of an event-based policy: `PolicyDetails$PolicyType`
/// documents `EVENT_BASED_POLICY` as the type that "performs specific actions
/// when a defined event occurs". A policy of this type has no schedules and no
/// resource locations, so none of the custom snapshot/AMI defaults below belong
/// to it.
pub const EVENT_BASED_POLICY_TYPE: &str = "EVENT_BASED_POLICY";

/// Where a custom snapshot or AMI policy's targets live when the request states
/// no `PolicyDetails$ResourceLocations`, and where a schedule creates its
/// snapshots when it states no `CreateRule$Location`: both members name the
/// in-Region location `CLOUD` ("If the source resources are located in a Region,
/// specify `CLOUD`", and `CreateRule$Location` documents "Default: `CLOUD`").
pub const CLOUD_LOCATION: &str = "CLOUD";

/// Whether a schedule copies the source resource's tags when the request states
/// no `Schedule$CopyTags`: the member targets the model's non-nullable `CopyTags`
/// boolean (as against the `CopyTagsNullable` the optional copy-tags members
/// target), and `CopyTags` is documented "If you do not specify a value, the
/// default is false."
pub const DEFAULT_SCHEDULE_COPY_TAGS: bool = false;

/// Whether a cross-Region snapshot or AMI copy carries the source's user-defined
/// tags when the copy rule states no `CrossRegionCopyRule$CopyTags`.
///
/// The member has no documented default, but a read of a policy whose copy rule
/// stated none reports the flag ON: a cross-Region copy carries the source's tags
/// unless the rule turns that off. Like the members above, it is materialized at
/// create, so a rule that states `false` keeps `false`.
pub const DEFAULT_COPY_TAGS_ON_COPY: bool = true;

/// The value a schedule rule's interval carries when the request states none.
///
/// `CreateRule$Interval` and `RetainRule$Interval` come back as `0` on a read of
/// a policy whose schedule stated neither — a cron-driven creation rule and a
/// count-based retention rule respectively — so the service materializes the
/// integer zero for them rather than leaving them out. Only the two interval
/// members are stamped: nothing is invented for a member no recorded read shows
/// the service filling in.
pub const UNSET_RULE_INTERVAL: i32 = 0;

/// The creation frequency a default policy runs at when the request names none:
/// `CreateInterval` documents "If you do not specify a value, the default is 1."
pub const DEFAULT_CREATE_INTERVAL: i32 = 1;

/// How long a default policy retains snapshots or AMIs when the request names
/// no value: `RetainInterval` documents "If you do not specify a value, the
/// default is 7."
pub const DEFAULT_RETAIN_INTERVAL: i32 = 7;

/// Whether a default policy copies tags from the source resource when the
/// request names no value: `CopyTags` documents "If you do not specify a value,
/// the default is false."
pub const DEFAULT_COPY_TAGS: bool = false;

/// Whether a default policy extends deletion when the request names no value:
/// `ExtendDeletion` documents "If you do not specify a value, the default is
/// false."
pub const DEFAULT_EXTEND_DELETION: bool = false;

/// The documented bounds, in days, of a default policy's creation frequency:
/// `CreateInterval` is "how often the policy should run and create snapshots or
/// AMIs. The creation frequency can range from 1 to 7 days"
/// (`smithy_query(query="doc", shape="CreateLifecyclePolicyRequest",
/// member="CreateInterval")`, and the same sentence on
/// `UpdateLifecyclePolicyRequest` and `PolicyDetails`).
///
/// The member targets `CreateInterval`, which is `@range(min: 1)` only, so the
/// floor is enforced by the model and the CEILING is enforced here.
pub const CREATE_INTERVAL_MIN_DAYS: i32 = 1;
/// See [`CREATE_INTERVAL_MIN_DAYS`].
pub const CREATE_INTERVAL_MAX_DAYS: i32 = 7;

/// The documented bounds, in days, of a default policy's retention period:
/// `RetainInterval` is "how long the policy should retain snapshots or AMIs
/// before deleting them. The retention period can range from 2 to 14 days"
/// (`smithy_query(query="doc", shape="CreateLifecyclePolicyRequest",
/// member="RetainInterval")`, and the same sentence on
/// `UpdateLifecyclePolicyRequest` and `PolicyDetails`).
///
/// The member targets `RetainInterval`, which is `@range(min: 1)`: BOTH the
/// 2-day floor and the 14-day ceiling fall outside what the model expresses, so
/// both are enforced here.
pub const RETAIN_INTERVAL_MIN_DAYS: i32 = 2;
/// See [`RETAIN_INTERVAL_MIN_DAYS`].
pub const RETAIN_INTERVAL_MAX_DAYS: i32 = 14;

/// The values a custom schedule's creation interval supports: `CreateRule$Interval`
/// is "the interval between snapshots. The supported values are 1, 2, 3, 4, 6,
/// 8, 12, and 24" (`smithy_query(query="doc", shape="CreateRule",
/// member="Interval")`), with `CreateRule$IntervalUnit` offering `HOURS` alone.
///
/// The member targets `Interval`, which is `@range(min: 1)` — every positive
/// integer — so the SET is enforced here. The member hands its boundary
/// validation over entirely (the `skip_validation` note on
/// [`CreateRule`](crate::ops::types::CreateRule)), because the service
/// materializes [`UNSET_RULE_INTERVAL`] for a rule that stated no interval and
/// reports it on a read: that value has to reach the handler to be read as the
/// filler it is, and this set, being narrower than the modelled bound, is what
/// decides every value a request does state.
pub const SUPPORTED_CREATE_RULE_INTERVALS: [i32; 8] = [1, 2, 3, 4, 6, 8, 12, 24];

/// The least standard-tier retention a schedule may keep when it also carries a
/// rule that acts on the snapshots it keeps: `RetainRule` documents "If you
/// specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must
/// specify a retention count of `1` or more" and, for the age-based mode, "then
/// you must specify a retention interval of `1` day or more"
/// (`smithy_query(query="doc", shape="RetainRule")`).
///
/// `StandardTierRetainRuleCount` is `@range(min: 0, max: 1000)` and
/// `StandardTierRetainRuleInterval` is `@range(min: 0)`, so the model admits `0`
/// — it is admissible for a schedule that carries none of those rules — and this
/// conditional floor is enforced here.
pub const RETENTION_FLOOR_WITH_DEPENDENT_RULE: i32 = 1;

/// The error code Amazon Data Lifecycle Manager answers when the caller may not
/// pass the `ExecutionRoleArn` it named.
///
/// It is not one of the errors the model lists for the operations that take an
/// `ExecutionRoleArn` (those are `InternalServerException`,
/// `InvalidRequestException`, `LimitExceededException` and, for the update,
/// `ResourceNotFoundException`): the `iam:PassRole` authorization runs in the
/// front door, ahead of the operation, so its denial is the generic AWS
/// authorization failure rather than a modelled operation error.
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The HTTP status an AWS `AccessDeniedException` carries: the request was
/// authenticated and then refused by authorization.
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// The denial for an `ExecutionRoleArn` that names another account's IAM
/// principal: Amazon Data Lifecycle Manager can only pass a role of the account
/// the request is made in.
pub const CROSS_ACCOUNT_PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";

/// Why passing `execution_role_arn` is denied, or `None` when the caller may
/// pass it.
///
/// Amazon Data Lifecycle Manager assumes the role the request names, so every
/// operation that takes an `ExecutionRoleArn` first authorizes `iam:PassRole`
/// for it. That authorization is about the role the ARN DENOTES, so it can only
/// be asked of a value that denotes an IAM principal at all: an
/// `ExecutionRoleArn` that is not a well-formed IAM ARN is left to the member's
/// own `@pattern`, which refuses it as a malformed request instead.
///
/// Of the IAM ARNs, two cannot be passed: one in another account (Amazon Data
/// Lifecycle Manager has no cross-account trust), and one that does not denote
/// a role (a user, a group, an instance profile — `iam:PassRole` has no role to
/// grant). The account is checked first, which is the order AWS answers in for
/// an ARN that is both.
pub fn pass_role_denial(execution_role_arn: &str) -> Option<String> {
    let (account, resource) = iam_arn_parts(execution_role_arn)?;
    if account != ::engine::emulator_account_id() {
        return Some(CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string());
    }
    let names_a_role = resource
        .strip_prefix("role/")
        .is_some_and(|name| !name.is_empty());
    if names_a_role {
        return None;
    }
    Some(format!(
        "The role {execution_role_arn} cannot be passed: it does not identify an IAM role."
    ))
}

/// The account id and the resource portion of `arn`, when `arn` is a well-formed
/// IAM ARN — `arn:<partition>:iam::<account>:<resource>`.
///
/// IAM is a global service, so an IAM ARN names an empty region; the partition
/// is the `aws(-[a-z]{1,3}){0,2}` family the `ExecutionRoleArn` `@pattern` also
/// spells, and the account is digits. Anything else is not an ARN this service
/// can resolve a principal from.
fn iam_arn_parts(arn: &str) -> Option<(&str, &str)> {
    // The resource portion may itself contain `:`, so it takes the remainder.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if partition != "aws" && !partition.starts_with("aws-") {
        return None;
    }
    if fields.next()? != "iam" {
        return None;
    }
    if !fields.next()?.is_empty() {
        return None;
    }
    let account = fields.next()?;
    if account.is_empty() || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let resource = fields.next()?;
    Some((account, resource))
}

/// How many policies the store holds before `CreateLifecyclePolicy` refuses with
/// the modelled `LimitExceededException`.
///
/// This is the emulator's OWN bound on the store, not a claim about any
/// account's Amazon Data Lifecycle Manager quota (which is adjustable, and which
/// this emulator does not model). It is set far above any workflow an emulator
/// serves, and it is what stops the state growing without end: every record it
/// holds is released by `DeleteLifecyclePolicy`.
pub const POLICY_QUOTA: usize = 1_000;

/// True when `details` configures an EVENT-BASED policy rather than a custom
/// snapshot or AMI one.
///
/// `PolicyDetails$PolicyType` is the ONE member that says which kind of policy a
/// configuration describes — "The type of policy ... Specify
/// `EVENT_BASED_POLICY` to create an event-based policy that performs specific
/// actions when a defined event occurs" — and a configuration naming no type
/// describes the kind its documented default names ([`DEFAULT_POLICY_TYPE`],
/// "The default is `EBS_SNAPSHOT_MANAGEMENT`"), which is a snapshot policy. On
/// an update the type the policy already holds is carried onto a replacement
/// stating none first ([`carry_over_policy_type`]), so the member read here is
/// the type the configuration will be stored with either way.
///
/// The kind is NEVER inferred from which OTHER members the configuration happens
/// to carry. `EventSource` is documented "[Event-based policies only]", but that
/// says where the member may be stated, not what the configuration's type is: a
/// configuration carrying one and naming no type still describes a policy of the
/// defaulted type, which is exactly the inference that default replaces. Reading
/// it as the type instead would make one request mean two different things to
/// two rules — the one that refuses a change of type, which reads the type
/// member, and the one that asks which members the configuration must state.
pub fn configures_event_based_policy(details: &PolicyDetails) -> bool {
    details.policy_type.as_deref() == Some(EVENT_BASED_POLICY_TYPE)
}

/// What a DEFAULT policy of the kind `resource_type` names MANAGES, as a
/// `PolicyTypeValues`.
///
/// `DefaultPolicy` / `PolicyDetails$ResourceType` and
/// `PolicyDetails$PolicyType` state ONE fact about a default policy from the two
/// ends: "To create a default policy for EBS-backed AMIs ... specify `INSTANCE`"
/// is the same policy as "Specify `IMAGE_MANAGEMENT` to create a lifecycle
/// policy that manages the lifecycle of EBS-backed AMIs", and `VOLUME`'s
/// snapshots of volumes are `EBS_SNAPSHOT_MANAGEMENT`'s snapshots. So the kind
/// of default policy decides the type it manages, and this is the ONE place that
/// correspondence is written.
///
/// A default policy whose kind nothing has stated manages the type
/// `PolicyDetails$PolicyType`'s own documented default names ("The default is
/// `EBS_SNAPSHOT_MANAGEMENT`"), which is the type the snapshot kind manages.
pub fn default_policy_managed_type(resource_type: Option<&str>) -> &'static str {
    match resource_type {
        Some(INSTANCE_DEFAULT_POLICY_TYPE) => IMAGE_POLICY_TYPE,
        _ => DEFAULT_POLICY_TYPE,
    }
}

/// The correspondence above read the other way: the kind of DEFAULT policy that
/// manages `policy_type`, or `None` when no kind of default policy manages it.
pub fn default_policy_kind_managing(policy_type: &str) -> Option<&'static str> {
    [VOLUME_DEFAULT_POLICY_TYPE, INSTANCE_DEFAULT_POLICY_TYPE]
        .into_iter()
        .find(|kind| default_policy_managed_type(Some(kind)) == policy_type)
}

/// Which KIND of policy a write path is storing a configuration for — the input
/// to [`materialized_policy_details`], which materializes the members that kind
/// of policy carries.
///
/// The kind is the write path's to decide, because only it knows one: a create
/// reads it off the request and an update off the policy it addresses, whose
/// kind it cannot change ([`immutable_member_changed`]).
#[derive(Debug, Clone, Copy)]
pub enum PolicyKind<'resource_type> {
    /// A DEFAULT policy, beside the kind of default policy it is — the
    /// `DefaultPolicyTypeValues` the record holds, which is what decides the
    /// type such a policy manages ([`default_policy_managed_type`]).
    Default {
        resource_type: Option<&'resource_type str>,
    },
    /// A CUSTOM policy, whose own configuration states its type.
    Custom,
}

/// The kind of DEFAULT policy a request describes, or why the `PolicyType` its
/// configuration states cannot describe one at all.
///
/// A default policy's kind and the type it manages are two statements of one
/// fact ([`default_policy_managed_type`]), so a request may state either member
/// — `DefaultPolicy` or `PolicyDetails$ResourceType` as `stated_resource_type`,
/// `PolicyDetails$PolicyType` inside `details` — and:
///
/// * stating both is accepted only when they agree, since a pair that disagrees
///   asks for a policy that manages one thing and targets the resources of the
///   other, which no default policy does;
/// * stating the type alone names the kind that manages it, so the member
///   decides what the policy manages exactly as its documentation says;
/// * stating a type no kind of default policy manages (`EVENT_BASED_POLICY`, the
///   type of a policy that "performs specific actions when a defined event
///   occurs") describes no default policy, so it is refused rather than stored
///   as one;
/// * stating neither leaves the kind unstated, and the policy manages the type
///   `PolicyDetails$PolicyType`'s documented default names.
pub fn default_policy_kind(
    stated_resource_type: Option<&str>,
    details: Option<&PolicyDetails>,
) -> Result<Option<String>, InvalidRequestException> {
    let stated_policy_type = details.and_then(|details| details.policy_type.as_deref());
    match (stated_resource_type, stated_policy_type) {
        (Some(resource_type), Some(policy_type)) => {
            let managed = default_policy_managed_type(Some(resource_type));
            if policy_type != managed {
                return Err(default_policy_manages_another_type(
                    resource_type,
                    managed,
                    policy_type,
                ));
            }
            Ok(Some(resource_type.to_string()))
        }
        (Some(resource_type), None) => Ok(Some(resource_type.to_string())),
        (None, Some(policy_type)) => default_policy_kind_managing(policy_type)
            .map(|kind| Some(kind.to_string()))
            .ok_or_else(|| no_default_policy_manages(policy_type)),
        (None, None) => Ok(None),
    }
}

/// The `InvalidRequestException` for a request whose configuration states a
/// `PolicyType` other than the one the kind of default policy it asks for
/// manages. Built from the request: it names the member refused, the kind of
/// default policy asked for, the type that kind manages and the type stated.
pub fn default_policy_manages_another_type(
    resource_type: &str,
    managed: &str,
    stated: &str,
) -> InvalidRequestException {
    invalid_request(format!(
        "The parameter {POLICY_DETAILS_PREFIX}{POLICY_TYPE_MEMBER} must be {managed} for a \
         default policy that targets {resource_type}, but {stated} was requested."
    ))
}

/// The `InvalidRequestException` for a request that asks for a default policy
/// whose configuration states a `PolicyType` no kind of default policy manages.
pub fn no_default_policy_manages(stated: &str) -> InvalidRequestException {
    invalid_request(format!(
        "The parameter {POLICY_DETAILS_PREFIX}{POLICY_TYPE_MEMBER} must be {} for a default \
         policy, but {stated} was requested.",
        DEFAULT_POLICY_MANAGED_TYPES.join(" or ")
    ))
}

/// The `PolicyDetails` this service STORES for a policy whose configuration was
/// stated as a `PolicyDetails` structure: the submitted structure with every
/// member the service itself materializes stamped onto it.
///
/// Amazon Data Lifecycle Manager fills a custom policy's omitted configuration
/// members in when it accepts the policy, and a read then reports the stored
/// configuration — the submitted members unchanged plus the materialized ones.
/// So the stamping happens here, where a create or an update writes the record,
/// and never in a read projection.
///
/// WHICH members are materialized follows the KIND of policy the record is —
/// [`PolicyKind`], a DEFAULT policy against a custom one — and never what the
/// submitted structure's own `PolicyLanguage` happens to say. The kind is the
/// caller's to decide, because only the write path knows it: a create derives it
/// from the request, and an update from the policy it addresses, whose kind it
/// cannot change ([`immutable_member_changed`]). A DEFAULT policy's
/// configuration carries none of the custom members below; its `[Default
/// policies only]` defaults are the documented ones, which
/// [`LifecyclePolicyRecord::effective_policy_details`] reports from the record's
/// own fields.
///
/// What is stamped, and why:
///
/// * `PolicyLanguage` — the language the policy's kind names, which
///   `PolicyDetails$PolicyLanguage` documents: `SIMPLIFIED` "To create a default
///   policy" and `STANDARD` "To create a custom policy". Stamping it is what
///   leaves no stored configuration whose language contradicts the kind of policy
///   holding it.
/// * `PolicyType` — the type the policy MANAGES, by the same rule: for a DEFAULT
///   policy the type the kind of default policy it is manages
///   ([`default_policy_managed_type`]), and for a custom one the documented
///   default `EBS_SNAPSHOT_MANAGEMENT` when its configuration names no type
///   ("The default is `EBS_SNAPSHOT_MANAGEMENT`"), whatever the policy targets.
///   Stamping it is what leaves no stored configuration whose type contradicts
///   the type every read of the policy reports.
/// * `ResourceLocations` — `[CLOUD]`, the in-Region location. The member is
///   `@length(min: 1, max: 1)`, so a read of a custom snapshot or AMI policy
///   reports exactly one location.
/// * per schedule — `CopyTags` false, the two rule intervals' zero,
///   `CreateRule$Location` `CLOUD` (its documented default) for a SNAPSHOT policy
///   only, since that member is `[Custom snapshot policies only]`, and a
///   cross-Region copy rule's `CopyTags`. This applies to EVERY element of
///   `Schedules` (`@length(min: 1, max: 4)`) and of `CrossRegionCopyRules`, not
///   just the first.
///
/// What is NOT stamped: a member for which neither the model, the documentation
/// nor a recorded read states a value the service fills in — a count, an
/// interval unit, a nested deprecate/archive/fast-restore rule's members. They
/// keep exactly what the request stated, because inventing one would be
/// fabrication.
pub fn materialized_policy_details(
    mut details: PolicyDetails,
    kind: PolicyKind<'_>,
) -> PolicyDetails {
    if let PolicyKind::Default { resource_type } = kind {
        // A DEFAULT policy's configuration, which carries none of the custom
        // members stamped below — and none of the members a custom policy alone
        // is configured from either: a request stating one of those for a
        // default policy is refused before anything is stored
        // ([`custom_policy_only_violation`]), so nothing of the kind reaches
        // here to be stamped or dropped. Its type is the one the kind of default policy
        // it is manages, which is also the type its reads report, so the stored
        // configuration states that type however the request stated it: a
        // request may name the type or the kind, and one naming a type the kind
        // it asks for does not manage is refused before anything is stored
        // ([`default_policy_kind`]).
        details.policy_language = Some(SIMPLIFIED_POLICY_LANGUAGE.to_string());
        details.policy_type = Some(default_policy_managed_type(resource_type).to_string());
        return details;
    }
    details.policy_language = Some(STANDARD_POLICY_LANGUAGE.to_string());

    // `PolicyType` — the documented default for a configuration that names no
    // type: "The default is `EBS_SNAPSHOT_MANAGEMENT`." It is stamped here, so
    // the type is stored once and both reads report the same one, and it is
    // never derived from what the policy TARGETS: `ResourceTypes` documents
    // `INSTANCE` as "create multi-volume snapshots from the volumes for an
    // instance", which is snapshot management. That holds for a configuration
    // carrying an `EventSource` and no type too: by the same documentation such
    // a request describes a snapshot policy that omits the members one is built
    // from, so there is no accepted event-based policy whose type was left
    // unstated, and guessing the type from which members a configuration
    // happens to carry is the very inference this default replaces.
    //
    // It is a default for a policy whose type nothing has stated YET, so it is
    // reached on the create path alone: an update cannot change the type a
    // policy already holds, and carries that type onto a replacement
    // configuration stating none ([`carry_over_policy_type`]) before this runs.
    details
        .policy_type
        .get_or_insert_with(|| DEFAULT_POLICY_TYPE.to_string());

    // Which kind of configuration this is: the kind its `PolicyType` names, now
    // that a configuration which named none holds the type that member's
    // documented default names. Resource locations and schedules are `[Custom
    // snapshot and AMI policies only]`; an event-based policy configures an
    // `EventSource` and `Actions` instead, so it carries neither.
    if configures_event_based_policy(&details) {
        return details;
    }
    // `CreateRule$Location` is `[Custom snapshot policies only]`, so an AMI
    // policy's creation rule has no location to report. `PolicyDetails$PolicyType`
    // is what states the type, stamped just above.
    let creates_snapshots = details.policy_type.as_deref() != Some(IMAGE_POLICY_TYPE);

    details
        .resource_locations
        .get_or_insert_with(|| vec![CLOUD_LOCATION.to_string()]);
    for schedule in details.schedules.iter_mut().flatten() {
        schedule.copy_tags.get_or_insert(DEFAULT_SCHEDULE_COPY_TAGS);
        if let Some(create_rule) = schedule.create_rule.as_mut() {
            if creates_snapshots {
                create_rule
                    .location
                    .get_or_insert_with(|| CLOUD_LOCATION.to_string());
            }
            create_rule.interval.get_or_insert(UNSET_RULE_INTERVAL);
        }
        if let Some(retain_rule) = schedule.retain_rule.as_mut() {
            retain_rule.interval.get_or_insert(UNSET_RULE_INTERVAL);
        }
        for copy_rule in schedule.cross_region_copy_rules.iter_mut().flatten() {
            copy_rule.copy_tags.get_or_insert(DEFAULT_COPY_TAGS_ON_COPY);
        }
    }
    details
}

/// A DEFAULT policy's `[Default policies only]` configuration, in the ONE form
/// [`LifecyclePolicyRecord`] keeps it in.
///
/// Every one of these members is declared twice over — by
/// `CreateLifecyclePolicyRequest` / `UpdateLifecyclePolicyRequest` at the request
/// body level and by `PolicyDetails` inside the configuration structure — and a
/// request states the configuration in one place or the other. Two stored copies
/// of one member would need a precedence between them, and any fixed precedence
/// makes a write to the losing place unobservable, so the write paths resolve the
/// two into this ONE value and the read reports it.
///
/// The resource type is one of them: `PolicyDetails$ResourceType` and
/// `CreateLifecyclePolicyRequest$DefaultPolicy` carry the SAME documentation
/// ("Specify the type of default policy to create", `VOLUME` or `INSTANCE`), so
/// they are the structure form and the body form of one member and it is resolved
/// here like the rest. It is the one member an update may not CHANGE
/// (`UpdateLifecyclePolicyRequest$PolicyDetails`: "You cannot update the policy
/// type or the resource type"), so it is the one member
/// [`LifecyclePolicyRecord::replace_default_policy_configuration`] does not reset
/// to a default.
#[derive(Debug, Clone, Default)]
pub struct DefaultPolicyConfiguration {
    /// `DefaultPolicyTypeValues` (`INSTANCE` | `VOLUME`), as
    /// `PolicyDetails$ResourceType` or `DefaultPolicy` states it.
    pub resource_type: Option<String>,
    pub create_interval: Option<i32>,
    pub retain_interval: Option<i32>,
    pub copy_tags: Option<bool>,
    pub extend_deletion: Option<bool>,
    pub cross_region_copy_targets: Option<Vec<CrossRegionCopyTarget>>,
    pub exclusions: Option<Exclusions>,
}

impl DefaultPolicyConfiguration {
    /// The configuration `details` states, MOVED OUT of it.
    ///
    /// Taking rather than copying is what leaves one stored copy per member: the
    /// structure the record goes on to hold carries the configuration members
    /// that are not `[Default policies only]`, and this carries the ones that
    /// are.
    pub fn taken_from_policy_details(details: &mut PolicyDetails) -> Self {
        Self {
            resource_type: details.resource_type.take(),
            create_interval: details.create_interval.take(),
            retain_interval: details.retain_interval.take(),
            copy_tags: details.copy_tags.take(),
            extend_deletion: details.extend_deletion.take(),
            cross_region_copy_targets: details.cross_region_copy_targets.take(),
            exclusions: details.exclusions.take(),
        }
    }

    /// This configuration where it states a member, `fallback` elsewhere.
    ///
    /// One request may state some of its configuration in the request body and
    /// the rest inside the structure — the two are alternatives per member, and
    /// a member stated in both places is refused
    /// ([`configuration_stated_twice`]) — so this MERGES the two halves, taking
    /// each member from whichever of them stated it. At most one side states any
    /// one member, so the precedence itself never decides anything a caller can
    /// observe.
    pub fn or(self, fallback: Self) -> Self {
        Self {
            resource_type: self.resource_type.or(fallback.resource_type),
            create_interval: self.create_interval.or(fallback.create_interval),
            retain_interval: self.retain_interval.or(fallback.retain_interval),
            copy_tags: self.copy_tags.or(fallback.copy_tags),
            extend_deletion: self.extend_deletion.or(fallback.extend_deletion),
            cross_region_copy_targets: self
                .cross_region_copy_targets
                .or(fallback.cross_region_copy_targets),
            exclusions: self.exclusions.or(fallback.exclusions),
        }
    }
}

/// One Amazon Data Lifecycle Manager lifecycle policy, as this emulator persists
/// it. Every member a read operation reports comes from here, never from the
/// request being served.
#[derive(Debug, Clone)]
pub struct LifecyclePolicyRecord {
    /// `^policy-[a-f0-9]+$`, minted by
    /// [`ComAmazonawsDlmDlm20180112State::mint_policy_id`].
    pub policy_id: String,
    /// `arn:aws:dlm:<region>:<account>:policy/<policy id>`, derived at create.
    pub policy_arn: String,
    pub description: String,
    /// One of `GettablePolicyStateValues` (`ENABLED` | `DISABLED` | `ERROR`).
    pub state: String,
    /// "The description of the status", when some failure path has recorded one.
    /// Nothing in this emulator moves a policy into `ERROR`, so no policy
    /// carries one and [`LifecyclePolicyRecord::wire_status_message`] reports the
    /// policy's activation state instead.
    pub status_message: Option<String>,
    pub execution_role_arn: String,
    pub date_created: SystemTime,
    pub date_modified: SystemTime,
    pub policy_details: Option<PolicyDetails>,
    /// True when this is a default policy rather than a custom one.
    pub default_policy: bool,
    /// `DefaultPolicyTypeValues` (`INSTANCE` | `VOLUME`) for a default policy:
    /// the ONE stored copy of the resource type, whichever of the two
    /// alternative places the request stated it in (`DefaultPolicy` at the
    /// request-body level, or `PolicyDetails$ResourceType` inside the
    /// configuration structure), so every read and the `resourceTypes` filter
    /// report the same type.
    pub default_policy_type: Option<String>,
    /// The `[Default policies only]` configuration, in the ONE place this record
    /// keeps it — see [`DefaultPolicyConfiguration`].
    pub create_interval: Option<i32>,
    pub retain_interval: Option<i32>,
    pub copy_tags: Option<bool>,
    pub extend_deletion: Option<bool>,
    pub cross_region_copy_targets: Vec<CrossRegionCopyTarget>,
    pub exclusions: Option<Exclusions>,
    pub tags: BTreeMap<String, String>,
}

impl LifecyclePolicyRecord {
    /// The `PolicyDetails` this policy reports: "The configuration of the
    /// lifecycle policy".
    ///
    /// A CUSTOM policy's configuration is exactly the structure the request
    /// carried. A DEFAULT policy's may have been stated in the request body
    /// instead — `CreateLifecyclePolicy` documents the two as alternatives
    /// ("either in the request body, or in the PolicyDetails request structure,
    /// but not both") — so whichever place a request stated it in, the write
    /// path moved it onto the record's own fields
    /// ([`DefaultPolicyConfiguration`]). This reports THOSE, each with the
    /// default its member documents when nothing stated a value, so there is one
    /// stored value per configuration member and the last write of it is what
    /// every later read reports.
    ///
    /// A CUSTOM policy therefore reports its stored structure and nothing else,
    /// and that loses nothing: every one of the `[Default policies only]` fields
    /// below is left unset on a custom record, because a create or an update
    /// that states one for a policy that is not a default policy is refused
    /// ([`default_policy_only_parameters_refused`]) instead of being stored
    /// where no read could reach it.
    pub fn effective_policy_details(&self) -> Option<PolicyDetails> {
        if !self.default_policy {
            return self.policy_details.clone();
        }
        let mut details = self.policy_details.clone().unwrap_or_default();
        // The language and the type a default policy's kind names. A request may
        // have stated the configuration in the request body alone, leaving the
        // record no structure to have stamped them onto, so they are reported
        // from the record here as well; for a record that holds a structure the
        // write path already stamped the same two values into it
        // ([`materialized_policy_details`]), so this reports what is stored
        // rather than deciding it.
        details.policy_language = Some(self.policy_language().to_string());
        details.policy_type = Some(self.policy_type());
        details.resource_type = self.default_policy_type.clone();
        details.create_interval = Some(self.create_interval.unwrap_or(DEFAULT_CREATE_INTERVAL));
        details.retain_interval = Some(self.retain_interval.unwrap_or(DEFAULT_RETAIN_INTERVAL));
        details.copy_tags = Some(self.copy_tags.unwrap_or(DEFAULT_COPY_TAGS));
        details.extend_deletion = Some(self.extend_deletion.unwrap_or(DEFAULT_EXTEND_DELETION));
        // A destination list is reported only when the policy copies somewhere:
        // `CrossRegionCopyTargetList` is `@length(min: 0)`, so a policy that
        // states the empty list states "no cross-Region copies", which the member
        // documents as the state of omitting it.
        details.cross_region_copy_targets = (!self.cross_region_copy_targets.is_empty())
            .then(|| self.cross_region_copy_targets.clone());
        details.exclusions = self.exclusions.clone();
        Some(details)
    }

    /// Replace this policy's `[Default policies only]` configuration with
    /// `configuration`.
    ///
    /// WHOLESALE: every member `configuration` states takes effect and every
    /// member it omits goes back to the default its own documentation publishes,
    /// because the `PolicyDetails` structure a request states IS "the
    /// configuration of the lifecycle policy" rather than a patch of it.
    ///
    /// The resource type is the exception the model states:
    /// `UpdateLifecyclePolicyRequest$PolicyDetails` is "the configuration of the
    /// lifecycle policy. You cannot update the policy type or the resource type",
    /// so the type the policy was created with stays in place whichever way the
    /// replacement treats it: a configuration that omits it asks for no change,
    /// and one that states it has been checked against the stored type already
    /// (the update refuses a different one), so it restates the value held. That
    /// is why the member is taken out of the replacement and dropped here rather
    /// than written: a default policy's resource type is written ONCE, by the
    /// create that required it.
    pub fn replace_default_policy_configuration(
        &mut self,
        configuration: DefaultPolicyConfiguration,
    ) {
        self.create_interval = configuration.create_interval;
        self.retain_interval = configuration.retain_interval;
        self.copy_tags = configuration.copy_tags;
        self.extend_deletion = configuration.extend_deletion;
        self.cross_region_copy_targets = configuration.cross_region_copy_targets.unwrap_or_default();
        self.exclusions = configuration.exclusions;
    }

    /// Stamp the instant this policy was last written, which
    /// `LifecyclePolicy$DateModified` reports as "the local date and time when
    /// the lifecycle policy was last modified".
    ///
    /// EVERY accepted write of this record goes through here, whichever member
    /// it changes — the configuration, the activation state, the tags — because
    /// a read projects the whole record ([`LifecyclePolicyRecord::to_policy`])
    /// and the instant it reports is therefore the instant of the last change
    /// to ANY of it, not of the last change to one chosen member. A refused
    /// request never reaches it: every refusal is decided before the first
    /// write, so a policy nothing changed keeps the instant it had.
    pub fn mark_modified(&mut self, now: SystemTime) {
        self.date_modified = now;
    }

    /// The `PolicyLanguage` this policy's KIND names:
    /// `PolicyDetails$PolicyLanguage` documents `SIMPLIFIED` as "To create a
    /// default policy" and `STANDARD` as "To create a custom policy", so the
    /// language is the wire name of the one flag the record keeps for it rather
    /// than a second, independently stored value that could contradict it.
    ///
    /// The kind is decided when the policy is created and an update cannot change
    /// it ([`immutable_member_changed`]), so this is also the only language a
    /// request addressing this policy may state.
    pub fn policy_language(&self) -> &'static str {
        if self.default_policy {
            SIMPLIFIED_POLICY_LANGUAGE
        } else {
            STANDARD_POLICY_LANGUAGE
        }
    }

    /// This policy's `PolicyTypeValues`.
    ///
    /// `PolicyDetails$PolicyType` is the member that states it, and a policy
    /// whose configuration states none carries the documented default
    /// ([`DEFAULT_POLICY_TYPE`]) — never a type derived from the resource the
    /// policy TARGETS, which says nothing about what it manages: a custom
    /// policy targeting `INSTANCE` is documented as creating "multi-volume
    /// snapshots from the volumes for an instance".
    ///
    /// A DEFAULT policy is the one case where the target IS the statement of
    /// what the policy manages, because such a policy configures no type of its
    /// own: `DefaultPolicy` `INSTANCE` creates EBS-backed AMIs
    /// ([`IMAGE_POLICY_TYPE`]) and `VOLUME` creates snapshots
    /// ([`default_policy_managed_type`], the one place that correspondence is
    /// written). That is judged FIRST, so the type a default policy reports
    /// follows the kind of default policy it is, which is also the type the
    /// write path stamped onto its stored configuration.
    pub fn policy_type(&self) -> String {
        if self.default_policy {
            return default_policy_managed_type(self.default_policy_type.as_deref()).to_string();
        }
        self.policy_details
            .as_ref()
            .and_then(|details| details.policy_type.clone())
            .unwrap_or_else(|| DEFAULT_POLICY_TYPE.to_string())
    }

    /// Every resource type this policy targets, for the `resourceTypes` filter.
    ///
    /// A DEFAULT policy targets the one type [`Self::default_policy_type`] holds
    /// — the single stored copy of it, which is also the one every read reports —
    /// and a CUSTOM policy the types its configuration's `ResourceTypes` names.
    /// The two are ALTERNATIVES, by the kind of policy this record is: which of
    /// the two members states the target is itself decided by that kind
    /// (`ResourceType` is `[Default policies only]` and `ResourceTypes` is
    /// documented for a custom policy alone, and a request stating one for a
    /// policy of the other kind is refused —
    /// [`DefaultPolicyOnlyRequest::violation`] and
    /// [`custom_policy_only_violation`]), so reading both would report one
    /// policy's single target twice over and name more types than
    /// `ResourceTypeValuesList`'s `@length(min: 1, max: 1)` admits.
    pub fn target_resource_types(&self) -> Vec<String> {
        if self.default_policy {
            return self.default_policy_type.iter().cloned().collect();
        }
        self.policy_details
            .as_ref()
            .and_then(|details| details.resource_types.clone())
            .unwrap_or_default()
    }

    /// This policy's target tags as the `key=value` strings the `targetTags`
    /// filter is expressed in ("Tags are strings in the format key=value").
    pub fn target_tag_filters(&self) -> Vec<String> {
        self.policy_details
            .as_ref()
            .and_then(|details| details.target_tags.as_ref())
            .map(|tags| tags.iter().map(tag_filter).collect())
            .unwrap_or_default()
    }

    /// The tags this policy's schedules add to the objects they create, as
    /// `key=value` strings, for the `tagsToAdd` filter.
    pub fn tags_to_add_filters(&self) -> Vec<String> {
        let Some(schedules) = self
            .policy_details
            .as_ref()
            .and_then(|details| details.schedules.as_ref())
        else {
            return Vec::new();
        };
        schedules
            .iter()
            .filter_map(|schedule| schedule.tags_to_add.as_ref())
            .flat_map(|tags| tags.iter().map(tag_filter))
            .collect()
    }

    /// The policy's tags as the wire map.
    ///
    /// A policy with no tags reports the EMPTY map, not an absent member: the
    /// read path reports the tag set a policy holds, and a client must be able to
    /// tell "no tags" from "not reported" without a second call. `TagMap`'s
    /// `@length(min: 1, max: 200)` bounds what a request may SEND — the
    /// validating walk runs on operation inputs only — and says nothing about
    /// what a read reports.
    pub fn wire_tags(&self) -> Option<HashMap<String, String>> {
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    /// "The description of the status" this policy reports.
    ///
    /// It describes the policy's CURRENT status, so a policy that carries no
    /// failure description reports its activation state — the same
    /// `GettablePolicyStateValues` member `State` reports. Nothing in this
    /// emulator runs a policy, so [`LifecyclePolicyRecord::status_message`] is
    /// set only if some future failure path records one, and it wins when it is.
    pub fn wire_status_message(&self) -> String {
        self.status_message
            .clone()
            .unwrap_or_else(|| self.state.clone())
    }

    /// The `LifecyclePolicy` projection `GetLifecyclePolicy` returns.
    pub fn to_policy(&self) -> LifecyclePolicy {
        LifecyclePolicy {
            policy_id: Some(self.policy_id.clone()),
            description: Some(self.description.clone()),
            state: Some(self.state.clone()),
            status_message: Some(self.wire_status_message()),
            execution_role_arn: Some(self.execution_role_arn.clone()),
            date_created: Some(::aws_smithy_types::DateTime::from(self.date_created)),
            date_modified: Some(::aws_smithy_types::DateTime::from(self.date_modified)),
            policy_details: self.effective_policy_details(),
            tags: self.wire_tags(),
            policy_arn: Some(self.policy_arn.clone()),
            default_policy: Some(self.default_policy),
        }
    }

    /// The `LifecyclePolicySummary` projection `GetLifecyclePolicies` returns.
    ///
    /// `DefaultPolicy` is modelled `boolean` on this shape too (the prose
    /// describing it as a type is stale), so it carries the same flag the full
    /// shape does.
    pub fn to_summary(&self) -> LifecyclePolicySummary {
        LifecyclePolicySummary {
            policy_id: Some(self.policy_id.clone()),
            description: Some(self.description.clone()),
            state: Some(self.state.clone()),
            tags: self.wire_tags(),
            policy_type: Some(self.policy_type()),
            default_policy: Some(self.default_policy),
        }
    }
}

/// A `Tag` as the `key=value` string Amazon Data Lifecycle Manager's tag filters
/// are expressed in.
fn tag_filter(tag: &Tag) -> String {
    format!("{}={}", tag.key, tag.value)
}

/// The lifecycle policies of ONE region, keyed by policy id. A `BTreeMap` gives
/// `GetLifecyclePolicies` a stable total order, and ids sort in the order they
/// were minted.
pub type RegionalPolicies = BTreeMap<String, LifecyclePolicyRecord>;

/// Per-service state for com-amazonaws-dlm-dlm-20180112. A handler reaches it through [`service_state`].
///
/// Policies are keyed by REGION and then by id, because a lifecycle policy
/// belongs to the region its request was signed for: Amazon Data Lifecycle
/// Manager is a regional control plane (per-region endpoints, and a `PolicyArn`
/// that names the region the policy lives in), so a policy created in one region
/// is the only scope in which its id resolves. The account is not part of the key
/// because this emulator serves exactly one account — the canonical account
/// [`::engine::emulator_arn`] mints every ARN for — so every request's policies
/// already belong to it.
#[derive(Debug, Default)]
pub struct ComAmazonawsDlmDlm20180112State {
    /// Every lifecycle policy, keyed by the region it was created in and then by
    /// its id — the single source of truth.
    pub regions: BTreeMap<String, RegionalPolicies>,
    /// The counter policy ids are minted from. It spans every region, so no two
    /// policies this store holds ever share an id — the per-region uniqueness a
    /// policy id has in Amazon Data Lifecycle Manager, and no weaker.
    pub next_policy_suffix: u64,
}

impl ComAmazonawsDlmDlm20180112State {
    /// Mint an unused policy id matching `PolicyId`'s `^policy-[a-f0-9]+$`.
    ///
    /// Sixteen lower-case hex digits keep every id the same width, so the
    /// `BTreeMap`'s key order is the order the ids were minted in; the loop skips
    /// an id already taken, which is what makes the wrap safe.
    pub fn mint_policy_id(&mut self) -> String {
        loop {
            self.next_policy_suffix = self.next_policy_suffix.wrapping_add(1);
            let id = format!("policy-{:016x}", self.next_policy_suffix);
            if !self.regions.values().any(|region| region.contains_key(&id)) {
                return id;
            }
        }
    }

    /// The policies of one region — the only scope a policy id resolves in.
    pub fn policies_in(&self, region: &str) -> Option<&RegionalPolicies> {
        self.regions.get(region)
    }

    /// The policy that id names in that region, whatever its state.
    pub fn policy(&self, region: &str, policy_id: &str) -> Option<&LifecyclePolicyRecord> {
        self.policies_in(region)
            .and_then(|policies| policies.get(policy_id))
    }

    /// The same, mutably, for the operations that change a policy.
    pub fn policy_mut(
        &mut self,
        region: &str,
        policy_id: &str,
    ) -> Option<&mut LifecyclePolicyRecord> {
        self.regions
            .get_mut(region)
            .and_then(|policies| policies.get_mut(policy_id))
    }

    /// Store a newly minted policy in the region it was created in.
    pub fn insert_policy(&mut self, region: &str, record: LifecyclePolicyRecord) {
        self.regions
            .entry(region.to_string())
            .or_default()
            .insert(record.policy_id.clone(), record);
    }

    /// Remove the policy that id names in that region, answering it when there
    /// was one. A region left holding no policies keeps no bucket.
    pub fn remove_policy(
        &mut self,
        region: &str,
        policy_id: &str,
    ) -> Option<LifecyclePolicyRecord> {
        let policies = self.regions.get_mut(region)?;
        let removed = policies.remove(policy_id);
        if policies.is_empty() {
            self.regions.remove(region);
        }
        removed
    }

    /// How many policies this store holds across every region — what the store's
    /// own bound ([`POLICY_QUOTA`]) is measured against.
    pub fn policy_count(&self) -> usize {
        self.regions.values().map(RegionalPolicies::len).sum()
    }

    /// The policy in that region whose stored `PolicyArn` is `arn`, or the
    /// refusal every ARN-addressed operation answers when none is.
    ///
    /// Resolving an ARN is one rule, so it lives here once: an ARN names the
    /// policy whose STORED `PolicyArn` it equals, among the policies of the
    /// region the request was signed for — no index is kept for it, because the
    /// ARN is derived from the id and the store is small — and an ARN that names
    /// none is a [`policy_not_found_by_arn`]. A stored ARN names the region its
    /// policy lives in, so an ARN naming another region resolves to no policy
    /// here.
    pub fn policy_by_arn(
        &self,
        region: &str,
        arn: &str,
    ) -> Result<&LifecyclePolicyRecord, ResourceNotFoundException> {
        self.policies_in(region)
            .and_then(|policies| policies.values().find(|policy| policy.policy_arn == arn))
            .ok_or_else(|| policy_not_found_by_arn(arn))
    }

    /// The same rule, mutably, for the ARN-addressed operations that write.
    pub fn policy_mut_by_arn(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Result<&mut LifecyclePolicyRecord, ResourceNotFoundException> {
        self.regions
            .get_mut(region)
            .and_then(|policies| policies.values_mut().find(|policy| policy.policy_arn == arn))
            .ok_or_else(|| policy_not_found_by_arn(arn))
    }
}

/// The region this request was signed for: the region whose policies it reaches,
/// and what a policy ARN minted for it names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the policy `policy_id` names, in `region`.
pub fn policy_arn(policy_id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("policy/{policy_id}"))
}

/// The `ResourceNotFoundException` payload for policy ids that name no policy.
///
/// Every member the shape declares is reported: the `Code` for the exception
/// class, the `ResourceType` token of the resource that was not found, the ids
/// that named none, and a message naming the resource asked for.
pub fn policies_not_found(ids: &[String]) -> ResourceNotFoundException {
    let message = match ids {
        [one] => format!("The resource {{{one}}} was not found."),
        many => format!("The resources {{{}}} were not found.", many.join(", ")),
    };
    ResourceNotFoundException {
        message: Some(message),
        code: Some(RESOURCE_NOT_FOUND_CODE.to_string()),
        resource_type: Some(POLICY_RESOURCE_TYPE.to_string()),
        resource_ids: Some(ids.to_vec()),
    }
}

/// The `ResourceNotFoundException` payload for an ARN that names no policy.
pub fn policy_not_found_by_arn(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("The resource {{{arn}}} was not found.")),
        code: Some(RESOURCE_NOT_FOUND_CODE.to_string()),
        resource_type: Some(POLICY_RESOURCE_TYPE.to_string()),
        // `ResourceIds` is "a list of resource IDs that were not found", and the
        // request named an ARN rather than an id that could be echoed here.
        resource_ids: None,
    }
}

/// The `InternalServerException` payload for a request that reached a handler
/// holding no store to serve it from — the ONE 500 this service answers, so
/// every handler reports it through this constructor.
///
/// Every member the shape declares is reported: the `Code` for the exception
/// class, and a message saying what was unavailable.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some(STORE_UNAVAILABLE_MESSAGE.to_string()),
        code: Some(INTERNAL_ERROR_CODE.to_string()),
    }
}

/// How this service states the 500 [`store_unavailable`] answers.
const STORE_UNAVAILABLE_MESSAGE: &str = "the dlm lifecycle-policy store is unavailable";

/// The `LimitExceededException` payload for a create the store's own bound
/// ([`POLICY_QUOTA`]) refuses.
///
/// Every member the shape declares is reported: the `Code` for the exception
/// class, the `ResourceType` token of the resource "for which a limit was
/// exceeded", and a message naming the limit reached.
pub fn policy_quota_exceeded() -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "this account already holds the maximum of {POLICY_QUOTA} lifecycle policies"
        )),
        code: Some(LIMIT_EXCEEDED_CODE.to_string()),
        resource_type: Some(POLICY_RESOURCE_TYPE.to_string()),
    }
}

/// The `InvalidRequestException` payload for a request this service refuses as
/// invalid, carrying the `Code` of the exception class and `message`.
///
/// `RequiredParameters` ("the request omitted one or more required parameters")
/// and `MutuallyExclusiveParameters` ("the request included parameters that
/// cannot be provided together") are left unset: each names a class of refusal,
/// so only the refusal of that class fills it — see
/// [`parameters_cannot_be_combined`] and [`required_parameters_omitted`].
pub fn invalid_request(message: String) -> InvalidRequestException {
    InvalidRequestException {
        message: Some(message),
        code: Some(INVALID_PARAMETER_CODE.to_string()),
        required_parameters: None,
        mutually_exclusive_parameters: None,
    }
}

/// The `InvalidRequestException` for a request that stated `parameters` which
/// cannot be provided together.
///
/// `MutuallyExclusiveParameters` is "the request included parameters that cannot
/// be provided together", so it lists the REQUEST's own parameters — the names a
/// caller passed to the operation. A rule about which FORM a structure nested
/// inside a request parameter was stated in is not a pair of request parameters:
/// it is reported by [`invalid_request`], through the message and code alone.
pub fn parameters_cannot_be_combined(
    message: String,
    parameters: Vec<String>,
) -> InvalidRequestException {
    InvalidRequestException {
        mutually_exclusive_parameters: Some(parameters),
        ..invalid_request(message)
    }
}

/// The `InvalidRequestException` for a request that omitted `parameters` it is
/// required to state.
pub fn required_parameters_omitted(
    message: String,
    parameters: Vec<String>,
) -> InvalidRequestException {
    InvalidRequestException {
        required_parameters: Some(parameters),
        ..invalid_request(message)
    }
}

/// How Amazon Data Lifecycle Manager says that a request left out parameters it
/// is required to state: the fixed text below, then the omitted parameters as a
/// brace-wrapped, comma-separated list.
///
/// This is the ONE place that wording lives, so every layer that refuses a
/// request for an omitted parameter — the modelled-constraint walk ahead of a
/// handler and the documented cross-member rules inside one — says it the same
/// way, and a client that classifies a validation failure by that wording (or
/// parses the name list out of it, as `RequiredParameters` lists the same names)
/// recognises it whichever layer refused the request.
pub fn required_parameters_missing_message(parameters: &[String]) -> String {
    format!(
        "The following required parameter(s) are missing: {{{}}}",
        parameters.join(", ")
    )
}

/// The whole `InvalidRequestException` for a request that omitted `parameters`:
/// the wording above, and the same names in `RequiredParameters` ("the request
/// omitted one or more required parameters").
pub fn required_parameters_missing(parameters: Vec<String>) -> InvalidRequestException {
    required_parameters_omitted(required_parameters_missing_message(&parameters), parameters)
}

/// How Amazon Data Lifecycle Manager says that a request asked to change
/// `parameters` that are fixed when the policy is created: the fixed text below,
/// then the parameters as a comma-separated list, and nothing of the values the
/// request stated or the policy holds — so the refusal reads the same whichever
/// policy, account or region it is answered for.
///
/// Carried by the message and the code alone: an immutable parameter was neither
/// omitted (`RequiredParameters`) nor stated beside a parameter it cannot be
/// combined with (`MutuallyExclusiveParameters`), so neither structured list
/// names it.
pub fn parameters_cannot_be_updated(parameters: &[&str]) -> InvalidRequestException {
    invalid_request(format!(
        "The following parameter(s) cannot be updated: {}",
        parameters.join(", ")
    ))
}

/// The prefix [`DefaultPolicyOnlyParameters::stated_names`] qualifies a member
/// stated inside the `PolicyDetails` configuration structure with, so a refusal
/// tells a caller WHERE it stated the parameter. The unqualified name of the
/// structure itself is [`POLICY_DETAILS_MEMBER`].
pub const POLICY_DETAILS_PREFIX: &str = "PolicyDetails.";

/// Which `[Default policies only]` configuration members a request states,
/// wherever it stated them.
///
/// `CreateLifecyclePolicyRequest`, `UpdateLifecyclePolicyRequest` and
/// `PolicyDetails` all declare the same configuration members, and the model
/// documents every one of them `[Default policies only]`. `ResourceType` is
/// declared by `PolicyDetails` alone — the request body states the same thing as
/// `CreateLifecyclePolicyRequest$DefaultPolicy`, which is the member that MAKES
/// a request a default-policy request — so a request-body set leaves it unset.
///
/// This is the ONE list of their wire names, so the refusal of a default-only
/// parameter and the "either in the request body, or in the PolicyDetails
/// request structure, but not both" rule name the same members.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultPolicyOnlyParameters {
    pub resource_type: bool,
    pub create_interval: bool,
    pub retain_interval: bool,
    pub copy_tags: bool,
    pub cross_region_copy_targets: bool,
    pub extend_deletion: bool,
    pub exclusions: bool,
}

impl DefaultPolicyOnlyParameters {
    /// Which of them `details` states.
    pub fn in_policy_details(details: Option<&PolicyDetails>) -> Self {
        let Some(details) = details else {
            return Self::default();
        };
        Self {
            resource_type: details.resource_type.is_some(),
            create_interval: details.create_interval.is_some(),
            retain_interval: details.retain_interval.is_some(),
            copy_tags: details.copy_tags.is_some(),
            cross_region_copy_targets: details.cross_region_copy_targets.is_some(),
            extend_deletion: details.extend_deletion.is_some(),
            exclusions: details.exclusions.is_some(),
        }
    }

    /// The members BOTH sets state, member by member — the parameters one
    /// request stated twice when the two sets are its two places.
    ///
    /// The "but not both" rule is about ONE parameter stated in two places, so
    /// it is decided per member: a parameter stated in one place alone is
    /// nowhere stated twice, whatever OTHER parameters the other place states.
    pub fn stated_in_both(&self, other: &Self) -> Self {
        Self {
            resource_type: self.resource_type && other.resource_type,
            create_interval: self.create_interval && other.create_interval,
            retain_interval: self.retain_interval && other.retain_interval,
            copy_tags: self.copy_tags && other.copy_tags,
            cross_region_copy_targets: self.cross_region_copy_targets
                && other.cross_region_copy_targets,
            extend_deletion: self.extend_deletion && other.extend_deletion,
            exclusions: self.exclusions && other.exclusions,
        }
    }

    /// The wire names of the members stated, in the order the model declares
    /// them, each qualified by `prefix` — [`POLICY_DETAILS_PREFIX`] for a member
    /// stated inside the configuration structure, `""` for a request-body one.
    pub fn stated_names(&self, prefix: &str) -> Vec<String> {
        [
            (RESOURCE_TYPE_MEMBER, self.resource_type),
            ("CreateInterval", self.create_interval),
            ("RetainInterval", self.retain_interval),
            ("CopyTags", self.copy_tags),
            ("CrossRegionCopyTargets", self.cross_region_copy_targets),
            ("ExtendDeletion", self.extend_deletion),
            ("Exclusions", self.exclusions),
        ]
        .into_iter()
        .filter(|(_, stated)| *stated)
        .map(|(name, _)| format!("{prefix}{name}"))
        .collect()
    }
}

/// A request that may state the `[Default policies only]` parameters in its
/// request BODY, as against inside its `PolicyDetails` structure.
///
/// `CreateLifecyclePolicyRequest` and `UpdateLifecyclePolicyRequest` declare the
/// same body members as the alternative to that structure, so WHICH members they
/// are is one rule for both: this is the ONE place the request-body half of
/// [`DefaultPolicyOnlyParameters`] is read, beside
/// [`DefaultPolicyOnlyParameters::in_policy_details`] for the structure half.
/// Every refusal built from either half — a default-only parameter stated for a
/// policy that is not a default policy, and the configuration stated both in the
/// body and in the structure — therefore names the same set on every operation
/// that takes it.
pub trait DefaultPolicyOnlyBody {
    /// Which of the `[Default policies only]` parameters this request states in
    /// the request body.
    fn default_policy_only_body_parameters(&self) -> DefaultPolicyOnlyParameters;
}

/// Writes the [`DefaultPolicyOnlyBody`] mapping ONCE, as one impl per request
/// shape that declares those members in its body under the model's own names.
///
/// The mapping is per-field on two distinct structures, so a generic function
/// cannot express it; this keeps the member list in a single place all the same.
///
/// `PolicyDetails$ResourceType` is not among them: it has no same-named body
/// member to read. A create states the resource type in the body as
/// `CreateLifecyclePolicyRequest$DefaultPolicy` — also the member that MAKES the
/// request a default-policy request, so it is read where that name belongs — and
/// an update may not change it at all ([`immutable_member_changed`]), so
/// a body set leaves it unset.
macro_rules! default_policy_only_body {
    ($($request:ty),+ $(,)?) => {
        $(
            impl DefaultPolicyOnlyBody for $request {
                fn default_policy_only_body_parameters(&self) -> DefaultPolicyOnlyParameters {
                    DefaultPolicyOnlyParameters {
                        resource_type: false,
                        create_interval: self.create_interval.is_some(),
                        retain_interval: self.retain_interval.is_some(),
                        copy_tags: self.copy_tags.is_some(),
                        cross_region_copy_targets: self.cross_region_copy_targets.is_some(),
                        extend_deletion: self.extend_deletion.is_some(),
                        exclusions: self.exclusions.is_some(),
                    }
                }
            }
        )+
    };
}

default_policy_only_body!(
    crate::ops::types::CreateLifecyclePolicyInput,
    crate::ops::types::UpdateLifecyclePolicyInput,
);

/// The `[Default policies only]` parameters ONE request states, beside the KIND
/// of policy it describes — the input to [`DefaultPolicyOnlyRequest::violation`].
///
/// Both halves of [`DefaultPolicyOnlyParameters`] are read HERE, beside the
/// member list itself, so the two refusals composed from them consult the same
/// halves, in the same order, under the same prefix, on every operation that
/// takes those parameters.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultPolicyOnlyRequest<'details> {
    /// Whether the policy this request creates or updates IS a default policy:
    /// the kind of policy those parameters belong to.
    pub default_policy: bool,
    /// Which of them the request BODY states, read through
    /// [`DefaultPolicyOnlyBody`].
    pub body: DefaultPolicyOnlyParameters,
    /// Whether the request body states the policy's RESOURCE TYPE.
    ///
    /// [`DEFAULT_POLICY_MEMBER`] is the only body member that states it, so only
    /// a create can carry one; an update may not change the resource type
    /// ([`resource_type_cannot_be_updated`]) and its body declares no member for
    /// it, so an update states none.
    pub body_resource_type: bool,
    /// The configuration structure this request carries, whose
    /// `[Default policies only]` members are the alternative to the body ones.
    pub details: Option<&'details PolicyDetails>,
}

impl DefaultPolicyOnlyRequest<'_> {
    /// Why Amazon Data Lifecycle Manager refuses this request's `[Default
    /// policies only]` parameters, or `None` when it states a set the
    /// documentation admits.
    ///
    /// Two documented rules, decided by the kind of policy the request
    /// describes, so at most one of them can refuse a request:
    ///
    /// * a policy that is NOT a default policy may carry none of those members,
    ///   so a request stating one anywhere — in the request body or inside the
    ///   configuration structure — is refused naming every place it stated one
    ///   ([`default_policy_only_parameters_refused`]);
    /// * a DEFAULT policy's request states each of them "either in the request
    ///   body, or in the PolicyDetails request structure, but not both", so a
    ///   request is refused for the parameters it states in BOTH places, named
    ///   by the body member it used ([`configuration_stated_twice`]) — and a
    ///   parameter stated in one place alone is accepted however many other
    ///   parameters the other place states.
    pub fn violation(&self) -> Option<InvalidRequestException> {
        if !self.default_policy {
            let refused = self.stated_names();
            return (!refused.is_empty())
                .then(|| default_policy_only_parameters_refused(&refused));
        }
        let duplicated = self.duplicated_body_names();
        (!duplicated.is_empty()).then(|| configuration_stated_twice(&duplicated))
    }

    /// Everywhere this request states a `[Default policies only]` parameter: the
    /// body half first, then the structure half, each structure-stated member
    /// qualified by [`POLICY_DETAILS_PREFIX`] so a refusal tells the caller
    /// WHERE it stated the parameter.
    fn stated_names(&self) -> Vec<String> {
        let mut stated = self.body.stated_names("");
        stated.extend(self.in_details().stated_names(POLICY_DETAILS_PREFIX));
        stated
    }

    /// The body parameters this request states a SECOND time inside its
    /// configuration structure, named by the BODY member the caller used — the
    /// alternative to the structure.
    ///
    /// Each parameter is compared across the two places on its own, so the list
    /// holds exactly the parameters the request really stated twice: a request
    /// carrying no structure, or one whose structure states none of the body's
    /// parameters, duplicates nothing.
    ///
    /// The resource type is one of those parameters, stated in the body as
    /// [`DEFAULT_POLICY_MEMBER`] and in the structure as
    /// [`RESOURCE_TYPE_MEMBER`]; that body member also MAKES a request a
    /// default-policy request, so it is a second statement of the resource type
    /// only when the structure states one too. It leads the list, the position
    /// [`DefaultPolicyOnlyParameters::stated_names`] gives it.
    fn duplicated_body_names(&self) -> Vec<String> {
        let in_details = self.in_details();
        let mut duplicated = self.body.stated_in_both(&in_details).stated_names("");
        if self.body_resource_type && in_details.resource_type {
            duplicated.insert(0, DEFAULT_POLICY_MEMBER.to_string());
        }
        duplicated
    }

    /// The structure half of the parameter set.
    fn in_details(&self) -> DefaultPolicyOnlyParameters {
        DefaultPolicyOnlyParameters::in_policy_details(self.details)
    }
}

/// The `InvalidRequestException` for a DEFAULT policy request that states its
/// configuration in the request body AND as a `PolicyDetails` structure.
///
/// `CreateLifecyclePolicy` documents the two as alternatives: "If you create a
/// default policy, you can specify the request parameters either in the request
/// body, or in the PolicyDetails request structure, but not both." The rule is
/// about one request's parameters, so it holds for every request that carries
/// them — the message is built from the request, naming the body parameters it
/// duplicated, and `MutuallyExclusiveParameters` lists them beside the structure
/// they cannot be combined with.
pub fn configuration_stated_twice(body_parameters: &[String]) -> InvalidRequestException {
    let mut parameters = vec![POLICY_DETAILS_MEMBER.to_string()];
    parameters.extend(body_parameters.iter().cloned());
    parameters_cannot_be_combined(
        format!(
            "a default policy specifies {} either in the request body or in \
             {POLICY_DETAILS_MEMBER}, but not both",
            body_parameters.join(", ")
        ),
        parameters,
    )
}

/// The wire name of the configuration structure the body parameters are an
/// alternative to.
pub const POLICY_DETAILS_MEMBER: &str = "PolicyDetails";

/// How a parameter name this service reports points at ONE element of a
/// collection: the collection member's own name, SUBSCRIPTED by the key that
/// picks the element out of it — a list element's 1-based position
/// ([`element_key`]), a map entry's key.
///
/// Every layer that names a refused parameter spells an element through here —
/// the constraint walk's path as `crate::service` renders it, and the
/// cross-member rules below — so one element of one collection is named one way
/// in `Message` and in `RequiredParameters` whichever of them refused the
/// request, and a caller needs one convention to find the field it must correct.
pub fn collection_element(collection: &str, key: &str) -> String {
    format!("{collection}[{key}]")
}

/// The key that picks the element at the 0-based `index` out of a list: 1-based,
/// as the validation path a request is walked along numbers a list element.
pub fn element_key(index: usize) -> String {
    (index + 1).to_string()
}

/// The wire name of the `PolicyDetails` member that carries a custom policy's
/// schedules: `PolicyDetails$Schedules`.
pub const SCHEDULES_MEMBER: &str = "Schedules";

/// The wire name of the `PolicyDetails` member that states a default policy's
/// resource type.
pub const RESOURCE_TYPE_MEMBER: &str = "ResourceType";

/// The wire name of the request-BODY member that states the same thing as
/// [`RESOURCE_TYPE_MEMBER`]: `CreateLifecyclePolicyRequest$DefaultPolicy` and
/// `PolicyDetails$ResourceType` carry the same documentation, so a create naming
/// both states the resource type twice.
pub const DEFAULT_POLICY_MEMBER: &str = "DefaultPolicy";

/// The wire name of the `PolicyDetails` member that states which KIND of policy
/// a configuration describes.
pub const POLICY_LANGUAGE_MEMBER: &str = "PolicyLanguage";

/// The wire name of the `PolicyDetails` member that states what a policy
/// manages: `PolicyDetails$PolicyType`.
pub const POLICY_TYPE_MEMBER: &str = "PolicyType";

/// The wire name of the `PolicyDetails` member that states what a CUSTOM
/// snapshot or AMI policy targets: `PolicyDetails$ResourceTypes`.
pub const RESOURCE_TYPES_MEMBER: &str = "ResourceTypes";

/// The member of the policy's identity `details` asks to CHANGE, by its own
/// wire name, or `None` when the replacement configuration asks for no change
/// this operation rules out.
///
/// The members are judged in the order `PolicyDetails` declares them, and the
/// FIRST one the request asks to change is the parameter the refusal names: a
/// replacement configuration that restates a whole policy of another kind
/// carries every member of that kind consistently, so naming each of them would
/// report the one change asked for several times over.
fn immutable_member_changed(
    policy: &LifecyclePolicyRecord,
    details: Option<&PolicyDetails>,
) -> Option<&'static str> {
    let details = details?;

    if details
        .policy_type
        .as_deref()
        .is_some_and(|stated| stated != policy.policy_type())
    {
        return Some(POLICY_TYPE_MEMBER);
    }

    if let Some(stated) = details.resource_types.as_ref() {
        // `ResourceTypeValuesList` is `@length(min: 1, max: 1)`, so the member
        // states exactly one type and the lists are compared as they stand.
        let stored = policy.target_resource_types();
        if !stored.is_empty() && *stated != stored {
            return Some(RESOURCE_TYPES_MEMBER);
        }
    }

    if details
        .policy_language
        .as_deref()
        .is_some_and(|stated| stated != policy.policy_language())
    {
        return Some(POLICY_LANGUAGE_MEMBER);
    }

    if let Some(stated) = details.resource_type.as_deref() {
        // Every DEFAULT policy holds a resource type — the create requires the
        // member that states which kind of default policy to create, since it
        // documents no value the service fills in
        // ([`omitted_default_policy_members`]) — and this operation may not
        // change it, so only a configuration restating the type the policy
        // holds asks for no change. A policy that is not a default policy may
        // state the member nowhere at all
        // ([`default_policy_only_parameters_refused`]).
        if policy.default_policy && policy.default_policy_type.as_deref() != Some(stated) {
            return Some(RESOURCE_TYPE_MEMBER);
        }
    }

    None
}

/// Why `details` cannot be applied to `policy` as its new configuration, or
/// `None` when it asks for no change this operation rules out.
///
/// `UpdateLifecyclePolicyRequest$PolicyDetails` is "the configuration of the
/// lifecycle policy. You cannot update the policy type or the resource type", so
/// the members of a policy's IDENTITY are fixed when it is created: a
/// replacement configuration may restate the value the policy already holds for
/// one of them (which asks for no change) but naming a different value is
/// refused with the modelled client error, whichever of those members it names
/// and whichever kind of policy holds it.
///
/// Which members those are, from `smithy_query(query="shape",
/// shape="PolicyDetails")`:
///
/// * `PolicyLanguage` — which KIND of policy the configuration describes,
///   `SIMPLIFIED` "To create a default policy" against `STANDARD` "To create a
///   custom policy".
/// * `PolicyType` — what the policy manages, "the type of policy": the policy
///   type the sentence names, compared against the one this policy reports
///   ([`LifecyclePolicyRecord::policy_type`]).
/// * `ResourceType` — "[Default policies only] ... the type of default policy",
///   the resource type of a DEFAULT policy.
/// * `ResourceTypes` — "[Custom snapshot policies only] The target resource type
///   for snapshot and AMI lifecycle policies", the resource type of a CUSTOM
///   one, compared against the types the policy targets
///   ([`LifecyclePolicyRecord::target_resource_types`]) so the `resourceTypes`
///   filter cannot start selecting a policy under another type.
///
/// Every DEFAULT policy holds a resource type, because the create requires the
/// member that states which kind of default policy to create
/// ([`omitted_default_policy_members`]), so a configuration stating one either
/// restates that type or is refused. A policy holding no value for one of the
/// OTHER members has none to change, so a configuration stating one fills in the
/// single stored copy instead of being refused.
///
/// The refusal is the service's non-updatable-parameter answer
/// ([`parameters_cannot_be_updated`]), naming the parameter
/// [`immutable_member_changed`] refuses on.
pub fn immutable_configuration_violation(
    policy: &LifecyclePolicyRecord,
    details: Option<&PolicyDetails>,
) -> Option<InvalidRequestException> {
    immutable_member_changed(policy, details)
        .map(|member| parameters_cannot_be_updated(&[member]))
}

/// Carry the type `policy` holds onto a replacement configuration that states
/// none, so the rest of the write path reads the policy's own type from it.
///
/// By the same sentence [`immutable_configuration_violation`] enforces, a
/// configuration that states no `PolicyType` asks for no change of type, so the
/// stored one stands. Without this the create-time default
/// ([`DEFAULT_POLICY_TYPE`], "The default is `EBS_SNAPSHOT_MANAGEMENT`") would
/// be stamped onto the replacement by [`materialized_policy_details`] and an AMI
/// or event-based policy would be converted to a snapshot one by an update that
/// asked for nothing of the kind.
///
/// A DEFAULT policy carries no type of its own — the kind of default policy it
/// is decides the type its reads report ([`LifecyclePolicyRecord::policy_type`]),
/// from the resource type this operation cannot change either — so there is
/// nothing to carry onto its configuration and no second copy of the type is
/// written into it.
pub fn carry_over_policy_type(policy: &LifecyclePolicyRecord, details: &mut PolicyDetails) {
    if policy.default_policy {
        return;
    }
    details
        .policy_type
        .get_or_insert_with(|| policy.policy_type());
}

/// The `InvalidRequestException` for `[Default policies only]` parameters stated
/// by a request that is not creating or updating a DEFAULT policy.
///
/// Every member [`DefaultPolicyOnlyParameters`] lists is documented `[Default
/// policies only]`, so a custom policy has nowhere to carry one and no read of
/// one can report it: the request states a configuration the service cannot
/// apply, which is exactly the "invalid parameters" the operation models
/// `InvalidRequestException` for. The message is built from the request — it
/// names the parameters refused and the constraint they fall outside.
pub fn default_policy_only_parameters_refused(parameters: &[String]) -> InvalidRequestException {
    parameters_supported_for_one_kind_only(parameters, DEFAULT_POLICY_KIND)
}

/// How Amazon Data Lifecycle Manager states the refusal of parameters that
/// belong to ONE kind of policy, stated by a request for a policy of the other
/// kind: the fixed text below, naming the parameters refused and the kind of
/// policy that takes them.
///
/// The two directions of that rule say it the same way, so the wording lives
/// here once — see [`default_policy_only_parameters_refused`] and
/// [`custom_policy_only_parameters_refused`].
fn parameters_supported_for_one_kind_only(
    parameters: &[String],
    policy_kind: &str,
) -> InvalidRequestException {
    let (parameter, is) = plural(parameters.len());
    invalid_request(format!(
        "The {parameter} {} {is} supported for {policy_kind} policies only.",
        parameters.join(", "),
    ))
}

/// How the refusal above names the kind of policy the `[Default policies only]`
/// members belong to.
const DEFAULT_POLICY_KIND: &str = "default";

/// How it names the other kind: `PolicyDetails$PolicyLanguage` documents
/// `STANDARD` as "To create a custom policy", so every policy that is not a
/// default policy is a custom one, whichever type it manages.
const CUSTOM_POLICY_KIND: &str = "custom";

/// The `PolicyDetails` members a CUSTOM policy alone is configured from that
/// `details` states, by their own wire names — empty when it states none.
///
/// `PolicyDetails` declares the members of every kind of policy at once, and the
/// documentation is what says which kind each one belongs to. These are the
/// members documented for a kind of CUSTOM policy only: `ResourceTypes`
/// ("[Custom snapshot policies only]"), `ResourceLocations`, `TargetTags`,
/// `Schedules` and `Parameters` ("[Custom snapshot and AMI policies only]"), and
/// `EventSource` and `Actions` ("[Event-based policies only]"). Every kind of
/// policy those sentences name is a custom policy — `DefaultPolicy` offers two
/// kinds of default policy and both are configured from the `[Default policies
/// only]` members ([`DefaultPolicyOnlyParameters`]) — so a default policy has
/// nowhere to carry one of them and no read of one could report it.
///
/// The names come back in the order `PolicyDetails` declares them, each
/// qualified by [`POLICY_DETAILS_PREFIX`]: the configuration structure is the
/// only place a request can state them, neither request body declaring a member
/// for any of them.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")`.
pub fn custom_policy_only_parameters_in_policy_details(
    details: Option<&PolicyDetails>,
) -> Vec<String> {
    let Some(details) = details else {
        return Vec::new();
    };
    [
        (RESOURCE_TYPES_MEMBER, details.resource_types.is_some()),
        ("ResourceLocations", details.resource_locations.is_some()),
        ("TargetTags", details.target_tags.is_some()),
        (SCHEDULES_MEMBER, details.schedules.is_some()),
        ("Parameters", details.parameters.is_some()),
        ("EventSource", details.event_source.is_some()),
        ("Actions", details.actions.is_some()),
    ]
    .into_iter()
    .filter(|(_, stated)| *stated)
    .map(|(name, _)| format!("{POLICY_DETAILS_PREFIX}{name}"))
    .collect()
}

/// Why Amazon Data Lifecycle Manager refuses the configuration a request states
/// for a DEFAULT policy, or `None` when it states only members such a policy
/// carries.
///
/// The mirror image of the `[Default policies only]` rule
/// ([`DefaultPolicyOnlyRequest::violation`]): a configuration member the
/// documentation gives to one kind of policy is accepted only by a request for a
/// policy of that kind, so a request creating or updating a DEFAULT policy may
/// state none of the members a custom policy alone is configured from
/// ([`custom_policy_only_parameters_in_policy_details`]). Refused rather than
/// dropped or stored: storing one would leave the record holding a configuration
/// of a kind it is not — a second copy of the target type beside
/// [`LifecyclePolicyRecord::default_policy_type`] for `ResourceTypes` — and
/// dropping it would report a configuration other than the one the caller
/// stated.
///
/// `default_policy` is the kind of policy the request is for, which is the write
/// path's to state: a create reads it off the request and an update off the
/// policy it addresses, whose kind it cannot change
/// ([`immutable_member_changed`]).
pub fn custom_policy_only_violation(
    default_policy: bool,
    details: Option<&PolicyDetails>,
) -> Option<InvalidRequestException> {
    if !default_policy {
        return None;
    }
    let refused = custom_policy_only_parameters_in_policy_details(details);
    (!refused.is_empty()).then(|| custom_policy_only_parameters_refused(&refused))
}

/// The `InvalidRequestException` for members a CUSTOM policy alone is configured
/// from, stated by a request for a DEFAULT policy.
///
/// The message is built from the request — it names the parameters refused and
/// the kind of policy that takes them — exactly as
/// [`default_policy_only_parameters_refused`] does for the other direction.
pub fn custom_policy_only_parameters_refused(parameters: &[String]) -> InvalidRequestException {
    parameters_supported_for_one_kind_only(parameters, CUSTOM_POLICY_KIND)
}

/// The first tag key in `keys` that begins with the reserved [`aws:`
/// prefix](RESERVED_TAG_KEY_PREFIX), or `None` when none does.
///
/// The prefix is reserved case-insensitively, so `AWS:`, `Aws:` and `aws:` are
/// one and the same reservation.
///
/// A tag key is arbitrary caller-supplied text, so the prefix is compared on
/// BYTES: a `&str` range index would panic on a key whose fourth byte falls
/// inside a multi-byte character (`aws\u{e9}`), and a request must never be able
/// to panic the handler. The reservation itself is ASCII, so a byte comparison
/// decides it exactly.
pub fn reserved_tag_key<'keys, I>(keys: I) -> Option<&'keys str>
where
    I: IntoIterator<Item = &'keys str>,
{
    keys.into_iter().find(|key| {
        key.as_bytes()
            .get(..RESERVED_TAG_KEY_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
    })
}

/// How this service refuses a tag key a caller may not set, as the whole error
/// channel of the operation that refuses it.
///
/// The reservation is a constraint of the tag key SHAPE — `TagKey` is
/// `@pattern(^(?!aws:)[a-zA-Z+-=._:/]+$)` (`smithy_query(query="shape",
/// shape="TagKey")`) — so a key under the prefix is input no request may carry,
/// whatever the store holds, and not a rule of the policy the request addresses.
/// Amazon Data Lifecycle Manager classifies that apart from the
/// `InvalidRequestException` its own rules are stated in: the answer is HTTP 400
/// under the error type `BadRequestException`, carrying
/// [`RESERVED_TAG_KEY_MESSAGE`] in the lowercase `message` member that class
/// declares. A client therefore tells "fix the request, never retry" from a
/// refusal about the addressed resource by the error type alone.
///
/// Generic over the operation's modeled error enum, so every operation that sets
/// tags refuses the same key with the same class, status and prose.
pub fn reserved_tag_key_refused<Modeled>() -> crate::scaffolded::Scaffolded<Modeled> {
    crate::scaffolded::Scaffolded::bad_request(RESERVED_TAG_KEY_MESSAGE)
}

/// A default policy's creation/retention cadence as ONE request states it,
/// beside what the policy already holds — the input to
/// [`Cadence::violation`].
///
/// `CreateInterval` and `RetainInterval` are declared three times over, by
/// `CreateLifecyclePolicyRequest`, by `UpdateLifecyclePolicyRequest` and by
/// `PolicyDetails`, and a request may state them in the body or in the
/// configuration structure. This is the ONE place their documented domain is
/// decided, so the create and the update refuse the same values.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cadence {
    /// `CreateInterval` as the request BODY states it.
    pub body_create_interval: Option<i32>,
    /// `RetainInterval` as the request BODY states it.
    pub body_retain_interval: Option<i32>,
    /// `PolicyDetails$CreateInterval` as this request's configuration states it.
    pub details_create_interval: Option<i32>,
    /// `PolicyDetails$RetainInterval` as this request's configuration states it.
    pub details_retain_interval: Option<i32>,
    /// The creation frequency the policy already holds because some earlier
    /// request stated it, for an update that states none; a create has nothing
    /// stored.
    pub stored_create_interval: Option<i32>,
    /// The retention period the policy already holds, by the same rule.
    pub stored_retain_interval: Option<i32>,
}

impl Cadence {
    /// Why Amazon Data Lifecycle Manager refuses this cadence, or `None` when it
    /// is one the documentation admits.
    ///
    /// Three documented rules, none of them expressible as a `@range`: the
    /// creation frequency's [1, 7] days, the retention period's [2, 14] days,
    /// and "it must be greater than the creation frequency to ensure that the
    /// policy retains at least 1 snapshot or AMI at any given time".
    ///
    /// The magnitude of a value is judged where the request STATES it, so the
    /// refusal names the member the caller used — the body member or the
    /// `PolicyDetails` one. The ordering is judged on the pair the policy would
    /// RUN on, which is exactly the pair a read of it reports
    /// ([`LifecyclePolicyRecord::effective_policy_details`]): each member as
    /// this request states it, else the value already stored for it (for an
    /// update that keeps its configuration), else the default that member's
    /// documentation publishes ([`DEFAULT_CREATE_INTERVAL`] /
    /// [`DEFAULT_RETAIN_INTERVAL`]). A default the service fills in is a value
    /// the policy runs on just as much as one the caller spelled out, so it
    /// takes part in the comparison: a cadence is refused precisely when the
    /// configuration it would leave stored breaks the ordering, whether the
    /// caller stated both members, one, or neither.
    pub fn violation(&self) -> Option<InvalidRequestException> {
        for (prefix, create_interval, retain_interval) in [
            ("", self.body_create_interval, self.body_retain_interval),
            (
                POLICY_DETAILS_PREFIX,
                self.details_create_interval,
                self.details_retain_interval,
            ),
        ] {
            if let Some(value) = create_interval {
                if !(CREATE_INTERVAL_MIN_DAYS..=CREATE_INTERVAL_MAX_DAYS).contains(&value) {
                    return Some(invalid_request(format!(
                        "The parameter {prefix}CreateInterval must be between \
                         {CREATE_INTERVAL_MIN_DAYS} and {CREATE_INTERVAL_MAX_DAYS} days, but \
                         {value} was requested."
                    )));
                }
            }
            if let Some(value) = retain_interval {
                if !(RETAIN_INTERVAL_MIN_DAYS..=RETAIN_INTERVAL_MAX_DAYS).contains(&value) {
                    return Some(invalid_request(format!(
                        "The parameter {prefix}RetainInterval must be between \
                         {RETAIN_INTERVAL_MIN_DAYS} and {RETAIN_INTERVAL_MAX_DAYS} days, but \
                         {value} was requested."
                    )));
                }
            }
        }

        let create_interval = self
            .body_create_interval
            .or(self.details_create_interval)
            .or(self.stored_create_interval)
            .unwrap_or(DEFAULT_CREATE_INTERVAL);
        let retain_interval = self
            .body_retain_interval
            .or(self.details_retain_interval)
            .or(self.stored_retain_interval)
            .unwrap_or(DEFAULT_RETAIN_INTERVAL);
        if retain_interval <= create_interval {
            return Some(invalid_request(format!(
                "The parameter RetainInterval ({retain_interval} days) must be greater than \
                 the creation frequency CreateInterval ({create_interval} days) to ensure \
                 that the policy retains at least 1 snapshot or AMI at any given time."
            )));
        }
        None
    }
}

/// The creation interval a `CreateRule` STATED, or `None` when it states none.
///
/// [`UNSET_RULE_INTERVAL`] is the value this service itself materializes for a
/// rule that stated no interval — a cron-driven creation rule
/// ([`materialized_policy_details`]) — and every read of such a policy reports
/// it, so it is a value this API's own responses carry rather than one a request
/// asked for. Every check that judges what a request asked for reads the
/// interval through here, so the configuration a read REPORTS is one the create
/// and the update accept back unchanged.
///
/// It is also why the member hands its boundary validation to these checks (the
/// `skip_validation` note on [`CreateRule`]): the value the service reports must
/// reach the handler to be read as the filler it is.
fn stated_create_interval(rule: &CreateRule) -> Option<i32> {
    rule.interval.filter(|value| *value != UNSET_RULE_INTERVAL)
}

/// The first schedule in `details` whose `CreateRule` states a creation interval
/// outside [`SUPPORTED_CREATE_RULE_INTERVALS`], as (how the schedule is named,
/// the interval it stated), or `None` when every schedule states a supported one.
///
/// Only an interval the request STATED is judged ([`stated_create_interval`]):
/// the materialized filler is not a value the caller chose. The supported set is
/// what `CreateRule$Interval`'s documentation offers a caller, and it is narrower
/// than the member's modelled bound, so this is the whole domain of the member —
/// every other value is refused here, whichever side of the set it falls.
///
/// EVERY element of `Schedules` is examined (`@length(min: 1, max: 4)`), not just
/// the first: a policy is refused for the interval of whichever schedule carries
/// one the service does not support.
pub fn schedule_with_unsupported_create_interval(
    details: Option<&PolicyDetails>,
) -> Option<(String, i32)> {
    details?
        .schedules
        .as_ref()?
        .iter()
        .enumerate()
        .find_map(|(position, schedule)| {
            let interval = stated_create_interval(schedule.create_rule.as_ref()?)?;
            (!SUPPORTED_CREATE_RULE_INTERVALS.contains(&interval))
                .then(|| (schedule_label(position, schedule), interval))
        })
}

/// The `InvalidRequestException` for a creation interval the service does not
/// support. The message is built from the request: it names the schedule that
/// carries the value, the value itself and the set it falls outside.
pub fn unsupported_create_rule_interval(schedule: &str, interval: i32) -> InvalidRequestException {
    let supported: Vec<String> = SUPPORTED_CREATE_RULE_INTERVALS
        .iter()
        .map(i32::to_string)
        .collect();
    invalid_request(format!(
        "The parameter CreateRule.Interval in schedule {{{schedule}}} was {interval}. The \
         supported values are {}.",
        supported.join(", ")
    ))
}

/// How a refusal names the schedule at `position` in `Schedules`.
///
/// `Schedule$Name` is optional, so a schedule that carries none is named by its
/// position instead — what matters is that the caller can tell WHICH of its
/// schedules to correct.
fn schedule_label(position: usize, schedule: &Schedule) -> String {
    match schedule.name.as_deref() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => collection_element(SCHEDULES_MEMBER, &element_key(position)),
    }
}

/// The wire path a refusal names the schedule at `position` by, so a caller can
/// tell which member of which list element to correct.
fn schedule_path(position: usize) -> String {
    collection_element(
        &format!("{POLICY_DETAILS_PREFIX}{SCHEDULES_MEMBER}"),
        &element_key(position),
    )
}

/// The `PolicyDetails` members a request OMITTED that the kind of policy its
/// configuration describes cannot be built from, each by its own wire name, as
/// a refusal lists an omitted parameter. Empty when the configuration states
/// every one of them.
///
/// `PolicyDetails` declares the members of every policy kind at once, so the
/// model can mark none of them `@required`: which ones a request must carry
/// depends on the kind of policy it configures, and the documentation is what
/// states them, member by member.
///
/// * A custom snapshot or AMI policy is built from `ResourceTypes` ("the target
///   resource type for snapshot and AMI lifecycle policies"), `TargetTags`
///   ("the single tag that identifies targeted resources for this policy") and
///   `Schedules` ("A policy can have up to four schedules—one mandatory
///   schedule and up to three optional schedules"). Without them nothing says
///   what the policy targets or when it acts.
/// * An event-based policy is built from `EventSource` ("the event that
///   activates the event-based policy") and `Actions` ("the actions to be
///   performed when the event-based policy is activated") instead.
///
/// Whether the policy is a DEFAULT one is the write path's to state ([`PolicyKind`],
/// as for [`materialized_policy_details`]), and WHICH kind of custom policy a
/// configuration describes is decided by its `PolicyType` alone
/// ([`configures_event_based_policy`]), so the set of members required of it is
/// fixed before any of them is read: a configuration that names no type
/// describes a snapshot policy ("The default is `EBS_SNAPSHOT_MANAGEMENT`") and
/// is required to state the members one is built from, whichever other members
/// it happens to carry.
///
/// A DEFAULT policy is configured from the `[Default policies only]` members,
/// which are judged by [`omitted_default_policy_members`] from the KIND of
/// default policy the request describes rather than from the structure: those
/// members may be stated in the request body instead of inside it.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")`.
pub fn omitted_configuration_members(
    details: &PolicyDetails,
    kind: PolicyKind<'_>,
) -> Vec<String> {
    if let PolicyKind::Default { resource_type } = kind {
        return omitted_default_policy_members(resource_type);
    }
    let required: Vec<(&str, bool)> = if configures_event_based_policy(details) {
        vec![
            ("EventSource", details.event_source.is_some()),
            ("Actions", details.actions.is_some()),
        ]
    } else {
        vec![
            ("ResourceTypes", details.resource_types.is_some()),
            ("TargetTags", details.target_tags.is_some()),
            (SCHEDULES_MEMBER, details.schedules.is_some()),
        ]
    };
    required
        .into_iter()
        .filter(|(_, stated)| !*stated)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The `[Default policies only]` member a DEFAULT policy's request leaves
/// unstated that such a policy cannot be built without, by its own wire name.
/// Empty when the request states it.
///
/// All but one of those members document the value the service fills in for a
/// request that omits it — `CreateInterval` "If you do not specify a value, the
/// default is 1", `RetainInterval` 7, `CopyTags` and `ExtendDeletion` false (the
/// sentence each of [`DEFAULT_CREATE_INTERVAL`], [`DEFAULT_RETAIN_INTERVAL`],
/// [`DEFAULT_COPY_TAGS`] and [`DEFAULT_EXTEND_DELETION`] quotes) — so a request
/// omitting one of them asks for the documented value and is complete without
/// it.
///
/// The resource type documents no such value: `PolicyDetails$ResourceType` and
/// `CreateLifecyclePolicyRequest$DefaultPolicy` say "Specify the type of default
/// policy to create", with `VOLUME` for the snapshot kind and `INSTANCE` for the
/// AMI kind and nothing for a request that names neither. It is also the member
/// that decides what such a policy manages and targets
/// ([`default_policy_managed_type`],
/// [`LifecyclePolicyRecord::target_resource_types`]), so a request stating it
/// NOWHERE — not in the body, not in the configuration structure, and not as the
/// `PolicyType` one kind of default policy alone manages
/// ([`default_policy_kind`]) — describes no kind of default policy and is
/// refused rather than stored as a policy whose reads and filters could not
/// agree on what it is.
///
/// Source: `smithy_query(query="doc", shape="PolicyDetails",
/// member="ResourceType")`.
pub fn omitted_default_policy_members(resource_type: Option<&str>) -> Vec<String> {
    match resource_type {
        Some(_) => Vec::new(),
        None => vec![RESOURCE_TYPE_MEMBER.to_string()],
    }
}

/// The `InvalidRequestException` for a create that states no policy
/// configuration at all.
///
/// `CreateLifecyclePolicy` documents two ways of saying what a policy would do:
/// `DefaultPolicy`, which "specif[ies] the type of default policy to create",
/// and `PolicyDetails`, "the configuration details of the lifecycle policy". A
/// request carrying neither describes no policy, so there is none to create.
pub fn no_policy_configuration() -> InvalidRequestException {
    required_parameters_missing(vec![POLICY_DETAILS_MEMBER.to_string()])
}

/// The `InvalidRequestException` for a configuration that omitted `parameters`
/// the kind of policy it describes is built from. The refusal is built from the
/// request: it names the parameters that were omitted, in the service's
/// missing-required-parameter wording.
pub fn configuration_members_omitted(parameters: &[String]) -> InvalidRequestException {
    required_parameters_missing(parameters.to_vec())
}

/// `("parameter", "is")` or `("parameters", "are")`, so a refusal that names one
/// member and one that names several read the same way.
fn plural(count: usize) -> (&'static str, &'static str) {
    if count == 1 {
        ("parameter", "is")
    } else {
        ("parameters", "are")
    }
}

/// Why Amazon Data Lifecycle Manager refuses one of the schedules `details`
/// carries, or `None` when every one of them states each of its nested rules in
/// a form the documentation admits, at a magnitude the rules beside it admit.
///
/// The documented cross-member rules a schedule's nested rules obey, none of
/// them expressible in the model, judged on EVERY element of `Schedules`
/// (`@length(min: 1, max: 4)`) so a policy is refused for whichever of its
/// schedules carries the fault:
///
/// * `CreateRule` — "You must specify either CronExpression, or Interval,
///   IntervalUnit, and Times." A schedule states when to create in exactly one
///   of the two forms: neither form, both forms, or an interval form missing a
///   member it names, are each refused.
/// * `RetainRule` — "You can retain snapshots based on either a count or a time
///   interval. Count-based retention: You must specify Count ... Age-based
///   retention: You must specify Interval and IntervalUnit." A retention rule
///   states exactly one of the two modes, and the retention that mode states
///   meets the floor the OTHER rules of the same schedule raise — see
///   [`retention_floor_violation`]. Those same rules are what make stating a
///   retention mandatory in the first place ("If you specify a FastRestoreRule,
///   ShareRule, or a CrossRegionCopyRule, then you must specify a retention
///   count of `1` or more"), so a schedule carrying one of them and no
///   `RetainRule` is refused too — see [`retention_required_violation`].
/// * `FastRestoreRule` — "You can enable fast snapshot restore based on either a
///   count or a time interval." The same either/or, so the same three refusals:
///   a rule that enables fast restore on neither a count nor an interval of
///   snapshots says nothing about what it enables, and one that states both
///   states two answers to the same question. This is also what makes the rule a
///   SPECIFIED one for [`rules_requiring_retention`] honest: a rule that raises
///   the schedule's retention floor has itself said how much it acts on.
/// * `DeprecateRule` — "For age-based schedules, you must specify Interval and
///   IntervalUnit. For count-based schedules, you must specify Count." The same
///   either/or, except that the schedule's own retention mode decides WHICH of
///   the two forms the rule states, so a form the other mode's sentence names is
///   refused as the form the schedule's mode requires being omitted.
/// * `ArchiveRule`'s `RetentionArchiveTier` — "For count-based schedules, you
///   must specify Count. For age-based schedules, you must specify Interval and
///   IntervalUnit." The same sentence as the deprecation rule's, judged the same
///   way, on the archive-tier retention nested inside the schedule's
///   `ArchiveRule` (`ArchiveRule$RetainRule` and
///   `ArchiveRetainRule$RetentionArchiveTier` are both `@required`, so a
///   schedule carrying an `ArchiveRule` carries the tier).
/// * `CreateRule` again, for the SECOND bullet of its note — "If you need to
///   specify an ArchiveRule for the schedule, then you must specify a creation
///   frequency of at least 28 days." A schedule carrying an `ArchiveRule` states
///   a creation frequency of at least that, whichever of the two forms states it
///   — see [`archive_creation_frequency_violation`]. It is judged after the
///   forms, since the frequency is read from the form the rule stated.
///
/// Every one of them is the one rule — state exactly one of the two documented
/// forms, and state it completely. The three whose two forms are a COUNT and a
/// time interval are judged by [`EitherOrRule`]; `CreateRule`, whose two forms
/// are a cron expression and an interval, is judged by the same rule over its
/// own pair of forms. A schedule rule the documentation gives no alternative
/// forms (a `ShareRule`, a `CrossRegionCopyRule`'s own retention and deprecation
/// rules) has no form for either of them to judge.
pub fn schedule_rule_violation(details: Option<&PolicyDetails>) -> Option<InvalidRequestException> {
    details?
        .schedules
        .as_ref()?
        .iter()
        .enumerate()
        .find_map(|(position, schedule)| {
            let label = schedule_label(position, schedule);
            let path = schedule_path(position);
            create_rule_violation(&label, &path, schedule.create_rule.as_ref())
                .or_else(|| retain_rule_violation(&label, &path, schedule))
                .or_else(|| fast_restore_rule_violation(&label, &path, schedule))
                .or_else(|| deprecate_rule_violation(&label, &path, schedule))
                .or_else(|| archive_tier_violation(&label, &path, schedule))
                .or_else(|| archive_creation_frequency_violation(&label, schedule))
        })
}

/// Why a schedule's creation rule is refused, or `None` when it states when to
/// create in exactly one of the two documented forms.
///
/// `Times` is not required of the interval form even though the sentence names
/// it: its own documentation gives it a default ("If you do not specify a time,
/// Amazon Data Lifecycle Manager selects a time within the next 24 hours"), so a
/// rule that omits it still says when to create.
///
/// A creation interval of [`UNSET_RULE_INTERVAL`] is the value this service
/// itself materializes for a rule that stated none, so it is not an interval the
/// request asked for ([`stated_create_interval`]): a unit, a time, or any other
/// interval is what states the interval form. That is what makes the cron form
/// a read reports — which carries the materialized interval beside the
/// expression — one this check still sees as the cron form alone.
fn create_rule_violation(
    label: &str,
    path: &str,
    rule: Option<&CreateRule>,
) -> Option<InvalidRequestException> {
    let cron_based = rule.is_some_and(|rule| rule.cron_expression.is_some());
    let interval = rule.and_then(stated_create_interval);
    let interval_unit = rule.and_then(|rule| rule.interval_unit.as_deref());
    // Any one of the interval form's three members states the schedule as an
    // interval, whether or not the other two are there.
    let interval_based = interval.is_some()
        || interval_unit.is_some()
        || rule.is_some_and(|rule| rule.times.as_ref().is_some_and(|times| !times.is_empty()));

    match (cron_based, interval_based) {
        (true, true) => Some(both_forms_stated(
            label,
            "Interval based CreateRule and Cron based CreateRule",
        )),
        (false, false) => Some(required_in_schedule(
            label,
            vec![format!("{path}.CreateRule")],
            "a schedule states when to create either as a CronExpression or as an Interval and \
             IntervalUnit",
        )),
        (false, true) => {
            let omitted = omitted_members(
                &format!("{path}.CreateRule"),
                [
                    ("Interval", interval.is_some()),
                    ("IntervalUnit", interval_unit.is_some()),
                ],
            );
            (!omitted.is_empty()).then(|| {
                required_in_schedule(
                    label,
                    omitted,
                    "an interval based CreateRule states both Interval and IntervalUnit",
                )
            })
        }
        (true, false) => None,
    }
}

/// Which of the two alternative forms a schedule rule states what it does in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleForm {
    /// A number of snapshots or AMIs: the rule's `Count`.
    Count,
    /// A span of time: the rule's `Interval` and `IntervalUnit`.
    Interval,
}

/// Which of the two forms a rule's documentation requires of it, and the
/// documented sentence a refusal of each cites.
enum RequiredForm {
    /// Either form says what the rule does and the caller chooses between them.
    /// `neither` is why a rule stating neither of them is refused, and
    /// `interval` why an interval form missing one of its two members is.
    Either {
        neither: &'static str,
        interval: &'static str,
    },
    /// The rule's documentation names one form per retention mode of the
    /// schedule carrying it, and that schedule retains by a COUNT: the count
    /// form is the one required, and `.0` is the sentence requiring it.
    Count(&'static str),
    /// The same, for a schedule that retains by a time INTERVAL.
    Interval(&'static str),
}

/// One rule nested in a schedule whose documentation offers two alternative
/// forms — a count of resources, or a time interval stated as an `Interval` and
/// an `IntervalUnit` — exactly one of which says what the rule does.
///
/// A rule stating NEITHER form says nothing about what it does, one stating BOTH
/// gives two answers to the one question, and one stating the interval form
/// without both of its members states it incompletely: each is refused, and
/// which of the two forms a rule may state is either the caller's choice or the
/// schedule's retention mode's ([`RequiredForm`]).
struct EitherOrRule<'a> {
    /// How a refusal names the schedule carrying the rule.
    label: &'a str,
    /// The wire path of the rule itself, which a refusal names it and its
    /// members by.
    path: String,
    /// The two forms, as [`both_forms_stated`] names them.
    forms: &'static str,
    /// The members the rule states its form in.
    count: Option<i32>,
    interval: Option<i32>,
    interval_unit: Option<&'a str>,
    /// Which form the documentation requires, and why.
    required: RequiredForm,
}

impl EitherOrRule<'_> {
    /// Why this rule's form is refused, or `None` when it states exactly one of
    /// the two forms, completely, and that form is the one required of it.
    fn violation(&self) -> Option<InvalidRequestException> {
        if self.count.is_some() && self.states_interval_form() {
            return Some(both_forms_stated(self.label, self.forms));
        }
        match self.required {
            RequiredForm::Either { neither, interval } => match self.stated() {
                None => Some(required_in_schedule(
                    self.label,
                    vec![self.path.clone()],
                    neither,
                )),
                Some(RuleForm::Interval) => self.incomplete_interval_form(interval),
                Some(RuleForm::Count) => None,
            },
            RequiredForm::Count(reason) => self.count.is_none().then(|| {
                required_in_schedule(self.label, vec![format!("{}.Count", self.path)], reason)
            }),
            // A rule required to state the interval form and stating the count
            // form instead has omitted BOTH of the interval form's members, so
            // the one check covers the wrong form and the incomplete one alike.
            RequiredForm::Interval(reason) => self.incomplete_interval_form(reason),
        }
    }

    /// Whether the rule states the interval form at all.
    ///
    /// An interval of [`UNSET_RULE_INTERVAL`] is the value this service itself
    /// materializes for a rule that stated none, and every read of such a policy
    /// reports it, so it is not an interval the request asked for: a unit, or any
    /// other interval, is what states the form.
    fn states_interval_form(&self) -> bool {
        self.interval_unit.is_some()
            || self
                .interval
                .is_some_and(|value| value != UNSET_RULE_INTERVAL)
    }

    /// Which form the rule STATED, or `None` when it states neither. A rule
    /// stating both answers the count form, which is why the callers refuse that
    /// pair before asking.
    fn stated(&self) -> Option<RuleForm> {
        if self.count.is_some() {
            Some(RuleForm::Count)
        } else if self.states_interval_form() {
            Some(RuleForm::Interval)
        } else {
            None
        }
    }

    /// Why the interval form as this rule states it is refused, naming the
    /// members of it the rule omitted, or `None` when it states both of them.
    fn incomplete_interval_form(&self, reason: &str) -> Option<InvalidRequestException> {
        let omitted = omitted_members(
            &self.path,
            [
                ("Interval", self.interval.is_some()),
                ("IntervalUnit", self.interval_unit.is_some()),
            ],
        );
        (!omitted.is_empty()).then(|| required_in_schedule(self.label, omitted, reason))
    }
}

/// The form rule a schedule's `RetainRule` obeys: "You can retain snapshots
/// based on either a count or a time interval", with "Count-based retention: You
/// must specify Count" and "Age-based retention: You must specify Interval and
/// IntervalUnit" stating each form completely.
fn retain_rule_form<'a>(label: &'a str, path: &str, rule: &'a RetainRule) -> EitherOrRule<'a> {
    EitherOrRule {
        label,
        path: format!("{path}.RetainRule"),
        forms: "Count based RetainRule and interval based RetainRule",
        count: rule.count,
        interval: rule.interval,
        interval_unit: rule.interval_unit.as_deref(),
        required: RequiredForm::Either {
            neither: "a RetainRule retains either based on a Count or based on an Interval and \
                      IntervalUnit",
            interval: "age based retention states both Interval and IntervalUnit",
        },
    }
}

/// Which retention mode the schedule's `RetainRule` states, or `None` when the
/// schedule states no retention at all.
///
/// It is read only after [`retain_rule_violation`] has judged that rule, so a
/// schedule reaching the rules whose form follows the mode states at most one of
/// the two modes.
fn schedule_retention_mode(label: &str, path: &str, schedule: &Schedule) -> Option<RuleForm> {
    retain_rule_form(label, path, schedule.retain_rule.as_ref()?).stated()
}

/// Which form a rule whose documentation names one per retention mode must
/// state, for the schedule's mode, and why.
///
/// A schedule that states no retention mode is held to neither sentence — there
/// is no mode for the rule's form to follow — so the rule is judged by the
/// either/or alone, which is what both sentences together say: state one of the
/// two forms, completely.
fn form_for_retention_mode(
    mode: Option<RuleForm>,
    count: &'static str,
    interval: &'static str,
    neither: &'static str,
) -> RequiredForm {
    match mode {
        Some(RuleForm::Count) => RequiredForm::Count(count),
        Some(RuleForm::Interval) => RequiredForm::Interval(interval),
        None => RequiredForm::Either { neither, interval },
    }
}

/// Why a schedule's retention rule is refused, or `None` when it states exactly
/// one of the two documented retention modes and retains at least as much as the
/// schedule's other rules require.
///
/// The whole `schedule` is judged, not the retention rule alone: the rules the
/// schedule carries beside it decide both whether a retention must be stated at
/// all ([`retention_required_violation`]) and the floor on how little a stated
/// one may retain ([`retention_floor_violation`]).
fn retain_rule_violation(
    label: &str,
    path: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    let Some(rule) = schedule.retain_rule.as_ref() else {
        return retention_required_violation(label, path, schedule);
    };
    let form = retain_rule_form(label, path, rule);
    if let Some(refusal) = form.violation() {
        return Some(refusal);
    }

    // The form check refuses a rule that states neither mode, so the retention
    // judged against the floor is the one the mode it DID state retains by.
    match form.stated()? {
        RuleForm::Interval => {
            // The floor is stated in DAYS ("a retention interval of 1 day or
            // more"), and every `RetentionIntervalUnitValues` member is at least
            // a day long, so an interval of 1 or more in ANY unit meets it and
            // only an interval of 0 falls short of it.
            let requirement = format!(
                "a retention interval of {RETENTION_FLOOR_WITH_DEPENDENT_RULE} day or more"
            );
            retention_floor_violation(
                label,
                schedule,
                "Interval",
                rule.interval.unwrap_or(UNSET_RULE_INTERVAL),
                &requirement,
            )
        }
        RuleForm::Count => {
            let requirement =
                format!("a retention count of {RETENTION_FLOOR_WITH_DEPENDENT_RULE} or more");
            retention_floor_violation(
                label,
                schedule,
                "Count",
                rule.count.unwrap_or_default(),
                &requirement,
            )
        }
    }
}

/// Why a schedule's fast-restore rule is refused, or `None` when the schedule
/// carries none or states it in exactly one of the two documented forms.
///
/// "You can enable fast snapshot restore based on either a count or a time
/// interval" — the same either/or as the retention rule's, and the caller's
/// choice between the two forms, since nothing ties this rule to how the
/// schedule retains. The Availability Zones the rule also names say WHERE fast
/// restore is enabled, not how much of it, so a rule naming them alone states
/// neither form.
fn fast_restore_rule_violation(
    label: &str,
    path: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    let rule: &FastRestoreRule = schedule.fast_restore_rule.as_ref()?;
    EitherOrRule {
        label,
        path: format!("{path}.FastRestoreRule"),
        forms: "Count based FastRestoreRule and interval based FastRestoreRule",
        count: rule.count,
        interval: rule.interval,
        interval_unit: rule.interval_unit.as_deref(),
        required: RequiredForm::Either {
            neither: "a FastRestoreRule enables fast snapshot restore either based on a Count or \
                      based on an Interval and IntervalUnit",
            interval: "an interval based FastRestoreRule states both Interval and IntervalUnit",
        },
    }
    .violation()
}

/// Why a schedule's AMI deprecation rule is refused, or `None` when the schedule
/// carries none or states it in the form its retention mode requires.
///
/// "For age-based schedules, you must specify Interval and IntervalUnit. For
/// count-based schedules, you must specify Count" — the same either/or as the
/// retention rule's, with the schedule's own retention mode deciding which of
/// the two forms the rule states, so a rule deprecating by the other mode's form
/// is refused for omitting the one its schedule's mode requires.
fn deprecate_rule_violation(
    label: &str,
    path: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    let rule: &DeprecateRule = schedule.deprecate_rule.as_ref()?;
    EitherOrRule {
        label,
        path: format!("{path}.DeprecateRule"),
        forms: "Count based DeprecateRule and interval based DeprecateRule",
        count: rule.count,
        interval: rule.interval,
        interval_unit: rule.interval_unit.as_deref(),
        required: form_for_retention_mode(
            schedule_retention_mode(label, path, schedule),
            "a count based schedule deprecates based on a Count",
            "an age based schedule deprecates based on an Interval and IntervalUnit",
            "a DeprecateRule deprecates based on a Count for a count based schedule and based on \
             an Interval and IntervalUnit for an age based schedule",
        ),
    }
    .violation()
}

/// Why the archive-tier retention of a schedule's `ArchiveRule` is refused, or
/// `None` when the schedule carries no such rule or states the tier in the form
/// its retention mode requires.
///
/// `RetentionArchiveTier` carries the deprecation rule's sentence word for word
/// ("For count-based schedules, you must specify Count. For age-based schedules,
/// you must specify Interval and IntervalUnit"), so it is judged the same way.
/// `ArchiveRule$RetainRule` and `ArchiveRetainRule$RetentionArchiveTier` are
/// both `@required`, so the tier is there to judge whenever the rule is.
fn archive_tier_violation(
    label: &str,
    path: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    let tier: &RetentionArchiveTier = &schedule
        .archive_rule
        .as_ref()?
        .retain_rule
        .retention_archive_tier;
    EitherOrRule {
        label,
        path: format!("{path}.ArchiveRule.RetainRule.RetentionArchiveTier"),
        forms: "Count based RetentionArchiveTier and interval based RetentionArchiveTier",
        count: tier.count,
        interval: tier.interval,
        interval_unit: tier.interval_unit.as_deref(),
        required: form_for_retention_mode(
            schedule_retention_mode(label, path, schedule),
            "a count based schedule retains archived snapshots based on a Count",
            "an age based schedule retains archived snapshots based on an Interval and \
             IntervalUnit",
            "a RetentionArchiveTier retains based on a Count for a count based schedule and based \
             on an Interval and IntervalUnit for an age based schedule",
        ),
    }
    .violation()
}

/// Why a schedule that states NO retention at all is refused, or `None` when
/// every other rule it carries acts on creation alone.
///
/// The documented requirement is on what the schedule SPECIFIES — "If you
/// specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must
/// specify a retention count of `1` or more", and the same for a retention
/// interval — so a schedule carrying one of those rules and NO retention rule
/// meets it no better than one stating a retention of nothing: it specifies no
/// retention whatsoever, and the rule has nothing kept to act on
/// ([`retention_floor_violation`]). The refusal is the omitted-parameter class,
/// since the member the sentence requires is the one that is missing, and it
/// names the rules that require it.
///
/// A schedule carrying none of those rules is refused nothing: the model
/// declares no retention rule for a schedule, and no other documented sentence
/// requires one.
fn retention_required_violation(
    label: &str,
    path: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    let dependent = rules_requiring_retention(schedule);
    if dependent.is_empty() {
        return None;
    }
    Some(required_in_schedule(
        label,
        vec![format!("{path}.RetainRule")],
        &format!(
            "a schedule that specifies {} must specify a retention count of \
             {RETENTION_FLOOR_WITH_DEPENDENT_RULE} or more",
            named_in_prose(&dependent)
        ),
    ))
}

/// Why `retained` — the retention the schedule's `member` states, in the mode it
/// retains by — is refused, or `None` when it meets the floor the schedule's
/// other rules raise.
///
/// A schedule may retain NOTHING (`StandardTierRetainRuleCount` is
/// `@range(min: 0, max: 1000)`, `StandardTierRetainRuleInterval` is
/// `@range(min: 0)`, and an `ArchiveRule` is documented as archiving such
/// snapshots "immediately after creation") only while every other rule it
/// carries acts on creation alone. A rule that acts on the snapshots the
/// schedule KEEPS — enabling fast restore on them, sharing them, copying them to
/// another Region — raises the floor to
/// [`RETENTION_FLOOR_WITH_DEPENDENT_RULE`]: a schedule retaining none of them has
/// none for that rule to act on. The floor applies to whichever mode the rule
/// retains by, since the documentation states it for both, and it holds whatever
/// else the schedule carries: the `ArchiveRule` sentence grants a permission for
/// a schedule that has no such rule, while this one states a requirement.
fn retention_floor_violation(
    label: &str,
    schedule: &Schedule,
    member: &str,
    retained: i32,
    requirement: &str,
) -> Option<InvalidRequestException> {
    if retained >= RETENTION_FLOOR_WITH_DEPENDENT_RULE {
        return None;
    }
    let dependent = rules_requiring_retention(schedule);
    if dependent.is_empty() {
        return None;
    }
    Some(invalid_request(format!(
        "The parameter RetainRule.{member} in schedule {{{label}}} was {retained}. A schedule \
         that specifies {} must specify {requirement}.",
        named_in_prose(&dependent)
    )))
}

/// Which of the schedule's rules act on the snapshots it RETAINS, in the order
/// `RetainRule` names them, and so raise its retention floor. Empty when the
/// schedule carries none of them.
///
/// `Schedule$ShareRules` (`@length(min: 0, max: 1)`) and
/// `Schedule$CrossRegionCopyRules` (`@length(min: 0, max: 3)`) carry their rules
/// as lists, so a schedule specifies one of those rules by carrying a NON-EMPTY
/// list; `Schedule$FastRestoreRule` is a single structure. Each is named in the
/// refusal by the name of the rule SHAPE, which is how the documented sentence
/// and the API reference name it.
fn rules_requiring_retention(schedule: &Schedule) -> Vec<&'static str> {
    [
        ("FastRestoreRule", schedule.fast_restore_rule.is_some()),
        (
            "ShareRule",
            schedule
                .share_rules
                .as_ref()
                .is_some_and(|rules| !rules.is_empty()),
        ),
        (
            "CrossRegionCopyRule",
            schedule
                .cross_region_copy_rules
                .as_ref()
                .is_some_and(|rules| !rules.is_empty()),
        ),
    ]
    .into_iter()
    .filter(|(_, specified)| *specified)
    .map(|(name, _)| name)
    .collect()
}

/// `names` as a refusal reads them: `a X`, `a X and a Y`, `a X, a Y and a Z`.
///
/// The empty arm is defensive: every caller refuses nothing when the schedule
/// carries none of the rules, so none of them reaches this with an empty slice.
fn named_in_prose(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => format!("a {one}"),
        [leading @ .., last] => format!("a {} and a {last}", leading.join(", a ")),
    }
}

/// The `InvalidRequestException` for a schedule rule that states BOTH of the two
/// alternative forms its documentation offers, where exactly one of them says
/// what the rule does. `forms` names the two, and the message names the schedule,
/// which is what tells a caller which of its schedules to correct.
///
/// The refusal is carried by the message and the code alone: this is a rule about
/// the FORM of a structure nested inside the configuration a request parameter
/// carries, not about two of the request's own parameters, so the structured
/// [`MutuallyExclusiveParameters`](parameters_cannot_be_combined) list a client
/// reads as the authoritative set of combined request parameters stays unset.
fn both_forms_stated(label: &str, forms: &str) -> InvalidRequestException {
    invalid_request(format!(
        "{forms} cannot both be present in schedule {{{label}}}."
    ))
}

/// The wire paths of the members of `path` that are OMITTED, for the
/// `RequiredParameters` of a refusal.
fn omitted_members<const N: usize>(path: &str, members: [(&str, bool); N]) -> Vec<String> {
    members
        .into_iter()
        .filter(|(_, stated)| !*stated)
        .map(|(name, _)| format!("{path}.{name}"))
        .collect()
}

/// The `InvalidRequestException` for a schedule that omitted `parameters` the
/// form its rule states requires. `why` is the rule the requirement comes from,
/// and the message names the schedule, which is what tells a caller which of its
/// schedules to correct.
fn required_in_schedule(
    label: &str,
    parameters: Vec<String>,
    why: &str,
) -> InvalidRequestException {
    let (noun, verb) = plural(parameters.len());
    let message = format!(
        "The {noun} {} {verb} required in schedule {{{label}}}: {why}.",
        parameters.join(", ")
    );
    required_parameters_omitted(message, parameters)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDlmDlm20180112State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDlmDlm20180112State>)
}
