// SPDX-License-Identifier: CC0-1.0

//! What a client that is handed `AcmeEndpoint$EndpointUrl` RECEIVES, field name by field
//! name, on the address the control plane publishes.
//!
//! RULE: the published ACME address is dialled verbatim by a stock ACME client (certbot,
//! lego, acme.sh, the python `acme` library), and such a client takes the values it needs
//! out of the answers' HTTP FIELDS — read under the names they were served with. So every
//! field the answers at that address carry is written in the canonical spelling its
//! specification states: the anti-replay nonce under `Replay-Nonce` (RFC 8555 §7.2),
//! required on BOTH the `200` to `HEAD <newNonce>` and the `204` to `GET <newNonce>`, the
//! account URL under `Location` (§7.3), the resource map under `Link` (§7.1), the
//! no-store directive under `Cache-Control` (§7.2) and the media type under
//! `Content-Type`. The nonce VALUE is per request and is no part of that requirement —
//! what is pinned here is the name it arrives under, and that the value a client reads by
//! that name is a nonce the server then accepts.
//!
//! Everything is read off a REAL socket: the address comes from what
//! `DescribeAcmeEndpoint` reported, it is dialled as it stands with no trust material
//! configured, and the answers are read as bytes — because a field name is decided by
//! whatever writes the response, so no in-process assertion can observe it.
//!
//! The rows span what the rule generalises over: the resource (the directory and the
//! nonce resource), the method it is addressed with (`GET`, `HEAD`, and the JWS `POST`),
//! and therefore the three statuses this surface answers a client with (`200`, `204`,
//! `201`) — plus, for the address itself, what it is NOT the address of.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::acme_account_registration::{external_account_binding, jws, AccountKey, Identification};
use crate::acme_directory_endpoint::{activate_endpoints_now, create_endpoint, split_url};
use crate::acme_external_account_binding_lifecycle::create_binding;
use crate::certificate_lifecycle::{call, router_with_engine, ACCOUNT, REGION};

/// The `Host` the control-plane calls below address, which is what a caller on the machine
/// this emulator runs on sends: the published URL has to be an address such a caller can
/// reach.
const REACHED: &str = "127.0.0.1:9297";

/// The media type an ACME JWS travels under (RFC 8555 §6.2).
const JOSE_CONTENT_TYPE: &str = "application/jose+json";

/// Call one operation signed for [`REGION`] and addressed to [`REACHED`], returning
/// `(status, body)`.
async fn call_at(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", REACHED)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CertificateManager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/acm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(::bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The `EndpointUrl` `DescribeAcmeEndpoint` publishes for `arn` to a caller that reached
/// this emulator at [`REACHED`].
async fn published_url(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let (status, response) =
        call_at(router, "DescribeAcmeEndpoint", &json!({ "AcmeEndpointArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeEndpoint must succeed, got {status} {response}"
    );
    response["AcmeEndpoint"]["EndpointUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no EndpointUrl in {response}"))
        .to_string()
}

/// One answer from the published address: its status line and head as written, and its
/// body.
struct Served {
    status: u16,
    head: String,
    body: String,
}

impl Served {
    /// The value this answer carries for `name`, matched EXACTLY as a client that reads
    /// the names it was served does — which is the whole point of this suite — and `None`
    /// when no field of that name was written.
    fn served(&self, name: &str) -> Option<String> {
        self.head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .find(|(field, _)| *field == name)
            .map(|(_, value)| value.trim().to_string())
    }

    /// Every field name this answer wrote, in the order it wrote them.
    fn names(&self) -> Vec<String> {
        self.head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(field, _)| field.to_string())
            .collect()
    }

    /// The body as JSON, or `Value::Null` for an empty one.
    fn json(&self, case: &str) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("case {case:?}: expected JSON, got {error}: {:?}", self.body))
    }
}

