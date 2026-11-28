// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-s3tables-s3tablebuckets` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. This service TAKES it, for one class of request: one whose body it
//!   cannot read into the operation's modelled input, which the codec faults before
//!   any handler runs (see [`request_layer`]). Everything else resolves through the
//!   generated routing table.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! A sixth seam, `override_protocol`, is the one this service DOES take: every timestamp
//! member of this model declares `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",
//! shape="TableSummary,TableBucketSummary,NamespaceSummary,GetTableResponse")` reports it on
//! each), so this service serves the model's `restJson1` codec with every timestamp in that
//! form — `service_common::date_time_json`, which is where the whole workspace's rendering
//! of such a member lives — and with the one read [`record_expiration_retention`] replaces.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, ServerHttpProtocol,
};
use std::sync::Arc;

use crate::ops::types::{BadRequestException, SCHEMA_BAD_REQUEST_EXCEPTION};

/// Whether the resource-based policy a table bucket carries refuses a request —
/// the decision every operation this service serves is taken behind.
mod authorization;

/// What a request addresses: the input members that name the resource it acts
/// on, read once for the decisions taken in front of every handler.
mod addressing;

/// Whether the ARNs a request addresses a resource by name the Region this
/// endpoint serves — the check every operation this service serves is taken
/// behind.
mod addressed_region;

/// How this service refuses a request body it cannot read into the operation's
/// modelled input at all — a layer in FRONT of the generated walk, because the
/// framework states that failure in its own codec's words under an error code
/// this model does not declare.
mod request_layer;

/// The refusals a maintenance put states itself in front of dispatch: the task
/// its path addresses, and the configuration value its body must carry.
mod maintenance_put;

/// Where a maintenance put's path carries the task it addresses, and which tasks
/// each resource has.
mod maintenance_type;

/// The Iceberg REST catalog this service serves beside its control plane, at a
/// path space the model binds no operation to.
mod iceberg_rest;

/// The name members this service resolves against stored state rather than
/// format-checks — the lookup keys, as opposed to the names a create or a
/// rename gives a resource.
mod name_lookups;

/// Which refusals are the parameter validator's rather than this service's own,
/// and the one choice this service makes about how that validator's envelope is
/// stated.
mod parameter_validation;

/// The one modelled bound this service refuses no request for: the retention a
/// record-expiration configuration names, read capped at the model's own maximum
/// so the operation — not a request-validation refusal — decides such a call.
mod record_expiration_retention;

/// The one client-input error this service models: `BadRequestException`,
/// `errorKind` client, `httpError` 400, one `message` member
/// (`smithy_query(query="shape", shape="BadRequestException")`). It is a
/// service-common error, so EVERY operation declares it — and it is the code of
/// every refusal the SERVICE makes, including the ones it makes at the boundary
/// with a fixed sentence of its own. A refusal of a parameter's shape that the
/// service words no sentence for is not one of them: that one is the parameter
/// validator's, and `service_common::validation_message` is what states it.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The HTTP status the model gives that error (`@httpError(400)`) — the status
/// the framework's own refusal renderer answers the class with too, so stating
/// the refusal here changes the error code and the sentence and nothing else.
const BAD_REQUEST_STATUS: u16 = 400;

/// How this service answers a request one of its layers in FRONT of dispatch
/// refuses: the one client-input error its model declares, carrying `message`,
/// rendered by the service's own codec.
///
/// Every such layer ([`request_layer`], [`maintenance_put`]) answers in this ONE
/// envelope, so what a caller is told differs between them in the sentence alone
/// — and neither can drift to an error code this model does not carry.
fn bad_request_response(svc: &dyn Service, req: &RtRequest, message: String) -> RtResponse {
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| svc.definition().protocol());
    let refusal = BadRequestException {
        message: Some(message),
    };
    protocol
        .serialize_error(
            &refusal,
            &SCHEMA_BAD_REQUEST_EXCEPTION,
            BAD_REQUEST_CODE,
            BAD_REQUEST_STATUS,
        )
        .unwrap_or_else(|error| svc.definition().framework_error_response(error, req))
}

