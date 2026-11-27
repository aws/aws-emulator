// SPDX-License-Identifier: CC0-1.0

//! `visibility`, the one member of the report projections this crate declares itself, end
//! to end over HTTP — and the local declaration checked against the upstream model.
//!
//! Every case goes through the real `MultiServiceRouter`, so each assertion is about what a
//! client would see: routing, restJson1 serialization, the handler and the persisted
//! catalog all run as they do in production.
//!
//! The rule under test, stated over the whole published catalog rather than over one
//! recorded response: EVERY projection of a stored report reports the visibility that
//! report's own record holds — `ListReports` and `ListReportVersions` on the summary,
//! `GetReportMetadata` on the detail — so the member is present on every read of every
//! version of every report, carrying the one value the catalog holds (`PUBLIC`: what AWS
//! publishes in Artifact is visible to every account).
//!
//! The member is absent from the upstream model and is declared by this crate from the
//! overlay `smithy-model/local-additions.json` (see `smithy-model/LOCAL-ADDITIONS.md` and
//! `src/ops/types/report_catalog_types.rs`), so the last test pins that declaration against
//! the model itself: everything the overlay restates must still BE what the model declares,
//! and the only member it may add is this one. A refreshed model that declares `visibility`
//! itself, drops a member the overlay restates, or changes a target, value set or
//! constraint fails there rather than silently narrowing what this service reports.

use serde_json::{Map, Value};

use crate::harness::{
    emulator, get_report_metadata, list_report_versions, list_reports, CATALOG,
};

/// The upstream model this crate is generated from, embedded at compile time so the local
/// declaration is checked against the model rather than against a copy of it.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-artifact-artifact.json"
));

/// The overlay model the local report declarations are generated from, embedded for the
/// same reason: what it restates of the upstream model must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The two shapes the overlay restates with the member the model lacks.
const PROJECTION_SHAPES: &[&str] = &[
    "com.amazonaws.artifact#ReportSummary",
    "com.amazonaws.artifact#ReportDetail",
];

/// The shape the overlay adds for that member, which the model declares nowhere.
const VISIBILITY_SHAPE: &str = "com.amazonaws.artifact#Visibility";

/// The member this crate declares locally, which the model does not.
const LOCAL_MEMBER: &str = "visibility";

/// The visibility a report AWS publishes in the Artifact catalog carries. Every row of the
/// catalog this emulator serves is published, so every projection reports this value.
const PUBLIC: &str = "PUBLIC";

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    Value::Object(
        parsed["shapes"]
            .as_object()
            .expect("a model document must carry a shapes object")
            .clone(),
    )
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the model documents and nothing else
/// compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait
/// that carries no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

/// Assert that one projected entry reports the member, as a string carrying `PUBLIC`.
fn assert_visibility(case: &str, projection: &Value) {
    let reported = projection.get(LOCAL_MEMBER).unwrap_or(&Value::Null);
    assert_eq!(
        reported.as_str(),
        Some(PUBLIC),
        "case {case:?}: every projection of a published report reports \
         {LOCAL_MEMBER}={PUBLIC:?}, got {projection:?}"
    );
}

