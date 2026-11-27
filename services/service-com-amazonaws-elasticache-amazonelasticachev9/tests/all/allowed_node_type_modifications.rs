// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAllowedNodeTypeModifications`.
//!
//! Every assertion goes over the wire through the router the production
//! emulator mounts, reusing the harness in
//! [`crate::cache_cluster_lifecycle`]: a cluster is created with
//! `CreateCacheCluster`, and what it may be scaled onto is read back from the
//! persisted record, so a response that is not computed from state cannot pass.
//!
//! The rule under test: the two lists are the node-type catalogue
//! `CacheNodeType` publishes, restricted to the types a new cluster can be
//! created on and that the cluster's own engine version can run, split by
//! whether a type's size sits above (`ScaleUpModifications`) or below
//! (`ScaleDownModifications`) the cluster's current node type on the size
//! ladder the names encode.
//!
//! The inputs are the axes that rule generalises over — the current node type's
//! position on the ladder (floor, middle, ceiling, absent), its
//! generation, and the engine and engine version that decide which families the
//! cluster can run. The catalogue is finite and published, so the invariant
//! test ENUMERATES it rather than sampling it with a generator crate.

use service_com_amazonaws_elasticache_amazonelasticachev9::ops::list_allowed_node_type_modifications as op;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::create_cluster;
use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION};

/// Create one cluster through the harness's own create helper, failing the case
/// if the create itself did not land.
async fn create(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) {
    let reply = create_cluster(emulator, id, extra).await;
    assert_eq!(
        reply.status, 200,
        "case {id}: create must succeed, got {}",
        reply.body
    );
}

/// Ask what the cluster `id` may be scaled onto.
async fn list_for(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "ListAllowedNodeTypeModifications",
            &params(&[("CacheClusterId", id)]),
        )
        .await
}

/// The two reported lists, in document order. `NodeTypeList`'s member carries
/// no `@xmlName`, so each element is the Smithy default `member`.
fn lists(reply: &Reply) -> (Vec<&str>, Vec<&str>) {
    (
        reply.all("ScaleUpModifications/member"),
        reply.all("ScaleDownModifications/member"),
    )
}

/// Assertions that hold of EVERY answer: each list is ordered by name without
/// duplicates (so two reads of an unchanged cluster agree), and no node type is
/// both a scale-up and a scale-down target.
fn assert_well_formed(case: &str, up: &[&str], down: &[&str]) {
    for (direction, list) in [("ScaleUpModifications", up), ("ScaleDownModifications", down)] {
        let mut ordered = list.to_vec();
        ordered.sort_unstable();
        ordered.dedup();
        assert_eq!(
            &ordered[..], list,
            "case {case}: {direction} must be ordered by name without duplicates, got {list:?}"
        );
    }
    for name in up {
        assert!(
            !down.contains(name),
            "case {case}: {name} cannot be both a scale-up and a scale-down target, got up \
             {up:?} and down {down:?}"
        );
    }
}

/// One row of the catalogue-split table.
#[derive(Debug, Clone)]
struct Case {
    /// The cluster this row creates, and the case name.
    id: &'static str,
    /// What it is created with beside `CacheClusterId`.
    extra: &'static [(&'static str, &'static str)],
    /// Node types `ScaleUpModifications` must carry.
    up: &'static [&'static str],
    /// Node types `ScaleDownModifications` must carry.
    down: &'static [&'static str],
    /// Node types NEITHER list may carry.
    absent: &'static [&'static str],
    up_is_empty: bool,
    down_is_empty: bool,
}

