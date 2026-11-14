// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT custom-authorizer store:
//! `CreateAuthorizer`, the `DescribeAuthorizer` reader that proves what it persisted,
//! `UpdateAuthorizer` and `DeleteAuthorizer`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: a single-resource reader of this control plane reports the
//! record its create persisted under that name, member for member, and refuses a name
//! the store does not hold with the operation's modelled not-found error instead of a
//! success. Here that means `CreateAuthorizer` RECORDS an authorizer under its name
//! and `DescribeAuthorizer` reports that record as an `AuthorizerDescription` — the
//! ARN (`arn:aws:iot:<region>:<account>:authorizer/<authorizerName>`), the Lambda
//! function ARN, the token key name and signing public keys, the status, and the
//! `creationDate` / `lastModifiedDate` the create stamped — with the optional members
//! resolved on the write (an absent `status` is `ACTIVE`, an absent `signingDisabled`
//! leaves signature validation on, an absent `enableCachingForHttp` is `false`) and
//! stable across reads. A second create under a name the store already holds is
//! refused `ResourceAlreadyExistsException` (409) and changes nothing; a name the
//! store does not hold is refused `ResourceNotFoundException` (404) by both
//! `DescribeAuthorizer` and `DeleteAuthorizer`.
//!
//! The table spans what that rule generalises over — the form and length of an
//! authorizer name, whether the create carries a token key name, how many signing
//! public keys it carries, which status it asks for, which optional booleans it
//! carries, whether it carries tags, and the region the call is signed for — rather
//! than one recorded request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    DEFAULT_AUTHORIZER_STATUS, MAX_AUTHORIZERS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `authorizerDescription` member of a `DescribeAuthorizer` response.
    fn description(&self) -> Option<&Value> {
        self.body.get("authorizerDescription")
    }

    /// A member of the reported `AuthorizerDescription`, or `None` when the
    /// description or the member is absent.
    fn member(&self, name: &str) -> Option<&Value> {
        self.description().and_then(|value| value.get(name))
    }

    /// A string member of the reported `AuthorizerDescription`.
    fn member_str(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// A timestamp member as the epoch seconds the restJson1 protocol renders a
    /// timestamp as.
    fn member_epoch(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create(&self, name: &str, body: &Value) -> Reply {
        self.create_in(name, body, REGION).await
    }

    async fn create_in(&self, name: &str, body: &Value, region: &str) -> Reply {
        self.call("POST", &format!("/authorizer/{name}"), Some(body), region)
            .await
    }

    async fn describe(&self, name: &str) -> Reply {
        self.call("GET", &format!("/authorizer/{name}"), None, REGION)
            .await
    }

    async fn delete(&self, name: &str) -> Reply {
        self.call("DELETE", &format!("/authorizer/{name}"), None, REGION)
            .await
    }

    /// `UpdateAuthorizer`, bound to `PUT /authorizer/{authorizerName}` by this service's
    /// own model.
    async fn update(&self, name: &str, body: &Value) -> Reply {
        self.call("PUT", &format!("/authorizer/{name}"), Some(body), REGION)
            .await
    }

    /// Takes the authorizer out of service, which is what `UpdateAuthorizer`'s `status`
    /// member is for and what a delete of it requires first.
    async fn deactivate(&self, name: &str) -> Reply {
        self.update(name, &json!({ "status": "INACTIVE" })).await
    }
}

/// The ARN an authorizer created in `region` must carry.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:authorizer/{name}")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One create request: a label, the authorizer name, the region it is signed for, the
/// request body, and what the body's optional members resolve to once stored.
struct Case {
    label: &'static str,
    name: String,
    region: &'static str,
    body: Value,
    /// What `authorizerDescription.status` must report: the body's value, or
    /// [`DEFAULT_AUTHORIZER_STATUS`] when it carried none.
    status: &'static str,
    /// What `authorizerDescription.signingDisabled` must report: the body's value, or
    /// `false` when it carried none.
    signing_disabled: bool,
    /// What `authorizerDescription.enableCachingForHttp` must report: the body's
    /// value, or `false` when it carried none —
    /// `CreateAuthorizer$enableCachingForHttp` documents "The default value is
    /// false".
    enable_caching_for_http: bool,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `authorizerName` — modelled
///   `length 1..128, pattern ^[\w=,@-]+$`
///   (`smithy_query(query="operation",operation="DescribeAuthorizer")`), so the rows
///   include the shortest legal name, the longest, and one using every character
///   class the pattern allows;
/// * which status the create asks for — `AuthorizerStatus` is `ACTIVE | INACTIVE`
///   (`smithy_query(query="shape",shape="AuthorizerStatus")`), so the rows cover both
///   values and the absent member;
/// * whether the create carries `tokenKeyName` and how many `tokenSigningPublicKeys`
///   it carries — `PublicKeyMap` is a map of `KeyName` to `KeyValue`
///   (`smithy_query(query="shape",shape="PublicKeyMap")`), so the rows cover none,
///   one and three keys;
/// * which optional booleans it carries — `signingDisabled` and
///   `enableCachingForHttp`, each absent, `false` and `true`;
/// * whether it carries `tags`, which manage the authorizer rather than describe it
///   and so appear in no `AuthorizerDescription` member;
/// * the region the call is signed for, which is the region the authorizer's ARN is
///   scoped to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the required function ARN",
            name: "minimal_authorizer".to_string(),
            region: REGION,
            body: json!({
                "authorizerFunctionArn":
                    "arn:aws:lambda:us-east-1:000000000000:function:authorize",
            }),
            status: DEFAULT_AUTHORIZER_STATUS,
            signing_disabled: false,
            enable_caching_for_http: false,
        },
        Case {
            label: "an explicitly ACTIVE authorizer with one signing public key",
            name: "active_signed".to_string(),
            region: REGION,
            body: json!({
                "authorizerFunctionArn":
                    "arn:aws:lambda:us-east-1:000000000000:function:authorize-signed",
                "status": "ACTIVE",
                "tokenKeyName": "X-Amz-Custom-Token",
                "tokenSigningPublicKeys": {
                    "FIRST_KEY": "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----",
                },
                "signingDisabled": false,
            }),
            status: "ACTIVE",
            signing_disabled: false,
            enable_caching_for_http: false,
        },
        Case {
            label: "an INACTIVE authorizer with signing disabled and no token key",
            name: "inactive-unsigned".to_string(),
            region: REGION,
            body: json!({
                "authorizerFunctionArn":
                    "arn:aws:lambda:us-east-1:000000000000:function:authorize-unsigned",
                "status": "INACTIVE",
                "signingDisabled": true,
            }),
            status: "INACTIVE",
            signing_disabled: true,
            enable_caching_for_http: false,
        },
        Case {
            label: "three signing public keys and HTTP caching enabled",
            name: "cached_keys".to_string(),
            region: REGION,
            body: json!({
                "authorizerFunctionArn":
                    "arn:aws:lambda:us-east-1:000000000000:function:authorize-cached",
                "tokenKeyName": "token",
                "tokenSigningPublicKeys": {
                    "key-1": "-----BEGIN PUBLIC KEY-----\nBBBB\n-----END PUBLIC KEY-----",
                    "key_2": "-----BEGIN PUBLIC KEY-----\nCCCC\n-----END PUBLIC KEY-----",
                    "key:3": "",
                },
                "enableCachingForHttp": true,
            }),
            status: DEFAULT_AUTHORIZER_STATUS,
            signing_disabled: false,
            enable_caching_for_http: true,
        },
        Case {
            label: "an authorizer created with tags and caching explicitly off",
            name: "tagged=authorizer,1".to_string(),
            region: REGION,
            body: json!({
                "authorizerFunctionArn":
                    "arn:aws:lambda:us-east-1:000000000000:function:authorize-tagged",
                "enableCachingForHttp": false,
                "tags": [
                    { "Key": "team", "Value": "platform" },
                    { "Key": "env", "Value": "test" },
                ],
            }),
            status: DEFAULT_AUTHORIZER_STATUS,
            signing_disabled: false,
            enable_caching_for_http: false,
        },
        Case {
            label: "a name using every character class the pattern allows",
            name: "Auth_01=aZ,x@y-z".to_string(),
            region: REGION,
            body: json!({
                "authorizerFunctionArn":
                    "arn:aws:lambda:us-east-1:000000000000:function:authorize-classes",
                "tokenKeyName": "Token_Key-1",
                "tokenSigningPublicKeys": {
                    "KEY": "-----BEGIN PUBLIC KEY-----\nDDDD\n-----END PUBLIC KEY-----",
                },
            }),
            status: DEFAULT_AUTHORIZER_STATUS,
            signing_disabled: false,
            enable_caching_for_http: false,
        },
        Case {
            label: "the shortest legal authorizer name, signed for another region",
            name: "a".to_string(),
            region: "eu-west-1",
            body: json!({
                "authorizerFunctionArn":
                    "arn:aws:lambda:eu-west-1:000000000000:function:authorize-eu",
                "status": "INACTIVE",
            }),
            status: "INACTIVE",
            signing_disabled: false,
            enable_caching_for_http: false,
        },
        Case {
            label: "the longest legal authorizer name",
            name: "a".repeat(128),
            region: REGION,
            body: json!({
                "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:long",
                "signingDisabled": true,
                "enableCachingForHttp": true,
            }),
            status: DEFAULT_AUTHORIZER_STATUS,
            signing_disabled: true,
            enable_caching_for_http: true,
        },
    ]
}

