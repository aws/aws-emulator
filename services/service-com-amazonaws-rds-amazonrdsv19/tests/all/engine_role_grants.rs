// SPDX-License-Identifier: CC0-1.0

//! `GRANT` and `REVOKE` on a DB instance's PostgreSQL endpoint, and the IAM database
//! authentication a role grant turns on.
//!
//! The rules under test, each asserted over a table of statements rather than one recorded
//! one:
//!
//! * a PostgreSQL endpoint SERVES the `GRANT`/`REVOKE` forms its grammar defines — role
//!   membership (`GRANT <role> TO <role>`) and object privileges (`GRANT <privileges> ON
//!   <object> TO <grantee>`) — recording each in the catalog PostgreSQL keeps it in
//!   (`pg_auth_members`, and the object's own `relacl` / `nspacl` / `datacl`), so a client
//!   reads back exactly what it granted; `syntax error at or near "<token>"` is what a
//!   statement the grammar does NOT define is refused with, and a statement that names a
//!   role or an object the instance does not have takes that condition's own sentence
//!   instead;
//! * a DB instance with `EnableIAMDatabaseAuthentication` admits a login as a database user
//!   ENABLED for it — on PostgreSQL, a member of `rds_iam` — presenting an authentication
//!   token generated for that endpoint, that port and that user, and admits nothing else:
//!   not a user nothing enabled, not a token for another user, and not a token outside its
//!   window.
//!
//! The rows differ along what each rule generalises over: the statement's form, the
//! privileges and the kind of object they are granted on, the grantee (a role and `PUBLIC`),
//! and, for the token, every claim it carries.

use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use service_com_amazonaws_rds_amazonrdsv19::state as st;

