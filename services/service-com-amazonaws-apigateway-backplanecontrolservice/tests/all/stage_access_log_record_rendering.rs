// SPDX-License-Identifier: CC0-1.0

//! What ONE access-log record a stage delivers CONTAINS, and WHERE it lands: the stage's own
//! `format` rendered against the request the stage answered, written into the group its
//! `destinationArn` names.
//!
//! The rule under test, one level above the recorded case:
//!
//! > With the account's CloudWatch role installed, every request a stage carrying
//! > `accessLogSettings` answers leaves exactly ONE record in the CloudWatch Logs group its
//! > `destinationArn` names — in either published form of a log-group ARN, the group itself or
//! > everything under it — and that record is the stage's `format` with its literal text
//! > passed through VERBATIM and each `$context.<member>` reference replaced by the value for
//! > THAT request (the id the answer was reported under, the verb, the matched resource path,
//! > the stage, the decimal status the stage answered with), a reference this plane produces
//! > no value for rendered as the one-character placeholder a field with no value carries.
//! > The integration type that produced the answer decides none of this: a `MOCK` method,
//! > which needs no backend at all, is logged exactly as any other.
//!
//! The sibling `stage_access_logging` suite spans the destinations and outcomes of a stage
//! whose method is run by AWS Lambda; what this suite adds is the RENDERING itself over a
//! table of formats — a JSON one carrying literal members beside its references, a line whose
//! only `$` opens no reference, and one naming members this plane has no value for — crossed
//! with both forms of the destination ARN and with two methods the stage answers with
//! different statuses, on the one integration type that reaches no backend.
//!
//! Only a cross-service suite can pin this: the api, its stage and the logging configuration
//! are API Gateway's, the role the account logs under is AWS IAM's, and the group a record
//! lands in is CloudWatch Logs' — all three on ONE engine, as `core/router/src/lib.rs` mounts
//! them. Every record is read back through CloudWatch Logs' own `FilterLogEvents`, so a
//! record that never reached that store cannot pass.

use serde_json::{json, Value};

use crate::account_logging_role::{create_role, replace_role, role_arn, trusting};
use crate::execute_api_data_plane::{
    deploy, integration_response, json_template, method_response, mock_resource, put_integration,
    put_method,
};
use crate::rest_api_lifecycle::{create_api, gateway_with_iam_and_logs, Gateway};
use crate::stage_access_logging::{delivered, invoke};

/// The Region every ARN below names, which is the one the harness signs and invokes in.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to, and the path part of the one resource it carries.
const STAGE: &str = "live";
const PATH_PART: &str = "ping";

/// The resource path the records name: the path of the matched method, as
/// `$context.resourcePath` reports it.
const RESOURCE_PATH: &str = "/ping";

/// The role the account writes its logs as, and the principal whose assume its trust policy
/// has to admit for API Gateway to be able to write at all.
const ROLE_NAME: &str = "apigw-access-logs";
const ROLE_PATH: &str = "/service-role/";
const SERVICE_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The two methods the stage serves, each a `MOCK` integration — "for testing without
/// actually invoking the backend" (`IntegrationType`) — so what is logged is a stage's own
/// doing and no backend's.
const SERVED_VERB: &str = "GET";
const SERVED_STATUS: &str = "200";
const CREATED_VERB: &str = "POST";
const CREATED_STATUS: &str = "201";

/// What each of those methods answers with.
const SERVED_BODY: &str = r#"{"pong":true}"#;
const CREATED_BODY: &str = r#"{"created":true}"#;

/// What a row's expected record writes where this request's own id belongs: the id the answer
/// reported in its `x-amzn-RequestId` header, which is per-run and so is substituted into the
/// expectation rather than written into it.
const REQUEST_ID: &str = "{requestId}";

/// One row: the logging configuration the stage carries, the request made under it, and the
/// record it has to leave.
struct Row {
    case: &'static str,
    /// The group the record lands in — one per row, so the count asserted is this row's own.
    group: &'static str,
    /// Whether the `destinationArn` names everything UNDER the group (the `:*` form) rather
    /// than the group itself: both published forms name the same group.
    everything_under: bool,
    /// The `accessLogSettings.format` the stage renders by.
    format: &'static str,
    /// The request: the verb and the path within the stage.
    verb: &'static str,
    path: &'static str,
    /// The status the stage answers with, which `$context.status` reports in decimal.
    status: u16,
    /// The whole record, with [`REQUEST_ID`] standing for this request's own id.
    expected: &'static str,
}

/// A JSON format carrying a literal member beside its references, which is what a client that
/// ships structured access logs writes.
const JSON_FORMAT: &str = concat!(
    r#"{"marker":"pinned-literal","requestId":"$context.requestId","#,
    r#""httpMethod":"$context.httpMethod","status":"$context.status","#,
    r#""resourcePath":"$context.resourcePath","stage":"$context.stage"}"#
);