#[tokio::test]
async fn a_created_authorizer_round_trips_through_describe_authorizer() {
    let emulator = Emulator::new();
    let before = now_epoch_seconds();

    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateAuthorizer must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("authorizerName").as_deref(),
            Some(case.name.as_str()),
            "case {label:?}: the create must report the name it recorded, got {created:?}"
        );
        assert_eq!(
            created.text("authorizerArn"),
            Some(expected_arn(case.region, &case.name)),
            "case {label:?}: the create must report the minted ARN, got {created:?}"
        );

        let read = emulator.describe(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: DescribeAuthorizer must find the created authorizer, got {read:?}"
        );
        assert_eq!(
            read.member_str("authorizerName"),
            Some(case.name.as_str()),
            "case {label:?}: the description must report the created name, got {read:?}"
        );
        assert_eq!(
            read.member_str("authorizerArn"),
            Some(expected_arn(case.region, &case.name).as_str()),
            "case {label:?}: the description must report the created ARN, got {read:?}"
        );
        assert_eq!(
            read.member("authorizerFunctionArn"),
            case.body.get("authorizerFunctionArn"),
            "case {label:?}: the description must report the persisted function ARN, got {read:?}"
        );
        assert_eq!(
            read.member("tokenKeyName"),
            case.body.get("tokenKeyName"),
            "case {label:?}: the description must report exactly the persisted tokenKeyName, \
             got {read:?}"
        );
        assert_eq!(
            read.member("tokenSigningPublicKeys"),
            case.body.get("tokenSigningPublicKeys"),
            "case {label:?}: the description must report the persisted signing public keys, \
             got {read:?}"
        );
        assert_eq!(
            read.member_str("status"),
            Some(case.status),
            "case {label:?}: expected status {:?}, got {read:?}",
            case.status
        );
        assert_eq!(
            read.member("signingDisabled").and_then(Value::as_bool),
            Some(case.signing_disabled),
            "case {label:?}: expected signingDisabled {}, got {read:?}",
            case.signing_disabled
        );
        assert_eq!(
            read.member("enableCachingForHttp").and_then(Value::as_bool),
            Some(case.enable_caching_for_http),
            "case {label:?}: expected enableCachingForHttp {}, got {read:?}",
            case.enable_caching_for_http
        );
        // Tags manage the authorizer rather than describe it, so no
        // `AuthorizerDescription` member reports them.
        assert!(
            read.member("tags").is_none(),
            "case {label:?}: AuthorizerDescription models no tags member, got {read:?}"
        );

        // `creationDate` is the instant of the create: it lies between the instant
        // before the first create and the instant after this read, so it is a real
        // clock reading rather than a fixed or zero timestamp. Nothing has updated the
        // authorizer, so `lastModifiedDate` is that same instant.
        let created_at = read.member_epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the description reported no creationDate: {read:?}")
        });
        let after = now_epoch_seconds();
        assert!(
            (before - 1.0..=after + 1.0).contains(&created_at),
            "case {label:?}: creationDate {created_at} is outside [{before}, {after}], got {read:?}"
        );
        assert_eq!(
            read.member_epoch("lastModifiedDate"),
            Some(created_at),
            "case {label:?}: an authorizer nothing has updated was last modified when it was \
             created ({created_at}), got {read:?}"
        );
    }
}

