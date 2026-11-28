// SPDX-License-Identifier: CC0-1.0

//! The Iceberg REST catalog this service serves beside its control plane.
//!
//! # Not a modelled operation
//!
//! S3 Tables serves an Apache Iceberg REST catalog at `/iceberg/v1` alongside
//! the control plane, and that catalog is how every data-plane consumer of a
//! table (Spark, Trino, PyIceberg, Athena) reads the table: it loads the table's
//! metadata document to learn the table's columns and their field ids, where the
//! table's files live and which UUID identifies it, and it commits a change to
//! the table's metadata back through the same route. None of it is in this
//! service's Smithy model — no modelled operation binds `/iceberg/v1/*` — so
//! nothing in the generated routing table can name these requests, and they are
//! reached through the seam an emulated data plane uses:
//!
//! * [`crate::service::EmulatedService::override_operation`] resolves a request
//!   addressed beneath the catalog's path space to [`IcebergCatalog`], which is
//!   an author-written operation rather than a modelled one;
//! * that operation declares SigV4 ([`ErasedOperation::auth_schemes`]), so the
//!   router verifies the request's signature before it runs — the catalog is
//!   signed traffic, exactly as the control plane is.
//!
//! # What is served
//!
//! The DISCOVERY endpoints a client finds the warehouse, its namespaces and a
//! namespace's tables through ([`discovery`]), and the table endpoint of the
//! catalog,
//! `/iceberg/v1/{prefix}/namespaces/{namespace}/tables/{table}`, where `prefix`
//! is the table bucket's ARN (the warehouse the catalog is addressed for):
//!
//! * `GET` loads the table: the table's stored metadata document and the
//!   location of the metadata file it is written to;
//! * `HEAD` answers whether the table exists;
//! * `POST` commits updates to the table's metadata, under the requirements the
//!   commit states, and answers with the committed document.
//!
//! Every answer is projected from the SAME records the control-plane handlers
//! read and write ([`crate::state::Table`] and its
//! [`crate::state::IcebergTableMetadata`]), so the catalog and the control plane
//! cannot disagree about a table: a schema a `CreateTable` declared is the
//! schema a load reports, and a sort order a commit adds is the sort order
//! `PutTableMaintenanceConfiguration` judges a compaction strategy against.
//!
//! # The envelope
//!
//! A refusal is answered in the envelope of the API the request addressed — the
//! Iceberg REST catalog's own error document (`error.message`, `error.type`,
//! `error.code`) — never in this service's restJson1 error shapes, which belong
//! to the modelled operations.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::{Map, Value};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The catalog's discovery endpoints: the warehouse's configuration, the
/// namespaces in it and the tables in one namespace.
mod discovery;

use crate::state::{
    IcebergPartitionFieldRecord, IcebergPartitionSpecRecord, IcebergSortFieldRecord,
    IcebergSortOrderRecord, MetadataRequirement, MetadataUpdate, Table,
};

/// The auth schemes this catalog is served under: SigV4, the one scheme this
/// service's model declares (`smithy_query(query="service_info")`). Stated here
/// because an author-written operation's schemes are what the router
/// authenticates a request against when the model names none for its route — so
/// the catalog is verified traffic rather than an unsigned side door.
const CATALOG_AUTH_SCHEMES: &[&str] = &["aws.auth#sigv4"];

/// The path space the Iceberg REST catalog is served under.
const CATALOG_PREFIX: &str = "/iceberg/v1/";

/// The path segment that opens a namespace's path space inside the catalog.
const NAMESPACES_SEGMENT: &str = "namespaces";

/// The path segment that opens a namespace's tables.
const TABLES_SEGMENT: &str = "tables";

/// The media type the catalog's documents travel as.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The separator the Iceberg REST catalog spells a multi-level namespace with
/// (the unit separator, so a namespace level may itself contain a dot). This
/// service's namespaces are single-level — `CreateNamespaceRequest.namespace` is
/// bounded at one element — so a request naming more levels names no namespace
/// this catalog holds.
const NAMESPACE_SEPARATOR: char = '\u{1f}';

/// The operation name the audit record of a commit carries. A commit changes
/// this service's state, so it leaves the record every mutating handler leaves;
/// the name is the catalog endpoint it was made through, since no modelled
/// operation names it.
const COMMIT_RECORD: &str = "IcebergUpdateTable";

/// The most requirements or updates this catalog reads from one commit, and the
/// most fields it reads from one schema, partition spec or sort order inside it.
///
/// CHOSEN, not an Iceberg-published figure: a commit is a caller-sized document
/// and what it asks for is written to this service's state, so the work and the
/// state one commit can grow are BOUNDED before either happens. A commit past
/// the bound is refused as the client request error it is.
const ENTRIES_PER_COMMIT: usize = 1_000;

