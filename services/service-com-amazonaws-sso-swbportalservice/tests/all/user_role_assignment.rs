// SPDX-License-Identifier: CC0-1.0

//! The role dimension of an assignment belongs to the (user, account) pair.
//!
//! RULE: the roles the portal reports for an account are the ones assigned to the user of
//! the PRESENTED sign-in there — so two sign-ins assigned differently in one account
//! report different role lists, and a role vends credentials only to the sign-in it is
//! assigned to.
//!
//! Every test below seeds one shared account and several sign-ins in it, and asserts the
//! rule over a table whose rows differ in how many roles the assignment names (none, one,
//! several), how they overlap another row's (disjoint, overlapping, a strict subset), and
//! how the names sort and spell.

use http::StatusCode;
use std::collections::BTreeSet;

use crate::portal_session::{body_json, body_text, error_code, Portal};

/// The account every sign-in below is assigned in, so nothing but the assignment itself
/// can distinguish their role listings. Its record (name and email) is seeded by
/// [`Portal::new`]; the roles are not, because they are not the account's.
const SHARED_ACCOUNT: &str = "111111111111";

/// A second seeded account, assigned to one sign-in only: the account dimension stays
/// per-user alongside the role dimension.
const OTHER_ACCOUNT: &str = "222222222222";

/// One sign-in, and the role names its user's assignment grants in [`SHARED_ACCOUNT`].
///
/// `expected` is `roles` in the ascending order `RoleListType` is reported in, which is
/// not the order the rows are written in — the rule is about the assignment, not about
/// how a fixture happened to spell it.
struct Row {
    token: &'static str,
    roles: &'static [&'static str],
}

/// The sign-ins sharing [`SHARED_ACCOUNT`]. Role names are permission-set names, so they
/// vary in case, digits, hyphens and underscores, and sort across each other.
const ROWS: &[Row] = &[
    Row {
        token: "tok-admin",
        roles: &["ReadOnly", "AdministratorAccess"],
    },
    // A strict subset of the row above: overlapping assignments must still differ.
    Row {
        token: "tok-reader",
        roles: &["ReadOnly"],
    },
    // Disjoint from both, and written out of order.
    Row {
        token: "tok-auditor",
        roles: &["ZZZ-Break-Glass", "Audit-Role_1", "Billing"],
    },
    // Assigned to the account and to no role in it: an empty list, not an error.
    Row {
        token: "tok-newcomer",
        roles: &[],
    },
];

