// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-athena-amazonathena service.
//!
//! Athena's workgroup and data-catalog registries live here. `CreateWorkGroup` writes a
//! [`WorkGroupRecord`] into [`ComAmazonawsAthenaAmazonathenaState::work_groups`] and
//! `GetWorkGroup` answers out of that same record, so every value a read reports is
//! one some earlier write persisted rather than one derived from the reading request.
//! `CreateDataCatalog` and `GetDataCatalog` stand in the same relation through
//! [`DataCatalogRecord`] and [`ComAmazonawsAthenaAmazonathenaState::data_catalogs`].
//!
//! A workgroup has no lifecycle the clock owes: Athena answers `CreateWorkGroup` with
//! the workgroup already usable, so the record is stored in its terminal state and a
//! read never has to materialise a pending transition. The one workgroup the registry
//! holds no record for is the account's default workgroup ([`DEFAULT_WORK_GROUP`]):
//! nothing created it, so [`default_work_group_record`] builds it from the instant the
//! registries were opened and `ListWorkGroups` reports it beside the created ones. A data
//! catalog has one only when
//! its creation is asynchronous (see [`DataCatalogRecord::status_at`]), and even then the
//! due instant is stored once and every read derives the status from it rather than
//! writing.
//!
//! Athena's TABLE METADATA is not kept here at all: Athena registers no tables of its
//! own, and the data catalog its metadata reads address — [`GLUE_DATA_CATALOG_NAME`], or
//! a registered `GLUE` catalog naming the same catalog id — IS the account's Glue Data
//! Catalog. So those reads project what GLUE persisted ([`to_table_metadata`]) instead of
//! holding a second copy that could drift from the catalog a client writes through.
//!
//! QUERY EXECUTIONS are the service's other resource, and the one with a lifecycle:
//! `StartQueryExecution` RUNS the submitted statement — against the Data Catalog, or on a
//! real SQL engine over the rows the catalog's tables hold ([`catalog_sql`],
//! [`query_engine`]) — and stores what it produced in a [`QueryExecutionRecord`], and
//! every later read — `GetQueryExecution`, `GetQueryResults`, `ListQueryExecutions`,
//! `BatchGetQueryExecution` — answers out of that record. The record's `Status.State`
//! is DERIVED from the clock and the stored outcome
//! ([`QueryExecutionRecord::state_at`]), so a query a caller reads straight back is
//! still QUEUED, is RUNNING a moment later and is terminal from its completion instant
//! on, and no read ever has to write to move it.
//!
//! NOTEBOOKS are kept in the [`NotebookRegistry`] of
//! [`ComAmazonawsAthenaAmazonathenaState::notebooks`]: `ImportNotebook` stores the ipynb
//! content it was given (or read out of the one object store, for a request that named an
//! Amazon S3 location) and `ExportNotebook` answers with that same content, so an export
//! reports what an import persisted rather than anything rebuilt from the reading request.
//!
//! PREPARED STATEMENTS live in the [`PreparedStatementRegistry`] of
//! [`ComAmazonawsAthenaAmazonathenaState::prepared_statements`], scoped to the workgroup
//! each belongs to: `CreatePreparedStatement` writes a [`PreparedStatementRecord`] there
//! and `GetPreparedStatement`, `BatchGetPreparedStatement` and an update or a delete all
//! address that same record, so a read reports what a write persisted. A statement has no
//! lifecycle the clock owes — a create hands back one a query can already name.
//!
//! NAMED QUERIES live in the [`NamedQueryRegistry`] of
//! [`ComAmazonawsAthenaAmazonathenaState::named_queries`], each belonging to the workgroup
//! its create named: `CreateNamedQuery` writes a [`NamedQueryRecord`] there and
//! `GetNamedQuery` answers out of that same record. A named query has no lifecycle the
//! clock owes either, and it lives exactly as long as its workgroup does — deleting the
//! workgroup takes its contents with it
//! ([`ComAmazonawsAthenaAmazonathenaState::delete_work_group`]), and a workgroup that still
//! holds contents is not deleted by a request that did not ask for them ([`WorkGroupContents`]).
//!
//! CAPACITY RESERVATIONS live in the [`CapacityReservationRegistry`] of
//! [`ComAmazonawsAthenaAmazonathenaState::capacity_reservations`], and they have a
//! lifecycle the clock owes as well: a create stores when the requested data processing
//! units are due to be allocated and every read derives the reservation's status and
//! allocated amount from that instant ([`CapacityReservationRecord::status_at`]), so a
//! caller reading one straight back still sees it PENDING and the reservation reaches
//! ACTIVE whether or not anybody polls it.

/// The SQL engine a query execution runs on, and the result set it produces.
pub mod query_engine;

/// What a submitted statement means to the AWS Glue Data Catalog, and how it is run.
pub mod catalog_sql;

/// The objects a completed run saves its results to, under the resolved output location.
pub mod query_results;

/// The notebooks a Spark enabled workgroup holds, and the ipynb content of each.
mod notebooks;
pub use notebooks::*;

/// The provisioned capacity this account reserved, and each reservation's lifecycle.
mod capacity_reservations;
pub use capacity_reservations::*;

/// Whether IAM lets a request perform the action it asks for on the workgroup it names.
pub mod authorization;

/// Which Athena resource an ARN names, and the tags that resource holds.
mod resource_tags;
pub use resource_tags::*;

/// The prepared statements each workgroup holds, and the query string of each.
mod prepared_statements;
pub use prepared_statements::*;

/// The named (saved) queries each workgroup holds, and the statements of each.
mod named_queries;
pub use named_queries::*;

/// What a workgroup holds, and what deleting the workgroup removes with it.
mod work_group_deletion;
pub use work_group_deletion::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use ::service_common::page_tokens::PageTokenKey;
use ::service_com_amazonaws_glue_awsglue::ops::types::Column as GlueColumn;
use ::service_com_amazonaws_glue_awsglue::state as glue_state;

use crate::ops::types::{
    Column, DataCatalog, EngineVersion, ManagedQueryResultsConfiguration, QueryExecution,
    QueryExecutionContext, QueryExecutionStatistics, QueryExecutionStatus, ResultConfiguration,
    ResultReuseConfiguration, ResultReuseInformation, TableMetadata, Tag, WorkGroup,
    WorkGroupConfiguration, WorkGroupSummary, CONNECTION_TYPE_VALUES,
};

/// The `WorkGroupState` a freshly created workgroup is in. One of
/// `WORK_GROUP_STATE_VALUES` in [`crate::ops::types`] (the model's `WorkGroupState`
/// enum, whose `WorkGroup$State` documentation reads "The state of the workgroup:
/// ENABLED or DISABLED").
pub const WORK_GROUP_STATE_ENABLED: &str = "ENABLED";
/// The other `WorkGroupState`; see [`WORK_GROUP_STATE_ENABLED`].
pub const WORK_GROUP_STATE_DISABLED: &str = "DISABLED";

/// The engine version that asks Athena to choose one.
///
/// `EngineVersion$SelectedEngineVersion` documents: "Possible values are determined by
/// the output of `ListEngineVersions`, including AUTO. The default is AUTO."
pub const ENGINE_VERSION_AUTO: &str = "AUTO";

/// The engine version this emulator resolves [`ENGINE_VERSION_AUTO`] to.
///
/// `EngineVersion$EffectiveEngineVersion` documents: "If the user requests Auto, the
/// effective engine version is chosen by Athena." Athena's own documentation names
/// Athena engine version 3 as the version new workgroups run on, so that is what this
/// emulator chooses. It is NOT read from the request: a caller that asks for AUTO gets
/// this version whatever else the request says.
pub const DEFAULT_EFFECTIVE_ENGINE_VERSION: &str = "Athena engine version 3";

/// How many tags one Athena resource carries.
///
/// Athena's own documentation states the limit: "You can have up to 50 tags per
/// resource." The model's `TagList` carries no length trait, so the limit is enforced
/// here, and a longer list is refused BEFORE anything walks or stores it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The largest caller-supplied open collection a stored workgroup configuration may
/// hold, in bytes of keys and values (see [`open_configuration_bytes`]).
///
/// This is the EMULATOR's own bound and has NO AWS counterpart: Athena publishes no
/// limit on these members, so the number is chosen generously — far above any plausible
/// Spark property set, engine classification list or CloudWatch log-type map — and
/// exists only so that [`MAX_WORK_GROUPS`] times the size of one record is a bounded
/// product rather than one bounded factor times an unbounded one. Most
/// `WorkGroupConfiguration` members are already bounded by the model (a 1024-byte
/// description, a 128-byte `AdditionalConfiguration`, a 2048-byte `ExecutionRole`); its
/// Spark properties, engine classifications and CloudWatch log types are the open maps
/// and lists the model bounds in neither length nor element count. A configuration past
/// this bound is refused with the operation's own `InvalidRequestException`.
pub const MAX_WORK_GROUP_CONFIGURATION_BYTES: usize = 64 * 1024;

/// How many workgroups one account holds in this emulator.
///
/// Athena's published service quota is 1000 workgroups per Region per account; the
/// quota is not carried in the model, so this bound is enforced here and a write past
/// it is refused with the operation's own `InvalidRequestException` rather than
/// growing the emulator's heap without limit.
pub const MAX_WORK_GROUPS: usize = 1000;

/// One workgroup, as this service persists it.
///
/// The configuration is held already normalised (see
/// [`resolved_engine_version`]), so the create path decides the engine version once
/// and every later read reports that same decision.
#[derive(Debug, Clone)]
pub struct WorkGroupRecord {
    /// The workgroup name, which is also its key in the registry.
    pub name: String,
    /// A `WorkGroupState` value: [`WORK_GROUP_STATE_ENABLED`] or
    /// [`WORK_GROUP_STATE_DISABLED`].
    pub state: String,
    /// The description the create call carried, if any.
    pub description: Option<String>,
    /// The workgroup configuration, with the engine version resolved.
    pub configuration: WorkGroupConfiguration,
    /// When the workgroup was created, reported as `WorkGroup$CreationTime`.
    pub created_at: SystemTime,
    /// The tags the create call carried; keys are unique within this list.
    pub tags: Vec<Tag>,
}

impl WorkGroupRecord {
    /// The record as the modelled `WorkGroupSummary` a listing reports it by.
    ///
    /// `WorkGroupSummary` is documented "The summary information for the workgroup, which
    /// includes its name, state, description, and the date and time it was created", so
    /// every member it carries is projected from this same record — the listing reports no
    /// value the detailed read ([`Self::to_work_group`]) would not.
    pub fn to_summary(&self) -> WorkGroupSummary {
        WorkGroupSummary {
            name: Some(self.name.clone()),
            state: Some(self.state.clone()),
            description: self.description.clone(),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            engine_version: self.configuration.engine_version.clone(),
            // Filled only for an IAM Identity Center enabled workgroup, whose Identity
            // Center application this emulator does not create.
            identity_center_application_arn: None,
        }
    }

    /// The record as the modelled `WorkGroup` shape a read answers with.
    pub fn to_work_group(&self) -> WorkGroup {
        WorkGroup {
            name: self.name.clone(),
            state: Some(self.state.clone()),
            configuration: Some(self.configuration.clone()),
            description: self.description.clone(),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // Athena fills this only for an IAM Identity Center enabled workgroup,
            // whose Identity Center application this emulator does not create.
            identity_center_application_arn: None,
        }
    }
}