#[tokio::test]
async fn reading_an_authorizer_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    // `DescribeAuthorizer` reports the authorizer; it must not re-time, re-mint or
    // otherwise rewrite it, so two reads of one authorizer are byte-identical.
    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateAuthorizer must succeed, got {created:?}"
        );

        let first = emulator.describe(&case.name).await;
        let second = emulator.describe(&case.name).await;
        assert_eq!(
            first.status, second.status,
            "case {label:?}: both reads must answer the same status, got {second:?}"
        );
        assert_eq!(
            first.body, second.body,
            "case {label:?}: a read must not change the authorizer, got {first:?} then {second:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_create_of_one_name_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    // The name is the authorizer's identity, so a second create under a name the
    // store holds is refused 409 `ResourceAlreadyExistsException` whatever the second
    // body says — and the first authorizer is left exactly as it was.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the same body again",
            "repeat_same",
            json!({ "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:a" }),
        ),
        (
            "a different function ARN",
            "repeat_function",
            json!({ "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:b" }),
        ),
        (
            "a body that asks for the other status",
            "repeat_status",
            json!({
                "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:a",
                "status": "INACTIVE",
            }),
        ),
        (
            "a body that adds signing public keys",
            "repeat_keys",
            json!({
                "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:a",
                "tokenKeyName": "token",
                "tokenSigningPublicKeys": { "KEY": "pem" },
            }),
        ),
    ];

    for (label, name, second_body) in rows {
        let first_body =
            json!({ "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:a" });
        let first = emulator.create(name, &first_body).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateAuthorizer must succeed, got {first:?}"
        );
        let before = emulator.describe(name).await;

        let second = emulator.create(name, &second_body).await;
        assert_eq!(
            second.status, 409,
            "case {label:?}: a repeat create must be refused 409, got {second:?}"
        );
        assert_eq!(
            second.error_code.as_deref(),
            Some("ResourceAlreadyExistsException"),
            "case {label:?}: expected ResourceAlreadyExistsException, got {second:?}"
        );
        // The refusal is built from the request: it names the authorizer that exists
        // and the ARN it was minted with.
        assert_eq!(
            second.text("resourceId").as_deref(),
            Some(name),
            "case {label:?}: the refusal must name the authorizer that exists, got {second:?}"
        );
        assert_eq!(
            second.text("resourceArn"),
            Some(expected_arn(REGION, name)),
            "case {label:?}: the refusal must name the existing authorizer's ARN, got {second:?}"
        );

        let after = emulator.describe(name).await;
        assert_eq!(
            before.body, after.body,
            "case {label:?}: the refused create must leave the authorizer untouched, got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_authorizer_the_store_does_not_hold_is_refused_not_found() {
    let emulator = Emulator::new();

    // One authorizer exists, so the refusals below are about the NAME not being in
    // the store rather than the store being empty.
    let created = emulator
        .create(
            "existing_authorizer",
            &json!({ "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:a" }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateAuthorizer must succeed, got {created:?}"
    );

    for name in [
        "a",
        "unknown_authorizer",
        "existing_authorizer_2",
        "EXISTING_AUTHORIZER",
        &"a".repeat(128),
    ] {
        let read = emulator.describe(name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: DescribeAuthorizer of an unknown name must be refused 404, \
             got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {read:?}"
        );
        assert!(
            read.description().is_none(),
            "case {name:?}: a refusal carries no authorizerDescription, got {read:?}"
        );

        let deleted = emulator.delete(name).await;
        assert_eq!(
            deleted.status, 404,
            "case {name:?}: DeleteAuthorizer of an unknown name must be refused 404, \
             got {deleted:?}"
        );
        assert_eq!(
            deleted.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {deleted:?}"
        );
    }

    // The authorizer that does exist is still readable: the refusals above wrote
    // nothing.
    let read = emulator.describe("existing_authorizer").await;
    assert_eq!(
        read.status, 200,
        "the existing authorizer must still be readable, got {read:?}"
    );
}

#[tokio::test]
async fn a_deleted_authorizer_is_gone_and_its_name_is_free_again() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateAuthorizer must succeed, got {created:?}"
        );

        // A delete is admissible only once the authorizer is out of service, so the
        // teardown of one walks the documented path: deactivate it, then delete it.
        let deactivated = emulator.deactivate(&case.name).await;
        assert_eq!(
            deactivated.status, 200,
            "case {label:?}: deactivating the authorizer must succeed, got {deactivated:?}"
        );

        let deleted = emulator.delete(&case.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteAuthorizer must succeed, got {deleted:?}"
        );
        // `DeleteAuthorizer`'s output shape models no members, so a successful delete
        // reports an empty object.
        assert_eq!(
            deleted.body,
            json!({}),
            "case {label:?}: DeleteAuthorizer models no output members, got {deleted:?}"
        );

        let read = emulator.describe(&case.name).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: a deleted authorizer must no longer be readable, got {read:?}"
        );

        // The name is free again: a create under it succeeds and records a NEW
        // authorizer, which is why the delete removed the record rather than marking
        // it.
        let recreated = emulator
            .create_in(
                &case.name,
                &json!({
                    "authorizerFunctionArn":
                        "arn:aws:lambda:us-east-1:000000000000:function:recreated",
                }),
                case.region,
            )
            .await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: the deleted name must be free again, got {recreated:?}"
        );
        let read = emulator.describe(&case.name).await;
        assert_eq!(
            read.member_str("authorizerFunctionArn"),
            Some("arn:aws:lambda:us-east-1:000000000000:function:recreated"),
            "case {label:?}: the re-created authorizer must report its own record, got {read:?}"
        );
    }
}