/// What the catalog serves on a table it holds no metadata document for: the
/// table exists in the control plane, but no metadata file was ever written for
/// it (its create described none and nothing has committed since), so there is
/// no document to load.
const NO_TABLE_METADATA: &str = "Table has no metadata location";

/// One request to the catalog: the table endpoint it addresses and what it asks
/// of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogRequest {
    /// The warehouse the catalog is addressed for: a table bucket's ARN.
    pub(crate) table_bucket_arn: String,
    pub(crate) namespace: String,
    pub(crate) table: String,
    pub(crate) endpoint: TableEndpoint,
}

/// What a request asks of a table endpoint of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableEndpoint {
    /// Load the table's metadata.
    Load,
    /// Answer whether the table exists.
    Exists,
    /// Commit updates to the table's metadata.
    Commit,
    /// A method of the table endpoint this catalog does not serve — the drop of
    /// a table, say. The endpoint IS this service's, so the request is answered
    /// as unserved rather than as a table that does not exist.
    Unserved,
}

impl TableEndpoint {
    /// The endpoint `method` addresses.
    fn of(method: &str) -> Self {
        match method {
            "GET" => Self::Load,
            "HEAD" => Self::Exists,
            "POST" => Self::Commit,
            _ => Self::Unserved,
        }
    }
}

/// The catalog request `method` and `uri` address, or `None` when the URI is not
/// a table endpoint of the catalog — in which case the request is left to the
/// modelled routing table exactly as it was.
///
/// Every segment is percent-decoded the way the codec that deserializes a path
/// label decodes one (`service_common::percent_decode_path_label`), because a
/// client addresses the warehouse by an ARN, whose `:` and `/` travel encoded.
pub(crate) fn catalog_request_of(method: &str, uri: &str) -> Option<CatalogRequest> {
    // The path component of either URI form — an absolute path, or the whole URL
    // a client composes from the endpoint it was configured with — is read by
    // the workspace's one reader for it, never split again here.
    let path = ::service_common::request_path(uri);
    let segments: Vec<&str> = path.strip_prefix(CATALOG_PREFIX)?.split('/').collect();
    let [prefix, namespaces, namespace, tables, table] = segments.as_slice() else {
        return None;
    };
    if *namespaces != NAMESPACES_SEGMENT || *tables != TABLES_SEGMENT {
        return None;
    }
    let decoded = |segment: &str| ::service_common::percent_decode_path_label(segment);
    let (table_bucket_arn, namespace, table) =
        (decoded(prefix), decoded(namespace), decoded(table));
    if table_bucket_arn.is_empty() || namespace.is_empty() || table.is_empty() {
        return None;
    }
    Some(CatalogRequest {
        table_bucket_arn,
        namespace,
        table,
        endpoint: TableEndpoint::of(method),
    })
}

/// Whether `method` and `uri` address an endpoint of this catalog at all — the
/// one question [`crate::service::EmulatedService::override_operation`] asks,
/// since the model binds no operation to any of these paths.
pub(crate) fn serves(method: &str, uri: &str) -> bool {
    catalog_request_of(method, uri).is_some() || discovery::request_of(method, uri).is_some()
}

/// The Iceberg REST catalog as an operation the router can run: author-written,
/// so it reads the request and writes the response itself rather than through a
/// modelled shape's codec.
///
/// Holds the SAME shared engine the generated entry threads into every
/// operation, so the tables a `CreateTable` stores are the tables this catalog
/// serves.
pub(crate) struct IcebergCatalog<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> IcebergCatalog<Eng> {
    pub(crate) fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for IcebergCatalog<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The AUTHORIZATION decision, taken before anything is read or
            // written — the same decision every modelled operation of this
            // service is served behind ([`super::authorization`]). The catalog is
            // reached through a seam the model binds no operation to, so the
            // gated routing table cannot wrap it: the decision is taken HERE
            // instead, against the control-plane action the endpoint's records
            // belong to and the resource it acts on ([`governed`]), so no
            // endpoint of this data plane is served with the policies in force
            // ignored.
            if let Some(message) = self.authorization_refusal(ctx, req) {
                return error(
                    crate::scaffolded::ACCESS_DENIED_STATUS,
                    crate::scaffolded::ACCESS_DENIED_CODE,
                    &message,
                );
            }
            // The URI resolved to this operation, so it names an endpoint of the
            // catalog: either its table endpoint or one of the discovery
            // endpoints beside it. A URI that names neither is never routed
            // here.
            if let Some(discovery) = discovery::request_of(req.method(), req.uri()) {
                return self.discover(&discovery);
            }
            let Some(request) = catalog_request_of(req.method(), req.uri()) else {
                return error(
                    404,
                    "NoSuchTableException",
                    "Table does not exist: the request names no table of this catalog",
                );
            };
            match request.endpoint {
                TableEndpoint::Load => self.load_table(&request),
                TableEndpoint::Exists => self.table_exists(&request),
                TableEndpoint::Commit => self.commit(req, ctx, &request),
                // The endpoint is served; this METHOD of it is not, so the
                // answer says so rather than reporting the table as absent.
                TableEndpoint::Unserved => error(
                    501,
                    "NotImplementedException",
                    &format!(
                        "This catalog does not serve {method} on a table",
                        method = req.method()
                    ),
                ),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(CATALOG_AUTH_SCHEMES)
    }
}

