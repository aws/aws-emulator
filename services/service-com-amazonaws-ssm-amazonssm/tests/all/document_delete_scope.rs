// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteDocument` removes, and what it leaves where it was.
//!
//! The rule under test: a delete removes exactly what the request names at the endpoint
//! it was signed for — the one version the `DocumentVersion` selector or the
//! `VersionName` names, or the document whole when it names neither — and leaves every
//! other version of that document, every other endpoint's document of that name and
//! every other document untouched. A name or a version nothing holds any more names
//! nothing to delete (`InvalidDocument`), and the default version of a document that
//! still holds others cannot go at all, because that is the version every read naming no
//! version resolves to.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`], [`error_code`],
//! [`error_message`]) dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so what a
//! delete persisted is read back only through `DescribeDocument` and
//! `ListDocumentVersions`.
//!
//! The suites are tables whose rows differ along what the rule generalises over: which
//! spelling of the modelled `DocumentVersion` selector names the version, whether a
//! version is named by number, by `$LATEST`, by `$DEFAULT` or by the `VersionName` it was
//! given, how many versions the document holds, and which endpoint the call is signed
//! for.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// An endpoint other than [`REGION`], so a delete is seen to reach one endpoint's
/// documents and not another's.
const OTHER_REGION: &str = "eu-central-1";

/// A third endpoint, which never holds the document at all.
const THIRD_REGION: &str = "ap-southeast-2";

/// A `Command` document whose content differs only in the description it declares, so two
/// versions of one document differ in nothing else.
fn command_json(description: &str) -> String {
    format!(
        r#"{{
  "schemaVersion": "2.2",
  "description": "{description}",
  "mainSteps": [
    {{
      "action": "aws:runShellScript",
      "name": "runShell",
      "inputs": {{ "runCommand": ["echo hi"] }}
    }}
  ]
}}"#
    )
}

/// The `VersionName` the `version`th version of a document created by
/// [`document_with_versions`] carries.
fn version_name(version: u32) -> String {
    format!("v{version}")
}

/// `DeleteDocument` at `region`'s endpoint, returning `(status, body)`.
async fn delete(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DeleteDocument", body).await
}

/// `DeleteDocument` at `region`'s endpoint, asserting it succeeded.
async fn delete_ok(router: &MultiServiceRouter, region: &str, case: &str, body: &Value) {
    let (status, response) = delete(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteDocument {body} at {region} must succeed, got {status} {response}"
    );
}

/// `DescribeDocument` at `region`'s endpoint, returning `(status, body)`.
async fn describe(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DescribeDocument", body).await
}

/// `DescribeDocument`'s `Document`, asserting the read succeeded.
async fn describe_ok(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
) -> Value {
    let (status, response) = describe(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribeDocument {body} at {region} must succeed, got {status} {response}"
    );
    response["Document"].clone()
}

/// Assert that `name` names no document reachable at `region`'s endpoint, in the words the
/// service words a document miss in.
async fn assert_no_such_document(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
) {
    let (status, response) = describe(router, region, &json!({ "Name": name })).await;
    assert_eq!(
        (status, error_code(&response), error_message(&response)),
        (
            400,
            "InvalidDocument".to_string(),
            format!("Document with name {name} does not exist.")
        ),
        "case {case}: {name} names no document at {region}, got {status} {response}"
    );
}

