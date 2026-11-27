// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a PostgreSQL endpoint answers a credential that did not admit a login with.
//!
//! THE RULE: a PostgreSQL server refuses a credential in the words of the authentication METHOD
//! the login was checked by, and Amazon RDS checks a database user enabled for IAM database
//! authentication through PAM — so such a user's refusal is `PAM authentication failed for user
//! "<user>"` under SQLSTATE `28000`, the class every method other than the password ones reports,
//! while a user that logs in with a password keeps `password authentication failed for user
//! "<user>"` under `28P01`.
//!
//! That is what tells a client which credential to renew: a token past its fifteen-minute window
//! ("the authentication token has a lifetime of 15 minutes", Amazon RDS User Guide, IAM database
//! authentication) must be answered promptly, and answered as a token refusal, so
//! credential-rotation logic knows to mint a new one rather than to look for a password.
//!
//! The rows below span what the rule generalises over: which authentication method the database
//! user logs in by, and what the login presented — a token outside its window, a token for
//! another Region, a password where only a token admits, no credential at all, and a wrong
//! password for a user that logs in with one.

use std::time::{Duration, SystemTime};

use crate::db_instance_engine_endpoint::{
    available_instance, master_session, must_run, postgres_login, Refusal, Transport, DATABASE,
    MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{emulator, REGION};
use crate::engine_role_grants::auth_token;

/// The database user enabled for IAM database authentication: a member of `rds_iam`, which is
/// how RDS for PostgreSQL records that this user authenticates with a token.
const TOKEN_USER: &str = "tokenuser";

/// A database user nothing enabled for it, which logs in with a password like any other.
const PASSWORD_USER: &str = "passworduser";

/// How long a token states it is usable for, which is the lifetime the SDKs presign one with.
const TOKEN_LIFETIME: u64 = 900;

/// How far in the PAST a token the rows refuse was signed: a whole lifetime and a minute more, so
/// its window closed before the login — the case a client whose token has expired is in.
const PAST_ITS_WINDOW: Duration = Duration::from_secs(TOKEN_LIFETIME + 60);

/// The refusal a user that authenticates with a TOKEN is answered with.
fn pam_refusal(user: &str) -> Refusal {
    Refusal {
        code: "28000".to_string(),
        message: format!("PAM authentication failed for user \"{user}\""),
    }
}

/// The refusal a user that authenticates with a PASSWORD is answered with.
fn password_refusal(user: &str) -> Refusal {
    Refusal {
        code: "28P01".to_string(),
        message: format!("password authentication failed for user \"{user}\""),
    }
}

#[tokio::test]
async fn a_refused_login_is_worded_for_the_method_the_user_authenticates_by() {
    let router = emulator();
    let case = "an instance serving IAM database authentication";
    let (address, port) = available_instance(
        &router,
        "emu-token-refusal",
        "postgres",
        &[(
            "EnableIAMDatabaseAuthentication",
            "true".to_string(),
        )],
    )
    .await;
    // The engine half of the decision: one user enabled for token authentication, and one that
    // is not.
    let mut master = master_session(case, &address, port).await;
    must_run(case, &mut master, &format!("CREATE USER {TOKEN_USER} WITH LOGIN")).await;
    must_run(case, &mut master, &format!("GRANT rds_iam TO {TOKEN_USER}")).await;
    must_run(
        case,
        &mut master,
        &format!("CREATE USER {PASSWORD_USER} WITH LOGIN"),
    )
    .await;

    let now = SystemTime::now();
    let rows: Vec<(&str, &str, String, Refusal)> = vec![
        (
            "a token whose window closed before the login",
            TOKEN_USER,
            auth_token(
                &address,
                port,
                TOKEN_USER,
                REGION,
                now - PAST_ITS_WINDOW,
                TOKEN_LIFETIME,
            ),
            pam_refusal(TOKEN_USER),
        ),
        (
            "a token signed for another Region",
            TOKEN_USER,
            auth_token(&address, port, TOKEN_USER, "eu-west-1", now, TOKEN_LIFETIME),
            pam_refusal(TOKEN_USER),
        ),
        (
            "a password where only a token admits",
            TOKEN_USER,
            MASTER_PASSWORD.to_string(),
            pam_refusal(TOKEN_USER),
        ),
        (
            "no credential at all where only a token admits",
            TOKEN_USER,
            String::new(),
            pam_refusal(TOKEN_USER),
        ),
        (
            "a wrong password for a user that logs in with one",
            PASSWORD_USER,
            "not-the-password".to_string(),
            password_refusal(PASSWORD_USER),
        ),
        (
            "a token presented by a user that logs in with a password",
            PASSWORD_USER,
            auth_token(&address, port, PASSWORD_USER, REGION, now, TOKEN_LIFETIME),
            password_refusal(PASSWORD_USER),
        ),
        (
            "the master user with the wrong password",
            MASTER_USER,
            "not-the-password".to_string(),
            password_refusal(MASTER_USER),
        ),
    ];
    for (row, user, credential, expected) in rows {
        let refusal = postgres_login(
            &address,
            port,
            Transport::Tls,
            user,
            &credential,
            Some(DATABASE),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("case {row:?}: expected the login to be refused"));
        assert_eq!(
            refusal, expected,
            "case {row:?}: expected {expected:?}, got {refusal:?}"
        );
    }

    // And the method a user logs in by follows the engine's own record of it: the token user
    // taken out of `rds_iam` logs in with a password from then on, and is refused in those
    // words.
    must_run(
        case,
        &mut master,
        &format!("REVOKE rds_iam FROM {TOKEN_USER}"),
    )
    .await;
    let refusal = postgres_login(
        &address,
        port,
        Transport::Tls,
        TOKEN_USER,
        &auth_token(&address, port, TOKEN_USER, REGION, now, TOKEN_LIFETIME),
        Some(DATABASE),
    )
    .await
    .err()
    .unwrap_or_else(|| panic!("case {case:?}: expected the login to be refused"));
    assert_eq!(
        refusal,
        password_refusal(TOKEN_USER),
        "case {case:?}: a user no longer enabled for token authentication must be refused as a \
         password login, got {refusal:?}"
    );
}
