// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a code signing configuration: `CreateCodeSigningConfig` "Creates
//! a code signing configuration. A code signing configuration defines a list of allowed
//! signing profiles and defines the code-signing validation policy", and
//! `GetCodeSigningConfig` "Returns information about the specified code signing
//! configuration".
//!
//! The rule under test: `CreateCodeSigningConfig` persists ONE code signing
//! configuration under an identity it mints once — a `CodeSigningConfigId` the model's
//! `CodeSigningConfigId` pattern admits, and the `CodeSigningConfigArn` built from it in
//! the region the caller signed for — carrying the allowed publishers, the code signing
//! policies and the description the request described, with the policy the model
//! documents a default for when the request stated none; and `GetCodeSigningConfig`
//! reports that same stored record, stably across reads, and answers a well-formed ARN
//! naming no configuration with the modelled 404.
//!
//! Every row drives real HTTP requests through the router and reads the configuration
//! back through `GetCodeSigningConfig`, so a configuration that was answered but never
//! stored fails here. The rows differ along what the rule generalises over: the number of
//! signing profiles (each modelled bound of `SigningProfileVersionArns`), whether a
//! policy is stated and which `CodeSigningPolicy` value it carries, the description
//! (absent, empty and at its modelled maximum) and the tags.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCodeSigningConfig` and `GetCodeSigningConfig` are bound, from
/// `smithy_query(query="operation", operation="CreateCodeSigningConfig")`
/// (`POST /2020-04-22/code-signing-configs`) and
/// `smithy_query(query="operation", operation="GetCodeSigningConfig")`
/// (`GET /2020-04-22/code-signing-configs/{CodeSigningConfigArn}`).
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `ListTags` is bound (`smithy_query(query="operation", operation="ListTags")`:
/// `GET /2017-03-31/tags/{Resource}`), which is how a configuration's tags are read back:
/// the `CodeSigningConfig` the create and the get answer carries none.
const TAGS_URI: &str = "/2017-03-31/tags";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The `CodeSigningPolicy` values the model declares, from
/// `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`).
const POLICY_WARN: &str = "Warn";
/// See [`POLICY_WARN`].
const POLICY_ENFORCE: &str = "Enforce";

/// The policy the model documents for a configuration whose request stated none:
/// `CodeSigningPolicies$UntrustedArtifactOnDeployment` says "Default value: `Warn`".
const DEFAULT_POLICY: &str = POLICY_WARN;

/// The bounds the model puts on `AllowedPublishers$SigningProfileVersionArns`
/// (`@length(min: 1, max: 20)`), from
/// `smithy_query(query="shape", shape="SigningProfileVersionArns")`.
const PROFILES_MIN: usize = 1;
const PROFILES_MAX: usize = 20;

/// The longest `Description` the model admits (`@length(min: 0, max: 256)`), from
/// `smithy_query(query="shape", shape="Description")`.
const DESCRIPTION_MAX: usize = 256;

/// How long a `CodeSigningConfigId` is after its `csc-` prefix, from the model's
/// `CodeSigningConfigId` pattern (`^csc-[a-zA-Z0-9-_\.]{17}$`).
const ID_SUFFIX_LEN: usize = 17;

/// A well-formed `CodeSigningConfigArn` for a configuration this account does not hold:
/// `csc-` and 17 characters, as the member's own pattern
/// (`code-signing-config:csc-[a-z0-9]{17}`) spells it.
const ABSENT_ARN: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdef0";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `CodeSigningConfig` member of the body, which both operations answer with.
    fn config(&self) -> Value {
        self.json()
            .get("CodeSigningConfig")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// A `String` member of the answered `CodeSigningConfig`.
    fn config_string(&self, member: &str) -> Option<String> {
        self.config()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `CreateCodeSigningConfig`.
async fn create_config(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", CODE_SIGNING_CONFIGS_URI, body).await
}

/// `GetCodeSigningConfig` on the configuration `arn` names. The ARN's `:` separators are
/// legal in a path segment, so it goes into the path as given.
async fn get_config(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
        &Value::Null,
    )
    .await
}

/// `ListTags` on the resource `arn` names.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("{TAGS_URI}/{arn}"), &Value::Null).await
}

/// A signing profile version ARN satisfying the model's `Arn` pattern
/// (`^arn:(aws[a-zA-Z0-9-]*):([a-zA-Z0-9\-])+:(...)?:(\d{12})?:(.*)$`), in the form AWS
/// Signer vends one.
fn signing_profile(index: usize) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/Profile{index}/{index:08x}")
}

