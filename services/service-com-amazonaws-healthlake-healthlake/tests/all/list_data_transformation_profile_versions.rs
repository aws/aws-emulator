// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `ListDataTransformationProfileVersions` over the real
//! awsJson1_0 wire, cross-checked against `GetDataTransformationProfile`.
//!
//! The rule under test: the listing reports EVERY version the named profile
//! holds — its DRAFT (version 0) and each version published from it — in reverse
//! chronological order, newest first, where a version's chronology is WHEN IT WAS
//! CREATED: the DRAFT is minted with the profile and each publish mints the next
//! version number, so the newest version is the highest-numbered one and the
//! DRAFT comes last, however often it is later rewritten. It reports each version
//! through the modelled summary, carrying the profile's own metadata and that
//! version's `ChangeDescription` and `LastUpdatedAt`; and it serves that one order
//! a page at a time, each page holding at most `MaxResults` entries and carrying a
//! `NextToken` exactly when versions remain after it. Because the order is keyed
//! on something no later call rewrites, the pages partition the listing — nothing
//! repeated and nothing skipped — even when the profile is written to between two
//! pages. A profile no record holds is refused, a token that resumes nothing is
//! refused, and the listing writes nothing, so two identical calls answer
//! identically.
//!
//! The rows differ along what the rule generalises over: the source format, how
//! many versions the profile holds, whether each version carries a
//! `ChangeDescription`, and whether the DRAFT was rewritten after a publish — a
//! rewrite restamps the DRAFT's `LastUpdatedAt` without moving it in the listing,
//! so the reported timestamps and the reported order come apart.
//!
//! Every call below goes through the shared [`Profiles`] harness, i.e.
//! `MultiServiceRouter::dispatch` — the same entry point `core/router` serves — so
//! routing, deserialization, the modelled constraints and the error envelope are
//! all exercised, not bypassed.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    MAX_PROFILE_VERSION, profile_not_found_message,
};

use crate::data_transformation_profile::{
    mapping, modelled_message, profile_mapping_source, Profiles,
};
use crate::fhir_datastore_lifecycle::string_at;
use crate::publish_data_transformation_profile::request_mapping;

/// The largest page the modelled `MaxResults` range (1..100) allows, which is
/// also the page a request naming no `MaxResults` gets.
const MAX_PAGE_SIZE: i32 = 100;

/// One step of a row's history, after the create that leaves the DRAFT behind.
///
/// Each step touches exactly ONE version: a publish CREATES the next version,
/// and a DRAFT rewrite creates nothing, so only the publishes extend the
/// listing.
#[derive(Debug, Clone)]
enum Step {
    /// `PublishDataTransformationProfile`: snapshots the DRAFT into the next
    /// published version, which carries this call's `ChangeDescription`.
    Publish(Option<&'static str>),
    /// `UpdateDataTransformationProfile`: rewrites the DRAFT in place with this
    /// content and this `ChangeDescription`, so the DRAFT becomes the version
    /// that changed most recently.
    UpdateDraft(&'static [(&'static str, &'static str)], Option<&'static str>),
}

/// One listing row: the profile to build and the history to build it with.
struct ListingCase {
    case: &'static str,
    profile_name: &'static str,
    /// The modelled `SourceFormat` of the profile (one of `CCDA` | `CSV`).
    source_format: &'static str,
    steps: Vec<Step>,
}

/// The listing table. The rule generalises over the source format, how many
/// versions a profile holds, whether a version carries a `ChangeDescription` and
/// the order the versions were last touched in, so the rows differ along exactly
/// those — from a profile that has only its DRAFT to one whose DRAFT and
/// published versions interleave in time.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        ListingCase {
            case: "CSV, never published: the DRAFT is the only version",
            profile_name: "csv-draft-only",
            source_format: "CSV",
            steps: Vec::new(),
        },
        ListingCase {
            case: "CSV, one publish carrying a change description",
            profile_name: "csv-one-publish",
            source_format: "CSV",
            steps: vec![Step::Publish(Some("first release of the patient mapping"))],
        },
        ListingCase {
            case: "CCDA, three publishes, only some of them described",
            profile_name: "ccda-three-publishes",
            source_format: "CCDA",
            steps: vec![
                Step::Publish(Some("sections 1-3")),
                Step::Publish(None),
                Step::Publish(Some("vitals added — ünïcode and punctuation: {}/[]")),
            ],
        },
        ListingCase {
            case: "CSV, a publish then a DRAFT rewrite, so the newest CHANGE is the \
                    oldest version",
            profile_name: "csv-draft-rewritten-last",
            source_format: "CSV",
            steps: vec![
                Step::Publish(Some("released before the rewrite")),
                Step::UpdateDraft(
                    &[("mapping-config.yaml", "tableName: people")],
                    Some("mrn replaces patient_id"),
                ),
            ],
        },
        ListingCase {
            case: "CCDA, DRAFT rewrites and publishes interleaved",
            profile_name: "ccda-interleaved",
            source_format: "CCDA",
            steps: vec![
                Step::UpdateDraft(&[("sections/problems.map", "Condition.code <- value")], None),
                Step::Publish(Some("problems only")),
                Step::UpdateDraft(
                    &[
                        ("sections/problems.map", "Condition.code <- value"),
                        ("sections/vitals.map", "Observation.value[x] <- vital"),
                    ],
                    Some("vitals in the DRAFT"),
                ),
                Step::Publish(Some("problems and vitals")),
                Step::UpdateDraft(
                    &[("sections/problems.map", "Condition.code <- code/value")],
                    None,
                ),
            ],
        },
        ListingCase {
            case: "CSV, seven publishes, more versions than the smallest pages hold",
            profile_name: "csv-seven-publishes",
            source_format: "CSV",
            steps: (0..7).map(|_| Step::Publish(None)).collect(),
        },
    ]
}

