// SPDX-License-Identifier: CC0-1.0

//! A resolver whose data source is of the `RELATIONAL_DATABASE` type, driven over the API's
//! own published `GRAPHQL` endpoint, with the Amazon RDS control plane that holds the cluster,
//! the Data API that serves its databases, the Secrets Manager (and the KMS its value is
//! sealed under) the login comes from, and IAM mounted on the same engine — so what a
//! resolver's statements did is read back where a client reads it: in the database itself,
//! through the service that owns it.
//!
//! The rule these cases assert, one level above the recorded case (one query reading one row
//! of one table):
//!
//! > A step whose data source is of the `RELATIONAL_DATABASE` type RUNS the statements its
//! > request half returned against the database that data source was configured with
//! > (`relationalDatabaseConfig.rdsHttpEndpointConfig`: the cluster's ARN, the secret holding
//! > the login and the logical database), through that cluster's own HTTP endpoint, under the
//! > permissions of the role it names (`serviceRoleArn`): every statement of the request is
//! > run, in the order the request states them, with every value it carries bound as a
//! > PARAMETER rather than written into the statement's text, and the result the response half
//! > is handed is the Data API's own answer — one entry per statement, with the rows and the
//! > column metadata that answer carries. A request this data source cannot act on, and a role
//! > whose documents withhold the Data API action on that cluster, run no statement at all and
//! > are reported as errors of the field.
//!
//! The rows span what the rule generalises over: which statement a resolver builds (a
//! selection by equality, by a prefix, with an ordering and a bound, an insert, an update and
//! a delete), how many statements one request carries, where the statement comes from (the
//! builders, a tagged template, and a Velocity request template), the values it binds
//! (numbers, text, and text that is itself SQL), the rows it matches (one, several, none) and
//! what the role's documents say.
//!
//! The helpers that set such a topology up — the router of the services that own it, the
//! cluster with its database, the role the data source names, the Data API call a statement is
//! run through, and the imports and response half a coded resolver shares — are shared with the
//! suite that drives the same data source from a DISCOVERED schema
//! ([`crate::graphql_api_introspection_driven_relational_resolver`]), so there is one reading of
//! that setup rather than one per suite.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{try_post_answer, GRAPHQL_PATH};
use crate::identity_authorization::{form_encode, xml_element};

/// The Region every call of this suite is signed for, which is the Region the API's data plane
/// publishes into: the control-plane helper this suite shares signs for it
/// ([`crate::event_api_data_plane::appsync`]).
const REGION: &str = "us-west-2";

/// The engine the cluster of this suite runs, the database it holds and the master user it is
/// created with.
const ENGINE: &str = "aurora-mysql";
pub(crate) const DATABASE: &str = "appdb";
const MASTER_USER: &str = "admin";

/// The table every statement of this suite is about, and the rows it is created with.
pub(crate) const TABLE: &str = "widgets";
pub(crate) const SEEDED: &[(i64, &str)] =
    &[(1, "widget-alpha"), (2, "widget-beta"), (3, "gadget-gamma")];

/// How long a case waits for the clock to perform a schema creation, far past the service's
/// own span, so a loaded machine still reaches it; and how long it leaves between two looks.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(50);

/// The `runtime` every coded resolver of this suite is defined under. Source:
/// `AppSyncRuntime`'s own values (`smithy_query(query="shape",shape="AppSyncRuntime")`).
pub(crate) const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The name of the data source every resolver of this suite resolves against.
const DATA_SOURCE: &str = "widgetsDb";

/// The `errorType` of a step whose own request the data source cannot act on, and the prefix
/// every class the called service answered carries.
const REQUEST_ERROR_TYPE: &str = "Code";
const ANSWERED_ERROR_TYPE_PREFIX: &str = "RDSHttp:";
const ACCESS_DENIED_ERROR_TYPE: &str = "RDSHttp:AccessDeniedException";

/// The schema every API of this suite is created with: the fields the cases drive, over the
/// one type a row of the table is.
const SCHEMA: &str = "schema { query: Query mutation: Mutation }\n\
                      \n\
                      type Query {\n  \
                      widget(id: Int!): Widget\n  \
                      widgetsNamed(prefix: String!): [Widget]\n  \
                      widgetByText(name: String!): Widget\n  \
                      widgetsTemplated(prefix: String!): AWSJSON\n}\n\
                      \n\
                      type Mutation {\n  \
                      addWidget(id: Int!, name: String!): Widget\n  \
                      renameWidget(id: Int!, name: String!): Widget\n  \
                      removeWidget(id: Int!): Int\n}\n\
                      \n\
                      type Widget {\n  id: Int\n  name: String\n}\n";

