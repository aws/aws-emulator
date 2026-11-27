// SPDX-License-Identifier: CC0-1.0

//! The RUN of a protected query: the SQL a membership submitted, EXECUTED over the data
//! the configured tables of its collaboration really hold, with the rows it computed
//! written where the query's result configuration points.
//!
//! Clean Rooms runs a protected query over the configured tables a collaboration
//! associates, and this module is where that happens: the relations the SQL names are
//! resolved to the configured table associations of the submitting membership, each one to
//! the Data Catalog table its configured table references, and the rows of that table are
//! the OBJECTS at the catalog table's location, read out of this emulator's own object
//! store. The statement is then handed as it stands to the in-process SQL engine this
//! workspace embeds ([`::service_common::sql_wire_planes::sql_engine`]), and what that
//! engine answers is what the query reports: nothing is read off the statement's text and
//! no row is synthesised.
//!
//! What a relation EXPOSES to the statement is the analysis rules of its configured table:
//! only the columns a rule lets a query read are loaded into the engine, so a statement
//! that names any other column is answered by the engine the way a statement naming a
//! column that is not there is answered, rather than reading a cell the collaboration
//! never allowed.
//!
//! The result is written as the result configuration in effect asks for
//! ([`super::effective_result_configuration`]): one object per query under
//! `<keyPrefix>/<query id>/`, in the `resultFormat` that configuration names, through the
//! object store's own write — so the location the query reports is an object a later
//! `GetObject` or `ListObjectsV2` really serves.
//!
//! # Bounds
//!
//! Every size a caller can grow is capped before it is read: at most [`MAX_RELATION_ROWS`]
//! rows per relation and [`MAX_RELATION_BYTES`] of object data per run, the statement text
//! is the one the submitting call already had refused past its own cap
//! ([`super::sql_referenced_tables`]), and the engine itself carries the statement
//! deadline, the row ceiling and the answer-byte ceiling of every SQL data plane of this
//! workspace. The run is BLOCKING work on the blocking pool, started by the call that
//! materialises the completion the clock owes ([`super::settle_protected_queries`]), and no
//! guard of this service is held while the statement executes.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_common::catalog_table_rows::{self, RowsRefusal};
use ::service_common::sql_wire_planes::sql_engine::{
    Cell, DatabaseEngine, EngineError, EngineView, ResultSet,
};

use crate::ops::types::{
    AnalysisSource, ConfiguredTable, ConfiguredTableAnalysisRulePolicy,
    ConfiguredTableAnalysisRulePolicyV1, ProtectedQuery, ProtectedQueryError,
    ProtectedQueryOutput, ProtectedQueryOutputConfiguration, ProtectedQueryResult,
    ProtectedQueryS3Output, ProtectedQueryS3OutputConfiguration, ProtectedQuerySingleMemberOutput,
    RetryInfo, TableReference,
};

use super::CleanRoomsState;

/// The `ProtectedQueryError.code` of a query whose statement is not one that can be run:
/// "the query is not valid (syntax, semantics, columns, or an unsupported operation)". The
/// MESSAGE of such a failure is the engine's or the parser's own sentence about the
/// statement, never a sentence written here.
pub const PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY: &str = "MALFORMED_QUERY";

/// The `ProtectedQueryError.code` of a query that named a resource nothing holds: "a
/// referenced resource doesn't exist" — the analysis template an ARN points at, the Data
/// Catalog table a configured table references, the bucket a result configuration writes
/// to.
pub const PROTECTED_QUERY_ERROR_CODE_RESOURCE_NOT_FOUND: &str = "RESOURCE_NOT_FOUND";

/// The `ProtectedQueryError.code` of a query whose resources are all there but cannot be
/// used as they stand: "a referenced resource exists, but you can't use it because its
/// configuration, state, or data is not valid" — a configured table carrying no analysis
/// rule, a table reference into a store with no data behind it, data that does not decode
/// as the format its own catalog entry declares, an output the results cannot be written
/// to.
pub const PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE: &str = "INVALID_RESOURCE";

/// The `ProtectedQueryError.code` of a query that was not allowed to reach what it reads:
/// "a required permission is missing for a data source or resource".
pub const PROTECTED_QUERY_ERROR_CODE_RESOURCE_ACCESS_DENIED: &str = "RESOURCE_ACCESS_DENIED";

/// The `ProtectedQueryError.code` of a query that asked for more than there was to run it
/// with: "the query exceeded the available compute (memory, disk, or time)" — which is
/// what a run past [`MAX_RELATION_ROWS`] or [`MAX_RELATION_BYTES`] went past.
pub const PROTECTED_QUERY_ERROR_CODE_INSUFFICIENT_COMPUTE: &str = "INSUFFICIENT_COMPUTE";

