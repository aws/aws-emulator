// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appsync-awsdeepdishcontrolplaneservice service.
//!
//! Holds the API lifecycle: every `Api` and every `GraphqlApi` the service created, each with
//! the API keys and data sources created under it, and every source-API association
//! between two of them. A handler reaches it through [`service_state`] and answers from what is
//! stored here — nothing is recomputed from the request.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The Event API DATA plane's records: the sockets each `Api` is served on, the realtime
/// connections open on them and the subscriptions those connections hold. A field of the
/// store below, so an API's records and the sockets it is reached at are read under one
/// lock.
pub mod event_data_plane;

/// The GraphQL API REAL-TIME plane's records: the socket each `GraphqlApi`'s `REALTIME`
/// endpoint is served on, the connections open on it and the subscriptions those
/// connections hold. A field of the store below for the same reason the Event API's plane
/// is one.
pub mod graphql_data_plane;

/// The GraphQL types created under a GraphQL API, and how one type definition is
/// parsed, kept and reported.
mod schema_types;
pub use schema_types::*;

/// How one stored type is written back as GraphQL SDL: the rendering every READ of a type
/// answers with, whichever way that type got into its API.
mod type_sdl;

/// How one stored type is written back as JSON: the type's own introspection document,
/// which is the other representation a read can ask for.
mod type_json;

/// The schema a GraphQL API holds, and the asynchronous creation that installs one.
mod schema_creation;
pub use schema_creation::*;

/// The introspection schema of a GraphQL API: the two representations its type system
/// is reported in, and the refusal for an API that holds no schema at all.
mod introspection_schema;
pub use introspection_schema::*;

/// The merge of a source API's schema into the Merged API it is associated with.
mod schema_merge;

/// The resolvers attached to a GraphQL API's fields, and how one is kept, addressed and
/// reported.
mod resolvers;
pub use resolvers::*;

/// The members whose modelled `@pattern` admits any text at all — mapping templates and
/// descriptions — and the `@length` that is the only bound left on them.
mod free_text;
pub use free_text::*;

/// The `name` an Event API carries: the form `ApiName` declares for it, and the
/// framework-level constraint report a value outside that form is refused with.
mod api_name;
pub use api_name::*;

/// What inside one API still uses one of its resources, and the refusal a delete that
/// would break that use answers with.
mod dependencies;
pub use dependencies::*;

/// The APPSYNC_JS `code` a resolver or a function is defined by: the compile a call that
/// stores one runs over it, and the refusal code that does not compile takes.
mod code_definition;
pub use code_definition::*;

/// The data source introspections this service has run, and how the schema one read is
/// reported.
mod data_source_introspection;
pub use data_source_introspection::*;

/// How that schema is READ: the RDS cluster's own Data API, asked for the tables, columns,
/// keys and indexes of the database an introspection names.
mod introspected_cluster;
pub use introspected_cluster::*;

use crate::ops::types::{
    AdditionalAuthenticationProvider, Api, ApiAssociation, ApiCache, ApiKey, AppSyncRuntime,
    AuthMode, ChannelNamespace, DataSource, DomainNameConfig, DynamodbDataSourceConfig,
    ElasticsearchDataSourceConfig, EnhancedMetricsConfig, EventConfig, EventBridgeDataSourceConfig,
    FunctionConfiguration, GraphqlApi, HandlerConfigs, HttpDataSourceConfig,
    LambdaAuthorizerConfig, LambdaDataSourceConfig, LogConfig, OpenIDConnectConfig,
    OpenSearchServiceDataSourceConfig, RelationalDatabaseDataSourceConfig, SourceApiAssociation,
    SourceApiAssociationConfig, SourceApiAssociationSummary, SyncConfig, UserPoolConfig,
};

/// One stored `GraphqlApi`, plus the API keys created under it.
///
/// Every member of the modelled `GraphqlApi` shape that the create call can set is
/// kept, so `GetGraphqlApi` and `ListGraphqlApis` project exactly what was created
/// rather than a subset.
#[derive(Debug, Clone)]
pub struct GraphqlApiRecord {
    /// Creation order, used to keep `ListGraphqlApis` (and its `nextToken` cursor)
    /// stable across calls: the minted `apiId` is opaque, so it cannot carry order.
    pub seq: u64,
    pub api_id: String,
    pub name: String,
    pub arn: String,
    pub authentication_type: String,
    pub api_type: String,
    pub visibility: String,
    pub introspection_config: String,
    /// The account that owns the API — this emulator's account id.
    pub owner: String,
    pub owner_contact: Option<String>,
    pub xray_enabled: bool,
    pub query_depth_limit: i32,
    pub resolver_count_limit: i32,
    pub merged_api_execution_role_arn: Option<String>,
    pub log_config: Option<LogConfig>,
    pub user_pool_config: Option<UserPoolConfig>,
    pub open_id_connect_config: Option<OpenIDConnectConfig>,
    pub lambda_authorizer_config: Option<LambdaAuthorizerConfig>,
    pub additional_authentication_providers: Vec<AdditionalAuthenticationProvider>,
    pub enhanced_metrics_config: Option<EnhancedMetricsConfig>,
    /// The API's DNS records — one hostname per endpoint, keyed as AppSync keys
    /// them (`GRAPHQL`, `REALTIME`). `uris` is derived from these, so the two
    /// members can never disagree about where the API is served.
    pub dns: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    /// The API's environmental variables, the whole list the last write left
    /// behind: a write REPLACES this map rather than merging into it.
    pub environment_variables: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// The API keys created under this API, keyed by key id.
    pub api_keys: BTreeMap<String, ApiKeyRecord>,
    /// The API's cache, or `None` while it has none: an API holds at most one
    /// `ApiCache` (`CreateApiCache` "Creates a cache for the GraphQL API", and
    /// `GetApiCache` resolves it from the `apiId` alone). Written through
    /// [`GraphqlApiRecord::put_api_cache`] and [`GraphqlApiRecord::take_api_cache`],
    /// which keep it and [`Self::api_cache_deleted`] in step.
    pub api_cache: Option<ApiCacheRecord>,
    /// Whether the empty cache slot above is empty because a TEARDOWN emptied it rather
    /// than because nothing ever filled it. The two are distinguishable to a caller — a
    /// call that finds a cache that was torn down is answered differently from one that
    /// finds an API that never had a cache — so the distinction is kept here rather than
    /// recomputed from anything the request carries. It reports whether a teardown of this
    /// API has removed a cache, so it is set by the teardown that removes one and cleared
    /// only by a create that fills the slot again; a repeated teardown removes nothing and
    /// leaves it alone.
    pub api_cache_deleted: bool,
    /// The data sources created under this API, keyed by their caller-supplied
    /// name — the name is what every data-source operation addresses one by, so it
    /// identifies the data source within its API and no second key is needed.
    /// Bounded: `CreateDataSource` refuses a create once the API holds the AppSync
    /// per-API quota of data sources, so this map cannot grow without limit.
    pub data_sources: BTreeMap<String, DataSourceRecord>,
    /// The functions created under this API, keyed by the opaque `functionId` the
    /// create minted for each — a function name "does not have to be unique"
    /// (`CreateFunctionRequest$name`), so the id is what identifies one within its
    /// API and the name cannot be the key.
    /// Bounded: `CreateFunction` refuses a create once the API holds the AppSync
    /// per-API quota of functions, so this map cannot grow without limit.
    pub functions: BTreeMap<String, FunctionRecord>,
    /// The GraphQL types created under this API, keyed by the type name their
    /// definition declares — the name is what every type operation addresses one by,
    /// so it identifies the type within its API and no second key is needed.
    /// Bounded: `CreateType` refuses a create once the API holds the AppSync per-API
    /// quota of types, so this map cannot grow without limit.
    pub types: BTreeMap<String, TypeRecord>,
    /// The resolvers attached to the fields of this API's types, keyed by the
    /// `typeName`/`fieldName` pair that addresses one ([`ResolverKey`]) — a field
    /// carries at most one resolver, so the pair identifies it within its API and no
    /// id is minted for it.
    /// Bounded: `CreateResolver` refuses a create once the API holds the AppSync
    /// per-API quota of resolvers, so this map cannot grow without limit.
    pub resolvers: BTreeMap<ResolverKey, ResolverRecord>,
    /// This API's schema creation, or `None` while no `StartSchemaCreation` has ever
    /// run for it: the document the last call carried, the `SchemaStatus` it left
    /// behind and the outcome the clock still owes ([`SchemaCreationRecord`]). A
    /// creation that succeeds installs the types its document declares into
    /// [`Self::types`], so the API's schema and its types are read under one lock and
    /// cannot disagree.
    pub schema: Option<SchemaCreationRecord>,
    /// The schema document the last SUCCESSFUL creation installed, or `None` while no
    /// creation of this API has ever succeeded.
    ///
    /// Kept beside [`Self::types`] because a schema document declares more than types: a
    /// `schema` definition binding the root operation types and the directives the
    /// schema defines declare no type, so they are not in that map, and the introspection
    /// schema reports them. It is written only when a creation SUCCEEDS, so a later
    /// document that failed to be created cannot be reported as this API's schema.
    pub schema_document: Option<String>,
}

impl GraphqlApiRecord {
    /// Store `cache` as this API's cache.
    ///
    /// The slot now holds an instance, so nothing about it was torn down: a create
    /// that fills the slot again leaves no deletion behind for a later read to report.
    pub fn put_api_cache(&mut self, cache: ApiCacheRecord) {
        self.api_cache = Some(cache);
        self.api_cache_deleted = false;
    }

    /// Tear down this API's cache, recording whether a cache of this API HAS been torn
    /// down.
    ///
    /// The slot is emptied either way. A teardown that found the slot already empty removed
    /// nothing, so it leaves the marker exactly as it was: a call that removes no instance
    /// changes nothing a later reader sees, and an API whose cache WAS deleted stays one
    /// whose cache was deleted however many times the teardown is repeated. Only a create
    /// that fills the slot again clears it ([`Self::put_api_cache`]).
    pub fn take_api_cache(&mut self) {
        if self.api_cache.take().is_some() {
            self.api_cache_deleted = true;
        }
    }

    /// Whether this API's cache slot is empty BECAUSE a teardown of it removed an
    /// instance — the one case a call reports as a torn-down cache rather than as one
    /// that was never configured.
    pub fn api_cache_was_deleted(&self) -> bool {
        self.api_cache.is_none() && self.api_cache_deleted
    }

    /// This record as the modelled `GraphqlApi` shape.
    ///
    /// The single projection used by create, get and list, so all three answer the
    /// same body for the same stored API.
    pub fn to_graphql_api(&self) -> GraphqlApi {
        GraphqlApi {
            name: Some(self.name.clone()),
            api_id: Some(self.api_id.clone()),
            authentication_type: Some(self.authentication_type.clone()),
            log_config: self.log_config.clone(),
            user_pool_config: self.user_pool_config.clone(),
            open_id_connect_config: self.open_id_connect_config.clone(),
            arn: Some(self.arn.clone()),
            uris: Some(uris_from_dns(&self.dns)),
            // The tag map is reported whether or not it holds tags, so a caller
            // reads the same member on every API. `TagMap`'s modelled minimum
            // length of 1 is not the rule for the RESPONSE: the recorded creates
            // this projection answers carry `tags` on an API created without any
            // (replaying them diverged at `$.graphqlApi.tags` while the member was
            // omitted, and stopped diverging once it was reported).
            tags: Some(map_out(&self.tags)),
            additional_authentication_providers: (!self
                .additional_authentication_providers
                .is_empty())
            .then(|| self.additional_authentication_providers.clone()),
            xray_enabled: self.xray_enabled,
            // Associating a WAF web ACL and a custom domain are their own
            // operations, still unimplemented, so nothing can have set either.
            waf_web_acl_arn: None,
            lambda_authorizer_config: self.lambda_authorizer_config.clone(),
            // "The DNS records for the API" — the hostnames the endpoints in
            // `uris` are served on.
            dns: Some(map_out(&self.dns)),
            visibility: Some(self.visibility.clone()),
            api_type: Some(self.api_type.clone()),
            merged_api_execution_role_arn: self.merged_api_execution_role_arn.clone(),
            owner: Some(self.owner.clone()),
            owner_contact: self.owner_contact.clone(),
            introspection_config: Some(self.introspection_config.clone()),
            query_depth_limit: self.query_depth_limit,
            resolver_count_limit: self.resolver_count_limit,
            enhanced_metrics_config: self.enhanced_metrics_config.clone(),
        }
    }

    /// This record as the reply to an UPDATE of the API reports it: the projection
    /// above WITHOUT the API's `dns` records.
    ///
    /// An update carries no member for the API's endpoints and cannot change them, so
    /// the reply to one reports the configuration the update left behind and not "the
    /// DNS records for the API" — which is what the recorded `UpdateGraphqlApi`
    /// replies carry (no `dns` member), while the create and the reads, whose replies
    /// are where a caller learns the endpoints, report them. The `uris` the endpoints
    /// are addressed through are reported on every reply alike.
    pub fn to_updated_graphql_api(&self) -> GraphqlApi {
        GraphqlApi {
            dns: None,
            ..self.to_graphql_api()
        }
    }
}

/// One stored `ApiCache` of a `GraphqlApiRecord`.
///
/// Every member of the modelled `ApiCache` shape the create call can set is kept, so
/// the create and `GetApiCache` project exactly what was created rather than a subset.
#[derive(Debug, Clone)]
pub struct ApiCacheRecord {
    pub ttl: i64,
    pub api_caching_behavior: String,
    pub r#type: String,
    pub transit_encryption_enabled: bool,
    pub at_rest_encryption_enabled: bool,
    pub health_metrics_config: Option<String>,
    /// The `ApiCacheStatus` the cache instance is in.
    pub status: String,
    /// The instant the cache the clock is provisioning (or re-provisioning) becomes
    /// available, or `None` once nothing is owed. Stored rather than slept on: see
    /// [`CACHE_SPAN`].
    pub available_at: Option<SystemTime>,
    /// The instant the cache's entries were last discarded, or `None` while nothing
    /// has discarded them. Not a member of the modelled `ApiCache`, so no operation
    /// projects it: it is read only by whatever serves an entry.
    ///
    /// A cache entry is a response the GraphQL endpoint served, so the entries are
    /// written by the data plane rather than by any control-plane call: what a flush
    /// leaves behind is this boundary, after which no entry cached before it is
    /// served. [`ApiCacheRecord::flush`] moves it; nothing else does.
    pub last_flushed_at: Option<SystemTime>,
}

impl ApiCacheRecord {
    /// Materialise the transition to [`CACHE_AVAILABLE`] the CLOCK already made due
    /// at `now`.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that started
    /// the provisioning, so the same transition is applied whether or not anybody read
    /// the cache in between.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.available_at else {
            return;
        };
        if now < due {
            return;
        }
        self.available_at = None;
        self.status = CACHE_AVAILABLE.to_string();
    }

    /// Whether the instance was ever created, i.e. whether the create that minted it
    /// ran to completion.
    ///
    /// `false` only for the two statuses the model defines as the creation itself —
    /// [`CACHE_CREATING`] ("The instance is currently creating") and [`CACHE_FAILED`]
    /// ("The instance has failed creation"). Every other status belongs to an instance
    /// that exists, whatever the service is doing to it at the moment.
    pub fn was_created(&self) -> bool {
        self.status != CACHE_CREATING && self.status != CACHE_FAILED
    }

    /// Discard every entry the cache holds, as of `now`.
    ///
    /// The instance itself is untouched — its settings and the status it is in are
    /// properties of the instance, not of the entries it holds — so the only thing a
    /// flush leaves behind is the new entry boundary ([`Self::last_flushed_at`]).
    pub fn flush(&mut self, now: SystemTime) {
        self.last_flushed_at = Some(now);
    }

    /// This record as the modelled `ApiCache` shape — the single projection the
    /// create and the read both answer with.
    pub fn to_api_cache(&self) -> ApiCache {
        ApiCache {
            ttl: self.ttl,
            api_caching_behavior: Some(self.api_caching_behavior.clone()),
            transit_encryption_enabled: self.transit_encryption_enabled,
            at_rest_encryption_enabled: self.at_rest_encryption_enabled,
            r#type: Some(self.r#type.clone()),
            status: Some(self.status.clone()),
            health_metrics_config: self.health_metrics_config.clone(),
        }
    }
}

/// One stored API key of an API, of either resource family — a
/// [`GraphqlApiRecord`] or an [`EventApiRecord`].
#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    /// Creation order within the parent API, for a stable `ListApiKeys` page order.
    pub seq: u64,
    pub id: String,
    pub description: Option<String>,
    /// Epoch seconds, floored to the hour — the model documents both `expires` and
    /// `deletes` as "seconds since the epoch, rounded down to the nearest hour".
    pub expires: i64,
    pub deletes: i64,
}

impl ApiKeyRecord {
    /// A key record with `expires` as the call that minted it asked for, set through
    /// [`ApiKeyRecord::set_expiration`] so a fresh key's two time members are built by
    /// the same rule an update applies to them.
    pub fn new(seq: u64, id: String, description: Option<String>, expires: i64) -> Self {
        let mut record = Self {
            seq,
            id,
            description,
            expires: 0,
            deletes: 0,
        };
        record.set_expiration(expires);
        record
    }

    /// This record as the modelled `ApiKey` shape.
    pub fn to_api_key(&self) -> ApiKey {
        ApiKey {
            id: Some(self.id.clone()),
            description: self.description.clone(),
            expires: self.expires,
            deletes: self.deletes,
        }
    }

