// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `GlobalCluster` envelope the MUTATION operations of the
//! global-database family answer with: `CreateGlobalCluster`, `ModifyGlobalCluster` and
//! `DeleteGlobalCluster`, read back against `DescribeGlobalClusters`.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on, so every assertion reads the XML the emulator itself produced.
//!
//! The rules under test:
//!
//! * `GlobalClusterResourceId` is a server-MINTED, opaque, immutable identifier of ONE
//!   global database: minted once when the global database is created, carried unchanged
//!   through every later envelope (a modify that renames it, a read, the delete), never
//!   shared with another global database, and never a rendering of the position the
//!   resource was created at — two stores that have each created one global database of the
//!   same name report two different ids;
//! * the answer to an accepted mutation reports the global database as the service's own
//!   state holds it. `DeleteGlobalCluster` removes the record with the call it accepts, so
//!   no transient phase of the global database is ever observable and the answer reports
//!   the status it held when the delete was accepted, rather than a status no read of this
//!   service could confirm.
//!
//! The tables span what those rules generalise over: both engines
//! `CreateGlobalClusterMessage$Engine` publishes, every way a global database reaches the
//! delete (untouched, renamed, released from deletion protection, upgraded, emptied of a
//! member cluster), the identifier spelled in another case, and several independent stores.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, text, texts, REGION};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The prefix `GlobalCluster$GlobalClusterResourceId` carries: AWS writes the
/// "partition-unique, immutable identifier for the global database cluster" as `cluster-`
/// followed by an opaque token. Source: the member's own documentation (smithy_query
/// query="doc", shape="GlobalCluster", member="GlobalClusterResourceId"), which the model
/// constrains with no pattern, so the token's alphabet and width are the recorded
/// envelope's: a fixed-length lowercase-hexadecimal token, which is what replaying the
/// recorded global-database envelope accepts for this member (and it refuses the
/// digits-and-uppercase form the REGIONAL resource ids of this service are written in).
const RESOURCE_ID_PREFIX: &str = "cluster-";

/// How long a poll waits for a transition this service owes to become due before giving up:
/// generous, because the deadline is not what any case here measures — the condition is.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A global cluster's ARN, which carries NO region: it is an account-scoped resource.
fn global_cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds::{ACCOUNT}:global-cluster:{identifier}")
}

/// A DB cluster's ARN, the form `RemoveFromGlobalCluster$DbClusterIdentifier` documents.
fn db_cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
}

/// `CreateGlobalCluster` for one row, answered with the envelope it reported.
async fn create_global(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    version: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", identifier),
                ("Engine", engine),
                ("EngineVersion", version),
            ],
        )
        .await,
    )
}

/// `ModifyGlobalCluster` naming `identifier`, with `extra` beside it.
async fn modify_global(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params: Vec<(&str, &str)> = vec![("GlobalClusterIdentifier", identifier)];
    params.extend(extra.iter().copied());
    expect_ok(case, call(router, "ModifyGlobalCluster", &params).await)
}

/// The one `<GlobalCluster>` `DescribeGlobalClusters` reports for `identifier`.
async fn describe_global(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", identifier)],
        )
        .await,
    );
    assert_eq!(
        texts(&body, "GlobalClusterArn").len(),
        1,
        "case {case:?}: reading {identifier} must report exactly one global database, got \
         {body}"
    );
    body
}

