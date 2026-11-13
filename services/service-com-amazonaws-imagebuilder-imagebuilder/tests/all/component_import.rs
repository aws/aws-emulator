// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ImportComponent`: what an imported SCRIPT becomes.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — with the emulator's own
//! Amazon S3 mounted beside imagebuilder on one engine, exactly the way the assembled
//! emulator runs them, so a script staged with `PutObject` is the script a `uri` import
//! wraps.
//!
//! The rules under test:
//!
//! * the operation "imports a component and transforms its data into a component document.
//!   For the SHELL format, Image Builder wraps your script in a component document with a
//!   single step that runs the script" — so the component the import mints HOLDS that
//!   document: one phase, the one the submitted `type` names, carrying one step that runs the
//!   submitted script whole through the action the submitted `platform` runs a shell script
//!   with, and every member a later `GetComponent` reports is the one the import submitted;
//! * the script arrives through exactly one of `data` ("the plain script content") and `uri`
//!   ("for scripts that exceed the inline length constraint"), so naming both or neither is
//!   refused as this service refuses a parameter whose value it cannot accept, naming both
//!   members, and an address the object store serves nothing at is refused with nothing
//!   minted;
//! * a repeat of one name+version pair "creates a new build version for it. If the content is
//!   also identical to the latest build version, the request fails because the component
//!   already exists", naming the build version that already holds that content;
//! * `clientToken` is an idempotency token: a retry carrying it answers with the ORIGINAL
//!   response and mints nothing, the same token carrying different parameters is the modelled
//!   mismatch, and the token is spent on THIS operation alone.
//!
//! The table spans what the rules generalise over: the component type, every platform the
//! model admits, the shape of the script (one line, several lines, YAML-looking text, quotes,
//! tabs and blank lines, a script past the inline length bound), whether a description and
//! tags were submitted, and which member carried the script.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
///
/// Amazon S3 places a bucket created with no `LocationConstraint` in us-east-1, which is also
/// the Region imagebuilder serves a request in by default, so one Region label serves the
/// object store and the control plane alike.
const REGION: &str = "us-east-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The bucket every staged script below lives in.
const BUCKET: &str = "imagebuilder-scripts";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. No authenticator is registered, so the signature is never checked; the scope is
/// what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The one format this operation imports (`smithy_query(query="shape",shape="ComponentFormat"`:
/// `SHELL` alone).
const SHELL: &str = "SHELL";

/// The two component types (`smithy_query(query="shape",shape="ComponentType")`).
const BUILD: &str = "BUILD";
const TEST: &str = "TEST";

/// Every platform the model admits (`smithy_query(query="shape",shape="Platform")`).
const LINUX: &str = "Linux";
const MACOS: &str = "macOS";
const WINDOWS: &str = "Windows";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// A component document a `CreateComponent` carries inline, so the create a shared token is
/// also carried by is answered for its own request rather than for its document.
const CREATED_DOCUMENT: &str = concat!(
    "name: Created\n",
    "schemaVersion: 1.0\n",
    "phases:\n",
    "  - name: build\n",
    "    steps:\n",
    "      - name: run\n",
    "        action: ExecuteBash\n",
    "        inputs:\n",
    "          commands:\n",
    "            - echo created\n",
);

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

/// Status and parsed body of one imagebuilder call, signed for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
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
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
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

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key live
/// in the URI and the body is the object's own bytes.
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

/// `PUT /<bucket>` — `CreateBucket` — so the staged scripts have a bucket to live in.
async fn given_bucket(router: &MultiServiceRouter) {
    let status = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status, 200,
        "case \"the script bucket\": expected it to be created, got {status}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — storing `script` as the object's own body.
async fn given_object(router: &MultiServiceRouter, key: &str, script: &str) {
    let status = s3(router, "PUT", &format!("/{BUCKET}/{key}"), script.as_bytes()).await;
    assert_eq!(
        status, 200,
        "case {key:?}: expected the script object to be stored, got {status}"
    );
}

/// The `uri` naming the object `key` in [`BUCKET`].
fn uri_of(key: &str) -> String {
    format!("s3://{BUCKET}/{key}")
}

/// `ImportComponent` with `body`.
async fn import_component(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/ImportComponent", Some(body)).await
}

/// `GetComponent` for `arn`.
async fn get_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// The build version ARN of the `build`th build of one component name+version pair.
fn component_arn(name: &str, version: &str, build: u32) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/{name}/{version}/{build}")
}