    /// Set the key's expiration, in epoch seconds, to the instant the call that sets
    /// it asked for, and move the key's deletion time onto it.
    ///
    /// Both `expires` and `deletes` are documented as "seconds since the epoch,
    /// rounded down to the nearest hour", so the instant is floored here rather than
    /// by each caller. The deletion time is derived from the expiration by adding the
    /// post-expiry retention the service keeps an expired key for
    /// ([`API_KEY_RETENTION_SECONDS`]), so every operation that reports a key — the
    /// create and the update that write it, and the listing that pages over it —
    /// reports one value per key.
    pub fn set_expiration(&mut self, expires: i64) {
        self.expires = floor_to_hour(expires);
        // Floored again so the documented form of `deletes` holds whatever the
        // retention span is, rather than only while it is a whole number of hours.
        self.deletes = floor_to_hour(self.expires.saturating_add(API_KEY_RETENTION_SECONDS));
    }
}

/// One hour in seconds: the granularity `ApiKey$expires` and `ApiKey$deletes` are
/// documented with — both are "seconds since the epoch, rounded down to the nearest
/// hour".
pub const SECONDS_PER_HOUR: i64 = 3_600;

/// One day in seconds: the unit the API-key validity window is stated in.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// The shortest validity an API key accepts, measured from the call that sets it.
///
/// Source: `ApiKeyValidityOutOfBoundsException` — "The API key expiration must be set
/// to a value between 1 and 365 days from creation (for CreateApiKey) or from update
/// (for UpdateApiKey)".
pub const API_KEY_MIN_VALIDITY_SECONDS: i64 = SECONDS_PER_DAY;

/// The longest validity an API key accepts ([`API_KEY_MIN_VALIDITY_SECONDS`] states
/// the source).
pub const API_KEY_MAX_VALIDITY_SECONDS: i64 = 365 * SECONDS_PER_DAY;

/// How long an API key is kept after it expires, before the service reclaims it —
/// the span between a key's `expires` and its `deletes`.
///
/// Source: the `ApiKey` shape's own documentation for the `da2` key version this
/// service mints — "Expired API keys are kept for 60 days after the expiration time"
/// (`smithy_query(query="doc", shape="ApiKey")`), which `ListApiKeys` repeats as "API
/// keys are deleted automatically 60 days after they expire".
pub const API_KEY_RETENTION_SECONDS: i64 = 60 * SECONDS_PER_DAY;

/// Whether `validity` seconds is inside the documented API-key validity window
/// ([`API_KEY_MIN_VALIDITY_SECONDS`]..=[`API_KEY_MAX_VALIDITY_SECONDS`]).
///
/// The window is measured from the call that sets the expiration, so a create and an
/// update judge the same span against the same bounds.
pub fn is_api_key_validity(validity: i64) -> bool {
    (API_KEY_MIN_VALIDITY_SECONDS..=API_KEY_MAX_VALIDITY_SECONDS).contains(&validity)
}

/// AppSync's fixed text for a validity outside that window, carried by
/// `ApiKeyValidityOutOfBoundsException` (HTTP 400): the bound the validity crossed,
/// stated in days, so the caller can correct the value from the message alone. The
/// rejected value is not echoed — the bound is what the caller has to stay within.
pub fn api_key_validity_out_of_bounds_message(validity: i64) -> String {
    if validity < API_KEY_MIN_VALIDITY_SECONDS {
        format!(
            "API key must be valid for a minimum of {} days.",
            API_KEY_MIN_VALIDITY_SECONDS / SECONDS_PER_DAY
        )
    } else {
        format!(
            "API key can be valid for a maximum of {} days.",
            API_KEY_MAX_VALIDITY_SECONDS / SECONDS_PER_DAY
        )
    }
}

/// `seconds` rounded DOWN to the nearest hour, the form both `ApiKey$expires` and
/// `ApiKey$deletes` are documented to take.
pub fn floor_to_hour(seconds: i64) -> i64 {
    seconds.saturating_sub(seconds.rem_euclid(SECONDS_PER_HOUR))
}

/// Now, as whole seconds since the Unix epoch — the form an API key's expiration takes.
pub fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The fixed half of the text `NotFoundException` (HTTP 404) carries for an API key its
/// parent API does not hold ([`api_key_not_found_message`] fills the rest).
pub const API_KEY_NOT_FOUND_PREFIX: &str = "API key not found: ";

/// The text `NotFoundException` (HTTP 404) carries for an API key its parent API does
/// not hold: AppSync's fixed sentence ([`API_KEY_NOT_FOUND_PREFIX`]) followed by the key
/// id the request addressed, so a caller deleting or updating several keys can see WHICH
/// id named nothing. Shared by every operation that addresses a key by id, so they word
/// one absence one way.
pub fn api_key_not_found_message(id: &str) -> String {
    format!("{API_KEY_NOT_FOUND_PREFIX}{id}")
}

/// The key version this service mints, and the only one whose keys an update is
/// available for.
///
/// Source: the `ApiKey` shape's own documentation (`smithy_query(query="doc",
/// shape="ApiKey")`), which describes two key versions — `da1`, introduced in November
/// 2017, which "ceased to be valid after February 21, 2018" and for which "UpdateApiKey
/// is not available", and `da2`, introduced in February 2018, whose `UpdateApiKey`
/// "accepts a user-provided expiration time in seconds". A key id states its version in
/// the segment before its first `-`, which is how a key is classified by version before
/// anything is resolved.
pub const CURRENT_API_KEY_VERSION: &str = "da2";

/// Whether `id` states the key version an update is available for
/// ([`CURRENT_API_KEY_VERSION`]), i.e. whether it could name a key an update can reach
/// at all.
///
/// The version is the id's leading segment, so the separator is part of the test: an id
/// that states another version, or no version at all, states no version an update
/// serves.
pub fn is_updatable_api_key_version(id: &str) -> bool {
    id.strip_prefix(CURRENT_API_KEY_VERSION)
        .is_some_and(|rest| rest.starts_with('-'))
}

/// AppSync's fixed text for a call that addresses a key of a version it is not available
/// for, carried by `BadRequestException` (HTTP 400): the request names a key version the
/// operation cannot serve, and no part of the sentence varies with the request.
pub const UNSUPPORTED_API_KEY_VERSION_MESSAGE: &str = "Unsupported API key version.";

/// One stored data source of a `GraphqlApiRecord`.
///
/// Every member of the modelled `DataSource` shape that a create or update call can
/// set is kept, including each per-type configuration, so a reader projects exactly
/// what was written rather than a subset. `description` is kept as an `Option`, so a
/// data source that has no description reports none rather than an empty one — see
/// [`data_source_description`] for which requests leave it with none.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// The `DataSourceType` this data source is of.
    pub r#type: String,
    pub service_role_arn: Option<String>,
    pub dynamodb_config: Option<DynamodbDataSourceConfig>,
    pub lambda_config: Option<LambdaDataSourceConfig>,
    pub elasticsearch_config: Option<ElasticsearchDataSourceConfig>,
    pub open_search_service_config: Option<OpenSearchServiceDataSourceConfig>,
    pub http_config: Option<HttpDataSourceConfig>,
    pub relational_database_config: Option<RelationalDatabaseDataSourceConfig>,
    pub event_bridge_config: Option<EventBridgeDataSourceConfig>,
    /// The `DataSourceLevelMetricsConfig` the request asked for, or `None` when it
    /// asked for none: the model documents no default for it.
    pub metrics_config: Option<String>,
}

impl DataSourceRecord {
    /// The configuration member this data source's `type` is set up through and which
    /// it does not carry, or `None` when the type carries it (or needs none at all).
    ///
    /// Source: the per-type configuration members of `CreateDataSourceRequest` /
    /// `UpdateDataSourceRequest` and the `DataSourceType` values they configure —
    /// `dynamodbConfig` ("Amazon DynamoDB settings"), `lambdaConfig` ("Lambda
    /// settings"), `elasticsearchConfig` / `openSearchServiceConfig` ("Amazon
    /// OpenSearch Service settings"), `httpConfig` ("HTTP endpoint settings"),
    /// `relationalDatabaseConfig` ("Relational database settings") and
    /// `eventBridgeConfig` ("Amazon EventBridge settings"). `NONE` and
    /// `AMAZON_BEDROCK_RUNTIME` are configured by no member of either request, so a
    /// data source of either type is complete without one.
    pub fn missing_config(&self) -> Option<&'static str> {
        match self.r#type.as_str() {
            "AMAZON_DYNAMODB" if self.dynamodb_config.is_none() => Some("dynamodbConfig"),
            "AWS_LAMBDA" if self.lambda_config.is_none() => Some("lambdaConfig"),
            "AMAZON_ELASTICSEARCH" if self.elasticsearch_config.is_none() => {
                Some("elasticsearchConfig")
            }
            "AMAZON_OPENSEARCH_SERVICE" if self.open_search_service_config.is_none() => {
                Some("openSearchServiceConfig")
            }
            "HTTP" if self.http_config.is_none() => Some("httpConfig"),
            "RELATIONAL_DATABASE" if self.relational_database_config.is_none() => {
                Some("relationalDatabaseConfig")
            }
            "AMAZON_EVENTBRIDGE" if self.event_bridge_config.is_none() => Some("eventBridgeConfig"),
            _ => None,
        }
    }

    /// This record as the modelled `DataSource` shape.
    ///
    /// The single projection every operation that reports a data source answers with,
    /// so the create and the reads cannot disagree about one data source.
    pub fn to_data_source(&self) -> DataSource {
        DataSource {
            data_source_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            r#type: Some(self.r#type.clone()),
            service_role_arn: self.service_role_arn.clone(),
            dynamodb_config: self.dynamodb_config.clone(),
            lambda_config: self.lambda_config.clone(),
            elasticsearch_config: self.elasticsearch_config.clone(),
            open_search_service_config: self.open_search_service_config.clone(),
            http_config: self.http_config.clone(),
            relational_database_config: self.relational_database_config.clone(),
            event_bridge_config: self.event_bridge_config.clone(),
            metrics_config: self.metrics_config.clone(),
        }
    }
}

/// The description a data source keeps for the `description` a request carried.
///
/// An empty description describes nothing, so no description is kept for it: a data
/// source whose request left `description` empty reports the member absent, exactly
/// as one whose request omitted it. (The recorded creates this projection answers
/// report no description for an explicitly empty one.)
pub fn data_source_description(description: Option<String>) -> Option<String> {
    description.filter(|description| !description.is_empty())
}

