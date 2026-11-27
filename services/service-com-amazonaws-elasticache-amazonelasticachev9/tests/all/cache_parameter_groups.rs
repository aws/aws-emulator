// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache cache-parameter-group control plane:
//! `CreateCacheParameterGroup` and the `DescribeCacheParameterGroups` reader it
//! is read back through.
//!
//! The rules under test: a create PERSISTS the group under the identity the API
//! documents (`CacheParameterGroupName` "is stored as a lowercase string"),
//! answers with the persisted record, and is refused — with nothing persisted —
//! for a name already held, a family the service does not publish or tags
//! outside the documented bound; and the describe reports the groups the account
//! HOLDS (the default group of every published family, plus every created
//! group), filtered to one when the request names one, refused with
//! `CacheParameterGroupNotFound` when it names a group the account does not
//! hold, and paged at the documented `MaxRecords` window. The groups the account
//! holds from the start are the default group of every published family plus the
//! cluster-mode-on default of every family that can run cluster mode (the
//! `catalog_publication` suite is where that second form is pinned).
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, reusing the harness in [`crate::cache_cluster_lifecycle`];
//! only the HTTP status and the XML body are inspected, so a response that is
//! not computed from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (the family, the name's case, the description's
//! content, the tag count, the page size, the Region signed for), so no single
//! recorded request is what makes them pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply};

/// The region every test signs with unless it is exercising the Region rule
/// itself.
const REGION: &str = "us-east-1";

/// The cache parameter group families this API publishes, written out here
/// rather than read from the crate so the test states the contract
/// independently. Source: `CreateCacheParameterGroupMessage$CacheParameterGroupFamily`
/// (`smithy_query(query="doc", shape="CreateCacheParameterGroupMessage",
/// member="CacheParameterGroupFamily")`: "Valid values are: valkey8 | valkey7 |
/// memcached1.4 | memcached1.5 | memcached1.6 | redis2.6 | redis2.8 | redis3.2
/// | redis4.0 | redis5.0 | redis6.x | redis7"), plus `valkey9` — the family of
/// the Valkey generation that value list does not name yet while the catalog
/// offers its versions — in the sorted order a describe reports them in. A
/// family outlives the engine versions it was published for, so the families of
/// the retired Redis OSS lines are here too.
const FAMILIES: [&str; 13] = [
    "memcached1.4",
    "memcached1.5",
    "memcached1.6",
    "redis2.6",
    "redis2.8",
    "redis3.2",
    "redis4.0",
    "redis5.0",
    "redis6.x",
    "redis7",
    "valkey7",
    "valkey8",
    "valkey9",
];

/// The families whose default parameter group comes in BOTH forms: the plain
/// `default.<family>` and the cluster-mode-on `default.<family>.cluster.on`.
///
/// Written out here rather than read from the crate so the test states the
/// contract independently. Source: cluster mode is a Valkey / Redis OSS feature
/// of Redis OSS 3.2 and later (`CreateReplicationGroup`'s own documentation: "To
/// create a Valkey or Redis OSS (cluster mode enabled) replication group, use
/// CacheParameterGroupName=default.redis3.2.cluster.on"), so every family from
/// that line onward publishes one and the Memcached and pre-3.2 families — which
/// cannot run cluster mode at all — publish only the plain default.
const CLUSTER_MODE_FAMILIES: [&str; 8] = [
    "redis3.2", "redis4.0", "redis5.0", "redis6.x", "redis7", "valkey7", "valkey8", "valkey9",
];

/// Every default parameter group this service publishes, in the sorted order a
/// describe reports them in: one per family in [`FAMILIES`], plus the
/// cluster-mode-on group of every family in [`CLUSTER_MODE_FAMILIES`].
fn published_default_groups() -> Vec<String> {
    let mut names: Vec<String> = FAMILIES
        .iter()
        .map(|family| format!("default.{family}"))
        .chain(
            CLUSTER_MODE_FAMILIES
                .iter()
                .map(|family| format!("default.{family}.cluster.on")),
        )
        .collect();
    names.sort();
    names
}

