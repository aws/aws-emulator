// SPDX-License-Identifier: CC0-1.0

//! IAM authentication of a CLIENT to a DB PROXY: the credential the proxy asks for, the
//! grant that admits it, and the refusal a client holding no such grant is answered with.
//!
//! THE RULE UNDER TEST: a DB proxy whose `Auth` requires IAM authentication for a database
//! user authenticates the CLIENT itself — it admits a connection as that user only when the
//! client presents an authentication token generated for the proxy's own published endpoint,
//! that user and the proxy's Region, AND the IAM identity that signed the token is allowed
//! `rds-db:connect` on that user's `dbuser` resource of that proxy; anything else is refused
//! on the proxy's own wire with the engine's access-denied refusal, and no statement runs. A
//! proxy whose `Auth` leaves IAM authentication disabled for a user decides nothing about its
//! clients' credentials and goes on admitting the database's own password.
//!
//! The grant is read where IAM holds it and the principal where the token service holds it,
//! so both are mounted on the SAME engine as this service, exactly as the assembled router
//! mounts them: the documents under test are the ones `iam:PutRolePolicy` stored, and the
//! identity a token speaks for is the one `sts:AssumeRole` minted.
//!
//! The rows differ along what the rule generalises over: the DOCUMENTS in force on the
//! principal (a grant on exactly that `dbuser` resource, a grant written with wildcards, a
//! grant on another database user's resource, a grant of another action, an explicit deny
//! beside a wildcard allow, no document at all), the KIND of credential the token was signed
//! with (a role session and an IAM user's long-term access key), the CLAIMS the token carries
//! (another endpoint, another user, a window that has passed), the credential presented
//! instead of a token (the database's own password), and the proxy's own `IAMAuth` mode.

use std::time::{Duration, SystemTime};

use hex;
use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_PROXY_STATUS_AVAILABLE, TARGET_STATE_AVAILABLE,
};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use crate::db_instance_engine_endpoint::{
    available_instance, mysql_login, MySqlSession, Refusal, Transport, DATABASE, MASTER_PASSWORD,
    MASTER_USER,
};
use crate::db_instances::{call, expect_ok, text, texts, REGION};
use crate::lifecycle_poll_loops::poll_until;

/// The host the core binds its listeners on, which is therefore the host every endpoint this
/// emulator publishes answers at.
const LOOPBACK: &str = "127.0.0.1";

/// The DB instance the proxies below pool connections to.
const INSTANCE: &str = "emu-proxy-iam-target";

/// The proxy that requires IAM authentication of its clients, and the one that disallows it.
const IAM_PROXY: &str = "emu-proxy-iam-required";
const PASSWORD_PROXY: &str = "emu-proxy-iam-disabled";

/// The two proxies whose `Auth` entry spells NO database user out: one whose secret holds the
/// database's own user, and one whose secret states no user at all.
const SECRET_USER_PROXY: &str = "emu-proxy-iam-from-secret";
const USERLESS_SECRET_PROXY: &str = "emu-proxy-iam-userless-secret";

/// An IAM role ARN of the form `CreateDBProxy$RoleArn` takes: the role a proxy reads its
/// `Auth` secret with.
const PROXY_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy";

/// A secret ARN of the form `UserAuthConfig$SecretArn` takes: the secret a proxy "uses to
/// authenticate to the RDS DB instance or Aurora DB cluster".
const SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:emu-proxy-credential-AbCdEf";

/// A subnet id of the published form (`subnet-` plus eight hexadecimal characters).
const SUBNET_ID: &str = "subnet-0a1b2c3d";

/// The API versions a client sends in each service's query body.
const IAM_API_VERSION: &str = "2010-05-08";
const STS_API_VERSION: &str = "2011-06-15";

/// The access key id of the account's own published credential, which provisions the roles and
/// users the rows are about.
const ACCOUNT_KEY_ID: &str = "AKIDEMULATOR";

/// How long a token below is generated for: "the token has a lifetime of 15 minutes" (Amazon
/// RDS User Guide, IAM database authentication), the lifetime `generate_db_auth_token` states.
const TOKEN_LIFETIME: u64 = 900;

