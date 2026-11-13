// SPDX-License-Identifier: CC0-1.0

//! Executing one GraphQL request against a GraphQL API's own resolvers: the work behind the
//! API's `GRAPHQL` endpoint ([`super::query`]).
//!
//! A GraphQL API is not a document that only gets read back. A resolver is "attached" to one
//! field of one type of its API and carries "the `resolver` code that contains the request
//! and response functions" (`Resolver$code`), run by the APPSYNC_JS runtime; so a request
//! that selects that field is answered by what the code RETURNS, under the field's response
//! key, and by nothing else.
//!
//! What this module does with one request, in order:
//!
//! 1. The DOCUMENT is parsed by the maintained GraphQL parser this crate already reads a
//!    schema with (`apollo-parser`) — never by anything hand-written — and the operation to
//!    run is the one the request named, or the document's only one. Its selected fields are
//!    read with their aliases, their arguments (the request's own `variables` substituted
//!    for every variable) and their sub-selections, with a fragment spread and an inline
//!    fragment spliced in where the document uses one.
//! 2. Each field is resolved against the record the control plane wrote: the resolver
//!    attached to that field of the type the field belongs to, the data source it names, and
//!    — for a `PIPELINE` resolver — the functions it invokes, in order. A field that carries
//!    no resolver takes its value from the object its parent resolved to, which is what a
//!    GraphQL object's own member is.
//! 3. The CODE is RUN, by the same engine embedded in this process that `EvaluateCode`
//!    evaluates a resolver with ([`crate::ops::evaluate_code::appsync_js`]): the `request`
//!    function over the field's `ctx`, then the `response` function over the result the data
//!    source produced. Every bound that module declares holds here — a wall clock, a step
//!    budget, memory and stack ceilings and an answer ceiling — and each run gets a fresh
//!    engine, so nothing of one field's run is reachable from the next.
//! 4. The sub-selections of each resolved value are PROJECTED onto it, as a GraphQL result
//!    carries exactly the fields the request selected.
//!
//! A field whose code raised, and one this emulator cannot resolve at all, is reported as an
//! error of that field's own path with the value `null`, which is how a GraphQL result
//! states a field that did not resolve.

use ::serde_json::{json, Map as JsonMap, Value as Json};

use super::PlaneContext;
use crate::ops::evaluate_code::appsync_js::{self, Outcome};
use crate::state::{DataSourceRecord, GraphqlApiRecord, ResolverRecord};

/// The GraphQL request a client POSTs: "a JSON object with a `query`", with the operation to
/// run and the variables its document is parameterised by.
#[derive(Debug, Clone, Default)]
pub struct GraphqlRequest {
    /// The GraphQL document.
    pub query: String,
    /// The operation within it to run, when the request names one.
    pub operation_name: Option<String>,
    /// The variables the document's own variable definitions take their values from.
    pub variables: JsonMap<String, Json>,
    /// The request's own fields, which a resolver reads as `ctx.request.headers`.
    pub headers: JsonMap<String, Json>,
}

/// What executing one request came to: the data it resolved, and the errors of the fields
/// that did not resolve.
#[derive(Debug, Clone, Default)]
pub struct Answer {
    /// The selected data, or `None` for a request no operation of which was executed at all
    /// — which is what a GraphQL result reports with no `data` member.
    pub data: Option<Json>,
    /// One entry per field that did not resolve, and the request-level faults that stopped
    /// the execution before any field ran.
    pub errors: Vec<Json>,
}

impl Answer {
    /// The answer that reports one request-level fault and no data.
    fn refused(message: String) -> Self {
        Self {
            data: None,
            errors: vec![json!({"message": message})],
        }
    }

    /// This answer as the body a GraphQL response carries: `data` where the execution
    /// produced any, and `errors` where anything did not resolve.
    pub fn document(&self) -> Json {
        let mut body = JsonMap::new();
        if let Some(data) = &self.data {
            body.insert(DATA_MEMBER.to_string(), data.clone());
        }
        if !self.errors.is_empty() {
            body.insert(ERRORS_MEMBER.to_string(), Json::Array(self.errors.clone()));
        }
        Json::Object(body)
    }
}

/// The members a GraphQL response carries.
const DATA_MEMBER: &str = "data";
const ERRORS_MEMBER: &str = "errors";

/// How deep a selection set this endpoint executes.
///
/// An emulator resource guard, NOT an AWS quota: every level of a selection is work the
/// REQUEST chooses, and a resolved field may be resolved again at the next level, so the
/// descent is bounded in host code rather than by what the document asks for.
const MAX_SELECTION_DEPTH: usize = 16;

