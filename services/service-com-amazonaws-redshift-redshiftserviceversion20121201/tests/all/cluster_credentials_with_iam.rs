// SPDX-License-Identifier: CC0-1.0

//! The temporary database credentials `GetClusterCredentialsWithIAM` issues for
//! the IAM identity a request is signed with, end to end over the awsQuery wire.
//!
//! The RULES these cases assert, each over a table rather than over one recorded
//! request:
//!
//! * the credentials are issued for the database OF a cluster, so the cluster the
//!   request names is resolved first and a name this account holds no cluster
//!   under answers `ClusterNotFoundFault` and issues nothing;
//! * the database user is the CALLER's — "the database user is mapped 1:1 to the
//!   source IAM identity" — so it carries the documented `IAM:` prefix, is the
//!   same for two calls signed with the same credential, and is decided by nothing
//!   in the request;
//! * the password expires at this call's instant plus the lifetime the request
//!   asked for, inside the documented 900..3600 window, and a lifetime outside it
//!   is refused;
//! * nothing is reported for `NextRefreshTime`, which the model documents as
//!   reserved for future use.
//!
//! The harness is shared with [`crate::cluster_lifecycle`], which owns it; it
//! signs every request with the same access key id, which is the caller identity
//! these cases are about.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The access key id the shared harness signs every request with, which IS the
/// caller's identity as far as this emulator can see it.
const PRESENTED_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The parameters one row of a table sends.
type Params = Vec<(&'static str, &'static str)>;

/// One row of the naming table: the case, the parameters, and the status and code
/// the request is refused with (`None` when it is served).
type NamingRow = (&'static str, Params, Option<(u16, &'static str)>);

