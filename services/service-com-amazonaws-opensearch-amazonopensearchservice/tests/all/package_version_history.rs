// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetPackageVersionHistory`: the versions of a package's
//! content this service persisted, reported for the package the request names.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the package helpers from
//! [`super::package_associations`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the history reports every version of the content the package holds, each with
//!   the commit message, creation instant and configuration stored WITH that version
//!   and no plugin properties, over a table of creates spanning package types, the
//!   configuration container present and absent, and both engine lines;
//! * the history is a SEQUENCE: it is reported MOST RECENT VERSION FIRST and paged
//!   through without repeating or skipping a version, whatever page size is asked for,
//!   over a table crossing the page size with a history longer than it;
//! * a page size outside the range the model publishes for `MaxResults`, a token that
//!   names no version of the package, and an id that names no package at all are each
//!   refused with the error the model declares, over a table spanning both ends of the
//!   range and both `PackageID` forms the pattern admits;
//! * reading a history is a REPORT: served and refused reads leave every record a
//!   later caller sees exactly as it was.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    PackageConfiguration, PACKAGE_TYPE_VALUES, REQUIREMENT_LEVEL_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PackageVersionRecord, FIRST_PACKAGE_VERSION, PACKAGE_NOT_FOUND, PACKAGE_TYPE_ZIP_PLUGIN,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::{SERVICE_PACKAGE_ID, UNKNOWN_PACKAGE_ID};

/// The largest page size `GetPackageVersionHistory` publishes: `MaxResults` is
/// `@range(max: 100)` with `@default(0)` and no minimum
/// (`smithy_query(query="operation",operation="GetPackageVersionHistory")`).
const PUBLISHED_MAX_RESULTS: i32 = 100;

/// The longest `CommitMessage` the model admits: it is `@length(min: 0, max: 160)`
/// (`smithy_query(query="shape",shape="CommitMessage")`), so this and the empty string
/// are the bounds a stored message is reported at.
const MAX_COMMIT_MESSAGE: usize = 160;

/// An instant with no sub-second part, so a seeded version's `CreatedAt` is reported
/// as exactly the epoch seconds this test wrote. Its value is arbitrary: what the
/// cases assert is the RELATION between the instants, not any one of them.
const SEED_EPOCH: Duration = Duration::from_secs(1_767_225_600);

/// `CreatePackage`: `POST /2021-01-01/packages`.
async fn create(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages"), Some(body))
        .await
}

/// `DescribePackages`: `POST /2021-01-01/packages/describe`.
async fn describe(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages/describe"), Some(body))
        .await
}

/// `GetPackageVersionHistory`: `GET /2021-01-01/packages/{PackageID}/history`,
/// carrying `query` verbatim — the cases send page sizes a typed helper could not
/// express (a negative one, one past the published maximum).
async fn history(emulator: &Emulator, package_id: &str, query: &str) -> Answer {
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/packages/{package_id}/history{suffix}"),
            None,
        )
        .await
}

/// The query string for a paged read: `page_size` of `0` sends no page size at all,
/// which is the request that asks for no particular one.
fn page_query(page_size: i32, token: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if page_size != 0 {
        parts.push(format!("maxResults={page_size}"));
    }
    if let Some(token) = token {
        parts.push(format!("nextToken={token}"));
    }
    parts.join("&")
}

/// The `PackageVersionHistoryList` of an answered read, with the case named on
/// failure.
fn entries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageVersionHistoryList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageVersionHistoryList array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `PackageVersion` of each reported entry, in the order they were reported.
fn reported_versions(answer: &Answer, case: &str) -> Vec<String> {
    entries(answer, case)
        .iter()
        .map(|entry| {
            entry["PackageVersion"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a string PackageVersion, got {entry:?}")
                })
                .to_owned()
        })
        .collect()
}

/// One create to make, and the `PackageConfiguration` it carries.
struct Row {
    /// `PackageName`, derived from the row's place in the value set so the table
    /// extends with the model rather than having to be re-written: it satisfies
    /// `PackageName`'s `@length(min: 3, max: 256)` with
    /// `@pattern("^[a-z][a-z0-9\-]+$")` for every index.
    name: String,
    /// `PackageType`.
    package_type: &'static str,
    /// `EngineVersion`, absent where `None` — it may be given only for the plugin
    /// type, so only that row carries one.
    engine_version: Option<&'static str>,
    /// `PackageConfiguration`, absent where `None`.
    configuration: Option<Value>,
}