/// How many resolvers one request may run.
///
/// An emulator resource guard, NOT an AWS quota, for the same reason as the depth: each run
/// is a fresh engine, so the number of them one request can ask for is bounded here.
const MAX_RESOLVER_RUNS: usize = 64;

/// How many fields one selection set may select, for the same reason.
const MAX_SELECTED_FIELDS: usize = 64;

/// How many fields one request's whole document may expand to, every level and every
/// fragment splice together.
///
/// An emulator resource guard, NOT an AWS quota, and the one that bounds the EXPANSION
/// rather than one level of it: see [`Expansion`] for why the width and the depth bounds do
/// not, and why a document past this is refused before anything is built.
const MAX_SELECTED_NODES: usize = 1_024;

/// The `DataSourceType` of a data source AppSync resolves against WITHOUT leaving itself:
/// `NONE` is "There is no data source", so the request function's own `payload` is the
/// result the response function is handed ("local resolvers" in the AppSync developer
/// guide).
const LOCAL_DATA_SOURCE: &str = "NONE";

/// The member a request function returns the result of a local resolver in.
const PAYLOAD_MEMBER: &str = "payload";

/// The names of the two functions a resolver's `code` defines (`Resolver$code`: "the
/// `resolver` code that contains the request and response functions").
const REQUEST_FUNCTION: &str = "request";
const RESPONSE_FUNCTION: &str = "response";

/// Execute `request` against the API `context` serves.
///
/// The store is read once per field, never across a run of the engine: a resolver's
/// definition is copied out under the lock and the code runs with the lock released, so one
/// field's run cannot hold up every other request to this service.
pub async fn execute(context: &PlaneContext, request: &GraphqlRequest) -> Answer {
    // The parse happens in a scope of its own: the parser's own tree is not `Send`, so what
    // leaves this call is the operation and the fields it selects, read out of that tree and
    // owned, and the tree itself is gone before anything is awaited.
    let (kind, fields) = match read_operation(request) {
        Ok(read) => read,
        Err(refusal) => return Answer::refused(refusal),
    };
    // The type the operation's fields belong to: the type the API's schema binds as that
    // operation's root, which is what says WHICH resolvers the selection addresses.
    let Some(root_type) = context
        .state
        .lock()
        .graphql_apis
        .get(&context.api_id)
        .map(|api| match kind {
            OperationKind::Query => api.query_root_type(),
            OperationKind::Mutation => api.mutation_root_type(),
            OperationKind::Subscription => api.subscription_root_type(),
        })
    else {
        return Answer::refused(crate::state::API_NOT_FOUND_MESSAGE.to_string());
    };
    if kind == OperationKind::Subscription {
        return Answer::refused(SUBSCRIPTIONS_ARE_REALTIME.to_string());
    }
    let mut execution = Execution {
        context,
        request,
        runs: 0,
        errors: Vec::new(),
    };
    let data = execution
        .resolve_fields(&root_type, &Json::Null, &fields, Vec::new(), 0)
        .await;
    Answer {
        data: Some(data),
        errors: execution.errors,
    }
}

/// Read `request`'s document: the operation to run and the fields it selects, owned, with
/// the request's own variables already substituted.
///
/// Synchronous on purpose — nothing of the parser's tree escapes it.
fn read_operation(request: &GraphqlRequest) -> Result<(OperationKind, Vec<SelectedField>), String> {
    let parsed = ::apollo_parser::Parser::new(&request.query)
        .recursion_limit(crate::state::DEFINITION_RECURSION_LIMIT)
        .token_limit(crate::state::DEFINITION_TOKEN_LIMIT)
        .parse();
    if let Some(error) = parsed.errors().next() {
        return Err(format!("{NOT_A_DOCUMENT} {}", error.message()));
    }
    let document = parsed.document();
    let operation = selected_operation(&document, request.operation_name.as_deref())
        .ok_or_else(|| no_such_operation(request.operation_name.as_deref()))?;
    let mut expansion = Expansion {
        document: &document,
        variables: &request.variables,
        remaining: MAX_SELECTED_NODES,
        spreading: Vec::new(),
    };
    let fields = expansion.selection_set(operation.selection_set.as_ref(), 0)?;
    Ok((operation.kind, fields))
}

/// One request being executed: what it is executing against, and what it has produced so
/// far.
struct Execution<'a> {
    context: &'a PlaneContext,
    request: &'a GraphqlRequest,
    /// How many resolvers this request has run, which [`MAX_RESOLVER_RUNS`] bounds.
    runs: usize,
    /// The errors of the fields that did not resolve, in the order they were resolved.
    errors: Vec<Json>,
}

