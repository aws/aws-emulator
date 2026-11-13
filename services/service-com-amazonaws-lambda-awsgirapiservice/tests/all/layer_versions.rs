// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the layer slice: `PublishLayerVersion`, the `GetLayerVersion`
//! read of what it stored, `DeleteLayerVersion` taking exactly one version back out, and
//! the `Layers` a function's configuration reports for the layer versions it was created
//! with.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than about any handler's internals. Each
//! published version is read back through `GetLayerVersion`, so nothing here trusts the
//! publishing response alone.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

// The announced-link concern, stated once, in the suite that owns it: a download link is
// presigned afresh by every read, so two reads are compared on the object it addresses.
use crate::announced_download_locations::record;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Neither the key id nor the
/// signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `GetAccountSettings` is bound.
const ACCOUNT_SETTINGS_URI: &str = "/2016-08-19/account-settings";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`) of an error response.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }
}

/// Dispatch one JSON request, the shape every operation here uses.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// `PublishLayerVersion` on a layer reference, with the given request body.
async fn publish_layer_version(
    router: &MultiServiceRouter,
    reference: &str,
    body: &Value,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{reference}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `GetLayerVersion` on a layer reference and a version number.
async fn get_layer_version(router: &MultiServiceRouter, reference: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{reference}/versions/{version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `DeleteLayerVersion` on a layer reference and a version number.
async fn delete_layer_version(router: &MultiServiceRouter, reference: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{reference}/versions/{version}");
    json_call(router, "DELETE", &uri, &Value::Null).await
}

/// The `AccountUsage$TotalCodeSize` `GetAccountSettings` reports.
async fn total_code_size(router: &MultiServiceRouter) -> i64 {
    let settings = json_call(router, "GET", ACCOUNT_SETTINGS_URI, &Value::Null).await;
    assert_eq!(
        settings.status,
        200,
        "expected 200 from GetAccountSettings, got {} {}",
        settings.status,
        settings.text()
    );
    settings
        .json()
        .get("AccountUsage")
        .and_then(|usage| usage.get("TotalCodeSize").cloned())
        .and_then(|size| size.as_i64())
        .unwrap_or_else(|| panic!("expected AccountUsage.TotalCodeSize, got {}", settings.text()))
}

/// `GetFunctionConfiguration` on a function name.
async fn get_function_configuration(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/configuration");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// Layer archive bytes of a given length. The content is irrelevant to the API — only
/// its length and its digest are reported — but it differs per length so two archives
/// never share a digest.
fn archive_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `Content.ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The base64 SHA-256 of `bytes` — the independent computation of what `CodeSha256`
/// must report, so the test never reads the value it is checking out of the response.
fn expected_code_sha256(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// The ARN a layer of `name` must carry.
fn layer_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{name}")
}

/// The ARN version `version` of a layer of `name` must carry.
fn layer_version_arn(name: &str, version: i64) -> String {
    format!("{}:{version}", layer_arn(name))
}

/// Assert one member of an answer: `None` means the member must be absent from the
/// answer altogether, not present and empty.
fn assert_member(case: &str, body: &Value, member: &str, expected: Option<Value>) {
    let actual = body.get(member);
    match expected {
        None => assert!(
            actual.is_none(),
            "case {case:?}: expected no {member} member, got {actual:?}"
        ),
        Some(value) => assert_eq!(
            actual,
            Some(&value),
            "case {case:?}: expected {member} {value:?}, got {actual:?}"
        ),
    }
}

/// One row of the publishing table: the layer to publish, the body to publish it with,
/// and the version the call must therefore report and store.
struct PublishCase {
    /// The case name, which is also the layer name.
    name: &'static str,
    /// The `PublishLayerVersion` body.
    body: Value,
    /// The `Content.CodeSize` the answer must report.
    code_size: i64,
    /// The `Content.CodeSha256` the answer must report.
    code_sha256: Option<String>,
    /// The `Content.ResolvedS3Object` the answer must report.
    resolved_s3_object: Option<Value>,
    /// The `Description` the answer must report. `Some(json!(""))` for a request that
    /// carried none: the member is part of every layer version AWS reports from the
    /// version's own read, as the empty string when it was omitted.
    description: Option<Value>,
    /// The `LicenseInfo` the answer must report.
    license_info: Option<Value>,
    /// The `CompatibleRuntimes` the answer must report.
    compatible_runtimes: Option<Value>,
    /// The `CompatibleArchitectures` the answer must report.
    compatible_architectures: Option<Value>,
}

