// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssm-amazonssm service.
//!
//! The slices below are modelled here, each reached through the one store. The first is
//! **Parameter Store**: `PutParameter`
//! creates or updates a parameter, `GetParameter` reads one back,
//! `GetParametersByPath` reads the parameters stored under one level of a hierarchy
//! ([`parameter_path`], [`is_under_path`]) and `DescribeParameters` reports the metadata
//! of the parameters a Region holds, narrowed by the filters it accepts
//! ([`is_describe_parameters_filter_key`], [`describe_parameter_matches`]). All of them
//! reach the one store below, so whatever a put persisted is what a later read reports —
//! the store is keyed by Region and name, never by the credential a request was signed
//! with, so every principal in the account reads the one state.
//!
//! A parameter lives at a name that is unique *within a Region* (the `PutParameter`
//! `Name` doc: "A parameter name must be unique within an Amazon Web Services Region"),
//! so the store is keyed by `(region, canonical name)`. Each parameter holds its
//! **versions**, oldest first: a put that overwrites an existing name appends a new
//! version rather than replacing the old one, which is what lets a `name:version`
//! selector resolve an earlier value.
//!
//! A version also holds the **labels** that point at it ([`label_is_well_formed`],
//! [`ParameterRecord::attach_label`]): `LabelParameterVersion` writes them,
//! `UnlabelParameterVersion` erases them, and a `name:label` selector resolves through
//! them ([`ParameterRecord::select`]), so a label is held by at most one version of a
//! parameter at a time. `DeleteParameter` and `DeleteParameters` remove a parameter
//! whole, labels and every retained version with it.
//!
//! The second is **SSM documents**: `CreateDocument` registers a document,
//! `UpdateDocument` appends a version to it, `DescribeDocument` reports one of those
//! versions and `GetDocument` reports its content. A document is keyed by
//! `(region, name)` the same way and holds its versions oldest-first
//! ([`DocumentVersionRecord`]), and everything a read reports beyond the request's own
//! members — the schema version, the description, the parameters, the platform types, the
//! hash — is derived from the content that version persisted (see
//! [`parse_document_content`]). The content itself is held exactly as the call supplied
//! it, and a read that asks for another `DocumentFormat` is served the same content
//! re-rendered in it ([`document_format::rendered_content`]).
//!
//! A document a caller created is not the only one an account holds: Systems Manager
//! also PUBLISHES a catalogue of its own documents ([`PUBLISHED_DOCUMENTS`]) in every
//! account and Region, which resolve by name without anybody creating them and are
//! owned by Amazon. A read goes through [`ComAmazonawsSsmAmazonssmState::resolve_document`],
//! which answers with the created document of that name or, failing that, the published
//! one — derived from the catalogue's body by the same content reader, so a published
//! document reports nothing a created one would not. `ListDocuments` reports exactly the
//! documents those reads resolve — the created ones of a Region beside the published
//! catalogue — narrowed by the filters a listing accepts and paged by document name
//! ([`document_listing`]).
//!
//! The third is **Run Command** ([`commands`]): `SendCommand` resolves the document and
//! the managed nodes a send names or describes, persists the command and one invocation
//! per resolved node, `GetCommandInvocation` reports one of those invocations and
//! `ListCommands` reports the commands a Region holds — so what a send persisted is what
//! a later read reports, per Region.
//!
//! The fourth is the **managed nodes** the node describes report
//! ([`ManagedNodeRecord`]), keyed by `(region, node id)`: `DescribeInstanceProperties`
//! projects a node's `properties` and `DescribeInstanceInformation` projects the same
//! record through [`ManagedNodeRecord::to_instance_information`], so both report the one
//! stored node. A node enters the store only by registering — nothing here invents one —
//! so an account that has registered none is reported as holding none. The filter keys a
//! describe accepts are [`is_node_filter_key`] (the property describe) and
//! [`is_instance_information_filter_key`] (the string-filter describe). The same record is
//! also the INVENTORY of that node ([`inventory`]): `GetInventory` projects it as the
//! node's `AWS:InstanceInformation` item, so an inventory query reports what registering
//! persisted rather than a second copy of it.
//!
//! The fifth is the **hybrid activations** ([`ActivationRecord`]), keyed by
//! `(region, activation id)`: `CreateActivation` mints one, `DescribeActivations`
//! reports what it persisted through [`ActivationRecord::to_activation`], narrowed by
//! the filter keys [`is_activation_filter_key`] accepts, and `DeleteActivation` drops it
//! again. The code a create mints is the activation's password and is handed to its
//! creator once; no read reports it.
//!
//! The sixth is the **cloud connectors** that connect Systems Manager to a third-party
//! cloud environment, keyed by `(region, connector id)` and modelled in
//! [`cloud_connectors`]: `CreateCloudConnector` mints one, `UpdateCloudConnector`
//! replaces the members an update carries and `GetCloudConnector` reports what is held.
//! A connector enters the store only by being created, and only under the id the create
//! minted, so an id nothing created names no connector.
//!
//! The seventh is the **resource policies** attached to this account's Systems Manager
//! resources ([`resource_policies`]): `PutResourcePolicy` attaches or rewrites one,
//! `GetResourcePolicies` reports what a resource holds and `DeleteResourcePolicy`
//! detaches it, all three reaching the one store below, keyed by the resource the policy
//! is attached to ([`resource_policies::PolicyResourceKey`]) rather than by the text of
//! the ARN that named it.
//!
//! Then **OpsCenter's OpsItems**, which live in [`ops_items`]: `CreateOpsItem`
//! mints one and `GetOpsItem` reports it, keyed by `(region, ops item id)` the same way,
//! and everything a read reports — the status, the version, who created it and when —
//! comes from what the create persisted. Each write to an OpsItem also appends one event
//! to that OpsItem's **event trail** ([`ops_item_events`]), which is what
//! `ListOpsItemEvents` reports: the trail holds what the writes persisted, so an account
//! that has written no OpsItem has no event to list and a listing appends none. The
//! resources associated with an OpsItem as its **related items** live beside them in
//! [`ops_item_related_items`], keyed by `(region, ops item id, association id)`:
//! `AssociateOpsItemRelatedItem` creates one and `ListOpsItemRelatedItems` reports what it
//! persisted.
//!
//! Then the **patch baselines** Patch Manager holds ([`patch_baselines`]):
//! `CreatePatchBaseline` mints one the account owns, keyed by `(region, baseline id)`,
//! `RegisterDefaultPatchBaseline` records a baseline as the default for the operating
//! system it applies to and `GetDefaultPatchBaseline` reports what is recorded. A
//! baseline nobody created is not the only one an account holds: Systems Manager
//! PREDEFINES one per supported operating system
//! ([`patch_baselines::PREDEFINED_BASELINES`]), which is the default until a register says
//! otherwise — so every operating system has a default and the read invents none.
//!
//! Beside the baselines is the **patch metadata Patch Manager publishes**
//! ([`patch_properties`]): the products, product families, classifications and severities
//! the patches available for an operating system carry, which exist in every account and
//! Region without anybody creating them and which `DescribePatchProperties` reports
//! ([`ComAmazonawsSsmAmazonssmState::patch_property_values`]) — so what that read answers
//! is a projection of the one catalogue, the same for every caller.
//!
//! Then the **resource data sync configurations** ([`resource_data_syncs`]), keyed by
//! `(region, sync name)`: `CreateResourceDataSync` persists one,
//! `ListResourceDataSync` reports what a Region holds, `UpdateResourceDataSync`
//! replaces the source of one and `DeleteResourceDataSync` removes it, so whatever a
//! write persisted is what a later listing reports.
//!
//! Then the **just-in-time node access requests** ([`just_in_time_node_access`]), keyed by
//! `(region, access request id)`: `StartAccessRequest` raises one and `GetAccessToken`
//! reports its status and the credentials an approved one is exchanged for. Whether the
//! account has the workflow at all is read from the documents it holds — an access-request
//! approval policy is an SSM document
//! ([`ComAmazonawsSsmAmazonssmState::has_access_request_approval_policy`]) — so an account that
//! created none has no access-request surface and a read invents no request.
//!
//! Then the **maintenance windows** ([`maintenance_windows`]), keyed by
//! `(region, window id)`: `CreateMaintenanceWindow` mints one and
//! `GetMaintenanceWindow` reports it, so whatever the create persisted is what a later
//! read reports. What the window will DO is not stored but read from its schedule: the
//! next execution is the first instant that cron or rate expression names inside the
//! interval the window is active over
//! ([`maintenance_windows::MaintenanceWindowRecord::next_execution_time`]).
//!
//! Then the **execution previews** ([`execution_previews`]), keyed by
//! `(region, execution preview id)`: `StartExecutionPreview` works out what running an
//! Automation runbook would do — from the steps that runbook declares and the resources its
//! targets resolve to in this store — and persists it, and `GetExecutionPreview` reports
//! what it persisted once the service has had time to work it out. Nothing is run and no
//! resource is touched: a preview is the report of what an execution WOULD do.
//!
//! Last is the account's **service settings** ([`service_settings`]):
//! `GetServiceSetting` reports one, `UpdateServiceSetting` overwrites it and
//! `ResetServiceSetting` restores the value its service team provisioned. The settings an
//! account holds are a catalogue nobody adds to, so only the ones an account has modified
//! are stored and the rest are reported from that catalogue
//! ([`ComAmazonawsSsmAmazonssmState::service_setting`]).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

/// The cloud connectors this service holds, and how a create, an update and a read of
/// one reach the single store below.
pub mod cloud_connectors;

/// Rendering a persisted document's content in the `DocumentFormat` a read asks for.
pub mod document_format;

/// Which operating-system platforms a document's own content leaves it compatible with.
pub mod document_platforms;

/// Which documents a listing of this account's SSM documents walks, the filters that
/// narrow it, and the identity each one is reported by.
pub mod document_listing;

/// Whether a read may have a `SecureString`'s value decrypted for its caller, which is a
/// decision the KMS key the value is sealed under makes rather than this service.
pub mod secure_string;

/// The parameters AWS itself publishes in the `/aws` hierarchy of every Region, which a
/// read resolves beside the parameters this account wrote.
pub mod public_parameters;

/// The reserved hierarchy of that same `/aws` namespace through which a read of Parameter
/// Store serves an Amazon Web Services Secrets Manager secret.
pub mod secretsmanager_reference;

/// A command a caller sent to the managed nodes of one Region, and the invocation each of
/// those nodes holds of it.
pub mod commands;

/// An automation execution a caller started, the step executions it is made of, and the
/// walk through them the clock owes.
pub mod automation;

/// The execution previews of an Automation runbook: what a preview works out from the
/// runbook and the targets, and the status the clock moves it to.
pub mod execution_previews;

/// The State Manager associations this service holds, each with the versions a create and
/// its updates wrote.
pub mod associations;

/// The resource policies attached to this account's Systems Manager resources, and the
/// hash that keeps two writers from overwriting each other's.
pub mod resource_policies;

/// The OpsItems OpsCenter holds: the record a create persists, the rules it is held to
/// and the store the reads project.
pub mod ops_items;

/// Which of the OpsItems a Region holds a `DescribeOpsItems` query selects, and the order
/// it reports them in.
pub mod ops_item_queries;

/// Just-in-time node access: whether an account has the workflow at all, the access
/// requests it holds and the credentials an approved one is exchanged for.
pub mod just_in_time_node_access;

/// Whether the caller's own identity-based IAM policies let a request perform the action
/// it asks for — the decision an operation takes before it resolves anything.
pub mod identity_authorization;

/// The event trail OpsCenter keeps for an OpsItem: the event a write appends and the
/// store `ListOpsItemEvents` projects.
pub mod ops_item_events;

/// The related-item resources an OpsItem carries: the record an association persists, the
/// rules it is held to and the store a listing projects.
pub mod ops_item_related_items;

/// The patch baselines Patch Manager holds, the ones Systems Manager predefines, and
/// which of them is the default for an operating system.
pub mod patch_baselines;

/// The patch metadata Patch Manager publishes for an operating system — the products,
/// product families, classifications and severities its available patches carry — which
/// `DescribePatchProperties` reports.
pub mod patch_properties;

/// The inventory this service holds about the managed nodes of a Region — the
/// `AWS:InstanceInformation` item every registered node has — and how a query filters and
/// aggregates it.
pub mod inventory;

/// The resource data sync configurations this service holds: the record a create
/// persists, the configuration rules it is held to, and how a listing, an update and a
/// delete address one.
pub mod resource_data_syncs;

/// The maintenance windows this service holds: the record a create persists, the schedule
/// grammar it is held to, and the instants that schedule names.
pub mod maintenance_windows;

/// The account-level settings the Systems Manager service teams provision, which
/// `GetServiceSetting` reports and `UpdateServiceSetting`/`ResetServiceSetting` modify.
pub mod service_settings;

/// The one tag set each resource this store holds carries, and how a tag request
/// addresses it.
pub mod tagging;

use service_settings::ServiceSettingRecord;

use crate::ops::types::{
    Activation, AttachmentInformation, AttachmentsSource, DocumentDescription, DocumentParameter,
    DocumentRequires, InstanceInformation, InstanceProperty, Parameter, ParameterHistory,
    ParameterInlinePolicy, ParameterMetadata, Tag, DESCRIBE_ACTIVATIONS_FILTER_KEYS_VALUES,
    INSTANCE_INFORMATION_FILTER_KEY_VALUES, INSTANCE_PROPERTY_FILTER_KEY_VALUES,
    PARAMETER_TIER_VALUES, PARAMETER_TYPE_VALUES,
};

/// The signing name, endpoint prefix and ARN namespace of this service, all three
/// `ssm` (the model's `aws.auth#sigv4` / `aws.api#service` traits).
pub const ARN_SERVICE: &str = "ssm";

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion a parameter ARN carries before the parameter's path, e.g.
/// `arn:aws:ssm:us-east-2:111122223333:parameter/Dev/DBServer` (the `PutParameter`
/// `Name` doc quotes the same prefix).
pub const ARN_RESOURCE_PREFIX: &str = "parameter/";

/// What every ARN starts with, and therefore how `GetParameter` tells a full ARN from
/// a bare parameter name.
pub const ARN_PREFIX: &str = "arn:";

/// `ParameterType` values, from the modelled enum
/// (`crate::ops::types::PARAMETER_TYPE_VALUES`).
pub const TYPE_SECURE_STRING: &str = "SecureString";

/// See [`TYPE_SECURE_STRING`]. A value of this type is a list: "Items in a StringList
/// must be separated by a comma (,)" (`PutParameter`'s `Type` doc).
pub const TYPE_STRING_LIST: &str = "StringList";

/// See [`TYPE_SECURE_STRING`]: the plain, unencrypted scalar value.
pub const TYPE_STRING: &str = "String";

/// The separator between the items of a [`TYPE_STRING_LIST`] value (same doc: "You
/// can't use other punctuation or special character to escape items in the list").
pub const STRING_LIST_SEPARATOR: char = ',';

/// `ParameterTier` values, from the modelled enum
/// (`crate::ops::types::PARAMETER_TIER_VALUES`).
pub const TIER_STANDARD: &str = "Standard";

/// See [`TIER_STANDARD`].
pub const TIER_ADVANCED: &str = "Advanced";

/// See [`TIER_STANDARD`].
pub const TIER_INTELLIGENT_TIERING: &str = "Intelligent-Tiering";

/// Content size a standard parameter holds: "Standard parameters have a content size
/// limit of 4 KB" (`PutParameter`'s `Tier` doc).
pub const STANDARD_VALUE_LIMIT: usize = 4 * 1024;

/// Content size an advanced parameter holds: "Advanced parameters have a content size
/// limit of 8 KB" (same doc).
pub const ADVANCED_VALUE_LIMIT: usize = 8 * 1024;

/// How deep a parameter hierarchy may go: "Parameter hierarchies are limited to a
/// maximum depth of fifteen levels" (`PutParameter`'s `Name` doc), which is what
/// `HierarchyLevelLimitExceededException` reports.
pub const MAX_HIERARCHY_LEVELS: usize = 15;

/// How many versions of one parameter are retained: "Parameter Store retains the 100
/// most recently created versions of a parameter. After this number of versions has
/// been created, Parameter Store deletes the oldest version when a new one is
/// created" (the `ParameterMaxVersionLimitExceeded` doc).
pub const MAX_VERSIONS_RETAINED: usize = 100;

/// How many labels one version of a parameter carries: "A version of a parameter can
/// have a maximum of 10 labels" (`LabelParameterVersion`'s own doc), which
/// `ParameterVersionLabelLimitExceeded` ("A parameter version can have a maximum of ten
/// labels") reports.
pub const MAX_LABELS_PER_VERSION: usize = 10;

/// The prefixes a label may not start with: "Labels can't begin with a number, "aws" or
/// "ssm" (not case sensitive)" (`LabelParameterVersion`'s doc).
pub const RESERVED_LABEL_PREFIXES: &[&str] = &["aws", "ssm"];

/// Whether `label` meets the documented label requirements, so that attaching it
/// associates it with a parameter version rather than reporting it among the invalid
/// labels: "Labels can contain letters (case sensitive), numbers, periods (.), hyphens
/// (-), or underscores (_). Labels can't begin with a number, "aws" or "ssm" (not case
/// sensitive)" (`LabelParameterVersion`'s doc).
///
/// The same doc's length rule ("A label can have a maximum of 100 characters") is the
/// modelled `ParameterLabel` length 1..100, so the boundary walk refuses a longer one
/// before a handler sees it and no length is re-checked here.
pub fn label_is_well_formed(label: &str) -> bool {
    // The character set is checked first, so everything after it is ASCII and a prefix
    // comparison lands on a character boundary.
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return false;
    }
    let Some(first) = label.chars().next() else {
        return false;
    };
    if first.is_ascii_digit() {
        return false;
    }
    !RESERVED_LABEL_PREFIXES
        .iter()
        .any(|prefix| label.len() >= prefix.len() && label[..prefix.len()].eq_ignore_ascii_case(prefix))
}

/// How many parameters one Region holds: "You can create a maximum of 10,000 standard
/// parameters for each Region in an Amazon Web Services account" (`PutParameter`'s
/// `Tier` doc). The advanced tier's published quota is ten times this; this emulator
/// applies the standard quota to both and reports a create past it as
/// `ParameterLimitExceeded`.
pub const MAX_PARAMETERS_PER_REGION: usize = 10_000;

/// How many parameters this store holds in TOTAL, across every Region.
///
/// Emulator resource guard, NOT a published AWS quota, and the bound that actually
/// keeps this process finite: the Region a parameter is stored under comes from the
/// request's own credential scope ([`request_region`]), so the per-Region quota alone
/// would be one quota per caller-chosen partition of the key space and the store could
/// grow without bound. A create past this total is refused with the same modelled
/// `ParameterLimitExceeded` as one past the per-Region quota.
pub const MAX_PARAMETERS_TOTAL: usize = MAX_PARAMETERS_PER_REGION;

/// The policy types a parameter may carry: "Parameter Store supports the following
/// policy types: Expiration, ExpirationNotification, and NoChangeNotification" (the
/// `InvalidPolicyTypeException` doc).
pub const POLICY_TYPES: &[&str] = &[
    "Expiration",
    "ExpirationNotification",
    "NoChangeNotification",
];

/// How many policies one parameter may carry: "You specified more than the maximum
/// number of allowed policies for the parameter. The maximum is 10" (the
/// `PoliciesLimitExceededException` doc).
pub const MAX_POLICIES: usize = 10;

/// The status a stored parameter policy is reported in: "Pending (the policy hasn't
/// been enforced or applied yet)" (the `ParameterInlinePolicy$PolicyStatus` doc).
///
/// `PutParameter` validates a policy document and persists it with the version; nothing
/// here enforces an expiration or raises a notification, so a stored policy has not been
/// applied and is reported as pending rather than as work nothing did.
pub const POLICY_STATUS_PENDING: &str = "Pending";

/// The policies a stored `Policies` document carries, as the modelled
/// `ParameterPolicyList`: one entry per policy in the document, carrying "the JSON text
/// of the policy" and the type that text declares.
///
/// A version that carries no policy document carries no policies, which is an EMPTY list
/// rather than a missing member: every parameter has "a list of policies associated with
/// a parameter" (`ParameterMetadata$Policies`) whether or not anything was attached to
/// it, so a client enumerating a version's policies reads the list it has instead of
/// having to tell "none attached" from "not reported". `PutParameter` refuses anything
/// that is not a JSON array of policy objects before it is stored, so the arms that give
/// up on a document here are backstops.
pub fn inline_policies(policies: Option<&str>) -> Vec<ParameterInlinePolicy> {
    let Some(document) = policies.filter(|policies| !policies.trim().is_empty()) else {
        return Vec::new();
    };
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return Vec::new();
    };
    let Some(entries) = parsed.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .map(|entry| ParameterInlinePolicy {
            policy_text: Some(entry.to_string()),
            policy_type: entry
                .get("Type")
                .and_then(::serde_json::Value::as_str)
                .map(str::to_string),
            policy_status: Some(POLICY_STATUS_PENDING.to_string()),
        })
        .collect()
}

/// The `DataType` values Parameter Store supports: "The following data type values are
/// supported. text aws:ec2:image aws:ssm:integration" (`PutParameter`'s `DataType`
/// doc).
pub const PARAMETER_DATA_TYPES: &[&str] = &["text", "aws:ec2:image", "aws:ssm:integration"];

/// The `DataType` a parameter carries when the request names none: "The data type of
/// the parameter, such as text or aws:ec2:image. The default is text" (the
/// `Parameter$DataType` doc).
pub const DEFAULT_DATA_TYPE: &str = "text";

/// The key a `SecureString` is encrypted under when the request names no `KeyId`: "If
/// you don't specify a key ID, the system uses the default key associated with your
/// Amazon Web Services account" (`PutParameter`'s `KeyId` doc), which Parameter Store
/// reaches through this alias.
pub const DEFAULT_SECURE_STRING_KEY_ID: &str = "alias/aws/ssm";

/// The characters a parameter name may carry: "Parameter names can include only the
/// following symbols and letters: a-zA-Z0-9_.-" plus the `/` hierarchy separator
/// (`PutParameter`'s `Name` doc).
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/')
}

/// The prefixes a parameter name may not start with: "A parameter name can't be
/// prefixed with "aws" or "ssm" (case-insensitive)" (`PutParameter`'s `Name` doc).
pub const RESERVED_NAME_PREFIXES: &[&str] = &["aws", "ssm"];

/// The store key for a parameter name: the name without its leading `/`.
///
/// A hierarchical name must carry the leading separator and a top-level one may
/// ("For parameters in a hierarchy, you must include a leading forward slash
/// character (/) when you create or reference a parameter"), so dropping it is what
/// makes `MyParameter` and `/MyParameter` the one parameter they are, while
/// `/Dev/DBServer` keeps its whole path.
pub fn canonical_name(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

/// The name a request carries, with the spaces rule applied, or the reason it is
/// refused.
///
/// "Parameter names can't contain spaces. The service removes any spaces specified for
/// the beginning or end of a parameter name. If the specified name for a parameter
/// contains spaces between characters, the request fails with a `ValidationException`
/// error" (`PutParameter`'s `Name` doc; `GetParameter`'s own doc repeats it). So the
/// name is trimmed, and a trimmed name that still carries whitespace is refused with a
/// message naming the member and the constraint.
pub fn trimmed_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.chars().any(char::is_whitespace) {
        return Err(format!(
            "Name '{name}' contains spaces between characters: a parameter name can't contain spaces"
        ));
    }
    Ok(trimmed)
}

