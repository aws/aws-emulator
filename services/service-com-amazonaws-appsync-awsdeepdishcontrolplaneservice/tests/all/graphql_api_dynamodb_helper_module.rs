// SPDX-License-Identifier: CC0-1.0

//! The DynamoDB helper module an APPSYNC_JS resolver IMPORTS, driven over the two sockets an
//! `AWS_IAM` GraphQL API publishes.
//!
//! The rule these cases assert, one level above the recorded case (one mutation whose request
//! handler returns `ddb.put({key, item})`):
//!
//! > An APPSYNC_JS resolver may `import` the DynamoDB helper module AppSync publishes
//! > (`@aws-appsync/utils/dynamodb`) rather than writing its data source's request object out,
//! > and each helper it exports BUILDS the request object of one operation from the plain
//! > values it was handed — marshalling them as the attribute values DynamoDB holds them as,
//! > and compiling a condition, a filter, a projection or an update stated as an OBJECT into
//! > the expression and the parameter maps the call carries — so a resolver that returns what a
//! > helper built resolves its field against the table its data source names, exactly as one
//! > returning the same request object by hand does.
//!
//! The rows span what the rule generalises over: the HELPER (`put`, `get`, `scan`, `update`
//! with an `operations` change, and `remove`), the import FORM each resolver reaches it through
//! (a namespace import, a named one, the default one), and the values a payload carries (a key,
//! an item of several attributes, a filter object, a counter the stored item does not have
//! yet). What the table holds is read back through DynamoDB's OWN api, so a write that did not
//! reach it cannot pass, and the signed mutation's delivery to a real-time subscriber is read
//! off that subscriber's own socket.
//!
//! `ctx.identity` is part of the same flow: the item the mutation writes carries the account
//! the call signed as, which only a request whose SigV4 material was evaluated can report.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_iam_realtime_dynamodb_flow::{
    create_role, create_table, dynamodb, emulator, signed_auth_object, signed_graphql,
    sigv4_material, GRAPHQL_PATH, KEY_ATTRIBUTE, REALTIME_PATH, REGION, TABLE,
};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_realtime_endpoint::{exchange, handshake, next_frame, started_message};

/// The name of the `AMAZON_DYNAMODB` data source every resolver of this suite is bound to.
const DATA_SOURCE: &str = "Notes";

/// The `runtime` the resolvers of this suite are defined under. Source: `AppSyncRuntime`'s own
/// values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The schema the API is created with: the mutations the helpers write through, the queries
/// they read through, and the subscription the schema links to the mutation that creates a
/// note.
const SCHEMA: &str = "type Note {\n  \
                      id: ID!\n  board: String\n  body: String\n  author: String\n  \
                      votes: Int\n}\n\
                      \n\
                      type Query {\n  \
                      getNote(id: ID!): Note\n  listNotes(board: String!): [Note]\n}\n\
                      \n\
                      type Mutation {\n  \
                      createNote(id: ID!, board: String!, body: String!): Note\n  \
                      claimNote(id: ID!, board: String!, body: String!): Note\n  \
                      bumpNote(id: ID!): Note\n  \
                      deleteNote(id: ID!): Note\n}\n\
                      \n\
                      type Subscription {\n  \
                      onCreateNote(board: String): Note \
                      @aws_subscribe(mutations: [\"createNote\"])\n}\n";

/// The response handler every resolver of this suite shares: a step whose call the data source
/// refused reports that refusal as the field's error, and every other step resolves to what the
/// call answered.
const RESPONSE_HANDLER: &str = "export function response(ctx) {\n  \
                                if (ctx.error) {\n    \
                                util.error(ctx.error.message, ctx.error.type);\n  }\n  \
                                return ctx.result;\n}\n";

