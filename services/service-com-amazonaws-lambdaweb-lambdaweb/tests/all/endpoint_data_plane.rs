// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS an endpoint publishes, and what this emulator answers there.
//!
//! RULE: `domainName` is "the domain name assigned to the endpoint" — the address the
//! endpoint is SERVED at, which a client reads off the endpoint and issues its HTTP
//! request to — so it is an address this emulator answers on, and the endpoint's own
//! requests are served there: a request for an endpoint that is not serving traffic is a
//! `404`, an `IamAuth` endpoint ("requests ... must be signed with Signature Version 4")
//! refuses a request presenting no SigV4 credential with the `403 Forbidden` PAGE — which
//! names the response it is, in the `lambda-web-request-id` header the page's `Request ID:`
//! line states — and an `ApplicationManaged` endpoint ("your application handles
//! authorization") refuses nothing of its own.
//!
//! RULE, for an AUTHORIZED request: what the client is served is the response THE
//! REVISION'S OWN CODE produced. "Your code listens on a network port and handles requests
//! directly" (`LambdaWeb`), so the revision the endpoint routes the request to is run, its
//! deployment package's server is handed the request, and the status, headers and body that
//! server answered are what comes back. A revision whose code answered nothing is reported
//! as that and never as a response nothing produced.
//!
//! The published address is read off the wire (`CreateWebFunction` and
//! `GetWebFunctionEndpoint`), the deployment packages are staged through Amazon S3's own
//! operations (so the code that runs is the code a client uploaded), and the decision the
//! core's listener hands every request at that address is driven both directly
//! (`state::endpoint_plane::serve_request`) and over a real socket. The rows span the
//! `authType` values, how a client may present a credential, the endpoint's lifecycle
//! state, and the module form a package ships its server in.

use serde_json::{json, Value};

use service_com_amazonaws_lambdaweb_lambdaweb::state::endpoint_plane::serve_request;
use service_com_amazonaws_lambdaweb_lambdaweb::state::service_state;

use super::revision_build_outcome::{archive, create_bucket, stage_object, BUCKET};
use super::web_function_endpoint::poll_until_active;
use super::web_function_lifecycle::{build_config, role_arn, Emulator};

/// The region every request here signs with.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header, as a signed client sends one.
const SIGNED: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/lambda/\
                      aws4_request, SignedHeaders=host, Signature=deadbeef";

/// The response header every answer a published address names the response it is by.
pub(crate) const REQUEST_ID_HEADER: &str = "lambda-web-request-id";

/// The key the CommonJS application below is staged under.
const CJS_KEY: &str = "data-plane-commonjs.zip";

/// The key the ES module application below is staged under.
const ESM_KEY: &str = "data-plane-esm.zip";

/// The key an application that registers no server is staged under.
const NO_SERVER_KEY: &str = "data-plane-no-server.zip";

/// The key the application that answers FROM the request it was handed is staged under.
const REQUEST_ECHO_KEY: &str = "data-plane-request-echo.zip";

/// A deployment package whose code is a Node.js HTTP server, written as a CommonJS module —
/// the form a web function's package ships its server in. It answers from the request it
/// was handed and from its own environment, so what a client reads back could not have been
/// assembled anywhere but inside that code.
fn commonjs_application() -> Vec<u8> {
    archive(&[(
        "index.js",
        br#"
const http = require('http');
http.createServer((req, res) => {
  if (req.url.indexOf('/items/') === 0) {
    res.writeHead(200, { 'content-type': 'application/json', 'x-served-by': 'commonjs' });
    res.end(JSON.stringify({ item: req.url.slice('/items/'.length), method: req.method }));
    return;
  }
  if (req.url === '/teapot') {
    res.writeHead(418, { 'content-type': 'text/plain', 'x-served-by': 'commonjs' });
    res.end('short and stout');
    return;
  }
  res.writeHead(200, { 'content-type': 'text/plain', 'x-served-by': 'commonjs' });
  res.end('hello from ' + (process.env.GREETING || 'the revision'));
}).listen(Number(process.env.PORT) || 8080);
"#,
    )])
}

/// The same server shipped as an ES module across two files, which is the other module form
/// a Node.js package takes.
fn es_module_application() -> Vec<u8> {
    archive(&[
        ("package.json", b"{\"main\": \"src/server.mjs\"}"),
        (
            "src/server.mjs",
            br#"
import http from 'node:http';
import { greeting } from './greeting.mjs';
http.createServer((req, res) => {
  res.writeHead(201, { 'content-type': 'text/plain', 'x-served-by': 'esm' });
  res.end(greeting());
}).listen(8080);
"#,
        ),
        (
            "src/greeting.mjs",
            b"export const greeting = () => 'hello from an es module';",
        ),
    ])
}

/// A package whose code runs and registers no server at all, which answers no request.
fn application_without_a_server() -> Vec<u8> {
    archive(&[("index.js", b"const held = 1 + 1;")])
}

/// A deployment package whose server computes EVERY part of its answer from the request it
/// was handed — the method, the request target, a request header and the request body — and
/// chooses its own status and content type per route.
///
/// Nothing it answers exists anywhere in this emulator: a client that reads one of these
/// bodies back read what the revision's code produced.
fn request_shaped_application() -> Vec<u8> {
    archive(&[(
        "index.js",
        br#"
const http = require('http');
http.createServer((req, res) => {
  let body = '';
  req.on('data', (chunk) => { body += chunk; });
  req.on('end', () => {
    if (req.url.indexOf('/echo') === 0) {
      res.writeHead(200, { 'content-type': 'application/json', 'x-served-by': 'request-echo' });
      res.end(JSON.stringify({
        method: req.method,
        url: req.url,
        note: req.headers['x-note'] || '',
        body: body,
      }));
      return;
    }
    if (req.url === '/gone') {
      res.writeHead(410, { 'content-type': 'text/plain', 'x-served-by': 'request-echo' });
      res.end('the application decided this is gone');
      return;
    }
    res.writeHead(200, { 'content-type': 'text/plain', 'x-served-by': 'request-echo' });
    res.end('the application answered ' + req.method + ' ' + req.url);
  });
}).listen(Number(process.env.PORT) || 8080);
"#,
    )])
}

/// Stage every package these suites run, through Amazon S3's own operations.
async fn stage_packages(emulator: &Emulator) {
    create_bucket(emulator).await;
    stage_object(emulator, CJS_KEY, commonjs_application()).await;
    stage_object(emulator, ESM_KEY, es_module_application()).await;
    stage_object(emulator, NO_SERVER_KEY, application_without_a_server()).await;
    stage_object(emulator, REQUEST_ECHO_KEY, request_shaped_application()).await;
}