/// Dial the published `authority` as it stands — one TCP connection to this machine's own
/// loopback address, no trust material, exactly what a client holding a published URL does —
/// send `request` verbatim, and read the whole answer back.
///
/// The answer is read by its own framing: the head up to the blank line, then exactly the
/// body the head states a length for, and to the end of the stream when it states none (the
/// requests below ask for the connection to be closed). Nothing here waits on a clock.
async fn exchanged(case: &str, authority: &str, request: &str) -> Served {
    /// The blank line that ends a response head (RFC 9112 §2.1).
    const HEAD_TERMINATOR: &str = "\r\n\r\n";

    let port: u16 = authority
        .rsplit_once(':')
        .and_then(|(_host, port)| port.parse().ok())
        .unwrap_or_else(|| panic!("case {case:?}: the published authority names no port: {authority}"));
    let mut socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the published address {authority} must accept a connection: {error}")
        });
    socket
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the request must be sent: {error}"));

    let mut answered = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let text = String::from_utf8_lossy(&answered).to_string();
        if let Some((head, body)) = text.split_once(HEAD_TERMINATOR) {
            // Complete once the body the head states a length for has arrived; a head that
            // states none is framed by the end of the stream.
            let stated = head
                .split("\r\n")
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok());
            if stated.is_some_and(|length| body.len() >= length) {
                break;
            }
        }
        let read = socket
            .read(&mut chunk)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the answer must be read: {error}"));
        if read == 0 {
            break;
        }
        answered.extend_from_slice(&chunk[..read]);
    }

    let answered = String::from_utf8_lossy(&answered).to_string();
    let (head, body) = answered
        .split_once(HEAD_TERMINATOR)
        .unwrap_or_else(|| panic!("case {case:?}: expected an HTTP response, got {answered:?}"));
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a status line, got {head:?}"));
    Served {
        status,
        head: head.to_string(),
        body: body.to_string(),
    }
}

/// One well-formed request to the published address: the method, the path, the `Host` the
/// published URL names, and a body with its media type when there is one.
async fn dialled(
    case: &str,
    authority: &str,
    method: &str,
    path: &str,
    body: Option<(&str, &str)>,
) -> Served {
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    if let Some((content_type, payload)) = body {
        request.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            payload.len()
        ));
    }
    request.push_str("\r\n");
    if let Some((_, payload)) = body {
        request.push_str(payload);
    }
    exchanged(case, authority, &request).await
}

/// The `host:port` and the path of a published `https://` URL, with the authority asserted
/// to be an address on this machine — the only address a client here can reach. Loopback is
/// read through the core's own predicate, so the assertion holds under any loopback bind.
fn address(case: &str, url: &str) -> (String, String) {
    let (authority, path) = split_url(case, url);
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: the published authority names no port: {authority}"));
    assert!(
        emulator_server::is_loopback_host(host),
        "case {case:?}: the published ACME address must be this machine's own, got {authority:?}"
    );
    assert!(
        port.parse::<u16>().is_ok_and(|port| port > 0),
        "case {case:?}: the published ACME address must name a port, got {authority:?}"
    );
    (authority, path)
}

/// Every field name an answer wrote is in its canonical spelling: each hyphen-separated
/// part begins with a capital, which is how the specifications write every field of this
/// surface.
fn assert_canonical_names(case: &str, served: &Served) {
    for name in served.names() {
        assert!(
            name.split('-').all(|part| {
                part.chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_uppercase())
            }),
            "case {case:?}: every field name must be served in its canonical spelling, got \
             {name:?} in {:?}",
            served.head
        );
    }
}

/// RULE: the published address is the address of ONE plane, and the transport checks a core
/// HTTP listener applies are applied here too — so a request that addresses something else,
/// names a host this address does not serve, or asks for a protocol it does not speak is
/// refused by this address itself rather than forwarded.
///
/// The rows span what that generalises over: the path addressed (the control plane's own
/// root, another data plane's namespace, a traversal out of this plane's prefix), the `Host`
/// (absent, and a routable name a browser could be made to address), and an upgrade to
/// another protocol.
#[tokio::test]
async fn the_published_address_refuses_what_it_is_not_the_address_of() {
    let (router, engine) = router_with_engine();
    let case = "a client holding the published ACME address";
    let arn = create_endpoint(&router, case, REGION, "refusal-marker").await;
    activate_endpoints_now(&engine);
    let url = published_url(&router, case, &arn).await;
    let (authority, path) = address(case, &url);

    let rows: Vec<(&str, String, u16)> = vec![
        (
            "the control plane's own root, which this address does not serve",
            format!("POST / HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"),
            404,
        ),
        (
            "another data plane's namespace",
            format!(
                "GET /_aws/execute-api/anything HTTP/1.1\r\nHost: {authority}\r\n\
                 Connection: close\r\n\r\n"
            ),
            404,
        ),
        (
            "a traversal out of this plane's prefix",
            format!(
                "GET /_aws/acm-acme-enroll/../../etc HTTP/1.1\r\nHost: {authority}\r\n\
                 Connection: close\r\n\r\n"
            ),
            400,
        ),
        (
            "a host this address does not serve",
            format!("GET {path} HTTP/1.1\r\nHost: attacker.example.com\r\nConnection: close\r\n\r\n"),
            421,
        ),
        (
            "an upgrade to WebSocket, which this address does not speak",
            format!(
                "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: Upgrade\r\n\
                 Upgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 Sec-WebSocket-Version: 13\r\n\r\n"
            ),
            501,
        ),
    ];
    for (row, request, expected) in rows {
        let served = exchanged(row, &authority, &request).await;
        assert_eq!(
            served.status, expected,
            "case {row:?}: expected HTTP {expected} from the published address, got {} {:?}",
            served.status, served.head
        );
        assert_canonical_names(row, &served);
    }

    // And the plane it IS the address of is still served, so the refusals above are not
    // this address refusing everything.
    let served = dialled(case, &authority, "GET", &path, None).await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: the published URL must still serve the directory, got {} {:?}",
        served.status, served.head
    );
}

