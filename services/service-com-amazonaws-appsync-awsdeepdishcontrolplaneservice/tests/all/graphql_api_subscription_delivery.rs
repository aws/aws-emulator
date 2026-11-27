// SPDX-License-Identifier: CC0-1.0

//! What a MUTATION of a GraphQL API delivers to the subscriptions open on its real-time
//! endpoint, driven over both of the API's own published sockets.
//!
//! The rule these rows assert, one level above any single schema:
//!
//! > A mutation served at a GraphQL API's `GRAPHQL` endpoint is PUBLISHED to the
//! > subscriptions its schema links to that mutation field with
//! > `@aws_subscribe(mutations: […])`: every live subscription on such a subscription field
//! > receives what the mutation field resolved to, as the GraphQL result of its own
//! > subscription — the published object under the subscription field's name, inside the
//! > `payload.data` of a `data` frame addressed by the subscription's own id. A subscription
//! > on a field the mutation is not linked to receives nothing, and a subscription's own
//! > ARGUMENTS filter what it receives: it is delivered a published object only where every
//! > argument it states equals the same-named member of that object.
//!
//! The rows span what the rule generalises over: which mutation was served (one linked to one
//! subscription, one linked to two, one linked to none), which field a subscriber is on (one
//! the served mutation is linked to, one it is not), the arguments a subscription states
//! (none, one that matches, one that does not) and how it states them (inline in the
//! document, through a variable).
//!
//! Nothing here is asserted off the store: every delivery is read off a real WebSocket, over
//! the address the API published for it, through the same client the real-time endpoint's own
//! suite dials with ([`crate::graphql_api_realtime_endpoint`]).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::{appsync, emulator};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_realtime_endpoint::{
    connected_with_key, exchange, next_frame, started_message, ClientSocket,
};

/// The path the GraphQL endpoint serves; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The `runtime` the resolvers of this suite are defined under. Source: `AppSyncRuntime`'s
/// own values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The name of the local data source every resolver of this suite resolves against: `NONE` is
/// "There is no data source", so the request function's own payload is the result.
const DATA_SOURCE: &str = "local";

/// The schema every API of this suite is created with: two mutations and three subscriptions
/// — one linked to the first mutation, one linked to BOTH, and one linked to no mutation at
/// all.
const SCHEMA: &str = "type Note {\n  id: ID!\n  owner: String\n  text: String\n}\n\
     \n\
     type Query {\n  note(id: ID!): Note\n}\n\
     \n\
     type Mutation {\n  addNote(id: ID!, owner: String!, text: String): Note\n  \
     editNote(id: ID!, owner: String!, text: String): Note\n}\n\
     \n\
     type Subscription {\n  \
     onAddNote(owner: String): Note @aws_subscribe(mutations: [\"addNote\"])\n  \
     onAnyNote(owner: String): Note @aws_subscribe(mutations: [\"addNote\", \"editNote\"])\n  \
     onDeleteNote: Note @aws_subscribe(mutations: [\"deleteNote\"])\n}\n";

/// One created GraphQL API: the addresses both its endpoints are served on, and a live key.
struct GraphqlApi {
    graphql: String,
    realtime: String,
    api_key: String,
}

#[tokio::test]
async fn a_mutation_is_delivered_to_the_subscriptions_its_schema_links_it_to() {
    let router = emulator();
    let api = create_api(&router, "delivery").await;
    let case = "a mutation linked to one subscription";

    // Two subscribers of one connection: one on the field the mutation is linked to, one on
    // the field it is not.
    let mut subscriber = connected_with_key(case, &api.realtime, &api.api_key).await;
    started(case, &mut subscriber, "watch-add", "onAddNote(owner: \"alice\")").await;
    started(case, &mut subscriber, "watch-none", "onDeleteNote").await;

    // The mutation, served at the API's own GraphQL endpoint.
    let answered = mutate(
        case,
        &api,
        "addNote",
        &json!({"id": "N-1", "owner": "alice", "text": "first"}),
    )
    .await;
    assert_eq!(
        answered["data"]["addNote"]["id"].as_str(),
        Some("N-1"),
        "case {case:?}: the mutation must be served by the API's own resolver, got {answered}"
    );

    // What the subscriber reads is the GraphQL result of ITS OWN subscription, addressed by
    // the id it started that subscription under.
    let delivered = next_frame(&mut subscriber)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the subscriber must receive the mutation"));
    assert_eq!(
        delivered,
        json!({
            "id": "watch-add",
            "type": "data",
            "payload": {"data": {"onAddNote": {"id": "N-1", "owner": "alice", "text": "first"}}},
        }),
        "case {case:?}: expected the published note under the subscription field, got \
         {delivered}"
    );
    // And nothing else: the subscription the mutation is not linked to received nothing.
    let further = next_frame(&mut subscriber).await;
    assert!(
        further.is_none(),
        "case {case:?}: a subscription the mutation is not linked to receives nothing, got \
         {further:?}"
    );
}

