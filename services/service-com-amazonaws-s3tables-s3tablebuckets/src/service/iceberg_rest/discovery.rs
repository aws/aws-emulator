// SPDX-License-Identifier: CC0-1.0

//! The DISCOVERY half of the Iceberg REST catalog: how a client finds the
//! warehouse it was configured for, the namespaces in it and the tables in a
//! namespace.
//!
//! An Iceberg engine (Spark, Trino, PyIceberg, Athena) is configured with the
//! catalog's URI and a `warehouse`, and the first thing it does is ask the
//! catalog for its configuration: the answer tells it which path PREFIX every
//! later request of the session is addressed under. From there it lists the
//! namespaces of that warehouse and the tables of a namespace before it loads
//! any table ([`super`] serves the table endpoint itself).
//!
//! Every answer is projected from the SAME records the control plane reads and
//! writes, so the catalog and the control plane cannot disagree about what a
//! table bucket holds: the namespaces this lists are the namespaces
//! `ListNamespaces` lists, and the tables are the ones `ListTables` lists.
//!
//! The documents are the Iceberg REST catalog specification's own — a
//! configuration of `defaults` and `overrides`, a namespace as the LIST of its
//! levels, a table as an identifier of its namespace and its name — and a
//! refusal is that API's error document, never this service's modelled error
//! shapes.
//!
//! # Two choices worth stating
//!
//! * The specification makes `warehouse` OPTIONAL on the configuration
//!   endpoint, for a catalog that serves one warehouse and therefore needs no
//!   prefix. This catalog serves a warehouse PER prefix — a table bucket is what
//!   holds the namespaces and tables, and the account holds many — so a
//!   configuration request that names none names no table bucket to answer a
//!   prefix for, and is the client request error it is rather than a
//!   configuration for a warehouse the caller did not choose.
//! * The namespace and table arms name their refusals with the specification's
//!   own exception types (`NoSuchNamespaceException`, `NoSuchTableException`),
//!   which is what an Iceberg client maps. The specification has no type for a
//!   warehouse — it is this service's own control-plane resource, the table
//!   bucket — so those arms carry the error shape names the control plane states
//!   that class with (`BadRequestException`, `NotFoundException`), the names the
//!   same caller reads from `GetTableBucket`.

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{Map, Value};

use super::{
    empty, error, json, IcebergCatalog, CATALOG_PREFIX, NAMESPACES_SEGMENT, NAMESPACE_SEPARATOR,
    STORE_UNAVAILABLE, TABLES_SEGMENT,
};

/// The endpoint that answers the catalog's own configuration, which is reached
/// under no warehouse prefix (it is what the prefix is learned FROM).
const CONFIG_SEGMENT: &str = "config";

/// The query parameter a client names the warehouse it was configured for in,
/// as the Iceberg REST catalog specification defines the configuration
/// endpoint's parameter.
const WAREHOUSE_PARAMETER: &str = "warehouse";

/// The query parameter a namespace listing may be scoped to a parent namespace
/// by, as the specification defines it.
const PARENT_PARAMETER: &str = "parent";

/// The configuration key whose value every later request of the session is
/// addressed under: the catalog answers it among the DEFAULTS, so a client that
/// was configured for a different prefix keeps the one it was given — nothing
/// this catalog settles overrides a client's own configuration.
const PREFIX_KEY: &str = "prefix";

/// The configuration key naming the FileIO implementation a client reads the
/// table's files through, and the one this catalog's warehouses are served by:
/// every warehouse location this service mints is an `s3://` location, so the
/// files of every table it holds are read with Amazon S3's own implementation.
const FILE_IO_KEY: &str = "io-impl";
const FILE_IO_IMPLEMENTATION: &str = "org.apache.iceberg.aws.s3.S3FileIO";

/// The configuration key that tells a writer to lay a table's data files out
/// under hashed prefixes rather than under the partition path, and the value
/// every warehouse of this catalog is served under: S3 Tables stores a table's
/// files that way, so a writer that respected the partition layout instead would
/// write them where the service does not keep them.
const OBJECT_STORAGE_KEY: &str = "write.object-storage.enabled";
const OBJECT_STORAGE_ENABLED: &str = "true";

