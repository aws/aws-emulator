// SPDX-License-Identifier: CC0-1.0

//! The `AWS/AppSync` series a GraphQL API's REAL-TIME plane publishes over a subscriber's
//! whole lifecycle, driven over both of the API's own published sockets.
//!
//! The rule these rows assert, one level above the recorded case (one connect, one filtered
//! subscribe, three matching mutations and one the filter excluded, a stop and a close):
//!
//! > Every step of a GraphQL API's real-time lifecycle is reported in CloudWatch's
//! > `AWS/AppSync` namespace under exactly the single dimension `GraphQLAPIId` of that API:
//! > one `ConnectSuccess` per connection the endpoint established, one `SubscribeSuccess`
//! > per subscription started on one, one `PublishDataMessageSuccess` per `data` frame
//! > DELIVERED to a subscriber — so a publication a subscription's installed filter excluded
//! > counts for nothing, and one that reached two subscribers counts twice — one
//! > `UnsubscribeSuccess` per subscription its subscriber stopped, and one
//! > `DisconnectSuccess` per connection closed. The connections open on the API and the
//! > subscriptions they hold are reported as the `ActiveConnections` and
//! > `ActiveSubscriptions` GAUGES, observed for as long as the API holds a connection, so a
//! > subscription that ended reads zero subscriptions while the connection that held it is
//! > still open.
//!
//! The rows span what the rule generalises over: how many connections the API holds, how
//! many subscriptions each of them holds, how many publications the subscriptions' installed
//! filter accepts and how many it excludes, and how many subscriptions the subscriber stops
//! — including a connection that subscribes to nothing at all.
//!
//! Nothing here is asserted off the store: every frame is read off a real WebSocket over the
//! address the API published for it, every mutation is POSTed to the address it published for
//! that, and every number is read back through CloudWatch's own `ListMetrics` and
//! `GetMetricStatistics`.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::{appsync, cloudwatch, emulator};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_realtime_endpoint::{
    connected_with_key, exchange, next_frame, started_message, ClientSocket,
};

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The `runtime` every resolver of this suite is defined under. Source: `AppSyncRuntime`'s
/// own values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The name of the local data source every resolver of this suite resolves against: `NONE`
/// is "There is no data source", so the request function's own payload is the result.
const DATA_SOURCE: &str = "local";

/// The dimension every series of a GraphQL API is published under; the service's own copy is
/// `service::graphql_api::metrics::GRAPHQL_API_ID_DIMENSION`.
const API_DIMENSION: &str = "GraphQLAPIId";

/// The namespace AppSync publishes its own series to.
const NAMESPACE: &str = "AWS/AppSync";

/// The channel the subscriptions' installed filter accepts, and one it excludes.
const ACCEPTED_CHANNEL: &str = "alpha";
const EXCLUDED_CHANNEL: &str = "beta";

/// The subscription field every subscriber of this suite starts on.
const SUBSCRIPTION_FIELD: &str = "onAddPost";

/// How long a case waits for a lifecycle step to be reported, and how often it looks again:
/// generous for a loaded machine, and bounded so a case that will never be answered fails
/// instead of hanging.
const REPORT_DEADLINE: Duration = Duration::from_secs(90);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// How many further gauge readings a case waits for before grading them. Two, so what is
/// graded is a CADENCE and not one trailing datapoint.
const READINGS_AWAITED: f64 = 2.0;

/// The schema every API of this suite is created with: a post carrying the channel its
/// subscriptions are filtered on, the mutation that publishes one, and the subscription
/// field that mutation is linked to.
const SCHEMA: &str = "type Post {\n  id: ID!\n  channel: String\n  body: String\n}\n\
     \n\
     type Query {\n  post(id: ID!): Post\n}\n\
     \n\
     type Mutation {\n  addPost(id: ID!, channel: String!, body: String): Post\n}\n\
     \n\
     type Subscription {\n  \
     onAddPost: Post @aws_subscribe(mutations: [\"addPost\"])\n}\n";

/// The mutation resolver: the post it publishes is what its arguments carry.
const ECHOED_POST: &str = "export function request(ctx) {\n  \
     return {payload: {id: ctx.args.id, channel: ctx.args.channel, body: ctx.args.body}};\n}\n\
     export function response(ctx) { return ctx.result; }";

