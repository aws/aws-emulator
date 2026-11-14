// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the guards a write or a read runs BEFORE it touches state:
//! `CreateDBInstance`, `ModifyDBInstance`, `CreateOptionGroup`, `DeleteOptionGroup`,
//! `CreateDBSubnetGroup`, `DescribeDBSubnetGroups` and every paginated describe.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the
//! one mounting helper this test target has), and each request is a real awsQuery form
//! POST carrying the SigV4 credential scope the router keys on. The assertions read
//! the XML the emulator answers with, so a handler that computed nothing could not
//! pass them.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. A request that NAMES another resource is resolved against this service's state
//!    before anything is written, and a name that resolves to nothing is answered with
//!    that resource type's modelled not-found fault — leaving no record behind.
//! 2. A request whose value must come from a catalogue or form this service publishes
//!    is resolved against THAT catalogue (the engine list its model publishes, the
//!    engine versions `DescribeDBEngineVersions` reports, the DB instance class form)
//!    before the existence and conflict checks, so a parameter error is reported as a
//!    parameter error.
//! 3. A `Marker` no prior page of the same report issued is refused, never
//!    resynchronised to the nearest record.
//!
//! The tables span what those rules generalise over: engines read out of the model's
//! own published list, engine versions and major versions read out of the service's own
//! `DescribeDBEngineVersions` report, the DB instance class sizes the published form
//! admits and refuses, the six paginated describes, and the regional scope a name
//! resolves in.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_INSTANCE_ENGINES, DEFAULT_DB_SUBNET_GROUP, INSTANCE_SIZES, MAX_SUBNETS_PER_DB_SUBNET_GROUP,
    OPTION_GROUP_ENGINE_NAMES, SERVERLESS_INSTANCE_CLASS, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, texts, REGION};

/// A DB instance class every row that is not ABOUT the class uses: of the published
/// form, and one `DescribeOrderableDBInstanceOptions` offers.
const A_CLASS: &str = "db.m5.large";

/// Two well-formed EC2 subnet ids (`subnet-` plus 8 or 17 hexadecimal characters) —
/// enough distinct subnets for a DB subnet group to be accepted.
const TWO_SUBNETS: [(&str, &str); 2] = [
    ("SubnetIds.SubnetIdentifier.1", "subnet-0a1b2c3d"),
    ("SubnetIds.SubnetIdentifier.2", "subnet-1b2c3d4e5f6a7b8c9"),
];

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// Wait out the span a status transition this service owes takes, plus slack for a
/// loaded test machine.
fn wait_for_settle() {
    std::thread::sleep(STATUS_TRANSITION_DELAY + std::time::Duration::from_millis(200));
}

/// `CreateDBInstance` with its three required members plus `extra`.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
    extra: &[(&'static str, String)],
) -> (u16, String) {
    let mut params = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => engine,
        "DBInstanceClass" => class,
    ]
    .to_vec();
    params.extend(extra.iter().cloned());
    call(router, "CreateDBInstance", &params).await
}

/// `CreateDBSubnetGroup` naming [`TWO_SUBNETS`], in `region`.
async fn create_subnet_group_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, String) {
    let mut params = params![
        "DBSubnetGroupName" => name,
        "DBSubnetGroupDescription" => "two-subnets",
    ]
    .to_vec();
    params.extend(
        TWO_SUBNETS
            .iter()
            .map(|(key, value)| (*key, (*value).to_string())),
    );
    call_in(router, region, "CreateDBSubnetGroup", &params).await
}

/// `CreateOptionGroup` for a MySQL major version this service's engine-version report
/// carries, which is all the delete tests need of it.
async fn create_mysql_option_group(
    router: &MultiServiceRouter,
    name: &str,
) -> (u16, String) {
    let majors = reported_major_versions(router, "mysql").await;
    call(
        router,
        "CreateOptionGroup",
        &params![
            "OptionGroupName" => name,
            "EngineName" => "mysql",
            "MajorEngineVersion" => &majors[0],
            "OptionGroupDescription" => "deletable",
        ],
    )
    .await
}