/// `PublishLayerVersion` stores a layer version and reports exactly what it stored, and
/// `GetLayerVersion` reads the same version back.
///
/// The rows differ along what the rule generalises over: where the archive came from
/// (inline bytes of two lengths, an S3 object, a versioned S3 object referenced rather
/// than copied), which optional members the request carries, how many compatible
/// runtimes it names (none, one, the published maximum of 15) and which architectures.
/// The runtime and architecture values are entries of the model's own `Runtime` and
/// `Architecture` enums (`smithy_query(query="shape",shape="Runtime,Architecture")`).
#[tokio::test]
async fn publish_layer_version_stores_the_archive_and_reports_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let small = archive_bytes(64);
    let large = archive_bytes(4_096);

    // Fifteen entries: the `CompatibleRuntimes` length maximum the model publishes
    // (`@length(min: 0, max: 15)`), spelled with values from the `Runtime` enum.
    let fifteen_runtimes = vec![
        "nodejs18.x",
        "nodejs20.x",
        "nodejs22.x",
        "python3.9",
        "python3.10",
        "python3.11",
        "python3.12",
        "python3.13",
        "java11",
        "java17",
        "java21",
        "ruby3.2",
        "ruby3.3",
        "dotnet8",
        "provided.al2023",
    ];

    let cases = vec![
        // The plain case: an inline archive and no optional member at all.
        PublishCase {
            name: "inline-only",
            body: json!({ "Content": { "ZipFile": zip_file(&small) } }),
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            resolved_s3_object: None,
            description: Some(json!("")),
            license_info: None,
            compatible_runtimes: None,
            compatible_architectures: None,
        },
        // A larger archive with every optional member named: an SPDX license
        // identifier, one runtime and one architecture.
        PublishCase {
            name: "inline-described",
            body: json!({
                "Content": { "ZipFile": zip_file(&large) },
                "Description": "shared helpers",
                "LicenseInfo": "MIT",
                "CompatibleRuntimes": ["python3.12"],
                "CompatibleArchitectures": ["arm64"],
            }),
            code_size: i64::try_from(large.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&large)),
            resolved_s3_object: None,
            description: Some(json!("shared helpers")),
            license_info: Some(json!("MIT")),
            compatible_runtimes: Some(json!(["python3.12"])),
            compatible_architectures: Some(json!(["arm64"])),
        },
        // The list bounds: fifteen compatible runtimes and both architectures, with the
        // license given as the URL form the member's documentation names.
        PublishCase {
            name: "inline-every-runtime",
            body: json!({
                "Content": { "ZipFile": zip_file(&small) },
                "LicenseInfo": "https://opensource.org/licenses/MIT",
                "CompatibleRuntimes": fifteen_runtimes.clone(),
                "CompatibleArchitectures": ["x86_64", "arm64"],
            }),
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            resolved_s3_object: None,
            description: Some(json!("")),
            license_info: Some(json!("https://opensource.org/licenses/MIT")),
            compatible_runtimes: Some(json!(fifteen_runtimes)),
            compatible_architectures: Some(json!(["x86_64", "arm64"])),
        },
        // An S3-sourced archive: this emulator holds no bytes for it, so it reports no
        // size it did not measure and no digest it did not compute. The default
        // storage mode copies the archive to Lambda, so the caller's object is no
        // longer the one that serves it.
        PublishCase {
            name: "from-s3-copied",
            body: json!({
                "Content": { "S3Bucket": "layer-archives", "S3Key": "helpers.zip" },
                "Description": "from s3",
            }),
            code_size: 0,
            code_sha256: None,
            resolved_s3_object: None,
            description: Some(json!("from s3")),
            license_info: None,
            compatible_runtimes: None,
            compatible_architectures: None,
        },
        // "REFERENCE – Lambda references the layer archive from the specified Amazon S3
        // bucket", so the resolved object is the caller's versioned object.
        PublishCase {
            name: "from-s3-referenced",
            body: json!({
                "Content": {
                    "S3Bucket": "layer-archives",
                    "S3Key": "helpers.zip",
                    "S3ObjectVersion": "3HL4kqtJlcpXroDTDmJ",
                    "S3ObjectStorageMode": "REFERENCE",
                },
                "CompatibleRuntimes": ["provided.al2"],
            }),
            code_size: 0,
            code_sha256: None,
            resolved_s3_object: Some(json!({
                "S3Bucket": "layer-archives",
                "S3Key": "helpers.zip",
                "S3ObjectVersion": "3HL4kqtJlcpXroDTDmJ",
            })),
            description: Some(json!("")),
            license_info: None,
            compatible_runtimes: Some(json!(["provided.al2"])),
            compatible_architectures: None,
        },
    ];

    for case in &cases {
        let answer = publish_layer_version(&router, case.name, &case.body).await;
        assert_eq!(
            answer.status,
            201,
            "case {:?}: expected 201 from PublishLayerVersion, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        let published = answer.json();

        // Every layer starts at version 1, and both ARNs are minted from the layer's
        // own name and region.
        assert_member(case.name, &published, "Version", Some(json!(1)));
        assert_member(
            case.name,
            &published,
            "LayerArn",
            Some(json!(layer_arn(case.name))),
        );
        assert_member(
            case.name,
            &published,
            "LayerVersionArn",
            Some(json!(layer_version_arn(case.name, 1))),
        );
        assert_member(case.name, &published, "Description", case.description.clone());
        assert_member(
            case.name,
            &published,
            "LicenseInfo",
            case.license_info.clone(),
        );
        assert_member(
            case.name,
            &published,
            "CompatibleRuntimes",
            case.compatible_runtimes.clone(),
        );
        assert_member(
            case.name,
            &published,
            "CompatibleArchitectures",
            case.compatible_architectures.clone(),
        );

        // "The date that the layer version was created, in ISO-8601 format".
        let created = published
            .get("CreatedDate")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            created.contains('T') && created.ends_with("+0000"),
            "case {:?}: expected a CreatedDate in the form the member documents \
             (\"in ISO 8601 format. For example, 2018-11-27T15:10:45.123+0000\"), got \
             {created:?}",
            case.name
        );

        let content = published
            .get("Content")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: expected a Content member", case.name));
        assert_member(case.name, &content, "CodeSize", Some(json!(case.code_size)));
        assert_member(
            case.name,
            &content,
            "CodeSha256",
            case.code_sha256.clone().map(Value::String),
        );
        assert_member(
            case.name,
            &content,
            "ResolvedS3Object",
            case.resolved_s3_object.clone(),
        );
        let location = content
            .get("Location")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            location.contains(case.name) && location.starts_with("https://"),
            "case {:?}: expected a download Location naming the layer, got {location:?}",
            case.name
        );

        // The round trip: the stored version reads back exactly as it was reported, and
        // reading it twice reports the same thing, because a read changes nothing.
        let first = get_layer_version(&router, case.name, 1).await;
        assert_eq!(
            first.status,
            200,
            "case {:?}: expected 200 from GetLayerVersion, got {} {}",
            case.name,
            first.status,
            first.text()
        );
        assert_eq!(
            record(case.name, &first.json()),
            record(case.name, &published),
            "case {:?}: expected GetLayerVersion to report the published version, got {}",
            case.name,
            first.text()
        );
        let second = get_layer_version(&router, case.name, 1).await;
        assert_eq!(
            record(case.name, &second.json()),
            record(case.name, &first.json()),
            "case {:?}: expected two reads to report the same version, got {} then {}",
            case.name,
            first.text(),
            second.text()
        );
    }
}

