// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devopsagent-devopsagent service.
//!
//! The records here are the single home for every resource this service persists:
//! an AgentSpace (the control-plane resource `CreateAgentSpace` mints) and the
//! backlog tasks, Triggers and assets that live inside one. Handlers read and write
//! them through [`service_state`]; the `to_api` methods are the one place a record is
//! rendered into the modelled wire shape, so `Create`, `Get` and `List` cannot
//! disagree about what a stored resource looks like.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AgentSpace, Asset, AssetFile, AssetFileBody, AssetFileSummary, AssetVersionMetadata,
    IamAuthConfiguration, IdcAuthConfiguration, IdpAuthConfiguration, Recommendation,
    RecommendationContent, Recurrence, ReferenceOutput, ScheduleCondition, ScheduleSpec, Task,
    Trigger, TriggerCondition,
};

/// The most AgentSpaces this emulator stores for the account before
/// `CreateAgentSpace` answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's AgentSpace quota is not stated in the model, so this bound is
/// CHOSEN, not cited: far above any plausible client's count, so normal use never
/// meets it, while the store a caller can grow stays bounded.
pub const MAX_AGENT_SPACES: usize = 1_000;

/// The most backlog tasks one AgentSpace holds before `CreateBacklogTask` answers
/// `ServiceQuotaExceededException`. CHOSEN for the same reason as
/// [`MAX_AGENT_SPACES`]: the model states no task quota.
pub const MAX_TASKS_PER_AGENT_SPACE: usize = 1_000;

/// The most Triggers one AgentSpace holds before `CreateTrigger` answers
/// `ServiceQuotaExceededException`. CHOSEN for the same reason as
/// [`MAX_AGENT_SPACES`]: the model states no Trigger quota.
pub const MAX_TRIGGERS_PER_AGENT_SPACE: usize = 1_000;

/// The most assets one AgentSpace holds before `CreateAsset` answers
/// `ServiceQuotaExceededException`. CHOSEN for the same reason as
/// [`MAX_AGENT_SPACES`]: the model states no asset quota.
pub const MAX_ASSETS_PER_AGENT_SPACE: usize = 1_000;

/// The most files one asset's content materialises.
///
/// CHOSEN: the model bounds the SIZE of a content bundle but not the number of
/// entries in it, so this caps how many records one archive can turn into.
pub const MAX_ASSET_FILES: usize = 1_000;

/// The most bytes of file content one asset's content may carry in total.
///
/// Cited: the model bounds every content blob and text body this service accepts —
/// `AssetZipBytes`, `AssetFileBytes` and `AssetFileText` — at `@length(max:
/// 6291456)`, so that is the largest content one request can carry. A zip bundle's
/// entries are held to the same total once decompressed, which is also what stops a
/// small archive from expanding without bound inside this process.
pub const MAX_ASSET_CONTENT_BYTES: usize = 6_291_456;

/// The version a freshly created asset, and each file created with it, carries.
///
/// `Asset$version` and `AssetFile$version` are the resource's revision counter, so
/// the first stored revision is 1 and an update mints the next one.
pub const INITIAL_ASSET_VERSION: i32 = 1;

/// How many revisions of one asset's content this service keeps readable behind the
/// revision the asset is at.
///
/// CHOSEN: the model reports an asset's revisions (`AssetVersionMetadata`) and lets a
/// read name one (`assetVersion`), but bounds neither how many an asset has nor how
/// many are kept, so this is how far back this emulator serves. Once a further
/// revision is minted the oldest retained one is dropped, which is what keeps one
/// asset's history bounded however many updates arrive.
pub const MAX_ASSET_VERSIONS_RETAINED: usize = 10;

/// What one asset type requires of an asset, and how a refusal names items of it.
///
/// `AssetType` is an open string in the model, so the metadata an asset must carry
/// is not expressed there: it belongs to the type. This table is the one place that
/// knowledge lives, so every asset operation enforces and words it the same way.
#[derive(Debug)]
pub struct AssetTypeRule {
    /// The `assetType` value this rule applies to.
    pub asset_type: &'static str,
    /// How the service names an item of this type when it refuses one.
    pub display_name: &'static str,
    /// The metadata fields an asset of this type must carry.
    pub required_metadata: &'static [&'static str],
}

/// The asset types whose requirements this service enforces, each with the
/// metadata field an item of that type is identified by.
///
/// The model leaves `AssetType` an open string and names its values only by
/// example — `AssetType` is documented as "The type of asset (e.g. skill,
/// artifact)" — and states no metadata requirement for any of them, so what each
/// type requires is the service's own behaviour: every knowledge item of a type
/// must carry the member that type names it by, which is what a refusal of a
/// create without it reports. A type with no entry here imposes no metadata
/// requirement of its own, so a create of one is not refused for a field this
/// table does not know about.
pub const ASSET_TYPE_RULES: &[AssetTypeRule] = &[
    AssetTypeRule {
        asset_type: "skill",
        display_name: "Skill knowledge",
        required_metadata: &["name"],
    },
    AssetTypeRule {
        asset_type: "artifact",
        display_name: "Artifact knowledge",
        required_metadata: &["title"],
    },
    AssetTypeRule {
        asset_type: "attachment",
        display_name: "Attachment knowledge",
        required_metadata: &["filename"],
    },
];

/// The rule for `asset_type`, when this service enforces one for it.
pub fn asset_type_rule(asset_type: &str) -> Option<&'static AssetTypeRule> {
    ASSET_TYPE_RULES
        .iter()
        .find(|rule| rule.asset_type == asset_type)
}

/// The refusal message for an asset whose metadata omits a field its type requires.
///
/// The fixed text is "<field> is required for <type> items": the field is the
/// member of `metadata` that is missing and the type is how the service names items
/// of that asset type ([`AssetTypeRule::display_name`]).
pub fn missing_metadata_field_message(field: &str, display_name: &str) -> String {
    format!("{field} is required for {display_name} items")
}

/// Whether `metadata` carries a value for `field`.
///
/// A member that is absent, null or an empty string carries no value, so it does
/// not satisfy a requirement that the field be provided.
pub fn metadata_field_is_provided(metadata: &::aws_smithy_types::Document, field: &str) -> bool {
    let ::aws_smithy_types::Document::Object(members) = metadata else {
        return false;
    };
    match members.get(field) {
        None | Some(::aws_smithy_types::Document::Null) => false,
        Some(::aws_smithy_types::Document::String(value)) => !value.is_empty(),
        Some(_) => true,
    }
}

/// How long a recorded `@idempotencyToken` replays the result of the create that
/// first used it.
///
/// Cited: `CreateAgentSpaceInput$clientToken`'s own documentation — "Client-provided
/// token to ensure request idempotency. When the same token is provided in
/// subsequent calls, the same response is returned within a 8-hour window." Past
/// the window the entry is expired and pruned, so the token map does not grow for
/// the life of the process.
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// The most idempotency-token entries kept at once. CHOSEN (the model states no
/// such limit) as a hard ceiling on top of [`CLIENT_TOKEN_TTL`]: a create prunes
/// expired entries first and evicts the oldest remaining one if the cap is still
/// met, so a caller cannot grow the map without end inside one TTL window.
pub const MAX_CLIENT_TOKENS: usize = 10_000;