/// A port of this machine that is neither a port the core assigns a listener (its range is
/// the ephemeral one) nor the port a MySQL-family proxy is dialled on: the port a token
/// generated for ANOTHER endpoint names.
const ANOTHER_PORT: u16 = 15_432;

/// The port a client dials a MySQL-family proxy on, which is the port of the databases that
/// family names: "Default: … RDS for MariaDB — 3306, … RDS for MySQL — 3306"
/// (`smithy_query(query="doc", shape="CreateDBInstanceMessage", member="Port")`). A client
/// completes the host `DBProxy$Endpoint` publishes with it, because that member carries no
/// port of its own.
const MYSQL_FAMILY_PORT: u16 = 3306;

/// This emulator's account id, which every ARN below carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The emulator the rows drive: this service beside the IAM and token services on ONE shared
/// engine, exactly as the assembled router mounts them, so the grant a row stores and the
/// session it mints are the ones the proxy's endpoint reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these calls sign
    // nothing, so the scope header is read for the key id and the Region alone.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// [`emulator`] with SECRETS MANAGER beside it, for the rows about an `Auth` entry whose
/// database user comes from the secret it names: what the entry is in force for is then a record
/// that service holds, written through its own operation and read through the in-process
/// callback a session on the proxy's endpoint holds.
fn emulator_with_secrets() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// The secret `name`, holding `document`, written through Secrets Manager's OWN operation:
/// answers the ARN it minted, which is the value an `Auth` entry's `SecretArn` carries.
async fn planted_secret(
    secrets: &aws_sdk_secretsmanager::Client,
    name: &str,
    document: &str,
) -> String {
    let created = secrets
        .create_secret()
        .name(name)
        .secret_string(document)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup {name:?}: the secret must be created: {error:?}"));
    created
        .arn()
        .unwrap_or_else(|| panic!("setup {name:?}: CreateSecret must report the ARN it minted"))
        .to_string()
}

/// One form value as a client puts it in an `application/x-www-form-urlencoded` body, so a
/// policy document travels as an SDK would send it.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// One query-protocol call to `service`, presented as signed by the access key id `akid`.
async fn call_as(
    router: &MultiServiceRouter,
    service: &str,
    api_version: &str,
    akid: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={api_version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = if service == "iam" {
        "iam.amazonaws.com".to_string()
    } else {
        format!("{service}.{REGION}.amazonaws.com")
    };
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{REGION}/{service}/aws4_request, \
                 SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM call, made by the account's own published credential.
async fn iam_call(router: &MultiServiceRouter, case: &str, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = call_as(
        router,
        "iam",
        IAM_API_VERSION,
        ACCOUNT_KEY_ID,
        action,
        params,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {case:?}: expected {action} to be served, got {status} with {body}"
    );
    body
}

/// A trust policy admitting the account's own principals to assume a role, which is what lets
/// the published credential mint a session of it.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
        account()
    )
}

/// An identity-based document stating `statements`, each `(effect, action, resource)`.
fn document(statements: &[(&str, &str, &str)]) -> String {
    let written: Vec<String> = statements
        .iter()
        .map(|(effect, action, resource)| {
            format!(
                r#"{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}"#
            )
        })
        .collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        written.join(",")
    )
}

