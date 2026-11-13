// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request that names a route the model binds but leaves out
//! a `@required` member bound to the QUERY string.
//!
//! RULE. A request whose method and URI path name an operation this model binds is served
//! by that operation, so the request's own values decide the answer: a `@required` member
//! the request carries no value for is the modelled client-input refusal
//! (`BadRequestException`, HTTP 400) naming that member, wherever the model binds it —
//! path, body or query string. The framework's REST router reads a `@required`
//! `@httpQuery` member as a route DISCRIMINATOR, so a request that omits one matches no
//! route; for a service that overloads no method + URI template, that can only turn a
//! client's missing parameter into "operation not implemented" (HTTP 501), a status and
//! code no operation here models.
//!
//! The tables span what the rule generalises over rather than one recorded call:
//!
//! * every route of this service whose operation binds a `@required` `@httpQuery` member
//!   — `UpdateConnector` (`currentVersion`, a string) and `UntagResource` (`tagKeys`, a
//!   list), the two the model declares
//!   (`smithy_query(query="operation", operation="UpdateConnector,UntagResource")`);
//! * every way a request can leave that member unbound: no query string at all, an empty
//!   query string, a query string carrying only other parameters, one carrying the
//!   parameter's name in another case (query names are matched literally), one carrying
//!   only the parameter's bare name, and one carrying that name as another parameter's
//!   VALUE;
//! * each resource kind the shared `/v1/tags/{resourceArn}` route addresses;
//! * every kind of value the other, PATH-bound `@required` member can carry — one naming
//!   a stored resource, a well-formed ARN naming none, an ARN of another service and a
//!   value that is no ARN at all — because the absent query member is refused at the
//!   request boundary, so the answer is the same refusal whatever the rest of the request
//!   names and whatever the handler would have made of it;
//! * the contrast cases the rule turns on — the same routes WITH the member, which are
//!   served; the member present but carrying a value the operation refuses, and the same
//!   resource kinds with the member bound, which are the operation's OWN refusals (its
//!   404 for a resource that is absent, its 400 for a value that is no ARN) and not this
//!   one; and a method and path the model binds no route for at all, which keeps the
//!   framework's own answer.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response read back
//! as JSON.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    INVALID_ARN, RESOURCE_DOES_NOT_EXIST, TRANSITION_SPAN,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN carries.
/// No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error code every one of these refusals carries: the shape this service's model
/// declares for incorrect input, for all 19 of its operations
/// (`smithy_query(query="operation", operation="UpdateConnector")` lists
/// `BadRequestException` @httpError 400).
const BAD_REQUEST: &str = "BadRequestException";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS
/// SDK encodes a label: every character a path segment reserves (the ARN's own `:` and
/// `/` included) is escaped.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object as a string, or `""`.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Long enough that one change the clock owes after a call is due. The span is the
/// service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// The `CreateCustomPlugin` body for `name`.
fn plugin_request(name: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": format!("arn:aws:s3:::plugin-bucket-{name}"),
            "fileKey": format!("plugins/{name}.zip"),
        }},
    })
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn` and carrying `tags`.
fn connector_request(name: &str, plugin_arn: &str, tags: &Value) -> Value {
    json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {"tasks.max": "2"},
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{name}.kafka.{REGION}.amazonaws.com:9092"),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
        "tags": tags,
    })
}

/// The `CreateWorkerConfiguration` body for `name`, carrying `tags`.
fn worker_configuration_request(name: &str, tags: &Value) -> Value {
    json!({
        "name": name,
        // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
        "propertiesFileContent": "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=",
        "tags": tags,
    })
}

/// `DescribeConnector` for `connector_arn`.
async fn describe(emulator: &MultiServiceRouter, connector_arn: &str) -> Value {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeConnector must succeed, got {status} {described}"
    );
    described
}

/// The tags `ListTagsForResource` reports for `resource_arn`.
async fn tags_of(emulator: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let (status, listed) = call(
        emulator,
        "GET",
        &format!("/v1/tags/{}", label(resource_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource must succeed, got {status} {listed}"
    );
    serde_json::from_value(listed["tags"].clone())
        .unwrap_or_else(|error| panic!("a tags map in {listed}: {error}"))
}

/// The tag set every taggable resource in this suite is created with.
fn created_tags() -> Value {
    json!({"env": "probe", "team": "data"})
}

/// `created_tags` as a map, for comparing against what a read reports.
fn created_tag_map() -> BTreeMap<String, String> {
    serde_json::from_value(created_tags()).expect("the created tags are a string map")
}

/// A running connector and a worker configuration, both tagged, with their ARNs.
async fn held_resources(emulator: &MultiServiceRouter) -> (String, String) {
    let (status, plugin) = call(
        emulator,
        "POST",
        "/v1/custom-plugins",
        plugin_request("absent-query-plugin"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the backing plugin must be created, got {status} {plugin}"
    );
    // Only the clock moves a plugin to ACTIVE, and a connector may only reference an
    // ACTIVE one.
    past_one_transition().await;

    let (status, connector) = call(
        emulator,
        "POST",
        "/v1/connectors",
        connector_request(
            "absent-query-connector",
            &text(&plugin, "customPluginArn"),
            &created_tags(),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "the connector must be created, got {status} {connector}"
    );

    let (status, configuration) = call(
        emulator,
        "POST",
        "/v1/worker-configurations",
        worker_configuration_request("absent-query-configuration", &created_tags()),
    )
    .await;
    assert_eq!(
        status, 200,
        "the worker configuration must be created, got {status} {configuration}"
    );

    // The connector is RUNNING only once the clock says so, which is the state an update
    // is served in.
    past_one_transition().await;
    (
        text(&connector, "connectorArn"),
        text(&configuration, "workerConfigurationArn"),
    )
}

/// One row: a request that binds no value for a `@required` `@httpQuery` member.
struct Unbound {
    /// The case name, for the assertion messages.
    case: String,
    method: &'static str,
    path: String,
    body: Value,
    /// The member the refusal must name.
    member: &'static str,
}

#[tokio::test]
async fn an_absent_required_query_member_is_the_modelled_bad_request_naming_it() {
    let emulator = emulator();
    let (connector, configuration) = held_resources(&emulator).await;

    let capacity_update = json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}}});
    let configuration_update = json!({"connectorConfiguration": {"tasks.max": "4"}});

    let mut rows: Vec<Unbound> = Vec::new();
    // `UpdateConnector` — `PUT /v1/connectors/{connectorArn}`, whose `currentVersion` is
    // `@required` + `@httpQuery`. Both body forms the operation accepts, so the absence is
    // what decides the answer and not which setting the update names.
    for (shape, body) in [
        ("a capacity", capacity_update.clone()),
        ("a configuration", configuration_update),
    ] {
        rows.push(Unbound {
            case: format!("UpdateConnector naming {shape} with no query string at all"),
            method: "PUT",
            path: format!("/v1/connectors/{}", label(&connector)),
            body,
            member: "currentVersion",
        });
    }
    for (shape, query) in [
        ("nothing at all", ""),
        ("only another parameter", "maxResults=10"),
        ("the parameter's name in another case", "currentversion=1"),
        ("a parameter whose name only starts the same", "currentVersionX=1"),
        ("the parameter's name as another parameter's value", "filter=currentVersion"),
    ] {
        rows.push(Unbound {
            case: format!("UpdateConnector with a query string carrying {shape}"),
            method: "PUT",
            path: format!("/v1/connectors/{}?{query}", label(&connector)),
            body: capacity_update.clone(),
            member: "currentVersion",
        });
    }
    // `UntagResource` — `DELETE /v1/tags/{resourceArn}`, whose `tagKeys` is `@required` +
    // `@httpQuery`. The route is shared by every taggable resource, so both kinds this
    // service holds are rows.
    for (kind, arn) in [
        ("a connector", &connector),
        ("a worker configuration", &configuration),
    ] {
        rows.push(Unbound {
            case: format!("UntagResource on {kind} with no query string at all"),
            method: "DELETE",
            path: format!("/v1/tags/{}", label(arn)),
            body: Value::Null,
            member: "tagKeys",
        });
        for (shape, query) in [
            ("nothing at all", ""),
            ("the parameter's name in another case", "tagkeys=env"),
            ("a parameter whose name only starts the same", "tagKeysToKeep=env"),
            ("the parameter's name as another parameter's value", "filter=tagKeys"),
        ] {
            rows.push(Unbound {
                case: format!(
                    "UntagResource on {kind} with a query string carrying {shape}"
                ),
                method: "DELETE",
                path: format!("/v1/tags/{}?{query}", label(arn)),
                body: Value::Null,
                member: "tagKeys",
            });
        }
    }

    for row in &rows {
        let (status, answered) = call(&emulator, row.method, &row.path, row.body.clone()).await;
        let case = &row.case;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains(BAD_REQUEST),
            "case {case:?}: expected {BAD_REQUEST}, got {answered}"
        );
        let message = text(&answered, "message");
        assert!(
            message.contains(row.member),
            "case {case:?}: expected a message naming {:?}, got {message:?}",
            row.member
        );
    }

    // None of those refusals wrote anything: the connector is still running at the
    // version it was created at, and both tag sets are exactly the ones their creates
    // stored.
    let described = describe(&emulator, &connector).await;
    let version = text(&described, "currentVersion");
    assert_eq!(
        text(&described, "connectorState"),
        "RUNNING",
        "the refused calls must not move the connector's state, got {described}"
    );
    for (kind, arn) in [
        ("the connector", &connector),
        ("the worker configuration", &configuration),
    ] {
        let reported = tags_of(&emulator, arn).await;
        assert_eq!(
            reported,
            created_tag_map(),
            "case {kind:?}: the refused calls must not have removed a tag, got {reported:?}"
        );
    }

    // The member PRESENT but carrying a value the operation refuses is the OPERATION's
    // own refusal, named in this service's parameter-rejection form — so the route really
    // matched and the handler really ran, rather than the request being refused for being
    // unroutable.
    for stale in ["", "9", "1.0"] {
        let (status, answered) = call(
            &emulator,
            "PUT",
            &format!(
                "/v1/connectors/{}?currentVersion={}",
                label(&connector),
                label(stale)
            ),
            capacity_update.clone(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {stale:?}: a currentVersion that is not the connector's is a 400, got \
             {status} {answered}"
        );
        let message = text(&answered, "message");
        assert_eq!(
            message, "Invalid parameter currentVersion.",
            "case {stale:?}: expected the operation's own refusal of the value, got {message:?}"
        );
    }

    // And the same routes WITH the member are served: the update is accepted at the
    // version the connector reports, and the untag removes exactly the key it names.
    let (status, updated) = call(
        &emulator,
        "PUT",
        &format!(
            "/v1/connectors/{}?currentVersion={}",
            label(&connector),
            label(&version)
        ),
        capacity_update,
    )
    .await;
    assert_eq!(
        status, 200,
        "the update carrying currentVersion={version:?} must be served, got {status} {updated}"
    );

    let (status, untagged) = call(
        &emulator,
        "DELETE",
        &format!("/v1/tags/{}?tagKeys=env", label(&configuration)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 204,
        "the untag carrying tagKeys must be served, got {status} {untagged}"
    );
    let reported = tags_of(&emulator, &configuration).await;
    let expected: BTreeMap<String, String> = created_tag_map()
        .into_iter()
        .filter(|(key, _)| key != "env")
        .collect();
    assert_eq!(
        reported, expected,
        "the untag must remove exactly the key it named, got {reported:?}"
    );
}

/// One value the PATH-bound `@required` member of either route can carry, and what the
/// operation answers a request that carries it AND the query member the route needs.
struct PathValue {
    /// The case name, for the assertion messages.
    case: &'static str,
    /// The `@httpLabel` value, unencoded.
    resource: String,
    /// The status the operation's own answer carries once it is reached.
    served_status: u16,
    /// The reason that answer's message carries.
    served_reason: &'static str,
}

#[tokio::test]
async fn the_absent_query_member_is_refused_before_the_handler_judges_the_rest() {
    let emulator = emulator();
    let (connector, _) = held_resources(&emulator).await;

    // The absent member is refused at the request BOUNDARY, so the answer cannot depend on
    // what the rest of the request names: the same refusal is due for a path label naming a
    // resource this emulator holds, a well-formed ARN of this service holding nothing, an
    // ARN of another service, and a value that is no ARN at all — each of which the handler
    // itself would have answered differently.
    let rows = [
        PathValue {
            case: "a resource this emulator holds",
            resource: connector.clone(),
            served_status: 204,
            served_reason: "",
        },
        PathValue {
            case: "a well-formed ARN of this service holding nothing",
            resource: format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/absent-query-ghost/\
                 11111111-2222-3333-4444-555555555555-1"
            ),
            served_status: 404,
            served_reason: RESOURCE_DOES_NOT_EXIST,
        },
        PathValue {
            case: "an ARN of another service",
            resource: "arn:aws:s3:::absent-query-bucket".to_string(),
            served_status: 400,
            served_reason: INVALID_ARN,
        },
        PathValue {
            case: "a value that is no ARN at all",
            resource: "absent-query-not-an-arn".to_string(),
            served_status: 400,
            served_reason: INVALID_ARN,
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, answered) = call(
            &emulator,
            "DELETE",
            &format!("/v1/tags/{}", label(&row.resource)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an absent tagKeys is the modelled 400 whatever the path names, \
             got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains(BAD_REQUEST),
            "case {case:?}: expected {BAD_REQUEST}, got {answered}"
        );
        let message = text(&answered, "message");
        assert!(
            message.contains("tagKeys"),
            "case {case:?}: expected a message naming the absent member, got {message:?}"
        );
        // The refusal is the BOUNDARY's, so none of the handler's own reasons for this
        // path value can be in it: the handler never ran.
        for reason in [RESOURCE_DOES_NOT_EXIST, INVALID_ARN] {
            assert!(
                !message.contains(reason),
                "case {case:?}: the absent member is refused before the handler judges \
                 {reason:?}, got {message:?}"
            );
        }

        // And the SAME path value WITH the member bound is the operation's own answer, so
        // the route really is served and the precedence above is a precedence and not an
        // operation that never runs.
        let (status, answered) = call(
            &emulator,
            "DELETE",
            &format!("/v1/tags/{}?tagKeys=env", label(&row.resource)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, row.served_status,
            "case {case:?}: with tagKeys bound, expected HTTP {}, got {status} {answered}",
            row.served_status
        );
        let message = text(&answered, "message");
        assert!(
            message.contains(row.served_reason),
            "case {case:?}: with tagKeys bound, expected a message carrying {:?}, got \
             {message:?}",
            row.served_reason
        );
    }

    // The one row naming a resource this emulator holds removed exactly the key the served
    // call named, and the refused call before it removed nothing.
    let reported = tags_of(&emulator, &connector).await;
    let expected: BTreeMap<String, String> = created_tag_map()
        .into_iter()
        .filter(|(key, _)| key != "env")
        .collect();
    assert_eq!(
        reported, expected,
        "only the served untag may change the stored tags, got {reported:?}"
    );
}

#[tokio::test]
async fn a_method_and_path_the_model_binds_no_route_for_is_still_unknown() {
    let emulator = emulator();
    let (connector, _) = held_resources(&emulator).await;

    // The rule reaches only a method and path the model DOES bind. A request that names
    // no route of this service keeps the framework's own answer for a request that named
    // no modelled operation, whichever way it misses: a method no route of that path
    // binds, a path one segment short of a route, a path one segment too long, and a path
    // this model does not publish at all.
    let rows: [(&str, &str, String); 5] = [
        (
            "a method no route of /v1/connectors/{connectorArn} binds",
            "PATCH",
            format!("/v1/connectors/{}", label(&connector)),
        ),
        (
            "a method no route of /v1/tags/{resourceArn} binds",
            "PUT",
            format!("/v1/tags/{}", label(&connector)),
        ),
        (
            "the update route one segment short of its label",
            "PUT",
            "/v1/connectors".to_string(),
        ),
        (
            "the untag route with a segment past its label",
            "DELETE",
            format!("/v1/tags/{}/keys", label(&connector)),
        ),
        (
            "a path this model does not publish",
            "PUT",
            "/v1/connector-groups/all".to_string(),
        ),
    ];

    for (case, method, path) in rows {
        let (status, answered) = call(&emulator, method, &path, Value::Null).await;
        assert_eq!(
            status, 501,
            "case {case:?}: a request naming no modelled route must keep the framework's \
             own answer, got {status} {answered}"
        );
    }
}