/// The companion of [`OBJECT_STORAGE_KEY`]: whether the hashed layout still
/// carries the partition path. S3 Tables stores a table's files under the hash
/// alone, so a writer must not add the partition path to it.
const PARTITIONED_PATHS_KEY: &str = "write.object-storage.partitioned-paths";
const PARTITIONED_PATHS_ENABLED: &str = "false";

/// The configuration key that tells a client whether it may delete a table's
/// files itself, and the value every warehouse of this catalog is served under:
/// S3 Tables owns the files under a table's warehouse location — its maintenance
/// is what removes one — so a client deletes none of them.
const DELETE_KEY: &str = "s3.delete-enabled";
const DELETE_ENABLED: &str = "false";

/// The characters a path label carries as themselves: RFC 3986's unreserved set
/// (`ALPHA`, `DIGIT`, `-`, `_`, `.`, `~`). Everything else of the prefix — the
/// `:` and `/` of an ARN — is percent-escaped, because the prefix is a path
/// SEGMENT of every later request of the session.
const PATH_LABEL: ::percent_encoding::AsciiSet = ::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// What the catalog says when the warehouse a request names is one this service
/// holds no table bucket for.
const NO_SUCH_WAREHOUSE: &str = "Warehouse does not exist";

/// What the catalog says to a configuration request that names no warehouse at
/// all: there is no table bucket to answer a prefix for.
const WAREHOUSE_REQUIRED: &str =
    "Malformed request: the config endpoint requires a warehouse to answer for";

/// One request to the catalog's discovery endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Discovery {
    /// `GET /iceberg/v1/config`: the configuration of the catalog for one
    /// warehouse.
    Config { warehouse: Option<String> },
    /// `GET /iceberg/v1/{prefix}/namespaces`: the namespaces of the warehouse,
    /// or of one parent namespace inside it.
    Namespaces {
        table_bucket_arn: String,
        parent: Option<String>,
    },
    /// `GET` or `HEAD /iceberg/v1/{prefix}/namespaces/{namespace}`: the
    /// namespace itself, or whether it exists.
    Namespace {
        table_bucket_arn: String,
        namespace: String,
        /// Set for the `HEAD` form, which answers a status and no document.
        exists_only: bool,
    },
    /// `GET /iceberg/v1/{prefix}/namespaces/{namespace}/tables`: the tables of
    /// one namespace.
    Tables {
        table_bucket_arn: String,
        namespace: String,
    },
}

/// The discovery request `method` and `uri` address, or `None` when the URI is
/// not one of these endpoints — in which case the request is left exactly as it
/// was, for the table endpoint or for the modelled routing table.
///
/// Only the methods this catalog SERVES are claimed: a method of one of these
/// path spaces that it does not serve (the create of a namespace, say) is left
/// unclaimed rather than answered as if it had been considered.
///
/// Every segment is percent-decoded the way the codec that deserializes a path
/// label decodes one, because a client addresses the warehouse by an ARN, whose
/// `:` and `/` travel encoded.
pub(super) fn request_of(method: &str, uri: &str) -> Option<Discovery> {
    let path = ::service_common::request_path(uri);
    let query = uri.split_once('?').map(|(_, query)| query).unwrap_or("");
    let parameter = |name: &str| {
        ::service_common::query_parameter(query, name)
            .map(::service_common::percent_decode_path_label)
            .filter(|value| !value.is_empty())
    };
    let segments: Vec<&str> = path.strip_prefix(CATALOG_PREFIX)?.split('/').collect();
    let decoded = |segment: &str| ::service_common::percent_decode_path_label(segment);
    match (method, segments.as_slice()) {
        ("GET", [config]) if *config == CONFIG_SEGMENT => Some(Discovery::Config {
            warehouse: parameter(WAREHOUSE_PARAMETER),
        }),
        ("GET", [prefix, namespaces]) if *namespaces == NAMESPACES_SEGMENT => {
            Some(Discovery::Namespaces {
                table_bucket_arn: decoded(prefix),
                parent: parameter(PARENT_PARAMETER),
            })
        }
        ("GET" | "HEAD", [prefix, namespaces, namespace]) if *namespaces == NAMESPACES_SEGMENT => {
            Some(Discovery::Namespace {
                table_bucket_arn: decoded(prefix),
                namespace: decoded(namespace),
                exists_only: method == "HEAD",
            })
        }
        ("GET", [prefix, namespaces, namespace, tables])
            if *namespaces == NAMESPACES_SEGMENT && *tables == TABLES_SEGMENT =>
        {
            Some(Discovery::Tables {
                table_bucket_arn: decoded(prefix),
                namespace: decoded(namespace),
            })
        }
        _ => None,
    }
}

