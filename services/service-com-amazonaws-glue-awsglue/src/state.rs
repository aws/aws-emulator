// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-glue-awsglue service.
//!
//! The Data Catalog is the service's primary resource tree: a catalog (keyed by
//! `CatalogId`, which defaults to the request's account id) holds databases, and a
//! database holds tables. Every handler reads and writes THIS store, so a
//! `CreateTable` in one request is visible to a `GetTable` in the next. A stored table
//! carries its own version history beside its live definition
//! ([`TableEntry::archived_versions`]), so a read of one version serves the definition
//! that version was written with and not what a later write replaced it by. A table also
//! holds its PARTITIONS ([`TableEntry::partitions`]), the slices of its data a partition
//! create registers, because a partition is addressed by its values within one table.
//!
//! Beside the catalog the store holds the two account-level resources an ETL client
//! registers: JOB DEFINITIONS (`CreateJob` writes one, `GetJob` reads it back) and, inside
//! each catalog, CONNECTION definitions (`CreateConnection` / `GetConnection`). A job is
//! named per account ("The name you assign to this job definition. It must be unique in
//! your account" — `CreateJobRequest$Name`) and a connection per catalog, which is where
//! each lives here; because Glue is a regional service, the job namespace is keyed by the
//! REGION the create was served in as well, so a job is reachable only through the
//! endpoint that created it. A job definition also holds the RUNS `StartJobRun` started
//! of it ([`JobRunEntry`]), because every operation that addresses a run names its job too.
//!
//! A catalog also holds ONE security configuration of its own
//! ([`Catalog::encryption_settings`]): the encryption settings a write set for it, which
//! every later read of that catalog's settings reports. What that setting does to the
//! catalog's OBJECTS is recorded on each object: a database, table or partition written
//! while the catalog encrypts its writes under a KMS key carries the key material that
//! write was encrypted under ([`catalog_encryption::ObjectEncryption`]), because the
//! setting applies "to every catalog write thereafter" and not to what is already stored.
//!
//! It holds ONE migration record in the same way ([`Catalog::migration`]): the import of
//! an Athena-managed data catalog an `ImportCatalogToGlue` performed into it, which is
//! what a later `GetCatalogImportStatus` of that catalog reports.
//!
//! Crawlers sit BESIDE that tree too, in one per-account namespace keyed by crawler name:
//! every operation that addresses a crawler does so by name alone, so a crawler's
//! `CatalogId` says where its output goes, not where the crawler itself lives.
//!
//! CLASSIFIERS — the custom classifiers `CreateClassifier` registers for a crawler to
//! recognise a data format with — sit in such a per-account namespace beside the crawlers
//! that refer to them ([`ComAmazonawsGlueAwsglueState::classifiers`]), keyed by classifier
//! name, because every operation that addresses one (`GetClassifier`, `UpdateClassifier`,
//! `DeleteClassifier`) names it by `Name` alone and carries no `CatalogId`. They follow
//! the crawler's per-account convention rather than the job's per-Region one because a
//! crawler's `Classifiers` list refers to a classifier by name alone, so the two have to
//! share one namespace. The four KINDS the model declares share that namespace rather
//! than having one each: a read takes a name alone and answers with whichever kind that
//! name holds ([`ClassifierEntry`]).
//!
//! COLUMN STATISTICS TASK RUNS — the runs `StartColumnStatisticsTaskRun` starts over one
//! table — sit beside the tree in the same way, in a per-account list
//! ([`ComAmazonawsGlueAwsglueState::column_statistics_task_runs`]): a run is ADDRESSED by
//! the id the start mints (`GetColumnStatisticsTaskRun` takes that id and nothing else),
//! so the table a run covers is recorded on the run rather than the run on the table.
//!
//! The SETTINGS those runs are scheduled by (`CreateColumnStatisticsTaskSettings`) sit the
//! other way round — ON the table, like its partitions
//! ([`TableEntry::column_statistics_task_settings`]) — because every operation that
//! addresses one settings record names its catalog, its database and then its table and
//! nothing else, so there is at most one of them per table.
//!
//! The SCHEMA REGISTRY is the store's second resource tree and is independent of the
//! catalog: a registry (keyed by name, per account) holds schema sets, and a schema set
//! holds the versions registered against it. It is keyed by name rather than by
//! `CatalogId` because every schema-registry operation addresses its resource by a
//! `RegistryId`/`SchemaId` wrapper carrying a name or an ARN and no catalog id at all.
//!
//! CUSTOM ENTITY TYPES — the custom sensitive-data patterns `CreateCustomEntityType`
//! registers — sit in the same kind of per-account namespace, keyed by pattern name:
//! "This name must be unique per Amazon Web Services account"
//! (`CustomEntityType$Name`), and every operation that addresses one names it by name
//! alone, with no `CatalogId` member.
//!
//! SECURITY CONFIGURATIONS — the sets of encryption properties
//! `CreateSecurityConfiguration` registers — sit in such a per-account namespace too
//! ([`ComAmazonawsGlueAwsglueState::security_configurations`]), keyed by configuration
//! name, because every operation that addresses one (`GetSecurityConfiguration`,
//! `DeleteSecurityConfiguration`) names it by `Name` alone and no `CatalogId`. This is
//! a DIFFERENT resource from [`Catalog::encryption_settings`] above: those settings are
//! the catalog's own, while a security configuration is a named record a job, crawler or
//! development endpoint refers to.
//!
//! FORM TYPES — the metadata schemas `PutFormType` registers — sit in such a per-account
//! namespace too, keyed by form type name, because a put names its form type by `Name`
//! alone and no `CatalogId`. They are ADDRESSED by the identifier a client holds for one —
//! the `FormTypeId` a put reports, which IS that name ([`FormTypeEntry::id`]) — which is
//! the shape every operation that reads, deletes or attaches to one takes.
//!
//! ASSET TYPES — the asset structures `PutAssetType` registers, each naming the forms an
//! asset of that type includes — sit in their own per-account namespace beside the form
//! types they reference ([`ComAmazonawsGlueAwsglueState::asset_types`]), keyed by asset
//! type name for the same reason: a put names its asset type by `Name` alone and no
//! `CatalogId`. Each of its form references carries the `FormTypeId` of a form type in
//! this same store, and the asset type is itself ADDRESSED by the opaque `AssetTypeId` the
//! first put under a name mints.
//!
//! ASSETS — the catalog entities `PutAsset` registers, each one an instance of an asset
//! type carrying its metadata forms — sit in their own per-account namespace beside the
//! asset types they are instances of ([`ComAmazonawsGlueAwsglueState::assets`]). They are
//! keyed by the `AssetId` the PUT's caller chose rather than by a minted id, because that
//! identifier is what every operation addressing an asset carries (`PutAsset$Identifier`,
//! `GetAsset$Identifier`, `AssociateGlossaryTerms$AssetIdentifier`) and it is what decides
//! whether a put creates an asset or updates the one already stored. The asset type an
//! asset is an instance of is recorded ON the asset ([`AssetEntry::asset_type_id`]), as the
//! put addressed it.
//!
//! BUSINESS GLOSSARIES — the containers for business-concept terms `CreateGlossary`
//! registers — sit beside that tree in another per-account namespace
//! ([`ComAmazonawsGlueAwsglueState::glossaries`]). They are keyed by the opaque
//! `GlossaryId` the create mints, because every operation that addresses a glossary takes
//! an `Identifier` targeting that shape and no `CatalogId`; the NAME a create carries is a
//! separate uniqueness constraint on the namespace ([`ComAmazonawsGlueAwsglueState::glossary_by_name`]),
//! which is what `CreateGlossary`'s modelled `AlreadyExistsException` refuses a second
//! create under.
//!
//! GLOSSARY TERMS — the business concepts `CreateGlossaryTerm` registers within a glossary
//! — sit BESIDE the glossaries in their own per-account namespace
//! ([`ComAmazonawsGlueAwsglueState::glossary_terms`]) keyed by the opaque `GlossaryTermId`
//! the create mints, because every operation that reads, updates or deletes a term
//! addresses it by that id alone and names no glossary; the glossary a term belongs to is
//! recorded ON the term ([`GlossaryTermEntry::glossary_id`]), and the term's NAME is a
//! uniqueness constraint within that one glossary
//! ([`ComAmazonawsGlueAwsglueState::glossary_term_by_name`]), which is what
//! `CreateGlossaryTerm`'s modelled `AlreadyExistsException` refuses a second create under.
//! Which terms an ASSET carries is recorded on the asset instead
//! ([`AssetEntry::glossary_terms`]), as the set of ids its associations named: the
//! association is to the id, so the two namespaces are independent and a term deleted
//! after being associated leaves its id on the asset.
//!
//! RESOURCE POLICIES — the IAM policy documents `PutResourcePolicy` attaches to a Glue
//! resource — sit beside the catalog tree in one per-account namespace keyed by the
//! resource's ARN ([`ComAmazonawsGlueAwsglueState::resource_policies`]). The ARN is the
//! key because every operation that addresses a policy addresses it by `ResourceArn`
//! alone, with no `CatalogId` member, and a request that names none addresses the Data
//! Catalog's own policy ("If not supplied, the Data Catalog resource policy is returned"
//! — `GetResourcePolicyRequest$ResourceArn`), which is therefore keyed by the Data
//! Catalog's own ARN ([`catalog_arn`]).
//!
//! INTEGRATION RESOURCE PROPERTIES — the processing properties
//! `CreateIntegrationResourceProperty` sets on the Glue connection an integration reads
//! from, or on the Glue database it writes into — sit in such a per-account namespace too,
//! keyed by that resource's ARN
//! ([`ComAmazonawsGlueAwsglueState::integration_resource_properties`]), for the reason a
//! resource policy is: every operation that addresses one record names it by `ResourceArn`
//! alone. One record per resource, so a second create for a resource that already has one
//! writes THAT record rather than adding a second.
//!
//! INTEGRATION TABLE PROPERTIES — the override properties
//! `CreateIntegrationTableProperties` sets for ONE table that an integration replicates —
//! sit in the same kind of per-account namespace
//! ([`ComAmazonawsGlueAwsglueState::integration_table_properties`]), keyed by the
//! `ResourceArn` the request named AND the `TableName` it carried, because those two
//! members are the whole address every operation that reaches one record
//! (`CreateIntegrationTableProperties`, `GetIntegrationTableProperties`,
//! `UpdateIntegrationTableProperties`, `DeleteIntegrationTableProperties`) takes. One
//! record per (resource, table) pair, so a second create for a pair that already has one
//! writes THAT record.
//!
//! WORKFLOWS — the containers for the dependent jobs and crawlers `CreateWorkflow`
//! registers — sit beside that tree in a per-account namespace keyed by workflow name
//! ([`ComAmazonawsGlueAwsglueState::workflows`]): "The name to be assigned to the
//! workflow. It should be unique within your account" (`CreateWorkflowRequest$Name`), and
//! every operation that addresses a workflow (`GetWorkflow`, `UpdateWorkflow`,
//! `DeleteWorkflow`, `StartWorkflowRun`) names it by `Name` alone, with no `CatalogId`
//! member. A workflow holds the RUNS `StartWorkflowRun` started of it
//! ([`WorkflowEntry::runs`]), because every operation that addresses a run names its
//! workflow too (`GetWorkflowRun` takes a `Name` beside the `RunId`).
//!
//! BLUEPRINTS — the reusable workflow templates a client registers with
//! `CreateBlueprint` — sit beside that tree in another per-account namespace keyed by
//! blueprint name ([`ComAmazonawsGlueAwsglueState::blueprints`]), because every operation
//! that addresses a blueprint (`GetBlueprint`, `UpdateBlueprint`, `DeleteBlueprint`,
//! `StartBlueprintRun`) names it by `Name` alone and carries no `CatalogId`, and
//! `ListBlueprints` "lists all the blueprint names in an account". Ordered, so that
//! listing has one deterministic order to page over.
//!
//! DATA QUALITY RULE RECOMMENDATION RUNS — the runs `StartDataQualityRuleRecommendationRun`
//! starts over one data source (a catalog table) — sit beside the catalog tree in the same
//! per-account list as the column statistics task runs
//! ([`ComAmazonawsGlueAwsglueState::data_quality_rule_recommendation_runs`]): a run is
//! ADDRESSED by the `RunId` the start mints, which
//! `GetDataQualityRuleRecommendationRun` takes and nothing else, so the data source a run
//! covers is recorded on the run rather than the run on the table.
//!
//! DATA QUALITY RULESET EVALUATION RUNS — the runs
//! `StartDataQualityRulesetEvaluationRun` starts of stored rulesets over one data source —
//! sit beside the catalog tree in their own per-account list
//! ([`ComAmazonawsGlueAwsglueState::data_quality_ruleset_evaluation_runs`]), for the same
//! reason a recommendation run does: `GetDataQualityRulesetEvaluationRun` addresses a run
//! by the `RunId` the start minted and nothing else, so the data source and the rulesets a
//! run evaluates are recorded on the run.
//!
//! DATA QUALITY RULESETS — the DQDL rule sets `CreateDataQualityRuleset` registers — sit
//! beside the catalog tree in a per-account namespace keyed by ruleset name
//! ([`ComAmazonawsGlueAwsglueState::data_quality_rulesets`]), because every operation that
//! addresses one (`GetDataQualityRuleset`, `UpdateDataQualityRuleset`,
//! `DeleteDataQualityRuleset`) names it by `Name` alone and carries no `CatalogId`. The
//! Glue table a ruleset is written against is recorded ON the ruleset, the way a
//! recommendation run records its data source, because the ruleset's name is its whole
//! address and the target table is optional.
//!
//! USAGE PROFILES — the job and session parameter sets an admin registers with
//! `CreateUsageProfile` — sit beside the catalog tree in another per-account namespace
//! keyed by profile name ([`ComAmazonawsGlueAwsglueState::usage_profiles`]), because every
//! operation that addresses a profile names it by `Name` alone and carries no `CatalogId`.
//!
//! TRIGGERS — the records `CreateTrigger` registers, each naming the jobs and crawlers it
//! initiates — sit beside the catalog tree too
//! ([`ComAmazonawsGlueAwsglueState::triggers`]), keyed by the REGION they were created in
//! and then by trigger name: every operation that addresses a trigger names it by `Name`
//! alone with no `CatalogId`, and the jobs a trigger's actions run are themselves reachable
//! only through the Region that created them, so a trigger is served by the endpoint that
//! created it exactly as a job definition is.
//!
//! MACHINE LEARNING TRANSFORMS — the transforms `CreateMLTransform` registers — sit beside
//! that tree in a per-account namespace keyed by the opaque `TransformId` the create mints
//! ([`ComAmazonawsGlueAwsglueState::ml_transforms`]), because every operation that
//! addresses one takes that id alone and no `CatalogId`, and a transform's NAME is no
//! address at all: "Names are not guaranteed unique and can be changed at any time"
//! (`MLTransform$Name`). That name is still the one the CREATE's client chooses, and a
//! create has to choose a free one ([`ComAmazonawsGlueAwsglueState::ml_transform_by_name`]).
//! The catalog TABLES a transform reads are recorded
//! on the transform as the create named them ([`MlTransformEntry::input_record_tables`]),
//! which is where its reported schema is taken from.
//!
//! INTERACTIVE SESSIONS — the remote Spark runtime environments `CreateSession` provisions
//! ([`ComAmazonawsGlueAwsglueState::sessions`]) — sit beside the catalog tree, keyed by the
//! REGION they were created in and then by the `Id` the create chose: every operation that
//! addresses a session names it by `Id` alone with no `CatalogId`, and the runtime a session
//! is is provisioned in the Region its create was served in, so a session is served by the
//! endpoint that created it exactly as a job definition is. A session's status is not final
//! when the create returns — the environment is still being provisioned — so the record
//! carries the instant the clock owes it the next status ([`SessionEntry::settles_at`]) and
//! the instant its own timeouts fall due, and whichever later call reads it materialises
//! what the clock had already reached.
//!
//! A session also holds the STATEMENTS run on it ([`SessionEntry::statements`]), in the
//! order they were submitted, because every operation that addresses a statement names its
//! session and then an `Id` the session itself mints — so a statement is a record of the
//! session it was run on and not an account-level resource. A statement that has not
//! finished is what makes its session busy, which is what a further run of that session is
//! refused by.
//!
//! Database and table names are folded to lowercase on the way in — the model says
//! so for both `DatabaseInput$Name` ("For Hive compatibility, this is folded to
//! lowercase when it is stored") and `TableInput$Name` — and every lookup folds the
//! requested name the same way, so the fold cannot make a resource unreachable. A JOB,
//! CONNECTION or CRAWLER name is NOT folded: the model documents the fold for the two
//! Hive-compatible catalog resources only.

// The records this state holds are grouped one module per resource family, so each file
// carries one concern; the struct that owns them all stays here. Every module is glob
// re-exported, so a record, a constant or a helper keeps resolving as `crate::state::<name>`
// wherever it lives.
pub mod catalog_encryption;
/// Whether the caller holds the Lake Formation permission a catalog metadata read needs;
/// reached as `crate::state::lake_formation` rather than glob re-exported, because what it
/// answers is a decision about a request and not a record this store holds.
pub mod lake_formation;
mod blueprints;
mod catalog;
mod classifiers;
mod clock_pump;
mod column_statistics;
mod column_statistics_source;
mod column_statistics_values;
mod crawl;
mod crawl_source;
mod crawler_events;
mod crawler_roles_and_connections;
mod crawlers;
mod data_assets;
mod data_catalog_export;
mod data_quality_results;
mod data_quality_rules;
mod hive_registration;
mod hive_types;
mod identity_center;
mod integrations;
mod job_run_events;
mod job_script;
mod jobs;
mod lookups;
mod ml_transforms;
mod partition_predicate;
mod schema_compatibility;
mod schema_registry;
mod security_configurations;
mod sessions;
mod table_rows;
mod triggers;
mod workflow_runs;
mod workflows;

pub use blueprints::*;
pub use catalog::*;
pub use classifiers::*;
pub use clock_pump::*;
pub use column_statistics::*;
pub use column_statistics_source::*;
pub use column_statistics_values::*;
pub use crawl::*;
pub use crawl_source::*;
pub use crawler_events::*;
pub use crawler_roles_and_connections::*;
pub use crawlers::*;
pub use data_assets::*;
pub use data_catalog_export::*;
pub use data_quality_results::*;
pub use data_quality_rules::*;
pub use hive_registration::*;
pub use hive_types::*;
pub use identity_center::*;
pub use integrations::*;
pub use job_run_events::*;
pub use job_script::*;
pub use jobs::*;
pub use lookups::*;
pub use ml_transforms::*;
pub use partition_predicate::*;
pub use schema_compatibility::*;
pub use schema_registry::*;
pub use security_configurations::*;
pub use sessions::*;
pub use table_rows::*;
pub use triggers::*;
pub use workflow_runs::*;
pub use workflows::*;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use self::catalog_encryption::ObjectEncryption;

use crate::ops::types::{
    Action, AssetFormEntry, AssetFormRecord, AssetTypeFormReference, CatalogImportStatus,
    CatalogInput,
    CatalogProperties,
    CatalogPropertiesOutput,
    Classifier,
    BinaryColumnStatisticsData, BooleanColumnStatisticsData,
    CodeGenConfigurationNode, Column, ColumnError, ColumnStatistics, ColumnStatisticsData,
    ColumnStatisticsTaskRun, ColumnStatisticsTaskSettings,
    DateColumnStatisticsData, DecimalColumnStatisticsData, DecimalNumber,
    DoubleColumnStatisticsData, LongColumnStatisticsData, StringColumnStatisticsData,
    ConfigurationObject,
    Condition,
    Connection, ConnectionInput,
    ConnectionPasswordEncryption, ConnectionsList, Crawler, CrawlerMetrics, CrawlerTargets,
    CreateClassifierInput,
    CreateColumnStatisticsTaskSettingsInput, CreateCrawlerInput,
    CreateCustomEntityTypeInput, CreateDataQualityRulesetInput, CreateGlossaryInput,
    CreateGlossaryTermInput,
    CreateIntegrationResourcePropertyInput, CreateIntegrationTablePropertiesInput, CreateJobInput,
    CreateIcebergTableInput, IcebergSchema, JobBookmarkEntry, TableOptimizer,
    TableOptimizerConfiguration,
    CreateMLTransformInput, CrawlerNodeDetails, CsvClassifier,
    CreateSecurityConfigurationInput, CreateSessionInput, CreateTriggerInput, CreateWorkflowInput,
    CustomEntityType,
    DataCatalogEncryptionSettings, DataLakeAccessPropertiesOutput, DataLakePrincipal,
    DataQualityRuleRecommendationRunAdditionalRunOptions, DataQualityEvaluationRunAdditionalRunOptions,
    DataQualityRulesetFilterCriteria,
    DataQualityRulesetListDetails, DataQualityTargetTable,
    DataQualityRuleResult,
    DataSource, Database, DatabaseIdentifier, DatabaseInput,
    Edge, EncryptionAtRest, EncryptionConfiguration, ErrorDetail, EventBatchingCondition,
    ExportEncryptionConfiguration,
    ExecutionAttempt, ExecutionProperty, FederatedCatalog, FederatedDatabase,
    FederatedTable, GlueTable, GrokClassifier, IcebergOptimizationPropertiesOutput, Job,
    JsonClassifier, LastCrawlInfo, MLTransform,
    CreateUsageProfileInput,
    JobCommand, JobNodeDetails, JobRun, JobUpdate, LakeFormationConfiguration,
    LineageConfiguration,
    Node, NotificationProperty, Partition, PartitionInput, Predecessor, Predicate,
    PrincipalPermissions,
    ProfileConfiguration,
    RLSProperties,
    RecrawlPolicy, RegistryId, S3Target, Schedule,
    SchemaChangePolicy, SchemaColumn, SchemaId, SecurityConfiguration, SerDeInfo, Session,
    SessionCommand,
    SourceControlDetails,
    SourceProcessingProperties, SourceTableConfig,
    StartColumnStatisticsTaskRunInput,
    StartDataQualityRuleRecommendationRunInput,
    StartDataQualityRulesetEvaluationRunInput,
    StartJobRunInput, StorageDescriptor, Table, TableIdentifier, TableInput,
    Statement, StatementOutput,
    TableVersion, TargetRedshiftCatalog,
    TransformEncryption, TransformFilterCriteria, TransformParameters, TransformSortCriteria,
    Blueprint, BlueprintDetails, BlueprintRun, CreateBlueprintInput, StartBlueprintRunInput,
    Trigger, TriggerNodeDetails,
    Tag, TargetProcessingProperties, TargetTableConfig,
    TriggerUpdate,
    UpdateClassifierInput, UpdateCrawlerInput, UpdateDataQualityRulesetInput, UpdateGlossaryInput,
    UpdateMLTransformInput,
    UpdateWorkflowInput,
    Workflow, WorkflowGraph,
    WorkflowRun,
    WorkflowRunStatistics, XMLClassifier,
};

