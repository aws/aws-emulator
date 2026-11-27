// SPDX-License-Identifier: CC0-1.0

//! What the `defaultAction` of a GraphQL API's Amazon Cognito user pool configuration decides
//! about the fields a caller its pool admitted reaches, driven over both of the addresses the
//! API publishes with tokens a user pool of this emulator really minted.
//!
//! The rule these rows assert, one level above any single API:
//!
//! > `defaultAction` is "the action that you want your GraphQL API to take when a request that
//! > uses Amazon Cognito user pool authentication doesn't match the Amazon Cognito user pool
//! > configuration" (`UserPoolConfig$defaultAction`,
//! > `smithy_query(query="shape",shape="UserPoolConfig")`, whose values are `ALLOW` and
//! > `DENY`), and one of the mode's own directives — `@aws_auth` or
//! > `@aws_cognito_user_pools` — is how a schema states which calls of that mode a field
//! > matches. So under `DENY` a field of an OPERATION ROOT type is reached only where such a
//! > directive is written on it, or on the type that declares it, whatever groups it names;
//! > under `ALLOW` every field is reached. Either way a directive that NAMES GROUPS is reached
//! > only by a caller whose token claims one of them, and a field the schema grants nowhere
//! > under `DENY` is answered exactly as a guarded one is for a caller outside its groups:
//! > `200` with the field `null`, an `errors` entry of the `Unauthorized` class naming the
//! > field and its type, and no resolver of it run — so the write behind a refused mutation
//! > does not happen.
//!
//! The rows span what that rule generalises over: the configuration's action (`ALLOW`,
//! `DENY`), what grants the field (nothing at all, a directive on the field, a directive on
//! the type that declares it, a directive naming groups the caller claims, one naming groups
//! it does not), the operation the field belongs to (a query, a mutation, a subscription
//! started on the real-time endpoint), where the field sits (a root field, a field of the
//! object a granted root field resolves to), and how the root type is named (GraphQL's own
//! default, and a name the schema binds itself).
//!
//! Nothing asserted here is a value this emulator filled in for the assertion's sake: every
//! token is one `AdminInitiateAuth` minted for a user of a pool this suite created, and what a
//! served mutation wrote is read back through Amazon DynamoDB's own reader — both services are
//! mounted beside AppSync on one engine.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    install_schema, try_post_answer, GRAPHQL_PATH,
};
use crate::graphql_api_realtime_endpoint::{exchange, handshake, started_message};
use crate::graphql_api_user_pool_authorization::{
    emulator, id_token, pool_with_client, sign_in, user_in_groups,
};

/// The Region every call of this suite is signed for, and the Region the pool is created in.
const REGION: &str = "us-west-2";

/// The refusal a `start` the endpoint does not admit carries. The service's own copies are
/// `service::event_api::UNAUTHORIZED_ERROR_TYPE` and `…_MESSAGE`.
const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";

/// The error AppSync reports for a field the caller may not access, and the fixed opening of
/// the sentence it states. The service's own copies are
/// `service::graphql_api::field_auth::UNAUTHORIZED_FIELD_ERROR_TYPE` and
/// `unauthorized_field_message`.
const UNAUTHORIZED_FIELD_ERROR_TYPE: &str = "Unauthorized";
const UNAUTHORIZED_FIELD: &str = "Not Authorized to access";

/// The `runtime` the code-defined resolvers of this suite are defined under. Source:
/// `AppSyncRuntime`'s own values (`smithy_query(query="shape",shape="AppSyncRuntime")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The group the guarded fields of the schemas below name, and the user that claims it.
const ADMINS: &str = "Admins";
const IN_GROUP: &str = "alice";
const IN_NO_GROUP: &str = "bob";

/// The values the query fields of [`SCHEMA`] resolve to.
const UNGRANTED_VALUE: &str = "ungranted-value";
const GRANTED_VALUE: &str = "granted-value";
const ADMIN_VALUE: &str = "admin-value";

/// The item a granted query field resolves to, whose own type grants nothing.
const NESTED_ID: &str = "I-1";
const NESTED_TEXT: &str = "nested-text";

/// The table the mutations of this suite write to, the attribute it is keyed on, and the text
/// every served write stores.
const TABLE: &str = "defaultActionItems";
const KEY_ATTRIBUTE: &str = "id";
const WRITTEN_TEXT: &str = "written";

