// SPDX-License-Identifier: CC0-1.0

//! WHICH VOICE a refusal of this service speaks in, end to end over the restJson1 wire:
//! the member's own AppSync sentence, or the generic constraint report the validation
//! layer in front of the service builds.
//!
//! The rule under test, one level above the six requests that filed the issue:
//!
//! > A member whose refusal AppSync WORDS ITSELF is refused with that sentence — its fixed
//! > text exactly, and its variable parts (the block a request left out, the bound it
//! > crossed, the direction a template is written for, the limit in the unit the service
//! > states it in) taken from the request and from the model. A member whose refusal is
//! > AWS's generic CONSTRAINT REPORT is refused with that report — `N validation error(s)
//! > detected: Value at '<member path>' failed to satisfy constraint: <requirement>`,
//! > naming the member path the walk reports and the requirement the model declares. Which
//! > of the two a member takes is a property of the member AND of the operation carrying
//! > it, so one shape reached by two operations can take one voice on each.
//!
//! The rows differ along what the rule generalises over: the MEMBER, the OPERATION carrying
//! it (every rule is asserted on each operation that carries its member, and on both the
//! create and the update where the pair shares the reading), WHICH bound or block the
//! request got wrong, and HOW FAR past the bound the value is. Each rule also carries its
//! served rows — each published bound included — so a refusal is about the value rather
//! than about the operation, and the EDGE rows that hold the two voices apart: the member
//! of another surface that shares a shape with a member under test keeps ITS voice.
//!
//! The harness (`call`, `router`, `Answer`, `create_api`) is the error-contract suite's;
//! the label encoder is the resource-tag suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};
use crate::resource_tags::{encode_label, ACCOUNT};

/// AppSync's own modelled client error for a request that is not well formed
/// (`smithy_query(query="shape", shape="BadRequestException")`, `@httpError` 400).
const BAD_REQUEST: &str = "BadRequestException";

/// The error code the validation layer in front of the service names a constraint refusal
/// with, which is what the `/v2/apis…` surface's recordings show for its page size.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status every refusal below takes.
const REFUSED: u16 = 400;

/// The Region every ARN below is written in.
const REGION: &str = "eu-west-1";

/// `TTL`'s published bounds (`@range` 0..3600 — "the maximum value is one hour (3600
/// seconds)", `smithy_query(query="shape", shape="TTL")`).
const TTL_MIN: i64 = 0;
const TTL_MAX: i64 = 3_600;

/// `MappingTemplate`'s published maximum length (`@length` 1..65536,
/// `smithy_query(query="shape", shape="MappingTemplate")`).
const TEMPLATE_MAX_LEN: usize = 65_536;

/// `Code`'s published length bounds (`@length` 1..32768,
/// `smithy_query(query="shape", shape="Code")`).
const CODE_MIN_LEN: usize = 1;
const CODE_MAX_LEN: usize = 32_768;

/// `Context`'s published length bounds (`@length` 2..28000,
/// `smithy_query(query="shape", shape="Context")`).
const CONTEXT_MIN_LEN: usize = 2;
const CONTEXT_MAX_LEN: usize = 28_000;

/// `MaxResults`'s published bounds (`@range` 0..25,
/// `smithy_query(query="shape", shape="MaxResults")`).
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 25;

/// `RdsDataApiConfig$resourceArn`'s published expression, transcribed from the model
/// (`smithy_query(query="shape", shape="RdsDataApiConfig")`) rather than computed, so the
/// suite states the published pattern independently of the code that quotes it.
const CLUSTER_ARN_PATTERN: &str = r"^arn:[a-z-]*:rds:[a-z0-9-]*:\d{12}:cluster:[0-9A-Za-z_/-]*$";