/// Create one cache parameter group, with whatever tags `tags` names.
async fn create_group(
    emulator: &Emulator,
    name: &str,
    family: &str,
    description: &str,
    tags: &[(&str, &str)],
) -> Reply {
    create_group_in(emulator, REGION, name, family, description, tags).await
}

/// [`create_group`], signed for `region`.
async fn create_group_in(
    emulator: &Emulator,
    region: &str,
    name: &str,
    family: &str,
    description: &str,
    tags: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[
        ("CacheParameterGroupName", name),
        ("CacheParameterGroupFamily", family),
        ("Description", description),
    ]);
    for (index, (key, value)) in tags.iter().enumerate() {
        pairs.push((format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()));
        pairs.push((
            format!("Tags.Tag.{}.Value", index + 1),
            (*value).to_string(),
        ));
    }
    emulator
        .call_in(region, "CreateCacheParameterGroup", &pairs)
        .await
}

/// Describe every group, or just the one `name` names.
async fn describe_groups(emulator: &Emulator, name: Option<&str>) -> Reply {
    let pairs = match name {
        Some(name) => params(&[("CacheParameterGroupName", name)]),
        None => Vec::new(),
    };
    emulator.call("DescribeCacheParameterGroups", &pairs).await
}

/// [`describe_groups`] for one group, signed for `region`.
async fn describe_group_in(emulator: &Emulator, region: &str, name: &str) -> Reply {
    emulator
        .call_in(
            region,
            "DescribeCacheParameterGroups",
            &params(&[("CacheParameterGroupName", name)]),
        )
        .await
}

/// Every `CacheParameterGroupName` a describe reply reports, in page order.
fn reported_names(reply: &Reply) -> Vec<&str> {
    reply.all("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupName")
}

/// Assert that `reply` reports `expected` for the element at `path`.
///
/// Two expected values are their own case, because the XML reader reports text
/// nodes trimmed and drops empty ones: the awsQuery serializer renders `Some("")`
/// as an EMPTY ELEMENT, and a value that is only whitespace (the single space a
/// blank description normalises to) carries no text node either — so both are
/// asserted on the ELEMENT in the raw body rather than on its text.
fn assert_member(reply: &Reply, case: &str, path: &str, expected: &str) {
    if expected.trim().is_empty() {
        let element = path.rsplit('/').next().unwrap_or(path);
        let rendered = if expected.is_empty() {
            vec![
                format!("<{element}></{element}>"),
                format!("<{element}/>"),
            ]
        } else {
            vec![format!("<{element}>{expected}</{element}>")]
        };
        assert!(
            rendered.iter().any(|form| reply.body.contains(form)),
            "case {case}: {path} must be reported as one of {rendered:?}, got {}",
            reply.body
        );
        return;
    }
    assert_eq!(
        reply.text(path),
        Some(expected),
        "case {case}: expected {expected:?} for {path}, got {:?}",
        reply.text(path)
    );
}

/// The ARN a group named `name` carries in `region`.
fn expected_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:elasticache:{region}:{}:parametergroup:{name}",
        engine::emulator_account_id()
    )
}

