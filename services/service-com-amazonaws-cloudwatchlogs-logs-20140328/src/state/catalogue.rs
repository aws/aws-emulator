// SPDX-License-Identifier: CC0-1.0

//! The service's DOCUMENTED CONSTANTS: the defaults a member omitted takes, the quotas a
//! request is held to, the catalogues of values an enum-like string member may carry, and
//! the ARN and name fragments the records are built from.
//!
//! One concern, one file: these are the values the handlers and the records beside them
//! read, each carrying the sentence of the model or the service documentation it comes
//! from. They live here rather than in [`super`] so that the state root holds the records
//! and the operations on them, and every path a caller already uses
//! (`crate::state::<NAME>`) still resolves — the parent re-exports this module.

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Generated emulators default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The log group class `CreateLogGroup` applies when the request omits
/// `logGroupClass` ("If you omit this parameter, the default of STANDARD is used").
pub const DEFAULT_LOG_GROUP_CLASS: &str = "STANDARD";

/// The storage tier `GetStorageTierPolicy` reports for an account no `PutStorageTierPolicy`
/// has ever set: the tier log data is stored in until the account opts into
/// `INTELLIGENT_TIERING`, which is what `PutStorageTierPolicy` documents as the opt-in
/// ("When you set the storage tier to INTELLIGENT_TIERING, the service automatically moves
/// log data to the most cost-effective storage tier based on access frequency"). Spelled
/// as the model's `StorageTier` enum spells it (`STORAGE_TIER_VALUES`).
pub const DEFAULT_STORAGE_TIER: &str = "STANDARD";

/// The one log group class a log-group-level transformer may be created on, from
/// `PutTransformer`: "You can create transformers only for the log groups in the
/// Standard log class." Spelled as the model's `LogGroupClass` spells it
/// (`LOG_GROUP_CLASS_VALUES`). Named apart from [`DEFAULT_LOG_GROUP_CLASS`] because the
/// two state different rules — the class a group gets when the request names none, and
/// the class this feature requires — even where the model spells them the same.
pub const TRANSFORMER_LOG_GROUP_CLASS: &str = "STANDARD";

/// Per-event storage overhead CloudWatch Logs charges a log group's `storedBytes`.
/// From the PutLogEvents quota: a batch's size "is calculated as the sum of all event
/// messages in UTF-8, plus 26 bytes for each log event".
pub const EVENT_OVERHEAD_BYTES: i64 = 26;

/// The largest single log event CloudWatch Logs holds, from `PutLogEvents`: "Each log event
/// can be no larger than 1 MB."
///
/// A bound on a log event MESSAGE wherever one arrives, not only on an ingested batch: a
/// `TestMetricFilter` sample message larger than the largest log event the service holds is
/// no log event message at all. It lives here so the two operations are held to the one
/// number.
pub const MAX_LOG_EVENT_BYTES: i64 = 1_048_576;

/// The refusal for a log event message larger than one CloudWatch Logs holds: the
/// service's own documented sentence for the condition, which states the ceiling and
/// interpolates nothing from the request. Kept beside
/// [`MAX_LOG_EVENT_BYTES`] so the sentence and the number it states cannot disagree.
pub const LOG_EVENT_TOO_LARGE: &str = "Each log event can be no larger than 1 MB.";

/// The number of items `DescribeLogGroups` / `DescribeLogStreams` return when the
/// request omits `limit` ("If you don't specify a value, the default is up to 50 items").
pub const DEFAULT_DESCRIBE_LIMIT: i32 = 50;

/// The number of events `GetLogEvents` returns when the request omits `limit`: the
/// documented ceiling of "up to 10,000 log events" per page.
pub const DEFAULT_EVENTS_LIMIT: i32 = 10_000;

/// The number of items the name-pattern list operations return when the request omits
/// `limit`: `ListLogGroups` ("If you omit this parameter, the default is up to 50 log
/// groups") and `ListAggregateLogGroupSummaries` ("If you omit this parameter, the
/// default is up to 50 aggregated summaries").
pub const DEFAULT_LIST_LIMIT: i32 = 50;

/// How long after a log group is created the AGGREGATE summaries of
/// `ListAggregateLogGroupSummaries` count it.
///
/// That operation "returns an aggregate summary of all log groups in the Region grouped by
/// specified data source characteristics" — a view maintained beside the log groups
/// themselves rather than read off them, so a group enters it a short span after the create
/// that made it, and a caller that aggregates straight after creating one is answered
/// without it while every later call counts it. The span is this emulator's own choice (the
/// service publishes none) and is the one every clock-owed transition here takes, so a
/// caller sees the view catch up rather than never settle. Only the aggregate view lags:
/// `DescribeLogGroups` and `ListLogGroups` report a group as soon as it is created.
pub const AGGREGATE_SUMMARY_VISIBILITY_DELAY_MS: i64 = 3_000;

/// How often the service MEASURES the volume a log group stores, which is the figure
/// `LogGroup$storedBytes` reports.
///
/// That member is "The number of bytes stored" — a figure the service meters for a log group
/// on its own cycle rather than one counted off the events a request just wrote, which is why
/// `PutRetentionPolicy` can state that events already marked for deletion "are also not
/// included when you use an API to retrieve the storedBytes value": what the member reports is
/// the last MEASUREMENT, not what the group holds at the instant of the read. So a group whose
/// events arrived since the last pass reports the figure that pass measured — zero for a group
/// that has never been metered — and the figure moves only when the next pass falls due.
///
/// The cycle's length is this emulator's own choice (the service publishes none). It is
/// deliberately far longer than the spans this emulator owes a resource's STATUS: a status is a
/// transition a caller waits on, while this is a measurement a caller must not see follow its
/// own writes — a client that reads straight after ingesting has to be answered the figure the
/// service would have answered then. The instant each pass falls due is stored on the group and
/// never slept on ([`crate::state::LogGroupRecord::materialize_due_metering`]).
pub const LOG_GROUP_METERING_INTERVAL_MS: i64 = 60 * 60 * 1_000;

/// The shortest `logGroupNamePattern` the model admits: `LogGroupNameRegexPattern`
/// carries `@length(min: 3, max: 129)`.
pub const LOG_GROUP_NAME_PATTERN_MIN_LEN: usize = 3;

/// The longest `logGroupNamePattern` the model admits. See
/// [`LOG_GROUP_NAME_PATTERN_MIN_LEN`].
pub const LOG_GROUP_NAME_PATTERN_MAX_LEN: usize = 129;

/// How many `|`-separated alternatives one `logGroupNamePattern` carries, from the
/// member's own documentation: "You can specify as many as five different regular
/// expression patterns in this field".
pub const MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES: usize = 5;

/// The shortest one alternative of a `logGroupNamePattern` can be, from the same
/// sentence: "each of which must be between 3 and 24 characters".
///
/// The MAXIMUM half of that sentence — and of the `@pattern`'s `{3,24}` segments — is
/// deliberately NOT a constant here: the service accepts an alternative longer than 24
/// characters (log group names routinely exceed it), so refusing one would refuse a
/// request the service answers. See [`check_log_group_name_pattern`].
pub const MIN_LOG_GROUP_NAME_PATTERN_ALTERNATIVE_LEN: usize = 3;

/// The prefix anchor a `logGroupNamePattern` alternative may carry: "Use ^ to match log
/// group names by prefix."
pub const LOG_GROUP_NAME_PATTERN_ANCHOR: char = '^';

/// The separator between a `logGroupNamePattern`'s alternatives: "To match multiple
/// patterns, separate them with a | as in the example ^/aws/lambda|discovery".
pub const LOG_GROUP_NAME_PATTERN_SEPARATOR: char = '|';

/// AWS's documented log group quota, from `CreateLogGroup`: "You can create up to
/// 1,000,000 log groups per Region per account."
pub const MAX_LOG_GROUPS: usize = 1_000_000;

/// The refusal a create that would need a log group past [`MAX_LOG_GROUPS`] carries: the
/// quota that refused it, so a caller can tell this limit from a request it could fix.
///
/// Stated once here because more than one operation creates a log group — `CreateLogGroup`
/// and the managed destination group an import task delivers into — and the quota is the
/// account's, so both refuse past it with the same sentence.
pub fn log_groups_quota_message() -> String {
    format!("the maximum of {MAX_LOG_GROUPS} log groups per Region per account has been reached")
}

/// What this service states for a delete of a log group under deletion protection: "When
/// enabled on a log group, deletion protection blocks all deletion operations until it is
/// explicitly disabled" (`PutLogGroupDeletionProtection`).
///
/// Stated once here because more than one caller deletes a log group — this service's own
/// `DeleteLogGroup`, and the resource operations another service runs on a group it holds —
/// and it is the same group under the same rule, so both report the same sentence.
pub fn log_group_delete_protected_message() -> String {
    "LogGroup has delete protection enabled. Disable delete protection before attempting to \
     delete the resource."
        .to_string()
}

