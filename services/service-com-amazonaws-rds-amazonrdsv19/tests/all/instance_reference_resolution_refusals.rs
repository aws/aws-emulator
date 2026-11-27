// SPDX-License-Identifier: CC0-1.0

//! What the DB instance create and modification paths REFUSE: the references a request
//! carries that resolve to nothing, the values outside what a member publishes, and the
//! members that cannot hold together.
//!
//! Four rules, each asserted over a table that spans what it generalises over rather than
//! one recorded request:
//!
//! * An `OptionGroupName` a request names must resolve to an option group the account can
//!   see — one a request created, or the default group AWS provides for an engine major
//!   version — and a name that resolves to neither is `OptionGroupNotFoundFault` (404).
//! * A `StorageType` a request names must be one the value list
//!   `CreateDBInstanceMessage$StorageType` publishes for the engine ("Valid Values: `gp2 |
//!   gp3 | io1 | io2 | standard`", with the cluster-managed types carved out for the engines
//!   whose "storage is managed by the DB cluster"), and any other value is
//!   `StorageTypeNotSupportedFault` (400).
//! * `DBSecurityGroups` is the EC2-Classic association, so it cannot be given to a DB
//!   instance placed in a VPC — a request that names a DB subnet group and a DB security
//!   group at once is `InvalidParameterCombination` (400) — and a name it does carry has to
//!   resolve to a DB security group of the account.
//! * `ModifyDBInstance$Engine` names ONE change, converting an Oracle non-CDB instance to a
//!   container database, so a value that asks for another engine's database over this
//!   instance's storage is `InvalidParameterCombination` (400).
//!
//! The refusal of a `CreateDBShardGroup` whose primary cluster is not an Aurora Limitless
//! Database cluster, and of a `DeleteDBClusterParameterGroup` whose group a DB cluster still
//! holds, are here too: both are a reference the caller has to reconcile before the call can
//! be served.
//!
//! Sources: `smithy_query(query="operation", operation="CreateDBInstance,ModifyDBInstance,
//! CreateDBShardGroup,DeleteDBClusterParameterGroup")` and `smithy_query(query="doc",
//! shape="CreateDBInstanceMessage", member="StorageType")`.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts};
use crate::lifecycle_poll_loops::poll_until;

/// The DB instance class every instance below is created with.
const CLASS: &str = "db.t3.medium";

/// The engine every instance below runs, unless its row names another: one whose storage is
/// its own, so the per-instance storage types apply to it.
const ENGINE: &str = "mysql";

/// `CreateDBInstance` with the three required members plus `extra`.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    create_instance_of_class(router, identifier, engine, CLASS, extra).await
}

/// [`create_instance`] on a DB instance class of its own, for an engine the smaller class
/// is not offered with.
async fn create_instance_of_class(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("DBInstanceClass", class.to_string()),
    ];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call(router, "CreateDBInstance", &params).await
}

/// `DescribeDBInstances` for one identifier, polled until the instance reports `available` —
/// the state a modification needs, and the one the clock owes a newly created instance.
async fn available_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let request = [("DBInstanceIdentifier", identifier.to_string())];
    let (body, _) = poll_until(
        case,
        &format!("DB instance {identifier} reaching available"),
        || call(router, "DescribeDBInstances", &request),
        |status, body| status == 200 && text(body, "DBInstanceStatus") == "available",
    )
    .await;
    body
}

/// `CreateOptionGroup` for a group the rows below name.
async fn create_option_group(
    router: &MultiServiceRouter,
    name: &str,
    engine_name: &str,
    major_engine_version: &str,
) {
    expect_ok(
        &format!("create the option group {name}"),
        call(
            router,
            "CreateOptionGroup",
            &[
                ("OptionGroupName", name),
                ("EngineName", engine_name),
                ("MajorEngineVersion", major_engine_version),
                ("OptionGroupDescription", "a group under test"),
            ],
        )
        .await,
    );
}

