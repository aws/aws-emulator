// SPDX-License-Identifier: CC0-1.0

//! The `CloudFront-*` request headers the EDGE GENERATES for an origin request, end to end
//! over HTTP across the services on one engine.
//!
//! RULE: the edge GENERATES the `CloudFront-*` headers that describe the viewer's CONNECTION
//! rather than relaying anything the viewer stated for them — `CloudFront-Viewer-Country` as
//! an `ISO 3166-1 alpha-2` code (two uppercase letters), resolved from the viewer and the same
//! for the same viewer whichever scheme it dialled, and `CloudFront-Forwarded-Proto` as
//! exactly the scheme that viewer dialled the edge over — and that generated value is the one
//! value such a header has WHEREVER it is read: it is what the origin request carries where the
//! matched cache behaviour's ORIGIN REQUEST POLICY NAMES the header
//! (`OriginRequestPolicyHeadersConfig$HeaderBehavior`: `whitelist` and
//! `allViewerAndWhitelistCloudFront` carry a list that may name one, while `allViewer`,
//! `allExcept` and `none` name none of them, because a generated header is not a header the
//! viewer sent), and it is what the CACHE KEY carries where the behaviour's CACHE POLICY names
//! it. A behaviour whose policy does not name one, and a behaviour with no origin request
//! policy at all, sends the origin none of it.
//!
//! Only a cross-service suite can pin this: the behaviour, its policies and the generated
//! values are CloudFront's, and the thing that REPORTS what reached the origin is an origin
//! another service serves — an API Gateway stage whose one method is a `MOCK` integration
//! reflecting its own method request (`$input.params(...)`), and, over the topology a function
//! URL behind a distribution is, a running Lambda function that counts each origin request it
//! serves in its own Amazon DynamoDB table. Each row therefore reads the origin request off
//! what the viewer was served. The sibling [`super::cloudfront_apigateway_origin_edge`] suite
//! pins the legacy `ForwardedValues` half of the same forwarding decision.
//!
//! The rows differ along what the rule generalises over: the SCHEME the viewer dialled (`https`
//! and plaintext `http`), what the viewer itself stated for the two generated headers (nothing,
//! and a value of its own for each), which behaviour the viewer path matches (the one naming an
//! origin request policy and the one naming none), every `HeaderBehavior` a policy can carry
//! (with the generated headers named, one of them named, and neither), which generated header
//! the cache key names, the ORIGIN the behaviour targets, and HOW the viewer addressed the edge
//! (through the emulator's endpoint, and by dialling the `host:port` the distribution
//! published).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness is the sibling CloudFront-over-API-Gateway suite's, reused rather than
// repeated: one reading of how a setup call, a viewer request and the wait for a deployed
// distribution are made, so a change to the setup path is made once.
use super::cloudfront_apigateway_origin_edge::{
    apigw, authorization, cloudfront, dispatch, emulator, member, view, wait_until_deployed,
    Answer, API_VERSION, POLL, POLLS, REGION, STAGE,
};

/// The two resources the reflecting api carries: the one a viewer reaches through the
/// behaviour that names an origin request policy, and the one it reaches through the default
/// behaviour, which names none.
const EDGE_PATH: &str = "edge";
const BARE_PATH: &str = "bare";

/// The path pattern of the ordered cache behaviour, which [`EDGE_PATH`] matches and
/// [`BARE_PATH`] does not.
const EDGE_PATTERN: &str = "/edge*";

/// The two headers under test, spelled as `CloudFront-Viewer-Country` and
/// `CloudFront-Forwarded-Proto` — the names a policy's `Headers` list carries for them.
const VIEWER_COUNTRY: &str = "CloudFront-Viewer-Country";
const FORWARDED_PROTO: &str = "CloudFront-Forwarded-Proto";

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero, so every
/// viewer request below reaches the origin and no row reads another row's cached answer. Its
/// id is the one the service's own managed catalog publishes.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// What a viewer may state for the two generated headers, to be dropped rather than relayed.
const SPOOFED_COUNTRY: &str = "ZZ";
const SPOOFED_PROTO: &str = "gopher";

/// The `DefaultTTL` the keyed cache policy below holds an answer for: long enough that no row
/// can outlive it, and nothing waits for it to pass.
const CACHED_TTL_SECONDS: u32 = 3600;

/// `X-Cache` for an answer the origin produced for this request, and for one the edge cache
/// held.
const MISS: &str = "Miss from cloudfront";
const HIT: &str = "Hit from cloudfront";

/// A REST api whose two resources each answer a `MOCK` integration that REFLECTS the two
/// generated headers its method request arrived with, deployed to [`STAGE`]. Answers the
/// api's id.
///
/// The response template reads the METHOD REQUEST's own parameters, so the document the stage
/// answers carries exactly what the origin request carried for each header — and the empty
/// string for one the origin request did not carry at all.
async fn deployed_reflecting_api(router: &MultiServiceRouter) -> String {
    let created = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "cloudfront-generated-header-echo" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    for path_part in [EDGE_PATH, BARE_PATH] {
        reflecting_resource(router, &api, &root, path_part).await;
    }
    apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    api
}

