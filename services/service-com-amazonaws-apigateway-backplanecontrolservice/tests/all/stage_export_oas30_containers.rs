// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONTAINERS an exported OpenAPI 3.0 document carries, in every
//! serialization the export is written in: the document's top-level Components Object, and
//! each Response Object's `content` map.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the raw `GetExport` call, the YAML
//! reading of an export and the `CreateRestApi`/`CreateResource`/`PutMethod`/
//! `PutMethodResponse`/`CreateDeployment` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `execute_api_data_plane`, `method_response`, `stage_export`
//! and `stage_export_content_types`.
//!
//! The rule asserted here, over a SET of inputs rather than one recorded export: a 3.0
//! document always carries the container a reader of the definition looks in — the one
//! Components Object the document's reusable definitions live in, and the `content` map a
//! Response Object states its representations in — whether or not the definition has
//! anything to put in it, in whichever serialization `accepts` selected, and what fills it
//! comes from the stored records. The sibling `stage_export_version_and_containers` suite
//! asserts the same rule on the JSON serialization and the 2.0 dialect's lack of either
//! container; what these rows add is the SERIALIZATION axis — an empty mapping is a value
//! of the document, so the YAML spelling of the export carries it too — and the rows that
//! FILL the Components Object, so a document that carries the key cannot be one that
//! hard-codes it empty.
//!
//! The rows differ along what the API declares that could fill the two containers: how
//! many method responses a method has (none, one, several), and which authorization it
//! carries (none, an API key, IAM and an API key — each naming a security scheme the
//! Components Object holds). The serializations are both values `accepts` takes plus the
//! request that sends no `Accept` header at all.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{deploy, put_integration, put_method};
use crate::method_response::put_method_response;
use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};
use crate::stage_export::export;
use crate::stage_export_content_types::yaml_as_json;

/// The stage every case exports.
const STAGE: &str = "dev";

/// The resource path every case puts its method on, and the verb it puts there.
const PATH: &str = "/widgets";
const VERB: &str = "GET";

/// The Path Item key [`VERB`] is exported under.
const OPERATION_KEY: &str = "get";

/// The serializations `accepts` selects, from `GetExportRequest$accepts`: "Currently
/// application/json and application/yaml are supported" — plus the request that asks for
/// neither, which is served in the default one.
const SERIALIZATIONS: &[(&str, Option<&str>)] = &[
    ("no Accept header", None),
    ("application/json", Some("application/json")),
    ("application/yaml", Some("application/yaml")),
];

/// One row of the table: the case, the `PutMethod` body, the status codes the method
/// declares a response for, and the security schemes the exported document's Components
/// Object must hold (sorted; empty for a row whose method needs no authorization, where
/// the Components Object is the empty mapping).
type ContainerCase<'a> = (&'a str, Value, &'a [&'a str], &'a [&'a str]);

/// Stand an API up with one [`VERB`] method on [`PATH`] configured by `method`, declaring a
/// method response for each of `responses`, deployed to [`STAGE`]. Answers the API id.
async fn deployed_api(
    gateway: &Gateway,
    case: &str,
    method: Value,
    responses: &[&str],
) -> String {
    let (api, root) = create_api(gateway, &format!("oas30-containers-{case}")).await;
    let resource = resource_under(
        gateway,
        &api,
        &root,
        PATH.trim_start_matches('/'),
    )
    .await;
    put_method(gateway, &api, &resource, VERB, method).await;
    // The integration a deployment requires of every method: `MOCK`, the type that needs
    // no backend.
    put_integration(gateway, &api, &resource, VERB, json!({ "type": "MOCK" })).await;
    for status in responses {
        let answer =
            put_method_response(gateway, &api, &resource, VERB, status, json!({})).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201 from PutMethodResponse({status}), got {} with {:?}",
            answer.status, answer.body
        );
    }
    deploy(gateway, &api, STAGE).await;
    api
}

