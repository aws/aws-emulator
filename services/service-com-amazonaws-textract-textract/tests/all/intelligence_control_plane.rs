// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Amazon Textract's document-intelligence CONTROL PLANE over the
//! wire (`awsJson1_1`, `POST /` with `X-Amz-Target: Textract.<Operation>`): the projects a
//! document insights job runs under, and the blueprints that say which fields to extract.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * `CreateProject` and `CreateBlueprint` persist what the request describes under a
//!   minted ARN, and `GetProject` / `GetBlueprint` report exactly that stored state back;
//! * `UpdateProject` replaces every configuration the request supplies and leaves every
//!   one it omits alone — with `KmsEncryptionContext` the one member AWS exempts from
//!   whole replacement;
//! * a blueprint version is immutable: it keeps the schema it was cut from however often
//!   the blueprint is updated afterwards, and deleting one version leaves the blueprint and
//!   its other versions;
//! * one `ClientRequestToken` names one resource — a replay answers with what the first
//!   call created rather than creating a second;
//! * a project and a blueprint name are unique per account, so a second create under one
//!   is refused with the modelled `ConflictException`;
//! * a listing is cut from the stored records in one stable order, so paging through it
//!   reports every resource exactly once, and each filter narrows it to the resources it
//!   names;
//! * an ARN that names no resource here is refused with the modelled
//!   `ResourceNotFoundException`, and the configuration rules AWS states in prose are
//!   refused with the modelled `ValidationException`;
//! * a read is a read: repeating it, and creating other resources, never changes what it
//!   answers.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_textract_textract::state::{insights, intelligence};
use service_com_amazonaws_textract_textract::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is served in: textract serves a request carrying no
/// credential scope in its default Region.
const REGION: &str = "us-east-1";

/// The emulator, with this one service mounted on a shared engine.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Textract.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn string_member(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `base` with every member of `changes` applied; a `null` in `changes` DROPS the member.
fn merged(base: &Value, changes: &Value) -> Value {
    let mut out: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    for (key, value) in changes.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            out.remove(&key);
        } else {
            out.insert(key, value);
        }
    }
    Value::Object(out)
}

/// An `ExtractionConfiguration` naming the granularity levels and text formats given.
///
/// `ExtractionConfiguration` is required on `CreateProject` and its three members are each
/// required (`smithy_query(query="shape", shape="ExtractionConfiguration")`), so every
/// project in these tables carries one.
fn extraction(bounding_box: &str, levels: &[&str], formats: &[&str]) -> Value {
    json!({
        "BoundingBox": { "State": bounding_box },
        "Granularity": { "Types": levels },
        "OutputFormat": {
            "AdditionalFileFormat": { "State": "DISABLED" },
            "TextFormat": { "Types": formats },
        },
    })
}

/// A blueprint schema naming `fields`: a JSON object whose `properties` are the fields to
/// extract, which is the form `CreateBlueprint` accepts.
fn schema(fields: &[&str]) -> String {
    let properties: Map<String, Value> = fields
        .iter()
        .map(|field| ((*field).to_string(), json!({ "type": "string" })))
        .collect();
    json!({ "class": "document", "properties": Value::Object(properties) }).to_string()
}

