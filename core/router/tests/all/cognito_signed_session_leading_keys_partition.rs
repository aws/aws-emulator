// SPDX-License-Identifier: CC0-1.0

//! The documented Cognito-Identity multi-tenant flow, driven end to end on a router that
//! AUTHENTICATES every request: an identity pool vends a session, and the client SIGNS its
//! DynamoDB calls with that session.
//!
//! RULE: a session an identity pool vends is a complete, immediately usable AWS credential
//! — one `GetCredentialsForIdentity` call answers an access key id, a secret key and a
//! session token for an `sts:AssumeRoleWithWebIdentity` session of the role the pool
//! resolved, so a request SIGNED with all three verifies and speaks for that assumed-role
//! session; nothing in the exchange is long-running and no leg of it has to be retried
//! before it answers. The decision on such a request is then the role's own policy: a
//! statement conditioned on the partitions the call addresses
//! (`dynamodb:LeadingKeys`) pinned to the subject the exchange published
//! (`${cognito-identity.amazonaws.com:sub}`) grants a call addressing the caller's OWN
//! partition and grants none addressing another identity's, which is refused HTTP 400
//! `AccessDeniedException` worded from the request: the session principal, the action and
//! the table's ARN.
//!
//! Five services have to agree for that to hold — the SigV4 request authenticator has to
//! verify the vended credential, Cognito Identity has to resolve the pool's role and make
//! the exchange, IAM has to hold the role and its policy, STS has to record the session and
//! name its principal, and DynamoDB has to make the decision — so this is the suite that
//! can pin it; a crate-local test reaches only one of them, and a suite that turns request
//! authentication OFF never asks whether the vended credential signs anything at all.
//!
//! The table spans what the rule generalises over: WHICH identity of the pool is signing
//! (three of them, so "its own partition" is a different partition for each and no row is
//! the pool's first or only identity), WHICH partition the call addresses (its own, or
//! either of the two others), and HOW the call names that partition (a `Query` naming it
//! through the key condition, a `GetItem` naming it through a key map) — and each vend is
//! read back through `sts:GetCallerIdentity` under the vended credential, which is where
//! the session principal a refusal names comes from.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), spelled
/// out so a wrong constant in the implementation cannot make this suite pass.
const ACCOUNT: &str = "000000000000";

/// The Region every call here is signed for and addressed in.
const REGION: &str = "us-east-1";

/// The table every row acts on, keyed on the identity id that owns each row.
const TABLE: &str = "SignedPerIdentityRows";

/// The partition-key attribute: the identity id a row belongs to, which is what the
/// fine-grained statement scopes a session to.
const PARTITION_ATTRIBUTE: &str = "UserId";

/// The sort-key attribute, so a key map names two attributes.
const SORT_ATTRIBUTE: &str = "ItemId";

/// The sort-key value of the row seeded in each identity's partition.
const SEEDED_ROW: &str = "row-1";

/// The role the pool vends unauthenticated sessions of.
const ROLE_NAME: &str = "SignedPerIdentityRole";

/// The identity-provider principal an identity pool's exchange is made under.
const COGNITO_IDENTITY_PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// The role-session name an identity pool's own exchange carries, which is the last segment
/// of the assumed-role ARN the session speaks for
/// (`service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::VENDED_ROLE_SESSION_NAME`).
const VENDED_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// How many guest identities of the one pool sign here.
const IDENTITIES: usize = 3;

/// How long a poll loop waits for a state the clock owes. Generous: what is under test is
/// the decision and the vend, never how fast the host reaches a transition.
const BUDGET: Duration = Duration::from_secs(60);

/// How long a poll loop waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// The five services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them — with request authentication ON, as the emulator serves traffic, so every call
/// below carries a real signature.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
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
        service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
    );
    router.set_request_authentication(true);
    router
}

/// A credential requests are signed with: the pair this process publishes (the account's
/// own), or a session an identity pool vended, which carries a session token too.
struct Signer {
    key_id: String,
    secret: String,
    session_token: Option<String>,
}