/// The `403 Forbidden` page an `IamAuth` endpoint refuses a request presenting no SigV4
/// credential with, naming the response that served it.
///
/// Written out here independently of the document the service builds — one entry per LINE
/// of the page, joined by the newlines that separate them, so the indentation is part of
/// the line and nothing is lost to a source continuation — so a drift in a tag, a sentence
/// or a line break is caught.
pub(crate) fn forbidden_page(request_id: &str) -> String {
    [
        "<!DOCTYPE html>",
        "<html>",
        "<head>",
        "    <title>HTTP Error 403</title>",
        "</head>",
        "<body>",
        "    <h1>403 Forbidden</h1>",
        "    <p>You are not authorized to access this endpoint.</p>",
        &format!("    <p>Request ID: {request_id}</p>"),
        "</body>",
        "</html>",
    ]
    .join("\n")
}

/// Create a function with one endpoint of `auth_type`, serving the package staged under
/// `key`, and report the `domainName` the create published for it.
async fn create_serving(
    emulator: &Emulator,
    name: &str,
    auth_type: &str,
    key: &str,
    environment: Value,
) -> String {
    let created = emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": name,
                "revisionConfig": {
                    "buildConfig": build_config("nodejs24.x", BUCKET, key, None),
                    "serviceConfig": {
                        "executionRoleArn": role_arn(name),
                        "environmentVariables": environment,
                    },
                },
                "endpointConfig": {
                    "endpointName": "ep",
                    "endpointType": "HomeRegion",
                    "authType": auth_type,
                },
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "CreateWebFunction {name}: expected 202, got {} {:?}",
        created.status, created.body
    );
    created
        .field("endpoint")
        .and_then(|endpoint| endpoint.get("domainName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// [`create_serving`] for the CommonJS application with no environment variables of its
/// own.
async fn create_with_endpoint(emulator: &Emulator, name: &str, auth_type: &str) -> String {
    create_serving(emulator, name, auth_type, CJS_KEY, json!({})).await
}

/// One request to a published endpoint, answered by the decision the core's listener
/// serves at that address — read out of the ONE store the emulator's services share.
async fn request_endpoint(
    emulator: &Emulator,
    function_name: &str,
    endpoint_name: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> ::http::Response<Vec<u8>> {
    requested(emulator, function_name, endpoint_name, "GET", uri, headers).await
}

/// [`request_endpoint`] for a request of any method, which an unsigned request may be.
async fn requested(
    emulator: &Emulator,
    function_name: &str,
    endpoint_name: &str,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> ::http::Response<Vec<u8>> {
    let mut builder = ::http::Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Vec::new()).expect("a well-formed request");
    let state = service_state(&**emulator.engine()).expect("this service's store");
    serve_request(&state, function_name, endpoint_name, &request).await
}

/// One row of the request table: the case, the request URI, the headers it carries, and
/// the status the published address answers it with.
type RequestRow<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], u16);

/// One row of the unsigned-request table: the case, the request's method, its URI and the
/// headers it carries.
type UnsignedRow<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, &'a str)]);

/// The body of an answer from a published address, as text.
fn body_text(response: &::http::Response<Vec<u8>>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

/// The value of `name` on an answer from a published address, empty when it carries none.
fn header_of(response: &::http::Response<Vec<u8>>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// RULE: the published `domainName` is an address this emulator answers on — a `host:port`
/// whose host is the loopback interface the emulator's listeners bind — and it is the same
/// value every reader of the endpoint reports. Two endpoints never publish one address.
#[tokio::test]
async fn a_published_domain_name_is_an_address_the_emulator_answers_on() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    // The rows span the `authType` values, since the address is published for both.
    let mut published = Vec::new();
    for (case, auth_type) in [
        ("iam-authorized", "IamAuth"),
        ("application-managed", "ApplicationManaged"),
    ] {
        let name = format!("published-{case}");
        let domain_name = create_with_endpoint(&emulator, &name, auth_type).await;
        let host = service_common::data_plane_addressing::host_only(&domain_name);
        let port = service_common::data_plane_addressing::addressed_port(Some(&domain_name));
        assert!(
            emulator_server::is_loopback_host(host) && port.is_some_and(|port| port > 0),
            "case {case:?}: expected an address this emulator answers on, got {domain_name:?}"
        );
        assert!(
            !service_common::data_plane_addressing::names_a_service(&domain_name),
            "case {case:?}: a published address must not be a host that names an AWS \
             service, got {domain_name:?}"
        );

        // Every reader of the endpoint reports the one stored address.
        let read = emulator
            .send(
                "GET",
                &format!("/web-functions/{name}/endpoints/ep"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetWebFunctionEndpoint expected 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("domainName").as_deref(),
            Some(domain_name.as_str()),
            "case {case:?}: the read reports another address, got {:?}",
            read.body
        );
        published.push(domain_name);
    }
    assert_ne!(
        published[0], published[1],
        "two endpoints must not publish one address, got {published:?}"
    );
}

/// RULE: at the published address, an `IamAuth` endpoint refuses a request presenting no
/// SigV4 credential with the `403 Forbidden` page, and serves one that presents a
/// credential; an `ApplicationManaged` endpoint refuses nothing of its own.
///
/// What an AUTHORIZED request is answered with is the revision's own handler response, so
/// every row that is not refused is the `200` the staged package's server produced.
#[tokio::test]
async fn the_published_address_applies_the_endpoints_auth_type() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    for (case, auth_type) in [
        ("iam-authorized", "IamAuth"),
        ("application-managed", "ApplicationManaged"),
    ] {
        let name = format!("auth-{case}");
        create_with_endpoint(&emulator, &name, auth_type).await;
        // Until the endpoint is serving traffic, nothing is published at its address.
        let pending =
            request_endpoint(&emulator, &name, "ep", "/", &[("authorization", SIGNED)]).await;
        assert_eq!(
            pending.status().as_u16(),
            404,
            "case {case:?}: an endpoint that is not serving traffic publishes nothing, got \
             {}",
            body_text(&pending)
        );

        // An endpoint serves traffic once it is `Active`, which the clock owes it: the wait
        // is for that CONDITION, through the one poll this crate's tests share.
        poll_until_active(&emulator, &name, "ep").await;

        // (case, the request URI, its headers, the status the address answers)
        let unsigned_status = if auth_type == "IamAuth" { 403 } else { 200 };
        let rows: &[RequestRow<'_>] = &[
            ("a signed GET", "/", &[("authorization", SIGNED)], 200),
            (
                "a presigned GET",
                "/?X-Amz-Credential=AKIDEXAMPLE&X-Amz-Signature=deadbeef",
                &[],
                200,
            ),
            ("an unsigned GET", "/", &[], unsigned_status),
            (
                "an unsigned GET deeper in the path",
                "/items/42?page=2",
                &[],
                unsigned_status,
            ),
            (
                "another authorization scheme",
                "/",
                &[("authorization", "Basic dXNlcjpwYXNz")],
                unsigned_status,
            ),
        ];
        for (request_case, uri, headers, expected) in rows {
            let answered = request_endpoint(&emulator, &name, "ep", uri, headers).await;
            assert_eq!(
                answered.status().as_u16(),
                *expected,
                "case {case:?}/{request_case:?}: expected {expected}, got {} with {:?}",
                answered.status(),
                body_text(&answered)
            );
            if *expected == 403 {
                let request_id = header_of(&answered, REQUEST_ID_HEADER);
                assert!(
                    !request_id.is_empty(),
                    "case {case:?}/{request_case:?}: a 403 must name the response it is in \
                     its {REQUEST_ID_HEADER} header"
                );
                assert_eq!(
                    body_text(&answered),
                    forbidden_page(&request_id),
                    "case {case:?}/{request_case:?}: expected the Forbidden page naming \
                     {request_id:?}"
                );
            } else {
                assert_eq!(
                    header_of(&answered, "x-served-by"),
                    "commonjs",
                    "case {case:?}/{request_case:?}: expected the revision's own response, \
                     got {:?}",
                    body_text(&answered)
                );
            }
        }
    }
}

/// RULE: an authorized request to a published address is answered by the revision's OWN
/// code — the status, the headers and the body that code produced, computed from the
/// request it was handed and from the revision's own environment.
///
/// The rows span what the one staged application answers: a route it answers as text, a
/// route it answers as JSON built from the path and the method it was handed, and a route
/// it answers with a status of its own. Nothing here could be assembled by this emulator:
/// each value is produced inside the package's code.
#[tokio::test]
async fn an_authorized_request_is_answered_by_the_revisions_own_code() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let name = "handler-response";
    create_serving(
        &emulator,
        name,
        "IamAuth",
        CJS_KEY,
        json!({ "GREETING": "the staged package" }),
    )
    .await;
    poll_until_active(&emulator, name, "ep").await;

    // (case, the method, the request URI, the status, the content type, the body)
    let rows: [(&str, &str, &str, u16, &str, &str); 5] = [
        (
            "the root route, answered from the revision's environment",
            "GET",
            "/",
            200,
            "text/plain",
            "hello from the staged package",
        ),
        (
            "a route the code answers as JSON built from the path",
            "GET",
            "/items/42",
            200,
            "application/json",
            "{\"item\":\"42\",\"method\":\"GET\"}",
        ),
        (
            "the same route under another method",
            "POST",
            "/items/42",
            200,
            "application/json",
            "{\"item\":\"42\",\"method\":\"POST\"}",
        ),
        (
            "a deeper path the same route serves",
            "GET",
            "/items/a/b",
            200,
            "application/json",
            "{\"item\":\"a/b\",\"method\":\"GET\"}",
        ),
        (
            "a route the code answers with a status of its own",
            "GET",
            "/teapot",
            418,
            "text/plain",
            "short and stout",
        ),
    ];
    for (case, method, uri, status, content_type, body) in rows {
        let answered = requested(
            &emulator,
            name,
            "ep",
            method,
            uri,
            &[("authorization", SIGNED)],
        )
        .await;
        assert_eq!(
            answered.status().as_u16(),
            status,
            "case {case:?}: expected {status}, got {} with {:?}",
            answered.status(),
            body_text(&answered)
        );
        assert_eq!(
            header_of(&answered, "content-type"),
            content_type,
            "case {case:?}: expected the content type the code set, got {:?}",
            header_of(&answered, "content-type")
        );
        assert_eq!(
            body_text(&answered),
            body,
            "case {case:?}: expected the body the code produced, got {:?}",
            body_text(&answered)
        );
        assert_eq!(
            header_of(&answered, "content-length"),
            body.len().to_string(),
            "case {case:?}: expected the length of the body the code produced"
        );
        assert!(
            !header_of(&answered, REQUEST_ID_HEADER).is_empty(),
            "case {case:?}: every answer names the response it is"
        );
    }
}

/// RULE, over the other module form: a package that ships its server as an ES module — and
/// imports another file of its own to build the answer — is run the same way, and what the
/// client reads is what that code answered.
#[tokio::test]
async fn a_package_that_ships_an_es_module_is_run_as_one() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let name = "esm-handler";
    create_serving(&emulator, name, "ApplicationManaged", ESM_KEY, json!({})).await;
    poll_until_active(&emulator, name, "ep").await;

    let answered = request_endpoint(&emulator, name, "ep", "/", &[]).await;
    assert_eq!(
        answered.status().as_u16(),
        201,
        "expected the status the ES module set, got {} with {:?}",
        answered.status(),
        body_text(&answered)
    );
    assert_eq!(
        (
            body_text(&answered),
            header_of(&answered, "x-served-by")
        ),
        (
            "hello from an es module".to_string(),
            "esm".to_string()
        ),
        "expected the ES module's own answer, got {:?}",
        body_text(&answered)
    );
}

