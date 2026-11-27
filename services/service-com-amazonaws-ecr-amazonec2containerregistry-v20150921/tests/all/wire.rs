// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares: one emulator, and one call on the
//! awsJson1_1 protocol this service speaks.
//!
//! It lives in its own module so a new suite does not copy it: a test that exercises an
//! operation goes through the router here, so it covers routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The access key id every call made through [`call`] presents: a key neither STS nor IAM
/// holds a record for, which speaks for the account's own published credential. Its
/// credential scope is what routes a request to ECR, and that scope's region is what a
/// repository ARN is built for.
pub const ACCOUNT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The region every call made through [`call`] and [`call_as`] is signed for, which a
/// repository ARN is built for.
pub const REGION: &str = "eu-west-1";

/// The endpoint host every call made through [`call`], [`call_as`] and [`call_in_region`]
/// is addressed to, for the Region it names.
pub fn endpoint_host(region: &str) -> String {
    format!("api.ecr.{region}.amazonaws.com")
}

/// The base URL this harness's emulator is SERVED at, which is what
/// `smithy_server_runtime::OperationContext::endpoint` reports and therefore the address a
/// registry client of these suites is handed to dial. A loopback socket, because that is the
/// only kind of address a client on the machine an emulator runs on can reach.
pub const SERVED_URL_BASE: &str = "https://127.0.0.1:9297";

/// The endpoint the registry URL a `docker login` of a call made through this harness is
/// pointed at is built on ([`announced_registry_url`]).
///
/// Its calls are addressed to [`endpoint_host`], which names a service and is therefore an
/// endpoint no registry client can use, so the address reported to them is the one this
/// emulator is served at ([`SERVED_URL_BASE`]) — host and port, without the scheme. A
/// repository's own URI is that same endpoint with the repository's name as its path
/// ([`published_registry_authority`]).
pub const ANNOUNCED_REGISTRY_ENDPOINT: &str = "127.0.0.1:9297";

/// The registry URL a `docker login` of a call made through this harness is pointed at
/// (`AuthorizationData$proxyEndpoint`): a scheme followed by the endpoint this emulator
/// answers the caller's registry requests at ([`ANNOUNCED_REGISTRY_ENDPOINT`]).
pub fn announced_registry_url() -> String {
    format!("https://{ANNOUNCED_REGISTRY_ENDPOINT}")
}

/// The account id every generated emulator vends, spelled out here rather than imported
/// so a wrong registry id in the implementation fails these tests.
pub const REGISTRY_ID: &str = "000000000000";

/// The host of the registry this account owns in `region`, in the spelling AWS PUBLISHES
/// (`AuthorizationData$proxyEndpoint`: "The Amazon ECR registry URL format is
/// `https://aws_account_id.dkr.ecr.region.amazonaws.com`"), with the DNS suffix `region`'s
/// own partition is served under — the `dnsSuffix` each partition publishes in the AWS SDK
/// partition metadata (`endpoints.json`), spelled out here rather than imported so a wrong
/// suffix in the implementation fails these tests.
///
/// It is the host every repository of that registry is addressed under ON AWS, so it is the
/// spelling a client's own reference may be written in, which a pull reads back — and never
/// the authority this registry PUBLISHES for a repository
/// ([`published_registry_authority`]), which is the endpoint this emulator answers the
/// caller's registry requests at, because no listener of this emulator answers for a name AWS
/// serves.
pub fn registry_host_in(region: &str) -> String {
    let suffix = match region {
        region if region.starts_with("cn-") => "amazonaws.com.cn",
        region if region.starts_with("eusc-") => "amazonaws.eu",
        region if region.starts_with("us-iso-") => "c2s.ic.gov",
        region if region.starts_with("us-isob-") => "sc2s.sgov.gov",
        region if region.starts_with("us-isof-") => "csp.hci.ic.gov",
        region if region.starts_with("eu-isoe-") => "cloud.adc-e.uk",
        _ => "amazonaws.com",
    };
    format!("{REGISTRY_ID}.dkr.ecr.{region}.{suffix}")
}