/// What a row's history leaves behind: the profile it built and what the listing
/// must report for it.
struct Built {
    profile_id: String,
    /// The versions the listing must report, NEWEST CREATED FIRST.
    expected_order: Vec<i32>,
    /// The `ChangeDescription` each version carries, `None` for one that has
    /// none.
    descriptions: BTreeMap<i32, Option<String>>,
}

/// Run `row`'s history and derive what the listing must report for it.
///
/// The expectation is derived from the order the versions were CREATED in, not
/// from any timestamp the service returned: the create mints the DRAFT and each
/// publish mints the next version number, so the version numbers descending is
/// the reverse chronological order under test — whatever a later DRAFT rewrite
/// does to the DRAFT's `LastUpdatedAt`.
async fn build(healthlake: &Profiles, row: &ListingCase) -> Built {
    let case = row.case;
    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": row.profile_name,
                "SourceFormat": row.source_format,
                "Source": profile_mapping_source(&mapping(&[(
                    "mapping-config.yaml",
                    "tableName: patients",
                )])),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    // The create leaves the DRAFT, version 0, behind.
    let mut descriptions: BTreeMap<i32, Option<String>> = BTreeMap::from([(0, None)]);
    let mut published = 0i32;

    for (index, step) in row.steps.iter().enumerate() {
        let step_number = index + 1;
        match step {
            Step::Publish(description) => {
                published += 1;
                let mut request = json!({
                    "ProfileId": profile_id,
                    "SourceFormat": row.source_format,
                });
                if let Some(text) = description {
                    request["ChangeDescription"] = json!(text);
                }
                let response = healthlake
                    .ok(case, "PublishDataTransformationProfile", &request)
                    .await;
                assert_eq!(
                    response["Version"].as_i64(),
                    Some(i64::from(published)),
                    "case {case:?}: step {step_number} must publish version {published}, \
                     got {response}"
                );
                descriptions.insert(published, description.map(str::to_string));
            }
            Step::UpdateDraft(content, description) => {
                let mut request = json!({
                    "ProfileId": profile_id,
                    "ProfileMapping": request_mapping(&mapping(content)),
                });
                if let Some(text) = description {
                    request["ChangeDescription"] = json!(text);
                }
                healthlake
                    .ok(case, "UpdateDataTransformationProfile", &request)
                    .await;
                descriptions.insert(0, description.map(str::to_string));
            }
        }
    }

    // Newest created first: the highest published version down to the DRAFT,
    // which was minted by the create and so is the oldest version there is.
    let expected_order: Vec<i32> = (0..=published).rev().collect();

    Built {
        profile_id,
        expected_order,
        descriptions,
    }
}

/// The `Version` of one reported summary.
fn version_of(case: &str, item: &Value) -> i32 {
    let version = item["Version"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: every summary carries a Version: {item}"));
    i32::try_from(version)
        .unwrap_or_else(|_| panic!("case {case:?}: Version is a modelled integer: {item}"))
}

/// The `LastUpdatedAt` of one reported summary, as the awsJson wire carries it:
/// epoch seconds.
fn last_updated_at(case: &str, item: &Value) -> f64 {
    item["LastUpdatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: every summary carries a LastUpdatedAt: {item}"))
}