#[tokio::test]
async fn the_lists_are_the_catalogue_split_around_the_stored_node_type() {
    // The axis: where the cluster's own node type sits on the ladder, which
    // generation it belongs to, and which engine version it runs — the engine
    // version is what decides whether the catalogue's gated families (M6g, R6g,
    // T4g: "available only for Redis OSS engine version 5.0.6 onward and for
    // Memcached engine version 1.5.16 onward") are offered at all. Every row
    // names a node type, because a create that provisions its own nodes has to;
    // the unplaceable branch of the split is asserted on the function the
    // handler calls, below.
    let cases = [
        Case {
            // `micro` is the bottom of the ladder, so there is nothing below it.
            id: "burstable-floor",
            extra: &[("CacheNodeType", "cache.t3.micro")],
            up: &[
                "cache.t3.small",
                // Redis OSS 7.1 is the default engine version, past the gate.
                "cache.t4g.small",
                "cache.m7g.large",
                "cache.r5.24xlarge",
            ],
            down: &[],
            absent: &[
                // Its own type, and the same size in other families: a move
                // sideways is not a scale.
                "cache.t3.micro",
                "cache.t2.micro",
                "cache.t4g.micro",
                // Previous generation: "creation of new clusters is not
                // supported for these types".
                "cache.t1.micro",
                "cache.m3.medium",
                "cache.c1.xlarge",
            ],
            up_is_empty: false,
            down_is_empty: true,
        },
        Case {
            // Below the gated families' Redis OSS floor of 5.0.6, on the oldest
            // Redis OSS line the catalog still offers.
            id: "redis-before-the-gate",
            extra: &[
                ("Engine", "redis"),
                ("EngineVersion", "4.0.10"),
                ("CacheNodeType", "cache.m5.xlarge"),
            ],
            up: &["cache.m5.2xlarge", "cache.r4.16xlarge"],
            down: &["cache.m5.large", "cache.t3.micro", "cache.t2.medium"],
            absent: &[
                "cache.m6g.large",
                "cache.r6g.2xlarge",
                "cache.t4g.micro",
                "cache.m5.xlarge",
                "cache.r5.xlarge",
            ],
            up_is_empty: false,
            down_is_empty: false,
        },
        Case {
            // Exactly the Memcached floor the gate names.
            id: "memcached-at-the-gate",
            extra: &[
                ("Engine", "memcached"),
                ("EngineVersion", "1.5.16"),
                ("CacheNodeType", "cache.r7g.16xlarge"),
            ],
            up: &["cache.m5.24xlarge", "cache.r5.24xlarge"],
            down: &[
                "cache.m6g.12xlarge",
                "cache.t4g.medium",
                "cache.m4.10xlarge",
            ],
            absent: &["cache.r7g.16xlarge", "cache.r4.16xlarge"],
            up_is_empty: false,
            down_is_empty: false,
        },
        Case {
            // The newest version the catalogue publishes BELOW the gate.
            id: "memcached-before-the-gate",
            extra: &[
                ("Engine", "memcached"),
                ("EngineVersion", "1.5.10"),
                ("CacheNodeType", "cache.t3.small"),
            ],
            up: &["cache.t3.medium", "cache.m7g.large"],
            down: &["cache.t3.micro"],
            absent: &[
                "cache.t4g.medium",
                "cache.m6g.large",
                "cache.r6g.large",
                "cache.t2.small",
            ],
            up_is_empty: false,
            down_is_empty: false,
        },
        Case {
            // The ceiling of the ladder: `24xlarge` is the largest size the
            // catalogue names, so nothing is above it. Valkey postdates the
            // gated families' Redis OSS floor, so they are offered.
            id: "valkey-ceiling",
            extra: &[("Engine", "valkey"), ("CacheNodeType", "cache.m5.24xlarge")],
            up: &[],
            down: &[
                "cache.r6g.16xlarge",
                "cache.t3.micro",
                "cache.m4.10xlarge",
            ],
            absent: &["cache.r5.24xlarge"],
            up_is_empty: true,
            down_is_empty: false,
        },
        Case {
            // A cluster RUNNING a previous-generation type still scales, onto
            // the current-generation types above and below its size.
            id: "previous-generation-cluster",
            extra: &[("CacheNodeType", "cache.m3.2xlarge")],
            up: &["cache.m5.4xlarge", "cache.r7g.4xlarge"],
            down: &["cache.m5.xlarge", "cache.t3.medium"],
            absent: &[
                "cache.m3.large",
                "cache.m3.xlarge",
                "cache.m5.2xlarge",
                "cache.r5.2xlarge",
            ],
            up_is_empty: false,
            down_is_empty: false,
        },
        Case {
            // A node type name is read case-insensitively: the catalogue spells
            // every one of them in lowercase, and a stored node type is
            // whatever the create supplied.
            id: "shouted-node-type",
            extra: &[("CacheNodeType", "CACHE.T3.MICRO")],
            up: &["cache.t3.small", "cache.m7g.large"],
            down: &[],
            absent: &["cache.t3.micro", "cache.t2.micro", "cache.t1.micro"],
            up_is_empty: false,
            down_is_empty: true,
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        create(&emulator, case.id, case.extra).await;
        let reply = list_for(&emulator, case.id).await;
        assert_eq!(
            reply.status, 200,
            "case {}: the list must be served, got {}",
            case.id, reply.body
        );
        let (up, down) = lists(&reply);
        assert_well_formed(case.id, &up, &down);
        assert_eq!(
            up.is_empty(),
            case.up_is_empty,
            "case {}: ScaleUpModifications emptiness, got {up:?}",
            case.id
        );
        assert_eq!(
            down.is_empty(),
            case.down_is_empty,
            "case {}: ScaleDownModifications emptiness, got {down:?}",
            case.id
        );
        for name in case.up {
            assert!(
                up.contains(name),
                "case {}: ScaleUpModifications must offer {name}, got {up:?}",
                case.id
            );
        }
        for name in case.down {
            assert!(
                down.contains(name),
                "case {}: ScaleDownModifications must offer {name}, got {down:?}",
                case.id
            );
        }
        for name in case.absent {
            assert!(
                !up.contains(name) && !down.contains(name),
                "case {}: {name} must be offered in neither direction, got up {up:?} and down \
                 {down:?}",
                case.id
            );
        }
    }

    // `CacheClusterId` "isn't case sensitive": the same cluster answers the
    // same way whatever case it is asked for in.
    let as_created = list_for(&emulator, "burstable-floor").await;
    let shouted = list_for(&emulator, "BURSTABLE-FLOOR").await;
    assert_eq!(
        lists(&shouted),
        lists(&as_created),
        "case id case-insensitivity: the same cluster must answer the same lists, got {} and {}",
        shouted.body,
        as_created.body
    );
}

#[test]
fn a_node_type_outside_the_catalogue_has_no_position_to_scale_from() {
    // A node type the catalogue does not name cannot reach the store any more —
    // every operation that persists one refuses it — so the split's own
    // unplaceable branch is asserted on the function the handler calls. The axis
    // is the FORM of the unplaceable name: a size no family names, a size
    // multiplier past the end of the rank range, a name with no size segment at
    // all, and one with too many.
    for node_type in [
        "cache.m5.enormous",
        "cache.m5.4294967295xlarge",
        "cache.m5",
        "cache.m5.large.extra",
        "m5.large",
    ] {
        let allowed = op::allowed_node_type_modifications("redis", "7.1", Some(node_type));
        assert_eq!(
            allowed,
            op::AllowedNodeTypeModifications::default(),
            "case {node_type}: an unplaceable node type must be scaled from in neither direction, \
             got {allowed:?}"
        );
    }
}

/// Every node type in the catalogue, with the family flags it carries.
fn catalogue() -> Vec<(String, &'static op::NodeTypeFamily)> {
    op::NODE_TYPE_FAMILIES
        .iter()
        .flat_map(|family| {
            family
                .sizes
                .iter()
                .map(move |size| (format!("{}.{size}", family.prefix), family))
        })
        .collect()
}

#[tokio::test]
async fn the_catalogue_carries_what_the_doc_names_and_what_the_offering_catalog_prices() {
    // Literals, so the catalogue table itself is under test rather than only
    // its consistency with the handler. Every one is from `CacheNodeType`'s own
    // list (`smithy_query(query="doc", shape="CreateCacheClusterMessage",
    // member="CacheNodeType")`).
    let names: Vec<String> = catalogue().into_iter().map(|(name, _)| name).collect();
    for named in [
        "cache.m7g.16xlarge",
        "cache.m6g.large",
        "cache.m5.24xlarge",
        "cache.m4.10xlarge",
        "cache.t4g.medium",
        "cache.t2.micro",
        "cache.t1.micro",
        "cache.m1.small",
        "cache.m3.2xlarge",
        "cache.c1.xlarge",
        "cache.r7g.12xlarge",
        "cache.r6g.8xlarge",
        "cache.r5.12xlarge",
        "cache.r4.16xlarge",
        "cache.m2.4xlarge",
        "cache.r3.8xlarge",
    ] {
        assert!(
            names.iter().any(|name| name == named),
            "case {named}: the catalogue must carry the node type the doc names, got {names:?}"
        );
    }
    // A name the doc does NOT list is not in the catalogue either.
    for absent in ["cache.m5.8xlarge", "cache.r5.16xlarge", "cache.t3.large"] {
        assert!(
            !names.iter().any(|name| name == absent),
            "case {absent}: the doc names no such node type, got {names:?}"
        );
    }
    // `24xlarge` is the top of the documented ladder and `micro` the bottom.
    let ceiling = op::node_type_size_rank("cache.m5.24xlarge").expect("a documented node type");
    let floor = op::node_type_size_rank("cache.t3.micro").expect("a documented node type");
    for (name, _) in catalogue() {
        let rank = op::node_type_size_rank(&name).expect("a documented node type");
        assert!(
            (floor..=ceiling).contains(&rank),
            "case {name}: rank {rank} must sit between the documented floor {floor} and ceiling \
             {ceiling}"
        );
    }
    // The two published node-type lists in this crate name the same node types:
    // every type the offering catalog prices is one this catalogue carries.
    for (priced, _) in state::OFFERING_NODE_TYPES {
        assert!(
            names.iter().any(|name| name == priced),
            "case {priced}: a priced offering node type must be in the node type catalogue, got \
             {names:?}"
        );
    }
}

#[tokio::test]
async fn the_split_is_complete_and_sound_over_the_whole_catalogue() {
    // The catalogue is finite and published, so this enumerates it: every
    // family's smallest and largest size becomes a cluster's current node type,
    // paired round-robin with one of the engine versions that straddle the
    // gated families' floors. For each, the two lists must be EXACTLY the
    // available current-generation types whose size differs from the cluster's,
    // split by direction.
    let engines = [
        ("redis", "7.1"),
        ("redis", "4.0.10"),
        ("memcached", "1.6.22"),
        ("memcached", "1.5.10"),
        ("valkey", "8.0"),
    ];

    let emulator = Emulator::new();
    // Built once: every row reads the same published catalogue.
    let catalogue = catalogue();
    let mut sampled: Vec<(String, &str, &str)> = Vec::new();
    for (index, family) in op::NODE_TYPE_FAMILIES.iter().enumerate() {
        let smallest = family.sizes.first().expect("a family names its sizes");
        let largest = family.sizes.last().expect("a family names its sizes");
        for size in [smallest, largest] {
            let (engine, engine_version) = engines[sampled.len() % engines.len()];
            let node_type = format!("{}.{size}", family.prefix);
            let id = format!("scale-{index}-{}", sampled.len());
            create(
                &emulator,
                &id,
                &[
                    ("Engine", engine),
                    ("EngineVersion", engine_version),
                    ("CacheNodeType", &node_type),
                ],
            )
            .await;
            let case = format!("{node_type} on {engine} {engine_version}");
            let reply = list_for(&emulator, &id).await;
            sampled.push((id, engine, engine_version));
            assert_eq!(
                reply.status, 200,
                "case {case}: the list must be served, got {}",
                reply.body
            );
            let (up, down) = lists(&reply);
            assert_well_formed(&case, &up, &down);

            let current_rank = op::node_type_size_rank(&node_type)
                .expect("every catalogue name is placeable on the ladder");
            // Sound: nothing is offered that the cluster could not be created
            // on — no previous-generation type, and no type this engine version
            // cannot run — and the direction matches the ladder.
            for name in up.iter().chain(down.iter()) {
                let rank = op::node_type_size_rank(name)
                    .unwrap_or_else(|| panic!("case {case}: {name} is not a node type name"));
                let family = catalogue
                    .iter()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, family)| *family)
                    .unwrap_or_else(|| panic!("case {case}: {name} is not in the catalogue"));
                assert!(
                    family.current_generation,
                    "case {case}: {name} is a previous-generation type and cannot be created, got \
                     up {up:?} and down {down:?}"
                );
                assert!(
                    op::family_runs_engine(family, engine, engine_version),
                    "case {case}: {name} cannot be run by this engine version, got up {up:?} and \
                     down {down:?}"
                );
                let direction_matches = if up.contains(name) {
                    rank > current_rank
                } else {
                    rank < current_rank
                };
                assert!(
                    direction_matches,
                    "case {case}: {name} (rank {rank}) is on the wrong side of {node_type} (rank \
                     {current_rank}), got up {up:?} and down {down:?}"
                );
            }

            // Complete: every available current-generation type of a different
            // size IS offered, in the direction its size puts it.
            let mut expected = 0usize;
            for (name, family) in &catalogue {
                if !family.current_generation
                    || !op::family_runs_engine(family, engine, engine_version)
                {
                    continue;
                }
                let rank = op::node_type_size_rank(name)
                    .expect("every catalogue name is placeable on the ladder");
                if rank == current_rank {
                    continue;
                }
                expected += 1;
                let offered = if rank > current_rank { &up } else { &down };
                assert!(
                    offered.contains(&name.as_str()),
                    "case {case}: {name} (rank {rank}) must be offered against {node_type} (rank \
                     {current_rank}), got up {up:?} and down {down:?}"
                );
            }
            assert_eq!(
                up.len() + down.len(),
                expected,
                "case {case}: exactly the available current-generation types of another size must \
                 be offered, got up {up:?} and down {down:?}"
            );
        }
    }

    // A read changes nothing a later caller sees: asking again answers the same
    // lists, and every cluster is still reported with the node type it was
    // created with.
    for (id, _, _) in &sampled {
        let first = list_for(&emulator, id).await;
        let second = list_for(&emulator, id).await;
        assert_eq!(
            lists(&second),
            lists(&first),
            "case {id}: a second read must answer the same lists, got {} and {}",
            second.body,
            first.body
        );
    }
}

