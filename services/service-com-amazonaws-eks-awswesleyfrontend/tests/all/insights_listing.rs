// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListInsights`, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: `ListInsights` reports "all insights checked for against the
//! specified cluster" — one insight per check Amazon EKS runs — and every part of an
//! insight is COMPUTED from what the service holds for that cluster: the status and
//! its reason from the resources the check examines, `kubernetesVersion` from the
//! version the check is about, `lastRefreshTime` from the cluster's latest completed
//! insights analysis and `lastTransitionTime` from the last change to the check's
//! subject. The filter keeps exactly the insights whose category, associated
//! Kubernetes version or status it names, `maxResults`/`nextToken` page that filtered
//! listing, and a cluster with no Amazon EKS control plane behind it has no insights
//! at all.
//!
//! Every suite asserts that rule over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (the cluster's
//! Kubernetes version, the node groups and add-ons hanging off it, its remote network
//! configuration, the version updates it has completed, which filter members are
//! given, the page size, which cluster the request names), and each assertion names
//! its case and prints the actual value. The Kubernetes versions and add-on builds
//! the rows use are read from the catalogs this service publishes rather than written
//! out here, so the table spans whatever those catalogs carry.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, next_kubernetes_minor_version, previous_kubernetes_minor_version,
    ComAmazonawsEksAwswesleyfrontendState, InsightCheck, CLUSTER_ACTIVATION_DELAY,
    INSIGHTS_REFRESH_DURATION, INSIGHT_CATEGORY_MISCONFIGURATION,
    INSIGHT_CATEGORY_ROLLBACK_READINESS, INSIGHT_CATEGORY_UPGRADE_READINESS, INSIGHT_STATUS_ERROR,
    INSIGHT_STATUS_PASSING, INSIGHT_STATUS_UNKNOWN, INSIGHT_STATUS_WARNING,
    UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_with_engine, encode_label, error_code, Answer};

const REGION: &str = "us-west-2";

/// A role of the single account this emulator serves, which is what `iam:PassRole`
/// allows.
const PASSABLE_ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// One Amazon EKS-managed add-on of the catalog this service publishes, used by the
/// add-on compatibility rows. Its builds are read from that catalog, never written
/// out here.
const ADDON: &str = "aws-ebs-csi-driver";

/// Slack added to every clock-owed wait, so a loaded machine still observes the move
/// the clock owed.
const SLACK: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// The requests these suites make, and the answers reduced to what they assert on.
// ---------------------------------------------------------------------------

async fn create_cluster(
    router: &MultiServiceRouter,
    name: &str,
    version: Option<&str>,
    remote_network_config: Option<serde_json::Value>,
) {
    let mut body = serde_json::json!({
        "name": name,
        "roleArn": PASSABLE_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    if let Some(version) = version {
        body["version"] = serde_json::json!(version);
    }
    if let Some(config) = remote_network_config {
        body["remoteNetworkConfig"] = config;
    }
    let created = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster for {name:?}, got {created:?}"
    );
}

async fn create_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    let body = serde_json::json!({
        "nodegroupName": name,
        "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
        "subnets": ["subnet-aaaa1111", "subnet-bbbb2222"],
    });
    let created = call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/node-groups", encode_label(cluster)),
        &body.to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateNodegroup for {name:?} on {cluster:?}, got {created:?}"
    );
    created
}

async fn create_addon(router: &MultiServiceRouter, cluster: &str, version: &str) -> Answer {
    let body = serde_json::json!({ "addonName": ADDON, "addonVersion": version });
    let created = call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/addons", encode_label(cluster)),
        &body.to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateAddon for {ADDON:?} at {version:?} on {cluster:?}, got \
         {created:?}"
    );
    created
}

/// `DescribeInsight` over the wire: `GET /clusters/{cluster}/insights/{id}`.
async fn describe_insight(router: &MultiServiceRouter, cluster: &str, id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/insights/{}",
            encode_label(cluster),
            encode_label(id)
        ),
        "",
    )
    .await
}

/// `UpdateClusterVersion` over the wire: `POST /clusters/{name}/updates`.
async fn update_version(
    router: &MultiServiceRouter,
    cluster: &str,
    version: &str,
    force: bool,
) -> Answer {
    let body = serde_json::json!({ "version": version, "force": force });
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/updates", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn list_insights(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/insights", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn describe_cluster(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}", encode_label(cluster)),
        "",
    )
    .await
}

async fn start_refresh(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/insights-refresh", encode_label(cluster)),
        "",
    )
    .await
}

async fn describe_refresh(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}/insights-refresh", encode_label(cluster)),
        "",
    )
    .await
}

/// The `insights` an answer carries, with the answer asserted to be a success.
fn insights(case: &str, answer: &Answer) -> Vec<serde_json::Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListInsights, got {answer:?}"
    );
    answer.body["insights"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an insights array, got {answer:?}"))
}

/// The one insight of a listing that is the result of `check`.
fn insight_of(case: &str, answer: &Answer, check: InsightCheck) -> serde_json::Value {
    let wanted = serde_json::json!(check.name());
    insights(case, answer)
        .into_iter()
        .find(|insight| insight["name"] == wanted)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an insight named {:?}, got {answer:?}",
                check.name()
            )
        })
}

/// A timestamp member of a JSON object, as the epoch-seconds number restJson1 carries.
fn instant(case: &str, value: &serde_json::Value, member: &str) -> f64 {
    value[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a numeric {member} in {value:?}");
    })
}