/// Why a name is not one `PutParameter` accepts. Each variant maps to the modelled
/// error that reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameRejection {
    /// The name is not shaped like a parameter name:
    /// `ParameterPatternMismatchException`.
    Pattern(String),
    /// The name is nested deeper than Parameter Store allows:
    /// `HierarchyLevelLimitExceededException`.
    Depth(String),
}

/// Why `name` is not a parameter name this service accepts, or `None` when it is.
///
/// Every rule here is one of the naming constraints `PutParameter`'s `Name` doc lists;
/// the modelled `PSParameterName` carries only a length, so none of them is refused by
/// the boundary walk.
pub fn name_rejection(name: &str) -> Option<NameRejection> {
    if name.starts_with(ARN_PREFIX) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' is an ARN: you can't enter the Amazon Resource Name (ARN) for a \
             parameter, only the parameter name itself"
        )));
    }
    let path = canonical_name(name);
    if path.is_empty() {
        return Some(NameRejection::Pattern(
            "Name carries no parameter name at all".to_string(),
        ));
    }
    if let Some(bad) = path.chars().find(|c| !is_name_char(*c)) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' carries '{bad}': parameter names can include only the symbols and \
             letters a-zA-Z0-9_.- and the / hierarchy separator"
        )));
    }
    if path.split('/').any(str::is_empty) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' carries an empty hierarchy level"
        )));
    }
    // The hierarchy rule: anything below the top level must carry the leading
    // separator, so `Dev/DBServer` is not a name Parameter Store accepts.
    if path.contains('/') && !name.starts_with('/') {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' is hierarchical and must include a leading forward slash character (/)"
        )));
    }
    if let Some(reserved) = RESERVED_NAME_PREFIXES.iter().find(|prefix| {
        // Case-insensitive, compared against the head of the path rather than a
        // lowercased copy of it, so matching allocates nothing. Every character of the
        // path has already been checked to be ASCII above, so the slice is on a
        // character boundary.
        path.len() >= prefix.len() && path[..prefix.len()].eq_ignore_ascii_case(prefix)
    }) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' is prefixed with '{reserved}': a parameter name can't be prefixed with \
             {} (case-insensitive)",
            RESERVED_NAME_PREFIXES.join(" or ")
        )));
    }
    let levels = path.split('/').count();
    if levels > MAX_HIERARCHY_LEVELS {
        return Some(NameRejection::Depth(format!(
            "Name '{name}' is {levels} levels deep: a parameter hierarchy is limited to a maximum \
             depth of {MAX_HIERARCHY_LEVELS} levels"
        )));
    }
    None
}

/// The content size limit a parameter of `tier` holds (see [`STANDARD_VALUE_LIMIT`]
/// and [`ADVANCED_VALUE_LIMIT`]).
pub fn value_limit(tier: &str) -> usize {
    if tier == TIER_ADVANCED {
        ADVANCED_VALUE_LIMIT
    } else {
        STANDARD_VALUE_LIMIT
    }
}

/// The tier a put serves the parameter in.
///
/// Straight from `PutParameter`'s `Tier` doc: a request that names a tier is served in
/// that tier; one that names none keeps the tier the parameter already has, or — for a
/// parameter being created — gets the default tier configuration, which is the standard
/// tier ("The default tier when you begin using Parameter Store is the
/// standard-parameter tier", and `UpdateServiceSetting` — the operation that changes
/// it — is still a scaffold, so this emulator never leaves that default). Under
/// `Intelligent-Tiering` the request is evaluated against the three options the same
/// doc says require an advanced parameter: content over 4 KB, a parameter policy, or
/// more than 10,000 parameters already in the Region.
///
/// `stored` is the tier the parameter is in today, so an update that only changes a
/// value stays in its own tier: the no-revert rule the caller then faces fires only
/// when the request ASKS for a lower tier.
pub fn resolve_tier(
    requested: Option<&str>,
    stored: Option<&str>,
    value_len: usize,
    has_policies: bool,
    parameters_in_region: usize,
) -> String {
    match requested.or(stored) {
        None => TIER_STANDARD.to_string(),
        Some(TIER_INTELLIGENT_TIERING) => {
            // An advanced parameter can never be reverted, so evaluating a request
            // against the options keeps it advanced whatever this request carries. The
            // parameter-count option is stated for completeness and cannot fire while
            // the quota guard stands: `PutParameter` refuses a create once a Region
            // holds `MAX_PARAMETERS_PER_REGION` parameters, so the store never gets
            // past it.
            let needs_advanced = value_len > STANDARD_VALUE_LIMIT
                || has_policies
                || stored == Some(TIER_ADVANCED)
                || parameters_in_region > MAX_PARAMETERS_PER_REGION;
            if needs_advanced {
                TIER_ADVANCED.to_string()
            } else {
                TIER_STANDARD.to_string()
            }
        }
        Some(tier) => tier.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The sentences Parameter Store words its parameter refusals with
// ---------------------------------------------------------------------------

/// The page a tier refusal points the caller at, as Parameter Store words those
/// refusals.
const ADVANCED_PARAMETERS_GUIDE: &str =
    "https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-advanced-parameters.html";

/// What a put against an existing name without `Overwrite` is refused with.
///
/// The sentence is wholly fixed and names no parameter: it tells the caller which
/// REQUEST MEMBER to set, which is the whole of what it has to say ("The parameter
/// already exists. You can't create duplicate parameters", `ParameterAlreadyExists`).
pub const PARAMETER_ALREADY_EXISTS_MESSAGE: &str = "The parameter already exists. To \
     overwrite this value, set the overwrite option in the request to true.";

/// What a value that does not satisfy the request's `AllowedPattern` is refused with.
///
/// The rejected pattern is the request's own, so the caller can see which constraint
/// its value failed; the value itself is never echoed, because a `SecureString`'s is
/// secret.
pub fn allowed_pattern_mismatch_message(pattern: &str) -> String {
    format!("Parameter value, cannot be validated against allowedPattern: {pattern}")
}

/// The one rendering of a parameter name: what the store keeps for a name a request
/// supplied, and therefore what every read and every refusal reports it by.
///
/// A top-level parameter's name carries no leading separator — that is the whole of its
/// name, which is why `MyParameter` and `/MyParameter` are the one parameter
/// ([`canonical_name`]) — while a hierarchical name is its whole path, separator and
/// all: "For parameters in a hierarchy, you must include a leading forward slash
/// character (/)" (`PutParameter`'s `Name` doc), so the separator is part of that name.
/// The parameter's ARN carries the same rendering after its `parameter/` prefix
/// ([`parameter_arn`]), so a client that round-trips a reported `Name` into another call
/// addresses the parameter it read.
pub fn reported_name(name: &str) -> &str {
    let path = canonical_name(name);
    if path.contains('/') {
        name
    } else {
        path
    }
}

/// The separator a `Name` carries between a parameter's name and the version or label a
/// read selects: "Specify selectors by using one of the following formats:
/// parameter_name:version parameter_name:label" (`Parameter$Selector`).
pub const SELECTOR_SEPARATOR: char = ':';

/// The selector a read echoes back, as `Parameter$Selector` renders it.
///
/// The member is "either the version number or the label used to retrieve the parameter
/// value" in the `parameter_name:version` / `parameter_name:label` forms its doc names,
/// so the separator is part of what is echoed: a client that rebuilds the reference it
/// read by appending the reported selector to the reported name gets a `Name` that
/// resolves to the very version it was served.
pub fn reported_selector(selector: &str) -> String {
    format!("{SELECTOR_SEPARATOR}{selector}")
}

/// What a read whose `name:version` selector names a version the parameter does not
/// carry is refused with.
///
/// `name` is quoted back as [`reported_name`] renders it, and the version is the one the
/// request asked for.
pub fn version_not_found_message(name: &str, version: i64) -> String {
    format!(
        "Systems Manager could not find version {version} of {}. Verify the version and \
         try again.",
        reported_name(name)
    )
}

/// What a read whose `name:label` selector names a label no version of the parameter
/// carries is refused with (the label form of [`version_not_found_message`]).
pub fn version_label_not_found_message(name: &str, label: &str) -> String {
    format!(
        "Systems Manager could not find a version of {} that has the specified label, \
         {label}. Verify the parameter label and try again.",
        reported_name(name)
    )
}

/// What a `name:selector` read that resolves to no version is refused with: the
/// version form for a selector that names a version number, the label form otherwise.
///
/// A selector is "either the version number or the label used to retrieve the parameter
/// value" (`Parameter$Selector`), which is the same split [`ParameterRecord::select`]
/// resolves by — so the sentence names whichever of the two the caller actually asked
/// for.
pub fn selector_not_found_message(name: &str, selector: &str) -> String {
    match selector.parse::<i64>() {
        Ok(version) => version_not_found_message(name, version),
        Err(_) => version_label_not_found_message(name, selector),
    }
}

/// What a mutation of a name the store holds no parameter for is refused with, where
/// the service words the refusal at all.
///
/// The name is the one the request carried: a caller that passed a hierarchy path reads
/// it back as it wrote it. (A READ of a missing parameter carries no message at all —
/// see `GetParameter` — so this is not every `ParameterNotFound`.)
pub fn parameter_not_found_message(name: &str) -> String {
    format!("Parameter {name} not found.")
}

/// What an overwrite that would have to displace a LABELLED oldest version is refused
/// with.
///
/// `oldest` is the version number the store holds as the oldest right now, so the
/// sentence names the version whose label the caller must move; it is recomputed on
/// every refusal rather than remembered. The modelled error carries this sentence in
/// its own documentation (`ParameterMaxVersionLimitExceeded`).
///
/// This refusal names the parameter by its hierarchy path with the leading separator
/// dropped, whichever depth the name has — not by [`reported_name`], which the version
/// refusals use: the two are worded by different parts of the service and each keeps
/// its own rendering.
pub fn max_version_limit_message(name: &str, oldest: i64) -> String {
    format!(
        "You attempted to create a new version of {} by calling the PutParameter API with \
         the overwrite flag. Version {oldest}, the oldest version, can\u{2019}t be deleted \
         because it has a label associated with it. Move the label to another version of \
         the parameter, and try again.",
        canonical_name(name)
    )
}

/// What a call that would put more than [`MAX_LABELS_PER_VERSION`] labels on one
/// version is refused with: fixed text carrying the quota.
pub fn label_limit_message() -> String {
    format!(
        "A parameter version can have maximum {MAX_LABELS_PER_VERSION} labels.Move one or \
         more labels to another version and try again."
    )
}

/// What a value larger than `tier`'s content size limit is refused with.
///
/// The standard tier's ceiling is one an upgrade lifts, so its refusal says so and
/// points at the tier guide; the advanced tier's is the service's own ceiling, with no
/// higher tier to name.
pub fn value_limit_message(tier: &str) -> String {
    let limit = value_limit(tier);
    if tier == TIER_ADVANCED {
        format!(
            "{TIER_ADVANCED} tier parameters support a maximum parameter value of {limit} \
             characters."
        )
    } else {
        format!(
            "{TIER_STANDARD} tier parameters support a maximum parameter value of {limit} \
             characters. To create a larger parameter value, upgrade the parameter to use the \
             advanced-parameter tier. For more information, see {ADVANCED_PARAMETERS_GUIDE}"
        )
    }
}

/// What a `Policies` document on a parameter that is not advanced is refused with:
/// "Standard parameters ... can't be configured to use parameter policies"
/// (`PutParameter`'s `Tier` doc), worded as the service words it.
pub fn policies_require_advanced_message() -> String {
    format!(
        "You can't assign parameter policies to a standard-parameter tier. You must upgrade \
         the parameter to use the advanced-parameter tier. For more information, see \
         {ADVANCED_PARAMETERS_GUIDE}"
    )
}

/// One stored version of one parameter: the value a put persisted, and everything the
/// service recorded alongside it.
#[derive(Debug, Clone)]
pub struct ParameterVersionRecord {
    /// This version's number, counting from 1 for a parameter's first version.
    pub version: i64,
    /// The value AT REST, which is what a read reports when it asks for no decryption.
    ///
    /// For a [`TYPE_SECURE_STRING`] version that is the base64 of the KMS ciphertext the
    /// put sealed the value into ([`secure_string::seal`]) — never the plaintext, which
    /// this store does not hold. For every other type the value is not encrypted at all,
    /// so it is the value as the put supplied it.
    pub value: String,
    /// A `ParameterType` value.
    pub type_: String,
    /// A `ParameterDataType` value (see [`PARAMETER_DATA_TYPES`]).
    pub data_type: String,
    /// A `ParameterTier` value, already resolved (see [`resolve_tier`]).
    pub tier: String,
    pub description: Option<String>,
    pub allowed_pattern: Option<String>,
    pub key_id: Option<String>,
    pub policies: Option<String>,
    pub last_modified_date: SystemTime,
    /// The ARN of the principal whose credential signed the write that created this
    /// version — "Amazon Resource Name (ARN) of the Amazon Web Services user who last
    /// changed the parameter" (`ParameterMetadata$LastModifiedUser`), as the write
    /// resolved it ([`identity_authorization::caller_arn`]), so a client reading its own
    /// identity recognises the writer of a parameter it wrote.
    ///
    /// `None` for a version no principal of this account wrote: a parameter AWS itself
    /// publishes ([`public_parameters`]) belongs to no account, so no account's user is
    /// reported as having changed it.
    pub last_modified_user: Option<String>,
    /// The labels that point at THIS version. `LabelParameterVersion` is what writes
    /// them ([`ParameterRecord::attach_label`]) and `UnlabelParameterVersion` what
    /// erases them, so the set holds exactly what was attached and never released — a
    /// `name:label` selector resolves against it rather than against anything invented,
    /// so a label nothing created resolves to nothing.
    pub labels: BTreeSet<String>,
}

impl ParameterVersionRecord {
    /// The modelled `Parameter` for this version of `record`.
    ///
    /// `selector` is the `name:selector` suffix the read carried, which is what the
    /// `Parameter$Selector` doc describes: "Either the version number or the label used
    /// to retrieve the parameter value".
    pub fn to_parameter(&self, record: &ParameterRecord, selector: Option<&str>) -> Parameter {
        Parameter {
            name: Some(record.name.clone()),
            r#type: Some(self.type_.clone()),
            value: Some(self.value.clone()),
            version: self.version,
            // The selector is echoed in the documented `parameter_name:version` /
            // `parameter_name:label` form, separator and all ([`reported_selector`]).
            selector: selector.map(reported_selector),
            // `SourceResult` is "the raw result or response from the source" for a
            // parameter that references another Amazon Web Services service. Nothing
            // in this store references one, so there is no source result to report.
            source_result: None,
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            arn: Some(record.arn()),
            data_type: Some(self.data_type.clone()),
        }
    }

    /// The modelled `ParameterMetadata` for this version of `record`: "metadata includes
    /// information like the Amazon Resource Name (ARN) of the last user to update the
    /// parameter and the date and time the parameter was last used" — everything a
    /// `ParameterHistory` entry carries except the value, which a metadata listing does
    /// not report.
    pub fn to_parameter_metadata(&self, record: &ParameterRecord) -> ParameterMetadata {
        ParameterMetadata {
            name: Some(record.name.clone()),
            arn: Some(record.arn()),
            r#type: Some(self.type_.clone()),
            // "Applies to SecureString parameters only", so only a version stored under
            // a key carries one.
            key_id: self.key_id.clone(),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            // As in [`Self::to_parameter_history`]: the principal the write that created
            // this version was signed for, recorded by that write and reported unchanged,
            // so two reads of one version report one writer.
            last_modified_user: self.last_modified_user.clone(),
            description: self.description.clone(),
            allowed_pattern: self.allowed_pattern.clone(),
            version: self.version,
            tier: Some(self.tier.clone()),
            // "A list of policies associated with a parameter": the list the version
            // holds, empty when no policy document was stored with it
            // ([`inline_policies`]).
            policies: Some(inline_policies(self.policies.as_deref())),
            data_type: Some(self.data_type.clone()),
        }
    }

    /// The modelled `ParameterHistory` for this version of `record`.
    ///
    /// Every member is read off the version it reports: the history of a parameter is
    /// the sequence of its retained versions, so an entry carries the value, type, key,
    /// description, pattern, tier, policies and labels THAT version was created with
    /// rather than the newest one's.
    pub fn to_parameter_history(&self, record: &ParameterRecord) -> ParameterHistory {
        ParameterHistory {
            name: Some(record.name.clone()),
            r#type: Some(self.type_.clone()),
            // "The alias of the Key Management Service (KMS) key used to encrypt the
            // parameter. Applies to SecureString parameters only", so only a version
            // stored under one carries it.
            key_id: self.key_id.clone(),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            // "Amazon Resource Name (ARN) of the Amazon Web Services user who last
            // changed the parameter": the principal the write that created THIS version
            // was signed for, so an entry names the writer of the version it reports
            // rather than the writer of the newest one.
            last_modified_user: self.last_modified_user.clone(),
            description: self.description.clone(),
            value: Some(self.value.clone()),
            allowed_pattern: self.allowed_pattern.clone(),
            version: self.version,
            // "Labels assigned to the parameter version": the labels pointing at THIS
            // version, which is an empty list for a version no label points at — a
            // version always has a label set, so a client enumerating it before a label
            // or policy edit never has to tell "none assigned" from "not reported".
            labels: Some(self.labels.iter().cloned().collect::<Vec<String>>()),
            tier: Some(self.tier.clone()),
            // As in [`Self::to_parameter_metadata`]: the policies the version holds,
            // empty when it was stored with no policy document.
            policies: Some(inline_policies(self.policies.as_deref())),
            data_type: Some(self.data_type.clone()),
        }
    }
}

impl ParameterVersionRecord {
    /// The values of `key` this version holds, for a `ParameterStringFilter` to compare
    /// against, or `None` when the key names no attribute of a parameter version.
    ///
    /// A version has exactly one type, at most one key id (only a `SecureString` names
    /// one) and any number of labels, so a key that names an attribute this version
    /// does not carry yields an EMPTY list — which matches no filter value, rather than
    /// refusing the filter.
    pub fn filter_values(&self, key: &str) -> Option<Vec<&str>> {
        match key {
            FILTER_KEY_TYPE => Some(vec![self.type_.as_str()]),
            FILTER_KEY_KEY_ID => Some(self.key_id.as_deref().into_iter().collect()),
            FILTER_KEY_LABEL => Some(self.labels.iter().map(String::as_str).collect()),
            _ => None,
        }
    }
}

/// One stored parameter: its name, where it lives, its tags, and every version of it
/// the store still retains.
#[derive(Debug, Clone)]
pub struct ParameterRecord {
    /// The name the create supplied, trimmed and rendered the one way the service
    /// renders a parameter name ([`reported_name`]), which is what every read reports
    /// and what this record's ARN carries.
    pub name: String,
    /// The Region the parameter was created in; part of its store key and of its ARN.
    pub region: String,
    pub tags: BTreeMap<String, String>,
    /// Every retained version, oldest first and the newest last. Never empty for a
    /// record that is in the store.
    pub versions: Vec<ParameterVersionRecord>,
}

impl ParameterRecord {
    /// The newest version, which is what a read with no selector answers with:
    /// "By default, if you don't specify a specific version, the system returns the
    /// latest parameter value when a parameter is called" (`PutParameter`'s `Version`
    /// doc).
    pub fn latest(&self) -> Option<&ParameterVersionRecord> {
        self.versions.last()
    }

    /// The oldest retained version, the one a new version displaces once the store
    /// holds [`MAX_VERSIONS_RETAINED`] of them.
    pub fn oldest(&self) -> Option<&ParameterVersionRecord> {
        self.versions.first()
    }

    /// The version a `name:selector` read resolves to, or `None` when this parameter
    /// has no such version or label.
    ///
    /// A selector is "either the version number or the label used to retrieve the
    /// parameter value" (`Parameter$Selector`), so an all-digit selector names a
    /// version and anything else names a label.
    pub fn select(&self, selector: Option<&str>) -> Option<&ParameterVersionRecord> {
        match selector {
            None => self.latest(),
            Some(selector) => match selector.parse::<i64>() {
                Ok(version) => self.versions.iter().find(|held| held.version == version),
                Err(_) => self
                    .versions
                    .iter()
                    .find(|held| held.labels.contains(selector)),
            },
        }
    }

    /// The number the next version of this parameter carries.
    pub fn next_version(&self) -> i64 {
        self.latest()
            .map_or(1, |latest| latest.version.saturating_add(1))
    }

    /// The version `version` numbers, to write its labels.
    pub fn version_mut(&mut self, version: i64) -> Option<&mut ParameterVersionRecord> {
        self.versions.iter_mut().find(|held| held.version == version)
    }

    /// Point `label` at the version `version` numbers, taking it off whatever other
    /// version of this parameter held it.
    ///
    /// That is the whole of the documented label rule: "You can't attach the same label
    /// to different versions of the same parameter ... You can move a label from one
    /// version of a parameter to another" (`LabelParameterVersion`), so the label is
    /// held by at most one version's set at a time and attaching it to a second version
    /// MOVES it rather than duplicating it.
    ///
    /// Answers `false` for a version this parameter does not carry, which is the
    /// not-found its caller reports.
    pub fn attach_label(&mut self, version: i64, label: &str) -> bool {
        if self.version_mut(version).is_none() {
            return false;
        }
        for held in &mut self.versions {
            if held.version == version {
                held.labels.insert(label.to_string());
            } else {
                held.labels.remove(label);
            }
        }
        true
    }

    /// Append `version`, dropping the oldest retained version once more than
    /// [`MAX_VERSIONS_RETAINED`] are held — "Parameter Store deletes the oldest
    /// version when a new one is created".
    pub fn push_version(&mut self, version: ParameterVersionRecord) {
        self.versions.push(version);
        while self.versions.len() > MAX_VERSIONS_RETAINED {
            self.versions.remove(0);
        }
    }

    /// This parameter's ARN. A hierarchical name keeps its whole path after the
    /// `parameter/` prefix: `arn:aws:ssm:<region>:<account>:parameter/Dev/DBServer`.
    pub fn arn(&self) -> String {
        parameter_arn(&self.region, &self.name)
    }
}

/// The ARN of the parameter `name` names in `region` — the one spelling of it, for a
/// caller that holds no record to ask ([`ParameterRecord::arn`] is the same ARN).
///
/// A hierarchical name keeps its whole path after the `parameter/` prefix. A name in AWS's
/// own published hierarchy carries no account at all (see below).
pub fn parameter_arn(region: &str, name: &str) -> String {
    let owned_by_this_account = ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_RESOURCE_PREFIX}{}", canonical_name(name)),
    );
    if !public_parameters::is_published_name(name) {
        return owned_by_this_account;
    }
    // A parameter in AWS's own hierarchy ([`public_parameters`]) belongs to no account —
    // AWS publishes it in every account and Region alike and no account can write it — so
    // its ARN names none: the account field is EMPTY where an account's own parameter
    // carries this endpoint's account. Derived from the canonical ARN above rather than
    // spelled out, so both forms name the same partition, service, Region and resource.
    owned_by_this_account.replacen(
        &format!(":{}:", ::engine::emulator_account_id()),
        "::",
        1,
    )
}