/// One resource of the reflecting api: `GET /{path_part}`, a `MOCK` integration whose `200`
/// response reflects the two generated headers.
async fn reflecting_resource(
    router: &MultiServiceRouter,
    api: &str,
    root: &str,
    path_part: &str,
) {
    let created = apigw(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": path_part })),
        201,
    )
    .await;
    let resource = member("CreateResource", &created, "id");
    apigw(
        router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
        Some(json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": r#"{"statusCode":200}"# },
        })),
        201,
    )
    .await;
    apigw(
        router,
        "PutMethodResponse",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET/responses/200"),
        Some(json!({})),
        201,
    )
    .await;
    let template = format!(
        r#"{{"country":"$input.params('{VIEWER_COUNTRY}')","proto":"$input.params('{FORWARDED_PROTO}')"}}"#
    );
    apigw(
        router,
        "PutIntegrationResponse",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"),
        Some(json!({
            "statusCode": "200",
            "responseTemplates": { "application/json": template },
        })),
        201,
    )
    .await;
}

/// `cloudfront:CreateCachePolicy` whose cache key carries the HEADER `keyed_header` and
/// nothing else beyond the URL path, answering the policy's id.
///
/// `DefaultTTL` is [`CACHED_TTL_SECONDS`], so an answer the edge served once is held: a second
/// request the policy says is the SAME object is answered from the cache, and one it says is a
/// different object is fetched again — which is what makes "the key carries the edge's own
/// value for a generated header" observable from outside.
async fn create_cache_policy(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    keyed_header: &str,
) -> String {
    let body = format!(
        "<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         <DefaultTTL>{CACHED_TTL_SECONDS}</DefaultTTL>\
         <MaxTTL>{CACHED_TTL_SECONDS}</MaxTTL>\
         <MinTTL>0</MinTTL>\
         <ParametersInCacheKeyAndForwardedToOrigin>\
         <EnableAcceptEncodingGzip>false</EnableAcceptEncodingGzip>\
         <HeadersConfig><HeaderBehavior>whitelist</HeaderBehavior>\
         <Headers><Quantity>1</Quantity><Items><Name>{keyed_header}</Name></Items></Headers>\
         </HeadersConfig>\
         <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
         <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior>\
         </QueryStringsConfig>\
         </ParametersInCacheKeyAndForwardedToOrigin>\
         </CachePolicyConfig>"
    );
    let created = cloudfront(
        router,
        "POST",
        &format!("/{API_VERSION}/cache-policy"),
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup {case:?}: CreateCachePolicy must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let id = created.element("Id");
    assert!(
        !id.is_empty(),
        "setup {case:?}: the created cache policy reports no Id: {:?}",
        created.text()
    );
    id
}

/// `cloudfront:CreateOriginRequestPolicy` carrying `header_behavior` over `names`, answering
/// the policy's id. `None` for `names` leaves the `Headers` wrapper out entirely, which is
/// what a behaviour naming no header list submits.
async fn create_origin_request_policy(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    header_behavior: &str,
    names: Option<&[&str]>,
) -> String {
    let headers = names
        .map(|names| {
            let items: String = names
                .iter()
                .map(|name| format!("<Name>{name}</Name>"))
                .collect();
            let items = if names.is_empty() {
                String::new()
            } else {
                format!("<Items>{items}</Items>")
            };
            format!(
                "<Headers><Quantity>{}</Quantity>{items}</Headers>",
                names.len()
            )
        })
        .unwrap_or_default();
    let body = format!(
        "<OriginRequestPolicyConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         <HeadersConfig><HeaderBehavior>{header_behavior}</HeaderBehavior>{headers}\
         </HeadersConfig>\
         <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
         <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior>\
         </QueryStringsConfig>\
         </OriginRequestPolicyConfig>"
    );
    let created = cloudfront(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-request-policy"),
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup {case:?}: CreateOriginRequestPolicy must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let id = created.element("Id");
    assert!(
        !id.is_empty(),
        "setup {case:?}: the created policy reports no Id: {:?}",
        created.text()
    );
    id
}

/// `cloudfront:CreateDistribution` over the api stage `api` as a custom origin, answering the
/// distribution's `(id, DomainName)`.
///
/// The ORDERED behaviour ([`EDGE_PATTERN`]) names `origin_request_policy` and the cache policy
/// `edge_cache_policy`; the DEFAULT behaviour names no origin request policy — the behaviour
/// the rule says receives no generated header at all — and [`CACHING_DISABLED`], so nothing a
/// row drives through it is served from the edge cache.
async fn create_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    api: &str,
    origin_request_policy: &str,
    edge_cache_policy: &str,
) -> (String, String) {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>api-origin</Id>\
         <DomainName>{api}.execute-api.{REGION}.amazonaws.com</DomainName>\
         <OriginPath>/{STAGE}</OriginPath>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         </CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>api-origin</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
         <PathPattern>{EDGE_PATTERN}</PathPattern>\
         <TargetOriginId>api-origin</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{edge_cache_policy}</CachePolicyId>\
         <OriginRequestPolicyId>{origin_request_policy}</OriginRequestPolicyId>\
         </CacheBehavior></Items></CacheBehaviors>\
         <Comment>the generated headers an origin request carries</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    );
    let created = cloudfront(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup {case:?}: CreateDistribution must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let (id, domain) = (created.element("Id"), created.element("DomainName"));
    assert!(
        !id.is_empty() && !domain.is_empty(),
        "setup {case:?}: the created distribution reports no Id or DomainName: {:?}",
        created.text()
    );
    (id, domain)
}

/// What one served viewer request reported: the `X-Cache` the edge answered it with, and
/// what the ORIGIN was told for the two generated headers — the empty string for one the
/// origin request did not carry.
struct Served {
    x_cache: String,
    country: String,
    proto: String,
}

/// Drive one viewer request and read [`Served`] off what it was answered.
async fn reflected(
    router: &MultiServiceRouter,
    case: &str,
    scheme: &str,
    domain: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Served {
    let answer = view(router, scheme, domain, path, headers).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the origin's answer through the edge, got {} with {:?}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    let read = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Served {
        x_cache: answer.header("x-cache"),
        country: read("country"),
        proto: read("proto"),
    }
}

/// Whether `value` is an `ISO 3166-1 alpha-2` code as this header carries one: "a two-letter,
/// uppercase country code", which is the spelling `GeoRestriction$Items` documents too.
fn is_alpha_2(value: &str) -> bool {
    value.len() == 2 && value.chars().all(|letter| letter.is_ascii_uppercase())
}

/// One row of the viewer table: the request a viewer makes, and what the origin must then
/// report.
struct ViewerRow {
    case: &'static str,
    /// The scheme the viewer dialled the edge over.
    scheme: &'static str,
    /// The viewer path, which also decides WHICH behaviour serves it.
    path: &'static str,
    /// What the viewer itself stated for the generated headers.
    stated: &'static [(&'static str, &'static str)],
    /// Whether the matched behaviour's policy names the generated headers at all.
    generated: bool,
}

#[tokio::test]
async fn the_edge_generates_the_viewer_country_and_the_scheme_the_viewer_dialled() {
    let router = emulator();
    let api = deployed_reflecting_api(&router).await;
    let case = "a policy whitelisting both generated headers";
    let policy = create_origin_request_policy(
        &router,
        case,
        "generated-headers-whitelist",
        "whitelist",
        Some(&[VIEWER_COUNTRY, FORWARDED_PROTO]),
    )
    .await;
    let (id, domain) = create_distribution(
        &router,
        case,
        "generated-headers",
        &api,
        &policy,
        CACHING_DISABLED,
    )
    .await;
    wait_until_deployed(&router, case, &id).await;

    // The rows differ along the scheme the viewer dialled, what it stated for the two
    // generated headers, and which behaviour its path matches.
    let rows: &[ViewerRow] = &[
        ViewerRow {
            case: "a viewer that dialled TLS and stated neither header",
            scheme: "https",
            path: "/edge",
            stated: &[],
            generated: true,
        },
        ViewerRow {
            case: "a viewer that dialled plaintext and stated neither header",
            scheme: "http",
            path: "/edge",
            stated: &[],
            generated: true,
        },
        ViewerRow {
            case: "a viewer that stated a country and a scheme of its own, over TLS",
            scheme: "https",
            path: "/edge",
            stated: &[
                (VIEWER_COUNTRY, SPOOFED_COUNTRY),
                (FORWARDED_PROTO, SPOOFED_PROTO),
            ],
            generated: true,
        },
        ViewerRow {
            case: "the same spoofing viewer over plaintext",
            scheme: "http",
            path: "/edge",
            stated: &[
                (VIEWER_COUNTRY, SPOOFED_COUNTRY),
                (FORWARDED_PROTO, SPOOFED_PROTO),
            ],
            generated: true,
        },
        ViewerRow {
            case: "a behaviour with no origin request policy, over TLS",
            scheme: "https",
            path: "/bare",
            stated: &[],
            generated: false,
        },
        ViewerRow {
            case: "a behaviour with no origin request policy, over plaintext",
            scheme: "http",
            path: "/bare",
            stated: &[],
            generated: false,
        },
        ViewerRow {
            case: "a viewer that spoofs both headers at a behaviour naming no policy",
            scheme: "https",
            path: "/bare",
            stated: &[
                (VIEWER_COUNTRY, SPOOFED_COUNTRY),
                (FORWARDED_PROTO, SPOOFED_PROTO),
            ],
            generated: false,
        },
    ];

    // The ONE country this emulator's viewers are in, read off the first row that is served
    // it: the value is the edge's own, and the rule is that it is the same for the same
    // viewer whichever scheme it dialled, so no row states it.
    let mut resolved_country: Option<String> = None;
    for row in rows {
        let case = row.case;
        let Served { country, proto, .. } =
            reflected(&router, case, row.scheme, &domain, row.path, row.stated).await;
        if !row.generated {
            assert_eq!(
                (country.as_str(), proto.as_str()),
                ("", ""),
                "case {case:?}: a behaviour whose origin request policy names no generated \
                 header must send the origin neither, got the country {country:?} and the \
                 scheme {proto:?}"
            );
            continue;
        }
        assert!(
            is_alpha_2(&country),
            "case {case:?}: expected a two-letter uppercase country code at the origin, got \
             {country:?}"
        );
        assert_ne!(
            country, SPOOFED_COUNTRY,
            "case {case:?}: expected the country the EDGE resolved, not the one the viewer \
             stated, got {country:?}"
        );
        assert_eq!(
            proto, row.scheme,
            "case {case:?}: expected the origin to be told the scheme the viewer dialled \
             ({:?}), got {proto:?}",
            row.scheme
        );
        match &resolved_country {
            None => resolved_country = Some(country),
            Some(first) => assert_eq!(
                &country, first,
                "case {case:?}: expected the same viewer's country on every scheme ({first:?}), \
                 got {country:?}"
            ),
        }
    }
    assert!(
        resolved_country.is_some(),
        "the table must drive at least one request through the behaviour that names the \
         generated headers"
    );
}

// ---------------------------------------------------------------------------
// The same rule over the other origin kind: a Lambda function URL
// ---------------------------------------------------------------------------

/// The role the echo function runs as, the function's own name, the table it counts its
/// invocations in, and the stack that declares all three.
const ECHO_ROLE: &str = "cloudfront-generated-header-echo-role";
const ECHO_FUNCTION: &str = "cloudfront-generated-header-echo";
const ECHO_TABLE: &str = "cloudfront-generated-header-echo-counter";
const ECHO_STACK: &str = "cloudfront-generated-header-echo-stack";

/// The item the echo function counts its invocations under, and the attribute holding the
/// count.
const COUNTER_KEY: &str = "invocations";
const COUNTER_ATTRIBUTE: &str = "seen";

/// The `StackStatus` the setup waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The handler behind the function URL: it COUNTS the origin request it is serving in its
/// table, with the AWS SDK its runtime supplies, and reports what that request carried for
/// the two generated headers — which a function URL event states lowercased in its `headers`
/// map. The count is what says the ORIGIN really ran for the answer the viewer got.
const ECHO_HANDLER: &str = r#"
import json
import os

import boto3


def lambda_handler(event, context):
    headers = event.get("headers") or {}
    boto3.client("dynamodb").update_item(
        TableName=os.environ["TABLE"],
        Key={"id": {"S": "COUNTER_KEY"}},
        UpdateExpression="ADD COUNTER_ATTRIBUTE :one",
        ExpressionAttributeValues={":one": {"N": "1"}},
    )
    return {
        "statusCode": 200,
        "headers": {"content-type": "application/json"},
        "body": json.dumps(
            {
                "country": headers.get("cloudfront-viewer-country", ""),
                "proto": headers.get("cloudfront-forwarded-proto", ""),
            }
        ),
    }
"#;

/// The WHOLE emulator as it is assembled, on one engine: this half of the suite needs
/// CloudFormation, IAM, Lambda and CloudFront to agree, and the function the edge fetches is
/// invoked over the same in-process transport.
fn assembled() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// One awsQuery call of `service`, answered with the status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(key));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let host = format!("{service}.{REGION}.amazonaws.com");
    dispatch(
        router,
        "POST",
        &host,
        &format!("https://{host}/"),
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization(service)),
        ],
        Bytes::from(body),
    )
    .await
}

