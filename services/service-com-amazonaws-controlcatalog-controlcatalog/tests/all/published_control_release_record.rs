// SPDX-License-Identifier: CC0-1.0

//! The published text and release instant a catalog control is served with.
//!
//! The rule under test: for a control the Control Catalog publishes, `Name`,
//! `Description` and `CreateTime` are the catalog's own published content for that
//! control ARN — the display name and the console description worded by the surface that
//! publishes the control, and the instant that control was released — and the catalog is
//! an Amazon Web Services-published, account- and Region-independent read-only dataset
//! (its ARNs carry neither account nor Region), so one ARN carries one name, one
//! description and one release instant for every caller, on every read and on every run.
//! `Description` is "[a] description of the control, as it may appear in the console",
//! and `CreateTime` "notes the time when the control was released (start of its life) as
//! a governance capability in Amazon Web Services"
//! (`smithy_query(query="shape", shape="ControlSummary",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`).
//! Two consequences each have a test below: a control's release instant is a property of
//! the control rather than of its position in any listing, so controls released together
//! carry ONE instant; and nothing a read does may re-word or re-date a control.
//!
//! Each test is a TABLE rather than one control picked by hand. The rows of the published
//! table differ in publishing surface (Control Tower's own control description against
//! the implementing AWS Config managed rule's), in whether the description restates the
//! name, in whether the release instant is shared with another control, and the whole
//! catalog — including controls no recorded page named — is covered by the stability and
//! read-only tests, which take every control the service serves.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::control_catalog::{catalog_arn, drain, Emulator};

/// One control's published catalog record.
///
/// Source: the Control Catalog's own published control list — the ARN
/// `arn:{PARTITION}:controlcatalog:::control/{identifier}` it names the control by, the
/// display name and console description it publishes for it, and the instant it records
/// as the control's release time (`CreateTime`), in seconds since the Unix epoch.
struct PublishedRecord {
    identifier: &'static str,
    name: &'static str,
    description: &'static str,
    release_time: i64,
}

/// Controls whose published record the catalog records, spanning the publishing surfaces
/// the catalog serves: a control Control Tower publishes carries Control Tower's own
/// control description, a control published over an AWS Config managed rule carries that
/// rule's own title and description, and a control Security Hub implements carries the
/// text Security Hub publishes for it.
const PUBLISHED_RECORDS: &[PublishedRecord] = &[
    PublishedRecord {
        identifier: "d5kdbugbadk3r481vtwnvoynv",
        name: "Checks that there is at least one multi-region AWS CloudTrail",
        description: "Checks that there is at least one multi-region AWS CloudTrail. The \
                      rule is non-compliant if the trails do not match input parameters",
        release_time: 1_539_993_600,
    },
    PublishedRecord {
        identifier: "d4wgeffz6izrb627c2yb8nq8d",
        name: "Checks if S3 buckets have default encryption enabled or have bucket \
               policies that explicitly deny put-object requests without server side \
               encryption using AES-256 or AWS KMS",
        description: "Checks if S3 buckets have default encryption enabled or have bucket \
                      policies that explicitly deny put-object requests without server \
                      side encryption using AES-256 or AWS KMS. The rule is \
                      NON_COMPLIANT if an S3 bucket is not encrypted by default.",
        release_time: 1_515_110_400,
    },
    PublishedRecord {
        identifier: "e5efjudq0b347dgxn86o8fg2o",
        name: "Checks if S3 buckets have policies that require requests to use SSL/TLS",
        description: "Checks if S3 buckets have policies that require requests to use \
                      SSL/TLS. The rule is NON_COMPLIANT if any S3 bucket has policies \
                      allowing HTTP requests.",
        release_time: 1_479_772_800,
    },
    PublishedRecord {
        identifier: "22484xhzccf9twis5gmdt7uic",
        name: "Checks if Amazon Virtual Private Cloud (Amazon VPC) flow logs are found \
               and enabled for all Amazon VPCs",
        description: "Checks if Amazon Virtual Private Cloud (Amazon VPC) flow logs are \
                      found and enabled for all Amazon VPCs. The rule is NON_COMPLIANT \
                      if flow logs are not enabled for at least one Amazon VPC.",
        release_time: 1_536_710_400,
    },
    PublishedRecord {
        identifier: "8sw3pbid15t9cbww8d2w2qwgf",
        name: "Disallow public read access to S3 buckets",
        description: "Secure access to data stored in Amazon S3 buckets by disallowing \
                      public reads.",
        release_time: 1_542_240_000,
    },
    PublishedRecord {
        identifier: "9jzlkwys54gzgsx3pmkqnx8l1",
        name: "Disallow public read access to log archive",
        description: "Control access to your log archive's Amazon S3 bucket by \
                      disallowing public read access.",
        release_time: 1_542_240_000,
    },
    PublishedRecord {
        identifier: "9j9nwxj789d82sypnukhyyowy",
        name: "Disallow public write access to S3 buckets",
        description: "Secure access to data stored in Amazon S3 buckets by disallowing \
                      public writes.",
        release_time: 1_542_240_000,
    },
    PublishedRecord {
        identifier: "ci9ov6vzjoi6vzkajgademvvo",
        name: "Disallow public write access to log archive",
        description: "Control access to your log archive's Amazon S3 bucket by \
                      disallowing public write access.",
        release_time: 1_542_240_000,
    },
    PublishedRecord {
        identifier: "1h4eyqyyonp19dlrreqf1i3w0",
        name: "Disallow public access to RDS database snapshots",
        description: "Secure your Amazon RDS database snapshots by disallowing public \
                      access.",
        release_time: 1_564_444_800,
    },
    PublishedRecord {
        identifier: "e34kieahgkm0lggs5g0s412jt",
        name: "Disallow RDS database instances without storage encryption",
        description: "Secure your Amazon RDS database instances at rest by encrypting the \
                      underlying storage for database instances and their automated \
                      backups, Read Replicas, and snapshots.",
        release_time: 1_564_444_800,
    },
    PublishedRecord {
        identifier: "435u8u4w5bsf02tkxga5jisbc",
        name: "CloudTrail should be enabled and configured with at least one multi-Region \
               trail that includes read and write management events",
        description: "This AWS control checks that there is at least one multi-region AWS \
                      CloudTrail trail includes read and write management events.",
        release_time: 1_669_593_600,
    },
];