/// What a read's `Name` names: which parameter, in which Region and account, and the
/// selector that chose a version or a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterQuery<'a> {
    /// The canonical name (no leading `/`) to look the parameter up by.
    pub name: &'a str,
    /// The name AS THE REQUEST WROTE IT, selector stripped: a parameter is reached by
    /// its canonical name, but a refusal quotes back the name the caller asked for.
    pub path: &'a str,
    /// The `name:selector` suffix the request carried, without the colon, or `None`
    /// when it named none.
    pub selector: Option<&'a str>,
    /// The Region a full ARN named, or `None` when the request named a bare name.
    pub region: Option<&'a str>,
    /// The account a full ARN named.
    pub account: Option<&'a str>,
}

/// What `name` names, or `None` when it is an ARN that names no `ssm` parameter.
///
/// `GetParameter` takes "the name or Amazon Resource Name (ARN) of the parameter that
/// you want to query", and either form may carry a selector ("To query by parameter
/// label, use "Name": "name:label". To query by parameter version, use "Name":
/// "name:version""). An ARN is split on its six fields first, so the colons inside it
/// are never mistaken for a selector; a parameter name itself can carry no colon.
pub fn parse_parameter_query(name: &str) -> Option<ParameterQuery<'_>> {
    let (region, account, rest) = if name.starts_with(ARN_PREFIX) {
        let mut fields = name.splitn(6, ':');
        // arn : partition : service : region : account : resource
        let (_arn, _partition) = (fields.next()?, fields.next()?);
        let service = fields.next()?;
        let region = fields.next()?;
        let account = fields.next()?;
        let resource = fields.next()?;
        if service != ARN_SERVICE {
            return None;
        }
        (
            Some(region),
            Some(account),
            resource.strip_prefix(ARN_RESOURCE_PREFIX)?,
        )
    } else {
        (None, None, name)
    };
    let (path, selector) = match rest.rsplit_once(':') {
        Some((before, after)) if !before.is_empty() => (before, Some(after)),
        _ => (rest, None),
    };
    Some(ParameterQuery {
        name: canonical_name(path),
        path,
        selector,
        region,
        account,
    })
}

/// Whether the account and the Region an ARN NAMED are reachable through the endpoint the
/// request was sent to — the one rule behind every member of this service that takes "a
/// name or an Amazon Resource Name".
///
/// A resource is reached through the endpoint it is asked for, so an ARN naming another
/// account or another Region than the request was signed for names nothing reachable here:
/// `region` comes from the signed credential scope ([`request_region`]) and never from the
/// request body. A field an ARN leaves EMPTY names no account and no Region AT ALL — the
/// spelling AWS writes an account-less or partition-global resource in
/// ([`::service_common::arn`]), and the spelling the ARN of a resource AWS itself PUBLISHES
/// in every account and Region carries ([`parameter_arn`]) — so it constrains nothing and
/// the value resolves through the endpoint it was asked for rather than resolving to
/// nothing. `None` is a bare name, which names no ARN field either.
pub fn arn_names_this_endpoint(
    account: Option<&str>,
    region: Option<&str>,
    request_region: &str,
) -> bool {
    let reaches = |named: Option<&str>, here: &str| {
        named.is_none_or(|named| named.is_empty() || named == here)
    };
    reaches(account, ::engine::emulator_account_id()) && reaches(region, request_region)
}

impl ParameterQuery<'_> {
    /// Whether what this query named is reachable through `region`'s endpoint
    /// ([`arn_names_this_endpoint`]).
    pub fn names_this_endpoint(&self, region: &str) -> bool {
        arn_names_this_endpoint(self.account, self.region, region)
    }
}

// ---------------------------------------------------------------------------
// SSM documents
// ---------------------------------------------------------------------------

/// The resource portion a document ARN carries before the document's name, e.g.
/// `arn:aws:ssm:us-east-2:111122223333:document/MyDocument`.
pub const ARN_DOCUMENT_RESOURCE_PREFIX: &str = "document/";

/// `DocumentStatus` values, from the modelled enum
/// (`crate::ops::types::DOCUMENT_STATUS_VALUES`): a document Systems Manager is still
/// registering is `Creating`, and a registered one is `Active`.
pub const DOCUMENT_STATUS_CREATING: &str = "Creating";

/// See [`DOCUMENT_STATUS_CREATING`].
pub const DOCUMENT_STATUS_ACTIVE: &str = "Active";

/// The `DocumentStatus` a version Systems Manager is still registering FOR AN EXISTING
/// document carries, from the same modelled enum: "the document is being updated" is
/// what `Updating` says, so a version `UpdateDocument` has just minted is `Updating`
/// where a version `CreateDocument` minted is [`DOCUMENT_STATUS_CREATING`].
pub const DOCUMENT_STATUS_UPDATING: &str = "Updating";

/// How long a document version stays `Creating` or `Updating` before the clock owes its
/// move to `Active`.
///
/// AWS publishes no span for it, so this is the emulator's CHOSEN transient window: three
/// seconds, the span this emulator gives a transition nobody documents.
/// It is deliberately never zero: a caller that reads a document straight after
/// creating it observes `Creating`, the way it would against the real service, and the
/// move is materialised by [`ComAmazonawsSsmAmazonssmState::materialize_due`] when the
/// clock makes it due rather than by any call.
pub const DOCUMENT_TRANSITION_DELAY: Duration = Duration::from_secs(3);

/// The `PlatformType` values, in the order the modelled enum
/// (`smithy_query(query="shape",shape="PlatformType")`) lists them — which is the order a
/// document's `PlatformTypes` are reported in, so a caller reading `PlatformTypes[0]`
/// reads the platform the service names first. The generated
/// `crate::ops::types::PLATFORM_TYPE_VALUES` carries the same values sorted; the test in
/// [`document_platforms`] holds the two to the same SET, so a model revision cannot leave
/// this listing behind.
pub const DOCUMENT_PLATFORM_TYPES: &[&str] = &["Windows", "Linux", "MacOS"];

/// The version a newly created document carries: "The document version" counts from 1,
/// and each `UpdateDocument` mints the next number after the latest one
/// ([`DocumentRecord::next_version`]).
pub const FIRST_DOCUMENT_VERSION: u64 = 1;

/// How many versions of ONE document this store keeps.
///
/// The model states no number for it — the modelled `DocumentVersionLimitExceeded`
/// ("The document has too many versions. Delete one or more document versions and try
/// again.") is the error an update past the limit reports — so this is the emulator's
/// own guard, the way [`MAX_DOCUMENTS_TOTAL`] is: a document's version history grows by
/// one on every update and each version holds up to [`MAX_DOCUMENT_CONTENT`] of its own
/// content, so without a bound one document could grow this process without end.
pub const MAX_DOCUMENT_VERSIONS: usize = 100;

/// The `DocumentVersion` selectors that name a version by role rather than by number
/// (the modelled `DocumentVersion` pattern `^([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$)$`).
pub const DOCUMENT_VERSION_LATEST: &str = "$LATEST";

/// See [`DOCUMENT_VERSION_LATEST`].
pub const DOCUMENT_VERSION_DEFAULT: &str = "$DEFAULT";

/// "The content of the document must not exceed 64KB" (`CreateDocument`'s `Content`
/// doc), which `MaxDocumentSizeExceeded` ("The size limit of a document is 64 KB")
/// reports.
pub const MAX_DOCUMENT_CONTENT: usize = 64 * 1024;

/// "You can have at most 500 active SSM documents" (the `DocumentLimitExceeded` doc):
/// the published per-Region quota.
pub const MAX_DOCUMENTS: usize = 500;

/// How many documents this store holds in TOTAL, across every Region.
///
/// Emulator resource guard, NOT a published AWS quota, and the bound that actually
/// keeps this process finite: the Region a document is stored under comes from the
/// request's own credential scope ([`request_region`]), so the per-Region quota alone
/// would be one quota per caller-chosen partition of the key space and the store could
/// grow without bound. Set ABOVE the per-Region quota so that quota stays exact, and
/// low enough that the worst case a create can reach — this many documents each holding
/// [`MAX_DOCUMENT_CONTENT`] of content — is a size a test process can hold (about 64
/// MB). A create past it is refused with the same modelled `DocumentLimitExceeded` as
/// one past the per-Region quota.
pub const MAX_DOCUMENTS_TOTAL: usize = 2 * MAX_DOCUMENTS;

/// "JSON is the default format" (`CreateDocument`'s `DocumentFormat` doc).
pub const DEFAULT_DOCUMENT_FORMAT: &str = "JSON";

/// The `DocumentFormat` whose content is free text rather than a document tree — a
/// Change Calendar entry, say — so nothing is derived from it.
pub const DOCUMENT_FORMAT_TEXT: &str = "TEXT";

/// The other `DocumentFormat` a document tree is written in: "The document format can be
/// either JSON or YAML" (`DocumentFormat`, `crate::ops::types::DOCUMENT_FORMAT_VALUES`).
pub const DOCUMENT_FORMAT_YAML: &str = "YAML";

/// The document type a create that names none produces: `Command`, Systems Manager's
/// default document type (`DocumentType` is optional on `CreateDocument` and a
/// run-command document is what it produces).
pub const DEFAULT_DOCUMENT_TYPE: &str = "Command";

/// The document type whose versions go through the review workflow, and so the only
/// one that reports a `ReviewStatus` ("The current status of the review",
/// `DocumentDescription$ReviewStatus`).
pub const CHANGE_TEMPLATE_DOCUMENT_TYPE: &str = "Automation.ChangeTemplate";

/// The review status a document version carries before anybody reviews it, from the
/// modelled `ReviewStatus` enum (`crate::ops::types::REVIEW_STATUS_VALUES`).
pub const DOCUMENT_REVIEW_STATUS_NOT_REVIEWED: &str = "NOT_REVIEWED";

/// The hash type a document carries: "The Sha256 or Sha1 hash created by the system
/// when the document was created" with "Sha1 hashes have been deprecated"
/// (`DocumentDescription$Hash` / `$HashType`), so the system hash is Sha256 and the
/// deprecated `Sha1` member is left unset.
pub const DOCUMENT_HASH_TYPE: &str = "Sha256";

/// "You can't use the following strings as document name prefixes. These are reserved
/// by Amazon Web Services for use as document name prefixes: aws amazon amzn AWSEC2
/// AWSConfigRemediation AWSSupport" (`CreateDocument`'s `Name` doc). Compared
/// case-insensitively, the way Systems Manager reserves the `aws` family.
pub const RESERVED_DOCUMENT_NAME_PREFIXES: &[&str] = &[
    "aws",
    "amazon",
    "amzn",
    "AWSEC2",
    "AWSConfigRemediation",
    "AWSSupport",
];

/// How deep a document's content may nest, and how many flow openers (`[`, `{`) and
/// compact indicators (`-`, `?`) the untrusted bytes may carry.
///
/// The YAML parser descends recursively, so nesting has to be bounded BEFORE a tree
/// exists to measure (see [`guard_document_content`]). Every bound is far above what a
/// real SSM document needs.
const MAX_DOCUMENT_DEPTH: usize = 64;

/// See [`MAX_DOCUMENT_DEPTH`].
const MAX_DOCUMENT_FLOW_OPENERS: usize = 1024;

/// See [`MAX_DOCUMENT_DEPTH`].
const MAX_DOCUMENT_COMPACT_PER_LINE: usize = 256;

/// See [`MAX_DOCUMENT_DEPTH`].
const MAX_DOCUMENT_INDENT: usize = 256;

/// The schema versions a document of `document_type` may declare, or an EMPTY slice
/// for a type whose content carries no Systems Manager schema at all (an AppConfig
/// configuration, a CloudFormation template, a Change Calendar entry).
///
/// From the schema-version rules Systems Manager publishes per document type
/// ("Systems Manager document schemas and features" in the User Guide): Command
/// documents support 1.2, 2.0 and 2.2, Policy documents 1.0 and 2.0, Session documents
/// 1.0, Package documents 2.0, and Automation documents and change templates 0.3.
/// A content declaring anything else for one of those types is what
/// `InvalidDocumentSchemaVersion` ("The version of the document schema isn't
/// supported") reports.
pub fn supported_schema_versions(document_type: &str) -> &'static [&'static str] {
    match document_type {
        "Command" => &["1.2", "2.0", "2.2"],
        "Policy" => &["1.0", "2.0"],
        "Session" => &["1.0"],
        "Package" => &["2.0"],
        "Automation" | "Automation.ChangeTemplate" => &["0.3"],
        _ => &[],
    }
}

/// Why `name` is not a document name `CreateDocument` accepts, or `None` when it is.
///
/// The modelled `DocumentName` pattern (`^[a-zA-Z0-9_\-.]{3,128}$`) is already refused
/// by the boundary walk, so the only rule left is the reserved-prefix one the `Name`
/// doc states ([`RESERVED_DOCUMENT_NAME_PREFIXES`]).
pub fn document_name_rejection(name: &str) -> Option<String> {
    let reserved = RESERVED_DOCUMENT_NAME_PREFIXES.iter().find(|prefix| {
        name.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })?;
    Some(format!(
        "Name '{name}' is prefixed with '{reserved}': the prefixes {} are reserved by \
         Amazon Web Services for use as document name prefixes",
        RESERVED_DOCUMENT_NAME_PREFIXES.join(" ")
    ))
}

/// How this service states that the document a call named is not one it holds.
///
/// `InvalidDocument` is "the specified SSM document doesn't exist" (its own
/// documentation), and the sentence Systems Manager answers a document read with is fixed
/// text around the NAME the request asked for: `Document with name <name> does not
/// exist.`. One function, so every read that resolves a document by name reports a miss
/// in the same words — the name comes from the request and nothing else of it is
/// interpolated.
pub fn document_not_found_message(name: &str) -> String {
    format!("Document with name {name} does not exist.")
}

/// How this service states that the name a `CreateDocument` asked for is already held.
///
/// `DocumentAlreadyExists` is "the specified document already exists" (its own
/// documentation), and the sentence is fixed text around the name the create asked for:
/// `Document with same name <name> already exists`. A document name is unique within a
/// Region, but the sentence states no Region — what the caller has to change is the name.
pub fn document_already_exists_message(name: &str) -> String {
    format!("Document with same name {name} already exists")
}

/// What a `DocumentARN` value names: which document, and — when the value is a full
/// ARN — in which Region and account.
pub type DocumentQuery<'a> = ResourceQuery<'a>;

/// What a value that may be a bare resource name or a full ARN names: the resource
/// portion after its type prefix, and — when the value is an ARN — the Region and account
/// that ARN names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceQuery<'a> {
    /// The resource the value names, to look it up by: a bare name as given, or the part
    /// of an ARN's resource after the resource-type prefix.
    pub name: &'a str,
    /// The Region a full ARN named, or `None` when the request named a bare name.
    pub region: Option<&'a str>,
    /// The account a full ARN named.
    pub account: Option<&'a str>,
}

/// What `value` names under the resource-type `prefix`, or `None` when it is an ARN that
/// names no `ssm` resource of that type.
///
/// The ONE parse behind every member of this service that takes "a name or an Amazon
/// Resource Name": the bare spelling is the name itself, and the ARN spelling is read
/// through the one parse every service reads a caller-supplied ARN with and then stripped
/// of its resource-type prefix, so the two spellings resolve to the same resource.
pub fn parse_resource_query<'a>(value: &'a str, prefix: &str) -> Option<ResourceQuery<'a>> {
    if !value.starts_with(ARN_PREFIX) {
        return Some(ResourceQuery {
            name: value,
            region: None,
            account: None,
        });
    }
    let arn = ::service_common::arn::parse_of(value, ARN_SERVICE)?;
    Some(ResourceQuery {
        name: arn.resource.strip_prefix(prefix)?,
        region: Some(arn.region),
        account: Some(arn.account),
    })
}

impl ResourceQuery<'_> {
    /// Whether what this query named is reachable through `region`'s endpoint
    /// ([`arn_names_this_endpoint`]).
    pub fn names_this_endpoint(&self, region: &str) -> bool {
        arn_names_this_endpoint(self.account, self.region, region)
    }
}

/// What `value` names, or `None` when it is an ARN that names no `ssm` document.
///
/// A `DocumentARN` is "the name of the required SSM document. The name can be an
/// Amazon Resource Name (ARN)" (`DocumentRequires$Name`), so either spelling resolves
/// to the same document, through [`parse_resource_query`].
pub fn parse_document_query(value: &str) -> Option<DocumentQuery<'_>> {
    parse_resource_query(value, ARN_DOCUMENT_RESOURCE_PREFIX)
}

/// Why a document's content is not one this service accepts. Each variant maps to the
/// modelled error that reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentRejection {
    /// The content isn't a document in the declared format: `InvalidDocumentContent`.
    Content(String),
    /// The content declares a schema version the document type doesn't support:
    /// `InvalidDocumentSchemaVersion`.
    SchemaVersion(String),
}

/// What a document's content declares: everything a read reports that is derived from
/// the content rather than echoed from the create's own members.
#[derive(Debug, Clone, Default)]
pub struct ParsedDocumentContent {
    /// The content's `schemaVersion`, which is what `SchemaVersion` reports.
    pub schema_version: Option<String>,
    /// The content's `description`.
    pub description: Option<String>,
    /// One entry per `parameters` entry, in the content's own order.
    pub parameters: Vec<DocumentParameter>,
    /// The platforms the content's steps leave the document compatible with, or every
    /// modelled platform when they restrict it to none
    /// ([`document_platforms::declared_platform_types`]).
    pub platform_types: Vec<String>,
    /// The names of the steps the content declares, in the content's own order: the
    /// steps a command's invocation runs, which `GetCommandInvocation`'s `PluginName`
    /// names one of ([`declared_step_names`]).
    pub step_names: Vec<String>,
}

/// The documented size bound, applied to content of EVERY format: "The content of the
/// document must not exceed 64KB" ([`MAX_DOCUMENT_CONTENT`]).
fn content_within_size(content: &str) -> Result<(), ContentRejection> {
    if content.len() > MAX_DOCUMENT_CONTENT {
        return Err(ContentRejection::Content(format!(
            "Content is {} bytes; a document's content must not exceed {MAX_DOCUMENT_CONTENT} bytes",
            content.len()
        )));
    }
    Ok(())
}

/// Pre-flight bounds on untrusted content, applied BEFORE any parser builds a tree
/// from it: the YAML parser descends recursively, so the nesting the bytes can ask for
/// has to be bounded while there is still no tree to measure.
///
/// This is a GUARD, not a parser: it knows nothing about YAML context and measures only
/// quantities a hiding trick can push UP but never DOWN — the content's length, the
/// number of flow openers (whose count IS their depth bound, since flow nesting costs
/// neither a line nor a column), the compact indicators on any one line, and a line's
/// leading indentation. Closers are never subtracted, so a `]` inside a scalar cannot
/// buy depth back. The same guard the Greengrass recipe parser
/// (`service-com-amazonaws-greengrassv2-greengrassv2/src/state.rs`) and the
/// CloudFormation template parser apply, for the same reason and with the same bounds;
/// the three copies are to be migrated onto one shared implementation rather than
/// copied a fourth time.
fn guard_document_content(content: &str) -> Result<(), ContentRejection> {
    content_within_size(content)?;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    for byte in content.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_DOCUMENT_INDENT {
                    return Err(ContentRejection::Content(format!(
                        "a line of Content must not be indented past {MAX_DOCUMENT_INDENT} columns"
                    )));
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_openers += 1;
                if flow_openers > MAX_DOCUMENT_FLOW_OPENERS {
                    return Err(ContentRejection::Content(format!(
                        "Content must not carry more than {MAX_DOCUMENT_FLOW_OPENERS} '[' or '{{' \
                         openers"
                    )));
                }
            }
            b'-' | b'?' => {
                counting_indent = false;
                compact_on_line += 1;
                if compact_on_line > MAX_DOCUMENT_COMPACT_PER_LINE {
                    return Err(ContentRejection::Content(format!(
                        "a line of Content must not carry more than \
                         {MAX_DOCUMENT_COMPACT_PER_LINE} '-' or '?' indicators"
                    )));
                }
            }
            _ => counting_indent = false,
        }
    }
    Ok(())
}

/// Refuses content that uses a YAML alias, BEFORE the loader can expand one.
///
/// `yaml-rust2`'s loader RESOLVES an alias by copying the anchored node, so by the
/// time a tree exists the expansion has already happened — a short alias chain can
/// expand to gigabytes. The parser's own event stream reports an alias as
/// `Event::Alias` and copies nothing, so the content is parsed once for this check and
/// only alias-free content is handed to the loader. An anchor no alias refers to
/// expands nothing and is accepted.
fn reject_document_aliases(content: &str) -> Result<(), ContentRejection> {
    #[derive(Default)]
    struct AliasDetector {
        saw_alias: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.saw_alias = true;
            }
        }
    }

    let mut detector = AliasDetector::default();
    ::yaml_rust2::parser::Parser::new_from_str(content)
        .load(&mut detector, true)
        .map_err(|why| {
            ContentRejection::Content(format!("Content could not be parsed as a document: {why}"))
        })?;
    if detector.saw_alias {
        return Err(ContentRejection::Content(
            "Content must not use YAML aliases".to_string(),
        ))
    }
    Ok(())
}

/// Look a key up in a document mapping.
fn mapping_get<'a>(
    map: &'a ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'a ::yaml_rust2::Yaml> {
    map.iter()
        .find(|(held, _)| held.as_str() == Some(key))
        .map(|(_, value)| value)
}

/// A document scalar as the string the API reports it as.
fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => Some(text.clone()),
        ::yaml_rust2::Yaml::Boolean(flag) => Some(flag.to_string()),
        ::yaml_rust2::Yaml::Integer(number) => Some(number.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Reading a hierarchy of parameters
// ---------------------------------------------------------------------------

/// A `ParameterStringFilter` key that filters on a parameter's `ParameterType`.
pub const FILTER_KEY_TYPE: &str = "Type";

/// A `ParameterStringFilter` key that filters on the KMS key a `SecureString` names.
pub const FILTER_KEY_KEY_ID: &str = "KeyId";

/// A `ParameterStringFilter` key that filters on a version's labels.
pub const FILTER_KEY_LABEL: &str = "Label";

/// The `Key` values a `ParameterStringFilter` may carry on a hierarchy read.
///
/// The modelled key pattern is `^(tag:.+|Name|Type|KeyId|Path|Label|Tier|DataType)$`,
/// and "For `GetParametersByPath`, the following patterns listed for `Key` aren't valid:
/// `tag`, `DataType`, `Name`, `Path`, and `Tier`" (the `ParameterStringFilter$Key`
/// doc) — which leaves these three.
pub const PARAMETERS_BY_PATH_FILTER_KEYS: &[&str] =
    &[FILTER_KEY_TYPE, FILTER_KEY_KEY_ID, FILTER_KEY_LABEL];

/// Compare a filter's value against the whole held value.
pub const FILTER_OPTION_EQUALS: &str = "Equals";

/// Compare a filter's value against the start of the held value.
pub const FILTER_OPTION_BEGINS_WITH: &str = "BeginsWith";

/// The comparison options a hierarchy read's filters accept: "For filters used with
/// `GetParametersByPath`, valid options include `Equals` and `BeginsWith`" (the
/// `ParameterStringFilter$Option` doc).
pub const PARAMETERS_BY_PATH_FILTER_OPTIONS: &[&str] =
    &[FILTER_OPTION_EQUALS, FILTER_OPTION_BEGINS_WITH];

/// The options a `Label` filter accepts: "(Exception: For filters using `Label` as the
/// Key name, the only valid option is `Equals`.)" (the same doc).
pub const LABEL_FILTER_OPTIONS: &[&str] = &[FILTER_OPTION_EQUALS];

/// Whether `key` is a filter key a hierarchy read accepts (see
/// [`PARAMETERS_BY_PATH_FILTER_KEYS`]).
pub fn is_parameters_by_path_filter_key(key: &str) -> bool {
    PARAMETERS_BY_PATH_FILTER_KEYS.contains(&key)
}

/// The comparison options a hierarchy read accepts for `key`.
pub fn parameters_by_path_filter_options(key: &str) -> &'static [&'static str] {
    if key == FILTER_KEY_LABEL {
        LABEL_FILTER_OPTIONS
    } else {
        PARAMETERS_BY_PATH_FILTER_OPTIONS
    }
}

