// SPDX-License-Identifier: CC0-1.0

//! The engine-version FLOOR an ElastiCache RBAC record reports: `User` and
//! `UserGroup`'s `MinimumEngineVersion`.
//!
//! The rule under test: an RBAC record reports the lowest engine version that
//! supports it, which is the oldest version the record's OWN engine publishes at
//! or above the floor of the latest RBAC feature its configuration uses — RBAC
//! itself for a password or passwordless user and for a user group, and IAM
//! authentication for a user that authenticates that way. Valkey's first
//! ElastiCache line is later than every one of those floors, so a record listed
//! for Valkey reports that line whatever it authenticates with, while a Redis OSS
//! record reports the floor itself.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from persisted state cannot pass. Each row is
//! one input along the axes the rule generalises over: the engine, its spelling,
//! which of the three members says how the user authenticates, and whether the
//! record was created that way or changed into it.

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

/// A password long enough for the published length rule; its only role here is to
/// be a password.
const PASSWORD: &str = "aVeryLongPassword1";

/// One row: the engine as the request spells it, the members that say how the
/// user authenticates, and the floor the user must report.
struct FloorCase {
    case: &'static str,
    user_id: &'static str,
    /// `Engine`, in the spelling the request sends.
    engine: &'static str,
    /// The engine the record is listed for once stored (lowercased).
    stored_engine: &'static str,
    authentication: &'static [(&'static str, &'static str)],
    /// `Authentication$Type` the stored user reports.
    authentication_type: &'static str,
    expected_floor: &'static str,
}

/// The floor table.
///
/// Source of the expected values: the engine lines
/// `DescribeCacheEngineVersions` publishes for each engine (the oldest Redis OSS
/// line that carries RBAC is 6.0 and the oldest Valkey line ElastiCache offers at
/// all is 7.2 — `state::CACHE_ENGINE_VERSIONS`), read against the feature floors
/// `state::USER_MIN_ENGINE_VERSION` (RBAC) and
/// `state::USER_MIN_ENGINE_VERSION_IAM` (IAM authentication). The rows below name
/// the resulting version literally, so a change of either side has to be stated
/// here too.
const FLOOR_CASES: &[FloorCase] = &[
    FloorCase {
        case: "redis, no password",
        user_id: "redis-nopass",
        engine: "redis",
        stored_engine: "redis",
        authentication: &[("NoPasswordRequired", "true")],
        authentication_type: "no-password",
        expected_floor: "6.0",
    },
    FloorCase {
        case: "redis, one password",
        user_id: "redis-password",
        engine: "redis",
        stored_engine: "redis",
        authentication: &[("Passwords.member.1", PASSWORD)],
        authentication_type: "password",
        expected_floor: "6.0",
    },
    FloorCase {
        case: "redis, IAM authentication",
        user_id: "redis-iam",
        engine: "redis",
        stored_engine: "redis",
        authentication: &[("AuthenticationMode.Type", "iam")],
        authentication_type: "iam",
        // IAM authentication arrived after RBAC, and Redis OSS publishes a line
        // at that later floor.
        expected_floor: "7.0",
    },
    FloorCase {
        case: "valkey, no password",
        user_id: "valkey-nopass",
        engine: "valkey",
        stored_engine: "valkey",
        authentication: &[("NoPasswordRequired", "true")],
        authentication_type: "no-password",
        expected_floor: "7.2",
    },
    FloorCase {
        case: "valkey, one password",
        user_id: "valkey-password",
        engine: "valkey",
        stored_engine: "valkey",
        authentication: &[("Passwords.member.1", PASSWORD)],
        authentication_type: "password",
        expected_floor: "7.2",
    },
    FloorCase {
        case: "valkey, IAM authentication",
        user_id: "valkey-iam",
        engine: "valkey",
        stored_engine: "valkey",
        authentication: &[("AuthenticationMode.Type", "iam")],
        authentication_type: "iam",
        // Valkey publishes nothing before its first line, so the later feature
        // floor does not raise this one.
        expected_floor: "7.2",
    },
    FloorCase {
        case: "the engine in another letter case",
        user_id: "mixed-case-engine",
        engine: "VaLkEy",
        stored_engine: "valkey",
        authentication: &[("NoPasswordRequired", "true")],
        authentication_type: "no-password",
        expected_floor: "7.2",
    },
];

/// `CreateUser` for one row.
async fn create_user(emulator: &Emulator, row: &FloorCase) -> Reply {
    let mut request = params(&[
        ("UserId", row.user_id),
        ("UserName", row.user_id),
        ("Engine", row.engine),
        ("AccessString", "on ~* +@all"),
    ]);
    request.extend(params(row.authentication));
    let created = emulator.call("CreateUser", &request).await;
    assert_eq!(
        created.status, 200,
        "case {}: CreateUser must answer 200, got {} {}",
        row.case, created.status, created.body
    );
    created
}