/// The error shape a refusal names, reduced to its shape name.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The sentence a response states in its `message`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// The component a read of `arn` reports, asserted to be a successful read.
async fn reported_component(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = get_component(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the read, got {status} with {body}"
    );
    body.get("component")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a component in {body}"))
}

/// Where the script a row imports comes from.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// Inline, in `data`.
    Inline,
    /// From the object store, named by `uri` under this key.
    Object(&'static str),
}

/// One import: what it submits, and what the component it mints must then report.
#[derive(Debug, Clone)]
struct ImportRow {
    case: &'static str,
    component_type: &'static str,
    platform: &'static str,
    script: String,
    description: Option<&'static str>,
    change_description: Option<&'static str>,
    source: Source,
    tags: Option<Value>,
}

/// The action module a shell script runs through on each platform: PowerShell is the Windows
/// shell, bash the shell of the two Unix platforms the model admits.
fn expected_action(platform: &str) -> &'static str {
    if platform == WINDOWS {
        "ExecutePowerShell"
    } else {
        "ExecuteBash"
    }
}

/// The phase the wrapping document declares: "If the document contains a single phase named
/// test, the component type is TEST. Otherwise, the component type is BUILD"
/// (`CreateComponent`), so an imported `TEST` component's one phase is the test phase.
fn expected_phase(component_type: &str) -> &'static str {
    if component_type == TEST {
        "test"
    } else {
        "build"
    }
}

/// A script longer than the inline `data` bound (`@length(1..1024)`), which is what `uri` is
/// documented for: "For scripts that exceed the inline length constraint, use the uri
/// property."
fn oversize_script() -> String {
    let mut script = String::from("#!/bin/bash\n");
    for line in 0..120 {
        script.push_str(&format!("echo \"line {line}: padding padding padding\"\n"));
    }
    assert!(
        script.len() > 1024,
        "the oversize script must exceed the inline bound, it is {} bytes",
        script.len()
    );
    script
}

/// Every import row.
fn import_rows() -> Vec<ImportRow> {
    vec![
        ImportRow {
            case: "a one-line build script on Linux",
            component_type: BUILD,
            platform: LINUX,
            script: "echo hello".to_string(),
            description: Some("a greeting"),
            change_description: None,
            source: Source::Inline,
            tags: None,
        },
        ImportRow {
            case: "a multi-line test script on Linux",
            component_type: TEST,
            platform: LINUX,
            script: "#!/bin/bash\nset -e\necho testing\n".to_string(),
            description: None,
            change_description: Some("the first revision"),
            source: Source::Inline,
            tags: Some(json!({"team": "images"})),
        },
        ImportRow {
            case: "a PowerShell script on Windows",
            component_type: BUILD,
            platform: WINDOWS,
            script: "Write-Host 'hello'\nGet-Date\n".to_string(),
            description: Some("a Windows script"),
            change_description: None,
            source: Source::Inline,
            tags: None,
        },
        ImportRow {
            case: "a script on macOS",
            component_type: TEST,
            platform: MACOS,
            script: "sw_vers".to_string(),
            description: None,
            change_description: None,
            source: Source::Inline,
            tags: Some(json!({"team": "images", "stage": "test"})),
        },
        ImportRow {
            case: "a script whose text looks like a component document",
            component_type: BUILD,
            platform: LINUX,
            script: "name: not-a-document\nphases: []\n".to_string(),
            description: Some("text that is a script, not a document"),
            change_description: None,
            source: Source::Inline,
            tags: None,
        },
        ImportRow {
            case: "a script carrying quotes, a tab and a blank line",
            component_type: BUILD,
            platform: LINUX,
            script: "echo 'single' \"double\"\n\techo\tindented\n\necho done".to_string(),
            description: None,
            change_description: None,
            source: Source::Inline,
            tags: None,
        },
        ImportRow {
            case: "a script named by uri",
            component_type: BUILD,
            platform: LINUX,
            script: "echo from-the-object-store\necho second line\n".to_string(),
            description: Some("a script fetched from Amazon S3"),
            change_description: None,
            source: Source::Object("scripts/fetched.sh"),
            tags: None,
        },
        ImportRow {
            case: "a script past the inline length bound, named by uri",
            component_type: BUILD,
            platform: LINUX,
            script: oversize_script(),
            description: None,
            change_description: None,
            source: Source::Object("scripts/nested/oversize.sh"),
            tags: None,
        },
    ]
}

