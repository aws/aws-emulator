// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the identity-pool lifecycle and the identity/credential flow,
//! driven over the wire the way a client drives it: `POST /` with
//! `X-Amz-Target: AWSCognitoIdentityService.<Operation>` on `awsJson1_1`, through the
//! same `MultiServiceRouter` the production router builds.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//! what `CreateIdentityPool` stores is what `DescribeIdentityPool` reports, `GetId` is
//! idempotent per federated login, credentials belong to one identity, a page resumes
//! where the previous one ended, and a resource that is not stored is the modelled
//! `ResourceNotFoundException`.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator holding this service, reached exactly as the production router reaches
/// it: one shared engine, one `MultiServiceRouter`. `pub(crate)` so a sibling suite in
/// this test target drives the service through the same harness rather than a second
/// copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one engine the service is mounted on, so a suite can seed the records ANOTHER
    /// service in that same engine holds — the IAM roles a vend assumes.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation and return its HTTP status with the parsed JSON body (`Null`
    /// for an empty body), signed for [`DEFAULT_REGION`].
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(DEFAULT_REGION, operation, body).await
    }

    /// The same call, signed for `region`: the SigV4 credential scope's Region, which is
    /// the Region the call is served in.
    pub(crate) async fn call_in(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://cognito-identity.{region}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityService.{operation}"),
            )
            // The SigV4 credential scope is what routes the request to this service and
            // names the region its ids are minted in. Signatures are not verified unless
            // the emulator is started with authentication on.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=EMULATORTESTKEY12345/20260101/{region}/cognito-identity/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "operation {operation} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Every event recorded in the account's management-event log of the SAME shared
    /// engine, oldest first, as the JSON document CloudTrail reports in an event's
    /// `CloudTrailEvent`.
    pub(crate) fn recorded_events(&self) -> Vec<Value> {
        self.engine
            .state::<service_common::management_events::ManagementEventLog>()
            .lock()
            .iter()
            .map(|event| event.to_cloudtrail_event_json())
            .collect()
    }

    /// Call an operation that must succeed, returning its output object.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "operation {operation} on {body} expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create a pool and return its minted `IdentityPoolId`.
    pub(crate) async fn pool(&self, name: &str, allow_unauthenticated: bool) -> String {
        let created = self
            .ok(
                "CreateIdentityPool",
                &json!({
                    "IdentityPoolName": name,
                    "AllowUnauthenticatedIdentities": allow_unauthenticated,
                }),
            )
            .await;
        created["IdentityPoolId"]
            .as_str()
            .expect("CreateIdentityPool returns an IdentityPoolId")
            .to_string()
    }

    /// Create a pool that DECLARES `providers` in its federated-provider configuration, and
    /// return its minted `IdentityPoolId`.
    ///
    /// A call that writes a provider's principal-tag attribute map is served only for a
    /// provider the pool's own configuration declares, so a suite that writes one needs the
    /// pool to declare the provider it writes for. Each name is declared under the member
    /// that names a provider of its kind: an Amazon Cognito user pool
    /// (`cognito-idp.<region>.amazonaws.com/<UserPoolId>`) under `CognitoIdentityProviders`,
    /// an ARN under `SamlProviderARNs`, and any other provider name under
    /// `SupportedLoginProviders` — which is modelled `length 0..10`, so past that a name is
    /// declared as the OpenID Connect provider of that issuer instead.
    pub(crate) async fn pool_with_providers(
        &self,
        name: &str,
        allow_unauthenticated: bool,
        providers: &[&str],
    ) -> String {
        let mut request = Map::new();
        request.insert("IdentityPoolName".to_string(), json!(name));
        request.insert(
            "AllowUnauthenticatedIdentities".to_string(),
            json!(allow_unauthenticated),
        );
        let mut user_pools: Vec<Value> = Vec::new();
        let mut saml: Vec<Value> = Vec::new();
        let mut public: Map<String, Value> = Map::new();
        let mut oidc: Vec<Value> = Vec::new();
        for provider in providers {
            if provider.starts_with("cognito-idp.") {
                user_pools.push(json!({"ProviderName": provider, "ClientId": "declaredclient"}));
            } else if provider.starts_with("arn:") {
                saml.push(json!(provider));
            } else if public.len() < 10 {
                public.insert((*provider).to_string(), json!("declared-app-id"));
            } else {
                oidc.push(json!(format!(
                    "arn:aws:iam::{}:oidc-provider/{provider}",
                    engine::emulator_account_id()
                )));
            }
        }
        if !user_pools.is_empty() {
            request.insert("CognitoIdentityProviders".to_string(), json!(user_pools));
        }
        if !saml.is_empty() {
            request.insert("SamlProviderARNs".to_string(), json!(saml));
        }
        if !public.is_empty() {
            request.insert("SupportedLoginProviders".to_string(), Value::Object(public));
        }
        if !oidc.is_empty() {
            request.insert("OpenIdConnectProviderARNs".to_string(), json!(oidc));
        }
        let created = self.ok("CreateIdentityPool", &Value::Object(request)).await;
        created["IdentityPoolId"]
            .as_str()
            .expect("CreateIdentityPool returns an IdentityPoolId")
            .to_string()
    }

    /// Mint an identity with the given `Logins` map (absent when `logins` is `None`).
    pub(crate) async fn identity(&self, pool_id: &str, logins: Option<Value>) -> String {
        let mut request = Map::new();
        request.insert("IdentityPoolId".to_string(), json!(pool_id));
        if let Some(logins) = logins {
            request.insert("Logins".to_string(), logins);
        }
        let got = self.ok("GetId", &Value::Object(request)).await;
        got["IdentityId"]
            .as_str()
            .expect("GetId returns an IdentityId")
            .to_string()
    }

    /// Seed `role_arn` into the IAM store of the SAME shared engine — the record
    /// `iam:CreateRole` writes — carrying `trust_policy` as its
    /// `AssumeRolePolicyDocument`, and answer the ARN the role is stored under.
    ///
    /// A vend assumes the pool's role, so a suite that vends credentials needs the role to
    /// exist where the vend reads it from.
    pub(crate) fn seed_role(&self, role_arn: &str, trust_policy: Value) -> String {
        seed_role(&self.engine, role_arn, trust_policy)
    }

    /// Take the WRITE CLAIM a handler of this service takes while it reconfigures a pool
    /// (`state::claimed_pool_write`), so a suite can drive the state a second writer of that
    /// pool meets instead of racing two requests against the clock. The claim is given back
    /// when the value is dropped.
    pub(crate) fn pool_write_claim(
        &self,
        identity_pool_id: &str,
    ) -> service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::PoolWriteClaim
    {
        let state = service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::service_state(
            &*self.engine,
        )
        .expect("the identity-pool store is on this engine");
        service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::claimed_pool_write(
            &state,
            identity_pool_id,
        )
        .expect("no other request holds this pool's write claim")
    }

    /// A pool whose two roles are configured and seeded: the shape a pool has to be in for
    /// it to be able to vend credentials at all. Answers its `IdentityPoolId`.
    pub(crate) async fn pool_with_roles(&self, name: &str, allow_unauthenticated: bool) -> String {
        let pool_id = self.pool(name, allow_unauthenticated).await;
        let authenticated =
            self.seed_role(&seeded_role_arn(name, "auth"), cognito_trust_policy(&pool_id));
        let unauthenticated = self.seed_role(
            &seeded_role_arn(name, "unauth"),
            cognito_trust_policy(&pool_id),
        );
        self.ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {
                    "authenticated": authenticated,
                    "unauthenticated": unauthenticated,
                },
            }),
        )
        .await;
        pool_id
    }
}