/// One row of the option-group table: the name a request carries, and what it must answer.
struct OptionGroupCase {
    case: &'static str,
    /// The `OptionGroupName` the create and the modification name.
    named: &'static str,
    /// `None` for a name that resolves, else the HTTP status and wire code it is refused
    /// with.
    refused: Option<(u16, &'static str)>,
}

/// The option-group names the rules generalise over: a group a request created, the two
/// default groups AWS provides for an engine major version (`default:mysql-8-0` is the form
/// `DescribeOptionGroups` reports for the MySQL 8.0 group), and names nothing resolves — one
/// that was never created, one differing from a created group only in case (`OptionGroupName`
/// carries no "isn't case-sensitive" note, so it is matched as stored), and one of the
/// default form naming a major version no engine publishes.
const OPTION_GROUP_CASES: &[OptionGroupCase] = &[
    OptionGroupCase {
        case: "a group a request created",
        named: "emu-refusal-og",
        refused: None,
    },
    OptionGroupCase {
        case: "the default group of the engine's major version",
        named: "default:mysql-8-0",
        refused: None,
    },
    OptionGroupCase {
        case: "a name nothing created",
        named: "emu-refusal-og-absent",
        refused: Some((404, "OptionGroupNotFoundFault")),
    },
    OptionGroupCase {
        case: "a created group named in another case",
        named: "EMU-REFUSAL-OG",
        refused: Some((404, "OptionGroupNotFoundFault")),
    },
    OptionGroupCase {
        case: "a default-form name of a major version nothing publishes",
        named: "default:mysql-99-0",
        refused: Some((404, "OptionGroupNotFoundFault")),
    },
];

#[tokio::test]
async fn an_option_group_a_request_names_must_resolve_wherever_an_instance_takes_one() {
    let router = emulator();
    create_option_group(&router, "emu-refusal-og", "mysql", "8.0").await;
    // The instance every modification row below names, settled so a modification is served.
    expect_ok(
        "the instance the modifications name",
        create_instance(&router, "emu-refusal-og-instance", ENGINE, &[]).await,
    );
    available_instance(
        &router,
        "the instance the modifications name",
        "emu-refusal-og-instance",
    )
    .await;

    for (index, row) in OPTION_GROUP_CASES.iter().enumerate() {
        let case = row.case;
        // The CREATE: the group is resolved before the instance is admitted, so a refused
        // create leaves no instance behind. The identifier carries the row's own index:
        // `DBInstanceIdentifier` "is stored as a lowercase string", so two rows differing
        // only in case would otherwise name one instance.
        let identifier = format!("emu-refusal-og-{index}");
        let created = create_instance(
            &router,
            &identifier,
            ENGINE,
            &[("OptionGroupName", row.named)],
        )
        .await;
        match row.refused {
            None => {
                let body = expect_ok(case, created);
                assert_eq!(
                    texts(&body, "OptionGroupName"),
                    vec![row.named.to_string()],
                    "case {case:?}: the created instance must report the group it was \
                     placed in, got {body}"
                );
            }
            Some((status, code)) => {
                let refused = expect_error(case, created, status, code);
                assert_eq!(
                    text(&refused, "Message"),
                    format!("Specified OptionGroupName: {} not found.", row.named),
                    "case {case:?}: expected the sentence the not-found fault carries, \
                     got {refused}"
                );
                // Nothing was stored for the refused create.
                expect_error(
                    case,
                    call(
                        &router,
                        "DescribeDBInstances",
                        &[("DBInstanceIdentifier", identifier.as_str())],
                    )
                    .await,
                    404,
                    "DBInstanceNotFound",
                );
            }
        }

        // The MODIFICATION: the same rule, over an instance that is already there.
        let modified = call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-refusal-og-instance"),
                ("OptionGroupName", row.named),
                ("ApplyImmediately", "true"),
            ],
        )
        .await;
        match row.refused {
            None => {
                expect_ok(case, modified);
            }
            Some((status, code)) => {
                let refused = expect_error(case, modified, status, code);
                assert_eq!(
                    text(&refused, "Message"),
                    format!("Specified OptionGroupName: {} not found.", row.named),
                    "case {case:?}: expected the sentence the not-found fault carries, \
                     got {refused}"
                );
            }
        }
    }
}