/// An `ImportComponent` body for `row`, naming `name` at `version` under `token`.
fn import_body(row: &ImportRow, name: &str, version: &str, token: &str) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("type".to_string(), json!(row.component_type));
    body.insert("format".to_string(), json!(SHELL));
    body.insert("platform".to_string(), json!(row.platform));
    body.insert("clientToken".to_string(), json!(token));
    match row.source {
        Source::Inline => {
            body.insert("data".to_string(), json!(row.script));
        }
        Source::Object(key) => {
            body.insert("uri".to_string(), json!(uri_of(key)));
        }
    }
    if let Some(description) = row.description {
        body.insert("description".to_string(), json!(description));
    }
    if let Some(change_description) = row.change_description {
        body.insert("changeDescription".to_string(), json!(change_description));
    }
    if let Some(tags) = &row.tags {
        body.insert("tags".to_string(), tags.clone());
    }
    Value::Object(body)
}

/// The commands the one step of `document` runs, with the phase and the action it declares.
///
/// Reads the stored document with the same YAML implementation this service reads a submitted
/// one with, so what is asserted is what a document reader finds in it.
fn only_step(case: &str, document: &str) -> (String, String, Vec<String>) {
    let mut loaded = yaml_rust2::YamlLoader::load_from_str(document)
        .unwrap_or_else(|why| panic!("case {case:?}: the document must parse, got {why:?}"));
    assert_eq!(
        loaded.len(),
        1,
        "case {case:?}: expected one YAML document, got {} in {document}",
        loaded.len()
    );
    let tree = loaded.remove(0);
    let phases = tree["phases"]
        .as_vec()
        .unwrap_or_else(|| panic!("case {case:?}: expected phases in {document}"))
        .clone();
    assert_eq!(
        phases.len(),
        1,
        "case {case:?}: expected one phase in {document}"
    );
    let phase = phases[0]["name"].as_str().unwrap_or_default().to_string();
    let steps = phases[0]["steps"]
        .as_vec()
        .unwrap_or_else(|| panic!("case {case:?}: expected steps in {document}"))
        .clone();
    assert_eq!(
        steps.len(),
        1,
        "case {case:?}: expected one step in {document}"
    );
    let action = steps[0]["action"].as_str().unwrap_or_default().to_string();
    let commands = steps[0]["inputs"]["commands"]
        .as_vec()
        .unwrap_or_else(|| panic!("case {case:?}: expected commands in {document}"))
        .iter()
        .map(|command| command.as_str().unwrap_or_default().to_string())
        .collect();
    (phase, action, commands)
}