/// `roles` in the order a role listing reports them: ascending, each name once.
fn ascending(roles: &[&str]) -> Vec<String> {
    roles
        .iter()
        .map(|role| (*role).to_string())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Every role name any row is assigned in [`SHARED_ACCOUNT`], ascending — the pool a
/// session may ask for and the pool it must be refused from.
fn every_role() -> Vec<String> {
    let mut roles: BTreeSet<String> = BTreeSet::new();
    for row in ROWS {
        roles.extend(row.roles.iter().map(|role| (*role).to_string()));
    }
    roles.into_iter().collect()
}

/// An emulator whose [`SHARED_ACCOUNT`] carries every row's sign-in, each with only its
/// own roles, plus one of them assigned a role in [`OTHER_ACCOUNT`] as well.
fn portal() -> Portal {
    let portal = Portal::new();
    for row in ROWS {
        portal.sign_in_assigned(row.token, [(SHARED_ACCOUNT, row.roles.to_vec())]);
    }
    // The same user, assigned in a second account: the account's own assignment decides
    // its roles, not the first account's.
    portal.sign_in_assigned(
        "tok-two-accounts",
        [
            (SHARED_ACCOUNT, vec!["ReadOnly"]),
            (OTHER_ACCOUNT, vec!["AdministratorAccess", "Billing"]),
        ],
    );
    portal
}

/// The `roleName`s a role listing reported, in the order it reported them.
async fn reported_roles(portal: &Portal, token: &str, account_id: &str) -> Vec<String> {
    let response = portal
        .get(
            &format!("/assignment/roles?account_id={account_id}"),
            Some(token),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "case {token:?} in {account_id:?}: expected 200, got {} {:?}",
        response.status(),
        body_text(&response)
    );
    let json = body_json(&response);
    let roles = json["roleList"].as_array().cloned().unwrap_or_default();
    for role in &roles {
        assert_eq!(
            role["accountId"].as_str().unwrap_or_default(),
            account_id,
            "case {token:?} in {account_id:?}: every RoleInfo must carry its own account, \
             got {role:?}"
        );
    }
    roles
        .iter()
        .map(|role| role["roleName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: `ListAccountRoles` reports the roles assigned to the PRESENTED sign-in's user in
/// the account, so sign-ins sharing one account report their own lists and nothing else.
#[tokio::test]
async fn list_account_roles_reports_the_asking_sessions_own_assignment() {
    let portal = portal();
    for row in ROWS {
        let reported = reported_roles(&portal, row.token, SHARED_ACCOUNT).await;
        let expected = ascending(row.roles);
        assert_eq!(
            reported, expected,
            "case {:?}: expected the roles assigned to this sign-in ({expected:?}), got \
             {reported:?}",
            row.token
        );
        // Nothing another user is assigned in the same account leaks in.
        for other in ROWS.iter().filter(|other| other.token != row.token) {
            for role in other.roles {
                let theirs_only = !row.roles.contains(role);
                assert!(
                    !(theirs_only && reported.contains(&(*role).to_string())),
                    "case {:?} vs {:?}: a role assigned only to the other sign-in \
                     ({role:?}) must not be reported, got {reported:?}",
                    row.token,
                    other.token
                );
            }
        }
    }
}

/// RULE: the assignment is per (user, account), so one sign-in's two accounts report the
/// roles each account's own assignment names.
#[tokio::test]
async fn one_sessions_accounts_report_their_own_assignments() {
    let portal = portal();
    // `(account, the roles this sign-in is assigned there)`.
    let cases: &[(&str, &[&str])] = &[
        (SHARED_ACCOUNT, &["ReadOnly"]),
        (OTHER_ACCOUNT, &["AdministratorAccess", "Billing"]),
    ];
    for (account_id, roles) in cases {
        let reported = reported_roles(&portal, "tok-two-accounts", account_id).await;
        let expected = ascending(roles);
        assert_eq!(
            reported, expected,
            "case {account_id:?}: expected {expected:?}, got {reported:?}"
        );
    }
    // And an account no assignment of this sign-in names does not exist for it.
    let response = portal
        .get(
            "/assignment/roles?account_id=333333333333",
            Some("tok-two-accounts"),
        )
        .await;
    assert_eq!(
        (response.status(), error_code(&response).as_str()),
        (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
        "case \"an unassigned account\": expected 404 ResourceNotFoundException, got {} {:?}",
        response.status(),
        body_text(&response)
    );
}

/// RULE: a role credential is vended for an (account, role) the PRESENTED sign-in is
/// assigned, and refused as absent for one only another user of the same account holds.
#[tokio::test]
async fn get_role_credentials_vends_only_the_asking_sessions_roles() {
    let portal = portal();
    for row in ROWS {
        for role_name in every_role() {
            let case = format!("{} asking for {role_name}", row.token);
            let assigned = row.roles.contains(&role_name.as_str());
            let response = portal
                .get(
                    &format!(
                        "/federation/credentials?account_id={SHARED_ACCOUNT}\
                         &role_name={role_name}"
                    ),
                    Some(row.token),
                )
                .await;
            if assigned {
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "case {case:?}: an assigned role must vend credentials, got {} {:?}",
                    response.status(),
                    body_text(&response)
                );
                let credentials = body_json(&response)["roleCredentials"].clone();
                assert!(
                    !credentials["accessKeyId"]
                        .as_str()
                        .unwrap_or_default()
                        .is_empty(),
                    "case {case:?}: the vended credential must carry an access key id, \
                     got {credentials:?}"
                );
            } else {
                assert_eq!(
                    (response.status(), error_code(&response).as_str()),
                    (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
                    "case {case:?}: a role this sign-in is not assigned must be 404 \
                     ResourceNotFoundException, got {} {:?}",
                    response.status(),
                    body_text(&response)
                );
            }
        }
    }
}

/// RULE: the roles live on the assignment, so reassigning ONE sign-in changes what that
/// sign-in reports and leaves every other sign-in in the account reporting its own.
#[tokio::test]
async fn reassigning_one_session_leaves_the_others_untouched() {
    let portal = portal();
    // Rows: a wider assignment, a narrower one, and none at all — each applied to the
    // same sign-in in turn.
    let reassignments: &[&[&str]] = &[
        &["AdministratorAccess", "Billing", "ReadOnly"],
        &["Billing"],
        &[],
    ];
    for roles in reassignments {
        portal.sign_in_assigned("tok-reader", [(SHARED_ACCOUNT, roles.to_vec())]);
        let reported = reported_roles(&portal, "tok-reader", SHARED_ACCOUNT).await;
        let expected = ascending(roles);
        assert_eq!(
            reported, expected,
            "case {roles:?}: the reassigned sign-in must report {expected:?}, got \
             {reported:?}"
        );
        for row in ROWS.iter().filter(|row| row.token != "tok-reader") {
            let others = reported_roles(&portal, row.token, SHARED_ACCOUNT).await;
            let expected = ascending(row.roles);
            assert_eq!(
                others, expected,
                "case {roles:?}: {:?} must still report its own {expected:?}, got \
                 {others:?}",
                row.token
            );
        }
    }
}
