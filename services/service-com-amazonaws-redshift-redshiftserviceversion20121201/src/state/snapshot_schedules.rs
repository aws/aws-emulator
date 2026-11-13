// SPDX-License-Identifier: CC0-1.0

//! The snapshot schedules an account created: the record each one is, the
//! expressions its definitions are read under, and the invocation times and
//! cluster associations a read of it reports.
//!
//! A snapshot schedule is "a regular interval for creating snapshots of a
//! cluster" (`smithy_query(query="shape", shape="SnapshotSchedule")`), so the
//! definitions it carries are what every upcoming invocation of it is COMPUTED
//! from — one reading of them ([`parse_schedule_definition`]) decides both
//! whether a definition is one the service admits at all and when the schedule
//! it states fires, so no stored schedule carries a definition nothing can be
//! computed from.
//!
//! The members a read reports that are NOT stored —
//! `SnapshotSchedule$NextInvocations`, `$AssociatedClusters` and
//! `$AssociatedClusterCount` — are derived here
//! ([`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshot_schedule_view`]),
//! from the definitions and from the clusters the store holds, so the create
//! path and the read path cannot report a different schedule.

use super::*;

/// The most snapshot schedules this store holds, past which
/// `CreateSnapshotSchedule` refuses with the modelled
/// `SnapshotScheduleQuotaExceededFault`.
///
/// The same reasoning as [`SNAPSHOT_COPY_GRANT_QUOTA`]: the emulator's own bound
/// on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshot_schedules`],
/// not a claim about an account's published limit. A schedule's identity is the
/// identifier its create gives it, and identifiers are unlimited in number, so
/// this bound is what stops the collection growing without end; it is set far
/// above any workflow an emulator serves.
pub const SNAPSHOT_SCHEDULE_QUOTA: usize = 10_000;

/// The most definitions one snapshot schedule may carry.
///
/// The emulator's own bound, like [`SNAPSHOT_SCHEDULE_QUOTA`], and for a
/// sharper reason: every definition of a schedule is read and evaluated on
/// every request that reports it, so the number a request may put in one
/// schedule is what the work of reporting it grows with. The bound is set far
/// above any schedule a client writes — a snapshot schedule states when a
/// cluster's backups are taken, and the published rate expressions take one at
/// most every minute — so it refuses no schedule a caller means and caps the
/// work one request can ask for.
pub const MAX_SCHEDULE_DEFINITIONS: usize = 100;

/// The member a `ScheduleIdentifier` refusal names.
pub const SNAPSHOT_SCHEDULE_IDENTIFIER_MEMBER: &str = "ScheduleIdentifier";

/// The longest a snapshot schedule's identifier may be, in characters.
///
/// The model bounds the member only at `len 0..2147483647`, which is no bound a
/// store can hold to, so the length every other Redshift identifier of a NAMED
/// resource is bounded at serves this one too ([`SNAPSHOT_IDENTIFIER_RULE`],
/// [`EVENT_SUBSCRIPTION_NAME_RULE`]).
pub const SNAPSHOT_SCHEDULE_IDENTIFIER_MAX_LEN: usize = 255;

/// Check a snapshot schedule's identifier, naming the constraint it broke.
///
/// Source: `smithy_query(query="doc", shape="CreateSnapshotScheduleMessage",
/// member="ScheduleIdentifier")` — "A unique identifier for a snapshot schedule.
/// Only alphanumeric characters are allowed for the identifier." That sentence
/// is the WHOLE published rule for this member, which is why this check is its
/// own rather than [`check_identifier`]'s: the rule the control-plane
/// identifiers share also demands a leading letter, no trailing hyphen and no
/// doubled hyphen, and a schedule identifier the service documents nothing of
/// the sort for (`2026backups`) is one a caller may name.
///
/// The hyphen is admitted beside those alphanumerics: Amazon Redshift's own
/// schedule identifiers carry hyphens, so refusing one would refuse a call the
/// service serves. Mixed case is admitted and never folded — the doc states no
/// folding, so a caller naming a schedule in another case names no schedule.
pub fn check_snapshot_schedule_identifier(value: &str) -> Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "Invalid value for {SNAPSHOT_SCHEDULE_IDENTIFIER_MEMBER}: '{value}'. {why} (allowed: 1 to {SNAPSHOT_SCHEDULE_IDENTIFIER_MAX_LEN} alphanumeric characters or hyphens)"
        ))
    };
    if value.is_empty() {
        return refuse("The identifier cannot be null, empty, or blank.");
    }
    if value.chars().count() > SNAPSHOT_SCHEDULE_IDENTIFIER_MAX_LEN {
        return refuse("The identifier is longer than allowed.");
    }
    if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return refuse(
            "The identifier contains a character that is neither alphanumeric nor a hyphen.",
        );
    }
    Ok(())
}

