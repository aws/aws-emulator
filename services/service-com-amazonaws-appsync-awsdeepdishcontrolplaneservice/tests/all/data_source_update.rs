// SPDX-License-Identifier: CC0-1.0

//! `UpdateDataSource`, end to end over HTTP: what an update replaces, and which
//! updates it refuses.
//!
//! Three rules are asserted here, each over a TABLE spanning the class it generalises
//! over rather than the one request that exposed the defect:
//!
//! 1. every settable member of the request is "the new …" value, so an update
//!    REPLACES the stored data source's settings: the new `type`, its per-type
//!    configuration, `serviceRoleArn`, `metricsConfig` and a non-empty `description`
//!    are what the data source reports, every configuration the request did not carry
//!    is gone from it, and the `name` and ARN that identify the record are kept — and
//!    `GetDataSource` answers the same body the update did. Asserted over a table that
//!    walks one data source through every `DataSourceType` the model declares, with
//!    the `description` and `metricsConfig` states rotating across it;
//! 2. the data source an update describes has to be one AppSync could reach, judged
//!    the way the create judges it: a type that is set up through a configuration
//!    member is refused without that member (400 `BadRequestException`, naming the
//!    backend that configuration describes) and the stored data source is left exactly
//!    as it was, while a type configured through no member is accepted without any;
//! 3. completeness of the configuration is judged BEFORE the service role, so a
//!    request that carries neither takes the configuration's refusal.
//!
//! The service-role requirement on its own has its own suite,
//! `data_source_service_role.rs`, whose type catalog, per-type configurations and body
//! builder this suite reuses rather than copying.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::data_source_service_role::{
    config_for, get_data_source, reported_service_role, request_body, DATA_SOURCE_TYPES, ROLE_ARN,
};
use crate::error_contract::{call, create_api, router, Answer};
use crate::graphql_api_lifecycle::REGION;

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `description` states an update can leave behind: a description, an EMPTY
/// description, and no description at all.
const DESCRIPTIONS: &[Option<&str>] = &[Some("the new description"), Some(""), None];

/// The `metricsConfig` states an update can leave behind. Source: the
/// `DataSourceLevelMetricsConfig` enum (`ENABLED`, `DISABLED`), plus the request that
/// names none.
const METRICS_CONFIGS: &[Option<&str>] = &[Some("ENABLED"), Some("DISABLED"), None];

/// The text the configuration refusal carries for each configuration member of
/// `UpdateDataSourceRequest`: it names the backend that member's own doc names — "the
/// new Lambda configuration", "the new Amazon DynamoDB configuration", "the new
/// OpenSearch configuration" (both the deprecated `elasticsearchConfig` and
/// `openSearchServiceConfig`), "the new HTTP endpoint configuration", "the new
/// relational database configuration" and "the new Amazon EventBridge settings".
const MISSING_CONFIG_REFUSALS: &[(&str, &str)] = &[
    ("lambdaConfig", "Missing Lambda data source config"),
    ("dynamodbConfig", "Missing Amazon DynamoDB data source config"),
    ("elasticsearchConfig", "Missing OpenSearch data source config"),
    ("openSearchServiceConfig", "Missing OpenSearch data source config"),
    ("httpConfig", "Missing HTTP endpoint data source config"),
    (
        "relationalDatabaseConfig",
        "Missing relational database data source config",
    ),
    (
        "eventBridgeConfig",
        "Missing Amazon EventBridge data source config",
    ),
];

/// The refusal text for a data source left without `member`'s configuration.
fn missing_config_refusal(member: &str) -> &'static str {
    MISSING_CONFIG_REFUSALS
        .iter()
        .find(|(name, _)| *name == member)
        .map_or_else(
            || panic!("case {member}: this suite carries no refusal text for it"),
            |(_, text)| *text,
        )
}

/// Every configuration member a `DataSourceType` can be set up through, so a
/// projection can assert that the ones an update did not carry are gone.
fn every_config_member() -> Vec<String> {
    let mut members: Vec<String> = DATA_SOURCE_TYPES
        .iter()
        .filter_map(|(data_source_type, _)| config_member(data_source_type))
        .collect();
    members.sort();
    members.dedup();
    members
}

/// The configuration member a data source of this type is set up through, or `None`
/// for a type that is configured through none.
fn config_member(data_source_type: &str) -> Option<String> {
    config_for(data_source_type)
        .as_object()
        .and_then(|members| members.keys().next().cloned())
}

/// One row of the replacement table: the settings an update asks for.
#[derive(Debug, Clone)]
struct Row {
    data_source_type: &'static str,
    /// The `serviceRoleArn` the row sends — present exactly for a type AppSync must
    /// assume a role to reach, as the shared catalog records.
    role: Option<&'static str>,
    description: Option<&'static str>,
    metrics_config: Option<&'static str>,
}

