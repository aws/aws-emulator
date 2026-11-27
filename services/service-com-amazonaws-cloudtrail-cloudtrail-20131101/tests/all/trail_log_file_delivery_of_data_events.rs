// SPDX-License-Identifier: CC0-1.0

//! What a LOGGING trail delivers for the account's DATA events — the calls on the CONTENTS
//! of a resource, as against the control-plane calls the neighbouring suites cover.
//!
//! Three services are mounted on ONE engine, because the behaviour spans them: CloudTrail
//! holds the trail and decides the delivery, Amazon Cognito Identity serves the identity
//! data plane, and the object store is where the log files land. Every call goes over the
//! wire the way a client makes it — the data-plane calls UNSIGNED, which is how an end user
//! holding no AWS credential makes them — and the bucket is created, policied, listed and
//! read through Amazon S3's OWN API over the same in-process callback a handler uses, so a
//! delivered log file counts only if the object store itself answers with it.
//!
//! The rule under test: while a trail is logging, every served call an ADVANCED event
//! selector of that trail admits is delivered into the trail's bucket under the documented
//! key layout — a DATA event included, which such a selector admits by naming the `Data`
//! category and the TYPE of the resource the call acted on. Each delivered record carries
//! the serving service as its `eventSource`, the operation as its `eventName`,
//! `eventCategory` `Data` with `managementEvent` false, the resource it acted on by type and
//! ARN, the id the CALLER was answered under as its `requestID`, the redaction literal in
//! place of every secret the call carried, and the identity the request was signed with — or
//! the unknown principal, for a request that carried no credential at all. A data event is
//! NOT event history: `LookupEvents` reports none of them.
//!
//! The rows span what the rule generalises over rather than the one call that motivated it:
//! which data-plane operation served the call, whether the call was served or refused,
//! whether it was signed or anonymous, and — in the second case — which selectors a trail
//! carries, since a trail delivers a data event only while one of its selectors admits it.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::emulator::{cloudtrail_bucket_policy, TARGET_PREFIX};

/// The Region every call below is signed for (and the Region an unsigned call is served in),
/// which is the Region the trail is homed in and so the Region its log files land under.
const REGION: &str = "us-east-1";

/// Amazon Cognito Identity's `X-Amz-Target` prefix (that service's model declares the
/// service shape `AWSCognitoIdentityService`) and its `sigv4Name`.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityService";
const COGNITO_SIGNING_NAME: &str = "cognito-identity";

/// The `eventSource` CloudTrail stamps on an Amazon Cognito Identity record
/// (`smithy_query(query="service_info")` on that service -> `endpointPrefix:
/// cognito-identity`).
const COGNITO_EVENT_SOURCE: &str = "cognito-identity.amazonaws.com";

/// The `eventCategory` of a data event, and of a control-plane one.
const DATA_CATEGORY: &str = "Data";
const MANAGEMENT_CATEGORY: &str = "Management";

/// The resource type an identity pool is named by in a record and in a selector's
/// `resources.type` field.
const IDENTITY_POOL_TYPE: &str = "AWS::Cognito::IdentityPool";

/// Another data-event resource type, for the selector that admits a class this account never
/// serves (`DataResource$Type` lists it among the basic types).
const S3_OBJECT_TYPE: &str = "AWS::S3::Object";

/// What CloudTrail writes in place of a value it withholds
/// (`service_common::management_events::HIDDEN_SENSITIVE_VALUE`).
const HIDDEN: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

/// `userIdentity.type` of a call that carried no credential at all.
const UNKNOWN_PRINCIPAL: &str = "Unknown";

/// The header a response names the request it answers in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The member a log file's document holds its records in.
const RECORDS_MEMBER: &str = "Records";

/// The first segment of a log-file key under the trail's own prefix, the segment naming the
/// service whose files it holds, and what a log file's name ends with.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";
const LOG_FILE_SERVICE_SEGMENT: &str = "CloudTrail";
const LOG_FILE_SUFFIX: &str = ".json.gz";

/// The public login provider the fixture pool declares, and the token presented under it.
/// The token is a SECRET by the model (`LoginsMap`'s value targets the `@sensitive`
/// `IdentityProviderToken`), so no delivered record may carry it.
const LOGIN_PROVIDER: &str = "graph.facebook.com";
const LOGIN_TOKEN: &str = "provider-token-must-not-be-delivered";

/// The developer provider the fixture pool declares, and the developer user identifier the
/// backend exchange presents under it.
const DEVELOPER_PROVIDER: &str = "trail-data-provider";
const DEVELOPER_USER: &str = "developer-user-0";

/// The access key id of the ROLE SESSION the signed data-plane call is made with, and the
/// role that issued it: what the record of that call reports as its `userIdentity`.
const SESSION_KEY_ID: &str = "ASIATRAILDATAEVENTS1";
const SESSION_ROLE: &str = "identity-pool-backend";

/// One answer as its caller reads it: the parsed body and the id the response named the
/// request by.
struct Answered {
    body: Value,
    request_id: String,
}

