// SPDX-License-Identifier: CC0-1.0

//! WHEN a GraphQL api's enhanced `AWS/AppSync` series are decided, and what UNIT they are
//! stored with.
//!
//! The rules these cases assert, one level above the recorded case (one `API_KEY` api whose
//! `enhancedMetricsConfig` is switched to `FULL_REQUEST_RESOLVER_METRICS` half-way through its
//! traffic, read back at `Period=60` under `Unit="None"` and under `Unit="Count"`):
//!
//! > 1. Which of an api's enhanced series a request publishes is read from the api's own
//! >    `enhancedMetricsConfig` and from each resolver's and data source's own `metricsConfig`
//! >    AS THE CONTROL PLANE HOLDS THEM WHEN THAT REQUEST ARRIVES — so a `Update*` call that
//! >    changes one of them changes what later requests publish, for that category alone: a
//! >    `FULL_REQUEST_…_METRICS` behavior starts reporting the resources that opted out, a
//! >    `PER_…_METRICS` behavior reports exactly those whose own `metricsConfig` is `ENABLED`
//! >    at that moment, `operationLevelMetricsConfig` turns the operation series on and off
//! >    outright, and the datapoints already published stay as they were.
//! > 2. Every enhanced datapoint is stored with no unit of its own, the span a `Latency`
//! >    series reports being the one measurement that has one — so a `GetMetricStatistics`
//! >    naming a unit returns "only data that was collected with that unit specified"
//! >    (`GetMetricStatistics$Unit`) and a read naming any other unit returns no datapoint of
//! >    that series at all.
//!
//! The rows span what each rule generalises over: each of the three members of
//! `EnhancedMetricsConfig` switched in turn and switched back, the opt-in moved on the
//! RESOURCE side (`UpdateResolver` / `UpdateDataSource`) as well as on the api's side, in both
//! directions; and every enhanced series name (`Requests`, `GraphQLError`, `Errors`,
//! `Latency`) read with no unit named, with the unit it was stored under and with two units it
//! was not.
//!
//! Nothing here is asserted off the store: every request is POSTed over a socket to the
//! address the api published for it, every opt-in is moved through the control plane's own
//! `Update*` call, and every number is read back through CloudWatch's own `ListMetrics` and
//! `GetMetricStatistics`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::{appsync, emulator, metric_sum, statistics_in_unit};
use crate::graphql_api_request_metrics::{
    answers, create_api, enhanced_keys, per_resource_metrics, post, raises, Api, Resolver,
    API_DIMENSION, CALLED_TYPE, DATA_SOURCE_DIMENSION, DATA_SOURCE_ERRORS, GRAPHQL_ERROR,
    GRAPHQL_PATH, LATENCY, MILLISECONDS, NO_UNIT, OPERATION_DIMENSION, REQUESTS,
    RESOLVER_DIMENSION, SERVED_SCHEMA,
};

/// The two data sources of these cases: one that opted into the enhanced data-source series
/// and one that did not.
const OPTED_IN_SOURCE: &str = "reported_source";
const OPTED_OUT_SOURCE: &str = "unreported_source";

/// The values a resource's own `metricsConfig` takes. Source:
/// `smithy_query(query="shape",shape="ResolverLevelMetricsConfig,DataSourceLevelMetricsConfig")`.
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// One `enhancedMetricsConfig`, by the three members it requires. Source:
/// `smithy_query(query="shape",shape="EnhancedMetricsConfig,ResolverLevelMetricsBehavior,\
/// DataSourceLevelMetricsBehavior,OperationLevelMetricsConfig")`.
fn enhanced(resolvers: &str, data_sources: &str, operations: &str) -> Value {
    json!({
        "resolverLevelMetricsBehavior": resolvers,
        "dataSourceLevelMetricsBehavior": data_sources,
        "operationLevelMetricsConfig": operations,
    })
}

/// The two readings of `resolverLevelMetricsBehavior` and of
/// `dataSourceLevelMetricsBehavior`.
const PER_RESOLVER: &str = "PER_RESOLVER_METRICS";
const FULL_REQUEST_RESOLVERS: &str = "FULL_REQUEST_RESOLVER_METRICS";
const PER_DATA_SOURCE: &str = "PER_DATA_SOURCE_METRICS";
const FULL_REQUEST_DATA_SOURCES: &str = "FULL_REQUEST_DATA_SOURCE_METRICS";