impl Discovery {
    /// What this discovery request acts on, and the control-plane action it is
    /// performed under — the question [`super::governed`] asks of every catalog
    /// request before it is served.
    ///
    /// Each arm names the modelled operation whose RECORDS the endpoint reports
    /// from, since that is the operation whose permission S3 Tables documents for
    /// reading them: the warehouse's configuration reports the table bucket, the
    /// namespace listings report what `ListNamespaces` and `GetNamespace` report,
    /// and the table listing what `ListTables` reports. A configuration request
    /// naming no warehouse addresses no resource of this service at all, which
    /// leaves the decision nothing to be taken on.
    pub(super) fn governed(&self) -> Option<super::Governed> {
        let (operation, table_bucket_arn) = match self {
            Self::Config { warehouse } => ("GetTableBucket", warehouse.clone()?),
            Self::Namespaces {
                table_bucket_arn, ..
            } => ("ListNamespaces", table_bucket_arn.clone()),
            Self::Namespace {
                table_bucket_arn, ..
            } => ("GetNamespace", table_bucket_arn.clone()),
            Self::Tables {
                table_bucket_arn, ..
            } => ("ListTables", table_bucket_arn.clone()),
        };
        Some(super::Governed {
            operation,
            table_bucket_arn,
            // A discovery endpoint addresses the warehouse or a namespace in it,
            // never one table, so the decision is taken on the table bucket.
            table: None,
        })
    }
}

