// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateGlobalReplicationGroup`.
//!
//! Every assertion goes over the wire through [`crate::harness`], so only the
//! HTTP status and the XML body are inspected and a response that is not
//! computed from persisted state cannot pass: each row creates the primary
//! replication group the way a client does, mints the Global datastore around
//! it, and reads the datastore back through `DescribeGlobalReplicationGroups`
//! and the primary back through `DescribeReplicationGroups`.
//!
//! The tests are tables. The axis of the first is the PRIMARY the datastore is
//! built around — its engine, its engine version, its node type, how many shards
//! its keyspace is partitioned over, whether it has automatic failover, and
//! which of its encryption settings are on — because a Global datastore has the
//! configuration of the group it is built around, so one primary is only one
//! row. The axis of the second is the REGION the create was addressed to, which
//! is what decides the prefix the stored id carries. The axis of the third is
//! the way a create is refused.

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION, REGION};

/// One create / round-trip row: a primary replication group, and the Global
/// datastore built around it.
#[derive(Debug, Clone)]
struct Case {
    /// `GlobalReplicationGroupIdSuffix`, as the request sends it.
    suffix: &'static str,
    /// The primary replication group the datastore is built around.
    primary: &'static str,
    /// The form pairs that create that primary, beside its id and description.
    primary_extra: &'static [(&'static str, &'static str)],
    /// `GlobalReplicationGroupDescription`, or `None` when the create sends
    /// none.
    description: Option<&'static str>,
    /// The id the datastore is stored and reported under: the region's prefix
    /// and the suffix, lowercased.
    stored_id: &'static str,
    engine: &'static str,
    engine_version: &'static str,
    cache_node_type: &'static str,
    cluster_enabled: &'static str,
    /// The keyspace of each slice group, in the order they are reported: one per
    /// shard of the primary.
    slots: &'static [&'static str],
    /// What the primary member reports for `AutomaticFailover`.
    automatic_failover: &'static str,
    auth_token_enabled: &'static str,
    transit_encryption: &'static str,
    at_rest_encryption: &'static str,
}

/// Create the primary replication group of `case` and let the clock settle it at
/// `available`, which is the state a datastore can be built around.
async fn create_primary(emulator: &Emulator, case: &Case) {
    let mut pairs = params(&[
        ("ReplicationGroupId", case.primary),
        (
            "ReplicationGroupDescription",
            "the global datastore suite's primary",
        ),
    ]);
    pairs.extend(params(case.primary_extra));
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: the primary {} must be creatable, got HTTP {} {}",
        case.suffix, case.primary, created.status, created.body
    );
    std::thread::sleep(PAST_TRANSITION);
}

/// Mint the Global datastore of `case`, in `region`.
async fn create_datastore(emulator: &Emulator, region: &str, case: &Case) -> Reply {
    let mut pairs = params(&[
        ("GlobalReplicationGroupIdSuffix", case.suffix),
        ("PrimaryReplicationGroupId", case.primary),
    ]);
    if let Some(description) = case.description {
        pairs.extend(params(&[("GlobalReplicationGroupDescription", description)]));
    }
    emulator
        .call_in(region, "CreateGlobalReplicationGroup", &pairs)
        .await
}

/// Every text node a reply carries about the datastore itself: the whole body
/// apart from the `ResponseMetadata` envelope, whose request id is minted per
/// call.
fn datastore_nodes(reply: &Reply) -> Vec<(String, String)> {
    reply
        .nodes
        .iter()
        .filter(|(path, _)| !path.contains("ResponseMetadata"))
        .cloned()
        .collect()
}