/// One stage of the matrix: the configuration it moves, the requests it then drives, and what
/// every enhanced series of the api sums to once it has.
struct Stage<'a> {
    case: &'a str,
    /// The `enhancedMetricsConfig` this stage installs through `UpdateGraphqlApi`, or
    /// `Value::Null` to leave the api as it stands.
    api_config: Value,
    /// The resolvers whose own `metricsConfig` this stage moves, by field and new value.
    resolver_config: &'a [(&'a str, &'a str)],
    /// The data sources whose own `metricsConfig` this stage moves, by name and new value.
    data_source_config: &'a [(&'a str, &'a str)],
    /// The query-root fields this stage drives one request each through, in order.
    driven: &'a [&'a str],
    /// What the `Requests` series of each enhanced key must sum to afterwards — `None` for a
    /// key no datapoint exists under at all.
    expected: &'a [(&'a str, &'a str, Option<f64>)],
    /// Every enhanced dimension key the api's series are catalogued under afterwards.
    catalogued: &'a [(&'a str, &'a str)],
}

#[tokio::test]
async fn moving_an_opt_in_changes_what_later_requests_publish() {
    let router = emulator();
    let api = create_api(
        &router,
        "switches",
        Some(per_resource_metrics()),
        SERVED_SCHEMA,
        &[
            (OPTED_IN_SOURCE, CALLED_TYPE, Some(ENABLED)),
            (OPTED_OUT_SOURCE, CALLED_TYPE, Some(DISABLED)),
        ],
        &[
            Resolver::coded("a", OPTED_IN_SOURCE, answers()).reporting(ENABLED),
            Resolver::coded("b", OPTED_OUT_SOURCE, answers()).reporting(DISABLED),
        ],
    )
    .await;

    let stages: &[Stage] = &[
        Stage {
            case: "the per-resource opt-in the api was created with",
            api_config: Value::Null,
            resolver_config: &[],
            data_source_config: &[],
            driven: &["a", "a", "a", "b", "b"],
            expected: &[
                (RESOLVER_DIMENSION, "Query.a", Some(3.0)),
                (RESOLVER_DIMENSION, "Query.b", None),
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE, Some(3.0)),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE, None),
                (OPERATION_DIMENSION, "Query", None),
            ],
            catalogued: &[
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE),
                (RESOLVER_DIMENSION, "Query.a"),
            ],
        },
        Stage {
            case: "the resolver behavior switched to the full-request one",
            api_config: enhanced(FULL_REQUEST_RESOLVERS, PER_DATA_SOURCE, DISABLED),
            resolver_config: &[],
            data_source_config: &[],
            driven: &["b", "b"],
            expected: &[
                (RESOLVER_DIMENSION, "Query.a", Some(3.0)),
                (RESOLVER_DIMENSION, "Query.b", Some(2.0)),
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE, Some(3.0)),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE, None),
                (OPERATION_DIMENSION, "Query", None),
            ],
            catalogued: &[
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE),
                (RESOLVER_DIMENSION, "Query.a"),
                (RESOLVER_DIMENSION, "Query.b"),
            ],
        },
        Stage {
            case: "the data source behavior switched to the full-request one as well",
            api_config: enhanced(FULL_REQUEST_RESOLVERS, FULL_REQUEST_DATA_SOURCES, DISABLED),
            resolver_config: &[],
            data_source_config: &[],
            driven: &["a", "b"],
            expected: &[
                (RESOLVER_DIMENSION, "Query.a", Some(4.0)),
                (RESOLVER_DIMENSION, "Query.b", Some(3.0)),
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE, Some(4.0)),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE, Some(1.0)),
                (OPERATION_DIMENSION, "Query", None),
            ],
            catalogued: &[
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE),
                (RESOLVER_DIMENSION, "Query.a"),
                (RESOLVER_DIMENSION, "Query.b"),
            ],
        },
        Stage {
            case: "the operation series turned on",
            api_config: enhanced(FULL_REQUEST_RESOLVERS, FULL_REQUEST_DATA_SOURCES, ENABLED),
            resolver_config: &[],
            data_source_config: &[],
            driven: &["a"],
            expected: &[
                (RESOLVER_DIMENSION, "Query.a", Some(5.0)),
                (RESOLVER_DIMENSION, "Query.b", Some(3.0)),
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE, Some(5.0)),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE, Some(1.0)),
                (OPERATION_DIMENSION, "Query", Some(1.0)),
            ],
            catalogued: &[
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE),
                (OPERATION_DIMENSION, "Query"),
                (RESOLVER_DIMENSION, "Query.a"),
                (RESOLVER_DIMENSION, "Query.b"),
            ],
        },
        Stage {
            case: "back to the per-resource opt-in, with the resources opting IN themselves",
            api_config: enhanced(PER_RESOLVER, PER_DATA_SOURCE, DISABLED),
            resolver_config: &[("b", ENABLED)],
            data_source_config: &[(OPTED_OUT_SOURCE, ENABLED)],
            driven: &["b"],
            expected: &[
                (RESOLVER_DIMENSION, "Query.a", Some(5.0)),
                (RESOLVER_DIMENSION, "Query.b", Some(4.0)),
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE, Some(5.0)),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE, Some(2.0)),
                // The operation series was turned off again, so this request added nothing to
                // what the stage above published under it.
                (OPERATION_DIMENSION, "Query", Some(1.0)),
            ],
            catalogued: &[
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE),
                (OPERATION_DIMENSION, "Query"),
                (RESOLVER_DIMENSION, "Query.a"),
                (RESOLVER_DIMENSION, "Query.b"),
            ],
        },
        Stage {
            case: "the same resources opting back OUT",
            api_config: Value::Null,
            resolver_config: &[("b", DISABLED)],
            data_source_config: &[(OPTED_OUT_SOURCE, DISABLED)],
            driven: &["b", "b"],
            expected: &[
                (RESOLVER_DIMENSION, "Query.a", Some(5.0)),
                (RESOLVER_DIMENSION, "Query.b", Some(4.0)),
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE, Some(5.0)),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE, Some(2.0)),
                (OPERATION_DIMENSION, "Query", Some(1.0)),
            ],
            catalogued: &[
                (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE),
                (DATA_SOURCE_DIMENSION, OPTED_OUT_SOURCE),
                (OPERATION_DIMENSION, "Query"),
                (RESOLVER_DIMENSION, "Query.a"),
                (RESOLVER_DIMENSION, "Query.b"),
            ],
        },
    ];

    for stage in stages {
        let case = stage.case;
        if !stage.api_config.is_null() {
            install_api_config(&router, &api, case, stage.api_config.clone()).await;
        }
        for (field, config) in stage.resolver_config {
            install_resolver_config(&router, &api, case, field, config).await;
        }
        for (name, config) in stage.data_source_config {
            install_data_source_config(&router, &api, case, name, config).await;
        }
        for field in stage.driven {
            drive(&api, case, field).await;
        }
        for (dimension, value, expected) in stage.expected {
            let reported = metric_sum(
                &router,
                REQUESTS,
                &[(API_DIMENSION, api.api_id.as_str()), (dimension, value)],
            )
            .await;
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected the {REQUESTS} series of [{dimension}={value}] to sum \
                 to {expected:?}, got {reported:?}"
            );
        }
        let catalogued: Vec<(String, String)> = stage
            .catalogued
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        let reported = enhanced_keys(&router, &api.api_id).await;
        assert_eq!(
            reported, catalogued,
            "case {case:?}: expected the enhanced keys {catalogued:?}, got {reported:?}"
        );
    }
}