/// Create the role `role_name` carrying `inline` as its one inline policy — or carrying none,
/// for `None` — and assume it: answers the session's access key id, which is the credential a
/// token below is signed with.
async fn role_session(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    inline: Option<String>,
) -> String {
    let trust = trust_policy();
    iam_call(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    if let Some(policy) = &inline {
        iam_call(
            router,
            case,
            "PutRolePolicy",
            &[
                ("RoleName", role_name),
                ("PolicyName", "connect"),
                ("PolicyDocument", policy),
            ],
        )
        .await;
    }
    let role_arn = format!("arn:aws:iam::{}:role/{role_name}", account());
    let session_name = format!("{role_name}-session");
    let (status, assumed) = call_as(
        router,
        "sts",
        STS_API_VERSION,
        ACCOUNT_KEY_ID,
        "AssumeRole",
        &[("RoleArn", &role_arn), ("RoleSessionName", &session_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {case:?}: the role must be assumable, got {status} with {assumed}"
    );
    let akid = text(&assumed, "AccessKeyId");
    assert!(
        !akid.is_empty(),
        "setup {case:?}: the session must report its access key, got {assumed}"
    );
    akid
}

/// Create the IAM user `user_name` carrying `inline` as its one inline policy and mint its
/// LONG-TERM access key: the other kind of credential a token can be signed with.
async fn user_key(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    inline: &str,
) -> String {
    iam_call(router, case, "CreateUser", &[("UserName", user_name)]).await;
    iam_call(
        router,
        case,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", "connect"),
            ("PolicyDocument", inline),
        ],
    )
    .await;
    let minted = iam_call(router, case, "CreateAccessKey", &[("UserName", user_name)]).await;
    let akid = text(&minted, "AccessKeyId");
    assert!(
        !akid.is_empty(),
        "setup {case:?}: the access key must be minted, got {minted}"
    );
    akid
}

/// `CreateDBProxy` with one `Auth` entry for the database's master user in the `IAMAuth` mode
/// `iam_auth`, answered with the `Endpoint` it published and its `DBProxyArn`, once the proxy
/// reports itself ready to handle requests.
async fn available_proxy(
    router: &MultiServiceRouter,
    name: &str,
    iam_auth: &str,
    require_tls: bool,
) -> (String, String) {
    available_proxy_with(router, name, iam_auth, require_tls, SECRET_ARN, true).await
}

/// [`available_proxy`], with the secret the `Auth` entry points at and whether that entry
/// SPELLS THE DATABASE USER OUT.
///
/// `UserName` is an optional member, and the create AWS documents states the scheme, the secret
/// and the IAM mode and leaves the user to the credential in that secret — so a proxy is created
/// both ways here.
async fn available_proxy_with(
    router: &MultiServiceRouter,
    name: &str,
    iam_auth: &str,
    require_tls: bool,
    secret_arn: &str,
    names_the_user: bool,
) -> (String, String) {
    let mut params: Vec<(&str, &str)> = vec![
        ("DBProxyName", name),
        ("EngineFamily", "MYSQL"),
        ("RoleArn", PROXY_ROLE_ARN),
        ("VpcSubnetIds.member.1", SUBNET_ID),
        ("RequireTLS", if require_tls { "true" } else { "false" }),
        ("Auth.member.1.AuthScheme", "SECRETS"),
        ("Auth.member.1.SecretArn", secret_arn),
        ("Auth.member.1.IAMAuth", iam_auth),
    ];
    if names_the_user {
        params.push(("Auth.member.1.UserName", MASTER_USER));
    }
    let created = expect_ok(name, call(router, "CreateDBProxy", &params).await);
    let published = text(&created, "Endpoint");
    let arn = text(&created, "DBProxyArn");
    // The proxy comes up on its own, so what this waits for is the CONDITION a client has to
    // see before it connects, read again until it holds.
    let described_params = [("DBProxyName", name.to_string())];
    poll_until(
        name,
        "the proxy",
        || call(router, "DescribeDBProxies", &described_params),
        |status, body| status == 200 && text(body, "Status") == DB_PROXY_STATUS_AVAILABLE,
    )
    .await;
    assert!(
        !published.is_empty() && !arn.is_empty(),
        "setup {name:?}: the create must publish an endpoint and an ARN, got {created}"
    );
    (published, arn)
}

/// Register the instance in `proxy`'s one target group and wait for the registration to be in
/// force, which is when the proxy pools connections to it.
async fn registered_target(router: &MultiServiceRouter, proxy: &str) {
    expect_ok(
        proxy,
        call(
            router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", proxy),
                ("DBInstanceIdentifiers.member.1", INSTANCE),
            ],
        )
        .await,
    );
    let target_params = [("DBProxyName", proxy.to_string())];
    poll_until(
        proxy,
        "the proxy's target",
        || call(router, "DescribeDBProxyTargets", &target_params),
        |status, body| {
            status == 200
                && texts(body, "State")
                    .iter()
                    .any(|state| state == TARGET_STATE_AVAILABLE)
        },
    )
    .await;
}

/// The `host:port` a published proxy endpoint spells, as a client reads it.
fn dialled(case: &str, endpoint: &str) -> (String, u16) {
    let (host, port) = endpoint.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected a published endpoint carrying a port, got {endpoint:?}")
    });
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a port, got {endpoint:?}"));
    (host.to_string(), port)
}