/// Ceiling on the databases one catalog may hold.
///
/// This emulator's own capacity, NOT an AWS quota: the store is resident memory and a
/// create is reachable by any caller, so the number of records one process will hold
/// is bounded here. A create past the ceiling is refused with the operation's modelled
/// `ResourceNumberLimitExceededException` rather than inserted.
pub const MAX_DATABASES_PER_CATALOG: usize = 1_000;

/// Ceiling on the tables one database may hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`].
pub const MAX_TABLES_PER_DATABASE: usize = 1_000;

/// Ceiling on the tables the whole store may hold, across every catalog and database.
/// Bounds the product of the two per-container ceilings, which on its own would still
/// let a caller grow the store far past what one process should hold.
pub const MAX_STORED_TABLES: usize = 10_000;

/// Ceiling on the partitions ONE table may hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`]: a partition create is reachable by any caller, so the
/// number of partition records one process will hold is bounded here. A create past the
/// ceiling is refused — per item, for a batch create — rather than inserted.
pub const MAX_PARTITIONS_PER_TABLE: usize = 1_000;

/// Ceiling on the partitions the whole store may hold, across every catalog, database
/// and table. Bounds the PRODUCT of [`MAX_PARTITIONS_PER_TABLE`] and
/// [`MAX_STORED_TABLES`], which on its own would still let a caller grow the store far
/// past what one process should hold.
pub const MAX_STORED_PARTITIONS: usize = 100_000;

/// Ceiling on the COLUMNS of one table the store keeps statistics for, for the same
/// reason as [`MAX_DATABASES_PER_CATALOG`]: a statistics write is reachable by any caller
/// and addresses a column by name, so the number of statistics records one process will
/// hold is bounded here. An entry for a further column is refused — per item, the way this
/// write reports every entry it cannot store — rather than inserted.
pub const MAX_COLUMN_STATISTICS_PER_TABLE: usize = 1_000;

/// Ceiling on the versions of ONE table the store keeps archived, for the same reason
/// as [`MAX_DATABASES_PER_CATALOG`]: every update archives the definition it replaces,
/// and the update is reachable by any caller, so the number of archived definitions one
/// process will hold is bounded here. An update that would archive past the ceiling is
/// refused with the operation's modelled `ResourceNumberLimitExceededException` rather
/// than applied.
pub const MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE: usize = 1_000;

/// Ceiling on the archived table versions the whole store may hold, across every
/// catalog, database and table. Bounds the PRODUCT of
/// [`MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE`] and [`MAX_STORED_TABLES`], which on its own
/// would still let a caller grow the store far past what one process should hold.
pub const MAX_STORED_ARCHIVED_TABLE_VERSIONS: usize = 100_000;

/// Ceiling on the connection definitions one catalog may hold, for the same reason as
/// [`MAX_JOBS`].
pub const MAX_CONNECTIONS_PER_CATALOG: usize = 1_000;

/// Ceiling on the CATALOGS the whole store may hold.
///
/// The ceilings above bound what one container holds; this one bounds how many
/// containers there are, which is what makes the store as a whole bounded. The map is
/// keyed by `CatalogId`, a member a request may name, so without this a create that
/// reached a catalog the store does not hold yet could add containers without end — each
/// able to hold its own [`MAX_DATABASES_PER_CATALOG`] and
/// [`MAX_CONNECTIONS_PER_CATALOG`] records. It is also what keeps the store-wide walks
/// below ([`ComAmazonawsGlueAwsglueState::stored_tables`] and its siblings) a bounded
/// cost per create. A create that would add one past the ceiling is refused with the
/// operation's modelled `ResourceNumberLimitExceededException`.
pub const MAX_CATALOGS: usize = 16;

/// Ceiling on the databases the whole store may hold, across every catalog, for the same
/// reason as [`MAX_STORED_TABLES`]: the per-catalog ceiling alone, multiplied by
/// [`MAX_CATALOGS`], would still let a caller grow the store past what one process
/// should hold.
pub const MAX_STORED_DATABASES: usize = 10_000;

/// Ceiling on the connection definitions the whole store may hold, across every catalog,
/// for the same reason as [`MAX_STORED_DATABASES`].
pub const MAX_STORED_CONNECTIONS: usize = 10_000;

/// The `ConnectionProperties` key holding a connection's password: "`PASSWORD` - A
/// password, if one is used, for the user name." (`Connection$ConnectionProperties`.) It is
/// what `GetConnection`'s `HidePassword` withholds.
pub const PASSWORD_PROPERTY: &str = "PASSWORD";

/// The `ConnectionProperties` key holding the encrypted form of that password: "
/// `ENCRYPTED_PASSWORD` - When you enable connection password protection … this field
/// stores the encrypted password." Withheld with [`PASSWORD_PROPERTY`].
pub const ENCRYPTED_PASSWORD_PROPERTY: &str = "ENCRYPTED_PASSWORD";

/// `Connection$ConnectionSchemaVersion` for a connection whose properties are the version 1
/// set, and for one carrying the per-compute-environment properties: "The version of the
/// connection schema for this connection. Version 2 supports properties for specific
/// compute environments."
pub const CONNECTION_SCHEMA_VERSION_V1: i32 = 1;
/// See [`CONNECTION_SCHEMA_VERSION_V1`].
pub const CONNECTION_SCHEMA_VERSION_V2: i32 = 2;

/// The three values the model's `ComputeEnvironment` enum carries, which
/// `GetConnection$ApplyOverrideForComputeEnvironment` names one of.
pub const COMPUTE_ENVIRONMENT_SPARK: &str = "SPARK";
/// See [`COMPUTE_ENVIRONMENT_SPARK`].
pub const COMPUTE_ENVIRONMENT_ATHENA: &str = "ATHENA";
/// See [`COMPUTE_ENVIRONMENT_SPARK`].
pub const COMPUTE_ENVIRONMENT_PYTHON: &str = "PYTHON";

/// How many names ONE request may give a batch reader of this service to look up.
///
/// This emulator's own REQUEST-SIZE bound, with no modelled counterpart: a batch reader's
/// name list bounds each element's length but says nothing about how many elements it may
/// hold, while the answer carries one whole resource projection per name the request
/// lists, repeats included. So what one request can make such an operation look up and
/// allocate is bounded here, at the figure this service's paginated readers already serve
/// in one answer (`GetTables`' page ceiling, 100 records), so a batch cannot ask for a
/// larger answer than a page. ONE figure, shared by every batch reader that needs it, so
/// two operations never enforce two different ceilings.
///
/// A batch past it is refused with the asking operation's own modelled
/// `InvalidInputException`, in a message that reports this figure.
pub const MAX_BATCH_NAMES: usize = 100;

/// Ceiling on the crawlers this emulator will hold, for the same reason as
/// [`MAX_DATABASES_PER_CATALOG`]: resident memory bounded against a create any caller
/// can reach. A create past it is refused with `CreateCrawler`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_CRAWLERS: usize = 1_000;

/// Ceiling on the job definitions this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateJob`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_JOBS: usize = 1_000;

/// Ceiling on the triggers this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateTrigger`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_TRIGGERS: usize = 1_000;

/// Ceiling on the custom entity types this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateCustomEntityType`'s own
/// modelled `ResourceNumberLimitExceededException`.
pub const MAX_CUSTOM_ENTITY_TYPES: usize = 1_000;

/// Ceiling on the security configurations this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. A create past it is refused with `CreateSecurityConfiguration`'s own
/// modelled `ResourceNumberLimitExceededException`.
pub const MAX_SECURITY_CONFIGURATIONS: usize = 1_000;

/// Ceiling on the data quality rulesets this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateDataQualityRuleset` writes a record any caller can reach, and
/// each one carries up to the modelled 65536 characters of DQDL, so the number of records
/// one process will hold is bounded here. A create past it is refused with
/// `CreateDataQualityRuleset`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_DATA_QUALITY_RULESETS: usize = 1_000;

/// Ceiling on the classifiers this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]. `CreateClassifier` models no limit error, so a create past it is
/// refused with the closest client error that operation does model — its
/// `InvalidInputException` — exactly as `PutFormType`'s own ceiling is.
pub const MAX_CLASSIFIERS: usize = 1_000;

/// The `Version` a classifier carries straight after `CreateClassifier`: the first
/// version of a classifier is 1, and an update moves it on, exactly as
/// [`FIRST_CRAWLER_VERSION`] does for a crawler. The model's `@default` of 0 on
/// `GrokClassifier$Version` and its three peers is what an ABSENT long deserializes to,
/// not the version of a stored classifier.
pub const FIRST_CLASSIFIER_VERSION: i64 = 1;

/// Ceiling on the form types this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `PutFormType` writes a record any caller can reach, and each one
/// carries a schema of up to the modelled 100000 characters, so the number of records one
/// process will hold is bounded here. A put that would add one past the ceiling is
/// refused rather than stored; `PutFormType` models no limit error, so the refusal is
/// the closest client error it does model (`InvalidInputException`).
pub const MAX_FORM_TYPES: usize = 1_000;

/// Ceiling on the asset types this emulator will hold, for the same reason as
/// [`MAX_FORM_TYPES`]: `PutAssetType` writes a record any caller can reach, each one
/// carrying up to the modelled 100 form references, so the number of records one process
/// will hold is bounded here. A put that would add one past the ceiling is refused rather
/// than stored; `PutAssetType` models no limit error, so the refusal is the closest client
/// error it does model (`InvalidInputException`).
pub const MAX_ASSET_TYPES: usize = 1_000;

/// Ceiling on the assets this emulator will hold, for the same reason as
/// [`MAX_ASSET_TYPES`]: `PutAsset` writes a record any caller can reach, each one carrying
/// its forms and their JSON content, so the number of records one process will hold is
/// bounded here. A put that would add one past the ceiling is refused rather than stored;
/// `PutAsset` models no limit error, so the refusal is the closest client error it does
/// model (`InvalidInputException`).
pub const MAX_ASSETS: usize = 1_000;

/// How many glossary terms one asset can carry: the modelled bound on the member that
/// REPORTS them, "len 1..10" on `GlossaryTermIdList`
/// (`smithy_query operation=AssociateGlossaryTerms`), which is the shape both
/// `AssociateGlossaryTermsResponse$GlossaryTerms` and `GetAssetOutput$GlossaryTerms` target
/// — so an asset holding more than this many terms could not be reported by either reader,
/// whatever the input list of one request is allowed to carry. An association that would
/// leave an asset past it is therefore refused with the closest client error the operation
/// models (`InvalidInputException`) rather than stored.
pub const MAX_ASSET_GLOSSARY_TERMS: usize = 10;

/// Ceiling on the business glossaries this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateGlossary` writes a record any caller can reach, so the number
/// of records one process will hold is bounded here. A create past the ceiling is refused
/// rather than stored; `CreateGlossary` models no limit error, so the refusal is the
/// closest client error it does model (`InvalidInputException`).
pub const MAX_GLOSSARIES: usize = 1_000;

/// Ceiling on the workflows this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateWorkflow` writes a record any caller can reach, so the number
/// of records one process will hold is bounded here. A create past the ceiling is refused
/// with `CreateWorkflow`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_WORKFLOWS: usize = 1_000;

/// Ceiling on the blueprints this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateBlueprint` writes a record any caller can reach, so the number
/// of records one process will hold is bounded here. A create past the ceiling is refused
/// with `CreateBlueprint`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_BLUEPRINTS: usize = 1_000;

/// Ceiling on the interactive sessions this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateSession` writes a record any caller can reach, each one carrying
/// its default arguments and connections, so the number of records one process will hold is
/// bounded here. A create past the ceiling is refused with `CreateSession`'s own modelled
/// `ResourceNumberLimitExceededException`.
pub const MAX_SESSIONS: usize = 1_000;

/// Ceiling on the glossary terms this emulator will hold, for the same reason as
/// [`MAX_GLOSSARIES`]: `CreateGlossaryTerm` writes a record any caller can reach, each one
/// carrying descriptions of up to the modelled 1024 and 4096 characters, so the number of
/// records one process will hold is bounded here. A create past the ceiling is refused
/// rather than stored; `CreateGlossaryTerm` models no limit error, so the refusal is the
/// closest client error it does model (`InvalidInputException`).
pub const MAX_GLOSSARY_TERMS: usize = 1_000;

/// Ceiling on the usage profiles this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `CreateUsageProfile` writes a record any caller can reach, each one
/// carrying two configuration maps, so the number of records one process will hold is
/// bounded here. A create past the ceiling is refused with `CreateUsageProfile`'s own
/// modelled `ResourceNumberLimitExceededException`.
pub const MAX_USAGE_PROFILES: usize = 1_000;

/// Ceiling on the machine learning transforms this emulator will hold, for the same reason
/// as [`MAX_CRAWLERS`]: `CreateMLTransform` writes a record any caller can reach, each one
/// carrying its input tables, parameters and schema, so the number of records one process
/// will hold is bounded here. A create past the ceiling is refused with
/// `CreateMLTransform`'s own modelled `ResourceNumberLimitExceededException`.
pub const MAX_ML_TRANSFORMS: usize = 1_000;

/// Ceiling on the resource policies this emulator will hold, for the same reason as
/// [`MAX_CRAWLERS`]: `PutResourcePolicy` writes a record any caller can reach, and each
/// one carries a policy document, so the number of records one process will hold is
/// bounded here. A write that would add one past the ceiling is refused rather than
/// stored; `PutResourcePolicy` models no limit error, so the refusal is the closest
/// client error it does model (`InvalidInputException`).
pub const MAX_RESOURCE_POLICIES: usize = 1_000;

/// Ceiling on the integration resource property records this emulator will hold, for the
/// same reason as [`MAX_CRAWLERS`]: `CreateIntegrationResourceProperty` writes a record any
/// caller can reach, each one carrying the processing properties of one resource, so the
/// number of records one process will hold is bounded here. A create that would add one
/// past the ceiling is refused rather than stored; that operation models no limit error, so
/// the refusal is the closest client error it does model (`InvalidInputException`).
pub const MAX_INTEGRATION_RESOURCE_PROPERTIES: usize = 1_000;

/// Ceiling on the integration table property records this emulator will hold, for the same
/// reason as [`MAX_INTEGRATION_RESOURCE_PROPERTIES`]: `CreateIntegrationTableProperties`
/// writes a record any caller can reach, one per (resource, table) pair, so the number of
/// records one process will hold is bounded here. A create that would add one past the
/// ceiling is refused rather than stored; that operation models no limit error, so the
/// refusal is the closest client error it does model (`InvalidInputException`).
pub const MAX_INTEGRATION_TABLE_PROPERTIES: usize = 1_000;

/// The `ExistCondition` a `PutResourcePolicy` carries to require that a policy is already
/// attached ("A value of MUST_EXIST is used to update a policy"). One of the modelled
/// `ExistCondition` values, whose published list the generated
/// [`crate::ops::types::EXIST_CONDITION_VALUES`] carries (`MUST_EXIST`, `NONE`,
/// `NOT_EXIST`) and the request layer validates against.
pub const EXIST_CONDITION_MUST_EXIST: &str = "MUST_EXIST";

/// The `ExistCondition` that requires that NO policy is attached yet ("A value of
/// NOT_EXIST is used to create a new policy"). See [`EXIST_CONDITION_MUST_EXIST`] for the
/// published list; the remaining value (`NONE`) states no dependency on an existing
/// policy at all, which is also what an absent member means ("If a value of NONE or a
/// null value is used, the call does not depend on the existence of a policy").
pub const EXIST_CONDITION_NOT_EXIST: &str = "NOT_EXIST";

/// The `CrawlerState` of a crawler with no run in progress — the state a crawler is in
/// the moment `CreateCrawler` returns, and the only one this emulator stores until
/// `StartCrawler`/`StopCrawler` are implemented. One of the modelled `CrawlerState`
/// values (`READY`, `RUNNING`, `STOPPING`).
pub const CRAWLER_STATE_READY: &str = "READY";

/// The `CrawlerState` of a crawler whose crawl is in progress — "indicates whether the
/// crawler is running, or whether a run is pending" (`Crawler$State`). A write that
/// changes the crawler's definition is refused while it is in this state.
pub const CRAWLER_STATE_RUNNING: &str = "RUNNING";

/// The `CrawlerState` of a crawler whose crawl has been asked to stop and has not
/// finished: a run is still in progress, so a definition write is refused here too.
pub const CRAWLER_STATE_STOPPING: &str = "STOPPING";

/// The `Schedule$State` of a crawler whose cron expression is in force: the state every
/// write that STATES a schedule leaves it in, because a schedule a write puts on a
/// crawler is in effect the moment that write returns. One of the modelled `ScheduleState`
/// values (`SCHEDULED`, `NOT_SCHEDULED`, `TRANSITIONING`); a crawler created WITHOUT a
/// `Schedule` has no schedule object at all.
pub const SCHEDULE_STATE_SCHEDULED: &str = "SCHEDULED";

/// The `Schedule$State` of a crawler whose schedule is SUSPENDED: what
/// `StopCrawlerSchedule` leaves it in ("sets the schedule state of the specified crawler
/// to NOT_SCHEDULED"), and the state a crawler that carries no cron expression at all is
/// in. Another of the modelled `ScheduleState` values.
pub const SCHEDULE_STATE_NOT_SCHEDULED: &str = "NOT_SCHEDULED";

/// The `TriggerState` of a trigger that is registered but not started: the state a
/// trigger is in the moment `CreateTrigger` returns when the create did not ask for it to
/// be started. One of the modelled `TriggerState` values, whose published list the
/// generated [`crate::ops::types::TRIGGER_STATE_VALUES`] carries.
///
/// A create answers with the trigger in this STEADY state rather than in the enum's
/// `CREATING`: the create is synchronous — it registers a definition and starts nothing —
/// so the clock owes a freshly created trigger no further transition.
pub const TRIGGER_STATE_CREATED: &str = "CREATED";

/// The `TriggerState` of a trigger that is started and waiting to fire: what
/// `CreateTrigger` leaves a trigger in when the request set `StartOnCreation` ("Set to
/// true to start SCHEDULED and CONDITIONAL triggers when created").
///
/// A trigger that waits on nothing but a caller is never in this state: it has no armed
/// state to be put into, so a start of one is a firing and leaves it in
/// [`TRIGGER_STATE_CREATED`]. See [`TriggerEntry::start`].
pub const TRIGGER_STATE_ACTIVATED: &str = "ACTIVATED";

/// The `TriggerState` of a trigger that has been stopped: the one modelled state of a
/// trigger that was asked to stop ("Stops a specified trigger" — `StopTrigger`), which
/// every later read of that trigger reports until a start moves it back to
/// [`TRIGGER_STATE_ACTIVATED`]. Another of the modelled `TriggerState` values.
///
/// A stop leaves the trigger in this STEADY state rather than in the enum's
/// `DEACTIVATING`, exactly as a start leaves it in [`TRIGGER_STATE_ACTIVATED`] rather than
/// in `ACTIVATING`: nothing in this emulator keeps a trigger's schedule or predicate
/// running, so the clock owes a stopped trigger no further transition.
pub const TRIGGER_STATE_DEACTIVATED: &str = "DEACTIVATED";

/// The `TriggerType` of a trigger that fires only when a caller starts it: the one type
/// `StartOnCreation` cannot be set for ("True is not supported for ON_DEMAND triggers" —
/// `CreateTriggerRequest$StartOnCreation`). One of the modelled `TriggerType` values,
/// whose published list the generated [`crate::ops::types::TRIGGER_TYPE_VALUES`] carries
/// (`CONDITIONAL`, `EVENT`, `ON_DEMAND`, `SCHEDULED`).
pub const TRIGGER_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// The `TriggerType` of a trigger that fires on a cron schedule, for which the request's
/// `Schedule` is required ("This field is required when the trigger type is SCHEDULED" —
/// `CreateTriggerRequest$Schedule`).
pub const TRIGGER_TYPE_SCHEDULED: &str = "SCHEDULED";

/// The `TriggerType` of a trigger that fires when its predicate is satisfied, for which
/// the request's `Predicate` is required ("This field is required when the trigger type is
/// CONDITIONAL" — `CreateTriggerRequest$Predicate`).
pub const TRIGGER_TYPE_CONDITIONAL: &str = "CONDITIONAL";