/// The resource portion of a data source's ARN: a data source is a child of its API,
/// so it is named under the API's own `apis/{apiId}` path, as `GetDataSource`'s URI
/// (`/v1/apis/{apiId}/datasources/{name}`) addresses it.
pub fn data_source_resource(api_id: &str, name: &str) -> String {
    format!("apis/{api_id}/datasources/{name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a data source the named API
/// does not hold: the absent data source, which is what the caller has to act on.
///
/// The requirement the refusal answers is the status and the error shape; the wording
/// is the one a recorded `GetDataSource` on a name no create left behind answers with,
/// which is a fixed template over the requested name rather than anything of the
/// request's own values.
pub fn data_source_not_found_message(name: &str) -> String {
    format!("Data source {name} not found")
}

/// The text a `BadRequestException` (HTTP 400) carries for a create whose name is
/// already taken within the API: the name, which is the member the caller has to
/// change.
pub fn data_source_exists_message(name: &str) -> String {
    format!("Data source with name {name} already exists.")
}

/// The text a `BadRequestException` (HTTP 400) carries for a data source whose type
/// is configured through a member the request did not carry
/// ([`DataSourceRecord::missing_config`]): the backend that configuration describes,
/// so the caller can fix the request from the message alone.
///
/// The backend each member names is the subject of that member's own doc on
/// `CreateDataSourceRequest` / `UpdateDataSourceRequest` — `lambdaConfig` "the new
/// Lambda configuration", `dynamodbConfig` "the new Amazon DynamoDB configuration",
/// `elasticsearchConfig` and `openSearchServiceConfig` "the new OpenSearch
/// configuration", `httpConfig` "the new HTTP endpoint configuration",
/// `relationalDatabaseConfig` "the new relational database configuration" and
/// `eventBridgeConfig` "the new Amazon EventBridge settings". A configuration member
/// added to the request later falls back to its own name.
pub fn missing_data_source_config_message(member: &str) -> String {
    let subject = match member {
        "lambdaConfig" => "Lambda",
        "dynamodbConfig" => "Amazon DynamoDB",
        "elasticsearchConfig" | "openSearchServiceConfig" => "OpenSearch",
        "httpConfig" => "HTTP endpoint",
        "relationalDatabaseConfig" => "relational database",
        "eventBridgeConfig" => "Amazon EventBridge",
        other => other,
    };
    format!("Missing {subject} data source config")
}

/// One stored function of a `GraphqlApiRecord`: "A function is a reusable entity. You
/// can use multiple functions to compose the resolver logic."
///
/// Every member of the modelled `FunctionConfiguration` shape a create call can set is
/// kept, so a reader projects exactly what was written rather than a subset.
#[derive(Debug, Clone)]
pub struct FunctionRecord {
    /// Creation order, used to keep a listing (and its `nextToken` cursor) stable
    /// across calls: the minted `functionId` is opaque, so it cannot carry order.
    pub seq: u64,
    pub function_id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub data_source_name: String,
    pub request_mapping_template: Option<String>,
    pub response_mapping_template: Option<String>,
    pub function_version: Option<String>,
    pub sync_config: Option<SyncConfig>,
    pub max_batch_size: i32,
    pub runtime: Option<AppSyncRuntime>,
    pub code: Option<String>,
}

impl FunctionRecord {
    /// This record as the modelled `FunctionConfiguration` shape.
    ///
    /// The single projection every operation that reports a function answers with, so
    /// the create and the reads cannot disagree about one function.
    pub fn to_function_configuration(&self) -> FunctionConfiguration {
        FunctionConfiguration {
            function_id: Some(self.function_id.clone()),
            function_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            data_source_name: Some(self.data_source_name.clone()),
            request_mapping_template: self.request_mapping_template.clone(),
            response_mapping_template: self.response_mapping_template.clone(),
            function_version: self.function_version.clone(),
            sync_config: self.sync_config.clone(),
            max_batch_size: self.max_batch_size,
            runtime: self.runtime.clone(),
            code: self.code.clone(),
        }
    }
}

/// The resource portion of a function's ARN: a function is a child of its API, so it
/// is named under the API's own `apis/{apiId}` path, as `GetFunction`'s URI
/// (`/v1/apis/{apiId}/functions/{functionId}`) addresses it.
pub fn function_resource(api_id: &str, function_id: &str) -> String {
    format!("apis/{api_id}/functions/{function_id}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a function the named API does
/// not hold: AppSync's own wording, a fixed template whose only variable part is the
/// `functionId` the request asked for, which is what the caller has to act on.
///
/// Shared by every operation that addresses a function by id (`GetFunction`,
/// `UpdateFunction`, `DeleteFunction`), so the three cannot disagree about how a
/// missing function is reported.
pub fn function_not_found_message(function_id: &str) -> String {
    format!("function id {function_id} not found")
}

/// The refusal text for a function definition AppSync could not run, or `None` when
/// the request describes one it could.
///
/// Three documented requirements, each over the members that carry the definition
/// rather than over one request's values:
///
/// * `code` "When code is used, the runtime is required";
/// * `runtime` "if a runtime is specified, code must also be specified";
/// * `functionVersion` "when using VTL and mapping templates, the functionVersion is
///   required" — a function that carries no `code` is defined in VTL, so it is the
///   one that needs the template version.
///
/// Shared by every operation that takes this member set (`CreateFunction` and
/// `UpdateFunction`), so the two cannot disagree about which definitions are runnable.
pub fn function_definition_refusal(
    code: Option<&str>,
    runtime: Option<&AppSyncRuntime>,
    function_version: Option<&str>,
) -> Option<String> {
    match (code, runtime) {
        (None, None) if function_version.is_none_or(str::is_empty) => Some(
            "Invalid functionVersion given. A functionVersion is required when using mapping \
             templates."
                .to_string(),
        ),
        pair => code_runtime_refusal(pair.0, pair.1),
    }
}

/// The refusal text for a `code`/`runtime` pair that defines nothing AppSync could run,
/// or `None` when the two agree.
///
/// Two documented requirements, both over the pair rather than over one request's
/// values, and both carried by every operation that takes the two members (a function's
/// `code` and a resolver's alike):
///
/// * "When code is used, the runtime is required";
/// * "if a runtime is specified, code must also be specified".
pub fn code_runtime_refusal(code: Option<&str>, runtime: Option<&AppSyncRuntime>) -> Option<String> {
    match (code, runtime) {
        (Some(_), None) => Some(
            "Invalid runtime given. A runtime is required when code is specified.".to_string(),
        ),
        (None, Some(_)) => Some(
            "Invalid code given. Code is required when a runtime is specified.".to_string(),
        ),
        (Some(_), Some(_)) | (None, None) => None,
    }
}

/// One stored `Api`: an AppSync API created with an Event API configuration
/// ("Describes an AppSync API. You can use `Api` for an AppSync API with your
/// preferred configuration, such as an Event API").
///
/// An `Api` is a resource of its own, kept apart from [`GraphqlApiRecord`]: the two
/// are addressed by different operations under different URI prefixes, so an id that
/// names one never names the other.
#[derive(Debug, Clone)]
pub struct EventApiRecord {
    /// Creation order, used to keep `ListApis` (and its `nextToken` cursor) stable
    /// across calls: the minted `apiId` is opaque, so it cannot carry order.
    pub seq: u64,
    pub api_id: String,
    pub name: String,
    pub api_arn: String,
    pub owner_contact: Option<String>,
    pub xray_enabled: bool,
    /// The web ACL associated with the API, or `None` while it has none.
    pub waf_web_acl_arn: Option<String>,
    /// The API's Event API configuration — its authorization providers and the
    /// authorization modes connecting, publishing and subscribing accept.
    pub event_config: EventConfig,
    /// The API's DNS records — one hostname per endpoint, keyed as AppSync keys
    /// them (`HTTP`, `REALTIME`): `Api$dns` "will include an HTTP and a real-time
    /// endpoint".
    pub dns: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub created: SystemTime,
    /// The channel namespaces created under this API, keyed by their caller-supplied
    /// name — a namespace "name must be unique within the `Api`", and the name is what
    /// every channel-namespace operation addresses one by, so no second key is needed.
    /// Bounded: `CreateChannelNamespace` refuses a create once the API holds the
    /// AppSync per-API quota of channel namespaces, so this map cannot grow without
    /// limit.
    pub channel_namespaces: BTreeMap<String, ChannelNamespaceRecord>,
    /// The API keys created under this API, keyed by key id — the same resource a
    /// `GraphqlApiRecord` holds, since the API-key operations address an API of
    /// either family by the same `apiId` label (see
    /// [`ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::api_keys_of`]).
    /// Bounded: `CreateApiKey` refuses a create once the API holds the AppSync
    /// per-API quota of keys, so this map cannot grow without limit.
    pub api_keys: BTreeMap<String, ApiKeyRecord>,
}

impl EventApiRecord {
    /// This record as the modelled `Api` shape.
    ///
    /// The single projection used by create, get and list, so all three answer the
    /// same body for the same stored API.
    pub fn to_api(&self) -> Api {
        Api {
            api_id: Some(self.api_id.clone()),
            name: Some(self.name.clone()),
            owner_contact: self.owner_contact.clone(),
            // Reported whether or not the API holds tags, the way
            // [`GraphqlApiRecord::to_graphql_api`] reports them, so a caller reads
            // the same member on every API of either kind.
            tags: Some(map_out(&self.tags)),
            dns: Some(map_out(&self.dns)),
            api_arn: Some(self.api_arn.clone()),
            created: Some(::aws_smithy_types::DateTime::from(self.created)),
            xray_enabled: self.xray_enabled,
            waf_web_acl_arn: self.waf_web_acl_arn.clone(),
            event_config: Some(self.event_config.clone()),
        }
    }
}

/// One stored channel namespace of an [`EventApiRecord`]: "Describes a channel
/// namespace associated with an `Api`. The `ChannelNamespace` contains the definitions
/// for code handlers for the `Api`."
///
/// Every member of the modelled `ChannelNamespace` shape a create or update call can
/// set is kept, so every reader projects exactly what was written rather than a
/// subset. No creation-order field is needed: a namespace is addressed by a name
/// unique within its `Api`, so the map key is itself the order a listing pages in.
#[derive(Debug, Clone)]
pub struct ChannelNamespaceRecord {
    /// The id of the `Api` this namespace belongs to, which the projection reports.
    pub api_id: String,
    pub name: String,
    pub channel_namespace_arn: String,
    /// The namespace's own authorization overrides, empty where it overrides nothing
    /// and the `Api`'s default configuration applies.
    pub subscribe_auth_modes: Vec<AuthMode>,
    pub publish_auth_modes: Vec<AuthMode>,
    /// "The event handler functions that run custom business logic to process
    /// published events and subscribe requests", or `None` when the namespace carries
    /// no handler code.
    pub code_handlers: Option<String>,
    pub handler_configs: Option<HandlerConfigs>,
    pub tags: BTreeMap<String, String>,
    pub created: SystemTime,
    /// When the namespace was last changed: its creation instant until an update
    /// replaces its configuration.
    pub last_modified: SystemTime,
}

impl ChannelNamespaceRecord {
    /// This record as the modelled `ChannelNamespace` shape, code handlers included.
    ///
    /// The single projection the create, the update and `GetChannelNamespace` answer
    /// with, so none of the three can disagree about one namespace.
    ///
    /// The namespace's COLLECTION members — its two authorization overrides and its
    /// handler configuration — are reported whether or not the namespace configures
    /// any of them, as the empty collection that says "nothing configured": a caller
    /// reads `handlerConfigs.onPublish` or iterates `publishAuthModes` on every
    /// namespace rather than having to tell an unconfigured member from one the reply
    /// left out. That is what the recorded creates, updates and reads this projection
    /// answers carry for a namespace with no handlers and no override (`{}` and `[]`),
    /// and it is the same rule the tag map above already follows.
    pub fn to_channel_namespace(&self) -> ChannelNamespace {
        ChannelNamespace {
            api_id: Some(self.api_id.clone()),
            name: Some(self.name.clone()),
            // An empty list overrides nothing, which is what a namespace that leaves
            // the `Api`'s default authorization configuration in place has.
            subscribe_auth_modes: Some(self.subscribe_auth_modes.clone()),
            publish_auth_modes: Some(self.publish_auth_modes.clone()),
            code_handlers: self.code_handlers.clone(),
            // Reported whether or not the namespace holds tags, the way
            // [`EventApiRecord::to_api`] reports an API's.
            tags: Some(map_out(&self.tags)),
            channel_namespace_arn: Some(self.channel_namespace_arn.clone()),
            created: Some(::aws_smithy_types::DateTime::from(self.created)),
            last_modified: Some(::aws_smithy_types::DateTime::from(self.last_modified)),
            // An empty `HandlerConfigs` carries neither an `onPublish` nor an
            // `onSubscribe` handler, which is what a namespace that configures no
            // handler has.
            handler_configs: Some(self.handler_configs.clone().unwrap_or_default()),
        }
    }

    /// This record as a listing reports it: the projection above WITHOUT the code
    /// handlers, because `ListChannelNamespaces` "returns only high level details for
    /// the channel namespace. To retrieve code handlers, use `GetChannelNamespace`".
    pub fn to_channel_namespace_summary(&self) -> ChannelNamespace {
        ChannelNamespace {
            code_handlers: None,
            ..self.to_channel_namespace()
        }
    }

    /// Replace this namespace's configuration with what an update call carried, and
    /// record that the change happened at `now`.
    ///
    /// Every settable member of the update request is the namespace's new value, so
    /// the update REPLACES the stored configuration rather than merging into it: a
    /// member the request omits is gone from the namespace. The identity (the name,
    /// the ARN, the owning API and the creation instant) is not settable and is kept.
    pub fn apply_update(
        &mut self,
        publish_auth_modes: Vec<AuthMode>,
        subscribe_auth_modes: Vec<AuthMode>,
        code_handlers: Option<String>,
        handler_configs: Option<HandlerConfigs>,
        now: SystemTime,
    ) {
        self.publish_auth_modes = publish_auth_modes;
        self.subscribe_auth_modes = subscribe_auth_modes;
        self.code_handlers = code_handlers;
        self.handler_configs = handler_configs;
        self.last_modified = now;
    }
}

/// The resource portion of an API's ARN — a GraphQL API's and an Event API's alike, both
/// of which are addressed under the one `apis/{apiId}` path their operations' URIs spell
/// (`/v1/apis/{apiId}`, `/v2/apis/{apiId}`).
///
/// The one spelling of it, so the ARN a create mints, the ARN a child resource is named
/// under and the ARN a request is authorized against cannot drift apart.
pub fn api_resource(api_id: &str) -> String {
    format!("apis/{api_id}")
}

/// The resource portion of a channel namespace's ARN: a namespace is a child of its
/// `Api`, so it is named under that API's own `apis/{apiId}` path, as
/// `GetChannelNamespace`'s URI (`/v2/apis/{apiId}/channelNamespaces/{name}`) addresses
/// it.
pub fn channel_namespace_resource(api_id: &str, name: &str) -> String {
    format!("{}/channelNamespace/{name}", api_resource(api_id))
}

/// How many channel namespaces one `Api` holds in this emulator, which bounds the
/// namespaces a single [`EventApiRecord`] stores: a create past it is refused with the
/// modelled `ServiceQuotaExceededException`, so repeated creates cannot grow the store
/// without limit. Set at AppSync's published quota of channel namespaces per Event
/// API.
pub const CHANNEL_NAMESPACES_PER_API: usize = 50;

/// The text a `NotFoundException` (HTTP 404) carries for a channel namespace the
/// addressed `Api` does not hold, on the path that REPORTS one.
///
/// A fixed sentence: the recordings this answers pin AppSync's own wording, which
/// states the absent resource and names neither the namespace nor the API. The
/// MUTATING paths word the same absence differently — they carry no message at all
/// (`UpdateChannelNamespace`, `DeleteChannelNamespace`) — so this is the read path's
/// text rather than the resource's.
pub const CHANNEL_NAMESPACE_NOT_FOUND_MESSAGE: &str = "Channel namespace not found.";

/// The text a `ConflictException` (HTTP 409) carries for a create whose name is already
/// taken within the `Api`.
///
/// A namespace's "name must be unique within the `Api`", so a second create under a
/// name the API already holds conflicts with the namespace that holds it rather than
/// being malformed. A fixed sentence, as the recordings this answers word it.
pub const CHANNEL_NAMESPACE_EXISTS_MESSAGE: &str = "Channel namespace already exists.";

/// The authorization types an Event API configuration sets up, i.e. the types its
/// authorization modes may name.
///
/// A provider identifies ONE way of authenticating the API, so the set holds at most
/// one entry per value of the modelled `AuthenticationType` enum.
pub fn configured_auth_types(event_config: &EventConfig) -> std::collections::BTreeSet<String> {
    event_config
        .auth_providers
        .iter()
        .map(|provider| provider.auth_type.clone())
        .collect()
}

/// The first mode of `modes` whose `authType` is not among `configured`, or `None`
/// when every one of them is.
pub fn unconfigured_auth_mode<'a>(
    modes: &'a [AuthMode],
    configured: &std::collections::BTreeSet<String>,
) -> Option<&'a str> {
    modes
        .iter()
        .find(|mode| !configured.contains(&mode.auth_type))
        .map(|mode| mode.auth_type.as_str())
}

/// The text a `BadRequestException` (HTTP 400) carries for an authorization mode that
/// names a type the API's authorization providers do not set up: the member that
/// carried it and the type, so the caller can fix the request from the message alone.
///
/// The wording of a CHANNEL NAMESPACE's own overrides
/// ([`channel_namespace_auth_modes_refusal`]). The API's own configuration states the
/// refusal differently — a fixed sentence naming the call the list authorizes
/// ([`invalid_event_config_auth_message`]) — because that is what the two surfaces'
/// recordings show.
pub fn unconfigured_auth_mode_message(member: &str, auth_type: &str) -> String {
    format!(
        "{member} specifies authorization mode {auth_type}, which is not configured as an \
         authorization provider for this API."
    )
}

/// The text a `BadRequestException` (HTTP 400) carries for an `EventConfig` authorization
/// mode list that names a type the API's own providers do not set up: a FIXED sentence
/// naming the CALL that list authorizes — connecting, publishing or subscribing — and
/// nothing of the request, so neither the member nor the rejected type is echoed.
///
/// AppSync states this one refusal for all three lists, distinguished only by the call:
/// the recorded refusal of an Event API whose connect list names an unconfigured type
/// reads `Event API configuration connect auth contains an invalid auth type.`, and the
/// other two lists name their own call in its place — RECORDED for `connect` only, so a
/// later recording of either of the other two is checked against this template and, if it
/// words them differently, corrected on the one table that pairs a list with its call
/// ([`event_config_refusal`]). The three calls, in the order the configuration is checked
/// in, are `connect` (`connectionAuthModes`), `publish` (`defaultPublishAuthModes`) and
/// `subscribe` (`defaultSubscribeAuthModes`) — the model's three mode lists
/// (`smithy_query(query="shape", shape="EventConfig")`), each "a list of valid
/// authorization modes" for one call on the API.
pub fn invalid_event_config_auth_message(call: &str) -> String {
    format!("Event API configuration {call} auth contains an invalid auth type.")
}

/// Why the Event API configuration a request carries is not well formed, as the text a
/// `BadRequestException` (HTTP 400) states it with, or `None` when it is well formed.
///
/// `EventConfig` is carried by the create and by the update alike, so the rules are
/// stated here once:
///
/// * an authorization type that is set up through a member of its own
///   ([`missing_auth_config`]) is not configured without that member — checked per
///   provider, which the refusal names by its 1-based position, the only way a caller
///   can tell two of them apart;
/// * an authorization type identifies ONE provider, so the API cannot name it twice;
/// * an OIDC provider is identified by its issuer, an absolute URL;
/// * `connectionAuthModes`, `defaultPublishAuthModes` and `defaultSubscribeAuthModes`
///   are each "a list of VALID authorization modes", i.e. modes the API's own
///   authorization providers set up — refused by the CALL the list authorizes
///   ([`invalid_event_config_auth_message`]).
pub fn event_config_refusal(event_config: &EventConfig) -> Option<String> {
    let mut configured: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (position, provider) in event_config.auth_providers.iter().enumerate() {
        if let Some(config) = missing_auth_config(
            Some(&provider.auth_type),
            provider.open_id_connect_config.is_some(),
            provider.cognito_config.is_some(),
            provider.lambda_authorizer_config.is_some(),
        ) {
            return Some(format!(
                "{config} is missing for authorization provider {}",
                position + 1
            ));
        }
        if provider
            .open_id_connect_config
            .as_ref()
            .is_some_and(|config| !is_absolute_url(&config.issuer))
        {
            return Some("Issuer is not a valid URL".to_string());
        }
        if !configured.insert(provider.auth_type.clone()) {
            return Some(format!(
                "Authentication type {} for authorization provider {} already specified on the \
                 API. It can only be specified once.",
                provider.auth_type,
                position + 1
            ));
        }
    }

    // One table of the three `EventConfig` mode lists, each beside the CALL it
    // authorizes ([`invalid_event_config_auth_message`]), in the order the configuration
    // is checked in — so a list and the call its refusal names cannot drift apart.
    let mode_lists = [
        ("connect", &event_config.connection_auth_modes),
        ("publish", &event_config.default_publish_auth_modes),
        ("subscribe", &event_config.default_subscribe_auth_modes),
    ];
    for (call, modes) in mode_lists {
        if unconfigured_auth_mode(modes, &configured).is_some() {
            return Some(invalid_event_config_auth_message(call));
        }
    }

    None
}

/// The text a `BadRequestException` (HTTP 400) carries for a channel namespace whose
/// authorization override names a type the parent API's authorization providers do not
/// set up.
///
/// AppSync states this one as a rule about the namespace rather than about the member
/// that carried the mode, so the sentence is fixed and has no variable part: the caller
/// is told the auth type has to be one the API offers. It is the sentence a recorded
/// `CreateChannelNamespace` whose `publishAuthModes`/`subscribeAuthModes` name a type the
/// parent `Api`'s `eventConfig.authProviders` does not carry is refused with. (The API
/// configuration's OWN mode lists are a different error class, worded by
/// [`unconfigured_auth_mode_message`].)
pub const CHANNEL_NAMESPACE_AUTH_MODE_MESSAGE: &str =
    "Channel namespace auth type must be present in the API's auth providers.";

/// Why the authorization overrides a channel-namespace create or update carries are
/// not well formed, as the text a `BadRequestException` (HTTP 400) states it with, or
/// `None` when they are.
///
/// A namespace's `publishAuthModes` and `subscribeAuthModes` are authorization modes
/// of the same kind the API's own configuration carries, and they "override the default
/// `Api` authorization configuration" rather than extend it, so the rule the API's own
/// mode lists follow holds for them too: a mode is valid only where the API's
/// authorization providers set its type up. Both the create and the update carry the
/// two members, so the rule lives here once; `configured` is the API's own
/// [`configured_auth_types`], taken by the caller so the check needs no borrow of the
/// API itself.
pub fn channel_namespace_auth_modes_refusal(
    configured: &std::collections::BTreeSet<String>,
    publish_auth_modes: &[AuthMode],
    subscribe_auth_modes: &[AuthMode],
) -> Option<String> {
    [publish_auth_modes, subscribe_auth_modes]
        .into_iter()
        .any(|modes| unconfigured_auth_mode(modes, configured).is_some())
        .then(|| CHANNEL_NAMESPACE_AUTH_MODE_MESSAGE.to_string())
}

/// One stored custom domain name, plus the API association it carries.
///
/// Every member of the modelled `DomainNameConfig` shape is kept, so the create and
/// `GetDomainName` project exactly what was created rather than a subset. The API
/// association lives ON the domain because a custom domain carries at most one — every
/// association operation addresses it by the domain alone
/// (`/v1/domainnames/{domainName}/apiassociation`).
#[derive(Debug, Clone)]
pub struct DomainNameRecord {
    /// Creation order, so a listing over domains has a stable page order.
    pub seq: u64,
    pub domain_name: String,
    pub description: Option<String>,
    pub certificate_arn: String,
    /// The domain name AppSync itself serves this custom domain through
    /// ([`ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::mint_appsync_domain_name`]).
    pub appsync_domain_name: String,
    pub hosted_zone_id: String,
    pub domain_name_arn: String,
    pub tags: BTreeMap<String, String>,
    /// The API this custom domain is mapped to, or `None` while it is mapped to none —
    /// before the first `AssociateApi` and after a `DisassociateApi`.
    pub api_association: Option<ApiAssociationRecord>,
}

impl DomainNameRecord {
    /// This record as the modelled `DomainNameConfig` shape — the single projection
    /// the create and the read both answer with.
    pub fn to_domain_name_config(&self) -> DomainNameConfig {
        DomainNameConfig {
            domain_name: Some(self.domain_name.clone()),
            description: self.description.clone(),
            certificate_arn: Some(self.certificate_arn.clone()),
            appsync_domain_name: Some(self.appsync_domain_name.clone()),
            hosted_zone_id: Some(self.hosted_zone_id.clone()),
            tags: Some(map_out(&self.tags)),
            domain_name_arn: Some(self.domain_name_arn.clone()),
        }
    }
}

/// One stored mapping of an API onto the custom domain that holds it.
#[derive(Debug, Clone)]
pub struct ApiAssociationRecord {
    /// The `apiId` of the associated API, of either resource family — a
    /// [`GraphqlApiRecord`] or an [`EventApiRecord`].
    pub api_id: String,
    /// The `AssociationStatus` the mapping is in.
    pub status: String,
    /// The instant the mapping the clock is deploying becomes successful, or `None`
    /// once nothing is owed. Stored rather than slept on: see [`API_ASSOCIATION_SPAN`].
    pub successful_at: Option<SystemTime>,
}

impl ApiAssociationRecord {
    /// A mapping onto `api_id` as the call that asked for it leaves it: the deployment
    /// of the association is owed by the clock from `now`, so the mapping starts out in
    /// the non-terminal [`ASSOCIATION_PROCESSING`] status.
    pub fn new(api_id: String, now: SystemTime) -> Self {
        Self {
            api_id,
            status: ASSOCIATION_PROCESSING.to_string(),
            successful_at: Some(now + API_ASSOCIATION_SPAN),
        }
    }

    /// Materialise the transition to [`ASSOCIATION_SUCCESS`] the CLOCK already made
    /// due at `now`.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that asked for
    /// the association, so the same transition is applied whether or not anybody read
    /// the domain in between.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.successful_at else {
            return;
        };
        if now < due {
            return;
        }
        self.successful_at = None;
        self.status = ASSOCIATION_SUCCESS.to_string();
    }

    /// Whether the service is still deploying the mapping, i.e. whether it is in the
    /// status during which `AssociationStatus` documents that "you cannot modify
    /// association requests".
    pub fn is_processing(&self) -> bool {
        self.status == ASSOCIATION_PROCESSING
    }

    /// This record as the modelled `ApiAssociation` shape — the single projection the
    /// associate call and the read both answer with. `domain_name` is the domain the
    /// record is stored under, so the two members can never disagree.
    pub fn to_api_association(&self, domain_name: &str) -> ApiAssociation {
        ApiAssociation {
            domain_name: Some(domain_name.to_string()),
            api_id: Some(self.api_id.clone()),
            association_status: Some(self.status.clone()),
            deployment_detail: Some(association_deployment_detail(&self.status).to_string()),
        }
    }
}