/// CloudTrail, Amazon Cognito Identity and the object store over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; a credential
        // scope is still carried where the client sends one, because it is what names the
        // Region and the caller.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            callback,
            engine,
        }
    }

    /// One `X-Amz-Target` RPC call, signed for `signing_name` in `region` when `signed` —
    /// its status, parsed body and the id it was answered under.
    async fn rpc(
        &self,
        signed: Option<(&str, &str)>,
        region: &str,
        target: &str,
        body: &Value,
    ) -> (u16, Answered) {
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", target)
            .header("content-type", "application/x-amz-json-1.1");
        if let Some((signing_name, access_key_id)) = signed {
            request = request
                .header("host", format!("{signing_name}.{region}.amazonaws.com"))
                .header(
                    "authorization",
                    format!(
                        "AWS4-HMAC-SHA256 \
                         Credential={access_key_id}/20260101/{region}/{signing_name}/aws4_request, \
                         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
                    ),
                );
        }
        let request = request
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (
            status,
            Answered {
                body: parsed,
                request_id,
            },
        )
    }

    /// One CloudTrail call in `region` that has to be served.
    async fn cloudtrail_in(
        &self,
        case: &str,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> Answered {
        let (status, answered) = self
            .rpc(
                Some(("cloudtrail", "AKIDEMULATOR")),
                region,
                &format!("{TARGET_PREFIX}{operation}"),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CloudTrail {operation}, got {status} with {}",
            answered.body
        );
        answered
    }

    /// One CloudTrail call in the suite's own Region.
    async fn cloudtrail(&self, case: &str, operation: &str, body: &Value) -> Answered {
        self.cloudtrail_in(case, REGION, operation, body).await
    }

    /// One Amazon Cognito Identity call, made the way a row names it: ANONYMOUS for the
    /// identity flow an end user performs holding no AWS credential, or signed with the role
    /// session's access key id for the backend exchange.
    async fn cognito(&self, caller: Caller, operation: &str, body: &Value) -> (u16, Answered) {
        self.cognito_in(caller, REGION, operation, body).await
    }

    /// The same call made in `region`: the Region a SIGNED call carries in its credential
    /// scope, and the Region an ANONYMOUS call carries nowhere at all — which is why a data
    /// event of a pool is recorded in the POOL's Region and not in the request's.
    async fn cognito_in(
        &self,
        caller: Caller,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Answered) {
        let signed = match caller {
            Caller::Anonymous => None,
            Caller::RoleSession => Some((COGNITO_SIGNING_NAME, SESSION_KEY_ID)),
        };
        self.rpc(
            signed,
            region,
            &format!("{COGNITO_TARGET_PREFIX}.{operation}"),
            body,
        )
        .await
    }

    /// One Amazon Cognito Identity call that has to be served.
    async fn cognito_ok(
        &self,
        case: &str,
        caller: Caller,
        operation: &str,
        body: &Value,
    ) -> Answered {
        self.cognito_ok_in(case, caller, REGION, operation, body)
            .await
    }

    /// One Amazon Cognito Identity call in `region` that has to be served.
    async fn cognito_ok_in(
        &self,
        case: &str,
        caller: Caller,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> Answered {
        let (status, answered) = self.cognito_in(caller, region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {}",
            answered.body
        );
        answered
    }

    /// Remember a ROLE SESSION under [`SESSION_KEY_ID`] in the account's session directory —
    /// the shared store STS writes a minted session into and every recorder reads a caller's
    /// session from — so a call signed with that key id is recorded as the session it was
    /// made by.
    fn remember_role_session(&self) {
        use service_common::management_events::{
            RecordedSession, RecordedSessionIssuer, SessionAuditDirectory,
            PRINCIPAL_TYPE_ASSUMED_ROLE, SESSION_ISSUER_TYPE_ROLE,
        };

        let account = account();
        let role_arn = format!("arn:aws:iam::{account}:role/{SESSION_ROLE}");
        let session = RecordedSession {
            principal_type: PRINCIPAL_TYPE_ASSUMED_ROLE.to_string(),
            principal_id: format!("AROATRAILDATAEVENTS1:{SESSION_ROLE}-session"),
            arn: format!("arn:aws:sts::{account}:assumed-role/{SESSION_ROLE}/{SESSION_ROLE}-session"),
            account_id: account.clone(),
            user_name: Some(format!("{SESSION_ROLE}-session")),
            source_identity: None,
            web_identity_federation: None,
            created_at: std::time::SystemTime::now(),
            expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(3600),
            issuer: Some(RecordedSessionIssuer {
                principal_type: SESSION_ISSUER_TYPE_ROLE.to_string(),
                principal_id: "AROATRAILDATAEVENTS1".to_string(),
                arn: role_arn,
                account_id: account,
                user_name: SESSION_ROLE.to_string(),
            }),
        };
        self.engine
            .state::<SessionAuditDirectory>()
            .lock()
            .remember(SESSION_KEY_ID, session, std::time::SystemTime::now());
    }

    /// The Amazon S3 client, over the same in-process callback the delivery uses — built by
    /// the same shared constructor, so the test's transport is the delivery's by
    /// construction.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create `bucket` and attach the policy that lets CloudTrail deliver into it.
    async fn log_bucket(&self, bucket: &str) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
        self.s3()
            .put_bucket_policy()
            .bucket(bucket)
            .policy(cloudtrail_bucket_policy(bucket))
            .send()
            .await
            .unwrap_or_else(|error| panic!("policying {bucket} must succeed, got {error:?}"));
    }

    /// Every object key the bucket holds under `prefix`, as Amazon S3 lists them.
    async fn keys_under(&self, bucket: &str, prefix: &str) -> Vec<String> {
        let listed = self
            .s3()
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .send()
            .await
            .unwrap_or_else(|error| panic!("listing {bucket}/{prefix} must succeed, got {error:?}"));
        let mut keys: Vec<String> = listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect();
        keys.sort();
        keys
    }

    /// The records one delivered log file holds, read through `GetObject` and gunzipped.
    async fn records(&self, bucket: &str, key: &str) -> Vec<Value> {
        let object = self
            .s3()
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("reading {key} must succeed, got {error:?}"));
        let body = object
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("reading {key}'s body must succeed, got {error:?}"))
            .into_bytes();
        let mut document = String::new();
        flate2::read::GzDecoder::new(body.as_ref())
            .read_to_string(&mut document)
            .unwrap_or_else(|error| panic!("{key} must be a gzip stream, got {error}"));
        let parsed: Value = serde_json::from_str(&document)
            .unwrap_or_else(|error| panic!("{key} must gunzip to JSON, got {error}"));
        parsed
            .get(RECORDS_MEMBER)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("{key} must hold {RECORDS_MEMBER}, got {parsed}"))
    }

    /// Every record the trail delivered under `prefix`, with the key each one arrived in.
    async fn delivered(&self, bucket: &str, prefix: &str) -> Vec<(String, Value)> {
        let mut delivered = Vec::new();
        for key in self.keys_under(bucket, prefix).await {
            for record in self.records(bucket, &key).await {
                delivered.push((key.clone(), record));
            }
        }
        delivered
    }
}

