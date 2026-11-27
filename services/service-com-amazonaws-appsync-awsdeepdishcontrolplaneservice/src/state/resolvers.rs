// SPDX-License-Identifier: CC0-1.0

//! The resolvers attached to a GraphQL API's fields: how one is kept, addressed and
//! reported.
//!
//! A resolver is "attached" to ONE field of ONE type of its API (`CreateResolver`'s URI
//! is `/v1/apis/{apiId}/types/{typeName}/resolvers`, and every operation that addresses
//! one takes the pair `typeName`/`fieldName`), so a stored resolver is keyed by that
//! pair within its API and no id is minted for it. The projection below is the single
//! body every operation that reports a resolver answers with, so a create and the reads
//! cannot disagree about one resolver.

use crate::ops::types::{AppSyncRuntime, CachingConfig, PipelineConfig, Resolver, SyncConfig};

use super::GraphqlApiRecord;

/// The pair a resolver is held under within its API: the type name and the field name
/// that together address it.
pub type ResolverKey = (String, String);

/// One stored resolver of a `GraphqlApiRecord`.
///
/// Every member of the modelled `Resolver` shape a create or update call can set is
/// kept, so a reader projects exactly what was written rather than a subset.
#[derive(Debug, Clone)]
pub struct ResolverRecord {
    /// Creation order, used to keep a listing (and its `nextToken` cursor) stable
    /// across calls: a resolver is keyed by a caller-supplied pair, which carries the
    /// caller's alphabet rather than the order the resolvers were attached in.
    pub seq: u64,
    pub type_name: String,
    pub field_name: String,
    pub arn: String,
    pub data_source_name: Option<String>,
    pub request_mapping_template: Option<String>,
    pub response_mapping_template: Option<String>,
    /// The `ResolverKind` this resolver is of, always one of [`UNIT_KIND`] or
    /// [`PIPELINE_KIND`]: a request that names none is a UNIT resolver, which is "the
    /// default resolver type" (`Resolver$kind`).
    pub kind: String,
    pub pipeline_config: Option<PipelineConfig>,
    pub sync_config: Option<SyncConfig>,
    pub caching_config: Option<CachingConfig>,
    pub max_batch_size: i32,
    pub runtime: Option<AppSyncRuntime>,
    pub code: Option<String>,
    /// The `ResolverLevelMetricsConfig` the request asked for, or `None` when it asked
    /// for none: the model documents no default for it.
    pub metrics_config: Option<String>,
}

impl ResolverRecord {
    /// This resolver's cursor within its API: the type and field pair that addresses
    /// it, in one value.
    ///
    /// A listing scoped to ONE type resumes from the field name alone, but one scoped
    /// to the whole API cannot — two types of an API may carry a resolver on fields of
    /// the same name — so a listing that crosses types keys its page on the pair.
    pub fn cursor(&self) -> String {
        format!("{}/{}", self.type_name, self.field_name)
    }

    /// Whether this resolver invokes the function `function_id`, i.e. whether the
    /// function is a step of its pipeline.
    ///
    /// `pipelineConfig` holds "A list of `Function` objects" (`PipelineConfig$functions`)
    /// — the functions the resolver invokes — so a resolver that carries no pipeline
    /// invokes no function.
    pub fn invokes_function(&self, function_id: &str) -> bool {
        self.pipeline_config
            .as_ref()
            .and_then(|pipeline| pipeline.functions.as_ref())
            .is_some_and(|functions| functions.iter().any(|function| function == function_id))
    }

    /// This record as the modelled `Resolver` shape.
    ///
    /// The single projection every operation that reports a resolver answers with, so
    /// the create and the reads cannot disagree about one resolver.
    pub fn to_resolver(&self) -> Resolver {
        Resolver {
            type_name: Some(self.type_name.clone()),
            field_name: Some(self.field_name.clone()),
            data_source_name: self.data_source_name.clone(),
            resolver_arn: Some(self.arn.clone()),
            request_mapping_template: self.request_mapping_template.clone(),
            response_mapping_template: self.response_mapping_template.clone(),
            kind: Some(self.kind.clone()),
            pipeline_config: self.pipeline_config.clone(),
            sync_config: self.sync_config.clone(),
            caching_config: self.caching_config.clone(),
            max_batch_size: self.max_batch_size,
            runtime: self.runtime.clone(),
            code: self.code.clone(),
            metrics_config: self.metrics_config.clone(),
        }
    }
}

impl GraphqlApiRecord {
    /// Whether this API holds a type under `type_name` — what an operation addressing a
    /// type BY NAME asks before it answers for it.
    ///
    /// A type is one the API holds when its stored type system declares it, or when a
    /// resolver of this API is attached to a type of that name: this service attaches a
    /// resolver to the `typeName` the request names without waiting for a schema to declare
    /// it, so an attachment is itself a statement that the API has that type.
    pub fn holds_type(&self, type_name: &str) -> bool {
        self.types.contains_key(type_name)
            || self
                .resolvers
                .values()
                .any(|resolver| resolver.type_name == type_name)
    }