impl<Eng: Send + Sync + 'static> IcebergCatalog<Eng> {
    /// The sentence this request is refused with when the policies in force on
    /// the caller and on what it addresses do not let it perform the action the
    /// endpoint takes, and `None` when nothing refuses it.
    ///
    /// The three pieces are the modelled gate's ([`super::authorization`]): the
    /// resource the request acts on, the resource-based document in force on the
    /// table bucket holding it, and the shared decision
    /// ([`crate::state::authorization::refusal`]) which words the refusal. The
    /// store is read ONCE, under one guard, nothing in it is written, and the
    /// guard is released before the decision is asked — the decision reads other
    /// services' stores.
    fn authorization_refusal(
        &self,
        ctx: &OperationContext<'_>,
        req: &RtRequest,
    ) -> Option<String> {
        let addressed = governed(req.method(), req.uri())?;
        let (resource, policy) = {
            let state = crate::state::service_state(&*self.engine)?;
            let store = state.lock();
            let bucket = store.table_buckets.get(&addressed.table_bucket_arn);
            // A table's permission is granted ON the table, under the policy of
            // the bucket holding it; a request naming a table this account does
            // not hold is decided against the bucket, which is the only resource
            // such a request names.
            let resource = addressed
                .table
                .as_ref()
                .and_then(|(namespace, name)| {
                    bucket
                        .and_then(|bucket| bucket.namespaces.get(namespace))
                        .and_then(|namespace| namespace.tables.get(name))
                        .map(|table| table.table_arn.clone())
                })
                .unwrap_or_else(|| addressed.table_bucket_arn.clone());
            (resource, bucket.and_then(|bucket| bucket.policy.clone()))
        };
        crate::state::authorization::refusal(
            &*self.engine,
            ctx,
            addressed.operation,
            &resource,
            policy.as_deref(),
        )
    }

    /// Serve `GET`: the table's metadata document and the location of the file
    /// it is written to.
    ///
    /// A REPORT — it reads the stored record and writes nothing — so two loads
    /// in a row answer the same document.
    fn load_table(&self, request: &CatalogRequest) -> RtResponse {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return error(503, "ServiceUnavailableException", STORE_UNAVAILABLE);
        };
        let store = state.lock();
        let loaded = match resolve(&store, request) {
            Err(refusal) => return *refusal,
            Ok(table) => match &table.iceberg {
                None => {
                    return error(
                        404,
                        "NoSuchTableException",
                        &format!("{NO_TABLE_METADATA}: {}", qualified(request)),
                    )
                }
                Some(metadata) => (table.metadata_location.clone(), metadata.document()),
            },
        };
        drop(store);
        let (metadata_location, document) = loaded;
        let mut members = Map::new();
        if let Some(location) = metadata_location {
            members.insert("metadata-location".to_string(), Value::from(location));
        }
        members.insert("metadata".to_string(), document);
        members.insert("config".to_string(), Value::Object(Map::new()));
        json(200, &Value::Object(members))
    }

    /// Serve `HEAD`: whether the table exists as a table of this catalog, which
    /// the Iceberg REST catalog answers with a status and no body.
    ///
    /// The same fact a load reports: a table the control plane holds but has no
    /// metadata document for is one this catalog cannot serve, so both methods
    /// answer that it is not here rather than contradicting each other.
    fn table_exists(&self, request: &CatalogRequest) -> RtResponse {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return error(503, "ServiceUnavailableException", STORE_UNAVAILABLE);
        };
        let store = state.lock();
        let found = resolve(&store, request).is_ok_and(|table| table.iceberg.is_some());
        drop(store);
        if found {
            empty(204)
        } else {
            empty(404)
        }
    }

    /// Serve `POST`: commit the updates the request states to the table's
    /// metadata.
    ///
    /// The commit is all of its updates or none of them: the requirements are
    /// checked against the stored metadata, the updates are applied to a copy of
    /// it ([`IcebergTableMetadata::committed`]), and only a copy that was built
    /// whole is stored — under a new metadata file and a new version token, so a
    /// caller can tell the table changed.
    fn commit(
        &self,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
        request: &CatalogRequest,
    ) -> RtResponse {
        let Some(body) = req.body().bytes() else {
            return error(500, "InternalServerErrorException", UNREADABLE_BODY);
        };
        // A commit is a JSON OBJECT carrying the requirements it states and the
        // updates it asks for; a body that is not one states no commit.
        let document: Value = match ::serde_json::from_slice::<Value>(body) {
            Ok(document) if document.is_object() => document,
            _ => return error(400, "BadRequestException", MALFORMED_COMMIT),
        };
        let requirements = match requirements_of(&document) {
            Ok(requirements) => requirements,
            Err(refusal) => return *refusal,
        };
        let updates = match updates_of(&document) {
            Ok(updates) => updates,
            Err(refusal) => return *refusal,
        };

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return error(503, "ServiceUnavailableException", STORE_UNAVAILABLE);
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        let (held, warehouse_location, held_location) = match resolve(&store, request) {
            Err(refusal) => return *refusal,
            Ok(table) => match &table.iceberg {
                None => {
                    return error(
                        404,
                        "NoSuchTableException",
                        &format!("{NO_TABLE_METADATA}: {}", qualified(request)),
                    )
                }
                Some(metadata) => (
                    metadata.clone(),
                    table.warehouse_location.clone(),
                    table.metadata_location.clone(),
                ),
            },
        };
        // A requirement the table does not satisfy is the conflict with a
        // previous write the catalog's `CommitFailedException` names: the
        // caller's view of the table is stale, so it may load the table again
        // and retry.
        for requirement in &requirements {
            if let Err(reason) = held.satisfies(requirement) {
                return error(409, "CommitFailedException", &reason);
            }
        }
        // A commit that asks for no update changes nothing, so it writes no
        // metadata file and mints no version token: it answers the table as it
        // stands, which is what it committed it to.
        if updates.is_empty() {
            let document = held.document();
            drop(store);
            let mut members = Map::new();
            if let Some(location) = held_location {
                members.insert("metadata-location".to_string(), Value::from(location));
            }
            members.insert("metadata".to_string(), document);
            return json(200, &Value::Object(members));
        }
        let mut committed = match held.committed(updates, now) {
            Ok(committed) => committed,
            Err(reason) => return error(400, "BadRequestException", &reason),
        };
        // The description the commit replaces is logged on the new one: the
        // metadata file the table was read from, beside the instant that
        // description was last updated. A table whose create wrote no metadata
        // file has none to log.
        if let Some(previous) = held_location.clone() {
            committed.record_previous_metadata_file(previous, held.last_updated_ms);
        }
        // The commit writes a new metadata file for the table and the table's
        // version token names the new state of it, both minted from the store.
        let metadata_id = store.mint_metadata_id();
        let version_token = store.mint_version_token();
        let metadata_location = crate::state::metadata_location(
            &warehouse_location,
            committed.metadata_version,
            &metadata_id,
        );
        let document = committed.document();
        let account = ::engine::emulator_account_id().to_string();
        let Ok(table) = resolve_mut(&mut store, request) else {
            return error(
                404,
                "NoSuchTableException",
                &format!("Table does not exist: {}", qualified(request)),
            );
        };
        table.iceberg = Some(committed);
        table.metadata_location = Some(metadata_location.clone());
        table.version_token = version_token;
        table.record_modification(account, now);
        let table_arn = table.table_arn.clone();
        // The lock covers the state change only; everything below is owned.
        drop(store);

        // The access record for a state-changing request: who, what and the
        // outcome, in the shape every audit record of this service carries.
        // Metadata only — never a property key or value the commit carried.
        ::tracing::info!(
            target: crate::state::AUDIT_TARGET,
            operation = COMMIT_RECORD,
            caller = ?crate::state::request_caller(ctx.parts()),
            resource = %table_arn,
            outcome = "committed",
            "iceberg table metadata committed",
        );

        let mut members = Map::new();
        members.insert(
            "metadata-location".to_string(),
            Value::from(metadata_location),
        );
        members.insert("metadata".to_string(), document);
        json(200, &Value::Object(members))
    }
}