/// The `AssociationStatus` a fresh API association carries: the service is deploying
/// the mapping, which has not been served yet. Source: the `AssociationStatus` enum,
/// "PROCESSING: The API association is being created".
pub const ASSOCIATION_PROCESSING: &str = "PROCESSING";

/// The `AssociationStatus` an API association reaches once the mapping has been
/// deployed. Source: the `AssociationStatus` enum, "SUCCESS: The API association was
/// successful".
pub const ASSOCIATION_SUCCESS: &str = "SUCCESS";

/// How long after an `AssociateApi` the mapping it asked for is deployed.
///
/// A chosen span, inside the emulator's 250-750 ms band for a transition the clock owes
/// (as [`MERGE_SPAN`] and [`CACHE_SPAN`] are): AppSync reports a fresh association in
/// the non-terminal [`ASSOCIATION_PROCESSING`] status, so the span is never zero and a
/// caller reading straight after the call still observes it. Nothing sleeps for it —
/// the due instant is stored on the record and [`ApiAssociationRecord::settle`] applies
/// it.
pub const API_ASSOCIATION_SPAN: Duration = Duration::from_millis(500);

/// The `deploymentDetail` ("Details about the last deployment status") that goes with
/// each `AssociationStatus`, in the enum's own words: "The API association is being
/// created" while it is [`ASSOCIATION_PROCESSING`], "The API association was
/// successful" once it is [`ASSOCIATION_SUCCESS`].
pub fn association_deployment_detail(status: &str) -> &'static str {
    if status == ASSOCIATION_SUCCESS {
        "The API association was successful."
    } else {
        "The API association is being created."
    }
}

/// The text `AssociateApi`, `DisassociateApi` and `DeleteDomainName` carry for a domain
/// this account does not hold.
///
/// AppSync states the same condition on two wordings, with and without the sentence's full
/// stop, and which operation states which is a fact about the service rather than a rule to
/// derive: these three state it punctuated, and the ones on
/// [`DOMAIN_NAME_NOT_FOUND_READ_MESSAGE`] state it bare. The two constants exist so neither
/// wording is rebuilt from the other.
pub const DOMAIN_NAME_NOT_FOUND_MESSAGE: &str = "Domain name not found.";

/// The text `GetDomainName`, `UpdateDomainName` and `GetApiAssociation` carry for a domain
/// this account does not hold — the bare wording of
/// [`DOMAIN_NAME_NOT_FOUND_MESSAGE`]'s condition.
pub const DOMAIN_NAME_NOT_FOUND_READ_MESSAGE: &str = "Domain name not found";

/// The text `BadRequestException` (HTTP 400) carries for a create under a domain name
/// this account already holds: a custom domain is created once and then addressed by
/// that name, so a second create conflicts with the domain holding it rather than
/// replacing it.
///
/// AppSync's own sentence for this refusal is not among the texts this service has to
/// go on, so this follows the wording the service uses for an existing resource of
/// another kind ([`CHANNEL_NAMESPACE_EXISTS_MESSAGE`]) and is provisional until the
/// real one is known.
pub const DOMAIN_NAME_EXISTS_MESSAGE: &str = "Domain name already exists.";

/// The text `NotFoundException` (404) carries for a domain that exists and carries no
/// API association: the association, not the domain, is the resource the call
/// addresses, so an absent one is reported as the absent association it is.
///
/// Worded after the service's other absent-resource sentences ([`API_NOT_FOUND_MESSAGE`],
/// [`API_KEY_NOT_FOUND_PREFIX`]); AppSync's own sentence for this one is not among the
/// texts this service has to go on, so it is provisional until the real one is known.
pub const API_ASSOCIATION_NOT_FOUND_MESSAGE: &str = "API association not found.";

/// The text `BadRequestException` (HTTP 400) carries for an `AssociateApi` on a mapping
/// the service is still deploying: `AssociationStatus` documents that during
/// [`ASSOCIATION_PROCESSING`] "you cannot modify association requests", and that is the
/// ground the request is malformed on.
pub const ASSOCIATION_PROCESSING_MESSAGE: &str =
    "You cannot modify association requests during processing.";

/// The text `ConcurrentModificationException` (HTTP 409) carries, in the model's own
/// words for the shape: "Another modification is in progress at this time and it must
/// complete before you can make your change."
pub const CONCURRENT_MODIFICATION_MESSAGE: &str =
    "Another modification is in progress at this time and it must complete before you can \
     make your change.";

/// The `GraphQLApiVisibility` of an API that is reachable only from inside a VPC, and
/// therefore cannot be mapped onto a custom domain: `AssociateApiRequest$apiId` states
/// "Private APIs can not be associated with custom domains". Source: the
/// `GraphQLApiVisibility` enum.
pub const PRIVATE_VISIBILITY: &str = "PRIVATE";

/// The text `BadRequestException` (HTTP 400) carries for an `AssociateApi` naming an
/// API whose visibility is [`PRIVATE_VISIBILITY`], in the member's own words.
pub const PRIVATE_API_NOT_ASSOCIABLE_MESSAGE: &str =
    "Private APIs can not be associated with custom domains.";

/// The Route 53 hosted zone every AppSync custom domain resolves through.
///
/// An AppSync custom domain is served by a CloudFront distribution
/// ([`appsync_domain_name`]), and AWS publishes ONE hosted zone id for every
/// CloudFront distribution, so `DomainNameConfig$hostedZoneId` is the same value for
/// every custom domain rather than a per-domain one.
pub const CLOUDFRONT_HOSTED_ZONE_ID: &str = "Z2FDTNDATAQYW2";

/// The domain name AppSync serves a custom domain through, built from the distribution
/// label `label`: a CloudFront distribution domain, which is what
/// `DomainNameConfig$appsyncDomainName` ("The domain name that AppSync provides")
/// reports.
pub fn appsync_domain_name(label: &str) -> String {
    format!("{label}.cloudfront.net")
}

/// The number of characters in the host label of a CloudFront distribution domain,
/// i.e. how much of a minted opaque id
/// [`mint_appsync_domain_name`] spends on one.
///
/// [`mint_appsync_domain_name`]: ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::mint_appsync_domain_name
pub const CLOUDFRONT_DOMAIN_LABEL_LEN: usize = 14;

/// The ARN resource part of a custom domain name, the form every AppSync domain-name
/// resource is addressed under.
pub fn domain_name_resource(domain_name: &str) -> String {
    format!("domainnames/{domain_name}")
}

/// The one Region a certificate an AppSync custom domain is served with is read from.
///
/// A custom domain is served by a CloudFront distribution (which is what
/// [`CLOUDFRONT_HOSTED_ZONE_ID`] and [`appsync_domain_name`] report), and a
/// distribution reads the certificate it presents from this Region alone, so a
/// certificate held in another one cannot be attached to the domain.
pub const CERTIFICATE_REGION: &str = "us-east-1";

/// The Region field of `arn` — the fourth colon-separated field — or `None` when the
/// value is not an ARN at all.
///
/// A GLOBAL service leaves that field EMPTY (an IAM server certificate is
/// `arn:aws:iam::<account>:server-certificate/<name>`), so an empty field is reported
/// as the empty string rather than as a Region.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.strip_prefix(ARN_PREFIX)?.split(':').nth(2)
}

/// Why the certificate `certificate_arn` names cannot serve a custom domain, as the
/// text a `BadRequestException` (HTTP 400) states it with, or `None` when it can.
///
/// The rule the create follows: the certificate has to be reachable from the Region a
/// custom domain's distribution reads certificates from ([`CERTIFICATE_REGION`]), so a
/// certificate ARN that names any other Region is refused. A certificate of a global
/// service names no Region ([`arn_region`]) and is reachable from every one, so it is
/// not refused on this ground.
pub fn certificate_region_refusal(certificate_arn: &str) -> Option<String> {
    let region = arn_region(certificate_arn)?;
    if region.is_empty() || region == CERTIFICATE_REGION {
        return None;
    }
    Some(format!("Certificate must be in {CERTIFICATE_REGION}."))
}

/// Why a call that modifies a custom domain's API association was refused.
///
/// Each operation answers it with the error ITS model declares and with that error's
/// own text, which is why this carries the ground alone and no message.
#[derive(Debug)]
pub enum DomainAssociationRefusal {
    /// This account holds no custom domain of that name, so there is no association to
    /// map or unmap.
    DomainNotFound,
    /// The service is still deploying the domain's association
    /// ([`ASSOCIATION_PROCESSING`]), during which the association cannot be modified.
    Processing,
}

/// The custom domain `domain_name` names, for a call that CHANGES the API association
/// it carries (`AssociateApi`, `DisassociateApi`), or the refusal the request meets.
///
/// The rule both modifications follow, in one place:
///
/// * an association is a property OF a custom domain, so a domain this account does
///   not hold carries none and there is nothing to change — whatever the request asks
///   for, and whoever else is asking at the same time, which is what makes concurrent
///   callers on an absent domain all get the same answer;
/// * while the service is deploying an association, "you cannot modify association
///   requests" (`AssociationStatus`), so the change is refused until the deployment the
///   clock owes has been applied.
pub fn domain_for_association_change<'a>(
    state: &'a mut ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState,
    domain_name: &str,
) -> Result<&'a mut DomainNameRecord, DomainAssociationRefusal> {
    let Some(domain) = state.domain_names.get_mut(domain_name) else {
        return Err(DomainAssociationRefusal::DomainNotFound);
    };
    if domain
        .api_association
        .as_ref()
        .is_some_and(ApiAssociationRecord::is_processing)
    {
        return Err(DomainAssociationRefusal::Processing);
    }
    Ok(domain)
}

/// One stored association between a Merged API and one of its source APIs.
///
/// Both sides are kept by `apiId` AND by ARN, the two forms the modelled
/// `SourceApiAssociation` reports and the two forms an `…Identifier` member accepts,
/// so a reader never has to rebuild either from the request.
#[derive(Debug, Clone)]
pub struct SourceApiAssociationRecord {
    /// Creation order, so a listing over associations has a stable page order the
    /// opaque `associationId` cannot carry.
    pub seq: u64,
    pub association_id: String,
    pub association_arn: String,
    pub source_api_id: String,
    pub source_api_arn: String,
    pub merged_api_id: String,
    pub merged_api_arn: String,
    pub description: Option<String>,
    /// The `MergeType` the association merges under ([`DEFAULT_MERGE_TYPE`] when the
    /// request named none).
    pub merge_type: String,
    /// The `SourceApiAssociationStatus` the association is in.
    pub status: String,
    /// "The detailed message related to the current state of the source API
    /// association" — the text that goes with [`Self::status`].
    pub status_detail: String,
    /// When the association last merged the source API's schema into the Merged API,
    /// or `None` while it never has.
    pub last_successful_merge_date: Option<SystemTime>,
    /// The instant the merge this association scheduled comes due, or `None` once
    /// there is no merge left owed. Stored rather than slept on: see [`MERGE_SPAN`].
    pub merge_due_at: Option<SystemTime>,
    /// The instant the deletion this association scheduled comes due, or `None` while
    /// no deletion was asked for. Stored rather than slept on: see [`DELETION_SPAN`].
    pub deletion_due_at: Option<SystemTime>,
}

impl SourceApiAssociationRecord {
    /// Whether a merge the clock owes this association has not been materialised yet.
    pub fn merge_pending(&self) -> bool {
        self.merge_due_at.is_some()
    }

    /// Whether the deletion of this association has been scheduled and the clock has
    /// not made it due yet.
    pub fn deletion_pending(&self) -> bool {
        self.deletion_due_at.is_some()
    }

    /// Whether a modification of this association is still in progress, i.e. whether
    /// the clock still owes it a merge or a deletion.
    pub fn modification_pending(&self) -> bool {
        self.merge_pending() || self.deletion_pending()
    }

    /// Schedule this association's deletion at `now`.
    ///
    /// The deletion is SCHEDULED rather than performed, so the association is left in
    /// the non-terminal [`DELETION_SCHEDULED`] status with the due instant the clock
    /// settles, and the merge it was still owed is dropped: an association being
    /// deleted has no schema left to merge.
    pub fn schedule_deletion(&mut self, now: SystemTime) {
        self.merge_due_at = None;
        self.deletion_due_at = Some(now + DELETION_SPAN);
        self.status = DELETION_SCHEDULED.to_string();
        self.status_detail = DELETION_SCHEDULED_DETAIL.to_string();
    }

    /// Whether the deletion this association scheduled has come due at `now`, i.e.
    /// whether the association is gone.
    pub fn deletion_due(&self, now: SystemTime) -> bool {
        self.deletion_due_at.is_some_and(|due| now >= due)
    }

    /// Materialise the merge the CLOCK already made due at `now`.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that
    /// scheduled the merge, so the same transition is applied whether or not anybody
    /// read the association in between, and the merge date is the due instant itself
    /// rather than the moment of the read — so every later read reports the same one.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.merge_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.merge_due_at = None;
        self.status = MERGE_SUCCESS.to_string();
        self.status_detail = MERGE_SUCCESS_DETAIL.to_string();
        self.last_successful_merge_date = Some(due);
    }

    /// Initiate a merge of the source API's schema into the Merged API, and report the
    /// `SourceApiAssociationStatus` the association is left in, or the refusal the call
    /// meets.
    ///
    /// Scheduling is all this does: the merge itself comes due one [`MERGE_SPAN`] later
    /// and [`Self::settle`] materialises it, exactly as the create that first scheduled
    /// one does. A merge the clock already owes IS the merge such a call initiates, so
    /// its due instant is kept rather than pushed out — repeated calls can neither
    /// starve the transition nor leave two merges owed for one association.
    ///
    /// An association whose DELETION the clock still owes is being destroyed and has no
    /// schema left to merge, and that deletion is a modification already in progress, so
    /// the call is refused rather than overwriting the status the deletion left behind.
    pub fn start_merge(&mut self, now: SystemTime) -> Result<String, AssociationRefusal> {
        if self.deletion_pending() {
            return Err(AssociationRefusal::ConcurrentModification(
                associations_being_updated_message(&self.merged_api_id),
            ));
        }
        if self.merge_due_at.is_none() {
            self.merge_due_at = Some(now + MERGE_SPAN);
        }
        self.status = MERGE_SCHEDULED.to_string();
        self.status_detail = MERGE_SCHEDULED_DETAIL.to_string();
        Ok(self.status.clone())
    }

    /// Apply the configuration choices an update asks this association for, or report
    /// the refusal the call meets.
    ///
    /// The two settable members are each "the new value" of a configuration choice, so
    /// the update REPLACES what was stored rather than merging into it: a member the
    /// request omits is gone from the association, and an omitted `mergeType` leaves
    /// the association on the modelled default ([`DEFAULT_MERGE_TYPE`]) — the same
    /// projection the create takes, so an update cannot leave an association in a state
    /// no create could produce.
    ///
    /// Only the configuration changes: the merge the association scheduled is neither
    /// performed nor re-scheduled here, so the status, the status detail and the merge
    /// date stay what the clock left behind.
    ///
    /// An association whose DELETION the clock still owes is being destroyed, and that
    /// deletion is a modification already in progress, so the call is refused rather
    /// than writing over the status the deletion left behind.
    pub fn apply_update(
        &mut self,
        description: Option<String>,
        merge_type: Option<String>,
    ) -> Result<(), AssociationRefusal> {
        if self.deletion_pending() {
            return Err(AssociationRefusal::ConcurrentModification(
                associations_being_updated_message(&self.merged_api_id),
            ));
        }
        self.description = description;
        self.merge_type = merge_type.unwrap_or_else(|| DEFAULT_MERGE_TYPE.to_string());
        Ok(())
    }

    /// This record as the modelled `SourceApiAssociation` shape.
    ///
    /// The single projection every operation that reports an association answers
    /// with, so the create and the reads cannot disagree about one association.
    pub fn to_source_api_association(&self) -> SourceApiAssociation {
        SourceApiAssociation {
            association_id: Some(self.association_id.clone()),
            association_arn: Some(self.association_arn.clone()),
            source_api_id: Some(self.source_api_id.clone()),
            source_api_arn: Some(self.source_api_arn.clone()),
            merged_api_id: Some(self.merged_api_id.clone()),
            merged_api_arn: Some(self.merged_api_arn.clone()),
            description: self.description.clone(),
            source_api_association_config: Some(SourceApiAssociationConfig {
                merge_type: Some(self.merge_type.clone()),
            }),
            source_api_association_status: Some(self.status.clone()),
            source_api_association_status_detail: Some(self.status_detail.clone()),
            last_successful_merge_date: self
                .last_successful_merge_date
                .map(::aws_smithy_types::DateTime::from),
        }
    }

    /// This record as the modelled `SourceApiAssociationSummary` shape: "the ARNs and
    /// IDs of associations, Merged APIs, and source APIs", plus the description.
    ///
    /// Projected from the SAME stored record [`Self::to_source_api_association`]
    /// answers from, so a listing and a read cannot disagree about the members both
    /// shapes carry; the members the summary leaves out are the association's
    /// merge configuration and status, which only the full shape reports.
    pub fn to_source_api_association_summary(&self) -> SourceApiAssociationSummary {
        SourceApiAssociationSummary {
            association_id: Some(self.association_id.clone()),
            association_arn: Some(self.association_arn.clone()),
            source_api_id: Some(self.source_api_id.clone()),
            source_api_arn: Some(self.source_api_arn.clone()),
            merged_api_id: Some(self.merged_api_id.clone()),
            merged_api_arn: Some(self.merged_api_arn.clone()),
            description: self.description.clone(),
        }
    }
}

