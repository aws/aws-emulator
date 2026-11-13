// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the download locations this service ANNOUNCES: a layer version's
//! `Content.Location` and a function's `Code.Location`.
//!
//! The RULE asserted here, over a table of announced locations rather than one: **a URL an
//! operation hands a caller to fetch later is addressed to the base URL this emulator is
//! reached on, SigV4 query-presigned by the emulator, and following it serves exactly the
//! bytes the store holds for the object it names.** The emulator answers on its own
//! endpoint alone, so an address on any other host — a production AWS hostname among them —
//! is one no client of it can fetch anything from. The object's own S3 address (the
//! per-region bucket Lambda keeps these objects in, and the key under it) travels in the
//! path, which is what makes two different objects two different locations.
//!
//! The rows differ along what the rule generalises over: which operation announces the
//! location (`PublishLayerVersion`, `GetLayerVersion`, `GetLayerVersionByArn`,
//! `GetFunction` unqualified and at a published version), which store the object is held in
//! (a layer archive, a deployment package), where the bytes came from (an inline archive, an
//! Amazon S3 object that is DELETED afterwards), the size of the object, and which version
//! of a layer is addressed.
//!
//! A second table drives the SAME publish over emulators reached at different base URLs,
//! because the announced address has to follow the endpoint rather than be a constant.
//!
//! Amazon S3 is mounted beside Lambda on one engine (as `core/router/src/lib.rs` mounts
//! them), so an archive taken from an S3 object is read out of that store through S3's own
//! surface. The Lambda-side harness (`Answer`, `json_call`, `package_bytes`) is the one
//! `function_lifecycle.rs` publishes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

use crate::function_lifecycle::{
    create_active_function, json_call, make_activation_due, package_bytes, zip_file, Answer,
    ACCOUNT, REGION,
};

/// The expiry every announced location carries, in seconds: the model documents a layer
/// archive's link as "A link to the layer archive in Amazon S3 that is valid for 10
/// minutes" (`smithy_query(query="doc", shape="LayerVersionContentOutput",
/// member="Location")`), and AWS documents the same span for the presigned URL a deployment
/// package is downloaded from.
const EXPIRES_SECONDS: &str = "600";

/// The algorithm a SigV4 query-presigned URL declares.
const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The bucket label of the per-region store a LAYER archive is served from.
const BUCKET_LAYERS: &str = "layers";

/// The bucket label of the per-region store a deployment PACKAGE is served from.
const BUCKET_TASKS: &str = "tasks";

/// The Amazon S3 bucket this suite publishes a layer archive out of.
const SOURCE_BUCKET: &str = "layer-archives";

/// A base URL an emulator in this suite is reached on, which no announced location may
/// disagree with.
const BASE: &str = "https://emulator.test:8443";

/// Lambda and Amazon S3 on one engine, reached at `base`: an archive named by an Amazon S3
/// location is read out of S3's own store, and `base` is the endpoint every URL this
/// emulator announces has to lead back to.
fn emulator_at(engine: &Arc<engine::InMemoryEngine>, base: &str) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.set_url_base(EndpointUrl::parse(base).expect("the test base URL must parse"));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(engine),
    ));
    router
}

/// A SigV4 credential scope naming Amazon S3 in the Region the Lambda harness signs for.
/// The signature is never checked: the router has request authentication off.
fn s3_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Amazon S3 request that must be served, through S3's own surface.
async fn s3_ok(router: &MultiServiceRouter, case: &str, method: &str, uri: &str, body: &[u8]) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host.clone())
        .header("authorization", s3_authorization())
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    assert!(
        (200..300).contains(&parts.status.as_u16()),
        "case {case:?}: S3 {method} {uri} must succeed, got {} {}",
        parts.status.as_u16(),
        String::from_utf8_lossy(&answered)
    );
}