/// Assert `reply` reports the datastore of `case`, whatever call answered it.
fn assert_reports_case(case: &Case, where_from: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {:?} ({where_from}): expected HTTP 200, got {} {}",
        case.suffix, reply.status, reply.body
    );
    for (member, expected) in [
        ("GlobalReplicationGroupId", Some(case.stored_id)),
        ("GlobalReplicationGroupDescription", case.description),
        ("Engine", Some(case.engine)),
        ("EngineVersion", Some(case.engine_version)),
        ("CacheNodeType", Some(case.cache_node_type)),
        ("ClusterEnabled", Some(case.cluster_enabled)),
        ("AuthTokenEnabled", Some(case.auth_token_enabled)),
        ("TransitEncryptionEnabled", Some(case.transit_encryption)),
        ("AtRestEncryptionEnabled", Some(case.at_rest_encryption)),
    ] {
        assert_eq!(
            reply.text(member),
            expected,
            "case {:?} ({where_from}): expected {member} {expected:?}, got {:?} in {}",
            case.suffix,
            reply.text(member),
            reply.body
        );
    }
    // The datastore's ARN names the datastore, in the region it was created in.
    let arn = reply.text("ARN").unwrap_or_default();
    assert!(
        arn.ends_with(&format!(":globalreplicationgroup:{}", case.stored_id))
            && arn.contains(&format!(":{REGION}:")),
        "case {:?} ({where_from}): expected an ARN of the datastore in {REGION}, got {arn:?} in {}",
        case.suffix,
        reply.body
    );
    // One slice group per shard of the primary, each with its own keyspace.
    let slots = reply.all("Slots");
    assert_eq!(
        slots, case.slots,
        "case {:?} ({where_from}): expected the slice groups {:?}, got {slots:?} in {}",
        case.suffix, case.slots, reply.body
    );
    let node_group_ids: Vec<String> = (1..=case.slots.len())
        .map(|ordinal| format!("{}-{ordinal:04}", case.stored_id))
        .collect();
    assert_eq!(
        reply.all("GlobalNodeGroupId"),
        node_group_ids,
        "case {:?} ({where_from}): expected the slice group ids {node_group_ids:?}, got {:?} in {}",
        case.suffix,
        reply.all("GlobalNodeGroupId"),
        reply.body
    );
    // The one member a just-created datastore has: the primary it was built
    // around, associated, in the region the create was addressed to.
    assert_eq!(
        (
            reply.all("ReplicationGroupId"),
            reply.all("Role"),
            reply.all("ReplicationGroupRegion"),
            reply.all("AutomaticFailover"),
        ),
        (
            vec![case.primary],
            vec!["PRIMARY"],
            vec![REGION],
            vec![case.automatic_failover],
        ),
        "case {:?} ({where_from}): expected the one primary member {}, got {}",
        case.suffix,
        case.primary,
        reply.body
    );
}