/// One AWS Lambda `restJson1` call that has to be accepted with `expected`.
async fn lambda(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let host = format!("lambda.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &host,
        &format!("https://{host}{path}"),
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answered.status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One Amazon DynamoDB `awsJson1_0` call.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let host = format!("dynamodb.{REGION}.amazonaws.com");
    dispatch(
        router,
        "POST",
        &host,
        &format!("https://{host}/"),
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("dynamodb")),
            ("x-amz-target", &format!("DynamoDB_20120810.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// How many invocations the echo function has counted in its table, read through DynamoDB's
/// own strongly consistent `GetItem` — `0` before the first one.
async fn counted_invocations(router: &MultiServiceRouter) -> u32 {
    let read = dynamodb(
        router,
        "GetItem",
        json!({
            "TableName": ECHO_TABLE,
            "Key": { "id": { "S": COUNTER_KEY } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "GetItem on the counter must succeed, got {} with {:?}",
        read.status,
        read.text()
    );
    read.json()
        .get("Item")
        .and_then(|item| item.get(COUNTER_ATTRIBUTE))
        .and_then(|number| number.get("N"))
        .and_then(Value::as_str)
        .and_then(|seen| seen.parse::<u32>().ok())
        .unwrap_or_default()
}

/// The HOST of a public function URL over the echo handler: the stack that declares the
/// function, the table it counts in and its execution role, then the URL and the grant that
/// makes it public.
async fn function_url_host(router: &MultiServiceRouter) -> String {
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Counter": {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": ECHO_TABLE,
                    "AttributeDefinitions": [
                        {"AttributeName": "id", "AttributeType": "S"},
                    ],
                    "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
                    "BillingMode": "PAY_PER_REQUEST",
                },
            },
            "EchoRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": ECHO_ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "count-its-own-invocations",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": ["dynamodb:UpdateItem"],
                                "Resource": {"Fn::GetAtt": ["Counter", "Arn"]},
                            }],
                        },
                    }],
                },
            },
            "Echo": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": ECHO_FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": {"Fn::GetAtt": ["EchoRole", "Arn"]},
                    "Timeout": 30,
                    "Environment": {"Variables": {"TABLE": {"Ref": "Counter"}}},
                    "Code": {"ZipFile": ECHO_HANDLER
                        .replace("COUNTER_KEY", COUNTER_KEY)
                        .replace("COUNTER_ATTRIBUTE", COUNTER_ATTRIBUTE)},
                },
            },
        },
    })
    .to_string();
    let created = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", ECHO_STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "setup: CreateStack must be accepted, got {} with {:?}",
        created.status,
        created.text()
    );
    let mut reported = String::new();
    for _ in 0..POLLS {
        let described = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStacks",
            &[("StackName", ECHO_STACK)],
        )
        .await;
        reported = described.element("StackStatus");
        if reported == CREATE_COMPLETE {
            break;
        }
        ::tokio::time::sleep(POLL).await;
    }
    assert_eq!(
        reported, CREATE_COMPLETE,
        "setup: the echo stack must reach {CREATE_COMPLETE}, last reported {reported:?}"
    );
    // An invocation needs the function Active, which is a span the service owes.
    let mut state = String::new();
    for _ in 0..POLLS {
        let configuration = lambda(
            router,
            "GetFunctionConfiguration",
            "GET",
            &format!("/2015-03-31/functions/{ECHO_FUNCTION}/configuration"),
            None,
            200,
        )
        .await;
        state = configuration["State"].as_str().unwrap_or_default().to_string();
        if state == "Active" {
            break;
        }
        ::tokio::time::sleep(POLL).await;
    }
    assert_eq!(
        state, "Active",
        "setup: the echo function must reach Active, last reported {state:?}"
    );
    let url = lambda(
        router,
        "CreateFunctionUrlConfig",
        "POST",
        &format!("/2021-10-31/functions/{ECHO_FUNCTION}/url"),
        Some(json!({ "AuthType": "NONE" })),
        201,
    )
    .await;
    // `AuthType=NONE` bypasses IAM authentication; the function's own resource policy is what
    // admits the anonymous caller the edge's unsigned origin fetch is.
    lambda(
        router,
        "AddPermission",
        "POST",
        &format!("/2015-03-31/functions/{ECHO_FUNCTION}/policy"),
        Some(json!({
            "StatementId": "public-url",
            "Action": "lambda:InvokeFunctionUrl",
            "Principal": "*",
            "FunctionUrlAuthType": "NONE",
        })),
        201,
    )
    .await;
    let endpoint = member("CreateFunctionUrlConfig", &url, "FunctionUrl");
    endpoint
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