/// The ARN of a role seeded for the pool `pool_name`, in the emulator's own account (the
/// account every role this emulator holds lives in).
pub(crate) fn seeded_role_arn(pool_name: &str, role_type: &str) -> String {
    iam::state::role_arn(
        iam::state::DEFAULT_PATH,
        &format!("{pool_name}-{role_type}"),
    )
}

/// Seed `role_arn` into `engine`'s IAM store, as `iam:CreateRole` records a role, carrying
/// `trust_policy` as its `AssumeRolePolicyDocument`; answer the ARN it is stored under.
///
/// A free function so a suite holding its own engine (rather than this harness) seeds a
/// role the same way instead of keeping a second copy of this.
pub(crate) fn seed_role(
    engine: &engine::InMemoryEngine,
    role_arn: &str,
    trust_policy: Value,
) -> String {
    let role_name = role_arn
        .rsplit('/')
        .next()
        .expect("a role ARN names a role")
        .to_string();
    let role = iam::ops::types::Role {
        path: iam::state::DEFAULT_PATH.to_string(),
        role_name: role_name.clone(),
        // Its own value, not one derived from the ARN, so an assertion that a session's
        // `AssumedRoleId` is built from the id IAM issued cannot pass against a handler
        // that derives an id of its own.
        role_id: format!("AROASEEDED{:0>11}", role_name.len()),
        arn: role_arn.to_string(),
        create_date: aws_smithy_types::DateTime::from_secs(1_767_225_600),
        assume_role_policy_document: Some(trust_policy.to_string()),
        description: None,
        max_session_duration: Some(43_200),
        permissions_boundary: None,
        tags: None,
        role_last_used: None,
        source_role_template: None,
    };
    engine
        .state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>()
        .lock()
        .roles
        .insert(iam::state::name_key(&role_name), role);
    role_arn.to_string()
}

/// A role trust policy that admits an identity pool's users, written the way the Amazon
/// Cognito Developer Guide writes one: the `Federated` Cognito Identity principal, the
/// web-identity assume, and the `:aud` condition restricting the assume to `audience`'s
/// own users.
pub(crate) fn cognito_trust_policy(audience: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": "cognito-identity.amazonaws.com"},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {"StringEquals": {"cognito-identity.amazonaws.com:aud": audience}},
        }],
    })
}

/// `IdentityPoolId` / `IdentityId` are modelled `len 1..55, pattern ^[\w-]+:[0-9a-f-]+$`
/// (`smithy_query(query="operation", operation="CreateIdentityPool")`), i.e. REGION:GUID.
/// Checked by hand rather than with a regex engine: the two halves are a character class
/// each.
fn is_cognito_id(value: &str) -> bool {
    let Some((region, guid)) = value.split_once(':') else {
        return false;
    };
    (1..=55).contains(&value.len())
        && !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && !guid.is_empty()
        && guid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

/// The error code an error response carries (`__type` on the awsJson protocols).
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error response carries a __type: {body}"))
        .to_string()
}