/// The only runtime AppSync supports, as `AppSyncRuntime` requires both its members.
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The report a refusal states for one broken requirement of `member`, with the submitted
/// value withheld.
fn report(member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The report a refusal states for one broken requirement of `member`, quoting the value
/// that broke it — the form this service's recordings show for the introspection
/// configuration's members.
fn report_quoting(member: &str, value: &str, requirement: &str) -> String {
    report_over(&[clause(member, value, requirement)])
}

/// One clause of a report that quotes the value it refused.
fn clause(member: &str, value: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The envelope a report of `clauses` carries: the count, and the clauses in the order the
/// service recites them, joined by `; `.
fn report_over(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The requirement a clause states for a value outside `pattern`.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The requirement a clause states for a value past a declared maximum length.
fn max_length_requirement(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a clause states for a value short of a declared minimum length.
fn min_length_requirement(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// Assert one refusal: its status, its error code and its whole message.
fn assert_refused(case: &str, code: &str, expected: &str, answer: &Answer) {
    assert_eq!(
        (
            answer.status,
            answer.error_type.as_str(),
            answer.message.as_str()
        ),
        (REFUSED, code, expected),
        "case {case:?}: expected {REFUSED} {code} {expected:?}, got {answer:?}"
    );
}

// =============================================================================
// Rule 1: a nested settings block the declared source type is set up through
// =============================================================================

/// A `relationalDatabaseConfig` naming the RDS HTTP endpoint source type, with the endpoint
/// settings `endpoint` states (`None` for a request that carries no block at all).
fn relational_config(endpoint: Option<Value>) -> Value {
    let mut config = json!({"relationalDatabaseSourceType": "RDS_HTTP_ENDPOINT"});
    if let Some(endpoint) = endpoint {
        config["rdsHttpEndpointConfig"] = endpoint;
    }
    config
}

/// A `CreateDataSource`/`UpdateDataSource` body for a relational database over `config`.
fn relational_body(name: &str, config: Value) -> Value {
    json!({
        "name": name,
        "type": "RELATIONAL_DATABASE",
        "serviceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/appsync-rds"),
        "relationalDatabaseConfig": config,
    })
}

/// The settings block a reachable RDS HTTP endpoint names: the cluster and its Region.
fn reachable_endpoint() -> Value {
    json!({
        "awsRegion": REGION,
        "dbClusterIdentifier": "orders-cluster",
        "databaseName": "orders",
        "awsSecretStoreArn": format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:db"),
    })
}

/// RULE 1: a relational-database configuration that names the RDS HTTP endpoint source type
/// and carries NO settings block for it names the block to add; one that carries the block
/// but names no endpoint inside it is refused for the endpoint. Asserted on both operations
/// that write a data source.
#[tokio::test]
async fn a_source_type_whose_settings_block_is_absent_names_that_block() {
    let router = router();
    let api_id = create_api(&router, "Api_rds_source_block").await;
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(relational_body(
            "orders",
            relational_config(Some(reachable_endpoint())),
        )),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case a configuration naming its cluster and Region: expected 200, got {created:?}"
    );

    // The rows differ along WHAT the configuration leaves out: the whole block, or one of
    // the two members inside it that name where the endpoint is.
    let rows: Vec<(&str, Option<Value>, &str)> = vec![
        ("no settings block at all", None, "Missing RDS Http config"),
        (
            "a settings block that names nothing",
            Some(json!({})),
            "Invalid endpoint configuration.",
        ),
        (
            "a settings block naming no cluster",
            Some(json!({"awsRegion": REGION})),
            "Invalid endpoint configuration.",
        ),
        (
            "a settings block naming no Region",
            Some(json!({"dbClusterIdentifier": "orders-cluster"})),
            "Invalid endpoint configuration.",
        ),
        (
            "a settings block naming an empty cluster",
            Some(json!({"awsRegion": REGION, "dbClusterIdentifier": ""})),
            "Invalid endpoint configuration.",
        ),
    ];

    for (case, endpoint, expected) in rows {
        for (operation, path, name) in [
            (
                "CreateDataSource",
                format!("/v1/apis/{api_id}/datasources"),
                "ledger",
            ),
            (
                "UpdateDataSource",
                format!("/v1/apis/{api_id}/datasources/orders"),
                "orders",
            ),
        ] {
            let case = format!("{operation} with {case}");
            let answer = call(
                &router,
                "POST",
                &path,
                Some(relational_body(name, relational_config(endpoint.clone()))),
            )
            .await;
            assert_refused(&case, BAD_REQUEST, expected, &answer);
        }
    }

    // Neither refused write changed the stored data source, and neither refused create
    // left one behind.
    let listed = call(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/datasources"),
        None,
    )
    .await;
    let names: Vec<&str> = listed.body["dataSources"]
        .as_array()
        .map(|sources| {
            sources
                .iter()
                .filter_map(|source| source["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["orders"],
        "case the store after the refusals: expected only the accepted data source, got \
         {listed:?}"
    );
}

// =============================================================================
// Rule 2: a Lambda authorizer's cached-result lifetime
// =============================================================================

/// A `CreateGraphqlApi`/`UpdateGraphqlApi` body authenticating with `AWS_LAMBDA`, whose
/// authorizer caches its result for `ttl` seconds — on the API's own configuration, or on
/// an additional provider when `on_provider`.
fn lambda_authorizer_body(name: &str, ttl: i64, on_provider: bool) -> Value {
    let config = json!({
        "authorizerUri": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:authorize"),
        "authorizerResultTtlInSeconds": ttl,
    });
    if on_provider {
        return json!({
            "name": name,
            "authenticationType": "API_KEY",
            "additionalAuthenticationProviders": [
                {"authenticationType": "AWS_LAMBDA", "lambdaAuthorizerConfig": config},
            ],
        });
    }
    json!({
        "name": name,
        "authenticationType": "AWS_LAMBDA",
        "lambdaAuthorizerConfig": config,
    })
}

/// RULE 2: a cached-result lifetime outside the window `TTL` declares names the lifetime and
/// the bound the value crossed, wherever in the request the authorizer sits and whichever
/// operation carries it; a lifetime inside the window — each published bound included — is
/// served.
#[tokio::test]
async fn an_authorizer_lifetime_outside_its_window_names_the_bound_it_crossed() {
    let router = router();
    let api_id = create_api(&router, "Api_authorizer_ttl").await;

    // The rows differ along WHICH bound the value crosses and how far past it it is.
    let rows: Vec<(&str, i64, String)> = vec![
        (
            "one second past the maximum",
            TTL_MAX + 1,
            format!("Authorizer result TTL cannot be > {TTL_MAX}"),
        ),
        (
            "two hours, well past the maximum",
            7_200,
            format!("Authorizer result TTL cannot be > {TTL_MAX}"),
        ),
        (
            "one second below the minimum",
            TTL_MIN - 1,
            format!("Authorizer result TTL cannot be < {TTL_MIN}"),
        ),
        (
            "far below the minimum",
            -86_400,
            format!("Authorizer result TTL cannot be < {TTL_MIN}"),
        ),
    ];

    for (case, ttl, expected) in rows {
        for on_provider in [false, true] {
            let carried = if on_provider {
                "an additional provider's authorizer"
            } else {
                "the API's own authorizer"
            };
            for (operation, path) in [
                ("CreateGraphqlApi", "/v1/apis".to_string()),
                ("UpdateGraphqlApi", format!("/v1/apis/{api_id}")),
            ] {
                let case = format!("{operation} with {case} on {carried}");
                let body = lambda_authorizer_body("Api_ttl_row", ttl, on_provider);
                let answer = call(&router, "POST", &path, Some(body)).await;
                assert_refused(&case, BAD_REQUEST, &expected, &answer);
            }
        }
    }

    // Each published bound, and a value between them, is served.
    for ttl in [TTL_MIN, TTL_MAX, 1_800] {
        let answer = call(
            &router,
            "POST",
            "/v1/apis",
            Some(lambda_authorizer_body(&format!("Api_ttl_{ttl}"), ttl, false)),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case CreateGraphqlApi with a lifetime of {ttl}: expected the value inside the \
             window to be served, got {answer:?}"
        );
        assert_eq!(
            answer.body["graphqlApi"]["lambdaAuthorizerConfig"]["authorizerResultTtlInSeconds"],
            json!(ttl),
            "case CreateGraphqlApi with a lifetime of {ttl}: expected the value back, got \
             {answer:?}"
        );
    }
}

// =============================================================================
// Rule 3: a mapping template past its size ceiling
// =============================================================================

/// A template of exactly `length` characters, built from the lines a real VTL template
/// carries so the case exercises newlines as well as size.
fn template_of(length: usize) -> String {
    let mut text = String::new();
    while text.chars().count() < length {
        text.push_str("#if($ctx.args.id)\n$util.toJson($ctx.result)\n#end\n");
    }
    text.chars().take(length).collect()
}

/// The sentence AppSync states a template past its ceiling with, for the `direction` the
/// member is written for.
fn template_size_sentence(direction: &str) -> String {
    format!(
        "Invalid {direction} template given, must be less than {}kb in size.",
        TEMPLATE_MAX_LEN / 1_024
    )
}

/// One operation that carries a pair of mapping templates: the label, the path, and the
/// body members it needs besides the templates.
fn template_carriers(api_id: &str, data_source: &str) -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "CreateFunction",
            format!("/v1/apis/{api_id}/functions"),
            json!({"name": "resolve_orders", "dataSourceName": data_source, "functionVersion": "2018-05-29"}),
        ),
        (
            "UpdateFunction",
            format!("/v1/apis/{api_id}/functions/absentfunction"),
            json!({"name": "resolve_orders", "dataSourceName": data_source, "functionVersion": "2018-05-29"}),
        ),
        (
            "CreateResolver",
            format!("/v1/apis/{api_id}/types/Query/resolvers"),
            json!({"fieldName": "orders", "dataSourceName": data_source}),
        ),
        (
            "UpdateResolver",
            format!("/v1/apis/{api_id}/types/Query/resolvers/orders"),
            json!({"dataSourceName": data_source}),
        ),
    ]
}

/// RULE 3: a mapping template past the ceiling `MappingTemplate` declares is refused with
/// the size sentence for the direction it is written for, on every operation that carries
/// one; a template AT the ceiling is served.
#[tokio::test]
async fn a_mapping_template_past_its_ceiling_states_the_size_sentence() {
    let router = router();
    let api_id = create_api(&router, "Api_template_size").await;
    let data_source = "orders";
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": data_source, "type": "NONE"})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed the data source: expected 200, got {created:?}"
    );

    // The rows differ along the DIRECTION the over-long template is written for and how far
    // past the ceiling it is.
    for (member, direction) in [
        ("requestMappingTemplate", "request"),
        ("responseMappingTemplate", "response"),
    ] {
        for over in [1, 1_024] {
            let template = template_of(TEMPLATE_MAX_LEN + over);
            for (operation, path, members) in template_carriers(&api_id, data_source) {
                let case = format!("{operation} with a {member} {over} past the ceiling");
                let mut body = members;
                body[member] = json!(template);
                // The counterpart template is inside the bound, so the row is refused for
                // the member it is about.
                let counterpart = if member == "requestMappingTemplate" {
                    "responseMappingTemplate"
                } else {
                    "requestMappingTemplate"
                };
                body[counterpart] = json!("$util.toJson($ctx.result)");
                let answer = call(&router, "POST", &path, Some(body)).await;
                assert_refused(
                    &case,
                    BAD_REQUEST,
                    &template_size_sentence(direction),
                    &answer,
                );
            }
        }
    }

    // A template AT the ceiling is served, so the rows above are refused for the size.
    let at_ceiling = template_of(TEMPLATE_MAX_LEN);
    let answer = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": "at_the_ceiling",
            "dataSourceName": data_source,
            "functionVersion": "2018-05-29",
            "requestMappingTemplate": at_ceiling,
            "responseMappingTemplate": at_ceiling,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case CreateFunction with templates at the ceiling: expected 200, got {}",
        answer.status
    );
}

// =============================================================================
// Rule 4: the members an evaluation is asked over
// =============================================================================

/// The operation that evaluates code: `POST /v1/dataplane-evaluatecode`.
const EVALUATE_CODE_PATH: &str = "/v1/dataplane-evaluatecode";

/// A module of exactly `length` characters that the APPSYNC_JS runtime can read: the
/// request/response pair a definition is made of, padded out with one trailing comment.
fn code_of(length: usize) -> String {
    let head = "export function request(ctx) { return {}; }\n\
                export function response(ctx) { return ctx.result; }\n// ";
    let mut code = String::from(head);
    code.push_str(&"a".repeat(length.saturating_sub(head.len())));
    code.chars().take(length).collect()
}

/// An `EvaluateCode` body over `code` and `context`.
fn evaluate_body(code: &str, context: &str) -> Value {
    json!({
        "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        "code": code,
        "context": context,
        "function": "request",
    })
}

/// RULE 4: the two members an evaluation is asked over are refused with the generic
/// constraint report — the member and the requirement, with nothing of the submitted value
/// quoted — while the operations that STORE a definition keep AppSync's own ceiling
/// sentence for the very same shape.
#[tokio::test]
async fn the_evaluated_members_state_the_constraint_report() {
    let router = router();

    // The rows differ along the MEMBER, WHICH bound it crosses and how far past it the
    // value is.
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a code one character past the ceiling",
            code_of(CODE_MAX_LEN + 1),
            "{}".to_string(),
            report(
                "code",
                &format!("Member must have length less than or equal to {CODE_MAX_LEN}"),
            ),
        ),
        (
            "a code well past the ceiling",
            code_of(CODE_MAX_LEN + 4_096),
            "{}".to_string(),
            report(
                "code",
                &format!("Member must have length less than or equal to {CODE_MAX_LEN}"),
            ),
        ),
        (
            "an empty code",
            String::new(),
            "{}".to_string(),
            report(
                "code",
                &format!("Member must have length greater than or equal to {CODE_MIN_LEN}"),
            ),
        ),
        (
            "a context one character short of the minimum",
            code_of(128),
            "{".to_string(),
            report(
                "context",
                &format!("Member must have length greater than or equal to {CONTEXT_MIN_LEN}"),
            ),
        ),
        (
            "a context one character past the maximum",
            code_of(128),
            format!("{{\"a\":\"{}\"}}", "c".repeat(CONTEXT_MAX_LEN)),
            report(
                "context",
                &format!("Member must have length less than or equal to {CONTEXT_MAX_LEN}"),
            ),
        ),
    ];

    for (case, code, context, expected) in rows {
        let answer = call(
            &router,
            "POST",
            EVALUATE_CODE_PATH,
            Some(evaluate_body(&code, &context)),
        )
        .await;
        assert_refused(case, BAD_REQUEST, &expected, &answer);
        assert!(
            !answer.message.contains("aaaa") && !answer.message.contains("cccc"),
            "case {case:?}: expected no part of the submitted value quoted, got a message of \
             {} characters",
            answer.message.len()
        );
    }

    // Values INSIDE the bounds are not refused for their size: a code at the ceiling and a
    // context at the minimum are evaluated rather than refused.
    for (case, code, context) in [
        ("a code at the ceiling", code_of(CODE_MAX_LEN), "{}"),
        ("a context at the minimum", code_of(128), "{}"),
    ] {
        let answer = call(
            &router,
            "POST",
            EVALUATE_CODE_PATH,
            Some(evaluate_body(&code, context)),
        )
        .await;
        assert!(
            !answer.message.contains("failed to satisfy constraint"),
            "case {case:?}: expected no constraint refusal, got {answer:?}"
        );
    }

    // The EDGE the voices are held apart at: the same shape on an operation that STORES a
    // definition keeps AppSync's own ceiling sentence.
    let api_id = create_api(&router, "Api_code_voices").await;
    let seeded = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": "steps", "type": "NONE"})),
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "case seed the data source: expected 200, got {seeded:?}"
    );
    let answer = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": "over_the_ceiling",
            "dataSourceName": "steps",
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            "code": code_of(CODE_MAX_LEN + 1),
        })),
    )
    .await;
    assert_refused(
        "CreateFunction with a definition past the ceiling",
        BAD_REQUEST,
        &format!("Code must be {CODE_MAX_LEN} bytes or less"),
        &answer,
    );
}

