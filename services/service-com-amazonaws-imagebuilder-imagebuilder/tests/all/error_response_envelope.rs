// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the envelope every FAILURE of this service answers in.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door, the modelled-constraint walk and the handlers
//! — so what is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a request this service does not answer is answered with the error
//! this service MODELS for that condition — that shape's own `@httpError` status, the shape
//! named in both places restJson1 carries it (`$.__type` and the `x-amzn-errortype` header),
//! and that condition's message — and every such failure names the request it answers in the
//! `requestId` member of its error body. No error shape of this model declares that member,
//! so it belongs to the envelope and not to any one handler: one id per response, newly
//! minted for each, and never added to a success, whose own `requestId` is the one its output
//! shape models.
//!
//! Two conditions anchor which error is modelled for what:
//!
//! * an ARN that resolves to no resource of the calling Region is the not-found error
//!   (`ResourceNotFoundException`, HTTP 404), whose message names the kind of resource the
//!   ARN identifies and the ARN the request carried;
//! * a value a modelled constraint of this service does not admit is the invalid-parameter
//!   error (`InvalidParameterValueException`, HTTP 400), whose message names the parameter
//!   and the requirement its value had to satisfy.
//!
//! The rows span what the rule generalises over: which layer answered (a handler lookup, the
//! front door's own refusal of a modelled pattern, a handler's refusal of an opaque token,
//! the framework's own refusal of a constraint class this service does not word), which
//! operation was called, where in the request the refused value rode (a body member, a query
//! parameter, a path label), and which HTTP status the modelled error carries.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every case signs for; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all a recipe these cases create
/// needs.
const BUILD_DOCUMENT: &str = "name: Probe\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: run\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hi\n";

/// The error this service reports a referenced resource it does not hold with
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`: a client error,
/// `@httpError` 404).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The pattern `ListImagePackages` states for `imageBuildVersionArn`: the modelled
/// `ImageBuildVersionArn` pattern (`smithy_query(query="shape",shape="ImageBuildVersionArn")`)
/// with its partition and Region fields spelled as the layer that reads a BODY member states
/// them.
const IMAGE_BUILD_VERSION_ARN_PATTERN: &str = "^arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?):image/[a-z0-9-_]+/[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+$";

/// The pattern `GetImageRecipe` states for `imageRecipeArn`: the modelled `ImageRecipeArn`
/// pattern (`smithy_query(query="shape",shape="ImageRecipeArn")`) with those two fields
/// spelled as the layer that reads a REQUEST-LINE parameter states them.
const IMAGE_RECIPE_ARN_PATTERN: &str = "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\\.(?:[0-9]+|x)\\.(?:[0-9]+|x)$";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on and
/// the handlers read the Region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
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
    builder.body(payload).expect("a well-formed test request")
}

/// What one call answered, as a client reads it.
struct Answer {
    status: u16,
    /// The error shape the BODY names (`$.__type`), reduced to its shape name.
    code: String,
    /// The error shape the HEADER names, reduced to its shape name.
    error_type: String,
    message: String,
    /// The `requestId` member of the body, or the empty string when it carries none.
    request_id: String,
    /// How many times the body spells the `requestId` member, so a response reported by two
    /// layers would be caught rather than parsed down to one.
    request_id_members: usize,
    body: Value,
}

/// The shape name a discriminator value names, without its namespace or locator.
fn shape_name(raw: &str) -> String {
    raw.rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`: the
/// `:` and `/` of an ARN travel as `%3A` and `%2F`, so the whole ARN stays ONE segment.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Call the emulator and read the answer.
async fn call(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(shape_name)
        .unwrap_or_default();
    let raw = response.into_body();
    let text = String::from_utf8_lossy(&raw).to_string();
    let parsed: Value = serde_json::from_slice(&raw)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {text:?}"));
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(shape_name)
        .unwrap_or_default();
    let message = parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let request_id = parsed
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        code,
        error_type,
        message,
        request_id,
        request_id_members: text.matches("\"requestId\"").count(),
        body: parsed,
    }
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// The not-found message this service states for `arn`, whose kind is `resource_type`.
fn not_found_message(resource_type: &str, arn: &str) -> String {
    format!("The following required resource '{resource_type}' cannot be found: '{arn}'.")
}

