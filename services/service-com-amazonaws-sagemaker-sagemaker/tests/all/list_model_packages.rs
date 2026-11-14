// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListModelPackages`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.ListModelPackages`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what
//! a real SDK client gets. The harness and the create/describe helpers are reused from
//! `model_package_registry`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! member of the modelled `ModelPackageType` and `ModelApprovalStatus` catalogs, the
//! defaulting of `ModelPackageType` from whether a group is named, both members of
//! `ModelPackageSortBy` crossed with both of `SortOrder`, the name and creation-time
//! filters, page sizes from one entry to past both published `MaxResults` bounds, and the
//! refusals for an input outside a documented value set.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    MODEL_APPROVAL_STATUS_VALUES, MODEL_PACKAGE_TYPE_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state;
use service_com_amazonaws_sagemaker_sagemaker::state::MAX_PAGE_SIZE;

use crate::inference_deployment_lifecycle::CREATE_SPACING;
use crate::model_package_registry::{
    container, error_code, expected_arn, str_field, validation_specification, Emulator,
};

/// How long a poll below gives the clock to settle a registration: far longer than
/// `state::MODEL_PACKAGE_COMPLETED_AFTER`, so a loaded host still settles well inside it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often such a poll looks again. Only the loop's condition decides when it stops.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The unversioned packages the seeded store holds, created in THIS order. Their names
/// are deliberately not in creation order, so a `Name` sort and a `CreationTime` sort
/// cannot accidentally agree.
const SOLO: [&str; 3] = ["solo-charlie", "solo-alpha", "solo-bravo"];

/// The two model package groups the seeded store holds, with how many versions each gets.
const GROUPS: [(&str, i64); 2] = [("pkg-group-a", 3), ("pkg-group-b", 2)];

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail this test for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// The store key (and ARN resource part) a summary identifies: `<name>` for an
/// unversioned package, `<group>/<version>` for a version.
fn summary_id(summary: &Value) -> String {
    let name = str_field(summary, "ModelPackageName");
    match summary.get("ModelPackageVersion").and_then(Value::as_i64) {
        Some(version) => format!("{name}/{version}"),
        None => name,
    }
}

/// The summaries a `ListModelPackages` response carries, in response order.
fn summaries(body: &Value) -> Vec<Value> {
    body.get("ModelPackageSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ModelPackageSummaryList in {body}"))
        .clone()
}

/// The listed packages' identities, in response order.
fn listed_ids(body: &Value) -> Vec<String> {
    summaries(body).iter().map(summary_id).collect()
}

/// Seed [`SOLO`] and [`GROUPS`], spaced so `CreationTime` ordering is unambiguous, then
/// wait past the `Completed` instant the clock owes so the assertions are not racing it.
///
/// Answers every seeded package's identity in CREATION order.
async fn seed(emulator: &Emulator) -> Vec<String> {
    let mut created = Vec::new();
    for name in SOLO {
        emulator
            .create(
                name,
                &json!({
                    "ModelPackageName": name,
                    "InferenceSpecification": {
                        "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/a:1")],
                        "SupportedContentTypes": ["application/json"],
                        "SupportedResponseMIMETypes": ["application/json"],
                    },
                }),
            )
            .await;
        created.push(name.to_string());
        tokio::time::sleep(CREATE_SPACING).await;
    }
    for (group, versions) in GROUPS {
        emulator.create_group(state::DEFAULT_REGION, group).await;
        for version in 1..=versions {
            emulator
                .create(group, &json!({"ModelPackageGroupName": group}))
                .await;
            created.push(format!("{group}/{version}"));
            tokio::time::sleep(CREATE_SPACING).await;
        }
    }
    created
}

/// Every seeded package, in `Name`-ascending order with the version as the tie-break.
fn by_name_ascending() -> Vec<String> {
    vec![
        "pkg-group-a/1".to_string(),
        "pkg-group-a/2".to_string(),
        "pkg-group-a/3".to_string(),
        "pkg-group-b/1".to_string(),
        "pkg-group-b/2".to_string(),
        "solo-alpha".to_string(),
        "solo-bravo".to_string(),
        "solo-charlie".to_string(),
    ]
}