/// What this service states for a tag set past [`MAX_TAGS_PER_RESOURCE`], which is the quota
/// `TooManyTagsException` states ("A resource can have no more than 50 tags").
///
/// Stated once here for the same reason as [`log_group_delete_protected_message`]: every
/// caller that assigns a resource's tags is held to the one quota and reports it alike.
pub fn too_many_tags_message() -> String {
    format!(
        "a resource can have no more than {MAX_TAGS_PER_RESOURCE} tags"
    )
}

/// AWS's documented resource-policy quota, from `PutResourcePolicy`: "An account can
/// have a maximum of 10 policies without resourceARN and one per LogGroup resourceARN".
/// The per-resource half of that quota is structural here: a resource-scoped policy is
/// stored ON its log group ([`LogGroupRecord::resource_policy`]), so a group can hold
/// exactly one.
pub const MAX_ACCOUNT_RESOURCE_POLICIES: usize = 10;

/// AWS's documented per-resource tag quota, from `TagResource`: "You can associate as many
/// as 50 tags with a CloudWatch Logs resource" — the quota `TooManyTagsException` states
/// ("A resource can have no more than 50 tags").
///
/// The modelled `Tags` map is `@length(max: 50)`, so the boundary validator already refuses
/// a single request carrying more than this; the quota this constant holds a resource to is
/// on the tag set the resource ends up with, which a request inside that length can still
/// push past when the resource already carries tags.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key namespace Amazon Web Services keeps for its own tags, from AWS's tag
/// restrictions: "Do not use the aws: prefix in your tag names or values because it is
/// reserved for Amazon Web Services use. You can't edit or delete tag names or values with
/// this prefix."
///
/// So a key in it is never a caller's to assign: a tagging request that names one is refused
/// and assigns nothing. `TagKey`'s own `@pattern` admits `:`, so nothing refuses such a key
/// before a handler runs.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether a tagging request names any key in [`RESERVED_TAG_KEY_PREFIX`] — the keys that are
/// not the caller's to assign.
///
/// What is reserved is the prefix itself rather than one spelling of it, so a key is matched
/// against it without regard to case. The comparison is on BYTES, so a key whose fifth byte
/// falls inside a multi-byte character is answered rather than panicked on.
pub fn names_reserved_tag_key<'a>(keys: impl IntoIterator<Item = &'a str>) -> bool {
    keys.into_iter().any(|key| {
        key.as_bytes()
            .get(..RESERVED_TAG_KEY_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
    })
}

/// The refusal CloudWatch Logs answers a tagging request that names a key in the reserved
/// namespace. ONE sentence for every such request: what is refused is the namespace itself,
/// so the refusal carries nothing the request chose, and it states the prefix in the casing
/// the service names it by even though the reservation ignores case.
pub const RESERVED_TAG_KEY_MESSAGE: &str = "Tags starting with 'AWS:' are reserved for AWS use.";

/// The longest `policyDocument` `PutDataProtectionPolicy` admits, from that member's own
/// documentation: "The JSON specified in policyDocument can be up to 30,720 characters."
/// The model puts no `@length` on `DataProtectionPolicyDocument`, so nothing refuses an
/// over-long document before the handler runs.
pub const MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS: usize = 30_720;

/// The account-level policy type that protects sensitive data, from the model's
/// `PolicyType` enum (`DATA_PROTECTION_POLICY` | `SUBSCRIPTION_FILTER_POLICY` |
/// `FIELD_INDEX_POLICY` | `TRANSFORMER_POLICY` | `METRIC_EXTRACTION_POLICY` |
/// `RETENTION_POLICY`, the same values listed as
/// [`crate::ops::types::POLICY_TYPE_VALUES`]). `policyType` targets that enum, so the
/// boundary validator has already held it to them by the time a handler runs.
pub const POLICY_TYPE_DATA_PROTECTION: &str = "DATA_PROTECTION_POLICY";

/// The account-level policy type that feeds log events to another service. See
/// [`POLICY_TYPE_DATA_PROTECTION`] for where these five values come from.
pub const POLICY_TYPE_SUBSCRIPTION_FILTER: &str = "SUBSCRIPTION_FILTER_POLICY";

/// The account-level policy type that indexes fields. See
/// [`POLICY_TYPE_DATA_PROTECTION`].
pub const POLICY_TYPE_FIELD_INDEX: &str = "FIELD_INDEX_POLICY";

/// The account-level policy type that transforms ingested log events. See
/// [`POLICY_TYPE_DATA_PROTECTION`].
pub const POLICY_TYPE_TRANSFORMER: &str = "TRANSFORMER_POLICY";

/// The account-level policy type that controls EMF metric creation. See
/// [`POLICY_TYPE_DATA_PROTECTION`].
pub const POLICY_TYPE_METRIC_EXTRACTION: &str = "METRIC_EXTRACTION_POLICY";

/// The account-level policy type that sets the retention period of the log groups it
/// selects — "A retention policy sets the retention period for log groups in your account"
/// — see [`POLICY_TYPE_DATA_PROTECTION`].
pub const POLICY_TYPE_RETENTION: &str = "RETENTION_POLICY";

/// The `IndexPolicy$source` of a policy that applies to a subset of the account's log
/// groups rather than to the one group it is reported for, from the model's `IndexSource`
/// enum (`ACCOUNT` | `LOG_GROUP`, the pair listed as
/// [`crate::ops::types::INDEX_SOURCE_VALUES`]). The member "indicates whether this is an
/// account-level index policy or an index policy that applies only to a single log group".
pub const INDEX_SOURCE_ACCOUNT: &str = "ACCOUNT";

/// The `IndexPolicy$source` of a policy stored on the one log group it applies to. See
/// [`INDEX_SOURCE_ACCOUNT`].
pub const INDEX_SOURCE_LOG_GROUP: &str = "LOG_GROUP";

/// The one log group class a METRIC FILTER can be put on: `logGroupClass` documents that
/// "the `Standard` log class supports all CloudWatch Logs features" while "the `Infrequent
/// Access` log class supports a subset of CloudWatch Logs features", and metric extraction
/// from ingested events is one of the features outside that subset (the member's own "Log
/// classes" reference). Spelled as the model's `LogGroupClass` spells it
/// (`LOG_GROUP_CLASS_VALUES`), and named apart from the other features' classes because each
/// states a rule of its own.
pub const METRIC_FILTER_LOG_GROUP_CLASS: &str = "STANDARD";

/// The one log group class a SUBSCRIPTION FILTER can be put on; see
/// [`METRIC_FILTER_LOG_GROUP_CLASS`] for where the rule comes from — real-time delivery of
/// ingested events is likewise outside the subset the Infrequent Access class supports.
pub const SUBSCRIPTION_FILTER_LOG_GROUP_CLASS: &str = "STANDARD";

/// The one log group class a LIVE TAIL session can tail; see
/// [`METRIC_FILTER_LOG_GROUP_CLASS`] for where the rule comes from — Live Tail is likewise
/// outside the subset the Infrequent Access class supports.
pub const LIVE_TAIL_LOG_GROUP_CLASS: &str = "STANDARD";

/// The one log group class whose stored events `GetLogEvents` and `FilterLogEvents` read
/// back; see [`METRIC_FILTER_LOG_GROUP_CLASS`] for where the rule comes from — a log group of
/// a class that supports only a subset of the features takes events in and is queried through
/// CloudWatch Logs Insights instead.
pub const EVENT_SEARCH_LOG_GROUP_CLASS: &str = "STANDARD";

/// How CloudWatch Logs refuses a call that asks for a feature the log group's CLASS does not
/// support: the service's fixed sentence for the class, which carries nothing of the request.
///
/// The ONE place this service words that refusal, so the features gated on a class cannot
/// drift apart in how they say so.
pub const LOG_CLASS_UNSUPPORTED_OPERATION_MESSAGE: &str =
    "This operation is only supported on the Standard log class.";

/// The one log group class a log-group-level field index policy can be put on, from
/// `PutIndexPolicy`: "Only log groups in the Standard log class support field index
/// policies." Spelled as the model's `LogGroupClass` spells it
/// (`LOG_GROUP_CLASS_VALUES`), and named apart from [`TRANSFORMER_LOG_GROUP_CLASS`]
/// because it is a different feature's sentence even where the class is spelled the same.
pub const INDEX_POLICY_LOG_GROUP_CLASS: &str = "STANDARD";

/// How many field indexes one log-group-level field index policy can name, from
/// `PutIndexPolicy`'s quota list: "As many as 20 fields can be included in the policy."
/// Only the fields the document names count — "Default field indexes are not counted
/// towards your field index quota."
pub const MAX_INDEX_POLICY_FIELDS: usize = 20;

/// How long one indexed field's name can be, from the same quota list: "Each field name
/// can include as many as 100 characters."
pub const MAX_INDEX_FIELD_NAME_CHARS: usize = 100;

/// The scope an account-level policy carries when the put naming it specified none, from
/// `PutAccountPolicy$scope`: "Currently the only valid value for this parameter is ALL,
/// which specifies that the data protection policy applies to all log groups in the
/// account. If you omit this parameter, the default of ALL is used." It is the only value
/// of the model's `Scope` enum, so every stored account policy reports it.
pub const DEFAULT_ACCOUNT_POLICY_SCOPE: &str = "ALL";

/// The longest `policyDocument` `PutAccountPolicy` admits, from that member's own
/// documentation: "The JSON specified in policyDocument can be up to 30,720 characters
/// long." The model puts no `@length` on `AccountPolicyDocument`, so nothing refuses an
/// over-long document before the handler runs.
///
/// It is the same bound the log-group-level
/// [`MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS`] states, named separately because it is a
/// different member's sentence; an account-level data protection document is held to both,
/// which is one bound either way.
pub const MAX_ACCOUNT_POLICY_DOCUMENT_CHARS: usize = MAX_DATA_PROTECTION_POLICY_DOCUMENT_CHARS;

/// The longest `selectionCriteria` `PutAccountPolicy` admits, from that member's own
/// documentation: "The selectionCriteria string can be up to 25KB in length. The length is
/// determined by using its UTF-8 bytes."
pub const MAX_SELECTION_CRITERIA_BYTES: usize = 25 * 1024;

/// The policy types a `selectionCriteria` may be put with.
/// `PutAccountPolicy$selectionCriteria` names four — "Specifying selectionCriteria is
/// valid only when you specify SUBSCRIPTION_FILTER_POLICY, FIELD_INDEX_POLICY,
/// TRANSFORMER_POLICY, or RETENTION_POLICY for policyType" — and the operation's own
/// documentation adds the fifth, describing metric extraction policies "that are each
/// scoped to a subset of log groups with the selectionCriteria parameter", so a put of one
/// is accepted rather than refused on the member sentence alone. A data protection policy
/// is the one type neither sentence admits: it "applies to all log groups in the account".
pub const SELECTION_CRITERIA_POLICY_TYPES: [&str; 5] = [
    POLICY_TYPE_SUBSCRIPTION_FILTER,
    POLICY_TYPE_FIELD_INDEX,
    POLICY_TYPE_TRANSFORMER,
    POLICY_TYPE_METRIC_EXTRACTION,
    POLICY_TYPE_RETENTION,
];

/// How many values one `IN` or `NOT IN` list of a `selectionCriteria` may carry, from the
/// metric extraction policy section of `PutAccountPolicy`: "The selection criteria supports
/// filtering by LogGroupName and LogGroupNamePrefix using the operators IN and NOT IN. You
/// can specify up to 50 values in each IN or NOT IN list." The sentence is stated for that
/// type's criteria, so it is the bound that type's criteria is held to.
pub const MAX_SELECTION_CRITERIA_VALUES: usize = 50;

/// The `selectionCriteria` filter that selects log groups by their exact name, from
/// `PutAccountPolicy$selectionCriteria` ("the only supported selectionCriteria filter is
/// `LogGroupName NOT IN []`") and from the metric extraction formats `PutAccountPolicy`
/// spells out (`LogGroupName IN ["log-group-1", "log-group-2"]`).
pub const SELECTION_FIELD_LOG_GROUP_NAME: &str = "LogGroupName";

/// The `selectionCriteria` filter that selects log groups by a prefix of their name, from
/// the same two places (`LogGroupNamePrefix NOT IN ["/aws/prefix1", "/aws/prefix2"]`).
pub const SELECTION_FIELD_LOG_GROUP_NAME_PREFIX: &str = "LogGroupNamePrefix";

/// The `selectionCriteria` filter naming a data source, from
/// `PutAccountPolicy$selectionCriteria`: a field index policy may "use either
/// LogGroupNamePrefix by itself or DataSourceName and DataSourceType together".
pub const SELECTION_FIELD_DATA_SOURCE_NAME: &str = "DataSourceName";

/// The `selectionCriteria` filter naming a data source's type. See
/// [`SELECTION_FIELD_DATA_SOURCE_NAME`].
pub const SELECTION_FIELD_DATA_SOURCE_TYPE: &str = "DataSourceType";

/// The member of a `METRIC_EXTRACTION_POLICY` document that says what the policy does to
/// the Embedded Metrics Format. Such a policy exists to "disable EMF metric creation for
/// your entire account or specific log groups" (`PutAccountPolicy`), and the recorded
/// behaviour of the real service is that the document carries
/// `EmbeddedMetricFormat.Status` and that the service "accepts only the exact string
/// 'Disabled'", refusing anything else with `InvalidParameterException` and
/// [`INVALID_JSON_POLICY_DOCUMENT_MESSAGE`].
pub const METRIC_EXTRACTION_EMF_MEMBER: &str = "EmbeddedMetricFormat";

/// The member of `EmbeddedMetricFormat` carrying the status. See
/// [`METRIC_EXTRACTION_EMF_MEMBER`].
pub const METRIC_EXTRACTION_STATUS_MEMBER: &str = "Status";

/// The one `EmbeddedMetricFormat.Status` a metric extraction policy document may carry:
/// EMF metric creation "is enabled for all log groups" by default and a policy only ever
/// turns it off, so `Disabled` is the whole of what such a document can say. See
/// [`METRIC_EXTRACTION_EMF_MEMBER`].
pub const METRIC_EXTRACTION_STATUS_DISABLED: &str = "Disabled";

/// How the real service words its refusal of a metric extraction policy document it does
/// not accept, as the corpus recorded it: `('InvalidParameterException', 'Invalid json
/// policy document')`. See [`METRIC_EXTRACTION_EMF_MEMBER`].
pub const INVALID_JSON_POLICY_DOCUMENT_MESSAGE: &str = "Invalid json policy document";

/// The refusal CloudWatch Logs answers a `policyDocument` it cannot READ as the JSON
/// structure the policy type's grammar states: a document that is not JSON at all, one that
/// is not the object the policy is expressed as, and one whose member carries a JSON type
/// the grammar does not admit (an array written as a string, an object written as an array).
///
/// The text is FIXED for that class — it names neither the member at fault nor the type it
/// should have had — because the complaint is about the document's FORM rather than about
/// what an otherwise readable document says: a document the service can read and then
/// rejects for what it states (a value outside a catalogue, a quota, a rule between two of
/// its members) is refused with that rule's own sentence instead.
pub const MALFORMED_POLICY_DOCUMENT_MESSAGE: &str = "Policy document contains invalid JSON format.";

/// The refusal CloudWatch Logs answers a field index `policyDocument` it CAN read that names
/// no field index at all: "It must contain at least one field index"
/// (`PutIndexPolicy$policyDocument`). Like [`MALFORMED_POLICY_DOCUMENT_MESSAGE`] the sentence
/// is fixed for the class — it states the rule and interpolates nothing of the document — and
/// it is the same rule however the document names its fields (`Fields`, `FieldsV2`), so one
/// sentence answers every form of naming none.
pub const EMPTY_INDEX_POLICY_DOCUMENT_MESSAGE: &str = "Policy document should contain at least 1 field.";

/// The refusal the FIELD INDEX operations answer when the log group they resolve is not one
/// the account holds — the three index-policy operations and the field-index read, which
/// report on the same resource.
///
/// This surface words a missing log group its own way: it states the kind of resource that
/// could not be found and nothing else — neither the identifier the request carried nor the
/// account that would own the group — unlike the service-wide sentence
/// ([`super::resource_not_found_message`]) and unlike the data protection operations, which
/// name the group and its owner because such a policy is keyed by both.
pub const INDEX_POLICY_LOG_GROUP_NOT_FOUND_MESSAGE: &str = "Log group cannot be found.";

/// How many account-level policies of one type the account may hold when the policy being
/// put is "scoped to a subset of log groups with the selectionCriteria parameter": for a
/// transformer policy, "you can create as many as 20 account-level transformer policies",
/// and for a field index policy, "as many as 20 account-level field index policies".
pub const MAX_SCOPED_ACCOUNT_POLICIES: usize = 20;

/// The same quota for a metric extraction policy, which has its own number: "Each account
/// can have either one account-level metric extraction policy that applies to all log
/// groups, or up to 5 policies that are each scoped to a subset of log groups with the
/// selectionCriteria parameter."
pub const MAX_SCOPED_METRIC_EXTRACTION_ACCOUNT_POLICIES: usize = 5;

/// How many account-level RETENTION policies the account may hold: "Each account can have
/// up to 20 account-level retention policies in each Region."
///
/// That sentence is ONE number for the type rather than the either/or every other type's
/// quota is stated as, so it bounds the policies of this type whether or not they are
/// scoped with `selectionCriteria` — which is also why "retention policies can have
/// overlapping selection criteria" without the account running out of slots.
pub const MAX_RETENTION_ACCOUNT_POLICIES: usize = 20;

/// How many account-level policies of one type the account may hold when the policy being
/// put applies to EVERY log group — the "one account-level transformer policy that applies
/// to all log groups in the account" half of each quota sentence, and the whole quota for
/// the two types that have no scoped form: "Each account can have only one account-level
/// data protection policy" and "Each account can have one account-level subscription filter
/// policy per Region."
pub const MAX_UNSCOPED_ACCOUNT_POLICIES: usize = 1;

/// The prefix a `policyName` may not start with, from `PutAccountPolicy$policyName`: "A
/// name for the policy. This must be unique within the account and cannot start with
/// `aws/`." The model puts no `@pattern` on `PolicyName`, so nothing refuses such a name
/// before the handler runs.
pub const RESERVED_POLICY_NAME_PREFIX: &str = "aws/";

/// The `dataProtectionStatus` of a log group that has a data protection policy right
/// now, from the model's `DataProtectionStatus` enum (`ACTIVATED` | `DELETED` |
/// `ARCHIVED` | `DISABLED`).
pub const DATA_PROTECTION_STATUS_ACTIVATED: &str = "ACTIVATED";

/// The `dataProtectionStatus` of a log group whose data protection policy has been
/// deleted — the "or whether it had one in the past" half of `LogGroup$dataProtectionStatus`
/// ("Displays whether this log group has a protection policy, or whether it had one in
/// the past"). From the same `DataProtectionStatus` enum.
pub const DATA_PROTECTION_STATUS_DELETED: &str = "DELETED";

/// The scope of a policy attached to no resource, from the model's `PolicyScope` enum
/// (`ACCOUNT` | `RESOURCE`). This is also the scope `DescribeResourcePolicies` reports
/// on when the request names no `policyScope` ("When not specified, defaults to
/// ACCOUNT").
pub const POLICY_SCOPE_ACCOUNT: &str = "ACCOUNT";

/// The scope of a policy attached to one CloudWatch Logs resource, from the model's
/// `PolicyScope` enum (`ACCOUNT` | `RESOURCE`).
pub const POLICY_SCOPE_RESOURCE: &str = "RESOURCE";

/// The retentions a log group can be set to, from the model's `Days` shape: "The number
/// of days to retain the log events in the specified log group. Possible values are: 1,
/// 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557,
/// 2922, 3288, and 3653." `Days` is a plain integer in the model, so nothing refuses a
/// value outside this list before a handler runs. Ascending, so the list renders in the
/// documented order in a refusal message.
pub const RETENTION_IN_DAYS_VALUES: [i32; 22] = [
    1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
    3288, 3653,
];

/// The retentions an ACCOUNT-LEVEL retention policy's document can state, from
/// `PutAccountPolicy$policyDocument`: "The number of days to retain log events. Valid
/// values are 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
/// 3288, and 3653."
///
/// Those are the values of [`RETENTION_IN_DAYS_VALUES`] from 30 upwards, which is the same
/// catalogue narrowed by the note that "account-level retention policies require a minimum
/// retention period of 30 days" ([`MIN_ACCOUNT_RETENTION_IN_DAYS`]) — so it is DERIVED from
/// the log group's catalogue rather than typed out a second time. Ascending, so the list
/// renders in the documented order in a refusal message.
pub fn account_retention_in_days_values() -> Vec<i32> {
    RETENTION_IN_DAYS_VALUES
        .into_iter()
        .filter(|days| *days >= MIN_ACCOUNT_RETENTION_IN_DAYS)
        .collect()
}

/// The shortest retention an account-level retention policy may state, from
/// `PutAccountPolicy`: "Account-level retention policies require a minimum retention period
/// of 30 days."
pub const MIN_ACCOUNT_RETENTION_IN_DAYS: i32 = 30;

/// What a retention policy document's `retentionInDays` states instead of a number of days
/// when the log events it governs are to be kept for good, from
/// `PutAccountPolicy$policyDocument`: "To retain log events indefinitely, specify the
/// string NEVER_EXPIRE." It is also the value `LogGroupImpact$currentRetention` and
/// `$proposedRetention` report for such a retention ("A value of NEVER_EXPIRE indicates
/// that log events in the log group never expire").
pub const ACCOUNT_RETENTION_NEVER_EXPIRE: &str = "NEVER_EXPIRE";

/// The member of a `RETENTION_POLICY` document stating the retention period: "A retention
/// policy must include the following attributes in a JSON block: retentionInDays [...]
/// enforcementMode" (`PutAccountPolicy$policyDocument`).
pub const ACCOUNT_RETENTION_DAYS_MEMBER: &str = "retentionInDays";

/// The member of a `RETENTION_POLICY` document stating which log groups the policy applies
/// to. See [`ACCOUNT_RETENTION_DAYS_MEMBER`].
pub const ACCOUNT_RETENTION_ENFORCEMENT_MODE_MEMBER: &str = "enforcementMode";

/// The `enforcementMode` that applies a retention policy "only to log groups that are
/// created after you create the policy. Log groups that already existed when you created
/// the policy keep their own retention period."
pub const ENFORCEMENT_MODE_NEW_ONLY: &str = "NEW_ONLY";

/// The `enforcementMode` that applies a retention policy "to every log group that matches
/// the selection criteria, including log groups that existed before you created the
/// policy".
pub const ENFORCEMENT_MODE_RETROACTIVE: &str = "RETROACTIVE";

/// The `enforcementMode` values a retention policy document may state, from
/// `PutAccountPolicy$policyDocument`: "Valid values are NEW_ONLY and RETROACTIVE."
pub const ENFORCEMENT_MODE_VALUES: [&str; 2] =
    [ENFORCEMENT_MODE_NEW_ONLY, ENFORCEMENT_MODE_RETROACTIVE];

/// How long after a put the retention an account-level retention policy states takes
/// effect — until then the retention that was in effect before the put stays in effect, as
/// `PutAccountPolicy` describes ("the previous retention period stays in effect until
/// then. During this period, DescribeLogGroups and log event expiration use the retention
/// period that is currently in effect, but a dry run reports the impact as if the policy
/// were already in effect").
///
/// CloudWatch Logs publishes a wait of 24 hours there. This emulator owes that transition
/// at the span every clock-owed transition here takes
/// ([`AGGREGATE_SUMMARY_VISIBILITY_DELAY_MS`]), so a caller watching for the policy to come
/// into effect sees it arrive rather than never settle: the span is never zero, so a caller
/// reading straight after the put is still answered the retention that was in effect then.
pub const ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS: i64 = AGGREGATE_SUMMARY_VISIBILITY_DELAY_MS;

/// How many log groups a dry run reports the impact on one by one, from
/// `DryRunResult$topLogGroupsImpacted`: "Up to 20 log groups in the scope of the policy,
/// sorted by stored bytes in descending order."
pub const MAX_DRY_RUN_IMPACTED_LOG_GROUPS: usize = 20;

/// The log group class a dry run of a retention policy change leaves out of its analysis,
/// from `DryRunResult`: "Log groups in the Delivery class are not included." Spelled as the
/// model's `LogGroupClass` spells it ([`crate::ops::types::LOG_GROUP_CLASS_VALUES`]).
pub const RETENTION_ANALYSIS_EXCLUDED_LOG_GROUP_CLASS: &str = "DELIVERY";

/// The subscription filters one log group can have ATTACHED at any one time. The
/// operation's own documentation still says "Each log group can have up to two
/// subscription filters associated with it", but the service accepts five attached
/// filters on one log group and refuses the one after that, so the quota is five.
///
/// It counts the filters a group HOLDS, not the puts made against it: a put naming a
/// filter the group already holds is an update of that filter, and deleting one frees its
/// slot for the next put.
pub const MAX_SUBSCRIPTION_FILTERS_PER_GROUP: usize = 5;

/// The refusal CloudWatch Logs answers when a request would take the account, or one log
/// group, past the resources AWS DOCUMENTS it may hold at one time: the resource policies an
/// account may hold without a `resourceArn` ([`MAX_ACCOUNT_RESOURCE_POLICIES`]), its log
/// groups ([`MAX_LOG_GROUPS`]) and its single integration ([`MAX_INTEGRATIONS`]), the metric
/// filters ([`MAX_METRIC_FILTERS_PER_GROUP`]) and subscription filters
/// ([`MAX_SUBSCRIPTION_FILTERS_PER_GROUP`]) one group can have attached, and the single
/// export task an account can have active ([`ExportTaskRecord::is_active`]). The text is
/// FIXED for the error class — it names neither the limit that was reached, nor anything
/// from the request, nor the resource holding the slot — so every refusal of that class
/// reads identically.
///
/// A ceiling THIS EMULATOR sets on what it retains, which AWS has no quota for and therefore
/// no sentence for ([`MAX_ANOMALY_DETECTORS`], [`MAX_QUERY_DEFINITIONS`],
/// [`MAX_LOOKUP_TABLES`], [`MAX_LOOKUP_TABLE_BYTES_TOTAL`]), is refused with its own
/// sentence naming that ceiling instead, so a caller can tell the two apart.
pub const LIMIT_EXCEEDED_MESSAGE: &str = "Resource limit exceeded.";

/// The destinations `PutSubscriptionFilter` documents as supported, as the service
/// segment of the `destinationArn` naming each: "An Amazon Kinesis stream", "A logical
/// destination (specified using an ARN)" created with `PutDestination` (a CloudWatch
/// Logs `destination` resource), "A Kinesis Data Firehose delivery stream" and "A Lambda
/// function". The resource prefix is the one each of those services' ARNs carries.
pub const SUBSCRIPTION_DESTINATIONS: [(&str, &str); 4] = [
    ("kinesis", "stream/"),
    ("firehose", "deliverystream/"),
    ("lambda", "function:"),
    ("logs", "destination:"),
];

/// AWS's documented metric-filter quota, from `PutMetricFilter`: "The maximum number of
/// metric filters that can be associated with a log group is 100." A put naming a filter
/// the group already holds is an update of that filter, so it does not count against this.
pub const MAX_METRIC_FILTERS_PER_GROUP: usize = 100;

/// The dimensions one metric filter can declare, from `MetricTransformation$dimensions`:
/// "One metric filter can include as many as three dimensions." The system fields a filter
/// emits as dimensions share the budget — `PutMetricFilter$emitSystemFieldDimensions`:
/// "These dimensions [...] count toward the total dimension limit for metric filters" — so
/// the two are counted together against this.
pub const MAX_METRIC_FILTER_DIMENSIONS: usize = 3;

/// The system fields a metric filter can emit as additional dimensions, from
/// `PutMetricFilter$emitSystemFieldDimensions`: "Valid values are @aws.account and
/// @aws.region." A SHORTER catalogue than a subscription filter's
/// ([`SUBSCRIPTION_SYSTEM_FIELDS`]), which also admits `@source.log`: the two members
/// document two different sets, so they are two constants.
pub const METRIC_FILTER_SYSTEM_FIELDS: [&str; 2] = ["@aws.account", "@aws.region"];

/// The regular expression patterns one `filterPattern` may carry, from
/// `PutMetricFilter`: "there is a quota of two regular expression patterns within a single
/// filter pattern".
pub const MAX_REGEX_PATTERNS_PER_FILTER_PATTERN: usize = 2;

/// The refusal CloudWatch Logs answers a `filterPattern` carrying more regular expression
/// patterns than one may hold. The quota is stated once
/// ([`MAX_REGEX_PATTERNS_PER_FILTER_PATTERN`]) and so is the sentence, so no two operations
/// that take a filter pattern can word the same refusal differently.
pub fn filter_pattern_regex_quota_message() -> String {
    format!(
        "a single filter pattern can contain at most \
         {MAX_REGEX_PATTERNS_PER_FILTER_PATTERN} regular expression patterns"
    )
}

/// The regular expression patterns one log group's metric filters may carry between them,
/// from `PutMetricFilter`: "There is also a quota of five regular expression patterns per
/// log group."
pub const MAX_REGEX_PATTERNS_PER_LOG_GROUP: usize = 5;

/// The prefix a metric filter's dimension value carries, because the value names a FIELD
/// of the log event rather than a literal: the filter-pattern syntax selects a JSON field
/// with `$.path` and a space-delimited field with `$name`. See
/// [`is_log_event_field_reference`].
pub const FIELD_REFERENCE_PREFIX: char = '$';

/// The system fields a subscription filter can emit, from
/// `PutSubscriptionFilter$emitSystemFields`: "Valid values are @aws.account,
/// @aws.region, and @source.log".
pub const SUBSCRIPTION_SYSTEM_FIELDS: [&str; 3] = ["@aws.account", "@aws.region", "@source.log"];

/// The `distribution` a subscription filter carries when the put naming it specified
/// none, from `PutSubscriptionFilter$distribution`: "By default, log data is grouped by
/// log stream, but the grouping can be set to random for a more even distribution." It is
/// one of the model's `Distribution` enum values (`ByLogStream` | `Random`), so a filter
/// always has a distribution to report.
pub const DEFAULT_DISTRIBUTION: &str = "ByLogStream";

/// The query language a query definition is saved with when the put named no
/// `queryLanguage`. `CWLI` is how the model's `QueryLanguage` enum spells Logs Insights
/// QL (`QUERY_LANGUAGE_VALUES`), and `PutQueryDefinition` offers `parameters` —
/// "supported only for Logs Insights QL queries" — on a request that names no language
/// at all, so a definition saved without one is a Logs Insights QL definition. Storing it
/// is what lets `DescribeQueryDefinitions$queryLanguage` ("limits the returned queries to
/// only the queries that use the specified query language") place every definition.
pub const DEFAULT_QUERY_LANGUAGE: &str = "CWLI";

/// The largest page `DescribeQueryDefinitions` hands back, which is also the page a
/// request that names no `maxResults` gets: `maxResults` "limits the number of returned
/// query definitions to the specified number" and documents no default, and the member's
/// own `QueryListMaxResults` range (`range 1..1000`) is the most the API admits in one
/// page.
pub const MAX_QUERY_DEFINITIONS_PER_PAGE: i32 = 1_000;

/// The bracketed forms a filter pattern can open, as `(open, close)` pairs: a JSON
/// selector pattern is wrapped in `{ … }` and a space-delimited pattern in `[ … ]`
/// ("Filter pattern syntax for metric filters, subscription filters, filter log events,
/// and Live Tail", referenced from `PutSubscriptionFilter`). A pattern that opens one of
/// these forms must close it, which is the rule
/// [`validate_filter_pattern`] holds a pattern to.
pub const FILTER_PATTERN_DELIMITERS: [(char, char); 2] = [('{', '}'), ('[', ']')];

/// THE EMULATOR'S OWN CEILING on the log streams one group holds, not an AWS quota:
/// CloudWatch Logs documents no limit on log streams, but this emulator keeps every
/// stream in memory, so it refuses to create more than this many in one group rather
/// than growing without bound on request.
pub const MAX_STREAMS_PER_GROUP: usize = 10_000;

/// THE EMULATOR'S OWN STORAGE CEILING for one log stream, not an AWS quota: the events
/// a stream holds live in memory, so it keeps at most this many of them, and at most
/// [`MAX_STREAM_BYTES`] of message bytes, discarding the OLDEST first when a write would
/// exceed either. That is the effect a caller already observes from the real service
/// when events pass a log group's retention period and are deleted.
pub const MAX_STREAM_EVENTS: usize = 20_000;

/// The byte half of the per-stream storage ceiling. See [`MAX_STREAM_EVENTS`].
pub const MAX_STREAM_BYTES: i64 = 8 * 1024 * 1024;

/// THE EMULATOR'S OWN CEILING on the syslog configurations one log group holds, not an AWS
/// quota: CloudWatch Logs documents no limit on them, but a `vpcEndpointId` is caller-chosen
/// (`^vpce-[0-9a-f]{1,64}$` admits astronomically many) and every configuration a put
/// creates stays resident in this store, so a group refuses to take on more than this many
/// rather than growing without bound on request. A put naming an endpoint the group already
/// holds is an UPDATE of that configuration, so it does not count against this.
pub const MAX_SYSLOG_CONFIGURATIONS_PER_GROUP: usize = 100;

/// An import task that is still running, from the model's `ImportStatus` enum
/// (`IN_PROGRESS` | `CANCELLED` | `COMPLETED` | `FAILED`). This is the only
/// non-terminal one: an import AWS accepts starts here and leaves it on its own.
pub const IMPORT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// An import task a caller cancelled, from the model's `ImportStatus` enum.
pub const IMPORT_STATUS_CANCELLED: &str = "CANCELLED";

/// An import task that finished importing, from the model's `ImportStatus` enum.
pub const IMPORT_STATUS_COMPLETED: &str = "COMPLETED";

/// An import task that stopped on an error, from the model's `ImportStatus` enum. A task
/// in this status reports why in `Import$errorMessage` ("Error message related to any
/// failed imports").
pub const IMPORT_STATUS_FAILED: &str = "FAILED";

/// How long after it is created an import task becomes `COMPLETED`.
///
/// AWS documents no duration for an import ("Starts an import from a data source to
/// CloudWatch Log"), so this span is THE EMULATOR'S OWN CHOICE: the import runs
/// asynchronously, so the task is handed back `IN_PROGRESS` and the clock owes it a
/// terminal status later. It is a WHOLE span of the length this emulator gives a transition
/// the clock owes, not a fraction of a client round-trip, which is what makes the one
/// non-terminal status observable: a caller that creates an import and then reads or cancels
/// it over one or more further API calls still finds it `IN_PROGRESS`, and so still meets the
/// state rules that status carries — chief among them that "there can be no more than 3
/// active imports per account at a given time" ([`MAX_ACTIVE_IMPORT_TASKS`]), a limit a task
/// settling inside one round-trip could never impose. The instant it falls due at is stored
/// on the record and never slept on.
pub const IMPORT_COMPLETION_DELAY_MS: i64 = 3_000;

/// How many import tasks of one account may be running at the same time: `CreateImportTask`
/// documents that "there can be no more than 3 active imports per account at a given time".
pub const MAX_ACTIVE_IMPORT_TASKS: usize = 3;

/// THE EMULATOR'S OWN CEILING on the import tasks one account's store holds, not a quota
/// cited from AWS: a task stays in memory for as long as the process runs and nothing expires
/// one, so the oldest finished tasks are dropped once the store holds this many rather than
/// growing without bound on request.
pub const MAX_IMPORT_TASKS: usize = 1_000;

/// The refusal `CreateImportTask` answers when the source it is given is not in a state an
/// import can read from: the operation documents the constraint as a sentence of its own —
/// "the specified source must be in an ACTIVE state" — and the text is fixed for the
/// condition, carrying neither the source ARN nor the state it is actually in.
pub const IMPORT_SOURCE_NOT_ACTIVE_MESSAGE: &str =
    "The specified source must be in an ACTIVE state.";

/// The refusal CloudWatch Logs answers when a request hands it a role of ANOTHER account to
/// act under: a role is passed to this service by the caller that holds it
/// (`CreateImportTask`'s own documentation states the caller must "have permissions to [...]
/// perform iam:PassRole on the provided import role"), and a principal of one account cannot
/// pass a role of another, so the request is a permanent defect in the value it carried.
///
/// Fixed text for the whole class: it names neither the member, the role nor the account, so
/// nothing of the request is interpolated.
pub const CROSS_ACCOUNT_ROLE_MESSAGE: &str = "Cross-account role passing is not allowed.";

/// That refusal for the role ARN `role_arn`, or `None` where the role may be passed: a role of
/// the caller's own account, and an ARN naming no account at all — which names no role either
/// and is refused for the form it is in instead
/// ([`super::unparsable_arn_message`]).
pub fn cross_account_role_refusal(role_arn: &str) -> Option<&'static str> {
    let account = super::arn_account_id(role_arn)?;
    (account != ::engine::emulator_account_id()).then_some(CROSS_ACCOUNT_ROLE_MESSAGE)
}