/// What the catalog says when this service's store is not reachable.
const STORE_UNAVAILABLE: &str = "The S3 Tables table bucket store is unavailable";

/// What the catalog says when the request body could not be read at all. The
/// router buffers every request before dispatch, so a body it cannot hand back
/// in memory is a failure of the server, never a caller's malformed request.
const UNREADABLE_BODY: &str = "The server could not read the request body";

/// What the catalog says to a commit whose body is not a JSON document at all.
const MALFORMED_COMMIT: &str = "Malformed request: the commit is not a JSON object";

/// The refusal the catalog answers a request it will not serve with. Boxed, so
/// a result carrying one stays small whatever a rendered response costs.
type Refusal = Box<RtResponse>;

/// The table the request addresses, read through the bucket the warehouse prefix
/// names, or the refusal the catalog answers when it holds no such table.
///
/// The namespace is resolved first, because a prefix naming no table bucket and
/// a bucket holding no such namespace are the same thing to a catalog client:
/// the namespace it addressed is not in this catalog.
fn resolve<'s>(
    store: &'s crate::state::ComAmazonawsS3tablesS3tablebucketsState,
    request: &CatalogRequest,
) -> Result<&'s Table, Refusal> {
    let namespace = store
        .table_buckets
        .get(&request.table_bucket_arn)
        .filter(|_| !request.namespace.contains(NAMESPACE_SEPARATOR))
        .and_then(|bucket| bucket.namespaces.get(&request.namespace))
        .ok_or_else(|| {
            refused(
                404,
                "NoSuchNamespaceException",
                &format!("Namespace does not exist: {}", request.namespace),
            )
        })?;
    namespace.tables.get(&request.table).ok_or_else(|| {
        refused(
            404,
            "NoSuchTableException",
            &format!("Table does not exist: {}", qualified(request)),
        )
    })
}

