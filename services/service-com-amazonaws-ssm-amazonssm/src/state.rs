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
//! account and Region, which resolve by name without anybody creating them, are
//! owned by Amazon and are dated by the day Amazon published each of them. A read goes
//! through [`ComAmazonawsSsmAmazonssmState::resolve_document`],
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
//! What a patching operation REPORTED about a node lives beside them
//! ([`instance_patch_states`]), keyed by `(region, node id)`: the high-level patch
//! compliance state of one node, which `DescribeInstancePatchStatesForPatchGroup` reports
//! for the nodes of one patch group, narrowed by the count filters that read accepts
//! ([`instance_patch_states::INSTANCE_PATCH_STATE_FILTER_KEYS`]). Patch compliance is
//! gathered on the node by the patching operation Patch Manager runs there, and no call of
//! this service's model reports one, so a group whose nodes have never been patched is
//! reported as holding no state rather than compliance nobody collected.
//!
//! Beside the baselines is the **patch metadata Patch Manager publishes**
//! ([`patch_properties`]): the products, product families, classifications and severities
//! the patches available for an operating system carry, which exist in every account and
//! Region without anybody creating them and which `DescribePatchProperties` reports
//! ([`ComAmazonawsSsmAmazonssmState::patch_property_values`]) — so what that read answers
//! is a projection of the one catalogue, the same for every caller. The PATCHES that
//! metadata describes are the same catalogue one patch at a time ([`patch_catalog`]): one
//! patch per published product per monthly release, which is what
//! `DescribeEffectivePatchesForPatchBaseline` pairs with the approval state a baseline's
//! own stored filters, rules and patch lists give each of them. The same catalogue is what
//! `DescribeAvailablePatches` reports the patches "eligible to be included in a patch
//! baseline" from ([`available_patches`]), narrowed by the patch properties its own filters
//! search on ([`ComAmazonawsSsmAmazonssmState::available_patches`]).
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
//! instants it opens at are the ones that cron or rate expression names inside the
//! interval the window is active over
//! ([`maintenance_windows::MaintenanceWindowRecord::scheduled_executions`]), the next
//! execution being the first of them
//! ([`maintenance_windows::MaintenanceWindowRecord::next_execution_time`]) and a report of
//! the window's upcoming executions the series itself. The **targets**
//! a window's tasks run against ([`maintenance_window_targets`]) hang off the same key
//! space, keyed by `(region, window target id)`:
//! `RegisterTargetWithMaintenanceWindow` mints one against a window the Region holds,
//! `DescribeMaintenanceWindowTargets` reports that window's own, and
//! `DeregisterTargetFromMaintenanceWindow` removes it. The **tasks** that window runs
//! ([`maintenance_window_tasks`]) hang off the same key space, keyed by
//! `(region, window task id)`: `RegisterTaskWithMaintenanceWindow` mints one against a window
//! the Region holds and `GetMaintenanceWindowTask` reports it, and a task naming one of that
//! window's registered targets by id is the reference that makes a safe deregistration of
//! that target one the service refuses.
//!
//! What a window's RUNS are is a third layer on the same key space: one record per run
//! ([`maintenance_window_executions`]), one per task that run reached
//! ([`maintenance_window_execution_tasks`]) and one per target such a task execution ran
//! against ([`maintenance_window_execution_task_invocations`]), each reached by the ids the
//! layer above it was reached by. `GetMaintenanceWindowExecution`,
//! `GetMaintenanceWindowExecutionTask` and `GetMaintenanceWindowExecutionTaskInvocation`
//! read one record of each layer and `DescribeMaintenanceWindowExecutions`,
//! `DescribeMaintenanceWindowExecutionTasks` and
//! `DescribeMaintenanceWindowExecutionTaskInvocations` list the layer below one — a window's
//! runs, a run's task executions, a task execution's invocations — all through
//! the one store below. What puts a record in any of them — and what does not — is stated in
//! the owning modules.
//!
//! Then the **execution previews** ([`execution_previews`]), keyed by
//! `(region, execution preview id)`: `StartExecutionPreview` works out what running an
//! Automation runbook would do — from the steps that runbook declares and the resources its
//! targets resolve to in this store — and persists it, and `GetExecutionPreview` reports
//! what it persisted once the service has had time to work it out. Nothing is run and no
//! resource is touched: a preview is the report of what an execution WOULD do.
//!
//! Then the **OpsMetadata objects** Application Manager holds ([`ops_metadata`]), keyed by
//! `(region, OpsMetadata ARN)`: `CreateOpsMetadata` registers one for an application,
//! `GetOpsMetadata` reports the metadata it holds and `DeleteOpsMetadata` removes it. The
//! ARN is minted by the create, so an ARN nothing created names no object — and one naming
//! another account names no object of this requester's at all.
//!
//! Beside the node inventory is the **custom inventory** a caller writes
//! ([`custom_inventory`]), keyed by `(region, node id, type name)`: `PutInventory` writes one
//! item per custom type per node and `DeleteInventory` removes the items of a type, which is
//! what makes the custom types a Region holds exactly the types its items are written under.
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

/// Whether the IAM role a hybrid activation is handed is one this service can act as,
/// which is what a create of an activation resolves before it mints one.
pub mod activation_role;

/// The cloud connectors this service holds, and how a create, an update and a read of
/// one reach the single store below.
pub mod cloud_connectors;

/// Whether a cloud connector can reach the cloud environment it targets: the cross-cloud
/// federation its role has to obtain, and the findings a validation of one reports.
pub mod cloud_connector_validation;

/// The provider credential a cloud connector acts with, and whether this account holds a
/// verified credential for the tenant and the application its configuration names.
pub mod provider_credentials;

/// Rendering a persisted document's content in the `DocumentFormat` a read asks for.
pub mod document_format;

/// Writing a document tree back out as the YAML a format conversion answers with.
pub mod document_yaml;

/// The Change Calendar entry a document of that type carries, and the state it leaves a
/// calendar in at one instant.
pub mod change_calendar;

/// Which operating-system platforms a document's own content leaves it compatible with.
pub mod document_platforms;

/// Which documents a listing of this account's SSM documents walks, the filters that
/// narrow it, and the identity each one is reported by.
pub mod document_listing;

/// Which versions of ONE SSM document a listing of them walks, and the identity each
/// version is reported by.
pub mod document_version_listing;

/// Which accounts an SSM document is shared with, and at which version of it.
pub mod document_permissions;

/// Whether a read may have a `SecureString`'s value decrypted for its caller, which is a
/// decision the KMS key the value is sealed under makes rather than this service.
pub mod secure_string;

/// Serving the retrieval of a parameter another AWS service makes on a ROLE's behalf, for the
/// workload it runs — the decision that service's launch of that workload turns on.
pub mod parameter_retrieval;

/// The parameters AWS itself publishes in the `/aws` hierarchy of every Region, which a
/// read resolves beside the parameters this account wrote.
pub mod public_parameters;

/// The EventBridge event a write to Parameter Store publishes: the create, the update and
/// the delete of a parameter, each announced on the account's default event bus.
pub mod parameter_events;

/// The AWS Global Infrastructure reference data published in that hierarchy: the Regions,
/// their long names, the partition each belongs to and the domain its endpoints are served
/// from.
pub mod global_infrastructure;

/// The reserved hierarchy of that same `/aws` namespace through which a read of Parameter
/// Store serves an Amazon Web Services Secrets Manager secret.
pub mod secretsmanager_reference;

/// The opaque continuation tokens Parameter Store's paginated reads hand out, and how each
/// of them tells a token it ISSUED from one a caller assembled.
pub mod parameter_pagination;

/// A command a caller sent to the managed nodes of one Region, and the invocation each of
/// those nodes holds of it.
pub mod commands;

/// The delivery a sent command still owes a node whose agent has not picked it up yet, and
/// the settle that performs it once that node's guest is up.
pub mod command_deliveries;

/// An automation execution a caller started, the step executions it is made of, and the
/// walk through them the clock owes.
pub mod automation;

/// The execution previews of an Automation runbook: what a preview works out from the
/// runbook and the targets, and the status the clock moves it to.
pub mod execution_previews;

/// The State Manager associations this service holds, each with the versions a create and
/// its updates wrote.
pub mod associations;

/// What creating ONE association resolves, refuses and persists — shared by the operation
/// that creates one and the one that creates a batch of them.
pub mod association_create;

/// The executions of a State Manager association: what the runs an association has had
/// report, and what the nodes one of those runs reached report.
pub mod association_executions;

/// Listing the versions of ONE association: the identity each version is reported by, and
/// the page a version listing is served.
pub mod association_version_listing;

/// The resource policies attached to this account's Systems Manager resources, and the
/// hash that keeps two writers from overwriting each other's.
pub mod resource_policies;

/// The OpsItems OpsCenter holds: the record a create persists, the rules it is held to
/// and the store the reads project.
pub mod ops_items;

/// Which of the OpsItems a Region holds a `DescribeOpsItems` query selects, and the order
/// it reports them in.
pub mod ops_item_queries;

/// The operations metadata (OpsData) this service reports a summary of: the data types it
/// publishes, the attributes each one carries, and the projection of the resources it holds
/// into the items a summary filters, counts and pages.
pub mod ops_data;

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

/// Which patch baseline a PATCH GROUP is patched against: the registration a register
/// persists, and the baseline in force for a node of that group.
pub mod patch_groups;

/// The high-level patch compliance state a patching operation reported for a managed
/// node, and how a patch-group read narrows, orders and pages those reports.
pub mod instance_patch_states;

/// The patch metadata Patch Manager publishes for an operating system — the products,
/// product families, classifications and severities its available patches carry — which
/// `DescribePatchProperties` reports.
pub mod patch_properties;

/// The individual patches Patch Manager publishes for an operating system, and which of
/// them one patch baseline makes effective, with the approval state that baseline gives
/// each of them.
pub mod patch_catalog;

/// Which `PatchFilter` keys a baseline of one operating system may select on — the patch
/// properties that operating system's own patches carry, which every write of a baseline is
/// held to.
pub mod patch_filter_keys;

/// Which of the published patches are AVAILABLE to be put in a patch baseline — the
/// catalogues an available-patch read walks and the filters it narrows them by.
pub mod available_patches;