#[tokio::test]
async fn every_published_family_has_its_default_group_reported() {
    let emulator = Emulator::new();
    let reply = describe_groups(&emulator, None).await;
    assert_eq!(
        reply.status, 200,
        "case describe-all: must succeed, got {} {}",
        reply.status, reply.body
    );

    // Every family's default group must be reported, including the ones no
    // cluster in this suite ever names; see [`FAMILIES`] for the catalog's
    // source.
    let names = reported_names(&reply);
    for family in FAMILIES {
        let expected = format!("default.{family}");
        assert!(
            names.contains(&expected.as_str()),
            "case family {family}: expected {expected} among the reported groups, got {names:?}"
        );
    }
    // A family that can run cluster mode publishes its cluster-mode-on default
    // beside the plain one, and one that cannot publishes only the plain one.
    for family in CLUSTER_MODE_FAMILIES {
        let expected = format!("default.{family}.cluster.on");
        assert!(
            names.contains(&expected.as_str()),
            "case family {family}: expected {expected} among the reported groups, got {names:?}"
        );
    }
    assert_eq!(
        names,
        published_default_groups(),
        "case describe-all: expected exactly the published default groups, got {names:?}"
    );

    // A describe is a read: calling it twice reports the same groups.
    let again = describe_groups(&emulator, None).await;
    assert_eq!(
        reported_names(&again),
        names,
        "case describe-twice: a read must not change what the next caller sees"
    );

    // Each reported group carries its family, a description and an ARN.
    let families = reply.all("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupFamily");
    assert_eq!(
        families.len(),
        names.len(),
        "case describe-all: every group must report its family, got {families:?}"
    );
    for (name, family) in names.iter().zip(families.iter()) {
        assert!(
            *name == format!("default.{family}")
                || *name == format!("default.{family}.cluster.on"),
            "case group {name}: a default group's name is `default.` plus its family, optionally \
             with the cluster-mode-on suffix, got {family}"
        );
    }
    let arns = reply.all("CacheParameterGroups/CacheParameterGroup/ARN");
    assert_eq!(
        arns.len(),
        names.len(),
        "case describe-all: every group must report its ARN, got {arns:?}"
    );
    let globals = reply.all("CacheParameterGroups/CacheParameterGroup/IsGlobal");
    assert!(
        globals.iter().all(|flag| *flag == "false"),
        "case describe-all: no group here belongs to a global datastore, got {globals:?}"
    );
}