#[tokio::test]
async fn each_imported_dynamodb_helper_builds_the_call_its_resolver_makes_against_the_table() {
    let router = emulator();
    let table_arn = create_table(&router).await;
    let role = create_role(&router, "appsync-helper-notes-role", &table_arn).await;
    let (graphql, realtime) = create_api(&router, &role).await;
    // The account the signed call speaks for, which is what `ctx.identity.accountId` reports
    // and what the mutation's request handler writes onto the item it creates.
    let account = engine::emulator_account_id().to_string();
    let subscribed = "ideas";
    let mut subscriber = subscriber(&graphql, &realtime, subscribed).await;

    /// One row: the document a client POSTs and the `data` the endpoint must answer it with.
    struct Row {
        case: &'static str,
        document: &'static str,
        variables: Value,
        data: Value,
    }
    // The rows run in order, each reading or changing what the ones before it wrote: that is
    // what makes a read's answer the write's own rather than a value the endpoint invented.
    let rows = vec![
        Row {
            case: "put through a namespace import, writing the item the payload addresses",
            document: "mutation Write($id: ID!, $board: String!, $body: String!) { \
                       createNote(id: $id, board: $board, body: $body) \
                       { id board body author } }",
            variables: json!({"id": "N-1", "board": subscribed, "body": "first"}),
            data: json!({"createNote": {
                "id": "N-1",
                "board": subscribed,
                "body": "first",
                "author": account,
            }}),
        },
        Row {
            case: "put again, on another board and with a body of its own",
            document: "mutation Write($id: ID!, $board: String!, $body: String!) { \
                       createNote(id: $id, board: $board, body: $body) \
                       { id board body author } }",
            variables: json!({"id": "N-2", "board": "todo", "body": "a longer second body"}),
            data: json!({"createNote": {
                "id": "N-2",
                "board": "todo",
                "body": "a longer second body",
                "author": account,
            }}),
        },
        Row {
            case: "get through a named import, reading back consistently what the put wrote",
            document: "query Read($id: ID!) { getNote(id: $id) { id board body author } }",
            variables: json!({"id": "N-1"}),
            data: json!({"getNote": {
                "id": "N-1",
                "board": subscribed,
                "body": "first",
                "author": account,
            }}),
        },
        Row {
            case: "scan through the default import, filtered to one board",
            document: "query Read($board: String!) { listNotes(board: $board) { id body } }",
            variables: json!({"board": "todo"}),
            data: json!({"listNotes": [{"id": "N-2", "body": "a longer second body"}]}),
        },
        Row {
            case: "update with an increment, counting from zero for an attribute the item \
                   does not carry yet",
            document: "mutation Bump($id: ID!) { bumpNote(id: $id) { id votes } }",
            variables: json!({"id": "N-1"}),
            data: json!({"bumpNote": {"id": "N-1", "votes": 1}}),
        },
        Row {
            case: "update with an increment again, counting from the value the item now holds",
            document: "mutation Bump($id: ID!) { bumpNote(id: $id) { id votes } }",
            variables: json!({"id": "N-1"}),
            data: json!({"bumpNote": {"id": "N-1", "votes": 2}}),
        },
        Row {
            case: "remove, answering the item as it stood",
            document: "mutation Drop($id: ID!) { deleteNote(id: $id) { id body votes } }",
            variables: json!({"id": "N-1"}),
            data: json!({"deleteNote": {"id": "N-1", "body": "first", "votes": 2}}),
        },
        Row {
            case: "get after the remove, which finds no item",
            document: "query Read($id: ID!) { getNote(id: $id) { id } }",
            variables: json!({"id": "N-1"}),
            data: json!({"getNote": null}),
        },
    ];
    for row in &rows {
        let case = row.case;
        let answered = signed_graphql(
            case,
            &graphql,
            &json!({"query": row.document, "variables": row.variables}),
        )
        .await;
        assert_eq!(
            answered.get("data"),
            Some(&row.data),
            "case {case:?}: expected the endpoint to answer {}, got {answered}",
            row.data
        );
    }

    // The mutation the subscription is linked to reached the subscriber, over the socket the
    // signed handshake established — the note on the board that subscriber asked for.
    let delivered = next_frame(&mut subscriber)
        .await
        .unwrap_or_else(|| panic!("the subscriber must receive the mutation it subscribed to"));
    assert_eq!(
        delivered,
        json!({
            "id": "watch-notes",
            "type": "data",
            "payload": {"data": {"onCreateNote": {
                "id": "N-1",
                "board": subscribed,
                "body": "first",
                "author": account,
            }}},
        }),
        "case the delivery of the signed mutation: got {delivered}"
    );

    // What the table itself holds, through DynamoDB's own reader: the item the second put
    // wrote is there as the helper marshalled it, and the one the remove dropped is gone.
    let held = dynamodb(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": "N-2"}}}),
    )
    .await;
    assert_eq!(
        held.pointer("/Item"),
        Some(&json!({
            "id": {"S": "N-2"},
            "board": {"S": "todo"},
            "body": {"S": "a longer second body"},
            "author": {"S": account},
        })),
        "case the item a put wrote, as the table holds it: got {held}"
    );
    let dropped = dynamodb(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": "N-1"}}}),
    )
    .await;
    assert_eq!(
        dropped.get("Item"),
        None,
        "case the item a remove dropped, as the table holds it: got {dropped}"
    );
}

