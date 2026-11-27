// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the INVENTORY the ElastiCache reference catalogs
//! publish, and for which of the two parameter lists a page bounds.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded answer:
//!
//! * every cache parameter group family this service publishes is the family of
//!   at least one published engine version AND has its default parameter group
//!   — plus the cluster-mode-on variant when it can run cluster mode — so a
//!   client that enumerates the generations from either catalog reaches the
//!   same set, the current engine generation included;
//! * the engine version a create may name is the version the catalog OFFERS it
//!   by, and ElastiCache names a Redis OSS 6 or later version and every Valkey
//!   version by its line, so a create on a published line joins that line's
//!   family default and one naming a version the catalog does not publish is
//!   refused in the service's own words;
//! * the reserved-node offering catalog stocks one offering per reservation
//!   term and payment option for a node type and engine, and each offering's
//!   price is internally consistent: whatever is not paid up front is the
//!   hourly recurring charge;
//! * the service-update catalog publishes both kinds of update ElastiCache
//!   releases — a security update per engine and the end-of-life update of an
//!   engine major version, one per kind of resource that runs it — and a name
//!   read off it can be applied to a resource the account holds;
//! * `MaxRecords` pages the `Parameters` list of both parameter describes (the
//!   operations' paginated items), while the cache-node-type-specific list is
//!   not paged: it comes whole with every page, on a family's engine defaults
//!   and on a group that has just been created and modified.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts (the shared [`crate::harness`]), so only the HTTP status and
//! the XML body are inspected.

use std::collections::BTreeSet;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::create_available_cluster;
use crate::harness::{params, Emulator, Reply};

/// One engine-generation row: the engine, a version the catalog offers it by,
/// and the parameter group family that version belongs to.
///
/// Source: the engine lines `DescribeCacheEngineVersions` publishes
/// (`state::CACHE_ENGINE_VERSIONS`) and the family of each line
/// (`state::parameter_group_family`). The rows span both naming conventions —
/// a Memcached or early Redis OSS version is named by its patch release, a
/// Redis OSS 6 or later version and every Valkey version by its line — and the
/// last row is a bare MAJOR, which names the whole line.
const OFFERED_VERSIONS: [(&str, &str, &str); 10] = [
    ("memcached", "1.4.5", "memcached1.4"),
    ("memcached", "1.5.16", "memcached1.5"),
    ("memcached", "1.6.22", "memcached1.6"),
    ("redis", "5.0.6", "redis5.0"),
    ("redis", "6.0", "redis6.x"),
    ("redis", "6.2", "redis6.x"),
    ("redis", "7.0", "redis7"),
    ("valkey", "7.2", "valkey7"),
    ("valkey", "9.0", "valkey9"),
    ("redis", "7", "redis7"),
];

/// One row of versions the catalog does NOT offer: the engine and the version.
///
/// The patch spellings of the Redis OSS lines above are the interesting rows:
/// the catalog offers `6.0`, `6.2` and `7.0`, so a request naming a patch
/// release behind one of them names a version the service does not publish.
/// Beside them sit a Memcached patch release below the oldest line, a retired
/// Redis OSS release and a Valkey generation past the newest.
const UNOFFERED_VERSIONS: [(&str, &str); 5] = [
    ("redis", "6.0.5"),
    ("redis", "6.2.6"),
    ("redis", "2.8.24"),
    ("memcached", "1.4.4"),
    ("valkey", "10.0"),
];