/// The `dbuser` resource an IAM policy grants `rds-db:connect` over for a login as `user`
/// through the proxy `proxy_arn` names, derived the way a CLIENT derives it: the proxy's id is
/// the segment its own `DBProxyArn` ends with, and the resource lives in the `rds-db`
/// namespace.
fn dbuser_arn(proxy_arn: &str, user: &str) -> String {
    let id = proxy_arn.rsplit(':').next().unwrap_or_default();
    format!(
        "arn:aws:rds-db:{REGION}:{}:dbuser:{id}/{user}",
        account()
    )
}

/// The IAM authentication token a client generates for one endpoint, one port and one database
/// user — the shape `generate_db_auth_token` produces, signed with `akid`.
///
/// This suite holds no signing credential, so the SIGNATURE is of the shape a SigV4 signature
/// has (the hex of a SHA-256) over the request the token states, which is what the endpoint
/// reads it as; what the endpoint resolves the PRINCIPAL from is the access key id in the
/// credential scope, which is why this builder takes one where
/// [`crate::engine_role_grants::auth_token`] fixes it to the account's published credential.
fn auth_token(
    akid: &str,
    host: &str,
    port: u16,
    user: &str,
    region: &str,
    signed_at: SystemTime,
) -> String {
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
    let signature = hex::encode(Sha256::digest(
        format!("{akid}/{host}:{port}/{user}/{region}/{stamp}").as_bytes(),
    ));
    format!(
        "{host}:{port}/?Action=connect&DBUser={user}&X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Credential={akid}%2F{date}%2F{region}%2Frds-db%2Faws4_request\
         &X-Amz-Date={stamp}&X-Amz-Expires={TOKEN_LIFETIME}&X-Amz-SignedHeaders=host\
         &X-Amz-Signature={signature}"
    )
}

/// MySQL's own access-denied refusal of a login on the proxy endpoint `host`, which is what a
/// client holding no grant — or presenting no token at all — is answered with.
fn access_denied(host: &str) -> Refusal {
    Refusal {
        code: "1045".to_string(),
        message: format!("Access denied for user '{MASTER_USER}'@'{host}' (using password: YES)"),
    }
}

