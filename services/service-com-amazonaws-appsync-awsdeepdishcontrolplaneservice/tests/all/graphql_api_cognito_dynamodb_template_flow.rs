// SPDX-License-Identifier: CC0-1.0

//! An `AMAZON_COGNITO_USER_POOLS` GraphQL API whose fields are resolved by VTL MAPPING
//! TEMPLATES over an `AMAZON_DYNAMODB` data source, driven over the two sockets the API
//! publishes.
//!
//! Three rules, each one level above the recorded case:
//!
//! > 1. A resolver is defined EITHER by `code` with a `runtime` or by its mapping templates,
//! >    and both are definitions AppSync RUNS: a field whose resolver carries
//! >    `requestMappingTemplate` / `responseMappingTemplate` is answered by rendering the
//! >    request template against the field's own `$context` — `$ctx.args` and, for a caller
//! >    the API's authorization mode admitted, `$ctx.identity` — into the request object the
//! >    data source is called with, making that call, and rendering the response template
//! >    against its result. Over an `AMAZON_DYNAMODB` data source the request object's
//! >    `operation` names the DynamoDB call to make against the table the data source was
//! >    configured with, so a successful field is `200` with the value under `data` and NO
//! >    `errors` member, and what the call wrote is what that service's own readers hold.
//! > 2. A call the data source cannot make is an error OF THE FIELD: the field resolves to
//! >    `null`, the response is still `200`, and the `errors` entry names the field's path and
//! >    carries the failure's own class and sentence — the IAM refusal naming the role session
//! >    the call is made under where the permissions in force withhold the action, and the
//! >    class the called service's model declares where that service refused the call.
//! > 3. A call on such an API is decided by the TOKEN it presents before any resolver runs:
//! >    only a token the configured pool really signed is served, and anything else is refused
//! >    `401` `UnauthorizedException` with no call made. An admitted mutation is PUBLISHED to
//! >    the subscriptions its schema links to that mutation field, filtered by each
//! >    subscription's own arguments.
//!
//! The rows span what those rules generalise over: the DynamoDB operation a request template
//! names (`PutItem`, `GetItem`, `Query` over a secondary index, `UpdateItem` under a
//! condition, `DeleteItem`, and `BatchPutItem` through a pipeline function), the definition
//! form (a unit resolver's two templates, and a pipeline whose function carries its own), what
//! makes a call one the data source cannot make (a role that withholds the action, a table
//! that is not there, a condition that does not hold), what makes a presented value no token
//! the API accepts (a rewritten signature, a token of another pool, no token at all), and
//! whether a subscription's arguments match what the mutation published.
//!
//! Nothing here is asserted off this service's own store: the pool is one Amazon Cognito user
//! pools created and the tokens are ones it minted, the service role and its policy are IAM's,
//! and every write is read back through Amazon DynamoDB's own reader — so a call that was
//! never made cannot pass.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_realtime_endpoint::{exchange, handshake, next_frame, started_message};

/// The Region every call of this suite is signed for, which is the Region the API, the pool
/// and the table are created in.
const REGION: &str = "us-west-2";

/// The path both of the API's published endpoints are served at.
const GRAPHQL_PATH: &str = "/graphql";

/// The table the data sources of this suite resolve against, the attributes it is keyed and
/// indexed by, and the secondary index a `Query` row selects over.
///
/// The name carries no `-`, so a template can walk a batch result's own per-table member by
/// name (`$ctx.result.data.<table>`).
const TABLE: &str = "vtlItems";
const KEY_ATTRIBUTE: &str = "id";
const CATEGORY_ATTRIBUTE: &str = "category";
const INDEX: &str = "byCategory";

/// A table name no `CreateTable` of this suite ever used: the data source that names it can
/// make no call against anything.
const GHOST_TABLE: &str = "vtlItemsNotThere";

/// The three `AMAZON_DYNAMODB` data sources of this suite: the one reached under a role
/// allowed every action on the table, the one reached under a role allowed only to READ it,
/// and the one naming [`GHOST_TABLE`] under a role allowed every table.
const DATA_SOURCE: &str = "Items";
const READ_ONLY_DATA_SOURCE: &str = "ItemsReadOnly";
const GHOST_DATA_SOURCE: &str = "ItemsGhost";

/// The service principal a data source's service role is assumed by.
const APPSYNC_PRINCIPAL: &str = "appsync.amazonaws.com";

/// The `functionVersion` a function defined in templates carries: "when using VTL and mapping
/// templates, the `functionVersion` is required" and "the only supported value is 2018-05-29"
/// (`CreateFunction$functionVersion`).
const FUNCTION_VERSION: &str = "2018-05-29";

/// A password satisfying a default pool's password policy. Written for this test and used
/// nowhere else: the pools it is set in live only in this test's own in-memory engine.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The user every token of this suite speaks for.
const USERNAME: &str = "alice";