/// The rows: the formats, both destination forms, and both methods.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a JSON format with a literal member, delivered to the group its arn names",
            group: "/apigw/access/json",
            everything_under: false,
            format: JSON_FORMAT,
            verb: SERVED_VERB,
            path: RESOURCE_PATH,
            status: 200,
            expected: concat!(
                r#"{"marker":"pinned-literal","requestId":"{requestId}","#,
                r#""httpMethod":"GET","status":"200","#,
                r#""resourcePath":"/ping","stage":"live"}"#
            ),
        },
        Row {
            case: "the same format, delivered through the arn naming everything under the group",
            group: "/apigw/access/everything-under",
            everything_under: true,
            format: JSON_FORMAT,
            verb: SERVED_VERB,
            path: RESOURCE_PATH,
            status: 200,
            expected: concat!(
                r#"{"marker":"pinned-literal","requestId":"{requestId}","#,
                r#""httpMethod":"GET","status":"200","#,
                r#""resourcePath":"/ping","stage":"live"}"#
            ),
        },
        Row {
            case: "a request the other method answered, whose own status the record names",
            group: "/apigw/access/created",
            everything_under: false,
            format: "$context.httpMethod $context.resourcePath $context.status $context.requestId",
            verb: CREATED_VERB,
            path: RESOURCE_PATH,
            status: 201,
            expected: "POST /ping 201 {requestId}",
        },
        Row {
            case: "a format whose literal text carries a dollar that opens no reference",
            group: "/apigw/access/literal",
            everything_under: false,
            format: "$5 flat, $input untouched: $context.requestId",
            verb: SERVED_VERB,
            path: RESOURCE_PATH,
            status: 200,
            expected: "$5 flat, $input untouched: {requestId}",
        },
        Row {
            case: "a format naming members this plane produces no value for",
            group: "/apigw/access/unresolved",
            everything_under: false,
            format: "$context.requestId|$context.identity.user|$context.authorizer.principalId",
            verb: SERVED_VERB,
            path: RESOURCE_PATH,
            status: 200,
            expected: "{requestId}|-|-",
        },
        Row {
            case: "a query string the client sent, which the resource path does not carry",
            group: "/apigw/access/query",
            everything_under: false,
            format: JSON_FORMAT,
            verb: SERVED_VERB,
            path: "/ping?id=7",
            status: 200,
            expected: concat!(
                r#"{"marker":"pinned-literal","requestId":"{requestId}","#,
                r#""httpMethod":"GET","status":"200","#,
                r#""resourcePath":"/ping","stage":"live"}"#
            ),
        },
    ]
}

/// The ARN of the log group `name`, as a stage's `destinationArn` names one: the group itself,
/// or — with `everything_under` — everything under it.
fn destination_arn(name: &str, everything_under: bool) -> String {
    let account = engine::emulator_account_id();
    let everything = if everything_under { ":*" } else { "" };
    format!("arn:aws:logs:{REGION}:{account}:log-group:{name}{everything}")
}

/// Install the account's CloudWatch role through AWS IAM's own `CreateRole` and API Gateway's
/// own `UpdateAccount`, which is what API Gateway writes the account's logs as.
async fn install_account_role(gateway: &Gateway) {
    create_role(gateway, ROLE_NAME, ROLE_PATH, &trusting(SERVICE_PRINCIPAL)).await;
    let installed = replace_role(gateway, &role_arn(ROLE_PATH, ROLE_NAME)).await;
    assert_eq!(
        installed.status, 200,
        "setup UpdateAccount /cloudwatchRoleArn: expected 200, got {} with {:?}",
        installed.status, installed.body
    );
}

