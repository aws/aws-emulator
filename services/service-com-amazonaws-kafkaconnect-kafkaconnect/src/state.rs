// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kafkaconnect-kafkaconnect service.
//!
//! Four resources live here — custom plugins, connectors, worker
//! configurations and the connector operations a call against a connector
//! starts — beside the conversions from the shapes a create request carries to
//! the `*Description` shapes the read operations answer with, and the paging
//! helper the list operations share. Every handler in `src/ops/` reaches this
//! ONE struct through [`service_state`], so no two operations can read two
//! copies of one resource.
//!
//! Every resource here belongs to exactly ONE region — the region the request
//! that created it was signed for — and every lookup takes the region of the
//! call that is asking ([`InRegion`]), so a call answered through one region's
//! endpoint neither lists, resolves nor meters another region's resources.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime};

use sha2::Digest;

#[allow(unused_imports)]
use crate::ops::types::*;

/// The marker every `nextToken` this service issues carries, ahead of the
/// discriminator of the listing that issued it ([`Listing::token_prefix`]), so a
/// token this service did not issue — or one another of its listings issued — is
/// refused rather than honoured as a resume point.
const PAGE_TOKEN_MARKER: &str = "kafkaconnect:v1:";

/// The region an ARN carries when the caller signed with no credential scope the
/// service can read (an unsigned request in a test, say). Every emulator
/// defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace (`arnNamespace` in the model's `aws.api#service`
/// trait) every ARN this service mints carries.
pub const ARN_SERVICE: &str = "kafkaconnect";

/// The ARN service namespace of the bucket a custom plugin's file lives in
/// (`S3Location$bucketArn`: "The Amazon Resource Name (ARN) of an S3 bucket").
pub const S3_ARN_SERVICE: &str = "s3";

/// The ARN service namespace of the role a connector runs under
/// (`CreateConnectorRequest$serviceExecutionRoleArn`: "The Amazon Resource Name
/// (ARN) of the IAM role used by the connector").
pub const IAM_ARN_SERVICE: &str = "iam";

/// How long after the call that starts it a resource's move out of its transient
/// state is due.
///
/// `CreateCustomPlugin` and `CreateConnector` both answer with a non-terminal
/// state (`CREATING`), and `DeleteConnector` / `DeleteCustomPlugin` both answer
/// with `DELETING` for a resource that is still there; the clock, not the next
/// call, is what finishes each of those. The span is deliberately non-zero, so a
/// caller that reads the resource straight after creating it still observes
/// `CREATING` — exactly as it would against the real service, where the
/// transition takes minutes. It is a chosen span (AWS documents no duration for
/// it), kept short enough that a test can wait it out.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `CustomPluginState` while the plugin's file is still being taken on; one of
/// [`CUSTOM_PLUGIN_STATE_VALUES`].
pub const CUSTOM_PLUGIN_CREATING: &str = "CREATING";
/// `CustomPluginState` once the plugin can be referenced by a connector.
pub const CUSTOM_PLUGIN_ACTIVE: &str = "ACTIVE";
/// `CustomPluginState` between `DeleteCustomPlugin` and the plugin's removal.
pub const CUSTOM_PLUGIN_DELETING: &str = "DELETING";

/// `WorkerConfigurationState` once the configuration can be referenced by a
/// connector; one of [`WORKER_CONFIGURATION_STATE_VALUES`]. A worker
/// configuration is nothing but stored properties, so
/// `CreateWorkerConfiguration` answers with this state straight away — the
/// model gives the shape no creating state at all (its two values are `ACTIVE`
/// and `DELETING`), so there is no transition for the clock to owe.
pub const WORKER_CONFIGURATION_ACTIVE: &str = "ACTIVE";
/// `WorkerConfigurationState` between `DeleteWorkerConfiguration` and the
/// configuration's removal.
pub const WORKER_CONFIGURATION_DELETING: &str = "DELETING";

/// The revision a freshly created worker configuration is at.
/// `WorkerConfigurationRevisionSummary$revision` is modelled `__long`, and a
/// configuration's first revision is one.
pub const FIRST_WORKER_CONFIGURATION_REVISION: i64 = 1;

/// `ConnectorState` while the connector's workers are being provisioned; one of
/// [`CONNECTOR_STATE_VALUES`].
pub const CONNECTOR_CREATING: &str = "CREATING";
/// `ConnectorState` once the connector's workers run.
pub const CONNECTOR_RUNNING: &str = "RUNNING";
/// `ConnectorState` between `DeleteConnector` and the connector's removal.
pub const CONNECTOR_DELETING: &str = "DELETING";

/// `ConnectorState` while a connector that was asked to restart is bringing its
/// workers back; one of [`CONNECTOR_STATE_VALUES`]. The clock, not the next
/// call, is what returns it to [`CONNECTOR_RUNNING`].
pub const CONNECTOR_RESTARTING: &str = "RESTARTING";

/// `ConnectorState` while a connector takes on the capacity or the
/// configuration an update named; one of [`CONNECTOR_STATE_VALUES`]. The clock,
/// not the next call, is what returns it to [`CONNECTOR_RUNNING`].
pub const CONNECTOR_UPDATING: &str = "UPDATING";

/// `ConnectorOperationType` of the operation a restart starts; one of
/// [`CONNECTOR_OPERATION_TYPE_VALUES`].
pub const CONNECTOR_OPERATION_RESTART: &str = "RESTART_CONNECTOR";

/// `ConnectorOperationType` of the operation an update to a connector's
/// capacity starts — capacity is the connector's worker setting, which is what
/// `WorkerSetting` carries; one of [`CONNECTOR_OPERATION_TYPE_VALUES`].
pub const CONNECTOR_OPERATION_UPDATE_WORKER_SETTING: &str = "UPDATE_WORKER_SETTING";

/// `ConnectorOperationType` of the operation an update to a connector's
/// configuration starts; one of [`CONNECTOR_OPERATION_TYPE_VALUES`].
pub const CONNECTOR_OPERATION_UPDATE_CONFIGURATION: &str = "UPDATE_CONNECTOR_CONFIGURATION";

/// `ConnectorOperationState` every connector operation starts in — the work is
/// recorded and has not begun; one of [`CONNECTOR_OPERATION_STATE_VALUES`].
pub const CONNECTOR_OPERATION_PENDING: &str = "PENDING";

/// `ConnectorOperationState` while a restart is running.
pub const CONNECTOR_OPERATION_RESTART_IN_PROGRESS: &str = "RESTART_IN_PROGRESS";

/// `ConnectorOperationState` a restart that finished ends in.
pub const CONNECTOR_OPERATION_RESTART_COMPLETE: &str = "RESTART_COMPLETE";

/// The states a restart passes through after [`CONNECTOR_OPERATION_PENDING`],
/// in order: the model's own two restart states, the last of which is terminal.
/// Each step is owed [`TRANSITION_SPAN`] after the one before it.
pub const RESTART_PROGRESSION: &[&str] = &[
    CONNECTOR_OPERATION_RESTART_IN_PROGRESS,
    CONNECTOR_OPERATION_RESTART_COMPLETE,
];

/// `ConnectorOperationState` while an update is running.
pub const CONNECTOR_OPERATION_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// `ConnectorOperationState` an update that finished ends in.
pub const CONNECTOR_OPERATION_UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// `ConnectorOperationStepState` of a step that has not begun.
pub const STEP_PENDING: &str = "PENDING";
/// `ConnectorOperationStepState` of the step an operation is working.
pub const STEP_IN_PROGRESS: &str = "IN_PROGRESS";
/// `ConnectorOperationStepState` of a step that has finished.
pub const STEP_COMPLETED: &str = "COMPLETED";

/// `ConnectorOperationStepType` of the step every update opens with.
const STEP_INITIALIZE_UPDATE: &str = "INITIALIZE_UPDATE";
/// `ConnectorOperationStepType` of the step that checks what an update asks
/// for before anything is changed.
const STEP_VALIDATE_UPDATE: &str = "VALIDATE_UPDATE";
/// `ConnectorOperationStepType` of the step every update closes with.
const STEP_FINALIZE_UPDATE: &str = "FINALIZE_UPDATE";

/// The steps an update of `operation_type` walks, in order.
///
/// `ConnectorOperationStepType` lists exactly five values
/// (`INITIALIZE_UPDATE`, `VALIDATE_UPDATE`, `UPDATE_WORKER_SETTING`,
/// `UPDATE_CONNECTOR_CONFIGURATION`, `FINALIZE_UPDATE`): the three an update of
/// either kind shares, around the one that names the change itself — and that
/// one is spelled the same as the operation's own type, which is why it is
/// taken from it rather than listed twice.
fn update_step_types(operation_type: &str) -> Vec<String> {
    [
        STEP_INITIALIZE_UPDATE,
        STEP_VALIDATE_UPDATE,
        operation_type,
        STEP_FINALIZE_UPDATE,
    ]
    .iter()
    .map(|step| (*step).to_string())
    .collect()
}

/// The version a freshly created connector is at. `currentVersion` is an opaque
/// string to a client, which only ever echoes it back on an update or a delete.
pub const FIRST_CONNECTOR_VERSION: &str = "1";

/// The revision a freshly created custom plugin is at. `CustomPlugin$revision`
/// is modelled `__longMin1` (range 1..), so revisions start at one.
pub const FIRST_PLUGIN_REVISION: i64 = 1;

/// The `networkType` a connector that named none takes: "Defaults to IPV4",
/// per the `networkType` member's own documentation.
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// The MCU counts one connector worker may be allocated.
///
/// `AutoScaling$mcuCount` and `ProvisionedCapacity$mcuCount` (and their `*Update`
/// twins) all document "The valid values are 1,2,4,8", while the model can only
/// bound the member as `__integerMin1Max8` — a range that also admits 3, 5, 6 and
/// 7. The set itself is therefore checked by the service, not by the framework.
pub const MCU_COUNT_VALUES: [i32; 4] = [1, 2, 4, 8];

/// The counts one capacity form asks for, read out of whichever shape carried
/// them (`AutoScaling` / `ProvisionedCapacity` on a create, their `*Update` twins
/// on an update), so the relations the service documents between them are decided
/// in ONE place for every form and every operation that takes a capacity.
#[derive(Debug, Clone, Copy, Default)]
pub struct CapacityCounts {
    /// `mcuCount`, which must be one of [`MCU_COUNT_VALUES`].
    pub mcu_count: i32,
    /// `(minWorkerCount, maxWorkerCount)` for an auto-scaled form, which the
    /// service bounds against each other; `None` for a fixed form, whose single
    /// `workerCount` is bounded by nothing else in the request.
    pub worker_count_range: Option<(i32, i32)>,
    /// `maxAutoscalingTaskCount`, or zero when the request named none (the
    /// member's modelled default, and the value the service reports for a
    /// capacity that carries no task ceiling).
    pub max_autoscaling_task_count: i32,
}

impl CapacityCounts {
    /// The counts the named form of `capacity` asks for, or `None` when it names
    /// neither form — which [`capacity_refusal`], the one check every operation
    /// that takes a capacity runs, has already refused on the `capacity` member
    /// itself ([`ONE_CAPACITY_FORM`]) before any counts are read.
    ///
    /// An update's `CapacityUpdate` mirrors `Capacity` member for member and is
    /// read through [`capacity_from_update`], so both operations' forms reach
    /// this one reader.
    pub fn of(capacity: &Capacity) -> Option<Self> {
        if let Some(scaling) = capacity.auto_scaling.as_ref() {
            return Some(Self {
                mcu_count: scaling.mcu_count,
                worker_count_range: Some((scaling.min_worker_count, scaling.max_worker_count)),
                max_autoscaling_task_count: scaling.max_autoscaling_task_count,
            });
        }
        let fixed = capacity.provisioned_capacity.as_ref()?;
        Some(Self {
            mcu_count: fixed.mcu_count,
            // A fixed form has a single `workerCount`, which no other member of
            // the request bounds, and no task ceiling at all.
            worker_count_range: None,
            max_autoscaling_task_count: 0,
        })
    }

    /// The message a refusal of these counts carries — in the one
    /// member-naming form every kafkaconnect rejection uses — or `None` when
    /// every documented relation holds.
    ///
    /// The relations are the members' own documentation: `mcuCount`'s valid
    /// value set ([`MCU_COUNT_VALUES`]), the worker window being a window at all
    /// (a minimum above the maximum is no window), and
    /// `maxAutoscalingTaskCount` "Must be at least equal to maxWorkerCount" —
    /// checked only when the request named one, since the member is optional and
    /// its absence reads as the modelled default zero.
    pub fn refusal(&self) -> Option<String> {
        if !MCU_COUNT_VALUES.contains(&self.mcu_count) {
            return Some(invalid_parameter_because("mcuCount", &mcu_count_reason()));
        }
        // A fixed form has no worker window, so there is nothing further to
        // check once its MCU count is one the service allocates: `?` here means
        // "no refusal", not an error.
        let (minimum, maximum) = self.worker_count_range?;
        if minimum > maximum {
            return Some(invalid_parameter_because(
                "minWorkerCount",
                "It must not exceed maxWorkerCount.",
            ));
        }
        if self.max_autoscaling_task_count != 0 && self.max_autoscaling_task_count < maximum {
            return Some(invalid_parameter_because(
                "maxAutoscalingTaskCount",
                "It must be at least equal to maxWorkerCount.",
            ));
        }
        None
    }
}