/// The `TriggerType` of a trigger that fires on the events EventBridge hands the workflow
/// it belongs to, as its `EventBatchingCondition` decides: "Number of events that must be
/// received from Amazon EventBridge before EventBridge event trigger fires"
/// (`EventBatchingCondition$BatchSize`). The one type whose firing comes from OUTSIDE this
/// service.
pub const TRIGGER_TYPE_EVENT: &str = "EVENT";

/// The batch size an EVENT trigger counts deliveries against when its
/// `EventBatchingCondition` names none at all.
///
/// `BatchSize` is `@required` INSIDE the condition but the condition itself is optional, so
/// a trigger carrying no condition batches nothing: it fires on the first event it is
/// handed, which is the lower bound of the member's `@range 1..100`.
pub const DEFAULT_EVENT_BATCH_SIZE: i32 = 1;

/// The window an EVENT trigger's open batch is given when its `EventBatchingCondition`
/// names no `BatchWindow`.
///
/// "Window of time in seconds after which EventBridge event trigger fires. Window starts
/// when first event is received" (`EventBatchingCondition$BatchWindow`, `@range 1..900`):
/// Glue documents 900 seconds — the member's upper bound — as the window an event trigger
/// batches over by default, so a condition that names only a `BatchSize` waits that long
/// for the batch to fill before firing with what it has.
pub const DEFAULT_EVENT_BATCH_WINDOW_SECONDS: u64 = 900;

/// The `NodeType` of a workflow-graph node that stands for a trigger. One of the modelled
/// `NodeType` values, whose published list the generated
/// [`crate::ops::types::NODE_TYPE_VALUES`] carries (`CRAWLER`, `JOB`, `TRIGGER`).
pub const NODE_TYPE_TRIGGER: &str = "TRIGGER";

/// The `NodeType` of a node that stands for a job definition. See [`NODE_TYPE_TRIGGER`].
pub const NODE_TYPE_JOB: &str = "JOB";

/// The `NodeType` of a node that stands for a crawler. See [`NODE_TYPE_TRIGGER`].
pub const NODE_TYPE_CRAWLER: &str = "CRAWLER";

/// The `Version` a crawler carries straight after `CreateCrawler`: the first version of
/// a crawler is 1, and an update moves it on. The model's `@default` of 0 on
/// `Crawler$Version` is what an ABSENT long deserializes to, not the version of a stored
/// crawler. (A table's first version is numbered differently — see
/// [`FIRST_TABLE_VERSION_ID`] — because the catalog's table version history starts at
/// 0.)
pub const FIRST_CRAWLER_VERSION: i64 = 1;

/// The Lake Formation principal that stands for "whoever IAM already allows": the
/// `DataLakePrincipalIdentifier` of the grant a Data Catalog database carries while
/// nothing has narrowed it to Lake Formation permissions. Spelled exactly like this on
/// the wire, and the same principal
/// `services/service-com-amazonaws-lakeformation-awslakeformation/src/state.rs` holds the
/// matching database grant under.
pub const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The permission that default grant carries.
///
/// From the documented Lake Formation default, not chosen: Lake Formation's
/// `DataLakeSettings$CreateDatabaseDefaultPermissions` documents that "A value that
/// assigns ALL to IAM_ALLOWED_PRINCIPALS indicates access control by IAM permissions.
/// This is referred to as the setting \"Use only IAM access control\", and is for
/// backward compatibility with the Glue permission model", and that "The only permitted
/// values are an empty array or an array that contains a single JSON object that grants
/// ALL to IAM_ALLOWED_PRINCIPALS". `ALL` is also one of the modelled `Permission` enum's
/// values (`ALL`, `SELECT`, `ALTER`, `DROP`, `DELETE`, `INSERT`, `CREATE_DATABASE`,
/// `CREATE_TABLE`, `DATA_LOCATION_ACCESS`).
pub const PERMISSION_ALL: &str = "ALL";

/// The `CreateTableDefaultPermissions` a database gets when its create named none.
///
/// `Database$CreateTableDefaultPermissions` "creates a set of default permissions on the
/// table for principals. Used by Lake Formation": a database the caller did not govern
/// with Lake Formation is open to every IAM-allowed principal, which the catalog records
/// as the single `ALL` grant to [`IAM_ALLOWED_PRINCIPALS`] rather than as no grant at
/// all. The distinction matters to the reader: an EMPTY grant list is the opposite
/// governance decision ("nobody, until Lake Formation grants it"), so it is stored only
/// when the client sends one.
pub fn default_create_table_default_permissions() -> Vec<PrincipalPermissions> {
    vec![PrincipalPermissions {
        principal: Some(DataLakePrincipal {
            data_lake_principal_identifier: Some(IAM_ALLOWED_PRINCIPALS.to_string()),
        }),
        permissions: Some(vec![PERMISSION_ALL.to_string()]),
        // Stamped by the read projection, not stored: see
        // [`tagged_principal_permissions`].
        __type: None,
    }]
}

/// The `__type` tag the Data Catalog writes onto a nested `PrincipalPermissions`.
///
/// The Data Catalog's awsJson1.1 serializer tags the nested structures it returns with a
/// type discriminator whose value is the structure's own Smithy shape name, unqualified,
/// so a client that dispatches on the tag can identify the entry. The two structures the
/// recorded wire traffic shows tagged are `PrincipalPermissions` (inside
/// `Database$CreateTableDefaultPermissions`) and [`ERROR_DETAIL_TYPE_TAG`]'s
/// `ErrorDetail`; the tag is therefore keyed on the SHAPE, which is why each constant is
/// spelled exactly like the shape it tags.
pub const PRINCIPAL_PERMISSIONS_TYPE_TAG: &str = "PrincipalPermissions";

/// The `__type` tag the Data Catalog writes onto a nested `ErrorDetail`. See
/// [`PRINCIPAL_PERMISSIONS_TYPE_TAG`] for the rule.
pub const ERROR_DETAIL_TYPE_TAG: &str = "ErrorDetail";

/// A stored grant as the catalog REPORTS it: the stored members plus the `__type` tag.
///
/// The tag belongs to the projection, not to the record: a client may send
/// `CreateTableDefaultPermissions` with or without a tag of its own, and the catalog
/// answers with its own either way — so the tag is stamped here on the way out rather
/// than carried over from whatever the request happened to contain.
pub fn tagged_principal_permissions(stored: &PrincipalPermissions) -> PrincipalPermissions {
    PrincipalPermissions {
        __type: Some(PRINCIPAL_PERMISSIONS_TYPE_TAG.to_string()),
        ..stored.clone()
    }
}

/// A per-item `ErrorDetail` the way the Data Catalog reports one: the error's code and
/// message, plus the `__type` tag ([`ERROR_DETAIL_TYPE_TAG`]).
///
/// Every batch operation that reports a per-item failure builds its `ErrorDetail` here,
/// so the tag cannot be present on one operation's entries and absent from another's.
pub fn catalog_error_detail(error_code: &str, error_message: String) -> ErrorDetail {
    ErrorDetail {
        error_code: Some(error_code.to_string()),
        error_message: Some(error_message),
        __type: Some(ERROR_DETAIL_TYPE_TAG.to_string()),
    }
}

/// The `CatalogEncryptionMode` a catalog is in while nothing encrypts its metadata at
/// rest.
///
/// Source: the modelled `CatalogEncryptionMode` enum, whose published values the
/// generated [`crate::ops::types::CATALOG_ENCRYPTION_MODE_VALUES`] carries (`DISABLED`,
/// `SSE-KMS`, `SSE-KMS-WITH-SERVICE-ROLE`) and the request layer validates against.
/// `DISABLED` is the only one of them that names no key, so it is the mode a catalog no
/// `PutDataCatalogEncryptionSettings` has configured reports.
pub const CATALOG_ENCRYPTION_MODE_DISABLED: &str = "DISABLED";

/// The `EncryptionAtRest` a catalog reports while no setting has been stored for it.
///
/// `EncryptionAtRest$CatalogEncryptionMode` is required, so the structure is only
/// well formed with a mode in it, and the mode of a catalog nothing configured is
/// [`CATALOG_ENCRYPTION_MODE_DISABLED`]; with no mode selected there is no key and no
/// role to report either.
pub fn default_encryption_at_rest() -> EncryptionAtRest {
    EncryptionAtRest {
        catalog_encryption_mode: CATALOG_ENCRYPTION_MODE_DISABLED.to_string(),
        sse_aws_kms_key_id: None,
        catalog_encryption_service_role: None,
    }
}

/// The `ConnectionPasswordEncryption` a catalog reports while no setting has been stored
/// for it.
///
/// `ConnectionPasswordEncryption$ReturnConnectionPasswordEncrypted` is required and
/// modelled with a default of `false`, so a catalog nothing configured hands connection
/// passwords back the way it stored them, under no key of its own.
pub fn default_connection_password_encryption() -> ConnectionPasswordEncryption {
    ConnectionPasswordEncryption {
        return_connection_password_encrypted: false,
        aws_kms_key_id: None,
    }
}

/// How many database ids this process has minted.
///
/// Not catalog state — the id itself is what gets stored, on the [`DatabaseEntry`]. This
/// is only a nonce source for [`new_database_id`], so that two creates landing in the
/// same nanosecond are still given different input to mint from.
static MINTED_DATABASE_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `DatabaseId`: the opaque id the catalog assigns a database when it is
/// created.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is the shape a
/// catalog-assigned Data Catalog id has on the wire. The bits come from [`mint_hex_id`],
/// drawn against [`MINTED_DATABASE_IDS`] — so two creates in the same nanosecond cannot be
/// minted from the same input. It is minted once, at create, and stored on the
/// [`DatabaseEntry`], so
/// every later read of that database — and every table in it — reports the same id, and
/// two databases never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_database_id() -> String {
    mint_hex_id(&MINTED_DATABASE_IDS, CATALOG_ID_WORDS)
}

/// How many 64-bit words a catalog-assigned Data Catalog id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters. The width of a database id and of a
/// form type id alike — both are ids the catalog assigns a resource.
const CATALOG_ID_WORDS: usize = 2;

/// `words` 64-bit draws rendered as lowercase hexadecimal: how this crate spells an opaque
/// id the catalog assigns, so every id-minting function below agrees on how it reads.
///
/// The bits of each draw come from [`id_draw`], which is the ONE place the entropy rule
/// lives.
///
/// This mints a RESOURCE id, not a credential: a credential would come from the credential
/// store, never from here.
fn mint_hex_id(minted: &AtomicU64, words: usize) -> String {
    use std::fmt::Write;

    let sequence = minted.fetch_add(1, Ordering::Relaxed);
    let mut id = String::with_capacity(words * 16);
    for word in 0..words {
        // Writing into the pre-sized buffer; `write!` to a `String` cannot fail.
        let _ = write!(id, "{:016x}", id_draw(sequence, word));
    }
    id
}

/// One 64-bit draw for the `index`-th word of the id `sequence` is minting: the ONE place
/// this crate draws the bits of an id from, so two id shapes cannot disagree about where
/// they come from.
///
/// The bits come from three sources: the OS-seeded randomness a `RandomState` draws, the
/// system clock in nanoseconds, and `sequence`, the minting caller's own count of how many
/// ids it has handed out — so two ids minted in the same nanosecond cannot come from the
/// same input. Each caller passes its OWN counter, so one kind of id running ahead cannot
/// skew another's.
///
/// This draws for a RESOURCE id, not for a credential: a credential would come from the
/// credential store, never from here.
pub(super) fn id_draw(sequence: u64, index: usize) -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    RandomState::new().hash_one((sequence, nanos, index))
}

