// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetPipelineBlueprint`: retrieving ONE blueprint of the catalog
//! the service publishes, in the format the request asks for.
//!
//! Every case drives the service the way a client does — one HTTP request per call,
//! through the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, asserted over the WHOLE published catalog rather than one
//! blueprint, and over every value the format member admits:
//!
//! 1. A retrieval answers the record the catalog holds: the five members
//!    `PipelineBlueprintSummary` models come back equal to what `ListPipelineBlueprints`
//!    reports for the same name, plus the configuration document only
//!    `PipelineBlueprint` carries.
//! 2. `Format` selects the rendering of that one document and is echoed back: an absent
//!    format renders the stored YAML, `YAML` renders the same, `JSON` renders the same
//!    document as JSON — so the two formats parse to the SAME configuration, which is the
//!    round-trip this operation has to keep.
//! 3. A name the catalog does not publish is refused 404 `ResourceNotFoundException`
//!    naming it; the lookup is exact, so a name differing only in case is not published.
//! 4. A format value outside the set `com.amazonaws.osis#BlueprintFormat` admits
//!    (`@pattern("^(YAML|JSON)$")`) is refused 400 `ValidationException`.
//! 5. A retrieval is a report: it changes nothing a later caller sees, in either
//!    operation.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A Region that is NOT the emulator default, so nothing here can be passing because it
/// was pinned to the default one.
const REGION: &str = "eu-west-2";

/// Every member `PipelineBlueprintSummary` models, which `PipelineBlueprint` carries too
/// (smithy_query: shape PipelineBlueprintSummary,PipelineBlueprint).
const SUMMARY_MEMBERS: [&str; 5] = [
    "BlueprintName",
    "DisplayName",
    "DisplayDescription",
    "Service",
    "UseCase",
];

/// Every member `PipelineBlueprint` models: the summary's five plus the configuration
/// document (smithy_query: shape PipelineBlueprint).
const BLUEPRINT_MEMBERS: [&str; 6] = [
    "BlueprintName",
    "PipelineConfigurationBody",
    "DisplayName",
    "DisplayDescription",
    "Service",
    "UseCase",
];

/// Every value `com.amazonaws.osis#BlueprintFormat` admits, from its
/// `@pattern("^(YAML|JSON)$")`.
const ADMITTED_FORMATS: [&str; 2] = ["YAML", "JSON"];

/// The pattern a refusal of a `Format` value quotes: the model's `@pattern("^(YAML|JSON)$")`
/// in the unanchored find-form this service publishes its patterns in (the rule
/// `tests/all/pattern_messages.rs` states for every patterned shape the service declares).
/// Written out here, so the message is compared against the model rather than against the
/// code that renders it.
const PUBLISHED_FORMAT_PATTERN: &str = "(YAML|JSON).*";

/// The format the service renders when a request names none: the blueprint's configuration
/// document is a YAML document (`PipelineBlueprint$PipelineConfigurationBody`: "The YAML
/// configuration of the blueprint").
const DEFAULT_FORMAT: &str = "YAML";

/// One published name, so a catalog that silently lost every entry cannot make the
/// table-driven cases pass by iterating nothing.
const A_PUBLISHED_BLUEPRINT: &str = "AWS-VPCFlowLogPipeline";

/// What one call answered: the status, the error type header and the parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error message the response carries, under either casing a protocol may render
    /// the `message` member with.
    fn message(&self) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("expected an error message in {:?}", self.body)
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://osis.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::new())
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// `ListPipelineBlueprints`, which is where a client discovers the names this operation
/// takes.
async fn list_blueprints(router: &MultiServiceRouter) -> Vec<Value> {
    let answer = call(router, "POST", "/2022-01-01/osis/listPipelineBlueprints").await;
    assert_eq!(
        answer.status, 200,
        "expected 200 from ListPipelineBlueprints, got {} / {:?}",
        answer.status, answer.body
    );
    answer.body["Blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a `Blueprints` array, got {:?}", answer.body))
        .clone()
}