/// The member `name` of a response object, as `Some(value)` when the body carries the key
/// at all and `None` when it is left out. `pub(crate)` so every suite in this target
/// asserts a member's PRESENCE the same way — a distinction indexing (`body["Member"]`,
/// `Null` either way) cannot make.
pub(crate) fn member(response: &Value, name: &str) -> Option<Value> {
    response
        .as_object()
        .unwrap_or_else(|| panic!("a response object, got {response}"))
        .get(name)
        .cloned()
}

/// Whether a JSON map carries any entry — the configurations that carry none are the ones
/// whose member a read leaves out.
pub(crate) fn non_empty(map: &Value) -> bool {
    !map.as_object()
        .unwrap_or_else(|| panic!("a map, got {map}"))
        .is_empty()
}

/// A well-formed `IdentityPoolId` that no pool was ever created with.
pub(crate) const ABSENT_POOL_ID: &str = "us-east-1:00000000-0000-0000-0000-000000000000";

/// The Region every call this harness makes is signed for unless a suite names another.
pub(crate) const DEFAULT_REGION: &str = "us-east-1";
/// A well-formed `IdentityId` that no `GetId` ever minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:11111111-1111-1111-1111-111111111111";

/// RULE: `DescribeIdentityPool` reports a pool exactly as `CreateIdentityPool` stored
/// it — every member, for every shape of pool the model allows — and each pool gets its
/// own minted `REGION:GUID` id.
#[tokio::test]
async fn describe_identity_pool_reports_what_create_identity_pool_stored() {
    let emulator = Emulator::new();
    // One row per member (and per interesting value) `CreateIdentityPoolInput` declares:
    // the catalog comes from
    // `smithy_query(query="operation", operation="CreateIdentityPool")`.
    let cases: Vec<(&str, Value)> = vec![
        (
            "only the required members",
            json!({"IdentityPoolName": "demo-pool", "AllowUnauthenticatedIdentities": false}),
        ),
        (
            "unauthenticated identities allowed",
            json!({"IdentityPoolName": "demo-pool", "AllowUnauthenticatedIdentities": true}),
        ),
        (
            "classic flow enabled",
            json!({
                "IdentityPoolName": "classic",
                "AllowUnauthenticatedIdentities": true,
                "AllowClassicFlow": true,
            }),
        ),
        (
            "classic flow disabled",
            json!({
                "IdentityPoolName": "no-classic",
                "AllowUnauthenticatedIdentities": false,
                "AllowClassicFlow": false,
            }),
        ),
        (
            "two supported login providers",
            json!({
                "IdentityPoolName": "federated",
                "AllowUnauthenticatedIdentities": false,
                "SupportedLoginProviders": {
                    "graph.facebook.com": "1234567890",
                    "accounts.google.com": "client.apps.googleusercontent.com",
                },
            }),
        ),
        (
            "ten supported login providers (the modelled map maximum)",
            json!({
                "IdentityPoolName": "ten-providers",
                "AllowUnauthenticatedIdentities": false,
                "SupportedLoginProviders": {
                    "p0.example.com": "0", "p1.example.com": "1", "p2.example.com": "2",
                    "p3.example.com": "3", "p4.example.com": "4", "p5.example.com": "5",
                    "p6.example.com": "6", "p7.example.com": "7", "p8.example.com": "8",
                    "p9.example.com": "9",
                },
            }),
        ),
        (
            "an empty supported-login-providers map (the modelled minimum)",
            json!({
                "IdentityPoolName": "no-providers",
                "AllowUnauthenticatedIdentities": false,
                "SupportedLoginProviders": {},
            }),
        ),
        (
            "a developer provider name and tags",
            json!({
                "IdentityPoolName": "dev-authenticated",
                "AllowUnauthenticatedIdentities": false,
                "DeveloperProviderName": "my.dev_provider-1",
                "IdentityPoolTags": {"Env": "test", "Owner": "identity-team"},
            }),
        ),
        (
            "OIDC and SAML provider ARNs",
            json!({
                "IdentityPoolName": "byo-providers",
                "AllowUnauthenticatedIdentities": false,
                "OpenIdConnectProviderARNs": [
                    "arn:aws:iam::000000000000:oidc-provider/accounts.google.com",
                    "arn:aws:iam::000000000000:oidc-provider/auth.example.com",
                ],
                "SamlProviderARNs": ["arn:aws:iam::000000000000:saml-provider/Example"],
            }),
        ),
        (
            "a Cognito user pool provider",
            json!({
                "IdentityPoolName": "user-pool-backed",
                "AllowUnauthenticatedIdentities": false,
                "CognitoIdentityProviders": [{
                    "ProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_abcd1234",
                    "ClientId": "1h57kf5cpq17m0eml12eqpq8m2",
                    "ServerSideTokenCheck": true,
                }],
            }),
        ),
        (
            "a name of the minimum modelled length",
            json!({"IdentityPoolName": "a", "AllowUnauthenticatedIdentities": false}),
        ),
        (
            "a name of the maximum modelled length",
            json!({
                "IdentityPoolName": "n".repeat(128),
                "AllowUnauthenticatedIdentities": false,
            }),
        ),
        (
            "a name using every character class the pattern allows",
            json!({
                "IdentityPoolName": "pool +=,.@-_name 1",
                "AllowUnauthenticatedIdentities": false,
            }),
        ),
    ];

    let mut minted = HashSet::new();
    for (case, request) in cases {
        let created = emulator.ok("CreateIdentityPool", &request).await;
        let identity_pool_id = created["IdentityPoolId"].as_str().unwrap_or_default();
        assert!(
            is_cognito_id(identity_pool_id),
            "case {case:?}: expected a minted REGION:GUID IdentityPoolId, got {identity_pool_id:?}"
        );
        assert!(
            minted.insert(identity_pool_id.to_string()),
            "case {case:?}: IdentityPoolId {identity_pool_id:?} was already minted for another pool"
        );

        // Every member the request sent comes back with the value it was sent with.
        for (member, sent) in request.as_object().expect("the request is an object") {
            // With ONE exception: an explicitly empty `SupportedLoginProviders` configures
            // no login provider, and AWS answers the pool the same way whether the member
            // was sent empty or not sent at all, so the member is ABSENT rather than echoed
            // as an empty map (`pool_configuration_rules` asserts that rule in full).
            if member == "SupportedLoginProviders" && sent.as_object().is_some_and(Map::is_empty) {
                assert!(
                    self::member(&created, member).is_none(),
                    "case {case:?}: an explicitly empty SupportedLoginProviders must leave the \
                     member out, got {created}"
                );
                continue;
            }
            assert_eq!(
                &created[member], sent,
                "case {case:?}: CreateIdentityPool answered {member} = {}, expected {sent}",
                created[member]
            );
        }

        // And the stored pool is what a later read reports — the whole record, not just
        // the members this row sent.
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": identity_pool_id}),
            )
            .await;
        assert_eq!(
            described, created,
            "case {case:?}: DescribeIdentityPool reported {described}, expected the created pool {created}"
        );
    }
}