/// One row of the unit matrix: the series read, the key it is published under, the unit the
/// read names (or none at all), and how many observations that read must see.
struct UnitRow<'a> {
    case: &'a str,
    metric: &'a str,
    dimension: (&'a str, &'a str),
    /// The `Unit` the read names, or `None` for a read that names none and so sees "all data
    /// that was collected with any unit" (`GetMetricStatistics$Unit`).
    unit: Option<&'a str>,
    /// The observations that read must report, by their `SampleCount` — `None` for a read
    /// that must see no datapoint at all.
    samples: Option<f64>,
}

/// The unit a count would naturally carry, which an enhanced series is nevertheless not
/// stored under. Source: `StandardUnit`'s own values
/// (`smithy_query(query="shape",shape="StandardUnit")` on the CloudWatch model).
const COUNT: &str = "Count";

#[tokio::test]
async fn an_enhanced_series_is_seen_only_under_the_unit_it_was_stored_with() {
    let router = emulator();
    let api = create_api(
        &router,
        "units",
        Some(enhanced(PER_RESOLVER, PER_DATA_SOURCE, ENABLED)),
        SERVED_SCHEMA,
        &[(OPTED_IN_SOURCE, CALLED_TYPE, Some(ENABLED))],
        &[
            Resolver::coded("a", OPTED_IN_SOURCE, answers()).reporting(ENABLED),
            Resolver::coded("raises", OPTED_IN_SOURCE, raises()).reporting(ENABLED),
        ],
    )
    .await;
    for field in ["a", "a", "raises"] {
        drive(&api, "the traffic the units are read over", field).await;
    }

    let id = api.api_id.clone();
    let resolver = (RESOLVER_DIMENSION, "Query.a");
    let raising = (RESOLVER_DIMENSION, "Query.raises");
    let source = (DATA_SOURCE_DIMENSION, OPTED_IN_SOURCE);
    let operation = (OPERATION_DIMENSION, "Query");
    let rows: &[UnitRow] = &[
        UnitRow {
            case: "a resolver's invocations, no unit named",
            metric: REQUESTS,
            dimension: resolver,
            unit: None,
            samples: Some(2.0),
        },
        UnitRow {
            case: "a resolver's invocations under the unit they were stored with",
            metric: REQUESTS,
            dimension: resolver,
            unit: Some(NO_UNIT),
            samples: Some(2.0),
        },
        UnitRow {
            case: "a resolver's invocations read as a count",
            metric: REQUESTS,
            dimension: resolver,
            unit: Some(COUNT),
            samples: None,
        },
        UnitRow {
            case: "a resolver's invocations read as a span",
            metric: REQUESTS,
            dimension: resolver,
            unit: Some(MILLISECONDS),
            samples: None,
        },
        UnitRow {
            case: "a resolver's GraphQL errors under the unit they were stored with",
            metric: GRAPHQL_ERROR,
            dimension: raising,
            unit: Some(NO_UNIT),
            samples: Some(1.0),
        },
        UnitRow {
            case: "a resolver's GraphQL errors read as a count",
            metric: GRAPHQL_ERROR,
            dimension: raising,
            unit: Some(COUNT),
            samples: None,
        },
        UnitRow {
            case: "a data source's errors under the unit they were stored with",
            metric: DATA_SOURCE_ERRORS,
            dimension: source,
            unit: Some(NO_UNIT),
            samples: Some(2.0),
        },
        UnitRow {
            case: "a data source's errors read as a count",
            metric: DATA_SOURCE_ERRORS,
            dimension: source,
            unit: Some(COUNT),
            samples: None,
        },
        UnitRow {
            case: "an operation's executions under the unit they were stored with",
            metric: REQUESTS,
            dimension: operation,
            unit: Some(NO_UNIT),
            samples: Some(3.0),
        },
        UnitRow {
            case: "an operation's executions read as a count",
            metric: REQUESTS,
            dimension: operation,
            unit: Some(COUNT),
            samples: None,
        },
        // The span a Latency series reports is the one measurement with a unit of its own, so
        // it is seen under THAT unit and under no other.
        UnitRow {
            case: "a resolver's spans, no unit named",
            metric: LATENCY,
            dimension: resolver,
            unit: None,
            samples: Some(2.0),
        },
        UnitRow {
            case: "a resolver's spans under the unit they were stored with",
            metric: LATENCY,
            dimension: resolver,
            unit: Some(MILLISECONDS),
            samples: Some(2.0),
        },
        UnitRow {
            case: "a resolver's spans read with no unit of their own",
            metric: LATENCY,
            dimension: resolver,
            unit: Some(NO_UNIT),
            samples: None,
        },
        UnitRow {
            case: "a resolver's spans read as a count",
            metric: LATENCY,
            dimension: resolver,
            unit: Some(COUNT),
            samples: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let points = statistics_in_unit(
            &router,
            row.metric,
            &[(API_DIMENSION, id.as_str()), row.dimension],
            row.unit,
        )
        .await;
        let observed: f64 = points
            .iter()
            .filter_map(|point| point["SampleCount"].as_f64())
            .sum();
        let seen = (!points.is_empty()).then_some(observed);
        assert_eq!(
            seen, row.samples,
            "case {case:?}: expected the {} series of [{}={}] read under {:?} to report {:?} \
             observations, got {seen:?} in {points:?}",
            row.metric, row.dimension.0, row.dimension.1, row.unit, row.samples
        );
        for point in &points {
            if let Some(unit) = row.unit {
                assert_eq!(
                    point["Unit"].as_str(),
                    Some(unit),
                    "case {case:?}: a datapoint of a read naming {unit:?} carries that unit, \
                     got {point}"
                );
            }
        }
    }
}

/// Install `config` as the api's `enhancedMetricsConfig` through `UpdateGraphqlApi`, leaving
/// the rest of the api as the control plane holds it: the two members the call requires beside
/// it are read back off the api itself rather than restated here.
async fn install_api_config(
    router: &MultiServiceRouter,
    api: &Api,
    case: &str,
    config: Value,
) {
    let path = format!("/v1/apis/{}", api.api_id);
    let (status, read) = appsync(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetGraphqlApi must answer 200, got {read}"
    );
    let held = &read["graphqlApi"];
    let request = json!({
        "name": held["name"],
        "authenticationType": held["authenticationType"],
        "enhancedMetricsConfig": config,
    });
    let (status, updated) = appsync(router, "POST", &path, Some(request)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateGraphqlApi must answer 200, got {updated}"
    );
    assert_eq!(
        updated["graphqlApi"]["enhancedMetricsConfig"], config,
        "case {case:?}: expected the installed enhancedMetricsConfig {config}, got {updated}"
    );
}

/// Move one resolver's own `metricsConfig` to `config` through `UpdateResolver`, restating the
/// definition the control plane already holds for it.
async fn install_resolver_config(
    router: &MultiServiceRouter,
    api: &Api,
    case: &str,
    field: &str,
    config: &str,
) {
    let path = format!("/v1/apis/{}/types/Query/resolvers/{field}", api.api_id);
    let (status, read) = appsync(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetResolver must answer 200, got {read}"
    );
    let held = &read["resolver"];
    let mut request = json!({
        "dataSourceName": held["dataSourceName"],
        "kind": held["kind"],
        "metricsConfig": config,
    });
    if held["code"].is_string() {
        request["code"] = held["code"].clone();
        request["runtime"] = held["runtime"].clone();
    }
    let (status, updated) = appsync(router, "POST", &path, Some(request)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateResolver must answer 200, got {updated}"
    );
    assert_eq!(
        updated["resolver"]["metricsConfig"].as_str(),
        Some(config),
        "case {case:?}: expected the resolver's metricsConfig to be {config}, got {updated}"
    );
}

/// Move one data source's own `metricsConfig` to `config` through `UpdateDataSource`.
async fn install_data_source_config(
    router: &MultiServiceRouter,
    api: &Api,
    case: &str,
    name: &str,
    config: &str,
) {
    let path = format!("/v1/apis/{}/datasources/{name}", api.api_id);
    let (status, read) = appsync(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDataSource must answer 200, got {read}"
    );
    let held = &read["dataSource"];
    let request = json!({
        "type": held["type"],
        "metricsConfig": config,
    });
    let (status, updated) = appsync(router, "POST", &path, Some(request)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateDataSource must answer 200, got {updated}"
    );
    assert_eq!(
        updated["dataSource"]["metricsConfig"].as_str(),
        Some(config),
        "case {case:?}: expected the data source's metricsConfig to be {config}, got {updated}"
    );
}

/// POST one request selecting `field` of the query root to the address the api published, and
/// require the api to have ANSWERED it: what is driven is traffic, so a stage's counts are the
/// requests it made and nothing else.
async fn drive(api: &Api, case: &str, field: &str) {
    let (status, answered) = post(
        case,
        api,
        GRAPHQL_PATH,
        "POST",
        &json!({"query": format!("query {{ {field} }}")}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a request through {field} is answered 200, got {status} and {answered}"
    );
}