impl Row {
    fn body(&self) -> Value {
        let mut body = json!({
            "PackageName": self.name,
            "PackageType": self.package_type,
            "PackageSource": {
                "S3BucketName": "my-package-bucket",
                "S3Key": format!("packages/{}", self.name),
            },
        });
        if let Some(engine_version) = self.engine_version {
            body["EngineVersion"] = json!(engine_version);
        }
        if let Some(configuration) = &self.configuration {
            body["PackageConfiguration"] = configuration.clone();
        }
        body
    }
}

/// The creates every table in this suite makes: one per `PackageType` value, so a
/// rule asserted over the table is asserted over every type a package can have. The
/// value set is this service's Smithy model's
/// (`smithy_query(query="shape",shape="PackageType")`), reached through the generated
/// `PACKAGE_TYPE_VALUES` rather than copied, and the configuration container is
/// present with every member, present with only its required ones, and absent.
fn rows() -> Vec<Row> {
    PACKAGE_TYPE_VALUES
        .iter()
        .enumerate()
        .map(|(index, package_type)| Row {
            name: format!("history-package-{index}"),
            package_type,
            engine_version: (*package_type == PACKAGE_TYPE_ZIP_PLUGIN).then_some("OpenSearch_2.11"),
            configuration: match index % 3 {
                // Every member the shape declares, with the license requirement taken
                // from the model's own `RequirementLevel` value set.
                0 => Some(json!({
                    "LicenseRequirement": REQUIREMENT_LEVEL_VALUES[2],
                    "LicenseFilepath": "licenses/LICENSE.txt",
                    "ConfigurationRequirement": REQUIREMENT_LEVEL_VALUES[1],
                    "RequiresRestartForConfigurationUpdate": true,
                })),
                // Only the two members the shape requires.
                1 => Some(json!({
                    "LicenseRequirement": REQUIREMENT_LEVEL_VALUES[0],
                    "ConfigurationRequirement": REQUIREMENT_LEVEL_VALUES[0],
                })),
                // No configuration at all.
                _ => None,
            },
        })
        .collect()
}

/// Create `row`'s package and answer with the `PackageID` the create minted.
async fn created(emulator: &Emulator, row: &Row) -> String {
    let case = format!("CreatePackage({:?})", row.name);
    let answer = create(emulator, row.body()).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetails"]["PackageID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a PackageID, got {:?}", answer.body))
        .to_owned()
}

/// One further version of a package's content to put in the store.
struct Seeded {
    /// `CommitMessage`, absent where `None`.
    commit_message: Option<String>,
    /// How far past [`SEED_EPOCH`] this version was imported.
    offset: u64,
    /// Whether this version carries a `PackageConfiguration`.
    configured: bool,
}

/// The further versions the paging cases need: one per way a stored version differs
/// from the first — a commit message at each end of the published length bound, a
/// message with neither, an absent message, and the configuration container present
/// and absent — each imported later than the one before.
fn seeded() -> Vec<Seeded> {
    vec![
        Seeded {
            commit_message: Some("Rebuilt the analyzer dictionary".to_owned()),
            offset: 60,
            configured: false,
        },
        Seeded {
            commit_message: Some(String::new()),
            offset: 120,
            configured: true,
        },
        Seeded {
            commit_message: Some("m".repeat(MAX_COMMIT_MESSAGE)),
            offset: 180,
            configured: false,
        },
        Seeded {
            commit_message: None,
            offset: 240,
            configured: true,
        },
        Seeded {
            commit_message: Some("Fifth import".to_owned()),
            offset: 300,
            configured: false,
        },
        Seeded {
            commit_message: Some("Sixth import".to_owned()),
            offset: 360,
            configured: true,
        },
    ]
}

/// The `PackageConfiguration` a seeded version carries, built from the modelled shape
/// with the requirement levels the model publishes.
fn seeded_configuration(version: u32) -> PackageConfiguration {
    PackageConfiguration {
        license_requirement: REQUIREMENT_LEVEL_VALUES[1].to_owned(),
        license_filepath: Some(format!("licenses/v{version}/LICENSE.txt")),
        configuration_requirement: REQUIREMENT_LEVEL_VALUES[2].to_owned(),
        requires_restart_for_configuration_update: Some(false),
    }
}