/// The message a refusal of `capacity` carries, or `None` when every relation
/// its members document holds.
///
/// The ONE capacity check every operation that takes a capacity runs: a create
/// on the `Capacity` its request carried, an update on the `Capacity` its
/// `CapacityUpdate` converts to ([`capacity_from_update`]), so neither can
/// accept a capacity the other refuses.
///
/// It decides the whole rule, the form as well as the counts: a connector is
/// "whether it is auto scaled or provisioned", so a capacity names exactly one
/// of the two optional forms the model gives it ([`ONE_CAPACITY_FORM`], blamed
/// on the `capacity` member that carried them), and only then are the counts
/// inside the named form read ([`CapacityCounts::refusal`]).
pub fn capacity_refusal(capacity: &Capacity) -> Option<String> {
    if capacity.auto_scaling.is_some() == capacity.provisioned_capacity.is_some() {
        return Some(invalid_parameter_because("capacity", ONE_CAPACITY_FORM));
    }
    CapacityCounts::of(capacity)?.refusal()
}

/// The reason an `mcuCount` outside [`MCU_COUNT_VALUES`] is refused with,
/// rendered from that set so the sentence and the check can never disagree.
pub fn mcu_count_reason() -> String {
    let values: Vec<String> = MCU_COUNT_VALUES
        .iter()
        .map(|value| value.to_string())
        .collect();
    format!("The valid values are {}.", values.join(","))
}

/// The largest page either list operation returns, and the page it returns when
/// the caller names no `maxResults`: the `MaxResults` shape's own maximum
/// (`range 1..100`).
pub const MAX_PAGE_SIZE: i32 = 100;
/// The smallest page `maxResults` may ask for, from the same `range`.
pub const MIN_PAGE_SIZE: i32 = 1;

/// How many custom plugins, and how many connectors, this emulator holds before
/// it refuses the next create with the modelled `TooManyRequestsException`
/// (HTTP 429, "Limit exceeded. Resource limit reached.").
///
/// The real service meters both resources against a service quota and answers
/// that same error once it is reached, so a cap is the faithful behaviour as
/// well as the one that keeps a long-lived emulator's state from growing without
/// bound on repeated calls. The value is the quota AWS ships for MSK Connect
/// connectors and custom plugins.
///
/// The cap, like the name uniqueness both creates check, is counted over the
/// resources of the CALL'S OWN region: an AWS service quota is a per-Region
/// quota, and a resource created under one signing region is not one a call
/// signed for another region sees, so each region meters its own.
pub const RESOURCE_QUOTA: usize = 100;

/// Absolute ceiling on the resources of one kind this store holds across every
/// region a caller signs for.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the signing region
/// comes from the request, so the per-region quota alone would still let a
/// caller grow this store without bound by naming region after region. Sized at
/// the per-region quota times more regions than AWS publishes, so it can only
/// ever be reached by a caller trying to exhaust the emulator, never by a
/// realistic client script.
pub const RESOURCE_CEILING: usize = RESOURCE_QUOTA * 64;

/// The most tags one resource's stored tag set may hold.
///
/// The `Tags` map is modelled `length 0..200`, so 200 entries is the largest
/// tag set the real service accepts for a resource, and a request that would
/// leave more than that is refused. The model bounds ONE request's map, which
/// the framework enforces before a handler runs; this bounds the set a
/// REPEATED `TagResource` accumulates, which is the one caller-driven
/// dimension of this store that would otherwise grow without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The tag-key prefix AWS reserves, across every service, for the tags it sets
/// itself: a caller may not create, change or remove a key that starts with it.
///
/// `TagKey` is modelled `len 1..128` with no `@pattern`, so the boundary
/// validator cannot refuse such a key and this service has to.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The fewest characters a tag key may carry: `TagKey` is modelled
/// `length 1..128` (`smithy_query(query="shape", shape="Tags,TagKey,TagValue")`
/// reports the same bound on the `Tags` map's key), so a key of nothing names no
/// tag.
pub const MIN_TAG_KEY_LENGTH: usize = 1;

/// The most characters a tag key may carry, from that same `TagKey` bound.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The characters a tag key or value may carry besides letters, numbers and
/// spaces, as AWS publishes the tag character set (letters, numbers, spaces,
/// and `+ - = . _ : / @`). Letters and numbers are read as the Unicode
/// categories, since the set is "representable in UTF-8" rather than ASCII.
pub const TAG_PUNCTUATION: &str = "+-=._:/@";

/// Which AWS tag rule a key or value in a request breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagRuleBreach {
    /// A key claims [`RESERVED_TAG_KEY_PREFIX`].
    ReservedKeyPrefix,
    /// A key or value carries a character outside the tag character set.
    OutsideCharacterSet,
}

impl TagRuleBreach {
    /// The reason a refusal of this breach carries — a full sentence, for the
    /// one member-naming form every kafkaconnect rejection uses
    /// ([`invalid_parameter_because`]).
    ///
    /// It names the RULE and interpolates nothing from the request: the rule is
    /// the same whatever key or value broke it, and a map's keys arrive in no
    /// particular order, so quoting one of them would make the message depend
    /// on iteration order.
    pub fn reason(self) -> String {
        match self {
            Self::ReservedKeyPrefix => format!(
                "A tag key must not begin with the reserved prefix {RESERVED_TAG_KEY_PREFIX}."
            ),
            Self::OutsideCharacterSet => format!(
                "A tag key or value may contain only letters, numbers, spaces and the characters {}.",
                TAG_PUNCTUATION
                    .chars()
                    .map(String::from)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }
    }
}

/// Whether `key` claims [`RESERVED_TAG_KEY_PREFIX`]. Compared
/// case-INSENSITIVELY, because AWS reserves the namespace rather than one
/// spelling of it.
///
/// The comparison is over BYTES: a tag key is any UTF-8 text the model's length
/// bound allows, and slicing such a string at a byte offset would panic when
/// the offset falls inside a multi-byte character. Taking bytes answers `false`
/// for those keys instead, which is correct — a key whose leading bytes are not
/// the prefix does not claim the namespace.
fn claims_reserved_tag_prefix(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Whether every character of `text` is one a tag key or value may carry:
/// a Unicode letter or number, a space, or one of [`TAG_PUNCTUATION`].
fn is_tag_text(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_alphanumeric() || c == ' ' || TAG_PUNCTUATION.contains(c))
}

/// The rule the entries break, or `None` when every one of them is a tag this
/// service may hold. `value` is `None` for an entry that is a bare key (a
/// removal names keys only).
///
/// A reserved key outranks a character breach, so a request that holds both is
/// answered the same way whatever order its map iterates in.
fn tag_breach<'a>(
    entries: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Option<TagRuleBreach> {
    let mut found: Option<TagRuleBreach> = None;
    for (key, value) in entries {
        if claims_reserved_tag_prefix(key) {
            return Some(TagRuleBreach::ReservedKeyPrefix);
        }
        if found.is_none()
            && (!is_tag_text(key) || value.is_some_and(|value| !is_tag_text(value)))
        {
            found = Some(TagRuleBreach::OutsideCharacterSet);
        }
    }
    found
}

/// The rule the tags of a request break, or `None` when the whole map may be
/// stored. The check every path that writes tags runs — `TagResource` through
/// [`ComAmazonawsKafkaconnectKafkaconnectState::attach_tags`], and each create over the `tags` its
/// request carried — so no operation can store a tag another would refuse.
pub fn tag_map_breach(tags: &HashMap<String, String>) -> Option<TagRuleBreach> {
    tag_breach(
        tags.iter()
            .map(|(key, value)| (key.as_str(), Some(value.as_str()))),
    )
}

/// The rule the tag keys of a request break, or `None` when every key may be
/// acted on. The caller-supplied keys of a removal are held to the same rules:
/// a key AWS reserves is not a caller's to strip.
pub fn tag_key_breach(keys: &[String]) -> Option<TagRuleBreach> {
    tag_breach(keys.iter().map(|key| (key.as_str(), None)))
}

/// The member every operation of this service binds a `Tags` map to — the
/// `tags` of `TagResource` and of each create — which is the path a violation
/// of one of its KEYS is reported at: a map key is a value the caller chose
/// rather than a member of its own, so the clause names the member that carried
/// the map and echoes the key.
const TAG_MAP_MEMBER: &str = "tags";

/// `TagKey`'s modelled `@length`, in the framework's own form, so the keys of a
/// map are judged by the same trait every other length-bearing member of this
/// service is judged by.
static TAG_KEY_LENGTH_TRAITS: ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    ::std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(Box::new(::aws_smithy_schema_server::LengthTrait::new(
            Some(MIN_TAG_KEY_LENGTH as i64),
            Some(MAX_TAG_KEY_LENGTH as i64),
        )));
        traits
    });

/// The schema a tag key is walked against: named after the member the map was
/// bound to, so the clause reports the position a boundary refusal of that
/// member would, and carrying `TagKey`'s bound.
static M_TAG_KEY: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
    ::aws_smithy_schema::ShapeId::from_static(
        "com.amazonaws.kafkaconnect#CheckedTagKeys$tags",
        "com.amazonaws.kafkaconnect",
        TAG_MAP_MEMBER,
    ),
    ::aws_smithy_schema::ShapeType::String,
    TAG_MAP_MEMBER,
    0,
)
.with_traits(&TAG_KEY_LENGTH_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is
/// empty on purpose: the walk visits what [`CheckedTagKeys`] WRITES, and a root
/// struct schema contributes no path segment of its own. The shape id is one
/// the model does not declare, so it cannot collide with a modelled shape.
static CHECKED_TAG_KEYS_SCHEMA: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.kafkaconnect#CheckedTagKeys",
            "com.amazonaws.kafkaconnect",
            "CheckedTagKeys",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );

/// The keys of one request's tag map, written for the validating walk the way a
/// codec writes a string member. Every key goes into ONE walk, so how many
/// clauses a refusal may state is the framework's own bound rather than a
/// second one invented here.
struct CheckedTagKeys<'a>(Vec<&'a str>);

impl ::aws_smithy_schema::serde::SerializableStruct for CheckedTagKeys<'_> {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        for key in &self.0 {
            s.write_string(&M_TAG_KEY, key)?;
        }
        Ok(())
    }
}

/// The message a request whose tag map carries a key outside `TagKey`'s
/// modelled `@length` is refused with, or `None` when every key is inside it.
///
/// WHY A HANDLER APPLIES THIS. The framework's boundary walk enforces the
/// `@length` of a map's VALUE and of a list's elements — which is what refuses
/// an over-long `TagValue` and an over-long `UntagResource` key already — but
/// not of a map's KEY: the codec writes an entry's key against the map member's
/// own schema before its value, and the walk captures it as that value's path
/// segment instead of checking it
/// (`vendor/smithy-schema-server/src/constraints.rs`, `write_leaf`). So the
/// handler that accepts a `Tags` map is the only place a rule on its keys can be
/// applied, and every operation that accepts one applies it here, so none of
/// them can store a key another would refuse.
///
/// NOTHING about constraint checking or its wording is reimplemented: the bound
/// is re-declared in the framework's own form, the check is
/// [`::aws_smithy_schema_server::validate`] — the same walk the boundary runs,
/// so the length is counted in characters and an over-long key is elided in the
/// clause exactly as it would be there — and the sentence is
/// [`incorrect_input_message`], the one this service words a modelled-constraint
/// refusal with. A refusal made here therefore reads exactly like one made at
/// the boundary.
///
/// The keys are walked in order of the KEY, because a `Tags` map reaches a
/// handler unordered: that is the one order the entries themselves fix, so one
/// request has exactly one refusal text however its map was built.
pub fn tag_key_length_refusal<'k>(keys: impl IntoIterator<Item = &'k str>) -> Option<String> {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    let violations = ::aws_smithy_schema_server::validate(
        &CheckedTagKeys(keys),
        &CHECKED_TAG_KEYS_SCHEMA,
    );
    incorrect_input_message(&violations)
}

/// The message a request that broke a MODELLED constraint is refused with, or
/// `None` when it broke none.
///
/// The ONE place this service words that class of refusal: the clauses are the
/// framework's own ([`::aws_smithy_schema_server::Violation::message`]), and the
/// sentence around them is the one `BadRequestException` documents ("HTTP Status
/// Code 400: Bad request due to incorrect input. Correct your request and then
/// retry it."). `crate::service` renders a refusal the boundary made through
/// here, and [`tag_key_length_refusal`] the one rule the boundary cannot reach,
/// so a caller cannot tell the two apart.
pub fn incorrect_input_message(
    violations: &[::aws_smithy_schema_server::Violation],
) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let clauses: Vec<String> = violations
        .iter()
        .map(::aws_smithy_schema_server::Violation::message)
        .collect();
    Some(format!(
        "Bad request due to incorrect input: {}. Correct your request and then retry it.",
        clauses.join("; ")
    ))
}

/// Why a tag write was refused, with nothing written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagWriteRefusal {
    /// This emulator holds no resource of that ARN.
    NoSuchResource,
    /// A key or value in the request breaks an AWS tag rule.
    IllegalTag(TagRuleBreach),
    /// The write would leave the resource holding more than
    /// [`MAX_TAGS_PER_RESOURCE`] tags; `would_hold` is how many.
    CapExceeded { would_hold: usize },
}

/// What the clock still owes a resource, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingChange {
    /// Which change is owed.
    pub kind: PendingKind,
    /// The instant it becomes due. Stored, never slept on.
    pub due: Instant,
}

impl PendingChange {
    /// The change `kind`, due [`TRANSITION_SPAN`] from now.
    pub fn in_a_moment(kind: PendingKind) -> Self {
        Self {
            kind,
            due: Instant::now() + TRANSITION_SPAN,
        }
    }

    /// The change `kind`, due at the instant `due`.
    ///
    /// For a change the clock owes at the instant some OTHER work it is tied to
    /// finishes rather than a span after this call — a connector leaving the
    /// state a call against it put it in, say, which is owed exactly when the
    /// connector operation reporting that call's work ends.
    pub fn at(kind: PendingKind, due: Instant) -> Self {
        Self { kind, due }
    }