#[tokio::test]
async fn deleting_one_authorizer_leaves_every_other_one_readable() {
    let emulator = Emulator::new();
    let all = cases();

    for case in &all {
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateAuthorizer must succeed, got {created:?}",
            case.label
        );
    }

    // The store is keyed by name, so a delete removes exactly one record: every other
    // authorizer still describes as it did before.
    for (index, case) in all.iter().enumerate() {
        let label = case.label;
        let deactivated = emulator.deactivate(&case.name).await;
        assert_eq!(
            deactivated.status, 200,
            "case {label:?}: deactivating the authorizer must succeed, got {deactivated:?}"
        );
        let deleted = emulator.delete(&case.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteAuthorizer must succeed, got {deleted:?}"
        );
        for other in all.iter().skip(index + 1) {
            let read = emulator.describe(&other.name).await;
            assert_eq!(
                read.status,
                200,
                "case {label:?}: deleting it must leave {:?} readable, got {read:?}",
                other.label
            );
            assert_eq!(
                read.member_str("authorizerName"),
                Some(other.name.as_str()),
                "case {label:?}: {:?} must still report its own record, got {read:?}",
                other.label
            );
        }
    }
}

#[tokio::test]
async fn the_authorizer_store_is_bounded() {
    let emulator = Emulator::new();
    let body = json!({ "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:a" });

    // The store is one a caller grows, so it has a ceiling: creates up to
    // `MAX_AUTHORIZERS` succeed, and every create past it is refused with the modelled
    // `LimitExceededException` (HTTP 410) while the authorizers already recorded stay
    // readable.
    for index in 0..MAX_AUTHORIZERS {
        let created = emulator
            .create(&format!("bounded_authorizer_{index}"), &body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {index}: a create below the ceiling must succeed, got {created:?}"
        );
    }

    for name in ["one_too_many", "another_too_many"] {
        let refused = emulator.create(name, &body).await;
        assert_eq!(
            refused.status, 410,
            "case {name:?}: a create past the ceiling must be refused 410, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("LimitExceededException"),
            "case {name:?}: expected LimitExceededException, got {refused:?}"
        );
        // The refused create recorded nothing under that name.
        let read = emulator.describe(name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: the refused create must record no authorizer, got {read:?}"
        );
    }

    // The authorizers that were recorded are untouched by the refusals.
    for index in [0, MAX_AUTHORIZERS / 2, MAX_AUTHORIZERS - 1] {
        let read = emulator
            .describe(&format!("bounded_authorizer_{index}"))
            .await;
        assert_eq!(
            read.status, 200,
            "case {index}: a recorded authorizer must still be readable, got {read:?}"
        );
    }
}