impl Row {
    /// This row as an `UpdateDataSource` body for `name`.
    fn body(&self, name: &str) -> Value {
        let mut extra = json!({});
        let object = extra.as_object_mut().expect("the fragment is an object");
        if let Some(description) = self.description {
            object.insert("description".to_string(), json!(description));
        }
        if let Some(metrics_config) = self.metrics_config {
            object.insert("metricsConfig".to_string(), json!(metrics_config));
        }
        request_body(name, self.data_source_type, self.role, extra)
    }
}

/// The replacement table: every `DataSourceType` the model declares, with the
/// `description` and `metricsConfig` states rotating across the rows so each state is
/// exercised on several types.
fn rows() -> Vec<Row> {
    DATA_SOURCE_TYPES
        .iter()
        .enumerate()
        .map(|(index, (data_source_type, needs_role))| Row {
            data_source_type,
            role: needs_role.then_some(ROLE_ARN),
            description: DESCRIPTIONS[index % DESCRIPTIONS.len()],
            metrics_config: METRICS_CONFIGS[index % METRICS_CONFIGS.len()],
        })
        .collect()
}

/// Send one `UpdateDataSource`.
async fn update_data_source(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources/{name}"),
        Some(body),
    )
    .await
}

/// Create one data source of a type that needs neither a configuration nor a role, so
/// every rule under test is one the UPDATE introduces.
async fn create_bare_data_source(router: &MultiServiceRouter, api_id: &str, name: &str) -> Answer {
    let created = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(request_body(name, "NONE", None, json!({}))),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create {name:?}: expected 200, got {created:?}"
    );
    created
}

/// Assert `data_source` reports exactly what `row` asked for, under `api_id`.
fn assert_projects(row: &Row, api_id: &str, name: &str, data_source: &Value, source: &str) {
    assert_eq!(
        data_source["name"],
        json!(name),
        "case {row:?} ({source}): expected the name {name:?} to be kept, got {:?}",
        data_source["name"]
    );
    // The ARN identifies the record and no update sets it: a data source is named
    // under its API's own path, as `GetDataSource`'s URI addresses it.
    let expected_arn = format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{api_id}/datasources/{name}");
    assert_eq!(
        data_source["dataSourceArn"],
        json!(expected_arn),
        "case {row:?} ({source}): expected the ARN {expected_arn} to be kept, got {:?}",
        data_source["dataSourceArn"]
    );
    assert_eq!(
        data_source["type"],
        json!(row.data_source_type),
        "case {row:?} ({source}): expected type {}, got {:?}",
        row.data_source_type,
        data_source["type"]
    );
    match reported_service_role(row.data_source_type, row.role) {
        Some(role) => assert_eq!(
            data_source["serviceRoleArn"],
            json!(role),
            "case {row:?} ({source}): expected serviceRoleArn {role}, got {:?}",
            data_source["serviceRoleArn"]
        ),
        // Either the request named no role, or it named one for a type AppSync
        // reaches without assuming one, which keeps no service role.
        None => assert!(
            data_source["serviceRoleArn"].is_null(),
            "case {row:?} ({source}): expected no serviceRoleArn, got {:?}",
            data_source["serviceRoleArn"]
        ),
    }
    // A description is reported only where the request carried a non-empty one: an
    // empty description describes nothing, so the member is reported absent, exactly
    // as for a request that omitted it.
    match row.description.filter(|description| !description.is_empty()) {
        Some(description) => assert_eq!(
            data_source["description"],
            json!(description),
            "case {row:?} ({source}): expected description {description:?}, got {:?}",
            data_source["description"]
        ),
        None => assert!(
            data_source["description"].is_null(),
            "case {row:?} ({source}): expected no description, got {:?}",
            data_source["description"]
        ),
    }
    match row.metrics_config {
        Some(metrics_config) => assert_eq!(
            data_source["metricsConfig"],
            json!(metrics_config),
            "case {row:?} ({source}): expected metricsConfig {metrics_config}, got {:?}",
            data_source["metricsConfig"]
        ),
        None => assert!(
            data_source["metricsConfig"].is_null(),
            "case {row:?} ({source}): expected no metricsConfig, got {:?}",
            data_source["metricsConfig"]
        ),
    }
    // The configuration the request carried is reported member for member ...
    let carried = config_member(row.data_source_type);
    if let (Some(member), Some(sent)) = (
        carried.as_deref(),
        config_for(row.data_source_type)
            .as_object()
            .and_then(|members| members.values().next().cloned()),
    ) {
        for (key, value) in sent.as_object().expect("the configuration is an object") {
            assert_eq!(
                &data_source[member][key], value,
                "case {row:?} ({source}): expected {member}.{key} {value:?}, got {:?}",
                data_source[member][key]
            );
        }
    }
    // ... and every configuration it did NOT carry is gone from the data source, so
    // the update replaced the settings rather than merging into them.
    for member in every_config_member()
        .into_iter()
        .filter(|member| Some(member.as_str()) != carried.as_deref())
    {
        assert!(
            data_source[&member].is_null(),
            "case {row:?} ({source}): expected no {member}, got {:?}",
            data_source[&member]
        );
    }
}