impl Execution<'_> {
    /// Resolve every field of one selection set against `source`, the value the parent field
    /// resolved to, and answer the object a GraphQL result carries them in.
    ///
    /// Boxed, because a field's own sub-selection is resolved the same way: the recursion is
    /// over a future, which Rust needs a pointer for.
    fn resolve_fields<'f>(
        &'f mut self,
        type_name: &'f str,
        source: &'f Json,
        fields: &'f [SelectedField],
        path: Vec<Json>,
        depth: usize,
    ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Json> + Send + 'f>> {
        Box::pin(async move {
            let mut resolved = JsonMap::new();
            for field in fields {
                let mut field_path = path.clone();
                field_path.push(json!(field.key));
                let value = self
                    .resolve_field(type_name, source, field, &field_path, depth)
                    .await;
                resolved.insert(field.key.clone(), value);
            }
            Json::Object(resolved)
        })
    }

    /// Resolve ONE field: run the resolver attached to it where the API holds one, take the
    /// member of the parent object where it does not, and project the field's own
    /// sub-selection onto whatever that came to.
    async fn resolve_field(
        &mut self,
        type_name: &str,
        source: &Json,
        field: &SelectedField,
        path: &[Json],
        depth: usize,
    ) -> Json {
        if depth >= MAX_SELECTION_DEPTH {
            self.errors
                .push(field_error(path, &format!("{TOO_DEEP} {MAX_SELECTION_DEPTH}.")));
            return Json::Null;
        }
        let plan = self.plan(type_name, &field.name);
        let value = match plan {
            // A field the API attaches no resolver to is the member of the object its
            // parent resolved to, which is what a GraphQL object's own field is.
            None => source.get(&field.name).cloned().unwrap_or(Json::Null),
            Some(plan) => {
                if self.runs >= MAX_RESOLVER_RUNS {
                    self.errors
                        .push(field_error(path, &format!("{TOO_MANY_RUNS} {MAX_RESOLVER_RUNS}.")));
                    return Json::Null;
                }
                self.runs = self.runs.saturating_add(1);
                match self.run(&plan, type_name, source, field).await {
                    Ok(value) => value,
                    Err(reason) => {
                        self.errors.push(field_error(path, &reason));
                        return Json::Null;
                    }
                }
            }
        };
        if field.selections.is_empty() {
            return value;
        }
        // The field's own type is what says which resolvers its sub-selection addresses.
        let child_type = self.field_type_name(type_name, &field.name);
        self.projected(&child_type, value, field, path, depth).await
    }

    /// `value` with `field`'s sub-selection projected onto it: a list element by element, an
    /// object by its selected fields, and anything else as it stands — a scalar carries no
    /// sub-selection, and `null` resolves to `null`.
    ///
    /// A list counts as a level of its own, so a value nested in lists is held to the same
    /// descent bound a selection set is.
    fn projected<'f>(
        &'f mut self,
        child_type: &'f str,
        value: Json,
        field: &'f SelectedField,
        path: &'f [Json],
        depth: usize,
    ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Json> + Send + 'f>> {
        Box::pin(async move {
            if depth >= MAX_SELECTION_DEPTH {
                self.errors
                    .push(field_error(path, &format!("{TOO_DEEP} {MAX_SELECTION_DEPTH}.")));
                return Json::Null;
            }
            match value {
                Json::Array(elements) => {
                    let mut projected: Vec<Json> = Vec::with_capacity(elements.len());
                    for (index, element) in elements.into_iter().enumerate() {
                        let mut element_path = path.to_vec();
                        element_path.push(json!(index));
                        projected.push(
                            self.projected(
                                child_type,
                                element,
                                field,
                                &element_path,
                                depth.saturating_add(1),
                            )
                            .await,
                        );
                    }
                    Json::Array(projected)
                }
                Json::Object(_) => {
                    self.resolve_fields(
                        child_type,
                        &value,
                        &field.selections,
                        path.to_vec(),
                        depth.saturating_add(1),
                    )
                    .await
                }
                other => other,
            }
        })
    }

    /// What this API's records say resolves `field_name` of `type_name`, or `None` when no
    /// resolver is attached to that field.
    ///
    /// Copied out under ONE read of the store, so a resolver, the data source it names and
    /// the functions it invokes cannot be read from records that changed in between — and
    /// so that no guard is held while the engine runs.
    fn plan(&self, type_name: &str, field_name: &str) -> Option<ResolverPlan> {
        let store = self.context.state.lock();
        let api = store.graphql_apis.get(&self.context.api_id)?;
        let resolver = api.resolver(type_name, field_name)?;
        Some(ResolverPlan {
            code: resolver.code.clone(),
            has_templates: resolver.request_mapping_template.is_some()
                || resolver.response_mapping_template.is_some(),
            data_source: step_data_source(api, resolver.data_source_name.as_deref()),
            functions: pipeline_steps(api, resolver),
            is_pipeline: resolver.kind == crate::state::PIPELINE_KIND,
        })
    }

    /// The name of the type `field_name` of `type_name` resolves to, or the empty name when
    /// the API's schema declares none — a field whose type is unknown addresses no
    /// resolver, so its sub-selection is taken from the value it resolved to.
    fn field_type_name(&self, type_name: &str, field_name: &str) -> String {
        let store = self.context.state.lock();
        store
            .graphql_apis
            .get(&self.context.api_id)
            .and_then(|api| api.types.get(type_name))
            .and_then(|record| {
                record
                    .parsed
                    .fields
                    .iter()
                    .find(|field| field.name == field_name)
            })
            .and_then(|field| field.value_type.as_ref().map(named_type))
            .unwrap_or_default()
    }

    /// Run `plan` for one field and answer the value the field resolves to.
    ///
    /// A UNIT resolver runs its own `request` function, resolves the result against the data
    /// source it names, and runs its `response` function over that result. A PIPELINE
    /// resolver runs its `request` function as the step before its functions, each function
    /// the same way in the order the pipeline names them — each handed the previous step's
    /// result as `ctx.prev.result` — and its `response` function as the step after them.
    /// The stash is carried from step to step, as "an object available inside each resolver
    /// and function handler".
    async fn run(
        &mut self,
        plan: &ResolverPlan,
        type_name: &str,
        source: &Json,
        field: &SelectedField,
    ) -> Result<Json, String> {
        let mut stash = Json::Object(JsonMap::new());
        let base = self.field_context(type_name, source, field);
        let mut previous = if plan.is_pipeline {
            // The pipeline's before step: what it returns is the `ctx.prev.result` the
            // first function is handed.
            match &plan.code {
                Some(code) => {
                    returned(
                        code,
                        REQUEST_FUNCTION,
                        &step_context(&base, &stash, &Json::Null, None),
                        &mut stash,
                    )
                    .await?
                }
                None => Json::Null,
            }
        } else {
            Json::Null
        };
        if plan.is_pipeline {
            if plan.functions.is_empty() {
                return Err(NO_PIPELINE_FUNCTIONS.to_string());
            }
            for step in &plan.functions {
                previous = self
                    .run_step(step, &base, &mut stash, &previous)
                    .await
                    .map_err(|reason| format!("{}: {reason}", step.name))?;
            }
            let Some(code) = &plan.code else {
                return Ok(previous);
            };
            return returned(
                code,
                RESPONSE_FUNCTION,
                &step_context(&base, &stash, &previous, Some(&previous)),
                &mut stash,
            )
            .await;
        }
        let step = PipelineStep {
            name: field.name.clone(),
            code: plan.code.clone(),
            has_templates: plan.has_templates,
            data_source: plan.data_source.clone(),
        };
        self.run_step(&step, &base, &mut stash, &previous).await
    }

    /// Run ONE step — a UNIT resolver, or one function of a pipeline — against its own data
    /// source, and answer what its `response` function returned.
    async fn run_step(
        &mut self,
        step: &PipelineStep,
        base: &FieldContext,
        stash: &mut Json,
        previous: &Json,
    ) -> Result<Json, String> {
        let Some(code) = &step.code else {
            // A definition written as VTL mapping templates rather than as APPSYNC_JS
            // code: this endpoint runs the code the APPSYNC_JS runtime runs, and nothing
            // of this emulator evaluates a Velocity template, so the field is reported as
            // one that did not resolve rather than answered with a value nothing computed.
            return Err(if step.has_templates {
                MAPPING_TEMPLATES_NOT_RUN.to_string()
            } else {
                NO_DEFINITION.to_string()
            });
        };
        let requested = returned(
            code,
            REQUEST_FUNCTION,
            &step_context(base, stash, &Json::Null, Some(previous)),
            stash,
        )
        .await?;
        let result = self.data_source_result(step, &requested)?;
        returned(
            code,
            RESPONSE_FUNCTION,
            &step_context(base, stash, &result, Some(previous)),
            stash,
        )
        .await
    }

    /// The result the step's data source produces for the request its `request` function
    /// returned.
    ///
    /// `NONE` is "There is no data source", so the request's own `payload` IS the result —
    /// the local resolver AppSync documents. Every other type names a resource of another
    /// service, which this endpoint does not call, so the field is reported as one that did
    /// not resolve rather than answered with a value nothing produced.
    fn data_source_result(&self, step: &PipelineStep, requested: &Json) -> Result<Json, String> {
        let Some(data_source) = &step.data_source else {
            return Err(crate::state::DATA_SOURCE_NAME_MISSING_MESSAGE.to_string());
        };
        if data_source.kind != LOCAL_DATA_SOURCE {
            return Err(format!(
                "{DATA_SOURCE_NOT_CALLED} {} data source {}.",
                data_source.kind, data_source.name
            ));
        }
        Ok(requested.get(PAYLOAD_MEMBER).cloned().unwrap_or(Json::Null))
    }

    /// The parts of a field's `ctx` that do not change between the steps that resolve it.
    fn field_context(&self, type_name: &str, source: &Json, field: &SelectedField) -> FieldContext {
        FieldContext {
            arguments: Json::Object(field.arguments.clone()),
            source: source.clone(),
            request: json!({"headers": Json::Object(self.request.headers.clone())}),
            info: json!({
                "fieldName": field.name,
                "parentTypeName": type_name,
                "variables": Json::Object(self.request.variables.clone()),
                "selectionSetList": field
                    .selections
                    .iter()
                    .map(|selected| json!(selected.name))
                    .collect::<Vec<Json>>(),
            }),
        }
    }
}

