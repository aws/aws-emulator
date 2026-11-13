// SPDX-License-Identifier: CC0-1.0

//! A CloudFront distribution over an API GATEWAY REST STAGE as a custom origin, end to end
//! over HTTP across the two services on one engine.
//!
//! Three rules, each a property of the matched cache behaviour and of the origin it names,
//! and each observable only where both services are mounted together — the behaviour and the
//! origin are CloudFront's records, and the thing that answers the origin fetch is API
//! Gateway's deployed stage:
//!
//! 1. the behaviour's `ViewerProtocolPolicy` decides the SCHEME a viewer may reach it over:
//!    `allow-all` means "viewers can use HTTP or HTTPS", `redirect-to-https` means "if a
//!    viewer submits an HTTP request, CloudFront returns an HTTP status code of 301 (Moved
//!    Permanently) to the viewer along with the HTTPS URL", and `https-only` means "if a
//!    viewer sends an HTTP request, CloudFront returns an HTTP status code of 403
//!    (Forbidden)". A viewer that dialled HTTPS is served under every one of them.
//! 2. a behaviour that names NO cache policy keys on, and forwards to the origin, exactly
//!    what its own legacy `ForwardedValues` names — "a `CacheBehavior` must include either a
//!    `CachePolicyId` or `ForwardedValues`" — so the query string travels when `QueryString`
//!    is true and the headers its `Headers` list names travel, CloudFront's own generated
//!    headers among them, while anything it does not name is withheld.
//! 3. the origin is fetched over the scheme its `OriginProtocolPolicy` names: `https-only`
//!    relays the origin's own answer to the viewer, and `http-only` against an endpoint that
//!    is served over TLS alone — which every AWS service endpoint of this emulator is — is an
//!    origin connection CloudFront cannot complete, so the viewer is answered `502` and none
//!    of the origin's body.
//!
//! The origin is a stage whose one method is a `MOCK` integration that REFLECTS what reached
//! it (`$input.params(...)` over the method request), so what the origin received is read off
//! what the viewer is served.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::PROPAGATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every setup call is signed for, and the Region the origin host names.
pub(crate) const REGION: &str = "us-east-1";

/// CloudFront's control-plane host, and the API version every route is under.
pub(crate) const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
pub(crate) const API_VERSION: &str = "2020-05-31";

/// The stage the api is deployed to, which is also the origin's `OriginPath`.
pub(crate) const STAGE: &str = "live";

/// The resource path of the api's one method.
const ECHO_PATH: &str = "echo";

/// The viewer header the forwarding rows whitelist, and the value a viewer sends it with.
const CUSTOM_HEADER: &str = "X-Custom-Header";
const CUSTOM_VALUE: &str = "header-alpha";

/// One header CloudFront GENERATES itself, which a legacy `Headers` list may name
/// (`CloudFront-Viewer-Country`), and the country this emulator's viewers are in.
const GENERATED_HEADER: &str = "CloudFront-Viewer-Country";
const GENERATED_VALUE: &str = "US";

/// The query parameter a viewer sends, and its value.
const QUERY_NAME: &str = "q";
const QUERY_VALUE: &str = "alpha";

/// How long to wait between polls of the clock-owed deployment, and how many polls to make.
pub(crate) const POLL: Duration = Duration::from_millis(50);
pub(crate) const POLLS: usize = 400;

/// What one call answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: ::http::HeaderMap,
    pub(crate) body: Bytes,
}

impl Answer {
    /// A response header's value, empty when the answer carries none.
    pub(crate) fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The body as text.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is neither JSON nor present.
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The text of `<name>…</name>` in the body, or the empty string.
    pub(crate) fn element(&self, name: &str) -> String {
        let body = self.text();
        body.split_once(&format!("<{name}>"))
            .and_then(|(_before, rest)| rest.split_once(&format!("</{name}>")))
            .map_or(String::new(), |(text, _after)| text.to_string())
    }
}

