// SPDX-License-Identifier: CC0-1.0

//! What the permissions in force on a data source's SERVICE ROLE decide about the call a
//! resolver step makes, driven over the API's own published `GRAPHQL` endpoint with IAM and
//! Amazon DynamoDB mounted on the same engine — so the documents the decision is taken on are
//! IAM's own and what a served call wrote is read back where a client reads it.
//!
//! The rule these cases assert, one level above the recorded case (one data source whose role
//! does not allow the write its resolver asks for):
//!
//! > A step calls its data source as a SESSION of the role the data source names
//! > ("the system assumes this role when accessing the data source"), so the call is decided by
//! > the permissions in force on THAT role and not by what the caller of the GraphQL request
//! > may do. Where this account's IAM HOLDS the role, that role is the authority on what may be
//! > done under it: the action has to be ALLOWED on the resource the call addresses, and a
//! > policy set of its that allows it nowhere — an empty set included — refuses the call, as
//! > does an explicit deny. Where IAM holds NO role under the value the data source names,
//! > nothing is known about it — a service role is routinely provisioned out of band — so only
//! > a statement that denies the action holds the call back. A refused call is an error OF THE
//! > FIELD: `200` with the field `null`, an `errors` entry carrying the called service's
//! > access-denied class and IAM's own sentence naming the role SESSION, the action and the
//! > resource, and nothing read or written.
//!
//! The rows span what the rule generalises over: what IAM holds about the role (a held role
//! allowed the action on this table, allowed it on every resource, allowing only another
//! action, allowing it only on another table, denying it beside a wildcard allow, carrying no
//! identity-based document at all; a value this account's IAM holds no role under; and a value
//! naming a role of another account), and so both refusal reasons IAM words and both reasons
//! nothing is withheld.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    install_schema, try_post_answer, GRAPHQL_PATH,
};
use crate::identity_authorization::{form_encode, xml_element};

/// The Region every call of this suite is signed for, which is the Region the table is created
/// in and the one the data sources name it in.
const REGION: &str = "us-west-2";

/// The table every data source of this suite resolves against, and the attribute it is keyed
/// on.
const TABLE: &str = "roleGuardedItems";
const KEY_ATTRIBUTE: &str = "id";

/// Another table no row's call ever addresses: the role whose allow names it is allowed the
/// action somewhere else than where the call is made.
const OTHER_TABLE_ARN: &str = "arn:aws:dynamodb:us-west-2:123456789012:table/elsewhere";

/// The IAM action the write every row makes performs, and the name of the session AppSync makes
/// a data source's call under (the service's own copy is
/// `service::graphql_api::data_source_role::ROLE_SESSION_NAME`).
const WRITE_ACTION: &str = "dynamodb:PutItem";
const ROLE_SESSION_NAME: &str = "APPSYNC_ASSUME_ROLE";

/// The `errorType` a refused call of this data source is reported under: the permissions are
/// weighed before the call, so there is no answer of the called service to classify and the
/// refusal is reported under that service's base exception. The service's own copies are
/// `service::graphql_api::dynamodb::ANSWERED_ERROR_TYPE_PREFIX` and its `BASE_EXCEPTION`.
const ACCESS_DENIED_ERROR_TYPE: &str = "DynamoDB:DynamoDbException";

/// How long a case waits for the clock to make a table or a schema creation due, and how often
/// it looks again. Generous: what is under test is what the endpoint answers, never how fast
/// the host runs.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The request half every resolver of this suite is defined by: the `PutItem` request object a
/// mapping template of the documented version returns, addressing the item by the key the table
/// is keyed on.
const PUT_REQUEST: &str = r#"{
  "version": "2018-05-29",
  "operation": "PutItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "attributeValues": $util.dynamodb.toMapValuesJson({"note": "written"})
}"#;

/// The response half: the item the call answered, as the field's value.
const PUT_RESPONSE: &str = "$util.toJson($ctx.result)";

/// The note every served write stores, which is what proves the call was made with the request
/// the template rendered.
const NOTE: &str = "written";

/// One row: what IAM holds about the role the data source names, and whether the call is made
/// under it.
struct Row {
    /// What the row exercises, named in every assertion of it.
    case: &'static str,
    /// The suffix of the role's name, which also names the row's data source and field.
    role: &'static str,
    /// The inline document the role carries, `None` for a role carrying none at all.
    policy: Option<String>,
    /// A value to name INSTEAD of creating a role: the case about a role IAM holds nothing
    /// under, and the one about a role of another account.
    named_instead: Option<String>,
    /// Whether the call is made, and so whether the item is written.
    served: bool,
    /// The reason clause IAM's refusal states, for a row whose call is withheld.
    reason: Option<String>,
}

