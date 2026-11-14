// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for installing an AWS Marketplace add-on the account holds no
//! subscription to, driven over the wire through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! The RULE both suites here assert is that the Marketplace subscription is a
//! precondition on the ADD-ON's health and not on the request's admissibility: an
//! install of a Marketplace-published add-on is ACCEPTED (HTTP 200) and the add-on
//! is created in `CREATE_FAILED` carrying one `AddonSubscriptionNeeded` health
//! issue, which `DescribeAddon` then reads back and `DeleteAddon` answers with as
//! it takes the add-on off its cluster — so the documented remediation path (read
//! the issue off the add-on, subscribe, re-install) is reachable. Nothing of such
//! an add-on is applied to the cluster, so there is no asynchronous install and
//! nothing here waits for one.
//!
//! The rows of the install table differ along what the rule generalises over: which
//! vendor publishes the listing, whether the add-on is a vendor's rebuild of an
//! Amazon EKS-managed add-on or a product of its own, whether it installs into the
//! shared namespace or one of its own, whether its builds require configuration, and
//! which optional members the request supplies. The second suite asserts the
//! publication side of the same rule over EVERY rebuild the catalog publishes.
//!
//! `addon_bootstrap_lifecycle.rs` asserts the same refusal over every Marketplace
//! add-on the catalog publishes at once; this suite is about what the add-on the
//! install left behind reports to the three calls a client makes on it.
//!
//! The request harness (`Answer`, `emulator`, `call`, `encode_label`) is the one
//! every suite in this crate shares, from `tests/all/wire.rs`; the add-on calls are
//! the helpers `tests/all/addon_lifecycle.rs`, `tests/all/addon_deletion.rs`,
//! `tests/all/addon_versions.rs` and `tests/all/addon_catalog_publication.rs`
//! already expose.

use std::collections::BTreeMap;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ADDON_ISSUE_SUBSCRIPTION_NEEDED, ADDON_ISSUE_SUBSCRIPTION_NEEDED_MESSAGE,
    ADDON_OWNER_MARKETPLACE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_catalog_publication::{configuration_schema, schema_root};
use crate::addon_deletion::delete_addon;
use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::addon_versions::{addons, describe_addon_versions, string_member, versions_of};
use crate::wire::{call, emulator};

/// The region every request in this suite is signed for — the one the shared
/// add-on helpers use, and so the region of the ARNs they mint.
const REGION: &str = "us-west-2";

/// The add-on the report behind this suite cited: one vendor's FIPS rebuild of an
/// Amazon EKS-managed CSI driver.
const CITED_REBUILD: &str = "chainguard_aws-efs-csi-driver-fips";

/// The smallest `CreateCluster` request: the three required members.
fn cluster_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

async fn start_cluster(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request(name).to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup CreateCluster {name:?}: got {answer:?}"
    );
}