    /// The resolver attached to `field_name` of `type_name`, or `None` when the field
    /// carries none.
    ///
    /// The single lookup every operation that addresses one resolver uses, so a read
    /// and a write of the same resolver can never disagree about which fields carry
    /// one.
    pub fn resolver(&self, type_name: &str, field_name: &str) -> Option<&ResolverRecord> {
        self.resolvers
            .get(&(type_name.to_string(), field_name.to_string()))
    }

    /// The refusal a write of a resolver on `field_name` of `type_name` earns from this API's
    /// ACTIVE SCHEMA, or `None` where that schema has nothing against it.
    ///
    /// A resolver is attached to a FIELD of a type of its API — `CreateResolver` "converts
    /// incoming requests" for one field, and its URI names the type the field belongs to — so
    /// a type the API's schema DECLARES decides which fields of it a resolver can be attached
    /// to: a name that type declares no field under is a field no query can select, and the
    /// write is refused with the modelled not-found error
    /// ([`no_field_named_message`]) rather than leaving the API holding a resolver bound to
    /// nothing.
    ///
    /// A type the schema declares NOTHING about is no judgement either way: this service
    /// attaches a resolver to the `typeName` the request names without waiting for a schema to
    /// declare it ([`Self::holds_type`]), so there is no declaration of its fields to weigh
    /// the field against.
    pub fn resolver_field_refusal(&self, type_name: &str, field_name: &str) -> Option<String> {
        let declared = self.types.get(type_name)?;
        declared
            .parsed
            .fields
            .iter()
            .all(|field| field.name != field_name)
            .then(|| no_field_named_message(field_name, type_name))
    }

    /// Every resolver attached to a field of `type_name`, in the order they were
    /// created.
    ///
    /// The order a listing pages over: the key pair is the caller's own vocabulary, so
    /// it carries no creation order, and the sequence each record keeps is the one
    /// total order every page is cut from.
    pub fn resolvers_of_type(&self, type_name: &str) -> Vec<&ResolverRecord> {
        let mut resolvers: Vec<&ResolverRecord> = self
            .resolvers
            .values()
            .filter(|resolver| resolver.type_name == type_name)
            .collect();
        resolvers.sort_by_key(|resolver| resolver.seq);
        resolvers
    }

    /// Every resolver of this API that invokes the function `function_id`, whatever type
    /// it is attached to, in the order they were created.
    ///
    /// A function is a step of a pipeline resolver, and a resolver of ANY type of the
    /// API may hold it, so the whole API's resolvers are the search space; the creation
    /// order each record keeps is the one total order a listing pages over.
    pub fn resolvers_using_function(&self, function_id: &str) -> Vec<&ResolverRecord> {
        let mut resolvers: Vec<&ResolverRecord> = self
            .resolvers
            .values()
            .filter(|resolver| resolver.invokes_function(function_id))
            .collect();
        resolvers.sort_by_key(|resolver| resolver.seq);
        resolvers
    }
}

/// The `ResolverKind` of a resolver that "run[s] a GraphQL query against a single data
/// source", and the kind a request that names none asks for: "A UNIT resolver is the
/// default resolver type" (`Resolver$kind`).
pub const UNIT_KIND: &str = "UNIT";

/// The `ResolverKind` of a resolver that "invoke[s] a series of `Function` objects in a
/// serial manner" (`Resolver$kind`).
pub const PIPELINE_KIND: &str = "PIPELINE";

