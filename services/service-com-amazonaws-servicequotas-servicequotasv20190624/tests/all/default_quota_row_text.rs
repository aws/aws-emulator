// SPDX-License-Identifier: CC0-1.0

//! The CATALOG TEXT a default-quota row carries over the wire — the service label and
//! the quota description.
//!
//! A `ServiceQuota` is not just a number: a client renders and joins on its
//! `ServiceName` and `Description`, so both are part of the published catalog rather
//! than decoration. The rules asserted here, each over a TABLE of inputs:
//!
//! * a `ServiceCode` is published under ONE `ServiceName` — the label `ListServices`
//!   reports for it — and every row of every quota listing of that code carries exactly
//!   that string, in every Region;
//! * where the catalog publishes a `Description` for a quota, that text is what the
//!   listing AND the single read report, identically;
//! * every `Description` any row serves satisfies the modelled `QuotaDescription`
//!   (`smithy_query(query="shape", shape="ServiceQuota")`: length 0..350, pattern
//!   `^.{0,350}$`, so one line), and is never empty when present;
//! * the rows a service publishes read back one by one under their own `QuotaCode`,
//!   with the members AWS publishes for each.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `service_inventory`,
//! `whole_default_catalog`) is the one `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, service_inventory as inventory, walk_pages,
    whole_default_catalog as whole_catalog, Client,
};

/// The `ServiceCode`s the label rule is asserted over.
///
/// The rows differ along what the rule generalises over — catalog size and the KIND of
/// quota a service publishes: a service of counted resource quotas (`kafka`), one of
/// metered request rates (`kms`), a large catalog (`apigateway`), one whose quotas are
/// all account-wide (`iam`), a small one (`signin`) and one whose every row is published
/// under an AWS-minted code (`access-analyzer`).
const LABELLED_SERVICES: &[&str] = &[
    "access-analyzer",
    "apigateway",
    "iam",
    "kafka",
    "kms",
    "signin",
];

/// One published default-quota row: `(ServiceCode, QuotaCode, QuotaName, Value,
/// Description)`.
struct PublishedRow {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    description: &'static str,
}

/// Amazon MSK's published default quotas, as the catalog serves them for `ServiceCode`
/// `kafka`.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response for `kafka`, which
/// states each row's `QuotaCode`, `QuotaName` and default `Value` and the published
/// description of the broker quota; `src/state.rs::seed_quota_catalog` is where the
/// table lives and says which description text is the service's own and which is this
/// catalog's wording in AWS's form.
const KAFKA_ROWS: &[PublishedRow] = &[
    PublishedRow {
        service_code: "kafka",
        quota_code: "L-FAB9E493",
        quota_name: "Number of brokers per cluster",
        value: 30.0,
        description: "The maximum number of brokers that a cluster can contain.",
    },
    PublishedRow {
        service_code: "kafka",
        quota_code: "L-B2FDE22A",
        quota_name: "Number of configurations per account",
        value: 100.0,
        description: "The maximum number of configurations that an account can contain.",
    },
    PublishedRow {
        service_code: "kafka",
        quota_code: "L-36D29E8C",
        quota_name: "Number of revisions per configuration",
        value: 50.0,
        description: "The maximum number of revisions that a configuration can contain.",
    },
    PublishedRow {
        service_code: "kafka",
        quota_code: "L-8F940D28",
        quota_name: "Number of replicators per account",
        value: 15.0,
        description: "The maximum number of replicators that an account can contain.",
    },
];

/// Described rows of OTHER services, so the description rule is asserted past the one
/// service the catalog change touched.
///
/// SOURCE: `src/state.rs::seed_quota_catalog` (each row's `.describe(...)`), whose own
/// sources are named there: the per-service published quota data, and for the KMS rate
/// the sentence its whole request-rate class is published with.
const DESCRIBED_ELSEWHERE: &[(&str, &str, &str)] = &[
    (
        "access-analyzer",
        "L-1E51937C",
        "The maximum number of archive rules per analyzer.",
    ),
    (
        "apigateway",
        "L-4D98A8A5",
        "The maximum number of domain name access associations that you can create in this \
         account in the current region",
    ),
    (
        "kms",
        "L-8A08DAEA",
        "Maximum ListKeys requests per second. When you reach this quota, KMS rejects requests \
         for this operation for the remainder of the interval.",
    ),
];