/// RULE: `GetId` resolves a federated login to the one identity linked to it, and mints a
/// fresh identity for every unauthenticated call; the identity is persisted, so
/// `ListIdentities` shows it.
#[tokio::test]
async fn get_id_is_idempotent_per_login_and_fresh_per_unauthenticated_call() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("idempotence", true).await;

    // One row per shape of `Logins` the model allows (`LoginsMap`, len 0..10), plus the
    // unauthenticated call that sends none.
    let logins_cases: Vec<(&str, Option<Value>)> = vec![
        ("one provider", Some(json!({"graph.facebook.com": "token-a"}))),
        (
            "the same provider with another login",
            Some(json!({"graph.facebook.com": "token-b"})),
        ),
        (
            "another provider",
            Some(json!({"accounts.google.com": "token-a"})),
        ),
        (
            "two providers at once",
            Some(json!({"appleid.apple.com": "token-c", "www.amazon.com": "token-d"})),
        ),
        (
            "ten providers (the modelled map maximum)",
            Some(json!({
                "p0.example.com": "0", "p1.example.com": "1", "p2.example.com": "2",
                "p3.example.com": "3", "p4.example.com": "4", "p5.example.com": "5",
                "p6.example.com": "6", "p7.example.com": "7", "p8.example.com": "8",
                "p9.example.com": "9",
            })),
        ),
        ("an empty Logins map", Some(json!({}))),
        ("no Logins member at all", None),
    ];

    let mut unauthenticated = Vec::new();
    let mut federated = Vec::new();
    for (case, logins) in &logins_cases {
        let first = emulator.identity(&pool_id, logins.clone()).await;
        let second = emulator.identity(&pool_id, logins.clone()).await;
        assert!(
            is_cognito_id(&first),
            "case {case:?}: expected a minted REGION:GUID IdentityId, got {first:?}"
        );
        let is_federated = logins
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|map| !map.is_empty());
        if is_federated {
            assert_eq!(
                first, second,
                "case {case:?}: GetId minted {second} for a login already linked to {first}"
            );
            federated.push(first);
        } else {
            assert_ne!(
                first, second,
                "case {case:?}: an unauthenticated GetId returned the same IdentityId twice ({first})"
            );
            unauthenticated.extend([first, second]);
        }
    }

    // Distinct logins are distinct identities.
    let distinct: BTreeSet<&String> = federated.iter().collect();
    assert_eq!(
        distinct.len(),
        federated.len(),
        "two different federated logins shared an IdentityId: {federated:?}"
    );

    // Every identity minted above is persisted in the pool, and nothing else is.
    let mut expected: BTreeSet<String> = federated.into_iter().collect();
    expected.extend(unauthenticated);
    let listed = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": &pool_id, "MaxResults": 60}),
        )
        .await;
    let listed_ids: BTreeSet<String> = listed["Identities"]
        .as_array()
        .expect("ListIdentities returns an Identities list")
        .iter()
        .map(|identity| identity["IdentityId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed_ids, expected,
        "ListIdentities reported {listed_ids:?}, expected the identities GetId minted {expected:?}"
    );

    // An identity carries the provider names it was linked with, and its timestamps.
    let described: Vec<&Value> = listed["Identities"].as_array().unwrap().iter().collect();
    for identity in described {
        let logins = identity["Logins"]
            .as_array()
            .unwrap_or_else(|| panic!("an IdentityDescription carries Logins: {identity}"));
        assert!(
            logins.iter().all(|login| login.is_string()),
            "an IdentityDescription's Logins must be provider names, got {identity}"
        );
        for member in ["CreationDate", "LastModifiedDate"] {
            assert!(
                identity[member].is_number(),
                "an IdentityDescription carries {member}, got {identity}"
            );
        }
    }
}