#[tokio::test]
async fn a_created_global_datastore_is_read_back_from_persisted_state() {
    // The axis: the primary the datastore is built around. Each row names a
    // different engine, engine version, node type, keyspace partition, failover
    // setting and encryption pair, and the datastore has to report the primary's
    // own — the create's request carries no member for any of them.
    let cases = [
        Case {
            suffix: "single-shard-ds",
            primary: "primary-single",
            primary_extra: &[
                ("Engine", "redis"),
                ("EngineVersion", "7.0"),
                ("CacheNodeType", "cache.t3.micro"),
                ("NumCacheClusters", "2"),
            ],
            description: Some("the single-shard datastore"),
            stored_id: "ldgnf-single-shard-ds",
            engine: "redis",
            engine_version: "7.0",
            cache_node_type: "cache.t3.micro",
            cluster_enabled: "false",
            slots: &["0-16383"],
            // "Default: false" for `AutomaticFailoverEnabled`.
            automatic_failover: "disabled",
            auth_token_enabled: "false",
            transit_encryption: "false",
            at_rest_encryption: "false",
        },
        Case {
            // A suffix is lowercased into the stored id, the way every other
            // identifier of this service is.
            suffix: "Mixed-Case-DS",
            primary: "primary-sharded",
            primary_extra: &[
                ("Engine", "valkey"),
                ("EngineVersion", "8.0"),
                ("CacheNodeType", "cache.m7g.large"),
                ("NumNodeGroups", "3"),
                ("ReplicasPerNodeGroup", "1"),
            ],
            // A create that names no description leaves the datastore without
            // one rather than inventing the primary's.
            description: None,
            stored_id: "ldgnf-mixed-case-ds",
            engine: "valkey",
            engine_version: "8.0",
            cache_node_type: "cache.m7g.large",
            cluster_enabled: "true",
            slots: &["0-5461", "5462-10922", "10923-16383"],
            automatic_failover: "enabled",
            auth_token_enabled: "false",
            transit_encryption: "false",
            // "true when using Valkey": the engine's documented default.
            at_rest_encryption: "true",
        },
        Case {
            suffix: "encrypted-ds",
            primary: "primary-encrypted",
            primary_extra: &[
                ("Engine", "redis"),
                ("EngineVersion", "7.1"),
                ("CacheNodeType", "cache.r7g.large"),
                ("NumCacheClusters", "2"),
                ("AutomaticFailoverEnabled", "true"),
                ("TransitEncryptionEnabled", "true"),
                ("AtRestEncryptionEnabled", "true"),
                ("AuthToken", "a-sixteen-plus-character-token"),
            ],
            description: Some("the encrypted datastore"),
            stored_id: "ldgnf-encrypted-ds",
            engine: "redis",
            engine_version: "7.1",
            cache_node_type: "cache.r7g.large",
            cluster_enabled: "false",
            slots: &["0-16383"],
            automatic_failover: "enabled",
            auth_token_enabled: "true",
            transit_encryption: "true",
            at_rest_encryption: "true",
        },
        // The suffix AT the bound: the stored id is the region's prefix, a
        // hyphen and the suffix, so the longest suffix this service accepts is
        // its identifier bound less those six characters — one past it is a row
        // of the refusal table.
        Case {
            suffix: "a-suffix-of-exactly-thirty-four-ch",
            primary: "primary-longest-suffix",
            primary_extra: &[
                ("Engine", "redis"),
                ("EngineVersion", "6.2"),
                ("CacheNodeType", "cache.t4g.medium"),
            ],
            description: None,
            stored_id: "ldgnf-a-suffix-of-exactly-thirty-four-ch",
            engine: "redis",
            engine_version: "6.2",
            cache_node_type: "cache.t4g.medium",
            cluster_enabled: "false",
            slots: &["0-16383"],
            automatic_failover: "disabled",
            auth_token_enabled: "false",
            transit_encryption: "false",
            at_rest_encryption: "false",
        },
    ];
    assert_eq!(
        cases[3].suffix.len(),
        34,
        "the at-the-bound row must carry the longest suffix this service accepts"
    );

    for case in &cases {
        let emulator = Emulator::new();
        create_primary(&emulator, case).await;

        let created = create_datastore(&emulator, REGION, case).await;
        assert_reports_case(case, "the create", &created);
        // A created datastore is not ready at the call: it answers `creating`
        // until the clock owes it the status it settles at, which for a
        // datastore that has only its primary is `primary-only`.
        assert_eq!(
            created.text("Status"),
            Some("creating"),
            "case {:?}: a just-created datastore must answer creating, got {:?} in {}",
            case.suffix,
            created.text("Status"),
            created.body
        );

        std::thread::sleep(PAST_TRANSITION);
        // The round trip: everything the create answered comes back out of the
        // store, through the reader a client polls.
        let described = emulator
            .call(
                "DescribeGlobalReplicationGroups",
                &params(&[
                    ("GlobalReplicationGroupId", case.stored_id),
                    ("ShowMemberInfo", "true"),
                ]),
            )
            .await;
        assert_reports_case(case, "the describe", &described);
        assert_eq!(
            described.text("Status"),
            Some("primary-only"),
            "case {:?}: a settled datastore with only its primary must answer primary-only, got \
             {:?} in {}",
            case.suffix,
            described.text("Status"),
            described.body
        );

        // A describe is a READ: a second one sees exactly what the first did,
        // apart from the per-call request id the envelope carries.
        let again = emulator
            .call(
                "DescribeGlobalReplicationGroups",
                &params(&[
                    ("GlobalReplicationGroupId", case.stored_id),
                    ("ShowMemberInfo", "true"),
                ]),
            )
            .await;
        assert_eq!(
            datastore_nodes(&again),
            datastore_nodes(&described),
            "case {:?}: two describes of the same datastore must agree, got {} then {}",
            case.suffix,
            described.body,
            again.body
        );

        // The membership is held once per record: the primary reports the
        // datastore it now belongs to and the role it has there.
        let primary = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", case.primary)]),
            )
            .await;
        assert_eq!(
            (
                primary.status,
                primary.text("GlobalReplicationGroupId"),
                primary.text("GlobalReplicationGroupMemberRole"),
            ),
            (200, Some(case.stored_id), Some("PRIMARY")),
            "case {:?}: the primary must report its Global datastore membership, got {}",
            case.suffix, primary.body
        );
    }
}

