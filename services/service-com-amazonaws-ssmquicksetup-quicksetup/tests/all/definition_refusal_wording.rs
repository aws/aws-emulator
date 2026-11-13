// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for two refusals of a configuration definition that the MODEL
//! cannot state, and for the one part of a modelled refusal AWS words its own way.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! Two rules are under test:
//!
//! * **the create's definition list must name a definition.**
//!   `CreateConfigurationManagerInput$ConfigurationDefinitions` is `@required` and its
//!   target list carries no `@length`, so an EMPTY list reaches the handler; a
//!   configuration manager is the deployment of its definitions, so one with none to
//!   deploy is refused HTTP 400 `ValidationException` with AWS's own fixed sentence
//!   (`crate::state::NO_CONFIGURATION_DEFINITIONS_MESSAGE`), which interpolates nothing
//!   from the request — asserted over requests that differ in everything else they carry,
//!   and against lists that DO name definitions, which are served.
//! * **a `@pattern` requirement is quoted with the model's OUTER anchor pair dropped and
//!   every anchor INSIDE the expression kept.** The anchors a whole-value Smithy pattern
//!   is wrapped in are how the model turns a find into a total match, so the expression a
//!   refusal advertises is the model's text less that pair; an anchor that belongs to one
//!   branch of an alternation is part of the expression and stays. The member this suite
//!   drives it through is `UpdateConfigurationDefinitionInput$TypeVersion`, whose declared
//!   `^\d{1,3}(\.\d{1,3})?$|^LATEST$` is the only pattern of this service carrying an
//!   anchor inside itself (`smithy_query(query="operation",
//!   operation="UpdateConfigurationDefinition")`), and the rows span values outside it in
//!   every way its two branches can be missed — while the values the pattern DOES admit
//!   are served, so the check still runs the model's text as written.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The `@pattern` the model declares for `UpdateConfigurationDefinitionInput$TypeVersion`,
/// copied from `smithy_query(query="operation",
/// operation="UpdateConfigurationDefinition")`.
const TYPE_VERSION_PATTERN: &str = r"^\d{1,3}(\.\d{1,3})?$|^LATEST$";

/// The expression a refusal of that member advertises: [`TYPE_VERSION_PATTERN`] with the
/// leading `^` and the trailing `$` dropped and the two anchors inside it kept.
const TYPE_VERSION_STATED: &str = r"\d{1,3}(\.\d{1,3})?$|^LATEST";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// Status and parsed body of one call, with the wire error code folded in under `__type`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://ssm-quicksetup.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup\
                 /aws4_request, SignedHeaders=host, Signature=0000"
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
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carried, under either casing an error structure's single message
/// member may render as.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no message>")
        .to_string()
}

/// One definition of the published type at `index` of the catalog, deployed through an
/// organizational unit so it needs no local-deployment role: no case here is about the
/// definition's own content.
fn sound_definition(index: usize) -> Value {
    let catalog = service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG;
    let type_ = catalog[index % catalog.len()].type_;
    json!({
        "Type": type_,
        "Parameters": super::type_parameter_fixtures::with_required_parameters(
            type_,
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" }),
        ),
    })
}