/// RULE: a request that reached no code produces no response, and that is reported as what
/// it was rather than as a response nothing produced — a revision whose code registers no
/// server is a `502`, and a revision whose build never read a package serves nothing at
/// all.
#[tokio::test]
async fn a_revision_that_answers_nothing_is_reported_as_that() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    // An object that is no archive at all: the build fails, so the revision is never ready
    // to serve traffic.
    stage_object(&emulator, "not-an-archive.zip", b"not a zip archive".to_vec()).await;

    // (case, the package the revision is built from, the status the address answers, the
    // body it answers with)
    let rows: [(&str, &str, u16, &str); 2] = [
        (
            "a package that registers no server",
            NO_SERVER_KEY,
            502,
            "The web function did not produce a response.\n",
        ),
        (
            "a revision whose build failed",
            "not-an-archive.zip",
            503,
            "No web function code is serving this endpoint.\n",
        ),
    ];
    for (index, (case, key, status, body)) in rows.into_iter().enumerate() {
        let name = format!("no-answer-{index}");
        create_serving(&emulator, &name, "ApplicationManaged", key, json!({})).await;
        poll_until_active(&emulator, &name, "ep").await;
        let answered = request_endpoint(&emulator, &name, "ep", "/", &[]).await;
        assert_eq!(
            answered.status().as_u16(),
            status,
            "case {case:?}: expected {status}, got {} with {:?}",
            answered.status(),
            body_text(&answered)
        );
        assert_eq!(
            body_text(&answered),
            body,
            "case {case:?}: expected {body:?}, got {:?}",
            body_text(&answered)
        );
    }
}