/// Every page of the listing for `profile_id` at `max_results`, flattened, with
/// each page's size and token checked as it arrives: a page holds at most
/// `MaxResults` entries, and carries a `NextToken` only when it is full and
/// versions remain after it.
async fn list_every_page(
    healthlake: &Profiles,
    case: &str,
    profile_id: &str,
    max_results: Option<i32>,
) -> Vec<Value> {
    let limit = usize::try_from(max_results.unwrap_or(MAX_PAGE_SIZE)).unwrap_or(1);
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let mut request = json!({ "ProfileId": profile_id });
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(next) = token.as_deref() {
            request["NextToken"] = json!(next);
        }
        let body = healthlake
            .ok(case, "ListDataTransformationProfileVersions", &request)
            .await;
        let page = body["Items"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the response carries an Items list: {body}"));
        assert!(
            page.len() <= limit,
            "case {case:?}: a page holds at most MaxResults ({limit}) entries, got {}",
            page.len()
        );
        let next_token = body.get("NextToken").and_then(Value::as_str).map(str::to_string);
        assert!(
            next_token.is_none() || page.len() == limit,
            "case {case:?}: a page that carries a NextToken is full, got {} of {limit}: {body}",
            page.len()
        );
        items.extend(page);
        pages += 1;
        // A profile holds at most its DRAFT and the modelled `ProfileVersion`
        // maximum of published versions, so at worst one page per version plus
        // one terminating page.
        let ceiling = usize::try_from(MAX_PROFILE_VERSION).unwrap_or(99) + 2;
        assert!(
            pages <= ceiling,
            "case {case:?}: paging must terminate, got {pages} pages"
        );
        match next_token {
            None => break,
            Some(next) => token = Some(next),
        }
    }

    items
}

/// Every row's listing reports exactly the versions the profile holds, newest
/// first, through the modelled summary — whichever page size it is read at — and
/// each summary agrees with `GetDataTransformationProfile` at that version.
#[tokio::test]
async fn the_listing_reports_every_version_newest_first() {
    let healthlake = Profiles::new();

    for row in listing_cases() {
        let case = row.case;
        let built = build(&healthlake, &row).await;

        // The rule holds at every page size the modelled range admits, so the
        // listing is read at its bounds (1 and the 100 maximum), at sizes that
        // split the rows into several pages, and with `MaxResults` absent.
        for max_results in [None, Some(1), Some(2), Some(3), Some(MAX_PAGE_SIZE)] {
            let items = list_every_page(&healthlake, case, &built.profile_id, max_results).await;
            let reported: Vec<i32> = items
                .iter()
                .map(|item| version_of(case, item))
                .collect();
            assert_eq!(
                reported, built.expected_order,
                "case {case:?}: at MaxResults {max_results:?} the listing reports every \
                 version newest first, got {reported:?}"
            );

            // Newest first is also visible in the entries on their own: each
            // version was created before the one above it, so the version
            // numbers strictly decrease down the listing.
            for pair in items.windows(2) {
                let (earlier, later) = (version_of(case, &pair[0]), version_of(case, &pair[1]));
                assert!(
                    earlier > later,
                    "case {case:?}: the version numbers must strictly decrease down the \
                     listing, got {earlier} then {later}"
                );
            }

            for item in &items {
                let version = version_of(case, item);
                let fetched = healthlake
                    .ok(
                        case,
                        "GetDataTransformationProfile",
                        &json!({ "ProfileId": built.profile_id, "ProfileVersion": version }),
                    )
                    .await;
                for member in [
                    "ProfileId",
                    "Version",
                    "SourceFormat",
                    "TargetFormat",
                    "ProfileName",
                    "LastUpdatedAt",
                ] {
                    assert_eq!(
                        item.get(member),
                        fetched.get(member),
                        "case {case:?}: version {version}'s summary {member} must match the \
                         version read back, got {item} against {fetched}"
                    );
                }
                let expected = built.descriptions.get(&version).cloned().flatten();
                assert_eq!(
                    item.get("ChangeDescription").and_then(Value::as_str),
                    expected.as_deref(),
                    "case {case:?}: version {version} carries the ChangeDescription its own \
                     call gave it, got {item}"
                );
                assert_eq!(
                    item.get("SourceFormat").and_then(Value::as_str),
                    Some(row.source_format),
                    "case {case:?}: every summary carries the profile's source format, \
                     got {item}"
                );
            }
        }

        // The listing writes nothing, so a second identical call answers
        // identically.
        let first = list_every_page(&healthlake, case, &built.profile_id, None).await;
        let second = list_every_page(&healthlake, case, &built.profile_id, None).await;
        assert_eq!(
            first, second,
            "case {case:?}: reading the listing twice must answer identically"
        );
    }
}

