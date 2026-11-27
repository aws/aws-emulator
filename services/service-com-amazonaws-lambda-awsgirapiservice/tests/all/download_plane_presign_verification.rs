// SPDX-License-Identifier: CC0-1.0

//! What AUTHORIZES a fetch of the download plane behind `Code$Location` and
//! `Content$Location`, on an emulator that verifies request signatures.
//!
//! RULE: an address an operation hands the caller to fetch later is a PRESIGNED one — the
//! signature travels in its query string rather than in an `Authorization` header — so the
//! plane serving it declares no scheme of its own and the emulator holds a fetch to that
//! signature: the address exactly as it was announced serves the object, and an address
//! whose signature is absent or does not verify over the request presented serves nothing.
//! A path alone is never a credential, however unguessable it looks.
//!
//! The rows differ along what the rule generalises over: WHICH object is addressed (a
//! function's deployment package and a layer version's archive, the two objects this plane
//! publishes), and HOW the presented address differs from the one announced (the whole query
//! dropped, the signature field dropped, one digit of the signature rewritten, the signed
//! window widened, the signature presented over the OTHER object's path). The controls are
//! each address exactly as announced, which must serve the stored bytes byte for byte.
//!
//! Every row drives the production `MultiServiceRouter` with request authentication ON — the
//! `--auth on` path a developer runs the emulator on — and follows the announced address the
//! way a client follows a presigned URL: no credential of any kind beyond the one the URL
//! itself carries.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The control-plane endpoint every call below is addressed to, and so the host every
/// announced address is signed over.
const SERVICE_HOST: &str = "lambda.us-east-1.amazonaws.com";

/// The base URL this emulator is reached at, which every announced address leads back to.
const BASE: &str = "https://lambda.us-east-1.amazonaws.com";

/// The Region every call is signed for, which the endpoint above names.
const REGION: &str = "us-east-1";

/// The signing name the emulator verifies a request to this service under.
const SIGNING_NAME: &str = "lambda";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The query field a presigned address carries its signature in.
const SIGNATURE_FIELD: &str = "X-Amz-Signature";

/// The query field a presigned address states its signed window in.
const EXPIRES_FIELD: &str = "X-Amz-Expires";

/// What a dispatched request answered.
struct Answered {
    status: u16,
    body: bytes::Bytes,
}

impl Answered {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn summary(&self) -> String {
        format!("{} {}", self.status, String::from_utf8_lossy(&self.body))
    }
}

/// One service on its own engine, on a router that VERIFIES every request signature.
fn authenticating_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    router.set_url_base(
        smithy_server_runtime::EndpointUrl::parse(BASE).expect("the test base URL must parse"),
    );
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// The keys this emulator process publishes, which a caller signs with and the emulator
/// verifies against.
fn published_keys() -> (String, String) {
    let keys = ::emulator_server::request_authentication::SigV4Authenticator::for_this_process();
    (keys.access_key_id.clone(), keys.secret_access_key.clone())
}

/// One CONTROL-plane call, SigV4-signed with those keys by the workspace's own signer.
async fn control(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answered {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{SERVICE_HOST}{uri}"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request must build");
    let (key_id, secret) = published_keys();
    ::service_common::in_process_sigv4::sign_in_process_request_as(
        &mut request,
        SIGNING_NAME,
        REGION,
        &key_id,
        &secret,
    )
    .await
    .expect("the test request must sign");
    let (parts, body) = request.into_parts();
    let response = router
        .dispatch(http::Request::from_parts(parts, bytes::Bytes::from(body)))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// Follow an announced address the way a client follows a presigned URL: at the address it
/// names, carrying no credential of its own.
async fn follow(router: &MultiServiceRouter, method: &str, url: &str) -> Answered {
    let host = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .unwrap_or_default()
        .to_string();
    let request = http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("the download request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// Deployment package / archive bytes of a given length, differing per `seed` so two
/// objects never carry the same content.
fn object_bytes(seed: usize, length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from((index * 7 + seed * 53 + 11) % 251).unwrap_or_default())
        .collect()
}

/// A base64 blob member, as restJson1 carries one in a JSON body.
fn blob(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The member of an answer at `pointer`, as the answered string.
fn member(case: &str, answered: &Answered, pointer: &str) -> String {
    answered
        .json()
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a location at {pointer}, got {}",
                answered.summary()
            )
        })
}

/// The path of a URL, with the query behind it.
fn target_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    rest.find('/')
        .map_or_else(|| "/".to_string(), |index| rest[index..].to_string())
}

/// One query field of a presented address, exactly as it was written.
fn field(url: &str, name: &str) -> String {
    target_of(url)
        .split_once('?')
        .map(|(_path, query)| query.to_string())
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('=').map(|(f, v)| (f.to_string(), v.to_string())))
        .find_map(|(f, value)| (f == name).then_some(value))
        .unwrap_or_default()
}

/// `url` with query field `name` set to `value`, every other pair left byte-identical.
fn with_field(url: &str, name: &str, value: &str) -> String {
    rewritten(url, |field, written| {
        if field == name {
            format!("{field}={value}")
        } else {
            written.to_string()
        }
    })
}