/// What resolves one field: the code, the data source and the pipeline the stored resolver
/// names, copied out of the store.
#[derive(Debug, Clone)]
struct ResolverPlan {
    /// The APPSYNC_JS `code` the resolver is defined by, or `None` for one defined another
    /// way.
    code: Option<String>,
    /// Whether the resolver carries mapping templates, which is the other way a definition
    /// is written.
    has_templates: bool,
    /// The data source the resolver names, where the API still holds it.
    data_source: Option<StepDataSource>,
    /// The steps of a `PIPELINE` resolver, in the order the pipeline invokes them.
    functions: Vec<PipelineStep>,
    /// Whether the resolver is of the `PIPELINE` kind.
    is_pipeline: bool,
}

/// One step of a resolution: a UNIT resolver, or one function of a pipeline.
#[derive(Debug, Clone)]
struct PipelineStep {
    /// What the step is called, which is what an error of the step names it by.
    name: String,
    code: Option<String>,
    has_templates: bool,
    data_source: Option<StepDataSource>,
}

/// The data source one step resolves against: its type, and the name it is held under.
#[derive(Debug, Clone)]
struct StepDataSource {
    kind: String,
    name: String,
}

/// The data source `name` of `api`, or `None` when the API holds none under that name.
fn step_data_source(api: &GraphqlApiRecord, name: Option<&str>) -> Option<StepDataSource> {
    let name = name?;
    api.data_sources
        .get(name)
        .map(|record: &DataSourceRecord| StepDataSource {
            kind: record.r#type.clone(),
            name: record.name.clone(),
        })
}