/// `CreateBlueprint`, asserting only that it succeeded, and the `BlueprintArn` it minted.
async fn create_blueprint(router: &MultiServiceRouter, name: &str, fields: &[&str]) -> String {
    let (status, body) = call(
        router,
        "CreateBlueprint",
        &json!({ "BlueprintName": name, "Schema": schema(fields) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected HTTP 200 from CreateBlueprint, got {status} with {body}"
    );
    let arn = string_member(&body, "BlueprintArn");
    assert!(
        arn.starts_with(&format!(
            "arn:aws:textract:{REGION}:{}:blueprint/",
            engine::emulator_account_id()
        )),
        "case {name:?}: a BlueprintArn must be this account's, got {arn:?}"
    );
    arn
}

/// `CreateProject`, asserting only that it succeeded, and the `ProjectArn` it minted.
async fn create_project(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, body) = call(router, "CreateProject", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateProject, got {status} with {body}"
    );
    assert_eq!(
        string_member(&body, "Status"),
        "COMPLETED",
        "case {case:?}: a stored project's creation completed, got {body}"
    );
    let arn = string_member(&body, "ProjectArn");
    assert!(
        arn.starts_with(&format!(
            "arn:aws:textract:{REGION}:{}:project/",
            engine::emulator_account_id()
        )),
        "case {case:?}: a ProjectArn must be this account's, got {arn:?}"
    );
    arn
}

/// The minimal `CreateProject` body for `name`.
fn project_request(name: &str) -> Value {
    json!({
        "ProjectName": name,
        "ExtractionConfiguration": extraction("ENABLED", &["PAGE", "LINE"], &["PLAIN_TEXT"]),
    })
}

/// RULE: a create persists what the request describes, and the matching read reports
/// exactly that stored state back — every configuration member included, and a member the
/// request omitted reported as absent rather than as a default.
#[tokio::test]
async fn create_project_persists_what_get_project_reports_back() {
    let router = harness();
    let blueprint = create_blueprint(&router, "invoice-blueprint", &["invoice_number"]).await;

    // The rows differ along what the rule generalises over: which optional configurations
    // the request carries, the name's character set, how long the description is (the
    // modelled bounds are ProjectName len 1..128 and ProjectDescription len 0..300), and
    // whether a custom configuration associates a blueprint with the project.
    let rows: Vec<(&str, Value)> = vec![
        ("only the required members", project_request("minimal")),
        (
            "name at the modelled minimum length",
            project_request("p"),
        ),
        (
            "name at the modelled maximum length",
            project_request(&"n".repeat(128)),
        ),
        (
            "name using the pattern's - and _ characters",
            project_request("Claims_Project-02"),
        ),
        (
            "an empty description, which the modelled minimum length allows",
            merged(
                &project_request("empty-description"),
                &json!({ "ProjectDescription": "" }),
            ),
        ),
        (
            "a description at the modelled maximum length",
            merged(
                &project_request("long-description"),
                &json!({ "ProjectDescription": "d".repeat(300) }),
            ),
        ),
        (
            "every optional configuration at once",
            merged(
                &project_request("fully-configured"),
                &json!({
                    "ProjectDescription": "Claims intake (2026): rev. 3!",
                    "ExtractionConfiguration": extraction(
                        "DISABLED",
                        &["DOCUMENT", "PAGE", "ELEMENT", "LINE", "WORD"],
                        &["PLAIN_TEXT", "MARKDOWN", "HTML", "CSV"],
                    ),
                    "GenerativeConfiguration": {
                        "Summarization": { "State": "ENABLED" },
                        "FigureCaption": { "State": "DISABLED" },
                    },
                    "CustomConfiguration": {
                        "Blueprints": [{ "BlueprintArn": blueprint.clone() }],
                        "FallbackBlueprints": [{ "BlueprintArn": blueprint.clone() }],
                    },
                    "SplitterConfiguration": { "State": "ENABLED" },
                    "SensitiveDataConfiguration": {
                        "DetectionMode": "DETECTION_AND_REDACTION",
                        "PiiEntitiesConfiguration": {
                            "PiiEntityTypes": ["NAME", "EMAIL"],
                            "RedactionMaskMode": "ENTITY_TYPE",
                        },
                    },
                    "EncryptionConfiguration": {
                        "KmsKeyId": "alias/textract-projects",
                        "KmsEncryptionContext": { "tenant": "claims" },
                    },
                    "Tags": { "team": "claims", "env": "test" },
                }),
            ),
        ),
        (
            "a blueprint pinned to a version the project associates",
            merged(
                &project_request("version-pinned"),
                &json!({
                    "CustomConfiguration": {
                        "Blueprints": [{ "BlueprintArn": blueprint.clone() }],
                    },
                }),
            ),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, request) in &rows {
        let project_arn = create_project(&router, case, request).await;
        assert!(
            minted.insert(project_arn.clone()),
            "case {case:?}: every created project needs its own ProjectArn, {project_arn:?} \
             repeated"
        );

        let (status, body) =
            call(&router, "GetProject", &json!({ "ProjectArn": project_arn.clone() })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetProject of {project_arn:?} must succeed, got {status} with {body}"
        );
        assert_eq!(
            string_member(&body, "ProjectArn"),
            project_arn,
            "case {case:?}: GetProject must report the ARN it was asked for, got {body}"
        );
        assert_eq!(
            string_member(&body, "Status"),
            "COMPLETED",
            "case {case:?}: expected a COMPLETED project, got {body}"
        );
        // Every member the request carried must come back, member by member.
        for member in [
            "ProjectName",
            "ProjectDescription",
            "ExtractionConfiguration",
            "GenerativeConfiguration",
            "CustomConfiguration",
            "SplitterConfiguration",
            "SensitiveDataConfiguration",
            "EncryptionConfiguration",
        ] {
            let sent = request.get(member).cloned();
            let read = body.get(member).cloned();
            assert_eq!(
                read, sent,
                "case {case:?}: expected {member} {sent:?} to read back, got {read:?} from {body}"
            );
        }
        let creation_time = body
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let last_modified = body
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            creation_time > 0.0,
            "case {case:?}: CreationTime must be the instant the project was created, got {body}"
        );
        assert!(
            (last_modified - creation_time).abs() < f64::EPSILON,
            "case {case:?}: an unmodified project was last modified when it was created, got \
             {body}"
        );
    }
}

/// One row of the `UpdateProject` table: what the update supplies, and what the read must
/// report afterwards.
struct UpdateCase {
    name: &'static str,
    /// The members the update carries.
    update: Value,
    /// The members the read must report, member by member, after the update.
    expect: Value,
}

/// RULE: `UpdateProject` replaces every configuration the request supplies, leaves every
/// one it omits unchanged, and — for `KmsEncryptionContext` alone — keeps the stored value
/// when the supplied encryption configuration omits it, clearing it only for an empty map.
#[tokio::test]
async fn update_project_replaces_what_it_supplies_and_keeps_what_it_omits() {
    let router = harness();
    let blueprint = create_blueprint(&router, "update-blueprint", &["total_amount"]).await;
    let created = merged(
        &project_request("under-update"),
        &json!({
            "ProjectDescription": "the first description",
            "SplitterConfiguration": { "State": "ENABLED" },
            "EncryptionConfiguration": {
                "KmsKeyId": "alias/first-key",
                "KmsEncryptionContext": { "tenant": "claims" },
            },
        }),
    );

    // The rows differ along WHICH configuration the update supplies, including none at
    // all, and along the three cases of the encryption-context exception AWS states.
    let rows = [
        UpdateCase {
            name: "an update that supplies nothing leaves every configuration alone",
            update: json!({}),
            expect: json!({
                "ProjectDescription": "the first description",
                "ExtractionConfiguration":
                    extraction("ENABLED", &["PAGE", "LINE"], &["PLAIN_TEXT"]),
                "SplitterConfiguration": { "State": "ENABLED" },
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/first-key",
                    "KmsEncryptionContext": { "tenant": "claims" },
                },
            }),
        },
        UpdateCase {
            name: "a new description replaces the stored one",
            update: json!({ "ProjectDescription": "the second description" }),
            expect: json!({
                "ProjectDescription": "the second description",
                "SplitterConfiguration": { "State": "ENABLED" },
            }),
        },
        UpdateCase {
            name: "a supplied extraction configuration replaces the stored one whole",
            update: json!({
                "ExtractionConfiguration": extraction("DISABLED", &["WORD"], &["MARKDOWN"]),
            }),
            expect: json!({
                "ExtractionConfiguration": extraction("DISABLED", &["WORD"], &["MARKDOWN"]),
                "ProjectDescription": "the first description",
            }),
        },
        UpdateCase {
            name: "a configuration the project did not carry is added",
            update: json!({
                "GenerativeConfiguration": { "Summarization": { "State": "ENABLED" } },
            }),
            expect: json!({
                "GenerativeConfiguration": { "Summarization": { "State": "ENABLED" } },
                "SplitterConfiguration": { "State": "ENABLED" },
            }),
        },
        UpdateCase {
            name: "an encryption configuration omitting the context keeps the stored context",
            update: json!({ "EncryptionConfiguration": { "KmsKeyId": "alias/second-key" } }),
            expect: json!({
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/second-key",
                    "KmsEncryptionContext": { "tenant": "claims" },
                },
            }),
        },
        UpdateCase {
            name: "an empty context clears the stored context",
            update: json!({
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/third-key",
                    "KmsEncryptionContext": {},
                },
            }),
            expect: json!({
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/third-key",
                    "KmsEncryptionContext": {},
                },
            }),
        },
        UpdateCase {
            name: "a new context replaces the stored one",
            update: json!({
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/fourth-key",
                    "KmsEncryptionContext": { "tenant": "billing" },
                },
            }),
            expect: json!({
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/fourth-key",
                    "KmsEncryptionContext": { "tenant": "billing" },
                },
            }),
        },
        UpdateCase {
            name: "a custom configuration naming a blueprint this account holds",
            update: json!({
                "CustomConfiguration": { "Blueprints": [{ "BlueprintArn": blueprint.clone() }] },
            }),
            expect: json!({
                "CustomConfiguration": { "Blueprints": [{ "BlueprintArn": blueprint.clone() }] },
            }),
        },
    ];

    for (index, case) in rows.iter().enumerate() {
        // A fresh project per row, so rows do not interfere.
        let request = merged(
            &created,
            &json!({ "ProjectName": format!("under-update-{index}") }),
        );
        let project_arn = create_project(&router, case.name, &request).await;
        let update = merged(
            &case.update,
            &json!({ "ProjectArn": project_arn.clone() }),
        );
        let (status, body) = call(&router, "UpdateProject", &update).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateProject must succeed, got {status} with {body}",
            case.name
        );
        assert_eq!(
            string_member(&body, "ProjectArn"),
            project_arn,
            "case {:?}: UpdateProject must report the project it changed, got {body}",
            case.name
        );

        let (status, read) =
            call(&router, "GetProject", &json!({ "ProjectArn": project_arn.clone() })).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetProject must succeed, got {status} with {read}",
            case.name
        );
        let expect = &case.expect;
        for (member, wanted) in expect.as_object().cloned().unwrap_or_default() {
            let got = read.get(&member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                got, wanted,
                "case {:?}: expected {member} {wanted} after the update, got {got} from {read}",
                case.name
            );
        }
        let creation_time = read
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let last_modified = read
            .get("LastModifiedTime")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            last_modified >= creation_time,
            "case {:?}: an update does not move CreationTime back, got {read}",
            case.name
        );
    }
}

