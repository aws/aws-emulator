// SPDX-License-Identifier: CC0-1.0

//! The IAM-AUTHENTICATED hop a client makes to a DB PROXY, end to end over the wire: a
//! `python3.x` function attached to the proxy's VPC that mints an authentication token through
//! its own SDK for the address Amazon RDS published, presents it in place of a password, and is
//! admitted or refused on the `rds-db:connect` grant its execution role holds.
//!
//! RULE: where a proxy's `Auth` requires IAM authentication of connections to it
//! (`UserAuthConfigInfo$IAMAuth`), the credential a client presents is an authentication token
//! it minted IN ITS OWN CLIENT for the address it then dials — the value `DBProxy$Endpoint`
//! published, completed with a port, whatever form that value takes — and the session is
//! admitted only when the principal that SIGNED the token is allowed `rds-db:connect` on
//! `arn:aws:rds-db:<region>:<account>:dbuser:<proxy id>/<database user>`. A principal holding
//! no such grant is refused by the engine's own access-denied answer (MySQL 1045) and runs no
//! statement.
//!
//! Every hop of that rule is the emulator's own: the token is minted by the runtime's SDK seam
//! for the address the control plane published (so a published value carrying its own port is
//! one a token can be minted for), the grant is read out of IAM's records for the session the
//! runtime minted of the function's execution role, and the refusal is the one the MySQL wire
//! words.
//!
//! The rows differ along what the rule generalises over: whether the proxy's `Auth` entry NAMES
//! the database user or leaves it to the credential its secret holds, which port completes the
//! published value (the one the engine family is dialled on and the one the published authority
//! spells), whether the role that signed the token is granted `rds-db:connect` on that database
//! user, and whether the token was minted for this proxy's address or another's.
//!
//! Four crates have to agree for that to hold — Amazon EC2 owns the VPC, Amazon RDS opens the
//! proxy's listener and decides the hop, IAM holds the grant and the token service the session —
//! so all of them are mounted here on ONE engine, exactly as the assembled router mounts them.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;

use crate::function_lifecycle::{create_function, json_call, make_activation_due, Answer};
use crate::handler_bedrock_agent_sdk::execution_role;
use crate::handler_rds_catalogue_calls::{element, rds};
use crate::handler_rds_proxy_native_session::PROBE_SOURCE;
use crate::handler_secret_relayed_to_queue::secrets;
use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::node_invoke_run::package;
use crate::vpc_attachment_interfaces::{ec2_client, network, Network};

/// The second octet of the VPC the database, the proxies and the functions all sit in.
const PREFIX: u8 = 63;

/// The Availability Zone the second subnet lies in, so the subnet group the database and the
/// proxies are placed in covers two.
const SECOND_ZONE: &str = "us-east-1b";

/// The Region every call below is signed for, which is the Region the proxies live in and the
/// Region a token for their endpoints has to be signed in.
const REGION: &str = "us-east-1";

/// The database resources every row acts on, and the credential the database was created with —
/// the pair the proxies' `Auth` secret holds.
const SUBNET_GROUP: &str = "proxy-iam-subnets";
const INSTANCE: &str = "proxy-iam-instance";
const MASTER_USER: &str = "admin";
const MASTER_PASSWORD: &str = "initial-Password-1";
const DATABASE: &str = "appdb";

/// The secret the proxies' `Auth` entry names as the credential they log in with.
const SECRET_NAME: &str = "proxy-iam-credential";

/// The two proxies the rows dial, both requiring IAM authentication of connections to them: one
/// whose `Auth` entry NAMES the database user, and one that leaves the user to the credential
/// the secret holds, as the `Auth` of a proxy created with no `UserName` does.
const NAMED_PROXY: &str = "proxy-iam-named-user";
const UNNAMED_PROXY: &str = "proxy-iam-secret-user";

/// An IAM role ARN of the form `CreateDBProxy$RoleArn` takes: the role a proxy reads its
/// credentials with.
const PROXY_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy";

