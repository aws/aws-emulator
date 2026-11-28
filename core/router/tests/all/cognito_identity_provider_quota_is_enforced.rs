// SPDX-License-Identifier: CC0-1.0

//! Two services and one engine: the per-identity-pool count of Amazon Cognito user pool
//! providers that Service Quotas publishes for `cognito-identity` is the limit the Amazon
//! Cognito Federated Identities service ENFORCES.
//!
//! RULE: a limit a service enforces on the account's resources is published in the Service
//! Quotas catalog of that service's own `ServiceCode`, under the code AWS mints for it, so a
//! client that reads its limits before provisioning finds the number the write it is about
//! to make will be judged against — and a write past that number is refused with the error
//! the operation declares for a limit of the resource (`LimitExceededException`), storing
//! nothing.
//!
//! Only a cross-service suite can pin this: the published quota is the Service Quotas
//! catalog's and the enforcement is the identity service's. Both services are mounted on ONE
//! engine, exactly as `core/router/src/lib.rs` mounts them.
//!
//! The rows span what the rule generalises over: both ways a client reads the quota (the
//! whole listing it enumerates and the single lookup by code), the write that fills the last
//! slot, the write one past it, both operations that carry the configuration
//! (`CreateIdentityPool` and `UpdateIdentityPool`), and what the store holds after a refusal.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix Service Quotas routes on (that crate's `OPERATIONS.md`).
const QUOTAS_TARGET_PREFIX: &str = "ServiceQuotasV20190624";

/// The `X-Amz-Target` prefix the identity-pool service routes on (that crate's
/// `OPERATIONS.md`).
const IDENTITY_TARGET_PREFIX: &str = "AWSCognitoIdentityService";

/// The quota under test: AWS's `ServiceCode` for Amazon Cognito Federated Identities and
/// the `QuotaCode` it publishes the per-pool user-pool-provider count under.
const SERVICE_CODE: &str = "cognito-identity";
const QUOTA_CODE: &str = "L-14403AD5";

/// The name AWS publishes that quota under, as the recorded default-quota listing of this
/// service code states it.
const QUOTA_NAME: &str = "User pool providers per identity pool";

/// The refusal a configuration past the quota earns, and the status this service answers
/// every client error with.
const LIMIT_EXCEEDED: &str = "LimitExceededException";
const REFUSAL_STATUS: u16 = 400;

/// The highest published count this suite will provision up to, so a quota raised beyond
/// anything a fixture needs fails the test instead of filling the store.
const MOST_PROVIDERS_PROVISIONED: usize = 200;

/// Both services on one engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These callers sign nothing, so they must not be refused for it; the credential
        // scope below is what routes each request and names its Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_servicequotas_servicequotasv20190624::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// One `awsJson1_1` call, by target prefix and signing name.
    async fn call(
        &self,
        target_prefix: &str,
        signing_name: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Service Quotas call that must succeed.
    async fn quotas_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, payload) = self
            .call(QUOTAS_TARGET_PREFIX, "servicequotas", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must succeed, got {status} with {payload}"
        );
        payload
    }

    /// One identity-pool call.
    async fn identity(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call(IDENTITY_TARGET_PREFIX, "cognito-identity", operation, body)
            .await
    }

    /// One identity-pool call that must succeed.
    async fn identity_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, payload) = self.identity(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must succeed, got {status} with {payload}"
        );
        payload
    }
}

/// The modelled error code an `awsJson1_1` error envelope carries in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `CognitoIdentityProviders` list naming `count` Amazon Cognito user pools, each its own
/// pool with its own app client — the provider name form the identity service documents for
/// a user pool (`cognito-idp.<region>.amazonaws.com/<UserPoolId>`).
fn user_pool_providers(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|position| {
                json!({
                    "ProviderName": format!(
                        "cognito-idp.{REGION}.amazonaws.com/{REGION}_quotapool{position:04}"
                    ),
                    "ClientId": format!("client{position:04}"),
                })
            })
            .collect(),
    )
}

/// How many user pool providers a pool reports.
fn configured_providers(pool: &Value) -> usize {
    pool["CognitoIdentityProviders"]
        .as_array()
        .map_or(0, Vec::len)
}