/// The account's default workgroup, as a record a listing can report.
///
/// Athena gives every account one workgroup nobody created: the one
/// [`DEFAULT_WORK_GROUP`] names, which a query that names no workgroup runs in and which
/// `DeleteWorkGroup` documents "cannot be deleted". It is therefore not in
/// [`ComAmazonawsAthenaAmazonathenaState::work_groups`] \u2014 no `CreateWorkGroup` call ever
/// wrote it \u2014 and it is built here instead, from `created_at`: the instant this account's
/// registries were opened, held once on the store so every read reports the same one.
///
/// Enabled (it is the workgroup queries run in by default), with no description of its
/// own and the engine version Athena resolves for a workgroup that selected none.
pub fn default_work_group_record(created_at: SystemTime) -> WorkGroupRecord {
    WorkGroupRecord {
        name: DEFAULT_WORK_GROUP.to_string(),
        state: WORK_GROUP_STATE_ENABLED.to_string(),
        description: None,
        configuration: WorkGroupConfiguration {
            engine_version: Some(resolved_engine_version(None)),
            ..WorkGroupConfiguration::default()
        },
        created_at,
        tags: Vec::new(),
    }
}

/// The engine version a workgroup runs on, given the one the request selected.
///
/// `EffectiveEngineVersion` is documented "Read only" and "When a request to update the
/// engine version is made by a `CreateWorkGroup` or `UpdateWorkGroup` operation, the
/// `EffectiveEngineVersion` field is ignored" — so whatever the caller put there is
/// dropped, and the effective version is the selected one, or
/// [`DEFAULT_EFFECTIVE_ENGINE_VERSION`] when the selection is
/// [`ENGINE_VERSION_AUTO`] (or absent, AUTO being the documented default).
pub fn resolved_engine_version(requested: Option<&EngineVersion>) -> EngineVersion {
    let selected = requested
        .and_then(|version| version.selected_engine_version.clone())
        .unwrap_or_else(|| ENGINE_VERSION_AUTO.to_string());
    let effective = if selected == ENGINE_VERSION_AUTO {
        DEFAULT_EFFECTIVE_ENGINE_VERSION.to_string()
    } else {
        selected.clone()
    };
    EngineVersion {
        selected_engine_version: Some(selected),
        effective_engine_version: Some(effective),
    }
}

/// Whether any tag in `tags` carries no key.
///
/// `Tag$Key` is 1 to 128 characters, so a present key is already checked at the
/// boundary; the member itself is optional in the model, and a tag with no key at all
/// names nothing to tag. Refused on its own terms, before keys are compared for
/// uniqueness.
pub fn tag_without_key(tags: &[Tag]) -> bool {
    tags.iter()
        .any(|tag| tag.key.as_deref().unwrap_or_default().is_empty())
}

/// The first tag key that appears more than once in `tags`, if any.
///
/// `Tag$Key` documents "Tag keys are case-sensitive and must be unique per resource",
/// so a tag list carrying one key twice describes no resource Athena would accept.
/// Linear in the list length: the keys already seen are held in a set, never rescanned.
pub fn first_duplicate_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(tags.len());
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if !seen.insert(key) {
            return Some(key);
        }
    }
    None
}

/// Why `tags` is not a tag list one Athena resource can carry, if it is not.
///
/// The rules `Tag` carries — the documented 50 tags per resource
/// ([`MAX_TAGS_PER_RESOURCE`]), a key on every tag ([`tag_without_key`]), no key inside the
/// namespace Amazon Web Services reserves ([`is_reserved_tag_key`]), only the characters a
/// key and a value may use ([`is_taggable_text`]) and keys unique within the list
/// ([`first_duplicate_tag_key`]) — stated once, so every operation that takes a `TagList`
/// refuses the same list with the same sentence and the wording of one create cannot drift
/// from another's.
pub fn tag_list_error(tags: &[Tag]) -> Option<String> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "Tags carries {} tags, more than the {MAX_TAGS_PER_RESOURCE} tags one \
             resource can hold.",
            tags.len()
        ));
    }
    // `Tag$Key` is 1 to 128 characters and a tag with no key names nothing to tag.
    if tag_without_key(tags) {
        return Some(
            "Tags contains a tag with no key; every tag must carry a Tag.Key of 1 to 128 \
             characters."
                .to_string(),
        );
    }
    // Athena's tag restrictions: the `aws:` namespace holds the tags Amazon Web Services
    // generates itself, which a caller may neither write nor remove.
    if tags
        .iter()
        .filter_map(|tag| tag.key.as_deref())
        .any(is_reserved_tag_key)
    {
        return Some(reserved_tag_key_message());
    }
    // `Tag$Key` and `Tag$Value` both state which characters a tag may use.
    for tag in tags {
        for (member, text) in [
            ("Tag key", tag.key.as_deref().unwrap_or_default()),
            ("Tag value", tag.value.as_deref().unwrap_or_default()),
        ] {
            if !is_taggable_text(text) {
                return Some(tag_text_message(member, text));
            }
        }
    }
    // `Tag$Key`: "Tag keys are case-sensitive and must be unique per resource."
    if let Some(key) = first_duplicate_tag_key(tags) {
        return Some(format!(
            "Tag keys must be unique per resource, and the key '{key}' appears more than \
             once in Tags."
        ));
    }
    None
}

/// The bytes of keys and values held in `configuration`'s OPEN collections — the ones
/// the model bounds neither in length nor in element count: the engine configuration's
/// additional configs, Spark properties and classifications, and the CloudWatch logging
/// configuration's log types. Checked against
/// [`MAX_WORK_GROUP_CONFIGURATION_BYTES`] before a configuration is stored.
///
/// Linear in the size of the request it measures, and it allocates nothing.
pub fn open_configuration_bytes(configuration: &WorkGroupConfiguration) -> usize {
    let mut bytes = 0usize;
    if let Some(engine) = configuration.engine_configuration.as_ref() {
        for map in [
            engine.additional_configs.as_ref(),
            engine.spark_properties.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            for (key, value) in map {
                bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
            }
        }
        for classification in engine.classifications.iter().flatten() {
            bytes = bytes
                .saturating_add(classification.name.as_ref().map_or(0, String::len));
            for (key, value) in classification.properties.iter().flatten() {
                bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
            }
        }
    }
    let log_types = configuration
        .monitoring_configuration
        .as_ref()
        .and_then(|monitoring| monitoring.cloud_watch_logging_configuration.as_ref())
        .and_then(|logging| logging.log_types.as_ref());
    for (key, values) in log_types.into_iter().flatten() {
        bytes = bytes.saturating_add(key.len());
        for value in values {
            bytes = bytes.saturating_add(value.len());
        }
    }
    bytes
}

/// The name Athena addresses the account's Glue Data Catalog by.
///
/// Athena's own documentation names it `AwsDataCatalog`, and a data catalog a client
/// registers of its own cannot take that name (`CreateDataCatalog`'s `Name` leaves "the
/// remainder of the length constraint of 256 ... reserved for use by Athena"), so this
/// one name always means the Glue Data Catalog.
pub const GLUE_DATA_CATALOG_NAME: &str = "AwsDataCatalog";

/// Whether `catalog_name` names the Glue Data Catalog ([`GLUE_DATA_CATALOG_NAME`]).
///
/// Matched without regard to ASCII case: Athena addresses this same catalog as
/// `awsdatacatalog` from its query layer, where every identifier is lowercased, so a
/// client that spells it that way addresses the catalog it means.
pub fn is_glue_data_catalog(catalog_name: &str) -> bool {
    catalog_name.eq_ignore_ascii_case(GLUE_DATA_CATALOG_NAME)
}

/// The `CatalogId` the Glue Data Catalog of THIS account is held under in Glue's store:
/// Glue's own default, which is the account id ("If none is provided, the Amazon Web
/// Services account ID is used by default"). Athena's `AwsDataCatalog` is that catalog,
/// so a metadata read resolves it through Glue's own rule rather than restating it.
pub fn glue_data_catalog_id() -> String {
    glue_state::resolve_catalog_id(None)
}

/// The refusal message for an operation whose named data catalog this account does not
/// hold.
///
/// The template is the one every Athena resource miss follows — the kind of entity, the
/// name the request addressed, and "was not found" — which is also how an execution miss
/// is worded ([`query_execution_not_found`]). ONE sentence for the class, so every
/// operation that answers a catalog miss words it identically.
pub fn data_catalog_not_found(catalog_name: &str) -> String {
    format!("DataCatalog {catalog_name} was not found.")
}

/// The refusal message a DELETE whose named data catalog this account does not hold
/// answers with.
///
/// The same miss as [`data_catalog_not_found`], worded as the DELETE path words it:
/// "DataCatalog " + the requested `Name` + " was not found", with no closing period.
/// Athena punctuates the two paths differently — the recorded behaviour of a read of an
/// absent catalog closes the sentence and that of a delete of one does not — so the two
/// sentences are separate helpers and neither path can drift onto the other's wording.
pub fn data_catalog_not_found_on_delete(catalog_name: &str) -> String {
    format!("DataCatalog {catalog_name} was not found")
}

/// The refusal message for a delete that asks for the Athena catalog alone to be removed
/// from a catalog whose type does not take that request.
///
/// `DeleteDataCatalogInput$DeleteCatalogOnly` documents "You can only use this with the
/// `FEDERATED` catalogs", because it is only a federated catalog that Athena creates a
/// Glue connection and a Lambda function alongside — for every other type there is no
/// second half of the resource to leave behind. That restriction is the only source for
/// this refusal: no recording exercises the member against a catalog of another type, so
/// the sentence is this service's own, built the way its other refusals are — naming the
/// member the request set, the catalog it addressed and the type that registry record
/// holds.
pub fn delete_catalog_only_unsupported(catalog_name: &str, catalog_type: &str) -> String {
    format!(
        "DeleteCatalogOnly can only be used with {DATA_CATALOG_TYPE_FEDERATED} data \
         catalogs, and DataCatalog {catalog_name} is a {catalog_type} data catalog."
    )
}

/// The refusal message for a metadata read whose `DatabaseName` names no database the
/// addressed data catalog holds.
///
/// The miss is the METASTORE's answer, which is why it is reported as `MetadataException`
/// rather than as a bad request — and the sentence is therefore the metastore's own, taken
/// from the Data Catalog this read resolved against rather than composed here, so a
/// database miss reads the same whether a client asked Athena or asked the catalog.
pub fn database_not_found(database_name: &str) -> String {
    glue_state::messages::database_not_found(database_name)
}

/// The refusal message for a metadata read whose `TableName` names no table the
/// addressed database holds. The per-kind counterpart of [`database_not_found`]: the same
/// metastore answers it, so the sentence is that metastore's own wording for a table its
/// store does not hold, which names no entity of the request.
pub fn table_not_found() -> &'static str {
    glue_state::messages::TABLE_ENTITY_NOT_FOUND
}

/// The refusal message for a listing whose `Expression` is no pattern the catalog can
/// match: a value that is not a valid regular expression, or one whose compiled matcher
/// would be larger than the catalog compiles
/// ([`glue_state::MAX_PATTERN_PROGRAM_BYTES`]). Beside the other refusal sentences of
/// this slice, so a reader of the pattern language is told the same thing wherever it is
/// matched.
pub fn malformed_pattern(pattern: &str, reason: &impl ::std::fmt::Display) -> String {
    format!("Expression {pattern:?} is not a valid regular expression: {reason}")
}