/// The two execution roles the functions run as, and the functions themselves: one role granted
/// `rds-db:connect` on the database user of each proxy, one granted it on another database user
/// only.
const GRANTED_ROLE: &str = "proxy-iam-granted";
const UNGRANTED_ROLE: &str = "proxy-iam-ungranted";
const GRANTED_FUNCTION: &str = "proxy-iam-granted-probe";
const UNGRANTED_FUNCTION: &str = "proxy-iam-ungranted-probe";

/// The database user the ungranted role IS allowed to connect as, which is what makes its
/// permissions readable and its refusal a matter of the RESOURCE rather than of a missing
/// document.
const OTHER_DB_USER: &str = "someone-else";

/// How long one invocation is given, as the function's own `Timeout`: well past what one MySQL
/// exchange needs on any host, because what is under test is the decision the proxy makes and
/// not how fast an interpreter gets through a handshake.
const FUNCTION_TIMEOUT: u64 = 60;

/// The memory the probe is configured for. No row tests a memory limit, so the bound is
/// generous: an interpreter standing up on a loaded host must not decide the outcome.
const FUNCTION_MEMORY: u64 = 3_008;

/// How long a wait for work a service's own clock owes is given, and how often to look. Every
/// wait is for the CONDITION, and the deadline is generous so a loaded host cannot decide the
/// outcome.
const CLOCK_BUDGET: Duration = Duration::from_secs(120);
/// See [`CLOCK_BUDGET`].
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// How many times an invocation is re-driven while the answer reports one of this test binary's
/// own host bounds rather than a run.
const HOST_BOUND_ATTEMPTS: usize = 6;

/// The port a client dials a MySQL-family proxy on: `DBProxy` carries no port member, so a
/// client completes the host `Endpoint` publishes with the port of the databases the proxy's
/// `EngineFamily` names, which is 3306 for the MySQL family ("Default: … RDS for MySQL — 3306",
/// `smithy_query(query="doc",shape="CreateDBInstanceMessage",member="Port")`).
const MYSQL_FAMILY_PORT: u64 = 3306;

/// MySQL's error number for a refused login: `ER_ACCESS_DENIED_ERROR`, the server-side code a
/// client reads for a credential the server does not admit.
const ACCESS_DENIED: u64 = 1045;

/// The statement an admitted session runs, and the value it must answer.
const SESSION_STATEMENT: &str = "SELECT 1";

/// The handler the functions run: it mints an authentication token through its own SDK for the
/// address the event names, then opens the session with that token in place of a password — the
/// reused MySQL client of the suite over a proxy's own data plane
/// ([`crate::handler_rds_proxy_native_session`]), which already speaks the cleartext-plugin
/// switch a token is presented through.
const TOKEN_PROBE_SOURCE: &str = r#"
import boto3
import index


def handler(event, context):
    token = boto3.client("rds").generate_db_auth_token(
        DBHostname=event["token_host"],
        Port=int(event["token_port"]),
        DBUsername=event["user"],
    )
    asked = dict(event)
    asked["password"] = token
    return index.handler(asked, context)
"#;

/// Lambda, Amazon EC2, Amazon RDS, Secrets Manager, IAM and the token service over ONE shared
/// engine, as `core/router/src/lib.rs` mounts them: the proxy the handler dials is the one this
/// suite created, and the grant its token is judged on is the one IAM holds here.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    ec2: aws_sdk_ec2::Client,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started; what is
    // under test is the session the handler opens, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
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
    let callback = EmbeddedEmulator::from_router(router.clone());
    Emulator {
        ec2: ec2_client(&callback),
        engine,
        router,
    }
}

/// One Amazon RDS call that must be served, as its XML body.
async fn rds_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = rds(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup {case:?}: expected {action} to be served, got {status} with {body}"
    );
    body
}

