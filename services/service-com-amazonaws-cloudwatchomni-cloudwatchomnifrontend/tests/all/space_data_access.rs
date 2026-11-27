// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetSpaceDataAccess`: the temporary credentials a caller works
//! with a space's data under.
//!
//! Every call goes through the real handler holding the SAME shared engine the router hands
//! every service, so the space a create persisted is the space the credentials are issued
//! for, and the record the issue wrote is the record the next call reads. The model declares
//! no operation that reads a credential set back, so the round trip is through the store the
//! issue wrote — the same store every other operation of this service reads.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * credentials are issued for a space the account HOLDS, and every part of them comes
//!   from the emulator's credential store rather than from the request: the access key id is
//!   not the caller's, no two calls report the same set, and the set the response carries is
//!   the set the store holds;
//! * `expiration` is the lifetime the request asked for past the call, and the lifetime a
//!   request that named none leaves is the documented default of 3600 seconds;
//! * the credentials are SCOPED to the actions the request listed, over the space's own ARN:
//!   one policy statement per requested group, carrying exactly that group's actions — and
//!   an action for any service at all is admitted rather than rejected, which is the
//!   operation's "Requested actions that cannot be conferred are ignored rather than
//!   rejected";
//! * a scope whose policy does not fit a session policy is refused
//!   `PolicyTooLargeException` (400) and issues nothing, while a scope that fits is issued —
//!   checked on both sides of the published limit;
//! * a `spaceId` the account holds no space record under authorizes nobody, so the call is
//!   refused `AccessDeniedException` (403) and nothing is issued;
//! * an action outside `DataAccessActionName`'s class — a wildcard among them, which its doc
//!   says is not supported — is refused `ValidationException` (400) and issues nothing;
//! * credentials confer access to the data of ONE space, so deleting that space releases
//!   them;
//! * every terminal outcome leaves a structured audit record naming the presented access key
//!   id, the space and the outcome, and no secret.
//!
//! The rows differ along the lifetime asked for (both published bounds of `durationSeconds`'
//! `@range(min: 900, max: 3600)`, one between them and absent), along the shape of the scope
//! (one group, several, the same service twice, several services, the largest the model
//! admits) and along the space id a refused call names, so none of them is one recorded
//! request restated.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row of
//! the table that broke.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::CreateSpace;
use omni::ops::delete_space::DeleteSpace;
use omni::ops::get_space_data_access::{GetSpaceDataAccess, GetSpaceDataAccessError};
use omni::ops::types::{
    CreateDomainInput, CreateSpaceInput, DataAccessTarget, DeleteSpaceInput,
    GetSpaceDataAccessInput, GetSpaceDataAccessOutput, GET_SPACE_DATA_ACCESS_INPUT_SCHEMA,
};
use omni::state::{
    data_access_scope_policy, policy_too_large, ComAmazonawsCloudwatchomniCloudwatchomnifrontendState,
    AUTHORIZATION_FAILED, DEFAULT_DATA_ACCESS_DURATION_SECONDS, MAX_SESSION_POLICY_CHARACTERS,
    POLICY_LANGUAGE_VERSION, POLICY_TOO_LARGE_MESSAGE,
};

use super::domain_space_lifecycle::SPACE_REGIONS;
use super::validation_message_wording::error_envelope;

/// The access key id the suite signs with — the caller's own, which the credentials issued
/// to it must never be.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// Well-formed `SpaceId`s (the member's `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-\
/// [0-9a-f]{12}$`) the store holds nothing under.
const MISSING_SPACE_IDS: &[(&str, &str)] = &[
    ("an id of nil digits", "00000000-0000-4000-8000-000000000000"),
    (
        "an id of every hexadecimal digit",
        "0123456f-4567-89ab-cdef-0123456789ab",
    ),
    (
        "an id of the highest digits",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
    ),
];