/// The authority this registry publishes every repository it holds at, to a call made through
/// this harness: the endpoint this emulator answers that caller's registry requests at
/// ([`ANNOUNCED_REGISTRY_ENDPOINT`]), host and port whole.
///
/// `Repository$repositoryUri` is an ADDRESS a container client splits on the first `/` and
/// DIALS, so the authority is an endpoint this process answers on rather than a name the
/// client would have to resolve: neither the zone AWS serves its own registries in
/// ([`registry_host_in`], which resolves at Amazon ECR rather than near this emulator) nor a
/// registry-shaped name of this emulator's own, which the core's front door refuses on a
/// loopback bind. The URL a `docker login` is pointed at is built on the same endpoint
/// ([`announced_registry_url`]), behind a scheme, so the two addresses this service publishes
/// cannot disagree. WHICH registry holds the repository is reported beside the URI, by
/// `Repository$registryId` and the Region in `Repository$repositoryArn`. Spelled out here
/// rather than imported so a wrong composition in the implementation fails these tests.
pub fn published_registry_authority() -> String {
    ANNOUNCED_REGISTRY_ENDPOINT.to_string()
}

/// The URI Amazon ECR reports for the repository `name`, to a call made through this harness
/// (`Repository$repositoryUri`: "The URI for the repository. You can use this URI for
/// container image `push` and `pull` operations"): the authority the repository's registry is
/// published at ([`published_registry_authority`]) followed by the repository's own name as
/// the path.
///
/// One authority serves every registry this emulator holds, so a repository of another
/// Region's registry is published at the same address — the credential a `/v2` request
/// presents says which registry it addresses.
pub fn published_repository_uri(name: &str) -> String {
    format!("{}/{name}", published_registry_authority())
}

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// One emulator: a router carrying only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator as [`emulator`], plus the engine its handlers read and write — for
/// a case that has to arrange state no reasonable number of wire calls would.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    emulator_served_at_with_engine(SERVED_URL_BASE)
}

/// The same emulator as [`emulator`], SERVED at `served_base` rather than at this harness's
/// own address ([`SERVED_URL_BASE`]) — for a suite whose subject is what the address this
/// emulator is reached at does to the addresses it announces.
pub fn emulator_served_at(served_base: &str) -> MultiServiceRouter {
    emulator_served_at_with_engine(served_base).0
}

/// [`emulator_served_at`], plus the engine its handlers read and write.
pub fn emulator_served_at_with_engine(
    served_base: &str,
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    // The emulator is SERVED somewhere, which is the address every registry address this
    // service announces is built on.
    router.set_url_base(
        ::smithy_server_runtime::EndpointUrl::parse(served_base)
            .unwrap_or_else(|| panic!("the base URL {served_base} must parse")),
    );
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// What the emulator answered: the HTTP status and the JSON body.
pub struct Answer {
    pub status: u16,
    pub body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, or the empty string when the answer carries none.
    ///
    /// Read from whichever member the body carries it in: this service's MODELLED errors
    /// report it under the lowercase `message` their shapes declare, while an AWS COMMON
    /// error — the authorization refusal, rendered in front of the service's own code —
    /// reports it under the capitalised `Message` of that envelope.
    pub fn message(&self) -> &str {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
    }
}

/// Call one operation of this service on the awsJson1_1 wire: `POST /` with
/// `X-Amz-Target: <prefix>.<operation>` and a JSON body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    call_as(router, ACCOUNT_ACCESS_KEY_ID, operation, body).await
}

