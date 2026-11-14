// SPDX-License-Identifier: CC0-1.0

//! Invoking an api through a CUSTOM DOMAIN NAME and its api mappings, over a real connection
//! to the address the domain name publishes.
//!
//! The rule these cases assert, one level above the recorded case (one domain name, one
//! mapping, one stage, invoked at the domain's root):
//!
//! > A custom domain name publishes an address a client can DIAL
//! > (`DomainNameConfiguration$ApiGatewayDomainName`, "the domain name associated with the
//! > regional endpoint for this custom domain name"), and a request made there is routed by
//! > the api mapping whose `ApiMappingKey` the request's path begins with — the LONGEST such
//! > key, on a segment boundary, with the mapping carrying no key answering every path no
//! > other mapping claims — to the `{ApiId, Stage}` that mapping binds, which then serves the
//! > request exactly as that api's own `execute-api` endpoint serves it, under the stage the
//! > mapping names. A path no mapping of the domain name answers to reaches no api at all.
//!
//! Nothing here is a value this emulator fills in: every served row is answered by a FUNCTION
//! that ran, which is why AWS Lambda is mounted beside this service, and the handler reports
//! which api, which stage and which path it was given — so a row proves WHICH stage served it
//! rather than merely that something answered.
//!
//! The rows span what the rule generalises over: the mapping key (none, one segment, two
//! segments), which of two apis and which stage the matched mapping names (a named stage and
//! the one that serves an api's root), the path under the key (a path, the key itself, the
//! domain root), a query string, both `Host` fields a client of the address sends (the address
//! itself and the domain name a caller pointed at it), a `Host` the address does not stand
//! for, a path no mapping answers to, and a mapping that has been deleted.
//!
//! The control-plane surface of a domain name and its mappings is not restated here:
//! `domain_name_lifecycle.rs` and `api_mapping_lifecycle.rs` own it.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::http_api_backend_dispatch::{
    emulator_with_lambda, given_api, given_function, lambda_integration,
};

/// The Region every control-plane request below is signed for — the one
/// `http_api_backend_dispatch` creates its functions in, so an integration's ARN names the
/// Region its function is in.
const REGION: &str = "us-east-1";

/// The status a request routed to a stage is answered with here: the function's own.
const SERVED: u16 = 200;

/// The status and body the domain name's front door turns a request away with when no api
/// mapping of it answers the path: the refusal API Gateway answers a request that reaches no
/// api of the account with, in the `{"message": …}` envelope every answer of this data plane
/// carries.
const NO_MAPPING: u16 = 403;
const NO_MAPPING_MESSAGE: &str = "Forbidden";

/// The status a request whose `Host` is not the name this address was published for is
/// answered with: the address stands for ONE domain name, and a request naming another
/// authority reached a server unwilling to answer for it (RFC 9110 §15.5.22).
const MISDIRECTED: u16 = 421;

/// How long one request on the published address may take before the case fails: generous for
/// a loaded machine, and bounded so a case that will never be answered fails instead of
/// hanging the suite.
const ANSWER_DEADLINE: Duration = Duration::from_secs(30);

/// The handler every row is served by: it reports the api, the stage and the path the event
/// says it was invoked for, which is what tells one stage's answer from another's.
const ECHO: &str = "import json\n\
     def lambda_handler(event, context):\n\
     \x20   return {\n\
     \x20       'statusCode': 200,\n\
     \x20       'headers': {'content-type': 'application/json'},\n\
     \x20       'body': json.dumps({\n\
     \x20           'api': event['requestContext']['apiId'],\n\
     \x20           'stage': event['requestContext']['stage'],\n\
     \x20           'path': event.get('rawPath'),\n\
     \x20           'query': event.get('rawQueryString'),\n\
     \x20       }),\n\
     \x20   }\n";