/// Whether `held` carries a value matching one of `wanted` under `option`.
///
/// `held` is every value the stored record carries for the filter's key (a parameter
/// has one type, at most one key id, one tier, one data type, at most one value for a
/// given tag, and any number of labels), and a filter matches when any held value
/// matches any wanted one. The comparison is the one `option` names, and a filter
/// naming none compares whole values.
pub fn string_filter_matches(held: &[&str], option: &str, wanted: &[String]) -> bool {
    held.iter().any(|value| {
        wanted.iter().any(|want| match option {
            FILTER_OPTION_BEGINS_WITH => value.starts_with(want.as_str()),
            FILTER_OPTION_CONTAINS => value.contains(want.as_str()),
            _ => *value == want.as_str(),
        })
    })
}

// ---------------------------------------------------------------------------
// The sentences a filter refusal is worded with
// ---------------------------------------------------------------------------

/// What a `ParameterFilters` entry naming a key the operation does not accept is
/// refused with.
///
/// `valid` is the accepting operation's OWN key vocabulary, so the caller is told what
/// it may ask for here rather than what Parameter Store accepts somewhere else — the
/// two operations that take these filters accept different keys
/// ([`DESCRIBE_PARAMETERS_FILTER_KEYS`], [`PARAMETERS_BY_PATH_FILTER_KEYS`]).
pub fn invalid_filter_key_message(key: &str, valid: &[&str]) -> String {
    format!(
        "The following filter key is not valid: {key}. Valid filter keys include: [{}].",
        valid.join(", ")
    )
}

/// What a `ParameterFilters` entry naming an option its key does not accept is refused
/// with, where `valid` is the option vocabulary of THAT key.
pub fn invalid_filter_option_message(option: &str, valid: &[&str]) -> String {
    format!(
        "The following filter option is not valid: {option}. Valid options include: [{}].",
        valid.join(", ")
    )
}

// ---------------------------------------------------------------------------
// The filters a parameter describe accepts
// ---------------------------------------------------------------------------

/// A `ParameterStringFilter` key that filters on a parameter's name.
pub const FILTER_KEY_NAME: &str = "Name";

/// A `ParameterStringFilter` key that filters on the hierarchy a parameter lies in.
pub const FILTER_KEY_PATH: &str = "Path";

/// A `ParameterStringFilter` key that filters on a version's `ParameterTier`.
pub const FILTER_KEY_TIER: &str = "Tier";

/// A `ParameterStringFilter` key that filters on a version's `ParameterDataType`.
pub const FILTER_KEY_DATA_TYPE: &str = "DataType";

/// Compare a filter's value against any part of the held value.
pub const FILTER_OPTION_CONTAINS: &str = "Contains";

/// A `Path` filter option that reaches every level under the path.
pub const FILTER_OPTION_RECURSIVE: &str = "Recursive";

/// A `Path` filter option that reaches only the level immediately below the path.
pub const FILTER_OPTION_ONE_LEVEL: &str = "OneLevel";

/// The non-tag `Key` values a `ParameterStringFilter` may carry on a parameter
/// describe.
///
/// The modelled key pattern is `^(tag:.+|Name|Type|KeyId|Path|Label|Tier|DataType)$`,
/// and "for `DescribeParameters`, all of the listed patterns are valid except `Label`"
/// (the `ParameterStringFilter$Key` doc) — which leaves these five plus the tag form
/// ([`TAG_FILTER_KEY_PREFIX`]). They are held in the order the service LISTS them in
/// when it refuses a key outside the set ([`invalid_filter_key_message`]).
pub const DESCRIBE_PARAMETERS_FILTER_KEYS: &[&str] = &[
    FILTER_KEY_PATH,
    FILTER_KEY_NAME,
    FILTER_KEY_TYPE,
    FILTER_KEY_KEY_ID,
    FILTER_KEY_TIER,
    FILTER_KEY_DATA_TYPE,
];

/// The `Key` values the deprecated `ParametersFilter` carries, from the modelled
/// `ParametersFilterKey` enum (`Name`, `Type`, `KeyId`). "This data type is deprecated.
/// Instead, use `ParameterStringFilter`", so each one filters on the attribute the
/// string filter of the same key does.
pub const LEGACY_PARAMETERS_FILTER_KEYS: &[&str] =
    &[FILTER_KEY_NAME, FILTER_KEY_TYPE, FILTER_KEY_KEY_ID];

/// The comparison options a parameter describe's filters accept: "For all filters used
/// with `DescribeParameters`, valid options include `Equals` and `BeginsWith`" (the
/// `ParameterStringFilter$Option` doc).
pub const DESCRIBE_PARAMETERS_FILTER_OPTIONS: &[&str] =
    &[FILTER_OPTION_EQUALS, FILTER_OPTION_BEGINS_WITH];

/// The options a `Name` filter accepts: "The `Name` filter additionally supports the
/// `Contains` option" (the same doc).
pub const NAME_FILTER_OPTIONS: &[&str] = &[
    FILTER_OPTION_EQUALS,
    FILTER_OPTION_BEGINS_WITH,
    FILTER_OPTION_CONTAINS,
];

/// The options a `Path` filter accepts: "(Exception: For filters using the key `Path`,
/// valid options include `Recursive` and `OneLevel`.)" (the same doc).
pub const PATH_FILTER_OPTIONS: &[&str] = &[FILTER_OPTION_RECURSIVE, FILTER_OPTION_ONE_LEVEL];

/// How many filters one parameter describe carries, across both the deprecated
/// `Filters` member and `ParameterFilters`.
///
/// Every filter is compared against every parameter of the Region, so the work one
/// request asks for is the number of filters times the number of parameters held times
/// the values per filter. The last two are already bounded — by
/// [`MAX_PARAMETERS_PER_REGION`] and by the modelled value-list length (1..50) — and
/// neither filter list carries a modelled length, so the count is bounded here. Ten
/// holds a describe's whole published key vocabulary (`Name`, `Type`, `KeyId`, `Path`,
/// `Tier`, `DataType`) with room for tag keys beside it, and is the order this service's
/// own filter lists are modelled at (a session describe's is 1..6, a patch describe's
/// 0..5).
pub const MAX_DESCRIBE_PARAMETERS_FILTERS: usize = 10;

/// Whether `key` is a filter key a parameter describe accepts (see
/// [`DESCRIBE_PARAMETERS_FILTER_KEYS`]).
pub fn is_describe_parameters_filter_key(key: &str) -> bool {
    DESCRIBE_PARAMETERS_FILTER_KEYS.contains(&key)
        || key
            .strip_prefix(TAG_FILTER_KEY_PREFIX)
            .is_some_and(|tag| !tag.is_empty())
}

/// The comparison options a parameter describe accepts for `key`.
pub fn describe_parameters_filter_options(key: &str) -> &'static [&'static str] {
    match key {
        FILTER_KEY_NAME => NAME_FILTER_OPTIONS,
        FILTER_KEY_PATH => PATH_FILTER_OPTIONS,
        _ => DESCRIBE_PARAMETERS_FILTER_OPTIONS,
    }
}

/// The comparison a filter on `key` that names no option performs.
///
/// A filter naming no option compares whole values, as every other string filter in
/// this service does — except a `Path` filter, whose options are the two reaches of a
/// hierarchy and whose narrower one is the reach a hierarchy read defaults to
/// (`GetParametersByPath`'s `Recursive` is "default false").
pub fn describe_parameters_default_filter_option(key: &str) -> &'static str {
    if key == FILTER_KEY_PATH {
        FILTER_OPTION_ONE_LEVEL
    } else {
        FILTER_OPTION_EQUALS
    }
}

/// The values an equality filter on `key` can be satisfied by, when the attribute it
/// names is a modelled enum, or `None` when it is not.
///
/// A parameter's type and tier are modelled enums (`ParameterType`, `ParameterTier`),
/// so an equality filter on a value outside one asks for an attribute value no
/// parameter can hold. A prefix comparison is not checked against the enum: a prefix of
/// a value is not a value.
pub fn describe_filter_enum_values(key: &str) -> Option<&'static [&'static str]> {
    match key {
        FILTER_KEY_TYPE => Some(PARAMETER_TYPE_VALUES),
        FILTER_KEY_TIER => Some(PARAMETER_TIER_VALUES),
        _ => None,
    }
}

/// The values of `key` the stored parameter `record` carries at `version`, for a
/// `ParameterStringFilter` on a parameter describe to compare against.
///
/// The version-held attributes come from [`ParameterVersionRecord::filter_values`], the
/// accessor a hierarchy read already compares against, so the two reads cannot drift;
/// the keys only a describe publishes (`Tier`, `DataType`, `Name` and a tag) are added
/// here. A key naming an attribute this parameter does not carry (a tag nobody set, a
/// key id on a parameter that is not a `SecureString`) yields an EMPTY list, which
/// matches no filter value rather than refusing the filter.
pub fn describe_filter_values<'a>(
    record: &'a ParameterRecord,
    version: &'a ParameterVersionRecord,
    key: &str,
) -> Vec<&'a str> {
    // The keys a hierarchy read and a describe share name attributes of the version, so
    // they are read through the one accessor both reads use rather than a second copy.
    if let Some(held) = version.filter_values(key) {
        return held;
    }
    match key {
        FILTER_KEY_TIER => vec![version.tier.as_str()],
        FILTER_KEY_DATA_TYPE => vec![version.data_type.as_str()],
        FILTER_KEY_NAME => vec![record.name.as_str()],
        _ => key
            .strip_prefix(TAG_FILTER_KEY_PREFIX)
            .and_then(|tag| record.tags.get(tag))
            .map(String::as_str)
            .into_iter()
            .collect(),
    }
}

/// Whether `name` is a name one of `values` asks for under `option`.
///
/// The leading separator is not part of a name's identity ([`canonical_name`]), so a
/// filter value spelled with one asks for the same parameter as one spelled without —
/// except under `Contains`, where the separator is one of the characters being searched
/// for, so the name is searched in its hierarchical spelling.
pub fn name_filter_matches(name: &str, option: &str, values: &[String]) -> bool {
    if option == FILTER_OPTION_CONTAINS {
        let hierarchical = format!("/{}", canonical_name(name));
        return values.iter().any(|want| hierarchical.contains(want));
    }
    let canonical: Vec<String> = values
        .iter()
        .map(|want| canonical_name(want).to_string())
        .collect();
    string_filter_matches(&[canonical_name(name)], option, &canonical)
}

/// Whether the parameter `record` at `version` carries what one describe filter asks
/// for: the key names an attribute of the parameter, and `option` is how its held value
/// is compared against `values`.
///
/// `Path` is the one key that does not compare a held value: it names a hierarchy, and a
/// parameter is in it when its name lies under the path ([`is_under_path`]) — the whole
/// hierarchy under `Recursive`, the level immediately below it under `OneLevel`.
pub fn describe_parameter_matches(
    record: &ParameterRecord,
    version: &ParameterVersionRecord,
    key: &str,
    option: &str,
    values: &[String],
) -> bool {
    match key {
        FILTER_KEY_NAME => name_filter_matches(&record.name, option, values),
        FILTER_KEY_PATH => values.iter().any(|want| {
            parameter_path(want).is_ok_and(|path| {
                is_under_path(
                    canonical_name(&record.name),
                    path,
                    option == FILTER_OPTION_RECURSIVE,
                )
            })
        }),
        _ => string_filter_matches(&describe_filter_values(record, version, key), option, values),
    }
}

/// The bounds the modelled `GetParametersByPathMaxResults` carries (range 1..10).
pub const MIN_PARAMETERS_BY_PATH_PAGE_SIZE: i32 = 1;

/// See [`MIN_PARAMETERS_BY_PATH_PAGE_SIZE`]. It is also the page size a request that
/// names no `MaxResults` gets: "If you specify `MaxResults` in the request, the response
/// includes information up to the limit specified" (`GetParametersByPath`'s doc), so a
/// request naming none is served up to the modelled maximum.
pub const MAX_PARAMETERS_BY_PATH_PAGE_SIZE: i32 = 10;

/// The page size a read bounded by `MaxResults` serves, or the refusal message for one
/// outside the modelled range — which the protocol's boundary validator already refuses,
/// so this is a backstop rather than the first line of defence.
///
/// A request naming no `MaxResults` is served up to `maximum`: "the maximum number of
/// items to return for this call" bounds a page the caller asked to be bounded, so a
/// caller asking for no bound is served the whole page the operation allows. The
/// bounds differ per operation, so each read passes its own modelled range.
fn page_size(max_results: Option<i32>, minimum: i32, maximum: i32) -> Result<usize, String> {
    let requested = max_results.unwrap_or(maximum);
    if !(minimum..=maximum).contains(&requested) {
        return Err(format!(
            "Value '{requested}' at 'maxResults' failed to satisfy constraint: Member must be \
             between {minimum} and {maximum}"
        ));
    }
    Ok(requested as usize)
}

/// The page size a hierarchy read serves (see [`page_size`]).
pub fn parameters_by_path_page_size(max_results: Option<i32>) -> Result<usize, String> {
    page_size(
        max_results,
        MIN_PARAMETERS_BY_PATH_PAGE_SIZE,
        MAX_PARAMETERS_BY_PATH_PAGE_SIZE,
    )
}

/// The lower bound the modelled `MaxResults` of a parameter-history read carries
/// (`GetParameterHistoryRequest$MaxResults`, range 1..50).
pub const MIN_PARAMETER_HISTORY_PAGE_SIZE: i32 = 1;

/// The upper bound of the same member. It is also the page size a request that names no
/// `MaxResults` gets: "The maximum number of items to return for this call" bounds a page
/// the caller asked to be bounded, so a caller asking for no bound is served up to the
/// modelled maximum.
pub const MAX_PARAMETER_HISTORY_PAGE_SIZE: i32 = 50;

/// The page size a parameter-history read serves (see [`page_size`]).
pub fn parameter_history_page_size(max_results: Option<i32>) -> Result<usize, String> {
    page_size(
        max_results,
        MIN_PARAMETER_HISTORY_PAGE_SIZE,
        MAX_PARAMETER_HISTORY_PAGE_SIZE,
    )
}

/// The lower bound the modelled `MaxResults` of a parameter describe carries
/// (`DescribeParametersRequest$MaxResults`, range 1..50).
pub const MIN_DESCRIBE_PARAMETERS_PAGE_SIZE: i32 = 1;

/// The upper bound of the same member, and the page size a request naming no
/// `MaxResults` is served up to (see [`page_size`]).
pub const MAX_DESCRIBE_PARAMETERS_PAGE_SIZE: i32 = 50;

/// The page size a parameter describe serves (see [`page_size`]).
pub fn describe_parameters_page_size(max_results: Option<i32>) -> Result<usize, String> {
    page_size(
        max_results,
        MIN_DESCRIBE_PARAMETERS_PAGE_SIZE,
        MAX_DESCRIBE_PARAMETERS_PAGE_SIZE,
    )
}

/// The hierarchy a read's `Path` names, canonical (no leading and no trailing `/`), or
/// the reason the request is refused.
///
/// "The hierarchy for the parameter. Hierarchies start with a forward slash (/)"
/// (`GetParametersByPath`'s `Path` doc), so a path that carries no leading separator
/// names no hierarchy and is refused. The spaces rule is the name rule
/// ([`trimmed_name`]): the path is a parameter-name prefix.
pub fn parameter_path(path: &str) -> Result<&str, String> {
    let trimmed = trimmed_name(path)?;
    if !trimmed.starts_with('/') {
        return Err(format!(
            "Path '{path}' doesn't start with a forward slash (/): a parameter hierarchy starts \
             with a forward slash (/)"
        ));
    }
    Ok(canonical_name(trimmed).trim_end_matches('/'))
}

/// Whether the parameter stored under canonical `name` lies in the hierarchy under
/// canonical `path` (both as [`canonical_name`] and [`parameter_path`] render them).
///
/// A parameter is under a path when the path is a whole prefix of its name's levels —
/// "the hierarchy is the parameter name except the last part of the parameter" — so the
/// parameter AT the path is not under it and a path is never half a level (`/Dev` does
/// not hold `/DevOps/x`). A non-recursive read reports only the level immediately below
/// the path; a recursive one reports every level under it ("Retrieve all parameters
/// within a hierarchy", the `Recursive` doc).
pub fn is_under_path(name: &str, path: &str, recursive: bool) -> bool {
    let rest = if path.is_empty() {
        name
    } else {
        match name.strip_prefix(path).and_then(|rest| rest.strip_prefix('/')) {
            Some(rest) => rest,
            None => return false,
        }
    };
    !rest.is_empty() && (recursive || !rest.contains('/'))
}

// ---------------------------------------------------------------------------
// The string filters `DescribeInstanceInformation` accepts
// ---------------------------------------------------------------------------
//
// The same managed nodes as the property describe reaches, through the filter vocabulary
// `DescribeInstanceInformation`'s own `Filters` member publishes.

/// The filter keys `DescribeInstanceInformation`'s `Filters` accepts beside the two tag
/// forms: "Valid filter key values: ActivationIds | AgentVersion | AssociationStatus |
/// IamRole | InstanceIds | PingStatus | PlatformTypes | ResourceType | SourceIds |
/// SourceTypes | "tag-key" | "tag:{keyname}"" (the `InstanceInformationStringFilter$Key`
/// doc). It is the property describe's set ([`is_node_filter_key`]) without
/// `DocumentName`, which that operation's filter publishes and this one's does not.
pub const INSTANCE_INFORMATION_FILTER_KEYS: &[&str] = &[
    "ActivationIds",
    "AgentVersion",
    "AssociationStatus",
    "IamRole",
    "InstanceIds",
    "PingStatus",
    "PlatformTypes",
    "ResourceType",
    "SourceIds",
    "SourceTypes",
];

/// Whether `key` names a tag rather than a property of a node ([`TAG_KEY_FILTER_KEY`] or
/// a [`TAG_FILTER_KEY_PREFIX`] key).
pub fn is_tag_filter_key(key: &str) -> bool {
    key == TAG_KEY_FILTER_KEY || key.starts_with(TAG_FILTER_KEY_PREFIX)
}

/// Whether `key` is a filter key `DescribeInstanceInformation`'s `Filters` documents.
///
/// `tag:` must name a tag key after the colon, so `tag:` alone is not one. The
/// comparison is exact, as it is for the property describe: a filter key is a name, so
/// neither a different spelling nor a different case names a property of a node.
pub fn is_instance_information_filter_key(key: &str) -> bool {
    if let Some(tag_key) = key.strip_prefix(TAG_FILTER_KEY_PREFIX) {
        return !tag_key.is_empty();
    }
    key == TAG_KEY_FILTER_KEY || INSTANCE_INFORMATION_FILTER_KEYS.contains(&key)
}

/// Whether `key` is a key the legacy `InstanceInformationFilterList` accepts — the
/// modelled `InstanceInformationFilterKey` enum, which carries no tag form ("The
/// `InstanceInformationFilterList` method is a legacy method and doesn't support tags").
pub fn is_legacy_node_filter_key(key: &str) -> bool {
    INSTANCE_INFORMATION_FILTER_KEY_VALUES.contains(&key)
}

/// Whether `id` is an activation id — the modelled `ActivationId` pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
pub fn is_activation_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return false;
        }
    }
    groups.next().is_none()
}

/// Whether `version` is an SSM Agent version — the modelled `Version` pattern
/// `^[0-9]{1,6}(\.[0-9]{1,6}){2,3}$`: three or four dot-separated runs of 1..6 digits.
pub fn is_agent_version(version: &str) -> bool {
    let groups: Vec<&str> = version.split('.').collect();
    (3..=4).contains(&groups.len())
        && groups
            .iter()
            .all(|group| (1..=6).contains(&group.len()) && group.bytes().all(|b| b.is_ascii_digit()))
}

/// Why `value` is not a value the node attribute `key` filters could hold, or `None`
/// when it could be one.
///
/// The rule is one sentence: a filter value has to be a legal value of the member it
/// filters, as the model constrains that member — one of its enum's values
/// ([`node_filter_key_value_set`]), or a string matching its pattern. A key whose member
/// the model leaves unconstrained (`IamRole`, `SourceIds`, a tag) constrains nothing
/// here, and a node id is checked by its caller, which answers it with the operation's
/// own `InvalidInstanceId` rather than with a filter-value error.
pub fn node_filter_value_rejection(key: &str, value: &str) -> Option<String> {
    if let Some(catalog) = node_filter_key_value_set(key) {
        if !catalog.contains(&value) {
            return Some(node_filter_value_set_rejection(key, catalog));
        }
        return None;
    }
    match key {
        "ActivationIds" if !is_activation_id(value) => Some(format!(
            "Value '{value}' for filter key '{key}' is not an activation id"
        )),
        "AgentVersion" if !is_agent_version(value) => Some(format!(
            "Value '{value}' for filter key '{key}' is not an SSM Agent version"
        )),
        _ => None,
    }
}

/// A parameter's `default` as the single string `DefaultValue` reports: the scalar
/// itself, or the JSON rendering of a list or mapping default.
///
/// `budget` is the recursion left, so a deeply nested default cannot drive this into
/// the stack even though [`guard_document_content`] already bounded the nesting; a
/// default that exhausts it is REFUSED rather than reported with the member missing.
fn default_value_string(
    name: &str,
    value: &::yaml_rust2::Yaml,
    budget: usize,
) -> Result<String, ContentRejection> {
    if let Some(scalar) = scalar_string(value) {
        return Ok(scalar);
    }
    let rendered = document_as_json(value, budget).ok_or_else(|| {
        ContentRejection::Content(format!(
            "the default of parameter '{name}' is not a value this service can read: it nests \
             past {MAX_DOCUMENT_DEPTH} levels or is keyed by a non-string"
        ))
    })?;
    Ok(rendered.to_string())
}

/// A document node as the JSON value that renders it.
pub(crate) fn document_as_json(
    node: &::yaml_rust2::Yaml,
    budget: usize,
) -> Option<::serde_json::Value> {
    use ::serde_json::Value;
    use ::yaml_rust2::Yaml;
    let budget = budget.checked_sub(1)?;
    Some(match node {
        Yaml::Null | Yaml::BadValue => Value::Null,
        Yaml::Boolean(flag) => Value::Bool(*flag),
        Yaml::Integer(number) => Value::Number((*number).into()),
        Yaml::Real(real) => real
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or_else(|| Value::String(real.clone()), Value::Number),
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| document_as_json(item, budget))
                .collect::<Option<Vec<_>>>()?,
        ),
        Yaml::Hash(map) => {
            let mut object = ::serde_json::Map::new();
            for (key, value) in map {
                object.insert(key.as_str()?.to_string(), document_as_json(value, budget)?);
            }
            Value::Object(object)
        }
        // An alias is refused before the load by `reject_document_aliases`, so a
        // resolved alias node can never reach here.
        Yaml::Alias(_) => return None,
    })
}