/// The refusal an unauthorized call on this data plane earns. The service's own copies are
/// `service::event_api::UNAUTHORIZED_ERROR_TYPE` and `…_MESSAGE`.
const UNAUTHORIZED_STATUS: u16 = 401;
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";

/// How the `errorType` of an error the CALLED service answered is prefixed: AppSync reports a
/// downstream refusal as the service it came from and the class of the exception its client
/// raised. The service's own copy is
/// `service::graphql_api::dynamodb::ANSWERED_ERROR_TYPE_PREFIX`.
const ANSWERED_ERROR_TYPE_PREFIX: &str = "DynamoDB:";

/// How long a case waits for a state the clock owes, and how often it looks again. Generous:
/// what is under test is what the endpoints answer, never how fast the host gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// Where a template names the data source's own table, which [`template`] substitutes
/// [`TABLE`] into — the templates are written as the text a client submits, so the name
/// cannot be interpolated by `format!` without escaping every brace of the request object.
const TABLE_PLACEHOLDER: &str = "<table>";

/// The schema every API of this suite is created with: the item type, the fields the resolvers
/// are attached to, and the subscription its own `@aws_subscribe` links to the write.
const SCHEMA: &str = "type Item {\n  id: ID!\n  category: String\n  note: String\n  \
                      owner: String\n}\n\
                      \n\
                      type Query {\n  getItem(id: ID!): Item\n  \
                      listByCategory(category: String!): [Item]\n}\n\
                      \n\
                      type Mutation {\n  \
                      putItem(id: ID!, category: String, note: String): Item\n  \
                      updateItemConditional(id: ID!, note: String!, expected: String!): Item\n  \
                      deleteItem(id: ID!): Item\n  \
                      batchPutItems(ids: [String!]!, category: String): [Item]\n  \
                      deniedPut(id: ID!): Item\n  ghostPut(id: ID!): Item\n}\n\
                      \n\
                      type Subscription {\n  \
                      onPutItem(category: String): Item @aws_subscribe(mutations: [\"putItem\"])\n}\n";

/// Every request object below states `"version": "2018-05-29"`, which is the version an
/// `AMAZON_DYNAMODB` request object of a mapping template carries, and addresses the item by
/// the key attribute the table is keyed on.
const PUT_REQUEST: &str = r#"{
  "version": "2018-05-29",
  "operation": "PutItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "attributeValues": $util.dynamodb.toMapValuesJson({
    "category": $ctx.args.category,
    "note": $ctx.args.note,
    "owner": $ctx.identity.username
  })
}"#;

/// The write of a data source this suite expects to be refused: it carries no argument beyond
/// the key, so what the row observes is the refusal and nothing of the item.
const REFUSED_PUT_REQUEST: &str = r#"{
  "version": "2018-05-29",
  "operation": "PutItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "attributeValues": $util.dynamodb.toMapValuesJson({"note": "refused"})
}"#;

const GET_REQUEST: &str = r#"{
  "version": "2018-05-29",
  "operation": "GetItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)}
}"#;

const QUERY_REQUEST: &str = r#"{
  "version": "2018-05-29",
  "operation": "Query",
  "index": "byCategory",
  "query": {
    "expression": "category = :category",
    "expressionValues": {":category": $util.dynamodb.toDynamoDBJson($ctx.args.category)}
  }
}"#;

const UPDATE_REQUEST: &str = r#"{
  "version": "2018-05-29",
  "operation": "UpdateItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "update": {
    "expression": "SET note = :note",
    "expressionValues": {":note": $util.dynamodb.toDynamoDBJson($ctx.args.note)}
  },
  "condition": {
    "expression": "note = :expected",
    "expressionValues": {":expected": $util.dynamodb.toDynamoDBJson($ctx.args.expected)}
  }
}"#;

const DELETE_REQUEST: &str = r#"{
  "version": "2018-05-29",
  "operation": "DeleteItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)}
}"#;

/// The pipeline FUNCTION's own request template: it builds one marshalled item per id the
/// field was given and writes them all in one batch.
const BATCH_REQUEST: &str = r#"#set($entries = [])
#foreach($id in $ctx.args.ids)
$util.qr($entries.add($util.dynamodb.toMapValues({"id": $id, "category": $ctx.args.category, "note": "batched"})))
#end
{
  "version": "2018-05-29",
  "operation": "BatchPutItem",
  "tables": {
    "<table>": $util.toJson($entries)
  }
}"#;

/// The response templates: the result as it stands, the items of a selection, the written
/// entries of one table of a batch, and what the previous step of a pipeline came to.
const RESULT_RESPONSE: &str = "$util.toJson($ctx.result)";
const ITEMS_RESPONSE: &str = "$util.toJson($ctx.result.items)";
const BATCH_RESPONSE: &str = "$util.toJson($ctx.result.data.<table>)";
const PREVIOUS_RESPONSE: &str = "$util.toJson($ctx.prev.result)";

