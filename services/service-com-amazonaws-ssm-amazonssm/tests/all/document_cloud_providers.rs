// SPDX-License-Identifier: CC0-1.0

//! The cloud providers an SSM document version can run against.
//!
//! `CloudProviders` is a per-VERSION member of a document — "the cloud providers that the
//! document can run against" on a create, "the cloud providers that the new document version
//! can run against" on an update — carrying `CloudProvider` values
//! (`smithy_query(query="shape", shape="CloudProvider")`: `AWS | Azure | GCP`). The rules
//! under test:
//!
//! 1. A create records the providers it named, and "if you don't specify a value, the
//!    default is `AWS`" (`CreateDocumentRequest$CloudProviders`) — reported back by the
//!    create itself, by a describe of the document, by a listing of this Region's documents
//!    and by a listing of the document's versions, all from the one record.
//! 2. An update records the providers IT named for the version it appends, and "if you don't
//!    specify a value, the new version uses the cloud providers of the latest version"
//!    (`UpdateDocumentRequest$CloudProviders`) — and the version already held keeps its own,
//!    because the member is per version.
//! 3. The list is bounded as the model bounds it (`CloudProviderList`, `@length(min: 1,
//!    max: 10)`) and admits only the values the enum declares, so a request outside either
//!    is refused and creates nothing.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`call`], [`error_code`], [`router`]) dispatches `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router — so every read
//! follows the write it must report.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, router};

/// The provider a version whose call named none runs against: "if you don't specify a value,
/// the default is `AWS`".
const DEFAULT_PROVIDER: &str = "AWS";

/// The published ceiling of `CloudProviderList`
/// (`smithy_query(query="shape", shape="CloudProviderList")`: `length 1..10`).
const PROVIDERS_MAX: usize = 10;

/// The published floor of the same list.
const PROVIDERS_MIN: usize = 1;

/// A `Command` document declaring one step, which every row creates.
fn content() -> String {
    r#"{
  "schemaVersion": "2.2",
  "description": "one step",
  "mainSteps": [
    {
      "name": "theStep",
      "action": "aws:runShellScript",
      "inputs": { "runCommand": ["echo hi"] }
    }
  ]
}"#
    .to_string()
}

/// A second body, so an update appends a version whose content differs.
fn other_content() -> String {
    content().replace("echo hi", "echo there")
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
        "case {case:?}: CreateDocument must succeed, got {status} {response}"
    );
    response["DocumentDescription"].clone()
}

/// `DescribeDocument`'s `Document` for `body`, asserting the read succeeded.
async fn describe_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "DescribeDocument", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeDocument {body} must succeed, got {status} {response}"
    );
    response["Document"].clone()
}

/// The `CloudProviders` a report carries, as the strings it lists.
fn providers(report: &Value) -> Vec<String> {
    report["CloudProviders"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `CloudProviders` the listing of this Region's documents reports for `name`, walked
/// page by page because the published catalogue is larger than one page.
async fn listed_providers(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<String> {
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": 50 });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "ListDocuments", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListDocuments must succeed, got {status} {response}"
        );
        let identifiers = response["DocumentIdentifiers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a listing reports a list, got {response}"))
            .clone();
        if let Some(found) = identifiers
            .iter()
            .find(|identifier| identifier["Name"].as_str() == Some(name))
        {
            return providers(found);
        }
        token = response["NextToken"].as_str().map(str::to_string);
        assert!(
            token.is_some(),
            "case {case:?}: the listing must report the document {name}"
        );
    }
}

/// The `CloudProviders` a listing of `name`'s versions reports for the version `version`.
async fn version_providers(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    version: &str,
) -> Vec<String> {
    let (status, response) = call(router, "ListDocumentVersions", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListDocumentVersions must succeed, got {status} {response}"
    );
    let versions = response["DocumentVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a listing reports a list, got {response}"))
        .clone();
    let found = versions
        .iter()
        .find(|info| info["DocumentVersion"].as_str() == Some(version))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must report version {version}, got {response}")
        });
    providers(found)
}