/// RULE: a pool that does not allow unauthenticated identities refuses a `GetId` carrying
/// no login (400 `NotAuthorizedException`), and allows one that carries a login.
#[tokio::test]
async fn get_id_refuses_an_unauthenticated_call_when_the_pool_forbids_it() {
    let emulator = Emulator::new();
    let closed = emulator.pool("closed-pool", false).await;
    let open = emulator.pool("open-pool", true).await;

    for (case, logins) in [("no Logins member", None), ("an empty Logins map", Some(json!({})))] {
        let mut request = Map::new();
        request.insert("IdentityPoolId".to_string(), json!(&closed));
        if let Some(logins) = logins {
            request.insert("Logins".to_string(), logins);
        }
        let (status, body) = emulator.call("GetId", &Value::Object(request)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an unauthenticated GetId, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException, got {body}"
        );
    }

    // The same pool still serves a federated caller, and an open pool serves both.
    for (case, pool_id) in [("closed pool", &closed), ("open pool", &open)] {
        let identity = emulator
            .identity(pool_id, Some(json!({"graph.facebook.com": "token"})))
            .await;
        assert!(
            is_cognito_id(&identity),
            "case {case:?}: a federated GetId must mint an identity, got {identity:?}"
        );
    }
    let unauthenticated = emulator.identity(&open, None).await;
    assert!(
        is_cognito_id(&unauthenticated),
        "an open pool must serve an unauthenticated GetId, got {unauthenticated:?}"
    );
}

/// RULE: `GetCredentialsForIdentity` issues credentials that belong to the one identity
/// asked for, with an expiration in the future, and hands the same session back while it
/// is still valid.
#[tokio::test]
async fn credentials_are_per_identity_and_stable_while_valid() {
    let emulator = Emulator::new();
    // A vend is a session of the pool's role, so the pool is configured with the roles it
    // vends; what the session STANDS FOR is `super::vended_role_session`'s own rule.
    let pool_id = emulator.pool_with_roles("credentials", true).await;

    // One row per way of reaching an identity: unauthenticated, and federated with one
    // and with several providers.
    let cases: Vec<(&str, Option<Value>)> = vec![
        ("an unauthenticated identity", None),
        (
            "a federated identity",
            Some(json!({"graph.facebook.com": "token-1"})),
        ),
        (
            "an identity with two providers",
            Some(json!({"accounts.google.com": "token-2", "appleid.apple.com": "token-3"})),
        ),
    ];

    let mut access_keys = HashSet::new();
    let mut secrets = HashSet::new();
    for (case, logins) in cases {
        let identity_id = emulator.identity(&pool_id, logins.clone()).await;
        // A vend for an identity a login has been linked to is authorized by a login of it,
        // so each row presents the very logins it was minted with; a guest identity has
        // none, and its id is the whole of the unauthenticated exchange.
        let vend = match &logins {
            Some(logins) => json!({"IdentityId": &identity_id, "Logins": logins}),
            None => json!({"IdentityId": &identity_id}),
        };
        let issued = emulator.ok("GetCredentialsForIdentity", &vend).await;
        assert_eq!(
            issued["IdentityId"].as_str(),
            Some(identity_id.as_str()),
            "case {case:?}: expected the credentials to name {identity_id}, got {issued}"
        );
        let credentials = &issued["Credentials"];
        for member in ["AccessKeyId", "SecretKey", "SessionToken"] {
            let value = credentials[member].as_str().unwrap_or_default();
            assert!(
                !value.is_empty(),
                "case {case:?}: Credentials.{member} must be non-empty, got {issued}"
            );
        }
        let expiration = credentials["Expiration"].as_f64().unwrap_or_default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        assert!(
            expiration > now,
            "case {case:?}: Credentials.Expiration {expiration} must be in the future (now {now})"
        );

        // No two identities share a credential.
        let access_key = credentials["AccessKeyId"].as_str().unwrap().to_string();
        let secret = credentials["SecretKey"].as_str().unwrap().to_string();
        assert!(
            access_keys.insert(access_key),
            "case {case:?}: an AccessKeyId was issued to two identities: {issued}"
        );
        assert!(
            secrets.insert(secret),
            "case {case:?}: a SecretKey was issued to two identities: {issued}"
        );

        // Asked again inside the validity window, the identity has the same session.
        let again = emulator.ok("GetCredentialsForIdentity", &vend).await;
        assert_eq!(
            again, issued,
            "case {case:?}: a second call inside the validity window answered {again}, expected {issued}"
        );
    }
}