#[tokio::test]
async fn a_named_group_is_reported_alone_and_an_unknown_name_is_refused() {
    let emulator = Emulator::new();

    // The axis: the name. A published family's default group, in a case the
    // request did not store it in (the name "is stored as a lowercase string"),
    // and names no account holds — a family-shaped one the catalog does not
    // publish among them.
    let found: [(&str, &str); 4] = [
        ("default.redis7", "default.redis7"),
        ("default.memcached1.4", "default.memcached1.4"),
        ("DEFAULT.VALKEY8", "default.valkey8"),
        ("default.redis6.x", "default.redis6.x"),
    ];
    for (requested, expected) in found {
        let reply = describe_groups(&emulator, Some(requested)).await;
        assert_eq!(
            reply.status, 200,
            "case {requested}: must succeed, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reported_names(&reply),
            vec![expected],
            "case {requested}: expected only {expected}, got {}",
            reply.body
        );
        assert_eq!(
            reply.marker(),
            None,
            "case {requested}: a single-group lookup pages nothing, got {}",
            reply.body
        );
    }

    let missing = [
        "no-such-group",
        // A family the published catalog does not carry, so its default group
        // does not exist either.
        "default.redis9",
        "default.memcached2.0",
        "default.redis6",
        "default.",
    ];
    for name in missing {
        let reply = describe_groups(&emulator, Some(name)).await;
        assert_eq!(
            reply.status, 404,
            "case {name}: must be refused 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "CacheParameterGroupNotFound",
            "case {name}: expected CacheParameterGroupNotFound, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn created_groups_are_reported_and_pages_are_bounded_and_resumable() {
    let emulator = Emulator::new();

    // 25 groups beyond the published defaults, so the store holds more than the
    // smallest page size the service serves. They go in through
    // `CreateCacheParameterGroup`, so what the describe pages is what another
    // operation persisted — a describe that reported a fixed catalog instead of
    // the store would miss every one of them.
    let custom: Vec<String> = (0..25).map(|index| format!("custom-{index:02}")).collect();
    for name in &custom {
        let created = create_group(&emulator, name, "redis7", &format!("group {name}"), &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name}: create must succeed, got {} {}",
            created.status, created.body
        );
    }
    let total = custom.len() + published_default_groups().len();

    // Each row is a page size: the published bounds of "minimum 20; maximum
    // 100", one past each, and the default when the member is absent.
    let cases: [(&str, Option<usize>); 6] = [
        ("20", Some(20)),
        ("21", Some(21)),
        ("100", Some(total.min(100))),
        ("19", None),
        ("101", None),
        ("0", None),
    ];
    for (max_records, accepted) in cases {
        let reply = emulator
            .call(
                "DescribeCacheParameterGroups",
                &params(&[("MaxRecords", max_records)]),
            )
            .await;
        match accepted {
            Some(expected) => {
                assert_eq!(
                    reply.status, 200,
                    "case MaxRecords={max_records}: must be accepted, got {} {}",
                    reply.status, reply.body
                );
                let page = reported_names(&reply);
                assert_eq!(
                    page.len(),
                    expected,
                    "case MaxRecords={max_records}: expected {expected} groups, got {page:?}"
                );
                assert_eq!(
                    reply.marker().is_some(),
                    total > expected,
                    "case MaxRecords={max_records}: a marker is present exactly when more \
                     records remain, got {}",
                    reply.body
                );
            }
            None => {
                assert_eq!(
                    reply.status, 400,
                    "case MaxRecords={max_records}: must be refused 400, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "InvalidParameterValue",
                    "case MaxRecords={max_records}: expected InvalidParameterValue, got {}",
                    reply.body
                );
            }
        }
    }

    // A `Marker` this service did not mint is refused rather than silently
    // serving the first page.
    for token in ["not-a-marker", "zz"] {
        let bogus = emulator
            .call(
                "DescribeCacheParameterGroups",
                &params(&[("Marker", token)]),
            )
            .await;
        assert_eq!(
            bogus.status, 400,
            "case Marker={token}: must be refused 400, got {} {}",
            bogus.status, bogus.body
        );
        assert_eq!(
            bogus.error_code(),
            "InvalidParameterValue",
            "case Marker={token}: expected InvalidParameterValue, got {}",
            bogus.body
        );
    }

    // Walking the pages visits every stored group exactly once, in one stable
    // order, and ends without a marker.
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut pairs = params(&[("MaxRecords", "20")]);
        if let Some(token) = marker.as_deref() {
            pairs.extend(params(&[("Marker", token)]));
        }
        let reply = emulator.call("DescribeCacheParameterGroups", &pairs).await;
        assert_eq!(
            reply.status, 200,
            "case page {pages}: must succeed, got {} {}",
            reply.status, reply.body
        );
        seen.extend(reported_names(&reply).into_iter().map(str::to_string));
        pages += 1;
        assert!(
            pages <= 5,
            "case walk: {total} groups at 20 per page must not take more than 5 pages, \
             got {pages}"
        );
        match reply.marker() {
            Some(token) => marker = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen.len(),
        total,
        "case walk: expected every one of the {total} groups exactly once, got {seen:?}"
    );
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        total,
        "case walk: the pages must not repeat a group, got {seen:?}"
    );
    assert_eq!(
        seen, sorted,
        "case walk: the pages must follow one stable order, got {seen:?}"
    );
    for name in &custom {
        assert!(
            seen.contains(name),
            "case walk: expected the created group {name} to be reported, got {seen:?}"
        );
    }

    // A created group is also reachable by name, and reports what the record
    // holds.
    let one = describe_groups(&emulator, Some("custom-07")).await;
    assert_eq!(
        one.status, 200,
        "case custom-07: must succeed, got {} {}",
        one.status, one.body
    );
    assert_eq!(
        one.text("Description"),
        Some("group custom-07"),
        "case custom-07: the description must come from the stored record, got {}",
        one.body
    );
    assert_eq!(
        one.text("CacheParameterGroupFamily"),
        Some("redis7"),
        "case custom-07: the family must come from the stored record, got {}",
        one.body
    );
}