#[tokio::test]
async fn an_imported_script_is_stored_as_the_component_document_it_is_wrapped_in() {
    let router = emulator();
    given_bucket(&router).await;

    for (index, row) in import_rows().into_iter().enumerate() {
        let case = row.case;
        if let Source::Object(key) = row.source {
            given_object(&router, key, &row.script).await;
        }
        let name = format!("imported-{index}");
        let version = "1.0.0";
        let token = format!("itok-{index}");
        let (status, answered) =
            import_component(&router, &import_body(&row, &name, version, &token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered}"
        );

        // The response names the request it answered, echoes the token, and carries the ARN
        // of the build version it minted: the first build of this name+version pair.
        let expected_arn = component_arn(&name, version, 1);
        let minted = text(case, &answered, "componentBuildVersionArn");
        assert_eq!(
            minted, expected_arn,
            "case {case:?}: expected the ARN {expected_arn:?}, got {minted:?}"
        );
        let echoed = text(case, &answered, "clientToken");
        assert_eq!(
            echoed, token,
            "case {case:?}: expected the token {token:?} echoed, got {echoed:?}"
        );
        let request_id = text(case, &answered, "requestId");
        assert!(
            !request_id.is_empty(),
            "case {case:?}: expected a requestId, got {answered}"
        );

        // Round trip: the stored component reports every member the import submitted.
        let component = reported_component(&router, case, &minted).await;
        let reported_name = text(case, &component, "name");
        assert_eq!(
            reported_name, name,
            "case {case:?}: expected the name {name:?}, got {reported_name:?}"
        );
        let reported_version = text(case, &component, "version");
        assert_eq!(
            reported_version, version,
            "case {case:?}: expected the version {version:?}, got {reported_version:?}"
        );
        let reported_type = text(case, &component, "type");
        assert_eq!(
            reported_type, row.component_type,
            "case {case:?}: expected the type {:?}, got {reported_type:?}",
            row.component_type
        );
        let reported_platform = text(case, &component, "platform");
        assert_eq!(
            reported_platform, row.platform,
            "case {case:?}: expected the platform {:?}, got {reported_platform:?}",
            row.platform
        );
        let owner = text(case, &component, "owner");
        assert_eq!(
            owner, ACCOUNT,
            "case {case:?}: expected the owner {ACCOUNT:?}, got {owner:?}"
        );
        let status_reported = component["state"]["status"].as_str().unwrap_or_default();
        assert_eq!(
            status_reported, "ACTIVE",
            "case {case:?}: expected an ACTIVE component, got {status_reported:?}"
        );
        let described = component.get("description").and_then(Value::as_str);
        assert_eq!(
            described, row.description,
            "case {case:?}: expected the description {:?}, got {described:?}",
            row.description
        );
        let change = component.get("changeDescription").and_then(Value::as_str);
        assert_eq!(
            change, row.change_description,
            "case {case:?}: expected the change description {:?}, got {change:?}",
            row.change_description
        );
        let reported_tags = component.get("tags").cloned().unwrap_or(json!({}));
        let expected_tags = row.tags.clone().unwrap_or(json!({}));
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags}, got {reported_tags}"
        );

        // And the document it holds is the one the script was WRAPPED in: one phase named for
        // the submitted type, one step running the script whole through the submitted
        // platform's action module.
        let document = text(case, &component, "data");
        let (phase, action, commands) = only_step(case, &document);
        let expected_phase = expected_phase(row.component_type);
        assert_eq!(
            phase, expected_phase,
            "case {case:?}: expected the phase {expected_phase:?}, got {phase:?}"
        );
        let expected_action = expected_action(row.platform);
        assert_eq!(
            action, expected_action,
            "case {case:?}: expected the action {expected_action:?}, got {action:?}"
        );
        assert_eq!(
            commands,
            vec![row.script.clone()],
            "case {case:?}: expected the script to ride whole, got {commands:?}"
        );
    }
}

