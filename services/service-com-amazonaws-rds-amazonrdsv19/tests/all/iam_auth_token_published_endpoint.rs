// SPDX-License-Identifier: CC0-1.0

//! WHICH endpoint an IAM database authentication token is scoped to: the endpoint the
//! control plane PUBLISHES for the DB instance.
//!
//! The rule under test: the `Endpoint{Address,Port}` a DB instance publishes is the one
//! address its engine data plane answers on, and that published pair is the pair an
//! `rds-db` `connect` token has to be scoped to — the AWS SDKs' `generate_db_auth_token`
//! presigns `<DBHostname>:<Port>/?Action=connect&DBUser=<user>&…`, so the token a client
//! mints from a describe begins with the published pair, and the endpoint admits exactly
//! that token: one scoped to another host or another port is refused on the very endpoint it
//! was presented to, however that endpoint is reached.
//!
//! The rows differ along what the rule generalises over: the wire the endpoint speaks (the
//! PostgreSQL one, where the database half of the decision is membership of `rds_iam`, and
//! the MySQL one, where it is the account's authentication plugin), whether the create asked
//! for a `Port` at all, and the claim a refused token differs in (its host, its port).
//!
//! A refused token is refused in the words of the authentication METHOD the database user logs
//! in by, which for a user enabled for IAM database authentication on the PostgreSQL wire is
//! PAM — Amazon RDS serves IAM authentication for PostgreSQL through it — rather than the
//! password one.
//!
//! What this does NOT assert is the FORM of the published address: whether a served engine
//! publishes AWS's per-instance DNS name on the engine's own port, rather than the address
//! the core bound its listener to, is a property of the core's listener surface and is
//! tracked on its own. This suite pins the identity between the published endpoint and the
//! token's scope, which holds whatever that address is.

use std::time::SystemTime;

use crate::db_instance_engine_endpoint::{
    available_instance, endpoint_of, mysql_login, postgres_login, MySqlSession, PostgresSession,
    Refusal, Transport, DATABASE, MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{call, emulator, expect_ok, text, REGION};
use crate::engine_role_grants::auth_token;

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The database user every token below is minted for.
const IAM_USER: &str = "iamuser";

/// How long a token states it is usable for, which is the lifetime the SDKs presign one
/// with.
const TOKEN_LIFETIME: u64 = 900;

/// The endpoint `DescribeDBInstances` publishes for `identifier`, with the value of
/// `IAMDatabaseAuthenticationEnabled` beside it — the two things AWS's documented recipe
/// reads before it mints a token.
async fn published_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
) -> (String, u16, String) {
    let described = expect_ok(
        identifier,
        call(
            router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => identifier],
        )
        .await,
    );
    let (address, port) = endpoint_of(&described);
    (
        address,
        port,
        text(&described, "IAMDatabaseAuthenticationEnabled"),
    )
}

/// Open a session as the master user on a PostgreSQL instance's published endpoint.
async fn postgres_master(case: &str, address: &str, port: u16) -> PostgresSession {
    postgres_login(
        address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| {
        panic!("case {case:?}: the master user must be admitted, got {refusal:?}")
    })
}

/// Run a statement that must succeed.
async fn run_postgres(case: &str, session: &mut PostgresSession, sql: &str) {
    if let Err(refusal) = session.query(sql).await {
        panic!("case {case:?}: {sql:?} must run, got {refusal:?}");
    }
}

/// The one value a PostgreSQL statement answered with.
async fn postgres_value(case: &str, session: &mut PostgresSession, sql: &str) -> Option<String> {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"))
        .only()
        .map(str::to_string)
}

/// The one value a MySQL statement answered with.
async fn mysql_value(case: &str, session: &mut MySqlSession, sql: &str) -> Option<String> {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"))
        .only()
        .map(str::to_string)
}