/// RULE: `ModelPackageType` selects which KIND of registration is listed — `Unversioned`
/// only packages registered under a name, `Versioned` only the versions registered into a
/// group, `Both` either — and `ModelPackageGroupName`, which accepts the group's name or
/// its ARN, restricts the result to the versions belonging to that group.
#[tokio::test]
async fn list_model_packages_selects_on_the_package_type_and_the_group() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let group_a_arn = emulator
        .ok(
            "group-a arn",
            "DescribeModelPackageGroup",
            &json!({"ModelPackageGroupName": GROUPS[0].0}),
        )
        .await;
    let group_a_arn = str_field(&group_a_arn, "ModelPackageGroupArn");

    let versioned: Vec<String> = by_name_ascending()
        .into_iter()
        .filter(|id| id.contains('/'))
        .collect();
    let unversioned: Vec<String> = by_name_ascending()
        .into_iter()
        .filter(|id| !id.contains('/'))
        .collect();
    let group_a: Vec<String> = vec![
        "pkg-group-a/1".to_string(),
        "pkg-group-a/2".to_string(),
        "pkg-group-a/3".to_string(),
    ];

    // Rows differ along the kind filter (every member of the modelled `ModelPackageType`
    // catalog, plus its absence), whether a group is named, and how the group is named.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("type-Both", json!({"ModelPackageType": "Both"}), by_name_ascending()),
        (
            "type-Versioned",
            json!({"ModelPackageType": "Versioned"}),
            versioned.clone(),
        ),
        (
            "type-Unversioned",
            json!({"ModelPackageType": "Unversioned"}),
            unversioned.clone(),
        ),
        // No `ModelPackageType`: `Unversioned` is the member's documented default.
        ("no-type-no-group", json!({}), unversioned.clone()),
        // A group is named, so the filter its own doc describes ("returns only model
        // VERSIONS that belong to the specified model group") decides the kind.
        (
            "no-type-group-by-name",
            json!({"ModelPackageGroupName": GROUPS[0].0}),
            group_a.clone(),
        ),
        (
            "no-type-group-by-arn",
            json!({"ModelPackageGroupName": group_a_arn}),
            group_a.clone(),
        ),
        (
            "type-Versioned-and-group",
            json!({"ModelPackageType": "Versioned", "ModelPackageGroupName": GROUPS[1].0}),
            vec!["pkg-group-b/1".to_string(), "pkg-group-b/2".to_string()],
        ),
        // `Both` plus a group still lists only that group's members, and an unversioned
        // package belongs to no group.
        (
            "type-Both-and-group",
            json!({"ModelPackageType": "Both", "ModelPackageGroupName": GROUPS[0].0}),
            group_a.clone(),
        ),
        // An unversioned package has no group, so the two filters select nothing together.
        (
            "type-Unversioned-and-group",
            json!({"ModelPackageType": "Unversioned", "ModelPackageGroupName": GROUPS[0].0}),
            vec![],
        ),
        (
            "group-that-holds-no-version",
            json!({"ModelPackageGroupName": "no-such-group"}),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let mut body = body;
        body["SortBy"] = Value::String("Name".to_string());
        body["SortOrder"] = Value::String("Ascending".to_string());
        let listed = emulator.ok(case, "ListModelPackages", &body).await;
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case}: ListModelPackages selection, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // Every member of the generated catalog is accepted, and each selects a subset of
    // `Both`.
    for kind in MODEL_PACKAGE_TYPE_VALUES {
        let listed = emulator
            .ok(
                kind,
                "ListModelPackages",
                &json!({"ModelPackageType": kind, "MaxResults": 100}),
            )
            .await;
        let ids = listed_ids(&listed);
        let every = by_name_ascending();
        assert!(
            ids.iter().all(|id| every.contains(id)),
            "case {kind}: every listed package must be one of the seeded ones, got {ids:?}"
        );
        let expected = match *kind {
            "Both" => every.len(),
            "Versioned" => versioned.len(),
            _ => unversioned.len(),
        };
        assert_eq!(
            ids.len(),
            expected,
            "case {kind}: expected {expected} packages, got {ids:?}"
        );
    }
}