/// Who makes a data-plane call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// No credential at all, which is how the identity flow is called.
    Anonymous,
    /// The role session the backend exchange is signed with.
    RoleSession,
}

/// The account every trail, bucket, pool and record below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The trail's whole delivery area under the key prefix it was created with, which is where
/// a consumer of the trail looks for its files.
fn delivery_prefix(key_prefix: &str) -> String {
    format!(
        "{key_prefix}/{LOG_FILE_ROOT_SEGMENT}/{}/{LOG_FILE_SERVICE_SEGMENT}/",
        account()
    )
}

/// The ARN of the identity pool `pool_id`, which is how a record names the resource the call
/// acted on.
fn pool_arn(pool_id: &str) -> String {
    format!(
        "arn:aws:cognito-identity:{REGION}:{}:identitypool/{pool_id}",
        account()
    )
}

/// The text a record holds at `member`, or `""` for one that holds none.
fn text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Point a trail at `bucket` under `key_prefix`, configure it with `selectors`, and start it
/// logging — the four calls a client makes, each of which must be served.
async fn logging_trail(
    emulator: &Emulator,
    trail: &str,
    bucket: &str,
    key_prefix: &str,
    selectors: Value,
) {
    emulator
        .cloudtrail(
            "setup",
            "CreateTrail",
            &json!({
                "Name": trail,
                "S3BucketName": bucket,
                "S3KeyPrefix": key_prefix,
                "IsMultiRegionTrail": false,
                "IncludeGlobalServiceEvents": false,
            }),
        )
        .await;
    let configured = emulator
        .cloudtrail(
            "setup",
            "PutEventSelectors",
            &json!({"TrailName": trail, "AdvancedEventSelectors": selectors}),
        )
        .await;
    assert_eq!(
        configured.body.get("AdvancedEventSelectors"),
        Some(&selectors),
        "setup: expected the selector echoed back unchanged, got {}",
        configured.body
    );
    emulator
        .cloudtrail("setup", "StartLogging", &json!({"Name": trail}))
        .await;
    let status = emulator
        .cloudtrail("setup", "GetTrailStatus", &json!({"Name": trail}))
        .await;
    assert_eq!(
        status.body.get("IsLogging"),
        Some(&json!(true)),
        "setup: expected the trail to report itself logging, got {}",
        status.body
    );
}

/// The advanced event selector AWS publishes for identity-pool data events: the `Data`
/// category and the identity-pool resource type.
fn identity_pool_data_selector() -> Value {
    json!([{
        "Name": "identity-pool-data",
        "FieldSelectors": [
            {"Field": "eventCategory", "Equals": [DATA_CATEGORY]},
            {"Field": "resources.type", "Equals": [IDENTITY_POOL_TYPE]},
        ],
    }])
}