/// The `ProtectedQueryError.code` of a query that failed on a condition that has since
/// passed: "a temporary condition caused the failure, such as throttling or a network
/// interruption".
pub const PROTECTED_QUERY_ERROR_CODE_TRANSIENT_ERROR: &str = "TRANSIENT_ERROR";

/// The `ProtectedQueryError.code` of a query the service itself could not carry out:
/// "Clean Rooms encountered an unexpected error" — which is what a store this emulator
/// could not reach at all is.
pub const PROTECTED_QUERY_ERROR_CODE_INTERNAL_ERROR: &str = "INTERNAL_ERROR";

/// The `ProtectedQueryError.code` of a failure none of the categories above describes:
/// "the failure couldn't be classified".
pub const PROTECTED_QUERY_ERROR_CODE_UNKNOWN_ERROR: &str = "UNKNOWN_ERROR";

/// Every failure category the model declares for a protected query, with whether running
/// the same query again might succeed.
///
/// The categories are the valid values the model documents for `ProtectedQueryError$code`,
/// and the second column is what `RetryInfo$retryable` says of each: `true` where the
/// category IS a temporary condition, `false` where the failure is decided by the request
/// or by state that running it again does not change, and nothing at all for the category
/// that stands for a failure the service could not classify — whose transience is, by that
/// definition, not known, so the member is left off rather than guessed.
pub const PROTECTED_QUERY_ERROR_CODE_RETRYABILITY: &[(&str, Option<bool>)] = &[
    (PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY, Some(false)),
    (PROTECTED_QUERY_ERROR_CODE_RESOURCE_NOT_FOUND, Some(false)),
    (PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE, Some(false)),
    (PROTECTED_QUERY_ERROR_CODE_RESOURCE_ACCESS_DENIED, Some(false)),
    (PROTECTED_QUERY_ERROR_CODE_INSUFFICIENT_COMPUTE, Some(false)),
    (PROTECTED_QUERY_ERROR_CODE_TRANSIENT_ERROR, Some(true)),
    (PROTECTED_QUERY_ERROR_CODE_INTERNAL_ERROR, Some(true)),
    (PROTECTED_QUERY_ERROR_CODE_UNKNOWN_ERROR, None),
];

/// What a failure of category `code` reports about running the same query again, as
/// `RetryInfo` carries it — `None` where that is not known, so nothing is reported.
pub fn protected_query_error_retry(code: &str) -> Option<RetryInfo> {
    let retryable = PROTECTED_QUERY_ERROR_CODE_RETRYABILITY
        .iter()
        .find(|(category, _)| *category == code)
        .and_then(|(_, retryable)| *retryable)?;
    Some(RetryInfo {
        retryable: Some(retryable),
    })
}

/// The error a failed protected query reports: the category it failed in, the sentence
/// saying why, and the retry advice that category carries.
pub fn protected_query_error(code: &str, message: String) -> ProtectedQueryError {
    ProtectedQueryError {
        code: code.to_string(),
        message,
        retry: protected_query_error_retry(code),
    }
}

/// The most rows one run loads out of ONE relation.
///
/// CHOSEN, like every other bound here: the rows are held in this process's memory while
/// the statement runs. A relation with more rows than this is reported as a configuration
/// this emulator cannot run the query over, rather than read in part — a partial read
/// would answer a query over data the collaboration holds with rows computed from a slice
/// of it.
pub const MAX_RELATION_ROWS: usize = 20_000;

/// The most object bytes one run reads in all, for the same reason.
pub const MAX_RELATION_BYTES: usize = 8 * 1024 * 1024;

/// The `ResultFormat` values the model declares (shape `ResultFormat`: CSV | PARQUET).
const RESULT_FORMAT_PARQUET: &str = "PARQUET";

/// What one run of a protected query produced.
#[derive(Debug, Clone)]
pub enum QueryRun {
    /// The statement ran, and this is where its rows were written (`None` where the result
    /// configuration in effect names no location an object can be written at, so nothing
    /// was written).
    Succeeded(Option<ProtectedQueryResult>),
    /// The run did not happen, and this is why — the error the query reports.
    Failed(ProtectedQueryError),
}

