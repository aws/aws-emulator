// SPDX-License-Identifier: CC0-1.0

//! What an SSM document REPORTS about itself, and what is left once it is deleted.
//!
//! The rules under test:
//!
//! 1. `PlatformTypes` is derived from the document's own steps: a step runs where its
//!    plugin AND its `platformType` precondition both allow it, the document is compatible
//!    with the union over its steps, and content whose steps restrict nothing is
//!    compatible with every platform the model lists — reported in the order the modelled
//!    `PlatformType` enum declares them.
//! 2. Every document reports its tag set as a list that is EMPTY rather than absent when
//!    it carries no tag, and NO read reports a `DocumentId`: no shape of this service's
//!    model declares that member, so a client reads a document's identity off the name it
//!    is addressed by and the version members beside it. A describe additionally reports the
//!    classification members as empty lists for a document nothing classified.
//! 3. `DeleteDocument` removes what a create persisted: the document (or the one version
//!    the call named) and the associations that applied it, so every later read of that
//!    name reports `InvalidDocument` and the name is free to create again.
//! 4. A create of a name the Region already holds is refused as `DocumentAlreadyExists`,
//!    worded around the name the request asked for.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`error_code`], [`error_message`])
//! dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the
//! multi-service router. Nothing reaches into the state struct, so every read follows the
//! write it must report.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — the plugin a step runs, its precondition, the schema the steps are
//! declared in, the document type, the tag set, which version a delete names, the name a
//! create asks for — rather than the single document a recorded unit happened to create.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, error_message, router};

/// The `PlatformType` values the model's enum declares, in the order it declares them
/// (`smithy_query(query="shape", shape="PlatformType")`), which is what a document whose
/// steps restrict it to no platform is compatible with.
const PLATFORM_TYPES: &[&str] = &["Windows", "Linux", "MacOS"];

/// A document Systems Manager publishes (`crate::state::PUBLISHED_DOCUMENTS`): a command
/// document whose steps are restricted to the Unix-like platforms.
const PUBLISHED_SHELL_DOCUMENT: &str = "AWS-RunShellScript";

/// A `Command` document declaring one step, parameterised by the plugin that step runs and
/// the precondition it carries, so a row differs in nothing else.
fn command_with_step(action: &str, precondition: Option<&str>) -> String {
    let precondition = precondition.map_or_else(String::new, |platform| {
        format!(r#""precondition": {{ "StringEquals": ["platformType", "{platform}"] }},"#)
    });
    format!(
        r#"{{
  "schemaVersion": "2.2",
  "description": "one step",
  "mainSteps": [
    {{
      "name": "theStep",
      "action": "{action}",
      {precondition}
      "inputs": {{ "runCommand": ["echo hi"] }}
    }}
  ]
}}"#
    )
}

/// `CreateDocument` with `body`, returning `(status, body)`.
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "CreateDocument", body).await
}

/// `CreateDocument`, asserting it succeeded, and returning its `DocumentDescription`.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateDocument must succeed, got {status} {response}"
    );
    response["DocumentDescription"].clone()
}

/// `DescribeDocument`, returning `(status, body)`.
async fn describe(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "DescribeDocument", body).await
}

/// `DescribeDocument`'s `Document`, asserting the read succeeded.
async fn describe_ok(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, response) = describe(router, &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribeDocument {name} must succeed, got {status} {response}"
    );
    response["Document"].clone()
}

/// `DeleteDocument` with `body`, returning `(status, body)`.
async fn delete(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "DeleteDocument", body).await
}

/// `DeleteDocument`, asserting it succeeded.
async fn delete_ok(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = delete(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteDocument {body} must succeed, got {status} {response}"
    );
}