#[tokio::test]
async fn a_postgres_token_minted_from_the_published_endpoint_admits_the_granted_role() {
    let router = emulator();
    // The rows differ in what the create asked of the endpoint: nothing at all, so the
    // instance takes the engine family's own port, and an explicit `Port`.
    for (case, identifier, extra) in [
        (
            "the port the engine family defaults to",
            "emu-iam-scope-default",
            params![].to_vec(),
        ),
        (
            "a requested port",
            "emu-iam-scope-port",
            params!["Port" => 5433].to_vec(),
        ),
    ] {
        let mut request = params!["EnableIAMDatabaseAuthentication" => "true"].to_vec();
        request.extend(extra);
        let (address, port) =
            available_instance(&router, identifier, "postgres", &request).await;
        // AWS's documented sequence starts here: read the endpoint and the IAM-auth flag
        // out of a describe, and mint a token for exactly that pair.
        let (described_address, described_port, iam_enabled) =
            published_endpoint(&router, identifier).await;
        assert_eq!(
            (described_address.as_str(), described_port),
            (address.as_str(), port),
            "case {case:?}: every read must publish the one endpoint of the instance, got \
             {described_address:?}:{described_port}"
        );
        assert_eq!(
            iam_enabled, "true",
            "case {case:?}: the instance must report IAM database authentication on, got \
             {iam_enabled:?}"
        );

        let now = SystemTime::now();
        let token = auth_token(&address, port, IAM_USER, REGION, now, TOKEN_LIFETIME);
        // The token a client holds is PREFIXED by the endpoint it was minted for, which is
        // the pair the describe published.
        let prefix = format!("{address}:{port}/");
        assert!(
            token.starts_with(&prefix),
            "case {case:?}: the token must be scoped to the published endpoint {prefix:?}, it \
             begins {:?}",
            &token[..prefix.len().min(token.len())]
        );
        assert!(
            token.contains(&format!("DBUser={IAM_USER}")),
            "case {case:?}: the token must name the database user it was minted for, got \
             {token:?}"
        );

        // The engine half of the decision: the database user, and the membership that
        // enables it for token authentication.
        let mut master = postgres_master(case, &address, port).await;
        run_postgres(case, &mut master, &format!("CREATE USER {IAM_USER} WITH LOGIN")).await;
        run_postgres(case, &mut master, &format!("GRANT rds_iam TO {IAM_USER}")).await;

        // And the token authenticates on the endpoint it names — the same pair the describe
        // published, dialled as a client dials it.
        let mut session = postgres_login(
            &address,
            port,
            Transport::Tls,
            IAM_USER,
            &token,
            Some(DATABASE),
        )
        .await
        .unwrap_or_else(|refusal| {
            panic!(
                "case {case:?}: a token minted for the published endpoint must authenticate on \
                 it, got {refusal:?}"
            )
        });
        let current_user = postgres_value(case, &mut session, "SELECT current_user").await;
        assert_eq!(
            current_user.as_deref(),
            Some(IAM_USER),
            "case {case:?}: the session must hold the identity the token named, got \
             {current_user:?}"
        );
        let encrypted = postgres_value(
            case,
            &mut session,
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
        )
        .await;
        // PostgreSQL writes a boolean as `t` in the text format a simple query answers in.
        assert_eq!(
            encrypted.as_deref(),
            Some("t"),
            "case {case:?}: the token session must be encrypted, got {encrypted:?}"
        );

        // What the same endpoint does NOT admit: a token scoped to any other address. The
        // rows differ in the claim that moved — the host, and the port.
        for (refused, token) in [
            (
                "a token scoped to another host",
                auth_token(
                    "emu-other.cluster.us-west-2.rds.amazonaws.com",
                    port,
                    IAM_USER,
                    REGION,
                    now,
                    TOKEN_LIFETIME,
                ),
            ),
            (
                "a token scoped to another port of this host",
                auth_token(&address, port + 1, IAM_USER, REGION, now, TOKEN_LIFETIME),
            ),
            (
                "a token scoped to the host with no port at all",
                auth_token(&address, 0, IAM_USER, REGION, now, TOKEN_LIFETIME),
            ),
        ] {
            let refusal = postgres_login(
                &address,
                port,
                Transport::Tls,
                IAM_USER,
                &token,
                Some(DATABASE),
            )
            .await
            .err()
            .unwrap_or_else(|| {
                panic!("case {case:?}/{refused:?}: expected the login to be refused")
            });
            assert_eq!(
                refusal,
                Refusal {
                    code: "28000".to_string(),
                    message: format!("PAM authentication failed for user \"{IAM_USER}\""),
                },
                "case {case:?}/{refused:?}: expected the refusal of the authentication method \
                 this database user logs in by, got {refusal:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_mysql_token_minted_from_the_published_endpoint_admits_the_iam_account() {
    let router = emulator();
    let case = "the MySQL wire";
    let identifier = "emu-iam-scope-mysql";
    let (address, port) = available_instance(
        &router,
        identifier,
        "mysql",
        &params!["EnableIAMDatabaseAuthentication" => "true"],
    )
    .await;
    let (described_address, described_port, iam_enabled) =
        published_endpoint(&router, identifier).await;
    assert_eq!(
        (described_address.as_str(), described_port),
        (address.as_str(), port),
        "case {case:?}: every read must publish the one endpoint of the instance, got \
         {described_address:?}:{described_port}"
    );
    assert_eq!(
        iam_enabled, "true",
        "case {case:?}: the instance must report IAM database authentication on, got \
         {iam_enabled:?}"
    );

    let mut master = mysql_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| {
        panic!("case {case:?}: the master user must be admitted, got {refusal:?}")
    });
    // The engine half on this wire: the account's authentication plugin is the IAM one.
    for statement in [
        format!("CREATE USER '{IAM_USER}'@'%' IDENTIFIED WITH AWSAuthenticationPlugin AS 'RDS'"),
        format!("GRANT SELECT ON *.* TO '{IAM_USER}'@'%'"),
    ] {
        if let Err(refusal) = master.query(&statement).await {
            panic!("case {case:?}: {statement:?} must run, got {refusal:?}");
        }
    }

    let now = SystemTime::now();
    let token = auth_token(&address, port, IAM_USER, REGION, now, TOKEN_LIFETIME);
    let prefix = format!("{address}:{port}/");
    assert!(
        token.starts_with(&prefix),
        "case {case:?}: the token must be scoped to the published endpoint {prefix:?}, it begins \
         {:?}",
        &token[..prefix.len().min(token.len())]
    );
    let mut session = mysql_login(
        &address,
        port,
        Transport::Tls,
        IAM_USER,
        &token,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| {
        panic!(
            "case {case:?}: a token minted for the published endpoint must authenticate on it, \
             got {refusal:?}"
        )
    });
    let current_user = mysql_value(case, &mut session, "SELECT CURRENT_USER()").await;
    assert_eq!(
        current_user.as_deref(),
        Some(format!("{IAM_USER}@%").as_str()),
        "case {case:?}: the session must hold the identity the token named, got {current_user:?}"
    );

    // And the same endpoint admits no token scoped elsewhere, the host and the port alike.
    for (refused, token) in [
        (
            "a token scoped to another host",
            auth_token(
                "emu-other.cluster.us-west-2.rds.amazonaws.com",
                port,
                IAM_USER,
                REGION,
                now,
                TOKEN_LIFETIME,
            ),
        ),
        (
            "a token scoped to another port of this host",
            auth_token(&address, port + 1, IAM_USER, REGION, now, TOKEN_LIFETIME),
        ),
    ] {
        let refusal = mysql_login(
            &address,
            port,
            Transport::Tls,
            IAM_USER,
            &token,
            Some(DATABASE),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("case {refused:?}: expected the login to be refused"));
        assert_eq!(
            refusal.code, "1045",
            "case {refused:?}: expected MySQL's own access-denied refusal, got {refusal:?}"
        );
    }
}