/// Read one Amazon RDS answer again until `holds`, which is how a change the CLOCK owes is
/// waited for: the condition, under a generous deadline, never a fixed span.
async fn rds_until(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
    holds: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let body = rds_ok(router, case, action, params).await;
        if holds(&body) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "setup {case:?}: {action} never reported the condition, last answer {body}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One more subnet of the VPC, in another Availability Zone: a database and a proxy are placed
/// in a subnet group that needs more than one zone covered, and the shared network helper mints
/// every subnet it is asked for in the Region's first zone.
async fn second_zone_subnet(ec2: &aws_sdk_ec2::Client, vpc_id: &str) -> String {
    let created = ec2
        .create_subnet()
        .vpc_id(vpc_id)
        .cidr_block(format!("10.{PREFIX}.9.0/24"))
        .availability_zone(SECOND_ZONE)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must create the second subnet: {error:?}"));
    created
        .subnet()
        .and_then(aws_sdk_ec2::types::Subnet::subnet_id)
        .unwrap_or_else(|| panic!("setup: CreateSubnet must report the subnet it minted"))
        .to_string()
}

/// The DB instance the proxies pool connections to, in the VPC the functions are attached to.
async fn plant_the_database(router: &MultiServiceRouter, network: &Network) {
    let mut subnets: Vec<(&str, &str)> = vec![
        ("DBSubnetGroupName", SUBNET_GROUP),
        ("DBSubnetGroupDescription", "the database's own subnets"),
    ];
    let subnet_members: Vec<(String, &str)> = network
        .subnet_ids
        .iter()
        .enumerate()
        .map(|(at, subnet)| {
            (
                format!("SubnetIds.SubnetIdentifier.{}", at + 1),
                subnet.as_str(),
            )
        })
        .collect();
    for (name, value) in &subnet_members {
        subnets.push((name.as_str(), value));
    }
    rds_ok(router, "the subnet group", "CreateDBSubnetGroup", &subnets).await;
    rds_ok(
        router,
        "the database",
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", INSTANCE),
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.t3.micro"),
            ("AllocatedStorage", "20"),
            ("MasterUsername", MASTER_USER),
            ("MasterUserPassword", MASTER_PASSWORD),
            ("DBName", DATABASE),
            ("DBSubnetGroupName", SUBNET_GROUP),
            ("PubliclyAccessible", "false"),
            (
                "VpcSecurityGroupIds.VpcSecurityGroupId.1",
                network.group_id.as_str(),
            ),
        ],
    )
    .await;
    rds_until(
        router,
        "the database",
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", INSTANCE)],
        |body| element(body, "DBInstanceStatus") == "available",
    )
    .await;
}

/// The secret the proxies' `Auth` entry names, holding the database's own credential — the pair
/// a proxy "uses the credentials in the secret" to log in with.
async fn plant_the_secret(router: &MultiServiceRouter) -> String {
    let created = secrets(
        router,
        "the proxy's credential",
        "CreateSecret",
        &json!({
            "Name": SECRET_NAME,
            // "You must provide a ClientRequestToken value. We recommend a UUID-type value" —
            // the idempotency token Secrets Manager requires of a create made without one
            // generated by an SDK.
            "ClientRequestToken": "2b7f1c90-6d3e-4a52-8c41-0e9f5a3d7b62",
            "SecretString": json!({
                "username": MASTER_USER,
                "password": MASTER_PASSWORD,
            })
            .to_string(),
        }),
    )
    .await;
    created
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: CreateSecret must report the ARN it minted: {created}"))
        .to_string()
}

/// What one proxy publishes: the address a client was handed, and the `dbuser` resource a grant
/// for a login through it is written over — derived from `DBProxyArn` exactly as a client
/// derives it, because that ARN is the only place the control plane publishes the proxy's id.
struct Published {
    endpoint: String,
    dbuser_arn: String,
    other_dbuser_arn: String,
}