/// One stored Glue table as the `TableMetadata` Athena reports for it.
///
/// Every member is the catalog's, never the reading request's: the name, the instant the
/// catalog recorded the create, the last access time the catalog holds, the table type,
/// the table's columns (its storage descriptor's), its partition keys and its table
/// properties ([`hive_table_properties`]). A member the catalog does not determine for
/// that table stays absent, so a client reads "unknown" only where the catalog knows
/// nothing — a table stored with no storage descriptor has no column list to report,
/// which is not the same as a table whose column list is empty.
///
/// `Parameters` is the ONE member always reported: a Hive table's property set is
/// something the catalog always determines for it — every stored member of the table's
/// storage descriptor contributes one, and a table that names none simply has an empty
/// set — so there is no "unknown" state for it to stand in for.
pub fn to_table_metadata(table: &glue_state::TableEntry) -> TableMetadata {
    TableMetadata {
        name: table.name.clone(),
        create_time: Some(::aws_smithy_types::DateTime::from(table.create_time)),
        last_access_time: table.last_access_time,
        table_type: table.table_type.clone(),
        columns: table
            .storage_descriptor
            .as_ref()
            .and_then(|storage| storage.columns.as_ref())
            .map(|columns| columns.iter().map(to_column).collect()),
        partition_keys: table
            .partition_keys
            .as_ref()
            .map(|keys| keys.iter().map(to_column).collect()),
        parameters: Some(hive_table_properties(table)),
    }
}

/// The Hive table property that carries where a table's data lives: the stored
/// `StorageDescriptor$Location`.
const LOCATION_PROPERTY: &str = "location";

/// The Hive table property carrying `StorageDescriptor$InputFormat`.
const INPUT_FORMAT_PROPERTY: &str = "inputformat";

/// The Hive table property carrying `StorageDescriptor$OutputFormat`.
const OUTPUT_FORMAT_PROPERTY: &str = "outputformat";

/// The Hive table property carrying the serialization library of the stored
/// `SerDeInfo`.
const SERDE_LIBRARY_PROPERTY: &str = "serde.serialization.lib";

/// The prefix each `SerDeInfo$Parameters` entry is carried under, one Hive table
/// property per entry: `serde.param.<name>`.
const SERDE_PARAMETER_PREFIX: &str = "serde.param.";

/// A stored Data Catalog table's properties, as the `TableMetadata$Parameters`
/// ("a set of custom key/value pairs for table properties") Athena reports for it.
///
/// Athena presents a Data Catalog table as the HIVE table it is, so this is one flat
/// property set rather than the structured definition the catalog holds: the table's own
/// `Parameters`, plus its STORAGE DESCRIPTOR rendered as the Hive properties that
/// describe it — where the data lives, the input and output formats, the serialization
/// library and each of the SerDe's own parameters under
/// [`SERDE_PARAMETER_PREFIX`]. Athena's `TableMetadata` models no storage descriptor of
/// its own, and these properties are how a client reads one back.
///
/// A member the catalog does not hold contributes no property, so the set says exactly
/// what the stored definition carries. A property derived from the storage descriptor
/// wins over a table parameter of the same name: it is what the catalog actually holds
/// for that aspect of the table.
pub fn hive_table_properties(
    table: &glue_state::TableEntry,
) -> ::std::collections::HashMap<String, String> {
    let mut properties = table.parameters.clone().unwrap_or_default();
    let Some(storage) = table.storage_descriptor.as_ref() else {
        return properties;
    };
    for (property, value) in [
        (LOCATION_PROPERTY, storage.location.as_ref()),
        (INPUT_FORMAT_PROPERTY, storage.input_format.as_ref()),
        (OUTPUT_FORMAT_PROPERTY, storage.output_format.as_ref()),
    ] {
        if let Some(value) = value {
            properties.insert(property.to_string(), value.clone());
        }
    }
    let Some(serde) = storage.serde_info.as_ref() else {
        return properties;
    };
    if let Some(library) = serde.serialization_library.as_ref() {
        properties.insert(SERDE_LIBRARY_PROPERTY.to_string(), library.clone());
    }
    for (name, value) in serde.parameters.iter().flatten() {
        properties.insert(format!("{SERDE_PARAMETER_PREFIX}{name}"), value.clone());
    }
    properties
}

/// One column of a stored Glue table as Athena's own `Column` shape.
///
/// Athena's column carries the three members it models — the name, the data type and the
/// comment — and the Data Catalog's per-column `Parameters`, which Athena's shape does
/// not model, are left behind rather than folded into another member.
fn to_column(column: &GlueColumn) -> Column {
    Column {
        name: column.name.clone(),
        r#type: column.r#type.clone(),
        comment: column.comment.clone(),
    }
}

/// Where the metadata of the data catalog a read names comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataSource {
    /// A Glue Data Catalog this emulator holds, under this `CatalogId` — the account's
    /// own, which a registered `GLUE` catalog may name a second time.
    GlueDataCatalog(String),
    /// A registered catalog whose metadata an external metastore or a connector function
    /// serves, carrying that catalog's `DataCatalogType`.
    ExternalMetastore(String),
    /// A registered `GLUE` catalog naming the Data Catalog of ANOTHER account, carrying
    /// that `CatalogId`. The obstacle is the catalog it names rather than an external
    /// metastore: this emulator holds the Data Catalog of its own account only.
    ForeignGlueCatalog(String),
    /// No data catalog of that name: neither the Glue Data Catalog nor one this account
    /// registered.
    Unregistered,
}

/// Which metadata source `catalog_name` addresses, given the `catalogs` this account
/// registered.
///
/// [`GLUE_DATA_CATALOG_NAME`] is the account's Glue Data Catalog. A catalog a client
/// registered is resolved from its OWN stored record: a `GLUE` catalog reads the Glue Data
/// Catalog the `catalog-id` parameter its type documents names ([`PARAMETER_CATALOG_ID`])
/// — so one naming the catalog this emulator holds is that same catalog under a second
/// Athena name, while one naming another account's names a Data Catalog this emulator
/// does not hold ([`MetadataSource::ForeignGlueCatalog`]); a `HIVE`, `LAMBDA` or
/// `FEDERATED` catalog reads through an external Hive metastore or a connector function,
/// which this emulator does not host.
pub fn metadata_source(
    catalogs: &BTreeMap<String, DataCatalogRecord>,
    catalog_name: &str,
) -> MetadataSource {
    let glue_catalog_id = glue_data_catalog_id();
    if is_glue_data_catalog(catalog_name) {
        return MetadataSource::GlueDataCatalog(glue_catalog_id);
    }
    let Some(record) = catalogs.get(catalog_name) else {
        return MetadataSource::Unregistered;
    };
    if record.catalog_type != DATA_CATALOG_TYPE_GLUE {
        return MetadataSource::ExternalMetastore(record.catalog_type.clone());
    }
    match record.parameters.get(PARAMETER_CATALOG_ID) {
        Some(catalog_id) if *catalog_id == glue_catalog_id => {
            MetadataSource::GlueDataCatalog(glue_catalog_id)
        }
        // A GLUE catalog carries that parameter (its type requires it), and one naming
        // any other id names a Data Catalog of another account.
        Some(catalog_id) => MetadataSource::ForeignGlueCatalog(catalog_id.clone()),
        None => MetadataSource::ExternalMetastore(record.catalog_type.clone()),
    }
}

/// The refusal message for a metadata read of a registered catalog whose metastore this
/// emulator does not host.
///
/// Athena reads such a catalog by calling that catalog's own metastore or connector
/// function, so a metastore that answers nothing is what `MetadataException` reports; the
/// sentence names the catalog the request addressed and the type that decides where its
/// metadata lives.
pub fn metastore_unreachable(catalog_name: &str, catalog_type: &str) -> String {
    format!(
        "Catalog {catalog_name} is a {catalog_type} data catalog, whose metadata is served \
         by a metastore this emulator does not host."
    )
}

/// The refusal message for a metadata read of a registered `GLUE` catalog that names the
/// Data Catalog of another account.
///
/// The counterpart of [`metastore_unreachable`] for the other obstacle: the metastore is
/// the Glue Data Catalog, and the catalog this one names is one this emulator does not
/// hold, so the sentence names the catalog the request addressed and the `CatalogId` it
/// points at.
pub fn foreign_glue_catalog(catalog_name: &str, catalog_id: &str) -> String {
    format!(
        "Catalog {catalog_name} names the Data Catalog {catalog_id}, which is not a Data \
         Catalog this account holds."
    )
}

/// The `DataCatalogType` a federated catalog carries — the one type whose creation the
/// `DataCatalog$Status` documentation calls asynchronous. One of
/// `DATA_CATALOG_TYPE_VALUES` in [`crate::ops::types`].
pub const DATA_CATALOG_TYPE_FEDERATED: &str = "FEDERATED";
/// The `DataCatalogType` of an Glue Data Catalog; see [`DATA_CATALOG_TYPE_FEDERATED`].
pub const DATA_CATALOG_TYPE_GLUE: &str = "GLUE";
/// The `DataCatalogType` of an external Apache Hive metastore; see
/// [`DATA_CATALOG_TYPE_FEDERATED`].
pub const DATA_CATALOG_TYPE_HIVE: &str = "HIVE";
/// The `DataCatalogType` of a Lambda-backed catalog; see
/// [`DATA_CATALOG_TYPE_FEDERATED`].
pub const DATA_CATALOG_TYPE_LAMBDA: &str = "LAMBDA";

/// The `DataCatalogStatus` of a catalog whose creation is still running.
///
/// `DataCatalog$Status` documents "`CREATE_IN_PROGRESS`: Federated data catalog creation
/// in progress." One of `DATA_CATALOG_STATUS_VALUES` in [`crate::ops::types`].
pub const DATA_CATALOG_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// The `DataCatalogStatus` of a created catalog: `DataCatalog$Status` documents
/// "`CREATE_COMPLETE`: Data catalog creation complete."
pub const DATA_CATALOG_STATUS_CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `DataCatalogStatus` of a catalog whose deletion is done: `DataCatalog$Status`
/// documents "`DELETE_COMPLETE`: Federated data catalog deleted." and is "the status of
/// the creation or deletion of the data catalog", so it is the status a delete reports for
/// the catalog it removed.
pub const DATA_CATALOG_STATUS_DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// How long a federated catalog stays [`DATA_CATALOG_STATUS_CREATE_IN_PROGRESS`] before
/// it is [`DATA_CATALOG_STATUS_CREATE_COMPLETE`].
///
/// `DataCatalog$Status` documents that the `FEDERATED` type "is created asynchronously",
/// so the catalog is handed back in a non-terminal status and the completion is a
/// transition the CLOCK owes. AWS publishes no duration for it (it builds a stack, a
/// Lambda function and a connection), so this span is the emulator's own choice: short
/// enough that a caller is not kept waiting, long enough that a caller reading straight
/// back still observes the in-progress status.
pub const FEDERATED_DATA_CATALOG_CREATION: Duration = Duration::from_millis(500);

/// How many characters a data catalog name may use.
///
/// `CreateDataCatalogInput$Name` documents: "The catalog name must be unique for the
/// Amazon Web Services account and can use a maximum of 127 alphanumeric, underscore, at
/// sign, or hyphen characters. The remainder of the length constraint of 256 is reserved
/// for use by Athena." The model bounds `CatalogNameString` at 256, so this tighter
/// documented bound is enforced here.
pub const MAX_DATA_CATALOG_NAME_CHARS: usize = 127;

/// How many characters a FEDERATED data catalog name may use.
///
/// `CreateDataCatalogInput$Name` documents: "since we use it to create other resources
/// that allow less characters and we prepend a prefix to it, the actual catalog name
/// limit for `FEDERATED` catalog is 64 - 23 = 41 characters."
pub const MAX_FEDERATED_DATA_CATALOG_NAME_CHARS: usize = 41;