/// Run `query`'s statement and report what it produced.
///
/// BLOCKING, and run with NO guard held while the statement executes: the caller hands
/// this to the blocking pool ([`super::settle_protected_queries`]). This service's guard is
/// taken only to RESOLVE what the statement reads — the SQL itself and, for each relation,
/// the catalog table and the columns its analysis rules expose — and is released before the
/// rows are read, before the statement is parsed into relations' data and before the engine
/// runs, so neither the async worker nor this service's store is occupied by a caller's SQL.
pub(crate) fn run_protected_query<Eng: Send + Sync + 'static>(
    engine: &Eng,
    query: &ProtectedQuery,
) -> QueryRun {
    // The RESOLUTION phase, under this service's guard: the statement, and which catalog
    // table each relation stands for with the columns a query may read of it.
    let resolved = {
        let Some(state) = super::service_state(engine) else {
            return QueryRun::Failed(protected_query_error(
                PROTECTED_QUERY_ERROR_CODE_INTERNAL_ERROR,
                "The collaboration this protected query runs in could not be read.".to_string(),
            ));
        };
        let store = state.lock();
        statement_of(&store, query)
            .and_then(|sql| resolve_relations(&store, query, &sql).map(|plans| (sql, plans)))
    };
    let (sql, plans) = match resolved {
        Ok(resolved) => resolved,
        Err(error) => return QueryRun::Failed(error),
    };
    // Everything below holds NO guard of this service: the rows are read out of the catalog
    // and the object store under their own, and the statement runs on the engine.
    let relations = match load_relations(engine, &plans) {
        Ok(relations) => relations,
        Err(error) => return QueryRun::Failed(error),
    };
    let answered = match compute(&sql, &relations) {
        Ok(answered) => answered,
        Err(error) => {
            return QueryRun::Failed(protected_query_error(
                PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY,
                // The ENGINE's own sentence about the statement it refused.
                error.message,
            ))
        }
    };
    match write_result(engine, query, &answered) {
        Ok(result) => QueryRun::Succeeded(result),
        Err(error) => QueryRun::Failed(error),
    }
}

/// The statement this query runs: the SQL the request carried inline, else the analysis
/// template it named.
///
/// `ProtectedQuerySQLParameters` carries the SQL either inline (`queryString`) or by
/// reference (`analysisTemplateArn`), and a template's own `source` is the analysis it
/// holds — so a query by reference runs the STORED template's text, which is what a later
/// update of that template changes for the next query rather than for this one.
fn statement_of(store: &CleanRoomsState, query: &ProtectedQuery) -> Result<String, ProtectedQueryError> {
    let parameters = query.sql_parameters.as_ref();
    if let Some(sql) = parameters
        .and_then(|parameters| parameters.query_string.as_deref())
        .filter(|sql| !sql.trim().is_empty())
    {
        return Ok(sql.to_string());
    }
    let Some(arn) = parameters.and_then(|parameters| parameters.analysis_template_arn.as_deref())
    else {
        return Err(protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY,
            "The protected query carries no SQL to run: neither queryString nor \
             analysisTemplateArn identifies a statement."
                .to_string(),
        ));
    };
    let Some(template) = store
        .analysis_templates
        .values()
        .find(|template| template.arn == arn)
    else {
        return Err(protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_RESOURCE_NOT_FOUND,
            format!("The analysis template {arn} does not exist."),
        ));
    };
    match &template.source {
        AnalysisSource::Text(text) => Ok(text.clone()),
        // The other member of the union carries the analysis as ARTIFACTS — objects an
        // analysis is assembled from, not a statement this engine runs.
        AnalysisSource::Artifacts(_) => Err(protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
            format!(
                "The analysis template {arn} carries its analysis as artifacts rather than as \
                 SQL text, so it is not a statement that can be run as a SQL protected query."
            ),
        )),
    }
}

/// One relation as the engine holds it: the name the statement writes, the columns the
/// analysis rules of its configured table expose, and the rows the objects at the catalog
/// table's location decode to.
#[derive(Debug, Clone)]
struct Relation {
    /// The configured table association's name — "used to query the underlying configured
    /// table" (`CreateConfiguredTableAssociationInput$name`), which is what the statement
    /// names.
    name: String,
    /// `(column name, the type the catalog declares for it)`, in the catalog's own column
    /// order, narrowed to the columns the analysis rules allow a query to read.
    columns: Vec<(String, String)>,
    /// The rows, one value per column above; `None` is a field that carries no value.
    rows: Vec<Vec<Option<String>>>,
}

/// What one relation of a statement stands for, resolved out of this service's own store:
/// everything the rows behind it are read with, and nothing of this store.
#[derive(Debug, Clone)]
struct RelationPlan {
    /// The configured table association's name — "used to query the underlying configured
    /// table" (`CreateConfiguredTableAssociationInput$name`), which is what the statement
    /// names and what the relation is held under in the engine.
    name: String,
    /// The configured table's own name, which is what a failure about it names.
    configured_table: String,
    /// The Data Catalog database and table the configured table references.
    database: String,
    table: String,
    /// The columns the analysis rules of that configured table let a query read, in the
    /// order the configured table declares them.
    exposed: Vec<String>,
}