impl Signer {
    /// The credential this process publishes once request authentication is on, which is
    /// the account's own: every SETUP call here is signed with it, and no graded one is.
    fn of_this_process() -> Self {
        let published = ::emulator_server::SigV4Authenticator::minted_credentials()
            .expect("turning request authentication on publishes this process's keys");
        Self {
            key_id: published.access_key_id.clone(),
            secret: published.secret_access_key.clone(),
            session_token: None,
        }
    }

    /// Sign `request` for `signing_name` in [`REGION`], in the `Authorization` header an SDK
    /// request carries — plus the `x-amz-security-token` a temporary credential is served
    /// with.
    fn sign(
        &self,
        mut request: http::Request<Vec<u8>>,
        signing_name: &str,
    ) -> http::Request<Vec<u8>> {
        let identity = Credentials::new(
            &self.key_id,
            &self.secret,
            self.session_token.clone(),
            None,
            "test",
        )
        .into();
        let params = SigningParams::builder()
            .identity(&identity)
            .region(REGION)
            .name(signing_name)
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .expect("the signing parameters are complete")
            .into();
        let (method, uri) = (request.method().to_string(), request.uri().to_string());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().expect("a text header").to_string(),
                )
            })
            .collect();
        let signable = SignableRequest::new(
            &method,
            &uri,
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            SignableBody::Bytes(request.body()),
        )
        .expect("the request is signable");
        let (instructions, _) = sign(signable, &params)
            .expect("the request signs")
            .into_parts();
        instructions.apply_to_request_http1x(&mut request);
        request
    }
}

/// One signed call: `POST /` on the regional endpoint of `signing_name`, carrying
/// `content_type`, the `X-Amz-Target` an RPC protocol routes on, and `body`. Answers the
/// status and the body text.
async fn call(
    router: &MultiServiceRouter,
    signer: &Signer,
    signing_name: &str,
    content_type: &str,
    target: Option<&str>,
    body: Vec<u8>,
) -> (u16, String) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", content_type);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let built = builder.body(body).expect("the test request is well formed");
    let (parts, payload) = signer.sign(built, signing_name).into_parts();
    let request = http::Request::from_parts(parts, bytes::Bytes::from(payload));
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One signed JSON-RPC call, answering the status and the parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signer: &Signer,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, text) = call(
        router,
        signer,
        signing_name,
        content_type,
        Some(target),
        serde_json::to_vec(body).expect("the test body serializes"),
    )
    .await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// One signed Amazon Cognito Identity call (`awsJson1_1`).
async fn cognito(
    router: &MultiServiceRouter,
    signer: &Signer,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        router,
        signer,
        "cognito-identity",
        "application/x-amz-json-1.1",
        &format!("AWSCognitoIdentityService.{operation}"),
        body,
    )
    .await
}

/// One signed DynamoDB call (`awsJson1_0`).
async fn ddb(
    router: &MultiServiceRouter,
    signer: &Signer,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        router,
        signer,
        "dynamodb",
        "application/x-amz-json-1.0",
        &format!("DynamoDB_20120810.{operation}"),
        body,
    )
    .await
}