#[tokio::test]
async fn the_lists_follow_the_node_type_the_cluster_is_actually_running() {
    let emulator = Emulator::new();
    create(
        &emulator,
        "modify-target",
        &[("CacheNodeType", "cache.t3.micro")],
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);

    let before = list_for(&emulator, "modify-target").await;
    let (up_before, down_before) = lists(&before);
    assert!(
        up_before.contains(&"cache.t3.small") && down_before.is_empty(),
        "the created cluster must be scaled from cache.t3.micro, got up {up_before:?} and down \
         {down_before:?}"
    );

    // A deferred modify leaves the new node type PENDING, so the cluster is
    // still running the old one and the lists must not move yet.
    let deferred = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "modify-target"),
                ("CacheNodeType", "cache.m5.4xlarge"),
            ]),
        )
        .await;
    assert_eq!(
        deferred.status, 200,
        "the deferred modify must be accepted, got {}",
        deferred.body
    );
    let pending = list_for(&emulator, "modify-target").await;
    assert_eq!(
        lists(&pending),
        lists(&before),
        "a node type left pending must not move the lists, got {}",
        pending.body
    );

    // Applied immediately, the stored node type changes and the lists follow
    // it: what was above the old type is now below the new one.
    let applied = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "modify-target"),
                ("CacheNodeType", "cache.m5.4xlarge"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        applied.status, 200,
        "the immediate modify must be accepted, got {}",
        applied.body
    );
    let after = list_for(&emulator, "modify-target").await;
    let (up_after, down_after) = lists(&after);
    assert_well_formed("after an applied modify", &up_after, &down_after);
    assert!(
        down_after.contains(&"cache.t3.micro") && down_after.contains(&"cache.m5.xlarge"),
        "the smaller types must now be scale-down targets, got {down_after:?}"
    );
    assert!(
        up_after.contains(&"cache.m5.12xlarge") && !up_after.contains(&"cache.t3.small"),
        "only the larger types may be scale-up targets, got {up_after:?}"
    );
}

