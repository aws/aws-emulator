// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetLayerVersionByArn`: the read that addresses one layer
//! version by the version's own ARN instead of by a layer name and a version number.
//!
//! The rule under test is that an ARN identifying a layer version addresses the same
//! stored version the name-and-number read addresses, so the two reads report the same
//! record member for member; an ARN in neither form the model's `LayerVersionArn`
//! admits is refused for that constraint, and a well-formed one naming a version this
//! account holds none of is reported missing.
//!
//! One member of that record is not stored state: `Content.Location` is "A link to the
//! layer archive in Amazon S3 that is valid for 10 minutes", so every read presigns one
//! afresh and the link carries the instant of THAT read (`X-Amz-Date`, the date in the
//! credential scope) and a signature over it. Those belong to the read; the object the
//! link addresses and the link's terms come from the stored version. So the record
//! comparisons below set the signing instant and its signature aside — see
//! [`addressed_object`] — and assert separately that each read announced a presigned link
//! to the same object. A comparison that included the instant would answer by whether two
//! reads happened to land in the same second, which is how fast the host runs.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than about any handler's internals.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The announced-link concern, stated once, in the suite that owns it.
use crate::announced_download_locations::{addressed_object, record};

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

/// The wire error code a refusal of a value outside a modelled constraint carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The wire error code a read of a resource this account does not hold carries.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

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

    /// The `message` of an error response.
    fn error_message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
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

/// Percent-encode the characters an ARN carries that a query string would otherwise
/// read as structure.
fn encoded(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace(' ', "%20")
        .replace('+', "%2B")
        .replace('#', "%23")
}

/// `PublishLayerVersion` on a layer name, with the given request body.
async fn publish_layer_version(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `DeleteLayerVersion` on a layer name and a version number.
async fn delete_layer_version(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, "DELETE", &uri, &Value::Null).await
}

