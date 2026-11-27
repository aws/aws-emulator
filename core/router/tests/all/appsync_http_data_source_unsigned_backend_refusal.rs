// SPDX-License-Identifier: CC0-1.0

//! What an AppSync `HTTP` data source's UNSIGNED backend call is REFUSED with, driven end to
//! end over the real `MultiServiceRouter` with request authentication ON — AppSync builds and
//! sends the call, and an AWS query-protocol front door of this same emulator answers it.
//!
//! RULE, stated one level above the recorded case:
//!
//! > An AWS query-protocol front door tells the two credential failures apart. A request that
//! > arrives presenting NO SigV4 credential — neither of SigV4's two presentations — never
//! > named a credential to resolve, so it is refused HTTP 403 with the MISSING-credential
//! > class: `Type` `Sender`, `Code` `MissingAuthenticationToken` and that class's fixed
//! > sentence. The unknown-credential class (`InvalidClientTokenId`, "the security token …
//! > is invalid") is reserved for a credential that WAS presented and resolves to nothing, so
//! > a caller that sent none is never told the key it did not send is unknown. An AppSync
//! > `HTTP` data source presents a credential only where its
//! > `httpConfig.authorizationConfig` names `AWS_IAM` — the one authorization type the model
//! > declares — so the unsigned twin of a signed data source observes exactly that refusal,
//! > bound into `$ctx.result` as the `statusCode` and the `body` the endpoint answered and
//! > forwarded verbatim by the response half, while the signed twin is SERVED and reports the
//! > session of the role the data source names.
//!
//! The rows span what the rule generalises over: whether the data source configures `AWS_IAM`
//! authorization at all, WHERE the backend request carries its call (the query string of a
//! `GET` and the form-encoded body of a `POST`), and WHICH query-protocol front door answers
//! it (the token service and the identity service, two services whose own vocabularies name
//! this class). The served twin is the row that shows the endpoint is not simply broken: the
//! same endpoint, the same call, signed.
//!
//! Three services have to agree for the rule to hold — AppSync decides whether to sign and
//! sends the call, the front door classifies what arrived, and IAM holds the role a signed call
//! is made as — so this is the target that can pin it. The GraphQL request is POSTed to the
//! address the API itself PUBLISHED, as a client does, and request authentication is ON from
//! the start, because the classification under test is the front door's own.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appsync_lambda_data_source_and_authorizer::post_published_uri;
use crate::query_wire::{form_encode, opt_element};

/// The Region every call of this suite is made in and signed for.
const REGION: &str = "us-east-1";

/// The API versions the two query-protocol front doors publish, each as its own model states
/// it: `smithy_query(query="service_info")` of `AWSSecurityTokenServiceV20110615` and of
/// `AWSIdentityManagementV20100508`.
const STS_API_VERSION: &str = "2011-06-15";
const IAM_API_VERSION: &str = "2010-05-08";

/// The signing name a call to the token service is signed under, which is what an `AWS_IAM`
/// data source's `awsIamConfig.signingServiceName` has to name for such a call to be admitted.
const STS_SIGNING_NAME: &str = "sts";

/// The role the signed data source names: "the system assumes this role when accessing the data
/// source" (`DataSource$serviceRoleArn`).
const SERVICE_ROLE: &str = "appsync-http-data-source-sts";

/// The session name AppSync opens a data source's role session with, which the principal a
/// signed call speaks for ends in.
const ROLE_SESSION_NAME: &str = "APPSYNC_ASSUME_ROLE";

/// The three data sources every API of this suite holds: the token-service endpoint AppSync
/// signs for, the SAME endpoint with no authorization configured at all, and the identity
/// service's endpoint with none either.
const SIGNED_SOURCE: &str = "stsSigned";
const PLAIN_SOURCE: &str = "stsPlain";
const OTHER_PLAIN_SOURCE: &str = "iamPlain";