/// The inventory TYPES Systems Manager publishes — the catalog a schema read reports — and
/// the naming rule a type name has to satisfy to name an inventory type at all.
pub mod inventory_catalog;

/// The inventory this service holds about the managed nodes of a Region — the
/// `AWS:InstanceInformation` item every registered node has — and how a query filters and
/// aggregates it.
pub mod inventory;

/// The CUSTOM inventory a caller writes about its nodes, the types those items register, and
/// the deletions that remove them again.
pub mod custom_inventory;

/// The per-patch compliance data a patching operation reported for one node, and the filter
/// keys and page a read of it accepts.
pub mod instance_patches;

/// The compliance items a managed node holds — the projection of the patch rows a patching
/// operation left and the items a caller's own `PutComplianceItems` wrote, which
/// `ListComplianceItems` reports.
pub mod compliance_items;

/// The roll-up of those items by compliance type, which `ListComplianceSummaries` answers
/// with.
pub mod compliance_summaries;

/// The roll-up of the same items by the RESOURCE each is about, which
/// `ListResourceComplianceSummaries` answers with.
pub mod resource_compliance_summaries;

/// The patch-baseline run Patch Manager performs on a managed node when the document it
/// publishes for that is sent to one, and the compliance that run reports.
pub mod patch_runs;

/// The patch SNAPSHOT a managed node deploys its patch baseline from: what one holds, where
/// this emulator serves it, and what a node's `Product` is.
pub mod patch_snapshots;

/// Whether a request was made BY the managed node it names — the question the calls a node's
/// own agent makes ask about their caller.
pub mod node_identity;

/// The patching a State Manager association's run brings back: the compliance the
/// patch-baseline document reports for a node one run has applied it on.
pub mod association_patch_runs;

/// The OpsMetadata objects Application Manager holds for this account: what a create
/// persists, the ARN it is reached by, and the two ways an ARN can name none.
pub mod ops_metadata;

/// Which of the account's Amazon EC2 instances are registered with Systems Manager as
/// managed nodes, and the node record that registration writes.
pub mod node_registration;

/// The operating system inside a managed node's guest, as the node reports it: the
/// `PlatformName` and `PlatformVersion` of the image the instance was launched from.
pub mod node_platform;

/// The SSM Agent builds this emulator publishes, and which of them the guest of a managed
/// node holds — the pair `InstanceInformation$AgentVersion` and
/// `InstanceInformation$IsLatestVersion` are answered from.
pub mod agent_releases;

/// The node listing of the Systems Manager integrated experience: whether an account has it
/// at all, which Regions' nodes one listing reports, and the `Node` a stored managed node is
/// projected as.
pub mod node_listing;

/// The node SUMMARY of the same experience: the counts `ListNodesSummary` groups those nodes
/// into, and the aggregations it can make.
pub mod node_summary;

/// The Session Manager sessions this endpoint holds: the record a start persists, what
/// ends one, and how a report filters and pages them.
pub mod sessions;

/// The resource data sync configurations this service holds: the record a create
/// persists, the configuration rules it is held to, and how a listing, an update and a
/// delete address one.
pub mod resource_data_syncs;

/// The maintenance windows this service holds: the record a create persists, the schedule
/// grammar it is held to, and the instants that schedule names.
pub mod maintenance_windows;

/// The targets registered with a maintenance window: the record a register persists, and how
/// a describe and a deregister address one.
pub mod maintenance_window_targets;

/// The tasks registered with a maintenance window: the record a register persists, the values
/// the service puts there instead of the ones it was handed, and how a read addresses one.
pub mod maintenance_window_tasks;

/// The RUNS of a maintenance window: the record one of its executions is held as, and how a
/// read addresses one by the id it was executed under.
pub mod maintenance_window_executions;

/// The TASK EXECUTIONS of one run of a maintenance window: the record one is held as, how a
/// read addresses one within its run, and how a listing of a run's own is narrowed and
/// bounded.
pub mod maintenance_window_execution_tasks;

/// The INVOCATIONS of one task execution of a maintenance window run: the record one is held
/// as, and how a read addresses one within its task execution.
pub mod maintenance_window_execution_task_invocations;

/// What an update that takes a `Replace` flag leaves behind: the one reading of an omitted
/// member every such operation of this service holds to.
pub mod replacing_update;

/// The account-level settings the Systems Manager service teams provision, which
/// `GetServiceSetting` reports and `UpdateServiceSetting`/`ResetServiceSetting` modify.
pub mod service_settings;

/// The IAM role an account registers for Default Host Management Configuration, and the
/// service setting that registration lives in.
pub mod default_host_management;

/// The one tag set each resource this store holds carries, and how a tag request
/// addresses it.
pub mod tagging;

/// The `{{ Name }}` references an SSM document's content carries, and resolving them
/// against the values a call supplied — the one scan and the one substitution every
/// document of this service reads its parameters through.
pub mod document_references;

/// Running a command document's own script in the GUEST of the node a command was sent to,
/// which is what an invocation's output and exit code come from.
pub mod guest_execution;

/// The RESOLVER CLIENTS a script delivered to a node's guest resolves a name with, answered by
/// the DNS of the VPC that node is in.
pub mod guest_resolver;

/// The PACKAGE QUERY client a script delivered to a node's guest reads its installed SSM Agent
/// package out of, answered from the build that node reports.
pub mod guest_packages;

/// The PYTHON INTERPRETER in a node's guest, which runs the inline program a command's script
/// hands it in the one contained engine this workspace embeds.
pub mod guest_python;

/// The AWS CLI in a node's guest, which performs the object calls a command's script makes as
/// the role the instance runs as — and on the terms the machine's own network carries.
pub mod guest_aws_cli;

/// Where this service's EC2-facing work lives: the registration half and the delivery half,
/// and why they are separate.
pub mod ec2_managed_nodes;

/// The validation a put of an `aws:ec2:image` parameter is held to, and the write that
/// validation gates when the value is no image the account can see.
pub mod parameter_image_validation;

/// What a parameter's `DataType` says about its VALUE: the data types whose value form the
/// service states, and the sentence a value that does not satisfy one is refused with.
pub mod parameter_data_types;
/// The `ParameterStringFilter` vocabulary both filter-taking parameter reads publish, how
/// a filter matches a stored parameter, and the sentences a rejected filter is refused
/// with. Re-exported, so every name stays reachable as `crate::state::<name>`.
mod parameter_filters;
pub use parameter_filters::*;

/// What one parameter read may walk: the page size each paged read serves and the
/// hierarchy a `Path` names. Re-exported like the rest.
mod parameter_reads;
pub use parameter_reads::*;

/// The string filters a managed-node read accepts, and the shapes an activation id and an
/// agent version carry. Re-exported like the rest.
mod node_filters;
pub use node_filters::*;

/// The documents this store holds: the guard on a document's untrusted content, the parse
/// that turns it into the modelled description, the catalog AWS publishes and the stored
/// record one document version is kept as. Re-exported, so every name stays reachable as
/// `crate::state::<name>`.
mod documents;
pub use documents::*;

/// The bodies of the Command and Session documents the published catalogue carries
/// ([`PUBLISHED_DOCUMENTS`]).
pub mod published_command_documents;

/// The bodies of the Automation runbooks the published catalogue carries
/// ([`PUBLISHED_DOCUMENTS`]).
pub mod published_automation_runbooks;

use service_settings::ServiceSettingRecord;

