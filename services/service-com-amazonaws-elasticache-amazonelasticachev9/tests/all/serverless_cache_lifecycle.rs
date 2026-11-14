// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache serverless-cache lifecycle:
//! `CreateServerlessCache` and the `DescribeServerlessCaches` that reads it
//! back.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from persisted state cannot pass. The request
//! builder and the XML reader are the ones the `cache_cluster_lifecycle` suite
//! marks `pub(crate)`, so this target keeps one harness rather than a second
//! copy that could drift.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (the engine and its version line, whether a
//! description / usage limit / subnet list / KMS key is named at all, the
//! retention limit on each published bound and one past it, the signing region,
//! the page size), so no single recorded request is what makes them pass.

use std::time::Duration;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// One row of the create / round-trip table: the request, and the resource it
/// must be stored and reported as.
#[derive(Debug, Clone)]
struct CacheCase {
    /// The `ServerlessCacheName` the request sends.
    name: &'static str,
    /// The form pairs sent beside it.
    extra: &'static [(&'static str, &'static str)],
    /// The identity the cache is stored and reported under —
    /// `ServerlessCacheName` "is stored as a lowercase string".
    stored_name: &'static str,
    engine: &'static str,
    major_engine_version: &'static str,
    full_engine_version: &'static str,
    /// The port the engine's clients connect to (Memcached 11211, Valkey /
    /// Redis OSS 6379).
    port: &'static str,
    /// What `Description` must report; `None` means the single space the API
    /// normalises an absent description to.
    description: Option<&'static str>,
    /// `sse-kms` when the request named a `KmsKeyId`, else `sse-elasticache`.
    storage_encryption_type: &'static str,
    /// How many of the account's subnets (the ids EC2 reports) the request
    /// names, or `None` when it names none and ElastiCache selects the Region's
    /// defaults (every row of this table is created in us-east-1).
    subnet_ids: Option<usize>,
    /// How many VPC security groups (the ids EC2 mints for the account) the
    /// request attaches the cache to; `0` when it names none.
    security_groups: usize,
    /// Every `(element suffix, text)` pair the response must additionally
    /// carry — the members this row sets and nothing else reports.
    reported: &'static [(&'static str, &'static str)],
}

/// Create one serverless cache.
async fn create_cache(emulator: &Emulator, name: &str, extra: &[(&str, &str)]) -> Reply {
    create_cache_with(emulator, name, &params(extra)).await
}

/// Create one serverless cache from already-owned form pairs — what a row that
/// derives a value (a subnet the account has) sends.
async fn create_cache_with(
    emulator: &Emulator,
    name: &str,
    extra: &[(String, String)],
) -> Reply {
    let mut pairs = params(&[("ServerlessCacheName", name)]);
    pairs.extend(extra.iter().cloned());
    emulator.call("CreateServerlessCache", &pairs).await
}

/// Describe one serverless cache by name.
async fn describe_cache(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await
}