/// Resolve every relation `sql` names, as the submitting membership may read it.
///
/// Reads THIS service's store and nothing else, so the guard it is called under is
/// released before any object data is read ([`load_relations`]).
fn resolve_relations(
    store: &CleanRoomsState,
    query: &ProtectedQuery,
    sql: &str,
) -> Result<Vec<RelationPlan>, ProtectedQueryError> {
    let named = super::sql_referenced_tables(sql).map_err(|error| {
        protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY,
            unparsed_statement_message(&error),
        )
    })?;
    let mut plans = Vec::new();
    for name in named {
        // A name the submitting membership has no association under is not a relation of
        // this collaboration at all: nothing is loaded for it, and the statement is
        // answered by the engine the way one naming a table that is not there is.
        let Some(table) = configured_table_for(store, &query.membership_id, &name) else {
            continue;
        };
        let exposed = exposed_columns(store, table);
        if exposed.is_empty() {
            return Err(protected_query_error(
                PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
                format!(
                    "The configured table {} associated as {name} carries no analysis rule that \
                     allows this query to read any of its columns.",
                    table.name
                ),
            ));
        }
        let TableReference::Glue(reference) = &table.table_reference else {
            // Every other member of the `TableReference` union names a table in a store
            // this emulator holds nothing of, so there is no data behind it to read.
            return Err(protected_query_error(
                PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
                format!(
                    "The configured table {} associated as {name} references a table this \
                     collaboration cannot read data from.",
                    table.name
                ),
            ));
        };
        plans.push(RelationPlan {
            name,
            configured_table: table.name.clone(),
            database: reference.database_name.clone(),
            table: reference.table_name.clone(),
            exposed,
        });
    }
    Ok(plans)
}

/// Read the rows of every resolved relation, charging them all to one byte budget.
fn load_relations<Eng: Send + Sync + 'static>(
    engine: &Eng,
    plans: &[RelationPlan],
) -> Result<Vec<Relation>, ProtectedQueryError> {
    let mut budget = MAX_RELATION_BYTES;
    plans
        .iter()
        .map(|plan| load_relation(engine, plan, &mut budget))
        .collect()
}

/// Why the statement of a protected query could not be read as a query at all, worded as
/// this service words the same reading of an analysis template's text: the parser's own
/// complaint, or the bound the text went past.
fn unparsed_statement_message(error: &super::SchemaError) -> String {
    match error {
        super::SchemaError::TooMuchText(cap) => format!(
            "The SQL of this protected query is longer than the {cap} characters of analysis \
             this service reads."
        ),
        super::SchemaError::TooComplex(tokens) => format!(
            "The SQL of this protected query carries more than the {tokens} tokens of analysis \
             this service parses."
        ),
        super::SchemaError::TooDeep(depth) => format!(
            "The SQL of this protected query nests deeper than the {depth} levels of analysis \
             this service parses."
        ),
        super::SchemaError::NotAQuery(complaint) => {
            format!("The SQL of this protected query is not a query: {complaint}")
        }
        super::SchemaError::TooManyRelations => {
            "The SQL of this protected query reads more relations than this service resolves \
             for one analysis."
                .to_string()
        }
        super::SchemaError::UnreportableRelation(relation) => format!(
            "The SQL of this protected query reads the relation {relation}, which is not a name \
             a configured table association can carry."
        ),
    }
}

/// The configured table the membership's association `name` points at.
fn configured_table_for<'store>(
    store: &'store CleanRoomsState,
    membership_id: &str,
    name: &str,
) -> Option<&'store ConfiguredTable> {
    let association_id = store
        .configured_table_association_by_name
        .get(&(membership_id.to_string(), name.to_string()))?;
    let association = store.configured_table_associations.get(association_id)?;
    store.configured_tables.get(&association.configured_table_id)
}