/// "Each time you call PublishLayerVersion with the same layer name, a new version is
/// created": the numbers count up per layer, an earlier version keeps the archive it
/// froze, and a layer named by its ARN is the same record as the one named by its name.
#[tokio::test]
async fn each_publish_under_one_name_creates_the_next_version() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Three archives of different lengths, so every version has its own digest, and a
    // fourth published through the layer's ARN rather than its name.
    let archives = [
        archive_bytes(32),
        archive_bytes(256),
        archive_bytes(1_024),
        archive_bytes(2_048),
    ];
    let references = [
        "counted".to_string(),
        "counted".to_string(),
        "counted".to_string(),
        layer_arn("counted"),
    ];

    for (index, (archive, reference)) in archives.iter().zip(references.iter()).enumerate() {
        let expected_version = i64::try_from(index + 1).unwrap_or_default();
        let answer = publish_layer_version(
            &router,
            reference,
            &json!({ "Content": { "ZipFile": zip_file(archive) } }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {reference:?}: expected 201 from PublishLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
        assert_member(
            reference,
            &answer.json(),
            "Version",
            Some(json!(expected_version)),
        );
        assert_member(
            reference,
            &answer.json(),
            "LayerVersionArn",
            Some(json!(layer_version_arn("counted", expected_version))),
        );
    }

    // Every version is still readable, and still reports the archive it was published
    // from: a later publish freezes nothing of an earlier version.
    for (index, archive) in archives.iter().enumerate() {
        let version = i64::try_from(index + 1).unwrap_or_default();
        let case = format!("counted:{version}");
        let answer = get_layer_version(&router, "counted", version).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200 from GetLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
        let content = answer.json().get("Content").cloned().unwrap_or_default();
        assert_member(
            &case,
            &content,
            "CodeSha256",
            Some(json!(expected_code_sha256(archive))),
        );
        assert_member(
            &case,
            &content,
            "CodeSize",
            Some(json!(i64::try_from(archive.len()).unwrap_or_default())),
        );
    }

    // A different layer name is a different record, so its numbering starts again.
    let other = publish_layer_version(
        &router,
        "separate",
        &json!({ "Content": { "ZipFile": zip_file(&archives[0]) } }),
    )
    .await;
    assert_member(
        "separate",
        &other.json(),
        "Version",
        Some(json!(1)),
    );
}

/// A `Content` that names no single archive, or a request that breaks a modelled
/// constraint, is refused — never stored and never answered 501.
#[tokio::test]
async fn publish_layer_version_refuses_a_content_it_cannot_store() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let archive = archive_bytes(64);
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        // "You can specify either an Amazon S3 location, or upload a layer archive
        // directly" — so not both, and not neither.
        (
            "both-sources",
            json!({
                "Content": {
                    "ZipFile": zip_file(&archive),
                    "S3Bucket": "layer-archives",
                    "S3Key": "helpers.zip",
                },
            }),
            Some("InvalidParameterValueException"),
        ),
        (
            "no-source",
            json!({ "Content": {} }),
            Some("InvalidParameterValueException"),
        ),
        // An S3 location is one object: its bucket AND its key.
        (
            "bucket-without-key",
            json!({ "Content": { "S3Bucket": "layer-archives" } }),
            Some("InvalidParameterValueException"),
        ),
        (
            "key-without-bucket",
            json!({ "Content": { "S3Key": "helpers.zip" } }),
            Some("InvalidParameterValueException"),
        ),
        // One past the published maximum of 15 compatible runtimes. The modelled
        // constraint is refused before the handler runs, so only the status is asserted.
        (
            "sixteen-runtimes",
            json!({
                "Content": { "ZipFile": zip_file(&archive) },
                "CompatibleRuntimes": [
                    "nodejs18.x", "nodejs20.x", "nodejs22.x", "python3.9", "python3.10",
                    "python3.11", "python3.12", "python3.13", "java11", "java17",
                    "java21", "ruby3.2", "ruby3.3", "dotnet8", "provided.al2023",
                    "provided.al2",
                ],
            }),
            None,
        ),
    ];

    for (case, body, expected_code) in &cases {
        let answer = publish_layer_version(&router, "refused", body).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 from PublishLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
        if let Some(code) = expected_code {
            let actual = answer.error_code();
            assert!(
                actual.contains(code),
                "case {case:?}: expected the {code} error code, got {actual:?}"
            );
        }
        // Nothing was stored, so there is no version 1 of this layer to read.
        let read = get_layer_version(&router, "refused", 1).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected the refused layer to hold no version, got {} {}",
            read.status,
            read.text()
        );
    }
}