/// The characters a data catalog name may use beside the alphanumeric ones, from the
/// `CreateDataCatalogInput$Name` documentation quoted on
/// [`MAX_DATA_CATALOG_NAME_CHARS`]: underscore, at sign and hyphen.
pub const DATA_CATALOG_NAME_PUNCTUATION: [char; 3] = ['_', '@', '-'];

/// How many data catalogs one account holds in this emulator.
///
/// This is the EMULATOR's own bound and has no counterpart in the model: it is chosen
/// far above any plausible set of registered catalogs and exists only so that the
/// registry cannot grow without limit. A write past it is refused with the operation's
/// own `InvalidRequestException`.
pub const MAX_DATA_CATALOGS: usize = 1000;

/// The largest `Parameters` map a stored data catalog may hold, in bytes of keys and
/// values (see [`parameters_bytes`]).
///
/// Another bound with no AWS counterpart: the model bounds one `ParametersMap` value at
/// 51200 bytes and one key at 255, but not how many entries the map holds, so this is
/// what keeps [`MAX_DATA_CATALOGS`] times the size of one record a bounded product. It
/// is well above the handful of entries a catalog of any type documents, including a
/// federated catalog's escaped-JSON connection properties.
pub const MAX_DATA_CATALOG_PARAMETERS_BYTES: usize = 128 * 1024;

/// The `Parameters` key a HIVE or LAMBDA catalog names its metadata function with.
///
/// Every key below is from the `DataCatalog$Parameters` documentation, which gives the
/// syntax per catalog type.
pub const PARAMETER_METADATA_FUNCTION: &str = "metadata-function";
/// The `Parameters` key a LAMBDA catalog names its record function with.
pub const PARAMETER_RECORD_FUNCTION: &str = "record-function";
/// The `Parameters` key a LAMBDA catalog names its composite function with.
pub const PARAMETER_FUNCTION: &str = "function";
/// The `Parameters` key a GLUE catalog names its catalog id with.
pub const PARAMETER_CATALOG_ID: &str = "catalog-id";
/// The `Parameters` key a FEDERATED catalog reuses an existing Glue connection with.
pub const PARAMETER_CONNECTION_ARN: &str = "connection-arn";
/// The `Parameters` key a FEDERATED catalog states a new connection's type with.
pub const PARAMETER_CONNECTION_TYPE: &str = "connection-type";

/// One data catalog, as this service persists it.
///
/// The status is NOT stored: it follows from [`create_complete_at`](Self::create_complete_at),
/// so a read derives it from the clock instead of a reader having to move it.
#[derive(Debug, Clone)]
pub struct DataCatalogRecord {
    /// The catalog name, which is also its key in the registry.
    pub name: String,
    /// A `DataCatalogType` value: one of [`DATA_CATALOG_TYPE_FEDERATED`],
    /// [`DATA_CATALOG_TYPE_GLUE`], [`DATA_CATALOG_TYPE_HIVE`] or
    /// [`DATA_CATALOG_TYPE_LAMBDA`].
    pub catalog_type: String,
    /// The description the create call carried, if any.
    pub description: Option<String>,
    /// The `Parameters` the create call carried. A `BTreeMap` so the stored map has one
    /// stable order whatever order the request listed it in.
    pub parameters: BTreeMap<String, String>,
    /// The `ConnectionType` of a federated catalog that states one, read once from
    /// [`PARAMETER_CONNECTION_TYPE`] at create time.
    pub connection_type: Option<String>,
    /// When this catalog's creation is complete. For a synchronously created type this
    /// is the instant of the create call itself; for a federated one it is
    /// [`FEDERATED_DATA_CATALOG_CREATION`] after it.
    pub create_complete_at: SystemTime,
    /// The tags the create call carried; keys are unique within this list.
    pub tags: Vec<Tag>,
}

impl DataCatalogRecord {
    /// The `DataCatalogStatus` this catalog is in at `now`.
    ///
    /// `DataCatalog$Status` documents the `LAMBDA`, `GLUE` and `HIVE` types as created
    /// synchronously and the `FEDERATED` type as created asynchronously, so a catalog is
    /// [`DATA_CATALOG_STATUS_CREATE_IN_PROGRESS`] until its completion is due and
    /// [`DATA_CATALOG_STATUS_CREATE_COMPLETE`] from then on. Derived, never written: two
    /// reads at the same instant report the same status.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.create_complete_at {
            DATA_CATALOG_STATUS_CREATE_IN_PROGRESS
        } else {
            DATA_CATALOG_STATUS_CREATE_COMPLETE
        }
    }

    /// The record as the modelled `DataCatalog` shape a read answers with, as of `now`.
    pub fn to_data_catalog(&self, now: SystemTime) -> DataCatalog {
        self.to_data_catalog_in_status(self.status_at(now))
    }

    /// The record as the `DataCatalog` a DELETE answers with: the same projection a read
    /// answers with, in the deletion status the removal reached.
    ///
    /// `DataCatalog$Status` is "the status of the creation or deletion of the data
    /// catalog", and what this projection reports is the deletion — complete, because the
    /// registry no longer holds the record when the delete returns, so no part of the
    /// removal is still owed and no later read of it is possible.
    pub fn to_deleted_data_catalog(&self) -> DataCatalog {
        self.to_data_catalog_in_status(DATA_CATALOG_STATUS_DELETE_COMPLETE)
    }

    /// The record as the modelled `DataCatalog` shape, reporting `status`. Every other
    /// member is the record's own, so a read and a delete differ in the status alone.
    fn to_data_catalog_in_status(&self, status: &str) -> DataCatalog {
        DataCatalog {
            name: self.name.clone(),
            description: self.description.clone(),
            r#type: self.catalog_type.clone(),
            parameters: if self.parameters.is_empty() {
                None
            } else {
                Some(
                    self.parameters
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
            },
            status: Some(status.to_string()),
            connection_type: self.connection_type.clone(),
            // `DataCatalog$Error` is the "text of the error that occurred during data
            // catalog creation or deletion"; nothing here fails, so there is none.
            error: None,
        }
    }
}

/// When a catalog of `catalog_type` created at `now` is complete.
///
/// `DataCatalog$Status` documents the `LAMBDA`, `GLUE` and `HIVE` types as created
/// synchronously — complete as the create call returns — and the `FEDERATED` type as
/// created asynchronously, which is due [`FEDERATED_DATA_CATALOG_CREATION`] later. The
/// addition is checked, so a clock far enough in the future to overflow yields a catalog
/// that is already complete rather than a panic.
pub fn create_complete_at(catalog_type: &str, now: SystemTime) -> SystemTime {
    if catalog_type == DATA_CATALOG_TYPE_FEDERATED {
        now.checked_add(FEDERATED_DATA_CATALOG_CREATION).unwrap_or(now)
    } else {
        now
    }
}

/// Why `name` is not a usable data catalog name for `catalog_type`, if it is not.
///
/// Enforces the two documented bounds quoted on [`MAX_DATA_CATALOG_NAME_CHARS`] and
/// [`MAX_FEDERATED_DATA_CATALOG_NAME_CHARS`] plus the documented character set, none of
/// which the model's `CatalogNameString` carries.
pub fn data_catalog_name_error(name: &str, catalog_type: &str) -> Option<String> {
    let limit = if catalog_type == DATA_CATALOG_TYPE_FEDERATED {
        MAX_FEDERATED_DATA_CATALOG_NAME_CHARS
    } else {
        MAX_DATA_CATALOG_NAME_CHARS
    };
    let length = name.chars().count();
    if length > limit {
        return Some(format!(
            "Name is {length} characters long, more than the {limit} characters a \
             {catalog_type} data catalog name can use."
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|ch| !ch.is_ascii_alphanumeric() && !DATA_CATALOG_NAME_PUNCTUATION.contains(ch))
    {
        return Some(format!(
            "Name contains the character '{bad}'; a data catalog name can use \
             alphanumeric, underscore, at sign, or hyphen characters."
        ));
    }
    None
}

/// Why `parameters` does not describe a catalog of `catalog_type`, if it does not.
///
/// `DataCatalog$Parameters` is "a mapping whose values depend on the catalog type" and
/// states, per type, which keys are required and which sets are mutually exclusive; a
/// map that satisfies none of those sets names no catalog Athena could register. A type
/// the documentation states no requirement for constrains nothing here.
pub fn data_catalog_parameters_error(
    catalog_type: &str,
    parameters: &HashMap<String, String>,
) -> Option<String> {
    let has = |key: &str| parameters.contains_key(key);
    match catalog_type {
        // "For the HIVE data catalog type ... The metadata-function parameter is
        // required."
        DATA_CATALOG_TYPE_HIVE if !has(PARAMETER_METADATA_FUNCTION) => Some(format!(
            "Parameters for a {DATA_CATALOG_TYPE_HIVE} data catalog must contain \
             {PARAMETER_METADATA_FUNCTION}."
        )),
        // "For the LAMBDA data catalog type, use one of the following sets of required
        // parameters, but not both": metadata-function with record-function, or
        // function on its own.
        DATA_CATALOG_TYPE_LAMBDA => {
            let composite = has(PARAMETER_FUNCTION);
            let split = has(PARAMETER_METADATA_FUNCTION) && has(PARAMETER_RECORD_FUNCTION);
            let partial_split = has(PARAMETER_METADATA_FUNCTION) || has(PARAMETER_RECORD_FUNCTION);
            if composite && partial_split {
                Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_LAMBDA} data catalog must contain \
                     either {PARAMETER_FUNCTION} or {PARAMETER_METADATA_FUNCTION} with \
                     {PARAMETER_RECORD_FUNCTION}, but not both."
                ))
            } else if composite || split {
                None
            } else {
                Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_LAMBDA} data catalog must contain \
                     either {PARAMETER_FUNCTION} or both {PARAMETER_METADATA_FUNCTION} and \
                     {PARAMETER_RECORD_FUNCTION}."
                ))
            }
        }
        // "The GLUE type takes a catalog ID parameter and is required."
        DATA_CATALOG_TYPE_GLUE if !has(PARAMETER_CATALOG_ID) => Some(format!(
            "Parameters for a {DATA_CATALOG_TYPE_GLUE} data catalog must contain \
             {PARAMETER_CATALOG_ID}."
        )),
        // "The FEDERATED data catalog type uses one of the following parameters, but not
        // both": connection-arn, or connection-type (with connection-properties).
        DATA_CATALOG_TYPE_FEDERATED => {
            match (has(PARAMETER_CONNECTION_ARN), has(PARAMETER_CONNECTION_TYPE)) {
                (true, true) => Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_FEDERATED} data catalog must contain \
                     either {PARAMETER_CONNECTION_ARN} or {PARAMETER_CONNECTION_TYPE}, but \
                     not both."
                )),
                (false, false) => Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_FEDERATED} data catalog must contain \
                     either {PARAMETER_CONNECTION_ARN} or {PARAMETER_CONNECTION_TYPE}."
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The `ConnectionType` a federated catalog's `Parameters` state, and whether it is one
/// the model's `ConnectionType` enum holds.
///
/// `DataCatalog$ConnectionType` is that enum and `DataCatalog$Parameters` writes the
/// parameter as `connection-type:MYSQL|REDSHIFT|....`, so a value outside the enum is
/// one no read could report. `Ok(None)` means the parameter is absent, which is what a
/// catalog reusing an existing connection, or one of any other type, looks like.
pub fn federated_connection_type(
    catalog_type: &str,
    parameters: &HashMap<String, String>,
) -> Result<Option<String>, String> {
    if catalog_type != DATA_CATALOG_TYPE_FEDERATED {
        return Ok(None);
    }
    match parameters.get(PARAMETER_CONNECTION_TYPE) {
        None => Ok(None),
        Some(value) if CONNECTION_TYPE_VALUES.contains(&value.as_str()) => Ok(Some(value.clone())),
        Some(value) => Err(format!(
            "Parameters names the connection type '{value}', which is not a supported \
             connection type. Supported connection types are {}.",
            CONNECTION_TYPE_VALUES.join(", ")
        )),
    }
}