/// The steps a `PIPELINE` resolver invokes, in the order its `pipelineConfig` names them.
///
/// A function the API no longer holds contributes a step with no definition, which is
/// reported as a step that could not run rather than skipped: the pipeline names it.
fn pipeline_steps(api: &GraphqlApiRecord, resolver: &ResolverRecord) -> Vec<PipelineStep> {
    resolver
        .pipeline_config
        .as_ref()
        .and_then(|pipeline| pipeline.functions.clone())
        .unwrap_or_default()
        .iter()
        .map(|function_id| match api.functions.get(function_id) {
            Some(function) => PipelineStep {
                name: function.name.clone(),
                code: function.code.clone(),
                has_templates: function.request_mapping_template.is_some()
                    || function.response_mapping_template.is_some(),
                data_source: step_data_source(api, Some(&function.data_source_name)),
            },
            None => PipelineStep {
                name: function_id.clone(),
                code: None,
                has_templates: false,
                data_source: None,
            },
        })
        .collect()
}

/// The parts of a field's `ctx` that every step resolving it shares.
#[derive(Debug, Clone)]
struct FieldContext {
    arguments: Json,
    source: Json,
    request: Json,
    info: Json,
}

/// The `ctx` one step is run with: what the field carries, the stash as the steps before it
/// left it, the result the data source produced and the previous step's result.
///
/// The member names are the ones APPSYNC_JS code reads them under: `arguments` (with `args`
/// as its documented alias), `source`, `identity`, `request`, `info`, `stash`, `result` and
/// `prev`.
fn step_context(
    base: &FieldContext,
    stash: &Json,
    result: &Json,
    previous: Option<&Json>,
) -> Json {
    json!({
        "arguments": base.arguments,
        "args": base.arguments,
        "source": base.source,
        // The material a call presents is judged by the endpoint before anything runs
        // (`super::authorized`), and an API key names no principal, so no identity is
        // reported to the code.
        "identity": Json::Null,
        "request": base.request,
        "info": base.info,
        "stash": stash,
        "result": result,
        "prev": json!({"result": previous.cloned().unwrap_or(Json::Null)}),
        "error": Json::Null,
    })
}

