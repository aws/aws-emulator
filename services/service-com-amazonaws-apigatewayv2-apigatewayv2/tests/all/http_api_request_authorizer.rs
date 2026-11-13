// SPDX-License-Identifier: CC0-1.0

//! The Lambda (`REQUEST`) authorizer of an HTTP api, from the create that mints it to the
//! gate it puts in front of a deployed route.
//!
//! The rules these cases assert, one level above the recorded case (one `REQUEST`
//! authorizer over `$request.header.Authorization`, payload format version `2.0`):
//!
//! > **A `REQUEST` authorizer is a resource of the api that owns it**: `CreateAuthorizer`
//! > persists it and answers the modelled success status (201) with the `AuthorizerId` it
//! > minted and the document the record holds — within the operation's modelled error set,
//! > which carries no scaffold refusal — and every reader of that authorizer
//! > (`GetAuthorizer`, `GetAuthorizers`) reports that same record, member for member, with
//! > nothing invented for a member the request left out.
//!
//! > **A route fronted by such an authorizer is gated by the authorizer's identity
//! > sources**: each entry of `IdentitySource` must be "present in the request, not null,
//! > and non-empty. Only when this is true does the authorizer invoke the authorizer
//! > Lambda function. Otherwise, it returns a 401 Unauthorized response without calling
//! > the Lambda function" (`Authorizer$IdentitySource`) — so the precondition is
//! > CONJUNCTIVE: dropping any ONE of the parts the request presents earns `401`
//! > `{"message":"Unauthorized"}`, and a request presenting all of them is not refused by
//! > this step.
//!
//! The rows span what those rules generalise over: the number of identity sources (none,
//! one, two), the request part each names (a standard header, a non-standard header, a
//! query-string parameter), the payload format version (`1.0`, `2.0`, the two values
//! `AuthorizerPayloadFormatVersion` publishes), the TTL across the modelled `0..3600`
//! range, `EnableSimpleResponses` either way, and whether the authorizer carries
//! `AuthorizerCredentialsArn`. Each gated row is then driven once per presentation it can
//! be missing a part of.
//!
//! What an ADMITTED request is answered with is NOT asserted here: past the identity-source
//! precondition the decision is the answer of the Lambda function `AuthorizerUri` names, so
//! it belongs to the suites that mount AWS Lambda beside this service and let that function
//! run (`http_api_backend_dispatch.rs` for the decision itself, and
//! `http_api_request_authorizer_cache_keying.rs` for how often it is asked). So the admitted
//! leg asserts only what this step owes: that the request was NOT refused `401` by the
//! precondition, and that the endpoint answered as a data plane rather than through the
//! scaffold channel.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, carries_nothing, emulator};
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The region the control-plane requests are signed for, and so the region the api's own
/// endpoint host and the authorizer's Lambda URI name.
const REGION: &str = "eu-west-1";

/// The status and sentence a request missing an identity source earns: the authorizer was
/// not invoked at all.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The error code the scaffold channel answers an unimplemented operation with, and the
/// status it carries — neither of which any modelled operation or data plane may answer.
const NOT_IMPLEMENTED: &str = "NotImplemented";
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The error code the front door answers when NO service claims a request.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The route the gated api serves, and the stage it is deployed to.
const ROUTE_KEY: &str = "GET /orders";
const ROUTE_PATH: &str = "/orders";
const STAGE_NAME: &str = "prod";

/// A well-formed Lambda authorizer URI, the form `AuthorizerUri` documents: "For REQUEST
/// authorizers, this must be a well-formed Lambda function URI, for example,
/// arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations".
fn lambda_uri(function: &str) -> String {
    format!(
        "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:{REGION}:123456789012:function:{function}/invocations"
    )
}

/// One authorizer configuration, the members the stored record must report for it, and the
/// request parts a client presents to satisfy its identity sources.
struct Row {
    case: &'static str,
    /// The `CreateAuthorizer` request body.
    request: Value,
    /// Members the stored authorizer must report, by name.
    expected: Vec<(&'static str, Value)>,
    /// Members the record must carry NOTHING for: the request named none of them.
    absent: &'static [&'static str],
    /// The headers that satisfy the header identity sources this row names — one header
    /// per source, so dropping any one of them drops exactly one source.
    headers: &'static [(&'static str, &'static str)],
    /// The query-string parameters that satisfy the query-string identity sources it
    /// names, one parameter per source.
    query: &'static [(&'static str, &'static str)],
}