/// The response half every coded resolver of this suite shares: it re-raises what went wrong
/// with the call, so a failure reaches the caller, and otherwise reads the rows of the
/// statement at `at`.
pub(crate) fn responds_with_rows(at: usize, one: bool) -> String {
    let value = match one {
        true => "rows.length === 0 ? null : rows[0]",
        false => "rows",
    };
    format!(
        "export function response(ctx) {{\n\
         if (ctx.error) {{ util.error(ctx.error.message, ctx.error.type); }}\n\
         const rows = toJsonObject(ctx.result)[{at}];\n\
         return {value};\n\
         }}\n"
    )
}

/// The import every coded resolver of this suite begins with: the helpers AppSync's own
/// relational-database resolvers import.
pub(crate) const IMPORTS: &str = "import { util } from '@aws-appsync/utils';\n\
                       import { select, insert, update, remove, sql, createMySQLStatement, \
                       toJsonObject } from '@aws-appsync/utils/rds';\n";

#[tokio::test]
async fn a_resolver_runs_the_statements_it_built_against_the_data_sources_database() {
    let router = emulator();
    let cluster = given_cluster(&router, "widgets-read").await;
    let role = given_role(&router, "appsync-rds-read", Some(&allowing(&cluster.arn))).await;
    let api = create_api(&router, "read", &cluster, &role).await;

    /// One row: the field the case selects, the arguments it passes and what the field must
    /// resolve to.
    struct Row<'a> {
        case: &'a str,
        document: String,
        field: &'a str,
        expected: Value,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a selection by equality, matching one row",
            document: "query { widget(id: 1) { id name } }".to_string(),
            field: "widget",
            expected: json!({"id": 1, "name": "widget-alpha"}),
        },
        Row {
            case: "a selection by equality, matching no row",
            document: "query { widget(id: 99) { id name } }".to_string(),
            field: "widget",
            expected: Value::Null,
        },
        Row {
            case: "a selection by prefix, ordered, matching several rows",
            document: "query { widgetsNamed(prefix: \"widget\") { id name } }".to_string(),
            field: "widgetsNamed",
            expected: json!([
                {"id": 1, "name": "widget-alpha"},
                {"id": 2, "name": "widget-beta"},
            ]),
        },
        Row {
            case: "a selection by prefix, matching one row",
            document: "query { widgetsNamed(prefix: \"gadget\") { id name } }".to_string(),
            field: "widgetsNamed",
            expected: json!([{"id": 3, "name": "gadget-gamma"}]),
        },
        Row {
            case: "a selection by prefix, matching no row",
            document: "query { widgetsNamed(prefix: \"nothing\") { id name } }".to_string(),
            field: "widgetsNamed",
            expected: json!([]),
        },
        Row {
            case: "a statement written as a tagged template",
            document: "query { widgetByText(name: \"widget-beta\") { id name } }".to_string(),
            field: "widgetByText",
            expected: json!({"id": 2, "name": "widget-beta"}),
        },
    ];
    for row in &rows {
        let case = row.case;
        let answered = post(case, &api, &json!({"query": row.document})).await;
        assert_eq!(
            answered["errors"].as_array(),
            None,
            "case {case:?}: a statement the database runs reports no error, got {answered}"
        );
        assert_eq!(
            answered["data"][row.field], row.expected,
            "case {case:?}: expected {}, got {answered}",
            row.expected
        );
    }
}