/// A SigV4 `Authorization` header whose credential scope names this service's signing name
/// (the router's routing key), `region`, and the access key id the caller presents.
fn signed_request_parts(region: &str) -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/cloudwatch/\
                 aws4_request, SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
    /// How many spaces this emulator has created, which picks each create's Region out of
    /// [`SPACE_REGIONS`] — an account holds at most one space per Region.
    spaces_created: std::sync::atomic::AtomicUsize,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(SPACE_REGIONS[0]),
            spaces_created: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// A space to issue credentials for, created through the handlers that own those
    /// resources, with its ARN.
    async fn space(&self, case: &str) -> (String, String) {
        let domain_id = CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: "omni-space-data-access".to_string(),
                    identity_providers: vec!["IAM".to_string()],
                    client_token: Some("token-space-data-access".to_string()),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
            .domain_id;
        let index = self
            .spaces_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let region = SPACE_REGIONS
            .get(index)
            .copied()
            .expect("the suite signs each space it creates for a Region of its own");
        let parts = signed_request_parts(region);
        let ctx = OperationContext::new(
            &parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        );
        let space = CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: format!("space-{index}"),
                    domain_id,
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &ctx,
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space;
        (space.space_id, space.space_arn)
    }

    async fn get_space_data_access(
        &self,
        input: &GetSpaceDataAccessInput,
    ) -> Result<GetSpaceDataAccessOutput, GetSpaceDataAccessError> {
        GetSpaceDataAccess::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn delete_space(&self, case: &str, space_id: &str) {
        DeleteSpace::new(Arc::clone(&self.engine))
            .handle(
                DeleteSpaceInput {
                    space_id: space_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteSpace failed: {error:?}"));
    }

    /// The credential sets this service's store holds, as `(access key id, space id,
    /// policy)`.
    fn held_sessions(&self) -> Vec<(String, String, String)> {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatchomniCloudwatchomnifrontendState>();
        let store = state.lock();
        let mut held: Vec<(String, String, String)> = store
            .data_access_sessions
            .values()
            .map(|stored| {
                (
                    stored.access_key_id.clone(),
                    stored.space_id.clone(),
                    stored.scope_policy.clone(),
                )
            })
            .collect();
        held.sort();
        held
    }

    /// The record the issue under `access_key_id` wrote, as `(space id, policy, audit
    /// context entries, expiry)`.
    fn held_session(
        &self,
        case: &str,
        access_key_id: &str,
    ) -> (String, String, Vec<(String, String)>, i64) {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatchomniCloudwatchomnifrontendState>();
        let store = state.lock();
        let stored = store
            .data_access_sessions
            .get(access_key_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the issued credential set must be held by the store")
            });
        let mut audit_context: Vec<(String, String)> = stored
            .audit_context
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        audit_context.sort();
        (
            stored.space_id.clone(),
            stored.scope_policy.clone(),
            audit_context,
            stored.expires_at.secs(),
        )
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// Require a modelled failure with the status and error code the model declares for it.
fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected_status: u16,
    expected_code: &str,
) {
    match result {
        Ok(output) => {
            panic!("case {case:?}: expected {expected_code} ({expected_status}), got Ok({output:?})")
        }
        Err(error) => assert_eq!(
            (error.shape_name(), error.http_status()),
            (expected_code, expected_status),
            "case {case:?}: expected {expected_code} ({expected_status}), got {error:?}"
        ),
    }
}

/// A scope of the actions in `actions`, as a request carries one.
fn scope(actions: &[&str]) -> DataAccessTarget {
    DataAccessTarget {
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
    }
}

/// The smallest request the model admits for `space_id`: one scope of one action, and no
/// lifetime or audit context.
fn access_input(space_id: &str, target_scopes: Vec<DataAccessTarget>) -> GetSpaceDataAccessInput {
    GetSpaceDataAccessInput {
        space_id: space_id.to_string(),
        target_scopes,
        duration_seconds: None,
        audit_context: None,
    }
}

/// Epoch seconds now, as the clock the handler stamps an expiry from reads them.
fn epoch_seconds() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch");
    i64::try_from(since.as_secs()).expect("epoch seconds fit an i64")
}

/// The actions each statement of `policy` allows, and the resource it allows them over.
fn policy_statements(case: &str, policy: &str) -> Vec<(Vec<String>, String)> {
    let document: serde_json::Value = serde_json::from_str(policy)
        .unwrap_or_else(|error| panic!("case {case:?}: the stored policy is JSON: {error}"));
    assert_eq!(
        document["Version"].as_str(),
        Some(POLICY_LANGUAGE_VERSION),
        "case {case:?}: expected the policy language version, got {document}"
    );
    document["Statement"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the policy carries statements, got {document}"))
        .iter()
        .map(|statement| {
            assert_eq!(
                statement["Effect"].as_str(),
                Some("Allow"),
                "case {case:?}: a scope statement allows, got {statement}"
            );
            let actions = statement["Action"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: statement lists actions: {statement}"))
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect();
            let resource = statement["Resource"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: statement names a resource: {statement}"))
                .to_string();
            (actions, resource)
        })
        .collect()
}

#[tokio::test]
async fn credentials_are_issued_from_the_credential_store_and_expire_after_the_lifetime_asked_for()
{
    let omni = Omni::new();
    let (space_id, _) = omni.space("the space credentials are issued for").await;
    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut seen_secrets: HashSet<String> = HashSet::new();

    // Rows differ along the lifetime asked for: both published bounds of `durationSeconds`'
    // `@range(min: 900, max: 3600)`, one between them, and none at all — which the member's
    // doc says defaults to 3600.
    for (case, requested, lifetime) in [
        ("the shortest lifetime the model admits", Some(900), 900),
        ("a lifetime between the bounds", Some(1800), 1800),
        ("the longest lifetime the model admits", Some(3600), 3600),
        (
            "no lifetime asked for",
            None,
            DEFAULT_DATA_ACCESS_DURATION_SECONDS,
        ),
    ] {
        let before = epoch_seconds();
        let issued = omni
            .get_space_data_access(&GetSpaceDataAccessInput {
                duration_seconds: requested,
                audit_context: Some(
                    [("requestedBy".to_string(), "the suite".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..access_input(&space_id, vec![scope(&["logs:StartQuery"])])
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetSpaceDataAccess failed: {error:?}"))
            .credentials;
        let after = epoch_seconds();

        // Nothing of the request is echoed back as a credential, and nothing is empty.
        assert!(
            !issued.access_key_id.is_empty()
                && !issued.secret_access_key.is_empty()
                && !issued.session_token.is_empty(),
            "case {case:?}: every part of a credential set is reported, got {issued:?}"
        );
        assert_ne!(
            issued.access_key_id, ACCESS_KEY_ID,
            "case {case:?}: the credentials must be minted, not the caller's own"
        );
        assert!(
            seen_keys.insert(issued.access_key_id.clone()),
            "case {case:?}: an access key id was reported twice: {:?}",
            issued.access_key_id
        );
        assert!(
            seen_secrets.insert(issued.secret_access_key.clone()),
            "case {case:?}: a secret access key was reported twice"
        );
        assert_ne!(
            issued.secret_access_key, issued.session_token,
            "case {case:?}: the secret and the session token are different secrets"
        );

        // The expiry is the lifetime past the call, at whole-second resolution. The call
        // happened between `before` and `after`, so the reported instant has to sit in that
        // window advanced by the lifetime.
        let expiration = issued.expiration.secs();
        assert_eq!(
            issued.expiration.subsec_nanos(),
            0,
            "case {case:?}: expected a whole-second expiry, got {:?}",
            issued.expiration
        );
        assert!(
            (before + i64::from(lifetime)..=after + i64::from(lifetime)).contains(&expiration),
            "case {case:?}: expected an expiry in {}..={} for a lifetime of {lifetime}, got \
             {expiration}",
            before + i64::from(lifetime),
            after + i64::from(lifetime)
        );

        // The response reports the set the STORE holds, with the space it was issued for
        // and the audit context the request carried.
        let (held_space, _, audit_context, held_expiry) = omni.held_session(case, &issued.access_key_id);
        assert_eq!(
            (held_space.as_str(), held_expiry),
            (space_id.as_str(), expiration),
            "case {case:?}: the record must carry the space and the expiry the response \
             reported"
        );
        assert_eq!(
            audit_context,
            vec![("requestedBy".to_string(), "the suite".to_string())],
            "case {case:?}: the audit context must be recorded, got {audit_context:?}"
        );
    }
}

#[tokio::test]
async fn the_credentials_are_scoped_to_the_actions_the_request_listed() {
    let omni = Omni::new();
    let (space_id, space_arn) = omni.space("the space the scope is over").await;

    // Rows differ along the shape of the scope: one action, several of one service, the same
    // service in two groups (`DataAccessTarget`'s doc: "The same service may appear in more
    // than one entry"), two services, and an action of a service this space holds no data
    // for at all — which is admitted rather than rejected.
    for (case, scopes) in [
        ("one action", vec![scope(&["logs:StartQuery"])]),
        (
            "several actions of one service",
            vec![scope(&["logs:StartQuery", "logs:GetQueryResults"])],
        ),
        (
            "the same service in two groups",
            vec![scope(&["logs:StartQuery"]), scope(&["logs:StopQuery"])],
        ),
        (
            "two services",
            vec![
                scope(&["logs:StartQuery"]),
                scope(&["cloudwatch:GetMetricData", "cloudwatch:ListMetrics"]),
            ],
        ),
        (
            "an action of another service entirely",
            vec![scope(&["s3:GetObject"])],
        ),
    ] {
        let issued = omni
            .get_space_data_access(&access_input(&space_id, scopes.clone()))
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetSpaceDataAccess failed: {error:?}"))
            .credentials;
        let (_, policy, _, _) = omni.held_session(case, &issued.access_key_id);

        let expected: Vec<(Vec<String>, String)> = scopes
            .iter()
            .map(|scope| (scope.actions.clone(), space_arn.clone()))
            .collect();
        let statements = policy_statements(case, &policy);
        assert_eq!(
            statements, expected,
            "case {case:?}: expected one statement per group over the space's own ARN, got \
             {statements:?}"
        );
    }
}

#[tokio::test]
async fn a_scope_that_does_not_fit_a_session_policy_is_refused_and_issues_nothing() {
    let omni = Omni::new();
    let (space_id, space_arn) = omni.space("the space the scope is too large for").await;

    // Rows straddle the published limit on a session policy's plaintext: the largest scope
    // whose rendered policy still fits, and the smallest number of further actions that puts
    // it past the limit, plus the largest scope the model admits at all
    // (`targetScopes`' `@length(min: 1, max: 10)` of `DataAccessActionList`'s
    // `@length(min: 1, max: 50)`).
    //
    // The actions are long ones — `DataAccessActionName` admits up to 256 characters — so
    // the policy limit is reached well inside the 50 actions one group admits, and the
    // request that trips it is one the model itself accepts.
    let action = |index: usize| format!("logs:StartQueryVariant{index}{}", "A".repeat(80));
    let mut fitting: Vec<String> = Vec::new();
    while !policy_too_large(&data_access_scope_policy(
        &space_arn,
        &[DataAccessTarget {
            actions: {
                let mut actions = fitting.clone();
                actions.push(action(fitting.len()));
                actions
            },
        }],
    )) {
        fitting.push(action(fitting.len()));
    }
    assert!(
        !fitting.is_empty(),
        "the smallest scope must fit a session policy"
    );
    let mut past_the_limit = fitting.clone();
    past_the_limit.push(action(fitting.len()));

    let largest: Vec<DataAccessTarget> = (0..10)
        .map(|group| DataAccessTarget {
            actions: (0..50).map(|index| action(group * 50 + index)).collect(),
        })
        .collect();

    // The scope that fits is issued, and its record is the only one the store holds.
    let issued = omni
        .get_space_data_access(&access_input(
            &space_id,
            vec![DataAccessTarget {
                actions: fitting.clone(),
            }],
        ))
        .await
        .unwrap_or_else(|error| {
            panic!("case \"the largest scope that fits\": GetSpaceDataAccess failed: {error:?}")
        })
        .credentials;
    let fitting_policy = data_access_scope_policy(
        &space_arn,
        &[DataAccessTarget {
            actions: fitting.clone(),
        }],
    );
    assert!(
        fitting_policy.chars().count() <= MAX_SESSION_POLICY_CHARACTERS,
        "case \"the largest scope that fits\": expected a policy within {MAX_SESSION_POLICY_CHARACTERS} characters, got {}",
        fitting_policy.chars().count()
    );
    let issued_key = issued.access_key_id.clone();

    for (case, scopes) in [
        (
            "one action past the policy limit",
            vec![DataAccessTarget {
                actions: past_the_limit,
            }],
        ),
        ("the largest scope the model admits", largest),
    ] {
        expect_modelled_error(
            case,
            omni.get_space_data_access(&access_input(&space_id, scopes.clone()))
                .await,
            400,
            "PolicyTooLargeException",
        );

        // The same refusal reaches a real client over the rpcv2Cbor wire, with the sentence
        // `PolicyTooLargeException` words the condition with.
        let (status, body) = omni
            .over_the_wire(
                "GetSpaceDataAccess",
                GET_SPACE_DATA_ACCESS_INPUT_SCHEMA,
                &access_input(&space_id, scopes),
            )
            .await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "PolicyTooLargeException", POLICY_TOO_LARGE_MESSAGE),
            "case {case:?}: expected the modelled refusal, got {status} {code:?}: {message:?}"
        );

        // And nothing was issued for it: the one record the store holds is the one the
        // scope that fits wrote.
        let held = omni.held_sessions();
        assert_eq!(
            held.iter()
                .map(|(access_key_id, _, _)| access_key_id.as_str())
                .collect::<Vec<&str>>(),
            vec![issued_key.as_str()],
            "case {case:?}: a refused request must issue nothing, got {held:?}"
        );
    }
}

#[tokio::test]
async fn a_space_the_account_holds_no_record_under_authorizes_nobody() {
    let omni = Omni::new();
    let (space_id, _) = omni.space("the space the account holds").await;

    for (case, missing) in MISSING_SPACE_IDS {
        expect_modelled_error(
            case,
            omni.get_space_data_access(&access_input(missing, vec![scope(&["logs:StartQuery"])]))
                .await,
            403,
            "AccessDeniedException",
        );

        // Over the wire: the refusal states the fixed text and names nothing the request
        // carried, so it cannot be used to learn whether that space exists.
        let (status, body) = omni
            .over_the_wire(
                "GetSpaceDataAccess",
                GET_SPACE_DATA_ACCESS_INPUT_SCHEMA,
                &access_input(missing, vec![scope(&["logs:StartQuery"])]),
            )
            .await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (403, "AccessDeniedException", AUTHORIZATION_FAILED),
            "case {case:?}: expected the modelled refusal, got {status} {code:?}: {message:?}"
        );
        assert!(
            !message.contains(missing),
            "case {case:?}: the refusal must name no value the caller sent, got {message:?}"
        );
        assert!(
            omni.held_sessions().is_empty(),
            "case {case:?}: a refused request must issue nothing, got {:?}",
            omni.held_sessions()
        );
    }

    // The space the account DOES hold is served, so the rows above are refused for the
    // space they named rather than because the operation refuses everything.
    let issued = omni
        .get_space_data_access(&access_input(&space_id, vec![scope(&["logs:StartQuery"])]))
        .await
        .expect("the space the account holds is served")
        .credentials;
    assert!(
        !issued.access_key_id.is_empty(),
        "case \"the space the account holds\": expected an issued credential set"
    );
}

#[tokio::test]
async fn credentials_are_released_with_the_space_they_were_issued_for() {
    let omni = Omni::new();
    let (space_id, _) = omni.space("the space that is deleted").await;

    // Two sets for the one space, so the release is of every set issued for it rather than
    // of the last one.
    let mut issued_keys: Vec<String> = Vec::new();
    for (case, scopes) in [
        ("the first set", vec![scope(&["logs:StartQuery"])]),
        (
            "the second set",
            vec![scope(&["cloudwatch:GetMetricData"])],
        ),
    ] {
        let credentials = omni
            .get_space_data_access(&access_input(&space_id, scopes))
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetSpaceDataAccess failed: {error:?}"))
            .credentials;
        issued_keys.push(credentials.access_key_id);
    }
    issued_keys.sort();
    let held: Vec<String> = omni
        .held_sessions()
        .into_iter()
        .map(|(access_key_id, _, _)| access_key_id)
        .collect();
    assert_eq!(
        held, issued_keys,
        "case \"before the delete\": expected both issued sets to be held, got {held:?}"
    );

    omni.delete_space("the space that is deleted", &space_id).await;
    assert!(
        omni.held_sessions().is_empty(),
        "case \"after the delete\": credentials of a deleted space must be released, got {:?}",
        omni.held_sessions()
    );

    // And the space authorizes nothing afterwards, so no further set can be issued for it.
    expect_modelled_error(
        "a further issue after the delete",
        omni.get_space_data_access(&access_input(&space_id, vec![scope(&["logs:StartQuery"])]))
            .await,
        403,
        "AccessDeniedException",
    );
}

#[tokio::test]
async fn the_issued_credentials_reach_a_real_client_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let (space_id, space_arn) = omni.space("the space served over the wire").await;

    let before = epoch_seconds();
    let (status, body) = omni
        .over_the_wire(
            "GetSpaceDataAccess",
            GET_SPACE_DATA_ACCESS_INPUT_SCHEMA,
            &GetSpaceDataAccessInput {
                duration_seconds: Some(900),
                ..access_input(
                    &space_id,
                    vec![scope(&["logs:StartQuery", "logs:GetQueryResults"])],
                )
            },
        )
        .await;
    let after = epoch_seconds();
    assert_eq!(
        status,
        200,
        "case \"over the wire\": expected the call to be served, got {status}: {:?}",
        String::from_utf8_lossy(&body)
    );

    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    let decoded = GetSpaceDataAccessOutput::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case \"over the wire\": the response does not deserialize: {error:?}")
    });
    let credentials = decoded.credentials;
    assert!(
        (before + 900..=after + 900).contains(&credentials.expiration.secs()),
        "case \"over the wire\": expected an expiry in {}..={}, got {}",
        before + 900,
        after + 900,
        credentials.expiration.secs()
    );

    let (held_space, policy, _, _) = omni.held_session("over the wire", &credentials.access_key_id);
    assert_eq!(
        held_space, space_id,
        "case \"over the wire\": the record must carry the space the request named"
    );
    assert_eq!(
        policy_statements("over the wire", &policy),
        vec![(
            vec![
                "logs:StartQuery".to_string(),
                "logs:GetQueryResults".to_string()
            ],
            space_arn
        )],
        "case \"over the wire\": the scope must be the actions the request listed"
    );
}

#[tokio::test]
async fn an_action_outside_the_modelled_class_is_refused_and_issues_nothing() {
    let omni = Omni::new();
    let (space_id, _) = omni.space("the space the scope is refused for").await;

    // Rows differ along WHAT puts the action outside `DataAccessActionName`'s class
    // (`@length(min: 3, max: 256)`, `^[a-z0-9][a-z0-9-]*:[A-Za-z][A-Za-z0-9]*$`, doc:
    // "Wildcards are not supported"): a bare wildcard, a wildcard for one service, a
    // wildcard inside the action, an action with no service prefix, an upper-case prefix, and
    // one shorter than the published minimum.
    for (case, action) in [
        ("a bare wildcard", "*"),
        ("a wildcard for one service", "logs:*"),
        ("a wildcard inside the action", "logs:Start*"),
        ("no service prefix at all", "StartQuery"),
        ("an upper-case service prefix", "Logs:StartQuery"),
        ("shorter than the published minimum", "a:"),
    ] {
        let (status, body) = omni
            .over_the_wire(
                "GetSpaceDataAccess",
                GET_SPACE_DATA_ACCESS_INPUT_SCHEMA,
                &access_input(&space_id, vec![scope(&[action])]),
            )
            .await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the modelled constraint refusal, got {status} {code:?}: \
             {message:?}"
        );
        assert!(
            omni.held_sessions().is_empty(),
            "case {case:?}: a refused request must issue nothing, got {:?}",
            omni.held_sessions()
        );
    }

    // An action INSIDE the class is served, so the rows above are refused for the action
    // they named rather than because the operation refuses every scope.
    let (status, _) = omni
        .over_the_wire(
            "GetSpaceDataAccess",
            GET_SPACE_DATA_ACCESS_INPUT_SCHEMA,
            &access_input(&space_id, vec![scope(&["logs:StartQuery"])]),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an action inside the class\": expected the call to be served, got {status}"
    );
}

/// One captured `tracing` event: its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedEvent {
    fields: std::collections::BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings, whatever type they were recorded as.
struct FieldCollector<'a>(&'a mut std::collections::BTreeMap<String, String>);