/// The name of the local data source the query fields resolve against: `NONE` is "There is no
/// data source", so the request function's own payload is the result.
const LOCAL_DATA_SOURCE: &str = "local";

/// The name of the `AMAZON_DYNAMODB` data source the mutations resolve against, and the role
/// it names — a value this emulator's IAM holds no role under, which withholds no call, so
/// what a row observes is the default action and nothing else.
const TABLE_DATA_SOURCE: &str = "items";
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/appsync-default-action";

/// How long a case waits for the clock to make the table due, and how often it looks again.
/// Generous: what is under test is what the endpoint answers, never how fast the host runs.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The schema both APIs under the two actions are created with: one query field under no
/// directive at all, one granted by a directive of its own, one guarded by a group, one
/// resolving to an object whose own type grants nothing, and the mutation and subscription
/// pairs that say the same about those operations.
const SCHEMA: &str = "schema { query: Query mutation: Mutation subscription: Subscription }\n\
     \n\
     type Item {\n  id: ID!\n  text: String\n}\n\
     \n\
     type Query {\n  \
     ungranted: String\n  \
     granted: String @aws_cognito_user_pools\n  \
     guardedForAdmins: String @aws_auth(cognito_groups: [\"Admins\"])\n  \
     grantedItem: Item @aws_cognito_user_pools\n  \
     ungrantedItem: Item\n}\n\
     \n\
     type Mutation {\n  \
     putUngranted(id: ID!): Item\n  \
     putGranted(id: ID!): Item @aws_cognito_user_pools\n}\n\
     \n\
     type Subscription {\n  \
     onUngranted: Item @aws_subscribe(mutations: [\"putUngranted\"])\n  \
     onGranted: Item @aws_cognito_user_pools @aws_subscribe(mutations: [\"putGranted\"])\n}\n";

/// The schema of the third API: the grant is written on the TYPE that declares the fields
/// rather than on any of them, and the schema BINDS its root types under names of its own, so
/// which types hold an operation's fields is the schema's own binding and not a name this
/// suite assumed.
const TYPE_GRANTED_SCHEMA: &str = "schema { query: ReadRoot mutation: WriteRoot }\n\
     \n\
     type Item {\n  id: ID!\n  text: String\n}\n\
     \n\
     type ReadRoot @aws_cognito_user_pools {\n  \
     ungranted: String\n  \
     guardedForAdmins: String @aws_auth(cognito_groups: [\"Admins\"])\n}\n\
     \n\
     type WriteRoot {\n  putUngranted(id: ID!): Item\n}\n";

/// One created GraphQL API of this suite: the addresses its two endpoints are served on.
struct Api {
    graphql: String,
    realtime: String,
}

/// What one schema of this suite declares: the types it binds as its operation roots, and the
/// fields on them this suite attaches a resolver to.
struct Layout {
    query_root: &'static str,
    mutation_root: &'static str,
    query_fields: &'static [&'static str],
    mutation_fields: &'static [&'static str],
}

/// The layout of [`SCHEMA`], whose roots carry GraphQL's own default names.
const LAYOUT: Layout = Layout {
    query_root: "Query",
    mutation_root: "Mutation",
    query_fields: &[
        "ungranted",
        "granted",
        "guardedForAdmins",
        "grantedItem",
        "ungrantedItem",
    ],
    mutation_fields: &["putUngranted", "putGranted"],
};

/// The layout of [`TYPE_GRANTED_SCHEMA`], whose roots are bound under names of its own.
const TYPE_GRANTED_LAYOUT: Layout = Layout {
    query_root: "ReadRoot",
    mutation_root: "WriteRoot",
    query_fields: &["ungranted", "guardedForAdmins"],
    mutation_fields: &["putUngranted"],
};

/// What a row expects of the call it makes.
#[derive(Debug)]
enum Expected {
    /// The field resolves to this value.
    Served(Value),
    /// The field is one the caller may not access.
    Refused,
}

