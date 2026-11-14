// SPDX-License-Identifier: CC0-1.0

//! Authorizing another account to restore a snapshot, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: `AuthorizeSnapshotAccess`
//! adds the account the request names to the SET of accounts the named,
//! `available` snapshot may be restored by, reports that snapshot back, and
//! persists the set for a later read. It is asserted over a TABLE of grantee
//! forms (account identifier, the documented Amazon Web Services Support alias),
//! over every way a request can name the snapshot (identifier in any case, ARN,
//! the pair), over the refusals (an unknown snapshot, a snapshot of another
//! cluster, a snapshot still being created, a repeated grant, a malformed
//! grantee) and over the bound on the set and one grant past it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The Region the shared harness signs every request for, which the snapshot
/// ARNs are derived from.
const REGION: &str = "us-west-2";

/// The cluster every case takes its snapshot of.
///
/// `pub(crate)` so the revoke suite shares this suite's seed and helpers rather
/// than keeping a second copy of them.
pub(crate) const CLUSTER: &str = "share-source";

/// The snapshot every case shares, deliberately in MIXED case: a request names a
/// snapshot case-insensitively, so the stored spelling has to differ from the
/// one the locator rows send.
pub(crate) const SNAPSHOT: &str = "Share-Source-Snap";

/// A second snapshot of the same cluster, so "the grant lands on the snapshot the
/// request named" is observable rather than vacuous.
pub(crate) const OTHER_SNAPSHOT: &str = "share-source-other";

/// One row of the refusal table: the case name, the request parameters, and the
/// `(status, error code)` the service must answer with.
pub(crate) type RefusalCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// One row of a locator table: the case name, the account the row grants access
/// to, and the parameters that name the snapshot.
pub(crate) type LocatorCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

/// An emulator holding [`CLUSTER`] and a snapshot of it that is still
/// `creating` — the clock-owed `creating` -> `available` transition has not
/// fallen due yet. Returns the snapshot's ARN, which the locator rows name it by.
///
/// The emulator's clock is STOPPED before anything is created, so the records
/// stay in their transient state however long a case takes over the calls that
/// read them, and [`seeded_emulator`] crosses the span by advancing that clock.
pub(crate) async fn creating_emulator(snapshots: &[&str]) -> (Emulator, String) {
    let emulator = Emulator::new();
    emulator.stop_clock();
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", CLUSTER),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {CLUSTER}: expected 200, got {} with body {}",
        created.status, created.body
    );

    let mut first_arn = String::new();
    for snapshot in snapshots {
        let taken = emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", snapshot),
                    ("ClusterIdentifier", CLUSTER),
                ],
            )
            .await;
        assert_eq!(
            taken.status, 200,
            "case seed snapshot {snapshot}: expected 200, got {} with body {}",
            taken.status, taken.body
        );
        // A snapshot carries no grant until one is authorized: "Returns null if
        // no accounts are authorized."
        assert!(
            granted_ids(&taken).is_empty() && granted_aliases(&taken).is_empty(),
            "case seed snapshot {snapshot}: expected no authorized accounts, got {:?} / {:?}",
            granted_ids(&taken),
            granted_aliases(&taken)
        );
        if first_arn.is_empty() {
            first_arn = taken
                .text("SnapshotArn")
                .expect("case seed snapshot: the response carries the snapshot ARN");
        }
    }
    (emulator, first_arn)
}

/// An emulator holding [`CLUSTER`] and an AVAILABLE snapshot of it.
pub(crate) async fn seeded_emulator() -> (Emulator, String) {
    let (emulator, arn) = creating_emulator(&[SNAPSHOT]).await;
    // Only an `available` snapshot can be shared, and the transition is due a
    // span after the call that minted it — crossed by advancing the stopped
    // clock, so the record is settled before the first call rather than after a
    // wait the host's speed decides.
    emulator.advance_clock(state::PROVISIONING_SPAN);
    (emulator, arn)
}

/// `AuthorizeSnapshotAccess` for `account`, naming the snapshot with `locator`.
pub(crate) async fn authorize(
    emulator: &Emulator,
    account: &str,
    locator: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![("AccountWithRestoreAccess", account)];
    params.extend_from_slice(locator);
    emulator.call("AuthorizeSnapshotAccess", &params).await
}

/// The snapshot `identifier` as `DescribeClusterSnapshots` reports it.
pub(crate) async fn describe(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "DescribeClusterSnapshots",
            &[("SnapshotIdentifier", identifier)],
        )
        .await
}

/// Every `AccountId` an answer's `AccountsWithRestoreAccess` list reports, in
/// the order the record holds them.
pub(crate) fn granted_ids(answer: &Answer) -> Vec<String> {
    all_text(&answer.body, "AccountId")
}