/// RULE: the answers at the published ACME address carry their field names in the
/// canonical spelling their specification states — the nonce under `Replay-Nonce` on both
/// the `200` to `HEAD` and the `204` to `GET`, with a fresh value every time.
#[tokio::test]
async fn the_published_acme_address_serves_its_fields_under_the_names_the_rfc_states() {
    let (router, engine) = router_with_engine();
    let case = "a client holding the published ACME address";
    let arn = create_endpoint(&router, case, REGION, "field-name-marker").await;
    activate_endpoints_now(&engine);

    let url = published_url(&router, case, &arn).await;
    let (authority, path) = address(case, &url);

    // The directory, fetched exactly as a client holding the URL fetches it.
    let directory = dialled(case, &authority, "GET", &path, None).await;
    assert_eq!(
        directory.status, 200,
        "case {case:?}: the published URL must serve the directory, got {} {:?}",
        directory.status, directory.body
    );
    assert_eq!(
        directory.served("Content-Type").as_deref(),
        Some("application/json"),
        "case {case:?}: expected the media type under Content-Type, got {:?}",
        directory.head
    );
    assert_canonical_names(case, &directory);
    let document = directory.json(case);
    let nonce_url = document["newNonce"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no newNonce in {document}"))
        .to_string();
    let (nonce_authority, nonce_path) = address(case, &nonce_url);
    assert_eq!(
        nonce_authority, authority,
        "case {case:?}: the directory must name its resources on the address it was fetched \
         from, got {nonce_url:?}"
    );

    // The rows: the two ways RFC 8555 §7.2 says the nonce resource is addressed, and the
    // status each answers with.
    let rows: &[(&str, &str, u16)] = &[
        ("HEAD, which RFC 8555 §7.2 answers 200", "HEAD", 200),
        ("GET, which RFC 8555 §7.2 answers 204", "GET", 204),
    ];
    // Every nonce handed out, so freshness is asserted across the whole run.
    let mut issued: BTreeSet<String> = BTreeSet::new();
    for (row, method, expected) in rows {
        // Twice per row: a nonce is fresh per REQUEST, not per method.
        for attempt in 0..2 {
            let case = format!("{row} (attempt {attempt})");
            let served = dialled(&case, &authority, method, &nonce_path, None).await;
            assert_eq!(
                served.status, *expected,
                "case {case:?}: expected HTTP {expected}, got {} {:?}",
                served.status, served.head
            );
            assert_canonical_names(&case, &served);
            let nonce = served.served("Replay-Nonce").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a field named Replay-Nonce, got {:?}",
                    served.head
                )
            });
            assert!(
                !nonce.is_empty(),
                "case {case:?}: the nonce field must carry a nonce, got {:?}",
                served.head
            );
            assert!(
                issued.insert(nonce.clone()),
                "case {case:?}: every answer must carry a fresh nonce, but {nonce:?} was \
                 already issued"
            );
            assert_eq!(
                served.served("Cache-Control").as_deref(),
                Some("no-store"),
                "case {case:?}: expected the no-store directive under Cache-Control, got {:?}",
                served.head
            );
            assert!(
                served
                    .served("Link")
                    .is_some_and(|link| link.contains(&path) && link.contains("rel=\"index\"")),
                "case {case:?}: expected the directory under Link, got {:?}",
                served.head
            );
            assert!(
                served.body.is_empty(),
                "case {case:?}: the nonce resource carries no body, got {:?}",
                served.body
            );
        }
    }
    assert_eq!(
        issued.len(),
        rows.len() * 2,
        "expected one fresh nonce per request, got {issued:?}"
    );
}