#[tokio::test]
async fn an_import_names_the_script_through_exactly_one_member() {
    let router = emulator();

    // The requirement holds over the two members TOGETHER — "You must specify exactly one of
    // the data or uri properties" — so it is stated naming both, whichever way the request
    // missed it, and nothing is minted either way.
    let expected = "The value supplied for parameter 'data' and 'uri' is not valid. You must \
                    specify either data or a uri.";
    let rows: Vec<(&str, Value)> = vec![
        ("neither member", json!({})),
        (
            "both members",
            json!({"data": "echo hello", "uri": uri_of("scripts/both.sh")}),
        ),
    ];
    for (index, (case, extra)) in rows.into_iter().enumerate() {
        let name = format!("exactly-one-{index}");
        let mut body = json!({
            "name": name,
            "semanticVersion": "1.0.0",
            "type": BUILD,
            "format": SHELL,
            "platform": LINUX,
            "clientToken": format!("etok-{index}"),
        });
        for (member, value) in extra.as_object().expect("an object of extra members") {
            body[member] = value.clone();
        }
        let (status, refused) = import_component(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE:?}, got {code:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // Nothing was minted: the ARN the import would have carried names nothing.
        let (status, missing) = get_component(&router, &component_arn(&name, "1.0.0", 1)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused import must store nothing, got {status} with {missing}"
        );
    }
}

#[tokio::test]
async fn a_uri_the_object_store_serves_nothing_at_mints_nothing() {
    let router = emulator();
    given_bucket(&router).await;
    given_object(&router, "scripts/present.sh", "echo present").await;

    // Image Builder fetches the script itself at import time, so an address that names no
    // object — and one that is not an S3 URL at all — is a value of `uri` this service cannot
    // accept, and nothing is minted for it.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a key nothing was stored under",
            uri_of("scripts/absent.sh"),
            "The value supplied for parameter 'uri' is not valid. Not Found",
        ),
        (
            "a bucket nothing created",
            "s3://imagebuilder-scripts-missing/scripts/present.sh".to_string(),
            "The value supplied for parameter 'uri' is not valid. Not Found",
        ),
        (
            "an address that is not an S3 URL",
            "https://example.com/scripts/present.sh".to_string(),
            "The value supplied for parameter 'uri' is not valid. uri must be an S3 URL \
             (s3://bucket/key)",
        ),
    ];
    for (index, (case, uri, expected)) in rows.into_iter().enumerate() {
        let name = format!("absent-script-{index}");
        let body = json!({
            "name": name,
            "semanticVersion": "1.0.0",
            "type": BUILD,
            "format": SHELL,
            "platform": LINUX,
            "clientToken": format!("atok-{index}"),
            "uri": uri,
        });
        let (status, refused) = import_component(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE:?}, got {code:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        let (status, missing) = get_component(&router, &component_arn(&name, "1.0.0", 1)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused import must store nothing, got {status} with {missing}"
        );
    }
}

#[tokio::test]
async fn importing_one_name_and_version_again_numbers_the_next_build_version() {
    let router = emulator();

    let name = "rebuilt-component";
    let version = "2.3.4";
    let base = |script: &str, token: &str| {
        json!({
            "name": name,
            "semanticVersion": version,
            "type": BUILD,
            "format": SHELL,
            "platform": LINUX,
            "clientToken": token,
            "data": script,
        })
    };

    // The first import is build 1.
    let (status, first) = import_component(&router, &base("echo one", "rtok-1")).await;
    assert_eq!(
        status, 200,
        "case \"the first import\": expected HTTP 200, got {status} with {first}"
    );
    let first_arn = text("the first import", &first, "componentBuildVersionArn");
    assert_eq!(
        first_arn,
        component_arn(name, version, 1),
        "case \"the first import\": expected build 1, got {first_arn:?}"
    );

    // A changed script is new CONTENT, so the same name+version pair gets the next build.
    let (status, second) = import_component(&router, &base("echo two", "rtok-2")).await;
    assert_eq!(
        status, 200,
        "case \"a changed script\": expected HTTP 200, got {status} with {second}"
    );
    let second_arn = text("a changed script", &second, "componentBuildVersionArn");
    assert_eq!(
        second_arn,
        component_arn(name, version, 2),
        "case \"a changed script\": expected build 2, got {second_arn:?}"
    );

    // Both builds are readable, each holding its own script.
    for (case, arn, script) in [
        ("build 1", &first_arn, "echo one"),
        ("build 2", &second_arn, "echo two"),
    ] {
        let component = reported_component(&router, case, arn).await;
        let document = text(case, &component, "data");
        let (_, _, commands) = only_step(case, &document);
        assert_eq!(
            commands,
            vec![script.to_string()],
            "case {case:?}: expected the script {script:?}, got {commands:?}"
        );
    }

    // The SAME script under a new token is content identical to the latest build version, so
    // "the request fails because the component already exists", naming that build version.
    let (status, refused) = import_component(&router, &base("echo two", "rtok-3")).await;
    assert_eq!(
        status, 400,
        "case \"an identical script\": expected HTTP 400, got {status} with {refused}"
    );
    let code = error_code("an identical script", &refused);
    assert_eq!(
        code, "ResourceAlreadyExistsException",
        "case \"an identical script\": expected ResourceAlreadyExistsException, got {code:?}"
    );
    let expected = format!("The following resource 'Component' already exists: '{second_arn}'.");
    let message = error_message(&refused);
    assert_eq!(
        message, expected,
        "case \"an identical script\": expected the message {expected:?}, got {message:?}"
    );

    // And no third build was minted by the refusal.
    let (status, missing) = get_component(&router, &component_arn(name, version, 3)).await;
    assert_eq!(
        status, 404,
        "case \"an identical script\": a refused import must store nothing, got {status} with \
         {missing}"
    );
}