/// Create the fixture pool: it declares the public provider a federated identity is minted
/// for and the developer provider the backend exchange presents a login under.
async fn fixture_pool(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .cognito_ok(
            "setup",
            Caller::RoleSession,
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "SupportedLoginProviders": {LOGIN_PROVIDER: "declared-app-id"},
            }),
        )
        .await;
    created.body["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string()
}

/// One data-plane call of a round, and what the record delivered for it has to say.
struct Served {
    case: &'static str,
    /// The operation called, which is also the `eventName` its record carries.
    operation: &'static str,
    /// Who made the call, which decides the `userIdentity` the record names.
    caller: Caller,
    /// The id the caller was answered under, which joins the record to this call.
    request_id: String,
    /// Whether the call was SERVED, so its record carries no error code.
    served: bool,
    /// The provider names whose `Logins` values the record has to withhold.
    redacted_logins: Vec<String>,
}

/// Issue one round of all five identity-pool data operations against `pool_id`, and answer
/// what each call was answered under, with the tokens the round minted.
///
/// The round is the shape a client's data plane has: mint an identity for a federated login,
/// exchange it for an OpenID token, ask for credentials, unlink the login, and exchange a
/// developer login from a backend. The vend is the REFUSED row — the pool configures no
/// role, which is the modelled refusal of a vend — so the rows span a served and a refused
/// call as well as an anonymous and a signed one.
async fn round(emulator: &Emulator, pool_id: &str) -> (Vec<Served>, Vec<String>) {
    let minted = emulator
        .cognito_ok(
            "a minted identity",
            Caller::Anonymous,
            "GetId",
            &json!({
                "IdentityPoolId": pool_id,
                "Logins": {LOGIN_PROVIDER: LOGIN_TOKEN},
            }),
        )
        .await;
    let identity_id = minted.body["IdentityId"]
        .as_str()
        .expect("GetId answers an IdentityId")
        .to_string();
    let token = emulator
        .cognito_ok(
            "a minted OpenID token",
            Caller::Anonymous,
            "GetOpenIdToken",
            &json!({"IdentityId": &identity_id}),
        )
        .await;
    let (vend_status, vended) = emulator
        .cognito(
            Caller::Anonymous,
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id}),
        )
        .await;
    assert_ne!(
        vend_status, 200,
        "case \"a refused vend\": a pool with no role configured cannot vend, got {}",
        vended.body
    );
    let unlinked = emulator
        .cognito_ok(
            "an unlinked login",
            Caller::Anonymous,
            "UnlinkIdentity",
            &json!({
                "IdentityId": &identity_id,
                "Logins": {LOGIN_PROVIDER: LOGIN_TOKEN},
                "LoginsToRemove": [LOGIN_PROVIDER],
            }),
        )
        .await;
    let exchanged = emulator
        .cognito_ok(
            "a developer-identity exchange",
            Caller::RoleSession,
            "GetOpenIdTokenForDeveloperIdentity",
            &json!({
                "IdentityPoolId": pool_id,
                "Logins": {DEVELOPER_PROVIDER: DEVELOPER_USER},
            }),
        )
        .await;

    let developer_token = exchanged.body["Token"]
        .as_str()
        .expect("the exchange answers a Token")
        .to_string();
    let open_id_token = token.body["Token"]
        .as_str()
        .expect("GetOpenIdToken answers a Token")
        .to_string();

    let calls = vec![
        Served {
            case: "an anonymous mint of an identity for a federated login",
            operation: "GetId",
            caller: Caller::Anonymous,
            request_id: minted.request_id,
            served: true,
            redacted_logins: vec![LOGIN_PROVIDER.to_string()],
        },
        Served {
            case: "an anonymous OpenID token exchange",
            operation: "GetOpenIdToken",
            caller: Caller::Anonymous,
            request_id: token.request_id,
            served: true,
            redacted_logins: Vec::new(),
        },
        Served {
            case: "an anonymous vend the pool's configuration refuses",
            operation: "GetCredentialsForIdentity",
            caller: Caller::Anonymous,
            request_id: vended.request_id,
            served: false,
            redacted_logins: Vec::new(),
        },
        Served {
            case: "an anonymous unlink of that login",
            operation: "UnlinkIdentity",
            caller: Caller::Anonymous,
            request_id: unlinked.request_id,
            served: true,
            redacted_logins: vec![LOGIN_PROVIDER.to_string()],
        },
        Served {
            case: "a developer-identity exchange signed with a role session",
            operation: "GetOpenIdTokenForDeveloperIdentity",
            caller: Caller::RoleSession,
            request_id: exchanged.request_id,
            served: true,
            redacted_logins: vec![DEVELOPER_PROVIDER.to_string()],
        },
    ];
    // The tokens the round minted are secrets of the caller, and the token it PRESENTED is
    // one too: what no delivered file may carry.
    let secrets = vec![LOGIN_TOKEN.to_string(), open_id_token, developer_token];
    (calls, secrets)
}