/// RULE: an endpoint whose `autoDeploymentMode` is `LatestRevision` "automatically serves
/// the newest revision", so once a newer revision has BUILT the code the address runs is
/// that revision's — and a `Disabled` endpoint keeps running the code it was given.
#[tokio::test]
async fn the_code_an_address_runs_is_the_revision_the_endpoint_routes_to() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;

    // (case, the endpoint's autoDeploymentMode, the body the address serves after a newer
    // revision has built)
    let rows: [(&str, &str, &str); 2] = [
        ("auto-deploying", "LatestRevision", "hello from an es module"),
        ("fixed", "Disabled", "hello from the first revision"),
    ];
    for (case, mode, expected) in rows {
        let name = format!("routed-{case}");
        let created = emulator
            .send(
                "PUT",
                "/web-functions",
                REGION,
                Some(json!({
                    "functionName": name,
                    "revisionConfig": {
                        "buildConfig": build_config("nodejs24.x", BUCKET, CJS_KEY, None),
                        "serviceConfig": {
                            "executionRoleArn": role_arn(&name),
                            "environmentVariables": { "GREETING": "the first revision" },
                        },
                    },
                    "endpointConfig": {
                        "endpointName": "ep",
                        "endpointType": "HomeRegion",
                        "authType": "ApplicationManaged",
                        "autoDeploymentMode": mode,
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateWebFunction expected 202, got {} {:?}",
            created.status, created.body
        );
        poll_until_active(&emulator, &name, "ep").await;
        let first = request_endpoint(&emulator, &name, "ep", "/", &[]).await;
        assert_eq!(
            body_text(&first),
            "hello from the first revision",
            "case {case:?}: expected the initial revision's own answer, got {:?}",
            body_text(&first)
        );

        // A newer revision, built from another package entirely: an auto-deploying endpoint
        // routes to it once it is ready to serve traffic.
        let added = emulator
            .send(
                "POST",
                &format!("/web-functions/{name}/revisions"),
                REGION,
                Some(json!({
                    "buildConfig": build_config("nodejs24.x", BUCKET, ESM_KEY, None),
                    "serviceConfig": { "executionRoleArn": role_arn(&name) },
                })),
            )
            .await;
        assert_eq!(
            added.status, 202,
            "case {case:?}: CreateWebFunctionRevision expected 202, got {} {:?}",
            added.status, added.body
        );
        // The newer revision's build settles on the CLOCK, so the test waits for THAT
        // condition — the revision reporting `Active`, which is when the switch is owed —
        // before reading what the address serves. Without this wait a row that expects no
        // switch would pass before the revision could have taken traffic at all.
        let revision_id = added
            .string("revisionId")
            .unwrap_or_else(|| panic!("case {case:?}: the create reports its revisionId"));
        poll_until_revision_active(&emulator, &name, &revision_id).await;

        // The switch itself is owed at the same instant, so the test looks again until the
        // address serves what the rule says it serves, or until a deadline far longer than
        // the service's own span has passed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut served = body_text(&request_endpoint(&emulator, &name, "ep", "/", &[]).await);
        while served != expected && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            served = body_text(&request_endpoint(&emulator, &name, "ep", "/", &[]).await);
        }
        assert_eq!(
            served, expected,
            "case {case:?}: expected the address to run {expected:?}, got {served:?}"
        );
    }
}

/// Poll `GetWebFunctionRevision` until the revision reports `Active` — the state in which it
/// is "ready to serve traffic", which is when a switch onto it is owed.
///
/// The build settles on the CLOCK, so this waits for the condition rather than for a
/// duration: it looks again until it holds, or until a deadline far longer than the service's
/// own span has passed.
async fn poll_until_revision_active(emulator: &Emulator, name: &str, revision_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let read = emulator
            .send(
                "GET",
                &format!("/web-functions/{name}/revisions/{revision_id}"),
                REGION,
                None,
            )
            .await;
        if read.string("state").as_deref() == Some("Active") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the revision {name}/{revision_id} never reached Active, last read {:?}",
            read.body
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// RULE: the `403` an `IamAuth` endpoint refuses a request presenting no SigV4 credential
/// with is the `Forbidden` PAGE, served as `text/html`, and it NAMES THE RESPONSE IT IS:
/// the id in its `Request ID:` line is the value of that same response's
/// `lambda-web-request-id` header, and every refusal carries its own.
///
/// The rows span the forms an unsigned request arrives in — its method, the depth of its
/// path, whether it carries a query string, and an `Authorization` header that is not a
/// SigV4 credential — because the refusal depends on none of them.
#[tokio::test]
async fn an_unsigned_request_is_refused_with_the_forbidden_page_naming_that_response() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let name = "forbidden-page";
    create_with_endpoint(&emulator, name, "IamAuth").await;
    // The address answers the endpoint's own decisions once the endpoint is `Active`.
    poll_until_active(&emulator, name, "ep").await;

    // (case, the request's method, its URI, its headers)
    let rows: &[UnsignedRow<'_>] = &[
        ("an unsigned GET", "GET", "/", &[]),
        (
            "an unsigned GET deeper in the path",
            "GET",
            "/items/42/reviews",
            &[],
        ),
        ("an unsigned GET with a query", "GET", "/items?page=2", &[]),
        ("an unsigned POST", "POST", "/orders", &[]),
        (
            "an unsigned PUT with a query",
            "PUT",
            "/orders/7?mode=replace",
            &[],
        ),
        (
            "another authorization scheme",
            "GET",
            "/",
            &[("authorization", "Basic dXNlcjpwYXNz")],
        ),
        (
            "a bearer token",
            "DELETE",
            "/sessions/9",
            &[("authorization", "Bearer abc.def")],
        ),
    ];
    let mut named = Vec::new();
    for (case, method, uri, headers) in rows {
        let answered = requested(&emulator, name, "ep", method, uri, headers).await;
        assert_eq!(
            answered.status().as_u16(),
            403,
            "case {case:?}: expected 403, got {} with {:?}",
            answered.status(),
            body_text(&answered)
        );
        assert_eq!(
            header_of(&answered, "content-type"),
            "text/html",
            "case {case:?}: expected the page to be served as HTML, got {:?}",
            header_of(&answered, "content-type")
        );
        let request_id = header_of(&answered, REQUEST_ID_HEADER);
        assert!(
            !request_id.is_empty(),
            "case {case:?}: a 403 response must carry a {REQUEST_ID_HEADER} header, got the \
             headers {:?}",
            answered.headers()
        );
        assert_eq!(
            body_text(&answered),
            forbidden_page(&request_id),
            "case {case:?}: expected the Forbidden page naming {request_id:?}, got {:?}",
            body_text(&answered)
        );
        named.push(request_id);
    }
    // The id names the RESPONSE, so two refusals are distinguishable by it.
    let distinct: std::collections::BTreeSet<&String> = named.iter().collect();
    assert_eq!(
        distinct.len(),
        named.len(),
        "each refusal must name its own response, got {named:?}"
    );
}

/// RULE: the published address is a socket this emulator ANSWERS on — a client that
/// followed `domainName` and dialled it gets the endpoint's own answer over HTTP, which for
/// an authorized request is the response the revision's code produced.
///
/// Dialled in the clear, which the address serves beside TLS (the scheme is negotiated
/// from the connection's first byte), so the test needs no certificate trust store to
/// prove the socket answers.
#[tokio::test]
async fn a_client_that_dials_the_published_address_is_answered_there() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let domain_name = create_with_endpoint(&emulator, "dialled-endpoint", "IamAuth").await;
    // The address answers the endpoint's own decisions once the endpoint is `Active`.
    poll_until_active(&emulator, "dialled-endpoint", "ep").await;

    // (case, the request's headers, the status line the address answers with, the body it
    // serves)
    let rows: [(&str, &str, &str, &str); 2] = [
        ("an unsigned GET", "", "HTTP/1.1 403 Forbidden", ""),
        (
            "a signed GET",
            &format!("authorization: {SIGNED}\r\n"),
            "HTTP/1.1 200 OK",
            "hello from the revision",
        ),
    ];
    for (case, headers, expected, body) in rows {
        let answered = dial(&domain_name, headers).await;
        assert!(
            answered.starts_with(expected),
            "case {case:?}: expected the address to answer {expected:?}, got {answered:?}"
        );
        if !body.is_empty() {
            let served = answered
                .split_once("\r\n\r\n")
                .map(|(_, body)| body.to_string())
                .unwrap_or_default();
            assert_eq!(
                served, body,
                "case {case:?}: expected the code's own body, got {served:?}"
            );
        }
    }
}

/// One HTTP/1.1 `GET /` to the published address `authority`, carrying `headers`, read
/// back as text.
///
/// The dial names the LOOPBACK interface itself: the address a published endpoint carries
/// is one of this emulator's own listeners (asserted above), so only its port is taken
/// from the published value and the host is this process's own loopback address. Nothing
/// outside this machine is reachable from here.
async fn dial(authority: &str, headers: &str) -> String {
    dial_request(authority, "GET", "/", headers).await
}

/// [`dial`] for a request of any method and request target, which is what a client that
/// followed `domainName` issues: the handler's answer is computed from both.
async fn dial_request(authority: &str, method: &str, target: &str, headers: &str) -> String {
    dial_body(authority, method, target, headers, "").await
}

/// [`dial_request`] for a request carrying a BODY, which is another part of the request a
/// handler computes its answer from: its length is stated the way the transport requires.
async fn dial_body(
    authority: &str,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let host = service_common::data_plane_addressing::host_only(authority);
    assert!(
        emulator_server::is_loopback_host(host),
        "the published address {authority} must be one of this emulator's own listeners"
    );
    let port = service_common::data_plane_addressing::addressed_port(Some(authority))
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or_else(|| panic!("the published address {authority} must name a port"));
    let mut socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| panic!("the published address {authority} must accept a connection: {error}"));
    // The length of what is written, stated once: a caller that states it in `headers`
    // itself keeps that value, so no request ever carries the header twice.
    let stated_length = headers
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .any(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"));
    let length = if stated_length {
        String::new()
    } else {
        format!("content-length: {}\r\n", body.len())
    };
    let request = format!(
        "{method} {target} HTTP/1.1\r\nhost: {authority}\r\n{headers}{length}\
         connection: close\r\n\r\n{body}"
    );
    socket
        .write_all(request.as_bytes())
        .await
        .expect("the request is written");
    let mut read = Vec::new();
    socket
        .read_to_end(&mut read)
        .await
        .expect("the answer is read");
    String::from_utf8_lossy(&read).to_string()
}

/// The status code of a raw answer read off the socket.
fn status_on_the_wire(answered: &str) -> u16 {
    answered
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("the answer must open with a status line, got {answered:?}"))
}

