// SPDX-License-Identifier: CC0-1.0

//! What a GraphQL API REPORTS about the requests it answered: the class an error of a field
//! carries, and the `AWS/AppSync` series the API publishes about its own work.
//!
//! The rules these cases assert, one level above the recorded case (one `API_KEY` api, one
//! `NONE` data source, one APPSYNC_JS resolver raising with a class, one forged key and one
//! opted-in resolver/data-source pair):
//!
//! > 1. An error raised by `util.error(message, errorType)` — in either language a definition
//! >    is written in — is reported as one entry of the result's `errors`, under the field's
//! >    own `path`, carrying the message the raise was handed VERBATIM and, in `errorType`,
//! >    the class it named; a raise that named no class carries no `errorType` member at all,
//! >    and the field itself resolves to `null`.
//! > 2. Every request the API itself ANSWERED is counted in CloudWatch's `AWS/AppSync`
//! >    namespace under the single dimension `GraphQLAPIId` of that API: one `Requests`
//! >    datapoint, one `Latency` datapoint in `Milliseconds`, and one `4XXError` / `5XXError`
//! >    datapoint reading one where the status it carried is of that class and zero where it
//! >    is not — so one api's refusals never appear in another api's series. Every one of
//! >    those counts is published with no unit of its own, the span being the one measurement
//! >    that has one.
//! > 3. An api whose `enhancedMetricsConfig` asks for granular data publishes, BESIDE that
//! >    dimension and with exactly one key added, the series of what happened WITHIN a
//! >    request: `Requests`/`Latency`/`GraphQLError` under `Resolver` =
//! >    "<typeName>.<fieldName>" per resolver invocation, `Requests`/`Latency`/`Errors` under
//! >    `DataSource` per data source invocation, and `Requests`/`GraphQLError` under
//! >    `Operation` — the ROOT OPERATION TYPE executed — per operation. A `PER_…_METRICS`
//! >    behavior reports only the resolvers and data sources whose own `metricsConfig` is
//! >    `ENABLED`, a `FULL_REQUEST_…_METRICS` behavior reports every one of them whatever it
//! >    says, and an api that configured no enhanced metrics at all publishes none of these
//! >    series.
//!
//! The rows span what each rule generalises over: the class a raise names and the language it
//! is written in (code and a mapping template), whether it raises or appends, the response key
//! the field is reported under; the STATUS class of the answer (served, unauthorized for two
//! different reasons, a route the endpoint does not serve, a method it does not serve, a body
//! that carries no document) and how many apis are being driven; and every cell of the
//! enhanced opt-in matrix (opted in, opted out, overridden by the full-request behavior,
//! configured away altogether), with a resolver that resolves, one that raises, one whose data
//! source this endpoint does not call at all, and operations of two root operation types, one
//! of which the document names and one of which it does not.
//!
//! Nothing here is asserted off the store: every request is POSTed over a socket to the
//! address the api published for it, and every number is read back through CloudWatch's own
//! `ListMetrics` and `GetMetricStatistics`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::data_source_service_role::{needs_service_role, ROLE_ARN};
use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::{appsync, cloudwatch, emulator, metric_sum, statistics};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};

/// The namespace AppSync publishes its own series to.
const NAMESPACE: &str = "AWS/AppSync";

/// The dimension every series of a GraphQL api carries, and the three keys an ENHANCED series
/// adds exactly one of beside it; the service's own copies are
/// `service::graphql_api::metrics::{GRAPHQL_API_ID_DIMENSION, RESOLVER_DIMENSION,
/// DATA_SOURCE_DIMENSION, OPERATION_DIMENSION}`.
pub(crate) const API_DIMENSION: &str = "GraphQLAPIId";
pub(crate) const RESOLVER_DIMENSION: &str = "Resolver";
pub(crate) const DATA_SOURCE_DIMENSION: &str = "DataSource";
pub(crate) const OPERATION_DIMENSION: &str = "Operation";

/// The series these cases read; the service's own copies are
/// `service::graphql_api::metrics::{REQUESTS_METRIC, LATENCY_METRIC, CLIENT_ERROR_METRIC,
/// SERVER_ERROR_METRIC, GRAPHQL_ERROR_METRIC, DATA_SOURCE_ERROR_METRIC}`.
pub(crate) const REQUESTS: &str = "Requests";
pub(crate) const LATENCY: &str = "Latency";
const CLIENT_ERROR: &str = "4XXError";
const SERVER_ERROR: &str = "5XXError";
pub(crate) const GRAPHQL_ERROR: &str = "GraphQLError";
pub(crate) const DATA_SOURCE_ERRORS: &str = "Errors";