/// The string members of a reported list, in the order reported.
fn string_list(value: &Value, member: &str) -> Vec<String> {
    value[member]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a report carries a `DocumentId` at all: no shape this service's model declares
/// has that member (`smithy_query(query="shape", shape="DocumentDescription")`), so every
/// read answers without one.
fn reports_a_document_id(description: &Value) -> bool {
    description.get("DocumentId").is_some()
}

/// The names a listing of this Region's documents reports, walked page by page.
///
/// The published catalogue is larger than the modelled `MaxResults` bound
/// (`ListDocumentsRequest$MaxResults`, range 1..50), so the names a Region holds are a walk
/// of the listing rather than one page of it.
async fn listed_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Generous: the catalogue is hundreds of documents and a page holds up to 50, so a walk
    // that has not ended by here is a broken token rather than a long listing.
    for _ in 0..200 {
        let mut body = json!({ "MaxResults": 50 });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "ListDocuments", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: ListDocuments must succeed, got {status} {response}"
        );
        if let Some(identifiers) = response["DocumentIdentifiers"].as_array() {
            names.extend(
                identifiers
                    .iter()
                    .map(|identifier| identifier["Name"].as_str().unwrap_or_default().to_string()),
            );
        }
        token = response["NextToken"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {case}: the walk of the listing must end, got a token after {} names",
        names.len()
    );
    names
}