/// The CONDITION a helper's payload states travels as the expression the call is held to: the
/// first write of a key is admitted and the second is refused by the table itself, as the error
/// class and the sentence that service words a failed condition with.
#[tokio::test]
async fn a_condition_a_helper_builds_is_the_one_the_table_holds_the_write_to() {
    let router = emulator();
    let table_arn = create_table(&router).await;
    let role = create_role(&router, "appsync-helper-claims-role", &table_arn).await;
    let (graphql, _) = create_api(&router, &role).await;
    let document = "mutation Claim($id: ID!, $board: String!, $body: String!) { \
                    claimNote(id: $id, board: $board, body: $body) { id body } }";
    let variables = json!({"id": "C-1", "board": "ideas", "body": "first"});

    let claimed = signed_graphql(
        "a conditional put of a key the table holds no item for",
        &graphql,
        &json!({"query": document, "variables": variables}),
    )
    .await;
    assert_eq!(
        claimed.pointer("/data/claimNote"),
        Some(&json!({"id": "C-1", "body": "first"})),
        "case a conditional put the condition admits: got {claimed}"
    );

    let case = "a conditional put of a key the table already holds an item for";
    let (status, _, refused) = try_post_answer(
        case,
        &graphql,
        GRAPHQL_PATH,
        "POST",
        &json!({
            "query": document,
            "variables": json!({"id": "C-1", "board": "ideas", "body": "second"}),
        }),
        &[("authorization", &sigv4_material())],
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the published GraphQL endpoint must answer"));
    assert_eq!(
        (status, refused.pointer("/data/claimNote")),
        (200, Some(&Value::Null)),
        "case {case:?}: a field whose call the table refused is answered 200 with the field \
         resolved to null, got {status} and {refused}"
    );
    let error = refused.pointer("/errors/0").cloned().unwrap_or(Value::Null);
    assert_eq!(
        error.get("errorType"),
        Some(&json!("DynamoDB:ConditionalCheckFailedException")),
        "case {case:?}: the error must name the class the table raised, got {refused}"
    );
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("The conditional request failed"),
        "case {case:?}: the error must carry the sentence that service words it with, got \
         {refused}"
    );
    // And the item is the one the admitted write left: the refused write changed nothing.
    let held = dynamodb(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": "C-1"}}}),
    )
    .await;
    assert_eq!(
        held.pointer("/Item/body"),
        Some(&json!({"S": "first"})),
        "case {case:?}: the refused write must leave the stored item untouched, got {held}"
    );
}

/// A SIGNED real-time subscriber of the API, started on the subscription field for one board.
async fn subscriber(
    graphql: &str,
    realtime: &str,
    board: &str,
) -> crate::graphql_api_realtime_endpoint::ClientSocket {
    let case = "a signed real-time subscriber of an IAM-authenticated API";
    let origin = format!("https://{realtime}");
    let mut socket = handshake(
        case,
        realtime,
        REALTIME_PATH,
        true,
        Some(&signed_auth_object(graphql)),
        &[("origin", origin.as_str())],
    )
    .await
    .unwrap_or_else(|status| {
        panic!("case {case:?}: the signed upgrade must be completed, got {status}")
    });
    let acked = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
    assert_eq!(
        acked.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("connection_ack"),
        "case {case:?}: connection_init must be answered connection_ack, got {acked:?}"
    );
    let started = exchange(
        case,
        &mut socket,
        &started_message(
            "watch-notes",
            "subscription Watch($board: String) { \
             onCreateNote(board: $board) { id board body author } }",
            &json!({"board": board}),
        ),
    )
    .await;
    assert_eq!(
        started.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("start_ack"),
        "case {case:?}: a start on a subscription field of the schema must be acked, got \
         {started:?}"
    );
    socket
}