/// The subscription field's subscribe-time handler, installing the filter that accepts
/// [`ACCEPTED_CHANNEL`] alone — so a publication on any other channel reaches no subscriber.
const FILTERS_ON_THE_ACCEPTED_CHANNEL: &str =
    "import { extensions } from '@aws-appsync/utils';\n\
     export function request(ctx) {\n  \
     extensions.setSubscriptionFilter({filterGroup: [{filters: [\n    \
     {fieldName: 'channel', operator: 'eq', value: 'alpha'},\n  ]}]});\n  \
     return {payload: null};\n}\n\
     export function response(ctx) { return null; }";

/// One created GraphQL API: its id, the addresses both its endpoints are served on, and a
/// live key.
struct GraphqlApi {
    api_id: String,
    graphql: String,
    realtime: String,
    api_key: String,
}

/// One row of the table: a real-time lifecycle, and the sums the API's series must then
/// carry.
struct Row {
    case: &'static str,
    /// How many connections the row opens on the API.
    connections: usize,
    /// How many subscriptions each of those connections starts.
    subscriptions_each: usize,
    /// The channel of each mutation the row serves, in order: [`ACCEPTED_CHANNEL`] is the
    /// one every subscription's filter accepts.
    mutations: &'static [&'static str],
    /// How many of each connection's subscriptions the row then stops.
    stopped_each: usize,
    /// How many `data` frames each connection must read: one per subscription it holds, per
    /// publication the filter accepted.
    frames_each: usize,
    /// What each series must sum to afterwards, `None` for a series that must hold no
    /// datapoint at all.
    connect_success: Option<f64>,
    subscribe_success: Option<f64>,
    data_messages: Option<f64>,
    unsubscribe_success: Option<f64>,
}

#[tokio::test]
async fn every_step_of_a_realtime_lifecycle_is_counted_under_the_apis_own_dimension() {
    let rows: &[Row] = &[
        Row {
            case: "the recorded lifecycle: one subscriber, three accepted and one excluded",
            connections: 1,
            subscriptions_each: 1,
            mutations: &[
                ACCEPTED_CHANNEL,
                ACCEPTED_CHANNEL,
                EXCLUDED_CHANNEL,
                ACCEPTED_CHANNEL,
            ],
            stopped_each: 1,
            frames_each: 3,
            connect_success: Some(1.0),
            subscribe_success: Some(1.0),
            // The excluded publication reached nobody, so nothing of it is counted.
            data_messages: Some(3.0),
            unsubscribe_success: Some(1.0),
        },
        Row {
            case: "two subscriptions on one connection: one publication counts twice",
            connections: 1,
            subscriptions_each: 2,
            mutations: &[ACCEPTED_CHANNEL],
            stopped_each: 2,
            frames_each: 2,
            connect_success: Some(1.0),
            subscribe_success: Some(2.0),
            data_messages: Some(2.0),
            unsubscribe_success: Some(2.0),
        },
        Row {
            case: "two connections, one subscription each, two publications",
            connections: 2,
            subscriptions_each: 1,
            mutations: &[ACCEPTED_CHANNEL, ACCEPTED_CHANNEL],
            stopped_each: 1,
            frames_each: 2,
            connect_success: Some(2.0),
            subscribe_success: Some(2.0),
            data_messages: Some(4.0),
            unsubscribe_success: Some(2.0),
        },
        Row {
            case: "a subscription the filter excluded every publication from",
            connections: 1,
            subscriptions_each: 1,
            mutations: &[EXCLUDED_CHANNEL, EXCLUDED_CHANNEL],
            stopped_each: 0,
            frames_each: 0,
            connect_success: Some(1.0),
            subscribe_success: Some(1.0),
            // Measured and none: the mutations were served, and delivered nothing.
            data_messages: Some(0.0),
            // Nothing was stopped, so nothing of that series is reported at all.
            unsubscribe_success: None,
        },
        Row {
            case: "a connection that subscribes to nothing at all",
            connections: 1,
            subscriptions_each: 0,
            mutations: &[ACCEPTED_CHANNEL],
            stopped_each: 0,
            frames_each: 0,
            connect_success: Some(1.0),
            subscribe_success: None,
            data_messages: Some(0.0),
            unsubscribe_success: None,
        },
    ];

    let router = emulator();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let api = create_api(&router, &format!("lifecycle-{index}")).await;
        let mut sockets = Vec::new();
        for connection in 0..row.connections {
            let mut socket = connected_with_key(case, &api.realtime, &api.api_key).await;
            for subscription in 0..row.subscriptions_each {
                started(case, &mut socket, &subscription_id(connection, subscription)).await;
            }
            sockets.push(socket);
        }

        for (served, channel) in row.mutations.iter().enumerate() {
            let answered = mutate(case, &api, &format!("P-{served}"), channel).await;
            assert_eq!(
                answered.pointer("/data/addPost/channel").and_then(Value::as_str),
                Some(*channel),
                "case {case:?}: the mutation must be served by the API's own resolver, got \
                 {answered}"
            );
        }

        // What each connection READ off its socket, which is what the delivery series
        // counts: a frame queued for a subscriber is a frame written to it.
        for (connection, socket) in sockets.iter_mut().enumerate() {
            for frame in 0..row.frames_each {
                let read = next_frame(socket).await.unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: connection {connection} must receive data frame {frame}"
                    )
                });
                assert_eq!(
                    read["type"].as_str(),
                    Some("data"),
                    "case {case:?}: connection {connection} must receive a data frame, got \
                     {read}"
                );
            }
        }

        for (connection, socket) in sockets.iter_mut().enumerate() {
            for subscription in 0..row.stopped_each {
                let id = subscription_id(connection, subscription);
                let completed =
                    exchange(case, socket, &json!({"id": id, "type": "stop"})).await;
                assert_eq!(
                    completed.as_ref().and_then(|frame| frame["type"].as_str()),
                    Some("complete"),
                    "case {case:?}: the stop of {id} must be answered complete, got \
                     {completed:?}"
                );
            }
        }

        for (metric, expected) in [
            ("ConnectSuccess", row.connect_success),
            ("SubscribeSuccess", row.subscribe_success),
            ("PublishDataMessageSuccess", row.data_messages),
            ("UnsubscribeSuccess", row.unsubscribe_success),
        ] {
            let summed = metric_sum(&router, metric, &api.api_id).await;
            assert_eq!(
                summed, expected,
                "case {case:?}: expected {metric} to sum to {expected:?} under \
                 [{API_DIMENSION}], got {summed:?}"
            );
        }

        // Closing a connection is what owes a `DisconnectSuccess`: it is published when the
        // connection ends, not when it was opened.
        for socket in &mut sockets {
            let _ = socket.close(None).await;
        }
        drop(sockets);
        let closed = row.connections as f64;
        let disconnected =
            awaited_sum(&router, case, "DisconnectSuccess", &api.api_id, closed).await;
        assert_eq!(
            disconnected,
            Some(closed),
            "case {case:?}: expected {closed} DisconnectSuccess under [{API_DIMENSION}] once \
             every connection closed, got {disconnected:?}"
        );
    }
}

