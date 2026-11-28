// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `ListDataTransformationProfiles` over the real
//! awsJson1_0 wire, cross-checked against `GetDataTransformationProfile`.
//!
//! The rule under test: the listing reports every data transformation profile the
//! service holds whose source data format is the one the request names — ONCE
//! each, never one entry per version — through the modelled summary carrying that
//! profile's latest version number, its metadata, and the instant the profile
//! itself last changed; and it serves one stable order a page at a time, each page
//! holding at most `MaxResults` entries and carrying a `NextToken` exactly when
//! profiles remain after it. A profile of another source format is not in the
//! listing, a deleted profile leaves it, a bad request is refused, and the listing
//! writes nothing, so two identical calls answer identically.
//!
//! The rows differ along what the rule generalises over: the source format, how
//! many profiles there are of each, how many versions a profile holds (from a
//! DRAFT that was never published to several published versions), whether the
//! profile carries a `ProfileDescription`, and WHICH version was written last — a
//! DRAFT rewritten after a publish leaves the latest version number behind the
//! profile's own last-updated instant, so the two come apart.
//!
//! Every call below goes through the shared [`Profiles`] harness, i.e.
//! `MultiServiceRouter::dispatch` — the same entry point `core/router` serves — so
//! routing, deserialization, the modelled constraints and the error envelope are
//! all exercised, not bypassed.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::ops::types::SOURCE_FORMAT_VALUES;
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;

use crate::data_transformation_profile::{mapping, profile_mapping_source, Profiles};
use crate::fhir_datastore_lifecycle::string_at;
use crate::publish_data_transformation_profile::request_mapping;

/// The largest page the modelled `MaxResults` range (1..100) allows, which is
/// also the page a request naming no `MaxResults` gets.
const MAX_PAGE_SIZE: i32 = 100;

