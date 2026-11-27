// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice service.
//!
//! This module owns everything the handlers share: the stored form of an
//! automation rule, of an automation event and of a recommended action, the
//! enrollment configuration and the service-wide enrollment pre-check every
//! action goes through, the client-token (idempotency) ledger, and the pure
//! helpers that mint a rule id, build an opaque page token, size one page of a
//! paginated listing, bound a queried time range and decide whether a rule, an
//! event or a recommended action matches a list filter. Handlers live in `src/ops/`, one file per operation, and build
//! their own output shapes from what they read back out of here.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
// The identity a request speaks for is read from the SAME store
// `sts:GetCallerIdentity` answers from, so this service and that read report one
// principal for one credential. See [`caller_identity_arn`].
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    AccountInfo, AutomationEvent, AutomationEventFilter, AutomationEventStep, AutomationRule,
    Criteria,
    DoubleCriteriaCondition, EbsVolume, EbsVolumeConfiguration, EstimatedMonthlySavings, Filter,
    IntegerCriteriaCondition,
    OptInRequiredException, OrganizationConfiguration, OrganizationScope, PreviewResult,
    RecommendedAction, RecommendedActionFilter, ResourceDetails, ResourceTagsCriteriaCondition,
    Schedule, StringCriteriaCondition, Tag, COMPARISON_OPERATOR_VALUES, ENROLLMENT_STATUS_VALUES,
    ORGANIZATION_ENROLLMENT_CONFIGURATION_TYPE_VALUES, ORGANIZATION_RULE_MODE_VALUES,
    ORGANIZATION_TARGET_TYPE_VALUES,
    RECOMMENDED_ACTION_TYPE_VALUES, RULE_APPLY_ORDER_VALUES,
    RULE_STATUS_VALUES, RULE_TYPE_VALUES, SAVINGS_ESTIMATION_MODE_VALUES,
};

// The organization enrollment configurations this service stores, the
// organization hierarchy they are resolved through and the effective
// organization rule mode that resolution produces: one concern, so one file.
// Re-exported, so a handler reaches it under `crate::state::…` exactly like
// everything else here.
mod organization_enrollment;
pub use organization_enrollment::*;

/// The ARN namespace this service stamps on a rule ARN, from the model's
/// `aws.api#service` trait (`arnNamespace: compute-optimizer`). The `RuleArn`
/// pattern (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`)
/// has an EMPTY region segment, so [`rule_arn`] passes an empty region.
const ARN_NAMESPACE: &str = "compute-optimizer";

/// The revision a freshly created rule carries. `ruleRevision` is the service's
/// optimistic-concurrency token and is documented as "incremented each time the
/// rule is updated", so the first stored revision is 1 and the first successful
/// update makes it 2.
pub const INITIAL_RULE_REVISION: i64 = 1;

/// The enrollment status an account that has never called
/// `UpdateEnrollmentConfiguration` reports: it is not enrolled.
pub const DEFAULT_ENROLLMENT_STATUS: &str = "Inactive";

/// The one `EnrollmentStatus` that means the account is OPTED IN to Compute
/// Optimizer Automation.
///
/// `EnrollmentStatus` is `Active | Inactive | Pending | Failed`
/// (`smithy_query(query="shape",shape="EnrollmentStatus")`) and the `status`
/// member of `UpdateEnrollmentConfigurationRequest` documents what the two
/// settable ones mean: "Active - Enables the Automation feature for your
/// account" and "Inactive - Disables the Automation feature for your account
/// and stops all of your automation rules". `Pending` (an opt-in the service is
/// still processing) and `Failed` (one that did not complete) are the
/// server-only statuses, and neither has the feature enabled either — so
/// `Active` is the only status under which the account may perform this
/// service's actions.
pub const ENROLLED_STATUS: &str = "Active";

/// The `OrganizationRuleMode` an ASSOCIATED member account carries.
///
/// `OrganizationRuleMode` is `AnyAllowed | NoneAllowed`
/// (`smithy_query(query="shape",shape="OrganizationRuleMode")`) and it
/// "Specifies whether the management account can create Automation rules that
/// implement optimization actions for this account"
/// (`smithy_query(query="doc",shape="AccountInfo",member="organizationRuleMode")`).
/// `AssociateAccounts` documents which of the two an association leaves the
/// account in: "When you associate a member account, its organization rule mode
/// is automatically set to \"Any allowed,\" which permits the management account
/// to create Automation rules that automatically apply actions to that account."
pub const ORGANIZATION_RULE_MODE_ANY_ALLOWED: &str = "AnyAllowed";

/// The other `OrganizationRuleMode`: the management account may NOT create
/// Automation rules that apply actions to the account.
///
/// It is what an account that carries no association reports, because an
/// association is exactly what grants that permission
/// (`AssociateAccounts`: "Once associated, the management account (or a
/// delegated administrator) can apply recommended actions to the member
/// account"), and `AccountInfo$organizationRuleMode` is a REQUIRED member, so a
/// listing has to name one of the two values for every account it reports.
pub const ORGANIZATION_RULE_MODE_NONE_ALLOWED: &str = "NoneAllowed";

/// How many account ids one organization-association request may name.
///
/// DOCUMENTED, not chosen: `accountIds` reads "The IDs of the member accounts to
/// associate. You can specify up to 50 account IDs."
/// (`smithy_query(query="doc",shape="AssociateAccountsRequest",member="accountIds")`).
/// The `AccountIdList` shape carries no `@length`, so the protocol boundary
/// admits a longer list and the bound is enforced by the handler.
pub const MAX_ACCOUNT_IDS_PER_REQUEST: usize = 50;

/// How many accounts the enrollment store holds before an association that would
/// bring in a new one is refused.
///
/// CHOSEN bound, for the same reason [`MAX_RULES_PER_ACCOUNT`] and
/// [`MAX_STORED_EVENTS`] are: the organization's real account quota is in neither
/// the model nor the operation docs, and this store lives in memory while
/// `AssociateAccounts` takes the account ids it stores FROM THE REQUEST (any id
/// matching `AccountId`'s `^[0-9]{12}$`). So the number of accounts the store
/// holds is capped rather than left to grow with every distinct id a caller
/// names: a request that would push it past the cap is refused and stores
/// NOTHING. [`MAX_ACCOUNT_IDS_PER_REQUEST`] does not bound it on its own — it
/// only caps ONE request — and an entry already in the store is re-associated
/// freely, because that adds nothing.
pub const MAX_ENROLLED_ACCOUNTS: usize = 1_000;

/// The page size a paginated list uses when the caller sends no `maxResults`.
///
/// Most of this service's `maxResults` members are bounded at
/// `range(min: 1, max: 100)` (`ListAutomationRules`,
/// `ListAutomationRulePreview`, …), so for those the largest page a caller may
/// ask for is also the default; `ListAccounts` is bounded at
/// `range(min: 1, max: 1000)`
/// (`smithy_query(query="operation",operation="ListAccounts")`), so there a
/// caller may ask for a larger page than the default. Reach it through
/// [`page_size`], which is where the default is taken.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The number of characters in a minted id. `RuleId`, `EventId` and
/// `RecommendedActionId` all share the pattern `^[0-9A-Za-z]{16}$`, so one
/// length serves all three
/// (`smithy_query(query="shape",shape="RuleId,EventId,RecommendedActionId")`).
const MINTED_ID_LEN: usize = 16;

/// The status an automation event carries while its automation is running, one
/// of the model's `EventStatus` values
/// (`smithy_query(query="shape",shape="EventStatus")`). `StartAutomationEvent`
/// accepted the request, so the event it mints is not yet terminal.
pub const EVENT_STATUS_IN_PROGRESS: &str = "InProgress";

/// The terminal `EventStatus` of an automation that was carried out: the
/// modelled success status every started automation reaches once its run is
/// over (`smithy_query(query="shape",shape="EventStatus")`). It is the status
/// `RollbackAutomationEvent` names as its subject — "Initiates a rollback for a
/// completed automation event".
pub const EVENT_STATUS_COMPLETE: &str = "Complete";

/// The modelled `EventStatus` an automation event that did not complete carries.
/// Reserved for an automation that could NOT be carried out, and it is the only
/// status that comes with an `eventStatusReason`. No path of this service
/// produces one yet — a run it accepts is carried through — so this is here as
/// the name of that status for the path that first refuses a run mid-flight
/// (a rollback that cannot be performed, say), not as a value any reader
/// currently reports.
pub const EVENT_STATUS_FAILED: &str = "Failed";

/// The `EventStatus` an automation event carries while the rollback of its
/// automation is running (`smithy_query(query="shape",shape="EventStatus")`).
/// `RollbackAutomationEvent` "Initiates a rollback", so the event the call
/// answers with is not yet terminal.
pub const EVENT_STATUS_ROLLBACK_IN_PROGRESS: &str = "RollbackInProgress";

/// The terminal `EventStatus` of a rollback that was carried out: the modelled
/// success status a rolled-back event reaches once its rollback run is over
/// (`smithy_query(query="shape",shape="EventStatus")`).
pub const EVENT_STATUS_ROLLBACK_COMPLETE: &str = "RollbackComplete";

/// The `StepStatus` of a step that has not started: it is queued behind the
/// steps ahead of it in its event's sequence
/// (`smithy_query(query="shape",shape="StepStatus")`).
pub const STEP_STATUS_READY: &str = "Ready";

/// The `StepStatus` of the step its event is currently on.
pub const STEP_STATUS_IN_PROGRESS: &str = "InProgress";

/// The `StepStatus` of a step that ran to the end.
pub const STEP_STATUS_COMPLETE: &str = "Complete";

/// The `StepStatus` of a step that did not complete, for the same reason
/// [`EVENT_STATUS_FAILED`] is here: the name of the status a step takes when its
/// run could not be carried out, which no path of this service produces yet.
pub const STEP_STATUS_FAILED: &str = "Failed";

/// The `EventType` (and `RecommendedActionType`) that snapshots an unattached
/// EBS volume and then deletes it.
pub const EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME: &str =
    "SnapshotAndDeleteUnattachedEbsVolume";

/// The `EventType` (and `RecommendedActionType`) that changes an EBS volume's
/// type.
pub const EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE: &str = "UpgradeEbsVolumeType";

/// The `StepType` that takes the snapshot of a volume.
pub const STEP_TYPE_CREATE_EBS_SNAPSHOT: &str = "CreateEbsSnapshot";

/// The `StepType` that deletes a volume.
pub const STEP_TYPE_DELETE_EBS_VOLUME: &str = "DeleteEbsVolume";

/// The `StepType` that changes a volume in place (its type, for an upgrade).
pub const STEP_TYPE_MODIFY_EBS_VOLUME: &str = "ModifyEbsVolume";

/// The `StepType` that creates a volume, which is how the rollback of a deleted
/// volume restores it from the snapshot the forward run took.
pub const STEP_TYPE_CREATE_EBS_VOLUME: &str = "CreateEbsVolume";

/// How long after the start call an automation event's terminal status is due,
/// and so the window the event's run occupies: its steps run back to back
/// inside it (see [`StoredEvent::settle`]).
///
/// CHOSEN span: neither the model nor the operation docs give how long an
/// automation takes. It is non-zero so a caller that reads the event straight
/// after starting it still sees the transient `InProgress` status, and short
/// enough that a test need not wait long for the terminal one.
pub const EVENT_SETTLEMENT_DELAY: Duration = Duration::from_millis(500);

/// How far back an automation event stays reportable. `ListAutomationEvents`
/// documents "You can retrieve events that were created within the past year"
/// (`smithy_query(query="doc",shape="ListAutomationEvents")`), so an event
/// created before that window is outside every query's reach however far back
/// the caller's `startTimeInclusive` points.
pub const EVENT_RETRIEVAL_WINDOW: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// How many automation events the store remembers.
///
/// CHOSEN bound, for the same reason [`MAX_RULES_PER_ACCOUNT`] is one: neither
/// the model nor the operation docs give a stored-event limit, and this store
/// lives in memory, so the oldest event is evicted to make room instead of
/// letting the process grow with every started event. [`EVENT_RETRIEVAL_WINDOW`]
/// does not bound it on its own: it only decides what a query may report.
pub const MAX_STORED_EVENTS: usize = 1_000;

/// How many automation rules one account may hold before `CreateAutomationRule`
/// answers the modelled `ServiceQuotaExceededException` ("The request would
/// exceed service quotas", HTTP 402).
///
/// CHOSEN bound: the service's real per-account rule quota is in neither the
/// model nor the operation docs, and this store lives in memory, so the count is
/// capped rather than left to grow with every request. It is the same order as
/// the largest page the model's `maxResults` allows.
pub const MAX_RULES_PER_ACCOUNT: usize = 100;

/// The description a rule holds when neither its create nor any later update
/// named one.
///
/// `description` is "A description of the automation rule"
/// (`smithy_query(query="doc",shape="AutomationRule",member="description")`) and
/// the service reports it for every rule it holds, so a rule nobody described
/// holds the empty description rather than none: free text nobody wrote is text
/// of no length, and `RuleDescription` admits it (`@length(min: 0)`,
/// `smithy_query(query="shape",shape="RuleDescription")`).
pub const DEFAULT_RULE_DESCRIPTION: &str = "";

/// The precedence a rule holds when neither its create nor any later update
/// named a `priority`.
///
/// `priority` is "a string representation of a decimal number between 0 and 1
/// (having up to 30 digits after the decimal point) that determines the priority
/// of the rule. When multiple rules match the same recommended action, Compute
/// Optimizer assigns the action to the rule with the lowest priority value
/// (highest priority)"
/// (`smithy_query(query="doc",shape="AutomationRule",member="priority")`), and
/// the service reports one for every rule it holds, so every rule has a
/// comparable precedence. CHOSEN value, grounded in that sentence: a rule nobody
/// prioritised must not outrank one somebody did, so it takes the TOP of the
/// documented range — the LOWEST precedence — and claims a recommended action
/// only when no prioritised rule matches the same action.
pub const DEFAULT_RULE_PRIORITY: &str = "1";

/// How many tags one resource of this service may carry.
///
/// DOCUMENTED, not chosen: the `tags` member of a rule create reads "A list of
/// key-value pairs used to categorize and organize the automation rule. Maximum
/// of 200 tags allowed."
/// (`smithy_query(query="doc",shape="CreateAutomationRuleRequest",member="tags")`),
/// and `TagList` — the shape every tag list of this service is, request and
/// response alike — carries `@length(max: 200)`
/// (`smithy_query(query="shape",shape="TagList")`). So the list a resource
/// HOLDS is bounded exactly as the list a request carries is, and an addition
/// that would push it past the bound is refused instead of applied.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// How many conditions one filter criteria of a rule may carry.
///
/// DOCUMENTED, not chosen: `Criteria` reads "You can specify up to 20 conditions
/// per filter criteria and 20 values per condition"
/// (`smithy_query(query="doc",shape="Criteria")`), and each MEMBER of `Criteria`
/// is one filter criteria — `region` is "Filter criteria for Amazon Web Services
/// regions where resources must be located", `resourceArn` is "Filter criteria
/// for specific resource ARNs to include or exclude", and so on — so the cap is
/// on the length of each condition list.
pub const MAX_CONDITIONS_PER_CRITERION: usize = 20;

/// How many values one criteria condition may compare against ("…and 20 values
/// per condition", the same `Criteria` doc sentence).
pub const MAX_VALUES_PER_CONDITION: usize = 20;

/// How many whitespace-separated fields the cron expression of a schedule may
/// have.
///
/// DOCUMENTED: "A `cron` expression consists of six fields separated by white
/// spaces: (`minutes` `hours` `day_of_month` `month` `day_of_week` `year`)",
/// and the same doc's own example — "For example: (`30 12 * * *`) runs daily at
/// 12:30 PM UTC" — writes the expression with the trailing `year` field left
/// out (`smithy_query(query="doc",shape="Schedule",member="scheduleExpression")`),
/// so a five-field expression says when the schedule runs just as well and is
/// accepted too.
pub const CRON_FIELD_COUNTS: &[usize] = &[5, 6];

/// How many client tokens the idempotency ledger remembers. Reached only by
/// distinct caller-supplied tokens; the oldest entry is evicted to make room, so
/// the ledger cannot grow without bound. A replay window that has been evicted
/// simply behaves as a fresh token.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 1024;

/// How long a client token stays replayable. CHOSEN bound: the model gives no
/// lifetime for `ClientToken`, and an entry that no retry will ever ask about
/// must not be kept for the life of the process.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The account id every emulated caller presents. Built through the engine's
/// canonical accessor rather than a literal, so every `accountId` and ARN this
/// service emits carries the one placeholder account.
pub fn caller_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The ARN of the rule identified by `rule_id`, in the form the model's
/// `RuleArn` pattern requires.
pub fn rule_arn(rule_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, "", &format!("automation-rule/{rule_id}"))
}

/// Does `candidate` name an automation rule of this service at all?
///
/// The form is the model's `RuleArn`
/// (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`,
/// from `smithy_query(query="shape",shape="RuleArn")`): the service's own ARN
/// namespace, an EMPTY region segment, a 12-digit account and the
/// `automation-rule/<ruleId>` resource. A string that does not have it names no
/// resource of this service, whether or not a rule with that ARN exists.
///
/// Checked by hand rather than with a regex engine: the pattern is a fixed
/// prefix plus two character classes, and this runs on the dispatch path.
pub fn names_automation_rule(candidate: &str) -> bool {
    let Some(rest) = candidate
        .strip_prefix("arn:aws:")
        .and_then(|rest| rest.strip_prefix(ARN_NAMESPACE))
        .and_then(|rest| rest.strip_prefix("::"))
    else {
        return false;
    };
    let Some((account_id, resource)) = rest.split_once(':') else {
        return false;
    };
    if account_id.len() != 12 || !account_id.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(rule_id) = resource.strip_prefix("automation-rule/") else {
        return false;
    };
    !rule_id.is_empty()
        && rule_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Is `key` the form the model's `TagKey` declares?
///
/// `TagKeyList`'s member targets a plain `String`
/// (`smithy_query(query="shape",shape="TagKeyList,TagKey")`), so the boundary
/// validator checks nothing for a key sent to `UntagResource` — but a tag key of
/// this service is a `TagKey`, which carries `@length(min: 1, max: 128)` and
/// `@pattern ^[\w\s\.\-\:\/\=\+\@]+$`, so that is the form checked here. `\w` is
/// `[A-Za-z0-9_]` plus the Unicode word characters `char::is_alphanumeric`
/// covers.
pub fn tag_key_is_valid(key: &str) -> bool {
    let length = key.chars().count();
    if !(1..=128).contains(&length) {
        return false;
    }
    key.chars().all(|c| {
        c.is_alphanumeric()
            || c.is_whitespace()
            || matches!(c, '_' | '.' | '-' | ':' | '/' | '=' | '+' | '@')
    })
}

/// The tag-key namespace that belongs to the provider, not to the caller: a key
/// opening with this prefix labels a provider-managed tag.
///
/// The model cannot express it — `TagKey`'s pattern `^[\w\s\.\-\:\/\=\+\@]+$`
/// admits a colon (`smithy_query(query="shape",shape="TagKey")`) — so a key in
/// this namespace reaches the handler and is refused there.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Is `key` in the namespace [`RESERVED_TAG_KEY_PREFIX`] names?
///
/// The prefix is compared without regard to case, because the namespace is the
/// one the prefix NAMES and not one spelling of it: a key differing only in the
/// case of the prefix is a key in the same namespace.
pub fn tag_key_is_reserved(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The message a tag key whose FORM the service refuses carries.
///
/// FIXED text: the requirement a tag rejection has to meet is its modelled code
/// and its HTTP 400, and this body echoes no caller-supplied key.
pub const TAG_VALIDATION_FAILED_MESSAGE: &str = "Tag validation failed";

/// The message a tag key in the reserved namespace carries. States the PREFIX
/// the caller may not use, which is the one fact it needs, and echoes no key the
/// request carried.
pub fn reserved_tag_key_message() -> String {
    format!(
        "Tag keys beginning with {RESERVED_TAG_KEY_PREFIX} are reserved for Amazon Web Services \
         use and cannot be specified"
    )
}

/// Why these caller-supplied tag keys may not be used, as the service's own
/// validation message, if they may not.
///
/// The ONE place a tag key a request carries is judged, so every operation that
/// takes tag keys — the tags a rule create carries, the tags an addition carries
/// and the keys a removal names — refuses the same keys the same way, and a
/// request carrying one such key is refused before any tag is written:
///
/// * a key outside the form the model's `TagKey` declares
///   ([`tag_key_is_valid`]), which the protocol boundary checks only where a
///   member targets `TagKey` and not where it targets a plain `String`;
/// * a key in the provider's reserved namespace ([`tag_key_is_reserved`]), which
///   is a caller's to neither set nor remove.
pub fn tag_keys_violation<'a, I>(keys: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    for key in keys {
        if !tag_key_is_valid(key) {
            return Some(TAG_VALIDATION_FAILED_MESSAGE.to_string());
        }
        if tag_key_is_reserved(key) {
            return Some(reserved_tag_key_message());
        }
    }
    None
}

/// Why this organization-association request's `accountIds` cannot be acted on,
/// if it cannot, as the service's own validation report ([`constraint_report`],
/// which echoes no request value).
///
/// The list of member accounts an association request names is read by both
/// association operations, so what makes it refusable is judged here once as
/// well: a list one of them cannot act on is one the other cannot act on either,
/// and the two cannot drift apart over which requests they refuse, over the
/// bound they refuse them at or over how they word the refusal. The clause is
/// the documented SIZE of the list, which the `AccountIdList` shape carries no
/// `@length` for: the paired operations name "one or more member accounts" and
/// the member reads "You can specify up to 50 account IDs"
/// ([`MAX_ACCOUNT_IDS_PER_REQUEST`]), so a request naming none, and one naming
/// more, is refused rather than acted on for a prefix of the list.
pub fn account_ids_violation(account_ids: &[String]) -> Option<String> {
    if let Some(report) = empty_value_list_violation("accountIds", account_ids.len()) {
        return Some(report);
    }
    if account_ids.len() > MAX_ACCOUNT_IDS_PER_REQUEST {
        return Some(constraint_report(
            "accountIds",
            &format!("Member must have length less than or equal to {MAX_ACCOUNT_IDS_PER_REQUEST}"),
        ));
    }
    None
}

/// The accounts an organization-association request names, each ONCE and in the
/// order the request first names them.
///
/// An id repeated in the list names the same account, and an account is
/// associated or it is not, so both association operations read their request's
/// list this way — once, here, so the two cannot disagree about what a repeated
/// id means or about the order they answer in.
pub fn distinct_account_ids(account_ids: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(account_ids.len());
    account_ids
        .iter()
        .filter(|account_id| seen.insert(account_id.as_str()))
        .cloned()
        .collect()
}