/// The Kubernetes versions this service publishes and has already released, newest
/// first — read from the store the handlers read, because which rows the calendar has
/// released depends on the clock.
fn published_versions(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let guard = store.lock();
    guard
        .released_cluster_versions(SystemTime::now())
        .map(|row| row.cluster_version.clone())
        .collect()
}

/// The build the add-on catalog publishes as the DEFAULT one of [`ADDON`] for one
/// Kubernetes version — the version `CreateAddon` installs when a caller names none,
/// and the only build of it the catalog marks for that version.
fn default_addon_version(engine: &engine::InMemoryEngine, kubernetes_version: &str) -> String {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let guard = store.lock();
    guard
        .default_addon_version(ADDON, kubernetes_version, SystemTime::now())
        .unwrap_or_else(|| {
            panic!("the add-on catalog must publish {ADDON} for Kubernetes {kubernetes_version}")
        })
}

/// The NEWEST published build of [`ADDON`] that is not published for one Kubernetes
/// version — the build an add-on left behind by a version update is installed at.
///
/// A build is published for the Kubernetes versions released up to it, so the builds
/// older than a version do not carry it: this is the newest of those.
fn build_not_published_for(
    engine: &engine::InMemoryEngine,
    kubernetes_version: &str,
) -> String {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let guard = store.lock();
    let now = SystemTime::now();
    guard
        .published_addon(ADDON, now)
        .and_then(|addon| {
            addon
                .published_versions(now)
                .find(|build| !build.supports(kubernetes_version))
                .map(|build| build.addon_version.clone())
        })
        .unwrap_or_else(|| {
            panic!(
                "the add-on catalog must publish a build of {ADDON} that Kubernetes \
                 {kubernetes_version} is not among the compatibilities of"
            )
        })
}

/// Two released Kubernetes versions one minor apart: the one a cluster is created at
/// and the one it would be updated to. Read from the published calendar, so the pair
/// is whatever that calendar currently offers.
fn version_pair(engine: &engine::InMemoryEngine) -> (String, String) {
    let published = published_versions(engine);
    assert!(
        published.len() >= 3,
        "setup: the published version calendar must carry at least three released versions, got \
         {published:?}"
    );
    let current = published[1].clone();
    let target = next_kubernetes_minor_version(&current)
        .unwrap_or_else(|| panic!("{current} must name a Kubernetes minor version"));
    assert_eq!(
        target, published[0],
        "setup: the calendar's two newest released versions must be one minor apart, got \
         {published:?}"
    );
    (current, target)
}

async fn settle_creation() {
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + SLACK).await;
}

async fn settle_update() {
    tokio::time::sleep(UPDATE_COMPLETION_DELAY + SLACK).await;
}

