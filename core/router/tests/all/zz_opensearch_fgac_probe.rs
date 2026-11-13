// SPDX-License-Identifier: CC0-1.0

//! The address an Amazon OpenSearch Service domain publishes for its own data plane, driven
//! end to end on an emulator that VERIFIES request signatures.
//!
//! The RULE the rows assert, one level above the recorded case:
//!
//! > The address a control-plane answer publishes for a resource's data plane is an address
//! > a client DIALS as published — it builds `https://{address}{path}` and signs what it
//! > sends — so the emulator publishes the endpoint it answers on, with the resource named
//! > in the path, and serves that resource there. A request whose `Host` header is not the
//! > one it was SIGNED for is refused by the verifier before it reaches the resource at all,
//! > however much its principal is allowed.
//! > Once it does reach the domain, the cluster decides: on a domain with fine-grained
//! > access control the principal `MasterUserOptions.MasterUserARN` names may do everything,
//! > a principal no role mapping binds is refused `403` with `error.type`
//! > `security_exception`, and binding its role makes the same request serve.
//!
//! The rows span what that rule generalises over: WHICH principal signs (a session of the
//! master role, a session of a role no mapping binds), WHAT it asks for (the cluster's
//! health, an index create, a document write, a search, the security API), whether the
//! request carries a body, and whether the host the request was SIGNED for is the host it
//! carries — the one case that must be refused however much the principal is allowed.
//! Only the assembled router can pin this: the credential store and the SigV4 verifier are
//! the emulator's, IAM holds the roles and their policies, STS mints the session credential
//! and names its principal, and Amazon OpenSearch Service publishes the address and serves
//! the domain at it.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_emulator_router::{build_router, InMemoryEngine};
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call is signed for, and so the region of the domain's own ARN and host.
const REGION: &str = "us-west-2";

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The emulator's own endpoint, which is where every request below is DIALLED: a client on
/// this machine resolves none of AWS's hosts, so it reaches the emulator and carries the
/// host it was told in the `Host` header.
const LOCAL_ENDPOINT: &str = "127.0.0.1:9297";

/// The domain the suite provisions.
const DOMAIN: &str = "fgac-master-domain";

/// The index the master seeds and the reader searches.
const INDEX: &str = "catalog";

/// The engine release the domain runs, from this service's own catalogue.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The role the domain's `MasterUserARN` names.
const MASTER_ROLE: &str = "fgac-master-role";

/// A role of the same account that no role mapping binds.
const READER_ROLE: &str = "fgac-reader-role";

/// One request answered by the router.
struct Answer {
    status: u16,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The `error.type` an engine refusal carries.
    fn error_type(&self) -> String {
        self.json()["error"]["type"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn summary(&self) -> String {
        format!("{} {}", self.status, self.text)
    }
}

async fn dispatch(router: &MultiServiceRouter, request: http::Request<Vec<u8>>) -> Answer {
    let (parts, body) = request.into_parts();
    let request = http::Request::from_parts(parts, bytes::Bytes::from(body));
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    Answer { status, text }
}

/// The credential scope a signed SDK request carries, with no real signature: what the SETUP
/// calls use, before request authentication is turned on, so the router can name the target
/// service.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery form call to `signing_name`, unsigned.
async fn query(router: &MultiServiceRouter, signing_name: &str, body: String) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", credential_scope(signing_name))
        .body(body.into_bytes())
        .expect("the test request is well formed");
    dispatch(router, request).await
}

/// Create `name` as a role the account root may assume, with an inline grant of this
/// service's data-plane actions on the domain — the IAM half of the decision, so what is
/// left to decide is the cluster's.
async fn created_role(router: &MultiServiceRouter, name: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string();
    let created = query(
        router,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}&AssumeRolePolicyDocument={}",
            form_encode(&trust)
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup CreateRole({name}): expected 200, got {}",
        created.summary()
    );
    let arn = element(&created.text, "Arn");
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "es:ESHttp*",
            "Resource": format!("arn:aws:es:{REGION}:{ACCOUNT}:domain/{DOMAIN}/*")
        }]
    })
    .to_string();
    let attached = query(
        router,
        "iam",
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}&PolicyName=data-plane\
             &PolicyDocument={}",
            form_encode(&document)
        ),
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "setup PutRolePolicy({name}): expected 200, got {}",
        attached.summary()
    );
    arn
}