/// RULE: a logging trail whose advanced selector names the `Data` category and the
/// identity-pool resource type delivers exactly one log-file record per identity-pool data
/// call, joined to its caller by `requestID` and carrying the call's category, resource,
/// redactions and identity — while event history reports none of them.
#[tokio::test]
async fn a_data_selector_delivers_one_record_per_identity_pool_data_call() {
    let emulator = Emulator::new();
    let bucket = "identity-pool-data-trail-logs";
    let trail = "identity-pool-data-trail";
    let key_prefix = "obs/data";
    emulator.remember_role_session();
    emulator.log_bucket(bucket).await;
    logging_trail(
        &emulator,
        trail,
        bucket,
        key_prefix,
        identity_pool_data_selector(),
    )
    .await;
    let pool_id = fixture_pool(&emulator, "trail-data-pool").await;
    let (served, secrets) = round(&emulator, &pool_id).await;

    let delivered = emulator.delivered(bucket, &delivery_prefix(key_prefix)).await;
    for call in &served {
        assert!(
            !call.request_id.is_empty(),
            "case {:?}: the answer must name the request it answers",
            call.case
        );
        let matching: Vec<&(String, Value)> = delivered
            .iter()
            .filter(|(_, record)| text(record, "requestID") == call.request_id)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {:?}: expected exactly ONE delivered record of the call answered as {:?}, \
             got {} among records of {:?}",
            call.case,
            call.request_id,
            matching.len(),
            delivered
                .iter()
                .map(|(_, record)| text(record, "eventName"))
                .collect::<Vec<String>>()
        );
        let (key, record) = matching[0];
        assert_eq!(
            (
                text(record, "eventName").as_str(),
                text(record, "eventSource").as_str(),
                text(record, "eventCategory").as_str(),
            ),
            (call.operation, COGNITO_EVENT_SOURCE, DATA_CATEGORY),
            "case {:?}: expected the data record of {} by {COGNITO_EVENT_SOURCE}, got {record}",
            call.case,
            call.operation
        );
        assert_eq!(
            record.get("managementEvent"),
            Some(&json!(false)),
            "case {:?}: a data event is no management event, got {record}",
            call.case
        );
        assert_eq!(
            (
                record.pointer("/resources/0/type"),
                record.pointer("/resources/0/ARN"),
            ),
            (
                Some(&json!(IDENTITY_POOL_TYPE)),
                Some(&json!(pool_arn(&pool_id))),
            ),
            "case {:?}: expected the pool as the record's resource, got {record}",
            call.case
        );
        assert_eq!(
            record.get("errorCode").is_none(),
            call.served,
            "case {:?}: expected a served call == {}, got {record}",
            call.case,
            call.served
        );
        for provider in &call.redacted_logins {
            assert_eq!(
                record.pointer(&format!("/requestParameters/logins/{provider}")),
                Some(&json!(HIDDEN)),
                "case {:?}: expected the token presented under {provider:?} withheld, got \
                 {record}",
                call.case
            );
        }
        match call.caller {
            Caller::Anonymous => assert_eq!(
                record.get("userIdentity"),
                Some(&json!({"type": UNKNOWN_PRINCIPAL})),
                "case {:?}: an unsigned call names no principal, got {record}",
                call.case
            ),
            Caller::RoleSession => assert_eq!(
                (
                    record.pointer("/userIdentity/type"),
                    record.pointer("/userIdentity/sessionContext/sessionIssuer/type"),
                    record.pointer("/userIdentity/sessionContext/sessionIssuer/arn"),
                ),
                (
                    Some(&json!("AssumedRole")),
                    Some(&json!("Role")),
                    Some(&json!(format!(
                        "arn:aws:iam::{}:role/{SESSION_ROLE}",
                        account()
                    ))),
                ),
                "case {:?}: expected the role session that made the call, got {record}",
                call.case
            ),
        }
        assert!(
            key.starts_with(&delivery_prefix(key_prefix)) && key.ends_with(LOG_FILE_SUFFIX),
            "case {:?}: expected the record under the trail's prefix in a {LOG_FILE_SUFFIX} \
             file, got {key:?}",
            call.case
        );
    }

    // No secret the round carried or minted reaches a delivered file.
    let written = delivered
        .iter()
        .map(|(_, record)| record.to_string())
        .collect::<Vec<String>>()
        .join("\n");
    let mut secrets = secrets;
    secrets.sort();
    for secret in secrets {
        assert!(
            !written.contains(&secret),
            "case \"a delivered record\": no file may carry the secret {secret:?}, got {written}"
        );
    }

    // ...and a data event is not event history: a lookup over this service's events reports
    // the pool's own CREATE and none of the five data calls.
    let looked_up = emulator
        .cloudtrail(
            "the lookup",
            "LookupEvents",
            &json!({
                "LookupAttributes": [{
                    "AttributeKey": "EventSource",
                    "AttributeValue": COGNITO_EVENT_SOURCE,
                }],
            }),
        )
        .await;
    let names: Vec<String> = looked_up
        .body
        .get("Events")
        .and_then(Value::as_array)
        .map(|events| events.iter().map(|event| text(event, "EventName")).collect())
        .unwrap_or_default();
    assert!(
        names.iter().any(|name| name == "CreateIdentityPool"),
        "case \"the lookup\": the pool's own create IS a management event, got {names:?}"
    );
    for call in &served {
        assert!(
            !names.iter().any(|name| name == call.operation),
            "case {:?}: a data event reaches no event history, got {names:?}",
            call.case
        );
    }
}