/// Now, as the wire timestamp type. The ONE clock the handlers read, so a stored
/// timestamp is always the real store time.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
}

/// A stable digest of `value`'s debug rendering, used both as a request
/// fingerprint for the client-token ledger and as the filter fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its
/// members in declaration order, so the same request always digests to the same
/// string and a different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    let digest = Sha256::digest(format!("{value:?}").as_bytes());
    hex::encode(digest)
}

/// The digest the client-token ledger compares for a request, taken over the
/// request AS THE HANDLER READS IT.
///
/// A client token exists so a retry of ONE request is replayed rather than acted
/// on twice, so what the ledger has to tell apart is requests the service acts
/// on differently ([`TokenOutcome::ParameterMismatch`]). `as_read` is where a
/// handler states that reading of its own input: the client token itself, which
/// is not part of the request it ties, and every member the handler NORMALISES
/// before acting — the accounts an association request names
/// ([`distinct_account_ids`]), a member read only by membership
/// ([`canonical_membership`]), a member a create leaves out and the handler
/// materialises a value for ([`DEFAULT_RULE_DESCRIPTION`],
/// [`DEFAULT_RULE_PRIORITY`]) — carry the normalised value in the probe. Two
/// spellings of one request therefore replay, exactly as two spellings of one
/// query continue one page walk ([`page_fingerprint`]), while every difference
/// the service does act on still digests differently and is still refused.
pub fn request_fingerprint<T: Clone + std::fmt::Debug>(
    input: &T,
    as_read: impl FnOnce(&mut T),
) -> String {
    let mut probe = input.clone();
    as_read(&mut probe);
    fingerprint(&probe)
}

/// The fingerprint a paginated listing binds into the page tokens it mints: the
/// name of the operation that issued the token, together with the query the walk
/// was taken for.
///
/// A page token is opaque and belongs to ONE walk, so what a handler accepts is
/// the fingerprint it would itself have minted: a token taken for a different
/// query, and equally a token one of this service's other paginated operations
/// minted, is refused rather than answered with a page of a different result
/// set. The operation name is a parameter of this helper rather than something
/// each caller remembers to mix in, so no listing can bind the query alone and
/// end up accepting a sibling's token. [`preview_selection_fingerprint`] is this
/// same rule over the preview selection.
pub fn page_fingerprint<T: std::fmt::Debug + ?Sized>(operation: &str, query: &T) -> String {
    fingerprint(&(operation, query))
}

/// `values` as the SET the walk reads it as: every distinct entry once, in one
/// order.
///
/// A query member a walk reads only by MEMBERSHIP — `contains`, or `any` over
/// its entries — names a set, not a sequence: neither the order the request
/// spells the entries in nor an entry it repeats changes which items are
/// selected, and the order a page advances in is the store's record order. So
/// what a page token digests for such a member is the set ([`page_fingerprint`]),
/// which is what makes two spellings of one selection continue ONE walk. A
/// member a walk reads in order would keep its spelling: there the order is part
/// of the sequence a page is a window onto.
///
/// Not [`distinct_account_ids`], which drops repeats but keeps the order the
/// request first names each account, because that order is the one an
/// association response reports back; here the request's order is exactly what
/// must not matter.
pub fn canonical_membership(values: &[String]) -> Vec<String> {
    let mut canonical = values.to_vec();
    canonical.sort_unstable();
    canonical.dedup();
    canonical
}

/// One `name`/`values` filter of a paginated listing, read as the filter
/// evaluators of this service read it.
///
/// The three filter shapes this service models — of an automation rule, of a
/// recommended action and of an automation event — carry the same two members
/// and are selected on the same way ([`rule_matches_filters`],
/// [`recommended_action_matches_filters`], [`event_matches_filters`]), so
/// [`canonical_filters`] normalises all three through this one reading.
pub trait SelectionFilter: Clone {
    /// The dimension this filter selects on.
    fn filter_name(&self) -> &str;
    /// The alternatives the filter accepts for that dimension.
    fn filter_values(&self) -> &[String];
    /// The same filter carrying `values` instead.
    fn with_filter_values(self, values: Vec<String>) -> Self;
}

macro_rules! impl_selection_filter {
    ($($shape:ident),+ $(,)?) => {
        $(impl SelectionFilter for $shape {
            fn filter_name(&self) -> &str {
                &self.name
            }

            fn filter_values(&self) -> &[String] {
                &self.values
            }

            fn with_filter_values(mut self, values: Vec<String>) -> Self {
                self.values = values;
                self
            }
        })+
    };
}

impl_selection_filter!(AutomationEventFilter, Filter, RecommendedActionFilter);

/// A listing's filter list as the SELECTION it performs, for a page token to
/// digest: every filter's alternatives as the set they are
/// ([`canonical_membership`]), the filters themselves in one order, and a filter
/// the request spelled twice only once.
///
/// Every filter evaluator of this service requires ALL of the request's filters
/// to hold and reads each filter's `values` by membership, so the order of the
/// filters, the order of the values inside one and a filter repeated verbatim
/// cannot change which items a walk selects or the order it serves them in. Two
/// filters that share a name and differ in their values are two conditions and
/// stay two, so a selection that really constrains differently still digests
/// differently.
pub fn canonical_filters<F: SelectionFilter>(filters: &[F]) -> Vec<F> {
    let mut canonical: Vec<F> = filters
        .iter()
        .map(|filter| {
            filter
                .clone()
                .with_filter_values(canonical_membership(filter.filter_values()))
        })
        .collect();
    canonical.sort_by(|left, right| {
        (left.filter_name(), left.filter_values()).cmp(&(right.filter_name(), right.filter_values()))
    });
    canonical.dedup_by(|left, right| {
        left.filter_name() == right.filter_name() && left.filter_values() == right.filter_values()
    });
    canonical
}

/// Why this listing's `filters` cannot narrow it, if they cannot: one of them
/// carries no value to select on.
///
/// A filter's `values` are the alternatives it accepts for its dimension, and
/// every filter evaluator of this service requires ALL of the request's filters
/// to hold ([`rule_matches_filters`], [`event_matches_filters`],
/// [`recommended_action_matches_filters`]), so a filter carrying no value states
/// a condition nothing can satisfy: the request is refused under the floor every
/// caller-supplied value list of this service is held to
/// ([`empty_value_list_violation`]) rather than answered with an empty page that
/// reads as an authoritative result set. The three filter shapes all target
/// `FilterValues`, which bounds its MEMBER at `length(1..255)` and carries no
/// `@length` of its own
/// (`smithy_query(query="shape",shape="FilterValues,Filter,AutomationEventFilter,RecommendedActionFilter")`),
/// so the protocol boundary admits the empty list — `@required` is satisfied by
/// one — and the check lives here, once, for every operation that filters. The
/// report names the offending filter by its position in the list, counted from
/// one, exactly as the boundary's own refusal of a filter `name` does.
pub fn filters_violation<F: SelectionFilter>(filters: &[F]) -> Option<String> {
    filters.iter().enumerate().find_map(|(index, filter)| {
        empty_value_list_violation(
            &format!("filters.{}.member.values", index + 1),
            filter.filter_values().len(),
        )
    })
}

// ---------------------------------------------------------------------------
// Error wording
//
// Every client error this service raises is worded HERE, so the body a caller
// parses is built in one place and two handlers raising the same error cannot
// word it differently. Two shapes of rule live here:
//
// * a FIXED message, which the error carries whatever the request named — it
//   must not echo a resource identifier or any other request value;
// * a message BUILT from the request or from the stored resource, which states
//   the fact the caller needs to recover (the member and the constraint it
//   failed, the current revision, the values that are settable).
// ---------------------------------------------------------------------------

/// The message a REPORTING operation's `ResourceNotFoundException` carries.
///
/// FIXED and resource-independent: a read reports that the resource the request
/// named is not there, and names neither the ARN nor the id, so the body of a
/// read's not-found echoes no resource identifier back to its caller. The
/// service words a MUTATION's not-found the other way round — see
/// [`rule_not_found_message`].
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The specified resource could not be found";

/// The `ResourceNotFoundException` message for a MUTATION whose target rule the
/// store does not hold.
///
/// A mutation of an automation rule names the `RuleArn` it was sent, because
/// that is the one fact a caller retrying or batching mutations needs to tell
/// WHICH of its targets is gone; a read of the same rule answers the fixed
/// sentence ([`RESOURCE_NOT_FOUND_MESSAGE`]) instead. Every operation that
/// changes a rule through its ARN — the rule's update and delete, and the tag
/// addition and removal that target the same ARN — words its 404 here, so the
/// mutation side of that split is worded in ONE place.
pub fn rule_not_found_message(rule_arn: &str) -> String {
    format!("Rule with ARN {rule_arn} does not exist")
}

/// The message every `OptInRequiredException` of this service carries.
///
/// FIXED and request-independent: the refusal states that the ACCOUNT is not
/// registered, and names neither the operation, the account nor anything else
/// the request carried. The requirement this error has to meet is its modelled
/// code and its HTTP 403; this is the service's own wording for it.
pub const OPT_IN_REQUIRED_MESSAGE: &str = "Aws account is not registered for recommendation.";

/// The message an `AccessDeniedException` that names NO resource of the request
/// carries: the sentence the shape itself documents
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
///
/// FIXED and request-independent, and raised where the decision is about which
/// ACCOUNTS a rule would automate in rather than about a resource the request
/// addressed: naming the accounts an organization reaches would disclose them to
/// a caller the decision just refused. A refusal taken on the resource string a
/// request DID supply is worded by [`unauthorized_resource_message`] instead.
pub const ACCESS_DENIED_MESSAGE: &str =
    "You do not have sufficient permissions to perform this action.";

/// The IAM action a request for `operation` of this service asks permission for.
///
/// An IAM action is `<service prefix>:<OperationName>`, and this service's
/// prefix is the signing name the model vends (`aco-automation`, the model's
/// `sigv4Name` and `endpointPrefix`,
/// `smithy_query(query="service_info")`) — read off this service's own identity
/// by the caller rather than restated here, so the action named in a refusal
/// cannot drift from the name the request signed with. The qualification itself is the
/// shared one (`service_common::iam_authorization::iam_action`), so this service and every
/// other that names an action in a refusal spell it the same way.
pub fn iam_action(signing_name: &str, operation: &str) -> String {
    ::service_common::iam_authorization::iam_action(signing_name, operation)
}

/// How an authorization decision taken on the resource string a request supplied
/// is worded: the standard IAM denial, built from the request.
///
/// Authorization is evaluated against the identity the request signed with, the
/// action it asks for and the resource string it carries, all three of which the
/// request itself states — so the refusal names them: the principal
/// ([`caller_identity_arn`]), the IAM action ([`iam_action`]) and the resource
/// exactly as sent. A client parsing the denied action and resource out of the
/// message, or telling a malformed identifier apart from a missing permission,
/// reads them here.
pub fn unauthorized_resource_message(principal: &str, action: &str, resource: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource}")
}

/// The ARN of the identity THIS request authenticates as, which is the principal
/// an authorization refusal names.
///
/// Resolved from the same records `sts:GetCallerIdentity` answers from — the STS
/// session the presented access key id names, else the IAM user that holds that
/// key as a long-term credential, else the ambient identity this emulator
/// publishes — so a caller that reads its own identity back from STS sees the
/// principal this service blames, rather than a second identity invented here.
pub fn caller_identity_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    sts_state::service_state(engine)
        .and_then(|sts| {
            // The identity a key that names no session speaks for: the IAM user
            // holding it, read through STS's own accessor so the fallback is the
            // one `GetCallerIdentity` uses.
            let long_term = match sts_state::iam_state(engine) {
                Some(iam) => sts_state::long_term_identity(Some(&iam.lock()), &access_key_id),
                None => sts_state::long_term_identity(None, &access_key_id),
            };
            sts_state::resolve_request_caller(
                &mut sts.lock(),
                &access_key_id,
                SystemTime::now(),
                ctx.credential_store(),
                long_term,
            )
            .ok()
            .map(|caller| caller.identity)
        })
        // An engine bearing no STS store, or a session credential that has
        // already expired: neither names a principal, so the refusal is about
        // the identity this emulator publishes.
        .unwrap_or_else(sts_state::ambient_identity)
        .arn
}

/// The message every `IdempotentParameterMismatchException` of this service
/// carries — the service's own wire wording for "this client token was first
/// used with a different request", FIXED and derived from nothing in the
/// request.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "The request tied to this token is different!";

/// The message every `IdempotencyTokenInUseException` of this service carries —
/// the sentence the shape itself documents
/// (`smithy_query(query="shape",shape="IdempotencyTokenInUseException")`),
/// FIXED and derived from nothing in the request.
///
/// The ledger is service-wide, so this is the other outcome of a token already
/// spent ([`TokenOutcome::TokenInUse`]) beside
/// [`IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`]: every operation of this service
/// that accepts a `clientToken` words that refusal from here, so the body a
/// caller parses is the same whichever operation first claimed the token and
/// whichever one re-presents it.
pub const IDEMPOTENCY_TOKEN_IN_USE_MESSAGE: &str = "The specified client token is already in use.";

/// The `InvalidParameterValueException` message for a request whose
/// `ruleRevision` is not the rule's current one.
///
/// `ruleRevision` is the rule's optimistic-concurrency token, so the rejection
/// states the rule's CURRENT revision — the one fact a caller that lost the race
/// needs to read-modify-write again — and not the rejected input or the rule's
/// ARN.
pub fn revision_mismatch_message(current_revision: i64) -> String {
    format!("Rule revision does not match the current revision number {current_revision}")
}

/// The `InvalidParameterValueException` message for a tag addition that would
/// leave the resource holding more than [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// States the BOUND, which is the one fact a caller past it needs, and echoes
/// neither a key, a value nor the resource.
pub fn tag_limit_exceeded_message() -> String {
    format!("The request would exceed the limit of {MAX_TAGS_PER_RESOURCE} tags per resource")
}

/// The `InvalidParameterValueException` message for an association that would
/// leave the enrollment store holding more than [`MAX_ENROLLED_ACCOUNTS`]
/// accounts.
///
/// States the BOUND, which is the one fact a caller past it needs, and echoes no
/// account id the request carried.
pub fn account_limit_exceeded_message() -> String {
    format!("The request would exceed the limit of {MAX_ENROLLED_ACCOUNTS} accounts")
}

/// The `ServiceQuotaExceededException` message for a create that would leave
/// `account_id` holding more than [`MAX_RULES_PER_ACCOUNT`] automation rules.
///
/// States the BOUND, which is the one fact a caller past it needs, and the
/// account the quota is counted per — the caller's own account, which is what
/// makes the bound actionable when the same credentials automate several of
/// them. It echoes nothing the request carried.
pub fn rule_quota_exceeded_message(account_id: &str) -> String {
    format!(
        "The request would exceed the quota of {MAX_RULES_PER_ACCOUNT} automation rules for \
         account {account_id}."
    )
}

/// The `EnrollmentStatus` values `UpdateEnrollmentConfiguration` accepts from a
/// caller.
///
/// `EnrollmentStatus` is `Active | Inactive | Pending | Failed`
/// (`smithy_query(query="shape",shape="EnrollmentStatus")`) and the request
/// member documents that "The Pending and Failed options cannot be used to
/// update the enrollment status of an account. They are returned in the
/// response", so those two are server-only and the rest are settable.
pub const SETTABLE_ENROLLMENT_STATUSES: &[&str] = &["Active", "Inactive"];

/// The `InvalidParameterValueException` message for an enrollment status a
/// caller may not set: it names the rejected value AND enumerates the settable
/// set, so the caller is told what to send instead.
pub fn invalid_enrollment_status_message(status: &str) -> String {
    format!(
        "Invalid status: {status}. Only {} are allowed.",
        settable_enrollment_statuses_phrase()
    )
}

/// The settable statuses as the rejection lists them: the model's value set,
/// upper-cased, joined so the last one reads "… and X".
fn settable_enrollment_statuses_phrase() -> String {
    let values: Vec<String> = SETTABLE_ENROLLMENT_STATUSES
        .iter()
        .map(|status| status.to_uppercase())
        .collect();
    match values.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} and {last}", leading.join(", ")),
    }
}

/// The marker that separates the member clause of a validation clause from the
/// requirement it failed, in the framework's own wording.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// AWS's standard validation report for the modelled constraints a request
/// failed at the protocol boundary, as `src/service.rs` words a refusal with.
///
/// The report opens with HOW MANY members failed, then states one clause per
/// failing member: the member's dotted path and the requirement it failed. It
/// does NOT echo the rejected value — a request value (a name, or a secret on
/// some other member) must not be replayed into an error body — and reports a
/// `@pattern` as the regular expression itself, without the anchors the model
/// spells it with.
///
/// ONE refusal is worded differently, because the service words it itself rather
/// than leaving it to the generic constraint report: a value outside the value
/// set of an enum PARAMETER TYPE ([`REQUEST_ENUM_TYPES`]) is refused by naming
/// that type, the value as sent and the type's modelled values — see
/// [`enum_parameter_rejection`].
pub fn validation_report(violations: &[Violation]) -> String {
    // A value outside one of this service's enum PARAMETER TYPES is reported by
    // the service's own parameter validation, which names the type and the
    // value it was sent, and answers AHEAD of the generic constraint report —
    // see [`enum_parameter_rejection`].
    if let Some(report) = enum_parameter_rejection(violations) {
        return report;
    }
    let count = violations.len();
    let noun = if count == 1 { "error" } else { "errors" };
    let clauses: Vec<String> = violations.iter().map(validation_clause).collect();
    format!("{count} validation {noun} detected: {}", clauses.join("; "))
}

/// One clause of [`validation_report`]: the member and what it failed, with no
/// value echoed.
fn validation_clause(violation: &Violation) -> String {
    format!(
        "Value at '{}' {CONSTRAINT_MARKER}{}",
        violation.path,
        violation_requirement(violation)
    )
}

/// The validation report for ONE member a HANDLER refused — a rule the model's
/// constraints cannot carry, so the protocol boundary admitted the value and
/// the check lives in the handler.
///
/// Same format as [`validation_report`], because a client counts and attributes
/// a constraint failure the same way wherever the service found it, and the
/// rejected value is echoed in neither.
pub fn constraint_report(member: &str, requirement: &str) -> String {
    format!("1 validation error detected: Value at '{member}' {CONSTRAINT_MARKER}{requirement}")
}

/// The enum parameter TYPES a REQUEST of this service can carry a value for,
/// each under the name the model gives the shape, with the value set that shape
/// declares.
///
/// Every entry is a typed Smithy `enum` SHAPE that some input member targets:
/// `ruleType` (`RuleType`), an element of `recommendedActionTypes`
/// (`RecommendedActionType`), `status` (`RuleStatus` on the rule operations,
/// `EnrollmentStatus` on `UpdateEnrollmentConfiguration`),
/// `organizationConfiguration.ruleApplyOrder` (`RuleApplyOrder`), a criteria
/// condition's `comparison` (`ComparisonOperator`), `organizationRuleMode`
/// (`OrganizationRuleMode`) on the organization-configuration put, an element of
/// its delete's `configurationTypes`
/// (`OrganizationEnrollmentConfigurationType`) and the configuration listing's
/// `targetType` (`OrganizationTargetType`) — the enum-typed input members of
/// this service
/// (`smithy_query(query="operation",operation="CreateAutomationRule,UpdateAutomationRule,UpdateEnrollmentConfiguration,ListAutomationRulePreview,ListAutomationRulePreviewSummaries,PutOrganizationEnrollmentConfiguration,DeleteOrganizationEnrollmentConfiguration,ListOrganizationEnrollmentConfigurations")`).
/// The model's remaining enum shapes — `EventStatus`, `EventType`, `StepStatus`,
/// `StepType`, `ResourceType`, `SavingsEstimationMode`,
/// `AutoEnrollMemberAccounts`, `OrganizationRuleModeSourceType`,
/// `OrganizationEnrollmentConfigurationStatus` —
/// appear in RESPONSES only, so no request can be refused against one, and
/// naming a type here cannot be ambiguous (`EventType` happens to share
/// `RecommendedActionType`'s value set, and
/// `OrganizationEnrollmentConfigurationStatus` `RuleStatus`'s; neither is a
/// request parameter).
///
/// Each value set is the GENERATED, model-derived constant for that shape, so no
/// value list is spelled here: the set a request is refused against and the set
/// the refusal enumerates move together with the model.
///
/// Membership of this table is also what separates a named parameter type from
/// the model's legacy `@enum`-constrained STRINGS (`AutomationRuleFilterName`,
/// `AutomationEventFilterName`, `RecommendedActionFilterName`,
/// `SummaryDimensionKey`), which name no parameter type of their own and so keep
/// the generic validation report.
const REQUEST_ENUM_TYPES: &[(&str, &[&str])] = &[
    ("ComparisonOperator", COMPARISON_OPERATOR_VALUES),
    ("EnrollmentStatus", ENROLLMENT_STATUS_VALUES),
    (
        "OrganizationEnrollmentConfigurationType",
        ORGANIZATION_ENROLLMENT_CONFIGURATION_TYPE_VALUES,
    ),
    ("OrganizationRuleMode", ORGANIZATION_RULE_MODE_VALUES),
    ("OrganizationTargetType", ORGANIZATION_TARGET_TYPE_VALUES),
    ("RecommendedActionType", RECOMMENDED_ACTION_TYPE_VALUES),
    ("RuleApplyOrder", RULE_APPLY_ORDER_VALUES),
    ("RuleStatus", RULE_STATUS_VALUES),
    ("RuleType", RULE_TYPE_VALUES),
];

/// The rejection for the FIRST value these violations carry that fell outside
/// one of this service's enum parameter types, or `None` when none did.
///
/// A request is judged parameter by parameter, so the refusal states the one
/// parameter type it could not accept a value for rather than a count of
/// constraint clauses; a refusal carrying no such parameter is left to the
/// generic report.
fn enum_parameter_rejection(violations: &[Violation]) -> Option<String> {
    violations.iter().find_map(|violation| {
        let ViolationKind::EnumValue(declared) = &violation.kind else {
            return None;
        };
        // An absent member carries no value to name, so it is not this refusal.
        let value = violation.value.as_deref()?;
        let enum_type = request_enum_type(declared)?;
        Some(invalid_enum_value_message(enum_type, value, declared))
    })
}

/// The enum parameter type whose declared value set is `declared`.
///
/// A violation states the value set it refused a value against but not the shape
/// that set came from, so the type is recovered by matching the set against
/// [`REQUEST_ENUM_TYPES`]. Compared as a SET: a violation lists the model's
/// declaration order, a generated value constant is sorted.
fn request_enum_type(declared: &[String]) -> Option<&'static str> {
    REQUEST_ENUM_TYPES
        .iter()
        .find(|(_, values)| same_value_set(values, declared))
        .map(|(enum_type, _)| *enum_type)
}