/// Read one user back through the implemented reader.
async fn describe_user(emulator: &Emulator, case: &str, user_id: &str) -> Reply {
    let described = emulator
        .call("DescribeUsers", &params(&[("UserId", user_id)]))
        .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeUsers must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// Every engine version `DescribeCacheEngineVersions` publishes for `engine`,
/// over the wire — the catalog a client resolves a floor against.
async fn published_versions(emulator: &Emulator, engine: &str) -> Vec<String> {
    let mut versions: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = params(&[("Engine", engine), ("MaxRecords", "100")]);
        if let Some(from) = marker.as_deref() {
            request.extend(params(&[("Marker", from)]));
        }
        let reply = emulator
            .call("DescribeCacheEngineVersions", &request)
            .await;
        assert_eq!(
            reply.status, 200,
            "DescribeCacheEngineVersions must answer 200, got {} {}",
            reply.status, reply.body
        );
        versions.extend(reply.all("EngineVersion").iter().map(|it| (*it).to_string()));
        match reply.marker() {
            Some(next) => marker = Some(next.to_string()),
            None => break,
        }
    }
    assert!(
        !versions.is_empty(),
        "engine {engine} must publish at least one version"
    );
    versions
}

#[tokio::test]
async fn a_created_user_reports_the_floor_its_engine_and_authentication_need() {
    let emulator = Emulator::new();
    for row in FLOOR_CASES {
        let created = create_user(&emulator, row).await;
        assert_eq!(
            created.text("MinimumEngineVersion"),
            Some(row.expected_floor),
            "case {}: the created user must report {}, got {}",
            row.case,
            row.expected_floor,
            created.body
        );
        assert_eq!(
            created.text("Engine"),
            Some(row.stored_engine),
            "case {}: the user must be listed for {}, got {}",
            row.case,
            row.stored_engine,
            created.body
        );
        assert_eq!(
            created.text("Authentication/Type"),
            Some(row.authentication_type),
            "case {}: the user must authenticate with {}, got {}",
            row.case,
            row.authentication_type,
            created.body
        );

        // The same floor out of persisted state, through the reader.
        let described = describe_user(&emulator, row.case, row.user_id).await;
        assert_eq!(
            described.text("MinimumEngineVersion"),
            Some(row.expected_floor),
            "case {}: the stored user must report {}, got {}",
            row.case,
            row.expected_floor,
            described.body
        );
    }
}

#[tokio::test]
async fn the_floor_a_user_reports_is_a_version_its_engine_publishes() {
    let emulator = Emulator::new();
    for row in FLOOR_CASES {
        let created = create_user(&emulator, row).await;
        let reported = created
            .text("MinimumEngineVersion")
            .unwrap_or_else(|| panic!("case {}: no MinimumEngineVersion: {}", row.case, created.body))
            .to_string();
        let published = published_versions(&emulator, row.stored_engine).await;
        assert!(
            published.contains(&reported),
            "case {}: {reported} must be a version {} publishes, got the catalog {published:?}",
            row.case,
            row.stored_engine
        );
        // The floor the feature needs, as this service states it, and the
        // OLDEST published version of the engine at or above it: that is what
        // the record has to report, so a client that gates on it plans a cache
        // on a version the user can actually be attached to.
        let feature_floor = if row.authentication_type == "iam" {
            state::USER_MIN_ENGINE_VERSION_IAM
        } else {
            state::USER_MIN_ENGINE_VERSION
        };
        let oldest_supported = published
            .iter()
            .filter(|version| {
                state::version_sort_key(version) >= state::version_sort_key(feature_floor)
            })
            .min_by_key(|version| state::version_sort_key(version))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: engine {} publishes no version at or above {feature_floor}",
                    row.case, row.stored_engine
                )
            });
        assert_eq!(
            &reported, oldest_supported,
            "case {}: the floor must be the oldest published version of {} at or above \
             {feature_floor}, got {reported}",
            row.case, row.stored_engine
        );
    }
}