/// One control-plane request against this service, signed for [`REGION`].
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("the body serializes")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/apigateway/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create the custom domain name and answer the address it published.
async fn given_domain_name(emulator: &MultiServiceRouter, case: &str, domain: &str) -> String {
    let (status, created) = control(
        emulator,
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": domain,
            "domainNameConfigurations": [{
                "certificateArn": format!(
                    "arn:aws:acm:{REGION}:000000000000:certificate/\
                     11111111-2222-3333-4444-555555555555"
                )
            }]
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateDomainName answered {status} {created}"
    );
    let published = created
        .pointer("/domainNameConfigurations/0/apiGatewayDomainName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // The member exists for a client to DIAL, so what is published has to be an address this
    // machine can open a connection to.
    assert!(
        published.parse::<std::net::SocketAddr>().is_ok(),
        "case {case:?}: expected an address a client can dial, got {published:?}"
    );
    published
}

/// Map `api`'s `stage` onto `key` of the domain name, and answer the mapping's id.
async fn given_mapping(
    emulator: &MultiServiceRouter,
    case: &str,
    domain: &str,
    api: &str,
    stage: &str,
    key: Option<&str>,
) -> String {
    let mut request = json!({ "apiId": api, "stage": stage });
    if let Some(key) = key {
        request["apiMappingKey"] = json!(key);
    }
    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/apimappings"),
        Some(request),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: CreateApiMapping answered {status} {created}"
    );
    created
        .get("apiMappingId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What one request to the published address was answered with.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The answer's body as JSON, for a row the function served.
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    /// The `message` the gateway's own refusals carry.
    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Send one `GET path` to the published address and read the whole answer.
///
/// The address is a core-owned listener that negotiates encryption in band, so a client that
/// opens in the clear is served in the clear — which is what this case does, one request per
/// connection (`Connection: close`), so the read ends with the answer.
///
/// `host` is the `Host` the client sends: the published address itself, or the custom DOMAIN
/// NAME a caller pointed at that address, which is what a real client of the domain sends.
async fn request_as(case: &str, authority: &str, path: &str, host: &str) -> Answer {
    let port = authority
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the published address {authority:?} is no address: {error}")
        })
        .port();
    let mut stream = tokio::time::timeout(
        ANSWER_DEADLINE,
        TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .unwrap_or_else(|_| panic!("case {case:?}: {authority} did not accept a connection in time"))
    .unwrap_or_else(|error| panic!("case {case:?}: {authority} refused a connection: {error}"));
    let wire =
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    tokio::time::timeout(ANSWER_DEADLINE, stream.write_all(wire.as_bytes()))
        .await
        .unwrap_or_else(|_| panic!("case {case:?}: the request was not written in time"))
        .unwrap_or_else(|error| panic!("case {case:?}: the request was not written: {error}"));
    let mut raw = Vec::new();
    tokio::time::timeout(ANSWER_DEADLINE, stream.read_to_end(&mut raw))
        .await
        .unwrap_or_else(|_| panic!("case {case:?}: {authority} did not answer in time"))
        .unwrap_or_else(|error| panic!("case {case:?}: the answer was not read: {error}"));
    let answered = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = answered
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("case {case:?}: expected an HTTP answer, got {answered:?}"));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a status line, got {head:?}"));
    Answer {
        status,
        body: body.to_string(),
    }
}

/// [`request_as`] with the published address as the `Host`.
async fn request(case: &str, authority: &str, path: &str) -> Answer {
    request_as(case, authority, path, authority).await
}

/// One row: the path a client asks the domain name for, and the api, stage and path the
/// answer must report.
struct Row {
    case: &'static str,
    path: &'static str,
    /// Which api of the two answered, the stage it answered under, and the path that stage
    /// reports it was addressed at.
    served: (Api, &'static str, &'static str),
}

