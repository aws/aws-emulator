// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeUpdateActions`.
//!
//! An update action is not a resource a caller creates: ElastiCache has one for
//! every (published service update, resource) pair the update applies to, and
//! `DescribeUpdateActions` reports the pair nothing has started as
//! `not-applied`. These tests assert exactly that rule: the reported page is the
//! published catalog crossed with the clusters and replication groups this
//! account holds, narrowed by the request's filters, with each action's status
//! taken from the record `BatchApplyUpdateAction` / `BatchStopUpdateAction`
//! persisted and the clock has carried.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, and the catalog and the resources are read back through
//! `DescribeServiceUpdates`, `DescribeCacheClusters` and
//! `DescribeReplicationGroups` rather than written here — so a response that is
//! not computed from the published catalog and persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (which filter narrows the page and how, the engine of
//! the resource, the number of nodes it holds, the page size, the status the
//! action has been carried to, and the refused value).

use std::collections::BTreeMap;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::create_available_cluster;
use crate::harness::{params, poll_until, Emulator, Reply, REGION};
use crate::service_updates::{epoch_secs, now_secs};

/// One published service update, as `DescribeServiceUpdates` reports it: the
/// members an update action repeats, plus the dates the SLA and the time-range
/// filter are read against.
#[derive(Debug, Clone)]
struct CatalogUpdate {
    name: String,
    engine: String,
    severity: String,
    status: String,
    update_type: String,
    estimated_update_time: String,
    release_secs: i64,
    apply_by_secs: i64,
    end_secs: i64,
}

/// One resource an update action can be reported for: the member its id is
/// reported under, the id, the engine that decides which updates apply to it,
/// and how many nodes the update would run on.
#[derive(Debug, Clone)]
struct Resource {
    member: &'static str,
    id: String,
    engine: String,
    nodes: usize,
}

/// `CacheClusterId` sorts before `ReplicationGroupId`, which is the order the
/// service reports the two kinds of resource in within one service update.
fn kind_rank(member: &str) -> u8 {
    u8::from(member == "ReplicationGroupId")
}

/// The whole published catalog, as `DescribeServiceUpdates` reports it.
async fn catalog(emulator: &Emulator) -> Vec<CatalogUpdate> {
    let case = "catalog";
    let reply = emulator.call("DescribeServiceUpdates", &[]).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: the catalog must be reported, got {} {}",
        reply.status, reply.body
    );
    let names = reply.all("ServiceUpdateName");
    let column = |member: &str| -> Vec<String> {
        let values = reply.all(member);
        assert_eq!(
            values.len(),
            names.len(),
            "case {case}: {member} must be reported once per published update, got {}",
            reply.body
        );
        values.into_iter().map(str::to_string).collect()
    };
    let engines = column("Engine");
    let severities = column("ServiceUpdateSeverity");
    let statuses = column("ServiceUpdateStatus");
    let types = column("ServiceUpdateType");
    let estimated = column("EstimatedUpdateTime");
    let releases = column("ServiceUpdateReleaseDate");
    let apply_bys = column("ServiceUpdateRecommendedApplyByDate");
    let ends = column("ServiceUpdateEndDate");
    let updates: Vec<CatalogUpdate> = (0..names.len())
        .map(|index| CatalogUpdate {
            name: names[index].to_string(),
            engine: engines[index].clone(),
            severity: severities[index].clone(),
            status: statuses[index].clone(),
            update_type: types[index].clone(),
            estimated_update_time: estimated[index].clone(),
            release_secs: epoch_secs(case, "ServiceUpdateReleaseDate", &releases[index]),
            apply_by_secs: epoch_secs(
                case,
                "ServiceUpdateRecommendedApplyByDate",
                &apply_bys[index],
            ),
            end_secs: epoch_secs(case, "ServiceUpdateEndDate", &ends[index]),
        })
        .collect();
    assert!(
        !updates.is_empty(),
        "case {case}: the service publishes a catalog of updates, got {}",
        reply.body
    );
    updates
}

/// The one published update of `engine` whose availability window is still open,
/// which is the update these tests apply.
fn available_update_for(published: &[CatalogUpdate], engine: &str) -> CatalogUpdate {
    published
        .iter()
        .find(|update| update.engine == engine && update.status == state::SERVICE_UPDATE_AVAILABLE)
        .unwrap_or_else(|| panic!("the catalog publishes an available update for {engine}"))
        .clone()
}