/// The two services this suite exercises, mounted on one engine. Request authentication is
/// off: a setup call carries a credential scope (which is what names the Region and routes
/// it) without a computed signature, and a VIEWER request carries no credential at all, as a
/// client of a distribution sends none.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// a setup call. The signature itself is never checked here.
pub(crate) fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire, addressed at `uri` with `host`.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
pub(crate) async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &host,
        &format!("https://{host}{path}"),
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
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

/// One CloudFront control-plane call.
pub(crate) async fn cloudfront(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Bytes,
) -> Answer {
    dispatch(
        router,
        method,
        CLOUDFRONT_HOST,
        &format!("https://{CLOUDFRONT_HOST}{path}"),
        &[
            ("content-type", "application/xml"),
            ("authorization", &authorization("cloudfront")),
        ],
        body,
    )
    .await
}

/// One VIEWER request to a distribution: addressed to the domain name the distribution
/// publishes, over `scheme`, carrying no credential of any kind.
pub(crate) async fn view(
    router: &MultiServiceRouter,
    scheme: &str,
    domain: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Answer {
    dispatch(
        router,
        "GET",
        domain,
        &format!("{scheme}://{domain}{path}"),
        headers,
        Bytes::new(),
    )
    .await
}

/// The member `name` of a JSON object, as text.
pub(crate) fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// A REST api with one `GET /echo` method whose `MOCK` integration REFLECTS what reached
/// it, deployed to [`STAGE`]. Answers the api's id.
///
/// The response template reads the METHOD REQUEST's own parameters, so the document the
/// stage answers carries exactly the query string and headers the origin request arrived
/// with.
async fn deployed_reflecting_api(router: &MultiServiceRouter) -> String {
    let created = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "cloudfront-origin-echo" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    let resource = apigw(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": ECHO_PATH })),
        201,
    )
    .await;
    let echo = member("CreateResource", &resource, "id");
    apigw(
        router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{echo}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{echo}/methods/GET/integration"),
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
        &format!("/restapis/{api}/resources/{echo}/methods/GET/responses/200"),
        Some(json!({})),
        201,
    )
    .await;
    let template = format!(
        r#"{{"query":"$input.params('{QUERY_NAME}')","header":"$input.params('{CUSTOM_HEADER}')","cfHeader":"$input.params('{GENERATED_HEADER}')"}}"#
    );
    apigw(
        router,
        "PutIntegrationResponse",
        "PUT",
        &format!("/restapis/{api}/resources/{echo}/methods/GET/integration/responses/200"),
        Some(json!({
            "statusCode": "200",
            "responseTemplates": { "application/json": template },
        })),
        201,
    )
    .await;
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

/// The `ForwardedValues` document a row asks for, in the model's member declaration order
/// (`smithy_query(query="shape",shape="ForwardedValues")`): `QueryString`, `Cookies`,
/// `Headers` and `QueryStringCacheKeys`, the last two left out when the row names none.
fn forwarded_values(query_string: bool, headers: &[&str], cache_keys: &[&str]) -> String {
    let mut out = String::from("<ForwardedValues>");
    out.push_str(&format!("<QueryString>{query_string}</QueryString>"));
    out.push_str("<Cookies><Forward>none</Forward></Cookies>");
    if !headers.is_empty() {
        let items: String = headers.iter().map(|name| format!("<Name>{name}</Name>")).collect();
        out.push_str(&format!(
            "<Headers><Quantity>{}</Quantity><Items>{items}</Items></Headers>",
            headers.len()
        ));
    }
    if !cache_keys.is_empty() {
        let items: String = cache_keys
            .iter()
            .map(|name| format!("<Name>{name}</Name>"))
            .collect();
        out.push_str(&format!(
            "<QueryStringCacheKeys><Quantity>{}</Quantity><Items>{items}</Items>\
             </QueryStringCacheKeys>",
            cache_keys.len()
        ));
    }
    out.push_str("</ForwardedValues>");
    out
}

/// `cloudfront:CreateDistribution` over the api stage `api` as a custom origin, answering the
/// distribution's `(id, DomainName)`.
///
/// The behaviour names no cache policy: its TTLs are the legacy ones, with `MinTTL` zero so
/// every viewer request reaches the origin, and what it forwards is the legacy
/// `ForwardedValues` the row asked for.
async fn create_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    api: &str,
    origin_protocol_policy: &str,
    viewer_protocol_policy: &str,
    forwarded: &str,
) -> (String, String) {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>api-origin</Id>\
         <DomainName>{api}.execute-api.{REGION}.amazonaws.com</DomainName>\
         <OriginPath>/{STAGE}</OriginPath>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>{origin_protocol_policy}</OriginProtocolPolicy>\
         </CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>api-origin</TargetOriginId>\
         <ViewerProtocolPolicy>{viewer_protocol_policy}</ViewerProtocolPolicy>\
         {forwarded}<MinTTL>0</MinTTL>\
         </DefaultCacheBehavior>\
         <Comment>an api gateway stage as a custom origin</Comment>\
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
        "case {case:?}: CreateDistribution must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let id = created.element("Id");
    let domain = created.element("DomainName");
    assert!(
        !id.is_empty() && !domain.is_empty(),
        "case {case:?}: the created distribution reports no Id or DomainName: {:?}",
        created.text()
    );
    (id, domain)
}

/// Poll `GetDistribution` until CloudFront first reports the distribution `Deployed`, which
/// is the state a viewer request is served in. The wait is on the CONDITION, with a deadline
/// far past the span the service owes it.
pub(crate) async fn wait_until_deployed(router: &MultiServiceRouter, case: &str, id: &str) {
    for _ in 0..POLLS {
        let read = cloudfront(
            router,
            "GET",
            &format!("/{API_VERSION}/distribution/{id}"),
            Bytes::new(),
        )
        .await;
        if read.element("Status") == "Deployed" {
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
    panic!(
        "case {case:?}: CloudFront never reported the distribution Deployed within {:?}",
        POLL * u32::try_from(POLLS).unwrap_or(u32::MAX) + PROPAGATION_DELAY
    );
}

/// One row of the viewer-protocol table: the policy the behaviour names, the scheme the
/// viewer dials, and what CloudFront answers.
struct ProtocolRow {
    case: &'static str,
    policy: &'static str,
    scheme: &'static str,
    status: u16,
    /// Whether the answer has to carry the origin's own reflected document.
    from_origin: bool,
    /// Whether the answer has to carry the HTTPS URL of the same request in `Location`.
    redirected: bool,
}

#[tokio::test]
async fn the_viewer_protocol_policy_decides_which_scheme_a_distribution_serves() {
    let router = emulator();
    let api = deployed_reflecting_api(&router).await;

    // Rows differ along the two things the rule generalises over: the enum's own values
    // (`smithy_query(query="shape",shape="ViewerProtocolPolicy")` — allow-all,
    // redirect-to-https, https-only) and the scheme the viewer dialled.
    let rows: &[ProtocolRow] = &[
        ProtocolRow {
            case: "allow-all over plaintext",
            policy: "allow-all",
            scheme: "http",
            status: 200,
            from_origin: true,
            redirected: false,
        },
        ProtocolRow {
            case: "allow-all over TLS",
            policy: "allow-all",
            scheme: "https",
            status: 200,
            from_origin: true,
            redirected: false,
        },
        ProtocolRow {
            case: "redirect-to-https over plaintext",
            policy: "redirect-to-https",
            scheme: "http",
            status: 301,
            from_origin: false,
            redirected: true,
        },
        ProtocolRow {
            case: "redirect-to-https over TLS",
            policy: "redirect-to-https",
            scheme: "https",
            status: 200,
            from_origin: true,
            redirected: false,
        },
        ProtocolRow {
            case: "https-only over plaintext",
            policy: "https-only",
            scheme: "http",
            status: 403,
            from_origin: false,
            redirected: false,
        },
        ProtocolRow {
            case: "https-only over TLS",
            policy: "https-only",
            scheme: "https",
            status: 200,
            from_origin: true,
            redirected: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (id, domain) = create_distribution(
            &router,
            case,
            &format!("viewer-protocol-{index}"),
            &api,
            "https-only",
            row.policy,
            &forwarded_values(true, &[CUSTOM_HEADER], &[]),
        )
        .await;
        wait_until_deployed(&router, case, &id).await;

        let path = format!("/{ECHO_PATH}?{QUERY_NAME}={QUERY_VALUE}");
        let answer = view(
            &router,
            row.scheme,
            &domain,
            &path,
            &[(CUSTOM_HEADER, CUSTOM_VALUE)],
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with {:?}",
            row.status,
            answer.status,
            answer.text()
        );
        if row.redirected {
            assert_eq!(
                answer.header("location"),
                format!("https://{domain}{path}"),
                "case {case:?}: expected the HTTPS URL of the same request, got {:?}",
                answer.header("location")
            );
        } else {
            assert!(
                answer.header("location").is_empty(),
                "case {case:?}: expected no redirect, got Location {:?}",
                answer.header("location")
            );
        }
        if row.from_origin {
            assert_eq!(
                answer.json().get("query").and_then(Value::as_str),
                Some(QUERY_VALUE),
                "case {case:?}: expected the origin's own reflected answer, got {:?}",
                answer.text()
            );
        } else {
            assert!(
                !answer.text().contains(QUERY_VALUE),
                "case {case:?}: an answer CloudFront produced itself must carry nothing of \
                 the origin, got {:?}",
                answer.text()
            );
        }
    }
}

/// One row of the forwarding table: the legacy `ForwardedValues` the behaviour carries, and
/// what the origin is then asked for.
struct ForwardingRow {
    case: &'static str,
    query_string: bool,
    headers: &'static [&'static str],
    cache_keys: &'static [&'static str],
    /// What the origin reflected for the query parameter, the viewer header and the header
    /// CloudFront generated.
    query: &'static str,
    header: &'static str,
    generated: &'static str,
}

#[tokio::test]
async fn a_behaviour_with_no_cache_policy_forwards_what_its_forwarded_values_name() {
    let router = emulator();
    let api = deployed_reflecting_api(&router).await;

    // Rows differ along every part of the member the rule reads: `QueryString` true and
    // false, a `Headers` list naming a viewer header, one naming a header CloudFront
    // generates itself, one naming neither, and `QueryStringCacheKeys` naming the parameter
    // (which narrows the KEY and still forwards every parameter).
    let rows: &[ForwardingRow] = &[
        ForwardingRow {
            case: "the query string and a whitelisted viewer header",
            query_string: true,
            headers: &[CUSTOM_HEADER],
            cache_keys: &[],
            query: QUERY_VALUE,
            header: CUSTOM_VALUE,
            generated: "",
        },
        ForwardingRow {
            case: "no query string and no headers",
            query_string: false,
            headers: &[],
            cache_keys: &[],
            query: "",
            header: "",
            generated: "",
        },
        ForwardingRow {
            case: "the query string alone, with no header whitelisted",
            query_string: true,
            headers: &[],
            cache_keys: &[],
            query: QUERY_VALUE,
            header: "",
            generated: "",
        },
        ForwardingRow {
            case: "named query-string cache keys, which still forward every parameter",
            query_string: true,
            headers: &[CUSTOM_HEADER],
            cache_keys: &[QUERY_NAME],
            query: QUERY_VALUE,
            header: CUSTOM_VALUE,
            generated: "",
        },
        ForwardingRow {
            case: "a whitelist naming a header CloudFront generates itself",
            query_string: true,
            headers: &[CUSTOM_HEADER, GENERATED_HEADER],
            cache_keys: &[],
            query: QUERY_VALUE,
            header: CUSTOM_VALUE,
            generated: GENERATED_VALUE,
        },
        ForwardingRow {
            case: "no query string with a whitelisted header",
            query_string: false,
            headers: &[CUSTOM_HEADER],
            cache_keys: &[],
            query: "",
            header: CUSTOM_VALUE,
            generated: "",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (id, domain) = create_distribution(
            &router,
            case,
            &format!("forwarded-values-{index}"),
            &api,
            "https-only",
            "allow-all",
            &forwarded_values(row.query_string, row.headers, row.cache_keys),
        )
        .await;
        wait_until_deployed(&router, case, &id).await;

        let answer = view(
            &router,
            "https",
            &domain,
            &format!("/{ECHO_PATH}?{QUERY_NAME}={QUERY_VALUE}"),
            &[(CUSTOM_HEADER, CUSTOM_VALUE)],
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the origin's answer through the edge, got {} with {:?}",
            answer.status,
            answer.text()
        );
        let reflected = answer.json();
        let read = |name: &str| {
            reflected
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(
            (
                read("query").as_str(),
                read("header").as_str(),
                read("cfHeader").as_str()
            ),
            (row.query, row.header, row.generated),
            "case {case:?}: expected the origin to be asked for ({:?}, {:?}, {:?}), got {:?}",
            row.query,
            row.header,
            row.generated,
            answer.text()
        );
    }
}

/// One row of the origin-protocol table: the policy the origin carries, the scheme the viewer
/// dials, and what the viewer is answered.
struct OriginRow {
    case: &'static str,
    policy: &'static str,
    scheme: &'static str,
    status: u16,
    from_origin: bool,
}

#[tokio::test]
async fn the_origin_protocol_policy_decides_whether_the_origin_can_be_reached() {
    let router = emulator();
    let api = deployed_reflecting_api(&router).await;

    // Rows differ along the enum's own values
    // (`smithy_query(query="shape",shape="OriginProtocolPolicy")` — http-only, match-viewer,
    // https-only) and the scheme the viewer dialled, which is what `match-viewer` reads.
    let rows: &[OriginRow] = &[
        OriginRow {
            case: "an https-only origin, which the stage answers",
            policy: "https-only",
            scheme: "https",
            status: 200,
            from_origin: true,
        },
        OriginRow {
            case: "an https-only origin reached by a plaintext viewer",
            policy: "https-only",
            scheme: "http",
            status: 200,
            from_origin: true,
        },
        OriginRow {
            case: "an http-only origin against an endpoint served over TLS alone",
            policy: "http-only",
            scheme: "https",
            status: 502,
            from_origin: false,
        },
        OriginRow {
            case: "a match-viewer origin for a viewer that dialled TLS",
            policy: "match-viewer",
            scheme: "https",
            status: 200,
            from_origin: true,
        },
        OriginRow {
            case: "a match-viewer origin for a viewer that dialled plaintext",
            policy: "match-viewer",
            scheme: "http",
            status: 502,
            from_origin: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (id, domain) = create_distribution(
            &router,
            case,
            &format!("origin-protocol-{index}"),
            &api,
            row.policy,
            "allow-all",
            &forwarded_values(true, &[CUSTOM_HEADER], &[]),
        )
        .await;
        wait_until_deployed(&router, case, &id).await;

        let answer = view(
            &router,
            row.scheme,
            &domain,
            &format!("/{ECHO_PATH}?{QUERY_NAME}={QUERY_VALUE}"),
            &[(CUSTOM_HEADER, CUSTOM_VALUE)],
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with {:?}",
            row.status,
            answer.status,
            answer.text()
        );
        if row.from_origin {
            assert_eq!(
                answer.json().get("query").and_then(Value::as_str),
                Some(QUERY_VALUE),
                "case {case:?}: expected the origin's own reflected answer, got {:?}",
                answer.text()
            );
        } else {
            assert!(
                !answer.text().contains(QUERY_VALUE),
                "case {case:?}: an origin CloudFront could not reach must leak none of its \
                 body, got {:?}",
                answer.text()
            );
            assert_eq!(
                answer.header("x-cache"),
                "Error from cloudfront",
                "case {case:?}: expected the answer to be reported as CloudFront's own, got \
                 X-Cache {:?}",
                answer.header("x-cache")
            );
        }
    }
}