/// The refusal sentence this service states for `parameter`, whose value had to satisfy
/// `requirement`.
fn refusal_message(parameter: &str, requirement: &str) -> String {
    format!("The value supplied for parameter '{parameter}' is not valid. {requirement}")
}

/// `CreateComponent` for `name`, answering its build version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("token-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name}");
    let answer = call(router, &case, "PUT", "/CreateComponent", Some(&body)).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("componentBuildVersionArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a component ARN in {}", answer.body))
        .to_string()
}

/// `CreateImageRecipe` for `name` over `component`, answering its ARN.
async fn create_recipe(router: &MultiServiceRouter, name: &str, component: &str) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!("1.0.0"));
    body.insert("parentImage".to_string(), json!("ami-0123456789abcdef0"));
    body.insert("clientToken".to_string(), json!(format!("token-{name}")));
    body.insert(
        "components".to_string(),
        json!([{"componentArn": component}]),
    );
    let case = format!("create the image recipe {name}");
    let answer = call(
        router,
        &case,
        "PUT",
        "/CreateImageRecipe",
        Some(&Value::Object(body)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("imageRecipeArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a recipe ARN in {}", answer.body))
        .to_string()
}

/// The ARN the image recipe `name` carries at `version`.
fn recipe_arn(name: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-recipe/{name}/{version}")
}

/// The ARN one build version of the component `name` carries.
fn component_arn(name: &str, version: &str, build: u32) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/{name}/{version}/{build}")
}

/// The ARN one build version of the image `name` carries.
fn image_build_version_arn(name: &str, version: &str, build: u32) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/{name}/{version}/{build}")
}

/// One row of the failure table.
struct Failure {
    /// What this row is, named in every assertion it makes.
    case: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The status the modelled error for this condition carries.
    status: u16,
    /// The error shape this service models for this condition.
    code: &'static str,
    /// The message it states, or `None` for a refusal this service leaves to the
    /// framework's own wording, whose text is the framework's and not this service's.
    message: Option<String>,
}