/// The credential `sts:AssumeRole` minted for `role_arn`: its key id, secret and token.
async fn assumed(router: &MultiServiceRouter, role_arn: &str) -> Session {
    let answer = query(
        router,
        "sts",
        format!(
            "Action=AssumeRole&Version=2011-06-15&RoleArn={}&RoleSessionName=probe",
            form_encode(role_arn)
        ),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup AssumeRole({role_arn}): expected 200, got {}",
        answer.summary()
    );
    let field = |name: &str| element(&answer.text, name);
    Session {
        access_key_id: field("AccessKeyId"),
        secret_access_key: field("SecretAccessKey"),
        session_token: field("SessionToken"),
    }
}

/// One assumed-role credential, as a client holds it.
struct Session {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
}

/// Sign `request` with `session` for this service's signing name, over the request AS BUILT.
fn signed(request: http::Request<Vec<u8>>, session: &Session) -> http::Request<Vec<u8>> {
    let mut request = request;
    let identity = Credentials::new(
        &session.access_key_id,
        &session.secret_access_key,
        Some(session.session_token.clone()),
        None,
        "test",
    )
    .into();
    let mut settings = SigningSettings::default();
    settings.excluded_headers = None;
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name("es")
        .time(SystemTime::now())
        .settings(settings)
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

/// One data-plane request to the address the control plane published, dialled exactly as a
/// client dials it: the client builds `https://{address}{path}`, so the AUTHORITY of the
/// published value is the host the request carries and signs, and whatever the value carries
/// after it is the prefix of the request path.
async fn at_published_host(
    router: &MultiServiceRouter,
    session: &Session,
    address: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body
        .map(|value| value.to_string().into_bytes())
        .unwrap_or_default();
    let authority = address.split('/').next().unwrap_or(address);
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{address}{path}"))
        .header("host", authority);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(payload)
        .expect("the test request is well formed");
    dispatch(router, signed(request, session)).await
}

/// `CreateDomain` with fine-grained access control and `master_user_arn` as its master user.
async fn create_domain(router: &MultiServiceRouter, master_user_arn: &str) -> Answer {
    let access_policies = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "es:*",
            "Resource": format!("arn:aws:es:{REGION}:{ACCOUNT}:domain/{DOMAIN}/*")
        }]
    })
    .to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://{LOCAL_ENDPOINT}/2021-01-01/opensearch/domain"
        ))
        .header("host", LOCAL_ENDPOINT)
        .header("content-type", "application/json")
        .header("authorization", credential_scope("es"))
        .body(
            json!({
                "DomainName": DOMAIN,
                "EngineVersion": ENGINE_VERSION,
                "ClusterConfig": {
                    "InstanceType": "t3.small.search",
                    "InstanceCount": 1,
                    "DedicatedMasterEnabled": false,
                    "ZoneAwarenessEnabled": false
                },
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
                "EncryptionAtRestOptions": { "Enabled": true },
                "NodeToNodeEncryptionOptions": { "Enabled": true },
                "DomainEndpointOptions": {
                    "EnforceHTTPS": true,
                    "TLSSecurityPolicy": "Policy-Min-TLS-1-2-2019-07"
                },
                "AdvancedSecurityOptions": {
                    "Enabled": true,
                    "InternalUserDatabaseEnabled": false,
                    "MasterUserOptions": { "MasterUserARN": master_user_arn }
                },
                "AccessPolicies": access_policies,
            })
            .to_string()
            .into_bytes(),
        )
        .expect("the test request is well formed");
    dispatch(router, request).await
}