/// `GetLayerVersion` answers the modelled 404 for every layer or version the store does
/// not hold, and does not create one by being asked for it.
#[tokio::test]
async fn get_layer_version_reports_a_version_the_store_does_not_hold_as_missing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let archive = archive_bytes(128);
    let published = publish_layer_version(
        &router,
        "present",
        &json!({ "Content": { "ZipFile": zip_file(&archive) } }),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "the layer under test must publish: got {} {}",
        published.status,
        published.text()
    );

    // The rows differ along what is missing: the layer, the version number, and the
    // boundary just past the one version that exists.
    let cases: Vec<(&str, String, i64)> = vec![
        ("unknown layer", "absent".to_string(), 1),
        ("unknown layer by ARN", layer_arn("absent"), 1),
        ("one past the last version", "present".to_string(), 2),
        ("a much later version", "present".to_string(), 99),
    ];

    for (case, reference, version) in &cases {
        let answer = get_layer_version(&router, reference, *version).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404 from GetLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
        let code = answer.error_code();
        assert!(
            code.contains("ResourceNotFoundException"),
            "case {case:?}: expected the ResourceNotFoundException error code, got {code:?}"
        );
    }

    // The reads above changed nothing: the one version that exists still does, and the
    // layer still holds exactly it.
    let read = get_layer_version(&router, "present", 1).await;
    assert_eq!(
        read.status,
        200,
        "expected the published version to survive the missing reads, got {} {}",
        read.status,
        read.text()
    );
}

