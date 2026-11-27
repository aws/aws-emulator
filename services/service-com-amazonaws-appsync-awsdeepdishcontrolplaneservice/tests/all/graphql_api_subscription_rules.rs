// SPDX-License-Identifier: CC0-1.0

//! What a SUBSCRIPTION field's own resolver and its subscriber's own document decide about
//! the frames that subscriber receives, driven over both of the API's published sockets.
//!
//! The rules these rows assert, one level above any single schema:
//!
//! > 1. A subscriber receives the GraphQL result of ITS OWN subscription: the published
//! >    object PROJECTED onto the selection that subscriber's document made — every member
//! >    it selected, under the key it reported it by, and nothing else — so a member the
//! >    subscription did not select is never delivered to it, whatever the publisher's own
//! >    mutation selected.
//! > 2. A subscription field's resolver decides the SUBSCRIPTION: its request half runs when
//! >    a subscriber subscribes, and the filter it installs through
//! >    `extensions.setSubscriptionFilter` is evaluated against every publication on the
//! >    field, so only the publications that filter accepts are delivered. A handler that
//! >    REFUSES refuses the subscribe, rather than registering an unfiltered subscription.
//! > 3. A mutation's response half may END subscriptions through
//! >    `extensions.invalidateSubscriptions({subscriptionField, payload})`: the mutation is
//! >    answered 200 with its own data and no errors, and exactly those subscriptions of
//! >    that field whose own `extensions.setSubscriptionInvalidationFilter` accepts the
//! >    payload are sent a `complete` frame and receive nothing more, while every other
//! >    subscriber keeps receiving its own publications.
//!
//! Nothing here is asserted off the store: every frame is read off a real WebSocket, over the
//! address the API published for it, and every mutation is POSTed to the address it published
//! for that — through the same client and the same harness the sibling delivery suite uses
//! ([`crate::graphql_api_subscription_delivery`]).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::{appsync, emulator};
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

/// One created GraphQL API: the addresses both its endpoints are served on, and a live key.
struct GraphqlApi {
    graphql: String,
    realtime: String,
    api_key: String,
}

/// One resolver to attach: the type, the field and the code it is defined by.
struct Resolver<'a> {
    type_name: &'a str,
    field: &'a str,
    code: &'a str,
}

/// RULE 1: what a subscriber receives is the publication projected onto the selection ITS
/// document made.
#[tokio::test]
async fn a_subscriber_receives_only_the_fields_its_own_document_selected() {
    let router = emulator();
    let api = create_api(
        &router,
        "projection",
        POST_SCHEMA,
        &[Resolver {
            type_name: "Mutation",
            field: "addPost",
            code: ECHOED_POST,
        }],
    )
    .await;

    /// One subscriber of a row: the selection its document makes, and the payload it must
    /// then read under the subscription field.
    struct Row {
        case: &'static str,
        id: &'static str,
        selection: &'static str,
        expected: Value,
    }
    // The rows differ along HOW MUCH of the publication the subscription selects (one
    // member, a subset, every member the publisher selected), and whether it renames one.
    let rows = vec![
        Row {
            case: "a subset of what the mutation selected",
            id: "watch-subset",
            selection: "id body",
            expected: json!({"id": "P-1", "body": "hello"}),
        },
        Row {
            case: "one member only",
            id: "watch-one",
            selection: "id",
            expected: json!({"id": "P-1"}),
        },
        Row {
            case: "every member the mutation selected",
            id: "watch-all",
            selection: "id body secret",
            expected: json!({"id": "P-1", "body": "hello", "secret": "shh"}),
        },
        Row {
            case: "an aliased member",
            id: "watch-aliased",
            selection: "identifier: id body",
            expected: json!({"identifier": "P-1", "body": "hello"}),
        },
    ];

    // Each subscriber on its OWN connection, so what it received is read off its own socket.
    let mut sockets: Vec<(&Row, ClientSocket)> = Vec::new();
    for row in &rows {
        let mut socket = connected_with_key(row.case, &api.realtime, &api.api_key).await;
        let document = format!(
            "subscription OnAdd {{ onAddPost {{ {} }} }}",
            row.selection
        );
        assert_started(row.case, &mut socket, row.id, &document).await;
        sockets.push((row, socket));
    }

    // ONE publication, carrying a member no subscription above has to select.
    let answered = post(
        "the published mutation",
        &api,
        "mutation Add($id: ID!, $body: String, $secret: String) { \
         addPost(id: $id, body: $body, secret: $secret) { id body secret } }",
        &json!({"id": "P-1", "body": "hello", "secret": "shh"}),
    )
    .await;
    assert_eq!(
        answered["data"]["addPost"]["secret"].as_str(),
        Some("shh"),
        "the mutation must be served with its own selection, got {answered}"
    );

    for (row, socket) in &mut sockets {
        let case = row.case;
        let frame = next_frame(socket)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: this subscriber must receive the mutation"));
        assert_eq!(
            frame["id"].as_str(),
            Some(row.id),
            "case {case:?}: the frame must be addressed by this subscription's id, got {frame}"
        );
        let delivered = frame
            .pointer("/payload/data/onAddPost")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            delivered, row.expected,
            "case {case:?}: expected exactly the selected members {}, got {delivered}",
            row.expected
        );
    }
}