/// Every control the catalog serves, keyed by its ARN, listed with `max_results` as the
/// page size and `body` as the request.
async fn listing_by_arn(
    emulator: &Emulator,
    max_results: Option<u32>,
    body: &str,
) -> BTreeMap<String, Value> {
    let controls = drain(emulator, "/list-controls", "Controls", max_results, body).await;
    assert!(
        !controls.is_empty(),
        "case maxResults={max_results:?} {body}: ListControls must publish the catalog's \
         controls"
    );
    controls
        .into_iter()
        .map(|control| {
            let arn = control["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {control:?}: a control carries its ARN"))
                .to_string();
            (arn, control)
        })
        .collect()
}

/// The `CreateTime` of a served control, in whole seconds since the Unix epoch.
fn create_time(case: &str, control: &Value) -> i64 {
    let seconds = control["CreateTime"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case}: CreateTime must be a timestamp, got {:?}",
            control["CreateTime"]
        )
    });
    assert!(
        seconds > 0.0 && seconds.fract() == 0.0,
        "case {case}: CreateTime must be a whole-second instant after the epoch, got \
         {seconds}"
    );
    seconds as i64
}

/// Every published control is served with the catalog's own name, description and
/// release instant, by `ListControls` whatever the page size and by `GetControl`.
#[tokio::test]
async fn every_published_control_carries_the_catalogs_own_text_and_release_time() {
    let emulator = Emulator::new();

    // Page sizes spanning one entry per page, a partial page and the modeled maximum of
    // `MaxListControlsResults` (@range 1..100): the record a control is served with
    // cannot depend on how it was paged.
    for page_size in [None, Some(1u32), Some(7), Some(100)] {
        let indexed = listing_by_arn(&emulator, page_size, "{}").await;
        for row in PUBLISHED_RECORDS {
            let arn = catalog_arn(row.identifier);
            let case = format!("{} maxResults={page_size:?}", row.identifier);
            let summary = indexed
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case}: the catalog must publish {arn}"));

            assert_eq!(
                summary["Name"].as_str(),
                Some(row.name),
                "case {case}: expected the catalog's published Name {:?}, got {:?}",
                row.name,
                summary["Name"]
            );
            assert_eq!(
                summary["Description"].as_str(),
                Some(row.description),
                "case {case}: expected the catalog's published Description {:?}, got {:?}",
                row.description,
                summary["Description"]
            );
            assert_eq!(
                create_time(&case, summary),
                row.release_time,
                "case {case}: expected the control's published release instant {}, got {:?}",
                row.release_time,
                summary["CreateTime"]
            );

            // The summary is a projection of the record GetControl answers with, so the
            // control reads back with the same three values.
            let read_back = emulator
                .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
                .await;
            assert_eq!(
                (
                    read_back["Name"].as_str(),
                    read_back["Description"].as_str(),
                    create_time(&case, &read_back),
                ),
                (Some(row.name), Some(row.description), row.release_time),
                "case {case}: GetControl must report the same published record, got \
                 {read_back:?}"
            );
        }
    }
}