    /// Whether this change is owed at `now`.
    pub fn is_due(&self, now: Instant) -> bool {
        self.due <= now
    }
}

/// The changes the clock owes a kafkaconnect resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    /// Leave the transient creating (or restarting) state for the resource's
    /// running state.
    Settle,
    /// Finish a delete: drop the resource.
    Remove,
    /// Move a connector operation to the next state of its own progression
    /// ([`ConnectorOperationRecord::remaining_states`]).
    Advance,
}

/// One stored custom plugin: the whole `CreateCustomPlugin` request plus the
/// identity and lifecycle the service assigned it.
#[derive(Debug, Clone)]
pub struct CustomPluginRecord {
    pub arn: String,
    /// The region this plugin lives in: the region the request that created it
    /// was signed for, and the region segment of its ARN. A plugin name is
    /// unique per account and region, and a regional endpoint serves only its
    /// own region's plugins.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    pub content_type: String,
    pub location: CustomPluginLocation,
    /// What the service took on about the plugin's artifact when it read the
    /// file at [`Self::location`] — the facts the revision reports in
    /// `fileDescription`. `None` only when nothing could be learned about the
    /// object at create time (there was no object store to ask).
    pub file: Option<CustomPluginFile>,
    pub state: String,
    pub revision: i64,
    pub creation_time: SystemTime,
    pub tags: HashMap<String, String>,
    pub pending: Option<PendingChange>,
}

/// The artifact a custom plugin revision was created from, as the service took
/// it on: both facts are computed from the file's own BYTES when the plugin is
/// created, and are then fixed for that revision — a revision describes the
/// bytes it was built from, so a later write to the same key cannot change what
/// an existing revision reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomPluginFile {
    /// The hex-encoded MD5 digest of the artifact's bytes
    /// (`CustomPluginFileDescription$fileMd5`: "The hex-encoded MD5 checksum of
    /// the custom plugin file. You can use it to validate the file.").
    pub md5: String,
    /// The artifact's size in bytes
    /// (`CustomPluginFileDescription$fileSize`: "The size in bytes of the
    /// custom plugin file.").
    pub size: i64,
}

impl CustomPluginRecord {
    /// This plugin's stored location, as the read operations report it.
    pub fn location_description(&self) -> CustomPluginLocationDescription {
        CustomPluginLocationDescription {
            s3_location: Some(S3LocationDescription {
                bucket_arn: Some(self.location.s3_location.bucket_arn.clone()),
                file_key: Some(self.location.s3_location.file_key.clone()),
                object_version: self.location.s3_location.object_version.clone(),
            }),
        }
    }

    /// The summary of this plugin's latest (only) revision.
    ///
    /// `fileDescription` reports the artifact this revision was created from —
    /// the digest and the size the service computed when it took the file on
    /// ([`CustomPluginFile`]) — and is unset only for a plugin created when
    /// nothing could be learned about its file at all.
    pub fn latest_revision(&self) -> CustomPluginRevisionSummary {
        CustomPluginRevisionSummary {
            content_type: Some(self.content_type.clone()),
            creation_time: Some(date_time(self.creation_time)),
            description: self.description.clone(),
            file_description: self.file.as_ref().map(|file| CustomPluginFileDescription {
                file_md5: Some(file.md5.clone()),
                file_size: file.size,
            }),
            location: Some(self.location_description()),
            revision: self.revision,
        }
    }

    /// This plugin as `ListCustomPlugins` reports it.
    pub fn summary(&self) -> CustomPluginSummary {
        CustomPluginSummary {
            creation_time: Some(date_time(self.creation_time)),
            custom_plugin_arn: Some(self.arn.clone()),
            custom_plugin_state: Some(self.state.clone()),
            description: self.description.clone(),
            latest_revision: Some(self.latest_revision()),
            name: Some(self.name.clone()),
        }
    }
}

/// The settings one `UpdateConnector` call moves a connector between: its
/// capacity — the worker setting `WorkerSetting` carries — or its
/// configuration. The operation's own documentation says a request names
/// exactly one of the two, so one value of this enum is one update.
#[derive(Debug, Clone)]
pub enum ConnectorSettings {
    /// A connector's capacity (`UpdateConnector$capacity`).
    Capacity(Capacity),
    /// A connector's configuration (`UpdateConnector$connectorConfiguration`),
    /// the whole map rather than a patch: the member carries "the configuration
    /// for the connector", the same wording the create's own configuration
    /// member carries, and the model gives no member naming keys to drop.
    Configuration(HashMap<String, String>),
}

impl ConnectorSettings {
    /// The `ConnectorOperationType` an update to these settings runs under.
    pub fn operation_type(&self) -> &'static str {
        match self {
            Self::Capacity(_) => CONNECTOR_OPERATION_UPDATE_WORKER_SETTING,
            Self::Configuration(_) => CONNECTOR_OPERATION_UPDATE_CONFIGURATION,
        }
    }

    /// These settings as the `WorkerSetting` a report carries, or `None` when
    /// they are not a worker setting at all.
    pub fn worker_setting(&self) -> Option<WorkerSetting> {
        match self {
            Self::Capacity(capacity) => Some(WorkerSetting {
                capacity: Some(capacity_description(capacity)),
            }),
            Self::Configuration(_) => None,
        }
    }

    /// These settings as the connector configuration a report carries, or
    /// `None` when they are not a configuration at all.
    pub fn configuration(&self) -> Option<HashMap<String, String>> {
        match self {
            Self::Capacity(_) => None,
            Self::Configuration(configuration) => Some(configuration.clone()),
        }
    }
}

/// The capacity an update names, as the shape a connector stores.
///
/// `CapacityUpdate` mirrors `Capacity` member for member — the two scaling
/// policies are required there and optional here — so the conversion keeps
/// every value the request carried and invents none.
pub fn capacity_from_update(update: CapacityUpdate) -> Capacity {
    Capacity {
        auto_scaling: update.auto_scaling.map(|scaling| AutoScaling {
            max_worker_count: scaling.max_worker_count,
            mcu_count: scaling.mcu_count,
            min_worker_count: scaling.min_worker_count,
            scale_in_policy: Some(ScaleInPolicy {
                cpu_utilization_percentage: scaling.scale_in_policy.cpu_utilization_percentage,
            }),
            scale_out_policy: Some(ScaleOutPolicy {
                cpu_utilization_percentage: scaling.scale_out_policy.cpu_utilization_percentage,
            }),
            max_autoscaling_task_count: scaling.max_autoscaling_task_count,
        }),
        provisioned_capacity: update
            .provisioned_capacity
            .map(|fixed| ProvisionedCapacity {
                mcu_count: fixed.mcu_count,
                worker_count: fixed.worker_count,
            }),
    }
}

/// `capacity` as the read operations report it.
///
/// A free function because two records report a capacity: the connector its own
/// ([`ConnectorRecord::capacity_description`]) and a connector operation the
/// capacity on each side of the update it applies.
pub fn capacity_description(capacity: &Capacity) -> CapacityDescription {
    CapacityDescription {
        auto_scaling: capacity
            .auto_scaling
            .as_ref()
            .map(|scaling| AutoScalingDescription {
                max_worker_count: scaling.max_worker_count,
                mcu_count: scaling.mcu_count,
                min_worker_count: scaling.min_worker_count,
                scale_in_policy: scaling.scale_in_policy.as_ref().map(|policy| {
                    ScaleInPolicyDescription {
                        cpu_utilization_percentage: policy.cpu_utilization_percentage,
                    }
                }),
                scale_out_policy: scaling.scale_out_policy.as_ref().map(|policy| {
                    ScaleOutPolicyDescription {
                        cpu_utilization_percentage: policy.cpu_utilization_percentage,
                    }
                }),
                max_autoscaling_task_count: scaling.max_autoscaling_task_count,
            }),
        provisioned_capacity: capacity.provisioned_capacity.as_ref().map(|fixed| {
            ProvisionedCapacityDescription {
                mcu_count: fixed.mcu_count,
                worker_count: fixed.worker_count,
            }
        }),
    }
}

/// The version a connector moves to from `current` once an update lands.
///
/// The versions this service mints are counted from
/// [`FIRST_CONNECTOR_VERSION`], so the next one is the one after it. A stored
/// value outside that count (which the API cannot produce — a client only ever
/// echoes back the version it was handed) restarts the count rather than
/// panicking.
fn next_connector_version(current: &str) -> String {
    current
        .parse::<u64>()
        .map_or_else(
            |_| FIRST_CONNECTOR_VERSION.to_string(),
            |version| version.saturating_add(1).to_string(),
        )
}

/// One stored connector: the whole `CreateConnector` request plus the identity,
/// version and lifecycle the service assigned it.
#[derive(Debug, Clone)]
pub struct ConnectorRecord {
    pub arn: String,
    /// The region this connector lives in: the region the request that created
    /// it was signed for, and the region segment of its ARN. A connector name
    /// is unique per account and region, and a regional endpoint serves only
    /// its own region's connectors.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    pub configuration: HashMap<String, String>,
    pub capacity: Capacity,
    pub kafka_cluster: KafkaCluster,
    pub kafka_cluster_client_authentication: KafkaClusterClientAuthentication,
    pub kafka_cluster_encryption_in_transit: KafkaClusterEncryptionInTransit,
    pub kafka_connect_version: String,
    pub network_type: String,
    pub plugins: Vec<Plugin>,
    pub service_execution_role_arn: String,
    pub worker_configuration: Option<WorkerConfiguration>,
    pub log_delivery: Option<LogDelivery>,
    pub state: String,
    pub current_version: String,
    pub creation_time: SystemTime,
    pub tags: HashMap<String, String>,
    pub pending: Option<PendingChange>,
    /// The settings an update that is still running will leave this connector
    /// holding, kept here until that update's last step is done: a connector
    /// reports what it is RUNNING with, so the change lands with the move back
    /// to that state and not before.
    pub pending_update: Option<ConnectorSettings>,
}

impl ConnectorRecord {
    /// Take on the settings an update has made due, and move to the next
    /// version.
    ///
    /// `currentVersion` names the settings the connector is running with, so it
    /// moves exactly when those settings do — one update's worth at a time,
    /// because a connector that is not running takes no further update.
    fn apply_update(&mut self, settings: ConnectorSettings) {
        match settings {
            ConnectorSettings::Capacity(capacity) => self.capacity = capacity,
            ConnectorSettings::Configuration(configuration) => self.configuration = configuration,
        }
        self.current_version = next_connector_version(&self.current_version);
    }

    /// The message a call that only a running connector can serve is refused
    /// with, or `None` when this connector is running and the call may proceed.
    ///
    /// The ONE state gate every operation that starts work on a connector runs
    /// (a restart and an update), so neither can differ from the other on which
    /// `ConnectorState` admits such a call, on the member the refusal blames, or
    /// on how it is worded: a connector still being created, already restarting
    /// or updating, or being deleted has no running workers for the call to act
    /// on. Only the modelled error VARIANT stays with the handler, since the
    /// error enum is per-operation — the same shape as
    /// [`capacity_refusal`] and
    /// [`ComAmazonawsKafkaconnectKafkaconnectState::start_connector_operation`], the
    /// other two rules these handlers share.
    pub fn not_running_refusal(&self) -> Option<String> {
        (self.state != CONNECTOR_RUNNING)
            .then(|| invalid_parameter_because("connectorArn", &connector_is_not_running()))
    }

    /// Whether this connector names the resource `arn` names among the
    /// resources it runs on: one of the custom plugins of its `plugins`, or the
    /// worker configuration it was created with.
    ///
    /// Both references are stored verbatim and both are echoed back by the read
    /// operations ([`Self::plugin_descriptions`] and
    /// [`Self::worker_configuration_description`]), so they are the same kind of
    /// dependency and are answered here by the one rule.
    pub fn names_dependency(&self, arn: &str) -> bool {
        self.plugins
            .iter()
            .any(|plugin| plugin.custom_plugin.custom_plugin_arn == arn)
            || self
                .worker_configuration
                .as_ref()
                .is_some_and(|worker| worker.worker_configuration_arn == arn)
    }

    /// This connector's stored capacity, as the read operations report it.
    pub fn capacity_description(&self) -> CapacityDescription {
        capacity_description(&self.capacity)
    }

    /// This connector's stored Kafka cluster, as the read operations report it.
    pub fn kafka_cluster_description(&self) -> KafkaClusterDescription {
        let cluster = &self.kafka_cluster.apache_kafka_cluster;
        KafkaClusterDescription {
            apache_kafka_cluster: Some(ApacheKafkaClusterDescription {
                bootstrap_servers: Some(cluster.bootstrap_servers.clone()),
                vpc: Some(VpcDescription {
                    security_groups: cluster.vpc.security_groups.clone(),
                    subnets: Some(cluster.vpc.subnets.clone()),
                }),
            }),
        }
    }

    /// This connector's stored client authentication, as reported.
    pub fn client_authentication_description(&self) -> KafkaClusterClientAuthenticationDescription {
        KafkaClusterClientAuthenticationDescription {
            authentication_type: Some(
                self.kafka_cluster_client_authentication
                    .authentication_type
                    .clone(),
            ),
        }
    }

    /// This connector's stored encryption in transit, as reported.
    pub fn encryption_in_transit_description(&self) -> KafkaClusterEncryptionInTransitDescription {
        KafkaClusterEncryptionInTransitDescription {
            encryption_type: Some(
                self.kafka_cluster_encryption_in_transit
                    .encryption_type
                    .clone(),
            ),
        }
    }

