// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DataZone asset lifecycle (`CreateAsset`/`GetAsset`).
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateAsset` mints an asset under an existing, `AVAILABLE` domain and an
//!    existing project in that domain, persists every member the request named, and
//!    `GetAsset` reports the stored record back.
//! 2. A created asset is its own first revision, created by the CALLER, and carries the
//!    metadata forms the request listed, in order.
//! 3. A member the request left out is filled from the rule the model states
//!    (`typeRevision` defaults to the type's first revision) and nothing else is
//!    invented.
//! 4. The preconditions are refused with the operation's own modelled errors: a missing
//!    domain or project with `ResourceNotFoundException`, an unsettled domain or a
//!    reused `externalIdentifier` with `ConflictException`, a request naming one form
//!    name twice with `ValidationException`.
//! 5. A repeated `clientToken` returns the asset the first call created instead of a
//!    second one, and a read changes nothing a later caller sees.
//! 6. Every metadata form an asset operation reports carries how the form renders, and
//!    the read-only forms are reported as a list — empty for an asset that has none.

use crate::harness::Emulator;
use serde_json::{json, Value};

/// `AssetId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query shape="AssetId").
fn is_asset_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One `CreateAsset` request. The rows differ along what the rule generalises over:
/// which optional members are present, how many forms are attached (the modelled
/// `FormInputList` is 0..10), how many glossary terms, and whether the request pins a
/// `typeRevision`.
struct AssetCase {
    label: &'static str,
    /// The request body, minus `owningProjectIdentifier`, which the test fills with the
    /// project it created.
    body: Value,
    /// What `typeRevision` must come back as: the one the request pinned, or the
    /// asset type's first revision when it pinned none.
    expect_type_revision: &'static str,
}

fn asset_cases() -> Vec<AssetCase> {
    vec![
        AssetCase {
            label: "required members only, no forms",
            body: json!({
                "name": "minimal-asset",
                // A type in the `amazon.datazone.` namespace is one the service defines
                // itself (the prefix the model's `FormTypeName` pattern reserves,
                // smithy_query shape="FormTypeName").
                "typeIdentifier": "amazon.datazone.RelationalTable",
            }),
            expect_type_revision: "1",
        },
        AssetCase {
            label: "description, one form and an external identifier",
            body: json!({
                "name": "orders table",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "description": "the orders fact table",
                "externalIdentifier": "arn:aws:glue:us-west-2:123456789012:table/sales/orders",
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": "{\"columns\":[{\"columnName\":\"order_id\",\"dataType\":\"bigint\"}]}",
                }],
            }),
            expect_type_revision: "1",
        },
        AssetCase {
            label: "pinned type revision, three forms, glossary terms and a prediction configuration",
            body: json!({
                "name": "customers.v2",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "typeRevision": "7",
                "glossaryTerms": ["term0000000000000001", "term0000000000000002"],
                "predictionConfiguration": { "businessNameGeneration": { "enabled": true } },
                "formsInput": [
                    {
                        "formName": "RelationalTableForm",
                        "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                        "typeRevision": "3",
                        "content": "{\"columns\":[]}",
                    },
                    { "formName": "_private_notes", "content": "no content type" },
                    {
                        "formName": "OwnershipForm",
                        "typeIdentifier": "CustomOwnership",
                        "content": "{\"owner\":\"data-eng\"}",
                    },
                ],
            }),
            expect_type_revision: "7",
        },
        AssetCase {
            label: "an empty form list and a long description",
            body: json!({
                "name": "a".repeat(256),
                "typeIdentifier": "amazon.datazone.GlueTableAssetType",
                "description": "d".repeat(2048),
                "formsInput": [],
            }),
            expect_type_revision: "1",
        },
        AssetCase {
            // `PredictionConfiguration` and its `businessNameGeneration` are both
            // optional structures (smithy_query shape="PredictionConfiguration"), so a
            // present structure that sets no member must round-trip as present.
            label: "a prediction configuration whose businessNameGeneration sets nothing",
            body: json!({
                "name": "empty-prediction-configuration",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "predictionConfiguration": { "businessNameGeneration": {} },
                "formsInput": [{ "formName": "OnlyAName" }],
            }),
            expect_type_revision: "1",
        },
    ]
}

/// One metadata form as the wire carries it, so the forms a request asked for and the
/// forms a response reports compare directly. Shared with the `asset_revisions` suite,
/// which compares the same two shapes for a revision of an asset.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReportedForm {
    form_name: String,
    type_name: Option<String>,
    type_revision: Option<String>,
    content: Option<String>,
}