/// `cloudfront:CreateDistribution` over the function URL `host` as a custom origin, with one
/// behaviour naming `origin_request_policy` — the topology a function URL behind a
/// distribution is: the viewer path reaches the function URL's own root path, so the api
/// suite's two resources are one function here.
async fn create_function_url_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    host: &str,
    origin_request_policy: &str,
) -> (String, String) {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>function-url</Id>\
         <DomainName>{host}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         </CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>function-url</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
         <PathPattern>{EDGE_PATTERN}</PathPattern>\
         <TargetOriginId>function-url</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         <OriginRequestPolicyId>{origin_request_policy}</OriginRequestPolicyId>\
         </CacheBehavior></Items></CacheBehaviors>\
         <Comment>a function url behind a distribution</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    );
    let created = cloudfront(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup {case:?}: CreateDistribution must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let (id, domain) = (created.element("Id"), created.element("DomainName"));
    assert!(
        !id.is_empty() && !domain.is_empty(),
        "setup {case:?}: the created distribution reports no Id or DomainName: {:?}",
        created.text()
    );
    (id, domain)
}

#[tokio::test]
async fn a_function_url_origin_is_told_the_country_and_the_scheme_the_viewer_dialled() {
    let router = assembled();
    let host = function_url_host(&router).await;
    let case = "a function URL origin under a policy whitelisting both generated headers";
    let policy = create_origin_request_policy(
        &router,
        case,
        "function-url-generated-headers",
        "whitelist",
        Some(&[VIEWER_COUNTRY, FORWARDED_PROTO]),
    )
    .await;
    let (id, domain) = create_function_url_distribution(
        &router,
        case,
        "function-url-generated-headers",
        &host,
        &policy,
    )
    .await;
    wait_until_deployed(&router, case, &id).await;

    // The rows differ along the scheme the viewer dialled and whether the behaviour its path
    // matches names an origin request policy at all — the same two dimensions the api-origin
    // table drives, now over a running function.
    let rows: &[ViewerRow] = &[
        ViewerRow {
            case: "a viewer that dialled TLS",
            scheme: "https",
            path: "/edge",
            stated: &[],
            generated: true,
        },
        ViewerRow {
            case: "a viewer that dialled plaintext",
            scheme: "http",
            path: "/edge",
            stated: &[],
            generated: true,
        },
        ViewerRow {
            case: "a viewer that stated a country and a scheme of its own",
            scheme: "https",
            path: "/edge",
            stated: &[
                (VIEWER_COUNTRY, SPOOFED_COUNTRY),
                (FORWARDED_PROTO, SPOOFED_PROTO),
            ],
            generated: true,
        },
        ViewerRow {
            case: "the behaviour that names no origin request policy",
            scheme: "https",
            path: "/bare",
            stated: &[],
            generated: false,
        },
    ];
    let mut resolved_country: Option<String> = None;
    let mut fetched = 0_u32;
    for row in rows {
        let case = row.case;
        let Served { country, proto, .. } =
            reflected(&router, case, row.scheme, &domain, row.path, row.stated).await;
        // Neither behaviour caches, so every row is an origin fetch: the function RAN for it,
        // and the count it keeps in its own table is what says so.
        fetched += 1;
        assert_eq!(
            counted_invocations(&router).await,
            fetched,
            "case {case:?}: expected the function to have run once per origin fetch ({fetched} \
             so far)"
        );
        if !row.generated {
            assert_eq!(
                (country.as_str(), proto.as_str()),
                ("", ""),
                "case {case:?}: a behaviour whose origin request policy names no generated \
                 header must send the origin neither, got the country {country:?} and the \
                 scheme {proto:?}"
            );
            continue;
        }
        assert!(
            is_alpha_2(&country),
            "case {case:?}: expected a two-letter uppercase country code at the origin, got \
             {country:?}"
        );
        assert_ne!(
            country, SPOOFED_COUNTRY,
            "case {case:?}: expected the country the EDGE resolved, not the one the viewer \
             stated, got {country:?}"
        );
        assert_eq!(
            proto, row.scheme,
            "case {case:?}: expected the origin to be told the scheme the viewer dialled \
             ({:?}), got {proto:?}",
            row.scheme
        );
        match &resolved_country {
            None => resolved_country = Some(country),
            Some(first) => assert_eq!(
                &country, first,
                "case {case:?}: expected the same viewer's country on every scheme ({first:?}), \
                 got {country:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The same rule at the ADDRESS a viewer is handed
// ---------------------------------------------------------------------------

/// How long a dial of the published address waits for its answer. Generous: nothing here
/// measures the wait, only what came back.
const DIAL_DEADLINE: Duration = Duration::from_secs(30);

/// Fetch `url` with a client of its own, over the scheme the URL names, carrying no
/// credential — a VIEWER handed the name a distribution published, speaking the protocol that
/// address answers on rather than going through the router.
///
/// The request runs on a blocking thread so the listener it addresses — a task of this same
/// runtime — is served while this call waits for the answer.
async fn dial(case: &str, url: String) -> (u16, String) {
    ::tokio::task::spawn_blocking(move || {
        let client = ::reqwest::blocking::Client::builder()
            .timeout(DIAL_DEADLINE)
            .build()
            .expect("a client");
        let answer = client.get(&url).send().expect("the published address answers");
        let status = answer.status().as_u16();
        (status, answer.text().unwrap_or_default())
    })
    .await
    .unwrap_or_else(|error| panic!("case {case:?}: the dial did not finish: {error}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_published_address_tells_the_origin_the_scheme_the_viewer_dialled() {
    let router = emulator();
    let api = deployed_reflecting_api(&router).await;
    let case = "the address the distribution published";
    let policy = create_origin_request_policy(
        &router,
        case,
        "published-address-generated-headers",
        "whitelist",
        Some(&[VIEWER_COUNTRY, FORWARDED_PROTO]),
    )
    .await;
    let (id, domain) = create_distribution(
        &router,
        case,
        "published-address-generated-headers",
        &api,
        &policy,
        CACHING_DISABLED,
    )
    .await;
    wait_until_deployed(&router, case, &id).await;
    assert!(
        domain
            .rsplit_once(':')
            .is_some_and(|(host, port)| host == "127.0.0.1" && port.parse::<u16>().is_ok()),
        "setup {case:?}: expected an address a client on this machine can dial, got {domain:?}"
    );

    // Dialled in the CLEAR, which is the half of the one published address a stock client
    // reaches without the certificate this emulator signed itself; the TLS half is what every
    // router-addressed row above drives. The rows differ along which behaviour the viewer path
    // matches.
    let rows: &[(&str, &str, bool)] = &[
        ("a viewer dialling the behaviour that names the generated headers", "/edge", true),
        ("a viewer dialling the behaviour that names no policy", "/bare", false),
    ];
    let scheme = "http";
    let mut resolved_country: Option<String> = None;
    for (case, path, generated) in rows.iter().copied() {
        let (status, body) = dial(case, format!("{scheme}://{domain}{path}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the published address to serve the origin's answer, got \
             {status} with {body:?}"
        );
        let document: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let read = |name: &str| {
            document
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let (country, proto) = (read("country"), read("proto"));
        if !generated {
            assert_eq!(
                (country.as_str(), proto.as_str()),
                ("", ""),
                "case {case:?}: a behaviour whose origin request policy names no generated \
                 header must send the origin neither, got the country {country:?} and the \
                 scheme {proto:?}"
            );
            continue;
        }
        assert!(
            is_alpha_2(&country),
            "case {case:?}: expected a two-letter uppercase country code at the origin, got \
             {country:?} (body {body:?})"
        );
        assert_eq!(
            proto, scheme,
            "case {case:?}: expected the origin to be told the scheme the viewer dialled \
             ({scheme:?}), got {proto:?} (body {body:?})"
        );
        match &resolved_country {
            None => resolved_country = Some(country),
            Some(first) => assert_eq!(
                &country, first,
                "case {case:?}: expected the same viewer's country on every dial ({first:?}), \
                 got {country:?}"
            ),
        }
    }
    assert!(
        resolved_country.is_some(),
        "the table must dial the published address at least once"
    );
}

// ---------------------------------------------------------------------------
// The same value, where the CACHE KEY names a generated header
// ---------------------------------------------------------------------------

/// One row of the cache-key table: one viewer request, and what the edge must answer it from.
struct KeyedRow {
    case: &'static str,
    /// The scheme the viewer dialled.
    scheme: &'static str,
    /// What the viewer itself stated for the generated headers.
    stated: &'static [(&'static str, &'static str)],
    /// The `X-Cache` the edge must answer with: the origin's answer, or the cache's.
    x_cache: &'static str,
}

#[tokio::test]
async fn the_cache_key_carries_the_edge_s_own_value_for_a_generated_header_it_names() {
    let router = emulator();
    let api = deployed_reflecting_api(&router).await;
    let forwards = create_origin_request_policy(
        &router,
        "the policy that lets the origin report both generated headers",
        "keyed-generated-headers-forwarded",
        "whitelist",
        Some(&[VIEWER_COUNTRY, FORWARDED_PROTO]),
    )
    .await;

    // Two keyed policies, one per generated header: the SCHEME, whose two values are two
    // objects, and the COUNTRY, whose one value for this viewer is one object however the
    // viewer spells the header itself.
    for (keyed_header, rows) in [
        (
            FORWARDED_PROTO,
            &[
                KeyedRow {
                    case: "the first TLS viewer, which the origin answers",
                    scheme: "https",
                    stated: &[],
                    x_cache: MISS,
                },
                KeyedRow {
                    case: "a second TLS viewer, which the key says is the same object",
                    scheme: "https",
                    stated: &[],
                    x_cache: HIT,
                },
                KeyedRow {
                    case: "a plaintext viewer, whose forwarded scheme is another object",
                    scheme: "http",
                    stated: &[],
                    x_cache: MISS,
                },
                KeyedRow {
                    case: "a second plaintext viewer, which is that other object again",
                    scheme: "http",
                    stated: &[],
                    x_cache: HIT,
                },
                KeyedRow {
                    case: "a TLS viewer stating a scheme of its own, which keys on nothing",
                    scheme: "https",
                    stated: &[(FORWARDED_PROTO, SPOOFED_PROTO)],
                    x_cache: HIT,
                },
            ][..],
        ),
        (
            VIEWER_COUNTRY,
            &[
                KeyedRow {
                    case: "the first viewer, which the origin answers",
                    scheme: "https",
                    stated: &[],
                    x_cache: MISS,
                },
                KeyedRow {
                    case: "a viewer stating a country of its own, which keys on nothing",
                    scheme: "https",
                    stated: &[(VIEWER_COUNTRY, SPOOFED_COUNTRY)],
                    x_cache: HIT,
                },
                KeyedRow {
                    case: "a viewer the edge resolves another country for, which is another \
                           object",
                    scheme: "https",
                    stated: &[(VIEWER_COUNTRY, "de")],
                    x_cache: MISS,
                },
                KeyedRow {
                    case: "that viewer again, which is that other object again",
                    scheme: "https",
                    stated: &[(VIEWER_COUNTRY, "DE")],
                    x_cache: HIT,
                },
            ][..],
        ),
    ] {
        let case = keyed_header;
        let keyed = create_cache_policy(
            &router,
            case,
            &format!("keyed-on-{keyed_header}"),
            keyed_header,
        )
        .await;
        let (id, domain) = create_distribution(
            &router,
            case,
            &format!("keyed-on-{keyed_header}"),
            &api,
            &forwards,
            &keyed,
        )
        .await;
        wait_until_deployed(&router, case, &id).await;

        for row in rows {
            let case = format!("{keyed_header} in the cache key: {}", row.case);
            let served = reflected(&router, &case, row.scheme, &domain, "/edge", row.stated).await;
            assert_eq!(
                served.x_cache, row.x_cache,
                "case {case:?}: expected X-Cache {:?}, got {:?} (the origin was told the \
                 country {:?} and the scheme {:?})",
                row.x_cache, served.x_cache, served.country, served.proto
            );
            // Whatever the key said, the answer the viewer gets is the one fetched for a
            // request whose generated values were the edge's own.
            assert!(
                is_alpha_2(&served.country),
                "case {case:?}: expected a two-letter uppercase country code at the origin, \
                 got {:?}",
                served.country
            );
            assert_ne!(
                served.country, SPOOFED_COUNTRY,
                "case {case:?}: expected the country the EDGE resolved, got {:?}",
                served.country
            );
        }
    }
}

/// One row of the policy table: the `HeaderBehavior` and the list a policy carries, and
/// WHICH of the two generated headers the origin request then carries.
struct PolicyRow {
    case: &'static str,
    /// A value of `OriginRequestPolicyHeaderBehavior`.
    header_behavior: &'static str,
    /// The policy's `Headers` list, or `None` for a policy carrying no wrapper at all.
    names: Option<&'static [&'static str]>,
    /// Whether the origin must be told the viewer's country, and the scheme it dialled.
    country: bool,
    proto: bool,
}

#[tokio::test]
async fn a_generated_header_reaches_the_origin_only_where_the_policy_names_it() {
    let router = emulator();
    let api = deployed_reflecting_api(&router).await;

    // The rows differ along the behaviour the policy's `HeadersConfig` carries — every value
    // of `OriginRequestPolicyHeaderBehavior`
    // (`smithy_query(query="shape",shape="OriginRequestPolicyHeaderBehavior")`: none,
    // whitelist, allViewer, allViewerAndWhitelistCloudFront, allExcept) — and along which of
    // the two generated headers its list names.
    let rows: &[PolicyRow] = &[
        PolicyRow {
            case: "a whitelist naming both generated headers",
            header_behavior: "whitelist",
            names: Some(&[VIEWER_COUNTRY, FORWARDED_PROTO]),
            country: true,
            proto: true,
        },
        PolicyRow {
            case: "a whitelist naming the country alone",
            header_behavior: "whitelist",
            names: Some(&[VIEWER_COUNTRY]),
            country: true,
            proto: false,
        },
        PolicyRow {
            case: "a whitelist naming the forwarded scheme alone",
            header_behavior: "whitelist",
            names: Some(&[FORWARDED_PROTO]),
            country: false,
            proto: true,
        },
        PolicyRow {
            case: "a whitelist naming a viewer header and neither generated one",
            header_behavior: "whitelist",
            names: Some(&["Accept-Language"]),
            country: false,
            proto: false,
        },
        PolicyRow {
            case: "allViewerAndWhitelistCloudFront naming both generated headers",
            header_behavior: "allViewerAndWhitelistCloudFront",
            names: Some(&[VIEWER_COUNTRY, FORWARDED_PROTO]),
            country: true,
            proto: true,
        },
        PolicyRow {
            case: "allViewerAndWhitelistCloudFront naming the forwarded scheme alone",
            header_behavior: "allViewerAndWhitelistCloudFront",
            names: Some(&[FORWARDED_PROTO]),
            country: false,
            proto: true,
        },
        PolicyRow {
            case: "allViewer, which names every header the VIEWER sent and no generated one",
            header_behavior: "allViewer",
            names: None,
            country: false,
            proto: false,
        },
        PolicyRow {
            case: "allExcept, which names no generated header either",
            header_behavior: "allExcept",
            names: Some(&["X-Internal"]),
            country: false,
            proto: false,
        },
        PolicyRow {
            case: "none, which forwards no header at all",
            header_behavior: "none",
            names: None,
            country: false,
            proto: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let policy = create_origin_request_policy(
            &router,
            case,
            &format!("generated-headers-{index}"),
            row.header_behavior,
            row.names,
        )
        .await;
        let (id, domain) = create_distribution(
            &router,
            case,
            &format!("generated-headers-policy-{index}"),
            &api,
            &policy,
            CACHING_DISABLED,
        )
        .await;
        wait_until_deployed(&router, case, &id).await;

        let Served { country, proto, .. } =
            reflected(&router, case, "https", &domain, "/edge", &[]).await;
        assert_eq!(
            (is_alpha_2(&country), proto == "https"),
            (row.country, row.proto),
            "case {case:?}: expected the origin to be told (country, scheme) == ({}, {}), got \
             the country {country:?} and the scheme {proto:?}",
            row.country,
            row.proto
        );
        if !row.country {
            assert_eq!(
                country, "",
                "case {case:?}: a policy that does not name the country must send the origin \
                 none, got {country:?}"
            );
        }
        if !row.proto {
            assert_eq!(
                proto, "",
                "case {case:?}: a policy that does not name the forwarded scheme must send the \
                 origin none, got {proto:?}"
            );
        }
    }
}