/// RULE: a blueprint version is immutable — it keeps the schema it was cut from, whatever a
/// later `UpdateBlueprint` does to the blueprint — and the blueprint itself reports its own
/// live schema.
#[tokio::test]
async fn a_blueprint_version_keeps_the_schema_it_was_cut_from() {
    let router = harness();
    let blueprint_arn = create_blueprint(&router, "versioned", &["invoice_number"]).await;

    // Rows differ along the schema each cut was taken from, so the rule is asserted over a
    // sequence of cuts rather than one.
    let cuts: Vec<Vec<&str>> = vec![
        vec!["invoice_number"],
        vec!["invoice_number", "total_amount"],
        vec!["vendor_name"],
    ];
    let mut versions: Vec<(String, String)> = Vec::new();
    for (index, fields) in cuts.iter().enumerate() {
        let case = format!("cut #{index}");
        // The cut snapshots the blueprint as it is NOW, so the schema is set first.
        let expected = schema(fields);
        let (status, body) = call(
            &router,
            "UpdateBlueprint",
            &json!({ "BlueprintArn": blueprint_arn.clone(), "Schema": expected.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateBlueprint must succeed, got {status} with {body}"
        );
        let (status, body) = call(
            &router,
            "CreateBlueprintVersion",
            &json!({ "BlueprintArn": blueprint_arn.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBlueprintVersion must succeed, got {status} with {body}"
        );
        let version = string_member(&body, "BlueprintVersion");
        // `BlueprintVersion`'s modelled pattern is `^[0-9]*$`, and versions start at 1.
        assert_eq!(
            version,
            (index + 1).to_string(),
            "case {case:?}: versions are numbered from 1 upwards, got {body}"
        );
        versions.push((version, expected));
    }

    // AWS, on `GetBlueprintRequest$BlueprintVersion`: "If omitted, the latest version is
    // returned" — the highest-numbered cut, which is the last one taken above.
    let (latest_version, latest_schema) = versions.last().cloned().unwrap_or_default();
    let (status, live) = call(
        &router,
        "GetBlueprint",
        &json!({ "BlueprintArn": blueprint_arn.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetBlueprint with no version must succeed, got {status} with {live}"
    );
    assert_eq!(
        string_member(&live, "BlueprintVersion"),
        latest_version,
        "with no version named, the latest version is returned, got {live}"
    );
    assert_eq!(
        string_member(&live, "Schema"),
        latest_schema,
        "the latest version reports the schema it was cut from, got {live}"
    );

    // Each cut still reports the schema it was taken from, and echoes the version asked
    // for.
    for (version, expected) in &versions {
        let (status, body) = call(
            &router,
            "GetBlueprint",
            &json!({ "BlueprintArn": blueprint_arn.clone(), "BlueprintVersion": version }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {version:?}: GetBlueprint of a version must succeed, got {status} with {body}"
        );
        assert_eq!(
            string_member(&body, "BlueprintVersion"),
            *version,
            "case {version:?}: a version read must echo the version asked for, got {body}"
        );
        assert_eq!(
            &string_member(&body, "Schema"),
            expected,
            "case {version:?}: expected the schema the version was cut from, got {body}"
        );
    }
}

/// RULE: deleting one version of a blueprint removes that version alone; deleting the
/// blueprint with no version named removes it and every version of it.
#[tokio::test]
async fn deleting_a_blueprint_version_leaves_the_blueprint_and_its_other_versions() {
    let router = harness();
    let blueprint_arn = create_blueprint(&router, "three-versions", &["field_one"]).await;
    for _ in 0..3u32 {
        let (status, body) = call(
            &router,
            "CreateBlueprintVersion",
            &json!({ "BlueprintArn": blueprint_arn.clone() }),
        )
        .await;
        assert_eq!(status, 200, "a version must be cut, got {status} with {body}");
    }

    // Rows differ along which of the three versions is removed, and each row asserts the
    // other two still read back.
    let (status, body) = call(
        &router,
        "DeleteBlueprint",
        &json!({ "BlueprintArn": blueprint_arn.clone(), "BlueprintVersion": "2" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting one version must succeed, got {status} with {body}"
    );
    for (version, expect) in [("1", 200), ("2", 400), ("3", 200)] {
        let (status, body) = call(
            &router,
            "GetBlueprint",
            &json!({ "BlueprintArn": blueprint_arn.clone(), "BlueprintVersion": version }),
        )
        .await;
        assert_eq!(
            status, expect,
            "case {version:?}: expected HTTP {expect} after deleting version 2, got {status} \
             with {body}"
        );
        if expect == 400 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {version:?}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
    // The blueprint itself is untouched by a version delete.
    let (status, body) = call(
        &router,
        "GetBlueprint",
        &json!({ "BlueprintArn": blueprint_arn.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a version delete must leave the blueprint, got {status} with {body}"
    );

    // With no version named, the blueprint and all of its versions go.
    let (status, body) = call(
        &router,
        "DeleteBlueprint",
        &json!({ "BlueprintArn": blueprint_arn.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the blueprint must succeed, got {status} with {body}"
    );
    for request in [
        json!({ "BlueprintArn": blueprint_arn.clone() }),
        json!({ "BlueprintArn": blueprint_arn.clone(), "BlueprintVersion": "1" }),
        json!({ "BlueprintArn": blueprint_arn.clone(), "BlueprintVersion": "3" }),
    ] {
        let (status, body) = call(&router, "GetBlueprint", &request).await;
        assert_eq!(
            status, 400,
            "case {request}: a deleted blueprint names nothing, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {request}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// RULE: one `ClientRequestToken` names one resource — a replay answers with what the first
/// call created, whatever else the replayed request says, and a different token creates a
/// second resource.
#[tokio::test]
async fn one_client_request_token_names_one_resource() {
    let router = harness();

    // Rows differ along WHICH create the token was sent to and what the replay changes:
    // the rule is the same for all three `@idempotencyToken` creates of this control plane.
    let project = merged(
        &project_request("tokened-project"),
        &json!({ "ClientRequestToken": "project-token-1" }),
    );
    let first = create_project(&router, "the first tokened create", &project).await;
    for (case, changes) in [
        ("an identical replay", json!({})),
        (
            "a replay naming a different project",
            json!({ "ProjectName": "some-other-name" }),
        ),
        (
            "a replay carrying a description",
            json!({ "ProjectDescription": "added on the replay" }),
        ),
    ] {
        let (status, body) =
            call(&router, "CreateProject", &merged(&project, &changes)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed token must succeed, got {status} with {body}"
        );
        assert_eq!(
            string_member(&body, "ProjectArn"),
            first,
            "case {case:?}: expected the first project {first:?}, got {body}"
        );
    }
    // A different token creates a second project.
    let second = create_project(
        &router,
        "another token",
        &merged(
            &project,
            &json!({ "ProjectName": "second-project", "ClientRequestToken": "project-token-2" }),
        ),
    )
    .await;
    assert_ne!(
        first, second,
        "two creates under different tokens must create two projects, both answered {first:?}"
    );

    // The same rule for a blueprint, and for a blueprint version.
    let blueprint = json!({
        "BlueprintName": "tokened-blueprint",
        "Schema": schema(&["field_one"]),
        "ClientRequestToken": "blueprint-token-1",
    });
    let (status, body) = call(&router, "CreateBlueprint", &blueprint).await;
    assert_eq!(status, 200, "the first blueprint create must succeed, got {body}");
    let blueprint_arn = string_member(&body, "BlueprintArn");
    let (status, replayed) = call(
        &router,
        "CreateBlueprint",
        &merged(&blueprint, &json!({ "BlueprintName": "a-different-name" })),
    )
    .await;
    assert_eq!(status, 200, "a replayed blueprint token must succeed, got {replayed}");
    assert_eq!(
        string_member(&replayed, "BlueprintArn"),
        blueprint_arn,
        "a replayed blueprint token must answer with the first blueprint, got {replayed}"
    );

    let version = json!({
        "BlueprintArn": blueprint_arn.clone(),
        "ClientRequestToken": "version-token-1",
    });
    let (status, body) = call(&router, "CreateBlueprintVersion", &version).await;
    assert_eq!(status, 200, "the first version cut must succeed, got {body}");
    let cut = string_member(&body, "BlueprintVersion");
    let (status, replayed) = call(&router, "CreateBlueprintVersion", &version).await;
    assert_eq!(status, 200, "a replayed version token must succeed, got {replayed}");
    assert_eq!(
        string_member(&replayed, "BlueprintVersion"),
        cut,
        "a replayed version token must answer with the version it first cut, got {replayed}"
    );
}

/// RULE: a project name and a blueprint name are unique per account, so a second create
/// under a name already in use is refused with the modelled `ConflictException` and creates
/// nothing.
#[tokio::test]
async fn a_name_already_in_use_is_refused() {
    let router = harness();

    // Rows differ along which resource kind the name belongs to, and along the name's own
    // character set.
    let long_name = "n".repeat(128);
    for name in ["taken", "Taken_Name-01", long_name.as_str()] {
        let first = create_project(&router, name, &project_request(name)).await;
        let (status, body) = call(&router, "CreateProject", &project_request(name)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a second project under one name must be refused, got {status} with \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {body}"
        );
        assert!(
            body.get("ProjectArn").is_none(),
            "case {name:?}: a refused create must mint no project, got {body}"
        );
        // The project that holds the name is not disclosed to the refused call.
        assert!(
            !body.to_string().contains(first.as_str()),
            "case {name:?}: a refusal must not name the project already holding it, got {body}"
        );
    }
    for name in ["blueprint-taken", "Blueprint_Name-01"] {
        create_blueprint(&router, name, &["field_one"]).await;
        let (status, body) = call(
            &router,
            "CreateBlueprint",
            &json!({ "BlueprintName": name, "Schema": schema(&["field_two"]) }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: a second blueprint under one name must be refused, got {status} with \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {body}"
        );
    }
}

/// RULE: an ARN that names no resource this account holds is refused with the modelled
/// `ResourceNotFoundException`, whichever operation resolved it — one never created, and
/// one already deleted.
#[tokio::test]
async fn an_arn_that_names_nothing_is_refused() {
    let router = harness();
    let live_project = create_project(&router, "the one real project", &project_request("real"))
        .await;
    let live_blueprint = create_blueprint(&router, "real-blueprint", &["field_one"]).await;
    let deleted_project = create_project(
        &router,
        "a project that gets deleted",
        &project_request("deleted"),
    )
    .await;
    let (status, _) = call(
        &router,
        "DeleteProject",
        &json!({ "ProjectArn": deleted_project.clone() }),
    )
    .await;
    assert_eq!(status, 200, "the project under test must be deleted first");

    // Rows span the identifier lengths `ProjectArn`'s modelled pattern allows
    // (`project/[a-zA-Z0-9-]{12,36}`), an id that differs from a real one by one
    // character, and a resource that really existed and was deleted.
    let account = engine::emulator_account_id();
    let mut flipped = live_project.clone();
    flipped.pop();
    flipped.push(if live_project.ends_with('0') { '1' } else { '0' });
    let project_rows: Vec<(String, String)> = vec![
        (
            "an identifier at the modelled minimum length".to_string(),
            format!("arn:aws:textract:{REGION}:{account}:project/{}", "a".repeat(12)),
        ),
        (
            "an identifier at the modelled maximum length".to_string(),
            format!("arn:aws:textract:{REGION}:{account}:project/{}", "b".repeat(36)),
        ),
        (
            "a real identifier with its last character changed".to_string(),
            flipped,
        ),
        (
            "a project that was deleted".to_string(),
            deleted_project.clone(),
        ),
    ];
    for (case, project_arn) in &project_rows {
        assert_ne!(
            project_arn, &live_project,
            "case {case:?}: the table must not name the project that exists"
        );
        for operation in ["GetProject", "DeleteProject"] {
            let (status, body) =
                call(&router, operation, &json!({ "ProjectArn": project_arn })).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {body}"
            );
        }
        let (status, body) = call(
            &router,
            "UpdateProject",
            &json!({ "ProjectArn": project_arn, "ProjectDescription": "x" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} on UpdateProject: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} on UpdateProject: expected ResourceNotFoundException, got {body}"
        );
    }

    // The same rule for a blueprint, over the identifier forms its own ARN pattern allows.
    let blueprint_rows: Vec<(String, String)> = vec![
        (
            "an identifier at the modelled minimum length".to_string(),
            format!(
                "arn:aws:textract:{REGION}:{account}:blueprint/{}",
                "c".repeat(12)
            ),
        ),
        (
            "a service blueprint identifier this emulator mints none of".to_string(),
            format!("arn:aws:textract:{REGION}:{account}:blueprint/public-invoice"),
        ),
    ];
    for (case, blueprint_arn) in &blueprint_rows {
        assert_ne!(
            blueprint_arn, &live_blueprint,
            "case {case:?}: the table must not name the blueprint that exists"
        );
        for operation in ["GetBlueprint", "DeleteBlueprint", "CreateBlueprintVersion"] {
            let (status, body) =
                call(&router, operation, &json!({ "BlueprintArn": blueprint_arn })).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// RULE: a create cannot associate a blueprint this account does not hold, so a custom
/// configuration naming one is refused with the modelled `ResourceNotFoundException` — and
/// so is one pinning a version its blueprint never had.
#[tokio::test]
async fn a_configuration_naming_an_absent_blueprint_is_refused() {
    let router = harness();
    let blueprint = create_blueprint(&router, "held-blueprint", &["field_one"]).await;
    let account = engine::emulator_account_id();
    let absent = format!(
        "arn:aws:textract:{REGION}:{account}:blueprint/{}",
        "d".repeat(12)
    );

    // Rows differ along where the unheld reference sits: the primary list, the fallback
    // list, and the version a held blueprint is pinned to.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a primary blueprint this account does not hold",
            json!({ "Blueprints": [{ "BlueprintArn": absent.clone() }] }),
        ),
        (
            "a fallback blueprint this account does not hold",
            json!({
                "Blueprints": [{ "BlueprintArn": blueprint.clone() }],
                "FallbackBlueprints": [{ "BlueprintArn": absent.clone() }],
            }),
        ),
        (
            "a held blueprint pinned to a version it never had",
            json!({
                "Blueprints": [{
                    "BlueprintArn": blueprint.clone(),
                    "BlueprintVersion": "7",
                }],
            }),
        ),
    ];
    for (case, custom) in &rows {
        let request = merged(
            &project_request("absent-reference"),
            &json!({ "CustomConfiguration": custom }),
        );
        let (status, body) = call(&router, "CreateProject", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            body.get("ProjectArn").is_none(),
            "case {case:?}: a refused create must mint no project, got {body}"
        );
    }
}

/// RULE: the configuration rules AWS states in prose — each list type only once, `CSV` not
/// the only text format, a fallback blueprint requiring a primary one, and a blueprint
/// schema that is a JSON object — are refused with the modelled `ValidationException`,
/// whether they arrive on a create or on an update.
#[tokio::test]
async fn the_prose_configuration_rules_are_refused() {
    let router = harness();
    let blueprint = create_blueprint(&router, "prose-blueprint", &["field_one"]).await;

    // Rows differ along WHICH rule is broken and WHICH member breaks it. Each sentence is
    // the one AWS states that rule with.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a granularity level named twice",
            json!({
                "ExtractionConfiguration":
                    extraction("ENABLED", &["LINE", "LINE"], &["PLAIN_TEXT"]),
            }),
            intelligence::DUPLICATE_TYPE_MESSAGE,
        ),
        (
            "a text format named twice",
            json!({
                "ExtractionConfiguration":
                    extraction("ENABLED", &["LINE"], &["PLAIN_TEXT", "PLAIN_TEXT"]),
            }),
            intelligence::DUPLICATE_TYPE_MESSAGE,
        ),
        (
            "CSV as the only text format",
            json!({
                "ExtractionConfiguration": extraction("ENABLED", &["LINE"], &["CSV"]),
            }),
            intelligence::CSV_ONLY_TEXT_FORMAT_MESSAGE,
        ),
        (
            "a PII entity type named twice",
            json!({
                "SensitiveDataConfiguration": {
                    "DetectionMode": "DETECTION",
                    "PiiEntitiesConfiguration": {
                        "PiiEntityTypes": ["NAME", "NAME"],
                        "RedactionMaskMode": "PII",
                    },
                },
            }),
            intelligence::DUPLICATE_TYPE_MESSAGE,
        ),
        (
            "a fallback blueprint with no primary one",
            json!({
                "CustomConfiguration": {
                    "Blueprints": [],
                    "FallbackBlueprints": [{ "BlueprintArn": blueprint.clone() }],
                },
            }),
            intelligence::FALLBACK_WITHOUT_BLUEPRINT_MESSAGE,
        ),
    ];

    for (index, (case, changes, message)) in rows.iter().enumerate() {
        let request = merged(&project_request("prose-rules"), changes);
        let (status, body) = call(&router, "CreateProject", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?} on CreateProject: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} on CreateProject: expected ValidationException, got {body}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(*message),
            "case {case:?} on CreateProject: expected the sentence AWS states this rule with, \
             got {body}"
        );

        // The same rule on the update path, against a project that really exists.
        let project_arn = create_project(
            &router,
            case,
            &project_request(&format!("prose-rules-{index}")),
        )
        .await;
        let update = merged(changes, &json!({ "ProjectArn": project_arn }));
        let (status, body) = call(&router, "UpdateProject", &update).await;
        assert_eq!(
            status, 400,
            "case {case:?} on UpdateProject: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} on UpdateProject: expected ValidationException, got {body}"
        );
    }

    // A blueprint schema must be the JSON object that names the fields to extract.
    for schema_value in ["not json at all", "[1,2,3]", "\"a string\"", "42"] {
        let (status, body) = call(
            &router,
            "CreateBlueprint",
            &json!({ "BlueprintName": "bad-schema", "Schema": schema_value }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {schema_value:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {schema_value:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(intelligence::SCHEMA_NOT_AN_OBJECT_MESSAGE),
            "case {schema_value:?}: expected the schema wording, got {body}"
        );
        let (status, body) = call(
            &router,
            "UpdateBlueprint",
            &json!({ "BlueprintArn": blueprint.clone(), "Schema": schema_value }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {schema_value:?} on UpdateBlueprint: expected HTTP 400, got {status} with \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {schema_value:?} on UpdateBlueprint: expected ValidationException, got {body}"
        );
    }
}

/// RULE: a listing is cut from the stored records in one stable order, so paging through it
/// with any page size reports every resource exactly once and in the same order.
#[tokio::test]
async fn a_listing_pages_through_every_resource_exactly_once() {
    let router = harness();

    // Bounded at 7 resources so the test stays cheap, and paged at every size from 1 to 8
    // — including one past the resource count, and none at all.
    let mut created: Vec<String> = Vec::new();
    for index in 0..7u32 {
        created.push(
            create_project(
                &router,
                &format!("project #{index}"),
                &project_request(&format!("listed-{index}")),
            )
            .await,
        );
    }
    let mut blueprints: Vec<String> = Vec::new();
    for index in 0..7u32 {
        blueprints
            .push(create_blueprint(&router, &format!("listed-blueprint-{index}"), &["f"]).await);
    }

    for (operation, member, arn_member, expected) in [
        ("ListProjects", "Projects", "ProjectArn", &created),
        ("ListBlueprints", "Blueprints", "BlueprintArn", &blueprints),
    ] {
        for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8)] {
            let case = format!("{operation} with MaxResults {page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut next: Option<String> = None;
            // A generous page budget: at one resource a page the listing takes 7 pages, so
            // a loop that has not finished by 32 is a defect rather than a slow host.
            for _ in 0..32u32 {
                let mut request = Map::new();
                if let Some(size) = page_size {
                    request.insert("MaxResults".to_string(), json!(size));
                }
                if let Some(token) = &next {
                    request.insert("NextToken".to_string(), json!(token));
                }
                let (status, body) = call(&router, operation, &Value::Object(request)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the listing must succeed, got {status} with {body}"
                );
                let page = body
                    .get(member)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if let Some(size) = page_size {
                    assert!(
                        page.len() <= size,
                        "case {case:?}: a page must hold at most MaxResults entries, got {body}"
                    );
                }
                for entry in page {
                    seen.push(string_member(&entry, arn_member));
                }
                next = body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if next.is_none() {
                    break;
                }
            }
            assert!(
                next.is_none(),
                "case {case:?}: the listing must end, still holding {next:?}"
            );
            assert_eq!(
                seen.len(),
                expected.len(),
                "case {case:?}: expected {} entries, got {seen:?}",
                expected.len()
            );
            let distinct: BTreeSet<&String> = seen.iter().collect();
            assert_eq!(
                distinct.len(),
                seen.len(),
                "case {case:?}: no resource may be reported twice, got {seen:?}"
            );
            let wanted: BTreeSet<&String> = expected.iter().collect();
            assert_eq!(
                distinct, wanted,
                "case {case:?}: the listing must report exactly what was created"
            );
        }
    }

    // A `NextToken` is an offset into ONE listing, so one cut under a different filter
    // names nothing.
    let (status, body) = call(&router, "ListProjects", &json!({ "MaxResults": 2 })).await;
    assert_eq!(status, 200, "the first page must succeed, got {body}");
    let token = string_member(&body, "NextToken");
    assert!(!token.is_empty(), "the first page must hand out a cursor, got {body}");
    let (status, body) = call(
        &router,
        "ListProjects",
        &json!({
            "NextToken": token,
            "BlueprintFilter": { "BlueprintArn": blueprints[0].clone() },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a cursor cut under no filter must not index a filtered listing, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterException",
        "expected InvalidParameterException for a cursor from another listing, got {body}"
    );
}

/// RULE: each listing filter narrows the listing to the resources it names, and a filter
/// naming a resource this account does not hold is refused with the modelled
/// `ResourceNotFoundException` rather than answered with an empty page.
#[tokio::test]
async fn a_listing_filter_narrows_the_listing_to_what_it_names() {
    let router = harness();
    let invoice = create_blueprint(&router, "invoices", &["invoice_number"]).await;
    let receipt = create_blueprint(&router, "receipts", &["total_amount"]).await;
    let unused = create_blueprint(&router, "unused", &["nothing"]).await;
    // One version of the invoice blueprint, so a version-pinned filter has something to
    // name.
    let (status, body) = call(
        &router,
        "CreateBlueprintVersion",
        &json!({ "BlueprintArn": invoice.clone() }),
    )
    .await;
    assert_eq!(status, 200, "the version must be cut, got {body}");

    let invoice_project = create_project(
        &router,
        "the invoice project",
        &merged(
            &project_request("invoice-project"),
            &json!({
                "CustomConfiguration": {
                    "Blueprints": [{ "BlueprintArn": invoice.clone(), "BlueprintVersion": "1" }],
                },
            }),
        ),
    )
    .await;
    let receipt_project = create_project(
        &router,
        "the receipt project",
        &merged(
            &project_request("receipt-project"),
            &json!({
                "CustomConfiguration": { "Blueprints": [{ "BlueprintArn": receipt.clone() }] },
            }),
        ),
    )
    .await;
    let plain_project = create_project(
        &router,
        "a project associating no blueprint",
        &project_request("plain-project"),
    )
    .await;

    // Rows differ along which filter is applied and what it names.
    let project_rows: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "no filter lists every project",
            json!({}),
            vec![&invoice_project, &receipt_project, &plain_project],
        ),
        (
            "a blueprint filter lists the projects associating it",
            json!({ "BlueprintFilter": { "BlueprintArn": invoice.clone() } }),
            vec![&invoice_project],
        ),
        (
            "a version-pinned filter lists the project pinning that version",
            json!({
                "BlueprintFilter": {
                    "BlueprintArn": invoice.clone(),
                    "BlueprintVersion": "1",
                },
            }),
            vec![&invoice_project],
        ),
        (
            "a blueprint no project associates lists nothing",
            json!({ "BlueprintFilter": { "BlueprintArn": unused.clone() } }),
            vec![],
        ),
    ];
    for (case, request, expected) in &project_rows {
        let (status, body) = call(&router, "ListProjects", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the listing must succeed, got {status} with {body}"
        );
        let listed: BTreeSet<String> = body
            .get("Projects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| string_member(entry, "ProjectArn"))
            .collect();
        let wanted: BTreeSet<String> = expected.iter().map(|arn| (*arn).clone()).collect();
        assert_eq!(
            listed, wanted,
            "case {case:?}: expected {wanted:?}, got {listed:?} from {body}"
        );
    }

    // The same rule for `ListBlueprints`, over its own three filters.
    let blueprint_rows: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "no filter lists every blueprint",
            json!({}),
            vec![&invoice, &receipt, &unused],
        ),
        (
            "a blueprint ARN lists that blueprint alone",
            json!({ "BlueprintArn": receipt.clone() }),
            vec![&receipt],
        ),
        (
            "a project filter lists the blueprints that project associates",
            json!({ "ProjectFilter": { "ProjectArn": invoice_project.clone() } }),
            vec![&invoice],
        ),
        (
            "a project associating nothing lists nothing",
            json!({ "ProjectFilter": { "ProjectArn": plain_project.clone() } }),
            vec![],
        ),
        (
            "the account's own blueprints are every blueprint here",
            json!({ "ResourceOwner": intelligence::RESOURCE_OWNER_ACCOUNT }),
            vec![&invoice, &receipt, &unused],
        ),
        (
            "this emulator publishes no service blueprints",
            json!({ "ResourceOwner": intelligence::RESOURCE_OWNER_SERVICE }),
            vec![],
        ),
    ];
    for (case, request, expected) in &blueprint_rows {
        let (status, body) = call(&router, "ListBlueprints", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the listing must succeed, got {status} with {body}"
        );
        let entries = body
            .get("Blueprints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let listed: BTreeSet<String> = entries
            .iter()
            .map(|entry| string_member(entry, "BlueprintArn"))
            .collect();
        let wanted: BTreeSet<String> = expected.iter().map(|arn| (*arn).clone()).collect();
        assert_eq!(
            listed, wanted,
            "case {case:?}: expected {wanted:?}, got {listed:?} from {body}"
        );
        for entry in &entries {
            assert_eq!(
                string_member(entry, "ResourceOwner"),
                intelligence::RESOURCE_OWNER_ACCOUNT,
                "case {case:?}: a blueprint this account created is owned by it, got {entry}"
            );
            // The summary reports the latest version cut from the blueprint, which is what
            // a project pins when it names none; a blueprint never versioned reports none.
            let version = entry.get("BlueprintVersion").and_then(Value::as_str);
            let expected_version = (string_member(entry, "BlueprintArn") == invoice)
                .then_some("1");
            assert_eq!(
                version, expected_version,
                "case {case:?}: expected BlueprintVersion {expected_version:?}, got {entry}"
            );
        }
    }

    // A filter naming a resource this account does not hold selects from nothing.
    let account = engine::emulator_account_id();
    let absent_blueprint = format!(
        "arn:aws:textract:{REGION}:{account}:blueprint/{}",
        "e".repeat(12)
    );
    let absent_project = format!(
        "arn:aws:textract:{REGION}:{account}:project/{}",
        "f".repeat(12)
    );
    let absent_rows: Vec<(&str, &str, Value)> = vec![
        (
            "a blueprint filter naming nothing",
            "ListProjects",
            json!({ "BlueprintFilter": { "BlueprintArn": absent_blueprint.clone() } }),
        ),
        (
            "a blueprint version the blueprint never had",
            "ListProjects",
            json!({
                "BlueprintFilter": { "BlueprintArn": invoice.clone(), "BlueprintVersion": "9" },
            }),
        ),
        (
            "a blueprint ARN naming nothing",
            "ListBlueprints",
            json!({ "BlueprintArn": absent_blueprint.clone() }),
        ),
        (
            "a project filter naming nothing",
            "ListBlueprints",
            json!({ "ProjectFilter": { "ProjectArn": absent_project.clone() } }),
        ),
    ];
    for (case, operation, request) in &absent_rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// RULE: the `Tags` a create assigns to a project or a blueprint are that resource's tags,
/// so `ListTagsForResource` of its own ARN reports them back.
#[tokio::test]
async fn the_tags_a_create_assigns_read_back_for_that_resource() {
    let router = harness();

    // Rows differ along the resource kind and along how many pairs were assigned,
    // including none (the modelled TagMap length is 0..200).
    let project = create_project(
        &router,
        "a tagged project",
        &merged(
            &project_request("tagged-project"),
            &json!({ "Tags": { "team": "claims", "env": "test" } }),
        ),
    )
    .await;
    let untagged = create_project(
        &router,
        "an untagged project",
        &project_request("untagged-project"),
    )
    .await;
    let (status, body) = call(
        &router,
        "CreateBlueprint",
        &json!({
            "BlueprintName": "tagged-blueprint",
            "Schema": schema(&["field_one"]),
            "Tags": { "owner": "finance-ops" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the tagged blueprint must be created, got {body}");
    let blueprint = string_member(&body, "BlueprintArn");

    let rows: Vec<(&str, &String, Value)> = vec![
        ("a project with two tags", &project, json!({ "team": "claims", "env": "test" })),
        ("a project with none", &untagged, json!({})),
        ("a blueprint with one tag", &blueprint, json!({ "owner": "finance-ops" })),
    ];
    for (case, arn, expected) in &rows {
        let (status, body) = call(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceARN": (*arn).clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} with {body}"
        );
        assert_eq!(
            body.get("Tags").cloned().unwrap_or(Value::Null),
            *expected,
            "case {case:?}: expected Tags {expected}, got {body}"
        );
    }
}

/// RULE: a read is a read — repeating it, and creating and deleting OTHER resources in
/// between, never changes what it answers.
#[tokio::test]
async fn a_read_does_not_change_what_a_later_caller_sees() {
    let router = harness();
    let blueprint = create_blueprint(&router, "stable-blueprint", &["field_one"]).await;
    let project_arn = create_project(
        &router,
        "the project under test",
        &merged(
            &project_request("stable-project"),
            &json!({
                "ProjectDescription": "read me twice",
                "CustomConfiguration": { "Blueprints": [{ "BlueprintArn": blueprint.clone() }] },
                "Tags": { "team": "claims" },
            }),
        ),
    )
    .await;
    let read = json!({ "ProjectArn": project_arn.clone() });
    let (status, first) = call(&router, "GetProject", &read).await;
    assert_eq!(status, 200, "the first read must succeed, got {first}");
    let blueprint_read = json!({ "BlueprintArn": blueprint.clone() });
    let (status, first_blueprint) = call(&router, "GetBlueprint", &blueprint_read).await;
    assert_eq!(
        status, 200,
        "the first blueprint read must succeed, got {first_blueprint}"
    );

    for index in 0..3u32 {
        let case = format!("after {index} intervening creates");
        let (status, repeat) = call(&router, "GetProject", &read).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated read must succeed, got {status} with {repeat}"
        );
        assert_eq!(
            repeat, first,
            "case {case:?}: a read must answer identically, got {repeat} after {first}"
        );
        let (status, repeat) = call(&router, "GetBlueprint", &blueprint_read).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated blueprint read must succeed, got {status} with {repeat}"
        );
        assert_eq!(
            repeat, first_blueprint,
            "case {case:?}: a blueprint read must answer identically, got {repeat}"
        );

        let other = create_project(
            &router,
            &case,
            &project_request(&format!("other-project-{index}")),
        )
        .await;
        let (status, _) = call(&router, "DeleteProject", &json!({ "ProjectArn": other })).await;
        assert_eq!(status, 200, "case {case:?}: the other project must be deleted");
        // A listing is a read too.
        let (status, listed) = call(&router, "ListProjects", &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: the listing must succeed, got {listed}");
    }
}

/// RULE: `UpdateBlueprint` replaces the members the request supplies and leaves the ones it
/// omits alone — with `KmsEncryptionContext` the one member AWS exempts from whole
/// replacement — and the read of a blueprint that has no versions reports that live state.
#[tokio::test]
async fn update_blueprint_replaces_what_it_supplies_and_keeps_what_it_omits() {
    let router = harness();

    // Rows differ along WHICH members the update supplies, and along the three cases of
    // the encryption-context exception AWS states.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a new schema replaces the stored one",
            json!({ "Schema": schema(&["field_two"]) }),
            json!({
                "Schema": schema(&["field_two"]),
                "KmsKeyId": "alias/first-key",
                "KmsEncryptionContext": { "tenant": "claims" },
            }),
        ),
        (
            "an encryption configuration omitting the context keeps the stored context",
            json!({ "EncryptionConfiguration": { "KmsKeyId": "alias/second-key" } }),
            json!({
                "Schema": schema(&["field_one"]),
                "KmsKeyId": "alias/second-key",
                "KmsEncryptionContext": { "tenant": "claims" },
            }),
        ),
        (
            "an empty context clears the stored context",
            json!({
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/third-key",
                    "KmsEncryptionContext": {},
                },
            }),
            json!({
                "Schema": schema(&["field_one"]),
                "KmsKeyId": "alias/third-key",
                "KmsEncryptionContext": {},
            }),
        ),
        (
            "both members at once",
            json!({
                "Schema": schema(&["field_three"]),
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/fourth-key",
                    "KmsEncryptionContext": { "tenant": "billing" },
                },
            }),
            json!({
                "Schema": schema(&["field_three"]),
                "KmsKeyId": "alias/fourth-key",
                "KmsEncryptionContext": { "tenant": "billing" },
            }),
        ),
        (
            "an update that supplies nothing leaves both alone",
            json!({}),
            json!({
                "Schema": schema(&["field_one"]),
                "KmsKeyId": "alias/first-key",
                "KmsEncryptionContext": { "tenant": "claims" },
            }),
        ),
    ];

    for (index, (case, update, expect)) in rows.iter().enumerate() {
        // A fresh blueprint per row, so rows do not interfere.
        let (status, body) = call(
            &router,
            "CreateBlueprint",
            &json!({
                "BlueprintName": format!("updated-blueprint-{index}"),
                "Schema": schema(&["field_one"]),
                "EncryptionConfiguration": {
                    "KmsKeyId": "alias/first-key",
                    "KmsEncryptionContext": { "tenant": "claims" },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the blueprint must be created, got {status} with {body}"
        );
        let blueprint_arn = string_member(&body, "BlueprintArn");

        let (status, body) = call(
            &router,
            "UpdateBlueprint",
            &merged(update, &json!({ "BlueprintArn": blueprint_arn.clone() })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateBlueprint must succeed, got {status} with {body}"
        );
        assert_eq!(
            string_member(&body, "BlueprintArn"),
            blueprint_arn,
            "case {case:?}: UpdateBlueprint must report the blueprint it changed, got {body}"
        );

        // A blueprint with no versions has only its live state, so that is what the read
        // reports — and it carries no version number.
        let (status, read) = call(
            &router,
            "GetBlueprint",
            &json!({ "BlueprintArn": blueprint_arn.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetBlueprint must succeed, got {status} with {read}"
        );
        assert!(
            read.get("BlueprintVersion").is_none(),
            "case {case:?}: a blueprint with no versions reports none, got {read}"
        );
        for (member, wanted) in expect.as_object().cloned().unwrap_or_default() {
            let got = read.get(&member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                got, wanted,
                "case {case:?}: expected {member} {wanted} after the update, got {got} from \
                 {read}"
            );
        }
    }
}

/// RULE: a `ClientRequestToken` slot never outlives the resource it named, and the slot
/// store is bounded, so a caller that creates and deletes under a fresh token each time
/// cannot grow this service's state without end.
#[tokio::test]
async fn a_token_slot_does_not_outlive_the_resource_it_named() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    /// The slots this service currently holds.
    fn slots_held(engine: &engine::InMemoryEngine) -> usize {
        service_com_amazonaws_textract_textract::state::service_state(engine)
            .expect("this service's store")
            .lock()
            .intelligence_idempotency
            .len()
    }

    // Rows differ along the resource kind the token was spent on, and along the round: the
    // rule is that the slot goes with its resource however many rounds a caller runs.
    for round in 0..8u32 {
        let project_token = format!("project-round-{round}");
        let project = create_project(
            &router,
            &project_token,
            &merged(
                &project_request("recycled-project"),
                &json!({ "ClientRequestToken": project_token.clone() }),
            ),
        )
        .await;
        let blueprint_token = format!("blueprint-round-{round}");
        let (status, body) = call(
            &router,
            "CreateBlueprint",
            &json!({
                "BlueprintName": "recycled-blueprint",
                "Schema": schema(&["field_one"]),
                "ClientRequestToken": blueprint_token.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {blueprint_token:?}: the blueprint must be created, got {status} with {body}"
        );
        let blueprint = string_member(&body, "BlueprintArn");
        let version_token = format!("version-round-{round}");
        let (status, body) = call(
            &router,
            "CreateBlueprintVersion",
            &json!({
                "BlueprintArn": blueprint.clone(),
                "ClientRequestToken": version_token.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {version_token:?}: the version must be cut, got {status} with {body}"
        );
        let version = string_member(&body, "BlueprintVersion");
        assert_eq!(
            slots_held(&engine),
            3,
            "case {round}: the three tokens spent this round each hold one slot"
        );

        // Deleting the version drops the slot that answered with it, and deleting the
        // resources drops theirs.
        let (status, body) = call(
            &router,
            "DeleteBlueprint",
            &json!({ "BlueprintArn": blueprint.clone(), "BlueprintVersion": version }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {round}: the version must be deleted, got {status} with {body}"
        );
        assert_eq!(
            slots_held(&engine),
            2,
            "case {round}: a deleted version's slot must go with it"
        );
        for request in [
            ("DeleteProject", json!({ "ProjectArn": project })),
            ("DeleteBlueprint", json!({ "BlueprintArn": blueprint })),
        ] {
            let (status, body) = call(&router, request.0, &request.1).await;
            assert_eq!(
                status, 200,
                "case {round}: {} must succeed, got {status} with {body}",
                request.0
            );
        }
        let held = slots_held(&engine);
        assert_eq!(
            held, 0,
            "case {round}: no slot may outlive the resource it named, {held} held"
        );
    }
}

/// RULE: the default extraction configuration a job runs under when no project names one
/// covers every granularity level the model declares, so a new level in the model shows up
/// here rather than silently going unextracted.
#[test]
fn the_default_extraction_covers_every_modelled_granularity_level() {
    let default = insights::default_extraction();
    let covered: BTreeSet<String> = default
        .granularity
        .types
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    // `EXTRACTION_GRANULARITY_TYPE_VALUES` is the ExtractionGranularityType enum from the
    // Smithy model.
    let modelled: BTreeSet<String> =
        service_com_amazonaws_textract_textract::ops::types::EXTRACTION_GRANULARITY_TYPE_VALUES
            .iter()
            .map(|value| (*value).to_string())
            .collect();
    assert_eq!(
        covered, modelled,
        "the default must extract at every modelled level, missing {:?}",
        modelled.difference(&covered).collect::<Vec<_>>()
    );
}