/// One row of the storage-type table: the engine, the `StorageType` a request names, and
/// what it must answer.
struct StorageTypeCase {
    case: &'static str,
    engine: &'static str,
    storage_type: &'static str,
    /// Whether the value list the engine publishes carries the type.
    published: bool,
}

/// The storage types the rule generalises over: every value
/// `CreateDBInstanceMessage$StorageType` publishes for an instance whose storage is its own
/// ("Valid Values: `gp2 | gp3 | io1 | io2 | standard`"), one of them in another case, the
/// types a CLUSTER manages (which that member carves out for the engines whose "storage is
/// managed by the DB cluster"), and values no engine publishes at all.
const STORAGE_TYPE_CASES: &[StorageTypeCase] = &[
    StorageTypeCase {
        case: "general purpose SSD",
        engine: "mysql",
        storage_type: "gp2",
        published: true,
    },
    StorageTypeCase {
        case: "the newer general purpose SSD",
        engine: "postgres",
        storage_type: "gp3",
        published: true,
    },
    StorageTypeCase {
        case: "provisioned IOPS SSD",
        engine: "mysql",
        storage_type: "io1",
        published: true,
    },
    StorageTypeCase {
        case: "provisioned IOPS SSD block express",
        engine: "mysql",
        storage_type: "io2",
        published: true,
    },
    StorageTypeCase {
        case: "the legacy magnetic type the member still lists",
        engine: "mysql",
        storage_type: "standard",
        published: true,
    },
    StorageTypeCase {
        case: "a published type in another case",
        engine: "mysql",
        storage_type: "GP3",
        published: true,
    },
    StorageTypeCase {
        case: "a type nothing publishes",
        engine: "mysql",
        storage_type: "turbo-ssd-9000",
        published: false,
    },
    StorageTypeCase {
        case: "a type that is not a type at all",
        engine: "postgres",
        storage_type: "gp4",
        published: false,
    },
    StorageTypeCase {
        case: "the storage a DB cluster manages, on an instance that owns its own",
        engine: "mysql",
        storage_type: "aurora",
        published: false,
    },
    StorageTypeCase {
        case: "the io-optimized storage a DB cluster manages",
        engine: "postgres",
        storage_type: "aurora-iopt1",
        published: false,
    },
];

#[tokio::test]
async fn a_storage_type_the_engine_does_not_publish_is_refused_wherever_a_request_names_one() {
    let router = emulator();
    // The instance every modification row below names, settled so a modification is served.
    expect_ok(
        "the instance the modifications name",
        create_instance(&router, "emu-refusal-st-instance", ENGINE, &[]).await,
    );
    available_instance(
        &router,
        "the instance the modifications name",
        "emu-refusal-st-instance",
    )
    .await;

    for (index, row) in STORAGE_TYPE_CASES.iter().enumerate() {
        let case = row.case;
        let identifier = format!("emu-refusal-st-{index}");
        let created = create_instance(
            &router,
            &identifier,
            row.engine,
            &[("StorageType", row.storage_type)],
        )
        .await;
        if row.published {
            let body = expect_ok(case, created);
            assert_eq!(
                text(&body, "StorageType"),
                row.storage_type,
                "case {case:?}: the created instance must report the type it was \
                 provisioned on, got {body}"
            );
        } else {
            let refused = expect_error(case, created, 400, "StorageTypeNotSupported");
            assert_eq!(
                text(&refused, "Message"),
                format!("Invalid storage type: {}", row.storage_type),
                "case {case:?}: expected the sentence this refusal carries, got {refused}"
            );
            // Nothing was stored for the refused create.
            expect_error(
                case,
                call(
                    &router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", identifier.as_str())],
                )
                .await,
                404,
                "DBInstanceNotFound",
            );
        }

        // The MODIFICATION is held to the same list, against the engine the instance runs.
        let modified = call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-refusal-st-instance"),
                ("StorageType", row.storage_type),
                ("ApplyImmediately", "true"),
            ],
        )
        .await;
        // Only the rows of the instance's OWN engine say anything about its modification:
        // the list is the engine's, and this instance runs `mysql`.
        if row.engine == ENGINE {
            if row.published {
                expect_ok(case, modified);
            } else {
                let refused = expect_error(case, modified, 400, "StorageTypeNotSupported");
                assert_eq!(
                    text(&refused, "Message"),
                    format!("Invalid storage type: {}", row.storage_type),
                    "case {case:?}: expected the sentence this refusal carries, got {refused}"
                );
            }
        }
    }
}