/// `TableBucketName`'s character set and length
/// (`smithy_query(query="shape", shape="TableBucketName")`).
const TABLE_BUCKET_NAME_PATTERN: &str = "^[0-9a-z-]*$";
const TABLE_BUCKET_NAME_MIN: i64 = 3;
const TABLE_BUCKET_NAME_MAX: i64 = 63;

/// `TableName`'s character set and length
/// (`smithy_query(query="shape", shape="TableName")`). The two name shapes of
/// this model differ in BOTH, which is what tells a refused `name` member
/// apart: `CreateTableBucket` names a table bucket with it and the table
/// operations name a table.
const TABLE_NAME_PATTERN: &str = "^[0-9a-z_]*$";
const TABLE_NAME_MIN: i64 = 1;
const TABLE_NAME_MAX: i64 = 255;

/// The one part a namespace has: `CreateNamespaceRequest.namespace` is a
/// `NamespaceList` with `@length(min: 1, max: 1)`
/// (`smithy_query(query="operation", operation="CreateNamespace")`). No other
/// member of this model named `namespace` is bounded at one — the namespace
/// NAME shape is bounded at 255 — so a list longer than this is the only
/// refusal that reaches the multipart clause below.
const NAMESPACE_PARTS_MAX: i64 = 1;

/// The two values a configuration's `status` member takes: `MaintenanceStatus`
/// and `TableRecordExpirationStatus` are each an enum of exactly these
/// (`smithy_query(query="shape",
/// shape="MaintenanceStatus,TableRecordExpirationStatus")`), and they are the
/// two the sentence for that class names — so the clause below reads the class
/// off the SET the member's enum declares rather than off the operation that
/// carried it.
const STATUS_VALUES: [&str; 2] = ["enabled", "disabled"];

/// The members a MAINTENANCE configuration of this service COUNTS in: the
/// members the model targets at `PositiveInteger` (`@range(1, 2147483647)`)
/// inside a maintenance task's settings (`smithy_query(query="shape",
/// shape="IcebergSnapshotManagementSettings,
/// IcebergUnreferencedFileRemovalSettings")` reports the target on each), and so
/// the members the positive-integer sentence is stated for.
///
/// `targetFileSizeMB` is the one other member of those shapes, and it is
/// deliberately absent: S3 Tables compacts towards a narrower window of its own,
/// and the sentence for a size outside it states that window, so the clause
/// above this list is what states it. A bounded member of any OTHER shape (a
/// page size, which the model bounds at a thousand) is not one of these either.
///
/// The retention a RECORD EXPIRATION configuration names
/// (`TableRecordExpirationSettings.days`, bounded at the same range) is absent
/// too, and for a different reason: S3 Tables states no sentence of its own for
/// it. A retention below the minimum is the parameter validator's refusal, in
/// that validator's envelope ([`parameter_validation`]), and a retention above
/// the maximum is no refusal at all ([`record_expiration_retention`]).
const POSITIVE_INTEGER_MEMBERS: [&str; 4] = [
    "minSnapshotsToKeep",
    "maxSnapshotAgeHours",
    "unreferencedDays",
    "nonCurrentDays",
];

/// The input member a violation path names: its FIRST segment, so a value
/// refused inside a collection (`namespace.1.member`) is named by the input
/// member that carried it rather than by the collection step.
fn violated_member(path: &str) -> &str {
    path.split('.').next().unwrap_or(path)
}

/// The member a violation path ENDS at: the leaf that actually carried the
/// value, so a member of a structure the request nests (an encryption
/// configuration's algorithm, carried by three operations under three different
/// input members) is named by itself rather than by whatever carried it.
fn violated_leaf(path: &str) -> &str {
    path.split('.').next_back().unwrap_or(path)
}