// =============================================================================
// Rule 5: the nested ARN an introspection names its cluster by
// =============================================================================

/// A `StartDataSourceIntrospection` body naming its cluster by `resource_arn`.
fn introspection_body(resource_arn: &str) -> Value {
    introspection_config(
        resource_arn,
        &format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:db-creds"),
        "inventory",
    )
}

/// A `StartDataSourceIntrospection` body over the three members the configuration carries.
fn introspection_config(resource_arn: &str, secret_arn: &str, database_name: &str) -> Value {
    json!({
        "rdsDataApiConfig": {
            "resourceArn": resource_arn,
            "secretArn": secret_arn,
            "databaseName": database_name,
        }
    })
}

/// `RdsDataApiConfig$secretArn`'s published expression and minimum length, and
/// `$databaseName`'s published maximum, transcribed from the model
/// (`smithy_query(query="shape", shape="RdsDataApiConfig")`).
const SECRET_ARN_PATTERN: &str =
    r"^arn:[a-z-]*:secretsmanager:[a-z0-9-]*:\d{12}:secret:[0-9A-Za-z_/+=.@!-]*$";
const SECRET_ARN_MIN_LEN: usize = 20;
const DATABASE_NAME_MAX_LEN: usize = 128;

/// RULE 5: an introspection's cluster ARN outside the expression its member declares is
/// refused with the constraint report naming that member PATH and the expression, while the
/// TAGGING surface's member of the same name keeps AppSync's own malformed-ARN sentence.
#[tokio::test]
async fn the_nested_cluster_arn_states_the_constraint_report() {
    let router = router();
    let requirement = pattern_requirement(CLUSTER_ARN_PATTERN);

    // The rows differ along WHAT the value is instead of a cluster ARN: another RDS
    // resource type, another vendor, another partition's cluster written wrongly, and a
    // value that is no ARN at all. Each is long enough to clear the member's `@length`
    // minimum, so the row is refused for its FORM.
    let rows: Vec<(&str, String)> = vec![
        (
            "a DB instance ARN where a cluster ARN is required",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:orders-instance"),
        ),
        (
            "a DB snapshot ARN",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:orders-snapshot"),
        ),
        (
            "an ARN of another vendor",
            format!("arn:aws:redshift:{REGION}:{ACCOUNT}:cluster:orders-cluster"),
        ),
        (
            "a cluster ARN whose account field is not twelve digits",
            format!("arn:aws:rds:{REGION}:12345:cluster:orders-cluster"),
        ),
        (
            "a value that is no ARN at all",
            "orders-cluster-in-this-account-and-region".to_string(),
        ),
    ];

    for (case, resource_arn) in rows {
        let answer = call(
            &router,
            "POST",
            "/v1/datasources/introspections",
            Some(introspection_body(&resource_arn)),
        )
        .await;
        assert_refused(
            case,
            BAD_REQUEST,
            &report_quoting("rdsDataApiConfig.resourceArn", &resource_arn, &requirement),
            &answer,
        );
    }

    // A configuration whose cluster ARN names a cluster is served, so the rows above are
    // refused for the ARN rather than for the operation.
    let served = call(
        &router,
        "POST",
        "/v1/datasources/introspections",
        Some(introspection_body(&format!(
            "arn:aws:rds:{REGION}:{ACCOUNT}:cluster:orders-cluster"
        ))),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case a cluster ARN the expression admits: expected 200, got {served:?}"
    );

    // The EDGE: the TAGGING surface's own `resourceArn`, which shares the member NAME and
    // nothing else, keeps the sentence the service states a malformed resource ARN in.
    let answer = call(
        &router,
        "GET",
        &format!("/v1/tags/{}", encode_label("not-an-arn")),
        None,
    )
    .await;
    assert_refused(
        "ListTagsForResource with a value that is no ARN",
        BAD_REQUEST,
        "Invalid input resource arn",
        &answer,
    );
}