/// Create a cluster and wait until it is serving.
async fn serving_cluster(emulator: &Emulator, case: &str, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateCluster {identifier} expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator
        .poll_until(
            case,
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
}

/// Assert one refusal: the status and the `<Code>` the model gives the fault.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// A live cluster yields credentials: the caller's own database user, a password
/// the emulator minted, and an expiry — and the user is the same for every call
/// signed with the same credential, which is what "mapped 1:1" means.
#[tokio::test]
async fn a_live_cluster_yields_credentials_for_the_calling_identity() {
    let emulator = Emulator::new();
    let identifier = "iam-credential-cluster";
    serving_cluster(&emulator, "credentials for a live cluster", identifier).await;

    let first = emulator
        .call(
            "GetClusterCredentialsWithIAM",
            &[("ClusterIdentifier", identifier)],
        )
        .await;
    assert_eq!(
        first.status, 200,
        "case credentials for a live cluster: expected 200, got {} with body {}",
        first.status, first.body
    );
    assert_eq!(
        first.text("DbUser").as_deref(),
        Some(state::iam_identity_db_user(PRESENTED_ACCESS_KEY_ID).as_str()),
        "case credentials for a live cluster: expected the caller's own database user, got {:?}",
        first.text("DbUser")
    );
    let password = first.text("DbPassword").unwrap_or_default();
    assert!(
        !password.is_empty(),
        "case credentials for a live cluster: expected a password, got body {}",
        first.body
    );
    assert!(
        first
            .text("Expiration")
            .is_some_and(|expiration| !expiration.is_empty()),
        "case credentials for a live cluster: expected an expiry, got body {}",
        first.body
    );
    // "Reserved for future use": nothing is reported for it.
    assert_eq!(
        first.text("NextRefreshTime"),
        None,
        "case credentials for a live cluster: expected no NextRefreshTime, got {:?}",
        first.text("NextRefreshTime")
    );

    // A second call maps to the SAME database user — the mapping is the caller's
    // identity, not a value drawn per call — and mints a DIFFERENT password.
    let second = emulator
        .call(
            "GetClusterCredentialsWithIAM",
            &[("ClusterIdentifier", identifier)],
        )
        .await;
    assert_eq!(
        second.status, 200,
        "case a second call: expected 200, got {} with body {}",
        second.status, second.body
    );
    assert_eq!(
        second.text("DbUser"),
        first.text("DbUser"),
        "case a second call: expected the same database user, got {:?} then {:?}",
        first.text("DbUser"),
        second.text("DbUser")
    );
    assert_ne!(
        second.text("DbPassword").unwrap_or_default(),
        password,
        "case a second call: expected a freshly minted password, got the first one again"
    );
}

/// The lifetime the request asks for decides the expiry, inside the documented
/// window — and a lifetime outside that window is refused with no credential
/// issued.
#[tokio::test]
async fn the_credential_lifetime_honours_the_documented_window() {
    let emulator = Emulator::new();
    let identifier = "iam-credential-window-cluster";
    serving_cluster(&emulator, "the documented lifetime window", identifier).await;

    // The window is documented on the member itself: "Range: 900-3600. Default:
    // 900" (`smithy_query(query="doc", shape="GetClusterCredentialsWithIAMMessage",
    // member="DurationSeconds")`), so both published bounds and one value past
    // each are rows here.
    let served: Vec<(&str, String)> = vec![
        (
            "the documented minimum",
            state::CREDENTIAL_DURATION_MIN_SECONDS.to_string(),
        ),
        (
            "the documented maximum",
            state::CREDENTIAL_DURATION_MAX_SECONDS.to_string(),
        ),
        ("a lifetime inside the window", "1800".to_string()),
    ];
    let mut expirations: Vec<(String, String)> = Vec::new();
    for (case, duration) in served {
        let answer = emulator
            .call(
                "GetClusterCredentialsWithIAM",
                &[
                    ("ClusterIdentifier", identifier),
                    ("DurationSeconds", &duration),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 for DurationSeconds={duration}, got {} with body {}",
            answer.status, answer.body
        );
        let expiration = answer.text("Expiration").unwrap_or_default();
        assert!(
            !expiration.is_empty(),
            "case {case}: expected an expiry, got body {}",
            answer.body
        );
        expirations.push((duration, expiration));
    }
    // A longer lifetime expires later: the expiry is this call's instant plus the
    // lifetime, so the ORDER of the three is the order of the lifetimes asked
    // for — a difference of minutes, which no host delay can reorder. An ISO-8601
    // instant in UTC sorts chronologically as text, so the comparison needs no
    // clock of its own.
    let shortest = expirations
        .iter()
        .find(|(duration, _)| duration == &state::CREDENTIAL_DURATION_MIN_SECONDS.to_string())
        .map(|(_, instant)| instant.clone())
        .unwrap_or_default();
    let longest = expirations
        .iter()
        .find(|(duration, _)| duration == &state::CREDENTIAL_DURATION_MAX_SECONDS.to_string())
        .map(|(_, instant)| instant.clone())
        .unwrap_or_default();
    assert!(
        longest > shortest,
        "case the documented maximum: expected a later expiry than the minimum's \
         ({longest} vs {shortest})"
    );

    // A lifetime outside the window is refused, and the refusal names the window
    // rather than the value the request sent.
    for (case, duration) in [
        ("one second under the documented minimum", "899"),
        ("one second over the documented maximum", "3601"),
        ("no lifetime at all", "0"),
    ] {
        let refused = emulator
            .call(
                "GetClusterCredentialsWithIAM",
                &[
                    ("ClusterIdentifier", identifier),
                    ("DurationSeconds", duration),
                ],
            )
            .await;
        assert_refused(case, &refused, 400, "InvalidParameterValue");
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(state::credential_duration_out_of_range_message().as_str()),
            "case {case}: expected the documented window named, got {:?}",
            refused.text("Message")
        );
    }
}

/// The cluster is named by `ClusterIdentifier` or by the custom domain one of this
/// account's clusters serves, and a request naming neither names no database to
/// log on to.
#[tokio::test]
async fn the_cluster_the_credentials_are_for_has_to_be_named() {
    let emulator = Emulator::new();
    let identifier = "iam-credential-named-cluster";
    serving_cluster(&emulator, "naming the cluster", identifier).await;

    // (case, the parameters, the status and code expected, or `None` when the
    // call is served)
    let rows: Vec<NamingRow> = vec![
        (
            "the cluster's own identifier",
            vec![("ClusterIdentifier", identifier)],
            None,
        ),
        (
            "an identifier no cluster of this account carries",
            vec![("ClusterIdentifier", "nowhere-cluster")],
            Some((404, "ClusterNotFound")),
        ),
        (
            "a custom domain no cluster of this account serves",
            vec![("CustomDomainName", "nowhere.example.com")],
            Some((404, "ClusterNotFound")),
        ),
        (
            "an identifier and a domain that name different clusters",
            vec![
                ("ClusterIdentifier", identifier),
                ("CustomDomainName", "nowhere.example.com"),
            ],
            Some((404, "ClusterNotFound")),
        ),
        (
            "neither member",
            Vec::new(),
            Some((400, "InvalidParameterCombination")),
        ),
    ];

    for (case, params, refusal) in rows {
        let answer = emulator.call("GetClusterCredentialsWithIAM", &params).await;
        match refusal {
            None => assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with body {}",
                answer.status, answer.body
            ),
            Some((status, code)) => assert_refused(case, &answer, status, code),
        }
    }
}

/// A `DbName` outside the documented database-name syntax is refused, under the
/// same rule every other database name of this service is checked by — and the
/// cluster is resolved BEFORE it, so a request that gets both wrong is answered
/// about the cluster.
#[tokio::test]
async fn the_database_name_follows_the_documented_syntax() {
    let emulator = Emulator::new();
    let identifier = "iam-credential-dbname-cluster";
    serving_cluster(&emulator, "the database name syntax", identifier).await;

    // (case, the `DbName`, whether it is admitted)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a plain name", "analytics", true),
        ("a name carrying the admitted punctuation", "sales_db.v2", true),
        ("a name starting with a digit", "9analytics", false),
        ("a name carrying a slash", "sales/db", false),
        ("an empty name", "", false),
    ];

    for (case, db_name, admitted) in rows {
        let answer = emulator
            .call(
                "GetClusterCredentialsWithIAM",
                &[("ClusterIdentifier", identifier), ("DbName", db_name)],
            )
            .await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 for DbName={db_name:?}, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    // A request that names neither a cluster this account holds nor a usable
    // database name is answered about the CLUSTER: that is the first thing the
    // operation resolves.
    let both_wrong = emulator
        .call(
            "GetClusterCredentialsWithIAM",
            &[
                ("ClusterIdentifier", "nowhere-cluster"),
                ("DbName", "9analytics"),
            ],
        )
        .await;
    assert_refused(
        "a request that gets the cluster and the database name wrong",
        &both_wrong,
        404,
        "ClusterNotFound",
    );
}