    /// This connector's stored log delivery, as reported; `None` when the
    /// request named none.
    pub fn log_delivery_description(&self) -> Option<LogDeliveryDescription> {
        let delivery = self.log_delivery.as_ref()?;
        let worker = &delivery.worker_log_delivery;
        Some(LogDeliveryDescription {
            worker_log_delivery: Some(WorkerLogDeliveryDescription {
                cloud_watch_logs: worker.cloud_watch_logs.as_ref().map(|logs| {
                    CloudWatchLogsLogDeliveryDescription {
                        enabled: logs.enabled,
                        log_group: logs.log_group.clone(),
                    }
                }),
                firehose: worker
                    .firehose
                    .as_ref()
                    .map(|firehose| FirehoseLogDeliveryDescription {
                        delivery_stream: firehose.delivery_stream.clone(),
                        enabled: firehose.enabled,
                    }),
                s3: worker.s3.as_ref().map(|s3| S3LogDeliveryDescription {
                    bucket: s3.bucket.clone(),
                    enabled: s3.enabled,
                    prefix: s3.prefix.clone(),
                }),
            }),
        })
    }

    /// This connector's stored worker configuration, as reported; `None` when
    /// the request named none.
    pub fn worker_configuration_description(&self) -> Option<WorkerConfigurationDescription> {
        self.worker_configuration
            .as_ref()
            .map(|worker| WorkerConfigurationDescription {
                revision: worker.revision,
                worker_configuration_arn: Some(worker.worker_configuration_arn.clone()),
            })
    }

    /// The plugins this connector was created with, as reported.
    pub fn plugin_descriptions(&self) -> Vec<PluginDescription> {
        self.plugins
            .iter()
            .map(|plugin| PluginDescription {
                custom_plugin: Some(CustomPluginDescription {
                    custom_plugin_arn: Some(plugin.custom_plugin.custom_plugin_arn.clone()),
                    revision: plugin.custom_plugin.revision,
                }),
            })
            .collect()
    }

    /// This connector as `ListConnectors` reports it.
    pub fn summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            capacity: Some(self.capacity_description()),
            connector_arn: Some(self.arn.clone()),
            connector_description: self.description.clone(),
            connector_name: Some(self.name.clone()),
            connector_state: Some(self.state.clone()),
            creation_time: Some(date_time(self.creation_time)),
            current_version: Some(self.current_version.clone()),
            kafka_cluster: Some(self.kafka_cluster_description()),
            kafka_cluster_client_authentication: Some(self.client_authentication_description()),
            kafka_cluster_encryption_in_transit: Some(self.encryption_in_transit_description()),
            kafka_connect_version: Some(self.kafka_connect_version.clone()),
            log_delivery: self.log_delivery_description(),
            network_type: Some(self.network_type.clone()),
            plugins: Some(self.plugin_descriptions()),
            service_execution_role_arn: Some(self.service_execution_role_arn.clone()),
            worker_configuration: self.worker_configuration_description(),
        }
    }
}

/// One stored connector operation: the asynchronous work a call against a
/// connector started, with the progression the clock walks it along.
///
/// Nothing creates one directly: `RestartConnector` mints one per restart and
/// `UpdateConnector` one per update, and `ListConnectorOperations` and
/// `DescribeConnectorOperation` report it. An operation belongs to its
/// connector, so it is dropped with it.
#[derive(Debug, Clone)]
pub struct ConnectorOperationRecord {
    pub arn: String,
    /// The connector this operation runs against — the ARN
    /// `ListConnectorOperations` lists by.
    pub connector_arn: String,
    /// `ConnectorOperationType`: which kind of work this is.
    pub operation_type: String,
    /// `ConnectorOperationState`: where in its progression this operation is.
    pub state: String,
    /// The states this operation has still to pass through, in order; empty
    /// once it has reached its terminal state.
    pub remaining_states: VecDeque<String>,
    /// The `ConnectorOperationStepType`s this operation walks, in order, one
    /// per entry of its progression; empty for an operation type the model
    /// declares no step for.
    pub step_types: Vec<String>,
    /// The settings the connector held when this operation started, and the
    /// ones it leaves the connector holding — the origin and target a report
    /// carries. Both `None` for an operation that changes no settings.
    pub origin: Option<ConnectorSettings>,
    pub target: Option<ConnectorSettings>,
    pub creation_time: SystemTime,
    /// When this operation reaches its terminal state. Fixed when it starts (it
    /// is [`TRANSITION_SPAN`] per step of the progression), because the clock
    /// alone decides it; [`Self::end_time`] reports it only once it is reached,
    /// so a running operation carries no `endTime`.
    pub ends_at: SystemTime,
    /// Whether the restart that started this operation asked for only the
    /// connector's failed tasks (`RestartConnector$onlyFailedTasks`), or `None`
    /// for an operation no restart started. Stored because it is what the
    /// caller asked for; no modelled response member of this service reports
    /// it.
    pub only_failed_tasks: Option<bool>,
    pub pending: Option<PendingChange>,
}

impl ConnectorOperationRecord {
    /// The operation a `RestartConnector` call against `connector_arn` starts:
    /// `PENDING`, with the whole [`RESTART_PROGRESSION`] still ahead of it.
    pub fn restart(arn: String, connector_arn: String, only_failed_tasks: bool) -> Self {
        let states: VecDeque<String> = RESTART_PROGRESSION
            .iter()
            .map(|state| (*state).to_string())
            .collect();
        Self {
            // A restart changes neither the connector's worker setting nor its
            // configuration, so there is no pair of settings to report.
            only_failed_tasks: Some(only_failed_tasks),
            // Every value `ConnectorOperationStepType` lists names a step of an
            // UPDATE, so a restart has no modelled step to report and none is
            // invented for it.
            ..Self::starting(
                arn,
                connector_arn,
                CONNECTOR_OPERATION_RESTART,
                Vec::new(),
                states,
            )
        }
    }

    /// The operation an `UpdateConnector` call starts: `PENDING`, with one step
    /// of this kind of update ahead of it per entry of its progression.
    ///
    /// `origin` is what the connector holds now and `target` what the update
    /// named; they are settings of the same kind, which is what fixes the
    /// operation's type and its steps.
    pub fn update(
        arn: String,
        connector_arn: String,
        origin: ConnectorSettings,
        target: ConnectorSettings,
    ) -> Self {
        let operation_type = target.operation_type();
        let step_types = update_step_types(operation_type);
        // One modelled update state per step: finishing a step that is not the
        // last leaves the operation running, and finishing the last one
        // completes it. So the states already passed count the steps done.
        let last = step_types.len().saturating_sub(1);
        let states: VecDeque<String> = (0..step_types.len())
            .map(|step| {
                if step == last {
                    CONNECTOR_OPERATION_UPDATE_COMPLETE.to_string()
                } else {
                    CONNECTOR_OPERATION_UPDATE_IN_PROGRESS.to_string()
                }
            })
            .collect();
        Self {
            origin: Some(origin),
            target: Some(target),
            ..Self::starting(
                arn,
                connector_arn,
                operation_type,
                step_types,
                states,
            )
        }
    }

    /// The shape every operation starts in, whatever kind it is: `PENDING`,
    /// typed `operation_type`, with `step_types` and `states` still ahead of it
    /// and its first step owed one [`TRANSITION_SPAN`] from now. The settings a
    /// kind of operation moves between, and the restart flag, are the caller's
    /// to fill where its kind carries them.
    fn starting(
        arn: String,
        connector_arn: String,
        operation_type: &str,
        step_types: Vec<String>,
        states: VecDeque<String>,
    ) -> Self {
        let creation_time = SystemTime::now();
        let steps = u32::try_from(states.len()).unwrap_or(1);
        Self {
            arn,
            connector_arn,
            operation_type: operation_type.to_string(),
            state: CONNECTOR_OPERATION_PENDING.to_string(),
            remaining_states: states,
            step_types,
            origin: None,
            target: None,
            creation_time,
            ends_at: creation_time + TRANSITION_SPAN * steps,
            only_failed_tasks: None,
            pending: Some(PendingChange::in_a_moment(PendingKind::Advance)),
        }
    }

    /// How many of this operation's steps are done: one per entry of its
    /// progression the clock has already walked it past.
    fn steps_done(&self) -> usize {
        self.step_types.len().saturating_sub(self.remaining_states.len())
    }

    /// `operationSteps`: this operation's steps, each in the state this
    /// operation's own position puts it in — the steps before the one being
    /// worked have completed, that one is in progress, and the ones after it
    /// have not begun; nothing has begun at all while the operation itself is
    /// still `PENDING`, and every step has completed once it has ended.
    ///
    /// `None` for an operation the model declares no step for, which is every
    /// operation that is not an update.
    pub fn steps(&self) -> Option<Vec<ConnectorOperationStep>> {
        if self.step_types.is_empty() {
            return None;
        }
        // An operation that has not left `PENDING` is working no step yet.
        let working =
            (self.state != CONNECTOR_OPERATION_PENDING).then(|| self.steps_done());
        Some(
            self.step_types
                .iter()
                .enumerate()
                .map(|(index, step_type)| ConnectorOperationStep {
                    step_type: Some(step_type.clone()),
                    step_state: Some(
                        match working {
                            None => STEP_PENDING,
                            Some(working) => match index.cmp(&working) {
                                ::std::cmp::Ordering::Less => STEP_COMPLETED,
                                ::std::cmp::Ordering::Equal => STEP_IN_PROGRESS,
                                ::std::cmp::Ordering::Greater => STEP_PENDING,
                            },
                        }
                        .to_string(),
                    ),
                })
                .collect(),
        )
    }

    /// Whether this operation has reached its terminal state.
    pub fn has_ended(&self) -> bool {
        self.remaining_states.is_empty()
    }

    /// `endTime`: when this operation ended, or `None` while it is still
    /// running — the member is modelled optional precisely because an operation
    /// that has not finished has no end.
    pub fn end_time(&self) -> Option<::aws_smithy_types::DateTime> {
        self.has_ended().then(|| date_time(self.ends_at))
    }

    /// The instant the clock has this operation reach its terminal state — the
    /// instant the asynchronous work it reports finishes.
    ///
    /// Its progression is walked one state per [`TRANSITION_SPAN`] from the
    /// instant the next one is owed, so the last state still ahead of it is owed
    /// one span per state after that one. An operation that has already ended
    /// owes nothing further, so its work finished at or before now.
    fn ends_due(&self) -> Instant {
        match self.pending {
            Some(next) => {
                let left = u32::try_from(self.remaining_states.len()).unwrap_or(1);
                next.due + TRANSITION_SPAN * left.saturating_sub(1)
            }
            None => Instant::now(),
        }
    }

    /// The change the clock owes the connector this operation runs against: its
    /// return to [`CONNECTOR_RUNNING`], due at the instant this operation ends.
    ///
    /// The connector's state and this operation's state are two reports of ONE
    /// piece of asynchronous work, so they are held on ONE clock: the connector
    /// leaves the transient state the call put it in at exactly the instant the
    /// operation reporting that call's work reaches its terminal state, and not
    /// before. So while that operation is still running the connector reads as
    /// not running — and a further call against it, which only a running
    /// connector takes, is refused.
    pub fn connector_settle(&self) -> PendingChange {
        PendingChange::at(PendingKind::Settle, self.ends_due())
    }

    /// Apply every step of this operation's progression the clock already owes
    /// at `now`.
    ///
    /// Each step falls due [`TRANSITION_SPAN`] after the one before it, counted
    /// from the instant that one fell due rather than from this call, so where
    /// an operation is depends on the clock alone and not on when anybody
    /// happened to read it.
    fn advance_due(&mut self, now: Instant) {
        while let Some(change) = self.pending.filter(|change| change.is_due(now)) {
            let Some(next) = self.remaining_states.pop_front() else {
                self.pending = None;
                break;
            };
            self.state = next;
            self.pending = (!self.remaining_states.is_empty()).then(|| PendingChange {
                kind: change.kind,
                due: change.due + TRANSITION_SPAN,
            });
        }
    }

    /// The key this operation pages under.
    ///
    /// The model documents no order for `connectorOperations`, so the
    /// requirement on the key is the one [`page`] rests on: unique and stable.
    /// The creation instant with the ARN as tie-break is both, and it lists a
    /// connector's operations oldest first — the order they happened in.
    pub fn page_key(&self) -> String {
        let nanos = self
            .creation_time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // Zero-padded to the width of `u128::MAX`, so the keys of two
        // operations compare by instant rather than by digit count.
        format!("{nanos:039}/{}", self.arn)
    }

    /// This operation as `ListConnectorOperations` reports it.
    pub fn summary(&self) -> ConnectorOperationSummary {
        ConnectorOperationSummary {
            connector_operation_arn: Some(self.arn.clone()),
            connector_operation_type: Some(self.operation_type.clone()),
            connector_operation_state: Some(self.state.clone()),
            creation_time: Some(date_time(self.creation_time)),
            end_time: self.end_time(),
        }
    }
}

/// One stored worker configuration: the whole `CreateWorkerConfiguration`
/// request plus the identity and lifecycle the service assigned it.
#[derive(Debug, Clone)]
pub struct WorkerConfigurationRecord {
    pub arn: String,
    /// The region this configuration lives in: the region the request that
    /// created it was signed for, and the region segment of its ARN. A
    /// configuration name is unique per account and region, and a regional
    /// endpoint serves only its own region's configurations.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    /// The base64 `connect-distributed.properties` body the create request
    /// carried, stored verbatim: the create refuses a value it could not decode
    /// ([`base64_content_refusal`]), so what is stored is always base64, and the
    /// read operations hand back exactly the encoding the caller sent.
    pub properties_file_content: String,
    pub state: String,
    pub revision: i64,
    pub creation_time: SystemTime,
    pub tags: HashMap<String, String>,
    pub pending: Option<PendingChange>,
}