/// The table the request addresses, for a caller that is about to change it.
fn resolve_mut<'s>(
    store: &'s mut crate::state::ComAmazonawsS3tablesS3tablebucketsState,
    request: &CatalogRequest,
) -> Result<&'s mut Table, ()> {
    store
        .table_buckets
        .get_mut(&request.table_bucket_arn)
        .and_then(|bucket| bucket.namespaces.get_mut(&request.namespace))
        .and_then(|namespace| namespace.tables.get_mut(&request.table))
        .ok_or(())
}

/// The table as a catalog client names it: its namespace and its name.
fn qualified(request: &CatalogRequest) -> String {
    format!("{}.{}", request.namespace, request.table)
}

/// The requirements a commit states, or the refusal for one this catalog does
/// not know.
fn requirements_of(document: &Value) -> Result<Vec<MetadataRequirement>, Refusal> {
    let mut requirements = Vec::new();
    let stated_requirements = entries(document, "requirements")?;
    for stated in stated_requirements {
        let kind = stated.get("type").and_then(Value::as_str).unwrap_or("");
        let requirement = match kind {
            "assert-create" => MetadataRequirement::AssertCreate,
            "assert-table-uuid" => MetadataRequirement::AssertTableUuid(
                text(stated, "uuid").ok_or_else(|| missing(kind, "uuid"))?,
            ),
            "assert-current-schema-id" => MetadataRequirement::AssertCurrentSchemaId(
                number(stated, "current-schema-id")
                    .ok_or_else(|| missing(kind, "current-schema-id"))?,
            ),
            "assert-default-sort-order-id" => MetadataRequirement::AssertDefaultSortOrderId(
                number(stated, "default-sort-order-id")
                    .ok_or_else(|| missing(kind, "default-sort-order-id"))?,
            ),
            "assert-default-spec-id" => MetadataRequirement::AssertDefaultSpecId(
                number(stated, "default-spec-id").ok_or_else(|| missing(kind, "default-spec-id"))?,
            ),
            "assert-last-assigned-field-id" => MetadataRequirement::AssertLastAssignedFieldId(
                number(stated, "last-assigned-field-id")
                    .ok_or_else(|| missing(kind, "last-assigned-field-id"))?,
            ),
            "assert-last-assigned-partition-id" => {
                MetadataRequirement::AssertLastAssignedPartitionId(
                    number(stated, "last-assigned-partition-id")
                        .ok_or_else(|| missing(kind, "last-assigned-partition-id"))?,
                )
            }
            "assert-ref-snapshot-id" => MetadataRequirement::AssertRefSnapshotId {
                reference: text(stated, "ref").ok_or_else(|| missing(kind, "ref"))?,
                snapshot_id: stated.get("snapshot-id").and_then(Value::as_i64),
            },
            other => {
                return Err(refused(
                    400,
                    "BadRequestException",
                    &format!("Invalid request: unknown requirement type {other}"),
                ))
            }
        };
        requirements.push(requirement);
    }
    Ok(requirements)
}