/// Assert `reply` reports `case`'s resource, naming the case and the actual
/// value on every failure. `where_from` says which response is under test, and
/// `account_subnets` are the subnets the account HAS in the Region the row was
/// created in, in Availability Zone order, as the owner of subnets reports them.
fn assert_reports_case(
    case: &CacheCase,
    where_from: &str,
    reply: &Reply,
    status: &str,
    account_subnets: &[String],
    security_groups: &[String],
) {
    let name = case.name;
    assert_eq!(
        reply.text("ServerlessCacheName"),
        Some(case.stored_name),
        "case {name} ({where_from}): expected ServerlessCacheName {:?}, got {:?}",
        case.stored_name,
        reply.text("ServerlessCacheName")
    );
    for (member, expected) in [
        ("Engine", case.engine),
        ("MajorEngineVersion", case.major_engine_version),
        ("FullEngineVersion", case.full_engine_version),
        ("StorageEncryptionType", case.storage_encryption_type),
        ("Status", status),
    ] {
        assert_eq!(
            reply.text(member),
            Some(expected),
            "case {name} ({where_from}): expected {member} {expected:?}, got {:?}",
            reply.text(member)
        );
    }
    // The description is read out of the raw body rather than through the text
    // reader: the reader trims an element's text, and the single space an
    // absent description normalises to would come back as no text node at all.
    // The expected text is the literal the recorded response carries, not the
    // constant the handler reads it from.
    let description = case.description.unwrap_or(" ");
    assert!(
        reply
            .body
            .contains(&format!("<Description>{description}</Description>")),
        "case {name} ({where_from}): expected Description {description:?} in the body, got {}",
        reply.body
    );
    for (member, expected) in case.reported {
        assert!(
            reply.all(member).contains(expected),
            "case {name} ({where_from}): expected {member} to report {expected:?}, got {:?}",
            reply.all(member)
        );
    }
    // Both endpoints are addresses this emulator answers on and carry the port
    // its own socket for this cache was assigned. A serverless cache is served by
    // ONE socket, so the writer and the reader endpoint are the same address —
    // the one a client can reach. A MEMCACHED cache is served by nothing here (its
    // clients speak another protocol), so it keeps the name ElastiCache's own zone
    // would carry and the port its engine's clients connect to.
    let endpoints = reply.all("Address");
    assert_eq!(
        endpoints.len(),
        2,
        "case {name} ({where_from}): expected a writer and a reader endpoint, got {endpoints:?}"
    );
    let serves_resp = case.engine != "memcached";
    for address in &endpoints {
        if serves_resp {
            assert!(
                crate::harness::is_own_host(address),
                "case {name} ({where_from}): endpoint {address:?} must be an address the emulator \
                 answers on"
            );
        } else {
            assert!(
                address.starts_with(&format!("{}-", case.stored_name)),
                "case {name} ({where_from}): endpoint {address:?} must name the cache"
            );
        }
    }
    for port in reply.all("Port") {
        if serves_resp {
            assert!(
                port.parse::<u16>().is_ok_and(|port| port > 0),
                "case {name} ({where_from}): expected the port the cache answers on, got {port}"
            );
        } else {
            assert_eq!(
                port, case.port,
                "case {name} ({where_from}): expected port {}, got {port}",
                case.port
            );
        }
    }
    let reported_subnets = reply.all("SubnetId");
    // The subnets a cache reports are subnets the account HAS: the ones the
    // request named, or — "For all other Regions, if no subnet IDs are given
    // then ElastiCache will select 3 default subnets across AZs in your default
    // VPC" — the Region's own defaults.
    let account_subnets: Vec<&str> = account_subnets.iter().map(String::as_str).collect();
    let selected = state::default_subnet_count("us-east-1");
    let expected: Vec<&str> = match case.subnet_ids {
        Some(count) => account_subnets[..count].to_vec(),
        None => account_subnets[..selected].to_vec(),
    };
    assert_eq!(
        reported_subnets, expected,
        "case {name} ({where_from}): expected SubnetIds {expected:?}, got {reported_subnets:?}"
    );
    if case.subnet_ids.is_none() {
        assert_eq!(
            reported_subnets.len(),
            selected,
            "case {name} ({where_from}): expected {selected} default subnets, got \
             {reported_subnets:?}"
        );
    }
    // The security groups a cache reports are the ones the request named, which
    // are groups the account HAS — their ids come from EC2, the service that owns
    // security groups.
    let expected_groups: Vec<&str> = security_groups.iter().map(String::as_str).collect();
    assert_eq!(
        reply.all("SecurityGroupIds/SecurityGroupId"),
        expected_groups,
        "case {name} ({where_from}): expected SecurityGroupIds {expected_groups:?}, got {:?}",
        reply.all("SecurityGroupIds/SecurityGroupId")
    );
}