/// `CreateDBProxy` of the MySQL family in the VPC, requiring TLS and IAM authentication of its
/// clients, and the wait for the status at which "the proxy is ready to handle requests".
///
/// `names_the_user` says whether the `Auth` entry spells `UserName` out: AWS's own documented
/// create states `AuthScheme`, `SecretArn` and `IAMAuth` and leaves the database user to the
/// credential the secret holds.
async fn plant_the_proxy(
    router: &MultiServiceRouter,
    network: &Network,
    name: &str,
    names_the_user: bool,
    secret_arn: &str,
) -> Published {
    let subnet_members: Vec<(String, &str)> = network
        .subnet_ids
        .iter()
        .enumerate()
        .map(|(at, subnet)| (format!("VpcSubnetIds.member.{}", at + 1), subnet.as_str()))
        .collect();
    let mut params: Vec<(&str, &str)> = vec![
        ("DBProxyName", name),
        ("EngineFamily", "MYSQL"),
        ("RoleArn", PROXY_ROLE_ARN),
        ("RequireTLS", "true"),
        ("VpcSecurityGroupIds.member.1", network.group_id.as_str()),
        ("Auth.member.1.AuthScheme", "SECRETS"),
        ("Auth.member.1.SecretArn", secret_arn),
        ("Auth.member.1.IAMAuth", "REQUIRED"),
    ];
    if names_the_user {
        params.push(("Auth.member.1.UserName", MASTER_USER));
    }
    for (member, value) in &subnet_members {
        params.push((member.as_str(), value));
    }
    let created = rds_ok(router, name, "CreateDBProxy", &params).await;
    let endpoint = element(&created, "Endpoint");
    let proxy_arn = element(&created, "DBProxyArn");
    assert!(
        !endpoint.is_empty(),
        "setup {name:?}: CreateDBProxy must publish an endpoint, got {created}"
    );
    let proxy_id = proxy_arn
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        !proxy_id.is_empty(),
        "setup {name:?}: DBProxyArn must name the proxy, got {created}"
    );
    rds_until(
        router,
        name,
        "DescribeDBProxies",
        &[("DBProxyName", name)],
        |body| element(body, "Status") == "available",
    )
    .await;
    rds_ok(
        router,
        name,
        "RegisterDBProxyTargets",
        &[
            ("DBProxyName", name),
            ("DBInstanceIdentifiers.member.1", INSTANCE),
        ],
    )
    .await;
    rds_until(
        router,
        name,
        "DescribeDBProxyTargets",
        &[("DBProxyName", name)],
        |body| element(body, "State") == "AVAILABLE",
    )
    .await;
    Published {
        endpoint,
        dbuser_arn: format!(
            "arn:aws:rds-db:{REGION}:{}:dbuser:{proxy_id}/{MASTER_USER}",
            engine::emulator_account_id()
        ),
        other_dbuser_arn: format!(
            "arn:aws:rds-db:{REGION}:{}:dbuser:{proxy_id}/{OTHER_DB_USER}",
            engine::emulator_account_id()
        ),
    }
}

/// One identity policy allowing `rds-db:connect` on exactly `resources`, which is the grant
/// "Creating and using an IAM policy for IAM database access" writes over a `dbuser` resource,
/// beside the network-interface calls attaching a function to a VPC takes — the grant AWS's own
/// `AWSLambdaVPCAccessExecutionRole` carries, which every row's role needs and no row is about.
fn connect_policy(resources: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": "rds-db:connect",
                "Resource": resources,
            },
            {
                "Effect": "Allow",
                "Action": [
                    "ec2:CreateNetworkInterface",
                    "ec2:DescribeNetworkInterfaces",
                    "ec2:DeleteNetworkInterface",
                ],
                "Resource": "*",
            },
        ],
    })
    .to_string()
}

/// The body of an answer as text, for a message that says what went wrong.
fn text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// Create one probe function, attached to the VPC the database and the proxies sit in, running
/// as `role_arn`, and make the activation the create owes due.
async fn plant_the_function(
    emulator: &Emulator,
    name: &str,
    role_arn: &str,
    network: &Network,
) {
    let created = create_function(
        &emulator.router,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "token_probe.handler",
            "Timeout": FUNCTION_TIMEOUT,
            "MemorySize": FUNCTION_MEMORY,
            "VpcConfig": {
                "SubnetIds": network.subnet_ids,
                "SecurityGroupIds": [network.group_id],
            },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(&[
                    ("index.py", PROBE_SOURCE),
                    ("token_probe.py", TOKEN_PROBE_SOURCE),
                ])),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: CreateFunction must answer 201 for {name}, got {} {}",
        created.status,
        text(&created)
    );
    // The activation the create owes is the CLOCK's, so it is brought forward rather than
    // waited out.
    make_activation_due(&emulator.engine);
}