/// Do these two value sets hold the same values? An enum's values are distinct,
/// so equal lengths plus containment is set equality.
fn same_value_set(declared: &[&str], refused: &[String]) -> bool {
    declared.len() == refused.len()
        && refused
            .iter()
            .all(|value| declared.contains(&value.as_str()))
}

/// The `InvalidParameterValueException` message for a value outside an enum
/// parameter type's modelled value set.
///
/// Built from the request and the model and from nothing else: the TYPE whose
/// set the value missed, the value exactly as the request sent it (bounded by
/// the framework's echo cap on a violation's value), and that type's modelled
/// values in the order the model declares them — which is the order the
/// violation carries them in. The rejected value IS named here, unlike in the
/// generic report: it is the token the caller has to replace, and a caller that
/// sent a list of them needs to know which one was refused.
fn invalid_enum_value_message(enum_type: &str, value: &str, valid: &[String]) -> String {
    format!(
        "Invalid {enum_type} value: {value}. Valid values: {}",
        valid.join(", ")
    )
}

/// The requirement a violation states.
///
/// A `@pattern` is reported unanchored, because the requirement is the
/// expression a value must match and the `^`/`$` are how the model spells
/// "whole value". Every other constraint keeps the framework's own requirement
/// wording, read off the clause it builds, so a constraint kind added to the
/// framework later is reported rather than dropped.
fn violation_requirement(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => {
            format!(
                "Member must satisfy regular expression pattern: {}",
                unanchored(pattern)
            )
        }
        _ => {
            // Read the requirement off a clause the framework builds for this
            // SAME violation with no value attached. A clause that carried the
            // value would put caller-controlled text before the marker, so a
            // value containing the marker itself would split inside the value
            // and copy the rest of it into the report.
            let valueless = Violation {
                value: None,
                ..violation.clone()
            };
            let clause = valueless.message();
            match clause.split_once(CONSTRAINT_MARKER) {
                Some((_, requirement)) => requirement.to_string(),
                None => clause,
            }
        }
    }
}

/// `pattern` without its `^` prefix and `$` suffix.
fn unanchored(pattern: &str) -> &str {
    let without_caret = pattern.strip_prefix('^').unwrap_or(pattern);
    without_caret.strip_suffix('$').unwrap_or(without_caret)
}

/// The enrollment configuration of one account.
#[derive(Debug, Clone)]
pub struct EnrollmentConfig {
    pub status: String,
    pub status_reason: Option<String>,
    pub organization_rule_mode: Option<String>,
    pub last_updated_timestamp: Option<::aws_smithy_types::DateTime>,
}

impl Default for EnrollmentConfig {
    fn default() -> Self {
        Self {
            status: DEFAULT_ENROLLMENT_STATUS.to_string(),
            status_reason: None,
            organization_rule_mode: None,
            last_updated_timestamp: None,
        }
    }
}

impl EnrollmentConfig {
    /// The `OrganizationRuleMode` every reader reports for this configuration:
    /// the mode the store holds, and [`ORGANIZATION_RULE_MODE_NONE_ALLOWED`]
    /// while it holds none.
    ///
    /// The mode "Specifies whether the management account can create Automation
    /// rules that implement optimization actions for this account", and an
    /// association is exactly what grants that permission
    /// (`AssociateAccounts`: the member account's "organization rule mode is
    /// automatically set to \"Any allowed\""), so an account no association has
    /// touched is in the other of the enum's two states rather than in no state
    /// at all. Deriving it HERE is what makes every reader of one account —
    /// `GetEnrollmentConfiguration` for the caller, `ListAccounts` for any
    /// account — report the same mode for it.
    pub fn reported_organization_rule_mode(&self) -> String {
        self.organization_rule_mode
            .clone()
            .unwrap_or_else(|| ORGANIZATION_RULE_MODE_NONE_ALLOWED.to_string())
    }

    /// This stored configuration as the `AccountInfo` a listing reports for
    /// `account_id`: its enrollment status ("The enrollment status of the
    /// account"), the reason that explains it, the organization rule mode
    /// `effective` resolved for it together with where that mode came from, and
    /// the instant it was last updated.
    ///
    /// `organizationRuleMode` and `lastUpdatedTimestamp` are REQUIRED on
    /// `AccountInfo` while a stored configuration holds each only once something
    /// has set it, so the projection has to name a value for a configuration
    /// that holds none:
    ///
    /// * the rule mode is the EFFECTIVE one
    ///   ([`ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState::effective_rule_mode_for_account`]),
    ///   which names a value for every account: the organization enrollment
    ///   configuration that applies to it, else the mode set on the account
    ///   itself, else [`ORGANIZATION_RULE_MODE_NONE_ALLOWED`];
    /// * `at` stands in for a missing update instant. Every call that writes a
    ///   configuration stamps one, so a stored entry without one is not
    ///   reachable over the wire; this keeps the required member present rather
    ///   than reporting an instant (the epoch, say) at which nothing happened.
    pub fn to_account_info(
        &self,
        account_id: &str,
        at: ::aws_smithy_types::DateTime,
        effective: EffectiveRuleMode,
    ) -> AccountInfo {
        AccountInfo {
            account_id: account_id.to_string(),
            status: self.status.clone(),
            organization_rule_mode: effective.mode,
            organization_rule_mode_source: Some(effective.source),
            status_reason: self.status_reason.clone(),
            last_updated_timestamp: self.last_updated_timestamp.unwrap_or(at),
        }
    }
}

/// An automation rule as stored. Every `Get`/`List`/`Update` response is built
/// from one of these, never from the request that created it.
#[derive(Debug, Clone)]
pub struct StoredRule {
    pub rule_arn: String,
    pub rule_id: String,
    pub account_id: String,
    pub name: String,
    /// What the create or the latest update supplied, and
    /// [`DEFAULT_RULE_DESCRIPTION`] while neither has. The service reports a
    /// description for every rule it holds, so this is a value and never an
    /// absence: a rule nobody described is described by the empty string, and
    /// every read path reports that rather than omitting the member.
    pub description: String,
    pub rule_type: String,
    pub rule_revision: i64,
    pub organization_configuration: Option<OrganizationConfiguration>,
    /// What the create or the latest update supplied, and
    /// [`DEFAULT_RULE_PRIORITY`] while neither has. `priority` decides which rule
    /// claims a recommended action when several match ("the rule with the lowest
    /// priority value (highest priority)",
    /// `smithy_query(query="doc",shape="AutomationRule",member="priority")`), and
    /// the service reports one for every rule it holds, so this is a value and
    /// never an absence: a rule nobody prioritised holds the bottom of the
    /// precedence order and every read path reports it.
    pub priority: String,
    pub recommended_action_types: Vec<String>,
    pub criteria: Option<Criteria>,
    pub schedule: Schedule,
    pub status: String,
    pub tags: Vec<Tag>,
    pub created_timestamp: ::aws_smithy_types::DateTime,
    /// When this rule was last changed. Always a value: a rule that has never
    /// been updated was last changed when it was created, so a create stores its
    /// own creation instant here.
    pub last_updated_timestamp: ::aws_smithy_types::DateTime,
}

impl StoredRule {
    /// Replace this rule's tag list with `tags`, stamping
    /// [`Self::last_updated_timestamp`] with `at` when that write actually
    /// changes the rule.
    ///
    /// The ONE path by which a tagging operation writes a stored rule, so both
    /// of them record when the rule changed — which is what the member every
    /// reader answers with means, "when the automation rule was last updated"
    /// (`smithy_query(query="doc",shape="AutomationRule",member="lastUpdatedTimestamp")`).
    /// A request that leaves the list exactly as it stood updated nothing —
    /// re-adding the tags the rule already carries, removing a key it does not
    /// carry — so it moves no timestamp, the same way an opt-out stamps only the
    /// rules whose status it changes.
    pub fn set_tags(&mut self, tags: Vec<Tag>, at: ::aws_smithy_types::DateTime) {
        let unchanged = tags.len() == self.tags.len()
            && tags
                .iter()
                .zip(&self.tags)
                .all(|(new, stored)| new.key == stored.key && new.value == stored.value);
        if unchanged {
            return;
        }
        self.tags = tags;
        self.last_updated_timestamp = at;
    }

    /// This rule as the `AutomationRule` summary `ListAutomationRules` returns.
    /// The summary shape carries neither `criteria` nor `tags` — the model does
    /// not declare them on it — so those are reachable only through
    /// `GetAutomationRule`.
    pub fn to_automation_rule(&self) -> AutomationRule {
        AutomationRule {
            rule_arn: Some(self.rule_arn.clone()),
            rule_id: Some(self.rule_id.clone()),
            name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            rule_type: Some(self.rule_type.clone()),
            rule_revision: Some(self.rule_revision),
            account_id: Some(self.account_id.clone()),
            organization_configuration: self.organization_configuration.clone(),
            priority: Some(self.priority.clone()),
            recommended_action_types: Some(self.recommended_action_types.clone()),
            schedule: Some(self.schedule.clone()),
            status: Some(self.status.clone()),
            created_timestamp: Some(self.created_timestamp),
            last_updated_timestamp: Some(self.last_updated_timestamp),
        }
    }
}

/// The rules of this service, in insertion order, with the two lookups the API
/// needs: by `ruleArn` (every operation takes one) and by `ruleId` (which events
/// reference). Keeping all three in one type is what keeps them consistent:
/// `insert` and `remove` are the only writers.
#[derive(Debug, Default)]
pub struct RuleStore {
    /// `ruleArn`s in creation order, so `ListAutomationRules` pages over a
    /// stable sequence.
    order: Vec<String>,
    by_arn: HashMap<String, StoredRule>,
    /// Secondary index: `ruleId` -> `ruleArn`.
    by_id: HashMap<String, String>,
}

impl RuleStore {
    /// Store a newly created rule, appending it to the insertion order.
    pub fn insert(&mut self, rule: StoredRule) {
        self.by_id
            .insert(rule.rule_id.clone(), rule.rule_arn.clone());
        self.order.push(rule.rule_arn.clone());
        self.by_arn.insert(rule.rule_arn.clone(), rule);
    }

    pub fn get(&self, rule_arn: &str) -> Option<&StoredRule> {
        self.by_arn.get(rule_arn)
    }

    pub fn get_mut(&mut self, rule_arn: &str) -> Option<&mut StoredRule> {
        self.by_arn.get_mut(rule_arn)
    }

    /// The rule a `ruleId` names, resolved through the secondary index rather
    /// than by scanning.
    pub fn get_by_rule_id(&self, rule_id: &str) -> Option<&StoredRule> {
        let arn = self.by_id.get(rule_id)?;
        self.by_arn.get(arn)
    }

    pub fn contains_rule_id(&self, rule_id: &str) -> bool {
        self.by_id.contains_key(rule_id)
    }

    /// Remove a rule, dropping it from the order and both indexes. `None` when
    /// no rule has that ARN.
    pub fn remove(&mut self, rule_arn: &str) -> Option<StoredRule> {
        let rule = self.by_arn.remove(rule_arn)?;
        self.by_id.remove(&rule.rule_id);
        self.order.retain(|arn| arn != rule_arn);
        Some(rule)
    }

    /// Every stored rule, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &StoredRule> {
        self.order.iter().filter_map(|arn| self.by_arn.get(arn))
    }

    /// How many rules `account_id` owns, for the [`MAX_RULES_PER_ACCOUNT`] quota.
    pub fn count_for_account(&self, account_id: &str) -> usize {
        self.by_arn
            .values()
            .filter(|rule| rule.account_id == account_id)
            .count()
    }
}

/// The recommended actions this service holds, in the order they were recorded,
/// with the by-id lookup `StartAutomationEvent` needs.
///
/// Compute Optimizer's own analysis is what produces a recommended action, so no
/// request CARRIES one: the store holds the modelled `RecommendedAction` as the
/// service recorded it (see [`recommended_action_inventory`], which is what the
/// enrollment of an account records), and `ListRecommendedActions` reports
/// entries straight out of it, never from the request. It reads like the map it
/// replaces — `insert(id, action)` / `get(id)` — while also keeping the record
/// order a paginated list has to page over. `insert` is the only writer, so the
/// order and the index cannot drift apart.
#[derive(Debug, Default)]
pub struct RecommendedActionStore {
    /// `recommendedActionId`s in record order, so a list pages over a stable
    /// sequence.
    order: Vec<String>,
    by_id: HashMap<String, RecommendedAction>,
}

impl RecommendedActionStore {
    /// Record `action` under `recommended_action_id`. Re-recording the same id
    /// replaces the stored action in place, keeping its position in the order.
    pub fn insert(&mut self, recommended_action_id: String, action: RecommendedAction) {
        if !self.by_id.contains_key(&recommended_action_id) {
            self.order.push(recommended_action_id.clone());
        }
        self.by_id.insert(recommended_action_id, action);
    }

    /// The action a `recommendedActionId` names.
    pub fn get(&self, recommended_action_id: &str) -> Option<&RecommendedAction> {
        self.by_id.get(recommended_action_id)
    }

    /// Is an action recorded under `recommended_action_id`?
    pub fn contains(&self, recommended_action_id: &str) -> bool {
        self.by_id.contains_key(recommended_action_id)
    }

    /// Every recorded action, in record order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &RecommendedAction> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// How many actions are recorded.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The EBS volume type an upgrade recommendation moves a volume TO.
///
/// `RecommendedActionType` names the two actions this service automates
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), and
/// `UpgradeEbsVolumeType`'s automation is the `ModifyEbsVolume` step
/// ([`step_types_for_event_type`]) — a volume-type change. The target is the
/// current-generation general-purpose SSD, which `EbsVolumeConfiguration$type`
/// lists among the volume types this service works with ("gp2, gp3, io1, io2,
/// st1, or sc1").
const UPGRADE_TARGET_VOLUME_TYPE: &str = "gp3";

/// The IOPS a gp3 volume carries without provisioning any: the baseline every
/// gp3 volume has.
const UPGRADE_TARGET_IOPS: i32 = 3_000;

/// The throughput in MiB/s a gp3 volume carries without provisioning any.
/// `EbsVolumeConfiguration$throughput` is "The throughput in MiB/s provisioned
/// for the volume (applicable to gp3, io1, and io2bx volumes)", so the upgraded
/// configuration is the one that names it.
const UPGRADE_TARGET_THROUGHPUT: i32 = 125;

/// The currency every estimate of this emulator's price list is denominated in.
const ESTIMATE_CURRENCY: &str = "USD";

/// Per-GiB-month storage rate of each EBS volume type, in thousandths of a
/// currency unit.
///
/// A price list is what turns a configuration into an `estimatedMonthlySavings`,
/// so the emulator carries one: integral thousandths, so an estimate is the same
/// number on every platform, over the volume types
/// `EbsVolumeConfiguration$type` names ("gp2, gp3, io1, io2, st1, or sc1"). A
/// type outside the list has no rate, so no saving is estimated for it rather
/// than a guessed one.
const EBS_STORAGE_RATES_PER_GIB_MONTH: &[(&str, i64)] = &[
    ("gp2", 100),
    ("gp3", 80),
    ("io1", 125),
    ("io2", 125),
    ("st1", 45),
    ("sc1", 15),
];

/// The per-GiB-month rate of the snapshot a `SnapshotAndDeleteUnattachedEbsVolume`
/// automation leaves behind: deleting the volume stops paying for the volume and
/// starts paying for the snapshot, so the saving is the difference.
const SNAPSHOT_STORAGE_RATE_PER_GIB_MONTH: i64 = 50;

/// How much of an estimated saving a discount takes off it, in percent.
///
/// `EstimatedMonthlySavings` requires BOTH a `beforeDiscountSavings` and an
/// `afterDiscountSavings`, so the price list needs one discount to tell them
/// apart: the after-discount saving of a discounted rate is proportionally
/// smaller than the on-demand one.
const ESTIMATE_DISCOUNT_PERCENT: i64 = 10;

/// The `SavingsEstimationMode` the inventory's estimates are computed in. The
/// figures are derived from a discounted rate, which is what
/// [`SAVINGS_MODE_AFTER_DISCOUNT`] names.
const INVENTORY_SAVINGS_ESTIMATION_MODE: &str = SAVINGS_MODE_AFTER_DISCOUNT;

/// One EBS volume of an account's analysed inventory: the volume as the analysis
/// found it, plus which of the two automations it recommends for it.
///
/// The analysis output is the same shape for every account, so the volumes are
/// described ONCE here and [`recommended_action_inventory`] renders them per
/// account.
#[derive(Debug)]
struct AnalysedVolume {
    /// The Region the volume is in, which is the action's `region`.
    region: &'static str,
    /// The volume's current `EbsVolumeConfiguration$type`.
    volume_type: &'static str,
    /// Its current `sizeInGib`.
    size_in_gib: i32,
    /// Its current `iops`.
    iops: i32,
    /// How many days of history the recommendation was drawn from.
    look_back_period_in_days: i32,
    /// Which `RecommendedActionType` the analysis recommends for it.
    recommended_action_type: &'static str,
    /// The tags the volume carries, which the `ResourceTagsKey` /
    /// `ResourceTagsValue` filters and the `resourceTag` criteria select on.
    resource_tags: &'static [(&'static str, &'static str)],
}

/// The EBS volumes this emulator's analysis reports for an enrolled account.
///
/// One row per volume the analysis has a recommendation for, spanning both
/// `RecommendedActionType`s, several Regions, several volume types and sizes and
/// several look-back periods, so every filter, criteria condition and summary
/// group of this service has something to select over for an account that has
/// done nothing but opt in.
const ANALYSED_VOLUMES: &[AnalysedVolume] = &[
    AnalysedVolume {
        region: "us-east-1",
        volume_type: "gp2",
        size_in_gib: 500,
        iops: 1_500,
        look_back_period_in_days: 14,
        recommended_action_type: EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE,
        resource_tags: &[("environment", "production"), ("team", "payments")],
    },
    AnalysedVolume {
        region: "us-east-1",
        volume_type: "io1",
        size_in_gib: 200,
        iops: 5_000,
        look_back_period_in_days: 32,
        recommended_action_type: EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE,
        resource_tags: &[("environment", "production")],
    },
    AnalysedVolume {
        region: "eu-west-1",
        volume_type: "gp2",
        size_in_gib: 150,
        iops: 450,
        look_back_period_in_days: 93,
        recommended_action_type: EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE,
        resource_tags: &[],
    },
    AnalysedVolume {
        region: "us-west-2",
        volume_type: "io2",
        size_in_gib: 300,
        iops: 8_000,
        look_back_period_in_days: 14,
        recommended_action_type: EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE,
        resource_tags: &[("environment", "staging"), ("team", "search")],
    },
    AnalysedVolume {
        region: "us-east-1",
        volume_type: "gp2",
        size_in_gib: 1_000,
        iops: 3_000,
        look_back_period_in_days: 93,
        recommended_action_type: EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME,
        resource_tags: &[("environment", "sandbox")],
    },
    AnalysedVolume {
        region: "eu-west-1",
        volume_type: "io1",
        size_in_gib: 80,
        iops: 2_400,
        look_back_period_in_days: 32,
        recommended_action_type: EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME,
        resource_tags: &[],
    },
];

/// The recommended actions this service holds for `account_id` once that account
/// is enrolled, as `(recommendedActionId, action)` pairs in analysis order.
///
/// Compute Optimizer analyses an account's resources once the account is opted
/// in to Automation, and a recommended action is what that analysis produces, so
/// the inventory belongs to the ENROLLED account rather than to the process: the
/// operations that enroll an account record it through
/// [`ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState::ensure_recommended_action_inventory`],
/// and every read path and `StartAutomationEvent` then compute from the one
/// store.
///
/// It is a PURE function of the account id and [`ANALYSED_VOLUMES`]: the same
/// account always gets the same ids, resources, configurations and estimates,
/// and two accounts never share an entry, because the identifiers are derived
/// from the account id and the volume's position in the analysis.
pub fn recommended_action_inventory(account_id: &str) -> Vec<(String, RecommendedAction)> {
    ANALYSED_VOLUMES
        .iter()
        .enumerate()
        .map(|(index, volume)| {
            // Two disjoint runs of one digest: the modelled `^[0-9A-Za-z]{16}$`
            // id, and the EBS volume id the resource is named by.
            let digest = hex::encode(Sha256::digest(
                format!("recommended-action:{account_id}:{index}").as_bytes(),
            ));
            let action_id: String = digest.chars().take(MINTED_ID_LEN).collect();
            let volume_id: String = format!(
                "vol-0{}",
                digest
                    .chars()
                    .skip(MINTED_ID_LEN)
                    .take(MINTED_ID_LEN)
                    .collect::<String>()
            );
            let upgrade = volume.recommended_action_type == EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE;
            let action = RecommendedAction {
                recommended_action_id: Some(action_id.clone()),
                // `ResourceArn`'s pattern carries the ARN's own account
                // segment, and a member account's volume belongs to THAT
                // account, so the segment is the action's account rather than
                // the one `engine::emulator_arn` fixes.
                resource_arn: Some(format!(
                    "arn:aws:ec2:{}:{account_id}:volume/{volume_id}",
                    volume.region
                )),
                resource_id: Some(volume_id),
                account_id: Some(account_id.to_string()),
                region: Some(volume.region.to_string()),
                // `ResourceType` models exactly one value, and every volume of
                // the analysis is one.
                resource_type: Some(RESOURCE_TYPE_EBS_VOLUME.to_string()),
                look_back_period_in_days: Some(volume.look_back_period_in_days),
                recommended_action_type: Some(volume.recommended_action_type.to_string()),
                current_resource_summary: Some(volume_summary(
                    volume.volume_type,
                    volume.size_in_gib,
                )),
                current_resource_details: Some(ebs_details(
                    volume.volume_type,
                    volume.size_in_gib,
                    volume.iops,
                    None,
                )),
                // An upgrade leaves a volume behind, so it has a recommended
                // configuration; a snapshot-and-delete leaves none, so it has
                // no recommended resource to describe.
                recommended_resource_summary: upgrade.then(|| {
                    volume_summary(UPGRADE_TARGET_VOLUME_TYPE, volume.size_in_gib)
                }),
                recommended_resource_details: upgrade.then(|| {
                    ebs_details(
                        UPGRADE_TARGET_VOLUME_TYPE,
                        volume.size_in_gib,
                        UPGRADE_TARGET_IOPS,
                        Some(UPGRADE_TARGET_THROUGHPUT),
                    )
                }),
                // Neither automation stops the resource it acts on: a
                // volume-type change is applied to the volume in place, and the
                // volume a snapshot-and-delete removes is an UNATTACHED one.
                restart_needed: Some(false),
                estimated_monthly_savings: estimated_monthly_savings(volume),
                resource_tags: Some(
                    volume
                        .resource_tags
                        .iter()
                        .map(|(key, value)| Tag {
                            key: (*key).to_string(),
                            value: (*value).to_string(),
                        })
                        .collect(),
                ),
            };
            (action_id, action)
        })
        .collect()
}