/// A pipeline resolver's own request half, which contributes nothing of its own.
const EMPTY_REQUEST: &str = "{}";

/// `text` with [`TABLE_PLACEHOLDER`] replaced by the table the data sources name.
fn template(text: &str) -> String {
    text.replace(TABLE_PLACEHOLDER, TABLE)
}

/// One created GraphQL API: its id and the addresses its two endpoints are served on.
struct GraphqlApi {
    graphql: String,
    realtime: String,
}

/// Everything a case drives: the router the services are mounted on, the API, and an identity
/// token the API's own pool minted for [`USERNAME`].
struct Fixture {
    router: MultiServiceRouter,
    api: GraphqlApi,
    pool_id: String,
    token: String,
}

/// A router with the four services the recorded topology spans, over ONE engine: Amazon
/// Cognito user pools owns the pool whose tokens the API accepts, IAM and STS the data
/// sources' service roles and their policies, and Amazon DynamoDB the table every resolver
/// resolves against.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // What a case pins is what the endpoints ANSWER; signature verification is the emulator's
    // own front door, not a service's.
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(EmulatedService::new(engine));
    router
}

/// The whole topology one case drives, built through the owning services' own operations.
async fn fixture(name: &str) -> Fixture {
    let router = emulator();
    let table_arn = create_table(&router).await;
    let roles = create_roles(&router, name, &table_arn).await;
    let (pool_id, client_id) = pool_with_client(&router, &format!("{name}-pool")).await;
    create_user(&router, &pool_id, USERNAME).await;
    let token = id_token(&sign_in(&router, &pool_id, &client_id, USERNAME).await);
    let api = create_api(&router, name, &pool_id, &roles).await;
    Fixture {
        router,
        api,
        pool_id,
        token,
    }
}

#[tokio::test]
async fn a_template_resolver_over_a_dynamodb_data_source_makes_the_call_it_renders() {
    let fixture = fixture("flow").await;

    /// One row: the document a client POSTs, the variables it carries and the value the field
    /// must take under `data`.
    struct Row {
        case: &'static str,
        document: &'static str,
        variables: Value,
        expected: Value,
    }
    // The rows differ along the DynamoDB operation the request template names and along the
    // definition form the field carries (a unit resolver's templates, a pipeline function's).
    let rows = vec![
        Row {
            case: "a PutItem template writes the item it renders",
            document: "mutation Write($id: ID!, $category: String, $note: String) { \
                       putItem(id: $id, category: $category, note: $note) \
                       { id category note owner } }",
            variables: json!({"id": "I-1", "category": "news", "note": "first"}),
            expected: json!({
                "putItem": {
                    "id": "I-1", "category": "news", "note": "first", "owner": USERNAME,
                }
            }),
        },
        Row {
            case: "a GetItem template reads that item back",
            document: "query Read($id: ID!) { getItem(id: $id) { id category note owner } }",
            variables: json!({"id": "I-1"}),
            expected: json!({
                "getItem": {
                    "id": "I-1", "category": "news", "note": "first", "owner": USERNAME,
                }
            }),
        },
        Row {
            case: "a second write, under another category",
            document: "mutation Write($id: ID!, $category: String, $note: String) { \
                       putItem(id: $id, category: $category, note: $note) { id category } }",
            variables: json!({"id": "I-2", "category": "sports", "note": "other"}),
            expected: json!({"putItem": {"id": "I-2", "category": "sports"}}),
        },
        Row {
            case: "a Query template selects over the secondary index it names",
            document: "query Listed($category: String!) { \
                       listByCategory(category: $category) { id category note } }",
            variables: json!({"category": "news"}),
            expected: json!({
                "listByCategory": [{"id": "I-1", "category": "news", "note": "first"}]
            }),
        },
        Row {
            case: "an UpdateItem template under a condition that holds applies its update",
            document: "mutation Change($id: ID!, $note: String!, $expected: String!) { \
                       updateItemConditional(id: $id, note: $note, expected: $expected) \
                       { id category note owner } }",
            variables: json!({"id": "I-1", "note": "second", "expected": "first"}),
            expected: json!({
                "updateItemConditional": {
                    "id": "I-1", "category": "news", "note": "second", "owner": USERNAME,
                }
            }),
        },
        Row {
            case: "a DeleteItem template answers the item it removed",
            document: "mutation Remove($id: ID!) { deleteItem(id: $id) { id note } }",
            variables: json!({"id": "I-1"}),
            expected: json!({"deleteItem": {"id": "I-1", "note": "second"}}),
        },
        Row {
            case: "a GetItem template of a key the table holds no item for",
            document: "query Read($id: ID!) { getItem(id: $id) { id note } }",
            variables: json!({"id": "I-1"}),
            expected: json!({"getItem": Value::Null}),
        },
        Row {
            case: "a pipeline function's BatchPutItem template writes every item it renders",
            document: "mutation Batch($ids: [String!]!, $category: String) { \
                       batchPutItems(ids: $ids, category: $category) { id category note } }",
            variables: json!({"ids": ["B-1", "B-2"], "category": "news"}),
            expected: json!({
                "batchPutItems": [
                    {"id": "B-1", "category": "news", "note": "batched"},
                    {"id": "B-2", "category": "news", "note": "batched"},
                ]
            }),
        },
    ];
    for row in &rows {
        let case = row.case;
        let (status, answered) = post_graphql(
            case,
            &fixture.api,
            &json!({"query": row.document, "variables": row.variables}),
            Some(&fixture.token),
        )
        .await;
        assert_eq!(
            (status, answered.get("errors")),
            (200, None),
            "case {case:?}: a field whose resolver is defined by templates is answered by \
             running them, got {status} and {answered}"
        );
        assert_eq!(
            answered.get("data"),
            Some(&row.expected),
            "case {case:?}: expected {} under data, got {answered}",
            row.expected
        );
    }

    // What the table itself holds, through DynamoDB's own reader: every write really landed
    // there, and the delete really removed what it addressed.
    for (case, id, expected) in [
        (
            "the item the second write wrote",
            "I-2",
            json!({"id": {"S": "I-2"}, "category": {"S": "sports"}, "note": {"S": "other"},
                   "owner": {"S": USERNAME}}),
        ),
        (
            "the first item the batch wrote",
            "B-1",
            json!({"id": {"S": "B-1"}, "category": {"S": "news"}, "note": {"S": "batched"}}),
        ),
        (
            "the second item the batch wrote",
            "B-2",
            json!({"id": {"S": "B-2"}, "category": {"S": "news"}, "note": {"S": "batched"}}),
        ),
    ] {
        let held = held_item(&fixture.router, id).await;
        assert_eq!(
            held,
            Some(expected.clone()),
            "case {case:?}: expected the table to hold {expected}, got {held:?}"
        );
    }
    let removed = held_item(&fixture.router, "I-1").await;
    assert_eq!(
        removed, None,
        "case \"the item the delete removed\": expected the table to hold no item for it, got \
         {removed:?}"
    );
}