/// RULE: the result is ordered by `SortBy`/`SortOrder`, defaulting to this operation's
/// own documented defaults — `CreationTime` and `Ascending`, the latter differing from
/// the `Descending` that `ListModels`/`ListEndpoints` document — with
/// `(ModelPackageName, ModelPackageVersion)` as the ascending tie-break in both
/// directions, so the versions of one group keep one order whichever way the page is
/// sorted, and `NameContains` keeps only the packages whose name contains it.
#[tokio::test]
async fn list_model_packages_honours_every_documented_sort_and_the_name_filter() {
    let emulator = Emulator::new();
    let creation_order = seed(&emulator).await;
    let newest_first: Vec<String> = creation_order.iter().rev().cloned().collect();

    // Rows differ along sort field (each member of `ModelPackageSortBy`), sort order
    // (each member of `SortOrder`), the defaulting of both, and the name filter's
    // selectivity.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            // `ListModelPackagesInput$SortOrder`: "The sort order for the results. The
            // default is Ascending." — this operation's own default, which is NOT the
            // Descending one `ListModels` and `ListEndpoints` document.
            "documented-defaults-are-creation-time-ascending",
            json!({"ModelPackageType": "Both"}),
            creation_order.clone(),
        ),
        (
            "creation-time-ascending",
            json!({"ModelPackageType": "Both", "SortBy": "CreationTime", "SortOrder": "Ascending"}),
            creation_order.clone(),
        ),
        (
            "creation-time-descending",
            json!({
                "ModelPackageType": "Both",
                "SortBy": "CreationTime",
                "SortOrder": "Descending",
            }),
            newest_first.clone(),
        ),
        (
            "name-ascending",
            json!({"ModelPackageType": "Both", "SortBy": "Name", "SortOrder": "Ascending"}),
            by_name_ascending(),
        ),
        // Name descending reverses the NAME only: the versions inside a group keep their
        // ascending order.
        (
            "name-descending-keeps-the-ascending-version-tie-break",
            json!({"ModelPackageType": "Both", "SortBy": "Name", "SortOrder": "Descending"}),
            vec![
                "solo-charlie".to_string(),
                "solo-bravo".to_string(),
                "solo-alpha".to_string(),
                "pkg-group-b/1".to_string(),
                "pkg-group-b/2".to_string(),
                "pkg-group-a/1".to_string(),
                "pkg-group-a/2".to_string(),
                "pkg-group-a/3".to_string(),
            ],
        ),
        (
            "name-contains-the-group-prefix",
            json!({
                "ModelPackageType": "Both",
                "NameContains": "pkg-group",
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
            vec![
                "pkg-group-a/1".to_string(),
                "pkg-group-a/2".to_string(),
                "pkg-group-a/3".to_string(),
                "pkg-group-b/1".to_string(),
                "pkg-group-b/2".to_string(),
            ],
        ),
        (
            "name-contains-one-match",
            json!({"ModelPackageType": "Both", "NameContains": "alpha"}),
            vec!["solo-alpha".to_string()],
        ),
        (
            "name-contains-no-match",
            json!({"ModelPackageType": "Both", "NameContains": "nothing-here"}),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok(case, "ListModelPackages", &body).await;
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case}: ListModelPackages order, got {listed:?}"
        );
    }
}