/// Which of the two apis a row's answer must come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// The api mapped on the domain's ROOT (no mapping key), on the stage that serves its own
    /// root.
    Root,
    /// The api mapped under a key, on a NAMED stage.
    Keyed,
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_to_a_custom_domain_name_is_served_by_the_stage_its_mapping_names() {
    let case = "two apis mapped onto one custom domain name";
    let (emulator, engine) = emulator_with_lambda();
    let function = given_function(&emulator, &engine, case, "domain-mapping-echo", ECHO).await;
    // The api the ROOT mapping names, on the stage that serves an api's root.
    let (root_api, _) = given_api(
        &emulator,
        case,
        lambda_integration(&function, "2.0"),
        "$default",
        json!({ "stageName": "$default", "autoDeploy": true }),
    )
    .await;
    // The api a KEYED mapping names, on a named stage, so a row proves the mapping chose the
    // api AND the stage.
    let (keyed_api, _) = given_api(
        &emulator,
        case,
        lambda_integration(&function, "2.0"),
        "$default",
        json!({ "stageName": "prod", "autoDeploy": true }),
    )
    .await;

    let domain = "shop.api.example.com";
    let authority = given_domain_name(&emulator, case, domain).await;
    let root_mapping = given_mapping(&emulator, case, domain, &root_api, "$default", None).await;
    given_mapping(&emulator, case, domain, &keyed_api, "prod", Some("shop")).await;
    given_mapping(&emulator, case, domain, &keyed_api, "prod", Some("shop/eu")).await;

    let rows = [
        Row {
            case: "the root mapping serves a path no key claims",
            path: "/items",
            served: (Api::Root, "$default", "/items"),
        },
        Row {
            case: "the domain's own root",
            path: "/",
            served: (Api::Root, "$default", "/"),
        },
        Row {
            case: "a one-segment key, which is stripped from the path the stage is given",
            path: "/shop/items",
            served: (Api::Keyed, "prod", "/prod/items"),
        },
        Row {
            case: "the key itself, with nothing under it",
            path: "/shop",
            served: (Api::Keyed, "prod", "/prod"),
        },
        Row {
            case: "the longer of two matching keys",
            path: "/shop/eu/items",
            served: (Api::Keyed, "prod", "/prod/items"),
        },
        Row {
            case: "a segment that merely starts with a key falls to the root mapping",
            path: "/shopping/items",
            served: (Api::Root, "$default", "/shopping/items"),
        },
        Row {
            // The query string is the client's and is carried on unchanged: the mapping
            // chooses the api and the stage, and nothing of the request's own target is
            // rewritten beyond the base path it was addressed under.
            case: "a query string under a mapping key",
            path: "/shop/items?colour=red&size=2",
            served: (Api::Keyed, "prod", "/prod/items"),
        },
    ];

    for row in rows {
        let (api, stage, path) = row.served;
        let query = row
            .path
            .split_once('?')
            .map(|(_path, query)| query)
            .unwrap_or_default();
        let expected = json!({
            "api": match api {
                Api::Root => root_api.clone(),
                Api::Keyed => keyed_api.clone(),
            },
            "stage": stage,
            "path": path,
            "query": query,
        });
        // The same request under BOTH `Host` fields a client of the address sends: the
        // published address itself, and the custom domain name a caller pointed at it — which
        // is what a client that resolved the domain sends, and what the record is reached by.
        for host in [authority.as_str(), domain] {
            let answer = request_as(row.case, &authority, row.path, host).await;
            assert_eq!(
                answer.status, SERVED,
                "case {:?} under Host {host:?}: expected {SERVED} from the mapped stage, got \
                 {answer:?}",
                row.case
            );
            assert_eq!(
                answer.json(),
                expected,
                "case {:?} under Host {host:?}: expected the answer of {expected}, got \
                 {answer:?}",
                row.case
            );
        }
    }

    // A mapping that is DELETED stops routing: the path it answered reaches no api of the
    // account, while the mapping beside it serves as before.
    let (status, deleted) = control(
        &emulator,
        "DELETE",
        &format!("/v2/domainnames/{domain}/apimappings/{root_mapping}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: DeleteApiMapping answered {status} {deleted}"
    );
    let answer = request("a path whose mapping was deleted", &authority, "/items").await;
    assert_eq!(
        answer.status, NO_MAPPING,
        "case {case:?}: expected {NO_MAPPING} once the root mapping is gone, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        NO_MAPPING_MESSAGE,
        "case {case:?}: expected the gateway's own sentence for a path no mapping answers, \
         got {answer:?}"
    );
    let answer = request("the mapping that is still there", &authority, "/shop/items").await;
    assert_eq!(
        answer.status, SERVED,
        "case {case:?}: expected the remaining mapping to serve as before, got {answer:?}"
    );

    // The address stands for ONE domain name: a request naming another authority is answered
    // for none of this account's domain names, whatever its path maps to.
    for (label, host) in [
        ("another domain name of the account", "other.api.example.com"),
        ("a name that merely ends with the served one", "evil-shop.api.example.com"),
    ] {
        let answer = request_as(label, &authority, "/shop/items", host).await;
        assert_eq!(
            answer.status, MISDIRECTED,
            "case {label:?}: expected {MISDIRECTED} for a Host this address does not stand \
             for, got {answer:?}"
        );
    }
}

/// Deleting the domain name closes the address it published: the socket is the record's, so a
/// client that keeps dialling it reaches nothing rather than an api it no longer maps.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_the_domain_name_closes_the_address_it_published() {
    let case = "the domain name is deleted while its address is published";
    let (emulator, engine) = emulator_with_lambda();
    let function = given_function(&emulator, &engine, case, "domain-closing-echo", ECHO).await;
    let (api_id, _) = given_api(
        &emulator,
        case,
        lambda_integration(&function, "2.0"),
        "$default",
        json!({ "stageName": "$default", "autoDeploy": true }),
    )
    .await;
    let domain = "closing.api.example.com";
    let authority = given_domain_name(&emulator, case, domain).await;
    given_mapping(&emulator, case, domain, &api_id, "$default", None).await;

    // While the domain name exists its address serves the mapped stage.
    let answer = request(case, &authority, "/items").await;
    assert_eq!(
        answer.status, SERVED,
        "case {case:?}: the published address must serve the mapped stage, got {answer:?}"
    );

    let (status, deleted) =
        control(&emulator, "DELETE", &format!("/v2/domainnames/{domain}"), None).await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteDomainName answered {status} {deleted}"
    );

    // The port is closed, so a client dialling the published address is refused. A port the
    // operating system has not yet released may still accept the connection, in which case
    // the request on it must not be answered by any api.
    let port = authority
        .parse::<std::net::SocketAddr>()
        .expect("the published address is an address")
        .port();
    match TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await {
        Err(_refused) => {}
        Ok(mut stream) => {
            let wire = format!("GET /items HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(wire.as_bytes()).await;
            let mut raw = Vec::new();
            let read = tokio::time::timeout(ANSWER_DEADLINE, stream.read_to_end(&mut raw)).await;
            let answered = read
                .map(|_| String::from_utf8_lossy(&raw).to_string())
                .unwrap_or_default();
            assert!(
                !answered.contains("200"),
                "case {case:?}: a deleted domain name's address must serve no api, got \
                 {answered:?}"
            );
        }
    }
}