#[tokio::test]
async fn a_dynamodb_call_the_data_source_cannot_make_is_an_error_of_the_field() {
    let fixture = fixture("refusals").await;

    // The item the conditional row addresses, written first through the data source that may
    // write it.
    let (status, written) = post_graphql(
        "the item a condition is weighed against",
        &fixture.api,
        &json!({
            "query": "mutation Write($id: ID!, $category: String, $note: String) { \
                      putItem(id: $id, category: $category, note: $note) { id note } }",
            "variables": {"id": "C-1", "category": "news", "note": "first"},
        }),
        Some(&fixture.token),
    )
    .await;
    assert_eq!(
        (status, written.get("errors")),
        (200, None),
        "setup: the item a condition is weighed against must be written, got {status} and \
         {written}"
    );

    /// One row: the document a client POSTs, the field it selects, the `errorType` the error
    /// of that field must carry, and a phrase its message must state.
    struct Row {
        case: &'static str,
        document: &'static str,
        variables: Value,
        field: &'static str,
        error_type: String,
        states: String,
    }
    // The rows differ along WHAT makes the call one the data source cannot make: the
    // permissions in force on the role it is made under, a table that is not there, and a
    // condition the stored item does not satisfy.
    let rows = vec![
        Row {
            case: "a data source whose role withholds the action",
            document: "mutation Write($id: ID!) { deniedPut(id: $id) { id note } }",
            variables: json!({"id": "D-1"}),
            field: "deniedPut",
            // The permissions are weighed before the call, so there is no answer of the
            // called service to classify: the refusal is reported under that service's base
            // exception, with the IAM sentence as its message.
            error_type: format!("{ANSWERED_ERROR_TYPE_PREFIX}DynamoDbException"),
            states: "is not authorized to perform: dynamodb:PutItem".to_string(),
        },
        Row {
            case: "a data source naming a table that is not there",
            document: "mutation Write($id: ID!) { ghostPut(id: $id) { id note } }",
            variables: json!({"id": "G-1"}),
            field: "ghostPut",
            error_type: format!("{ANSWERED_ERROR_TYPE_PREFIX}ResourceNotFoundException"),
            // What that service answers for a write against a table it does not hold, as it
            // words it: the class is its own modelled one and the sentence is its own.
            states: "Requested resource not found".to_string(),
        },
        Row {
            case: "an UpdateItem whose condition the stored item does not satisfy",
            document: "mutation Change($id: ID!, $note: String!, $expected: String!) { \
                       updateItemConditional(id: $id, note: $note, expected: $expected) \
                       { id note } }",
            variables: json!({"id": "C-1", "note": "second", "expected": "another"}),
            field: "updateItemConditional",
            error_type: format!("{ANSWERED_ERROR_TYPE_PREFIX}ConditionalCheckFailedException"),
            states: "The conditional request failed".to_string(),
        },
    ];
    for row in &rows {
        let case = row.case;
        let (status, answered) = post_graphql(
            case,
            &fixture.api,
            &json!({"query": row.document, "variables": row.variables}),
            Some(&fixture.token),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a field whose call could not be made is still answered 200, got \
             {status} and {answered}"
        );
        assert_eq!(
            answered.pointer(&format!("/data/{}", row.field)),
            Some(&Value::Null),
            "case {case:?}: the field must resolve to null, got {answered}"
        );
        let error = answered
            .pointer("/errors/0")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            error.get("path"),
            Some(&json!([row.field])),
            "case {case:?}: the error must name the path of the field, got {answered}"
        );
        assert_eq!(
            error.get("errorType").and_then(Value::as_str),
            Some(row.error_type.as_str()),
            "case {case:?}: expected the error class {:?}, got {error}",
            row.error_type
        );
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(&row.states),
            "case {case:?}: expected a message stating {:?}, got {message:?}",
            row.states
        );
    }

    // Neither refused write reached a table, and the item the condition was weighed against
    // is as it was.
    for (case, id) in [
        ("the write the role withheld", "D-1"),
        ("the write against the table that is not there", "G-1"),
    ] {
        let held = held_item(&fixture.router, id).await;
        assert_eq!(
            held, None,
            "case {case:?}: a call that was refused writes nothing, got {held:?}"
        );
    }
    let unchanged = held_item(&fixture.router, "C-1").await;
    assert_eq!(
        unchanged.as_ref().and_then(|item| item.pointer("/note/S")),
        Some(&json!("first")),
        "case \"the item a failed condition addressed\": it must be as it was, got \
         {unchanged:?}"
    );
}