/// A `NextToken` resumes the listing exactly where the page that handed it out
/// ended: the pages partition the listing, with nothing repeated and nothing
/// skipped.
#[tokio::test]
async fn a_next_token_resumes_where_its_page_ended() {
    let healthlake = Profiles::new();
    let case = "resuming a page of a profile with five published versions";
    let built = build(
        &healthlake,
        &ListingCase {
            case,
            profile_name: "csv-paging",
            source_format: "CSV",
            steps: vec![
                Step::Publish(Some("v1")),
                Step::Publish(Some("v2")),
                Step::UpdateDraft(
                    &[("mapping-config.yaml", "tableName: people")],
                    Some("draft"),
                ),
                Step::Publish(Some("v3")),
                Step::Publish(None),
                Step::Publish(Some("v5")),
            ],
        },
    )
    .await;

    for page_size in 1..=4 {
        let first = healthlake
            .ok(
                case,
                "ListDataTransformationProfileVersions",
                &json!({ "ProfileId": built.profile_id, "MaxResults": page_size }),
            )
            .await;
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a page of {page_size} of {} versions carries a NextToken: \
                     {first}",
                    built.expected_order.len()
                )
            })
            .to_string();
        let resumed = healthlake
            .ok(
                case,
                "ListDataTransformationProfileVersions",
                &json!({ "ProfileId": built.profile_id, "NextToken": token }),
            )
            .await;
        let resumed_versions: Vec<i32> = resumed["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| version_of(case, item))
            .collect();
        let expected: Vec<i32> = built
            .expected_order
            .iter()
            .skip(usize::try_from(page_size).unwrap_or(1))
            .copied()
            .collect();
        assert_eq!(
            resumed_versions, expected,
            "case {case:?}: the token from a page of {page_size} resumes at the next version, \
             got {resumed_versions:?}"
        );
    }
}

/// One write that lands BETWEEN two pages of a single walk.
#[derive(Debug, Clone, Copy)]
enum MidWalkWrite {
    /// Nothing is written between the pages — the control row.
    Nothing,
    /// `UpdateDataTransformationProfile` rewrites the DRAFT, restamping its
    /// `LastUpdatedAt` and replacing its `ChangeDescription`. This is the write
    /// a chronology measured on `LastUpdatedAt` would move to the front of the
    /// listing, shifting every entry after it.
    RewriteDraft,
    /// `PublishDataTransformationProfile` mints a version AFTER the walk began,
    /// so the new version belongs ahead of every entry the walk has served and
    /// the walk itself serves the versions it started with.
    PublishNewVersion,
}

impl MidWalkWrite {
    /// How a row names itself in a failure message.
    fn label(self) -> &'static str {
        match self {
            MidWalkWrite::Nothing => "no write between two pages",
            MidWalkWrite::RewriteDraft => "a DRAFT rewrite between two pages",
            MidWalkWrite::PublishNewVersion => "a publish between two pages",
        }
    }
}