/// Open a session on the proxy endpoint `host:port` as the database's master user, presenting
/// `credential` as its password.
async fn login(
    host: &str,
    port: u16,
    credential: &str,
) -> Result<MySqlSession, Refusal> {
    mysql_login(
        host,
        port,
        Transport::Tls,
        MASTER_USER,
        credential,
        Some(DATABASE),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_requiring_iam_auth_admits_the_client_its_dbuser_grant_covers() {
    let router = emulator();
    available_instance(&router, INSTANCE, "mysql", &[]).await;
    let (published, proxy_arn) = available_proxy(&router, IAM_PROXY, "REQUIRED", true).await;
    registered_target(&router, IAM_PROXY).await;
    let (host, port) = dialled("the IAM-auth proxy", &published);
    assert_eq!(
        host, LOOPBACK,
        "case \"the published endpoint\": expected this machine's loopback address, got \
         {published:?}"
    );
    let resource = dbuser_arn(&proxy_arn, MASTER_USER);

    // The principals, each with the documents its row is about. Every grant is written over
    // the resource a CLIENT derives from `DBProxyArn`, so what is under test is the resource
    // the proxy asks IAM about.
    let granted = role_session(
        &router,
        "a grant on that dbuser resource",
        "emu-proxy-iam-granted",
        Some(document(&[("Allow", "rds-db:connect", &resource)])),
    )
    .await;
    let wildcard = role_session(
        &router,
        "a grant written with wildcards",
        "emu-proxy-iam-wildcard",
        Some(document(&[(
            "Allow",
            "rds-db:*",
            &format!("arn:aws:rds-db:{REGION}:{}:dbuser:*/*", account()),
        )])),
    )
    .await;
    let another_user = role_session(
        &router,
        "a grant on another database user",
        "emu-proxy-iam-other-user",
        Some(document(&[(
            "Allow",
            "rds-db:connect",
            &dbuser_arn(&proxy_arn, "someone_else"),
        )])),
    )
    .await;
    let another_action = role_session(
        &router,
        "a grant of another action",
        "emu-proxy-iam-other-action",
        Some(document(&[("Allow", "rds:DescribeDBProxies", "*")])),
    )
    .await;
    let denied = role_session(
        &router,
        "an explicit deny beside a wildcard allow",
        "emu-proxy-iam-denied",
        Some(document(&[
            ("Allow", "rds-db:connect", "*"),
            ("Deny", "rds-db:connect", &resource),
        ])),
    )
    .await;
    let unstated = role_session(
        &router,
        "a principal stating no document at all",
        "emu-proxy-iam-unstated",
        None,
    )
    .await;
    let long_term = user_key(
        &router,
        "an IAM user's long-term key",
        "emu-proxy-iam-user",
        &document(&[("Allow", "rds-db:connect", &resource)]),
    )
    .await;

    let now = SystemTime::now();
    let token_of = |akid: &str| auth_token(akid, &host, port, MASTER_USER, REGION, now);

    // (case, the credential presented, whether the proxy admits the session)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a token signed by a role session granted connect on that dbuser resource",
            token_of(&granted),
            true,
        ),
        (
            "a token signed by a principal whose grant is written with wildcards",
            token_of(&wildcard),
            true,
        ),
        (
            "a token signed by an IAM user granted connect on that dbuser resource",
            token_of(&long_term),
            true,
        ),
        (
            "a token signed by a principal granted connect on another database user",
            token_of(&another_user),
            false,
        ),
        (
            "a token signed by a principal granted another action entirely",
            token_of(&another_action),
            false,
        ),
        (
            "a token signed by a principal an explicit deny covers",
            token_of(&denied),
            false,
        ),
        (
            "a token signed by a principal stating no document at all",
            token_of(&unstated),
            false,
        ),
        (
            "a token generated for the published host completed with the engine family's port",
            auth_token(&granted, &host, MYSQL_FAMILY_PORT, MASTER_USER, REGION, now),
            true,
        ),
        (
            // `DBProxy$Endpoint` carries no port member on AWS, so a client takes the whole
            // published value as the host it completes with its engine family's port — which
            // is the form `generate_db_auth_token` is called in with the endpoint this
            // emulator publishes.
            "a token generated for the published endpoint completed with that port",
            auth_token(&granted, &published, MYSQL_FAMILY_PORT, MASTER_USER, REGION, now),
            true,
        ),
        (
            // The other reading of the same published value: the whole authority as the host,
            // completed with the port that authority itself spells. Both readings name this one
            // endpoint, so a token generated for either is a token for it.
            "a token generated for the published endpoint completed with the port it spells",
            auth_token(&granted, &published, port, MASTER_USER, REGION, now),
            true,
        ),
        (
            "a token the granted principal generated for another endpoint",
            auth_token(&granted, &host, ANOTHER_PORT, MASTER_USER, REGION, now),
            false,
        ),
        (
            "a token the granted principal generated for another database user",
            auth_token(&granted, &host, port, "someone_else", REGION, now),
            false,
        ),
        (
            "a token the granted principal signed in another Region",
            auth_token(&granted, &host, port, MASTER_USER, "eu-west-1", now),
            false,
        ),
        (
            "a token whose window has passed",
            auth_token(
                &granted,
                &host,
                port,
                MASTER_USER,
                REGION,
                now - Duration::from_secs(3_600),
            ),
            false,
        ),
        (
            // IAM authentication is REQUIRED for this user, so the database's own password
            // authenticates nothing on the proxy's hop.
            "the database's own password, where IAM authentication is required",
            MASTER_PASSWORD.to_string(),
            false,
        ),
    ];

    for (case, credential, admitted) in rows {
        match login(&host, port, &credential).await {
            Ok(mut session) => {
                assert!(
                    admitted,
                    "case {case:?}: expected the proxy to refuse the session, got one it \
                     admitted"
                );
                // An admitted session is a session like any other: it runs statements against
                // the database the proxy's target group holds, as the user the token named.
                let answered = session
                    .query("SELECT 1")
                    .await
                    .unwrap_or_else(|refusal| panic!("case {case:?}: the admitted session must serve statements, got {refusal:?}"));
                assert_eq!(
                    answered.only(),
                    Some("1"),
                    "case {case:?}: expected the statement to answer 1, got {answered:?}"
                );
                let who = session.query("SELECT CURRENT_USER()").await.unwrap_or_else(
                    |refusal| panic!("case {case:?}: the session must report its user, got {refusal:?}"),
                );
                assert_eq!(
                    who.only(),
                    Some(format!("{MASTER_USER}@%").as_str()),
                    "case {case:?}: expected the session to hold the user the token named, got \
                     {who:?}"
                );
            }
            Err(refusal) => {
                assert!(
                    !admitted,
                    "case {case:?}: expected the proxy to admit the session, got {refusal:?}"
                );
                assert_eq!(
                    refusal,
                    access_denied(&host),
                    "case {case:?}: expected MySQL's own access-denied refusal, got {refusal:?}"
                );
            }
        }
    }
}