/// One replication-group row: the group id, the form pairs it is created with,
/// and the node types `ScaleUpModifications`, `ScaleDownModifications` and
/// neither list must carry.
type GroupCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

#[tokio::test]
async fn a_replication_group_is_scaled_from_its_own_stored_node_type() {
    // The other half of this operation's input surface: a group is reported on
    // exactly like a cluster, from the engine, engine version and node type
    // `CreateReplicationGroup` persisted — so the same rule has to hold over
    // the same axes, read through a different resource.
    let emulator = Emulator::new();

    // Each row: the group id, what it is created with, and names each list must
    // and must not carry.
    let rows: &[GroupCase] = &[
        (
            "group-at-the-floor",
            &[("CacheNodeType", "cache.t3.micro")],
            // up
            &["cache.t3.small", "cache.m7g.large", "cache.t4g.medium"],
            // down (nothing is below `micro`)
            &[],
            // neither
            &["cache.t3.micro", "cache.t2.micro", "cache.m3.medium"],
        ),
        (
            "group-before-the-gate",
            &[
                ("Engine", "redis"),
                ("EngineVersion", "4.0.10"),
                ("CacheNodeType", "cache.m5.2xlarge"),
            ],
            &["cache.m5.4xlarge", "cache.r7g.8xlarge"],
            &["cache.m5.large", "cache.t3.micro"],
            &["cache.m6g.4xlarge", "cache.r6g.4xlarge", "cache.r5.2xlarge"],
        ),
        (
            "group-at-the-ceiling",
            &[
                ("Engine", "valkey"),
                ("CacheNodeType", "cache.r5.24xlarge"),
            ],
            &[],
            &["cache.r6g.16xlarge", "cache.t3.micro"],
            &["cache.m5.24xlarge"],
        ),
    ];

    for (id, extra, up_must, down_must, absent) in rows {
        let mut pairs = params(&[
            ("ReplicationGroupId", id),
            ("ReplicationGroupDescription", "an emulated group"),
        ]);
        pairs.extend(params(extra));
        let created = emulator.call("CreateReplicationGroup", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {id}: the group must be created, got {}",
            created.body
        );

        let reply = emulator
            .call(
                "ListAllowedNodeTypeModifications",
                &params(&[("ReplicationGroupId", id)]),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {id}: the group's list must be served, got {}",
            reply.body
        );
        let (up, down) = lists(&reply);
        assert_well_formed(id, &up, &down);
        assert_eq!(
            down.is_empty(),
            down_must.is_empty(),
            "case {id}: ScaleDownModifications emptiness, got {down:?}"
        );
        assert_eq!(
            up.is_empty(),
            up_must.is_empty(),
            "case {id}: ScaleUpModifications emptiness, got {up:?}"
        );
        for name in *up_must {
            assert!(
                up.contains(name),
                "case {id}: ScaleUpModifications must offer {name}, got {up:?}"
            );
        }
        for name in *down_must {
            assert!(
                down.contains(name),
                "case {id}: ScaleDownModifications must offer {name}, got {down:?}"
            );
        }
        for name in *absent {
            assert!(
                !up.contains(name) && !down.contains(name),
                "case {id}: {name} must be offered in neither direction, got up {up:?} and down \
                 {down:?}"
            );
        }

        // The group's id is matched the way the store keys it, so asking in
        // another case answers the same lists.
        let shouted = emulator
            .call(
                "ListAllowedNodeTypeModifications",
                &params(&[("ReplicationGroupId", &id.to_uppercase())]),
            )
            .await;
        assert_eq!(
            lists(&shouted),
            lists(&reply),
            "case {id}: the same group must answer the same lists whatever case it is asked for \
             in, got {}",
            shouted.body
        );
    }

    // A group of the SAME name as nothing in the store is the modelled
    // not-found — the refusal is about what the store holds, not about the
    // member being present.
    let absent = emulator
        .call(
            "ListAllowedNodeTypeModifications",
            &params(&[("ReplicationGroupId", "no-such-group")]),
        )
        .await;
    assert_eq!(
        absent.status, 404,
        "an absent group must be refused 404, got {} with {}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.error_code(),
        "ReplicationGroupNotFoundFault",
        "an absent group must carry the modelled error code, got {}",
        absent.body
    );
}