/// Resume a walk from `token` and report every version the rest of it serves,
/// page by page at `max_results`, until a page hands out no token.
async fn resume_walk(
    healthlake: &Profiles,
    case: &str,
    profile_id: &str,
    max_results: i32,
    token: String,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut next = Some(token);
    let mut pages = 0usize;

    while let Some(current) = next {
        let body = healthlake
            .ok(
                case,
                "ListDataTransformationProfileVersions",
                &json!({
                    "ProfileId": profile_id,
                    "MaxResults": max_results,
                    "NextToken": current,
                }),
            )
            .await;
        items.extend(body["Items"].as_array().cloned().unwrap_or_default());
        pages += 1;
        // A profile holds at most its DRAFT and the modelled `ProfileVersion`
        // maximum of published versions, so at worst one page per version plus
        // one terminating page.
        let ceiling = usize::try_from(MAX_PROFILE_VERSION).unwrap_or(99) + 2;
        assert!(
            pages <= ceiling,
            "case {case:?}: paging must terminate, got {pages} pages"
        );
        next = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    items
}

/// The pages PARTITION the listing even when the profile is WRITTEN TO between
/// two of them: whichever write lands mid-walk, the walk serves each version the
/// profile held when it began exactly once, in the one order — nothing repeated
/// and nothing skipped.
///
/// The rows differ along the write that lands mid-walk (none, a DRAFT rewrite, a
/// publish) and along the page size the walk is read at, so each row's walk
/// spans a different number of pages.
#[tokio::test]
async fn a_write_between_two_pages_repeats_and_skips_nothing() {
    let healthlake = Profiles::new();
    let rewritten_draft = &[("mapping-config.yaml", "tableName: people")];

    for write in [
        MidWalkWrite::Nothing,
        MidWalkWrite::RewriteDraft,
        MidWalkWrite::PublishNewVersion,
    ] {
        for page_size in 1..=3 {
            let case = write.label();
            // Three publishes and then a DRAFT rewrite: the DRAFT is the version
            // most recently CHANGED while being the version created first, which
            // is exactly the state the two chronologies disagree about.
            let built = build(
                &healthlake,
                &ListingCase {
                    case,
                    profile_name: "csv-mid-walk-write",
                    source_format: "CSV",
                    steps: vec![
                        Step::Publish(Some("v1")),
                        Step::Publish(None),
                        Step::Publish(Some("v3")),
                        Step::UpdateDraft(rewritten_draft, Some("draft ahead of v3")),
                    ],
                },
            )
            .await;
            let row = format!("{case} at MaxResults {page_size}");

            let first = healthlake
                .ok(
                    &row,
                    "ListDataTransformationProfileVersions",
                    &json!({ "ProfileId": built.profile_id, "MaxResults": page_size }),
                )
                .await;
            let token = first["NextToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {row:?}: a page of {page_size} of {} versions carries a \
                         NextToken: {first}",
                        built.expected_order.len()
                    )
                })
                .to_string();
            let mut served: Vec<Value> = first["Items"].as_array().cloned().unwrap_or_default();

            // The write that lands between the page above and the rest of the
            // walk below.
            match write {
                MidWalkWrite::Nothing => {}
                MidWalkWrite::RewriteDraft => {
                    healthlake
                        .ok(
                            &row,
                            "UpdateDataTransformationProfile",
                            &json!({
                                "ProfileId": built.profile_id,
                                "ProfileMapping": request_mapping(&mapping(&[(
                                    "mapping-config.yaml",
                                    "tableName: identified_people",
                                )])),
                                "ChangeDescription": "rewritten mid-walk",
                            }),
                        )
                        .await;
                }
                MidWalkWrite::PublishNewVersion => {
                    healthlake
                        .ok(
                            &row,
                            "PublishDataTransformationProfile",
                            &json!({
                                "ProfileId": built.profile_id,
                                "SourceFormat": "CSV",
                                "ChangeDescription": "published mid-walk",
                            }),
                        )
                        .await;
                }
            }

            served.extend(
                resume_walk(
                    &healthlake,
                    &row,
                    &built.profile_id,
                    page_size,
                    token,
                )
                .await,
            );
            let versions: Vec<i32> = served.iter().map(|item| version_of(&row, item)).collect();
            assert_eq!(
                versions, built.expected_order,
                "case {row:?}: the walk must serve the versions the profile held when it \
                 began, each exactly once and in the one order, got {versions:?}"
            );

            // Each page is read from the live profile, so the DRAFT served after
            // a mid-walk rewrite carries what that rewrite wrote.
            if matches!(write, MidWalkWrite::RewriteDraft) {
                let draft = served
                    .iter()
                    .find(|item| version_of(&row, item) == 0)
                    .unwrap_or_else(|| panic!("case {row:?}: the walk serves the DRAFT"));
                assert_eq!(
                    draft.get("ChangeDescription").and_then(Value::as_str),
                    Some("rewritten mid-walk"),
                    "case {row:?}: the DRAFT summary must carry the mid-walk rewrite's \
                     ChangeDescription, got {draft}"
                );
                let published = served
                    .iter()
                    .find(|item| version_of(&row, item) == 3)
                    .unwrap_or_else(|| panic!("case {row:?}: the walk serves version 3"));
                assert!(
                    last_updated_at(&row, draft) >= last_updated_at(&row, published),
                    "case {row:?}: the rewrite restamped the DRAFT, so its LastUpdatedAt is \
                     no older than the publish it follows, got {draft} against {published}"
                );
            }

            // A walk started AFTER the mid-walk publish does report the version
            // it minted, at the front: nothing was lost, it was simply not part
            // of the listing the earlier walk was partitioning.
            if matches!(write, MidWalkWrite::PublishNewVersion) {
                let fresh = list_every_page(&healthlake, &row, &built.profile_id, None).await;
                let fresh_versions: Vec<i32> =
                    fresh.iter().map(|item| version_of(&row, item)).collect();
                let expected: Vec<i32> = (0..=4).rev().collect();
                assert_eq!(
                    fresh_versions, expected,
                    "case {row:?}: a walk started after the publish reports the version it \
                     minted first, got {fresh_versions:?}"
                );
            }
        }
    }
}