/// The keys of a JSON object, SORTED — a JSON object is unordered, so a key SET is what a
/// case may assert about one.
fn sorted_keys(object: &Value, case: &str) -> Vec<String> {
    let mut keys: Vec<String> = object
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected an object, got {object}"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The export body as the document it spells, in whichever serialization it was written
/// in: the YAML reading goes through the sibling suite's one conversion, so a YAML export
/// and a JSON export of one stage are compared as the same document.
fn document(body: &[u8], accepts: Option<&str>, case: &str) -> Value {
    let text = String::from_utf8(body.to_vec())
        .unwrap_or_else(|error| panic!("case {case}: expected a UTF-8 export, got {error}"));
    if accepts == Some("application/yaml") {
        let documents = ::yaml_rust2::yaml::YamlLoader::load_from_str(&text).unwrap_or_else(
            |error| panic!("case {case}: expected a YAML export, got {error} for {text}"),
        );
        let parsed = documents
            .first()
            .unwrap_or_else(|| panic!("case {case}: expected one YAML document, got {text}"));
        return yaml_as_json(parsed);
    }
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {case}: expected a JSON export, got {error} for {text}"))
}

#[tokio::test]
async fn an_oas30_export_carries_its_containers_in_every_serialization() {
    let cases: Vec<ContainerCase<'_>> = vec![
        (
            "no response and no authorization",
            json!({ "authorizationType": "NONE" }),
            &[],
            &[],
        ),
        (
            "one response and no authorization",
            json!({ "authorizationType": "NONE" }),
            &["200"],
            &[],
        ),
        (
            "several responses and no authorization",
            json!({ "authorizationType": "NONE" }),
            &["200", "404", "500"],
            &[],
        ),
        (
            "one response behind an api key",
            json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
            &["200"],
            &["api_key"],
        ),
        (
            "several responses behind iam and an api key",
            json!({ "authorizationType": "AWS_IAM", "apiKeyRequired": true }),
            &["201", "404"],
            &["api_key", "sigv4"],
        ),
    ];

    for (case, method, responses, schemes) in cases {
        let gateway = Gateway::new();
        let api = deployed_api(&gateway, case, method, responses).await;

        let mut spelled: Vec<(&str, Value)> = Vec::new();
        for (serialization, accepts) in SERIALIZATIONS {
            let case = format!("{case} as {serialization}");
            let answer = export(&gateway, &api, STAGE, "oas30", *accepts, &[]).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 from GetExport, got {} with {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let document = document(&answer.body, *accepts, &case);

            // The Components Object: carried whatever the definition has to put in it, and
            // holding exactly the schemes the stored methods name.
            let components = &document["components"];
            assert!(
                components.is_object(),
                "case {case}: a 3.0 document carries a components object, got {document}"
            );
            if schemes.is_empty() {
                assert_eq!(
                    *components,
                    json!({}),
                    "case {case}: an API with nothing reusable carries the empty components \
                     object, got {components}"
                );
            } else {
                let named = sorted_keys(&components["securitySchemes"], &case);
                assert_eq!(
                    named, *schemes,
                    "case {case}: expected the security schemes {schemes:?}, got {named:?} in \
                     {components}"
                );
            }

            // Every Response Object states the representations it is returned in, and this
            // export references no payload schema, so each one is the empty mapping.
            let operation = &document["paths"][PATH][OPERATION_KEY];
            for status in responses {
                let response = &operation["responses"][*status];
                assert_eq!(
                    response["content"],
                    json!({}),
                    "case {case}: the {status} response carries an empty content map, got \
                     {response}"
                );
            }

            // The byte spelling of the YAML serialization: an empty mapping is a value of
            // the document, written in YAML's flow style on its key's own line, so a
            // consumer byte-comparing or re-importing the file reads the same containers
            // the JSON spelling carries.
            if *accepts == Some("application/yaml") {
                let text = String::from_utf8_lossy(&answer.body).to_string();
                let expected = if schemes.is_empty() {
                    "components: {}"
                } else {
                    "components:"
                };
                assert!(
                    text.lines().any(|line| line == expected),
                    "case {case}: expected the line {expected:?} at the document's own \
                     indentation, got {text}"
                );
                let contents = text
                    .lines()
                    .filter(|line| line.trim_start() == "content: {}")
                    .count();
                assert_eq!(
                    contents,
                    responses.len(),
                    "case {case}: expected one empty content mapping per method response \
                     ({}), got {contents} in {text}",
                    responses.len()
                );
            }

            spelled.push((serialization, document));
        }

        // Every serialization spells the SAME document, so neither container is a product
        // of one writer.
        let (first_name, first) = &spelled[0];
        for (serialization, other) in &spelled[1..] {
            assert_eq!(
                other, first,
                "case {case}: the {serialization} export spells the {first_name} document, \
                 got {other} and {first}"
            );
        }
    }
}