/// What a request to associate a source API with a Merged API asks for, in the form
/// both association creates share.
///
/// AppSync creates one and the same link from either side — `AssociateMergedGraphqlApi`
/// addresses the source API and names the Merged API in the body,
/// `AssociateSourceGraphqlApi` the other way round — so the two differ only in which
/// member the path carries and reach [`create_source_api_association`] with the same
/// request.
#[derive(Debug, Clone)]
pub struct SourceApiAssociationRequest {
    /// The Merged API side, in either accepted form ([`is_api_identifier`]).
    pub merged_api_identifier: String,
    /// The source API side, in either accepted form ([`is_api_identifier`]).
    pub source_api_identifier: String,
    pub description: Option<String>,
    /// The `mergeType` the request's `sourceApiAssociationConfig` named, or `None` when
    /// it named none ([`DEFAULT_MERGE_TYPE`] is then stored).
    pub merge_type: Option<String>,
    /// The region the caller addressed, which the minted association ARN carries.
    pub region: String,
}

/// Why a source-API association could not be created, as the HTTP answer every
/// operation that creates one gives it.
///
/// Stated independently of the operation so both association creates refuse the same
/// request the same way; each handler lifts a variant into its own modelled error.
#[derive(Debug, Clone)]
pub enum AssociationRefusal {
    /// The request names something that cannot be associated: `BadRequestException`
    /// (HTTP 400), carrying this text.
    BadRequest(String),
    /// An identifier that names no API this account holds: `NotFoundException`
    /// (HTTP 404), carrying this text.
    NotFound(String),
    /// A modification of the pair's association is still in progress:
    /// `ConcurrentModificationException` (HTTP 409), carrying this text.
    ConcurrentModification(String),
}

/// Why an `(mergedApiIdentifier, associationId)` pair resolves to no one association,
/// as the HTTP answer every operation that addresses an association that way gives it.
///
/// Stated independently of the operation, so the read and the merge refuse the same
/// pair the same way; each handler lifts a variant into its own modelled error. Only
/// the refusals the LOOKUP itself can meet are here — what an operation then does
/// with the association it found is its own business.
///
/// Which SIDE of the pair resolved to nothing is kept apart, because the operations do
/// not all name the refused resource the same way: an absent Merged API carries the
/// not-found text every operation answers for an API, while an `associationId` that
/// names no association of it carries the association's own not-found text.
#[derive(Debug, Clone)]
pub enum AssociationLookupRefusal {
    /// The request names an identifier that cannot name an API at all:
    /// `BadRequestException` (HTTP 400), carrying this text.
    BadRequest(String),
    /// The Merged API the request addresses is not one this account holds:
    /// `NotFoundException` (HTTP 404), carrying this text.
    NotFound(String),
    /// The Merged API exists, but holds no association with the `associationId` the
    /// request names — which is carried here, so an operation whose refusal names the
    /// rejected id can build it.
    AssociationNotFound(String),
}

/// A lookup refusal is one of the refusals an association operation can give, so an
/// operation that both resolves a pair and then acts on it answers both kinds the same
/// way.
impl From<AssociationLookupRefusal> for AssociationRefusal {
    fn from(refusal: AssociationLookupRefusal) -> Self {
        match refusal {
            AssociationLookupRefusal::BadRequest(message) => Self::BadRequest(message),
            AssociationLookupRefusal::NotFound(message) => Self::NotFound(message),
            AssociationLookupRefusal::AssociationNotFound(_) => {
                Self::NotFound(ASSOCIATION_NOT_FOUND_MESSAGE.to_string())
            }
        }
    }
}

/// Create the association `request` asks for and return it as the modelled shape, or
/// the refusal it meets.
///
/// The whole rule both association creates follow, in one place:
///
/// * an identifier that is neither an API id nor an ARN names no API at all, so the
///   request is malformed rather than pointed at a missing resource;
/// * both sides are resolved from the store in either identifier form, so the stored
///   association records each API's own id and ARN rather than whatever the request
///   named it by, and an identifier that resolves to nothing is a not-found;
/// * an association links a standard GraphQL API (the source) to a Merged API, so an
///   API on the wrong side of it is a bad request;
/// * a source API is linked to a Merged API at most once and to ONE Merged API, so a
///   pair that is already associated leaves the call nothing to create (400, the
///   service's own fixed [`ASSOCIATION_EXISTS_MESSAGE`]) and a source API already
///   associated with another Merged API cannot take a second one (400,
///   [`SOURCE_API_ALREADY_MERGED_MESSAGE`]) — both decided from the association state
///   alone, never from what the clock still owes;
/// * the association's `associationId` is drawn from the state's own counter and its
///   ARN sits under the Merged API, and creating it SCHEDULES the merge — so it is
///   stored in the non-terminal [`MERGE_SCHEDULED`] status with the due instant the
///   clock settles ([`MERGE_SPAN`]).
///
/// The answer is projected from the STORED record, so it is the body
/// `GetSourceApiAssociation` reports for the same association later.
pub fn create_source_api_association(
    state: &mut ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState,
    request: SourceApiAssociationRequest,
    now: SystemTime,
) -> Result<SourceApiAssociation, AssociationRefusal> {
    // Checked on BOTH sides, since either can carry either form.
    for identifier in [
        &request.merged_api_identifier,
        &request.source_api_identifier,
    ] {
        if !is_api_identifier(identifier) {
            return Err(AssociationRefusal::BadRequest(invalid_api_id_message(
                identifier,
            )));
        }
    }

    // Materialise every merge the clock already owes first, so the decisions below are
    // taken against the association state of THIS instant rather than the one the last
    // caller left behind.
    state.settle_associations(now);

    let Some(merged) = api_by_identifier(&state.graphql_apis, &request.merged_api_identifier)
    else {
        return Err(AssociationRefusal::NotFound(
            API_NOT_FOUND_MESSAGE.to_string(),
        ));
    };
    let merged_api_id = merged.api_id.clone();
    let merged_api_arn = merged.arn.clone();
    let merged_api_type = merged.api_type.clone();

    let Some(source) = api_by_identifier(&state.graphql_apis, &request.source_api_identifier)
    else {
        return Err(AssociationRefusal::NotFound(
            API_NOT_FOUND_MESSAGE.to_string(),
        ));
    };
    let source_api_id = source.api_id.clone();
    let source_api_arn = source.arn.clone();
    let source_api_type = source.api_type.clone();

    // "A source API is a GraphQL API that is linked to a merged API"
    // (`SourceApiAssociation`). An API on the wrong side of that sentence exists and is
    // named well, so the request is a bad one rather than a not-found.
    if merged_api_type != MERGED_API_TYPE {
        return Err(AssociationRefusal::BadRequest(invalid_api_type_message(
            MERGED_API_TYPE,
            &merged_api_id,
        )));
    }
    if source_api_type != GRAPHQL_API_TYPE {
        return Err(AssociationRefusal::BadRequest(invalid_api_type_message(
            GRAPHQL_API_TYPE,
            &source_api_id,
        )));
    }

    // The pair is already linked, so there is nothing left to create. A duplicate is a
    // permanent property of the REQUEST, so it is decided before anything the clock
    // still owes on the existing association is consulted: a caller that repeats a
    // duplicate request must not be told to retry.
    if state
        .association_for_pair(&merged_api_id, &source_api_id)
        .is_some()
    {
        return Err(AssociationRefusal::BadRequest(
            ASSOCIATION_EXISTS_MESSAGE.to_string(),
        ));
    }

    // A source API belongs to ONE Merged API: it is already a side of an association
    // with another Merged API, so this one cannot be created either.
    if state.association_of_source_api(&source_api_id).is_some() {
        return Err(AssociationRefusal::BadRequest(
            SOURCE_API_ALREADY_MERGED_MESSAGE.to_string(),
        ));
    }

    // The id is drawn from the state's own counter, never from the request, so
    // associating two different pairs can never collide on it.
    let mut association_id = state.mint_id();
    while state.source_api_associations.contains_key(&association_id) {
        association_id = state.mint_id();
    }
    let seq = state.next_seq();

    let record = SourceApiAssociationRecord {
        seq,
        association_arn: ::engine::emulator_arn(
            ARN_SERVICE,
            &request.region,
            &format!("apis/{merged_api_id}/sourceApiAssociations/{association_id}"),
        ),
        association_id: association_id.clone(),
        source_api_id,
        source_api_arn,
        merged_api_id,
        merged_api_arn,
        description: request.description,
        merge_type: request
            .merge_type
            .unwrap_or_else(|| DEFAULT_MERGE_TYPE.to_string()),
        // Creating the association SCHEDULES the merge; the clock owes its outcome, so
        // nothing here performs it or waits for it.
        status: MERGE_SCHEDULED.to_string(),
        status_detail: MERGE_SCHEDULED_DETAIL.to_string(),
        last_successful_merge_date: None,
        merge_due_at: Some(now + MERGE_SPAN),
        deletion_due_at: None,
    };
    let association = record.to_source_api_association();
    state
        .source_api_associations
        .insert(association_id, record);

    Ok(association)
}

/// Which side of an association the request's path addressed.
///
/// One association links two APIs, and AppSync lets it be addressed from either side —
/// `DisassociateSourceGraphqlApi` by the Merged API, `DisassociateMergedGraphqlApi` by
/// the source API — so this is the only way the two requests differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationSide {
    /// The Merged API side.
    MergedApi,
    /// The source API side.
    SourceApi,
}

impl AssociationSide {
    /// The `apiId` `association` holds on this side.
    pub fn api_id(self, association: &SourceApiAssociationRecord) -> &str {
        match self {
            Self::MergedApi => &association.merged_api_id,
            Self::SourceApi => &association.source_api_id,
        }
    }
}

/// What a request to delete a source-API association asks for, in the form both
/// disassociates share.
#[derive(Debug, Clone)]
pub struct SourceApiDisassociationRequest {
    /// Which side of the association the path addressed.
    pub side: AssociationSide,
    /// That side's API, in either accepted form ([`is_api_identifier`]).
    pub api_identifier: String,
    /// The `associationId` AppSync generated for the association.
    pub association_id: String,
}

/// Delete the association `request` names and return the `SourceApiAssociationStatus`
/// it is left in, or the refusal the request meets.
///
/// The whole rule a disassociate follows, in one place:
///
/// * an identifier that is neither an API id nor an ARN names no API at all, so the
///   request is malformed rather than pointed at a missing resource;
/// * an `associationId` that is not a reference the service itself could have issued
///   ([`is_association_id`]) is likewise a malformed request — stated by the request
///   alone, so it is refused before anything is resolved from the store — and names the
///   rejected id ([`invalid_association_id_message`]);
/// * the addressed API is resolved from the store in either identifier form, and one
///   that resolves to nothing is a not-found;
/// * an `associationId` names an association OF the API addressed, so an association
///   under another API on that side is not found under this one, and so is a well-formed
///   id the service minted and has since removed
///   ([`ASSOCIATION_NOT_FOUND_MESSAGE`]);
/// * deleting the association SCHEDULES the deletion, so the call leaves it in the
///   non-terminal [`DELETION_SCHEDULED`] status with the due instant the clock settles
///   ([`DELETION_SPAN`]), and reports that status; the association stops existing once
///   that instant has passed, whether or not anybody calls again.
pub fn delete_source_api_association(
    state: &mut ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState,
    request: SourceApiDisassociationRequest,
    now: SystemTime,
) -> Result<String, AssociationRefusal> {
    if !is_api_identifier(&request.api_identifier) {
        return Err(AssociationRefusal::BadRequest(invalid_api_id_message(
            &request.api_identifier,
        )));
    }

    if !is_association_id(&request.association_id) {
        return Err(AssociationRefusal::BadRequest(
            invalid_association_id_message(&request.association_id),
        ));
    }

    // Materialise everything the clock already owes first, so this call decides against
    // the association state of THIS instant: an association whose deletion came due is
    // gone before it can be deleted again.
    state.settle_associations(now);

    let Some(api) = api_by_identifier(&state.graphql_apis, &request.api_identifier) else {
        return Err(AssociationRefusal::NotFound(
            API_NOT_FOUND_MESSAGE.to_string(),
        ));
    };
    let api_id = api.api_id.clone();

    let side = request.side;
    let association = match state
        .source_api_associations
        .get_mut(&request.association_id)
    {
        Some(association) if api_id == side.api_id(association) => association,
        _ => {
            return Err(AssociationRefusal::NotFound(
                ASSOCIATION_NOT_FOUND_MESSAGE.to_string(),
            ))
        }
    };

    // Scheduling is idempotent: a second call reaching an association whose deletion is
    // already owed has nothing left to schedule, so it neither moves the due instant nor
    // reports a different status.
    if !association.deletion_pending() {
        association.schedule_deletion(now);
    }

    Ok(association.status.clone())
}

/// Per-service state for com-amazonaws-appsync-awsdeepdishcontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Every created GraphQL API, keyed by its minted `apiId`.
    pub graphql_apis: BTreeMap<String, GraphqlApiRecord>,
    /// Every created `Api` (an API with an Event API configuration), keyed by its
    /// minted `apiId`. A separate resource from [`Self::graphql_apis`]: the two are
    /// created and addressed by different operations.
    pub event_apis: BTreeMap<String, EventApiRecord>,
    /// Every source-API association, keyed by its minted `associationId`.
    pub source_api_associations: BTreeMap<String, SourceApiAssociationRecord>,
    /// Every data source introspection this service has run, keyed by its minted
    /// `introspectionId`. A record of its own resource family: an introspection belongs to
    /// no API, which is why the operations that create and read one name none.
    pub data_source_introspections: BTreeMap<String, IntrospectionRecord>,
    /// Every created custom domain name, keyed by the domain name itself — the name is
    /// what every domain-name and association operation addresses one by, so it
    /// identifies the domain and no second key is needed.
    pub domain_names: BTreeMap<String, DomainNameRecord>,
    /// The sockets each Event API's two endpoints are served on, and what is open on
    /// them ([`event_data_plane`]). Held beside the API records so a reader cannot see an
    /// API whose data plane this store does not hold.
    pub event_data_plane: event_data_plane::EventDataPlane,
    /// The socket each GraphQL API's real-time endpoint is served on, and what is open on
    /// it ([`graphql_data_plane`]). Held beside the API records so a reader cannot see an
    /// API whose real-time plane this store does not hold.
    pub graphql_data_plane: graphql_data_plane::GraphqlDataPlane,
    /// Advanced once per minted id, so no id is ever a function of the request.
    id_seed: u64,
    /// Advanced once per created resource, giving each record its creation order.
    seq_counter: u64,
}