/// The bytes of keys and values held in a `ParametersMap`, checked against
/// [`MAX_DATA_CATALOG_PARAMETERS_BYTES`] before the map is stored.
///
/// Linear in the size of the map it measures, and it allocates nothing.
pub fn parameters_bytes(parameters: &HashMap<String, String>) -> usize {
    parameters
        .iter()
        .fold(0usize, |bytes, (key, value)| {
            bytes.saturating_add(key.len()).saturating_add(value.len())
        })
}

// ---------------------------------------------------------------------------
// Query executions
// ---------------------------------------------------------------------------

/// The workgroup a query belongs to when the request names none.
///
/// `ListQueryExecutionsInput$WorkGroup` documents it: "If a workgroup is not specified,
/// a list of available query execution IDs for the queries in the primary workgroup is
/// returned", and `StartQueryExecution` likewise runs in the primary workgroup. It is
/// Athena's own default workgroup, present in every account, which is why a query may
/// name it without this registry holding a [`WorkGroupRecord`] for it.
pub const DEFAULT_WORK_GROUP: &str = "primary";

/// The database a query runs in when its `QueryExecutionContext` names none. The Glue
/// Data Catalog's own default database carries this name, so a query that qualifies
/// nothing runs where an unqualified name resolves.
pub const DEFAULT_QUERY_DATABASE: &str = "default";

/// The `QueryExecutionState` of a query that has been submitted and is waiting for
/// resources. `QueryExecutionStatus$State` documents it: "QUEUED indicates that the
/// query has been submitted to the service, and Athena will execute the query as soon as
/// resources are available." One of `QUERY_EXECUTION_STATE_VALUES` in
/// [`crate::ops::types`].
pub const QUERY_STATE_QUEUED: &str = "QUEUED";
/// The `QueryExecutionState` of a query in its execution phase; see
/// [`QUERY_STATE_QUEUED`].
pub const QUERY_STATE_RUNNING: &str = "RUNNING";
/// The `QueryExecutionState` of a query that "completed without errors"; see
/// [`QUERY_STATE_QUEUED`].
pub const QUERY_STATE_SUCCEEDED: &str = "SUCCEEDED";
/// The `QueryExecutionState` of a query that "experienced an error and did not complete
/// processing"; see [`QUERY_STATE_QUEUED`].
pub const QUERY_STATE_FAILED: &str = "FAILED";
/// The `QueryExecutionState` of a query "a user input interrupted"; see
/// [`QUERY_STATE_QUEUED`].
pub const QUERY_STATE_CANCELLED: &str = "CANCELLED";

/// How long after `StartQueryExecution` a query's completion is DUE.
///
/// Athena hands back a query execution in a non-terminal state and the terminal state
/// arrives later, so this is a transition the CLOCK owes: the instant is stored on the
/// record and every read derives the state from it. The span is this emulator's, not a
/// duration AWS publishes (a real query takes as long as the data it scans), and it is
/// never zero — a caller reading straight back must still observe the transient states
/// and the errors they carry.
pub const QUERY_EXECUTION_COMPLETION: Duration = Duration::from_secs(3);

/// How long a query stays [`QUERY_STATE_QUEUED`] before it is [`QUERY_STATE_RUNNING`]:
/// the first half of [`QUERY_EXECUTION_COMPLETION`], so both non-terminal states Athena
/// reports are observable and neither is skipped.
pub const QUERY_EXECUTION_QUEUED: Duration = Duration::from_millis(1_500);

/// The `StateChangeReason` of a query a `StopQueryExecution` call interrupted.
pub const QUERY_CANCELLED_BY_USER: &str = "Query cancelled by user";

/// The `AthenaError$ErrorCategory` of a query the engine refused: the documented
/// category list is "1 - System, 2 - User, 3 - Other", and SQL the engine would not run
/// is the caller's own input.
const ERROR_CATEGORY_USER: i32 = 2;

/// The `AthenaError$ErrorCategory` of a query this EMULATOR stopped — a bound it holds
/// a run to, or its confinement refusing what the statement asked for: neither the
/// caller's SQL being wrong nor the engine failing, which is what "3 - Other" names in
/// the list quoted on [`ERROR_CATEGORY_USER`].
const ERROR_CATEGORY_OTHER: i32 = 3;

/// The extension of the result object a statement whose results are ROWS writes.
///
/// Athena's own documentation states the rule for where a query's results are saved:
/// "<query-result-location>/<QueryExecutionId>.csv" for a statement that produces a
/// result set, so the result location a read reports names that object rather than the
/// directory it sits in.
pub const RESULT_OBJECT_ROWS_SUFFIX: &str = ".csv";

/// The extension of the result object a statement whose result is the engine's TEXT
/// output writes — the statements Athena classifies as DDL and UTILITY; see
/// [`RESULT_OBJECT_ROWS_SUFFIX`].
pub const RESULT_OBJECT_TEXT_SUFFIX: &str = ".txt";

/// How many query executions this emulator retains.
///
/// The EMULATOR's own bound, with no counterpart in the model: each retained execution
/// holds its query text and its result set, so without it a caller could make the
/// emulator keep result sets without limit. Athena retains query history for 45 days,
/// which is a retention this emulator cannot usefully apply in a process lifetime, so
/// the bound is a count and the OLDEST execution is dropped when a new one arrives past
/// it.
pub const MAX_QUERY_EXECUTIONS: usize = 200;

/// How many query databases may be open at once. The catalog and database a query names
/// are caller-chosen, so without this bound a caller could make the emulator open engine
/// connections without limit.
pub const MAX_QUERY_DATABASES: usize = 16;

/// One query execution, as this service persists it.
///
/// The outcome is stored once, by the call that ran the SQL; the STATE is derived from
/// it and the clock ([`QueryExecutionRecord::state_at`]), so no read moves a query
/// along and two reads at the same instant report the same state.
#[derive(Debug, Clone)]
pub struct QueryExecutionRecord {
    /// The `QueryExecutionId` this execution was minted under, which is also its key.
    pub id: String,
    /// The `QueryString` submitted, byte for byte, reported as `QueryExecution$Query`.
    pub query_string: String,
    /// The `StatementType` the submitted SQL parses as, or `None` when it does not
    /// parse at all — there is then no statement to classify.
    pub statement_type: Option<String>,
    /// The `SubstatementType` the submitted SQL parses as — the kind of statement, a
    /// finer classification than [`Self::statement_type`] — or `None` for a statement
    /// whose kind has no such name.
    pub substatement_type: Option<String>,
    /// The workgroup the query belongs to, [`DEFAULT_WORK_GROUP`] when it named none.
    pub work_group: String,
    /// The `QueryExecutionContext` the request carried, as it carried it: the database
    /// and catalog the query addressed, or an empty context when it named neither (the
    /// defaults an unqualified query resolves against are
    /// [`GLUE_DATA_CATALOG_NAME`] and [`DEFAULT_QUERY_DATABASE`]).
    pub context: QueryExecutionContext,
    /// The result configuration in force for this query, with `OutputLocation`
    /// resolved against the workgroup's own settings by the call that started it.
    pub result_configuration: ResultConfiguration,
    /// The managed-results configuration this query ran under, when its workgroup keeps
    /// its results in Athena's own storage rather than at an output location (see
    /// [`resolved_managed_query_results`]).
    pub managed_query_results: Option<ManagedQueryResultsConfiguration>,
    /// The result-reuse configuration the request carried, if any.
    pub result_reuse_configuration: Option<ResultReuseConfiguration>,
    /// The `ExecutionParameters` the request carried, in order.
    pub execution_parameters: Vec<String>,
    /// The engine version the query ran on.
    pub engine_version: EngineVersion,
    /// When `StartQueryExecution` accepted it, reported as `SubmissionDateTime`.
    pub submitted_at: SystemTime,
    /// When its completion is due: at least [`QUERY_EXECUTION_COMPLETION`] after the
    /// submit, and never before the engine had actually finished.
    pub completion_due_at: SystemTime,
    /// What running the SQL produced, or `None` while the run has not reported — the
    /// record is written BEFORE the SQL runs, so one `ClientRequestToken` can never
    /// start a second run, and a query whose run has not reported is still RUNNING.
    pub outcome: Option<query_engine::Outcome>,
    /// What the engine spent running it, reported as `EngineExecutionTimeInMillis`.
    pub engine_time: Duration,
    /// What resolving this query's statement against the data catalog spent before the
    /// engine ran it, reported as `QueryPlanningTimeInMillis`.
    pub planning_time: Duration,
    /// When the engine reported on this run, or `None` while it has not. With
    /// [`Self::engine_time`] behind it and [`Self::submitted_at`] in front of it, this is
    /// what the service's own pre-processing and publishing spans are measured between.
    pub engine_reported_at: Option<SystemTime>,
    /// When a `StopQueryExecution` call interrupted it, if one did. Only a query that
    /// had not reached a terminal state can carry this.
    pub cancelled_at: Option<SystemTime>,
}

impl QueryExecutionRecord {
    /// When this query stops being [`QUERY_STATE_QUEUED`].
    fn running_due_at(&self) -> SystemTime {
        self.submitted_at
            .checked_add(QUERY_EXECUTION_QUEUED)
            .unwrap_or(self.completion_due_at)
    }