/// The resource portion of a resolver's ARN: a resolver is attached to a field of a type
/// of its API, so it is named under the API's own `apis/{apiId}` path, as
/// `GetResolver`'s URI (`/v1/apis/{apiId}/types/{typeName}/resolvers/{fieldName}`)
/// addresses it.
pub fn resolver_resource(api_id: &str, type_name: &str, field_name: &str) -> String {
    format!("apis/{api_id}/types/{type_name}/resolvers/{field_name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a `dataSourceName` the resolver's
/// own API does not hold: AppSync's sentence for an unresolvable data source REFERENCE,
/// whose one variable part is the name the request asked for.
///
/// A resolver "run[s] a GraphQL query against a single data source" (`Resolver$kind`) of
/// the API the request addresses (`/v1/apis/{apiId}/types/{typeName}/resolvers`), so a name
/// that API does not hold names no data source the resolver could resolve against — a name
/// held only by ANOTHER API among them, which is why the lookup is scoped to the API in the
/// request.
pub fn no_data_source_named_message(name: &str) -> String {
    format!("No data source found named {name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a resolver addressed at a field the
/// type does not declare: AppSync's sentence for an unresolvable FIELD reference, whose two
/// variable parts are the field and the type the request named.
///
/// A resolver resolves one field of one type, so a field the type declares none of is the
/// reference that resolves to nothing — the absence the sentence states.
pub fn no_field_named_message(field_name: &str, type_name: &str) -> String {
    format!("No field named {field_name} found on type {type_name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a field that carries no
/// resolver: AppSync's fixed sentence, which names nothing of the request that asked for
/// it. Shared by every operation that addresses a resolver by its type and field, so
/// they word one absence one way.
pub const RESOLVER_NOT_FOUND_MESSAGE: &str = "No resolver found.";

/// The text a `BadRequestException` (HTTP 400) carries for a create addressing a field
/// that already carries a resolver: a field is resolved by at most one resolver, so the
/// resolver already attached is in the way and the sentence has no variable part.
pub const RESOLVER_EXISTS_MESSAGE: &str = "Only one resolver is allowed per field.";

/// The text a `BadRequestException` (HTTP 400) carries for a UNIT resolver carrying a
/// pipeline configuration: a UNIT resolver has no pipeline, so the member describes
/// nothing it could run.
pub const PIPELINE_CONFIG_NOT_ALLOWED_MESSAGE: &str = "PipelineConfig should not be specified.";

/// The text a `BadRequestException` (HTTP 400) carries for a PIPELINE resolver carrying
/// no pipeline configuration: the functions a pipeline resolver invokes are what the
/// member holds, so a PIPELINE resolver without one runs nothing.
pub const PIPELINE_CONFIG_REQUIRED_MESSAGE: &str = "PipelineConfig can't be null.";

/// The text a `BadRequestException` (HTTP 400) carries for a UNIT resolver naming no
/// data source: a UNIT resolver "run[s] a GraphQL query against a single data source"
/// (`Resolver$kind`), so without one it resolves against nothing.
pub const DATA_SOURCE_NAME_MISSING_MESSAGE: &str = "Data source name not specified.";

/// The AppSync service quota for resolvers on ONE API, which also bounds the per-API
/// collection a create writes to: a create past it is refused, so repeating the call
/// cannot grow the store without limit.
pub const RESOLVERS_PER_API: usize = 10_000;

/// How many functions ONE `PIPELINE` resolver is composed of at most.
///
/// `PipelineConfig$functions` targets a list with no modelled bound, so the ceiling is the
/// service's: a pipeline of exactly this many functions is stored and one function more is
/// refused with `BadRequestException` (HTTP 400), the client error every operation that
/// writes a resolver models.
pub const FUNCTIONS_PER_PIPELINE_RESOLVER: usize = 10;

/// The text a `BadRequestException` (HTTP 400) carries for a pipeline longer than
/// [`FUNCTIONS_PER_PIPELINE_RESOLVER`]: the ceiling is the sentence's one variable part,
/// and nothing of the submitted list is echoed.
pub fn pipeline_functions_limit_message() -> String {
    format!("A resolver can only be composed of {FUNCTIONS_PER_PIPELINE_RESOLVER} functions.")
}

/// The refusal text for a resolver definition AppSync could not run, or `None` when the
/// request describes one it could.
///
/// The rule is over the MEMBERS that carry the definition rather than over one request's
/// values, and it is the same whichever operation writes the resolver:
///
/// * a PIPELINE resolver is defined by the functions it invokes, which `pipelineConfig`
///   holds ("The pipeline configuration for a resolver of kind `PIPELINE`"), so it needs
///   one, holding at most [`FUNCTIONS_PER_PIPELINE_RESOLVER`] of them
///   ([`pipeline_functions_limit_message`]);
/// * a UNIT resolver has no pipeline, so it must carry no `pipelineConfig`, and it
///   "run[s] a GraphQL query against a single data source" (`Resolver$kind`), so it must
///   name that data source;
/// * `code` and `runtime` define each other — "When code is used, the `runtime` is
///   required" (`Resolver$code`) — which is the pair rule
///   [`super::code_runtime_refusal`] states for every operation that takes the two;
/// * a `runtime` and a mapping template are two definitions of one resolver, so they
///   cannot both be supplied ([`super::runtime_templates_refusal`], the same rule the
///   function writes carry), and that is decided before the pair rule above.
pub fn resolver_definition_refusal(
    kind: &str,
    data_source_name: Option<&str>,
    pipeline_config: Option<&PipelineConfig>,
    code: Option<&str>,
    runtime: Option<&AppSyncRuntime>,
    request_mapping_template: Option<&str>,
    response_mapping_template: Option<&str>,
) -> Option<String> {
    if kind == PIPELINE_KIND {
        let Some(pipeline_config) = pipeline_config else {
            return Some(PIPELINE_CONFIG_REQUIRED_MESSAGE.to_string());
        };
        let functions = pipeline_config.functions.as_deref().unwrap_or_default();
        if functions.len() > FUNCTIONS_PER_PIPELINE_RESOLVER {
            return Some(pipeline_functions_limit_message());
        }
    } else if pipeline_config.is_some() {
        return Some(PIPELINE_CONFIG_NOT_ALLOWED_MESSAGE.to_string());
    } else if data_source_name.is_none_or(str::is_empty) {
        return Some(DATA_SOURCE_NAME_MISSING_MESSAGE.to_string());
    }
    super::runtime_templates_refusal(
        runtime,
        request_mapping_template,
        response_mapping_template,
    )
    .or_else(|| super::code_runtime_refusal(code, runtime))
}
