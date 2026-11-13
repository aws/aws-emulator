// SPDX-License-Identifier: CC0-1.0

//! The identity and the body of a custom-data-identifier response, end to end
//! over the wire.
//!
//! Each case drives the production `MultiServiceRouter` through the harness
//! `macie_session_and_custom_data_identifiers` builds, so what is pinned is the
//! HTTP behaviour and the response BODIES rather than a handler called
//! directly.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! request:
//!
//! * a minted id carries the form the service issues for THAT resource — a
//!   custom data identifier's is a canonical UUID — and the id a create reports
//!   is the one every later report of the resource carries, unchanged, inside
//!   its ARN too;
//! * a list member a read always serializes is reported PRESENT, as an empty
//!   array when the resource holds nothing in it, rather than omitted;
//! * a member carrying a documented default is stored with that default when a
//!   create names none, so every later read reports the configuration that is
//!   in effect — `severityLevels` on the same footing as `maximumMatchDistance`.

use smithy_server_runtime::MultiServiceRouter;

use super::macie_session_and_custom_data_identifiers::{
    assert_id_grammar, call_in, default_severity_levels_json, emulator, enable_in,
};

/// The registry's URI, from `CreateCustomDataIdentifier`'s `@http` binding.
const IDENTIFIERS_URI: &str = "/custom-data-identifiers";

/// The account segment every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The regions the cases sign for: the rule is about the resource, not about
/// one region, so every table below runs in each of them.
const REGIONS: [&str; 2] = ["us-west-2", "eu-central-1"];

/// Create an identifier and hand back the id it minted.
async fn create(
    router: &MultiServiceRouter,
    region: &str,
    body: &serde_json::Value,
    case: &str,
) -> String {
    let (status, reply) = call_in(router, region, "POST", IDENTIFIERS_URI, &body.to_string()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a create inside every bound must answer 200, got {status} with {reply:?}"
    );
    reply["customDataIdentifierId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create must report an id, got {reply:?}"))
        .to_string()
}