/// RULE 2: the filter a subscribe-time handler installs decides which publications reach the
/// subscriber, and a handler that refuses refuses the subscribe.
#[tokio::test]
async fn a_subscribe_time_filter_decides_which_publications_are_delivered() {
    let router = emulator();
    let api = create_api(
        &router,
        "filtering",
        CHANNEL_SCHEMA,
        &[
            Resolver {
                type_name: "Mutation",
                field: "addPost",
                code: ECHOED_CHANNEL_POST,
            },
            Resolver {
                type_name: "Subscription",
                field: "onAddPost",
                code: FILTERS_ON_ALPHA,
            },
            Resolver {
                type_name: "Subscription",
                field: "onSecretPost",
                code: REFUSES_THE_SUBSCRIBE,
            },
        ],
    )
    .await;

    let case = "a filter on one channel";
    let mut subscriber = connected_with_key(case, &api.realtime, &api.api_key).await;
    assert_started(
        case,
        &mut subscriber,
        "watch-alpha",
        "subscription OnAdd { onAddPost { id channel body } }",
    )
    .await;

    // Three publications, in this order: two the filter rejects and one it accepts. The
    // accepted one is published LAST, so a frame read before it would be a rejected
    // publication that was delivered anyway.
    for (id, channel) in [("P-beta", "beta"), ("P-gamma", "gamma"), ("P-alpha", "alpha")] {
        let answered = post(
            case,
            &api,
            "mutation Add($id: ID!, $channel: String!, $body: String) { \
             addPost(id: $id, channel: $channel, body: $body) { id channel body } }",
            &json!({"id": id, "channel": channel, "body": id}),
        )
        .await;
        assert_eq!(
            answered["data"]["addPost"]["channel"].as_str(),
            Some(channel),
            "case {case:?}: every publication must be served, got {answered}"
        );
    }

    let frame = next_frame(&mut subscriber)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the accepted publication must be delivered"));
    assert_eq!(
        frame.pointer("/payload/data/onAddPost"),
        Some(&json!({"id": "P-alpha", "channel": "alpha", "body": "P-alpha"})),
        "case {case:?}: expected only the channel the filter names, got {frame}"
    );
    let further = next_frame(&mut subscriber).await;
    assert!(
        further.is_none(),
        "case {case:?}: a publication the filter rejects is delivered to nobody, got {further:?}"
    );

    // A subscribe-time handler that REFUSES is not a subscription at all: the start is
    // answered an error frame rather than an ack, so nothing is ever delivered to it.
    let refused_case = "a subscribe-time handler that refuses";
    let mut refused = connected_with_key(refused_case, &api.realtime, &api.api_key).await;
    let answer = exchange(
        refused_case,
        &mut refused,
        &started_message(
            "watch-refused",
            "subscription OnSecret { onSecretPost { id } }",
            &json!({}),
        ),
    )
    .await;
    let answer =
        answer.unwrap_or_else(|| panic!("case {refused_case:?}: the start must be answered"));
    assert_eq!(
        (
            answer["type"].as_str(),
            answer.pointer("/payload/errors/0/errorType").cloned()
        ),
        (Some("error"), Some(json!("UnauthorizedException"))),
        "case {refused_case:?}: expected the subscribe to be refused, got {answer}"
    );
}