/// The refusal `CreateImportTask` answers when the `importSourceArn` it is given is in no
/// form a CloudTrail Lake event data store is named by: an ARN of another resource kind or
/// another service, a value that is not an ARN at all, or an event data store ARN whose
/// resource suffix is no identifier that service mints.
///
/// CloudWatch Logs reads that member as an event data store ARN and states what it could not
/// make of the value handed to it, so the refusal is about the REQUEST's parameter: the text
/// is fixed for the condition and the one value it interpolates is the caller's own
/// `importSourceArn`, echoed verbatim, so an operator can see which ARN was rejected. A
/// WELL-FORMED ARN no store answers to is a missing resource rather than a bad value, and is
/// refused as one ([`EVENT_DATA_STORE_RESOURCE_KIND`]).
pub fn invalid_event_data_store_arn_message(source_arn: &str) -> String {
    format!("Invalid Event Data Store ARN: {source_arn}")
}

/// The resource kind `CreateImportTask` names in the missing-resource refusal it answers when
/// the well-formed `importSourceArn` it is given resolves to no event data store the owning
/// service holds ([`super::resource_not_found_message`]) — the resource the import would have
/// read its events from, which is what the caller has to create for the import to start.
pub const EVENT_DATA_STORE_RESOURCE_KIND: &str = "event data store";

