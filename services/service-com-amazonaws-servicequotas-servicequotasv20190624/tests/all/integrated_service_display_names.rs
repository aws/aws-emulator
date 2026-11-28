// SPDX-License-Identifier: CC0-1.0

//! What `ListServices` publishes as a service's DISPLAY NAME, and that the same string
//! is the one every quota listing of that code reports.
//!
//! THE RULE, over the whole integrated-service inventory rather than over one entry:
//! `ListServices` reports one `ServiceInfo` per integrated `ServiceCode`, whose
//! `ServiceName` is the display name the inventory gives THAT code — reproduced as the
//! inventory spells it (with or without an `AWS`/`Amazon` prefix, with the case it uses
//! for a product name, with a parenthesised code, even when it is not a product name at
//! all), never a sibling code's name, and never the `ServiceCode` echoed back — and the
//! same string is what `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` report in
//! `ServiceQuota.ServiceName` for that code (`smithy_query(query="shape",
//! shape="ServiceInfo")`: `ServiceInfo{ServiceCode @length 1..63 @pattern
//! ^[a-zA-Z][a-zA-Z0-9-]{1,63}$, ServiceName}`).
//!
//! Why these suites: `ListServices` is the discovery call for a `ServiceCode`, so a code
//! it omits is unreachable and a name that is really the code — or the neighbouring
//! service's name — makes the listing useless for display and for matching a service by
//! name. The one code with no citable display name is the exception
//! `catalog_identity.rs` pins the exact set of; everything here is about the codes that
//! DO have one.
//!
//! The harness (`client`, `emulator`, `service_inventory`, `default_quotas`) is the one
//! `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use crate::quota_workflow::{client, default_quotas, emulator, service_inventory, Client};

/// One row of the integrated-service inventory: a `ServiceCode` and the display name
/// `ListServices` reports for it.
struct Listed {
    code: &'static str,
    name: &'static str,
    /// What makes this row a different SHAPE of display name from the others.
    shape: &'static str,
}

/// The rows this suite pins.
///
/// SOURCE: AWS's own integrated-service inventory — the `ServiceInfo{ServiceCode,
/// ServiceName}` pairs `aws service-quotas list-services` reports, which is the table
/// `src/state.rs::SEED_SERVICE_CATALOG` carries.
///
/// The rows span what the rule generalises over rather than repeating one case: a
/// display name with an `Amazon` prefix and one with `AWS`, one with NO prefix, one that
/// carries a parenthesised abbreviation, one that carries the service code itself in
/// parentheses, one whose product words are lower case, one that is another service's
/// CODE rather than a product name, one that is an initialism, a renamed product, and —
/// for each sibling pair of one product family — both codes, because the defect this
/// suite exists for gave one sibling the other's name. Over half the rows name codes the
/// issue behind this suite never cited.
const INVENTORY: &[Listed] = &[
    Listed {
        code: "access-analyzer",
        name: "Access Analyzer",
        shape: "no AWS/Amazon prefix",
    },
    Listed {
        code: "acm",
        name: "AWS Certificate Manager (ACM)",
        shape: "a parenthesised abbreviation",
    },
    Listed {
        code: "airflow",
        name: "Amazon Managed Workflows for Apache Airflow",
        shape: "the long product name of a sibling of airflow-serverless",
    },
    Listed {
        code: "airflow-serverless",
        name: "Amazon MWAA Serverless (airflow-serverless)",
        shape: "the service code carried inside the display name",
    },
    Listed {
        code: "aiops",
        name: "Amazon AI Operations",
        shape: "a recent launch with a cited name",
    },
    Listed {
        code: "autoscaling",
        name: "Amazon EC2 Auto Scaling",
        shape: "one half of a family pair",
    },
    Listed {
        code: "autoscaling-plans",
        name: "AWS Auto Scaling Plans",
        shape: "the other half of that pair, named for its own plane",
    },
    Listed {
        code: "aws-mcp",
        name: "AWS MCP",
        shape: "an initialism",
    },
    Listed {
        code: "axis",
        name: "sagemaker-geospatial",
        shape: "a display name that is another service's code",
    },
    Listed {
        code: "b2bi",
        name: "B2B Data Interchange",
        shape: "a product name the inventory carries no prefix on",
    },
    Listed {
        code: "bedrock",
        name: "Amazon Bedrock",
        shape: "the root of a three-code family",
    },
    Listed {
        code: "bedrock-agentcore",
        name: "Amazon Bedrock AgentCore",
        shape: "a family member whose code the catalog used to omit",
    },
    Listed {
        code: "bedrock-websearch",
        name: "Amazon Bedrock Web Search",
        shape: "a family member with spaced product words",
    },
    Listed {
        code: "cleanrooms",
        name: "AWS Clean Rooms",
        shape: "the root of a two-code family",
    },
    Listed {
        code: "cleanrooms-ml",
        name: "AWS Clean Rooms ML",
        shape: "a family member whose code the catalog used to omit",
    },
    Listed {
        code: "cloudwatch",
        name: "Amazon CloudWatch Omni",
        shape: "a code whose name is NOT the famous product of that word",
    },
    Listed {
        code: "cloudwatchpredictions",
        name: "Amazon CloudWatch Anomaly Detection",
        shape: "a name that does not follow from the code at all",
    },
    Listed {
        code: "monitoring",
        name: "Amazon CloudWatch",
        shape: "the code Amazon CloudWatch itself is integrated under",
    },
    Listed {
        code: "connect",
        name: "Amazon Connect",
        shape: "the root of the Amazon Connect family",
    },
    Listed {
        code: "connect-campaigns",
        name: "Amazon Connect outbound campaigns",
        shape: "lower-case product words after the service name",
    },
    Listed {
        code: "es",
        name: "Amazon OpenSearch Service",
        shape: "the code Amazon OpenSearch Service itself is integrated under",
    },
    Listed {
        code: "opensearch",
        name: "OpenSearch Application",
        shape: "a plane of that product named for the plane alone, not for the service",
    },
    Listed {
        code: "events",
        name: "Amazon EventBridge (CloudWatch Events)",
        shape: "a renamed product, with the former name in parentheses",
    },
    Listed {
        code: "route53resolver",
        name: "Route 53 Resolver",
        shape: "a product listed without its Amazon prefix",
    },
    Listed {
        code: "appstream2",
        name: "Amazon WorkSpaces Applications",
        shape: "a renamed product whose code keeps the old name",
    },
];