/// The recognised agent-space preference keys.
///
/// Source: the value set of the model's `AgentSpacePreferenceKey` enum —
/// `smithy_query(query="shape", shape="AgentSpacePreferenceKey")`. The
/// `AgentSpacePreferences` map documents that "each key must be a recognized
/// preference key; unrecognized keys are rejected", and the framework's
/// constraint walk does not validate map KEYS, so the handler enforces it.
pub const AGENT_SPACE_PREFERENCE_KEYS: &[&str] = &["elevatedActionsEnabled"];

/// The `AuthFlow` value set: the authentication flows an Operator App can have
/// enabled, each configured and reported independently of the others.
///
/// Source: the value set of the model's `AuthFlow` enum —
/// `smithy_query(query="shape", shape="AuthFlow")`.
pub const AUTH_FLOW_IAM: &str = "iam";
pub const AUTH_FLOW_IDC: &str = "idc";
pub const AUTH_FLOW_IDP: &str = "idp";

/// This service's own endpoint and ARN namespace (`aidevops`), from the model's
/// `aws.api#service` trait — `smithy_query(query="service_info")`.
pub const ARN_NAMESPACE: &str = "aidevops";

/// The prefix every goal identifier carries.
///
/// A Trigger is the schedule of one goal in its agent space, so the identifier
/// that names a Trigger IS that goal's identifier. The model constrains
/// `triggerId` no further than `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128), so the
/// narrower form a goal identifier takes is enforced here.
pub const GOAL_ID_PREFIX: &str = "goal-";

/// The form a goal identifier takes: [`GOAL_ID_PREFIX`] followed by a UUID.
/// Rendered into the refusal a malformed identifier gets, so the caller reads the
/// requirement it missed.
pub const GOAL_ID_PATTERN: &str =
    "^goal-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

/// The member a request naming a goal is validated as, as a JSON pointer: the
/// goal identifier the service resolves, whatever the wire member it arrived on is
/// called. Rendered into the refusal's message and into its
/// `ValidationExceptionField` path.
pub const GOAL_ID_PATH: &str = "/goalId";

/// Whether `value` has the form of a goal identifier, i.e. satisfies
/// [`GOAL_ID_PATTERN`].
///
/// An identifier that does not is refused as invalid rather than looked up: it
/// names no goal, so it can name no Trigger in any agent space.
pub fn is_goal_id(value: &str) -> bool {
    value.strip_prefix(GOAL_ID_PREFIX).is_some_and(is_uuid)
}

/// Whether `value` is a UUID in its canonical 8-4-4-4-12 hexadecimal form, which
/// is what [`GOAL_ID_PATTERN`] states after the prefix.
fn is_uuid(value: &str) -> bool {
    /// The hexadecimal digits each dash-separated group of a UUID carries.
    const GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

    let mut groups = value.split('-');
    for length in GROUP_LENGTHS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != length || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
    }
    groups.next().is_none()
}

/// The refusal message a member whose value fails a regular-expression constraint
/// this service enforces itself carries.
///
/// The fixed text of the class is "1 validation error detected. Value at
/// '<path>' failed to satisfy constraint: Member must satisfy regular expression
/// pattern: <pattern>" — one sentence, with no echo of the rejected value. Its
/// variable parts are the JSON-pointer path of the validated member and the
/// pattern that member must satisfy.
pub fn pattern_validation_message(path: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected. Value at '{path}' failed to satisfy constraint: {}",
        pattern_constraint_message(pattern)
    )
}

/// The constraint clause a `ValidationExceptionField` carries for a member whose
/// value fails a regular-expression constraint: the same clause the message states.
pub fn pattern_constraint_message(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The refusal message an operation in the goal plane answers with when the
/// identifier it was given is well formed but names no goal in the agent space
/// the caller reached.
///
/// A Trigger is the schedule of a goal, so the goal is the resource actually
/// resolved and the one the refusal names. The fixed text is "Goal not found: ";
/// the identifier is taken verbatim from the request.
pub fn goal_not_found_message(goal_id: &str) -> String {
    format!("Goal not found: {goal_id}")
}

/// The refusal message an operation in the backlog plane answers with when the
/// identifier it was given names no task in the agent space the caller reached.
///
/// The fixed text names the kind of resource that was resolved and the identifier
/// the request carried, taken verbatim from it; the parent space is already known
/// to the caller, so it is not repeated — the same form as
/// [`goal_not_found_message`] and [`asset_not_found_message`].
pub fn task_not_found_message(task_id: &str) -> String {
    format!("Task not found: {task_id}")
}

/// The UUID a minted goal identifier carries, rendered from `sequence`.
///
/// The 16 bytes are the store's monotonic sequence and its complement, so no two
/// mints agree; the version and variant bits and the `8-4-4-4-12` text form come
/// from [`service_common::uuid_from_random_bytes`], this tree's one rendering of
/// them.
fn goal_uuid(sequence: u64) -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&(!sequence).to_be_bytes());
    bytes[8..].copy_from_slice(&sequence.to_be_bytes());
    ::service_common::uuid_from_random_bytes(bytes)
}

/// The prefix every recommendation identifier carries.
///
/// A recommendation is named by its own identifier, which the model constrains no
/// further than `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128), so the narrower form
/// the identifier takes is enforced here — the same way a goal identifier's is
/// ([`GOAL_ID_PREFIX`]).
pub const RECOMMENDATION_ID_PREFIX: &str = "rec-";

/// The form a recommendation identifier takes: [`RECOMMENDATION_ID_PREFIX`]
/// followed by a UUID. Rendered into the refusal a malformed identifier gets, so
/// the caller reads the requirement it missed.
pub const RECOMMENDATION_ID_PATTERN: &str =
    "^rec-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

/// The member a request naming a recommendation is validated as, as a JSON
/// pointer. Rendered into the refusal's message and into its
/// `ValidationExceptionField` path.
pub const RECOMMENDATION_ID_PATH: &str = "/recommendationId";

/// Whether `value` has the form of a recommendation identifier, i.e. satisfies
/// [`RECOMMENDATION_ID_PATTERN`].
///
/// An identifier that does not is refused as invalid rather than looked up: it
/// names no recommendation in any agent space.
pub fn is_recommendation_id(value: &str) -> bool {
    value
        .strip_prefix(RECOMMENDATION_ID_PREFIX)
        .is_some_and(is_uuid)
}

/// The refusal message a recommendation operation answers with when the identifier
/// it was given is well formed but names no recommendation in the agent space the
/// caller reached.
///
/// The same form as every other missing-resource refusal of this service
/// ([`asset_not_found_message`], [`goal_not_found_message`]): the kind of resource
/// and the identifier the request carried, taken verbatim from the request. The
/// parent space is already known to the caller, so it is not repeated.
pub fn recommendation_not_found_message(recommendation_id: &str) -> String {
    format!("Recommendation not found: {recommendation_id}")
}

/// The refusal message a recommendation read answers with when the version it names
/// is not one the recommendation has: the same form as
/// [`recommendation_not_found_message`], naming the version that was asked for — as
/// [`asset_version_not_found_message`] does for an asset revision.
pub fn recommendation_version_not_found_message(recommendation_id: &str, version: i64) -> String {
    format!("Recommendation not found: {recommendation_id} version {version}")
}

/// The ARN of one AgentSpace, in the partition and region the request was signed
/// for and the account the request is served in.
///
/// This service's ARN namespace is [`ARN_NAMESPACE`] and an AgentSpace is reached
/// under `agent-space/<agentSpaceId>`, which is the resource path its own
/// operations address it by.
pub fn agent_space_arn(region: &str, agent_space_id: &str) -> String {
    let account_id = caller_account_id();
    format!("arn:{ARN_PARTITION}:{ARN_NAMESPACE}:{region}:{account_id}:agent-space/{agent_space_id}")
}