/// The `ScheduleState` of an association the service has finished applying.
///
/// Source: `smithy_query(query="shape", shape="ClusterAssociatedToSchedule")` —
/// `ScheduleAssociationState` targets the `ScheduleState` enum, whose values are
/// `ACTIVE`, `MODIFYING` and `FAILED`.
pub const SCHEDULE_ASSOCIATION_STATE_ACTIVE: &str = "ACTIVE";

/// The `ScheduleState` of an association the service is still applying: the
/// cluster it is an association of is not serving yet, so the schedule is not
/// taking snapshots of it yet either. See
/// [`SCHEDULE_ASSOCIATION_STATE_ACTIVE`].
pub const SCHEDULE_ASSOCIATION_STATE_MODIFYING: &str = "MODIFYING";

/// The most clusters a schedule reports as associated with it.
///
/// Source: `smithy_query(query="doc", shape="SnapshotSchedule",
/// member="AssociatedClusters")` — "A list of clusters associated with the
/// schedule. A maximum of 100 clusters is returned." The COUNT
/// (`AssociatedClusterCount`) is not bounded by it: it is "the number of
/// clusters associated with the schedule", so it counts every association even
/// when the list reports the first hundred of them.
pub const MAX_REPORTED_ASSOCIATED_CLUSTERS: usize = 100;

/// How many upcoming invocations a schedule reports when the request asks for no
/// particular number: the next one.
///
/// `SnapshotSchedule$NextInvocations` is the schedule's upcoming snapshot times,
/// and `CreateSnapshotScheduleMessage$NextInvocations` is how many of them a
/// create asks to be reported (both members are modelled without
/// documentation — `smithy_query(query="doc", shape="SnapshotSchedule",
/// member="NextInvocations")` is empty). `DescribeSnapshotSchedules` carries no
/// such member at all, so the number it reports is this service's own: the next
/// time the schedule fires, which is what a caller polling a schedule reads.
pub const DEFAULT_REPORTED_NEXT_INVOCATIONS: usize = 1;

/// The most upcoming invocations one response reports, whatever a request asks
/// for.
///
/// What bounds the walk over a schedule's definitions: each definition is
/// evaluated at most this many times, so one request's work is proportional to
/// this bound rather than to the number a caller names.
pub const MAX_REPORTED_NEXT_INVOCATIONS: usize = 100;

/// The `rate(...)` expression type: a fixed interval between snapshots.
const RATE_EXPRESSION: &str = "rate";

/// The `cron(...)` expression type: a calendar schedule.
///
/// `pub(crate)` with [`expression_body`] and [`parse_cron_body`], because the
/// scheduled actions of [`super::scheduled_actions`] read a calendar schedule
/// from the SAME documented AWS cron format — so there is one reading of it in
/// this store rather than one per family.
pub(crate) const CRON_EXPRESSION: &str = "cron";