impl ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Apply every merge the clock owes at `now`, so an association reports the same
    /// state to every caller whether or not another call touched it first.
    pub fn settle_associations(&mut self, now: SystemTime) {
        // A deletion the clock has made due leaves no association behind, so it is
        // applied before the merges: a record that is gone has no status to settle.
        self.source_api_associations
            .retain(|_, association| !association.deletion_due(now));
        // The merges the clock owes are performed with the schema creations they depend
        // on, in that order (`settle_schema_creations`).
        self.settle_schema_creations(now);
    }

    /// Apply every API-association transition the clock owes at `now`, so a custom
    /// domain's association reports the same status to every caller whether or not
    /// another call touched it first.
    pub fn settle_api_associations(&mut self, now: SystemTime) {
        for domain in self.domain_names.values_mut() {
            if let Some(association) = domain.api_association.as_mut() {
                association.settle(now);
            }
        }
    }

    /// A fresh `appsyncDomainName`: a CloudFront distribution domain
    /// ([`appsync_domain_name`]) whose label is minted from this state's own id
    /// counter, never from the request, so two domains never share one.
    pub fn mint_appsync_domain_name(&mut self) -> String {
        let minted = self.mint_id();
        appsync_domain_name(&minted[..CLOUDFRONT_DOMAIN_LABEL_LEN])
    }

    /// Apply every cache transition the clock owes at `now`, so a cache reports the
    /// same status to every caller whether or not another call touched it first.
    pub fn settle_api_caches(&mut self, now: SystemTime) {
        for api in self.graphql_apis.values_mut() {
            if let Some(cache) = api.api_cache.as_mut() {
                cache.settle(now);
            }
        }
    }

    /// The association between the Merged API `merged_api_id` and the source API
    /// `source_api_id`, or `None` when the two are not associated.
    ///
    /// A source API is linked to a Merged API at most once, so this is the pair's
    /// whole association state.
    pub fn association_for_pair(
        &self,
        merged_api_id: &str,
        source_api_id: &str,
    ) -> Option<&SourceApiAssociationRecord> {
        self.source_api_associations.values().find(|association| {
            association.merged_api_id == merged_api_id && association.source_api_id == source_api_id
        })
    }

    /// The association the source API `source_api_id` already takes part in, or `None`
    /// when it is associated with no Merged API.
    ///
    /// A source API belongs to one Merged API, so this is the whole merged-API
    /// membership of a source API: the link that makes a second one impossible.
    pub fn association_of_source_api(
        &self,
        source_api_id: &str,
    ) -> Option<&SourceApiAssociationRecord> {
        self.source_api_associations
            .values()
            .find(|association| association.source_api_id == source_api_id)
    }

    /// Whether the API `api_id` is the SOURCE side of an association that is not being
    /// torn down, i.e. whether it is still a source API of a Merged API.
    ///
    /// An association whose deletion the clock already owes is on its way out, so it no
    /// longer holds the source API: a caller that has asked for the disassociation is
    /// free to delete the API without waiting for the deletion to come due.
    pub fn is_source_api_of_a_merged_api(&self, api_id: &str) -> bool {
        self.source_api_associations
            .values()
            .any(|association| association.source_api_id == api_id && !association.deletion_pending())
    }


    /// addresses, or the refusal the pair meets.
    ///
    /// The rule every operation that addresses one association by that pair follows, in
    /// one place:
    ///
    /// * an identifier that is neither an API id nor an ARN names no API at all, so the
    ///   request is malformed rather than pointed at a missing resource;
    /// * an `associationId` that is not a reference the service itself could have issued
    ///   ([`is_association_id`]) is likewise malformed, and names the rejected id
    ///   ([`invalid_association_id_message`]);
    /// * the Merged API is resolved in either identifier form, and one that resolves to
    ///   nothing is a not-found;
    /// * a well-formed `associationId` names an association OF the Merged API addressed,
    ///   so one that belongs to another Merged API — or to none — resolves to nothing
    ///   under this one, which each operation answers in its own way
    ///   ([`AssociationLookupRefusal`]).
    pub fn resolve_association(
        &self,
        merged_api_identifier: &str,
        association_id: &str,
    ) -> Result<&SourceApiAssociationRecord, AssociationLookupRefusal> {
        if !is_api_identifier(merged_api_identifier) {
            return Err(AssociationLookupRefusal::BadRequest(invalid_api_id_message(
                merged_api_identifier,
            )));
        }

        if !is_association_id(association_id) {
            return Err(AssociationLookupRefusal::BadRequest(
                invalid_association_id_message(association_id),
            ));
        }

        let merged = api_by_identifier(&self.graphql_apis, merged_api_identifier).ok_or_else(
            || AssociationLookupRefusal::NotFound(API_NOT_FOUND_MESSAGE.to_string()),
        )?;

        self.source_api_associations
            .get(association_id)
            .filter(|association| association.merged_api_id == merged.api_id)
            .ok_or_else(|| {
                AssociationLookupRefusal::AssociationNotFound(association_id.to_string())
            })
    }

    /// Every association the API `api_id` takes part in, on either side, in creation
    /// order.
    ///
    /// An association names two APIs and is addressed from either of them, so the API's
    /// own association state is every link it is a side of — the same set
    /// [`Self::remove_api_associations`] drops with the API. Creation order is the
    /// stable order a listing pages over, which the opaque `associationId` cannot
    /// carry.
    pub fn associations_of_api(&self, api_id: &str) -> Vec<&SourceApiAssociationRecord> {
        let mut associations: Vec<&SourceApiAssociationRecord> = self
            .source_api_associations
            .values()
            .filter(|association| {
                association.source_api_id == api_id || association.merged_api_id == api_id
            })
            .collect();
        associations.sort_by_key(|association| association.seq);
        associations
    }

    /// Drop every association the API `api_id` takes part in, on either side.
    ///
    /// An association names two APIs, so it cannot outlive either of them: deleting
    /// an API deletes the links it was a side of.
    pub fn remove_api_associations(&mut self, api_id: &str) {
        self.source_api_associations.retain(|_, association| {
            association.source_api_id != api_id && association.merged_api_id != api_id
        });
    }

    /// The API keys the API `api_id` holds, or `None` when no stored API has that id.
    ///
    /// An API key belongs to the API its `apiId` path label names, and that label is
    /// ONE identifier space over both API resource families: a `GraphqlApi` and an
    /// `Api` are created by operations of their own, but either is given its keys
    /// through the same `/v1/apis/{apiId}/apikeys` path. So an `apiId` is resolved
    /// against both stores here — the single lookup every API-key operation uses —
    /// and only an id that names no API at all is a not-found.
    pub fn api_keys_of(&self, api_id: &str) -> Option<&BTreeMap<String, ApiKeyRecord>> {
        self.graphql_apis
            .get(api_id)
            .map(|api| &api.api_keys)
            .or_else(|| self.event_apis.get(api_id).map(|api| &api.api_keys))
    }

    /// The API keys the API `api_id` holds, for a call that writes one.
    ///
    /// Resolves `api_id` exactly as [`Self::api_keys_of`] does, so a write and a read
    /// of the same API's keys can never disagree about which APIs exist.
    pub fn api_keys_of_mut(&mut self, api_id: &str) -> Option<&mut BTreeMap<String, ApiKeyRecord>> {
        match self.graphql_apis.get_mut(api_id) {
            Some(api) => Some(&mut api.api_keys),
            None => self.event_apis.get_mut(api_id).map(|api| &mut api.api_keys),
        }
    }

    /// The tags the resource `resource_arn` names holds, or `None` when this account
    /// holds no such resource.
    ///
    /// `ResourceArn` admits one ARN shape (`^arn:aws:appsync:…:apis/…$`), and an API of
    /// EITHER resource family is published under exactly that shape, so the ARN is
    /// resolved against both stores here — the single lookup every tag operation uses,
    /// so a tag read and a tag write can never disagree about which resources are
    /// taggable. The whole ARN is compared, so a resource is addressed only in the
    /// account and Region it was created in.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        self.graphql_apis
            .values()
            .find(|api| api.arn == resource_arn)
            .map(|api| &api.tags)
            .or_else(|| {
                self.event_apis
                    .values()
                    .find(|api| api.api_arn == resource_arn)
                    .map(|api| &api.tags)
            })
    }

    /// The tags the resource `resource_arn` names holds, for a call that writes them.
    ///
    /// Resolves `resource_arn` exactly as [`Self::tags_of`] does.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(api) = self
            .graphql_apis
            .values_mut()
            .find(|api| api.arn == resource_arn)
        {
            return Some(&mut api.tags);
        }
        self.event_apis
            .values_mut()
            .find(|api| api.api_arn == resource_arn)
            .map(|api| &mut api.tags)
    }
    /// Mint a fresh opaque id: 26 lowercase base-32 characters, the shape an AppSync
    /// `apiId` takes.
    ///
    /// Drawn from a counter the state advances (seeded from the clock on first use),
    /// never from the request, so creating the same API twice yields two ids.
    pub fn mint_id(&mut self) -> String {
        let seed = self.next_id_draw();
        let low = splitmix64(seed);
        let mut id = base32_13(seed);
        id.push_str(&base32_13(low));
        id
    }

    /// The next draw of this state's own id counter: the bits a minted id is formed from.
    ///
    /// Seeded from the clock on first use and advanced through a bijection, so consecutive
    /// draws neither repeat nor spell consecutive numbers. Shared by every id this service
    /// mints, whatever FORM that id takes ([`Self::mint_id`], and the UUID an
    /// introspection is referenced by), so no two of them can be drawn from the same
    /// value.
    fn next_id_draw(&mut self) -> u64 {
        if self.id_seed == 0 {
            self.id_seed = now_nanos();
        }
        self.id_seed = splitmix64(self.id_seed);
        self.id_seed
    }

    /// Mint a fresh opaque id that can also be addressed as a `ResourceName`: a
    /// [`mint_id`] id whose first character is a letter.
    ///
    /// Every operation that addresses a function (`GetFunction`, `UpdateFunction`,
    /// `DeleteFunction`) takes the `functionId` as a `ResourceName`, whose pattern
    /// `^[_A-Za-z][_0-9A-Za-z]*$` rules out an id starting with a digit, so an id of
    /// that shape would name a function no caller could read back.
    ///
    /// [`mint_id`]: Self::mint_id
    pub fn mint_resource_id(&mut self) -> String {
        // Each turn draws a FRESH id (the counter advances through a bijection), and
        // 26 of the 32 first characters qualify, so the loop ends after a couple of
        // turns rather than retrying one rejected value for ever.
        loop {
            let id = self.mint_id();
            if id.starts_with(|first: char| first.is_ascii_alphabetic()) {
                return id;
            }
        }
    }

    /// The next creation-order number.
    pub fn next_seq(&mut self) -> u64 {
        self.seq_counter = self.seq_counter.wrapping_add(1);
        self.seq_counter
    }
}

/// The number of characters an AppSync API id has.
///
/// The ids this service mints are 26 base-32 characters ([`mint_id`]), the shape
/// AppSync's own `apiId` takes. The model types every `apiId` member as a plain
/// `String`, so nothing refuses a longer one at the boundary; [`is_api_id`] is where
/// this service refuses it.
///
/// [`mint_id`]: ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::mint_id
pub const API_ID_LEN: usize = 26;

/// Whether `candidate` is syntactically an AppSync API id, i.e. whether it could name
/// an API at all.
///
/// The test is on LENGTH only: an id longer than [`API_ID_LEN`] holds more characters
/// than an AppSync API id has, so it names no API and the call is a bad request, while
/// a shorter one is simply an id this account does not hold and the call is a
/// not-found. The alphabet is deliberately NOT part of the test: AppSync answers
/// not-found for a short id drawn from outside the id alphabet, so a character outside
/// it does not make the request malformed.
pub fn is_api_id(candidate: &str) -> bool {
    !candidate.is_empty() && candidate.chars().count() <= API_ID_LEN
}

/// AppSync's refusal text for an `apiId` that is not an API id ([`is_api_id`]): the
/// rejected id, so the caller can see which value was refused.
pub fn invalid_api_id_message(api_id: &str) -> String {
    format!("Invalid API ID {api_id}")
}

/// `ownerContact`'s documented maximum length: "This field accepts any string input
/// with a length of 0 - 256 characters."
pub const OWNER_CONTACT_MAX_LEN: usize = 256;

/// The name a refusal names the OIDC configuration by.
pub const OPEN_ID_CONNECT_CONFIG: &str = "OpenID Connect Config";

/// The name a refusal names the Cognito user pool configuration by.
pub const USER_POOL_CONFIG: &str = "User Pool Config";

/// The name a refusal names the Lambda authorizer configuration by.
pub const LAMBDA_AUTHORIZER_CONFIG: &str = "Lambda Authorizer Config";

/// Why the authentication configuration a request carries for a GraphQL API is not
/// well formed, as the text a `BadRequestException` (HTTP 400) states it with, or
/// `None` when it is well formed.
///
/// The configuration members of a GraphQL API are the same whether the request
/// CREATES the API or UPDATES it, and so are the grounds on which either request is
/// refused, so the rules are stated here once:
///
/// * an authentication type that is set up through a member of its own
///   ([`missing_auth_config`]) is not configured without that member — checked for
///   the API's own `authenticationType` and, independently, for every
///   `additionalAuthenticationProviders` entry, which the refusal names by its
///   1-based position, the only way a caller can tell them apart;
/// * an authentication type identifies ONE way of authenticating the API, so the API
///   cannot name it twice;
/// * an OIDC provider is identified by its issuer, which "must exactly match the
///   value of `iss` in the ID token" — an absolute URL;
/// * `ownerContact` "accepts any string input with a length of 0 - 256 characters"
///   (its own member documentation), a bound the plain `String` target leaves to the
///   service.
///
/// The per-type presence of the API's OWN user-pool and Lambda configurations is
/// taken as a flag rather than as the structure, because the API-level and the
/// per-provider user-pool configurations are modelled with different shapes
/// (`UserPoolConfig` vs `CognitoUserPoolConfig`) while the rule over them is one.
pub fn graphql_api_config_refusal(
    authentication_type: &str,
    open_id_connect_config: Option<&OpenIDConnectConfig>,
    user_pool_config: bool,
    lambda_authorizer_config: bool,
    additional_authentication_providers: &[AdditionalAuthenticationProvider],
    owner_contact: Option<&str>,
) -> Option<String> {
    if let Some(config) = missing_auth_config(
        Some(authentication_type),
        open_id_connect_config.is_some(),
        user_pool_config,
        lambda_authorizer_config,
    ) {
        return Some(api_level_missing_config_message(config));
    }
    for (position, provider) in additional_authentication_providers.iter().enumerate() {
        if let Some(config) = missing_auth_config(
            provider.authentication_type.as_deref(),
            provider.open_id_connect_config.is_some(),
            provider.user_pool_config.is_some(),
            provider.lambda_authorizer_config.is_some(),
        ) {
            return Some(format!(
                "{config} is missing for additional authentication provider {}",
                position + 1
            ));
        }
    }

    // The set of types already named holds at most one entry per value of the
    // modelled `AuthenticationType` enum, because the request is refused as soon as a
    // type repeats: its size is bounded by the model, not by the length of the list
    // the caller sent, and membership is a set lookup rather than a scan of it.
    let mut specified = std::collections::BTreeSet::from([authentication_type.to_string()]);
    for (position, provider) in additional_authentication_providers.iter().enumerate() {
        let Some(provider_type) = provider.authentication_type.clone() else {
            continue;
        };
        if specified.contains(&provider_type) {
            return Some(format!(
                "Authentication type {provider_type} for additional authentication provider {} \
                 already specified on the API. It can only be specified once.",
                position + 1
            ));
        }
        specified.insert(provider_type);
    }

    for config in open_id_connect_config.into_iter().chain(
        additional_authentication_providers
            .iter()
            .filter_map(|provider| provider.open_id_connect_config.as_ref()),
    ) {
        if !is_absolute_url(&config.issuer) {
            return Some("Issuer is not a valid URL".to_string());
        }
    }

    if owner_contact.is_some_and(|contact| contact.chars().count() > OWNER_CONTACT_MAX_LEN) {
        return Some(format!(
            "ownerContact accepts 0-{OWNER_CONTACT_MAX_LEN} characters"
        ));
    }

    None
}

/// The configuration `authentication_type` is set up through and the request did
/// not carry, or `None` when it carried it (or the type needs none at all:
/// `API_KEY` and `AWS_IAM` are configured by nothing).
///
/// Source: the per-type configuration members of `CreateGraphqlApiRequest`,
/// `UpdateGraphqlApiRequest` and `AdditionalAuthenticationProvider` —
/// `openIDConnectConfig` ("The OIDC configuration"), `userPoolConfig` ("The Amazon
/// Cognito user pool configuration") and `lambdaAuthorizerConfig` ("Configuration
/// for Lambda function authorization").
fn missing_auth_config(
    authentication_type: Option<&str>,
    open_id_connect_config: bool,
    user_pool_config: bool,
    lambda_authorizer_config: bool,
) -> Option<&'static str> {
    match authentication_type? {
        "OPENID_CONNECT" if !open_id_connect_config => Some(OPEN_ID_CONNECT_CONFIG),
        "AMAZON_COGNITO_USER_POOLS" if !user_pool_config => Some(USER_POOL_CONFIG),
        "AWS_LAMBDA" if !lambda_authorizer_config => Some(LAMBDA_AUTHORIZER_CONFIG),
        _ => None,
    }
}

/// How the service states the absence of the API's OWN authentication
/// configuration, per configuration.
///
/// The service does not word these from one template: the recorded refusals state
/// the OIDC configuration's absence as "OpenID Connect Config missing" and the
/// Cognito user pool's as "User Pool Config is empty", so each is stated as the
/// service states it rather than derived from the other. No recording covers the
/// Lambda authorizer configuration, so its absence is worded the way the
/// per-provider refusal words a missing configuration, and the requirement that
/// wording answers is the 400 and `BadRequestException` rather than the text.
fn api_level_missing_config_message(config: &str) -> String {
    match config {
        OPEN_ID_CONNECT_CONFIG => format!("{OPEN_ID_CONNECT_CONFIG} missing"),
        USER_POOL_CONFIG => format!("{USER_POOL_CONFIG} is empty"),
        other => format!("{other} is missing"),
    }
}

/// Whether `candidate` is an absolute URL, as an OIDC issuer has to be.
///
/// Parsed with the `url` crate (WHATWG URL), so the judgement is a real parse
/// rather than a string test: a relative reference, a bare host or a value with no
/// scheme is not one. A scheme-relative or opaque URL (`mailto:…`) carries no host,
/// which an issuer an ID token is checked against must have.
fn is_absolute_url(candidate: &str) -> bool {
    ::url::Url::parse(candidate).is_ok_and(|parsed| parsed.has_host())
}

/// The modelled bounds and alphabet of an `EnvironmentVariableKey`, and whether
/// `candidate` satisfies them.
///
/// Source: the `EnvironmentVariableKey` shape — `@length` 2..64 and `@pattern`
/// `^[A-Za-z]+\w*$` — which `PutGraphqlApiEnvironmentVariables` also states in prose:
/// "Keys must begin with a letter. Keys must be at least two characters long. Keys can
/// only contain letters, numbers, and the underscore character (_)." A map KEY is not
/// checked by the request boundary (only map values and the entry count are), so the
/// service enforces it.
pub const ENVIRONMENT_VARIABLE_KEY_MIN_LEN: usize = 2;

/// The maximum length of an `EnvironmentVariableKey` ([`ENVIRONMENT_VARIABLE_KEY_MIN_LEN`]).
pub const ENVIRONMENT_VARIABLE_KEY_MAX_LEN: usize = 64;