/// The `Type`, `Code` and `Message` an AWS query-protocol front door refuses a request that
/// presented NO credential with: the fault is the caller's, the class is the one that says
/// "you sent no credentials", and the sentence is entirely fixed text with no part taken from
/// the request or from any stored state. The tree states that sentence once, for every service
/// its one front door signs for (`service_common::sigv4_presentation`), and the codes are each
/// service's own (`service_common::authentication_refusal`); they are written out here because
/// this target's manifest holds no path to that crate.
const SENDER_FAULT: &str = "Sender";
const MISSING_CREDENTIAL_CODE: &str = "MissingAuthenticationToken";
const MISSING_CREDENTIAL_MESSAGE: &str = "Request is missing Authentication Token";

/// The class a query-protocol front door reserves for a credential that WAS presented and
/// resolves to nothing, which no row of this suite may be answered with: it sends a client to
/// key validity, where a caller that presented nothing has to be sent to credential DISCOVERY.
const UNRESOLVED_CREDENTIAL_CODE: &str = "InvalidClientTokenId";

/// How long a wait on a CONDITION may take, and how often this suite looks again. Generous:
/// what is under test is what the front door answers, never how fast the host reaches a
/// transition.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The schema every API of this suite is created with: one field per row, each resolving to the
/// status and the body its backend answered.
const SCHEMA: &str = "\
type Answer {\n  statusCode: Int\n  body: String\n}\n\
\n\
type Query {\n  \
  unsignedInQueryString: Answer\n  \
  unsignedInFormBody: Answer\n  \
  unsignedAtTheIdentityService: Answer\n  \
  signedInQueryString: Answer\n}\n";

/// The response half every resolver of this suite carries: a step that did not resolve is
/// re-raised as the field's own error — the `$util.error($ctx.error.message, $ctx.error.type)`
/// idiom — and otherwise the field takes the status and the body the endpoint answered,
/// forwarded verbatim.
const RESPONSE_TEMPLATE: &str = "#if($ctx.error)$util.error($ctx.error.message, \
                                 $ctx.error.type)#end\
                                 {\"statusCode\": $ctx.result.statusCode, \
                                 \"body\": $util.toJson($ctx.result.body)}";

/// The three services this suite spans, over ONE engine, with request authentication ON:
/// whether what arrived at the front door presented a credential is the whole question here.
///
/// Built here rather than through the shared helper the sibling AppSync suites use: that one
/// stands a router up with authentication OFF and signs its setup calls with a header a
/// verifier never checks, which cannot serve a suite whose subject is the verifier's own
/// classification — so the router, its setup calls and the key they are signed with are this
/// suite's own, and the data-plane client stays the shared one.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService::new(engine),
    );
    router
}

/// The key pair this process publishes once request authentication is on, which every SETUP
/// call of this suite is signed with.
fn published() -> &'static (String, String) {
    static PUBLISHED: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    PUBLISHED.get_or_init(|| {
        let minted = ::emulator_server::SigV4Authenticator::minted_credentials()
            .expect("turning request authentication on publishes this process's keys");
        (
            minted.access_key_id.clone(),
            minted.secret_access_key.clone(),
        )
    })
}

/// One setup request: what it is addressed to, what it is signed for, and what it carries.
struct Wire<'a> {
    signing_name: &'a str,
    method: &'a str,
    authority: String,
    path: &'a str,
    content_type: &'a str,
    body: Vec<u8>,
}