/// The sentence Amazon RDS refuses a DB security group on a VPC DB instance with.
const DB_SECURITY_GROUPS_IN_VPC: &str = "DB Security Groups can only be associated with VPC \
     DB Instances using API versions 2012-01-15 through 2012-04-23.";

#[tokio::test]
async fn a_db_security_group_is_refused_for_every_db_instance_a_create_places_in_a_vpc() {
    let router = emulator();
    expect_ok(
        "the DB security group the rows name",
        call(
            &router,
            "CreateDBSecurityGroup",
            &[
                ("DBSecurityGroupName", "emu-refusal-dbsg"),
                ("DBSecurityGroupDescription", "a group under test"),
            ],
        )
        .await,
    );
    expect_ok(
        "the DB subnet group some rows name",
        call(
            &router,
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", "emu-refusal-subnets"),
                ("DBSubnetGroupDescription", "two subnets"),
                ("SubnetIds.SubnetIdentifier.1", "subnet-0a1b2c3d"),
                ("SubnetIds.SubnetIdentifier.2", "subnet-1b2c3d4e"),
            ],
        )
        .await,
    );

    // Every DB instance a create places in a VPC, which is every instance this service
    // creates: the rows differ along what the rule is indifferent to — whether the group the
    // request names is one the account holds, the spelling it names it in, how many it names,
    // and whether the request names a DB subnet group of its own or leaves the instance in
    // the account's default group.
    struct SecurityGroupCase {
        case: &'static str,
        /// The `DBSecurityGroups` entries the create names.
        named: &'static [&'static str],
        /// Whether the create also names a DB subnet group of its own.
        names_a_subnet_group: bool,
    }
    let rows: &[SecurityGroupCase] = &[
        SecurityGroupCase {
            case: "a stored group, on an instance in the account's default group",
            named: &["emu-refusal-dbsg"],
            names_a_subnet_group: false,
        },
        SecurityGroupCase {
            case: "a stored group named in another case",
            named: &["EMU-REFUSAL-DBSG"],
            names_a_subnet_group: false,
        },
        SecurityGroupCase {
            case: "a group nothing created",
            named: &["emu-refusal-dbsg-absent"],
            names_a_subnet_group: false,
        },
        SecurityGroupCase {
            case: "a stored group, on an instance placed in a named DB subnet group",
            named: &["emu-refusal-dbsg"],
            names_a_subnet_group: true,
        },
        SecurityGroupCase {
            case: "two groups, one of them stored",
            named: &["emu-refusal-dbsg", "emu-refusal-dbsg-absent"],
            names_a_subnet_group: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let identifier = format!("emu-refusal-dbsg-{index}");
        let members: Vec<(String, &str)> = row
            .named
            .iter()
            .enumerate()
            .map(|(position, named)| {
                (
                    format!("DBSecurityGroups.DBSecurityGroupName.{}", position + 1),
                    *named,
                )
            })
            .collect();
        let mut extra: Vec<(&str, &str)> = members
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect();
        if row.names_a_subnet_group {
            extra.push(("DBSubnetGroupName", "emu-refusal-subnets"));
        }
        let answer = create_instance(&router, &identifier, ENGINE, &extra).await;
        let refused = expect_error(case, answer, 400, "InvalidParameterCombination");
        assert_eq!(
            text(&refused, "Message"),
            DB_SECURITY_GROUPS_IN_VPC,
            "case {case:?}: expected the sentence this combination is refused with, \
             got {refused}"
        );
        // Nothing was stored for the refused create.
        expect_error(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier.as_str())],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
    }

    // And a create that names NO DB security group is served, with the membership list
    // reported empty: the refusal is about the member the request carried, not about the
    // instance.
    let served = expect_ok(
        "a create that names no DB security group",
        create_instance(&router, "emu-refusal-dbsg-none", ENGINE, &[]).await,
    );
    assert!(
        texts(&served, "DBSecurityGroupName").is_empty(),
        "a VPC DB instance holds no DB security group, got {served}"
    );
}

