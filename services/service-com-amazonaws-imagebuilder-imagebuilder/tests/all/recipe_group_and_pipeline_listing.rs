// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a create that must name exactly one of a GROUP of parameters,
//! and for the pipeline listing that shows such a refused create stored nothing.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: where a create has to name exactly one of a group of parameters —
//! `imageRecipeArn` or `containerRecipeArn`, which the model makes individually optional
//! because either may be the one — a request that names NONE of the group is refused as a
//! MISSING PARAMETER, with the wire code `MissingParametersException` (HTTP 400) and the
//! sentence "Required parameters are missing from the request: '<member>' or '<member>'.
//! Verify that you have entered all of the required parameters in your request and try
//! again.", quoting the members in the order the input shape declares them. That is a
//! different failure from naming BOTH, which is a combination that cannot hold together and
//! keeps the modelled `InvalidRequestException`, and from naming one that resolves to
//! nothing, which reports the resource missing. Nothing is stored by any of the three, which
//! the listing of the resource the create would have made is what shows.
//!
//! The rows span what the rule generalises over: which create carries the group
//! (`CreateImagePipeline`, `CreateImage`), and which of the group's members the request
//! names.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The sentence a create that named neither recipe is refused with. The two member names
/// are the group the request had to name one of, in the order `CreateImagePipeline`'s input
/// shape declares them.
const MISSING_RECIPE_MESSAGE: &str =
    "Required parameters are missing from the request: 'imageRecipeArn' or \
     'containerRecipeArn'. Verify that you have entered all of the required parameters in \
     your request and try again.";

/// A build component document: one `build` phase, which is all a recipe these cases name
/// needs.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    \
                              steps:\n      - name: Greet\n        action: ExecuteBash\n        \
                              inputs:\n          commands:\n            - echo hello\n";

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

/// A request signed for `imagebuilder` in `region`.
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