/// The partition every ARN this emulator mints names: this emulator presents one
/// commercial-partition account, so every resource it reports lives in `aws`.
const ARN_PARTITION: &str = "aws";

/// The region a request whose SigV4 credential scope names none is served in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The longest credential-scope region field this service reads off a request.
///
/// Every region code AWS publishes is far shorter; the bound exists because the
/// field is caller-controlled, so without it one request could put a host label of
/// arbitrary size into a minted URL.
const MAX_REGION_LEN: usize = 32;

/// The region this request was signed for, or [`DEFAULT_REGION`] when it names
/// none.
///
/// Constrained to the `[a-z0-9-]` segment a region code is made of and to
/// [`MAX_REGION_LEN`], so a request signed for a nonsense region cannot mint a URL
/// that fails its own modelled pattern.
pub fn region_of(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_LEN
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The AWS account whose credentials every request this emulator serves is signed
/// with, which is the account scope an AgentSpace lookup is performed in.
///
/// This emulator presents exactly one account, so the lookup scope of every request
/// is this id.
pub fn caller_account_id() -> &'static str {
    ::emulator_server::emulator_account_id()
}

/// The refusal message a control-plane operation answers with when the
/// `agentSpaceId` it was given names no AgentSpace in the caller's account.
///
/// The not-found refusal of this service's control plane names BOTH the identifier
/// the request carried and the account the lookup ran in, each double-quoted, so an
/// operator can tell an AgentSpace that was deleted from a lookup performed against
/// a different account than they expected. The id is taken verbatim from the
/// request and the account from the calling identity ([`caller_account_id`]); the
/// model states no message text, so the wording is the service's own.
pub fn agent_space_not_found_message(agent_space_id: &str) -> String {
    let account_id = caller_account_id();
    format!(
        "Could not find Agent Space with provided agentSpaceId \"{agent_space_id}\" in accountId \"{account_id}\""
    )
}

/// The message a refusal of an AgentSpace the caller cannot reach carries.
///
/// An operation addressed to a resource INSIDE an AgentSpace — an asset, a
/// Trigger — authorises the caller against that space before it can know whether
/// anything inside it exists, so a space it cannot see fails closed: the refusal
/// names no identifier at all, because naming one would tell a caller which spaces
/// exist outside their authorization. The text is wholly fixed, with no variable
/// part.
pub const ACCESS_DENIED_MESSAGE: &str = "Access denied";

/// The refusal message an asset operation answers with when the `assetId` it was
/// given names no asset in an AgentSpace the caller can reach.
///
/// The fixed text names the kind of resource and the identifier the request
/// carried, taken verbatim from the request; the parent space is already known to
/// the caller, so it is not repeated.
pub fn asset_not_found_message(asset_id: &str) -> String {
    format!("Asset not found: {asset_id}")
}

/// The refusal message an asset read answers with when the version it names is not
/// one the asset has: the same form as [`asset_not_found_message`], naming the
/// revision that was asked for.
pub fn asset_version_not_found_message(asset_id: &str, asset_version: i32) -> String {
    format!("Asset not found: {asset_id} version {asset_version}")
}

/// The refusal message a file operation answers with when the asset holds no file
/// at the path the request names: the same form as [`asset_not_found_message`],
/// naming the path that was asked for.
pub fn asset_file_not_found_message(path: &str) -> String {
    format!("Asset file not found: {path}")
}

/// The refusal message a create answers with when a value cannot name a file of
/// the asset ([`names_a_file_of_the_asset`]).
pub fn not_a_file_of_the_asset_message(path: &str) -> String {
    format!("{path} does not name a file of the asset")
}

/// The refusal message a file create answers with when the asset's content IS one
/// file and the asset already holds it.
///
/// How many files an asset holds is a property of its TYPE, so that is what the
/// refusal reports: the fixed text is "Assets of type '<assetType>' support only
/// one file. Use UpdateAssetFile to replace the existing file.", whose only
/// variable part is the asset's own `assetType`, read from the stored asset. It
/// names neither the asset nor the path it already holds, and it points the caller
/// at the operation that replaces a stored file.
pub fn asset_type_holds_one_file_message(asset_type: &str) -> String {
    format!(
        "Assets of type '{asset_type}' support only one file. Use UpdateAssetFile to replace the existing file."
    )
}

/// The refusal message a file create answers with when the asset already holds a
/// file at the path the request names.
pub fn asset_file_exists_message(asset_id: &str, path: &str) -> String {
    format!("A file already exists at {path} in asset {asset_id}")
}

/// The refusal message an asset operation answers with when the content it would
/// store exceeds [`MAX_ASSET_CONTENT_BYTES`].
pub fn asset_content_size_exceeded_message() -> String {
    format!("The content exceeds the maximum size of {MAX_ASSET_CONTENT_BYTES} bytes for one asset")
}

/// The refusal message an asset operation answers with when one asset would hold
/// more than [`MAX_ASSET_FILES`] files.
pub fn asset_file_quota_message() -> String {
    format!("The request would exceed the maximum number of files ({MAX_ASSET_FILES}) for one asset")
}

/// The most metadata fields one stored asset file holds.
///
/// CHOSEN: the model types a file's `metadata` as an open `Document` and bounds
/// neither its fields nor its size, while an update MERGES the fields it names into
/// the ones the file already carries — so this is what keeps the metadata of one
/// file from growing across updates without end.
pub const MAX_ASSET_FILE_METADATA_FIELDS: usize = 100;

/// The refusal message a file operation answers with when one file's metadata would
/// hold more than [`MAX_ASSET_FILE_METADATA_FIELDS`] fields.
pub fn asset_file_metadata_quota_message() -> String {
    format!(
        "The request would exceed the maximum number of metadata fields ({MAX_ASSET_FILE_METADATA_FIELDS}) for one asset file"
    )
}

/// The longest `AssetFilePath` this service stores, from the model's own
/// `@length(min: 1, max: 512)` (`smithy_query(query="shape",
/// shape="AssetFilePath")`).
pub const MAX_ASSET_FILE_PATH_LEN: usize = 512;

/// Whether `path` names a file OF an asset: an `AssetFilePath` (the model's
/// `^[a-zA-Z0-9_./ ()-]+$` and its length bound, [`MAX_ASSET_FILE_PATH_LEN`]) that
/// is relative to the asset's own root.
///
/// The modelled pattern admits `.` and `/`, so it also admits a path that walks out
/// of the asset (`..`) or starts at a root (`/`). Such a path names no file of the
/// asset, so it is refused for a path a request names and for an entry an archive
/// carries alike.
pub fn names_a_file_of_the_asset(path: &str) -> bool {
    is_asset_file_path(path)
        && !path.starts_with('/')
        && !path.split('/').any(|segment| segment == "..")
}

/// Whether `path` is an `AssetFilePath`: the model's `^[a-zA-Z0-9_./ ()-]+$`
/// pattern and its length bound ([`MAX_ASSET_FILE_PATH_LEN`]).
///
/// The framework already refuses a modelled member that violates either, before a
/// handler runs; what needs the check here is a name an ARCHIVE carries, which
/// reaches no modelled member on the way in and so is held to the same requirement
/// as the path a request names.
fn is_asset_file_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_ASSET_FILE_PATH_LEN
        && path.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b' ' | b'(' | b')' | b'-')
        })
}