/// The fixed sentence this service states a refused member of an
/// `encryptionConfiguration` with, or `None` when the violation is of neither of
/// that structure's two members.
///
/// Read off the LEAF member, because the structure is carried by a different
/// input member in each operation that takes one (`CreateTableBucket` and
/// `CreateTable` configure encryption as part of a create, and
/// `PutTableBucketEncryption` sets it on a bucket that exists) while the member
/// that failed, and the reason it failed, are the same in all of them.
fn encryption_sentence(violation: &Violation) -> Option<&'static str> {
    match (violated_leaf(&violation.path), &violation.kind) {
        // The algorithm names the encryption the data is written under: the
        // model targets this member at the `SSEAlgorithm` enum, so a value
        // outside that set names an encryption this service has no keys for.
        ("sseAlgorithm", ViolationKind::EnumValue(_)) => {
            Some(crate::state::INVALID_SSE_ALGORITHM_MESSAGE)
        }
        // The KMS key is declared with both a length and a pattern, and either
        // way the value cannot be a KMS key ARN at all.
        (
            "kmsKeyArn",
            ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_),
        ) => Some(crate::state::INVALID_KMS_KEY_ARN_MESSAGE),
        _ => None,
    }
}

/// The fixed sentence this service states a refused member of a CONFIGURATION's
/// value with, or `None` when the violation is of another member or of another
/// constraint.
///
/// Read off the LEAF member, because each of these sits inside a different
/// structure in each operation that configures something — a table's
/// maintenance, a table bucket's maintenance, a table's record expiration —
/// while the member that failed, and the reason it failed, are the same in all
/// of them.
fn configuration_sentence(violation: &Violation) -> Option<&'static str> {
    match (violated_leaf(&violation.path), &violation.kind) {
        // Whether the task runs: the two enums a `status` member of this model
        // targets declare the same pair of values, and the sentence names that
        // pair, so the class is read off the SET rather than off the operation
        // that carried the member.
        ("status", ViolationKind::EnumValue(allowed)) if allowed.as_slice() == STATUS_VALUES => {
            Some(crate::state::INVALID_CONFIGURATION_STATUS_MESSAGE)
        }
        // The strategy names how compaction rewrites a table's files: the model
        // targets this member at the `IcebergCompactionStrategy` enum, so a
        // value outside that set names no compaction this service runs.
        ("strategy", ViolationKind::EnumValue(_)) => {
            Some(crate::state::INVALID_COMPACTION_STRATEGY_MESSAGE)
        }
        _ => None,
    }
}

/// The fixed sentence this service states `violation` with, or `None` when the
/// member it refused is not one of the classes this service words itself.
///
/// The class is read off the MEMBER and the CONSTRAINT the value failed — the
/// two facts a violation carries — never off the operation or the rejected
/// value, so one arm covers every operation that takes the member.
fn fixed_sentence(violation: &Violation) -> Option<&'static str> {
    if let Some(sentence) = encryption_sentence(violation) {
        return Some(sentence);
    }
    if let Some(sentence) = configuration_sentence(violation) {
        return Some(sentence);
    }
    let member = violated_member(&violation.path);
    match (member, &violation.kind) {
        // `TableBucketARN` declares a pattern and no length, so its only
        // value-rejecting constraint is the pattern.
        ("tableBucketARN", ViolationKind::Pattern(_)) => {
            Some(crate::state::INVALID_TABLE_BUCKET_ARN_MESSAGE)
        }
        // `ResourceArn` declares both, and either way the value cannot be a
        // resource ARN at all.
        (
            "resourceArn",
            ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_),
        ) => Some(crate::state::INVALID_RESOURCE_ARN_MESSAGE),
        // The namespace LIST's own bound: more parts than a namespace has.
        ("namespace", ViolationKind::MaxLength(NAMESPACE_PARTS_MAX)) => {
            Some(crate::state::MULTIPART_NAMESPACE_MESSAGE)
        }
        // A namespace NAME — the list's element, and the label the namespace and
        // table operations address one by — outside its character set or length.
        (
            "namespace" | "newNamespaceName",
            ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_),
        ) => Some(crate::state::INVALID_NAMESPACE_NAME_MESSAGE),
        ("name", ViolationKind::Pattern(pattern)) if pattern == TABLE_BUCKET_NAME_PATTERN => {
            Some(crate::state::INVALID_TABLE_BUCKET_NAME_MESSAGE)
        }
        (
            "name",
            ViolationKind::MinLength(TABLE_BUCKET_NAME_MIN)
            | ViolationKind::MaxLength(TABLE_BUCKET_NAME_MAX),
        ) => Some(crate::state::INVALID_TABLE_BUCKET_NAME_MESSAGE),
        ("name" | "newName", ViolationKind::Pattern(pattern)) if pattern == TABLE_NAME_PATTERN => {
            Some(crate::state::INVALID_TABLE_NAME_MESSAGE)
        }
        (
            "name" | "newName",
            ViolationKind::MinLength(TABLE_NAME_MIN) | ViolationKind::MaxLength(TABLE_NAME_MAX),
        ) => Some(crate::state::INVALID_TABLE_NAME_MESSAGE),
        // The open table format a table is created in: the model targets this
        // member at the `OpenTableFormat` enum and makes it `@required`, so a
        // request either names a format the service has or names none — and
        // both are the one failure this sentence states, since neither gives
        // the create a format to work in.
        (
            "format",
            ViolationKind::EnumValue(_) | ViolationKind::MissingRequiredMember,
        ) => Some(crate::state::INVALID_OPEN_TABLE_FORMAT_MESSAGE),
        // Anything the model rules out INSIDE the metadata document — a sort
        // direction or null order outside its enumerated set, a value outside a
        // declared bound — makes the document one this service will not create a
        // table from, which is the class its metadata sentence states. The
        // violation's first path segment is the input member that carried the
        // value, so one arm covers every depth of the document.
        ("metadata", _) => Some(crate::state::INVALID_METADATA_MESSAGE),
        _ => None,
    }
}