/// Run `function` of `code` over `context` and answer what it returned, carrying the stash
/// it left behind onto `stash`.
///
/// `Err` is a step that did not return: the code raised (`util.error` is how a resolver
/// refuses), it defines no such function, or one of the engine's bounds stopped it. The text
/// is the engine's own.
async fn returned(
    code: &str,
    function: &str,
    context: &Json,
    stash: &mut Json,
) -> Result<Json, String> {
    let evaluation =
        appsync_js::evaluate(code.to_string(), function.to_string(), context.clone()).await;
    if let Some(left) = evaluation
        .stash
        .as_deref()
        .and_then(|text| ::serde_json::from_str::<Json>(text).ok())
    {
        *stash = left;
    }
    match evaluation.outcome {
        Outcome::Returned(text) => Ok(::serde_json::from_str(&text).unwrap_or(Json::Null)),
        Outcome::Raised(message) | Outcome::Stopped(message) => Err(message),
        Outcome::Faults(faults) => Err(faults
            .first()
            .map(|fault| fault.value.clone())
            .unwrap_or_else(|| NO_DEFINITION.to_string())),
    }
}

/// One error of a field that did not resolve, as a GraphQL result reports one: where in the
/// result the field sits, and what went wrong.
fn field_error(path: &[Json], message: &str) -> Json {
    json!({"path": path.to_vec(), "message": message})
}

/// The named type a reference resolves to, whatever it is wrapped in: a list of non-null
/// `Post`s is still a reference to `Post`.
fn named_type(reference: &crate::state::TypeRef) -> String {
    match reference {
        crate::state::TypeRef::Named(name) => name.clone(),
        crate::state::TypeRef::List(inner) | crate::state::TypeRef::NonNull(inner) => {
            named_type(inner)
        }
    }
}

/// Which root operation type an operation's fields belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

/// One operation of a document: which kind it is, and the fields it selects.
struct Operation {
    kind: OperationKind,
    selection_set: Option<::apollo_parser::cst::SelectionSet>,
}

/// The operation to execute: the one the request named, or — for a request that names none —
/// the document's only operation, which is the only case GraphQL leaves unambiguous
/// ("if `operationName` is null ... the document is expected to contain only one
/// operation").
fn selected_operation(
    document: &::apollo_parser::cst::Document,
    operation_name: Option<&str>,
) -> Option<Operation> {
    use ::apollo_parser::cst;
    let mut operations: Vec<cst::OperationDefinition> = document
        .definitions()
        .filter_map(|definition| match definition {
            cst::Definition::OperationDefinition(operation) => Some(operation),
            _ => None,
        })
        .collect();
    let chosen = match operation_name {
        Some(wanted) => operations.into_iter().find(|operation| {
            operation
                .name()
                .is_some_and(|name| name.text().as_str() == wanted)
        })?,
        None if operations.len() == 1 => operations.remove(0),
        None => return None,
    };
    let kind = match chosen.operation_type() {
        None => OperationKind::Query,
        Some(declared) => {
            if declared.mutation_token().is_some() {
                OperationKind::Mutation
            } else if declared.subscription_token().is_some() {
                OperationKind::Subscription
            } else {
                OperationKind::Query
            }
        }
    };
    Some(Operation {
        kind,
        selection_set: chosen.selection_set(),
    })
}

/// One selected field of a selection set: the key it is reported under, the field it
/// selects, the arguments it carries and its own sub-selection.
#[derive(Debug, Clone)]
pub struct SelectedField {
    /// The response key: the alias where the selection carries one, and the field's own name
    /// otherwise ("the response key of a selection is its alias if one is defined").
    pub key: String,
    pub name: String,
    pub arguments: JsonMap<String, Json>,
    pub selections: Vec<SelectedField>,
}