/// The parameters a document declares: one `DocumentParameter` per `parameters` entry,
/// in the content's own order.
///
/// "A description of the parameters for a document"
/// (`DocumentDescription$Parameters`): each entry's `type`, `description` and
/// `default` are reported as the content declares them — the declared type is reported
/// verbatim rather than narrowed, because an Automation document's parameters carry
/// types (`Integer`, `StringMap`, …) beyond the two a Command document's do.
fn declared_parameters(
    map: &::yaml_rust2::yaml::Hash,
) -> Result<Vec<DocumentParameter>, ContentRejection> {
    let Some(declared) = mapping_get(map, "parameters") else {
        return Ok(Vec::new());
    };
    if matches!(declared, ::yaml_rust2::Yaml::Null) {
        return Ok(Vec::new());
    }
    let Some(declared) = declared.as_hash() else {
        return Err(ContentRejection::Content(
            "the parameters of a document must be a mapping of parameter name to its definition"
                .to_string(),
        ));
    };
    let mut parameters = Vec::with_capacity(declared.len());
    for (name, definition) in declared {
        let Some(name) = name.as_str() else {
            return Err(ContentRejection::Content(
                "every parameter of a document must be named by a string".to_string(),
            ));
        };
        let Some(definition) = definition.as_hash() else {
            return Err(ContentRejection::Content(format!(
                "the definition of parameter '{name}' must be a mapping"
            )));
        };
        let default_value = match mapping_get(definition, "default") {
            None => None,
            Some(default) => Some(default_value_string(name, default, MAX_DOCUMENT_DEPTH)?),
        };
        parameters.push(DocumentParameter {
            name: Some(name.to_string()),
            r#type: mapping_get(definition, "type").and_then(scalar_string),
            description: mapping_get(definition, "description").and_then(scalar_string),
            default_value,
        });
    }
    Ok(parameters)
}

/// The names of the steps `content` declares, in its own order.
///
/// A schema 2.x document declares its steps as `mainSteps`, each with its own `name`
/// ("The name of the step"), and a schema 1.2 document declares them as the `runtimeConfig`
/// plugin entries, whose keys are the plugin names. A `GetCommandInvocation` asks for one
/// step's detailed results by that name, so both spellings are read.
fn declared_step_names(map: &::yaml_rust2::yaml::Hash) -> Vec<String> {
    if let Some(steps) = mapping_get(map, "mainSteps").and_then(::yaml_rust2::Yaml::as_vec) {
        return steps
            .iter()
            .filter_map(|step| mapping_get(step.as_hash()?, "name").and_then(scalar_string))
            .collect();
    }
    mapping_get(map, "runtimeConfig")
        .and_then(::yaml_rust2::Yaml::as_hash)
        .map(|plugins| {
            plugins
                .keys()
                .filter_map(::yaml_rust2::Yaml::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The ONE document tree `content` holds, or why it is not one document tree this
/// service accepts.
///
/// This is what "one document tree" means for this crate: the create that derives a
/// document's report from its content and the read that re-renders that content in
/// another format ([`document_format`]) both read it through here.
pub fn load_single_document(content: &str) -> Result<::yaml_rust2::Yaml, ContentRejection> {
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(content).map_err(|why| {
        ContentRejection::Content(format!("Content could not be parsed as a document: {why}"))
    })?;
    if documents.len() != 1 {
        return Err(ContentRejection::Content(format!(
            "Content must be exactly one document; found {}",
            documents.len()
        )));
    }
    Ok(documents.remove(0))
}

/// What `content` declares, or why it is not a document this service accepts.
///
/// "The content for the new SSM document in JSON or YAML format"
/// (`CreateDocument`'s `Content` doc), so the content is parsed in the format the
/// request declared — YAML 1.2 is a superset of JSON, so one parser reads either, and
/// `JSON` is additionally required to BE JSON rather than only YAML that resembles it.
/// `TEXT` content (a Change Calendar entry) is free text with no document tree, so
/// nothing is derived from it and nothing in it is refused.
pub fn parse_document_content(
    content: &str,
    format: &str,
    document_type: &str,
) -> Result<ParsedDocumentContent, ContentRejection> {
    // The documented size bound is measured for content of every format, including the
    // one with no document tree to parse.
    content_within_size(content)?;
    if format == DOCUMENT_FORMAT_TEXT {
        return Ok(ParsedDocumentContent::default());
    }
    guard_document_content(content)?;
    if format == DEFAULT_DOCUMENT_FORMAT
        && ::serde_json::from_str::<::serde_json::Value>(content).is_err()
    {
        return Err(ContentRejection::Content(
            "Content isn't a valid JSON document".to_string(),
        ));
    }
    reject_document_aliases(content)?;
    let document = load_single_document(content)?;
    let Some(map) = document.as_hash() else {
        return Err(ContentRejection::Content(
            "Content must be a mapping of document keys to values".to_string(),
        ));
    };

    let schema_version = mapping_get(map, "schemaVersion").and_then(scalar_string);
    let supported = supported_schema_versions(document_type);
    if !supported.is_empty() {
        match schema_version.as_deref() {
            None => {
                return Err(ContentRejection::Content(format!(
                    "Content declares no schemaVersion; a {document_type} document must declare \
                     one of {}",
                    supported.join(", ")
                )))
            }
            Some(declared) if !supported.contains(&declared) => {
                return Err(ContentRejection::SchemaVersion(format!(
                    "Document schema version '{declared}' isn't supported for a {document_type} \
                     document; the supported schema versions are {}",
                    supported.join(", ")
                )))
            }
            Some(_) => {}
        }
    }

    Ok(ParsedDocumentContent {
        schema_version,
        description: mapping_get(map, "description").and_then(scalar_string),
        parameters: declared_parameters(map)?,
        platform_types: document_platforms::declared_platform_types(map),
        step_names: declared_step_names(map),
    })
}

// ---------------------------------------------------------------------------
// The SSM documents Systems Manager publishes
// ---------------------------------------------------------------------------

/// The `Owner` a document Systems Manager publishes reports.
///
/// `Owner` is "the Amazon Web Services user that created the document"
/// (`DocumentDescription$Owner`), and a published document was created by Amazon rather
/// than by the account that reads it — the owner a listing selects it by
/// (`ListDocuments`' `Owner` filter: `Self`, `Amazon`, `Public`, `Private`,
/// `ThirdParty`).
pub const PUBLISHED_DOCUMENT_OWNER: &str = "Amazon";

/// One document of the catalogue Systems Manager publishes: present in every account and
/// Region without anybody creating it, and readable — never writable — by the account
/// that reads it.
#[derive(Debug, Clone, Copy)]
pub struct PublishedDocument {
    /// The name the document resolves by, which is also what a read reports.
    pub name: &'static str,
    /// A `DocumentType` value (`crate::ops::types::DOCUMENT_TYPE_VALUES`).
    pub document_type: &'static str,
    /// The document's body, in [`DEFAULT_DOCUMENT_FORMAT`]. Everything a read reports
    /// beyond the name and the type is derived from it by the one content reader
    /// ([`parse_document_content`]), exactly as it is for a document a caller created.
    pub content: &'static str,
}

/// The body of the runbook that configures versioning on an S3 bucket.
///
/// The parameters each published runbook takes are the ones the "Systems Manager
/// Automation runbook reference" documents for it; the body below is this emulator's
/// stand-in for the published body, declaring those parameters so a read derives them.
const CONFIGURE_S3_BUCKET_VERSIONING: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Configures versioning on an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket to configure versioning on."
    },
    "VersioningState": {
      "type": "String",
      "description": "(Optional) The versioning state of the bucket.",
      "default": "Enabled",
      "allowedValues": ["Enabled", "Suspended"]
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketVersioning",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutBucketVersioning",
        "Bucket": "{{ BucketName }}",
        "VersioningConfiguration": { "Status": "{{ VersioningState }}" }
      }
    },
    {
      "name": "VerifyBucketVersioning",
      "action": "aws:executeAwsApi",
      "inputs": { "Service": "s3", "Api": "GetBucketVersioning", "Bucket": "{{ BucketName }}" }
    }
  ]
}"#;

/// The body of the runbook that blocks public access on an S3 bucket. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
const CONFIGURE_S3_BUCKET_PUBLIC_ACCESS_BLOCK: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Configures the public access block of an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket to configure."
    },
    "BlockPublicAcls": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 blocks public ACLs for this bucket.",
      "default": true
    },
    "IgnorePublicAcls": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 ignores public ACLs for this bucket.",
      "default": true
    },
    "BlockPublicPolicy": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 blocks public bucket policies for this bucket.",
      "default": true
    },
    "RestrictPublicBuckets": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 restricts public bucket policies for this bucket.",
      "default": true
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketPublicAccessBlock",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutPublicAccessBlock",
        "Bucket": "{{ BucketName }}",
        "PublicAccessBlockConfiguration": {
          "BlockPublicAcls": "{{ BlockPublicAcls }}",
          "IgnorePublicAcls": "{{ IgnorePublicAcls }}",
          "BlockPublicPolicy": "{{ BlockPublicPolicy }}",
          "RestrictPublicBuckets": "{{ RestrictPublicBuckets }}"
        }
      }
    }
  ]
}"#;

/// The body of the runbook that removes a bucket's public read and write grants. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
const DISABLE_S3_BUCKET_PUBLIC_READ_WRITE: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Removes the public read and write grants of an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "S3BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket whose public grants are removed."
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketAcl",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutBucketAcl",
        "Bucket": "{{ S3BucketName }}",
        "ACL": "private"
      }
    }
  ]
}"#;

/// The body of the runbook that turns on default encryption of an S3 bucket. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
const ENABLE_S3_BUCKET_ENCRYPTION: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Turns on default server-side encryption of an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket to encrypt."
    },
    "SSEAlgorithm": {
      "type": "String",
      "description": "(Optional) The server-side encryption algorithm to apply by default.",
      "default": "AES256"
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketEncryption",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutBucketEncryption",
        "Bucket": "{{ BucketName }}",
        "ServerSideEncryptionConfiguration": {
          "Rules": [
            {
              "ApplyServerSideEncryptionByDefault": { "SSEAlgorithm": "{{ SSEAlgorithm }}" }
            }
          ]
        }
      }
    }
  ]
}"#;

/// The body of the runbook that publishes a notification to an SNS topic. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
const PUBLISH_SNS_NOTIFICATION: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Publishes a notification to an Amazon SNS topic.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "TopicArn": {
      "type": "String",
      "description": "(Required) The ARN of the SNS topic to publish to."
    },
    "Message": {
      "type": "String",
      "description": "(Required) The message to publish."
    }
  },
  "mainSteps": [
    {
      "name": "Publish",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "sns",
        "Api": "Publish",
        "TopicArn": "{{ TopicArn }}",
        "Message": "{{ Message }}"
      }
    }
  ]
}"#;

/// The body of the command document that runs a shell script on a managed node.
///
/// A command document's step actions and preconditions are what restrict it to a platform
/// ([`document_platforms::declared_platform_types`]), so the platforms the published
/// document is reported
/// compatible with are declared the same way a created document's are.
const RUN_SHELL_SCRIPT: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Run a shell script or specify the commands to run.",
  "parameters": {
    "commands": {
      "type": "StringList",
      "description": "(Required) Specify a shell script or a command to run."
    },
    "workingDirectory": {
      "type": "String",
      "description": "(Optional) The path to the working directory on your managed node.",
      "default": ""
    },
    "executionTimeout": {
      "type": "String",
      "description": "(Optional) The time in seconds for a command to be completed before it is considered to have failed.",
      "default": "3600"
    }
  },
  "mainSteps": [
    {
      "name": "runShellScriptOnLinux",
      "action": "aws:runShellScript",
      "precondition": { "StringEquals": ["platformType", "Linux"] },
      "inputs": {
        "commands": "{{ commands }}",
        "workingDirectory": "{{ workingDirectory }}",
        "timeoutSeconds": "{{ executionTimeout }}"
      }
    },
    {
      "name": "runShellScriptOnMacOS",
      "action": "aws:runShellScript",
      "precondition": { "StringEquals": ["platformType", "MacOS"] },
      "inputs": {
        "commands": "{{ commands }}",
        "workingDirectory": "{{ workingDirectory }}",
        "timeoutSeconds": "{{ executionTimeout }}"
      }
    }
  ]
}"#;

/// The body of the command document that runs a PowerShell script on a managed node. See
/// [`RUN_SHELL_SCRIPT`] for how the platforms are declared.
const RUN_POWER_SHELL_SCRIPT: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Run a PowerShell script or specify the commands to run.",
  "parameters": {
    "commands": {
      "type": "StringList",
      "description": "(Required) Specify a PowerShell script or a command to run."
    },
    "workingDirectory": {
      "type": "String",
      "description": "(Optional) The path to the working directory on your managed node.",
      "default": ""
    },
    "executionTimeout": {
      "type": "String",
      "description": "(Optional) The time in seconds for a command to be completed before it is considered to have failed.",
      "default": "3600"
    }
  },
  "mainSteps": [
    {
      "name": "runPowerShellScript",
      "action": "aws:runPowerShellScript",
      "precondition": { "StringEquals": ["platformType", "Windows"] },
      "inputs": {
        "commands": "{{ commands }}",
        "workingDirectory": "{{ workingDirectory }}",
        "timeoutSeconds": "{{ executionTimeout }}"
      }
    }
  ]
}"#;

/// The body of the command document that updates the SSM Agent on a managed node. It
/// declares no precondition, so it is compatible with every platform the model lists.
const UPDATE_SSM_AGENT: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Update the SSM Agent to the latest version or specify an older version.",
  "parameters": {
    "version": {
      "type": "String",
      "description": "(Optional) The version of the SSM Agent to install. An empty value installs the latest version.",
      "default": ""
    },
    "allowDowngrade": {
      "type": "String",
      "description": "(Optional) Whether the SSM Agent may be downgraded to an earlier version.",
      "default": "false",
      "allowedValues": ["true", "false"]
    }
  },
  "mainSteps": [
    {
      "name": "updateSSMAgent",
      "action": "aws:updateSsmAgent",
      "inputs": {
        "agentName": "amazon-ssm-agent",
        "source": "https://s3.amazonaws.com/ec2-downloads-windows/SSMAgent/{{version}}/amazon-ssm-agent.msi",
        "allowDowngrade": "{{ allowDowngrade }}",
        "targetVersion": "{{ version }}"
      }
    }
  ]
}"#;

/// The documents Systems Manager publishes, which exist in every account and Region
/// without anybody creating them.
///
/// The names and document types are the ones the AWS Systems Manager User Guide
/// publishes — the "Systems Manager Automation runbook reference" for the `AWS-*`
/// Automation runbooks (the runbooks a Config remediation configuration names as its
/// target) and the "Systems Manager command document" reference for the `AWS-*` command
/// documents. Each body declares the parameters that reference documents for its
/// document, so a read derives its description, schema version, parameters and platform
/// types from the body rather than restating them here.
///
/// Every name here starts with one of the prefixes [`RESERVED_DOCUMENT_NAME_PREFIXES`]
/// reserves, which is why no `CreateDocument` can collide with one.
pub const PUBLISHED_DOCUMENTS: &[PublishedDocument] = &[
    PublishedDocument {
        name: "AWS-ConfigureS3BucketVersioning",
        document_type: "Automation",
        content: CONFIGURE_S3_BUCKET_VERSIONING,
    },
    PublishedDocument {
        name: "AWS-ConfigureS3BucketPublicAccessBlock",
        document_type: "Automation",
        content: CONFIGURE_S3_BUCKET_PUBLIC_ACCESS_BLOCK,
    },
    PublishedDocument {
        name: "AWS-DisableS3BucketPublicReadWrite",
        document_type: "Automation",
        content: DISABLE_S3_BUCKET_PUBLIC_READ_WRITE,
    },
    PublishedDocument {
        name: "AWS-EnableS3BucketEncryption",
        document_type: "Automation",
        content: ENABLE_S3_BUCKET_ENCRYPTION,
    },
    PublishedDocument {
        name: "AWS-PublishSNSNotification",
        document_type: "Automation",
        content: PUBLISH_SNS_NOTIFICATION,
    },
    PublishedDocument {
        name: "AWS-RunShellScript",
        document_type: "Command",
        content: RUN_SHELL_SCRIPT,
    },
    PublishedDocument {
        name: "AWS-RunPowerShellScript",
        document_type: "Command",
        content: RUN_POWER_SHELL_SCRIPT,
    },
    PublishedDocument {
        name: "AWS-UpdateSSMAgent",
        document_type: "Command",
        content: UPDATE_SSM_AGENT,
    },
];

/// The published document `name` names, or `None` when the catalogue holds none.
///
/// A name is resolved as written: an SSM document name is case-sensitive, so a
/// differently-cased spelling names no document.
pub fn published_document(name: &str) -> Option<&'static PublishedDocument> {
    PUBLISHED_DOCUMENTS
        .iter()
        .find(|published| published.name == name)
}

/// What each catalogue body yields: the members the content reader derives from it, and
/// its hash.
///
/// Derived ONCE for the process rather than per read. The bodies are `const`, so two
/// reads of one published document cannot differ, and no request pays for parsing them.
/// A body this reader cannot read — a mistake in a `const` of this crate, which the test
/// suite reads every entry to catch, never a caller's input — yields nothing derived
/// rather than panicking a live request: the document does exist either way.
static PUBLISHED_PROJECTIONS: ::std::sync::LazyLock<Vec<(ParsedDocumentContent, String)>> =
    ::std::sync::LazyLock::new(|| {
        PUBLISHED_DOCUMENTS
            .iter()
            .map(|published| {
                let parsed = parse_document_content(
                    published.content,
                    DEFAULT_DOCUMENT_FORMAT,
                    published.document_type,
                )
                .unwrap_or_default();
                (parsed, document_content_hash(published.content))
            })
            .collect()
    });

/// The published document `name` names together with what its body yields, or `None`
/// when the catalogue holds no document of that name.
fn published_document_projection(
    name: &str,
) -> Option<(
    &'static PublishedDocument,
    &'static ParsedDocumentContent,
    &'static str,
)> {
    let index = PUBLISHED_DOCUMENTS
        .iter()
        .position(|published| published.name == name)?;
    let published = PUBLISHED_DOCUMENTS.get(index)?;
    let (parsed, hash) = PUBLISHED_PROJECTIONS.get(index)?;
    Some((published, parsed, hash.as_str()))
}

/// The hash the system takes over a document's content: "The Sha256 or Sha1 hash created
/// by the system when the document was created" (`DocumentDescription$Hash`), with "Sha1
/// hashes have been deprecated", rendered hex the way AWS reports it.
pub fn document_content_hash(content: &str) -> String {
    use ::sha2::Digest;
    ::hex::encode(::sha2::Sha256::digest(content.as_bytes()))
}

/// The attachment names `AttachmentsInformation` reports — "details about the document
/// attachments, including names" — for the attachment sources one call named.
///
/// An `AttachmentsSource` carries "the name of the document attachment file" in `Name`;
/// a source that names none is identified by the file its `Values` URL points at, which
/// is the last segment of that URL. Read by every call that supplies attachments for a
/// version of a document, so a create and an update identify them the same way.
pub fn attachment_names(attachments: Option<&[AttachmentsSource]>) -> Vec<String> {
    attachments
        .unwrap_or_default()
        .iter()
        .filter_map(|source| {
            source
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    let value = source.values.as_ref()?.first()?;
                    let file = value.rsplit('/').next().unwrap_or(value);
                    (!file.is_empty()).then(|| file.to_string())
                })
        })
        .collect()
}

/// ONE version of a stored SSM document: what the call that minted it supplied, what the
/// service derived from that version's content, and the status move the clock still owes
/// it.
///
/// Everything here is per version because that is what the model says varies between
/// them: `DisplayName` "can differ for each version of the document", a `VersionName` is
/// "unique across all versions of a document", and the content — and so the hash, the
/// schema version, the description, the parameters and the platform types derived from it
/// — is what an update supplies a new one of.
#[derive(Debug, Clone)]
pub struct DocumentVersionRecord {
    /// "The document version", counting from [`FIRST_DOCUMENT_VERSION`].
    pub version: u64,
    pub display_name: Option<String>,
    pub version_name: Option<String>,
    /// A `DocumentFormat` value: the format this version's content is written in.
    pub document_format: String,
    pub target_type: Option<String>,
    /// The content exactly as the call supplied it: what
    /// [`DocumentVersionRecord::hash`] is taken over, and what `GetDocument` reports.
    pub content: String,
    /// Hex-rendered Sha256 of [`DocumentVersionRecord::content`].
    pub hash: String,
    /// Everything derived from the content (see [`parse_document_content`]).
    pub parsed: ParsedDocumentContent,
    pub requires: Option<Vec<DocumentRequires>>,
    /// The attachment names the call named, which is what `AttachmentsInformation`
    /// reports.
    pub attachment_names: Vec<String>,
    pub created_date: SystemTime,
    /// A `DocumentStatus` value.
    pub status: String,
    /// When the clock owes this version's move to `Active`, or `None` once that move has
    /// been materialised.
    pub active_due: Option<SystemTime>,
}

impl DocumentVersionRecord {
    /// Materialise the status move the clock already owes at `now` — the move to
    /// `Active` that was due whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.active_due.is_some_and(|due| now >= due) {
            self.active_due = None;
            self.status = DOCUMENT_STATUS_ACTIVE.to_string();
        }
    }
}

/// One stored SSM document: the identity and the metadata the whole document carries,
/// and every version of it the store holds.
///
/// A create mints the first version and each `UpdateDocument` appends the next one, so
/// `LatestVersion` is the last version held. The DEFAULT version moves only when
/// `UpdateDocumentDefaultVersion` moves it, which is why an update's new version is the
/// latest without becoming the default.
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    /// The identifier Systems Manager assigned the document when it was created, which is
    /// what a read reports as `DocumentId`: minted once, per document, so two reads of one
    /// document report the same id and no two documents share one
    /// ([`ComAmazonawsSsmAmazonssmState::mint_document_id`]). Nothing of the request is in
    /// it, and it is not the key the document is reached by — a document is addressed by
    /// name.
    pub document_id: String,
    /// The name the create supplied, which is what a read reports and part of the
    /// store key.
    pub name: String,
    /// The Region the document was created in; part of its store key and of its ARN.
    pub region: String,
    /// A `DocumentType` value. One document is of one type: `UpdateDocument` carries no
    /// `DocumentType` member, so a version never changes it.
    pub document_type: String,
    pub tags: BTreeMap<String, String>,
    /// "The Amazon Web Services user that created the document"
    /// (`DocumentDescription$Owner`): the account for a document a caller created here,
    /// and [`PUBLISHED_DOCUMENT_OWNER`] for one Systems Manager publishes.
    pub owner: String,
    /// Every version this document holds, oldest first and the newest last. Never empty
    /// for a record that is in the store.
    pub versions: Vec<DocumentVersionRecord>,
    /// "The default version", the one a read that names no version resolves to.
    pub default_version: u64,
}

/// Why a read's `DocumentVersion` / `VersionName` pair names no version of a document,
/// carrying what was asked for so each operation can report it in its own words.
#[derive(Debug, Clone)]
pub enum DocumentVersionRejection {
    /// No version of the document is the one this `DocumentVersion` selector names.
    Version(String),
    /// No version of the document carries this `VersionName`.
    VersionName(String),
}