/// Every resource this account holds that an update action can be reported for,
/// read back through the describes that own them: the cache clusters (including
/// the member clusters of a replication group, which are ordinary clusters) and
/// the replication groups `group_ids` names.
async fn inventory(emulator: &Emulator, group_ids: &[&str]) -> Vec<Resource> {
    let clusters = emulator.call("DescribeCacheClusters", &[]).await;
    assert_eq!(
        clusters.status, 200,
        "the stored clusters must be reported, got {} {}",
        clusters.status, clusters.body
    );
    let ids = clusters.all("CacheClusterId");
    let engines = clusters.all("Engine");
    let counts = clusters.all("NumCacheNodes");
    assert!(
        ids.len() == engines.len() && ids.len() == counts.len(),
        "every cluster must report its engine and node count, got {}",
        clusters.body
    );
    let mut resources: Vec<Resource> = (0..ids.len())
        .map(|index| Resource {
            member: "CacheClusterId",
            id: ids[index].to_string(),
            engine: engines[index].to_string(),
            nodes: counts[index]
                .parse()
                .unwrap_or_else(|_| panic!("NumCacheNodes {:?} is a count", counts[index])),
        })
        .collect();
    for group_id in group_ids {
        let group = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", group_id)]),
            )
            .await;
        assert_eq!(
            group.status, 200,
            "case {group_id}: the stored group must be reported, got {} {}",
            group.status, group.body
        );
        let members = group.all("NodeGroupMember/CacheClusterId").len();
        assert!(
            members > 0,
            "case {group_id}: a replication group holds at least one member, got {}",
            group.body
        );
        resources.push(Resource {
            member: "ReplicationGroupId",
            id: (*group_id).to_string(),
            engine: group
                .text("Engine")
                .unwrap_or_else(|| panic!("case {group_id}: the group reports its engine"))
                .to_string(),
            nodes: members,
        });
    }
    resources
}

/// Every `UpdateAction` a reply reports, in document order, each as its members
/// keyed by name.
///
/// A record is closed when a member repeats, or when one of the two id members
/// that OPEN a record arrives after a complete one — which is what keeps a page
/// mixing cache clusters and replication groups from being read as one record.
/// Callers that ask for node-level status assert it with [`Reply::all`] instead:
/// the nested lists repeat the member names this reader splits on.
fn wire_actions(reply: &Reply) -> Vec<BTreeMap<String, String>> {
    let mut actions: Vec<BTreeMap<String, String>> = Vec::new();
    let mut pending: BTreeMap<String, String> = BTreeMap::new();
    for (path, text) in &reply.nodes {
        if !path.contains("/UpdateActions/") {
            continue;
        }
        let member = path.rsplit('/').next().unwrap_or_default();
        let opens_a_record = matches!(member, "CacheClusterId" | "ReplicationGroupId")
            && pending.contains_key("ServiceUpdateName");
        if opens_a_record || pending.contains_key(member) {
            actions.push(std::mem::take(&mut pending));
        }
        pending.insert(member.to_string(), text.clone());
    }
    if !pending.is_empty() {
        actions.push(pending);
    }
    actions
}

/// One member of a reported update action.
fn member<'a>(action: &'a BTreeMap<String, String>, case: &str, name: &str) -> &'a str {
    action
        .get(name)
        .unwrap_or_else(|| panic!("case {case}: an update action must report {name}: {action:?}"))
        .as_str()
}

/// The `(ServiceUpdateName, id member, id)` an action is reported for — the
/// identity of an update action in this API.
fn identity(case: &str, action: &BTreeMap<String, String>) -> (String, &'static str, String) {
    let member_name = if action.contains_key("CacheClusterId") {
        "CacheClusterId"
    } else {
        "ReplicationGroupId"
    };
    assert!(
        !(action.contains_key("CacheClusterId") && action.contains_key("ReplicationGroupId")),
        "case {case}: an update action belongs to ONE resource, got {action:?}"
    );
    (
        member(action, case, "ServiceUpdateName").to_string(),
        member_name,
        member(action, case, member_name).to_string(),
    )
}

/// The ONE action a request naming one update and one resource reports, read
/// through two calls: the record comes from a describe WITHOUT node-level
/// status, which is what [`wire_actions`] can split into records, and the reply
/// returned beside it is the same request WITH node-level status, whose nested
/// lists are asserted with [`Reply::all`].
async fn action_and_nodes(
    emulator: &Emulator,
    case: &str,
    pairs: &[(String, String)],
) -> (BTreeMap<String, String>, Reply) {
    let (reply, actions) = describe(emulator, case, pairs).await;
    assert_eq!(
        actions.len(),
        1,
        "case {case}: one update and one resource name one action, got {}",
        reply.body
    );
    let action = actions
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case}: the one action"));
    let mut with_nodes = pairs.to_vec();
    with_nodes.push(("ShowNodeLevelUpdateStatus".to_string(), "true".to_string()));
    let (nodes_reply, _) = describe(emulator, case, &with_nodes).await;
    (action, nodes_reply)
}