#[tokio::test]
async fn a_user_pool_api_refuses_a_token_its_pool_did_not_sign_before_any_resolver_runs() {
    let fixture = fixture("tokens").await;
    // A SECOND pool, unrelated to the API's: its tokens are ones a real pool really signed,
    // and ones this API accepts from nobody.
    let (other_pool, other_client) = pool_with_client(&fixture.router, "foreign-pool").await;
    create_user(&fixture.router, &other_pool, USERNAME).await;
    let foreign = id_token(&sign_in(&fixture.router, &other_pool, &other_client, USERNAME).await);
    assert_ne!(
        other_pool, fixture.pool_id,
        "setup: the second pool must be another pool than the API's"
    );

    /// One row: the id its write addresses, the value it presents in the request's
    /// `authorization` field, and whether the call is served.
    struct Row {
        case: &'static str,
        id: &'static str,
        presented: Option<String>,
        served: bool,
    }
    // The rows differ along what makes the presented value no token the API accepts: a
    // rewritten signature, a token of another pool, and no token at all.
    let rows = vec![
        Row {
            case: "a token the configured pool signed",
            id: "T-1",
            presented: Some(fixture.token.clone()),
            served: true,
        },
        Row {
            case: "the same token with its signature rewritten",
            id: "T-2",
            presented: Some(with_rewritten_signature(&fixture.token)),
            served: false,
        },
        Row {
            case: "a token another user pool signed",
            id: "T-3",
            presented: Some(foreign.clone()),
            served: false,
        },
        Row {
            case: "no token at all",
            id: "T-4",
            presented: None,
            served: false,
        },
    ];
    for row in &rows {
        let case = row.case;
        let document = json!({
            "query": "mutation Write($id: ID!, $category: String, $note: String) { \
                      putItem(id: $id, category: $category, note: $note) { id note owner } }",
            "variables": {"id": row.id, "category": "news", "note": "written"},
        });
        let (status, answered) =
            post_graphql(case, &fixture.api, &document, row.presented.as_deref()).await;
        if row.served {
            assert_eq!(
                (status, answered.get("errors")),
                (200, None),
                "case {case:?}: a token the pool signed is served, got {status} and {answered}"
            );
            assert_eq!(
                answered.pointer("/data/putItem/owner").and_then(Value::as_str),
                Some(USERNAME),
                "case {case:?}: the admitted token's own claims are the identity the template \
                 wrote, got {answered}"
            );
        } else {
            assert_eq!(
                status, UNAUTHORIZED_STATUS,
                "case {case:?}: expected {UNAUTHORIZED_STATUS}, got {status} and {answered}"
            );
            assert_eq!(
                answered
                    .pointer("/errors/0/errorType")
                    .and_then(Value::as_str),
                Some(UNAUTHORIZED_ERROR_TYPE),
                "case {case:?}: expected the refusal of an unauthorized call, got {answered}"
            );
            assert_eq!(
                answered.get("data"),
                None,
                "case {case:?}: a refused call resolves no field at all, got {answered}"
            );
        }
        // Whatever the answer said, what the table holds decides whether a resolver ran.
        let held = held_item(&fixture.router, row.id).await;
        assert_eq!(
            held.is_some(),
            row.served,
            "case {case:?}: expected the table to hold the written item: {}, got {held:?}",
            row.served
        );
    }
}