/// The same call as [`call`], presenting `access_key_id` in the SigV4 credential scope —
/// what a suite that exercises WHO a request speaks for varies, since the access key id is
/// where the calling principal is resolved from.
pub async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", endpoint_host(REGION))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/ecr/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The same call as [`call`], carrying `payload` as the request body EXACTLY as written —
/// for a case about the body's own form, which a JSON value cannot express (a top level that
/// is not an object, a body that is not JSON at all).
pub async fn call_with_payload(
    router: &MultiServiceRouter,
    operation: &str,
    payload: &str,
) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", endpoint_host(REGION))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={ACCOUNT_ACCESS_KEY_ID}/20260101/{REGION}/ecr/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(payload.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful call, or a panic naming the case and what came back instead.
pub async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The value at a JSON pointer, or `null` when the answer carries nothing there — so an
/// assertion reports the member that is missing rather than panicking on the lookup.
pub fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// `GET <path>` at the registry address `authority`, presenting `token` the way a container
/// client presents a `docker login` credential (HTTP Basic, which is what an authorization
/// token decodes to) — the ONE dialling convention this crate's suites use for the Docker
/// Registry v2 plane this registry serves. `accept` is the media type the client offers,
/// where the addressed endpoint serves content.
///
/// The HTTP status and the body, so a suite asserts on what a registry client would see.
pub async fn registry_get(
    router: &MultiServiceRouter,
    authority: &str,
    path: &str,
    token: &str,
    accept: Option<&str>,
) -> (u16, String) {
    let mut request = ::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", authority)
        .header("authorization", format!("Basic {token}"));
    if let Some(media_type) = accept {
        request = request.header("accept", media_type);
    }
    let request = request
        .body(Bytes::new())
        .expect("the registry request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, body)
}

/// How long [`ok_until`] keeps looking before it gives up.
///
/// Generous on purpose: a clock-owed transition is due on the emulator's own clock, and
/// this only bounds a wait that would otherwise never end, so neither a slow nor a loaded
/// host decides the outcome of a poll.
pub const POLL_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(60);

/// How long [`ok_until`] waits between two looks.
pub const POLL_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(10);

/// Call `operation` again and again until its answer satisfies `settled`, and answer the
/// first answer that does.
///
/// A suite waiting for a clock-owed transition waits for the CONDITION rather than for a
/// span of wall clock: it looks again until the transition has happened, bounded only by
/// [`POLL_DEADLINE`].
pub async fn ok_until(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    settled: impl Fn(&Value) -> bool,
) -> Value {
    let since = ::std::time::Instant::now();
    loop {
        let answer = ok(router, case, operation, body).await;
        if settled(&answer) {
            return answer;
        }
        assert!(
            since.elapsed() < POLL_DEADLINE,
            "case {case:?}: {operation} never reached the state this poll waits for; \
             its last answer was {answer:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The wire error code and HTTP status an authorization refusal carries: the AWS "Common
/// Errors" table's `AccessDeniedException`, which this service's model does not declare.
pub const ACCESS_DENIED: (u16, &str) = (400, "AccessDeniedException");

/// The member an AWS COMMON error carries its sentence in: capitalised `Message`, the member
/// of that envelope — unlike the lowercase `message` this service's own modelled errors
/// declare.
pub const COMMON_ERROR_DETAIL: &str = "Message";

/// The principal every call made through [`call`] speaks for: the IAM user this emulator
/// publishes a long-term key for, as `sts:GetCallerIdentity` reports it. Spelled out here
/// rather than imported, so a refusal blaming any other principal fails these suites.
pub const ACCOUNT_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The refusal a call addressed to the registry of ANOTHER account is answered with: an
/// Amazon ECR registry belongs to one account, which shares its resources only through its
/// own resource-based policy, so such a call is an authorization failure naming the caller,
/// the action asked for and the resource in the registry ADDRESSED — the repository's ARN
/// there, or the wildcard for a call that names no repository.
pub fn cross_account_refusal(
    region: &str,
    operation: &str,
    registry_id: &str,
    repository_name: Option<&str>,
) -> String {
    let resource = repository_name.map_or_else(
        || "*".to_string(),
        |name| format!("arn:aws:ecr:{region}:{registry_id}:repository/{name}"),
    );
    format!(
        "User: {ACCOUNT_PRINCIPAL} is not authorized to perform: ecr:{operation} on resource: \
         {resource} because no resource-based policy allows the ecr:{operation} action"
    )
}

/// [`cross_account_refusal`] for a call signed for the Region these suites sign for
/// ([`REGION`]).
pub fn cross_account_refusal_here(
    operation: &str,
    registry_id: &str,
    repository_name: Option<&str>,
) -> String {
    cross_account_refusal(REGION, operation, registry_id, repository_name)
}

/// Assert `operation` was refused as unauthorized because it addressed the registry
/// `registry_id`, which is not this account's: the status, the common error's code and the
/// sentence it carries, as [`cross_account_refusal_here`] words it.
pub async fn refused_cross_account(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    registry_id: &str,
    repository_name: Option<&str>,
) {
    let answer = call(router, operation, body).await;
    let (status, code) = ACCESS_DENIED;
    assert_eq!(
        (answer.status, answer.error_code()),
        (status, code),
        "case {case:?}: expected {status} {code} from {operation}, got {} with {:?}",
        answer.status,
        answer.body
    );
    let expected = cross_account_refusal_here(operation, registry_id, repository_name);
    assert_eq!(
        answer.body[COMMON_ERROR_DETAIL].as_str().unwrap_or_default(),
        expected,
        "case {case:?}: expected the refusal {expected:?} under {COMMON_ERROR_DETAIL} from \
         {operation}, got {:?}",
        answer.body
    );
}

/// The same poll as [`ok_until`], made against `region` and over the WHOLE answer — the
/// status included — so a suite can wait for a call that is refused until the state it
/// waits for exists.
///
/// The one poll loop of this harness, parameterised: [`ok_until`] waits for a successful
/// answer to say something, and this waits for an answer at all, which is what a read of a
/// resource another Region's registry is still receiving needs.
pub async fn call_until_in_region(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
    settled: impl Fn(&Answer) -> bool,
) -> Answer {
    let since = ::std::time::Instant::now();
    loop {
        let answer = call_in_region(router, region, operation, body).await;
        if settled(&answer) {
            return answer;
        }
        assert!(
            since.elapsed() < POLL_DEADLINE,
            "case {case:?}: {operation} in {region} never answered what this poll waits \
             for; its last answer was HTTP {} with {:?}",
            answer.status,
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Follow a URL this emulator handed out exactly as a client follows a pre-signed one — the
/// address it names, no credential of any kind — and answer the status and the bytes served.
///
/// The address is taken apart here rather than in a suite: a URL this emulator announces
/// carries the endpoint the caller reached it on, so the authority to send and the path to
/// request both come out of the URL itself.
pub async fn fetch_url(router: &MultiServiceRouter, case: &str, url: &str) -> (u16, Vec<u8>) {
    let (authority, path) = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .unwrap_or_else(|| panic!("case {case:?}: {url:?} is not an https URL with a path"));
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("/{path}"))
        .header("host", authority)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body().to_vec())
}

/// A refused call: HTTP 400 with the wire error code `code`, or a panic naming the case
/// and what came back instead. The refusal's body is returned for a caller that wants to
/// look at its message.
pub async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected {code} from {operation}, got {:?}",
        answer.body
    );
    answer.body
}

/// The content digest of `content`: the `sha256:<hex>` form a registry addresses a blob
/// by, computed here independently of the crate's own helper.
pub fn layer_digest(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(content)))
}

/// Put `content` into `repository` as a layer, through the documented push sequence —
/// `InitiateLayerUpload`, `UploadLayerPart`, `CompleteLayerUpload` — and answer the
/// digest the registry registered it under, which is the digest a manifest references it
/// by. A manifest cannot be pushed before its blobs are in the repository, so a suite
/// that pushes one seeds its blobs through here.
pub async fn given_layer(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    content: &[u8],
) -> String {
    given_layer_in_region(router, case, REGION, repository, content).await
}

/// The same push sequence as [`given_layer`], made against `region`: a repository lives in
/// the registry of ONE Region, so a suite that created it in another Region pushes its
/// layers there too.
pub async fn given_layer_in_region(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    repository: &str,
    content: &[u8],
) -> String {
    let opened = ok_in_region(
        router,
        case,
        region,
        "InitiateLayerUpload",
        &serde_json::json!({"repositoryName": repository}),
    )
    .await;
    let upload_id = opened["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload must hand back an uploadId"))
        .to_string();
    let last = i64::try_from(content.len()).expect("a test layer fits in an i64") - 1;
    ok_in_region(
        router,
        case,
        region,
        "UploadLayerPart",
        &serde_json::json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "partFirstByte": 0,
            "partLastByte": last,
            // A blob travels base64-encoded on the awsJson1_1 wire, with the codec the
            // wire serializer itself uses.
            "layerPartBlob": aws_smithy_types::base64::encode(content),
        }),
    )
    .await;
    let completed = ok_in_region(
        router,
        case,
        region,
        "CompleteLayerUpload",
        &serde_json::json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [layer_digest(content)],
        }),
    )
    .await;
    completed["layerDigest"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CompleteLayerUpload must report the registered layerDigest")
        })
        .to_string()
}