/// The one `ResourceType` the model declares
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const RESOURCE_TYPE_EBS_VOLUME: &str = "EbsVolume";

/// A volume configuration's one-line `currentResourceSummary` /
/// `recommendedResourceSummary`: the type and the provisioned size.
fn volume_summary(volume_type: &str, size_in_gib: i32) -> String {
    format!("{volume_type} {size_in_gib} GiB")
}

/// A volume configuration as the `ResourceDetails` union carries it.
fn ebs_details(
    volume_type: &str,
    size_in_gib: i32,
    iops: i32,
    throughput: Option<i32>,
) -> ResourceDetails {
    ResourceDetails::ebsVolume(EbsVolume {
        configuration: Some(EbsVolumeConfiguration {
            r#type: Some(volume_type.to_string()),
            size_in_gib: Some(size_in_gib),
            iops: Some(iops),
            throughput,
        }),
    })
}

/// What an analysed volume's recommendation saves per month, computed from the
/// volume's own configuration and the emulator's price list: the monthly cost of
/// what the account pays for now, less the monthly cost of what it would pay for
/// after the action.
///
/// `None` for a volume whose type the price list has no rate for, and for a
/// recommendation that would cost no less than the current configuration: an
/// estimate is a figure this service computed, so there is none rather than a
/// zero that looks computed.
fn estimated_monthly_savings(volume: &AnalysedVolume) -> Option<EstimatedMonthlySavings> {
    let current_rate = ebs_storage_rate_per_gib_month(volume.volume_type)?;
    let recommended_rate = match volume.recommended_action_type {
        EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE => {
            ebs_storage_rate_per_gib_month(UPGRADE_TARGET_VOLUME_TYPE)?
        }
        // The volume is gone and its snapshot is what the account keeps paying
        // for.
        EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME => {
            SNAPSHOT_STORAGE_RATE_PER_GIB_MONTH
        }
        _ => return None,
    };
    let saved_per_gib = current_rate.checked_sub(recommended_rate)?;
    let before_thousandths = i64::from(volume.size_in_gib).checked_mul(saved_per_gib)?;
    if before_thousandths <= 0 {
        return None;
    }
    let after_thousandths = before_thousandths * (100 - ESTIMATE_DISCOUNT_PERCENT) / 100;
    Some(EstimatedMonthlySavings {
        currency: ESTIMATE_CURRENCY.to_string(),
        before_discount_savings: before_thousandths as f64 / 1_000.0,
        after_discount_savings: after_thousandths as f64 / 1_000.0,
        savings_estimation_mode: INVENTORY_SAVINGS_ESTIMATION_MODE.to_string(),
    })
}

/// The price list's monthly rate for one provisioned GiB of `volume_type`, in
/// thousandths of a currency unit, or `None` for a type it has no rate for.
fn ebs_storage_rate_per_gib_month(volume_type: &str) -> Option<i64> {
    EBS_STORAGE_RATES_PER_GIB_MONTH
        .iter()
        .find(|(listed, _)| *listed == volume_type)
        .map(|(_, rate)| *rate)
}

/// This action as the `PreviewResult` `ListAutomationRulePreview` reports. The
/// model's `PreviewResult` repeats every member of `RecommendedAction`, so the
/// preview is the stored action itself, projected member for member.
pub fn preview_result_for(action: &RecommendedAction) -> PreviewResult {
    PreviewResult {
        recommended_action_id: action.recommended_action_id.clone(),
        resource_arn: action.resource_arn.clone(),
        resource_id: action.resource_id.clone(),
        account_id: action.account_id.clone(),
        region: action.region.clone(),
        resource_type: action.resource_type.clone(),
        look_back_period_in_days: action.look_back_period_in_days,
        recommended_action_type: action.recommended_action_type.clone(),
        current_resource_summary: action.current_resource_summary.clone(),
        current_resource_details: action.current_resource_details.clone(),
        recommended_resource_summary: action.recommended_resource_summary.clone(),
        recommended_resource_details: action.recommended_resource_details.clone(),
        restart_needed: action.restart_needed,
        estimated_monthly_savings: action.estimated_monthly_savings.clone(),
        resource_tags: action.resource_tags.clone(),
    }
}

/// The recommended actions ONE rule-preview request selects, in the order the
/// store holds them: every action the caller may retrieve
/// ([`ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState::retrievable_actions`])
/// that belongs to an account `scope` covers, whose `recommendedActionType` is
/// one of `recommended_action_types`, and that satisfies every condition of
/// `criteria`.
///
/// A preview request names its selection the same way whichever preview
/// operation reads it — `ListAutomationRulePreview` reports the selected actions
/// one by one and `ListAutomationRulePreviewSummaries` aggregates the very same
/// set — so the selection lives here once. An action that records no
/// `accountId` belongs to no account, so no scope covers it.
///
/// The scope a request NAMES is narrowed by the accounts the caller actually
/// reaches: a preview is what the rule would act on, and a rule is only ever
/// stored for accounts the caller may automate in
/// ([`ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState::may_scope_rule_to`]),
/// so a scope naming an account whose association has gone previews nothing of
/// that account.
pub fn previewed_actions<'store>(
    state: &'store ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState,
    hierarchy: &'store OrganizationHierarchy,
    scope: &'store AccountScope,
    recommended_action_types: &'store [String],
    criteria: &'store Criteria,
) -> impl Iterator<Item = &'store RecommendedAction> + 'store {
    state
        .retrievable_actions(hierarchy)
        .filter(move |action| {
            action
                .account_id
                .as_deref()
                .is_some_and(|account_id| scope.covers(hierarchy, account_id))
        })
        .filter(move |action| {
            action
                .recommended_action_type
                .as_ref()
                .is_some_and(|action_type| recommended_action_types.contains(action_type))
        })
        .filter(move |action| action_matches_criteria(action, criteria))
}

/// Why this preview request's selection cannot be evaluated, if it cannot, as
/// the service's own validation report ([`constraint_report`], which echoes no
/// request value).
///
/// The selection a preview request names is read by both preview operations
/// ([`previewed_actions`]), so what makes a selection refusable is judged here
/// once as well: a request one preview operation cannot evaluate is one the
/// other cannot evaluate either, and the two cannot drift apart over which
/// requests they refuse or how they word the refusal. The clauses are ordered
/// from the required member outwards: the list of action types to include, then
/// the scope that member list is read in, then the conditions applied inside it.
///
/// * an EMPTY `recommendedActionTypes` selects no action type at all
///   ([`recommended_action_types_violation`]);
/// * an `organizationScope` is the set of accounts of an ORGANIZATION-wide
///   preview, so it names nothing for a rule type scoped to one account, and
///   silently ignoring a member the caller sent would be worse than reporting
///   it;
/// * a `criteria` this service cannot evaluate ([`criteria_violation`]).
pub fn preview_selection_violation(
    rule_type: &str,
    organization_scope: Option<&OrganizationScope>,
    recommended_action_types: &[String],
    criteria: Option<&Criteria>,
) -> Option<String> {
    if let Some(report) = recommended_action_types_violation(recommended_action_types) {
        return Some(report);
    }
    if rule_type != "OrganizationRule" && organization_scope.is_some() {
        return Some(constraint_report(
            "organizationScope",
            "Member is valid only for the OrganizationRule rule type",
        ));
    }
    criteria.and_then(criteria_violation)
}

/// The fingerprint a preview page token carries: the operation that issued it
/// and the whole selection it was taken with.
///
/// Bound into the token ([`PageCursor::filter_fingerprint`]) so that resuming a
/// walk with a different account scope, action-type list or criteria — or with
/// the other preview operation's token — is refused instead of answered with a
/// page of a different result set.
///
/// What it digests is the selection as [`previewed_actions`] PERFORMS it: the
/// accounts the rule type and `organizationScope` derive to
/// ([`preview_account_scope`]) as the SET of accounts they cover, the requested
/// action types as the SET the selection tests membership of
/// ([`canonical_membership`] for both), and the criteria after defaulting and
/// with every criterion that constrains nothing spelled one way
/// ([`canonical_criteria`]) — never the raw optional request members. Two
/// spellings of one selection therefore digest to one fingerprint and continue
/// one walk (`criteria` absent, `criteria: {}` and a criterion holding no
/// condition all constrain nothing; a scope carrying no `accountIds` covers the
/// same accounts as no scope at all; a reordered or repeat-carrying action-type
/// or account list covers the same accounts and types), while a selection that
/// really is different digests differently. The two preview operations are
/// handed the same values they select by, so they cannot disagree about which
/// requests continue one walk, and the normalisation matches every other
/// paginated listing of this service, which digests its optional query member
/// after `unwrap_or_default` and its filter list as the selection it performs
/// ([`canonical_filters`]).
pub fn preview_selection_fingerprint(
    operation: &str,
    scope: &AccountScope,
    recommended_action_types: &[String],
    criteria: &Criteria,
) -> String {
    page_fingerprint(
        operation,
        &(
            scope.canonical(),
            canonical_membership(recommended_action_types),
            canonical_criteria(criteria),
        ),
    )
}

/// `criteria` with every criterion that constrains nothing spelled the ONE way
/// the selection reads it: absent.
///
/// A criterion the caller left out and a criterion present but holding no
/// condition select identically — [`action_matches_criteria`] requires every
/// condition of a criterion to hold, and a criterion with no condition has
/// nothing to fail — so the two spellings name one selection and digest to one
/// fingerprint. Only a criterion spelled as nothing is normalised: a criterion
/// that carries conditions is digested as the request spells it, so a selection
/// that constrains anything differently is still a different fingerprint.
fn canonical_criteria(criteria: &Criteria) -> Criteria {
    let mut canonical = criteria.clone();
    drop_unconstraining(&mut canonical.region);
    drop_unconstraining(&mut canonical.resource_arn);
    drop_unconstraining(&mut canonical.ebs_volume_type);
    drop_unconstraining(&mut canonical.ebs_volume_size_in_gib);
    drop_unconstraining(&mut canonical.estimated_monthly_savings);
    drop_unconstraining(&mut canonical.resource_tag);
    drop_unconstraining(&mut canonical.look_back_period_in_days);
    drop_unconstraining(&mut canonical.restart_needed);
    canonical
}

/// Spell a criterion that holds no condition the way an absent one is spelled.
fn drop_unconstraining<T>(criterion: &mut Option<Vec<T>>) {
    if criterion
        .as_ref()
        .is_some_and(|conditions| conditions.is_empty())
    {
        *criterion = None;
    }
}

/// One group of summarised items a summaries list reports as a row — of
/// recommended actions, or of automation events: the grouping key and what the
/// items under it accumulate.
///
/// An item Compute Optimizer has not estimated savings for carries no
/// estimate, which adds nothing to the group's savings and is still one item
/// of its count.
#[derive(Debug, Default, Clone)]
pub struct ActionGroup {
    /// The value of the grouping dimension this group holds, which is the row's
    /// `key`.
    pub key: String,
    /// How many items the group holds, estimate or not.
    pub count: i32,
    /// The estimates the group's items carry, which [`total_savings`] reduces
    /// to the group's `estimatedMonthlySavings`.
    pub estimates: Vec<EstimatedMonthlySavings>,
}

/// The items of `items`, grouped by the key `key` reads off each of them: one
/// group per distinct key the set holds, in the order the set first holds an
/// item carrying it, each group tallying how many items it holds and the
/// estimates `estimate` reads off them.
///
/// The ONE grouping every summaries list of this service that reports a row per
/// DIMENSION VALUE uses — the automation-event summaries, whose dimension the
/// model names: such a summary row is one group of the dimension it is grouped
/// by, so the rows are the distinct values the
/// summarised items carry for it, never a key for a dimension nothing was
/// grouped by. The summarised items are what gets grouped, so a value nothing
/// matched is no group and an item the store records no value for belongs to no
/// group.
///
/// The groups are kept as a SEQUENCE, so a group's key and its tally are one
/// value: there is no second lookup that could miss and report a plausible zero
/// total. The number of groups is bounded by the value set of the dimension
/// grouped by, so finding one by key is a walk over a handful of entries.
fn group_summary_rows<'a, T: 'a>(
    items: impl Iterator<Item = &'a T>,
    key: impl Fn(&'a T) -> Option<&'a str>,
    estimate: impl Fn(&'a T) -> Option<&'a EstimatedMonthlySavings>,
) -> Vec<ActionGroup> {
    let mut grouped: Vec<ActionGroup> = Vec::new();
    for item in items {
        let Some(group_key) = key(item) else {
            continue;
        };
        let index = match grouped.iter().position(|group| group.key == group_key) {
            Some(index) => index,
            None => {
                grouped.push(ActionGroup {
                    key: group_key.to_string(),
                    ..ActionGroup::default()
                });
                grouped.len().saturating_sub(1)
            }
        };
        if let Some(group) = grouped.get_mut(index) {
            group.count = group.count.saturating_add(1);
            if let Some(savings) = estimate(item) {
                group.estimates.push(savings.clone());
            }
        }
    }
    grouped
}

/// The automation events of `events`, grouped by `eventStatus`: one group per
/// status the set holds, in the order the set first holds an event in it.
///
/// `EventStatus` is the one dimension an automation-event summary is grouped by,
/// because it is the one value the model's `SummaryDimensionKey` declares
/// (`smithy_query(query="shape",shape="SummaryDimensionKey")`, generated as
/// `SUMMARY_DIMENSION_KEY_VALUES`). Every stored event carries a status, so no
/// event is left out of the grouping; the rule itself is
/// [`group_summary_rows`]'s.
pub fn group_events_by_event_status<'a>(
    events: impl Iterator<Item = &'a StoredEvent>,
) -> Vec<ActionGroup> {
    group_summary_rows(
        events,
        |event| Some(event.event_status.as_str()),
        |event| event.estimated_monthly_savings.as_ref(),
    )
}

/// The `SummaryDimensionKey` an automation-event summary names: the dimension
/// its rows are grouped by, which is the single value that shape declares
/// (generated as `SUMMARY_DIMENSION_KEY_VALUES`).
pub const SUMMARY_DIMENSION_EVENT_STATUS: &str = "EventStatus";

/// The `key` every recommended-action summary row carries: the fixed text that
/// names the ACCOUNT-WIDE aggregate of a selection, rather than a value read off
/// the items summarised.
///
/// A recommended-action summary listing reports one row per requested
/// selection — the total over everything that selection holds — so the key is
/// the same literal for every such row and is derived from no request member.
pub const ALL_RECOMMENDED_ACTION_SUMMARY_KEY: &str = "ALL_Recommended_Action_Summary";

/// The ONE summary row a recommended-action summary listing reports for a
/// selection: the aggregate of `actions`, keyed
/// [`ALL_RECOMMENDED_ACTION_SUMMARY_KEY`].
///
/// A recommended-action summary is the account-wide total of the selection the
/// request names, not a breakdown of it: one row, counting EVERY selected action
/// (so the count equals the number of actions the unsummarised listing reports
/// for the same selection) and carrying every estimate those actions hold, which
/// [`total_savings`] reduces to the row's `estimatedMonthlySavings`. The row is
/// reported even for a selection that holds nothing — an account with nothing to
/// recommend still has a total, which is zero — so a caller can always read the
/// total without paging the items.
pub fn aggregate_recommended_actions<'a>(
    actions: impl Iterator<Item = &'a RecommendedAction>,
) -> Vec<ActionGroup> {
    let mut aggregate = ActionGroup {
        key: ALL_RECOMMENDED_ACTION_SUMMARY_KEY.to_string(),
        ..ActionGroup::default()
    };
    for action in actions {
        aggregate.count = aggregate.count.saturating_add(1);
        if let Some(savings) = action.estimated_monthly_savings.as_ref() {
            aggregate.estimates.push(savings.clone());
        }
    }
    vec![aggregate]
}

/// The `estimatedMonthlySavings` of a GROUP of recommended actions: the sums of
/// the before- and after-discount figures of the estimates the group carries.
///
/// An action Compute Optimizer has not estimated savings for carries no
/// estimate, which adds nothing to either sum (and is still one action of the
/// group, so the group's count is not this function's business).
///
/// * `currency` is the currency the group's estimates AGREE on. The model types
///   it as a free-form `String`, so a group holding no estimate — or estimates
///   in different currencies, whose amounts are not one currency's total —
///   names no currency: the empty string.
/// * `savingsEstimationMode` is typed as the `SavingsEstimationMode` enum
///   (`BeforeDiscount | AfterDiscount`), so a total must name one of the two
///   even when the group's estimates disagree or carry none. It reports the mode
///   the group agrees on, else `BeforeDiscount`, the conservative reading this
///   module already takes for an estimate whose mode it cannot place (see
///   `savings_for_criteria`).
pub fn total_savings<'a>(
    estimates: impl Iterator<Item = &'a EstimatedMonthlySavings>,
) -> EstimatedMonthlySavings {
    let mut total = EstimatedMonthlySavings {
        currency: String::new(),
        before_discount_savings: 0.0,
        after_discount_savings: 0.0,
        savings_estimation_mode: SAVINGS_MODE_BEFORE_DISCOUNT.to_string(),
    };
    let mut currency: Option<&str> = None;
    let mut mode: Option<&str> = None;
    let mut agreed_currency = true;
    let mut agreed_mode = true;
    for estimate in estimates {
        total.before_discount_savings += estimate.before_discount_savings;
        total.after_discount_savings += estimate.after_discount_savings;
        match currency {
            None => currency = Some(estimate.currency.as_str()),
            Some(seen) if seen != estimate.currency => agreed_currency = false,
            Some(_) => {}
        }
        match mode {
            None => mode = Some(estimate.savings_estimation_mode.as_str()),
            Some(seen) if seen != estimate.savings_estimation_mode => agreed_mode = false,
            Some(_) => {}
        }
    }
    if agreed_currency {
        if let Some(currency) = currency {
            total.currency = currency.to_string();
        }
    }
    if agreed_mode {
        // A stored mode outside the model's `SavingsEstimationMode` value set
        // cannot be put on the wire as that enum, so it reads as the
        // conservative before-discount mode the default already names.
        if let Some(mode) = mode.filter(|mode| SAVINGS_ESTIMATION_MODE_VALUES.contains(mode)) {
            total.savings_estimation_mode = mode.to_string();
        }
    }
    total
}

/// The `SavingsEstimationMode` a total names when the group's estimates do not
/// agree on one, carry none, or name one the model does not declare: the
/// before-discount reading, which is the conservative one (see
/// [`total_savings`]).
pub const SAVINGS_MODE_BEFORE_DISCOUNT: &str = "BeforeDiscount";

/// The other `SavingsEstimationMode` the model declares: the figure AFTER any
/// discounts.
pub const SAVINGS_MODE_AFTER_DISCOUNT: &str = "AfterDiscount";

/// The EBS volume configuration carried by `details`, which is what the
/// `ebsVolumeType` and `ebsVolumeSizeInGib` criteria read off the CURRENT
/// resource: the criteria choose which resources a rule acts on, so they read
/// the resource as it is now, never the recommended one.
fn ebs_configuration(details: Option<&ResourceDetails>) -> Option<&EbsVolumeConfiguration> {
    match details? {
        ResourceDetails::ebsVolume(volume) => volume.configuration.as_ref(),
    }
}

/// The value an `estimatedMonthlySavings` criterion compares against: the figure
/// for the estimate's OWN `savingsEstimationMode`, so a `BeforeDiscount`
/// estimate is compared before discounts and an `AfterDiscount` one after them.
fn savings_for_criteria(savings: &EstimatedMonthlySavings) -> f64 {
    match savings.savings_estimation_mode.as_str() {
        SAVINGS_MODE_AFTER_DISCOUNT => savings.after_discount_savings,
        // `BeforeDiscount` is the enum's only other member, and the
        // before-discount figure is the conservative reading of a mode this
        // model does not yet declare.
        _ => savings.before_discount_savings,
    }
}

/// The EBS volume type of `details`, for the
/// `CurrentResourceDetailsEbsVolumeType` filter. `None` when the action carries
/// no such configuration.
fn ebs_volume_type(details: Option<&ResourceDetails>) -> Option<String> {
    ebs_configuration(details)?.r#type.clone()
}

/// The steps an automation event of `event_type` performs, in the order it
/// performs them.
///
/// `EventType` is `SnapshotAndDeleteUnattachedEbsVolume | UpgradeEbsVolumeType`
/// and `StepType` is `CreateEbsSnapshot | CreateEbsVolume | DeleteEbsVolume |
/// ModifyEbsVolume` (`smithy_query(query="shape",shape="EventType,StepType")`).
/// An event type names the work its automation does, and its steps are exactly
/// that work in the order the name states it: a snapshot-and-delete takes the
/// snapshot ([`STEP_TYPE_CREATE_EBS_SNAPSHOT`]) and then deletes the volume
/// ([`STEP_TYPE_DELETE_EBS_VOLUME`]), and a volume-type upgrade modifies the
/// volume in place ([`STEP_TYPE_MODIFY_EBS_VOLUME`]). `CreateEbsVolume` belongs
/// to the ROLLBACK of a delete (restoring the volume from the snapshot the
/// first step took), which no forward run performs.
///
/// An event type outside the modelled set has no known step sequence, so it
/// gets none rather than a guessed one.
pub fn step_types_for_event_type(event_type: &str) -> &'static [&'static str] {
    match event_type {
        EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME => {
            &[STEP_TYPE_CREATE_EBS_SNAPSHOT, STEP_TYPE_DELETE_EBS_VOLUME]
        }
        EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE => &[STEP_TYPE_MODIFY_EBS_VOLUME],
        _ => &[],
    }
}

/// The steps the ROLLBACK of an automation event of `event_type` performs, in
/// the order it performs them: the work that undoes what
/// [`step_types_for_event_type`] did, named with the same `StepType` value set.
///
/// Undoing a snapshot-and-delete means putting the volume back, which is
/// [`STEP_TYPE_CREATE_EBS_VOLUME`] — the step type no forward run performs,
/// restoring the volume from the snapshot the forward run took, which the
/// rollback leaves in place. Undoing a volume-type upgrade changes the volume
/// back in place, which is the same [`STEP_TYPE_MODIFY_EBS_VOLUME`] the upgrade
/// performed. An event type outside the modelled set has no known step sequence
/// forwards, so it has none backwards either.
pub fn rollback_step_types_for_event_type(event_type: &str) -> &'static [&'static str] {
    match event_type {
        EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME => &[STEP_TYPE_CREATE_EBS_VOLUME],
        EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE => &[STEP_TYPE_MODIFY_EBS_VOLUME],
        _ => &[],
    }
}