/// `ServiceCode -> ServiceName`, as `ListServices` reports the inventory.
async fn listed_labels(client: &Client) -> BTreeMap<String, String> {
    inventory(client)
        .await
        .iter()
        .filter_map(|service| {
            Some((
                service["ServiceCode"].as_str()?.to_string(),
                service["ServiceName"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// Every `ServiceCode` is published under one `ServiceName`: the label the inventory
/// reports, on every row of both quota listings, at every endpoint.
#[tokio::test]
async fn a_service_code_is_published_under_one_service_label() {
    let engine = emulator();

    // Two endpoints, because the label belongs to the SERVICE and not to one Region's
    // view of its catalog.
    for region in ["us-west-2", "eu-central-1"] {
        let client = client(&engine, region);
        let labels = listed_labels(&client).await;

        for service_code in LABELLED_SERVICES {
            let case = format!("{region} {service_code}");
            let expected = labels.get(*service_code).unwrap_or_else(|| {
                panic!("case {case}: the inventory must list this service, got {labels:?}")
            });
            assert!(
                !expected.is_empty() && expected != service_code,
                "case {case}: a service label is a display name, not the code, got \
                 {expected:?}"
            );

            for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
                let rows = walk_pages(
                    &client,
                    operation,
                    &json!({"ServiceCode": service_code}),
                    "Quotas",
                    None,
                )
                .await;
                assert!(
                    !rows.is_empty(),
                    "case {case}: {operation} must publish this service's quotas, got {rows:?}"
                );
                let labels_served: BTreeSet<String> = rows
                    .iter()
                    .map(|quota| quota["ServiceName"].to_string())
                    .collect();
                assert_eq!(
                    labels_served,
                    BTreeSet::from([json!(expected).to_string()]),
                    "case {case}: {operation} must label every row with the one published \
                     service name {expected:?}, got {labels_served:?}"
                );
            }
        }
    }
}

/// A row the catalog publishes a description for serves that text through the listing
/// and the single read alike.
#[tokio::test]
async fn a_published_description_is_the_text_both_reads_report() {
    let engine = emulator();
    let region = "ap-southeast-2";
    let client = client(&engine, region);

    let rows = KAFKA_ROWS
        .iter()
        .map(|row| (row.service_code, row.quota_code, row.description))
        .chain(DESCRIBED_ELSEWHERE.iter().copied());

    for (service_code, quota_code, description) in rows {
        let case = format!("{service_code}/{quota_code}");
        let listed = defaults(&client, service_code).await;
        let row = listed
            .iter()
            .find(|quota| quota["QuotaCode"] == json!(quota_code))
            .unwrap_or_else(|| {
                panic!("case {case}: the listing must publish this quota, got {listed:?}")
            });
        assert_eq!(
            row["Description"],
            json!(description),
            "case {case}: expected the published description, got {row}"
        );

        let single = client
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await;
        assert_eq!(
            single["Quota"], *row,
            "case {case}: the single read must report the listed row, got {single}"
        );

        // The applied plane reports the same catalog text: the description belongs to
        // the quota, not to the plane it is read on.
        let applied = client
            .ok(
                "GetServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await;
        assert_eq!(
            applied["Quota"]["Description"],
            json!(description),
            "case {case}: the applied read must report the same description, got {applied}"
        );
    }
}

/// Every `Description` the catalog serves satisfies the modelled `QuotaDescription`:
/// present means non-empty, at most 350 characters and one line.
#[tokio::test]
async fn every_served_description_satisfies_the_modelled_bound() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let mut described = 0_usize;
    for (service_code, rows) in whole_catalog(&client).await {
        for quota in rows {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            match &quota["Description"] {
                Value::Null => {}
                Value::String(text) => {
                    described += 1;
                    assert!(
                        !text.is_empty(),
                        "case {case}: a published description is never the empty string, got \
                         {quota}"
                    );
                    assert!(
                        text.chars().count() <= 350,
                        "case {case}: a description is at most 350 characters, got {} in {quota}",
                        text.chars().count()
                    );
                    assert!(
                        !text.contains('\n') && !text.contains('\r'),
                        "case {case}: a description is one line, got {text:?}"
                    );
                }
                other => panic!("case {case}: Description must be a string, got {other}"),
            }
        }
    }
    assert!(
        described > DESCRIBED_ELSEWHERE.len() + KAFKA_ROWS.len(),
        "case catalog: the catalog must serve the descriptions it publishes, got {described}"
    );
}

/// The rows this catalog publishes for Amazon MSK are exactly the ones it can cite, each
/// reporting the members AWS publishes for it and reading back under its own code.
#[tokio::test]
async fn the_msk_catalog_publishes_the_rows_it_cites() {
    let engine = emulator();
    let region = "us-west-2";
    let client = client(&engine, region);

    let listed = defaults(&client, "kafka").await;
    let codes: BTreeSet<&str> = listed
        .iter()
        .filter_map(|quota| quota["QuotaCode"].as_str())
        .collect();
    assert_eq!(
        codes,
        KAFKA_ROWS.iter().map(|row| row.quota_code).collect(),
        "case kafka: expected exactly the cited published rows"
    );

    for row in KAFKA_ROWS {
        let case = format!("kafka/{}", row.quota_code);
        let quota = client
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": "kafka", "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();

        assert_eq!(
            (
                quota["ServiceCode"].as_str(),
                quota["QuotaName"].as_str(),
                quota["Value"].as_f64(),
                quota["Unit"].as_str(),
                quota["Adjustable"].as_bool(),
                quota["GlobalQuota"].as_bool(),
                quota["QuotaArn"].as_str(),
            ),
            (
                Some("kafka"),
                Some(row.quota_name),
                Some(row.value),
                Some("None"),
                Some(true),
                Some(false),
                Some(
                    format!(
                        "arn:aws:servicequotas:{region}::kafka/{}",
                        row.quota_code
                    )
                    .as_str()
                ),
            ),
            "case {case}: expected the published counted, adjustable, regional default row, got \
             {quota}"
        );
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
    }

    // The rows differ in their published default value, so the table spans more than
    // one shape of row rather than repeating one.
    let values: BTreeSet<String> = listed
        .iter()
        .map(|quota| quota["Value"].to_string())
        .collect();
    assert_eq!(
        values.len(),
        KAFKA_ROWS.len(),
        "case kafka: each cited row carries its own default value, got {values:?}"
    );
}