impl Row {
    /// Whether this row's authorizer names any identity source at all. "For a REQUEST
    /// authorizer, this is optional" (`Authorizer$IdentitySource`), and an authorizer that
    /// names none has no precondition to fail.
    fn gated(&self) -> bool {
        !self.headers.is_empty() || !self.query.is_empty()
    }
}

/// The authorizer table: one row per configuration the class admits.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one standard header source, payload format 2.0, simple responses, \
                   caching disabled",
            request: json!({
                "name": "request-authorizer-header",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("deny-everything"),
                "identitySource": ["$request.header.Authorization"],
                "authorizerPayloadFormatVersion": "2.0",
                "enableSimpleResponses": true,
                "authorizerResultTtlInSeconds": 0
            }),
            expected: vec![
                ("name", json!("request-authorizer-header")),
                ("authorizerType", json!("REQUEST")),
                ("identitySource", json!(["$request.header.Authorization"])),
                ("authorizerPayloadFormatVersion", json!("2.0")),
                ("enableSimpleResponses", json!(true)),
                ("authorizerResultTtlInSeconds", json!(0)),
            ],
            absent: &[
                "authorizerCredentialsArn",
                "identityValidationExpression",
                "jwtConfiguration",
            ],
            headers: &[("authorization", "Bearer opaque-request-token")],
            query: &[],
        },
        Row {
            case: "a non-standard header source, payload format 1.0, the upper TTL bound, \
                   and invocation credentials",
            request: json!({
                "name": "request-authorizer-custom-header",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("inspect-request"),
                "authorizerCredentialsArn":
                    "arn:aws:iam::123456789012:role/apigateway-invoke-authorizer",
                "identitySource": ["$request.header.X-Gateway-Token"],
                "authorizerPayloadFormatVersion": "1.0",
                "authorizerResultTtlInSeconds": 3600
            }),
            expected: vec![
                ("authorizerType", json!("REQUEST")),
                ("identitySource", json!(["$request.header.X-Gateway-Token"])),
                ("authorizerPayloadFormatVersion", json!("1.0")),
                ("authorizerResultTtlInSeconds", json!(3600)),
                (
                    "authorizerCredentialsArn",
                    json!("arn:aws:iam::123456789012:role/apigateway-invoke-authorizer"),
                ),
            ],
            absent: &["enableSimpleResponses", "jwtConfiguration"],
            headers: &[("x-gateway-token", "opaque-gateway-token")],
            query: &[],
        },
        Row {
            case: "two sources, a header and a query-string parameter",
            request: json!({
                "name": "request-authorizer-two-sources",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("two-source-authorizer"),
                "identitySource": [
                    "$request.header.Authorization",
                    "$request.querystring.access_token"
                ],
                "authorizerPayloadFormatVersion": "2.0",
                "authorizerResultTtlInSeconds": 300
            }),
            expected: vec![
                ("authorizerType", json!("REQUEST")),
                (
                    "identitySource",
                    json!([
                        "$request.header.Authorization",
                        "$request.querystring.access_token"
                    ]),
                ),
                ("authorizerResultTtlInSeconds", json!(300)),
            ],
            absent: &["authorizerCredentialsArn", "jwtConfiguration"],
            headers: &[("authorization", "Bearer opaque-request-token")],
            query: &[("access_token", "opaque-query-token")],
        },
        Row {
            case: "a query-string source alone, simple responses off",
            request: json!({
                "name": "request-authorizer-query",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("query-source-authorizer"),
                "identitySource": ["$request.querystring.token"],
                "authorizerPayloadFormatVersion": "2.0",
                "enableSimpleResponses": false
            }),
            expected: vec![
                ("authorizerType", json!("REQUEST")),
                ("identitySource", json!(["$request.querystring.token"])),
                ("enableSimpleResponses", json!(false)),
            ],
            absent: &[
                "authorizerCredentialsArn",
                "authorizerResultTtlInSeconds",
                "jwtConfiguration",
            ],
            headers: &[],
            query: &[("token", "opaque-query-token")],
        },
        Row {
            case: "no identity source at all, which a REQUEST authorizer may omit",
            request: json!({
                "name": "request-authorizer-no-source",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("unconditional-authorizer"),
                "identitySource": [],
                "authorizerPayloadFormatVersion": "2.0"
            }),
            expected: vec![
                ("authorizerType", json!("REQUEST")),
                ("authorizerPayloadFormatVersion", json!("2.0")),
            ],
            absent: &[
                "identitySource",
                "authorizerCredentialsArn",
                "enableSimpleResponses",
                "jwtConfiguration",
            ],
            headers: &[],
            query: &[],
        },
    ]
}