/// Per-service state for com-amazonaws-glue-awsglue. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGlueAwsglueState {
    /// Every Data Catalog this emulator holds, keyed by `CatalogId`.
    pub catalogs: HashMap<String, Catalog>,
    /// The instant the Data Catalogs of this store carry their DEFAULT migration status
    /// from ([`CatalogStatusStamp`]): the instant a status read reports as
    /// `CatalogImportStatus$ImportTime` for a catalog no `ImportCatalogToGlue` has
    /// migrated, which is every catalog until one does.
    ///
    /// On the store rather than on a catalog because the status is reported for a
    /// `CatalogId` whether or not this store holds a record under it: the account's own
    /// Data Catalog answers a status read before any create has materialised it.
    pub catalog_status_stamp: CatalogStatusStamp,
    /// Every crawler this emulator holds, keyed by its name.
    ///
    /// Crawlers live BESIDE the catalog tree rather than inside it: `GetCrawler`,
    /// `DeleteCrawler` and `StartCrawler` address a crawler by name alone, with no
    /// `CatalogId` member, so the namespace is per account and a crawler's `CatalogId`
    /// says only where its output goes. The key is the name exactly as sent — unlike
    /// `DatabaseInput$Name`/`TableInput$Name`, the model documents no case fold for
    /// `Crawler$Name`.
    pub crawlers: HashMap<String, CrawlerEntry>,
    /// Every classifier this emulator holds, keyed by its name, in name order.
    ///
    /// Per account and beside the catalog tree, like the crawlers that refer to them:
    /// `GetClassifier`, `UpdateClassifier` and `DeleteClassifier` address a classifier by
    /// `Name` alone, with no `CatalogId` member, so the name a create sends is the whole
    /// address. Ordered, so `GetClassifiers` has one deterministic order to page over, and
    /// bounded by [`MAX_CLASSIFIERS`]. The key is the name exactly as sent — the model
    /// documents no case fold for a classifier name.
    ///
    /// NOT keyed by Region, which is the one choice this namespace makes differently from
    /// [`Self::jobs`] and [`Self::triggers`]: it follows the CRAWLER convention
    /// ([`Self::crawlers`]) because a classifier is reached only through a crawler's
    /// `Classifiers` list, which names classifiers by name alone, so a classifier has to
    /// live in the same namespace as the crawlers that may refer to it or a crawler could
    /// name a classifier its own namespace cannot resolve. A job is keyed by Region
    /// instead because its own uniqueness is documented per account and its runs are
    /// served only by the endpoint that created it; nothing of that kind holds for a
    /// classifier.
    pub classifiers: BTreeMap<String, ClassifierEntry>,
    /// Every job definition this emulator holds, keyed by the REGION it was created in
    /// and then by its name.
    ///
    /// Jobs live BESIDE the catalog tree, like crawlers: `GetJob`, `UpdateJob`,
    /// `DeleteJob` and `StartJobRun` address a job by name alone, with no `CatalogId`
    /// member, and `CreateJob$Name` is documented as unique per account ("It must be
    /// unique in your account"). Glue is a REGIONAL service, so that namespace is per
    /// account AND per Region: a job created through one Region's endpoint is served only
    /// through that endpoint, and the same name is free in every other Region. The inner
    /// key is the name exactly as sent — the model documents no case fold for `Job$Name`.
    pub jobs: HashMap<String, HashMap<String, JobEntry>>,
    /// Every trigger this emulator holds, keyed by the REGION it was created in and then
    /// by its name, in name order.
    ///
    /// Triggers live BESIDE the catalog tree, like jobs: `GetTrigger`, `UpdateTrigger`,
    /// `StartTrigger`, `StopTrigger` and `DeleteTrigger` address a trigger by name alone,
    /// with no `CatalogId` member. The outer key is the Region for the reason a job's is:
    /// Glue is a REGIONAL service, and a trigger's actions name the jobs and crawlers of
    /// the endpoint that created it, so a trigger created through one Region's endpoint is
    /// served only through that endpoint and the same name is free in every other Region.
    /// The inner map is ordered, so `ListTriggers`/`GetTriggers` have one deterministic
    /// order to page over, and its key is the name exactly as sent — the model documents
    /// no case fold for `Trigger$Name`.
    pub triggers: HashMap<String, BTreeMap<String, TriggerEntry>>,
    /// Every schema registry this emulator holds, keyed by `RegistryName`, in name order.
    ///
    /// Registries live BESIDE the catalog tree: a schema-registry operation addresses its
    /// resource through a `RegistryId`/`SchemaId` wrapper carrying a name or an ARN and no
    /// `CatalogId`, so the namespace is per account. The key is the name exactly as sent —
    /// the model documents no case fold for `SchemaRegistryNameString`, whose pattern
    /// (`^[a-zA-Z0-9-_$#.]+$`) admits both cases and so distinguishes them.
    ///
    /// The map is NOT pre-seeded with [`DEFAULT_REGISTRY_NAME`]: that entry is materialised
    /// by the first create that reaches it, which is what `CreateSchema` documents ("this
    /// will create an entry for a "default-registry" in the registry database tables, if it
    /// is not already present").
    pub registries: BTreeMap<String, RegistryEntry>,
    /// Every custom entity type (custom sensitive-data pattern) this emulator holds,
    /// keyed by its name.
    ///
    /// Per account, like crawlers and job definitions: `GetCustomEntityType`,
    /// `DeleteCustomEntityType` and `BatchGetCustomEntityTypes` address a pattern by name
    /// alone, and `CustomEntityType$Name` is documented as unique per account. Ordered,
    /// so `ListCustomEntityTypes` has one deterministic order to page over. The key is
    /// the name exactly as sent — the model documents no case fold for it.
    pub custom_entity_types: BTreeMap<String, CustomEntityTypeEntry>,
    /// Every security configuration this emulator holds, keyed by its name.
    ///
    /// Per account, like crawlers and custom entity types: `GetSecurityConfiguration`,
    /// `DeleteSecurityConfiguration` and `GetSecurityConfigurations` address a security
    /// configuration by name alone, with no `CatalogId` member, so the namespace is per
    /// account and the name a create sends is the whole address. Ordered, so
    /// `GetSecurityConfigurations` has one deterministic order to page over. The key is
    /// the name exactly as sent — the model documents no case fold for `NameString`.
    pub security_configurations: BTreeMap<String, SecurityConfigurationEntry>,
    /// Every metadata form type this emulator holds, keyed by `FormTypeName`.
    ///
    /// Per account, like crawlers and job definitions: `PutFormType` names a form type by
    /// `Name` alone, with no `CatalogId` member, and that name is what decides whether a
    /// put creates a form type or updates the one already stored. Ordered, so
    /// `ListFormTypes` has one deterministic order to page over. The key is the name
    /// exactly as sent — the model documents no case fold for `FormTypeName`, whose
    /// pattern (`^[A-Z]\w*$`) already fixes the first character's case.
    ///
    /// A form type is ADDRESSED by an `Identifier` targeting `FormTypeId`, the same shape
    /// `PutFormType`'s `Id` carries, and that identifier is the form type's NAME
    /// ([`FormTypeEntry::id`]) — so this key is the address.
    /// [`Self::form_type_by_reference`] is that resolution.
    pub form_types: BTreeMap<String, FormTypeEntry>,
    /// Every asset type this emulator holds, keyed by `AssetTypeName`.
    ///
    /// Per account and beside the catalog tree, like form types: `PutAssetType` names an
    /// asset type by `Name` alone, with no `CatalogId` member, and that name is what
    /// decides whether a put creates an asset type or updates the one already stored.
    /// Ordered, so `ListAssetTypes` has one deterministic order to page over. The key is
    /// the name exactly as sent — the model documents no case fold for `AssetTypeName`.
    ///
    /// An asset type is ADDRESSED by an `Identifier` rather than by this key:
    /// `GetAssetType` and `DeleteAssetType` take one targeting `AssetTypeId`, the same
    /// shape `PutAssetType`'s `Id` carries, and a client holds either that minted id or
    /// the name it registered the asset type under. [`Self::asset_type_by_reference`] is
    /// that resolution.
    pub asset_types: BTreeMap<String, AssetTypeEntry>,
    /// Every asset this emulator holds, keyed by its `AssetId`.
    ///
    /// Per account and beside the asset types they are instances of: `PutAsset` names an
    /// asset by `Identifier` alone, with no `CatalogId` member, and every operation that
    /// reads, updates, deletes or annotates an asset carries that same identifier. Keyed by
    /// it because the identifier is the whole address and is the CLIENT's, not minted here:
    /// "The unique identifier of the asset. If an asset with this identifier already
    /// exists, it is updated" (`PutAssetRequest$Identifier`). Ordered, so a listing of
    /// assets has one deterministic order to page over, and bounded by [`MAX_ASSETS`]. The
    /// key is the identifier exactly as sent — the model documents no case fold for
    /// `AssetId`.
    pub assets: BTreeMap<String, AssetEntry>,
    /// Every business glossary this emulator holds, keyed by its `GlossaryId`.
    ///
    /// Per account, like crawlers and job definitions: `GetGlossary`, `UpdateGlossary`,
    /// `DeleteGlossary` and `CreateGlossaryTerm` address a glossary by an identifier
    /// targeting `GlossaryId` alone, with no `CatalogId` member. Keyed by that id rather
    /// than by name because the id is the whole address and a glossary's name can be
    /// changed by an update, while the map stays ordered so `ListGlossaries` has one
    /// deterministic order to page over.
    ///
    /// The NAME is a uniqueness constraint over this namespace rather than its key:
    /// [`Self::glossary_by_name`] is that resolution, and `CreateGlossary`'s modelled
    /// `AlreadyExistsException` is what a second create under a stored name gets.
    pub glossaries: BTreeMap<String, GlossaryEntry>,
    /// Every glossary term this emulator holds, keyed by its `GlossaryTermId`.
    ///
    /// BESIDE the glossaries rather than inside them, exactly as a column statistics task
    /// run sits beside the table it covers: `GetGlossaryTerm`, `UpdateGlossaryTerm` and
    /// `DeleteGlossaryTerm` address a term by a `GlossaryTermId` alone, with no glossary
    /// member at all, so the id is the whole address and the glossary a term belongs to is
    /// recorded ON the term ([`GlossaryTermEntry::glossary_id`]). That is also why deleting
    /// a glossary leaves its terms addressable: the term records are not held by the
    /// glossary record. Ordered, so `ListGlossaryTerms` has one deterministic order to page
    /// over, and bounded by [`MAX_GLOSSARY_TERMS`].
    ///
    /// A term's NAME is a uniqueness constraint within ONE glossary rather than its key:
    /// [`Self::glossary_term_by_name`] is that resolution, and `CreateGlossaryTerm`'s
    /// modelled `AlreadyExistsException` is what a second create of that name in that
    /// glossary gets.
    pub glossary_terms: BTreeMap<String, GlossaryTermEntry>,
    /// Every usage profile this emulator holds, keyed by its name, in name order.
    ///
    /// Per account, like crawlers and custom entity types: `GetUsageProfile`,
    /// `UpdateUsageProfile` and `DeleteUsageProfile` address a profile by `Name` alone,
    /// with no `CatalogId` member. Ordered, so `ListUsageProfiles` has one deterministic
    /// order to page over. The key is the name exactly as sent — the model documents no
    /// case fold for a usage profile name.
    pub usage_profiles: BTreeMap<String, UsageProfileEntry>,
    /// Every machine learning transform this emulator holds, keyed by its `TransformId`.
    ///
    /// Per account and beside the catalog tree, like crawlers and usage profiles:
    /// `GetMLTransform`, `UpdateMLTransform` and `DeleteMLTransform` address a transform by
    /// an id alone, with no `CatalogId` member. Keyed by that minted id rather than by name
    /// because the id is the whole address and a transform's name is not even unique —
    /// "Names are not guaranteed unique and can be changed at any time"
    /// (`MLTransform$Name`) — so two transforms may share one. Ordered, so the two
    /// transform listings have one deterministic order to fall back on, and bounded by
    /// [`MAX_ML_TRANSFORMS`].
    pub ml_transforms: BTreeMap<String, MlTransformEntry>,
    /// Every column statistics task run this emulator holds, oldest first.
    ///
    /// A run lives BESIDE the catalog tree rather than on the table it was started for,
    /// because `GetColumnStatisticsTaskRun` addresses one by `ColumnStatisticsTaskRunId`
    /// alone, with no catalog, database or table member: the id is the whole address, so
    /// the namespace is per account and the table a run covers is recorded ON the run
    /// ([`ColumnStatisticsTaskRunEntry`]). Kept in start order, so
    /// `ListColumnStatisticsTaskRuns` has one deterministic order to page over, and
    /// bounded by [`MAX_COLUMN_STATISTICS_TASK_RUNS`].
    pub column_statistics_task_runs: Vec<ColumnStatisticsTaskRunEntry>,
    /// Every data quality rule recommendation run this emulator holds, oldest first.
    ///
    /// A run lives BESIDE the catalog tree rather than on the table it covers, for the
    /// same reason a column statistics task run does:
    /// `GetDataQualityRuleRecommendationRun` addresses one by `RunId` alone, with no
    /// catalog, database or table member, so the namespace is per account and the data
    /// source a run covers is recorded ON the run
    /// ([`DataQualityRuleRecommendationRunEntry`]). Kept in start order and bounded by
    /// [`MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS`].
    pub data_quality_rule_recommendation_runs: Vec<DataQualityRuleRecommendationRunEntry>,
    /// Every data quality ruleset evaluation run this emulator holds, oldest first.
    ///
    /// A run lives BESIDE the catalog tree rather than on the table it evaluates, for the
    /// same reason a rule recommendation run does:
    /// `GetDataQualityRulesetEvaluationRun` addresses one by `RunId` alone, with no
    /// catalog, database or table member, so the namespace is per account and the data
    /// source and the rulesets a run evaluates are recorded ON the run
    /// ([`DataQualityRulesetEvaluationRunEntry`]). Kept in start order and bounded by
    /// [`MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS`].
    pub data_quality_ruleset_evaluation_runs: Vec<DataQualityRulesetEvaluationRunEntry>,
    /// Every data quality result this emulator holds, oldest first.
    ///
    /// A result lives beside the runs that produce them, for the reason a run lives beside
    /// the catalog tree: `GetDataQualityResult` addresses one by `ResultId` alone, so the
    /// namespace is per account and the run, the ruleset and the data source a result came
    /// out of are recorded ON the result ([`DataQualityResultEntry`]). Kept in the order the
    /// runs produced them and bounded by [`MAX_DATA_QUALITY_RESULTS`].
    pub data_quality_results: Vec<DataQualityResultEntry>,
    /// Every data quality ruleset this emulator holds, keyed by its name, in name order.
    ///
    /// Per account and beside the catalog tree, like crawlers and custom entity types:
    /// `GetDataQualityRuleset`, `UpdateDataQualityRuleset` and `DeleteDataQualityRuleset`
    /// address a ruleset by `Name` alone, with no `CatalogId` member, and
    /// `CreateDataQualityRulesetRequest$Name` is "a unique name for the data quality
    /// ruleset". The target table a ruleset is written against is recorded ON the ruleset
    /// ([`DataQualityRulesetEntry::target_table`]) rather than the ruleset on the table,
    /// because that name is the whole address and the `TargetTable` is optional. Ordered,
    /// so `ListDataQualityRulesets` has one deterministic order to page over, and bounded
    /// by [`MAX_DATA_QUALITY_RULESETS`]. The key is the name exactly as sent — the model
    /// documents no case fold for `NameString`.
    pub data_quality_rulesets: BTreeMap<String, DataQualityRulesetEntry>,
    /// Every resource policy this emulator holds, keyed by the ARN of the resource it is
    /// attached to, in ARN order.
    ///
    /// A policy lives BESIDE the catalog tree because every operation that addresses one
    /// (`GetResourcePolicy`, `PutResourcePolicy`, `DeleteResourcePolicy`) addresses it by
    /// `ResourceArn` alone, with no `CatalogId` member — so the namespace is per account
    /// and the resource a policy governs is recorded in the key. A request that names no
    /// `ResourceArn` addresses the Data Catalog's own policy, which is keyed by the Data
    /// Catalog's ARN ([`catalog_arn`]), so the default and the catalog ARN spelled out
    /// name ONE record. Ordered, so `GetResourcePolicies` has one deterministic order to
    /// page over, and bounded by [`MAX_RESOURCE_POLICIES`].
    pub resource_policies: BTreeMap<String, ResourcePolicyEntry>,
    /// Every integration resource property record this emulator holds, keyed by the ARN of
    /// the resource whose properties it carries, in ARN order.
    ///
    /// A record lives BESIDE the catalog tree, like a resource policy, because every
    /// operation that addresses one (`CreateIntegrationResourceProperty`,
    /// `GetIntegrationResourceProperty`, `UpdateIntegrationResourceProperty`,
    /// `DeleteIntegrationResourceProperty`) addresses it by `ResourceArn` alone, with no
    /// `CatalogId` member — so the namespace is per account and the resource a record
    /// belongs to is the key. The key is the ARN exactly as sent: the model documents no
    /// fold for it. Ordered, so `ListIntegrationResourceProperties` has one deterministic
    /// order to page over, and bounded by [`MAX_INTEGRATION_RESOURCE_PROPERTIES`].
    pub integration_resource_properties: BTreeMap<String, IntegrationResourcePropertyEntry>,
    /// Every integration table property record this emulator holds, keyed by the
    /// `ResourceArn` whose leg of the integration the properties belong to and then by the
    /// `TableName` they override, in that order.
    ///
    /// A record lives BESIDE the catalog tree, like an integration resource property
    /// record, because every operation that addresses one
    /// (`CreateIntegrationTableProperties`, `GetIntegrationTableProperties`,
    /// `UpdateIntegrationTableProperties`, `DeleteIntegrationTableProperties`) addresses it
    /// by `ResourceArn` and `TableName` alone, with no `CatalogId` member — so the
    /// namespace is per account and the pair a record belongs to is the key. Both halves of
    /// the key are as sent: the model documents no fold for either member, and the table
    /// named is "the table to be replicated"
    /// (`CreateIntegrationTablePropertiesRequest$TableName`), a source-side entity rather
    /// than a catalog table whose name the catalog folds. Ordered, so
    /// `ListIntegrationTableProperties` has one deterministic order to page over, and
    /// bounded by [`MAX_INTEGRATION_TABLE_PROPERTIES`].
    pub integration_table_properties: BTreeMap<(String, String), IntegrationTablePropertiesEntry>,
    /// Every workflow this emulator holds, keyed by its name.
    ///
    /// Per account and beside the catalog tree, like crawlers and custom entity types:
    /// `GetWorkflow`, `UpdateWorkflow`, `DeleteWorkflow` and `StartWorkflowRun` address a
    /// workflow by `Name` alone, with no `CatalogId` member, and `CreateWorkflowRequest$Name`
    /// is documented as unique per account ("It should be unique within your account").
    /// Ordered, so `ListWorkflows` has one deterministic order to page over, and bounded by
    /// [`MAX_WORKFLOWS`]. The key is the name exactly as sent — the model documents no case
    /// fold for a workflow name.
    pub workflows: BTreeMap<String, WorkflowEntry>,
    /// Every blueprint this emulator holds, keyed by its name.
    ///
    /// Per account and beside the catalog tree, like crawlers and workflows:
    /// `GetBlueprint`, `UpdateBlueprint` and `DeleteBlueprint` address a blueprint by
    /// `Name` alone, with no `CatalogId` member, and `ListBlueprints` "lists all the
    /// blueprint names in an account". Ordered, so that listing has one deterministic
    /// order to page over. The key is the name exactly as sent — the model documents no
    /// case fold for a blueprint name, whose pattern (`^[\.\-_A-Za-z0-9]+$`) admits both
    /// cases and so distinguishes them.
    pub blueprints: BTreeMap<String, BlueprintEntry>,
    /// Every interactive session this emulator holds, keyed by the REGION it was created in
    /// and then by the `Id` its create chose, in id order.
    ///
    /// Sessions live BESIDE the catalog tree, like jobs and triggers: `GetSession`,
    /// `StopSession`, `DeleteSession` and every statement operation address a session by
    /// `Id` alone, with no `CatalogId` member. The outer key is the Region for the reason a
    /// job's is: Glue is a REGIONAL service and a session is a runtime provisioned in the
    /// Region its create was served in, so a session created through one Region's endpoint is
    /// served only through that endpoint and the same id is free in every other Region. The
    /// inner map is ordered, so `ListSessions` has one deterministic order to page over, and
    /// bounded by [`MAX_SESSIONS`]; its key is the id exactly as sent — the model documents
    /// no case fold for `NameString`.
    pub sessions: HashMap<String, BTreeMap<String, SessionEntry>>,
    /// The Glue IDENTITY CENTER CONFIGURATION of this account, or `None` while the account
    /// has none.
    ///
    /// An ACCOUNT SINGLETON: `GetGlueIdentityCenterConfiguration`,
    /// `DeleteGlueIdentityCenterConfiguration` and
    /// `UpdateGlueIdentityCenterConfiguration` take no input at all, so the account is the
    /// whole address of the configuration and there is at most one of it. A create while
    /// one is stored is therefore a duplicate rather than an overwrite, and a read or a
    /// delete while none is stored is a miss.
    pub identity_center_configuration: Option<IdentityCenterConfigurationEntry>,
    /// The job-run state changes this store has observed and not published yet, oldest
    /// first ([`job_run_events`]).
    ///
    /// A run that has COME TO REST is an event this service publishes onto the account's
    /// default EventBridge bus, and the queue is what carries it from the critical section
    /// that observed it to the publication, which is made with no guard held. It is bounded
    /// ([`MAX_PENDING_EVENTS`], oldest dropped first) because every run that comes to rest
    /// appends to it, and each entry names the Region whose bus it belongs on.
    pub pending_job_run_events: VecDeque<JobRunStateChange>,
    /// The crawler state changes this store has observed and not published yet, oldest first
    /// ([`crawler_events`]).
    ///
    /// A crawl that has COME TO REST is an event this service publishes onto the account's
    /// default EventBridge bus, and the queue is what carries it from the critical section that
    /// observed it to the publication, which is made with no guard held. It is bounded
    /// ([`MAX_PENDING_CRAWLER_EVENTS`], oldest dropped first) because every crawl that finishes
    /// appends to it, and each entry names the Region whose bus it belongs on.
    pub pending_crawler_events: VecDeque<CrawlerStateChange>,
    /// Whether this store's CLOCK PUMP is running ([`drive_clock`]), so at most one does.
    pub clock_pump_running: bool,
    /// Which claim on that pump is the live one: a pump releases its own claim and never a
    /// later pump's.
    pub clock_pump_generation: u64,
    /// The running pump's task handle, kept so the pump it replaces is aborted rather than
    /// merely forgotten.
    pub clock_pump: Option<::tokio::task::JoinHandle<()>>,
}

impl ComAmazonawsGlueAwsglueState {
    /// Give every column statistics task run the state the CLOCK already owed it at
    /// `now`.
    ///
    /// The one write a reporting operation performs, exactly as [`JobEntry::settle_due_runs`]
    /// is for a job run: a run's next state was due at
    /// [`ColumnStatisticsTaskRunEntry::settles_at`] whether or not anybody called, so
    /// materialising it on read reports the state the clock had already reached.
    ///
    /// A run that reaches its SUCCEEDED state is the run that generated the statistics of
    /// the columns it covered, so settling it also stores those statistics on the table —
    /// which is what makes them readable through `GetColumnStatisticsForTable`. The work
    /// itself was done when the run was started
    /// ([`compute_column_statistics`]); nothing is computed here. One walk moves the runs
    /// and the next writes the tables, because a run and the table it covers are reached
    /// through the same store.
    pub fn settle_due_column_statistics_task_runs(&mut self, now: SystemTime) {
        let mut generated: Vec<(String, String, String, SettledColumnStatistics)> = Vec::new();
        for run in &mut self.column_statistics_task_runs {
            if let Some(settled) = run.settle_if_due(now) {
                generated.push((
                    run.catalog_id.clone(),
                    run.database_name.clone(),
                    run.table_name.clone(),
                    settled,
                ));
            }
        }
        for (catalog_id, database_name, table_name, settled) in generated {
            // A table dropped while its run was in flight is a table there is nothing to
            // publish onto: the run still reports the status it reached.
            let Some(table) = self.table_mut(&catalog_id, &database_name, &table_name) else {
                continue;
            };
            for computed in &settled.statistics {
                store_column_statistics(
                    table,
                    computed.to_column_statistics(settled.analyzed_at),
                );
            }
        }
    }

    /// The stored column statistics task run with this id, or `None` when this account
    /// holds no such run.
    pub fn column_statistics_task_run(&self, id: &str) -> Option<&ColumnStatisticsTaskRunEntry> {
        self.column_statistics_task_runs
            .iter()
            .find(|run| run.id == id)
    }

    /// The column statistics task run of this table that has not finished, if one has
    /// not.
    ///
    /// Call [`Self::settle_due_column_statistics_task_runs`] first, or a run the clock has
    /// already finished still counts as in flight. `database_name` and `table_name` are
    /// matched folded, the way the catalog stores them.
    pub fn running_column_statistics_task_run(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&ColumnStatisticsTaskRunEntry> {
        let database = fold_name(database_name);
        let table = fold_name(table_name);
        self.column_statistics_task_runs
            .iter()
            .find(|run| run.is_running_for(catalog_id, &database, &table))
    }

    /// The column statistics task run of this table that has not finished, mutably: the
    /// one `StopColumnStatisticsTaskRun` moves into its stopped state.
    ///
    /// The lookup rule is [`Self::running_column_statistics_task_run`]'s — both ask
    /// [`ColumnStatisticsTaskRunEntry::is_running_for`] — so the run a stop ends is
    /// exactly the run a second start would have been refused for.
    pub fn running_column_statistics_task_run_mut(
        &mut self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&mut ColumnStatisticsTaskRunEntry> {
        let database = fold_name(database_name);
        let table = fold_name(table_name);
        self.column_statistics_task_runs
            .iter_mut()
            .find(|run| run.is_running_for(catalog_id, &database, &table))
    }

    /// The LAST column statistics task run started for this table, or `None` when none
    /// has been.
    ///
    /// [`Self::column_statistics_task_runs`] is kept in start order, so the last match is
    /// the most recent attempt — which is what a settings read reports as its
    /// `LastExecutionAttempt`. Call
    /// [`Self::settle_due_column_statistics_task_runs`] first, or the attempt is reported
    /// in a status the clock has already moved it out of. `database_name` and `table_name`
    /// are matched folded, the way the catalog stores them.
    pub fn last_column_statistics_task_run(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&ColumnStatisticsTaskRunEntry> {
        let database = fold_name(database_name);
        let table = fold_name(table_name);
        self.column_statistics_task_runs.iter().rev().find(|run| {
            run.catalog_id == catalog_id
                && run.database_name == database
                && run.table_name == table
        })
    }

    /// Append a column statistics task run, keeping at most
    /// [`MAX_COLUMN_STATISTICS_TASK_RUNS`] of them.
    pub fn record_column_statistics_task_run(&mut self, run: ColumnStatisticsTaskRunEntry) {
        self.column_statistics_task_runs.push(run);
        if self.column_statistics_task_runs.len() > MAX_COLUMN_STATISTICS_TASK_RUNS {
            let overflow = self.column_statistics_task_runs.len() - MAX_COLUMN_STATISTICS_TASK_RUNS;
            self.column_statistics_task_runs.drain(..overflow);
        }
    }

    /// Give every data quality rule recommendation run the state the CLOCK already owed
    /// it at `now`, exactly as [`Self::settle_due_column_statistics_task_runs`] does for a
    /// statistics run.
    pub fn settle_due_data_quality_rule_recommendation_runs(&mut self, now: SystemTime) {
        // A run a cancel asked to stop settles as that stop, whatever its data source
        // holds, so those are settled first and read no catalog at all.
        for run in &mut self.data_quality_rule_recommendation_runs {
            if run.is_stopping() {
                run.settle_stop_if_due(now);
            }
        }
        // What an ANALYSING run settles into is read off the catalog — the columns the
        // covered table declares — so the outcome of every run still due is resolved first
        // and applied afterwards: one walk reads the catalog, the next writes the runs.
        let outcomes: Vec<(usize, RecommendationRunOutcome)> = self
            .data_quality_rule_recommendation_runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.is_due(now))
            .map(|(index, run)| (index, self.covered_table_outcome(run)))
            .collect();
        for (index, outcome) in outcomes {
            if let Some(run) = self.data_quality_rule_recommendation_runs.get_mut(index) {
                run.settle(outcome);
            }
        }
    }

    /// What the data source of `run` offers a recommendation run: the columns the covered
    /// table declares, or the reason there is nothing to analyse.
    fn covered_table_outcome(
        &self,
        run: &DataQualityRuleRecommendationRunEntry,
    ) -> RecommendationRunOutcome {
        if let Some(reason) =
            self.catalog_table_miss(&run.catalog_id, &run.database_name, &run.table_name)
        {
            return RecommendationRunOutcome::NothingToAnalyse(reason);
        }
        let columns = self
            .database(&run.catalog_id, &run.database_name)
            .and_then(|database| database.tables.get(&fold_name(&run.table_name)))
            .map(table_column_names)
            .unwrap_or_default();
        RecommendationRunOutcome::Recommended(columns)
    }

    /// Why the catalog offers a data quality run nothing to read at this address — the
    /// database it does not hold, or the table that database does not hold — or `None` when
    /// it holds the table.
    ///
    /// Every data quality run resolves its data source against the catalog in these same
    /// terms, so the reason a run carries for a data source the catalog holds nothing for is
    /// worded here once rather than once per kind of run.
    pub fn catalog_table_miss(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<String> {
        let Some(database) = self.database(catalog_id, database_name) else {
            return Some(messages::database_not_found(database_name));
        };
        if database.tables.contains_key(&fold_name(table_name)) {
            None
        } else {
            Some(messages::table_not_found(table_name))
        }
    }

    /// The stored data quality rule recommendation run with this id, or `None` when this
    /// account holds no such run.
    pub fn data_quality_rule_recommendation_run(
        &self,
        id: &str,
    ) -> Option<&DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs
            .iter()
            .find(|run| run.id == id)
    }

    /// The stored data quality rule recommendation run with this id, mutably: what a cancel
    /// moves.
    pub fn data_quality_rule_recommendation_run_mut(
        &mut self,
        id: &str,
    ) -> Option<&mut DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs
            .iter_mut()
            .find(|run| run.id == id)
    }

    /// The run an earlier start with this `ClientToken` recorded, if one did.
    ///
    /// `StartDataQualityRuleRecommendationRunRequest$ClientToken` is "used for idempotency
    /// … to avoid creating or starting multiple instances of the same resource", so a start
    /// carrying a token already recorded is the duplicate the service refuses rather than a
    /// second run.
    pub fn data_quality_rule_recommendation_run_of_client_token(
        &self,
        client_token: &str,
    ) -> Option<&DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs
            .iter()
            .find(|run| run.client_token.as_deref() == Some(client_token))
    }