/// Every `AccountAlias` an answer's `AccountsWithRestoreAccess` list reports, in
/// the order the record holds them.
pub(crate) fn granted_aliases(answer: &Answer) -> Vec<String> {
    all_text(&answer.body, "AccountAlias")
}

/// Assert one refusal row: the answer's status and `<Code>`.
pub(crate) fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
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

/// A grant is recorded in the shape its grantee takes — an account identifier
/// under `AccountId`, the documented Amazon Web Services Support alias under
/// `AccountAlias` — and the set ACCUMULATES: each call adds one entry, and a
/// later read reports every entry granted so far.
#[tokio::test]
async fn a_grant_is_recorded_in_the_shape_its_grantee_takes_and_the_set_accumulates() {
    let (emulator, _) = seeded_emulator().await;

    // The rows differ along the FORM of the grantee the rule generalises over.
    // The alias is the one `smithy_query(query="doc",
    // shape="AuthorizeSnapshotAccessMessage", member="AccountWithRestoreAccess")`
    // names ("To share a snapshot with Amazon Web Services Support, specify
    // amazon-redshift-support"); the account identifiers are three different
    // 12-digit values, including the all-nines bound of that width.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a plain account", "111122223333", false),
        ("a second account", "444455556666", false),
        ("an account of all nines", "999999999999", false),
        ("the support alias", state::SUPPORT_ACCOUNT_ALIAS, true),
    ];

    let mut expected_ids: Vec<String> = Vec::new();
    let mut expected_aliases: Vec<String> = Vec::new();
    for (case, account, is_alias) in rows {
        let answer = authorize(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        // The operation reports the snapshot it shared, identified as stored.
        assert_eq!(
            answer.text("SnapshotIdentifier").as_deref(),
            Some(SNAPSHOT),
            "case {case}: expected the stored snapshot identifier back, got {:?}",
            answer.text("SnapshotIdentifier")
        );
        assert_eq!(
            answer.text("Status").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the shared snapshot to stay {}, got {:?}",
            state::STATUS_AVAILABLE,
            answer.text("Status")
        );

        if is_alias {
            expected_aliases.push(account.to_string());
        } else {
            expected_ids.push(account.to_string());
        }
        assert_eq!(
            granted_ids(&answer),
            expected_ids,
            "case {case}: expected the account identifiers {expected_ids:?}, got {:?}",
            granted_ids(&answer)
        );
        assert_eq!(
            granted_aliases(&answer),
            expected_aliases,
            "case {case}: expected the account aliases {expected_aliases:?}, got {:?}",
            granted_aliases(&answer)
        );

        // ROUND TRIP: the set the grant persisted is what a later read reports.
        let described = describe(&emulator, SNAPSHOT).await;
        assert_eq!(
            described.status, 200,
            "case {case} read back: expected 200, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            granted_ids(&described),
            expected_ids,
            "case {case} read back: expected the persisted identifiers {expected_ids:?}, got {:?}",
            granted_ids(&described)
        );
        assert_eq!(
            granted_aliases(&described),
            expected_aliases,
            "case {case} read back: expected the persisted aliases {expected_aliases:?}, got {:?}",
            granted_aliases(&described)
        );
    }
}

/// Every way a request can name the snapshot reaches the same record: the
/// identifier in any case, the ARN, or both locators together — while a pair
/// whose two locators name different snapshots names neither.
#[tokio::test]
async fn every_way_of_naming_the_snapshot_reaches_the_same_record() {
    let (emulator, arn) = seeded_emulator().await;
    let lowercased = SNAPSHOT.to_ascii_lowercase();
    let uppercased = SNAPSHOT.to_ascii_uppercase();
    // A well-formed ARN of a snapshot this store does not hold, built by the
    // service's own rule, for the disagreeing-pair row.
    let foreign_arn = state::snapshot_arn(CLUSTER, "no-such-snapshot", REGION);

    // Each row grants a DIFFERENT account, so a row cannot pass on a grant an
    // earlier row left behind.
    let rows: Vec<LocatorCase> = vec![
        (
            "the identifier as stored",
            "100000000001",
            vec![("SnapshotIdentifier", SNAPSHOT)],
        ),
        (
            "the identifier lowercased",
            "100000000002",
            vec![("SnapshotIdentifier", &lowercased)],
        ),
        (
            "the identifier uppercased",
            "100000000003",
            vec![("SnapshotIdentifier", &uppercased)],
        ),
        ("the ARN alone", "100000000004", vec![("SnapshotArn", &arn)]),
        (
            "the identifier and its cluster",
            "100000000005",
            vec![
                ("SnapshotIdentifier", SNAPSHOT),
                ("SnapshotClusterIdentifier", CLUSTER),
            ],
        ),
        (
            "the identifier and its ARN",
            "100000000006",
            vec![("SnapshotIdentifier", SNAPSHOT), ("SnapshotArn", &arn)],
        ),
    ];

    let mut expected_ids: Vec<String> = Vec::new();
    for (case, account, locator) in rows {
        let answer = authorize(&emulator, account, &locator).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        expected_ids.push(account.to_string());
        assert_eq!(
            granted_ids(&answer),
            expected_ids,
            "case {case}: expected the one record to hold {expected_ids:?}, got {:?}",
            granted_ids(&answer)
        );
    }

    // A pair of locators that name different snapshots names neither, so the
    // grant that would have been the seventh is refused.
    let disagreeing = authorize(
        &emulator,
        "100000000007",
        &[
            ("SnapshotIdentifier", SNAPSHOT),
            ("SnapshotArn", &foreign_arn),
        ],
    )
    .await;
    assert_refused(
        "a pair naming two snapshots",
        &disagreeing,
        404,
        "ClusterSnapshotNotFound",
    );
    let described = describe(&emulator, SNAPSHOT).await;
    assert_eq!(
        granted_ids(&described),
        expected_ids,
        "case a pair naming two snapshots: expected the refused grant to change nothing, got {:?}",
        granted_ids(&described)
    );
}