#[tokio::test]
async fn a_mutation_resolved_by_templates_is_published_to_the_subscription_it_is_linked_to() {
    let fixture = fixture("delivery").await;
    let case = "a subscriber of a user-pool API on the mutation's own subscription";

    // The subscriber dials the address the API published for its REALTIME endpoint, carrying
    // the pool's token in the handshake's base64url `header` object, and starts a subscription
    // whose argument selects one category.
    let mut subscriber = handshake(
        case,
        &fixture.api.realtime,
        GRAPHQL_PATH,
        true,
        Some(&json!({"host": fixture.api.realtime, "Authorization": fixture.token})),
        &[],
    )
    .await
    .unwrap_or_else(|status| panic!("case {case:?}: the handshake must complete, got {status}"));
    let acked = exchange(case, &mut subscriber, &json!({"type": "connection_init"})).await;
    assert_eq!(
        acked.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("connection_ack"),
        "case {case:?}: connection_init must be answered connection_ack, got {acked:?}"
    );
    let started = exchange(
        case,
        &mut subscriber,
        &started_message(
            "watch-news",
            "subscription Watch($category: String) { \
             onPutItem(category: $category) { id category note owner } }",
            &json!({"category": "news"}),
        ),
    )
    .await;
    assert_eq!(
        started.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("start_ack"),
        "case {case:?}: a start on a subscription field of the schema must be acked, got \
         {started:?}"
    );

    // The mutation, resolved by its templates against the table, is published to it.
    // The mutation selects every member the subscriber does: AppSync publishes the mutation's
    // own selection, so a member the mutation did not select is published for nobody.
    let write = |id: &str, category: &str| {
        json!({
            "query": "mutation Write($id: ID!, $category: String, $note: String) { \
                      putItem(id: $id, category: $category, note: $note) \
                      { id category note owner } }",
            "variables": {"id": id, "category": category, "note": "published"},
        })
    };
    let (status, answered) = post_graphql(
        case,
        &fixture.api,
        &write("S-1", "news"),
        Some(&fixture.token),
    )
    .await;
    assert_eq!(
        (status, answered.get("errors")),
        (200, None),
        "case {case:?}: the mutation must be resolved by its templates, got {status} and \
         {answered}"
    );
    let delivered = next_frame(&mut subscriber)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the subscriber must receive the mutation"));
    assert_eq!(
        delivered,
        json!({
            "id": "watch-news",
            "type": "data",
            "payload": {"data": {"onPutItem": {
                "id": "S-1", "category": "news", "note": "published", "owner": USERNAME,
            }}},
        }),
        "case {case:?}: expected the written item under the subscription field, got {delivered}"
    );

    // A write of another category does not match the subscription's own argument, so nothing
    // further is delivered on it — while the write itself is served.
    let case = "a mutation whose published item the subscription's argument does not select";
    let (status, answered) = post_graphql(
        case,
        &fixture.api,
        &write("S-2", "sports"),
        Some(&fixture.token),
    )
    .await;
    assert_eq!(
        (status, answered.get("errors")),
        (200, None),
        "case {case:?}: the mutation must still be served, got {status} and {answered}"
    );
    let further = next_frame(&mut subscriber).await;
    assert_eq!(
        further, None,
        "case {case:?}: nothing may be delivered on that subscription, got {further:?}"
    );
    let held = held_item(&fixture.router, "S-2").await;
    assert!(
        held.is_some(),
        "case {case:?}: the write itself must have reached the table, got {held:?}"
    );
}

/// POST one GraphQL document to the address the API published, presenting `presented` in the
/// request's `authorization` field — or nothing at all — and answer the status and document.
async fn post_graphql(
    case: &str,
    api: &GraphqlApi,
    document: &Value,
    presented: Option<&str>,
) -> (u16, Value) {
    let fields: Vec<(&str, &str)> = match presented {
        Some(value) => vec![("authorization", value)],
        None => Vec::new(),
    };
    let (status, _, answered) =
        try_post_answer(case, &api.graphql, GRAPHQL_PATH, "POST", document, &fields)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the published GraphQL endpoint {:?} must answer this \
                     request",
                    api.graphql
                )
            });
    (status, answered)
}