use crate::db_instance_engine_endpoint::{
    available_instance, postgres_login, PostgresSession, Refusal, Transport, DATABASE,
    MASTER_PASSWORD, MASTER_USER,
};use crate::db_instances::{emulator, REGION};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// Open a session as the master user on a PostgreSQL instance's published endpoint.
async fn master_session(address: &str, port: u16) -> PostgresSession {
    postgres_login(
        address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted")
}

/// Run a statement that must succeed.
async fn run(case: &str, session: &mut PostgresSession, sql: &str) {
    if let Err(refusal) = session.query(sql).await {
        panic!("case {case:?}: {sql:?} must run, got {refusal:?}");
    }
}

/// The one value a statement answered with, or `None` for an answer with no rows.
async fn value(case: &str, session: &mut PostgresSession, sql: &str) -> Option<String> {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"))
        .only()
        .map(str::to_string)
}

#[tokio::test]
async fn a_postgresql_endpoint_serves_role_grants_and_records_the_membership() {
    let router = emulator();
    let (address, port) =
        available_instance(&router, "emu-grant-roles", "postgres", &params![]).await;
    let mut session = master_session(&address, port).await;
    run("the member role", &mut session, "CREATE USER appuser").await;
    run("a second member", &mut session, "CREATE USER reporting").await;
    run("a group role", &mut session, "CREATE ROLE analysts").await;

    // The membership a grant records is read back by joining the two catalogs PostgreSQL
    // keeps it in, exactly as a client reads it.
    let member_of = |role: &str, member: &str| {
        format!(
            "SELECT count(*) FROM pg_auth_members AS m \
               JOIN pg_roles AS r ON r.oid = m.roleid \
               JOIN pg_roles AS g ON g.oid = m.member \
             WHERE r.rolname = '{role}' AND g.rolname = '{member}'"
        )
    };

    // The rows span the grant's shape: one role to one member, a predefined RDS role, a
    // role to several members at once, and the `WITH ADMIN OPTION` clause.
    for (case, statement, role, member) in [
        (
            "one role to one member",
            "GRANT analysts TO appuser".to_string(),
            "analysts",
            "appuser",
        ),
        (
            "a predefined RDS role",
            "GRANT rds_iam TO appuser".to_string(),
            "rds_iam",
            "appuser",
        ),
        (
            "a role to several members",
            "GRANT analysts TO appuser, reporting".to_string(),
            "analysts",
            "reporting",
        ),
        (
            "a grant with ADMIN OPTION",
            "GRANT rds_password TO reporting WITH ADMIN OPTION".to_string(),
            "rds_password",
            "reporting",
        ),
        (
            "a quoted role name",
            "GRANT \"analysts\" TO \"appuser\"".to_string(),
            "analysts",
            "appuser",
        ),
    ] {
        run(case, &mut session, &statement).await;
        let held = value(case, &mut session, &member_of(role, member)).await;
        assert_eq!(
            held.as_deref(),
            Some("1"),
            "case {case:?}: {statement:?} must record {member} as a member of {role}, got \
             {held:?}"
        );
    }

    // And a revoke takes it off again, while leaving the memberships it does not name.
    run(
        "a revoke",
        &mut session,
        "REVOKE analysts FROM appuser",
    )
    .await;
    assert_eq!(
        value("a revoke", &mut session, &member_of("analysts", "appuser")).await,
        Some("0".to_string()),
        "case a revoke: the membership it named must be gone"
    );
    assert_eq!(
        value("a revoke", &mut session, &member_of("rds_iam", "appuser")).await,
        Some("1".to_string()),
        "case a revoke: a membership it did not name must stay"
    );
    // A revoke of a membership nothing granted is a success that changes nothing, as
    // PostgreSQL answers it.
    run(
        "a revoke of nothing",
        &mut session,
        "REVOKE analysts FROM appuser",
    )
    .await;

    // A role the instance does not have is the UNDEFINED-OBJECT condition, not a parse
    // failure: the rows span which side of the grant names it.
    for (case, statement, named) in [
        ("an unknown role granted", "GRANT nosuchrole TO appuser", "nosuchrole"),
        ("an unknown member", "GRANT analysts TO nosuchuser", "nosuchuser"),
        (
            "an unknown role revoked",
            "REVOKE nosuchrole FROM appuser",
            "nosuchrole",
        ),
    ] {
        let refusal = session
            .query(statement)
            .await
            .expect_err(&format!("case {case:?}: {statement:?} must be refused"));
        assert_eq!(
            refusal,
            Refusal {
                code: "42704".to_string(),
                message: format!("role \"{named}\" does not exist"),
            },
            "case {case:?}: expected PostgreSQL's own sentence for a role that is not \
             there, got {refusal:?}"
        );
    }
}

/// What this endpoint's OWN limit on such a statement is: the engine work a grant drives is
/// the product of its two lists, so a statement naming more names than the endpoint serves
/// is refused before any of that work is done — and a name a statement repeats is one name.
#[tokio::test]
async fn a_grant_naming_more_names_than_the_endpoint_serves_is_refused() {
    let router = emulator();
    let (address, port) =
        available_instance(&router, "emu-grant-bounds", "postgres", &params![]).await;
    let mut session = master_session(&address, port).await;
    run("the member role", &mut session, "CREATE USER appuser").await;

    // A repeated name is one name, so a statement naming one role many times is served and
    // records the one membership it states.
    let repeated = std::iter::repeat_n("rds_iam", 200).collect::<Vec<_>>().join(", ");
    run(
        "a repeated role name",
        &mut session,
        &format!("GRANT {repeated} TO appuser"),
    )
    .await;
    assert_eq!(
        value(
            "a repeated role name",
            &mut session,
            // The memberships of the role the grant named: the instance's own membership
            // (the master user in `rds_superuser`) is not one this statement made.
            "SELECT count(*) FROM pg_auth_members AS m \
               JOIN pg_roles AS g ON g.oid = m.member \
             WHERE g.rolname = 'appuser'",
        )
        .await,
        Some("1".to_string()),
        "case a repeated role name: a name a statement repeats is one name"
    );

    // Past the cap on DISTINCT names, the statement is refused with this endpoint's own
    // limit rather than served: the roles below do not exist either, and the limit is
    // answered first, before anything is looked up or written.
    let distinct = (0..st::data_plane::pg_grants::MAX_IDENTIFIERS_PER_SIDE + 1)
        .map(|index| format!("emu_role_{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let refusal = session
        .query(&format!("GRANT {distinct} TO appuser"))
        .await
        .expect_err("a statement past the endpoint's limit must be refused");
    assert_eq!(
        refusal.message,
        format!(
            "this endpoint serves a GRANT or REVOKE naming at most {} names on either side",
            st::data_plane::pg_grants::MAX_IDENTIFIERS_PER_SIDE
        ),
        "case more names than the endpoint serves: expected its own limit, got {refusal:?}"
    );
    assert_eq!(
        value(
            "more names than the endpoint serves",
            &mut session,
            // As above: the one membership the statements before it recorded, beside the
            // instance's own.
            "SELECT count(*) FROM pg_auth_members AS m \
               JOIN pg_roles AS g ON g.oid = m.member \
             WHERE g.rolname = 'appuser'",
        )
        .await,
        Some("1".to_string()),
        "case more names than the endpoint serves: a refused statement writes nothing"
    );
}

/// A database holding no relation at all is still a database whose catalogs a client reads:
/// the relation catalog is empty rather than an engine error.
#[tokio::test]
async fn the_relation_catalog_of_a_database_with_no_tables_is_empty() {
    let router = emulator();
    let (address, port) =
        available_instance(&router, "emu-grant-empty", "postgres", &params![]).await;
    let mut session = master_session(&address, port).await;
    run("a table to drop", &mut session, "CREATE TABLE widgets (id integer)").await;
    run("the drop", &mut session, "DROP TABLE widgets").await;
    assert_eq!(
        value(
            "a database with no relation of its own",
            &mut session,
            "SELECT count(*) FROM pg_class WHERE relname = 'widgets'",
        )
        .await,
        Some("0".to_string()),
        "case a dropped table: its catalog row goes with it"
    );
    let refusal = session
        .query("GRANT SELECT ON widgets TO PUBLIC")
        .await
        .expect_err("a relation that is not there must be refused");
    assert_eq!(
        refusal,
        Refusal {
            code: "42P01".to_string(),
            message: "relation \"widgets\" does not exist".to_string(),
        },
        "case a dropped table: expected PostgreSQL's own sentence, got {refusal:?}"
    );
}

#[tokio::test]
async fn a_postgresql_endpoint_serves_object_privilege_grants() {
    let router = emulator();
    let (address, port) =
        available_instance(&router, "emu-grant-objects", "postgres", &params![]).await;
    let mut session = master_session(&address, port).await;
    run("the table", &mut session, "CREATE TABLE widgets (id integer)").await;
    run("a second table", &mut session, "CREATE TABLE orders (id integer)").await;
    run("the grantee", &mut session, "CREATE USER appuser").await;

    // The access control list of one object, as the client reads it back.
    let relation_acl =
        |name: &str| format!("SELECT relacl FROM pg_class WHERE relname = '{name}'");

    // The rows span the privileges granted and the grantee: one privilege, several at
    // once, `ALL PRIVILEGES`, and the `PUBLIC` pseudo-role, whose entry carries no name.
    // The letters are PostgreSQL's own abbreviations (the "Privileges" table of its
    // documentation): a=INSERT, r=SELECT, w=UPDATE, d=DELETE, D=TRUNCATE, x=REFERENCES,
    // t=TRIGGER.
    for (case, statement, expected) in [
        (
            "one privilege",
            "GRANT SELECT ON widgets TO appuser",
            format!("{{appuser=r/{MASTER_USER}}}"),
        ),
        (
            "a second privilege for the same grantee",
            "GRANT INSERT, UPDATE ON widgets TO appuser",
            format!("{{appuser=arw/{MASTER_USER}}}"),
        ),
        (
            "the PUBLIC pseudo-role",
            "GRANT SELECT ON widgets TO PUBLIC",
            format!("{{appuser=arw/{MASTER_USER},=r/{MASTER_USER}}}"),
        ),
        (
            "a privilege already held",
            "GRANT SELECT ON widgets TO appuser",
            format!("{{appuser=arw/{MASTER_USER},=r/{MASTER_USER}}}"),
        ),
        (
            "a qualified table name",
            "GRANT DELETE ON public.widgets TO appuser",
            format!("{{appuser=arwd/{MASTER_USER},=r/{MASTER_USER}}}"),
        ),
    ] {
        run(case, &mut session, statement).await;
        let held = value(case, &mut session, &relation_acl("widgets")).await;
        assert_eq!(
            held.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: {statement:?} must leave the list {expected:?}, got {held:?}"
        );
    }

    // `ALL PRIVILEGES` is every privilege a table admits, and `ALL TABLES IN SCHEMA`
    // reaches every table the schema holds.
    run(
        "ALL TABLES IN SCHEMA",
        &mut session,
        "GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO appuser",
    )
    .await;
    for table in ["widgets", "orders"] {
        let held = value("ALL TABLES IN SCHEMA", &mut session, &relation_acl(table)).await;
        assert!(
            held.as_deref()
                .is_some_and(|acl| acl.contains(&format!("appuser=arwdDxt/{MASTER_USER}"))),
            "case ALL TABLES IN SCHEMA: {table} must carry every table privilege, got {held:?}"
        );
    }

    // A revoke takes exactly what it names off, and the entry goes when it holds nothing.
    run(
        "a revoke",
        &mut session,
        "REVOKE ALL PRIVILEGES ON orders FROM appuser",
    )
    .await;
    assert_eq!(
        value("a revoke", &mut session, &relation_acl("orders")).await,
        None,
        "case a revoke: an object nothing is granted on carries no list at all"
    );

    // `ALL <class>S IN SCHEMA` over a class this server holds no object of covers an EMPTY
    // set in a schema that exists: the statement is served and grants nothing, which is
    // what a grant over no object does — and it touches no table's list.
    let before = value("an empty class", &mut session, &relation_acl("widgets")).await;
    run(
        "an empty class",
        &mut session,
        "GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO appuser",
    )
    .await;
    assert_eq!(
        value("an empty class", &mut session, &relation_acl("widgets")).await,
        before,
        "case an empty class: a grant over no object must leave every list as it was"
    );

    // The other object classes this server holds a catalog of, each with the privileges
    // its own class admits.
    run(
        "a schema",
        &mut session,
        "GRANT USAGE, CREATE ON SCHEMA public TO appuser",
    )
    .await;
    assert_eq!(
        value(
            "a schema",
            &mut session,
            "SELECT nspacl FROM pg_namespace WHERE nspname = 'public'",
        )
        .await,
        Some(format!("{{appuser=UC/{MASTER_USER}}}")),
        "case a schema: the privileges granted on it must be on its own catalog row"
    );
    run(
        "a database",
        &mut session,
        &format!("GRANT CONNECT, TEMPORARY ON DATABASE {DATABASE} TO appuser"),
    )
    .await;
    assert_eq!(
        value(
            "a database",
            &mut session,
            &format!("SELECT datacl FROM pg_database WHERE datname = '{DATABASE}'"),
        )
        .await,
        Some(format!("{{appuser=Tc/{MASTER_USER}}}")),
        "case a database: the privileges granted on it must be on its own catalog row"
    );

    // What such a statement is refused for: an object the instance does not have, a
    // grantee that is not a role, and a privilege the object class does not admit.
    for (case, statement, expected) in [
        (
            "a table that does not exist",
            "GRANT SELECT ON nosuchtable TO appuser".to_string(),
            Refusal {
                code: "42P01".to_string(),
                message: "relation \"nosuchtable\" does not exist".to_string(),
            },
        ),
        (
            "a schema that does not exist",
            "GRANT USAGE ON SCHEMA nosuchschema TO appuser".to_string(),
            Refusal {
                code: "3F000".to_string(),
                message: "schema \"nosuchschema\" does not exist".to_string(),
            },
        ),
        (
            "a database that does not exist",
            "GRANT CONNECT ON DATABASE nosuchdb TO appuser".to_string(),
            Refusal {
                code: "3D000".to_string(),
                message: "database \"nosuchdb\" does not exist".to_string(),
            },
        ),
        (
            "a grantee that is not a role",
            "GRANT SELECT ON widgets TO nosuchuser".to_string(),
            Refusal {
                code: "42704".to_string(),
                message: "role \"nosuchuser\" does not exist".to_string(),
            },
        ),
        (
            "a privilege the object class does not admit",
            "GRANT USAGE ON widgets TO appuser".to_string(),
            Refusal {
                code: "0LP01".to_string(),
                message: "invalid privilege type USAGE for table".to_string(),
            },
        ),
        (
            // A privilege PostgreSQL's grammar defines for a class this server holds no
            // object of: it is READ as that privilege, so what refuses the statement is the
            // class it was asked for, never a parse error.
            "a privilege defined for another object class",
            "GRANT EXECUTE ON widgets TO appuser".to_string(),
            Refusal {
                code: "0LP01".to_string(),
                message: "invalid privilege type EXECUTE for table".to_string(),
            },
        ),
        (
            // An object class this server holds NONE of: the refusal is that class's own
            // sentence naming the OBJECT, not the class keyword read as a table name.
            "a sequence, of which this server holds none",
            "GRANT SELECT ON SEQUENCE orders_id_seq TO appuser".to_string(),
            Refusal {
                code: "42P01".to_string(),
                message: "relation \"orders_id_seq\" does not exist".to_string(),
            },
        ),
        (
            "a function, of which this server holds none",
            "GRANT EXECUTE ON FUNCTION do_things TO appuser".to_string(),
            Refusal {
                code: "42883".to_string(),
                message: "function do_things does not exist".to_string(),
            },
        ),
        (
            // `ALL <class>S IN SCHEMA` in a schema the database does not have: the schema
            // is what does not exist, and the statement grants nothing.
            "every table of a schema that does not exist",
            "GRANT SELECT ON ALL TABLES IN SCHEMA nosuchschema TO appuser".to_string(),
            Refusal {
                code: "3F000".to_string(),
                message: "schema \"nosuchschema\" does not exist".to_string(),
            },
        ),
        (
            "every sequence of a schema that does not exist",
            "GRANT SELECT ON ALL SEQUENCES IN SCHEMA nosuchschema TO appuser".to_string(),
            Refusal {
                code: "3F000".to_string(),
                message: "schema \"nosuchschema\" does not exist".to_string(),
            },
        ),
        (
            "a privilege no object admits",
            "GRANT TELEPORT ON widgets TO appuser".to_string(),
            Refusal {
                code: "42601".to_string(),
                message: "syntax error at or near \"TELEPORT\"".to_string(),
            },
        ),
    ] {
        let refusal = session
            .query(&statement)
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: {statement:?} must be refused"));
        assert_eq!(
            refusal, expected,
            "case {case:?}: expected {expected:?}, got {refusal:?}"
        );
    }
}

/// The IAM authentication token a client generates for one endpoint, one port and one
/// database user — the shape `generate_db_auth_token` produces.
///
/// The SIGNATURE is the client's: this suite holds no signing credential, so the value is
/// of the shape a SigV4 signature has (the hex of a SHA-256) over the request the token
/// states, which is what the endpoint reads it as.
pub(crate) fn auth_token(host: &str, port: u16, user: &str, region: &str, signed_at: SystemTime, expires: u64) -> String {
    let seconds = signed_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the signing instant is after the epoch")
        .as_secs();
    let stamp = chrono::DateTime::from_timestamp(
        i64::try_from(seconds).expect("the signing instant fits"),
        0,
    )
    .expect("the signing instant is a calendar instant")
    .format("%Y%m%dT%H%M%SZ")
    .to_string();
    let date = &stamp[..8];
    let request = format!("{host}:{port}/{user}/{region}/{stamp}");
    let signature = hex::encode(Sha256::digest(request.as_bytes()));
    format!(
        "{host}:{port}/?Action=connect&DBUser={user}&X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Credential=AKIAEMULATOREXAMPLE%2F{date}%2F{region}%2Frds-db%2Faws4_request\
         &X-Amz-Date={stamp}&X-Amz-Expires={expires}&X-Amz-SignedHeaders=host\
         &X-Amz-Signature={signature}"
    )
}

#[tokio::test]
async fn an_iam_enabled_user_authenticates_with_an_authentication_token() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-iam-auth",
        "postgres",
        &params!["EnableIAMDatabaseAuthentication" => "true"],
    )
    .await;
    let mut session = master_session(&address, port).await;
    // The engine-side half: the database user, and the membership that enables it for
    // token authentication.
    run("the IAM user", &mut session, "CREATE USER iamuser").await;
    run("the grant", &mut session, "GRANT rds_iam TO iamuser").await;
    // A user created the same way but NOT granted the role, to show the grant is what
    // admits the login.
    run("an unenabled user", &mut session, "CREATE USER plainuser").await;

    let now = SystemTime::now();
    let token = auth_token(&address, port, "iamuser", REGION, now, 900);
    let mut iam_session = postgres_login(
        &address,
        port,
        Transport::Tls,
        "iamuser",
        &token,
        Some(DATABASE),
    )
    .await
    .expect("a database user enabled for IAM authentication must be admitted by its token");
    // The session is a session like any other: it runs statements.
    assert_eq!(
        value("the IAM session", &mut iam_session, "SELECT 1").await,
        Some("1".to_string()),
        "case the IAM session: the admitted session must serve statements"
    );

    // What such a login is NOT admitted for. The rows span the two halves of the decision
    // and every claim the token carries.
    for (case, user, token) in [
        (
            "a user nothing enabled for it",
            "plainuser",
            auth_token(&address, port, "plainuser", REGION, now, 900),
        ),
        (
            "a token generated for another user",
            "iamuser",
            auth_token(&address, port, "someone", REGION, now, 900),
        ),
        (
            "a token generated for another port",
            "iamuser",
            auth_token(&address, port + 1, "iamuser", REGION, now, 900),
        ),
        (
            "a token signed in another Region",
            "iamuser",
            auth_token(&address, port, "iamuser", "eu-west-1", now, 900),
        ),
        (
            "a token whose window has passed",
            "iamuser",
            auth_token(
                &address,
                port,
                "iamuser",
                REGION,
                now - Duration::from_secs(3_600),
                900,
            ),
        ),
        (
            "a password that is no token at all",
            "iamuser",
            "not-a-token".to_string(),
        ),
    ] {
        let refusal = postgres_login(
            &address,
            port,
            Transport::Tls,
            user,
            &token,
            Some(DATABASE),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("case {case:?}: expected the login to be refused"));
        // A refusal is worded for the authentication METHOD the database user logs in by: PAM
        // for one enabled for IAM database authentication, which is what Amazon RDS serves it
        // through on this wire, and the password one for a user that logs in with a password.
        let expected = match user {
            "iamuser" => Refusal {
                code: "28000".to_string(),
                message: format!("PAM authentication failed for user \"{user}\""),
            },
            _ => Refusal {
                code: "28P01".to_string(),
                message: format!("password authentication failed for user \"{user}\""),
            },
        };
        assert_eq!(
            refusal, expected,
            "case {case:?}: expected {expected:?}, got {refusal:?}"
        );
    }
}

#[tokio::test]
async fn a_token_admits_no_login_on_an_instance_that_does_not_serve_iam_authentication() {
    let router = emulator();
    // The same engine-side grant, on an instance whose control plane leaves IAM database
    // authentication OFF: both halves have to hold, so the token admits nobody.
    let (address, port) =
        available_instance(&router, "emu-iam-off", "postgres", &params![]).await;
    let mut session = master_session(&address, port).await;
    run("the IAM user", &mut session, "CREATE USER iamuser").await;
    run("the grant", &mut session, "GRANT rds_iam TO iamuser").await;
    let token = auth_token(&address, port, "iamuser", REGION, SystemTime::now(), 900);
    let refusal = postgres_login(
        &address,
        port,
        Transport::Tls,
        "iamuser",
        &token,
        Some(DATABASE),
    )
    .await
    .err()
    .expect("an instance that does not serve IAM authentication must refuse the login");
    assert_eq!(
        refusal.code, "28P01",
        "expected PostgreSQL's authentication refusal, got {refusal:?}"
    );
}

/// A login this endpoint REFUSES leaves nothing behind for a later session to find: the
/// decision reads the resource's own facts and, at most, one database the resource has — so
/// a client naming a database the resource does not have creates none, and the databases a
/// later authenticated session sees are the ones the resource came with.
#[tokio::test]
async fn a_refused_login_creates_no_database_and_leaves_no_catalog() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-iam-no-trace",
        "postgres",
        &params!["EnableIAMDatabaseAuthentication" => "true"],
    )
    .await;
    // The rows span what a refused login can name: a database the resource does not have,
    // under a user it does not have and under one it does, and a password that is no token.
    for (case, user, database) in [
        ("a database the resource does not have", "iamuser", "nosuchdb"),
        ("another such name, with the master user", MASTER_USER, "alsonosuchdb"),
        ("a third, with no password at all", "iamuser", "stillnosuchdb"),
    ] {
        let refusal = postgres_login(
            &address,
            port,
            Transport::Tls,
            user,
            "not-a-token",
            Some(database),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("case {case:?}: expected the login to be refused"));
        assert_eq!(
            refusal.code, "28P01",
            "case {case:?}: expected the authentication refusal, got {refusal:?}"
        );
    }
    // What a session that IS admitted sees afterwards: exactly the databases the instance
    // came with, and a create of one of the refused names still accepted — so nothing the
    // refused logins named was opened behind them.
    let mut session = master_session(&address, port).await;
    let listed = value(
        "after the refused logins",
        &mut session,
        "SELECT count(*) FROM pg_database WHERE datname IN \
         ('nosuchdb', 'alsonosuchdb', 'stillnosuchdb')",
    )
    .await;
    assert_eq!(
        listed.as_deref(),
        Some("0"),
        "case after the refused logins: expected none of the named databases to exist, got \
         {listed:?}"
    );
    run(
        "a create of a name a refused login had named",
        &mut session,
        "CREATE DATABASE nosuchdb",
    )
    .await;
}