/// The units a `rate(value unit)` definition states its interval in, each
/// paired with how long one of it lasts.
///
/// Source: the AWS rate-expression syntax the model's own example writes
/// (`smithy_query(query="doc", shape="CreateSnapshotScheduleMessage",
/// member="ScheduleDefinitions")` — "for example … \"rate(12 hours)\""): a
/// positive value and a unit, singular when the value is one and plural
/// otherwise. Matched case-insensitively, and the singular and plural spellings
/// are both admitted for every unit, because the value decides which spelling is
/// the natural one rather than the service doing so.
const RATE_UNITS: [(&str, Duration); 6] = [
    ("minute", Duration::from_secs(60)),
    ("minutes", Duration::from_secs(60)),
    ("hour", Duration::from_secs(60 * 60)),
    ("hours", Duration::from_secs(60 * 60)),
    ("day", Duration::from_secs(24 * 60 * 60)),
    ("days", Duration::from_secs(24 * 60 * 60)),
];

/// How many fields the SHORTEST cron expression this service reads carries:
/// the minute, the hour and the day-of-month.
///
/// Source: `smithy_query(query="doc", shape="CreateSnapshotScheduleMessage",
/// member="ScheduleDefinitions")` — "for example \"cron(30 12 *)\"": the
/// service's own example of a snapshot schedule definition carries three fields,
/// so an expression that names only the leading fields of the format below is
/// one this service reads. A field the expression omits stands for every value
/// of it, which is what makes the three-field example the same schedule as
/// `cron(30 12 * * ? *)`.
const CRON_FIELDS_MINIMUM: usize = 3;

/// How many fields the longest one carries: the minute, the hour, the
/// day-of-month, the month, the day-of-week and the year.
///
/// Source: `smithy_query(query="doc", shape="ScheduledAction", member="Schedule")`
/// — "Format of cron expressions is \"cron(Minutes Hours Day-of-month Month
/// Day-of-week Year)\". For example, \"cron(0 10 ? * MON *)\"" — the cron
/// expression Amazon Redshift reads a schedule from, which is why a definition
/// may be written with the year field or without it.
const CRON_FIELDS_WITH_YEAR: usize = 6;

/// The cron fields a `?` ("no specific value") may stand in, as positions within
/// the fields above: the day-of-month and the day-of-week, the pair the
/// documented example puts it in.
const CRON_NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [2, 4];

/// One definition of a snapshot schedule, read as the schedule it states.
///
/// The two expression types a definition may be written in (see
/// [`parse_schedule_definition`]), each carrying what the invocation times are
/// computed from: an interval for a rate expression and a parsed schedule for a
/// cron one.
#[derive(Debug, Clone)]
pub enum SnapshotScheduleDefinition {
    /// `rate(value unit)`: snapshots this far apart, the first one this far
    /// after the schedule was asked about.
    Rate(Duration),
    /// `cron(...)`: snapshots at the calendar instants the expression names.
    /// Boxed, because a parsed schedule is much larger than an interval and this
    /// enum is held per definition.
    Cron(Box<::cron::Schedule>),
}

impl SnapshotScheduleDefinition {
    /// The first `count` instants this definition fires at, strictly after
    /// `after`.
    ///
    /// Every conversion is fallible on purpose: a definition came from a
    /// caller's request, so an instant outside the range this service computes
    /// in drops out of the report instead of being reported as some other
    /// instant.
    #[must_use]
    pub fn upcoming(&self, after: SystemTime, count: usize) -> Vec<SystemTime> {
        match self {
            Self::Rate(interval) => (1..=count)
                .filter_map(|step| {
                    let elapsed = interval.checked_mul(u32::try_from(step).ok()?)?;
                    after.checked_add(elapsed)
                })
                .collect(),
            Self::Cron(schedule) => {
                let Some(after) = utc_datetime(after) else {
                    return Vec::new();
                };
                schedule
                    .after(&after)
                    .take(count)
                    .filter_map(system_time)
                    .collect()
            }
        }
    }
}