/// The `StandardUnit` a span is reported in, and the one a count AppSync publishes with no
/// unit of its own carries. Source: `StandardUnit`'s own values
/// (`smithy_query(query="shape",shape="StandardUnit")` on the CloudWatch model).
pub(crate) const MILLISECONDS: &str = "Milliseconds";
pub(crate) const NO_UNIT: &str = "None";

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
pub(crate) const GRAPHQL_PATH: &str = "/graphql";

/// The `runtime` a resolver defined by `code` names. Source: `AppSyncRuntime`'s own values
/// (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// One created GraphQL api: its id, the address its GraphQL endpoint is served on and a live
/// API key.
pub(crate) struct Api {
    pub(crate) api_id: String,
    graphql: String,
    pub(crate) api_key: String,
}

/// One resolver a case attaches: the type and field it resolves, the data source it names,
/// its definition and the `metricsConfig` it opts in (or out) with.
pub(crate) struct Resolver<'a> {
    type_name: &'a str,
    field: &'a str,
    data_source: &'a str,
    code: Option<String>,
    templates: Option<(&'a str, &'a str)>,
    metrics_config: Option<&'a str>,
}

impl<'a> Resolver<'a> {
    /// A resolver defined by APPSYNC_JS `code`, on a field of the query root.
    pub(crate) fn coded(field: &'a str, data_source: &'a str, code: String) -> Self {
        Self {
            type_name: "Query",
            field,
            data_source,
            code: Some(code),
            templates: None,
            metrics_config: None,
        }
    }

    /// A resolver defined by MAPPING TEMPLATES, which is the other form a definition is
    /// written in.
    fn templated(field: &'a str, data_source: &'a str, request: &'a str, response: &'a str) -> Self {
        Self {
            type_name: "Query",
            field,
            data_source,
            code: None,
            templates: Some((request, response)),
            metrics_config: None,
        }
    }

    /// The same resolver, attached to a field of another type of the schema.
    pub(crate) fn on_type(mut self, type_name: &'a str) -> Self {
        self.type_name = type_name;
        self
    }

    /// The same resolver, opting in or out of the enhanced resolver-level series.
    pub(crate) fn reporting(mut self, metrics_config: &'a str) -> Self {
        self.metrics_config = Some(metrics_config);
        self
    }
}

/// The schema the error-class cases are driven over: one field per way a definition reports
/// an error of its own.
const RAISING_SCHEMA: &str = "type Query {\n  \
     raised(message: String!, kind: String!): String\n  \
     untyped(message: String!): String\n  \
     appended(message: String!, kind: String!): String\n  \
     templated(message: String!, kind: String!): String\n}\n";

/// The data source every resolver of this suite resolves against: `NONE` is "There is no data
/// source", so the request half's own payload is the result.
const LOCAL: &str = "local";
/// What the appending resolver still answers beside the error it appended.
const APPENDED_VALUE: &str = "answered-anyway";