#[tokio::test]
async fn a_global_datastore_id_carries_the_prefix_of_the_region_it_was_created_in() {
    // The axis: the region the create was addressed to. "Each Amazon Region has
    // its own prefix … The suffix, combined with the auto-generated prefix,
    // guarantees uniqueness of the Global datastore name across multiple
    // regions." Two of these prefixes this service's own sources name; the rest
    // only have to satisfy the published rule.
    let regions = [
        ("us-east-1", Some("ldgnf")),
        ("us-west-1", Some("dsdfu")),
        ("eu-west-2", None),
        ("ap-southeast-1", None),
    ];
    let suffix = "region-rule";
    let emulator = Emulator::new();
    let mut prefixes: Vec<String> = Vec::new();

    for (region, published) in regions {
        // Each region's datastore is built around its own primary, created in
        // that region.
        let primary = format!("primary-{region}");
        let created = emulator
            .call_in(
                region,
                "CreateReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", primary.as_str()),
                    ("ReplicationGroupDescription", "the region-rule primary"),
                    ("CacheNodeType", "cache.t3.micro"),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the primary must be creatable, got HTTP {} {}",
            created.status, created.body
        );
        std::thread::sleep(PAST_TRANSITION);

        let minted = emulator
            .call_in(
                region,
                "CreateGlobalReplicationGroup",
                &params(&[
                    ("GlobalReplicationGroupIdSuffix", suffix),
                    ("PrimaryReplicationGroupId", primary.as_str()),
                ]),
            )
            .await;
        assert_eq!(
            minted.status, 200,
            "case {region:?}: expected HTTP 200, got {} {}",
            minted.status, minted.body
        );
        let id = minted
            .text("GlobalReplicationGroupId")
            .unwrap_or_default()
            .to_string();
        let prefix = id
            .strip_suffix(&format!("-{suffix}"))
            .unwrap_or_else(|| {
                panic!("case {region:?}: expected an id ending in -{suffix}, got {id:?}")
            })
            .to_string();
        if let Some(published) = published {
            assert_eq!(
                prefix, published,
                "case {region:?}: expected the published prefix {published:?}, got {prefix:?}"
            );
        }
        assert!(
            prefix.len() == 5 && prefix.chars().all(|letter| letter.is_ascii_lowercase()),
            "case {region:?}: expected a five-letter lowercase prefix, got {prefix:?}"
        );

        // The id is stable: the datastore is readable back under exactly the id
        // the create answered with.
        let described = emulator
            .call_in(
                region,
                "DescribeGlobalReplicationGroups",
                &params(&[("GlobalReplicationGroupId", id.as_str())]),
            )
            .await;
        assert_eq!(
            (described.status, described.text("GlobalReplicationGroupId")),
            (200, Some(id.as_str())),
            "case {region:?}: the datastore must be readable back under {id:?}, got {}",
            described.body
        );

        // The same suffix in another region is another datastore, so the prefix
        // has to differ from every region before it.
        assert!(
            !prefixes.contains(&prefix),
            "case {region:?}: the prefix {prefix:?} is already used by another region ({prefixes:?})"
        );
        prefixes.push(prefix);
    }

    // The prefix is the REGION's, not the datastore's: a second datastore
    // created in the first region carries the same prefix as the first.
    let second_primary = "primary-second-suffix";
    let created = emulator
        .call_in(
            "us-east-1",
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", second_primary),
                ("ReplicationGroupDescription", "the second-suffix primary"),
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the second-suffix primary must be creatable, got HTTP {} {}",
        created.status, created.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let second = emulator
        .call_in(
            "us-east-1",
            "CreateGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupIdSuffix", "region-rule-again"),
                ("PrimaryReplicationGroupId", second_primary),
            ]),
        )
        .await;
    assert_eq!(
        (second.status, second.text("GlobalReplicationGroupId")),
        (200, Some("ldgnf-region-rule-again")),
        "a second suffix in us-east-1 must carry the same prefix, got {}",
        second.body
    );
}

/// One refusal row: the case name, the form pairs the create sends, and the HTTP
/// status plus `awsQuery` error code it must be answered with.
type Refusal = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