#[tokio::test]
async fn an_imports_client_token_answers_the_original_response() {
    let router = emulator();

    let body = |script: &str| {
        json!({
            "name": "token-component",
            "semanticVersion": "1.0.0",
            "type": BUILD,
            "format": SHELL,
            "platform": LINUX,
            "clientToken": "shared-token",
            "data": script,
        })
    };

    let (status, original) = import_component(&router, &body("echo once")).await;
    assert_eq!(
        status, 200,
        "case \"the original import\": expected HTTP 200, got {status} with {original}"
    );

    // "If you retry a request with the same client token, Image Builder returns the original
    // response without running the operation again" — down to the requestId, and no second
    // build version.
    let (status, retried) = import_component(&router, &body("echo once")).await;
    assert_eq!(
        status, 200,
        "case \"a retry\": expected HTTP 200, got {status} with {retried}"
    );
    assert_eq!(
        retried, original,
        "case \"a retry\": expected the original response {original}, got {retried}"
    );
    let (status, missing) =
        get_component(&router, &component_arn("token-component", "1.0.0", 2)).await;
    assert_eq!(
        status, 404,
        "case \"a retry\": a retry must mint nothing, got {status} with {missing}"
    );

    // The same token carrying different parameters is the modelled mismatch.
    let (status, refused) = import_component(&router, &body("echo twice")).await;
    assert_eq!(
        status, 400,
        "case \"a changed retry\": expected HTTP 400, got {status} with {refused}"
    );
    let code = error_code("a changed retry", &refused);
    assert_eq!(
        code, "IdempotentParameterMismatchException",
        "case \"a changed retry\": expected IdempotentParameterMismatchException, got {code:?}"
    );
    let expected = "The request parameters do not match those in the original idempotent request.";
    let message = error_message(&refused);
    assert_eq!(
        message, expected,
        "case \"a changed retry\": expected the message {expected:?}, got {message:?}"
    );

    // A token is spent on ONE operation, so the same text carried by a CREATE is a different
    // request and mints its own component.
    let created = json!({
        "name": "created-component",
        "semanticVersion": "1.0.0",
        "platform": LINUX,
        "clientToken": "shared-token",
        "data": CREATED_DOCUMENT,
    });
    let (status, answered) = call(&router, "PUT", "/CreateComponent", Some(&created)).await;
    assert_eq!(
        status, 200,
        "case \"one token on two operations\": expected HTTP 200, got {status} with {answered}"
    );
    let minted = text("one token on two operations", &answered, "componentBuildVersionArn");
    let expected_arn = component_arn("created-component", "1.0.0", 1);
    assert_eq!(
        minted, expected_arn,
        "case \"one token on two operations\": expected {expected_arn:?}, got {minted:?}"
    );
}