/// One row of the create / round-trip table.
#[derive(Debug, Clone)]
struct GroupCase {
    /// The `CacheParameterGroupName` as the request sends it.
    sent_name: &'static str,
    /// The identity the group is stored and reported under:
    /// `CacheParameterGroupName` "is stored as a lowercase string".
    stored_name: &'static str,
    family: &'static str,
    /// `Description` as the request sends it.
    description: &'static str,
    /// What the group REPORTS for it: the value as sent, byte for byte, except
    /// that a blank description is normalised to a single space
    /// (`state::BLANK_DESCRIPTION`).
    reported_description: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_created_parameter_group_is_read_back_from_persisted_state() {
    // The axis: the family (one per engine line, including families the issue
    // did not cite), the case of the name, what the description holds (words, an
    // EMPTY string, non-ASCII text) and how many tags come with it.
    let cases = [
        GroupCase {
            sent_name: "my-group",
            stored_name: "my-group",
            family: "redis7",
            description: "A Redis OSS 7 group",
            reported_description: "A Redis OSS 7 group",
            tags: &[("env", "prod")],
        },
        GroupCase {
            sent_name: "Mixed-Case-Group",
            stored_name: "mixed-case-group",
            family: "memcached1.4",
            description: "The oldest published Memcached line",
            reported_description: "The oldest published Memcached line",
            tags: &[],
        },
        GroupCase {
            sent_name: "empty-description",
            stored_name: "empty-description",
            family: "redis6.x",
            // An explicit empty description is a value, not an absent member,
            // and the service normalises a blank one to a single space — which
            // is what every later read reports.
            description: "",
            reported_description: state::BLANK_DESCRIPTION,
            tags: &[],
        },
        GroupCase {
            sent_name: "blank-description",
            stored_name: "blank-description",
            family: "redis7",
            // Whitespace is blank too, and normalises to the same single space.
            description: "   ",
            reported_description: state::BLANK_DESCRIPTION,
            tags: &[],
        },
        GroupCase {
            sent_name: "unicode-description",
            stored_name: "unicode-description",
            family: "valkey8",
            description: "キャッシュ · grupo de parámetros · 🧊",
            reported_description: "キャッシュ · grupo de parámetros · 🧊",
            tags: &[("ключ", "значение")],
        },
        GroupCase {
            sent_name: "two-tags",
            stored_name: "two-tags",
            family: "valkey7",
            // A tag with an empty value is accepted ("although null is
            // accepted"), and neither tag changes what the group reports.
            description: "Two tags, one of them valueless",
            reported_description: "Two tags, one of them valueless",
            tags: &[("a", "1"), ("b", "")],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let created = create_group(
            &emulator,
            case.sent_name,
            case.family,
            case.description,
            case.tags,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: create must answer 200, got {} {}",
            created.status, created.body
        );

        // The response is the persisted group, and so is every later read of
        // it — by the name as sent, and by the lowercase identity it is stored
        // under.
        let sent_lookup = describe_groups(&emulator, Some(case.sent_name)).await;
        let stored_lookup = describe_groups(&emulator, Some(case.stored_name)).await;
        for (reader, reply) in [
            ("create", &created),
            ("describe by the name as sent", &sent_lookup),
            ("describe by the stored name", &stored_lookup),
        ] {
            let case_label = format!("{case:?} via {reader}");
            assert_eq!(
                reply.status, 200,
                "case {case_label}: must answer 200, got {} {}",
                reply.status, reply.body
            );
            assert_member(
                reply,
                &case_label,
                "CacheParameterGroup/CacheParameterGroupName",
                case.stored_name,
            );
            assert_member(
                reply,
                &case_label,
                "CacheParameterGroup/CacheParameterGroupFamily",
                case.family,
            );
            assert_member(
                reply,
                &case_label,
                "CacheParameterGroup/Description",
                case.reported_description,
            );
            // "Indicates whether the parameter group is associated with a
            // Global datastore": nothing here associates one.
            assert_member(reply, &case_label, "CacheParameterGroup/IsGlobal", "false");
            assert_member(
                reply,
                &case_label,
                "CacheParameterGroup/ARN",
                &expected_arn(REGION, case.stored_name),
            );
        }
    }

    // Every row is still there afterwards, beside the published family
    // defaults: a create persisted a group, it did not replace the store.
    let listed = emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("MaxRecords", "100")]),
        )
        .await;
    let names = reported_names(&listed);
    for case in &cases {
        assert!(
            names.contains(&case.stored_name),
            "case {case:?}: the group must be listed, got {names:?}"
        );
    }
    assert_eq!(
        names.len(),
        published_default_groups().len() + cases.len(),
        "the listing must hold every published default plus every created group, got {names:?}"
    );
}