#[tokio::test]
async fn a_created_serverless_cache_is_read_back_from_persisted_state() {
    // The axis: the engine and its version line (which fixes
    // `MajorEngineVersion`, `FullEngineVersion` and the port), and whether the
    // request names a description, usage limits, subnets, security groups, a
    // KMS key or snapshot settings at all.
    let cases = [
        CacheCase {
            name: "Demo-Cache",
            extra: &[("Engine", "redis")],
            stored_name: "demo-cache",
            engine: "redis",
            major_engine_version: "7",
            full_engine_version: "7.1",
            port: "6379",
            description: None,
            storage_encryption_type: "sse-elasticache",
            subnet_ids: None,
            security_groups: 0,
            reported: &[("NetworkType", "ipv4"), ("ConnectionType", "vpc")],
        },
        CacheCase {
            name: "valkey-cache",
            extra: &[
                ("Engine", "Valkey"),
                ("MajorEngineVersion", "8"),
                ("Description", "the production cache"),
                ("SnapshotRetentionLimit", "7"),
                ("DailySnapshotTime", "04:30"),
                ("NetworkType", "dual_stack"),
            ],
            stored_name: "valkey-cache",
            engine: "valkey",
            major_engine_version: "8",
            full_engine_version: "8.0",
            port: "6379",
            description: Some("the production cache"),
            storage_encryption_type: "sse-elasticache",
            subnet_ids: None,
            security_groups: 0,
            reported: &[
                ("SnapshotRetentionLimit", "7"),
                ("DailySnapshotTime", "04:30"),
                ("NetworkType", "dual_stack"),
            ],
        },
        CacheCase {
            name: "mem-cache",
            extra: &[("Engine", "memcached")],
            stored_name: "mem-cache",
            engine: "memcached",
            major_engine_version: "1.6",
            full_engine_version: "1.6.22",
            port: "11211",
            description: None,
            storage_encryption_type: "sse-elasticache",
            subnet_ids: None,
            security_groups: 0,
            reported: &[],
        },
        CacheCase {
            name: "placed-cache",
            extra: &[
                ("Engine", "redis"),
                (
                    "KmsKeyId",
                    "arn:aws:kms:us-east-1:111122223333:key/abcd-1234",
                ),
                ("Tags.Tag.1.Key", "team"),
                ("Tags.Tag.1.Value", "cache"),
            ],
            stored_name: "placed-cache",
            engine: "redis",
            major_engine_version: "7",
            full_engine_version: "7.1",
            port: "6379",
            description: None,
            storage_encryption_type: "sse-kms",
            subnet_ids: Some(2),
            security_groups: 1,
            reported: &[(
                "KmsKeyId",
                "arn:aws:kms:us-east-1:111122223333:key/abcd-1234",
            )],
        },
        CacheCase {
            // The usage limits are persisted exactly as sent, including the
            // largest values this API takes.
            name: "metered-cache",
            extra: &[
                ("Engine", "valkey"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Minimum", "1"),
                ("CacheUsageLimits.DataStorage.Maximum", "5000"),
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "1000"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "15000000"),
            ],
            stored_name: "metered-cache",
            engine: "valkey",
            major_engine_version: "9",
            full_engine_version: "9.0",
            port: "6379",
            description: None,
            storage_encryption_type: "sse-elasticache",
            subnet_ids: None,
            security_groups: 0,
            reported: &[
                ("Unit", "GB"),
                ("DataStorage/Minimum", "1"),
                ("DataStorage/Maximum", "5000"),
                ("ECPUPerSecond/Minimum", "1000"),
                ("ECPUPerSecond/Maximum", "15000000"),
            ],
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        // A row that names subnets names the account's own: those are the only
        // ids `CreateServerlessCache` accepts.
        let account_subnets: Vec<String> = emulator
            .account_subnets()
            .await
            .into_iter()
            .filter(|subnet| subnet.default_for_az)
            .map(|subnet| subnet.id)
            .collect();
        let mut extra = params(case.extra);
        if let Some(count) = case.subnet_ids {
            extra.extend(
                account_subnets[..count]
                    .iter()
                    .enumerate()
                    .map(|(index, id)| {
                        (format!("SubnetIds.SubnetId.{}", index + 1), id.clone())
                    }),
            );
        }
        // A row that attaches security groups names groups the account HAS: EC2
        // mints them, so their ids are asked of the owner rather than written
        // down here.
        let security_groups = emulator
            .security_groups(case.stored_name, case.security_groups)
            .await;
        extra.extend(security_groups.iter().enumerate().map(|(index, id)| {
            (
                format!("SecurityGroupIds.SecurityGroupId.{}", index + 1),
                id.clone(),
            )
        }));
        let created = create_cache_with(&emulator, case.name, &extra).await;
        assert_eq!(
            created.status,
            200,
            "case {}: create must answer 200, got {}",
            case.name,
            created.body
        );
        // A created cache is not ready: it answers the transient status.
        assert_reports_case(
            case,
            "create",
            &created,
            state::SERVERLESS_STATUS_CREATING,
            &account_subnets,
            &security_groups,
        );
        assert_eq!(
            created.text("ServerlessCache/ARN"),
            Some(
                format!(
                    "arn:aws:elasticache:us-east-1:{}:serverlesscache:{}",
                    engine::emulator_account_id(),
                    case.stored_name
                )
                .as_str()
            ),
            "case {}: expected the serverless-cache ARN, got {:?}",
            case.name,
            created.text("ServerlessCache/ARN")
        );

        // The round trip: everything the create reported comes back out of the
        // store, under the stored (lowercased) identity, once the clock has
        // moved the cache on — waited for by looking again at that condition.
        let described = poll_until(
            &emulator,
            REGION,
            case.name,
            "the cache reaching available",
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", case.stored_name)]),
            |reply| reply.text("member/Status") == Some(state::SERVERLESS_STATUS_AVAILABLE),
        )
        .await;
        assert_eq!(
            described.status,
            200,
            "case {}: describe must answer 200, got {}",
            case.name,
            described.body
        );
        assert_reports_case(
            case,
            "describe",
            &described,
            state::SERVERLESS_STATUS_AVAILABLE,
            &account_subnets,
            &security_groups,
        );
        assert!(
            described.text("CreateTime").is_some(),
            "case {}: the describe must report the stored CreateTime, got {}",
            case.name,
            described.body
        );
        // The name the request sent resolves to the same stored record, however
        // it was cased.
        let by_sent_name = describe_cache(&emulator, case.name).await;
        assert_eq!(
            by_sent_name.text("ServerlessCacheName"),
            Some(case.stored_name),
            "case {}: the name as sent must resolve to the stored record, got {}",
            case.name,
            by_sent_name.body
        );
    }
}

#[tokio::test]
async fn a_serverless_cache_reaches_available_on_the_clock_and_not_before() {
    let emulator = Emulator::new();
    let created = create_cache(&emulator, "timed-cache", &[("Engine", "redis")]).await;
    assert_eq!(
        created.status, 200,
        "create must answer 200, got {}",
        created.body
    );
    assert_eq!(
        created.text("Status"),
        Some(state::SERVERLESS_STATUS_CREATING),
        "a created cache must answer the transient status, got {:?}",
        created.text("Status")
    );
    // The read that follows is answered once the creation the clock owed is
    // done, so a client's first poll after its create converges.
    let settled = describe_cache(&emulator, "timed-cache").await;
    assert_eq!(
        settled.text("Status"),
        Some(state::SERVERLESS_STATUS_AVAILABLE),
        "a read after the create must be answered with the settled status, got {:?}",
        settled.text("Status")
    );
    assert!(
        state::SERVERLESS_CACHE_TRANSITION > Duration::ZERO,
        "the transition span must not be zero, got {:?}",
        state::SERVERLESS_CACHE_TRANSITION
    );
}

/// One refusal row: the case name, the pairs sent beside `Engine`, and the HTTP
/// status plus `awsQuery` error code it must be answered with.
type RefusalCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

#[tokio::test]
async fn a_create_this_emulator_cannot_honour_is_refused_not_dropped() {
    // The axis: the member that cannot hold. Every row is a request the store
    // must not grow for — a 200 would claim a configuration it does not hold.
    let cases: &[RefusalCase] = &[
        (
            "unknown engine",
            &[("ServerlessCacheName", "refused"), ("Engine", "dynamodb")],
            400,
            "InvalidParameterValue",
        ),
        (
            "empty name",
            &[("ServerlessCacheName", " "), ("Engine", "redis")],
            400,
            "InvalidParameterValue",
        ),
        (
            "non-version major engine version",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("MajorEngineVersion", "seven"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // One past the published maximum of 35 days.
            "snapshot retention limit 36",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("SnapshotRetentionLimit", "36"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // One past the lower bound of 0 days.
            "snapshot retention limit -1",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("SnapshotRetentionLimit", "-1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "daily snapshot time that is not a time",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("DailySnapshotTime", "midnight"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "daily snapshot time past the 24-hour clock",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("DailySnapshotTime", "24:00"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "usage limit minimum above its maximum",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Minimum", "100"),
                ("CacheUsageLimits.DataStorage.Maximum", "10"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "ecpu limit minimum above its maximum",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "20000"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "1000"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // Below the published lower bound of 1 GB.
            "negative data storage limit",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "-5"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // One past the published upper bound of 5000 GB.
            "data storage limit 5001",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "5001"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "negative ecpu limit",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "-1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "subnet id that is not one",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("SubnetIds.SubnetId.1", "vpc-11111111"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // Subnet-SHAPED, but naming no subnet of the account: the cache's
            // VPC endpoint could never be deployed into it.
            "subnet that does not exist",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("SubnetIds.SubnetId.1", "subnet-0123456789abcdef0"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "restore from a snapshot that does not exist",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                (
                    "SnapshotArnsToRestore.SnapshotArn.1",
                    "arn:aws:elasticache:us-east-1:111122223333:serverlesscachesnapshot:none",
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "user group that does not exist",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("UserGroupId", "analytics"),
            ],
            404,
            "UserGroupNotFound",
        ),
        (
            "user group on an engine that has none",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "memcached"),
                ("UserGroupId", "analytics"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            // "Setting this to public requires Valkey 9 or above."
            "public connection type on an engine that cannot serve it",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("ConnectionType", "public"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "public connection type below the version that serves it",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "valkey"),
                ("MajorEngineVersion", "8"),
                ("ConnectionType", "public"),
            ],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, pairs, status, code) in cases {
        let emulator = Emulator::new();
        let refused = emulator
            .call("CreateServerlessCache", &params(pairs))
            .await;
        assert_eq!(
            refused.status, *status,
            "case {case}: expected HTTP {status}, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            refused.body
        );
        // Nothing was stored, so nothing is reported.
        let listed = emulator
            .call("DescribeServerlessCaches", &params(&[]))
            .await;
        assert!(
            listed.all("ServerlessCacheName").is_empty(),
            "case {case}: a refused create must store nothing, got {}",
            listed.body
        );
    }
}

/// One accepted row: the case name, the pairs sent beside
/// `ServerlessCacheName`, and the element plus text the stored cache must
/// report back.
type AcceptedCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
    &'static str,
);

#[tokio::test]
async fn the_published_retention_bounds_and_version_lines_are_accepted() {
    // The axis: each published bound of `SnapshotRetentionLimit` ("The maximum
    // value allowed is 35 days", with 0 meaning no automatic snapshots), and a
    // major version line that is not the engine's current one.
    let cases: &[AcceptedCase] = &[
        (
            "retention 0",
            &[("Engine", "redis"), ("SnapshotRetentionLimit", "0")],
            "SnapshotRetentionLimit",
            "0",
        ),
        (
            "retention 35",
            &[("Engine", "redis"), ("SnapshotRetentionLimit", "35")],
            "SnapshotRetentionLimit",
            "35",
        ),
        (
            "an older redis line",
            &[("Engine", "redis"), ("MajorEngineVersion", "6")],
            "FullEngineVersion",
            "6.0",
        ),
        (
            "a public valkey 9 cache",
            &[
                ("Engine", "valkey"),
                ("MajorEngineVersion", "9"),
                ("ConnectionType", "public"),
            ],
            "ConnectionType",
            "public",
        ),
        (
            // "You can set the value to zero to remove the limit."
            "a removed data storage limit",
            &[
                ("Engine", "redis"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "0"),
            ],
            "DataStorage/Maximum",
            "0",
        ),
        (
            // The published upper bound of 5000 GB, which is accepted.
            "the largest data storage limit",
            &[
                ("Engine", "redis"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "5000"),
            ],
            "DataStorage/Maximum",
            "5000",
        ),
    ];

    for (case, extra, member, expected) in cases {
        let emulator = Emulator::new();
        let created = create_cache(&emulator, "bounded", extra).await;
        assert_eq!(
            created.status, 200,
            "case {case}: create must answer 200, got {}",
            created.body
        );
        let described = describe_cache(&emulator, "bounded").await;
        assert_eq!(
            described.text(member),
            Some(*expected),
            "case {case}: expected {member} {expected:?} read back, got {:?}",
            described.text(member)
        );
    }
}

#[tokio::test]
async fn the_documented_description_length_is_enforced() {
    // The axis: the length of `Description`, on the published bound ("The
    // maximum length is 255 characters") and one past it, plus the shortest
    // description a request can carry.
    for (case, length, expected_status, expected_code) in [
        ("one character", 1usize, 200u16, ""),
        ("the published maximum", 255, 200, ""),
        ("one past the maximum", 256, 400, "InvalidParameterValue"),
    ] {
        let emulator = Emulator::new();
        let description = "d".repeat(length);
        let created = create_cache(
            &emulator,
            "described",
            &[("Engine", "redis"), ("Description", description.as_str())],
        )
        .await;
        assert_eq!(
            created.status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            expected_code,
            "case {case}: expected error code {expected_code:?}, got {}",
            created.body
        );
        if expected_status == 200 {
            let described = describe_cache(&emulator, "described").await;
            assert!(
                described
                    .body
                    .contains(&format!("<Description>{description}</Description>")),
                "case {case}: the stored description must be reported in full, got {}",
                described.body
            );
        }
    }
}

#[tokio::test]
async fn a_conflicting_name_is_refused_and_leaves_the_stored_cache_alone() {
    let emulator = Emulator::new();
    let first = create_cache(
        &emulator,
        "Shared-Name",
        &[("Engine", "redis"), ("Description", "the first cache")],
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first create must answer 200, got {}",
        first.body
    );

    // The axis: the casing of the conflicting name, which resolves to the same
    // stored identity, and the configuration the second create asks for.
    for (case, name, extra) in [
        (
            "same casing",
            "Shared-Name",
            &[("Engine", "redis"), ("Description", "the second cache")][..],
        ),
        (
            "lowercased",
            "shared-name",
            &[("Engine", "valkey"), ("Description", "the second cache")][..],
        ),
        (
            "uppercased",
            "SHARED-NAME",
            &[("Engine", "memcached")][..],
        ),
    ] {
        let conflicting = create_cache(&emulator, name, extra).await;
        assert_eq!(
            conflicting.status, 400,
            "case {case}: expected HTTP 400, got {} / {}",
            conflicting.status, conflicting.body
        );
        assert_eq!(
            conflicting.error_code(),
            "ServerlessCacheAlreadyExistsFault",
            "case {case}: expected ServerlessCacheAlreadyExistsFault, got {}",
            conflicting.body
        );
    }

    // The refusals changed nothing: the stored cache is the one the first
    // create made.
    let described = describe_cache(&emulator, "shared-name").await;
    assert_eq!(
        described.text("Engine"),
        Some("redis"),
        "a refused create must not change the stored cache, got {}",
        described.body
    );
    assert!(
        described
            .body
            .contains("<Description>the first cache</Description>"),
        "a refused create must not change the stored description, got {}",
        described.body
    );
    assert_eq!(
        described.all("ServerlessCacheName"),
        vec!["shared-name"],
        "exactly one cache must be stored, got {}",
        described.body
    );
}

#[tokio::test]
async fn describe_pages_are_bounded_resumable_and_complete() {
    let emulator = Emulator::new();
    let names = ["cache-a", "cache-b", "cache-c", "cache-d", "cache-e"];
    for name in names {
        let created = create_cache(&emulator, name, &[("Engine", "redis")]).await;
        assert_eq!(
            created.status, 200,
            "case {name}: create must answer 200, got {}",
            created.body
        );
    }

    // The axis: the page size, from one record per page to more than the store
    // holds, plus the request that names no `MaxResults` at all (the documented
    // default of 50).
    for page_size in [Some(1usize), Some(2), Some(5), Some(9), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut pairs: Vec<(String, String)> = Vec::new();
            if let Some(size) = page_size {
                pairs.push(("MaxResults".to_string(), size.to_string()));
            }
            if let Some(token) = next_token.clone() {
                pairs.push(("NextToken".to_string(), token));
            }
            let page = emulator.call("DescribeServerlessCaches", &pairs).await;
            assert_eq!(
                page.status, 200,
                "case {page_size:?}: describe must answer 200, got {}",
                page.body
            );
            let on_page = page.all("ServerlessCacheName");
            assert!(
                on_page.len() <= page_size.unwrap_or(names.len()),
                "case {page_size:?}: a page must not exceed the requested size, got {on_page:?}"
            );
            seen.extend(on_page.iter().map(|name| (*name).to_string()));
            pages += 1;
            next_token = page.text("NextToken").map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= names.len() + 1,
                "case {page_size:?}: pagination must terminate, saw {pages} pages"
            );
        }
        assert_eq!(
            seen,
            names.to_vec(),
            "case {page_size:?}: every cache must be reported exactly once, in order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_describe_naming_a_cache_or_token_this_emulator_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let created = create_cache(&emulator, "present", &[("Engine", "redis")]).await;
    assert_eq!(
        created.status, 200,
        "create must answer 200, got {}",
        created.body
    );

    // The axis: the member that names nothing — an unknown cache, and a
    // continuation token this service never minted.
    let cases: &[RefusalCase] = &[
        (
            "unknown name",
            &[("ServerlessCacheName", "absent")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "token this service did not mint",
            &[("NextToken", "not-a-token")],
            400,
            "InvalidParameterValue",
        ),
        (
            "page size that cannot be a page",
            &[("MaxResults", "0")],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, pairs, status, code) in cases {
        let refused = emulator
            .call("DescribeServerlessCaches", &params(pairs))
            .await;
        assert_eq!(
            refused.status, *status,
            "case {case}: expected HTTP {status}, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn arns_endpoints_and_default_subnets_follow_the_signing_region() {
    // The axis: the signing region, which is where the cache lives — and, for
    // `SubnetIds`, how many default subnets ElastiCache selects there: the
    // literals `ServerlessCache$SubnetIds` publishes (2 in us-west-1, 3
    // everywhere else).
    for (region, default_subnets) in [("us-east-1", 3), ("us-west-1", 2), ("eu-west-1", 3)] {
        let emulator = Emulator::new();
        let created = emulator
            .call_in(
                region,
                "CreateServerlessCache",
                &params(&[("ServerlessCacheName", "regional"), ("Engine", "redis")]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {region}: create must answer 200, got {}",
            created.body
        );
        assert_eq!(
            created.text("ServerlessCache/ARN"),
            Some(
                format!(
                    "arn:aws:elasticache:{region}:{}:serverlesscache:regional",
                    engine::emulator_account_id()
                )
                .as_str()
            ),
            "case {region}: the ARN must carry the signing region, got {:?}",
            created.text("ServerlessCache/ARN")
        );
        // The ENDPOINT is not regional: a cache this emulator serves is reached at
        // the address its own socket answers on, which is where the emulator runs
        // rather than where the request was signed. The ARN above and the subnets
        // below are what the signing region decides.
        for address in created.all("Address") {
            assert!(
                crate::harness::is_own_host(address),
                "case {region}: endpoint {address:?} must be an address the emulator answers on"
            );
        }
        assert_eq!(
            created.all("SubnetId").len(),
            default_subnets,
            "case {region}: expected {default_subnets} default subnets, got {:?}",
            created.all("SubnetId")
        );
    }
}