#[tokio::test]
async fn the_two_gauges_read_what_the_plane_holds_while_the_connection_stays_open() {
    let router = emulator();
    let api = create_api(&router, "gauges").await;
    let case = "a subscription stopped on a connection held open";
    let mut socket = connected_with_key(case, &api.realtime, &api.api_key).await;
    started(case, &mut socket, "watch-1").await;

    // While subscribed, the plane holds one connection and one subscription, and both gauges
    // report it.
    for (metric, expected) in [("ActiveConnections", 1.0), ("ActiveSubscriptions", 1.0)] {
        let reading = awaited_maximum(&router, case, metric, &api.api_id, expected).await;
        assert!(
            reading.is_some_and(|read| read >= expected),
            "case {case:?}: expected a {metric} reading of at least {expected} while \
             subscribed, got {reading:?}"
        );
    }

    // The stop ends the subscription and leaves the connection open, which is what the two
    // gauges have to tell apart from then on.
    let completed = exchange(case, &mut socket, &json!({"id": "watch-1", "type": "stop"})).await;
    assert_eq!(
        completed.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("complete"),
        "case {case:?}: the stop must be answered complete, got {completed:?}"
    );
    let subscriptions_before = observations(&router, "ActiveSubscriptions", &api.api_id).await;
    let connections_before = observations(&router, "ActiveConnections", &api.api_id).await;

    // Every reading taken after the stop reads what the plane holds THEN: no subscription,
    // and the connection that held it still open.
    for (metric, before, expected) in [
        ("ActiveSubscriptions", subscriptions_before, 0.0),
        ("ActiveConnections", connections_before, 1.0),
    ] {
        let after = awaited_readings(&router, case, metric, &api.api_id, before).await;
        let readings = after.count - before.count;
        let read = after.sum - before.sum;
        assert_eq!(
            read,
            readings * expected,
            "case {case:?}: expected every one of the {readings} further {metric} readings to \
             read {expected}, got {read} over them ({before:?} then {after:?})"
        );
    }

    // The connection was deliberately held open for all of it, which is what those readings
    // were taken while: it still answers over its own socket.
    let answered = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
    assert_eq!(
        answered.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("connection_ack"),
        "case {case:?}: the connection must still be held open, got {answered:?}"
    );
}