/// Create one HTTP api and answer `(ApiId, the host of its own ApiEndpoint)`.
async fn create_api(emulator: &MultiServiceRouter, case: &str) -> (String, String) {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "lambda-authorized-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
    );
    let endpoint = as_str(&api, "apiEndpoint");
    let host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();
    (as_str(&api, "apiId"), host)
}

/// Create the row's authorizer under `api_id` and answer the created document.
async fn create_authorizer(
    emulator: &MultiServiceRouter,
    api_id: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(request.clone()),
    )
    .await
}

/// Build the invoke query string from the parameters given, or the empty string for none.
fn query_string(parameters: &[(&str, &str)]) -> String {
    if parameters.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = parameters
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    format!("?{}", rendered.join("&"))
}

/// Send one invoke of the gated route to the api's own endpoint host, carrying `headers`
/// and `query` and no AWS credential — which is what a client of a Lambda-authorized route
/// sends.
async fn invoke(
    emulator: &MultiServiceRouter,
    endpoint_host: &str,
    headers: &[(&str, &str)],
    query: &[(&str, &str)],
) -> Answer {
    send(
        emulator,
        endpoint_host,
        "GET",
        &format!("/{STAGE_NAME}{ROUTE_PATH}{}", query_string(query)),
        headers,
    )
    .await
}

/// Assert the endpoint answered as a data plane: the request reached this service, and the
/// answer did not come from the scaffold channel (an unmodelled `NotImplemented`, which is
/// outside every operation's modelled error set and which no data plane ever answers).
fn served_by_the_data_plane(case: &str, presentation: &str, answer: &Answer) {
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?} / {presentation}: the invoke URL must be served by this service, \
         got {}",
        answer.summary()
    );
    assert_ne!(
        answer.error_type.as_deref(),
        Some(NOT_IMPLEMENTED),
        "case {case:?} / {presentation}: no answer may carry the scaffold's \
         {NOT_IMPLEMENTED:?} code, got {}",
        answer.summary()
    );
    assert_ne!(
        answer.status, NOT_IMPLEMENTED_STATUS,
        "case {case:?} / {presentation}: no answer may be the scaffold's \
         {NOT_IMPLEMENTED_STATUS}, got {}",
        answer.summary()
    );
}

/// `CreateAuthorizer` mints and persists a `REQUEST` authorizer of an HTTP api, answers
/// 201 with the document the record holds, and every reader reports that same record.
#[tokio::test]
async fn a_request_authorizer_is_created_and_reported_from_the_one_record() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case;
        let (api_id, _host) = create_api(&emulator, case).await;

        let (status, created) = create_authorizer(&emulator, &api_id, &row.request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateAuthorizer must answer 201, got {status} {created}"
        );
        let authorizer_id = as_str(&created, "authorizerId");
        assert_minted_id(case, "authorizerId", &authorizer_id);

        for (member, expected) in &row.expected {
            assert_eq!(
                created.get(*member),
                Some(expected),
                "case {case:?}: expected {member} == {expected}, got {created}"
            );
        }
        for member in row.absent {
            assert!(
                carries_nothing(&created, member),
                "case {case:?}: the request named no {member}, so the record must carry \
                 nothing for it, got {created}"
            );
        }

        // Read back through the implemented reader: what the create answered is what a
        // later GetAuthorizer reports, member for member.
        let (status, reported) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAuthorizer must answer 200, got {status} {reported}"
        );
        assert_eq!(
            reported, created,
            "case {case:?}: GetAuthorizer must report the record the create answered, \
             got {reported} against {created}"
        );

        // And the collection reader reports the same ONE document.
        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/authorizers"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAuthorizers must answer 200, got {status} {listed}"
        );
        assert_eq!(
            listed.get("items"),
            Some(&json!([created])),
            "case {case:?}: GetAuthorizers must list the one stored authorizer, got {listed}"
        );
    }
}