/// The updates a commit asks for, or the refusal for one this catalog does not
/// apply.
///
/// An update that commits a SNAPSHOT names data files, and this emulator serves
/// no Iceberg data plane that could have written any, so it is answered as
/// unserved rather than recorded as if it had happened.
fn updates_of(document: &Value) -> Result<Vec<MetadataUpdate>, Refusal> {
    /// The updates of the Iceberg REST catalog specification this catalog does
    /// not apply: the ones that commit a snapshot, a snapshot reference or a
    /// statistics file for one — all of which name data files only a data plane
    /// writes — and the ones that remove part of a table's metadata history or
    /// turn on a table feature this catalog does not keep.
    const UNSERVED_UPDATES: &[&str] = &[
        "add-snapshot",
        "set-snapshot-ref",
        "remove-snapshots",
        "remove-snapshot-ref",
        "remove-snapshot",
        "set-statistics",
        "remove-statistics",
        "set-partition-statistics",
        "remove-partition-statistics",
        "remove-partition-specs",
        "remove-schemas",
        "enable-row-lineage",
    ];

    let mut updates = Vec::new();
    let stated_updates = entries(document, "updates")?;
    for stated in stated_updates {
        let action = stated.get("action").and_then(Value::as_str).unwrap_or("");
        let update = match action {
            "assign-uuid" => {
                MetadataUpdate::AssignUuid(text(stated, "uuid").ok_or_else(|| missing(action, "uuid"))?)
            }
            "upgrade-format-version" => MetadataUpdate::UpgradeFormatVersion(
                stated
                    .get("format-version")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| missing(action, "format-version"))?,
            ),
            "add-schema" => MetadataUpdate::AddSchema(
                schema_of(stated.get("schema").ok_or_else(|| missing(action, "schema"))?)
                    .map_err(|reason| refused(400, "BadRequestException", &reason))?,
            ),
            "set-current-schema" => MetadataUpdate::SetCurrentSchema(
                number(stated, "schema-id").ok_or_else(|| missing(action, "schema-id"))?,
            ),
            "add-spec" => MetadataUpdate::AddPartitionSpec(
                spec_of(stated.get("spec").ok_or_else(|| missing(action, "spec"))?)
                    .map_err(|reason| refused(400, "BadRequestException", &reason))?,
            ),
            "set-default-spec" => MetadataUpdate::SetDefaultSpec(
                number(stated, "spec-id").ok_or_else(|| missing(action, "spec-id"))?,
            ),
            "add-sort-order" => MetadataUpdate::AddSortOrder(
                sort_order_of(
                    stated
                        .get("sort-order")
                        .ok_or_else(|| missing(action, "sort-order"))?,
                )
                .map_err(|reason| refused(400, "BadRequestException", &reason))?,
            ),
            "set-default-sort-order" => MetadataUpdate::SetDefaultSortOrder(
                number(stated, "sort-order-id").ok_or_else(|| missing(action, "sort-order-id"))?,
            ),
            "set-properties" => {
                let properties = stated
                    .get("updates")
                    .and_then(Value::as_object)
                    .ok_or_else(|| missing(action, "updates"))?;
                // The properties a commit writes are caller-sized key/value
                // data, so their count is bounded like every other list of a
                // commit before any of it is stored.
                if properties.len() > ENTRIES_PER_COMMIT {
                    return Err(refused(
                        400,
                        "BadRequestException",
                        &format!(
                            "Invalid request: a commit sets at most {ENTRIES_PER_COMMIT} \
                             properties"
                        ),
                    ));
                }
                MetadataUpdate::SetProperties(
                    properties
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect(),
                )
            }
            "remove-properties" => {
                let removals = stated
                    .get("removals")
                    .and_then(Value::as_array)
                    .ok_or_else(|| missing(action, "removals"))?;
                if removals.len() > ENTRIES_PER_COMMIT {
                    return Err(refused(
                        400,
                        "BadRequestException",
                        &format!(
                            "Invalid request: a commit removes at most {ENTRIES_PER_COMMIT} \
                             properties"
                        ),
                    ));
                }
                MetadataUpdate::RemoveProperties(
                    removals
                        .iter()
                        .filter_map(|removal| removal.as_str().map(str::to_string))
                        .collect(),
                )
            }
            "set-location" => MetadataUpdate::SetLocation(
                text(stated, "location").ok_or_else(|| missing(action, "location"))?,
            ),
            unserved if UNSERVED_UPDATES.contains(&unserved) => {
                return Err(refused(
                    501,
                    "NotImplementedException",
                    &format!("This catalog does not serve the {unserved} update"),
                ))
            }
            other => {
                return Err(refused(
                    400,
                    "BadRequestException",
                    &format!("Invalid request: unknown update action {other}"),
                ))
            }
        };
        updates.push(update);
    }
    Ok(updates)
}

/// The entries of the `member` list of a commit, refused when the list is longer
/// than [`ENTRIES_PER_COMMIT`]. A member the commit does not carry is no entries.
fn entries<'d>(document: &'d Value, member: &str) -> Result<&'d [Value], Refusal> {
    let entries = document
        .get(member)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if entries.len() > ENTRIES_PER_COMMIT {
        return Err(refused(
            400,
            "BadRequestException",
            &format!(
                "Invalid request: a commit states at most {ENTRIES_PER_COMMIT} {member}"
            ),
        ));
    }
    Ok(entries)
}