/// The `InvalidParameterValueException` message for an event no rollback may be
/// initiated for. States the RULE — which status a rollback starts from, which
/// is the one fact the caller needs — and echoes neither the event the request
/// named nor the status that event is in.
pub fn rollback_not_allowed_message() -> String {
    format!(
        "A rollback can only be initiated for an automation event in the \
         {EVENT_STATUS_COMPLETE} status"
    )
}

/// One step of an automation event as stored, which is what
/// `ListAutomationEventSteps` reports. A step belongs to exactly one event and
/// is stored with it, so an evicted event takes its steps with it and no reader
/// sees a step whose event is gone.
#[derive(Debug, Clone)]
pub struct StoredStep {
    pub step_id: String,
    pub step_type: String,
    pub step_status: String,
    pub resource_id: Option<String>,
    pub start_timestamp: Option<::aws_smithy_types::DateTime>,
    pub completed_timestamp: Option<::aws_smithy_types::DateTime>,
    pub estimated_monthly_savings: Option<EstimatedMonthlySavings>,
}

impl StoredStep {
    /// This step as the `AutomationEventStep` the list operation returns.
    /// `eventId` is the owning event's, because a step is only ever reached
    /// through it.
    pub fn to_automation_event_step(&self, event_id: &str) -> AutomationEventStep {
        AutomationEventStep {
            event_id: Some(event_id.to_string()),
            step_id: Some(self.step_id.clone()),
            step_type: Some(self.step_type.clone()),
            step_status: Some(self.step_status.clone()),
            resource_id: self.resource_id.clone(),
            start_timestamp: self.start_timestamp,
            completed_timestamp: self.completed_timestamp,
            estimated_monthly_savings: self.estimated_monthly_savings.clone(),
        }
    }
}

/// The `stepId` of step `index` of the event `event_id`: the leading characters
/// of a digest over the pair. Derived rather than drawn from a counter so a
/// step's id is fixed when its event is created and no read ever mints one, and
/// it satisfies the model's `StepId` pattern `^[0-9A-Za-z]{16}$` by
/// construction.
fn step_id(event_id: &str, index: usize) -> String {
    let digest = Sha256::digest(format!("{event_id}:step:{index}").as_bytes());
    hex::encode(digest).chars().take(MINTED_ID_LEN).collect()
}

/// The steps of a freshly started event: the sequence its event type performs,
/// with the FIRST one running — the event is [`EVENT_STATUS_IN_PROGRESS`] and
/// that is the step it is on — and every later one queued
/// [`STEP_STATUS_READY`]. A step that has not started carries no timestamp.
pub fn steps_for_started_event(
    event_id: &str,
    event_type: &str,
    resource_id: Option<&str>,
    estimated_monthly_savings: Option<&EstimatedMonthlySavings>,
    started_at: ::aws_smithy_types::DateTime,
) -> Vec<StoredStep> {
    let step_types = step_types_for_event_type(event_type);
    step_types
        .iter()
        .enumerate()
        .map(|(index, step_type)| StoredStep {
            step_id: step_id(event_id, index),
            step_type: (*step_type).to_string(),
            step_status: if index == 0 {
                STEP_STATUS_IN_PROGRESS.to_string()
            } else {
                STEP_STATUS_READY.to_string()
            },
            // Every step of an event acts on the resource the event automates.
            resource_id: resource_id.map(str::to_string),
            start_timestamp: (index == 0).then_some(started_at),
            completed_timestamp: None,
            // The event's estimate is realised by the step that changes what
            // the resource costs — the LAST of the sequence, which deletes or
            // modifies the volume — so only that step carries it and summing
            // the steps of an event never double-counts its estimate.
            estimated_monthly_savings: (index + 1 == step_types.len())
                .then(|| estimated_monthly_savings.cloned())
                .flatten(),
        })
        .collect()
}

/// An automation event as stored: the run of ONE recommended action, which is
/// what `ListAutomationEvents` and the event readers report. Every response
/// member is built from one of these, never from the request that asked for it.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: String,
    pub event_description: Option<String>,
    pub event_type: String,
    pub event_status: String,
    pub event_status_reason: Option<String>,
    pub resource_arn: Option<String>,
    pub resource_id: Option<String>,
    pub recommended_action_id: Option<String>,
    pub account_id: String,
    pub region: Option<String>,
    pub rule_id: Option<String>,
    pub resource_type: String,
    pub created_timestamp: ::aws_smithy_types::DateTime,
    pub completed_timestamp: Option<::aws_smithy_types::DateTime>,
    pub estimated_monthly_savings: Option<EstimatedMonthlySavings>,
    /// The steps this event's automation performs, in the order it performs
    /// them (see [`steps_for_started_event`]). Stored WITH the event because a
    /// step is only ever reached through its event.
    pub steps: Vec<StoredStep>,
    /// The status transition the clock owes this event, or `None` when it owes
    /// none (it is already terminal).
    pub pending: Option<PendingTransition>,
}

/// A status transition the clock owes an automation event: the terminal
/// `EventStatus` the event reaches, when it falls due, and where in the event's
/// step sequence the run that is still in flight begins.
///
/// Stored as an instant rather than slept for, so no handler blocks on it, and
/// it carries the terminal status because an event has more than one run in its
/// life: the automation itself settles to [`EVENT_STATUS_COMPLETE`], and the
/// rollback of that automation to [`EVENT_STATUS_ROLLBACK_COMPLETE`].
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// When the terminal status falls due.
    pub due: SystemTime,
    /// The terminal `EventStatus` the event reaches then.
    pub terminal_status: String,
    /// The index of the first step of the run in flight. The steps from here on
    /// share the run window that ends at `due`; the ones before it belong to a
    /// run that is already over and keep the timestamps they were stamped with.
    pub first_step: usize,
}

impl StoredEvent {
    /// Materialise the status the clock has already made due.
    ///
    /// The transition was owed at its due instant whether or not anybody called, so
    /// applying it on a read reports what the event already is rather than
    /// advancing it because it was read. A no-op before the due instant and
    /// after the event has settled.
    ///
    /// A run the service accepted performs its steps and then ENDS: the event
    /// reaches the terminal status that run settles to (see
    /// [`PendingTransition`]), carrying the instant it ended and no
    /// `eventStatusReason` — the reason member explains a status that needs
    /// explaining, which neither a completion nor a completed rollback does, and
    /// [`EVENT_STATUS_FAILED`] stays reserved for an automation that could not be
    /// carried out. Every step of the run is terminal too
    /// ([`STEP_STATUS_COMPLETE`]), stamped with the slice of the run window it
    /// occupied so the steps read back in the order the event performed them.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.event_status = pending.terminal_status;
        self.event_status_reason = None;
        self.completed_timestamp = Some(::aws_smithy_types::DateTime::from(pending.due));
        self.settle_steps(pending.due, pending.first_step);
        self.pending = None;
    }

    /// May a rollback be initiated for this event?
    ///
    /// `RollbackAutomationEvent` "Initiates a rollback for a COMPLETED
    /// automation event", so the automation has to have been carried out:
    /// [`EVENT_STATUS_COMPLETE`] is the one status a rollback starts from. An
    /// event whose run is not over, or that has already been rolled back, is in
    /// no such state.
    pub fn rollback_can_start(&self) -> bool {
        self.event_status == EVENT_STATUS_COMPLETE
    }

    /// Start the rollback of this completed automation at `at`.
    ///
    /// The event goes back to running — the non-terminal
    /// [`EVENT_STATUS_ROLLBACK_IN_PROGRESS`], with no completion instant, since
    /// the instant it completed its automation is not when its rollback ends —
    /// and the terminal [`EVENT_STATUS_ROLLBACK_COMPLETE`] falls due
    /// [`EVENT_SETTLEMENT_DELAY`] later. The rollback's steps
    /// ([`rollback_step_types_for_event_type`]) are APPENDED to the ones the
    /// forward run performed, with the first of them running and every later one
    /// queued, so a reader sees the whole history of the event in the order it
    /// happened and the forward steps keep the timestamps they were stamped
    /// with.
    pub fn begin_rollback(&mut self, at: SystemTime) {
        let first_step = self.steps.len();
        let started_at = ::aws_smithy_types::DateTime::from(at);
        let step_types = rollback_step_types_for_event_type(&self.event_type);
        for (offset, step_type) in step_types.iter().enumerate() {
            self.steps.push(StoredStep {
                step_id: step_id(&self.event_id, first_step + offset),
                step_type: (*step_type).to_string(),
                step_status: if offset == 0 {
                    STEP_STATUS_IN_PROGRESS.to_string()
                } else {
                    STEP_STATUS_READY.to_string()
                },
                // Every step of a rollback acts on the resource the event
                // automated, exactly as the forward steps did.
                resource_id: self.resource_id.clone(),
                start_timestamp: (offset == 0).then_some(started_at),
                completed_timestamp: None,
                // A rollback undoes the change the automation made, so it
                // realises none of the event's estimated monthly savings.
                estimated_monthly_savings: None,
            });
        }
        self.event_status = EVENT_STATUS_ROLLBACK_IN_PROGRESS.to_string();
        self.event_status_reason = None;
        self.completed_timestamp = None;
        self.pending = Some(PendingTransition {
            due: at + EVENT_SETTLEMENT_DELAY,
            terminal_status: EVENT_STATUS_ROLLBACK_COMPLETE.to_string(),
            first_step,
        });
    }

    /// Stamp every step of a completed run as terminal.
    ///
    /// The steps of a run are a SEQUENCE ([`steps_for_started_event`]), so they
    /// share the run window that ends at `due`: step `i` of the `n` steps from
    /// `first_step` on occupies the `i`-th of its `n` equal slices, and the last
    /// one ends exactly when the run did. A step that was already running keeps
    /// the start timestamp it was stamped with, so settling never rewrites when
    /// the run began — and a step never ends before it started, whatever that
    /// stored start is. The steps BEFORE `first_step` belong to an earlier run
    /// of the same event and are left exactly as they are.
    fn settle_steps(&mut self, due: SystemTime, first_step: usize) {
        let total = self.steps.len().saturating_sub(first_step);
        let Ok(slices) = u32::try_from(total) else {
            return;
        };
        if slices == 0 {
            return;
        }
        let window_start = due.checked_sub(EVENT_SETTLEMENT_DELAY).unwrap_or(due);
        let slice = EVENT_SETTLEMENT_DELAY / slices;
        let completed = ::aws_smithy_types::DateTime::from(due);
        let boundary = |slices_in: u32| {
            window_start
                .checked_add(slice * slices_in)
                .map_or(completed, ::aws_smithy_types::DateTime::from)
        };
        for (position, step) in (0u32..slices).zip(self.steps.iter_mut().skip(first_step)) {
            step.step_status = STEP_STATUS_COMPLETE.to_string();
            let start = *step.start_timestamp.get_or_insert_with(|| boundary(position));
            let end = if position + 1 == slices {
                completed
            } else {
                boundary(position + 1)
            };
            step.completed_timestamp = Some(if end.as_secs_f64() < start.as_secs_f64() {
                start
            } else {
                end
            });
        }
    }

    /// This event as the `AutomationEvent` the list and get operations return.
    pub fn to_automation_event(&self) -> AutomationEvent {
        AutomationEvent {
            event_id: Some(self.event_id.clone()),
            event_description: self.event_description.clone(),
            event_type: Some(self.event_type.clone()),
            event_status: Some(self.event_status.clone()),
            event_status_reason: self.event_status_reason.clone(),
            resource_arn: self.resource_arn.clone(),
            resource_id: self.resource_id.clone(),
            recommended_action_id: self.recommended_action_id.clone(),
            account_id: Some(self.account_id.clone()),
            region: self.region.clone(),
            rule_id: self.rule_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            created_timestamp: Some(self.created_timestamp),
            completed_timestamp: self.completed_timestamp,
            estimated_monthly_savings: self.estimated_monthly_savings.clone(),
        }
    }
}

/// The automation events of this service, in creation order, keyed by the
/// `eventId` every event-scoped operation takes. Keeping the order beside the
/// index is what lets a list page over a stable sequence; `insert` is the only
/// writer, and it is what keeps the store bounded by [`MAX_STORED_EVENTS`].
#[derive(Debug, Default)]
pub struct EventStore {
    /// `eventId`s in creation order, oldest first, so the store can evict.
    order: VecDeque<String>,
    by_id: HashMap<String, StoredEvent>,
}

impl EventStore {
    /// Store an event, appending it to the creation order the first time its id
    /// is seen so a re-stored event keeps its place in the sequence. Evicts the
    /// oldest events once the store is full: an evicted event is the one no
    /// query can still reach first.
    pub fn insert(&mut self, event: StoredEvent) {
        let event_id = event.event_id.clone();
        if self.by_id.insert(event_id.clone(), event).is_none() {
            self.order.push_back(event_id);
        }
        while self.order.len() > MAX_STORED_EVENTS {
            if let Some(oldest) = self.order.pop_front() {
                self.by_id.remove(&oldest);
            }
        }
    }

    /// Every stored event, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &StoredEvent> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// The event an `eventId` names, which is how every event-scoped operation
    /// reaches one.
    pub fn get(&self, event_id: &str) -> Option<&StoredEvent> {
        self.by_id.get(event_id)
    }

    /// As [`get`](Self::get), for a caller that has to materialise the status
    /// the clock owes the event (see [`StoredEvent::settle`]).
    pub fn get_mut(&mut self, event_id: &str) -> Option<&mut StoredEvent> {
        self.by_id.get_mut(event_id)
    }

    /// The event an `eventId` names, as a reader that REPORTS it reaches one:
    /// only while the event is still retrievable.
    ///
    /// An event created before the retrieval window ([`EVENT_RETRIEVAL_WINDOW`]
    /// before `now`) is outside every query's reach, so a per-id reader answers
    /// for it exactly as it answers for an id this store never held — nothing
    /// a listing has dropped is reportable through an id. The window is checked
    /// BEFORE the status transition the clock owes is materialised, so an
    /// unreachable event is not written to on the way to being refused; a
    /// reachable one is settled, so every reader of this store reports the same
    /// status for it ([`StoredEvent::settle`]).
    pub fn retrievable_mut(
        &mut self,
        event_id: &str,
        now: SystemTime,
    ) -> Option<&mut StoredEvent> {
        let window_start = effective_window_start(None, now);
        let event = self.by_id.get_mut(event_id)?;
        if !within_window(event.created_timestamp, Some(window_start), None) {
            return None;
        }
        event.settle(now);
        Some(event)
    }

    /// Materialise every status transition the clock has already made due, so
    /// each reader of this store reports the same status for an event (see
    /// [`StoredEvent::settle`]). A no-op for an event that owes none.
    pub fn settle_due(&mut self, now: SystemTime) {
        for event in self.by_id.values_mut() {
            event.settle(now);
        }
    }

    pub fn contains(&self, event_id: &str) -> bool {
        self.by_id.contains_key(event_id)
    }
}

/// A newly started automation event for `action`, owned by `account_id`.
///
/// The event is built from the stored recommended action — never from the
/// request — and starts in the non-terminal [`EVENT_STATUS_IN_PROGRESS`] with
/// [`EVENT_STATUS_COMPLETE`] due [`EVENT_SETTLEMENT_DELAY`] later. Its steps are the
/// sequence its event type performs ([`steps_for_started_event`]), stored at
/// start so every later read reports the same steps with the same ids.
pub fn event_for_action(
    event_id: String,
    action: &RecommendedAction,
    account_id: String,
    started_at: SystemTime,
) -> StoredEvent {
    let event_type = action.recommended_action_type.clone().unwrap_or_default();
    let created_timestamp = ::aws_smithy_types::DateTime::from(started_at);
    let steps = steps_for_started_event(
        &event_id,
        &event_type,
        action.resource_id.as_deref(),
        action.estimated_monthly_savings.as_ref(),
        created_timestamp,
    );
    StoredEvent {
        event_id,
        event_description: None,
        // `EventType` and `RecommendedActionType` carry the same value set, and
        // the event automates exactly this action
        // (`smithy_query(query="shape",shape="EventType,RecommendedActionType")`).
        event_type,
        event_status: EVENT_STATUS_IN_PROGRESS.to_string(),
        event_status_reason: None,
        resource_arn: action.resource_arn.clone(),
        resource_id: action.resource_id.clone(),
        recommended_action_id: action.recommended_action_id.clone(),
        account_id,
        region: action.region.clone(),
        // The rule whose schedule started the event; an on-demand
        // `StartAutomationEvent` has none.
        rule_id: None,
        resource_type: action.resource_type.clone().unwrap_or_default(),
        created_timestamp,
        completed_timestamp: None,
        estimated_monthly_savings: action.estimated_monthly_savings.clone(),
        steps,
        pending: Some(PendingTransition {
            due: started_at + EVENT_SETTLEMENT_DELAY,
            terminal_status: EVENT_STATUS_COMPLETE.to_string(),
            // The automation's own steps are the event's first run.
            first_step: 0,
        }),
    }
}

/// Which accounts a preview covers.
///
/// An `OrganizationRule` preview is scoped by its `organizationScope` — the
/// accounts and OUs it includes, less the accounts and OUs it excludes
/// ([`AccountScope`]) — and one that names no scope at all spans the
/// organization. An `AccountRule` covers only the calling account, because a
/// rule of that type is scoped to the account it lives in.
///
/// An EMPTY list cannot reach here: the model constrains every list member of
/// `OrganizationScope` at `length(min: 1, …)`, so the protocol boundary refuses
/// `accountIds: []` before the handler runs. A scope present with no member at
/// all is the same as no scope at all.
pub fn preview_account_scope(
    rule_type: &str,
    organization_scope: Option<&OrganizationScope>,
) -> AccountScope {
    if rule_type != "OrganizationRule" {
        return AccountScope::of_accounts(vec![caller_account_id()]);
    }
    let Some(scope) = organization_scope else {
        return AccountScope::default();
    };
    AccountScope {
        account_ids: scope.account_ids.clone(),
        organizational_unit_ids: scope.organizational_unit_ids.clone(),
        excluded_account_ids: scope.excluded_account_ids.clone().unwrap_or_default(),
        excluded_organizational_unit_ids: scope
            .excluded_organizational_unit_ids
            .clone()
            .unwrap_or_default(),
    }
}

/// The accounts an organization rule's `organizationConfiguration` scopes it to:
/// the same four members under the same documented rules as a preview's
/// `organizationScope` ([`preview_account_scope`]), so a rule and the preview of
/// that rule cover the same accounts.
pub fn rule_account_scope(configuration: Option<&OrganizationConfiguration>) -> AccountScope {
    let Some(configuration) = configuration else {
        return AccountScope::default();
    };
    AccountScope {
        account_ids: configuration.account_ids.clone(),
        organizational_unit_ids: configuration.organizational_unit_ids.clone(),
        excluded_account_ids: configuration.excluded_account_ids.clone().unwrap_or_default(),
        excluded_organizational_unit_ids: configuration
            .excluded_organizational_unit_ids
            .clone()
            .unwrap_or_default(),
    }
}

/// Does `action` satisfy every condition of `criteria`?
///
/// A criterion holds a LIST of conditions and `criteria` holds several
/// criterions: all of them must hold (AND), while the values inside one
/// condition are alternatives (OR). A criterion the caller left out constrains
/// nothing, so empty criteria match every action.
pub fn action_matches_criteria(action: &RecommendedAction, criteria: &Criteria) -> bool {
    let configuration = ebs_configuration(action.current_resource_details.as_ref());
    // `restartNeeded` is modelled as a STRING criterion over a boolean member,
    // so it is compared against the member's wire rendering.
    let restart_needed = action
        .restart_needed
        .map(|needed| if needed { "true" } else { "false" });

    criteria
        .region
        .iter()
        .flatten()
        .all(|condition| string_condition_holds(condition, action.region.as_deref()))
        && criteria
            .resource_arn
            .iter()
            .flatten()
            .all(|condition| string_condition_holds(condition, action.resource_arn.as_deref()))
        && criteria.ebs_volume_type.iter().flatten().all(|condition| {
            string_condition_holds(
                condition,
                configuration.and_then(|configuration| configuration.r#type.as_deref()),
            )
        })
        && criteria
            .restart_needed
            .iter()
            .flatten()
            .all(|condition| string_condition_holds(condition, restart_needed))
        && criteria
            .ebs_volume_size_in_gib
            .iter()
            .flatten()
            .all(|condition| {
                integer_condition_holds(
                    condition,
                    configuration
                        .and_then(|configuration| configuration.size_in_gib)
                        .map(f64::from),
                )
            })
        && criteria
            .look_back_period_in_days
            .iter()
            .flatten()
            .all(|condition| {
                integer_condition_holds(condition, action.look_back_period_in_days.map(f64::from))
            })
        && criteria
            .estimated_monthly_savings
            .iter()
            .flatten()
            .all(|condition| {
                double_condition_holds(
                    condition,
                    action
                        .estimated_monthly_savings
                        .as_ref()
                        .map(savings_for_criteria),
                )
            })
        && criteria
            .resource_tag
            .iter()
            .flatten()
            .all(|condition| tag_condition_holds(condition, action))
}

/// The `String*` comparison operators, i.e. the members of the model's
/// `ComparisonOperator` enum that compare strings, with the `IfExists` suffix
/// stripped (`smithy_query(query="shape",shape="ComparisonOperator")`).
const STRING_COMPARISONS: &[&str] = &[
    "StringEquals",
    "StringNotEquals",
    "StringEqualsIgnoreCase",
    "StringNotEqualsIgnoreCase",
    "StringLike",
    "StringNotLike",
];

/// The `Numeric*` comparison operators of the same enum, `IfExists` stripped.
const NUMERIC_COMPARISONS: &[&str] = &[
    "NumericEquals",
    "NumericNotEquals",
    "NumericLessThan",
    "NumericLessThanEquals",
    "NumericGreaterThan",
    "NumericGreaterThanEquals",
];

/// A comparison operator split into the base operator and whether it carried the
/// `IfExists` suffix. `IfExists` is what decides a condition over an attribute
/// the resource does not have: it holds, while every other operator does not.
fn split_comparison(comparison: &str) -> (&str, bool) {
    match comparison.strip_suffix("IfExists") {
        Some(base) => (base, true),
        None => (comparison, false),
    }
}

/// Does a string condition hold for `attribute`?
fn string_comparison_holds(comparison: &str, attribute: Option<&str>, values: &[String]) -> bool {
    let (base, if_exists) = split_comparison(comparison);
    let Some(attribute) = attribute else {
        return if_exists;
    };
    match base {
        "StringEquals" => values.iter().any(|value| value == attribute),
        "StringNotEquals" => !values.iter().any(|value| value == attribute),
        "StringEqualsIgnoreCase" => values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(attribute)),
        "StringNotEqualsIgnoreCase" => !values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(attribute)),
        "StringLike" => values
            .iter()
            .any(|value| wildcard_matches(value, attribute)),
        "StringNotLike" => !values
            .iter()
            .any(|value| wildcard_matches(value, attribute)),
        // Unreachable: a comparison outside the operator's family is refused
        // before any action is matched.
        _ => false,
    }
}