/// RULE, to the end of the account lifecycle the nonce unlocks: the value a client reads
/// under `Replay-Nonce` is a nonce this endpoint accepts, so a registration signed with it
/// is admitted at the published address — `201` with the account URL under `Location` —
/// and the account it created is the one the control plane reports, revokes, and still
/// reports as revoked when the revocation is repeated.
#[tokio::test]
async fn a_nonce_read_under_that_name_registers_an_account_at_the_published_address() {
    let (router, engine) = router_with_engine();
    let case = "a client that registers through the published address";
    let endpoint_arn = create_endpoint(&router, case, REGION, "registration-marker").await;
    activate_endpoints_now(&engine);

    let url = published_url(&router, case, &endpoint_arn).await;
    let (authority, path) = address(case, &url);
    let directory = dialled(case, &authority, "GET", &path, None).await;
    let document = directory.json(case);
    let new_account = document["newAccount"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no newAccount in {document}"))
        .to_string();
    let nonce_url = document["newNonce"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no newNonce in {document}"))
        .to_string();

    // The external account binding this endpoint registers accounts with, and the
    // credentials a client signs it with — both reported by the control plane.
    let binding = create_binding(
        &router,
        case,
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/AcmeIssuer"),
        }),
    )
    .await;
    let binding_arn = binding["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the binding carried no ARN"))
        .to_string();
    let (status, credentials) = call(
        &router,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the binding's credentials must be reported, got {status} {credentials}"
    );
    let key_id = credentials["KeyId"].as_str().unwrap_or_default().to_string();
    let mac_key = credentials["MacKey"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The nonce is taken from the field name RFC 8555 fixes, and from nothing else: a
    // client has no other way to find it.
    let (_, nonce_path) = address(case, &nonce_url);
    let served = dialled(case, &authority, "HEAD", &nonce_path, None).await;
    let nonce = served.served("Replay-Nonce").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a field named Replay-Nonce, got {:?}",
            served.head
        )
    });

    let key = AccountKey::ecdsa("ES256");
    let payload = json!({
        "termsOfServiceAgreed": true,
        "contact": ["mailto:published@example.com"],
        "externalAccountBinding":
            external_account_binding(&key_id, &mac_key, &new_account, &key.jwk()),
    });
    let signed = jws(
        &key,
        &Identification::Jwk,
        &nonce,
        &new_account,
        Some(&payload),
    )
    .to_string();
    let (_, account_path) = address(case, &new_account);
    let registered = dialled(
        case,
        &authority,
        "POST",
        &account_path,
        Some((JOSE_CONTENT_TYPE, &signed)),
    )
    .await;
    assert_eq!(
        registered.status, 201,
        "case {case:?}: the registration must be accepted, got {} {:?}",
        registered.status, registered.body
    );
    assert_canonical_names(case, &registered);
    let account_url = registered.served("Location").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the account URL under Location, got {:?}",
            registered.head
        )
    });
    assert_eq!(
        address(case, &account_url).0,
        authority,
        "case {case:?}: the account URL must be on the address the client is talking to, got \
         {account_url:?}"
    );
    assert!(
        registered
            .served("Replay-Nonce")
            .is_some_and(|fresh| fresh != nonce),
        "case {case:?}: every answer on this wire carries a fresh nonce, got {:?}",
        registered.head
    );

    // The control plane resolves that URL: the account it names is the one registered
    // through the published address, revoking it is observable through the same handle, and
    // a repeated revocation leaves it revoked rather than refusing.
    reported_status(
        &router,
        case,
        "after the registration",
        &endpoint_arn,
        &account_url,
        "VALID",
    )
    .await;
    for attempt in 0..2 {
        let (status, response) = call_at(
            &router,
            "RevokeAcmeAccount",
            &json!({ "AcmeEndpointArn": endpoint_arn, "AccountUrl": account_url }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: RevokeAcmeAccount #{attempt} must succeed, got {status} {response}"
        );
    }
    reported_status(
        &router,
        case,
        "after the revocation",
        &endpoint_arn,
        &account_url,
        "REVOKED",
    )
    .await;
}

/// Assert `DescribeAcmeAccount` resolves `account_url` on `endpoint_arn` and reports
/// `expected` as its status.
async fn reported_status(
    router: &MultiServiceRouter,
    case: &str,
    step: &str,
    endpoint_arn: &str,
    account_url: &str,
    expected: &str,
) {
    let (status, response) = call_at(
        router,
        "DescribeAcmeAccount",
        &json!({ "AcmeEndpointArn": endpoint_arn, "AccountUrl": account_url }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} {step}: DescribeAcmeAccount must resolve the account URL, got {status} \
         {response}"
    );
    assert_eq!(
        response["AcmeAccount"]["Status"].as_str(),
        Some(expected),
        "case {case:?} {step}: expected {expected}, got {response}"
    );
}
