// SPDX-License-Identifier: CC0-1.0

//! Revoking another account's ability to restore a snapshot, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the seed and helpers of
//! [`crate::snapshot_access`], so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: `RevokeSnapshotAccess`
//! removes the ONE account the request names from the set of accounts the named
//! snapshot may be restored by, reports that snapshot back, and persists the
//! shrunken set for a later read, while an account the set does not hold is
//! refused with the modelled access fault and changes nothing. It is
//! asserted over a TABLE of grantee forms (account identifier, the documented
//! Amazon Web Services Support alias), over every way a request can name the
//! snapshot (identifier in any case, ARN, the pair), over the refusals (an
//! unknown snapshot, a snapshot of another cluster, an account holding no
//! access, a repeated revoke, a malformed grantee) and over the bound the
//! released entry makes room under.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};
use crate::snapshot_access::{
    assert_refused, authorize, creating_emulator, describe, granted_aliases, granted_ids,
    seeded_emulator, LocatorCase, RefusalCase, CLUSTER, OTHER_SNAPSHOT, SNAPSHOT,
};

/// The Region the shared harness signs every request for, which the snapshot
/// ARNs are derived from.
const REGION: &str = "us-west-2";

/// `RevokeSnapshotAccess` for `account`, naming the snapshot with `locator`.
async fn revoke(emulator: &Emulator, account: &str, locator: &[(&str, &str)]) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![("AccountWithRestoreAccess", account)];
    params.extend_from_slice(locator);
    emulator.call("RevokeSnapshotAccess", &params).await
}

