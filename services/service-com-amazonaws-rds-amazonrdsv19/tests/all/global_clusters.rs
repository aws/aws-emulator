// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's Aurora global database slice:
//! `CreateGlobalCluster` and the `DescribeGlobalClusters` report that reads what it
//! persisted.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: a resource a create persisted is visible to every later call that
//! looks it up, and a lookup answers the modelled not-found fault only for a resource
//! nothing created. Here that is `CreateGlobalCluster` -> `DescribeGlobalClusters`: the
//! global cluster the create acknowledged is reported, with the members the create
//! resolved, and an identifier no create stored is the modelled
//! `GlobalClusterNotFoundFault`.
//!
//! The tables below span the classes those rules generalise over: both engines
//! `CreateGlobalClusterMessage$Engine` publishes, the four forms `EngineVersion` resolves
//! from (an exact catalogue version, a bare major version, a published version this
//! emulator's catalogue does not carry, and none at all), both
//! `EngineLifecycleSupport` values and its default, both settings of the two boolean
//! members, an identifier stored lowercased and read back in another case, the
//! account-wide scope a global cluster is visible in, and the published account allowance
//! plus one create past it.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DEFAULT_ENGINE_LIFECYCLE_SUPPORT, ENGINE_LIFECYCLE_SUPPORT_VALUES, GLOBAL_CLUSTER_ENGINES,
    GLOBAL_CLUSTER_QUOTA_PER_ACCOUNT, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, text, texts, REGION,
};

/// A second region, so the ACCOUNT scope of a global cluster is observable: unlike every
/// other record this service holds, a global cluster is reported whatever region the
/// request was signed for.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// One row of the create/describe round trip.
struct GlobalClusterCase {
    case: &'static str,
    /// The `GlobalClusterIdentifier` the create names.
    identifier: &'static str,
    /// What the identifier is stored and reported as: "This parameter is stored as a
    /// lowercase string".
    stored_identifier: &'static str,
    engine: &'static str,
    /// The `EngineVersion` the create names, if any.
    requested_version: Option<&'static str>,
    /// The version the global cluster runs, resolved against the engine-version catalogue
    /// `DescribeDBEngineVersions` publishes.
    engine_version: &'static str,
    database_name: Option<&'static str>,
    engine_lifecycle_support: Option<&'static str>,
    storage_encrypted: bool,
    deletion_protection: bool,
}

/// The global clusters the round trip creates and reads back.
///
/// The rows differ along what the rule generalises over: the engine (both published
/// values), the form `EngineVersion` takes (exact, major-only, absent), whether a default
/// database is named, the lifecycle type (both published values and the default), the two
/// booleans, and the case the identifier is given in.
const GLOBAL_CLUSTER_CASES: &[GlobalClusterCase] = &[
    GlobalClusterCase {
        case: "aurora-mysql, nothing but the identifier and the engine",
        identifier: "emu-global-mysql",
        stored_identifier: "emu-global-mysql",
        engine: "aurora-mysql",
        requested_version: None,
        // The engine's own default available version, which is what RDS provisions for a
        // request that names no version.
        engine_version: "8.0.mysql_aurora.3.10.3",
        database_name: None,
        engine_lifecycle_support: None,
        storage_encrypted: false,
        deletion_protection: false,
    },
    GlobalClusterCase {
        case: "aurora-postgresql, exact version, database name, both booleans on",
        identifier: "emu-global-pg",
        stored_identifier: "emu-global-pg",
        engine: "aurora-postgresql",
        requested_version: Some("15.8"),
        engine_version: "15.8",
        database_name: Some("inventory"),
        engine_lifecycle_support: Some("open-source-rds-extended-support-disabled"),
        storage_encrypted: true,
        deletion_protection: true,
    },
    GlobalClusterCase {
        case: "aurora-mysql, bare major version, identifier in mixed case",
        identifier: "EMU-Global-Mysql57",
        stored_identifier: "emu-global-mysql57",
        engine: "aurora-mysql",
        requested_version: Some("5.7"),
        // A bare major version resolves to that major version's default.
        engine_version: "5.7.mysql_aurora.2.11.6",
        database_name: None,
        engine_lifecycle_support: Some("open-source-rds-extended-support"),
        storage_encrypted: false,
        deletion_protection: true,
    },
    GlobalClusterCase {
        case: "aurora-postgresql, bare major version, identifier at its published alphabet",
        // An Amazon RDS identifier is letters, digits and hyphens: the `.` the model's own
        // `@pattern` would admit is not one an identifier may carry, and a create naming one
        // is refused (`crate::state::check_identifier`), so the row that reads a global
        // cluster back carries a hyphenated identifier.
        identifier: "emu-global-pg16",
        stored_identifier: "emu-global-pg16",
        engine: "aurora-postgresql",
        requested_version: Some("16"),
        engine_version: "16.6",
        database_name: Some("ledger"),
        engine_lifecycle_support: None,
        storage_encrypted: true,
        deletion_protection: false,
    },
    GlobalClusterCase {
        // A published Aurora version the engine-version catalogue this emulator carries
        // does not: that catalogue is not the real service's live list, so the version
        // the request named stands rather than being refused.
        case: "aurora-mysql, a version outside this emulator's catalogue",
        identifier: "emu-global-uncatalogued",
        stored_identifier: "emu-global-uncatalogued",
        engine: "aurora-mysql",
        requested_version: Some("8.0.mysql_aurora.3.04.0"),
        engine_version: "8.0.mysql_aurora.3.04.0",
        database_name: None,
        engine_lifecycle_support: None,
        storage_encrypted: false,
        deletion_protection: false,
    },
];