/// RULE: `ModelApprovalStatus` returns exactly the model packages whose stored approval
/// status equals it, over every member of the modelled catalog
/// (`MODEL_APPROVAL_STATUS_VALUES`, i.e. `smithy_query(query="shape",
/// shape="ModelApprovalStatus")`). An unversioned package records no approval status —
/// the member "does not apply" to it — so no value of the filter selects one.
#[tokio::test]
async fn list_model_packages_filters_on_every_modelled_approval_status() {
    let emulator = Emulator::new();
    let group = "approval-group";
    emulator.create_group(state::DEFAULT_REGION, group).await;
    // One version per modelled approval status, in catalog order, so each value of the
    // filter has exactly one answer.
    for status in MODEL_APPROVAL_STATUS_VALUES {
        emulator
            .create(
                status,
                &json!({"ModelPackageGroupName": group, "ModelApprovalStatus": status}),
            )
            .await;
        tokio::time::sleep(CREATE_SPACING).await;
    }
    // An unversioned package beside them, which carries no approval status at all.
    emulator
        .create("solo", &json!({"ModelPackageName": "approval-solo"}))
        .await;

    for (index, status) in MODEL_APPROVAL_STATUS_VALUES.iter().enumerate() {
        let expected = vec![format!("{group}/{}", index + 1)];
        let listed = emulator
            .ok(
                status,
                "ListModelPackages",
                &json!({"ModelPackageType": "Both", "ModelApprovalStatus": status}),
            )
            .await;
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {status}: the approval filter must select only that status, got {listed:?}"
        );
        assert_eq!(
            summaries(&listed)
                .first()
                .map(|summary| str_field(summary, "ModelApprovalStatus")),
            Some((*status).to_string()),
            "case {status}: the summary must report the stored approval status, got {listed:?}"
        );

        // The same filter with the unversioned kind selects nothing: an unversioned
        // package has no approval status to match.
        let listed = emulator
            .ok(
                status,
                "ListModelPackages",
                &json!({"ModelPackageType": "Unversioned", "ModelApprovalStatus": status}),
            )
            .await;
        assert!(
            listed_ids(&listed).is_empty(),
            "case {status}: an unversioned package records no approval status, got {listed:?}"
        );
    }
}

/// RULE: `CreationTimeAfter`/`CreationTimeBefore` select exactly the packages on their
/// side of the instant and compose with each other and with the other filters.
#[tokio::test]
async fn list_model_packages_filters_on_the_creation_time_bounds() {
    let emulator = Emulator::new();
    let creation_order = seed(&emulator).await;

    // The stored instants, read back through the listing itself in creation order.
    let listed = emulator
        .ok(
            "instants",
            "ListModelPackages",
            &json!({
                "ModelPackageType": "Both",
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
        )
        .await;
    let instants: Vec<f64> = summaries(&listed)
        .iter()
        .map(|summary| {
            summary
                .get("CreationTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected a numeric CreationTime in {summary}"))
        })
        .collect();
    assert_eq!(
        instants.len(),
        creation_order.len(),
        "every seeded package must be listed, got {listed:?}"
    );

    // Bounds strictly between two neighbours, so each split is unambiguous.
    let split = (instants[2] + instants[3]) / 2.0;
    let upper = (instants[4] + instants[5]) / 2.0;

    // Rows differ along which bound is used, which side it selects, how many bounds
    // compose, whether a bound sits exactly on a stored instant, and what it composes
    // with.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "before-the-split",
            json!({"CreationTimeBefore": split}),
            creation_order[..3].to_vec(),
        ),
        (
            "after-the-split",
            json!({"CreationTimeAfter": split}),
            creation_order[3..].to_vec(),
        ),
        (
            "both-bounds",
            json!({"CreationTimeAfter": split, "CreationTimeBefore": upper}),
            creation_order[3..5].to_vec(),
        ),
        // A millisecond either side of an end instant: the finest split that is safe
        // through the wire's epoch-seconds float, and still well inside the 5 ms the
        // seeded packages are spaced by.
        (
            "bound-just-past-the-first-instant",
            json!({"CreationTimeBefore": instants[0] + 0.001}),
            creation_order[..1].to_vec(),
        ),
        (
            "bound-just-before-the-last-instant",
            json!({"CreationTimeAfter": instants[instants.len() - 1] - 0.001}),
            creation_order[creation_order.len() - 1..].to_vec(),
        ),
        (
            "bound-that-excludes-everything",
            json!({"CreationTimeAfter": instants[instants.len() - 1] + 3600.0}),
            vec![],
        ),
        (
            "bound-combined-with-the-group-filter",
            json!({"CreationTimeAfter": split, "ModelPackageGroupName": GROUPS[0].0}),
            vec!["pkg-group-a/1".to_string(), "pkg-group-a/2".to_string(), "pkg-group-a/3".to_string()],
        ),
        (
            "bound-combined-with-the-name-filter",
            json!({"CreationTimeBefore": split, "NameContains": "solo-alpha"}),
            vec!["solo-alpha".to_string()],
        ),
    ];

    for (case, body, expected) in cases {
        let mut body = body;
        body["ModelPackageType"] = Value::String("Both".to_string());
        body["SortBy"] = Value::String("CreationTime".to_string());
        body["SortOrder"] = Value::String("Ascending".to_string());
        let listed = emulator.ok(case, "ListModelPackages", &body).await;
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case}: ListModelPackages selection, got {listed:?}"
        );
    }
}