/// RULE 5's other half: a request that breaks SEVERAL of the configuration's requirements
/// is refused with every clause, in the order this surface recites them — the database the
/// introspection would read, then the secret that logs in to it, then the cluster its ARN
/// reaches, and within one member the FORM it must have before the SIZE it must be.
#[tokio::test]
async fn the_introspection_report_recites_every_broken_requirement_in_order() {
    let router = router();
    let bad_cluster = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:orders-instance");
    let good_cluster = format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:orders-cluster");
    let good_secret = format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:db-creds");
    // One character short of the member's minimum length, and no secret ARN either: one
    // value breaking two of one member's requirements.
    let short_secret = "arn:aws:secretsmana".to_string();
    let long_database = "d".repeat(DATABASE_NAME_MAX_LEN + 1);

    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "a member of each end of the configuration",
            introspection_config(&bad_cluster, &good_secret, &long_database),
            vec![
                clause(
                    "rdsDataApiConfig.databaseName",
                    &long_database,
                    &max_length_requirement(DATABASE_NAME_MAX_LEN),
                ),
                clause(
                    "rdsDataApiConfig.resourceArn",
                    &bad_cluster,
                    &pattern_requirement(CLUSTER_ARN_PATTERN),
                ),
            ],
        ),
        (
            "one member breaking both its form and its size",
            introspection_config(&good_cluster, &short_secret, "inventory"),
            vec![
                clause(
                    "rdsDataApiConfig.secretArn",
                    &short_secret,
                    &pattern_requirement(SECRET_ARN_PATTERN),
                ),
                clause(
                    "rdsDataApiConfig.secretArn",
                    &short_secret,
                    &min_length_requirement(SECRET_ARN_MIN_LEN),
                ),
            ],
        ),
        (
            "every member of the configuration at once",
            introspection_config(&bad_cluster, &short_secret, &long_database),
            vec![
                clause(
                    "rdsDataApiConfig.databaseName",
                    &long_database,
                    &max_length_requirement(DATABASE_NAME_MAX_LEN),
                ),
                clause(
                    "rdsDataApiConfig.secretArn",
                    &short_secret,
                    &pattern_requirement(SECRET_ARN_PATTERN),
                ),
                clause(
                    "rdsDataApiConfig.secretArn",
                    &short_secret,
                    &min_length_requirement(SECRET_ARN_MIN_LEN),
                ),
                clause(
                    "rdsDataApiConfig.resourceArn",
                    &bad_cluster,
                    &pattern_requirement(CLUSTER_ARN_PATTERN),
                ),
            ],
        ),
    ];

    for (case, body, clauses) in rows {
        let answer = call(
            &router,
            "POST",
            "/v1/datasources/introspections",
            Some(body),
        )
        .await;
        assert_refused(case, BAD_REQUEST, &report_over(&clauses), &answer);
    }
}