/// `CreateGlobalCluster` for one row of the table.
async fn create_case(router: &MultiServiceRouter, row: &GlobalClusterCase) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("GlobalClusterIdentifier", row.identifier.to_string()),
        ("Engine", row.engine.to_string()),
        ("StorageEncrypted", row.storage_encrypted.to_string()),
        ("DeletionProtection", row.deletion_protection.to_string()),
    ];
    if let Some(version) = row.requested_version {
        params.push(("EngineVersion", version.to_string()));
    }
    if let Some(database_name) = row.database_name {
        params.push(("DatabaseName", database_name.to_string()));
    }
    if let Some(lifecycle) = row.engine_lifecycle_support {
        params.push(("EngineLifecycleSupport", lifecycle.to_string()));
    }
    call(router, "CreateGlobalCluster", &params).await
}

/// Assert every member of one row on the `<GlobalCluster>` subtree of an answer.
fn assert_reported(case: &str, label: &str, xml: &str, row: &GlobalClusterCase) {
    let case = format!("{case} ({label})");
    assert_eq!(
        text(xml, "GlobalClusterIdentifier"),
        row.stored_identifier,
        "case {case:?}: expected the identifier stored as a lowercase string, got {xml}"
    );
    assert_eq!(
        text(xml, "GlobalClusterArn"),
        format!(
            "arn:aws:rds::{ACCOUNT}:global-cluster:{}",
            row.stored_identifier
        ),
        "case {case:?}: a global cluster's ARN carries no region, got {xml}"
    );
    assert_eq!(
        text(xml, "Engine"),
        row.engine,
        "case {case:?}: expected the engine the request named, got {xml}"
    );
    assert_eq!(
        text(xml, "EngineVersion"),
        row.engine_version,
        "case {case:?}: expected the resolved engine version, got {xml}"
    );
    assert_eq!(
        text(xml, "EngineLifecycleSupport"),
        row.engine_lifecycle_support
            .unwrap_or(DEFAULT_ENGINE_LIFECYCLE_SUPPORT),
        "case {case:?}: expected the lifecycle type, got {xml}"
    );
    assert_eq!(
        optional_text(xml, "DatabaseName").as_deref(),
        row.database_name,
        "case {case:?}: expected the default database name the request named, got {xml}"
    );
    assert_eq!(
        text(xml, "StorageEncrypted"),
        row.storage_encrypted.to_string(),
        "case {case:?}: expected the storage encryption setting, got {xml}"
    );
    assert_eq!(
        text(xml, "StorageEncryptionType"),
        if row.storage_encrypted {
            // KMS encryption was asked for, under the managed key RDS uses when the
            // request names none of its own.
            "sse-kms"
        } else {
            // Aurora storage is encrypted either way: without KMS encryption the key is
            // the service's own.
            "sse-rds"
        },
        "case {case:?}: expected the encryption type the setting implies, got {xml}"
    );
    let endpoint = text(xml, "Endpoint");
    assert!(
        endpoint.starts_with(&format!("{}.global-", row.stored_identifier))
            && endpoint.ends_with(".global.rds.amazonaws.com"),
        "case {case:?}: expected the global database's own writer endpoint, got \
         {endpoint:?} in {xml}"
    );
    assert_eq!(
        text(xml, "DeletionProtection"),
        row.deletion_protection.to_string(),
        "case {case:?}: expected the deletion protection setting, got {xml}"
    );
    assert_eq!(
        text(xml, "Status"),
        "available",
        "case {case:?}: an empty global database provisions no DB cluster, so it is \
         available, got {xml}"
    );
    let resource_id = text(xml, "GlobalClusterResourceId");
    assert!(
        resource_id.starts_with("cluster-") && resource_id.len() > "cluster-".len(),
        "case {case:?}: expected a minted GlobalClusterResourceId, got {resource_id:?} in \
         {xml}"
    );
    assert!(
        texts(xml, "DBClusterArn").is_empty() && texts(xml, "IsWriter").is_empty(),
        "case {case:?}: a global database created empty has no member cluster, got {xml}"
    );
}