/// A route fronted by a `REQUEST` authorizer admits a request only when EVERY identity
/// source the authorizer names is presented: dropping any one part is `401`
/// `{"message":"Unauthorized"}` and the authorizer is not invoked.
#[tokio::test]
async fn every_identity_source_must_be_presented_for_a_lambda_authorized_route() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case;
        let (api_id, endpoint_host) = create_api(&emulator, case).await;

        let (status, authorizer) = create_authorizer(&emulator, &api_id, &row.request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateAuthorizer must answer 201, got {status} {authorizer}"
        );
        let authorizer_id = as_str(&authorizer, "authorizerId");

        let (status, integration) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": format!(
                    "arn:aws:lambda:{REGION}:123456789012:function:orders-handler"
                ),
                "payloadFormatVersion": "2.0"
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
        );
        let integration_id = as_str(&integration, "integrationId");

        // `CUSTOM` is the `AuthorizationType` "for using a Lambda authorizer"
        // (`Route$AuthorizationType`), associated with the authorizer by `AuthorizerId`.
        let (status, route) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": ROUTE_KEY,
                "target": format!("integrations/{integration_id}"),
                "authorizationType": "CUSTOM",
                "authorizerId": authorizer_id
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRoute must answer 201, got {status} {route}"
        );

        let (status, stage) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": STAGE_NAME, "autoDeploy": true })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateStage must answer 201, got {status} {stage}"
        );

        // Every part presented: the precondition holds, so this step owes no refusal.
        let answer = invoke(&emulator, &endpoint_host, row.headers, row.query).await;
        served_by_the_data_plane(case, "every identity source presented", &answer);
        assert_ne!(
            answer.status, UNAUTHORIZED,
            "case {case:?}: a request presenting every identity source must not be \
             refused {UNAUTHORIZED}, got {}",
            answer.summary()
        );

        if !row.gated() {
            continue;
        }

        // Each part dropped in turn: the precondition is conjunctive, so each omission is
        // the same 401 and the authorizer is not invoked.
        for dropped in 0..row.headers.len() {
            let headers: Vec<(&str, &str)> = row
                .headers
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != dropped)
                .map(|(_, header)| *header)
                .collect();
            let presentation = format!("without the {:?} header", row.headers[dropped].0);
            let answer = invoke(&emulator, &endpoint_host, &headers, row.query).await;
            served_by_the_data_plane(case, &presentation, &answer);
            assert_eq!(
                answer.status, UNAUTHORIZED,
                "case {case:?} / {presentation}: expected {UNAUTHORIZED}, got {}",
                answer.summary()
            );
            assert_eq!(
                message(&answer),
                UNAUTHORIZED_MESSAGE,
                "case {case:?} / {presentation}: expected the {UNAUTHORIZED_MESSAGE:?} \
                 body, got {}",
                answer.summary()
            );
        }
        for dropped in 0..row.query.len() {
            let query: Vec<(&str, &str)> = row
                .query
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != dropped)
                .map(|(_, parameter)| *parameter)
                .collect();
            let presentation = format!(
                "without the {:?} query parameter",
                row.query[dropped].0
            );
            let answer = invoke(&emulator, &endpoint_host, row.headers, &query).await;
            served_by_the_data_plane(case, &presentation, &answer);
            assert_eq!(
                answer.status, UNAUTHORIZED,
                "case {case:?} / {presentation}: expected {UNAUTHORIZED}, got {}",
                answer.summary()
            );
            assert_eq!(
                message(&answer),
                UNAUTHORIZED_MESSAGE,
                "case {case:?} / {presentation}: expected the {UNAUTHORIZED_MESSAGE:?} \
                 body, got {}",
                answer.summary()
            );
        }
    }
}