#[tokio::test]
async fn the_default_action_decides_which_fields_a_pool_token_reaches() {
    let signed_in = given_pool().await;
    let under_allow = create_api(&signed_in, "allow", "ALLOW", SCHEMA, &LAYOUT).await;
    let under_deny = create_api(&signed_in, "deny", "DENY", SCHEMA, &LAYOUT).await;

    /// One row: the API whose configuration decides it, the caller's token, the field asked
    /// for and what the call must come to.
    struct Row<'a> {
        case: &'static str,
        api: &'a Api,
        token: &'a str,
        /// The field's own selection, as the document writes it.
        selection: &'static str,
        /// The field the answer is read under, and the type that declares it.
        field: &'static str,
        type_name: &'static str,
        expected: Expected,
    }
    let rows = vec![
        Row {
            case: "a field under no directive, on an API whose action is ALLOW",
            api: &under_allow,
            token: &signed_in.in_no_group,
            selection: "ungranted",
            field: "ungranted",
            type_name: "Query",
            expected: Expected::Served(json!(UNGRANTED_VALUE)),
        },
        Row {
            case: "the same field, on an API whose action is DENY",
            api: &under_deny,
            token: &signed_in.in_no_group,
            selection: "ungranted",
            field: "ungranted",
            type_name: "Query",
            expected: Expected::Refused,
        },
        Row {
            case: "a field the schema grants this mode, under DENY",
            api: &under_deny,
            token: &signed_in.in_no_group,
            selection: "granted",
            field: "granted",
            type_name: "Query",
            expected: Expected::Served(json!(GRANTED_VALUE)),
        },
        Row {
            case: "a field guarded by a group the caller claims, under DENY",
            api: &under_deny,
            token: &signed_in.in_group,
            selection: "guardedForAdmins",
            field: "guardedForAdmins",
            type_name: "Query",
            expected: Expected::Served(json!(ADMIN_VALUE)),
        },
        Row {
            case: "the same guarded field for a caller claiming no group, under DENY",
            api: &under_deny,
            token: &signed_in.in_no_group,
            selection: "guardedForAdmins",
            field: "guardedForAdmins",
            type_name: "Query",
            expected: Expected::Refused,
        },
        Row {
            case: "the same guarded field for a caller claiming no group, under ALLOW",
            api: &under_allow,
            token: &signed_in.in_no_group,
            selection: "guardedForAdmins",
            field: "guardedForAdmins",
            type_name: "Query",
            expected: Expected::Refused,
        },
        Row {
            // The grant is about the OPERATION's field: the object a granted field resolves
            // to is projected whole, as the developer guide's own example — one annotated
            // root field over an unannotated object type — is served.
            case: "the members of the object a granted field resolves to, under DENY",
            api: &under_deny,
            token: &signed_in.in_no_group,
            selection: "grantedItem { id text }",
            field: "grantedItem",
            type_name: "Query",
            expected: Expected::Served(json!({"id": NESTED_ID, "text": NESTED_TEXT})),
        },
        Row {
            case: "the same object behind a root field the schema grants nowhere, under DENY",
            api: &under_deny,
            token: &signed_in.in_no_group,
            selection: "ungrantedItem { id text }",
            field: "ungrantedItem",
            type_name: "Query",
            expected: Expected::Refused,
        },
        Row {
            case: "the same object behind that root field, under ALLOW",
            api: &under_allow,
            token: &signed_in.in_no_group,
            selection: "ungrantedItem { id text }",
            field: "ungrantedItem",
            type_name: "Query",
            expected: Expected::Served(json!({"id": NESTED_ID, "text": NESTED_TEXT})),
        },
    ];

    for row in &rows {
        let case = row.case;
        let document = format!("query Ask {{ {} }}", row.selection);
        let (status, answered) = post(case, row.api, &document, row.token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call the mode admitted is answered 200, got {status} and \
             {answered}"
        );
        assert_expected(
            case,
            &answered,
            row.field,
            row.type_name,
            &row.expected,
        );
    }
}

