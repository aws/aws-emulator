// SPDX-License-Identifier: CC0-1.0

//! `ModifyLakehouseConfiguration`: the lakehouse registration a cluster carries,
//! end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! Two rules are under test, and each row is one member of the class the rule
//! generalises over rather than one recorded request:
//!
//! 1. The configuration is a RECORD of the cluster: each member of the request
//!    changes the part it names and no other, a `DryRun` changes nothing, and
//!    what is stored is what a cluster read reports back
//!    (`Cluster$LakehouseRegistrationStatus` and `$CatalogArn` are members of the
//!    cluster itself).
//! 2. Each dependency the request names is resolved against the state that holds
//!    it, and each answers its OWN fault: a cluster this account does not hold is
//!    `ClusterNotFound` (404) with this operation's own sentence, a cluster the
//!    service is still working on is `InvalidClusterState` (400), and an IAM
//!    Identity Center application this account did not register is
//!    `RedshiftIdcApplicationNotExists` (404).

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator, REGION};

/// Create one cluster and wait for it to be serving, by POLLING the read.
async fn available_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator
        .poll_until(
            &format!("seed cluster {identifier}"),
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
}

/// `ModifyLakehouseConfiguration` with these parameters.
async fn modify(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator
        .call("ModifyLakehouseConfiguration", params)
        .await
}

/// `DescribeClusters` for one cluster.
async fn describe(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await
}

/// One row of the configuration table: the case name, the parameters the call
/// carries, and the registration status and catalog the cluster must then carry.
type ConfigurationCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Option<&'static str>,
    Option<&'static str>,
);