/// The refusal `CreateImportTask` answers when the `importFilter` it is given bounds the
/// event time at both ends in the wrong order: the operation documents the constraint as a
/// sentence of its own — "the startEventTime must be less than or equal to endEventTime" —
/// and the text is fixed for the condition, carrying neither bound.
pub const IMPORT_EVENT_TIME_RANGE_MESSAGE: &str =
    "The startEventTime must be less than or equal to endEventTime.";

/// The refusal `CreateImportTask` answers when the account already has
/// [`MAX_ACTIVE_IMPORT_TASKS`] imports running: the quota that refused it, so a caller can
/// tell this limit from a request it could fix. The slots are the ACCOUNT's, so the text
/// carries nothing of the request or of the tasks holding them. It is refused with the only
/// quota-shaped error the operation models (`ThrottlingException`, "The request was throttled
/// because of quota limits").
pub fn active_import_tasks_limit_message() -> String {
    format!("the maximum of {MAX_ACTIVE_IMPORT_TASKS} active import(s) per account has been reached")
}

/// The refusal `DescribeImportTasks` answers when the `importId` it is given names no import
/// task of this account: the service states that nothing it holds matched, rather than naming
/// the resource kind the way [`resource_not_found_message`] does, and the text is fixed for
/// the condition — it carries not even the id it was handed.
pub const NO_MATCHING_IMPORT_MESSAGE: &str = "No matching import found.";