/// One step of a row's history, after the create that leaves the DRAFT behind.
///
/// Each step writes exactly ONE version, so the last step is the one that fixes
/// what the profile's last-updated instant must be, and the publishes alone fix
/// its latest version number.
#[derive(Debug, Clone)]
enum Step {
    /// `PublishDataTransformationProfile`: snapshots the DRAFT into the next
    /// published version, which becomes the profile's latest version.
    Publish,
    /// `UpdateDataTransformationProfile`: rewrites the DRAFT in place, which
    /// changes the profile without adding a version.
    UpdateDraft(&'static [(&'static str, &'static str)]),
}

/// One listing row: the profile to build and the history to build it with.
struct ProfileCase {
    case: &'static str,
    profile_name: &'static str,
    /// The modelled `SourceFormat` of the profile (one of `CCDA` | `CSV`).
    source_format: &'static str,
    description: Option<&'static str>,
    steps: Vec<Step>,
}

/// The listing table. The rule generalises over the source format, how many
/// versions a profile holds, whether it carries a description and which version
/// was written last, so the rows differ along exactly those — and both modelled
/// source formats are represented more than once, so the filter has something to
/// leave out as well as something to report.
fn profile_cases() -> Vec<ProfileCase> {
    vec![
        ProfileCase {
            case: "CSV, never published, no description",
            profile_name: "csv-draft-only",
            source_format: "CSV",
            description: None,
            steps: Vec::new(),
        },
        ProfileCase {
            case: "CSV, one publish, described",
            profile_name: "csv-one-publish",
            source_format: "CSV",
            description: Some("patients only"),
            steps: vec![Step::Publish],
        },
        ProfileCase {
            case: "CCDA, three publishes",
            profile_name: "ccda-three-publishes",
            source_format: "CCDA",
            description: Some("sections, with ünïcode and punctuation: {}/[]"),
            steps: vec![Step::Publish, Step::Publish, Step::Publish],
        },
        ProfileCase {
            case: "CCDA, a publish then a DRAFT rewrite, so the DRAFT was written last",
            profile_name: "ccda-draft-rewritten-last",
            source_format: "CCDA",
            description: None,
            steps: vec![
                Step::Publish,
                Step::UpdateDraft(&[("sections/problems.map", "Condition.code <- value")]),
            ],
        },
        ProfileCase {
            case: "CSV, DRAFT rewrites and publishes interleaved",
            profile_name: "csv-interleaved",
            source_format: "CSV",
            description: Some("interleaved history"),
            steps: vec![
                Step::UpdateDraft(&[("mapping-config.yaml", "tableName: people")]),
                Step::Publish,
                Step::UpdateDraft(&[("mapping-config.yaml", "tableName: people_and_vitals")]),
                Step::Publish,
            ],
        },
        ProfileCase {
            case: "CSV, a fourth profile of this format, so paging has something to split",
            profile_name: "csv-fourth",
            source_format: "CSV",
            description: Some("the fourth CSV profile"),
            steps: vec![Step::Publish, Step::Publish],
        },
    ]
}

/// What a row's history left behind: the profile it built and what the listing
/// must report for it.
struct Built {
    case: &'static str,
    profile_id: String,
    source_format: &'static str,
    profile_name: &'static str,
    description: Option<&'static str>,
    /// The latest version number: the DRAFT's 0 until something is published,
    /// then the number of publishes.
    expected_version: i32,
    /// The `LastUpdatedAt` the LAST call that wrote the profile answered with —
    /// the instant the profile itself last changed, whichever version that call
    /// wrote.
    expected_last_updated_at: Value,
    /// The `TargetFormat` the create reported, so the expectation is the model's
    /// single target format rather than a literal written here.
    expected_target_format: String,
}

/// Run `row`'s history and derive what the listing must report for it.
async fn build(healthlake: &Profiles, row: &ProfileCase) -> Built {
    let case = row.case;
    let mut request = json!({
        "ProfileName": row.profile_name,
        "SourceFormat": row.source_format,
        "Source": profile_mapping_source(&mapping(&[(
            "mapping-config.yaml",
            "tableName: patients",
        )])),
    });
    if let Some(text) = row.description {
        request["ProfileDescription"] = json!(text);
    }
    let created = healthlake
        .ok(case, "CreateDataTransformationProfile", &request)
        .await;
    let profile_id = string_at(&created, "ProfileId");
    let expected_target_format = string_at(&created, "TargetFormat");
    // The create wrote the DRAFT, so until a step writes something else the
    // profile last changed when it was created.
    let mut expected_last_updated_at = created["LastUpdatedAt"].clone();
    let mut expected_version = 0i32;

    for step in &row.steps {
        let response = match step {
            Step::Publish => {
                expected_version += 1;
                healthlake
                    .ok(
                        case,
                        "PublishDataTransformationProfile",
                        &json!({
                            "ProfileId": profile_id,
                            "SourceFormat": row.source_format,
                        }),
                    )
                    .await
            }
            Step::UpdateDraft(content) => {
                healthlake
                    .ok(
                        case,
                        "UpdateDataTransformationProfile",
                        &json!({
                            "ProfileId": profile_id,
                            "ProfileMapping": request_mapping(&mapping(content)),
                        }),
                    )
                    .await
            }
        };
        // Every one of these calls reports the instant it wrote, so the last
        // step's answer is when the profile last changed.
        expected_last_updated_at = response["LastUpdatedAt"].clone();
    }

    Built {
        case,
        profile_id,
        source_format: row.source_format,
        profile_name: row.profile_name,
        description: row.description,
        expected_version,
        expected_last_updated_at,
        expected_target_format,
    }
}

/// The `ProfileId` of one reported summary.
fn profile_id_of(case: &str, item: &Value) -> String {
    item["ProfileId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: every summary carries a ProfileId: {item}"))
        .to_string()
}

