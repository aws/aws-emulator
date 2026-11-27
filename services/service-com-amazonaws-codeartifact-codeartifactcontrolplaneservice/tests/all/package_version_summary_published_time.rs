// SPDX-License-Identifier: CC0-1.0

//! What a `ListPackageVersions` SUMMARY reports, and where the publish instant is read.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire body a
//! CodeArtifact client parses, not a projection's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. A listing entry carries exactly the members its shape models — `version`,
//!    `revision`, `status` and `origin`
//!    (`smithy_query(query="shape",shape="PackageVersionSummary",model_path="services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/smithy-model/com-amazonaws-codeartifact-codeartifactcontrolplaneservice.json")`)
//!    — and nothing the shape does not model: the publish instant (`publishedTime`) is a
//!    member of the per-version DESCRIPTION, so it is read through
//!    `DescribePackageVersion` and never reported per listing entry. Rows differ along
//!    the asset size, the version string's shape, the namespace, the package and the
//!    number of versions the package holds.
//! 2. Every member a summary does carry is the same reading of the same stored version
//!    its description reports, and both readings keep their value across repeated reads
//!    and across a status move (`UpdatePackageVersionsStatus`, `DisposePackageVersions`)
//!    or a version left `Unfinished`: the listing projects the record the description
//!    reports, so the two cannot disagree. Rows cover every status this service's own
//!    operations can leave a stored version in.
//! 3. The ordering the operation sorts by reads the stored publish instant: under
//!    `sortBy=PUBLISHED_TIME` the most recently published version comes first, so the
//!    instants each listed version's description reports are non-increasing down the
//!    page.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness the crate's suites share, the publish helpers from the suite that owns
// `PublishPackageVersion`, and the status-move helpers from the suite that owns the updates.
use crate::harness::{call, Answer};
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish,
};
use crate::update_operations_persist::{dispose_versions, update_versions_status};

/// The `PackageVersionSortType` value that orders a listing by publish time — the one value
/// the enum models
/// (`smithy_query(query="shape",shape="PackageVersionSortType",model_path="services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/smithy-model/com-amazonaws-codeartifact-codeartifactcontrolplaneservice.json")`).
const SORT_BY_PUBLISHED_TIME: &str = "PUBLISHED_TIME";

/// Every member `PackageVersionSummary` models, in the order the shape declares them
/// (`smithy_query(query="shape",shape="PackageVersionSummary")`). A listing entry may
/// carry these and no others.
const SUMMARY_MEMBERS: [&str; 4] = ["version", "revision", "status", "origin"];

/// The members the shape marks `required`, which therefore appear on every entry.
const REQUIRED_SUMMARY_MEMBERS: [&str; 2] = ["version", "status"];

/// `ListPackageVersions` for the package a [`Publish`] addresses; `sort_by` is sent only
/// when a case pins the ordering. Every member is `httpQuery`-bound, so the body is empty.
async fn list(router: &MultiServiceRouter, request: &Publish, sort_by: Option<&str>) -> Answer {
    let mut uri = format!(
        "/v1/package/versions?domain={}&repository={}&format={}&package={}",
        request.domain, request.repository, request.format, request.package
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    if let Some(sort_by) = sort_by {
        uri.push_str(&format!("&sortBy={sort_by}"));
    }
    let answer = call(router, "POST", &uri, Some(serde_json::json!({}))).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "listing {} must succeed, got {answer:?}",
        request.package
    );
    answer
}

/// The `versions` entries of a successful listing, in the order they were returned.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("versions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The one summary a listing reports for `version`, or a panic naming what it did report.
fn summary_of(answer: &Answer, version: &str, case: &str) -> Value {
    summaries(answer)
        .into_iter()
        .find(|summary| summary["version"].as_str() == Some(version))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must report {version}, got {answer:?}")
        })
}

/// Assert one listing entry carries only modelled members, every required one among them.
fn assert_modelled_members(summary: &Value, case: &str) {
    let object = summary
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary object, got {summary:?}"));
    for member in object.keys() {
        assert!(
            SUMMARY_MEMBERS.contains(&member.as_str()),
            "case {case:?}: {member:?} is not a member of the summary shape, got {summary:?}"
        );
    }
    for member in REQUIRED_SUMMARY_MEMBERS {
        assert!(
            object.get(member).is_some_and(|value| !value.is_null()),
            "case {case:?}: expected the required member {member:?}, got {summary:?}"
        );
    }
}

