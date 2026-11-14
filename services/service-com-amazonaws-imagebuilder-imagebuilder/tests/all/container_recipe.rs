// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the container-recipe slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — with the emulator's own
//! Amazon S3 mounted beside imagebuilder on one engine, exactly the way the assembled
//! emulator runs them. So a Dockerfile template staged with `PutObject` is the template a
//! `dockerfileTemplateUri` create resolves.
//!
//! The rules under test:
//!
//! * `CreateContainerRecipe` persists the recipe the request describes, under an ARN derived
//!   from a NORMALIZED form of the name plus the version, and `GetContainerRecipe` reports
//!   that stored record back — resolving a reference whose trailing version nodes are `x`
//!   wildcards to the latest matching recipe, and reaching only the calling Region's
//!   recipes.
//! * The recipe carries its Dockerfile template as TEXT, named exactly one way: "You must
//!   specify exactly one of the dockerfileTemplateData or dockerfileTemplateUri properties",
//!   and a location is read ONCE, at create time, so a later overwrite of the object cannot
//!   change what a read of the recipe reports.
//! * Every component reference must name a stored component, each component named only
//!   once, all of them targeting one platform, and at most the documented 20 per recipe; the
//!   platform the recipe reports is the override, else the components', else the base
//!   image's — only ever one of the two platforms a container recipe supports.
//! * `clientToken` is an idempotency token (a retry answers with the original response, and
//!   the same token carrying different parameters is a mismatch), and `dryRun` answers the
//!   dry-run refusal having stored nothing.
//! * A stored container recipe is a recipe a BUILD reaches: a pipeline and a build that name
//!   one inherit its platform and produce container images rather than reporting it missing.
//!
//! The tables span what the rules generalise over: which member carried the template, the
//! shape of the object key, which optional members the create named, the form of the name
//! and of the version, how a reference resolves to nothing, and which part of a request the
//! service cannot accept.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
///
/// Amazon S3 places a bucket created with no `LocationConstraint` in us-east-1, which is also
/// the Region imagebuilder serves a request in by default, so one Region label serves the
/// object store and the control plane alike.
const REGION: &str = "us-east-1";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The bucket every staged template below lives in.
const BUCKET: &str = "imagebuilder-templates";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. No authenticator is registered, so the signature is never checked; the scope is
/// what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// A build component document: one `build` phase, which is all a component these cases
/// reference needs in order to report a platform.
const BUILD_DOCUMENT: &str = concat!(
    "name: HelloWorld\n",
    "schemaVersion: 1.0\n",
    "phases:\n",
    "  - name: build\n",
    "    steps:\n",
    "      - name: Greet\n",
    "        action: ExecuteBash\n",
    "        inputs:\n",
    "          commands:\n",
    "            - echo hello\n",
);

/// An inline Dockerfile template, with the contextual variable Image Builder replaces at
/// build time.
const INLINE_TEMPLATE: &str = "FROM {{{ imagebuilder:parentImage }}}\nRUN echo hello\n";

/// A second template, for the cases that stage one in the object store.
const STAGED_TEMPLATE: &str = "FROM {{{ imagebuilder:parentImage }}}\nRUN echo staged\n";

/// Members a create carries that the stored recipe does not report back under the same
/// name: `clientToken` and `dryRun` describe the call, `semanticVersion` is reported as
/// `version`, `dockerfileTemplateUri` names WHERE the template was fetched from — the
/// template itself is reported as `dockerfileTemplateData` — and `platformOverride` and
/// `imageOsVersionOverride` are reported through the recipe's own members.
const NOT_RECIPE_MEMBERS: &[&str] = &[
    "clientToken",
    "dryRun",
    "semanticVersion",
    "dockerfileTemplateUri",
    "platformOverride",
    "imageOsVersionOverride",
];

/// One emulator: imagebuilder and Amazon S3 on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the two services under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `imagebuilder` in `region`: the signal the router routes on and the
/// handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One call signed for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key live
/// in the URI and the body is the object's own bytes, so this is a raw request rather than a
/// modelled call.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    router.dispatch(request).await.status().as_u16()
}