/// The columns of `table` a query may READ, in the order the configured table declares
/// them.
///
/// A configured table says which of its columns may be used "within collaborations" at all
/// (`ConfiguredTable$allowedColumns`), and its ANALYSIS RULES say what a query may do with
/// them: a `list` rule names the columns that can be joined on and the ones that can be
/// listed in the output, an `aggregation` rule the columns that can be joined on, grouped
/// by and aggregated, and a `custom` rule allows the table's columns except the ones it
/// disallows in output. So the columns a statement can reach are the union of what the
/// table's rules allow, intersected with the columns the table allows at all — and a
/// column outside that set is not loaded, which is what keeps a statement from reading it.
fn exposed_columns(store: &CleanRoomsState, table: &ConfiguredTable) -> Vec<String> {
    let mut allowed: Vec<String> = Vec::new();
    for rule_type in &table.analysis_rule_types {
        let Some(rule) = store
            .configured_table_analysis_rules
            .get(&(table.id.clone(), rule_type.clone()))
        else {
            continue;
        };
        let ConfiguredTableAnalysisRulePolicy::V1(policy) = &rule.policy;
        match policy {
            ConfiguredTableAnalysisRulePolicyV1::List(list) => {
                allowed.extend(list.join_columns.iter().cloned());
                allowed.extend(list.list_columns.iter().cloned());
            }
            ConfiguredTableAnalysisRulePolicyV1::Aggregation(aggregation) => {
                allowed.extend(aggregation.join_columns.iter().cloned());
                allowed.extend(aggregation.dimension_columns.iter().cloned());
                for column in &aggregation.aggregate_columns {
                    allowed.extend(column.column_names.iter().cloned());
                }
            }
            ConfiguredTableAnalysisRulePolicyV1::Custom(custom) => {
                let disallowed = custom.disallowed_output_columns.clone().unwrap_or_default();
                allowed.extend(
                    table
                        .allowed_columns
                        .iter()
                        .filter(|column| !disallowed.contains(column))
                        .cloned(),
                );
            }
        }
    }
    table
        .allowed_columns
        .iter()
        .filter(|column| allowed.iter().any(|named| named == *column))
        .cloned()
        .collect()
}