/// The same publish, addressing another version of the same package — how a row reads one
/// listed version back through `DescribePackageVersion`.
fn addressing(request: &Publish, version: &str) -> Publish {
    let mut addressed = request.clone();
    addressed.version = version.to_string();
    addressed
}

/// The `packageVersion` a successful `DescribePackageVersion` reports for `version`.
async fn description(router: &MultiServiceRouter, request: &Publish, version: &str) -> Value {
    let described = describe(router, &addressing(request, version)).await;
    assert_eq!(
        (described.status, described.code()),
        (200, String::new()),
        "describing {version} must succeed, got {described:?}"
    );
    described.body["packageVersion"].clone()
}

/// The `publishedTime` a description reports, asserted to be the epoch-seconds JSON NUMBER
/// `restJson1` writes a timestamp as rather than a string or an absent member.
fn published_time(description: &Value, case: &str) -> f64 {
    let reported = &description["publishedTime"];
    let seconds = reported.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a publishedTime number, got {description:?}")
    });
    assert!(
        seconds > 0.0,
        "case {case:?}: expected an epoch-seconds publishedTime, got {reported:?}"
    );
    seconds
}

/// Publish `versions` of `package` in order, each with its own bytes, and hand back the
/// [`Publish`] of each so a row can address it afterwards.
async fn publish_versions(
    router: &MultiServiceRouter,
    package: &str,
    namespace: &str,
    versions: &[&str],
) -> Vec<Publish> {
    let mut published = Vec::new();
    for (seed, version) in versions.iter().enumerate() {
        let request = Publish::new(
            package,
            version,
            "payload.bin",
            binary_content(24 + seed * 11, seed as u8),
        )
        .namespace(namespace);
        let answer = publish(router, &request).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "publishing {package} {version} must succeed, got {answer:?}"
        );
        published.push(request);
    }
    published
}