/// A schema an update carries, as this service stores it. The schema's own id is
/// assigned by the catalog when the update is applied, so the one the document
/// states is not read here.
fn schema_of(document: &Value) -> Result<crate::state::IcebergSchemaRecord, String> {
    use crate::state::{IcebergField, IcebergSchemaRecord, INITIAL_SCHEMA_ID};

    let fields = fields_of(document, "schema")?;
    let fields = fields
        .iter()
        .map(|field| {
            Ok(IcebergField {
                id: field
                    .get("id")
                    .and_then(Value::as_i64)
                    .and_then(|id| i32::try_from(id).ok())
                    .ok_or_else(|| "Invalid schema: a field states no id".to_string())?,
                name: field
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "Invalid schema: a field states no name".to_string())?
                    .to_string(),
                field_type: field
                    .get("type")
                    .cloned()
                    .ok_or_else(|| "Invalid schema: a field states no type".to_string())?,
                required: field
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or_default(),
                doc: field
                    .get("doc")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect::<Result<Vec<IcebergField>, String>>()?;
    Ok(IcebergSchemaRecord {
        schema_id: INITIAL_SCHEMA_ID,
        fields,
        identifier_field_ids: document
            .get("identifier-field-ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_i64().and_then(|id| i32::try_from(id).ok()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// A partition spec an update carries, as this service stores it. The spec's own
/// id and its partition field ids are assigned by the catalog when the update is
/// applied.
fn spec_of(document: &Value) -> Result<IcebergPartitionSpecRecord, String> {
    let fields = fields_of(document, "partition spec")?;
    let fields = fields
        .iter()
        .map(|field| {
            Ok(IcebergPartitionFieldRecord {
                source_id: field
                    .get("source-id")
                    .and_then(Value::as_i64)
                    .and_then(|id| i32::try_from(id).ok())
                    .ok_or_else(|| {
                        "Invalid partition spec: a field states no source-id".to_string()
                    })?,
                field_id: 0,
                name: field
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "Invalid partition spec: a field states no name".to_string())?
                    .to_string(),
                transform: field
                    .get("transform")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        "Invalid partition spec: a field states no transform".to_string()
                    })?
                    .to_string(),
            })
        })
        .collect::<Result<Vec<IcebergPartitionFieldRecord>, String>>()?;
    Ok(IcebergPartitionSpecRecord {
        spec_id: crate::state::INITIAL_SPEC_ID,
        fields,
    })
}

/// A sort order an update carries, as this service stores it. The order's own id
/// is assigned by the catalog when the update is applied.
fn sort_order_of(document: &Value) -> Result<IcebergSortOrderRecord, String> {
    /// The direction a sort field takes when the order states none, as the
    /// Iceberg table specification defines the default.
    const DEFAULT_DIRECTION: &str = "asc";
    /// The null order a sort field takes when it states none: nulls sort first
    /// in an ascending order and last in a descending one, as the Iceberg table
    /// specification defines it.
    const NULLS_FIRST: &str = "nulls-first";
    const NULLS_LAST: &str = "nulls-last";

    let fields = fields_of(document, "sort order")?;
    let fields = fields
        .iter()
        .map(|field| {
            let direction = field
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_DIRECTION)
                .to_string();
            let null_order = field
                .get("null-order")
                .and_then(Value::as_str)
                .unwrap_or(if direction == DEFAULT_DIRECTION {
                    NULLS_FIRST
                } else {
                    NULLS_LAST
                })
                .to_string();
            Ok(IcebergSortFieldRecord {
                source_id: field
                    .get("source-id")
                    .and_then(Value::as_i64)
                    .and_then(|id| i32::try_from(id).ok())
                    .ok_or_else(|| "Invalid sort order: a field states no source-id".to_string())?,
                transform: field
                    .get("transform")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "Invalid sort order: a field states no transform".to_string())?
                    .to_string(),
                direction,
                null_order,
            })
        })
        .collect::<Result<Vec<IcebergSortFieldRecord>, String>>()?;
    Ok(IcebergSortOrderRecord {
        order_id: crate::state::UNSORTED_ORDER_ID,
        fields,
    })
}

/// The `fields` of a schema, partition spec or sort order an update carries, as
/// the document states them: the member must be there (what it describes is its
/// fields), and it is bounded at [`ENTRIES_PER_COMMIT`] like the commit's own
/// lists, since every field it states is stored.
fn fields_of<'d>(document: &'d Value, kind: &str) -> Result<&'d [Value], String> {
    let fields = document
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("Invalid {kind}: no fields"))?;
    if fields.len() > ENTRIES_PER_COMMIT {
        return Err(format!(
            "Invalid {kind}: it states more than {ENTRIES_PER_COMMIT} fields"
        ));
    }
    Ok(fields)
}