/// Grant every account in `accounts` restore access to [`SNAPSHOT`], asserting
/// each grant was admitted.
async fn grant_all(emulator: &Emulator, accounts: &[&str]) {
    for account in accounts {
        let answer = authorize(emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_eq!(
            answer.status, 200,
            "case seed grant {account}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
    }
}

/// A revoke releases the ONE grantee the request names, in whichever shape that
/// grantee is held, and leaves every other grant in place — and the set it
/// persists is what a later read reports, down to the member going absent once
/// the last grant is released.
#[tokio::test]
async fn a_revoke_releases_the_grantee_it_names_and_leaves_every_other_grant() {
    let (emulator, _) = seeded_emulator().await;
    let alias = state::SUPPORT_ACCOUNT_ALIAS;
    // Three account identifiers (including the all-nines bound of that width)
    // and the documented Amazon Web Services Support alias, so the rows differ
    // along the FORM of the grantee the rule generalises over.
    grant_all(
        &emulator,
        &["111122223333", "444455556666", "999999999999", alias],
    )
    .await;

    // Each row names one grantee to release and the set that must remain, so a
    // revoke that released the wrong entry — or more than one — fails here. The
    // middle identifier goes first, so "the entry it names" is not the first or
    // the last of the list.
    let rows: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        (
            "an identifier from the middle of the set",
            "444455556666",
            vec!["111122223333", "999999999999"],
            vec![alias],
        ),
        (
            "the support alias",
            alias,
            vec!["111122223333", "999999999999"],
            vec![],
        ),
        (
            "the last identifier of the set",
            "999999999999",
            vec!["111122223333"],
            vec![],
        ),
        (
            "the only identifier left",
            "111122223333",
            vec![],
            vec![],
        ),
    ];

    for (case, account, expected_ids, expected_aliases) in rows {
        let answer = revoke(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        // The operation reports the snapshot it stopped sharing, identified as
        // stored and otherwise untouched.
        assert_eq!(
            answer.text("SnapshotIdentifier").as_deref(),
            Some(SNAPSHOT),
            "case {case}: expected the stored snapshot identifier back, got {:?}",
            answer.text("SnapshotIdentifier")
        );
        assert_eq!(
            answer.text("Status").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the snapshot to stay {}, got {:?}",
            state::STATUS_AVAILABLE,
            answer.text("Status")
        );
        assert_eq!(
            granted_ids(&answer),
            expected_ids,
            "case {case}: expected the account identifiers {expected_ids:?} to remain, got {:?}",
            granted_ids(&answer)
        );
        assert_eq!(
            granted_aliases(&answer),
            expected_aliases,
            "case {case}: expected the account aliases {expected_aliases:?} to remain, got {:?}",
            granted_aliases(&answer)
        );

        // ROUND TRIP: the set the revoke persisted is what a later read reports.
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

    // `Snapshot$AccountsWithRestoreAccess` "returns null if no accounts are
    // authorized", so a snapshot whose last grant was released reports neither
    // an identifier nor an alias — the shape a snapshot nobody shared is in.
    let emptied = describe(&emulator, SNAPSHOT).await;
    assert!(
        granted_ids(&emptied).is_empty() && granted_aliases(&emptied).is_empty(),
        "case the set emptied: expected no authorized accounts, got {:?} / {:?}",
        granted_ids(&emptied),
        granted_aliases(&emptied)
    );
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

    // Each row releases a DIFFERENT account, so a row cannot pass on the entry
    // an earlier row released.
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

    let held: Vec<&str> = rows.iter().map(|(_, account, _)| *account).collect();
    grant_all(&emulator, &held).await;

    let mut remaining: Vec<String> = held.iter().map(|a| (*a).to_string()).collect();
    for (case, account, locator) in &rows {
        let answer = revoke(&emulator, account, locator).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        remaining.retain(|held| held != account);
        assert_eq!(
            granted_ids(&answer),
            remaining,
            "case {case}: expected the one record to hold {remaining:?}, got {:?}",
            granted_ids(&answer)
        );
    }

    // A pair of locators that name different snapshots names neither, so a
    // revoke carrying such a pair finds no snapshot at all.
    let regranted = "100000000007";
    grant_all(&emulator, &[regranted]).await;
    let disagreeing = revoke(
        &emulator,
        regranted,
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
        vec![regranted.to_string()],
        "case a pair naming two snapshots: expected the refused revoke to release nothing, got {:?}",
        granted_ids(&described)
    );
}

/// Only an authorization the snapshot HOLDS can be released: an account that was
/// never granted access, and one whose grant an earlier revoke already released,
/// are refused with the modelled access fault and change nothing.
#[tokio::test]
async fn a_revoke_of_an_account_holding_no_access_is_refused_and_changes_nothing() {
    let (emulator, arn) = seeded_emulator().await;
    let held = "222233334444";
    grant_all(&emulator, &[held]).await;

    // The rows differ along WHY the named grantee holds no access: never
    // granted, granted to a different account, the support alias while only
    // identifiers are held — and, last, an account whose grant this test
    // releases first, so the second revoke of it is the repeat case.
    let rows: Vec<LocatorCase> = vec![
        (
            "an account never granted access",
            "555566667777",
            vec![("SnapshotIdentifier", SNAPSHOT)],
        ),
        (
            "an account never granted access, named by ARN",
            "888899990000",
            vec![("SnapshotArn", &arn)],
        ),
        (
            "the support alias while only identifiers are held",
            state::SUPPORT_ACCOUNT_ALIAS,
            vec![("SnapshotIdentifier", SNAPSHOT)],
        ),
    ];

    for (case, account, locator) in rows {
        let before = describe(&emulator, SNAPSHOT).await;
        let answer = revoke(&emulator, account, &locator).await;
        assert_refused(case, &answer, 400, "AccessToSnapshotDenied");
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

    // A grant is released ONCE: the second revoke of the same account finds no
    // authorization to release.
    let first = revoke(&emulator, held, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_eq!(
        first.status, 200,
        "case the first revoke of a held grant: expected 200, got {} with body {}",
        first.status, first.body
    );
    let again = revoke(&emulator, held, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_refused(
        "the same account revoked twice",
        &again,
        400,
        "AccessToSnapshotDenied",
    );
}

/// A request that names no snapshot this store holds, or no account at all, is
/// refused — and releases nothing.
#[tokio::test]
async fn a_request_that_names_no_snapshot_or_no_account_is_refused() {
    let (emulator, _) = seeded_emulator().await;
    let account = "333344445555";
    grant_all(&emulator, &[account]).await;
    let too_long = "1111222233334";
    let too_short = "11112222333";

    // The rows differ along what each one gets wrong: the snapshot locator
    // (absent, empty, unknown by identifier, unknown by ARN, of another
    // cluster).
    let rows: Vec<RefusalCase> = vec![
        (
            "an unknown snapshot identifier",
            vec![("SnapshotIdentifier", "no-such-snapshot")],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "an unknown snapshot ARN",
            vec![(
                "SnapshotArn",
                "arn:aws:redshift:us-west-2:000000000000:snapshot:nope/nope",
            )],
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
        let answer = revoke(&emulator, account, &locator).await;
        assert_refused(case, &answer, status, code);
    }

    // The grantee's own form: an account identifier is 12 digits, or the
    // documented support alias exactly.
    let malformed: Vec<(&str, &str)> = vec![
        ("an empty account", ""),
        ("an account one digit short", too_short),
        ("an account one digit long", too_long),
        ("an account carrying a letter", "11112222333a"),
        ("a name that is no account", "not-an-account"),
        ("the support alias miscased", "Amazon-Redshift-Support"),
    ];
    for (case, value) in malformed {
        let answer = revoke(&emulator, value, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_refused(case, &answer, 400, "ValidationException");
    }

    // Every row above was a refusal, so the one grant is still held.
    let described = describe(&emulator, SNAPSHOT).await;
    assert_eq!(
        granted_ids(&described),
        vec![account.to_string()],
        "case a refused request: expected the grant still held, got {:?}",
        granted_ids(&described)
    );
}

/// A revoke releases the grant on the snapshot the request named and on no
/// other, even when both snapshots share the same account.
#[tokio::test]
async fn a_revoke_touches_only_the_snapshot_it_names() {
    let (emulator, _) = creating_emulator(&[SNAPSHOT, OTHER_SNAPSHOT]).await;
    emulator.advance_clock(state::PROVISIONING_SPAN);
    let account = "777788889999";

    for snapshot in [SNAPSHOT, OTHER_SNAPSHOT] {
        let granted = authorize(&emulator, account, &[("SnapshotIdentifier", snapshot)]).await;
        assert_eq!(
            granted.status, 200,
            "case seed grant on {snapshot}: expected 200, got {} with body {}",
            granted.status, granted.body
        );
    }

    let answer = revoke(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_eq!(
        answer.status, 200,
        "case a revoke on one of two snapshots: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    let named = describe(&emulator, SNAPSHOT).await;
    assert!(
        granted_ids(&named).is_empty(),
        "case the snapshot the revoke named: expected no grant left, got {:?}",
        granted_ids(&named)
    );
    let other = describe(&emulator, OTHER_SNAPSHOT).await;
    assert_eq!(
        granted_ids(&other),
        vec![account.to_string()],
        "case the snapshot nobody revoked: expected its grant untouched, got {:?}",
        granted_ids(&other)
    );
}

/// A revoke asserts nothing about the snapshot's own status: a snapshot still
/// being created holds no grant, so the request is the access fault — not the
/// snapshot-state fault the grant path answers with.
#[tokio::test]
async fn a_revoke_asserts_nothing_about_the_snapshots_status() {
    let (emulator, _) = creating_emulator(&[SNAPSHOT]).await;
    let account = "666677778888";

    let while_creating = revoke(&emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_refused(
        "a revoke on a snapshot still creating",
        &while_creating,
        400,
        "AccessToSnapshotDenied",
    );
}

/// The set of accounts one snapshot may be restored by is BOUNDED, and a release
/// makes room under that bound: the grant refused at the bound is admitted once
/// a revoke has freed an entry, and the set never grows past the bound.
#[tokio::test]
async fn releasing_a_grant_makes_room_under_the_bound() {
    let (emulator, _) = seeded_emulator().await;
    let quota = state::RESTORE_ACCOUNTS_PER_SNAPSHOT_QUOTA;

    // Distinct 12-digit account identifiers, one per grant up to the bound and
    // one past it.
    let accounts: Vec<String> = (0..=quota)
        .map(|index| format!("{:012}", 100_000_000_000u64 + index as u64))
        .collect();
    let at_bound: Vec<&str> = accounts.iter().take(quota).map(String::as_str).collect();
    grant_all(&emulator, &at_bound).await;

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

    let released = revoke(&emulator, &accounts[0], &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_eq!(
        released.status, 200,
        "case a release at the bound: expected 200, got {} with body {}",
        released.status, released.body
    );
    assert_eq!(
        granted_ids(&released).len(),
        quota - 1,
        "case a release at the bound: expected {} grants left, got {:?}",
        quota - 1,
        granted_ids(&released)
    );

    let admitted = authorize(
        &emulator,
        &accounts[quota],
        &[("SnapshotIdentifier", SNAPSHOT)],
    )
    .await;
    assert_eq!(
        admitted.status, 200,
        "case the grant the release made room for: expected 200, got {} with body {}",
        admitted.status, admitted.body
    );
    let after = describe(&emulator, SNAPSHOT).await;
    assert_eq!(
        granted_ids(&after).len(),
        quota,
        "case the grant the release made room for: expected the set back at {quota}, got {:?}",
        granted_ids(&after)
    );
    assert!(
        !granted_ids(&after).contains(&accounts[0]),
        "case the grant the release made room for: expected the released account gone, got {:?}",
        granted_ids(&after)
    );
}