/// The platforms a listing reports for one document.
async fn listed_platforms(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<String> {
    let (status, response) = call(
        router,
        "ListDocuments",
        &json!({ "Filters": [{ "Key": "Name", "Values": [name] }], "MaxResults": 50 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListDocuments must succeed, got {status} {response}"
    );
    let identifier = response["DocumentIdentifiers"]
        .as_array()
        .and_then(|identifiers| {
            identifiers
                .iter()
                .find(|identifier| identifier["Name"].as_str() == Some(name))
                .cloned()
        })
        .unwrap_or(Value::Null);
    string_list(&identifier, "PlatformTypes")
}

/// One platform row: a document body, and the platforms every read of it must report.
struct Platforms<'a> {
    case: &'a str,
    name: &'a str,
    document_type: &'a str,
    format: &'a str,
    content: String,
    expected: &'a [&'a str],
}

/// RULE: a step runs where its plugin AND its `platformType` precondition both allow it,
/// the document is compatible with the union over its steps, and content that restricts
/// nothing is compatible with every modelled platform — reported in the enum's order.
#[tokio::test]
async fn the_platforms_a_document_reports_are_derived_from_its_steps() {
    let router = router();
    let rows = vec![
        Platforms {
            case: "a shell step with no precondition",
            name: "ShellNoPrecondition",
            document_type: "Command",
            format: "JSON",
            content: command_with_step("aws:runShellScript", None),
            expected: &["Linux", "MacOS"],
        },
        Platforms {
            case: "a PowerShell step with no precondition",
            name: "PowerShellNoPrecondition",
            document_type: "Command",
            format: "JSON",
            content: command_with_step("aws:runPowerShellScript", None),
            expected: &["Windows"],
        },
        Platforms {
            case: "a shell step narrowed to MacOS",
            name: "ShellOnMacOS",
            document_type: "Command",
            format: "JSON",
            content: command_with_step("aws:runShellScript", Some("MacOS")),
            expected: &["MacOS"],
        },
        Platforms {
            case: "a shell step narrowed to Linux",
            name: "ShellOnLinux",
            document_type: "Command",
            format: "JSON",
            content: command_with_step("aws:runShellScript", Some("Linux")),
            expected: &["Linux"],
        },
        Platforms {
            case: "a PowerShell step and a shell step, so the union spans both",
            name: "ShellAndPowerShell",
            document_type: "Command",
            format: "YAML",
            content: r#"schemaVersion: '2.2'
description: two steps
mainSteps:
  - name: onUnix
    action: 'aws:runShellScript'
    inputs:
      runCommand: ['echo hi']
  - name: onWindows
    action: 'aws:runPowerShellScript'
    inputs:
      runCommand: ['Write-Host hi']
"#
            .to_string(),
            expected: PLATFORM_TYPES,
        },
        Platforms {
            // `aws:psModule` is a Windows-only plugin of the catalog
            // (`crate::state::document_platforms::plugin_platform_types`), declared the
            // schema 1.2 way: the plugin NAMES are the `runtimeConfig` keys.
            case: "a schema 1.2 runtimeConfig plugin",
            name: "LegacyPsModule",
            document_type: "Command",
            format: "JSON",
            content: r#"{
  "schemaVersion": "1.2",
  "description": "a legacy document",
  "runtimeConfig": { "aws:psModule": { "properties": [] } }
}"#
            .to_string(),
            expected: &["Windows"],
        },
        Platforms {
            case: "an Automation runbook, whose actions run in the service",
            name: "AutomationRunsAnywhere",
            document_type: "Automation",
            format: "JSON",
            content: r#"{
  "schemaVersion": "0.3",
  "description": "a runbook",
  "mainSteps": [
    { "name": "call", "action": "aws:executeAwsApi", "inputs": { "Service": "s3", "Api": "ListBuckets" } }
  ]
}"#
            .to_string(),
            expected: PLATFORM_TYPES,
        },
        Platforms {
            case: "a document that declares no step at all",
            name: "SessionNoSteps",
            document_type: "Session",
            format: "JSON",
            content: r#"{ "schemaVersion": "1.0", "description": "a session", "sessionType": "Standard_Stream" }"#
                .to_string(),
            expected: PLATFORM_TYPES,
        },
    ];
    // Every modelled platform has to be the first one some row reports, so the ORDER the
    // rule fixes is seen rather than only the set.
    for platform in PLATFORM_TYPES {
        assert!(
            rows.iter()
                .any(|row| row.expected.first() == Some(platform)),
            "the table must carry a row whose first reported platform is {platform}"
        );
    }

    for row in &rows {
        let case = row.case;
        let created = create_ok(
            &router,
            case,
            &json!({
                "Name": row.name,
                "Content": row.content,
                "DocumentType": row.document_type,
                "DocumentFormat": row.format,
            }),
        )
        .await;
        let expected: Vec<String> = row.expected.iter().map(|p| (*p).to_string()).collect();
        assert_eq!(
            string_list(&created, "PlatformTypes"),
            expected,
            "case {case}: expected the create to report PlatformTypes {:?}, got {created}",
            row.expected
        );
        let read = describe_ok(&router, case, row.name).await;
        assert_eq!(
            string_list(&read, "PlatformTypes"),
            expected,
            "case {case}: expected the describe to report PlatformTypes {:?}, got {read}",
            row.expected
        );
        assert_eq!(
            listed_platforms(&router, case, row.name).await,
            expected,
            "case {case}: expected the listing to report PlatformTypes {:?}",
            row.expected
        );
    }

    // A document Systems Manager publishes is read the same way: its own steps decide it.
    let published = describe_ok(&router, "the published shell document", PUBLISHED_SHELL_DOCUMENT).await;
    assert_eq!(
        string_list(&published, "PlatformTypes"),
        vec!["Linux".to_string(), "MacOS".to_string()],
        "case the published shell document: its steps restrict it to the Unix-like \
         platforms, got {published}"
    );
}