/// `count` signing profile version ARNs, as a JSON list.
fn signing_profiles(count: usize) -> Value {
    Value::Array((0..count).map(|index| json!(signing_profile(index))).collect())
}

/// The ARN a code signing configuration of `id` carries, as the model's
/// `CodeSigningConfigArn` pattern spells it.
fn config_arn(id: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:code-signing-config:{id}")
}

/// One row of the code-signing table: the request, and what the rule says the stored
/// configuration then reports.
struct ConfigCase {
    name: &'static str,
    request: Value,
    /// The `SigningProfileVersionArns` the record must hold, in request order.
    expected_profiles: Vec<String>,
    /// The `UntrustedArtifactOnDeployment` the record must report.
    expected_policy: &'static str,
    /// The `Description` the CREATE's own answer must report; `None` for a request that
    /// stated none, whose echo carries no such member. Every later READ of the record
    /// renders it, as the empty string — see [`read_description`].
    expected_description: Option<String>,
    /// The tags the record must then hold (they are not reported on the wire).
    expected_tags: Vec<(String, String)>,
}

/// The rows. Each differs from the others in the number of signing profiles, the policy
/// it states, the description or the tags — the dimensions the rule generalises over.
fn config_cases() -> Vec<ConfigCase> {
    vec![
        // Nothing but the required member, the profile list at its modelled minimum and
        // no policy at all: the answer carries the documented default policy and no
        // description.
        ConfigCase {
            name: "only the required allowed publishers, with the fewest profiles",
            request: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(PROFILES_MIN) },
            }),
            expected_profiles: (0..PROFILES_MIN).map(signing_profile).collect(),
            expected_policy: DEFAULT_POLICY,
            expected_description: None,
            expected_tags: Vec::new(),
        },
        // Every optional member stated, the profile list at its modelled maximum, the
        // stricter policy, the longest description the model admits and tags.
        ConfigCase {
            name: "every optional member, with the most profiles and the longest description",
            request: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(PROFILES_MAX) },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE },
                "Description": "d".repeat(DESCRIPTION_MAX),
                "Tags": { "team": "platform", "env": "test" },
            }),
            expected_profiles: (0..PROFILES_MAX).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some("d".repeat(DESCRIPTION_MAX)),
            expected_tags: vec![
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "platform".to_string()),
            ],
        },
        // The other `CodeSigningPolicy` value, stated explicitly, with an empty
        // description (the bottom of the member's modelled length range).
        ConfigCase {
            name: "the warning policy stated explicitly, with an empty description",
            request: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(2) },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_WARN },
                "Description": "",
            }),
            expected_profiles: (0..2).map(signing_profile).collect(),
            expected_policy: POLICY_WARN,
            expected_description: Some(String::new()),
            expected_tags: Vec::new(),
        },
        // An empty `CodeSigningPolicies` structure: the member inside it is optional, so
        // the configuration still carries the documented default.
        ConfigCase {
            name: "a policy structure stating no policy at all",
            request: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(3) },
                "CodeSigningPolicies": {},
                "Description": "three profiles, no policy member",
                "Tags": { "single": "" },
            }),
            expected_profiles: (0..3).map(signing_profile).collect(),
            expected_policy: DEFAULT_POLICY,
            expected_description: Some("three profiles, no policy member".to_string()),
            expected_tags: vec![("single".to_string(), String::new())],
        },
        // A second row with the same request shape as the first: the identity is minted
        // per configuration, so this one is a DIFFERENT configuration rather than the
        // same record again.
        ConfigCase {
            name: "a second configuration with the same allowed publishers",
            request: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(PROFILES_MIN) },
                "Description": "the same publishers as the first row",
            }),
            expected_profiles: (0..PROFILES_MIN).map(signing_profile).collect(),
            expected_policy: DEFAULT_POLICY,
            expected_description: Some("the same publishers as the first row".to_string()),
            expected_tags: Vec::new(),
        },
    ]
}