#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    let router = emulator();

    for row in GLOBAL_CLUSTER_CASES {
        let created = expect_ok(row.case, create_case(&router, row).await);
        // The create's own answer, and then the global cluster read back through
        // `DescribeGlobalClusters` — the round trip is what proves the record was
        // PERSISTED rather than echoed, which is the defect this suite pins.
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", row.identifier)],
            )
            .await,
        );
        for (label, xml) in [
            ("CreateGlobalCluster", &created),
            ("DescribeGlobalClusters", &described),
        ] {
            assert_reported(row.case, label, xml, row);
        }
        // The writer endpoint is the global database's own, and the same on every read:
        // a minted label, not a value recomputed per response.
        assert_eq!(
            text(&created, "Endpoint"),
            text(&described, "Endpoint"),
            "case {:?}: expected one stable writer endpoint, got {created} and {described}",
            row.case
        );
        // "This parameter isn't case-sensitive": the same record answers a lookup in
        // another case.
        let by_upper = expect_ok(
            row.case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[(
                    "GlobalClusterIdentifier",
                    row.identifier.to_ascii_uppercase().as_str(),
                )],
            )
            .await,
        );
        assert_eq!(
            text(&by_upper, "GlobalClusterIdentifier"),
            row.stored_identifier,
            "case {:?}: the identifier is not case-sensitive, got {by_upper}",
            row.case
        );
    }

    // Every created cluster is reported by the unnamed report too, in identifier order.
    let all = expect_ok(
        "every global cluster",
        call(&router, "DescribeGlobalClusters", &[] as &[(&str, &str)]).await,
    );
    let mut expected: Vec<&str> = GLOBAL_CLUSTER_CASES
        .iter()
        .map(|row| row.stored_identifier)
        .collect();
    expected.sort_unstable();
    assert_eq!(
        texts(&all, "GlobalClusterIdentifier"),
        expected,
        "expected every created global cluster in identifier order, got {all}"
    );
}

#[tokio::test]
async fn a_created_global_cluster_is_visible_from_every_region() {
    let router = emulator();

    expect_ok(
        "create in us-west-2",
        call_in(
            &router,
            REGION,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-global-account-scoped"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    // A global cluster is account-scoped: a request signed for another region sees it,
    // where a DB instance or an option group of that region would not be reported.
    let body = expect_ok(
        "describe from eu-west-1",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "emu-global-account-scoped")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "GlobalClusterIdentifier"),
        "emu-global-account-scoped",
        "expected the global cluster to be visible from any region, got {body}"
    );
    // The one documented filter selects on the regions of the MEMBER clusters, and an
    // empty global database has none — so it matches no region.
    let filtered = expect_ok(
        "the region filter",
        call(
            &router,
            "DescribeGlobalClusters",
            &[
                ("Filters.Filter.1.Name", "region"),
                ("Filters.Filter.1.Values.Value.1", REGION),
            ],
        )
        .await,
    );
    assert!(
        texts(&filtered, "GlobalClusterIdentifier").is_empty(),
        "a global database with no member cluster lies in no region, got {filtered}"
    );
}

#[tokio::test]
async fn a_second_create_of_one_identifier_is_refused_and_changes_nothing() {
    let router = emulator();

    expect_ok(
        "the first create",
        call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-global-twice"),
                ("Engine", "aurora-mysql"),
                ("DatabaseName", "first"),
            ],
        )
        .await,
    );
    expect_error(
        "the second create of the same identifier",
        call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "EMU-GLOBAL-TWICE"),
                ("Engine", "aurora-postgresql"),
                ("DatabaseName", "second"),
            ],
        )
        .await,
        400,
        "GlobalClusterAlreadyExistsFault",
    );
    // The refused create left the first record exactly as it was.
    let body = expect_ok(
        "the stored global cluster after the refusal",
        call(
            &router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "emu-global-twice")],
        )
        .await,
    );
    assert_eq!(
        text(&body, "DatabaseName"),
        "first",
        "expected the refused create to change nothing, got {body}"
    );
    assert_eq!(
        text(&body, "Engine"),
        "aurora-mysql",
        "expected the refused create to change nothing, got {body}"
    );
}