/// A function reports the layer versions it was created with, and a function created
/// with none reports no `Layers` member at all.
///
/// The rows differ along the count and the order of the layers named, and along where
/// the layer comes from: a version this account published (whose archive size is the
/// one it was published with) and a layer Lambda itself provides.
#[tokio::test]
async fn a_function_reports_the_layer_versions_it_was_created_with() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let first_archive = archive_bytes(512);
    let second_archive = archive_bytes(2_048);
    // Two DIFFERENT layers: a function's layer list may name at most one version of any
    // one layer, so the row that pins the ORDER of the list names one version of each.
    for (name, archive) in [("shared", &first_archive), ("helpers", &second_archive)] {
        let answer = publish_layer_version(
            &router,
            name,
            &json!({ "Content": { "ZipFile": zip_file(archive) } }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "the layers under test must publish: got {} {}",
            answer.status,
            answer.text()
        );
    }
    let first = layer_version_arn("shared", 1);
    let second = layer_version_arn("helpers", 1);
    // `arn:<partition>:lambda:::awslayer:<name>` is the second form the model's
    // `LayerVersionArn` pattern admits: a layer Lambda provides, which belongs to no
    // account and whose archive this emulator holds none of.
    let managed = "arn:aws:lambda:::awslayer:AmazonLinux1803";
    let first_size = i64::try_from(first_archive.len()).unwrap_or_default();
    let second_size = i64::try_from(second_archive.len()).unwrap_or_default();

    let cases: Vec<(&str, Value, Option<Value>)> = vec![
        ("no-layers", Value::Null, None),
        (
            // `UncompressedCodeSize` is the size of the layer archive's CONTENTS: the
            // archives these rows publish are bytes this emulator cannot read as an
            // archive, so there is nothing to expand and the member reports 0 beside the
            // stored size. The member tracking a real archive's contents is pinned in
            // `function_layer_unzipped_size`.
            "one-layer",
            json!([first]),
            Some(json!([
                { "Arn": first, "CodeSize": first_size, "UncompressedCodeSize": 0 },
            ])),
        ),
        (
            "two-layers-in-order",
            json!([second, first]),
            Some(json!([
                { "Arn": second, "CodeSize": second_size, "UncompressedCodeSize": 0 },
                { "Arn": first, "CodeSize": first_size, "UncompressedCodeSize": 0 },
            ])),
        ),
        (
            "aws-managed-layer",
            json!([managed]),
            Some(json!([
                { "Arn": managed, "CodeSize": 0, "UncompressedCodeSize": 0 },
            ])),
        ),
    ];

    for (case, layers, expected) in &cases {
        let mut body = Map::new();
        body.insert("FunctionName".to_string(), json!(case));
        body.insert("Role".to_string(), json!(ROLE));
        body.insert("Runtime".to_string(), json!("python3.12"));
        body.insert("Handler".to_string(), json!("index.handler"));
        body.insert(
            "Code".to_string(),
            json!({ "ZipFile": zip_file(&archive_bytes(64)) }),
        );
        if !layers.is_null() {
            body.insert("Layers".to_string(), layers.clone());
        }
        let created = json_call(&router, "POST", FUNCTIONS_URI, &Value::Object(body)).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201 from CreateFunction, got {} {}",
            created.status,
            created.text()
        );
        assert_member(case, &created.json(), "Layers", expected.clone());

        // Read back through `GetFunctionConfiguration`, so the layers reported are the
        // ones the store holds rather than the ones the create call echoed.
        let configuration = get_function_configuration(&router, case).await;
        assert_eq!(
            configuration.status,
            200,
            "case {case:?}: expected 200 from GetFunctionConfiguration, got {} {}",
            configuration.status,
            configuration.text()
        );
        assert_member(case, &configuration.json(), "Layers", expected.clone());

        // A published version freezes the layers the configuration held.
        let published = json_call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{case}/versions"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "case {case:?}: expected 201 from PublishVersion, got {} {}",
            published.status,
            published.text()
        );
        assert_member(case, &published.json(), "Layers", expected.clone());
    }

    // A `Layers` entry naming a layer version this account has not published names no
    // layer at all, and a function is not created for it.
    let missing = json_call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": "missing-layer",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&archive_bytes(64)) },
            "Layers": [layer_version_arn("shared", 9)],
        }),
    )
    .await;
    assert_eq!(
        missing.status,
        400,
        "case \"missing-layer\": expected 400 from CreateFunction, got {} {}",
        missing.status,
        missing.text()
    );
    let code = missing.error_code();
    assert!(
        code.contains("InvalidParameterValueException"),
        "case \"missing-layer\": expected the InvalidParameterValueException error code, \
         got {code:?}"
    );
    let configuration = get_function_configuration(&router, "missing-layer").await;
    assert_eq!(
        configuration.status,
        404,
        "case \"missing-layer\": expected the refused function not to exist, got {} {}",
        configuration.status,
        configuration.text()
    );
}