/// The sentence this service states `violation` with, or `None` when the member
/// it refused is not one of the classes this service words itself.
///
/// Most of those sentences are fixed text ([`fixed_sentence`]); the one below is
/// built instead, because it quotes a window of this SERVICE's own, and building
/// it from the constants that window is enforced from is what keeps the sentence
/// and the refusal in step.
fn stated_sentence(violation: &Violation) -> Option<String> {
    match (violated_leaf(&violation.path), &violation.kind) {
        // The file size compaction rewrites a table's files towards: the window
        // S3 Tables compacts towards is narrower than the model's own bound on
        // the member, and a size outside EITHER is one this service does not
        // compact towards — so the boundary states the sentence the put states
        // for the rest of that class.
        ("targetFileSizeMB", ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)) => {
            Some(crate::state::invalid_compaction_file_size_message())
        }
        // Every other member a MAINTENANCE configuration counts in: the model
        // bounds each at the positive-integer range, and this service states a
        // value outside that range by naming the member it was carried in. The
        // retention a record-expiration configuration names is not one of them
        // (see [`POSITIVE_INTEGER_MEMBERS`]), so it falls through to the
        // validator's envelope below.
        (member, ViolationKind::MinValue(_) | ViolationKind::MaxValue(_))
            if POSITIVE_INTEGER_MEMBERS.contains(&member) =>
        {
            Some(crate::state::invalid_positive_integer_message(member))
        }
        _ => fixed_sentence(violation).map(str::to_string),
    }
}