/// Seed one pull through cache rule straight into the registry's store, under `prefix`
/// as its key.
///
/// `CreatePullThroughCacheRule` is the operation that creates one, and it refuses an
/// upstream URL it cannot resolve to a supported registry while the request boundary
/// refuses a `credentialArn` outside its modelled pattern — so a suite that has to
/// exercise a STORED record the create path cannot produce places it here, in the same
/// store the handlers read. Every other case goes through the wire.
pub fn seed_pull_through_cache_rule(
    engine: &engine::InMemoryEngine,
    prefix: &str,
    upstream_registry: Option<&str>,
    upstream_url: &str,
    credential_arn: Option<&str>,
    custom_role_arn: Option<&str>,
) {
    use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;

    let state = ecr::state::service_state(engine).expect("the ecr store is reachable");
    state.lock().pull_through_cache_rules.insert(
        prefix.to_string(),
        ecr::ops::types::PullThroughCacheRule {
            ecr_repository_prefix: Some(prefix.to_string()),
            upstream_registry_url: Some(upstream_url.to_string()),
            created_at: Some(::aws_smithy_types::DateTime::from(
                ::std::time::SystemTime::now(),
            )),
            registry_id: Some(REGISTRY_ID.to_string()),
            credential_arn: credential_arn.map(ToString::to_string),
            custom_role_arn: custom_role_arn.map(ToString::to_string),
            upstream_repository_prefix: Some("ROOT".to_string()),
            upstream_registry: upstream_registry.map(ToString::to_string),
            updated_at: None,
        },
    );
}