/// The refusal an operation answers when the `importId` it is given is not written in the
/// form an import id has at all.
///
/// An import id is a UUID ([`super::mint_import_task_id`]), and the modelled `ImportId`
/// pattern (`^[\-a-zA-Z0-9]+$`) admits far more than that: a value inside the pattern but
/// outside the form names no import that could ever exist, which is the incorrectly specified
/// parameter `InvalidParameterException` reports rather than the absent resource
/// [`NO_MATCHING_IMPORT_MESSAGE`] reports. Fixed text for the condition, naming the member at
/// fault and quoting nothing of the value it rejected.
pub const IMPORT_ID_FORM_MESSAGE: &str = "ImportId must be a valid UUID format";

/// The refusal `CancelImportTask` answers when the import task it names has already settled:
/// the operation "cancels an active import task", and a task in any status but
/// [`IMPORT_STATUS_IN_PROGRESS`] has no import left to stop, so the call is not valid on that
/// resource. Fixed text for the condition, carrying neither the id it was handed nor the
/// status the task settled at.
pub const IMPORT_TASK_NOT_ACTIVE_MESSAGE: &str = "The specified import task is not active";

/// The name prefix of the managed log group an import task delivers into, which is also the
/// one `CreateImportTask`'s own IAM example documents for an import destination
/// ("arn:aws:logs:us-east-1:123456789012:log-group:/aws/cloudtrail/*").
pub const IMPORT_DESTINATION_LOG_GROUP_PREFIX: &str = "/aws/cloudtrail/";