#[tokio::test]
async fn a_grant_written_on_the_type_reaches_every_field_of_it() {
    let signed_in = given_pool().await;
    let api = create_api(
        &signed_in,
        "type-granted",
        "DENY",
        TYPE_GRANTED_SCHEMA,
        &TYPE_GRANTED_LAYOUT,
    )
    .await;

    // Every field of the type the grant is written on is reached, whatever the field itself
    // writes — and a field whose OWN guard names a group the caller does not claim is not.
    let rows: &[(&str, &str, &str, &str, Expected)] = &[
        (
            "a field of a granted type, under DENY",
            &signed_in.in_no_group,
            "ungranted",
            "ReadRoot",
            Expected::Served(json!(UNGRANTED_VALUE)),
        ),
        (
            "a guarded field of that type, for a caller claiming its group",
            &signed_in.in_group,
            "guardedForAdmins",
            "ReadRoot",
            Expected::Served(json!(ADMIN_VALUE)),
        ),
        (
            "the same guarded field, for a caller claiming no group",
            &signed_in.in_no_group,
            "guardedForAdmins",
            "ReadRoot",
            Expected::Refused,
        ),
    ];
    for (case, token, field, type_name, expected) in rows {
        let document = format!("query Ask {{ {field} }}");
        let (status, answered) = post(case, &api, &document, token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call the mode admitted is answered 200, got {status} and \
             {answered}"
        );
        assert_expected(case, &answered, field, type_name, expected);
    }

    // And a field of the type that grants nothing is reached by nobody, whichever name the
    // schema binds that root type under.
    let case = "a mutation of a type granting nothing, under DENY";
    let (status, answered) = post(
        case,
        &api,
        "mutation Write { putUngranted(id: \"never\") { id } }",
        &signed_in.in_group,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a call the mode admitted is answered 200, got {status} and {answered}"
    );
    assert_expected(case, &answered, "putUngranted", "WriteRoot", &Expected::Refused);
}

#[tokio::test]
async fn a_mutation_the_default_action_refuses_writes_nothing() {
    let signed_in = given_pool().await;
    let under_allow = create_api(&signed_in, "allow", "ALLOW", SCHEMA, &LAYOUT).await;
    let under_deny = create_api(&signed_in, "deny", "DENY", SCHEMA, &LAYOUT).await;

    // The rows differ along the action in force and along what grants the mutation field: the
    // one the write is performed for, and the two it is not.
    let rows: &[(&str, &Api, &str, &str, bool)] = &[
        (
            "a mutation under no directive, under ALLOW",
            &under_allow,
            "putUngranted",
            "allow-ungranted",
            true,
        ),
        (
            "the same mutation, under DENY",
            &under_deny,
            "putUngranted",
            "deny-ungranted",
            false,
        ),
        (
            "a mutation the schema grants this mode, under DENY",
            &under_deny,
            "putGranted",
            "deny-granted",
            true,
        ),
    ];
    for (case, api, field, id, served) in rows {
        let document = format!("mutation Write {{ {field}(id: \"{id}\") {{ id text }} }}");
        let (status, answered) = post(case, api, &document, &signed_in.in_no_group).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call the mode admitted is answered 200, got {status} and \
             {answered}"
        );
        let expected = match served {
            true => Expected::Served(json!({"id": id, "text": WRITTEN_TEXT})),
            false => Expected::Refused,
        };
        assert_expected(case, &answered, field, "Mutation", &expected);
        // What the table HOLDS is the proof no resolver of a refused field ran: a refused
        // mutation performs no write, and a served one performs its own.
        let stored = stored_item(&signed_in.router, id).await;
        let expected_item = match served {
            true => Some(json!({"id": {"S": id}, "text": {"S": WRITTEN_TEXT}})),
            false => None,
        };
        assert_eq!(
            stored, expected_item,
            "case {case:?}: expected the table to hold {expected_item:?}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_subscription_start_is_decided_by_the_same_grant() {
    let signed_in = given_pool().await;
    let under_allow = create_api(&signed_in, "allow", "ALLOW", SCHEMA, &LAYOUT).await;
    let under_deny = create_api(&signed_in, "deny", "DENY", SCHEMA, &LAYOUT).await;

    // The rows differ along the action in force and along what grants the subscription field.
    let rows: &[(&str, &Api, &str, bool)] = &[
        (
            "a subscription field under no directive, under ALLOW",
            &under_allow,
            "onUngranted",
            true,
        ),
        (
            "the same subscription field, under DENY",
            &under_deny,
            "onUngranted",
            false,
        ),
        (
            "a subscription field the schema grants this mode, under DENY",
            &under_deny,
            "onGranted",
            true,
        ),
    ];
    for (case, api, field, acked) in rows {
        // The material travels in the handshake's base64url `header` object, which is where a
        // real-time client of a GraphQL API puts a request's own fields.
        let mut socket = handshake(
            case,
            &api.realtime,
            GRAPHQL_PATH,
            true,
            Some(&json!({"host": api.realtime, "Authorization": signed_in.in_no_group})),
            &[],
        )
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the handshake must complete, got {status}")
        });
        let connected = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
        assert_eq!(
            connected.as_ref().and_then(|frame| frame["type"].as_str()),
            Some("connection_ack"),
            "case {case:?}: connection_init must be acked, got {connected:?}"
        );
        let document = format!("subscription Watch {{ {field} {{ id text }} }}");
        let started = exchange(
            case,
            &mut socket,
            &started_message("sub-1", &document, &json!({})),
        )
        .await;
        let kind = started.as_ref().and_then(|frame| frame["type"].as_str());
        match acked {
            true => assert_eq!(
                kind,
                Some("start_ack"),
                "case {case:?}: the start must be acked, got {started:?}"
            ),
            false => {
                assert_eq!(
                    kind,
                    Some("error"),
                    "case {case:?}: a start on a field the schema grants nowhere must be \
                     refused, got {started:?}"
                );
                assert_eq!(
                    started
                        .as_ref()
                        .and_then(|frame| frame.pointer("/payload/errors/0/errorType"))
                        .and_then(Value::as_str),
                    Some(UNAUTHORIZED_ERROR_TYPE),
                    "case {case:?}: expected the refusal of an unauthorized call, got \
                     {started:?}"
                );
            }
        }
    }
}