/// Invoke one probe with a row's event and answer the report the handler returned.
async fn probe(router: &MultiServiceRouter, case: &str, function: &str, event: &Value) -> Value {
    let uri = format!("/2015-03-31/functions/{function}/invocations");
    let mut answer = json_call(router, "POST", &uri, event).await;
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answer.status, &text(&answer)) {
            break;
        }
        answer = json_call(router, "POST", &uri, event).await;
    }
    assert!(
        !is_host_bound(answer.status, &text(&answer)),
        "case {case:?}: every attempt reported a bound this whole test binary shares, so nothing \
         of the rule was read: {}",
        text(&answer)
    );
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: the invocation must be served, got {} {}",
        answer.status,
        text(&answer)
    );
    let raised = answer.header("x-amz-function-error");
    assert_eq!(
        raised, None,
        "case {case:?}: the handler must run to completion, got {raised:?} with {}",
        text(&answer)
    );
    answer.json()
}

/// Which proxy a row dials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proxy {
    /// The proxy whose `Auth` entry names the database user.
    Named,
    /// The proxy whose `Auth` entry leaves the user to the credential its secret holds.
    FromSecret,
}

/// What a row expects of the session it opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The token admits the session and the statement answers its value.
    Admitted,
    /// The login is refused with MySQL's access-denied number, and no statement runs.
    AccessDenied,
}

/// One row: which proxy is dialled, which port completes its published value, whose token is
/// presented, and what the proxy must answer.
struct Row {
    case: &'static str,
    at: Proxy,
    /// Whether the row completes the published value with the port the engine FAMILY is dialled
    /// on rather than the one the published authority spells.
    at_family_port: bool,
    /// Whether the role that signs the token is the one granted `rds-db:connect` on this
    /// proxy's database user.
    granted: bool,
    /// Whether the token is minted for ANOTHER proxy's address than the one dialled.
    token_for_another_proxy: bool,
    expect: Expected,
}

/// The rows: the combinations of the proxy's own `Auth`, the address the token was minted for
/// and the grant the signing principal holds that the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "the granted role, on the port the engine family is dialled on",
        at: Proxy::Named,
        at_family_port: true,
        granted: true,
        token_for_another_proxy: false,
        expect: Expected::Admitted,
    },
    Row {
        case: "the role with no grant for that database user, on the family port",
        at: Proxy::Named,
        at_family_port: true,
        granted: false,
        token_for_another_proxy: false,
        expect: Expected::AccessDenied,
    },
    Row {
        case: "the granted role, on the port the published authority spells",
        at: Proxy::Named,
        at_family_port: false,
        granted: true,
        token_for_another_proxy: false,
        expect: Expected::Admitted,
    },
    Row {
        case: "a proxy whose Auth leaves the database user to its secret, the granted role",
        at: Proxy::FromSecret,
        at_family_port: false,
        granted: true,
        token_for_another_proxy: false,
        expect: Expected::Admitted,
    },
    Row {
        case: "that same proxy, the role with no grant for that database user",
        at: Proxy::FromSecret,
        at_family_port: false,
        granted: false,
        token_for_another_proxy: false,
        expect: Expected::AccessDenied,
    },
    Row {
        case: "the granted role, presenting a token minted for another proxy's address",
        at: Proxy::Named,
        at_family_port: true,
        granted: true,
        token_for_another_proxy: true,
        expect: Expected::AccessDenied,
    },
];

/// The port of a published `host:port` authority, as a client reads it off the string the
/// control plane handed it.
fn published_port(case: &str, published: &str) -> u64 {
    let (_host, port) = published.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected a published host:port authority, got {published:?}")
    });
    port.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric port in {published:?}"))
}