// =============================================================================
// Rule 6: the page size of the `/v2/apis…` surface
// =============================================================================

/// RULE 6: a page size outside the range `MaxResults` declares is refused by `ListApis` with
/// the constraint report under the validation layer's own error code, where the v1 list
/// operations state the service's page-size sentence; each published bound — the member's
/// own default of 0 included — is served.
#[tokio::test]
async fn the_v2_page_size_states_the_constraint_report() {
    let router = router();

    // The rows differ along WHICH bound the value crosses and how far past it it is.
    let rows: Vec<(&str, i64, String)> = vec![
        (
            "one past the maximum",
            MAX_RESULTS_MAX + 1,
            report(
                "maxResults",
                &format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
            ),
        ),
        (
            "far past the maximum",
            1_000,
            report(
                "maxResults",
                &format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
            ),
        ),
        (
            "one below the minimum",
            MAX_RESULTS_MIN - 1,
            report(
                "maxResults",
                &format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
            ),
        ),
        (
            "far below the minimum",
            -99,
            report(
                "maxResults",
                &format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
            ),
        ),
    ];

    for (case, value, expected) in rows {
        let answer = call(&router, "GET", &format!("/v2/apis?maxResults={value}"), None).await;
        assert_refused(case, VALIDATION_EXCEPTION, &expected, &answer);
    }

    // Each published bound, and a value between them, is served — the minimum included,
    // which is the member's own default.
    for value in [MAX_RESULTS_MIN, MAX_RESULTS_MAX, 7] {
        let answer = call(&router, "GET", &format!("/v2/apis?maxResults={value}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case ListApis with maxResults {value}: expected the value inside the range to be \
             served, got {answer:?}"
        );
    }

    // The EDGE: a v1 list operation's `maxResults` — the same shape, another surface —
    // keeps the service's own page-size sentence under its own error code.
    let answer = call(
        &router,
        "GET",
        &format!("/v1/apis?maxResults={}", MAX_RESULTS_MAX + 1),
        None,
    )
    .await;
    assert_refused(
        "ListGraphqlApis with a page size past the maximum",
        BAD_REQUEST,
        &format!(
            "maxResults must be greater than {MAX_RESULTS_MIN} and less than or equal to \
             {MAX_RESULTS_MAX}."
        ),
        &answer,
    );
}