/// The body of a raw answer read off the socket, empty when the answer carries none.
fn body_on_the_wire(answered: &str) -> String {
    answered
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default()
}

/// The value of the response header `name` on a raw answer read off the socket, empty when
/// the answer carries none.
fn header_on_the_wire(answered: &str, name: &str) -> String {
    answered
        .split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(header, _)| header.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default()
}

/// RULE: what a CLIENT is served at the published address is the refusal itself — the page
/// as the body and the id on the response — so a client that followed `domainName` and
/// dialled it can show the page and correlate the refusal by its id.
#[tokio::test]
async fn a_dialled_unsigned_request_is_served_the_page_and_its_id() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let name = "dialled-refusal";
    let domain_name = create_with_endpoint(&emulator, name, "IamAuth").await;
    // The address answers the endpoint's own decisions once the endpoint is `Active`.
    poll_until_active(&emulator, name, "ep").await;

    // Two dials, so the per-response id is proved over the wire and not just once.
    let mut named = Vec::new();
    for case in ["the first dial", "the second dial"] {
        let answered = dial(&domain_name, "").await;
        assert!(
            answered.starts_with("HTTP/1.1 403 Forbidden"),
            "case {case:?}: expected a 403, got {answered:?}"
        );
        assert_eq!(
            header_on_the_wire(&answered, "content-type"),
            "text/html",
            "case {case:?}: expected the page to be served as HTML, got {answered:?}"
        );
        let request_id = header_on_the_wire(&answered, REQUEST_ID_HEADER);
        assert!(
            !request_id.is_empty(),
            "case {case:?}: a 403 response must carry a {REQUEST_ID_HEADER} header, got \
             {answered:?}"
        );
        let body = answered
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        assert_eq!(
            body,
            forbidden_page(&request_id),
            "case {case:?}: expected the Forbidden page naming {request_id:?}, got {body:?}"
        );
        named.push(request_id);
    }
    assert_ne!(
        named[0], named[1],
        "each refusal must name its own response, got {named:?}"
    );
}

/// The SHA-256 a signed request states for an empty body, which a signed GET carries in its
/// `x-amz-content-sha256` header.
const EMPTY_BODY_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// One row of the dialled-handler table: the case, the headers the request presents its
/// SigV4 credential in, its method, its request target, and the status, the content type and
/// the body the revision's own code answered it with.
type DialledRow<'a> = (&'a str, String, &'a str, &'a str, u16, &'a str, &'a str);