/// Does `candidate` match the `StringLike` wildcard `pattern`?
///
/// The wildcard language of a `String*Like` condition value is the two
/// metacharacters an AWS condition uses: `*` stands for any run of characters
/// (including none) and `?` for exactly one; every other character is literal,
/// compared by Unicode scalar. Two metacharacters are not a pattern language, so
/// this is the textbook greedy walk rather than a dependency: `star` remembers
/// the last `*` seen and `resume` the input position it matched from, so a later
/// mismatch backtracks by giving that `*` one more character. It visits each
/// input position at most once per `*`, so it always terminates.
fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let candidate: Vec<char> = candidate.chars().collect();
    let mut pattern_at = 0usize;
    let mut candidate_at = 0usize;
    let mut star: Option<usize> = None;
    let mut resume = 0usize;

    while candidate_at < candidate.len() {
        match pattern.get(pattern_at) {
            Some('*') => {
                star = Some(pattern_at);
                resume = candidate_at;
                pattern_at += 1;
            }
            Some('?') => {
                pattern_at += 1;
                candidate_at += 1;
            }
            Some(literal) if *literal == candidate[candidate_at] => {
                pattern_at += 1;
                candidate_at += 1;
            }
            // No literal match here: hand one more character to the last `*`, or
            // report the mismatch when there was none.
            _ => {
                let Some(star_at) = star else {
                    return false;
                };
                pattern_at = star_at + 1;
                resume += 1;
                candidate_at = resume;
            }
        }
    }

    // The input is consumed: what is left of the pattern may only be `*`s, each
    // of which can match the empty rest.
    pattern[pattern_at..].iter().all(|left| *left == '*')
}

/// Does a numeric condition hold for `attribute`? Several values are
/// alternatives, as they are for a string condition.
fn numeric_comparison_holds(comparison: &str, attribute: Option<f64>, values: &[f64]) -> bool {
    let (base, if_exists) = split_comparison(comparison);
    let Some(attribute) = attribute else {
        return if_exists;
    };
    match base {
        "NumericEquals" => values.contains(&attribute),
        "NumericNotEquals" => !values.contains(&attribute),
        "NumericLessThan" => values.iter().any(|value| attribute < *value),
        "NumericLessThanEquals" => values.iter().any(|value| attribute <= *value),
        "NumericGreaterThan" => values.iter().any(|value| attribute > *value),
        "NumericGreaterThanEquals" => values.iter().any(|value| attribute >= *value),
        _ => false,
    }
}

fn string_condition_holds(condition: &StringCriteriaCondition, attribute: Option<&str>) -> bool {
    string_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        condition.values.as_deref().unwrap_or_default(),
    )
}

fn integer_condition_holds(condition: &IntegerCriteriaCondition, attribute: Option<f64>) -> bool {
    let values: Vec<f64> = condition
        .values
        .iter()
        .flatten()
        .copied()
        .map(f64::from)
        .collect();
    numeric_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        &values,
    )
}

fn double_condition_holds(condition: &DoubleCriteriaCondition, attribute: Option<f64>) -> bool {
    numeric_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        condition.values.as_deref().unwrap_or_default(),
    )
}

/// Does a `resourceTag` condition hold? It names the tag KEY it is about, and
/// compares its values against the value of that one tag; a resource without the
/// tag has no attribute, so only an `IfExists` comparison holds for it.
fn tag_condition_holds(
    condition: &ResourceTagsCriteriaCondition,
    action: &RecommendedAction,
) -> bool {
    let key = condition.key.as_deref().unwrap_or_default();
    let attribute = action
        .resource_tags
        .iter()
        .flatten()
        .find(|tag| tag.key == key)
        .map(|tag| tag.value.as_str());
    string_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        condition.values.as_deref().unwrap_or_default(),
    )
}

/// Whether a criterion compares strings or numbers, which is what decides the
/// family of comparison operators it accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CriterionKind {
    Text,
    Numeric,
}