#[tokio::test]
async fn an_update_replaces_every_setting_and_get_reads_it_back() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_update").await;
    let name = "orders";
    create_bare_data_source(&router, &api_id, name).await;

    for row in rows() {
        let updated = update_data_source(&router, &api_id, name, row.body(name)).await;
        assert_eq!(
            updated.status, 200,
            "case {row:?}: expected UpdateDataSource 200, got {updated:?}"
        );
        assert_projects(&row, &api_id, name, &updated.body["dataSource"], "update");

        let read = get_data_source(&router, &api_id, name).await;
        assert_eq!(
            read.status, 200,
            "case {row:?}: expected GetDataSource 200, got {read:?}"
        );
        assert_projects(&row, &api_id, name, &read.body["dataSource"], "get");
        // The read answers the SAME record the update persisted, member for member.
        assert_eq!(
            read.body["dataSource"], updated.body["dataSource"],
            "case {row:?}: expected GetDataSource to answer the updated data source {:?}, \
             got {:?}",
            updated.body["dataSource"], read.body["dataSource"]
        );
    }
}

#[tokio::test]
async fn an_update_to_a_type_configured_through_a_member_is_refused_without_it() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_update_config").await;
    let name = "orders";
    create_bare_data_source(&router, &api_id, name).await;

    for row in rows() {
        // The same row, stripped of its per-type configuration. Its role is present,
        // so the only thing its refusal can be about is the configuration.
        let mut body = row.body(name);
        let member = config_member(row.data_source_type);
        let with_role = Row {
            role: Some(ROLE_ARN),
            ..row.clone()
        };
        body["serviceRoleArn"] = json!(ROLE_ARN);
        if let Some(member) = member.as_deref() {
            body.as_object_mut()
                .expect("the body is an object")
                .remove(member);
        }

        let before = get_data_source(&router, &api_id, name).await;
        let answer = update_data_source(&router, &api_id, name, body).await;
        match member.as_deref() {
            // A type that names a backend must say where to find it.
            Some(member) => {
                assert_eq!(
                    answer.status, 400,
                    "case {with_role:?} without {member}: expected 400, got {answer:?}"
                );
                assert_eq!(
                    answer.error_type, "BadRequestException",
                    "case {with_role:?} without {member}: expected BadRequestException, \
                     got {answer:?}"
                );
                assert_eq!(
                    answer.message,
                    missing_config_refusal(member),
                    "case {with_role:?} without {member}: expected the refusal to name the \
                     backend, got {answer:?}"
                );
                // A refused update changes nothing.
                let after = get_data_source(&router, &api_id, name).await;
                assert_eq!(
                    after.body["dataSource"], before.body["dataSource"],
                    "case {with_role:?} without {member}: expected the stored data source to \
                     be unchanged from {:?}, got {after:?}",
                    before.body["dataSource"]
                );
            }
            // A type configured through no member is complete without one.
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {with_role:?}: expected 200 for a type needing no configuration, \
                     got {answer:?}"
                );
                assert_projects(
                    &with_role,
                    &api_id,
                    name,
                    &answer.body["dataSource"],
                    "update",
                );
            }
        }
    }
}

#[tokio::test]
async fn a_request_missing_both_takes_the_configuration_refusal() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_update_order").await;
    let name = "orders";
    create_bare_data_source(&router, &api_id, name).await;

    for (data_source_type, needs_role) in DATA_SOURCE_TYPES {
        let Some(member) = config_member(data_source_type) else {
            continue;
        };
        if !needs_role {
            // Nothing to order: this type needs no role in the form the catalog
            // carries, so the configuration is the only thing missing.
            continue;
        }
        let case = format!("an update to {data_source_type} with neither {member} nor a role");
        let mut body = request_body(name, data_source_type, None, json!({}));
        body.as_object_mut()
            .expect("the body is an object")
            .remove(&member);

        let answer = update_data_source(&router, &api_id, name, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {answer:?}"
        );
        assert_eq!(
            answer.message,
            missing_config_refusal(&member),
            "case {case:?}: expected the configuration refusal, got {answer:?}"
        );
    }
}