/// Read one identifier back.
async fn read(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> serde_json::Value {
    let (status, record) = call_in(
        router,
        region,
        "GET",
        &format!("{IDENTIFIERS_URI}/{id}"),
        "",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a read of a stored identifier must answer 200, got {status} with \
         {record:?}"
    );
    record
}

/// The summaries the region's registry enumerates.
async fn summaries(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
) -> Vec<serde_json::Value> {
    let (status, page) = call_in(
        router,
        region,
        "POST",
        &format!("{IDENTIFIERS_URI}/list"),
        "{}",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the enumeration must answer 200, got {status} with {page:?}"
    );
    page["items"].as_array().cloned().unwrap_or_default()
}

/// Every id a create mints is a canonical UUID, the form the service issues a
/// custom data identifier's id in; it is unique across creates; and it reaches
/// the read, the ARN and the enumeration unchanged. The rows differ along the
/// members the create carries and the region it addresses, none of which the id
/// format depends on.
#[tokio::test]
async fn a_minted_identifier_id_is_a_canonical_uuid_everything_else_carries_unchanged() {
    let router = emulator();

    let creates = [
        (
            "required members only",
            serde_json::json!({ "name": "minimal", "regex": "[0-9]{3}" }),
        ),
        (
            "with keywords and a description",
            serde_json::json!({
                "name": "worded",
                "regex": "[a-z]+",
                "description": "described",
                "keywords": ["ssn"],
            }),
        ),
        (
            "with every optional member",
            serde_json::json!({
                "name": "everything",
                "regex": "^[0-9]{3}-[0-9]{2}-[0-9]{4}$",
                "description": "all members",
                "keywords": ["ssn", "social security"],
                "ignoreWords": ["sample"],
                "maximumMatchDistance": 7,
                "severityLevels": [{ "severity": "HIGH", "occurrencesThreshold": 2 }],
                "tags": { "team": "security" },
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for region in REGIONS {
        enable_in(&router, region, "{}").await;
        for (name, body) in &creates {
            let case = format!("{name} in {region}");
            let id = create(&router, region, body, &case).await;
            assert_id_grammar(&id, &case);
            assert!(
                !minted.contains(&id),
                "case {case:?}: {id:?} was already minted for another create, among {minted:?}"
            );

            let record = read(&router, region, &id, &case).await;
            assert_eq!(
                record["id"].as_str(),
                Some(id.as_str()),
                "case {case:?}: the read must report the id the create minted, got {record:?}"
            );
            let expected_arn =
                format!("arn:aws:macie2:{region}:{ACCOUNT}:custom-data-identifier/{id}");
            assert_eq!(
                record["arn"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the ARN to carry the id unchanged \
                 ({expected_arn:?}), got {:?}",
                record["arn"]
            );

            let listed = summaries(&router, region, &case).await;
            let summary = listed
                .iter()
                .find(|item| item["id"].as_str() == Some(id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the enumeration must carry {id:?}, got {listed:?}")
                });
            assert_eq!(
                summary["arn"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?}: the summary must carry the same ARN as the read, got {summary:?}"
            );
            minted.push(id);
        }
    }
}

/// `keywords` and `ignoreWords` are PRESENT in every read, as the array that is
/// stored and as an empty array when nothing is. The rows span what a create
/// may say about a list member: nothing at all, an explicitly empty array, one
/// entry, and several — in each region.
#[tokio::test]
async fn the_list_members_of_a_read_are_present_even_when_empty() {
    let router = emulator();

    let cases: [(&str, serde_json::Value, serde_json::Value, serde_json::Value); 5] = [
        (
            "neither list named",
            serde_json::json!({ "name": "neither", "regex": "a" }),
            serde_json::json!([]),
            serde_json::json!([]),
        ),
        (
            "both lists named empty",
            serde_json::json!({
                "name": "empty",
                "regex": "a",
                "keywords": [],
                "ignoreWords": [],
            }),
            serde_json::json!([]),
            serde_json::json!([]),
        ),
        (
            "one keyword, no ignore words",
            serde_json::json!({ "name": "one-keyword", "regex": "a", "keywords": ["acct"] }),
            serde_json::json!(["acct"]),
            serde_json::json!([]),
        ),
        (
            "one ignore word, no keywords",
            serde_json::json!({ "name": "one-word", "regex": "a", "ignoreWords": ["sample"] }),
            serde_json::json!([]),
            serde_json::json!(["sample"]),
        ),
        (
            "several of each",
            serde_json::json!({
                "name": "several",
                "regex": "a",
                "keywords": ["ssn", "social security", "taxpayer id"],
                "ignoreWords": ["sample", "example"],
            }),
            serde_json::json!(["ssn", "social security", "taxpayer id"]),
            serde_json::json!(["sample", "example"]),
        ),
    ];

    for region in REGIONS {
        enable_in(&router, region, "{}").await;
        for (name, body, keywords, ignore_words) in &cases {
            let case = format!("{name} in {region}");
            let id = create(&router, region, body, &case).await;
            let record = read(&router, region, &id, &case).await;

            for (member, expected) in [("keywords", keywords), ("ignoreWords", ignore_words)] {
                assert_eq!(
                    record.get(member),
                    Some(expected),
                    "case {case:?}: {member} must be present as {expected:?}, got {:?}",
                    record.get(member)
                );
            }
        }
    }
}

/// `severityLevels` reports the severity configuration in EFFECT: the levels
/// the create named, or the service's own default level when it named none —
/// the same way `maximumMatchDistance` reports its documented default. The rows
/// span the level sets a create may send, from none to one per severity.
#[tokio::test]
async fn severity_levels_report_the_configuration_in_effect() {
    let router = emulator();
    let default_levels = default_severity_levels_json();

    let cases: [(&str, serde_json::Value, Vec<serde_json::Value>); 5] = [
        (
            "no severity levels named",
            serde_json::json!({ "name": "defaulted", "regex": "a" }),
            default_levels.clone(),
        ),
        (
            "an empty severity level array",
            serde_json::json!({ "name": "defaulted-empty", "regex": "a", "severityLevels": [] }),
            default_levels.clone(),
        ),
        (
            "one level",
            serde_json::json!({
                "name": "one-level",
                "regex": "a",
                "severityLevels": [{ "severity": "LOW", "occurrencesThreshold": 3 }],
            }),
            vec![serde_json::json!({ "severity": "LOW", "occurrencesThreshold": 3 })],
        ),
        (
            "two levels",
            serde_json::json!({
                "name": "two-levels",
                "regex": "a",
                "severityLevels": [
                    { "severity": "MEDIUM", "occurrencesThreshold": 10 },
                    { "severity": "HIGH", "occurrencesThreshold": 100 },
                ],
            }),
            vec![
                serde_json::json!({ "severity": "MEDIUM", "occurrencesThreshold": 10 }),
                serde_json::json!({ "severity": "HIGH", "occurrencesThreshold": 100 }),
            ],
        ),
        (
            "one level per severity",
            serde_json::json!({
                "name": "every-level",
                "regex": "a",
                "severityLevels": [
                    { "severity": "LOW", "occurrencesThreshold": 1 },
                    { "severity": "MEDIUM", "occurrencesThreshold": 2 },
                    { "severity": "HIGH", "occurrencesThreshold": 3 },
                ],
            }),
            vec![
                serde_json::json!({ "severity": "LOW", "occurrencesThreshold": 1 }),
                serde_json::json!({ "severity": "MEDIUM", "occurrencesThreshold": 2 }),
                serde_json::json!({ "severity": "HIGH", "occurrencesThreshold": 3 }),
            ],
        ),
    ];

    for region in REGIONS {
        enable_in(&router, region, "{}").await;
        for (name, body, expected) in &cases {
            let case = format!("{name} in {region}");
            let id = create(&router, region, body, &case).await;
            let record = read(&router, region, &id, &case).await;

            let mut reported = record["severityLevels"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: severityLevels must be present, got {:?}",
                        record.get("severityLevels")
                    )
                });
            let mut wanted = expected.clone();
            reported.sort_by_key(|level| level["severity"].as_str().unwrap_or("").to_string());
            wanted.sort_by_key(|level| level["severity"].as_str().unwrap_or("").to_string());
            assert_eq!(
                reported, wanted,
                "case {case:?}: expected the levels in effect {wanted:?}, got {reported:?}"
            );

            // A second read reports the same configuration: the default is
            // STORED, not synthesised per call.
            let again = read(&router, region, &id, &case).await;
            assert_eq!(
                again["severityLevels"], record["severityLevels"],
                "case {case:?}: two reads must report the same severity levels, got {:?} then {:?}",
                record["severityLevels"], again["severityLevels"]
            );
        }
    }
}