#[tokio::test]
async fn an_error_a_definition_raises_carries_the_class_it_named() {
    let router = emulator();
    let api = create_api(
        &router,
        "classes",
        None,
        RAISING_SCHEMA,
        &[(LOCAL, CALLED_TYPE, None)],
        &[
            Resolver::coded(
                "raised",
                LOCAL,
                "import { util } from '@aws-appsync/utils';\n\
                 export function request(ctx) { util.error(ctx.args.message, ctx.args.kind); }\n\
                 export function response(ctx) { return ctx.result; }"
                    .to_string(),
            ),
            Resolver::coded(
                "untyped",
                LOCAL,
                "import { util } from '@aws-appsync/utils';\n\
                 export function request(ctx) { util.error(ctx.args.message); }\n\
                 export function response(ctx) { return ctx.result; }"
                    .to_string(),
            ),
            Resolver::coded(
                "appended",
                LOCAL,
                format!(
                    "import {{ util }} from '@aws-appsync/utils';\n\
                     export function request(ctx) {{\n  \
                     util.appendError(ctx.args.message, ctx.args.kind);\n  \
                     return {{payload: \"{APPENDED_VALUE}\"}};\n}}\n\
                     export function response(ctx) {{ return ctx.result; }}"
                ),
            ),
            Resolver::templated(
                "templated",
                LOCAL,
                "$util.error($ctx.args.message, $ctx.args.kind)",
                "$util.toJson($ctx.result)",
            ),
        ],
    )
    .await;

    /// One row: the document a client POSTs, and what the one error it must report carries.
    struct Row<'a> {
        case: &'a str,
        query: String,
        /// The response key the error's `path` must name, and the value `data` carries for it.
        key: &'a str,
        value: Value,
        message: &'a str,
        /// The `errorType` the entry must carry, or `None` for an entry that must carry no
        /// such member at all.
        error_type: Option<&'a str>,
    }
    let rows: &[Row] = &[
        Row {
            case: "a raise naming a class of the author's own",
            query: "query { raised(message: \"the field refuses\", kind: \"Sim2RealInjected\") }"
                .to_string(),
            key: "raised",
            value: Value::Null,
            message: "the field refuses",
            error_type: Some("Sim2RealInjected"),
        },
        Row {
            case: "the same resolver under another class",
            query: "query { raised(message: \"a different sentence\", kind: \"BusinessRule\") }"
                .to_string(),
            key: "raised",
            value: Value::Null,
            message: "a different sentence",
            error_type: Some("BusinessRule"),
        },
        Row {
            case: "a class spelled the way a service's own errors are",
            query: "query { raised(message: \"not found here\", kind: \"DynamoDB:\
                    ConditionalCheckFailedException\") }"
                .to_string(),
            key: "raised",
            value: Value::Null,
            message: "not found here",
            error_type: Some("DynamoDB:ConditionalCheckFailedException"),
        },
        Row {
            case: "an alias over the raising field",
            query: "query { aliased: raised(message: \"aliased refusal\", kind: \"Aliased\") }"
                .to_string(),
            key: "aliased",
            value: Value::Null,
            message: "aliased refusal",
            error_type: Some("Aliased"),
        },
        Row {
            case: "a raise naming no class at all",
            query: "query { untyped(message: \"no class was named\") }".to_string(),
            key: "untyped",
            value: Value::Null,
            message: "no class was named",
            error_type: None,
        },
        Row {
            case: "an error APPENDED beside a value the field still answers",
            query: "query { appended(message: \"noted in passing\", kind: \"Noted\") }"
                .to_string(),
            key: "appended",
            value: json!(APPENDED_VALUE),
            message: "noted in passing",
            error_type: Some("Noted"),
        },
        Row {
            case: "a raise written as a mapping template",
            query: "query { templated(message: \"the template refuses\", kind: \"Templated\") }"
                .to_string(),
            key: "templated",
            value: Value::Null,
            message: "the template refuses",
            error_type: Some("Templated"),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answered) = post(
            case,
            &api,
            GRAPHQL_PATH,
            "POST",
            &json!({"query": row.query}),
            &[("x-api-key", api.api_key.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a GraphQL result is answered 200, got {status} and {answered}"
        );
        let errors = answered["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected exactly one error entry, got {answered}"
        );
        let entry = &errors[0];
        assert_eq!(
            (
                entry["message"].as_str(),
                entry["path"].as_array().cloned().unwrap_or_default(),
                &answered["data"][row.key],
            ),
            (
                Some(row.message),
                vec![json!(row.key)],
                &row.value,
            ),
            "case {case:?}: expected the message {:?} under the path [{:?}] beside the value \
             {}, got {answered}",
            row.message,
            row.key,
            row.value
        );
        assert_eq!(
            entry.get("errorType").and_then(Value::as_str),
            row.error_type,
            "case {case:?}: expected the errorType {:?}, got {answered}",
            row.error_type
        );
    }
}

/// The schema the series cases are driven over.
pub(crate) const SERVED_SCHEMA: &str = "type Query {\n  a: String\n  b: String\n  raises: String\n  \
     unreached: String\n}\n\
     \n\
     type Mutation {\n  touch: String\n}\n";

/// What the two resolving fields of [`SERVED_SCHEMA`] answer.
const SERVED_VALUE: &str = "served";

/// The definition every resolving field of the series cases carries.
pub(crate) fn answers() -> String {
    format!(
        "export function request(ctx) {{ return {{payload: \"{SERVED_VALUE}\"}}; }}\n\
         export function response(ctx) {{ return ctx.result; }}"
    )
}

/// The definition the raising field carries: a class of its own, so what it produces is one
/// GraphQL error of that field.
pub(crate) fn raises() -> String {
    "import { util } from '@aws-appsync/utils';\n\
     export function request(ctx) { util.error('this field refuses', 'Refused'); }\n\
     export function response(ctx) { return ctx.result; }"
        .to_string()
}

/// A key no api of this suite ever minted, in the shape AppSync's own keys take: what a
/// request presenting material that authorizes nothing carries.
const FORGED_KEY: &str = "da2-xxxxxxxxxxxxxxxxxxxxxxxxxx";

#[tokio::test]
async fn every_request_the_api_answered_is_counted_under_its_own_api_dimension() {
    let router = emulator();
    let driven = create_api(
        &router,
        "counted",
        None,
        SERVED_SCHEMA,
        &[(LOCAL, CALLED_TYPE, None)],
        &[Resolver::coded("a", LOCAL, answers())],
    )
    .await;
    let sibling = create_api(
        &router,
        "sibling",
        None,
        SERVED_SCHEMA,
        &[(LOCAL, CALLED_TYPE, None)],
        &[Resolver::coded("a", LOCAL, answers())],
    )
    .await;

    /// One call the api answers: how it is addressed, and the status that answer carries.
    struct Call<'a> {
        case: &'a str,
        path: &'a str,
        method: &'a str,
        body: Value,
        key: Option<&'a str>,
        status: u16,
    }
    let calls: &[Call] = &[
        Call {
            case: "a request the api served",
            path: GRAPHQL_PATH,
            method: "POST",
            body: json!({"query": "query { a }"}),
            key: Some("live"),
            status: 200,
        },
        Call {
            case: "a request presenting a key the api never minted",
            path: GRAPHQL_PATH,
            method: "POST",
            body: json!({"query": "query { a }"}),
            key: Some(FORGED_KEY),
            status: 401,
        },
        Call {
            case: "a request presenting no material at all",
            path: GRAPHQL_PATH,
            method: "POST",
            body: json!({"query": "query { a }"}),
            key: None,
            status: 401,
        },
        Call {
            case: "a request addressing a route the endpoint does not serve",
            path: "/no-such-route",
            method: "POST",
            body: json!({"query": "query { a }"}),
            key: Some("live"),
            status: 404,
        },
        Call {
            case: "a request addressing the endpoint with a method it does not serve",
            path: GRAPHQL_PATH,
            method: "GET",
            body: json!({}),
            key: Some("live"),
            status: 405,
        },
        Call {
            case: "a request whose body carries no document",
            path: GRAPHQL_PATH,
            method: "POST",
            body: json!({"not-a-query": 1}),
            key: Some("live"),
            status: 400,
        },
    ];

    for call in calls {
        let case = call.case;
        let key = call.key.map(|named| match named {
            "live" => driven.api_key.clone(),
            forged => forged.to_string(),
        });
        let fields: Vec<(&str, &str)> = key
            .as_deref()
            .map(|value| vec![("x-api-key", value)])
            .unwrap_or_default();
        let (status, answered) =
            post(case, &driven, call.path, call.method, &call.body, &fields).await;
        assert_eq!(
            status, call.status,
            "case {case:?}: expected {}, got {status} and {answered}",
            call.status
        );
    }

    // One request on the OTHER api, so each api's series is read while the other has traffic
    // of its own.
    let case = "the sibling api's own served request";
    let (status, answered) = post(
        case,
        &sibling,
        GRAPHQL_PATH,
        "POST",
        &json!({"query": "query { a }"}),
        &[("x-api-key", sibling.api_key.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} and {answered}"
    );

    let answered_calls = calls.len() as f64;
    let refused_calls = calls
        .iter()
        .filter(|call| (400..500).contains(&call.status))
        .count() as f64;
    for (case, api, requests, client_errors) in [
        ("the driven api", &driven, answered_calls, refused_calls),
        ("the sibling api", &sibling, 1.0, 0.0),
    ] {
        let api_only = &[(API_DIMENSION, api.api_id.as_str())];
        for (metric, expected) in [
            (REQUESTS, requests),
            (CLIENT_ERROR, client_errors),
            (SERVER_ERROR, 0.0),
        ] {
            let reported = metric_sum(&router, metric, api_only).await;
            assert_eq!(
                reported,
                Some(expected),
                "case {case:?}: expected the {metric} series to sum to {expected}, got \
                 {reported:?}"
            );
            for point in statistics(&router, metric, api_only).await {
                assert_eq!(
                    point["Unit"].as_str(),
                    Some(NO_UNIT),
                    "case {case:?}: the {metric} series carries no unit of its own, got {point}"
                );
            }
        }
        // The span is a measurement, not a count: what is graded is that one is reported per
        // request, in the unit AppSync reports a latency in — never how long the host took.
        let latency = statistics(&router, LATENCY, api_only).await;
        let samples: f64 = latency
            .iter()
            .filter_map(|point| point["SampleCount"].as_f64())
            .sum();
        assert_eq!(
            samples, requests,
            "case {case:?}: expected one Latency observation per request answered, got \
             {latency:?}"
        );
        for point in &latency {
            assert_eq!(
                point["Unit"].as_str(),
                Some(MILLISECONDS),
                "case {case:?}: a Latency datapoint is reported in {MILLISECONDS}, got {point}"
            );
            assert!(
                point["Minimum"].as_f64().is_some_and(|least| least >= 0.0),
                "case {case:?}: a Latency datapoint carries a measured span, got {point}"
            );
        }
    }
}

/// `enhancedMetricsConfig` asking for the per-resource opt-in, and the one that reports every
/// resolver and data source of the request whatever each says. Source:
/// `smithy_query(query="shape",shape="EnhancedMetricsConfig,ResolverLevelMetricsBehavior,\
/// DataSourceLevelMetricsBehavior,OperationLevelMetricsConfig")`.
pub(crate) fn per_resource_metrics() -> Value {
    json!({
        "resolverLevelMetricsBehavior": "PER_RESOLVER_METRICS",
        "dataSourceLevelMetricsBehavior": "PER_DATA_SOURCE_METRICS",
        "operationLevelMetricsConfig": "DISABLED",
    })
}

fn full_request_metrics() -> Value {
    json!({
        "resolverLevelMetricsBehavior": "FULL_REQUEST_RESOLVER_METRICS",
        "dataSourceLevelMetricsBehavior": "FULL_REQUEST_DATA_SOURCE_METRICS",
        "operationLevelMetricsConfig": "ENABLED",
    })
}

/// The two data sources the opt-in matrix is driven over: one that opted in and one that did
/// not. `NONE` is "There is no data source", so a step resolving against either IS an
/// invocation of it.
const OPTED_IN_SOURCE: &str = "reported";
const OPTED_OUT_SOURCE: &str = "unreported";

/// The type every data source of this suite but one is of, and the type of the one this
/// endpoint does not call at all — configured by no member of either kind, so it is created
/// with nothing beside its type (`crate::data_source_configuration_refusals` grades that
/// rule). A step resolving against it makes no invocation, so no data-source series of it
/// exists.
pub(crate) const CALLED_TYPE: &str = "NONE";
const UNCALLED_TYPE: &str = "AMAZON_BEDROCK_RUNTIME";
const UNCALLED_SOURCE: &str = "uncalled";

#[tokio::test]
async fn enhanced_series_are_published_only_where_the_api_asked_for_them() {
    let router = emulator();
    let per_resource = create_api(
        &router,
        "opted",
        Some(per_resource_metrics()),
        SERVED_SCHEMA,
        &[
            (OPTED_IN_SOURCE, CALLED_TYPE, Some("ENABLED")),
            (OPTED_OUT_SOURCE, CALLED_TYPE, Some("DISABLED")),
            (UNCALLED_SOURCE, UNCALLED_TYPE, Some("ENABLED")),
        ],
        &[
            Resolver::coded("a", OPTED_IN_SOURCE, answers()).reporting("ENABLED"),
            Resolver::coded("b", OPTED_OUT_SOURCE, answers()).reporting("DISABLED"),
            Resolver::coded("raises", OPTED_IN_SOURCE, raises()).reporting("ENABLED"),
            Resolver::coded("unreached", UNCALLED_SOURCE, answers()).reporting("ENABLED"),
        ],
    )
    .await;

    // Two requests through the opted-in resolver, one through the opted-out one, one through a
    // resolver that raises and one through a resolver whose data source this endpoint does not
    // call: the counts below are those, and nothing else.
    for (case, query) in [
        ("the first request through the opted-in resolver", "query { a }"),
        ("the second request through the opted-in resolver", "query { a }"),
        ("the request through the opted-out resolver", "query { b }"),
        ("the request through the raising resolver", "query { raises }"),
        (
            "the request whose data source is never called",
            "query { unreached }",
        ),
    ] {
        let (status, answered) = post(
            case,
            &per_resource,
            GRAPHQL_PATH,
            "POST",
            &json!({"query": query}),
            &[("x-api-key", per_resource.api_key.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} and {answered}"
        );
    }

    let api = per_resource.api_id.as_str();
    for (case, metric, dimension, value, expected) in [
        (
            "the opted-in resolver's invocations",
            REQUESTS,
            RESOLVER_DIMENSION,
            "Query.a",
            2.0,
        ),
        (
            "the opted-in resolver's GraphQL errors",
            GRAPHQL_ERROR,
            RESOLVER_DIMENSION,
            "Query.a",
            0.0,
        ),
        (
            "the raising resolver's invocations",
            REQUESTS,
            RESOLVER_DIMENSION,
            "Query.raises",
            1.0,
        ),
        (
            "the raising resolver's GraphQL errors",
            GRAPHQL_ERROR,
            RESOLVER_DIMENSION,
            "Query.raises",
            1.0,
        ),
        (
            // The raising resolver's request half never reached its data source, so only the
            // two resolutions that did are invocations of it.
            "the opted-in data source's invocations",
            REQUESTS,
            DATA_SOURCE_DIMENSION,
            OPTED_IN_SOURCE,
            2.0,
        ),
        (
            "the opted-in data source's errors",
            DATA_SOURCE_ERRORS,
            DATA_SOURCE_DIMENSION,
            OPTED_IN_SOURCE,
            0.0,
        ),
        (
            // Its own resolver DID run and did not resolve the field, so the resolver's series
            // reports the invocation and its error; the data source it names reports neither,
            // below.
            "the resolver whose data source is never called",
            REQUESTS,
            RESOLVER_DIMENSION,
            "Query.unreached",
            1.0,
        ),
        (
            "that resolver's GraphQL errors",
            GRAPHQL_ERROR,
            RESOLVER_DIMENSION,
            "Query.unreached",
            1.0,
        ),
    ] {
        let reported = metric_sum(
            &router,
            metric,
            &[(API_DIMENSION, api), (dimension, value)],
        )
        .await;
        assert_eq!(
            reported,
            Some(expected),
            "case {case:?}: expected the {metric} series of [{dimension}={value}] to sum to \
             {expected}, got {reported:?}"
        );
        for point in statistics(
            &router,
            metric,
            &[(API_DIMENSION, api), (dimension, value)],
        )
        .await
        {
            assert_eq!(
                point["Unit"].as_str(),
                Some(NO_UNIT),
                "case {case:?}: the {metric} series carries no unit of its own, got {point}"
            );
        }
    }
    // The span of a resolver invocation is a measurement too, reported in the unit a latency
    // carries.
    let latency = statistics(
        &router,
        LATENCY,
        &[(API_DIMENSION, api), (RESOLVER_DIMENSION, "Query.a")],
    )
    .await;
    let samples: f64 = latency
        .iter()
        .filter_map(|point| point["SampleCount"].as_f64())
        .sum();
    assert_eq!(
        samples, 2.0,
        "the opted-in resolver must report one Latency observation per invocation, got \
         {latency:?}"
    );
    for point in &latency {
        assert_eq!(
            point["Unit"].as_str(),
            Some(MILLISECONDS),
            "a resolver's Latency is reported in {MILLISECONDS}, got {point}"
        );
    }

    // A data source this endpoint never CALLED made no invocation, so it has no series at
    // all: reporting one would be an invocation nothing performed, and reporting it with no
    // error a successful one.
    for metric in [REQUESTS, LATENCY, DATA_SOURCE_ERRORS] {
        let reported = metric_sum(
            &router,
            metric,
            &[
                (API_DIMENSION, api),
                (DATA_SOURCE_DIMENSION, UNCALLED_SOURCE),
            ],
        )
        .await;
        assert_eq!(
            reported, None,
            "a data source this endpoint does not call reports no {metric}, got {reported:?}"
        );
    }

    // WHICH enhanced series exist at all is the opt-in matrix itself: the opted-out resolver
    // and the opted-out data source have none, the data source that was never called has
    // none even though it opted in, and no operation series exists under an api whose
    // `operationLevelMetricsConfig` is `DISABLED`.
    assert_eq!(
        enhanced_keys(&router, api).await,
        vec![
            (DATA_SOURCE_DIMENSION.to_string(), OPTED_IN_SOURCE.to_string()),
            (RESOLVER_DIMENSION.to_string(), "Query.a".to_string()),
            (RESOLVER_DIMENSION.to_string(), "Query.raises".to_string()),
            (RESOLVER_DIMENSION.to_string(), "Query.unreached".to_string()),
        ],
        "only the resolvers that ran and the data sources that were called, having opted in, \
         may carry an enhanced series"
    );

    // The FULL-REQUEST behavior overrides a resource's own opt-out, and the operation series
    // is published under the ROOT OPERATION TYPE the operation is of.
    let full_request = create_api(
        &router,
        "full",
        Some(full_request_metrics()),
        SERVED_SCHEMA,
        &[(OPTED_OUT_SOURCE, CALLED_TYPE, Some("DISABLED"))],
        &[
            Resolver::coded("a", OPTED_OUT_SOURCE, answers()).reporting("DISABLED"),
            Resolver::coded("touch", OPTED_OUT_SOURCE, answers())
                .on_type("Mutation")
                .reporting("DISABLED"),
        ],
    )
    .await;
    for (case, query) in [
        ("a query the document names", "query Alpha { a }"),
        ("a query the document names nothing", "query { a }"),
        ("a mutation", "mutation { touch }"),
    ] {
        let (status, answered) = post(
            case,
            &full_request,
            GRAPHQL_PATH,
            "POST",
            &json!({"query": query}),
            &[("x-api-key", full_request.api_key.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} and {answered}"
        );
    }
    let full = full_request.api_id.as_str();
    for (case, metric, dimension, value, expected) in [
        (
            "the opted-OUT resolver, reported anyway",
            REQUESTS,
            RESOLVER_DIMENSION,
            "Query.a",
            2.0,
        ),
        (
            "the opted-OUT data source, reported anyway",
            REQUESTS,
            DATA_SOURCE_DIMENSION,
            OPTED_OUT_SOURCE,
            3.0,
        ),
        (
            // Both queries are of one root operation type, whatever either document called
            // its operation; the mutation is of another.
            "the queries executed",
            REQUESTS,
            OPERATION_DIMENSION,
            "Query",
            2.0,
        ),
        (
            "the queries' GraphQL errors",
            GRAPHQL_ERROR,
            OPERATION_DIMENSION,
            "Query",
            0.0,
        ),
        (
            "the mutation executed",
            REQUESTS,
            OPERATION_DIMENSION,
            "Mutation",
            1.0,
        ),
        (
            "the mutation's resolver",
            REQUESTS,
            RESOLVER_DIMENSION,
            "Mutation.touch",
            1.0,
        ),
    ] {
        let reported = metric_sum(
            &router,
            metric,
            &[(API_DIMENSION, full), (dimension, value)],
        )
        .await;
        assert_eq!(
            reported,
            Some(expected),
            "case {case:?}: expected the {metric} series of [{dimension}={value}] to sum to \
             {expected}, got {reported:?}"
        );
    }
    // And the operation key is the root operation type and nothing else: the name one document
    // gave its operation is no key of its own.
    assert_eq!(
        enhanced_keys(&router, full).await,
        vec![
            (DATA_SOURCE_DIMENSION.to_string(), OPTED_OUT_SOURCE.to_string()),
            (OPERATION_DIMENSION.to_string(), "Mutation".to_string()),
            (OPERATION_DIMENSION.to_string(), "Query".to_string()),
            (RESOLVER_DIMENSION.to_string(), "Mutation.touch".to_string()),
            (RESOLVER_DIMENSION.to_string(), "Query.a".to_string()),
        ],
        "a full-request api reports every resolver and data source, and each operation under \
         the root operation type it is of"
    );

    // And an api that configured no enhanced metrics at all publishes no such series, while
    // still being counted at its own dimension.
    let plain = create_api(
        &router,
        "plain",
        None,
        SERVED_SCHEMA,
        &[(OPTED_IN_SOURCE, CALLED_TYPE, Some("ENABLED"))],
        &[Resolver::coded("a", OPTED_IN_SOURCE, answers()).reporting("ENABLED")],
    )
    .await;
    let case = "an api that asked for no enhanced metrics";
    let (status, answered) = post(
        case,
        &plain,
        GRAPHQL_PATH,
        "POST",
        &json!({"query": "query Alpha { a }"}),
        &[("x-api-key", plain.api_key.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} and {answered}"
    );
    assert_eq!(
        enhanced_keys(&router, &plain.api_id).await,
        Vec::new(),
        "case {case:?}: an api that asked for no granular data publishes no enhanced series"
    );
    assert_eq!(
        metric_sum(&router, REQUESTS, &[(API_DIMENSION, plain.api_id.as_str())]).await,
        Some(1.0),
        "case {case:?}: its own api-level series is published all the same"
    );
}

/// Every ENHANCED dimension KEY one api's `AWS/AppSync` series are published under, sorted:
/// the `(name, value)` pair each series carries beside the api's own dimension, read through
/// CloudWatch's own `ListMetrics`, with duplicates folded away.
pub(crate) async fn enhanced_keys(
    router: &MultiServiceRouter,
    api_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = cloudwatch(
        router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": API_DIMENSION, "Value": api_id}],
        }),
    )
    .await;
    assert_eq!(status, 200, "ListMetrics must answer 200, got {body}");
    let mut keys: Vec<(String, String)> = body["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|metric| {
            metric["Dimensions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|dimension| {
                    let name = dimension["Name"].as_str().unwrap_or_default().to_string();
                    let value = dimension["Value"].as_str().unwrap_or_default().to_string();
                    (name != API_DIMENSION).then_some((name, value))
                })
                .collect::<Vec<(String, String)>>()
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// POST (or otherwise address) one request to the address an api publishes for its GraphQL
/// endpoint, through the crate's one socket client for a published address.
pub(crate) async fn post(
    case: &str,
    api: &Api,
    path: &str,
    method: &str,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Value) {
    let (status, _, document) = try_post_answer(case, &api.graphql, path, method, body, fields)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
                api.graphql
            )
        });
    (status, document)
}