/// Create the `AWS_IAM` API with [`SCHEMA`], the `AMAZON_DYNAMODB` data source reached under
/// `role_arn`, and one resolver per field — each written against the DynamoDB helper module,
/// reached through the import form its own row names — and answer the two addresses the API
/// publishes.
async fn create_api(router: &MultiServiceRouter, role_arn: &str) -> (String, String) {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": "helper-notes", "authenticationType": "AWS_IAM"})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let address = |key: &str| {
        let published = crate::graphql_api_endpoint_dns_records::published_authority(api, key);
        assert!(
            published.starts_with(&format!("{LOOPBACK_HOST}:")),
            "uris.{key} must be published on an address this emulator serves, got {published:?}"
        );
        published
    };
    let graphql = address("GRAPHQL");
    let realtime = address("REALTIME");
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "AMAZON_DYNAMODB",
            "serviceRoleArn": role_arn,
            "dynamodbConfig": {"tableName": TABLE, "awsRegion": REGION},
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    // Every resolver's code is its own request handler over the shared response handler, and
    // the one that reads a page carries both of its own.
    let mut definitions: Vec<(&str, &str, String)> = RESOLVERS
        .iter()
        .map(|(type_name, field, request_handler)| {
            (
                *type_name,
                *field,
                format!("{request_handler}\n{RESPONSE_HANDLER}"),
            )
        })
        .collect();
    definitions.push((LIST_RESOLVER.0, LIST_RESOLVER.1, LIST_RESOLVER.2.to_string()));
    for (type_name, field, code) in &definitions {
        let (status, resolver) = appsync(
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
            "CreateResolver for {type_name}.{field} must answer 200, got {resolver}"
        );
    }
    (graphql, realtime)
}

/// Every resolver of this suite: the type and the field it is defined on, and the request
/// handler it is defined by — which is where the helper and the import form each case drives
/// are written.
///
/// `util` is reached as the runtime's own global by the handlers that do not import it, and
/// through the utilities module by the one that does, because a resolver written either way is
/// one this runtime runs.
const RESOLVERS: &[(&str, &str, &str)] = &[
    (
        "Mutation",
        "createNote",
        "import { util } from '@aws-appsync/utils';\n\
         import * as ddb from '@aws-appsync/utils/dynamodb';\n\
         \n\
         export function request(ctx) {\n  \
         return ddb.put({\n    \
         key: {id: ctx.args.id},\n    \
         item: {board: ctx.args.board, body: ctx.args.body, author: ctx.identity.accountId},\n  \
         });\n}\n",
    ),
    (
        "Mutation",
        "claimNote",
        "import * as ddb from '@aws-appsync/utils/dynamodb';\n\
         \n\
         export function request(ctx) {\n  \
         return ddb.put({\n    \
         key: {id: ctx.args.id},\n    \
         item: {board: ctx.args.board, body: ctx.args.body},\n    \
         condition: {id: {attributeExists: false}},\n  \
         });\n}\n",
    ),
    (
        "Query",
        "getNote",
        "import { get } from '@aws-appsync/utils/dynamodb';\n\
         \n\
         export function request(ctx) {\n  \
         return get({key: {id: ctx.args.id}, consistentRead: true});\n}\n",
    ),
    (
        "Mutation",
        "bumpNote",
        "import { operations, update } from '@aws-appsync/utils/dynamodb';\n\
         \n\
         export function request(ctx) {\n  \
         return update({\n    \
         key: {id: ctx.args.id},\n    \
         update: {votes: operations.increment(1)},\n  \
         });\n}\n",
    ),
    (
        "Mutation",
        "deleteNote",
        "import { remove } from '@aws-appsync/utils/dynamodb';\n\
         \n\
         export function request(ctx) {\n  \
         return remove({key: {id: ctx.args.id}});\n}\n",
    ),
];

/// The resolver that reads a page of the table, whose response handler answers the `items` of
/// the selection rather than the page itself — so it is defined beside the others rather than
/// among them.
const LIST_RESOLVER: (&str, &str, &str) = (
    "Query",
    "listNotes",
    "import ddb from '@aws-appsync/utils/dynamodb';\n\
     \n\
     export function request(ctx) {\n  \
     return ddb.scan({filter: {board: {eq: ctx.args.board}}});\n}\n\
     \n\
     export function response(ctx) {\n  \
     if (ctx.error) {\n    \
     util.error(ctx.error.message, ctx.error.type);\n  }\n  \
     return ctx.result.items;\n}\n",
);