/// Reading ONE request's document into the fields it selects: what the reading needs, and
/// what bounds how much of it the document may ask for.
///
/// The budget is what makes the expansion safe. A selection set's own width
/// ([`MAX_SELECTED_FIELDS`]) and the depth it may nest to ([`MAX_SELECTION_DEPTH`]) bound
/// each LEVEL, and their product does not bound the total — a chain of fragments each
/// spreading the next multiplies one level's width by the next's — so the number of fields
/// the whole document expands to is counted down from [`MAX_SELECTED_NODES`] across every
/// level and every splice. A document that asks for more is refused before the expansion is
/// built, so one request cannot choose how much memory reading it costs.
struct Expansion<'a> {
    document: &'a ::apollo_parser::cst::Document,
    variables: &'a JsonMap<String, Json>,
    /// How many more fields this document may expand to, all levels together.
    remaining: usize,
    /// The fragments being spliced right now, innermost last: a spread that re-enters one is
    /// refused rather than expanded, so a cyclic fragment is reported instead of fanning
    /// out until a bound stops it.
    spreading: Vec<String>,
}

impl Expansion<'_> {
    /// Read one selection set into the fields it selects, splicing in the selections of
    /// every fragment it spreads.
    fn selection_set(
        &mut self,
        selection_set: Option<&::apollo_parser::cst::SelectionSet>,
        depth: usize,
    ) -> Result<Vec<SelectedField>, String> {
        use ::apollo_parser::cst;
        if depth >= MAX_SELECTION_DEPTH {
            return Err(format!("{TOO_DEEP} {MAX_SELECTION_DEPTH}."));
        }
        let Some(selection_set) = selection_set else {
            return Ok(Vec::new());
        };
        let mut fields: Vec<SelectedField> = Vec::new();
        for selection in selection_set.selections() {
            if fields.len() >= MAX_SELECTED_FIELDS {
                return Err(format!("{TOO_MANY_FIELDS} {MAX_SELECTED_FIELDS}."));
            }
            match selection {
                cst::Selection::Field(selected) => {
                    let Some(name) = selected.name().map(|name| name.text().to_string()) else {
                        continue;
                    };
                    let key = selected
                        .alias()
                        .and_then(|alias| alias.name())
                        .map(|alias| alias.text().to_string())
                        .unwrap_or_else(|| name.clone());
                    // Spent BEFORE the field is read, so the sub-selection below is read
                    // under what is left rather than past it.
                    self.spend()?;
                    let arguments = read_arguments(selected.arguments(), self.variables);
                    let selections =
                        self.selection_set(selected.selection_set().as_ref(), depth + 1)?;
                    fields.push(SelectedField {
                        key,
                        name,
                        arguments,
                        selections,
                    });
                }
                // An inline fragment selects on the same object its parent does, so its own
                // selections are the parent's. The type condition decides nothing here: what
                // a field resolves to is the resolver attached to it, not the document's
                // guess at the object's type.
                cst::Selection::InlineFragment(inline) => {
                    let spliced = self.selection_set(inline.selection_set().as_ref(), depth + 1)?;
                    fields.extend(spliced);
                    self.within_width(fields.len())?;
                }
                cst::Selection::FragmentSpread(spread) => {
                    let Some(name) = spread
                        .fragment_name()
                        .and_then(|fragment| fragment.name())
                        .map(|name| name.text().to_string())
                    else {
                        continue;
                    };
                    if self.spreading.contains(&name) {
                        return Err(format!("{CYCLIC_FRAGMENT} {name}."));
                    }
                    let Some(definition) = fragment_definition(self.document, &name) else {
                        return Err(format!("{NO_SUCH_FRAGMENT} {name}."));
                    };
                    self.spreading.push(name);
                    let spliced = self.selection_set(Some(&definition), depth + 1);
                    self.spreading.pop();
                    fields.extend(spliced?);
                    self.within_width(fields.len())?;
                }
            }
        }
        Ok(fields)
    }

    /// Spend one field of this document's budget, or refuse the document for asking for more
    /// than [`MAX_SELECTED_NODES`] fields in all.
    fn spend(&mut self) -> Result<(), String> {
        if self.remaining == 0 {
            return Err(format!("{TOO_MANY_NODES} {MAX_SELECTED_NODES}."));
        }
        self.remaining -= 1;
        Ok(())
    }

    /// Refuse a selection set that `selected` fields wide: the width is checked again after
    /// a splice, because a fragment's own selections land in the set all at once and so can
    /// carry it past [`MAX_SELECTED_FIELDS`] rather than up to it.
    fn within_width(&self, selected: usize) -> Result<(), String> {
        if selected > MAX_SELECTED_FIELDS {
            return Err(format!("{TOO_MANY_FIELDS} {MAX_SELECTED_FIELDS}."));
        }
        Ok(())
    }
}