/// Read the rows of one resolved relation: the objects at the location the Data Catalog
/// table its configured table references holds, decoded as the table's own SerDe says they
/// are.
fn load_relation<Eng: Send + Sync + 'static>(
    engine: &Eng,
    plan: &RelationPlan,
    budget: &mut usize,
) -> Result<Relation, ProtectedQueryError> {
    let unreadable = |code: &str, detail: &str| {
        protected_query_error(
            code,
            format!(
                "The Data Catalog table {}.{} that the configured table {} references {detail}",
                plan.database, plan.table, plan.configured_table
            ),
        )
    };
    let Some(catalog) = ::service_com_amazonaws_glue_awsglue::state::service_state(engine) else {
        return Err(unreadable(
            PROTECTED_QUERY_ERROR_CODE_INTERNAL_ERROR,
            "could not be read.",
        ));
    };
    // What the catalog holds for this table is copied under its guard, and the guard is
    // released before the object store is reached.
    let held = {
        let store = catalog.lock();
        store
            .database(crate::state::caller_account_id(), &plan.database)
            .and_then(|database| {
                database
                    .tables
                    .get(&::service_com_amazonaws_glue_awsglue::state::fold_name(&plan.table))
            })
            .cloned()
    };
    let Some(held) = held else {
        return Err(unreadable(
            PROTECTED_QUERY_ERROR_CODE_RESOURCE_NOT_FOUND,
            "does not exist.",
        ));
    };
    let storage = held.storage_descriptor.as_ref();
    let declared: Vec<(String, String)> = storage
        .and_then(|storage| storage.columns.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|column| (column.name, column.r#type.unwrap_or_default()))
        .collect();
    // How the objects at this table's location decode, off the table's own parameters: the
    // one reading of that, shared with every other service that answers a query over a
    // catalog table.
    let format = catalog_table_rows::text_format(
        &storage
            .and_then(|storage| storage.serde_info.as_ref())
            .and_then(|serde| serde.parameters.clone())
            .unwrap_or_default(),
        &held.parameters.clone().unwrap_or_default(),
    );
    // Which of the table's declared columns the statement may read, and where each one
    // stands in a decoded row.
    let read: Vec<(usize, (String, String))> = declared
        .iter()
        .enumerate()
        .filter(|(_, (column, _))| {
            plan.exposed
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(column))
        })
        .map(|(position, column)| (position, column.clone()))
        .collect();
    if read.is_empty() {
        return Err(unreadable(
            PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
            "declares none of the columns its analysis rules allow this query to read.",
        ));
    }

    let bodies = object_bodies(
        engine,
        storage.and_then(|storage| storage.location.as_deref()),
        &plan.database,
        &plan.table,
        budget,
    )?;
    let picked: Vec<usize> = read.iter().map(|(position, _)| *position).collect();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    for body in bodies {
        let room = MAX_RELATION_ROWS - rows.len();
        match catalog_table_rows::object_rows(&body, &format, &picked, room) {
            Ok(decoded) => rows.extend(decoded),
            // The bytes behind the table are not the format its SerDe declares, or they
            // hold more rows than this emulator reads: either way what decoded is a SLICE
            // of the data, and answering the query over it would answer it from part of
            // what the collaboration holds.
            Err(RowsRefusal::Undecodable) => {
                return Err(unreadable(
                    PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
                    "holds data that could not be decoded as the format its SerDe declares.",
                ))
            }
            Err(RowsRefusal::TooManyRows) => {
                return Err(protected_query_error(
                    PROTECTED_QUERY_ERROR_CODE_INSUFFICIENT_COMPUTE,
                    format!(
                        "The data behind the configured table {} holds more than the \
                         {MAX_RELATION_ROWS} rows this emulator reads for one protected query.",
                        plan.configured_table
                    ),
                ))
            }
        }
    }
    Ok(Relation {
        name: plan.name.clone(),
        columns: read.into_iter().map(|(_, column)| column).collect(),
        rows,
    })
}

/// The bytes of every object under a catalog table's location, charged to `budget`.
fn object_bodies<Eng: Send + Sync + 'static>(
    engine: &Eng,
    location: Option<&str>,
    database_name: &str,
    table_name: &str,
    budget: &mut usize,
) -> Result<Vec<Vec<u8>>, ProtectedQueryError> {
    let unreadable = |code: &str, detail: &str| {
        protected_query_error(
            code,
            format!(
                "The data of the Data Catalog table {database_name}.{table_name} could not be \
                 read: {detail}"
            ),
        )
    };
    // A table whose catalog entry names no location has nothing under it to read, which is
    // an empty relation rather than a failure.
    let Some(location) = location.filter(|at| !at.is_empty()) else {
        return Ok(Vec::new());
    };
    let Some((bucket_name, prefix)) = location_address(location) else {
        return Err(unreadable(
            PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
            &format!("{location} is not an Amazon S3 location"),
        ));
    };
    let Some(objects) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return Err(unreadable(
            PROTECTED_QUERY_ERROR_CODE_INTERNAL_ERROR,
            "there is no object store to read it from",
        ));
    };
    let store = objects.lock();
    let Some(bucket) = store.bucket(&bucket_name) else {
        return Err(unreadable(
            PROTECTED_QUERY_ERROR_CODE_RESOURCE_NOT_FOUND,
            &format!("the bucket {bucket_name} that {location} names does not exist"),
        ));
    };
    let mut bodies = Vec::new();
    for (key, object) in bucket.objects() {
        // A key that only shares the prefix's text is not under it, and a key a reader of a
        // table's location skips is not one of its splits.
        if !key.starts_with(&prefix) || catalog_table_rows::is_hidden_key(key) {
            continue;
        }
        if object.body.len() > *budget {
            return Err(unreadable(
                PROTECTED_QUERY_ERROR_CODE_INSUFFICIENT_COMPUTE,
                &format!(
                    "reading it would scan more than the {MAX_RELATION_BYTES} bytes this \
                     emulator reads for one protected query"
                ),
            ));
        }
        *budget -= object.body.len();
        bodies.push(object.body.clone());
    }
    Ok(bodies)
}

/// `(bucket, key prefix)` of a catalog table's `s3://bucket/prefix` location, through the
/// one reading of an `s3://` URI this workspace has
/// ([`::service_common::s3_uri`]) — a location naming a bucket and nothing under it is
/// every object of that bucket, which is the empty prefix.
fn location_address(location: &str) -> Option<(String, String)> {
    let bucket = ::service_common::s3_uri::bucket_name(location)?;
    let prefix = ::service_common::s3_uri::object_address(location)
        .map(|(_, key)| key)
        .unwrap_or_default();
    Some((bucket, prefix))
}

/// Load `relations` into a fresh in-process engine and run `sql` over them.
///
/// The engine is the one this workspace embeds for its SQL data planes: in memory, bounded
/// and confined to its own tables. Each relation is held in it as a table carrying the
/// columns the analysis rules exposed, through the one load of a catalog relation into that
/// engine ([`::service_common::catalog_table_rows::load_relation`]), and the statement is
/// handed over as it stands.
fn compute(sql: &str, relations: &[Relation]) -> Result<ResultSet, EngineError> {
    let engine = DatabaseEngine::open()?;
    let mut view = EngineView::default();
    for relation in relations {
        catalog_table_rows::load_relation(
            &engine,
            &mut view,
            &relation.name,
            &relation.columns,
            &relation.rows,
        )?;
    }
    engine.execute_as(&mut view, sql)
}

