// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeCacheEngineVersions`.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from the published catalog cannot pass. The
//! request builder and the XML reader are the ones the `cache_cluster_lifecycle`
//! suite marks `pub(crate)`, so this target keeps one harness rather than a
//! second copy that could drift.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (which filter carries a value, how that value selects —
//! an exact version, a version line, a family, a case variant, a value nothing
//! matches — the page size, and the refused value), so no single recorded
//! request is what makes them pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// Every `CacheParameterGroupFamily` this service publishes, as `state` states
/// it once for the whole crate (`state::PARAMETER_GROUP_FAMILIES`, whose source
/// comment cites `smithy_query(query="doc",
/// shape="CreateCacheParameterGroupMessage", member="CacheParameterGroupFamily")`).
/// Read through that constant rather than copied here, so the engine-version
/// catalog and the parameter groups cannot be asserted against two different
/// lists.
const DOCUMENTED_FAMILIES: [&str; 13] = state::PARAMETER_GROUP_FAMILIES;

/// One `CacheEngineVersion` element of a response, as the wire reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireVersion {
    engine: String,
    engine_version: String,
    family: String,
    engine_description: String,
    version_description: String,
}

/// Every `CacheEngineVersion` a reply reports, in document order.
///
/// Each member is read as a column and the columns are zipped, which is also the
/// assertion that every reported version carries every member of the shape.
fn wire_versions(case: &str, reply: &Reply) -> Vec<WireVersion> {
    let engines = reply.all("Engine");
    let count = engines.len();
    let column = |member: &str| -> Vec<String> {
        let values = reply.all(member);
        assert_eq!(
            values.len(),
            count,
            "case {case}: {member} must be reported once per version ({count}), got {}: {}",
            values.len(),
            reply.body
        );
        values.into_iter().map(str::to_string).collect()
    };
    let versions = column("EngineVersion");
    let families = column("CacheParameterGroupFamily");
    let engine_descriptions = column("CacheEngineDescription");
    let version_descriptions = column("CacheEngineVersionDescription");
    (0..count)
        .map(|index| WireVersion {
            engine: engines[index].to_string(),
            engine_version: versions[index].clone(),
            family: families[index].clone(),
            engine_description: engine_descriptions[index].clone(),
            version_description: version_descriptions[index].clone(),
        })
        .collect()
}

/// One `DescribeCacheEngineVersions` call that must succeed, with the versions
/// it reported.
async fn describe(
    emulator: &Emulator,
    case: &str,
    pairs: &[(String, String)],
) -> (Reply, Vec<WireVersion>) {
    let reply = emulator
        .call("DescribeCacheEngineVersions", pairs)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeCacheEngineVersions must answer 200, got {}: {}",
        reply.status, reply.body
    );
    let versions = wire_versions(case, &reply);
    (reply, versions)
}

/// The whole published catalog, in the order the service reports it.
async fn catalog(emulator: &Emulator, case: &str) -> Vec<WireVersion> {
    describe(emulator, case, &[]).await.1
}

/// One filter row: which of the three filters carry a value.
///
/// Every row's `Engine` names an engine the catalog publishes, because an
/// `Engine` it does not publish is REFUSED rather than filtered — see
/// `describe_cache_engine_versions_refuses_an_engine_it_does_not_publish` in the
/// `read_validate_fidelity` suite, and the engine rows of
/// [`a_value_the_describe_cannot_serve_is_refused_with_invalid_parameter_value`].
#[derive(Debug, Clone, Copy)]
struct FilterCase {
    name: &'static str,
    engine: Option<&'static str>,
    family: Option<&'static str>,
    engine_version: Option<&'static str>,
    /// Versions the filter must report whatever else it reports — the row's own
    /// statement of what it selects, independent of [`selects`] below.
    must_report: &'static [&'static str],
    /// True when the filter names something the catalog does not publish, so
    /// the answer is an empty page rather than a refusal.
    must_be_empty: bool,
}

impl FilterCase {
    /// The form pairs that carry this row's filters.
    fn pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        for (key, value) in [
            ("Engine", self.engine),
            ("CacheParameterGroupFamily", self.family),
            ("EngineVersion", self.engine_version),
        ] {
            if let Some(value) = value {
                pairs.push((key.to_string(), value.to_string()));
            }
        }
        pairs
    }
}