/// The rows, over the ARN the table is held under.
fn rows(table_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "a held role allowed the action on this table",
            role: "allowed",
            policy: Some(statement("Allow", WRITE_ACTION, table_arn)),
            named_instead: None,
            served: true,
            reason: None,
        },
        Row {
            case: "a held role allowed it on every resource",
            role: "wildcard",
            policy: Some(statement("Allow", "dynamodb:*", "*")),
            named_instead: None,
            served: true,
            reason: None,
        },
        Row {
            case: "a held role whose only allow names another action",
            role: "other-action",
            policy: Some(statement("Allow", "dynamodb:GetItem", table_arn)),
            named_instead: None,
            served: false,
            reason: Some(no_policy_allows()),
        },
        Row {
            case: "a held role whose only allow names another table",
            role: "other-table",
            policy: Some(statement("Allow", WRITE_ACTION, OTHER_TABLE_ARN)),
            named_instead: None,
            served: false,
            reason: Some(no_policy_allows()),
        },
        Row {
            case: "a held role denying the action beside a wildcard allow",
            role: "denied",
            policy: Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Effect\":\"Allow\",\"Action\":\"dynamodb:*\",\"Resource\":\"*\"}},\
                 {{\"Effect\":\"Deny\",\"Action\":\"{WRITE_ACTION}\",\"Resource\":\"*\"}}]}}"
            )),
            named_instead: None,
            served: false,
            reason: Some(EXPLICIT_DENY.to_string()),
        },
        Row {
            // The policy set of a role IAM holds is what that role may do, and an EMPTY set
            // allows nothing — the case the recorded stack's second data source is in.
            case: "a held role carrying no identity-based document at all",
            role: "undocumented",
            policy: None,
            named_instead: None,
            served: false,
            reason: Some(no_policy_allows()),
        },
        Row {
            // Nothing is known about a role IAM holds nothing under: such a role is routinely
            // provisioned out of band, so its absence from this store is no answer about it.
            case: "a value this account's IAM holds no role under",
            role: "absent",
            policy: None,
            named_instead: Some(format!(
                "arn:aws:iam::{}:role/appsync-ds-never-created",
                ::engine::emulator_account_id()
            )),
            served: true,
            reason: None,
        },
        Row {
            // Another account's IAM is not this emulator's to ask, so a value naming one of its
            // roles establishes nothing either.
            case: "a value naming a role of another account",
            role: "foreign",
            policy: None,
            named_instead: Some(
                "arn:aws:iam::210987654321:role/appsync-ds-another-account".to_string(),
            ),
            served: true,
            reason: None,
        },
    ]
}

/// One `{Effect, Action, Resource}` statement as an inline policy document, written compactly
/// and free of the characters an `application/x-www-form-urlencoded` body treats specially.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"{effect}\",\"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// IAM's reason clause for a write no identity-based document in force allows, and the one for a
/// write an explicit deny refuses. The service's own copies are
/// `::service_common::iam_authorization::no_identity_policy_allows` and
/// `::service_common::iam_authorization::IDENTITY_POLICY_EXPLICIT_DENY`.
fn no_policy_allows() -> String {
    format!("because no identity-based policy allows the {WRITE_ACTION} action")
}
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// A router with AppSync, IAM and Amazon DynamoDB over ONE engine: the role and its documents
/// belong to IAM, and the table every call addresses to DynamoDB.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // What a case pins is what the endpoint ANSWERS; signature verification is the emulator's
    // own front door, not a service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

#[tokio::test]
async fn the_documents_in_force_on_a_data_sources_role_decide_the_call_it_makes() {
    let router = emulator();
    let table_arn = given_table(&router).await;
    let rows = rows(&table_arn);
    let api = create_api(&router, &rows).await;

    for row in &rows {
        let case = row.case;
        let id = format!("item-{}", row.role);
        let field = field_of(row);
        let answered = write(case, &api, &field, &id).await;

        if row.served {
            assert_eq!(
                answered.get("errors"),
                None,
                "case {case:?}: expected the call to be made with no error reported, got \
                 {answered}"
            );
            assert_eq!(
                answered["data"][&field],
                json!({"id": id, "note": NOTE}),
                "case {case:?}: expected the written item as the field's value, got {answered}"
            );
            let stored = stored_item(&router, &id).await;
            assert_eq!(
                stored,
                Some(json!({"id": {"S": id}, "note": {"S": NOTE}})),
                "case {case:?}: expected the item in the table DynamoDB holds, got {stored:?}"
            );
            continue;
        }

        assert_eq!(
            answered["data"][&field],
            Value::Null,
            "case {case:?}: a field whose call was refused carries no value, got {answered}"
        );
        let error = &answered["errors"][0];
        assert_eq!(
            error["errorType"].as_str(),
            Some(ACCESS_DENIED_ERROR_TYPE),
            "case {case:?}: expected the error class of a refused call, got {answered}"
        );
        // The refusal names the SESSION of the role the data source names, the action the call
        // performs, the table it addresses and IAM's reason for withholding it.
        let message = error["message"].as_str().unwrap_or_default();
        let session = format!(
            "arn:aws:sts::{}:assumed-role/appsync-ds-{}/{ROLE_SESSION_NAME}",
            ::engine::emulator_account_id(),
            row.role
        );
        let reason = row.reason.clone().unwrap_or_default();
        for named in [
            session.as_str(),
            WRITE_ACTION,
            table_arn.as_str(),
            reason.as_str(),
        ] {
            assert!(
                message.contains(named),
                "case {case:?}: expected the refusal to state {named:?}, got {answered}"
            );
        }
        let stored = stored_item(&router, &id).await;
        assert!(
            stored.is_none(),
            "case {case:?}: a refused call writes nothing, got {stored:?}"
        );
    }
}