    /// The `QueryExecutionState` this query is in at `now`.
    ///
    /// Derived, never written: a cancelled query is [`QUERY_STATE_CANCELLED`] from the
    /// instant it was cancelled, and every other query walks QUEUED, then RUNNING, then
    /// the terminal state its stored outcome names — and stays RUNNING while its run has
    /// reported no outcome at all, because a query whose work is not over has not
    /// reached a terminal state however long ago it was submitted.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.cancelled_at.is_some() {
            return QUERY_STATE_CANCELLED;
        }
        if now < self.running_due_at() {
            return QUERY_STATE_QUEUED;
        }
        let Some(outcome) = &self.outcome else {
            return QUERY_STATE_RUNNING;
        };
        if now < self.completion_due_at {
            return QUERY_STATE_RUNNING;
        }
        match outcome {
            query_engine::Outcome::Succeeded { .. } => QUERY_STATE_SUCCEEDED,
            query_engine::Outcome::Failed { .. } => QUERY_STATE_FAILED,
        }
    }

    /// Whether this query has reached a terminal state at `now`.
    pub fn is_terminal_at(&self, now: SystemTime) -> bool {
        !matches!(
            self.state_at(now),
            QUERY_STATE_QUEUED | QUERY_STATE_RUNNING
        )
    }

    /// When this query completed, or `None` while it has not: `CompletionDateTime` is
    /// the completion a terminal state reports, and a running query has none.
    pub fn completion_at(&self, now: SystemTime) -> Option<SystemTime> {
        if let Some(cancelled) = self.cancelled_at {
            return Some(cancelled);
        }
        (self.outcome.is_some() && now >= self.completion_due_at)
            .then_some(self.completion_due_at)
    }

    /// The result set this query produced, once it has SUCCEEDED. `None` for a query
    /// that is not there yet, failed, or was cancelled: a result set is readable only
    /// from a query that completed without errors.
    pub fn result_set_at(&self, now: SystemTime) -> Option<&query_engine::StoredResultSet> {
        if self.state_at(now) != QUERY_STATE_SUCCEEDED {
            return None;
        }
        match self.outcome.as_ref()? {
            query_engine::Outcome::Succeeded { result, .. } => Some(result),
            query_engine::Outcome::Failed { .. } => None,
        }
    }

    /// How many rows this query's statement wrote, for `UpdateCount`.
    ///
    /// A statement whose written rows the engine counted reports that count. One that
    /// produced a RESULT SET returned its rows rather than writing any, so it reports a
    /// count of zero rather than no count at all. A statement that produced no result set
    /// either — one whose result is the engine's text output — reports none.
    pub fn update_count(&self) -> Option<i64> {
        match self.outcome.as_ref()? {
            query_engine::Outcome::Succeeded {
                update_count: Some(count),
                ..
            } => Some(*count),
            query_engine::Outcome::Succeeded { result, .. } => {
                (!result.column_info.is_empty()).then_some(0)
            }
            query_engine::Outcome::Failed { .. } => None,
        }
    }

    /// The result set a read of this execution's results reports at `now`, or `None` for
    /// a query that has no output to read.
    ///
    /// A statement whose results are ROWS — one Athena classifies as DML — has them only
    /// once it has SUCCEEDED. A statement whose result is the engine's TEXT output — one
    /// Athena classifies as DDL or UTILITY — has that output as soon as the statement was
    /// REPORTED ON, whether what reported was the engine (the statement ran, or ran into
    /// an error) or the data catalog (the statement named something it does not hold), and
    /// the result set that output carries is the empty one. A query still running, one a
    /// caller cancelled and one this emulator stopped before anything could report
    /// produced no output at all, so there is nothing to read from any of them.
    pub fn readable_result_at(&self, now: SystemTime) -> Option<&query_engine::StoredResultSet> {
        match self.state_at(now) {
            QUERY_STATE_SUCCEEDED => self.result_set_at(now),
            QUERY_STATE_FAILED
                if self.writes_text_output()
                    && self.failure().is_some_and(|(_, _, cause)| {
                        matches!(
                            cause,
                            query_engine::FailureCause::Sql
                                | query_engine::FailureCause::Athena { .. }
                        )
                    }) =>
            {
                Some(&query_engine::TEXT_OUTPUT_RESULT_SET)
            }
            _ => None,
        }
    }

    /// Whether this execution's statement writes its result as the engine's TEXT output:
    /// the statements Athena classifies as DDL and UTILITY return no result set, and what
    /// their run produces is the text the engine wrote about the task.
    pub(crate) fn writes_text_output(&self) -> bool {
        matches!(
            self.statement_type.as_deref(),
            Some(query_engine::STATEMENT_TYPE_DDL) | Some(query_engine::STATEMENT_TYPE_UTILITY)
        )
    }

    /// The result object this execution's run wrote, as `OutputLocation` reports it from
    /// the instant the query is terminal: the configured results DIRECTORY with the object
    /// inside it, named after the execution and carrying the extension its statement's
    /// results take ([`RESULT_OBJECT_ROWS_SUFFIX`], [`RESULT_OBJECT_TEXT_SUFFIX`]).
    ///
    /// `None` while the query has not run to a terminal state, for one a caller cancelled
    /// and for text that parses as no statement at all: none of those wrote an object to
    /// name, so the configured directory is what the record still reports.
    fn result_object_at(&self, now: SystemTime) -> Option<String> {
        if self.cancelled_at.is_some() || !self.is_terminal_at(now) {
            return None;
        }
        self.result_object_uri()
    }

    /// The key of the result object this execution's statement writes, as the `s3://` URI
    /// of the configured results directory with that object under it — the ONE place that
    /// key is formed, so the object a read NAMES ([`Self::result_object_at`]) and the
    /// object a completed run SAVES ([`query_results::result_objects`]) are the same key.
    ///
    /// `None` for text that parses as no statement at all, and for a submission that
    /// resolved no results location: neither addresses an object.
    pub(crate) fn result_object_uri(&self) -> Option<String> {
        let suffix = match self.statement_type.as_deref()? {
            query_engine::STATEMENT_TYPE_DML => RESULT_OBJECT_ROWS_SUFFIX,
            query_engine::STATEMENT_TYPE_DDL | query_engine::STATEMENT_TYPE_UTILITY => {
                RESULT_OBJECT_TEXT_SUFFIX
            }
            _ => return None,
        };
        // The configured location names a directory, whether or not it was written with
        // the trailing separator, so the object is joined under it by exactly one.
        let directory = self
            .result_configuration
            .output_location
            .as_deref()?
            .trim_end_matches('/');
        Some(format!("{directory}/{}{suffix}", self.id))
    }

    /// The `ResultConfiguration` a read of this execution reports at `now`: the one the
    /// query ran under, with `OutputLocation` naming the result object once there is one
    /// (see [`Self::result_object_at`]).
    fn reported_result_configuration(&self, now: SystemTime) -> ResultConfiguration {
        let mut configuration = self.result_configuration.clone();
        if let Some(object) = self.result_object_at(now) {
            configuration.output_location = Some(object);
        }
        configuration
    }

    /// The `ResultReuseConfiguration` this query ran under: the one its submission
    /// carried, and otherwise result reuse turned OFF.
    ///
    /// `ResultReuseByAgeConfiguration$Enabled` is "True if previous query results can be
    /// reused when the query is run; otherwise, false" — and a query whose submission
    /// asked for no reuse ran the statement itself, which is that member as `false`.
    fn effective_result_reuse_configuration(&self) -> ResultReuseConfiguration {
        let by_age = self
            .result_reuse_configuration
            .as_ref()
            .and_then(|configuration| configuration.result_reuse_by_age_configuration.clone())
            .unwrap_or_default();
        ResultReuseConfiguration {
            result_reuse_by_age_configuration: Some(by_age),
        }
    }

    /// The message for why this query failed, the `StateChangeReason` reported beside it
    /// where the two differ, and whose doing the failure was, if it failed.
    fn failure(&self) -> Option<(&str, Option<&str>, query_engine::FailureCause)> {
        match self.outcome.as_ref()? {
            query_engine::Outcome::Failed {
                message,
                reason,
                cause,
            } => Some((message, reason.as_deref(), *cause)),
            query_engine::Outcome::Succeeded { .. } => None,
        }
    }

    /// The record as the modelled `QueryExecution` a read answers with.
    pub fn to_query_execution(&self, now: SystemTime) -> QueryExecution {
        let state = self.state_at(now);
        let failure = (state == QUERY_STATE_FAILED).then(|| self.failure()).flatten();
        let state_change_reason = match state {
            QUERY_STATE_CANCELLED => Some(QUERY_CANCELLED_BY_USER.to_string()),
            // The reason a failed query reports is what performed the statement wrote
            // about the failure, which for a DDL task is the task's own line and for
            // every other class is the failure's own message.
            QUERY_STATE_FAILED => failure
                .map(|(message, reason, _)| reason.unwrap_or(message).to_string()),
            _ => None,
        };
        let athena_error = failure.map(|(message, _, cause)| crate::ops::types::AthenaError {
            error_category: Some(match cause {
                query_engine::FailureCause::Sql => ERROR_CATEGORY_USER,
                query_engine::FailureCause::Emulator => ERROR_CATEGORY_OTHER,
                // A failure of one of Athena's own classes carries that class's
                // category, which is neither the engine's doing nor a bound of this
                // emulator.
                query_engine::FailureCause::Athena { category, .. } => category,
            }),
            // Athena's error TYPE is a published code per failure class: the one
            // reported is the one the failure's own class carries, and a failure the
            // engine reported carries none, so none is invented for it.
            error_type: match cause {
                query_engine::FailureCause::Athena { error_type, .. } => error_type,
                _ => None,
            },
            // "The FAILED state is always terminal with no automatic retry."
            retryable: false,
            error_message: Some(message.to_string()),
        });
        QueryExecution {
            query_execution_id: Some(self.id.clone()),
            query: Some(self.query_string.clone()),
            statement_type: self.statement_type.clone(),
            // The configuration a query whose results Athena keeps in its own storage ran
            // under; a query that wrote them at an output location ran under none.
            managed_query_results_configuration: self.managed_query_results.clone(),
            result_configuration: (self.managed_query_results.is_none()
                || self.result_configuration.output_location.is_some())
            .then(|| self.reported_result_configuration(now)),
            result_reuse_configuration: Some(self.effective_result_reuse_configuration()),
            query_execution_context: Some(self.context.clone()),
            status: Some(QueryExecutionStatus {
                state: Some(state.to_string()),
                state_change_reason,
                submission_date_time: Some(::aws_smithy_types::DateTime::from(self.submitted_at)),
                completion_date_time: self
                    .completion_at(now)
                    .map(::aws_smithy_types::DateTime::from),
                athena_error,
            }),
            statistics: Some(self.statistics_at(now)),
            work_group: Some(self.work_group.clone()),
            engine_version: Some(self.engine_version.clone()),
            // `ExecutionParameters` holds at least one value, so a query that carried
            // none reports the member as absent rather than as an empty list.
            execution_parameters: (!self.execution_parameters.is_empty())
                .then(|| self.execution_parameters.clone()),
            substatement_type: self.substatement_type.clone(),
            query_results_s3_access_grants_configuration: None,
        }
    }

    /// The `QueryExecutionStatistics` this query reports at `now`.
    ///
    /// Measured, not chosen: the engine time is what the engine actually spent, the
    /// scanned bytes are the size of the values it read, and the queue and total times
    /// come from the instants on the record.
    fn statistics_at(&self, now: SystemTime) -> QueryExecutionStatistics {
        let terminal = self.is_terminal_at(now);
        let queue_until = now.min(self.running_due_at());
        let total_until = self.completion_at(now).unwrap_or(now);
        // The service's own spans around the engine's, measured between the record's
        // instants: everything before the engine started is what this service spent
        // preprocessing the query, and everything from the engine's report to the
        // completion is what it spent finalising and publishing the results \u2014 which is
        // exactly when they become readable. Reported once the query is terminal, when
        // both spans are over; a query whose run never reported has neither.
        let engine_started_at = self
            .engine_reported_at
            .and_then(|reported| reported.checked_sub(self.engine_time));
        let pre_processing = engine_started_at
            .filter(|_| terminal)
            .map(|started| millis_between(self.submitted_at, started));
        let processing = self
            .engine_reported_at
            .filter(|_| terminal)
            .zip(self.completion_at(now))
            .map(|(reported, completed)| millis_between(reported, completed));
        let data_scanned = match (self.outcome.as_ref(), terminal) {
            (
                Some(query_engine::Outcome::Succeeded {
                    data_scanned_bytes, ..
                }),
                true,
            ) => *data_scanned_bytes,
            _ => 0,
        };
        QueryExecutionStatistics {
            // The engine ran once, when the query was submitted; its cost is reported
            // from the moment the query is terminal, which is when the run is over as
            // far as a caller can see.
            engine_execution_time_in_millis: Some(if terminal {
                millis_of(self.engine_time)
            } else {
                0
            }),
            data_scanned_in_bytes: Some(data_scanned),
            // Written only when a query's results are written as a manifest, which this
            // emulator does not do.
            data_manifest_location: None,
            total_execution_time_in_millis: Some(millis_between(self.submitted_at, total_until)),
            query_queue_time_in_millis: Some(millis_between(self.submitted_at, queue_until)),
            service_pre_processing_time_in_millis: pre_processing,
            // Athena reports the planning phase separately from the engine's own work:
            // "the number of milliseconds that Athena took to plan the query processing
            // flow[,] includ[ing] the time spent retrieving table partitions from the data
            // source". That is what resolving this statement against the data catalog
            // spent, measured on the run itself, and it is reported from the moment the
            // query is terminal, exactly as the engine's own span is.
            query_planning_time_in_millis: terminal.then(|| millis_of(self.planning_time)),
            service_processing_time_in_millis: processing,
            // Every query here is RUN: no read of a result set is answered out of an
            // earlier query's results, so no run reused a previous result.
            result_reuse_information: Some(ResultReuseInformation {
                reused_previous_result: false,
            }),
            dpu_count: None,
        }
    }
}