/// True when `row` is a version this filter selects.
///
/// The rule, restated here rather than borrowed from the implementation: a
/// filter keeps a version whose engine and family equal the value it names
/// (ignoring case, the engine being stored lowercase), and whose version is the
/// one `EngineVersion` names or lies in the version line it names (`5.0` names
/// `5.0.5` and `5.0.6`).
fn selects(case: &FilterCase, row: &WireVersion) -> bool {
    case.engine
        .is_none_or(|engine| row.engine.eq_ignore_ascii_case(engine))
        && case
            .family
            .is_none_or(|family| row.family.eq_ignore_ascii_case(family))
        && case.engine_version.is_none_or(|wanted| {
            row.engine_version == wanted
                || row
                    .engine_version
                    .strip_prefix(wanted)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
}

/// The filter table every filter test walks.
const FILTER_CASES: [FilterCase; 12] = [
    FilterCase {
        name: "no filter",
        engine: None,
        family: None,
        engine_version: None,
        must_report: &["7.1", "1.6.22", "9.1"],
        must_be_empty: false,
    },
    FilterCase {
        name: "engine redis",
        engine: Some("redis"),
        family: None,
        engine_version: None,
        must_report: &["4.0.10", "7.1"],
        must_be_empty: false,
    },
    FilterCase {
        name: "engine memcached",
        engine: Some("memcached"),
        family: None,
        engine_version: None,
        // The version `EngineVersion`'s own documentation uses as its example.
        must_report: &["1.4.14", "1.6.22"],
        must_be_empty: false,
    },
    FilterCase {
        name: "engine valkey",
        engine: Some("valkey"),
        family: None,
        engine_version: None,
        must_report: &["7.2", "8.0", "9.1"],
        must_be_empty: false,
    },
    FilterCase {
        name: "engine in the wrong case",
        engine: Some("REDIS"),
        family: None,
        engine_version: None,
        must_report: &["7.1"],
        must_be_empty: false,
    },
    FilterCase {
        // A family the issue cites nowhere, and the one whose name the
        // engine-plus-major rule cannot spell.
        name: "family redis6.x",
        engine: None,
        family: Some("redis6.x"),
        engine_version: None,
        must_report: &["6.0", "6.2"],
        must_be_empty: false,
    },
    FilterCase {
        name: "family memcached1.4",
        engine: None,
        family: Some("memcached1.4"),
        engine_version: None,
        must_report: &["1.4.14", "1.4.34"],
        must_be_empty: false,
    },
    FilterCase {
        name: "family in the wrong case",
        engine: None,
        family: Some("VALKEY7"),
        engine_version: None,
        must_report: &["7.2"],
        must_be_empty: false,
    },
    FilterCase {
        name: "family nothing belongs to",
        engine: None,
        family: Some("redis9.9"),
        engine_version: None,
        must_report: &[],
        must_be_empty: true,
    },
    FilterCase {
        name: "one exact version",
        engine: None,
        family: None,
        engine_version: Some("7.1"),
        must_report: &["7.1"],
        must_be_empty: false,
    },
    FilterCase {
        name: "a whole version line",
        engine: None,
        family: None,
        engine_version: Some("6"),
        must_report: &["6.0", "6.2"],
        must_be_empty: false,
    },
    FilterCase {
        name: "engine and version line together",
        engine: Some("memcached"),
        family: None,
        engine_version: Some("1.4"),
        must_report: &["1.4.14", "1.4.24"],
        must_be_empty: false,
    },
];

#[tokio::test]
async fn the_published_catalog_is_reported_with_every_member_of_the_shape() {
    let emulator = Emulator::new();
    let case = "unfiltered";
    let published = catalog(&emulator, case).await;
    assert!(
        !published.is_empty(),
        "case {case}: the service publishes a catalog, so an unfiltered describe reports versions"
    );

    for row in &published {
        let name = format!("{} {}", row.engine, row.engine_version);
        // `Engine` is "the name of the cache engine" and `EngineVersion` "the
        // version number of the cache engine".
        assert!(
            state::ENGINES.contains(&row.engine.as_str()),
            "case {case}: {name} must run one of {:?}, got {:?}",
            state::ENGINES,
            row.engine
        );
        assert!(
            !row.engine_version.is_empty()
                && row
                    .engine_version
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.'),
            "case {case}: {name} must carry a dotted version number, got {:?}",
            row.engine_version
        );
        // "The name of the cache parameter group family associated with this
        // cache engine" — one of the families this service publishes, and the
        // family whose default parameter group another operation reports (the
        // cross-check below).
        assert!(
            DOCUMENTED_FAMILIES.contains(&row.family.as_str()),
            "case {case}: {name}'s family must be one of {DOCUMENTED_FAMILIES:?}, got {:?}",
            row.family
        );
        // The two descriptions: of the engine, and of this version of it.
        assert_eq!(
            row.engine_description,
            state::engine_description(&row.engine),
            "case {case}: {name} must describe its engine, got {:?}",
            row.engine_description
        );
        assert!(
            row.version_description.contains(&row.engine_version)
                && row.version_description.contains(&row.engine),
            "case {case}: {name}'s version description must name the engine and the version, got \
             {:?}",
            row.version_description
        );
    }

    // Every engine the service runs publishes at least one version, and every
    // family the model documents has one.
    for engine in state::ENGINES {
        assert!(
            published.iter().any(|row| row.engine == engine),
            "case {case}: the catalog must publish a version of {engine}, got {:?}",
            published.iter().map(|row| &row.engine).collect::<Vec<_>>()
        );
    }
    // Every family of an engine line the service still OFFERS carries a
    // published version. The documented family list OUTLIVES the versions: the
    // families of the retired Redis OSS lines (redis2.6, redis2.8, redis3.2)
    // are still published, and still carry their default parameter groups,
    // though the catalog offers no version of them — so the documented families
    // are a superset of the families below.
    for family in [
        "memcached1.4",
        "memcached1.5",
        "memcached1.6",
        "redis4.0",
        "redis5.0",
        "redis6.x",
        "redis7",
        "valkey7",
        "valkey8",
        "valkey9",
    ] {
        assert!(
            DOCUMENTED_FAMILIES.contains(&family),
            "case {case}: {family} must be one of the documented families \
             {DOCUMENTED_FAMILIES:?}"
        );
        assert!(
            published.iter().any(|row| row.family == family),
            "case {case}: the catalog must publish a version in the published {family} family, \
             got {:?}",
            published.iter().map(|row| &row.family).collect::<Vec<_>>()
        );
    }

    // Cross-checked against the operation that reports the groups themselves:
    // every family a published version names has its default parameter group,
    // so the two catalogs agree over the wire and not just in one module.
    let groups = emulator.call("DescribeCacheParameterGroups", &[]).await;
    assert_eq!(
        groups.status, 200,
        "case {case}: DescribeCacheParameterGroups must answer 200, got {}: {}",
        groups.status, groups.body
    );
    let group_names = groups.all("CacheParameterGroupName");
    for row in &published {
        let expected = format!("default.{}", row.family);
        assert!(
            group_names.contains(&expected.as_str()),
            "case {case}: {} {} is in the {} family, so {expected} must be a group this service \
             holds, got {group_names:?}",
            row.engine,
            row.engine_version,
            row.family
        );
    }
    // More than one line per engine and more than one version in a line: the
    // shape `DefaultOnly` and the version-line filter are about.
    for engine in state::ENGINES {
        let mut families: Vec<&str> = published
            .iter()
            .filter(|row| row.engine == engine)
            .map(|row| row.family.as_str())
            .collect();
        families.sort_unstable();
        families.dedup();
        assert!(
            families.len() >= 2,
            "case {case}: {engine} must publish more than one version line, got {families:?}"
        );
    }

    // An engine and a version together identify a row, and the catalog is
    // ordered by engine and then by version from oldest to newest.
    let keys: Vec<String> = published
        .iter()
        .map(|row| state::engine_version_key(&row.engine, &row.engine_version))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        keys, sorted,
        "case {case}: the catalog must be reported in engine-then-version order"
    );
    let mut unique = keys.clone();
    unique.dedup();
    assert_eq!(
        unique.len(),
        keys.len(),
        "case {case}: every engine and version pair must be published once, got {keys:?}"
    );
}

#[tokio::test]
async fn each_filter_reports_exactly_the_versions_it_selects() {
    let emulator = Emulator::new();
    let published = catalog(&emulator, "unfiltered").await;

    for case in FILTER_CASES {
        let name = case.name;
        let expected: Vec<WireVersion> = published
            .iter()
            .filter(|row| selects(&case, row))
            .cloned()
            .collect();
        let (reply, reported) = describe(&emulator, name, &case.pairs()).await;
        assert_eq!(
            reported, expected,
            "case {name}: must report exactly the versions the filter selects, got {:?}",
            reported
                .iter()
                .map(|row| format!("{} {}", row.engine, row.engine_version))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            reported.is_empty(),
            case.must_be_empty,
            "case {name}: a filter naming something the catalog does not publish reports an \
             empty page, got {} versions: {}",
            reported.len(),
            reply.body
        );
        for version in case.must_report {
            assert!(
                reported.iter().any(|row| row.engine_version == *version),
                "case {name}: must report version {version}, got {:?}",
                reported
                    .iter()
                    .map(|row| row.engine_version.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[tokio::test]
async fn default_only_reports_the_newest_version_of_each_version_line_it_selects() {
    let emulator = Emulator::new();
    let published = catalog(&emulator, "unfiltered").await;

    // `DefaultOnly` composes with every filter row: whatever the other filters
    // leave, it keeps the newest version of each engine-and-version-line — the
    // combination a `CacheParameterGroupFamily` names — among them.
    for case in FILTER_CASES {
        let name = format!("DefaultOnly with {}", case.name);
        let mut expected: Vec<WireVersion> = Vec::new();
        for row in published.iter().filter(|row| selects(&case, row)) {
            match expected
                .iter_mut()
                .find(|kept| kept.engine == row.engine && kept.family == row.family)
            {
                // The newest version wins, compared as version numbers rather
                // than as strings (`1.6.22` is newer than `1.6.6`).
                Some(kept) => {
                    if state::version_sort_key(&kept.engine_version)
                        < state::version_sort_key(&row.engine_version)
                    {
                        *kept = row.clone();
                    }
                }
                None => expected.push(row.clone()),
            }
        }
        expected.sort_by_key(|row| state::engine_version_key(&row.engine, &row.engine_version));
        let mut pairs = case.pairs();
        pairs.push(("DefaultOnly".to_string(), "true".to_string()));
        let (_, reported) = describe(&emulator, &name, &pairs).await;
        assert_eq!(
            reported, expected,
            "case {name}: must report the newest version of each version line the other filters \
             leave"
        );
    }

    // Pinned explicitly, without going through the rule above: with nothing else
    // named, the answer is one version per published family, and it is the
    // newest one in that family.
    let (_, every_line) = describe(
        &emulator,
        "DefaultOnly unfiltered",
        &params(&[("DefaultOnly", "true")]),
    )
    .await;
    let mut reported_families: Vec<&str> = every_line.iter().map(|row| row.family.as_str()).collect();
    reported_families.sort_unstable();
    let mut published_families: Vec<&str> = published.iter().map(|row| row.family.as_str()).collect();
    published_families.sort_unstable();
    published_families.dedup();
    assert_eq!(
        reported_families, published_families,
        "case DefaultOnly unfiltered: must report exactly one version per published family"
    );

    // The engine's own default version — the one a cluster that names no version
    // runs — is the default of its newest line.
    for engine in state::ENGINES {
        let name = format!("DefaultOnly for {engine}");
        let (_, reported) = describe(
            &emulator,
            &name,
            &params(&[("Engine", engine), ("DefaultOnly", "true")]),
        )
        .await;
        let newest = reported
            .last()
            .unwrap_or_else(|| panic!("case {name}: {engine} must publish a version line"));
        assert_eq!(
            newest.engine_version,
            state::default_engine_version(engine),
            "case {name}: the newest line's default must be the version a new cluster runs, got \
             {:?}",
            newest.engine_version
        );
        for row in &reported {
            assert_eq!(
                row.engine, engine,
                "case {name}: must report only {engine} versions, got {:?}",
                row.engine
            );
        }
    }
    for (family, newest) in [
        ("memcached1.4", "1.4.34"),
        ("redis6.x", "6.2"),
        ("redis7", "7.1"),
    ] {
        let name = format!("DefaultOnly for family {family}");
        let (_, reported) = describe(
            &emulator,
            &name,
            &params(&[
                ("CacheParameterGroupFamily", family),
                ("DefaultOnly", "true"),
            ]),
        )
        .await;
        let versions: Vec<&str> = reported
            .iter()
            .map(|row| row.engine_version.as_str())
            .collect();
        assert_eq!(
            versions,
            vec![newest],
            "case {name}: must report only the newest version of that family"
        );
    }

    // `DefaultOnly=false` is the unfiltered answer: the flag only ever narrows.
    let (_, all_of_them) = describe(
        &emulator,
        "DefaultOnly false",
        &params(&[("DefaultOnly", "false")]),
    )
    .await;
    assert_eq!(
        all_of_them, published,
        "case DefaultOnly false: must report the whole catalog"
    );
}

/// Walk every page of a describe, asserting each page holds at most
/// `max_records` versions and that the marker is opaque, and return the versions
/// in the order the pages reported them.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    max_records: Option<i32>,
    engine: Option<&str>,
) -> Vec<WireVersion> {
    let mut collected: Vec<WireVersion> = Vec::new();
    let mut marker: Option<String> = None;
    for page_number in 1..=64 {
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Some(engine) = engine {
            pairs.push(("Engine".to_string(), engine.to_string()));
        }
        if let Some(size) = max_records {
            pairs.push(("MaxRecords".to_string(), size.to_string()));
        }
        if let Some(token) = &marker {
            pairs.push(("Marker".to_string(), token.clone()));
        }
        let (reply, page) = describe(emulator, case, &pairs).await;
        if let Some(size) = max_records {
            assert!(
                page.len() <= usize::try_from(size).expect("a positive page size"),
                "case {case}: page {page_number} must hold at most {size} versions, got {}",
                page.len()
            );
        }
        collected.extend(page);
        match reply.marker() {
            Some(token) => {
                assert!(
                    !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit()),
                    "case {case}: the Marker must be an opaque token, got {token:?}"
                );
                assert!(
                    !collected
                        .iter()
                        .any(|row| token.contains(row.engine_version.as_str())),
                    "case {case}: the Marker must not spell out a record's version, got {token:?}"
                );
                marker = Some(token.to_string());
            }
            None => return collected,
        }
    }
    panic!("case {case}: pagination did not terminate");
}

#[tokio::test]
async fn pagination_covers_the_catalog_at_every_page_size_it_serves() {
    let emulator = Emulator::new();
    let published = catalog(&emulator, "unfiltered").await;
    let total = i32::try_from(published.len()).expect("a small catalog");
    assert!(
        total > state::MAX_RECORDS_MIN,
        "the catalog must hold more versions than the smallest page the service serves \
         ({total} > {}), or no page would ever carry a Marker",
        state::MAX_RECORDS_MIN
    );

    // The axis: the page size, over the whole published range — both bounds,
    // the size that splits the catalog, one that does not divide it evenly, and
    // the request that names no MaxRecords at all.
    let rows: [Option<i32>; 6] = [
        Some(state::MAX_RECORDS_MIN),
        Some(state::MAX_RECORDS_MIN + 1),
        Some(total - 1),
        Some(total),
        Some(state::MAX_RECORDS_MAX),
        None,
    ];
    for max_records in rows {
        let case = format!("MaxRecords {max_records:?}");
        let walked = walk_pages(&emulator, &case, max_records, None).await;
        assert_eq!(
            walked, published,
            "case {case}: walking the pages must report the whole catalog, in order"
        );
    }

    // Pagination and the engine filter compose: a filtered walk reports the
    // filtered catalog.
    for engine in state::ENGINES {
        let case = format!("paged walk of {engine}");
        let expected: Vec<WireVersion> = published
            .iter()
            .filter(|row| row.engine == engine)
            .cloned()
            .collect();
        let walked = walk_pages(
            &emulator,
            &case,
            Some(state::MAX_RECORDS_MIN),
            Some(engine),
        )
        .await;
        assert_eq!(
            walked, expected,
            "case {case}: a filtered walk must report every published {engine} version, in order"
        );
    }
}

#[tokio::test]
async fn a_value_the_describe_cannot_serve_is_refused_with_invalid_parameter_value() {
    let emulator = Emulator::new();

    // `MaxRecords` documents "minimum 20; maximum 100", so each published bound
    // is served and the value one past it is refused; a `Marker` is a token only
    // this service mints, so one it did not mint is refused too.
    let served = [state::MAX_RECORDS_MIN, state::MAX_RECORDS_MAX];
    for size in served {
        let case = format!("MaxRecords {size}");
        let (_, reported) = describe(
            &emulator,
            &case,
            &params(&[("MaxRecords", &size.to_string())]),
        )
        .await;
        assert!(
            !reported.is_empty(),
            "case {case}: a published page size must be served"
        );
    }

    let refused = [
        ("MaxRecords below the minimum", state::MAX_RECORDS_MIN - 1),
        ("MaxRecords above the maximum", state::MAX_RECORDS_MAX + 1),
        ("MaxRecords of no records", 0),
        ("MaxRecords negative", -5),
    ];
    for (case, size) in refused {
        let reply = emulator
            .call(
                "DescribeCacheEngineVersions",
                &params(&[("MaxRecords", &size.to_string())]),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: must be refused with HTTP 400, got {}: {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: must be refused with InvalidParameterValue, got {:?}: {}",
            reply.error_code(),
            reply.body
        );
    }

    let markers = [
        ("marker not minted here", "not-a-marker"),
        ("marker of an odd length", "zzzz"),
        ("marker spelling a version", "7.1"),
    ];
    for (case, marker) in markers {
        let reply = emulator
            .call(
                "DescribeCacheEngineVersions",
                &params(&[("Marker", marker)]),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: must be refused with HTTP 400, got {}: {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: must be refused with InvalidParameterValue, got {:?}: {}",
            reply.error_code(),
            reply.body
        );
    }

    // `Engine` names an engine of the published catalog, so a value that names
    // none is refused rather than answered with an empty page: a client could
    // not otherwise tell a typo from an engine with no versions.
    let engines = [
        ("engine nothing runs", "sqlite"),
        ("engine misspelled", "readis"),
        ("empty engine", ""),
    ];
    for (case, engine) in engines {
        let reply = emulator
            .call(
                "DescribeCacheEngineVersions",
                &params(&[("Engine", engine)]),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: must be refused with HTTP 400, got {}: {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: must be refused with InvalidParameterValue, got {:?}: {}",
            reply.error_code(),
            reply.body
        );
        // The template too, so the suite that owns the engine filter pins the
        // words the refusal is made of. An engine sent EMPTY leaves the message
        // ending in a space, which the XML reader trims away, so that row is
        // asserted on the raw element.
        let expected = state::invalid_engine_name_message(engine);
        if engine.is_empty() {
            assert!(
                reply.body.contains(&format!("<Message>{expected}</Message>")),
                "case {case}: expected the message {expected:?}, got {}",
                reply.body
            );
        } else {
            assert_eq!(
                reply.error_message(),
                expected,
                "case {case}: expected the message {expected:?}, got {:?}",
                reply.error_message()
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_created_without_a_version_runs_a_published_one() {
    let emulator = Emulator::new();

    // The catalog is the one statement of which versions exist, so the version
    // and parameter group a cluster gets when it names neither must come out of
    // it. The axis is the engine.
    for engine in state::ENGINES {
        let id = format!("{engine}-default-version");
        let created = emulator
            .call(
                "CreateCacheCluster",
                &crate::harness::cluster_create_params(&id, &[("Engine", engine)]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {id}: create must answer 200, got {}",
            created.body
        );
        let described = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("CacheClusterId", &id)]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {id}: describe must answer 200, got {}",
            described.body
        );
        let version = described
            .text("CacheCluster/EngineVersion")
            .unwrap_or_default()
            .to_string();
        let group = described
            .text("CacheParameterGroup/CacheParameterGroupName")
            .unwrap_or_default()
            .to_string();

        // The version it runs is the default of that engine's newest version
        // line, which is the last row `DefaultOnly` reports for the engine — a
        // cluster reports the RELEASE behind that line, which is what
        // `CacheCluster$EngineVersion` ("the version of the cache engine that is
        // used in this cluster") carries.
        let (_, default_only) = describe(
            &emulator,
            &id,
            &params(&[("Engine", engine), ("DefaultOnly", "true")]),
        )
        .await;
        let newest = default_only
            .last()
            .unwrap_or_else(|| panic!("case {id}: {engine} must publish a version line"));
        assert_eq!(
            state::patch_release(engine, &newest.engine_version),
            version,
            "case {id}: the cluster runs {version}, so that is the release behind the default \
             version of {engine}'s newest line, got {:?}",
            newest.engine_version
        );

        // …and its default parameter group is named for the family the catalog
        // gives that exact version.
        let (_, exact) = describe(
            &emulator,
            &id,
            &params(&[("Engine", engine), ("EngineVersion", &newest.engine_version)]),
        )
        .await;
        let family = exact
            .first()
            .unwrap_or_else(|| {
                panic!(
                    "case {id}: {engine} {} must be published",
                    newest.engine_version
                )
            })
            .family
            .clone();
        assert_eq!(
            group,
            format!("default.{family}"),
            "case {id}: {engine} {version} is in the {family} family, so its default group is \
             default.{family}"
        );
    }
}

#[tokio::test]
async fn describing_the_catalog_twice_reports_the_same_versions() {
    let emulator = Emulator::new();
    // A describe is a read: the second call sees exactly what the first did.
    let first = catalog(&emulator, "first read").await;
    let second = catalog(&emulator, "second read").await;
    assert_eq!(
        first, second,
        "case repeated read: a describe must not change what a later caller sees"
    );
}