#[tokio::test]
async fn a_request_that_names_no_cluster_this_emulator_holds_is_refused() {
    let emulator = Emulator::new();
    create(
        &emulator,
        "present-cluster",
        &[("CacheNodeType", "cache.t3.micro")],
    )
    .await;

    // Each row: the form pairs, and the HTTP status plus `awsQuery` error code
    // the operation's modelled errors carry
    // (`smithy_query(query="operation", operation="ListAllowedNodeTypeModifications")`).
    let rows: &[RefusalCase] = &[
        // "You must provide a value for either the CacheClusterId or the
        // ReplicationGroupId."
        ("neither identifier", &[], 400, "InvalidParameterCombination"),
        (
            "both identifiers",
            &[
                ("CacheClusterId", "present-cluster"),
                ("ReplicationGroupId", "present-cluster"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "unknown cluster",
            &[("CacheClusterId", "absent-cluster")],
            404,
            "CacheClusterNotFound",
        ),
        // A replication group this emulator does not hold — the one the
        // refusal is really about; a group it DOES hold is answered with its
        // lists by `a_replication_group_is_scaled_from_its_own_stored_node_type`.
        (
            "absent replication group",
            &[("ReplicationGroupId", "any-group")],
            404,
            "ReplicationGroupNotFoundFault",
        ),
    ];

    for (case, pairs, status, code) in rows {
        let reply = emulator
            .call("ListAllowedNodeTypeModifications", &params(pairs))
            .await;
        assert_eq!(
            reply.status, *status,
            "case {case}: expected HTTP {status}, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case}: expected error code {code}, got {}",
            reply.body
        );
        let (up, down) = lists(&reply);
        assert!(
            up.is_empty() && down.is_empty(),
            "case {case}: a refusal must carry no node types, got {}",
            reply.body
        );
    }
}
