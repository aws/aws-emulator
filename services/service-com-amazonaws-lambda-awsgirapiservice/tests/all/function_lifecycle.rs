// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Lambda function lifecycle: create a function, read it
//! back, page through the list, resolve its name in each documented form, invoke it and
//! delete it.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are
//! about the wire behaviour (status, body, bound headers) rather than about any
//! handler's internals. The clock-driven half — the move from the `Pending` state the
//! model documents on `CreateFunction` to `Active` — is exercised by moving the stored
//! due instant into the past, so nothing here sleeps.
//!
//! The harness below (the router, the `Answer` wrapper and the request helpers) is
//! `pub(crate)` so the other suites of this test target drive the same wire, rather than
//! each rebuilding one that could disagree with it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Neither the key id nor the
/// signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
pub(crate) const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// One service mounted on its own engine, reached exactly the way the production
/// router reaches it.
pub(crate) fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body, or `None` when absent or not a string.
    pub(crate) fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }

    /// A bound response header's value.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The wire error code (`__type`) of an error response.
    pub(crate) fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| self.header("x-amzn-errortype"))
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }
}

/// Dispatch one request with the given body and content type.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
    headers: &[(&str, String)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", content_type);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// A JSON request, the shape every control-plane operation here uses.
pub(crate) async fn json_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &serde_json::Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    send(router, method, uri, "application/json", payload, &[]).await
}

/// `CreateFunction` with the given request body.
pub(crate) async fn create_function(
    router: &MultiServiceRouter,
    body: &serde_json::Value,
) -> Answer {
    json_call(router, "POST", "/2015-03-31/functions", body).await
}

/// `GetFunction` on a function reference, with no qualifier.
async fn get_function(router: &MultiServiceRouter, reference: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}");
    json_call(router, "GET", &uri, &serde_json::Value::Null).await
}

/// `GetFunction` on a function reference at one qualifier, which is the read of ONE
/// identity of it.
async fn get_function_at(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: &str,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}?Qualifier={qualifier}");
    json_call(router, "GET", &uri, &serde_json::Value::Null).await
}