/// The `DomainStatus` the control plane reports for the domain, read at the emulator's own
/// endpoint — the endpoint a client on this machine reaches it at.
async fn describe_domain(router: &MultiServiceRouter) -> Answer {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://{LOCAL_ENDPOINT}/2021-01-01/opensearch/domain/{DOMAIN}"
        ))
        .header("host", LOCAL_ENDPOINT)
        .header("authorization", credential_scope("es"))
        .body(Vec::new())
        .expect("the test request is well formed");
    dispatch(router, request).await
}

/// How long a poll waits for the domain's provisioning to fall due before it gives up:
/// generous, because no case here tests how long that span is — the wait is for the
/// CONDITION (the domain reporting nothing in flight) and this is only the point at which
/// waiting is called a failure.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll waits between two looks at the domain.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wait until the control plane reports the domain created with nothing in flight — the
/// condition a client polls for — rather than for a fixed time, so the test is about what
/// the service reports and not about how fast the host runs.
async fn settled(router: &MultiServiceRouter) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let answer = describe_domain(router).await;
        let status = &answer.json()["DomainStatus"];
        if status["Created"].as_bool() == Some(true)
            && status["Processing"].as_bool() == Some(false)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup DescribeDomain: the domain never reported itself settled, last read {}",
            answer.summary()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[test]