/// `PUT /<bucket>` — `CreateBucket` — so the staged templates below have a bucket to live in.
async fn given_bucket(router: &MultiServiceRouter) {
    let status = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status, 200,
        "case \"the template bucket\": expected it to be created, got {status}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — storing `template` as the object's own body.
async fn given_object(router: &MultiServiceRouter, key: &str, template: &str) {
    let status = s3(router, "PUT", &format!("/{BUCKET}/{key}"), template.as_bytes()).await;
    assert_eq!(
        status, 200,
        "case {key:?}: expected the template object to be stored, got {status}"
    );
}

/// The `dockerfileTemplateUri` naming the object `key` in [`BUCKET`].
fn uri_of(key: &str) -> String {
    format!("s3://{BUCKET}/{key}")
}

/// A `CreateContainerRecipe` body: the required members, `clientToken`, and whatever else
/// `extra` names. `extra` carrying either template member replaces the inline template.
fn recipe_body(name: &str, version: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("containerType".to_string(), json!("DOCKER"));
    body.insert("parentImage".to_string(), json!("amazonlinux:latest"));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert(
        "targetRepository".to_string(),
        json!({ "service": "ECR", "repositoryName": "team-a/my-repo" }),
    );
    if let Value::Object(extra) = &extra {
        if !extra.contains_key("dockerfileTemplateData")
            && !extra.contains_key("dockerfileTemplateUri")
        {
            body.insert(
                "dockerfileTemplateData".to_string(),
                json!(INLINE_TEMPLATE),
            );
        }
        for (member, value) in extra {
            body.insert(member.clone(), value.clone());
        }
    }
    Value::Object(body)
}

/// `CreateContainerRecipe` with `body`, in `region`.
async fn create_recipe_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "PUT", "/CreateContainerRecipe", Some(body)).await
}

/// `CreateContainerRecipe` with `body`, in [`REGION`].
async fn create_recipe(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    create_recipe_in(router, REGION, body).await
}

/// `GetContainerRecipe` for `reference`, in `region`.
async fn get_recipe_in(router: &MultiServiceRouter, region: &str, reference: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetContainerRecipe?containerRecipeArn={reference}"),
        None,
    )
    .await
}

/// `GetContainerRecipe` for `reference`, in [`REGION`].
async fn get_recipe(router: &MultiServiceRouter, reference: &str) -> (u16, Value) {
    get_recipe_in(router, REGION, reference).await
}