/// Why an expression is not one this service reads a schedule from, or not one
/// it stores a schedule with.
///
/// The split both schedule families share: an expression written in a TYPE the
/// service does not read a schedule from at all is a different refusal from one
/// written in an offered type that states no schedule. Which fault each maps to
/// is the OPERATION's: `CreateSnapshotSchedule` answers
/// `ScheduleDefinitionTypeUnsupportedFault` and `InvalidScheduleFault`, while
/// `CreateScheduledAction` answers the `InvalidParameterValue` code and
/// `InvalidScheduleFault` (see
/// [`super::scheduled_actions::parse_scheduled_action_schedule`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleDefinitionRefusal {
    /// The expression is not written in a type this service reads a schedule
    /// from at all.
    Unsupported(String),
    /// The expression is written in one of those types, but states no schedule
    /// an invocation can be computed from.
    Invalid(String),
}

/// The body of `expression` when it is written in the `kind` expression type —
/// what stands between `kind(` and the closing `)` — and `None` when it is
/// written in another type.
///
/// The type name is matched case-insensitively, as the expression types
/// themselves are, and the body is handed back untrimmed: each type's own
/// reading decides what its fields are.
pub(crate) fn expression_body<'a>(expression: &'a str, kind: &str) -> Option<&'a str> {
    let rest = expression.strip_suffix(')')?;
    let (named, body) = rest.split_at_checked(kind.len())?;
    if !named.eq_ignore_ascii_case(kind) {
        return None;
    }
    body.strip_prefix('(')
}

/// One definition of a snapshot schedule, read as the schedule it states, or why
/// it is not a definition this service stores a schedule with.
///
/// The definition "is made up of schedule expressions, for example
/// \"cron(30 12 *)\" or \"rate(12 hours)\"" (`smithy_query(query="doc",
/// shape="CreateSnapshotScheduleMessage", member="ScheduleDefinitions")`), so
/// those two expression types are the ones a schedule is read from and any other
/// is [`ScheduleDefinitionRefusal::Unsupported`]. A rate expression names a
/// positive interval; a cron expression names the calendar instants of the
/// documented AWS cron format, whole or down to its leading fields (see
/// [`CRON_FIELDS_MINIMUM`]), read in UTC.
pub fn parse_schedule_definition(
    definition: &str,
) -> Result<SnapshotScheduleDefinition, ScheduleDefinitionRefusal> {
    let expression = definition.trim();
    if let Some(body) = expression_body(expression, RATE_EXPRESSION) {
        return parse_rate_expression(expression, body);
    }
    if let Some(body) = expression_body(expression, CRON_EXPRESSION) {
        return parse_cron_expression(expression, body);
    }
    Err(ScheduleDefinitionRefusal::Unsupported(
        unsupported_schedule_definition_message(expression),
    ))
}

/// A `rate(value unit)` body as the interval it states.
fn parse_rate_expression(
    definition: &str,
    body: &str,
) -> Result<SnapshotScheduleDefinition, ScheduleDefinitionRefusal> {
    let invalid = || ScheduleDefinitionRefusal::Invalid(invalid_schedule_message(definition));
    let fields: Vec<&str> = body.split_whitespace().collect();
    let [value, unit] = fields.as_slice() else {
        return Err(invalid());
    };
    // The value is a plain decimal number: the published syntax spells it in
    // digits, so a signed or otherwise decorated spelling of one is not a rate
    // the service reads. Checked before the parse, which would admit a leading
    // `+`.
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    // A rate of zero states no interval at all, so no invocation follows from it.
    let value: u32 = value.parse().map_err(|_| invalid())?;
    if value == 0 {
        return Err(invalid());
    }
    let (_, span) = RATE_UNITS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .ok_or_else(invalid)?;
    let interval = span.checked_mul(value).ok_or_else(invalid)?;
    Ok(SnapshotScheduleDefinition::Rate(interval))
}

/// A `cron(...)` body as the schedule it states.
///
/// A field the expression omits stands for every value of it (see
/// [`CRON_FIELDS_MINIMUM`]), so the trailing fields are filled with the `*` that
/// spells exactly that before the whole expression is read.
fn parse_cron_expression(
    definition: &str,
    body: &str,
) -> Result<SnapshotScheduleDefinition, ScheduleDefinitionRefusal> {
    let schedule = parse_cron_body(body)
        .ok_or_else(|| ScheduleDefinitionRefusal::Invalid(invalid_schedule_message(definition)))?;
    Ok(SnapshotScheduleDefinition::Cron(Box::new(schedule)))
}

