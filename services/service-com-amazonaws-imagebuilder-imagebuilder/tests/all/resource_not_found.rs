// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what this service answers a request that names a resource it
//! does not hold.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: when an ARN a request carries resolves to no resource of the
//! calling Region — because it was deleted, or was never created, or belongs to another
//! Region — the call is answered with the not-found error this service models for a
//! referenced resource it does not hold (`ResourceNotFoundException`, HTTP 404, "At least
//! one of the resources referenced by your request does not exist"), whose message names
//! the kind of resource the ARN identifies and the ARN the request carried, verbatim.
//! That is one answer for every lookup THIS SERVICE performs, whatever the operation does
//! with what it finds: a read, a delete, a policy write, a tagging write, and the reference
//! resolution of a create.
//!
//! The TAG LISTING is the one lookup answered by another layer and so by another class: it
//! reports an ARN that resolves to nothing as the resource-tagging layer's own
//! `NotFoundException` (HTTP 404) with the fixed sentence "Resource was not found", which
//! names neither the kind nor the ARN. Each row below states which of the two classes its
//! answer carries.
//!
//! The rows span what the rule generalises over: which operation performs the lookup,
//! which kind of resource the ARN names, and why the ARN resolves to nothing (never
//! created, deleted, another Region, a version or build number nothing was stored under,
//! a wildcard version that matches nothing).

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all a component these cases
/// look up needs.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

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

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
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
        .and_then(|v| v.to_str().ok())
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

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`: the
/// `:` and `/` of an ARN travel as `%3A` and `%2F`, so the whole ARN stays ONE segment.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
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

/// The `message` a refusal carries.
fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"))
        .to_string()
}

/// Assert `status` and the answer are the not-found report the rule requires for `arn`:
/// HTTP 404, the error type this service models for a resource it does not hold, and the
/// message that names `resource_type` and `arn`.
fn assert_reported_missing(
    case: &str,
    status: u16,
    body: &Value,
    resource_type: &str,
    arn: &str,
) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?} with {body}"
    );
    let expected = format!("The following required resource '{resource_type}' cannot be found: '{arn}'.");
    let actual = error_message(case, body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
}

/// Assert `status` and the answer are the resource-tagging layer's own not-found report:
/// HTTP 404, its own error class, and its fixed sentence — which names neither the kind of
/// resource nor the ARN the request carried.
fn assert_tagging_not_found(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, "NotFoundException",
        "case {case:?}: expected NotFoundException, got {code:?} with {body}"
    );
    let actual = error_message(case, body);
    assert_eq!(
        actual, "Resource was not found",
        "case {case:?}: expected the tagging layer's fixed sentence, got {actual:?}"
    );
}

/// Read `name` out of an answer, panicking with the case when it is absent.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Assert the call succeeded and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// `CreateComponent` for one build version of `name` in `region`, answering its build
/// version ARN.
async fn create_component(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    version: &str,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert(
        "clientToken".to_string(),
        json!(format!("token-{region}-{name}-{version}")),
    );
    body.insert("data".to_string(), json!(BUILD_DOCUMENT));
    let case = format!("create the component {name} {version} in {region}");
    let (status, answer) =
        call_in(router, region, "PUT", "/CreateComponent", Some(&Value::Object(body))).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name` in `region`, including whatever `components` names,
/// answering the status and the body.
async fn create_recipe_with(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    components: Option<Value>,
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!("1.0.0"));
    body.insert("parentImage".to_string(), json!("ami-0123456789abcdef0"));
    body.insert(
        "clientToken".to_string(),
        json!(format!("token-{region}-{name}")),
    );
    if let Some(components) = components {
        body.insert("components".to_string(), components);
    }
    call_in(
        router,
        region,
        "PUT",
        "/CreateImageRecipe",
        Some(&Value::Object(body)),
    )
    .await
}