/// Point the stage's access logging at `group` — in the ARN form the row asks for — rendered
/// by `format`, exactly as a client configures it.
async fn configure_access_logging(
    gateway: &Gateway,
    api: &str,
    case: &str,
    group: &str,
    everything_under: bool,
    format: &str,
) {
    let answer = gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({
                "patchOperations": [
                    {
                        "op": "replace",
                        "path": "/accessLogSettings/destinationArn",
                        "value": destination_arn(group, everything_under),
                    },
                    { "op": "replace", "path": "/accessLogSettings/format", "value": format },
                ],
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateStage to store the logging configuration, got {} with \
         {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["accessLogSettings"]["format"],
        json!(format),
        "case {case:?}: expected the stored format to be the one configured, got {:?}",
        answer.body["accessLogSettings"]
    );
}

/// The whole setup: an api with one resource carrying two `MOCK` methods, deployed to the
/// stage. Answers the api id.
async fn deployed(gateway: &Gateway) -> String {
    let (api, root) = create_api(gateway, "access-log-rendering").await;
    let resource = mock_resource(
        gateway,
        &api,
        &root,
        PATH_PART,
        SERVED_VERB,
        SERVED_STATUS,
        json_template(SERVED_BODY),
    )
    .await;
    put_method(
        gateway,
        &api,
        &resource,
        CREATED_VERB,
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(
        gateway,
        &api,
        &resource,
        CREATED_VERB,
        json!({ "type": "MOCK" }),
    )
    .await;
    method_response(gateway, &api, &resource, CREATED_VERB, CREATED_STATUS, json!({})).await;
    integration_response(
        gateway,
        &api,
        &resource,
        CREATED_VERB,
        CREATED_STATUS,
        json!({ "responseTemplates": json_template(CREATED_BODY) }),
    )
    .await;
    deploy(gateway, &api, STAGE).await;
    api
}

/// RULE: the record a stage delivers is its own format with the literal text verbatim and
/// every `$context` reference replaced by this request's value, in the group its
/// `destinationArn` names in either form — whatever answered the request.
#[tokio::test]
async fn one_record_per_request_renders_the_stages_format_against_that_request() {
    let gateway = gateway_with_iam_and_logs();
    install_account_role(&gateway).await;
    let api = deployed(&gateway).await;

    for row in rows() {
        let case = row.case;
        configure_access_logging(
            &gateway,
            &api,
            case,
            row.group,
            row.everything_under,
            row.format,
        )
        .await;

        let answered = invoke(&gateway.router, &api, STAGE, row.verb, row.path).await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?}: expected HTTP {} from the stage, got {} with {:?}",
            row.status,
            answered.status,
            answered.text()
        );
        let request_id = answered
            .request_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the answer named no request id"));

        // Read back through CloudWatch Logs' own reader: a group API Gateway has not had to
        // create is a destination nothing was delivered to at all.
        let records = delivered(&gateway.router, row.group)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected API Gateway to have delivered into {:?}, but \
                     CloudWatch Logs reports no such group at all",
                    row.group
                )
            });
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected exactly one record in {:?}, got {records:?}",
            row.group
        );
        let expected = row.expected.replace(REQUEST_ID, &request_id);
        assert_eq!(
            records[0], expected,
            "case {case:?}: expected the record {expected:?} in {:?}, got {:?}",
            row.group, records[0]
        );
        // The references the format spells are RESOLVED, not passed through as the template
        // text they were written as.
        assert!(
            !records[0].contains("$context."),
            "case {case:?}: expected no unsubstituted reference in {:?}, got {:?}",
            row.group,
            records[0]
        );
    }
}

/// RULE, read the other way round: the placeholder a reference with no value renders as is
/// the one AWS writes such a field with, which is what keeps a rendered line's field count
/// the same whatever the call carried.
#[tokio::test]
async fn a_reference_with_no_value_renders_the_placeholder_this_emulator_publishes() {
    let gateway = gateway_with_iam_and_logs();
    install_account_role(&gateway).await;
    let api = deployed(&gateway).await;
    let group = "/apigw/access/placeholder";
    let case = "a format naming one member with a value and one without";
    configure_access_logging(
        &gateway,
        &api,
        case,
        group,
        false,
        "$context.requestId $context.identity.user",
    )
    .await;

    let answered = invoke(&gateway.router, &api, STAGE, SERVED_VERB, RESOURCE_PATH).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the stage to serve the mocked method, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let request_id = answered
        .request_id
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: the answer named no request id"));
    let records = delivered(&gateway.router, group)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: no group {group:?} at all"));
    let expected = format!(
        "{request_id} {}",
        service_common::api_gateway_access_logs::UNRESOLVED_VALUE
    );
    assert_eq!(
        records,
        vec![expected.clone()],
        "case {case:?}: expected {expected:?} in {group:?}, got {records:?}"
    );
}

/// RULE, the other direction: with the stage's logging configuration REMOVED, the requests it
/// goes on serving leave no record — the destination is read off the stage each time rather
/// than fixed when it was first configured.
#[tokio::test]
async fn a_stage_whose_logging_configuration_was_removed_delivers_nothing_more() {
    let gateway = gateway_with_iam_and_logs();
    install_account_role(&gateway).await;
    let api = deployed(&gateway).await;
    let group = "/apigw/access/removed";
    let case = "the configuration that was then removed";
    configure_access_logging(&gateway, &api, case, group, false, JSON_FORMAT).await;

    let first = invoke(&gateway.router, &api, STAGE, SERVED_VERB, RESOURCE_PATH).await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected the stage to serve the mocked method, got {} with {:?}",
        first.status,
        first.text()
    );
    let before = delivered(&gateway.router, group)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: no group {group:?} at all"));
    assert_eq!(
        before.len(),
        1,
        "case {case:?}: expected the configured stage to have delivered one record, got \
         {before:?}"
    );

    let removed = gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({
                "patchOperations": [{ "op": "remove", "path": "/accessLogSettings" }],
            })),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case {case:?}: expected the logging configuration to be removable, got {} with {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        removed.body["accessLogSettings"],
        Value::Null,
        "case {case:?}: expected the stage to report no accessLogSettings, got {:?}",
        removed.body["accessLogSettings"]
    );

    let after_removal = invoke(&gateway.router, &api, STAGE, SERVED_VERB, RESOURCE_PATH).await;
    assert_eq!(
        after_removal.status, 200,
        "case {case:?}: expected the stage to go on serving its traffic, got {} with {:?}",
        after_removal.status,
        after_removal.text()
    );
    let after = delivered(&gateway.router, group)
        .await
        .unwrap_or_default();
    assert_eq!(
        after, before,
        "case {case:?}: expected no further record in {group:?} after the configuration was \
         removed, got {after:?}"
    );
}