/// RULE 3: an invalidation a mutation raises ends exactly the subscriptions whose own
/// invalidation filter accepts its payload.
#[tokio::test]
async fn an_invalidation_completes_only_the_subscriptions_its_payload_names() {
    let router = emulator();
    let api = create_api(
        &router,
        "invalidation",
        MESSAGE_SCHEMA,
        &[
            Resolver {
                type_name: "Mutation",
                field: "publish",
                code: ECHOED_MESSAGE,
            },
            Resolver {
                type_name: "Mutation",
                field: "revoke",
                code: RAISES_THE_INVALIDATION,
            },
            Resolver {
                type_name: "Subscription",
                field: "onMsg",
                code: INVALIDATED_BY_GROUP,
            },
        ],
    )
    .await;

    // Two subscribers, each on a group of its own, each on its own connection.
    let mut invalidated = connected_with_key("the invalidated group", &api.realtime, &api.api_key)
        .await;
    assert_started(
        "the invalidated group",
        &mut invalidated,
        "watch-a",
        "subscription OnMsg { onMsg(group: \"A\") { group body } }",
    )
    .await;
    let mut untouched =
        connected_with_key("the untouched group", &api.realtime, &api.api_key).await;
    assert_started(
        "the untouched group",
        &mut untouched,
        "watch-b",
        "subscription OnMsg { onMsg(group: \"B\") { group body } }",
    )
    .await;

    // The mutation that raises the invalidation answers normally: its own data, no errors.
    let revoked = post(
        "the invalidating mutation",
        &api,
        "mutation Revoke($group: String!) { revoke(group: $group) { group body } }",
        &json!({"group": "A"}),
    )
    .await;
    assert_eq!(
        (
            revoked["data"]["revoke"]["group"].as_str(),
            revoked.get("errors")
        ),
        (Some("A"), None),
        "the invalidating mutation must be answered with its own data and no errors, got \
         {revoked}"
    );

    // The subscription the payload names is OVER: its subscriber reads the `complete` frame
    // for its own id.
    let completed = next_frame(&mut invalidated)
        .await
        .unwrap_or_else(|| panic!("the invalidated subscriber must be completed"));
    assert_eq!(
        completed,
        json!({"id": "watch-a", "type": "complete"}),
        "expected the invalidated subscription to be completed by its own id, got {completed}"
    );

    // The other subscription is untouched: it still receives its own group's publications.
    for (group, body) in [("A", "after-a"), ("B", "after-b")] {
        let published = post(
            "a publication after the invalidation",
            &api,
            "mutation Publish($group: String!, $body: String!) { \
             publish(group: $group, body: $body) { group body } }",
            &json!({"group": group, "body": body}),
        )
        .await;
        assert_eq!(
            published["data"]["publish"]["body"].as_str(),
            Some(body),
            "the publication must be served, got {published}"
        );
    }
    let delivered = next_frame(&mut untouched)
        .await
        .unwrap_or_else(|| panic!("the untouched subscriber must still receive its own group"));
    assert_eq!(
        delivered.pointer("/payload/data/onMsg"),
        Some(&json!({"group": "B", "body": "after-b"})),
        "expected the untouched subscription to receive its own group, got {delivered}"
    );
    // And the invalidated one receives nothing further, not even the publication of its own
    // group that followed the invalidation.
    let further = next_frame(&mut invalidated).await;
    assert!(
        further.is_none(),
        "an invalidated subscription receives nothing more, got {further:?}"
    );
}

/// The schema rule 1 is asserted over: a mutation whose result carries a member no
/// subscriber has to select.
const POST_SCHEMA: &str = "type Post {\n  id: ID!\n  body: String\n  secret: String\n}\n\
     \n\
     type Query {\n  post(id: ID!): Post\n}\n\
     \n\
     type Mutation {\n  addPost(id: ID!, body: String, secret: String): Post\n}\n\
     \n\
     type Subscription {\n  \
     onAddPost: Post @aws_subscribe(mutations: [\"addPost\"])\n}\n";

/// The schema rule 2 is asserted over: a publication carrying the member its filter names,
/// and a second subscription field whose handler refuses the subscribe.
const CHANNEL_SCHEMA: &str = "type Post {\n  id: ID!\n  channel: String\n  body: String\n}\n\
     \n\
     type Query {\n  post(id: ID!): Post\n}\n\
     \n\
     type Mutation {\n  addPost(id: ID!, channel: String!, body: String): Post\n}\n\
     \n\
     type Subscription {\n  \
     onAddPost: Post @aws_subscribe(mutations: [\"addPost\"])\n  \
     onSecretPost: Post @aws_subscribe(mutations: [\"addPost\"])\n}\n";