/// RULE: paging walks the whole ordered result exactly once — each page holds at most the
/// effective page size, a `NextToken` is present only while more remain, and feeding it
/// back resumes immediately after the previous page. The effective size is `MaxResults`
/// capped by the service's own ceiling, so no single call serialises the whole store
/// however large `MaxResults` is.
#[tokio::test]
async fn list_model_packages_paging_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    // Seeded past the server ceiling, so a capped page and an uncapped one are
    // distinguishable. The names sort the same way they are created, so the expected
    // order is the creation order.
    let names: Vec<String> = (0..PAGE_CEILING + 3)
        .map(|index| format!("page-{index:03}"))
        .collect();
    for name in &names {
        emulator
            .create(name, &json!({"ModelPackageName": name}))
            .await;
    }

    // Rows: every page size from one entry up to past the end of the result set, plus
    // both published bounds of `MaxResults` (`range 1..100`, from
    // smithy_query(query="operation",operation="ListModelPackages")).
    for page_size in [1_usize, 2, 7, PAGE_CEILING, names.len(), names.len() + 1, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "MaxResults": page_size,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok(&case, "ListModelPackages", &body).await;
            let page = listed_ids(&listed);
            let cap = page_size.min(PAGE_CEILING);
            assert!(
                page.len() <= cap,
                "case {case}: a page must hold at most {cap} entries, got {page:?}"
            );
            seen.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }
}

/// RULE: paging keeps the versions of one group apart even though they share a name — the
/// resume token names ONE package, so a one-entry page per version walks every version of
/// a group exactly once.
#[tokio::test]
async fn list_model_packages_pages_through_the_versions_of_one_group() {
    let emulator = Emulator::new();
    let group = "paging-group";
    emulator.create_group(state::DEFAULT_REGION, group).await;
    let versions = 5;
    for _ in 1..=versions {
        emulator
            .create(group, &json!({"ModelPackageGroupName": group}))
            .await;
    }

    let expected: Vec<String> = (1..=versions).map(|v| format!("{group}/{v}")).collect();
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "ModelPackageGroupName": group,
            "MaxResults": 1,
            "SortBy": "Name",
            "SortOrder": "Ascending",
        });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("one-at-a-time", "ListModelPackages", &body).await;
        let page = listed_ids(&listed);
        assert_eq!(
            page.len(),
            1,
            "case one-at-a-time: a one-entry page must hold one version, got {listed:?}"
        );
        seen.extend(page);
        token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, expected,
        "case one-at-a-time: paging must walk every version of the group exactly once"
    );
}