/// RULE 1: every read of a document reports the cloud providers its create named, or the
/// default for a create that named none.
#[tokio::test]
async fn a_create_records_the_cloud_providers_it_named() {
    /// One row: the document a case creates, the `CloudProviders` the create carries (`None`
    /// for a create that names none) and what every read of it must report.
    struct Row {
        case: &'static str,
        name: &'static str,
        named: Option<Vec<&'static str>>,
        expected: Vec<&'static str>,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "a create naming none, which is the documented default",
            name: "ProvidersDefaulted",
            named: None,
            expected: vec![DEFAULT_PROVIDER],
        },
        Row {
            case: "a create naming the default explicitly",
            name: "ProvidersAws",
            named: Some(vec!["AWS"]),
            expected: vec!["AWS"],
        },
        Row {
            case: "a create naming one third-party provider",
            name: "ProvidersAzure",
            named: Some(vec!["Azure"]),
            expected: vec!["Azure"],
        },
        Row {
            case: "a create naming the other third-party provider",
            name: "ProvidersGcp",
            named: Some(vec!["GCP"]),
            expected: vec!["GCP"],
        },
        Row {
            case: "a create naming every value the enum declares, in the order it sent them",
            name: "ProvidersAll",
            named: Some(vec!["GCP", "AWS", "Azure"]),
            expected: vec!["GCP", "AWS", "Azure"],
        },
    ];

    let router = router();
    for row in &rows {
        let (case, name) = (row.case, row.name);
        let expected: Vec<String> = row
            .expected
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        let mut body = json!({ "Name": name, "Content": content() });
        if let Some(named) = &row.named {
            body["CloudProviders"] = json!(named);
        }
        let created = create_ok(&router, case, &body).await;
        assert_eq!(
            providers(&created),
            expected,
            "case {case:?}: expected the create to report CloudProviders {expected:?}, got \
             {created}"
        );
        let read = describe_ok(&router, case, &json!({ "Name": name })).await;
        assert_eq!(
            providers(&read),
            expected,
            "case {case:?}: expected the describe to report {expected:?}, got {read}"
        );
        assert_eq!(
            listed_providers(&router, case, name).await,
            expected,
            "case {case:?}: expected the document listing to report {expected:?}"
        );
        assert_eq!(
            version_providers(&router, case, name, "1").await,
            expected,
            "case {case:?}: expected the version listing to report {expected:?}"
        );
    }
}

/// RULE 2: the version an update appends carries the providers the UPDATE named, or the
/// latest version's where it named none, and the version already held keeps its own.
#[tokio::test]
async fn an_update_records_the_providers_of_the_version_it_appends() {
    /// One row: the document a case creates, the providers its create and its update each
    /// name (`None` for a call that names none) and what the version the update appends must
    /// run against.
    struct Row {
        case: &'static str,
        name: &'static str,
        created_with: Option<Vec<&'static str>>,
        updated_with: Option<Vec<&'static str>>,
        expected: Vec<&'static str>,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "an update naming none inherits the latest version's",
            name: "InheritedProviders",
            created_with: Some(vec!["Azure"]),
            updated_with: None,
            expected: vec!["Azure"],
        },
        Row {
            case: "an update naming none where the create named none either",
            name: "InheritedDefaultProviders",
            created_with: None,
            updated_with: None,
            expected: vec![DEFAULT_PROVIDER],
        },
        Row {
            case: "an update naming providers of its own",
            name: "ReplacedProviders",
            created_with: Some(vec!["AWS"]),
            updated_with: Some(vec!["GCP", "Azure"]),
            expected: vec!["GCP", "Azure"],
        },
        Row {
            case: "an update narrowing the providers to one",
            name: "NarrowedProviders",
            created_with: Some(vec!["AWS", "Azure", "GCP"]),
            updated_with: Some(vec!["AWS"]),
            expected: vec!["AWS"],
        },
    ];

    let router = router();
    for row in &rows {
        let (case, name) = (row.case, row.name);
        let first: Vec<String> = row
            .created_with
            .clone()
            .map(|named| named.iter().map(|value| (*value).to_string()).collect())
            .unwrap_or_else(|| vec![DEFAULT_PROVIDER.to_string()]);
        let expected: Vec<String> = row
            .expected
            .iter()
            .map(|value| (*value).to_string())
            .collect();

        let mut body = json!({ "Name": name, "Content": content() });
        if let Some(named) = &row.created_with {
            body["CloudProviders"] = json!(named);
        }
        create_ok(&router, case, &body).await;

        let mut update = json!({ "Name": name, "Content": other_content() });
        if let Some(named) = &row.updated_with {
            update["CloudProviders"] = json!(named);
        }
        let (status, response) = call(&router, "UpdateDocument", &update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateDocument must succeed, got {status} {response}"
        );
        let appended = response["DocumentDescription"].clone();
        assert_eq!(
            providers(&appended),
            expected,
            "case {case:?}: expected the appended version to report {expected:?}, got {appended}"
        );
        // The version already held keeps its own providers: the member is per version.
        let held = describe_ok(
            &router,
            case,
            &json!({ "Name": name, "DocumentVersion": "1" }),
        )
        .await;
        assert_eq!(
            providers(&held),
            first,
            "case {case:?}: expected version 1 to keep {first:?}, got {held}"
        );
        assert_eq!(
            version_providers(&router, case, name, "1").await,
            first,
            "case {case:?}: the version listing reports version 1's own providers"
        );
        assert_eq!(
            version_providers(&router, case, name, "2").await,
            expected,
            "case {case:?}: the version listing reports version 2's own providers"
        );
    }
}