/// Whether `DescribeDBInstances` reports an instance under `identifier`: the question
/// "did the refused create leave a record behind?".
async fn instance_exists(router: &MultiServiceRouter, identifier: &str) -> bool {
    let (status, _) = call(
        router,
        "DescribeDBInstances",
        &params!["DBInstanceIdentifier" => identifier],
    )
    .await;
    status == 200
}

/// The engine versions THIS service reports for `engine`, read out of its own
/// `DescribeDBEngineVersions` report rather than written here by hand. Only the
/// AVAILABLE ones, which are the versions a create can name ("The default is to list
/// only available engine versions").
///
/// Read through the entry reader of the engine-version suite, so only the version of
/// each ENTRY is collected: a reported entry also carries the version of every upgrade
/// target it names, and those are the same versions again.
async fn reported_engine_versions(router: &MultiServiceRouter, engine: &str) -> Vec<String> {
    let body = expect_ok(
        engine,
        call(
            router,
            "DescribeDBEngineVersions",
            &params!["Engine" => engine],
        )
        .await,
    );
    crate::db_engine_versions::versions(&body)
        .into_iter()
        .map(|version| version.engine_version)
        .collect()
}

/// The major engine versions THIS service reports for `engine`, deduplicated, read out
/// of the same report.
async fn reported_major_versions(router: &MultiServiceRouter, engine: &str) -> Vec<String> {
    let body = expect_ok(
        engine,
        call(
            router,
            "DescribeDBEngineVersions",
            &params!["Engine" => engine],
        )
        .await,
    );
    let mut majors: Vec<String> = Vec::new();
    for major in texts(&body, "MajorEngineVersion") {
        if !majors.contains(&major) {
            majors.push(major);
        }
    }
    majors
}

// ---------------------------------------------------------------------------
// Rule 1 — a named resource is resolved in state before anything is written
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_resolves_the_db_subnet_group_it_names_and_persists_nothing_when_it_cannot() {
    let router = emulator();

    // Every row names a DB subnet group that was never created: different name shapes,
    // including one that differs from a group that DOES exist only in its suffix and
    // one that exists in another region only.
    expect_ok(
        "a group exists to be confused with",
        create_subnet_group_in(&router, REGION, "emu-guard-subnets").await,
    );
    expect_ok(
        "a group exists in another region",
        create_subnet_group_in(&router, "eu-central-1", "emu-elsewhere-subnets").await,
    );

    let absent: &[(&str, &str)] = &[
        ("a name nothing was created under", "emu-no-such-group"),
        ("a near miss of a stored name", "emu-guard-subnets-2"),
        ("a single letter", "e"),
        ("a group of another region", "emu-elsewhere-subnets"),
        ("a name the store never saw, mixed case", "EMU-Absent-Group"),
    ];
    for (index, (case, group)) in absent.iter().enumerate() {
        let identifier = format!("emu-sg-absent-{index}");
        let body = expect_error(
            case,
            create_instance(
                &router,
                &identifier,
                "mysql",
                A_CLASS,
                &params!["DBSubnetGroupName" => group],
            )
            .await,
            404,
            "DBSubnetGroupNotFoundFault",
        );
        let message = text(&body, "Message");
        assert!(
            message.to_ascii_lowercase().contains(&group.to_ascii_lowercase()),
            "case {case:?}: the refusal must name the group it could not resolve, got {message:?}"
        );
        assert!(
            !instance_exists(&router, &identifier).await,
            "case {case:?}: the refused create must leave no DB instance behind, but \
             DescribeDBInstances reported {identifier}"
        );
    }

    // And the three names that DO resolve: none at all, the account's default group,
    // and a group this service holds.
    let resolving: &[(&str, Option<&str>)] = &[
        ("no subnet group named", None),
        ("the account's default group", Some(DEFAULT_DB_SUBNET_GROUP)),
        ("a stored group", Some("emu-guard-subnets")),
        ("a stored group, named in another case", Some("EMU-Guard-Subnets")),
    ];
    for (index, (case, group)) in resolving.iter().enumerate() {
        let identifier = format!("emu-sg-ok-{index}");
        let extra: Params = match group {
            Some(group) => params!["DBSubnetGroupName" => group].to_vec(),
            None => Vec::new(),
        };
        expect_ok(
            case,
            create_instance(&router, &identifier, "mysql", A_CLASS, &extra).await,
        );
        assert!(
            instance_exists(&router, &identifier).await,
            "case {case:?}: the accepted create must be readable back through \
             DescribeDBInstances"
        );
    }
}