/// RULE: every document reports its tag set as a list that is empty rather than absent when
/// it carries no tag, and no read reports a `DocumentId` — no shape of the model declares
/// one — whichever read answers and however many versions the document holds.
#[tokio::test]
async fn every_document_carries_a_tag_list_and_no_document_id() {
    let router = router();
    let content = command_with_step("aws:runShellScript", None);
    // (case, name, the tags the create submits, the tag list every read must report)
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        ("no tag at all", "UntaggedDocument", Value::Null, json!([])),
        (
            "one tag",
            "OneTagDocument",
            json!([{ "Key": "Environment", "Value": "Production" }]),
            json!([{ "Key": "Environment", "Value": "Production" }]),
        ),
        (
            "three tags",
            "ThreeTagDocument",
            json!([
                { "Key": "Environment", "Value": "Staging" },
                { "Key": "Owner", "Value": "payments" },
                { "Key": "cost:center", "Value": "4711" },
            ]),
            json!([
                { "Key": "Environment", "Value": "Staging" },
                { "Key": "Owner", "Value": "payments" },
                { "Key": "cost:center", "Value": "4711" },
            ]),
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, name, submitted, expected_tags) in &rows {
        let mut body = json!({ "Name": name, "Content": content });
        if !submitted.is_null() {
            body["Tags"] = submitted.clone();
        }
        let created = create_ok(&router, case, &body).await;
        assert!(
            !reports_a_document_id(&created),
            "case {case}: no member of the created document's report is a DocumentId, got \
             {created}"
        );
        seen.push((*name).to_string());
        assert_eq!(
            created["Tags"], *expected_tags,
            "case {case}: expected the create to report Tags {expected_tags}, got {created}"
        );
        // The create's own response carries no classification members; a describe carries
        // both, as the empty lists a document nothing classified has.
        for member in ["Category", "CategoryEnum"] {
            assert_eq!(
                created[member],
                Value::Null,
                "case {case}: a create reports no {member}, got {created}"
            );
        }

        // Two reads report the same document, and the same tag list.
        for round in ["first read", "second read"] {
            let read = describe_ok(&router, case, name).await;
            assert_eq!(
                read["Name"].as_str(),
                Some(*name),
                "case {case} {round}: a read must report the document the create named, got {read}"
            );
            assert!(
                !reports_a_document_id(&read),
                "case {case} {round}: no member of a read is a DocumentId, got {read}"
            );
            assert_eq!(
                read["Tags"], *expected_tags,
                "case {case} {round}: expected Tags {expected_tags}, got {read}"
            );
            for member in ["Category", "CategoryEnum"] {
                assert_eq!(
                    read[member],
                    json!([]),
                    "case {case} {round}: a describe reports an empty {member} list, got {read}"
                );
            }
        }

        // An update appends a version to the SAME document, so the id does not move.
        let (status, response) = call(
            &router,
            "UpdateDocument",
            &json!({
                "Name": name,
                "Content": command_with_step("aws:runPowerShellScript", None),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateDocument must succeed, got {status} {response}"
        );
        let updated = response["DocumentDescription"].clone();
        assert_eq!(
            (updated["Name"].clone(), updated["LatestVersion"].clone()),
            (json!(name), json!("2")),
            "case {case}: an appended version belongs to the same document, got {updated}"
        );
    }

    // A document Systems Manager publishes is read the same way: two reads report the one
    // document under the name it is addressed by, and neither carries a DocumentId.
    let first = describe_ok(&router, "the published document", PUBLISHED_SHELL_DOCUMENT).await;
    let second = describe_ok(&router, "the published document again", PUBLISHED_SHELL_DOCUMENT).await;
    assert_eq!(
        first["Name"].as_str(),
        Some(PUBLISHED_SHELL_DOCUMENT),
        "case the published document: expected the name it was read by, got {first}"
    );
    assert_eq!(
        first, second,
        "case the published document: two reads report one document, got {second}"
    );
    assert!(
        !reports_a_document_id(&first),
        "case the published document: no member of its report is a DocumentId, got {first}"
    );
    assert!(
        !seen.contains(&PUBLISHED_SHELL_DOCUMENT.to_string()),
        "case the published document: its name must differ from every created one, got {seen:?}"
    );
    assert_eq!(
        first["Tags"],
        json!([]),
        "case the published document: an untagged document reports an empty Tags list, got {first}"
    );
}

/// One deletion row: the document a case creates, and how the delete names it.
struct Deletion<'a> {
    case: &'a str,
    name: &'a str,
    document_type: &'a str,
    format: &'a str,
    content: String,
    /// What the delete adds to `{"Name": …}` beside the name.
    selector: Value,
}

/// RULE: a delete removes what the create persisted, so every later read of that name
/// reports `InvalidDocument` — worded around the name the request asked for — the name is
/// gone from a listing, and it is free to create again.
#[tokio::test]
async fn a_deleted_document_is_gone_from_every_read() {
    let router = router();
    let shell = command_with_step("aws:runShellScript", None);
    let rows = vec![
        Deletion {
            case: "a JSON Command document, no selector at all",
            name: "DeleteWholeCommand",
            document_type: "Command",
            format: "JSON",
            content: shell.clone(),
            selector: json!({}),
        },
        Deletion {
            case: "a YAML Automation runbook, with the Force flag",
            name: "DeleteWholeAutomation",
            document_type: "Automation",
            format: "YAML",
            content: r#"schemaVersion: '0.3'
description: a runbook
mainSteps:
  - name: call
    action: 'aws:executeAwsApi'
    inputs:
      Service: s3
      Api: ListBuckets
"#
            .to_string(),
            selector: json!({ "Force": true }),
        },
        Deletion {
            case: "a TEXT Change Calendar entry",
            name: "DeleteWholeCalendar",
            document_type: "ChangeCalendar",
            format: "TEXT",
            content: "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR".to_string(),
            selector: json!({}),
        },
        Deletion {
            case: "the only version named by number",
            name: "DeleteOnlyVersionByNumber",
            document_type: "Command",
            format: "JSON",
            content: shell.clone(),
            selector: json!({ "DocumentVersion": "1" }),
        },
        Deletion {
            case: "the only version named by its version name",
            name: "DeleteOnlyVersionByName",
            document_type: "Command",
            format: "JSON",
            content: shell.clone(),
            selector: json!({ "VersionName": "Release12.1" }),
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut body = json!({
            "Name": row.name,
            "Content": row.content,
            "DocumentType": row.document_type,
            "DocumentFormat": row.format,
        });
        if row.selector["VersionName"].is_string() {
            body["VersionName"] = row.selector["VersionName"].clone();
        }
        let created = create_ok(&router, case, &body).await;
        assert!(
            listed_names(&router, case).await.contains(&row.name.to_string()),
            "case {case}: the created document must be listed before the delete"
        );
        assert_eq!(
            created["LatestVersion"].clone(),
            json!("1"),
            "case {case}: a created document holds one version, got {created}"
        );

        let mut request = json!({ "Name": row.name });
        for (member, value) in row.selector.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        delete_ok(&router, case, &request).await;

        // Every read of the name reports the miss, in the words the service words one in.
        let expected_message = format!("Document with name {} does not exist.", row.name);
        for operation in ["DescribeDocument", "GetDocument"] {
            let (status, response) = call(&router, operation, &json!({ "Name": row.name })).await;
            assert_eq!(
                (status, error_code(&response), error_message(&response)),
                (400, "InvalidDocument".to_string(), expected_message.clone()),
                "case {case}: {operation} of a deleted document reports the miss, got \
                 {status} {response}"
            );
        }
        assert!(
            !listed_names(&router, case).await.contains(&row.name.to_string()),
            "case {case}: a deleted document is gone from the listing"
        );

        // The name is free again, and the document created under it is a NEW document: its
        // version history starts over, so nothing of the deleted one is left behind.
        let recreated = create_ok(
            &router,
            case,
            &json!({ "Name": row.name, "Content": shell, "DocumentType": "Command" }),
        )
        .await;
        assert_eq!(
            (
                recreated["LatestVersion"].clone(),
                recreated["DocumentVersion"].clone()
            ),
            (json!("1"), json!("1")),
            "case {case}: a recreated document is a new document whose history starts over, \
             got {recreated}"
        );
    }
}

/// RULE: a delete that names no document of this account's, a document Systems Manager
/// owns, a document type whose delete requires the `Force` flag, or the default version of
/// a document that holds others, is refused and removes nothing.
#[tokio::test]
async fn a_delete_the_service_cannot_serve_is_refused_and_removes_nothing() {
    let router = router();
    let shell = command_with_step("aws:runShellScript", None);

    // A name no document is held under, however it is spelled.
    for name in ["NoSuchDocument", "Another.Missing-Document_1", "abc"] {
        let (status, response) = delete(&router, &json!({ "Name": name })).await;
        assert_eq!(
            (status, error_code(&response), error_message(&response)),
            (
                400,
                "InvalidDocument".to_string(),
                format!("Document with name {name} does not exist.")
            ),
            "case {name:?}: a delete of a name nothing holds is refused, got {status} {response}"
        );
    }

    // A document Systems Manager publishes is not the account's to remove, and is still
    // there afterwards.
    let (status, response) = delete(&router, &json!({ "Name": PUBLISHED_SHELL_DOCUMENT })).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidDocumentOperation"),
        "case a published document: the delete is refused, got {status} {response}"
    );
    let still_there = describe_ok(&router, "a published document", PUBLISHED_SHELL_DOCUMENT).await;
    assert_eq!(
        still_there["Name"],
        json!(PUBLISHED_SHELL_DOCUMENT),
        "case a published document: a refused delete leaves it where it was, got {still_there}"
    );

    // "You must specify a Force flag to delete a document of type
    // ApplicationConfigurationSchema" (`DeleteDocument`'s `Force` doc).
    let schema_name = "ForcedSchemaDocument";
    create_ok(
        &router,
        "a schema document",
        &json!({
            "Name": schema_name,
            "Content": r#"{ "$schema": "http://json-schema.org/draft-07/schema#", "type": "object" }"#,
            "DocumentType": "ApplicationConfigurationSchema",
        }),
    )
    .await;
    for (case, request, expected) in [
        (
            "no Force flag",
            json!({ "Name": schema_name }),
            Some("InvalidDocumentOperation"),
        ),
        (
            "Force explicitly false",
            json!({ "Name": schema_name, "Force": false }),
            Some("InvalidDocumentOperation"),
        ),
        ("Force true", json!({ "Name": schema_name, "Force": true }), None),
    ] {
        let (status, response) = delete(&router, &request).await;
        match expected {
            Some(code) => {
                assert_eq!(
                    (status, error_code(&response).as_str()),
                    (400, code),
                    "case {case}: the delete is refused, got {status} {response}"
                );
                // Refused, so the document is still readable.
                describe_ok(&router, case, schema_name).await;
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: the delete is served, got {status} {response}"
                );
                let (status, response) = describe(&router, &json!({ "Name": schema_name })).await;
                assert_eq!(
                    (status, error_code(&response).as_str()),
                    (400, "InvalidDocument"),
                    "case {case}: the document is gone, got {status} {response}"
                );
            }
        }
    }

    // A document holding three versions, the first of which is its default: the default
    // cannot go while the others are held, a version it holds none of names nothing, and a
    // version that is not the default goes on its own.
    let versioned = "VersionedDeleteDocument";
    create_ok(
        &router,
        "the versioned document",
        &json!({ "Name": versioned, "Content": shell }),
    )
    .await;
    for appended in ["second", "third"] {
        let (status, response) = call(
            &router,
            "UpdateDocument",
            &json!({
                "Name": versioned,
                "Content": command_with_step("aws:runShellScript", Some(if appended == "second" { "Linux" } else { "MacOS" })),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "the {appended} version must be appended, got {status} {response}"
        );
    }
    for (case, selector, expected) in [
        ("the default version while others are held", json!("1"), Some("InvalidDocumentOperation")),
        ("a version the document holds none of", json!("9"), Some("InvalidDocument")),
        ("a version that is not the default", json!("2"), None),
    ] {
        let (status, response) = delete(
            &router,
            &json!({ "Name": versioned, "DocumentVersion": selector }),
        )
        .await;
        match expected {
            Some(code) => assert_eq!(
                (status, error_code(&response).as_str()),
                (400, code),
                "case {case}: the delete is refused, got {status} {response}"
            ),
            None => assert_eq!(
                status, 200,
                "case {case}: the delete is served, got {status} {response}"
            ),
        }
    }
    // What is left is the document, without the one version that was deleted.
    let read = describe_ok(&router, "after the version delete", versioned).await;
    assert_eq!(
        (read["DocumentVersion"].clone(), read["LatestVersion"].clone()),
        (json!("1"), json!("3")),
        "case after the version delete: the default version answers a read, got {read}"
    );
    let (status, response) = describe(
        &router,
        &json!({ "Name": versioned, "DocumentVersion": "2" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidDocumentVersion"),
        "case after the version delete: the deleted version names nothing, got {status} {response}"
    );
    for held in ["1", "3"] {
        let (status, response) = describe(
            &router,
            &json!({ "Name": versioned, "DocumentVersion": held }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case after the version delete: version {held} is still held, got {status} {response}"
        );
    }
}

/// RULE: "deletes the … document and all managed node associations to the document", so an
/// association that applied the deleted document is gone with it.
#[tokio::test]
async fn deleting_a_document_deletes_the_associations_that_applied_it() {
    let router = router();
    let content = command_with_step("aws:runShellScript", None);
    // Two documents, so the delete is seen to take the associations of the document it
    // removed and to leave every other document's alone.
    for name in ["AssociatedDocument", "UntouchedDocument"] {
        create_ok(&router, name, &json!({ "Name": name, "Content": content })).await;
    }
    let mut associations: Vec<(&str, String)> = Vec::new();
    for name in ["AssociatedDocument", "UntouchedDocument"] {
        let (status, response) = call(&router, "CreateAssociation", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name}: CreateAssociation must succeed, got {status} {response}"
        );
        let id = response["AssociationDescription"]["AssociationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty(),
            "case {name}: the create must report an AssociationId, got {response}"
        );
        associations.push((name, id));
    }

    delete_ok(
        &router,
        "the associated document",
        &json!({ "Name": "AssociatedDocument" }),
    )
    .await;

    for (document, id) in &associations {
        let (status, response) =
            call(&router, "DescribeAssociation", &json!({ "AssociationId": id })).await;
        if *document == "AssociatedDocument" {
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "AssociationDoesNotExist"),
                "case {document}: the association went with the document, got {status} {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {document}: another document's association is untouched, got \
                 {status} {response}"
            );
        }
    }

    let (status, response) = call(&router, "ListAssociations", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListAssociations must succeed, got {status} {response}"
    );
    let listed: Vec<String> = response["Associations"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec!["UntouchedDocument".to_string()],
        "only the surviving document's association is listed, got {response}"
    );
}

/// RULE: a create of a name the Region already holds is refused as
/// `DocumentAlreadyExists`, worded around the name the request asked for and naming no
/// Region.
#[tokio::test]
async fn a_second_create_of_a_held_name_is_refused_in_the_services_words() {
    let router = router();
    let content = command_with_step("aws:runShellScript", None);
    // Names spanning what the modelled `DocumentName` pattern admits: the shortest it
    // allows, one with every admitted punctuation, and a long one.
    for name in [
        "abc",
        "my-probe.doc_1",
        "AVeryLongDocumentNameThatIsStillWellWithinTheModelledLengthLimit",
    ] {
        create_ok(
            &router,
            name,
            &json!({ "Name": name, "Content": content }),
        )
        .await;
        // Different content, so the refusal cannot be mistaken for content validation.
        let (status, response) = create(
            &router,
            &json!({ "Name": name, "Content": command_with_step("aws:runPowerShellScript", None) }),
        )
        .await;
        let message = error_message(&response);
        assert_eq!(
            (status, error_code(&response), message.clone()),
            (
                400,
                "DocumentAlreadyExists".to_string(),
                format!("Document with same name {name} already exists")
            ),
            "case {name}: a second create of one name is refused, got {status} {response}"
        );
        // The sentence states the name and nothing of where the document is held.
        assert!(
            !message.contains("us-west-2"),
            "case {name}: the refusal must name no Region, got {message:?}"
        );
        // The refused create left the first document as it was.
        let read = describe_ok(&router, name, name).await;
        assert_eq!(
            string_list(&read, "PlatformTypes"),
            vec!["Linux".to_string(), "MacOS".to_string()],
            "case {name}: a refused create must leave the stored content alone, got {read}"
        );
    }
}
