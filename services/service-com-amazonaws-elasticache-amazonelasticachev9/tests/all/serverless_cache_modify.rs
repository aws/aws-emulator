// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyServerlessCache`.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from persisted state cannot pass. The request
//! builder and the XML reader are the shared `harness` ones, so this target
//! keeps one harness rather than a copy that could drift.
//!
//! The rule under test: a modify resolves the serverless cache the request names
//! (folded to the stored, lowercased identity), applies every member the request
//! names to that ONE stored record and leaves every member it does not name
//! alone, and refuses a value the cache could not have been created with — the
//! limits it would end up with among them — with the modelled 400 before
//! anything is written, so a refused modify leaves the live cache byte-identical.
//!
//! The tests are tables: each row is one input along the axis the rule
//! generalises over (which member is named, a usage bound named on its own or
//! beside the other, each published bound and one past it, the engine the cache
//! moves onto, the case the name is sent in, and why a refusal is owed), so no
//! single recorded request is what makes them pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION};

/// The configuration every row that does not create its own cache starts from,
/// so one stored baseline is what each modify is measured against.
const BASELINE: &[(&str, &str)] = &[
    ("Engine", "redis"),
    ("Description", "the baseline cache"),
    ("CacheUsageLimits.DataStorage.Unit", "GB"),
    ("CacheUsageLimits.DataStorage.Minimum", "1"),
    ("CacheUsageLimits.DataStorage.Maximum", "10"),
    ("CacheUsageLimits.ECPUPerSecond.Minimum", "1000"),
    ("CacheUsageLimits.ECPUPerSecond.Maximum", "5000"),
    ("SnapshotRetentionLimit", "7"),
    ("DailySnapshotTime", "03:00"),
    ("SecurityGroupIds.SecurityGroupId.1", "sg:baseline"),
];

/// What a row writes where a VPC security group id belongs: the marker, followed by the
/// NAME of the group it means.
///
/// A security group is an Amazon EC2 resource and its id is EC2's to mint, so no static row
/// can carry one. A row names the group; [`resolve_groups`] asks the owner for its id.
const GROUP_MARKER: &str = "sg:";

/// `pairs` with every security-group marker resolved to the id EC2 minted for that group.
async fn resolve_groups(emulator: &Emulator, pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut resolved = Vec::with_capacity(pairs.len());
    for (key, value) in pairs {
        let value = match value.strip_prefix(GROUP_MARKER) {
            Some(name) => emulator.security_group_named(name).await,
            None => (*value).to_string(),
        };
        resolved.push(((*key).to_string(), value));
    }
    resolved
}

/// One expected value with its security-group marker resolved the same way.
async fn resolve_group(emulator: &Emulator, value: &str) -> String {
    match value.strip_prefix(GROUP_MARKER) {
        Some(name) => emulator.security_group_named(name).await,
        None => value.to_string(),
    }
}

/// One row of the apply table.
#[derive(Debug, Clone, Copy)]
struct ModifyCase {
    /// What the row is called in a failure message.
    case: &'static str,
    /// The `ServerlessCacheName` the create sends.
    name: &'static str,
    /// The name the modify sends, which may be in another case: the cache is
    /// stored under the lowercased one.
    request_name: &'static str,
    /// The create pairs sent beside the name; empty means [`BASELINE`].
    create: &'static [(&'static str, &'static str)],
    /// The modify pairs sent beside the name.
    modify: &'static [(&'static str, &'static str)],
    /// Every `(element suffix, text)` the modify's answer AND a later read of
    /// the cache must report — both the members this row changes and the ones it
    /// must have left alone.
    reported: &'static [(&'static str, &'static str)],
    /// Every `(element suffix, texts)` whose whole list must match, for a member
    /// a row replaces wholesale.
    reported_lists: &'static [(&'static str, &'static [&'static str])],
    /// Whether this change is provisioning work the service carries out after
    /// the call answers: the capacity limits, the engine and its version line.
    transient: bool,
}