fn the_published_domain_host_serves_the_signed_requests_its_master_and_readers_make() {
    std::thread::Builder::new()
        .name("opensearch-fgac-published-host".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);

                // SETUP, with authentication off so the control-plane calls need no real
                // signature: two roles of the account, a session of each, and a domain whose
                // master user is the first role.
                router.set_request_authentication(false);
                let master_arn = created_role(&router, MASTER_ROLE).await;
                let reader_arn = created_role(&router, READER_ROLE).await;
                let master = assumed(&router, &master_arn).await;
                let reader = assumed(&router, &reader_arn).await;

                let created = create_domain(&router, &master_arn).await;
                assert_eq!(
                    created.status, 200,
                    "setup CreateDomain: expected 200, got {}",
                    created.summary()
                );
                settled(&router).await;

                // The published address is one a client DIALS as published: the endpoint
                // this emulator answers on, with the reserved path naming this service's
                // data plane and the domain behind it — because the host AWS names a
                // domain's endpoint by resolves nowhere for a client on this machine.
                let described = describe_domain(&router).await;
                let host = described.json()["DomainStatus"]["Endpoint"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                // The authority is read from the framework, which owns what this emulator's
                // endpoint IS for a router that is not served on a port, rather than
                // spelled out here.
                let endpoint = smithy_server_runtime::UNSERVED_URL_BASE;
                let authority = endpoint
                    .split_once("://")
                    .map_or(endpoint, |(_scheme, authority)| authority);
                assert_eq!(
                    host,
                    format!("{authority}/_aws/es/{DOMAIN}"),
                    "case \"the published address\": expected this emulator's own endpoint \
                     with the reserved path naming this plane and the domain, got {} ",
                    described.summary()
                );

                // From here on the SAME router verifies request signatures, as the emulator
                // does when it is started with authentication on.
                router.set_request_authentication(true);

                // (the case, the session, the method, the path, the body, the status, the
                // `error.type` where the answer is a refusal)
                type Row<'r> = (
                    &'static str,
                    &'r Session,
                    &'static str,
                    String,
                    Option<Value>,
                    u16,
                    Option<&'static str>,
                );
                let served: Vec<Row<'_>> = vec![
                    (
                        "the master role reading the cluster health",
                        &master,
                        "GET",
                        "/_cluster/health".to_string(),
                        None,
                        200,
                        None,
                    ),
                    (
                        "the master role creating an index",
                        &master,
                        "PUT",
                        format!("/{INDEX}"),
                        Some(json!({
                            "mappings": { "properties": { "title": { "type": "keyword" } } }
                        })),
                        200,
                        None,
                    ),
                    (
                        "the master role writing a document",
                        &master,
                        "PUT",
                        format!("/{INDEX}/_doc/1"),
                        Some(json!({ "title": "first" })),
                        201,
                        None,
                    ),
                    (
                        "a role no mapping binds searching",
                        &reader,
                        "POST",
                        format!("/{INDEX}/_search"),
                        Some(json!({})),
                        403,
                        Some("security_exception"),
                    ),
                    (
                        "a role no mapping binds reading the cluster health",
                        &reader,
                        "GET",
                        "/_cluster/health".to_string(),
                        None,
                        403,
                        Some("security_exception"),
                    ),
                ];
                for (case, session, method, path, body, status, error_type) in served {
                    let answer =
                        at_published_host(&router, session, &host, method, &path, body).await;
                    assert_eq!(
                        answer.status, status,
                        "case {case:?}: expected HTTP {status} from the published host, got {}",
                        answer.summary()
                    );
                    if let Some(expected) = error_type {
                        assert_eq!(
                            answer.error_type(),
                            expected,
                            "case {case:?}: expected the cluster's {expected}, got {}",
                            answer.summary()
                        );
                    }
                }

                // The health document is the cluster's own, read out of the domain's
                // configuration rather than a fixed body.
                let health = at_published_host(
                    &router,
                    &master,
                    &host,
                    "GET",
                    "/_cluster/health",
                    None,
                )
                .await;
                assert_eq!(
                    (
                        health.json()["cluster_name"].as_str(),
                        health.json()["number_of_data_nodes"].as_i64(),
                    ),
                    (Some(format!("{ACCOUNT}:{DOMAIN}").as_str()), Some(1)),
                    "case \"the cluster health document\": expected this domain's own \
                     cluster, got {}",
                    health.summary()
                );

                // Binding the reader's role is the one change that makes the same search
                // serve, and the document the master wrote is what it answers with.
                let mapped = at_published_host(
                    &router,
                    &master,
                    &host,
                    "PUT",
                    "/_plugins/_security/api/rolesmapping/all_access",
                    Some(json!({ "backend_roles": [reader_arn] })),
                )
                .await;
                assert_eq!(
                    mapped.status, 201,
                    "case \"the master binding the reader's role\": expected 201, got {}",
                    mapped.summary()
                );
                let searched = at_published_host(
                    &router,
                    &reader,
                    &host,
                    "POST",
                    &format!("/{INDEX}/_search"),
                    Some(json!({})),
                )
                .await;
                assert_eq!(
                    (
                        searched.status,
                        searched.json()["hits"]["hits"][0]["_source"]["title"].as_str(),
                    ),
                    (200, Some("first")),
                    "case \"a role a mapping binds searching\": expected the master's own \
                     document, got {}",
                    searched.summary()
                );

                // And the invariant the verifier holds whatever is published: a request
                // whose `Host` header is not the one it was SIGNED for is refused, however
                // much its principal is allowed — which is why a client signs the address it
                // dials. The request still ADDRESSES the domain (its path names the plane
                // and the domain), so what refuses it is the signature and nothing else.
                let built = http::Request::builder()
                    .method("GET")
                    .uri(format!("https://{host}/_cluster/health"))
                    .header("host", host.split('/').next().unwrap_or(&host))
                    .body(Vec::new())
                    .expect("the test request is well formed");
                let mut mis_signed = signed(built, &master);
                mis_signed.headers_mut().insert(
                    "host",
                    http::HeaderValue::from_str("another.host.invalid")
                        .expect("the other host is a header"),
                );
                let refused = dispatch(&router, mis_signed).await;
                assert_eq!(
                    refused.status, 403,
                    "case \"a request signed for another host than it carries\": expected the \
                     verifier's refusal, got {}",
                    refused.summary()
                );
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