/// Put the further versions of `package_id`'s content in the store the way a handler
/// reaches it.
///
/// `UpdatePackage` imports another version of a package's content over the wire, but
/// each import it serves is stamped with the instant it was made; these cases assert
/// the RELATION between the instants of versions imported minutes apart, so the
/// versions are written here with instants of this suite's choosing — the same seam
/// [`super::package_associations`] uses for a newer package version.
fn seed_versions(emulator: &Emulator, package_id: &str, versions: &[Seeded]) {
    let store = emulator.store();
    let mut guard = store.lock();
    let package = guard
        .packages
        .get_mut(package_id)
        .expect("the created package is in the store");
    for (index, version) in versions.iter().enumerate() {
        let number = FIRST_PACKAGE_VERSION + 1 + u32::try_from(index).unwrap_or_default();
        package.versions.push(PackageVersionRecord {
            version: number,
            commit_message: version.commit_message.clone(),
            created_at: SystemTime::UNIX_EPOCH + SEED_EPOCH + Duration::from_secs(version.offset),
            configuration: version.configured.then(|| seeded_configuration(number)),
            // The seeded version records a history entry, not content to analyze, and
            // not the descriptor of a plugin archive.
            plugin_properties: None,
            content: None,
        });
        package.version = number;
    }
}

/// The versions a walk of `package_id`'s history reports, from the first page to the
/// last, over pages of `page_size` (`0` to send no page size at all).
async fn walk(emulator: &Emulator, package_id: &str, page_size: i32, case: &str) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let answer = history(emulator, package_id, &page_query(page_size, cursor.as_deref())).await;
        let page = reported_versions(&answer, case);
        if let Ok(asked_for) = usize::try_from(page_size) {
            if asked_for > 0 {
                assert!(
                    page.len() <= asked_for,
                    "case {case}: a page is never larger than the request asked for, got {page:?}"
                );
            }
        }
        walked.extend(page);
        cursor = answer.body["NextToken"].as_str().map(str::to_owned);
        pages += 1;
        assert!(
            pages <= seeded().len() + 2,
            "case {case}: paging kept issuing tokens past the end of the history, got {walked:?}"
        );
        if cursor.is_none() {
            return walked;
        }
    }
}

#[tokio::test]
async fn the_history_reports_the_version_of_the_content_the_create_imported() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = format!("GetPackageVersionHistory({:?})", row.name);
        let id = created(&emulator, &row).await;

        let answer = history(&emulator, &id, "").await;
        assert_eq!(
            answer.body["PackageID"].as_str(),
            Some(id.as_str()),
            "case {case}: the history is reported for the package the request named, got {:?}",
            answer.body
        );

        let reported = entries(&answer, &case);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: a package whose content was imported once has one version, got \
             {reported:?}"
        );
        let entry = &reported[0];
        assert_eq!(
            entry["PackageVersion"].as_str(),
            Some(FIRST_PACKAGE_VERSION.to_string().as_str()),
            "case {case}: the first version of a package's content is numbered one, got {entry:?}"
        );
        assert!(
            entry["CommitMessage"].is_null(),
            "case {case}: a create carries no CommitMessage to record, got {:?}",
            entry["CommitMessage"]
        );
        assert!(
            entry["PluginProperties"].is_null(),
            "case {case}: the plugin descriptor inside the archive is not fetched, so no plugin \
             properties are reported, got {:?}",
            entry["PluginProperties"]
        );
        assert_eq!(
            entry["PackageConfiguration"],
            row.configuration.clone().unwrap_or(Value::Null),
            "case {case}: the version carries the configuration the import gave it, got {entry:?}"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no token once the page ended the history, got {:?}",
            answer.body["NextToken"]
        );

        // The instant the version was imported is the instant the package was
        // created, because the create is the import that made it: the two are read
        // from the one stored record rather than invented per call.
        let described = describe(
            &emulator,
            json!({ "Filters": [{ "Name": "PackageID", "Value": [id] }] }),
        )
        .await;
        let package = &described.body["PackageDetailsList"][0];
        assert_eq!(
            entry["CreatedAt"].as_f64(),
            package["CreatedAt"].as_f64(),
            "case {case}: expected the version's CreatedAt to be the package's, got {entry:?} \
             against {package:?}"
        );
    }
}

#[tokio::test]
async fn the_history_a_package_carries_does_not_change_when_its_copy_lands() {
    let emulator = Emulator::new();
    let row = &rows()[0];
    let id = created(&emulator, row).await;

    // The instant the create returns the package's content is still being copied, and
    // the version it imported is already in its history.
    let before = history(&emulator, &id, "").await;
    assert_eq!(
        reported_versions(&before, "the read before the copy landed"),
        vec![FIRST_PACKAGE_VERSION.to_string()],
        "case the read before the copy landed: expected the version the create imported, got {:?}",
        before.body
    );

    // Past the span the clock owes the copy: the package becomes available, which
    // changes which version is available for an association and not which versions of
    // the content it has had.
    tokio::time::sleep(SETTLE_WAIT).await;
    let after = history(&emulator, &id, "").await;
    assert_eq!(
        after.body, before.body,
        "case the read after the copy landed: expected the same history, got {:?}",
        after.body
    );
}