/// The axis: which member the modify names, a usage bound on its own or beside
/// the other, each published bound, and the engine the cache moves onto.
const CASES: &[ModifyCase] = &[
    ModifyCase {
        case: "the description alone",
        name: "described-cache",
        request_name: "described-cache",
        create: &[],
        modify: &[("Description", "a changed description")],
        reported: &[
            ("Description", "a changed description"),
            // Everything the row did not name keeps the baseline value.
            ("DataStorage/Minimum", "1"),
            ("DataStorage/Maximum", "10"),
            ("ECPUPerSecond/Minimum", "1000"),
            ("ECPUPerSecond/Maximum", "5000"),
            ("SnapshotRetentionLimit", "7"),
            ("DailySnapshotTime", "03:00"),
            ("Engine", "redis"),
        ],
        reported_lists: &[],
        transient: false,
    },
    ModifyCase {
        case: "both usage limits widened at once",
        name: "widened-cache",
        request_name: "widened-cache",
        create: &[],
        modify: &[
            ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ("CacheUsageLimits.DataStorage.Minimum", "1"),
            ("CacheUsageLimits.DataStorage.Maximum", "20"),
            ("CacheUsageLimits.ECPUPerSecond.Minimum", "1000"),
            ("CacheUsageLimits.ECPUPerSecond.Maximum", "9000"),
        ],
        reported: &[
            ("DataStorage/Unit", "GB"),
            ("DataStorage/Minimum", "1"),
            ("DataStorage/Maximum", "20"),
            ("ECPUPerSecond/Minimum", "1000"),
            ("ECPUPerSecond/Maximum", "9000"),
            ("Description", "the baseline cache"),
        ],
        reported_lists: &[],
        transient: true,
    },
    ModifyCase {
        // A bound is a bound rather than a document: the one named moves and
        // the ones left out keep the values the cache is running, which is why
        // the API publishes a zero for removing a bound.
        case: "one usage bound named, the rest kept",
        name: "one-bound-cache",
        request_name: "one-bound-cache",
        create: &[],
        modify: &[("CacheUsageLimits.ECPUPerSecond.Maximum", "9000")],
        reported: &[
            ("ECPUPerSecond/Maximum", "9000"),
            ("ECPUPerSecond/Minimum", "1000"),
            ("DataStorage/Unit", "GB"),
            ("DataStorage/Minimum", "1"),
            ("DataStorage/Maximum", "10"),
        ],
        reported_lists: &[],
        transient: true,
    },
    ModifyCase {
        // "You can set the value to zero to remove the limit."
        case: "a usage bound removed with the published zero",
        name: "removed-bound-cache",
        request_name: "Removed-Bound-Cache",
        create: &[],
        modify: &[
            ("CacheUsageLimits.DataStorage.Unit", "GB"),
            ("CacheUsageLimits.DataStorage.Minimum", "0"),
        ],
        reported: &[
            ("DataStorage/Minimum", "0"),
            ("DataStorage/Maximum", "10"),
            ("ECPUPerSecond/Maximum", "5000"),
        ],
        reported_lists: &[],
        transient: true,
    },
    ModifyCase {
        // The published maximum retention (35 days) and the last minute of the
        // day, both accepted.
        case: "the snapshot members on their bound",
        name: "snapshot-cache",
        request_name: "snapshot-cache",
        create: &[],
        modify: &[
            ("SnapshotRetentionLimit", "35"),
            ("DailySnapshotTime", "23:59"),
        ],
        reported: &[
            ("SnapshotRetentionLimit", "35"),
            ("DailySnapshotTime", "23:59"),
            ("DataStorage/Maximum", "10"),
        ],
        reported_lists: &[],
        transient: false,
    },
    ModifyCase {
        case: "the retention dropped to the lowest it takes",
        name: "retention-cache",
        request_name: "retention-cache",
        create: &[],
        modify: &[("SnapshotRetentionLimit", "0")],
        reported: &[
            ("SnapshotRetentionLimit", "0"),
            ("DailySnapshotTime", "03:00"),
        ],
        reported_lists: &[],
        transient: false,
    },
    ModifyCase {
        // "Populating this list means the current VPC security groups will be
        // removed."
        case: "the security group list replaced wholesale",
        name: "grouped-sg-cache",
        request_name: "grouped-sg-cache",
        create: &[],
        modify: &[
            ("SecurityGroupIds.SecurityGroupId.1", "sg:replacement-one"),
            ("SecurityGroupIds.SecurityGroupId.2", "sg:replacement-two"),
        ],
        reported: &[("Description", "the baseline cache")],
        reported_lists: &[(
            "SecurityGroupIds/SecurityGroupId",
            &["sg:replacement-one", "sg:replacement-two"],
        )],
        transient: false,
    },
    ModifyCase {
        // A cache moved onto another engine reports that engine's version line,
        // the full version derived from it, and that engine's client port
        // (Memcached 11211, Valkey / Redis OSS 6379).
        case: "the engine and its version line",
        name: "engine-cache",
        request_name: "engine-cache",
        create: &[("Engine", "memcached"), ("DailySnapshotTime", "04:30")],
        modify: &[("Engine", "valkey"), ("MajorEngineVersion", "8")],
        reported: &[
            ("Engine", "valkey"),
            ("MajorEngineVersion", "8"),
            ("FullEngineVersion", "8.0"),
            ("Endpoint/Port", "6379"),
            ("DailySnapshotTime", "04:30"),
        ],
        reported_lists: &[],
        transient: true,
    },
];