/// `GetPipelineBlueprint` for `name`, with `format` as its query parameter when given.
async fn get_blueprint(router: &MultiServiceRouter, name: &str, format: Option<&str>) -> Answer {
    let query = match format {
        Some(value) => format!("?format={value}"),
        None => String::new(),
    };
    call(
        router,
        "GET",
        &format!("/2022-01-01/osis/getPipelineBlueprint/{name}{query}"),
    )
    .await
}

fn text(value: &Value, member: &str, case: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `{member}` in {value:?}"))
        .to_string()
}

/// The one document `rendered` holds, read with a YAML 1.2 reader.
///
/// YAML 1.2 is a superset of JSON, so the same reader reads either rendering and the two
/// can be compared as the data model they both state.
fn read_yaml(rendered: &str, case: &str) -> yaml_rust2::Yaml {
    let mut documents = yaml_rust2::YamlLoader::load_from_str(rendered)
        .unwrap_or_else(|error| panic!("case {case:?}: expected one document ({error}), got {rendered:?}"));
    assert_eq!(
        documents.len(),
        1,
        "case {case:?}: expected exactly one document, got {rendered:?}"
    );
    documents.pop().expect("one document")
}

/// A YAML mapping key as the member name a JSON object would state for it: JSON names its
/// members with strings, so a non-string key is compared by its text.
fn key_text(key: &yaml_rust2::Yaml) -> String {
    use yaml_rust2::Yaml;
    match key {
        Yaml::String(text) | Yaml::Real(text) => text.clone(),
        Yaml::Integer(number) => number.to_string(),
        Yaml::Boolean(held) => held.to_string(),
        other => format!("{other:?}"),
    }
}

/// `node` with every mapping's entries in key order and every key as its text.
///
/// A JSON object's members are UNORDERED, so two renderings of one configuration state the
/// same thing whatever order each writes its members in; this is what lets the comparison
/// be about the configuration rather than about the layout.
fn canonical(node: &yaml_rust2::Yaml) -> yaml_rust2::Yaml {
    use yaml_rust2::Yaml;
    match node {
        Yaml::Array(items) => Yaml::Array(items.iter().map(canonical).collect()),
        Yaml::Hash(entries) => {
            let mut sorted: Vec<(String, Yaml)> = entries
                .iter()
                .map(|(key, value)| (key_text(key), canonical(value)))
                .collect();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            Yaml::Hash(
                sorted
                    .into_iter()
                    .map(|(key, value)| (Yaml::String(key), value))
                    .collect(),
            )
        }
        scalar => scalar.clone(),
    }
}

/// The names the catalog publishes, with a floor so an empty catalog fails loudly rather
/// than making a table-driven case vacuous.
async fn published_names(router: &MultiServiceRouter) -> Vec<String> {
    let listed = list_blueprints(router).await;
    let names: Vec<String> = listed
        .iter()
        .map(|summary| text(summary, "BlueprintName", "<listing>"))
        .collect();
    assert!(
        names.contains(&A_PUBLISHED_BLUEPRINT.to_string()),
        "the catalog must publish {A_PUBLISHED_BLUEPRINT:?} for these cases to mean \
         anything, got {names:?}"
    );
    names
}

/// Rule 1: for EVERY published blueprint, a retrieval answers that record — the summary
/// members equal the listing's, and the configuration document the summary does not carry
/// is there.
#[tokio::test]
async fn every_published_blueprint_can_be_retrieved_and_matches_the_listing() {
    let router = new_router();
    let listed = list_blueprints(&router).await;
    assert!(
        !listed.is_empty(),
        "the catalog must publish at least one blueprint, got {listed:?}"
    );

    for summary in &listed {
        let case = text(summary, "BlueprintName", "<listing>");
        let answer = get_blueprint(&router, &case, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a published blueprint must answer 200, got {} / {:?}",
            answer.status, answer.body
        );

        let blueprint = answer.body["Blueprint"].clone();
        let members = blueprint
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a `Blueprint` object, got {blueprint:?}"));

        // Every member the shape models is populated, and nothing it does not model.
        for member in BLUEPRINT_MEMBERS {
            let value = text(&blueprint, member, &case);
            assert!(
                !value.is_empty(),
                "case {case:?}: `{member}` must be populated, got {value:?}"
            );
        }
        for member in members.keys() {
            assert!(
                BLUEPRINT_MEMBERS.contains(&member.as_str()),
                "case {case:?}: `{member}` is not a member of PipelineBlueprint, got {blueprint:?}"
            );
        }

        // The two blueprint operations project ONE record, so the summary members cannot
        // disagree between them.
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                text(&blueprint, member, &case),
                text(summary, member, &case),
                "case {case:?}: `{member}` must equal what the listing reports, got {blueprint:?}"
            );
        }
        assert_eq!(
            text(&blueprint, "BlueprintName", &case),
            case,
            "case {case:?}: the retrieved blueprint must be the one the request named, \
             got {blueprint:?}"
        );
    }
}