/// Every projection of every report in the catalog reports the visibility its record holds,
/// on every read that projects a report.
///
/// The rows differ along what the rule generalises over: the operation (the two listings and
/// the per-report metadata read), the report (every row of the catalog, across both of the
/// classes it holds), and the version (every published version of each, not only the latest).
#[tokio::test]
async fn every_report_read_reports_the_visibility_the_catalog_record_holds() {
    let router = emulator();

    // `ListReports` projects the latest version of every report.
    let listed = list_reports(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "ListReports must answer, got {} with {:?}",
        listed.status, listed.body
    );
    let entries = listed.body["reports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListReports must report a list, got {:?}", listed.body));
    assert_eq!(
        entries.len(),
        CATALOG.len(),
        "ListReports must project the whole catalog, got {:?}",
        listed.body
    );
    for entry in entries {
        let id = entry["id"].as_str().unwrap_or_default();
        assert_visibility(&format!("ListReports {id}"), entry);
    }

    for report in CATALOG {
        // `ListReportVersions` projects every published version of one report.
        let versions =
            list_report_versions(&router, &format!("?reportId={}", report.id)).await;
        assert_eq!(
            versions.status, 200,
            "case {:?}: ListReportVersions must answer, got {} with {:?}",
            report.id, versions.status, versions.body
        );
        let projected = versions.body["reports"].as_array().unwrap_or_else(|| {
            panic!(
                "case {:?}: ListReportVersions must report a list, got {:?}",
                report.id, versions.body
            )
        });
        assert_eq!(
            projected.len(),
            report.versions.len(),
            "case {:?}: every published version must be projected, got {:?}",
            report.id,
            versions.body
        );
        for entry in projected {
            let version = entry["version"].as_i64().unwrap_or_default();
            assert_visibility(
                &format!("ListReportVersions {} v{version}", report.id),
                entry,
            );
        }

        // And `GetReportMetadata` on each of those versions, plus the read that names none.
        for version in report.versions.iter().copied().chain([0]) {
            let (case, query) = if version == 0 {
                (
                    format!("GetReportMetadata {} latest", report.id),
                    format!("?reportId={}", report.id),
                )
            } else {
                (
                    format!("GetReportMetadata {} v{version}", report.id),
                    format!("?reportId={}&reportVersion={version}", report.id),
                )
            };
            let detail = get_report_metadata(&router, &query).await;
            assert_eq!(
                detail.status, 200,
                "case {case:?}: GetReportMetadata must answer, got {} with {:?}",
                detail.status, detail.body
            );
            assert_visibility(&case, &detail.body["reportDetails"]);
        }
    }
}

/// The local declaration against the model: the overlay restates the model exactly, and the
/// only member it adds is the one the model does not declare.
#[test]
fn the_overlay_restates_the_model_exactly_but_for_the_member_it_adds() {
    let model = model_shapes(MODEL);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        if shape_id == VISIBILITY_SHAPE {
            assert!(
                model[shape_id].is_null(),
                "the model now declares {shape_id}, so the local declaration should give \
                 way to the modelled shape"
            );
            continue;
        }
        let modelled = &model[shape_id];
        assert!(
            !modelled.is_null(),
            "the overlay restates {shape_id}, which the model no longer declares"
        );

        if PROJECTION_SHAPES.contains(&shape_id.as_str()) {
            // The shapes that differ on purpose: the model's members, restated as the model
            // declares them, plus the member AWS reports and it does not.
            let declared_members = declared["members"]
                .as_object()
                .unwrap_or_else(|| panic!("the overlay declares {shape_id}'s members"));
            let modelled_members = modelled["members"]
                .as_object()
                .unwrap_or_else(|| panic!("the model declares {shape_id}'s members"));
            for (member, modelled_member) in modelled_members {
                assert_eq!(
                    without_docs(&declared_members[member]),
                    without_docs(modelled_member),
                    "member {member} of {shape_id} must be restated as the model declares \
                     it, got {:?}",
                    declared_members.get(member)
                );
            }
            let added: Vec<&String> = declared_members
                .keys()
                .filter(|member| !modelled_members.contains_key(*member))
                .collect();
            assert_eq!(
                added,
                vec![LOCAL_MEMBER],
                "{shape_id} may add {LOCAL_MEMBER} and nothing else, added {added:?}"
            );
            continue;
        }

        assert_eq!(
            without_docs(declared),
            without_docs(modelled),
            "{shape_id} must be restated exactly as the model declares it"
        );
    }

    // Every shape the restated members target is itself restated: codegen reads the member's
    // target out of the one model file the declaration is generated from, so a target the
    // overlay does not carry would not resolve at all.
    for shape_id in PROJECTION_SHAPES {
        let members = overlay[*shape_id]["members"]
            .as_object()
            .unwrap_or_else(|| panic!("the overlay declares {shape_id}'s members"))
            .clone();
        for (member, declared) in &members {
            let target = declared["target"]
                .as_str()
                .unwrap_or_else(|| panic!("member {member} of {shape_id} names a target"));
            assert!(
                !overlay[target].is_null(),
                "the overlay must restate {target}, the target of {shape_id}${member}"
            );
        }
    }
}