/// RULE: every `ModelPackageSummary` replays the stored package — the name it is listed
/// under, its group and version where it is a version, its minted ARN, its description,
/// the creation instant the service recorded, its status, its approval status and its
/// life cycle — and agrees member for member with `DescribeModelPackage`.
#[tokio::test]
async fn list_model_packages_summaries_replay_the_stored_package() {
    let emulator = Emulator::new();
    let group = "replay-group";
    emulator.create_group(state::DEFAULT_REGION, group).await;
    // Rows differ in kind (unversioned / versioned) and in which optional members the
    // registration carried.
    emulator
        .create(
            "unversioned",
            &json!({
                "ModelPackageName": "replay-solo",
                "ModelPackageDescription": "a solo package",
            }),
        )
        .await;
    emulator
        .create(
            "versioned",
            &json!({
                "ModelPackageGroupName": group,
                "ModelPackageDescription": "version one",
                "ModelApprovalStatus": "Approved",
                "ModelLifeCycle": {"Stage": "Development", "StageStatus": "InProgress"},
                "ModelPackageRegistrationType": "Registered",
            }),
        )
        .await;

    let listed = emulator
        .ok(
            "replay",
            "ListModelPackages",
            &json!({"ModelPackageType": "Both", "SortBy": "Name", "SortOrder": "Ascending"}),
        )
        .await;
    let found = summaries(&listed);
    assert_eq!(
        found.len(),
        2,
        "every stored package must be listed, got {listed:?}"
    );

    for summary in &found {
        let id = summary_id(summary);
        assert_eq!(
            str_field(summary, "ModelPackageArn"),
            expected_arn(state::DEFAULT_REGION, &id),
            "case {id}: the summary must carry the minted ARN, got {summary:?}"
        );
        let described = emulator.describe(&id, &id).await;
        for member in [
            "ModelPackageName",
            "ModelPackageGroupName",
            "ModelPackageVersion",
            "ModelPackageArn",
            "ModelPackageDescription",
            "CreationTime",
            "ModelPackageStatus",
            "ModelApprovalStatus",
            "ModelLifeCycle",
            "ModelPackageRegistrationType",
        ] {
            assert_eq!(
                summary.get(member),
                described.get(member),
                "case {id}: {member} must agree with DescribeModelPackage, got {summary:?} \
                 against {described:?}"
            );
        }
        assert_eq!(
            str_field(summary, "ModelPackageStatus"),
            "Completed",
            "case {id}: the settled status, got {summary:?}"
        );
    }

    // The versioned row reports its group and version; the unversioned row neither.
    // `replay-group` sorts before `replay-solo`, so the version comes first.
    let solo = &found[1];
    assert_eq!(
        summary_id(solo),
        "replay-solo",
        "the unversioned package sorts last by name, got {solo:?}"
    );
    assert!(
        solo.get("ModelPackageGroupName").is_none()
            && solo.get("ModelPackageVersion").is_none(),
        "an unversioned package belongs to no group and has no version, got {solo:?}"
    );
    let version = &found[0];
    assert_eq!(
        str_field(version, "ModelPackageGroupName"),
        group,
        "a version reports its group, got {version:?}"
    );
    assert_eq!(
        version.get("ModelPackageVersion").and_then(Value::as_i64),
        Some(1),
        "a version reports its number, got {version:?}"
    );
    // "The name of the model package", documented with no "if versioned" qualifier — so a
    // version is named here by its group, exactly as `DescribeModelPackage` names it.
    assert_eq!(
        str_field(version, "ModelPackageName"),
        group,
        "a version is named by its group, got {version:?}"
    );
}