/// One signed `awsQuery` form call (IAM, STS), answering the status and the body text.
async fn form(
    router: &MultiServiceRouter,
    signer: &Signer,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    call(
        router,
        signer,
        signing_name,
        "application/x-www-form-urlencoded",
        None,
        body.into_bytes(),
    )
    .await
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The wire error code (`__type`) of a refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message`/`Message` member of a refusal, whichever casing it carries.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The table's ARN, the resource the role's statement names and a refusal reports.
fn table_arn() -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}")
}

/// Create the table every row acts on, as the account's own credential.
async fn create_table(router: &MultiServiceRouter, account: &Signer) {
    let (status, created) = ddb(
        router,
        account,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": PARTITION_ATTRIBUTE, "KeyType": "HASH"},
                {"AttributeName": SORT_ATTRIBUTE, "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": PARTITION_ATTRIBUTE, "AttributeType": "S"},
                {"AttributeName": SORT_ATTRIBUTE, "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateTable expected 200, got {status} {created}"
    );
}

/// Wait until DynamoDB first reports the table `ACTIVE`: "you can perform read and write
/// operations only on an ACTIVE table".
async fn await_table_active(router: &MultiServiceRouter, account: &Signer) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (_, described) = ddb(
            router,
            account,
            "DescribeTable",
            &json!({"TableName": TABLE}),
        )
        .await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: DynamoDB never reported {TABLE} ACTIVE, last read {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Seed one row in `user`'s partition, as the account's own credential.
async fn seed_row(router: &MultiServiceRouter, account: &Signer, user: &str) {
    let (status, put) = ddb(
        router,
        account,
        "PutItem",
        &json!({
            "TableName": TABLE,
            "Item": {
                PARTITION_ATTRIBUTE: {"S": user},
                SORT_ATTRIBUTE: {"S": SEEDED_ROW},
                "payload": {"S": "the row that belongs to this identity"},
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutItem for {user:?} expected 200, got {status} {put}"
    );
}

/// `cognito-identity:CreateIdentityPool`, answering the pool's id.
async fn create_identity_pool(
    router: &MultiServiceRouter,
    account: &Signer,
    pool_name: &str,
) -> String {
    let (status, pool) = cognito(
        router,
        account,
        "CreateIdentityPool",
        &json!({"IdentityPoolName": pool_name, "AllowUnauthenticatedIdentities": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateIdentityPool({pool_name:?}) expected 200, got {status} {pool}"
    );
    pool["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateIdentityPool reports no IdentityPoolId: {pool}"))
        .to_string()
}

/// `cognito-identity:GetId`, answering the id of a fresh guest identity of `pool_id`.
async fn get_id(router: &MultiServiceRouter, account: &Signer, pool_id: &str) -> String {
    let (status, identity) =
        cognito(router, account, "GetId", &json!({"IdentityPoolId": pool_id})).await;
    assert_eq!(
        status, 200,
        "setup: GetId({pool_id:?}) expected 200, got {status} {identity}"
    );
    identity["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: GetId reports no IdentityId: {identity}"))
        .to_string()
}

/// The trust policy of the role an identity pool vends guest sessions of, written the way
/// the Amazon Cognito documentation writes one: the `Federated` Cognito Identity principal,
/// the web-identity assume, the `:aud` condition that restricts the assume to the users of
/// that one pool, and the `:amr` condition that restricts it to the unauthenticated flow.
///
/// Compact and carrying none of the characters that are significant in an
/// `application/x-www-form-urlencoded` body (`&`, `=`, `+`, `%`, space), so it needs no
/// encoding.
fn pool_trust_policy(pool_id: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"{COGNITO_IDENTITY_PRINCIPAL}\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\
         \"StringEquals\":{{\"{COGNITO_IDENTITY_PRINCIPAL}:aud\":\"{pool_id}\"}},\
         \"ForAnyValue:StringLike\":\
         {{\"{COGNITO_IDENTITY_PRINCIPAL}:amr\":\"unauthenticated\"}}}}}}]}}"
    )
}

/// The role's own identity-based policy: the two documented fine-grained read actions on the
/// table, each scoped to the partitions the call addresses, pinned to the subject of the
/// token the session was minted by exchanging.
///
/// Compact and free of the characters a form body treats specially, exactly like
/// [`pool_trust_policy`].
fn role_policy() -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":[\"dynamodb:Query\",\"dynamodb:GetItem\"],\"Resource\":\"{}\",\
         \"Condition\":{{\"ForAllValues:StringEquals\":{{\"dynamodb:LeadingKeys\":\
         [\"${{{COGNITO_IDENTITY_PRINCIPAL}:sub}}\"]}}}}}}]}}",
        table_arn()
    )
}

/// `iam:CreateRole` plus `iam:PutRolePolicy` for the role the pool vends sessions of;
/// answers the role's ARN.
async fn create_pool_role(
    router: &MultiServiceRouter,
    account: &Signer,
    pool_id: &str,
) -> String {
    let (status, created) = form(
        router,
        account,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={ROLE_NAME}\
             &AssumeRolePolicyDocument={}",
            pool_trust_policy(pool_id)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole({ROLE_NAME:?}) expected 200, got {status} {created}"
    );
    let (status, attached) = form(
        router,
        account,
        "iam",
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={ROLE_NAME}\
             &PolicyName=signed-per-identity-rows&PolicyDocument={}",
            role_policy()
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutRolePolicy({ROLE_NAME:?}) expected 200, got {status} {attached}"
    );
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: CreateRole reports no <Arn>: {created}"))
}

/// `cognito-identity:SetIdentityPoolRoles` — the role a guest exchange of `pool_id` vends a
/// session of.
async fn set_unauthenticated_role(
    router: &MultiServiceRouter,
    account: &Signer,
    pool_id: &str,
    role_arn: &str,
) {
    let (status, configured) = cognito(
        router,
        account,
        "SetIdentityPoolRoles",
        &json!({"IdentityPoolId": pool_id, "Roles": {"unauthenticated": role_arn}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: SetIdentityPoolRoles expected 200, got {status} {configured}"
    );
}

/// ONE `GetCredentialsForIdentity` call for `identity_id`, which must answer a complete,
/// signable session: the three members a client signs with, each present and non-empty, and
/// the expiry the session was minted with. No poll loop — the exchange answers or this
/// fails, which is the difference between a verdict and a hang.
fn vended_session(case: &str, vended: &Value) -> Signer {
    let member = |name: &str| {
        let value = vended["Credentials"][name].as_str().unwrap_or_default();
        // No secret reaches an assertion message: presence and emptiness are what is
        // reported.
        assert!(
            !value.is_empty(),
            "case {case:?}: the vended session must carry a non-empty {name:?}, got the \
             member absent or empty"
        );
        value.to_string()
    };
    let key_id = member("AccessKeyId");
    let secret = member("SecretKey");
    let session_token = member("SessionToken");
    assert!(
        vended["Credentials"]["Expiration"].is_number(),
        "case {case:?}: the vended session must carry the expiry it was minted with, got \
         {:?}",
        vended["Credentials"]["Expiration"]
    );
    Signer {
        key_id,
        secret,
        session_token: Some(session_token),
    }
}

/// `sts:GetCallerIdentity` under the vended credential: the principal the session speaks
/// for, which is what every refusal below names. A credential the request authenticator
/// does not verify never reaches this at all.
async fn signed_caller_arn(router: &MultiServiceRouter, case: &str, session: &Signer) -> String {
    let (status, body) = form(
        router,
        session,
        "sts",
        "Action=GetCallerIdentity&Version=2011-06-15".to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a request signed with the vended session expected HTTP 200 from \
         sts:GetCallerIdentity, got {status} {body}"
    );
    xml_element(&body, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: GetCallerIdentity reports no <Arn>: {body}"))
}

/// One call a row is driven through: the IAM action it performs, the operation that performs
/// it, and the request body that addresses `user`'s partition with it.
struct Driven {
    action: &'static str,
    operation: &'static str,
    request: Value,
}

/// The two reads each row drives against `user`'s partition: a `Query` naming the partition
/// in its key condition, and a `GetItem` naming it in a key map — the rule is about the
/// partition a call addresses, however it names it.
fn driven(user: &str) -> Vec<Driven> {
    vec![
        Driven {
            action: "dynamodb:Query",
            operation: "Query",
            request: json!({
                "TableName": TABLE,
                "KeyConditionExpression": "#p = :u",
                "ExpressionAttributeNames": {"#p": PARTITION_ATTRIBUTE},
                "ExpressionAttributeValues": {":u": {"S": user}},
            }),
        },
        Driven {
            action: "dynamodb:GetItem",
            operation: "GetItem",
            request: json!({
                "TableName": TABLE,
                "Key": {
                    PARTITION_ATTRIBUTE: {"S": user},
                    SORT_ATTRIBUTE: {"S": SEEDED_ROW},
                },
            }),
        },
    ]
}

#[tokio::test]
async fn a_signed_cognito_session_reads_only_the_partition_its_subject_names() {
    let router = build_router();
    let account = Signer::of_this_process();

    create_table(&router, &account).await;
    await_table_active(&router, &account).await;

    let pool_id = create_identity_pool(&router, &account, "signed-per-identity-pool").await;
    let mut identities: Vec<String> = Vec::new();
    for position in 0..IDENTITIES {
        let identity = get_id(&router, &account, &pool_id).await;
        assert!(
            !identities.contains(&identity),
            "setup: GetId number {position} must mint an identity of its own, got {identity:?} \
             which is already among {identities:?}"
        );
        seed_row(&router, &account, &identity).await;
        identities.push(identity);
    }

    let role_arn = create_pool_role(&router, &account, &pool_id).await;
    set_unauthenticated_role(&router, &account, &pool_id, &role_arn).await;
    let session_arn =
        format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE_NAME}/{VENDED_SESSION_NAME}");

    for own in &identities {
        let case = format!("the session vended to {own}");
        // ONE exchange, answered in one call: the whole flow is synchronous.
        let (status, vended) = cognito(
            &router,
            &account,
            "GetCredentialsForIdentity",
            &json!({"IdentityId": own}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCredentialsForIdentity expected HTTP 200, got {status} {vended}"
        );
        assert_eq!(
            vended["IdentityId"].as_str(),
            Some(own.as_str()),
            "case {case:?}: the vend must answer for the identity it was asked for, got {vended}"
        );
        let session = vended_session(&case, &vended);
        let signed_arn = signed_caller_arn(&router, &case, &session).await;
        assert_eq!(
            signed_arn, session_arn,
            "case {case:?}: a request the vended session signs must speak for the \
             assumed-role session of the pool's role, got {signed_arn:?}"
        );

        for addressed in &identities {
            let own_partition = addressed == own;
            let whose = if own_partition {
                "its own partition"
            } else {
                "another identity's partition"
            };
            for drive in driven(addressed) {
                let Driven {
                    action,
                    operation,
                    request,
                } = drive;
                let (status, answer) = ddb(&router, &session, operation, &request).await;
                let expected = if own_partition { 200 } else { 400 };
                assert_eq!(
                    status, expected,
                    "case {case:?}: {operation} on {whose} ({addressed}) expected HTTP \
                     {expected}, got {status} {answer}"
                );
                if !own_partition {
                    assert_eq!(
                        error_code(&answer),
                        "AccessDeniedException",
                        "case {case:?}: {operation} on {whose} ({addressed}) expected \
                         AccessDeniedException, got {answer}"
                    );
                    let wanted = format!(
                        "User: {session_arn} is not authorized to perform: {action} on \
                         resource: {} because no identity-based policy allows the {action} \
                         action",
                        table_arn()
                    );
                    let message = message_of(&answer);
                    assert_eq!(
                        message, wanted,
                        "case {case:?}: {operation} on {whose} ({addressed}) expected the \
                         refusal {wanted:?}, got {message:?}"
                    );
                    continue;
                }
                // Its own partition answers the row the account seeded there, and nothing of
                // any other identity's.
                let read: Vec<&Value> = if operation == "Query" {
                    answer["Items"].as_array().into_iter().flatten().collect()
                } else {
                    vec![&answer["Item"]]
                };
                assert_eq!(
                    read.len(),
                    1,
                    "case {case:?}: {operation} on {whose} ({addressed}) must answer the one \
                     row seeded there, got {answer}"
                );
                for item in read {
                    assert_eq!(
                        item[PARTITION_ATTRIBUTE],
                        json!({"S": addressed}),
                        "case {case:?}: every row {operation} answers belongs to the \
                         partition it addressed, got {item}"
                    );
                }
            }
        }
    }
}