/// Ask one describe and assert it succeeded.
async fn describe(emulator: &Emulator, case: &str, action: &str, extra: &[(&str, &str)]) -> Reply {
    let reply = emulator.call(action, &params(extra)).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: {action} must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

#[tokio::test]
async fn every_published_family_is_offered_a_version_and_a_default_group() {
    let emulator = Emulator::new();
    let versions = describe(
        &emulator,
        "families",
        "DescribeCacheEngineVersions",
        &[("MaxRecords", "100")],
    )
    .await;
    let groups = describe(
        &emulator,
        "families",
        "DescribeCacheParameterGroups",
        &[("MaxRecords", "100")],
    )
    .await;
    let version_families: BTreeSet<&str> = versions
        .all("CacheEngineVersion/CacheParameterGroupFamily")
        .into_iter()
        .collect();
    let group_names: Vec<&str> = groups.all("CacheParameterGroup/CacheParameterGroupName");
    let group_families: Vec<&str> = groups.all("CacheParameterGroup/CacheParameterGroupFamily");
    let descriptions: Vec<&str> = groups.all("CacheParameterGroup/Description");
    let arns: Vec<&str> = groups.all("CacheParameterGroup/ARN");
    assert_eq!(
        group_names.len(),
        group_families.len(),
        "case families: every group reports its family, got {}",
        groups.body
    );

    // Every family the VERSION catalog names is one this service publishes, so
    // the published families are a superset of them: a family outlives the
    // engine versions it was published for, which is why the retired Redis OSS
    // lines are published here with their default groups while the version
    // catalog offers no version of them.
    for family in &version_families {
        assert!(
            state::PARAMETER_GROUP_FAMILIES.contains(family),
            "case {family}: a version's family must be one this service publishes, got {:?}",
            state::PARAMETER_GROUP_FAMILIES
        );
    }

    // The axis: the family — every one this service publishes, the current
    // engine generation included. A family OUTLIVES the engine versions it was
    // published for: the retired Redis OSS lines still carry their default
    // parameter groups (asserted for every family below), so only a family the
    // catalog still offers a version of is checked against the version catalog.
    for family in state::PARAMETER_GROUP_FAMILIES {
        if let Some(version) = state::CACHE_ENGINE_VERSIONS.iter().find(|row| {
            state::parameter_group_family(row.engine, row.engine_version) == Some(family)
        }) {
            assert!(
                version_families.contains(family),
                "case {family}: the family of the published version {} {} must be reported by \
                 the version catalog, got {version_families:?}",
                version.engine,
                version.engine_version
            );
        }
        let plain = format!("default.{family}");
        let mut expected = vec![(
            plain.clone(),
            format!("Default parameter group for {family}"),
        )];
        if state::family_publishes_cluster_mode(family) {
            expected.push((
                format!("{plain}{}", state::CLUSTER_MODE_ON_SUFFIX),
                format!("Customized default parameter group for {family} with cluster mode on"),
            ));
        }
        for (name, description) in expected {
            let position = group_names.iter().position(|reported| *reported == name);
            let position = position.unwrap_or_else(|| {
                panic!("case {family}: {name} must be published, got {group_names:?}")
            });
            assert_eq!(
                group_families[position], family,
                "case {family}: {name} must belong to {family}, got {:?}",
                group_families[position]
            );
            assert_eq!(
                descriptions[position], description,
                "case {family}: expected the description {description:?} for {name}, got {:?}",
                descriptions[position]
            );
            assert_eq!(
                arns[position],
                format!(
                    "arn:aws:elasticache:{}:{}:parametergroup:{name}",
                    crate::harness::REGION,
                    engine::emulator_account_id()
                ),
                "case {family}: unexpected ARN for {name}, got {:?}",
                arns[position]
            );
            // The group the catalog publishes is a group the per-group describe
            // serves: the two read one copy of the family's defaults.
            let parameters = describe(
                &emulator,
                &name,
                "DescribeCacheParameters",
                &[("CacheParameterGroupName", &name)],
            )
            .await;
            assert!(
                !parameters.all("Parameter/ParameterName").is_empty(),
                "case {family}: {name} must report its family's parameters, got {}",
                parameters.body
            );
        }
    }
}

#[tokio::test]
async fn a_create_may_name_the_versions_the_catalog_offers_and_no_others() {
    let emulator = Emulator::new();

    for (index, (cache_engine, version, family)) in OFFERED_VERSIONS.into_iter().enumerate() {
        let case = format!("{cache_engine} {version}");
        // The catalog itself offers it: the filter selects at least one entry,
        // and every entry it selects belongs to the line's family.
        let reported = describe(
            &emulator,
            &case,
            "DescribeCacheEngineVersions",
            &[("Engine", cache_engine), ("EngineVersion", version)],
        )
        .await;
        let selected = reported.all("CacheEngineVersion/EngineVersion");
        assert!(
            !selected.is_empty(),
            "case {case}: the catalog must offer this version, got {}",
            reported.body
        );
        for reported_family in reported.all("CacheEngineVersion/CacheParameterGroupFamily") {
            assert_eq!(
                reported_family, family,
                "case {case}: the version must be bound to {family}, got {reported_family:?}"
            );
        }

        // And a create naming it is served, joining the family's default group.
        let id = format!("offered-{index}");
        let created = emulator
            .call(
                "CreateCacheCluster",
                &crate::harness::cluster_create_params(
                    &id,
                    &[("Engine", cache_engine), ("EngineVersion", version)],
                ),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: a create on an offered version must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("CacheParameterGroup/CacheParameterGroupName"),
            Some(format!("default.{family}").as_str()),
            "case {case}: the cluster must join the family's default group, got {}",
            created.body
        );
        assert_eq!(
            created.text("CacheCluster/EngineVersion"),
            Some(state::patch_release(cache_engine, version)),
            "case {case}: the cluster must run the release behind the version the request \
             named, got {}",
            created.body
        );
    }

    for (index, (cache_engine, version)) in UNOFFERED_VERSIONS.into_iter().enumerate() {
        let case = format!("{cache_engine} {version}");
        let empty = describe(
            &emulator,
            &case,
            "DescribeCacheEngineVersions",
            &[("Engine", cache_engine), ("EngineVersion", version)],
        )
        .await;
        assert!(
            empty.all("CacheEngineVersion/EngineVersion").is_empty(),
            "case {case}: a version the catalog does not publish selects nothing, got {}",
            empty.body
        );
        let refused = emulator
            .call(
                "CreateCacheCluster",
                &crate::harness::cluster_create_params(
                    &format!("unoffered-{index}"),
                    &[("Engine", cache_engine), ("EngineVersion", version)],
                ),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: a create on a version nothing offers must be refused, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_message(),
            state::version_not_offered_message(cache_engine, version),
            "case {case}: unexpected refusal wording, got {:?}",
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn the_offering_catalog_stocks_every_term_and_option_at_a_consistent_price() {
    let emulator = Emulator::new();

    // The axis: the engine a reservation is bought for and the node type it is
    // on — a current-generation memory-optimized type, a burstable one, a
    // previous-generation one and a large general-purpose one.
    for (product, node_type) in [
        ("redis", "cache.r6g.large"),
        ("redis", "cache.t4g.micro"),
        ("memcached", "cache.m3.medium"),
        ("valkey", "cache.m5.2xlarge"),
    ] {
        let case = format!("{product} on {node_type}");
        let reply = describe(
            &emulator,
            &case,
            "DescribeReservedCacheNodesOfferings",
            &[
                ("ProductDescription", product),
                ("CacheNodeType", node_type),
                ("MaxRecords", "100"),
            ],
        )
        .await;
        let durations = reply.all("ReservedCacheNodesOffering/Duration");
        let options = reply.all("ReservedCacheNodesOffering/OfferingType");
        let fixed_prices = reply.all("ReservedCacheNodesOffering/FixedPrice");
        let usage_prices = reply.all("ReservedCacheNodesOffering/UsagePrice");
        let amounts = reply.all("RecurringCharge/RecurringChargeAmount");
        let frequencies = reply.all("RecurringCharge/RecurringChargeFrequency");
        for (member, column) in [
            ("OfferingType", options.len()),
            ("FixedPrice", fixed_prices.len()),
            ("UsagePrice", usage_prices.len()),
            ("RecurringChargeAmount", amounts.len()),
            ("RecurringChargeFrequency", frequencies.len()),
        ] {
            assert_eq!(
                column,
                durations.len(),
                "case {case}: {member} must be reported once per offering ({}), got {column}: {}",
                durations.len(),
                reply.body
            );
        }

        // One offering per (term, payment option): the catalog stocks the whole
        // grid for a node type, so a client pricing a reservation finds every
        // combination it may buy.
        let stocked: BTreeSet<(&str, &str)> = durations
            .iter()
            .copied()
            .zip(options.iter().copied())
            .collect();
        let expected: BTreeSet<(String, String)> = state::OFFERING_TERMS
            .iter()
            .flat_map(|(_, seconds)| {
                state::STOCKED_OFFERING_TYPES
                    .iter()
                    .map(move |option| (seconds.to_string(), (*option).to_string()))
            })
            .collect();
        let stocked_owned: BTreeSet<(String, String)> = stocked
            .iter()
            .map(|(duration, option)| ((*duration).to_string(), (*option).to_string()))
            .collect();
        assert_eq!(
            stocked_owned, expected,
            "case {case}: expected one offering per term and payment option, got {stocked:?}"
        );
        assert_eq!(
            durations.len(),
            expected.len(),
            "case {case}: no combination may be published twice, got {:?}",
            durations.iter().zip(options.iter()).collect::<Vec<_>>()
        );

        // The pricing shape: whatever a payment option does not take up front
        // is its hourly recurring charge, and an hourly charge is what
        // `RecurringCharges` reports.
        for index in 0..durations.len() {
            let option = options[index];
            let row = format!("{case} / {} {option}", durations[index]);
            let number = |member: &str, text: &str| -> f64 {
                text.parse().unwrap_or_else(|_| {
                    panic!("case {row}: {member} must be a number, got {text:?}")
                })
            };
            let fixed = number("FixedPrice", fixed_prices[index]);
            let recurring = number("RecurringChargeAmount", amounts[index]);
            assert_eq!(
                number("UsagePrice", usage_prices[index]),
                0.0,
                "case {row}: a reservation's hourly charge is its recurring charge, so its \
                 UsagePrice is 0, got {:?}",
                usage_prices[index]
            );
            assert_eq!(
                frequencies[index],
                state::RECURRING_CHARGE_HOURLY,
                "case {row}: expected an hourly recurring charge, got {:?}",
                frequencies[index]
            );
            match option {
                state::OFFERING_TYPE_ALL_UPFRONT => {
                    assert!(
                        fixed > 0.0 && recurring == 0.0,
                        "case {row}: the whole term is paid up front, so FixedPrice is above 0 \
                         and the recurring charge is 0, got {fixed} and {recurring}"
                    );
                }
                state::OFFERING_TYPE_NO_UPFRONT => {
                    assert!(
                        fixed == 0.0 && recurring > 0.0,
                        "case {row}: nothing is paid up front, so FixedPrice is 0 and the \
                         recurring charge is above 0, got {fixed} and {recurring}"
                    );
                }
                _ => {
                    assert!(
                        fixed > 0.0 && recurring > 0.0,
                        "case {row}: part of the term is paid up front, so both prices are \
                         above 0, got {fixed} and {recurring}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn the_update_catalog_publishes_both_kinds_of_update_and_one_can_be_applied() {
    let emulator = Emulator::new();
    let reply = describe(
        &emulator,
        "updates",
        "DescribeServiceUpdates",
        &[("MaxRecords", "100")],
    )
    .await;
    let names: Vec<&str> = reply.all("ServiceUpdate/ServiceUpdateName");
    let engines: Vec<&str> = reply.all("ServiceUpdate/Engine");
    let versions: Vec<&str> = reply.all("ServiceUpdate/EngineVersion");
    let types: Vec<&str> = reply.all("ServiceUpdate/ServiceUpdateType");
    let severities: Vec<&str> = reply.all("ServiceUpdate/ServiceUpdateSeverity");
    let statuses: Vec<&str> = reply.all("ServiceUpdate/ServiceUpdateStatus");
    for (member, column) in [
        ("Engine", engines.len()),
        ("EngineVersion", versions.len()),
        ("ServiceUpdateType", types.len()),
        ("ServiceUpdateSeverity", severities.len()),
        ("ServiceUpdateStatus", statuses.len()),
    ] {
        assert_eq!(
            column,
            names.len(),
            "case updates: {member} must be reported once per update ({}), got {column}: {}",
            names.len(),
            reply.body
        );
    }

    // The axis: the KIND of update. The catalog publishes the security updates
    // of every engine it runs and the end-of-life update of an engine major
    // version, one per kind of resource that runs it.
    for update_type in [
        state::SERVICE_UPDATE_TYPE,
        state::SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION,
    ] {
        assert!(
            types.contains(&update_type),
            "case updates: the catalog must publish a {update_type}, got {types:?}"
        );
    }
    let end_of_life: Vec<(&str, usize)> = names
        .iter()
        .copied()
        .enumerate()
        .filter(|(index, _)| types[*index] == state::SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION)
        .map(|(index, name)| (name, index))
        .collect();
    assert_eq!(
        end_of_life.len(),
        2,
        "case updates: an end of life is published once per kind of resource that runs the \
         version, got {end_of_life:?}"
    );
    for (name, index) in &end_of_life {
        assert!(
            name.starts_with("elasticache-") && name.contains("-eol-"),
            "case {name}: an end-of-life update's name must say so, got {name:?}"
        );
        assert!(
            name.contains(engines[*index]),
            "case {name}: the name must carry the engine it applies to, got {:?}",
            engines[*index]
        );
        let line = versions[*index]
            .strip_prefix(&format!("{} ", engines[*index]))
            .unwrap_or_else(|| {
                panic!(
                    "case {name}: the version must be the engine and the line it applies to, \
                     got {:?}",
                    versions[*index]
                )
            });
        assert!(
            state::parameter_group_family(engines[*index], line).is_some(),
            "case {name}: the line must be one this service publishes a family for, got {line:?}"
        );
        assert_eq!(
            statuses[*index],
            state::SERVICE_UPDATE_AVAILABLE,
            "case {name}: an update inside its window is available, got {:?}",
            statuses[*index]
        );
        assert!(
            ["critical", "important", "medium", "low"].contains(&severities[*index]),
            "case {name}: expected a modelled severity, got {:?}",
            severities[*index]
        );
    }
    for kind in ["cache-clusters", "replication-groups"] {
        assert!(
            end_of_life.iter().any(|(name, _)| name.ends_with(kind)),
            "case updates: the end of life must be published for {kind}, got {end_of_life:?}"
        );
    }

    // A name read off the catalog is a name the apply serves: the end-of-life
    // update of the Redis OSS line applies to a cache cluster running that
    // engine.
    let (eol_name, eol_index) = end_of_life
        .iter()
        .find(|(name, _)| name.ends_with("cache-clusters"))
        .expect("the end of life is published for cache clusters");
    let cluster = "eol-target";
    create_available_cluster(&emulator, cluster, &[("Engine", engines[*eol_index])]).await;
    let applied = emulator
        .call(
            "BatchApplyUpdateAction",
            &params(&[
                ("ServiceUpdateName", eol_name),
                ("CacheClusterIds.member.1", cluster),
            ]),
        )
        .await;
    assert_eq!(
        applied.status, 200,
        "case {eol_name}: applying a published update must answer 200, got {} {}",
        applied.status, applied.body
    );
    assert_eq!(
        applied.text("ProcessedUpdateAction/CacheClusterId"),
        Some(cluster),
        "case {eol_name}: the apply must report the cluster it started on, got {}",
        applied.body
    );
    let actions = describe(
        &emulator,
        eol_name,
        "DescribeUpdateActions",
        &[("ServiceUpdateName", eol_name)],
    )
    .await;
    assert!(
        actions
            .all("UpdateAction/CacheClusterId")
            .contains(&cluster),
        "case {eol_name}: the started action must be reported for {cluster}, got {}",
        actions.body
    );
}

/// The `ParameterName`s of the paged list — the plain `Parameters` — in page
/// order.
fn paged(reply: &Reply) -> Vec<&str> {
    reply.all("Parameters/Parameter/ParameterName")
}

/// The `ParameterName`s of the list `MaxRecords` does not page.
fn unpaged(reply: &Reply) -> Vec<&str> {
    reply.all("CacheNodeTypeSpecificParameters/CacheNodeTypeSpecificParameter/ParameterName")
}

/// Every cache node type a reply reports a node-type-specific value for.
fn node_types(reply: &Reply) -> BTreeSet<&str> {
    reply
        .all("CacheNodeTypeSpecificValue/CacheNodeType")
        .into_iter()
        .collect()
}

/// Assert `reply` reports the whole node-type-specific list of `family`, with a
/// value per published cache node type for each of its parameters.
fn assert_node_type_specific_is_whole(case: &str, family: &str, reply: &Reply) {
    let expected: Vec<String> = state::family_default_parameters(family)
        .unwrap_or_else(|| panic!("case {case}: a published family has a catalog"))
        .values()
        .filter(|parameter| parameter.is_node_type_specific())
        .map(|parameter| parameter.name.clone())
        .collect();
    assert!(
        !expected.is_empty(),
        "case {case}: {family} must publish node-type-specific parameters for this rule to bite"
    );
    assert_eq!(
        unpaged(reply),
        expected,
        "case {case}: the cache-node-type-specific list is not paged, so it comes whole, got {}",
        reply.body
    );
    let published: BTreeSet<&str> = state::OFFERING_NODE_TYPES
        .iter()
        .map(|(node_type, _)| *node_type)
        .collect();
    assert_eq!(
        node_types(reply),
        published,
        "case {case}: every published cache node type must carry a value, got {}",
        reply.body
    );
}

#[tokio::test]
async fn max_records_pages_the_parameters_and_not_the_node_type_specific_list() {
    let emulator = Emulator::new();

    // The axis: the family (and with it the engine catalog and the version
    // line), and the page size — each published bound of "minimum 20; maximum
    // 100" and a size between them, plus the request that names no page size.
    for family in ["redis7", "memcached1.6", "valkey9"] {
        let total = state::family_default_parameters(family)
            .expect("a published family has a catalog")
            .values()
            .filter(|parameter| !parameter.is_node_type_specific())
            .count();
        for page_size in ["20", "37", "100", ""] {
            let case = format!("{family} at {page_size:?}");
            let extra: Vec<(&str, &str)> = if page_size.is_empty() {
                vec![("CacheParameterGroupFamily", family)]
            } else {
                vec![
                    ("CacheParameterGroupFamily", family),
                    ("MaxRecords", page_size),
                ]
            };
            let reply = describe(
                &emulator,
                &case,
                "DescribeEngineDefaultParameters",
                &extra,
            )
            .await;
            let window: usize = page_size.parse().unwrap_or(100);
            assert_eq!(
                paged(&reply).len(),
                total.min(window),
                "case {case}: the window bounds the Parameters list, got {:?}",
                paged(&reply)
            );
            assert_eq!(
                reply.marker().is_some(),
                total > window,
                "case {case}: a marker is present exactly when more parameters remain, got {}",
                reply.body
            );
            assert_node_type_specific_is_whole(&case, family, &reply);
        }
    }

    // The same rule on a group a client has just created, and again after it has
    // modified one of that group's parameters: the read-back carries the
    // family's parameters with the modified one reported as the customer's, and
    // the node-type-specific list is still whole.
    let family = "redis7";
    let group = "inventory-group";
    let created = emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", group),
                ("CacheParameterGroupFamily", family),
                ("Description", "created by the catalog inventory suite"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {group}: CreateCacheParameterGroup must answer 200, got {} {}",
        created.status, created.body
    );
    for page_size in ["20", "100"] {
        let case = format!("{group} at {page_size}");
        let reply = describe(
            &emulator,
            &case,
            "DescribeCacheParameters",
            &[
                ("CacheParameterGroupName", group),
                ("MaxRecords", page_size),
            ],
        )
        .await;
        assert_node_type_specific_is_whole(&case, family, &reply);
    }

    let modified = emulator
        .call(
            "ModifyCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", group),
                (
                    "ParameterNameValues.ParameterNameValue.1.ParameterName",
                    "timeout",
                ),
                (
                    "ParameterNameValues.ParameterNameValue.1.ParameterValue",
                    "300",
                ),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "case {group}: ModifyCacheParameterGroup must answer 200, got {} {}",
        modified.status, modified.body
    );
    for page_size in ["20", "100"] {
        let case = format!("{group} modified at {page_size}");
        let reply = describe(
            &emulator,
            &case,
            "DescribeCacheParameters",
            &[
                ("CacheParameterGroupName", group),
                ("MaxRecords", page_size),
            ],
        )
        .await;
        assert_node_type_specific_is_whole(&case, family, &reply);
    }

    // The modified value itself is read back on the page that carries it: the
    // group reports its family's defaults with the customer's value laid over
    // the one it set.
    let case = format!("{group} modified");
    let whole = describe(
        &emulator,
        &case,
        "DescribeCacheParameters",
        &[("CacheParameterGroupName", group)],
    )
    .await;
    let reported = crate::describe_cache_parameters::one(&whole, &case, "timeout");
    assert_eq!(
        reported.field("ParameterValue"),
        Some("300"),
        "case {case}: the modified value must be read back, got {reported:?}"
    );
    assert_eq!(
        reported.field("Source"),
        Some("user"),
        "case {case}: a value the customer set is reported as theirs, got {reported:?}"
    );
}