/// One `DescribeUpdateActions` call that must succeed, with the actions it
/// reported.
async fn describe(
    emulator: &Emulator,
    case: &str,
    pairs: &[(String, String)],
) -> (Reply, Vec<BTreeMap<String, String>>) {
    let reply = emulator.call("DescribeUpdateActions", pairs).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeUpdateActions must answer 200, got {} {}",
        reply.status, reply.body
    );
    let actions = wire_actions(&reply);
    (reply, actions)
}

/// The identities a describe reported, in document order.
async fn reported_identities(
    emulator: &Emulator,
    case: &str,
    pairs: &[(String, String)],
) -> Vec<(String, &'static str, String)> {
    let (_, actions) = describe(emulator, case, pairs).await;
    actions
        .iter()
        .map(|action| identity(case, action))
        .collect()
}

/// The form pairs that carry `ids` in the list filter `member` names.
fn id_filter(member: &str, ids: &[&str]) -> Vec<(String, String)> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| (format!("{member}.member.{}", index + 1), (*id).to_string()))
        .collect()
}

/// `at`, in seconds since the epoch, as a timestamp member on the wire.
fn wire_time(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .expect("an instant this century renders")
}

/// Create one replication group running `engine`, named `id`, and wait out the
/// span after which the clock owes it `available`.
async fn create_group(emulator: &Emulator, id: &str, engine: &str) {
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", "an emulated replication group"),
                ("Engine", engine),
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {id}: CreateReplicationGroup must succeed, got {} {}",
        created.status, created.body
    );
    // Read it back: the read is answered once the create has settled the group,
    // which is the state an update action is applied to.
    let described = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await;
    assert_eq!(
        described.text("ReplicationGroup/Status"),
        Some("available"),
        "case {id}: the read after the create must report the group available, got {}",
        described.body
    );
}

/// Apply one published service update to one cache cluster, answering with the
/// call's own reply.
async fn apply(emulator: &Emulator, service_update_name: &str, cluster: &str) -> Reply {
    let mut pairs = params(&[("ServiceUpdateName", service_update_name)]);
    pairs.extend(id_filter("CacheClusterIds", &[cluster]));
    let applied = emulator.call("BatchApplyUpdateAction", &pairs).await;
    assert_eq!(
        applied.status, 200,
        "case apply {service_update_name} to {cluster}: must answer the results message, got {} {}",
        applied.status, applied.body
    );
    applied
}

/// The three resources and the one replication group every test below starts
/// from: one cluster per engine the service publishes updates for, a
/// multi-node Memcached cluster among them, and a replication group.
async fn stocked() -> (Emulator, Vec<CatalogUpdate>, Vec<Resource>) {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "ua-redis", &[("Engine", "redis")]).await;
    create_available_cluster(
        &emulator,
        "ua-memcached",
        &[("Engine", "memcached"), ("NumCacheNodes", "3")],
    )
    .await;
    create_available_cluster(&emulator, "ua-valkey", &[("Engine", "valkey")]).await;
    create_group(&emulator, "ua-group", "redis").await;
    let published = catalog(&emulator).await;
    let resources = inventory(&emulator, &["ua-group"]).await;
    (emulator, published, resources)
}