/// A duration in whole milliseconds, saturating.
fn millis_of(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// The whole milliseconds from `from` to `until`, or 0 when `until` is not after `from`.
fn millis_between(from: SystemTime, until: SystemTime) -> i64 {
    until
        .duration_since(from)
        .map_or(0, millis_of)
}

/// A fresh `QueryExecutionId`: a version-4 UUID, which is the form Athena mints a query
/// execution id in and satisfies the modelled `QueryExecutionId` (1 to 128 characters,
/// `^\S+$`). Minted from the maintained crate rather than laid out here.
pub fn new_query_execution_id() -> String {
    ::uuid::Uuid::new_v4().as_hyphenated().to_string()
}

/// The result configuration ONE query runs under, given the configuration the request
/// carried and the workgroup the query belongs to.
///
/// `ResultConfiguration$OutputLocation` documents the rule: "To run the query, you must
/// specify the query results location using one of the ways: either for individual
/// queries using either this setting (client-side), or in the workgroup […] If none of
/// them is set, Athena issues an error that no output location is provided. If workgroup
/// settings override client-side settings, then the query uses the settings specified for
/// the workgroup." So a workgroup that enforces its own configuration replaces the
/// request's whole configuration with the one it SPECIFIES, and otherwise the request's
/// configuration stands with the workgroup's output location behind it. A workgroup that
/// specifies no result configuration at all overrides nothing — there is no setting of its
/// own to use — so the client's own stands even where the workgroup enforces its
/// configuration. `None` when neither names a location, which is the refusal the quoted
/// sentence calls for.
pub fn resolved_result_configuration(
    requested: Option<ResultConfiguration>,
    work_group: Option<&WorkGroupRecord>,
) -> Option<ResultConfiguration> {
    let group = work_group.and_then(|record| record.configuration.result_configuration.clone());
    let enforced = work_group
        .and_then(|record| record.configuration.enforce_work_group_configuration)
        .unwrap_or(false)
        && output_location(group.as_ref()).is_some();
    let resolved = if enforced {
        group.clone()
    } else {
        match (requested, group.clone()) {
            (Some(mut requested), group) => {
                if output_location(Some(&requested)).is_none() {
                    requested.output_location = group.and_then(|group| group.output_location);
                }
                Some(requested)
            }
            (None, group) => group,
        }
    };
    let resolved = resolved?;
    output_location(Some(&resolved))
        .is_some()
        .then_some(resolved)
}

/// The output location a result configuration names, treating an empty one as naming
/// none: a location of no characters addresses no bucket.
fn output_location(configuration: Option<&ResultConfiguration>) -> Option<&str> {
    configuration
        .and_then(|configuration| configuration.output_location.as_deref())
        .map(str::trim)
        .filter(|location| !location.is_empty())
}

/// The managed-results configuration a query in `work_group` runs under, when that
/// workgroup stores its results in Athena's own storage rather than at an output location.
///
/// `ManagedQueryResultsConfiguration$Enabled` documents the rule: "If set to true, allows
/// you to store query results in Athena owned storage. If set to false, workgroup member
/// stores query results in location specified under
/// `ResultConfiguration$OutputLocation`." So a query in such a workgroup needs no output
/// location of its own — which is the second of the two ways the missing-location refusal
/// ([`NO_OUTPUT_LOCATION`]) names — and the configuration it ran under is what a read of
/// the execution reports.
pub fn resolved_managed_query_results(
    work_group: Option<&WorkGroupRecord>,
) -> Option<ManagedQueryResultsConfiguration> {
    work_group
        .and_then(|record| record.configuration.managed_query_results_configuration.clone())
        .filter(|managed| managed.enabled)
}

/// The message a query with nowhere to write its results is refused with: the error the
/// `ResultConfiguration$OutputLocation` sentence quoted on
/// [`resolved_result_configuration`] calls for.
/// Wholly fixed text: it states the rule and the two ways of satisfying it, and
/// interpolates nothing from the request.
pub const NO_OUTPUT_LOCATION: &str = concat!(
    "No output location provided. You did not provide an output location for  your query ",
    "results. Either specify an S3 bucket location or enable Athena managed query results ",
    "in your workgroup settings."
);

/// The message a `StartQueryExecution` that reuses a `ClientRequestToken` under changed
/// parameters is refused with. `ClientRequestToken` documents it: "An error is returned
/// if a parameter, such as QueryString, has changed."
pub const IDEMPOTENT_PARAMETERS_DO_NOT_MATCH: &str = "Idempotent parameters do not match";

/// Whether two result configurations say the same thing, member by member.
///
/// Compared rather than derived: a reused `ClientRequestToken` may not have changed any
/// parameter of the query, and where its results are written and how they are encrypted
/// are parameters of it.
pub fn same_result_configuration(one: &ResultConfiguration, other: &ResultConfiguration) -> bool {
    let encryption = |configuration: &ResultConfiguration| {
        configuration
            .encryption_configuration
            .as_ref()
            .map(|encryption| (encryption.encryption_option.clone(), encryption.kms_key.clone()))
    };
    let acl = |configuration: &ResultConfiguration| {
        configuration
            .acl_configuration
            .as_ref()
            .map(|acl| acl.s3_acl_option.clone())
    };
    one.output_location == other.output_location
        && one.expected_bucket_owner == other.expected_bucket_owner
        && encryption(one) == encryption(other)
        && acl(one) == acl(other)
}

/// Whether two result-reuse configurations say the same thing; see
/// [`same_result_configuration`].
pub fn same_result_reuse_configuration(
    one: Option<&ResultReuseConfiguration>,
    other: Option<&ResultReuseConfiguration>,
) -> bool {
    let by_age = |configuration: Option<&ResultReuseConfiguration>| {
        configuration
            .and_then(|configuration| configuration.result_reuse_by_age_configuration.as_ref())
            .map(|by_age| (by_age.enabled, by_age.max_age_in_minutes))
    };
    by_age(one) == by_age(other)
}

/// The message a read of a query execution no `StartQueryExecution` minted answers with.
pub fn query_execution_not_found(id: &str) -> String {
    format!("QueryExecution {id} was not found")
}

/// The message an operation addressing a workgroup this account holds none of answers
/// with.
///
/// Wholly fixed text: the workgroup the request named is NOT interpolated into it, so
/// every operation that resolves a `WorkGroup` — a read of one, an update of one, a query
/// submitted to one, a notebook imported into one — states this one sentence.
pub const WORK_GROUP_NOT_FOUND: &str = "WorkGroup is not found.";

/// The message a create of a workgroup under a name this account already holds one under
/// answers with. Fixed text as well, and with no closing period: the name the request
/// carried is the caller's own and is not echoed back.
pub const WORK_GROUP_ALREADY_CREATED: &str = "WorkGroup is already created";

/// The message a query submitted to a workgroup in state [`WORK_GROUP_STATE_DISABLED`]
/// is refused with.
///
/// A workgroup's state is what decides whether queries may run in it — Athena documents
/// that members of a disabled workgroup cannot submit queries to it — so a submission to
/// a disabled workgroup is refused rather than queued. Not recorded, unlike the miss
/// [`WORK_GROUP_NOT_FOUND`] states, so this one still names the workgroup it refused.
pub fn work_group_disabled(name: &str) -> String {
    format!("WorkGroup {name} is disabled.")
}

/// The error code of a refusal whose cause is the request itself: a member naming a
/// resource this account does not hold, a name already taken, a value outside a rule this
/// service enforces. The code every Athena refusal carries unless the refusal class has
/// one of its own ([`ERROR_CODE_QUERY_EXECUTION_NOT_FOUND`],
/// [`ERROR_CODE_INVALID_QUERY_EXECUTION_STATE`]), and the one
/// `BatchGetQueryExecution` reports beside [`query_execution_not_found`] on an
/// `UnprocessedQueryExecutionId`.
pub const ERROR_CODE_INVALID_INPUT: &str = "INVALID_INPUT";

/// The error code of a refusal whose cause is that the `QueryExecutionId` names no
/// execution this account minted: its own class, so a poller can tell a permanent miss
/// from any other bad request and stop.
pub const ERROR_CODE_QUERY_EXECUTION_NOT_FOUND: &str = "QUERY_EXECUTION_NOT_FOUND";

/// The error code of a refusal whose cause is the STATE the addressed execution is in
/// rather than the request: a read of results of a query that has not finished, or that
/// did not finish successfully. Its own class, so a client can tell "poll again" from a
/// request it must not repeat.
pub const ERROR_CODE_INVALID_QUERY_EXECUTION_STATE: &str = "INVALID_QUERY_EXECUTION_STATE";

/// The message `GetQueryResults` refuses a query that has not finished with.
pub fn query_not_yet_finished(state: &str) -> String {
    format!("Query has not yet finished. Current state: {state}")
}

/// The message `GetQueryResults` refuses a query that finished in a state other than
/// SUCCEEDED with: there are no results to read from a query that did not succeed.
pub fn query_did_not_succeed(state: &str) -> String {
    format!("Query did not finish successfully. Final query state: {state}")
}

/// The message a paginated read refuses a `NextToken` it did not mint with.
pub const INVALID_NEXT_TOKEN: &str = "Invalid NextToken is provided";

/// The separator inside a page cursor. A unit separator, which no identifier, workgroup
/// name or decimal position can contain, so the parts of a cursor can never run together.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The cursor a `GetQueryResults` page hands back: the execution it is paging and the
/// row to resume at.
fn results_cursor(id: &str, position: usize) -> String {
    format!("results{CURSOR_SEPARATOR}{id}{CURSOR_SEPARATOR}{position}")
}

/// The cursor a `ListQueryExecutions` page hands back: the workgroup it filtered on and
/// the position to resume at.
fn executions_cursor(work_group: &str, position: usize) -> String {
    format!("executions{CURSOR_SEPARATOR}{work_group}{CURSOR_SEPARATOR}{position}")
}

/// The cursor a `ListWorkGroups` page hands back: the scope it walked \u2014 the whole
/// account, since the operation models no member that narrows it \u2014 and the position to
/// resume at.
fn work_groups_cursor(position: usize) -> String {
    format!("workgroups{CURSOR_SEPARATOR}{ACCOUNT_SCOPE}{CURSOR_SEPARATOR}{position}")
}

/// The scope of a listing that walks everything this account holds, carried in that
/// listing's cursors where a filtered listing carries what it filtered on.
const ACCOUNT_SCOPE: &str = "account";

/// The position `cursor` names for `kind` within `scope`, or `None` for any cursor that
/// is not one of those — including one minted for another execution, another workgroup
/// or another operation, which names no position in THIS read.
fn cursor_position(cursor: &str, kind: &str, scope: &str) -> Option<usize> {
    let mut parts = cursor.split(CURSOR_SEPARATOR);
    let minted_kind = parts.next()?;
    let minted_scope = parts.next()?;
    let position = parts.next()?;
    if parts.next().is_some() || minted_kind != kind || minted_scope != scope {
        return None;
    }
    position.parse().ok()
}

/// Per-service state for com-amazonaws-athena-amazonathena. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsAthenaAmazonathenaState {
    /// When this account's Athena registries were opened, which is the `CreationTime` of
    /// the account's default workgroup: that workgroup is there before any
    /// `CreateWorkGroup` call, so its creation instant belongs to the account rather than
    /// to a request. Held once, so every read reports the same instant for it.
    opened_at: SystemTime,
    /// Every workgroup a `CreateWorkGroup` call registered, keyed by its name. A
    /// `BTreeMap` so a listing walks the registry in one stable order. The account's
    /// default workgroup is NOT here \u2014 nothing created it; see
    /// [`default_work_group_record`] and [`Self::work_group_summaries`].
    pub work_groups: BTreeMap<String, WorkGroupRecord>,
    /// Every data catalog this account holds, keyed by its name — the key the
    /// `CreateDataCatalogInput$Name` documentation calls unique per account. A
    /// `BTreeMap` for the same reason as [`Self::work_groups`].
    pub data_catalogs: BTreeMap<String, DataCatalogRecord>,
    /// Every query execution this account still holds, keyed by its
    /// `QueryExecutionId`. Written only through [`Self::record_query_execution`], which
    /// is what keeps it and [`Self::query_execution_order`] in step.
    query_executions: HashMap<String, QueryExecutionRecord>,
    /// The ids of [`Self::query_executions`] in submission order, oldest first — the
    /// order `ListQueryExecutions` reports reversed, and the order executions are
    /// dropped in once [`MAX_QUERY_EXECUTIONS`] is reached.
    query_execution_order: Vec<String>,
    /// Which `QueryExecutionId` each `ClientRequestToken` already minted, so the
    /// documented idempotency ("the same response is returned and another query is not
    /// created") is answered from state. An entry is dropped with the execution it
    /// names.
    idempotency_tokens: HashMap<String, String>,
    /// The engine databases queries run in, keyed by the catalog and database they
    /// address. Opened on demand by [`Self::query_database`], never by a read.
    query_databases: BTreeMap<String, query_engine::DatabaseHandle>,
    /// Every notebook this account holds, with the import tokens that minted them.
    /// `ImportNotebook` writes it and `ExportNotebook` reads it; see
    /// [`NotebookRegistry`].
    pub notebooks: NotebookRegistry,
    /// Every capacity reservation this account holds. `CreateCapacityReservation` writes
    /// it and `GetCapacityReservation` reads it; see [`CapacityReservationRegistry`].
    pub capacity_reservations: CapacityReservationRegistry,
    /// Every prepared statement this account holds, by workgroup.
    /// `CreatePreparedStatement` writes it and `GetPreparedStatement` reads it; see
    /// [`PreparedStatementRegistry`].
    pub prepared_statements: PreparedStatementRegistry,
    /// Every named query this account holds, each belonging to one workgroup.
    /// `CreateNamedQuery` writes it and `GetNamedQuery` reads it; see
    /// [`NamedQueryRegistry`].
    pub named_queries: NamedQueryRegistry,
    /// The secret this store's page tokens are authenticated under, so a `NextToken`
    /// this store did not mint names no page. Drawn from the system's randomness when
    /// the store is created and never reported.
    page_token_key: PageTokenKey,
}