/// Read `identifier` until it reports `available` again, or until [`SETTLE_DEADLINE`]
/// passes — the condition is waited for, never a fixed span.
async fn poll_until_available(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let body = describe_global(router, case, identifier).await;
        let status = text(&body, "Status");
        if status == "available" {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {identifier} must report available once the change it accepted \
             is due, still {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What a row does to its global database between the create and the delete.
#[derive(Debug, Clone, Copy)]
enum Mutation {
    /// Nothing at all: the create's envelope and the delete's are the only mutations.
    None,
    /// `ModifyGlobalCluster` renaming the global database to the value given.
    RenamedTo(&'static str),
    /// Deletion protection turned on, then off again, in two modifies.
    ProtectedThenReleased,
    /// `ModifyGlobalCluster` upgrading the engine version, which runs for a span: the row
    /// polls the global database back to `available` before deleting it.
    UpgradedTo(&'static str),
    /// A member DB cluster created into the global database and then detached, so the
    /// delete sees a global database that HELD a member.
    MemberJoinedThenRemoved(&'static str),
}

/// One row of the envelope table.
struct EnvelopeCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    version: &'static str,
    mutation: Mutation,
    /// The identifier the global database is stored under when the delete names it.
    stored_as: &'static str,
}

/// The rows both envelope rules are asserted over.
///
/// They differ along the engine, the version, which mutation the global database goes
/// through on its way to the delete, and whether it ever held a member cluster.
const ENVELOPE_CASES: &[EnvelopeCase] = &[
    EnvelopeCase {
        case: "aurora-mysql, untouched between create and delete",
        identifier: "emu-gcme-plain",
        engine: "aurora-mysql",
        version: "8.0.mysql_aurora.3.05.2",
        mutation: Mutation::None,
        stored_as: "emu-gcme-plain",
    },
    EnvelopeCase {
        case: "aurora-postgresql, renamed by a modify",
        identifier: "emu-gcme-renamed",
        engine: "aurora-postgresql",
        version: "15.8",
        mutation: Mutation::RenamedTo("emu-gcme-renamed-again"),
        stored_as: "emu-gcme-renamed-again",
    },
    EnvelopeCase {
        case: "aurora-postgresql, protected and then released",
        identifier: "emu-gcme-protected",
        engine: "aurora-postgresql",
        version: "16.6",
        mutation: Mutation::ProtectedThenReleased,
        stored_as: "emu-gcme-protected",
    },
    EnvelopeCase {
        case: "aurora-mysql, engine version upgraded",
        identifier: "emu-gcme-upgraded",
        engine: "aurora-mysql",
        version: "8.0.mysql_aurora.3.05.2",
        mutation: Mutation::UpgradedTo("8.0.mysql_aurora.3.08.0"),
        stored_as: "emu-gcme-upgraded",
    },
    EnvelopeCase {
        case: "aurora-mysql, a member cluster joined and was removed",
        identifier: "emu-gcme-member",
        engine: "aurora-mysql",
        version: "8.0.mysql_aurora.3.05.2",
        mutation: Mutation::MemberJoinedThenRemoved("emu-gcme-member-cluster"),
        stored_as: "emu-gcme-member",
    },
];

/// Drive one row's mutation, answering with every envelope it produced along the way.
async fn drive_mutation(router: &MultiServiceRouter, row: &EnvelopeCase) -> Vec<String> {
    let case = row.case;
    match row.mutation {
        Mutation::None => Vec::new(),
        Mutation::RenamedTo(new_identifier) => vec![
            modify_global(
                router,
                case,
                row.identifier,
                &[("NewGlobalClusterIdentifier", new_identifier)],
            )
            .await,
        ],
        Mutation::ProtectedThenReleased => {
            let protected = modify_global(
                router,
                case,
                row.identifier,
                &[("DeletionProtection", "true")],
            )
            .await;
            // The identifier "isn't case-sensitive", so the release names the same global
            // database spelled differently.
            let released = modify_global(
                router,
                case,
                &row.identifier.to_ascii_uppercase(),
                &[("DeletionProtection", "false")],
            )
            .await;
            vec![protected, released]
        }
        Mutation::UpgradedTo(version) => {
            let upgrading = modify_global(
                router,
                case,
                row.identifier,
                &[
                    ("EngineVersion", version),
                    ("AllowMajorVersionUpgrade", "false"),
                ],
            )
            .await;
            let settled = poll_until_available(router, case, row.stored_as).await;
            vec![upgrading, settled]
        }
        Mutation::MemberJoinedThenRemoved(cluster) => {
            let joined = expect_ok(
                case,
                call(
                    router,
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", cluster),
                        ("Engine", row.engine),
                        ("EngineVersion", row.version),
                        ("GlobalClusterIdentifier", row.identifier),
                    ],
                )
                .await,
            );
            let member_arn = db_cluster_arn(cluster);
            let detached = expect_ok(
                case,
                call(
                    router,
                    "RemoveFromGlobalCluster",
                    &[
                        ("GlobalClusterIdentifier", row.identifier),
                        ("DbClusterIdentifier", member_arn.as_str()),
                    ],
                )
                .await,
            );
            // The global database a detach emptied is deletable straight away; the poll is
            // what waits for that condition rather than for a span.
            let emptied = poll_until_available(router, case, row.stored_as).await;
            vec![joined, detached, emptied]
        }
    }
}

/// The resource id one envelope reports, asserted to be of the minted form before it is
/// compared with anything: the `cluster-` prefix and an opaque token in the recorded
/// alphabet, of the one width every global database's id is minted at.
fn minted_resource_id(case: &str, label: &str, xml: &str) -> String {
    let reported = text(xml, "GlobalClusterResourceId");
    let token = reported.strip_prefix(RESOURCE_ID_PREFIX).unwrap_or_default();
    assert!(
        !token.is_empty() && token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: {label} must report a minted {RESOURCE_ID_PREFIX}<token> resource \
         id, got {reported:?}"
    );
    reported
}

#[tokio::test]
async fn the_minted_resource_id_is_carried_unchanged_through_every_envelope() {
    for row in ENVELOPE_CASES {
        let case = row.case;
        // One store per row, so the row's own ids cannot be told apart by the order the
        // other rows ran in.
        let router = emulator();
        let created = create_global(&router, case, row.identifier, row.engine, row.version).await;
        let minted = minted_resource_id(case, "CreateGlobalCluster", &created);

        for (index, envelope) in drive_mutation(&router, row).await.iter().enumerate() {
            let reported = minted_resource_id(case, "the mutation envelope", envelope);
            assert_eq!(
                reported, minted,
                "case {case:?}: mutation envelope {index} must carry the id minted at \
                 create {minted:?}, got {reported:?}"
            );
        }

        // Every read answers the same id, twice over: the id is stored with the global
        // database, not minted by whoever reports it.
        for attempt in 1..=2 {
            let read = describe_global(&router, case, row.stored_as).await;
            let reported = minted_resource_id(case, "DescribeGlobalClusters", &read);
            assert_eq!(
                reported, minted,
                "case {case:?} read {attempt}: expected the minted id {minted:?}, got \
                 {reported:?}"
            );
        }

        // And the delete's envelope — the last chance a client has to record the id — is
        // the same id again.
        let deleted = expect_ok(
            case,
            call(
                &router,
                "DeleteGlobalCluster",
                &[("GlobalClusterIdentifier", row.stored_as)],
            )
            .await,
        );
        let reported = minted_resource_id(case, "DeleteGlobalCluster", &deleted);
        assert_eq!(
            reported, minted,
            "case {case:?}: the delete must report the id minted at create {minted:?}, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_delete_envelope_reports_the_status_held_when_it_was_accepted() {
    for row in ENVELOPE_CASES {
        let case = row.case;
        let router = emulator();
        create_global(&router, case, row.identifier, row.engine, row.version).await;
        drive_mutation(&router, row).await;

        // The status the global database holds the instant before the delete is taken: the
        // state the delete is accepted against, read through the report.
        let before = describe_global(&router, case, row.stored_as).await;
        let at_acceptance = text(&before, "Status");
        assert_eq!(
            at_acceptance, "available",
            "case {case:?}: a delete is only accepted on an available global database, got \
             {at_acceptance:?}"
        );

        let deleted = expect_ok(
            case,
            call(
                &router,
                "DeleteGlobalCluster",
                // "This parameter isn't case-sensitive" for the stored identifier.
                &[(
                    "GlobalClusterIdentifier",
                    row.stored_as.to_ascii_uppercase().as_str(),
                )],
            )
            .await,
        );
        let reported = text(&deleted, "Status");
        assert_eq!(
            reported, at_acceptance,
            "case {case:?}: the delete must report the status the global database held when \
             it was accepted ({at_acceptance:?}), got {reported:?}"
        );
        // The rest of the envelope is that same accepted snapshot.
        for (member, expected) in [
            ("GlobalClusterIdentifier", row.stored_as.to_string()),
            ("GlobalClusterArn", global_cluster_arn(row.stored_as)),
            ("Engine", text(&before, "Engine")),
            ("EngineVersion", text(&before, "EngineVersion")),
            ("DeletionProtection", text(&before, "DeletionProtection")),
        ] {
            let actual = text(&deleted, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: the delete must report {member} {expected:?} as it stood at \
                 acceptance, got {actual:?}"
            );
        }

        // And the record really is gone: the status it reported was a snapshot, not a
        // phase a later read could still find.
        expect_error(
            case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", row.stored_as)],
            )
            .await,
            404,
            "GlobalClusterNotFoundFault",
        );
    }
}

#[tokio::test]
async fn no_two_global_databases_of_one_store_share_a_minted_resource_id() {
    let router = emulator();
    // The rows differ along the engine and the identifier; the last one REUSES the name of
    // a global database that was deleted, which is a different resource and so carries a
    // different id.
    let rows: &[(&str, &str, &str)] = &[
        ("aurora-mysql", "emu-gcmu-a", "8.0.mysql_aurora.3.05.2"),
        ("aurora-mysql", "emu-gcmu-b", "8.0.mysql_aurora.3.05.2"),
        ("aurora-postgresql", "emu-gcmu-c", "15.8"),
        ("aurora-postgresql", "emu-gcmu-recreated", "16.6"),
    ];

    let mut minted: Vec<(&str, String)> = Vec::new();
    for (engine, identifier, version) in rows {
        let case = *identifier;
        let created = create_global(&router, case, identifier, engine, version).await;
        minted.push((identifier, minted_resource_id(case, "the create", &created)));
    }

    // The re-created one: deleted and created again under the same identifier.
    let recreated_identifier = rows[rows.len() - 1].1;
    expect_ok(
        recreated_identifier,
        call(
            &router,
            "DeleteGlobalCluster",
            &[("GlobalClusterIdentifier", recreated_identifier)],
        )
        .await,
    );
    let recreated = create_global(
        &router,
        recreated_identifier,
        recreated_identifier,
        "aurora-postgresql",
        "16.6",
    )
    .await;
    minted.push((
        recreated_identifier,
        minted_resource_id(recreated_identifier, "the second create", &recreated),
    ));

    for (left, (one, first)) in minted.iter().enumerate() {
        // Every id is minted at one width: the token is a fixed-length one, so its length
        // carries no information about the resource either.
        assert_eq!(
            first.len(),
            minted[0].1.len(),
            "a minted resource id is a fixed-length token: {one} reports {first:?} beside \
             {:?}",
            minted[0].1
        );
        for (two, second) in minted.iter().skip(left + 1) {
            assert_ne!(
                first, second,
                "a minted resource id identifies ONE global database: {one} and {two} both \
                 report {first:?}"
            );
        }
    }
}

#[tokio::test]
async fn two_stores_mint_two_ids_for_the_same_identifier_and_creation_order() {
    // Each store is an independent emulator that has created exactly one global database,
    // of the same name, as its first resource. A resource id that identified the position
    // of the create rather than the resource would be the same in all of them; a
    // partition-unique id is not.
    let identifier = "emu-gcpu-first";
    let mut minted: Vec<String> = Vec::new();
    for store in 1..=3 {
        let case = format!("store {store}");
        let router = emulator();
        let created = create_global(
            &router,
            &case,
            identifier,
            "aurora-postgresql",
            "15.8",
        )
        .await;
        minted.push(minted_resource_id(&case, "the create", &created));
    }

    for (index, first) in minted.iter().enumerate() {
        for second in minted.iter().skip(index + 1) {
            assert_ne!(
                first, second,
                "two stores must mint two ids for the same identifier and creation order, \
                 both report {first:?} (all of {minted:?})"
            );
        }
    }
}
