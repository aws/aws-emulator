// SPDX-License-Identifier: CC0-1.0

//! `GetIdentityCenterAuthToken`: the IAM Identity Center authentication token a
//! caller asks Amazon Redshift to generate, end to end over the awsQuery wire.
//!
//! ONE rule is under test, and each row below is one member of the class it
//! generalises over rather than one recorded request: the token this operation
//! generates carries the identity embedded in the CALLER's credentials, this
//! emulator attaches an IAM Identity Center identity to none of the credentials
//! it accepts, and that gate is UNCONDITIONAL and checked BEFORE anything about
//! the request — so every call is refused with `UnsupportedOperation` (400) and
//! Amazon Redshift's own sentence, whatever scope it asked for and whether or not
//! this account holds the clusters it named.
//!
//! The class is therefore the whole request surface: the SIZE of the scope
//! (including past the maximum the `ClusterIds` member documents), the FORM of
//! its entries (including spellings that member's documented syntax does not
//! admit) and what the account holds of them. The precedence is what the rows
//! that break a documented constraint assert: a scope the operation would refuse
//! as a bad parameter is answered on the CALLER instead, because the caller is
//! judged first.
//!
//! No call ever answers a `Token`, and no call changes anything a later caller
//! reads — which the last case asserts by reading every cluster back after a
//! batch of refused requests.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The most cluster identifiers the `ClusterIds` member documents, and the
/// longest one it documents.
///
/// Source: `smithy_query(query="doc", shape="GetIdentityCenterAuthTokenRequest",
/// member="ClusterIds")` — "`ClusterIds` can hold a maximum of 20 cluster
/// identifiers", "cluster identifiers must be 1 to 63 characters in length". The
/// numbers are the member's published bounds, which this suite uses to build a
/// row AT each bound and one PAST it: all of them are answered on the caller.
const DOCUMENTED_MAX_SCOPE_CLUSTERS: usize = 20;
const DOCUMENTED_MAX_IDENTIFIER_LEN: usize = 63;

/// A cluster identifier of exactly `len` characters that starts with a letter.
fn identifier_of_length(len: usize) -> String {
    format!("c{}", "a".repeat(len - 1))
}

/// `ClusterIds` travels as its member's own element name — the list member's
/// `xmlName` is `ClusterIdentifier` — which is what an SDK sends.
fn scope(cluster_ids: &[String]) -> Vec<(String, String)> {
    cluster_ids
        .iter()
        .enumerate()
        .map(|(position, cluster_id)| {
            (
                format!("ClusterIds.ClusterIdentifier.{}", position + 1),
                cluster_id.clone(),
            )
        })
        .collect()
}

/// One `GetIdentityCenterAuthToken` call scoped to these cluster identifiers.
async fn token(emulator: &Emulator, cluster_ids: &[String]) -> Answer {
    let params = scope(cluster_ids);
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("GetIdentityCenterAuthToken", &borrowed)
        .await
}

/// Owned identifiers from a slice of literals, so a row can be written inline.
fn ids(cluster_ids: &[&str]) -> Vec<String> {
    cluster_ids.iter().map(|id| (*id).to_string()).collect()
}

/// Create one cluster and wait for it to be serving, by POLLING the read, so the
/// row that names a cluster of this account names one that is `available`.
async fn create_available_cluster(emulator: &Emulator, identifier: &str) {
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

/// A request whose scope satisfies every documented constraint is refused as
/// unsupported, because no credential this emulator accepts carries an embedded
/// IAM Identity Center identity — so the refusal is about the CALLER and not
/// about the clusters, and it is the same for a scope of held and unheld clusters
/// alike.
#[tokio::test]
async fn a_caller_without_an_embedded_identity_center_identity_is_refused_as_unsupported() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "warehouse-one").await;
    create_available_cluster(&emulator, "warehouse-two").await;

    // The class: how many clusters the token is scoped to, and what the account
    // holds of them. Every entry satisfies the member's documented syntax.
    let twenty: Vec<String> = (1..=DOCUMENTED_MAX_SCOPE_CLUSTERS)
        .map(|index| format!("warehouse-{index}"))
        .collect();
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one cluster this account holds", ids(&["warehouse-one"])),
        (
            "one cluster this account does not hold",
            ids(&["no-such-warehouse"]),
        ),
        (
            "two held clusters",
            ids(&["warehouse-one", "warehouse-two"]),
        ),
        (
            "a held cluster beside an unheld one",
            ids(&["warehouse-one", "no-such-warehouse"]),
        ),
        ("the documented maximum number of clusters", twenty),
        (
            "an identifier of the documented maximum length",
            vec![identifier_of_length(DOCUMENTED_MAX_IDENTIFIER_LEN)],
        ),
        (
            "mixed case and digits, which this member's syntax admits",
            ids(&["Warehouse-7"]),
        ),
    ];

    for (case, cluster_ids) in rows {
        let answered = token(&emulator, &cluster_ids).await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (400, Some("UnsupportedOperation")),
            "case {case:?}: expected 400 UnsupportedOperation, got {} / {:?} with body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        assert_eq!(
            answered.text("Message").as_deref(),
            Some(state::IDENTITY_CENTER_ENHANCED_CREDENTIALS_REQUIRED),
            "case {case:?}: expected Amazon Redshift's refusal of a caller that is not an \
             Identity Center user, got {:?}",
            answered.text("Message")
        );
        assert_eq!(
            (answered.text("Token"), answered.text("ExpirationTime")),
            (None, None),
            "case {case:?}: expected no token and no expiry in a refusal, got {:?} / {:?}",
            answered.text("Token"),
            answered.text("ExpirationTime")
        );
    }
}