/// The name of the managed log group `CreateImportTask` creates "as the destination for the
/// imported data", for an import whose source carries the store id `source_id`.
///
/// The group is named after the EVENT DATA STORE the events come from, which is what the
/// operation's IAM example pairs with it: the import destination it grants
/// `log-group:/aws/cloudtrail/<id>:*` on carries the same id as the
/// `eventdatastore/<id>` it grants `cloudtrail:GetEventDataStoreData` on.
pub fn import_destination_log_group_name(source_id: &str) -> String {
    format!("{IMPORT_DESTINATION_LOG_GROUP_PREFIX}{source_id}")
}

/// An export task that has been accepted but has not started moving data yet, from the
/// model's `ExportTaskStatusCode` enum (`CANCELLED` | `COMPLETED` | `FAILED` | `PENDING` |
/// `PENDING_CANCEL` | `RUNNING`). `CreateExportTask` "is an asynchronous call. If all the
/// required information is provided, this operation initiates an export task and responds
/// with the ID of the task", so an accepted task starts here.
pub const EXPORT_TASK_STATUS_PENDING: &str = "PENDING";

/// An export task that is moving data, from the model's `ExportTaskStatusCode` enum. One of
/// the two statuses `CreateExportTask` calls active ("Each account can only have one active
/// (RUNNING or PENDING) export task at a time").
pub const EXPORT_TASK_STATUS_RUNNING: &str = "RUNNING";

/// An export task that finished exporting, from the model's `ExportTaskStatusCode` enum.
pub const EXPORT_TASK_STATUS_COMPLETED: &str = "COMPLETED";

/// An export task a caller cancelled, from the model's `ExportTaskStatusCode` enum.
pub const EXPORT_TASK_STATUS_CANCELLED: &str = "CANCELLED";

/// An export task whose export stopped on an error, from the model's `ExportTaskStatusCode`
/// enum. Terminal like [`EXPORT_TASK_STATUS_COMPLETED`]: the task has finished, so nothing
/// moves it on and a cancel of it has no execution left to stop.
pub const EXPORT_TASK_STATUS_FAILED: &str = "FAILED";

/// The status message a cancelled export task reports beside that status code
/// ([`ExportTaskRecord::status_message`]): `ExportTaskStatus$message` is "the status message
/// related to the status code", and what there is to say about a task in this status is that
/// it stopped because a caller cancelled it rather than because the export itself ended. The
/// text is fixed for the status: it interpolates neither the task nor the instant.
pub const EXPORT_TASK_CANCELLED_STATUS_MESSAGE: &str = "Export task cancelled by user";