/// `CreateImageRecipe` for `name` in `region`, answering its ARN.
async fn create_recipe(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let case = format!("create the image recipe {name} in {region}");
    let (status, answer) = create_recipe_with(router, region, name, None).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name` in `region`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert(
        "clientToken".to_string(),
        json!(format!("token-{region}-{name}")),
    );
    body.insert(
        "instanceProfileName".to_string(),
        json!("EC2InstanceProfileForImageBuilder"),
    );
    let case = format!("create the infrastructure configuration {name} in {region}");
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&Value::Object(body)),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// The ARN one build version of the component `normalized` carries in `region`.
fn component_arn(region: &str, normalized: &str, version: &str, build: u32) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:component/{normalized}/{version}/{build}")
}

/// The ARN the image recipe `normalized` carries at `version` in `region`.
fn recipe_arn(region: &str, normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image-recipe/{normalized}/{version}")
}

/// The ARN the infrastructure configuration `normalized` carries in `region`.
fn configuration_arn(region: &str, normalized: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:infrastructure-configuration/{normalized}")
}

/// Which not-found class a lookup's answer carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MissingReport {
    /// This service's own modelled not-found error, naming the kind and the ARN.
    Modelled,
    /// The resource-tagging layer's own class, whose sentence is fixed text.
    TaggingPlane,
}

/// One call that looks a resource up: how to send it, and the ARN plus resource-type token
/// its answer has to report missing.
struct Lookup {
    /// What this row is, named in every assertion it makes.
    case: String,
    /// The HTTP method the modelled operation binds.
    method: &'static str,
    /// The request target, the ARN included where the operation takes it in the URI.
    uri: String,
    /// The request body, for an operation that takes the ARN in one.
    body: Option<Value>,
    /// The Region the request is signed for.
    region: &'static str,
    /// The resource-type token the report has to name.
    resource_type: &'static str,
    /// The ARN the report has to quote, verbatim as the request carried it.
    arn: String,
    /// Which class the answer carries.
    report: MissingReport,
}

/// Assert one lookup's answer is the not-found report its class requires.
fn assert_lookup_missing(lookup: &Lookup, status: u16, body: &Value) {
    match lookup.report {
        MissingReport::Modelled => assert_reported_missing(
            &lookup.case,
            status,
            body,
            lookup.resource_type,
            &lookup.arn,
        ),
        MissingReport::TaggingPlane => assert_tagging_not_found(&lookup.case, status, body),
    }
}

/// Every operation that looks up the resource `arn` names by the member that names it.
fn lookups_of(case: &str, region: &'static str, resource_type: &'static str, arn: &str) -> Vec<Lookup> {
    let mut rows = Vec::new();
    let mut push = |operation: &str, method: &'static str, uri: String, body: Option<Value>| {
        rows.push(Lookup {
            case: format!("{operation} on {case}"),
            method,
            uri,
            body,
            region,
            resource_type,
            arn: arn.to_string(),
            report: MissingReport::Modelled,
        });
    };
    match resource_type {
        "Component" => {
            push(
                "GetComponent",
                "GET",
                format!("/GetComponent?componentBuildVersionArn={arn}"),
                None,
            );
            // `DeleteComponent` and the policy pair model their ARN member as a BUILD
            // version ARN (`ComponentBuildVersionArn`), so a version ARN whose trailing
            // nodes are wildcards is refused by that pattern before any handler runs and
            // never reaches the lookup under test.
            let names_a_build_version = arn
                .rsplit(':')
                .next()
                .is_some_and(|resource| resource.split('/').count() == 4);
            if names_a_build_version {
                push(
                    "DeleteComponent",
                    "DELETE",
                    format!("/DeleteComponent?componentBuildVersionArn={arn}"),
                    None,
                );
                push(
                    "GetComponentPolicy",
                    "GET",
                    format!("/GetComponentPolicy?componentArn={arn}"),
                    None,
                );
                push(
                    "PutComponentPolicy",
                    "PUT",
                    "/PutComponentPolicy".to_string(),
                    Some(json!({"componentArn": arn, "policy": "{\"Version\":\"2012-10-17\"}"})),
                );
            }
        }
        "ImageRecipe" => {
            push(
                "GetImageRecipe",
                "GET",
                format!("/GetImageRecipe?imageRecipeArn={arn}"),
                None,
            );
            push(
                "DeleteImageRecipe",
                "DELETE",
                format!("/DeleteImageRecipe?imageRecipeArn={arn}"),
                None,
            );
        }
        "InfrastructureConfiguration" => {
            push(
                "GetInfrastructureConfiguration",
                "GET",
                format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
                None,
            );
        }
        other => panic!("case {case:?}: no lookup is declared for the resource type {other:?}"),
    }
    // The tagging pair takes the ARN of ANY kind, in the URI path. The LISTING is answered
    // by the tagging layer's own class; the WRITE is this service's own lookup.
    rows.push(Lookup {
        case: format!("ListTagsForResource on {case}"),
        method: "GET",
        uri: format!("/tags/{}", encode_label(arn)),
        body: None,
        region,
        resource_type,
        arn: arn.to_string(),
        report: MissingReport::TaggingPlane,
    });
    rows.push(Lookup {
        case: format!("TagResource on {case}"),
        method: "POST",
        uri: format!("/tags/{}", encode_label(arn)),
        body: Some(json!({"tags": {"Owner": "platform"}})),
        region,
        resource_type,
        arn: arn.to_string(),
        report: MissingReport::Modelled,
    });
    rows
}

#[tokio::test]
async fn an_arn_that_resolves_to_nothing_is_reported_missing_by_every_lookup() {
    let router = emulator();

    // One resource of each kind this store holds, so the absent ARNs below differ from a
    // stored one only in the way each row names.
    let component = create_component(&router, REGION, "present-component", "1.0.0").await;
    let recipe = create_recipe(&router, REGION, "present-recipe").await;
    let configuration = create_configuration(&router, REGION, "Present Infra").await;

    // Each row is an ARN that resolves to nothing, and why. The reasons span the whole
    // rule: a resource nothing ever created, a stored resource reached from another
    // Region's endpoint, a name that only resembles a stored one, and — for the kinds
    // whose ARN carries a version — a version, a build number and a wildcard version
    // nothing was stored under.
    let absent: Vec<(String, &'static str, String)> = vec![
        (
            "a component nothing created".to_string(),
            "Component",
            component_arn(REGION, "absent-component", "1.0.0", 1),
        ),
        (
            "a build number nothing was stored under".to_string(),
            "Component",
            component_arn(REGION, "present-component", "1.0.0", 2),
        ),
        (
            "a version nothing was stored under".to_string(),
            "Component",
            component_arn(REGION, "present-component", "9.9.9", 1),
        ),
        (
            "a wildcard component version that matches nothing".to_string(),
            "Component",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/present-component/2.x.x"),
        ),
        (
            "a stored component read from another Region's endpoint".to_string(),
            "Component",
            component.clone(),
        ),
        (
            "an image recipe nothing created".to_string(),
            "ImageRecipe",
            recipe_arn(REGION, "absent-recipe", "1.0.0"),
        ),
        (
            "an image recipe version nothing was stored under".to_string(),
            "ImageRecipe",
            recipe_arn(REGION, "present-recipe", "2.0.0"),
        ),
        (
            "a stored image recipe read from another Region's endpoint".to_string(),
            "ImageRecipe",
            recipe.clone(),
        ),
        (
            "an infrastructure configuration nothing created".to_string(),
            "InfrastructureConfiguration",
            configuration_arn(REGION, "absent-infra"),
        ),
        (
            "a configuration name that only resembles a stored one".to_string(),
            "InfrastructureConfiguration",
            configuration_arn(REGION, "present-infra-2"),
        ),
        (
            "a stored configuration read from another Region's endpoint".to_string(),
            "InfrastructureConfiguration",
            configuration.clone(),
        ),
    ];

    for (case, resource_type, arn) in &absent {
        // A row naming a STORED resource is the one asked from the other Region's
        // endpoint; every other row is asked from the Region that holds its neighbours.
        let region = if arn == &component || arn == &recipe || arn == &configuration {
            OTHER_REGION
        } else {
            REGION
        };
        for lookup in lookups_of(case, region, resource_type, arn) {
            let (status, body) = call_in(
                &router,
                lookup.region,
                lookup.method,
                &lookup.uri,
                lookup.body.as_ref(),
            )
            .await;
            assert_lookup_missing(&lookup, status, &body);
        }
    }

    // The same lookups answer for the resources that ARE stored, so the rows above report
    // what is absent rather than refusing everything.
    for (case, arn) in [
        ("the stored component", &component),
        ("the stored image recipe", &recipe),
        ("the stored configuration", &configuration),
    ] {
        let (status, body) = call_in(
            &router,
            REGION,
            "GET",
            &format!("/tags/{}", encode_label(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading its tags, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_read_or_a_repeat_delete_after_a_delete_reports_the_resource_missing() {
    let router = emulator();

    // Create a component and a recipe, prove both read back, then delete each and prove
    // every later lookup of them reports them missing — the read-after-delete and the
    // mutate-after-delete of the same ARN.
    let component = create_component(&router, REGION, "lifecycle-component", "1.0.0").await;
    let recipe = create_recipe(&router, REGION, "lifecycle-recipe").await;

    let (status, read) = call_in(
        &router,
        REGION,
        "GET",
        &format!("/GetComponent?componentBuildVersionArn={component}"),
        None,
    )
    .await;
    assert_ok("the component before the delete", status, read);
    let (status, read) = call_in(
        &router,
        REGION,
        "GET",
        &format!("/GetImageRecipe?imageRecipeArn={recipe}"),
        None,
    )
    .await;
    assert_ok("the recipe before the delete", status, read);

    let (status, deleted) = call_in(
        &router,
        REGION,
        "DELETE",
        &format!("/DeleteImageRecipe?imageRecipeArn={recipe}"),
        None,
    )
    .await;
    assert_ok("the recipe delete", status, deleted);
    let (status, deleted) = call_in(
        &router,
        REGION,
        "DELETE",
        &format!("/DeleteComponent?componentBuildVersionArn={component}"),
        None,
    )
    .await;
    assert_ok("the component delete", status, deleted);

    for (case, resource_type, arn) in [
        ("the deleted component", "Component", &component),
        ("the deleted image recipe", "ImageRecipe", &recipe),
    ] {
        for lookup in lookups_of(case, REGION, resource_type, arn) {
            let (status, body) = call_in(
                &router,
                lookup.region,
                lookup.method,
                &lookup.uri,
                lookup.body.as_ref(),
            )
            .await;
            assert_lookup_missing(&lookup, status, &body);
        }
    }
}

#[tokio::test]
async fn a_create_whose_component_reference_resolves_to_nothing_reports_it_missing() {
    let router = emulator();
    let component = create_component(&router, REGION, "referenced-component", "1.0.0").await;

    // A create resolves the components it names the same way a read resolves one, so a
    // reference that reaches nothing is the same report — naming the kind the reference
    // names and the reference verbatim, not the recipe the request would have created.
    let references: Vec<(&str, String)> = vec![
        (
            "a componentArn nothing created",
            component_arn(REGION, "absent-component", "1.0.0", 1),
        ),
        (
            "a build number nothing was stored under",
            component_arn(REGION, "referenced-component", "1.0.0", 9),
        ),
        (
            "a wildcard version that matches nothing",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/referenced-component/3.x.x"),
        ),
        (
            "a stored component of another Region",
            component.replace(REGION, OTHER_REGION),
        ),
    ];

    for (n, (case, reference)) in references.iter().enumerate() {
        let name = format!("refused-recipe-{n}");
        let (status, body) = create_recipe_with(
            &router,
            REGION,
            &name,
            Some(json!([{"componentArn": reference}])),
        )
        .await;
        assert_reported_missing(case, status, &body, "Component", reference);

        // The refused create stored nothing, so the ARN it would have minted is itself
        // reported missing — under the recipe's own kind.
        let arn = recipe_arn(REGION, &name, "1.0.0");
        let (status, body) = call_in(
            &router,
            REGION,
            "GET",
            &format!("/GetImageRecipe?imageRecipeArn={arn}"),
            None,
        )
        .await;
        assert_reported_missing(
            &format!("{case}, the recipe it would have created"),
            status,
            &body,
            "ImageRecipe",
            &arn,
        );
    }

    // The same create, naming the component that IS stored, is stored itself.
    let (status, created) = create_recipe_with(
        &router,
        REGION,
        "accepted-recipe",
        Some(json!([{"componentArn": component}])),
    )
    .await;
    assert_ok("a reference that resolves", status, created);
}

#[tokio::test]
async fn the_reported_resource_type_is_the_kind_the_arn_identifies() {
    let router = emulator();

    // The tagging pair takes the ARN of ANY Image Builder resource, so it is where every
    // kind an ARN may name reaches a lookup. The kinds are the alternation of the modelled
    // `ImageBuilderArn` pattern (`smithy_query(query="shape",shape="ImageBuilderArn")`):
    // image-recipe, container-recipe, infrastructure-configuration,
    // distribution-configuration, component, image, image-pipeline, lifecycle-policy and
    // workflow/(build|test|distribution). Each row pairs one of them with the name this
    // model gives that kind, which is the token the report names.
    let kinds: &[(&str, &str, &str)] = &[
        ("a component", "component/absent/1.0.0/1", "Component"),
        ("an image", "image/absent/1.0.0/1", "Image"),
        ("an image recipe", "image-recipe/absent/1.0.0", "ImageRecipe"),
        (
            "a container recipe",
            "container-recipe/absent/1.0.0",
            "ContainerRecipe",
        ),
        ("an image pipeline", "image-pipeline/absent", "ImagePipeline"),
        (
            "an infrastructure configuration",
            "infrastructure-configuration/absent",
            "InfrastructureConfiguration",
        ),
        (
            "a distribution configuration",
            "distribution-configuration/absent",
            "DistributionConfiguration",
        ),
        (
            "a lifecycle policy",
            "lifecycle-policy/absent",
            "LifecyclePolicy",
        ),
        ("a build workflow", "workflow/build/absent/1.0.0/1", "Workflow"),
    ];

    for (case, resource, resource_type) in kinds {
        let arn = format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:{resource}");
        let (status, body) = call_in(
            &router,
            REGION,
            "GET",
            &format!("/tags/{}", encode_label(&arn)),
            None,
        )
        .await;
        assert_tagging_not_found(&format!("ListTagsForResource on {case}"), status, &body);

        let (status, body) = call_in(
            &router,
            REGION,
            "POST",
            &format!("/tags/{}", encode_label(&arn)),
            Some(&json!({"tags": {"Owner": "platform"}})),
        )
        .await;
        assert_reported_missing(
            &format!("TagResource on {case}"),
            status,
            &body,
            resource_type,
            &arn,
        );
    }
}