#[tokio::test]
async fn a_resolver_that_writes_changes_the_database_the_data_source_names() {
    let router = emulator();
    let cluster = given_cluster(&router, "widgets-write").await;
    let role = given_role(&router, "appsync-rds-write", Some(&allowing(&cluster.arn))).await;
    let api = create_api(&router, "write", &cluster, &role).await;

    // An insert of a row the table does not hold: the request carries the write and the read
    // that reports it, so the field answers the row the database now holds.
    let case = "a mutation inserting a row";
    let answered = post(
        case,
        &api,
        &json!({"query": "mutation { addWidget(id: 4, name: \"widget-delta\") { id name } }"}),
    )
    .await;
    assert_eq!(
        answered["data"]["addWidget"],
        json!({"id": 4, "name": "widget-delta"}),
        "case {case:?}: expected the inserted row, got {answered}"
    );
    assert_eq!(
        held(&router, &cluster, 4).await,
        Some("widget-delta".to_string()),
        "case {case:?}: expected the database itself to hold the inserted row"
    );

    let case = "a mutation updating a row";
    let answered = post(
        case,
        &api,
        &json!({"query": "mutation { renameWidget(id: 4, name: \"widget-epsilon\") { id name } }"}),
    )
    .await;
    assert_eq!(
        answered["data"]["renameWidget"],
        json!({"id": 4, "name": "widget-epsilon"}),
        "case {case:?}: expected the updated row, got {answered}"
    );
    assert_eq!(
        held(&router, &cluster, 4).await,
        Some("widget-epsilon".to_string()),
        "case {case:?}: expected the database itself to hold the update"
    );

    let case = "a mutation deleting a row";
    let answered = post(
        case,
        &api,
        &json!({"query": "mutation { removeWidget(id: 4) }"}),
    )
    .await;
    assert_eq!(
        answered["data"]["removeWidget"],
        json!(1),
        "case {case:?}: expected the count the Data API reported, got {answered}"
    );
    assert_eq!(
        held(&router, &cluster, 4).await,
        None,
        "case {case:?}: expected the row to be gone from the database itself"
    );

    let case = "a mutation deleting a row the table does not hold";
    let answered = post(
        case,
        &api,
        &json!({"query": "mutation { removeWidget(id: 98) }"}),
    )
    .await;
    assert_eq!(
        answered["data"]["removeWidget"],
        json!(0),
        "case {case:?}: expected a count of no rows, got {answered}"
    );
}

#[tokio::test]
async fn every_value_a_statement_carries_is_bound_rather_than_written_into_it() {
    let router = emulator();
    let cluster = given_cluster(&router, "widgets-bound").await;
    let role = given_role(&router, "appsync-rds-bound", Some(&allowing(&cluster.arn))).await;
    let api = create_api(&router, "bound", &cluster, &role).await;

    // Text that is itself SQL, and text carrying the quote a statement delimits text with:
    // each is a VALUE of the statement, so it matches the row whose name IS that text and
    // changes nothing about what the statement does.
    let rows: &[(&str, &str)] = &[
        ("a name carrying a quote", "it's a widget"),
        ("a name that is itself a statement", "'; DROP TABLE widgets; --"),
        ("a name carrying the wildcard a prefix match uses", "100% widget"),
        ("a name carrying no characters at all", ""),
    ];
    for (at, (case, name)) in rows.iter().enumerate() {
        let id = FIRST_BOUND_ID + at as i64;
        let added = post(
            case,
            &api,
            &json!({
                "query": "mutation Add($id: Int!, $name: String!) { \
                          addWidget(id: $id, name: $name) { id name } }",
                "variables": {"id": id, "name": name},
            }),
        )
        .await;
        assert_eq!(
            added["errors"].as_array(),
            None,
            "case {case:?}: a value the statement binds is written as it stands, got {added}"
        );
        assert_eq!(
            added["data"]["addWidget"]["name"],
            json!(name),
            "case {case:?}: expected the row to hold the name as it stands, got {added}"
        );
        // The table the statement would have dropped is still there, with every row it was
        // created with: the one that matters for the statement-shaped value, and asserted for
        // every row so no case can pass by having emptied the table.
        for (id, seeded) in SEEDED {
            assert_eq!(
                held(&router, &cluster, *id).await,
                Some((*seeded).to_string()),
                "case {case:?}: expected the database to still hold the row {id}"
            );
        }
        let found = post(
            case,
            &api,
            &json!({
                "query": "query Named($prefix: String!) { widgetsNamed(prefix: $prefix) \
                          { name } }",
                "variables": {"prefix": name},
            }),
        )
        .await;
        let names: Vec<String> = found["data"]["widgetsNamed"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|row| row["name"].as_str().map(str::to_string))
            .collect();
        assert!(
            names.contains(&(*name).to_string()),
            "case {case:?}: expected the row named {name:?} among {names:?}, got {found}"
        );
    }
}