#[tokio::test]
async fn every_subscription_the_mutation_is_linked_to_receives_it_and_the_filters_decide_which() {
    let router = emulator();
    let api = create_api(&router, "filters").await;

    /// One subscriber of a row: the case, the subscription id and the document's own
    /// selection, with the variables it is parameterised by.
    struct Subscriber {
        case: &'static str,
        id: &'static str,
        selection: &'static str,
        variables: Value,
        /// Whether this subscriber must receive the mutation below.
        delivered: bool,
    }
    // The rows differ along the field subscribed to, the arguments stated and HOW they are
    // stated: none at all, an inline literal that matches, an inline literal that does not,
    // and one carried in a variable.
    let rows = vec![
        Subscriber {
            case: "a subscription stating no argument",
            id: "all",
            selection: "onAnyNote",
            variables: json!({}),
            delivered: true,
        },
        Subscriber {
            case: "an inline argument the published note matches",
            id: "inline-match",
            selection: "onAnyNote(owner: \"alice\")",
            variables: json!({}),
            delivered: true,
        },
        Subscriber {
            case: "an inline argument the published note does not match",
            id: "inline-mismatch",
            selection: "onAnyNote(owner: \"bob\")",
            variables: json!({}),
            delivered: false,
        },
        Subscriber {
            case: "the same argument carried in a variable",
            id: "varied",
            selection: "onAnyNote(owner: $owner)",
            variables: json!({"owner": "alice"}),
            delivered: true,
        },
        Subscriber {
            case: "a field this mutation is not linked to",
            id: "other-field",
            selection: "onAddNote",
            variables: json!({}),
            delivered: false,
        },
    ];

    // Each subscriber on its OWN connection, so what it received is read off its own socket.
    let mut sockets: Vec<(&Subscriber, ClientSocket)> = Vec::new();
    for row in &rows {
        let mut socket = connected_with_key(row.case, &api.realtime, &api.api_key).await;
        let document = match row.variables.as_object().is_some_and(|held| held.is_empty()) {
            true => format!("subscription Watch {{ {} {{ id owner text }} }}", row.selection),
            false => format!(
                "subscription Watch($owner: String) {{ {} {{ id owner text }} }}",
                row.selection
            ),
        };
        let acked = exchange(
            row.case,
            &mut socket,
            &started_message(row.id, &document, &row.variables),
        )
        .await;
        assert_eq!(
            acked.as_ref().and_then(|frame| frame["type"].as_str()),
            Some("start_ack"),
            "case {:?}: the start must be acked, got {acked:?}",
            row.case
        );
        sockets.push((row, socket));
    }

    let answered = mutate(
        "the mutation every row is judged against",
        &api,
        "editNote",
        &json!({"id": "N-2", "owner": "alice", "text": "edited"}),
    )
    .await;
    assert_eq!(
        answered["data"]["editNote"]["owner"].as_str(),
        Some("alice"),
        "the mutation must be served, got {answered}"
    );

    for (row, socket) in &mut sockets {
        let case = row.case;
        let delivered = next_frame(socket).await;
        match row.delivered {
            true => {
                let frame = delivered.unwrap_or_else(|| {
                    panic!("case {case:?}: this subscriber must receive the mutation")
                });
                assert_eq!(
                    frame["id"].as_str(),
                    Some(row.id),
                    "case {case:?}: the frame must be addressed by this subscription's id, \
                     got {frame}"
                );
                let subscribed = row
                    .selection
                    .split_once('(')
                    .map_or(row.selection, |(field, _)| field);
                assert_eq!(
                    frame.pointer(&format!("/payload/data/{subscribed}")),
                    Some(&json!({"id": "N-2", "owner": "alice", "text": "edited"})),
                    "case {case:?}: expected the published note under {subscribed}, got {frame}"
                );
            }
            false => assert!(
                delivered.is_none(),
                "case {case:?}: a subscription whose arguments the note does not match \
                 receives nothing, got {delivered:?}"
            ),
        }
    }
}

/// Create one GraphQL API with an API key, [`SCHEMA`], a `NONE` data source and one resolver
/// per mutation, and read the addresses its two endpoints are served on.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("subscriptions-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    let realtime = crate::graphql_api_endpoint_dns_records::published_authority(api, "REALTIME");
    for published in [&graphql, &realtime] {
        assert!(
            published.starts_with(&format!("{LOOPBACK_HOST}:")),
            "an API's endpoints must be published on an address this emulator serves, got \
             {published:?}"
        );
    }
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
    // One resolver per mutation: the note it writes is what the mutation's arguments carry,
    // which is what the subscribers below must receive.
    let code = "export function request(ctx) {\n\
           return {payload: {id: ctx.args.id, owner: ctx.args.owner, text: ctx.args.text}};\n\
         }\n\
         export function response(ctx) { return ctx.result; }";
    for field in ["addNote", "editNote"] {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
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
            "CreateResolver for {field} must answer 200, got {created}"
        );
    }
    GraphqlApi {
        graphql,
        realtime,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Start one subscription on `selection` under `id`, asserting the endpoint acked it.
async fn started(case: &str, socket: &mut ClientSocket, id: &str, selection: &str) {
    let document = format!("subscription Watch {{ {selection} {{ id owner text }} }}");
    let acked = exchange(case, socket, &started_message(id, &document, &json!({}))).await;
    assert_eq!(
        acked.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("start_ack"),
        "case {case:?}: a start on a subscription field of the schema must be acked, got \
         {acked:?}"
    );
}

/// POST one mutation of `field` with `arguments` to the API's published GraphQL endpoint, as
/// a client does, and answer the document it was served.
async fn mutate(case: &str, api: &GraphqlApi, field: &str, arguments: &Value) -> Value {
    let document = json!({
        "query": format!(
            "mutation Write($id: ID!, $owner: String!, $text: String) {{ \
             {field}(id: $id, owner: $owner, text: $text) {{ id owner text }} }}"
        ),
        "variables": arguments,
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