/// `GetLayerVersion` on a layer name and a version number.
async fn get_layer_version(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetLayerVersionByArn`, whose `Arn` member is bound to the `Arn` query parameter of
/// `GET /2018-10-31/layers?find=LayerVersion`.
async fn get_layer_version_by_arn(router: &MultiServiceRouter, arn: &str) -> Answer {
    let uri = format!("/2018-10-31/layers?find=LayerVersion&Arn={}", encoded(arn));
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

/// The ARN a layer of `name` must carry.
fn layer_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{name}")
}

/// The ARN version `version` of a layer of `name` must carry.
fn layer_version_arn(name: &str, version: i64) -> String {
    format!("{}:{version}", layer_arn(name))
}

/// One published layer version: the layer it belongs to, the body it was published
/// with, and the version number it was given.
struct Published {
    /// The case name.
    case: &'static str,
    /// The layer name the version was published under.
    layer: &'static str,
    /// The version number the publish assigned.
    version: i64,
    /// What the publish itself answered, which announces a download link of its own.
    announced: Value,
}

/// Publish the table of layer versions every read suite below addresses, spanning what
/// the rule generalises over: where the archive came from, which optional members the
/// version carries, how many runtimes and architectures it declares, and which version
/// number it got (a first version, a later one, the newest of several).
async fn published_versions(router: &MultiServiceRouter) -> Vec<Published> {
    let small = archive_bytes(64);
    let large = archive_bytes(2_048);

    // Publish requests, in order, as (case, layer, body).
    let requests: Vec<(&'static str, &'static str, Value)> = vec![
        (
            "a first version with no optional member",
            "plain-layer",
            json!({ "Content": { "ZipFile": zip_file(&small) } }),
        ),
        (
            "a version with every optional member",
            "described-layer",
            json!({
                "Content": { "ZipFile": zip_file(&large) },
                "Description": "shared helpers",
                "LicenseInfo": "MIT",
                "CompatibleRuntimes": ["python3.12"],
                "CompatibleArchitectures": ["arm64"],
            }),
        ),
        (
            "the first of two versions of one layer",
            "versioned-layer",
            json!({
                "Content": { "ZipFile": zip_file(&small) },
                "Description": "the first",
                "CompatibleRuntimes": ["nodejs20.x", "nodejs22.x"],
            }),
        ),
        (
            "the newest of two versions of one layer",
            "versioned-layer",
            json!({
                "Content": { "ZipFile": zip_file(&large) },
                "Description": "the second",
                "LicenseInfo": "https://opensource.org/licenses/MIT",
                "CompatibleArchitectures": ["x86_64", "arm64"],
            }),
        ),
        (
            "a version published from an Amazon S3 object",
            "s3-layer",
            json!({
                "Content": { "S3Bucket": "layer-archives", "S3Key": "helpers.zip" },
                "Description": "from s3",
            }),
        ),
        (
            "a version referencing an Amazon S3 object",
            "referenced-layer",
            json!({
                "Content": {
                    "S3Bucket": "layer-archives",
                    "S3Key": "helpers.zip",
                    "S3ObjectVersion": "3HL4kqtJlcpXroDTDmJ",
                    "S3ObjectStorageMode": "REFERENCE",
                },
                "CompatibleRuntimes": ["provided.al2"],
            }),
        ),
    ];

    let mut published = Vec::new();
    for (case, layer, body) in requests {
        let answer = publish_layer_version(router, layer, &body).await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: expected 201 from PublishLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
        let version = answer
            .json()
            .get("Version")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a Version member, got {}",
                    answer.text()
                )
            });
        published.push(Published {
            case,
            layer,
            version,
            announced: answer.json(),
        });
    }
    published
}

#[tokio::test]
async fn get_layer_version_by_arn_reports_the_version_the_arn_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for row in published_versions(&router).await {
        let case = row.case;
        let arn = layer_version_arn(row.layer, row.version);

        let answer = get_layer_version_by_arn(&router, &arn).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200 from GetLayerVersionByArn on {arn}, got {} {}",
            answer.status,
            answer.text()
        );
        let reported = answer.json();

        // The ARN the answer carries is the stored version's own, and the version
        // number is the one that was published — neither is echoed from the request's
        // shape, but both must agree with it.
        assert_eq!(
            reported.get("LayerVersionArn"),
            Some(&json!(arn)),
            "case {case:?}: expected LayerVersionArn {arn}, got {}",
            answer.text()
        );
        assert_eq!(
            reported.get("LayerArn"),
            Some(&json!(layer_arn(row.layer))),
            "case {case:?}: expected LayerArn {}, got {}",
            layer_arn(row.layer),
            answer.text()
        );
        assert_eq!(
            reported.get("Version"),
            Some(&json!(row.version)),
            "case {case:?}: expected Version {}, got {}",
            row.version,
            answer.text()
        );

        // The rule: the two reads address the same stored record, so they report it
        // member for member — the announced link by the object it addresses, since the
        // link is presigned afresh per read (see `addressed_object`).
        let by_name = get_layer_version(&router, row.layer, row.version).await;
        assert_eq!(
            by_name.status,
            200,
            "case {case:?}: expected 200 from GetLayerVersion, got {} {}",
            by_name.status,
            by_name.text()
        );
        assert_eq!(
            record(case, &reported),
            record(case, &by_name.json()),
            "case {case:?}: expected GetLayerVersionByArn to report what GetLayerVersion \
             reports, got {} against {}",
            answer.text(),
            by_name.text()
        );

        // A read changes nothing a later caller sees.
        let again = get_layer_version_by_arn(&router, &arn).await;
        assert_eq!(
            record(case, &again.json()),
            record(case, &reported),
            "case {case:?}: expected two reads to report the same version, got {} then {}",
            answer.text(),
            again.text()
        );

        // The layer ARN is the other form of the same name, so the name-and-number read
        // addressed by it reports the same record too.
        let by_layer_arn = get_layer_version(&router, &encoded(&layer_arn(row.layer)), row.version)
            .await;
        assert_eq!(
            record(case, &by_layer_arn.json()),
            record(case, &reported),
            "case {case:?}: expected the layer-ARN read to report the same version, got {}",
            by_layer_arn.text()
        );
    }
}

/// Every read that reports one layer version announces a link to the SAME archive object
/// on the same terms, presigned at the instant of that read: the object and the terms come
/// from the stored version, so they do not move between reads, while the instant and the
/// signature over it belong to each read.
///
/// The rows are the published table, so they differ along what the rule generalises over:
/// where the archive came from, which optional members the version carries, and which
/// version number of a layer is addressed. The reads differ along which operation
/// announced the link: the publish itself, the name-and-number read, and the ARN read
/// twice over.
#[tokio::test]
async fn every_read_of_one_layer_version_announces_a_link_to_the_same_archive() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let mut objects: Vec<(&'static str, String)> = Vec::new();
    for row in published_versions(&router).await {
        let case = row.case;
        let arn = layer_version_arn(row.layer, row.version);

        let by_arn = get_layer_version_by_arn(&router, &arn).await;
        let by_arn_again = get_layer_version_by_arn(&router, &arn).await;
        let by_name = get_layer_version(&router, row.layer, row.version).await;
        for (read, answer) in [
            ("GetLayerVersionByArn", &by_arn),
            ("GetLayerVersionByArn a second time", &by_arn_again),
            ("GetLayerVersion", &by_name),
        ] {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected 200 from {read} on {arn}, got {} {}",
                answer.status,
                answer.text()
            );
        }
        let announced = [
            ("the publish", row.announced.clone()),
            ("GetLayerVersionByArn", by_arn.json()),
            ("GetLayerVersionByArn a second time", by_arn_again.json()),
            ("GetLayerVersion", by_name.json()),
        ];

        let object = addressed_object(case, &row.announced["Content"]["Location"]);
        for (read, body) in &announced {
            let reported = addressed_object(case, &body["Content"]["Location"]);
            assert_eq!(
                reported, object,
                "case {case:?}: expected {read} to announce a link to {object}, got {reported}"
            );
        }
        // The address is the stored version's own: its key names the layer and the version
        // number the publish assigned.
        let key_suffix = format!("/{}-{}", row.layer, row.version);
        let address = object.split('?').next().unwrap_or_default().to_string();
        assert!(
            address.ends_with(&key_suffix),
            "case {case:?}: expected an address whose key ends with {key_suffix:?}, got \
             {address:?}"
        );
        objects.push((case, address));
    }

    // Two versions are two archives, so no two of the published versions share an address.
    for (index, (case, address)) in objects.iter().enumerate() {
        for (other_case, other) in objects.iter().skip(index + 1) {
            assert_ne!(
                address, other,
                "case {case:?}: expected its own archive address, got the one case \
                 {other_case:?} is served from: {address:?}"
            );
        }
    }
}

/// One ARN that identifies no layer version this account holds, and the case it covers.
struct Missing {
    case: &'static str,
    arn: String,
}

#[tokio::test]
async fn get_layer_version_by_arn_reports_a_version_this_account_does_not_hold_missing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    published_versions(&router).await;

    let cases = vec![
        Missing {
            case: "a layer no version was ever published under",
            arn: layer_version_arn("absent-layer", 1),
        },
        Missing {
            case: "a version above the newest one published",
            arn: layer_version_arn("versioned-layer", 3),
        },
        Missing {
            case: "a far higher version number",
            arn: layer_version_arn("plain-layer", 4_096),
        },
        // Version numbers count from 1 upwards, so these satisfy the member's pattern
        // (`[0-9]+` as the version segment) while naming a number no layer is assigned.
        Missing {
            case: "version zero",
            arn: layer_version_arn("plain-layer", 0),
        },
        Missing {
            case: "a version number past every number that can be assigned",
            arn: format!("{}:{}", layer_arn("plain-layer"), "9".repeat(32)),
        },
        // "arn:<partition>:lambda:::awslayer:<name>" — the second form the member's
        // pattern admits: a layer Lambda itself provides, which belongs to no account,
        // so this account holds no version of it.
        Missing {
            case: "a layer Lambda itself provides",
            arn: "arn:aws:lambda:::awslayer:AWSLambda-Python37-SciPy1x".to_string(),
        },
    ];

    for case in &cases {
        let answer = get_layer_version_by_arn(&router, &case.arn).await;
        assert_eq!(
            answer.status,
            404,
            "case {:?}: expected 404 from GetLayerVersionByArn on {}, got {} {}",
            case.case,
            case.arn,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            RESOURCE_NOT_FOUND,
            "case {:?}: expected {RESOURCE_NOT_FOUND}, got {:?} in {}",
            case.case,
            answer.error_code(),
            answer.text()
        );
    }

    // A version the store no longer holds is a version this read reports missing, so
    // the answer follows the state rather than the ARN's form: the ARN that read back a
    // version before the delete reports it missing after it, and the layer's other
    // version still reads back.
    let deleted = layer_version_arn("versioned-layer", 1);
    let before = get_layer_version_by_arn(&router, &deleted).await;
    assert_eq!(
        before.status,
        200,
        "expected 200 for {deleted} before the delete, got {} {}",
        before.status,
        before.text()
    );
    let removal = delete_layer_version(&router, "versioned-layer", 1).await;
    assert_eq!(
        removal.status,
        204,
        "expected 204 from DeleteLayerVersion, got {} {}",
        removal.status,
        removal.text()
    );
    let after = get_layer_version_by_arn(&router, &deleted).await;
    assert_eq!(
        after.status,
        404,
        "expected 404 for {deleted} after the delete, got {} {}",
        after.status,
        after.text()
    );
    let kept = layer_version_arn("versioned-layer", 2);
    let survivor = get_layer_version_by_arn(&router, &kept).await;
    assert_eq!(
        survivor.status,
        200,
        "expected 200 for {kept} after deleting version 1, got {} {}",
        survivor.status,
        survivor.text()
    );
}

#[tokio::test]
async fn get_layer_version_by_arn_refuses_a_value_that_is_not_a_layer_version_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    published_versions(&router).await;

    // Every row is outside the `LayerVersionArn` pattern the member carries
    // (`smithy_query(query="operation",operation="GetLayerVersionByArn")`), so the
    // framework's own constraint walk refuses the request before the operation reads
    // anything: the published form needs the version appended, and the Lambda-provided
    // form needs the `awslayer` resource type with no region and no account.
    let cases: Vec<(&str, String)> = vec![
        (
            "an unversioned layer ARN of a layer that exists",
            layer_arn("plain-layer"),
        ),
        (
            "an unversioned layer ARN of a layer that does not",
            layer_arn("absent-layer"),
        ),
        (
            "a layer ARN with a trailing colon and no version",
            format!("{}:", layer_arn("plain-layer")),
        ),
        (
            "a layer ARN whose version is not a number",
            format!("{}:latest", layer_arn("plain-layer")),
        ),
        ("a bare layer name", "plain-layer".to_string()),
        (
            "a function version ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:helpers:1"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:layer:plain-layer:1"),
        ),
        (
            "a Lambda-provided layer ARN carrying a region and an account",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:awslayer:AWSLambda-Python37-SciPy1x"),
        ),
    ];

    for (case, value) in &cases {
        let answer = get_layer_version_by_arn(&router, value).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 from GetLayerVersionByArn on {value}, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?} in {}",
            answer.error_code(),
            answer.text()
        );
        let message = answer.error_message();
        assert!(
            message.contains("at 'arn'") && message.contains(value.as_str()),
            "case {case:?}: expected a refusal naming the arn member and the value, got \
             {message:?}"
        );
    }

    // The refused reads left the store where it was: the published version still reads
    // back through the ARN that does name it.
    let answer = get_layer_version_by_arn(&router, &layer_version_arn("plain-layer", 1)).await;
    assert_eq!(
        answer.status,
        200,
        "expected the published version to still read back, got {} {}",
        answer.status,
        answer.text()
    );
}