/// RULE 3: the list is bounded and enumerated as the model states, so a create outside
/// either is refused and creates nothing.
#[tokio::test]
async fn a_cloud_provider_list_outside_the_model_is_refused() {
    let router = router();
    // A list at the published ceiling, which the enum's three values fill by repetition, and
    // one past it.
    let at_ceiling: Vec<&str> = (0..PROVIDERS_MAX)
        .map(|nth| ["AWS", "Azure", "GCP"][nth % 3])
        .collect();
    let past_ceiling: Vec<&str> = (0..=PROVIDERS_MAX)
        .map(|nth| ["AWS", "Azure", "GCP"][nth % 3])
        .collect();

    // (case, the name, the list the create carries, whether the create is served)
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "a list at the published floor",
            "ProvidersAtFloor",
            json!(["Azure"]),
            true,
        ),
        (
            "a list at the published ceiling",
            "ProvidersAtCeiling",
            json!(at_ceiling),
            true,
        ),
        (
            "a list one past the ceiling",
            "ProvidersPastCeiling",
            json!(past_ceiling),
            false,
        ),
        (
            "a list below the floor",
            "ProvidersBelowFloor",
            json!([]),
            false,
        ),
        (
            "a provider outside the enum",
            "ProvidersUnknown",
            json!(["OracleCloud"]),
            false,
        ),
        (
            "a provider in the wrong case",
            "ProvidersMiscased",
            json!(["azure"]),
            false,
        ),
    ];

    assert_eq!(
        at_ceiling.len(),
        PROVIDERS_MAX,
        "the ceiling row must hold exactly {PROVIDERS_MAX} providers"
    );
    assert_eq!(
        PROVIDERS_MIN, 1,
        "the floor row must hold exactly {PROVIDERS_MIN} provider"
    );

    for (case, name, list, served) in &rows {
        let (status, response) = create(
            &router,
            &json!({ "Name": name, "Content": content(), "CloudProviders": list }),
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: the create must be served, got {status} {response}"
            );
            let created = response["DocumentDescription"].clone();
            let expected: Vec<String> = list
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                providers(&created),
                expected,
                "case {case:?}: expected {expected:?}, got {created}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {response}"
        );
        // Nothing was created: the name still names no document.
        let (status, response) = call(&router, "DescribeDocument", &json!({ "Name": name })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocument"),
            "case {case:?}: a refused create must create nothing, got {status} {response}"
        );
    }
}