/// RULE's other side: a trail delivers a data event only while one of its own selectors
/// admits it — so the selector class decides the delivery, over a table of selectors that
/// differ along the category, the resource type and the read/write narrowing they name.
#[tokio::test]
async fn a_trail_delivers_a_data_event_only_where_its_selectors_admit_it() {
    // (case, the trail's advanced selectors, whether a data call is delivered)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "the published identity-pool data selector",
            identity_pool_data_selector(),
            true,
        ),
        (
            "a management-events selector",
            json!([{
                "Name": "management",
                "FieldSelectors": [{"Field": "eventCategory", "Equals": [MANAGEMENT_CATEGORY]}],
            }]),
            false,
        ),
        (
            "a data selector over another resource type",
            json!([{
                "Name": "object-data",
                "FieldSelectors": [
                    {"Field": "eventCategory", "Equals": [DATA_CATEGORY]},
                    {"Field": "resources.type", "Equals": [S3_OBJECT_TYPE]},
                ],
            }]),
            false,
        ),
        (
            "a data selector narrowed to read-only calls",
            json!([{
                "Name": "read-only-data",
                "FieldSelectors": [
                    {"Field": "eventCategory", "Equals": [DATA_CATEGORY]},
                    {"Field": "resources.type", "Equals": [IDENTITY_POOL_TYPE]},
                    {"Field": "readOnly", "Equals": ["true"]},
                ],
            }]),
            false,
        ),
        (
            "a data selector that also pins the operation",
            json!([{
                "Name": "mints-only",
                "FieldSelectors": [
                    {"Field": "eventCategory", "Equals": [DATA_CATEGORY]},
                    {"Field": "resources.type", "Equals": [IDENTITY_POOL_TYPE]},
                    {"Field": "eventName", "Equals": ["GetId"]},
                ],
            }]),
            true,
        ),
    ];

    for (case, selectors, delivers) in rows {
        let emulator = Emulator::new();
        let bucket = "selector-class-trail-logs";
        let trail = "selector-class-trail";
        let key_prefix = "obs/selector";
        let prefix = delivery_prefix(key_prefix);
        emulator.log_bucket(bucket).await;
        logging_trail(&emulator, trail, bucket, key_prefix, selectors).await;
        let pool_id = fixture_pool(&emulator, "selector-class-pool").await;
        let minted = emulator
            .cognito_ok(
                case,
                Caller::Anonymous,
                "GetId",
                &json!({"IdentityPoolId": &pool_id}),
            )
            .await;

        let delivered = emulator.delivered(bucket, &prefix).await;
        let found = delivered
            .iter()
            .any(|(_, record)| text(record, "requestID") == minted.request_id);
        assert_eq!(
            found, delivers,
            "case {case:?}: expected the data call answered as {:?} delivered == {delivers}, \
             got records of {:?}",
            minted.request_id,
            delivered
                .iter()
                .map(|(_, record)| (
                    text(record, "eventName"),
                    text(record, "eventCategory")
                ))
                .collect::<Vec<(String, String)>>()
        );
    }
}

/// RULE's third side: a trail that has STOPPED logging delivers nothing for a later data
/// call, while the same call before the stop was delivered.
#[tokio::test]
async fn a_stopped_trail_delivers_no_later_data_event() {
    let emulator = Emulator::new();
    let bucket = "stopped-data-trail-logs";
    let trail = "stopped-data-trail";
    let key_prefix = "obs/stopped-data";
    let prefix = delivery_prefix(key_prefix);
    emulator.log_bucket(bucket).await;
    logging_trail(
        &emulator,
        trail,
        bucket,
        key_prefix,
        identity_pool_data_selector(),
    )
    .await;
    let pool_id = fixture_pool(&emulator, "stopped-data-pool").await;

    let while_logging = emulator
        .cognito_ok(
            "while logging",
            Caller::Anonymous,
            "GetId",
            &json!({"IdentityPoolId": &pool_id}),
        )
        .await;
    let delivered = emulator.delivered(bucket, &prefix).await;
    assert!(
        delivered
            .iter()
            .any(|(_, record)| text(record, "requestID") == while_logging.request_id),
        "case \"while logging\": expected the data call answered as {:?} delivered, got {} \
         record(s)",
        while_logging.request_id,
        delivered.len()
    );

    emulator
        .cloudtrail("stopping", "StopLogging", &json!({"Name": trail}))
        .await;
    let before = emulator.keys_under(bucket, &prefix).await;
    let after_stop = emulator
        .cognito_ok(
            "after the stop",
            Caller::Anonymous,
            "GetId",
            &json!({"IdentityPoolId": &pool_id}),
        )
        .await;
    let after = emulator.keys_under(bucket, &prefix).await;
    assert_eq!(
        after, before,
        "case \"after the stop\": expected no further log file for the data call answered as \
         {:?}, got {after:?}",
        after_stop.request_id
    );
}