/// Every page of the listing at `source_format` and `max_results`, flattened,
/// with each page's size and token checked as it arrives: a page holds at most
/// `MaxResults` entries, and carries a `NextToken` only when it is full and
/// profiles remain after it.
async fn list_every_page(
    healthlake: &Profiles,
    case: &str,
    source_format: &str,
    max_results: Option<i32>,
) -> Vec<Value> {
    let limit = usize::try_from(max_results.unwrap_or(MAX_PAGE_SIZE)).unwrap_or(1);
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let mut request = json!({ "SourceFormat": source_format });
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(next) = token.as_deref() {
            request["NextToken"] = json!(next);
        }
        let body = healthlake
            .ok(case, "ListDataTransformationProfiles", &request)
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
        let next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert!(
            next_token.is_none() || page.len() == limit,
            "case {case:?}: a page that carries a NextToken is full, got {} of {limit}: {body}",
            page.len()
        );
        items.extend(page);
        pages += 1;
        // One page per profile the table builds plus one terminating page is the
        // worst case, so paging that does not terminate is caught rather than
        // looping forever.
        let ceiling = profile_cases().len() + 2;
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

/// Every profile this table builds, read back through the listing of its own
/// source format: one entry per profile, at its latest version, whichever page
/// size the listing is read at.
#[tokio::test]
async fn the_listing_reports_every_profile_of_the_named_source_format() {
    let healthlake = Profiles::new();
    let mut built: Vec<Built> = Vec::new();
    for row in profile_cases() {
        built.push(build(&healthlake, &row).await);
    }

    // The source formats come from the modelled `SourceFormat` enum — the value
    // set `smithy_query(query="shape",shape="SourceFormat")` reports — so the
    // listing is read at EVERY format the model declares, including one the table
    // did not have to cite.
    for source_format in SOURCE_FORMAT_VALUES {
        let case = *source_format;
        let expected: Vec<&Built> = built
            .iter()
            .filter(|row| row.source_format == *source_format)
            .collect();
        let expected_ids: BTreeSet<String> =
            expected.iter().map(|row| row.profile_id.clone()).collect();

        // One stable order, independent of the page size it is read at: the rule
        // holds at the bounds of the modelled range (1 and 100), at sizes that
        // split the profiles across several pages, and with `MaxResults` absent.
        let mut orders: Vec<Vec<String>> = Vec::new();
        for max_results in [None, Some(1), Some(2), Some(3), Some(MAX_PAGE_SIZE)] {
            let items = list_every_page(&healthlake, case, source_format, max_results).await;
            let reported: Vec<String> = items
                .iter()
                .map(|item| profile_id_of(case, item))
                .collect();
            let reported_ids: BTreeSet<String> = reported.iter().cloned().collect();
            assert_eq!(
                reported_ids, expected_ids,
                "case {case:?}: at MaxResults {max_results:?} the listing reports exactly the \
                 profiles of this source format, got {reported:?}"
            );
            assert_eq!(
                reported.len(),
                expected_ids.len(),
                "case {case:?}: at MaxResults {max_results:?} every profile is reported ONCE, \
                 got {reported:?}"
            );
            orders.push(reported);

            for item in &items {
                let id = profile_id_of(case, item);
                let row = expected
                    .iter()
                    .find(|row| row.profile_id == id)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the listing reported an unknown profile: {item}")
                    });
                let row_case = row.case;
                assert_eq!(
                    item["Version"].as_i64(),
                    Some(i64::from(row.expected_version)),
                    "case {row_case:?}: the summary carries the profile's LATEST version \
                     number, got {item}"
                );
                assert_eq!(
                    item["SourceFormat"].as_str(),
                    Some(row.source_format),
                    "case {row_case:?}: the summary carries the profile's source format, \
                     got {item}"
                );
                assert_eq!(
                    item["TargetFormat"].as_str(),
                    Some(row.expected_target_format.as_str()),
                    "case {row_case:?}: the summary carries the profile's target format, \
                     got {item}"
                );
                assert_eq!(
                    item["ProfileName"].as_str(),
                    Some(row.profile_name),
                    "case {row_case:?}: the summary carries the profile's name, got {item}"
                );
                assert_eq!(
                    item.get("ProfileDescription").and_then(Value::as_str),
                    row.description,
                    "case {row_case:?}: the summary carries the description the create gave \
                     the profile, got {item}"
                );
                assert_eq!(
                    item["LastUpdatedAt"], row.expected_last_updated_at,
                    "case {row_case:?}: the summary carries the instant the profile last \
                     changed, got {item}"
                );

                // The summary and the version it names agree, member for member,
                // with what the profile reads back as at that version.
                let fetched = healthlake
                    .ok(
                        row_case,
                        "GetDataTransformationProfile",
                        &json!({
                            "ProfileId": row.profile_id,
                            "ProfileVersion": row.expected_version,
                        }),
                    )
                    .await;
                for member in [
                    "ProfileId",
                    "Version",
                    "SourceFormat",
                    "TargetFormat",
                    "ProfileName",
                    "ProfileDescription",
                ] {
                    assert_eq!(
                        item.get(member),
                        fetched.get(member),
                        "case {row_case:?}: the summary's {member} must match the version read \
                         back, got {item} against {fetched}"
                    );
                }
            }
        }

        for order in orders.windows(2) {
            assert_eq!(
                order[0], order[1],
                "case {case:?}: the listing serves ONE order whatever the page size, got \
                 {:?} then {:?}",
                order[0], order[1]
            );
        }

        // The listing writes nothing, so a second identical call answers
        // identically.
        let first = list_every_page(&healthlake, case, source_format, None).await;
        let second = list_every_page(&healthlake, case, source_format, None).await;
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
    for row in profile_cases() {
        build(&healthlake, &row).await;
    }
    let case = "resuming a page of the CSV listing";
    let whole: Vec<String> = list_every_page(&healthlake, case, "CSV", None)
        .await
        .iter()
        .map(|item| profile_id_of(case, item))
        .collect();
    assert!(
        whole.len() >= 4,
        "case {case:?}: the table must build several CSV profiles to page through, got {whole:?}"
    );

    for page_size in 1..=3 {
        let first = healthlake
            .ok(
                case,
                "ListDataTransformationProfiles",
                &json!({ "SourceFormat": "CSV", "MaxResults": page_size }),
            )
            .await;
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a page of {page_size} of {} profiles carries a NextToken: \
                     {first}",
                    whole.len()
                )
            })
            .to_string();
        let resumed = healthlake
            .ok(
                case,
                "ListDataTransformationProfiles",
                &json!({ "SourceFormat": "CSV", "NextToken": token }),
            )
            .await;
        let resumed_ids: Vec<String> = resumed["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| profile_id_of(case, item))
            .collect();
        let expected: Vec<String> = whole
            .iter()
            .skip(usize::try_from(page_size).unwrap_or(1))
            .cloned()
            .collect();
        assert_eq!(
            resumed_ids, expected,
            "case {case:?}: the token from a page of {page_size} resumes at the next profile, \
             got {resumed_ids:?}"
        );
    }
}