/// RULE: a page of `ListIdentities` carries at most `MaxResults` identities in the order
/// they were minted, and its `NextToken` resumes exactly where it ended — absent on the
/// last page.
#[tokio::test]
async fn list_identities_pages_through_every_identity_exactly_once() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("pagination", true).await;

    let mut minted = Vec::new();
    for index in 0..7 {
        minted.push(
            emulator
                .identity(&pool_id, Some(json!({"p.example.com": format!("user-{index}")})))
                .await,
        );
    }

    // One row per page size: the modelled bounds (1 and 60), a size that divides the
    // count, one that does not, and one larger than the count.
    for page_size in [1, 2, 3, 7, 8, 60] {
        let mut seen = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = Map::new();
            request.insert("IdentityPoolId".to_string(), json!(&pool_id));
            request.insert("MaxResults".to_string(), json!(page_size));
            if let Some(token) = &next_token {
                request.insert("NextToken".to_string(), json!(token));
            }
            let page = emulator.ok("ListIdentities", &Value::Object(request)).await;
            pages += 1;
            assert!(
                pages <= minted.len() + 1,
                "page size {page_size}: pagination did not terminate after {pages} pages"
            );
            assert_eq!(
                page["IdentityPoolId"].as_str(),
                Some(pool_id.as_str()),
                "page size {page_size}: a page must name its pool, got {page}"
            );
            let identities = page["Identities"]
                .as_array()
                .unwrap_or_else(|| panic!("page size {page_size}: no Identities in {page}"));
            assert!(
                identities.len() <= page_size,
                "page size {page_size}: a page carried {} identities, expected at most {page_size}",
                identities.len()
            );
            seen.extend(
                identities
                    .iter()
                    .map(|identity| identity["IdentityId"].as_str().unwrap_or_default().to_string()),
            );
            next_token = page["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                !identities.is_empty(),
                "page size {page_size}: a page with a NextToken must not be empty: {page}"
            );
        }
        assert_eq!(
            seen, minted,
            "page size {page_size}: paging reported {seen:?}, expected every identity once in mint order {minted:?}"
        );
    }

    // A token this service did not issue for THIS pool is a bad input parameter, not an
    // empty page.
    let other_pool = emulator.pool("other-pagination", true).await;
    for index in 0..3 {
        emulator
            .identity(&other_pool, Some(json!({"p.example.com": format!("other-{index}")})))
            .await;
    }
    let other_page = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": &other_pool, "MaxResults": 1}),
        )
        .await;
    let other_token = other_page["NextToken"]
        .as_str()
        .expect("a pool with three identities and MaxResults 1 has a NextToken")
        .to_string();
    for (case, token) in [
        ("not hex", "not-a-token".to_string()),
        ("hex of an id from no pool", "61626364".to_string()),
        ("a token minted for another pool", other_token),
    ] {
        let (status, body) = emulator
            .call(
                "ListIdentities",
                &json!({"IdentityPoolId": &pool_id, "MaxResults": 2, "NextToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a foreign NextToken, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
    }
}

/// RULE: logins presented together name one account — every one of them is linked to the
/// identity resolved for the call — and logins already linked to different identities
/// cannot be merged by presenting them together (`ResourceConflictException`).
#[tokio::test]
async fn logins_presented_together_are_linked_to_one_identity() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("implicit-linking", true).await;

    // A first login mints an identity; presenting it beside a second login links the
    // second to the same identity, so the second alone resolves to it afterwards.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a second provider",
            json!({"graph.facebook.com": "user-1"}),
            json!({"graph.facebook.com": "user-1", "accounts.google.com": "user-1"}),
        ),
        (
            "the same provider with another login",
            json!({"appleid.apple.com": "user-2"}),
            json!({"appleid.apple.com": "user-2", "www.amazon.com": "user-2"}),
        ),
        (
            "two further providers at once",
            json!({"api.twitter.com": "user-3"}),
            json!({
                "api.twitter.com": "user-3",
                "p1.example.com": "user-3",
                "p2.example.com": "user-3",
            }),
        ),
    ];

    for (case, first_logins, together) in cases {
        let identity = emulator.identity(&pool_id, Some(first_logins.clone())).await;
        let linked = emulator.identity(&pool_id, Some(together.clone())).await;
        assert_eq!(
            linked, identity,
            "case {case:?}: presenting {together} beside an already linked login expected {identity}, got {linked}"
        );

        // Each login presented together now resolves to that identity on its own.
        for (provider, login) in together.as_object().expect("the logins are an object") {
            let alone = emulator
                .identity(&pool_id, Some(json!({provider: login})))
                .await;
            assert_eq!(
                alone, identity,
                "case {case:?}: login {provider}={login} alone expected the linked identity {identity}, got {alone}"
            );
        }

        // And the identity records every provider it is linked to.
        let listed = emulator
            .ok(
                "ListIdentities",
                &json!({"IdentityPoolId": &pool_id, "MaxResults": 60}),
            )
            .await;
        let described = listed["Identities"]
            .as_array()
            .expect("Identities is a list")
            .iter()
            .find(|candidate| candidate["IdentityId"].as_str() == Some(identity.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {identity} is missing from {listed}"));
        let mut providers: Vec<&str> = described["Logins"]
            .as_array()
            .expect("Logins is a list")
            .iter()
            .map(|login| login.as_str().unwrap_or_default())
            .collect();
        providers.sort_unstable();
        let mut expected: Vec<&str> = together
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        expected.sort_unstable();
        assert_eq!(
            providers, expected,
            "case {case:?}: {identity} is linked to {providers:?}, expected {expected:?}"
        );
    }

    // An identity is linked to at most this emulator's own ceiling of providers, reached
    // over as many calls as it takes (a `Logins` map carries ten at a time), and the
    // provider past it is refused rather than growing the stored record. The ceiling is
    // read from the crate, so this asserts the rule and not one number.
    const ANCHOR: &str = "c0.example.com";
    let cap = service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::MAX_LOGIN_PROVIDERS_PER_IDENTITY;
    let capped = emulator
        .identity(&pool_id, Some(json!({ANCHOR: "cap"})))
        .await;
    let mut linked = 1;
    let mut next = 1;
    while linked < cap {
        // Every call presents a provider the identity already has, so it resolves to that
        // identity, plus as many new ones as the map has room for.
        let batch = (cap - linked).min(9);
        let mut request = Map::new();
        request.insert(ANCHOR.to_string(), json!("cap"));
        for _ in 0..batch {
            request.insert(format!("c{next}.example.com"), json!("cap"));
            next += 1;
        }
        let same = emulator
            .identity(&pool_id, Some(Value::Object(request)))
            .await;
        assert_eq!(
            same, capped,
            "linking {batch} more providers expected the identity {capped}, got {same}"
        );
        linked += batch;
    }
    let past_the_cap = json!({ANCHOR: "cap", format!("c{next}.example.com"): "cap"});
    let (status, body) = emulator.call(
        "GetId",
        &json!({"IdentityPoolId": &pool_id, "Logins": past_the_cap}),
    )
    .await;
    assert_eq!(
        status, 400,
        "linking provider {} past the ceiling of {cap} to {capped} expected HTTP 400, got {status} with {body}",
        next
    );
    assert!(
        error_code(&body).contains("LimitExceededException"),
        "linking a provider past the ceiling expected LimitExceededException, got {body}"
    );

    // A provider name and a login value are distinct key parts: two logins that differ
    // only in where a colon falls are two different federated users, not one.
    let colon_cases: Vec<(&str, Value)> = vec![
        ("a colon inside the provider name", json!({"host.example.com:x": "y"})),
        ("a colon inside the login value", json!({"host.example.com": "x:y"})),
    ];
    let mut colon_identities: Vec<String> = Vec::new();
    for (case, logins) in colon_cases {
        let first = emulator.identity(&pool_id, Some(logins.clone())).await;
        let again = emulator.identity(&pool_id, Some(logins.clone())).await;
        assert_eq!(
            first, again,
            "case {case:?}: {logins} must resolve to one identity, got {first} then {again}"
        );
        colon_identities.push(first);
    }
    assert_ne!(
        colon_identities[0], colon_identities[1],
        "logins differing only in where the colon falls must be distinct identities, got {colon_identities:?}"
    );

    // Two logins each already linked to their own identity are a conflict, not a merge.
    let first = emulator
        .identity(&pool_id, Some(json!({"left.example.com": "a"})))
        .await;
    let second = emulator
        .identity(&pool_id, Some(json!({"right.example.com": "b"})))
        .await;
    assert_ne!(first, second, "two unrelated logins must be two identities");
    let (status, body) = emulator
        .call(
            "GetId",
            &json!({
                "IdentityPoolId": &pool_id,
                "Logins": {"left.example.com": "a", "right.example.com": "b"},
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "merging two linked identities expected HTTP 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ResourceConflictException"),
        "merging two linked identities expected ResourceConflictException, got {body}"
    );
}

/// RULE: a request this slice cannot answer faithfully is refused with the modelled
/// error, never with a session that ignores the member — a `CustomRoleArn` named by an
/// exchange whose identity provider offered no choice of role is a bad input parameter, and
/// a pool that names no role at all leaves the vend no role it could be a session of.
#[tokio::test]
async fn a_custom_role_arn_is_refused_while_the_pool_has_no_roles() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("custom-role", true).await;
    let identity_id = emulator.identity(&pool_id, None).await;

    for (case, custom_role_arn) in [
        (
            "an authenticated role",
            "arn:aws:iam::000000000000:role/Cognito_poolAuth_Role",
        ),
        (
            "a mapped role",
            "arn:aws:iam::000000000000:role/some-other-mapped-role",
        ),
    ] {
        let (status, body) = emulator
            .call(
                "GetCredentialsForIdentity",
                &json!({"IdentityId": &identity_id, "CustomRoleArn": custom_role_arn}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a CustomRoleArn, got {status} with {body}"
        );
        // This pool holds no role mapping and no SAML provider, and the exchange presents
        // no login at all, so there were never several roles to choose between: naming one
        // is the bad parameter the service says it is.
        assert!(
            error_code(&body).contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        assert_eq!(
            member(&body, "message").and_then(|message| message.as_str().map(str::to_string)),
            Some("Only SAML providers and providers with RoleMappings support custom role ARN."
                .to_string()),
            "case {case:?}: expected the fixed custom-role message, got {body}"
        );
    }

    // Without one, the pool still has no role for the request — "the identity pool has no
    // unauthenticated role configured" (`InvalidIdentityPoolConfigurationException`, this
    // service's model) — so there is no session to vend either.
    let (status, body) = emulator
        .call(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id}),
        )
        .await;
    assert_eq!(
        status, 400,
        "a vend from a pool with no role expected HTTP 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidIdentityPoolConfigurationException"),
        "a vend from a pool with no role expected \
         InvalidIdentityPoolConfigurationException, got {body}"
    );
    assert_eq!(
        member(&body, "Credentials"),
        None,
        "a refused vend carries no Credentials, got {body}"
    );
}