/// The status message an export task that finished exporting reports beside that status code,
/// on the same terms as [`EXPORT_TASK_CANCELLED_STATUS_MESSAGE`]: what there is to say about a
/// task in this status is that the export ran to its end without an error. Fixed text for the
/// status, interpolating nothing of the task.
pub const EXPORT_TASK_COMPLETED_STATUS_MESSAGE: &str = "Completed successfully";

/// How long after it is created an export task moves `PENDING` -> `RUNNING`, and how long
/// after that it reaches `COMPLETED`.
///
/// AWS documents no duration for an export, only that the call "is an asynchronous call",
/// so these spans are THE EMULATOR'S OWN CHOICE. Each is a WHOLE span of the length this
/// emulator gives a transition the clock owes, not a fraction of a client round-trip, which
/// is what makes the statuses the model calls active observable: a caller that creates an
/// export and then reads, cancels or polls it over one or more further API calls still finds
/// it `PENDING` and then `RUNNING`, and so still meets the state rules those statuses carry —
/// chief among them that "each account can only have one active (RUNNING or PENDING) export
/// task at a time", a limit a task settling inside one round-trip could never impose. The
/// instants both fall due at are stored on the record and never slept on.
pub const EXPORT_TASK_RUNNING_DELAY_MS: i64 = 3_000;

/// See [`EXPORT_TASK_RUNNING_DELAY_MS`]: the further span a `RUNNING` export task takes to
/// reach `COMPLETED`. An export finishes moving data only after it has started moving it, so
/// this span follows that one instead of overlapping it, and a task is therefore active for
/// both of them together.
pub const EXPORT_TASK_COMPLETION_DELAY_MS: i64 = 3_000;

/// The `destinationPrefix` an export task gets when the create names none: "If you don't
/// specify a value, the default is exportedlogs" (`CreateExportTask$destinationPrefix`).
pub const DEFAULT_EXPORT_DESTINATION_PREFIX: &str = "exportedlogs";

/// THE EMULATOR'S OWN CEILING on the export tasks one account's store holds, not a quota
/// cited from AWS: a task stays in memory for as long as the process runs and nothing
/// expires one, so the oldest completed tasks are dropped once the store holds this many
/// rather than growing without bound on request.
pub const MAX_EXPORT_TASKS: usize = 1_000;

/// The status a log anomaly detector is CREATED in, from the model's
/// `AnomalyDetectorStatus` enum: a detector starts out learning the log group's patterns —
/// `CreateLogAnomalyDetector` documents that it "uses the evaluationFrequency to compare
/// current log events and patterns with trained baselines", so training precedes analysis —
/// and a create therefore hands back a detector that is not analyzing anything yet.
pub const ANOMALY_DETECTOR_STATUS_TRAINING: &str = "TRAINING";

/// The status a trained log anomaly detector settles in: it is scanning log events for
/// anomalies. Nothing but an `UpdateLogAnomalyDetector` (which can pause it) or a delete
/// moves a detector out of this status, so the clock owes it nothing further.
pub const ANOMALY_DETECTOR_STATUS_ANALYZING: &str = "ANALYZING";

/// The status a log anomaly detector a call has PAUSED sits in, from the model's
/// `AnomalyDetectorStatus` enum: `UpdateLogAnomalyDetector$enabled` is used "to pause or
/// restart the anomaly detector", so a detector paused that way is not initializing,
/// training or analyzing, and only another call restarts it — the clock owes it nothing.
pub const ANOMALY_DETECTOR_STATUS_PAUSED: &str = "PAUSED";

/// How long after it is created a log anomaly detector takes to leave
/// [`ANOMALY_DETECTOR_STATUS_TRAINING`] for [`ANOMALY_DETECTOR_STATUS_ANALYZING`].
///
/// AWS documents no duration for this — the detector trains "using machine learning
/// algorithms" over however long its log group needs — so this span is THE EMULATOR'S OWN
/// CHOICE. It is deliberately not zero: a caller that reads the detector straight after
/// creating it still observes the status the create handed back.
pub const ANOMALY_DETECTOR_ANALYSIS_DELAY_MS: i64 = 3_000;

/// The `evaluationFrequency` a log anomaly detector runs with when no call named one — "how
/// often the anomaly detector is to run and look for anomalies"
/// (`CreateLogAnomalyDetector$evaluationFrequency`), one value of the model's
/// `EvaluationFrequency` enum. The member is optional on the create and the service runs the
/// detector on a schedule regardless, so a detector created without one carries the default
/// the service fills in rather than no value at all.
pub const DEFAULT_EVALUATION_FREQUENCY: &str = "FIVE_MIN";

/// The `anomalyVisibilityTime` a log anomaly detector runs with when no call named one, in
/// days — "The number of days to have visibility on an anomaly"
/// (`CreateLogAnomalyDetector$anomalyVisibilityTime`). The member is optional on the create
/// and anomalies are baselined after some span regardless, so a detector created without one
/// carries the default the service fills in; it is the low end of the member's modelled
/// `@range(min: 7, max: 90)`.
pub const DEFAULT_ANOMALY_VISIBILITY_TIME_DAYS: i64 = 7;

/// `INFINITE`, one of the two values the model's `SuppressionType` enum carries: a
/// suppression that does not run out — "If you specify INFINITE, any value for
/// suppressionPeriod is ignored." (`UpdateAnomaly$suppressionType`)
pub const SUPPRESSION_TYPE_INFINITE: &str = "INFINITE";

/// `Active`, one value of the model's `State` enum: the state an anomaly nothing has
/// suppressed or baselined reports — "Indicates the current state of this anomaly. If it is
/// still being treated as an anomaly, the value is `Active`." (`Anomaly$state`)
pub const ANOMALY_STATE_ACTIVE: &str = "Active";

/// `Suppressed`, one value of the model's `State` enum: the state an anomaly a suppression
/// is in force over reports — "If you have suppressed this anomaly by using the
/// UpdateAnomaly operation, the value is `Suppressed`." (`Anomaly$state`)
pub const ANOMALY_STATE_SUPPRESSED: &str = "Suppressed";

/// `Baseline`, one value of the model's `State` enum: the state an anomaly whose behavior
/// has been accepted as normal reports — "If this behavior is now considered to be normal,
/// the value is `Baseline`." (`Anomaly$state`), which is what `UpdateAnomaly$baseline` says
/// of the suppression it places: "Set this to true to prevent CloudWatch Logs from
/// displaying this behavior as an anomaly in the future. The behavior is then treated as
/// baseline behavior."
pub const ANOMALY_STATE_BASELINE: &str = "Baseline";

/// How many log groups one log anomaly detector watches, from
/// `CreateLogAnomalyDetector$logGroupArnList`: "An array containing the ARN of the log
/// group that this anomaly detector will watch. You can specify only one log group ARN."
pub const ANOMALY_DETECTOR_LOG_GROUPS: usize = 1;

/// How the service NAMES the log groups a detector is created over in a refusal about them:
/// the input `CreateLogAnomalyDetector$logGroupArnList` carries, stated the way AWS states
/// it when the request provided none of it — one ARN or several, hence the plural in
/// parentheses.
pub const ANOMALY_DETECTOR_LOG_GROUP_INPUT: &str = "LogGroupArn(s)";

/// The refusal `CreateLogAnomalyDetector` answers a request listing more log groups than one
/// detector watches ([`ANOMALY_DETECTOR_LOG_GROUPS`]): the restriction
/// `CreateLogAnomalyDetector$logGroupArnList` documents in its own words — "You can specify
/// only one log group ARN." — stated for the class, so nothing of the request is
/// interpolated.
pub const ANOMALY_DETECTOR_ONE_LOG_GROUP_MESSAGE: &str = "You can specify only one log group ARN.";

/// THE EMULATOR'S OWN CEILING on the log anomaly detectors one account holds, not a quota
/// cited from AWS: every accepted create mints a fresh ARN and keeps the detector's
/// configuration in memory for as long as the process runs, and nothing expires one, so
/// `CreateLogAnomalyDetector` refuses to create more than this many with the
/// `LimitExceededException` it models ("You have reached the maximum number of resources
/// that can be created") rather than growing without bound on request. It is set far above
/// the number of detectors a caller legitimately creates, so only a caller hammering the
/// create reaches it.
pub const MAX_ANOMALY_DETECTORS: usize = 1_000;

/// THE EMULATOR'S OWN CEILING on the saved query definitions one account holds, not a
/// quota cited from AWS: each definition keeps its query string (modelled `@length` up to
/// 10,000), name and parameters in memory for as long as the process runs and nothing
/// expires one, so `PutQueryDefinition` refuses to create more than this many with the
/// `LimitExceededException` it models ("You have reached the maximum number of resources
/// that can be created") rather than growing without bound on request. An UPDATE of a
/// definition already stored is never refused by it: it adds nothing.
pub const MAX_QUERY_DEFINITIONS: usize = 1_000;