/// The product FAMILIES whose codes each have a display name of their own.
///
/// SOURCE: the inventory rows above — these are the codes that share a leading word, so
/// they are where a name can silently be a sibling's.
const FAMILIES: &[&[&str]] = &[
    &["autoscaling", "autoscaling-plans"],
    &["bedrock", "bedrock-agentcore", "bedrock-websearch"],
    &["cleanrooms", "cleanrooms-ml"],
    &["cloudwatch", "cloudwatchpredictions", "monitoring"],
    &["connect", "connect-campaigns"],
    &["airflow", "airflow-serverless"],
];

/// `ServiceCode -> ServiceName`, as `ListServices` reports the whole inventory.
async fn inventory(client: &Client) -> BTreeMap<String, String> {
    service_inventory(client)
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

/// Every row reports its own display name, at every endpoint, and the code it is
/// discovered under resolves.
#[tokio::test]
async fn a_listed_code_reports_the_display_name_the_inventory_gives_it() {
    let engine = emulator();

    // Two endpoints: the inventory is the same in every Region, so the rule is about
    // the catalog rather than one Region's view of it.
    for region in ["us-east-1", "ap-southeast-2"] {
        let client = client(&engine, region);
        let listed = inventory(&client).await;

        for row in INVENTORY {
            let case = format!("{region} {} ({})", row.code, row.shape);
            let reported = listed.get(row.code).map(String::as_str);
            assert_eq!(
                reported,
                Some(row.name),
                "case {case}: expected the display name the inventory gives this code, got \
                 {reported:?}"
            );
            assert_ne!(
                reported,
                Some(row.code),
                "case {case}: a cited display name is never the ServiceCode echoed back"
            );

            // A code a client DISCOVERS here resolves: the discovery call and the
            // quota calls key on the same catalog, so no listed code is refused.
            let quotas = client
                .ok(
                    "ListAWSDefaultServiceQuotas",
                    json!({"ServiceCode": row.code}),
                )
                .await;
            assert!(
                quotas["Quotas"].is_array(),
                "case {case}: a listed code must resolve its default-quota listing, got \
                 {quotas}"
            );
        }
    }
}

/// Each code of a product family carries its OWN name: no code reports a sibling's.
#[tokio::test]
async fn a_family_code_never_reports_a_sibling_codes_display_name() {
    let engine = emulator();
    let client = client(&engine, "eu-central-1");
    let listed = inventory(&client).await;

    for family in FAMILIES {
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for code in *family {
            let name = listed
                .get(*code)
                .map(String::as_str)
                .unwrap_or_else(|| panic!("case {code}: the inventory must list this code"));
            if let Some(twin) = seen.iter().find(|(_, held)| **held == name) {
                panic!(
                    "case {code}: expected a display name of its own, got {name:?}, which is \
                     also what {} reports",
                    twin.0
                );
            }
            seen.insert(code, name);
        }
    }
}

/// Every quota row of a listed code reports the SAME display name the inventory does —
/// over the whole catalog, so the two listings cannot drift apart for any code.
#[tokio::test]
async fn a_quota_row_reports_the_display_name_its_code_is_listed_under() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");
    let listed = inventory(&client).await;

    for (code, name) in &listed {
        for quota in default_quotas(&client, code).await {
            assert_eq!(
                quota["ServiceName"],
                json!(name),
                "case {code}: a quota row must carry the display name ListServices reports \
                 for its code, got {quota}"
            );
        }
    }
}

/// The inventory is ONE ascending run of distinct codes: a client that walks it to
/// discover a code sees each one once, in `ServiceCode` order.
#[tokio::test]
async fn the_inventory_is_one_ascending_run_of_distinct_codes() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let services = service_inventory(&client).await;

    let codes: Vec<&str> = services
        .iter()
        .filter_map(|service| service["ServiceCode"].as_str())
        .collect();
    assert_eq!(
        codes.len(),
        services.len(),
        "case ListServices: every entry carries a ServiceCode, got {} of {}",
        codes.len(),
        services.len()
    );

    let distinct: BTreeSet<&str> = codes.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        codes.len(),
        "case ListServices: expected {} distinct codes, got {}",
        codes.len(),
        distinct.len()
    );

    for pair in codes.windows(2) {
        assert!(
            pair[0] < pair[1],
            "case ListServices: the inventory is ordered by ServiceCode, got {:?} before {:?}",
            pair[0],
            pair[1]
        );
    }

    // And the codes this suite names are all in it, so the table above is pinning the
    // catalog that is served rather than a subset of it.
    for row in INVENTORY {
        assert!(
            distinct.contains(row.code),
            "case {}: expected the inventory to list this code",
            row.code
        );
    }
}