/// A second Region, so the Region a data event belongs to can be told from the Region the
/// suite's own trail is homed in.
const OTHER_REGION: &str = "us-west-2";

/// RULE's fourth side: a data event belongs to the Region of the RESOURCE the call acted on
/// — an identity pool's own id names that Region — so a single-Region trail delivers the
/// data events of its own Region and no other's, whatever Region the request named (an
/// anonymous data-plane call names none at all).
#[tokio::test]
async fn a_single_region_trail_delivers_only_the_data_events_of_its_own_region() {
    let emulator = Emulator::new();
    // One trail per Region, each with its own bucket and the same data selector, so each
    // Region's delivery is read on its own.
    let here = ("regional-data-trail-logs", "regional-data-trail", "obs/here");
    let there = (
        "regional-data-trail-logs-other",
        "regional-data-trail-other",
        "obs/there",
    );
    for (bucket, trail, key_prefix) in [here, there] {
        emulator.log_bucket(bucket).await;
        emulator
            .cloudtrail_in(
                "setup",
                if bucket == there.0 { OTHER_REGION } else { REGION },
                "CreateTrail",
                &json!({
                    "Name": trail,
                    "S3BucketName": bucket,
                    "S3KeyPrefix": key_prefix,
                    "IsMultiRegionTrail": false,
                    "IncludeGlobalServiceEvents": false,
                }),
            )
            .await;
        emulator
            .cloudtrail_in(
                "setup",
                if bucket == there.0 { OTHER_REGION } else { REGION },
                "PutEventSelectors",
                &json!({
                    "TrailName": trail,
                    "AdvancedEventSelectors": identity_pool_data_selector(),
                }),
            )
            .await;
        emulator
            .cloudtrail_in(
                "setup",
                if bucket == there.0 { OTHER_REGION } else { REGION },
                "StartLogging",
                &json!({"Name": trail}),
            )
            .await;
    }

    // One pool per Region: a pool's id carries the Region it was created in.
    let mut answered: Vec<(&str, String, String)> = Vec::new();
    for (case, region) in [
        ("a pool of the trail's own Region", REGION),
        ("a pool of another Region", OTHER_REGION),
    ] {
        let created = emulator
            .cognito_ok_in(
                case,
                Caller::RoleSession,
                region,
                "CreateIdentityPool",
                &json!({
                    "IdentityPoolName": format!("regional-data-pool-{region}"),
                    "AllowUnauthenticatedIdentities": true,
                }),
            )
            .await;
        let pool_id = created.body["IdentityPoolId"]
            .as_str()
            .expect("CreateIdentityPool answers an IdentityPoolId")
            .to_string();
        assert!(
            pool_id.starts_with(&format!("{region}:")),
            "case {case:?}: a pool's id names the Region it lives in, got {pool_id:?}"
        );
        // The data call is ANONYMOUS, so the request itself names no Region at all: what
        // decides the Region of the event is the pool.
        let minted = emulator
            .cognito_ok(case, Caller::Anonymous, "GetId", &json!({"IdentityPoolId": &pool_id}))
            .await;
        answered.push((case, region.to_string(), minted.request_id));
    }

    for (bucket, _trail, key_prefix) in [here, there] {
        let trail_region = if bucket == there.0 { OTHER_REGION } else { REGION };
        let delivered = emulator.delivered(bucket, &delivery_prefix(key_prefix)).await;
        for (case, pool_region, request_id) in &answered {
            let found = delivered
                .iter()
                .any(|(_, record)| text(record, "requestID") == *request_id);
            let expected = pool_region == trail_region;
            assert_eq!(
                found, expected,
                "case {case:?}: expected the {pool_region} data call delivered to the \
                 {trail_region} trail == {expected}, got records of {:?}",
                delivered
                    .iter()
                    .map(|(_, record)| (text(record, "eventName"), text(record, "awsRegion")))
                    .collect::<Vec<(String, String)>>()
            );
        }
    }
}

/// An advanced selector that admits the identity-pool data events of ONE pool: the `Data`
/// category, the identity-pool resource type and that pool's own ARN.
fn arn_scoped_selector(pool_id: &str) -> Value {
    json!([{
        "Name": "identity-pool-data-of-one-pool",
        "FieldSelectors": [
            {"Field": "eventCategory", "Equals": [DATA_CATEGORY]},
            {"Field": "resources.type", "Equals": [IDENTITY_POOL_TYPE]},
            {"Field": "resources.ARN", "Equals": [pool_arn(pool_id)]},
        ],
    }])
}

/// The selector that admits MANAGEMENT events, which a trail carries beside its data
/// selector while a client audits both classes into one bucket.
fn management_selector() -> Value {
    json!({
        "Name": "management",
        "FieldSelectors": [{"Field": "eventCategory", "Equals": [MANAGEMENT_CATEGORY]}],
    })
}