impl DocumentRecord {
    /// This document's ARN: `arn:aws:ssm:<region>:<account>:document/<name>`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            &self.region,
            &format!("{ARN_DOCUMENT_RESOURCE_PREFIX}{}", self.name),
        )
    }

    /// Materialise the status move the clock already owes every version at `now` — the
    /// move to `Active` that was due whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for version in &mut self.versions {
            version.materialize_due(now);
        }
    }

    /// The newest version, which is "the latest version of the document"
    /// (`DocumentDescription$LatestVersion`).
    pub fn latest(&self) -> Option<&DocumentVersionRecord> {
        self.versions.last()
    }

    /// The number "the latest version of the document" carries.
    pub fn latest_version(&self) -> u64 {
        self.latest()
            .map_or(FIRST_DOCUMENT_VERSION, |latest| latest.version)
    }

    /// The number the next version of this document carries: each update mints the one
    /// after the latest.
    pub fn next_version(&self) -> u64 {
        self.latest_version().saturating_add(1)
    }

    /// The version `version` numbers, or `None` when this document holds no such version.
    pub fn version(&self, version: u64) -> Option<&DocumentVersionRecord> {
        self.versions.iter().find(|held| held.version == version)
    }

    /// The version `version_name` names: "This value is unique across all versions of a
    /// document" (`DocumentVersionInfo$VersionName`), so it names at most one of them.
    pub fn version_named(&self, version_name: &str) -> Option<&DocumentVersionRecord> {
        self.versions
            .iter()
            .find(|held| held.version_name.as_deref() == Some(version_name))
    }

    /// The version a `DocumentVersion` selector names, or `None` when this document
    /// holds none.
    ///
    /// The modelled `DocumentVersion` pattern admits three spellings: `$LATEST` names the
    /// newest version, `$DEFAULT` names "the default version", and a number names the
    /// version it counts.
    pub fn select(&self, selector: &str) -> Option<&DocumentVersionRecord> {
        match selector {
            DOCUMENT_VERSION_LATEST => self.latest(),
            DOCUMENT_VERSION_DEFAULT => self.version(self.default_version),
            number => self.version(number.parse().ok()?),
        }
    }

    /// The version a read's `DocumentVersion` and `VersionName` pair names, or why it
    /// names none of this document's versions.
    ///
    /// A read that names neither resolves to the default version. A read that names only
    /// a version NAME resolves to the version carrying it, because that name is "unique
    /// across all versions of a document"; one that names both has to name the same
    /// version twice, so a name the selected version does not carry names no version.
    pub fn select_requested(
        &self,
        document_version: Option<&str>,
        version_name: Option<&str>,
    ) -> Result<&DocumentVersionRecord, DocumentVersionRejection> {
        let document_version = document_version.filter(|selector| !selector.is_empty());
        let version_name = version_name.filter(|name| !name.is_empty());
        let selected = match (document_version, version_name) {
            (Some(selector), _) => self
                .select(selector)
                .ok_or_else(|| DocumentVersionRejection::Version(selector.to_string()))?,
            (None, Some(name)) => {
                return self
                    .version_named(name)
                    .ok_or_else(|| DocumentVersionRejection::VersionName(name.to_string()));
            }
            (None, None) => self.select(DOCUMENT_VERSION_DEFAULT).ok_or_else(|| {
                DocumentVersionRejection::Version(DOCUMENT_VERSION_DEFAULT.to_string())
            })?,
        };
        match version_name {
            Some(name) if selected.version_name.as_deref() != Some(name) => {
                Err(DocumentVersionRejection::VersionName(name.to_string()))
            }
            _ => Ok(selected),
        }
    }

    /// Whether `requested` names a version name this document carries.
    pub fn holds_version_name(&self, requested: &str) -> bool {
        self.version_named(requested).is_some()
    }

    /// The version whose content hashes to `hash`, or `None` when no version of this
    /// document holds that content.
    ///
    /// The hash is taken over the content ([`document_content_hash`]), so two versions
    /// hash alike exactly when their content is the same — which is what
    /// `DuplicateDocumentContent` is reported for.
    pub fn version_with_hash(&self, hash: &str) -> Option<&DocumentVersionRecord> {
        self.versions.iter().find(|held| held.hash == hash)
    }

    /// "The current review status of a new custom Systems Manager document"
    /// (`DocumentDescription$ReviewStatus`).
    ///
    /// A review is the change-template approval workflow, and `UpdateDocumentMetadata` —
    /// the operation that records one — is still a scaffold, so a change template created
    /// here has not been reviewed and no other document type carries a review at all.
    pub fn review_status(&self) -> Option<String> {
        (self.document_type == CHANGE_TEMPLATE_DOCUMENT_TYPE)
            .then(|| DOCUMENT_REVIEW_STATUS_NOT_REVIEWED.to_string())
    }

    /// The modelled `DocumentDescription` of `version` of this document: the version's
    /// own members beside the whole document's name, owner, type, tags, latest version
    /// and default version.
    pub fn description_of(&self, version: &DocumentVersionRecord) -> DocumentDescription {
        DocumentDescription {
            // The id Systems Manager assigned the document, which every read of it
            // reports.
            document_id: Some(self.document_id.clone()),
            // "Sha1 hashes have been deprecated", so the system hash is the Sha256 in
            // `Hash` and the deprecated member is left unset.
            sha1: None,
            hash: Some(version.hash.clone()),
            hash_type: Some(DOCUMENT_HASH_TYPE.to_string()),
            name: Some(self.name.clone()),
            display_name: version.display_name.clone(),
            version_name: version.version_name.clone(),
            owner: Some(self.owner.clone()),
            created_date: Some(::aws_smithy_types::DateTime::from(version.created_date)),
            status: Some(version.status.clone()),
            // "A message returned by Amazon Web Services Systems Manager that explains
            // the Status value", which it sets for a failure; nothing here failed.
            status_information: None,
            document_version: Some(version.version.to_string()),
            description: version.parsed.description.clone(),
            parameters: (!version.parsed.parameters.is_empty())
                .then(|| version.parsed.parameters.clone()),
            platform_types: (!version.parsed.platform_types.is_empty())
                .then(|| version.parsed.platform_types.clone()),
            document_type: Some(self.document_type.clone()),
            schema_version: version.parsed.schema_version.clone(),
            latest_version: Some(self.latest_version().to_string()),
            default_version: Some(self.default_version.to_string()),
            document_format: Some(version.document_format.clone()),
            target_type: version.target_type.clone(),
            // "The tags, or metadata, that have been applied to the document"
            // (`DocumentDescription$Tags`, a `TagList` of length 0..1000): a document
            // carrying none has an EMPTY tag list rather than no list at all, so a caller
            // iterating `Tags` never has to tell a missing member from an empty one.
            tags: Some(tagging::to_tag_list(&self.tags)),
            attachments_information: (!version.attachment_names.is_empty()).then(|| {
                version
                    .attachment_names
                    .iter()
                    .map(|name| AttachmentInformation {
                        name: Some(name.clone()),
                    })
                    .collect()
            }),
            requires: version.requires.clone(),
            // "The user in your organization who created the document": the emulator
            // authenticates no human author, so there is none to report.
            author: None,
            // A review is the change-template approval workflow, and `UpdateDocument`
            // metadata — the operation that records one — is still a scaffold, so a
            // change template created here has not been reviewed and no other document
            // type carries a review at all.
            review_information: None,
            approved_version: None,
            pending_review_version: None,
            review_status: self.review_status(),
            // "The classification of a document to help you identify and categorize
            // its use", which Systems Manager assigns to the documents it publishes;
            // nothing classifies a document created through this API.
            category: None,
            category_enum: None,
        }
    }

    /// The modelled `DocumentDescription` of the version `selector` names, or `None` when
    /// this document holds no such version.
    pub fn description_of_version(&self, selector: &str) -> Option<DocumentDescription> {
        Some(self.description_of(self.select(selector)?))
    }

    /// The same description as a DESCRIBE of this document reports it: the document's
    /// classification members carried as the empty lists a document nothing classified
    /// has.
    ///
    /// `Category` is "the classification of a document to help you identify and
    /// categorize its use" and `CategoryEnum` "the value that identifies a document's
    /// category" (both `CategoryList`/`CategoryEnumList`, `@length` 0..3): Systems Manager
    /// classifies the documents it publishes, so a document created through this API
    /// carries NO category — which a describe reports as an empty list rather than as a
    /// missing member, the way it reports an untagged document's `Tags`. RECORDED
    /// BEHAVIOUR (corpus `simparity-wire-aws-v1`, fixture `ssm-association-create-and-list`
    /// step `setup#2`): the create's own response carries neither member while the describe
    /// carries both as `[]`, so this shape belongs to the describe rather than to every
    /// read.
    pub fn described_version(&self, version: &DocumentVersionRecord) -> DocumentDescription {
        DocumentDescription {
            category: Some(Vec::new()),
            category_enum: Some(Vec::new()),
            ..self.description_of(version)
        }
    }

    /// Drop the version `version` numbers from this document, reporting whether it held
    /// one. A document whose last version is dropped holds none and is removed whole by
    /// its store ([`ComAmazonawsSsmAmazonssmState::remove_document_version`]).
    pub fn remove_version(&mut self, version: u64) -> bool {
        let held = self.version(version).is_some();
        self.versions.retain(|candidate| candidate.version != version);
        held
    }
}

/// The `PingStatus` a node that has just pinged the service carries (from the modelled
/// enum, `crate::ops::types::PING_STATUS_VALUES`).
pub const PING_STATUS_ONLINE: &str = "Online";

/// The `ResourceType` of a node that is an EC2 instance (from the modelled enum,
/// `crate::ops::types::RESOURCE_TYPE_VALUES`).
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "EC2Instance";

/// The `ResourceType` of a node registered through a hybrid activation (same enum).
pub const RESOURCE_TYPE_MANAGED_INSTANCE: &str = "ManagedInstance";

// ---------------------------------------------------------------------------
// Paging a node report
// ---------------------------------------------------------------------------

/// How many nodes a page holds when the request names no `MaxResults`: "The default
/// value is 10 items" (`DescribeInstanceInformationRequest$MaxResults`).
pub const DEFAULT_NODE_PAGE_SIZE: i32 = 10;

/// The bounds the modelled `MaxResultsEC2Compatible` carries (range 5..50).
pub const MIN_NODE_PAGE_SIZE: i32 = 5;

/// See [`MIN_NODE_PAGE_SIZE`].
pub const MAX_NODE_PAGE_SIZE: i32 = 50;

/// The page size a node report serves, or the refusal message for a `MaxResults` outside
/// the modelled range — which the protocol's boundary validator already refuses, so this
/// is a backstop rather than the first line of defence.
pub fn node_page_size(max_results: Option<i32>) -> Result<usize, String> {
    let requested = max_results.unwrap_or(DEFAULT_NODE_PAGE_SIZE);
    if !(MIN_NODE_PAGE_SIZE..=MAX_NODE_PAGE_SIZE).contains(&requested) {
        return Err(format!(
            "Value '{requested}' at 'maxResults' failed to satisfy constraint: Member must be \
             between {MIN_NODE_PAGE_SIZE} and {MAX_NODE_PAGE_SIZE}"
        ));
    }
    Ok(requested as usize)
}

/// How this service states a `NextToken` it did not issue. `InvalidNextToken`'s own
/// documentation is "The specified token isn't valid".
pub const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";

/// The name this service is reported by in the envelope that follows a refusal worded by
/// the service's own validation: the name its wire protocol carries as the `X-Amz-Target`
/// prefix (`OPERATIONS.md`, "Wire protocol").
pub const SDK_ENVELOPE_SERVICE: &str = "AmazonSSM";