#[tokio::test]
async fn modify_resolves_the_db_subnet_group_it_moves_the_instance_to() {
    let router = emulator();
    expect_ok(
        "the target group is created",
        create_subnet_group_in(&router, REGION, "emu-move-subnets").await,
    );
    expect_ok(
        "the instance is created",
        create_instance(&router, "emu-move-me", "mysql", A_CLASS, &[]).await,
    );
    wait_for_settle();

    // A group that resolves to nothing is refused as a parameter error, and the stored
    // record is untouched: the following modify onto a group that DOES resolve must
    // still see the instance.
    for (case, group) in [
        ("a group nothing was created under", "emu-absent-move"),
        ("a near miss", "emu-move-subnets-2"),
    ] {
        expect_error(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-move-me",
                    "DBSubnetGroupName" => group,
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }
    for (case, group) in [
        ("the stored group", "emu-move-subnets"),
        ("the account's default group", DEFAULT_DB_SUBNET_GROUP),
    ] {
        let body = expect_ok(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-move-me",
                    // Moving the instance to another DB subnet group "causes an outage
                    // during the change. The change is applied during the next maintenance
                    // window, unless you enable ApplyImmediately" — and what this case is
                    // about is the group the instance ends up in.
                    "ApplyImmediately" => true,
                    "DBSubnetGroupName" => group,
                ],
            )
            .await,
        );
        let reported = text(&body, "DBSubnetGroupName");
        assert_eq!(
            reported, group,
            "case {case:?}: expected the stored DB subnet group {group}, got {reported:?}"
        );
        wait_for_settle();
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — a catalogue or form value is resolved before existence and conflict
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_resolves_the_engine_against_the_published_value_list() {
    let router = emulator();

    // Every engine the model's own `Engine` value list publishes is accepted, and the
    // case it is written in does not decide it. The sibling control planes' engines
    // (`docdb`, `neptune`) are published by THEIR models and tested with their own
    // cluster semantics in `docdb_query_surface`, so this table stays on this
    // service's list.
    for (index, engine) in DB_INSTANCE_ENGINES.iter().enumerate() {
        let identifier = format!("emu-engine-{index}");
        expect_ok(
            engine,
            create_instance(&router, &identifier, engine, A_CLASS, &[]).await,
        );
        let body = expect_ok(
            engine,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => &identifier],
            )
            .await,
        );
        let reported = text(&body, "Engine");
        assert_eq!(
            reported, *engine,
            "case {engine:?}: expected the stored engine {engine}, got {reported:?}"
        );
    }

    // A value that is not on that list names no database engine at all.
    let not_engines: &[(&str, &str)] = &[
        ("a product name, not an engine id", "MySQLCommunity"),
        ("an engine id with a version glued on", "mysql8.0"),
        ("an engine family that is not a value", "aurora"),
        ("a typo", "postgre"),
        ("a plausible but unpublished edition", "sqlserver-dev"),
        ("a path-looking value", "mysql-x"),
    ];
    for (index, (case, engine)) in not_engines.iter().enumerate() {
        let identifier = format!("emu-bad-engine-{index}");
        expect_error(
            case,
            create_instance(&router, &identifier, engine, A_CLASS, &[]).await,
            400,
            "InvalidParameterValue",
        );
        assert!(
            !instance_exists(&router, &identifier).await,
            "case {case:?}: the refused create must leave no DB instance behind"
        );
    }
}