/// `AccountUsage$TotalCodeSize` is "The amount of storage space, in bytes, that's being
/// used by deployment packages and layer archives", so every published layer archive is
/// counted beside the deployment packages.
#[tokio::test]
async fn account_usage_counts_the_published_layer_archives() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let package = archive_bytes(1_024);
    let archives = [archive_bytes(256), archive_bytes(4_096)];

    let created = json_call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": "counted",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the function under test must be created: got {} {}",
        created.status,
        created.text()
    );

    let mut expected = i64::try_from(package.len()).unwrap_or_default();
    for (index, archive) in archives.iter().enumerate() {
        let answer = publish_layer_version(
            &router,
            "counted-layer",
            &json!({ "Content": { "ZipFile": zip_file(archive) } }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {index}: expected 201 from PublishLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
        expected += i64::try_from(archive.len()).unwrap_or_default();

        let actual = total_code_size(&router).await;
        assert_eq!(
            actual,
            expected,
            "case {:?}: expected TotalCodeSize {expected}, got {actual}",
            format!("after {} layer version(s)", index + 1)
        );
    }
}

/// Publish one version per archive under `layer`, asserting each call reports the next
/// version number counting up from `first_version`.
async fn publish_each(
    router: &MultiServiceRouter,
    layer: &str,
    archives: &[Vec<u8>],
    first_version: i64,
) {
    for (index, archive) in archives.iter().enumerate() {
        let expected = first_version + i64::try_from(index).unwrap_or_default();
        let answer = publish_layer_version(
            router,
            layer,
            &json!({ "Content": { "ZipFile": zip_file(archive) } }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {:?}: expected 201 from PublishLayerVersion, got {} {}",
            format!("{layer}:{expected}"),
            answer.status,
            answer.text()
        );
        assert_member(
            &format!("{layer}:{expected}"),
            &answer.json(),
            "Version",
            Some(json!(expected)),
        );
    }
}

/// The archive length of version `version` of a layer published from `archives`, as the
/// test holds them: version numbers count up from 1, so version `n` is the `n`th archive.
fn archive_size(archives: &[Vec<u8>], version: i64) -> i64 {
    let index = usize::try_from(version - 1).unwrap_or_default();
    i64::try_from(archives[index].len()).unwrap_or_default()
}

/// One row of the deletion table: the version to delete, how its layer is addressed, and
/// the versions of that layer the store must still hold afterwards.
struct DeleteCase {
    /// The case name.
    case: &'static str,
    /// The `LayerName` the delete addresses the layer by.
    reference: String,
    /// The version number to delete.
    version: i64,
    /// Every version of the layer that must still be readable after the delete.
    remaining: Vec<i64>,
}

/// "Deletes a version of an Lambda layer. Deleted versions can no longer be viewed":
/// a delete takes exactly the version it names out of the store, leaves every other
/// version of the same layer readable, and stops the deleted archive counting towards the
/// account's used code storage.
///
/// The rows differ along which version is deleted (one in the middle, the latest, the
/// first, the last one left), how the layer is addressed (its bare name or its ARN, the
/// two forms `LayerName` documents), and whether the version is there at all — the repeat
/// of an already deleted version, which the `@idempotent` operation answers just the same.
#[tokio::test]
async fn delete_layer_version_removes_only_the_version_it_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Four versions of one layer, each published from an archive of its own length, so a
    // read's digest says which version answered it.
    let archives = [
        archive_bytes(32),
        archive_bytes(256),
        archive_bytes(1_024),
        archive_bytes(2_048),
    ];
    publish_each(&router, "pruned", &archives, 1).await;

    let cases = vec![
        DeleteCase {
            case: "a middle version by name",
            reference: "pruned".to_string(),
            version: 2,
            remaining: vec![1, 3, 4],
        },
        DeleteCase {
            case: "the latest version by ARN",
            reference: layer_arn("pruned"),
            version: 4,
            remaining: vec![1, 3],
        },
        DeleteCase {
            case: "a version already deleted",
            reference: "pruned".to_string(),
            version: 2,
            remaining: vec![1, 3],
        },
        DeleteCase {
            case: "the first version",
            reference: "pruned".to_string(),
            version: 1,
            remaining: vec![3],
        },
        DeleteCase {
            case: "the last version left",
            reference: layer_arn("pruned"),
            version: 3,
            remaining: vec![],
        },
    ];

    for case in &cases {
        let answer = delete_layer_version(&router, &case.reference, case.version).await;
        assert_eq!(
            answer.status,
            204,
            "case {:?}: expected 204 from DeleteLayerVersion, got {} {}",
            case.case,
            answer.status,
            answer.text()
        );
        assert!(
            answer.body.is_empty(),
            "case {:?}: expected an empty body on the 204, got {:?}",
            case.case,
            answer.text()
        );

        // Every version of the layer, read back: the ones the row leaves behind still
        // report the archive they were published from, and the deleted ones are gone.
        for version in 1..=i64::try_from(archives.len()).unwrap_or_default() {
            let read = get_layer_version(&router, "pruned", version).await;
            if case.remaining.contains(&version) {
                assert_eq!(
                    read.status,
                    200,
                    "case {:?}: expected version {version} to survive, got {} {}",
                    case.case,
                    read.status,
                    read.text()
                );
                let content = read.json().get("Content").cloned().unwrap_or_default();
                let index = usize::try_from(version - 1).unwrap_or_default();
                assert_member(
                    case.case,
                    &content,
                    "CodeSha256",
                    Some(json!(expected_code_sha256(&archives[index]))),
                );
            } else {
                assert_eq!(
                    read.status,
                    404,
                    "case {:?}: expected version {version} to be deleted, got {} {}",
                    case.case,
                    read.status,
                    read.text()
                );
                let code = read.error_code();
                assert!(
                    code.contains("ResourceNotFoundException"),
                    "case {:?}: expected ResourceNotFoundException for version {version}, \
                     got {code:?}",
                    case.case
                );
            }
        }

        // The deleted archive is no longer storage this account uses, so the usage the
        // account reports is the sum of what the layer still holds.
        let expected: i64 = case
            .remaining
            .iter()
            .map(|version| archive_size(&archives, *version))
            .sum();
        let actual = total_code_size(&router).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected TotalCodeSize {expected} after the delete, got {actual}",
            case.case
        );
    }
}