impl WorkerConfigurationRecord {
    /// The summary of this configuration's latest (only) revision, as the create
    /// and list operations report it. It carries no properties file: only the
    /// `WorkerConfigurationRevisionDescription` a describe answers with does.
    pub fn latest_revision(&self) -> WorkerConfigurationRevisionSummary {
        WorkerConfigurationRevisionSummary {
            creation_time: Some(date_time(self.creation_time)),
            description: self.description.clone(),
            revision: self.revision,
        }
    }

    /// This configuration as `ListWorkerConfigurations` reports it.
    ///
    /// Member set from
    /// `smithy_query(query="shape", shape="WorkerConfigurationSummary")`: it
    /// carries the identity, the description, the state, the creation instant and
    /// the latest revision summary, and NO properties file — only the
    /// `WorkerConfigurationRevisionDescription` a describe answers with does.
    pub fn summary(&self) -> WorkerConfigurationSummary {
        WorkerConfigurationSummary {
            creation_time: Some(date_time(self.creation_time)),
            description: self.description.clone(),
            latest_revision: Some(self.latest_revision()),
            name: Some(self.name.clone()),
            worker_configuration_arn: Some(self.arn.clone()),
            worker_configuration_state: Some(self.state.clone()),
        }
    }

    /// This configuration's latest revision as `DescribeWorkerConfiguration`
    /// reports it: the summary plus the stored properties file.
    pub fn revision_description(&self) -> WorkerConfigurationRevisionDescription {
        WorkerConfigurationRevisionDescription {
            creation_time: Some(date_time(self.creation_time)),
            description: self.description.clone(),
            properties_file_content: Some(self.properties_file_content.clone()),
            revision: self.revision,
        }
    }
}

/// A stored resource that belongs to one region and carries a name.
///
/// kafkaconnect is a REGIONAL service: its `aws.api#service` trait gives it the
/// `kafkaconnect` endpoint prefix with standard regional endpoints, so every
/// resource it holds lives in exactly one region, every ARN it mints carries
/// that region, and a call is answered from the resources of the region it was
/// signed for alone. Implemented by each stored record, so the region-scoped
/// lookups below are written ONCE rather than once per resource kind.
pub trait RegionalResource {
    /// The region this resource lives in.
    fn region(&self) -> &str;
    /// This resource's name, which is unique within its own region.
    fn name(&self) -> &str;
}

impl RegionalResource for CustomPluginRecord {
    fn region(&self) -> &str {
        &self.region
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl RegionalResource for ConnectorRecord {
    fn region(&self) -> &str {
        &self.region
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl RegionalResource for WorkerConfigurationRecord {
    fn region(&self) -> &str {
        &self.region
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A stored resource a delete of this service starts the removal of, and whose
/// removal the clock — not the next call — finishes.
///
/// The ONE removal-start rule every delete runs, so no delete can differ from
/// another on when the clock is owed the removal: a record that is not already
/// reporting its kind's [`Self::DELETING`] state moves there and the clock is
/// owed a [`PendingKind::Remove`] [`TRANSITION_SPAN`] out, while a record that
/// already is keeps both its state and its due instant, so a repeat of an
/// `@idempotent` delete answers the same state and never postpones the removal.
/// Only the state the kind reports while deleting and the record's own fields
/// stay per kind, and each handler still reads the record's `state` for its own
/// output member after the call — the same shape as [`capacity_refusal`],
/// [`ConnectorRecord::not_running_refusal`] and
/// [`ComAmazonawsKafkaconnectKafkaconnectState::start_connector_operation`], the
/// other rules these handlers share.
pub trait RemovableResource {
    /// The state this kind of resource reports while its removal is in flight.
    const DELETING: &'static str;

    /// This record's lifecycle state and the change the clock owes it, both
    /// mutably, so [`Self::start_removal`] writes the pair together.
    fn removal_lifecycle(&mut self) -> (&mut String, &mut Option<PendingChange>);

    /// Start this resource's removal, idempotently — the rule in the trait docs.
    fn start_removal(&mut self) {
        let (state, pending) = self.removal_lifecycle();
        if state.as_str() != Self::DELETING {
            *state = Self::DELETING.to_string();
            *pending = Some(PendingChange::in_a_moment(PendingKind::Remove));
        }
    }
}

impl RemovableResource for CustomPluginRecord {
    const DELETING: &'static str = CUSTOM_PLUGIN_DELETING;

    fn removal_lifecycle(&mut self) -> (&mut String, &mut Option<PendingChange>) {
        (&mut self.state, &mut self.pending)
    }
}

impl RemovableResource for ConnectorRecord {
    const DELETING: &'static str = CONNECTOR_DELETING;

    fn removal_lifecycle(&mut self) -> (&mut String, &mut Option<PendingChange>) {
        (&mut self.state, &mut self.pending)
    }
}

impl RemovableResource for WorkerConfigurationRecord {
    const DELETING: &'static str = WORKER_CONFIGURATION_DELETING;

    fn removal_lifecycle(&mut self) -> (&mut String, &mut Option<PendingChange>) {
        (&mut self.state, &mut self.pending)
    }
}

/// The record `arn` names, as an endpoint for `region` sees it.
///
/// A regional endpoint serves only its own region's resources, so a well-formed
/// ARN belonging to another region resolves to `None` exactly as an ARN this
/// store never minted does.
fn record_in<'store, T: RegionalResource>(
    records: &'store HashMap<String, T>,
    region: &str,
    arn: &str,
) -> Option<&'store T> {
    records.get(arn).filter(|record| record.region() == region)
}

/// The record `arn` names in `region`, mutably — the write counterpart of
/// [`record_in`], under the same regional rule, so a mutating call resolves a
/// resource exactly as a read does.
fn record_in_mut<'store, T: RegionalResource>(
    records: &'store mut HashMap<String, T>,
    region: &str,
    arn: &str,
) -> Option<&'store mut T> {
    records
        .get_mut(arn)
        .filter(|record| record.region() == region)
}

/// The record of `name` in `region`, if any.
///
/// A name identifies at most one resource of a kind PER REGION, which is what
/// makes a repeat of it within one region a conflict while the same name in
/// another region is a second, independent resource.
fn record_named<'store, T: RegionalResource>(
    records: &'store HashMap<String, T>,
    region: &str,
    name: &str,
) -> Option<&'store T> {
    records
        .values()
        .find(|record| record.region() == region && record.name() == name)
}

/// Every record of `region`, in no particular order.
fn records_in<'store, T: RegionalResource>(
    records: &'store HashMap<String, T>,
    region: &'store str,
) -> impl Iterator<Item = &'store T> + 'store {
    records
        .values()
        .filter(move |record| record.region() == region)
}

/// Whether one more resource of this kind may be stored for a call signed for
/// `region`: refused once that region holds [`RESOURCE_QUOTA`] of them — a
/// service quota is per Region — or once the store as a whole reaches
/// [`RESOURCE_CEILING`].
fn quota_reached<T: RegionalResource>(records: &HashMap<String, T>, region: &str) -> bool {
    records
        .values()
        .filter(|record| record.region() == region)
        .count()
        >= RESOURCE_QUOTA
        || records.len() >= RESOURCE_CEILING
}

/// Per-service state for com-amazonaws-kafkaconnect-kafkaconnect. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKafkaconnectKafkaconnectState {
    /// Stored custom plugins, keyed by `customPluginArn`.
    pub custom_plugins: HashMap<String, CustomPluginRecord>,
    /// Stored connectors, keyed by `connectorArn`.
    pub connectors: HashMap<String, ConnectorRecord>,
    /// Stored worker configurations, keyed by `workerConfigurationArn`.
    pub worker_configurations: HashMap<String, WorkerConfigurationRecord>,
    /// Stored connector operations, keyed by `connectorOperationArn`.
    pub connector_operations: HashMap<String, ConnectorOperationRecord>,
    /// How many resources this service has minted an id for. Feeds the ARN
    /// suffix, so two resources of one name (one created after the other was
    /// deleted) never share an ARN.
    pub minted: u64,
}

impl ComAmazonawsKafkaconnectKafkaconnectState {
    /// Apply every change the clock already owes: a creating resource becomes
    /// available, and a deleting one is dropped, once its due instant has
    /// passed.
    ///
    /// Called at the top of EVERY handler, read or write, so what a caller sees
    /// depends on the clock alone and not on which operation happened to run.
    /// This is the one write a reporting operation may make: each change was
    /// owed at the same instant whether or not anybody called.
    pub fn settle_due(&mut self) {
        let now = Instant::now();

        self.custom_plugins
            .retain(|_, plugin| !removal_due(plugin.pending, now));
        for plugin in self.custom_plugins.values_mut() {
            if settling_due(plugin.pending, now) {
                plugin.state = CUSTOM_PLUGIN_ACTIVE.to_string();
                plugin.pending = None;
            }
        }

        self.connectors
            .retain(|_, connector| !removal_due(connector.pending, now));
        for connector in self.connectors.values_mut() {
            if settling_due(connector.pending, now) {
                // The settings an update made due land with the move back to
                // the running state: a connector reports what it is running
                // with, so its capacity, its configuration and the version that
                // names them all move at the instant the update finishes.
                if let Some(settings) = connector.pending_update.take() {
                    connector.apply_update(settings);
                }
                connector.state = CONNECTOR_RUNNING.to_string();
                connector.pending = None;
            }
        }

        // A worker configuration has no creating state to leave (the model gives
        // `WorkerConfigurationState` only `ACTIVE` and `DELETING`), so the only
        // change the clock can owe one is the end of a delete.
        self.worker_configurations
            .retain(|_, worker| !removal_due(worker.pending, now));

        // A connector operation walks its own progression on the clock, from
        // `PENDING` to the state its type ends in.
        for operation in self.connector_operations.values_mut() {
            operation.advance_due(now);
        }
        // An operation belongs to the connector it ran against: once that
        // connector is gone (the delete the clock has just finished above),
        // there is no resource left for it to report an operation of.
        let connectors = &self.connectors;
        self.connector_operations
            .retain(|_, operation| connectors.contains_key(&operation.connector_arn));
    }

    /// The custom plugin `arn` names, as an endpoint for `region` sees it:
    /// `None` for a plugin of another region, exactly as for one this store
    /// does not hold.
    pub fn custom_plugin_in(&self, region: &str, arn: &str) -> Option<&CustomPluginRecord> {
        record_in(&self.custom_plugins, region, arn)
    }

    /// The custom plugin `arn` names in `region`, mutably — the write
    /// counterpart of [`Self::custom_plugin_in`], under the same regional rule.
    pub fn custom_plugin_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut CustomPluginRecord> {
        record_in_mut(&mut self.custom_plugins, region, arn)
    }