/// Whether `candidate` is a well-formed `EnvironmentVariableKey`: between
/// [`ENVIRONMENT_VARIABLE_KEY_MIN_LEN`] and [`ENVIRONMENT_VARIABLE_KEY_MAX_LEN`]
/// characters, beginning with a letter and otherwise holding only letters, digits and
/// the underscore.
pub fn is_environment_variable_key(candidate: &str) -> bool {
    let length = candidate.chars().count();
    if !(ENVIRONMENT_VARIABLE_KEY_MIN_LEN..=ENVIRONMENT_VARIABLE_KEY_MAX_LEN).contains(&length) {
        return false;
    }
    let mut characters = candidate.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// AppSync's refusal text for an `environmentVariables` key that is not an
/// `EnvironmentVariableKey` ([`is_environment_variable_key`]): the rejected key and the
/// constraint it broke, so the caller can fix the request from the message alone.
pub fn invalid_environment_variable_key_message(key: &str) -> String {
    format!(
        "Invalid environment variable key {key}: a key must be between \
         {ENVIRONMENT_VARIABLE_KEY_MIN_LEN} and {ENVIRONMENT_VARIABLE_KEY_MAX_LEN} characters \
         long, must begin with a letter, and can only contain letters, numbers, and the \
         underscore character (_)."
    )
}

/// The minimum length of a `TagKey`. Modelled: `TagKey` carries `@length` 1..128.
pub const TAG_KEY_MIN_LEN: usize = 1;

/// The maximum length of a `TagKey` ([`TAG_KEY_MIN_LEN`]).
pub const TAG_KEY_MAX_LEN: usize = 128;

/// The key prefix AWS reserves for itself, which `TagKey`'s modelled pattern
/// (`^(?!aws:)[ a-zA-Z+-=._:/]+$`) rules out: a caller cannot set a tag under it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The number of tags one resource holds at most.
///
/// AppSync's per-resource tag quota, and the same number the modelled `TagMap` and
/// `TagKeyList` bound one call to (`@length` 1..50): a resource cannot carry more tags
/// than a caller can submit, so the per-call bound is also the per-resource ceiling.
pub const TAGS_PER_RESOURCE: usize = 50;

/// Whether `candidate` sits under the key prefix AWS reserves
/// ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// Compared WITHOUT regard to case, which is how the service states the rule: a caller
/// "cannot set tag keys starting with aws: (case insensitive)", so a key differing from
/// the prefix only in case is reserved too.
pub fn is_reserved_tag_key(candidate: &str) -> bool {
    candidate
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Whether `candidate` is a well-formed `TagKey`: between [`TAG_KEY_MIN_LEN`] and
/// [`TAG_KEY_MAX_LEN`] characters drawn from the alphabet `TagKey`'s modelled pattern
/// admits (`[ a-zA-Z+-=._:/]`, whose `+-=` range also admits the digits and `,;<`),
/// and not under the reserved prefix ([`is_reserved_tag_key`]).
///
/// A map KEY is not checked by the request boundary (only map values and the entry
/// count are), so the service enforces the key's own rule — the same split
/// [`is_environment_variable_key`] is there for.
pub fn is_tag_key(candidate: &str) -> bool {
    let length = candidate.chars().count();
    (TAG_KEY_MIN_LEN..=TAG_KEY_MAX_LEN).contains(&length)
        && !is_reserved_tag_key(candidate)
        && candidate
            .chars()
            .all(|character| matches!(character, ' ' | '_' | 'a'..='z' | 'A'..='Z' | '+'..='='))
}

/// AppSync's fixed text for a tag key under the prefix AWS reserves, carried by
/// `BadRequestException` (HTTP 400).
///
/// The reserved prefix is a rule of its own rather than one clause of the tag-key
/// grammar, and the service states it as one sentence that names the prefix and the
/// comparison: no part of it varies with the request, so the rejected key is not echoed.
pub const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// Why `key` is not a tag key a caller may set, as the text a `BadRequestException`
/// (HTTP 400) states it with, or `None` when it is one.
///
/// The two grounds are worded separately, because the service states them separately: a
/// key under the reserved prefix takes [`RESERVED_TAG_KEY_MESSAGE`], and a key outside
/// `TagKey`'s length bounds or alphabet takes [`invalid_tag_key_message`]. Shared by
/// every operation that carries tag keys, so one key is refused one way.
pub fn tag_key_refusal(key: &str) -> Option<String> {
    if is_reserved_tag_key(key) {
        return Some(RESERVED_TAG_KEY_MESSAGE.to_string());
    }
    if !is_tag_key(key) {
        return Some(invalid_tag_key_message(key));
    }
    None
}

/// AppSync's refusal text for a tag key that is not a `TagKey` ([`is_tag_key`]): the
/// rejected key and the constraint it broke, so the caller can fix the request from the
/// message alone.
pub fn invalid_tag_key_message(key: &str) -> String {
    format!(
        "Invalid tag key {key}: a key must be between {TAG_KEY_MIN_LEN} and {TAG_KEY_MAX_LEN} \
         characters long, cannot begin with {RESERVED_TAG_KEY_PREFIX}, and can only contain \
         letters, numbers, spaces and the characters + - = . _ : /"
    )
}

/// AppSync's fixed text for a tag whose VALUE carries a character tags may not hold,
/// carried by `BadRequestException` (HTTP 400).
///
/// The sentence states the permitted characters and nothing of the request, so the rejected
/// value is not echoed. The set it names is the one `TagValue`'s modelled pattern admits
/// (`smithy_query(query="shape", shape="TagValue")`: `^[\s\w+-=\.:/@]*$` — letters, digits,
/// the underscore and whitespace through `\w`/`\s`, and `. : / = + - @`).
pub const INVALID_TAG_VALUE_MESSAGE: &str =
    "Tags can only contain letters, numbers, spaces, and the following special characters: \
     _ . : / = + - @";

/// AppSync's refusal text for a tag write that would leave the resource holding more
/// than [`TAGS_PER_RESOURCE`] tags: the quota, and the number the resource would end up
/// with, so the caller can see how far past it the request went.
pub fn tag_limit_exceeded_message(requested: usize) -> String {
    format!(
        "A resource can have a maximum of {TAGS_PER_RESOURCE} tags, and this request would \
         leave it with {requested}."
    )
}

/// AppSync's fixed, request-independent text for an API that is absent, carried by
/// `NotFoundException` (HTTP 404) by every operation that resolves an `apiId`.
pub const API_NOT_FOUND_MESSAGE: &str = "API not found.";

/// AppSync's fixed, request-independent text for a tagged resource that is absent,
/// carried by `NotFoundException` (HTTP 404) by every operation that resolves a
/// `resourceArn`.
///
/// The tagging surface addresses a RESOURCE rather than an API — one route tags every
/// resource kind an ARN can name — so its refusal states the absent resource and names
/// no resource kind, where an operation addressing an `apiId` states the absent API
/// ([`API_NOT_FOUND_MESSAGE`]).
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// AppSync's fixed, request-independent text for a `resourceArn` that is not a resource
/// ARN at all, carried by `BadRequestException` (HTTP 400).
///
/// The tagging operations model no constraint-violation error, so a value their
/// `resourceArn` cannot be is refused as the malformed request it is, under the one
/// client-error shape they do model; the rejected value is not echoed.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn";

/// How the tagging surface states a `resourceArn` it refuses, for the value `candidate`
/// the request carried (`None` when the refusal is about a value this reading cannot
/// see).
///
/// The surface reads the ARN's VENDOR field — the third of
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>`, the namespace that says
/// whose resource the value names — before it reads anything else about the value: an
/// ARN of another vendor names nothing this service can address however the account
/// changes, so the refusal says which vendor was expected
/// ([`::service_common::validation_message::expected_vendor`]) instead of only that the
/// value was not a resource ARN. The expected vendor is this service's own namespace
/// ([`ARN_SERVICE`]), which is also the one the member's modelled pattern fixes, and
/// nothing of the submitted value is echoed.
///
/// Every other ground — a value that is no ARN at all, or an ARN of this vendor outside
/// the member's length bounds, account, resource type or id alphabet — keeps the surface's
/// bare sentence ([`INVALID_RESOURCE_ARN_MESSAGE`]): the vendor field of those values is
/// the one this service serves, or is not there to name.
///
/// Which ARNs are of this vendor is `service_common::arn`'s reading, the one parse of an
/// ARN's fields in this tree.
pub fn invalid_resource_arn_message(candidate: Option<&str>) -> String {
    match candidate.and_then(::service_common::arn::vendor) {
        Some(vendor) if vendor != ARN_SERVICE => {
            ::service_common::validation_message::expected_vendor(ARN_SERVICE)
        }
        _ => INVALID_RESOURCE_ARN_MESSAGE.to_string(),
    }
}

/// AppSync's fixed, request-independent text for a `nextToken` a list operation cannot
/// resume from, carried by `BadRequestException` (HTTP 400).
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str = "Invalid pagination token given.";

/// The text a `NotFoundException` (HTTP 404) carries for an association this account
/// does not hold.
///
/// AppSync's own fixed text for the absent association, as the service answers it when
/// a merge is initiated against an `associationId` that resolves to nothing: it states
/// the absent resource and names nothing of the request that asked for it, so it has no
/// variable part to fill. Shared with `GetSourceApiAssociation`, which resolves the same
/// pair by the same rule.
pub const ASSOCIATION_NOT_FOUND_MESSAGE: &str = "No source API association was found";

/// The text `BadRequestException` (HTTP 400) carries for a create that names a pair this
/// account already holds an association for.
///
/// AppSync's own fixed sentence for the duplicate: it states the resource that is in the
/// way and names nothing of the request, so it has no variable part to fill. Shared by
/// both association creates, which create ONE and the same link.
pub const ASSOCIATION_EXISTS_MESSAGE: &str = "SourceApiAssociation already exists.";

/// The text `BadRequestException` (HTTP 400) carries for a create that would give a
/// source API a SECOND Merged API.
///
/// A source API is merged into one Merged API, so the topology the request asks for
/// cannot exist. AppSync's own fixed sentence, naming neither API of the request.
pub const SOURCE_API_ALREADY_MERGED_MESSAGE: &str =
    "Source Api can not be associated with multiple Merged APIs.";

/// The text `BadRequestException` (HTTP 400) carries for a `DeleteGraphqlApi` naming an
/// API that is still a source API of a Merged API.
///
/// An association names two APIs and cannot outlive either, so AppSync refuses to leave
/// a Merged API pointing at a source API that is gone: the source API has to be
/// disassociated first. The sentence is fixed and names neither API.
pub const API_ASSOCIATED_TO_MERGED_API_MESSAGE: &str =
    "API can not be deleted as it is associated to a Merged API";

/// Whether `candidate` is a recognizable source-API association reference, i.e. whether
/// it could be an `associationId` the AppSync service generated.
///
/// An `associationId` is "the ID generated by the AppSync service for the source API
/// association" (`DisassociateSourceGraphqlApiRequest$associationId`), never a value a
/// caller chose, and AppSync generates it as a UUID in the canonical hyphenated form.
/// A value of neither that form nor the opaque form this service's own id counter mints
/// ([`is_opaque_id`]) references nothing the service could have issued, which is a defect
/// of the REQUEST rather than an association this account no longer holds; a well-formed
/// one is a reference the service could have issued, so a request carrying it is well
/// formed whether or not the association is still there, and the absent association is
/// then reported as the missing resource it is ([`ASSOCIATION_NOT_FOUND_MESSAGE`]).
pub fn is_association_id(candidate: &str) -> bool {
    is_uuid(candidate) || is_opaque_id(candidate)
}

/// Whether `candidate` is a UUID written in the canonical hyphenated lowercase form,
/// i.e. the form the AppSync service's generated identifiers take on the wire.
///
/// Parsed by the `uuid` crate rather than by a hand-rolled scan, and compared back
/// against the canonical rendering so only that one spelling of a UUID is accepted.
fn is_uuid(candidate: &str) -> bool {
    ::uuid::Uuid::parse_str(candidate)
        .is_ok_and(|parsed| parsed.hyphenated().to_string() == candidate)
}

/// AppSync's refusal text for an `associationId` that resolves to no association of the
/// Merged API addressed, where the operation treats that as a MALFORMED request rather
/// than an absent resource ([`AssociationLookupRefusal::AssociationNotFound`]).
///
/// Built from the request: the rejected `associationId` is named, so the caller can see
/// which value was refused, exactly as [`invalid_api_id_message`] names a rejected API
/// id.
pub fn invalid_association_id_message(association_id: &str) -> String {
    format!("Invalid SourceApi Association ID {association_id}")
}

/// The text `ConcurrentModificationException` (HTTP 409) carries when the modification
/// already in progress is one of a Merged API's SOURCE-API ASSOCIATIONS.
///
/// AppSync scopes this conflict to the contended resource rather than reporting it as an
/// account-wide one: the refusal names the Merged API whose associations are being
/// updated and asks the caller to retry, so a caller racing on one Merged API can tell
/// which one is busy. Every association operation that can meet the conflict addresses
/// exactly one Merged API, so the Merged API's own `apiId` is what fills the template —
/// whichever identifier form the request named it by.
pub fn associations_being_updated_message(merged_api_id: &str) -> String {
    format!(
        "SourceApiAssociations for Merged API {merged_api_id} are currently being updated, please \
         wait and try again."
    )
}

/// AppSync's refusal text for an API that is on a side of an association its
/// `GraphQLApiType` cannot take: the type that side requires and the API that was
/// refused.
///
/// One template for both sides — an association links a source API of type
/// [`GRAPHQL_API_TYPE`] to a Merged API of type [`MERGED_API_TYPE`]
/// (`GraphqlApi$apiType`, `SourceApiAssociation`) — so the refusal states the type the
/// addressed side had to have rather than wording each side separately.
pub fn invalid_api_type_message(expected_api_type: &str, api_id: &str) -> String {
    format!("Invalid API type, expected api to be type {expected_api_type} for api : {api_id}")
}

/// The ARN namespace (and endpoint prefix) this service's resources carry.
pub const ARN_SERVICE: &str = "appsync";

/// The prefix of every AWS ARN, i.e. what tells an `…Identifier` member's ARN form
/// apart from its API-id form.
pub const ARN_PREFIX: &str = "arn:";

/// The `GraphQLApiType` of a standard GraphQL API — one that can be the SOURCE of an
/// association. Source: the `GraphQLApiType` enum and `GraphqlApi$apiType` ("whether
/// the GraphQL API is a standard API (GRAPHQL) or merged API (MERGED)").
pub const GRAPHQL_API_TYPE: &str = "GRAPHQL";

/// The `GraphQLApiType` of a Merged API — the only kind of API source APIs associate
/// WITH. Source: the `GraphQLApiType` enum.
pub const MERGED_API_TYPE: &str = "MERGED";

/// The `MergeType` an association takes when `sourceApiAssociationConfig` names none:
/// "Valid merge types are `MANUAL_MERGE` (default) and `AUTO_MERGE`"
/// (`SourceApiAssociationConfig$mergeType`).
pub const DEFAULT_MERGE_TYPE: &str = "MANUAL_MERGE";

/// The `GraphQLApiIntrospectionConfig` a GraphQL API takes when the request that
/// created or updated it named none: "If no value is provided, the introspection
/// configuration will be set to `ENABLED` by default"
/// (`GraphqlApi$introspectionConfig`).
pub const DEFAULT_INTROSPECTION_CONFIG: &str = "ENABLED";


/// The `SourceApiAssociationStatus` a fresh association carries: creating it schedules
/// the merge of the source API's schema into the Merged API, which has not run yet.
pub const MERGE_SCHEDULED: &str = "MERGE_SCHEDULED";

/// The `SourceApiAssociationStatus` an association reaches once its scheduled merge
/// has been performed.
pub const MERGE_SUCCESS: &str = "MERGE_SUCCESS";

/// The `sourceApiAssociationStatusDetail` that goes with [`MERGE_SCHEDULED`].
pub const MERGE_SCHEDULED_DETAIL: &str =
    "The source API merge is scheduled and has not been performed yet.";

/// The `sourceApiAssociationStatusDetail` that goes with [`MERGE_SUCCESS`].
pub const MERGE_SUCCESS_DETAIL: &str = "The source API was merged into the Merged API.";

/// How long after an association is created the merge it scheduled comes due.
///
/// A chosen span, inside the emulator's 250-750 ms band for a transition the clock
/// owes: AppSync reports a new association in the non-terminal [`MERGE_SCHEDULED`]
/// status, so the span is never zero and a caller reading straight after the create
/// still observes it. Nothing sleeps for it — the due instant is stored on the record
/// and [`SourceApiAssociationRecord::settle`] applies it.
pub const MERGE_SPAN: Duration = Duration::from_millis(500);

/// The `SourceApiAssociationStatus` an association whose deletion has been asked for
/// carries until the deletion has been performed. Source: the
/// `SourceApiAssociationStatus` enum, whose deletion values this is the first of.
pub const DELETION_SCHEDULED: &str = "DELETION_SCHEDULED";

/// The `sourceApiAssociationStatusDetail` that goes with [`DELETION_SCHEDULED`].
pub const DELETION_SCHEDULED_DETAIL: &str =
    "The source API association deletion is scheduled and has not been performed yet.";

/// How long after a disassociate the deletion it scheduled comes due.
///
/// A chosen span, inside the emulator's 250-750 ms band for a transition the clock
/// owes: a disassociate reports the association in the non-terminal
/// [`DELETION_SCHEDULED`] status, so the span is never zero and a caller reading
/// straight after the call still observes it. Nothing sleeps for it — the due instant
/// is stored on the record and [`ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::settle_associations`]
/// applies it.
pub const DELETION_SPAN: Duration = Duration::from_millis(500);

/// The smallest `ttl` an API cache accepts, in seconds.
///
/// Source: `CreateApiCacheRequest$ttl` and `UpdateApiCacheRequest$ttl`, both
/// documented "TTL in seconds for cache entries. Valid values are 1–3,600 seconds".
/// The members target an unconstrained `Long`, so the window is the service's to
/// enforce rather than the request boundary's.
pub const CACHE_TTL_MIN_SECONDS: i64 = 1;

/// The largest `ttl` an API cache accepts, in seconds
/// ([`CACHE_TTL_MIN_SECONDS`] states the source).
pub const CACHE_TTL_MAX_SECONDS: i64 = 3_600;

/// Whether `ttl` is inside the documented cache-entry window
/// ([`CACHE_TTL_MIN_SECONDS`]..=[`CACHE_TTL_MAX_SECONDS`]).
pub fn is_cache_ttl(ttl: i64) -> bool {
    (CACHE_TTL_MIN_SECONDS..=CACHE_TTL_MAX_SECONDS).contains(&ttl)
}

/// AppSync's refusal text for a `ttl` outside the documented window
/// ([`is_cache_ttl`]): the window itself, built from the two bounds, so the caller can
/// correct the value from the message alone.
pub fn invalid_cache_ttl_message() -> String {
    format!(
        "TTL value cannot be smaller than {CACHE_TTL_MIN_SECONDS} second and larger than \
         {CACHE_TTL_MAX_SECONDS} seconds."
    )
}

/// The `ApiCacheStatus` a cache being provisioned carries: "The instance is currently
/// creating."
pub const CACHE_CREATING: &str = "CREATING";

/// The `ApiCacheStatus` a cache whose settings are being applied carries.
pub const CACHE_MODIFYING: &str = "MODIFYING";

/// The `ApiCacheStatus` of an instance the service could not bring up. Source: the
/// `ApiCacheStatus` enum, which defines it as "The instance has failed creation".
pub const CACHE_FAILED: &str = "FAILED";

/// The `ApiCacheStatus` a provisioned cache carries: "The instance is available for
/// use."
pub const CACHE_AVAILABLE: &str = "AVAILABLE";

/// How long after a cache is created or updated the instance becomes available.
///
/// A chosen span, inside the emulator's 250-750 ms band for a transition the clock
/// owes: AppSync reports a fresh cache in the non-terminal [`CACHE_CREATING`] status,
/// so the span is never zero and a caller reading straight after the create still
/// observes it. Nothing sleeps for it — the due instant is stored on the record and
/// [`ApiCacheRecord::settle`] applies it.
pub const CACHE_SPAN: Duration = Duration::from_millis(500);

/// One of the cache operations that ADDRESS an API's cache — the call a refusal about
/// that cache names.
///
/// AppSync's absent-cache refusal names the call that was attempted rather than the
/// resource alone, so each handler hands its own variant to
/// [`absent_cache_message`] and no handler borrows another's wording. `DeleteApiCache`
/// is not among them: a teardown asks for the absence of the cache, so it is satisfied
/// by an empty slot and renders no absent-cache refusal at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCall {
    /// `GetApiCache`.
    Get,
    /// `UpdateApiCache`.
    Update,
    /// `FlushApiCache`.
    Flush,
}

impl CacheCall {
    /// The verb a refusal names this call by: what the caller was trying to do to the
    /// cache.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Update => "update",
            Self::Flush => "flush",
        }
    }
}

/// The text `NotFoundException` (HTTP 404) carries when `call` found no cache to
/// address on an API that never had one: the call that was attempted, the absent
/// resource, and the call that would create it.
pub fn cache_not_found_message(call: CacheCall) -> String {
    format!(
        "Unable to {} the cache as it doesn't exist, please create the cache first.",
        call.verb()
    )
}

/// The text `NotFoundException` (HTTP 404) carries when `call` found no cache to
/// address on an API whose cache the last teardown removed
/// ([`GraphqlApiRecord::api_cache_was_deleted`]): a cache that was torn down is not a
/// cache that was never configured, so the refusal names the deletion and points the
/// caller at re-creating it rather than at creating a first one.
pub fn cache_deleted_message(call: CacheCall) -> String {
    format!(
        "Unable to {} the cache as it has been deleted, please re-create the cache.",
        call.verb()
    )
}

/// The text `NotFoundException` (HTTP 404) carries when `call` found no cache to
/// address, for either reason an API can have none: `was_deleted` says whether a delete
/// emptied the cache slot, which the refusal reports as such.
pub fn absent_cache_message(call: CacheCall, was_deleted: bool) -> String {
    if was_deleted {
        cache_deleted_message(call)
    } else {
        cache_not_found_message(call)
    }
}

/// The text `BadRequestException` (HTTP 400) carries for a create on an API that
/// already holds a cache: an API holds at most one, so caching is either enabled on it
/// or not.
pub const CACHE_ALREADY_EXISTS_MESSAGE: &str = "The API has already enabled caching.";

/// The text `BadRequestException` (HTTP 400) carries for a delete of an instance that
/// was never created ([`ApiCacheRecord::was_created`]): one sentence for the whole
/// not-deletable class, naming neither the API nor the status the instance is in.
pub const CACHE_NOT_DELETABLE_MESSAGE: &str =
    "Cannot delete cache as it is either not available or its creation failed.";

/// The `DataSourceType` of an HTTP endpoint, the one type whose need for a service
/// role depends on how the endpoint is authorized ([`data_source_needs_service_role`]).
pub const HTTP_DATA_SOURCE_TYPE: &str = "HTTP";

/// The `AuthorizationType` an `HttpDataSourceConfig` selects to have AppSync sign its
/// requests to the endpoint with IAM credentials — which it can only do by assuming a
/// role. Source: the `AuthorizationType` enum, whose only value this is.
pub const IAM_AUTHORIZATION_TYPE: &str = "AWS_IAM";

/// Every `DataSourceType` AppSync reaches by assuming the data source's service role.
///
/// Source: `DataSource$type`, which says what each type IS (a Lambda function, a
/// DynamoDB table, an OpenSearch domain, an EventBridge configuration, the Bedrock
/// runtime, a relational database), and `serviceRoleArn` — "The system assumes this
/// role when accessing the data source". The two types left out are the two AppSync
/// reaches without credentials of its own: `NONE` ("There is no data source") and
/// [`HTTP_DATA_SOURCE_TYPE`] (a plain endpoint), the latter only while its
/// `authorizationConfig` does not select [`IAM_AUTHORIZATION_TYPE`].
pub const ROLE_ASSUMING_DATA_SOURCE_TYPES: &[&str] = &[
    "AMAZON_BEDROCK_RUNTIME",
    "AMAZON_DYNAMODB",
    "AMAZON_ELASTICSEARCH",
    "AMAZON_EVENTBRIDGE",
    "AMAZON_OPENSEARCH_SERVICE",
    "AWS_LAMBDA",
    "RELATIONAL_DATABASE",
];

/// Whether these HTTP endpoint settings have AppSync sign its calls to the endpoint
/// with IAM credentials, which it can only do by assuming the data source's service
/// role ([`data_source_needs_service_role`]).
///
/// `false` for a data source that carries no `httpConfig`, and for one whose
/// `authorizationConfig` names another authorization type. Shared by the create and
/// the update, so the two cannot disagree about one configuration.
pub fn selects_iam_authorization(http_config: Option<&HttpDataSourceConfig>) -> bool {
    http_config
        .and_then(|config| config.authorization_config.as_ref())
        .is_some_and(|authorization| {
            authorization.authorization_type == IAM_AUTHORIZATION_TYPE
        })
}

/// Whether AppSync must assume a role to reach a data source of this `type`, i.e.
/// whether `serviceRoleArn` has to be present for the data source to be invocable.
///
/// `iam_authorized` says whether the request's `httpConfig` selects
/// [`IAM_AUTHORIZATION_TYPE`], which is what makes an otherwise credential-free HTTP
/// endpoint need a role; it is ignored for every other type.
pub fn data_source_needs_service_role(data_source_type: &str, iam_authorized: bool) -> bool {
    if data_source_type == HTTP_DATA_SOURCE_TYPE {
        return iam_authorized;
    }
    ROLE_ASSUMING_DATA_SOURCE_TYPES.contains(&data_source_type)
}

/// The text `BadRequestException` (HTTP 400) carries for a data source whose type
/// needs a service role ([`data_source_needs_service_role`]) and whose request carried
/// none: the absent member.
pub const SERVICE_ROLE_NOT_SPECIFIED_MESSAGE: &str = "Service role arn not specified.";

/// The service role a data source of this `type` KEEPS out of the `serviceRoleArn` a
/// create or update request carried.
///
/// `serviceRoleArn` is the role "the system assumes [...] when ACCESSING the data
/// source", so it is part of a data source only where AppSync reaches the data source
/// by assuming a role ([`data_source_needs_service_role`]): a type AppSync reaches
/// without credentials of its own — `NONE`, and an HTTP endpoint whose
/// `authorizationConfig` does not select [`IAM_AUTHORIZATION_TYPE`] — has no service
/// role, so a role such a request carries is not stored and no `serviceRoleArn` is
/// reported for it. Shared by the create and the update, so the stored data source and
/// every read of it agree.
pub fn data_source_service_role(
    data_source_type: &str,
    iam_authorized: bool,
    service_role_arn: Option<String>,
) -> Option<String> {
    service_role_arn
        .filter(|_| data_source_needs_service_role(data_source_type, iam_authorized))
}

/// Whether `candidate` could identify an API at all: an API id ([`is_api_id`]) or a
/// full resource ARN.
///
/// Those are the two forms every `sourceApiIdentifier` / `mergedApiIdentifier` member
/// documents — "only require the API ID value or ARN of the \[…\] API" — so a value
/// that is neither names no API and the call is a bad request rather than a not-found.
pub fn is_api_identifier(candidate: &str) -> bool {
    candidate.starts_with(ARN_PREFIX) || is_api_id(candidate)
}

/// The stored API `identifier` names, in either accepted form ([`is_api_identifier`]):
/// an ARN is matched against the API's own ARN, anything else against its `apiId`.
/// `None` when this account holds no such API.
pub fn api_by_identifier<'a>(
    apis: &'a BTreeMap<String, GraphqlApiRecord>,
    identifier: &str,
) -> Option<&'a GraphqlApiRecord> {
    if identifier.starts_with(ARN_PREFIX) {
        apis.values().find(|record| record.arn == identifier)
    } else {
        apis.get(identifier)
    }
}

/// The region the caller addressed, read from the request's SigV4 credential scope.
///
/// Every ARN this service mints carries it, so a resource is reported in the region it
/// was created in. Falls back to [`DEFAULT_REGION`] for an unsigned request, which
/// carries no scope to read.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The region an unsigned request is served in, the emulator's default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState>)
}

/// The page size this service serves when a list call names no `maxResults`.
///
/// The list operations of this service all model `maxResults` on the same shape, with
/// a range of 0..25 and a default of `0`, i.e. "unspecified", so an unspecified page is
/// the whole modelled maximum.
pub const DEFAULT_PAGE_SIZE: usize = 25;

/// One page of a list operation: the items to return and the `nextToken` that
/// resumes after them (`None` when the page is the last one).
#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: Vec<&'a T>,
    pub next_token: Option<String>,
}

/// Cut the page `next_token` and `max_results` select out of `ordered`.
///
/// The cursor is the id of the first item of the NEXT page, so it keys on the
/// ordering itself rather than on an index: items created or deleted between two
/// pages never shift an already-returned item into or out of view twice.
/// `None` when `next_token` names no item in `ordered` — a caller that cannot
/// resume answers the list operation's modelled client error.
pub fn paginate<'a, T, K>(
    ordered: &[&'a T],
    key: K,
    next_token: Option<&str>,
    max_results: i32,
) -> Option<Page<'a, T>>
where
    K: Fn(&T) -> &str,
{
    paginate_by(ordered, |item| key(item).to_string(), next_token, max_results)
}

/// Cut the same page out of `ordered` where an item's cursor is COMPOSED rather than
/// carried by one member, so the key function hands back an owned value.
///
/// One pagination for both: [`paginate`] is this function over a borrowed member, so a
/// listing keyed on a single id and one keyed on a pair cut their pages, bound their
/// sizes and report their cursors identically.
pub fn paginate_by<'a, T, K>(
    ordered: &[&'a T],
    key: K,
    next_token: Option<&str>,
    max_results: i32,
) -> Option<Page<'a, T>>
where
    K: Fn(&T) -> String,
{
    let start = match next_token {
        None => 0,
        Some(token) => ordered.iter().position(|item| key(item) == token)?,
    };
    let page_size = if max_results <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        max_results as usize
    };
    let end = start.saturating_add(page_size).min(ordered.len());
    Some(Page {
        items: ordered[start..end].to_vec(),
        next_token: ordered.get(end).map(|item| key(item)),
    })
}

/// The API's endpoint URIs, built from the DNS record of each endpoint: the
/// realtime endpoint is a WebSocket URL and every other endpoint an HTTPS one, both
/// addressing the API's `/graphql` path on the hostname `dns` publishes.
fn uris_from_dns(dns: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    dns.iter()
        .map(|(endpoint, host)| {
            let scheme = if endpoint == REALTIME_ENDPOINT {
                "wss"
            } else {
                "https"
            };
            (endpoint.clone(), format!("{scheme}://{host}/graphql"))
        })
        .collect()
}

/// The key AppSync gives the realtime (WebSocket) endpoint in `dns` and `uris`.
pub const REALTIME_ENDPOINT: &str = "REALTIME";

/// The key AppSync gives the GraphQL (HTTPS) endpoint in `dns` and `uris`.
pub const GRAPHQL_ENDPOINT: &str = "GRAPHQL";

/// The key AppSync gives the HTTP endpoint of an `Api` in its `dns` records:
/// `Api$dns` "will include an HTTP and a real-time endpoint", the realtime one under
/// [`REALTIME_ENDPOINT`].
pub const HTTP_ENDPOINT: &str = "HTTP";

/// The DNS records AppSync publishes for a GRAPHQL API it created: one hostname per
/// endpoint, both on the API's own endpoint host label — except the REALTIME one, which
/// is the address the API's real-time socket is SERVED on.
///
/// `http_endpoint` is the key the HTTPS endpoint is published under — a GraphQL API
/// publishes it as [`GRAPHQL_ENDPOINT`] and derives its `uris` from these records — so a
/// create and an update cannot drift on the hostname shape.
///
/// `realtime_authority` is the `host:port` the API's real-time endpoint is reached at
/// ([`crate::service::graphql_api`]). AWS publishes the API's
/// `<label>.appsync-realtime-api.<region>.amazonaws.com` hostname there; this emulator
/// publishes the address that endpoint is REACHED at, because the whole purpose of the
/// value is that a client dials it — a subscriber completes a WebSocket handshake against
/// `wss://<value>/graphql` — and no client on this machine resolves an AWS hostname
/// (`service_common::data_plane_addressing`). An Event API publishes the addresses of both
/// its endpoints the same way ([`event_api_dns`]).
pub fn api_dns(
    host_label: &str,
    region: &str,
    http_endpoint: &str,
    realtime_authority: &str,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            http_endpoint.to_string(),
            format!("{host_label}.appsync-api.{region}.amazonaws.com"),
        ),
        (
            REALTIME_ENDPOINT.to_string(),
            realtime_authority.to_string(),
        ),
    ])
}