/// The version numbers `name` holds at `region`'s endpoint, as `ListDocumentVersions`
/// reports them (newest first).
async fn held_versions(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
) -> Vec<String> {
    let (status, response) =
        call_in_region(router, region, "ListDocumentVersions", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case}: ListDocumentVersions {name} at {region} must succeed, got \
         {status} {response}"
    );
    response["DocumentVersions"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry["DocumentVersion"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create `name` at `region`'s endpoint holding `held` versions, each carrying the
/// [`version_name`] of its number, and return the `DocumentDescription` of the create.
///
/// The first version is the document's default, so a delete that names the default of a
/// document holding others is the refused case without any default having to be moved.
async fn document_with_versions(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
    held: u32,
) -> Value {
    let (status, response) = call_in_region(
        router,
        region,
        "CreateDocument",
        &json!({
            "Name": name,
            "Content": command_json("version 1"),
            "VersionName": version_name(1),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: CreateDocument {name} at {region} must succeed, got {status} {response}"
    );
    let created = response["DocumentDescription"].clone();
    for version in 2..=held {
        let (status, response) = call_in_region(
            router,
            region,
            "UpdateDocument",
            &json!({
                "Name": name,
                "Content": command_json(&format!("version {version}")),
                "VersionName": version_name(version),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateDocument {name} must append version {version}, got \
             {status} {response}"
        );
    }
    created
}

/// RULE: a delete that names ONE version removes that version and leaves the rest of the
/// document — the same document, still answering a read that names no version with its
/// default version.
///
/// The rows are the ways a request names one version: the modelled `DocumentVersion`
/// pattern admits `$LATEST`, `$DEFAULT` and a number
/// (`smithy_query(query="shape", shape="DocumentVersion")`), and `VersionName` names a
/// version by the name it was given.
#[tokio::test]
async fn a_delete_removes_only_the_version_its_selector_names() {
    let router = router();

    struct Row {
        case: &'static str,
        name: &'static str,
        /// The members that name the version, merged into the request.
        selector: Value,
        /// The version number those members name.
        removed: u32,
    }

    let rows = [
        Row {
            case: "the newest version by $LATEST",
            name: "DeleteLatestBySelector",
            selector: json!({ "DocumentVersion": "$LATEST" }),
            removed: 3,
        },
        Row {
            case: "the newest version by its number",
            name: "DeleteLatestByNumber",
            selector: json!({ "DocumentVersion": "3" }),
            removed: 3,
        },
        Row {
            case: "a version between the first and the newest, by its number",
            name: "DeleteMiddleByNumber",
            selector: json!({ "DocumentVersion": "2" }),
            removed: 2,
        },
        Row {
            case: "a version between the first and the newest, by its version name",
            name: "DeleteMiddleByName",
            selector: json!({ "VersionName": "v2" }),
            removed: 2,
        },
    ];

    let held = 3;
    for row in &rows {
        let case = row.case;
        let created = document_with_versions(&router, REGION, case, row.name, held).await;

        let mut request = json!({ "Name": row.name });
        for (member, value) in row.selector.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        delete_ok(&router, REGION, case, &request).await;

        // What is left is every other version the document held, newest first.
        let expected: Vec<String> = (1..=held)
            .rev()
            .filter(|version| *version != row.removed)
            .map(|version| version.to_string())
            .collect();
        assert_eq!(
            held_versions(&router, REGION, case, row.name).await,
            expected,
            "case {case}: only version {} is gone from {}",
            row.removed,
            row.name
        );

        // The version that went names nothing now, by number or by the name it carried.
        let removed = row.removed.to_string();
        let (status, response) = describe(
            &router,
            REGION,
            &json!({ "Name": row.name, "DocumentVersion": removed }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocumentVersion"),
            "case {case}: version {removed} names nothing after the delete, got \
             {status} {response}"
        );
        // ...and the NAME that version carried addresses no document now: a VersionName no
        // version carries is a missing document rather than an invalid version.
        let (status, response) = describe(
            &router,
            REGION,
            &json!({ "Name": row.name, "VersionName": version_name(row.removed) }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocument"),
            "case {case}: the version name {} names nothing after the delete, got \
             {status} {response}",
            version_name(row.removed)
        );

        // The document itself is the SAME document, and a read naming no version still
        // resolves to its default version.
        let read = describe_ok(&router, REGION, case, &json!({ "Name": row.name })).await;
        assert_eq!(
            (read["Name"].clone(), read["DocumentVersion"].clone()),
            (created["Name"].clone(), json!("1")),
            "case {case}: a version delete leaves the document and its default version, \
             got {read}"
        );
        // Every version that is still held is still readable, content and all.
        for version in expected {
            describe_ok(
                &router,
                REGION,
                case,
                &json!({ "Name": row.name, "DocumentVersion": version }),
            )
            .await;
        }
    }
}

/// RULE: the default version of a document goes only when it is the one version the
/// document holds — in which case the document goes whole; while the document holds
/// others, the delete is refused and removes nothing.
#[tokio::test]
async fn the_default_version_goes_only_with_the_document_whose_only_version_it_is() {
    let router = router();

    struct Row {
        case: &'static str,
        name: &'static str,
        /// How many versions the document holds when the delete is asked for.
        held: u32,
        /// The members that name the version.
        selector: Value,
    }

    // One row per spelling that resolves to the default version of a document holding one
    // version: the document's only version IS its default, its newest and its first.
    let only_version_rows = [
        Row {
            case: "$DEFAULT of a document holding one version",
            name: "DeleteOnlyVersionByDefault",
            held: 1,
            selector: json!({ "DocumentVersion": "$DEFAULT" }),
        },
        Row {
            case: "$LATEST of a document holding one version",
            name: "DeleteOnlyVersionByLatest",
            held: 1,
            selector: json!({ "DocumentVersion": "$LATEST" }),
        },
        Row {
            case: "the version name of a document holding one version",
            name: "DeleteOnlyVersionByItsName",
            held: 1,
            selector: json!({ "VersionName": "v1" }),
        },
    ];
    for row in &only_version_rows {
        let case = row.case;
        document_with_versions(&router, REGION, case, row.name, row.held).await;
        let mut request = json!({ "Name": row.name });
        for (member, value) in row.selector.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        delete_ok(&router, REGION, case, &request).await;
        // Nothing of the document is left, so every read of the name reports the miss.
        assert_no_such_document(&router, REGION, case, row.name).await;
    }

    // The same spelling against a document that holds others: refused, and every version
    // is still there afterwards.
    let refused = Row {
        case: "$DEFAULT of a document holding three versions",
        name: "DeleteDefaultOfVersionedDocument",
        held: 3,
        selector: json!({ "DocumentVersion": "$DEFAULT" }),
    };
    let case = refused.case;
    document_with_versions(&router, REGION, case, refused.name, refused.held).await;
    let mut request = json!({ "Name": refused.name });
    for (member, value) in refused.selector.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    let (status, response) = delete(&router, REGION, &request).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidDocumentOperation"),
        "case {case}: the delete is refused, got {status} {response}"
    );
    assert_eq!(
        held_versions(&router, REGION, case, refused.name).await,
        vec!["3".to_string(), "2".to_string(), "1".to_string()],
        "case {case}: a refused delete removes nothing"
    );
}

/// RULE: a delete names what the store holds, so a document or a version that is already
/// gone names nothing to delete and the call is a miss — the document the first delete
/// removed does not come back, and what it left is still held.
#[tokio::test]
async fn a_delete_of_what_is_already_gone_is_a_miss() {
    let router = router();

    // Names spanning what the modelled `DocumentName` pattern admits
    // (`smithy_query(query="shape", shape="DocumentName")`: `^[a-zA-Z0-9_\-.]{3,128}$`):
    // the shortest it allows, one carrying every admitted punctuation, and a long one.
    for name in [
        "abc",
        "Gone.Twice-Document_1",
        "ADocumentNameWellWithinTheModelledLengthLimitButFarLongerThanThree",
    ] {
        document_with_versions(&router, REGION, name, name, 1).await;
        delete_ok(&router, REGION, name, &json!({ "Name": name })).await;
        let (status, response) = delete(&router, REGION, &json!({ "Name": name })).await;
        assert_eq!(
            (status, error_code(&response), error_message(&response)),
            (
                400,
                "InvalidDocument".to_string(),
                format!("Document with name {name} does not exist.")
            ),
            "case {name}: a second delete of one document is a miss, got {status} {response}"
        );
        assert_no_such_document(&router, REGION, name, name).await;
    }

    // A version that is already gone, named each of the ways that named it before.
    let name = "VersionGoneTwiceDocument";
    document_with_versions(&router, REGION, name, name, 2).await;
    delete_ok(
        &router,
        REGION,
        name,
        &json!({ "Name": name, "DocumentVersion": "2" }),
    )
    .await;
    for selector in [
        json!({ "DocumentVersion": "2" }),
        json!({ "VersionName": "v2" }),
    ] {
        let case = format!("{selector} of a version already gone");
        let mut request = json!({ "Name": name });
        for (member, value) in selector.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let (status, response) = delete(&router, REGION, &request).await;
        let message = error_message(&response);
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocument"),
            "case {case}: the delete is a miss, got {status} {response}"
        );
        // The miss is worded around the document and the version the request named.
        assert!(
            message.contains(name),
            "case {case}: the miss must name the document, got {message:?}"
        );
    }
    // The version the first delete left is still held, and so is the document.
    assert_eq!(
        held_versions(&router, REGION, name, name).await,
        vec!["1".to_string()],
        "case {name}: the version that was not deleted is still held"
    );
}

/// RULE: a document is held at the endpoint it was created through, so a delete removes
/// only what the endpoint it was signed for holds — another endpoint's document of the
/// same name is not this call's to remove, and an endpoint holding no such document
/// reports the miss.
#[tokio::test]
async fn a_delete_reaches_only_the_documents_its_own_endpoint_holds() {
    let router = router();
    let name = "RegionalDeleteDocument";
    for region in [REGION, OTHER_REGION] {
        document_with_versions(&router, region, region, name, 1).await;
    }

    // An endpoint that holds no document of that name reports the miss, and the endpoints
    // that do hold one are left alone.
    let (status, response) = delete(&router, THIRD_REGION, &json!({ "Name": name })).await;
    assert_eq!(
        (status, error_code(&response), error_message(&response)),
        (
            400,
            "InvalidDocument".to_string(),
            format!("Document with name {name} does not exist.")
        ),
        "case {THIRD_REGION}: a delete at an endpoint holding no such document is a miss, \
         got {status} {response}"
    );
    for region in [REGION, OTHER_REGION] {
        let read = describe_ok(&router, region, region, &json!({ "Name": name })).await;
        assert_eq!(
            read["Name"],
            json!(name),
            "case {region}: a miss at another endpoint removes nothing here, got {read}"
        );
    }

    // The delete at one endpoint removes that endpoint's document and no other's.
    delete_ok(&router, REGION, REGION, &json!({ "Name": name })).await;
    assert_no_such_document(&router, REGION, REGION, name).await;
    let elsewhere = describe_ok(&router, OTHER_REGION, OTHER_REGION, &json!({ "Name": name })).await;
    assert_eq!(
        elsewhere["Name"],
        json!(name),
        "case {OTHER_REGION}: another endpoint's document of the same name is still held, \
         got {elsewhere}"
    );

    // And that other endpoint's document is still the account's to delete.
    delete_ok(&router, OTHER_REGION, OTHER_REGION, &json!({ "Name": name })).await;
    assert_no_such_document(&router, OTHER_REGION, OTHER_REGION, name).await;
}