/// The `Description` a READ of the configuration reports: the one it was described with, and
/// the empty string for a configuration nobody described, which is the member every read of
/// the stored record renders (the create echoes its own request instead).
fn read_description(expected: Option<&String>) -> String {
    expected.cloned().unwrap_or_default()
}

/// The members of an answered `CodeSigningConfig` the rule says come out of the stored
/// record, as a comparable value.
///
/// `Description` is normalised to the value a READ renders, so a create's echo and a read of
/// the same record compare equal on every member they report the same way.
fn reported(answer: &Answer) -> Value {
    let config = answer.config();
    json!({
        "CodeSigningConfigId": config["CodeSigningConfigId"].clone(),
        "CodeSigningConfigArn": config["CodeSigningConfigArn"].clone(),
        "Description": json!(config["Description"].as_str().unwrap_or_default()),
        "AllowedPublishers": config["AllowedPublishers"].clone(),
        "CodeSigningPolicies": config["CodeSigningPolicies"].clone(),
        "LastModified": config["LastModified"].clone(),
    })
}

#[tokio::test]
async fn a_created_code_signing_config_is_persisted_and_reported() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for case in config_cases() {
        let label = case.name;
        let created = create_config(&router, &case.request).await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: expected 201 from CreateCodeSigningConfig, got {} {}",
            created.status,
            created.text()
        );

        // The identity: an id the model's pattern admits, minted once per configuration,
        // and the ARN built from it in the region the caller signed for.
        let id = created.config_string("CodeSigningConfigId").unwrap_or_default();
        assert!(
            id.starts_with("csc-")
                && id.len() == "csc-".len() + ID_SUFFIX_LEN
                && id["csc-".len()..]
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
            "case {label:?}: expected a csc- id of {ID_SUFFIX_LEN} further characters, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {label:?}: every configuration gets its own id, but {id:?} was minted twice"
        );
        let arn = config_arn(&id);
        assert_eq!(
            created.config_string("CodeSigningConfigArn"),
            Some(arn.clone()),
            "case {label:?}: expected the ARN {arn:?}, got {:?}",
            created.config_string("CodeSigningConfigArn")
        );

        // The configuration the request described.
        let expected_publishers = json!({
            "SigningProfileVersionArns": Value::Array(
                case.expected_profiles.iter().map(|profile| json!(profile)).collect(),
            ),
        });
        assert_eq!(
            created.config()["AllowedPublishers"],
            expected_publishers,
            "case {label:?}: expected the allowed publishers {expected_publishers}, got {}",
            created.text()
        );
        assert_eq!(
            created
                .config()
                .get("CodeSigningPolicies")
                .and_then(|policies| policies.get("UntrustedArtifactOnDeployment"))
                .and_then(Value::as_str),
            Some(case.expected_policy),
            "case {label:?}: expected the policy {:?}, got {}",
            case.expected_policy,
            created.text()
        );
        assert_eq!(
            created.config_string("Description"),
            case.expected_description,
            "case {label:?}: expected the description {:?}, got {:?}",
            case.expected_description,
            created.config_string("Description")
        );
        let last_modified = created.config_string("LastModified").unwrap_or_default();
        assert!(
            last_modified.starts_with("20") && last_modified.ends_with("+0000"),
            "case {label:?}: expected an ISO-8601 LastModified carrying the numeric UTC \
             offset this service renders (the form the model's own instant example, \
             `2018-11-27T15:10:45.123+0000`, spells), got {last_modified:?}"
        );

        // Read the configuration back: one that was answered but never persisted fails
        // here, and a read that changed it fails the second read.
        let first = get_config(&router, &arn).await;
        assert_eq!(
            first.status,
            200,
            "case {label:?}: expected 200 from GetCodeSigningConfig on {arn:?}, got {} {}",
            first.status,
            first.text()
        );
        assert_eq!(
            reported(&first),
            reported(&created),
            "case {label:?}: expected GetCodeSigningConfig to report what the create \
             persisted, {}, got {}",
            created.text(),
            first.text()
        );
        // ... and the read RENDERS `Description`, as the empty string for a configuration
        // nobody described.
        assert_eq!(
            first.config_string("Description"),
            Some(read_description(case.expected_description.as_ref())),
            "case {label:?}: expected GetCodeSigningConfig to report the description {:?}, \
             got {}",
            read_description(case.expected_description.as_ref()),
            first.text()
        );
        let second = get_config(&router, &arn).await;
        assert_eq!(
            reported(&second),
            reported(&first),
            "case {label:?}: two reads of an unchanged configuration report the same \
             thing, got {} then {}",
            first.text(),
            second.text()
        );

        // The tags the request stated are on the record, read back through `ListTags`:
        // the `CodeSigningConfig` shape reports none.
        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            tags.status,
            200,
            "case {label:?}: expected 200 from ListTags on {arn:?}, got {} {}",
            tags.status,
            tags.text()
        );
        let expected_tags = Value::Object(
            case.expected_tags
                .iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect(),
        );
        assert_eq!(
            tags.json().get("Tags").cloned().unwrap_or(json!({})),
            expected_tags,
            "case {label:?}: expected the tags {expected_tags}, got {}",
            tags.text()
        );
    }
}