    /// The custom plugins of `region` — what `ListCustomPlugins` answered
    /// through that region's endpoint reports, and nothing of any other region.
    pub fn custom_plugins_in<'store>(
        &'store self,
        region: &'store str,
    ) -> impl Iterator<Item = &'store CustomPluginRecord> + 'store {
        records_in(&self.custom_plugins, region)
    }

    /// The connector `arn` names, as an endpoint for `region` sees it: `None`
    /// for a connector of another region, exactly as for one this store does
    /// not hold.
    pub fn connector_in(&self, region: &str, arn: &str) -> Option<&ConnectorRecord> {
        record_in(&self.connectors, region, arn)
    }

    /// The connector `arn` names in `region`, mutably — the write counterpart
    /// of [`Self::connector_in`], under the same regional rule.
    pub fn connector_in_mut(&mut self, region: &str, arn: &str) -> Option<&mut ConnectorRecord> {
        record_in_mut(&mut self.connectors, region, arn)
    }

    /// The connectors of `region` — what `ListConnectors` answered through that
    /// region's endpoint reports ("all the connectors in this account and
    /// Region"), and nothing of any other region.
    pub fn connectors_in<'store>(
        &'store self,
        region: &'store str,
    ) -> impl Iterator<Item = &'store ConnectorRecord> + 'store {
        records_in(&self.connectors, region)
    }

    /// The message a delete of the resource `arn` names is refused with while a
    /// connector this store still holds runs on it, or `None` when no connector
    /// names it and the delete may proceed.
    ///
    /// The ONE rule both deletes of a connector's dependencies run (a custom
    /// plugin and a worker configuration), so neither can differ from the other
    /// on when a dependency may be removed, on the member the refusal blames or
    /// on how it is worded. A stored connector names the plugins it runs on and
    /// the configuration it runs with by ARN, and a connector is only stored
    /// against references that resolve, in its own region, to a resource that is
    /// ACTIVE: removing a resource a connector still names would leave the store
    /// holding a connector the create path refuses to store, and the reads
    /// echoing an ARN that resolves to nothing. `member` is the request member
    /// that carried the ARN and `resource` the service's own name for the kind
    /// of resource it names; only the modelled error VARIANT stays with the
    /// handler, since the error enum is per-operation — the same shape as
    /// [`ConnectorRecord::not_running_refusal`] and [`capacity_refusal`], the
    /// other rules these handlers share.
    pub fn dependency_in_use_refusal(
        &self,
        region: &str,
        arn: &str,
        member: &str,
        resource: &str,
    ) -> Option<String> {
        self.connectors_in(region)
            .any(|connector| connector.names_dependency(arn))
            .then(|| invalid_parameter_because(member, &resource_is_in_use(resource)))
    }

    /// The worker configuration `arn` names, as an endpoint for `region` sees
    /// it: `None` for a configuration of another region, exactly as for one
    /// this store does not hold.
    pub fn worker_configuration_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&WorkerConfigurationRecord> {
        record_in(&self.worker_configurations, region, arn)
    }

    /// The worker configuration `arn` names in `region`, mutably — the write
    /// counterpart of [`Self::worker_configuration_in`], under the same
    /// regional rule.
    pub fn worker_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut WorkerConfigurationRecord> {
        record_in_mut(&mut self.worker_configurations, region, arn)
    }

    /// The worker configurations of `region` — what `ListWorkerConfigurations`
    /// answered through that region's endpoint reports ("all of the worker
    /// configurations in this account and Region"), and nothing of any other
    /// region.
    pub fn worker_configurations_in<'store>(
        &'store self,
        region: &'store str,
    ) -> impl Iterator<Item = &'store WorkerConfigurationRecord> + 'store {
        records_in(&self.worker_configurations, region)
    }

    /// The connector operation `arn` names, as an endpoint for `region` sees
    /// it.
    ///
    /// An operation belongs to the connector it ran against, so it lives in
    /// that connector's region: this region holds the operation exactly when it
    /// holds its connector.
    pub fn connector_operation_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&ConnectorOperationRecord> {
        let operation = self.connector_operations.get(arn)?;
        self.connector_in(region, &operation.connector_arn)?;
        Some(operation)
    }

    /// Whether `region` already holds as many custom plugins as it may
    /// ([`quota_reached`]), so the next `CreateCustomPlugin` signed for it must
    /// be refused.
    pub fn custom_plugin_quota_reached(&self, region: &str) -> bool {
        quota_reached(&self.custom_plugins, region)
    }

    /// Whether `region` already holds as many connectors as it may
    /// ([`quota_reached`]), so the next `CreateConnector` signed for it must be
    /// refused.
    pub fn connector_quota_reached(&self, region: &str) -> bool {
        quota_reached(&self.connectors, region)
    }

    /// The plugin of this name in `region`, if any — the per-region name rule
    /// [`record_named`] reads under.
    pub fn custom_plugin_named(&self, region: &str, name: &str) -> Option<&CustomPluginRecord> {
        record_named(&self.custom_plugins, region, name)
    }

    /// Whether `region` already holds as many worker configurations as it may
    /// ([`quota_reached`]), so the next `CreateWorkerConfiguration` signed for
    /// it must be refused.
    pub fn worker_configuration_quota_reached(&self, region: &str) -> bool {
        quota_reached(&self.worker_configurations, region)
    }

    /// The connector of this name in `region`, if any — the per-region name
    /// rule [`record_named`] reads under.
    pub fn connector_named(&self, region: &str, name: &str) -> Option<&ConnectorRecord> {
        record_named(&self.connectors, region, name)
    }

    /// Whether this store already holds as many operations against the
    /// connector `connector_arn` names as it may ([`RESOURCE_QUOTA`]), so the
    /// next call that would start one must be refused.
    ///
    /// AWS publishes no quota for connector operations, but a connector can be
    /// restarted and updated over and over and each such call records one more
    /// operation: this is the bound that keeps that caller-driven dimension of
    /// the store from growing without limit, answered — like the two resource
    /// quotas above — with the modelled `TooManyRequestsException`.
    pub fn connector_operation_quota_reached(&self, connector_arn: &str) -> bool {
        self.connector_operations
            .values()
            .filter(|operation| operation.connector_arn == connector_arn)
            .count()
            >= RESOURCE_QUOTA
    }

    /// Start one operation against the connector `connector_arn` names, for a
    /// call signed for `region`: the ARN the new operation is reported under, or
    /// — as an `Err` — the message the refusal of a connector that already holds
    /// as many operations as it may carries.
    ///
    /// The ONE way an operation against a connector is started, run by every
    /// handler that starts one (a restart and an update), so neither can meter
    /// the store, word that refusal, or mint an operation's ARN differently from
    /// the other. Only the modelled error VARIANT and the stored record stay with
    /// the handler, since the error enum and the operation's type are
    /// per-operation.
    ///
    /// The meter is read BEFORE anything is minted, so a refused call moves the
    /// mint counter no more than it stores a record.
    pub fn start_connector_operation(
        &mut self,
        connector_arn: &str,
        connector_name: &str,
        region: &str,
    ) -> Result<String, String> {
        // The operations stored against one connector are metered, so a
        // connector restarted or updated without end does not grow this store
        // without end. The refusal blames no member of the request, so it keeps
        // its own wording rather than the member-naming form.
        if self.connector_operation_quota_reached(connector_arn) {
            return Err(format!(
                "Limit exceeded. Resource limit reached: this connector already holds {RESOURCE_QUOTA} operations."
            ));
        }
        let id = self.mint_resource_id(CONNECTOR_OPERATION_KIND, connector_name);
        // The operation's ARN is derived from the connector's, so it names the
        // same partition, region and account the connector itself lives in. The
        // fallback covers only a connector ARN of a shape this service did not
        // mint, which keeps the call total without a panic.
        Ok(connector_operation_arn(connector_arn, &id).unwrap_or_else(|| {
            ::engine::emulator_arn(
                ARN_SERVICE,
                region,
                &format!("{CONNECTOR_OPERATION_KIND}/{connector_name}/{id}"),
            )
        }))
    }

    /// The operations stored against the connector `connector_arn` names, each
    /// behind the key it pages under ([`ConnectorOperationRecord::page_key`]),
    /// ready for [`page`].
    pub fn connector_operations_of(
        &self,
        connector_arn: &str,
    ) -> Vec<(String, &ConnectorOperationRecord)> {
        self.connector_operations
            .values()
            .filter(|operation| operation.connector_arn == connector_arn)
            .map(|operation| (operation.page_key(), operation))
            .collect()
    }

    /// The tags stored on the resource `arn` names, as an endpoint for `region`
    /// sees it, or `None` when this region holds no resource of that ARN.
    ///
    /// Every taggable resource this emulator holds is keyed by its own ARN, so
    /// one lookup covers all three kinds and a reader never has to know which
    /// kind an ARN names. The tag seam is scoped to the call's region exactly as
    /// every other read is, so another region's resource is as absent here as
    /// an ARN this store never minted.
    pub fn tags_of(&self, region: &str, arn: &str) -> Option<&HashMap<String, String>> {
        self.custom_plugin_in(region, arn)
            .map(|plugin| &plugin.tags)
            .or_else(|| self.connector_in(region, arn).map(|connector| &connector.tags))
            .or_else(|| {
                self.worker_configuration_in(region, arn)
                    .map(|worker| &worker.tags)
            })
    }

    /// The tags stored on the resource `arn` names in `region`, for a caller
    /// that is about to change them, or `None` when this region holds no
    /// resource of that ARN.
    ///
    /// The mutable twin of [`Self::tags_of`], under the same regional rule.
    /// PRIVATE on purpose: every tag write goes through [`Self::attach_tags`] or
    /// [`Self::remove_tags`], so the cap on a resource's stored tag count cannot
    /// be bypassed by a handler holding the map directly.
    fn tags_of_mut(&mut self, region: &str, arn: &str) -> Option<&mut HashMap<String, String>> {
        if let Some(plugin) = record_in_mut(&mut self.custom_plugins, region, arn) {
            return Some(&mut plugin.tags);
        }
        if let Some(connector) = record_in_mut(&mut self.connectors, region, arn) {
            return Some(&mut connector.tags);
        }
        record_in_mut(&mut self.worker_configurations, region, arn)
            .map(|worker| &mut worker.tags)
    }

    /// Attach `incoming` to the resource `arn` names, per key: a key the
    /// resource already carries takes the new value, a key `incoming` does not
    /// name keeps the value it had.
    ///
    /// Refused, with nothing written, when a key or value breaks an AWS tag
    /// rule ([`tag_map_breach`]), when the ARN names no resource this emulator
    /// holds, or when the merged set would hold more than
    /// [`MAX_TAGS_PER_RESOURCE`] keys — the one caller-driven dimension of this
    /// store that a repeated call could otherwise grow without bound.
    ///
    /// The tag rules are checked BEFORE the ARN is resolved, the way a service
    /// reads a request before looking anything up: a request the rules refuse is
    /// refused whether or not the resource it names exists.
    pub fn attach_tags(
        &mut self,
        region: &str,
        arn: &str,
        incoming: HashMap<String, String>,
    ) -> Result<(), TagWriteRefusal> {
        if let Some(breach) = tag_map_breach(&incoming) {
            return Err(TagWriteRefusal::IllegalTag(breach));
        }
        let tags = self
            .tags_of_mut(region, arn)
            .ok_or(TagWriteRefusal::NoSuchResource)?;
        // The size the merge would leave: the keys already stored that the
        // request does not name, plus the request's own keys.
        let would_hold = tags
            .keys()
            .filter(|key| !incoming.contains_key(*key))
            .count()
            + incoming.len();
        if would_hold > MAX_TAGS_PER_RESOURCE {
            return Err(TagWriteRefusal::CapExceeded { would_hold });
        }
        for (key, value) in incoming {
            tags.insert(key, value);
        }
        Ok(())
    }

    /// Remove the named keys from the resource `arn` names, per key: a key the
    /// request does not name keeps its value, and a key the resource does not
    /// carry is a no-op (so the call is safe to retry).
    ///
    /// Refused, with nothing removed, when a key breaks an AWS tag rule
    /// ([`tag_key_breach`]) — a key AWS reserves is not a caller's to strip —
    /// and when the ARN names no resource this emulator holds; a removal can
    /// never grow the stored set, so there is no cap to check.
    pub fn remove_tags(
        &mut self,
        region: &str,
        arn: &str,
        keys: &[String],
    ) -> Result<(), TagWriteRefusal> {
        if let Some(breach) = tag_key_breach(keys) {
            return Err(TagWriteRefusal::IllegalTag(breach));
        }
        let tags = self
            .tags_of_mut(region, arn)
            .ok_or(TagWriteRefusal::NoSuchResource)?;
        for key in keys {
            tags.remove(key);
        }
        Ok(())
    }

    /// The worker configuration of this name in `region`, if any — the
    /// per-region name rule [`record_named`] reads under.
    pub fn worker_configuration_named(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&WorkerConfigurationRecord> {
        record_named(&self.worker_configurations, region, name)
    }

    /// Mint the random-looking suffix of a resource ARN: the UUID AWS gives a
    /// kafkaconnect resource.
    ///
    /// Derived from this service's own mint counter through SHA-256, with the
    /// version and variant nibbles set, so the result is a syntactically valid
    /// v4-shaped UUID that is unique within the emulator and reproducible for a
    /// given sequence of calls — never derived from the request's own values,
    /// which would collide across two resources of the same name.
    pub fn mint_resource_id(&mut self, kind: &str, name: &str) -> String {
        self.minted = self.minted.saturating_add(1);
        let seed = format!("{ARN_SERVICE}/{kind}/{name}/{}", self.minted);
        let digest = sha2::Sha256::digest(seed.as_bytes());
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
}

/// Whether `pending` is a removal that is already owed at `now`.
fn removal_due(pending: Option<PendingChange>, now: Instant) -> bool {
    pending.is_some_and(|change| change.kind == PendingKind::Remove && change.is_due(now))
}

/// Whether `pending` is a move out of the creating state that is already owed.
fn settling_due(pending: Option<PendingChange>, now: Instant) -> bool {
    pending.is_some_and(|change| change.kind == PendingKind::Settle && change.is_due(now))
}

/// The resource type a connector operation is named under: the kind its minted
/// id is seeded with ([`ComAmazonawsKafkaconnectKafkaconnectState::mint_resource_id`])
/// and the type its ARN reports it as, so the two can never name it differently.
const CONNECTOR_OPERATION_KIND: &str = "connector-operation";

/// The resource type a connector itself is named under: the kind its minted id
/// is seeded with and the type its ARN's resource field opens with, so the mint
/// and the ARN can never name it differently.
pub const CONNECTOR_KIND: &str = "connector";

/// The ARN of an operation against the connector `connector_arn` names, or
/// `None` when that value is not a connector ARN this service minted.
///
/// An operation belongs to its connector, so its ARN is the connector's own —
/// same partition, region and account, so neither the region of the call that
/// started the operation nor anything else can move it to another connector's
/// scope — with the `connector/<name>/<id>` resource re-typed
/// `connector-operation/<name>/<id>` and the operation's own minted id
/// appended.
///
/// The five fields come from the ONE shared ARN parse
/// (`service_common::arn`), which ends the scope at the ARN's own field
/// boundaries and the resource TYPE at the first separator of the resource
/// field. A resource name is caller-supplied text — `connectorName` carries a
/// `@length` and no `@pattern`, so a name may hold `:` and `/`, including the
/// very `:connector/` this re-typing looks for — and text inside the name can
/// never be read as a field boundary.
pub fn connector_operation_arn(connector_arn: &str, operation_id: &str) -> Option<String> {
    let arn = ::service_common::arn::parse_of(connector_arn, ARN_SERVICE)?;
    let named = ::service_common::arn::resource_id_of_type(connector_arn, CONNECTOR_KIND)?;
    (!named.is_empty()).then(|| {
        format!(
            "arn:{}:{}:{}:{}:{CONNECTOR_OPERATION_KIND}/{named}/{operation_id}",
            arn.partition, arn.vendor, arn.region, arn.account
        )
    })
}

/// `at` as the `__timestampIso8601` the read operations carry.
pub fn date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The longest Region LABEL this service will read out of a credential scope.
///
/// Every `*Arn` pattern in the AWS models bounds the Region field at
/// `[a-z0-9-]{1,20}`, so a longer value is not a Region any AWS endpoint has.
/// The same bound `service_common::arn` reads an ARN's Region under.
const MAX_REGION_LABEL: usize = 20;

/// Whether `value` is a Region LABEL rather than arbitrary text: non-empty, at
/// most [`MAX_REGION_LABEL`] characters, and composed only of `[a-z0-9-]`.
///
/// The region a request is read with is not just data: it is embedded in every
/// ARN this service mints AND in the authority of the endpoint URL the
/// in-process S3 resolution is addressed to, so a value carrying `/`, `@`, `:`,
/// `?` or `#` — which would re-point or truncate an authority — is not a Region
/// this service will use for either.
fn is_region_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REGION_LABEL
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region every ARN this request mints carries, and the region its
/// in-process calls to other emulated services are addressed in: the one the
/// caller signed for, or [`DEFAULT_REGION`] when the request carries no
/// credential scope — or one whose region field is not a Region label at all
/// ([`is_region_label`]), which names no AWS endpoint and must never reach an
/// ARN or a URL authority.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_label(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// The validation message every kafkaconnect parameter rejection carries.
//
// kafkaconnect answers a rejection it blames on a request member with a message
// that NAMES that member: `Invalid parameter <member>`, optionally followed by
// `: <reason>`. The model states no message text, so the form is taken from the
// service's own recorded rejections (`Invalid parameter nextToken.`,
// `Invalid parameter bucketArn: Invalid ARN.`,
// `Invalid parameter customPluginArn: The requested resource doesn't exist.`).
// Both builders live here so every handler in `src/ops/` renders the one form
// rather than each inventing a sentence.
//
// They are for a rejection the service blames on a member of the request: a
// value it cannot parse, one naming a resource it does not hold, and one whose
// value collides with state the account already holds (a name already taken) —
// the request's own `name`/`connectorName` is the member that conflicted, so the
// same member-naming form carries the modelled conflict reason. A quota refusal
// blames no member of the request and keeps its own wording.
// ---------------------------------------------------------------------------

/// The lead-in of every kafkaconnect parameter-rejection message.
const INVALID_PARAMETER: &str = "Invalid parameter";

/// The reason a parameter that names a resource this service does not hold is
/// rejected with. The apostrophe is U+2019, as the service writes it.
pub const RESOURCE_DOES_NOT_EXIST: &str = "The requested resource doesn\u{2019}t exist.";

/// The reason a parameter that must carry an ARN, and does not, is rejected
/// with.
pub const INVALID_ARN: &str = "Invalid ARN.";

/// The reason a capacity naming both of its forms, or neither, is refused with.
/// `Capacity` models `autoScaling` and `provisionedCapacity` as two optional
/// members of which a connector has exactly one — "whether it is auto scaled or
/// provisioned", as the member's own documentation puts it.
pub const ONE_CAPACITY_FORM: &str = "Specify exactly one of autoScaling or provisionedCapacity.";

/// The reason a delete of a resource a connector this account holds still runs
/// on is refused with: a connector is only stored against references that
/// resolve to an ACTIVE resource, so the resource stays while the connector
/// naming it does. `resource` is the service's own name for the kind of
/// resource the refused ARN names, and nothing else is interpolated: the
/// refusal is the same whichever connector names it and whatever ARN that
/// connector carries.
pub fn resource_is_in_use(resource: &str) -> String {
    format!("The {resource} is in use by a connector.")
}

/// The reason a call only a RUNNING connector can serve is refused with for a
/// connector in any other state: one still being created, already restarting,
/// updating or being deleted has no running workers for the call to act on.
pub fn connector_is_not_running() -> String {
    format!("The connector is not {CONNECTOR_RUNNING}.")
}

/// The reason a create whose name is already taken is refused with: the
/// modelled `ConflictException` sentence, as its own documentation states it
/// ("HTTP Status Code 409: Conflict. A resource with this name already exists.
/// Retry your request with another name." — the status prefix is the HTTP status
/// itself and the retry advice is advice, so the reason is the middle sentence).
///
/// It names NO resource and interpolates nothing: the conflict is the same
/// whatever name was asked for and whatever ARN the held resource carries, and a
/// generated ARN in a message would differ on every run.
pub const NAME_ALREADY_EXISTS: &str = "A resource with this name already exists.";

/// The message for a rejection of the member `member`, with no further reason:
/// `Invalid parameter <member>.`
///
/// The form this service's own recorded rejections of a value it simply does
/// not accept carry (`Invalid parameter nextToken.`,
/// `Invalid parameter currentVersion.`): the member name is the only variable
/// part, and no value of the request or of the store is interpolated.
pub fn invalid_parameter(member: &str) -> String {
    format!("{INVALID_PARAMETER} {member}.")
}

/// The message for a rejection of the member `member` for `reason` (itself a
/// full sentence): `Invalid parameter <member>: <reason>`
pub fn invalid_parameter_because(member: &str, reason: &str) -> String {
    format!("{INVALID_PARAMETER} {member}: {reason}")
}

/// The reason a member that must carry base64-encoded content, and does not, is
/// rejected with.
pub const NOT_BASE64: &str = "It must be base64 encoded.";

/// How many characters one base64 quantum encodes to.
const BASE64_QUANTUM: usize = 4;

/// Whether `value` is content the service can base64-decode.
///
/// The codec is `aws_smithy_types::base64` — the standard-alphabet decoder the
/// wire serializer already uses for every Smithy blob, so the alphabet read here
/// is the one RFC 4648 section 4 names (`+` and `/`, not the URL-safe `-` and
/// `_`) and no second base64 implementation enters this crate.
///
/// That decoder requires a whole number of quanta, while a client may send the
/// encoding without its trailing `=`, so a value carrying no `=` has the missing
/// padding supplied before it is decoded rather than being refused. A value that
/// carries `=` STATES its own padding and is decoded exactly as given, so padding
/// that is wrong (one `=` short, padding in the middle, a last quantum holding a
/// single character) is refused rather than repaired.
fn is_base64(value: &str) -> bool {
    if value.contains('=') {
        return ::aws_smithy_types::base64::decode(value).is_ok();
    }
    let missing = (BASE64_QUANTUM - value.len() % BASE64_QUANTUM) % BASE64_QUANTUM;
    let padded = format!("{value}{}", "=".repeat(missing));
    ::aws_smithy_types::base64::decode(&padded).is_ok()
}

/// The message a refusal of `member` carries when its documented base64 content
/// cannot be decoded, or `None` when the service can read `value`.
///
/// A member the model documents as base64 is DECODED by the service to read what
/// it carries, so a value that is not base64 carries nothing it can store: the
/// request is refused on that member, in the one member-naming form every
/// kafkaconnect rejection uses ([`invalid_parameter_because`]), and nothing is
/// persisted. Only the ENCODING is judged — whatever bytes a legal encoding
/// decodes to are content this service stores — and the message quotes NONE of
/// the value, which is modelled sensitive. The decoded bytes answer that one
/// question and are dropped at once, so nothing sized from the request is kept.
pub fn base64_content_refusal(member: &str, value: &str) -> Option<String> {
    (!is_base64(value)).then(|| invalid_parameter_because(member, NOT_BASE64))
}

/// Whether `value` is a well-formed ARN naming `service`.
///
/// The grammar is the one AWS publishes for every ARN,
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`, read through
/// the ONE caller-supplied-ARN parse this emulator shares
/// (`service_common::arn`): six `:`-separated fields, the first literally `arn`,
/// a non-empty partition, the service field the one asked for, and a non-empty
/// resource. `region` and `account-id` are deliberately unconstrained there — an
/// S3 bucket ARN leaves both empty (`arn:aws:s3:::bucket`) while most other ARNs
/// fill them — and the resource keeps every `:` and `/` it carries.
pub fn is_arn_for(value: &str, service: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(value, service)
}

/// The message a refusal of `member` carries when the value it holds is not a
/// well-formed ARN of `service` ([`is_arn_for`]), or `None` when it is one.
///
/// Every member this service documents as carrying an ARN — the ones bound to a
/// URI path as well as the ones a request body holds — is modelled a plain
/// `__string` with no `@pattern`, so the framework cannot refuse a value that is
/// not an ARN at the boundary and the handler is where it has to be refused.
///
/// A value that is not an ARN of the service owning the resource it is meant to
/// name names NO resource at all: that is incorrect INPUT, so it is refused on
/// the member that carried it with [`INVALID_ARN`] — the reason this service's
/// own recorded rejection of an ARN-typed member carries — and the modelled 400.
/// A WELL-FORMED ARN the store holds nothing at is a different answer: the
/// resource is absent, so that one keeps [`RESOURCE_DOES_NOT_EXIST`] and the
/// modelled 404. One seam, so every ARN-carrying member renders the one form.
///
/// What it judges is the ARN's GRAMMAR and its service field, nothing narrower. A
/// member documented as carrying the ARN of one KIND of resource needs that kind
/// judged as well, which is what [`s3_bucket_arn`] does for the one member this
/// service documents as a bucket ARN: reach for that one, not this, whenever a
/// member names a kind.
pub fn arn_syntax_refusal(member: &str, value: &str, service: &str) -> Option<String> {
    (!is_arn_for(value, service)).then(|| invalid_parameter_because(member, INVALID_ARN))
}

/// The name of the bucket the ARN `value` names, or `None` when `value` is not
/// an S3 bucket ARN.
///
/// An S3 bucket ARN is partition-global and account-less
/// (`arn:aws:s3:::my-bucket`), so the bucket name is the whole resource field —
/// read through the one shared ARN parse. A value naming an object
/// (`…:::bucket/key`) or an access point (`…:accesspoint/name`) is not a bucket
/// ARN this emulator addresses a bucket by, so neither is reported.
///
/// This is the ONE parse of a bucket-ARN-typed member: the handler refuses what
/// it rejects ([`s3_bucket_arn`]) and the file resolution addresses the bucket it
/// reports ([`take_on_custom_plugin_file`]), so the refusal and the read cannot
/// disagree about what a bucket ARN is.
pub fn s3_bucket_name(value: &str) -> Option<&str> {
    let bucket = ::service_common::arn::parse_of(value, S3_ARN_SERVICE)?.resource;
    (!bucket.is_empty() && !bucket.contains(['/', ':'])).then_some(bucket)
}

/// The bucket the member `member` names, or the message a refusal of that member
/// carries when its value is not the ARN of an S3 BUCKET.
///
/// A member documented as carrying the ARN of one kind of resource names a
/// resource of that kind or nothing at all: an `s3` ARN whose resource field is
/// an object under a bucket (`…:::bucket/key`) or a sub-resource such as an
/// access point (`…:accesspoint/name`) addresses no bucket, so it is incorrect
/// INPUT and is refused on the member that carried it with [`INVALID_ARN`] and
/// the modelled 400 — the same reason and the same
/// `Invalid parameter <member>: <reason>` form every other ARN-typed member of
/// this service renders ([`arn_syntax_refusal`]). The model types every such
/// member a plain `__string` with no `@pattern`, so the handler is where it has
/// to be refused.
///
/// Reporting the bucket rather than just a verdict is what keeps the check and
/// the later read on ONE parse ([`s3_bucket_name`]): a caller cannot accept a
/// value the resolution then cannot address.
pub fn s3_bucket_arn<'a>(member: &str, value: &'a str) -> Result<&'a str, String> {
    s3_bucket_name(value).ok_or_else(|| invalid_parameter_because(member, INVALID_ARN))
}

/// The member a refusal of the custom plugin's file blames: the request's
/// `location.s3Location`, the only member naming an object this service has to
/// read (`CustomPluginLocation$s3Location`, "The S3 bucket Amazon Resource Name
/// (ARN), file key, and object version of the plugin file stored in Amazon S3").
pub const S3_LOCATION_MEMBER: &str = "s3Location";

/// The reason a custom plugin file this account cannot read is rejected with.
///
/// The service takes the plugin's bytes on at create time, so a file it cannot
/// read is a bad REQUEST (400) rather than a resource that is missing: the
/// wording names the three things that make a file unreadable, as the service
/// words it.
pub const CUSTOM_PLUGIN_FILE_UNREADABLE: &str =
    "Could not access the specified custom plugin file. Make sure the bucket is in the same \
     region, the file exists and you have access to it.";

/// The Amazon S3 error codes in which S3 reports that the object a request
/// names cannot be READ: it is not there (`NoSuchBucket`, `NoSuchKey`,
/// `NoSuchVersion`, `InvalidBucketName` — a name S3 can address no bucket by,
/// under which no bucket can ever be held) or reading it is not permitted
/// (`AccessDenied`, `AllAccessDisabled`, a bucket policy or an ownership check
/// refusing the read).
///
/// Keyed on S3's OWN code rather than on the HTTP status, because a status is
/// not evidence that S3 answered at all: a request that reached no S3 is a 404
/// too, and that is this emulator failing to ask rather than a verdict about the
/// object. The credential refusals (`InvalidAccessKeyId`,
/// `SignatureDoesNotMatch`) are left out for the same reason — they say the call
/// was made wrong, not that the caller may not read the file.
///
/// The list is deliberately CLOSED: a refusal code S3 adds later (an ownership
/// or a KMS refusal, say) falls into the warn-and-accept branch of
/// [`take_on_custom_plugin_file`] — [`CustomPluginFileVerdict::Unknown`] — rather
/// than refusing the create, because a code this service does not recognise is
/// not evidence about the object.
const UNREADABLE_CODES: &[&str] = &[
    "NoSuchBucket",
    "NoSuchKey",
    "NoSuchVersion",
    // A name S3 can address no bucket by at all: no bucket can ever be held under it, so
    // this is S3 reporting that the file is not there just as `NoSuchBucket` is.
    "InvalidBucketName",
    "AccessDenied",
    "AllAccessDisabled",
];

/// An Amazon S3 client over the IN-PROCESS callback into this same emulator.
///
/// NOTHING here opens a socket or leaves the host: `ctx.callback()` is the
/// framework's documented seam for calling another emulated service, and the
/// `http_client` it hands over dispatches the request inside this process to
/// whichever mounted service claims it (the S3 emulator, on the request's own
/// `s3` credential scope). The `endpoint_url` below is therefore an ADDRESS the
/// in-process router reads, never a host any resolver looks up — and `region` is
/// the request's own signing region, already narrowed to a Region label by
/// [`request_region`], so the caller cannot steer that authority.
///
/// The credentials are the emulator's own root credentials, which only sign this
/// operation's in-process calls; they are never put in a response or a log.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    // The one value that reaches the endpoint's authority is narrowed HERE, in
    // every build: `request_region` already answers only a Region label, and
    // this is the guarantee the authority rests on, so it is re-established
    // rather than asserted (a `debug_assert!` is compiled out of the build that
    // ships). A value outside the grammar falls back to the default Region,
    // exactly as a request carrying no credential scope does.
    let region = if is_region_label(region) {
        region
    } else {
        DEFAULT_REGION
    };
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so the
        // bucket is addressed in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// What reading the custom plugin file at a create's `location` established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomPluginFileVerdict {
    /// Amazon S3 reports the object cannot be read — it is not there, or the
    /// read is not permitted — so the create is refused and nothing is stored.
    Unreadable,
    /// The file was read: these are the facts the plugin's revision reports.
    TakenOn(CustomPluginFile),
    /// Nothing is known about the object, because there was no store to ask.
    /// Nothing is refused on an assumption, and the revision reports no file
    /// description it did not compute.
    Unknown,
}

/// Take the custom plugin file `location` names on: read it through the
/// emulator's own Amazon S3 and report what that read established.
///
/// A custom plugin's file is resolved when the plugin is CREATED — the service
/// takes the artifact on at that point — so a location naming an object that is
/// not there, or one this account may not read, is refused with the operation's
/// modelled `BadRequestException` and nothing is persisted. S3 is reached the
/// way any emulated service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`], which dispatches
/// inside this process — see [`s3_client`]: no socket, no outbound request, no
/// host resolution), and with the caller's `objectVersion` when it named one, so
/// the version the plugin pins is the version that has to resolve.
///
/// The read is a WHOLE-OBJECT `GetObject`, because the two facts a revision
/// reports about its file — the hex MD5 checksum and the size in bytes — are
/// properties of the artifact's own bytes, so they are computed here from those
/// bytes rather than taken from anything S3 reports about the object. (A ranged
/// read is not an option: `GetObject` with a `Range` is answered 501 by the S3
/// emulator, which would make every create unverifiable.) The body is read one
/// yielded chunk at a time and folded straight into the digest and the running
/// byte count ([`digest_plugin_file`]), so no copy of the whole artifact is
/// built however large it is and nothing of it is stored in this service.
///
/// The `bucket` to read from is the one the caller's `bucketArn` names, parsed
/// out of it by [`s3_bucket_name`] — the parse this request was already judged
/// by ([`s3_bucket_arn`]), passed in rather than taken again here, so there is no
/// second reading of the member that could accept what the first refused.
///
/// The verdict is read from S3's own error CODE and never from the HTTP status:
/// a request that reached no S3 at all is a 404 too, so a status cannot tell S3
/// reporting an unreadable object from this emulator failing to ask. That is why
/// anything other than a recognised refusal answers [`CustomPluginFileVerdict::Unknown`]
/// rather than refusing the create: with no S3 to ask (a router with only this
/// service mounted, say) nothing is known about the object, and a refusal has to
/// be grounded in what S3 answered, never assumed.
pub async fn take_on_custom_plugin_file(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    location: &S3Location,
) -> CustomPluginFileVerdict {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let mut read = s3_client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(&location.file_key);
    if let Some(object_version) = location.object_version.as_deref() {
        read = read.version_id(object_version);
    }
    match read.send().await {
        Ok(object) => match digest_plugin_file(object.body).await {
            Ok(file) => CustomPluginFileVerdict::TakenOn(file),
            Err(error) => {
                // The object resolved, so there is nothing to refuse, but its
                // bytes did not arrive whole: a digest over a truncated body
                // would describe a file the caller never uploaded.
                ::tracing::warn!(
                    bucket_arn = ?location.bucket_arn,
                    file_key = ?location.file_key,
                    error = %error,
                    "the custom plugin file resolved but its bytes could not be read whole"
                );
                CustomPluginFileVerdict::Unknown
            }
        },
        Err(error) => {
            if UNREADABLE_CODES.contains(&error.code().unwrap_or_default()) {
                // S3 says the object is not there, or not readable by this
                // account: the refusal is grounded.
                return CustomPluginFileVerdict::Unreadable;
            }
            // Any other answer is this emulator failing to ask its own S3, not
            // S3 reporting an unreadable object, so it refuses nothing — and it
            // is worth a warning, because a deployment with no S3 mounted to ask
            // is one where this check does not run. The caller's own values are
            // rendered with their escaped `Debug` form, so a key carrying a
            // newline cannot forge a log line.
            ::tracing::warn!(
                bucket_arn = ?location.bucket_arn,
                file_key = ?location.file_key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                code = ?error.code(),
                "the custom plugin file could not be resolved against the emulator's Amazon S3"
            );
            CustomPluginFileVerdict::Unknown
        }
    }
}