/// RULE: every minted id has the modelled `REGION:GUID` shape, names its pool's region,
/// and no two resources ever share one.
#[tokio::test]
async fn minted_ids_are_distinct_and_well_formed() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("minting", true).await;
    let region = pool_id
        .split_once(':')
        .expect("a pool id is REGION:GUID")
        .0
        .to_string();
    // The request is signed for us-east-1, so that is the region the pool and every
    // identity in it is minted for.
    assert_eq!(
        region, "us-east-1",
        "the pool must be minted in the region its request is signed for, got {pool_id}"
    );

    let mut seen = HashSet::new();
    seen.insert(pool_id.clone());
    for mint in 0..200 {
        let identity_id = emulator.identity(&pool_id, None).await;
        assert!(
            is_cognito_id(&identity_id),
            "mint {mint}: expected a REGION:GUID IdentityId, got {identity_id:?}"
        );
        assert!(
            identity_id.starts_with(&format!("{region}:")),
            "mint {mint}: expected an id in the pool's region {region}, got {identity_id:?}"
        );
        // A GUID is 32 hex digits in the 8-4-4-4-12 layout.
        let guid = identity_id.split_once(':').unwrap().1;
        let groups: Vec<usize> = guid.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "mint {mint}: expected an 8-4-4-4-12 GUID, got {guid:?}"
        );
        assert!(
            seen.insert(identity_id.clone()),
            "mint {mint}: id {identity_id} was minted twice"
        );
    }
}