#[tokio::test]
async fn each_series_is_catalogued_under_exactly_the_apis_own_dimension() {
    let router = emulator();
    let api = create_api(&router, "catalogue").await;
    let case = "one connect, one subscribe, one delivery, one stop and one close";
    let mut socket = connected_with_key(case, &api.realtime, &api.api_key).await;
    started(case, &mut socket, "watch-1").await;
    mutate(case, &api, "P-0", ACCEPTED_CHANNEL).await;
    let delivered = next_frame(&mut socket).await;
    assert_eq!(
        delivered.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("data"),
        "case {case:?}: the subscriber must receive the publication, got {delivered:?}"
    );
    let completed = exchange(case, &mut socket, &json!({"id": "watch-1", "type": "stop"})).await;
    assert_eq!(
        completed.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("complete"),
        "case {case:?}: the stop must be answered complete, got {completed:?}"
    );
    let _ = socket.close(None).await;
    drop(socket);
    let _ = awaited_sum(&router, case, "DisconnectSuccess", &api.api_id, 1.0).await;

    // Every series the whole lifecycle owes is catalogued, so a client can find it without
    // knowing it is there.
    let catalogued = catalogued(&router, &api.api_id).await;
    let owed = [
        "ConnectSuccess",
        "SubscribeSuccess",
        "PublishDataMessageSuccess",
        "UnsubscribeSuccess",
        "DisconnectSuccess",
        "ActiveSubscriptions",
        "ActiveConnections",
    ];
    for metric in owed {
        assert!(
            catalogued.iter().any(|(name, _)| name == metric),
            "case {case:?}: expected {NAMESPACE} to catalogue {metric} for this API, got \
             {catalogued:?}"
        );
    }
    // And each of them under exactly the one dimension that names the API: a connect, a
    // subscribe and a delivery are all of the API, and of nothing narrower.
    for (metric, dimensions) in &catalogued {
        assert_eq!(
            dimensions,
            &vec![API_DIMENSION.to_string()],
            "case {case:?}: {metric} belongs to the API alone, got {dimensions:?}"
        );
    }
    // Nothing else: a series neither of the API's planes publishes would be one nobody can
    // account for. The two refusal counters are the rest of what the real-time plane
    // publishes ([`crate::graphql_api_realtime_endpoint`] grades those), and the four series
    // the REQUEST plane reports about each mutation this case POSTed to the API's own GraphQL
    // endpoint are the rest of what the API publishes at this dimension
    // ([`crate::graphql_api_request_metrics`] grades those).
    let published = {
        let mut published: Vec<&str> = owed.to_vec();
        published.extend(["ConnectRequests", "ConnectClientError"]);
        published.extend(["Requests", "Latency", "4XXError", "5XXError"]);
        published
    };
    for (metric, _) in &catalogued {
        assert!(
            published.contains(&metric.as_str()),
            "case {case:?}: {NAMESPACE} carries a series this plane does not publish: \
             {metric}"
        );
    }
}

/// The id one connection's `index`th subscription is started under: the client's own, so it
/// is unique within the connection that chose it.
fn subscription_id(connection: usize, index: usize) -> String {
    format!("watch-{connection}-{index}")
}

/// Create one GraphQL API with an API key, [`SCHEMA`], a `NONE` data source, the mutation's
/// resolver and the subscription field's filtering handler.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("realtime-metrics-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    for (type_name, field, code) in [
        ("Mutation", "addPost", ECHOED_POST),
        (
            "Subscription",
            SUBSCRIPTION_FIELD,
            FILTERS_ON_THE_ACCEPTED_CHANNEL,
        ),
    ] {
        let (status, attached) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": code,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {type_name}.{field} must answer 200, got {attached}"
        );
    }
    GraphqlApi {
        api_id,
        graphql: crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL"),
        realtime: crate::graphql_api_endpoint_dns_records::published_authority(api, "REALTIME"),
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Start one subscription on the suite's subscription field under `id`, asserting the
/// endpoint acked it.
async fn started(case: &str, socket: &mut ClientSocket, id: &str) {
    let document = format!("subscription Watch {{ {SUBSCRIPTION_FIELD} {{ id channel body }} }}");
    let acked = exchange(case, socket, &started_message(id, &document, &json!({}))).await;
    assert_eq!(
        acked.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("start_ack"),
        "case {case:?}: a start on a subscription field of the schema must be acked, got \
         {acked:?}"
    );
}

/// POST one `addPost` of `channel` to the API's published GraphQL endpoint, as a client
/// does, and answer the document it was served.
async fn mutate(case: &str, api: &GraphqlApi, id: &str, channel: &str) -> Value {
    let document = json!({
        "query": "mutation Write($id: ID!, $channel: String!, $body: String) { \
                  addPost(id: $id, channel: $channel, body: $body) { id channel body } }",
        "variables": {"id": id, "channel": channel, "body": "written"},
    });
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &document,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    assert_eq!(
        status, 200,
        "case {case:?}: a GraphQL request is answered 200, got {status} and {answered}"
    );
    answered
}

/// What one reading of a series amounts to in the store: how many observations it holds, and
/// what they add up to.
///
/// The pair is what makes a CADENCE observable: the count says how many readings were taken,
/// and the sum over exactly those readings says what each of them read.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Observations {
    count: f64,
    sum: f64,
}