/// The item the table holds under `id`, as DynamoDB's own reader reports it, or `None` when it
/// holds none.
async fn held_item(router: &MultiServiceRouter, id: &str) -> Option<Value> {
    let read = dynamodb(
        router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": id}}}),
    )
    .await;
    read.get("Item").filter(|item| !item.is_null()).cloned()
}

/// One Amazon DynamoDB call over its own wire (`awsJson1_0`, `POST /` with `X-Amz-Target`),
/// asserting it was served.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/dynamodb/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "setup: DynamoDB {operation} must answer 200, got {status} and {answered}"
    );
    answered
}

/// One IAM call over its own wire (`awsQuery`, a form-encoded body), answering the XML it was
/// served.
async fn iam(router: &MultiServiceRouter, body: String) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.clone()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: iam {body} must answer 200, got {status} and {text}"
    );
    text
}

/// One Amazon Cognito user pools call that must succeed (`awsJson1_1`, `POST /` with
/// `X-Amz-Target`).
async fn cognito(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cognito-idp.{REGION}.amazonaws.com/"))
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/cognito-idp/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "setup: cognito-idp {operation} must answer 200, got {status} {parsed}"
    );
    parsed
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One user pool with one app client that admits the administrative password flow, as
/// (`UserPoolId`, `ClientId`).
async fn pool_with_client(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let pool = cognito(router, "CreateUserPool", json!({"PoolName": name})).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an Id, got {pool}"))
        .to_string();
    let client = cognito(
        router,
        "CreateUserPoolClient",
        json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{name}-client"),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a ClientId, got {client}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` in `user_pool_id` with a permanent password, so a sign-in can mint its
/// tokens.
async fn create_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    cognito(
        router,
        "AdminCreateUser",
        json!({"UserPoolId": user_pool_id, "Username": username}),
    )
    .await;
    cognito(
        router,
        "AdminSetUserPassword",
        json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// Sign `username` in and answer its `AuthenticationResult` — the tokens the pool minted.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Value {
    let signed_in = cognito(
        router,
        "AdminInitiateAuth",
        json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": {"USERNAME": username, "PASSWORD": GOOD_PASSWORD},
        }),
    )
    .await;
    signed_in["AuthenticationResult"].clone()
}

/// The identity token of a sign-in.
fn id_token(result: &Value) -> String {
    result["IdToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in must mint an IdToken, got {result}"))
        .to_string()
}

/// `token` with its SIGNATURE rewritten: the same header and claim set under a signature the
/// issuer did not produce.
fn with_rewritten_signature(token: &str) -> String {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted token must be a compact JWS of three segments, got {token}"
    );
    let flipped: String = segments[2]
        .chars()
        .map(|character| match character {
            'a' => 'b',
            _ => 'a',
        })
        .collect();
    format!("{}.{}.{}", segments[0], segments[1], flipped)
}