#[tokio::test]
async fn the_engine_a_modification_names_may_only_convert_an_oracle_instance_to_a_container_db() {
    let router = emulator();

    // One row per instance the modifications act on: the engine it runs, and the engines a
    // modification of it admits and refuses. `ModifyDBInstance$Engine` is "the target Oracle
    // DB engine when you convert a non-CDB to a CDB", where "you specify `oracle-ee-cdb` or
    // `oracle-se2-cdb`" — so the admitted values are the CDB form of the edition the instance
    // runs and the engine it already runs.
    let rows: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "an Oracle Enterprise Edition instance",
            "oracle-ee",
            &["oracle-ee", "oracle-ee-cdb"],
            &["oracle-se2-cdb", "mysql", "postgres"],
        ),
        (
            "an Oracle Standard Edition Two instance",
            "oracle-se2",
            &["oracle-se2", "oracle-se2-cdb"],
            &["oracle-ee-cdb", "mariadb"],
        ),
        (
            "a MySQL instance",
            "mysql",
            &["mysql"],
            &["postgres", "mariadb", "oracle-ee-cdb", "aurora-mysql"],
        ),
        (
            "a PostgreSQL instance",
            "postgres",
            &["postgres"],
            &["mysql", "oracle-se2-cdb", "sqlserver-ex"],
        ),
    ];

    for (index, (case, engine, admitted, refused)) in rows.iter().enumerate() {
        let identifier = format!("emu-refusal-engine-{index}");
        // Oracle is offered on the general-purpose classes rather than the burstable one the
        // other rows run, and on a volume of at least the Provisioned IOPS floor.
        expect_ok(
            case,
            create_instance_of_class(
                &router,
                &identifier,
                engine,
                "db.m5.large",
                &[("AllocatedStorage", "100")],
            )
            .await,
        );
        available_instance(&router, case, &identifier).await;

        for target in *refused {
            let answer = call(
                &router,
                "ModifyDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("Engine", target),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await;
            let body = expect_error(
                &format!("{case}: onto {target}"),
                answer,
                400,
                "InvalidParameterCombination",
            );
            let message = text(&body, "Message");
            assert!(
                message.contains(target) && message.contains(engine),
                "case {case:?} onto {target:?}: expected the refusal to name the engine \
                 asked for and the one the instance runs, got {message:?}"
            );
            // The refused modification changed nothing: the instance still runs its engine.
            let described = expect_ok(
                &format!("{case}: after the refusal"),
                call(
                    &router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", identifier.as_str())],
                )
                .await,
            );
            assert_eq!(
                text(&described, "Engine"),
                *engine,
                "case {case:?}: a refused modification must leave the engine as it stood, \
                 got {described}"
            );
        }
        for target in *admitted {
            expect_ok(
                &format!("{case}: onto {target}"),
                call(
                    &router,
                    "ModifyDBInstance",
                    &[
                        ("DBInstanceIdentifier", identifier.as_str()),
                        ("Engine", target),
                        ("ApplyImmediately", "true"),
                    ],
                )
                .await,
            );
        }
    }
}