/// The facts a revision reports about an artifact, folded out of the artifact's
/// own bytes as the read yields them: the hex-encoded MD5 digest of those bytes
/// and how many of them there are.
///
/// MD5 is an incremental digest, so each chunk the body yields is folded into it
/// and into the byte count and then dropped. No second copy of the artifact is
/// built — not even a concatenated one — so the memory one create needs does not
/// grow with the size of the file, and there is no ceiling past which this
/// service stops describing a plugin it read.
///
/// The digest is MD5 because that is the checksum the member reports
/// (`CustomPluginFileDescription$fileMd5`, "The hex-encoded MD5 checksum of the
/// custom plugin file"), and it is rendered lowercase hex, the form "hex-encoded"
/// names and the form a client comparing it against its own digest of the file
/// computes. It is a CONTENT checksum the API contract fixes — what a client
/// validates an upload with — and nothing here authenticates anything by it.
///
/// The error is the read's own: a body that did not arrive whole describes no
/// file, so the caller reports nothing rather than a digest over a truncated
/// artifact.
async fn digest_plugin_file(
    mut body: ::aws_smithy_types::byte_stream::ByteStream,
) -> Result<CustomPluginFile, ::aws_smithy_types::byte_stream::error::Error> {
    use ::md5::Digest as _;

    let mut digest = ::md5::Md5::new();
    let mut size: i64 = 0;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        size = size.saturating_add(i64::try_from(chunk.len()).unwrap_or(i64::MAX));
        digest.update(&chunk);
    }
    Ok(CustomPluginFile {
        md5: ::hex::encode(digest.finalize()),
        size,
    })
}