/// The DNS records an Event API publishes for the two endpoints it is SERVED on: the
/// `host:port` of the API's own HTTP socket under [`HTTP_ENDPOINT`] and of its real-time
/// socket under [`REALTIME_ENDPOINT`] (`Api$dns` "will include an HTTP and a real-time
/// endpoint").
///
/// AWS publishes the API's `appsync-api` and `appsync-realtime-api` hostnames there; this
/// emulator publishes the addresses those endpoints are REACHED at, because the whole
/// purpose of the value is that a client dials it — a publisher POSTs `/event` to the HTTP
/// one and a subscriber opens `/event/realtime` on the real-time one — and no client on
/// this machine resolves an AWS hostname
/// (`service_common::data_plane_addressing`). The two endpoints stay distinct, by PORT
/// rather than by host label, and each is a socket this emulator answers on
/// ([`crate::service::event_api`]).
pub fn event_api_dns(http_authority: &str, realtime_authority: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (HTTP_ENDPOINT.to_string(), http_authority.to_string()),
        (
            REALTIME_ENDPOINT.to_string(),
            realtime_authority.to_string(),
        ),
    ])
}

/// A `BTreeMap` field as the modelled map shape (`HashMap`).
pub fn map_out(map: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Wall-clock nanoseconds since the epoch, used only to seed the id counter.
pub(crate) fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never
/// revisits a value before the counter wraps.
pub(crate) fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The alphabet AppSync's opaque ids are written in: RFC 4648 base-32, lowercased
/// (`a`-`z` and `2`-`7`).
///
/// Both the ids this service MINTS ([`base32_13`]) and the test that recognises one it
/// could have minted ([`is_opaque_id`]) are written against this one alphabet, so a
/// minted id is always recognisable as one.
const OPAQUE_ID_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// `value` as 13 base-32 characters over the alphabet AppSync's opaque ids are
/// written in ([`OPAQUE_ID_ALPHABET`]).
fn base32_13(value: u64) -> String {
    (0..13)
        .map(|i| {
            let shift = 5 * (12 - i);
            let digit = (value >> shift) & 0x1f;
            char::from(OPAQUE_ID_ALPHABET[digit as usize])
        })
        .collect()
}

/// Whether `candidate` has the shape of an opaque id AppSync itself issues: exactly
/// [`API_ID_LEN`] characters, every one of them from [`OPAQUE_ID_ALPHABET`].
///
/// This is a test on the FORM of a reference, not on what the store holds: a value that
/// fails it is one the service could never have generated, so no resource of this
/// account was ever addressable by it.
pub fn is_opaque_id(candidate: &str) -> bool {
    candidate.len() == API_ID_LEN
        && candidate
            .bytes()
            .all(|byte| OPAQUE_ID_ALPHABET.contains(&byte))
}