#[tokio::test]
async fn a_code_signing_config_arn_naming_no_configuration_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Nothing has been created, so even a well-formed ARN names no configuration.
    let missing = get_config(&router, ABSENT_ARN).await;
    assert_eq!(
        missing.status,
        404,
        "case {ABSENT_ARN:?}: expected 404 from GetCodeSigningConfig on a configuration \
         this account does not hold, got {} {}",
        missing.status,
        missing.text()
    );
    assert!(
        missing.error_code().contains("ResourceNotFoundException"),
        "case {ABSENT_ARN:?}: expected ResourceNotFoundException, got {}",
        missing.error_code()
    );

    // One configuration exists; a DIFFERENT well-formed ARN still names none of it.
    let created = create_config(
        &router,
        &json!({
            "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "expected 201 from CreateCodeSigningConfig, got {} {}",
        created.status,
        created.text()
    );
    let still_missing = get_config(&router, ABSENT_ARN).await;
    assert_eq!(
        still_missing.status,
        404,
        "case {ABSENT_ARN:?}: expected 404 even once another configuration exists, got {} {}",
        still_missing.status,
        still_missing.text()
    );

    // A value that is no code-signing-config ARN at all is a client refusal, never a
    // report of some other configuration.
    let malformed = get_config(&router, "not-a-code-signing-config-arn").await;
    assert_eq!(
        malformed.status,
        400,
        "case \"not-a-code-signing-config-arn\": expected 400 from GetCodeSigningConfig, \
         got {} {}",
        malformed.status,
        malformed.text()
    );
}

#[tokio::test]
async fn every_created_code_signing_config_is_its_own_record_in_the_store() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The same request, three times: each call creates its own configuration, so the
    // store holds three records rather than one overwritten one.
    let mut arns: Vec<String> = Vec::new();
    for round in 0..3 {
        let created = create_config(
            &router,
            &json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
                "Description": format!("round {round}"),
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {round}: expected 201 from CreateCodeSigningConfig, got {} {}",
            created.status,
            created.text()
        );
        arns.push(
            created
                .config_string("CodeSigningConfigArn")
                .unwrap_or_default(),
        );
    }

    let stored = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .code_signing_configs
        .len();
    assert_eq!(
        stored, 3,
        "expected three stored code signing configurations, got {stored}"
    );

    // Each ARN still reports its own description, so no call replaced another's record.
    for (round, arn) in arns.iter().enumerate() {
        let fetched = get_config(&router, arn).await;
        assert_eq!(
            fetched.config_string("Description"),
            Some(format!("round {round}")),
            "case {round}: expected {arn:?} to still report its own description, got {}",
            fetched.text()
        );
    }
}