/// Assert one answer against what the row expects of it: the field's value, or the refusal
/// AppSync states for a field the caller may not access.
fn assert_expected(
    case: &str,
    answered: &Value,
    field: &str,
    type_name: &str,
    expected: &Expected,
) {
    match expected {
        Expected::Served(value) => {
            assert_eq!(
                answered.get("errors"),
                None,
                "case {case:?}: expected the field to resolve with no error reported, got \
                 {answered}"
            );
            assert_eq!(
                &answered["data"][field], value,
                "case {case:?}: expected {value} as the field's value, got {answered}"
            );
        }
        Expected::Refused => {
            assert_eq!(
                answered["data"][field],
                Value::Null,
                "case {case:?}: a field the caller may not access carries no value, got \
                 {answered}"
            );
            let error = &answered["errors"][0];
            assert_eq!(
                error["errorType"].as_str(),
                Some(UNAUTHORIZED_FIELD_ERROR_TYPE),
                "case {case:?}: expected the error class of a refused field, got {answered}"
            );
            let expected_message = format!("{UNAUTHORIZED_FIELD} {field} on type {type_name}");
            assert_eq!(
                error["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?}: expected {expected_message:?}, got {answered}"
            );
        }
    }
}

/// The emulator this suite drives, the pool it created, and the identity tokens of its two
/// users: one claiming the group the guards name, one claiming none.
struct SignedIn {
    router: MultiServiceRouter,
    pool_id: String,
    in_group: String,
    in_no_group: String,
}

/// Mount AppSync, Amazon Cognito user pools and Amazon DynamoDB on ONE engine, create the
/// pool with its two users and the table the mutations write to, and sign both users in.
async fn given_pool() -> SignedIn {
    // The pool, user and sign-in helpers are the user-pool suite's own, so this suite's
    // tokens are minted exactly as that one's are.
    let (mut router, engine) = emulator();
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    let (pool_id, client_id) = pool_with_client(&router, "default-action-pool").await;
    user_in_groups(&router, &pool_id, IN_GROUP, &[ADMINS]).await;
    user_in_groups(&router, &pool_id, IN_NO_GROUP, &[]).await;
    let in_group = id_token(&sign_in(&router, &pool_id, &client_id, IN_GROUP).await);
    let in_no_group = id_token(&sign_in(&router, &pool_id, &client_id, IN_NO_GROUP).await);
    given_table(&router).await;
    SignedIn {
        router,
        pool_id,
        in_group,
        in_no_group,
    }
}

/// Create one `AMAZON_COGNITO_USER_POOLS` API over the suite's pool whose configuration
/// states `default_action`, with `schema`, both data sources and one resolver per field
/// `layout` names.
async fn create_api(
    signed_in: &SignedIn,
    name: &str,
    default_action: &str,
    schema: &str,
    layout: &Layout,
) -> Api {
    let router = &signed_in.router;
    let user_pool_config = json!({
        "userPoolId": signed_in.pool_id,
        "awsRegion": REGION,
        "defaultAction": default_action,
    });
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("default-action-{name}"),
            "authenticationType": "AMAZON_COGNITO_USER_POOLS",
            "userPoolConfig": user_pool_config,
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
    install_schema(router, &api_id, schema).await;
    // The action the mode reads is the STORED one: what a later read reports is what the
    // create persisted, rather than what the request echoed.
    let (status, fetched) = appsync(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetGraphqlApi must answer 200, got {status} {fetched}"
    );
    assert_eq!(
        fetched["graphqlApi"]["userPoolConfig"], user_pool_config,
        "GetGraphqlApi must report the userPoolConfig the create stored, got {fetched}"
    );
    for source in [
        json!({"name": LOCAL_DATA_SOURCE, "type": "NONE"}),
        json!({
            "name": TABLE_DATA_SOURCE,
            "type": "AMAZON_DYNAMODB",
            "serviceRoleArn": SERVICE_ROLE,
            "dynamodbConfig": {"tableName": TABLE, "awsRegion": REGION},
        }),
    ] {
        let (status, answered) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(source.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateDataSource {source} must answer 200, got {answered}"
        );
    }
    create_resolvers(router, &api_id, layout).await;
    Api { graphql, realtime }
}

/// Attach one resolver per field the schema declares on its root types: a code-defined one
/// over the local data source for the query fields, and a template-defined `PutItem` over the
/// table for the mutations.
async fn create_resolvers(router: &MultiServiceRouter, api_id: &str, layout: &Layout) {
    let constant = |value: &str| {
        format!(
            "export function request(ctx) {{ return {{payload: \"{value}\"}}; }}\n\
             export function response(ctx) {{ return ctx.result; }}"
        )
    };
    let item = format!(
        "export function request(ctx) {{ \
           return {{payload: {{id: \"{NESTED_ID}\", text: \"{NESTED_TEXT}\"}}}}; }}\n\
         export function response(ctx) {{ return ctx.result; }}"
    );
    for field in layout.query_fields {
        let code = match *field {
            "granted" => constant(GRANTED_VALUE),
            "guardedForAdmins" => constant(ADMIN_VALUE),
            "grantedItem" | "ungrantedItem" => item.clone(),
            _ => constant(UNGRANTED_VALUE),
        };
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{}/resolvers", layout.query_root),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": LOCAL_DATA_SOURCE,
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
    // The `PutItem` request object a mapping template of the documented version returns,
    // addressing the item by the key the table is keyed on.
    let put_request = format!(
        "{{\n  \"version\": \"2018-05-29\",\n  \"operation\": \"PutItem\",\n  \
         \"key\": {{\"{KEY_ATTRIBUTE}\": $util.dynamodb.toDynamoDBJson($ctx.args.id)}},\n  \
         \"attributeValues\": $util.dynamodb.toMapValuesJson({{\"text\": \
         \"{WRITTEN_TEXT}\"}})\n}}"
    );
    for field in layout.mutation_fields {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{}/resolvers", layout.mutation_root),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": TABLE_DATA_SOURCE,
                "requestMappingTemplate": put_request,
                "responseMappingTemplate": "$util.toJson($ctx.result)",
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {created}"
        );
    }
}

/// POST one GraphQL document to the API's published endpoint, presenting `token` in the
/// request's `authorization` field, as a client does.
async fn post(case: &str, api: &Api, document: &str, token: &str) -> (u16, Value) {
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &json!({"query": document}),
        &[("authorization", token)],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    (status, answered)
}

/// The item the table holds under `id`, as DynamoDB's own reader answers it, or `None` for a
/// key it holds nothing under.
async fn stored_item(router: &MultiServiceRouter, id: &str) -> Option<Value> {
    let read = dynamodb(
        router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": id}}, "ConsistentRead": true}),
    )
    .await;
    read.get("Item").filter(|item| !item.is_null()).cloned()
}

/// Create the table the data source names, through DynamoDB's own API, and wait until that
/// service first reports it `ACTIVE`.
async fn given_table(router: &MultiServiceRouter) {
    dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    // The wait is on the CONDITION that service itself reports, so nothing here depends on
    // how fast the host runs.
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = dynamodb(router, "DescribeTable", json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "setup: the table must become ACTIVE, last read {described}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// One Amazon DynamoDB call over its own wire (`awsJson1_0`, `POST /` with `X-Amz-Target`),
/// having insisted it was served.
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
        .body(bytes::Bytes::from(body.to_string()))
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
