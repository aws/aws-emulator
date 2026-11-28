// SPDX-License-Identifier: CC0-1.0

//! That every `ServiceInfo` the integrated-service inventory publishes carries the
//! display name AWS REGISTERS for that `ServiceCode`, as registered — and that each
//! plane of a product family is registered under its own code with its own name.
//!
//! THE RULE, stated over the inventory rather than over one entry: `ListServices`
//! reports one `ServiceInfo` per `ServiceCode` Service Quotas registers, and an entry's
//! `ServiceName` is the display name registered for THAT code, reproduced exactly as
//! registered — with the `Amazon`/`AWS` prefix only where the registration carries one,
//! with the product spelled out or abbreviated as registered, and never the
//! `ServiceCode` echoed back as a stand-in (`smithy_query(query="shape",
//! shape="ServiceInfo")`: `ServiceInfo{ServiceCode @length 1..63 @pattern
//! ^[a-zA-Z][a-zA-Z0-9-]{1,63}$, ServiceName}`; `smithy_query(query="operation",
//! operation="ListServices")`: output `{NextToken, Services:
//! ServiceInfoListDefinition}`). The model enumerates no catalog, so WHICH codes exist
//! and what each is named are AWS's registered facts, carried by
//! `src/state.rs::SEED_SERVICE_CATALOG`.
//!
//! What this suite owns, and what it does NOT: `integrated_service_display_names.rs`
//! owns the SHAPES a display name takes and that a name is never a sibling's;
//! `inventory_registered_display_names.rs` owns coverage and discovery through paging;
//! `inventory_registered_plane_names.rs` owns the MEMBERSHIP half of the rule — that a
//! registered code is listed and resolves even where its registered name cannot be
//! cited. This suite owns the entries whose registered name does NOT follow from the
//! product's marketing name — a prefix the registration drops, a qualifying suffix it
//! drops, a product the registration spells out, two words the registration spaces
//! apart, a bare product name — and the sibling PLANES that are registered as codes of
//! their own, each of which a catalog can get wrong by "tidying" the name or by folding
//! the plane into its sibling.
//!
//! The harness (`client`, `emulator`, `service_inventory`, `default_quotas`) is the one
//! `quota_workflow.rs` owns.

use std::collections::BTreeMap;

use serde_json::json;

use crate::quota_workflow::{client, default_quotas, emulator, service_inventory, Client};

/// One registered inventory entry.
struct Registered {
    /// `ServiceInfo.ServiceCode`.
    code: &'static str,
    /// `ServiceInfo.ServiceName`, as AWS registers it for that code.
    name: &'static str,
    /// What makes this row a different case of the rule from the others.
    form: &'static str,
}

/// The rows this suite pins.
///
/// SOURCE: AWS's own integrated-service inventory — the `ServiceInfo{ServiceCode,
/// ServiceName}` pairs `aws service-quotas list-services` reports, which is the table
/// `src/state.rs::SEED_SERVICE_CATALOG` carries. The model states no catalog at all
/// (`smithy_query(query="shape", shape="ServiceInfo")` carries the two members and the
/// `ServiceCode` constraints and nothing more), so the registered pair is the authority.
///
/// The rows span the ways a registered name does not follow from the product's own
/// marketing name, rather than repeating one case: a registration with no prefix at
/// all, one that keeps its prefix but drops the product's `Service` suffix, one that
/// drops only the `Amazon` prefix, one that spells out an abbreviation the product is
/// better known by, one that spaces two words the product runs together, a family root
/// named for the product itself, two sibling PLANES registered as codes of their own,
/// a plane named for its product family rather than for anything its code spells out,
/// and a plane registered under the product name ALONE. Several rows are entries no
/// report touched, so the table pins the rule rather than one repair.
const REGISTERED: &[Registered] = &[
    Registered {
        code: "migrationhubstrategy",
        name: "Migration Hub Strategy Recommendations",
        form: "a registration that drops the product's AWS prefix",
    },
    Registered {
        code: "rolesanywhere",
        name: "IAM Roles Anywhere",
        form: "the same dropped prefix on an unrelated product",
    },
    Registered {
        code: "observabilityadmin",
        name: "Amazon CloudWatch Observability Admin",
        form: "a prefixed registration that drops the product's Service suffix",
    },
    Registered {
        code: "oam",
        name: "Amazon CloudWatch Observability Access Manager",
        form: "its sibling plane, whose registration spells the product out in full",
    },
    Registered {
        code: "qbusiness",
        name: "Amazon Q",
        form: "the product name alone on a code that names a plane of it",
    },
    Registered {
        code: "qapps",
        name: "Amazon Q Apps",
        form: "its sibling plane, named for the plane after the product",
    },
    Registered {
        code: "networkinsights",
        name: "Network Insights",
        form: "a feature's two words alone, with no prefix at all",
    },
    Registered {
        code: "network-firewall",
        name: "AWS Network Firewall",
        form: "the prefixed code beside it, which keeps its prefix",
    },
    Registered {
        code: "opensearch",
        name: "OpenSearch Application",
        form: "a plane's own name, not the name of the service beside it",
    },
    Registered {
        code: "es",
        name: "Amazon OpenSearch Service",
        form: "the code that service itself is integrated under",
    },
    Registered {
        code: "health-agent",
        name: "Amazon Connect Health",
        form: "a plane named for its product family, not for its code",
    },
    Registered {
        code: "interconnect",
        name: "AWS Interconnect",
        form: "a prefixed product name on a code that carried no citable name before",
    },
    Registered {
        code: "internetmonitor",
        name: "CloudWatch Internet Monitor",
        form: "a CloudWatch plane registered without the Amazon prefix",
    },
    Registered {
        code: "iot",
        name: "AWS IoT",
        form: "the family root, named for the product itself",
    },
    Registered {
        code: "iotcore",
        name: "AWS IoT Core",
        form: "the Core plane, registered as a code of its own",
    },
    Registered {
        code: "iotwireless",
        name: "AWS IoT Wireless",
        form: "another plane of that family, registered as a code of its own",
    },
    Registered {
        code: "ivschat",
        name: "Amazon Interactive Video Service Chat",
        form: "a product spelled out where it is better known abbreviated",
    },
    Registered {
        code: "kafkaconnect",
        name: "Managed Streaming for Kafka Connect",
        form: "a product spelled out AND registered with no prefix",
    },
    Registered {
        code: "medical-imaging",
        name: "AWS Health Imaging",
        form: "two words the product runs together, spaced apart as registered",
    },
    Registered {
        code: "mgn",
        name: "Application Migration",
        form: "a bare product name: no prefix and no Service suffix",
    },
    Registered {
        code: "migrationhuborchestrator",
        name: "Migration Hub Orchestrator",
        form: "another bare product name",
    },
    Registered {
        code: "elasticfilesystem",
        name: "Amazon Elastic File System (EFS)",
        form: "an unprefixed abbreviation in parentheses",
    },
    Registered {
        code: "axis",
        name: "sagemaker-geospatial",
        form: "a registration that is another service's code, not a product name",
    },
];