/// `url` with query field `name` removed altogether.
fn without_field(url: &str, name: &str) -> String {
    let target = target_of(url);
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| pair.split_once('=').map(|(f, _)| f) != Some(name))
        .collect();
    format!("{path}?{}", kept.join("&"))
}

/// `url`'s request target with each query pair put through `rewrite`.
fn rewritten(url: &str, rewrite: impl Fn(&str, &str) -> String) -> String {
    let target = target_of(url);
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| {
            let field = pair.split_once('=').map_or(pair, |(f, _)| f);
            rewrite(field, pair)
        })
        .collect();
    format!("{path}?{}", pairs.join("&"))
}

/// One hex digit rewritten to another: `0` becomes `1`, anything else becomes `0`.
fn flip(digit: char) -> char {
    if digit == '0' {
        '1'
    } else {
        '0'
    }
}

/// `url`'s signature with its first digit rewritten: still a 64-character hex signature,
/// and not the signature of this address.
fn flipped_signature(url: &str) -> String {
    field(url, SIGNATURE_FIELD)
        .chars()
        .enumerate()
        .map(|(index, digit)| if index == 0 { flip(digit) } else { digit })
        .collect()
}

/// The announced object under test: what it is, the address announced for it, and the bytes
/// the store holds for it.
struct Announced {
    case: &'static str,
    url: String,
    bytes: Vec<u8>,
}

/// Create a function with an inline package and publish a layer version, and answer the two
/// announced download addresses with the bytes each has to serve.
async fn announced_objects(router: &MultiServiceRouter) -> Vec<Announced> {
    let package = object_bytes(1, 512);
    let created = control(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": "download-presign",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": blob(&package) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction must succeed, got {}",
        created.summary()
    );
    let read = control(
        router,
        "GET",
        "/2015-03-31/functions/download-presign",
        &Value::Null,
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "GetFunction must succeed, got {}",
        read.summary()
    );

    let archive = object_bytes(2, 333);
    let published = control(
        router,
        "POST",
        "/2018-10-31/layers/download-presign-layer/versions",
        &json!({ "Content": { "ZipFile": blob(&archive) } }),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "PublishLayerVersion must succeed, got {}",
        published.summary()
    );

    vec![
        Announced {
            case: "a function's deployment package",
            url: member("a function's deployment package", &read, "/Code/Location"),
            bytes: package,
        },
        Announced {
            case: "a layer version's archive",
            url: member(
                "a layer version's archive",
                &published,
                "/Content/Location",
            ),
            bytes: archive,
        },
    ]
}

/// The address exactly as announced serves the stored object; an address whose signature is
/// absent or does not verify over the request presented serves nothing.
#[tokio::test]
async fn a_download_is_served_only_to_the_presigned_address_it_was_announced_at() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = authenticating_router(&engine);
    let objects = announced_objects(&router).await;

    for object in &objects {
        let case = object.case;
        let url = &object.url;
        assert!(
            !field(url, SIGNATURE_FIELD).is_empty(),
            "case {case:?}: the announced address must be presigned, got {url:?}"
        );

        // The control: the address as announced, which serves the object's own bytes.
        let served = follow(&router, "GET", url).await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: the announced address must serve the object, got {}",
            served.summary()
        );
        assert_eq!(
            served.body.as_ref(),
            object.bytes.as_slice(),
            "case {case:?}: expected the {} stored bytes, got {}",
            object.bytes.len(),
            served.body.len()
        );

        // Every other object's address, with this object's signature on it: the signature
        // covers the path, so it authorises nothing else.
        let other = objects
            .iter()
            .find(|candidate| candidate.case != case)
            .expect("the suite announces two objects");
        let other_path = target_of(&other.url)
            .split_once('?')
            .map(|(path, _query)| path.to_string())
            .unwrap_or_default();
        let retargeted = format!(
            "{other_path}?{}",
            target_of(url).split_once('?').map_or("", |(_, query)| query)
        );

        let presented: Vec<(String, String)> = vec![
            (
                "the whole query dropped".to_string(),
                target_of(url)
                    .split_once('?')
                    .map(|(path, _)| path.to_string())
                    .unwrap_or_default(),
            ),
            (
                "the signature field dropped".to_string(),
                without_field(url, SIGNATURE_FIELD),
            ),
            (
                "one digit of the signature rewritten".to_string(),
                with_field(url, SIGNATURE_FIELD, &flipped_signature(url)),
            ),
            (
                "the signed window widened".to_string(),
                with_field(url, EXPIRES_FIELD, "604800"),
            ),
            (
                "this signature over the other object's address".to_string(),
                retargeted,
            ),
        ];

        for (how, address) in presented {
            let refused = follow(&router, "GET", &address).await;
            assert_eq!(
                refused.status,
                403,
                "case {case:?} with {how:?}: expected HTTP 403, got {}",
                refused.summary()
            );
            assert_ne!(
                refused.body.as_ref(),
                object.bytes.as_slice(),
                "case {case:?} with {how:?}: a refused fetch serves no stored byte"
            );
            assert_ne!(
                refused.body.as_ref(),
                other.bytes.as_slice(),
                "case {case:?} with {how:?}: a refused fetch serves no stored byte"
            );
        }
    }
}