impl tracing::field::Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a suite can read the
/// audit records back. Nothing is written to disk.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<std::sync::Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// Whether a captured record attributes `operation` on `space_id` to the caller with
    /// `outcome`.
    fn holds(&self, operation: &str, space_id: &str, outcome: &str) -> bool {
        let addressed = format!("{space_id:?}");
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .any(|event| {
                event.field("operation") == Some(operation)
                    && event.field("space_id") == Some(addressed.as_str())
                    && event.field("outcome") == Some(outcome)
                    && event.field("access_key_id") == Some(ACCESS_KEY_ID)
            })
    }

    /// Whether any captured record carries `secret`, whatever field it sits in.
    fn carries(&self, secret: &str) -> bool {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .any(|event| {
                event
                    .fields
                    .values()
                    .any(|rendered| rendered.contains(secret))
            })
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // This suite asserts on events, not spans; one id for every span is enough.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = std::collections::BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent { fields });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The ONE capture this test binary installs, built on first use.
static AUDIT_CAPTURE: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();

/// The capture every audit record of this process reaches.
///
/// Installed as the process-wide subscriber rather than this thread's, because `tracing`
/// caches per callsite whether anything is interested in it: a thread-scoped subscriber
/// would see nothing for a callsite another suite of this binary had already reached with
/// none installed. Installing it rebuilds that cache, so the records that follow arrive.
/// Records of other suites arrive too, which is why every assertion below names the space
/// and the outcome it is about.
fn audit_capture() -> &'static Capture {
    AUDIT_CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone())
            .expect("this binary installs one process-wide subscriber");
        capture
    })
}