#[tokio::test]
async fn a_create_is_refused_without_persisting_anything() {
    let emulator = Emulator::new();

    // The group every duplicate row collides with.
    let first = create_group(&emulator, "taken", "redis7", "The first one", &[]).await;
    assert_eq!(
        first.status, 200,
        "the group a duplicate collides with must be created, got {}",
        first.body
    );

    // Each row: the case, the name, the family, and the status plus awsQuery
    // error code the request must be refused with.
    let cases: [(&str, &str, &str, u16, &str); 9] = [
        // A group has to be named to be addressable later.
        ("empty name", "", "redis7", 400, "InvalidParameterValue"),
        // Only the published families exist: `redis6` is not one of them
        // (`redis6.x` is), and neither is a version line AWS never shipped.
        (
            "a version line that is no family",
            "bad-family",
            "redis6",
            400,
            "InvalidParameterValue",
        ),
        (
            "a valkey line that is no family",
            "bad-family",
            "valkey10",
            400,
            "InvalidParameterValue",
        ),
        (
            "a memcached line that is no family",
            "bad-family",
            "memcached1.7",
            400,
            "InvalidParameterValue",
        ),
        (
            "no family at all",
            "bad-family",
            "",
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine with no version line",
            "bad-family",
            "redis",
            400,
            "InvalidParameterValue",
        ),
        // The name is taken — by a custom group, or by a family default the
        // store comes up holding.
        (
            "a name already created",
            "taken",
            "redis7",
            400,
            "CacheParameterGroupAlreadyExists",
        ),
        (
            "the name of a family default",
            "default.redis7",
            "redis7",
            400,
            "CacheParameterGroupAlreadyExists",
        ),
        // The `default.` namespace belongs to the published defaults.
        (
            "a reserved default name",
            "default.mine",
            "redis7",
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, name, family, status, code) in cases {
        let refused = create_group(&emulator, name, family, "A refused group", &[]).await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            refused.body
        );
    }

    // `Tag$Key` "may not be null" while the value may be, so a keyless tag is
    // refused rather than dropped.
    let keyless = emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", "keyless-tag"),
                ("CacheParameterGroupFamily", "redis7"),
                ("Description", "A keyless tag"),
                ("Tags.Tag.1.Value", "orphan"),
            ]),
        )
        .await;
    assert_eq!(
        keyless.status, 400,
        "case keyless tag: must be refused 400, got {} {}",
        keyless.status, keyless.body
    );
    assert_eq!(
        keyless.error_code(),
        "InvalidParameterValue",
        "case keyless tag: expected InvalidParameterValue, got {}",
        keyless.body
    );

    // "The maximum number of tags permitted on a resource is 50": the bound is a
    // row and so is one past it.
    for (count, accepted) in [
        (state::MAX_TAGS_PER_RESOURCE, true),
        (state::MAX_TAGS_PER_RESOURCE + 1, false),
    ] {
        let tags: Vec<(String, String)> = (1..=count)
            .map(|index| (format!("k{index}"), format!("v{index}")))
            .collect();
        let borrowed: Vec<(&str, &str)> = tags
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let reply = create_group(
            &emulator,
            &format!("tags-{count}"),
            "redis7",
            "Tagged",
            &borrowed,
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {count} tags: must be accepted, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {count} tags: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "TagQuotaPerResourceExceeded",
                "case {count} tags: expected TagQuotaPerResourceExceeded, got {}",
                reply.body
            );
        }
    }

    // Nothing a refused create named was persisted: only the group that was
    // accepted, plus the tag row that was.
    for name in [
        "bad-family",
        "default.mine",
        "keyless-tag",
        &format!("tags-{}", state::MAX_TAGS_PER_RESOURCE + 1),
    ] {
        let described = describe_groups(&emulator, Some(name)).await;
        assert_eq!(
            described.status, 404,
            "case {name}: a refused create must persist nothing, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "CacheParameterGroupNotFound",
            "case {name}: expected CacheParameterGroupNotFound, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_cluster_joins_a_group_the_account_holds_and_nothing_else() {
    let emulator = Emulator::new();

    // A group this account created is one a cluster may join, and so is a
    // published family's default group.
    for (name, family) in [("cluster-group", "redis7"), ("later-group", "redis7")] {
        let created = create_group(&emulator, name, family, "For a cluster", &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name}: create must answer 200, got {}",
            created.body
        );
    }

    // The axis: the `CacheParameterGroupName` a create names — a created group
    // and a family default, in a case the store does not hold them in; the
    // cluster reports the name the group is STORED as.
    let accepted: [(&str, &str, &str); 5] = [
        ("joins-own-group", "Cluster-Group", "cluster-group"),
        ("joins-redis7", "default.redis7", "default.redis7"),
        ("joins-redis6", "default.redis6.x", "default.redis6.x"),
        ("joins-upper", "DEFAULT.REDIS7", "default.redis7"),
        ("joins-memcached", "default.memcached1.6", "default.memcached1.6"),
    ];
    for (id, requested, stored) in accepted {
        let mut pairs = crate::harness::cluster_create_params(
            id,
            &[("CacheParameterGroupName", requested)],
        );
        // A memcached family only fits a memcached cluster's engine; the group a
        // cluster joins is not checked against its engine here, but keeping the
        // row's engine with its family keeps the row realistic.
        if requested.contains("memcached") {
            pairs.extend(params(&[("Engine", "memcached")]));
        }
        let created = emulator.call("CreateCacheCluster", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {requested}: create must succeed, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("CacheParameterGroup/CacheParameterGroupName"),
            Some(stored),
            "case {requested}: expected the cluster to join {stored}, got {}",
            created.body
        );
        // The group the cluster joined is one this account holds.
        let described = describe_groups(&emulator, Some(stored)).await;
        assert_eq!(
            described.status, 200,
            "case {requested}: {stored} must be a group the account holds, got {} {}",
            described.status, described.body
        );
    }

    for requested in ["default.redis9", "nope", "never-created"] {
        let refused = emulator
            .call(
                "CreateCacheCluster",
                &crate::harness::cluster_create_params(
                    "rejected",
                    &[("CacheParameterGroupName", requested)],
                ),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {requested}: create must be refused 404, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "CacheParameterGroupNotFound",
            "case {requested}: expected CacheParameterGroupNotFound, got {}",
            refused.body
        );
    }

    // A create that names NO parameter group joins the default group of its
    // engine version's family, resolved through the same stored map — so an
    // engine version no published family covers has no group to join and is
    // refused rather than the cluster reporting a group the account does not
    // hold.
    let derived: [(&str, &str, Option<&str>); 4] = [
        ("derived-redis6", "6.2", Some("default.redis6.x")),
        ("derived-redis7", "7.1", Some("default.redis7")),
        ("derived-redis40", "4.0.10", Some("default.redis4.0")),
        ("derived-unsupported", "9.9", None),
    ];
    for (id, version, expected) in derived {
        let created = emulator
            .call(
                "CreateCacheCluster",
                &crate::harness::cluster_create_params(id, &[("EngineVersion", version)]),
            )
            .await;
        match expected {
            Some(group) => {
                assert_eq!(
                    created.status, 200,
                    "case EngineVersion={version}: create must succeed, got {} {}",
                    created.status, created.body
                );
                assert_eq!(
                    created.text("CacheParameterGroup/CacheParameterGroupName"),
                    Some(group),
                    "case EngineVersion={version}: expected the cluster to join {group}, got {}",
                    created.body
                );
                let described = describe_groups(&emulator, Some(group)).await;
                assert_eq!(
                    described.status, 200,
                    "case EngineVersion={version}: {group} must be a group the account holds, \
                     got {} {}",
                    described.status, described.body
                );
            }
            None => {
                assert_eq!(
                    created.status, 400,
                    "case EngineVersion={version}: create must be refused 400, got {} {}",
                    created.status, created.body
                );
                // A version the engine's published catalogue does not OFFER is refused for
                // being absent from it, which is the combination refusal AWS answers —
                // before the derived default group it has no family for is looked for.
                assert_eq!(
                    created.error_code(),
                    "InvalidParameterCombination",
                    "case EngineVersion={version}: expected InvalidParameterCombination, got {}",
                    created.body
                );
                assert_eq!(
                    created.error_message(),
                    format!("Cannot find version {version} for redis"),
                    "case EngineVersion={version}: expected the version-not-offered sentence, \
                     got {:?}",
                    created.error_message()
                );
            }
        }
    }

    // The same rule on the modify path: a created group is accepted there too,
    // and the cluster reports the group it moved to. A modify is only accepted
    // once the cluster has settled, so this waits for THAT condition rather
    // than for a span of time.
    poll_until(
        &emulator,
        REGION,
        "joins-own-group",
        "the cluster settling at available",
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", "joins-own-group")]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    let modified = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "joins-own-group"),
                ("CacheParameterGroupName", "Later-Group"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "a modify naming a created group must be accepted, got {} {}",
        modified.status, modified.body
    );
    let described = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", "joins-own-group")]),
        )
        .await;
    assert_eq!(
        described.text("CacheParameterGroup/CacheParameterGroupName"),
        Some("later-group"),
        "the cluster must report the group the modify moved it to, got {}",
        described.body
    );
}