    /// The recommendation run of this data source that has not finished, if one has not.
    ///
    /// Call [`Self::settle_due_data_quality_rule_recommendation_runs`] first, or a run the
    /// clock has already finished still counts as in flight. `database_name` and
    /// `table_name` are matched folded, the way the catalog stores them.
    pub fn running_data_quality_rule_recommendation_run(
        &self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&DataQualityRuleRecommendationRunEntry> {
        self.data_quality_rule_recommendation_runs.iter().find(|run| {
            !run.is_terminal()
                && run.catalog_id == catalog_id
                && run.database_name == fold_name(database_name)
                && run.table_name == fold_name(table_name)
        })
    }

    /// Append a data quality rule recommendation run, keeping at most
    /// [`MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS`] of them.
    pub fn record_data_quality_rule_recommendation_run(
        &mut self,
        run: DataQualityRuleRecommendationRunEntry,
    ) {
        self.data_quality_rule_recommendation_runs.push(run);
        if self.data_quality_rule_recommendation_runs.len()
            > MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS
        {
            let overflow = self.data_quality_rule_recommendation_runs.len()
                - MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS;
            self.data_quality_rule_recommendation_runs.drain(..overflow);
        }
    }

    /// Give every data quality ruleset evaluation run the state the CLOCK already owed it at
    /// `now`, exactly as [`Self::settle_due_data_quality_rule_recommendation_runs`] does for
    /// a recommendation run.
    ///
    /// What a due run settles into is read off the catalog — whether it still holds every
    /// table the run evaluates — so the outcome of every run still due is resolved first and
    /// applied afterwards: one walk reads the catalog, the next writes the runs. A run that
    /// finished its evaluation publishes the results it produced
    /// ([`DataQualityResultEntry`]), which is what `GetDataQualityResult` reads back.
    pub fn settle_due_data_quality_ruleset_evaluation_runs(&mut self, now: SystemTime) {
        let misses: Vec<(usize, Option<String>)> = self
            .data_quality_ruleset_evaluation_runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.is_due(now))
            .map(|(index, run)| (index, self.evaluated_tables_miss(run)))
            .collect();
        let mut produced: Vec<DataQualityResultEntry> = Vec::new();
        for (index, miss) in misses {
            if let Some(run) = self.data_quality_ruleset_evaluation_runs.get_mut(index) {
                produced.extend(run.settle(miss));
            }
        }
        self.record_data_quality_results(produced);
    }

    /// Why the catalog offers `run` nothing to evaluate — the first of its data sources it
    /// holds no table for, in the order the run records them, named as the start spelled it —
    /// or `None` when it holds every one of them.
    fn evaluated_tables_miss(&self, run: &DataQualityRulesetEvaluationRunEntry) -> Option<String> {
        run.evaluated_tables().find_map(|table| {
            self.catalog_table_miss(&table.catalog_id, &table.database_name, &table.table_name)
        })
    }

    /// The stored data quality ruleset evaluation run with this id, or `None` when this
    /// account holds no such run.
    pub fn data_quality_ruleset_evaluation_run(
        &self,
        id: &str,
    ) -> Option<&DataQualityRulesetEvaluationRunEntry> {
        self.data_quality_ruleset_evaluation_runs
            .iter()
            .find(|run| run.id == id)
    }

    /// The evaluation run an earlier start with this `ClientToken` recorded, if one did.
    ///
    /// `StartDataQualityRulesetEvaluationRunRequest$ClientToken` is "used for idempotency
    /// … to avoid creating or starting multiple instances of the same resource", so a start
    /// carrying a token already recorded is the duplicate the service refuses rather than a
    /// second run.
    pub fn data_quality_ruleset_evaluation_run_of_client_token(
        &self,
        client_token: &str,
    ) -> Option<&DataQualityRulesetEvaluationRunEntry> {
        self.data_quality_ruleset_evaluation_runs
            .iter()
            .find(|run| run.client_token.as_deref() == Some(client_token))
    }

    /// Append a data quality ruleset evaluation run, keeping at most
    /// [`MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS`] of them.
    pub fn record_data_quality_ruleset_evaluation_run(
        &mut self,
        run: DataQualityRulesetEvaluationRunEntry,
    ) {
        self.data_quality_ruleset_evaluation_runs.push(run);
        if self.data_quality_ruleset_evaluation_runs.len()
            > MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS
        {
            let overflow = self.data_quality_ruleset_evaluation_runs.len()
                - MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS;
            self.data_quality_ruleset_evaluation_runs.drain(..overflow);
        }
    }