/// RULE: the address `GetWebFunctionEndpoint` reports as `domainName` IS the web function's
/// HTTP data plane once that same read says the endpoint's `state` is `Active` — so an
/// authorized request a client issues there is answered with the response the revision the
/// endpoint routes it to produced: its status, its headers and its body are all that code's
/// own, and nothing fixed of this emulator's is served in their place.
///
/// This walks the whole path a client walks, over a real socket: read `domainName` off the
/// Active endpoint, present a SigV4 credential, read what comes back. The rows differ along
/// everything the handler decides — the status (a `2xx` of its own and a non-`2xx` one), the
/// content type, the shape of the body, and the method and path the answer is computed from
/// — and along the two forms a credential is presented in (an `Authorization` header, and
/// the query parameters of a presigned URL), because what is served is the code's answer
/// whichever way the request was authorized.
#[tokio::test]
async fn a_signed_request_to_an_active_endpoints_domain_name_is_served_the_handlers_response() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let name = "dialled-handler-response";
    create_serving(
        &emulator,
        name,
        "IamAuth",
        CJS_KEY,
        json!({ "GREETING": "the dialled revision" }),
    )
    .await;

    // The address a client uses is the one the READ reports, taken once that same read says
    // the endpoint is serving traffic — the order a client follows.
    let active = poll_until_active(&emulator, name, "ep").await;
    let domain_name = active
        .get("domainName")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("an Active endpoint reports the domain name it is served at, got {active:?}")
        })
        .to_string();

    let signed = format!("authorization: {SIGNED}\r\nx-amz-content-sha256: {EMPTY_BODY_SHA256}\r\n");
    let rows: [DialledRow<'_>; 5] = [
        (
            "a signed GET of the route the code answers from its own environment",
            signed.clone(),
            "GET",
            "/",
            200,
            "text/plain",
            "hello from the dialled revision",
        ),
        (
            "a signed GET of a route the code answers as JSON built from the path",
            signed.clone(),
            "GET",
            "/items/42",
            200,
            "application/json",
            "{\"item\":\"42\",\"method\":\"GET\"}",
        ),
        (
            "a signed POST of that same route",
            signed.clone(),
            "POST",
            "/items/42",
            200,
            "application/json",
            "{\"item\":\"42\",\"method\":\"POST\"}",
        ),
        (
            "a signed GET of a route the code answers with a status of its own",
            signed.clone(),
            "GET",
            "/teapot",
            418,
            "text/plain",
            "short and stout",
        ),
        (
            "a presigned GET, whose credential is in the query",
            String::new(),
            "GET",
            "/?X-Amz-Credential=AKIDEXAMPLE&X-Amz-Signature=deadbeef",
            200,
            "text/plain",
            "hello from the dialled revision",
        ),
    ];
    for (case, headers, method, target, status, content_type, body) in rows {
        let answered = dial_request(&domain_name, method, target, &headers).await;
        assert_eq!(
            status_on_the_wire(&answered),
            status,
            "case {case:?}: expected the status the code set, got {answered:?}"
        );
        assert_eq!(
            header_on_the_wire(&answered, "content-type"),
            content_type,
            "case {case:?}: expected the content type the code set, got {:?}",
            header_on_the_wire(&answered, "content-type")
        );
        assert_eq!(
            header_on_the_wire(&answered, "x-served-by"),
            "commonjs",
            "case {case:?}: expected the header the code set, got {answered:?}"
        );
        assert_eq!(
            body_on_the_wire(&answered),
            body,
            "case {case:?}: expected the body the code produced, got {:?}",
            body_on_the_wire(&answered)
        );
        assert!(
            !header_on_the_wire(&answered, REQUEST_ID_HEADER).is_empty(),
            "case {case:?}: every answer names the response it is, got {answered:?}"
        );
    }
}

/// RULE: the address belongs to the endpoint — once the endpoint is gone, nothing is
/// published there, and the listener it answered on is released.
#[tokio::test]
async fn a_deleted_endpoints_address_publishes_nothing() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let name = "address-lifetime";
    create_with_endpoint(&emulator, name, "ApplicationManaged").await;
    // The address answers the endpoint's own decisions once the endpoint is `Active`.
    poll_until_active(&emulator, name, "ep").await;
    let serving = request_endpoint(&emulator, name, "ep", "/", &[]).await;
    assert_eq!(
        serving.status().as_u16(),
        200,
        "an active endpoint answers at its address with its code's own response, got {}",
        body_text(&serving)
    );

    // The delete of the function takes its endpoints with it, so the address stops
    // answering and the listener it answered on is released by that delete.
    let deleted = emulator
        .send("DELETE", &format!("/web-functions/{name}"), REGION, None)
        .await;
    assert!(
        (200..300).contains(&deleted.status),
        "DeleteWebFunction expected a 2xx, got {} {:?}",
        deleted.status,
        deleted.body
    );
    let gone =
        request_endpoint(&emulator, name, "ep", "/", &[("authorization", SIGNED)]).await;
    assert_eq!(
        gone.status().as_u16(),
        404,
        "a deleted endpoint publishes nothing at its address, got {}",
        body_text(&gone)
    );
    let state = service_state(&**emulator.engine()).expect("this service's store");
    assert!(
        state.lock().endpoint_planes.is_empty(),
        "the listener of a deleted endpoint must be released"
    );
}

/// One row of the auth-gate table below: the endpoint's `authType`, the package the
/// revision it routes to runs, and the answer THAT PACKAGE'S OWN CODE produces for the
/// request — the status, the content type, the body and the header it sets.
struct AuthGateRow {
    case: &'static str,
    auth_type: &'static str,
    key: &'static str,
    /// The revision's own environment variables, which the CommonJS package answers from.
    environment: Value,
    status: u16,
    content_type: &'static str,
    body: &'static str,
    served_by: &'static str,
}

/// The rows: the rule generalises over the endpoint's `authType` and over the code the
/// revision ships, so the rows differ along both — each `authType` value crossed with each
/// module form a Node.js package takes. The recorded case (an `IamAuth` endpoint running a
/// CommonJS package) is one row.
fn auth_gate_rows() -> Vec<AuthGateRow> {
    vec![
        AuthGateRow {
            case: "an IamAuth endpoint running a CommonJS package",
            auth_type: "IamAuth",
            key: CJS_KEY,
            environment: json!({ "GREETING": "the iam-authorized revision" }),
            status: 200,
            content_type: "text/plain",
            body: "hello from the iam-authorized revision",
            served_by: "commonjs",
        },
        AuthGateRow {
            case: "an IamAuth endpoint running an ES module package",
            auth_type: "IamAuth",
            key: ESM_KEY,
            environment: json!({}),
            status: 201,
            content_type: "text/plain",
            body: "hello from an es module",
            served_by: "esm",
        },
        AuthGateRow {
            case: "an ApplicationManaged endpoint running a CommonJS package",
            auth_type: "ApplicationManaged",
            key: CJS_KEY,
            environment: json!({ "GREETING": "the application-managed revision" }),
            status: 200,
            content_type: "text/plain",
            body: "hello from the application-managed revision",
            served_by: "commonjs",
        },
        AuthGateRow {
            case: "an ApplicationManaged endpoint running an ES module package",
            auth_type: "ApplicationManaged",
            key: ESM_KEY,
            environment: json!({}),
            status: 201,
            content_type: "text/plain",
            body: "hello from an es module",
            served_by: "esm",
        },
    ]
}