/// A source format no stored profile converts from reports an empty listing and
/// no token — not the profiles of the other format, and not a refusal.
#[tokio::test]
async fn a_format_no_profile_converts_from_lists_nothing() {
    let healthlake = Profiles::new();

    // Before anything is created, every modelled format lists nothing.
    for source_format in SOURCE_FORMAT_VALUES {
        let case = *source_format;
        let body = healthlake
            .ok(
                case,
                "ListDataTransformationProfiles",
                &json!({ "SourceFormat": source_format }),
            )
            .await;
        assert_eq!(
            body["Items"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a service holding no profile reports an empty listing, got {body}"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {case:?}: an empty listing hands out no token, got {body}"
        );
    }

    let built = build(
        &healthlake,
        &ProfileCase {
            case: "one CSV profile only",
            profile_name: "csv-only",
            source_format: "CSV",
            description: None,
            steps: vec![Step::Publish],
        },
    )
    .await;

    let csv = list_every_page(&healthlake, built.case, "CSV", None).await;
    assert_eq!(
        csv.iter()
            .map(|item| profile_id_of(built.case, item))
            .collect::<Vec<String>>(),
        vec![built.profile_id.clone()],
        "case {:?}: the profile is in the listing of its own format, got {csv:?}",
        built.case
    );
    let other = list_every_page(&healthlake, built.case, "CCDA", None).await;
    assert!(
        other.is_empty(),
        "case {:?}: a profile of another source format is not in this listing, got {other:?}",
        built.case
    );
}

/// A profile that is deleted leaves the listing, so the listing reports the
/// records the service holds now rather than the ones it once held.
#[tokio::test]
async fn a_deleted_profile_leaves_the_listing() {
    let healthlake = Profiles::new();
    let case = "deleting one of two profiles of a format";
    let kept = build(
        &healthlake,
        &ProfileCase {
            case,
            profile_name: "csv-kept",
            source_format: "CSV",
            description: Some("stays"),
            steps: vec![Step::Publish],
        },
    )
    .await;
    let removed = build(
        &healthlake,
        &ProfileCase {
            case,
            profile_name: "csv-removed",
            source_format: "CSV",
            description: None,
            steps: Vec::new(),
        },
    )
    .await;

    healthlake
        .ok(
            case,
            "DeleteDataTransformationProfile",
            &json!({ "ProfileId": removed.profile_id }),
        )
        .await;

    let items = list_every_page(&healthlake, case, "CSV", None).await;
    let reported: Vec<String> = items
        .iter()
        .map(|item| profile_id_of(case, item))
        .collect();
    assert_eq!(
        reported,
        vec![kept.profile_id.clone()],
        "case {case:?}: only the profile that was not deleted is reported, got {reported:?}"
    );
}

/// Every refused listing is refused with the modelled status and code, and
/// reports nothing.
#[tokio::test]
async fn refused_listings_report_nothing() {
    let healthlake = Profiles::new();
    let mut built: Vec<Built> = Vec::new();
    for row in profile_cases() {
        built.push(build(&healthlake, &row).await);
    }
    let before = list_every_page(&healthlake, "refusals", "CSV", None).await;
    let ccda_id = built
        .iter()
        .find(|row| row.source_format == "CCDA")
        .map(|row| row.profile_id.clone())
        .expect("the table builds at least one CCDA profile");

    let bad_requests = [
        (
            "MaxResults below the modelled minimum",
            json!({ "SourceFormat": "CSV", "MaxResults": 0 }),
        ),
        (
            "MaxResults past the modelled maximum",
            json!({ "SourceFormat": "CSV", "MaxResults": MAX_PAGE_SIZE + 1 }),
        ),
        (
            "a SourceFormat outside the modelled value set",
            json!({ "SourceFormat": "HL7V2" }),
        ),
        (
            "no SourceFormat at all, which the model requires",
            json!({ "MaxResults": 1 }),
        ),
        (
            "a NextToken naming no profile at all",
            json!({ "SourceFormat": "CSV", "NextToken": "0".repeat(32) }),
        ),
        (
            "a NextToken naming a profile of the OTHER source format",
            json!({ "SourceFormat": "CSV", "NextToken": ccda_id }),
        ),
    ];
    for (row, request) in bad_requests {
        let (status, code) = healthlake
            .refused(row, "ListDataTransformationProfiles", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (wire_http_status(400), "ValidationException"),
            "case {row:?}: the request must be refused as invalid"
        );
    }

    // The refusals changed nothing: the listing still reports exactly the
    // profiles it reported before them.
    let after = list_every_page(&healthlake, "refusals", "CSV", None).await;
    assert_eq!(
        after, before,
        "case \"refusals\": the refused requests left the listing as it was"
    );
}