/// Create one serverless cache and insist the create itself succeeded.
async fn create_cache(emulator: &Emulator, case: &str, name: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[("ServerlessCacheName", name)]);
    pairs.extend(resolve_groups(emulator, extra).await);
    let created = emulator.call("CreateServerlessCache", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateServerlessCache must answer 200, got {} {}",
        created.status, created.body
    );
    created
}

/// `ModifyServerlessCache` with the pairs a row sends.
async fn modify_cache(emulator: &Emulator, name: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[("ServerlessCacheName", name)]);
    pairs.extend(resolve_groups(emulator, extra).await);
    emulator.call("ModifyServerlessCache", &pairs).await
}

/// `DescribeServerlessCaches` for one cache by name.
async fn describe_cache(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await
}

/// Assert `reply` reports every member `case` expects, naming the case, the
/// member and the actual value on every failure. `where_from` says which
/// response is under test.
async fn assert_reports(emulator: &Emulator, case: &ModifyCase, where_from: &str, reply: &Reply) {
    for (member, expected) in case.reported {
        let expected = resolve_group(emulator, expected).await;
        assert_eq!(
            reply.text(member),
            Some(expected.as_str()),
            "case {} ({where_from}): expected {member} {expected:?}, got {:?}",
            case.case,
            reply.text(member)
        );
    }
    for (member, expected) in case.reported_lists {
        let mut resolved = Vec::with_capacity(expected.len());
        for value in *expected {
            resolved.push(resolve_group(emulator, value).await);
        }
        assert_eq!(
            reply.all(member),
            resolved.iter().map(String::as_str).collect::<Vec<&str>>(),
            "case {} ({where_from}): expected {member} {resolved:?}, got {:?}",
            case.case,
            reply.all(member)
        );
    }
}

/// Assert `reply` is the modelled refusal `code` with HTTP `status`.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case}: expected HTTP {status}, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case}: expected error code {code:?}, got {:?} ({})",
        reply.error_code(),
        reply.body
    );
}