#[tokio::test]
async fn a_mapping_template_resolver_reads_the_same_rows() {
    let router = emulator();
    let cluster = given_cluster(&router, "widgets-template").await;
    let role = given_role(&router, "appsync-rds-template", Some(&allowing(&cluster.arn))).await;
    let api = create_api(&router, "template", &cluster, &role).await;

    let case = "a resolver whose halves are Velocity templates";
    let answered = post(
        case,
        &api,
        &json!({"query": "query { widgetsTemplated(prefix: \"widget\") }"}),
    )
    .await;
    assert_eq!(
        answered["errors"].as_array(),
        None,
        "case {case:?}: a statement the database runs reports no error, got {answered}"
    );
    let reported: Value = answered["data"]["widgetsTemplated"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| answered["data"]["widgetsTemplated"].clone());
    assert_eq!(
        reported,
        json!([[
            {"id": 1, "name": "widget-alpha"},
            {"id": 2, "name": "widget-beta"},
        ]]),
        "case {case:?}: expected the rows of the one statement, got {answered}"
    );
}

#[tokio::test]
async fn the_documents_in_force_on_the_data_sources_role_decide_the_call() {
    let router = emulator();
    let cluster = given_cluster(&router, "widgets-guarded").await;
    let other = "arn:aws:rds:us-west-2:123456789012:cluster:elsewhere";

    /// One row: the document the data source's role carries, and whether the statements run
    /// under it.
    struct Row<'a> {
        case: &'a str,
        role: &'a str,
        policy: Option<String>,
        ran: bool,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a role allowed the Data API action on this cluster",
            role: "allowed",
            policy: Some(allowing(&cluster.arn)),
            ran: true,
        },
        Row {
            case: "a role allowed it on every resource",
            role: "wildcard",
            policy: Some(statement("Allow", "rds-data:ExecuteStatement", "*")),
            ran: true,
        },
        Row {
            case: "a role whose allow names another cluster",
            role: "other-cluster",
            policy: Some(statement("Allow", "rds-data:ExecuteStatement", other)),
            ran: false,
        },
        Row {
            case: "a role whose allow names another action",
            role: "other-action",
            policy: Some(statement("Allow", "rds-data:BeginTransaction", &cluster.arn)),
            ran: false,
        },
        Row {
            case: "a role whose documents deny it explicitly beside a wildcard allow",
            role: "denied",
            policy: Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Effect\":\"Allow\",\"Action\":\"rds-data:*\",\"Resource\":\"*\"}},\
                 {{\"Effect\":\"Deny\",\"Action\":\"rds-data:ExecuteStatement\",\
                 \"Resource\":\"{}\"}}]}}",
                cluster.arn
            )),
            ran: false,
        },
        Row {
            // A role IAM HOLDS is the authority on what may be done under it, so an action
            // nothing in force on it allows is refused — an EMPTY set of documents allows
            // nothing. (A role IAM holds nothing under is the case that says nothing, since
            // a service role is routinely provisioned out of band.)
            case: "a role carrying no identity-based document at all",
            role: "undocumented",
            policy: None,
            ran: false,
        },
    ];
    for row in &rows {
        let case = row.case;
        let role = given_role(&router, &format!("appsync-rds-{}", row.role), row.policy.as_deref())
            .await;
        let api = create_api(&router, row.role, &cluster, &role).await;
        let answered = post(case, &api, &json!({"query": "query { widget(id: 1) { name } }"})).await;
        if row.ran {
            assert_eq!(
                answered["data"]["widget"],
                json!({"name": "widget-alpha"}),
                "case {case:?}: expected the statement to run, got {answered}"
            );
            continue;
        }
        assert_eq!(
            answered["errors"][0]["errorType"].as_str(),
            Some(ACCESS_DENIED_ERROR_TYPE),
            "case {case:?}: expected the refused call to be reported to the caller, got \
             {answered}"
        );
        assert_eq!(
            answered["data"]["widget"],
            Value::Null,
            "case {case:?}: a field whose call was refused carries no value, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_call_this_data_source_cannot_make_is_reported_to_the_caller() {
    let router = emulator();
    let cluster = given_cluster(&router, "widgets-refused").await;
    let role = given_role(&router, "appsync-rds-refused", Some(&allowing(&cluster.arn))).await;
    let api = create_api(&router, "refused", &cluster, &role).await;

    // One row: the resolver attached to the field, the text its refusal must state, and the
    // class that refusal is of — the resolver's own CODE where its request is one this data
    // source cannot act on, and the called service's own class where the DATABASE refused the
    // statement.
    let rows: &[(&str, Value, &str, &str)] = &[
        (
            "a request naming a statement version this data source is not written to",
            json!({
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": "export function request(ctx) {\n\
                         return {version: '2017-02-28', statements: ['SELECT 1']};\n\
                         }\n\
                         export function response(ctx) { return ctx.result; }",
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            }),
            "2018-05-29",
            REQUEST_ERROR_TYPE,
        ),
        (
            "a request carrying no statement at all",
            json!({
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": "export function request(ctx) { return {statements: []}; }\n\
                         export function response(ctx) { return ctx.result; }",
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            }),
            "statements",
            REQUEST_ERROR_TYPE,
        ),
        (
            "a statement the database raises on",
            json!({
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": "export function request(ctx) {\n\
                         return {statements: ['SELECT * FROM no_such_table']};\n\
                         }\n\
                         export function response(ctx) { return ctx.result; }",
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            }),
            "no_such_table",
            ANSWERED_ERROR_TYPE_PREFIX,
        ),
    ];
    for (case, resolver, stated, class) in rows {
        let (status, updated) = appsync(
            &router,
            "POST",
            &format!("/v1/apis/{}/types/Query/resolvers/widget", api.api_id),
            Some(resolver.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateResolver must answer 200, got {updated}"
        );
        let answered = post(case, &api, &json!({"query": "query { widget(id: 1) { name } }"})).await;
        let message = answered["errors"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(stated),
            "case {case:?}: expected the refusal to state {stated:?}, got {answered}"
        );
        let reported = answered["errors"][0]["errorType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            reported.starts_with(class),
            "case {case:?}: expected a refusal of the class {class:?}, got {answered}"
        );
        assert_eq!(
            answered["data"]["widget"],
            Value::Null,
            "case {case:?}: a field that did not resolve carries no value, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_relational_database_that_names_no_endpoint_runs_no_statement() {
    let router = emulator();
    let cluster = given_cluster(&router, "widgets-unconfigured").await;
    let role = given_role(&router, "appsync-rds-unconfigured", Some(&allowing(&cluster.arn)))
        .await;
    let api = create_api(&router, "unconfigured", &cluster, &role).await;

    // A configuration the CONTROL plane will not hold at all: no cluster to address, and no
    // source type naming how the database is reached. Each repointing is refused, and the
    // data source goes on running the statements it was created for.
    let refused: &[(&str, Value)] = &[
        (
            "a relational database naming no cluster",
            json!({
                "relationalDatabaseSourceType": "RDS_HTTP_ENDPOINT",
                "rdsHttpEndpointConfig": {
                    "awsRegion": REGION,
                    "databaseName": DATABASE,
                    "awsSecretStoreArn": "arn:aws:secretsmanager:us-west-2:1:secret:none",
                },
            }),
        ),
        (
            "a relational database naming no source type",
            json!({
                "rdsHttpEndpointConfig": {
                    "awsRegion": REGION,
                    "dbClusterIdentifier": "arn:aws:rds:us-west-2:1:cluster:none",
                    "databaseName": DATABASE,
                    "awsSecretStoreArn": "arn:aws:secretsmanager:us-west-2:1:secret:none",
                },
            }),
        ),
    ];
    for (case, config) in refused {
        let (status, updated) = repoint(&router, &api, &role, config).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the repointing to be refused, got {status} {updated}"
        );
        let answered = post(case, &api, &json!({"query": "query { widget(id: 1) { name } }"})).await;
        assert_eq!(
            answered["data"]["widget"],
            json!({"name": "widget-alpha"}),
            "case {case:?}: expected the data source to be unchanged, got {answered}"
        );
    }

    // And a configuration the control plane DOES hold, which still names no endpoint a
    // statement can be run through: the step is refused where the call would be made, naming
    // the setting to correct, and no statement runs.
    let unusable: &[(&str, Value, &str)] = &[
        (
            "a relational database naming no secret to authenticate with",
            json!({
                "relationalDatabaseSourceType": "RDS_HTTP_ENDPOINT",
                "rdsHttpEndpointConfig": {
                    "awsRegion": REGION,
                    "dbClusterIdentifier": cluster.arn,
                    "databaseName": DATABASE,
                },
            }),
            "awsSecretStoreArn",
        ),
        (
            "a relational database naming no database to run against",
            json!({
                "relationalDatabaseSourceType": "RDS_HTTP_ENDPOINT",
                "rdsHttpEndpointConfig": {
                    "awsRegion": REGION,
                    "dbClusterIdentifier": cluster.arn,
                    "awsSecretStoreArn": cluster.secret_arn,
                },
            }),
            "databaseName",
        ),
    ];
    for (case, config, named) in unusable {
        let (status, updated) = repoint(&router, &api, &role, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateDataSource must answer 200, got {updated}"
        );
        let answered = post(case, &api, &json!({"query": "query { widget(id: 1) { name } }"})).await;
        let error = &answered["errors"][0];
        let message = error["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {answered}"
        );
        assert_eq!(
            error["errorType"].as_str(),
            Some(REQUEST_ERROR_TYPE),
            "case {case:?}: expected the class of a request this data source cannot act on, \
             got {answered}"
        );
        assert_eq!(
            answered["data"]["widget"],
            Value::Null,
            "case {case:?}: a field that did not resolve carries no value, got {answered}"
        );
    }
}

/// Repoint this suite's data source at `config`, answering what the control plane said.
async fn repoint(
    router: &MultiServiceRouter,
    api: &Api,
    role_arn: &str,
    config: &Value,
) -> (u16, Value) {
    appsync(
        router,
        "POST",
        &format!("/v1/apis/{}/datasources/{DATA_SOURCE}", api.api_id),
        Some(json!({
            "type": "RELATIONAL_DATABASE",
            "serviceRoleArn": role_arn,
            "relationalDatabaseConfig": config,
        })),
    )
    .await
}

/// The id the first row of the case about bound values is inserted under, past every id
/// [`SEEDED`] holds.
const FIRST_BOUND_ID: i64 = 10;

/// A router with AppSync, the Amazon RDS control plane, the Data API, Secrets Manager, KMS and
/// IAM over ONE engine: the cluster the data source names, the database its statements run
/// against, the secret the call authenticates with and the documents it is authorized against
/// all belong to those services.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_rdsdata_rdsdataservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One cluster of this suite: the ARN a data source names it by and the ARN of the secret
/// holding its login.
pub(crate) struct Cluster {
    pub(crate) arn: String,
    pub(crate) secret_arn: String,
}

/// Create one DB cluster through the RDS control plane's own `CreateDBCluster`, with its HTTP
/// endpoint on and RDS managing the master user password — so the secret is RDS's own — and
/// create [`TABLE`] with the rows of [`SEEDED`] in it through the Data API's own
/// `ExecuteStatement`.
pub(crate) async fn given_cluster(router: &MultiServiceRouter, identifier: &str) -> Cluster {
    let created = rds(router)
        .create_db_cluster()
        .db_cluster_identifier(identifier)
        .engine(ENGINE)
        .master_username(MASTER_USER)
        .manage_master_user_password(true)
        .database_name(DATABASE)
        .enable_http_endpoint(true)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!(
                "CreateDBCluster for {identifier} must be served: {}",
                aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        });
    let cluster = created
        .db_cluster()
        .unwrap_or_else(|| panic!("CreateDBCluster for {identifier} reports its cluster"));
    let held = Cluster {
        arn: cluster
            .db_cluster_arn()
            .unwrap_or_else(|| panic!("a created cluster carries its ARN"))
            .to_string(),
        secret_arn: cluster
            .master_user_secret()
            .and_then(aws_sdk_rds::types::MasterUserSecret::secret_arn)
            .unwrap_or_else(|| panic!("a managed-password cluster reports its secret"))
            .to_string(),
    };
    execute(
        router,
        &held,
        &format!(
            "CREATE TABLE {TABLE} (id INT NOT NULL PRIMARY KEY, name VARCHAR(64) NOT NULL)"
        ),
    )
    .await;
    for (id, name) in SEEDED {
        execute(
            router,
            &held,
            &format!("INSERT INTO {TABLE} (id, name) VALUES ({id}, '{name}')"),
        )
        .await;
    }
    held
}

/// The name the database itself holds for the row `id`, read through the service that owns the
/// database, and `None` for a row it does not hold.
async fn held(router: &MultiServiceRouter, cluster: &Cluster, id: i64) -> Option<String> {
    let answered = execute(
        router,
        cluster,
        &format!("SELECT name FROM {TABLE} WHERE id = {id}"),
    )
    .await;
    answered["records"][0][0]["stringValue"]
        .as_str()
        .map(str::to_string)
}

/// Run one statement against `cluster` through the Data API, which must serve it.
pub(crate) async fn execute(router: &MultiServiceRouter, cluster: &Cluster, sql: &str) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://rds-data.{REGION}.amazonaws.com/Execute"))
        .header("host", format!("rds-data.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/rds-data/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            json!({
                "resourceArn": cluster.arn,
                "secretArn": cluster.secret_arn,
                "database": DATABASE,
                "sql": sql,
                "includeResultMetadata": true,
            })
            .to_string(),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "the Data API must serve {sql:?} of this suite, got {status} with {body}"
    );
    body
}