/// The operation is `@idempotent`, so deleting a version the store does not hold is a
/// success that removes nothing: the post-condition already holds. It also creates
/// nothing by being asked, and refuses a `LayerName` that names no layer.
///
/// The rows differ along what is missing: the layer (by name and by ARN), the version
/// number (one past the last, the member's own default of 0, a much later one).
#[tokio::test]
async fn delete_layer_version_is_idempotent_on_a_version_the_store_does_not_hold() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let archive = archive_bytes(128);
    publish_each(&router, "kept", std::slice::from_ref(&archive), 1).await;

    let cases: Vec<(&str, String, i64)> = vec![
        ("unknown layer", "absent".to_string(), 1),
        ("unknown layer by ARN", layer_arn("absent"), 1),
        ("one past the last version", "kept".to_string(), 2),
        ("the VersionNumber default", "kept".to_string(), 0),
        ("a much later version", "kept".to_string(), 99),
    ];

    for (case, reference, version) in &cases {
        let answer = delete_layer_version(&router, reference, *version).await;
        assert_eq!(
            answer.status,
            204,
            "case {case:?}: expected 204 from DeleteLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.body.is_empty(),
            "case {case:?}: expected an empty body on the 204, got {:?}",
            answer.text()
        );

        // Nothing was removed: the one version that exists still does, and still reports
        // its own archive.
        let read = get_layer_version(&router, "kept", 1).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the published version to survive, got {} {}",
            read.status,
            read.text()
        );
        let content = read.json().get("Content").cloned().unwrap_or_default();
        assert_member(
            case,
            &content,
            "CodeSha256",
            Some(json!(expected_code_sha256(&archive))),
        );
        // And nothing was created: the version the row named is still absent.
        let absent = get_layer_version(&router, reference, *version).await;
        assert_eq!(
            absent.status,
            404,
            "case {case:?}: expected the deleted version to stay absent, got {} {}",
            absent.status,
            absent.text()
        );
    }

    // A `LayerName` that is well formed for the member's pattern but names a Lambda
    // resource of another kind names no layer, so the call is refused rather than
    // reported as a delete of nothing.
    let refused = delete_layer_version(
        &router,
        "arn:aws:lambda:us-east-1:000000000000:function:not-a-layer",
        1,
    )
    .await;
    assert_eq!(
        refused.status,
        400,
        "case \"not a layer name\": expected 400 from DeleteLayerVersion, got {} {}",
        refused.status,
        refused.text()
    );
}

/// One row of the renumbering table: how many versions the layer holds before the
/// delete, which of them are deleted, and the version number the next publish must
/// therefore report.
struct RenumberCase {
    /// The case name, which is also the layer name.
    case: &'static str,
    /// The versions published before any delete.
    published_before: i64,
    /// The versions deleted, in order.
    deleted: Vec<i64>,
    /// The version number the publish after those deletes must report.
    expected_next: i64,
}

/// A deleted version number is never handed out again: the next `PublishLayerVersion`
/// counts from the highest number ever published under the layer's name, not from the
/// versions the layer still holds.
///
/// The rows differ along how many versions existed and which were deleted: the only one,
/// the latest, one in the middle, and every one of them.
#[tokio::test]
async fn a_deleted_layer_version_number_is_not_reused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let cases = vec![
        RenumberCase {
            case: "only-version",
            published_before: 1,
            deleted: vec![1],
            expected_next: 2,
        },
        RenumberCase {
            case: "latest-version",
            published_before: 3,
            deleted: vec![3],
            expected_next: 4,
        },
        RenumberCase {
            case: "middle-version",
            published_before: 3,
            deleted: vec![2],
            expected_next: 4,
        },
        RenumberCase {
            case: "every-version",
            published_before: 3,
            deleted: vec![1, 2, 3],
            expected_next: 4,
        },
    ];

    for case in &cases {
        let archives: Vec<Vec<u8>> = (1..=case.published_before)
            .map(|version| archive_bytes(64 * usize::try_from(version).unwrap_or(1)))
            .collect();
        publish_each(&router, case.case, &archives, 1).await;

        for version in &case.deleted {
            let answer = delete_layer_version(&router, case.case, *version).await;
            assert_eq!(
                answer.status,
                204,
                "case {:?}: expected 204 deleting version {version}, got {} {}",
                case.case,
                answer.status,
                answer.text()
            );
        }

        let next = archive_bytes(4_096);
        let published = publish_layer_version(
            &router,
            case.case,
            &json!({ "Content": { "ZipFile": zip_file(&next) } }),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "case {:?}: expected 201 from PublishLayerVersion, got {} {}",
            case.case,
            published.status,
            published.text()
        );
        assert_member(
            case.case,
            &published.json(),
            "Version",
            Some(json!(case.expected_next)),
        );
        assert_member(
            case.case,
            &published.json(),
            "LayerVersionArn",
            Some(json!(layer_version_arn(case.case, case.expected_next))),
        );

        // The new version is readable under its own number, and every deleted number
        // stays deleted rather than being answered by the version just published.
        let read = get_layer_version(&router, case.case, case.expected_next).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected 200 reading version {}, got {} {}",
            case.case,
            case.expected_next,
            read.status,
            read.text()
        );
        assert_member(
            case.case,
            &read.json().get("Content").cloned().unwrap_or_default(),
            "CodeSha256",
            Some(json!(expected_code_sha256(&next))),
        );
        for version in &case.deleted {
            let gone = get_layer_version(&router, case.case, *version).await;
            assert_eq!(
                gone.status,
                404,
                "case {:?}: expected version {version} to stay deleted, got {} {}",
                case.case,
                gone.status,
                gone.text()
            );
        }
    }
}