    /// The stored database, or `None` when either the catalog or the database is absent.
    pub fn database(&self, catalog_id: &str, name: &str) -> Option<&DatabaseEntry> {
        self.catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.databases.get(&fold_name(name)))
    }

    /// The stored database, mutably.
    pub fn database_mut(&mut self, catalog_id: &str, name: &str) -> Option<&mut DatabaseEntry> {
        self.catalogs
            .get_mut(catalog_id)
            .and_then(|catalog| catalog.databases.get_mut(&fold_name(name)))
    }

    /// The stored table, mutably: `None` when the catalog, the database or the table is
    /// absent. Names are matched folded, the way the catalog stores them.
    pub fn table_mut(
        &mut self,
        catalog_id: &str,
        database_name: &str,
        table_name: &str,
    ) -> Option<&mut TableEntry> {
        let table = fold_name(table_name);
        self.database_mut(catalog_id, database_name)
            .and_then(|database| database.tables.get_mut(&table))
    }

    /// The stored connection definition `name` of `catalog_id`, or `None` when either the
    /// catalog or the connection is absent.
    pub fn connection(&self, catalog_id: &str, name: &str) -> Option<&ConnectionEntry> {
        self.catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.connections.get(name))
    }

    /// The stored connection definition `name` of `catalog_id`, mutably.
    pub fn connection_mut(
        &mut self,
        catalog_id: &str,
        name: &str,
    ) -> Option<&mut ConnectionEntry> {
        self.catalogs
            .get_mut(catalog_id)
            .and_then(|catalog| catalog.connections.get_mut(name))
    }

    /// Remove the connection definition `name` from `catalog_id`, answering the record
    /// that was removed, or `None` when either the catalog or the connection is absent.
    ///
    /// The whole record goes, tag set included: a connection's tags are held ON the entry
    /// ([`Self::resource_tags`]), so the name a delete frees carries no tags into the next
    /// create under it. The removed entry is handed back rather than dropped, so a caller
    /// that has to report what it deleted reads it from here instead of looking the record
    /// up again before removing it. Matched by name exactly as sent, the way the read path
    /// matches it ([`Self::connection`]).
    pub fn remove_connection(&mut self, catalog_id: &str, name: &str) -> Option<ConnectionEntry> {
        self.catalogs
            .get_mut(catalog_id)
            .and_then(|catalog| catalog.connections.remove(name))
    }

    /// How many records the whole store holds of one catalog-scoped kind: `count` says
    /// how many ONE catalog holds, and this sums it over every catalog.
    ///
    /// The one walk the three store-wide counts below share, so a count and the ceiling
    /// checked against it can never disagree about which records they cover. Bounded by
    /// [`MAX_CATALOGS`] catalogs, so the walk itself is a bounded cost.
    fn stored_in_catalogs(&self, count: impl Fn(&Catalog) -> usize) -> usize {
        self.catalogs.values().map(count).sum()
    }

    /// How many tables the whole store holds. Bounded by the ceilings above, so this
    /// walk is over at most [`MAX_CATALOGS`] catalogs of
    /// [`MAX_DATABASES_PER_CATALOG`] entries each.
    pub fn stored_tables(&self) -> usize {
        self.stored_in_catalogs(|catalog| {
            catalog
                .databases
                .values()
                .map(|database| database.tables.len())
                .sum()
        })
    }

    /// Whether the store as a whole can hold another table, i.e. is below
    /// [`MAX_STORED_TABLES`].
    pub fn can_store_table(&self) -> bool {
        self.stored_tables() < MAX_STORED_TABLES
    }

    /// How many archived table versions the whole store holds. Bounded the same way
    /// [`Self::stored_tables`] is, over at most [`MAX_STORED_TABLES`] tables.
    pub fn stored_archived_table_versions(&self) -> usize {
        self.stored_in_catalogs(|catalog| {
            catalog
                .databases
                .values()
                .map(|database| {
                    database
                        .tables
                        .values()
                        .map(|table| table.archived_versions.len())
                        .sum::<usize>()
                })
                .sum()
        })
    }

    /// Whether the store as a whole can archive another table version, i.e. is below
    /// [`MAX_STORED_ARCHIVED_TABLE_VERSIONS`].
    pub fn can_archive_table_version(&self) -> bool {
        self.stored_archived_table_versions() < MAX_STORED_ARCHIVED_TABLE_VERSIONS
    }

    /// How many partitions the whole store holds. Bounded the same way
    /// [`Self::stored_tables`] is, over at most [`MAX_STORED_TABLES`] tables.
    pub fn stored_partitions(&self) -> usize {
        self.stored_in_catalogs(|catalog| {
            catalog
                .databases
                .values()
                .map(|database| {
                    database
                        .tables
                        .values()
                        .map(|table| table.partitions.len())
                        .sum::<usize>()
                })
                .sum()
        })
    }

    /// Whether the store as a whole can hold another partition, i.e. is below
    /// [`MAX_STORED_PARTITIONS`].
    ///
    /// A batch create writes SEVERAL partitions under one lock, so a caller that walks a
    /// batch reads this count once and then counts the partitions it has inserted
    /// itself: re-walking the store per item would be the same answer at a cost per
    /// item.
    pub fn can_store_partition(&self) -> bool {
        self.stored_partitions() < MAX_STORED_PARTITIONS
    }

    /// How many databases the whole store holds, across every catalog. Bounded by
    /// [`MAX_CATALOGS`] times [`MAX_DATABASES_PER_CATALOG`], so this walk is bounded.
    pub fn stored_databases(&self) -> usize {
        self.stored_in_catalogs(|catalog| catalog.databases.len())
    }

    /// Whether the store as a whole can hold another database, i.e. is below
    /// [`MAX_STORED_DATABASES`].
    pub fn can_store_database(&self) -> bool {
        self.stored_databases() < MAX_STORED_DATABASES
    }

    /// How many connection definitions the whole store holds, across every catalog.
    pub fn stored_connections(&self) -> usize {
        self.stored_in_catalogs(|catalog| catalog.connections.len())
    }

    /// Whether the store as a whole can hold another connection definition, i.e. is
    /// below [`MAX_STORED_CONNECTIONS`].
    pub fn can_store_connection(&self) -> bool {
        self.stored_connections() < MAX_STORED_CONNECTIONS
    }

    /// The catalog `catalog_id`, for a create that is about to write a record into it:
    /// the one this store already holds, or a fresh empty one when there is room for
    /// another container ([`MAX_CATALOGS`]). `None` means the store holds no such catalog
    /// and cannot take one, so the create must be refused rather than served.
    ///
    /// The ONE place a catalog is brought into existence, so no create can grow the
    /// container map past the ceiling, and a create that is refused earlier leaves no
    /// empty catalog behind: a handler calls this only once every other check has passed.
    pub fn catalog_mut_for_create(&mut self, catalog_id: &str) -> Option<&mut Catalog> {
        if !self.catalogs.contains_key(catalog_id) && self.catalogs.len() >= MAX_CATALOGS {
            return None;
        }
        Some(self.catalogs.entry(catalog_id.to_string()).or_default())
    }

    /// The catalog RESOURCE a `CreateCatalog` registered under `reference`, or `None`
    /// when this store holds no catalog of that name in that account.
    ///
    /// A catalog is keyed by its folded name ([`fold_name`]), so a read reaches it under
    /// any casing. The account's own Data Catalog is not one of these records — it is a
    /// container nothing created ([`Catalog::definition`]) — so an id naming it resolves
    /// to nothing here.
    pub fn catalog_definition(&self, reference: &CatalogReference) -> Option<&CatalogDefinition> {
        self.catalogs
            .get(&child_catalog_id(
                &reference.account_id,
                &fold_name(&reference.name),
            ))
            .and_then(|catalog| catalog.definition.as_ref())
    }

    /// Remove the catalog RESOURCE `reference` names, handing back the entry that held
    /// it, or `None` when this store holds no catalog of that name in that account — the
    /// delete counterpart of [`Self::catalog_definition`], keyed the same way, so a
    /// delete reaches a catalog under any casing of its name.
    ///
    /// The WHOLE entry goes, not just its definition: what a catalog holds is stored
    /// inside it, which is the cascade a catalog delete describes ("you no longer have
    /// access to the databases, tables ... and the user-defined functions in the deleted
    /// catalog" — `DeleteCatalog`). An entry carrying no definition is a container no
    /// create wrote — the account's own Data Catalog — and so no catalog resource a
    /// delete addresses; it is left where it is.
    pub fn remove_catalog(&mut self, reference: &CatalogReference) -> Option<Catalog> {
        let key = child_catalog_id(&reference.account_id, &fold_name(&reference.name));
        if self
            .catalogs
            .get(&key)
            .is_none_or(|catalog| catalog.definition.is_none())
        {
            return None;
        }
        self.catalogs.remove(&key)
    }

    /// The Data Catalog encryption settings `catalog_id` REPORTS: the record a
    /// `PutDataCatalogEncryptionSettings` stored for it, with each setting the stored
    /// record left unset filled in with that setting's default
    /// ([`default_encryption_at_rest`], [`default_connection_password_encryption`]).
    ///
    /// Both settings are therefore always reported, for a catalog nothing ever
    /// configured as much as for one a write named only half of: the catalog holds a
    /// value for each of them at all times, and a write that named one setting leaves
    /// the other at its default rather than at whatever an earlier write put there — the
    /// stored record is the whole settings structure the write carried.
    pub fn data_catalog_encryption_settings(
        &self,
        catalog_id: &str,
    ) -> DataCatalogEncryptionSettings {
        let stored = self
            .catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.encryption_settings.as_ref());
        DataCatalogEncryptionSettings {
            encryption_at_rest: Some(
                stored
                    .and_then(|settings| settings.encryption_at_rest.clone())
                    .unwrap_or_else(default_encryption_at_rest),
            ),
            connection_password_encryption: Some(
                stored
                    .and_then(|settings| settings.connection_password_encryption.clone())
                    .unwrap_or_else(default_connection_password_encryption),
            ),
        }
    }

    /// The export configuration record stored for `catalog_id`, or `None` while no
    /// `PutDataCatalogExportConfiguration` has written one for it — the record a write reads
    /// back and a read reports through
    /// [`DataCatalogExportConfigurationEntry::to_output`].
    pub fn data_catalog_export_configuration(
        &self,
        catalog_id: &str,
    ) -> Option<&DataCatalogExportConfigurationEntry> {
        self.catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.export_configuration.as_ref())
    }

    /// The migration status `catalog_id` REPORTS: whether a migration of an
    /// Athena-managed data catalog into it has completed, the marker the service names as
    /// its importer, and the instant that status was stamped — the instant an
    /// `ImportCatalogToGlue` was served here ([`CatalogMigrationEntry`]) where one was.
    ///
    /// A Data Catalog this emulator serves is a Glue Data Catalog and holds no migration
    /// still running: an import has nothing left to copy once it has been served, so the
    /// catalog's migration is complete whether an import ran here or the catalog was a
    /// Glue Data Catalog all along — which is what "`True` if the migration has
    /// completed" reports.
    ///
    /// Both of the other members are the SERVICE's, not the caller's. The importer is the
    /// service's own marker ([`STATUS_SET_BY_DEFAULT`]) rather than a person or a
    /// principal: no request this service serves carries "the person who initiated the
    /// migration", so there is none to report. The instant is the one the reported status
    /// was stamped at — the instant of the migration the catalog's record holds, or the
    /// store's own stamp ([`Self::catalog_status_stamp`]) for a catalog no import has
    /// migrated — so the member is reported for every catalog rather than only for a
    /// migrated one.
    pub fn catalog_import_status(&self, catalog_id: &str) -> CatalogImportStatus {
        let migration = self
            .catalogs
            .get(catalog_id)
            .and_then(|catalog| catalog.migration.as_ref());
        CatalogImportStatus {
            import_completed: true,
            import_time: Some(to_date_time(migration.map_or_else(
                || self.catalog_status_stamp.instant(),
                |stored| stored.import_time,
            ))),
            imported_by: Some(STATUS_SET_BY_DEFAULT.to_string()),
        }
    }

    /// The resource policy attached to `resource_arn`, or `None` when no policy is
    /// attached to that resource.
    ///
    /// `resource_arn` is the ARN the request addressed, as
    /// [`resolve_policy_resource_arn`] resolved it, so a request that named none and one
    /// that spelled the Data Catalog's ARN out resolve to the same record.
    pub fn resource_policy(&self, resource_arn: &str) -> Option<&ResourcePolicyEntry> {
        self.resource_policies.get(resource_arn)
    }

    /// Whether the store can hold another resource policy, i.e. is below
    /// [`MAX_RESOURCE_POLICIES`].
    pub fn can_store_resource_policy(&self) -> bool {
        self.resource_policies.len() < MAX_RESOURCE_POLICIES
    }

    /// Attach `policy_in_json` to `resource_arn`, and answer the `PolicyHash` of the
    /// policy now attached.
    ///
    /// A resource that already carries a policy has that policy's DOCUMENT replaced \u2014 the
    /// put carries the whole document, so nothing is merged \u2014 while the record keeps the
    /// `CreateTime` of the policy that was first attached and takes `now` as its
    /// `UpdateTime`. `None` means the store holds no policy for this resource and cannot
    /// take another ([`MAX_RESOURCE_POLICIES`]), so the write must be refused rather than
    /// served.
    pub fn set_resource_policy(
        &mut self,
        resource_arn: &str,
        policy_in_json: String,
        now: SystemTime,
    ) -> Option<String> {
        if let Some(stored) = self.resource_policies.get_mut(resource_arn) {
            stored.replace_document(policy_in_json, now);
            return Some(stored.policy_hash.clone());
        }
        if !self.can_store_resource_policy() {
            return None;
        }
        let attached = ResourcePolicyEntry::new(policy_in_json, now);
        let policy_hash = attached.policy_hash.clone();
        self.resource_policies
            .insert(resource_arn.to_string(), attached);
        Some(policy_hash)
    }

    /// Detach the policy of `resource_arn`, answering whether one was attached.
    pub fn remove_resource_policy(&mut self, resource_arn: &str) -> bool {
        self.resource_policies.remove(resource_arn).is_some()
    }

    /// The policies attached to INDIVIDUAL resources, in the order their ARNs sort in.
    ///
    /// Every stored policy except an account-level Data Catalog policy
    /// ([`names_a_catalog`]), which is the set `GetResourcePolicies` enumerates: the
    /// catalog's own policy is reported by `GetResourcePolicy`, which addresses it
    /// directly, and is not one of the individual resources' policies that list reports.
    pub fn individual_resource_policies(&self) -> Vec<(&String, &ResourcePolicyEntry)> {
        self.resource_policies
            .iter()
            .filter(|(resource_arn, _)| !names_a_catalog(resource_arn))
            .collect()
    }

    /// Whether the store can hold another workflow, i.e. is below [`MAX_WORKFLOWS`].
    pub fn can_store_workflow(&self) -> bool {
        self.workflows.len() < MAX_WORKFLOWS
    }

    /// Whether the store can hold another blueprint, i.e. is below [`MAX_BLUEPRINTS`].
    pub fn can_store_blueprint(&self) -> bool {
        self.blueprints.len() < MAX_BLUEPRINTS
    }

    /// Register `entry` under its own name, which is the key every blueprint operation
    /// addresses a blueprint by.
    pub fn insert_blueprint(&mut self, entry: BlueprintEntry) {
        self.blueprints.insert(entry.name.clone(), entry);
    }

    /// The stored blueprint `name`, or `None` when this account holds no such blueprint.
    pub fn blueprint(&self, name: &str) -> Option<&BlueprintEntry> {
        self.blueprints.get(name)
    }

    /// [`Self::blueprint`] mutably: the one record every write to a blueprint lands in.
    pub fn blueprint_mut(&mut self, name: &str) -> Option<&mut BlueprintEntry> {
        self.blueprints.get_mut(name)
    }

    /// Remove the blueprint `name`, answering the record that was removed, or `None` when
    /// this account holds no blueprint under that name.
    ///
    /// The whole record goes: a blueprint's tags and the runs started of it are held ON the
    /// entry ([`BlueprintEntry::tags`], [`BlueprintEntry::runs`]), so the name a delete
    /// frees carries neither into the next create under it. The workflows those runs laid
    /// out are separate records of this store and are left alone, because a workflow
    /// outlives the run that created it. Matched by name exactly as the read path matches
    /// it ([`Self::blueprint`]), and the removed entry is handed back rather than dropped
    /// for symmetry with [`Self::remove_job`].
    pub fn remove_blueprint(&mut self, name: &str) -> Option<BlueprintEntry> {
        self.blueprints.remove(name)
    }

    /// Whether the store can hold another interactive session, i.e. is below
    /// [`MAX_SESSIONS`].
    pub fn can_store_session(&self) -> bool {
        self.stored_sessions() < MAX_SESSIONS
    }

    /// How many interactive sessions this store holds across every Region. The walk is over
    /// the Regions one process has been addressed in, so it is bounded.
    pub fn stored_sessions(&self) -> usize {
        self.sessions.values().map(BTreeMap::len).sum()
    }

    /// The session `id` of the Region `region`, or `None` when that Region holds no session
    /// under that id — which is every session of every other Region, because a session is
    /// served only through the endpoint that created it.
    pub fn session(&self, region: &str, id: &str) -> Option<&SessionEntry> {
        self.sessions
            .get(region)
            .and_then(|sessions| sessions.get(id))
    }

    /// [`Self::session`] mutably: the one record every write to a session lands in, so the
    /// operations that read, stop and delete a session reach the record its reads report
    /// rather than a copy of it.
    pub fn session_mut(&mut self, region: &str, id: &str) -> Option<&mut SessionEntry> {
        self.sessions
            .get_mut(region)
            .and_then(|sessions| sessions.get_mut(id))
    }

    /// Store `entry` as the Region `region`'s session under its own id.
    pub fn insert_session(&mut self, region: &str, entry: SessionEntry) {
        self.sessions
            .entry(region.to_string())
            .or_default()
            .insert(entry.id.clone(), entry);
    }

    /// Remove the session `id` from the Region `region`, answering the record that was
    /// removed, or `None` when that Region holds no session under that id.
    ///
    /// The whole record goes: the statements run on a session are held ON the entry
    /// ([`SessionEntry::statements`]), so the id a delete frees carries none of them into
    /// the next create under it. Matched by id and Region exactly as the read path matches
    /// them ([`Self::session`]), and the removed entry is handed back rather than dropped,
    /// for symmetry with [`Self::remove_job`].
    pub fn remove_session(&mut self, region: &str, id: &str) -> Option<SessionEntry> {
        self.sessions
            .get_mut(region)
            .and_then(|sessions| sessions.remove(id))
    }

    /// Start a run of the blueprint `input` names at `now` and answer the id minted for that
    /// run, or why no run could be started.
    ///
    /// The ONE place a blueprint run is started, so a run and the workflow it produced are
    /// written together or not at all. In order: the blueprint is resolved, the registration
    /// status the CLOCK already owed it is materialised, a run is refused of a registration
    /// that has not finished, the workflow the run lays out is created, and the run of it is
    /// recorded carrying what that layout came to.
    ///
    /// "The name of a workflow that is created as a result of a successful blueprint run"
    /// (`BlueprintRun$WorkflowName`) is what a run produces, and the workflow it creates
    /// carries the run that created it ("The details of the blueprint that this particular
    /// workflow is created from", `Workflow$BlueprintDetails`). A name this account already
    /// holds a workflow under is NOT overwritten: that run creates nothing and fails, the
    /// way any blueprint run that cannot create its entities does.
    pub fn start_blueprint_run(
        &mut self,
        input: &StartBlueprintRunInput,
        now: SystemTime,
    ) -> Result<String, BlueprintRunNotStarted> {
        // Whether the store has room for the workflow the run would create, read before the
        // two maps below are borrowed apart.
        let can_store_workflow = self.can_store_workflow();
        // The blueprint and the workflows are two different maps of this store, borrowed
        // apart here so the run and the workflow it lays out are written in ONE pass rather
        // than by resolving the blueprint a second time.
        let Self {
            blueprints,
            workflows,
            ..
        } = self;
        let Some(blueprint) = blueprints.get_mut(&input.blueprint_name) else {
            return Err(BlueprintRunNotStarted::NoSuchBlueprint);
        };
        // The status the clock had already made due, materialised before it is read: the
        // registration was owed its terminal status at its due instant whether or not
        // anybody called.
        blueprint.settle_if_due(now);
        if !blueprint.is_active() {
            return Err(BlueprintRunNotStarted::NoActiveDefinition);
        }

        let workflow_name =
            blueprint_run_workflow_name(&blueprint.name, input.parameters.as_deref());
        let run_id = new_blueprint_run_id();
        let layout = if workflows.contains_key(&workflow_name) {
            BlueprintRunLayout::WorkflowAlreadyExists(workflow_name)
        } else if can_store_workflow {
            insert_workflow_into(
                workflows,
                WorkflowEntry::from_blueprint_run(&workflow_name, &blueprint.name, &run_id, now),
            );
            BlueprintRunLayout::Created(workflow_name)
        } else {
            return Err(BlueprintRunNotStarted::WorkflowCeilingReached(MAX_WORKFLOWS));
        };

        blueprint.begin_run(input, run_id.clone(), now, &layout);
        Ok(run_id)
    }

    /// The stored workflow `name`, or `None` when this account holds no such workflow.
    pub fn workflow(&self, name: &str) -> Option<&WorkflowEntry> {
        self.workflows.get(name)
    }

    /// [`Self::workflow`] mutably: the one record every write to a workflow, and every
    /// write to one of its runs, lands in.
    pub fn workflow_mut(&mut self, name: &str) -> Option<&mut WorkflowEntry> {
        self.workflows.get_mut(name)
    }

    /// Remove the workflow `name`, answering the record that was removed, or `None` when
    /// this account holds no workflow under that name.
    ///
    /// The whole record goes: a workflow's tags, its default run properties, its blueprint
    /// attribution and the runs started of it are held ON the entry ([`WorkflowEntry`]), so
    /// the name a delete frees carries none of them into the next create under it. The
    /// triggers whose `WorkflowName` named it are their own records of this store and are
    /// left where they are, because a trigger is created, read and deleted by name alone and
    /// outlives the workflow it was attached to. Matched by name exactly as the read path
    /// matches it ([`Self::workflow`]), and the removed entry is handed back rather than
    /// dropped for symmetry with [`Self::remove_job`].
    pub fn remove_workflow(&mut self, name: &str) -> Option<WorkflowEntry> {
        self.workflows.remove(name)
    }

    /// The trigger of the Region `region` that can BEGIN a run of the workflow
    /// `workflow_name`, or `None` when that workflow has no entry point.
    ///
    /// A trigger belongs to the workflow its `WorkflowName` names, and it can begin a run
    /// unless firing it depends on something inside the workflow having run first: a
    /// `CONDITIONAL` trigger "fires when its predicate is satisfied"
    /// (`CreateTriggerRequest$Predicate` — "required when the trigger type is
    /// CONDITIONAL"), and that predicate's conditions are the states of this workflow's own
    /// jobs and crawlers, so it can only continue a run, never start one. Every other
    /// modelled `TriggerType` fires on something outside the workflow — a caller, the
    /// clock, an event — so any of them is an entry point.
    ///
    /// The walk is over the Region's triggers, bounded by [`MAX_TRIGGERS`], and takes them
    /// in name order (the map is ordered), so the same workflow resolves to the same entry
    /// point on every call.
    pub fn starting_trigger(&self, region: &str, workflow_name: &str) -> Option<&TriggerEntry> {
        self.triggers.get(region)?.values().find(|trigger| {
            trigger.workflow_name.as_deref() == Some(workflow_name)
                && trigger.trigger_type != TRIGGER_TYPE_CONDITIONAL
        })
    }

    /// The graph of the workflow `workflow_name`: "all the Glue components present in the
    /// workflow and all the directed connections between them" (`WorkflowGraph`).
    ///
    /// A workflow holds the triggers whose `WorkflowName` names it, and each of those
    /// triggers names the components it works with: "the actions initiated by this trigger"
    /// (`Trigger$Actions`), each naming a job or a crawler, and the conditions of "the
    /// predicate of this trigger, which defines when it will fire" (`Trigger$Predicate`),
    /// each naming the job or crawler whose state it waits on. So the nodes are those
    /// triggers plus every component they name, and an edge is directed the way the work
    /// flows: from a trigger to what it initiates, and from a component to the trigger that
    /// waits on it. A component named twice is one node, and a connection named twice is
    /// one edge.
    ///
    /// `region` is the Region the read is served in, which is where a trigger lives — a
    /// trigger is served only through the endpoint that created it — while a workflow is
    /// per-account, so a workflow read in another Region reports the components attached
    /// there.
    ///
    /// The walk is over that Region's triggers, bounded by [`MAX_TRIGGERS`], and takes them
    /// in name order (the map is ordered), so two reads of an unchanged store report the
    /// same graph.
    ///
    /// `run` is the workflow run the graph is reported BESIDE, whose job runs each `JOB`
    /// node reports as "the information for the job runs represented by the job node"
    /// (`JobNodeDetails$JobRuns`), and `None` for a read that reports no run at all.
    pub fn workflow_graph(
        &self,
        region: &str,
        workflow_name: &str,
        run: Option<&WorkflowRunEntry>,
    ) -> WorkflowGraph {
        let mut graph = WorkflowGraphBuilder::for_workflow(workflow_name);
        let attached = self
            .triggers
            .get(region)
            .into_iter()
            .flat_map(BTreeMap::values)
            .filter(|trigger| trigger.workflow_name.as_deref() == Some(workflow_name));
        for trigger in attached {
            let trigger_id = graph.node(NODE_TYPE_TRIGGER, &trigger.name, Some(trigger), &[]);
            for action in &trigger.actions {
                for (node_type, name) in
                    named_components(action.job_name.as_deref(), action.crawler_name.as_deref())
                {
                    let runs = self.component_job_runs(region, run, node_type, name);
                    let initiated = graph.node(node_type, name, None, &runs);
                    graph.edge(&trigger_id, &initiated);
                }
            }
            for condition in trigger
                .predicate
                .as_ref()
                .and_then(|predicate| predicate.conditions.as_ref())
                .into_iter()
                .flatten()
            {
                for (node_type, name) in named_components(
                    condition.job_name.as_deref(),
                    condition.crawler_name.as_deref(),
                ) {
                    let runs = self.component_job_runs(region, run, node_type, name);
                    let waited_on = graph.node(node_type, name, None, &runs);
                    graph.edge(&waited_on, &trigger_id);
                }
            }
        }
        graph.build()
    }

    /// The runs of the workflow run `run` that belong to the graph node for the component
    /// `name` of type `node_type`, as the modelled `JobRun` shape.
    ///
    /// Only a `JOB` node has job runs to report, and only the runs THIS workflow run began
    /// are its: a run of the same job that another workflow run — or a caller's own
    /// `StartJobRun` — began is not part of this run. The runs are read out of the job
    /// definition they belong to ([`JobEntry::runs`]), so a node reports the state each run
    /// has reached by now rather than the state it was begun in, and they are taken in the
    /// order the actions initiated them, so two reads of an unchanged store agree.
    fn component_job_runs(
        &self,
        region: &str,
        run: Option<&WorkflowRunEntry>,
        node_type: &str,
        name: &str,
    ) -> Vec<JobRun> {
        if node_type != NODE_TYPE_JOB {
            return Vec::new();
        }
        let Some(run) = run else {
            return Vec::new();
        };
        let Some(job) = self.job(region, name) else {
            return Vec::new();
        };
        run.action_job_runs
            .iter()
            .filter(|begun| begun.job_name == name)
            .filter_map(|begun| job.run(&begun.run_id))
            .map(JobRunEntry::to_job_run)
            .collect()
    }

    /// `WorkflowRun$Statistics`: "Workflow run statistics provides statistics about the
    /// workflow run."
    ///
    /// "Total number of Actions in the workflow run" is what the entry point that began the
    /// run initiates, which the run itself records ([`WorkflowRunEntry::total_actions`]).
    /// Every other count is of the run's actions that are IN one state, so each is taken
    /// from the state the job run that action began has reached by now — the same records
    /// the graph's `JOB` nodes report — rather than from anything stored on the run, so a
    /// later read reports the counts the clock has since moved.
    ///
    /// An action whose job run the store no longer holds, and a terminal `JobRunState` the
    /// shape has no counter for, are counted in none of them.
    pub fn workflow_run_statistics(
        &self,
        region: &str,
        run: &WorkflowRunEntry,
    ) -> WorkflowRunStatistics {
        let mut statistics = WorkflowRunStatistics {
            total_actions: run.total_actions,
            ..WorkflowRunStatistics::default()
        };
        for begun in &run.action_job_runs {
            let Some(state) = self
                .job(region, &begun.job_name)
                .and_then(|job| job.run(&begun.run_id))
                .map(|job_run| job_run.state.as_str())
            else {
                continue;
            };
            let counted = match state {
                JOB_RUN_STATE_SUCCEEDED => &mut statistics.succeeded_actions,
                JOB_RUN_STATE_FAILED => &mut statistics.failed_actions,
                JOB_RUN_STATE_TIMEOUT => &mut statistics.timeout_actions,
                JOB_RUN_STATE_STOPPED => &mut statistics.stopped_actions,
                JOB_RUN_STATE_ERROR => &mut statistics.errored_actions,
                JOB_RUN_STATE_WAITING => &mut statistics.waiting_actions,
                // "Total number Actions in running state": a run that has not finished and
                // is not waiting for a slot is consuming resources.
                other if !TERMINAL_JOB_RUN_STATES.contains(&other) => {
                    &mut statistics.running_actions
                }
                _ => continue,
            };
            *counted = counted.saturating_add(1);
        }
        statistics
    }

    /// The modelled `Workflow` for the stored workflow `workflow`, read in `region`.
    ///
    /// `include_graph` is the reader's own `IncludeGraph`; the graph is reported beside
    /// "the information about the last execution of the workflow" (`Workflow$LastRun`), so
    /// that run is the one its `JOB` nodes report the job runs of.
    pub fn to_workflow(
        &self,
        region: &str,
        workflow: &WorkflowEntry,
        include_graph: bool,
    ) -> Workflow {
        let last_run = workflow.runs.last();
        let graph =
            include_graph.then(|| self.workflow_graph(region, &workflow.name, last_run));
        workflow.to_workflow(
            graph,
            last_run.map(|run| {
                run.to_workflow_run(None, self.workflow_run_statistics(region, run))
            }),
        )
    }

    /// The modelled `WorkflowRun` for the stored run `run`, read in `region`: the ONE
    /// projection every read of a workflow run serves, so the single-run read and the run
    /// listing report a run identically.
    ///
    /// `include_graph` is the reader's own `IncludeGraph`, and the graph it asks for is the
    /// graph of THIS run — its `JOB` nodes report the runs this run's actions began.
    pub fn to_workflow_run(
        &self,
        region: &str,
        run: &WorkflowRunEntry,
        include_graph: bool,
    ) -> WorkflowRun {
        let graph =
            include_graph.then(|| self.workflow_graph(region, &run.workflow_name, Some(run)));
        run.to_workflow_run(graph, self.workflow_run_statistics(region, run))
    }

    /// Give the workflow `workflow_name`'s runs, and the job runs their actions began, the
    /// state the CLOCK already owed each of them at `now`.
    ///
    /// The ONE materialisation every read of a workflow or of one of its runs performs, so
    /// no two reads can disagree about what the clock has made of a run. It is the one write
    /// a reporting operation makes: each state was due at the record's own settle instant
    /// whether or not anybody called.
    ///
    /// A workflow run is a COMPOSITE of the work it initiated — "total number of Actions in
    /// the workflow run", each of them a run of a job — so it comes to rest only once those
    /// runs have: while any of them is still owed a state, the run's own due instant is held
    /// at the latest instant one of its actions is owed one, and the run is reported as still
    /// running. The actions are settled FIRST, so what they have come to is what decides it.
    pub fn settle_due_workflow_runs(
        &mut self,
        region: &str,
        workflow_name: &str,
        now: SystemTime,
    ) {
        self.settle_due_action_job_runs(region, workflow_name, now);
        let awaited = self.workflow_runs_awaiting_their_actions(region, workflow_name);
        let Some(workflow) = self.workflow_mut(workflow_name) else {
            return;
        };
        for run in &mut workflow.runs {
            if let (Some(action_due), Some(due)) = (awaited.get(&run.id), run.settles_at) {
                run.settles_at = Some(due.max(*action_due));
            }
            run.settle_if_due(now);
        }
    }

    /// For each run of the workflow `workflow_name` whose actions have NOT all come to rest,
    /// the latest instant one of those actions is owed its next state.
    ///
    /// A run whose actions have all come to rest, and one whose actions began no job run at
    /// all, is absent from the answer: there is nothing left for it to wait for. An action
    /// whose job run the store no longer holds is nothing to wait for either.
    ///
    /// The walk is over the runs this workflow holds, bounded by [`MAX_RUNS_PER_WORKFLOW`],
    /// each naming the actions of one trigger.
    fn workflow_runs_awaiting_their_actions(
        &self,
        region: &str,
        workflow_name: &str,
    ) -> BTreeMap<String, SystemTime> {
        let Some(workflow) = self.workflow(workflow_name) else {
            return BTreeMap::new();
        };
        let mut awaited = BTreeMap::new();
        for run in &workflow.runs {
            let latest = run
                .action_job_runs
                .iter()
                .filter_map(|begun| {
                    self.job(region, &begun.job_name)
                        .and_then(|job| job.run(&begun.run_id))
                })
                .filter(|job_run| !job_run.is_terminal())
                .filter_map(|job_run| job_run.settles_at)
                .max();
            if let Some(latest) = latest {
                awaited.insert(run.id.clone(), latest);
            }
        }
        awaited
    }

    /// Give the job runs the workflow `workflow_name`'s runs began the state the CLOCK
    /// already owed them at `now`.
    ///
    /// A read of one of those workflow runs reports the state each of its actions has
    /// reached — under the `JOB` nodes of its graph and in its statistics — and that state
    /// was due at the job run's own settle instant whether or not anybody called, so the
    /// read materialises it exactly as it materialises the statuses the workflow's own runs
    /// were owed ([`WorkflowEntry::settle_due_runs`]). Nothing else about those runs moves.
    ///
    /// The walk is over the jobs this workflow's runs named, bounded by
    /// [`MAX_RUNS_PER_WORKFLOW`] runs each naming the actions of one trigger.
    pub fn settle_due_action_job_runs(
        &mut self,
        region: &str,
        workflow_name: &str,
        now: SystemTime,
    ) {
        for job_name in self.action_job_run_job_names(workflow_name) {
            if let Some(job) = self.job_mut(region, &job_name) {
                job.settle_due_runs(now);
            }
        }
    }

    /// The job definitions the runs of the workflow `workflow_name` began runs of, in name
    /// order and without repetition.
    ///
    /// The jobs a read of one of those workflow runs reports the state of, which is also the
    /// set whose owed SCRIPT executions such a read performs
    /// ([`perform_owed_job_runs`]). The walk is over the runs this workflow's runs named,
    /// bounded by [`MAX_RUNS_PER_WORKFLOW`] runs each naming the actions of one trigger.
    #[must_use]
    pub fn action_job_run_job_names(&self, workflow_name: &str) -> Vec<String> {
        let named: BTreeSet<String> = self
            .workflow(workflow_name)
            .into_iter()
            .flat_map(|workflow| workflow.runs.iter())
            .flat_map(|run| run.action_job_runs.iter())
            .map(|begun| begun.job_name.clone())
            .collect();
        named.into_iter().collect()
    }

    /// Record on the run `run_id` of the workflow `workflow_name` the job runs `begun` that
    /// its actions started, so a later read of that run reports them.
    ///
    /// The association lives on the RUN because a job run is addressed by its job and its
    /// own id and carries no workflow run of its own: without it, two runs of the same
    /// workflow could not be told apart by what each initiated.
    pub fn record_workflow_run_job_runs(
        &mut self,
        workflow_name: &str,
        run_id: &str,
        begun: &[ActionJobRun],
    ) {
        if begun.is_empty() {
            return;
        }
        if let Some(run) = self
            .workflow_mut(workflow_name)
            .and_then(|workflow| workflow.run_mut(run_id))
        {
            run.action_job_runs.extend(begun.iter().cloned());
        }
    }

    /// Store `entry` as this account's workflow under its own name, which is the key every
    /// read addresses it by ([`Self::workflow`]) — so the namespace's key form lives in
    /// [`insert_workflow_into`] rather than in each handler that writes to it.
    pub fn insert_workflow(&mut self, entry: WorkflowEntry) {
        insert_workflow_into(&mut self.workflows, entry);
    }

    /// Whether the store can hold another crawler, i.e. is below [`MAX_CRAWLERS`].
    pub fn can_store_crawler(&self) -> bool {
        self.crawlers.len() < MAX_CRAWLERS
    }

    /// The crawler `name`, or `None` when this account holds no crawler under that name.
    ///
    /// A crawler is addressed by name alone, in ONE per-account namespace — the rule the
    /// `crawlers` field documents — so this is where that addressing lives, as
    /// [`Self::job`] is for a job definition's Region-scoped namespace.
    pub fn crawler(&self, name: &str) -> Option<&CrawlerEntry> {
        self.crawlers.get(name)
    }

    /// Whether the store can hold another job definition, i.e. is below [`MAX_JOBS`].
    pub fn can_store_job(&self) -> bool {
        self.stored_jobs() < MAX_JOBS
    }

    /// How many job definitions this store holds across every Region. The walk is over
    /// the Regions one process has been addressed in, so it is bounded.
    pub fn stored_jobs(&self) -> usize {
        self.jobs.values().map(HashMap::len).sum()
    }

    /// The job definition `name` of the Region `region`, or `None` when that Region holds
    /// no job under that name — which is every job of every other Region, because a Glue
    /// job exists only in the Region it was created in.
    pub fn job(&self, region: &str, name: &str) -> Option<&JobEntry> {
        self.jobs.get(region).and_then(|jobs| jobs.get(name))
    }

    /// [`Self::job`] mutably: the one record every write to a job definition, and every
    /// write to one of its runs, lands in.
    pub fn job_mut(&mut self, region: &str, name: &str) -> Option<&mut JobEntry> {
        self.jobs
            .get_mut(region)
            .and_then(|jobs| jobs.get_mut(name))
    }

    /// Store `entry` as the Region `region`'s job definition under its own name.
    pub fn insert_job(&mut self, region: &str, entry: JobEntry) {
        self.jobs
            .entry(region.to_string())
            .or_default()
            .insert(entry.name.clone(), entry);
    }

    /// Remove the job definition `name` from the Region `region`, answering the record
    /// that was removed, or `None` when that Region holds no job under that name.
    ///
    /// The whole record goes: a job's tags and the runs started of it are held ON the
    /// entry ([`JobEntry::tags`], [`JobEntry::runs`]), so the name a delete frees carries
    /// neither into the next create under it. The removed entry is handed back rather
    /// than dropped, for symmetry with [`Self::remove_connection`] — today's caller only
    /// needs to know WHETHER a record went. Matched by name and Region exactly as the
    /// read path matches them ([`Self::job`]).
    pub fn remove_job(&mut self, region: &str, name: &str) -> Option<JobEntry> {
        self.jobs
            .get_mut(region)
            .and_then(|jobs| jobs.remove(name))
    }

    /// Every job definition the Region `region` holds, in name order.
    pub fn jobs_in(&self, region: &str) -> Vec<&JobEntry> {
        let mut found: Vec<&JobEntry> = self
            .jobs
            .get(region)
            .into_iter()
            .flat_map(HashMap::values)
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name));
        found
    }

    /// Whether the store can hold another trigger, i.e. is below [`MAX_TRIGGERS`].
    pub fn can_store_trigger(&self) -> bool {
        self.stored_triggers() < MAX_TRIGGERS
    }

    /// How many triggers this store holds across every Region. The walk is over the
    /// Regions one process has been addressed in, so it is bounded.
    pub fn stored_triggers(&self) -> usize {
        self.triggers.values().map(BTreeMap::len).sum()
    }

    /// The trigger `name` of the Region `region`, or `None` when that Region holds no
    /// trigger under that name — which is every trigger of every other Region, because a
    /// trigger is served only through the endpoint that created it.
    pub fn trigger(&self, region: &str, name: &str) -> Option<&TriggerEntry> {
        self.triggers
            .get(region)
            .and_then(|triggers| triggers.get(name))
    }

    /// [`Self::trigger`] mutably: where every write to a stored trigger lands, so the
    /// operations that start, stop and update a trigger reach the one record its reads
    /// report rather than a copy of it.
    pub fn trigger_mut(&mut self, region: &str, name: &str) -> Option<&mut TriggerEntry> {
        self.triggers
            .get_mut(region)
            .and_then(|triggers| triggers.get_mut(name))
    }

    /// Every trigger the Region `region` holds, in name order — the triggers served
    /// through this endpoint, which is every trigger reads of this Region can address.
    ///
    /// The order is the inner map's own key order, so a listing that walks this set pages
    /// deterministically across calls.
    pub fn triggers_in(&self, region: &str) -> Vec<&TriggerEntry> {
        self.triggers
            .get(region)
            .into_iter()
            .flat_map(BTreeMap::values)
            .collect()
    }

    /// Store `entry` as the Region `region`'s trigger under its own name.
    pub fn insert_trigger(&mut self, region: &str, entry: TriggerEntry) {
        self.triggers
            .entry(region.to_string())
            .or_default()
            .insert(entry.name.clone(), entry);
    }

    /// Remove the trigger `name` from the Region `region`, answering the record that was
    /// removed, or `None` when that Region holds no trigger under that name.
    ///
    /// The whole record goes: a trigger's tags, its state and the definition it fires on
    /// are held ON the entry ([`TriggerEntry`]), so the name a delete frees carries none of
    /// them into the next create under it. The jobs and crawlers its actions named, and the
    /// workflow it was attached to, are their own records of this store and are left where
    /// they are. Matched by name and Region exactly as the read path matches them
    /// ([`Self::trigger`]), and the removed entry is handed back rather than dropped for
    /// symmetry with [`Self::remove_job`].
    pub fn remove_trigger(&mut self, region: &str, name: &str) -> Option<TriggerEntry> {
        self.triggers
            .get_mut(region)
            .and_then(|triggers| triggers.remove(name))
    }

    /// Initiate `actions` — the actions of the trigger `trigger_name`, which has just FIRED
    /// in the Region `region` — and answer every job run they began, in action order.
    ///
    /// "Defines an action to be initiated by a trigger" (`Action`): an action naming a job
    /// begins a RUN of that job, carrying the overrides the action names over the job's own
    /// definition — "The job arguments used when this trigger fires. For this job run, they
    /// replace the default arguments set in the job definition itself" (`Action$Arguments`),
    /// and the same for its `Timeout`, `SecurityConfiguration` and `NotificationProperty` —
    /// so what a firing begins is the record [`JobRunEntry::start`] builds for a direct
    /// start of that job, named as the run of a trigger (`JobRun$TriggerName`), and a later
    /// read of the job's runs reports it.
    ///
    /// The ceiling a direct start respects bounds a firing too: a job already at its
    /// `ExecutionProperty$MaxConcurrentRuns` takes no further run, and because the firing is
    /// not the caller's own start of that job, the firing begins the runs it can rather than
    /// being refused. An action naming a job this Region no longer holds begins no run for
    /// the same reason.
    ///
    /// An action's `CrawlerName` initiates a CRAWL, which is the crawl lifecycle
    /// `StartCrawler` owns and which this crate does not hold yet ([`CrawlerEntry`] records
    /// no crawl), so such an action begins nothing here.
    pub fn initiate_trigger_actions(
        &mut self,
        region: &str,
        trigger_name: &str,
        actions: &[Action],
        now: SystemTime,
    ) -> Vec<ActionJobRun> {
        let mut begun = Vec::new();
        // The runs this firing was PRECEDED by, resolved before any job is borrowed to write
        // to, so every run the firing begins reports the same predecessors.
        let predecessors = self.satisfied_predicate_runs(region, trigger_name, now);
        for action in actions {
            let Some(job_name) = action.job_name.as_deref() else {
                continue;
            };
            let request = job_run_request_for_action(action, job_name);
            let Some(job) = self.job_mut(region, job_name) else {
                continue;
            };
            // Everything the clock already owed this job's runs is materialised before they
            // are counted, so a run it has already finished does not hold a slot.
            job.settle_due_runs(now);
            let allowed = usize::try_from(job.max_concurrent_runs()).unwrap_or(usize::MAX);
            if job.runs_in_flight() >= allowed {
                continue;
            }
            let run_id = new_job_run_id();
            let mut run = JobRunEntry::start(
                job,
                &request,
                run_id.clone(),
                now,
                None,
                Some(trigger_name),
            );
            // "A job run that was used in the predicate of a conditional trigger that
            // triggered this job run" (`Predecessor`): the runs that satisfied the firing
            // trigger's predicate precede every run the firing begins.
            run.predecessors = predecessors.clone();
            job.record_run(run);
            begun.push(ActionJobRun {
                job_name: job_name.to_string(),
                run_id,
            });
        }
        begun
    }

    /// The runs that SATISFIED the predicate of the trigger `trigger_name`, in condition
    /// order, as the `Predecessor` list a run that firing begins reports.
    ///
    /// "The predicate of this trigger, which defines when it will fire"
    /// (`Trigger$Predicate`) belongs to one trigger kind: only a CONDITIONAL trigger carries
    /// one, and each of its conditions names a job and the `JobRunState` that job has to
    /// have reached. The run that satisfied a condition is the LATEST run of that job in
    /// that state, which is the run the firing follows; a condition naming a crawler names
    /// no job run, and a condition no run of the job satisfies contributes none.
    fn satisfied_predicate_runs(
        &mut self,
        region: &str,
        trigger_name: &str,
        now: SystemTime,
    ) -> Vec<Predecessor> {
        let conditions: Vec<(String, Option<String>)> = self
            .trigger(region, trigger_name)
            .filter(|trigger| trigger.trigger_type == TRIGGER_TYPE_CONDITIONAL)
            .and_then(|trigger| trigger.predicate.as_ref())
            .and_then(|predicate| predicate.conditions.as_ref())
            .map(|conditions| {
                conditions
                    .iter()
                    .filter_map(|condition| {
                        condition
                            .job_name
                            .clone()
                            .map(|job_name| (job_name, condition.state.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut predecessors = Vec::new();
        for (job_name, state) in conditions {
            let Some(job) = self.job_mut(region, &job_name) else {
                continue;
            };
            // The state a run is in is the one the clock already owed it, so a run that has
            // reached the condition's state counts whether or not anybody read it.
            job.settle_due_runs(now);
            let satisfying = job
                .runs
                .iter()
                .rev()
                .find(|run| state.as_deref().is_none_or(|wanted| run.state == wanted))
                .map(|run| run.id.clone());
            if let Some(run_id) = satisfying {
                predecessors.push(Predecessor {
                    job_name: Some(job_name),
                    run_id: Some(run_id),
                });
            }
        }
        predecessors
    }

    /// The EVENT trigger of the Region `region` that the events delivered to the workflow
    /// `workflow_name` are counted against, or `None` when that workflow has none.
    ///
    /// An EVENT trigger is the one kind driven from outside this service: the events an
    /// EventBridge rule hands a workflow are counted against the `EventBatchingCondition`
    /// of the EVENT trigger that belongs to it ("number of events that must be received
    /// from Amazon EventBridge before EventBridge event trigger fires",
    /// `EventBatchingCondition$BatchSize`), so this is the trigger an ingress resolves. A
    /// trigger is served only through the endpoint that created it, so the search is over
    /// THIS Region's triggers, bounded by [`MAX_TRIGGERS`] and taken in name order (the map
    /// is ordered), so one workflow resolves to the same trigger on every call — the same
    /// way [`Self::starting_trigger`] resolves an entry point.
    pub fn event_trigger(&self, region: &str, workflow_name: &str) -> Option<&TriggerEntry> {
        self.triggers.get(region)?.values().find(|trigger| {
            trigger.workflow_name.as_deref() == Some(workflow_name)
                && trigger.trigger_type == TRIGGER_TYPE_EVENT
        })
    }

    /// Count ONE event EventBridge delivered to the workflow `workflow_name` against that
    /// workflow's EVENT trigger, and answer what became of the delivery.
    ///
    /// The ingress AWS exposes as `glue:notifyEvent`: an EventBridge rule whose target is a
    /// Glue workflow hands every matched event to that workflow, and the workflow's EVENT
    /// trigger is what turns those events into runs — "batch condition that must be met
    /// (specified number of events received or batch time window expired) before
    /// EventBridge event trigger fires" (`EventBatchingCondition`). So one delivery is
    /// counted ([`TriggerEntry::receive_event`]) and the trigger fires EXACTLY ONCE when
    /// the count reaches its `BatchSize`, beginning one run of the workflow and initiating
    /// that trigger's actions — the same firing [`Self::fire_event_trigger`] performs for a
    /// window that ran out.
    ///
    /// Before the event is counted, a batch whose window the clock has already run out
    /// fires on its own, so an event arriving after the window belongs to the next batch
    /// rather than to the one it closed.
    pub fn notify_workflow_event(
        &mut self,
        region: &str,
        workflow_name: &str,
        now: SystemTime,
    ) -> WorkflowEventDelivery {
        if self.workflow(workflow_name).is_none() {
            return WorkflowEventDelivery::NoWorkflow;
        }
        self.settle_due_event_batches(region, workflow_name, now);
        let Some(trigger) = self.event_trigger(region, workflow_name) else {
            return WorkflowEventDelivery::NoEventTrigger;
        };
        if !trigger.batches_events() {
            return WorkflowEventDelivery::Deactivated;
        }
        let trigger_name = trigger.name.clone();
        let fires = self
            .trigger_mut(region, &trigger_name)
            .is_some_and(|trigger| trigger.receive_event(now));
        if !fires {
            let (received, batch_size) = self
                .trigger(region, &trigger_name)
                .map_or((0, 0), |trigger| {
                    (trigger.batched_events, trigger.event_batch_size())
                });
            return WorkflowEventDelivery::Counted {
                received,
                batch_size,
            };
        }
        self.fire_event_trigger(region, &trigger_name, now)
    }

    /// Fire the batch of the EVENT trigger of the workflow `workflow_name` that the CLOCK
    /// has already run the window out on, at `now`.
    ///
    /// "Window of time in seconds after which EventBridge event trigger fires. Window
    /// starts when first event is received" (`EventBatchingCondition$BatchWindow`): a batch
    /// that has received at least one event and whose window has run out fires with the
    /// events it holds, and it was owed that firing at the instant the window ran out
    /// whether or not anybody called — so a read of the workflow's runs materialises it
    /// exactly as it materialises the statuses those runs were owed
    /// ([`WorkflowEntry::settle_due_runs`]), and the run begins at the DUE instant rather
    /// than at the instant of the read. Nothing else moves, and a batch still inside its
    /// window fires nothing.
    pub fn settle_due_event_batches(
        &mut self,
        region: &str,
        workflow_name: &str,
        now: SystemTime,
    ) {
        let Some(trigger) = self.event_trigger(region, workflow_name) else {
            return;
        };
        if !trigger.batches_events() {
            return;
        }
        let trigger_name = trigger.name.clone();
        let Some(due) = trigger
            .batch_window_due_at()
            .filter(|due| now >= *due)
        else {
            return;
        };
        if let Some(trigger) = self.trigger_mut(region, &trigger_name) {
            trigger.close_event_batch();
        }
        self.fire_event_trigger(region, &trigger_name, due);
    }

    /// Begin the one run of its workflow that the trigger `trigger_name` of `region` FIRING
    /// starts, at `now`, and answer what became of it.
    ///
    /// A firing begins a run of the workflow the trigger belongs to and initiates "the
    /// actions initiated by this trigger" (`Trigger$Actions`) inside it, which is what
    /// `StartWorkflowRun` does for the entry point a caller starts: the run is recorded
    /// through the ONE place a run of a workflow is recorded
    /// ([`WorkflowEntry::begin_run`]), so a run a trigger began is counted against the same
    /// concurrency ceiling as one a caller started and reported identically by every later
    /// read.
    fn fire_event_trigger(
        &mut self,
        region: &str,
        trigger_name: &str,
        now: SystemTime,
    ) -> WorkflowEventDelivery {
        // The workflow the firing runs and the actions it initiates, resolved before the
        // workflow is borrowed to write to.
        let Some((workflow_name, actions)) = self.trigger(region, trigger_name).and_then(|trigger| {
            trigger
                .workflow_name
                .clone()
                .map(|workflow_name| (workflow_name, trigger.actions.clone()))
        }) else {
            return WorkflowEventDelivery::NoWorkflow;
        };
        let total_actions = i32::try_from(actions.len()).unwrap_or(i32::MAX);
        let Some(workflow) = self.workflow_mut(&workflow_name) else {
            return WorkflowEventDelivery::NoWorkflow;
        };
        // A firing passes no run properties of its own, so the run carries the workflow's
        // own defaults.
        let run_id = match workflow.begin_run(None, now, total_actions) {
            Ok(run_id) => run_id,
            // "If you leave this parameter blank, there is no limit to the number of
            // concurrent workflow runs": past a ceiling the workflow named, the firing
            // begins no run, exactly as a caller's start is refused past it.
            Err(ceiling) => return WorkflowEventDelivery::ConcurrentRunsExceeded { ceiling },
        };
        let begun_job_runs = self.initiate_trigger_actions(region, trigger_name, &actions, now);
        self.record_workflow_run_job_runs(&workflow_name, &run_id, &begun_job_runs);
        ::tracing::info!(
            trigger = %trigger_name,
            workflow = %workflow_name,
            run = %run_id,
            job_runs = begun_job_runs.len(),
            "a Glue EVENT trigger's batch fired and began a workflow run"
        );
        WorkflowEventDelivery::Fired { run_id }
    }

    /// Whether the store can hold another schema registry, i.e. is below
    /// [`MAX_REGISTRIES`].
    pub fn can_store_registry(&self) -> bool {
        self.registries.len() < MAX_REGISTRIES
    }

    /// The stored registry `name`, or `None` when this store holds no such registry.
    pub fn registry(&self, name: &str) -> Option<&RegistryEntry> {
        self.registries.get(name)
    }

    /// The stored registry `name`, mutably.
    pub fn registry_mut(&mut self, name: &str) -> Option<&mut RegistryEntry> {
        self.registries.get_mut(name)
    }

    /// The stored schema set `schema_name` of registry `registry_name`, or `None` when
    /// either the registry or the schema is absent.
    pub fn schema(&self, registry_name: &str, schema_name: &str) -> Option<&SchemaEntry> {
        self.registries
            .get(registry_name)
            .and_then(|registry| registry.schemas.get(schema_name))
    }

    /// The stored schema set `schema_name` of registry `registry_name`, mutably.
    pub fn schema_mut(
        &mut self,
        registry_name: &str,
        schema_name: &str,
    ) -> Option<&mut SchemaEntry> {
        self.registries
            .get_mut(registry_name)
            .and_then(|registry| registry.schemas.get_mut(schema_name))
    }

    /// How many schema sets the whole store holds. Bounded by [`MAX_REGISTRIES`], so this
    /// walk is over at most that many entries.
    pub fn stored_schemas(&self) -> usize {
        self.registries
            .values()
            .map(|registry| registry.schemas.len())
            .sum()
    }

    /// Whether the store as a whole can hold another schema set, i.e. is below
    /// [`MAX_STORED_SCHEMAS`].
    pub fn can_store_schema(&self) -> bool {
        self.stored_schemas() < MAX_STORED_SCHEMAS
    }

    /// Whether the store can hold another custom entity type, i.e. is below
    /// [`MAX_CUSTOM_ENTITY_TYPES`].
    pub fn can_store_custom_entity_type(&self) -> bool {
        self.custom_entity_types.len() < MAX_CUSTOM_ENTITY_TYPES
    }

    /// Whether the store can hold another data quality ruleset, i.e. is below
    /// [`MAX_DATA_QUALITY_RULESETS`].
    pub fn can_store_data_quality_ruleset(&self) -> bool {
        self.data_quality_rulesets.len() < MAX_DATA_QUALITY_RULESETS
    }

    /// The integration resource property record stored for `resource_arn`, if this account
    /// holds one.
    ///
    /// The ONE resolution every read of such a record goes through, so an ARN names the
    /// same record whichever operation resolved it. The ARN is matched exactly as sent: the
    /// model documents no fold for `ResourceArn`.
    pub fn integration_resource_property(
        &self,
        resource_arn: &str,
    ) -> Option<&IntegrationResourcePropertyEntry> {
        self.integration_resource_properties.get(resource_arn)
    }

    /// Whether the store can hold another integration resource property record, i.e. is
    /// below [`MAX_INTEGRATION_RESOURCE_PROPERTIES`].
    pub fn can_store_integration_resource_property(&self) -> bool {
        self.integration_resource_properties.len() < MAX_INTEGRATION_RESOURCE_PROPERTIES
    }

    /// The integration table property record stored for `resource_arn` and `table_name`, if
    /// this account holds one for that pair.
    ///
    /// The pair is the record's whole address, so the lookup is the key lookup. One copy,
    /// shared, so the create and every read of one record cannot disagree about which
    /// record a request names.
    pub fn integration_table_properties(
        &self,
        resource_arn: &str,
        table_name: &str,
    ) -> Option<&IntegrationTablePropertiesEntry> {
        self.integration_table_properties
            .get(&(resource_arn.to_string(), table_name.to_string()))
    }

    /// Whether the store can hold another integration table property record, i.e. is below
    /// [`MAX_INTEGRATION_TABLE_PROPERTIES`].
    pub fn can_store_integration_table_properties(&self) -> bool {
        self.integration_table_properties.len() < MAX_INTEGRATION_TABLE_PROPERTIES
    }

    /// Whether this account holds ANY integration table property record for `resource_arn`,
    /// whatever table each one overrides.
    ///
    /// This is what tells the miss of ONE pair apart from a resource that carries no
    /// integration table properties at all, which the two operations that address a record
    /// report differently ([`Self::integration_table_properties_miss`]).
    pub fn holds_integration_table_properties_of(&self, resource_arn: &str) -> bool {
        self.integration_table_properties
            .keys()
            .any(|(arn, _)| arn == resource_arn)
    }

    /// The message an operation that addresses ONE integration table property record
    /// (`GetIntegrationTableProperties`, `DeleteIntegrationTableProperties`) carries when it
    /// reaches no record for the pair it was asked about.
    ///
    /// RECORDED behaviour: a resource carrying no integration table properties at all is
    /// reported as the resource itself not being found ([`messages::target_resource_not_found`]),
    /// while a resource that carries properties for other tables answers with the miss of the
    /// pair the request addressed ([`messages::integration_table_properties_not_found`]). The
    /// ONE reading of that rule, so the read and the delete cannot disagree about which miss
    /// a request earns.
    pub fn integration_table_properties_miss(&self, resource_arn: &str, table_name: &str) -> String {
        if self.holds_integration_table_properties_of(resource_arn) {
            messages::integration_table_properties_not_found(resource_arn, table_name)
        } else {
            messages::target_resource_not_found(resource_arn)
        }
    }

    /// Whether this store holds the resource an integration table property call's
    /// `ResourceArn` named ([`integration_table_reference`]).
    ///
    /// The properties belong to that resource, so it is the resource that has to exist —
    /// for a connection or a database this is the same question a tagging call asks
    /// ([`Self::resource_tags`]), and for a table ARN it is the table itself, inside the
    /// database its ARN names.
    pub fn holds_integration_table_resource(&self, resource: &IntegrationTableResource) -> bool {
        match resource {
            IntegrationTableResource::Tagged(tagged) => self.resource_tags(tagged).is_some(),
            IntegrationTableResource::Table {
                account_id,
                database,
                table,
            } => self
                .database(account_id, database)
                .is_some_and(|database| database.tables.contains_key(table)),
        }
    }

    /// Whether this store has room for another classifier, bounded by
    /// [`MAX_CLASSIFIERS`].
    pub fn can_store_classifier(&self) -> bool {
        self.classifiers.len() < MAX_CLASSIFIERS
    }

    /// Whether the store can hold another security configuration, i.e. is below
    /// [`MAX_SECURITY_CONFIGURATIONS`].
    pub fn can_store_security_configuration(&self) -> bool {
        self.security_configurations.len() < MAX_SECURITY_CONFIGURATIONS
    }

    /// Whether the store can hold another usage profile, i.e. is below
    /// [`MAX_USAGE_PROFILES`].
    pub fn can_store_usage_profile(&self) -> bool {
        self.usage_profiles.len() < MAX_USAGE_PROFILES
    }

    /// Whether the store can hold another machine learning transform, i.e. is below
    /// [`MAX_ML_TRANSFORMS`].
    pub fn can_store_ml_transform(&self) -> bool {
        self.ml_transforms.len() < MAX_ML_TRANSFORMS
    }

    /// The machine learning transform stored under `name`, if this account holds one.
    ///
    /// A `Name` is not a transform's ADDRESS — every operation that addresses one takes
    /// the minted `TransformId` — but it IS the name the client chooses at create time, so
    /// a create resolves it to refuse a name this account already holds a transform under.
    /// At most one transform is ever stored under one name, because such a create is
    /// refused. Bounded by [`MAX_ML_TRANSFORMS`].
    pub fn ml_transform_by_name(&self, name: &str) -> Option<&MlTransformEntry> {
        self.ml_transforms
            .values()
            .find(|transform| transform.name == name)
    }

    /// Every stored machine learning transform `filter` admits, in the order `sort` names,
    /// each paired with its ordering key — which is what a listing's continuation token
    /// carries ([`MlTransformEntry::ordering_key`]).
    ///
    /// One walk, shared by the two transform listings, so the listing that reports
    /// transforms and the one that reports their ids can never disagree about which
    /// transforms a filter admits or what order they come in. Bounded by
    /// [`MAX_ML_TRANSFORMS`].
    pub fn ml_transforms_in_order(
        &self,
        filter: Option<&TransformFilterCriteria>,
        sort: Option<&TransformSortCriteria>,
    ) -> Vec<(String, &MlTransformEntry)> {
        let column = sort.map(|criteria| criteria.column.as_str());
        let mut ordered: Vec<(String, &MlTransformEntry)> = self
            .ml_transforms
            .values()
            .filter(|transform| filter.is_none_or(|criteria| transform.matches(criteria)))
            .map(|transform| (transform.ordering_key(column), transform))
            .collect();
        ordered.sort_by(|left, right| left.0.cmp(&right.0));
        if sort.is_some_and(|criteria| criteria.sort_direction == SORT_DIRECTION_DESCENDING) {
            // The ordering key is unique per transform, so reversing the ascending order
            // is exactly the descending one.
            ordered.reverse();
        }
        ordered
    }

    /// The schema a machine learning transform over `tables` runs against: "the columns and
    /// data types that this transform can run against" (`MLTransform$Schema`).
    ///
    /// A create carries no schema, so the columns are read out of the CATALOG, from the
    /// input record tables the request addressed, in the order those tables name them. A
    /// column name already contributed by an earlier table is not repeated — the schema is
    /// a map from column to type — and a table this store does not hold contributes
    /// nothing, the way a run over an absent table recommends nothing rather than being
    /// refused. Capped at [`MAX_TRANSFORM_SCHEMA_COLUMNS`], which is the modelled length of
    /// the member.
    pub fn ml_transform_schema(&self, tables: &[GlueTable]) -> Vec<SchemaColumn> {
        let mut schema: Vec<SchemaColumn> = Vec::new();
        for table in tables {
            let catalog_id = resolve_catalog_id(table.catalog_id.as_ref());
            let Some(stored) = self
                .database(&catalog_id, &table.database_name)
                .and_then(|database| database.tables.get(&fold_name(&table.table_name)))
            else {
                continue;
            };
            for column in stored
                .storage_descriptor
                .as_ref()
                .and_then(|descriptor| descriptor.columns.as_ref())
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                if schema.len() >= MAX_TRANSFORM_SCHEMA_COLUMNS {
                    return schema;
                }
                if schema
                    .iter()
                    .any(|held| held.name.as_deref() == Some(column.name.as_str()))
                {
                    continue;
                }
                schema.push(SchemaColumn {
                    name: Some(column.name.clone()),
                    data_type: column.r#type.clone(),
                });
            }
        }
        schema
    }

    /// Whether the store can hold another form type, i.e. is below [`MAX_FORM_TYPES`].
    pub fn can_store_form_type(&self) -> bool {
        self.form_types.len() < MAX_FORM_TYPES
    }

    /// The stored form type an `Identifier` targeting `FormTypeId` names — a
    /// `GetFormType$Identifier`, a `DeleteFormType$Identifier` or an
    /// `AssetTypeFormReference$FormTypeIdentifier` — or `None` when this store holds no
    /// such form type.
    ///
    /// A form type's identifier is its name ([`FormTypeEntry::id`]), so the resolution is
    /// this map's own lookup, exact as sent. One copy, shared, so no two operations can
    /// disagree about which record an identifier names.
    pub fn form_type_by_reference(&self, identifier: &str) -> Option<&FormTypeEntry> {
        self.form_types.get(identifier)
    }

    /// Remove the form type an `Identifier` targeting `FormTypeId` names, reporting the
    /// record that was removed, or `None` when this store holds no such form type.
    ///
    /// The identifier resolves exactly as a read resolves one, i.e. as this namespace's
    /// own key ([`Self::form_type_by_reference`]), so a delete reaches the record a get of
    /// the same identifier would report. Only the form type itself is removed: an asset
    /// type's form references are written by that asset type's own puts and are left as
    /// they stand.
    pub fn remove_form_type_by_reference(&mut self, identifier: &str) -> Option<FormTypeEntry> {
        self.form_types.remove(identifier)
    }

    /// Whether the store can hold another asset type, i.e. is below [`MAX_ASSET_TYPES`].
    pub fn can_store_asset_type(&self) -> bool {
        self.asset_types.len() < MAX_ASSET_TYPES
    }

    /// The stored asset type whose `Id` is `id`, or `None` when this store holds no asset
    /// type under that id.
    ///
    /// Every operation that ADDRESSES an asset type does so by an `AssetTypeId`
    /// (`GetAssetType$Identifier`, `DeleteAssetType$Identifier` and `PutAsset$AssetTypeId`
    /// all target that shape), while the records are keyed by the name a put addresses
    /// them by — so the id resolution is a walk, bounded by [`MAX_ASSET_TYPES`]. One copy,
    /// shared, so no two operations can disagree about which record an id names.
    pub fn asset_type_by_id(&self, id: &str) -> Option<&AssetTypeEntry> {
        self.asset_types
            .values()
            .find(|asset_type| asset_type.id == id)
    }

    /// The stored asset type an `Identifier` targeting `AssetTypeId` names, or `None` when
    /// this store holds no such asset type.
    ///
    /// An identifier carries an asset type's address as the CLIENT holds it, which is any
    /// of the forms this surface hands one out in: the `AssetTypeName` it was registered
    /// under, which is also the `Id` the put reports ([`AssetTypeEntry::id`]), and the
    /// catalog-qualified form a READ of it reports ([`AssetTypeEntry::qualified_id`]). A
    /// client that kept the identifier from either answer addresses the record with it, and
    /// the catalog serves that read.
    pub fn asset_type_by_reference(&self, identifier: &str) -> Option<&AssetTypeEntry> {
        self.asset_type_by_id(identifier)
            .or_else(|| self.asset_types.get(identifier))
            .or_else(|| {
                asset_type_name_from_qualified_id(identifier)
                    .and_then(|name| self.asset_types.get(name))
            })
    }

    /// Remove the asset type an `Identifier` targeting `AssetTypeId` names, reporting the
    /// record that was removed, or `None` when this store holds no such asset type.
    ///
    /// The identifier resolves exactly as a read resolves one
    /// ([`Self::asset_type_by_reference`]), so a delete reaches the record a get of the
    /// same identifier would report. Only the asset type record is removed: the assets
    /// that are instances of it record the identifier their own puts sent and are left as
    /// they stand.
    pub fn remove_asset_type_by_reference(&mut self, identifier: &str) -> Option<AssetTypeEntry> {
        let name = self.asset_type_by_reference(identifier)?.name.clone();
        self.asset_types.remove(&name)
    }

    /// Whether the store can hold another asset, i.e. is below [`MAX_ASSETS`].
    pub fn can_store_asset(&self) -> bool {
        self.assets.len() < MAX_ASSETS
    }

    /// The stored asset an `AssetId` names, or `None` when this account holds no asset
    /// under that identifier.
    ///
    /// Every operation that addresses an asset does so by that identifier, which is this
    /// namespace's own key, so the resolution is this map's lookup, exact as sent. One copy,
    /// shared, so no two operations can disagree about which record an identifier names.
    pub fn asset(&self, identifier: &str) -> Option<&AssetEntry> {
        self.assets.get(identifier)
    }

    /// The stored asset an `AssetId` names, mutably.
    pub fn asset_mut(&mut self, identifier: &str) -> Option<&mut AssetEntry> {
        self.assets.get_mut(identifier)
    }

    /// Remove the asset an `AssetId` names, reporting the record that was removed, or
    /// `None` when this account holds no asset under that identifier.
    ///
    /// The identifier resolves exactly as a read resolves one ([`Self::asset`]) — it is
    /// this namespace's own key — so a delete reaches the record a get of the same
    /// identifier would report. The asset's own record is all that is removed: the asset
    /// type it was an instance of and the glossary terms its associations named are their
    /// own records in their own namespaces.
    pub fn remove_asset(&mut self, identifier: &str) -> Option<AssetEntry> {
        self.assets.remove(identifier)
    }

    /// Whether the store can hold another business glossary, i.e. is below
    /// [`MAX_GLOSSARIES`].
    pub fn can_store_glossary(&self) -> bool {
        self.glossaries.len() < MAX_GLOSSARIES
    }

    /// The stored glossary whose `Id` is `id`, or `None` when this account holds no
    /// glossary under that id.
    ///
    /// Every operation that addresses a glossary does so by a `GlossaryId`, which is this
    /// map's key, so the lookup is the key lookup. One copy, shared, so no two operations
    /// can disagree about which record an identifier names.
    pub fn glossary(&self, id: &str) -> Option<&GlossaryEntry> {
        self.glossaries.get(id)
    }

    /// The stored glossary whose `Id` is `id`, mutably.
    pub fn glossary_mut(&mut self, id: &str) -> Option<&mut GlossaryEntry> {
        self.glossaries.get_mut(id)
    }

    /// Remove the glossary whose `Id` is `id`, reporting the record removed and `None` when
    /// this account holds none under that id.
    ///
    /// The one place a glossary leaves the namespace, resolving the identifier the way
    /// [`Self::glossary`] does — the `GlossaryId` is the whole address — so the delete and
    /// the reads cannot disagree about which record an identifier names. A glossary's terms
    /// are their own records in their own namespace, so this removes none of them.
    pub fn remove_glossary(&mut self, id: &str) -> Option<GlossaryEntry> {
        self.glossaries.remove(id)
    }

    /// The stored glossary carrying this `Name`, or `None` when the name is free.
    ///
    /// A glossary's name is unique over this per-account namespace — that is what
    /// `CreateGlossary`'s modelled `AlreadyExistsException` refuses a second create under
    /// — and the records are keyed by id, so the resolution is a walk bounded by
    /// [`MAX_GLOSSARIES`]. The name is matched exactly as sent: the model documents no
    /// case fold for `GlossaryName`.
    pub fn glossary_by_name(&self, name: &str) -> Option<&GlossaryEntry> {
        self.glossaries
            .values()
            .find(|glossary| glossary.name == name)
    }

    /// Whether the store can hold another glossary term, i.e. is below
    /// [`MAX_GLOSSARY_TERMS`].
    pub fn can_store_glossary_term(&self) -> bool {
        self.glossary_terms.len() < MAX_GLOSSARY_TERMS
    }

    /// The stored glossary term whose `Id` is `id`, or `None` when this account holds no
    /// term under that id.
    ///
    /// Every operation that addresses a term does so by a `GlossaryTermId`, which is this
    /// map's key, so the lookup is the key lookup. One copy, shared, so no two operations
    /// can disagree about which record an identifier names.
    pub fn glossary_term(&self, id: &str) -> Option<&GlossaryTermEntry> {
        self.glossary_terms.get(id)
    }

    /// The stored glossary term whose `Id` is `id`, mutably.
    pub fn glossary_term_mut(&mut self, id: &str) -> Option<&mut GlossaryTermEntry> {
        self.glossary_terms.get_mut(id)
    }

    /// Remove the glossary term whose `Id` is `id`, reporting the record removed and
    /// `None` when this account holds none under that id.
    ///
    /// The one place a term leaves the namespace, resolving the identifier the way
    /// [`Self::glossary_term`] does — the `GlossaryTermId` is the whole address — so the
    /// delete and the reads cannot disagree about which record an identifier names. Only
    /// the term record is removed: an asset an association ALREADY put this id on keeps it
    /// ([`AssetEntry::glossary_terms`]), because the delete rewrites no other namespace's
    /// records. A NEW association naming the removed id is a different question and is
    /// refused, because an association resolves the ids it names against this namespace
    /// first ([`Self::first_unheld_glossary_term`]).
    pub fn remove_glossary_term(&mut self, id: &str) -> Option<GlossaryTermEntry> {
        self.glossary_terms.remove(id)
    }

    /// The first of `identifiers` this store holds no glossary term under, in the order the
    /// request named them, and `None` when every one of them resolves to a live term.
    ///
    /// This is how an operation that writes a REFERENCE to a glossary term resolves the
    /// references it was given before it writes any of them: each id is resolved the one way
    /// every operation addressing a term resolves one ([`Self::glossary_term`]), so a term
    /// that was never created and a term that has since been deleted are the same miss here.
    /// The FIRST miss is reported, in request order, so a refusal that names one id names
    /// the same id however many of the request's ids miss.
    pub fn first_unheld_glossary_term<'ids>(
        &self,
        identifiers: &'ids [String],
    ) -> Option<&'ids str> {
        identifiers
            .iter()
            .map(String::as_str)
            .find(|identifier| self.glossary_term(identifier).is_none())
    }

    /// The term carrying this `Name` within the glossary `glossary_id` names, or `None`
    /// when that name is free in that glossary.
    ///
    /// A term's name identifies it WITHIN its glossary — that is what
    /// `CreateGlossaryTerm`'s modelled `AlreadyExistsException` refuses a second create
    /// under — so the same name in another glossary is a different term. The records are
    /// keyed by id, so the resolution is a walk bounded by [`MAX_GLOSSARY_TERMS`]. The name
    /// is matched exactly as sent: the model documents no case fold for `GlossaryTermName`.
    pub fn glossary_term_by_name(
        &self,
        glossary_id: &str,
        name: &str,
    ) -> Option<&GlossaryTermEntry> {
        self.glossary_terms
            .values()
            .find(|term| term.glossary_id == glossary_id && term.name == name)
    }

    /// The tag set of the resource `resource` names, or `None` when this store holds no
    /// such resource.
    ///
    /// The ONE resolution every tag READ goes through, so an ARN names the same record
    /// whichever operation — this service's own `GetTags`, or the account-wide tagging
    /// API that projects this store — resolved it.
    pub fn resource_tags(&self, resource: &TaggedResource) -> Option<&HashMap<String, String>> {
        match resource.kind {
            // A database and a connection live INSIDE a catalog, so the ARN's account
            // segment is the `CatalogId` they are looked for under.
            TaggedResourceKind::Database => self
                .catalogs
                .get(&resource.account_id)
                .and_then(|catalog| catalog.databases.get(&resource.name))
                .map(|database| &database.tags),
            TaggedResourceKind::Connection => self
                .catalogs
                .get(&resource.account_id)
                .and_then(|catalog| catalog.connections.get(&resource.name))
                .map(|connection| &connection.tags),
            // A crawler and a job definition live BESIDE the catalog tree in one
            // per-account namespace, so the account segment is what decides whether this
            // emulator could hold them at all.
            TaggedResourceKind::Crawler => is_served_account(&resource.account_id)
                .then(|| self.crawlers.get(&resource.name).map(|crawler| &crawler.tags))
                .flatten(),
            TaggedResourceKind::Job => is_served_account(&resource.account_id)
                .then(|| {
                    self.job(&resource.region, &resource.name)
                        .map(|job| &job.tags)
                })
                .flatten(),
        }
    }

    /// [`Self::resource_tags`] mutably: the ONE record every tag WRITE lands in, so a tag
    /// written through this service's own `TagResource` and one written through the
    /// account-wide tagging API are the same tag.
    pub fn resource_tags_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut HashMap<String, String>> {
        match resource.kind {
            TaggedResourceKind::Database => self
                .catalogs
                .get_mut(&resource.account_id)
                .and_then(|catalog| catalog.databases.get_mut(&resource.name))
                .map(|database| &mut database.tags),
            TaggedResourceKind::Connection => self
                .catalogs
                .get_mut(&resource.account_id)
                .and_then(|catalog| catalog.connections.get_mut(&resource.name))
                .map(|connection| &mut connection.tags),
            TaggedResourceKind::Crawler => {
                if !is_served_account(&resource.account_id) {
                    return None;
                }
                self.crawlers
                    .get_mut(&resource.name)
                    .map(|crawler| &mut crawler.tags)
            }
            TaggedResourceKind::Job => {
                if !is_served_account(&resource.account_id) {
                    return None;
                }
                let region = resource.region.clone();
                let name = resource.name.clone();
                self.job_mut(&region, &name).map(|job| &mut job.tags)
            }
        }
    }

    /// Every resource of the account `account_id` this store holds a tag set for, as
    /// `(ARN, tags)`.
    ///
    /// This is ONE account's tag-store rows for this service: the tags a create recorded
    /// are the resource's own, so the account-wide tagging API projects THESE records
    /// rather than keeping a second copy that a create would have to be taught to write.
    /// Resources carrying NO tag are included — the caller decides what to do with an
    /// untagged resource — and `region` is the Region the ARNs are minted for, for the
    /// reason [`request_region`] gives.
    ///
    /// The account is the boundary the single-resource accessors enforce, so it bounds this
    /// projection too: a database and a connection are reached through the catalog of
    /// `account_id` alone, exactly as [`Self::resource_tags`] reaches them, and the crawlers
    /// and job definitions that live beside the catalog tree in one per-account namespace are
    /// projected only for an account this emulator serves ([`is_served_account`]), exactly as
    /// that accessor decides it. So no account's rows are ever reported to another's.
    pub fn tagged_resources(
        &self,
        account_id: &str,
        region: &str,
    ) -> Vec<(String, HashMap<String, String>)> {
        let mut found: Vec<(String, HashMap<String, String>)> = Vec::new();
        if let Some(catalog) = self.catalogs.get(account_id) {
            for database in catalog.databases.values() {
                found.push((
                    TaggedResourceKind::Database.arn(region, &database.name),
                    database.tags.clone(),
                ));
            }
            for connection in catalog.connections.values() {
                found.push((
                    TaggedResourceKind::Connection.arn(region, &connection.name),
                    connection.tags.clone(),
                ));
            }
        }
        if !is_served_account(account_id) {
            return found;
        }
        for crawler in self.crawlers.values() {
            found.push((
                TaggedResourceKind::Crawler.arn(region, &crawler.name),
                crawler.tags.clone(),
            ));
        }
        for job in self.jobs_in(region) {
            found.push((
                TaggedResourceKind::Job.arn(region, &job.name),
                job.tags.clone(),
            ));
        }
        found
    }
}

// The shapes a machine learning TASK RUN is made of, declared beside the records and the
// selection that use them rather than in the catalog's own import list above.
use crate::ops::types::{TaskRun, TaskRunFilterCriteria, TaskRunProperties, TaskRunSortCriteria};

// ---------------------------------------------------------------------------
// The schema registry
// ---------------------------------------------------------------------------

/// The text the service puts in an error body's `Message` member.
///
/// Error wording here is PER OPERATION AND PER RESOURCE, never one template this
/// emulator invents and reuses: the service answers a fixed constant where its message
/// names no resource, and otherwise a template carrying only the request member the
/// refusal is about — never the name of a container the caller also sent, and never
/// emulator-side context such as the resolved `CatalogId`.
///
/// Four of these are pinned by the recorded wire corpus: `GetCrawler`'s crawler miss
/// ("Crawler entry with name `<Name>` does not exist"), `DeleteTable`'s table miss
/// ("Table `<Name>` not found."), `GetTable`'s table miss (the constant
/// "Entity Not Found", which names neither table nor database) and `CreateDatabase`'s
/// duplicate (the constant "Database already exists.", which withholds the requested
/// name). A fifth and a sixth are the classifier's own refusals
/// ([`messages::classifier_not_found`], [`messages::classifier_already_exists`]), whose
/// recorded wordings state the miss and the collision TWICE ("Classifier `<Name>` not
/// found. not found.") because the service composes its per-resource sentence with its
/// generic one; they are enumerated here so the doubling reads as the requirement it is
/// rather than as a typo to tidy. The classifier DELETE words its own miss differently
/// again ([`messages::classifier_not_found_for_delete`]), which is why the read's
/// sentence and the delete's are two strings and not one.
/// The rest are the same per-resource wording for the same refusal on the
/// sibling operations. Every handler refuses through these constructors, so one
/// operation's wording is never silently borrowed for another's refusal.
///
/// This is the TOP-LEVEL error body only. A per-item failure a batch reports inside its
/// output (`BatchDeleteTable`'s `TableError$ErrorDetail`) has its own recorded wording
/// and is built by [`super::catalog_error_detail`]. Of the job and connection slices the
/// job-definition miss ([`messages::JOB_NOT_FOUND`]) and `StartJobRun`'s
/// missing-metadata refusal are here, because the corpus records both; the rest of their
/// messages are not requirements and are left to the issues that own them rather than
/// guessed at.
///
/// The schema-registry misses ARE here, because every operation that resolves a
/// `RegistryId` or a `SchemaId` resolves the same two references and so has to describe
/// the same two misses. The SCHEMA miss is RECORDED: it is the fixed constant
/// [`messages::SCHEMA_NOT_FOUND`], which names neither the schema the request addressed
/// nor the registry it was addressed in. The registry miss is not recorded: what is pinned
/// there is the behaviour — 400 with the operation's modelled `EntityNotFoundException` —
/// and its sentence follows the per-resource template the catalog slice's recorded misses
/// use.
pub mod messages;