#[tokio::test]
async fn create_and_modify_resolve_the_engine_version_against_the_report() {
    // The versions come from the service's OWN report, so this table cannot drift from
    // the catalogue it is asserting against. Each engine gets its own store: the rows
    // of one engine would otherwise count against the next engine's region quota.
    for engine in ["mysql", "postgres", "mariadb", "oracle-se2", "sqlserver-ex"] {
        let router = emulator();
        let reported = reported_engine_versions(&router, engine).await;
        assert!(
            !reported.is_empty(),
            "case {engine:?}: DescribeDBEngineVersions reported no version to create with"
        );
        for (index, version) in reported.iter().enumerate() {
            let identifier = format!("emu-ver-{engine}-{index}");
            let body = expect_ok(
                &format!("{engine} {version}"),
                create_instance(
                    &router,
                    &identifier,
                    engine,
                    A_CLASS,
                    &params!["EngineVersion" => version],
                )
                .await,
            );
            let stored = text(&body, "EngineVersion");
            assert_eq!(
                stored, *version,
                "case {engine:?} {version:?}: expected the stored engine version {version}, \
                 got {stored:?}"
            );
        }

        // A version the report does not carry cannot be run, whatever its shape.
        let absent = [
            format!("{}99", reported[0]),
            "0.0.0".to_string(),
            "99.99.99".to_string(),
        ];
        for (index, version) in absent.iter().enumerate() {
            let case = format!("{engine} {version}");
            let identifier = format!("emu-badver-{engine}-{index}");
            expect_error(
                &case,
                create_instance(
                    &router,
                    &identifier,
                    engine,
                    A_CLASS,
                    &params!["EngineVersion" => version],
                )
                .await,
                400,
                "InvalidParameterCombination",
            );
            assert!(
                !instance_exists(&router, &identifier).await,
                "case {case:?}: the refused create must leave no DB instance behind"
            );
        }
    }

    // The same resolution on the modify path, where the engine the version belongs to
    // is the one the modification leaves the instance on.
    let router = emulator();
    let versions = reported_engine_versions(&router, "postgres").await;
    expect_ok(
        "an instance to modify",
        create_instance(&router, "emu-modver", "postgres", A_CLASS, &[]).await,
    );
    wait_for_settle();
    expect_error(
        "a version postgres does not have",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-modver",
                "EngineVersion" => "0.0.1",
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    let body = expect_ok(
        "a version postgres does have",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-modver",
                // An engine-version change is "applied during the next maintenance window
                // unless the ApplyImmediately parameter is enabled for this request", and
                // what this case is about is the version the instance ends up running.
                "ApplyImmediately" => true,
                "EngineVersion" => &versions[0],
            ],
        )
        .await,
    );
    let stored = text(&body, "EngineVersion");
    assert_eq!(
        stored, versions[0],
        "expected the modification to store engine version {}, got {stored:?}",
        versions[0]
    );
}