#[tokio::test]
async fn a_registration_is_stored_on_the_cluster_and_read_back() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "lakehouse-cluster").await;

    // The rows differ along what the rule generalises over: which part of the
    // configuration the request names, and the catalog name's form (the
    // characters the member's own pattern admits).
    let rows: Vec<ConfigurationCase> = vec![
        (
            "a registration naming a catalog",
            vec![
                ("ClusterIdentifier", "lakehouse-cluster"),
                ("LakehouseRegistration", "Register"),
                ("CatalogName", "analytics"),
            ],
            Some(state::LAKEHOUSE_REGISTRATION_REGISTERED),
            Some("analytics"),
        ),
        (
            "a registration naming a catalog with every admitted character class",
            vec![
                ("ClusterIdentifier", "lakehouse-cluster"),
                ("LakehouseRegistration", "Register"),
                ("CatalogName", "my-catalog_01"),
            ],
            Some(state::LAKEHOUSE_REGISTRATION_REGISTERED),
            Some("my-catalog_01"),
        ),
        (
            "a registration naming no catalog, which leaves the stored one",
            vec![
                ("ClusterIdentifier", "lakehouse-cluster"),
                ("LakehouseRegistration", "Register"),
            ],
            Some(state::LAKEHOUSE_REGISTRATION_REGISTERED),
            Some("my-catalog_01"),
        ),
        (
            "a catalog named with no registration, which leaves the status",
            vec![
                ("ClusterIdentifier", "lakehouse-cluster"),
                ("CatalogName", "warehouse"),
            ],
            Some(state::LAKEHOUSE_REGISTRATION_REGISTERED),
            Some("warehouse"),
        ),
        (
            "the registration withdrawn",
            vec![
                ("ClusterIdentifier", "lakehouse-cluster"),
                ("LakehouseRegistration", "Deregister"),
            ],
            Some(state::LAKEHOUSE_REGISTRATION_DEREGISTERED),
            Some("warehouse"),
        ),
    ];

    for (case, params, expected_status, expected_catalog) in rows {
        let answered = modify(&emulator, &params).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answered.status, answered.body
        );
        let expected_catalog_arn = expected_catalog
            .map(|catalog| engine::emulator_arn("glue", REGION, &format!("catalog/{catalog}")));
        assert_eq!(
            (
                answered.text("ClusterIdentifier"),
                answered.text("LakehouseRegistrationStatus"),
                answered.text("CatalogArn"),
            ),
            (
                Some("lakehouse-cluster".to_string()),
                expected_status.map(str::to_string),
                expected_catalog_arn.clone(),
            ),
            "case {case:?}: expected the configuration the request asked for, got body {}",
            answered.body
        );

        // ROUND TRIP: both values are members of the CLUSTER, so a cluster read
        // reports exactly what this write stored.
        let read = describe(&emulator, "lakehouse-cluster").await;
        assert_eq!(
            (
                read.text("LakehouseRegistrationStatus"),
                read.text("CatalogArn"),
            ),
            (expected_status.map(str::to_string), expected_catalog_arn),
            "case {case:?}: expected a cluster read to report the stored configuration, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_dry_run_validates_and_changes_nothing() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "dryrun-cluster").await;

    let registered = modify(
        &emulator,
        &[
            ("ClusterIdentifier", "dryrun-cluster"),
            ("LakehouseRegistration", "Register"),
            ("CatalogName", "kept"),
        ],
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "case seed the registration: expected 200, got {} with body {}",
        registered.status, registered.body
    );
    let before = describe(&emulator, "dryrun-cluster").await;

    // A dry run of a request that WOULD change the configuration takes every
    // check and reports the configuration the cluster carries now.
    let dry = modify(
        &emulator,
        &[
            ("ClusterIdentifier", "dryrun-cluster"),
            ("LakehouseRegistration", "Deregister"),
            ("CatalogName", "replaced"),
            ("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        dry.status, 200,
        "case a dry run: expected 200, got {} with body {}",
        dry.status, dry.body
    );
    assert_eq!(
        (
            dry.text("LakehouseRegistrationStatus"),
            dry.text("CatalogArn")
        ),
        (
            before.text("LakehouseRegistrationStatus"),
            before.text("CatalogArn")
        ),
        "case a dry run: expected the configuration as it stands, got body {}",
        dry.body
    );

    let after = describe(&emulator, "dryrun-cluster").await;
    assert_eq!(
        (
            after.text("LakehouseRegistrationStatus"),
            after.text("CatalogArn")
        ),
        (
            before.text("LakehouseRegistrationStatus"),
            before.text("CatalogArn")
        ),
        "case a dry run: expected the cluster unchanged, got body {}",
        after.body
    );

    // And a dry run of a request that would be REFUSED is refused too: it
    // validates the request rather than skipping the checks.
    let refused = modify(
        &emulator,
        &[
            ("ClusterIdentifier", "no-such-cluster"),
            ("LakehouseRegistration", "Register"),
            ("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (404, Some("ClusterNotFound")),
        "case a dry run naming no cluster of this account: expected 404 ClusterNotFound, got {} / \
         {:?} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
}

#[tokio::test]
async fn an_identity_center_application_this_account_did_not_register_is_refused() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "idc-cluster").await;

    // One application this account DID register, so a refusal below cannot be an
    // "everything is refused" artefact.
    let registered = emulator
        .call(
            "CreateRedshiftIdcApplication",
            &[
                (
                    "IdcInstanceArn",
                    "arn:aws:sso:::instance/ssoins-1111111111111111",
                ),
                ("RedshiftIdcApplicationName", "lakehouse-app"),
                ("IdcDisplayName", "Lakehouse"),
                (
                    "IamRoleArn",
                    "arn:aws:iam::000000000000:role/RedshiftIdcRole",
                ),
            ],
        )
        .await;
    assert_eq!(
        registered.status, 200,
        "case seed the application: expected 200, got {} with body {}",
        registered.status, registered.body
    );
    let application_arn = registered
        .text("RedshiftIdcApplicationArn")
        .expect("the create reports the application's ARN");

    // The class: ARNs that name no application of this account — one of the
    // right shape, one of another service, and one that is no ARN at all.
    let rows: Vec<(&str, String)> = vec![
        (
            "an ARN of the right shape naming no application",
            format!("{application_arn}-absent"),
        ),
        (
            "an ARN of another service",
            "arn:aws:sso::123456789012:application/ssoins-1/apl-absent".to_string(),
        ),
        ("a value that is no ARN at all", "lakehouse-app".to_string()),
    ];

    for (case, arn) in rows {
        let refused = modify(
            &emulator,
            &[
                ("ClusterIdentifier", "idc-cluster"),
                ("LakehouseIdcRegistration", "Associate"),
                ("LakehouseIdcApplicationArn", arn.as_str()),
            ],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (404, Some("RedshiftIdcApplicationNotExists")),
            "case {case:?}: expected 404 RedshiftIdcApplicationNotExists, got {} / {:?} with \
             body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }

    // The application this account DID register is associated, and the
    // association is reported back.
    let associated = modify(
        &emulator,
        &[
            ("ClusterIdentifier", "idc-cluster"),
            ("LakehouseRegistration", "Register"),
            ("LakehouseIdcRegistration", "Associate"),
            ("LakehouseIdcApplicationArn", application_arn.as_str()),
        ],
    )
    .await;
    assert_eq!(
        (
            associated.status,
            associated.text("LakehouseIdcApplicationArn")
        ),
        (200, Some(application_arn.clone())),
        "case an application this account registered: expected it associated, got {} with body {}",
        associated.status,
        associated.body
    );

    // Disassociating leaves the cluster with no application to propagate an
    // identity from, and leaves the registration itself alone.
    let disassociated = modify(
        &emulator,
        &[
            ("ClusterIdentifier", "idc-cluster"),
            ("LakehouseIdcRegistration", "Disassociate"),
        ],
    )
    .await;
    assert_eq!(
        (
            disassociated.status,
            disassociated.text("LakehouseIdcApplicationArn"),
            disassociated.text("LakehouseRegistrationStatus"),
        ),
        (
            200,
            None,
            Some(state::LAKEHOUSE_REGISTRATION_REGISTERED.to_string())
        ),
        "case the association withdrawn: expected no application and the registration kept, got \
         body {}",
        disassociated.body
    );
}

#[tokio::test]
async fn a_cluster_that_cannot_take_the_configuration_is_refused_with_its_own_sentence() {
    let emulator = Emulator::new();

    // One cluster still being provisioned, created last so its transient state
    // is the one the row below meets.
    available_cluster(&emulator, "lakehouse-present").await;

    // The class: identifiers this account holds no cluster under — differing in
    // spelling, length and case.
    let longest: String = std::iter::once('c')
        .chain(std::iter::repeat_n(
            'x',
            state::CLUSTER_IDENTIFIER_RULE.max_len - 1,
        ))
        .collect();
    let rows: Vec<(&str, String)> = vec![
        (
            "a plain identifier nothing created",
            "no-lakehouse-target".to_string(),
        ),
        (
            "the shortest identifier the documented rule admits",
            "c".to_string(),
        ),
        ("the longest identifier the documented rule admits", longest),
        (
            "an identifier one character off a stored one",
            "lakehouse-presen".to_string(),
        ),
        (
            "an identifier in mixed case",
            "Lakehouse-Present".to_string(),
        ),
    ];

    for (case, identifier) in rows {
        let refused = modify(
            &emulator,
            &[
                ("ClusterIdentifier", identifier.as_str()),
                ("LakehouseRegistration", "Register"),
            ],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (404, Some("ClusterNotFound")),
            "case {case:?}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        // This operation's OWN sentence: fixed text with no variable part, which
        // is NOT the one a cluster read answers for the same name.
        assert_eq!(
            refused.text("Message").as_deref(),
            Some("The specified cluster identifier can't be found."),
            "case {case:?}: expected this operation's own sentence, got {:?}",
            refused.text("Message")
        );
        let read = describe(&emulator, &identifier).await;
        assert_ne!(
            refused.text("Message"),
            read.text("Message"),
            "case {case:?}: expected the read to word the same refusal differently, got {:?} from \
             both",
            refused.text("Message")
        );
    }

    // A cluster the service is still working on takes no configuration change
    // either, which is the state fault rather than the not-found one. The clock
    // is STOPPED first, so the cluster stays inside its provisioning span
    // however long the calls below take and the refusal is asserted outright.
    emulator.stop_clock();
    let busy = emulator.create_minimal("lakehouse-creating").await;
    assert_eq!(
        busy.status, 200,
        "case seed the provisioning cluster: expected 200, got {} with body {}",
        busy.status, busy.body
    );
    let refused = modify(
        &emulator,
        &[
            ("ClusterIdentifier", "lakehouse-creating"),
            ("LakehouseRegistration", "Register"),
        ],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (400, Some("InvalidClusterState")),
        "case a cluster still being provisioned: expected 400 InvalidClusterState, got {} / \
         {:?} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
}