/// "To avoid breaking functions, a copy of the version remains in Lambda until no
/// functions refer to it" — so deleting a version a function already holds succeeds and
/// leaves that function's configuration exactly as it was, while "deleted versions can no
/// longer be viewed or added to functions": a read of it answers the modelled 404 and a
/// new function naming it is refused.
#[tokio::test]
async fn deleting_a_layer_version_a_function_refers_to_leaves_that_function_alone() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let archives = [archive_bytes(512), archive_bytes(1_024)];
    // TWO layers, one version each: a function's layer list may name at most one version
    // of any one layer, and what this test needs is a version the delete removes beside
    // one it leaves alone.
    publish_each(&router, "referenced", &archives[..1], 1).await;
    publish_each(&router, "companion", &archives[1..], 1).await;
    let first = layer_version_arn("referenced", 1);
    let second = layer_version_arn("companion", 1);
    let expected_layers = json!([
        // The archives here are bytes this emulator cannot read as an archive, so the
        // expanded size of their contents is 0 (see `function_layer_unzipped_size`).
        { "Arn": first, "CodeSize": archive_size(&archives, 1), "UncompressedCodeSize": 0 },
        { "Arn": second, "CodeSize": archive_size(&archives, 2), "UncompressedCodeSize": 0 },
    ]);

    let created = json_call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": "holder",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&archive_bytes(64)) },
            "Layers": [first, second],
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the function under test must be created: got {} {}",
        created.status,
        created.text()
    );

    let deleted = delete_layer_version(&router, "referenced", 1).await;
    assert_eq!(
        deleted.status,
        204,
        "case \"referenced version\": expected 204 from DeleteLayerVersion, got {} {}",
        deleted.status,
        deleted.text()
    );

    // The function still refers to the version it was configured with, archive size and
    // all: the delete breaks nothing that already holds the version.
    let configuration = get_function_configuration(&router, "holder").await;
    assert_eq!(
        configuration.status,
        200,
        "case \"referenced version\": expected 200 from GetFunctionConfiguration, got {} {}",
        configuration.status,
        configuration.text()
    );
    assert_member(
        "referenced version",
        &configuration.json(),
        "Layers",
        Some(expected_layers),
    );

    // The deleted version can no longer be viewed.
    let read = get_layer_version(&router, "referenced", 1).await;
    assert_eq!(
        read.status,
        404,
        "case \"referenced version\": expected the deleted version to be gone, got {} {}",
        read.status,
        read.text()
    );

    // Nor added to a function: a new function naming it is refused, while one naming the
    // version that was not deleted is created, so the refusal is about the deleted
    // version alone.
    let cases: Vec<(&str, &str, u16)> = vec![
        ("newcomer-on-deleted", first.as_str(), 400),
        ("newcomer-on-surviving", second.as_str(), 201),
    ];
    for (case, layer, expected_status) in &cases {
        let answer = json_call(
            &router,
            "POST",
            FUNCTIONS_URI,
            &json!({
                "FunctionName": case,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&archive_bytes(64)) },
                "Layers": [layer],
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            *expected_status,
            "case {case:?}: expected {expected_status} from CreateFunction, got {} {}",
            answer.status,
            answer.text()
        );
        if *expected_status == 400 {
            let code = answer.error_code();
            assert!(
                code.contains("InvalidParameterValueException"),
                "case {case:?}: expected the InvalidParameterValueException error code, \
                 got {code:?}"
            );
        }
    }
}
