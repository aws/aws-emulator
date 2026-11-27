// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONTENT of the ElastiCache reference catalogs — the
//! read-only sets this API reports but no operation creates: the cache engine
//! versions, a family's engine default parameters, the reserved-cache-node
//! offerings and the published service updates.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded
//! answer: a reference catalog is the vendor's PUBLISHED set, so every entry of
//! it is one the service still offers, carries the published attributes of that
//! entry, is identical on every read and on every store, and is derived from
//! neither the clock nor the request — and a resource created from a catalog
//! inherits the whole of it.
//!
//! The rows below therefore span the catalogs: an engine line of each engine and
//! of each naming convention beside a retired one, a parameter of each
//! generation that introduced one beside the families that predate it, both
//! kinds of published service update, and every payment option an offering is
//! sold under.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts (the shared [`crate::harness`]), so only the HTTP status and
//! the XML body are inspected.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::describe_cache_parameters::{
    create_group, describe_ok as describe_group_parameters, one as one_parameter, paged_names,
    reported, unpaged_names,
};
use crate::harness::{params, Emulator, Reply};

/// One engine-version row: the engine, the version, and the parameter group
/// family that version belongs to — or `None` for a version the service does
/// NOT offer.
///
/// Source: the engine versions ElastiCache offers, which is what
/// `state::CACHE_ENGINE_VERSIONS` publishes and what the version catalog
/// reports. The offered rows span all three engines and both naming conventions
/// (a Memcached or early Redis OSS version is named by its patch release, a
/// Redis OSS 6 or later version and every Valkey version by its line), and
/// include entries the issue cites nowhere (`1.6.17`, `4.0.10`, `7.2`); the
/// unoffered rows are the retired Redis OSS lines, which the catalog no longer
/// publishes a version of.
const VERSION_ROWS: [(&str, &str, Option<&str>); 12] = [
    ("memcached", "1.4.5", Some("memcached1.4")),
    ("memcached", "1.6.17", Some("memcached1.6")),
    ("redis", "4.0.10", Some("redis4.0")),
    ("redis", "5.0.6", Some("redis5.0")),
    ("redis", "6.0", Some("redis6.x")),
    ("redis", "7.1", Some("redis7")),
    ("valkey", "7.2", Some("valkey7")),
    ("valkey", "8.2", Some("valkey8")),
    ("valkey", "9.0", Some("valkey9")),
    // Retired: a line ElastiCache no longer offers is reported by nothing.
    ("redis", "2.6.13", None),
    ("redis", "2.8.24", None),
    ("redis", "3.2.10", None),
];

/// One engine-default-parameter row: the parameter, the engine version that
/// introduced it, the families whose line covers that version — each with the
/// `MinimumEngineVersion` that family reports the parameter under — and
/// families whose line predates it.
type ParameterFamilyRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

/// The rows [`a_family_reports_every_published_parameter_of_its_line_and_none_of_a_later_one`]
/// is driven over, in the shape [`ParameterFamilyRow`] gives them.
///
/// Source: the parameter catalogs this service publishes
/// (`state::family_default_parameters`) and the family lines they are read for
/// (`state::FAMILY_LINES`). The rows span the generations: the parameters the
/// Redis OSS 4, 5, 6 and 7 lines introduced, and a Memcached one, so a family
/// reports every parameter its line covers and none of a later one.
///
/// The reported version is the earliest version of THAT family the parameter
/// applies to, so it is the introducing version only where the family's own line
/// introduced it, and the family's line floor for a parameter the engine already
/// had before that line (`lfu-log-factor` arrived with Redis OSS 4.0.9 and is
/// reported under 7.0.0 by `redis7`, whose line starts there).
const PARAMETER_ROWS: [ParameterFamilyRow; 8] = [
    (
        "lfu-log-factor",
        "4.0.9",
        &[("redis4.0", "4.0.9"), ("redis7", "7.0.0")],
        &["redis3.2"],
    ),
    (
        "cluster-allow-reads-when-down",
        "5.0.6",
        &[
            ("redis5.0", "5.0.6"),
            ("redis6.x", "6.0.5"),
            ("valkey9", "9.0.0"),
        ],
        &["redis4.0"],
    ),
    (
        "lazyfree-lazy-user-del",
        "6.0.5",
        &[("redis6.x", "6.0.5"), ("redis7", "7.0.0")],
        &["redis5.0"],
    ),
    (
        "maxmemory-eviction-tenacity",
        "6.0.5",
        &[("redis6.x", "6.0.5"), ("redis7", "7.0.0")],
        &["redis5.0"],
    ),
    (
        "lazyfree-lazy-user-flush",
        "6.2.6",
        &[("redis6.x", "6.2.6"), ("redis7", "7.0.0")],
        &["redis5.0"],
    ),
    (
        "hash-max-listpack-value",
        "7.0.0",
        &[
            ("redis7", "7.0.0"),
            ("valkey7", "7.2.0"),
            ("valkey8", "8.0.0"),
        ],
        &["redis6.x"],
    ),
    (
        "propagation-error-behavior",
        "7.0.0",
        &[("redis7", "7.0.0"), ("valkey9", "9.0.0")],
        &["redis6.x", "memcached1.6"],
    ),
    (
        "slab_chunk_max",
        "1.5.10",
        &[("memcached1.5", "1.5.10"), ("memcached1.6", "1.6.6")],
        &["memcached1.4", "redis7"],
    ),
];