/// `CreateComponent` for one build version of `name` on `platform` in `region`, answering its
/// build version ARN.
async fn create_component(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    platform: &str,
) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": platform,
        "clientToken": format!("component-{region}-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name} in {region}");
    let (status, answer) = call_in(router, region, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateInfrastructureConfiguration` for `name` in [`REGION`], answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let case = format!("create the infrastructure configuration {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// The ARN the container recipe `normalized` carries at `version` in `region`.
fn recipe_arn(region: &str, normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:container-recipe/{normalized}/{version}")
}

/// The version ARN the build versions of the image `normalized` at `version` are listed
/// under.
fn image_version_arn(normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/{normalized}/{version}")
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// Assert `status` is a success and hand back the body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// Assert the status and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
}

/// One member of a response, or a panic naming the case.
fn member<'body>(case: &str, body: &'body Value, name: &str) -> &'body Value {
    body.get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"))
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    member(case, body, name)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} to be text in {body}"))
        .to_string()
}

/// The `containerRecipe` a read reports.
fn reported_recipe(case: &str, body: &Value) -> Value {
    member(case, body, "containerRecipe").clone()
}

/// Assert every RESOURCE member of `sent` comes back from the read as it was sent.
fn assert_round_tripped(case: &str, sent: &Value, reported: &Value) {
    let sent = sent.as_object().expect("a create body is an object");
    for (name, value) in sent {
        if NOT_RECIPE_MEMBERS.contains(&name.as_str()) {
            continue;
        }
        let actual = reported.get(name);
        assert_eq!(
            actual,
            Some(value),
            "case {case:?}: expected the stored {name} to be {value}, got {actual:?}"
        );
    }
    // `semanticVersion` is what a create assigns and `version` is what a read reports.
    let expected_version = sent.get("semanticVersion");
    let actual_version = reported.get("version");
    assert_eq!(
        actual_version, expected_version,
        "case {case:?}: expected version {expected_version:?}, got {actual_version:?}"
    );
}

/// The wildcard references a response reports beside a recipe, by member name.
fn version_references(case: &str, body: &Value) -> BTreeMap<String, String> {
    let references = member(case, body, "latestVersionReferences");
    ["latestVersionArn", "latestMajorVersionArn", "latestMinorVersionArn", "latestPatchVersionArn"]
        .into_iter()
        .map(|name| (name.to_string(), text(case, references, name)))
        .collect()
}

/// One row of the round-trip table: the name and version a create assigns, the normalized
/// name its ARN carries, the members it names beside the required ones, and the platform the
/// stored recipe reports.
struct RecipeRow {
    case: &'static str,
    name: &'static str,
    normalized: &'static str,
    version: &'static str,
    extra: fn() -> Value,
    platform: &'static str,
}

/// RULE: a create persists the recipe it describes under an ARN derived from a normalized
/// form of its name plus its version, and a read reports that record — every member the
/// create named, the platform it builds for, and the encryption the model fixes.
#[tokio::test]
async fn created_container_recipes_are_reported_by_a_read() {
    let router = emulator();
    let linux_component = create_component(&router, REGION, "prep-linux", "Linux").await;
    let windows_component = create_component(&router, REGION, "prep-windows", "Windows").await;

    let rows: Vec<(RecipeRow, Value)> = vec![
        (
            RecipeRow {
                case: "only the required members",
                name: "boxed",
                normalized: "boxed",
                version: "1.0.0",
                extra: || json!({}),
                // No override and no component, so the base image decides — and
                // `amazonlinux:latest` names no Windows image.
                platform: "Linux",
            },
            json!({}),
        ),
        (
            RecipeRow {
                case: "a name that normalizes, and every optional member",
                name: "Boxed Recipe_Two",
                normalized: "boxed-recipe-two",
                version: "2.11.0",
                extra: || {
                    json!({
                        "description": "a recipe that names everything",
                        "workingDirectory": "/tmp",
                        "kmsKeyId": "alias/aws/imagebuilder",
                        "imageOsVersionOverride": "Amazon Linux 2023",
                        "instanceConfiguration": {
                            "image": "ami-0123456789abcdef0",
                            "blockDeviceMappings": [{
                                "deviceName": "/dev/xvda",
                                "ebs": {
                                    "volumeSize": 30,
                                    "volumeType": "gp3",
                                    "deleteOnTermination": true,
                                },
                            }],
                        },
                        "tags": { "owner": "platform", "tier": "build" },
                    })
                },
                platform: "Linux",
            },
            json!({}),
        ),
        (
            RecipeRow {
                case: "the platform its components target",
                name: "boxed-windows",
                normalized: "boxed-windows",
                version: "1.2.3",
                extra: || json!({}),
                platform: "Windows",
            },
            json!({ "components": [{ "componentArn": "WINDOWS_COMPONENT" }] }),
        ),
        (
            RecipeRow {
                case: "an override, which wins over the components",
                name: "boxed-override",
                normalized: "boxed-override",
                version: "0.0.0",
                extra: || json!({ "platformOverride": "Windows" }),
                platform: "Windows",
            },
            json!({ "components": [{ "componentArn": "LINUX_COMPONENT" }] }),
        ),
        (
            RecipeRow {
                case: "the documented upper bound of a version node",
                name: "boxed-bounds",
                normalized: "boxed-bounds",
                version: "1073741823.0.1",
                extra: || json!({}),
                platform: "Linux",
            },
            json!({}),
        ),
    ];

    for (row, components) in rows {
        let mut extra = (row.extra)().as_object().cloned().unwrap_or_default();
        // The component rows name the stored components by the ARNs this run created.
        if let Some(listed) = components.get("components").and_then(Value::as_array) {
            let resolved: Vec<Value> = listed
                .iter()
                .map(|configuration| {
                    let reference = configuration
                        .get("componentArn")
                        .and_then(Value::as_str)
                        .expect("a row names its component");
                    let arn = match reference {
                        "LINUX_COMPONENT" => linux_component.clone(),
                        "WINDOWS_COMPONENT" => windows_component.clone(),
                        other => other.to_string(),
                    };
                    json!({ "componentArn": arn })
                })
                .collect();
            extra.insert("components".to_string(), Value::Array(resolved));
        }
        let sent = recipe_body(
            row.name,
            row.version,
            &format!("token-{}", row.normalized),
            Value::Object(extra),
        );

        let (status, created) = create_recipe(&router, &sent).await;
        let created = assert_ok(row.case, status, created);
        let arn = recipe_arn(REGION, row.normalized, row.version);
        assert_eq!(
            text(row.case, &created, "containerRecipeArn"),
            arn,
            "case {:?}: expected the create to mint {arn}",
            row.case
        );

        let (status, read) = get_recipe(&router, &arn).await;
        let read = assert_ok(row.case, status, read);
        let reported = reported_recipe(row.case, &read);
        assert_round_tripped(row.case, &sent, &reported);
        assert_eq!(
            text(row.case, &reported, "platform"),
            row.platform,
            "case {:?}: expected the recipe to build for {}, got {reported}",
            row.case,
            row.platform
        );
        assert_eq!(
            text(row.case, &reported, "arn"),
            arn,
            "case {:?}: expected the read to report the recipe's own ARN",
            row.case
        );
        assert_eq!(
            text(row.case, &reported, "owner"),
            ACCOUNT,
            "case {:?}: expected the recipe to be owned by this account",
            row.case
        );
        // "Image Builder encrypts all Dockerfile template data at rest, so this value is
        // always true."
        assert_eq!(
            member(row.case, &reported, "encrypted"),
            &json!(true),
            "case {:?}: expected the stored template data to report as encrypted",
            row.case
        );
        let created_at = text(row.case, &reported, "dateCreated");
        assert!(
            created_at.contains('T') && created_at.ends_with('Z') && created_at.len() >= 20,
            "case {:?}: expected an ISO 8601 dateCreated, got {created_at:?}",
            row.case
        );
        // A recipe that was never tagged reports an empty tag map rather than none.
        assert!(
            member(row.case, &reported, "tags").is_object(),
            "case {:?}: expected a tags map in {reported}",
            row.case
        );

        // "A set of wildcard version ARNs that always reference the latest version of the
        // resource", the same set from the create and from the read.
        let references = version_references(row.case, &created);
        assert_eq!(
            references, version_references(row.case, &read),
            "case {:?}: expected the create and the read to report one reference set",
            row.case
        );
        let (major, minor, patch) = {
            let mut nodes = row.version.split('.');
            (
                nodes.next().expect("a major node").to_string(),
                nodes.next().expect("a minor node").to_string(),
                nodes.next().expect("a patch node").to_string(),
            )
        };
        for (name, version) in [
            ("latestVersionArn", "x.x.x".to_string()),
            ("latestMajorVersionArn", format!("{major}.x.x")),
            ("latestMinorVersionArn", format!("{major}.{minor}.x")),
            ("latestPatchVersionArn", format!("{major}.{minor}.{patch}")),
        ] {
            let expected = recipe_arn(REGION, row.normalized, &version);
            let actual = references.get(name);
            assert_eq!(
                actual,
                Some(&expected),
                "case {:?}: expected {name} to be {expected}, got {actual:?}",
                row.case
            );
        }
    }
}

/// RULE: a template named by LOCATION is read from the object store once, at create time, and
/// what it held then is the recipe's template from then on.
#[tokio::test]
async fn a_template_named_by_location_is_read_once_from_the_object_store() {
    let router = emulator();
    given_bucket(&router).await;

    // (case, the key the template is staged under)
    let rows: &[(&str, &str)] = &[
        ("a bare key", "Dockerfile"),
        ("a nested prefix", "team/build/Dockerfile.container"),
    ];
    for (case, key) in rows {
        given_object(&router, key, STAGED_TEMPLATE).await;
        let name = format!("staged-{}", key.replace(['/', '.'], "-").to_lowercase());
        let body = recipe_body(
            &name,
            "1.0.0",
            &format!("token-{name}"),
            json!({ "dockerfileTemplateUri": uri_of(key) }),
        );
        let (status, created) = create_recipe(&router, &body).await;
        assert_ok(case, status, created);

        let arn = recipe_arn(REGION, &name, "1.0.0");
        let (status, read) = get_recipe(&router, &arn).await;
        let read = assert_ok(case, status, read);
        let reported = reported_recipe(case, &read);
        assert_eq!(
            text(case, &reported, "dockerfileTemplateData"),
            STAGED_TEMPLATE,
            "case {case:?}: expected the stored template to be the object's own bytes"
        );
        // The LOCATION is not a member of the recipe: what the recipe carries is the
        // template, so an overwrite of the object cannot change what this read reports.
        given_object(&router, key, "FROM scratch\n").await;
        let (status, again) = get_recipe(&router, &arn).await;
        let again = assert_ok(case, status, again);
        assert_eq!(
            text(case, &reported_recipe(case, &again), "dockerfileTemplateData"),
            STAGED_TEMPLATE,
            "case {case:?}: expected the create-time snapshot to survive an overwrite"
        );
    }

    // A location the object store serves nothing at, and one that is not an S3 URL at all,
    // are values of that member this service cannot accept — and nothing is minted.
    let refusals: &[(&str, &str)] = &[
        ("a key nothing staged", "s3://imagebuilder-templates/absent"),
        ("a bucket nothing created", "s3://imagebuilder-absent/Dockerfile"),
        ("a URL of another scheme", "https://example.com/Dockerfile"),
        ("no scheme at all", "imagebuilder-templates/Dockerfile"),
    ];
    for (n, (case, uri)) in refusals.iter().enumerate() {
        let name = format!("unstaged-{n}");
        let body = recipe_body(
            &name,
            "1.0.0",
            &format!("token-{name}"),
            json!({ "dockerfileTemplateUri": uri }),
        );
        let (status, refused) = create_recipe(&router, &body).await;
        assert_refused(case, status, &refused, 400, "InvalidParameterValueException");
        let (status, missing) = get_recipe(&router, &recipe_arn(REGION, &name, "1.0.0")).await;
        assert_refused(case, status, &missing, 404, "ResourceNotFoundException");
    }
}

/// RULE: a request this service cannot accept is refused as the class the failure belongs to,
/// and nothing is stored for it.
#[tokio::test]
async fn a_create_refuses_what_it_cannot_accept() {
    let router = emulator();
    let linux = create_component(&router, REGION, "refusal-linux", "Linux").await;
    let windows = create_component(&router, REGION, "refusal-windows", "Windows").await;
    let macos = create_component(&router, REGION, "refusal-macos", "macOS").await;
    let absent_component =
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/absent/1.0.0/1");
    let over_quota: Vec<Value> = (0..21)
        .map(|_| json!({ "componentArn": linux.clone() }))
        .collect();

    // (case, what the request names beside the required members, the status, the error code)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "neither template",
            json!({ "dockerfileTemplateData": Value::Null }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "both templates",
            json!({
                "dockerfileTemplateData": INLINE_TEMPLATE,
                "dockerfileTemplateUri": "s3://imagebuilder-templates/Dockerfile",
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a platform a container recipe cannot build for",
            json!({ "platformOverride": "macOS" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a component reference naming nothing stored",
            json!({ "components": [{ "componentArn": absent_component }] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "one component named twice, by two references that resolve to it",
            json!({ "components": [
                { "componentArn": linux.clone() },
                { "componentArn": format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/refusal-linux/1.x.x") },
            ] }),
            400,
            "InvalidRequestException",
        ),
        (
            "components that target two platforms",
            json!({ "components": [
                { "componentArn": linux.clone() },
                { "componentArn": windows.clone() },
            ] }),
            400,
            "InvalidRequestException",
        ),
        (
            "components that target a platform a container recipe cannot build for",
            json!({ "components": [{ "componentArn": macos.clone() }] }),
            400,
            "InvalidRequestException",
        ),
        (
            "more components than the documented quota",
            json!({ "components": over_quota }),
            402,
            "ServiceQuotaExceededException",
        ),
        (
            "a request that only asks for validation",
            json!({ "dryRun": true }),
            412,
            "DryRunOperationException",
        ),
    ];

    for (n, (case, extra, expect_status, expect_code)) in rows.into_iter().enumerate() {
        let name = format!("refused-{n}");
        let mut body = recipe_body(&name, "1.0.0", &format!("token-{name}"), extra)
            .as_object()
            .cloned()
            .expect("a create body is an object");
        // A row naming no template at all drops the one the helper fills in.
        if body.get("dockerfileTemplateData") == Some(&Value::Null) {
            body.remove("dockerfileTemplateData");
        }
        let body = Value::Object(body);
        let (status, refused) = create_recipe(&router, &body).await;
        assert_refused(case, status, &refused, expect_status, expect_code);

        // Nothing was stored for a refused create.
        let (status, missing) = get_recipe(&router, &recipe_arn(REGION, &name, "1.0.0")).await;
        assert_refused(case, status, &missing, 404, "ResourceNotFoundException");
    }

    // A version node outside the bound its values are assignable in, and the `x` wildcard
    // the member's pattern admits for FILTERING, are values this create cannot accept.
    for (case, version) in [
        ("a version node past the documented bound", "1073741824.0.0"),
        ("a wildcard version", "1.x.x"),
    ] {
        let body = recipe_body("version-refusal", version, "token-version-refusal", json!({}));
        let (status, refused) = create_recipe(&router, &body).await;
        assert_refused(case, status, &refused, 400, "InvalidParameterValueException");
    }

    // One name+version pair is stored at most once, whatever case or separators the second
    // create spells the name with.
    let body = recipe_body("Unique Boxed", "1.0.0", "token-unique", json!({}));
    let (status, created) = create_recipe(&router, &body).await;
    assert_ok("the first create", status, created);
    let again = recipe_body("unique_boxed", "1.0.0", "token-unique-2", json!({}));
    let (status, refused) = create_recipe(&router, &again).await;
    assert_refused(
        "a name and version already taken",
        status,
        &refused,
        400,
        "ResourceAlreadyExistsException",
    );
}

/// RULE: a retry carrying a spent `clientToken` answers with the ORIGINAL response, and the
/// same token carrying different creation parameters is a mismatch.
#[tokio::test]
async fn one_client_token_answers_one_create() {
    let router = emulator();
    let body = recipe_body("idempotent-boxed", "1.0.0", "token-idempotent", json!({}));

    let (status, first) = create_recipe(&router, &body).await;
    let first = assert_ok("the first create", status, first);
    let (status, retried) = create_recipe(&router, &body).await;
    let retried = assert_ok("a retry of the same request", status, retried);
    assert_eq!(
        first, retried,
        "case \"a retry of the same request\": expected the original response, got {retried}"
    );

    // The same token carrying different creation parameters is the modelled mismatch.
    let changed = recipe_body(
        "idempotent-boxed",
        "1.0.0",
        "token-idempotent",
        json!({ "description": "a different request under one token" }),
    );
    let (status, refused) = create_recipe(&router, &changed).await;
    assert_refused(
        "one token, different parameters",
        status,
        &refused,
        400,
        "IdempotentParameterMismatchException",
    );

    // A dry run leaves its token free for the real request.
    let dry = recipe_body(
        "dry-boxed",
        "1.0.0",
        "token-dry",
        json!({ "dryRun": true }),
    );
    let (status, refused) = create_recipe(&router, &dry).await;
    assert_refused(
        "a dry run",
        status,
        &refused,
        412,
        "DryRunOperationException",
    );
    let real = recipe_body("dry-boxed", "1.0.0", "token-dry", json!({}));
    let (status, created) = create_recipe(&router, &real).await;
    assert_ok("the real create after a dry run", status, created);
}

/// RULE: a read resolves a reference whose trailing version nodes are `x` wildcards to the
/// latest matching recipe, and a regional endpoint reaches only its own Region's recipes.
#[tokio::test]
async fn a_read_resolves_a_wildcard_reference_within_its_own_region() {
    let router = emulator();
    for version in ["1.0.0", "1.2.0", "2.0.0"] {
        let body = recipe_body(
            "versioned-boxed",
            version,
            &format!("token-versioned-{version}"),
            json!({}),
        );
        let (status, created) = create_recipe(&router, &body).await;
        assert_ok(version, status, created);
    }
    // The same name in another Region, at a version this Region holds none of.
    let elsewhere = recipe_body("versioned-boxed", "9.9.9", "token-elsewhere", json!({}));
    let (status, created) = create_recipe_in(&router, OTHER_REGION, &elsewhere).await;
    assert_ok("a recipe of another Region", status, created);

    // (case, the reference a read carries, the version it resolves to)
    let rows: &[(&str, &str, &str)] = &[
        ("an exact version", "1.2.0", "1.2.0"),
        ("the latest overall", "x.x.x", "2.0.0"),
        ("the latest of one major version", "1.x.x", "1.2.0"),
        ("the latest of one minor version", "1.0.x", "1.0.0"),
    ];
    for (case, reference, resolved) in rows {
        let (status, read) =
            get_recipe(&router, &recipe_arn(REGION, "versioned-boxed", reference)).await;
        let read = assert_ok(case, status, read);
        let reported = reported_recipe(case, &read);
        assert_eq!(
            text(case, &reported, "version"),
            *resolved,
            "case {case:?}: expected the read to resolve to {resolved}, got {reported}"
        );
        assert_eq!(
            text(case, &reported, "arn"),
            recipe_arn(REGION, "versioned-boxed", resolved),
            "case {case:?}: expected the read to report the resolved recipe's own ARN"
        );
    }

    // (case, a reference this Region holds nothing under)
    let missing: &[(&str, String)] = &[
        (
            "a name nothing created",
            recipe_arn(REGION, "absent-boxed", "1.0.0"),
        ),
        (
            "a version no stored recipe carries",
            recipe_arn(REGION, "versioned-boxed", "9.x.x"),
        ),
        (
            "a recipe of another Region",
            recipe_arn(OTHER_REGION, "versioned-boxed", "9.9.9"),
        ),
    ];
    for (case, reference) in missing {
        let (status, refused) = get_recipe(&router, reference).await;
        assert_refused(case, status, &refused, 404, "ResourceNotFoundException");
        let message = text(case, &refused, "message");
        let expected =
            format!("The following required resource 'ContainerRecipe' cannot be found: '{reference}'.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The same rule holds for the OTHER kind of recipe, and for the same reason: a regional
    // endpoint serves only its own Region's resources, so an image recipe stored only in
    // another Region is reached by no reference of this one — whatever name and version it
    // carries, exact or wildcard.
    let image_recipe = json!({
        "name": "versioned-image",
        "semanticVersion": "9.9.9",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": "token-elsewhere-image",
    });
    let (status, created) = call_in(
        &router,
        OTHER_REGION,
        "PUT",
        "/CreateImageRecipe",
        Some(&image_recipe),
    )
    .await;
    assert_ok("an image recipe of another Region", status, created);
    for (case, version) in [
        ("an exact version of another Region's image recipe", "9.9.9"),
        ("a wildcard reaching another Region's image recipe", "x.x.x"),
    ] {
        let reference = format!(
            "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-recipe/versioned-image/{version}"
        );
        let (status, refused) = call(
            &router,
            "GET",
            &format!("/GetImageRecipe?imageRecipeArn={reference}"),
            None,
        )
        .await;
        assert_refused(case, status, &refused, 404, "ResourceNotFoundException");
    }
}

/// RULE: a stored container recipe is a recipe a BUILD reaches — a pipeline and a build that
/// name one inherit its platform and produce container images.
#[tokio::test]
async fn a_build_reaches_a_stored_container_recipe() {
    let router = emulator();
    let windows_component = create_component(&router, REGION, "build-windows", "Windows").await;
    let configuration = create_configuration(&router, "build-infra").await;
    let body = recipe_body(
        "built-boxed",
        "1.0.0",
        "token-built",
        json!({ "components": [{ "componentArn": windows_component }] }),
    );
    let (status, created) = create_recipe(&router, &body).await;
    assert_ok("the recipe a build names", status, created);
    let recipe = recipe_arn(REGION, "built-boxed", "1.0.0");

    // A pipeline inherits the platform of the recipe it builds, whichever kind that is.
    let (status, pipeline) = call(
        &router,
        "PUT",
        "/CreateImagePipeline",
        Some(&json!({
            "name": "built-boxed-pipeline",
            "clientToken": "token-built-pipeline",
            "containerRecipeArn": recipe.clone(),
            "infrastructureConfigurationArn": configuration.clone(),
        })),
    )
    .await;
    let pipeline = assert_ok("a pipeline naming a container recipe", status, pipeline);
    let pipeline_arn = text("a pipeline naming a container recipe", &pipeline, "imagePipelineArn");
    let (status, read) = call(
        &router,
        "GET",
        &format!("/GetImagePipeline?imagePipelineArn={pipeline_arn}"),
        None,
    )
    .await;
    let read = assert_ok("the stored pipeline", status, read);
    let stored = member("the stored pipeline", &read, "imagePipeline").clone();
    assert_eq!(
        text("the stored pipeline", &stored, "platform"),
        "Windows",
        "case \"the stored pipeline\": expected the recipe's platform, got {stored}"
    );
    assert_eq!(
        text("the stored pipeline", &stored, "containerRecipeArn"),
        recipe,
        "case \"the stored pipeline\": expected the recipe it builds, got {stored}"
    );

    // A build the caller asks for by hand takes the image's name and version from the
    // recipe, and "for an image recipe, this is always AMI" — a container recipe's builds
    // are container images instead.
    let (status, build) = call(
        &router,
        "PUT",
        "/CreateImage",
        Some(&json!({
            "containerRecipeArn": recipe.clone(),
            "infrastructureConfigurationArn": configuration,
            "clientToken": "token-built-image",
        })),
    )
    .await;
    let build = assert_ok("a build naming a container recipe", status, build);
    let build_arn = text("a build naming a container recipe", &build, "imageBuildVersionArn");
    assert_eq!(
        build_arn,
        format!("{}/1", image_version_arn("built-boxed", "1.0.0")),
        "case \"a build naming a container recipe\": expected the recipe's name and version"
    );

    // And the pipeline's own run queues a second build of the same name+version pair.
    let (status, started) = call(
        &router,
        "PUT",
        "/StartImagePipelineExecution",
        Some(&json!({ "imagePipelineArn": pipeline_arn, "clientToken": "token-built-run" })),
    )
    .await;
    assert_ok("a pipeline run of a container recipe", status, started);

    let (status, listed) = call(
        &router,
        "POST",
        "/ListImageBuildVersions",
        Some(&json!({ "imageVersionArn": image_version_arn("built-boxed", "1.0.0") })),
    )
    .await;
    let listed = assert_ok("the build versions of the container image", status, listed);
    let summaries = member(
        "the build versions of the container image",
        &listed,
        "imageSummaryList",
    )
    .as_array()
    .expect("a list of build versions")
    .clone();
    assert_eq!(
        summaries.len(),
        2,
        "case \"the build versions of the container image\": expected the two builds, got {listed}"
    );
    for summary in &summaries {
        assert_eq!(
            text("one listed build version", summary, "type"),
            "DOCKER",
            "case \"one listed build version\": expected a container image, got {summary}"
        );
        assert_eq!(
            text("one listed build version", summary, "platform"),
            "Windows",
            "case \"one listed build version\": expected the recipe's platform, got {summary}"
        );
    }
}

/// RULE: tags written onto a container recipe are what every later read of it reports, the
/// recipe's own read included.
#[tokio::test]
async fn tags_written_onto_a_container_recipe_are_read_back() {
    let router = emulator();
    let body = recipe_body("tagged-boxed", "1.0.0", "token-tagged", json!({}));
    let (status, created) = create_recipe(&router, &body).await;
    assert_ok("the recipe a tagging call names", status, created);
    let arn = recipe_arn(REGION, "tagged-boxed", "1.0.0");
    let label =
        percent_encoding::utf8_percent_encode(&arn, percent_encoding::NON_ALPHANUMERIC).to_string();

    let (status, tagged) = call(
        &router,
        "POST",
        &format!("/tags/{label}"),
        Some(&json!({ "tags": { "owner": "platform" } })),
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "case \"tagging a container recipe\": expected the call to be served, got {status} with \
         {tagged}"
    );

    let (status, listed) = call(&router, "GET", &format!("/tags/{label}"), None).await;
    let listed = assert_ok("the tags of a container recipe", status, listed);
    assert_eq!(
        member("the tags of a container recipe", &listed, "tags"),
        &json!({ "owner": "platform" }),
        "case \"the tags of a container recipe\": expected the written tag, got {listed}"
    );

    let (status, read) = get_recipe(&router, &arn).await;
    let read = assert_ok("the recipe's own read", status, read);
    assert_eq!(
        member(
            "the recipe's own read",
            &reported_recipe("the recipe's own read", &read),
            "tags"
        ),
        &json!({ "owner": "platform" }),
        "case \"the recipe's own read\": expected the written tag, got {read}"
    );
}