/// Write what the statement computed where the query's result configuration points, and
/// report the `ProtectedQueryResult` the query carries for it.
fn write_result<Eng: Send + Sync + 'static>(
    engine: &Eng,
    query: &ProtectedQuery,
    answered: &ResultSet,
) -> Result<Option<ProtectedQueryResult>, ProtectedQueryError> {
    let Some(configuration) = query.result_configuration.as_ref() else {
        // Nothing names a location for the results, so none was written and the query
        // reports no result rather than one nothing holds.
        return Ok(None);
    };
    let output = match &configuration.output_configuration {
        ProtectedQueryOutputConfiguration::S3(s3) => {
            ProtectedQueryOutput::S3(write_object(engine, query, answered, s3)?)
        }
        // A `member` output configuration delivers the results to that member rather than
        // to a location of the caller's, which is what the `memberList` output reports.
        ProtectedQueryOutputConfiguration::Member(member) => {
            ProtectedQueryOutput::memberList(vec![ProtectedQuerySingleMemberOutput {
                account_id: member.account_id.clone(),
            }])
        }
        ProtectedQueryOutputConfiguration::Distribute(distribute) => {
            let mut s3 = None;
            let mut member_list: Vec<ProtectedQuerySingleMemberOutput> = Vec::new();
            for location in &distribute.locations {
                match location {
                    crate::ops::types::ProtectedQueryDistributeOutputConfigurationLocation::S3(
                        target,
                    ) => s3 = Some(write_object(engine, query, answered, target)?),
                    crate::ops::types::ProtectedQueryDistributeOutputConfigurationLocation::Member(
                        member,
                    ) => member_list.push(ProtectedQuerySingleMemberOutput {
                        account_id: member.account_id.clone(),
                    }),
                }
            }
            ProtectedQueryOutput::Distribute(crate::ops::types::ProtectedQueryDistributeOutput {
                s3,
                member_list: (!member_list.is_empty()).then_some(member_list),
            })
        }
        // An intermediate-table output writes the rows into a collaboration resource this
        // slice holds no data plane for.
        ProtectedQueryOutputConfiguration::intermediateTable(_) => {
            return Err(protected_query_error(
                PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
                "The results of this protected query cannot be written to an intermediate table."
                    .to_string(),
            ))
        }
    };
    Ok(Some(ProtectedQueryResult { output }))
}

/// Write the rows as ONE object under `<keyPrefix>/<query id>/`, and report where.
///
/// The object is written through the object store's own write, so what the query reports
/// as its result location is an address a later read of that store really serves. The
/// object's own key is the query's id under that folder, carrying the extension of the
/// format the result configuration asked for, and the LOCATION the query reports is the
/// folder — the address every object of this one query is under.
fn write_object<Eng: Send + Sync + 'static>(
    engine: &Eng,
    query: &ProtectedQuery,
    answered: &ResultSet,
    target: &ProtectedQueryS3OutputConfiguration,
) -> Result<ProtectedQueryS3Output, ProtectedQueryError> {
    use ::service_com_amazonaws_s3_amazons3::state as objects;

    let parquet = target.result_format.eq_ignore_ascii_case(RESULT_FORMAT_PARQUET);
    let body = if parquet {
        encode_parquet(answered)
    } else {
        encode_csv(answered)
    }
    .ok_or_else(|| {
        protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_INVALID_RESOURCE,
            format!(
                "The results of this protected query could not be written in the {} format.",
                target.result_format
            ),
        )
    })?;
    let folder = result_folder(target.key_prefix.as_deref(), &query.id);
    let extension = if parquet { "parquet" } else { "csv" };
    let key = format!("{folder}{id}.{extension}", id = query.id);
    let Some(store) = objects::service_state(engine) else {
        return Err(protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_INTERNAL_ERROR,
            "There is no object store to write the results of this protected query to."
                .to_string(),
        ));
    };
    let mut store = store.lock();
    // A bucket the result configuration names that nothing holds is a resource the query
    // referenced and that does not exist, rather than a write this emulator refused.
    if store.bucket(&target.bucket).is_none() {
        return Err(protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_RESOURCE_NOT_FOUND,
            format!(
                "The bucket {} that the results of this protected query are written to does not \
                 exist.",
                target.bucket
            ),
        ));
    }
    let encryption = objects::written_object_encryption(&store, &target.bucket, None);
    let object = objects::Object {
        etag: objects::stored_object_etag(&objects::object_etag(&body), &encryption),
        checksum: objects::stored_object_checksum(None, &body),
        body,
        version_id: None,
        last_modified: SystemTime::now(),
        storage_class: objects::DEFAULT_STORAGE_CLASS.to_string(),
        content_type: None,
        cache_control: None,
        content_disposition: None,
        content_encoding: None,
        content_language: None,
        expires: None,
        website_redirect_location: None,
        metadata: BTreeMap::new(),
        tags: Vec::new(),
        encryption,
        retention: None,
        legal_hold: None,
        restore: None,
        annotations: BTreeMap::new(),
        acl: None,
        parts: Vec::new(),
        replication: None,
    };
    if !matches!(
        store.put_object(&target.bucket, &key, object),
        Some(objects::ObjectWrite::Created | objects::ObjectWrite::Replaced)
    ) {
        // The bucket is there, so what refused the write is a ceiling of the object store:
        // the results went past the room there was to hold them.
        return Err(protected_query_error(
            PROTECTED_QUERY_ERROR_CODE_INSUFFICIENT_COMPUTE,
            format!(
                "The results of this protected query could not be written to the bucket {}.",
                target.bucket
            ),
        ));
    }
    Ok(ProtectedQueryS3Output {
        // The `s3://` spelling of the folder, through the scheme this workspace names an
        // S3 location by (`service_common::s3_uri`).
        location: format!(
            "{scheme}://{bucket}/{folder}",
            scheme = ::service_common::s3_uri::S3_SCHEME,
            bucket = target.bucket
        ),
    })
}