/// A profile that holds every version the modelled `ProfileVersion` range allows
/// is listed in full: `1 + MAX_PROFILE_VERSION` entries, which is exactly what
/// one page of the modelled `MaxResults` maximum holds, so no token is handed
/// out.
#[tokio::test]
async fn a_profile_at_the_modelled_version_maximum_lists_in_one_page() {
    let healthlake = Profiles::new();
    let case = "every version the modelled range allows";
    let built = build(
        &healthlake,
        &ListingCase {
            case,
            profile_name: "csv-every-version",
            source_format: "CSV",
            steps: (0..MAX_PROFILE_VERSION).map(|_| Step::Publish(None)).collect(),
        },
    )
    .await;

    let body = healthlake
        .ok(
            case,
            "ListDataTransformationProfileVersions",
            &json!({ "ProfileId": built.profile_id, "MaxResults": MAX_PAGE_SIZE }),
        )
        .await;
    let versions: Vec<i32> = body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| version_of(case, item))
        .collect();
    // Published newest first, so the highest version comes first and the DRAFT,
    // untouched since the create, comes last.
    let expected: Vec<i32> = (0..=MAX_PROFILE_VERSION).rev().collect();
    assert_eq!(
        versions, expected,
        "case {case:?}: the listing holds the DRAFT and every published version, \
         got {versions:?}"
    );
    assert_eq!(
        body.get("NextToken"),
        None,
        "case {case:?}: a page that holds the whole listing hands out no token, got {body}"
    );
}

/// Every refused listing is refused with the modelled status and code, and
/// reports nothing.
#[tokio::test]
async fn refused_listings_report_nothing() {
    let healthlake = Profiles::new();
    let case = "refusals";
    let built = build(
        &healthlake,
        &ListingCase {
            case,
            profile_name: "csv-refusals",
            source_format: "CSV",
            steps: vec![Step::Publish(Some("only version"))],
        },
    )
    .await;

    // A well-formed `ProfileId` no record holds: the profile, not a version of
    // it, is what is missing.
    let (status, code, payload) = healthlake
        .refused_body(
            case,
            "ListDataTransformationProfileVersions",
            &json!({ "ProfileId": "0".repeat(32) }),
        )
        .await;
    assert_eq!(
        (status, code.as_str(), modelled_message(&payload)),
        (
            wire_http_status(404),
            "ResourceNotFoundException",
            Some(profile_not_found_message(&"0".repeat(32)).as_str())
        ),
        "case {case:?}: listing a profile no record holds is refused with the sentence \
         naming that profile, got {payload}"
    );

    // Each bad request on a profile that DOES exist, so what is refused is the
    // request and not the profile: `MaxResults` outside the modelled 1..100
    // range, and tokens that resume nothing.
    let bad_requests = [
        (
            "MaxResults below the modelled minimum",
            json!({ "ProfileId": built.profile_id, "MaxResults": 0 }),
        ),
        (
            "MaxResults past the modelled maximum",
            json!({ "ProfileId": built.profile_id, "MaxResults": MAX_PAGE_SIZE + 1 }),
        ),
        (
            "a NextToken naming a version this profile does not hold",
            json!({ "ProfileId": built.profile_id, "NextToken": MAX_PROFILE_VERSION.to_string() }),
        ),
        (
            "a NextToken that is not a version number at all",
            json!({ "ProfileId": built.profile_id, "NextToken": "not-a-version" }),
        ),
    ];
    for (row, request) in bad_requests {
        let (status, code) = healthlake
            .refused(row, "ListDataTransformationProfileVersions", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (wire_http_status(400), "ValidationException"),
            "case {row:?}: the request must be refused as invalid"
        );
    }

    // The refusals changed nothing: the listing still reports exactly the
    // versions the profile held before them.
    let items = list_every_page(&healthlake, case, &built.profile_id, None).await;
    let versions: Vec<i32> = items.iter().map(|item| version_of(case, item)).collect();
    assert_eq!(
        versions, built.expected_order,
        "case {case:?}: the refusals left the listing as it was, got {versions:?}"
    );
}