/// Create the table the data sources resolve against — keyed on [`KEY_ATTRIBUTE`], with a
/// secondary index on [`CATEGORY_ATTRIBUTE`] — through DynamoDB's own API, wait until that
/// service first reports it and its index `ACTIVE`, and answer the ARN it holds it under.
async fn create_table(router: &MultiServiceRouter) -> String {
    let created = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"},
                {"AttributeName": CATEGORY_ATTRIBUTE, "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": INDEX,
                "KeySchema": [{"AttributeName": CATEGORY_ATTRIBUTE, "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let arn = created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "setup: CreateTable must report the ARN the table is held under, got {created}"
    );
    // The wait is on the CONDITION the service itself reports, so nothing here depends on how
    // fast the host runs.
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = dynamodb(router, "DescribeTable", json!({"TableName": TABLE})).await;
        let table = &described["Table"];
        let indexes_active = table["GlobalSecondaryIndexes"]
            .as_array()
            .is_some_and(|indexes| {
                indexes
                    .iter()
                    .all(|index| index["IndexStatus"].as_str() == Some("ACTIVE"))
            });
        if table["TableStatus"].as_str() == Some("ACTIVE") && indexes_active {
            return arn;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "setup: the table and its index must become ACTIVE, last read {described}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// The three service roles the data sources are reached under: every action on the table, only
/// the read of it, and every action on every table.
struct Roles {
    writer: String,
    reader: String,
    any_table: String,
}

/// Create those roles — each trusted by AppSync — and answer their ARNs.
///
/// Every document is written compactly and free of the characters an
/// `application/x-www-form-urlencoded` body treats specially (`&`, `=`, `+`, `%`, space), so
/// none needs encoding.
async fn create_roles(router: &MultiServiceRouter, name: &str, table_arn: &str) -> Roles {
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{APPSYNC_PRINCIPAL}\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    );
    let mut created = Vec::new();
    for (suffix, actions, resource) in [
        ("writer", "\"dynamodb:*\"", table_arn),
        ("reader", "\"dynamodb:GetItem\"", table_arn),
        ("any", "\"dynamodb:*\"", "*"),
    ] {
        let role_name = format!("appsync-{name}-{suffix}");
        let answered = iam(
            router,
            format!(
                "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
                 &AssumeRolePolicyDocument={trust}"
            ),
        )
        .await;
        let arn = xml_element(&answered, "Arn").unwrap_or_else(|| {
            panic!("setup: CreateRole({role_name}) reports no <Arn>: {answered}")
        });
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Action\":[{actions}],\"Resource\":\"{resource}\"}}]}}"
        );
        iam(
            router,
            format!(
                "Action=PutRolePolicy&Version=2010-05-08&RoleName={role_name}\
                 &PolicyName=items&PolicyDocument={document}"
            ),
        )
        .await;
        created.push(arn);
    }
    Roles {
        writer: created[0].clone(),
        reader: created[1].clone(),
        any_table: created[2].clone(),
    }
}

/// Create the `AMAZON_COGNITO_USER_POOLS` API over `user_pool_id` with [`SCHEMA`], the three
/// `AMAZON_DYNAMODB` data sources and one template-defined resolver per field, and answer the
/// two addresses it publishes.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    user_pool_id: &str,
    roles: &Roles,
) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("cognito-dynamodb-{name}"),
            "authenticationType": "AMAZON_COGNITO_USER_POOLS",
            "userPoolConfig": {
                "userPoolId": user_pool_id,
                "awsRegion": REGION,
                "defaultAction": "ALLOW",
            },
        })),
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
    for (source, role, table) in [
        (DATA_SOURCE, &roles.writer, TABLE),
        (READ_ONLY_DATA_SOURCE, &roles.reader, TABLE),
        (GHOST_DATA_SOURCE, &roles.any_table, GHOST_TABLE),
    ] {
        let (status, answered) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(json!({
                "name": source,
                "type": "AMAZON_DYNAMODB",
                "serviceRoleArn": role,
                "dynamodbConfig": {"tableName": table, "awsRegion": REGION},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateDataSource {source} must answer 200, got {answered}"
        );
    }
    create_resolvers(router, &api_id).await;
    GraphqlApi { graphql, realtime }
}

/// Attach one resolver per field of [`SCHEMA`]: a UNIT resolver defined by its two mapping
/// templates for every field, and a PIPELINE resolver whose one function is defined by
/// templates of its own for the batch.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str) {
    for (type_name, field, source, request, response) in [
        ("Mutation", "putItem", DATA_SOURCE, PUT_REQUEST, RESULT_RESPONSE),
        ("Query", "getItem", DATA_SOURCE, GET_REQUEST, RESULT_RESPONSE),
        (
            "Query",
            "listByCategory",
            DATA_SOURCE,
            QUERY_REQUEST,
            ITEMS_RESPONSE,
        ),
        (
            "Mutation",
            "updateItemConditional",
            DATA_SOURCE,
            UPDATE_REQUEST,
            RESULT_RESPONSE,
        ),
        (
            "Mutation",
            "deleteItem",
            DATA_SOURCE,
            DELETE_REQUEST,
            RESULT_RESPONSE,
        ),
        (
            "Mutation",
            "deniedPut",
            READ_ONLY_DATA_SOURCE,
            REFUSED_PUT_REQUEST,
            RESULT_RESPONSE,
        ),
        (
            "Mutation",
            "ghostPut",
            GHOST_DATA_SOURCE,
            REFUSED_PUT_REQUEST,
            RESULT_RESPONSE,
        ),
    ] {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": source,
                "requestMappingTemplate": template(request),
                "responseMappingTemplate": template(response),
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {type_name}.{field} must answer 200, got {created}"
        );
    }
    let (status, function) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": "batchWrite",
            "dataSourceName": DATA_SOURCE,
            "functionVersion": FUNCTION_VERSION,
            "requestMappingTemplate": template(BATCH_REQUEST),
            "responseMappingTemplate": template(BATCH_RESPONSE),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateFunction must answer 200, got {function}"
    );
    let function_id = function["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction must mint a functionId, got {function}"))
        .to_string();
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
        Some(json!({
            "fieldName": "batchPutItems",
            "kind": "PIPELINE",
            "pipelineConfig": {"functions": [function_id]},
            "requestMappingTemplate": EMPTY_REQUEST,
            "responseMappingTemplate": PREVIOUS_RESPONSE,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for Mutation.batchPutItems must answer 200, got {created}"
    );
}