/// RULE: a create whose `ConfigurationDefinitions` names no definition is refused with the
/// fixed sentence AWS states for that error class, whatever else the request carries; one
/// that names at least one definition is served.
#[tokio::test]
async fn an_empty_definition_list_is_refused_with_aws_fixed_sentence() {
    let router = emulator();
    // The sentence as AWS states it — wholly fixed text, naming no member, value or
    // resource, which is why every refused row below expects the identical string.
    let expected = "At least 1 configuration definition must be provided.";

    // (case, the create's body) — every row carries an EMPTY list and differs in
    // everything else a create may say.
    let refused: Vec<(&str, Value)> = vec![
        (
            "nothing but the empty list",
            json!({ "ConfigurationDefinitions": [] }),
        ),
        (
            "a named manager with an empty list",
            json!({ "Name": "empty-one", "ConfigurationDefinitions": [] }),
        ),
        (
            "a described and tagged manager with an empty list",
            json!({
                "Name": "empty-two",
                "Description": "a manager with nothing to deploy",
                "Tags": { "Owner": "qa" },
                "ConfigurationDefinitions": [],
            }),
        ),
    ];
    for (case, body) in &refused {
        let (status, answer) = call(&router, "POST", "/configurationManager", Some(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} / {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {answer:?}"
        );
        let message = error_message(&answer);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A refused create stores nothing: the listing still reports no manager.
    let (status, answer) = call(
        &router,
        "POST",
        "/listConfigurationManagers",
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the listing after the refusals\": expected it served, got {status} / {answer:?}"
    );
    let held = answer
        .get("ConfigurationManagersList")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        held, 0,
        "case \"the listing after the refusals\": expected no manager stored, got {answer:?}"
    );

    // (case, how many definitions the list names)
    for (case, count) in [("one definition", 1), ("two definitions", 2)] {
        let definitions: Vec<Value> = (0..count).map(sound_definition).collect();
        let (status, answer) = call(
            &router,
            "POST",
            "/configurationManager",
            Some(&json!({
                "Name": format!("served-{count}"),
                "ConfigurationDefinitions": definitions,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create served, got {status} / {answer:?}"
        );
        assert!(
            answer
                .get("ManagerArn")
                .and_then(Value::as_str)
                .is_some_and(|arn| arn.contains(":configuration-manager/")),
            "case {case:?}: expected a minted ManagerArn, got {answer:?}"
        );
    }
}

/// Create one manager and answer its ARN beside the id of its one configuration
/// definition, read back through `GetConfigurationManager`.
async fn manager_with_definition(router: &MultiServiceRouter) -> (String, String) {
    let (status, answer) = call(
        router,
        "POST",
        "/configurationManager",
        Some(&json!({
            "Name": "type-version-rows",
            "ConfigurationDefinitions": [sound_definition(0)],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the create must be served, got {status} / {answer:?}"
    );
    let manager_arn = answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: the create answers a ManagerArn, got {answer:?}"))
        .to_string();
    let label = manager_arn.replace('/', "%2F");
    let (status, answer) =
        call(router, "GET", &format!("/configurationManager/{label}"), None).await;
    assert_eq!(
        status, 200,
        "setup: the read-back must be served, got {status} / {answer:?}"
    );
    let id = answer
        .get("ConfigurationDefinitions")
        .and_then(Value::as_array)
        .and_then(|definitions| definitions.first())
        .and_then(|definition| definition.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: the manager reports its definition id, got {answer:?}"))
        .to_string();
    (manager_arn, id)
}

/// RULE: the `@pattern` requirement a refusal states is the model's expression with its
/// OUTER anchor pair dropped and every anchor inside it kept — so a value outside
/// `TypeVersion`'s alternation is told the expression that alternation really is, and a
/// value the alternation admits is served.
#[tokio::test]
async fn a_pattern_refusal_keeps_the_anchors_inside_the_expression() {
    let router = emulator();
    let (manager_arn, definition_id) = manager_with_definition(&router).await;
    let route = format!(
        "/configurationDefinition/{}/{definition_id}",
        manager_arn.replace('/', "%2F")
    );

    // (case, the value sent) — each one is outside a different part of the declared
    // alternation.
    let refused: &[(&str, &str)] = &[
        ("text that is no version at all", "not-a-version"),
        ("a three-part version", "1.2.3"),
        ("the alias in another case", "latest"),
        ("the alias with trailing space", "LATEST "),
        ("more digits than the class admits", "1234"),
        ("a version with a trailing dot", "1."),
    ];
    for (case, value) in refused {
        let (status, answer) = call(
            &router,
            "PUT",
            &route,
            Some(&json!({ "TypeVersion": value })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} / {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {answer:?}"
        );
        let expected = format!(
            "1 validation error detected: Value '{value}' at 'typeVersion' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {TYPE_VERSION_STATED}"
        );
        let message = error_message(&answer);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(TYPE_VERSION_PATTERN),
            "case {case:?}: the model's anchored text must not be quoted, got {message:?}"
        );
    }

    // (case, a value the declared expression admits) — the check still runs the model's
    // text as written, interior anchors included.
    let served: &[(&str, &str)] = &[
        ("the alias", "LATEST"),
        ("a major.minor version", "1.0"),
        ("a bare major version", "1"),
    ];
    for (case, value) in served {
        let (status, answer) = call(
            &router,
            "PUT",
            &route,
            Some(&json!({ "TypeVersion": value })),
        )
        .await;
        assert!(
            status == 200 || status == 400 && error_code(&answer) == "ValidationException",
            "case {case:?}: expected the value to pass the modelled pattern, got {status} / \
             {answer:?}"
        );
        if status == 400 {
            // A value the pattern admits may still be refused by the service's own rule
            // about which versions its catalog publishes — which is a different sentence,
            // and never the pattern clause.
            let message = error_message(&answer);
            assert!(
                !message.contains("regular expression pattern"),
                "case {case:?}: expected no pattern clause for an admitted value, got \
                 {message:?}"
            );
        }
    }
}