#[tokio::test]
async fn a_user_group_reports_the_floor_of_the_engine_it_is_listed_for() {
    // The axis: the engine RBAC exists for (`state::USER_ENGINES`), and the
    // spelling it is sent in.
    for (case, engine, stored_engine, expected_floor) in [
        ("redis", "redis", "redis", "6.0"),
        ("valkey", "valkey", "valkey", "7.2"),
        ("valkey in another letter case", "VALKEY", "valkey", "7.2"),
    ] {
        let emulator = Emulator::new();
        let created = emulator
            .call(
                "CreateUserGroup",
                &params(&[
                    ("UserGroupId", "floor-group"),
                    ("Engine", engine),
                    // "Redis user group needs to contain a user with the user
                    // name default": the built-in user every account has.
                    ("UserIds.member.1", state::DEFAULT_USER_ID),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: CreateUserGroup must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("MinimumEngineVersion"),
            Some(expected_floor),
            "case {case}: the created group must report {expected_floor}, got {}",
            created.body
        );
        assert_eq!(
            created.text("Engine"),
            Some(stored_engine),
            "case {case}: the group must be listed for {stored_engine}, got {}",
            created.body
        );

        let described = emulator
            .call(
                "DescribeUserGroups",
                &params(&[("UserGroupId", "floor-group")]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: DescribeUserGroups must answer 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("MinimumEngineVersion"),
            Some(expected_floor),
            "case {case}: the stored group must report {expected_floor}, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn the_floor_follows_a_user_whose_engine_or_authentication_changes() {
    // Each row: what the user is changed to, and the floor it must report
    // afterwards. The user starts as a Redis OSS password user, which reports
    // RBAC's own floor.
    for (case, change, expected_floor) in [
        (
            "the engine moves to the later line",
            &[("Engine", "valkey")][..],
            "7.2",
        ),
        (
            "the authentication moves to IAM",
            &[("AuthenticationMode.Type", "iam")][..],
            "7.0",
        ),
        (
            "both move at once",
            &[("Engine", "valkey"), ("AuthenticationMode.Type", "iam")][..],
            "7.2",
        ),
    ] {
        let emulator = Emulator::new();
        let created = create_user(
            &emulator,
            &FloorCase {
                case,
                user_id: "moving-user",
                engine: "redis",
                stored_engine: "redis",
                authentication: &[("Passwords.member.1", PASSWORD)],
                authentication_type: "password",
                expected_floor: "6.0",
            },
        )
        .await;
        assert_eq!(
            created.text("MinimumEngineVersion"),
            Some(state::USER_MIN_ENGINE_VERSION),
            "case {case}: the user must start at {}, got {}",
            state::USER_MIN_ENGINE_VERSION,
            created.body
        );

        let mut request = params(&[("UserId", "moving-user")]);
        request.extend(params(change));
        let modified = emulator.call("ModifyUser", &request).await;
        assert_eq!(
            modified.status, 200,
            "case {case}: ModifyUser must answer 200, got {} {}",
            modified.status, modified.body
        );
        assert_eq!(
            modified.text("MinimumEngineVersion"),
            Some(expected_floor),
            "case {case}: the modified user must report {expected_floor}, got {}",
            modified.body
        );
        // And out of persisted state, not only in the answer the change made.
        let described = describe_user(&emulator, case, "moving-user").await;
        assert_eq!(
            described.text("MinimumEngineVersion"),
            Some(expected_floor),
            "case {case}: the stored user must report {expected_floor}, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn the_floor_follows_a_user_group_whose_engine_changes() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "moving-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", state::DEFAULT_USER_ID),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateUserGroup must answer 200, got {} {}",
        created.status, created.body
    );
    assert_eq!(
        created.text("MinimumEngineVersion"),
        Some(state::USER_MIN_ENGINE_VERSION),
        "the group must start at {}, got {}",
        state::USER_MIN_ENGINE_VERSION,
        created.body
    );

    // A change needs an ACTIVE group, which the clock owes the created one.
    let request = params(&[("UserGroupId", "moving-group")]);
    poll_until(
        &emulator,
        REGION,
        "engine change",
        "the created group becoming active",
        "DescribeUserGroups",
        &request,
        |reply| reply.text("Status") == Some(state::USER_STATUS_ACTIVE),
    )
    .await;

    let mut change = params(&[("UserGroupId", "moving-group")]);
    change.extend(params(&[("Engine", "valkey")]));
    let modified = emulator.call("ModifyUserGroup", &change).await;
    assert_eq!(
        modified.status, 200,
        "ModifyUserGroup must answer 200, got {} {}",
        modified.status, modified.body
    );
    // The engine has no pending-value member to be reported through, so the
    // change applies at once — and the floor follows it.
    assert_eq!(
        modified.text("MinimumEngineVersion"),
        Some("7.2"),
        "the group must report 7.2 once it is listed for valkey, got {}",
        modified.body
    );
    let described = emulator
        .call("DescribeUserGroups", &request)
        .await;
    assert_eq!(
        described.text("MinimumEngineVersion"),
        Some("7.2"),
        "the stored group must report 7.2, got {}",
        described.body
    );
}