#[tokio::test]
async fn a_modify_applies_every_member_it_names_and_leaves_the_rest() {
    for case in CASES {
        let emulator = Emulator::new();
        let create = if case.create.is_empty() {
            BASELINE
        } else {
            case.create
        };
        create_cache(&emulator, case.case, case.name, create).await;
        // A cache is reconfigured from its settled, usable status, and reading it
        // back is answered once the create has settled it.
        let created = describe_cache(&emulator, case.request_name).await;
        assert_eq!(
            created.text("Status"),
            Some(state::SERVERLESS_STATUS_AVAILABLE),
            "case {}: the read after the create must report the cache available, got {}",
            case.case,
            created.body
        );

        let modified = modify_cache(&emulator, case.request_name, case.modify).await;
        assert_eq!(
            modified.status, 200,
            "case {}: ModifyServerlessCache must answer 200, got {} {}",
            case.case, modified.status, modified.body
        );
        assert_eq!(
            modified.text("ServerlessCacheName"),
            Some(case.name),
            "case {}: expected the stored, lowercased ServerlessCacheName {:?}, got {:?}",
            case.case,
            case.name,
            modified.text("ServerlessCacheName")
        );
        assert_reports(&emulator, case, "the modify's answer", &modified).await;

        // Capacity and engine changes are work the service carries out after the
        // call, so the call itself answers with the cache transient until the
        // clock owes it `AVAILABLE` again; a settings change leaves it usable at
        // once.
        let after_call = if case.transient {
            state::SERVERLESS_STATUS_MODIFYING
        } else {
            state::SERVERLESS_STATUS_AVAILABLE
        };
        assert_eq!(
            modified.text("Status"),
            Some(after_call),
            "case {} (the modify's answer): expected Status {after_call:?}, got {:?}",
            case.case,
            modified.text("Status")
        );

        // The read that follows is answered once the work is done, so the cache
        // is available again and still reports exactly what the modify stored:
        // the change is persisted, not echoed.
        let settled = describe_cache(&emulator, case.request_name).await;
        assert_eq!(
            settled.text("Status"),
            Some(state::SERVERLESS_STATUS_AVAILABLE),
            "case {}: a read after the modify must report {:?}, got {:?}",
            case.case,
            state::SERVERLESS_STATUS_AVAILABLE,
            settled.text("Status")
        );
        assert_reports(&emulator, case, "a read after the modify", &settled).await;
    }
}