/// Why this `criteria` cannot be evaluated, if it cannot, as the service's own
/// validation report ([`constraint_report`], which echoes no request value).
///
/// The model types `comparison` and `values` as OPTIONAL members of every
/// condition and types `comparison` as the whole `ComparisonOperator` enum, so
/// the protocol boundary admits a condition with no operator, with no values, or
/// with an operator from the wrong family (a `Numeric*` operator on a string
/// criterion). None of those can select resources, so each is refused rather
/// than quietly matching nothing. The documented size caps
/// ([`MAX_CONDITIONS_PER_CRITERION`], [`MAX_VALUES_PER_CONDITION`]) are checked
/// here too, for the same reason: the model carries no `@length` for either.
pub fn criteria_violation(criteria: &Criteria) -> Option<String> {
    let mut conditions: Vec<(&str, CriterionKind, Option<&str>, usize)> = Vec::new();
    // (path, how many conditions that filter criteria carries)
    let mut criterion_sizes: Vec<(&str, usize)> = Vec::new();
    let text_criteria: [(&str, &Option<Vec<StringCriteriaCondition>>); 4] = [
        ("criteria.region", &criteria.region),
        ("criteria.resourceArn", &criteria.resource_arn),
        ("criteria.ebsVolumeType", &criteria.ebs_volume_type),
        ("criteria.restartNeeded", &criteria.restart_needed),
    ];
    for (path, criterion) in text_criteria {
        criterion_sizes.push((path, criterion.as_ref().map_or(0, Vec::len)));
        for condition in criterion.iter().flatten() {
            conditions.push((
                path,
                CriterionKind::Text,
                condition.comparison.as_deref(),
                condition.values.as_ref().map_or(0, Vec::len),
            ));
        }
    }
    criterion_sizes.push((
        "criteria.resourceTag",
        criteria.resource_tag.as_ref().map_or(0, Vec::len),
    ));
    for condition in criteria.resource_tag.iter().flatten() {
        conditions.push((
            "criteria.resourceTag",
            CriterionKind::Text,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    criterion_sizes.push((
        "criteria.ebsVolumeSizeInGib",
        criteria.ebs_volume_size_in_gib.as_ref().map_or(0, Vec::len),
    ));
    for condition in criteria.ebs_volume_size_in_gib.iter().flatten() {
        conditions.push((
            "criteria.ebsVolumeSizeInGib",
            CriterionKind::Numeric,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    criterion_sizes.push((
        "criteria.lookBackPeriodInDays",
        criteria
            .look_back_period_in_days
            .as_ref()
            .map_or(0, Vec::len),
    ));
    for condition in criteria.look_back_period_in_days.iter().flatten() {
        conditions.push((
            "criteria.lookBackPeriodInDays",
            CriterionKind::Numeric,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    criterion_sizes.push((
        "criteria.estimatedMonthlySavings",
        criteria
            .estimated_monthly_savings
            .as_ref()
            .map_or(0, Vec::len),
    ));
    for condition in criteria.estimated_monthly_savings.iter().flatten() {
        conditions.push((
            "criteria.estimatedMonthlySavings",
            CriterionKind::Numeric,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    if let Some((path, _)) = criterion_sizes
        .into_iter()
        .find(|(_, size)| *size > MAX_CONDITIONS_PER_CRITERION)
    {
        return Some(constraint_report(
            path,
            &format!(
                "Member must have length less than or equal to {MAX_CONDITIONS_PER_CRITERION}"
            ),
        ));
    }
    // A `resourceTag` condition is also about one tag KEY, so a condition
    // without one names no attribute to compare.
    let missing_tag_key = criteria
        .resource_tag
        .iter()
        .flatten()
        .any(|condition| condition.key.as_deref().unwrap_or_default().is_empty());
    if missing_tag_key {
        return Some(constraint_report(
            "criteria.resourceTag.member.key",
            "Member is required",
        ));
    }

    conditions
        .into_iter()
        .find_map(|(path, kind, comparison, value_count)| {
            condition_violation(path, kind, comparison, value_count)
        })
}

/// Why one criteria condition cannot be evaluated, if it cannot.
fn condition_violation(
    path: &str,
    kind: CriterionKind,
    comparison: Option<&str>,
    value_count: usize,
) -> Option<String> {
    let Some(comparison) = comparison else {
        return Some(constraint_report(
            &format!("{path}.member.comparison"),
            "Member is required",
        ));
    };
    let (base, _) = split_comparison(comparison);
    let (accepted, family) = match kind {
        CriterionKind::Text => (STRING_COMPARISONS, "String"),
        CriterionKind::Numeric => (NUMERIC_COMPARISONS, "Numeric"),
    };
    if !accepted.contains(&base) {
        return Some(constraint_report(
            &format!("{path}.member.comparison"),
            &format!("Member must be a {family} comparison operator for this criterion"),
        ));
    }
    if let Some(report) =
        empty_value_list_violation(&format!("{path}.member.values"), value_count)
    {
        return Some(report);
    }
    if value_count > MAX_VALUES_PER_CONDITION {
        return Some(constraint_report(
            &format!("{path}.member.values"),
            &format!("Member must have length less than or equal to {MAX_VALUES_PER_CONDITION}"),
        ));
    }
    None
}

/// Why this `tags` list cannot be applied as sent, if it cannot: one request may
/// not name the same tag key twice.
///
/// A tag list is a MAP of key to value — a resource holds at most one value per
/// key, which is what [`crate::ops::tag_resource`]'s merge and every reader of a
/// stored list assume — so a request naming one key twice states two values for
/// one tag and is refused rather than silently resolved to one of them. The
/// `TagList` shape is a list with a `@length` only, so the boundary admits the
/// repeat and the rule lives here. The report names no key: a validation report
/// of this service echoes no request value.
pub fn duplicate_tag_key_violation(tags: &[Tag]) -> Option<String> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(tags.len());
    for tag in tags {
        if !seen.insert(tag.key.as_str()) {
            return Some(constraint_report(
                "tags",
                "Member must not name the same tag key more than once",
            ));
        }
    }
    None
}

/// Why this caller-supplied `tags` list cannot be applied as sent, if it cannot
/// — the WHOLE judgement of a `tags` list, for every operation that takes one.
///
/// A list is judged by both rules this service owns over a `tags` member, in ONE
/// order: first the list as a whole ([`duplicate_tag_key_violation`] — a list
/// naming one key twice is not a tag map at all), then each key in the order the
/// request states them ([`tag_keys_violation`] — a key outside the modelled form,
/// or inside the provider's reserved namespace). Ordering the rules HERE instead
/// of in each handler is what makes a list that breaks more than one of them
/// refused with one and the same report whichever operation received it: two
/// handlers applying the same two rules in opposite orders word the same refusal
/// differently, which is exactly what single-homing the wording in this module is
/// for.
pub fn tag_list_violation(tags: &[Tag]) -> Option<String> {
    duplicate_tag_key_violation(tags)
        .or_else(|| tag_keys_violation(tags.iter().map(|tag| tag.key.as_str())))
}

/// Why this `recommendedActionTypes` list cannot drive a rule, if it cannot.
///
/// The list names the action types the rule may execute, so an empty one selects
/// no action at all and leaves a rule that automates nothing; the model carries
/// no `@length` on `RecommendedActionTypeList`, so the check lives here, as the
/// service-wide floor every caller-supplied value list is held to
/// ([`empty_value_list_violation`]). The preview operations refuse their own
/// (required) list the same way.
pub fn recommended_action_types_violation(action_types: &[String]) -> Option<String> {
    empty_value_list_violation("recommendedActionTypes", action_types.len())
}

/// The requirement a member carrying nothing fails — an empty value list, or a
/// name of no characters. Written once, so the floor is worded the same whichever
/// member of this service is judged against it.
const MIN_LENGTH_REQUIREMENT: &str = "Member must have length greater than or equal to 1";

/// Why a value list a request supplies to act THROUGH cannot be acted on, if it
/// cannot: it carries no value, so it selects nothing.
///
/// A list a caller sends to say WHAT a call applies to — the accounts an
/// association names, the action types a rule executes, the values a criteria
/// condition compares against, the values a listing's filter selects on, the
/// tags an addition carries, the keys a removal names — states the call's whole
/// subject, so one carrying nothing states a call with nothing to do; such a
/// request is refused rather than answered as though it had acted on something,
/// acted on a prefix of the list, or selected nothing at all. The ONE place this
/// service judges that floor, under the member's own name, so every such list is
/// refused at the same bound and with the same report. None of the list shapes
/// carries a `@length` minimum
/// (`smithy_query(query="shape",shape="AccountIdList,RecommendedActionTypeList,TagKeyList,TagList,StringList,FilterValues")`),
/// so the protocol boundary admits the empty list — `@required` is satisfied by
/// one — and the check lives here. A list member the model leaves OPTIONAL is
/// not judged here when it is absent: omitting it leaves the stored value
/// alone, and sending it empty is judged exactly as sending it is.
pub fn empty_value_list_violation(member: &str, count: usize) -> Option<String> {
    (count == 0).then(|| constraint_report(member, MIN_LENGTH_REQUIREMENT))
}

/// Why this `name` cannot name a rule, if it cannot, as the service's own
/// validation report ([`constraint_report`], which echoes no request value).
///
/// A rule's name "Must be 1-128 characters long"
/// (`smithy_query(query="doc",shape="UpdateAutomationRuleRequest",member="name")`),
/// of which only the ceiling reaches the protocol boundary: the `@length` on
/// `RuleName` has `min: 0` (`smithy_query(query="shape",shape="RuleName")`), so
/// the boundary admits the empty name and the floor is this service's own, worded
/// as every member of this service that carries nothing is refused with.
///
/// Both operations that write a rule's name — the create that requires it and
/// the update that REPLACES it — judge it here, so the two cannot drift apart
/// over which names they refuse or over how they word the refusal.
pub fn rule_name_violation(name: &str) -> Option<String> {
    name.is_empty()
        .then(|| constraint_report("name", MIN_LENGTH_REQUIREMENT))
}

/// The members of a rule's own form a write path supplies, as that request
/// carries them: `None` is a member the request does not write, which leaves the
/// stored (already judged) value alone and so is not judged.
///
/// A create fills in the members its input requires (`name`,
/// `recommendedActionTypes`, `schedule`) and leaves the rest as sent; an update
/// fills in only what it replaces.
#[derive(Debug, Default)]
pub struct RuleFormMembers<'a> {
    pub name: Option<&'a str>,
    pub priority: Option<&'a str>,
    pub recommended_action_types: Option<&'a [String]>,
    pub schedule: Option<&'a Schedule>,
    pub criteria: Option<&'a Criteria>,
}

/// Why the rule a request would write cannot stand as sent, if it cannot — the
/// WHOLE judgement of the members a rule's own form is made of, for every
/// operation that writes one.
///
/// The members are judged in ONE order, fixed HERE rather than in each handler:
/// the name ([`rule_name_violation`]), the precedence ([`priority_violation`]),
/// the action types ([`recommended_action_types_violation`]), the schedule
/// ([`schedule_violation`]) and the criteria ([`criteria_violation`]). A request
/// that breaks more than one of them is therefore refused with one and the same
/// report whichever operation received it, exactly as a `tags` list is
/// ([`tag_list_violation`]): two handlers applying the same rules in different
/// orders word the same refusal differently, which is what homing both the
/// wording and the order in this module prevents.
///
/// A rule's `ruleType` and its `organizationConfiguration` are judged separately
/// ([`organization_configuration_violation`]), because the pair an update must
/// judge is the MERGED one and so is not known until the stored rule has been
/// read; both write paths judge that pair AFTER these members, so the two agree
/// over it as well.
pub fn rule_form_violation(members: &RuleFormMembers<'_>) -> Option<String> {
    if let Some(report) = members.name.and_then(rule_name_violation) {
        return Some(report);
    }
    if let Some(report) = members.priority.and_then(priority_violation) {
        return Some(report);
    }
    if let Some(report) = members
        .recommended_action_types
        .and_then(recommended_action_types_violation)
    {
        return Some(report);
    }
    if let Some(report) = members.schedule.and_then(schedule_violation) {
        return Some(report);
    }
    members.criteria.and_then(criteria_violation)
}

/// Why this rule's `ruleType` and `organizationConfiguration` do not go
/// together, if they do not, as the service's own validation report
/// ([`constraint_report`], which echoes no request value).
///
/// The configuration is the organization-wide half of a rule — the order
/// organization rules apply in and the accounts they apply to — so the member
/// and the `OrganizationRule` rule type stand and fall together, in BOTH
/// directions: the type needs the configuration ("Required for OrganizationRule
/// type", `smithy_query(query="doc",shape="CreateAutomationRuleRequest",member="organizationConfiguration")`),
/// and the configuration names nothing for a rule type scoped to one account,
/// where silently storing and reporting back a member that puts nothing into
/// effect would be worse than reporting it — the same reading
/// [`preview_selection_violation`] refuses an `organizationScope` outside that
/// rule type by.
///
/// Both operations that can set either member (a create, and an update on the
/// rule it MERGES into) judge the pair here, so the two cannot drift apart over
/// which pairs they refuse or how they word the refusal.
pub fn organization_configuration_violation(
    rule_type: &str,
    organization_configuration: Option<&OrganizationConfiguration>,
) -> Option<String> {
    match (
        rule_type == "OrganizationRule",
        organization_configuration.is_some(),
    ) {
        (true, false) => Some(constraint_report(
            "organizationConfiguration",
            "Member is required for the OrganizationRule rule type",
        )),
        (false, true) => Some(constraint_report(
            "organizationConfiguration",
            "Member is valid only for the OrganizationRule rule type",
        )),
        _ => None,
    }
}

/// The cron expression a schedule expression carries: its text with the
/// optional `cron(...)` function wrapper removed and trimmed.
///
/// A schedule expression is written either bare (`30 12 * * ? *`) or wrapped in
/// the `cron(...)` form a schedule expression names its kind with
/// (`cron(30 12 * * ? *)`). The wrapper says which kind of expression follows
/// and is not one of the expression's whitespace-separated fields, so it is
/// stripped before the fields are read; the keyword is matched without regard
/// to case, and the parenthesis follows it directly, as in every expression the
/// form is written in. Text that is not so wrapped is its own cron expression.
pub fn cron_expression(expression: &str) -> &str {
    let trimmed = expression.trim();
    trimmed
        .strip_suffix(')')
        .and_then(|body| {
            let open = body.find('(')?;
            let (keyword, rest) = body.split_at(open);
            keyword
                .eq_ignore_ascii_case("cron")
                // `rest` starts at the `(` the keyword introduced, which is one
                // byte wide, so the remainder is the wrapped expression.
                .then_some(&rest[1..])
        })
        .unwrap_or(trimmed)
        .trim()
}

/// Why this `schedule` cannot say when a rule runs, if it cannot.
///
/// `scheduleExpression` is "The expression that defines when the schedule runs",
/// so a schedule without one never runs and is refused rather than stored. The
/// expression itself is a cron of [`CRON_FIELD_COUNTS`] fields
/// (`minutes hours day_of_month month day_of_week` and an optional `year`),
/// optionally in the `cron(...)` wrapper ([`cron_expression`]), of which "You
/// can schedule rules to run at most once per day. Your cron expression must
/// use specific values (not wildcards) for the minutes and hours fields"
/// (`smithy_query(query="doc",shape="Schedule",member="scheduleExpression")`),
/// so those two fields must each name ONE value in range: a wildcard, a list, a
/// range or a step in either would make the rule run more than once a day (or
/// not at a fixed time at all). The remaining fields are left to the
/// expression's own vocabulary, which the doc does not constrain.
///
/// `executionWindowInMinutes` carries `@range(60..1440)` in the model, so the
/// protocol boundary has already refused a window outside it.
pub fn schedule_violation(schedule: &Schedule) -> Option<String> {
    // The wrapper comes off FIRST, so an empty expression and an empty wrapper
    // report the same thing: neither says when the schedule runs.
    let expression = cron_expression(schedule.schedule_expression.as_deref().unwrap_or_default());
    if expression.is_empty() {
        return Some(constraint_report(
            "schedule.scheduleExpression",
            "Member is required",
        ));
    }
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if !CRON_FIELD_COUNTS.contains(&fields.len()) {
        return Some(constraint_report(
            "schedule.scheduleExpression",
            "Member must be a cron expression of the fields: minutes hours day_of_month month \
             day_of_week [year]",
        ));
    }
    // (field index, field name, the largest value that field admits)
    for (index, field, highest) in [(0usize, "minutes", 59u32), (1, "hours", 23)] {
        if !cron_field_is_one_value(fields[index], highest) {
            return Some(constraint_report(
                "schedule.scheduleExpression",
                &format!(
                    "Member must use a single specific value from 0 to {highest} for the {field} \
                     field, not a wildcard, list, range or step"
                ),
            ));
        }
    }
    None
}

/// Does a cron field name exactly ONE value in `0..=highest`?
///
/// Every way a cron field can stand for more than one value — the wildcards `*`
/// and `?`, a list (`0,30`), a range (`0-30`) and a step (`*/15`) — carries a
/// character that is not a digit, so a field of digits alone within range is
/// exactly the "specific value" the doc asks for.
fn cron_field_is_one_value(field: &str, highest: u32) -> bool {
    !field.is_empty()
        && field.chars().all(|character| character.is_ascii_digit())
        && field.parse::<u32>().is_ok_and(|value| value <= highest)
}

/// A total order key for a wire timestamp: `DateTime` carries seconds and
/// sub-second nanos, so comparing the pair orders two instants exactly.
pub fn instant_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// Is `at` inside the half-open window `[start, end)`? An absent bound leaves
/// that side unbounded, and the member names fix which side is which:
/// `startTimeInclusive` is included, `endTimeExclusive` is not.
pub fn within_window(
    at: ::aws_smithy_types::DateTime,
    start: Option<::aws_smithy_types::DateTime>,
    end: Option<::aws_smithy_types::DateTime>,
) -> bool {
    let at = instant_key(at);
    start.is_none_or(|start| at >= instant_key(start))
        && end.is_none_or(|end| at < instant_key(end))
}

/// Is the queried time range a window that can hold anything?
///
/// `endTimeExclusive` is exclusive, so a range whose end does not come after
/// its start describes an empty — or inverted — window and selects nothing; the
/// service refuses such a request instead of reporting an empty page. A range
/// with only one bound is always valid: the other side is unbounded.
pub fn time_range_is_valid(
    start: Option<::aws_smithy_types::DateTime>,
    end: Option<::aws_smithy_types::DateTime>,
) -> bool {
    match (start, end) {
        (Some(start), Some(end)) => instant_key(start) < instant_key(end),
        _ => true,
    }
}

/// What the service says when [`time_range_is_valid`] refuses a range: one
/// constant sentence, carrying neither bound's value, so every refused range
/// reports the same text. Recorded from the service itself.
pub const INVALID_TIME_RANGE_MESSAGE: &str = "Start date must be before end date";

/// The effective start of the window an event query reports over: the LATER of
/// the caller's `startTimeInclusive` and the oldest retrievable instant
/// ([`EVENT_RETRIEVAL_WINDOW`] before `now`), so an event created outside the
/// retrieval window is never reported however far back the caller asks.
pub fn effective_window_start(
    requested: Option<::aws_smithy_types::DateTime>,
    now: SystemTime,
) -> ::aws_smithy_types::DateTime {
    let floor = ::aws_smithy_types::DateTime::from(
        now.checked_sub(EVENT_RETRIEVAL_WINDOW).unwrap_or(UNIX_EPOCH),
    );
    match requested {
        Some(start) if instant_key(start) > instant_key(floor) => start,
        _ => floor,
    }
}

/// How many characters a calendar date with no time of day has
/// (`yyyy-MM-dd`).
const DATE_ONLY_LEN: usize = "yyyy-MM-dd".len();

/// The instant a STRING-typed date member of this service names, or `None` for
/// text that names none.
///
/// `ListAutomationEventSummaries` takes its window as `startDateInclusive` and
/// `endDateExclusive`, which the model targets at a plain `String` rather than
/// at `timestamp`
/// (`smithy_query(query="operation",operation="ListAutomationEventSummaries")`),
/// so the framework never reads them as instants and the text is read here.
///
/// The model documents no format for either member, so every form a caller can
/// name an instant in is read, and the READING is the library's in all of them:
///
/// * a calendar date, which names that day's FIRST instant, because a day is
///   included from its start;
/// * a date and time, with the UTC designator supplied when the text carries no
///   zone at all — since every other instant this service reads or reports is
///   UTC — and handed over exactly as written when it does carry one, so which
///   instants are readable is the library's `Format::DateTime` answer and never
///   a zone table of this service's own;
/// * the seconds since the epoch this protocol renders every MODELLED timestamp
///   of this service as, which is what `Format::EpochSeconds` reads.
///
/// The calendar conversion itself (month lengths, leap years, a fractional
/// second) is the library's, so no timestamp is parsed here.
///
/// TOTAL over arbitrary text: the value comes straight off the wire, so text no
/// form reads answers `None` for the caller to refuse as its modelled client
/// error, rather than being answered for a window it does not name.
pub fn parse_date_member(value: &str) -> Option<::aws_smithy_types::DateTime> {
    let date_time = |text: &str| {
        ::aws_smithy_types::DateTime::from_str(
            text,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .ok()
    };
    if value.len() == DATE_ONLY_LEN {
        if let Some(at) = date_time(&format!("{value}T00:00:00Z")) {
            return Some(at);
        }
    }
    let completed = if carries_zone(value) {
        value.to_string()
    } else {
        format!("{value}Z")
    };
    if let Some(at) = date_time(&completed) {
        return Some(at);
    }
    ::aws_smithy_types::DateTime::from_str(
        value,
        ::aws_smithy_types::date_time::Format::EpochSeconds,
    )
    .ok()
}

/// Does `value` already say which zone its time of day is in?
///
/// A UTC designator, or a sign inside the TIME part — the date's own separators
/// are hyphens, so a `-` before the designator belongs to the date. Which
/// instant the zone it names makes is the library's reading, not this
/// function's: all this decides is whether the text needs the UTC designator
/// supplied before it is read.
fn carries_zone(value: &str) -> bool {
    value.ends_with(['Z', 'z'])
        || value
            .rsplit_once(['T', 't'])
            .is_some_and(|(_, time)| time.contains('+') || time.contains('-'))
}

/// What the service says when a string-typed date member names no instant: the
/// requirement [`parse_date_member`] reads that member against, worded for
/// [`constraint_report`] and echoing no value.
pub const DATE_MEMBER_REQUIREMENT: &str =
    "Member must be a date, a date and time, or seconds since the epoch";

/// Does `event` satisfy every `ListAutomationEvents` filter?
///
/// Same rule as [`rule_matches_filters`]: a filter matches when the event's
/// corresponding field equals any of that filter's values, and several filters
/// are ANDed. The names are the model's `AutomationEventFilterName` value set,
/// and a name outside it never gets this far: the member is typed on that set,
/// so the generated request walk refuses the request before the handler runs.
/// Neither does a filter carrying no value, which selects nothing and is refused
/// by [`filters_violation`] instead.
pub fn event_matches_filters(event: &StoredEvent, filters: &[AutomationEventFilter]) -> bool {
    filters.iter().all(|filter| {
        let field: Option<&str> = match filter.name.as_str() {
            "AccountId" => Some(event.account_id.as_str()),
            "ResourceType" => Some(event.resource_type.as_str()),
            "EventType" => Some(event.event_type.as_str()),
            "EventStatus" => Some(event.event_status.as_str()),
            // An unknown name never reaches here: the request walk refuses it.
            _ => None,
        };
        field.is_some_and(|value| filter.values.iter().any(|candidate| candidate == value))
    })
}

/// What a client token already in the ledger says about the request carrying it.
#[derive(Debug)]
pub enum TokenOutcome {
    /// The token has not been seen: do the work.
    Fresh,
    /// The same token with the same parameters for the same operation: replay
    /// the earlier result, recomputed from the store.
    Replay(IdempotencyRecord),
    /// The same token and operation with DIFFERENT parameters.
    ParameterMismatch,
    /// The same token already used by a different operation.
    TokenInUse,
}

/// Which resource a client token's first use acted on.
///
/// A replay is recomputed from the store through this, so it can never report a
/// resource that is no longer there — and the two kinds this service mints are
/// named apart, because an `eventId` is not a rule ARN.
#[derive(Debug, Clone)]
pub enum TokenSubject {
    /// The `ruleArn` of the automation rule the call acted on.
    Rule(String),
    /// The `eventId` of the automation event the call started.
    Event(String),
}

impl TokenSubject {
    /// The rule ARN this subject names, or `None` when it names an event.
    pub fn rule_arn(&self) -> Option<&str> {
        match self {
            Self::Rule(rule_arn) => Some(rule_arn),
            Self::Event(_) => None,
        }
    }

    /// The event id this subject names, or `None` when it names a rule.
    pub fn event_id(&self) -> Option<&str> {
        match self {
            Self::Event(event_id) => Some(event_id),
            Self::Rule(_) => None,
        }
    }
}

/// One entry of the client-token ledger. It records WHICH resource the call
/// acted on rather than a serialized body, so a replay is recomputed from the
/// store and can never report a resource that is no longer there.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub operation: &'static str,
    pub request_fingerprint: String,
    pub subject: Option<TokenSubject>,
    /// When the token was first used, for the [`IDEMPOTENCY_TTL`] window.
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState {
    /// Enrollment configuration per account id.
    pub enrollments: HashMap<String, EnrollmentConfig>,
    /// The ORGANIZATION enrollment configurations, keyed by the target id they
    /// are stored on — an account id, an OU id or the organization root id.
    ///
    /// Written by `PutOrganizationEnrollmentConfiguration` and
    /// `DeleteOrganizationEnrollmentConfiguration`; read by every operation that
    /// reports an organization rule mode, through
    /// [`Self::effective_rule_mode_for_account`] and
    /// [`Self::effective_rule_mode_for_node`], so no two readers resolve two
    /// copies of one configuration. A `BTreeMap`, because the listing over it
    /// pages one stable sequence. Bounded by
    /// [`MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS`].
    pub organization_enrollment_configurations:
        BTreeMap<String, StoredOrganizationEnrollmentConfiguration>,
    /// Every automation rule, in creation order.
    pub rules: RuleStore,
    /// The recommended actions this service can automate, by
    /// `recommendedActionId` and in record order.
    ///
    /// A recommended action is produced by the analysis of an account's
    /// resources that follows the account's enrollment, so the operations that
    /// enroll an account record that account's inventory here through
    /// [`Self::ensure_recommended_action_inventory`]. No request carries an
    /// action: `StartAutomationEvent` resolves every id against this ONE store,
    /// `ListRecommendedActions` pages over it and `ListAutomationRulePreview`
    /// previews it, so no two of them read two copies of one resource. The
    /// selections read it through [`Self::retrievable_actions`], which is what
    /// scopes them to the accounts the caller currently reaches.
    pub recommended_actions: RecommendedActionStore,
    /// Every automation event, in creation order. Written by the operations
    /// that start and roll back an event; read by the event readers.
    pub events: EventStore,
    /// `clientToken` -> what the call that first used it did. Bounded by
    /// [`MAX_IDEMPOTENCY_ENTRIES`] and [`IDEMPOTENCY_TTL`].
    pub idempotency: HashMap<String, IdempotencyRecord>,
    /// The tokens in `idempotency`, oldest first, so the ledger can evict.
    idempotency_order: VecDeque<String>,
    /// Monotonic counter mixed into a minted id, so two resources created in
    /// the same nanosecond still get different ids.
    id_sequence: u64,
}

impl ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState {
    /// The enrollment configuration of `account_id`, or the unenrolled default
    /// when that account has never configured one. A READ: it stores nothing.
    pub fn enrollment(&self, account_id: &str) -> EnrollmentConfig {
        self.enrollments.get(account_id).cloned().unwrap_or_default()
    }

    /// Every account the enrollment store holds a configuration for, projected
    /// through [`EnrollmentConfig::to_account_info`], ordered by account id.
    ///
    /// These are the accounts this service KNOWS: one entry is written when an
    /// account configures its own enrollment and one when it is associated with
    /// the organization's management account, and each entry's `status` is what
    /// says whether that account has the Automation feature enabled. An account
    /// the store has no entry for has never been seen and so is reported by
    /// nothing.
    ///
    /// The store is a map, whose iteration order is its own business, so the
    /// ORDER is imposed here: ascending account id is the same sequence on every
    /// call, which is what paging over it requires — the pages of one walk must
    /// be windows onto ONE sequence rather than samples of a shifting one.
    ///
    /// A READ: it stores nothing. `at` is only what
    /// [`EnrollmentConfig::to_account_info`] falls back to for an entry that
    /// carries no update instant, and `hierarchy` is the organization the
    /// effective rule mode of each account is resolved through
    /// ([`Self::effective_rule_mode_for_account`]).
    pub fn accounts_in_order(
        &self,
        at: ::aws_smithy_types::DateTime,
        hierarchy: &OrganizationHierarchy,
    ) -> Vec<AccountInfo> {
        let mut accounts: Vec<AccountInfo> = self
            .enrollments
            .iter()
            .map(|(account_id, enrollment)| {
                enrollment.to_account_info(
                    account_id,
                    at,
                    self.effective_rule_mode_for_account(hierarchy, account_id),
                )
            })
            .collect();
        accounts.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        accounts
    }

    /// Is `account_id` opted in to Compute Optimizer Automation, i.e. does its
    /// stored enrollment report [`ENROLLED_STATUS`]? An account that never
    /// enrolled is not: [`EnrollmentConfig::default`] is the unenrolled one.
    pub fn is_enrolled(&self, account_id: &str) -> bool {
        self.enrollment(account_id).status == ENROLLED_STATUS
    }

    /// The service-wide enrollment pre-check, as the refusal it produces.
    ///
    /// `OptInRequiredException` is one of this service's COMMON errors
    /// (`smithy_query(query="service_info")`), so it is bound to every operation,
    /// and its shape documents "The account must be opted in to Compute Optimizer
    /// Automation before performing this action" with `httpError` 403. Every
    /// operation that performs one of this service's ACTIONS therefore refuses a
    /// caller whose account is not opted in, BEFORE it resolves a resource or
    /// evaluates a filter — so an unenrolled caller is never told "no such
    /// resource" or handed an empty listing.
    ///
    /// The two enrollment-configuration operations are the exception, and are the
    /// only ones that do not call this: `UpdateEnrollmentConfiguration` is the
    /// documented remedy for this very error (gating it would make the refusal
    /// unrecoverable) and `GetEnrollmentConfiguration` is how a caller reads the
    /// status it has to change, including [`DEFAULT_ENROLLMENT_STATUS`].
    ///
    /// `Some(..)` is the refusal to return; `None` means the gate is open. It
    /// reads the CALLER's own account, which is what the gate is about; an
    /// operation that has to judge some other, NAMED account (one listed in an
    /// organization request, say) asks [`Self::is_enrolled`] about that account.
    pub fn opt_in_required(&self) -> Option<OptInRequiredException> {
        (!self.is_enrolled(&caller_account_id())).then(|| OptInRequiredException {
            message: Some(OPT_IN_REQUIRED_MESSAGE.to_string()),
        })
    }

    /// Is `account_id` DIRECTLY associated with the organization's management
    /// account, i.e. does its own stored enrollment report
    /// [`ORGANIZATION_RULE_MODE_ANY_ALLOWED`]?
    ///
    /// Association is what that rule mode records: `AssociateAccounts` sets it
    /// to "Any allowed", which is what "permits the management account to create
    /// Automation rules that automatically apply actions to that account". An
    /// account that has never been associated reports the other mode
    /// ([`EnrollmentConfig::reported_organization_rule_mode`]), so it is not
    /// associated.
    ///
    /// This is the DIRECT setting, which is what the two association operations
    /// report back as the accounts they associated or disassociated: the call
    /// either wrote that setting or it did not. Whether the management account
    /// may actually reach into the account is the EFFECTIVE mode
    /// ([`Self::may_initiate_in`]), which an organization enrollment
    /// configuration takes precedence in.
    pub fn is_associated(&self, account_id: &str) -> bool {
        self.enrollment(account_id).reported_organization_rule_mode()
            == ORGANIZATION_RULE_MODE_ANY_ALLOWED
    }

    /// May the caller initiate automation work in `account_id`?
    ///
    /// The operations that INITIATE work in an account — the on-demand
    /// automation of a recommended action, and the rollback of an automation
    /// event — each document that a management account or a delegated
    /// administrator "can only initiate" that work "for associated member
    /// accounts", and point at `AssociateAccounts` as the remedy. So work in
    /// ANOTHER account is initiated only while that account's EFFECTIVE
    /// organization rule mode permits organization rules to apply to it
    /// ([`Self::effective_rule_mode_for_account`]) — the mode an organization
    /// enrollment configuration on the account, or inherited from its OU or the
    /// organization root, supplies, and otherwise the mode the association
    /// itself set, since such a configuration "take[s] precedence over settings
    /// made by `AssociateAccounts` and `DisassociateAccounts`".
    ///
    /// Judged per call, never taken from the stored resource: a disassociation
    /// removes the association and leaves the member's events and analysed
    /// actions exactly where they were ([`Self::disassociate_account`]), so a
    /// resource of that account stays reachable by id after the organization
    /// has stopped reaching into it.
    ///
    /// The caller's own account is nobody's associated MEMBER account, and what
    /// the restriction is about is reaching into another account, so an
    /// automation of the caller's own resources needs no association. The
    /// caller's enrollment is a separate gate ([`Self::opt_in_required`]).
    pub fn may_initiate_in(&self, hierarchy: &OrganizationHierarchy, account_id: &str) -> bool {
        account_id == caller_account_id()
            || self
                .effective_rule_mode_for_account(hierarchy, account_id)
                .mode
                == ORGANIZATION_RULE_MODE_ANY_ALLOWED
    }

    /// May a rule of `rule_type` be scoped to every account `configuration`
    /// names?
    ///
    /// An organization configuration's `accountIds` are "specific Amazon Web
    /// Services account IDs where the organization rule should be applied"
    /// (`smithy_query(query="shape",shape="OrganizationConfiguration")`), so a
    /// rule stored with that configuration automatically applies its recommended
    /// actions in each of them. Applying actions in another account is exactly
    /// what an association permits — `AssociateAccounts` "permits the management
    /// account to create Automation rules that automatically apply actions to
    /// that account", and after `DisassociateAccounts` the management account
    /// (or delegated administrator) "cannot create Automation rules for that
    /// account" — so the accounts a rule may be scoped to are the ones the
    /// caller may automate in right now ([`Self::may_initiate_in`], the same
    /// standing the operations that initiate work in an account are judged by).
    ///
    /// Judged over the account ids of the configuration the RESULTING rule would
    /// carry — the one a create supplies, or the one an update merges over the
    /// stored rule — which are the accounts that call makes the rule apply its
    /// actions in; a configuration naming no `accountIds` names no SPECIFIC
    /// account, so such a rule spans the organization itself
    /// (which is how a preview without an `organizationScope` is scoped too —
    /// [`preview_account_scope`]) and reaches a member account only for as long
    /// as that account's association lasts. An absent configuration is only
    /// valid for a rule that is not an organization rule at all.
    ///
    /// An INCLUDED OU or organization root carries its own documented
    /// requirement, which the same standing cannot express because an OU has no
    /// association: "Each included OU or organization root must have an
    /// effective organization rule mode of `AnyAllowed`"
    /// (`smithy_query(query="shape",shape="OrganizationConfiguration")`), so
    /// every id the configuration includes is judged by the mode that OU or root
    /// is in right now ([`Self::effective_rule_mode_for_node`]). An EXCLUDED id
    /// is judged by nothing: the rule does not apply there.
    ///
    /// Judged for the RESULTING rule type too, because the configuration is the
    /// organization-wide half of a rule and the `OrganizationRule` type is what
    /// puts it into effect: a rule of any other type applies its actions in the
    /// account it lives in, so the accounts such a configuration names scope
    /// nothing and no association can be required for them. Such a pairing is
    /// not stored either — it is refused as an inapplicable member
    /// ([`organization_configuration_violation`]), the same answer a preview
    /// gives an `organizationScope` outside that rule type
    /// ([`preview_selection_violation`]) — so this standing check never decides
    /// what happens to it.
    pub fn may_scope_rule_to(
        &self,
        hierarchy: &OrganizationHierarchy,
        rule_type: &str,
        configuration: Option<&OrganizationConfiguration>,
    ) -> bool {
        if rule_type != "OrganizationRule" {
            return true;
        }
        let scope = rule_account_scope(configuration);
        scope
            .account_ids
            .iter()
            .flatten()
            .all(|account_id| self.may_initiate_in(hierarchy, account_id))
            && scope
                .organizational_unit_ids
                .iter()
                .flatten()
                .all(|node_id| {
                    self.effective_rule_mode_for_node(hierarchy, node_id).mode
                        == ORGANIZATION_RULE_MODE_ANY_ALLOWED
                })
    }

    /// The stored recommended actions the caller may RETRIEVE right now, in the
    /// order the store holds them.
    ///
    /// `ListRecommendedActions` and `ListRecommendedActionSummaries` each state
    /// the retrieval scope: a management account or a delegated administrator
    /// "can retrieve recommended actions that include associated member
    /// accounts", and point at `AssociateAccounts` as the way to bring a member
    /// account into it. So ANOTHER account's analysis output is retrieved only
    /// while that account's association is in place — the same standing the
    /// operations that reach into an account are judged by
    /// ([`Self::may_initiate_in`]), read PER CALL and never inferred from the
    /// action being stored: a disassociation leaves the member's analysed
    /// actions exactly where they were ([`Self::disassociate_account`]), so it
    /// is the association that decides what a caller retrieves, not the
    /// presence of the action.
    ///
    /// Every SELECTION over the stored actions starts here — the two retrieval
    /// listings and the preview selection ([`previewed_actions`]), which is what
    /// a rule would act on — so no read path reports an action of an account the
    /// caller no longer reaches, and none of them can drift apart from the
    /// operations that automate those actions. Resolving ONE action by its id
    /// still goes to the whole store ([`RecommendedActionStore::get`]), which is
    /// what lets `StartAutomationEvent` refuse with the caller's standing
    /// instead of claiming the resource has gone away.
    ///
    /// An action that records no `accountId` names no other account, so no
    /// association gates it: the restriction is about reaching INTO another
    /// account, and such an action is in nobody else's. It is still outside
    /// every preview, because a preview selects by account scope and no scope
    /// covers an account-less action ([`previewed_actions`]).
    pub fn retrievable_actions<'store>(
        &'store self,
        hierarchy: &'store OrganizationHierarchy,
    ) -> impl Iterator<Item = &'store RecommendedAction> + 'store {
        self.recommended_actions.iter_in_order().filter(|action| {
            action
                .account_id
                .as_deref()
                .is_none_or(|account_id| self.may_initiate_in(hierarchy, account_id))
        })
    }

    /// How many of `account_ids` the enrollment store does not hold an entry for
    /// yet — i.e. how much an association of exactly those accounts would GROW
    /// the store. An id already stored costs nothing, whatever its status.
    pub fn accounts_not_stored(&self, account_ids: &[String]) -> usize {
        account_ids
            .iter()
            .filter(|account_id| !self.enrollments.contains_key(*account_id))
            .count()
    }

    /// Would storing an entry for each of `account_ids` leave the enrollment
    /// store holding more than [`MAX_ENROLLED_ACCOUNTS`] accounts? The store is
    /// in memory and `AssociateAccounts` keys it by request-supplied ids, so a
    /// request past the cap is refused instead of applied.
    pub fn would_exceed_account_limit(&self, account_ids: &[String]) -> bool {
        self.enrollments
            .len()
            .saturating_add(self.accounts_not_stored(account_ids))
            > MAX_ENROLLED_ACCOUNTS
    }

    /// Associate `account_id` with the organization's management account, as
    /// `AssociateAccounts` documents the association: the account's organization
    /// rule mode becomes [`ORGANIZATION_RULE_MODE_ANY_ALLOWED`], and an account
    /// that "has not previously enabled the Automation feature" has it enabled —
    /// which is [`ENROLLED_STATUS`] — by the association itself.
    ///
    /// Answers whether the stored configuration CHANGED. `at` is stamped as the
    /// account's `lastUpdatedTimestamp` ("The timestamp when the account's
    /// Automation enrollment status was last updated") only when it did, so
    /// re-associating an account that is already associated and enrolled leaves
    /// its stored configuration, timestamp included, exactly where it was.
    pub fn associate_account(&mut self, account_id: &str, at: ::aws_smithy_types::DateTime) -> bool {
        let enrollment = self.enrollments.entry(account_id.to_string()).or_default();
        let mut changed = false;
        if enrollment.status != ENROLLED_STATUS {
            enrollment.status = ENROLLED_STATUS.to_string();
            // The reason belongs to the status it explains, and the status this
            // account now reports was not reached by a failure.
            enrollment.status_reason = None;
            changed = true;
        }
        if enrollment.organization_rule_mode.as_deref() != Some(ORGANIZATION_RULE_MODE_ANY_ALLOWED) {
            enrollment.organization_rule_mode =
                Some(ORGANIZATION_RULE_MODE_ANY_ALLOWED.to_string());
            changed = true;
        }
        if changed {
            enrollment.last_updated_timestamp = Some(at);
        }
        changed
    }

    /// Set `account_id`'s OWN enrollment status to `status`, as
    /// `UpdateEnrollmentConfiguration` documents: the status it stores is "The
    /// desired enrollment status", and the reason belongs to the status it
    /// explains, so a status a caller set successfully carries none.
    ///
    /// Answers whether the stored configuration CHANGED. `at` is stamped as the
    /// account's `lastUpdatedTimestamp` ("The timestamp when the enrollment
    /// configuration was last updated") only when it did — the same rule the
    /// other writers of this field follow ([`Self::associate_account`],
    /// [`Self::disassociate_account`], [`StoredRule::set_tags`]) — so a repeat of
    /// the status the account already reports leaves its stored configuration,
    /// timestamp included, exactly where it was, and no reader claims an instant
    /// at which nothing was updated.
    ///
    /// Recording a configuration for an account the store holds none for IS a
    /// change, whatever status that first call asks for: the entry it creates is
    /// what makes the account one this service knows and reports
    /// ([`Self::accounts_in_order`]), so that call stamps the instant it stored
    /// it.
    pub fn set_enrollment_status(
        &mut self,
        account_id: &str,
        status: &str,
        at: ::aws_smithy_types::DateTime,
    ) -> bool {
        let mut changed = !self.enrollments.contains_key(account_id);
        let enrollment = self.enrollments.entry(account_id.to_string()).or_default();
        if enrollment.status != status {
            enrollment.status = status.to_string();
            changed = true;
        }
        if enrollment.status_reason.is_some() {
            enrollment.status_reason = None;
            changed = true;
        }
        if changed {
            enrollment.last_updated_timestamp = Some(at);
        }
        changed
    }

    /// Remove `account_id`'s association with the organization's management
    /// account, as `DisassociateAccounts` documents the removal: "Once
    /// disassociated, organization rules no longer apply to the member account,
    /// and the management account (or delegated administrator) cannot create
    /// Automation rules for that account" — which is exactly what the other
    /// `OrganizationRuleMode`, [`ORGANIZATION_RULE_MODE_NONE_ALLOWED`], records
    /// ([`EnrollmentConfig::reported_organization_rule_mode`]).
    ///
    /// The account's OWN enrollment is left alone: an association and an
    /// enrollment are two different facts about an account, the status is "The
    /// enrollment status of the account" and only that account's
    /// `UpdateEnrollmentConfiguration` sets it, so a member account that enabled
    /// the Automation feature keeps it enabled (and keeps the inventory that
    /// enrollment produced) once the organization stops reaching it — the
    /// organization's own retrieval scope is what stops covering that inventory
    /// ([`Self::retrievable_actions`]).
    ///
    /// Only an account the enrollment store already holds an entry for is
    /// written: an account the store has never seen carries no association to
    /// remove, so a disassociation never GROWS the store and no capacity bound
    /// applies to it.
    ///
    /// Answers whether the stored configuration CHANGED. `at` is stamped as the
    /// account's `lastUpdatedTimestamp` only when it did, so disassociating an
    /// account that carries no association leaves its stored configuration,
    /// timestamp included, exactly where it was.
    pub fn disassociate_account(
        &mut self,
        account_id: &str,
        at: ::aws_smithy_types::DateTime,
    ) -> bool {
        let Some(enrollment) = self.enrollments.get_mut(account_id) else {
            return false;
        };
        if enrollment.reported_organization_rule_mode() == ORGANIZATION_RULE_MODE_NONE_ALLOWED {
            return false;
        }
        enrollment.organization_rule_mode = Some(ORGANIZATION_RULE_MODE_NONE_ALLOWED.to_string());
        enrollment.last_updated_timestamp = Some(at);
        true
    }

    /// Record the recommended actions the analysis of `account_id`'s resources
    /// produces, for an account this service now holds as enrolled.
    ///
    /// Compute Optimizer analyses an account's resources once it is opted in to
    /// Automation, and `ListRecommendedActions`, the two preview lists and
    /// `StartAutomationEvent` all read that output, so an enrolled account has
    /// an inventory to list, preview and automate without any call having
    /// supplied one. Called from the operations that ENROLL an account — never
    /// from a read path, so what a reader sees does not depend on who read
    /// before it.
    ///
    /// Idempotent: the inventory of an account is a pure function of its id
    /// ([`recommended_action_inventory`]), and an entry already recorded is left
    /// exactly as it is, so opting out and back in neither duplicates an action
    /// nor rewrites one. The store therefore holds at most
    /// [`ANALYSED_VOLUMES`]`.len()` actions per account, and the accounts
    /// themselves are capped by [`MAX_ENROLLED_ACCOUNTS`].
    pub fn ensure_recommended_action_inventory(&mut self, account_id: &str) {
        for (action_id, action) in recommended_action_inventory(account_id) {
            if !self.recommended_actions.contains(&action_id) {
                self.recommended_actions.insert(action_id, action);
            }
        }
    }

    /// A fresh `ruleId` matching the model's `^[0-9A-Za-z]{16}$`, retried in the
    /// (astronomically unlikely) event of a collision with a live rule.
    pub fn mint_rule_id(&mut self) -> String {
        loop {
            let candidate = self.next_id();
            if !self.rules.contains_rule_id(&candidate) {
                return candidate;
            }
        }
    }

    /// A fresh `eventId` matching the model's `^[0-9A-Za-z]{16}$`, retried in
    /// the (astronomically unlikely) event of a collision with a live event.
    pub fn mint_event_id(&mut self) -> String {
        loop {
            let candidate = self.next_id();
            if !self.events.contains(&candidate) {
                return candidate;
            }
        }
    }

    /// The next candidate id: the leading hex of a digest over this store's
    /// counter and the current clock. The ONE place an id's characters are
    /// chosen, so every minted id satisfies the same `^[0-9A-Za-z]{16}$`.
    fn next_id(&mut self) -> String {
        self.id_sequence += 1;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let digest = Sha256::digest(format!("{}:{nanos}", self.id_sequence).as_bytes());
        hex::encode(digest).chars().take(MINTED_ID_LEN).collect()
    }

    /// What the ledger says about `token` for `operation` with `request_fingerprint`.
    /// An absent token is always [`TokenOutcome::Fresh`]: a caller that sends no
    /// token asks for no replay.
    pub fn classify_token(
        &self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
    ) -> TokenOutcome {
        let Some(token) = token else {
            return TokenOutcome::Fresh;
        };
        match self.idempotency.get(token) {
            None => TokenOutcome::Fresh,
            // Past its replay window: the token is as good as unseen.
            Some(record)
                if record
                    .recorded_at
                    .elapsed()
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL) =>
            {
                TokenOutcome::Fresh
            }
            Some(record) if record.operation != operation => TokenOutcome::TokenInUse,
            Some(record) if record.request_fingerprint != request_fingerprint => {
                TokenOutcome::ParameterMismatch
            }
            Some(record) => TokenOutcome::Replay(record.clone()),
        }
    }

    /// Record that `token` was used by `operation` for `request_fingerprint`.
    /// A no-op when the caller sent no token.
    pub fn record_token(
        &mut self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
        subject: Option<TokenSubject>,
    ) {
        let Some(token) = token else { return };
        let now = SystemTime::now();
        // Drop what no retry can still ask about, then make room for this entry.
        self.evict_expired_tokens(now);
        if self
            .idempotency
            .insert(
                token.to_string(),
                IdempotencyRecord {
                    operation,
                    request_fingerprint: request_fingerprint.to_string(),
                    subject,
                    recorded_at: now,
                },
            )
            .is_none()
        {
            self.idempotency_order.push_back(token.to_string());
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Forget every token past its [`IDEMPOTENCY_TTL`] window.
    fn evict_expired_tokens(&mut self, now: SystemTime) {
        while let Some(oldest) = self.idempotency_order.front() {
            let expired = self.idempotency.get(oldest).is_none_or(|record| {
                now.duration_since(record.recorded_at)
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL)
            });
            if !expired {
                return;
            }
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }
}

/// Does `rule` satisfy every `ListAutomationRules` filter?
///
/// A filter matches when the rule's corresponding field equals any of the
/// filter's values; several filters are ANDed. The filter names are the model's
/// `AutomationRuleFilterName` value set, and a name outside it never gets this
/// far: the member is typed on that set, so the generated request walk refuses
/// the request before the handler runs. Neither does a filter carrying no value,
/// which selects nothing and is refused by [`filters_violation`] instead.
pub fn rule_matches_filters(rule: &StoredRule, filters: &[Filter]) -> bool {
    filters.iter().all(|filter| {
        let field: Option<String> = match filter.name.as_str() {
            "Name" => Some(rule.name.clone()),
            "RecommendedActionType" => {
                // The rule's action types are a LIST, so the filter matches when
                // any requested value is among them.
                return rule
                    .recommended_action_types
                    .iter()
                    .any(|t| filter.values.iter().any(|v| v == t));
            }
            "Status" => Some(rule.status.clone()),
            "RuleType" => Some(rule.rule_type.clone()),
            "OrganizationConfigurationRuleApplyOrder" => rule
                .organization_configuration
                .as_ref()
                .and_then(|c| c.rule_apply_order.clone()),
            "AccountId" => Some(rule.account_id.clone()),
            // An unknown name never reaches here: the request walk refuses it.
            _ => None,
        };
        field.is_some_and(|value| filter.values.contains(&value))
    })
}

/// Does `action` satisfy every `ListRecommendedActions` filter?
///
/// A filter matches when the action's corresponding member equals any of the
/// filter's values; several filters are ANDed, exactly as for
/// [`rule_matches_filters`]. A member the stored action does not carry matches
/// nothing. The names are the model's `RecommendedActionFilterName` value set,
/// and a name outside it never gets this far: the member is typed on that set,
/// so the generated request walk refuses the request before the handler runs.
/// Neither does a filter carrying no value, which selects nothing and is refused
/// by [`filters_violation`] instead.
pub fn recommended_action_matches_filters(
    action: &RecommendedAction,
    filters: &[RecommendedActionFilter],
) -> bool {
    filters.iter().all(|filter| {
        let field: Option<String> = match filter.name.as_str() {
            "ResourceType" => action.resource_type.clone(),
            "RecommendedActionType" => action.recommended_action_type.clone(),
            "ResourceId" => action.resource_id.clone(),
            "AccountId" => action.account_id.clone(),
            "CurrentResourceDetailsEbsVolumeType" => {
                ebs_volume_type(action.current_resource_details.as_ref())
            }
            // An integer member: the filter carries its decimal rendering, so
            // the comparison is numeric and a value that is not a number
            // matches nothing rather than matching by spelling.
            "LookBackPeriodInDays" => {
                let Some(days) = action.look_back_period_in_days else {
                    return false;
                };
                return filter
                    .values
                    .iter()
                    .filter_map(|value| value.trim().parse::<i32>().ok())
                    .any(|wanted| wanted == days);
            }
            // A boolean member: `true`/`false`, matched without regard to case
            // so neither JSON's nor the console's spelling is rejected.
            "RestartNeeded" => {
                let Some(restart_needed) = action.restart_needed else {
                    return false;
                };
                return filter
                    .values
                    .iter()
                    .filter_map(|value| value.trim().to_ascii_lowercase().parse::<bool>().ok())
                    .any(|wanted| wanted == restart_needed);
            }
            // The resource's tags are a LIST, so the filter matches when any
            // requested key (or value) occurs among them.
            "ResourceTagsKey" => {
                return action.resource_tags.iter().flatten().any(|tag| {
                    filter.values.contains(&tag.key)
                });
            }
            "ResourceTagsValue" => {
                return action.resource_tags.iter().flatten().any(|tag| {
                    filter.values.contains(&tag.value)
                });
            }
            // An unknown name never reaches here: the request walk refuses it.
            _ => None,
        };
        field.is_some_and(|value| filter.values.contains(&value))
    })
}

/// Is `priority` the form the member documents?
///
/// "A string representation of a decimal number between 0 and 1 (having up to
/// 30 digits after the decimal point)"
/// (`smithy_query(query="doc",shape="CreateAutomationRuleRequest",member="priority")`).
/// The model types the member as a plain `String`, so this rule is the service's
/// own and is checked here rather than at the protocol boundary. The report for a
/// value outside the form is [`priority_violation`].
fn priority_is_valid(priority: &str) -> bool {
    let (integral, fractional) = match priority.split_once('.') {
        Some((integral, fractional)) => (integral, Some(fractional)),
        None => (priority, None),
    };
    if integral.is_empty() || !integral.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if let Some(fractional) = fractional {
        if fractional.is_empty()
            || fractional.len() > 30
            || !fractional.bytes().all(|b| b.is_ascii_digit())
        {
            return false;
        }
    }
    // Between 0 and 1: any `0.<digits>`, or 1 with no non-zero fraction.
    match integral.parse::<u64>() {
        Ok(0) => true,
        Ok(1) => fractional.is_none_or(|f| f.bytes().all(|b| b == b'0')),
        _ => false,
    }
}

/// Why this `priority` cannot order a rule, if it cannot, as the service's own
/// validation report ([`constraint_report`], which echoes no request value).
///
/// The form is the documented one ([`priority_is_valid`]). Both operations that
/// write a rule's precedence — the create and the update that replaces it —
/// judge it here, so the two cannot drift apart over which values they refuse,
/// over the bound and the fractional width they refuse them at, or over how they
/// word the refusal.
pub fn priority_violation(priority: &str) -> Option<String> {
    (!priority_is_valid(priority)).then(|| {
        constraint_report(
            "priority",
            "Member must be a decimal number between 0 and 1 with up to 30 digits after the \
             decimal point",
        )
    })
}

/// Where a page of a paginated list operation resumed from: how far into the
/// filtered sequence the previous page stopped, and a fingerprint of the filters
/// it was taken with, so resuming with different filters is refused instead of
/// returning a page of a different result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub offset: usize,
    pub filter_fingerprint: String,
}

/// Encode a cursor as the opaque `nextToken` every paginated operation of this
/// service describes (`^[A-Za-z0-9+/=]+$`). Hex of `<offset>:<fingerprint>` satisfies that
/// pattern and carries the cursor itself, so a page token needs no server-side
/// entry and a read never writes.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    hex::encode(format!("{}:{}", cursor.offset, cursor.filter_fingerprint))
}

/// Decode a `nextToken` produced by [`encode_page_token`]. `None` when the token
/// was not produced by this service.
///
/// A token this service produced is EXACTLY what the encoder writes for some
/// cursor, both halves of the spelling included: the lowercase hex `hex::encode`
/// emits, of `<offset>:<fingerprint>` whose offset is the single decimal
/// `usize`'s `Display` renders. The readers underneath accept more than that —
/// `hex::decode` reads the same bytes spelled with uppercase digits, and
/// `usize::from_str` reads a leading `+` and leading zeros — so each is pinned to
/// the spelling the encoder emits, else a cursor this service never minted would
/// decode and resume a walk on nothing but its fingerprint.
///
/// What a decoded cursor may resume is the caller's own check: it compares the
/// cursor's fingerprint against the one it would have issued for this request,
/// and serves the page through [`resume_page`], which refuses a cursor naming a
/// position its sequence does not reach.
pub fn decode_page_token(token: &str) -> Option<PageCursor> {
    let bytes = hex::decode(token).ok()?;
    // The hex half: a decoding is this service's token only when it re-encodes
    // to the very string presented.
    if hex::encode(&bytes) != token {
        return None;
    }
    let decoded = String::from_utf8(bytes).ok()?;
    let (offset, fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: canonical_offset(offset)?,
        filter_fingerprint: fingerprint.to_string(),
    })
}

/// The offset `spelling` names, when it is the ONE spelling
/// [`encode_page_token`] writes for that offset: the bare decimal `usize`'s
/// `Display` renders, which carries no sign and no leading zero beyond the single
/// digit of zero itself. `None` for every other spelling of the same number, so
/// it is accepted only when the value renders back to it.
fn canonical_offset(spelling: &str) -> Option<usize> {
    let offset: usize = spelling.parse().ok()?;
    (offset.to_string() == spelling).then_some(offset)
}

/// Why a `nextToken` a caller presented cannot enter a page walk: the two
/// things a paginated operation refuses it for, each of which it words itself.
///
/// [`page_entry_offset`] answers one of these instead of a message, because the
/// requirement a refusal reports names what the token had to have been taken for
/// — the operation, and the query members that operation pages under — which is
/// per-operation wording the handler owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageEntryRefusal {
    /// The token is not one this service minted at all: it decodes to no cursor
    /// ([`decode_page_token`]).
    Unissued,
    /// It decodes to a cursor, but one taken for a different walk than the call
    /// resuming with it — another query of this operation, or another operation
    /// of this service ([`page_fingerprint`]).
    OtherQuery,
}

/// Where a paginated listing resumes, from the caller's optional `nextToken`.
///
/// One rule for every paginated operation of this service, beside the rules
/// [`page_size`], [`decode_page_token`] and [`resume_page`] already state, so the
/// decision each one makes on the way INTO its walk is made once rather than
/// copied per operation: a request carrying no token starts at the beginning; a
/// token that does not decode is one this service never minted
/// ([`PageEntryRefusal::Unissued`]); a token that decodes but carries a
/// fingerprint other than `query_fingerprint` — the one this very call would
/// mint — was taken for another walk and cannot resume this one
/// ([`PageEntryRefusal::OtherQuery`]), because its offset indexes a different
/// sequence; otherwise the cursor's own offset is where the previous page
/// stopped.
///
/// The fingerprint is compared AFTER the token decodes, so a token that is no
/// cursor at all is reported as the token this service never issued rather than
/// as one taken for another query.
pub fn page_entry_offset(
    next_token: Option<&str>,
    query_fingerprint: &str,
) -> Result<usize, PageEntryRefusal> {
    let Some(token) = next_token else {
        return Ok(0);
    };
    let Some(cursor) = decode_page_token(token) else {
        return Err(PageEntryRefusal::Unissued);
    };
    if cursor.filter_fingerprint != query_fingerprint {
        return Err(PageEntryRefusal::OtherQuery);
    }
    Ok(cursor.offset)
}

/// How many items one page of a paginated listing serves, from the caller's
/// `maxResults`.
///
/// One rule for every paginated operation of this service: a caller that sends
/// no `maxResults` gets [`DEFAULT_MAX_RESULTS`], and the size is floored at one,
/// because a zero page would return nothing while still carrying a token whose
/// offset never advances, so a walk would never terminate. Nothing is bounded
/// above here: each operation's own `maxResults` `@range` is enforced by the
/// generated request walk BEFORE its handler runs, so only the values that range
/// admits ever reach this.
pub fn page_size(max_results: Option<i32>) -> usize {
    usize::try_from(max_results.unwrap_or(DEFAULT_MAX_RESULTS))
        .unwrap_or(1)
        .max(1)
}

/// Serve one page of an already-selected sequence, and mint the `nextToken` that
/// resumes it.
///
/// The walk itself, beside the rules [`page_size`], [`PageCursor`] and
/// [`encode_page_token`] already state, so every paginated operation of this
/// service performs ONE walk rather than a copy of it: resume at `offset` —
/// where the previous page stopped — take one item BEYOND the page, whose
/// presence is what says another page exists (so "is there more" never needs a
/// second pass over the sequence), cut the page back to `page_size`, and carry a
/// token only while that extra item was there, pointing at exactly the item this
/// page stopped before.
///
/// `selected` is the sequence the caller's filters already narrowed, in the order
/// the operation reports it; `filter_fingerprint` is the one that selection would
/// be issued under, so a token can only resume the query it was minted for.
///
/// A handler reaches this through [`resume_page`], which adds the one thing a
/// caller-supplied cursor has to satisfy and a first page does not.
pub fn serve_page<T, I>(
    selected: I,
    offset: usize,
    page_size: usize,
    filter_fingerprint: String,
) -> (Vec<T>, Option<String>)
where
    I: IntoIterator<Item = T>,
{
    let (page, next_token, _) = walk_page(selected, offset, page_size, filter_fingerprint);
    (page, next_token)
}

/// Serve the page a CURSOR asks for, or refuse a cursor the sequence cannot be
/// resumed at.
///
/// The walk of [`serve_page`], plus the one thing that separates a resumed page
/// from a first one: a cursor resumes a suffix of the sequence it was taken over,
/// so that sequence has to REACH the position the cursor names. A page token is
/// minted only while the walk still had an item to resume at
/// ([`serve_page`] mints `offset + page.len()` with a further item present), so a
/// sequence that ENDS BEFORE `offset` is one no page of this listing ever stopped
/// that far into: `None` is answered and the caller refuses the token it did not
/// issue, instead of reporting an empty page and no `nextToken`, which a client
/// reads as the authoritative end of the sequence. The end itself is reachable —
/// `offset` equal to the sequence's length serves its empty remainder — because
/// that is where a walk whose tail was removed after the token was minted
/// resumes.
pub fn resume_page<T, I>(
    selected: I,
    offset: usize,
    page_size: usize,
    filter_fingerprint: String,
) -> Option<(Vec<T>, Option<String>)>
where
    I: IntoIterator<Item = T>,
{
    let (page, next_token, reached) = walk_page(selected, offset, page_size, filter_fingerprint);
    reached.then_some((page, next_token))
}

/// The walk both entry points above perform, answering additionally whether the
/// sequence reached `offset` at all — which is what tells a resumed walk apart
/// from one asked to start past the end of its sequence.
fn walk_page<T, I>(
    selected: I,
    offset: usize,
    page_size: usize,
    filter_fingerprint: String,
) -> (Vec<T>, Option<String>, bool)
where
    I: IntoIterator<Item = T>,
{
    let mut items = selected.into_iter();
    // What the page resumes after, COUNTED rather than discarded, so the walk
    // knows whether the sequence holds the position it was asked to resume at.
    let before = items.by_ref().take(offset).count();
    let mut page: Vec<T> = items.take(page_size.saturating_add(1)).collect();
    let has_more = page.len() > page_size;
    page.truncate(page_size);
    let next_token = has_more.then(|| {
        encode_page_token(&PageCursor {
            offset: offset.saturating_add(page.len()),
            filter_fingerprint,
        })
    });
    (page, next_token, before == offset)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState>)
}