/// The `(code, message)` pairs an add-on's `health.issues` carries, in the order
/// it reports them.
fn health_issues(addon: &serde_json::Value) -> Vec<(String, String)> {
    addon["health"]["issues"]
        .as_array()
        .map(|issues| {
            issues
                .iter()
                .map(|issue| (string_member(issue, "code"), string_member(issue, "message")))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `value` has the shape of the UUID the ARN of an add-on ends in:
/// `8-4-4-4-12` lowercase hexadecimal.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|digit| digit.is_ascii_hexdigit()))
}

/// The one `AddonInfo` the catalog publishes for `addon_name`.
async fn published_addon(router: &MultiServiceRouter, addon_name: &str) -> serde_json::Value {
    let answer = describe_addon_versions(router, &format!("addonName={addon_name}")).await;
    assert_eq!(
        answer.status, 200,
        "DescribeAddonVersions({addon_name}): expected HTTP 200, got {answer:?}"
    );
    let listed = addons(&answer);
    assert_eq!(
        listed.len(),
        1,
        "case {addon_name}: a published add-on is reported as exactly one AddonInfo, \
         got {:?}",
        answer.body["addons"]
    );
    listed[0].clone()
}

// ---------------------------------------------------------------------------
// RULE: a missing AWS Marketplace subscription is a precondition on the add-on's
// HEALTH, not on the admissibility of the install — so CreateAddon answers 200
// with the add-on in `CREATE_FAILED` carrying the `AddonSubscriptionNeeded`
// issue, and DescribeAddon and DeleteAddon answer for that add-on.
// ---------------------------------------------------------------------------

/// One install this suite drives: the add-on, and the optional members the
/// request carries beside its name.
struct Case {
    /// What the row varies, named in every assertion.
    case: &'static str,
    addon: &'static str,
    /// The members the request sends beside `addonName`, as the request object
    /// would carry them.
    extra: serde_json::Value,
}

#[tokio::test]
async fn an_unsubscribed_marketplace_addon_is_installed_in_create_failed() {
    let router = emulator();
    let cluster = "marketplace-install";
    start_cluster(&router, cluster).await;

    // The rows differ along the vendor, whether the add-on is a REBUILD of an
    // Amazon EKS-managed add-on or a product of its own, the namespace it
    // installs into, whether its builds require configuration, and which
    // optional members the request supplies. Four of the five are add-ons the
    // report behind this suite did not cite.
    let cases = [
        Case {
            case: "the cited FIPS rebuild of a CSI driver, nothing but the name",
            addon: CITED_REBUILD,
            extra: serde_json::json!({}),
        },
        Case {
            case: "a FIPS rebuild that installs into the shared namespace",
            addon: "chainguard_coredns-fips",
            extra: serde_json::json!({}),
        },
        Case {
            case: "a FIPS rebuild with a namespace of its own, with tags",
            addon: "chainguard_cert-manager-fips",
            extra: serde_json::json!({ "tags": { "team": "platform" } }),
        },
        Case {
            case: "a vendor product that is no rebuild, with a namespace override",
            addon: "accuknox_kubearmor",
            extra: serde_json::json!({ "namespaceConfig": { "namespace": "vendor-system" } }),
        },
        Case {
            case: "a vendor product whose builds require configuration",
            addon: "datadog_operator",
            extra: serde_json::json!({}),
        },
    ];

    for row in &cases {
        let case = row.case;
        // The namespace the add-on goes into is the catalog's own for it unless
        // the request overrides it, so the expectation is read from the
        // publication rather than written out here.
        let published = published_addon(&router, row.addon).await;
        let expected_namespace = row.extra["namespaceConfig"]["namespace"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| string_member(&published, "defaultNamespace"));

        let mut request = serde_json::json!({ "addonName": row.addon });
        for (member, value) in row.extra.as_object().expect("an object of members") {
            request[member] = value.clone();
        }

        let created = create_addon(&router, cluster, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: an install of a Marketplace add-on is accepted, got {created:?}"
        );
        let addon = created.addon().clone();

        assert_eq!(
            addon["addonName"],
            serde_json::json!(row.addon),
            "case {case:?}: addonName must be echoed, got {:?}",
            addon["addonName"]
        );
        assert_eq!(
            addon["clusterName"],
            serde_json::json!(cluster),
            "case {case:?}: clusterName must name the cluster installed on, got {:?}",
            addon["clusterName"]
        );
        assert_eq!(
            addon["status"],
            serde_json::json!("CREATE_FAILED"),
            "case {case:?}: an unsubscribed listing is never pulled, so the install is \
             settled at the call, got {:?}",
            addon["status"]
        );
        assert_eq!(
            health_issues(&addon),
            vec![(
                ADDON_ISSUE_SUBSCRIPTION_NEEDED.to_string(),
                ADDON_ISSUE_SUBSCRIPTION_NEEDED_MESSAGE.to_string(),
            )],
            "case {case:?}: expected exactly the subscription issue, got {:?}",
            addon["health"]
        );
        assert_eq!(
            addon["namespaceConfig"]["namespace"],
            serde_json::json!(expected_namespace),
            "case {case:?}: expected the add-on to report namespace \
             {expected_namespace:?}, got {:?}",
            addon["namespaceConfig"]
        );
        assert_eq!(
            addon["tags"],
            row.extra
                .get("tags")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
            "case {case:?}: an add-on reports the tags it was installed with, got {:?}",
            addon["tags"]
        );
        // Who publishes the add-on, who owns it and the listing it is sold
        // through are what the CATALOG states about the name (asserted of the
        // publication in the suite below); an installed add-on reports none of
        // them.
        for member in ["owner", "publisher", "marketplaceInformation"] {
            assert_eq!(
                addon[member],
                serde_json::Value::Null,
                "case {case:?}: an installed add-on reports no {member}, got {:?}",
                addon[member]
            );
        }
        assert!(
            addon["addonVersion"]
                .as_str()
                .is_some_and(|version| !version.is_empty()),
            "case {case:?}: a created add-on reports the build it was installed at, \
             got {:?}",
            addon["addonVersion"]
        );

        // The ARN of an add-on: the add-on resource of its own cluster, under the
        // account and Region the request was signed for, ending in an identifier
        // of the shape every resource id of this service takes.
        let arn = string_member(&addon, "addonArn");
        let prefix = format!(
            "arn:aws:eks:{REGION}:{}:addon/{cluster}/{}/",
            engine::emulator_account_id(),
            row.addon
        );
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected an ARN under {prefix:?}, got {arn:?}"
        );
        assert!(
            is_uuid_shaped(&arn[prefix.len()..]),
            "case {case:?}: expected the ARN to end in a UUID, got {arn:?}"
        );

        // The read of the add-on the client believes it created: the same add-on,
        // with the same issue to branch on, on every read.
        let described = describe_addon(&router, cluster, row.addon).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the failed install is still an add-on of its cluster, \
             got {described:?}"
        );
        assert_eq!(
            described.addon(),
            &addon,
            "case {case:?}: the read must answer the add-on the install wrote, got {:?}",
            described.addon()
        );
        let again = describe_addon(&router, cluster, row.addon).await;
        assert_eq!(
            again.addon(),
            &addon,
            "case {case:?}: a second read answers identically, got {:?}",
            again.addon()
        );

        // The clean-up the remediation path ends in: the add-on is deletable from
        // `CREATE_FAILED`, and the answer is that add-on with the removal under
        // way, still carrying the issue that says why it failed.
        let deleted = delete_addon(&router, cluster, row.addon, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: a CREATE_FAILED add-on is deletable, got {deleted:?}"
        );
        assert_eq!(
            deleted.addon()["status"],
            serde_json::json!("DELETING"),
            "case {case:?}: the delete answers with the removal under way, got {:?}",
            deleted.addon()["status"]
        );
        assert_eq!(
            health_issues(deleted.addon()),
            health_issues(&addon),
            "case {case:?}: the removal preserves the add-on's health issues, got {:?}",
            deleted.addon()["health"]
        );
        for member in ["addonName", "clusterName", "addonArn", "namespaceConfig"] {
            assert_eq!(
                deleted.addon()[member], addon[member],
                "case {case:?}: {member} must be preserved by the delete, got {:?}",
                deleted.addon()[member]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a vendor's rebuild of an Amazon EKS-managed add-on is published in the
// same catalog as the add-on it rebuilds, under the vendor's own name and owned
// by the Marketplace, and carries the published surface of that add-on: its
// category, the namespace its workloads run in and its configuration keys.
// ---------------------------------------------------------------------------

/// The add-on a rebuild's name says it rebuilds: `<vendor>_<add-on>-fips`.
fn rebuilt_addon(addon_name: &str) -> Option<&str> {
    addon_name
        .split_once('_')
        .and_then(|(_, product)| product.strip_suffix("-fips"))
}

#[tokio::test]
async fn a_vendor_rebuild_publishes_the_surface_of_the_addon_it_rebuilds() {
    let router = emulator();

    // Every rebuild the catalog publishes, read from the publication itself —
    // the rows are whatever it offers, so the rule is asserted over the whole
    // family rather than over the one name the report cited.
    let answer = describe_addon_versions(&router, &format!("owners={ADDON_OWNER_MARKETPLACE}"))
        .await;
    assert_eq!(
        answer.status, 200,
        "DescribeAddonVersions(owners=marketplace): expected HTTP 200, got {answer:?}"
    );
    let rebuilds: BTreeMap<String, serde_json::Value> = addons(&answer)
        .into_iter()
        .filter_map(|addon| {
            let name = string_member(&addon, "addonName");
            rebuilt_addon(&name).is_some().then_some((name, addon))
        })
        .collect();
    assert!(
        rebuilds.contains_key(CITED_REBUILD),
        "the catalog must publish {CITED_REBUILD}, got {:?}",
        rebuilds.keys().collect::<Vec<_>>()
    );
    assert!(
        rebuilds.len() > 1,
        "a rebuild is a CLASS of add-on, so the catalog publishes more than one, got {:?}",
        rebuilds.keys().collect::<Vec<_>>()
    );

    for (name, rebuild) in &rebuilds {
        let case = name.as_str();
        let rebuilt = rebuilt_addon(case).expect("a rebuild names what it rebuilds");
        let published = published_addon(&router, rebuilt).await;

        for member in ["type", "defaultNamespace"] {
            assert_eq!(
                rebuild[member], published[member],
                "case {case}: a rebuild publishes the {member} of {rebuilt}, got {:?}",
                rebuild[member]
            );
        }
        assert_eq!(
            string_member(rebuild, "owner"),
            ADDON_OWNER_MARKETPLACE,
            "case {case}: a rebuild is sold through the Marketplace, got {:?}",
            rebuild["owner"]
        );
        // A rebuild is published under its vendor's OWN published name, which is
        // the seller's capitalization of the vendor token its `<vendor>_<add-on>`
        // name starts with rather than that lower-case token itself.
        let vendor = case.split_once('_').map(|(vendor, _)| vendor).unwrap_or("");
        let published_name = string_member(rebuild, "publisher");
        assert_eq!(
            published_name.to_lowercase(),
            vendor,
            "case {case}: a rebuild is published under its vendor's published name, got {:?}",
            rebuild["publisher"]
        );
        assert_ne!(
            published_name, vendor,
            "case {case}: the seller's published name is not the lower-case name token, got {:?}",
            rebuild["publisher"]
        );
        for member in ["productId", "productUrl"] {
            assert!(
                rebuild["marketplaceInformation"][member]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "case {case}: the listing must report its {member}, got {:?}",
                rebuild["marketplaceInformation"]
            );
        }

        // The same software, so the same configurable keys: the schema served for
        // the rebuild's newest build declares exactly the keys the rebuilt
        // add-on's does.
        let newest = |addon: &serde_json::Value| {
            let builds = versions_of(addon);
            let newest = builds
                .first()
                .unwrap_or_else(|| panic!("case {case}: expected a published build"));
            string_member(newest, "addonVersion")
        };
        let rebuild_schema = configuration_schema(&router, case, &newest(rebuild)).await;
        let rebuilt_schema = configuration_schema(&router, rebuilt, &newest(&published)).await;
        let keys = |schema: &serde_json::Value| {
            schema_root(schema)["properties"]
                .as_object()
                .map(|properties| properties.keys().cloned().collect::<Vec<String>>())
                .unwrap_or_default()
        };
        assert_eq!(
            keys(&rebuild_schema),
            keys(&rebuilt_schema),
            "case {case}: a rebuild publishes the configuration keys of {rebuilt}, got {:?}",
            keys(&rebuild_schema)
        );
    }
}