/// The mutation field the row's own resolver is attached to.
fn field_of(row: &Row) -> String {
    format!("put_{}", row.role.replace('-', "_"))
}

/// POST the row's write to the address the API publishes, as a client does, and answer the
/// document it was served.
async fn write(case: &str, api: &Api, field: &str, id: &str) -> Value {
    let document = json!({
        "query": format!("mutation Write($id: ID!) {{ {field}(id: $id) {{ id note }} }}"),
        "variables": {"id": id},
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

/// One created GraphQL API of this suite: the address its `GRAPHQL` endpoint is served on, and
/// a live API key to present to it.
struct Api {
    graphql: String,
    api_key: String,
}

/// Create the API every row drives: one `AMAZON_DYNAMODB` data source per row, under the role
/// that row names, and one template-defined `UNIT` resolver per row on its own field.
async fn create_api(router: &MultiServiceRouter, rows: &[Row]) -> Api {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": "data-source-role-permissions", "authenticationType": "API_KEY"})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    assert!(
        !graphql.is_empty(),
        "setup: the API must publish the address its GRAPHQL endpoint is served on, got {created}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    install_schema(router, &api_id, &schema(rows)).await;

    for row in rows {
        let role_arn = match &row.named_instead {
            Some(named) => named.clone(),
            None => {
                given_role(
                    router,
                    &format!("appsync-ds-{}", row.role),
                    row.policy.as_deref(),
                )
                .await
            }
        };
        let source = format!("source_{}", row.role.replace('-', "_"));
        let (status, data_source) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(json!({
                "name": source,
                "type": "AMAZON_DYNAMODB",
                "serviceRoleArn": role_arn,
                "dynamodbConfig": {"tableName": TABLE, "awsRegion": REGION},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateDataSource {source} must answer 200, got {data_source}"
        );
        let (status, resolver) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
            Some(json!({
                "fieldName": field_of(row),
                "kind": "UNIT",
                "dataSourceName": source,
                "requestMappingTemplate": PUT_REQUEST,
                "responseMappingTemplate": PUT_RESPONSE,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {} must answer 200, got {resolver}",
            field_of(row)
        );
    }
    Api {
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// The schema the API is created with: the item the writes answer, and one mutation field per
/// row, so every row is driven through one installed schema.
fn schema(rows: &[Row]) -> String {
    let fields: String = rows
        .iter()
        .map(|row| format!("  {}(id: ID!): Item\n", field_of(row)))
        .collect();
    format!(
        "schema {{ query: Query mutation: Mutation }}\n\n\
         type Item {{\n  id: ID!\n  note: String\n}}\n\n\
         type Query {{\n  ping: String\n}}\n\n\
         type Mutation {{\n{fields}}}\n"
    )
}

/// Create the table every data source of this suite names, through DynamoDB's own API, wait
/// until that service first reports it `ACTIVE`, and answer the ARN it holds it under.
async fn given_table(router: &MultiServiceRouter) -> String {
    let created = dynamodb(
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
    let arn = created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "setup: CreateTable must report the ARN the table is held under, got {created}"
    );
    // The wait is on the CONDITION that service itself reports, so nothing here depends on how
    // fast the host runs.
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = dynamodb(router, "DescribeTable", json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return arn;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "setup: the table must become ACTIVE, last read {described}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Create an IAM role called `role_name`, trusted by AppSync and carrying `policy` as its
/// inline document when one is given, through IAM's OWN operations; answer its ARN.
async fn given_role(
    router: &MultiServiceRouter,
    role_name: &str,
    policy: Option<&str>,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "appsync.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role's Arn, got {created}"));
    if let Some(policy) = policy {
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }
    arn
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

/// One awsQuery call to IAM, answering the XML body it was served and having insisted the call
/// succeeded.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "the IAM {action} call of this suite must succeed, got {status} with {text}"
    );
    text
}