/// `body` followed by the envelope that trails a refusal of this service worded by its own
/// validation: ` (Service: AmazonSSM; Status Code: <status>; Error Code: <code>; Request
/// ID: <id>; Proxy: null)`.
///
/// `status_code` and `error_code` are the status and the code the response itself carries,
/// so the envelope cannot disagree with the wire. The request id is the ONLY per-run field:
/// a request id is minted by the service per call — nothing in the request carries one — so
/// it comes from the emulator's one request-id mint
/// ([`service_common::query_error_envelope::new_request_id`]), a fresh v4 UUID
/// indistinguishable from the one the framework's own renderers report. Nothing of the
/// request is interpolated.
///
/// One function, so every refusal worded this way carries the envelope in the same shape.
pub fn with_sdk_error_envelope(body: &str, status_code: u16, error_code: &str) -> String {
    format!(
        "{body} (Service: {SDK_ENVELOPE_SERVICE}; Status Code: {status_code}; \
         Error Code: {error_code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// Per-service state for com-amazonaws-ssm-amazonssm. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSsmAmazonssmState {
    /// Every parameter, keyed by `(region, canonical name)`: "A parameter name must be
    /// unique within an Amazon Web Services Region". A `BTreeMap` so a listing walks a
    /// stable, total order.
    pub parameters: BTreeMap<(String, String), ParameterRecord>,
    /// Every SSM document, keyed by `(region, name)`: a document name is unique within
    /// a Region, which is what `DocumentAlreadyExists` reports a collision on. A
    /// `BTreeMap` so a listing walks a stable, total order.
    pub documents: BTreeMap<(String, String), DocumentRecord>,

    /// The `DocumentId` of each document Systems Manager publishes ([`PUBLISHED_DOCUMENTS`]),
    /// keyed by its name: a published document exists for as long as the account does, so
    /// there is no create to mint its id. Minted ONCE here, so two reads of one published
    /// document — through any Region's endpoint — report the same id.
    pub published_document_ids: BTreeMap<String, String>,

    /// Every command a caller sent, keyed by `(region, command id)`: a command is sent
    /// to the Systems Manager endpoint of one Region and identified there by the id this
    /// service minted for it. A `BTreeMap` so a listing walks a stable, total order.
    pub commands: BTreeMap<(String, String), commands::CommandRecord>,

    /// How many command ids this store has minted, which is the entropy behind the next
    /// one ([`ComAmazonawsSsmAmazonssmState::mint_command_id`]).
    pub minted_command_ids: u64,

    /// Every automation execution a caller started, keyed by `(region, execution id)`: an
    /// automation runs at the Systems Manager endpoint of one Region and is identified
    /// there by the id the start minted. A `BTreeMap` so a listing walks a stable, total
    /// order. See [`automation`].
    pub automation_executions: BTreeMap<(String, String), automation::AutomationExecutionRecord>,

    /// How many automation execution and step-execution ids this store has minted, which
    /// is the entropy behind the next one
    /// ([`ComAmazonawsSsmAmazonssmState::mint_automation_id`]).
    pub minted_automation_ids: u64,

    /// Every execution preview a caller started, keyed by `(region, execution preview
    /// id)`: a preview is worked out at the Systems Manager endpoint of one Region and is
    /// identified there by the id the start minted. A `BTreeMap` so a walk over them is
    /// stable and total. See [`execution_previews`].
    pub execution_previews:
        BTreeMap<(String, String), execution_previews::ExecutionPreviewRecord>,

    /// How many execution preview ids this store has minted, which is the entropy behind
    /// the next one ([`ComAmazonawsSsmAmazonssmState::mint_execution_preview_id`]).
    pub minted_execution_preview_ids: u64,

    /// Every managed node, keyed by `(region, node id)`: a node is registered with the
    /// Systems Manager endpoint of one Region and is identified there by its node id. A
    /// `BTreeMap` so a describe walks a stable, total order — ascending by node id
    /// within one Region, whatever order its nodes registered in — and can resume a page
    /// from the id it stopped at rather than from an offset.
    ///
    /// Nothing registers a node yet: registration is the SSM Agent's own call, which
    /// this service's model does not expose — `CreateActivation` mints the activation an
    /// agent registers WITH, not the node — and the operation that would admit one
    /// another way (`PutInventory`) is still a scaffold. A describe therefore reports the
    /// empty set for a fresh emulator — what the store holds — rather than nodes nobody
    /// registered. An association names the nodes it targets without registering any of
    /// them, so creating one adds no node here either.
    pub managed_nodes: BTreeMap<(String, String), ManagedNodeRecord>,

    /// Every hybrid activation, keyed by `(region, activation id)`: an activation is
    /// created at one Region's endpoint and is identified there by the id the create
    /// minted. A `BTreeMap` so a report walks a stable, total order — ascending by
    /// activation id within one Region — and can resume a page from the id it stopped at
    /// rather than from an offset.
    pub activations: BTreeMap<(String, String), ActivationRecord>,

    /// Every cloud connector, keyed by `(region, connector id)`: a connector is reached
    /// through the endpoint of the Region it was created in, and is identified there by
    /// the id the create minted. A `BTreeMap` so a listing walks a stable, total order.
    /// See [`cloud_connectors`].
    pub cloud_connectors: BTreeMap<(String, String), cloud_connectors::CloudConnectorRecord>,

    /// Every OpsItem, keyed by `(region, ops item id)`: an OpsItem is created through one
    /// Region's endpoint and is reached through that endpoint by the id the create minted
    /// ([`ops_items::mint_ops_item_id`]). A `BTreeMap` so a listing walks a stable, total
    /// order.
    pub ops_items: BTreeMap<(String, String), ops_items::OpsItemRecord>,

    /// How many OpsItem ids this store has minted, which is what the next mint is drawn
    /// from ([`ComAmazonawsSsmAmazonssmState::next_ops_item_id`]) — so two OpsItems of one
    /// store never carry one id.
    pub ops_item_sequence: u64,

    /// Every event an OpsItem write appended, keyed by `(region, event id)`: an event
    /// belongs to the Region of the OpsItem it is about and is identified there by the id
    /// the append minted ([`ComAmazonawsSsmAmazonssmState::mint_ops_item_event_id`]). A
    /// `BTreeMap` so a listing walks a stable, total order. See [`ops_item_events`].
    pub ops_item_events: BTreeMap<(String, String), ops_item_events::OpsItemEventRecord>,

    /// How many OpsItem event ids this store has minted, which is what the next mint is
    /// drawn from and the order two events of one instant are reported in
    /// ([`ComAmazonawsSsmAmazonssmState::record_ops_item_event`]).
    pub ops_item_event_sequence: u64,

    /// Every related-item association, keyed by `(region, ops item id, association id)`:
    /// an association belongs to the OpsItem it was created against and is identified
    /// there by the id the create minted. A `BTreeMap` so a listing walks a stable, total
    /// order and can resume a page from the association id it stopped at. See
    /// [`ops_item_related_items`].
    pub ops_item_related_items:
        BTreeMap<(String, String, String), ops_item_related_items::RelatedItemRecord>,

    /// How many related-item association ids this store has minted, which is what the next
    /// mint is drawn from
    /// ([`ComAmazonawsSsmAmazonssmState::next_related_item_association_id`]).
    pub ops_item_related_item_sequence: u64,

    /// Every service setting an account has MODIFIED, keyed by `(region, setting id)`: a
    /// setting is reached through one Region's endpoint and its ARN names that Region. A
    /// setting nobody has modified holds no entry and is reported from the catalogue
    /// instead ([`Self::service_setting`]), so the store holds the account's own choices
    /// and invents none.
    pub service_settings: BTreeMap<(String, String), ServiceSettingRecord>,

    /// When this store came into being, and so how old the documents Systems Manager
    /// publishes ([`PUBLISHED_DOCUMENTS`]) are here: they exist for as long as the
    /// account does, so there is no create to date them from. Captured ONCE, so two
    /// reads of one published document report the same `CreatedDate`.
    pub created_at: SystemTime,

    /// Every resource policy this account holds, keyed by the resource it is attached to
    /// — the one store `PutResourcePolicy` writes, `GetResourcePolicies` reads and
    /// `DeleteResourcePolicy` removes from.
    pub resource_policies: resource_policies::ResourcePolicyStore,

    /// Every State Manager association, keyed by `(region, association id)`: an
    /// association is created through one Region's endpoint and is reached there by the id
    /// the create minted. A `BTreeMap` so a listing walks a stable, total order. See
    /// [`associations`].
    pub associations: BTreeMap<(String, String), associations::AssociationRecord>,

    /// Every patch baseline the account created, keyed by `(region, baseline id)`: a
    /// baseline is reached through the endpoint of the Region it was created in and is
    /// identified there by the id the create minted
    /// ([`patch_baselines::mint_baseline_id`]). A `BTreeMap` so a listing walks a stable,
    /// total order. The baselines Systems Manager PREDEFINES hold no entry here and are
    /// resolved from the catalogue instead
    /// ([`ComAmazonawsSsmAmazonssmState::resolve_patch_baseline`]), so the store holds the
    /// account's own baselines and invents none.
    pub patch_baselines: BTreeMap<(String, String), patch_baselines::PatchBaselineRecord>,

    /// How many patch baseline ids this store has minted, which is what the next mint is
    /// drawn from ([`ComAmazonawsSsmAmazonssmState::next_patch_baseline_id`]) — so two
    /// baselines of one store never carry one id.
    pub patch_baseline_sequence: u64,

    /// The default patch baseline of each operating system, keyed by
    /// `(region, operating system)`: "Systems Manager supports creating multiple default
    /// patch baselines. For example, you can create a default patch baseline for each
    /// operating system" (`GetDefaultPatchBaseline`). The value is how the registered
    /// baseline is NAMED (`patch_baselines::ResolvedBaseline::reference`). An operating
    /// system nothing has registered a baseline for holds no entry and is reported from
    /// the predefined catalogue instead.
    pub default_patch_baselines: BTreeMap<(String, String), String>,

    /// Every just-in-time node access request, keyed by `(region, access request id)`: a
    /// request is raised through one Region's endpoint and is reached there by the id the
    /// start minted
    /// ([`ComAmazonawsSsmAmazonssmState::next_access_request_id`]). A `BTreeMap` so a walk
    /// over them is stable and total. See [`just_in_time_node_access`].
    pub access_requests:
        BTreeMap<(String, String), just_in_time_node_access::AccessRequestRecord>,

    /// How many access request ids this store has minted, which is what the next mint is
    /// drawn from — so two access requests of one store never carry one id.
    pub access_request_sequence: u64,

    /// Every resource data sync configuration, keyed by `(region, sync name)`: a sync is
    /// created through one Region's endpoint and its name is unique within that Region. A
    /// `BTreeMap` so a listing walks a stable, total order — ascending by sync name
    /// within one Region — and can resume a page from the name it stopped at rather than
    /// from an offset. See [`resource_data_syncs`].
    pub resource_data_syncs: BTreeMap<(String, String), resource_data_syncs::ResourceDataSyncRecord>,

    /// Every maintenance window the account created, keyed by `(region, window id)`: a
    /// window is reached through the endpoint of the Region it was created in and is
    /// identified there by the id the create minted
    /// ([`maintenance_windows::mint_window_id`]). A `BTreeMap` so a listing walks a
    /// stable, total order. See [`maintenance_windows`].
    pub maintenance_windows:
        BTreeMap<(String, String), maintenance_windows::MaintenanceWindowRecord>,

    /// How many maintenance window ids this store has minted, which is what the next mint
    /// is drawn from ([`ComAmazonawsSsmAmazonssmState::next_maintenance_window_id`]) — so
    /// two windows of one store never carry one id.
    pub maintenance_window_sequence: u64,
}

impl Default for ComAmazonawsSsmAmazonssmState {
    fn default() -> Self {
        Self {
            parameters: BTreeMap::new(),
            documents: BTreeMap::new(),
            published_document_ids: PUBLISHED_DOCUMENTS
                .iter()
                .map(|published| (published.name.to_string(), new_uuid()))
                .collect(),
            commands: BTreeMap::new(),
            minted_command_ids: 0,
            automation_executions: BTreeMap::new(),
            minted_automation_ids: 0,
            execution_previews: BTreeMap::new(),
            minted_execution_preview_ids: 0,
            managed_nodes: BTreeMap::new(),
            activations: BTreeMap::new(),
            cloud_connectors: BTreeMap::new(),
            ops_items: BTreeMap::new(),
            ops_item_sequence: 0,
            ops_item_events: BTreeMap::new(),
            ops_item_event_sequence: 0,
            ops_item_related_items: BTreeMap::new(),
            ops_item_related_item_sequence: 0,
            service_settings: BTreeMap::new(),
            created_at: SystemTime::now(),
            resource_policies: resource_policies::ResourcePolicyStore::default(),
            associations: BTreeMap::new(),
            patch_baselines: BTreeMap::new(),
            patch_baseline_sequence: 0,
            default_patch_baselines: BTreeMap::new(),
            resource_data_syncs: BTreeMap::new(),
            access_requests: BTreeMap::new(),
            access_request_sequence: 0,
            maintenance_windows: BTreeMap::new(),
            maintenance_window_sequence: 0,
        }
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// Materialise every transition the clock already owes at `now`, so a read reports
    /// what was due whether or not anybody called (see [`DOCUMENT_TRANSITION_DELAY`]).
    pub fn materialize_due(&mut self, now: SystemTime) {
        for document in self.documents.values_mut() {
            document.materialize_due(now);
        }
        // A command is reported for as long as Run Command keeps its history
        // (`commands::COMMAND_HISTORY_RETENTION`); one older than that was already gone
        // at `now`, whether or not anybody called.
        self.commands
            .retain(|_, command| now < command.retained_until());
        // An automation execution's pickup and every step of its walk that `now` allows
        // were owed whether or not anybody called (`automation`).
        self.advance_automations(now);
        // The approval an auto-approval policy owes an access request was due at the same
        // instant whether or not anybody called (`just_in_time_node_access`).
        self.materialize_access_requests_due(now);
        // So was the instant an execution preview the service had worked out became
        // reportable (`execution_previews`).
        self.advance_execution_previews(now);
        // And so was the removal of an OpsItem whose delete had settled by then
        // (`ops_items::OPS_ITEM_DELETE_SETTLES_AFTER`).
        self.settle_ops_item_deletions_due(now);
    }

    /// The command `command_id` names in `region`, or `None` when that Region holds none.
    pub fn command(&self, region: &str, command_id: &str) -> Option<&commands::CommandRecord> {
        self.commands
            .get(&(region.to_string(), command_id.to_string()))
    }

    /// Every command `region` holds, as `(command id, record)`: the history that Region's
    /// endpoint reports, in the store's own ascending command-id order. A read that
    /// reports them in another order ranks them itself.
    pub fn commands_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a commands::CommandRecord)> + 'a {
        self.commands
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, id), record)| (id.as_str(), record))
    }

    /// Store `record` as `region`'s command of that id.
    ///
    /// Past [`commands::MAX_COMMANDS_RETAINED`] the oldest command's history is dropped
    /// early, so this store holds a bounded number of records however many sends arrive.
    pub fn insert_command(&mut self, record: commands::CommandRecord) {
        let key = (record.region.clone(), record.command_id.clone());
        self.commands.insert(key, record);
        while self.commands.len() > commands::MAX_COMMANDS_RETAINED {
            let Some(oldest) = self
                .commands
                .iter()
                .min_by_key(|(_, command)| command.requested_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.commands.remove(&oldest);
        }
    }

    /// Mint the `CommandId` of a command being sent.
    ///
    /// "A unique identifier for this command" (`Command$CommandId`), modelled exactly 36
    /// characters long — the hyphenated UUID form, minted through [`minted_uuid`] from
    /// this store's own creation instant and its mint counter, so no two commands of one
    /// run collide and the value is not a count a caller can read off the wire.
    pub fn mint_command_id(&mut self) -> String {
        self.minted_command_ids += 1;
        let since_epoch = self
            .created_at
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default();
        minted_uuid(since_epoch.as_nanos(), self.minted_command_ids)
    }

    /// What `setting` holds at `region`'s endpoint.
    ///
    /// A setting the account has modified is reported from its stored record; one nobody
    /// has modified is reported from the catalogue, at the value its service team
    /// provisioned, the `Default` status and that team as its last modifier. The
    /// provisioned reading is dated by the store's own beginning, because a setting
    /// exists for as long as the account does and no call created it.
    pub fn service_setting(
        &self,
        region: &str,
        setting: &service_settings::SettingDefinition,
    ) -> ServiceSettingRecord {
        self.service_settings
            .get(&(region.to_string(), setting.id.to_string()))
            .cloned()
            .unwrap_or_else(|| ServiceSettingRecord {
                value: setting.default_value.to_string(),
                status: service_settings::STATUS_DEFAULT.to_string(),
                last_modified_date: self.created_at,
                last_modified_user: service_settings::PROVISIONING_USER.to_string(),
            })
    }

    /// Record a modification of the setting `id` names at `region`'s endpoint — what an
    /// update and a reset each persist, so a later read reports the modification rather
    /// than the provisioned reading — or the reason this emulator cannot hold it.
    ///
    /// The store is bounded by [`service_settings::MAX_SERVICE_SETTINGS_TOTAL`], because
    /// the Region a setting is recorded under comes from the request's own credential
    /// scope: a modification that would record a setting for a Region past that total is
    /// refused, while a modification of a setting the store already holds is always
    /// accepted, so the guard can never wedge a setting an account has already set.
    pub fn record_service_setting(
        &mut self,
        region: &str,
        id: &str,
        record: ServiceSettingRecord,
    ) -> Result<(), String> {
        let key = (region.to_string(), id.to_string());
        if !self.service_settings.contains_key(&key)
            && self.service_settings.len() >= service_settings::MAX_SERVICE_SETTINGS_TOTAL
        {
            return Err(service_settings::store_full_message());
        }
        self.service_settings.insert(key, record);
        Ok(())
    }

    /// The document `name` names in `region`, or `None` when that Region holds none.
    pub fn document(&self, region: &str, name: &str) -> Option<&DocumentRecord> {
        self.documents.get(&(region.to_string(), name.to_string()))
    }

    /// The document `name` names in `region`, to write it — to append a version to it or
    /// to change its tags — or `None` when that Region holds none of that name.
    ///
    /// Only a document a caller CREATED here is reachable through this: the catalogue
    /// Systems Manager publishes is projected on read ([`Self::published_document_record`])
    /// and is not the account's to write.
    pub fn document_mut(&mut self, region: &str, name: &str) -> Option<&mut DocumentRecord> {
        self.documents
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// The document Systems Manager publishes under `name`, as it is reached through
    /// `region`'s endpoint, or `None` when the catalogue holds none of that name.
    ///
    /// Built from the catalogue's own body, so everything the read reports beyond the
    /// name and the type — the schema version, the description, the parameters, the
    /// platform types, the hash — is derived by the one content reader that reads a
    /// created document's content. A published document is already registered, so it is
    /// `Active` with no transition owed, and it is owned by Amazon rather than by the
    /// account reading it.
    pub fn published_document_record(&self, region: &str, name: &str) -> Option<DocumentRecord> {
        let (published, parsed, hash) = published_document_projection(name)?;
        Some(DocumentRecord {
            // The id this store minted for the catalogue entry when it came into being;
            // every read of that published document reports it.
            document_id: self
                .published_document_ids
                .get(published.name)
                .cloned()
                .unwrap_or_default(),
            name: published.name.to_string(),
            region: region.to_string(),
            document_type: published.document_type.to_string(),
            tags: BTreeMap::new(),
            owner: PUBLISHED_DOCUMENT_OWNER.to_string(),
            versions: vec![DocumentVersionRecord {
                version: FIRST_DOCUMENT_VERSION,
                display_name: None,
                version_name: None,
                document_format: DEFAULT_DOCUMENT_FORMAT.to_string(),
                target_type: None,
                content: published.content.to_string(),
                hash: hash.to_string(),
                parsed: parsed.clone(),
                requires: None,
                attachment_names: Vec::new(),
                created_date: self.created_at,
                status: DOCUMENT_STATUS_ACTIVE.to_string(),
                active_due: None,
            }],
            default_version: FIRST_DOCUMENT_VERSION,
        })
    }

    /// The document `name` names through `region`'s endpoint: the one a caller created
    /// there, or the one Systems Manager publishes under that name. A name neither holds
    /// names no document, which is what the not-found error reports.
    pub fn resolve_document(&self, region: &str, name: &str) -> Option<DocumentRecord> {
        if let Some(stored) = self.document(region, name) {
            return Some(stored.clone());
        }
        self.published_document_record(region, name)
    }

    /// The document a request names, resolved the way EVERY document read resolves one,
    /// or `None` when no document of that name is reachable through this endpoint — which
    /// is what each read's own not-found error reports.
    ///
    /// A `DocumentARN` value is "the name of the … document. The name can be an Amazon
    /// Resource Name (ARN)", so either spelling names the same document
    /// ([`parse_document_query`]); a value this service could never have stored (an ARN of
    /// another service, say) names no document, so it is a miss rather than a refusal. A
    /// document is reached through the endpoint it is asked for, so an ARN naming another
    /// account or another Region than the request was signed for names nothing reachable
    /// here: `region` comes from the signed credential scope ([`request_region`]) and
    /// never from the request body. An ARN field left EMPTY names no account at all, the
    /// way the ARN of a document Systems Manager publishes carries none, so it resolves
    /// through this endpoint rather than being a miss.
    ///
    /// The one write a report makes happens here: the status move the clock already owed
    /// at `now` is materialised before anything resolves against the store, so a read
    /// reports what was due whether or not anybody called.
    pub fn resolve_document_for_request(
        &mut self,
        name: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<DocumentRecord> {
        let query = parse_document_query(name)?;
        if !query.names_this_endpoint(region) {
            return None;
        }
        self.materialize_due(now);
        self.resolve_document(region, query.name)
    }

    /// Store `record` as `region`'s document of that name.
    pub fn insert_document(&mut self, record: DocumentRecord) {
        let key = (record.region.clone(), record.name.clone());
        self.documents.insert(key, record);
    }

    /// Remove `region`'s document of that name and every version of it, reporting whether
    /// that Region held one.
    ///
    /// "Deletes the … document and all managed node associations to the document"
    /// (`DeleteDocument`), so the associations that apply the document go with it: an
    /// association names the document it applies by name, and the document it named is no
    /// longer there to apply.
    pub fn remove_document(&mut self, region: &str, name: &str) -> bool {
        let removed = self
            .documents
            .remove(&(region.to_string(), name.to_string()))
            .is_some();
        if removed {
            self.associations
                .retain(|(held_region, _), association| {
                    held_region != region || association.name != name
                });
        }
        removed
    }

    /// Remove ONE version of `region`'s document of that name, reporting whether that
    /// version was held.
    ///
    /// "The version of the document that you want to delete" (`DeleteDocument`'s
    /// `DocumentVersion`): a document that holds other versions keeps them, and one whose
    /// LAST version this was holds nothing and goes whole, associations with it.
    pub fn remove_document_version(&mut self, region: &str, name: &str, version: u64) -> bool {
        let Some(document) = self.document_mut(region, name) else {
            return false;
        };
        if !document.remove_version(version) {
            return false;
        }
        if document.versions.is_empty() {
            self.remove_document(region, name);
        }
        true
    }

    /// How many documents `region` holds, which is what the documented document quota
    /// is measured against.
    pub fn documents_in_region(&self, region: &str) -> usize {
        self.documents
            .keys()
            .filter(|(held, _)| held == region)
            .count()
    }

    /// Whether this store can take another document: the emulator's own total guard
    /// ([`MAX_DOCUMENTS_TOTAL`]), measured across EVERY Region.
    ///
    /// The Region a document is stored under comes from the request's own credential
    /// scope ([`request_region`]), so the published per-Region quota alone would be one
    /// quota per caller-chosen partition of the key space and the store could grow
    /// without bound. `CreateDocument` refuses a create past this total rather than
    /// persisting it.
    pub fn can_hold_another_document(&self) -> bool {
        self.documents.len() < MAX_DOCUMENTS_TOTAL
    }

    /// The parameter `name` names in `region`, or `None` when that Region holds none.
    pub fn parameter(&self, region: &str, name: &str) -> Option<&ParameterRecord> {
        self.parameters
            .get(&(region.to_string(), canonical_name(name).to_string()))
    }

    /// The same record, to append a version or replace its tags.
    pub fn parameter_mut(&mut self, region: &str, name: &str) -> Option<&mut ParameterRecord> {
        self.parameters
            .get_mut(&(region.to_string(), canonical_name(name).to_string()))
    }

    /// Store `record` as `region`'s parameter of that name.
    pub fn insert_parameter(&mut self, record: ParameterRecord) {
        let key = (
            record.region.clone(),
            canonical_name(&record.name).to_string(),
        );
        self.parameters.insert(key, record);
    }

    /// How many parameters `region` holds, which is what the per-Region quota and the
    /// `Intelligent-Tiering` evaluation are measured against.
    pub fn parameters_in_region(&self, region: &str) -> usize {
        self.parameters
            .keys()
            .filter(|(held, _)| held == region)
            .count()
    }

    /// Whether this store can take another parameter: the emulator's own total guard
    /// ([`MAX_PARAMETERS_TOTAL`]), measured across EVERY Region for the same reason
    /// [`ComAmazonawsSsmAmazonssmState::can_hold_another_document`] is. `PutParameter`
    /// refuses a create past it rather than persisting it.
    pub fn can_hold_another_parameter(&self) -> bool {
        self.parameters.len() < MAX_PARAMETERS_TOTAL
    }

    /// Every parameter `region` holds, in ascending canonical-name order, as
    /// `(canonical name, record)`. The order is what makes a hierarchy read's resume
    /// cursor total: it is a parameter name, not an offset into a list that may have
    /// changed between two pages.
    pub fn parameters_of_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a ParameterRecord)> + 'a {
        self.parameters
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, name), record)| (name.as_str(), record))
    }

    /// The managed node `id` names in `region`, or `None` when that Region has no node
    /// registered under it. The one lookup of a single node, so an operation that
    /// RESOLVES a node id does not reach into the store's key shape itself.
    pub fn managed_node(&self, region: &str, id: &str) -> Option<&ManagedNodeRecord> {
        self.managed_nodes
            .get(&(region.to_string(), id.to_string()))
    }

    /// The same node, to write it — its tags, say.
    pub fn managed_node_mut(&mut self, region: &str, id: &str) -> Option<&mut ManagedNodeRecord> {
        self.managed_nodes
            .get_mut(&(region.to_string(), id.to_string()))
    }

    /// Every managed node `region` holds, in ascending node-id order, as
    /// `(node id, record)`. The order is what makes a describe's resume cursor total: it
    /// is a node id, not an offset into a list that may have changed.
    pub fn managed_nodes_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a ManagedNodeRecord)> + 'a {
        self.managed_nodes
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, id), node)| (id.as_str(), node))
    }

    /// Whether this store can take another activation: the emulator's own total guard
    /// ([`MAX_ACTIVATIONS_TOTAL`]), measured across EVERY Region for the same reason
    /// [`ComAmazonawsSsmAmazonssmState::can_hold_another_document`] is.
    /// `CreateActivation` refuses a create past it rather than persisting it.
    pub fn can_hold_another_activation(&self) -> bool {
        self.activations.len() < MAX_ACTIVATIONS_TOTAL
    }

    /// Store `record` as `region`'s activation of that id.
    pub fn insert_activation(&mut self, record: ActivationRecord) {
        let key = (record.region.clone(), record.id.clone());
        self.activations.insert(key, record);
    }

    /// The activation `id` names in `region`, or `None` when that Region holds none.
    pub fn activation(&self, region: &str, id: &str) -> Option<&ActivationRecord> {
        self.activations.get(&(region.to_string(), id.to_string()))
    }

    /// Drop `region`'s activation of that id, answering whether it held one. The nodes it
    /// registered are untouched: "Deleting an activation doesn't de-register managed
    /// nodes" (`DeleteActivation`).
    pub fn remove_activation(&mut self, region: &str, id: &str) -> bool {
        self.activations
            .remove(&(region.to_string(), id.to_string()))
            .is_some()
    }

    /// Every activation `region` holds, in ascending activation-id order, as
    /// `(activation id, record)`. The order is what makes a report's resume cursor total:
    /// it is an activation id, not an offset into a list that may have changed between
    /// two pages.
    pub fn activations_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a ActivationRecord)> + 'a {
        self.activations
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, id), record)| (id.as_str(), record))
    }

    /// Store `node` as a managed node of `region`, keyed by the node id it reports.
    ///
    /// Answers `false` for a record reporting no node id: a managed node is identified
    /// by that id, so there is no key to store one without it.
    pub fn insert_managed_node(&mut self, region: &str, node: ManagedNodeRecord) -> bool {
        let Some(id) = node.node_id().map(str::to_string) else {
            return false;
        };
        self.managed_nodes.insert((region.to_string(), id), node);
        true
    }
}

/// `value` when a request set the member to something, and `None` when it set it to
/// nothing: an optional member sent EMPTY names no document version, no schedule, no node
/// and no association, the same way an omitted one does. The one reading of that across
/// this service's handlers.
pub fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// A modelled `TagList` as the store holds it: ordered, so two reads of one record
/// report its tags in the same order.
pub fn to_btree_map(tags: Option<Vec<Tag>>) -> BTreeMap<String, String> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect()
}

// ---------------------------------------------------------------------------
// Managed nodes
// ---------------------------------------------------------------------------

/// The filter keys a managed-node describe accepts beside the modelled
/// `InstancePropertyFilterKey` enum (`ActivationIds`, `AgentVersion`,
/// `AssociationStatus`, `DocumentName`, `IamRole`, `InstanceIds`, `PingStatus`,
/// `PlatformTypes`, `ResourceType`).
///
/// Both come from this service's own model: the enum is what
/// `InstancePropertyFilter$key` targets, and these two are the remaining keys the
/// sibling string filter over the same managed nodes publishes — "Valid filter key
/// values: ActivationIds | AgentVersion | AssociationStatus | IamRole | InstanceIds |
/// PingStatus | PlatformTypes | ResourceType | SourceIds | SourceTypes | "tag-key" |
/// "tag:{keyname}"" (`InstanceInformationStringFilter$Key`). `InstanceProperty`
/// reports a `SourceId` and a `SourceType` for every node, so both are properties this
/// store can answer a filter on.
pub const EXTRA_NODE_FILTER_KEYS: &[&str] = &["SourceIds", "SourceTypes"];

/// The filter key that matches a node carrying a tag of a given KEY, whatever its
/// value: "Valid tag examples: `Key=tag-key,Values=Purpose`" (same doc).
pub const TAG_KEY_FILTER_KEY: &str = "tag-key";

/// The prefix of the filter key that matches a node whose tag of that name has a given
/// value: "`Key=tag:Purpose,Values=Test`" (same doc).
pub const TAG_FILTER_KEY_PREFIX: &str = "tag:";

/// The filter operator a filter that names none is served with: the modelled
/// `InstancePropertyFilterOperator` values are `Equal`, `NotEqual`, `BeginWith`,
/// `LessThan` and `GreaterThan`, and `InstancePropertyFilter` — which carries no
/// operator at all — selects the nodes whose property EQUALS one of its values.
pub const OPERATOR_EQUAL: &str = "Equal";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_NOT_EQUAL: &str = "NotEqual";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_BEGIN_WITH: &str = "BeginWith";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_LESS_THAN: &str = "LessThan";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_GREATER_THAN: &str = "GreaterThan";

/// Whether `key` is a filter key a managed-node describe knows, which is what tells an
/// accepted filter from a refused one. The comparison is exact: a filter key is a name,
/// not a prose label, so neither a different spelling nor a different case names a
/// property of a node.
pub fn is_node_filter_key(key: &str) -> bool {
    INSTANCE_PROPERTY_FILTER_KEY_VALUES.contains(&key)
        || EXTRA_NODE_FILTER_KEYS.contains(&key)
        || key == TAG_KEY_FILTER_KEY
        || key
            .strip_prefix(TAG_FILTER_KEY_PREFIX)
            .is_some_and(|name| !name.is_empty())
}

/// Why `key` is not a filter key a managed-node describe accepts, as the service words
/// it: the key the caller submitted, then the member it filled — "`<Key>` filter Key is
/// invalid." The text is built from the request, so a caller reading it knows which
/// filter to fix.
pub fn filter_key_rejection(key: &str) -> String {
    format!("{key} filter Key is invalid.")
}

/// Every filter key a managed-node describe accepts, for a refusal or a report that
/// lists them. The tag form is listed as the doc writes it, with the tag name as a
/// placeholder.
pub fn node_filter_keys() -> Vec<&'static str> {
    INSTANCE_PROPERTY_FILTER_KEY_VALUES
        .iter()
        .chain(EXTRA_NODE_FILTER_KEYS.iter())
        .copied()
        .chain([TAG_KEY_FILTER_KEY, "tag:{keyname}"])
        .collect()
}

/// The values `key` accepts, for the filter keys whose value set this service's model
/// publishes, or `None` for a key whose values are free text (a node id, an agent
/// version, a role name, a tag value).
///
/// Each set is the modelled enum the property targets, LISTED in the order
/// `InstanceInformationStringFilter$Key` publishes it ("Valid values for the `PingStatus`
/// filter key: Online | ConnectionLost | Inactive (deprecated)", and so on for
/// `AssociationStatus`, `PlatformTypes`, `ResourceType` and `SourceType`) — which is the
/// order a refusal lists them in, so a caller reads the published list back. The generated
/// `*_VALUES` constants carry the same values sorted; the test below holds the two to the
/// same SET, so a model revision cannot leave this listing behind.
pub fn node_filter_key_value_set(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "AssociationStatus" => Some(&["Success", "Pending", "Failed"]),
        "PingStatus" => Some(&["Online", "ConnectionLost", "Inactive"]),
        "PlatformTypes" => Some(&["Windows", "Linux", "MacOS"]),
        "ResourceType" => Some(&["EC2Instance", "ManagedInstance"]),
        "SourceTypes" => Some(&[
            "AWS::EC2::Instance",
            "AWS::SSM::ManagedInstance",
            "AWS::IoT::Thing",
            "Microsoft.Compute/virtualMachines",
        ]),
        _ => None,
    }
}

/// How a managed-node describe refuses a filter whose VALUE is outside the set its key
/// publishes: the key the caller filtered on, then the values that key accepts, as
/// `InstanceInformationStringFilter$Key` publishes them
/// ([`node_filter_key_value_set`]).
///
/// The sentence names the KEY and the set, and quotes none of the values the request sent:
/// what a caller has to fix is which values that key may carry, which is the same answer
/// whichever of them was wrong and however many were. It is the same shape as
/// [`filter_key_rejection`], the refusal of a key the describe does not know at all.
///
/// RECORDED BEHAVIOUR (corpus `simparity-wire-aws-v1`, fixture
/// `ssm-w22n-traj-instance-properties-route`, step `task#2`): a `DescribeInstanceProperties`
/// filtering on `PlatformTypes` with a value outside the published set is answered 400
/// `InvalidInstancePropertyFilterValue` with
/// `PlatformTypes filter is invalid, valid values are Windows, Linux, MacOS` — the key, then
/// the set in the order the doc above publishes it, and no value echoed. The same sentence
/// serves the sibling describe, which reads the same catalog for the same keys.
pub fn node_filter_value_set_rejection(key: &str, published: &[&str]) -> String {
    format!(
        "{key} filter is invalid, valid values are {}",
        published.join(", ")
    )
}