#[tokio::test]
async fn an_arn_names_the_region_the_request_was_signed_for() {
    // A group AWS provides exists in every Region and a custom group is created
    // in one, so the ARN a read reports names the Region the request was signed
    // for. The axis: that Region.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::new();
        let name = format!("group-{region}");
        let created = create_group_in(&emulator, region, &name, "redis7", "Regional", &[]).await;
        assert_eq!(
            created.status, 200,
            "case {region}: create must answer 200, got {}",
            created.body
        );
        assert_member(
            &created,
            region,
            "CacheParameterGroup/ARN",
            &expected_arn(region, &name),
        );
        let described = describe_group_in(&emulator, region, &name).await;
        assert_member(
            &described,
            region,
            "CacheParameterGroup/ARN",
            &expected_arn(region, &name),
        );

        let default = describe_group_in(&emulator, region, "default.redis7").await;
        assert_member(
            &default,
            region,
            "CacheParameterGroup/ARN",
            &expected_arn(region, "default.redis7"),
        );
    }
}

#[tokio::test]
async fn concurrent_creates_of_one_name_leave_exactly_one_group() {
    let emulator = Emulator::new();

    // Four creates of the same name, in flight together: the name can only be
    // free for one of them.
    let (a, b, c, d) = tokio::join!(
        create_group(&emulator, "contended", "redis7", "racer a", &[]),
        create_group(&emulator, "contended", "redis7", "racer b", &[]),
        create_group(&emulator, "contended", "redis7", "racer c", &[]),
        create_group(&emulator, "contended", "redis7", "racer d", &[]),
    );
    let replies = [a, b, c, d];
    let successes: Vec<&Reply> = replies.iter().filter(|reply| reply.status == 200).collect();
    assert_eq!(
        successes.len(),
        1,
        "exactly one concurrent create may succeed, got {:?}",
        replies
            .iter()
            .map(|reply| (reply.status, reply.error_code().to_string()))
            .collect::<Vec<_>>()
    );
    for refused in replies.iter().filter(|reply| reply.status != 200) {
        assert_eq!(
            refused.status, 400,
            "case loser: must be refused 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "CacheParameterGroupAlreadyExists",
            "case loser: expected CacheParameterGroupAlreadyExists, got {}",
            refused.body
        );
    }

    // And the store holds ONE group of that name, the one the winner created.
    let listed = emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("MaxRecords", "100")]),
        )
        .await;
    let names = reported_names(&listed);
    assert_eq!(
        names.iter().filter(|name| **name == "contended").count(),
        1,
        "the store must hold exactly one group of the contended name, got {names:?}"
    );
    let described = describe_groups(&emulator, Some("contended")).await;
    assert_eq!(
        described.text("CacheParameterGroup/Description"),
        successes[0].text("CacheParameterGroup/Description"),
        "the stored group must be the one the winning create reported, got {}",
        described.body
    );
}