/// RULE: the status the listing reports is the one the clock owes — `Pending` until the
/// registration's due instant and `Completed` from it — and it is the same status
/// `DescribeModelPackage` reports at that instant, stable across later reads.
#[tokio::test]
async fn list_model_packages_reports_the_status_the_clock_owes() {
    let emulator = Emulator::new();
    let group = "settling-group";
    emulator.create_group(state::DEFAULT_REGION, group).await;
    for _ in 0..3 {
        // Each registration names a validation profile, which is what leaves it
        // settling: a registration with nothing to validate is complete when it answers.
        emulator
            .create(
                group,
                &json!({
                    "ModelPackageGroupName": group,
                    "ValidationSpecification": validation_specification(1),
                }),
            )
            .await;
    }

    let statuses = |body: &Value| -> Vec<String> {
        summaries(body)
            .iter()
            .map(|summary| str_field(summary, "ModelPackageStatus"))
            .collect()
    };

    // Before the owed transition is due. Only one wire call is taken here, so the
    // pre-due assertion does not have to finish inside the transition's span.
    let listed = emulator
        .ok(
            "before-due",
            "ListModelPackages",
            &json!({"ModelPackageGroupName": group}),
        )
        .await;
    assert_eq!(
        statuses(&listed),
        vec!["Pending".to_string(); 3],
        "case before-due: a freshly registered package is reported Pending, got {listed:?}"
    );

    // Look again until every registration has settled, however long the host takes: what
    // is under test is that the listing reports the transition, not when it happens.
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    let listed = loop {
        let listed = emulator
            .ok(
                "past-due",
                "ListModelPackages",
                &json!({"ModelPackageGroupName": group}),
            )
            .await;
        if statuses(&listed) == vec!["Completed".to_string(); 3] {
            break listed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case past-due: expected every registration to settle within \
             {SETTLE_DEADLINE:?}, got {listed:?}"
        );
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    };
    assert_eq!(
        statuses(&listed),
        vec!["Completed".to_string(); 3],
        "case past-due: the owed transition must be materialised, got {listed:?}"
    );

    // The reader agrees, and the status is stable across a later listing.
    for version in 1..=3 {
        let id = format!("{group}/{version}");
        let described = emulator.describe(&id, &id).await;
        assert_eq!(
            str_field(&described, "ModelPackageStatus"),
            "Completed",
            "case {id}: DescribeModelPackage must agree with the listing, got {described:?}"
        );
    }
    let listed = emulator
        .ok(
            "stability",
            "ListModelPackages",
            &json!({"ModelPackageGroupName": group}),
        )
        .await;
    assert_eq!(
        statuses(&listed),
        vec!["Completed".to_string(); 3],
        "case stability: a settled status must not change on a later read, got {listed:?}"
    );
}

/// RULE: an input outside a documented value set is refused with HTTP 400
/// `ValidationException` rather than silently ignored, and the refusal writes nothing.
#[tokio::test]
async fn list_model_packages_refuses_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;

    // Rows: one outside each documented value set — `ModelPackageSortBy`, `SortOrder`,
    // `ModelApprovalStatus`, `ModelPackageType`, an unminted `NextToken`, `MaxResults`
    // one past each published bound (`range 1..100`), and `NameContains` outside
    // `^[a-zA-Z0-9\-]+$` / len 0..63.
    let too_long = "a".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("sort-by-outside-the-enum", json!({"SortBy": "Nope"})),
        ("sort-order-outside-the-enum", json!({"SortOrder": "Sideways"})),
        (
            "approval-status-outside-the-enum",
            json!({"ModelApprovalStatus": "Maybe"}),
        ),
        (
            "package-type-outside-the-enum",
            json!({"ModelPackageType": "Neither"}),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({"NextToken": "not-a-token"}),
        ),
        ("max-results-below-1", json!({"MaxResults": 0})),
        ("max-results-above-100", json!({"MaxResults": 101})),
        (
            "name-contains-with-an-illegal-character",
            json!({"NameContains": "bad name"}),
        ),
        ("name-contains-past-63", json!({"NameContains": too_long})),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("ListModelPackages", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: ListModelPackages must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed}"
        );
    }

    // Nothing above reached state: every seeded package is still listed.
    let listed = emulator
        .ok(
            "after-refusals",
            "ListModelPackages",
            &json!({"ModelPackageType": "Both", "SortBy": "Name", "SortOrder": "Ascending"}),
        )
        .await;
    assert_eq!(
        listed_ids(&listed).len(),
        seeded.len(),
        "a refused request must write nothing, got {listed:?}"
    );
}