/// The body of a `cron(...)` expression — what stands inside its brackets — as
/// the schedule it states, and `None` when it states none.
///
/// A field the expression omits stands for every value of it (see
/// [`CRON_FIELDS_MINIMUM`]), so the trailing fields are filled with the `*` that
/// spells exactly that before the whole expression is read.
pub(crate) fn parse_cron_body(body: &str) -> Option<::cron::Schedule> {
    let fields: Vec<&str> = body.split_whitespace().collect();
    if !(CRON_FIELDS_MINIMUM..=CRON_FIELDS_WITH_YEAR).contains(&fields.len()) {
        return None;
    }
    let stated = fields.iter().enumerate().map(|(position, field)| {
        if *field == "?" && CRON_NO_SPECIFIC_VALUE_FIELDS.contains(&position) {
            "*"
        } else {
            *field
        }
    });
    let omitted = ::std::iter::repeat_n("*", CRON_FIELDS_WITH_YEAR - fields.len());
    // The `cron` grammar starts with a SECONDS field, which this format has no
    // field for: the finest unit either family schedules on is the minute, so
    // the second is a literal zero.
    let seconds = ::std::iter::once("0");
    let normalised: Vec<&str> = seconds.chain(stated).chain(omitted).collect();
    normalised.join(" ").parse::<::cron::Schedule>().ok()
}

/// Why a definition written in an expression type this service does not read a
/// schedule from is refused, naming the definition the request sent.
///
/// The fixed text is what the model states the fault is
/// (`smithy_query(query="shape", shape="ScheduleDefinitionTypeUnsupportedFault")`
/// — "The definition you submitted is not supported"), with the definition it is
/// about named after it.
pub fn unsupported_schedule_definition_message(definition: &str) -> String {
    format!("The schedule definition '{definition}' you submitted is not supported.")
}

/// Why a definition that states no schedule is refused, naming the definition
/// the request sent.
///
/// The fixed text is what the model states the fault is
/// (`smithy_query(query="shape", shape="InvalidScheduleFault")` — "The schedule
/// you submitted isn't valid"), with the definition it is about named after it.
pub fn invalid_schedule_message(definition: &str) -> String {
    format!("The schedule '{definition}' you submitted isn't valid.")
}

/// Why a schedule carrying more definitions than [`MAX_SCHEDULE_DEFINITIONS`]
/// is refused, naming how many the request carried.
///
/// The fixed text is what the model states the fault is, as
/// [`invalid_schedule_message`] words it for one definition; the variable part
/// is the bound and the count the request sent.
pub fn too_many_schedule_definitions_message(carried: usize) -> String {
    format!(
        "The schedule you submitted isn't valid: a snapshot schedule holds at most {MAX_SCHEDULE_DEFINITIONS} schedule definitions, and the request carried {carried}."
    )
}

/// Why a create under an identifier the account already holds a snapshot
/// schedule under is refused, naming the schedule the request asked for.
///
/// The IDENTIFIER is what an account's schedules are unique on, so the refusal
/// names it — single-quoted, as this family words an identifier it reports back
/// inside a sentence (see [`snapshot_copy_grant_already_exists_message`]).
pub fn snapshot_schedule_already_exists_message(identifier: &str) -> String {
    format!(
        "Cannot create the snapshot schedule because a schedule with the identifier '{identifier}' already exists."
    )
}

/// Why a create past [`SNAPSHOT_SCHEDULE_QUOTA`] is refused.
pub fn snapshot_schedule_quota_message() -> String {
    format!(
        "The request would exceed the allowed number of snapshot schedules ({SNAPSHOT_SCHEDULE_QUOTA})."
    )
}