/// `UpdateAuthorizer` applies the members the request states to the authorizer that name
/// holds, leaves the ones it does not state exactly as they were, and keeps the
/// authorizer's identity — its name and the ARN its create minted.
///
/// The rows span what the rule generalises over: which member the update states (the
/// function ARN, the status, the token key name, the signing public keys, the caching
/// flag), how many it states at once, and a request that states none at all.
#[tokio::test]
async fn an_update_applies_the_members_it_states_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let name = "updated_authorizer";
    let created = json!({
        "authorizerFunctionArn": format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:first"),
        "tokenKeyName": "token",
        "tokenSigningPublicKeys": {"FIRST_KEY": "-----BEGIN PUBLIC KEY-----first"},
        "status": "ACTIVE",
        "signingDisabled": false,
        "enableCachingForHttp": false,
    });
    let create = emulator.create(name, &created).await;
    assert_eq!(
        create.status, 200,
        "case \"the create the updates are applied to\": expected 200, got {create:?}"
    );

    // One row of the table below: a case name, the update's body, and the members the
    // authorizer must then report.
    struct Row {
        case: &'static str,
        body: Value,
        expected: Vec<(&'static str, Value)>,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "a request that states nothing: every member is left as it was",
            body: json!({}),
            expected: vec![
                (
                    "authorizerFunctionArn",
                    json!(format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:first")),
                ),
                ("tokenKeyName", json!("token")),
                ("status", json!("ACTIVE")),
                ("signingDisabled", json!(false)),
                ("enableCachingForHttp", json!(false)),
            ],
        },
        Row {
            case: "only the status",
            body: json!({"status": "INACTIVE"}),
            expected: vec![
                ("status", json!("INACTIVE")),
                (
                    "authorizerFunctionArn",
                    json!(format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:first")),
                ),
                ("tokenKeyName", json!("token")),
            ],
        },
        Row {
            case: "the function ARN and the caching flag together",
            body: json!({
                "authorizerFunctionArn":
                    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:second"),
                "enableCachingForHttp": true,
            }),
            expected: vec![
                (
                    "authorizerFunctionArn",
                    json!(format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:second")),
                ),
                ("enableCachingForHttp", json!(true)),
                // Still the status the row before set, and still the first token key.
                ("status", json!("INACTIVE")),
                ("tokenKeyName", json!("token")),
            ],
        },
        Row {
            case: "the token key name and the signing public keys",
            body: json!({
                "tokenKeyName": "other-token",
                "tokenSigningPublicKeys": {"SECOND_KEY": "-----BEGIN PUBLIC KEY-----second"},
            }),
            expected: vec![
                ("tokenKeyName", json!("other-token")),
                (
                    "tokenSigningPublicKeys",
                    json!({"SECOND_KEY": "-----BEGIN PUBLIC KEY-----second"}),
                ),
                ("enableCachingForHttp", json!(true)),
            ],
        },
    ];

    for Row {
        case,
        body,
        expected,
    } in rows
    {
        let updated = emulator.update(name, &body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateAuthorizer, got {updated:?}"
        );
        // The authorizer keeps its identity: the update reports the name it was created
        // under and the ARN that create minted.
        assert_eq!(
            updated.text("authorizerName").as_deref(),
            Some(name),
            "case {case:?}: expected the update to report the authorizer's name, got {updated:?}"
        );
        assert_eq!(
            updated.text("authorizerArn"),
            Some(expected_arn(REGION, name)),
            "case {case:?}: expected the update to report the minted ARN, got {updated:?}"
        );

        let read = emulator.describe(name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the updated authorizer to be readable, got {read:?}"
        );
        for (member, want) in expected {
            assert_eq!(
                read.member(member),
                Some(&want),
                "case {case:?}: expected {member} to be {want}, got {read:?}"
            );
        }
    }

    // A name the store does not hold is the modelled not-found refusal rather than a
    // success that recorded a new authorizer.
    let refused = emulator.update("no_such_authorizer", &json!({"status": "ACTIVE"})).await;
    assert_eq!(
        refused.status, 404,
        "case \"a name the store does not hold\": expected 404, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"a name the store does not hold\": expected ResourceNotFoundException, got \
         {refused:?}"
    );
    let read = emulator.describe("no_such_authorizer").await;
    assert_eq!(
        read.status, 404,
        "case \"a name the store does not hold\": the refused update must record nothing, got \
         {read:?}"
    );
}

/// An update writes to the authorizer its URI addresses and to nothing else: it moves
/// that record's `lastModifiedDate` to the instant of the write, leaves the
/// `creationDate` its create stamped where it was, and leaves every other authorizer the
/// account holds exactly as a client read it a moment earlier.
///
/// The rows span what that rule generalises over — which member the update states, how
/// many it states at once, whether the value it states differs from the one the record
/// already carries, a request that states no member at all, and which of several
/// authorizers it addresses — over names that differ in form and length within the
/// `authorizerName` `@pattern` (`^[\w=,@-]+$`, from the model's `AuthorizerName`).
#[tokio::test]
async fn an_update_writes_only_to_the_authorizer_it_addresses() {
    let emulator = Emulator::new();

    // Three authorizers in one account: a plain word, a one-character name, and a long
    // name carrying every punctuation character the pattern admits.
    let names = ["alpha", "b", "Long-Authorizer_Name=42,@x"];
    for (index, name) in names.iter().enumerate() {
        let created = emulator
            .create(
                name,
                &json!({
                    "authorizerFunctionArn":
                        format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:f{index}"),
                    "tokenKeyName": "token",
                    "status": "ACTIVE",
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateAuthorizer must succeed, got {created:?}"
        );
    }

    /// One row of the table: a case name, the authorizer the update addresses, and the
    /// body it sends.
    struct Row {
        case: &'static str,
        target: &'static str,
        body: Value,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "the status flips to the other value",
            target: names[0],
            body: json!({"status": "INACTIVE"}),
        },
        Row {
            case: "the status the record already carries, stated again",
            target: names[0],
            body: json!({"status": "INACTIVE"}),
        },
        Row {
            case: "a new function ARN",
            target: names[1],
            body: json!({
                "authorizerFunctionArn":
                    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:replacement"),
            }),
        },
        Row {
            case: "signing public keys, a token key name and the caching flag together",
            target: names[2],
            body: json!({
                "tokenKeyName": "other-token",
                "tokenSigningPublicKeys": {"KEY": "-----BEGIN PUBLIC KEY-----pem"},
                "enableCachingForHttp": true,
            }),
        },
        Row {
            case: "a request that states no member",
            target: names[2],
            body: json!({}),
        },
    ];

    for Row { case, target, body } in rows {
        // What a client reads of every authorizer in the account before the update.
        let mut before: Vec<(&str, Value)> = Vec::new();
        for name in names {
            let read = emulator.describe(name).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: {name:?} must be readable before the update, got {read:?}"
            );
            before.push((name, read.description().cloned().unwrap_or(Value::Null)));
        }

        let window_opens = now_epoch_seconds();
        let updated = emulator.update(target, &body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateAuthorizer, got {updated:?}"
        );
        let window_closes = now_epoch_seconds();

        for (name, was) in &before {
            let read = emulator.describe(name).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: {name:?} must still be readable after the update, got {read:?}"
            );
            let now = read.description().cloned().unwrap_or(Value::Null);

            if *name != target {
                assert_eq!(
                    &now, was,
                    "case {case:?}: an update of {target:?} must leave {name:?} exactly as it \
                     was ({was:?}), got {now:?}"
                );
                continue;
            }

            assert_eq!(
                now.get("creationDate"),
                was.get("creationDate"),
                "case {case:?}: the update must leave creationDate as the create stamped it \
                 ({was:?}), got {now:?}"
            );
            let created_at = read.member_epoch("creationDate").unwrap_or_else(|| {
                panic!("case {case:?}: the description reported no creationDate: {read:?}")
            });
            let last_modified = read.member_epoch("lastModifiedDate").unwrap_or_else(|| {
                panic!("case {case:?}: the description reported no lastModifiedDate: {read:?}")
            });
            assert!(
                (window_opens - 1.0..=window_closes + 1.0).contains(&last_modified),
                "case {case:?}: lastModifiedDate {last_modified} is outside the window of the \
                 update [{window_opens}, {window_closes}], got {now:?}"
            );
            assert!(
                last_modified >= created_at,
                "case {case:?}: lastModifiedDate {last_modified} is before creationDate \
                 {created_at}, got {now:?}"
            );
        }
    }
}