impl<Eng: Send + Sync + 'static> IcebergCatalog<Eng> {
    /// Serve one discovery request.
    ///
    /// Every arm is a REPORT: it reads the stored records and writes nothing, so
    /// two identical requests answer the same document.
    pub(super) fn discover(&self, request: &Discovery) -> RtResponse {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return error(503, "ServiceUnavailableException", STORE_UNAVAILABLE);
        };
        let store = state.lock();
        // Whether this store holds the warehouse a request is addressed for is
        // the first question of every arm but the configuration's, which reads
        // the warehouse from the query instead.
        let holds = |arn: &String| store.table_buckets.contains_key(arn);
        match request {
            Discovery::Config { warehouse } => {
                let Some(warehouse) = warehouse else {
                    return error(400, "BadRequestException", WAREHOUSE_REQUIRED);
                };
                if !holds(warehouse) {
                    return error(
                        404,
                        "NotFoundException",
                        &format!("{NO_SUCH_WAREHOUSE}: {warehouse}"),
                    );
                }
                // The prefix every later request of the session is addressed
                // under is the warehouse itself: this catalog serves one table
                // bucket per prefix, and the table bucket is named by the ARN
                // the client was configured with. It is answered ENCODED, since
                // a client builds the path of its next request by putting this
                // value in a segment of it, and an ARN's `:` and `/` cannot
                // travel in one as themselves.
                let prefix =
                    ::percent_encoding::utf8_percent_encode(warehouse, &PATH_LABEL).to_string();
                let mut defaults = Map::new();
                defaults.insert(PREFIX_KEY.to_string(), Value::from(prefix));
                defaults.insert(
                    FILE_IO_KEY.to_string(),
                    Value::from(FILE_IO_IMPLEMENTATION),
                );
                defaults.insert(
                    OBJECT_STORAGE_KEY.to_string(),
                    Value::from(OBJECT_STORAGE_ENABLED),
                );
                defaults.insert(
                    PARTITIONED_PATHS_KEY.to_string(),
                    Value::from(PARTITIONED_PATHS_ENABLED),
                );
                defaults.insert(DELETE_KEY.to_string(), Value::from(DELETE_ENABLED));
                let mut document = Map::new();
                document.insert("defaults".to_string(), Value::Object(defaults));
                // Nothing a client configures is overridden: every property this
                // catalog states is one the client may replace, which is what a
                // default is.
                document.insert("overrides".to_string(), Value::Object(Map::new()));
                json(200, &Value::Object(document))
            }
            Discovery::Namespaces {
                table_bucket_arn,
                parent,
            } => {
                let Some(bucket) = store.table_buckets.get(table_bucket_arn) else {
                    return error(
                        404,
                        "NotFoundException",
                        &format!("{NO_SUCH_WAREHOUSE}: {table_bucket_arn}"),
                    );
                };
                // A namespace of this service has ONE level
                // (`CreateNamespaceRequest.namespace` is bounded at one
                // element), so no namespace it holds has a child: a listing
                // scoped to a parent answers the empty list for a namespace
                // that exists and that it is not here for one that does not.
                let namespaces: Vec<Value> = match parent {
                    Some(parent) => {
                        if !resident(bucket, parent) {
                            return error(
                                404,
                                "NoSuchNamespaceException",
                                &format!("Namespace does not exist: {parent}"),
                            );
                        }
                        Vec::new()
                    }
                    None => bucket
                        .namespaces
                        .values()
                        .map(|namespace| levels(&namespace.namespace))
                        .collect(),
                };
                let mut document = Map::new();
                document.insert("namespaces".to_string(), Value::Array(namespaces));
                json(200, &Value::Object(document))
            }
            Discovery::Namespace {
                table_bucket_arn,
                namespace,
                exists_only,
            } => {
                let held = store
                    .table_buckets
                    .get(table_bucket_arn)
                    .filter(|_| !namespace.contains(NAMESPACE_SEPARATOR))
                    .and_then(|bucket| bucket.namespaces.get(namespace));
                let Some(held) = held else {
                    return if *exists_only {
                        empty(404)
                    } else {
                        error(
                            404,
                            "NoSuchNamespaceException",
                            &format!("Namespace does not exist: {namespace}"),
                        )
                    };
                };
                if *exists_only {
                    return empty(204);
                }
                let mut document = Map::new();
                document.insert("namespace".to_string(), levels(&held.namespace));
                // A namespace of this service carries no properties of its own:
                // nothing sets any, so the document states none rather than
                // claiming properties nothing wrote.
                document.insert("properties".to_string(), Value::Object(Map::new()));
                json(200, &Value::Object(document))
            }
            Discovery::Tables {
                table_bucket_arn,
                namespace,
            } => {
                let held = store
                    .table_buckets
                    .get(table_bucket_arn)
                    .filter(|_| !namespace.contains(NAMESPACE_SEPARATOR))
                    .and_then(|bucket| bucket.namespaces.get(namespace));
                let Some(held) = held else {
                    return error(
                        404,
                        "NoSuchNamespaceException",
                        &format!("Namespace does not exist: {namespace}"),
                    );
                };
                // One identifier per table the namespace holds, in the order the
                // control plane's own listing pages over them, so the two
                // surfaces report the same tables in the same order.
                let identifiers: Vec<Value> = held
                    .tables
                    .values()
                    .map(|table| {
                        let mut identifier = Map::new();
                        identifier.insert("namespace".to_string(), levels(&table.namespace));
                        identifier.insert("name".to_string(), Value::from(table.name.clone()));
                        Value::Object(identifier)
                    })
                    .collect();
                let mut document = Map::new();
                document.insert("identifiers".to_string(), Value::Array(identifiers));
                json(200, &Value::Object(document))
            }
        }
    }
}

/// Whether `bucket` holds a namespace of that name.
fn resident(bucket: &crate::state::TableBucket, namespace: &str) -> bool {
    !namespace.contains(NAMESPACE_SEPARATOR) && bucket.namespaces.contains_key(namespace)
}

/// A namespace as the Iceberg REST catalog spells one: the list of its levels.
fn levels(namespace: &[String]) -> Value {
    Value::Array(
        namespace
            .iter()
            .map(|level| Value::from(level.clone()))
            .collect(),
    )
}