#[tokio::test]
async fn a_global_datastore_create_this_emulator_cannot_serve_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    // The primary the already-exists datastore is built around, and a second,
    // settled one for the rows whose refusal is NOT about the primary.
    let settled = Case {
        suffix: "refusals",
        primary: "primary-refusals",
        primary_extra: &[("CacheNodeType", "cache.t3.micro")],
        description: None,
        stored_id: "ldgnf-refusals",
        engine: "redis",
        engine_version: "",
        cache_node_type: "cache.t3.micro",
        cluster_enabled: "false",
        slots: &["0-16383"],
        automatic_failover: "disabled",
        auth_token_enabled: "false",
        transit_encryption: "false",
        at_rest_encryption: "false",
    };
    create_primary(&emulator, &settled).await;
    create_primary(
        &emulator,
        &Case {
            primary: "primary-spare",
            ..settled.clone()
        },
    )
    .await;
    // The datastore the already-exists and already-a-member rows need.
    let minted = create_datastore(&emulator, REGION, &settled).await;
    assert_eq!(
        minted.status, 200,
        "the datastore the refusal rows build on must be creatable, got {}",
        minted.body
    );

    // The axis: WHY the create cannot be served — an id the account holds no
    // group for, a group that is not available, a group that is already in a
    // datastore, a datastore name already taken, a suffix no identifier of this
    // service can carry (each shape of the rule: a leading digit, a leading
    // hyphen, two consecutive hyphens, a trailing hyphen, and one character past
    // the bound), and a tag key a caller may not write.
    const LONG_SUFFIX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert_eq!(
        LONG_SUFFIX.len(),
        35,
        "the over-long row must be one character past the bound the accepted table pins"
    );
    let rows: [Refusal; 9] = [
        (
            "an unknown primary",
            &[
                ("GlobalReplicationGroupIdSuffix", "unknown-primary"),
                ("PrimaryReplicationGroupId", "no-such-replication-group"),
            ],
            404,
            "ReplicationGroupNotFoundFault",
        ),
        (
            "a primary that is already a member",
            &[
                ("GlobalReplicationGroupIdSuffix", "second-datastore"),
                ("PrimaryReplicationGroupId", "primary-refusals"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a suffix already taken in this region",
            &[
                ("GlobalReplicationGroupIdSuffix", "refusals"),
                ("PrimaryReplicationGroupId", "primary-spare"),
            ],
            400,
            "GlobalReplicationGroupAlreadyExistsFault",
        ),
        (
            "a suffix that begins with a digit",
            &[
                ("GlobalReplicationGroupIdSuffix", "1-datastore"),
                ("PrimaryReplicationGroupId", "primary-spare"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a suffix that begins with a hyphen",
            &[
                ("GlobalReplicationGroupIdSuffix", "-datastore"),
                ("PrimaryReplicationGroupId", "primary-spare"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a suffix with two consecutive hyphens",
            &[
                ("GlobalReplicationGroupIdSuffix", "data--store"),
                ("PrimaryReplicationGroupId", "primary-spare"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a suffix that ends with a hyphen",
            &[
                ("GlobalReplicationGroupIdSuffix", "datastore-"),
                ("PrimaryReplicationGroupId", "primary-spare"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a suffix one character past the bound",
            &[
                ("GlobalReplicationGroupIdSuffix", LONG_SUFFIX),
                ("PrimaryReplicationGroupId", "primary-spare"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag key reserved for AWS",
            &[
                ("GlobalReplicationGroupIdSuffix", "reserved-tag"),
                ("PrimaryReplicationGroupId", "primary-spare"),
                ("Tags.Tag.1.Key", "aws:owner"),
                ("Tags.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, request, status, code) in rows {
        let reply = emulator
            .call("CreateGlobalReplicationGroup", &params(request))
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got HTTP {} {:?} — {}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        // A refused create stores nothing: the datastore the row named is not
        // readable afterwards. (The already-exists row names the one datastore
        // that IS stored, so it is the one row with nothing to check.)
        let suffix = request
            .iter()
            .find_map(|(key, value)| (*key == "GlobalReplicationGroupIdSuffix").then_some(*value))
            .expect("every row names a suffix");
        if suffix == settled.suffix {
            continue;
        }
        let id = format!("ldgnf-{}", suffix.to_ascii_lowercase());
        let described = emulator
            .call(
                "DescribeGlobalReplicationGroups",
                &params(&[("GlobalReplicationGroupId", id.as_str())]),
            )
            .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "GlobalReplicationGroupNotFoundFault"),
            "case {case:?}: a refused create must store nothing, got HTTP {} {}",
            described.status,
            described.body
        );
    }

    // The state gate, which needs a primary that is still being created: the
    // create of that group is followed straight away by the datastore create,
    // because a report of the group is answered only once the clock has settled
    // it, so a read between the two would wait it out of `creating`.
    let creating = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "primary-creating"),
                ("ReplicationGroupDescription", "still creating"),
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        )
        .await;
    assert_eq!(
        creating.status, 200,
        "the creating primary must be creatable, got {}",
        creating.body
    );
    let unsettled = emulator
        .call(
            "CreateGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupIdSuffix", "unsettled-primary"),
                ("PrimaryReplicationGroupId", "primary-creating"),
            ]),
        )
        .await;
    assert_eq!(
        (unsettled.status, unsettled.error_code()),
        (400, "InvalidReplicationGroupState"),
        "case \"a primary that is still creating\": expected HTTP 400 \
         InvalidReplicationGroupState, got HTTP {} {:?} — {}",
        unsettled.status,
        unsettled.error_code(),
        unsettled.body
    );
    let absent = emulator
        .call(
            "DescribeGlobalReplicationGroups",
            &params(&[("GlobalReplicationGroupId", "ldgnf-unsettled-primary")]),
        )
        .await;
    assert_eq!(
        (absent.status, absent.error_code()),
        (404, "GlobalReplicationGroupNotFoundFault"),
        "case \"a primary that is still creating\": a refused create must store nothing, got \
         HTTP {} {}",
        absent.status,
        absent.body
    );
}

#[tokio::test]
async fn a_global_datastore_create_carries_the_tags_a_resource_may_hold() {
    let emulator = Emulator::new();
    let case = Case {
        suffix: "tag-quota",
        primary: "primary-tag-quota",
        primary_extra: &[("CacheNodeType", "cache.t3.micro")],
        description: None,
        stored_id: "ldgnf-tag-quota",
        engine: "redis",
        engine_version: "",
        cache_node_type: "cache.t3.micro",
        cluster_enabled: "false",
        slots: &["0-16383"],
        automatic_failover: "disabled",
        auth_token_enabled: "false",
        transit_encryption: "false",
        at_rest_encryption: "false",
    };
    create_primary(&emulator, &case).await;

    // The axis: how many tags the create carries, one past the published bound
    // ("the maximum number of tags permitted on a resource is 50") and at it.
    // The refused row comes first, so the accepted one is still free to take
    // this suite's primary.
    for (count, expected) in [
        (51_usize, (400, "TagQuotaPerResourceExceeded")),
        (50, (200, "")),
    ] {
        let suffix = format!("tag-quota-{count}");
        let mut request = params(&[
            ("GlobalReplicationGroupIdSuffix", suffix.as_str()),
            ("PrimaryReplicationGroupId", case.primary),
        ]);
        for index in 1..=count {
            request.push((format!("Tags.Tag.{index}.Key"), format!("key-{index}")));
            request.push((format!("Tags.Tag.{index}.Value"), format!("value-{index}")));
        }
        let reply = emulator
            .call("CreateGlobalReplicationGroup", &request)
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            expected,
            "case {count} tags: expected HTTP {} {:?}, got HTTP {} {:?} — {}",
            expected.0,
            expected.1,
            reply.status,
            reply.error_code(),
            reply.body
        );
        if reply.status != 200 {
            continue;
        }
        // The accepted row's tags are the datastore's own: they come back
        // through the tagging surface, keyed by the ARN the create answered
        // with.
        let arn = reply
            .text("ARN")
            .expect("the create reports the datastore's ARN")
            .to_string();
        let listed = emulator
            .call(
                "ListTagsForResource",
                &params(&[("ResourceName", arn.as_str())]),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {count} tags: the datastore's tags must be listable, got {}",
            listed.body
        );
        let keys = listed.all("Key");
        assert_eq!(
            keys.len(),
            count,
            "case {count} tags: expected {count} tags on the datastore, got {keys:?}"
        );
    }
}