/// RULE: an operation naming a resource this service does not hold answers the modelled
/// `ResourceNotFoundException` — for every operation that names one.
#[tokio::test]
async fn an_unknown_resource_is_not_found() {
    let emulator = Emulator::new();
    let cases: Vec<(&str, Value)> = vec![
        (
            "DescribeIdentityPool",
            json!({"IdentityPoolId": ABSENT_POOL_ID}),
        ),
        (
            "DeleteIdentityPool",
            json!({"IdentityPoolId": ABSENT_POOL_ID}),
        ),
        ("GetId", json!({"IdentityPoolId": ABSENT_POOL_ID})),
        (
            "ListIdentities",
            json!({"IdentityPoolId": ABSENT_POOL_ID, "MaxResults": 10}),
        ),
        (
            "GetCredentialsForIdentity",
            json!({"IdentityId": ABSENT_IDENTITY_ID}),
        ),
    ];

    for (operation, request) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "operation {operation}: expected HTTP 400 for an absent resource, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "operation {operation}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// RULE: an id outside the modelled `len 1..55, pattern ^[\w-]+:[0-9a-f-]+$` never
/// reaches the operation — the constraint-validation layer ahead of it refuses the
/// request with 400 `ValidationException`, naming the member and the constraint the
/// value failed — while an id that satisfies the constraint, including at its length
/// bound, is looked up and simply not found.
#[tokio::test]
async fn an_id_outside_its_modelled_constraint_is_a_validation_error() {
    let emulator = Emulator::new();
    // 18 + 1 + 36 = 55 characters: the maximum the model allows.
    let at_max_length = format!("{}:{}", "r".repeat(18), "0".repeat(36));
    // One character past it.
    let past_max_length = format!("{}:{}", "r".repeat(19), "0".repeat(36));

    let refused: Vec<(&str, String)> = vec![
        ("no region separator", "us-east-1".to_string()),
        ("empty", String::new()),
        (
            "a GUID that is not lowercase hex",
            "us-east-1:ZZZZZZZZ-0000-0000-0000-000000000000".to_string(),
        ),
        ("an empty GUID half", "us-east-1:".to_string()),
        ("one character past the maximum length", past_max_length),
    ];
    for (case, identity_pool_id) in refused {
        let (status, body) = emulator
            .call(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": identity_pool_id}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {identity_pool_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException for {identity_pool_id:?}, got {body}"
        );
    }

    // A required member that is absent is the same class of refusal.
    let (status, body) = emulator.call("DescribeIdentityPool", &json!({})).await;
    assert_eq!(
        status, 400,
        "an absent IdentityPoolId expected HTTP 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "an absent IdentityPoolId expected ValidationException, got {body}"
    );

    // At the bound the constraint is satisfied, so the answer is about the resource.
    let (status, body) = emulator
        .call(
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": &at_max_length}),
        )
        .await;
    assert_eq!(
        status, 400,
        "an id at the maximum modelled length ({at_max_length}) expected HTTP 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "an id at the maximum modelled length ({at_max_length}) expected the resource to be \
         looked up and not found, got {body}"
    );
}

/// RULE: deleting a pool removes the pool and everything that belongs to it, and leaves
/// every other pool alone.
#[tokio::test]
async fn deleting_a_pool_removes_it_with_its_identities() {
    let emulator = Emulator::new();
    let doomed = emulator.pool("doomed", true).await;
    let survivor = emulator.pool("survivor", true).await;
    let doomed_identity = emulator.identity(&doomed, None).await;
    let surviving_identity = emulator.identity(&survivor, None).await;

    emulator
        .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &doomed}))
        .await;

    // Everything that named the deleted pool, or one of its identities, is now absent.
    let gone: Vec<(&str, Value)> = vec![
        ("DescribeIdentityPool", json!({"IdentityPoolId": &doomed})),
        (
            "ListIdentities",
            json!({"IdentityPoolId": &doomed, "MaxResults": 10}),
        ),
        ("GetId", json!({"IdentityPoolId": &doomed})),
        (
            "GetCredentialsForIdentity",
            json!({"IdentityId": &doomed_identity}),
        ),
        // A second delete has nothing left to delete.
        ("DeleteIdentityPool", json!({"IdentityPoolId": &doomed})),
    ];
    for (operation, request) in gone {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "operation {operation} after the delete: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "operation {operation} after the delete: expected ResourceNotFoundException, got {body}"
        );
    }

    // The other pool is untouched.
    let described = emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": &survivor}))
        .await;
    assert_eq!(
        described["IdentityPoolName"].as_str(),
        Some("survivor"),
        "the surviving pool must still be stored, got {described}"
    );
    let listed = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": &survivor, "MaxResults": 10}),
        )
        .await;
    let ids: Vec<&str> = listed["Identities"]
        .as_array()
        .expect("Identities is a list")
        .iter()
        .map(|identity| identity["IdentityId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec![surviving_identity.as_str()],
        "the surviving pool must keep its identity, got {listed}"
    );
}