/// The base64 SHA-256 of `bytes` — the independent computation of what `CodeSha256`
/// must report, so the test never reads the value it is checking out of the response.
fn expected_code_sha256(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// Deployment package bytes of a given length. The content is irrelevant to the API —
/// only its length and its digest are reported — but it differs per length so two
/// packages never share a digest.
pub(crate) fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `Code.ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
pub(crate) fn zip_file(bytes: &[u8]) -> serde_json::Value {
    serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The unqualified ARN a function of `name` must carry.
pub(crate) fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what
/// waiting would do, without waiting.
pub(crate) fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
    for provider in store.capacity_providers.values_mut() {
        provider.active_due = past;
    }
}

/// Move every stored function's UPDATE completion instant into the past as well, the way
/// [`make_activation_due`] moves its activation: "you can't modify the function" while an
/// update is in progress, so a suite driving several writes at one function brings each
/// settle forward instead of waiting it out.
pub(crate) fn make_update_due(engine: &Arc<engine::InMemoryEngine>) {
    make_activation_due(engine);
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// Push an image to the Amazon ECR registry of the SAME engine, so a container-image
/// `ImageUri` naming it RESOLVES: a `PackageType=Image` create follows its reference into
/// that registry, and a reference naming nothing pushed is refused.
///
/// The image goes in through ECR's OWN `CreateRepository`/`PutImage`, over a router
/// carrying ECR alone on this engine, so what a Lambda create resolves is what the
/// registry minted. The manifest declares no layer and no configuration blob, so the push
/// needs no layer upload behind it. Answers the manifest digest the registry minted.
pub(crate) async fn seed_image(
    engine: &Arc<engine::InMemoryEngine>,
    image_uri: &str,
) -> String {
    let path = image_uri
        .split_once('/')
        .map_or(image_uri, |(_, path)| path);
    let (repository, tag) = match path.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => (repository, tag),
        _ => (path, "latest"),
    };
    let mut registry = MultiServiceRouter::new();
    registry.set_request_authentication(false);
    registry.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "layers": [],
        "annotations": { "org.opencontainers.image.ref.name": image_uri },
    })
    .to_string();
    let call = |operation: &'static str, body: serde_json::Value| {
        let request = http::Request::builder()
            .method("POST")
            .uri("http://api.ecr.us-east-1.amazonaws.com/")
            .header("host", "api.ecr.us-east-1.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=emulator-test-key/20260101/us-east-1/ecr/aws4_request, \
                 SignedHeaders=host, Signature=not-verified",
            )
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AmazonEC2ContainerRegistry_V20150921.{operation}"),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the registry request is well formed");
        request
    };
    let _ = registry
        .dispatch(call(
            "CreateRepository",
            serde_json::json!({ "repositoryName": repository }),
        ))
        .await;
    let pushed = registry
        .dispatch(call(
            "PutImage",
            serde_json::json!({
                "repositoryName": repository,
                "imageTag": tag,
                "imageManifest": manifest,
            }),
        ))
        .await;
    let body = pushed.into_body();
    let document: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "expected PutImage to answer JSON ({error}): {}",
            String::from_utf8_lossy(&body)
        )
    });
    document
        .pointer("/image/imageId/imageDigest")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "expected PutImage to mint a manifest digest for {image_uri}, got {}",
                String::from_utf8_lossy(&body)
            )
        })
        .to_string()
}

/// Create a function and bring it to `Active`, the state every invocation test needs.
pub(crate) async fn create_active_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    code: &[u8],
) {
    let answer = create_function(
        router,
        &serde_json::json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(code) },
        }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected 201 from CreateFunction, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    make_activation_due(engine);
}

/// One row of the create table: what is asked for, and what the stored configuration
/// must therefore report.
struct CreateCase {
    name: &'static str,
    request: serde_json::Value,
    code_size: i64,
    code_sha256: Option<String>,
    version: &'static str,
    package_type: &'static str,
    memory_size: i64,
    timeout: i64,
    ephemeral_storage: i64,
    architecture: &'static str,
    tracing_mode: &'static str,
}

/// `CreateFunction` persists the function and answers with the configuration it stored:
/// the values the request named, and the documented default for every value it did not.
#[tokio::test]
async fn create_function_persists_what_it_reports() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let small = package_bytes(64);
    let large = package_bytes(5_000);

    // The container-image row's reference is RESOLVED against the registry when the
    // package is stored, so the image it names is pushed there first — and the digest the
    // registry minted is what that function's `CodeSha256` reports, since the image IS its
    // deployment package.
    let image_digest =
        seed_image(&engine, "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest").await;
    let image_sha256 = image_digest
        .split_once(':')
        .map(|(_, hexadecimal)| hexadecimal.to_string());

    let cases = vec![
        // An inline .zip archive, everything but the required members defaulted.
        CreateCase {
            name: "defaults",
            request: serde_json::json!({
                "FunctionName": "defaults",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&small) },
            }),
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            version: "$LATEST",
            package_type: "Zip",
            memory_size: 128,
            timeout: 3,
            ephemeral_storage: 512,
            architecture: "x86_64",
            tracing_mode: "PassThrough",
        },
        // A larger archive with every optional member named, so none of the defaults
        // can be what is reported.
        CreateCase {
            name: "configured",
            request: serde_json::json!({
                "FunctionName": "configured",
                "Role": ROLE,
                "Runtime": "nodejs20.x",
                "Handler": "app.main",
                "Description": "a configured function",
                "Timeout": 900,
                "MemorySize": 1024,
                "Architectures": ["arm64"],
                "EphemeralStorage": { "Size": 2048 },
                "TracingConfig": { "Mode": "Active" },
                "Environment": { "Variables": { "STAGE": "prod" } },
                "Tags": { "team": "platform" },
                "Code": { "ZipFile": zip_file(&large) },
            }),
            code_size: i64::try_from(large.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&large)),
            version: "$LATEST",
            package_type: "Zip",
            memory_size: 1024,
            timeout: 900,
            ephemeral_storage: 2048,
            architecture: "arm64",
            tracing_mode: "Active",
        },
        // `Publish` creates version 1 "from its initial configuration", and the call
        // reports that version rather than the unpublished one.
        CreateCase {
            name: "published",
            request: serde_json::json!({
                "FunctionName": "published",
                "Role": ROLE,
                "Runtime": "java21",
                "Handler": "Handler::run",
                "Publish": true,
                "Code": { "ZipFile": zip_file(&small) },
            }),
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            version: "1",
            package_type: "Zip",
            memory_size: 128,
            timeout: 3,
            ephemeral_storage: 512,
            architecture: "x86_64",
            tracing_mode: "PassThrough",
        },
        // An S3-sourced archive: this emulator holds no bytes for it, so it reports no
        // size it did not measure and no digest it did not compute.
        CreateCase {
            name: "from-s3",
            request: serde_json::json!({
                "FunctionName": "from-s3",
                "Role": ROLE,
                "Runtime": "ruby3.3",
                "Handler": "app.handler",
                "Code": { "S3Bucket": "deployments", "S3Key": "fn.zip" },
            }),
            code_size: 0,
            code_sha256: None,
            version: "$LATEST",
            package_type: "Zip",
            memory_size: 128,
            timeout: 3,
            ephemeral_storage: 512,
            architecture: "x86_64",
            tracing_mode: "PassThrough",
        },
        // A container image package: no runtime and no handler, per the model's own
        // "You do not need to specify the handler and runtime properties".
        CreateCase {
            name: "from-image",
            request: serde_json::json!({
                "FunctionName": "from-image",
                "Role": ROLE,
                "PackageType": "Image",
                "Code": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest" },
            }),
            code_size: 0,
            // An `Image` package's `CodeSha256` is the digest of the image the function is
            // frozen to, which for a pushed image is the one the REGISTRY minted.
            code_sha256: image_sha256,
            version: "$LATEST",
            package_type: "Image",
            memory_size: 128,
            timeout: 3,
            ephemeral_storage: 512,
            architecture: "x86_64",
            tracing_mode: "PassThrough",
        },
    ];

    for case in &cases {
        let answer = create_function(&router, &case.request).await;
        assert_eq!(
            answer.status,
            201,
            "case {}: expected 201, got {} {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let created = answer.json();

        // `CreateFunction`'s subject is the function, so the ARN it reports is the
        // function's own unqualified one whether or not the call also published a version;
        // the published version is reported by `Version` alone, asserted below.
        let expected_arn = function_arn(case.name);
        assert_eq!(
            created["FunctionArn"],
            serde_json::json!(expected_arn),
            "case {}: expected FunctionArn {expected_arn}, got {:?}",
            case.name,
            created["FunctionArn"]
        );
        assert_eq!(
            created["FunctionName"],
            serde_json::json!(case.name),
            "case {}: expected FunctionName {}, got {:?}",
            case.name,
            case.name,
            created["FunctionName"]
        );
        assert_eq!(
            created["CodeSize"],
            serde_json::json!(case.code_size),
            "case {}: expected CodeSize {}, got {:?}",
            case.name,
            case.code_size,
            created["CodeSize"]
        );
        assert_eq!(
            created["CodeSha256"].as_str(),
            case.code_sha256.as_deref(),
            "case {}: expected CodeSha256 {:?}, got {:?}",
            case.name,
            case.code_sha256,
            created["CodeSha256"]
        );
        assert_eq!(
            created["Version"],
            serde_json::json!(case.version),
            "case {}: expected Version {}, got {:?}",
            case.name,
            case.version,
            created["Version"]
        );
        assert_eq!(
            created["PackageType"],
            serde_json::json!(case.package_type),
            "case {}: expected PackageType {}, got {:?}",
            case.name,
            case.package_type,
            created["PackageType"]
        );
        assert_eq!(
            created["MemorySize"],
            serde_json::json!(case.memory_size),
            "case {}: expected MemorySize {}, got {:?}",
            case.name,
            case.memory_size,
            created["MemorySize"]
        );
        assert_eq!(
            created["Timeout"],
            serde_json::json!(case.timeout),
            "case {}: expected Timeout {}, got {:?}",
            case.name,
            case.timeout,
            created["Timeout"]
        );
        assert_eq!(
            created["EphemeralStorage"]["Size"],
            serde_json::json!(case.ephemeral_storage),
            "case {}: expected EphemeralStorage.Size {}, got {:?}",
            case.name,
            case.ephemeral_storage,
            created["EphemeralStorage"]
        );
        assert_eq!(
            created["Architectures"],
            serde_json::json!([case.architecture]),
            "case {}: expected Architectures [{}], got {:?}",
            case.name,
            case.architecture,
            created["Architectures"]
        );
        assert_eq!(
            created["TracingConfig"]["Mode"],
            serde_json::json!(case.tracing_mode),
            "case {}: expected TracingConfig.Mode {}, got {:?}",
            case.name,
            case.tracing_mode,
            created["TracingConfig"]
        );
        assert_eq!(
            created["Role"],
            serde_json::json!(ROLE),
            "case {}: expected the Role as supplied, got {:?}",
            case.name,
            created["Role"]
        );
        // "When you create a function, Lambda provisions an instance of the function
        // and its supporting resources" — so it is not ready the instant it is made.
        assert_eq!(
            created["State"],
            serde_json::json!("Pending"),
            "case {}: expected State Pending straight after creation, got {:?}",
            case.name,
            created["State"]
        );
        assert_eq!(
            created["StateReasonCode"],
            serde_json::json!("Creating"),
            "case {}: expected StateReasonCode Creating, got {:?}",
            case.name,
            created["StateReasonCode"]
        );
        assert!(
            created["LastUpdateStatus"].is_null(),
            "case {}: LastUpdateStatus is \"first set to Successful after function \
             creation completes\", got {:?}",
            case.name,
            created["LastUpdateStatus"]
        );
        let revision = created["RevisionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !revision.is_empty(),
            "case {}: expected a RevisionId, got {:?}",
            case.name,
            created["RevisionId"]
        );
        let last_modified = created["LastModified"].as_str().unwrap_or_default();
        assert!(
            last_modified.contains('T'),
            "case {}: expected an ISO-8601 LastModified, got {last_modified:?}",
            case.name
        );

        // The round trip: GetFunction reads the persisted record, so it reports the
        // same configuration the create call did.
        let read = get_function(&router, case.name).await;
        assert_eq!(
            read.status,
            200,
            "case {}: expected 200 from GetFunction, got {} {}",
            case.name,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let fetched = read.json();
        let configuration = &fetched["Configuration"];
        for member in [
            "FunctionName",
            "CodeSize",
            "CodeSha256",
            "PackageType",
            "MemorySize",
            "Timeout",
            "Architectures",
            "Role",
            "Handler",
            "Runtime",
            "Description",
            "Environment",
            "RevisionId",
            "LastModified",
        ] {
            // `RevisionId` belongs to one IDENTITY, so it is asserted below rather than
            // here for a create that published a version: that call answered with THAT
            // version's revision, and this read is of the unpublished version.
            if member == "RevisionId" && case.version != "$LATEST" {
                continue;
            }
            assert_eq!(
                configuration[member], created[member],
                "case {}: GetFunction reported a different {member} ({:?}) than \
                 CreateFunction stored ({:?})",
                case.name, configuration[member], created[member]
            );
        }
        // A create that published a version answered with the VERSION's revision: the
        // qualified read of that version reports the same one, and the unqualified read
        // of the unpublished version reports its own.
        if case.version != "$LATEST" {
            let qualified = get_function_at(&router, case.name, case.version).await;
            assert_eq!(
                qualified.json()["Configuration"]["RevisionId"], created["RevisionId"],
                "case {}: expected version {} to report the revision CreateFunction \
                 answered ({:?}), got {:?}",
                case.name,
                case.version,
                created["RevisionId"],
                qualified.json()["Configuration"]["RevisionId"]
            );
            assert_ne!(
                configuration["RevisionId"], created["RevisionId"],
                "case {}: expected the unpublished version to carry a revision of its \
                 own, got the published version's {:?}",
                case.name, configuration["RevisionId"]
            );
        }
        // GetFunction with no qualifier reports the unpublished version, whatever
        // version the create call published.
        assert_eq!(
            configuration["Version"],
            serde_json::json!("$LATEST"),
            "case {}: expected GetFunction to report $LATEST, got {:?}",
            case.name,
            configuration["Version"]
        );

        let code = &fetched["Code"];
        if case.package_type == "Image" {
            assert_eq!(
                code["RepositoryType"],
                serde_json::json!("ECR"),
                "case {}: expected RepositoryType ECR for an image package, got {:?}",
                case.name,
                code["RepositoryType"]
            );
            assert_eq!(
                code["ImageUri"], case.request["Code"]["ImageUri"],
                "case {}: expected the ImageUri as supplied, got {:?}",
                case.name, code["ImageUri"]
            );
        } else {
            assert_eq!(
                code["RepositoryType"],
                serde_json::json!("S3"),
                "case {}: expected RepositoryType S3 for a zip package, got {:?}",
                case.name,
                code["RepositoryType"]
            );
            let location = code["Location"].as_str().unwrap_or_default();
            // The location belongs to the identity READ, so the revision in it is the one
            // this read reports — which for a create that published a version is the
            // unpublished version's rather than that version's.
            let read_revision = configuration["RevisionId"].as_str().unwrap_or_default();
            assert!(
                location.contains(case.name) && location.contains(read_revision),
                "case {}: expected a Location naming the function and its revision \
                 {read_revision:?}, got {location:?}",
                case.name
            );
        }
    }

    // Tags are reported only for a function that carries them.
    let tagged = get_function(&router, "configured").await;
    assert_eq!(
        tagged.json()["Tags"],
        serde_json::json!({ "team": "platform" }),
        "expected the tags as supplied at create, got {:?}",
        tagged.json()["Tags"]
    );
    let untagged = get_function(&router, "defaults").await;
    assert!(
        untagged.json()["Tags"].is_null(),
        "expected no Tags for a function created without any, got {:?}",
        untagged.json()["Tags"]
    );
    // Nothing has set reserved concurrency, so none is reported.
    assert!(
        untagged.json()["Concurrency"].is_null(),
        "expected no Concurrency before any is reserved, got {:?}",
        untagged.json()["Concurrency"]
    );

    // A published version is readable by its qualifier, and reports that version.
    let published = json_call(
        &router,
        "GET",
        "/2015-03-31/functions/published?Qualifier=1",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        published.status,
        200,
        "expected 200 for version 1 of a published function, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );
    assert_eq!(
        published.json()["Configuration"]["FunctionArn"],
        serde_json::json!(format!("{}:1", function_arn("published"))),
        "expected the version-qualified ARN, got {:?}",
        published.json()["Configuration"]["FunctionArn"]
    );
}

/// Creating a name that already exists conflicts, and a request whose package type and
/// `Code` do not agree is refused with the modelled 400.
#[tokio::test]
async fn create_function_refuses_what_it_cannot_store() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let code = package_bytes(32);

    let first = create_function(
        &router,
        &serde_json::json!({
            "FunctionName": "taken",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&code) },
        }),
    )
    .await;
    assert_eq!(first.status, 201, "the first create must succeed");

    let conflict = create_function(
        &router,
        &serde_json::json!({
            "FunctionName": "taken",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&code) },
        }),
    )
    .await;
    assert_eq!(
        conflict.status, 409,
        "expected 409 for a name that already exists, got {} {}",
        conflict.status,
        String::from_utf8_lossy(&conflict.body)
    );
    assert!(
        conflict.error_code().contains("ResourceConflictException"),
        "expected ResourceConflictException, got {}",
        conflict.error_code()
    );

    // Each row names one documented rule the service enforces that the model's own
    // constraints cannot express.
    let refusals = vec![
        (
            "a zip package with no code at all",
            serde_json::json!({
                "FunctionName": "no-code",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": {},
            }),
        ),
        (
            "a zip package with an S3 bucket but no key",
            serde_json::json!({
                "FunctionName": "no-key",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "S3Bucket": "deployments" },
            }),
        ),
        (
            "a zip package with no runtime",
            serde_json::json!({
                "FunctionName": "no-runtime",
                "Role": ROLE,
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&code) },
            }),
        ),
        (
            "a zip package with no handler",
            serde_json::json!({
                "FunctionName": "no-handler",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Code": { "ZipFile": zip_file(&code) },
            }),
        ),
        (
            "an image package with no image uri",
            serde_json::json!({
                "FunctionName": "no-image",
                "Role": ROLE,
                "PackageType": "Image",
                "Code": { "ZipFile": zip_file(&code) },
            }),
        ),
        (
            "an image package that also names a runtime",
            serde_json::json!({
                "FunctionName": "image-runtime",
                "Role": ROLE,
                "PackageType": "Image",
                "Runtime": "python3.12",
                "Code": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest" },
            }),
        ),
        (
            "a zip package that names an image uri",
            serde_json::json!({
                "FunctionName": "zip-image",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest" },
            }),
        ),
        (
            "a bare name longer than the documented 64 characters",
            serde_json::json!({
                "FunctionName": "a".repeat(65),
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&code) },
            }),
        ),
        (
            "a name qualified with a version",
            serde_json::json!({
                "FunctionName": "qualified:3",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&code) },
            }),
        ),
        (
            "environment variables past the documented 4 KB combined quota",
            serde_json::json!({
                "FunctionName": "big-env",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Environment": { "Variables": { "BLOB": "x".repeat(4 * 1024 + 1) } },
                "Code": { "ZipFile": zip_file(&code) },
            }),
        ),
    ];

    for (case, request) in &refusals {
        let answer = create_function(&router, request).await;
        assert_eq!(
            answer.status,
            400,
            "case {case}: expected 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer
                .error_code()
                .contains("InvalidParameterValueException"),
            "case {case}: expected InvalidParameterValueException, got {}",
            answer.error_code()
        );
    }

    // A refused create persists nothing.
    for case in [
        "no-code",
        "no-key",
        "no-runtime",
        "no-handler",
        "no-image",
        "image-runtime",
        "zip-image",
        "qualified",
        "big-env",
    ] {
        let answer = get_function(&router, case).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: a refused create must store nothing, got {}",
            answer.status
        );
    }
}

/// Every documented `FunctionName` form — bare name, `name:qualifier`, the bare
/// `function:` prefix, a partial ARN and a full ARN — names the same record.
#[tokio::test]
async fn every_function_name_form_resolves_to_the_same_record() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "resolve-me", &package_bytes(16)).await;

    let references = [
        "resolve-me",
        "resolve-me:$LATEST",
        "function:resolve-me",
        "000000000000:function:resolve-me",
        "arn:aws:lambda:us-east-1:000000000000:function:resolve-me",
        "arn:aws:lambda:us-east-1:000000000000:function:resolve-me:$LATEST",
    ];
    for reference in references {
        let answer = get_function(&router, reference).await;
        assert_eq!(
            answer.status,
            200,
            "case {reference}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.json()["Configuration"]["FunctionArn"],
            serde_json::json!(function_arn("resolve-me")),
            "case {reference}: expected the one record's ARN, got {:?}",
            answer.json()["Configuration"]["FunctionArn"]
        );
    }

    // A reference that names no stored function, or no such version of one, is a 404 —
    // and one that names two different versions at once is a 400.
    let not_found = [
        "absent",
        "resolve-me:2",
        "arn:aws:lambda:us-east-1:000000000000:function:absent",
        "resolve-me:prod",
    ];
    for reference in not_found {
        let answer = get_function(&router, reference).await;
        assert_eq!(
            answer.status,
            404,
            "case {reference}: expected 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {reference}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
    }

    let contradiction = json_call(
        &router,
        "GET",
        "/2015-03-31/functions/resolve-me:$LATEST?Qualifier=1",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        contradiction.status,
        400,
        "expected 400 when the name and the Qualifier name different versions, got {} {}",
        contradiction.status,
        String::from_utf8_lossy(&contradiction.body)
    );
}

/// `ListFunctions` reports every persisted function, honours `MaxItems`, and continues
/// from `NextMarker` without repeating or dropping an entry.
#[tokio::test]
async fn list_functions_pages_over_every_persisted_function() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let names: Vec<String> = (0..7).map(|index| format!("fn-{index}")).collect();
    for name in &names {
        create_active_function(&router, &engine, name, &package_bytes(8 + name.len())).await;
    }

    // Each row is a page size; the walk must see every function exactly once whatever
    // it is.
    for page_size in [1_usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= names.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let uri = match &marker {
                Some(marker) => format!(
                    "/2015-03-31/functions?MaxItems={page_size}&Marker={marker}"
                ),
                None => format!("/2015-03-31/functions?MaxItems={page_size}"),
            };
            let answer = json_call(&router, "GET", &uri, &serde_json::Value::Null).await;
            assert_eq!(
                answer.status,
                200,
                "case page size {page_size}: expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let body = answer.json();
            let page = body["Functions"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case page size {page_size}: expected a Functions array, got {body:?}")
                })
                .clone();
            assert!(
                page.len() <= page_size.min(50),
                "case page size {page_size}: expected at most {} entries, got {}",
                page_size.min(50),
                page.len()
            );
            for entry in &page {
                let name = entry["FunctionName"].as_str().unwrap_or_default().to_string();
                assert!(
                    !seen.contains(&name),
                    "case page size {page_size}: {name} was reported twice"
                );
                // "The ListFunctions operation returns a subset of the
                // FunctionConfiguration fields. To get the additional fields (State,
                // StateReasonCode, StateReason, LastUpdateStatus, ...) for a function
                // or version, use GetFunction."
                for omitted in [
                    "State",
                    "StateReason",
                    "StateReasonCode",
                    "LastUpdateStatus",
                    "LastUpdateStatusReason",
                    "LastUpdateStatusReasonCode",
                    "RuntimeVersionConfig",
                ] {
                    assert!(
                        entry[omitted].is_null(),
                        "case page size {page_size}: ListFunctions reports a subset of the \
                         fields, so {omitted} must be absent, got {:?}",
                        entry[omitted]
                    );
                }
                seen.push(name);
            }
            match body["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case page size {page_size}: expected every function once in name order, got {seen:?}"
        );
    }

    // `FunctionVersion=ALL` adds an entry per published version. Publishing at create
    // time is the only way into a version in this slice.
    let published = create_function(
        &router,
        &serde_json::json!({
            "FunctionName": "with-version",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": zip_file(&package_bytes(24)) },
        }),
    )
    .await;
    assert_eq!(published.status, 201, "the published create must succeed");

    let latest_only = json_call(
        &router,
        "GET",
        "/2015-03-31/functions",
        &serde_json::Value::Null,
    )
    .await;
    let latest_versions: Vec<String> = latest_only.json()["Functions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Version"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        latest_versions.iter().all(|version| version == "$LATEST"),
        "expected only unpublished versions without FunctionVersion=ALL, got {latest_versions:?}"
    );

    let all = json_call(
        &router,
        "GET",
        "/2015-03-31/functions?FunctionVersion=ALL",
        &serde_json::Value::Null,
    )
    .await;
    let arns: Vec<String> = all.json()["Functions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["FunctionArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        arns.contains(&format!("{}:1", function_arn("with-version"))),
        "expected FunctionVersion=ALL to include the published version, got {arns:?}"
    );

    // "MasterRegion [...] If specified, you must set FunctionVersion to ALL."
    let bad_master = json_call(
        &router,
        "GET",
        "/2015-03-31/functions?MasterRegion=us-east-1",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        bad_master.status,
        400,
        "expected 400 for MasterRegion without FunctionVersion=ALL, got {} {}",
        bad_master.status,
        String::from_utf8_lossy(&bad_master.body)
    );

    // `MaxItems` is modelled `@range(min: 1, max: 10000)`: a page size outside it is
    // refused rather than paged over, and the request answers instead of failing.
    for (case, max_items) in [("zero", "0"), ("negative", "-1"), ("past the maximum", "10001")] {
        let answer = json_call(
            &router,
            "GET",
            &format!("/2015-03-31/functions?MaxItems={max_items}"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {case}: expected 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

/// A paged `FunctionVersion=ALL` walk reports every function AND every published
/// version exactly once, including when one function's name is a prefix of another's
/// — the case where a version key does not sort directly after its own function.
#[tokio::test]
async fn list_functions_pages_over_every_version_without_losing_one() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // `fn-1` is a prefix of `fn-10`, so the key `fn-1:1` sorts after `fn-10`: an
    // ordering a page boundary has to agree with.
    let names = ["fn-1", "fn-10", "fn-2"];
    for name in names {
        let answer = create_function(
            &router,
            &serde_json::json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Publish": true,
                "Code": { "ZipFile": zip_file(&package_bytes(16 + name.len())) },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {name}: the published create must succeed, got {}",
            answer.status
        );
    }
    make_activation_due(&engine);

    let mut expected: Vec<String> = Vec::new();
    for name in names {
        expected.push(function_arn(name));
        expected.push(format!("{}:1", function_arn(name)));
    }
    expected.sort();

    for page_size in [1_usize, 2, 4, 6, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let uri = match &marker {
                Some(marker) => format!(
                    "/2015-03-31/functions?FunctionVersion=ALL&MaxItems={page_size}&Marker={marker}"
                ),
                None => {
                    format!("/2015-03-31/functions?FunctionVersion=ALL&MaxItems={page_size}")
                }
            };
            let answer = json_call(&router, "GET", &uri, &serde_json::Value::Null).await;
            assert_eq!(
                answer.status,
                200,
                "case page size {page_size}: expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let body = answer.json();
            for entry in body["Functions"].as_array().cloned().unwrap_or_default() {
                let arn = entry["FunctionArn"].as_str().unwrap_or_default().to_string();
                assert!(
                    !seen.contains(&arn),
                    "case page size {page_size}: {arn} was reported twice"
                );
                seen.push(arn);
            }
            match body["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        seen.sort();
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every function and version exactly \
             once, got {seen:?}"
        );
    }

    // `MasterRegion` names the region of a Lambda@Edge replica's master function. No
    // operation in this slice can create a replica, so no stored function carries a
    // `MasterArn` and a filtered listing matches none of them.
    let filtered = json_call(
        &router,
        "GET",
        "/2015-03-31/functions?FunctionVersion=ALL&MasterRegion=us-east-1",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        filtered.status,
        200,
        "expected 200 for a master-region filter, got {} {}",
        filtered.status,
        String::from_utf8_lossy(&filtered.body)
    );
    assert_eq!(
        filtered.json()["Functions"],
        serde_json::json!([]),
        "expected no replica to match a master-region filter, got {:?}",
        filtered.json()["Functions"]
    );
    assert!(
        filtered.json()["NextMarker"].is_null(),
        "expected no NextMarker on an exhausted page, got {:?}",
        filtered.json()["NextMarker"]
    );
}

/// `Invoke` resolves the function out of state, refuses what the service refuses, and
/// answers the one invocation type that runs no function code.
#[tokio::test]
async fn invoke_resolves_the_function_and_runs_nothing_it_cannot() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "invokable", &package_bytes(48)).await;

    let invoke = |name: String, query: &str, invocation_type: Option<&str>, body: Vec<u8>| {
        let headers: Vec<(&str, String)> = invocation_type
            .map(|value| vec![("x-amz-invocation-type", value.to_string())])
            .unwrap_or_default();
        let uri = format!("/2015-03-31/functions/{name}/invocations{query}");
        let router = &router;
        async move {
            send(
                router,
                "POST",
                &uri,
                "application/octet-stream",
                body,
                &headers,
            )
            .await
        }
    };

    // A dry run validates the invocation without running the function, which is what
    // the model says it does; its documented status is 204.
    for reference in [
        "invokable",
        "function:invokable",
        "arn:aws:lambda:us-east-1:000000000000:function:invokable",
    ] {
        let answer = invoke(
            reference.to_string(),
            "",
            Some("DryRun"),
            br#"{"key":1}"#.to_vec(),
        )
        .await;
        assert_eq!(
            answer.status,
            204,
            "case {reference}: expected 204 for a dry run, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.header("x-amz-executed-version").as_deref(),
            Some("$LATEST"),
            "case {reference}: expected the resolved version in X-Amz-Executed-Version, got {:?}",
            answer.header("x-amz-executed-version")
        );
    }

    // An unknown function is never invoked, whatever the invocation type.
    for invocation_type in [None, Some("Event"), Some("DryRun"), Some("RequestResponse")] {
        let answer = invoke("absent".to_string(), "", invocation_type, b"{}".to_vec()).await;
        assert_eq!(
            answer.status,
            404,
            "case {invocation_type:?}: expected 404 for an unknown function, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {invocation_type:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
    }

    // An unknown version of a known function is equally not found.
    let unknown_version = invoke(
        "invokable".to_string(),
        "?Qualifier=4",
        Some("DryRun"),
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(
        unknown_version.status, 404,
        "expected 404 for a version the function does not have, got {}",
        unknown_version.status
    );

    // "The JSON that you want to provide to your Lambda function as input": a body that
    // is not JSON is refused, and an empty one is accepted.
    for (case, body, expected) in [
        ("an empty body", Vec::new(), 204_u16),
        ("a JSON object", br#"{"k":1}"#.to_vec(), 204),
        ("a JSON scalar", b"42".to_vec(), 204),
        ("a truncated object", br#"{"k":"#.to_vec(), 400),
        ("plain text", b"not json".to_vec(), 400),
    ] {
        let answer = invoke("invokable".to_string(), "", Some("DryRun"), body).await;
        assert_eq!(
            answer.status,
            expected,
            "case {case}: expected {expected}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected == 400 {
            assert!(
                answer
                    .error_code()
                    .contains("InvalidRequestContentException"),
                "case {case}: expected InvalidRequestContentException, got {}",
                answer.error_code()
            );
        }
    }

    // "For synchronous invocations, the maximum payload size is 6 MB. For asynchronous
    // invocations, the maximum payload size is 1 MB." A payload one byte past the
    // asynchronous limit is refused as asynchronous and accepted as a dry run.
    let oversized = {
        let mut payload = Vec::with_capacity(1024 * 1024 + 3);
        payload.push(b'"');
        payload.extend(std::iter::repeat_n(b'a', 1024 * 1024 + 1));
        payload.push(b'"');
        payload
    };
    let too_large = invoke(
        "invokable".to_string(),
        "",
        Some("Event"),
        oversized.clone(),
    )
    .await;
    assert_eq!(
        too_large.status,
        413,
        "expected 413 for an asynchronous payload past 1 MB, got {} {}",
        too_large.status,
        String::from_utf8_lossy(&too_large.body)
    );
    let within_sync_limit =
        invoke("invokable".to_string(), "", Some("DryRun"), oversized).await;
    assert_eq!(
        within_sync_limit.status, 204,
        "expected the same payload to be within the synchronous 6 MB limit, got {}",
        within_sync_limit.status
    );

    // The SYNCHRONOUS invocation type reaches the RUN, and this function's deployment
    // package is not a readable archive — "Lambda could not unzip the deployment package",
    // the modelled `InvalidZipFileException` at 502 — so the invocation is refused rather
    // than answered with a payload no function produced.
    for invocation_type in [None, Some("RequestResponse")] {
        let answer = invoke(
            "invokable".to_string(),
            "",
            invocation_type,
            b"{}".to_vec(),
        )
        .await;
        assert_eq!(
            answer.status,
            502,
            "case {invocation_type:?}: expected the modelled 502 for a package that cannot \
             be read, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("InvalidZipFileException"),
            "case {invocation_type:?}: expected InvalidZipFileException, got {}",
            answer.error_code()
        );
    }

    // "Event – Invoke the function asynchronously. […] The API response only includes a
    // status code": the answer is the queueing receipt and says nothing about the run,
    // so it is served — "For the Event invocation type, this status code is 202" — with
    // no payload and no executed version, exactly as `InvokeAsync` answers one.
    for (case, payload) in [
        ("an event document", br#"{"k":1}"#.to_vec()),
        ("no event at all", Vec::new()),
    ] {
        let queued = invoke("invokable".to_string(), "", Some("Event"), payload).await;
        assert_eq!(
            queued.status,
            202,
            "case {case}: expected 202 for an accepted asynchronous invocation, got {} {}",
            queued.status,
            String::from_utf8_lossy(&queued.body)
        );
        assert!(
            queued.body.is_empty(),
            "case {case}: an accepted asynchronous invocation carries no payload, got {:?}",
            String::from_utf8_lossy(&queued.body)
        );
        assert_eq!(
            queued.header("x-amz-executed-version"),
            None,
            "case {case}: no version has executed when the event is queued, got {:?}",
            queued.header("x-amz-executed-version")
        );
    }

    // A function still being provisioned cannot be invoked: "During this time, you
    // can't invoke or modify the function."
    let pending = create_function(
        &router,
        &serde_json::json!({
            "FunctionName": "still-pending",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(8)) },
        }),
    )
    .await;
    assert_eq!(pending.status, 201, "the pending create must succeed");
    let refused = invoke(
        "still-pending".to_string(),
        "",
        Some("DryRun"),
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(
        refused.status,
        409,
        "expected 409 while the function is Pending, got {} {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert!(
        refused.error_code().contains("ResourceConflictException"),
        "expected ResourceConflictException, got {}",
        refused.error_code()
    );
}

/// The `Pending` -> `Active` move the clock owes happens once it is due, and not before.
#[tokio::test]
async fn a_function_becomes_active_when_its_provisioning_is_due() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let created = create_function(
        &router,
        &serde_json::json!({
            "FunctionName": "activating",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(12)) },
        }),
    )
    .await;
    assert_eq!(created.status, 201, "the create must succeed");

    // Before the activation is due, every read still reports the transient state and
    // the reason for it.
    let early = get_function(&router, "activating").await;
    assert_eq!(
        early.json()["Configuration"]["State"],
        serde_json::json!("Pending"),
        "expected Pending before the activation is due, got {:?}",
        early.json()["Configuration"]["State"]
    );
    assert_eq!(
        early.json()["Configuration"]["StateReason"],
        serde_json::json!("The function is being created."),
        "expected the documented StateReason, got {:?}",
        early.json()["Configuration"]["StateReason"]
    );

    make_activation_due(&engine);

    let late = get_function(&router, "activating").await;
    let configuration = late.json()["Configuration"].clone();
    assert_eq!(
        configuration["State"],
        serde_json::json!("Active"),
        "expected Active once the activation is due, got {:?}",
        configuration["State"]
    );
    assert!(
        configuration["StateReason"].is_null() && configuration["StateReasonCode"].is_null(),
        "expected no state reason once the function is Active, got {:?} / {:?}",
        configuration["StateReason"],
        configuration["StateReasonCode"]
    );
    assert_eq!(
        configuration["LastUpdateStatus"],
        serde_json::json!("Successful"),
        "LastUpdateStatus \"is first set to Successful after function creation \
         completes\", got {:?}",
        configuration["LastUpdateStatus"]
    );
    // The transition is reported, not re-run: a second read answers the same thing.
    let again = get_function(&router, "activating").await;
    assert_eq!(
        again.json()["Configuration"]["State"],
        serde_json::json!("Active"),
        "expected a stable Active state across reads, got {:?}",
        again.json()["Configuration"]["State"]
    );
}

/// `DeleteFunction` removes what it says it removes, and a second delete finds nothing.
#[tokio::test]
async fn delete_function_removes_the_record_from_state() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for name in ["keep-me", "delete-me"] {
        create_active_function(&router, &engine, name, &package_bytes(20)).await;
    }
    let published = create_function(
        &router,
        &serde_json::json!({
            "FunctionName": "versioned",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": zip_file(&package_bytes(20)) },
        }),
    )
    .await;
    assert_eq!(published.status, 201, "the published create must succeed");

    let deleted = json_call(
        &router,
        "DELETE",
        "/2015-03-31/functions/delete-me",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "expected 204 from DeleteFunction, got {} {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    assert_eq!(
        get_function(&router, "delete-me").await.status,
        404,
        "expected 404 for a deleted function"
    );
    assert_eq!(
        get_function(&router, "keep-me").await.status,
        200,
        "deleting one function must not touch another"
    );

    let listed: Vec<String> = json_call(
        &router,
        "GET",
        "/2015-03-31/functions",
        &serde_json::Value::Null,
    )
    .await
    .json()["Functions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["FunctionName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !listed.contains(&"delete-me".to_string()) && listed.contains(&"keep-me".to_string()),
        "expected the deleted function to be gone from the list, got {listed:?}"
    );

    // A second delete of the same name finds nothing to delete.
    let again = json_call(
        &router,
        "DELETE",
        "/2015-03-31/functions/delete-me",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        again.status, 404,
        "expected 404 for a function that was already deleted, got {}",
        again.status
    );

    // "To delete a specific function version, use the Qualifier parameter": the
    // function survives, the version does not.
    let version_gone = json_call(
        &router,
        "DELETE",
        "/2015-03-31/functions/versioned?Qualifier=1",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        version_gone.status,
        204,
        "expected 204 when deleting one version, got {} {}",
        version_gone.status,
        String::from_utf8_lossy(&version_gone.body)
    );
    assert_eq!(
        get_function(&router, "versioned").await.status,
        200,
        "deleting a version must leave the function itself"
    );
    assert_eq!(
        json_call(
            &router,
            "GET",
            "/2015-03-31/functions/versioned?Qualifier=1",
            &serde_json::Value::Null,
        )
        .await
        .status,
        404,
        "expected the deleted version to be gone"
    );

    // The unpublished version is not a version a caller can delete on its own.
    let latest = json_call(
        &router,
        "DELETE",
        "/2015-03-31/functions/versioned?Qualifier=$LATEST",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        latest.status,
        400,
        "expected 400 when asked to delete only $LATEST, got {} {}",
        latest.status,
        String::from_utf8_lossy(&latest.body)
    );

    // The store itself holds exactly what the wire reported.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let names: BTreeMap<String, usize> = state
        .lock()
        .functions
        .iter()
        .map(|(name, record)| (name.clone(), record.versions.len()))
        .collect();
    assert_eq!(
        names,
        BTreeMap::from([
            ("keep-me".to_string(), 0),
            ("versioned".to_string(), 0),
        ]),
        "expected only the undeleted functions, with no published versions left, got {names:?}"
    );
}