/// THE EMULATOR'S OWN CEILING on the lookup tables one account holds, not a quota cited
/// from AWS: each table keeps its whole CSV body (modelled `@length` up to 10 MB) in memory
/// for as long as the process runs and nothing expires one, so `CreateLookupTable` refuses
/// to create more than this many with the `LimitExceededException` it models ("You have
/// reached the maximum number of resources that can be created") rather than growing
/// without bound on request. It is set far above the number of tables a caller legitimately
/// creates, so only a caller hammering the create reaches it.
pub const MAX_LOOKUP_TABLES: usize = 1_000;

/// How many BYTES of lookup-table CSV content this emulator will hold across every table of
/// the account.
///
/// [`MAX_LOOKUP_TABLES`] alone would leave total memory bounded only by its product with
/// the modelled `TableBody` length (10 MB), so this is what actually bounds the process:
/// whatever a caller does — one table or a thousand — the lookup-table store holds at most
/// this many bytes of caller-chosen content. Not a fidelity claim, and the same order as
/// the service-wide retained budgets of this workspace's peer services (s3's
/// `MAX_OBJECT_BYTES_TOTAL`, appconfig's `MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`,
/// codeguruprofiler's `MAX_RETAINED_PROFILE_BYTES`). A create that would take the store past
/// it is refused with the `LimitExceededException` `CreateLookupTable` models, BEFORE the
/// body is retained, rather than recorded.
pub const MAX_LOOKUP_TABLE_BYTES_TOTAL: usize = 64 * 1024 * 1024;

/// The only integration type CloudWatch Logs supports, from the model's `IntegrationType`
/// enum (`OPENSEARCH`), which `PutIntegration` states as well: "Currently, only
/// integrations with OpenSearch Service are supported".
pub const INTEGRATION_TYPE_OPENSEARCH: &str = "OPENSEARCH";

/// The status an integration is created in, from the model's `IntegrationStatus` enum:
/// "After you create an integration, it takes a few minutes to complete. During this time,
/// you'll see the status as PROVISIONING" (`PutIntegrationResponse$integrationStatus`).
pub const INTEGRATION_STATUS_PROVISIONING: &str = "PROVISIONING";

/// The status an integration settles in once its OpenSearch Service resources are
/// provisioned, from the model's `IntegrationStatus` enum. Nothing but a delete moves an
/// integration out of it, so the clock owes it nothing further.
pub const INTEGRATION_STATUS_ACTIVE: &str = "ACTIVE";

/// The status an integration whose provisioning failed carries, from the model's
/// `IntegrationStatus` enum. Nothing in this emulator puts an integration here: a
/// `PutIntegration` it cannot accept is refused outright, and one it accepts provisions.
pub const INTEGRATION_STATUS_FAILED: &str = "FAILED";

/// How long after it is created an integration takes to leave
/// [`INTEGRATION_STATUS_PROVISIONING`] for [`INTEGRATION_STATUS_ACTIVE`].
///
/// AWS states only that it "takes a few minutes to complete", so the span is THE
/// EMULATOR'S OWN CHOICE: the provisioning runs asynchronously, so the create hands back a
/// PROVISIONING integration and the clock owes it ACTIVE later. It is deliberately not
/// zero — a caller that reads, lists or deletes the integration straight after creating it
/// still observes PROVISIONING, and the rules that go with that status.
pub const INTEGRATION_PROVISIONING_DELAY_MS: i64 = 500;

/// How many integrations one account may hold, from `PutIntegration`: "currently you can
/// have only one integration in your account", which `ListIntegrations` repeats
/// ("Currently, only one integration can be created in an account").
pub const MAX_INTEGRATIONS: usize = 1;

/// The status a data-source association of an S3 Table Integration is in while it gives
/// query access, from the model's `S3TableIntegrationSourceStatus` enum (`ACTIVE` |
/// `UNHEALTHY` | `FAILED` | `DATA_SOURCE_DELETE_IN_PROGRESS`).
///
/// `AssociateSourceToS3TableIntegration` "enables querying log data using analytics
/// engines that support Iceberg" and the enum holds no status for an association still
/// being made, so an accepted associate stores one already in this status.
pub const S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE: &str = "ACTIVE";

/// The status a data-source association carries once it has been disassociated, from the
/// model's `S3TableIntegrationSourceStatus` enum.
///
/// `DisassociateSourceFromS3TableIntegration` is "removing query access and deleting all
/// associated data from the integration": the deletion of that data is work the call leaves
/// running, which is what this status reports while it runs.
pub const S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS: &str =
    "DATA_SOURCE_DELETE_IN_PROGRESS";

/// How long after it is disassociated a data-source association takes to be gone — the
/// span it is reported in [`S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS`] for.
///
/// AWS documents no duration for deleting the associated data, so the span is THE
/// EMULATOR'S OWN CHOICE. It is deliberately not zero: the model carries a status for an
/// association whose data is still being deleted, so a caller that lists the integration's
/// sources straight after disassociating one still observes it in that status.
pub const S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS: i64 = 3_000;

/// The most data-source associations of S3 Table Integrations this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: CloudWatch Logs publishes no quota for the
/// data sources an S3 Table Integration may have, and nothing in the request bounds how
/// many a caller makes — the integration they are made against is named by an ARN rather
/// than by a resource of this store, so no other cap dominates this one. An associate past
/// it is refused with the only quota-shaped error the operation models
/// (`ThrottlingException`, "The request was throttled because of quota limits") BEFORE a
/// record is allocated, rather than retained.
pub const MAX_S3_TABLE_INTEGRATION_SOURCES: usize = 500;

/// The refusal an associate past [`MAX_S3_TABLE_INTEGRATION_SOURCES`] carries: the cap that
/// refused it, so a caller can tell this guard from a request it could fix.
pub fn s3_table_integration_sources_cap_message() -> String {
    format!(
        "the maximum of {MAX_S3_TABLE_INTEGRATION_SOURCES} data source association(s) has been \
         reached"
    )
}

/// An OpenSearch Service resource of an integration that is there, from the model's
/// `OpenSearchResourceStatusType` enum (`ACTIVE` | `NOT_FOUND` | `ERROR`).
pub const OPENSEARCH_RESOURCE_STATUS_ACTIVE: &str = "ACTIVE";

/// An OpenSearch Service resource of an integration that is not there yet, from the
/// model's `OpenSearchResourceStatusType` enum.
pub const OPENSEARCH_RESOURCE_STATUS_NOT_FOUND: &str = "NOT_FOUND";

/// An OpenSearch Service resource of an integration that failed, from the model's
/// `OpenSearchResourceStatusType` enum.
pub const OPENSEARCH_RESOURCE_STATUS_ERROR: &str = "ERROR";

/// The ARN service segment of IAM, the service whose roles and users
/// `OpenSearchResourceConfig$dataSourceRoleArn` and `$dashboardViewerPrincipals` name.
pub(super) const IAM_SERVICE: &str = "iam";

/// The ARN service segment of KMS, the service whose keys
/// `OpenSearchResourceConfig$kmsKeyArn` names.
pub(super) const KMS_SERVICE: &str = "kms";

/// The ARN service segment of OpenSearch Service, whose applications
/// `OpenSearchResourceConfig$applicationArn` names and whose application an integration's
/// setup creates.
pub(super) const OPENSEARCH_SERVICE: &str = "opensearch";

/// The ARN service segment of OpenSearch Serverless, which owns the collection an
/// integration's setup creates (`OpenSearchCollection$collectionArn`).
pub(super) const OPENSEARCH_SERVERLESS_SERVICE: &str = "aoss";

/// The suffixes the OpenSearch Service resources an integration's setup creates are named
/// with, each appended to the integration's own name so one integration's resources are
/// distinguishable from another's and stable across reads. They are the emulator's own
/// naming: AWS documents that the setup creates these resources but not what it calls
/// them. Every suffix uses only characters the resource-name patterns admit
/// (`^[\.\-_/#A-Za-z0-9]+$`).
pub(super) const APPLICATION_SUFFIX: &str = "-application";
/// See [`APPLICATION_SUFFIX`].
pub(super) const COLLECTION_SUFFIX: &str = "-collection";
/// See [`APPLICATION_SUFFIX`].
pub(super) const WORKSPACE_SUFFIX: &str = "-workspace";
/// See [`APPLICATION_SUFFIX`].
pub(super) const ENCRYPTION_POLICY_SUFFIX: &str = "-encryption-policy";
/// See [`APPLICATION_SUFFIX`].
pub(super) const NETWORK_POLICY_SUFFIX: &str = "-network-policy";
/// See [`APPLICATION_SUFFIX`].
pub(super) const ACCESS_POLICY_SUFFIX: &str = "-access-policy";
/// See [`APPLICATION_SUFFIX`].
pub(super) const LIFECYCLE_POLICY_SUFFIX: &str = "-lifecycle-policy";