#[tokio::test]
async fn every_outcome_of_a_credential_issue_leaves_an_audit_record() {
    let capture = audit_capture();
    let omni = Omni::new();
    let (space_id, space_arn) = omni.space("the space the records are about").await;

    // Rows differ along the outcome: the issue, the space that authorizes nobody, and the
    // scope that does not fit a session policy.
    let issued = omni
        .get_space_data_access(&access_input(&space_id, vec![scope(&["logs:StartQuery"])]))
        .await
        .expect("the space the account holds is served")
        .credentials;

    let missing = MISSING_SPACE_IDS[0].1;
    expect_modelled_error(
        "the space that authorizes nobody",
        omni.get_space_data_access(&access_input(missing, vec![scope(&["logs:StartQuery"])]))
            .await,
        403,
        "AccessDeniedException",
    );

    // The smallest scope past the policy limit, built through the same renderer the handler
    // uses, so the row is a refusal rather than a guess at a size.
    let action = |index: usize| format!("logs:StartQueryVariant{index}{}", "A".repeat(80));
    let mut too_large: Vec<String> = Vec::new();
    while !policy_too_large(&data_access_scope_policy(
        &space_arn,
        &[DataAccessTarget {
            actions: too_large.clone(),
        }],
    )) {
        too_large.push(action(too_large.len()));
    }
    expect_modelled_error(
        "the scope past the policy limit",
        omni.get_space_data_access(&access_input(
            &space_id,
            vec![DataAccessTarget { actions: too_large }],
        ))
        .await,
        400,
        "PolicyTooLargeException",
    );

    // Every terminal outcome is attributable: the operation, the presented access key id,
    // the space the request addressed and how the call came out.
    for (case, addressed, outcome) in [
        ("the issue", space_id.as_str(), "issued"),
        (
            "the space that authorizes nobody",
            missing,
            "refused_not_authorized",
        ),
        (
            "the scope past the policy limit",
            space_id.as_str(),
            "refused_policy_too_large",
        ),
    ] {
        assert!(
            capture.holds("GetSpaceDataAccess", addressed, outcome),
            "case {case:?}: expected an audit record of {outcome:?} for {addressed:?} by \
             {ACCESS_KEY_ID}, captured {:?}",
            capture.events.lock().expect("the capture buffer")
        );
    }

    // And no record carries a minted credential — not the access key id it was issued
    // under, not its secret, not its session token.
    for (case, secret) in [
        ("the minted access key id", issued.access_key_id.as_str()),
        ("the secret access key", issued.secret_access_key.as_str()),
        ("the session token", issued.session_token.as_str()),
    ] {
        assert!(
            !capture.carries(secret),
            "case {case:?}: no audit record may carry a minted credential"
        );
    }
}