#[tokio::test]
async fn a_refused_modify_leaves_the_live_cache_untouched() {
    let emulator = Emulator::new();
    create_cache(&emulator, "baseline", "available-cache", BASELINE).await;
    create_cache(
        &emulator,
        "memcached",
        "memcached-cache",
        &[("Engine", "memcached")],
    )
    .await;
    let group = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "modify-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "CreateUserGroup must answer 200, got {} {}",
        group.status, group.body
    );
    // The row that moves a cache carrying a user group onto Memcached needs such
    // a cache, which only an accepted modify can make.
    create_cache(&emulator, "grouped", "grouped-cache", &[("Engine", "redis")]).await;
    // Every cache above, and the group, is settled before the table runs.
    std::thread::sleep(PAST_TRANSITION);
    let attached = modify_cache(
        &emulator,
        "grouped-cache",
        &[("UserGroupId", "modify-group")],
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "attaching a user group must answer 200, got {} {}",
        attached.status, attached.body
    );
    // This one is still inside its create span while the table below runs.
    create_cache(
        &emulator,
        "creating",
        "creating-cache",
        &[("Engine", "valkey")],
    )
    .await;

    // The axis: why the modify cannot be honoured — a bound inverted in the
    // request, a bound inverted only against the limits the cache is running, a
    // bound past what the API publishes, a member outside its documented range,
    // a value that names no engine or version, two members that contradict each
    // other, a group that does not exist, a cache that does not exist, and a
    // cache in a status no modify is accepted from.
    let rows: &[RefusalCase] = &[
        (
            "data storage minimum above its own maximum",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Minimum", "50"),
                ("CacheUsageLimits.DataStorage.Maximum", "10"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "ecpu minimum above its own maximum",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "9000"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "1000"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a minimum inverted against the maximum the cache is running",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Minimum", "50"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a maximum inverted against the minimum the cache is running",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "100"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a data storage bound one past the published maximum",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "5001"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative ecpu bound",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "-1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a retention limit one past the documented maximum",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("SnapshotRetentionLimit", "36"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative retention limit",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("SnapshotRetentionLimit", "-1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a daily snapshot time that names no time of day",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("DailySnapshotTime", "24:00"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine this service does not run",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("Engine", "not-an-engine"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a major engine version that is not a version",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("MajorEngineVersion", "latest"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a user group both attached and removed",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("UserGroupId", "modify-group"),
                ("RemoveUserGroup", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a user group on a memcached cache",
            &[
                ("ServerlessCacheName", "memcached-cache"),
                ("UserGroupId", "modify-group"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an engine the cache's user group cannot serve",
            &[
                ("ServerlessCacheName", "grouped-cache"),
                ("Engine", "memcached"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a user group no account holds",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("UserGroupId", "no-such-group"),
            ],
            404,
            "UserGroupNotFound",
        ),
        (
            "a name no cache was created under",
            &[("ServerlessCacheName", "no-such-cache")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "the same absent name in mixed case",
            &[("ServerlessCacheName", "No-Such-Cache")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "a cache still inside its create span",
            &[
                ("ServerlessCacheName", "creating-cache"),
                ("Description", "too early"),
            ],
            400,
            "InvalidServerlessCacheStateFault",
        ),
    ];

    for (case, pairs, status, code) in rows {
        let refused = emulator
            .call("ModifyServerlessCache", &params(pairs))
            .await;
        assert_refused(case, &refused, *status, code);
    }

    // A description longer than the documented maximum is refused too; the
    // value is built rather than written out, so the bound itself is the input.
    let too_long = "d".repeat(state::MAX_SERVERLESS_DESCRIPTION_LENGTH + 1);
    let refused = emulator
        .call(
            "ModifyServerlessCache",
            &params(&[
                ("ServerlessCacheName", "available-cache"),
                ("Description", too_long.as_str()),
            ]),
        )
        .await;
    assert_refused("a description past the documented maximum", &refused, 400, "InvalidParameterValue");

    // The wording of the refusal a contradictory envelope is owed, which is one
    // sentence per member rather than per request.
    for (member, pairs) in [
        (
            "DataStorage",
            vec![
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Minimum", "50"),
                ("CacheUsageLimits.DataStorage.Maximum", "10"),
            ],
        ),
        (
            "ECPUPerSecond",
            vec![
                ("ServerlessCacheName", "available-cache"),
                ("CacheUsageLimits.ECPUPerSecond.Minimum", "9000"),
                ("CacheUsageLimits.ECPUPerSecond.Maximum", "1000"),
            ],
        ),
    ] {
        let refused = emulator
            .call("ModifyServerlessCache", &params(&pairs))
            .await;
        let expected =
            format!("Invalid values for {member}. Minimum must not be greater than Maximum.");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {member}: expected the refusal message {expected:?}, got {:?}",
            refused.error_message()
        );
    }

    // Nothing the table asked for happened: the live cache still runs every
    // value it was created with, in the status it was in.
    let untouched = describe_cache(&emulator, "available-cache").await;
    // The baseline's security group is the one EC2 minted for it, so the id is asked of
    // the owner here the same way the create asked for it.
    let baseline_group = resolve_group(&emulator, "sg:baseline").await;
    for (member, expected) in [
        ("Description", "the baseline cache"),
        ("DataStorage/Unit", "GB"),
        ("DataStorage/Minimum", "1"),
        ("DataStorage/Maximum", "10"),
        ("ECPUPerSecond/Minimum", "1000"),
        ("ECPUPerSecond/Maximum", "5000"),
        ("SnapshotRetentionLimit", "7"),
        ("DailySnapshotTime", "03:00"),
        ("Engine", "redis"),
        ("MajorEngineVersion", "7"),
        ("SecurityGroupId", baseline_group.as_str()),
        ("Status", state::SERVERLESS_STATUS_AVAILABLE),
    ] {
        assert_eq!(
            untouched.text(member),
            Some(expected),
            "a refused modify must leave {member} at {expected:?}, got {:?} ({})",
            untouched.text(member),
            untouched.body
        );
    }
    // And no refusal attached a user group to it.
    assert!(
        untouched.text("UserGroupId").is_none(),
        "a refused modify must not attach a user group, got {:?}",
        untouched.text("UserGroupId")
    );
}

#[tokio::test]
async fn a_modify_moves_the_cache_between_user_groups_and_is_recorded() {
    let emulator = Emulator::new();
    for id in ["first-group", "second-group"] {
        let group = emulator
            .call(
                "CreateUserGroup",
                &params(&[
                    ("UserGroupId", id),
                    ("Engine", "redis"),
                    ("UserIds.member.1", "default"),
                ]),
            )
            .await;
        assert_eq!(
            group.status, 200,
            "case {id}: CreateUserGroup must answer 200, got {} {}",
            group.status, group.body
        );
    }
    create_cache(&emulator, "moved", "moved-cache", &[("Engine", "redis")]).await;
    std::thread::sleep(PAST_TRANSITION);

    /// Which caches a group reports it serves.
    async fn served(emulator: &Emulator, id: &str) -> Vec<String> {
        let described = emulator
            .call("DescribeUserGroups", &params(&[("UserGroupId", id)]))
            .await;
        assert_eq!(
            described.status, 200,
            "case {id}: DescribeUserGroups must answer 200, got {} {}",
            described.status, described.body
        );
        described
            .all("ServerlessCaches/member")
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// One step of the user-group walk: the case name, the modify pairs, and the
    /// group the cache must carry afterwards.
    type GroupStep = (
        &'static str,
        &'static [(&'static str, &'static str)],
        Option<&'static str>,
    );

    // The axis: the group the cache is attached to, in order — none, one, the
    // other, none again. The cache and the group agree at every step.
    let steps: &[GroupStep] = &[
        ("attached to the first group", &[("UserGroupId", "first-group")], Some("first-group")),
        ("moved to the second group", &[("UserGroupId", "second-group")], Some("second-group")),
        ("the group taken away", &[("RemoveUserGroup", "true")], None),
    ];
    for (case, pairs, expected) in steps {
        let modified = modify_cache(&emulator, "Moved-Cache", pairs).await;
        assert_eq!(
            modified.status, 200,
            "case {case}: ModifyServerlessCache must answer 200, got {} {}",
            modified.status, modified.body
        );
        assert_eq!(
            modified.text("UserGroupId"),
            *expected,
            "case {case}: expected UserGroupId {expected:?} in the answer, got {:?}",
            modified.text("UserGroupId")
        );
        let read_back = describe_cache(&emulator, "moved-cache").await;
        assert_eq!(
            read_back.text("UserGroupId"),
            *expected,
            "case {case}: expected UserGroupId {expected:?} read back, got {:?}",
            read_back.text("UserGroupId")
        );
        for id in ["first-group", "second-group"] {
            let caches = served(&emulator, id).await;
            let should_serve = *expected == Some(id);
            assert_eq!(
                caches.contains(&"moved-cache".to_string()),
                should_serve,
                "case {case}: group {id} must{} report the cache, got {caches:?}",
                if should_serve { "" } else { " not" }
            );
        }
    }

    // Modifying a serverless cache is one of the events this service records.
    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceIdentifier", "moved-cache"),
                ("SourceType", state::SOURCE_TYPE_SERVERLESS_CACHE),
            ]),
        )
        .await;
    assert!(
        events
            .all("Message")
            .iter()
            .any(|message| message.contains("modified")),
        "a modify must be recorded in the event log, got {}",
        events.body
    );

    // A cache the clock has removed takes no further modification: the name
    // resolves no cache at all.
    let deleted = emulator
        .call(
            "DeleteServerlessCache",
            &params(&[("ServerlessCacheName", "moved-cache")]),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteServerlessCache must answer 200, got {} {}",
        deleted.status, deleted.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let gone = modify_cache(&emulator, "moved-cache", &[("Description", "too late")]).await;
    assert_refused(
        "a modify of a cache the clock has removed",
        &gone,
        404,
        "ServerlessCacheNotFoundFault",
    );
}