/// Dispatch one SETUP call through the router's front door, SIGNED with the published key pair
/// the way an SDK signs it, and answer its status and body.
///
/// The ONE signing site of this suite, so no two setup calls can differ on how they are signed.
async fn call(router: &MultiServiceRouter, wire: Wire<'_>) -> (u16, Vec<u8>) {
    let Wire {
        signing_name,
        method,
        authority,
        path,
        content_type,
        body,
    } = wire;
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{authority}{path}"))
        .header("host", authority.as_str())
        .header("content-type", content_type)
        .body(body)
        .expect("the test request builds");
    let (key_id, secret) = published();
    let identity = Credentials::new(key_id, secret, None, None, "the published key pair").into();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(signing_name)
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .expect("the signing parameters are complete")
        .into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let fields: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("a text header").to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        &method,
        &uri,
        fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(request.body()),
    )
    .expect("the request is signable");
    let (instructions, _) = sign(signable, &params)
        .expect("the request signs")
        .into_parts();
    instructions.apply_to_request_http1x(&mut request);
    let (parts, body) = request.into_parts();
    let response = router
        .dispatch(http::Request::from_parts(parts, bytes::Bytes::from(body)))
        .await;
    let (parts, body) = response.into_parts();
    (parts.status.as_u16(), body.to_vec())
}

/// One AppSync control-plane call (restJson1), asserting it was served.
async fn appsync_ok(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let payload = body.map_or_else(Vec::new, |body| body.to_string().into_bytes());
    let (status, answered) = call(
        router,
        Wire {
            signing_name: "appsync",
            method,
            authority: format!("appsync.{REGION}.amazonaws.com"),
            path,
            content_type: "application/json",
            body: payload,
        },
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    assert_eq!(
        status, 200,
        "setup: AppSync {method} {path} must answer 200, got {status} and {text}"
    );
    serde_json::from_slice(&answered).unwrap_or(Value::Null)
}

/// One IAM call over its awsQuery wire, asserting it was served, and answering the XML body.
async fn iam_ok(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = call(
        router,
        Wire {
            signing_name: "iam",
            method: "POST",
            authority: format!("iam.{REGION}.amazonaws.com"),
            path: "/",
            content_type: "application/x-www-form-urlencoded",
            body: body.into_bytes(),
        },
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    assert_eq!(
        status, 200,
        "setup: IAM {action} must answer 200, got {status} and {text}"
    );
    text
}

/// A required string member of a setup answer.
fn member(what: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what} must carry a non-empty {name:?}, got {body}"))
}

/// Create the role the signed data source names, trusted by the AppSync service principal, and
/// answer its ARN.
async fn create_role(router: &MultiServiceRouter) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "appsync.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam_ok(
        router,
        "CreateRole",
        &[
            ("RoleName", SERVICE_ROLE),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    opt_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: CreateRole reports no <Arn>: {created}"))
}

/// The request half of one row, as the HTTP data source's own request object: a call carried in
/// the QUERY STRING of a `GET`, as a query-protocol client may send one.
fn get_in_query_string(action: &str, version: &str) -> String {
    format!(
        r#"{{"version": "2018-05-29", "method": "GET", "resourcePath": "/",
"params": {{"query": {{"Action": "{action}", "Version": "{version}"}}}}}}"#
    )
}

/// The same call carried in the form-encoded BODY of a `POST`, the other place a query-protocol
/// request puts it.
fn post_in_form_body(action: &str, version: &str) -> String {
    format!(
        r#"{{"version": "2018-05-29", "method": "POST", "resourcePath": "/",
"params": {{"headers": {{"content-type": "application/x-www-form-urlencoded"}},
"body": "Action={action}&Version={version}"}}}}"#
    )
}

/// Everything a row is driven against: the address the API published and the key a call on an
/// `API_KEY` API presents.
struct Served {
    graphql_uri: String,
    api_key: String,
}