#[tokio::test]
async fn create_refuses_a_value_that_is_not_a_db_instance_class_before_any_conflict() {
    let router = emulator();

    // One row per shape the published class form admits: each named size, a numbered
    // xlarge, a family with a generation suffix, an optimised class with trailing
    // tokens, and the Aurora Serverless capacity class.
    let mut well_formed: Vec<String> = INSTANCE_SIZES
        .iter()
        .map(|size| format!("db.m5.{size}"))
        .collect();
    well_formed.extend([
        "db.m5.2xlarge".to_string(),
        "db.x2iedn.32xlarge".to_string(),
        "db.m6i.large".to_string(),
        "db.r5b.4xlarge.tpc2.mem8x".to_string(),
        SERVERLESS_INSTANCE_CLASS.to_string(),
    ]);
    for (index, class) in well_formed.iter().enumerate() {
        let identifier = format!("emu-class-{index}");
        let body = expect_ok(
            class,
            create_instance(&router, &identifier, "mysql", class, &[]).await,
        );
        let stored = text(&body, "DBInstanceClass");
        assert_eq!(
            stored, *class,
            "case {class:?}: expected the stored class {class}, got {stored:?}"
        );
    }

    // And one row per way a value fails that form — the size token above all, which is
    // what distinguishes a class from a value that merely looks like one.
    let malformed: &[(&str, &str)] = &[
        // A size that is not a size, in each way a value can fail to be one.
        ("a size that is not a size", "db.t4g.nonexistent"),
        ("no size at all", "db.t4g"),
        ("no db prefix", "t3.micro"),
        ("an empty family", "db..large"),
        ("an empty size", "db.m5."),
        ("a size with a suffix", "db.m5.largest"),
        ("an EC2 instance type", "m5.large"),
        ("a number where a size belongs", "db.m5.4"),
        ("a size with too many digits", "db.m5.1234xlarge"),
        ("a zero-size xlarge", "db.m5.0xlarge"),
        ("a zero-led xlarge", "db.m5.04xlarge"),
    ];
    for (index, (case, class)) in malformed.iter().enumerate() {
        let identifier = format!("emu-badclass-{index}");
        let body = expect_error(
            case,
            create_instance(&router, &identifier, "mysql", class, &[]).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(class),
            "case {case:?}: the refusal must name the class it rejected, got {message:?}"
        );
        assert!(
            !instance_exists(&router, &identifier).await,
            "case {case:?}: the refused create must leave no DB instance behind"
        );
    }

    // The ordering the rule requires: a request that is wrong in BOTH its class and its
    // identifier (which is already taken) is refused for the class, so the caller is
    // told what it can actually fix.
    expect_ok(
        "an instance to collide with",
        create_instance(&router, "emu-taken", "mysql", A_CLASS, &[]).await,
    );
    expect_error(
        "a taken identifier AND an impossible class",
        create_instance(&router, "emu-taken", "mysql", "db.t4g.nonexistent", &[]).await,
        400,
        "InvalidParameterValue",
    );
    // With a class of the published form, the same identifier is the conflict it is.
    expect_error(
        "a taken identifier alone",
        create_instance(&router, "emu-taken", "mysql", A_CLASS, &[]).await,
        400,
        "DBInstanceAlreadyExists",
    );
}