/// The families a group round trip is driven over, with a parameter of that
/// family a customer may set and the value to set it to.
///
/// Both engine catalogs, and two different data types (a Redis OSS-compatible
/// enumeration and a Memcached number), so the rule is asserted over more than
/// the one family the recording exercised.
const GROUP_ROWS: [(&str, &str, &str); 3] = [
    ("redis7", "latency-tracking", "no"),
    ("redis7", "maxmemory-samples", "7"),
    ("memcached1.6", "max_item_size", "2097152"),
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

/// The engine defaults of `family`, over the whole catalog (the operation's
/// documented maximum page).
async fn engine_defaults(emulator: &Emulator, family: &str) -> Reply {
    describe(
        emulator,
        family,
        "DescribeEngineDefaultParameters",
        &[
            ("CacheParameterGroupFamily", family),
            ("MaxRecords", "100"),
        ],
    )
    .await
}

#[tokio::test]
async fn the_version_catalog_reports_the_versions_the_service_offers_and_no_others() {
    let emulator = Emulator::new();

    for (engine, version, family) in VERSION_ROWS {
        let case = format!("{engine} {version}");
        let reply = describe(
            &emulator,
            &case,
            "DescribeCacheEngineVersions",
            &[("Engine", engine), ("EngineVersion", version)],
        )
        .await;
        let reported: Vec<&str> = reply.all("CacheEngineVersion/EngineVersion");
        match family {
            Some(family) => {
                assert_eq!(
                    reported,
                    vec![version],
                    "case {case}: an offered version is reported once, got {reported:?}"
                );
                assert_eq!(
                    reply.text("CacheEngineVersion/CacheParameterGroupFamily"),
                    Some(family),
                    "case {case}: expected the family {family}, got {}",
                    reply.body
                );
                // The description names the engine and the release the version
                // stands for, which is the version itself wherever the catalog
                // names a patch release.
                let description = reply
                    .text("CacheEngineVersion/CacheEngineVersionDescription")
                    .unwrap_or_default();
                let release = description
                    .strip_prefix(&format!("{engine} version "))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: expected '{engine} version <release>', got \
                             {description:?}"
                        )
                    });
                assert!(
                    release.starts_with(version) && release.split('.').count() == 3,
                    "case {case}: the description must name the release of {version}, got \
                     {description:?}"
                );
            }
            None => {
                assert!(
                    reported.is_empty(),
                    "case {case}: a version the service no longer offers is reported by \
                     nothing, got {reported:?}"
                );
                // And nothing can be created on it: the catalog is what a create
                // may name, in the service's own words.
                let refused = emulator
                    .call(
                        "CreateCacheCluster",
                        &crate::harness::cluster_create_params(
                            &format!("retired-{}", version.replace('.', "-")),
                            &[("Engine", engine), ("EngineVersion", version)],
                        ),
                    )
                    .await;
                assert_eq!(
                    refused.status, 400,
                    "case {case}: a create on an unoffered version is refused, got {} {}",
                    refused.status, refused.body
                );
                assert_eq!(
                    refused.error_message(),
                    format!("Cannot find version {version} for {engine}"),
                    "case {case}: unexpected refusal, got {}",
                    refused.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_family_reports_every_published_parameter_of_its_line_and_none_of_a_later_one() {
    let emulator = Emulator::new();

    for (parameter, introduced, carried_by, absent_from) in PARAMETER_ROWS {
        for (family, reported_version) in carried_by {
            let (family, reported_version) = (*family, *reported_version);
            let reply = engine_defaults(&emulator, family).await;
            let names: BTreeSet<String> =
                reported(&reply).into_iter().map(|row| row.name).collect();
            assert!(
                names.contains(parameter),
                "case {family}/{parameter}: a family whose line covers {introduced} publishes \
                 it, got {names:?}"
            );
            let published = one_parameter(&reply, family, parameter);
            assert_eq!(
                published.field("MinimumEngineVersion"),
                Some(reported_version),
                "case {family}/{parameter}: expected MinimumEngineVersion {reported_version} — \
                 the earliest version of this family the parameter applies to — got \
                 {published:?}"
            );
        }
        for family in absent_from {
            let reply = engine_defaults(&emulator, family).await;
            let names: BTreeSet<String> =
                reported(&reply).into_iter().map(|row| row.name).collect();
            assert!(
                !names.contains(parameter),
                "case {family}/{parameter}: a family whose line predates {introduced} \
                 publishes it with nothing, got {names:?}"
            );
        }
    }

    // Every member of every published parameter, over the whole of two
    // catalogs: a client reading the defaults to decide what it may set needs
    // each of them, and a modifiable numeric parameter needs the range it is
    // held to. The counts are the floor of what each engine's published catalog
    // holds — the Redis OSS-compatible generations publish several dozen
    // parameters, Memcached rather fewer — so a catalog trimmed back to a
    // sample of itself fails here.
    for (family, at_least) in [("redis7", 60), ("memcached1.6", 15)] {
        let reply = engine_defaults(&emulator, family).await;
        let rows = reported(&reply);
        assert!(
            rows.len() >= at_least,
            "case {family}: a family publishes its engine's whole catalog ({at_least} \
             parameters or more), got {}",
            rows.len()
        );
        for row in &rows {
            for member in ["Description", "DataType", "IsModifiable", "MinimumEngineVersion"] {
                assert!(
                    row.field(member).is_some_and(|text| !text.is_empty()),
                    "case {family}/{}: {member} must be published, got {row:?}",
                    row.name
                );
            }
            assert_eq!(
                row.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {family}/{}: an engine default is the service's own value, got {row:?}",
                row.name
            );
            let modifiable = row.field("IsModifiable") == Some("true");
            let numeric = matches!(
                row.field("DataType"),
                Some(state::PARAMETER_DATA_TYPE_INTEGER) | Some(state::PARAMETER_DATA_TYPE_FLOAT)
            );
            if modifiable && numeric {
                let allowed = row.field("AllowedValues").unwrap_or_default();
                assert!(
                    allowed.contains('-') || allowed.contains(','),
                    "case {family}/{}: a modifiable numeric parameter publishes the values it \
                     takes, got {allowed:?}",
                    row.name
                );
            }
            if row.node_type_specific {
                // A value per cache node type, and no single value: that is what
                // makes it node-type-specific rather than a plain parameter.
                assert!(
                    row.field("ParameterValue").is_none(),
                    "case {family}/{}: a node-type-specific parameter carries no single value, \
                     got {row:?}",
                    row.name
                );
            }
        }
    }

    // A read does not change what the next caller sees.
    let once = engine_defaults(&emulator, "redis7").await;
    let twice = engine_defaults(&emulator, "redis7").await;
    assert_eq!(
        reported(&once)
            .iter()
            .map(|row| (row.name.clone(), row.fields.clone()))
            .collect::<Vec<_>>(),
        reported(&twice)
            .iter()
            .map(|row| (row.name.clone(), row.fields.clone()))
            .collect::<Vec<_>>(),
        "case twice: two reads of a published catalog report the same parameters"
    );
}

#[tokio::test]
async fn a_created_group_inherits_its_familys_whole_catalog_and_reports_what_was_set() {
    let emulator = Emulator::new();

    for (index, (family, parameter, value)) in GROUP_ROWS.into_iter().enumerate() {
        let case = format!("{family}/{parameter}={value}");
        let group = format!("inherits-{index}");
        create_group(&emulator, &group, family).await;

        // Inherited: the group's two lists are the family's engine defaults,
        // every one of them reported as the service's own value.
        let defaults = engine_defaults(&emulator, family).await;
        let inherited = describe_group_parameters(&emulator, &group, &[("MaxRecords", "100")]).await;
        assert_eq!(
            paged_names(&inherited),
            reported(&defaults)
                .iter()
                .filter(|row| !row.node_type_specific)
                .map(|row| row.name.clone())
                .collect::<Vec<_>>(),
            "case {case}: a new group inherits its family's whole parameter set, got {:?}",
            paged_names(&inherited)
        );
        assert_eq!(
            unpaged_names(&inherited),
            reported(&defaults)
                .iter()
                .filter(|row| row.node_type_specific)
                .map(|row| row.name.clone())
                .collect::<Vec<_>>(),
            "case {case}: a new group inherits its family's node-type-specific set, got {:?}",
            unpaged_names(&inherited)
        );
        for row in reported(&inherited) {
            assert_eq!(
                row.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {case}: an inherited value is the service's own, got {row:?}"
            );
        }
        let before: BTreeMap<String, Option<String>> = reported(&inherited)
            .into_iter()
            .map(|row| {
                (
                    row.name.clone(),
                    row.field("ParameterValue").map(str::to_string),
                )
            })
            .collect();

        // Set one of them, and the group reports THAT value as the customer's
        // while every other parameter keeps the family default.
        let modified = emulator
            .call(
                "ModifyCacheParameterGroup",
                &params(&[
                    ("CacheParameterGroupName", &group),
                    ("ParameterNameValues.ParameterNameValue.1.ParameterName", parameter),
                    ("ParameterNameValues.ParameterNameValue.1.ParameterValue", value),
                ]),
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {case}: the modify must be accepted, got {} {}",
            modified.status, modified.body
        );
        let after = describe_group_parameters(&emulator, &group, &[("MaxRecords", "100")]).await;
        let set = one_parameter(&after, &case, parameter);
        assert_eq!(
            set.field("ParameterValue"),
            Some(value),
            "case {case}: the group must report the value that was set, got {set:?}"
        );
        assert_eq!(
            set.field("Source"),
            Some(state::PARAMETER_SOURCE_USER),
            "case {case}: a value the customer set is reported as theirs, got {set:?}"
        );
        for row in reported(&after) {
            if row.name == parameter {
                continue;
            }
            assert_eq!(
                row.field("ParameterValue").map(str::to_string),
                before.get(&row.name).cloned().flatten(),
                "case {case}: {} must keep its family default, got {row:?}",
                row.name
            );
            assert_eq!(
                row.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {case}: {} was not set by the customer, got {row:?}",
                row.name
            );
        }
    }
}

/// The service-update catalog as the describe reports it: each update's name,
/// mapped to the columns [`UPDATE_COLUMNS`] names, in that order.
async fn service_update_catalog(emulator: &Emulator) -> BTreeMap<String, Vec<String>> {
    let reply = describe(emulator, "service updates", "DescribeServiceUpdates", &[]).await;
    let names = reply.all("ServiceUpdate/ServiceUpdateName");
    let mut catalog: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (index, name) in names.iter().enumerate() {
        let mut row = Vec::new();
        for column in UPDATE_COLUMNS {
            let values = reply.all(column);
            assert_eq!(
                values.len(),
                names.len(),
                "case service updates: {column} must be reported once per update, got {}",
                reply.body
            );
            row.push(values[index].to_string());
        }
        catalog.insert((*name).to_string(), row);
    }
    assert!(
        !catalog.is_empty(),
        "case service updates: the service publishes a catalog of updates"
    );
    catalog
}

/// The members of a published service update every row of
/// [`service_update_catalog`] carries, in column order: the type, the severity,
/// the status, the three dates and the engine.
const UPDATE_COLUMNS: [&str; 7] = [
    "ServiceUpdate/ServiceUpdateType",
    "ServiceUpdate/ServiceUpdateSeverity",
    "ServiceUpdate/ServiceUpdateStatus",
    "ServiceUpdate/ServiceUpdateReleaseDate",
    "ServiceUpdate/ServiceUpdateRecommendedApplyByDate",
    "ServiceUpdate/ServiceUpdateEndDate",
    "ServiceUpdate/Engine",
];

#[tokio::test]
async fn the_service_update_catalog_is_published_rather_than_derived_from_the_clock() {
    // The axis: the store. Two independently built stores publish ONE catalog —
    // same names, same types, same severities and the same three dates — which
    // is what a published set means and what a clock-derived one cannot be.
    let first = Emulator::new();
    let second = Emulator::new();

    let published = service_update_catalog(&first).await;
    assert_eq!(
        service_update_catalog(&second).await,
        published,
        "case two stores: a published catalog is the same set on every store, dates included"
    );
    assert_eq!(
        service_update_catalog(&first).await,
        published,
        "case twice: a read does not change what the next caller sees"
    );

    // Both kinds of update ElastiCache publishes are in it, each with its own
    // type, and the end-of-life ones are still inside their window.
    let types: BTreeSet<&str> = published
        .values()
        .map(|row| row[0].as_str())
        .collect();
    for update_type in [
        state::SERVICE_UPDATE_TYPE,
        state::SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION,
    ] {
        assert!(
            types.contains(update_type),
            "case types: the catalog publishes an update of type {update_type}, got {types:?}"
        );
    }
    for (name, row) in &published {
        if row[0] == state::SERVICE_UPDATE_TYPE_ENGINE_MAJOR_VERSION {
            assert_eq!(
                row[2], state::SERVICE_UPDATE_AVAILABLE,
                "case {name}: an end-of-life update is published as available, got {row:?}"
            );
        }
        assert!(
            row[3] < row[4] && row[4] < row[5],
            "case {name}: a published window runs release < apply-by < end, got {row:?}"
        );
    }
}

#[tokio::test]
async fn every_offering_is_a_stable_catalog_entry_whose_prices_fit_its_payment_option() {
    // The axis: the cache node type and the reservation term — two node types of
    // different families and both published terms — crossed with every payment
    // option the catalog is stocked with.
    let emulator = Emulator::new();
    let terms = state::OFFERING_TERMS.map(|(_, seconds)| seconds.to_string());

    for node_type in ["cache.r6g.large", "cache.m5.xlarge"] {
        for term in &terms {
            let case = format!("{node_type} for {term}s");
            let read = || async {
                describe(
                    &emulator,
                    &case,
                    "DescribeReservedCacheNodesOfferings",
                    &[
                        ("CacheNodeType", node_type),
                        ("Duration", term),
                        ("ProductDescription", state::DEFAULT_ENGINE),
                    ],
                )
                .await
            };
            let reply = read().await;
            let ids = reply.all("ReservedCacheNodesOffering/ReservedCacheNodesOfferingId");
            let options = reply.all("ReservedCacheNodesOffering/OfferingType");
            let fixed = reply.all("ReservedCacheNodesOffering/FixedPrice");
            let recurring = reply.all("RecurringCharge/RecurringChargeAmount");
            assert_eq!(
                options.len(),
                state::STOCKED_OFFERING_TYPES.len(),
                "case {case}: the catalog stocks one offering per payment option, got \
                 {options:?}"
            );
            assert!(
                ids.len() == options.len()
                    && fixed.len() == options.len()
                    && recurring.len() == options.len(),
                "case {case}: every offering reports its id, its prices and its recurring \
                 charge, got {}",
                reply.body
            );
            for (index, option) in options.iter().enumerate() {
                let price: f64 = fixed[index]
                    .parse()
                    .unwrap_or_else(|_| panic!("case {case}: FixedPrice {:?}", fixed[index]));
                let hourly: f64 = recurring[index].parse().unwrap_or_else(|_| {
                    panic!("case {case}: RecurringChargeAmount {:?}", recurring[index])
                });
                // What is not paid up front is the hourly charge, so the two
                // prices follow from the payment option rather than varying
                // independently of it.
                let (fixed_expected, hourly_expected) = match *option {
                    state::OFFERING_TYPE_ALL_UPFRONT => (true, false),
                    state::OFFERING_TYPE_NO_UPFRONT => (false, true),
                    _ => (true, true),
                };
                assert_eq!(
                    price > 0.0,
                    fixed_expected,
                    "case {case}: a {option} offering's FixedPrice must {}be charged, got \
                     {price}",
                    if fixed_expected { "" } else { "not " }
                );
                assert_eq!(
                    hourly > 0.0,
                    hourly_expected,
                    "case {case}: a {option} offering's hourly charge must {}be charged, got \
                     {hourly}",
                    if hourly_expected { "" } else { "not " }
                );
                // The id is a CATALOG id: it identifies the same offering on a
                // later call, so a client can carry it into a purchase.
                let by_id = describe(
                    &emulator,
                    &case,
                    "DescribeReservedCacheNodesOfferings",
                    &[("ReservedCacheNodesOfferingId", ids[index])],
                )
                .await;
                assert_eq!(
                    by_id.all("ReservedCacheNodesOffering/OfferingType"),
                    vec![*option],
                    "case {case}: {} must resolve to the {option} offering, got {}",
                    ids[index],
                    by_id.body
                );
            }
            assert_eq!(
                read()
                    .await
                    .all("ReservedCacheNodesOffering/ReservedCacheNodesOfferingId"),
                ids,
                "case {case}: a published offering keeps its id across reads"
            );
        }
    }
}