/// An Amazon RDS client over this emulator's own in-process callback.
fn rds(router: &MultiServiceRouter) -> aws_sdk_rds::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_rds::Config::builder()
        .behavior_version(aws_sdk_rds::config::BehaviorVersion::latest())
        .region(aws_sdk_rds::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://rds.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_rds::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_rds::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_rds::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_rds::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_rds::Client::from_conf(config)
}

/// One created GraphQL API of this suite: its id, the address its `GRAPHQL` endpoint is served
/// on and a live API key to present to it.
struct Api {
    api_id: String,
    graphql: String,
    api_key: String,
}

/// Create one GraphQL API with an API key, [`SCHEMA`], a `RELATIONAL_DATABASE` data source on
/// `cluster` under `role`, and every resolver this suite drives.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    cluster: &Cluster,
    role_arn: &str,
) -> Api {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("rds-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    // The address a client dials is the one the API PUBLISHES for its GraphQL endpoint: the
    // suite's socket client takes the authority out of that URL, so every case below drives
    // exactly what `uris.GRAPHQL` told a client to use.
    let published = api["uris"]["GRAPHQL"]
        .as_str()
        .unwrap_or_else(|| panic!("a created API publishes its GRAPHQL endpoint, got {created}"))
        .to_string();
    let graphql = authority_of(&published);
    assert_eq!(
        api["dns"]["GRAPHQL"].as_str(),
        Some(::service_common::data_plane_addressing::host_only(&graphql)),
        "the API's DNS record is the bare host of the URL it published, got {created}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    create_schema(router, &api_id).await;
    let (status, data_source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "RELATIONAL_DATABASE",
            "serviceRoleArn": role_arn,
            "relationalDatabaseConfig": {
                "relationalDatabaseSourceType": "RDS_HTTP_ENDPOINT",
                "rdsHttpEndpointConfig": {
                    "awsRegion": REGION,
                    "dbClusterIdentifier": cluster.arn,
                    "databaseName": DATABASE,
                    "awsSecretStoreArn": cluster.secret_arn,
                },
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateDataSource must answer 200, got {data_source}"
    );
    for (type_name, resolver) in resolvers() {
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            Some(resolver.clone()),
        )
        .await;
        assert_eq!(status, 200, "CreateResolver must answer 200, got {created}");
    }
    Api {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// The authority a client dials out of a published URL: everything between the scheme and the
/// resource path, which is what a socket is opened on.
pub(crate) fn authority_of(published: &str) -> String {
    published
        .split_once("://")
        .map_or(published, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Every resolver this suite attaches, as the type it belongs to and the create call that
/// attaches it.
fn resolvers() -> Vec<(&'static str, Value)> {
    let coded = |type_name: &'static str, field: &str, request: &str, response: String| {
        (
            type_name,
            json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": format!("{IMPORTS}{request}{response}"),
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            }),
        )
    };
    vec![
        coded(
            "Query",
            "widget",
            "export function request(ctx) {\n\
             return createMySQLStatement(select({\n\
             table: 'widgets',\n\
             columns: ['id', 'name'],\n\
             where: {id: {eq: ctx.args.id}},\n\
             limit: 1,\n\
             }));\n\
             }\n",
            responds_with_rows(0, true),
        ),
        coded(
            "Query",
            "widgetsNamed",
            "export function request(ctx) {\n\
             return createMySQLStatement(select({\n\
             table: 'widgets',\n\
             columns: ['id', 'name'],\n\
             where: {name: {beginsWith: ctx.args.prefix}},\n\
             orderBy: [{column: 'id', dir: 'ASC'}],\n\
             }));\n\
             }\n",
            responds_with_rows(0, false),
        ),
        coded(
            "Query",
            "widgetByText",
            "export function request(ctx) {\n\
             return createMySQLStatement(\n\
             sql`select id, name from widgets where name = ${ctx.args.name}`\n\
             );\n\
             }\n",
            responds_with_rows(0, true),
        ),
        coded(
            "Mutation",
            "addWidget",
            "export function request(ctx) {\n\
             return createMySQLStatement(\n\
             insert({table: 'widgets', values: {id: ctx.args.id, name: ctx.args.name}}),\n\
             select({\n\
             table: 'widgets',\n\
             columns: ['id', 'name'],\n\
             where: {id: {eq: ctx.args.id}},\n\
             })\n\
             );\n\
             }\n",
            responds_with_rows(1, true),
        ),
        coded(
            "Mutation",
            "renameWidget",
            "export function request(ctx) {\n\
             return createMySQLStatement(\n\
             update({\n\
             table: 'widgets',\n\
             values: {name: ctx.args.name},\n\
             where: {id: {eq: ctx.args.id}},\n\
             }),\n\
             select({\n\
             table: 'widgets',\n\
             columns: ['id', 'name'],\n\
             where: {id: {eq: ctx.args.id}},\n\
             })\n\
             );\n\
             }\n",
            responds_with_rows(1, true),
        ),
        coded(
            "Mutation",
            "removeWidget",
            "export function request(ctx) {\n\
             return createMySQLStatement(\n\
             remove({table: 'widgets', where: {id: {eq: ctx.args.id}}})\n\
             );\n\
             }\n",
            // The count of rows the statement changed, which is the Data API's own report of
            // what a delete did.
            "export function response(ctx) {\n\
             if (ctx.error) { util.error(ctx.error.message, ctx.error.type); }\n\
             return ctx.result.sqlStatementResults[0].numberOfRecordsUpdated;\n\
             }\n"
                .to_string(),
        ),
        // A resolver whose halves are VELOCITY templates: the request names its statement and
        // the value its placeholder stands for, and the response reads the rows through the
        // same helper the coded resolvers reach.
        (
            "Query",
            json!({
                "fieldName": "widgetsTemplated",
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate":
                    "{\"version\": \"2018-05-29\", \"statements\": [\"select id, name from \
                     widgets where name like :PREFIX order by id\"], \"variableMap\": \
                     {\":PREFIX\": $util.toJson(\"${ctx.args.prefix}%\")}}",
                "responseMappingTemplate":
                    "#if($ctx.error)$util.error($ctx.error.message, $ctx.error.type)#end\n\
                     $util.toJson($util.rds.toJsonObject($ctx.result))",
            }),
        ),
    ]
}

/// Install [`SCHEMA`] on `api_id`, waiting for the clock to perform the creation.
async fn create_schema(router: &MultiServiceRouter, api_id: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(SCHEMA);
    let (status, started) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": definition})),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSchemaCreation must answer 200, got {started}"
    );
    let deadline = tokio::time::Instant::now() + CREATION_DEADLINE;
    loop {
        let (status, settled) = appsync(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSchemaCreationStatus must answer 200, got {settled}"
        );
        if settled["status"].as_str() == Some("SUCCESS") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the schema creation must be performed, got {settled}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// POST one GraphQL request to the address the API published, as a client does, and answer the
/// document it was served.
async fn post(case: &str, api: &Api, document: &Value) -> Value {
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        document,
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

/// Create an IAM role called `role_name` carrying `policy` as its inline policy (when one is
/// given), through IAM's OWN operations, and answer its ARN.
pub(crate) async fn given_role(
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

/// The document allowing the Data API action on one cluster and nothing else.
pub(crate) fn allowing(cluster_arn: &str) -> String {
    statement("Allow", "rds-data:ExecuteStatement", cluster_arn)
}

/// One-statement identity-based document.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": action, "Resource": resource}],
    })
    .to_string()
}

/// One awsQuery call to IAM, signed with this suite's own credential; answers the XML body,
/// having insisted the call succeeded.
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