/// The URL operators use to reach the Operator App of one AgentSpace.
///
/// One host per AgentSpace in the region the enabling request was signed for,
/// under this service's own namespace. It satisfies `OperatorAppUrl`'s
/// `^https://[a-zA-Z0-9.-]+(/.*)?$` for every `AgentSpaceId` (`^[a-zA-Z0-9-]{1,64}$`)
/// and region [`region_of`] admits.
pub fn operator_app_url(agent_space_id: &str, region: &str) -> String {
    format!("https://{agent_space_id}.operator.{ARN_NAMESPACE}.{region}.on.aws")
}

/// The partition and account id of an IAM role ARN
/// (`arn:<partition>:iam::<account>:role/<name>`).
///
/// `RoleArn`'s modelled pattern already holds for any value that reaches a
/// handler, so this only splits what the pattern guarantees is there.
pub fn role_arn_partition_and_account(role_arn: &str) -> Option<(&str, &str)> {
    let mut fields = role_arn.split(':');
    let (Some("arn"), Some(partition), Some("iam"), Some(_), Some(account)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    (!partition.is_empty() && !account.is_empty()).then_some((partition, account))
}

/// The instance id of an AWS Identity Center instance ARN
/// (`arn:<partition>:sso:::instance/<instance id>`), or `None` when the value names
/// no IdC instance.
pub fn idc_instance_id(instance_arn: &str) -> Option<&str> {
    let mut fields = instance_arn.split(':');
    let (Some("arn"), Some(_), Some("sso"), Some(_), Some(_), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let id = resource.strip_prefix("instance/")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// The ARN of the IdC application an `idc` enablement creates.
///
/// AWS Identity Center names an application
/// `arn:<partition>:sso::<account>:application/<instance id>/<application id>`: the
/// partition and account are the ones the caller's `operatorAppRoleArn` names, the
/// instance the one its `idcInstanceArn` names, and the application id comes from
/// this service's monotonic sequence, so no two enablements share one.
pub fn idc_application_arn(
    partition: &str,
    account: &str,
    instance_id: &str,
    application_id: &str,
) -> String {
    format!("arn:{partition}:sso::{account}:application/{instance_id}/{application_id}")
}

/// Convert a stored instant into the modelled `Timestamp` representation.
///
/// Every timestamp a response carries is derived from the stored instant, so two
/// reads of one resource render byte-identical values.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// A persisted AgentSpace and the backlog tasks it owns.
#[derive(Debug, Clone)]
pub struct AgentSpaceRecord {
    pub agent_space_id: String,
    pub name: String,
    pub description: Option<String>,
    pub locale: Option<String>,
    pub kms_key_arn: Option<String>,
    /// Only the preferences the caller configured; an unset preference takes its
    /// documented default and is not materialised here.
    pub preferences: HashMap<String, bool>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tasks: HashMap<String, TaskRecord>,
    /// The Triggers created in this AgentSpace, keyed by `triggerId`. Bounded by
    /// [`MAX_TRIGGERS_PER_AGENT_SPACE`].
    pub triggers: HashMap<String, TriggerRecord>,
    /// The assets created in this AgentSpace, keyed by their minted `assetId`.
    /// Bounded by [`MAX_ASSETS_PER_AGENT_SPACE`]. An asset is reachable only
    /// through its space, so deleting the space takes its assets with it.
    pub assets: HashMap<String, AssetRecord>,
    /// The recommendations this AgentSpace holds, keyed by `recommendationId`. A
    /// recommendation is reachable only through the space it belongs to, so an
    /// identifier belonging to another space is as absent as one that never
    /// existed.
    pub recommendations: HashMap<String, RecommendationRecord>,
    /// The Operator App configured for this AgentSpace, absent until an
    /// authentication flow is enabled for it.
    pub operator_app: Option<OperatorAppRecord>,
}

impl AgentSpaceRecord {
    /// The modelled `AgentSpace` for this record.
    pub fn to_api(&self) -> AgentSpace {
        AgentSpace {
            name: self.name.clone(),
            description: self.description.clone(),
            locale: self.locale.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            kms_key_arn: self.kms_key_arn.clone(),
            agent_space_id: self.agent_space_id.clone(),
            preferences: (!self.preferences.is_empty()).then(|| self.preferences.clone()),
        }
    }

    /// The stored tags, omitted entirely when the resource carries none.
    pub fn tags_to_api(&self) -> Option<HashMap<String, String>> {
        (!self.tags.is_empty()).then(|| self.tags.clone())
    }
}

/// The persisted Operator App of one AgentSpace: the URL operators reach it at,
/// and every authentication flow currently enabled on it.
///
/// The flows are independent — the model reports each one in its own output member
/// and lets a disable name a single flow — so enabling one leaves the others as
/// they were.
#[derive(Debug, Clone)]
pub struct OperatorAppRecord {
    pub operator_app_url: String,
    pub iam: Option<IamFlowRecord>,
    pub idc: Option<IdcFlowRecord>,
    pub idp: Option<IdpFlowRecord>,
}

impl OperatorAppRecord {
    /// An Operator App reachable at `operator_app_url` with no flow enabled yet.
    pub fn new(operator_app_url: String) -> Self {
        Self {
            operator_app_url,
            iam: None,
            idc: None,
            idp: None,
        }
    }

    /// Whether any authentication flow is enabled, which is what makes the
    /// Operator App reachable at all.
    pub fn is_enabled(&self) -> bool {
        self.iam.is_some() || self.idc.is_some() || self.idp.is_some()
    }

    /// The URL to report, which exists only while a flow is enabled.
    pub fn url_to_api(&self) -> Option<String> {
        self.is_enabled().then(|| self.operator_app_url.clone())
    }

    /// The modelled `IamAuthConfiguration`, when the `iam` flow is enabled.
    pub fn iam_to_api(&self) -> Option<IamAuthConfiguration> {
        self.iam.as_ref().map(IamFlowRecord::to_api)
    }

    /// The modelled `IdcAuthConfiguration`, when the `idc` flow is enabled.
    pub fn idc_to_api(&self) -> Option<IdcAuthConfiguration> {
        self.idc.as_ref().map(IdcFlowRecord::to_api)
    }

    /// The modelled `IdpAuthConfiguration`, when the `idp` flow is enabled.
    pub fn idp_to_api(&self) -> Option<IdpAuthConfiguration> {
        self.idp.as_ref().map(IdpFlowRecord::to_api)
    }
}

/// The enabled `iam` authentication flow of one Operator App.
#[derive(Debug, Clone)]
pub struct IamFlowRecord {
    pub operator_app_role_arn: String,
    pub created_at: SystemTime,
    /// Set when a later enablement reconfigured this flow; absent while the flow
    /// still holds what the first enablement stored.
    pub updated_at: Option<SystemTime>,
}

impl IamFlowRecord {
    pub fn to_api(&self) -> IamAuthConfiguration {
        IamAuthConfiguration {
            operator_app_role_arn: self.operator_app_role_arn.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: self.updated_at.map(to_date_time),
        }
    }
}

/// The enabled `idc` authentication flow of one Operator App.
#[derive(Debug, Clone)]
pub struct IdcFlowRecord {
    pub operator_app_role_arn: String,
    pub idc_instance_arn: String,
    /// The IdC application this enablement created in the named instance.
    pub idc_application_arn: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: Option<SystemTime>,
}

impl IdcFlowRecord {
    pub fn to_api(&self) -> IdcAuthConfiguration {
        IdcAuthConfiguration {
            operator_app_role_arn: self.operator_app_role_arn.clone(),
            idc_instance_arn: self.idc_instance_arn.clone(),
            idc_application_arn: self.idc_application_arn.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: self.updated_at.map(to_date_time),
        }
    }
}

/// The enabled `idp` authentication flow of one Operator App.
#[derive(Clone)]
pub struct IdpFlowRecord {
    pub operator_app_role_arn: String,
    pub issuer_url: String,
    pub client_id: String,
    /// The OIDC client secret. `@sensitive` in the model and reported by no
    /// operation: it is held only so a later update can replace it.
    pub client_secret: String,
    pub provider: String,
    pub created_at: SystemTime,
    pub updated_at: Option<SystemTime>,
}

/// `Debug` is written by hand rather than derived so the client secret is never
/// rendered: this record reaches a formatter through the state struct's own
/// `Debug`, and the secret is `@sensitive` in the model.
impl ::std::fmt::Debug for IdpFlowRecord {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("IdpFlowRecord")
            .field("operator_app_role_arn", &self.operator_app_role_arn)
            .field("issuer_url", &self.issuer_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("provider", &self.provider)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl IdpFlowRecord {
    pub fn to_api(&self) -> IdpAuthConfiguration {
        IdpAuthConfiguration {
            issuer_url: self.issuer_url.clone(),
            client_id: self.client_id.clone(),
            operator_app_role_arn: self.operator_app_role_arn.clone(),
            provider: self.provider.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: self.updated_at.map(to_date_time),
        }
    }
}

/// A persisted backlog task.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: String,
    pub agent_space_id: String,
    pub title: String,
    pub description: Option<String>,
    pub task_type: String,
    pub priority: String,
    pub status: String,
    pub reference: Option<ReferenceOutput>,
    pub execution_id: Option<String>,
    pub metadata: Option<::aws_smithy_types::Document>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub version: i32,
}

impl TaskRecord {
    /// The modelled `Task` for this record.
    pub fn to_api(&self) -> Task {
        Task {
            agent_space_id: self.agent_space_id.clone(),
            task_id: self.task_id.clone(),
            execution_id: self.execution_id.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            reference: self.reference.clone(),
            task_type: self.task_type.clone(),
            priority: self.priority.clone(),
            status: self.status.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            version: self.version,
            support_metadata: None,
            metadata: self.metadata.clone(),
            primary_task_id: None,
            status_reason: None,
            has_linked_tasks: false,
        }
    }
}

/// A persisted asset: what it is, the metadata describing it, and the files its
/// content materialised into.
///
/// The files are keyed by path in sorted order, so a read of one asset renders its
/// content in the same order however it was created.
#[derive(Debug, Clone)]
pub struct AssetRecord {
    pub asset_id: String,
    pub agent_space_id: String,
    pub asset_type: String,
    /// The metadata the CALLER described this asset with. `Asset$metadata` is
    /// required on the wire, so an asset created without any is stored with an
    /// empty set of members rather than nothing; what the SERVICE maintains in
    /// that document is added where the asset is served ([`AssetRecord::to_api`]).
    pub metadata: ::aws_smithy_types::Document,
    /// The lifecycle state of this asset, reported as the `status` member of the
    /// metadata document every read of it serves.
    pub status: String,
    pub version: i32,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the asset reached the revision [`Self::version`] names.
    ///
    /// A revision is created when the asset moves to it, which for the asset's
    /// first revision is the asset's own creation, so when each revision came into
    /// being is tracked per revision rather than read off the asset.
    pub version_created_at: SystemTime,
    /// The files this asset holds, by path: the content of the revision
    /// [`Self::version`] names.
    pub files: BTreeMap<String, AssetFileRecord>,
    /// Every earlier revision of this asset, by that revision's version number.
    ///
    /// A revision of an asset is a snapshot of its content, so the content a
    /// revision was read with stays readable at that version once a later one
    /// supersedes it. Bounded by [`MAX_ASSET_VERSIONS_RETAINED`].
    pub prior_versions: BTreeMap<i32, AssetRevisionRecord>,
    /// The external source the asset's content is imported from, when the create
    /// named one instead of supplying the content inline. Held so a read reports
    /// where the content comes from; nothing is fetched, because this emulator
    /// makes no outbound connections, so the import materialises no files.
    pub source_url: Option<String>,
    /// Whether this asset's content IS one file.
    ///
    /// `AssetContent` is "a single file, a zip bundle, or a source URL to import
    /// from", so an asset either holds the ONE file its content named or the SET
    /// of files a bundle or an import materialises. Which of the two an asset is,
    /// is settled by the content the create carried, so it is persisted with the
    /// asset rather than guessed at later from how many files it happens to hold.
    pub content_is_one_file: bool,
}

impl AssetRecord {
    /// The modelled `Asset` for this record.
    pub fn to_api(&self) -> Asset {
        Asset {
            asset_id: self.asset_id.clone(),
            asset_type: self.asset_type.clone(),
            metadata: metadata_with_maintained(
                Some(&self.metadata),
                &[(METADATA_STATUS_FIELD, &self.status)],
            ),
            version: self.version,
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// The bytes of content this asset currently holds, across all of its files.
    pub fn content_bytes(&self) -> usize {
        self.files
            .values()
            .map(AssetFileRecord::content_bytes)
            .fold(0usize, |total, size| total.saturating_add(size))
    }

    /// The files this asset's content held as of `version`, or `None` when that is
    /// no revision this asset has (one it never reached, or one no longer retained
    /// — [`MAX_ASSET_VERSIONS_RETAINED`]).
    pub fn files_at(&self, version: i32) -> Option<&BTreeMap<String, AssetFileRecord>> {
        if version == self.version {
            Some(&self.files)
        } else {
            self.prior_versions
                .get(&version)
                .map(|revision| &revision.files)
        }
    }

    /// The modelled metadata of every revision this asset still has, oldest first.
    ///
    /// One entry per revision the asset holds: the earlier ones it retains, then
    /// the revision it is at. A revision's `createdAt` is the instant the asset
    /// reached it and its `updatedAt` the instant of the last change made while it
    /// was the asset's current revision — so the revision the asset is at reports
    /// the asset's own `updatedAt`, and a superseded one keeps the instant it was
    /// last changed. A revision no longer retained
    /// ([`MAX_ASSET_VERSIONS_RETAINED`]) is not reported, because nothing of it is
    /// readable any more.
    pub fn versions_to_api(&self) -> Vec<AssetVersionMetadata> {
        self.prior_versions
            .iter()
            .map(|(version, revision)| AssetVersionMetadata {
                version: *version,
                created_at: to_date_time(revision.created_at),
                updated_at: to_date_time(revision.updated_at),
            })
            .chain(::std::iter::once(AssetVersionMetadata {
                version: self.version,
                created_at: to_date_time(self.version_created_at),
                updated_at: to_date_time(self.updated_at),
            }))
            .collect()
    }

    /// Move this asset to its next revision, freezing the content it holds now as
    /// the revision it is leaving, and answer the version the asset is now at.
    ///
    /// Every file of the content belongs to the revision the asset is at —
    /// `AssetFile$version` is "the asset version this file belongs to" — so the
    /// files carried into the new revision report it, while the frozen snapshot
    /// keeps reporting the one it was read with. The oldest retained revision is
    /// dropped once the history is longer than [`MAX_ASSET_VERSIONS_RETAINED`].
    pub fn begin_next_version(&mut self, now: SystemTime) -> i32 {
        self.prior_versions.insert(
            self.version,
            AssetRevisionRecord {
                created_at: self.version_created_at,
                // The revision being left was last changed when the asset was, so
                // its `updatedAt` is the one the asset carries before this change
                // moves on to the next revision.
                updated_at: self.updated_at,
                files: self.files.clone(),
            },
        );
        while self.prior_versions.len() > MAX_ASSET_VERSIONS_RETAINED {
            let oldest = self.prior_versions.keys().next().copied();
            match oldest {
                Some(version) => {
                    self.prior_versions.remove(&version);
                }
                None => break,
            }
        }
        self.version = self.version.saturating_add(1);
        self.version_created_at = now;
        self.updated_at = now;
        for file in self.files.values_mut() {
            file.version = self.version;
        }
        self.version
    }
}

/// One superseded revision of an asset: the content it held, and when that
/// revision came into being and was last changed.
///
/// A revision is frozen once the asset moves past it, so both instants and the
/// files stay as they were for as long as the revision is retained.
#[derive(Debug, Clone)]
pub struct AssetRevisionRecord {
    /// When the asset reached this revision.
    pub created_at: SystemTime,
    /// The last change made to the asset while this was the revision it was at.
    pub updated_at: SystemTime,
    /// The files this asset's content held as of this revision, by path.
    pub files: BTreeMap<String, AssetFileRecord>,
}

/// One persisted file of an asset's content.
#[derive(Debug, Clone)]
pub struct AssetFileRecord {
    pub path: String,
    pub body: AssetFileBody,
    pub metadata: Option<::aws_smithy_types::Document>,
    pub version: i32,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AssetFileRecord {
    /// The modelled `AssetFile` for this record.
    pub fn to_api(&self) -> AssetFile {
        AssetFile {
            path: self.path.clone(),
            content: self.body.clone(),
            metadata: self.metadata_to_api(),
            version: self.version,
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// The modelled `AssetFileSummary` for this record: every member of
    /// [`AssetFileRecord::to_api`] except the content itself, which a listing does
    /// not carry. Rendered here so a listing and a read of one file cannot
    /// disagree about the file's path, metadata, revision or timestamps.
    pub fn to_summary(&self) -> AssetFileSummary {
        AssetFileSummary {
            path: self.path.clone(),
            metadata: self.metadata_to_api(),
            version: self.version,
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// The metadata document served for this file: the members the caller
    /// described it with, completed with the media type of the content stored for
    /// it ([`metadata_with_maintained`]).
    fn metadata_to_api(&self) -> Option<::aws_smithy_types::Document> {
        Some(metadata_with_maintained(
            self.metadata.as_ref(),
            &[(METADATA_MIME_TYPE_FIELD, file_media_type(&self.body))],
        ))
    }

    /// The size of this file's content, in bytes: the bytes themselves, or the
    /// encoded length of the text.
    pub fn content_bytes(&self) -> usize {
        body_bytes(&self.body)
    }
}

/// The CONTENT of one file body as the bytes a file of that content holds: the
/// bytes themselves, or the encoding of the text.
///
/// The model types a file's content as either TEXT or BYTES, but a file IS a
/// stream of bytes whichever member it was written through, so this is the one
/// place that correspondence lives — what the bytes of a stored file are, for
/// anything that has to hand the file's content on as bytes.
pub fn body_content(body: &AssetFileBody) -> &[u8] {
    match body {
        AssetFileBody::Bytes(bytes) => bytes.as_slice(),
        AssetFileBody::Text(text) => text.as_bytes(),
    }
}

/// The size of one file body, in bytes: the bytes themselves, or the encoded length
/// of the text.
///
/// Taken off the body rather than off a stored record, so the content a request
/// carries is measured against what an asset holds before anything is persisted.
pub fn body_bytes(body: &AssetFileBody) -> usize {
    body_content(body).len()
}

/// What a document node IS, for a refusal that has to tell the caller what they
/// sent instead of a set of members.
pub fn document_kind(document: &::aws_smithy_types::Document) -> &'static str {
    match document {
        ::aws_smithy_types::Document::Object(_) => "a set of members",
        ::aws_smithy_types::Document::Array(_) => "a list",
        ::aws_smithy_types::Document::Number(_) => "a number",
        ::aws_smithy_types::Document::String(_) => "a string",
        ::aws_smithy_types::Document::Bool(_) => "a boolean",
        ::aws_smithy_types::Document::Null => "nothing",
    }
}

/// An empty set of document members: what an asset's required `metadata` holds
/// when the create named none.
pub fn empty_metadata() -> ::aws_smithy_types::Document {
    ::aws_smithy_types::Document::Object(HashMap::new())
}

/// The `status` member of an asset's metadata: the lifecycle state the service
/// keeps for the asset itself.
pub const METADATA_STATUS_FIELD: &str = "status";

/// The lifecycle state of an asset that exists and is readable.
///
/// The model leaves `Asset$metadata` an open `Document` and names no lifecycle
/// member, so this is the service's own behaviour: an asset is created ready —
/// `CreateAsset` answers with it already readable — so there is no transient state
/// to pass through and no later state but gone, which `DeleteAsset` reaches by
/// removing the asset.
pub const ASSET_STATUS_ACTIVE: &str = "ACTIVE";

/// The `mimeType` member of an asset file's metadata: the media type of the
/// content the service holds for that file.
pub const METADATA_MIME_TYPE_FIELD: &str = "mimeType";

/// The media type of one stored file's content.
///
/// The model types a file's content as either TEXT (`AssetFileText`) or BYTES
/// (`AssetFileBytes`), and that is what the service knows about it: text content is
/// the markdown this service's assets are authored in, and bytes are an opaque
/// stream, because nothing about them says what they are.
pub fn file_media_type(body: &AssetFileBody) -> &'static str {
    match body {
        AssetFileBody::Text(_) => MEDIA_TYPE_MARKDOWN,
        AssetFileBody::Bytes(_) => MEDIA_TYPE_OCTET_STREAM,
    }
}

/// The media type of text content an asset holds.
pub const MEDIA_TYPE_MARKDOWN: &str = "text/markdown";

/// The media type of binary content an asset holds: a stream of bytes whose
/// meaning the service is told nothing about.
pub const MEDIA_TYPE_OCTET_STREAM: &str = "application/octet-stream";

/// The metadata document this service SERVES for a resource, given the document
/// the caller described it with.
///
/// A `metadata` document describes its resource, and part of that description is
/// the service's own rather than the caller's: the lifecycle state of an asset, the
/// media type of a file's content. Those members are reported alongside the ones
/// the caller described and never replace one of them, so a document a caller
/// round-trips through this service comes back carrying exactly the members they
/// wrote, plus what the service knows about the resource itself.
///
/// A described document that is not a set of members has no members to complete, so
/// it is reported as it was stored.
pub fn metadata_with_maintained(
    described: Option<&::aws_smithy_types::Document>,
    maintained: &[(&str, &str)],
) -> ::aws_smithy_types::Document {
    let mut members = match described {
        Some(::aws_smithy_types::Document::Object(described)) => described.clone(),
        Some(other) => return other.clone(),
        None => HashMap::new(),
    };
    for (field, value) in maintained {
        members
            .entry((*field).to_string())
            .or_insert_with(|| ::aws_smithy_types::Document::String((*value).to_string()));
    }
    ::aws_smithy_types::Document::Object(members)
}

/// A persisted Trigger: the schedule it fires on, the action it performs, and the
/// status it is in.
///
/// The schedule is held as the structured specification the caller supplied, when
/// they supplied one, plus the EventBridge expression the Trigger fires on — the
/// caller's own expression, or the one derived from the specification. That is
/// exactly what `ScheduleCondition` reports: "Trigger responses always use this
/// condition, include the persisted or derived expression, and also include spec
/// when the trigger was created from a structured schedule".
#[derive(Debug, Clone)]
pub struct TriggerRecord {
    pub trigger_id: String,
    pub agent_space_id: String,
    pub trigger_type: String,
    /// The EventBridge expression this Trigger fires on; always present, because
    /// a structured schedule derives one.
    pub schedule_expression: String,
    /// The structured schedule this Trigger was created from, when it was.
    pub schedule_spec: Option<ScheduleSpec>,
    pub action: ::aws_smithy_types::Document,
    pub status: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl TriggerRecord {
    /// The modelled `Trigger` for this record.
    pub fn to_api(&self) -> Trigger {
        Trigger {
            trigger_id: self.trigger_id.clone(),
            agent_space_id: self.agent_space_id.clone(),
            r#type: self.trigger_type.clone(),
            condition: TriggerCondition::Schedule(ScheduleCondition {
                expression: Some(self.schedule_expression.clone()),
                spec: self.schedule_spec.clone(),
            }),
            action: self.action.clone(),
            status: self.status.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }
}

/// A persisted recommendation and every revision of it this service still serves.
///
/// What a recommendation IS stays fixed once it exists — the task that generated
/// it, the goal it was generated against and when it was created — while its
/// content, status, priority and ranking are what a revision carries, because
/// `Recommendation$version` is the resource's "version number for optimistic
/// locking" and a read may name one (`GetRecommendationRequest$recommendationVersion`
/// — "Specific version of the recommendation to retrieve. If not specified,
/// returns the latest version"). So a version stays readable once a later one
/// supersedes it, and the revisions are the one home of everything that changes.
#[derive(Debug, Clone)]
pub struct RecommendationRecord {
    pub recommendation_id: String,
    pub agent_space_id: String,
    /// The task that generated this recommendation.
    pub task_id: String,
    /// The goal it was generated against, when it was generated against one.
    pub goal_id: Option<String>,
    /// The version that goal was at when this recommendation was generated.
    pub goal_version: Option<i64>,
    pub created_at: SystemTime,
    /// Every revision of this recommendation, by the `version` that names it. The
    /// highest is the one it is at; the earlier ones are what a read naming a
    /// version still resolves.
    pub revisions: BTreeMap<i64, RecommendationRevision>,
}

/// What one revision of a recommendation holds: everything about it that a later
/// revision can change.
#[derive(Debug, Clone)]
pub struct RecommendationRevision {
    pub title: String,
    /// `RecommendationContent$summary`: the brief summary, required on the wire.
    pub summary: String,
    /// `RecommendationContent$spec`: the agent-ready specification, when the
    /// revision carries one.
    pub spec: Option<String>,
    pub status: String,
    pub priority: String,
    pub additional_context: Option<String>,
    /// Position in the ranked list, when this revision has been ranked.
    pub rank_position: Option<i32>,
    /// When this revision was last ranked, which only a ranked revision has.
    pub ranked_at: Option<SystemTime>,
    pub updated_at: SystemTime,
}

impl RecommendationRecord {
    /// The version of the latest revision this recommendation has, or `None` for a
    /// record holding no revision at all.
    pub fn latest_version(&self) -> Option<i64> {
        self.revisions.keys().next_back().copied()
    }

    /// The revision `version` names, or the latest one when `version` is `None`;
    /// `None` when that is no revision this recommendation has.
    pub fn revision(&self, version: Option<i64>) -> Option<(i64, &RecommendationRevision)> {
        let version = match version {
            Some(version) => version,
            None => self.latest_version()?,
        };
        self.revisions
            .get(&version)
            .map(|revision| (version, revision))
    }

    /// The modelled `Recommendation` for one revision of this record: the revision
    /// `version` names, or the latest when it names none. `None` when the record
    /// has no such revision, which is what makes a read of an absent version a
    /// not-found rather than a different recommendation.
    pub fn to_api(&self, agent_space_arn: &str, version: Option<i64>) -> Option<Recommendation> {
        let (version, revision) = self.revision(version)?;
        Some(Recommendation {
            agent_space_arn: agent_space_arn.to_string(),
            recommendation_id: self.recommendation_id.clone(),
            task_id: self.task_id.clone(),
            goal_id: self.goal_id.clone(),
            title: revision.title.clone(),
            content: RecommendationContent {
                summary: revision.summary.clone(),
                spec: revision.spec.clone(),
            },
            status: revision.status.clone(),
            priority: revision.priority.clone(),
            goal_version: self.goal_version,
            additional_context: revision.additional_context.clone(),
            rank_position: revision.rank_position,
            ranked_at: revision.ranked_at.map(to_date_time),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(revision.updated_at),
            version,
        })
    }
}

/// The EventBridge expression a structured schedule derives.
///
/// Cited: `ScheduleCondition$expression` — "For a structured schedule response,
/// this is the expression derived by Backlog" — and `TimeRangeSchedule` — "The
/// service derives an EventBridge expression anchored at startAfter and a
/// flexible-window width from the interval to startBefore". A `cron`
/// specification already carries the expression that anchors its window, so that
/// expression IS the derived one. A `timeRange` is anchored at its `startAfter`
/// time of day, and its recurrence chooses the day fields of the six-field
/// EventBridge cron expression (minute, hour, day-of-month, month, day-of-week,
/// year), exactly one of whose two day fields is `?`.
///
/// `None` only when `startAfter` is not a time of day, which `TimeOfDay`'s own
/// `^([01]\d|2[0-3]):[0-5]\d$` pattern already rules out for any value that
/// reaches a handler.
///
/// The flexible-window WIDTH the same sentence describes — the interval from
/// `startAfter` to `startBefore` — has no output member in this model and no place
/// in a cron expression, so it is persisted as part of the specification and
/// reported back there rather than rendered into the derived expression.
pub fn derived_schedule_expression(spec: &ScheduleSpec) -> Option<String> {
    match spec {
        ScheduleSpec::Cron(cron) => Some(cron.expression.clone()),
        ScheduleSpec::timeRange(range) => {
            let (hour, minute) = time_of_day(&range.start_after)?;
            Some(match &range.recurrence {
                Recurrence::Daily(_) => format!("cron({minute} {hour} * * ? *)"),
                Recurrence::Weekly(weekly) => format!(
                    "cron({minute} {hour} ? * {} *)",
                    day_of_week_field(&weekly.day_of_week)
                ),
                Recurrence::Monthly(monthly) => {
                    format!("cron({minute} {hour} {} * ? *)", monthly.day_of_month)
                }
            })
        }
    }
}

/// The hour and minute of a `TimeOfDay` (`HH:MM`, UTC).
fn time_of_day(value: &str) -> Option<(u32, u32)> {
    let (hour, minute) = value.split_once(':')?;
    Some((hour.parse().ok()?, minute.parse().ok()?))
}

/// The day-of-week field an EventBridge cron expression names a `DayOfWeek` with:
/// the first three letters of the model's enum value
/// (`smithy_query(query="shape", shape="DayOfWeek")` — MONDAY..SUNDAY), which is
/// the three-letter day name EventBridge cron takes.
fn day_of_week_field(day_of_week: &str) -> String {
    day_of_week.chars().take(3).collect()
}

/// What one recorded idempotency token remembers: the resource the first create
/// made, and when that happened (so the entry can expire).
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    pub resource_id: String,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-devopsagent-devopsagent. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDevopsagentDevopsagentState {
    /// Every AgentSpace, keyed by its minted `agentSpaceId`. Bounded by
    /// [`MAX_AGENT_SPACES`].
    pub agent_spaces: HashMap<String, AgentSpaceRecord>,
    /// `clientToken` -> the resource the first call with that token created, so an
    /// `@idempotent` create replays its first result. Entries expire after
    /// [`CLIENT_TOKEN_TTL`] and the map is capped at [`MAX_CLIENT_TOKENS`].
    pub client_tokens: HashMap<String, ClientTokenRecord>,
    /// Source of minted resource ids; monotonic, so no id is ever reused.
    pub next_id: u64,
}

impl ComAmazonawsDevopsagentDevopsagentState {
    /// The next value of the store's monotonic id sequence.
    ///
    /// The one place that advances it, so every minted id — whatever form it takes
    /// — comes out of the same sequence and no value is handed out twice.
    fn next_sequence(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Mint a fresh resource id of the form `<prefix>-<hex sequence>`.
    ///
    /// The sequence is monotonic within this emulator, so an id is never handed
    /// out twice. `prefix` must be alphanumeric so the result satisfies both
    /// `AgentSpaceId` (`^[a-zA-Z0-9-]{1,64}$`) and `ResourceId`
    /// (`^[a-zA-Z0-9_.-]+$`, max 128).
    pub fn mint_id(&mut self, prefix: &str) -> String {
        format!("{prefix}-{:016x}", self.next_sequence())
    }

    /// Mint a fresh Trigger identifier.
    ///
    /// A Trigger is the schedule of a goal, so the identifier is a goal
    /// identifier: [`GOAL_ID_PREFIX`] and a UUID minted from the store's own
    /// monotonic sequence, so it satisfies [`is_goal_id`] by construction and no
    /// two mints agree.
    pub fn mint_trigger_id(&mut self) -> String {
        format!("{GOAL_ID_PREFIX}{}", goal_uuid(self.next_sequence()))
    }

    /// The id a previous `@idempotent` create recorded for `client_token`, when
    /// the entry is still inside [`CLIENT_TOKEN_TTL`] at `now`.
    pub fn replayed_id(&self, scope: &str, client_token: &str, now: SystemTime) -> Option<String> {
        let record = self.client_tokens.get(&token_key(scope, client_token))?;
        (now < record.recorded_at + CLIENT_TOKEN_TTL).then(|| record.resource_id.clone())
    }

    /// Record that `client_token` created `resource_id`.
    ///
    /// Prunes the entries whose TTL the clock has already run out before
    /// inserting, and evicts the oldest remaining entry while the map is still at
    /// [`MAX_CLIENT_TOKENS`], so this map is bounded however many creates arrive.
    pub fn record_client_token(
        &mut self,
        scope: &str,
        client_token: &str,
        resource_id: &str,
        now: SystemTime,
    ) {
        self.client_tokens
            .retain(|_, record| now < record.recorded_at + CLIENT_TOKEN_TTL);
        while self.client_tokens.len() >= MAX_CLIENT_TOKENS {
            let oldest = self
                .client_tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.client_tokens.remove(&key);
                }
                None => break,
            }
        }
        self.client_tokens.insert(
            token_key(scope, client_token),
            ClientTokenRecord {
                resource_id: resource_id.to_string(),
                recorded_at: now,
            },
        );
    }

    /// Drop the idempotency-token entries that name any of `resource_ids`.
    ///
    /// Called when those resources are deleted: a token whose resource is gone can
    /// never replay anything, so the entry is reclaimed with the resource instead
    /// of waiting out its TTL.
    pub fn forget_client_tokens_for(&mut self, resource_ids: &[String]) {
        self.client_tokens
            .retain(|_, record| !resource_ids.contains(&record.resource_id));
    }

    /// The stored AgentSpaces in a stable order: by `agentSpaceId`.
    ///
    /// Ids come from the monotonic [`mint_id`](Self::mint_id) sequence and are
    /// fixed-width, so ordering by id IS creation order, and an opaque pagination
    /// token can resume at a position in this sequence by naming the id it stopped
    /// after.
    pub fn agent_spaces_ordered(&self) -> Vec<&AgentSpaceRecord> {
        let mut ordered: Vec<&AgentSpaceRecord> = self.agent_spaces.values().collect();
        ordered.sort_by(|a, b| a.agent_space_id.cmp(&b.agent_space_id));
        ordered
    }
}

/// Idempotency tokens are scoped per create operation, so the same token used on
/// two different creates names two different resources.
fn token_key(scope: &str, client_token: &str) -> String {
    format!("{scope}#{client_token}")
}

/// The prefix every pagination token this service issues carries, so a token it
/// did not issue is rejected instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1:";

/// The prefix of a minted `agentSpaceId`, and the hex width of its sequence.
const AGENT_SPACE_ID_PREFIX: &str = "as-";
const MINTED_SEQUENCE_WIDTH: usize = 16;

/// Encode an opaque pagination token that resumes AFTER `last_id`.
///
/// Opaque to the client (a version marker plus hex of the cursor), and it
/// round-trips through [`decode_next_token`].
pub fn encode_next_token(last_id: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(last_id.as_bytes()))
}

/// The asset revision a pagination token this service issued over one asset's
/// version history resumes after, or `None` when the token is not one it issued.
///
/// A cursor in that listing names a revision by its version number, which is what
/// makes a token that decodes to anything else a token this service did not issue
/// rather than a resume point past every revision.
pub fn decode_version_token(token: &str) -> Option<i32> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let cursor = String::from_utf8(hex::decode(body).ok()?).ok()?;
    cursor.parse().ok()
}

/// The file of an asset a pagination token this service issued over one asset's
/// files resumes after, or `None` when the token is not one it issued.
///
/// A cursor in that listing names a file of the asset by its path, so a token that
/// decodes to anything that cannot name one ([`names_a_file_of_the_asset`]) is a
/// token this service did not issue rather than a resume point past every file.
pub fn decode_asset_file_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let cursor = String::from_utf8(hex::decode(body).ok()?).ok()?;
    names_a_file_of_the_asset(&cursor).then_some(cursor)
}

/// The cursor a token carries, or `None` when the token is not one this service
/// issued.
///
/// A cursor this service issued always names a minted `agentSpaceId`, so a token
/// that decodes to anything else is rejected rather than read as a resume point
/// past every stored agent space.
pub fn decode_next_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let cursor = String::from_utf8(hex::decode(body).ok()?).ok()?;
    let sequence = cursor.strip_prefix(AGENT_SPACE_ID_PREFIX)?;
    let names_an_agent_space = sequence.len() == MINTED_SEQUENCE_WIDTH
        && sequence.chars().all(|c| c.is_ascii_hexdigit());
    names_an_agent_space.then_some(cursor)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDevopsagentDevopsagentState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDevopsagentDevopsagentState>)
}