#[tokio::test]
async fn every_failure_answers_the_modelled_error_and_names_the_request_it_answers() {
    let router = emulator();

    // One component and one recipe, so the deleted-resource rows differ from a stored one
    // only by the delete, and the success control reads something real back.
    let component = create_component(&router, "envelope-component").await;
    let stored = create_recipe(&router, "envelope-recipe", &component).await;

    // A second recipe, deleted: the read-after-delete lookup.
    let deleted = create_recipe(&router, "envelope-deleted-recipe", &component).await;
    let removed = call(
        &router,
        "delete the recipe",
        "DELETE",
        &format!("/DeleteImageRecipe?imageRecipeArn={deleted}"),
        None,
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case \"delete the recipe\": expected HTTP 200, got {} with {}",
        removed.status, removed.body
    );

    let absent_recipe = recipe_arn("envelope-absent-recipe", "1.0.0");
    let absent_component = component_arn("envelope-absent-component", "1.0.0", 1);
    let absent_image = image_build_version_arn("envelope-absent-image", "1.0.0", 1);

    let rows: Vec<Failure> = vec![
        // A lookup that resolves to nothing: the not-found error, at the status it models.
        Failure {
            case: "a read of an image recipe nothing created".to_string(),
            method: "GET",
            uri: format!("/GetImageRecipe?imageRecipeArn={absent_recipe}"),
            body: None,
            status: 404,
            code: RESOURCE_NOT_FOUND,
            message: Some(not_found_message("ImageRecipe", &absent_recipe)),
        },
        Failure {
            case: "a read of an image recipe after its delete".to_string(),
            method: "GET",
            uri: format!("/GetImageRecipe?imageRecipeArn={deleted}"),
            body: None,
            status: 404,
            code: RESOURCE_NOT_FOUND,
            message: Some(not_found_message("ImageRecipe", &deleted)),
        },
        Failure {
            case: "a repeat delete of an image recipe after its delete".to_string(),
            method: "DELETE",
            uri: format!("/DeleteImageRecipe?imageRecipeArn={deleted}"),
            body: None,
            status: 404,
            code: RESOURCE_NOT_FOUND,
            message: Some(not_found_message("ImageRecipe", &deleted)),
        },
        Failure {
            case: "a read of a component nothing created".to_string(),
            method: "GET",
            uri: format!("/GetComponent?componentBuildVersionArn={absent_component}"),
            body: None,
            status: 404,
            code: RESOURCE_NOT_FOUND,
            message: Some(not_found_message("Component", &absent_component)),
        },
        // The same lookup through a PATH label rather than a query parameter. The tag WRITE
        // is the one of the pair this service's own lookup answers; the tag READ is
        // answered by the resource-tagging layer in ITS own class, which
        // `tests/all/tagging_not_found_class.rs` covers.
        Failure {
            case: "a tag write to an image recipe nothing created".to_string(),
            method: "POST",
            uri: format!("/tags/{}", encode_label(&absent_recipe)),
            body: Some(json!({ "tags": { "Owner": "platform" } })),
            status: 404,
            code: RESOURCE_NOT_FOUND,
            message: Some(not_found_message("ImageRecipe", &absent_recipe)),
        },
        // A value a modelled pattern does not admit, read out of the BODY: the
        // invalid-parameter error, naming the parameter and the expression its value had to
        // match.
        Failure {
            case: "a package listing of an image ARN carrying no build number".to_string(),
            method: "POST",
            uri: "/ListImagePackages".to_string(),
            body: Some(json!({
                "imageBuildVersionArn":
                    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/envelope-image/1.0.0"),
            })),
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            message: Some(refusal_message(
                "imageBuildVersionArn",
                &format!(
                    "imageBuildVersionArn must match pattern {IMAGE_BUILD_VERSION_ARN_PATTERN}"
                ),
            )),
        },
        Failure {
            case: "a package listing of text that is no ARN at all".to_string(),
            method: "POST",
            uri: "/ListImagePackages".to_string(),
            body: Some(json!({"imageBuildVersionArn": "envelope-image"})),
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            message: Some(refusal_message(
                "imageBuildVersionArn",
                &format!(
                    "imageBuildVersionArn must match pattern {IMAGE_BUILD_VERSION_ARN_PATTERN}"
                ),
            )),
        },
        Failure {
            case: "a package listing of another resource kind's ARN".to_string(),
            method: "POST",
            uri: "/ListImagePackages".to_string(),
            body: Some(json!({"imageBuildVersionArn": stored.clone()})),
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            message: Some(refusal_message(
                "imageBuildVersionArn",
                &format!(
                    "imageBuildVersionArn must match pattern {IMAGE_BUILD_VERSION_ARN_PATTERN}"
                ),
            )),
        },
        // The same class of refusal for a value read out of the REQUEST LINE, which states
        // the expression in the form that layer writes.
        Failure {
            case: "a recipe read whose ARN names another resource kind".to_string(),
            method: "GET",
            uri: format!("/GetImageRecipe?imageRecipeArn={absent_component}"),
            body: None,
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            message: Some(refusal_message(
                "imageRecipeArn",
                &format!("imageRecipeArn must match pattern {IMAGE_RECIPE_ARN_PATTERN}"),
            )),
        },
        // A continuation token is opaque, so a value this listing did not issue is a value
        // that parameter does not admit — refused behind the door, by the handler itself.
        Failure {
            case: "a package listing carrying a token this listing did not issue".to_string(),
            method: "POST",
            uri: "/ListImagePackages".to_string(),
            body: Some(json!({
                "imageBuildVersionArn": absent_image.clone(),
                "nextToken": "not-a-token-this-listing-issued==",
            })),
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            message: Some(refusal_message(
                "nextToken",
                "The provided token does not meet the required format.",
            )),
        },
        // A page size outside the modelled range, which the same sentence states.
        Failure {
            case: "a package listing asking for more than the modelled page maximum".to_string(),
            method: "POST",
            uri: "/ListImagePackages".to_string(),
            body: Some(json!({
                "imageBuildVersionArn": absent_image.clone(),
                "maxResults": 26,
            })),
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            message: Some(refusal_message(
                "maxResults",
                "maxResults must be smaller than or equal to 25",
            )),
        },
        // A constraint class this service states no requirement for keeps the framework's
        // own refusal — which still names the request it answers, because the report is the
        // envelope's and not any one wording's.
        Failure {
            case: "a create omitting a member its shape requires".to_string(),
            method: "PUT",
            uri: "/CreateComponent".to_string(),
            body: Some(json!({
                "name": "envelope-incomplete",
                "platform": "Linux",
                "clientToken": "token-envelope-incomplete",
                "data": BUILD_DOCUMENT,
            })),
            status: 400,
            code: "ValidationException",
            message: None,
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case.as_str();
        let answered = call(&router, case, row.method, &row.uri, row.body.as_ref()).await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with {}",
            row.status, answered.status, answered.body
        );
        assert_eq!(
            answered.code, row.code,
            "case {case:?}: expected $.__type {:?}, got {:?}",
            row.code, answered.code
        );
        assert_eq!(
            answered.error_type, row.code,
            "case {case:?}: expected x-amzn-errortype {:?}, got {:?}",
            row.code, answered.error_type
        );
        if let Some(expected) = &row.message {
            assert_eq!(
                answered.message, *expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                answered.message
            );
        }
        assert!(
            is_request_id(&answered.request_id),
            "case {case:?}: expected the failure to name the request it answers, got {:?} in {}",
            answered.request_id,
            answered.body
        );
        assert_eq!(
            answered.request_id_members, 1,
            "case {case:?}: expected exactly one requestId member, got {} in {}",
            answered.request_id_members, answered.body
        );
        assert!(
            !seen.contains(&answered.request_id),
            "case {case:?}: expected an id of its own, got {:?}, already answered to another \
             request",
            answered.request_id
        );
        seen.push(answered.request_id);
    }
}