/// The grants are a SET: granting an account that already holds restore access
/// is refused and leaves the set exactly as it was.
#[tokio::test]
async fn a_repeated_grant_is_refused_and_changes_nothing() {
    let (emulator, arn) = seeded_emulator().await;

    // Both grantee forms, and both ways of naming the snapshot, so the rule is
    // not keyed to the spelling of either.
    let rows: Vec<LocatorCase> = vec![
        (
            "an account granted twice by identifier",
            "222233334444",
            vec![("SnapshotIdentifier", SNAPSHOT)],
        ),
        (
            "an account granted again by ARN",
            "555566667777",
            vec![("SnapshotArn", &arn)],
        ),
        (
            "the support alias granted twice",
            state::SUPPORT_ACCOUNT_ALIAS,
            vec![("SnapshotIdentifier", SNAPSHOT)],
        ),
    ];

    for (case, account, locator) in rows {
        let first = authorize(&emulator, account, &locator).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected the first grant admitted with 200, got {} with body {}",
            first.status, first.body
        );
        let before = describe(&emulator, SNAPSHOT).await;

        let again = authorize(&emulator, account, &locator).await;
        assert_refused(case, &again, 400, "AuthorizationAlreadyExists");

        let after = describe(&emulator, SNAPSHOT).await;
        assert_eq!(
            granted_ids(&after),
            granted_ids(&before),
            "case {case}: expected the identifiers unchanged, got {:?}",
            granted_ids(&after)
        );
        assert_eq!(
            granted_aliases(&after),
            granted_aliases(&before),
            "case {case}: expected the aliases unchanged, got {:?}",
            granted_aliases(&after)
        );
    }
}