/// Create one GraphQL api with an API key, `schema`, the data sources `data_sources` names
/// (each with its own type and `metricsConfig`, or none) and one resolver per entry of
/// `resolvers`, and read the address its GraphQL endpoint is served on.
pub(crate) async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    enhanced_metrics: Option<Value>,
    schema: &str,
    data_sources: &[(&str, &str, Option<&str>)],
    resolvers: &[Resolver<'_>],
) -> Api {
    let mut request = json!({
        "name": format!("metrics-{name}"),
        "authenticationType": "API_KEY",
    });
    if let Some(config) = enhanced_metrics {
        request["enhancedMetricsConfig"] = config;
    }
    let (status, created) = appsync(router, "POST", "/v1/apis", Some(request)).await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    assert!(
        graphql.starts_with(&format!("{LOOPBACK_HOST}:")),
        "uris.GRAPHQL must be published on an address this emulator serves, got {graphql:?}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    install_schema(router, &api_id, schema).await;
    for (data_source, data_source_type, metrics_config) in data_sources {
        let mut body = json!({"name": data_source, "type": data_source_type});
        // A data source of a type AppSync reaches by assuming a role is created with one,
        // through the crate's one reading of which types those are
        // ([`crate::data_source_service_role`]).
        if needs_service_role(data_source_type) {
            body["serviceRoleArn"] = json!(ROLE_ARN);
        }
        if let Some(config) = metrics_config {
            body["metricsConfig"] = json!(config);
        }
        let (status, answered) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateDataSource for {data_source} must answer 200, got {answered}"
        );
    }
    for resolver in resolvers {
        let mut body = json!({
            "fieldName": resolver.field,
            "kind": "UNIT",
            "dataSourceName": resolver.data_source,
        });
        if let Some(code) = &resolver.code {
            body["code"] = json!(code);
            body["runtime"] = json!({"name": RUNTIME.0, "runtimeVersion": RUNTIME.1});
        }
        if let Some((request, response)) = resolver.templates {
            body["requestMappingTemplate"] = json!(request);
            body["responseMappingTemplate"] = json!(response);
        }
        if let Some(config) = resolver.metrics_config {
            body["metricsConfig"] = json!(config);
        }
        let (status, answered) = appsync(
            router,
            "POST",
            &format!(
                "/v1/apis/{api_id}/types/{}/resolvers",
                resolver.type_name
            ),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {} must answer 200, got {answered}",
            resolver.field
        );
    }
    Api {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}