/// Wait until the series `metric` of `api_id` sums to at least `wanted`, and answer what it
/// sums to then — a poll loop on the CONDITION, so a loaded machine still reaches it.
async fn awaited_sum(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    api_id: &str,
    wanted: f64,
) -> Option<f64> {
    let deadline = tokio::time::Instant::now() + REPORT_DEADLINE;
    loop {
        let summed = metric_sum(router, metric, api_id).await;
        if summed.is_some_and(|sum| sum >= wanted) {
            return summed;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected {metric} to sum to at least {wanted}, got {summed:?}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Wait until some reading of the gauge `metric` of `api_id` reads at least `wanted`, and
/// answer the largest reading it holds then.
async fn awaited_maximum(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    api_id: &str,
    wanted: f64,
) -> Option<f64> {
    let deadline = tokio::time::Instant::now() + REPORT_DEADLINE;
    loop {
        let highest = statistics(router, metric, api_id)
            .await
            .iter()
            .filter_map(|point| point["Maximum"].as_f64())
            .reduce(f64::max);
        if highest.is_some_and(|read| read >= wanted) {
            return highest;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected a {metric} reading of at least {wanted}, got {highest:?}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Wait until the series `metric` of `api_id` holds at least [`READINGS_AWAITED`] more
/// observations than `before`, and answer what it holds then.
async fn awaited_readings(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    api_id: &str,
    before: Observations,
) -> Observations {
    let deadline = tokio::time::Instant::now() + REPORT_DEADLINE;
    loop {
        let now = observations(router, metric, api_id).await;
        if now.count - before.count >= READINGS_AWAITED {
            return now;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected at least {READINGS_AWAITED} further {metric} readings \
             while the connection is open, got {now:?} after {before:?}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// What the series `metric` of `api_id` holds, folded across every period it covers, so a
/// reading that lands in the next minute while a case is polling is counted exactly once.
async fn observations(router: &MultiServiceRouter, metric: &str, api_id: &str) -> Observations {
    let datapoints = statistics(router, metric, api_id).await;
    let fold = |member: &str| {
        datapoints
            .iter()
            .filter_map(|point| point[member].as_f64())
            .sum::<f64>()
    };
    Observations {
        count: fold("SampleCount"),
        sum: fold("Sum"),
    }
}

/// The `Sum` of every datapoint of one `[GraphQLAPIId]` series, or `None` when the series
/// holds none.
async fn metric_sum(router: &MultiServiceRouter, metric: &str, api_id: &str) -> Option<f64> {
    let datapoints = statistics(router, metric, api_id).await;
    if datapoints.is_empty() {
        return None;
    }
    Some(
        datapoints
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum(),
    )
}

/// Every datapoint of one `[GraphQLAPIId]` series, as `GetMetricStatistics` reports them at
/// the 60-second period a client reads these series at.
async fn statistics(router: &MultiServiceRouter, metric: &str, api_id: &str) -> Vec<Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let (status, body) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": API_DIMENSION, "Value": api_id}],
            "StartTime": now - 1_800.0,
            "EndTime": now + 60.0,
            "Period": 60,
            "Statistics": ["SampleCount", "Sum", "Minimum", "Maximum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {metric} must answer 200, got {body}"
    );
    body["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// Every `AWS/AppSync` series of one API, as `ListMetrics` catalogues it: the metric name
/// and the dimension NAMES it is published under.
async fn catalogued(router: &MultiServiceRouter, api_id: &str) -> Vec<(String, Vec<String>)> {
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
    body["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|metric| {
            let names: Vec<String> = metric["Dimensions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|dimension| dimension["Name"].as_str().map(str::to_string))
                .collect();
            (
                metric["MetricName"].as_str().unwrap_or_default().to_string(),
                names,
            )
        })
        .collect()
}