/// The product FAMILIES whose planes are each registered as a code of their own.
///
/// SOURCE: the rows above — these are the codes that share a leading word, or that name
/// two planes of one product, so they are where one plane can silently be served under
/// its sibling's name, or be folded away.
const FAMILIES: &[&[&str]] = &[
    &["iot", "iotcore", "iotdeviceadvisor", "iotwireless"],
    &["ivs", "ivschat"],
    &["kafka", "kafkaconnect"],
    &["internetmonitor", "monitoring"],
    &["health-agent", "healthlake"],
    &["es", "opensearch"],
    &["oam", "observabilityadmin"],
    &["qapps", "qbusiness"],
    &["network-firewall", "networkinsights"],
    &["mgn", "migrationhuborchestrator", "migrationhubstrategy"],
];

/// `ServiceCode -> ServiceName`, as `ListServices` reports the whole inventory, with
/// each code listed exactly once.
///
/// `pub(crate)` so the suite that owns the inventory's MEMBERSHIP
/// (`inventory_registered_plane_names.rs`) reads the inventory through this one walk
/// rather than keeping a copy of it.
pub(crate) async fn inventory(client: &Client) -> BTreeMap<String, String> {
    let listed = service_inventory(client).await;
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for service in &listed {
        let code = service["ServiceCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case ListServices: an entry without a code: {service}"))
            .to_string();
        let name = service["ServiceName"]
            .as_str()
            .unwrap_or_else(|| panic!("case ListServices: an entry without a name: {service}"))
            .to_string();
        if let Some(first) = names.insert(code.clone(), name.clone()) {
            panic!("case {code}: a code is listed once, got {first:?} and {name:?}");
        }
    }
    names
}

/// Every row is listed under the name its code is registered with, at every endpoint.
#[tokio::test]
async fn an_inventory_entry_carries_the_name_its_code_is_registered_under() {
    let engine = emulator();

    // Two endpoints: the inventory is the same in every Region, so the rule is about
    // the catalog rather than one Region's view of it.
    for region in ["us-east-1", "ap-southeast-2"] {
        let client = client(&engine, region);
        let listed = inventory(&client).await;

        for row in REGISTERED {
            let case = format!("{region} {} ({})", row.code, row.form);
            let reported = listed.get(row.code).map(String::as_str);
            assert_eq!(
                reported,
                Some(row.name),
                "case {case}: expected the display name AWS registers for this code, got \
                 {reported:?}"
            );
            assert_ne!(
                reported,
                Some(row.code),
                "case {case}: a registered display name is never the ServiceCode echoed back"
            );
        }
    }
}

/// A code the inventory publishes RESOLVES: a client that discovered it there reads its
/// default-quota listing back, so a plane registered as a code of its own is reachable
/// rather than merely listed.
#[tokio::test]
async fn a_registered_code_resolves_its_default_quota_listing() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    for row in REGISTERED {
        let case = format!("{} ({})", row.code, row.form);
        let response = client
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": row.code}),
            )
            .await;
        assert!(
            response["Quotas"].is_array(),
            "case {case}: a listed code must resolve its default-quota listing, got {response}"
        );
    }
}

/// Each plane of a family carries its OWN registered name: no plane reports a
/// sibling's, and none is missing from the inventory.
#[tokio::test]
async fn a_family_plane_is_registered_under_its_own_code_and_name() {
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
                    "case {code}: expected a registered name of its own, got {name:?}, which is \
                     also what {} reports",
                    twin.0
                );
            }
            seen.insert(code, name);
        }
    }
}

/// Every quota row of a row's code repeats the registered name, so the display name a
/// client discovers in the inventory is the one every later row carries.
#[tokio::test]
async fn a_quota_row_repeats_the_registered_name_of_its_code() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    for row in REGISTERED {
        let case = format!("{} ({})", row.code, row.form);
        for quota in default_quotas(&client, row.code).await {
            assert_eq!(
                quota["ServiceName"],
                json!(row.name),
                "case {case}: a quota row carries the name its code is registered under, got \
                 {quota}"
            );
            assert_eq!(
                quota["ServiceCode"],
                json!(row.code),
                "case {case}: a quota row carries the code it was listed for, got {quota}"
            );
        }
    }
}