#[tokio::test]
async fn create_option_group_resolves_the_major_engine_version_against_the_report() {
    let router = emulator();

    // Every engine an option group may be associated with, against the major versions
    // the service's own engine-version report carries for it.
    for (index, engine) in OPTION_GROUP_ENGINE_NAMES.iter().enumerate() {
        let majors = reported_major_versions(&router, engine).await;
        assert!(
            !majors.is_empty(),
            "case {engine:?}: DescribeDBEngineVersions reported no major version for an \
             engine option groups support"
        );
        let name = format!("emu-og-{index}");
        expect_ok(
            engine,
            call(
                &router,
                "CreateOptionGroup",
                &params![
                    "OptionGroupName" => &name,
                    "EngineName" => engine,
                    "MajorEngineVersion" => &majors[0],
                    "OptionGroupDescription" => "guarded",
                ],
            )
            .await,
        );
        // Read it back out of the store through the implemented describe.
        let body = expect_ok(
            engine,
            call(
                &router,
                "DescribeOptionGroups",
                &params!["OptionGroupName" => &name],
            )
            .await,
        );
        let stored = text(&body, "MajorEngineVersion");
        assert_eq!(
            stored, majors[0],
            "case {engine:?}: expected the stored major version {}, got {stored:?}",
            majors[0]
        );

        // A major version that engine has no entry for is a combination this service
        // cannot associate — and nothing may be persisted under it.
        for (offset, major) in ["9.9", "0.0", "2999"].iter().enumerate() {
            let case = format!("{engine} major {major}");
            let refused = format!("emu-og-bad-{index}-{offset}");
            expect_error(
                &case,
                call(
                    &router,
                    "CreateOptionGroup",
                    &params![
                        "OptionGroupName" => &refused,
                        "EngineName" => engine,
                        "MajorEngineVersion" => major,
                        "OptionGroupDescription" => "guarded",
                    ],
                )
                .await,
                400,
                "InvalidParameterCombination",
            );
            expect_error(
                &case,
                call(
                    &router,
                    "DescribeOptionGroups",
                    &params!["OptionGroupName" => &refused],
                )
                .await,
                404,
                "OptionGroupNotFoundFault",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — a Marker no prior page issued is refused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_paginated_describe_refuses_a_marker_it_never_issued() {
    let router = emulator();
    // State for the describes that report records rather than reference data.
    expect_ok(
        "a subnet group to page over",
        create_subnet_group_in(&router, REGION, "emu-page-subnets").await,
    );
    expect_ok(
        "an option group to page over",
        call(
            &router,
            "CreateOptionGroup",
            &params![
                "OptionGroupName" => "emu-page-og",
                "EngineName" => "mysql",
                "MajorEngineVersion" => "8.0",
                "OptionGroupDescription" => "paged",
            ],
        )
        .await,
    );
    expect_ok(
        "an instance to page over",
        create_instance(&router, "emu-page-inst", "mysql", A_CLASS, &[]).await,
    );
    wait_for_settle();
    expect_ok(
        "a snapshot to page over",
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-page-inst",
                "DBSnapshotIdentifier" => "emu-page-snap",
            ],
        )
        .await,
    );

    // Every paginated describe this service implements, against markers no page of it
    // can have issued: one that is not a key at all, one that is a key of ANOTHER
    // report, one that is empty, and one that is a plausible-looking but unissued key.
    let describes: &[(&str, Params)] = &[
        ("DescribeDBEngineVersions", Vec::new()),
        (
            "DescribeOrderableDBInstanceOptions",
            params!["Engine" => "mysql"].to_vec(),
        ),
        ("DescribeDBInstances", Vec::new()),
        ("DescribeDBSnapshots", Vec::new()),
        ("DescribeOptionGroups", Vec::new()),
        ("DescribeDBSubnetGroups", Vec::new()),
        ("DescribeCertificates", Vec::new()),
    ];
    let unissued = [
        "not-a-marker",
        "emu-page-inst-but-longer",
        "zzzzzzzz",
        "mysql#0.0.0",
    ];
    for (action, base) in describes {
        for marker in unissued {
            let case = format!("{action} resuming at {marker:?}");
            let mut request = base.clone();
            request.push(("Marker", marker.to_string()));
            expect_error(&case, call(&router, action, &request).await, 400, "InvalidParameterValue");
        }
    }

    // A marker a prior page DID issue resumes the report: the second page starts at the
    // record the marker names and reports records the first page did not. The member
    // compared is one that differs between records of that report.
    for (action, base, compare) in [
        ("DescribeDBEngineVersions", Vec::new(), "EngineVersion"),
        (
            "DescribeOrderableDBInstanceOptions",
            params!["Engine" => "mysql"].to_vec(),
            "DBInstanceClass",
        ),
    ] {
        let mut first_request = base.clone();
        first_request.push(("MaxRecords", "20".to_string()));
        let first = expect_ok(action, call(&router, action, &first_request).await);
        let marker = text(&first, "Marker");
        assert!(
            !marker.is_empty(),
            "case {action:?}: the first page must carry the Marker the next one resumes at"
        );
        let mut second_request = base.clone();
        second_request.push(("MaxRecords", "20".to_string()));
        second_request.push(("Marker", marker.clone()));
        let second = expect_ok(action, call(&router, action, &second_request).await);
        let first_page = texts(&first, compare);
        let second_page = texts(&second, compare);
        assert!(
            !second_page.is_empty(),
            "case {action:?}: resuming at an issued marker must report the records that \
             remain, got none"
        );
        assert!(
            second_page.iter().any(|entry| !first_page.contains(entry)),
            "case {action:?}: the second page must report records the first did not; \
             first {first_page:?}, second {second_page:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The DB subnet group slice the guards resolve against, and its own refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db_subnet_groups_round_trip_and_refuse_what_cannot_be_a_group() {
    let router = emulator();

    let created = expect_ok(
        "a group with two subnets",
        create_subnet_group_in(&router, REGION, "emu-rt-subnets").await,
    );
    {
        let name = text(&created, "DBSubnetGroupName");
        assert_eq!(
            name, "emu-rt-subnets",
            "CreateDBSubnetGroup reported DB subnet group name {name:?}"
        );
    }
    // The same record, read back out of the store.
    let read_back = expect_ok(
        "the group reads back",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &params!["DBSubnetGroupName" => "emu-rt-subnets"],
        )
        .await,
    );
    let subnets = texts(&read_back, "SubnetIdentifier");
    let expected: Vec<String> = TWO_SUBNETS
        .iter()
        .map(|(_, value)| (*value).to_string())
        .collect();
    assert_eq!(
        subnets, expected,
        "expected the stored subnets {expected:?}, got {subnets:?}"
    );
    let status = text(&read_back, "SubnetGroupStatus");
    assert_eq!(
        status, "Complete",
        "expected the stored group's status, got {status:?}"
    );
    let arn = text(&read_back, "DBSubnetGroupArn");
    assert_eq!(
        arn, "arn:aws:rds:us-west-2:000000000000:subgrp:emu-rt-subnets",
        "expected the ARN computed from the stored region and name, got {arn:?}"
    );

    // The account's default group resolves and reports, and the unnamed report carries
    // both it and the stored one, in name order.
    let all = expect_ok(
        "every visible group",
        call(&router, "DescribeDBSubnetGroups", &Vec::<(&str, String)>::new()).await,
    );
    let names = texts(&all, "DBSubnetGroupName");
    assert_eq!(
        names,
        vec![
            DEFAULT_DB_SUBNET_GROUP.to_string(),
            "emu-rt-subnets".to_string()
        ],
        "expected the default group and the stored one in name order, got {names:?}"
    );

    // A name nothing was created under is the modelled 404, not an empty list.
    expect_error(
        "a group nothing created",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &params!["DBSubnetGroupName" => "emu-absent-subnets"],
        )
        .await,
        404,
        "DBSubnetGroupNotFoundFault",
    );

    // One row per documented constraint on the create.
    expect_error(
        "the same name twice",
        create_subnet_group_in(&router, REGION, "emu-rt-subnets").await,
        400,
        "DBSubnetGroupAlreadyExists",
    );
    for (case, name) in [
        ("the reserved name", DEFAULT_DB_SUBNET_GROUP),
        ("a name starting with a digit", "9-subnets"),
        ("a name with a character the member excludes", "emu/subnets"),
    ] {
        expect_error(
            case,
            create_subnet_group_in(&router, REGION, name).await,
            400,
            "InvalidParameterValue",
        );
    }
    for (case, subnets) in [
        ("one subnet", vec!["subnet-0a1b2c3d"]),
        (
            "the same subnet twice",
            vec!["subnet-0a1b2c3d", "subnet-0a1b2c3d"],
        ),
    ] {
        let mut request = params![
            "DBSubnetGroupName" => "emu-one-az",
            "DBSubnetGroupDescription" => "one-az",
        ]
        .to_vec();
        for (index, subnet) in subnets.iter().enumerate() {
            request.push((
                match index {
                    0 => "SubnetIds.SubnetIdentifier.1",
                    _ => "SubnetIds.SubnetIdentifier.2",
                },
                (*subnet).to_string(),
            ));
        }
        expect_error(
            case,
            call(&router, "CreateDBSubnetGroup", &request).await,
            400,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        );
    }
    for (case, subnet) in [
        ("a subnet id of the wrong length", "subnet-0a1b"),
        ("an uppercase subnet id", "subnet-0A1B2C3D"),
        ("another resource's id", "vpc-0a1b2c3d"),
        ("a bare id", "0a1b2c3d"),
    ] {
        let mut request = params![
            "DBSubnetGroupName" => "emu-bad-subnet",
            "DBSubnetGroupDescription" => "bad-subnet",
            "SubnetIds.SubnetIdentifier.1" => subnet,
        ]
        .to_vec();
        request.push((
            "SubnetIds.SubnetIdentifier.2",
            "subnet-1b2c3d4e".to_string(),
        ));
        expect_error(
            case,
            call(&router, "CreateDBSubnetGroup", &request).await,
            400,
            "InvalidSubnet",
        );
    }
    // One past the published number of subnets per group. The parameter NAMES are
    // numbered, so this request is built as owned pairs rather than from the macro.
    let mut too_many: Vec<(String, String)> = vec![
        (
            "DBSubnetGroupName".to_string(),
            "emu-too-many-subnets".to_string(),
        ),
        (
            "DBSubnetGroupDescription".to_string(),
            "too-many".to_string(),
        ),
    ];
    for index in 1..=MAX_SUBNETS_PER_DB_SUBNET_GROUP + 1 {
        too_many.push((
            format!("SubnetIds.SubnetIdentifier.{index}"),
            format!("subnet-{index:08x}"),
        ));
    }
    expect_error(
        "one subnet past the published allowance",
        call(&router, "CreateDBSubnetGroup", &too_many).await,
        400,
        "DBSubnetQuotaExceededFault",
    );
}

// ---------------------------------------------------------------------------
// DeleteOptionGroup — the delete that makes a later describe observe the deletion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_option_group_removes_it_and_refuses_the_absent_and_the_in_use() {
    let router = emulator();

    expect_ok("a group to delete", create_mysql_option_group(&router, "emu-del-og").await);
    expect_ok(
        "the delete",
        call(
            &router,
            "DeleteOptionGroup",
            &params!["OptionGroupName" => "emu-del-og"],
        )
        .await,
    );
    // The deletion is observable: the describe that reported the group now 404s.
    expect_error(
        "the describe after the delete",
        call(
            &router,
            "DescribeOptionGroups",
            &params!["OptionGroupName" => "emu-del-og"],
        )
        .await,
        404,
        "OptionGroupNotFoundFault",
    );
    // And so is a second delete of the same name.
    for (case, name, region) in [
        ("the same group twice", "emu-del-og", REGION),
        ("a group nothing created", "emu-never-og", REGION),
        ("a group of another region", "emu-del-og", "eu-central-1"),
    ] {
        expect_error(
            case,
            call_in(
                &router,
                region,
                "DeleteOptionGroup",
                &params!["OptionGroupName" => name],
            )
            .await,
            404,
            "OptionGroupNotFoundFault",
        );
    }

    // A group an instance is using cannot be deleted, and the guard reads the store:
    // once the instance is gone the same delete succeeds.
    expect_ok("a group in use", create_mysql_option_group(&router, "emu-used-og").await);
    expect_ok(
        "the instance using it",
        create_instance(
            &router,
            "emu-og-user",
            "mysql",
            A_CLASS,
            &params!["OptionGroupName" => "emu-used-og"],
        )
        .await,
    );
    wait_for_settle();
    let body = expect_error(
        "the group while it is in use",
        call(
            &router,
            "DeleteOptionGroup",
            &params!["OptionGroupName" => "emu-used-og"],
        )
        .await,
        400,
        "InvalidOptionGroupStateFault",
    );
    // The refusal is worded from the REQUEST alone: it names the group the delete
    // addressed, single-quoted, and says it is in use — it does not report WHICH instance
    // holds it.
    let message = text(&body, "Message");
    assert_eq!(
        message,
        "The option group 'emu-used-og' cannot be deleted because it is in use.",
        "the refusal must carry the in-use sentence for the requested group, got {message:?}"
    );
    assert!(
        !message.contains("emu-og-user"),
        "the refusal must not disclose the instance that is using the group, got {message:?}"
    );
    expect_ok(
        "the instance is deleted",
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-og-user",
                "SkipFinalSnapshot" => "true",
            ],
        )
        .await,
    );
    // The delete leaves the instance `deleting`; it is gone once that settles, and only
    // then does nothing use the group.
    wait_for_settle();
    expect_ok(
        "the group once nothing uses it",
        call(
            &router,
            "DeleteOptionGroup",
            &params!["OptionGroupName" => "emu-used-og"],
        )
        .await,
    );
}