/// `PublishLayerVersion` on a layer name.
async fn publish_layer_version(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `GetLayerVersion` on a layer name and a version number.
async fn get_layer_version(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetLayerVersionByArn`, whose `Arn` member is bound to the `Arn` query parameter of
/// `GET /2018-10-31/layers?find=LayerVersion`.
async fn get_layer_version_by_arn(router: &MultiServiceRouter, arn: &str) -> Answer {
    let encoded = arn.replace(':', "%3A");
    let uri = format!("/2018-10-31/layers?find=LayerVersion&Arn={encoded}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetFunction` on a function name, optionally at one qualifier.
async fn get_function(
    router: &MultiServiceRouter,
    name: &str,
    qualifier: Option<&str>,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let uri = format!("/2015-03-31/functions/{name}{query}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `PublishVersion` on a function name.
async fn publish_version(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/versions");
    json_call(router, "POST", &uri, &Value::Null).await
}

/// The location member of an answer, by JSON pointer, as the answered string.
fn location_of(case: &str, answer: &Answer, pointer: &str) -> String {
    assert!(
        (200..300).contains(&answer.status),
        "case {case:?}: the read announcing the location must succeed, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a location at {pointer}, got {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
        .to_string()
}

/// Fetch a URL this service ANNOUNCED, the way a client follows a presigned URL: at the
/// address the URL names, carrying no SigV4 `Authorization` header at all.
async fn fetch(router: &MultiServiceRouter, method: &str, url: &str) -> Answer {
    let host = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .unwrap_or_default()
        .to_string();
    let request = ::http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("the download request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The path of a URL, and the query behind it.
fn path_and_query(url: &str) -> (String, String) {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let from_path = rest.find('/').map_or("/", |index| &rest[index..]);
    match from_path.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (from_path.to_string(), String::new()),
    }
}

/// One query field of a presigned URL, decoded enough to read a credential scope.
fn query_field(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (field, value) = pair.split_once('=')?;
        (field == name).then(|| value.replace("%2F", "/"))
    })
}

/// The JSON pointers a read carries an announced download link at: a layer version's
/// `Content.Location` and a function's `Code.Location`.
pub(crate) const ANNOUNCED_LINK_POINTERS: [&str; 2] = ["/Content/Location", "/Code/Location"];

/// What an announced download link says about the OBJECT it addresses and on what TERMS,
/// with the signing instant of the read that announced it — and the signature over that
/// instant — set aside.
///
/// A link is "valid for 10 minutes" from the read that announced it, so every read presigns
/// one afresh: `X-Amz-Date`, the signing date in the credential scope and `X-Amz-Signature`
/// belong to that read, while the address the object is held at, the algorithm, the expiry
/// and the Region and service the scope names come from stored state. Two reads of one
/// stored resource therefore agree on this reduction and need not agree on the link's text,
/// which is why a comparison of two reads is made over this rather than over the URL: the
/// URL answers by which second each read was served in, that is, by how fast the host runs.
///
/// Asserts along the way that the link really is presigned: a non-empty instant, a
/// non-empty signature and a scope carrying a signing date — what is being set aside.
pub(crate) fn addressed_object(case: &str, location: &Value) -> String {
    let url = location
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a location string, got {location}"));
    let (address, query) = url.split_once('?').unwrap_or((url, ""));
    let mut terms: Vec<String> = Vec::new();
    let mut carries_instant = false;
    let mut carries_signature = false;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (field, value) = pair.split_once('=').unwrap_or((pair, ""));
        match field {
            "X-Amz-Date" | "X-Amz-Signature" => {
                assert!(
                    !value.is_empty(),
                    "case {case:?}: expected a presigned location, got an empty {field} in \
                     {url}"
                );
                carries_instant |= field == "X-Amz-Date";
                carries_signature |= field == "X-Amz-Signature";
            }
            // `<key id>/<signing date>/<region>/<service>/aws4_request`: the date is the
            // read's, the rest is not.
            "X-Amz-Credential" => {
                let scope = value.replace("%2F", "/");
                let mut segments = scope.split('/');
                let key_id = segments.next().unwrap_or_default().to_string();
                let signing_date = segments.next().unwrap_or_default();
                assert!(
                    !signing_date.is_empty(),
                    "case {case:?}: expected a credential scope carrying a signing date, got \
                     {scope:?}"
                );
                let rest: Vec<&str> = segments.collect();
                terms.push(format!("{field}={key_id}/{}", rest.join("/")));
            }
            _ => terms.push(format!("{field}={value}")),
        }
    }
    assert!(
        carries_instant && carries_signature,
        "case {case:?}: expected a presigned location carrying an instant and a signature, \
         got {url}"
    );
    terms.sort();
    format!("{address}?{}", terms.join("&"))
}

/// One read's answer as the RECORD it reports: every announced download link it carries is
/// reduced to the object it addresses ([`addressed_object`]), so comparing two answers
/// compares what the store holds rather than which second each read was served in.
pub(crate) fn record(case: &str, body: &Value) -> Value {
    let mut body = body.clone();
    let announced: Vec<(&str, String)> = ANNOUNCED_LINK_POINTERS
        .iter()
        .filter_map(|pointer| {
            let location = body.pointer(pointer)?;
            Some((*pointer, addressed_object(case, location)))
        })
        .collect();
    assert!(
        !announced.is_empty(),
        "case {case:?}: expected a read carrying an announced download link, got {body}"
    );
    for (pointer, object) in announced {
        if let Some(slot) = body.pointer_mut(pointer) {
            *slot = Value::String(object);
        }
    }
    body
}

/// One announced download location under test.
struct Announced {
    /// The case name, which every assertion reports.
    case: String,
    /// The announced URL.
    url: String,
    /// The bucket label the object is held in.
    bucket: &'static str,
    /// What the object's key starts with: the resource the location addresses.
    key_prefix: String,
    /// The bytes the store holds for the object.
    expected: Vec<u8>,
}

/// Every announced location is addressed to the emulator's own base URL, is presigned, and
/// serves the bytes the store holds for the object it names.
#[tokio::test]
async fn an_announced_download_location_is_presigned_on_this_emulators_own_endpoint() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_at(&engine, BASE);

    // A layer with two versions, published from inline archives of different sizes: the
    // newest version's location has to serve the newest archive.
    let first = package_bytes(96);
    let second = package_bytes(201);
    let inline = publish_layer_version(
        &router,
        "lyr-inline",
        &json!({ "Content": { "ZipFile": zip_file(&first) } }),
    )
    .await;
    let newest = publish_layer_version(
        &router,
        "lyr-inline",
        &json!({ "Content": { "ZipFile": zip_file(&second) } }),
    )
    .await;

    // A layer published out of an Amazon S3 object, whose SOURCE object is then deleted:
    // Lambda holds its own copy of the archive, so the announced location keeps serving it.
    let from_s3 = package_bytes(144);
    s3_ok(&router, "an S3-sourced archive", "PUT", &format!("/{SOURCE_BUCKET}"), &[]).await;
    s3_ok(
        &router,
        "an S3-sourced archive",
        "PUT",
        &format!("/{SOURCE_BUCKET}/archive.zip"),
        &from_s3,
    )
    .await;
    let copied = publish_layer_version(
        &router,
        "lyr-from-s3",
        &json!({ "Content": { "S3Bucket": SOURCE_BUCKET, "S3Key": "archive.zip" } }),
    )
    .await;
    s3_ok(
        &router,
        "an S3-sourced archive",
        "DELETE",
        &format!("/{SOURCE_BUCKET}/archive.zip"),
        &[],
    )
    .await;

    // A function's deployment package, and the package a published version froze.
    let package = package_bytes(128);
    create_active_function(&router, &engine, "downloaded-fn", &package).await;
    let published = publish_version(&router, "downloaded-fn").await;
    assert_eq!(
        published.status,
        201,
        "PublishVersion must succeed, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );
    make_activation_due(&engine);

    let version_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:lyr-inline:1");
    let rows = vec![
        Announced {
            case: "the publish of an inline archive announces it".to_string(),
            url: location_of("the publish of an inline archive announces it", &inline, "/Content/Location"),
            bucket: BUCKET_LAYERS,
            key_prefix: "lyr-inline-1".to_string(),
            expected: first.clone(),
        },
        Announced {
            case: "GetLayerVersion of that version".to_string(),
            url: location_of(
                "GetLayerVersion of that version",
                &get_layer_version(&router, "lyr-inline", 1).await,
                "/Content/Location",
            ),
            bucket: BUCKET_LAYERS,
            key_prefix: "lyr-inline-1".to_string(),
            expected: first.clone(),
        },
        Announced {
            case: "GetLayerVersionByArn of that version".to_string(),
            url: location_of(
                "GetLayerVersionByArn of that version",
                &get_layer_version_by_arn(&router, &version_arn).await,
                "/Content/Location",
            ),
            bucket: BUCKET_LAYERS,
            key_prefix: "lyr-inline-1".to_string(),
            expected: first.clone(),
        },
        Announced {
            case: "the newest version of the same layer".to_string(),
            url: location_of("the newest version of the same layer", &newest, "/Content/Location"),
            bucket: BUCKET_LAYERS,
            key_prefix: "lyr-inline-2".to_string(),
            expected: second.clone(),
        },
        Announced {
            case: "GetLayerVersion of the newest version".to_string(),
            url: location_of(
                "GetLayerVersion of the newest version",
                &get_layer_version(&router, "lyr-inline", 2).await,
                "/Content/Location",
            ),
            bucket: BUCKET_LAYERS,
            key_prefix: "lyr-inline-2".to_string(),
            expected: second.clone(),
        },
        Announced {
            case: "an archive whose source Amazon S3 object has been deleted".to_string(),
            url: location_of(
                "an archive whose source Amazon S3 object has been deleted",
                &get_layer_version(&router, "lyr-from-s3", 1).await,
                "/Content/Location",
            ),
            bucket: BUCKET_LAYERS,
            key_prefix: "lyr-from-s3-1".to_string(),
            expected: from_s3.clone(),
        },
        Announced {
            case: "the publish of an S3-sourced archive announces it".to_string(),
            url: location_of(
                "the publish of an S3-sourced archive announces it",
                &copied,
                "/Content/Location",
            ),
            bucket: BUCKET_LAYERS,
            key_prefix: "lyr-from-s3-1".to_string(),
            expected: from_s3.clone(),
        },
        Announced {
            case: "a function's deployment package".to_string(),
            url: location_of(
                "a function's deployment package",
                &get_function(&router, "downloaded-fn", None).await,
                "/Code/Location",
            ),
            bucket: BUCKET_TASKS,
            key_prefix: "downloaded-fn-".to_string(),
            expected: package.clone(),
        },
        Announced {
            case: "the package a published version froze".to_string(),
            url: location_of(
                "the package a published version froze",
                &get_function(&router, "downloaded-fn", Some("1")).await,
                "/Code/Location",
            ),
            bucket: BUCKET_TASKS,
            key_prefix: "downloaded-fn-".to_string(),
            expected: package.clone(),
        },
    ];

    for row in &rows {
        let case = &row.case;
        // The address is the emulator's own, and nothing of AWS's own hostnames is in it.
        assert!(
            row.url.starts_with(&format!("{BASE}/")),
            "case {case:?}: expected a location on {BASE}, got {:?}",
            row.url
        );
        assert!(
            !row.url.contains("amazonaws.com"),
            "case {case:?}: a location may not name a host only AWS serves, got {:?}",
            row.url
        );

        // The path carries the object's own S3 address: the per-region bucket and the key.
        let (path, query) = path_and_query(&row.url);
        let prefix = format!("/awslambda-{REGION}-{}/snapshots/{ACCOUNT}/", row.bucket);
        let key = path.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case:?}: expected a path under {prefix}, got {path:?}")
        });
        assert!(
            key.starts_with(&row.key_prefix),
            "case {case:?}: expected a key naming {:?}, got {key:?}",
            row.key_prefix
        );

        // It is PRESIGNED: the signature, the algorithm, the ten-minute expiry, and a
        // credential scope naming this service in the Region the request was signed for.
        assert_eq!(
            query_field(&query, "X-Amz-Algorithm").as_deref(),
            Some(SIGV4_ALGORITHM),
            "case {case:?}: expected the SigV4 algorithm, got {query:?}"
        );
        assert_eq!(
            query_field(&query, "X-Amz-Expires").as_deref(),
            Some(EXPIRES_SECONDS),
            "case {case:?}: expected an expiry of {EXPIRES_SECONDS} seconds, got {query:?}"
        );
        assert!(
            query_field(&query, "X-Amz-Signature").is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected a signature, got {query:?}"
        );
        let scope = query_field(&query, "X-Amz-Credential").unwrap_or_default();
        assert!(
            scope.ends_with(&format!("/{REGION}/lambda/aws4_request")),
            "case {case:?}: expected a credential scope naming lambda in {REGION}, got \
             {scope:?}"
        );

        // And following it serves exactly the bytes the store holds for that object.
        let answer = fetch(&router, "GET", &row.url).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200 following {:?}, got {} {}",
            row.url,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.body.as_ref(),
            row.expected.as_slice(),
            "case {case:?}: expected the {} stored bytes, got {} bytes",
            row.expected.len(),
            answer.body.len()
        );
        // A download is a pure READ: the same bytes again, and a `HEAD` without them.
        let again = fetch(&router, "GET", &row.url).await;
        assert_eq!(
            again.body, answer.body,
            "case {case:?}: expected the same bytes on a second fetch, got {} bytes",
            again.body.len()
        );
        let head = fetch(&router, "HEAD", &row.url).await;
        assert_eq!(
            (head.status, head.body.len()),
            (200, 0),
            "case {case:?}: expected a 200 HEAD with no body, got {} with {} bytes",
            head.status,
            head.body.len()
        );
    }

    // Two different objects are two different locations: the newest version of a layer is
    // not the address of the first one.
    let (first_path, _) = path_and_query(&rows[1].url);
    let (newest_path, _) = path_and_query(&rows[4].url);
    assert_ne!(
        first_path, newest_path,
        "each version's location must address its own archive, got {first_path:?} twice"
    );
}