/// One row of the refusal table: the case, the create's parameters, and the HTTP status
/// and wire error code it must be refused with.
type RefusalRow = (&'static str, Vec<(&'static str, &'static str)>, u16, &'static str);

#[tokio::test]
async fn a_create_this_service_refuses_stores_nothing() {
    let router = emulator();

    let rows: Vec<RefusalRow> = vec![
        (
            "an engine no global database runs",
            vec![
                ("GlobalClusterIdentifier", "emu-global-refused"),
                ("Engine", "mysql"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine version that is no version at all",
            vec![
                ("GlobalClusterIdentifier", "emu-global-refused"),
                ("Engine", "aurora-mysql"),
                ("EngineVersion", "not-a-version"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a lifecycle type outside the published values",
            vec![
                ("GlobalClusterIdentifier", "emu-global-refused"),
                ("Engine", "aurora-mysql"),
                ("EngineLifecycleSupport", "open-source-rds-no-support"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a source DB cluster no DB cluster create stored",
            vec![
                ("GlobalClusterIdentifier", "emu-global-refused"),
                (
                    "SourceDBClusterIdentifier",
                    "arn:aws:rds:us-west-2:000000000000:cluster:emu-absent",
                ),
            ],
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "a source DB cluster together with the settings it supplies",
            vec![
                ("GlobalClusterIdentifier", "emu-global-refused"),
                (
                    "SourceDBClusterIdentifier",
                    "arn:aws:rds:us-west-2:000000000000:cluster:emu-absent",
                ),
                ("Engine", "aurora-mysql"),
            ],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, params, status, code) in rows {
        expect_error(
            case,
            call(&router, "CreateGlobalCluster", &params).await,
            status,
            code,
        );
        // A refused create must leave nothing behind for a later describe to find.
        expect_error(
            case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", "emu-global-refused")],
            )
            .await,
            404,
            "GlobalClusterNotFoundFault",
        );
    }
}

#[tokio::test]
async fn every_published_engine_and_lifecycle_value_is_accepted() {
    let router = emulator();

    // The published value lists, from `CreateGlobalClusterMessage$Engine` ("Valid Values:
    // aurora-mysql | aurora-postgresql") and `$EngineLifecycleSupport`, each exercised in
    // turn: the catalogues the handler resolves against are the model's, so every entry
    // must be servable.
    for (index, engine) in GLOBAL_CLUSTER_ENGINES.iter().enumerate() {
        for (offset, lifecycle) in ENGINE_LIFECYCLE_SUPPORT_VALUES.iter().enumerate() {
            let case = format!("{engine} with {lifecycle}");
            let identifier = format!("emu-global-matrix-{index}-{offset}");
            expect_ok(
                &case,
                call(
                    &router,
                    "CreateGlobalCluster",
                    &[
                        ("GlobalClusterIdentifier", identifier.as_str()),
                        ("Engine", *engine),
                        ("EngineLifecycleSupport", *lifecycle),
                    ],
                )
                .await,
            );
            let body = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeGlobalClusters",
                    &[("GlobalClusterIdentifier", identifier.as_str())],
                )
                .await,
            );
            assert_eq!(
                text(&body, "EngineLifecycleSupport"),
                *lifecycle,
                "case {case:?}: expected the lifecycle type to be reported, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_persisted_global_cluster_is_accepted_and_only_then_is_it_not_found() {
    let router = emulator();

    // The delete counterpart of the round trip above: the global cluster the create
    // persisted is deletable, and only once it is gone is the identifier not found.
    for (case, identifier, deletion_protection) in [
        ("an unprotected global cluster", "emu-global-deletable", false),
        ("a protected one, released first", "emu-global-protected", true),
    ] {
        expect_ok(
            case,
            call(
                &router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", identifier),
                    ("Engine", "aurora-postgresql"),
                    ("DeletionProtection", &deletion_protection.to_string()),
                ],
            )
            .await,
        );
        if deletion_protection {
            // "The global database can't be deleted when deletion protection is
            // enabled": a setting the request conflicts with — the caller clears it and
            // calls again — so a parameter refusal, not a state fault, and the cluster
            // is still there.
            expect_error(
                case,
                call(
                    &router,
                    "DeleteGlobalCluster",
                    &[("GlobalClusterIdentifier", identifier)],
                )
                .await,
                400,
                "InvalidParameterCombination",
            );
            let still_there = expect_ok(
                case,
                call(
                    &router,
                    "DescribeGlobalClusters",
                    &[("GlobalClusterIdentifier", identifier)],
                )
                .await,
            );
            assert_eq!(
                text(&still_there, "DeletionProtection"),
                "true",
                "case {case:?}: expected the refused delete to change nothing, got \
                 {still_there}"
            );
            // Deletion protection is released the only way this slice can release it:
            // this service's `ModifyGlobalCluster` is still scaffolded, so the protected
            // cluster stays — and the delete stays refused, which is the whole assertion.
            continue;
        }
        let deleted = expect_ok(
            case,
            call(
                &router,
                "DeleteGlobalCluster",
                // "This parameter isn't case-sensitive" for the stored identifier.
                &[("GlobalClusterIdentifier", &identifier.to_ascii_uppercase())],
            )
            .await,
        );
        assert_eq!(
            text(&deleted, "GlobalClusterIdentifier"),
            identifier,
            "case {case:?}: expected the delete to answer with the cluster it removed, got \
             {deleted}"
        );
        assert_eq!(
            text(&deleted, "Status"),
            "available",
            "case {case:?}: expected the status the global database held when the delete \
             was accepted, got {deleted}"
        );
        // Only now is the identifier not found — by the report and by a second delete
        // alike.
        for action in ["DescribeGlobalClusters", "DeleteGlobalCluster"] {
            expect_error(
                case,
                call(&router, action, &[("GlobalClusterIdentifier", identifier)]).await,
                404,
                "GlobalClusterNotFoundFault",
            );
        }
    }
}

#[tokio::test]
async fn the_stored_tag_list_is_bounded_at_its_published_quota() {
    let router = emulator();

    // `Tags` is stored request data and publishes no bound of its own, so this service
    // enforces AWS's published tag quota: a create at the quota is served, one tag past
    // it is refused and stores nothing.
    for (case, identifier, count, served) in [
        ("tags at the published quota", "emu-global-tags", MAX_TAGS_PER_RESOURCE, true),
        (
            "one tag past the published quota",
            "emu-global-tags-over",
            MAX_TAGS_PER_RESOURCE + 1,
            false,
        ),
    ] {
        let mut params: Vec<(String, String)> = vec![
            (
                "GlobalClusterIdentifier".to_string(),
                identifier.to_string(),
            ),
            ("Engine".to_string(), "aurora-mysql".to_string()),
        ];
        for index in 0..count {
            params.push((format!("Tags.Tag.{}.Key", index + 1), format!("key-{index}")));
            params.push((
                format!("Tags.Tag.{}.Value", index + 1),
                format!("value-{index}"),
            ));
        }
        let answer = call(&router, "CreateGlobalCluster", &params).await;
        if served {
            expect_ok(case, answer);
            let body = expect_ok(
                case,
                call(
                    &router,
                    "DescribeGlobalClusters",
                    &[("GlobalClusterIdentifier", identifier)],
                )
                .await,
            );
            assert_eq!(
                texts(&body, "Key").len(),
                count,
                "case {case:?}: expected every tag at the quota to be stored, got {body}"
            );
        } else {
            expect_error(case, answer, 400, "InvalidParameterValue");
            expect_error(
                case,
                call(
                    &router,
                    "DescribeGlobalClusters",
                    &[("GlobalClusterIdentifier", identifier)],
                )
                .await,
                404,
                "GlobalClusterNotFoundFault",
            );
        }
    }
}

#[tokio::test]
async fn the_account_allowance_bounds_what_one_account_may_hold() {
    let router = emulator();

    for index in 0..GLOBAL_CLUSTER_QUOTA_PER_ACCOUNT {
        let case = format!("global cluster {index} of the allowance");
        expect_ok(
            &case,
            call(
                &router,
                "CreateGlobalCluster",
                &[
                    (
                        "GlobalClusterIdentifier",
                        format!("emu-global-quota-{index}").as_str(),
                    ),
                    ("Engine", "aurora-mysql"),
                ],
            )
            .await,
        );
    }
    // One past the published allowance is the modelled quota fault, and it is answered
    // whatever region the request is signed for: the allowance is the ACCOUNT's.
    expect_error(
        "one global cluster past the account allowance",
        call_in(
            &router,
            OTHER_REGION,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-global-quota-over"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
        400,
        "GlobalClusterQuotaExceededFault",
    );
}

#[tokio::test]
async fn a_global_database_created_from_a_stored_cluster_adopts_it() {
    let router = emulator();

    // (case, how the create NAMES the source cluster). `SourceDBClusterIdentifier` is
    // resolved the way every cluster-naming member accepts a cluster — the identifier or
    // the ARN, in either case — so the rows differ along that form, which is what the
    // rule generalises over.
    /// How one row NAMES the source cluster: the form the member accepts it in.
    type NameSource = fn(&str) -> String;
    let rows: [(&str, NameSource); 4] = [
        ("by identifier", |identifier| identifier.to_string()),
        ("by identifier in another case", |identifier| {
            identifier.to_ascii_uppercase()
        }),
        ("by ARN", |identifier| {
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
        }),
        ("by ARN in another case", |identifier| {
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{}", {
                identifier.to_ascii_uppercase()
            })
        }),
    ];
    for (index, (case, name_source)) in rows.iter().enumerate() {
        let source = format!("emu-global-source-{index}");
        let global = format!("emu-global-adopt-{index}");
        // The source cluster carries the four settings the member documents a global
        // database takes from it: "Amazon Aurora uses the values from the specified source
        // DB cluster: DatabaseName Engine EngineVersion StorageEncrypted."
        expect_ok(
            case,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", source.clone()),
                    ("Engine", "aurora-postgresql".to_string()),
                    ("EngineVersion", "15.8".to_string()),
                    ("DatabaseName", "ledger".to_string()),
                    ("StorageEncrypted", "true".to_string()),
                ],
            )
            .await,
        );
        let created = expect_ok(
            case,
            call(
                &router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", global.clone()),
                    ("SourceDBClusterIdentifier", name_source(&source)),
                ],
            )
            .await,
        );
        // The global database reports the source's settings and adopts it as its FIRST
        // member — its primary, which is what `IsWriter` reports — and a later describe
        // reads the same record back.
        let read_back = expect_ok(
            case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", global.as_str())],
            )
            .await,
        );
        for body in [&created, &read_back] {
            for (element, want) in [
                ("Engine", "aurora-postgresql".to_string()),
                ("EngineVersion", "15.8".to_string()),
                ("DatabaseName", "ledger".to_string()),
                ("StorageEncrypted", "true".to_string()),
                (
                    "DBClusterArn",
                    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{source}"),
                ),
                ("IsWriter", "true".to_string()),
            ] {
                assert_eq!(
                    text(body, element),
                    want,
                    "case {case:?}: the global database takes <{element}> from the cluster it \
                     adopted, expected {want:?}, got {body}"
                );
            }
        }
        // And the reference holds in the OTHER direction: the adopted cluster reports the
        // global database it now belongs to, so the two records cannot disagree about the
        // membership.
        let member = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", source.as_str())],
            )
            .await,
        );
        assert_eq!(
            text(&member, "GlobalClusterIdentifier"),
            global,
            "case {case:?}: the adopted cluster must name the global database it joined, \
             got {member}"
        );
        // A cluster belongs to at most ONE global database: a second global create naming
        // the same source is refused for the cluster's state, and stores nothing — which
        // is what keeps the first global database's member list pointing at a cluster that
        // still points back.
        let second = format!("emu-global-readopt-{index}");
        expect_error(
            case,
            call(
                &router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", second.clone()),
                    ("SourceDBClusterIdentifier", name_source(&source)),
                ],
            )
            .await,
            400,
            "InvalidDBClusterStateFault",
        );
        expect_error(
            case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", second.as_str())],
            )
            .await,
            404,
            "GlobalClusterNotFoundFault",
        );
    }

    // The counterpart, which the row above is the complement of: a source nothing created
    // resolves to no cluster, so the create is the modelled not-found fault and stores no
    // global database. (The identifier forms above all resolve; this one cannot.)
    expect_error(
        "a source cluster no create stored",
        call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-global-adopt-absent"),
                ("SourceDBClusterIdentifier", "emu-cluster-never-created"),
            ],
        )
        .await,
        404,
        "DBClusterNotFoundFault",
    );
    expect_error(
        "a source cluster no create stored",
        call(
            &router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "emu-global-adopt-absent")],
        )
        .await,
        404,
        "GlobalClusterNotFoundFault",
    );
}