/// Controls the catalog released together are served with ONE release instant: the
/// instant belongs to the control, not to its position in a listing.
#[tokio::test]
async fn controls_released_together_share_one_release_instant() {
    let emulator = Emulator::new();
    let indexed = listing_by_arn(&emulator, None, "{}").await;

    // The release instants the published table records for more than one control.
    let mut grouped: BTreeMap<i64, Vec<&PublishedRecord>> = BTreeMap::new();
    for row in PUBLISHED_RECORDS {
        grouped.entry(row.release_time).or_default().push(row);
    }
    let shared: Vec<(i64, Vec<&PublishedRecord>)> = grouped
        .into_iter()
        .filter(|(_, rows)| rows.len() > 1)
        .collect();
    assert!(
        shared.len() >= 2,
        "case catalog: the table must record more than one shared release instant for \
         the rule to be tested, got {}",
        shared.len()
    );

    for (release_time, rows) in shared {
        let mut served: Vec<(String, i64)> = Vec::new();
        for row in &rows {
            let arn = catalog_arn(row.identifier);
            let case = format!("release {release_time} control {}", row.identifier);
            let summary = indexed
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case}: the catalog must publish {arn}"));
            served.push((arn, create_time(&case, summary)));
        }
        for (arn, actual) in &served {
            assert_eq!(
                *actual, release_time,
                "case release {release_time} control {arn}: controls released together \
                 must share one CreateTime, got {served:?}"
            );
        }
    }
}

/// Every control the catalog serves — not only the rows above — is served with the same
/// name, description and release instant whatever filter selected it and however often
/// it is read: reading the catalog neither re-words nor re-dates a control.
#[tokio::test]
async fn a_controls_published_record_is_stable_across_reads_and_filters() {
    let emulator = Emulator::new();
    let first = listing_by_arn(&emulator, None, "{}").await;

    // Requests that select the catalog differently: the whole catalog again, the same
    // catalog paged one control at a time, and each implementation type the served
    // controls report, which narrows to a subset of them.
    let mut requests: Vec<(Option<u32>, String)> =
        vec![(None, "{}".to_string()), (Some(1), "{}".to_string())];
    let mut types: Vec<String> = first
        .values()
        .filter_map(|control| control["Implementation"]["Type"].as_str().map(str::to_string))
        .collect();
    types.sort();
    types.dedup();
    assert!(
        types.len() >= 2,
        "case catalog: the catalog must publish more than one implementation type for \
         the filter cases to narrow anything, got {types:?}"
    );
    for implementation_type in &types {
        requests.push((
            None,
            format!(r#"{{"Filter":{{"Implementations":{{"Types":["{implementation_type}"]}}}}}}"#),
        ));
    }

    for (page_size, body) in &requests {
        let again = listing_by_arn(&emulator, *page_size, body).await;
        for (arn, control) in &again {
            let case = format!("{arn} maxResults={page_size:?} {body}");
            let expected = first
                .get(arn)
                .unwrap_or_else(|| panic!("case {case}: the unfiltered listing must hold {arn}"));
            for member in ["Name", "Description"] {
                assert_eq!(
                    control[member], expected[member],
                    "case {case} member {member}: a control's published text must not \
                     change between reads, got {:?}",
                    control[member]
                );
            }
            assert_eq!(
                create_time(&case, control),
                create_time(&case, expected),
                "case {case}: a control's release instant must not change between reads"
            );
        }
    }
}