/// A member of `form` as an owned string, when it is there.
fn member(form: &Value, name: &str) -> Option<String> {
    form.get(name).and_then(Value::as_str).map(str::to_string)
}

/// The forms a case's request asked for, with the defaults the service applies: a form
/// that names a type but no revision of it is bound to that type's first revision, and
/// a form that names no type has no type revision either.
pub(crate) fn expected_forms(body: &Value) -> Vec<ReportedForm> {
    body.get("formsInput")
        .and_then(Value::as_array)
        .map(|forms| {
            forms
                .iter()
                .map(|form| {
                    let type_name = member(form, "typeIdentifier");
                    ReportedForm {
                        form_name: member(form, "formName").unwrap_or_default(),
                        type_revision: member(form, "typeRevision")
                            .or_else(|| type_name.as_ref().map(|_| "1".to_string())),
                        type_name,
                        content: member(form, "content"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `formsOutput` a response reported.
pub(crate) fn reported_forms(body: &Value) -> Vec<ReportedForm> {
    body.get("formsOutput")
        .and_then(Value::as_array)
        .map(|forms| {
            forms
                .iter()
                .map(|form| ReportedForm {
                    form_name: member(form, "formName").unwrap_or_default(),
                    type_name: member(form, "typeName"),
                    type_revision: member(form, "typeRevision"),
                    content: member(form, "content"),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn created_assets_are_persisted_and_reported_by_get_asset() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-domain").await;
    let project = dz.project(&domain, "asset owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    let mut created: Vec<(&'static str, String, Value)> = Vec::new();
    for case in asset_cases() {
        let mut body = case.body.clone();
        body["owningProjectIdentifier"] = json!(project);
        let answer = dz.call("POST", &assets_uri, Some(body.clone())).await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, answer.status, answer.body
        );
        let id = answer.s("id");
        assert!(
            is_asset_id(&id),
            "case {:?}: expected an id matching ^[a-zA-Z0-9_-]{{1,36}}$, got {id:?}",
            case.label
        );
        // A created asset IS the asset's first revision, and it belongs to the domain
        // and project the request named.
        assert_eq!(
            (
                answer.s("revision").as_str(),
                answer.s("domainId").as_str(),
                answer.s("owningProjectId").as_str()
            ),
            ("1", domain.as_str(), project.as_str()),
            "case {:?}: expected the first revision of an asset in {domain}/{project}, got {:?}",
            case.label,
            answer.body
        );
        assert_eq!(
            answer.s("typeRevision"),
            case.expect_type_revision,
            "case {:?}: unexpected typeRevision",
            case.label
        );
        // `createdBy` is the CALLER's DataZone user, not anything the body carried, and
        // the first revision was made by the same caller at the same instant.
        assert_eq!(
            answer.s("createdBy"),
            service_com_amazonaws_datazone_datazone::state::CALLER_USER_PROFILE_ID,
            "case {:?}: createdBy must name the calling user",
            case.label
        );
        assert_eq!(
            (
                answer.s("firstRevisionCreatedBy"),
                answer.body.get("firstRevisionCreatedAt").cloned()
            ),
            (
                answer.s("createdBy"),
                answer.body.get("createdAt").cloned()
            ),
            "case {:?}: the first revision of a new asset is the creation itself",
            case.label
        );
        assert!(
            answer.body.get("createdAt").is_some(),
            "case {:?}: expected a createdAt, got {:?}",
            case.label,
            answer.body
        );
        // Nothing is published, imported or governed yet, so none of those members is
        // invented for a freshly created asset.
        for absent in [
            "listing",
            "governedGlossaryTerms",
            "latestTimeSeriesDataPointFormsOutput",
        ] {
            assert!(
                answer.body.get(absent).is_none(),
                "case {:?}: expected no {absent} on a new asset, got {:?}",
                case.label,
                answer.body.get(absent)
            );
        }
        // The prediction configuration is the one the request set, and none is
        // invented for a request that set none.
        assert_eq!(
            answer.body.get("predictionConfiguration").cloned(),
            case.body.get("predictionConfiguration").cloned(),
            "case {:?}: unexpected predictionConfiguration {:?}",
            case.label,
            answer.body.get("predictionConfiguration")
        );
        // The create response already reports the forms it stored, in the order the
        // request listed them.
        assert_eq!(
            reported_forms(&answer.body),
            expected_forms(&case.body),
            "case {:?}: unexpected formsOutput {:?}",
            case.label,
            answer.body.get("formsOutput")
        );
        created.push((case.label, id, body));
    }

    // Distinct creates never share an id.
    for (i, (label_a, id_a, _)) in created.iter().enumerate() {
        for (label_b, id_b, _) in created.iter().skip(i + 1) {
            assert_ne!(
                id_a, id_b,
                "cases {label_a:?} and {label_b:?}: two assets were given the same id {id_a:?}"
            );
        }
    }

    // GetAsset reports the stored record for every case.
    for (label, id, request) in &created {
        let got = dz.call("GET", &format!("{assets_uri}/{id}"), None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: expected 200 from GetAsset, got {} {:?}",
            got.status, got.body
        );
        for member in ["name", "description", "externalIdentifier", "typeIdentifier"] {
            assert_eq!(
                request.get(member).and_then(Value::as_str),
                got.body.get(member).and_then(Value::as_str),
                "case {label:?}: GetAsset must report the persisted {member}, got {:?}",
                got.body.get(member)
            );
        }
        assert_eq!(
            got.body.get("glossaryTerms").cloned(),
            request.get("glossaryTerms").cloned(),
            "case {label:?}: GetAsset must report the persisted glossaryTerms, got {:?}",
            got.body.get("glossaryTerms")
        );
        assert_eq!(
            (got.s("id"), got.s("revision"), got.s("owningProjectId")),
            (id.clone(), "1".to_string(), project.clone()),
            "case {label:?}: GetAsset must identify the stored asset"
        );
        // The forms come back in the order the request listed them, with the type
        // revision default applied.
        assert_eq!(
            reported_forms(&got.body),
            expected_forms(request),
            "case {label:?}: GetAsset must report the persisted formsOutput, got {:?}",
            got.body.get("formsOutput")
        );

        // A read changes nothing a later caller sees.
        let again = dz.call("GET", &format!("{assets_uri}/{id}"), None).await;
        assert_eq!(
            again.body, got.body,
            "case {label:?}: reading an asset twice must answer identically"
        );
        // The asset has exactly one revision, so any other revision names nothing.
        let wrong_revision = dz
            .call("GET", &format!("{assets_uri}/{id}?revision=9"), None)
            .await;
        assert_eq!(
            (wrong_revision.status, wrong_revision.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a revision the asset does not have must be 404, got {:?}",
            wrong_revision.body
        );
        let right_revision = dz
            .call("GET", &format!("{assets_uri}/{id}?revision=1"), None)
            .await;
        assert_eq!(
            right_revision.body, got.body,
            "case {label:?}: asking for the stored revision must answer identically"
        );
    }

    // An asset lives in its domain: deleting the domain takes it with it.
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    // Wait for the CONDITION the delete owes — the domain no longer being served — rather
    // than for a fixed time, so the read below is made once the domain really is gone.
    dz.await_domain_gone(&domain).await;
    let (_, orphan, _) = &created[0];
    // The domain is gone, so no permission is held on it any more and the read is
    // decided by authorization before any asset lookup: the asset is no longer served
    // under that scope, and the answer says "not allowed", not "not there".
    let gone = dz.call("GET", &format!("{assets_uri}/{orphan}"), None).await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (403, "AccessDeniedException"),
        "an asset must not outlive its domain, got {} {:?}",
        gone.status,
        gone.body
    );
    assert_eq!(
        gone.s("message"),
        "User is not permitted to perform operation: GetAsset",
        "the denial must name the operation only, got {:?}",
        gone.body
    );
}

#[tokio::test]
async fn an_asset_needs_a_settled_domain_and_a_project_that_owns_it() {
    let dz = Emulator::new();
    let domain = dz.available_domain("precondition-domain").await;
    let project = dz.project(&domain, "owner").await;

    /// Each row: the label, the domain the request addresses, the owning project it
    /// names, and the (status, error code) the service must answer with.
    struct Row {
        label: &'static str,
        domain: String,
        project: String,
        expect: (u16, &'static str),
    }

    // A second settled domain: a project is scoped to ITS domain, so the same project
    // id is not an owner over there. Created FIRST, because waiting for it to settle
    // would also settle the still-creating domain below.
    let other_domain = dz.available_domain("other-domain").await;

    // A third domain that is still CREATING: it cannot take an asset yet.
    let creating = dz
        .create_domain(json!({ "name": "still-creating" }))
        .await;
    assert_eq!(
        creating.status, 201,
        "creating the still-creating domain: got {} {:?}",
        creating.status, creating.body
    );
    let creating_domain = creating.s("id");

    let rows = vec![
        Row {
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // create before any lookup.
            label: "a domain that was never created",
            domain: "dzd_0000000000".to_string(),
            project: project.clone(),
            expect: (403, "AccessDeniedException"),
        },
        Row {
            label: "a domain that is still CREATING",
            domain: creating_domain.clone(),
            project: project.clone(),
            expect: (409, "ConflictException"),
        },
        Row {
            label: "a project that does not exist in the domain",
            domain: domain.clone(),
            project: "no-such-project".to_string(),
            expect: (404, "ResourceNotFoundException"),
        },
        Row {
            label: "a project that exists in ANOTHER domain",
            domain: other_domain.clone(),
            project: project.clone(),
            expect: (404, "ResourceNotFoundException"),
        },
        Row {
            label: "both the domain and the project in place",
            domain: domain.clone(),
            project: project.clone(),
            expect: (201, ""),
        },
    ];

    for row in rows {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{}/assets", row.domain),
                Some(json!({
                    "name": "precondition-asset",
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": row.project,
                })),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            row.expect,
            "case {:?}: unexpected answer {:?}",
            row.label,
            answer.body
        );
    }

    // And a get for an asset id nothing created is a 404, not an empty success.
    let missing = dz
        .call("GET", &format!("/v2/domains/{domain}/assets/nothing"), None)
        .await;
    assert_eq!(
        (missing.status, missing.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "an unknown asset id must be 404, got {} {:?}",
        missing.status,
        missing.body
    );
}

#[tokio::test]
async fn a_reused_external_identifier_or_a_repeated_form_name_is_refused() {
    let dz = Emulator::new();
    let domain = dz.available_domain("uniqueness-domain").await;
    let project = dz.project(&domain, "owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    // `externalIdentifier` "must be a unique value": the second asset claiming one
    // conflicts with the first, whatever else differs between the two requests.
    for external in [
        "arn:aws:s3:::bucket/prefix",
        "glue://sales/orders",
        "x",
        &"e".repeat(600),
    ] {
        let first = dz
            .call(
                "POST",
                &assets_uri,
                Some(json!({
                    "name": "first",
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                    "externalIdentifier": external,
                })),
            )
            .await;
        assert_eq!(
            first.status, 201,
            "case {external:?}: expected the first claim to succeed, got {} {:?}",
            first.status, first.body
        );
        let second = dz
            .call(
                "POST",
                &assets_uri,
                Some(json!({
                    "name": "second",
                    "typeIdentifier": "amazon.datazone.GlueTableAssetType",
                    "owningProjectIdentifier": project,
                    "externalIdentifier": external,
                    "description": "a different request, the same external identifier",
                })),
            )
            .await;
        assert_eq!(
            (second.status, second.error_code.as_str()),
            (409, "ConflictException"),
            "case {external:?}: a reused externalIdentifier must conflict, got {:?}",
            second.body
        );
    }

    // Two assets with NO external identifier never conflict with each other.
    for name in ["no-external-a", "no-external-b"] {
        let answer = dz
            .call(
                "POST",
                &assets_uri,
                Some(json!({
                    "name": name,
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case {name:?}: an asset without an externalIdentifier must be accepted, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A form is identified by its name, so one request cannot name a form twice.
    for forms in [
        json!([{ "formName": "Dup" }, { "formName": "Dup" }]),
        json!([
            { "formName": "A", "content": "1" },
            { "formName": "B", "content": "2" },
            { "formName": "A", "content": "3" },
        ]),
    ] {
        let answer = dz
            .call(
                "POST",
                &assets_uri,
                Some(json!({
                    "name": "duplicate-forms",
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                    "formsInput": forms,
                })),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {forms:?}: a repeated formName must be refused, got {:?}",
            answer.body
        );
    }

    // Distinct form names in one request are fine.
    let accepted = dz
        .call(
            "POST",
            &assets_uri,
            Some(json!({
                "name": "distinct-forms",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
                "formsInput": [{ "formName": "A" }, { "formName": "B" }],
            })),
        )
        .await;
    assert_eq!(
        accepted.status, 201,
        "distinct form names must be accepted, got {} {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn an_asset_takes_the_revision_of_the_asset_type_it_names() {
    let dz = Emulator::new();
    let domain = dz.available_domain("type-revision-domain").await;
    let project = dz.project(&domain, "owner").await;

    // A custom asset type, created through the operation that owns that store.
    let asset_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/asset-types"),
            Some(json!({
                "name": "SalesTable",
                "owningProjectIdentifier": project,
                "formsInput": {
                    "SalesForm": {
                        "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                        "typeRevision": "1",
                    },
                },
            })),
        )
        .await;
    assert_eq!(
        asset_type.status, 201,
        "creating the asset type: expected 201, got {} {:?}",
        asset_type.status, asset_type.body
    );
    let stored_revision = asset_type.s("revision");

    // Each row: the type the asset names, the `typeRevision` it pins (if any) and
    // either the revision the asset must report, or the refusal the pin earns.
    enum Expect {
        Revision(String),
        Refused(u16, &'static str),
    }
    let rows = vec![
        (
            "a stored type, no pinned revision",
            "SalesTable",
            None,
            Expect::Revision(stored_revision.clone()),
        ),
        (
            "a stored type, pinning the revision it is at",
            "SalesTable",
            Some(stored_revision.clone()),
            Expect::Revision(stored_revision.clone()),
        ),
        (
            "a stored type, pinning a revision it does not have",
            "SalesTable",
            Some("4".to_string()),
            Expect::Refused(400, "ValidationException"),
        ),
        (
            "a service-defined type no store holds",
            "amazon.datazone.RelationalTable",
            None,
            Expect::Revision("1".to_string()),
        ),
        (
            "a service-defined type with a pinned revision, which no store can check",
            "amazon.datazone.RelationalTable",
            Some("3".to_string()),
            Expect::Revision("3".to_string()),
        ),
    ];

    for (label, type_identifier, pinned, expect) in rows {
        let mut body = json!({
            "name": format!("asset for {label}"),
            "typeIdentifier": type_identifier,
            "owningProjectIdentifier": project,
        });
        if let Some(revision) = &pinned {
            body["typeRevision"] = json!(revision);
        }
        let created = dz
            .call("POST", &format!("/v2/domains/{domain}/assets"), Some(body))
            .await;
        let expected = match expect {
            Expect::Refused(status, code) => {
                assert_eq!(
                    (created.status, created.error_code.as_str()),
                    (status, code),
                    "case {label:?}: unexpected answer {:?}",
                    created.body
                );
                continue;
            }
            Expect::Revision(revision) => revision,
        };
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.s("typeRevision"),
            expected,
            "case {label:?}: unexpected typeRevision"
        );
        // And the stored asset reports the same revision on the way back out.
        let got = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/assets/{}", created.s("id")),
                None,
            )
            .await;
        assert_eq!(
            got.s("typeRevision"),
            expected,
            "case {label:?}: GetAsset must report the stored typeRevision"
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_asset() {
    let dz = Emulator::new();
    let domain = dz.available_domain("idempotency-domain").await;
    let project = dz.project(&domain, "owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    for token in ["token-1", "A!~", &"t".repeat(128)] {
        let request = json!({
            "name": "idempotent-asset",
            "typeIdentifier": "amazon.datazone.RelationalTable",
            "owningProjectIdentifier": project,
            "clientToken": token,
        });
        let first = dz.call("POST", &assets_uri, Some(request.clone())).await;
        assert_eq!(
            first.status, 201,
            "case {token:?}: expected 201, got {} {:?}",
            first.status, first.body
        );
        let repeat = dz.call("POST", &assets_uri, Some(request)).await;
        assert_eq!(
            repeat.status, 201,
            "case {token:?}: a retry must succeed, got {} {:?}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.body, first.body,
            "case {token:?}: a retry must answer with the asset the first call created"
        );

        // A DIFFERENT token creates a second asset.
        let other = dz
            .call(
                "POST",
                &assets_uri,
                Some(json!({
                    "name": "idempotent-asset",
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                    "clientToken": format!("{token}-other"),
                })),
            )
            .await;
        assert_ne!(
            other.s("id"),
            first.s("id"),
            "case {token:?}: a different clientToken must create a different asset"
        );
    }
}

/// One asset whose metadata forms are read back from every operation that reports them.
/// The rows differ along how many forms the asset carries and along each form's shape:
/// whether it names a form type, a type revision, or content at all.
struct FormReportCase {
    label: &'static str,
    /// The `formsInput` entries, as the request lists them.
    forms: Value,
    /// How many form outputs the responses must carry.
    count: usize,
}

fn form_report_cases() -> Vec<FormReportCase> {
    vec![
        FormReportCase {
            label: "an asset carrying no form",
            forms: json!([]),
            count: 0,
        },
        FormReportCase {
            label: "one form naming a type and JSON content",
            forms: json!([{
                "formName": "RelationalTableForm",
                "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                "content": "{\"columns\":[]}",
            }]),
            count: 1,
        },
        FormReportCase {
            label: "three forms: a pinned type revision, a form with no type, a form with no content",
            forms: json!([
                {
                    "formName": "Pinned",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "typeRevision": "3",
                    "content": "{}",
                },
                { "formName": "_untyped", "content": "not json" },
                { "formName": "NoContent", "typeIdentifier": "CustomOwnership" },
            ]),
            count: 3,
        },
    ]
}

/// The form outputs the responses must carry for `forms`, the `formsInput` entries a
/// request listed: a form is reported with exactly the members `FormOutput` declares —
/// `formName`/`typeName`/`typeRevision`/`content` (smithy_query shape="FormOutput") —
/// projected from what the request set. A form that names a form type but pins no
/// revision is bound to that type's FIRST revision; one that names no type reports
/// neither a type name nor a type revision.
fn expected_form_outputs(forms: &Value) -> Vec<Value> {
    forms
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|form| {
            let mut expected = serde_json::Map::new();
            expected.insert("formName".to_string(), form["formName"].clone());
            if let Some(type_name) = form.get("typeIdentifier") {
                expected.insert("typeName".to_string(), type_name.clone());
                expected.insert(
                    "typeRevision".to_string(),
                    form.get("typeRevision").cloned().unwrap_or(json!("1")),
                );
            }
            if let Some(content) = form.get("content") {
                expected.insert("content".to_string(), content.clone());
            }
            Value::Object(expected)
        })
        .collect()
}

/// Assert the rule on one answer: `formsOutput` reports every form the request attached,
/// each with the members `FormOutput` declares and nothing else, and
/// `readOnlyFormsOutput` is a list — empty for an asset that has no read-only form, never
/// a missing member.
fn assert_form_members(body: &Value, case: &FormReportCase, operation: &str) {
    let expected = expected_form_outputs(&case.forms);
    let forms = body
        .get("formsOutput")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        forms.len(),
        case.count,
        "case {:?} ({operation}): expected {} form outputs, got {:?}",
        case.label,
        case.count,
        body.get("formsOutput")
    );
    assert_eq!(
        forms,
        expected,
        "case {:?} ({operation}): every form output carries exactly the modelled members, got {:?}",
        case.label,
        body.get("formsOutput")
    );
    assert_eq!(
        body.get("readOnlyFormsOutput"),
        Some(&json!([])),
        "case {:?} ({operation}): readOnlyFormsOutput must be an empty list, got {:?}",
        case.label,
        body.get("readOnlyFormsOutput")
    );
}

#[tokio::test]
async fn every_reported_form_carries_the_modelled_members_beside_an_empty_read_only_list() {
    let dz = Emulator::new();
    let domain = dz.available_domain("form-member-domain").await;
    let project = dz.project(&domain, "form member owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    for case in form_report_cases() {
        let created = dz
            .call(
                "POST",
                &assets_uri,
                Some(json!({
                    "name": format!("form-member asset: {}", case.label),
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                    "formsInput": case.forms,
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateAsset expected 201, got {} {:?}",
            case.label, created.status, created.body
        );
        assert_form_members(&created.body, &case, "CreateAsset");

        let id = created.s("id");
        let read = dz.call("GET", &format!("{assets_uri}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: GetAsset expected 200, got {} {:?}",
            case.label, read.status, read.body
        );
        assert_form_members(&read.body, &case, "GetAsset");

        // The same rule holds for the next revision of the same asset, which reports
        // the forms it carries through the same projection.
        let revised = dz
            .call(
                "POST",
                &format!("{assets_uri}/{id}/revisions"),
                Some(json!({
                    "name": format!("form-member asset revised: {}", case.label),
                    "formsInput": case.forms,
                })),
            )
            .await;
        assert_eq!(
            revised.status, 200,
            "case {:?}: CreateAssetRevision expected 200, got {} {:?}",
            case.label, revised.status, revised.body
        );
        assert_form_members(&revised.body, &case, "CreateAssetRevision");
    }
}