/// The string member `name` of `document`, when it carries one.
fn text(document: &Value, name: &str) -> Option<String> {
    document
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The integer member `name` of `document`, when it carries one.
fn number(document: &Value, name: &str) -> Option<i32> {
    document
        .get(name)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
}

/// The refusal for a requirement or update that leaves out a member it is
/// defined by: the request states no complete instruction, so there is nothing
/// to apply.
fn missing(kind: &str, member: &str) -> Refusal {
    refused(
        400,
        "BadRequestException",
        &format!("Invalid request: {kind} states no {member}"),
    )
}

/// One JSON answer of the catalog. The media type is the Iceberg REST catalog's
/// own, not this service's modelled envelope: the body is that API's document.
fn json(status: u16, body: &Value) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
}

/// One answer of the catalog with no body, which is what its existence check
/// answers with.
fn empty(status: u16) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::empty(),
    );
    response.headers_mut().insert("content-length", "0");
    response
}

/// What a catalog request ACTS ON, and the control-plane action it is performed
/// under.
///
/// The catalog is not an operation of the model, so no modelled permission names
/// it; but every endpoint of it reads or writes the SAME records a modelled
/// operation does, and S3 Tables documents the permission for each of those. So
/// the action a catalog request is decided under is the control-plane action
/// whose records the endpoint reaches — a load reads the table's metadata
/// location, a commit moves it, a namespace listing lists namespaces — and the
/// resource is the one the modelled gate decides those operations against
/// ([`super::authorization`]): the table when the request addresses one, else the
/// table bucket it is addressed within.
struct Governed {
    /// The modelled operation whose permission this endpoint takes.
    operation: &'static str,
    /// The warehouse the request is addressed for: a table bucket's ARN.
    table_bucket_arn: String,
    /// The namespace and name of the table the request addresses inside it, when
    /// it addresses one.
    table: Option<(String, String)>,
}

impl Governed {
    /// The decision this request is taken under, as the test in
    /// [`crate::service`] reads it: the action and the resource.
    #[cfg(test)]
    fn decided(&self) -> Decided {
        Decided {
            operation: self.operation,
            table_bucket_arn: self.table_bucket_arn.clone(),
            table: self.table.clone(),
        }
    }
}

/// The decision ONE catalog request is taken under: the control-plane action the
/// endpoint takes, the table bucket it is addressed for, and the table inside it
/// when it addresses one.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Decided {
    pub(super) operation: &'static str,
    pub(super) table_bucket_arn: String,
    pub(super) table: Option<(String, String)>,
}

/// The decision the catalog request `method` and `uri` is taken under, or `None`
/// when it addresses no resource of this service at all.
///
/// The one reader outside this module is the test in [`crate::service`] that
/// holds this catalog to the gate every modelled operation of the service is
/// served behind: the catalog is reached through a seam the model binds no
/// operation to, so no entry of that service's gated routing table can wrap it,
/// and a request it CLAIMS but names no resource for would be served with the
/// policies in force ignored.
#[cfg(test)]
pub(super) fn decided_as(method: &str, uri: &str) -> Option<Decided> {
    Some(governed(method, uri)?.decided())
}

/// What the catalog request `method` and `uri` acts on, or `None` when it
/// addresses no resource of this service at all — the configuration endpoint of a
/// request naming no warehouse, which leaves the decision nothing to be taken on
/// (and is refused for naming none).
fn governed(method: &str, uri: &str) -> Option<Governed> {
    if let Some(discovery) = discovery::request_of(method, uri) {
        return discovery.governed();
    }
    let request = catalog_request_of(method, uri)?;
    Some(Governed {
        operation: match request.endpoint {
            // A load answers the table's metadata document and the location of
            // the file it is written to.
            TableEndpoint::Load => "GetTableMetadataLocation",
            // Whether the table is there at all, which is what a read of the
            // table answers.
            TableEndpoint::Exists | TableEndpoint::Unserved => "GetTable",
            // A commit MOVES the table to another metadata document, which is
            // what the modelled write of that member does.
            TableEndpoint::Commit => "UpdateTableMetadataLocation",
        },
        table_bucket_arn: request.table_bucket_arn,
        table: Some((request.namespace, request.table)),
    })
}

/// One refusal of the catalog, in the Iceberg REST catalog's own error envelope:
/// the sentence, the exception type the specification names the class by, and
/// the status.
fn error(status: u16, kind: &str, message: &str) -> RtResponse {
    let mut inner = Map::new();
    inner.insert("message".to_string(), Value::from(message));
    inner.insert("type".to_string(), Value::from(kind));
    inner.insert("code".to_string(), Value::from(status));
    let mut body = Map::new();
    body.insert("error".to_string(), Value::Object(inner));
    json(status, &Value::Object(body))
}

/// The same refusal, boxed for a [`Refusal`] result.
fn refused(status: u16, kind: &str, message: &str) -> Refusal {
    Box::new(error(status, kind, message))
}