/// RULE: `UserAuthConfig$UserName` is optional, and the credential a proxy logs in with is the
/// one in the secret the same entry names — so an `Auth` entry that spells no user out is in
/// force for the DATABASE USER THAT SECRET HOLDS, and an entry whose secret states no user is in
/// force for nobody and decides nothing about any client.
///
/// The secret is Secrets Manager's own record here, written through its own operation and read
/// through the in-process callback a session on the endpoint holds, so what the proxy reads is
/// the document a caller really stored — fields of every type a credential document carries
/// included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_auth_entry_naming_no_user_is_in_force_for_the_user_its_secret_holds() {
    let router = emulator_with_secrets();
    let secrets = crate::managed_master_user_password::secrets_manager_in(&router, REGION);
    // The credential document AWS documents for a database secret, beside the other fields such
    // a document carries — including one that is not a string.
    let credential = planted_secret(
        &secrets,
        "emu-proxy-iam-secret-user",
        &format!(
            r#"{{"username":"{MASTER_USER}","password":"{MASTER_PASSWORD}","engine":"mysql","port":3306,"dbname":"{DATABASE}"}}"#
        ),
    )
    .await;
    // A document that states no user at all, so the entry pointing at it is in force for nobody.
    let userless = planted_secret(
        &secrets,
        "emu-proxy-iam-secret-userless",
        &format!(r#"{{"password":"{MASTER_PASSWORD}","engine":"mysql","port":3306}}"#),
    )
    .await;
    available_instance(&router, INSTANCE, "mysql", &[]).await;
    let (published, proxy_arn) = available_proxy_with(
        &router,
        SECRET_USER_PROXY,
        "REQUIRED",
        true,
        &credential,
        false,
    )
    .await;
    registered_target(&router, SECRET_USER_PROXY).await;
    let (host, port) = dialled("the proxy whose Auth names no user", &published);
    let granted = user_key(
        &router,
        "a grant on the dbuser resource of the secret's user",
        "emu-proxy-iam-secret-user-key",
        &document(&[(
            "Allow",
            "rds-db:connect",
            &dbuser_arn(&proxy_arn, MASTER_USER),
        )]),
    )
    .await;
    let other = user_key(
        &router,
        "a grant on another database user's resource",
        "emu-proxy-iam-secret-other-key",
        &document(&[(
            "Allow",
            "rds-db:connect",
            &dbuser_arn(&proxy_arn, "someone_else"),
        )]),
    )
    .await;
    let now = SystemTime::now();

    // (case, the credential presented, whether the proxy admits the session)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a token signed by the principal granted connect on the secret's own user",
            auth_token(&granted, &host, port, MASTER_USER, REGION, now),
            true,
        ),
        (
            "a token signed by a principal granted connect on another database user",
            auth_token(&other, &host, port, MASTER_USER, REGION, now),
            false,
        ),
        (
            // IAM authentication is REQUIRED for the user the secret holds, so the database's
            // own password authenticates nothing on this proxy's hop either.
            "the database's own password, where the secret's user requires IAM authentication",
            MASTER_PASSWORD.to_string(),
            false,
        ),
    ];
    for (case, credential, admitted) in rows {
        match login(&host, port, &credential).await {
            Ok(mut session) => {
                assert!(
                    admitted,
                    "case {case:?}: expected the proxy to refuse the session, got one it \
                     admitted"
                );
                let answered = session.query("SELECT 1").await.unwrap_or_else(|refusal| {
                    panic!(
                        "case {case:?}: the admitted session must serve statements, got {refusal:?}"
                    )
                });
                assert_eq!(
                    answered.only(),
                    Some("1"),
                    "case {case:?}: expected the statement to answer 1, got {answered:?}"
                );
            }
            Err(refusal) => {
                assert!(
                    !admitted,
                    "case {case:?}: expected the proxy to admit the session, got {refusal:?}"
                );
                assert_eq!(
                    refusal,
                    access_denied(&host),
                    "case {case:?}: expected MySQL's own access-denied refusal, got {refusal:?}"
                );
            }
        }
    }

    // And the boundary: an entry whose secret states no user names no user, so the proxy decides
    // nothing about this client and the database's own credential stands.
    let (published, _arn) = available_proxy_with(
        &router,
        USERLESS_SECRET_PROXY,
        "REQUIRED",
        true,
        &userless,
        false,
    )
    .await;
    registered_target(&router, USERLESS_SECRET_PROXY).await;
    let (host, port) = dialled("the proxy whose secret states no user", &published);
    let case = "the database's own password, where the entry's secret states no user";
    let mut session = login(&host, port, MASTER_PASSWORD)
        .await
        .unwrap_or_else(|refusal| {
            panic!("case {case:?}: expected the database's own password to admit the session, got {refusal:?}")
        });
    let answered = session
        .query("SELECT 1")
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: the admitted session must serve statements, got {refusal:?}"));
    assert_eq!(
        answered.only(),
        Some("1"),
        "case {case:?}: expected the statement to answer 1, got {answered:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_that_disallows_iam_auth_still_admits_the_databases_own_password() {
    let router = emulator();
    available_instance(&router, INSTANCE, "mysql", &[]).await;
    let (published, proxy_arn) = available_proxy(&router, PASSWORD_PROXY, "DISABLED", false).await;
    registered_target(&router, PASSWORD_PROXY).await;
    let (host, port) = dialled("the password proxy", &published);
    let resource = dbuser_arn(&proxy_arn, MASTER_USER);
    // A principal granted exactly the connect action on this proxy's own dbuser resource: the
    // grant is not what admits a client of a proxy that disallows IAM authentication, so a
    // token signed by it authenticates nothing here.
    let granted = user_key(
        &router,
        "a grant on that dbuser resource",
        "emu-proxy-password-user",
        &document(&[("Allow", "rds-db:connect", &resource)]),
    )
    .await;
    let token = auth_token(&granted, &host, port, MASTER_USER, REGION, SystemTime::now());

    // (case, the credential presented, whether the proxy admits the session)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the database's own password",
            MASTER_PASSWORD.to_string(),
            true,
        ),
        (
            "a token signed by a principal granted connect on that dbuser resource",
            token,
            false,
        ),
    ];
    for (case, credential, admitted) in rows {
        // This proxy requires no encryption of its clients, so each row is driven in the
        // clear as well as inside a TLS session: what admits a login is the credential, not
        // the transport.
        for transport in [Transport::Plaintext, Transport::Tls] {
            let answered = mysql_login(
                &host,
                port,
                transport,
                MASTER_USER,
                &credential,
                Some(DATABASE),
            )
            .await;
            match answered {
                Ok(mut session) => {
                    assert!(
                        admitted,
                        "case {case:?} over {transport:?}: expected the proxy to refuse the \
                         session, got one it admitted"
                    );
                    let read = session.query("SELECT 1").await.unwrap_or_else(|refusal| {
                        panic!(
                            "case {case:?} over {transport:?}: the admitted session must serve \
                             statements, got {refusal:?}"
                        )
                    });
                    assert_eq!(
                        read.only(),
                        Some("1"),
                        "case {case:?} over {transport:?}: expected the statement to answer 1, \
                         got {read:?}"
                    );
                }
                Err(refusal) => {
                    assert!(
                        !admitted,
                        "case {case:?} over {transport:?}: expected the proxy to admit the \
                         session, got {refusal:?}"
                    );
                    assert_eq!(
                        refusal,
                        access_denied(&host),
                        "case {case:?} over {transport:?}: expected MySQL's own access-denied \
                         refusal, got {refusal:?}"
                    );
                }
            }
        }
    }
}