/// The folder every object of ONE query's results is written under: the effective key
/// prefix, then the query's own id. Always ends in `/`, so a key is the folder followed by
/// an object name and the location is the folder itself.
fn result_folder(key_prefix: Option<&str>, query_id: &str) -> String {
    match key_prefix.map(|prefix| prefix.trim_end_matches('/')) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{query_id}/"),
        _ => format!("{query_id}/"),
    }
}

/// The rows as comma-separated values: one header record naming the columns the statement
/// answered with, then one record per row.
fn encode_csv(answered: &ResultSet) -> Option<Vec<u8>> {
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    writer
        .write_record(answered.columns.iter().map(|column| column.name.as_str()))
        .ok()?;
    for row in &answered.rows {
        writer.write_record(row.iter().map(cell_text)).ok()?;
    }
    writer.into_inner().ok()
}

/// The rows as a Parquet file: one UTF-8 byte-array column per column the statement
/// answered with, written by the Apache implementation of that format.
fn encode_parquet(answered: &ResultSet) -> Option<Vec<u8>> {
    use ::parquet::basic::{ConvertedType, Repetition, Type as PhysicalType};
    use ::parquet::data_type::{ByteArray, ByteArrayType};
    use ::parquet::file::properties::WriterProperties;
    use ::parquet::file::writer::SerializedFileWriter;
    use ::parquet::schema::types::Type as SchemaType;

    let mut fields = Vec::with_capacity(answered.columns.len());
    for column in &answered.columns {
        fields.push(::std::sync::Arc::new(
            SchemaType::primitive_type_builder(&column.name, PhysicalType::BYTE_ARRAY)
                // Optional: a row need not carry a value for every column.
                .with_repetition(Repetition::OPTIONAL)
                .with_converted_type(ConvertedType::UTF8)
                .build()
                .ok()?,
        ));
    }
    let schema = SchemaType::group_type_builder("protected_query_result")
        .with_fields(fields)
        .build()
        .ok()?;
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        ::std::sync::Arc::new(schema),
        ::std::sync::Arc::new(WriterProperties::builder().build()),
    )
    .ok()?;
    if !answered.rows.is_empty() {
        let mut row_group = writer.next_row_group().ok()?;
        let mut position = 0usize;
        while let Some(mut column_writer) = row_group.next_column().ok()? {
            let mut values: Vec<ByteArray> = Vec::new();
            let mut levels: Vec<i16> = Vec::with_capacity(answered.rows.len());
            for row in &answered.rows {
                match row.get(position) {
                    Some(Cell::Null) | None => levels.push(0),
                    Some(cell) => {
                        values.push(ByteArray::from(cell_text(cell).as_bytes()));
                        levels.push(1);
                    }
                }
            }
            column_writer
                .typed::<ByteArrayType>()
                .write_batch(&values, Some(&levels), None)
                .ok()?;
            column_writer.close().ok()?;
            position += 1;
        }
        row_group.close().ok()?;
    }
    writer.into_inner().ok()
}

/// One answered value as the text a result file carries: a value the engine answered with,
/// in its own storage class, rendered the way that class reads.
fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::Integer(number) => number.to_string(),
        Cell::Real(number) => number.to_string(),
        Cell::Text(text) => text.clone(),
        Cell::Blob(bytes) => String::from_utf8_lossy(bytes).to_string(),
    }
}