/// Stand the whole estate up: the role, the GraphQL API, its schema, its three HTTP data
/// sources over the two front doors, and one UNIT resolver per field.
async fn standing(router: &MultiServiceRouter) -> Served {
    let role_arn = create_role(router).await;
    let sts_endpoint = format!("https://sts.{REGION}.amazonaws.com");
    // The identity service's own global endpoint, which names no Region in its host: the second
    // front door this rule has to hold at.
    let iam_endpoint = "https://iam.amazonaws.com".to_string();

    let created = appsync_ok(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": "appsync-http-unsigned-backend",
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    let api = &created["graphqlApi"];
    let api_id = member("CreateGraphqlApi", api, "apiId");
    let graphql_uri = api["uris"]["GRAPHQL"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        graphql_uri.contains("127.0.0.1:"),
        "setup: uris.GRAPHQL must be a URL this emulator serves, got {graphql_uri:?}"
    );

    appsync_ok(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({ "definition": ::aws_smithy_types::base64::encode(SCHEMA) })),
    )
    .await;
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let settled = appsync_ok(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        if settled["status"].as_str() == Some("SUCCESS") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the schema creation must be performed, last read {settled}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }

    // The signed data source: the one whose `authorizationConfig` names `AWS_IAM`, for the
    // service and the Region a call to the token service has to be signed under, made as the
    // role it names.
    appsync_ok(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": SIGNED_SOURCE,
            "type": "HTTP",
            "serviceRoleArn": role_arn,
            "httpConfig": {
                "endpoint": sts_endpoint,
                "authorizationConfig": {
                    "authorizationType": "AWS_IAM",
                    "awsIamConfig": {
                        "signingRegion": REGION,
                        "signingServiceName": STS_SIGNING_NAME,
                    },
                },
            },
        })),
    )
    .await;
    // The two unsigned twins: the same endpoint, and another front door, with no
    // `authorizationConfig` at all and no role — so the call must go out anonymous.
    for (name, endpoint) in [
        (PLAIN_SOURCE, &sts_endpoint),
        (OTHER_PLAIN_SOURCE, &iam_endpoint),
    ] {
        appsync_ok(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(json!({
                "name": name,
                "type": "HTTP",
                "httpConfig": { "endpoint": endpoint },
            })),
        )
        .await;
    }

    let resolvers: &[(&str, &str, String)] = &[
        (
            "unsignedInQueryString",
            PLAIN_SOURCE,
            get_in_query_string("GetCallerIdentity", STS_API_VERSION),
        ),
        (
            "unsignedInFormBody",
            PLAIN_SOURCE,
            post_in_form_body("GetCallerIdentity", STS_API_VERSION),
        ),
        (
            "unsignedAtTheIdentityService",
            OTHER_PLAIN_SOURCE,
            post_in_form_body("GetUser", IAM_API_VERSION),
        ),
        (
            "signedInQueryString",
            SIGNED_SOURCE,
            get_in_query_string("GetCallerIdentity", STS_API_VERSION),
        ),
    ];
    for (field, data_source, request) in resolvers {
        appsync_ok(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": data_source,
                "requestMappingTemplate": request,
                "responseMappingTemplate": RESPONSE_TEMPLATE,
            })),
        )
        .await;
    }

    let key = appsync_ok(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    let api_key = member("CreateApiKey", &key["apiKey"], "id");
    Served {
        graphql_uri,
        api_key,
    }
}