/// A request that names no snapshot this store holds, or no account at all, is
/// refused — and records nothing.
#[tokio::test]
async fn a_request_that_names_no_snapshot_or_no_account_is_refused() {
    let (emulator, _) = seeded_emulator().await;
    let account = "333344445555";
    let too_long = "1111222233334";
    let too_short = "11112222333";

    // The rows differ along what each one gets wrong: the snapshot locator
    // (absent, unknown, of another cluster) and the grantee's form (empty, both
    // widths one off the 12 digits an account identifier has, a non-digit, a
    // near-miss of the documented support alias).
    let rows: Vec<RefusalCase> = vec![
        (
            "an unknown snapshot identifier",
            vec![("SnapshotIdentifier", "no-such-snapshot")],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "an unknown snapshot ARN",
            vec![("SnapshotArn", "arn:aws:redshift:us-west-2:000000000000:snapshot:nope/nope")],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a snapshot of another cluster",
            vec![
                ("SnapshotIdentifier", SNAPSHOT),
                ("SnapshotClusterIdentifier", "some-other-cluster"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        ("no locator at all", vec![], 400, "ValidationException"),
        (
            "an empty locator",
            vec![("SnapshotIdentifier", "")],
            400,
            "ValidationException",
        ),
    ];

    for (case, locator, status, code) in rows {
        let answer = authorize(&emulator, account, &locator).await;
        assert_refused(case, &answer, status, code);
    }

    let malformed: Vec<(&str, &str)> = vec![
        ("an empty account", ""),
        ("an account one digit short", too_short),
        ("an account one digit long", too_long),
        ("an account carrying a letter", "11112222333a"),
        ("a name that is no account", "not-an-account"),
        ("the support alias miscased", "Amazon-Redshift-Support"),
    ];
    for (case, value) in malformed {
        let answer = authorize(&emulator, value, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_refused(case, &answer, 400, "ValidationException");
    }

    // Every row above was a refusal, so the snapshot still holds no grant.
    let described = describe(&emulator, SNAPSHOT).await;
    assert!(
        granted_ids(&described).is_empty() && granted_aliases(&described).is_empty(),
        "case a refused request: expected no grant recorded, got {:?} / {:?}",
        granted_ids(&described),
        granted_aliases(&described)
    );
}

/// A snapshot that is still being created is not in the `available` state, so it
/// cannot be shared yet — and the same request succeeds once the clock-owed
/// transition has fallen due.
#[tokio::test]
async fn a_snapshot_still_being_created_cannot_be_shared_yet() {
    let (emulator, _) = creating_emulator(&[SNAPSHOT]).await;
    let account = "666677778888";

    let too_early = authorize(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_refused(
        "a snapshot still creating",
        &too_early,
        400,
        "InvalidClusterSnapshotState",
    );

    emulator.advance_clock(state::PROVISIONING_SPAN);
    let later = authorize(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_eq!(
        later.status, 200,
        "case a snapshot the clock made available: expected 200, got {} with body {}",
        later.status, later.body
    );
    assert_eq!(
        granted_ids(&later),
        vec![account.to_string()],
        "case a snapshot the clock made available: expected the grant recorded, got {:?}",
        granted_ids(&later)
    );
}

/// A grant lands on the snapshot the request named and on no other.
#[tokio::test]
async fn a_grant_touches_only_the_snapshot_it_names() {
    let (emulator, _) = creating_emulator(&[SNAPSHOT, OTHER_SNAPSHOT]).await;
    emulator.advance_clock(state::PROVISIONING_SPAN);
    let account = "777788889999";

    let answer = authorize(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_eq!(
        answer.status, 200,
        "case a grant on one of two snapshots: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    let other = describe(&emulator, OTHER_SNAPSHOT).await;
    assert!(
        granted_ids(&other).is_empty() && granted_aliases(&other).is_empty(),
        "case the snapshot nobody shared: expected no grant, got {:?} / {:?}",
        granted_ids(&other),
        granted_aliases(&other)
    );

    // A read changes nothing a later read sees.
    let first = describe(&emulator, SNAPSHOT).await;
    let second = describe(&emulator, SNAPSHOT).await;
    assert_eq!(
        granted_ids(&second),
        granted_ids(&first),
        "case two reads of one snapshot: expected the same grants, got {:?} then {:?}",
        granted_ids(&first),
        granted_ids(&second)
    );
    assert_eq!(
        granted_ids(&first),
        vec![account.to_string()],
        "case two reads of one snapshot: expected the one grant, got {:?}",
        granted_ids(&first)
    );
}

/// The set of accounts one snapshot may be restored by is BOUNDED: the published
/// bound's worth of grants is admitted, and the one past it is refused with the
/// modelled quota fault, leaving the set at the bound.
#[tokio::test]
async fn the_authorized_set_stops_at_its_bound() {
    let (emulator, _) = seeded_emulator().await;
    let quota = state::RESTORE_ACCOUNTS_PER_SNAPSHOT_QUOTA;

    // Distinct 12-digit account identifiers, one per grant up to the bound and
    // one past it.
    let accounts: Vec<String> = (0..=quota)
        .map(|index| format!("{:012}", 100_000_000_000u64 + index as u64))
        .collect();

    for (index, account) in accounts.iter().take(quota).enumerate() {
        let answer = authorize(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_eq!(
            answer.status, 200,
            "case grant {} of {quota}: expected 200, got {} with body {}",
            index + 1,
            answer.status,
            answer.body
        );
    }

    let at_bound = describe(&emulator, SNAPSHOT).await;
    assert_eq!(
        granted_ids(&at_bound).len(),
        quota,
        "case the set at its bound: expected {quota} grants, got {:?}",
        granted_ids(&at_bound)
    );

    let past_bound = authorize(
        &emulator,
        &accounts[quota],
        &[("SnapshotIdentifier", SNAPSHOT)],
    )
    .await;
    assert_refused(
        "one grant past the bound",
        &past_bound,
        400,
        "AuthorizationQuotaExceeded",
    );

    let after = describe(&emulator, SNAPSHOT).await;
    assert_eq!(
        granted_ids(&after),
        granted_ids(&at_bound),
        "case one grant past the bound: expected the set unchanged, got {:?}",
        granted_ids(&after)
    );
}