#[tokio::test]
async fn one_condition_answered_twice_names_each_request_and_leaves_the_store_alone() {
    let router = emulator();
    let component = create_component(&router, "repeat-component").await;
    let stored = create_recipe(&router, "repeat-recipe", &component).await;
    let absent = recipe_arn("repeat-absent-recipe", "1.0.0");

    // The id names the REQUEST, not the condition: two identical failures carry two ids,
    // and both state the same condition.
    let mut ids: Vec<String> = Vec::new();
    for attempt in 1..=2 {
        let case = format!("attempt {attempt} at a recipe nothing created");
        let answered = call(
            &router,
            &case,
            "GET",
            &format!("/GetImageRecipe?imageRecipeArn={absent}"),
            None,
        )
        .await;
        assert_eq!(
            answered.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.message,
            not_found_message("ImageRecipe", &absent),
            "case {case:?}: expected the same condition stated, got {:?}",
            answered.message
        );
        assert!(
            is_request_id(&answered.request_id),
            "case {case:?}: expected a request id, got {:?}",
            answered.request_id
        );
        ids.push(answered.request_id);
    }
    assert_ne!(
        ids[0], ids[1],
        "case \"two reads of one absent recipe\": expected an id per request, got {:?} twice",
        ids[0]
    );

    // A SUCCESS states the id through the member its own output shape models, and the
    // failure envelope adds nothing to it: one member, and a usable id.
    let case = "a read of the stored recipe";
    let answered = call(
        &router,
        case,
        "GET",
        &format!("/GetImageRecipe?imageRecipeArn={stored}"),
        None,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.request_id_members, 1,
        "case {case:?}: expected exactly one requestId member, got {} in {}",
        answered.request_id_members, answered.body
    );
    assert!(
        is_request_id(&answered.request_id),
        "case {case:?}: expected the output's own requestId, got {:?}",
        answered.request_id
    );
    assert!(
        answered.body.get("__type").is_none(),
        "case {case:?}: expected no error discriminator on a success, got {}",
        answered.body
    );
}