/// One page of a list operation: the items to answer with, and the `nextToken`
/// that resumes after them (`None` when this page is the last).
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `nextToken` that is not a resume point for the listing it was handed to:
/// one the service never emitted, or one another listing emitted.
#[derive(Debug)]
pub struct BadPageToken;

/// WHICH listing a `nextToken` resumes.
///
/// A token is a resume point for exactly ONE listing — the operation that minted
/// it, over the key space that call paged. No two of this service's listings
/// page one key space: each kind of resource is keyed by its own ARN kind, a
/// connector's operations are keyed by their creation instant, a listing only
/// ever covers the resources of the region it is answered through, and a listing
/// the request NARROWED (a name prefix, a parent resource) covers only the keys
/// that survived the narrowing. So a token carries the listing that minted it
/// and [`page`] refuses one minted anywhere else, instead of comparing a key
/// from a foreign key space against its own keys — which silently truncates a
/// page (every key sorting past the foreign one is dropped) or silently replays
/// one (none of them does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listing<'a> {
    /// The operation that both mints and accepts the token: its wire name.
    operation: &'a str,
    /// The key space that operation paged: what distinguishes two listings of
    /// the same operation from each other.
    scope: &'a str,
    /// What the REQUEST narrowed that key space to, verbatim — the filter value
    /// the call paged under, or `""` when the call paged the whole scope. Two
    /// calls that filter differently page different key spaces, so neither one's
    /// resume key means anything in the other's.
    filter: &'a str,
}

impl<'a> Listing<'a> {
    /// A listing of every resource of one kind in ONE region: two regions hold
    /// independent resources, so each region's listing has its own key space.
    pub fn regional(operation: &'a str, region: &'a str) -> Self {
        Self {
            operation,
            scope: region,
            filter: "",
        }
    }

    /// A listing of what is stored against one parent resource, named by its
    /// ARN — which carries the region too, so it scopes the listing by itself.
    pub fn of_resource(operation: &'a str, parent_arn: &'a str) -> Self {
        Self {
            operation,
            scope: parent_arn,
            filter: "",
        }
    }

    /// The same listing narrowed by a filter the REQUEST carried — the name
    /// prefix every one of this service's filtered listings takes.
    ///
    /// Narrowing drops keys, so a narrowed listing is a key space of its own:
    /// its resume key is the first key of ITS next page, which in the wider
    /// listing has keys before it that the wider listing still owes the caller.
    /// Binding the filter into the listing is what makes a token minted under
    /// one filter refused under another, rather than honoured and answered with
    /// a page that silently drops everything sorting below that key.
    ///
    /// A filter of `""` narrows nothing — every key starts with the empty
    /// prefix — so it names the SAME key space as a request that carried no
    /// filter at all, and their tokens are interchangeable, as they must be: the
    /// two calls page identical keys.
    pub fn narrowed_by(self, filter: &'a str) -> Self {
        Self { filter, ..self }
    }

    /// The prefix a token this listing minted carries, behind the service-wide
    /// marker: a fixed-width digest of the listing's identity.
    ///
    /// A digest rather than the identity itself, so no listing's prefix can be a
    /// prefix of another's however the scope happens to be punctuated, and so a
    /// token's length says nothing about the resource it resumes after.
    ///
    /// Each part of the identity is digested behind its own length, so the parts
    /// cannot be re-cut: with a separator alone, a caller choosing a filter that
    /// contains the separator could make two different listings digest the same
    /// bytes and so share a token.
    fn token_prefix(&self) -> String {
        let mut digest = sha2::Sha256::new();
        for part in [self.operation, self.scope, self.filter] {
            let length = u64::try_from(part.len()).unwrap_or(u64::MAX);
            digest.update(length.to_be_bytes());
            digest.update(part.as_bytes());
        }
        format!(
            "{PAGE_TOKEN_MARKER}{}:",
            hex::encode(&digest.finalize()[..8])
        )
    }
}

/// Page `keyed` — `(sort key, item)` pairs, one per stored resource — honouring
/// `max_results` and resuming after `next_token`, a token `listing` minted.
///
/// The key is the resource's ARN: unique and stable, so sorting by it gives both
/// a total order and a resume point that survives a resource being created or
/// deleted between two pages. The token is opaque (hex of that key behind
/// `listing`'s own prefix), as the model's `@paginated` contract requires: a
/// client only ever echoes it back.
///
/// A token that is not a resume point for `listing` — one the service never
/// emitted, or one a DIFFERENT listing emitted — is [`BadPageToken`], which
/// every caller answers as the modelled `BadRequestException`. It is never
/// honoured as a resume point: a key from another listing's key space would
/// hide, or replay, the resources this listing holds.
///
/// `max_results` is clamped rather than refused because a value outside the
/// `MaxResults` shape's `range 1..100` never reaches here: the framework refuses
/// it from the model's own constraint before the handler runs (the tests assert
/// 0 and 101 are 400s). The clamp is what keeps this helper total should a later
/// caller page an unconstrained count.
pub fn page<T>(
    mut keyed: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    listing: Listing<'_>,
) -> Result<Page<T>, BadPageToken> {
    let resume_after = match next_token {
        Some(token) => Some(decode_page_token(token, listing)?),
        None => None,
    };
    let page_size = max_results
        .unwrap_or(MAX_PAGE_SIZE)
        .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
    let page_size = usize::try_from(page_size).unwrap_or(1);

    keyed.sort_by(|left, right| left.0.cmp(&right.0));

    let mut items = Vec::new();
    let mut next = None;
    for (key, item) in keyed {
        if resume_after.as_deref().is_some_and(|from| key.as_str() < from) {
            continue;
        }
        if items.len() == page_size {
            next = Some(encode_page_token(&key, listing));
            break;
        }
        items.push(item);
    }
    Ok(Page {
        items,
        next_token: next,
    })
}

/// The opaque form of a resume key: the key behind a prefix only `listing`
/// writes, hex-encoded. The prefix is what makes a token from somewhere else
/// detectable — without it any hex string would decode to "some" resume key and
/// be honoured as a silent resume point.
fn encode_page_token(key: &str, listing: Listing<'_>) -> String {
    hex::encode(format!("{}{key}", listing.token_prefix()))
}

/// The resume key inside an opaque token, or [`BadPageToken`] when the token is
/// not one `listing` emitted.
fn decode_page_token(token: &str, listing: Listing<'_>) -> Result<String, BadPageToken> {
    let bytes = hex::decode(token).map_err(|_| BadPageToken)?;
    let decoded = String::from_utf8(bytes).map_err(|_| BadPageToken)?;
    decoded
        .strip_prefix(&listing.token_prefix())
        .map(ToString::to_string)
        .ok_or(BadPageToken)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKafkaconnectKafkaconnectState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKafkaconnectKafkaconnectState>)
}