/// The selection set the fragment `name` defines, or `None` when the document defines no
/// such fragment.
fn fragment_definition(
    document: &::apollo_parser::cst::Document,
    name: &str,
) -> Option<::apollo_parser::cst::SelectionSet> {
    use ::apollo_parser::cst;
    document.definitions().find_map(|definition| {
        let cst::Definition::FragmentDefinition(fragment) = definition else {
            return None;
        };
        let defined = fragment
            .fragment_name()
            .and_then(|fragment| fragment.name())
            .map(|name| name.text().to_string())?;
        (defined == name).then(|| fragment.selection_set())?
    })
}

/// The arguments one selected field carries, as the document writes them and with the
/// request's own variables substituted for every variable it references.
fn read_arguments(
    arguments: Option<::apollo_parser::cst::Arguments>,
    variables: &JsonMap<String, Json>,
) -> JsonMap<String, Json> {
    let mut read = JsonMap::new();
    let Some(arguments) = arguments else {
        return read;
    };
    for argument in arguments.arguments() {
        let Some(name) = argument.name().map(|name| name.text().to_string()) else {
            continue;
        };
        let Some(value) = argument.value() else {
            continue;
        };
        read.insert(name, read_value(&value, variables, 0));
    }
    read
}

/// One GraphQL value as the document writes it, in the JSON a resolver reads it as.
///
/// A variable takes the value the request's `variables` gives it, and one the request does
/// not carry is `null` — "if a variable is not provided, it is considered to have no value".
fn read_value(
    value: &::apollo_parser::cst::Value,
    variables: &JsonMap<String, Json>,
    depth: usize,
) -> Json {
    use ::apollo_parser::cst::{CstNode, Value};
    if depth >= MAX_SELECTION_DEPTH {
        return Json::Null;
    }
    match value {
        Value::Variable(variable) => variable
            .name()
            .and_then(|name| variables.get(name.text().as_str()).cloned())
            .unwrap_or(Json::Null),
        Value::StringValue(text) => Json::String(String::from(text.clone())),
        Value::ListValue(list) => Json::Array(
            list.values()
                .map(|element| read_value(&element, variables, depth.saturating_add(1)))
                .collect(),
        ),
        Value::ObjectValue(object) => Json::Object(
            object
                .object_fields()
                .filter_map(|member| {
                    let name = member.name()?.text().to_string();
                    let value = member.value()?;
                    Some((
                        name,
                        read_value(&value, variables, depth.saturating_add(1)),
                    ))
                })
                .collect(),
        ),
        // An enum value is a name, which is how a resolver reads one; every remaining form
        // is a JSON literal already — an integer, a float, a boolean or `null` — so it is
        // read as the document wrote it.
        Value::EnumValue(enumerated) => enumerated
            .name()
            .map(|name| Json::String(name.text().to_string()))
            .unwrap_or(Json::Null),
        other => {
            let text = other.syntax().text().to_string();
            ::serde_json::from_str(text.trim()).unwrap_or(Json::Null)
        }
    }
}

/// How an operation the document does not hold is stated.
fn no_such_operation(operation_name: Option<&str>) -> String {
    match operation_name {
        Some(named) => format!("{NO_SUCH_OPERATION} {named}."),
        None => ONE_OPERATION.to_string(),
    }
}

/// The sentences this module's own refusals state.
const NOT_A_DOCUMENT: &str = "The request's query is not a GraphQL document:";
const NO_SUCH_OPERATION: &str = "This document declares no operation named";
const ONE_OPERATION: &str =
    "A request that names no operation must carry a document with exactly one operation.";
const SUBSCRIPTIONS_ARE_REALTIME: &str =
    "A subscription is served at this API's real-time endpoint.";
const NO_SUCH_FRAGMENT: &str = "This document spreads a fragment it does not define:";
const TOO_DEEP: &str = "This endpoint resolves a selection set no deeper than";
const TOO_MANY_FIELDS: &str = "A selection set carries at most";
const TOO_MANY_NODES: &str = "One request selects at most";
const CYCLIC_FRAGMENT: &str = "This document spreads a fragment into itself:";
const TOO_MANY_RUNS: &str = "One request runs at most";
const NO_DEFINITION: &str = "This field's resolver carries no definition to run.";
const MAPPING_TEMPLATES_NOT_RUN: &str =
    "This field's resolver is defined by mapping templates, which this endpoint does not \
     run; define it with APPSYNC_JS code.";
const DATA_SOURCE_NOT_CALLED: &str = "This endpoint does not call the";
const NO_PIPELINE_FUNCTIONS: &str = "This field's pipeline resolver invokes no function.";