/// The Aurora PostgreSQL engine version Aurora Limitless Database is OFFERED on, read out of
/// the engine-version report this service publishes — the row that reports
/// `SupportsLimitlessDatabase` — exactly as a client picks one, rather than written here.
///
/// A DB shard group holds the shards of a Limitless database, which no other build offers, so
/// every suite that creates one creates its primary cluster on this version. `pub(crate)` so
/// the sibling shard-group suites read the one answer rather than growing a copy of this
/// lookup.
pub(crate) async fn limitless_engine_version(router: &MultiServiceRouter) -> String {
    let published = expect_ok(
        "the engine versions the catalogue publishes",
        call(
            router,
            "DescribeDBEngineVersions",
            &[("Engine", "aurora-postgresql"), ("MaxRecords", "100")],
        )
        .await,
    );
    subtrees(&published, "DBEngineVersion")
        .into_iter()
        .find(|version| {
            optional_text(version, "SupportsLimitlessDatabase").as_deref() == Some("true")
        })
        .map(|version| text(&version, "EngineVersion"))
        .unwrap_or_else(|| {
            panic!(
                "the catalogue must publish an aurora-postgresql version that supports Aurora \
                 Limitless Database, got {published}"
            )
        })
}

#[tokio::test]
async fn a_shard_group_is_served_only_for_a_cluster_that_runs_a_limitless_database() {
    let router = emulator();
    let limitless_version = limitless_engine_version(&router).await;

    // One row per cluster a shard group is asked for: the scalability mode it was created in
    // and the engine version it runs, both of which Aurora Limitless Database needs. A shard
    // group holds the shards of a Limitless database, so a cluster that runs none — because
    // its `ClusterScalabilityType` is not `limitless`, or because the build it runs does not
    // offer Limitless at all — cannot take one. The rows span the mode named explicitly, the
    // documented default a create that names none takes, and a Limitless cluster on a build
    // that offers no Limitless database.
    struct ShardGroupCase {
        case: &'static str,
        /// The `ClusterScalabilityType` the cluster create names, if any.
        scalability: Option<&'static str>,
        /// Whether the cluster is created on the build that offers Aurora Limitless Database.
        limitless_version: bool,
        /// Whether the shard group create is served, and the sentence it is refused with
        /// otherwise.
        served: bool,
    }
    let rows: &[ShardGroupCase] = &[
        ShardGroupCase {
            case: "a limitless cluster on a limitless build",
            scalability: Some("limitless"),
            limitless_version: true,
            served: true,
        },
        ShardGroupCase {
            case: "a standard cluster on a limitless build",
            scalability: Some("standard"),
            limitless_version: true,
            served: false,
        },
        ShardGroupCase {
            case: "a cluster that named no mode, on a limitless build",
            scalability: None,
            limitless_version: true,
            served: false,
        },
        ShardGroupCase {
            case: "a limitless cluster on a build that offers no limitless database",
            scalability: Some("limitless"),
            limitless_version: false,
            served: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let cluster = format!("emu-refusal-cluster-{index}");
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", cluster.clone()),
            ("Engine", "aurora-postgresql".to_string()),
        ];
        if let Some(scalability) = row.scalability {
            params.push(("ClusterScalabilityType", scalability.to_string()));
            params.push(("StorageType", "aurora-iopt1".to_string()));
        }
        if row.limitless_version {
            params.push(("EngineVersion", limitless_version.clone()));
        }
        expect_ok(case, call(&router, "CreateDBCluster", &params).await);

        let shard_group = format!("emu-refusal-shards-{index}");
        let answer = call(
            &router,
            "CreateDBShardGroup",
            &[
                ("DBShardGroupIdentifier", shard_group.as_str()),
                ("DBClusterIdentifier", cluster.as_str()),
                ("MaxACU", "64"),
            ],
        )
        .await;
        if row.served {
            let body = expect_ok(case, answer);
            assert_eq!(
                text(&body, "DBClusterIdentifier"),
                cluster,
                "case {case:?}: the shard group must name the cluster it was created in, \
                 got {body}"
            );
        } else {
            let refused = expect_error(case, answer, 400, "InvalidParameterCombination");
            let message = text(&refused, "Message");
            // A cluster on a build that offers no Limitless database is refused for the
            // BUILD, naming the version it runs; one on a Limitless build is refused for the
            // mode it was created in, naming the cluster.
            let names = match row.limitless_version {
                true => message.contains(cluster.as_str()) && message.contains("limitless"),
                false => message.contains("Aurora Limitless Database"),
            };
            assert!(
                names,
                "case {case:?}: expected the refusal to name what the cluster would need, \
                 got {message:?}"
            );
            // Nothing was stored for the refused create.
            expect_error(
                case,
                call(
                    &router,
                    "DescribeDBShardGroups",
                    &[("DBShardGroupIdentifier", shard_group.as_str())],
                )
                .await,
                404,
                "DBShardGroupNotFound",
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_parameter_group_a_cluster_still_holds_cannot_be_deleted() {
    let router = emulator();
    // The group every row below deletes, and a second one no cluster ever holds.
    for name in ["emu-refusal-cpg", "emu-refusal-cpg-free"] {
        expect_ok(
            &format!("create the cluster parameter group {name}"),
            call(
                &router,
                "CreateDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", name),
                    ("DBParameterGroupFamily", "aurora-postgresql17"),
                    ("Description", "a group under test"),
                ],
            )
            .await,
        );
    }
    expect_ok(
        "the cluster that holds the group",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-refusal-cpg-cluster"),
                ("Engine", "aurora-postgresql"),
                ("DBClusterParameterGroupName", "emu-refusal-cpg"),
            ],
        )
        .await,
    );

    // The group a cluster holds is refused however the delete spells its name — the record
    // keeps the spelling its create carried, and the group is keyed lowercased — while the
    // group no cluster holds is deleted.
    for (case, named) in [
        ("the delete in the stored spelling", "emu-refusal-cpg"),
        ("the delete in upper case", "EMU-REFUSAL-CPG"),
    ] {
        let refused = expect_error(
            case,
            call(
                &router,
                "DeleteDBClusterParameterGroup",
                &[("DBClusterParameterGroupName", named)],
            )
            .await,
            400,
            "InvalidDBParameterGroupState",
        );
        assert_eq!(
            text(&refused, "Message"),
            "One or more database instances are still members of this parameter group \
             emu-refusal-cpg, so the group cannot be deleted",
            "case {case:?}: expected the sentence the in-use refusal carries, got {refused}"
        );
        // The refused delete left the group where it was.
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterParameterGroups",
                &[("DBClusterParameterGroupName", "emu-refusal-cpg")],
            )
            .await,
        );
        assert_eq!(
            texts(&described, "DBClusterParameterGroupName"),
            vec!["emu-refusal-cpg".to_string()],
            "case {case:?}: expected the refused delete to change nothing, got {described}"
        );
    }
    expect_ok(
        "the group no cluster holds",
        call(
            &router,
            "DeleteDBClusterParameterGroup",
            &[("DBClusterParameterGroupName", "emu-refusal-cpg-free")],
        )
        .await,
    );
    expect_error(
        "the group no cluster holds, after the delete",
        call(
            &router,
            "DescribeDBClusterParameterGroups",
            &[("DBClusterParameterGroupName", "emu-refusal-cpg-free")],
        )
        .await,
        404,
        "DBParameterGroupNotFound",
    );

    // And once the cluster that held the group is gone, the same delete is served: the
    // teardown order a caller that created both has to follow. The cluster takes a delete
    // once it is up, which the clock owes it.
    let (_, _) = poll_until(
        "the cluster that held the group",
        "the DB cluster reaching available",
        || {
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", "emu-refusal-cpg-cluster")],
            )
        },
        |status, body| status == 200 && text(body, "Status") == "available",
    )
    .await;
    expect_ok(
        "delete the cluster that held the group",
        call(
            &router,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "emu-refusal-cpg-cluster"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    let (_, _) = poll_until(
        "the group once its cluster is gone",
        "the DB cluster leaving the store",
        || {
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", "emu-refusal-cpg-cluster")],
            )
        },
        |status, _| status == 404,
    )
    .await;
    expect_ok(
        "the group once its cluster is gone",
        call(
            &router,
            "DeleteDBClusterParameterGroup",
            &[("DBClusterParameterGroupName", "emu-refusal-cpg")],
        )
        .await,
    );
}