use crate::ops::types::{
    Activation, AttachmentInformation, AttachmentsSource, DocumentDescription, DocumentParameter,
    DocumentRequires, InstanceInformation, InstanceProperty, Parameter, ParameterHistory,
    ParameterInlinePolicy, ParameterMetadata, Tag, DESCRIBE_ACTIVATIONS_FILTER_KEYS_VALUES,
    INSTANCE_INFORMATION_FILTER_KEY_VALUES, INSTANCE_PROPERTY_FILTER_KEY_VALUES,
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

/// The `ParameterType` values in the order the modelled enum DECLARES them
/// (`smithy_query(query="shape",shape="ParameterType")`: `String`, `StringList`,
/// `SecureString`), which is the order the service lists them in when it tells a caller
/// what the attribute can be ([`invalid_filter_value_message`]). The generated
/// [`crate::ops::types::PARAMETER_TYPE_VALUES`] holds the same three sorted, which is what
/// a membership test reads and what the boundary walk refuses an unmodelled value by.
pub const PARAMETER_TYPES_AS_MODELLED: &[&str] = &[TYPE_STRING, TYPE_STRING_LIST, TYPE_SECURE_STRING];

/// The `ParameterTier` values in that same declaration order (`Standard`, `Advanced`,
/// `Intelligent-Tiering`), for the same reason.
pub const PARAMETER_TIERS_AS_MODELLED: &[&str] =
    &[TIER_STANDARD, TIER_ADVANCED, TIER_INTELLIGENT_TIERING];

/// Content size a standard parameter holds: "Standard parameters have a content size
/// limit of 4 KB" (`PutParameter`'s `Tier` doc).
pub const STANDARD_VALUE_LIMIT: usize = 4 * 1024;

/// Content size an advanced parameter holds: "Advanced parameters have a content size
/// limit of 8 KB" (same doc).
pub const ADVANCED_VALUE_LIMIT: usize = 8 * 1024;

/// How deep a parameter hierarchy may go: "Parameter hierarchies are limited to a
/// maximum depth of fifteen levels" (`PutParameter`'s `Name` doc), which is what
/// `HierarchyLevelLimitExceededException` reports.
///
/// The depth is the HIERARCHY's, and "the hierarchy is the parameter name except the last
/// part of the parameter" (`GetParametersByPath`'s `Path` doc): the leaf is the parameter
/// itself rather than a level of the tree it hangs in, so a name carries at most this many
/// levels BESIDE its leaf ([`name_rejection`]).
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
pub const POLICY_TYPE_EXPIRATION: &str = "Expiration";

/// See [`POLICY_TYPE_EXPIRATION`]. This one is a notification ABOUT an expiration: "This
/// policy initiates an event in Amazon CloudWatch Events that notifies you about the
/// expiration. By using this policy, you can receive notification before or after the
/// expiration time is reached" (`PutParameter`'s `Policies` doc), so the time it is
/// measured from is the one an [`POLICY_TYPE_EXPIRATION`] policy sets.
pub const POLICY_TYPE_EXPIRATION_NOTIFICATION: &str = "ExpirationNotification";

/// See [`POLICY_TYPE_EXPIRATION`]. This one is measured from the parameter's own last
/// change ("initiates a CloudWatch Events event if a parameter hasn't been modified for a
/// specified period of time", same doc), so it stands on its own.
pub const POLICY_TYPE_NO_CHANGE_NOTIFICATION: &str = "NoChangeNotification";

/// The policy types a parameter may carry (see [`POLICY_TYPE_EXPIRATION`]).
pub const POLICY_TYPES: &[&str] = &[
    POLICY_TYPE_EXPIRATION,
    POLICY_TYPE_EXPIRATION_NOTIFICATION,
    POLICY_TYPE_NO_CHANGE_NOTIFICATION,
];

/// What a `Policies` document carrying a policy of a type Parameter Store does not support is
/// refused with: the sentence the service words that refusal with, naming the type the request
/// asked for and nothing else of it — the supported types are what the modelled
/// `InvalidPolicyTypeException` states ([`POLICY_TYPES`]), not what the refusal lists.
pub fn unsupported_policy_type_message(policy_type: &str) -> String {
    format!("Policy type {policy_type} is not supported.")
}

/// What a `Policies` document whose policies cannot stand together is refused with.
///
/// Each sentence is fixed text naming the policy TYPES in conflict, in the prose spelling
/// the service words them in ([`policy_type_in_prose`]) rather than the modelled
/// `PascalCase` token, and carries nothing else of the request: what the refusal has to
/// say is which policies cannot stand beside each other.
pub fn duplicate_policy_message(policy_type: &str) -> String {
    format!(
        "There can be only one {} policy",
        policy_type_in_prose(policy_type)
    )
}

/// Whether a parameter may carry MORE THAN ONE policy of `policy_type`.
///
/// An [`POLICY_TYPE_EXPIRATION`] policy sets the ONE time the parameter expires, so a second
/// one configures that time twice over: "You can't, for example, specify two Expiration
/// policies for a parameter" (the `IncompatiblePolicyException` doc). A NOTIFICATION policy
/// configures one notification about an event, and a parameter may be notified about that
/// event more than once — several leads before the same expiration, several periods without a
/// change — so those types repeat, and a document carrying two of them is a document
/// configuring two notifications rather than one configuration stated twice.
pub fn policy_type_may_repeat(policy_type: &str) -> bool {
    policy_type != POLICY_TYPE_EXPIRATION
}

/// The missing-dependency form of [`duplicate_policy_message`]: a notification ABOUT an
/// expiration is measured from the time an [`POLICY_TYPE_EXPIRATION`] policy sets, so it
/// cannot be the only policy of the set.
///
/// The sentence is the service's own wording for that conflict and carries nothing of the
/// request: it states the policy KINDS that cannot stand apart, not how many of them the
/// document carried or what they were addressed to.
pub fn notification_requires_expiration_message() -> String {
    let mut sentence = policy_type_in_prose(POLICY_TYPE_EXPIRATION_NOTIFICATION);
    if let Some(first) = sentence.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    format!(
        "{sentence} policies are missing {} policy",
        policy_type_in_prose(POLICY_TYPE_EXPIRATION)
    )
}

/// A policy type as a refusal spells it out: the modelled `PascalCase` token in lower-case
/// words, so `ExpirationNotification` is "expiration notification".
fn policy_type_in_prose(policy_type: &str) -> String {
    let mut prose = String::with_capacity(policy_type.len() + 2);
    for character in policy_type.chars() {
        if character.is_ascii_uppercase() && !prose.is_empty() {
            prose.push(' ');
        }
        prose.extend(character.to_lowercase());
    }
    prose
}

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
        // An EMPTY policy object states no policy: it is one of the two ways a `Policies`
        // document asks for the parameter to carry none ("All existing policies are
        // preserved until you send new policies or an empty policy"), so it is not reported
        // as a policy with no type.
        .filter(|entry| entry.as_object().is_none_or(|policy| !policy.is_empty()))
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

/// The `DataType` values `PutParameter`'s own documentation lists: "The following data type
/// values are supported. text aws:ec2:image aws:ssm:integration" (`PutParameter`'s
/// `DataType` doc).
///
/// A LISTING rather than the set the wire admits: `ParameterDataType` is a plain string
/// carrying only a length (0..128) and no enum or pattern, so a data type outside this
/// listing is a value the member accepts and what the service holds such a put to is its
/// VALUE ([`parameter_data_types`]).
pub const DOCUMENTED_DATA_TYPES: &[&str] = &[
    DEFAULT_DATA_TYPE,
    DATA_TYPE_EC2_IMAGE,
    "aws:ssm:integration",
];

/// The `DataType` a parameter carries when the request names none: "The data type of
/// the parameter, such as text or aws:ec2:image. The default is text" (the
/// `Parameter$DataType` doc).
pub const DEFAULT_DATA_TYPE: &str = "text";

/// The `DataType` whose value is an Amazon EC2 image id: "When you create a String
/// parameter and specify aws:ec2:image, Amazon Web Services Systems Manager validates the
/// parameter value is in the required format … and that the specified AMI is available in
/// your Amazon Web Services account" (`PutParameterRequest$DataType`). That validation is
/// the one [`parameter_image_validation`] makes, and it gates the WRITE: the put answers
/// success either way, and a value it rejects lands nowhere.
pub const DATA_TYPE_EC2_IMAGE: &str = "aws:ec2:image";

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
    /// The name is not a parameter name at ALL — it ends in the hierarchy separator, so
    /// its last level has no name in it, or it names a hierarchy without naming the
    /// hierarchy's own root — so it is refused as a malformed request value rather than as
    /// a name that fails one of the naming constraints: `ValidationException`.
    Malformed(String),
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
    // A name that ends in the separator names a last level with no name in it, which is
    // not a parameter name at all: it is refused as a malformed request value, before any
    // of the naming constraints is read off it.
    if path.ends_with('/') {
        return Some(NameRejection::Malformed(
            TRAILING_SEPARATOR_MESSAGE.to_string(),
        ));
    }
    if path.split('/').any(str::is_empty) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' carries an empty hierarchy level"
        )));
    }
    // The hierarchy rule: a name below the top level must name the hierarchy it lives in,
    // by the leading separator, or it is not the fully qualified name a parameter is
    // addressed by at all — so `Dev/DBServer` is a malformed request value rather than a
    // name that broke one of the naming constraints.
    if path.contains('/') && !name.starts_with('/') {
        return Some(NameRejection::Malformed(
            UNQUALIFIED_NAME_MESSAGE.to_string(),
        ));
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
    // The levels are the hierarchy the name hangs in, which is the name without its leaf
    // ([`MAX_HIERARCHY_LEVELS`]), so a top-level name is at depth zero and a name of
    // `MAX_HIERARCHY_LEVELS` levels plus a leaf is still accepted.
    let levels = path.split('/').count().saturating_sub(1);
    if levels > MAX_HIERARCHY_LEVELS {
        return Some(NameRejection::Depth(hierarchy_depth_message()));
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

/// What a `Path` or a name that does not meet Parameter Store's parameter-name
/// requirements is refused with: the one paragraph the service states those requirements
/// in.
///
/// Wholly fixed text with no variable part — it echoes neither the rejected value nor the
/// single rule that value broke, because what it has to say is the whole of what a name
/// has to satisfy, ending in an example of one that does.
pub const PARAMETER_NAME_REQUIREMENTS: &str = concat!(
    r#"The parameter doesn't meet the parameter name requirements. "#,
    r#"The parameter name must begin with a forward slash "/". "#,
    r#"It can't be prefixed with \"aws\" or \"ssm\" (case-insensitive). "#,
    r#"It must use only letters, numbers, or the following symbols: . (period), "#,
    r#"- (hyphen), _ (underscore). Special characters are not allowed. "#,
    r#"All sub-paths, if specified, must use the forward slash symbol "/". "#,
    r#"Valid example: /get/parameters2-/by1./path0_."#,
);

/// What a call addressed to a name in a hierarchy AWS PUBLISHES in is refused with.
///
/// Fixed text naming nothing of the request: the refusal is not that the hierarchy holds
/// no such parameter — it is that AWS's own published data is not the account's to read as
/// its own or to change ([`public_parameters`]), so a caller learns to stop rather than to
/// retry under another name.
pub const RESTRICTED_PARAMETER_MESSAGE: &str =
    "Cannot access/update data for the restricted parameter.";

/// What a `Policies` document carrying more policies than a parameter may hold is refused
/// with: the sentence the service words that quota with, carrying the published maximum
/// ([`MAX_POLICIES`]) and nothing of the request — not how many policies the document
/// carried, and not the parameter it addressed.
pub fn policies_limit_message() -> String {
    format!("Number of policies attached to a parameter cannot exceed {MAX_POLICIES}")
}

/// What a call addressed to a reserved name AWS publishes nothing under is refused with.
///
/// The sentence names the reserved name the request addressed, canonically (the spelling the
/// parameter's ARN carries after its `parameter/` prefix). It is NOT
/// [`RESTRICTED_PARAMETER_MESSAGE`], which a call addressed to a name in a hierarchy AWS
/// really publishes in is refused with: which of the two a refusal carries is decided by
/// where in the reserved hierarchy the name lies
/// ([`public_parameters::reserved_name_refusal_message`]), not by which call asked.
pub fn reserved_name_denial_message(name: &str) -> String {
    format!(
        "No access to reserved parameter name: {}.",
        canonical_name(name)
    )
}

/// What a name that ENDS in the hierarchy separator is refused with.
///
/// Wholly fixed text stating the mistake and nothing of the request: the level after the
/// final separator has no name in it, so the value names no parameter at all.
pub const TRAILING_SEPARATOR_MESSAGE: &str = "Parameter name must not end with slash.";

/// What a hierarchical name that does not name the hierarchy it lives in is refused with:
/// the name is not the "fully qualified name" a parameter is addressed by ("The fully
/// qualified name includes the complete hierarchy of the parameter path and name. For
/// parameters in a hierarchy, you must include a leading forward slash character (/)",
/// `PutParameter`'s `Name` doc), so the value names no parameter at all.
///
/// Wholly fixed text stating what the name must be, with nothing of the request in it.
pub const UNQUALIFIED_NAME_MESSAGE: &str = "Parameter name must be a fully qualified name.";

/// What a name nested deeper than Parameter Store allows is refused with.
///
/// Wholly fixed text carrying the published depth and nothing of the request: what the
/// refusal has to say is the limit the tree has ([`MAX_HIERARCHY_LEVELS`]), which neither
/// the rejected name nor its own depth adds to.
pub fn hierarchy_depth_message() -> String {
    format!("Depth of the tree cannot exceed {MAX_HIERARCHY_LEVELS}")
}

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
/// The hierarchy separator LEADS a name that names a hierarchy: "The fully qualified name
/// includes the complete hierarchy of the parameter path and name. For parameters in a
/// hierarchy, you must include a leading forward slash character (/) when you create or
/// reference a parameter" (`PutParameter`'s `Name` doc). A name of a SINGLE level names no
/// hierarchy to lead, so it carries no leading separator at all: `MyParameter` and
/// `/MyParameter` are the one parameter ([`canonical_name`]) and that parameter is held and
/// reported as `MyParameter`, while `/Dev/DBServer` keeps its whole path, separator and all.
///
/// That rendering is the parameter's IDENTITY: it is the store key ([`canonical_name`]) and
/// the resource the parameter's ARN carries after its `parameter/` prefix
/// ([`parameter_arn`]). So either spelling addresses the parameter, a client that round-trips
/// a reported `Name` into another call reaches the one it read, and the `Name` a read reports
/// never disagrees with the ARN beside it.
///
/// A read of a SINGLE named parameter reports it under the spelling its own request asked
/// with instead ([`echoed_name`]), which is what a client keying its results by the string it
/// sent needs; a listing and a history, which report the parameter rather than echo a name,
/// report this one.
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

/// The `Name` a read of ONE named parameter echoes for what it served.
///
/// A read takes "the name or Amazon Resource Name (ARN) of the parameter that you want to
/// query" (`GetParameter`'s `Name` doc): asked by NAME it echoes that name back as the
/// request WROTE it — selector stripped, and the surrounding padding the service trims
/// before it resolves anything kept — so a client that keys its results by the string it
/// sent finds them, which is what `GetParameters` is answered by name for at all. Asked by
/// ARN it names the parameter rather than the ARN, so it reports `stored`, the name the
/// store holds.
///
/// `asked` is the request's own value, untrimmed; `query` is what that value named
/// ([`parse_parameter_query`]), whose `region` is set only for an ARN.
pub fn echoed_name<'a>(asked: &'a str, query: &ParameterQuery<'_>, stored: &'a str) -> &'a str {
    if query.region.is_some() {
        return stored;
    }
    match query.selector {
        None => asked,
        // The selector was read off the trimmed value, so it is stripped from the end of
        // that same reading; a value that does not end in it is echoed whole.
        Some(selector) => asked
            .trim_end()
            .strip_suffix(selector)
            .and_then(|head| head.strip_suffix(SELECTOR_SEPARATOR))
            .unwrap_or(asked),
    }
}