/// Every address under a published download store is answered BY this plane: a key it holds
/// no object at is the plane's own `NoSuchKey`, and a write method is refused as the method
/// it is, rather than either falling through to the router's answer for a request that names
/// no operation.
///
/// The rows differ along what the rule generalises over: which store the address names, and
/// what makes the key name no object (a resource never published, a key of no published
/// shape at all).
#[tokio::test]
async fn an_address_under_a_published_store_is_answered_by_the_download_plane() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_at(&engine, BASE);

    for (case, path) in [
        (
            "a layer version nobody published",
            format!("/awslambda-{REGION}-{BUCKET_LAYERS}/snapshots/{ACCOUNT}/never-published-1"),
        ),
        (
            "a function revision nobody stored",
            format!(
                "/awslambda-{REGION}-{BUCKET_TASKS}/snapshots/{ACCOUNT}\
                 /absent-00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "a key of no published shape",
            format!("/awslambda-{REGION}-{BUCKET_LAYERS}/snapshots/{ACCOUNT}/not-a-key-at-all"),
        ),
    ] {
        let answer = fetch(&router, "GET", &format!("{BASE}{path}")).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404 for an object this store does not hold, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = String::from_utf8_lossy(&answer.body).to_string();
        assert!(
            body.contains("NoSuchKey"),
            "case {case:?}: expected this plane's own NoSuchKey answer, got {body}"
        );
    }

    // A download is a read, so a write method to one of these addresses is refused as the
    // method it is.
    let archive = package_bytes(32);
    let published = publish_layer_version(
        &router,
        "lyr-methods",
        &json!({ "Content": { "ZipFile": zip_file(&archive) } }),
    )
    .await;
    let url = location_of("a write method", &published, "/Content/Location");
    for method in ["POST", "PUT", "DELETE"] {
        let answer = fetch(&router, method, &url).await;
        assert_eq!(
            answer.status,
            405,
            "case {method:?}: expected 405 for a write to a download URL, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

/// The announced address FOLLOWS the endpoint this emulator is reached on: the same publish
/// over emulators with different base URLs announces a location on each one, and each is
/// fetchable there. The rows differ along the base URL's host form (a name, a loopback
/// address with a port, the base an unserved emulator reports).
#[tokio::test]
async fn an_announced_download_location_follows_the_endpoint_the_emulator_is_reached_on() {
    // `https://emulator.invalid` is the base an emulator that is not served on a port
    // reports, which is the one a test router carries unless it is given another.
    for base in [BASE, "https://127.0.0.1:9297", "https://emulator.invalid"] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_at(&engine, base);
        let archive = package_bytes(64);
        let published = publish_layer_version(
            &router,
            "lyr-based",
            &json!({ "Content": { "ZipFile": zip_file(&archive) } }),
        )
        .await;
        let url = location_of(base, &published, "/Content/Location");
        assert!(
            url.starts_with(&format!("{base}/")),
            "case {base:?}: expected a location on {base}, got {url:?}"
        );
        let answer = fetch(&router, "GET", &url).await;
        assert_eq!(
            (answer.status, answer.body.as_ref()),
            (200, archive.as_slice()),
            "case {base:?}: expected the {} stored bytes at {url:?}, got {} {}",
            archive.len(),
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}