/// Mint an identity in `pool_id` and exchange it for an OpenID token, answering the id each
/// call was answered under.
async fn identity_calls(emulator: &Emulator, pool_id: &str) -> Vec<(&'static str, String)> {
    let minted = emulator
        .cognito_ok(
            "a minted identity",
            Caller::Anonymous,
            "GetId",
            &json!({"IdentityPoolId": pool_id, "Logins": {LOGIN_PROVIDER: LOGIN_TOKEN}}),
        )
        .await;
    let identity_id = minted.body["IdentityId"]
        .as_str()
        .expect("GetId answers an IdentityId")
        .to_string();
    let exchanged = emulator
        .cognito_ok(
            "a minted OpenID token",
            Caller::Anonymous,
            "GetOpenIdToken",
            &json!({
                "IdentityId": &identity_id,
                "Logins": {LOGIN_PROVIDER: LOGIN_TOKEN},
            }),
        )
        .await;
    vec![
        ("GetId", minted.request_id),
        ("GetOpenIdToken", exchanged.request_id),
    ]
}

#[tokio::test]
async fn an_arn_scoped_selector_installed_on_a_logging_trail_delivers_only_that_pool() {
    let emulator = Emulator::new();
    let bucket = "arn-scoped-data-trail-logs";
    let trail = "arn-scoped-data-trail";
    let key_prefix = "obs/arn-scoped";
    let prefix = delivery_prefix(key_prefix);
    emulator.log_bucket(bucket).await;
    // The trail starts out logging MANAGEMENT events only, which is where a client that adds
    // a data selector later starts from, and it is addressed by its ARN from here on.
    let created = emulator
        .cloudtrail(
            "setup",
            "CreateTrail",
            &json!({
                "Name": trail,
                "S3BucketName": bucket,
                "S3KeyPrefix": key_prefix,
                "IsMultiRegionTrail": false,
                "IncludeGlobalServiceEvents": false,
            }),
        )
        .await;
    let trail_arn = text(&created.body, "TrailARN");
    assert!(
        !trail_arn.is_empty(),
        "setup: CreateTrail must answer the trail's ARN, got {}",
        created.body
    );
    emulator
        .cloudtrail(
            "setup",
            "PutEventSelectors",
            &json!({
                "TrailName": &trail_arn,
                "AdvancedEventSelectors": json!([management_selector()]),
            }),
        )
        .await;
    emulator
        .cloudtrail("setup", "StartLogging", &json!({"Name": &trail_arn}))
        .await;

    let named = fixture_pool(&emulator, "arn-scoped-pool-named").await;
    let other = fixture_pool(&emulator, "arn-scoped-pool-other").await;

    // The warm leg a client uses to prove the trail is capturing at all: a READ-ONLY
    // management call of the same service, which the management selector admits.
    let marker = emulator
        .cognito_ok(
            "a management marker",
            Caller::RoleSession,
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": &named}),
        )
        .await;

    let before_any_data_selector = identity_calls(&emulator, &other).await;

    // The data selector is ADDED beside the management one, on a trail that is already
    // logging, and it names ONE pool's ARN.
    let mut selectors = arn_scoped_selector(&named);
    selectors
        .as_array_mut()
        .expect("the data selector set is a list")
        .insert(0, management_selector());
    let configured = emulator
        .cloudtrail(
            "the selector",
            "PutEventSelectors",
            &json!({"TrailName": &trail_arn, "AdvancedEventSelectors": &selectors}),
        )
        .await;
    assert_eq!(
        configured.body.get("AdvancedEventSelectors"),
        Some(&selectors),
        "case \"the selector\": expected the set echoed back unchanged, got {}",
        configured.body
    );

    let on_named = identity_calls(&emulator, &named).await;
    let on_other = identity_calls(&emulator, &other).await;

    let delivered = emulator.delivered(bucket, &prefix).await;
    /// One row: the case, the calls it made (each an operation and the id it was answered
    /// under), and whether the trail delivers them.
    type Row = (&'static str, Vec<(&'static str, String)>, bool);
    let rows: Vec<Row> = vec![
        (
            "a management call of the same service",
            vec![("DescribeIdentityPool", marker.request_id)],
            true,
        ),
        (
            "a data call made before any data selector",
            before_any_data_selector,
            false,
        ),
        ("the pool the data selector names", on_named, true),
        (
            "a pool the data selector does not name",
            on_other,
            false,
        ),
    ];
    for (case, calls, expected) in rows {
        for (operation, request_id) in calls {
            let found = delivered
                .iter()
                .any(|(_, record)| text(record, "requestID") == request_id);
            assert_eq!(
                found, expected,
                "case {case:?}: expected {operation} answered as {request_id:?} delivered == \
                 {expected}, got records of {:?}",
                delivered
                    .iter()
                    .map(|(_, record)| (
                        text(record, "eventName"),
                        text(record, "requestID"),
                        record.pointer("/resources/0/ARN").cloned(),
                    ))
                    .collect::<Vec<(String, String, Option<Value>)>>()
            );
        }
    }
}