/// PRECEDENCE: the caller's credential type is judged BEFORE the scope, so a
/// request whose `ClusterIds` breaks a constraint that member documents is still
/// answered on the caller — the same code, status and sentence as a well-formed
/// one. One row per documented constraint, including the published bound's first
/// value past it.
#[tokio::test]
async fn the_credential_gate_precedes_every_judgement_of_the_scope() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "warehouse-one").await;

    let too_many: Vec<String> = (1..=DOCUMENTED_MAX_SCOPE_CLUSTERS + 1)
        .map(|index| format!("warehouse-{index}"))
        .collect();
    // Each row names the documented constraint it breaks.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("more identifiers than the documented maximum", too_many),
        (
            "an identifier one character past the documented maximum length",
            vec![identifier_of_length(DOCUMENTED_MAX_IDENTIFIER_LEN + 1)],
        ),
        (
            "an identifier that does not start with a letter",
            ids(&["1warehouse"]),
        ),
        (
            "an identifier ending with a hyphen",
            ids(&["warehouse-"]),
        ),
        (
            "an identifier with two consecutive hyphens",
            ids(&["ware--house"]),
        ),
        (
            "an identifier with a character the member does not admit",
            ids(&["ware_house"]),
        ),
        (
            "a well-formed identifier beside a malformed one",
            ids(&["warehouse-one", "ware_house"]),
        ),
    ];

    for (case, cluster_ids) in rows {
        let answered = token(&emulator, &cluster_ids).await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (400, Some("UnsupportedOperation")),
            "case {case:?}: expected the caller to be judged first, so 400 UnsupportedOperation, \
             got {} / {:?} with body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        assert_eq!(
            answered.text("Message").as_deref(),
            Some(state::IDENTITY_CENTER_ENHANCED_CREDENTIALS_REQUIRED),
            "case {case:?}: expected the same sentence a well-formed scope earns, got {:?}",
            answered.text("Message")
        );
        assert_eq!(
            answered.text("Token"),
            None,
            "case {case:?}: expected no token in a refusal, got {:?}",
            answered.text("Token")
        );
    }

    // The one thing judged before the caller is the BOUNDARY: "ClusterIds must
    // contain at least 1 cluster identifier" is the member's own `@required`
    // trait, so a request naming none never reaches the handler at all.
    let silent = token(&emulator, &[]).await;
    assert_eq!(
        (silent.status, silent.error_code().as_deref()),
        (400, Some("MalformedInput")),
        "case \"no cluster identifier at all\": expected the boundary to refuse it, got {} / \
         {:?} with body {}",
        silent.status,
        silent.error_code(),
        silent.body
    );
    assert!(
        silent
            .text("Message")
            .is_some_and(|message| message.to_ascii_lowercase().contains("clusterids")),
        "case \"no cluster identifier at all\": expected the refusal to name the member, got {:?}",
        silent.text("Message")
    );
}

/// A refused request is not a write: every cluster of the account reads back
/// exactly as it did before a batch of refused calls, and no new cluster appears.
#[tokio::test]
async fn a_refused_request_changes_nothing_a_later_caller_reads() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "warehouse-one").await;
    create_available_cluster(&emulator, "warehouse-two").await;

    let listed = |answer: &Answer| {
        crate::cluster_lifecycle::all_text(&answer.body, "ClusterIdentifier")
            .into_iter()
            .zip(crate::cluster_lifecycle::all_text(
                &answer.body,
                "ClusterStatus",
            ))
            .collect::<Vec<(String, String)>>()
    };

    let before = emulator.call("DescribeClusters", &[]).await;
    assert_eq!(
        listed(&before),
        vec![
            ("warehouse-one".to_string(), state::STATUS_AVAILABLE.to_string()),
            ("warehouse-two".to_string(), state::STATUS_AVAILABLE.to_string()),
        ],
        "case the seeded account: expected two available clusters, got {:?}",
        listed(&before)
    );

    // Refused for the caller, refused at the boundary, and repeated — none of
    // which may leave a trace.
    for cluster_ids in [
        ids(&["warehouse-one"]),
        ids(&["warehouse-one", "warehouse-two"]),
        ids(&["no-such-warehouse"]),
        ids(&["ware_house"]),
        Vec::new(),
    ] {
        let answered = token(&emulator, &cluster_ids).await;
        assert_eq!(
            answered.status, 400,
            "case scope {cluster_ids:?}: expected a 400 refusal, got {} with body {}",
            answered.status, answered.body
        );
    }

    let after = emulator.call("DescribeClusters", &[]).await;
    assert_eq!(
        listed(&after),
        listed(&before),
        "case the account after refused token requests: expected the clusters unchanged, \
         got {:?}",
        listed(&after)
    );
}