/// How a refusal that names a version or a label QUOTES the parameter back: by the one
/// rendering of a parameter name this service holds and reports it under
/// ([`reported_name`]), so a caller reads the same name in a refusal as in a read — the
/// hierarchy path for a hierarchical name, separator and all, and the bare name for a
/// top-level one, which is in no hierarchy to lead.
pub fn quoted_name(name: &str) -> &str {
    reported_name(name)
}

/// What a read whose `name:version` selector names a version the parameter does not
/// carry is refused with.
///
/// `name` is quoted back as [`quoted_name`] renders it, and the version is the one the
/// request asked for.
pub fn version_not_found_message(name: &str, version: i64) -> String {
    format!(
        "Systems Manager could not find version {version} of {}. Verify the version and \
         try again.",
        quoted_name(name)
    )
}

/// What a read whose `name:label` selector names a label no version of the parameter
/// carries is refused with (the label form of [`version_not_found_message`]).
pub fn version_label_not_found_message(name: &str, label: &str) -> String {
    format!(
        "Systems Manager could not find a version of {} that has the specified label, \
         {label}. Verify the parameter label and try again.",
        quoted_name(name)
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
/// dropped, whichever depth the name has — not by [`quoted_name`], which the version
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
/// higher tier to name, so its refusal opens by naming the VALUE as what was too large.
/// Each sentence carries the tier's own published limit and nothing from the request:
/// the size the caller sent is never echoed back.
pub fn value_limit_message(tier: &str) -> String {
    let limit = value_limit(tier);
    if tier == TIER_ADVANCED {
        format!(
            "The specified parameter value is too large. {TIER_ADVANCED}-tier parameters \
             support a maximum parameter value of {limit} characters."
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

/// What a put that would take an ADVANCED parameter back to the standard tier is refused
/// with: "You can change a standard parameter to an advanced parameter any time. But you
/// can't revert an advanced parameter to a standard parameter" (`PutParameter`'s `Tier`
/// doc).
///
/// Fixed text throughout — nothing of the request is echoed into it: the refusal states the
/// rule, the way out of it (delete the advanced parameter and recreate it) and the ceiling
/// the standard tier would then hold the value to, which is that tier's own content size
/// limit ([`STANDARD_VALUE_LIMIT`]) in characters.
pub fn tier_downgrade_message() -> String {
    format!(
        "This parameter uses the advanced-parameter tier. You can't downgrade a parameter \
         from the advanced-parameter tier to the standard-parameter tier. If necessary, you \
         can delete the advanced parameter and recreate it as a standard parameter. Be aware \
         that standard parameters have a value limit of {STANDARD_VALUE_LIMIT} characters."
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
    /// A `ParameterDataType` value (see [`DOCUMENTED_DATA_TYPES`]).
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
    ///
    /// The `Name` is the one the store holds; a read of a SINGLE named parameter reports it
    /// under the spelling its request asked with instead ([`echoed_name`]), while a listing,
    /// which asked for no name of its own, reports the stored one.
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
            last_modified_date: Some(wire_instant(self.last_modified_date)),
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
            key_id: reported_text(self.key_id.clone()),
            last_modified_date: Some(wire_instant(self.last_modified_date)),
            // As in [`Self::to_parameter_history`]: the principal the write that created
            // this version was signed for, recorded by that write and reported unchanged,
            // so two reads of one version report one writer.
            last_modified_user: reported_text(self.last_modified_user.clone()),
            // An attribute the parameter carries no value for is reported as none rather
            // than as empty text ([`reported_text`]): a parameter with no effective allowed
            // pattern has no pattern member, and one with no description no description.
            description: reported_text(self.description.clone()),
            allowed_pattern: reported_text(self.allowed_pattern.clone()),
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
            key_id: reported_text(self.key_id.clone()),
            last_modified_date: Some(wire_instant(self.last_modified_date)),
            // "Amazon Resource Name (ARN) of the Amazon Web Services user who last
            // changed the parameter": the principal the write that created THIS version
            // was signed for, so an entry names the writer of the version it reports
            // rather than the writer of the newest one.
            last_modified_user: reported_text(self.last_modified_user.clone()),
            // As in [`Self::to_parameter_metadata`]: an attribute the version carries no
            // value for is reported as none rather than as empty text.
            description: reported_text(self.description.clone()),
            value: Some(self.value.clone()),
            allowed_pattern: reported_text(self.allowed_pattern.clone()),
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

/// When the clock owes a freshly minted document version its move to `Active`: AT THE
/// INSTANT the call that minted it completes.
///
/// A document is registered by the call that writes it. RECORDED BEHAVIOUR: a
/// `CreateDocument` answers with `Status` `Creating` — the transient status is what the
/// minting call itself reports — and an UNPOLLED `GetDocument`/`DescribeDocument` straight
/// after it reads `Active`, so the document is usable as an association or Run Command
/// target without anybody waiting. The move is therefore owed from `minted`, the instant
/// the mint happened, and is materialised by
/// [`ComAmazonawsSsmAmazonssmState::materialize_due`] on the first read that comes after
/// it rather than by any call doing the promotion for itself.
pub fn document_active_due(minted: SystemTime) -> SystemTime {
    minted
}

/// The `DocumentStatus` a version Systems Manager is still registering FOR AN EXISTING
/// document carries, from the same modelled enum: "the document is being updated" is
/// what `Updating` says, so a version `UpdateDocument` has just minted is `Updating`
/// where a version `CreateDocument` minted is [`DOCUMENT_STATUS_CREATING`].
pub const DOCUMENT_STATUS_UPDATING: &str = "Updating";

/// How many nanoseconds a millisecond is, which is the resolution this service states an
/// instant at: a finer reading of the clock than what goes on the wire states a precision
/// the value does not hold, so it is dropped rather than written. The one declaration of
/// it, so every place that quantises an instant — a maintenance window's rendered
/// instants ([`maintenance_windows`]) and the one writer every reported instant goes
/// through ([`wire_instant`]) — agrees on what the resolution is.
pub const NANOS_PER_MILLISECOND: u32 = 1_000_000;

/// `at` as the timestamp a response reports it as: the instant TRUNCATED to the millisecond
/// it falls in ([`NANOS_PER_MILLISECOND`]).
///
/// This protocol puts a `timestamp` on the wire as epoch seconds with a fraction, and the
/// finest fraction AWS states one with is the millisecond — so the nanosecond remainder the
/// host clock reads is no part of the value, and dropping it is what keeps a reported
/// instant in the form a client's parser reads back. Truncated rather than rounded, so a
/// reported instant never names one later than the call that stamped it.
///
/// THE one encoder for every instant this service reports, whichever plane reports it, so no
/// operation's projection can state an instant at a precision another's does not.
pub fn wire_instant(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let stamped = ::aws_smithy_types::DateTime::from(at);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        stamped.secs(),
        stamped.subsec_nanos() / NANOS_PER_MILLISECOND * NANOS_PER_MILLISECOND,
    )
}

/// `value` as an OPTIONAL string member of a response: the text the resource carries, or
/// nothing at all when it carries none.
///
/// An optional member reports a value the resource HAS; a resource that has none for it is
/// serialized without the member rather than with the empty string, which is a value — a
/// client telling "nothing configured" from "configured as empty" reads the two apart, and
/// one that compiles a pattern or formats a description only when the member is present is
/// handed no empty text to work on. An empty string supplied for such a member states no
/// value, so it is reported as none.
pub fn reported_text(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

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

/// How this service states that the document a call named holds no version of the number
/// the call asked for, when the refusal names BOTH the document and that number.
///
/// `InvalidDocumentVersion` is "the document version isn't valid or doesn't exist" (its
/// own documentation), and the sentence is fixed text around the two values the request
/// carried: `Document with name <name> with version <version> does not exist.`. Both
/// variable parts are echoed from the request and nothing is read out of the store for it
/// — the document's own latest version number belongs to the sentence a version READ is
/// refused with, not to this one.
pub fn document_version_not_found_message(name: &str, document_version: &str) -> String {
    format!("Document with name {name} with version {document_version} does not exist.")
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
    /// What each of those parameters ADMITS as a value, in the same order: the part of a
    /// `parameters` entry the modelled `DocumentParameter` has no member for
    /// ([`DeclaredParameterConstraints`]).
    pub parameter_constraints: Vec<DeclaredParameterConstraints>,
    /// The platforms the content's steps leave the document compatible with, or every
    /// modelled platform when they restrict it to none
    /// ([`document_platforms::declared_platform_types`]).
    pub platform_types: Vec<String>,
    /// The names of the steps the content declares, in the content's own order: the
    /// steps a command's invocation runs, which `GetCommandInvocation`'s `PluginName`
    /// names one of ([`declared_step_names`]).
    pub step_names: Vec<String>,
    /// The top-level member the content declares its steps in — [`DOCUMENT_MAIN_STEPS`] in
    /// a schema 2.x or 0.3 body, [`DOCUMENT_RUNTIME_CONFIG`] in a schema 1.2 one — or
    /// `None` when it declares neither, which is what a body missing the member its schema
    /// requires is refused for ([`ParsedDocumentContent::missing_steps_member`]).
    pub steps_member: Option<String>,
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

/// How large a regular expression a REQUEST carries may compile to, in bytes of compiled
/// program, and how large the lazy-DFA cache a match through it may use.
///
/// Emulator resource guards: an expression a caller supplied — a parameter's
/// `AllowedPattern`, a document's `allowedPattern` — builds an automaton and matches through
/// a cache, so both are bounded explicitly rather than left to the crate's defaults, and a
/// pattern past the first is treated as one this service cannot evaluate. The `regex` crate's
/// matching is linear in the input and has no backtracking blowup, so with the automaton and
/// the cache bounded here, the cost of one match grows no faster than the value the request
/// carried.
///
/// ONE budget for every caller-supplied expression of this service, so no reader of one can
/// be more generous than another ([`compiled_pattern`], and the two readings built on it:
/// [`parameter_pattern`] and [`allowed_pattern_rejection`]).
pub const COMPILED_PATTERN_LIMIT: usize = 64 * 1024;

/// See [`COMPILED_PATTERN_LIMIT`].
pub const PATTERN_DFA_LIMIT: usize = 64 * 1024;

/// `pattern`, as supplied by a caller, compiled under this service's budget for one
/// ([`COMPILED_PATTERN_LIMIT`]) — or the compiler's own reason for an expression this service
/// cannot evaluate, which is what a refusal about the PATTERN itself states.
///
/// The one place that budget is applied, so every reading of a supplied expression here is
/// bounded alike.
fn compiled_pattern(pattern: &str) -> Result<::regex::Regex, ::regex::Error> {
    ::regex::RegexBuilder::new(pattern)
        .size_limit(COMPILED_PATTERN_LIMIT)
        .dfa_size_limit(PATTERN_DFA_LIMIT)
        .build()
}

/// `pattern`, as supplied by a caller, compiled under this service's budget for one
/// ([`COMPILED_PATTERN_LIMIT`]) — `None` for an expression this service cannot evaluate,
/// which restricts nothing rather than making the document that carries it unusable (the
/// framework's own treatment of a pattern it cannot compile).
pub fn parameter_pattern(pattern: &str) -> Option<::regex::Regex> {
    compiled_pattern(pattern).ok()
}

/// Why a value is not one an `AllowedPattern` accepts, named by the modelled error that
/// reports it.
///
/// The two mean opposite things, so they are never reported interchangeably: one says
/// the caller's VALUE is wrong, the other that the caller's PATTERN is.
#[derive(Debug, PartialEq, Eq)]
pub enum AllowedPatternRejection {
    /// The supplied `AllowedPattern` is not a regular expression at all:
    /// `InvalidAllowedPatternException` ("The request doesn't meet the regular
    /// expression requirement").
    Pattern(String),
    /// The value does not satisfy a well-formed `AllowedPattern`:
    /// `ParameterPatternMismatchException`.
    Value(String),
}

/// Why `value` does not satisfy `pattern`, or `None` when it does (or when the write named
/// no pattern).
///
/// `AllowedPattern` is "a regular expression used to validate the parameter value", so
/// the value is matched against it in full — the whole value, not a substring, the way
/// a regular-expression constraint on a member reads. A `StringList`'s value is not one
/// value but a list ("Items in a StringList must be separated by a comma (,)"), so the
/// constraint holds of each ITEM of it rather than of the comma-joined whole; every
/// other type, `SecureString` included, is matched as the one value it is — and a
/// `SecureString` is matched on the plaintext the request carried, before it is sealed.
///
/// The expression comes from the caller, so it is evaluated by the `regex` crate, whose
/// matching is linear in the input and has no backtracking blowup, under this service's own
/// compiled-size budget for a supplied expression ([`compiled_pattern`]); the modelled
/// `AllowedPattern` length (0..1024) bounds what is compiled.
///
/// This is the ONE reading of the rule for this service: `PutParameter` asks it of the write
/// it is about to make, and so does every other path that stores a parameter version,
/// whichever API that path belongs to.
pub fn allowed_pattern_rejection(
    pattern: Option<&str>,
    type_: &str,
    value: &str,
) -> Option<AllowedPatternRejection> {
    let pattern = pattern.filter(|pattern| !pattern.is_empty())?;
    let anchored = format!("^(?:{pattern})$");
    let expression = match compiled_pattern(&anchored) {
        Ok(expression) => expression,
        Err(why) => {
            return Some(AllowedPatternRejection::Pattern(format!(
                "AllowedPattern '{pattern}' is not a regular expression this service can \
                 evaluate: {why}"
            )))
        }
    };
    let satisfied = if type_ == TYPE_STRING_LIST {
        value
            .split(STRING_LIST_SEPARATOR)
            .all(|item| expression.is_match(item))
    } else {
        expression.is_match(value)
    };
    (!satisfied).then(|| AllowedPatternRejection::Value(allowed_pattern_mismatch_message(pattern)))
}

/// The `PingStatus` a node that has just pinged the service carries (from the modelled
/// enum, `crate::ops::types::PING_STATUS_VALUES`).
pub const PING_STATUS_ONLINE: &str = "Online";

/// The `PingStatus` a node whose agent is NO LONGER reaching the service carries (same
/// enum): the node is registered — Systems Manager still reports it — and the agent inside it
/// has stopped reporting, which is what an instance that cannot reach the service's endpoints
/// any more leaves behind.
pub const PING_STATUS_CONNECTION_LOST: &str = "ConnectionLost";

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

/// The member path a page-size refusal names, as the validating walk spells it.
pub const NODE_PAGE_SIZE_MEMBER: &str = "maxResults";

/// The page size a node report serves, or the refusal message for a `MaxResults` outside
/// the modelled range — which the protocol's boundary validator already refuses, so this
/// is a backstop rather than the first line of defence.
///
/// The refusal states the MEMBER and the BOUND the model declares, and nothing of the
/// request: a broken range is the violated constraint, reported in the framework's own
/// envelope and clause (`service_common::validation_message`), so this backstop cannot word
/// the same refusal differently from the walk that normally catches it — and the operations
/// whose boundary clauses state no value ([`crate::service`]) state none here either.
pub fn node_page_size(max_results: Option<i32>) -> Result<usize, String> {
    let requested = max_results.unwrap_or(DEFAULT_NODE_PAGE_SIZE);
    let broken = if requested < MIN_NODE_PAGE_SIZE {
        Some(::aws_smithy_schema_server::constraints::ViolationKind::MinValue(
            MIN_NODE_PAGE_SIZE.to_string(),
        ))
    } else if requested > MAX_NODE_PAGE_SIZE {
        Some(::aws_smithy_schema_server::constraints::ViolationKind::MaxValue(
            MAX_NODE_PAGE_SIZE.to_string(),
        ))
    } else {
        None
    };
    if let Some(kind) = broken {
        let clause = ::aws_smithy_schema_server::constraints::Violation {
            path: NODE_PAGE_SIZE_MEMBER.to_string(),
            kind,
            // No value: this operation's clauses state the member and the requirement alone.
            value: None,
            sensitive: true,
        }
        .message();
        return Err(
            ::service_common::validation_message::envelope_over(std::slice::from_ref(&clause))
                .unwrap_or(clause),
        );
    }
    Ok(requested as usize)
}

/// How this service states a `NextToken` it did not issue. `InvalidNextToken`'s own
/// documentation is "The specified token isn't valid".
pub const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";

/// How a Parameter Store read states a `NextToken` it did not issue: the same class, worded
/// with the rejected token, so a paginating client can see WHICH of the tokens it holds was
/// refused.
///
/// The token is the caller's own value, echoed back; nothing of the store is in the
/// sentence.
pub fn invalid_next_token_message(token: &str) -> String {
    format!("The following token is invalid: {token}.")
}

/// The name this service is reported by in the envelope that follows a refusal worded by
/// the service's own validation: the name its wire protocol carries as the `X-Amz-Target`
/// prefix (`OPERATIONS.md`, "Wire protocol").
pub const SDK_ENVELOPE_SERVICE: &str = "AmazonSSM";

/// Where the envelope that trails such a refusal opens, naming the service that answered.
const SDK_ENVELOPE_OPENING: &str = " (Service: ";

/// The field of that envelope naming the request the response answers, which is the one
/// field read back out of a rendered message ([`sdk_envelope_request_id`]).
const SDK_ENVELOPE_REQUEST_ID: &str = "; Request ID: ";

/// Where that envelope closes: the proxy field, which an in-process answer came through
/// none of.
const SDK_ENVELOPE_CLOSING: &str = "; Proxy: null)";

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
        "{body}{SDK_ENVELOPE_OPENING}{SDK_ENVELOPE_SERVICE}; Status Code: {status_code}; Error \
         Code: {error_code}{SDK_ENVELOPE_REQUEST_ID}{}{SDK_ENVELOPE_CLOSING}",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// The request id a message stated in that envelope, or `None` for a message that states
/// none.
///
/// The envelope above names the request the response answers INSIDE the message, so the
/// response has to report that same id in the header a client correlates on
/// ([`crate::service`] does, through this reading): a message naming one request and a
/// header naming another would be two ids for one call. Read off the rendered answer, from
/// the two markers the writer above puts the field between, so the writer and the reader
/// cannot drift apart.
///
/// Only a well-formed id of the form this emulator mints is reported
/// (`service_common::query_error_envelope::is_request_id`), and only one stated inside a
/// complete envelope: the reading is textual, and the body of a message is the one part of
/// a refusal a request can influence.
pub fn sdk_envelope_request_id(message: &str) -> Option<&str> {
    let closing = message.rfind(SDK_ENVELOPE_CLOSING)?;
    let stated = &message[..closing];
    let field = stated.rfind(SDK_ENVELOPE_REQUEST_ID)?;
    if !stated[..field].contains(SDK_ENVELOPE_OPENING) {
        return None;
    }
    let request_id = &stated[field + SDK_ENVELOPE_REQUEST_ID.len()..];
    ::service_common::query_error_envelope::is_request_id(request_id).then_some(request_id)
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
    /// No CALL of this service registers a node: registration is the SSM Agent's own call,
    /// which this service's model does not expose — `CreateActivation` mints the activation
    /// an agent registers WITH, not the node. What does register one is an Amazon EC2
    /// instance of this account whose instance profile lets its agent register, which
    /// [`node_registration::settle_registrations`] materialises here once the clock makes
    /// that registration due. A describe therefore reports the empty set for an account
    /// holding no such instance — what the store holds — rather than nodes nobody
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

    /// Every OpsMetadata object, keyed by `(region, OpsMetadata ARN)`: an object is created
    /// through one Region's endpoint and is reached there by the ARN the create minted
    /// ([`ops_metadata::ops_metadata_arn`]), which is the only thing the reads take. A
    /// `BTreeMap` so a listing walks a stable, total order.
    pub ops_metadata: BTreeMap<(String, String), ops_metadata::OpsMetadataRecord>,

    /// Every custom inventory item a caller wrote, keyed by `(region, node id, type name)`:
    /// one item per inventory type per node, which is what makes a second put of that type
    /// an update of the item rather than a second one. See [`custom_inventory`].
    pub custom_inventory:
        BTreeMap<(String, String, String), custom_inventory::CustomInventoryItemRecord>,

    /// Every custom inventory TYPE a write has defined, keyed by `(region, type name)`: the
    /// type an item is written under is DEFINED for the Region by that write, and stays
    /// defined until a `DeleteInventory` of its SCHEMA removes it — a different question from
    /// whether any item of it is still held, since "the system only deletes existing
    /// inventory data associated with the custom inventory type" when no schema option is
    /// chosen. See [`custom_inventory`].
    pub custom_inventory_types: BTreeSet<(String, String)>,

    /// Every inventory deletion a `DeleteInventory` recorded, keyed by `(region, deletion
    /// id)`: the id a delete hands back resolves to the deletion it names, and the client
    /// token a delete carried is what makes a repeat of it the same deletion.
    pub inventory_deletions:
        BTreeMap<(String, String), custom_inventory::InventoryDeletionRecord>,

    /// Every compliance item a caller reported through `PutComplianceItems`, keyed by
    /// `(region, resource type, resource id, compliance type, position)`: a put states one
    /// resource and ONE compliance type and carries the full list of items for it, so the
    /// position is the item's place in that list. A `BTreeMap` so a listing walks a stable,
    /// total order. The compliance Patch Manager GATHERED holds no entry here and is
    /// projected from the patch reports instead ([`Self::compliance_items`], which reports
    /// both). See [`compliance_items`].
    pub written_compliance_items:
        BTreeMap<compliance_items::ComplianceItemKey, compliance_items::ComplianceItemRecord>,

    /// The `ItemContentHash` the last `PutComplianceItems` carried for each `(region, resource
    /// type, resource id, compliance type)`: "the content hash is used to determine if
    /// existing information should be overwritten or ignored. If the content hashes match, the
    /// request to put compliance information is ignored", so the hash belongs to the PUT
    /// rather than to one of the items it wrote. Held only for a pair that holds items, so
    /// this map is a subset of the one above and the ceiling on that one bounds both. See
    /// [`compliance_items`].
    pub compliance_content_hashes:
        BTreeMap<compliance_items::ComplianceContentHashKey, String>,

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

    /// The patch compliance state of each managed node a patching operation has reported
    /// one for, keyed by `(region, node id)`: a report is gathered on a node registered
    /// with one Region's endpoint, and the next patching operation on that node supersedes
    /// it, so one node holds at most one. A `BTreeMap` so a read walks a stable, total
    /// order — ascending by node id within one Region — and can resume a page from the id
    /// it stopped at rather than from an offset. Nothing a caller can send starts a
    /// patching operation here, so the store holds the reports that have been made and
    /// invents none: see [`instance_patch_states`].
    pub instance_patch_states:
        BTreeMap<(String, String), instance_patch_states::InstancePatchStateRecord>,

    /// Which patch baseline each PATCH GROUP is registered with, keyed by
    /// `(region, patch group, operating system)`: a group can be patched against a baseline
    /// of each operating system, which is why `GetPatchBaselineForPatchGroup` takes an
    /// `OperatingSystem` beside the group. The value is how the registered baseline is NAMED
    /// (`patch_baselines::ResolvedBaseline::reference`). A group nothing has registered a
    /// baseline for holds no entry and is patched against the default of its operating
    /// system instead. See [`patch_groups`].
    pub patch_group_baselines: BTreeMap<(String, String, String), String>,

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

    /// Every target registered with a maintenance window, keyed by
    /// `(region, window target id)`: a target is registered through the endpoint of the
    /// Region its window lives in and is identified there by the id the register minted,
    /// with the window it belongs to on the record. A `BTreeMap` so a listing walks a
    /// stable, total order. See [`maintenance_window_targets`].
    pub maintenance_window_targets: BTreeMap<
        (String, String),
        maintenance_window_targets::MaintenanceWindowTargetRecord,
    >,

    /// Every task registered with a maintenance window, keyed by `(region, window task id)`:
    /// a task is registered through the endpoint of the Region its window lives in and is
    /// identified there by the id the register minted, with the window it belongs to on the
    /// record. A `BTreeMap` so a listing walks a stable, total order. See
    /// [`maintenance_window_tasks`].
    pub maintenance_window_tasks:
        BTreeMap<(String, String), maintenance_window_tasks::MaintenanceWindowTaskRecord>,

    /// Every execution of a maintenance window this service has recorded, keyed by
    /// `(region, window execution id)`: a window runs at the endpoint of the Region it was
    /// created in and each of its runs is identified there by the id that run was executed
    /// under. A `BTreeMap` so a walk over them is stable and total. See
    /// [`maintenance_window_executions`], which is also where what puts a record here —
    /// and what does not — is stated.
    pub maintenance_window_executions: BTreeMap<
        (String, String),
        maintenance_window_executions::MaintenanceWindowExecutionRecord,
    >,

    /// Every task execution of a maintenance window run this service has recorded, keyed by
    /// `(region, window execution id, task execution id)`: a task execution belongs to ONE
    /// run, which is reached at the endpoint of the Region its window lives in, and is
    /// identified within that run by the id it ran under. A `BTreeMap` so a walk over them is
    /// stable and total. See [`maintenance_window_execution_tasks`], which is also where what
    /// puts a record here — and what does not — is stated.
    pub maintenance_window_execution_tasks: BTreeMap<
        (String, String, String),
        maintenance_window_execution_tasks::MaintenanceWindowExecutionTaskRecord,
    >,

    /// Every invocation of such a task execution, keyed by
    /// `(region, window execution id, task execution id, invocation id)`: an invocation
    /// belongs to ONE task execution of one run and is identified within it by the id it ran
    /// under. A `BTreeMap` so a walk over them is stable and total. See
    /// [`maintenance_window_execution_task_invocations`].
    pub maintenance_window_execution_task_invocations: BTreeMap<
        (String, String, String, String),
        maintenance_window_execution_task_invocations::MaintenanceWindowExecutionTaskInvocationRecord,
    >,

    /// Every patch snapshot this endpoint has taken, keyed by
    /// `(region, node id, snapshot id)`: a snapshot is taken through one Region's endpoint,
    /// for one node, under the id the caller asked for it with — one maintenance-window task
    /// hands the same snapshot id to many nodes, so the node is part of the key. A
    /// `BTreeMap` so a walk over them is stable and total. See [`patch_snapshots`], which is
    /// also where what puts a record here — and what does not — is stated.
    pub patch_snapshots:
        BTreeMap<(String, String, String), patch_snapshots::PatchSnapshotRecord>,

    /// Every Session Manager session, keyed by `(region, session id)`: a session is started
    /// through one Region's endpoint and is reached there by the id the start minted
    /// ([`sessions::mint_session_id`]). A `BTreeMap` so a report walks a stable, total
    /// order — ascending by session id within one Region — and can resume a page from the
    /// id it stopped at rather than from an offset. See [`sessions`].
    pub sessions: BTreeMap<(String, String), sessions::SessionRecord>,

    /// The secret this store's Parameter Store continuation tokens are authenticated
    /// under, drawn from the system's randomness when the store is created
    /// ([`parameter_pagination`]): it is what lets a paginated read tell a token it ISSUED
    /// from one a caller assembled. Never reported and never logged — its own `Debug`
    /// renders nothing of it, so this state's derived one carries nothing of it either.
    page_token_key: parameter_pagination::PageTokenKey,
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
            ops_metadata: BTreeMap::new(),
            custom_inventory: BTreeMap::new(),
            custom_inventory_types: BTreeSet::new(),
            inventory_deletions: BTreeMap::new(),
            written_compliance_items: BTreeMap::new(),
            compliance_content_hashes: BTreeMap::new(),
            service_settings: BTreeMap::new(),
            created_at: SystemTime::now(),
            resource_policies: resource_policies::ResourcePolicyStore::default(),
            associations: BTreeMap::new(),
            patch_baselines: BTreeMap::new(),
            patch_baseline_sequence: 0,
            default_patch_baselines: BTreeMap::new(),
            patch_group_baselines: BTreeMap::new(),
            instance_patch_states: BTreeMap::new(),
            patch_snapshots: BTreeMap::new(),
            resource_data_syncs: BTreeMap::new(),
            access_requests: BTreeMap::new(),
            access_request_sequence: 0,
            maintenance_windows: BTreeMap::new(),
            maintenance_window_sequence: 0,
            maintenance_window_targets: BTreeMap::new(),
            maintenance_window_tasks: BTreeMap::new(),
            maintenance_window_executions: BTreeMap::new(),
            maintenance_window_execution_tasks: BTreeMap::new(),
            maintenance_window_execution_task_invocations: BTreeMap::new(),
            sessions: BTreeMap::new(),
            // One key per store, from the system's randomness: see `parameter_pagination`.
            page_token_key: parameter_pagination::PageTokenKey::new(),
        }
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// The secret this store's Parameter Store continuation tokens are authenticated
    /// under.
    ///
    /// Handed out by value — `PageTokenKey` is `Copy` — so a read takes the key off the
    /// store and goes on to borrow the records it is about to report in the same call.
    pub fn page_token_key(&self) -> parameter_pagination::PageTokenKey {
        self.page_token_key
    }

    /// Materialise every transition the clock already owes at `now`, so a read reports
    /// what was due whether or not anybody called (see [`document_active_due`]).
    pub fn materialize_due(&mut self, now: SystemTime) {
        for document in self.documents.values_mut() {
            document.materialize_due(now);
        }
        // A command is reported for as long as Run Command keeps its history
        // (`commands::COMMAND_HISTORY_RETENTION`); one older than that was already gone
        // at `now`, whether or not anybody called.
        self.commands
            .retain(|_, command| now < command.retained_until());
        // And the invocations whose run had finished by `now` carry the status the plugin's
        // exit code gives them, with the command's own status read off them
        // (`commands::CommandRecord::materialize_due`): owed at that instant whether or not
        // anybody called.
        for command in self.commands.values_mut() {
            command.materialize_due(now);
        }
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
        // And so was the application an association owes a node that has registered since its
        // current run was dispatched ([`Self::settle_due_association_targets`]) — and the run
        // State Manager owes an association a version was applied to
        // ([`Self::dispatch_due_associations`]), dispatched first so the run the clock has
        // just started settles its own targets in the same pass.
        self.dispatch_due_associations(now);
        self.settle_due_association_targets();
    }

    /// The command `command_id` names in `region`, or `None` when that Region holds none.
    pub fn command(&self, region: &str, command_id: &str) -> Option<&commands::CommandRecord> {
        self.commands
            .get(&(region.to_string(), command_id.to_string()))
    }

    /// The same command, to write it — the cancel a caller asked for, say.
    pub fn command_mut(
        &mut self,
        region: &str,
        command_id: &str,
    ) -> Option<&mut commands::CommandRecord> {
        self.commands
            .get_mut(&(region.to_string(), command_id.to_string()))
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
        minted_uuid(ID_KIND_COMMAND, since_epoch.as_nanos(), self.minted_command_ids)
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
    /// name, the type and the publication dates — the schema version, the description, the
    /// parameters, the platform types, the hash, the document format — is derived by the
    /// one content reader that reads a created document's content. A published document is
    /// already
    /// registered, so every version of it is `Active` with no transition owed, it is owned
    /// by Amazon rather than by the account reading it, and each version is dated by the
    /// catalogue's own publication date for THAT version
    /// ([`PublishedDocument::published_versions`]) rather than by this store's beginning:
    /// nobody here created it. The default version is the newest the catalogue publishes
    /// ([`PublishedDocument::default_published_version`]).
    pub fn published_document_record(&self, region: &str, name: &str) -> Option<DocumentRecord> {
        let (published, parsed, hash, document_format) = published_document_projection(name)?;
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
            versions: published
                .published_versions()
                .into_iter()
                .map(|release| DocumentVersionRecord {
                    version: release.version,
                    display_name: None,
                    version_name: None,
                    // The format Amazon authored the document in, which is the format the
                    // catalogue's body for it is written in
                    // ([`PublishedDocument::document_format`]) rather than the one a create
                    // that declares none falls back to: a read of a document Amazon
                    // authored in YAML reports YAML.
                    document_format: document_format.to_string(),
                    // The target type is the catalogue's own
                    // ([`PublishedDocument::target_type`]): Amazon declares what kinds of
                    // resource a document it publishes runs on, so a read reports it and the
                    // listing's `TargetType` filter selects on it.
                    target_type: published.target_type.map(str::to_string),
                    content: published.content.to_string(),
                    hash: hash.to_string(),
                    parsed: parsed.clone(),
                    requires: None,
                    // A document Amazon publishes runs against Amazon Web Services
                    // resources ([`documents::DEFAULT_CLOUD_PROVIDER`]): the catalogue
                    // declares no other provider for one.
                    cloud_providers: vec![DEFAULT_CLOUD_PROVIDER.to_string()],
                    attachment_names: Vec::new(),
                    created_date: ::std::time::UNIX_EPOCH
                        + ::std::time::Duration::from_millis(release.created_date_millis),
                    status: DOCUMENT_STATUS_ACTIVE.to_string(),
                    active_due: None,
                })
                .collect(),
            default_version: published.default_published_version(),
            // A document Systems Manager publishes is Amazon's to share and already
            // reaches every account, so the account reading it has recorded no share of
            // its own on it.
            shared_with: BTreeMap::new(),
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

    /// Whether `region` holds a node under `id` whose AGENT IS RESPONDING: the node pinged
    /// Systems Manager to report itself, which is what [`PING_STATUS_ONLINE`] records.
    ///
    /// The one reading of whether a node is reachable through this service, so every
    /// operation that needs it agrees: a session is opened only to such a node
    /// (`StartSession`, else `TargetNotConnected`), a command is delivered only to one
    /// (`SendCommand`, else `InvalidInstanceId`) and `GetConnectionStatus` reports exactly
    /// these nodes `connected`. A node this Region holds none of — and a node held per
    /// Region is held by one — is not reachable, and neither is one whose agent has stopped
    /// reporting.
    pub fn node_agent_is_responding(&self, region: &str, id: &str) -> bool {
        self.managed_node(region, id)
            .and_then(|node| node.properties.ping_status.as_deref())
            .is_some_and(|status| status == PING_STATUS_ONLINE)
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

    /// Drop the managed node `id` names in `region`, answering whether one was held.
    ///
    /// This is a node DEREGISTRATION — "removes the server or virtual machine from the list
    /// of registered servers" (`DeregisterManagedInstance`) — so the node goes whole, with
    /// the tags it carried; an id the Region holds no node under is what the deregister
    /// refuses as an invalid node id.
    pub fn remove_managed_node(&mut self, region: &str, id: &str) -> bool {
        self.managed_nodes
            .remove(&(region.to_string(), id.to_string()))
            .is_some()
    }
}

/// `value` when a request set the member to something, and `None` when it set it to
/// nothing: an optional member sent EMPTY names no document version, no schedule, no node
/// and no association, the same way an omitted one does. The one reading of that across
/// this service's handlers.
pub fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// The instant a caller-supplied timestamp names, or `None` for a value that names none:
/// the ISO 8601 form an SDK renders a timestamp as, or the epoch seconds one sends when it
/// sends a number.
///
/// THE ONE reading of such a value across this service — the time keys an OpsItem query
/// compares against ([`ops_item_queries`]) and the two instant keys a session report filters
/// by ([`sessions`]) — so two members of this service cannot accept different spellings of
/// one instant.
pub fn request_instant(value: &str) -> Option<SystemTime> {
    for format in [
        ::aws_smithy_types::date_time::Format::DateTime,
        ::aws_smithy_types::date_time::Format::EpochSeconds,
    ] {
        if let Some(at) = ::aws_smithy_types::DateTime::from_str(value, format)
            .ok()
            .and_then(|at| SystemTime::try_from(at).ok())
        {
            return Some(at);
        }
    }
    None
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
/// Each set is the modelled enum the property the key selects on targets, and the ORDER is
/// the order the service publishes that key's values in — one row per key, because the two
/// places this service's model states a set do not always agree and the service follows
/// neither of them for every key:
///
/// * `AssociationStatus` is listed as the modelled `AssociationStatusName` enum DECLARES
///   its values (`smithy_query(query="shape", shape="AssociationStatusName")`:
///   `Pending`, `Success`, `Failed`), which is the order recorded for this refusal
///   (corpus `simparity-wire-aws-v1`, fixture `ssm-w22n-traj-filter-value-enum-casing`,
///   step `task#6`) and NOT the order the filter-key doc's prose lists them in.
/// * `ResourceType` is listed as the `InstanceInformationStringFilter$Key` doc publishes it
///   ("Valid values for the `ResourceType` filter key: EC2Instance | ManagedInstance"),
///   which is the order recorded for it (same fixture, step `task#5`) and the reverse of
///   the modelled `ResourceType` declaration.
/// * `PlatformTypes` is listed as both state it (`Windows`, `Linux`, `MacOS`), recorded in
///   fixture `ssm-w22n-traj-instance-properties-route`, step `task#2`.
/// * `PingStatus` and `SourceTypes` have no recorded listing of their own, so each is
///   listed as that same filter-key doc publishes it — the list a caller reads to pick a
///   legal value.
///
/// The generated `*_VALUES` constants carry the same values sorted, so they pin the SET and
/// this listing the order; a test holds the two to the same set, so a model revision cannot
/// leave this listing behind.
pub fn node_filter_key_value_set(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "AssociationStatus" => Some(&["Pending", "Success", "Failed"]),
        "PingStatus" => Some(&["Online", "ConnectionLost", "Inactive"]),
        "PlatformTypes" => Some(&["Windows", "Linux", "MacOS"]),
        "ResourceType" => Some(&["EC2Instance", "ManagedInstance"]),
        "SourceTypes" => Some(&[
            "AWS::EC2::Instance",
            "AWS::SSM::ManagedInstance",
            "AWS::IoT::Thing",
            "Microsoft.Compute/virtualMachines",
            "compute.googleapis.com/Instance",
        ]),
        _ => None,
    }
}

/// How a managed-node describe refuses a filter whose VALUE is outside the set its key
/// publishes: the key the caller filtered on, then the values that key accepts, in the
/// order the model declares them ([`node_filter_key_value_set`]).
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

/// The shortest a managed-node id can be, as a refusal reports the constraint.
///
/// The `InstanceId` shape carries a LENGTH beside its pattern in the live service, which
/// this checkout's model does not declare: a value shorter than this is refused for the
/// length as well as for the pattern, and a refusal states both.
///
/// RECORDED BEHAVIOUR (corpus `simparity-wire-aws-v1`, fixtures
/// `ssm-vscan-traj-inventory-entries-type-and-id-mistakes` and
/// `ssm-vscan-traj-patch-and-compliance-read-mistakes`): an `InstanceId` of `i-zzz` is
/// refused "2 validation errors detected: … Member must have length greater than or equal
/// to 10; … Member must satisfy regular expression pattern: …", and the aggregated clause a
/// LIST of them carries lists both bounds beside the pattern ("Member must have length less
/// than or equal to 20, Member must have length greater than or equal to 10, …"). The bounds
/// are reported, never enforced: which values the shape ADMITS is the modelled `@pattern`
/// alone, which the framework's own walk decides before any wording is chosen.
pub const MANAGED_NODE_ID_MIN_LENGTH: i64 = 10;

/// The longest a managed-node id can be, the other half of that length — the longest id the
/// pattern admits is `mi-` and 17 characters, which is exactly this many. Recorded in the
/// same place as [`MANAGED_NODE_ID_MIN_LENGTH`], and reported the same way.
pub const MANAGED_NODE_ID_MAX_LENGTH: i64 = 20;

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

/// How `DescribeInstanceInformation` refuses a request that names BOTH its LEGACY filter
/// list and its current filter member — `InstanceInformationFilterList` beside `Filters`.
///
/// The two members are alternatives rather than a union ("Attempting to use
/// InstanceInformationFilterList and Filters leads to an exception error"), and the sentence
/// names NEITHER of them: it states that the legacy and the current filter cannot be used
/// together, which is the same answer whatever the two lists held.
///
/// RECORDED BEHAVIOUR (corpus `simparity-wire-aws-v1`, fixture
/// `ssm-w22n-traj-filter-combination-mistakes`, step `task#1`): a
/// `DescribeInstanceInformation` naming both members is answered 400 `ValidationException`
/// with exactly this sentence, terminating period included.
pub const LEGACY_AND_NEW_FILTERS_REJECTION: &str =
    "Cannot use both legacy and new filter simultaneously.";

/// The same refusal as the PROPERTY describe words it — `InstancePropertyFilterList` beside
/// `FiltersWithOperator`: the same sentence, which that operation ends without a
/// terminating period.
///
/// The rule the two operations share is the one [`LEGACY_AND_NEW_FILTERS_REJECTION`] states;
/// what differs is punctuation, and each operation is worded as its OWN recording words it
/// rather than as a guess from the other's. RECORDED BEHAVIOUR (corpus
/// `simparity-wire-aws-v1`, fixture `ssm-w22n-traj-instance-properties-route`, step
/// `task#3`): a `DescribeInstanceProperties` naming both members is answered 400
/// `ValidationException` with exactly this sentence.
pub const PROPERTY_LEGACY_AND_NEW_FILTERS_REJECTION: &str =
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
    /// Whether this node's registration is the one an Amazon EC2 instance of this account
    /// makes by running with an instance profile that lets its agent register
    /// ([`node_registration`]).
    ///
    /// Such a registration is DERIVED from the instance, so it is also withdrawn with it:
    /// it is the one kind of node [`node_registration::settle_registrations`] may drop
    /// when the instance behind it stops being a managed node. A node registered any other
    /// way — a hybrid activation's — carries `false` and is never dropped by that walk.
    pub registered_from_instance: bool,
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
            // targets. Which documents a node is associated with is not a fact of the
            // node record — the associations are a separate store, resolved against the
            // nodes when a run is dispatched
            // ([`ComAmazonawsSsmAmazonssmState::association_run_nodes`]) — so a filter on
            // it selects nothing here, which is the record's answer, not a refusal.
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
        let at = wire_instant(at);
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
            // Whatever registers the node says how: an instance's own registration sets
            // this ([`node_registration`]), and every other registration leaves it.
            registered_from_instance: false,
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

/// How far ahead of now an activation may be asked to expire: "You can specify a date up
/// to 30 days in advance" (`CreateActivationRequest$ExpirationDate`).
pub const MAX_ACTIVATION_LEAD: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How Systems Manager refuses an `ExpirationDate` outside that window — entirely fixed
/// text, naming neither the date the request asked for nor the bound it broke. The bound
/// the sentence states is [`MAX_ACTIVATION_LEAD`].
pub const ACTIVATION_EXPIRATION_OUT_OF_WINDOW: &str =
    "expirationDate must be greater than now and lesser than 30 days.";

/// Whether `requested` is a date an activation cannot be asked to expire at, as of `now`.
///
/// The window is the documented one, open at both ends: an expiry already reached (or
/// reached exactly now) would mint an activation no node could ever register with, and one
/// further ahead than [`MAX_ACTIVATION_LEAD`] is past "a date up to 30 days in advance".
/// A request naming no date at all is not held to it — the service picks that date itself
/// ([`DEFAULT_ACTIVATION_LIFETIME`]), inside the window by construction.
pub fn activation_expiration_out_of_window(requested: SystemTime, now: SystemTime) -> bool {
    requested <= now || requested > now + MAX_ACTIVATION_LEAD
}

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
            expiration_date: Some(wire_instant(self.expiration_date)),
            expired: self.is_expired(now),
            created_date: Some(wire_instant(self.created_date)),
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

/// The kind of resource a `CommandId` identifies, as [`minted_uuid`] reads it.
///
/// The kinds below are the id SPACES this service mints into. Each one is a different kind
/// of resource, identified by its own id and reached through its own operations, so an id
/// of one kind names no resource of another however alike the two ids are shaped: a Run
/// Command command id and an Automation execution id are both 36-character UUIDs and
/// neither resolves in the other's API.
pub const ID_KIND_COMMAND: &str = "command";

/// The kind an automation execution's (and its step executions') id identifies
/// ([`ID_KIND_COMMAND`]).
pub const ID_KIND_AUTOMATION_EXECUTION: &str = "automation-execution";

/// The kind an execution preview's id identifies ([`ID_KIND_COMMAND`]).
pub const ID_KIND_EXECUTION_PREVIEW: &str = "execution-preview";

/// The kind a resource policy's `PolicyId` identifies ([`ID_KIND_COMMAND`]).
pub const ID_KIND_RESOURCE_POLICY: &str = "resource-policy";

/// The kind an OpsItem event's id identifies ([`ID_KIND_COMMAND`]).
pub const ID_KIND_OPS_ITEM_EVENT: &str = "ops-item-event";

/// The kind an OpsItem related-item association's id identifies ([`ID_KIND_COMMAND`]).
pub const ID_KIND_OPS_ITEM_RELATED_ITEM: &str = "ops-item-related-item";

/// This service's ONE minter of an opaque, UUID-shaped identifier: the Sha256 of the kind
/// of resource being identified, an instant and a mint counter, rendered as a UUID.
///
/// Every id this service mints for a resource it creates goes through here — a
/// `CommandId` ([`ComAmazonawsSsmAmazonssmState::mint_command_id`]), a resource policy's
/// `PolicyId` ([`resource_policies::ResourcePolicyStore`]) — so the digest and the
/// layout are written once. The 16 bytes are the digest's first half and the UUID LAYOUT
/// (the group boundaries and the version and variant bits) is `service_common`'s one
/// rendering of 16 opaque bytes, so neither is restated per caller.
///
/// `kind` names the RESOURCE KIND the id is for ([`ID_KIND_COMMAND`] and its siblings), and
/// it is part of the digest because every kind this service identifies holds its own mint
/// counter: two kinds' counters stand at the same figure as often as not, so without the
/// kind in the digest the first command and the first automation execution of one store
/// would carry the SAME id and an operation that resolves one kind would resolve an id of
/// the other. The kinds are disjoint id spaces — an id names a resource of one kind and of
/// no other — which is what lets each operation refuse an id it does not own. The sibling
/// minter of a prefixed hexadecimal id ([`minted_prefixed_hex_id`]) is kept disjoint the
/// same way, by the prefix each kind's ids carry.
///
/// `seed_nanos` is an instant of the store's own (its creation, or now) and `counter` the
/// number of ids that minter has handed out, so two ids of one run differ, consecutive
/// ones are unrelated, and the value is not a count a caller can read off the wire.
/// Nothing of the request is in it.
pub fn minted_uuid(kind: &str, seed_nanos: u128, counter: u64) -> String {
    use ::sha2::Digest;

    let mut digest = ::sha2::Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0u8]);
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
        .filter(|region| region_is_well_formed(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` could be the name of a Region: non-empty, no longer than the modelled
/// `NodeRegion` (`len 1..64`), and carrying nothing outside `[A-Za-z0-9-]`.
///
/// THE ONE reading of that across this service, because a Region name lands inside an ARN
/// and in a store key: it decides which credential scope [`request_region`] honours, and
/// which page cursor of a node listing names a Region this store could hold nodes under
/// ([`node_listing::page_cursor`]).
pub fn region_is_well_formed(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 64
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
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