/// RULE: at ONE published address, the endpoint's `authType` decides only what a request
/// PRESENTING NO SigV4 credential earns — an `IamAuth` endpoint ("requests ... must be
/// signed with Signature Version 4") refuses it with the `403 Forbidden` page naming that
/// response, an `ApplicationManaged` endpoint ("your application handles authorization")
/// refuses nothing — while a request that DOES present a credential is answered by the
/// revision's own code either way, with that code's status, headers and body.
///
/// So the refusal is observable at an address that SERVES: the same address, in the same
/// state, answers the authorized request with the handler's response, which is what makes
/// the unsigned request's `403` mean the auth gate rather than an endpoint that serves
/// nothing. A request is also not a consumable: once the refusal has been earned, the next
/// authorized request earns the same answer the first did, under an id of its own.
///
/// Driven over a real socket at the address `GetWebFunctionEndpoint` reports, in the order
/// a client follows: read `domainName` off the `Active` endpoint, issue the authorized
/// request, then the same request with no credential at all.
#[tokio::test]
async fn an_authorized_request_is_served_at_the_same_address_that_refuses_an_unsigned_one() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;
    let rows = auth_gate_rows();

    // Every function is created FIRST, so the clock-owed `Pending -> Active` spans of the
    // rows' endpoints are waited out together rather than one after another.
    let mut published = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("auth-gate-{index}");
        let address = create_serving(
            &emulator,
            &name,
            row.auth_type,
            row.key,
            row.environment.clone(),
        )
        .await;
        assert!(
            !address.is_empty(),
            "case {:?}: the create must publish the endpoint's address, got {address:?}",
            row.case
        );
        published.push((name, address));
    }

    let signed =
        format!("authorization: {SIGNED}\r\nx-amz-content-sha256: {EMPTY_BODY_SHA256}\r\n");
    for (row, (name, address)) in rows.iter().zip(published) {
        let case = row.case;
        // The address a client uses is the one the READ reports, taken once that same read
        // says the endpoint is serving traffic.
        let active = poll_until_active(&emulator, &name, "ep").await;
        let domain_name = active
            .get("domainName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            domain_name, address,
            "case {case:?}: the Active read must report the published address, got {active:?}"
        );

        // The positive control, in both forms a client presents a SigV4 credential in.
        let presentations: [(&str, &str, String); 2] = [
            ("a credential in the Authorization header", "/", signed.clone()),
            (
                "a presigned credential in the query",
                "/?X-Amz-Credential=AKIDEXAMPLE&X-Amz-Signature=deadbeef",
                String::new(),
            ),
        ];
        let mut named = Vec::new();
        for (presentation, target, headers) in presentations {
            let answered = dial_request(&domain_name, "GET", target, &headers).await;
            assert_eq!(
                status_on_the_wire(&answered),
                row.status,
                "case {case:?}/{presentation:?}: expected the status the code set, got {answered:?}"
            );
            assert_eq!(
                header_on_the_wire(&answered, "content-type"),
                row.content_type,
                "case {case:?}/{presentation:?}: expected the content type the code set, got {:?}",
                header_on_the_wire(&answered, "content-type")
            );
            assert_eq!(
                header_on_the_wire(&answered, "x-served-by"),
                row.served_by,
                "case {case:?}/{presentation:?}: expected the header the code set, got {answered:?}"
            );
            assert_eq!(
                body_on_the_wire(&answered),
                row.body,
                "case {case:?}/{presentation:?}: expected the body the code produced, got {:?}",
                body_on_the_wire(&answered)
            );
            let request_id = header_on_the_wire(&answered, REQUEST_ID_HEADER);
            assert!(
                !request_id.is_empty(),
                "case {case:?}/{presentation:?}: every answer names the response it is in its \
                 {REQUEST_ID_HEADER} header, got {answered:?}"
            );
            named.push(request_id);
        }

        // The SAME request at that same address, presenting no credential at all.
        let unsigned = dial_request(&domain_name, "GET", "/", "").await;
        if row.auth_type == "IamAuth" {
            assert_eq!(
                status_on_the_wire(&unsigned),
                403,
                "case {case:?}: an unsigned request to an IamAuth endpoint is refused, got \
                 {unsigned:?}"
            );
            assert_eq!(
                header_on_the_wire(&unsigned, "content-type"),
                "text/html",
                "case {case:?}: expected the refusal to be served as the page, got {:?}",
                header_on_the_wire(&unsigned, "content-type")
            );
            let request_id = header_on_the_wire(&unsigned, REQUEST_ID_HEADER);
            assert!(
                !request_id.is_empty(),
                "case {case:?}: a 403 must carry a {REQUEST_ID_HEADER} header, got {unsigned:?}"
            );
            assert_eq!(
                body_on_the_wire(&unsigned),
                forbidden_page(&request_id),
                "case {case:?}: expected the Forbidden page naming {request_id:?}, got {:?}",
                body_on_the_wire(&unsigned)
            );
            assert!(
                !named.contains(&request_id),
                "case {case:?}: each answer names its own response, got {request_id:?} twice"
            );
            named.push(request_id);
        } else {
            assert_eq!(
                status_on_the_wire(&unsigned),
                row.status,
                "case {case:?}: an ApplicationManaged endpoint refuses nothing of its own, got \
                 {unsigned:?}"
            );
            assert_eq!(
                body_on_the_wire(&unsigned),
                row.body,
                "case {case:?}: expected the body the code produced, got {:?}",
                body_on_the_wire(&unsigned)
            );
        }

        // Nothing was consumed by either answer: the next authorized request earns the same
        // answer the first did, under an id of its own.
        let again = dial_request(&domain_name, "GET", "/", &signed).await;
        assert_eq!(
            status_on_the_wire(&again),
            row.status,
            "case {case:?}: expected the code's own status again, got {again:?}"
        );
        assert_eq!(
            body_on_the_wire(&again),
            row.body,
            "case {case:?}: expected the code's own body again, got {:?}",
            body_on_the_wire(&again)
        );
        let request_id = header_on_the_wire(&again, REQUEST_ID_HEADER);
        assert!(
            !request_id.is_empty() && !named.contains(&request_id),
            "case {case:?}: each answer names its own response, got {request_id:?} among \
             {named:?}"
        );
    }
}