/// Every (update, resource) pair whose engines match, in the order the service
/// reports them.
fn expected_pairs(
    published: &[CatalogUpdate],
    resources: &[Resource],
) -> Vec<(String, &'static str, String)> {
    let mut pairs: Vec<(String, &'static str, String)> = Vec::new();
    for update in published {
        for resource in resources {
            if resource.engine == update.engine {
                pairs.push((update.name.clone(), resource.member, resource.id.clone()));
            }
        }
    }
    pairs.sort_by(|left, right| {
        (&left.0, kind_rank(left.1), &left.2).cmp(&(&right.0, kind_rank(right.1), &right.2))
    });
    pairs
}

#[tokio::test]
async fn every_published_update_has_an_action_for_every_resource_its_engine_applies_to() {
    let before = now_secs();
    let (emulator, published, resources) = stocked().await;
    let case = "unfiltered";
    let (reply, reported) = describe(&emulator, case, &[]).await;

    // The axis: the engine of the resource (one cluster per engine the catalog
    // publishes updates for), the KIND of resource (a cache cluster and a
    // replication group, including the group's own member clusters) and the
    // number of nodes the update runs on.
    let expected = expected_pairs(&published, &resources);
    let got: Vec<(String, &'static str, String)> = reported
        .iter()
        .map(|action| identity(case, action))
        .collect();
    assert_eq!(
        got, expected,
        "case {case}: every published update must have an action for every resource its engine \
         applies to, got {}",
        reply.body
    );

    let now = now_secs();
    for action in &reported {
        let (name, id_member, id) = identity(case, action);
        let case = format!("{name} on {id}");
        let update = published
            .iter()
            .find(|update| update.name == name)
            .unwrap_or_else(|| panic!("case {case}: the catalog publishes {name}"));
        let resource = resources
            .iter()
            .find(|resource| resource.member == id_member && resource.id == id)
            .unwrap_or_else(|| panic!("case {case}: this account holds {id}"));

        // The service update's own members are reported exactly as the catalog
        // has them.
        for (name, want, got) in [
            ("Engine", &update.engine, member(action, &case, "Engine")),
            (
                "ServiceUpdateSeverity",
                &update.severity,
                member(action, &case, "ServiceUpdateSeverity"),
            ),
            (
                "ServiceUpdateStatus",
                &update.status,
                member(action, &case, "ServiceUpdateStatus"),
            ),
            (
                "ServiceUpdateType",
                &update.update_type,
                member(action, &case, "ServiceUpdateType"),
            ),
            (
                "EstimatedUpdateTime",
                &update.estimated_update_time,
                member(action, &case, "EstimatedUpdateTime"),
            ),
        ] {
            assert_eq!(
                got, want,
                "case {case}: {name} must be the catalog's, got {got:?}"
            );
        }
        for (name, want) in [
            ("ServiceUpdateReleaseDate", update.release_secs),
            (
                "ServiceUpdateRecommendedApplyByDate",
                update.apply_by_secs,
            ),
        ] {
            let got = epoch_secs(&case, name, member(action, &case, name));
            assert_eq!(
                got, want,
                "case {case}: {name} must be the catalog's {want}, got {got}"
            );
        }

        // Nothing has applied anything, so every action is `not-applied` and no
        // node has been updated.
        assert_eq!(
            member(action, &case, "UpdateActionStatus"),
            state::UPDATE_ACTION_NOT_APPLIED,
            "case {case}: an action nothing has started must be {}, got {action:?}",
            state::UPDATE_ACTION_NOT_APPLIED
        );
        assert_eq!(
            member(action, &case, "NodesUpdated"),
            format!("0/{}", resource.nodes),
            "case {case}: {} nodes hold the update, none updated, got {action:?}",
            resource.nodes
        );

        // "The date that the service update is available to a replication
        // group": the update is available to a resource once both exist, so a
        // resource created during this test gains it at its own creation, and
        // that is also when its action became `not-applied`.
        let available = epoch_secs(&case, "UpdateActionAvailableDate", member(action, &case, "UpdateActionAvailableDate"));
        let modified = epoch_secs(
            &case,
            "UpdateActionStatusModifiedDate",
            member(action, &case, "UpdateActionStatusModifiedDate"),
        );
        assert!(
            available >= update.release_secs && (before..=now).contains(&available),
            "case {case}: the action became available with the resource, between {before} and \
             {now} and not before the update was released ({}), got {available}",
            update.release_secs
        );
        assert_eq!(
            modified, available,
            "case {case}: a never-applied action last changed status when it became available \
             ({available}), got {modified}"
        );

        // `SlaMet`: nothing has been updated, so the SLA is missed once the
        // recommended apply-by date has passed and is not yet answerable before
        // it.
        let want_sla = if now > update.apply_by_secs {
            state::SLA_MET_NO
        } else {
            state::SLA_MET_NOT_APPLICABLE
        };
        assert_eq!(
            member(action, &case, "SlaMet"),
            want_sla,
            "case {case}: the apply-by date is {} and now is {now}, so SlaMet must be \
             {want_sla}, got {action:?}",
            update.apply_by_secs
        );
    }

    // The catalog spans both sides of the apply-by rule, so the table above
    // covered both answers.
    for want in [state::SLA_MET_NO, state::SLA_MET_NOT_APPLICABLE] {
        assert!(
            reply.all("SlaMet").contains(&want),
            "case {case}: the published catalog must produce an action whose SlaMet is {want}, \
             got {:?}",
            reply.all("SlaMet")
        );
    }

    // `ShowNodeLevelUpdateStatus` was not asked for, so no node-level status is
    // reported.
    assert!(
        reply.all("NodeUpdateStatus").is_empty() && reply.all("NodeGroupId").is_empty(),
        "case {case}: node-level status must be reported only when it is asked for, got {}",
        reply.body
    );

    // A describe is a read: the second call sees exactly what the first did.
    let (_, again) = describe(&emulator, "repeated read", &[]).await;
    assert_eq!(
        again, reported,
        "case repeated read: a describe must not change what a later caller sees"
    );
}

#[tokio::test]
async fn each_filter_reports_exactly_the_actions_it_names() {
    let (emulator, published, resources) = stocked().await;
    let baseline = expected_pairs(&published, &resources);
    let redis = available_update_for(&published, "redis");
    let expired = published
        .iter()
        .find(|update| update.status == state::SERVICE_UPDATE_EXPIRED)
        .expect("the catalog publishes an expired update")
        .clone();

    /// One row: the case, the form pairs that filter the page, and the
    /// predicate the reported pairs must satisfy.
    type Row<'a> = (
        String,
        Vec<(String, String)>,
        Box<dyn Fn(&(String, &'static str, String)) -> bool + 'a>,
    );
    // The axis: which filter narrows the page, and how — one name, an engine,
    // one or several ids of either kind, an id in another case, an id this
    // account does not hold, a status on each side of the clock's rule, and two
    // filters at once.
    let rows: Vec<Row> = vec![
        (
            "no filter".to_string(),
            Vec::new(),
            Box::new(|_| true),
        ),
        (
            format!("ServiceUpdateName {}", redis.name),
            params(&[("ServiceUpdateName", &redis.name)]),
            Box::new({
                let name = redis.name.clone();
                move |pair: &(String, &'static str, String)| pair.0 == name
            }),
        ),
        (
            "ServiceUpdateName the catalog does not hold".to_string(),
            params(&[("ServiceUpdateName", "no-such-update")]),
            Box::new(|_| false),
        ),
        (
            "Engine memcached".to_string(),
            params(&[("Engine", "memcached")]),
            Box::new({
                let names: Vec<String> = published
                    .iter()
                    .filter(|update| update.engine == "memcached")
                    .map(|update| update.name.clone())
                    .collect();
                move |pair: &(String, &'static str, String)| names.contains(&pair.0)
            }),
        ),
        (
            "Engine Valkey in another case".to_string(),
            params(&[("Engine", "Valkey")]),
            Box::new({
                let names: Vec<String> = published
                    .iter()
                    .filter(|update| update.engine == "valkey")
                    .map(|update| update.name.clone())
                    .collect();
                move |pair: &(String, &'static str, String)| names.contains(&pair.0)
            }),
        ),
        (
            "one CacheClusterId".to_string(),
            id_filter("CacheClusterIds", &["ua-redis"]),
            Box::new(|pair: &(String, &'static str, String)| {
                pair.1 == "CacheClusterId" && pair.2 == "ua-redis"
            }),
        ),
        (
            "two CacheClusterIds, one in another case".to_string(),
            id_filter("CacheClusterIds", &["Ua-Redis", "ua-memcached"]),
            Box::new(|pair: &(String, &'static str, String)| {
                pair.1 == "CacheClusterId" && ["ua-redis", "ua-memcached"].contains(&pair.2.as_str())
            }),
        ),
        (
            "a CacheClusterId this account does not hold".to_string(),
            id_filter("CacheClusterIds", &["no-such-cluster"]),
            Box::new(|_| false),
        ),
        (
            "one ReplicationGroupId".to_string(),
            id_filter("ReplicationGroupIds", &["ua-group"]),
            Box::new(|pair: &(String, &'static str, String)| {
                pair.1 == "ReplicationGroupId" && pair.2 == "ua-group"
            }),
        ),
        (
            "both id lists at once".to_string(),
            {
                let mut pairs = id_filter("CacheClusterIds", &["ua-redis"]);
                pairs.extend(id_filter("ReplicationGroupIds", &["ua-group"]));
                pairs
            },
            Box::new(|pair: &(String, &'static str, String)| {
                (pair.1 == "CacheClusterId" && pair.2 == "ua-redis")
                    || (pair.1 == "ReplicationGroupId" && pair.2 == "ua-group")
            }),
        ),
        (
            format!("ServiceUpdateStatus {}", state::SERVICE_UPDATE_AVAILABLE),
            params(&[(
                "ServiceUpdateStatus.member.1",
                state::SERVICE_UPDATE_AVAILABLE,
            )]),
            Box::new({
                let names: Vec<String> = published
                    .iter()
                    .filter(|update| update.status == state::SERVICE_UPDATE_AVAILABLE)
                    .map(|update| update.name.clone())
                    .collect();
                move |pair: &(String, &'static str, String)| names.contains(&pair.0)
            }),
        ),
        (
            format!("ServiceUpdateStatus {}", state::SERVICE_UPDATE_CANCELLED),
            params(&[(
                "ServiceUpdateStatus.member.1",
                state::SERVICE_UPDATE_CANCELLED,
            )]),
            Box::new(|_| false),
        ),
        (
            format!("UpdateActionStatus {}", state::UPDATE_ACTION_NOT_APPLIED),
            params(&[(
                "UpdateActionStatus.member.1",
                state::UPDATE_ACTION_NOT_APPLIED,
            )]),
            Box::new(|_| true),
        ),
        (
            format!("UpdateActionStatus {}", state::UPDATE_ACTION_COMPLETE),
            params(&[(
                "UpdateActionStatus.member.1",
                state::UPDATE_ACTION_COMPLETE,
            )]),
            Box::new(|_| false),
        ),
        (
            "a name and an id together".to_string(),
            {
                let mut pairs = params(&[("ServiceUpdateName", &redis.name)]);
                pairs.extend(id_filter("CacheClusterIds", &["ua-redis"]));
                pairs
            },
            Box::new({
                let name = redis.name.clone();
                move |pair: &(String, &'static str, String)| {
                    pair.0 == name && pair.1 == "CacheClusterId" && pair.2 == "ua-redis"
                }
            }),
        ),
        (
            "a name and an engine that cannot hold together".to_string(),
            params(&[("ServiceUpdateName", &expired.name), ("Engine", "memcached")]),
            Box::new({
                let name = expired.name.clone();
                let memcached = expired.engine == "memcached";
                move |pair: &(String, &'static str, String)| memcached && pair.0 == name
            }),
        ),
    ];

    for (case, pairs, keep) in rows {
        let expected: Vec<(String, &'static str, String)> =
            baseline.iter().filter(|pair| keep(pair)).cloned().collect();
        let got = reported_identities(&emulator, &case, &pairs).await;
        assert_eq!(
            got, expected,
            "case {case}: expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn the_time_range_filter_keeps_the_updates_available_during_it() {
    let (emulator, published, resources) = stocked().await;
    let baseline = expected_pairs(&published, &resources);
    let now = now_secs();
    let day = 24 * 60 * 60;

    // The axis: where the range sits relative to an update's availability
    // window (release date to end date), and which of its two bounds the
    // request names. The catalog publishes one update per engine on each side,
    // so every row has records to keep and records to drop.
    let rows: [(&str, Option<i64>, Option<i64>); 6] = [
        ("around now", Some(now - day), Some(now + day)),
        ("long before any release", Some(now - 400 * day), Some(now - 300 * day)),
        ("over the older window only", Some(now - 200 * day), Some(now - 170 * day)),
        ("open start, ending before the recent release", None, Some(now - 170 * day)),
        ("open end, starting yesterday", Some(now - day), None),
        ("the whole span", Some(now - 400 * day), Some(now + 400 * day)),
    ];
    for (case, start, end) in rows {
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Some(start) = start {
            pairs.push((
                "ServiceUpdateTimeRange.StartTime".to_string(),
                wire_time(start),
            ));
        }
        if let Some(end) = end {
            pairs.push(("ServiceUpdateTimeRange.EndTime".to_string(), wire_time(end)));
        }
        // An update was available during the range when its window overlaps it:
        // it had not ended before the range opened and had been released by the
        // time the range closed.
        let expected: Vec<(String, &'static str, String)> = baseline
            .iter()
            .filter(|pair| {
                let update = published
                    .iter()
                    .find(|update| update.name == pair.0)
                    .expect("every reported action names a published update");
                start.is_none_or(|start| update.end_secs > start)
                    && end.is_none_or(|end| update.release_secs <= end)
            })
            .cloned()
            .collect();
        let got = reported_identities(&emulator, case, &pairs).await;
        assert_eq!(
            got, expected,
            "case {case}: expected {} actions, got {}",
            expected.len(),
            got.len()
        );
    }
}

#[tokio::test]
async fn an_applied_action_is_reported_as_the_clock_carries_it() {
    let (emulator, published, resources) = stocked().await;
    let update = available_update_for(&published, "memcached");
    let nodes = resources
        .iter()
        .find(|resource| resource.id == "ua-memcached")
        .expect("the memcached cluster is stored")
        .nodes;

    let mut pairs = params(&[("ServiceUpdateName", &update.name)]);
    pairs.extend(id_filter("CacheClusterIds", &["ua-memcached"]));

    // Before the apply: `not-applied`, nothing updated, and no node reports an
    // initiator or a start date, because nothing has been triggered.
    let case = "before the apply";
    let (before, reply) = action_and_nodes(&emulator, case, &pairs).await;
    assert_eq!(
        member(&before, case, "UpdateActionStatus"),
        state::UPDATE_ACTION_NOT_APPLIED,
        "case {case}: got {}",
        reply.body
    );
    assert_eq!(
        reply.all("CacheNodeUpdateStatus/NodeUpdateStatus"),
        vec![state::UPDATE_ACTION_NOT_APPLIED; nodes],
        "case before the apply: every one of the {nodes} nodes must report the action's status, \
         got {}",
        reply.body
    );
    assert!(
        reply.all("NodeUpdateInitiatedBy").is_empty()
            && reply.all("NodeUpdateStartDate").is_empty()
            && reply.all("NodeUpdateEndDate").is_empty(),
        "case before the apply: nothing triggered the update, so no node has been started, got {}",
        reply.body
    );

    // Applied, the action is under way: the apply's own answer reports
    // `waiting-to-start`, and the report that follows is answered once the clock
    // has carried the action its next step, with every node reporting the
    // customer as the initiator.
    let applied_reply = apply(&emulator, &update.name, "ua-memcached").await;
    let applied_at = now_secs();
    let case = "just applied";
    assert_eq!(
        applied_reply.text("UpdateActionStatus"),
        Some(state::UPDATE_ACTION_WAITING_TO_START),
        "case {case}: the apply must start the action, got {}",
        applied_reply.body
    );
    let case = "under way";
    let (applied, reply) = action_and_nodes(&emulator, case, &pairs).await;
    assert_eq!(
        member(&applied, case, "UpdateActionStatus"),
        state::UPDATE_ACTION_IN_PROGRESS,
        "case {case}: the clock must carry the action to its next status, got {}",
        reply.body
    );
    assert_eq!(
        member(&applied, case, "NodesUpdated"),
        format!("0/{nodes}"),
        "case {case}: no node has been updated yet, got {}",
        reply.body
    );
    assert_eq!(
        reply.all("NodeUpdateInitiatedBy"),
        vec![state::NODE_UPDATE_INITIATED_BY_CUSTOMER; nodes],
        "case {case}: the update was asked for by the customer, got {}",
        reply.body
    );
    for initiated in reply.all("NodeUpdateInitiatedDate") {
        let at = epoch_secs(case, "NodeUpdateInitiatedDate", initiated);
        assert!(
            (applied_at - 5..=applied_at + 5).contains(&at),
            "case {case}: the update was triggered at the apply ({applied_at}), got {at}"
        );
    }

    // The clock carries it through `in-progress` to `complete`: every node is
    // updated, the progress reports all of them, and the SLA was met because it
    // finished before the recommended apply-by date. An action walks a chain of
    // statuses, so this waits for the CONDITION — looking again until the action
    // reports complete — rather than for a time.
    let mut completion = pairs.clone();
    completion.push((
        "ShowNodeLevelUpdateStatus".to_string(),
        "true".to_string(),
    ));
    poll_until(
        &emulator,
        REGION,
        "settled",
        "the update action reaching complete",
        "DescribeUpdateActions",
        &completion,
        |reply| reply.text("UpdateActionStatus") == Some(state::UPDATE_ACTION_COMPLETE),
    )
    .await;
    let case = "settled";
    let (settled, reply) = action_and_nodes(&emulator, case, &pairs).await;
    assert_eq!(
        member(&settled, case, "UpdateActionStatus"),
        state::UPDATE_ACTION_COMPLETE,
        "case {case}: the clock must carry the action to completion, got {}",
        reply.body
    );
    assert_eq!(
        member(&settled, case, "NodesUpdated"),
        format!("{nodes}/{nodes}"),
        "case {case}: a complete action has updated every node, got {}",
        reply.body
    );
    assert_eq!(
        member(&settled, case, "SlaMet"),
        state::SLA_MET_YES,
        "case {case}: the update finished before {}, so the SLA was met, got {}",
        update.apply_by_secs,
        reply.body
    );
    assert_eq!(
        reply.all("CacheNodeUpdateStatus/NodeUpdateStatus"),
        vec![state::UPDATE_ACTION_COMPLETE; nodes],
        "case {case}: every node must report the action's status, got {}",
        reply.body
    );
    let started: Vec<i64> = reply
        .all("NodeUpdateStartDate")
        .iter()
        .map(|text| epoch_secs(case, "NodeUpdateStartDate", text))
        .collect();
    let ended: Vec<i64> = reply
        .all("NodeUpdateEndDate")
        .iter()
        .map(|text| epoch_secs(case, "NodeUpdateEndDate", text))
        .collect();
    assert_eq!(
        (started.len(), ended.len()),
        (nodes, nodes),
        "case {case}: a finished update reports a start and an end date per node, got {}",
        reply.body
    );
    for (start, end) in started.iter().zip(&ended) {
        assert!(
            applied_at - 5 <= *start && start <= end && *end <= now_secs(),
            "case {case}: a node's update ran between the apply ({applied_at}) and now, got \
             {start} .. {end}"
        );
    }

    // Only what the apply named moved: every other action is untouched.
    let case = "the other resources";
    let (_, others) = describe(&emulator, case, &[]).await;
    for action in &others {
        let (name, _, id) = identity(case, action);
        if name == update.name && id == "ua-memcached" {
            continue;
        }
        assert_eq!(
            member(action, case, "UpdateActionStatus"),
            state::UPDATE_ACTION_NOT_APPLIED,
            "case {case}: applying one update to one cluster must move nothing else, got \
             {action:?}"
        );
    }

    // The action status filter now selects it, and the statuses nothing is in
    // select nothing.
    let mut complete = params(&[(
        "UpdateActionStatus.member.1",
        state::UPDATE_ACTION_COMPLETE,
    )]);
    complete.extend(id_filter("CacheClusterIds", &["ua-memcached"]));
    let got = reported_identities(&emulator, "filter on complete", &complete).await;
    assert_eq!(
        got,
        vec![(
            update.name.clone(),
            "CacheClusterId",
            "ua-memcached".to_string()
        )],
        "case filter on complete: the completed action must be the one reported, got {got:?}"
    );
}

#[tokio::test]
async fn pagination_covers_every_action_at_every_page_size() {
    let (emulator, published, resources) = stocked().await;
    let expected = expected_pairs(&published, &resources);
    let total = i32::try_from(expected.len()).expect("a small page");

    // The axis: the page size, from one record per page to more than there are
    // records, plus the request that names no MaxRecords at all.
    let mut rows: Vec<Option<i32>> = (1..=total + 1).map(Some).collect();
    rows.push(None);
    for max_records in rows {
        let case = format!("MaxRecords {max_records:?}");
        let mut walked: Vec<(String, &'static str, String)> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(pages <= 64, "case {case}: pagination did not terminate");
            let mut pairs: Vec<(String, String)> = Vec::new();
            if let Some(size) = max_records {
                pairs.push(("MaxRecords".to_string(), size.to_string()));
            }
            if let Some(token) = &marker {
                pairs.push(("Marker".to_string(), token.clone()));
            }
            let (reply, page) = describe(&emulator, &case, &pairs).await;
            if let Some(size) = max_records {
                assert!(
                    page.len() <= usize::try_from(size).expect("a positive page size"),
                    "case {case}: a page must hold at most {size} actions, got {}",
                    page.len()
                );
            }
            walked.extend(page.iter().map(|action| identity(&case, action)));
            match reply.marker() {
                Some(token) => {
                    assert!(
                        !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit()),
                        "case {case}: the Marker must be an opaque token, got {token:?}"
                    );
                    assert!(
                        !walked.iter().any(|(name, _, _)| token.contains(name.as_str())),
                        "case {case}: the Marker must not spell out a record's name, got {token:?}"
                    );
                    marker = Some(token.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case {case}: walking the pages must report every action, in order"
        );
    }
}

#[tokio::test]
async fn a_value_the_describe_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    let now = now_secs();

    /// One refusal row: the case, the form pairs that make the request, and the
    /// `awsQuery` error code it must be refused with.
    type Row = (&'static str, Vec<(String, String)>, &'static str);
    // The axis: which parameter carries a value the service cannot serve — a
    // page size that is no page, a marker this service never minted, an
    // `Engine` naming no cache engine, and a time range whose two bounds cannot
    // hold together.
    let rows: [Row; 8] = [
        ("MaxRecords 0", params(&[("MaxRecords", "0")]), "InvalidParameterValue"),
        ("MaxRecords -5", params(&[("MaxRecords", "-5")]), "InvalidParameterValue"),
        (
            "marker not minted here",
            params(&[("Marker", "not-a-marker")]),
            "InvalidParameterValue",
        ),
        (
            "marker of an odd length",
            params(&[("Marker", "zzzz")]),
            "InvalidParameterValue",
        ),
        ("Engine mysql", params(&[("Engine", "mysql")]), "InvalidParameterValue"),
        ("Engine empty", params(&[("Engine", "")]), "InvalidParameterValue"),
        (
            "StartTime after EndTime",
            vec![
                (
                    "ServiceUpdateTimeRange.StartTime".to_string(),
                    wire_time(now),
                ),
                (
                    "ServiceUpdateTimeRange.EndTime".to_string(),
                    wire_time(now - 60),
                ),
            ],
            "InvalidParameterCombination",
        ),
        (
            "UpdateActionStatus outside the modelled enum",
            params(&[("UpdateActionStatus.member.1", "finished")]),
            "ValidationError",
        ),
    ];
    for (case, pairs, code) in rows {
        let reply = emulator.call("DescribeUpdateActions", &pairs).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: must be refused with HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: must be refused with {code}, got {:?} {}",
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn an_account_holding_no_resource_has_no_update_action() {
    let emulator = Emulator::new();
    // The catalog is published whether or not the account holds anything, and an
    // update action belongs to a resource — so an account with no cluster and no
    // replication group has none, however many updates are published.
    let published = catalog(&emulator).await;
    assert!(
        !published.is_empty(),
        "the service publishes a catalog even for an empty account"
    );
    let got = reported_identities(&emulator, "empty account", &[]).await;
    assert!(
        got.is_empty(),
        "case empty account: an update action belongs to a resource, got {got:?}"
    );
}