/// Status and parsed body of one call.
async fn call_in(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(REGION, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error code a refusal carries, from `__type`.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Assert the answer is a refusal carrying `status`, `code` and `message`.
fn assert_refused(case: &str, status: u16, body: &Value, expected: u16, code: &str, message: &str) {
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected}, got {status} with {body}"
    );
    let actual_code = error_code(case, body);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected {code:?}, got {actual_code:?} with {body}"
    );
    let actual_message = text(case, body, "message");
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected the message {message:?}, got {actual_message:?}"
    );
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// `CreateComponent` for one build version of `name`, answering its build version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("component-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name}");
    let (status, answer) = call_in(router, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name`, including `component_arn`, answering the recipe's ARN.
async fn create_recipe(router: &MultiServiceRouter, name: &str, component_arn: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": format!("recipe-{name}"),
        "components": [{ "componentArn": component_arn }],
    });
    let case = format!("create the image recipe {name}");
    let (status, answer) = call_in(router, "PUT", "/CreateImageRecipe", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let case = format!("create the infrastructure configuration {name}");
    let (status, answer) = call_in(
        router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// `ListImagePipelines` with `body`.
async fn list_pipelines(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in(router, "POST", "/ListImagePipelines", Some(body)).await
}

/// The ARNs one `ListImagePipelines` answer carries, in the order it reports them.
fn listed_pipelines(case: &str, body: &Value) -> Vec<String> {
    body.get("imagePipelineList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipelineList in {body}"))
        .iter()
        .map(|entry| text(case, entry, "arn"))
        .collect()
}

/// The pipelines whose `name` is `name`, as the listing reports them.
async fn pipelines_named(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<String> {
    let body = json!({ "filters": [{ "name": "name", "values": [name] }] });
    let (status, listed) = list_pipelines(router, &body).await;
    let listed = assert_ok(case, status, listed);
    listed_pipelines(case, &listed)
}

/// Which create carries the recipe group, and how a request to it is built.
struct Create {
    /// What a failing assertion calls it.
    case: &'static str,
    /// Its request target, which is also its operation name.
    target: &'static str,
    /// The member its successful answer names the created resource by.
    created_member: &'static str,
}

/// One refusal a row expects: the status, the wire code and the sentence.
type Refusal = (u16, &'static str, String);

/// One row of the group table: what it is called, the recipe members the request names, and
/// the refusal it earns (or `None` when the request succeeds).
type GroupRow = (&'static str, Vec<(&'static str, String)>, Option<Refusal>);

#[tokio::test]
async fn a_create_naming_none_of_the_recipe_group_reports_the_group_missing() {
    let router = emulator();
    let component = create_component(&router, "group-component").await;
    let recipe = create_recipe(&router, "group-recipe", &component).await;
    let configuration = create_configuration(&router, "group-infra").await;
    let absent_container =
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:container-recipe/absent/1.0.0");

    let creates = [
        Create {
            case: "CreateImagePipeline",
            target: "/CreateImagePipeline",
            created_member: "imagePipelineArn",
        },
        Create {
            case: "CreateImage",
            target: "/CreateImage",
            created_member: "imageBuildVersionArn",
        },
    ];

    for create in &creates {
        // The request every row below starts from: valid but for the recipe group.
        let base = |token: &str| {
            let mut body = Map::new();
            // Only the pipeline create takes a name; the build create does not model one.
            if create.created_member == "imagePipelineArn" {
                body.insert("name".to_string(), json!("group-pipeline"));
            }
            body.insert(
                "infrastructureConfigurationArn".to_string(),
                json!(configuration.clone()),
            );
            body.insert("clientToken".to_string(), json!(token));
            body
        };

        // (case, the recipe members the request names, the refusal it earns)
        let rows: Vec<GroupRow> = vec![
            (
                "neither member of the group",
                Vec::new(),
                Some((
                    400,
                    "MissingParametersException",
                    MISSING_RECIPE_MESSAGE.to_string(),
                )),
            ),
            (
                "both members of the group",
                vec![
                    ("imageRecipeArn", recipe.clone()),
                    ("containerRecipeArn", absent_container.clone()),
                ],
                Some((
                    400,
                    "InvalidRequestException",
                    "The request must specify exactly one of imageRecipeArn and \
                     containerRecipeArn."
                        .to_string(),
                )),
            ),
            (
                "one member of the group, naming nothing stored",
                vec![("containerRecipeArn", absent_container.clone())],
                Some((
                    404,
                    "ResourceNotFoundException",
                    format!(
                        "The following required resource 'ContainerRecipe' cannot be found: \
                         '{absent_container}'."
                    ),
                )),
            ),
            (
                "exactly one member of the group",
                vec![("imageRecipeArn", recipe.clone())],
                None,
            ),
        ];

        for (position, (row, members, refusal)) in rows.into_iter().enumerate() {
            let case = format!("{}, {row}", create.case);
            let mut body = base(&format!("{}-{position}", create.created_member));
            for (member, value) in members {
                body.insert(member.to_string(), json!(value));
            }
            let (status, answer) =
                call_in(&router, "PUT", create.target, Some(&Value::Object(body))).await;
            match refusal {
                Some((expected_status, code, message)) => {
                    assert_refused(&case, status, &answer, expected_status, code, &message);
                    // Nothing was created by a refused request. For the pipeline create
                    // that is the listing of the pipeline it would have made; the build
                    // create's own listing is covered by the build suite.
                    if create.created_member == "imagePipelineArn" {
                        let listed = pipelines_named(&router, &case, "group-pipeline").await;
                        assert!(
                            listed.is_empty(),
                            "case {case:?}: expected a refused create to store nothing, got \
                             {listed:?}"
                        );
                    }
                }
                None => {
                    let answer = assert_ok(&case, status, answer);
                    let created = text(&case, &answer, create.created_member);
                    assert!(
                        created.starts_with(&format!("arn:aws:imagebuilder:{REGION}:")),
                        "case {case:?}: expected a created resource's ARN, got {created:?}"
                    );
                    if create.created_member == "imagePipelineArn" {
                        let listed = pipelines_named(&router, &case, "group-pipeline").await;
                        assert_eq!(
                            listed,
                            vec![created],
                            "case {case:?}: expected the listing to carry the created \
                             pipeline, got {listed:?}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_pipeline_listing_filters_and_pages_over_what_it_holds() {
    let router = emulator();
    let component = create_component(&router, "listing-component").await;
    let recipe = create_recipe(&router, "listing-recipe", &component).await;
    let configuration = create_configuration(&router, "listing-infra").await;

    // Three pipelines, differing in the members a filter selects on.
    let mut created: Vec<String> = Vec::new();
    for (name, status_value) in [
        ("alpha", "ENABLED"),
        ("beta", "DISABLED"),
        ("gamma", "ENABLED"),
    ] {
        let body = json!({
            "name": name,
            "clientToken": format!("pipeline-{name}"),
            "imageRecipeArn": recipe,
            "infrastructureConfigurationArn": configuration,
            "status": status_value,
            "description": format!("the {name} pipeline"),
        });
        let case = format!("create the pipeline {name}");
        let (status, answer) = call_in(&router, "PUT", "/CreateImagePipeline", Some(&body)).await;
        let answer = assert_ok(&case, status, answer);
        created.push(text(&case, &answer, "imagePipelineArn"));
    }

    // (case, the request, the pipelines it answers with)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter", json!({}), created.clone()),
        (
            "a filter on the name",
            json!({ "filters": [{ "name": "name", "values": ["beta"] }] }),
            vec![created[1].clone()],
        ),
        (
            "a filter on the status, naming two values",
            json!({ "filters": [{ "name": "status", "values": ["ENABLED", "DISABLED"] }] }),
            created.clone(),
        ),
        (
            "a filter on the recipe every pipeline names",
            json!({ "filters": [{ "name": "imageRecipeArn", "values": [recipe.clone()] }] }),
            created.clone(),
        ),
        (
            "a filter on a description none carries",
            json!({ "filters": [{ "name": "description", "values": ["absent"] }] }),
            Vec::new(),
        ),
    ];
    for (case, body, expected) in cases {
        let (status, listed) = list_pipelines(&router, &body).await;
        let listed = assert_ok(case, status, listed);
        let arns = listed_pipelines(case, &listed);
        assert_eq!(
            arns, expected,
            "case {case:?}: expected {expected:?}, got {arns:?}"
        );
    }

    // Two entries per page, followed to the end: the pages together are the whole listing.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = Map::new();
        body.insert("maxResults".to_string(), json!(2));
        if let Some(token) = &token {
            body.insert("nextToken".to_string(), json!(token));
        }
        let (status, listed) = list_pipelines(&router, &Value::Object(body)).await;
        let listed = assert_ok("two entries per page", status, listed);
        let page = listed_pipelines("two entries per page", &listed);
        assert!(
            page.len() <= 2,
            "case \"two entries per page\": expected at most two entries, got {page:?}"
        );
        seen.extend(page);
        token = listed
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() <= created.len(),
            "case \"two entries per page\": the pages did not end, got {seen:?}"
        );
    }
    assert_eq!(
        seen, created,
        "case \"two entries per page\": expected the whole listing, got {seen:?}"
    );

    // A token this store did not issue is refused rather than read as a position, and so is
    // a filter naming a member this listing does not filter on.
    let (status, answer) = list_pipelines(
        &router,
        &json!({ "nextToken": "not-a-real-pagination-token==" }),
    )
    .await;
    assert_refused(
        "a forged page token",
        status,
        &answer,
        400,
        "InvalidParameterValueException",
        "The value supplied for parameter 'nextToken' is not valid. The provided token does not \
         meet the required format.",
    );
    let (status, answer) = list_pipelines(
        &router,
        &json!({ "filters": [{ "name": "platform", "values": ["Linux"] }] }),
    )
    .await;
    assert_eq!(
        (
            status,
            error_code("a filter on an unlisted member", &answer)
        ),
        (400, "InvalidParameterValueException".to_string()),
        "case \"a filter on an unlisted member\": got {answer}"
    );
}