/// Why a request asking for a number of upcoming invocations outside the window
/// one response reports is refused, naming the number it asked for and the bound.
///
/// The member is modelled without documentation and without a `@range`
/// (`smithy_query(query="doc", shape="CreateSnapshotScheduleMessage",
/// member="NextInvocations")` is empty), so the bound is enforced here and NAMED
/// in the refusal: a count the service will not report is answered as a bad
/// value rather than served a different number of invocations than it asked for.
pub fn next_invocations_out_of_range_message(asked: i32) -> String {
    format!(
        "Invalid value for NextInvocations: {asked}. NextInvocations must be between 0 and {MAX_REPORTED_NEXT_INVOCATIONS}."
    )
}

/// The first `count` instants `definitions` fire at, strictly after `after`, in
/// chronological order.
///
/// A schedule's definitions are the expressions it takes snapshots on, so its
/// upcoming invocations are every definition's upcoming instants merged into ONE
/// chronological list and cut at `count` — no definition can contribute more
/// than that many, which is what bounds the walk. Two definitions due at the
/// same instant are ONE invocation: the schedule takes one snapshot then.
///
/// A definition that states no schedule contributes nothing rather than failing
/// the report: the create that stored it refused every such definition
/// ([`parse_schedule_definition`]), so this is reachable only for a definition a
/// later caller could not have stored at all.
#[must_use]
pub fn next_invocations(
    definitions: &[String],
    after: SystemTime,
    count: usize,
) -> Vec<::aws_smithy_types::DateTime> {
    let count = count.min(MAX_REPORTED_NEXT_INVOCATIONS);
    if count == 0 {
        return Vec::new();
    }
    let mut instants: Vec<SystemTime> = definitions
        .iter()
        .filter_map(|definition| parse_schedule_definition(definition).ok())
        .flat_map(|definition| definition.upcoming(after, count))
        .collect();
    instants.sort_unstable();
    instants.dedup();
    instants.truncate(count);
    instants
        .into_iter()
        .map(::aws_smithy_types::DateTime::from)
        .collect()
}

/// `at` as a UTC date and time, or `None` when it is outside the range a date
/// and time can represent.
///
/// `pub(crate)` with [`system_time`], so the scheduled actions of
/// [`super::scheduled_actions`] convert between the two clocks through the same
/// pair.
pub(crate) fn utc_datetime(at: SystemTime) -> Option<::chrono::DateTime<::chrono::Utc>> {
    let seconds = at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    ::chrono::DateTime::from_timestamp(i64::try_from(seconds).ok()?, 0)
}