#[tokio::test]
async fn the_published_user_pool_provider_quota_is_the_limit_the_identity_pool_enforces() {
    let emulator = Emulator::new();

    // The first way a client reads its limits: the whole published catalog of the service
    // code, which is what it enumerates when it holds no code of its own.
    let listing = emulator
        .quotas_ok(
            "the published catalog",
            "ListAWSDefaultServiceQuotas",
            &json!({ "ServiceCode": SERVICE_CODE, "MaxResults": 100 }),
        )
        .await;
    let rows = listing["Quotas"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing carries a Quotas list, got {listing}"))
        .clone();
    let listed = rows
        .iter()
        .find(|quota| quota["QuotaName"].as_str() == Some(QUOTA_NAME))
        .unwrap_or_else(|| {
            panic!("case {QUOTA_NAME:?}: the listing of {SERVICE_CODE} must publish it, got {rows:?}")
        })
        .clone();
    assert_eq!(
        listed["QuotaCode"].as_str(),
        Some(QUOTA_CODE),
        "case {QUOTA_NAME:?}: it must be keyed on the code AWS mints for it, got {listed}"
    );

    // The second way: the single lookup by that code, which has to answer the same record.
    let read = emulator
        .quotas_ok(
            "the single lookup",
            "GetAWSDefaultServiceQuota",
            &json!({ "ServiceCode": SERVICE_CODE, "QuotaCode": QUOTA_CODE }),
        )
        .await;
    let published = read["Quota"].clone();
    assert_eq!(
        published, listed,
        "case {QUOTA_CODE:?}: the single read must answer the record the listing carries, got \
         {published}"
    );
    assert_eq!(
        published["Adjustable"].as_bool(),
        Some(true),
        "case {QUOTA_CODE:?}: AWS lets an increase be requested for it, got {published}"
    );
    let value = published["Value"]
        .as_f64()
        .unwrap_or_else(|| panic!("the quota carries a numeric Value, got {published}"));
    assert!(
        value >= 1.0 && value <= MOST_PROVIDERS_PROVISIONED as f64,
        "the published count must be a provisionable number of providers, got {value}"
    );
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let allowed = value as usize;

    // A create naming exactly that many providers is served, and the pool reports them all.
    let created = emulator
        .identity_ok(
            "a create at the quota",
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "provider-quota-pool",
                "AllowUnauthenticatedIdentities": false,
                "CognitoIdentityProviders": user_pool_providers(allowed),
            }),
        )
        .await;
    assert_eq!(
        configured_providers(&created),
        allowed,
        "case a create at the quota: the pool must hold every provider it was created with, \
         got {created}"
    );
    let identity_pool_id = created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("a created pool carries an id, got {created}"))
        .to_string();

    // One provider past the quota is refused, on the create that mints a pool and on the
    // update that reconfigures one, and neither stores anything.
    let past = user_pool_providers(allowed + 1);
    let writes = [
        (
            "a create past the quota",
            "CreateIdentityPool",
            json!({
                "IdentityPoolName": "provider-quota-overflow",
                "AllowUnauthenticatedIdentities": false,
                "CognitoIdentityProviders": past.clone(),
            }),
        ),
        (
            "an update past the quota",
            "UpdateIdentityPool",
            json!({
                "IdentityPoolId": &identity_pool_id,
                "IdentityPoolName": "provider-quota-pool",
                "AllowUnauthenticatedIdentities": false,
                "CognitoIdentityProviders": past.clone(),
            }),
        ),
    ];
    for (case, operation, request) in writes {
        let (status, payload) = emulator.identity(operation, &request).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (REFUSAL_STATUS, LIMIT_EXCEEDED),
            "case {case:?}: expected the published quota to refuse it, got {status} with \
             {payload}"
        );
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>");
        assert_eq!(
            message,
            format!("Cognito Sign In Providers per Identity pool limit of {allowed} reached."),
            "case {case:?}: the refusal must state the limit it was judged against, got \
             {message:?}"
        );
    }

    // The refused update left the pool exactly as it was.
    let described = emulator
        .identity_ok(
            "the pool after the refusals",
            "DescribeIdentityPool",
            &json!({ "IdentityPoolId": &identity_pool_id }),
        )
        .await;
    assert_eq!(
        configured_providers(&described),
        allowed,
        "case the pool after the refusals: a refused write must leave the stored providers \
         alone, got {described}"
    );

    // And the refused create minted nothing: no pool of the account answers to its name.
    let pools = emulator
        .identity_ok(
            "the account's pools",
            "ListIdentityPools",
            &json!({ "MaxResults": 60 }),
        )
        .await;
    let minted = pools["IdentityPools"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing carries IdentityPools, got {pools}"))
        .iter()
        .any(|pool| pool["IdentityPoolName"] == json!("provider-quota-overflow"));
    assert!(
        !minted,
        "case a create past the quota: it must store no pool, got {pools}"
    );
}