/// RULE, end to end: a client of an IAM-auth DB proxy presents a token it minted for the
/// address the control plane published, and the proxy admits the session only for a principal
/// allowed `rds-db:connect` on that proxy's own database user.
#[tokio::test]
async fn an_iam_auth_proxy_admits_the_token_of_a_granted_principal_only() {
    let _stream = one_invocation_stream_at_a_time().await;
    let emulator = emulator();
    let case = "the network the database, the proxies and the functions share";
    let mut network = network(&emulator.ec2, case, PREFIX, 1).await;
    let second = second_zone_subnet(&emulator.ec2, &network.vpc_id).await;
    network.subnet_ids.push(second);
    plant_the_database(&emulator.router, &network).await;
    let secret_arn = plant_the_secret(&emulator.router).await;
    // The proxy whose `Auth` leaves the database user to its secret is created FIRST and the one
    // that names the user SECOND, because the port an engine family is dialled on is one port of
    // the one interface this emulator is reached on: the proxy that published its endpoint last
    // is the one answering there, so the rows that dial that port are the rows about that proxy.
    let from_secret =
        plant_the_proxy(&emulator.router, &network, UNNAMED_PROXY, false, &secret_arn).await;
    let named = plant_the_proxy(&emulator.router, &network, NAMED_PROXY, true, &secret_arn).await;
    assert_ne!(
        named.endpoint, from_secret.endpoint,
        "setup: each proxy publishes an address of its own, got {} twice",
        named.endpoint
    );

    // The grants: one role allowed to connect as the database user of EACH proxy, and one
    // allowed to connect as another database user of the same proxies — a readable permission
    // set that does not reach the resource under test.
    let granted = execution_role(
        &emulator.router,
        GRANTED_ROLE,
        &connect_policy(&[&named.dbuser_arn, &from_secret.dbuser_arn]),
    )
    .await;
    let ungranted = execution_role(
        &emulator.router,
        UNGRANTED_ROLE,
        &connect_policy(&[&named.other_dbuser_arn, &from_secret.other_dbuser_arn]),
    )
    .await;
    plant_the_function(&emulator, GRANTED_FUNCTION, &granted, &network).await;
    plant_the_function(&emulator, UNGRANTED_FUNCTION, &ungranted, &network).await;

    for row in ROWS {
        let case = row.case;
        let dialled = match row.at {
            Proxy::Named => &named,
            Proxy::FromSecret => &from_secret,
        };
        let port = match row.at_family_port {
            true => MYSQL_FAMILY_PORT,
            false => published_port(case, &dialled.endpoint),
        };
        // The address the TOKEN was minted for: the one the client is about to dial, or another
        // proxy's — a token is a credential for one endpoint and no other.
        let (token_host, token_port) = match row.token_for_another_proxy {
            true => (
                from_secret.endpoint.clone(),
                published_port(case, &from_secret.endpoint),
            ),
            false => (dialled.endpoint.clone(), port),
        };
        let function = match row.granted {
            true => GRANTED_FUNCTION,
            false => UNGRANTED_FUNCTION,
        };
        let report = probe(
            &emulator.router,
            case,
            function,
            &json!({
                // The client dials the value the control plane published, as it stands: that is
                // the host a driver is handed, and it completes it with a port.
                "host": dialled.endpoint,
                "port": port,
                "token_host": token_host,
                "token_port": token_port,
                "user": MASTER_USER,
                "database": Value::Null,
                // A token is a credential, so the session that presents one is an encrypted
                // one, which is also what the proxies' own `RequireTLS` demands.
                "tls": true,
                "preamble": Vec::<String>::new(),
                "statements": [SESSION_STATEMENT],
            }),
        )
        .await;
        match row.expect {
            Expected::Admitted => {
                assert_eq!(
                    report["ok"],
                    json!(true),
                    "case {case:?}: expected the token to admit the session, got {report}"
                );
                assert_eq!(
                    report.pointer("/statements/0/rows"),
                    Some(&json!([["1"]])),
                    "case {case:?}: expected the value {SESSION_STATEMENT} selects, got {report}"
                );
                let negotiated = report["tls_version"].as_str().unwrap_or_default();
                assert!(
                    negotiated.starts_with("TLS"),
                    "case {case:?}: expected a negotiated TLS session, got {report}"
                );
            }
            Expected::AccessDenied => {
                assert_eq!(
                    (report["ok"].clone(), report["stage"].clone()),
                    (json!(false), json!("login")),
                    "case {case:?}: expected the login to be refused by the server, got {report}"
                );
                assert_eq!(
                    report["code"],
                    json!(ACCESS_DENIED),
                    "case {case:?}: expected MySQL's access-denied number, got {report}"
                );
                let message = report["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(MASTER_USER),
                    "case {case:?}: the refusal must name the user it refused, got {report}"
                );
                assert_eq!(
                    report.get("statements"),
                    None,
                    "case {case:?}: a refused login runs no statement, got {report}"
                );
            }
        }
    }
}