/// Select `field` through the address the API published and answer what the result reports for
/// it: the field's value and the errors the result carries.
async fn selected(case: &str, served: &Served, field: &str) -> (Value, Vec<(String, String)>) {
    let document = json!({ "query": format!("{{ {field} {{ statusCode body }} }}") });
    let (status, answered) = post_published_uri(
        case,
        &served.graphql_uri,
        &document,
        &[("x-api-key", served.api_key.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the GraphQL endpoint must answer 200, got {status} {answered}"
    );
    let errors = answered["errors"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["errorType"].as_str().unwrap_or_default().to_string(),
                        entry["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    (answered["data"][field].clone(), errors)
}

/// The body the field reports, which is the one the backend answered, forwarded verbatim.
fn forwarded_body(case: &str, value: &Value) -> String {
    value["body"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the field must report a body, got {value}"))
        .to_string()
}

/// One row: the case, the field it selects, and what the backend must answer it — the
/// `(Type, Code, Message)` of a refusal, or `None` for the row that must be SERVED.
struct Row<'a> {
    case: &'a str,
    field: &'a str,
    refused: Option<(&'a str, &'a str, &'a str)>,
}

/// RULE: a backend request an `HTTP` data source sends with no credential at all is refused
/// with the MISSING-credential class and never with the unknown-credential one, whichever
/// query-protocol front door answers it and wherever the request carries its call — and the
/// signed twin of the same call is served.
#[tokio::test]
async fn an_unsigned_data_source_call_is_refused_for_the_credential_it_never_presented() {
    let router = build_router();
    let served = standing(&router).await;

    let rows: &[Row<'_>] = &[
        Row {
            case: "an unsigned call carrying its action in the query string",
            field: "unsignedInQueryString",
            refused: Some((
                SENDER_FAULT,
                MISSING_CREDENTIAL_CODE,
                MISSING_CREDENTIAL_MESSAGE,
            )),
        },
        Row {
            case: "an unsigned call carrying its action in a form-encoded body",
            field: "unsignedInFormBody",
            refused: Some((
                SENDER_FAULT,
                MISSING_CREDENTIAL_CODE,
                MISSING_CREDENTIAL_MESSAGE,
            )),
        },
        Row {
            case: "an unsigned call at another query-protocol front door",
            field: "unsignedAtTheIdentityService",
            refused: Some((
                SENDER_FAULT,
                MISSING_CREDENTIAL_CODE,
                MISSING_CREDENTIAL_MESSAGE,
            )),
        },
        Row {
            case: "the signed twin of the first row, on the same endpoint",
            field: "signedInQueryString",
            refused: None,
        },
    ];

    for Row {
        case,
        field,
        refused,
    } in rows
    {
        let (value, errors) = selected(case, &served, field).await;
        // The backend's answer — a refusal included — is the step's RESULT, so the GraphQL call
        // reports the field rather than an error of it.
        assert!(
            errors.is_empty(),
            "case {case:?}: expected the field to resolve to the endpoint's answer, got errors \
             {errors:?}"
        );
        let body = forwarded_body(case, &value);
        match refused {
            Some((expected_type, expected_code, expected_message)) => {
                assert_eq!(
                    value["statusCode"].as_u64(),
                    Some(403),
                    "case {case:?}: expected the front door's own 403, got {value}"
                );
                let actual = (
                    opt_element(&body, "Type").unwrap_or_default(),
                    opt_element(&body, "Code").unwrap_or_default(),
                    opt_element(&body, "Message").unwrap_or_default(),
                );
                assert_eq!(
                    (actual.0.as_str(), actual.1.as_str(), actual.2.as_str()),
                    (*expected_type, *expected_code, *expected_message),
                    "case {case:?}: expected the missing-credential class in AWS's own words, \
                     got {actual:?} in {body}"
                );
                assert!(
                    !body.contains(UNRESOLVED_CREDENTIAL_CODE),
                    "case {case:?}: a request that presented no credential must not be answered \
                     {UNRESOLVED_CREDENTIAL_CODE}, got {body}"
                );
            }
            None => {
                assert_eq!(
                    value["statusCode"].as_u64(),
                    Some(200),
                    "case {case:?}: expected the signed call to be served, got {value}"
                );
                // WHO the signed call was made as: a session of the role the data source names,
                // under the session name AppSync opens one with.
                let arn = opt_element(&body, "Arn").unwrap_or_default();
                let session = format!(":assumed-role/{SERVICE_ROLE}/{ROLE_SESSION_NAME}");
                assert!(
                    arn.starts_with("arn:aws:sts::") && arn.ends_with(&session),
                    "case {case:?}: expected the identity of a session of {SERVICE_ROLE}, got \
                     {arn:?} in {body}"
                );
            }
        }
    }
}