/// A UTC date and time as a `SystemTime`, or `None` when it is before the epoch
/// and so outside the range this service computes in.
pub(crate) fn system_time(at: ::chrono::DateTime<::chrono::Utc>) -> Option<SystemTime> {
    let seconds = u64::try_from(at.timestamp()).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

/// A snapshot schedule as this service stores it: what the create gave it, and
/// nothing a read derives.
///
/// The modelled `SnapshotSchedule` shape is NOT the whole record and is not
/// stored as one: three of its members —
/// `NextInvocations`, `AssociatedClusters` and `AssociatedClusterCount` — are
/// computed when the schedule is reported, from the definitions below and from
/// the clusters the store holds
/// ([`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshot_schedule_view`]).
/// Storing them would let a schedule report invocation times that have already
/// passed and clusters that have since been deleted.
#[derive(Debug, Clone)]
pub struct StoredSnapshotSchedule {
    /// The identifier the create gave the schedule, which is what an account's
    /// schedules are unique on.
    pub schedule_identifier: String,
    /// The schedule expressions the schedule takes snapshots on, in the order
    /// the request carried them.
    pub schedule_definitions: Vec<String>,
    /// The description the create gave the schedule, and `None` when it gave
    /// none.
    pub schedule_description: Option<String>,
    /// The tags the create gave the schedule — an empty list when it gave none,
    /// which is what the schedule then carries.
    pub tags: Vec<crate::ops::types::Tag>,
}

impl ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// The stored snapshot schedule with this `ScheduleIdentifier`, if any.
    ///
    /// The identifier is compared exactly as the create that minted the schedule
    /// spelled it: the documented syntax
    /// ([`check_snapshot_schedule_identifier`]) states no case folding, so a
    /// caller naming it in another case names no schedule.
    pub fn snapshot_schedule(&self, identifier: &str) -> Option<&StoredSnapshotSchedule> {
        self.snapshot_schedules
            .iter()
            .find(|schedule| schedule.schedule_identifier == identifier)
    }

    /// The clusters associated with the schedule this `ScheduleIdentifier`
    /// names, in the order the store holds them, and how many there are in all.
    ///
    /// A cluster is associated with a schedule by naming it
    /// (`Cluster$SnapshotScheduleIdentifier`), so the associations are read off
    /// the clusters rather than held beside the schedule: a cluster that is
    /// deleted, or whose schedule is changed, stops being one of them at that
    /// moment, with nothing to keep in step.
    ///
    /// The association is reported as still being applied while the cluster
    /// itself is not serving — the schedule takes snapshots of a cluster only
    /// once that cluster is up — and as active from then on (see
    /// [`SCHEDULE_ASSOCIATION_STATE_ACTIVE`]). The list is cut at
    /// [`MAX_REPORTED_ASSOCIATED_CLUSTERS`] while the count is not.
    pub fn clusters_associated_to_schedule(
        &self,
        identifier: &str,
    ) -> (Vec<ClusterAssociatedToSchedule>, usize) {
        let associated = self.clusters.iter().filter(|stored| {
            stored.cluster.snapshot_schedule_identifier.as_deref() == Some(identifier)
        });
        let count = associated.clone().count();
        let reported = associated
            .take(MAX_REPORTED_ASSOCIATED_CLUSTERS)
            .map(|stored| {
                // Only a cluster the store reports as SERVING has a schedule
                // that is taking snapshots of it; a cluster in any other status,
                // or one reporting none at all, is one the association is still
                // being applied to.
                let serving =
                    stored.cluster.cluster_status.as_deref() == Some(STATUS_AVAILABLE);
                ClusterAssociatedToSchedule {
                    cluster_identifier: stored.cluster.cluster_identifier.clone(),
                    schedule_association_state: Some(
                        if serving {
                            SCHEDULE_ASSOCIATION_STATE_ACTIVE
                        } else {
                            SCHEDULE_ASSOCIATION_STATE_MODIFYING
                        }
                        .to_string(),
                    ),
                }
            })
            .collect();
        (reported, count)
    }

    /// One stored schedule as the modelled `SnapshotSchedule` every response
    /// carries it as: what the create stored, plus the members derived from it
    /// and from the clusters around it.
    ///
    /// `next_invocations` is how many upcoming invocation times to report (see
    /// [`DEFAULT_REPORTED_NEXT_INVOCATIONS`]), computed at `now` so a schedule
    /// read twice reports the invocations still ahead of each read rather than
    /// the ones ahead of its create.
    pub fn snapshot_schedule_view(
        &self,
        stored: &StoredSnapshotSchedule,
        now: SystemTime,
        next_invocations: usize,
    ) -> CreateSnapshotScheduleOutput {
        let (associated_clusters, associated_cluster_count) =
            self.clusters_associated_to_schedule(&stored.schedule_identifier);
        CreateSnapshotScheduleOutput {
            schedule_identifier: Some(stored.schedule_identifier.clone()),
            schedule_definitions: Some(stored.schedule_definitions.clone()),
            schedule_description: stored.schedule_description.clone(),
            tags: Some(stored.tags.clone()),
            next_invocations: Some(self::next_invocations(
                &stored.schedule_definitions,
                now,
                next_invocations,
            )),
            associated_cluster_count: Some(
                i32::try_from(associated_cluster_count).unwrap_or(i32::MAX),
            ),
            associated_clusters: Some(associated_clusters),
        }
    }
}