impl Default for ComAmazonawsAthenaAmazonathenaState {
    /// A store holding nothing a request wrote, opened NOW: the account's default
    /// workgroup dates from this instant (see [`Self::opened_at`]), which is why this
    /// store is constructed rather than derived.
    fn default() -> Self {
        Self {
            opened_at: SystemTime::now(),
            work_groups: BTreeMap::new(),
            data_catalogs: BTreeMap::new(),
            query_executions: HashMap::new(),
            query_execution_order: Vec::new(),
            idempotency_tokens: HashMap::new(),
            query_databases: BTreeMap::new(),
            notebooks: NotebookRegistry::default(),
            capacity_reservations: CapacityReservationRegistry::default(),
            prepared_statements: PreparedStatementRegistry::default(),
            named_queries: NamedQueryRegistry::default(),
            page_token_key: PageTokenKey::new(),
        }
    }
}

impl ComAmazonawsAthenaAmazonathenaState {
    /// Whether this account has the workgroup `name`: one `CreateWorkGroup` registered,
    /// or [`DEFAULT_WORK_GROUP`] \u2014 Athena's own default workgroup, which every account
    /// has and which therefore needs no record here.
    pub fn holds_work_group(&self, name: &str) -> bool {
        name == DEFAULT_WORK_GROUP || self.work_groups.contains_key(name)
    }

    /// Every workgroup this account holds, as the summaries a listing reports, in name
    /// order.
    ///
    /// That is each one a `CreateWorkGroup` call registered and has not been deleted,
    /// plus the account's default workgroup ([`default_work_group_record`]), which exists
    /// without a record of its own \u2014 the same set [`Self::holds_work_group`] answers for.
    /// A record registered under the default workgroup's own name is reported from that
    /// record, so no workgroup is reported twice.
    pub fn work_group_summaries(&self) -> Vec<WorkGroupSummary> {
        let mut by_name: BTreeMap<&str, WorkGroupSummary> = self
            .work_groups
            .iter()
            .map(|(name, record)| (name.as_str(), record.to_summary()))
            .collect();
        by_name
            .entry(DEFAULT_WORK_GROUP)
            .or_insert_with(|| default_work_group_record(self.opened_at).to_summary());
        by_name.into_values().collect()
    }

    /// The execution `id` names, or `None` when this account holds none.
    pub fn query_execution(&self, id: &str) -> Option<&QueryExecutionRecord> {
        self.query_executions.get(id)
    }

    /// The `QueryExecutionId` a `ClientRequestToken` already minted, if it did.
    pub fn query_execution_for_token(&self, token: &str) -> Option<&QueryExecutionRecord> {
        let id = self.idempotency_tokens.get(token)?;
        self.query_executions.get(id)
    }

    /// The ids of the executions of `work_group`, newest first — the order
    /// `ListQueryExecutions` reports.
    pub fn query_execution_ids(&self, work_group: &str) -> Vec<&str> {
        self.query_execution_order
            .iter()
            .rev()
            .filter_map(|id| self.query_executions.get(id))
            .filter(|record| record.work_group == work_group)
            .map(|record| record.id.as_str())
            .collect()
    }

    /// Store a query execution, under `token` when the request carried one.
    ///
    /// The oldest execution is dropped once [`MAX_QUERY_EXECUTIONS`] are held, together
    /// with any token that named it: a token whose execution is gone cannot answer with
    /// the same id, so it is not kept pointing at nothing.
    pub fn record_query_execution(&mut self, record: QueryExecutionRecord, token: Option<String>) {
        while self.query_execution_order.len() >= MAX_QUERY_EXECUTIONS {
            let oldest = self.query_execution_order.remove(0);
            self.query_executions.remove(&oldest);
            self.idempotency_tokens
                .retain(|_, minted| minted != &oldest);
        }
        let id = record.id.clone();
        if let Some(token) = token {
            self.idempotency_tokens.insert(token, id.clone());
        }
        self.query_execution_order.push(id.clone());
        self.query_executions.insert(id, record);
    }

    /// Write what the run of execution `id` produced onto its record.
    ///
    /// Called by the request that started the query, once the engine has reported. The
    /// completion instant is moved out to `now` when the run took longer than the span
    /// the record was stored with, so a query is never reported complete while its work
    /// was still running. Does nothing for an execution the registry no longer holds.
    pub fn complete_query_execution(
        &mut self,
        id: &str,
        completion: query_engine::Completion,
        now: SystemTime,
    ) {
        let Some(record) = self.query_executions.get_mut(id) else {
            return;
        };
        record.completion_due_at = record.completion_due_at.max(now);
        record.engine_time = completion.engine_time;
        record.planning_time = record.planning_time.saturating_add(completion.planning_time);
        record.engine_reported_at = Some(now);
        record.outcome = Some(completion.outcome);
    }

    /// Cancel the execution `id` names, as `StopQueryExecution` does.
    ///
    /// `true` when this call is what cancelled it; `false` for an execution that had
    /// already reached a terminal state, which a stop leaves exactly as it was — a query
    /// that already succeeded or failed was not interrupted by a user.
    pub fn cancel_query_execution(&mut self, id: &str, now: SystemTime) -> bool {
        let Some(record) = self.query_executions.get_mut(id) else {
            return false;
        };
        if record.is_terminal_at(now) {
            return false;
        }
        record.cancelled_at = Some(now);
        true
    }

    /// The engine database for `catalog` and `database`, opened on first use.
    ///
    /// `Err` carries the engine's own message when a database cannot be opened, and the
    /// emulator's when [`MAX_QUERY_DATABASES`] are already open.
    pub fn query_database(
        &mut self,
        catalog: &str,
        database: &str,
    ) -> Result<query_engine::DatabaseHandle, String> {
        let key = format!("{catalog}{CURSOR_SEPARATOR}{database}");
        if let Some(handle) = self.query_databases.get(&key) {
            return Ok(::std::sync::Arc::clone(handle));
        }
        if self.query_databases.len() >= MAX_QUERY_DATABASES {
            return Err(format!(
                "this emulator holds the maximum of {MAX_QUERY_DATABASES} query \
                 databases open, so no query can be run in {database}"
            ));
        }
        let handle = query_engine::open_database()?;
        self.query_databases
            .insert(key, ::std::sync::Arc::clone(&handle));
        Ok(handle)
    }

    /// The `NextToken` a `GetQueryResults` page of execution `id` resumes at `position`
    /// hands back.
    pub fn results_page_token(&self, id: &str, position: usize) -> String {
        ::hex::encode(self.page_token_key.mint(&results_cursor(id, position)))
    }

    /// The row `token` resumes execution `id` at, or `None` for a token this store did
    /// not mint for THAT execution.
    pub fn results_page_position(&self, id: &str, token: &str) -> Option<usize> {
        let cursor = self.page_cursor(token)?;
        cursor_position(&cursor, "results", id)
    }

    /// The `NextToken` a `ListQueryExecutions` page of `work_group` resumed at
    /// `position` hands back.
    pub fn executions_page_token(&self, work_group: &str, position: usize) -> String {
        ::hex::encode(
            self.page_token_key
                .mint(&executions_cursor(work_group, position)),
        )
    }

    /// The position `token` resumes a listing of `work_group` at, or `None` for a token
    /// this store did not mint for THAT workgroup.
    pub fn executions_page_position(&self, work_group: &str, token: &str) -> Option<usize> {
        let cursor = self.page_cursor(token)?;
        cursor_position(&cursor, "executions", work_group)
    }

    /// The `NextToken` a `ListWorkGroups` page resumed at `position` hands back.
    pub fn work_groups_page_token(&self, position: usize) -> String {
        ::hex::encode(self.page_token_key.mint(&work_groups_cursor(position)))
    }

    /// The position `token` resumes a workgroup listing at, or `None` for a token this
    /// store did not mint for THAT listing.
    pub fn work_groups_page_position(&self, token: &str) -> Option<usize> {
        let cursor = self.page_cursor(token)?;
        cursor_position(&cursor, "workgroups", ACCOUNT_SCOPE)
    }

    /// The cursor `token` carries, or `None` for any value this store did not mint.
    fn page_cursor(&self, token: &str) -> Option<String> {
        let bytes = ::hex::decode(token).ok()?;
        self.page_token_key.cursor_of(&bytes)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAthenaAmazonathenaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAthenaAmazonathenaState>)
}