/// The schema rule 3 is asserted over: one mutation that publishes and one that revokes.
const MESSAGE_SCHEMA: &str = "type Msg {\n  group: String\n  body: String\n}\n\
     \n\
     type Query {\n  msg(group: String!): Msg\n}\n\
     \n\
     type Mutation {\n  publish(group: String!, body: String!): Msg\n  \
     revoke(group: String!): Msg\n}\n\
     \n\
     type Subscription {\n  \
     onMsg(group: String!): Msg @aws_subscribe(mutations: [\"publish\"])\n}\n";

/// A mutation resolver whose result is the post its arguments carry.
const ECHOED_POST: &str = "export function request(ctx) {\n  \
     return {payload: {id: ctx.args.id, body: ctx.args.body, secret: ctx.args.secret}};\n}\n\
     export function response(ctx) { return ctx.result; }";

/// The same, for the schema whose post carries a channel.
const ECHOED_CHANNEL_POST: &str = "export function request(ctx) {\n  \
     return {payload: {id: ctx.args.id, channel: ctx.args.channel, body: ctx.args.body}};\n}\n\
     export function response(ctx) { return ctx.result; }";

/// The same, for the schema whose publication is a message of one group.
const ECHOED_MESSAGE: &str = "export function request(ctx) {\n  \
     return {payload: {group: ctx.args.group, body: ctx.args.body}};\n}\n\
     export function response(ctx) { return ctx.result; }";

/// A subscribe-time handler installing the filter that accepts one channel only.
const FILTERS_ON_ALPHA: &str = "import { extensions } from '@aws-appsync/utils';\n\
     export function request(ctx) {\n  \
     extensions.setSubscriptionFilter({filterGroup: [{filters: [\n    \
     {fieldName: 'channel', operator: 'eq', value: 'alpha'},\n  ]}]});\n  \
     return {payload: null};\n}\n\
     export function response(ctx) { return null; }";

/// A subscribe-time handler that REFUSES the subscriber.
const REFUSES_THE_SUBSCRIBE: &str = "export function request(ctx) {\n  \
     util.unauthorized();\n}\n\
     export function response(ctx) { return null; }";

/// A subscribe-time handler installing the invalidation filter of the group the subscription
/// was started for.
const INVALIDATED_BY_GROUP: &str = "import { extensions } from '@aws-appsync/utils';\n\
     export function request(ctx) {\n  \
     extensions.setSubscriptionInvalidationFilter({filterGroup: [{filters: [\n    \
     {fieldName: 'group', operator: 'eq', value: ctx.args.group},\n  ]}]});\n  \
     return {payload: null};\n}\n\
     export function response(ctx) { return null; }";

/// A mutation whose RESPONSE half raises the invalidation of one group's subscriptions and
/// still returns its own result.
const RAISES_THE_INVALIDATION: &str = "import { extensions } from '@aws-appsync/utils';\n\
     export function request(ctx) {\n  \
     return {payload: {group: ctx.args.group, body: 'revoked'}};\n}\n\
     export function response(ctx) {\n  \
     extensions.invalidateSubscriptions({subscriptionField: 'onMsg', \
     payload: {group: ctx.args.group}});\n  \
     return ctx.result;\n}";

/// Create one GraphQL API with an API key, `schema`, a `NONE` data source and `resolvers`,
/// and read the addresses its two endpoints are served on.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    schema: &str,
    resolvers: &[Resolver<'_>],
) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("subscription-rules-{name}"),
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
    install_schema(router, &api_id, schema).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    for resolver in resolvers {
        let (status, attached) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{}/resolvers", resolver.type_name),
            Some(json!({
                "fieldName": resolver.field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": resolver.code,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {}.{} must answer 200, got {attached}",
            resolver.type_name, resolver.field
        );
    }
    GraphqlApi {
        graphql: crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL"),
        realtime: crate::graphql_api_endpoint_dns_records::published_authority(api, "REALTIME"),
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Start the subscription `document` under `id`, asserting the endpoint acked it.
async fn assert_started(case: &str, socket: &mut ClientSocket, id: &str, document: &str) {
    let acked = exchange(case, socket, &started_message(id, document, &json!({}))).await;
    assert_eq!(
        acked.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("start_ack"),
        "case {case:?}: a start on a subscription field of the schema must be acked, got \
         {acked:?}"
    );
}

/// POST one operation to the API's published GraphQL endpoint, as a client does, and answer
/// the document it was served.
async fn post(case: &str, api: &GraphqlApi, query: &str, variables: &Value) -> Value {
    let document = json!({"query": query, "variables": variables});
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