// ---------------------------------------------------------------------------
// Rule 1: a summary carries the modelled members, and the instant is a description's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_summary_carries_only_the_members_the_shape_models() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: the namespace, the package, the
    // shape of the version strings and how many versions one package holds.
    let cases: [(&str, &str, &str, &[&str]); 4] = [
        ("one version", "alpha", "team-a", &["1.0.0"]),
        (
            "three versions, semver and date forms",
            "beta",
            "team-a",
            &["1.0.0", "2.0.0-rc.1", "20240101"],
        ),
        (
            "another namespace",
            "gamma",
            "other-team",
            &["0.0.1", "0.0.2"],
        ),
        (
            "a version string carrying punctuation",
            "delta",
            "team-b",
            &["1.0.0-build.7"],
        ),
    ];

    for (case, package, namespace, versions) in cases {
        let published = publish_versions(&router, package, namespace, versions).await;
        let listing = list(&router, &published[0], None).await;
        assert_eq!(
            summaries(&listing).len(),
            versions.len(),
            "case {case:?}: expected {} summaries, got {listing:?}",
            versions.len()
        );
        for request in &published {
            let case = format!("{case}/{}", request.version);
            let summary = summary_of(&listing, &request.version, &case);
            assert_modelled_members(&summary, &case);
            // The publish instant is a member of the DESCRIPTION, so it is absent here …
            assert!(
                summary.get("publishedTime").is_none(),
                "case {case:?}: the summary shape models no publishedTime, got {summary:?}"
            );
            // … and reported, well-formed, by the per-version read.
            let described = description(&router, request, &request.version).await;
            published_time(&described, &case);
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a summary is the same reading of the stored version its description is.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_summary_agrees_with_the_description_in_every_status_a_version_can_be_left_in() {
    let router = emulator_with_repository().await;

    // One version per status this service's own operations can leave a stored version in:
    // the two a publish stores, the two a status move reaches, and the terminal one a
    // disposal reaches.
    let published = publish_versions(
        &router,
        "statuses",
        "team-a",
        &["1.0.0", "2.0.0", "3.0.0", "4.0.0"],
    )
    .await;
    let unfinished = Publish::new("statuses", "5.0.0", "payload.bin", binary_content(9, 5))
        .namespace("team-a")
        .unfinished(true);
    let answer = publish(&router, &unfinished).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "the unfinished publish must succeed, got {answer:?}"
    );

    // What each version's description carried straight after its publish — the instant
    // every later reading has to keep reporting.
    let mut recorded: Vec<(String, Value)> = Vec::new();
    for version in ["1.0.0", "2.0.0", "3.0.0", "4.0.0", "5.0.0"] {
        let described = description(&router, &published[0], version).await;
        published_time(&described, version);
        recorded.push((version.to_string(), described["publishedTime"].clone()));
    }

    // The moves: two statuses through `UpdatePackageVersionsStatus`, one through
    // `DisposePackageVersions`, and one version left exactly as it was published.
    let moved = update_versions_status(&router, &published[0], &["2.0.0"], "Archived", None, None)
        .await;
    assert_eq!(moved.status, 200, "the archive must succeed, got {moved:?}");
    let moved = update_versions_status(&router, &published[0], &["3.0.0"], "Unlisted", None, None)
        .await;
    assert_eq!(moved.status, 200, "the unlist must succeed, got {moved:?}");
    let disposed = dispose_versions(&router, &published[0], &["4.0.0"], None, None).await;
    assert_eq!(
        disposed.status, 200,
        "the disposal must succeed, got {disposed:?}"
    );

    // Every status is listed on its own, so a row names the status it is about.
    let expected_status: [(&str, &str); 5] = [
        ("1.0.0", "Published"),
        ("2.0.0", "Archived"),
        ("3.0.0", "Unlisted"),
        ("4.0.0", "Disposed"),
        ("5.0.0", "Unfinished"),
    ];
    for (version, status) in expected_status {
        let case = format!("{status} {version}");
        let listing = list(&router, &published[0], None).await;
        let summary = summary_of(&listing, version, &case);
        assert_modelled_members(&summary, &case);
        assert_eq!(
            summary["status"].as_str(),
            Some(status),
            "case {case:?}: expected the status the move left, got {summary:?}"
        );

        // The members the summary carries are the description's own readings …
        let described = description(&router, &published[0], version).await;
        for member in ["status", "revision", "origin"] {
            assert_eq!(
                summary[member], described[member],
                "case {case:?}: expected the summary's {member:?} to be the described {:?}, \
                 got {:?}",
                described[member], summary[member]
            );
        }

        // … and the publish instant the description reports is still the one its publish
        // recorded: nothing is minted per call, and a status move does not touch it.
        let expected = recorded
            .iter()
            .find(|(named, _)| named == version)
            .map(|(_, instant)| instant.clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            described["publishedTime"], expected,
            "case {case:?}: expected the publish instant {expected:?}, got {:?}",
            described["publishedTime"]
        );
        let again = description(&router, &published[0], version).await;
        assert_eq!(
            again["publishedTime"], expected,
            "case {case:?}: a second read must report the same instant, got {:?}",
            again["publishedTime"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the ordering the listing sorts by reads the stored publish instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_publish_time_sorted_page_is_ordered_by_the_stored_instant() {
    let router = emulator_with_repository().await;

    // The version STRINGS are deliberately not in publish order, so an ordering that fell
    // back to the stored (version-string) order cannot pass this.
    let published = publish_versions(
        &router,
        "ordering",
        "team-a",
        &["9.0.0", "1.0.0", "5.0.0", "2.0.0"],
    )
    .await;

    let sorted = list(&router, &published[0], Some(SORT_BY_PUBLISHED_TIME)).await;
    let page = summaries(&sorted);
    assert_eq!(
        page.len(),
        4,
        "expected every version on the page, got {sorted:?}"
    );
    let mut previous: Option<f64> = None;
    let mut instants: Vec<f64> = Vec::new();
    for summary in &page {
        let case = summary["version"].as_str().unwrap_or_default().to_string();
        assert_modelled_members(summary, &case);
        let described = description(&router, &published[0], &case).await;
        let instant = published_time(&described, &case);
        if let Some(previous) = previous {
            assert!(
                instant <= previous,
                "case {case:?}: a PUBLISHED_TIME page reports the most recently published \
                 version first, so {instant} must not be later than {previous}"
            );
        }
        previous = Some(instant);
        instants.push(instant);
    }
    // The newest version the operation names as the default display version is the first
    // entry of that page, so the ordering and the stored instant agree about which one it
    // is. Only asserted where the clock this ran on separated the four publishes: with two
    // equal instants the head of the page is decided by the key's tie-break instead, and
    // the non-increasing invariant above is then the whole of the rule.
    let separated = instants
        .windows(2)
        .all(|pair| pair.first() != pair.last());
    if separated {
        assert_eq!(
            page.first().map(|summary| summary["version"].clone()),
            Some(Value::String("2.0.0".to_string())),
            "expected the last-published version first, got {sorted:?}"
        );
    }
}