/// Whether `id` is shaped like a managed-node id, from the modelled `InstanceId`
/// pattern `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`: an EC2 instance id with an 8- or
/// 17-character suffix, or a 17-character hybrid-activation (`mi-`) id.
pub fn managed_node_id_is_well_formed(id: &str) -> bool {
    let is_word = |rest: &str| rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if let Some(rest) = id.strip_prefix("i-") {
        return (rest.len() == 8 || rest.len() == 17) && is_word(rest);
    }
    if let Some(rest) = id.strip_prefix("mi-") {
        return rest.len() == 17 && is_word(rest);
    }
    false
}

/// The regular expression a managed-node id has to satisfy, as a refusal reports it: the
/// alternation the modelled `InstanceId` pattern `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`
/// is, without the outer anchors the model wraps it in — each branch anchors itself, so
/// the alternation is the whole constraint. [`managed_node_id_is_well_formed`] decides the
/// same expression by hand.
pub const MANAGED_NODE_ID_PATTERN: &str = r"(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)";

/// The input member a managed-node id fills, as a refusal names it. The `InstanceId`
/// shape is ONE identifier space across this service, so a value that is not one of its
/// ids is reported at that member whichever filter or list carried it.
pub const MANAGED_NODE_ID_MEMBER: &str = "instanceId";

/// How a value that violates a modelled regular-expression constraint is reported: the
/// member the value filled and the expression the model constrains that member with, in
/// the uniform wording a constraint violation carries.
///
/// The text is DERIVED from the constraint rather than written out as prose, because that
/// is what makes it readable by something other than a human: a client, or an SDK-level
/// validator, extracts the member path and the pattern from it to report which input to
/// fix and what it has to satisfy.
///
/// The requirement clause is the one the framework's boundary validator renders for the
/// same violation (`smithy_schema_server::constraints`, `Violation::message`), which this
/// cannot call: that form echoes the offending value ("Value '<v>' at '<member>' …"),
/// while a managed-node filter's refusal states only the member and the pattern it had to
/// satisfy.
pub fn pattern_constraint_violation(member: &str, pattern: &str) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {pattern}"
    )
}

/// The refusal a value that is not shaped like a managed-node id carries — the violated
/// constraint, built from [`MANAGED_NODE_ID_MEMBER`] and [`MANAGED_NODE_ID_PATTERN`] — so
/// every operation that reads a node id reports the same violation of the same
/// constraint.
pub fn managed_node_id_constraint_violation() -> String {
    pattern_constraint_violation(MANAGED_NODE_ID_MEMBER, MANAGED_NODE_ID_PATTERN)
}

/// How a managed-node describe refuses a request that names BOTH its LEGACY filter list and
/// its current filter member — `InstancePropertyFilterList` beside `FiltersWithOperator`, or
/// `InstanceInformationFilterList` beside `Filters`.
///
/// The two members are alternatives rather than a union ("Attempting to use
/// InstanceInformationFilterList and Filters leads to an exception error"), and the sentence
/// names NEITHER of them: it states that the legacy and the current filter cannot be used
/// together, which is the same answer whichever describe was called and whatever the two
/// lists held.
///
/// RECORDED BEHAVIOUR (corpus `simparity-wire-aws-v1`, fixture
/// `ssm-w22n-traj-instance-properties-route`, step `task#3`): a `DescribeInstanceProperties`
/// naming both `InstancePropertyFilterList` and `FiltersWithOperator` is answered 400
/// `ValidationException` with exactly this sentence. It carries nothing of the request, so
/// the sibling describe — whose doc states the rule — states the same one.
pub const LEGACY_AND_NEW_FILTERS_REJECTION: &str =
    "Cannot use both legacy and new filter simultaneously";

/// The refusal a request that mixes filter kinds carries, as the service words it: one
/// sentence about the KINDS, carrying none of the request's keys — a tag filter and an
/// inventory filter each select by something other than a property of the node, so neither
/// can narrow the same query a filter on a property does.
pub const MIXED_FILTER_KINDS_REJECTION: &str =
    "Cannot mix tag or inventory filter with any other filter.";

/// Why the filter keys of a managed-node describe cannot hold together, or `None` when
/// they can.
///
/// "Tag filters can't be combined with other filter types"
/// (`DescribeInstanceInformationRequest$Filters`): a filter on a tag and a filter on a
/// property of a node cannot be ANDed in one request, while two tag filters, and two
/// property filters, can — the rule is about which filter KINDS may be combined. Every
/// key reaching this is a key the operation accepts, so what it refuses is the
/// COMBINATION of two recognised keys, which is a validation of the request rather than an
/// unknown key.
pub fn tag_filter_combination_rejection<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    let mut tags = 0usize;
    let mut properties = 0usize;
    for key in keys {
        if is_tag_filter_key(key) {
            tags += 1;
        } else {
            properties += 1;
        }
    }
    (tags != 0 && properties != 0).then_some(MIXED_FILTER_KINDS_REJECTION)
}

/// How `LessThan` and `GreaterThan` order two filter values.
///
/// Both operators are declared for a filter over properties that are mostly dotted
/// numbers (`AgentVersion` is modelled as `^[0-9]{1,6}(\.[0-9]{1,6}){2,3}$`), so two
/// values whose every dot-separated segment is a number are ordered segment by segment
/// as numbers — `3.0.1` is above `3.0.10` nowhere, unlike a text comparison. Anything
/// else is ordered as text.
pub fn compare_filter_values(left: &str, right: &str) -> Ordering {
    let segments = |value: &str| {
        value
            .split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<u64>, _>>()
            .ok()
    };
    match (segments(left), segments(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// Whether a node's `held` value satisfies `operator` against a filter's `wanted`
/// value.
pub fn operator_matches(operator: &str, held: &str, wanted: &str) -> bool {
    match operator {
        OPERATOR_NOT_EQUAL => held != wanted,
        OPERATOR_BEGIN_WITH => held.starts_with(wanted),
        OPERATOR_LESS_THAN => compare_filter_values(held, wanted) == Ordering::Less,
        OPERATOR_GREATER_THAN => compare_filter_values(held, wanted) == Ordering::Greater,
        // `Equal`, and the operator an `InstancePropertyFilter` does not carry.
        _ => held == wanted,
    }
}

/// One managed node, as the node describes report it.
///
/// `properties` is the modelled `InstanceProperty` itself, so a describe reports what
/// was persisted rather than a second copy of it; `tags` is what the `tag-key` and
/// `tag:<name>` filter keys match against, which no response member carries;
/// `is_latest_version` is the one member `InstanceInformation` reports that
/// `InstanceProperty` has no place for, so the record holds it beside them rather than
/// in a second node store.
#[derive(Debug, Clone, Default)]
pub struct ManagedNodeRecord {
    /// Everything a describe reports about this node.
    pub properties: InstanceProperty,
    /// The node's tags, keyed by tag key.
    pub tags: BTreeMap<String, String>,
    /// Whether the node runs the latest SSM Agent, as the agent last reported
    /// (`InstanceInformation$IsLatestVersion`). `None` for a node that has reported no
    /// agent version, rather than a guess either way.
    pub is_latest_version: Option<bool>,
}

impl ManagedNodeRecord {
    /// The node id this record is stored under, from the properties it reports.
    pub fn node_id(&self) -> Option<&str> {
        self.properties.instance_id.as_deref()
    }

    /// The values of this node that a filter on `key` is matched against, or `None`
    /// when `key` is not a filter key at all ([`is_node_filter_key`]).
    ///
    /// A node the store holds no value of that property for answers with an EMPTY list
    /// and so is reported by no filter on it, whatever the operator: a filter selects
    /// the nodes whose property satisfies it, and a node without the property has
    /// nothing to satisfy it with.
    pub fn filter_values(&self, key: &str) -> Option<Vec<&str>> {
        fn one(value: &Option<String>) -> Option<Vec<&str>> {
            Some(value.as_deref().into_iter().collect())
        }
        match key {
            "ActivationIds" => one(&self.properties.activation_id),
            "AgentVersion" => one(&self.properties.agent_version),
            "AssociationStatus" => one(&self.properties.association_status),
            "IamRole" => one(&self.properties.iam_role),
            "InstanceIds" => one(&self.properties.instance_id),
            "PingStatus" => one(&self.properties.ping_status),
            "PlatformTypes" => one(&self.properties.platform_type),
            "ResourceType" => one(&self.properties.resource_type),
            "SourceIds" => one(&self.properties.source_id),
            "SourceTypes" => one(&self.properties.source_type),
            // `DocumentName` selects the nodes an association of that SSM document
            // targets. Associations are a separate resource of this service and no
            // operation that creates one is implemented yet, so no node in this store
            // is the target of any document and a filter on it selects nothing — which
            // is the store's answer, not a refusal.
            "DocumentName" => Some(Vec::new()),
            TAG_KEY_FILTER_KEY => Some(self.tags.keys().map(String::as_str).collect()),
            _ => match key.strip_prefix(TAG_FILTER_KEY_PREFIX) {
                Some(name) if !name.is_empty() => {
                    Some(self.tags.get(name).map(String::as_str).into_iter().collect())
                }
                _ => None,
            },
        }
    }

    /// Whether this node satisfies one filter: any value it holds for `key` against any
    /// of the filter's `values`, under `operator`.
    pub fn matches_filter(&self, key: &str, values: &[String], operator: &str) -> bool {
        let Some(held) = self.filter_values(key) else {
            return false;
        };
        held.iter().any(|held| {
            values
                .iter()
                .any(|wanted| operator_matches(operator, held, wanted))
        })
    }

    /// A node registering now: the id it registered under and which kind of node it is
    /// (a `ResourceType` value).
    ///
    /// A node that has just completed registration pinged the service to do it, so it
    /// starts `Online` with that ping and that registration date recorded; every other
    /// property is left for whatever registered it to fill in.
    pub fn registering(
        instance_id: impl Into<String>,
        resource_type: impl Into<String>,
        at: SystemTime,
    ) -> Self {
        let at = ::aws_smithy_types::DateTime::from(at);
        Self {
            properties: InstanceProperty {
                instance_id: Some(instance_id.into()),
                resource_type: Some(resource_type.into()),
                ping_status: Some(PING_STATUS_ONLINE.to_string()),
                last_ping_date_time: Some(at),
                registration_date: Some(at),
                ..InstanceProperty::default()
            },
            tags: BTreeMap::new(),
            is_latest_version: None,
        }
    }

    /// The modelled `InstanceInformation` for this node — the same stored node
    /// `properties` reports, through the members `DescribeInstanceInformation` answers
    /// with.
    ///
    /// Every member is copied from what was persisted; the ones `InstanceProperty` does
    /// not carry (`IsLatestVersion`) come from the record's own field, and the ones
    /// neither carries are left unset rather than invented.
    pub fn to_instance_information(&self) -> InstanceInformation {
        let held = &self.properties;
        InstanceInformation {
            instance_id: held.instance_id.clone(),
            ping_status: held.ping_status.clone(),
            last_ping_date_time: held.last_ping_date_time,
            agent_version: held.agent_version.clone(),
            is_latest_version: self.is_latest_version,
            platform_type: held.platform_type.clone(),
            platform_name: held.platform_name.clone(),
            platform_version: held.platform_version.clone(),
            activation_id: held.activation_id.clone(),
            iam_role: held.iam_role.clone(),
            registration_date: held.registration_date,
            resource_type: held.resource_type.clone(),
            name: held.name.clone(),
            ip_address: held.ip_address.clone(),
            computer_name: held.computer_name.clone(),
            association_status: held.association_status.clone(),
            last_association_execution_date: held.last_association_execution_date,
            last_successful_association_execution_date: held
                .last_successful_association_execution_date,
            association_overview: held.association_overview.clone(),
            source_id: held.source_id.clone(),
            source_type: held.source_type.clone(),
            source_location: held.source_location.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Hybrid activations
// ---------------------------------------------------------------------------

/// How long an activation lasts when the request names no expiry: "If you don't provide
/// an expiration date, the activation code expires in 24 hours"
/// (`CreateActivationRequest$ExpirationDate`).
pub const DEFAULT_ACTIVATION_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// How many nodes an activation may register when the request names no limit: "The
/// default value is 1" (`CreateActivationRequest$RegistrationLimit`).
pub const DEFAULT_REGISTRATION_LIMIT: i32 = 1;

/// How many nodes have registered with an activation that has just been created: none
/// has used it yet, and `RegistrationsCount` is "the number of managed nodes already
/// registered with this activation".
pub const INITIAL_REGISTRATIONS_COUNT: i32 = 0;

/// The bounds the modelled `MaxResults` of an activation report carries (range 1..50).
pub const MIN_ACTIVATION_PAGE_SIZE: i32 = 1;

/// See [`MIN_ACTIVATION_PAGE_SIZE`]. It is also the page size a request that names no
/// `MaxResults` gets, the way every other read of this service serves one (see
/// [`page_size`]).
pub const MAX_ACTIVATION_PAGE_SIZE: i32 = 50;

/// The page size an activation report serves (see [`page_size`]).
pub fn activation_page_size(max_results: Option<i32>) -> Result<usize, String> {
    page_size(
        max_results,
        MIN_ACTIVATION_PAGE_SIZE,
        MAX_ACTIVATION_PAGE_SIZE,
    )
}

/// How many activations this store holds in TOTAL, across every Region.
///
/// Emulator resource guard, NOT a published AWS quota, and the bound that keeps this
/// process finite: the Region an activation is stored under comes from the request's own
/// credential scope ([`request_region`]), so a per-Region bound would be one bound per
/// caller-chosen partition of the key space and the store could grow without bound. A
/// create past it is refused rather than persisted.
pub const MAX_ACTIVATIONS_TOTAL: usize = 10_000;

/// How many characters an activation code carries, which is the form Systems Manager
/// mints one in: 20 characters of the base64 alphabet.
pub const ACTIVATION_CODE_LEN: usize = 20;

/// How many bytes an activation code renders as its [`ACTIVATION_CODE_LEN`] characters:
/// base64 carries three bytes in every four characters.
pub const ACTIVATION_CODE_BYTES: usize = ACTIVATION_CODE_LEN / 4 * 3;

/// A fresh v4 identifier in the canonical lowercase hyphenated form: the tree's own
/// identifier draw ([`::emulator_telemetry`]'s uuid-backed id newtype), so this crate
/// names ONE random source and every id it mints is drawn the same way.
pub fn new_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// A fresh document id: the identifier Systems Manager assigns an SSM document when it is
/// created, which every read of that document reports as `DocumentId`.
///
/// Drawn once per document and PERSISTED with it ([`DocumentRecord::document_id`]), so two
/// reads of one document report the same id, no two documents share one, and nothing of
/// the request is in it. The draw is the crate's one identifier source ([`new_uuid`]).
pub fn new_document_id() -> String {
    new_uuid()
}

/// A fresh activation id: "The ID number generated by the system when it processed the
/// activation", in the hyphenated form the modelled `ActivationId` pattern is — which
/// [`is_activation_id`] is the one reading of.
///
/// The draw is the crate's one identifier source ([`new_uuid`]).
pub fn new_activation_id() -> String {
    new_uuid()
}

/// A fresh activation code: "The code the system generates when it processes the
/// activation. The activation code functions like a password to validate the activation
/// ID" (`CreateActivationResult$ActivationCode`).
///
/// It is a SECRET the caller installs on the machine it registers, so it is opaque
/// entropy and nothing about the request or the activation can be read back out of it.
/// Systems Manager hands it out as [`ACTIVATION_CODE_LEN`] characters of the base64
/// alphabet, which is [`ACTIVATION_CODE_BYTES`] bytes rendered base64 without padding —
/// inside the modelled `ActivationCode` length of 20..250.
///
/// The bytes are the leading ones of a digest over two freshly drawn v4 identifiers: the
/// digest is what makes every bit of the code uniform, rather than carrying the version
/// and variant bits a v4 identifier fixes, and the draws come from the crate's one
/// identifier source ([`new_uuid`]).
pub fn new_activation_code() -> String {
    use ::sha2::Digest;

    let mut entropy = ::sha2::Sha256::new();
    entropy.update(new_uuid());
    entropy.update(new_uuid());
    let digest = entropy.finalize();
    ::aws_smithy_types::base64::encode(&digest[..ACTIVATION_CODE_BYTES])
}

/// Whether `key` is a filter key an activation report knows (the modelled
/// `DescribeActivationsFilterKeys` enum), which is what tells an accepted filter from a
/// refused one. The comparison is exact: a filter key is a name, not a prose label.
pub fn is_activation_filter_key(key: &str) -> bool {
    DESCRIBE_ACTIVATIONS_FILTER_KEYS_VALUES.contains(&key)
}

/// How this service states a filter an activation report cannot serve. `InvalidFilter`'s
/// own documentation is "The filter name isn't valid. Verify that you entered the correct
/// name and try again."
pub const INVALID_ACTIVATION_FILTER: &str =
    "The filter name isn't valid. Verify that you entered the correct name and try again.";

/// One hybrid activation, as `CreateActivation` persisted it and `DescribeActivations`
/// reports it.
///
/// Everything a read reports is held here, so the report is the activation that was
/// created rather than a second copy of the request. [`ActivationRecord::code`] is the
/// one field no read reports: the code is the activation's password, handed to its
/// creator once.
#[derive(Debug, Clone)]
pub struct ActivationRecord {
    /// The id the create minted; part of the store key and what a read reports.
    pub id: String,
    /// The Region the activation was created in, which is the other half of its key: an
    /// activation registers nodes with the endpoint it was created at.
    pub region: String,
    /// The activation's password, returned once by the create. No read reports it.
    pub code: String,
    pub description: Option<String>,
    pub default_instance_name: Option<String>,
    /// "The Identity and Access Management (IAM) role to assign to the managed node"
    /// (`Activation$IamRole`) — the role name the create required.
    pub iam_role: String,
    /// "The maximum number of managed nodes that can be registered using this
    /// activation".
    pub registration_limit: i32,
    /// "The number of managed nodes already registered with this activation": a count of
    /// the registrations this store holds for it, which starts at none.
    pub registrations_count: i32,
    /// "The date when this activation can no longer be used to register managed nodes",
    /// which is what makes it expired once the clock passes it.
    pub expiration_date: SystemTime,
    pub created_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// `RegistrationMetadata`, which the model documents as "Reserved for internal use":
    /// persisted as supplied, and reported by no member of any response.
    pub registration_metadata: BTreeMap<String, String>,
}

impl ActivationRecord {
    /// Whether this activation can no longer register a node at `now`, which is
    /// `Expired`: the clock has reached the date the activation ends at.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expiration_date
    }

    /// The modelled `Activation` for this record, as of `now`.
    ///
    /// Every member is what was persisted; `Expired` is derived from the stored
    /// expiration date and the clock, so a read reports the activation as expired once
    /// its date has passed whether or not anybody called in between.
    pub fn to_activation(&self, now: SystemTime) -> Activation {
        Activation {
            activation_id: Some(self.id.clone()),
            description: self.description.clone(),
            default_instance_name: self.default_instance_name.clone(),
            iam_role: Some(self.iam_role.clone()),
            registration_limit: Some(self.registration_limit),
            registrations_count: Some(self.registrations_count),
            expiration_date: Some(::aws_smithy_types::DateTime::from(self.expiration_date)),
            expired: self.is_expired(now),
            created_date: Some(::aws_smithy_types::DateTime::from(self.created_date)),
            tags: (!self.tags.is_empty()).then(|| tagging::to_tag_list(&self.tags)),
        }
    }

    /// The values of this activation that a filter on `key` is matched against, or `None`
    /// when `key` is not a filter key of an activation report
    /// ([`is_activation_filter_key`]).
    ///
    /// An activation the store holds no value of that property for answers with an EMPTY
    /// list and so is reported by no filter on it: a filter selects the activations whose
    /// property satisfies it, and one without the property has nothing to satisfy it with.
    pub fn filter_values(&self, key: &str) -> Option<Vec<&str>> {
        match key {
            "ActivationIds" => Some(vec![self.id.as_str()]),
            "DefaultInstanceName" => {
                Some(self.default_instance_name.as_deref().into_iter().collect())
            }
            "IamRole" => Some(vec![self.iam_role.as_str()]),
            _ => None,
        }
    }

    /// Whether this activation satisfies one filter: any value it holds for `key` equals
    /// any of the filter's `values`.
    ///
    /// A `DescribeActivationsFilter` carries no operator, so every filter compares whole
    /// values for equality ([`OPERATOR_EQUAL`]).
    pub fn matches_filter(&self, key: &str, values: &[String]) -> bool {
        let Some(held) = self.filter_values(key) else {
            return false;
        };
        held.iter().any(|held| {
            values
                .iter()
                .any(|wanted| operator_matches(OPERATOR_EQUAL, held, wanted))
        })
    }
}

/// This service's ONE minter of an opaque, UUID-shaped identifier: the Sha256 of an
/// instant and a mint counter, rendered as a UUID.
///
/// Every id this service mints for a resource it creates goes through here — a
/// `CommandId` ([`ComAmazonawsSsmAmazonssmState::mint_command_id`]), a resource policy's
/// `PolicyId` ([`resource_policies::ResourcePolicyStore`]) — so the digest and the
/// layout are written once. The 16 bytes are the digest's first half and the UUID LAYOUT
/// (the group boundaries and the version and variant bits) is `service_common`'s one
/// rendering of 16 opaque bytes, so neither is restated per caller.
///
/// `seed_nanos` is an instant of the store's own (its creation, or now) and `counter` the
/// number of ids that minter has handed out, so two ids of one run differ, consecutive
/// ones are unrelated, and the value is not a count a caller can read off the wire.
/// Nothing of the request is in it.
pub fn minted_uuid(seed_nanos: u128, counter: u64) -> String {
    use ::sha2::Digest;

    let mut digest = ::sha2::Sha256::new();
    digest.update(seed_nanos.to_be_bytes());
    digest.update(counter.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    ::service_common::uuid_from_random_bytes(bytes)
}

/// This service's ONE minter of an opaque identifier of the form `<prefix><hex digits>`:
/// the Sha256 of a mint counter and an instant, rendered as `digits` lowercase hex digits
/// behind `prefix`.
///
/// Every id this service mints in that form goes through here — an `OpsItemId`
/// ([`ops_items::mint_ops_item_id`]), a `BaselineId`
/// ([`patch_baselines::mint_baseline_id`]) — so the digest and the rendering are written
/// once and each caller states only the prefix and the digit count its modelled pattern
/// requires.
///
/// `sequence` is the number of ids that minter has handed out and `at` an instant of the
/// store's own, so two ids of one store differ, consecutive ones are unrelated, and the
/// value is not a count a caller can read off the wire. Nothing of the request is in it.
pub fn minted_prefixed_hex_id(prefix: &str, digits: usize, sequence: u64, at: SystemTime) -> String {
    use ::sha2::Digest;

    let nanos = at
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let mut rendered = ::hex::encode(hasher.finalize());
    rendered.truncate(digits);
    format!("{prefix}{rendered}")
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN and in a
/// store key, so anything outside `[A-Za-z0-9-]` is refused here and the default used
/// instead: a generated ARN can never carry text an ARN may not hold.
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

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmAmazonssmState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmAmazonssmState>)
}