/// Rule 2: the format member selects the rendering of the one stored document and is
/// echoed back, and both renderings parse to the same configuration.
#[tokio::test]
async fn both_admitted_formats_render_the_same_configuration_and_are_echoed_back() {
    let router = new_router();
    let names = published_names(&router).await;

    for name in &names {
        // The rows: no format at all, and each value the member admits.
        let mut rendered: Vec<(String, String, String)> = Vec::new();
        for requested in [None, Some(ADMITTED_FORMATS[0]), Some(ADMITTED_FORMATS[1])] {
            let case = format!("{name} as {requested:?}");
            let answer = get_blueprint(&router, name, requested).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an admitted format must answer 200, got {} / {:?}",
                answer.status, answer.body
            );

            let reported = text(&answer.body, "Format", &case);
            assert_eq!(
                reported,
                requested.unwrap_or(DEFAULT_FORMAT),
                "case {case:?}: the answer must report the format it is in, got {reported:?}"
            );

            let body = text(&answer.body["Blueprint"], "PipelineConfigurationBody", &case);
            rendered.push((case, reported, body));
        }

        let yaml_default = &rendered[0];
        let yaml = &rendered[1];
        let json = &rendered[2];

        assert_eq!(
            yaml_default.2, yaml.2,
            "case {:?}: an absent format renders what {DEFAULT_FORMAT} renders, got {:?} vs {:?}",
            yaml_default.0, yaml_default.2, yaml.2
        );

        // The round trip: both renderings are the SAME configuration, so a caller can
        // pick either and build the same pipeline from it. YAML 1.2 is a superset of
        // JSON, so one reader reads both forms and the comparison needs no second
        // rendering of its own.
        let as_json = canonical(&read_yaml(&json.2, &json.0));
        let as_yaml = canonical(&read_yaml(&yaml.2, &yaml.0));
        assert_eq!(
            as_json, as_yaml,
            "case {:?}: both formats must describe one configuration, got {as_json:?} vs {as_yaml:?}",
            json.0
        );
        assert!(
            as_json.as_hash().is_some(),
            "case {:?}: a Data Prepper configuration is a mapping, got {as_json:?}",
            json.0
        );
        // The JSON rendering is a JSON document, which is what asking for that format
        // means and what a handler that ignored the format could not answer.
        let json_rendering = serde_json::from_str::<Value>(&json.2);
        assert!(
            json_rendering.is_ok(),
            "case {:?}: the JSON rendering must be a JSON document, got {:?}",
            json.0,
            json.2
        );
        // The YAML rendering is the stored document itself, so it is the same text as the
        // JSON one only where the stored document happens to be written in the flow style
        // that is also JSON — never because the format was ignored.
        assert!(
            json.2 != yaml.2 || serde_json::from_str::<Value>(&yaml.2).is_ok(),
            "case {:?}: the two renderings are one text only for a stored document that is \
             itself JSON, got {:?}",
            json.0,
            yaml.2
        );

        // The YAML rendering is the document as the record holds it, not JSON under
        // another name.
        assert!(
            serde_json::from_str::<Value>(&yaml.2).is_err(),
            "case {:?}: the {DEFAULT_FORMAT} rendering is not a JSON document, got {:?}",
            yaml.0,
            yaml.2
        );
    }
}