// ---------------------------------------------------------------------------
// RULE: a cluster's listing reports ONE insight per check, each carrying the check's
// own name, category and description, a status with a reason, an id derived from the
// cluster and the check, and the two instants of the analysis — and reading it twice
// reports the same thing, because a listing writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_check_is_reported_once_for_a_cluster() {
    let (router, engine) = emulator_with_engine();
    // Rows differ along the Kubernetes version the cluster runs, which is what the
    // upgrade-readiness insights are about: the three newest released versions of the
    // published calendar, plus a cluster that names no version at all and so lands on
    // the calendar's default row.
    let published = published_versions(&engine);
    assert!(
        published.len() >= 3,
        "setup: expected at least three released versions, got {published:?}"
    );
    let mut cases: Vec<(String, Option<String>)> = published
        .iter()
        .take(3)
        .map(|version| (format!("cluster at {version}"), Some(version.clone())))
        .collect();
    cases.push(("cluster at the default version".to_string(), None));

    for (index, (_case, version)) in cases.iter().enumerate() {
        let name = format!("surface-{index}");
        create_cluster(&router, &name, version.as_deref(), None).await;
    }
    settle_creation().await;

    for (index, (case, _)) in cases.iter().enumerate() {
        let name = format!("surface-{index}");
        let described = describe_cluster(&router, &name).await;
        let cluster_version = described.cluster()["version"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let created_at = instant(case, described.cluster(), "createdAt");

        let listed = list_insights(&router, &name, &serde_json::json!({})).await;
        let reported = insights(case, &listed);
        assert_eq!(
            reported.len(),
            InsightCheck::all().len(),
            "case {case:?}: expected one insight per check, got {listed:?}"
        );
        assert!(
            listed.body["nextToken"]
                .as_str()
                .is_none_or(str::is_empty),
            "case {case:?}: a listing that fits in one page carries no nextToken, got {listed:?}"
        );

        let mut ids: Vec<String> = Vec::new();
        for check in InsightCheck::all() {
            let insight = insight_of(case, &listed, check);
            let id = insight["id"].as_str().unwrap_or_default().to_owned();
            // An insight id is opaque, but it is an id: the 8-4-4-4-12 form AWS
            // insight ids take.
            let segments: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                segments,
                vec![8, 4, 4, 4, 12],
                "case {case:?}: {:?} must report an 8-4-4-4-12 id, got {id:?}",
                check.name()
            );
            ids.push(id);
            assert_eq!(
                insight["category"],
                serde_json::json!(check.category()),
                "case {case:?}: {:?} reports its own category, got {insight:?}",
                check.name()
            );
            assert_eq!(
                insight["description"],
                serde_json::json!(check.description()),
                "case {case:?}: {:?} reports its own description, got {insight:?}",
                check.name()
            );
            assert!(
                insight["insightStatus"]["reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: {:?} reports a reason for its status, got {insight:?}",
                check.name()
            );
            // Nothing has happened to this cluster since it was created, so the
            // analysis it reports is the one Amazon EKS ran when it created it, and no
            // check's subject has changed since.
            assert_eq!(
                instant(case, &insight, "lastRefreshTime"),
                created_at,
                "case {case:?}: {:?} must report the cluster's own creation as its last \
                 analysis, got {insight:?}",
                check.name()
            );
            assert_eq!(
                instant(case, &insight, "lastTransitionTime"),
                created_at,
                "case {case:?}: {:?} must report the cluster's own creation as its last \
                 transition, got {insight:?}",
                check.name()
            );

            // The version an insight is ABOUT: the version the cluster would be
            // updated to for an upgrade-readiness check, and none for a check that is
            // about no particular version — a cluster that has never been updated has
            // no version to roll back to either.
            let expected_version = match check.category() {
                INSIGHT_CATEGORY_UPGRADE_READINESS => {
                    next_kubernetes_minor_version(&cluster_version)
                }
                _ => None,
            };
            assert_eq!(
                insight["kubernetesVersion"].as_str().map(str::to_owned),
                expected_version,
                "case {case:?}: {:?} reports the version it is about, got {insight:?}",
                check.name()
            );
        }

        ids.sort();
        let distinct = {
            let mut unique = ids.clone();
            unique.dedup();
            unique.len()
        };
        assert_eq!(
            distinct,
            ids.len(),
            "case {case:?}: every check of a cluster has an id of its own, got {ids:?}"
        );

        // A read changes nothing a later caller sees: the same answer, twice.
        let again = list_insights(&router, &name, &serde_json::json!({})).await;
        assert_eq!(
            again.body, listed.body,
            "case {case:?}: a second listing must report the same insights, got {again:?}"
        );
    }

    // And an insight belongs to ONE cluster: no id is shared between two of them.
    let first = list_insights(&router, "surface-0", &serde_json::json!({})).await;
    let second = list_insights(&router, "surface-1", &serde_json::json!({})).await;
    for check in InsightCheck::all() {
        assert_ne!(
            insight_of("two clusters", &first, check)["id"],
            insight_of("two clusters", &second, check)["id"],
            "the same check of two clusters must not share an id, got {:?} for {:?}",
            insight_of("two clusters", &first, check),
            check.name()
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the managed node group check is ERROR exactly when a node group's Kubernetes
// version does not match the cluster's — the one readiness issue that also BLOCKS an
// unforced version update, so the two cannot disagree — and PASSING otherwise.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_node_group_check_reports_the_version_skew_that_blocks_an_update() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 3,
        "setup: expected at least three released versions, got {published:?}"
    );
    let (newest, middle, oldest) = (
        published[0].clone(),
        published[1].clone(),
        published[2].clone(),
    );

    // Rows differ in the node groups hanging off the cluster: none at all, one at the
    // cluster's own version, and one left behind by a version update of the cluster —
    // the only way a node group can lag, because `CreateNodegroup` accepts no other
    // version than the cluster's.
    create_cluster(&router, "skew-none", Some(&middle), None).await;
    create_cluster(&router, "skew-matching", Some(&middle), None).await;
    create_cluster(&router, "skew-lagging", Some(&oldest), None).await;
    settle_creation().await;
    create_nodegroup(&router, "skew-matching", "matching").await;
    create_nodegroup(&router, "skew-lagging", "left-behind").await;
    let carried = update_version(&router, "skew-lagging", &middle, false).await;
    assert_eq!(
        carried.status, 200,
        "setup: expected 200 moving skew-lagging to {middle:?} while its node group still \
         matches, got {carried:?}"
    );
    settle_update().await;

    let rows: Vec<(&str, &str, &str)> = vec![
        ("no node groups", "skew-none", INSIGHT_STATUS_PASSING),
        (
            "a node group at the cluster's version",
            "skew-matching",
            INSIGHT_STATUS_PASSING,
        ),
        (
            "a node group left behind by a version update",
            "skew-lagging",
            INSIGHT_STATUS_ERROR,
        ),
    ];

    for (case, cluster, expected) in rows {
        let listed = list_insights(&router, cluster, &serde_json::json!({})).await;
        let insight = insight_of(case, &listed, InsightCheck::NodegroupVersions);
        assert_eq!(
            insight["insightStatus"]["status"],
            serde_json::json!(expected),
            "case {case:?}: expected {expected}, got {insight:?}"
        );
        if expected == INSIGHT_STATUS_ERROR {
            let reason = insight["insightStatus"]["reason"]
                .as_str()
                .unwrap_or_default();
            assert!(
                reason.contains("left-behind") && reason.contains(&oldest),
                "case {case:?}: the reason must name the lagging node group and its version, got \
                 {reason:?}"
            );
        }

        // The SAME rule the operation enforces: an unforced version update is refused
        // exactly when this check found a mismatch.
        let attempted = update_version(&router, cluster, &newest, false).await;
        let refused = error_code(&attempted) == "InvalidStateException";
        assert_eq!(
            refused,
            expected == INSIGHT_STATUS_ERROR,
            "case {case:?}: an unforced update must be refused exactly when the insight is \
             {INSIGHT_STATUS_ERROR}, got {attempted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the add-on compatibility check is ERROR exactly when an installed add-on's
// build is not published for the Kubernetes version the cluster would be updated to.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_addon_check_reports_builds_that_do_not_cover_the_next_version() {
    let (router, engine) = emulator_with_engine();
    let (current, target) = version_pair(&engine);
    // Rows differ in the add-on build installed: none at all; the newest build the
    // catalog does NOT publish for the version the cluster would move to, which is
    // installable on the cluster's own version; and the default build for THAT
    // version, which is published for it. A build the catalog does not publish cannot
    // be installed at all, so no row can carry one.
    let build_for_current = build_not_published_for(&engine, &target);
    let build_for_target = default_addon_version(&engine, &target);
    assert_ne!(
        build_for_current, build_for_target,
        "setup: a build that is not published for {target} cannot be its default build"
    );
    let rows: Vec<(&str, Option<String>, &str)> = vec![
        ("no add-ons installed", None, INSIGHT_STATUS_PASSING),
        (
            "a build not published for the version the cluster would move to",
            Some(build_for_current.clone()),
            INSIGHT_STATUS_ERROR,
        ),
        (
            "the default build for the version the cluster would move to",
            Some(build_for_target.clone()),
            INSIGHT_STATUS_PASSING,
        ),
    ];

    for (index, (case, build, expected)) in rows.iter().enumerate() {
        let name = format!("addons-{index}");
        create_cluster(&router, &name, Some(&current), None).await;
        settle_creation().await;
        if let Some(build) = build {
            create_addon(&router, &name, build).await;
        }

        let listed = list_insights(&router, &name, &serde_json::json!({})).await;
        let insight = insight_of(case, &listed, InsightCheck::AddonVersions);
        assert_eq!(
            insight["insightStatus"]["status"],
            serde_json::json!(expected),
            "case {case:?}: expected {expected}, got {insight:?}"
        );
        assert_eq!(
            insight["kubernetesVersion"],
            serde_json::json!(target),
            "case {case:?}: the check is about the version the cluster would move to, got \
             {insight:?}"
        );
        let reason = insight["insightStatus"]["reason"]
            .as_str()
            .unwrap_or_default();
        if *expected == INSIGHT_STATUS_ERROR {
            assert!(
                reason.contains(ADDON) && reason.contains(&build_for_current),
                "case {case:?}: the reason must name the add-on and the build it runs, got \
                 {reason:?}"
            );
        } else {
            assert!(
                reason.contains(&target),
                "case {case:?}: the reason must name the version the add-ons were checked \
                 against, got {reason:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the hybrid nodes check reads the cluster's remote network configuration: a
// cluster with no remote node networks runs no hybrid nodes and is not evaluated
// (UNKNOWN), remote node networks without remote pod networks are the
// misconfiguration (WARNING), and both configured is PASSING.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_hybrid_nodes_check_reads_the_remote_network_configuration() {
    let (router, _engine) = emulator_with_engine();
    let node_networks = serde_json::json!({
        "remoteNodeNetworks": [{ "cidrs": ["10.80.0.0/16"] }],
    });
    let both_networks = serde_json::json!({
        "remoteNodeNetworks": [{ "cidrs": ["10.80.0.0/16"] }],
        "remotePodNetworks": [{ "cidrs": ["10.81.0.0/16"] }],
    });
    // Rows differ in the remote networks the cluster was created with.
    let rows: Vec<(&str, Option<serde_json::Value>, &str)> = vec![
        ("no remote network configuration", None, INSIGHT_STATUS_UNKNOWN),
        (
            "remote node networks only",
            Some(node_networks),
            INSIGHT_STATUS_WARNING,
        ),
        (
            "remote node and pod networks",
            Some(both_networks),
            INSIGHT_STATUS_PASSING,
        ),
    ];

    for (index, (case, config, expected)) in rows.into_iter().enumerate() {
        let name = format!("hybrid-{index}");
        create_cluster(&router, &name, None, config).await;
        settle_creation().await;

        let listed = list_insights(&router, &name, &serde_json::json!({})).await;
        let insight = insight_of(case, &listed, InsightCheck::HybridNodeNetworks);
        assert_eq!(
            insight["insightStatus"]["status"],
            serde_json::json!(expected),
            "case {case:?}: expected {expected}, got {insight:?}"
        );
        assert_eq!(
            insight["category"],
            serde_json::json!(INSIGHT_CATEGORY_MISCONFIGURATION),
            "case {case:?}: a hybrid nodes finding is a misconfiguration, got {insight:?}"
        );
        assert!(
            insight["kubernetesVersion"].is_null(),
            "case {case:?}: the check is about no Kubernetes version, got {insight:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the rollback readiness check is PASSING, naming the version the cluster would
// return to, exactly when the cluster has completed a Kubernetes version UPDATE; a
// cluster that never completed one, and one whose last version change was itself a
// rollback, have no version update to roll back.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_rollback_check_follows_the_cluster_s_completed_version_changes() {
    let (router, engine) = emulator_with_engine();
    let (current, target) = version_pair(&engine);
    let rolled_back_to = previous_kubernetes_minor_version(&target)
        .unwrap_or_else(|| panic!("{target} must name a Kubernetes minor version"));
    assert_eq!(
        rolled_back_to, current,
        "setup: the version before {target} must be the version the cluster started at"
    );

    // Rows differ in the version changes the cluster has completed: none, one update
    // forward, and that update followed by a rollback of it.
    let rows: Vec<(&str, usize, &str, Option<String>)> = vec![
        (
            "a cluster that has never been updated",
            0,
            INSIGHT_STATUS_UNKNOWN,
            None,
        ),
        (
            "a cluster that completed a version update",
            1,
            INSIGHT_STATUS_PASSING,
            Some(current.clone()),
        ),
        (
            "a cluster whose last version change was a rollback",
            2,
            INSIGHT_STATUS_UNKNOWN,
            None,
        ),
    ];

    for (index, (case, changes, expected, expected_version)) in rows.iter().enumerate() {
        let name = format!("rollback-{index}");
        create_cluster(&router, &name, Some(&current), None).await;
        settle_creation().await;
        // Each change is one minor version, forward then back, which is what
        // `UpdateClusterVersion` mints a `VersionUpdate` and a `VersionRollback` for.
        for round in 0..*changes {
            let moved_to = if round % 2 == 0 { &target } else { &current };
            let answer = update_version(&router, &name, moved_to, false).await;
            assert_eq!(
                answer.status, 200,
                "setup {case:?}: expected 200 moving {name:?} to {moved_to:?}, got {answer:?}"
            );
            settle_update().await;
        }

        let listed = list_insights(&router, &name, &serde_json::json!({})).await;
        let insight = insight_of(case, &listed, InsightCheck::VersionRollback);
        assert_eq!(
            insight["insightStatus"]["status"],
            serde_json::json!(expected),
            "case {case:?}: expected {expected}, got {insight:?}"
        );
        assert_eq!(
            insight["kubernetesVersion"].as_str().map(str::to_owned),
            *expected_version,
            "case {case:?}: expected the version it would roll back to, got {insight:?}"
        );
        assert_eq!(
            insight["category"],
            serde_json::json!(INSIGHT_CATEGORY_ROLLBACK_READINESS),
            "case {case:?}: a rollback finding is reported under its own category, got {insight:?}"
        );
        // A completed version update is a change to the check's subject, so the
        // transition it reports is no earlier than the cluster's own creation.
        let created_at = instant(case, describe_cluster(&router, &name).await.cluster(), "createdAt");
        let transition = instant(case, &insight, "lastTransitionTime");
        assert!(
            transition >= created_at,
            "case {case:?}: expected a transition at or after the cluster's creation \
             ({created_at}), got {transition}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: "The default filter lists all categories and every status", and a filter
// member keeps exactly the insights whose category, associated Kubernetes version or
// status it names — several members narrowing together.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_filter_keeps_exactly_the_insights_it_names() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 3,
        "setup: expected at least three released versions, got {published:?}"
    );
    let (middle, oldest) = (published[1].clone(), published[2].clone());
    // ONE cluster whose checks report four different statuses, so a filter has
    // something to keep and something to drop in every category: a node group left
    // behind by a version update (ERROR), no incompatible add-on (PASSING), remote
    // node networks without pod networks (WARNING) and a completed rollback (UNKNOWN).
    let name = "filtered";
    create_cluster(
        &router,
        name,
        Some(&oldest),
        Some(serde_json::json!({
            "remoteNodeNetworks": [{ "cidrs": ["10.80.0.0/16"] }],
        })),
    )
    .await;
    settle_creation().await;
    create_nodegroup(&router, name, "left-behind").await;
    let carried = update_version(&router, name, &middle, false).await;
    assert_eq!(
        carried.status, 200,
        "setup: expected 200 moving {name:?} to {middle:?}, got {carried:?}"
    );
    settle_update().await;

    let unfiltered = list_insights(&router, name, &serde_json::json!({})).await;
    let all = insights("unfiltered", &unfiltered);
    let reported_statuses: Vec<&str> = all
        .iter()
        .map(|insight| {
            insight["insightStatus"]["status"]
                .as_str()
                .unwrap_or_default()
        })
        .collect();
    for status in [
        INSIGHT_STATUS_PASSING,
        INSIGHT_STATUS_WARNING,
        INSIGHT_STATUS_ERROR,
    ] {
        assert!(
            reported_statuses.contains(&status),
            "setup: the cluster's checks must span the statuses the rows filter on; {status} is \
             missing from {reported_statuses:?}"
        );
    }
    let a_version = all
        .iter()
        .find_map(|insight| insight["kubernetesVersion"].as_str())
        .unwrap_or_default()
        .to_owned();

    // Rows differ along which filter members are given, and how many values each
    // carries. The expected answer is computed from the unfiltered listing by the
    // same predicate the filter states, so no row writes out a count.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        ("no filter member", serde_json::json!({})),
        (
            "one category",
            serde_json::json!({ "categories": [INSIGHT_CATEGORY_UPGRADE_READINESS] }),
        ),
        (
            "two categories",
            serde_json::json!({
                "categories": [INSIGHT_CATEGORY_MISCONFIGURATION, INSIGHT_CATEGORY_ROLLBACK_READINESS],
            }),
        ),
        (
            "one status",
            serde_json::json!({ "statuses": [INSIGHT_STATUS_PASSING] }),
        ),
        (
            "two statuses",
            serde_json::json!({ "statuses": [INSIGHT_STATUS_ERROR, INSIGHT_STATUS_WARNING] }),
        ),
        (
            "a status nothing reports",
            serde_json::json!({ "statuses": [INSIGHT_STATUS_UNKNOWN, INSIGHT_STATUS_WARNING] }),
        ),
        (
            "the Kubernetes version the upgrade checks are about",
            serde_json::json!({ "kubernetesVersions": [a_version.clone()] }),
        ),
        (
            "a Kubernetes version no insight is about",
            serde_json::json!({ "kubernetesVersions": ["1.1"] }),
        ),
        (
            "a category and a status together",
            serde_json::json!({
                "categories": [INSIGHT_CATEGORY_UPGRADE_READINESS],
                "statuses": [INSIGHT_STATUS_ERROR],
            }),
        ),
        (
            "a category and a version that name different insights",
            serde_json::json!({
                "categories": [INSIGHT_CATEGORY_MISCONFIGURATION],
                "kubernetesVersions": [a_version.clone()],
            }),
        ),
    ];

    for (case, filter) in rows {
        let wanted_categories: Vec<String> = string_list(&filter, "categories");
        let wanted_statuses: Vec<String> = string_list(&filter, "statuses");
        let wanted_versions: Vec<String> = string_list(&filter, "kubernetesVersions");
        let expected: Vec<String> = all
            .iter()
            .filter(|insight| {
                let category = insight["category"].as_str().unwrap_or_default();
                let status = insight["insightStatus"]["status"]
                    .as_str()
                    .unwrap_or_default();
                let version = insight["kubernetesVersion"].as_str();
                (wanted_categories.is_empty()
                    || wanted_categories.iter().any(|wanted| wanted == category))
                    && (wanted_statuses.is_empty()
                        || wanted_statuses.iter().any(|wanted| wanted == status))
                    && (wanted_versions.is_empty()
                        || version.is_some_and(|version| {
                            wanted_versions.iter().any(|wanted| wanted == version)
                        }))
            })
            .map(|insight| insight["id"].as_str().unwrap_or_default().to_owned())
            .collect();

        let listed = list_insights(&router, name, &serde_json::json!({ "filter": filter })).await;
        let got: Vec<String> = insights(case, &listed)
            .iter()
            .map(|insight| insight["id"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected exactly the insights the filter names, got {listed:?}"
        );
    }
}

/// The values of one string-list member of a filter document, empty when the member
/// is absent.
fn string_list(filter: &serde_json::Value, member: &str) -> Vec<String> {
    filter[member]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: a filter token no modelled enum declares names no insight at all, which is
// an `InvalidParameterException` naming the member, the value and the members the
// enum accepts — not an empty page.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_filter_token_the_model_does_not_declare_is_refused() {
    let (router, _engine) = emulator_with_engine();
    create_cluster(&router, "bad-filter", None, None).await;
    settle_creation().await;

    // Rows differ along which member carries the unknown token and what is wrong with
    // it: a category that is not a member, a status that is not one, the right member
    // in the wrong case, and a value from the OTHER member's enum.
    let rows: Vec<(&str, serde_json::Value, &str, &str)> = vec![
        (
            "an unknown category",
            serde_json::json!({ "categories": ["COST_OPTIMIZATION"] }),
            "filter.categories",
            "COST_OPTIMIZATION",
        ),
        (
            "an unknown status",
            serde_json::json!({ "statuses": ["FAILING"] }),
            "filter.statuses",
            "FAILING",
        ),
        (
            "a status in the wrong case",
            serde_json::json!({ "statuses": ["passing"] }),
            "filter.statuses",
            "passing",
        ),
        (
            "a category value given as a status",
            serde_json::json!({ "statuses": [INSIGHT_CATEGORY_UPGRADE_READINESS] }),
            "filter.statuses",
            INSIGHT_CATEGORY_UPGRADE_READINESS,
        ),
        (
            "an unknown token beside a known one",
            serde_json::json!({
                "categories": [INSIGHT_CATEGORY_UPGRADE_READINESS, "UPGRADE"],
            }),
            "filter.categories",
            "UPGRADE",
        ),
    ];

    for (case, filter, member, token) in rows {
        let refused =
            list_insights(&router, "bad-filter", &serde_json::json!({ "filter": filter })).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from ListInsights, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {refused:?}"
        );
        let message = refused.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member) && message.contains(token),
            "case {case:?}: the refusal must name the member and the value, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `maxResults` bounds a page and `nextToken` resumes after the last insight of
// the previous one, so paging a listing walks every insight exactly once in the same
// order; a token this listing did not issue is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_walks_every_insight_exactly_once() {
    let (router, _engine) = emulator_with_engine();
    let name = "paged";
    create_cluster(&router, name, None, None).await;
    settle_creation().await;

    let unpaged: Vec<String> = insights(
        "unpaged",
        &list_insights(&router, name, &serde_json::json!({})).await,
    )
    .iter()
    .map(|insight| insight["id"].as_str().unwrap_or_default().to_owned())
    .collect();
    assert_eq!(
        unpaged.len(),
        InsightCheck::all().len(),
        "setup: expected one insight per check, got {unpaged:?}"
    );

    // Rows differ along the page size: every size from one insight at a time up to
    // one past the whole listing, plus a page bounded by a filter so the walk is of a
    // FILTERED listing too.
    let sizes: Vec<i32> = (1..=(unpaged.len() as i32 + 1)).collect();
    for size in sizes {
        let case = format!("maxResults {size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = serde_json::json!({ "maxResults": size });
            if let Some(token) = &token {
                body["nextToken"] = serde_json::json!(token);
            }
            let page = list_insights(&router, name, &body).await;
            let reported = insights(&case, &page);
            assert!(
                reported.len() <= size as usize,
                "case {case:?}: a page must not exceed maxResults, got {page:?}"
            );
            walked.extend(
                reported
                    .iter()
                    .map(|insight| insight["id"].as_str().unwrap_or_default().to_owned()),
            );
            pages += 1;
            assert!(
                pages <= unpaged.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {} insights",
                unpaged.len()
            );
            match page.body["nextToken"].as_str() {
                Some(next) if !next.is_empty() => token = Some(next.to_owned()),
                _ => break,
            }
        }
        assert_eq!(
            walked, unpaged,
            "case {case:?}: paging must walk every insight exactly once, in order"
        );
    }

    // A token this listing did not issue is a bad parameter, not an empty page — and
    // neither is a token carried over to a differently filtered call.
    let first = list_insights(&router, name, &serde_json::json!({ "maxResults": 1 })).await;
    let issued = first.body["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !issued.is_empty(),
        "setup: a page of one must leave a nextToken, got {first:?}"
    );
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "a token this service never issued",
            serde_json::json!({ "nextToken": "deadbeefdeadbeef" }),
        ),
        (
            "a token from the same listing, carried to a different filter",
            serde_json::json!({
                "nextToken": issued,
                "filter": { "categories": [INSIGHT_CATEGORY_MISCONFIGURATION] },
            }),
        ),
    ];
    for (case, body) in rows {
        let refused = list_insights(&router, name, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from ListInsights, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {refused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: insights are an analysis of a cluster Amazon EKS manages, so a cluster this
// region does not hold is the missing cluster and a cluster `RegisterCluster`
// connected has no insights at all.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_needs_an_eks_managed_cluster() {
    let (router, _engine) = emulator_with_engine();
    create_cluster(&router, "present", None, None).await;
    let registration = serde_json::json!({
        "name": "connected",
        "connectorConfig": { "roleArn": PASSABLE_ROLE, "provider": "EKS_ANYWHERE" },
    });
    let registered = call(
        &router,
        REGION,
        "POST",
        "/cluster-registrations",
        &registration.to_string(),
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "setup: expected 200 from RegisterCluster, got {registered:?}"
    );
    settle_creation().await;

    // Rows differ along what is wrong with the cluster the request names: no such
    // cluster, a name only another Region holds, and a cluster EKS does not manage.
    let rows: Vec<(&str, &str, &str, u16, &str)> = vec![
        (
            "no such cluster",
            REGION,
            "absent",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a cluster of another Region",
            "eu-west-1",
            "present",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a connected cluster",
            REGION,
            "connected",
            400,
            "ClientException",
        ),
    ];

    for (case, region, cluster, status, code) in rows {
        let answer = call(
            &router,
            region,
            "POST",
            &format!("/clusters/{}/insights", encode_label(cluster)),
            "{}",
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} from ListInsights, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            code,
            "case {case:?}: expected {code}, got {answer:?}"
        );
        if status == 404 {
            assert_eq!(
                answer.body["message"],
                serde_json::json!(cluster_not_found_message(cluster)),
                "case {case:?}: expected the service's missing-cluster sentence, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: `lastRefreshTime` is the instant of the cluster's latest COMPLETED insights
// analysis: the cluster's own creation until a refresh has completed, unmoved while
// one is still running, and that refresh's `endedAt` once the clock ends it — while
// `lastTransitionTime` is untouched by a refresh that found nothing new.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_completed_refresh_moves_the_last_refresh_time() {
    let (router, _engine) = emulator_with_engine();
    // Rows differ along how many refreshes the cluster runs in a row: each one moves
    // the analysis instant again.
    let rows: Vec<(&str, usize)> = vec![("one refresh", 1), ("three refreshes", 3)];

    for (index, (case, rounds)) in rows.iter().enumerate() {
        let name = format!("refreshed-{index}");
        create_cluster(&router, &name, None, None).await;
        settle_creation().await;
        let created_at = instant(
            case,
            describe_cluster(&router, &name).await.cluster(),
            "createdAt",
        );

        let before = list_insights(&router, &name, &serde_json::json!({})).await;
        for check in InsightCheck::all() {
            let insight = insight_of(case, &before, check);
            assert_eq!(
                instant(case, &insight, "lastRefreshTime"),
                created_at,
                "case {case:?}: before any refresh, {:?} reports the cluster's creation, got \
                 {insight:?}",
                check.name()
            );
        }
        let transitions: Vec<f64> = InsightCheck::all()
            .into_iter()
            .map(|check| instant(case, &insight_of(case, &before, check), "lastTransitionTime"))
            .collect();

        let mut previous_refresh = created_at;
        for round in 1..=*rounds {
            let started = start_refresh(&router, &name).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: round {round} expected 200 from StartInsightsRefresh, got \
                 {started:?}"
            );
            // A refresh that is still running has produced no analysis, so it has not
            // moved the instant the insights were last analysed at. The refresh is
            // read AFTER the listing, so a refresh still running then was running at
            // the listing too — a status only moves forward.
            let during = list_insights(&router, &name, &serde_json::json!({})).await;
            let still = instant(
                case,
                &insight_of(case, &during, InsightCheck::NodegroupVersions),
                "lastRefreshTime",
            );
            let running = describe_refresh(&router, &name).await;
            assert!(
                still >= previous_refresh,
                "case {case:?}: round {round} the analysis instant must never go backwards from \
                 {previous_refresh}, got {still}"
            );
            if running.body["status"] == serde_json::json!("IN_PROGRESS") {
                assert_eq!(
                    still, previous_refresh,
                    "case {case:?}: round {round} expected the previous analysis instant while \
                     the refresh runs, got {still}"
                );
            }

            tokio::time::sleep(INSIGHTS_REFRESH_DURATION + SLACK).await;
            let described = describe_refresh(&router, &name).await;
            assert_eq!(
                described.body["status"],
                serde_json::json!("COMPLETED"),
                "case {case:?}: round {round} the clock must complete the refresh, got \
                 {described:?}"
            );
            let ended_at = instant(case, &described.body, "endedAt");

            let after = list_insights(&router, &name, &serde_json::json!({})).await;
            for (position, check) in InsightCheck::all().into_iter().enumerate() {
                let insight = insight_of(case, &after, check);
                assert_eq!(
                    instant(case, &insight, "lastRefreshTime"),
                    ended_at,
                    "case {case:?}: round {round} {:?} must report the completed refresh as its \
                     last analysis, got {insight:?}",
                    check.name()
                );
                assert_eq!(
                    instant(case, &insight, "lastTransitionTime"),
                    transitions[position],
                    "case {case:?}: round {round} a refresh that found nothing new must not move \
                     {:?}'s transition, got {insight:?}",
                    check.name()
                );
            }
            assert!(
                ended_at > previous_refresh,
                "case {case:?}: round {round} expected a later analysis than \
                 {previous_refresh}, got {ended_at}"
            );
            previous_refresh = ended_at;
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: every insight a listing reports is readable by its id through
// `DescribeInsight`, which reports the same insight — the same status, instants and
// version — plus the check's recommendation and the resources it examined, each with
// its own status; an id that is not one of that cluster's insights is the missing
// insight.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_listed_insight_is_read_back_by_its_id() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 3,
        "setup: expected at least three released versions, got {published:?}"
    );
    let (middle, oldest) = (published[1].clone(), published[2].clone());
    // One cluster with something for each resource-bearing check to list: a node
    // group left behind by a version update, and an add-on installed at a build the
    // catalog does not publish for the version the cluster is moved to.
    let name = "described";
    create_cluster(&router, name, Some(&oldest), None).await;
    settle_creation().await;
    let nodegroup_arn = create_nodegroup(&router, name, "left-behind").await.body["nodegroup"]
        ["nodegroupArn"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let addon_arn = create_addon(&router, name, &build_not_published_for(&engine, &middle))
        .await
        .addon()["addonArn"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let carried = update_version(&router, name, &middle, true).await;
    assert_eq!(
        carried.status, 200,
        "setup: expected 200 moving {name:?} to {middle:?} with force, got {carried:?}"
    );
    settle_update().await;

    let listed = list_insights(&router, name, &serde_json::json!({})).await;

    // Rows: every check of the cluster, each with the resources its own subject is.
    for check in InsightCheck::all() {
        let case = check.name();
        let summary = insight_of(case, &listed, check);
        let id = summary["id"].as_str().unwrap_or_default().to_owned();
        let described = describe_insight(&router, name, &id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeInsight for {id:?}, got {described:?}"
        );
        let insight = &described.body["insight"];
        for member in [
            "id",
            "name",
            "category",
            "description",
            "kubernetesVersion",
            "lastRefreshTime",
            "lastTransitionTime",
            "insightStatus",
        ] {
            assert_eq!(
                insight[member], summary[member],
                "case {case:?}: DescribeInsight must report the listing's {member}, got \
                 {described:?}"
            );
        }
        assert_eq!(
            insight["recommendation"],
            serde_json::json!(check.recommendation()),
            "case {case:?}: expected the check's own recommendation, got {described:?}"
        );

        // The resources a check examined: the node groups for the node group check,
        // the add-ons for the add-on check, and none for the checks whose subject is
        // the cluster itself.
        let expected_arns: Vec<String> = match check {
            InsightCheck::NodegroupVersions => vec![nodegroup_arn.clone()],
            InsightCheck::AddonVersions => vec![addon_arn.clone()],
            InsightCheck::HybridNodeNetworks | InsightCheck::VersionRollback => Vec::new(),
        };
        let reported: Vec<String> = insight["resources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|resource| resource["arn"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            reported, expected_arns,
            "case {case:?}: expected the resources the check examined, got {described:?}"
        );
        for resource in insight["resources"].as_array().cloned().unwrap_or_default() {
            // Both of this cluster's listed resources are behind the version it now
            // runs, so each one carries the finding that the insight reports.
            assert_eq!(
                resource["insightStatus"]["status"],
                serde_json::json!(INSIGHT_STATUS_ERROR),
                "case {case:?}: expected the resource's own status, got {resource:?}"
            );
            assert!(
                resource["insightStatus"]["reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: expected a reason for the resource's status, got {resource:?}"
            );
        }

        // A read changes nothing a later caller sees.
        let again = describe_insight(&router, name, &id).await;
        assert_eq!(
            again.body, described.body,
            "case {case:?}: a second read must report the same insight, got {again:?}"
        );
    }

    // And an id that is not one of this cluster's insights is the missing insight.
    create_cluster(&router, "other", Some(&oldest), None).await;
    settle_creation().await;
    let other = list_insights(&router, "other", &serde_json::json!({})).await;
    let other_id = insight_of("other cluster", &other, InsightCheck::NodegroupVersions)["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let rows: Vec<(&str, String)> = vec![
        ("an id this service never issued", "not-an-insight".to_string()),
        ("the same check's id on another cluster", other_id),
    ];
    for (case, id) in rows {
        let refused = describe_insight(&router, name, &id).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404 from DescribeInsight, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert!(
            refused
                .body
                .get("clusterName")
                .is_none_or(serde_json::Value::is_null),
            "case {case:?}: a missing insight names no cluster member, got {refused:?}"
        );
    }
}