/// How this service answers a request the boundary refused for breaking a
/// modelled constraint.
///
/// Which LAYER refused it is what a caller is told, and the member class is what
/// decides the layer:
///
/// * a class this service validates itself — the names a create gives a
///   resource, the ARNs an operation is addressed by, the encryption, format and
///   metadata members it will not act on, and the members of a configuration's
///   value (its status, its compaction strategy, the numbers a maintenance task
///   counts in) — is
///   the service's own refusal: its modelled `BadRequestException`, worded with
///   the one sentence it states that class with;
/// * every other class is refused by the parameter validator in front of the
///   service, which answers under its own code and envelope — the code is not
///   one of this model's shapes precisely because the refusal is not the
///   service's. Nothing of that envelope is worded in this crate: see
///   [`parameter_validation`].
///
/// `None` when there is nothing to state, which leaves the framework's wording.
fn constraint_refusal(refusal: &Refusal, violations: &[Violation]) -> Option<Refused> {
    match violations.iter().find_map(stated_sentence) {
        Some(sentence) => Some(Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: sentence,
        }),
        None => parameter_validation::refusal(refusal),
    }
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `$table`, each
/// wrapped in [`authorization::Authorized`] (the policies in force on the caller
/// and on the resource the request addresses decide the call) and keyed by its
/// operation shape name — the routing key `ServerHttpProtocol::routing_key`
/// resolves and the generated table is keyed by.
macro_rules! gate_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(authorization::Authorized::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                    ::std::sync::Arc::clone(&$engine),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// The `com-amazonaws-s3tables-s3tablebuckets` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same operations the generated table holds, each wrapped in
    /// [`authorization::Authorized`] so the policies in force on the caller and
    /// on the table bucket a request addresses decide the call before the
    /// handler runs. Keyed by the routing key the protocol resolves — the key
    /// the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
    gated: smithy_server_runtime::OperationRoutingTable,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: record_expiration_retention::CappedRetentionRestJson,
    /// Answers a request whose body this service cannot read into the
    /// operation's modelled input (see [`request_layer`]). Held on the service
    /// because [`Service::override_operation`] hands back a borrow of one of its
    /// own operations.
    unreadable_body: request_layer::UnreadableBody<Eng>,
    /// Serves the Iceberg REST catalog's table endpoint (see [`iceberg_rest`]).
    /// Held on the service for the same reason as the layer above: the seam that
    /// selects it hands back a borrow of one of the service's own operations.
    iceberg_catalog: iceberg_rest::IcebergCatalog<Eng>,
    /// Serves a request whose only unmet constraint is the name FORMAT of a
    /// member the operation LOOKS UP (see [`name_lookups`]). Held on the service
    /// for the same reason as the two layers above.
    name_lookups: name_lookups::NameLookups<Eng>,
    /// Refuses a maintenance put for what its path or its body states (see
    /// [`maintenance_put`]). Held on the service for the same reason as the
    /// layers above.
    maintenance_put: maintenance_put::MaintenancePutRefusal<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut gated = smithy_server_runtime::OperationRoutingTable::new();
        // EVERY operation this crate serves (one per file in `src/ops/`), because
        // the decision is the SERVICE's and not one operation's: every operation
        // of this service documents the permission it takes, and a bucket policy
        // governs every call that acts on that bucket or on anything inside it.
        // An operation whose input addresses no resource simply has nothing to be
        // decided against, which the gate reads off that operation's own input.
        // An operation added later is added here as well, or it is served with
        // the policies in force ignored; the test below fails when one is
        // missing.
        gate_operations!(
            gated,
            engine,
            complete_table_migration::CompleteTableMigration,
            create_namespace::CreateNamespace,
            create_table::CreateTable,
            create_table_bucket::CreateTableBucket,
            delete_namespace::DeleteNamespace,
            delete_table::DeleteTable,
            delete_table_bucket::DeleteTableBucket,
            delete_table_bucket_encryption::DeleteTableBucketEncryption,
            delete_table_bucket_metrics_configuration::DeleteTableBucketMetricsConfiguration,
            delete_table_bucket_policy::DeleteTableBucketPolicy,
            delete_table_bucket_replication::DeleteTableBucketReplication,
            delete_table_policy::DeleteTablePolicy,
            delete_table_replication::DeleteTableReplication,
            disassociate_table_replication::DisassociateTableReplication,
            get_namespace::GetNamespace,
            get_table::GetTable,
            get_table_bucket::GetTableBucket,
            get_table_bucket_encryption::GetTableBucketEncryption,
            get_table_bucket_logging::GetTableBucketLogging,
            get_table_bucket_maintenance_configuration::GetTableBucketMaintenanceConfiguration,
            get_table_bucket_metrics_configuration::GetTableBucketMetricsConfiguration,
            get_table_bucket_policy::GetTableBucketPolicy,
            get_table_bucket_replication::GetTableBucketReplication,
            get_table_bucket_storage_class::GetTableBucketStorageClass,
            get_table_encryption::GetTableEncryption,
            get_table_maintenance_configuration::GetTableMaintenanceConfiguration,
            get_table_maintenance_job_status::GetTableMaintenanceJobStatus,
            get_table_metadata_location::GetTableMetadataLocation,
            get_table_migration::GetTableMigration,
            get_table_policy::GetTablePolicy,
            get_table_record_expiration_configuration::GetTableRecordExpirationConfiguration,
            get_table_record_expiration_job_status::GetTableRecordExpirationJobStatus,
            get_table_replication::GetTableReplication,
            get_table_replication_status::GetTableReplicationStatus,
            get_table_storage_class::GetTableStorageClass,
            list_namespaces::ListNamespaces,
            list_table_buckets::ListTableBuckets,
            list_table_migrations::ListTableMigrations,
            list_tables::ListTables,
            list_tags_for_resource::ListTagsForResource,
            put_table_bucket_encryption::PutTableBucketEncryption,
            put_table_bucket_logging::PutTableBucketLogging,
            put_table_bucket_maintenance_configuration::PutTableBucketMaintenanceConfiguration,
            put_table_bucket_metrics_configuration::PutTableBucketMetricsConfiguration,
            put_table_bucket_policy::PutTableBucketPolicy,
            put_table_bucket_replication::PutTableBucketReplication,
            put_table_bucket_storage_class::PutTableBucketStorageClass,
            put_table_maintenance_configuration::PutTableMaintenanceConfiguration,
            put_table_policy::PutTablePolicy,
            put_table_record_expiration_configuration::PutTableRecordExpirationConfiguration,
            put_table_replication::PutTableReplication,
            rename_table::RenameTable,
            start_table_migration::StartTableMigration,
            sync_table_migration::SyncTableMigration,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_table_metadata_location::UpdateTableMetadataLocation,
            update_table_migration::UpdateTableMigration,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            gated,
            protocol: record_expiration_retention::CappedRetentionRestJson::new(),
            unreadable_body: request_layer::UnreadableBody::new(),
            iceberg_catalog: iceberg_rest::IcebergCatalog::new(Arc::clone(&engine)),
            name_lookups: name_lookups::NameLookups::new(engine),
            maintenance_put: maintenance_put::MaintenancePutRefusal::new(),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A body this service cannot read into the operation's modelled input is
        // refused HERE, in front of the generated walk, because that failure
        // happens in the codec: the value never becomes a value at all, so no
        // handler runs and no modelled error is reached. The framework would
        // state it in its codec's own words under an error code this model does
        // not declare; this service states its own invalid-input sentence under
        // the one client-input error it models. Everything else — a request whose
        // body READS — resolves through the generated routing table, which is
        // what `None` selects.
        //
        // WHICH operation the request names is resolved through the generated
        // entry's own protocol, because that is the one holding the model's route
        // table; the overridden protocol below carries none (a protocol's routes
        // are read by `routing_key` alone) and is what READS the body, exactly as
        // the generated walk reads it.
        // The Iceberg REST catalog's table endpoint is addressed by a path the
        // model binds no operation to, so the generated routing table cannot
        // name it and the request would be answered as an operation this
        // emulator does not implement. It IS served — by this service, out of
        // the same table records the control plane reads and writes — so a
        // request that addresses it resolves to the catalog operation.
        if iceberg_rest::serves(req.method(), req.uri()) {
            return Some(&self.iceberg_catalog);
        }
        let routing = self.entry.protocol();
        let codec: &dyn ServerHttpProtocol = &self.protocol;
        if let Ok(key) = routing.routing_key(req) {
            // A maintenance put whose path addresses a task the resource does
            // not have, or whose body carries no configuration value at all, is
            // refused with the sentence this service states that class with, in
            // front of the layer below: the settings a maintenance
            // configuration carries are a union whose members are the tasks
            // themselves, so a request naming an unknown task usually carries
            // that task's settings too — a body this service cannot read into
            // the input at all — and the caller would be told about its body
            // where the path is what named no task.
            if maintenance_put::refuses::<Eng>(key, codec, req) {
                return Some(&self.maintenance_put);
            }
            if request_layer::body_is_unreadable::<Eng>(key, codec, req) {
                return Some(&self.unreadable_body);
            }
            // A name this operation ADDRESSES a resource by is a lookup key, not
            // a name this service is being asked to create: the model's
            // character set and length for it gate the create paths, and a
            // lookup resolves the string against state and answers the modelled
            // not-found for one nothing is stored under. The framework validates
            // every modelled constraint before a handler runs, so that one class
            // of refusal is taken off the lookup paths HERE, in front of the
            // generated walk — every other refusal, including a create's own
            // name, is left to it.
            if name_lookups::resolves_names::<Eng>(key, codec, req) {
                return Some(&self.name_lookups);
            }
            // Everything the model names is served out of the gated table: the
            // SAME operation the generated table holds, behind the
            // authorization decision (see [`authorization`]). The key is the
            // one that table's own protocol resolved, so this is no re-route —
            // and a key no handler serves resolves to `None` here exactly as it
            // does in the generated table, leaving the router's routing-error
            // path unchanged.
            return self.gated.get(key);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. A member class the service validates ITSELF — a
        // name, a namespace, an ARN, a format, an encryption configuration — is
        // its own `BadRequestException` carrying the fixed sentence it states
        // that class with; every other class is the parameter validator's
        // refusal and is answered under that validator's code and envelope.
        // BOTH the code and the message are chosen together, which is what
        // [`constraint_refusal`] builds.
        //
        // An authentication refusal, and any refusal kind added later, keeps the
        // framework's wording.
        match refusal {
            Refusal::Invalid(violations) => constraint_refusal(refusal, violations),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every timestamp member of this model declares
        // `@timestampFormat("date-time")` — a table's and a table bucket's
        // `createdAt`/`modifiedAt`, a namespace's `createdAt`, a maintenance or
        // expiration job's `lastRunTimestamp`, a replicated update's `timestamp` — so
        // every instant this service reports is an RFC 3339 string rather than the
        // protocol's default epoch-seconds number. The one other difference from
        // the model's own `restJson1` is on the way IN: the retention a
        // record-expiration configuration names is read capped at the model's
        // maximum rather than refused above it (see
        // [`record_expiration_retention`]).
        Some(&self.protocol)
    }
}

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above a wire test cannot see: that the
    //! hand-maintained `gate_operations!` list is the WHOLE surface this crate
    //! serves.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json"
    );

    /// The namespace and the table the catalog cases below are addressed for.
    const NAMESPACE: &str = "ns";
    const TABLE: &str = "t";

    /// Every operation name the MODEL declares, read from the model this crate
    /// was generated from rather than from a hand-kept list — so a rule stated
    /// over "every operation of this service" is checked over the surface the
    /// model publishes.
    fn modelled_operations() -> Vec<String> {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let operations: Vec<String> = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes")
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next().map(str::to_string))
            .collect();
        assert!(
            operations.len() >= 58,
            "case \"the model's surface\": expected this service's operations, got {operations:?}"
        );
        operations
    }

    /// RULE applied to the whole surface: every operation the generated routing
    /// table serves is also served behind the authorization gate, under the same
    /// key that table is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an
    /// operation that gains a handler and not an entry in `gate_operations!`
    /// fails here instead of silently serving a call the policies in force
    /// refuse — and a key spelled differently from the generated table's
    /// fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_operation_this_crate_serves_is_served_behind_the_authorization_gate() {
        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        let mut served = 0usize;
        for operation in modelled_operations() {
            let operation = operation.as_str();
            if service.entry.routing_table().get(operation).is_none() {
                continue;
            }
            served += 1;
            assert!(
                service.gated.get(operation).is_some(),
                "case {operation:?}: every operation this crate serves is served behind the \
                 authorization gate; this one is missing from gate_operations!, so a request it \
                 serves is answered without the authorization decision"
            );
        }
        assert_eq!(
            served, 58,
            "case \"the served surface\": expected the 58 operations this crate has a handler \
             for, got {served}"
        );
    }

    /// RULE applied to the OTHER seam `override_operation` serves: the Iceberg
    /// REST catalog is not an operation of this model, so no entry of
    /// `gate_operations!` can wrap it in [`authorization::Authorized`] — the
    /// authorization decision is taken inside the catalog instead, against the
    /// control-plane action each of its endpoints takes
    /// ([`iceberg_rest::decided_as`]).
    ///
    /// Every request the catalog CLAIMS must therefore name an action and a
    /// resource for that decision, or it would be served with the policies in
    /// force ignored. The one exception is a configuration request naming no
    /// warehouse: it addresses no resource of this service at all, and the
    /// catalog refuses it for naming none.
    #[test]
    fn every_request_the_iceberg_catalog_claims_is_decided_against_a_resource() {
        // The warehouse prefix travels as one path segment, so a value with no
        // separator in it stands in for the table bucket ARN a client addresses
        // the catalog by; what this asserts is which action and which resource
        // the decision is taken on, not how the segment is spelled.
        let warehouse = "warehouse";
        let table = || Some((NAMESPACE.to_string(), TABLE.to_string()));
        // (case, method, uri, the decision the endpoint must be taken under)
        let rows: Vec<(&str, &str, String, iceberg_rest::Decided)> = vec![
            (
                "the warehouse's configuration",
                "GET",
                format!("/iceberg/v1/config?warehouse={warehouse}"),
                iceberg_rest::Decided {
                    operation: "GetTableBucket",
                    table_bucket_arn: warehouse.to_string(),
                    table: None,
                },
            ),
            (
                "the warehouse's namespaces",
                "GET",
                format!("/iceberg/v1/{warehouse}/namespaces"),
                iceberg_rest::Decided {
                    operation: "ListNamespaces",
                    table_bucket_arn: warehouse.to_string(),
                    table: None,
                },
            ),
            (
                "one namespace",
                "GET",
                format!("/iceberg/v1/{warehouse}/namespaces/{NAMESPACE}"),
                iceberg_rest::Decided {
                    operation: "GetNamespace",
                    table_bucket_arn: warehouse.to_string(),
                    table: None,
                },
            ),
            (
                "whether a namespace exists",
                "HEAD",
                format!("/iceberg/v1/{warehouse}/namespaces/{NAMESPACE}"),
                iceberg_rest::Decided {
                    operation: "GetNamespace",
                    table_bucket_arn: warehouse.to_string(),
                    table: None,
                },
            ),
            (
                "a namespace's tables",
                "GET",
                format!("/iceberg/v1/{warehouse}/namespaces/{NAMESPACE}/tables"),
                iceberg_rest::Decided {
                    operation: "ListTables",
                    table_bucket_arn: warehouse.to_string(),
                    table: None,
                },
            ),
            (
                "a table's metadata",
                "GET",
                format!("/iceberg/v1/{warehouse}/namespaces/{NAMESPACE}/tables/{TABLE}"),
                iceberg_rest::Decided {
                    operation: "GetTableMetadataLocation",
                    table_bucket_arn: warehouse.to_string(),
                    table: table(),
                },
            ),
            (
                "whether a table exists",
                "HEAD",
                format!("/iceberg/v1/{warehouse}/namespaces/{NAMESPACE}/tables/{TABLE}"),
                iceberg_rest::Decided {
                    operation: "GetTable",
                    table_bucket_arn: warehouse.to_string(),
                    table: table(),
                },
            ),
            (
                "a commit to a table",
                "POST",
                format!("/iceberg/v1/{warehouse}/namespaces/{NAMESPACE}/tables/{TABLE}"),
                iceberg_rest::Decided {
                    operation: "UpdateTableMetadataLocation",
                    table_bucket_arn: warehouse.to_string(),
                    table: table(),
                },
            ),
            (
                "a table method the catalog does not serve",
                "DELETE",
                format!("/iceberg/v1/{warehouse}/namespaces/{NAMESPACE}/tables/{TABLE}"),
                iceberg_rest::Decided {
                    operation: "GetTable",
                    table_bucket_arn: warehouse.to_string(),
                    table: table(),
                },
            ),
        ];
        for (case, method, uri, expected) in rows {
            assert!(
                iceberg_rest::serves(method, &uri),
                "case {case:?}: the catalog must claim {method} {uri}"
            );
            let decided = iceberg_rest::decided_as(method, &uri).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: every request the catalog claims names a resource to decide \
                     against; {method} {uri} names none, so it would be served with the policies \
                     in force ignored"
                )
            });
            assert_eq!(
                decided, expected,
                "case {case:?}: expected the decision to be taken on that action and resource"
            );
        }

        // The one request that names no resource: a configuration request naming
        // no warehouse, which the catalog refuses for naming none.
        let no_warehouse = "/iceberg/v1/config";
        assert!(
            iceberg_rest::serves("GET", no_warehouse),
            "case \"a configuration naming no warehouse\": the catalog claims it"
        );
        assert!(
            iceberg_rest::decided_as("GET", no_warehouse).is_none(),
            "case \"a configuration naming no warehouse\": it addresses no resource of this \
             service"
        );
    }
}