#[tokio::test]
async fn the_history_reports_every_stored_version_newest_first() {
    let emulator = Emulator::new();
    let row = &rows()[0];
    let id = created(&emulator, row).await;
    let versions = seeded();
    seed_versions(&emulator, &id, &versions);

    let case = "the unbounded read";
    let answer = history(&emulator, &id, "").await;
    let reported = entries(&answer, case);
    assert_eq!(
        reported.len(),
        versions.len() + 1,
        "case {case}: expected every stored version, got {reported:?}"
    );

    // The history is reported most recent version first, so row 0 is the newest
    // seeded version and the last row is the version the create imported.
    let newest = FIRST_PACKAGE_VERSION + u32::try_from(versions.len()).unwrap_or_default();
    for (index, entry) in reported.iter().enumerate() {
        let number = newest - u32::try_from(index).unwrap_or_default();
        let case = format!("{case} (version {number})");
        assert_eq!(
            entry["PackageVersion"].as_str(),
            Some(number.to_string().as_str()),
            "case {case}: expected the history most recent version first, got {reported:?}"
        );
        let Some(seeded) = versions.get((number as usize).wrapping_sub(2)) else {
            continue;
        };
        assert_eq!(
            entry["CommitMessage"].as_str(),
            seeded.commit_message.as_deref(),
            "case {case}: expected the commit message stored with this version, got {entry:?}"
        );
        assert_eq!(
            entry["CreatedAt"].as_f64(),
            Some((SEED_EPOCH.as_secs() + seeded.offset) as f64),
            "case {case}: expected the instant this version was imported, got {entry:?}"
        );
        assert_eq!(
            entry["PackageConfiguration"]["LicenseFilepath"].as_str(),
            seeded
                .configured
                .then(|| format!("licenses/v{number}/LICENSE.txt"))
                .as_deref(),
            "case {case}: expected the configuration stored with this version, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn paging_the_history_reports_every_version_exactly_once() {
    let emulator = Emulator::new();
    let row = &rows()[0];
    let id = created(&emulator, row).await;
    let versions = seeded();
    seed_versions(&emulator, &id, &versions);

    // Every version the package has had, in the order a read reports them: most
    // recent first.
    let whole: Vec<String> = (0..=versions.len())
        .rev()
        .map(|index| (FIRST_PACKAGE_VERSION as usize + index).to_string())
        .collect();

    // One row per page size: no page size at all, the smallest, sizes that divide the
    // history evenly and unevenly, exactly its length, one past it, and the published
    // maximum.
    for page_size in [
        0_i32,
        1,
        2,
        3,
        i32::try_from(versions.len() + 1).unwrap_or(PUBLISHED_MAX_RESULTS),
        i32::try_from(versions.len() + 2).unwrap_or(PUBLISHED_MAX_RESULTS),
        PUBLISHED_MAX_RESULTS,
    ] {
        let case = format!("walked at page size {page_size}");
        let walked = walk(&emulator, &id, page_size, &case).await;
        assert_eq!(
            walked, whole,
            "case {case}: expected every version exactly once, most recent first, got {walked:?}"
        );
    }

    // A token names a PLACE in the history, so the page size that resumes it changes
    // how the remainder is cut into pages and not which versions it reports.
    for (issued_with, resumed_with) in [(1_i32, 1_i32), (1, 3), (3, 1), (2, 0), (4, 100)] {
        let case = format!("token issued at page size {issued_with}, resumed at {resumed_with}");
        let first = history(&emulator, &id, &page_query(issued_with, None)).await;
        let head = reported_versions(&first, &case);
        assert_eq!(
            head.len(),
            usize::try_from(issued_with).unwrap_or_default(),
            "case {case}: a page with more to come is as large as the request asked for, got \
             {head:?}"
        );
        let token = first.body["NextToken"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a token while versions are unreported, got {:?}",
                    first.body
                )
            });
        assert_eq!(
            Some(token.as_str()),
            whole.get(head.len()).map(String::as_str),
            "case {case}: expected the token to name the version the next page starts at, got \
             {token:?}"
        );

        let mut rejoined = head;
        let mut cursor = Some(token);
        while let Some(resume) = cursor {
            let answer = history(&emulator, &id, &page_query(resumed_with, Some(&resume))).await;
            rejoined.extend(reported_versions(&answer, &case));
            cursor = answer.body["NextToken"].as_str().map(str::to_owned);
        }
        assert_eq!(
            rejoined, whole,
            "case {case}: expected the resumed walk to report the history's remainder, got \
             {rejoined:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_served_reports_no_history() {
    let emulator = Emulator::new();
    let row = &rows()[0];
    let id = created(&emulator, row).await;

    // An id that names no package of this account names no history, and is refused as
    // the bad input value it is: both forms the model's `PackageID` pattern admits, the
    // caller's own and the one addressing a package the service publishes.
    for unknown in [UNKNOWN_PACKAGE_ID, SERVICE_PACKAGE_ID] {
        let case = format!("GetPackageVersionHistory({unknown:?})");
        let answer = history(&emulator, unknown, "").await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected the modelled 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(PACKAGE_NOT_FOUND),
            "case {case}: expected the fixed id-free sentence, got {:?}",
            answer.body
        );
    }

    // A page size outside the range the model publishes is not a page size at all:
    // one past the maximum, far past it, and the negative values the shape's own
    // range (a maximum and no minimum) does not catch.
    for value in [PUBLISHED_MAX_RESULTS + 1, 1_000, i32::MAX, -1, i32::MIN] {
        let case = format!("GetPackageVersionHistory(maxResults={value})");
        let answer = history(&emulator, &id, &format!("maxResults={value}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // Below the range the refusal is this operation's own, and names the bound it
    // broke and the value it carried.
    for value in [-1, i32::MIN] {
        let case = format!("GetPackageVersionHistory(maxResults={value})");
        let answer = history(&emulator, &id, &format!("maxResults={value}")).await;
        assert_eq!(
            answer.body["message"].as_str(),
            Some(
                format!("MaxResults must be between 0 and {PUBLISHED_MAX_RESULTS}: {value}")
                    .as_str()
            ),
            "case {case}: expected the refusal to name the bound and the value, got {:?}",
            answer.body
        );
    }

    // A token is only ever a value this operation handed out, so one that names no
    // version of this package is a malformed request rather than an empty page: a
    // version past the newest, one below the first, and values of no version form.
    for token in ["2", "0", "-1", "99", "not-a-version", "pkg-1"] {
        let case = format!("GetPackageVersionHistory(nextToken={token:?})");
        let answer = history(&emulator, &id, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // Every page size inside the range, and the published bound itself, are served:
    // the refusals above are the value each request carried rather than the call.
    for value in [1, 2, PUBLISHED_MAX_RESULTS] {
        let case = format!("GetPackageVersionHistory(maxResults={value})");
        let answer = history(&emulator, &id, &format!("maxResults={value}")).await;
        assert_eq!(
            reported_versions(&answer, &case),
            vec![FIRST_PACKAGE_VERSION.to_string()],
            "case {case}: the published bound is accepted and reports the history, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn reading_a_version_history_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let row = &rows()[0];
    let id = created(&emulator, row).await;
    seed_versions(&emulator, &id, &seeded());

    // Past the span the clock owes the copy, so the records below are the ones it
    // settled on and any later difference is a read's doing.
    tokio::time::sleep(SETTLE_WAIT).await;
    let before = history(&emulator, &id, "").await;
    let described_before = describe(&emulator, json!({})).await;

    // Served, paged and refused reads, all of which only report.
    for query in [
        "",
        "maxResults=1",
        "maxResults=3",
        &format!("maxResults={PUBLISHED_MAX_RESULTS}"),
        "maxResults=-1",
        "maxResults=101",
        "nextToken=3",
        "nextToken=no-such-version",
    ] {
        history(&emulator, &id, query).await;
    }
    history(&emulator, UNKNOWN_PACKAGE_ID, "").await;
    for page_size in [0_i32, 1, 2, 5] {
        walk(&emulator, &id, page_size, "the walk between the two reads").await;
    }

    let after = history(&emulator, &id, "").await;
    assert_eq!(
        after.body, before.body,
        "case the read after every other read: expected the history the first read reported, got \
         {:?}",
        after.body
    );
    let described_after = describe(&emulator, json!({})).await;
    assert_eq!(
        described_after.body, described_before.body,
        "case the package read after every history read: expected the records the first read \
         reported, got {:?}",
        described_after.body
    );
}