/// One row of the unsigned-request table below: the case, the request a client issues with
/// no credential of any kind (its method, its request target, the headers it carries and
/// its body), and the answer THE REVISION'S OWN CODE computes for that request — the
/// status, the content type and the body.
struct UnsignedServedRow {
    case: &'static str,
    method: &'static str,
    target: &'static str,
    headers: &'static str,
    body: &'static str,
    status: u16,
    content_type: &'static str,
    answered: &'static str,
}

/// The rows: the rule generalises over the whole request, so they differ along everything
/// the handler's answer is computed from — the method, the depth and query of the request
/// target, a request header, the request body — and along what the handler decides from it:
/// a `2xx` of its own, a non-`2xx` of its own, a text body and a JSON one. They also differ
/// along the FORM an unsigned request arrives in, because none of them presents a SigV4
/// credential: no headers at all, and an `Authorization` header that is not one.
fn unsigned_served_rows() -> Vec<UnsignedServedRow> {
    vec![
        UnsignedServedRow {
            case: "a GET with no headers of its own",
            method: "GET",
            target: "/",
            headers: "",
            body: "",
            status: 200,
            content_type: "text/plain",
            answered: "the application answered GET /",
        },
        UnsignedServedRow {
            case: "a GET of a deeper path, which the code answers from",
            method: "GET",
            target: "/orders/7/items",
            headers: "",
            body: "",
            status: 200,
            content_type: "text/plain",
            answered: "the application answered GET /orders/7/items",
        },
        UnsignedServedRow {
            case: "a GET carrying a query and a header the code reads, answered as JSON",
            method: "GET",
            target: "/echo?item=7",
            headers: "x-note: kept\r\n",
            body: "",
            status: 200,
            content_type: "application/json",
            answered: r#"{"method":"GET","url":"/echo?item=7","note":"kept","body":""}"#,
        },
        UnsignedServedRow {
            case: "a POST whose body the code reads back",
            method: "POST",
            target: "/echo",
            headers: "content-type: text/plain\r\n",
            body: "ship it",
            status: 200,
            content_type: "application/json",
            answered: r#"{"method":"POST","url":"/echo","note":"","body":"ship it"}"#,
        },
        UnsignedServedRow {
            case: "a DELETE, which the code answers from the method it was handed",
            method: "DELETE",
            target: "/echo",
            headers: "",
            body: "",
            status: 200,
            content_type: "application/json",
            answered: r#"{"method":"DELETE","url":"/echo","note":"","body":""}"#,
        },
        UnsignedServedRow {
            case: "a GET of a route the code answers with a non-2xx status of its own",
            method: "GET",
            target: "/gone",
            headers: "",
            body: "",
            status: 410,
            content_type: "text/plain",
            answered: "the application decided this is gone",
        },
        UnsignedServedRow {
            case: "a GET whose Authorization header is not a SigV4 credential",
            method: "GET",
            target: "/private",
            headers: "authorization: Bearer not-a-sigv4-credential\r\n",
            body: "",
            status: 200,
            content_type: "text/plain",
            answered: "the application answered GET /private",
        },
    ]
}

/// RULE: an `ApplicationManaged` endpoint ("your application handles authorization")
/// refuses nothing of its own, so a request presenting NO credential is an authorized
/// request there — and what the published address answers an authorized request with is the
/// response the revision's own code produced: its status, its headers and its body, computed
/// from the request that code was handed. Nothing fixed of this emulator's is served in
/// their place, for any method, any path or any body.
///
/// That is what makes `authType` observable: the SAME unsigned request, at an endpoint whose
/// only difference is `authType: IamAuth` ("requests ... must be signed with Signature
/// Version 4"), earns the `403 Forbidden` page instead, so the two values of the enum answer
/// one request two different ways.
///
/// Driven over a real socket, along the path a client walks: read `domainName` off the
/// `Active` endpoint, then issue the unsigned request there.
#[tokio::test]
async fn an_unsigned_request_to_an_application_managed_endpoint_is_served_the_handlers_response() {
    let emulator = Emulator::with_object_store();
    stage_packages(&emulator).await;

    // Both endpoints are created first, so their clock-owed `Pending -> Active` spans are
    // waited out together.
    let served = create_serving(
        &emulator,
        "unsigned-served",
        "ApplicationManaged",
        REQUEST_ECHO_KEY,
        json!({}),
    )
    .await;
    let refusing = create_serving(
        &emulator,
        "unsigned-refused",
        "IamAuth",
        REQUEST_ECHO_KEY,
        json!({}),
    )
    .await;

    // The address a client uses is the one the READ reports, taken once that same read says
    // the endpoint is serving traffic.
    let active = poll_until_active(&emulator, "unsigned-served", "ep").await;
    let domain_name = active
        .get("domainName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        domain_name, served,
        "the Active read must report the published address, got {active:?}"
    );
    poll_until_active(&emulator, "unsigned-refused", "ep").await;

    let mut named: Vec<String> = Vec::new();
    for row in unsigned_served_rows() {
        let case = row.case;
        let answered = dial_body(&domain_name, row.method, row.target, row.headers, row.body).await;
        assert_eq!(
            status_on_the_wire(&answered),
            row.status,
            "case {case:?}: expected the status the revision's code set, got {answered:?}"
        );
        assert_eq!(
            header_on_the_wire(&answered, "content-type"),
            row.content_type,
            "case {case:?}: expected the content type the code set, got {:?}",
            header_on_the_wire(&answered, "content-type")
        );
        assert_eq!(
            header_on_the_wire(&answered, "x-served-by"),
            "request-echo",
            "case {case:?}: expected the header the code set, got {answered:?}"
        );
        assert_eq!(
            body_on_the_wire(&answered),
            row.answered,
            "case {case:?}: expected the body the code produced, got {:?}",
            body_on_the_wire(&answered)
        );
        let request_id = header_on_the_wire(&answered, REQUEST_ID_HEADER);
        assert!(
            !request_id.is_empty() && !named.contains(&request_id),
            "case {case:?}: every answer names the response it is in its {REQUEST_ID_HEADER} \
             header, got {request_id:?} among {named:?}"
        );
        named.push(request_id);

        // The other arm of `authType`, answering the identical unsigned request.
        let refused = dial_body(&refusing, row.method, row.target, row.headers, row.body).await;
        assert_eq!(
            status_on_the_wire(&refused),
            403,
            "case {case:?}: an IamAuth endpoint refuses the same unsigned request, got {refused:?}"
        );
        let refusal_id = header_on_the_wire(&refused, REQUEST_ID_HEADER);
        assert_eq!(
            body_on_the_wire(&refused),
            forbidden_page(&refusal_id),
            "case {case:?}: expected the Forbidden page naming {refusal_id:?}, got {:?}",
            body_on_the_wire(&refused)
        );
    }
}