/// Seed, straight into AWS Secrets Manager's own store, the secret a pull through cache
/// rule's `credentialArn` names, holding `value` as its current value.
///
/// Validating a rule asks Secrets Manager whether the secret its `credentialArn` names
/// holds a retrievable credential, so a suite whose rule has to carry a credential of a
/// given state puts that secret where that service keeps it — this emulator's one Secrets
/// Manager store, in the Region the ARN itself names.
pub fn seed_secret_value(engine: &engine::InMemoryEngine, arn: &str, value: &str) {
    use service_com_amazonaws_secretsmanager_secretsmanager::state as secrets;

    let now = ::std::time::SystemTime::now();
    let region = arn.split(':').nth(3).unwrap_or_default().to_string();
    let mut secret = secrets::Secret::new(
        secrets::secret_name_in(arn).to_string(),
        arn.to_string(),
        region,
        now,
    );
    let held = secrets::envelope::SecretValue::from_request(Some(&value.to_string()), None)
        .expect("the seeded secret carries a value");
    secret.put_version(
        secrets::SecretVersion {
            version_id: secrets::new_version_id(),
            value: secrets::envelope::StoredValue::as_sent(&held),
            version_stages: vec![secrets::STAGE_CURRENT.to_string()],
            created_date: now,
            last_accessed_date: None,
            // The seeded secret names no KMS key, so its version is held under the
            // account's default Secrets Manager key.
            kms_key_id: None,
        },
        now,
    );
    let state = secrets::service_state(engine).expect("the secretsmanager store is reachable");
    state.lock().insert(secret);
}

/// The same secret as [`seed_secret_value`], holding the document the Amazon ECR user guide
/// writes an upstream registry credential as: `{"username":…,"accessToken":…}`.
///
/// `deleted` schedules the secret for deletion the way `DeleteSecret` does, which is how a
/// case covers a credential whose secret has gone.
pub fn seed_credential_secret(engine: &engine::InMemoryEngine, arn: &str, deleted: bool) {
    use service_com_amazonaws_secretsmanager_secretsmanager::state as secrets;

    seed_secret_value(
        engine,
        arn,
        r#"{"username":"upstream-user","accessToken":"upstream-token"}"#,
    );
    if !deleted {
        return;
    }
    let now = ::std::time::SystemTime::now();
    let region = arn.split(':').nth(3).unwrap_or_default();
    let state = secrets::service_state(engine).expect("the secretsmanager store is reachable");
    let mut store = state.lock();
    let secret = store
        .get_mut(region, arn)
        .expect("the seeded secret is in the store");
    secret.schedule_deletion(now, now, now);
}

/// The same call as [`call`], made against `region`: the credential scope's region is
/// the Region a request is made against, so a suite exercising regional behaviour picks
/// it per call instead of taking [`REGION`].
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", endpoint_host(region))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ecr/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful [`call_in_region`], or a panic naming the case and what came back.
pub async fn ok_in_region(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let answer = call_in_region(router, region, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}