/// Rule 3: a name the catalog does not publish is refused 404, naming it; the lookup is
/// exact.
#[tokio::test]
async fn a_name_the_catalog_does_not_publish_is_refused() {
    let router = new_router();
    let names = published_names(&router).await;

    // The rows differ in HOW the name misses: nothing like a published name, a published
    // name in the wrong case, one with a suffix, and one with a prefix.
    let published = A_PUBLISHED_BLUEPRINT;
    let cases = vec![
        ("no resemblance to a published name", "NotABlueprint".to_string()),
        ("a published name lower-cased", published.to_lowercase()),
        ("a published name upper-cased", published.to_uppercase()),
        ("a published name with a suffix", format!("{published}2")),
        ("a published name with a prefix", format!("Copy-Of-{published}")),
        ("the service prefix alone", "AWS-".to_string()),
    ];

    for (case, name) in cases {
        assert!(
            !names.contains(&name),
            "case {case:?}: {name:?} must not be published for this row to test a miss, \
             got {names:?}"
        );
        let answer = get_blueprint(&router, &name, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unpublished name must answer 404, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_type
        );
        let message = answer.message();
        assert_eq!(
            message,
            format!("The blueprint {name} could not be found"),
            "case {case:?}: the refusal must name the blueprint the request named, \
             got {message:?}"
        );
    }
}

/// Rule 4: a format outside the admitted set is refused 400, whatever shape the offending
/// value takes.
#[tokio::test]
async fn a_format_outside_the_admitted_set_is_refused() {
    let router = new_router();

    // The rows span the ways a value can miss the admitted set: wrong case, an admitted
    // value inside a longer one, another serialization format, and the empty string.
    let cases = [
        ("an admitted value lower-cased", "yaml"),
        ("an admitted value mixed-case", "Json"),
        ("an admitted value with a suffix", "JSONL"),
        ("an admitted value with a prefix", "NOT-YAML"),
        ("a format the service does not serve", "XML"),
        ("the empty string", ""),
    ];

    for (case, requested) in cases {
        assert!(
            !ADMITTED_FORMATS.contains(&requested),
            "case {case:?}: {requested:?} must be outside the admitted set for this row \
             to test a refusal"
        );
        let answer = get_blueprint(&router, A_PUBLISHED_BLUEPRINT, Some(requested)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unadmitted format must answer 400, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_type
        );
        let message = answer.message();
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value '{requested}' at 'format' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {PUBLISHED_FORMAT_PATTERN}"
            ),
            "case {case:?}: the refusal must name the member and quote the published \
             pattern, got {message:?}"
        );
    }
}

/// Rule 5: a retrieval is a report — repeating it answers the same bytes, and neither
/// blueprint operation sees anything a retrieval changed.
#[tokio::test]
async fn a_retrieval_changes_nothing_a_later_caller_sees() {
    let router = new_router();
    let before = list_blueprints(&router).await;
    let names = published_names(&router).await;

    for name in &names {
        for requested in [None, Some(ADMITTED_FORMATS[0]), Some(ADMITTED_FORMATS[1])] {
            let case = format!("{name} as {requested:?}");
            let first = get_blueprint(&router, name, requested).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: expected 200, got {} / {:?}",
                first.status, first.body
            );
            // Three calls, so a handler that consumed or rewrote the record on the first
            // read fails here.
            for repeat in 1..=3 {
                let again = get_blueprint(&router, name, requested).await;
                assert_eq!(
                    (again.status, &again.body),
                    (first.status, &first.body),
                    "case {case:?}: repeat {repeat} must answer what the first call did, \
                     got {} / {:?}",
                    again.status,
                    again.body
                );
            }
        }
    }

    let after = list_blueprints(&router).await;
    assert_eq!(
        after, before,
        "the listing must be unchanged by every retrieval, got {after:?} vs {before:?}"
    );

    // A second, independent service instance answers the same records: they are seeded
    // from the service's own data, not accumulated by use.
    let fresh = new_router();
    for name in &names {
        let here = get_blueprint(&router, name, Some("JSON")).await;
        let there = get_blueprint(&fresh, name, Some("JSON")).await;
        assert_eq!(
            (there.status, &there.body),
            (here.status, &here.body),
            "case {name:?}: a fresh store must answer the same record, got {:?} vs {:?}",
            there.body,
            here.body
        );
    }
}
