// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ONE resource SSM GUI Connect models: the
//! per-account/per-Region connection recording preferences singleton.
//!
//! Every case drives real HTTP through the production
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) — the same
//! dispatch path the emulator's front door uses — so what is asserted is wire
//! behaviour (status, error code, body), not an internal call.
//!
//! The rules under test, stated once:
//!
//! 1. The singleton is PERSISTED STATE. Update creates-or-replaces it, Get reports
//!    exactly what the last Update wrote (never an echo of the reader's request),
//!    Delete makes it observably gone, and a read with nothing configured is
//!    `ResourceNotFoundException` (404) rather than an empty 200.
//! 2. Both mutating operations are `@idempotent` with an `@idempotencyToken`: the
//!    same `ClientToken` replayed answers with the first call's outcome and does not
//!    mutate again; a request that omits it gets a service-provided token back.
//! 3. A value outside a modeled constraint is `ValidationException` (400).
//! 4. The resource exists PER REGION: a call reaches the record of the Region its own
//!    credential scope names and of no other, so a read answers that Region's record (or
//!    404), and a write or a delete leaves every other Region's record exactly as it was.
//!    The Regions that hold a record at once are capped, because that Region is a value
//!    the caller chooses.
//!
//! Each rule is asserted over a TABLE of inputs spanning what it generalises over
//! (bucket name form and length, owner-id shape, list length, key-ARN length, token
//! length, presence of the token), not over the one case the bootstrap issue cited.
//!
//! Every case about a CONFIGURED resource runs against an account enrolled in
//! just-in-time node access ([`enrolled_router`]), because
//! `UpdateConnectionRecordingPreferences` is gated on that enrollment. The gate
//! itself, the idempotent delete of an unconfigured account and this service's error
//! text are the sibling suite `jit_node_access_gate_and_error_messages`.

use std::sync::Arc;

use service_com_amazonaws_ssmguiconnect_ssmguiconnect::state::{
    bucket_name_matches_model_pattern, idempotency_key, region_ceiling_message,
    IdempotentOperation, StoredResponse, BUCKET_NAME_PATTERN, MAX_CONFIGURED_REGIONS,
    MAX_IDEMPOTENCY_OUTCOMES,
};
use service_com_amazonaws_ssmguiconnect_ssmguiconnect::{
    ComAmazonawsSsmguiconnectSsmguiconnectState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_authorization::{
    enroll as enroll_region, error_code as shared_error_code, error_message,
    register_systems_manager, request as signed_request, REGION,
};

/// A router carrying this service, over a fresh engine — one isolated store per test —
/// for an account as AWS leaves it: NOT enrolled in just-in-time node access, which is
/// what `UpdateConnectionRecordingPreferences` is gated on. Systems Manager rides along
/// because the account's enrollment is that service's own state.
fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same router, beside the engine it was built over, for the one case that has to
/// inspect the store directly (the idempotency bound is not observable on the wire).
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the framework's job and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    // Systems Manager, because the account's just-in-time node access enrollment is that
    // service's own state: a case that needs an enrolled account creates the enrollment
    // through its API (see `enroll`).
    register_systems_manager(&mut router, &engine);
    (router, engine)
}

/// A router over an account that IS enrolled in just-in-time node access — the
/// precondition `UpdateConnectionRecordingPreferences` is gated on, and therefore the
/// store every case about a CONFIGURED resource needs.
async fn enrolled_router() -> MultiServiceRouter {
    enrolled_router_with_engine().await.0
}

/// [`enrolled_router`], beside its engine.
async fn enrolled_router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    enroll(&router, true).await;
    (router, engine)
}

/// Enroll (or un-enroll) the account in just-in-time node access, over the wire: the
/// enrollment is an access-request approval policy Systems Manager holds, so it is created
/// and deleted through that service's own API — there is no in-process back door.
async fn enroll(router: &MultiServiceRouter, enrolled: bool) {
    crate::identity_authorization::enroll(router, REGION, enrolled).await;
}

/// The access key id every case signs with unless it needs its own.
const DEFAULT_AKID: &str = "AKIDEXAMPLE";

/// A restJson1 request for `op`, routed by the SigV4 credential scope the way every
/// signed AWS SDK request is, presenting `akid` as its access key id.
///
/// Signed for [`REGION`] through the Region-parameterised builder the sibling
/// authorization suite owns, so there is one way a request to this service is built.
fn request(op: &str, body: &serde_json::Value, akid: &str) -> http::Request<bytes::Bytes> {
    signed_request(REGION, akid, op, body)
}

/// Status code plus parsed JSON body of one call.
async fn call(
    router: &MultiServiceRouter,
    op: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call_as(router, op, body, DEFAULT_AKID).await
}

/// The same, presenting `akid` so a case can tell its own calls apart from a sibling
/// test's in process-wide state such as the audit log.
async fn call_as(
    router: &MultiServiceRouter,
    op: &str,
    body: serde_json::Value,
    akid: &str,
) -> (u16, serde_json::Value) {
    reply_of(router.dispatch(request(op, &body, akid)).await)
}

/// One response reduced to what every case below reads: its status and its parsed body.
/// The one place a response is read, so a call signed for any Region is reported alike.
fn reply_of(response: http::Response<bytes::Bytes>) -> (u16, serde_json::Value) {
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// The `__type` of a restJson1 error body, shortened to the shape name — read through
/// the sibling authorization suite's reader, so an error body is read one way here.
fn error_code(body: &serde_json::Value) -> String {
    shared_error_code(body)
}

fn preferences(owner: &str, bucket: &str, key_arn: &str) -> serde_json::Value {
    serde_json::json!({
        "RecordingDestinations": {
            "S3Buckets": [{ "BucketOwner": owner, "BucketName": bucket }]
        },
        "KMSKeyArn": key_arn,
    })
}

/// A valid KMS key ARN to pair with whatever the case under test varies.
const KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

// ---------------------------------------------------------------------------
// Rule 1 — the singleton is persisted state
// ---------------------------------------------------------------------------

/// Update → Get round trip, over a TABLE of accepted preference values that differ
/// along everything the stored record generalises over: bucket-name form (single
/// label, hyphens, dotted labels, a digit label beside a letter one), bucket-name
/// length at both published bounds (3 and 63), owner id, and KMS key ARN length
/// (the shortest 1-character value the `@length(1, 2048)` admits, and a long one).
///
/// Each row is written and then read back through `GetConnectionRecordingPreferences`
/// — the resource's own reader — so a response that echoed the writer's request
/// rather than reading state would not pass.
#[tokio::test]
async fn update_then_get_reports_every_stored_member() {
    // (case, bucket owner, bucket name, kms key arn)
    let cases: [(&str, &str, &str, &str); 7] = [
        ("single label", "123456789012", "my-rdp-recordings", KEY_ARN),
        ("shortest name (3, the pattern's lower bound)", "000000000000", "abc", KEY_ARN),
        (
            "longest name (63, the pattern's upper bound)",
            "999999999999",
            "a234567890123456789012345678901234567890123456789012345678901b",
            KEY_ARN,
        ),
        ("dotted labels", "123456789012", "logs.rdp.recordings", KEY_ARN),
        ("digit label beside a letter label", "123456789012", "1.rdp", KEY_ARN),
        ("minimum-length key arn", "123456789012", "another-bucket", "k"),
        (
            "long key arn",
            "210987654321",
            "a-b-c",
            "arn:aws:kms:eu-west-1:210987654321:key/ffffffff-ffff-4fff-bfff-ffffffffffff",
        ),
    ];

    for (case, owner, bucket, key_arn) in cases {
        let router = enrolled_router().await;
        let token = format!("token-for-{case}").replace(' ', "-");
        let token: String = token.chars().take(64).collect();

        let (status, updated) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences(owner, bucket, key_arn),
                "ClientToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Update to answer 200, got {status:?} with {updated:?}"
        );
        assert_eq!(
            updated["ClientToken"].as_str(),
            Some(token.as_str()),
            "case {case:?}: expected Update to echo the ClientToken, got {updated:?}"
        );

        let (status, got) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Get to answer 200, got {status:?} with {got:?}"
        );

        let stored = &got["ConnectionRecordingPreferences"];
        assert_eq!(
            stored["KMSKeyArn"].as_str(),
            Some(key_arn),
            "case {case:?}: expected the stored KMSKeyArn, got {got:?}"
        );
        let buckets = stored["RecordingDestinations"]["S3Buckets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an S3Buckets list, got {got:?}"));
        assert_eq!(
            buckets.len(),
            1,
            "case {case:?}: expected exactly one stored bucket, got {buckets:?}"
        );
        assert_eq!(
            buckets[0]["BucketOwner"].as_str(),
            Some(owner),
            "case {case:?}: expected the stored BucketOwner, got {got:?}"
        );
        assert_eq!(
            buckets[0]["BucketName"].as_str(),
            Some(bucket),
            "case {case:?}: expected the stored BucketName, got {got:?}"
        );
        assert_eq!(
            got["ClientToken"].as_str(),
            Some(token.as_str()),
            "case {case:?}: expected Get to report the recording Update's ClientToken, got {got:?}"
        );
    }
}

/// A READ with nothing configured has no resource to report: 404
/// `ResourceNotFoundException`, not an empty 200. A DELETE with nothing configured is
/// a different rule — it is asked for the state the store is already in, so it
/// succeeds — and is asserted in `jit_node_access_gate_and_error_messages`.
#[tokio::test]
async fn reading_an_unconfigured_singleton_is_not_found() {
    let router = router();

    for read in 1..=3 {
        let (status, response) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case \"read {read}\": expected 404 on a clean store, got {status:?} with \
             {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case \"read {read}\": expected ResourceNotFoundException, got {response:?}"
        );
    }
}

/// A second Update REPLACES the stored record; it never merges with or appends to the
/// previous one. Each row replaces the row before it, so the reader must report the
/// latest write and nothing of the earlier ones.
#[tokio::test]
async fn a_later_update_replaces_the_stored_record() {
    let router = enrolled_router().await;
    // (case, owner, bucket, key arn, token)
    let cases: [(&str, &str, &str, &str, &str); 4] = [
        ("first write", "123456789012", "first-bucket", KEY_ARN, "tok-1"),
        ("different bucket name", "123456789012", "second-bucket", KEY_ARN, "tok-2"),
        ("different owner and key", "210987654321", "third.bucket", "k", "tok-3"),
        ("back to the first values", "123456789012", "first-bucket", KEY_ARN, "tok-4"),
    ];

    for (case, owner, bucket, key_arn, token) in cases {
        let (status, updated) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences(owner, bucket, key_arn),
                "ClientToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Update to answer 200, got {status:?} with {updated:?}"
        );

        let (status, got) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected Get to answer 200, got {status:?}");
        let buckets = got["ConnectionRecordingPreferences"]["RecordingDestinations"]["S3Buckets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an S3Buckets list, got {got:?}"))
            .clone();
        assert_eq!(
            buckets.len(),
            1,
            "case {case:?}: a replace must not append — got {buckets:?}"
        );
        assert_eq!(
            buckets[0]["BucketName"].as_str(),
            Some(bucket),
            "case {case:?}: expected the replacing BucketName, got {got:?}"
        );
        assert_eq!(
            buckets[0]["BucketOwner"].as_str(),
            Some(owner),
            "case {case:?}: expected the replacing BucketOwner, got {got:?}"
        );
        assert_eq!(
            got["ConnectionRecordingPreferences"]["KMSKeyArn"].as_str(),
            Some(key_arn),
            "case {case:?}: expected the replacing KMSKeyArn, got {got:?}"
        );
        assert_eq!(
            got["ClientToken"].as_str(),
            Some(token),
            "case {case:?}: expected the replacing ClientToken, got {got:?}"
        );
    }
}

/// Delete removes real persisted state: the next Get reports not-found, and the
/// resource can then be configured again. Run over the full lifecycle twice, so the
/// second round proves the delete left no residue behind.
#[tokio::test]
async fn delete_makes_the_singleton_observably_gone() {
    let router = enrolled_router().await;

    for round in ["first round", "second round"] {
        let (status, _) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": format!("update-{round}").replace(' ', "-"),
            }),
        )
        .await;
        assert_eq!(status, 200, "case {round:?}: expected Update to answer 200");

        let (status, _) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, 200, "case {round:?}: expected the configured Get to answer 200");

        let delete_token = format!("delete-{round}").replace(' ', "-");
        let (status, deleted) = call(
            &router,
            "DeleteConnectionRecordingPreferences",
            serde_json::json!({ "ClientToken": delete_token }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {round:?}: expected Delete to answer 200, got {status:?} with {deleted:?}"
        );
        assert_eq!(
            deleted["ClientToken"].as_str(),
            Some(delete_token.as_str()),
            "case {round:?}: expected Delete to echo the ClientToken, got {deleted:?}"
        );

        let (status, got) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {round:?}: expected 404 after Delete, got {status:?} with {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceNotFoundException",
            "case {round:?}: expected ResourceNotFoundException after Delete, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — both mutating operations are @idempotent
// ---------------------------------------------------------------------------

/// Replaying an Update with the same `ClientToken` answers with the FIRST call's
/// record and writes nothing, so a replay carrying different preferences does not
/// overwrite what the token already stored. Rows vary the replayed payload: identical,
/// a different bucket, and a different key.
#[tokio::test]
async fn replaying_an_update_token_returns_the_first_outcome() {
    let cases: [(&str, serde_json::Value); 3] = [
        (
            "identical replay",
            preferences("123456789012", "first-bucket", KEY_ARN),
        ),
        (
            "replay with a different bucket",
            preferences("123456789012", "other-bucket", KEY_ARN),
        ),
        (
            "replay with a different owner and key",
            preferences("210987654321", "third-bucket", "k"),
        ),
    ];

    for (case, replayed) in cases {
        let router = enrolled_router().await;
        let first = preferences("123456789012", "first-bucket", KEY_ARN);
        let (status, _) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences": first,
                "ClientToken": "same-token",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected the first Update to answer 200");

        let (status, replay) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences": replayed,
                "ClientToken": "same-token",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replay to answer 200, got {status:?} with {replay:?}"
        );
        assert_eq!(
            replay["ConnectionRecordingPreferences"]["RecordingDestinations"]["S3Buckets"][0]
                ["BucketName"]
                .as_str(),
            Some("first-bucket"),
            "case {case:?}: a replayed token must answer with the first outcome, got {replay:?}"
        );

        let (_, got) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            got["ConnectionRecordingPreferences"]["RecordingDestinations"]["S3Buckets"][0]
                ["BucketName"]
                .as_str(),
            Some("first-bucket"),
            "case {case:?}: a replayed token must not mutate the store, got {got:?}"
        );
    }
}

/// Replaying a Delete with the same `ClientToken` answers with the first call's
/// outcome — 200 and that token — instead of refusing because the singleton it
/// already removed is gone. Rows vary how many times the token is replayed.
#[tokio::test]
async fn replaying_a_delete_token_returns_the_first_outcome() {
    for replays in [1_u32, 2, 5] {
        let router = enrolled_router().await;
        let (status, _) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": "setup",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {replays:?} replays: expected Update to answer 200");

        for attempt in 0..=replays {
            let (status, deleted) = call(
                &router,
                "DeleteConnectionRecordingPreferences",
                serde_json::json!({ "ClientToken": "delete-token" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {replays:?} replays, attempt {attempt}: expected 200, got {status:?} with {deleted:?}"
            );
            assert_eq!(
                deleted["ClientToken"].as_str(),
                Some("delete-token"),
                "case {replays:?} replays, attempt {attempt}: expected the token back, got {deleted:?}"
            );
        }

        // A DIFFERENT token after the singleton is gone is a fresh call with nothing
        // to remove — which Delete is asked for, so it succeeds and reports that
        // token.
        let (status, response) = call(
            &router,
            "DeleteConnectionRecordingPreferences",
            serde_json::json!({ "ClientToken": "another-token" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {replays:?} replays: a new token on an empty store must still succeed, got {status:?} with {response:?}"
        );
        assert_eq!(
            response["ClientToken"].as_str(),
            Some("another-token"),
            "case {replays:?} replays: expected the new token back, got {response:?}"
        );
    }
}

/// An `@idempotent` call's outcome is keyed by ONE derivation —
/// `state::idempotency_key(operation, Region, ClientToken)` — and both sides of that key
/// go through it: the handler that looks a replay up, and the state API that records the
/// outcome. A row per operation carrying an `@idempotencyToken`, so each side is driven
/// end to end for both of them.
///
/// Each row asserts, for its operation: the outcome the call recorded is retained under
/// exactly the key that constructor derives, and under no key differing in any ONE of
/// the key's three parts; a replay of the same token is answered from that outcome and
/// leaves the stored record exactly as the first call left it; and the SAME token under
/// the OTHER `@idempotent` operation is a different key, so that operation runs for real.
/// A lookup key spelled differently from the record key would miss, and both mutating
/// operations would mutate a second time instead of replaying.
#[tokio::test]
async fn the_lookup_key_and_the_record_key_are_one_derivation() {
    /// The token whose key both sides must derive the same way.
    const TOKEN: &str = "one-derivation-token";
    /// The token the Delete row's setup Update is written under — not the key under test.
    const SETUP: &str = "one-derivation-setup";
    /// A Region no call below is made in, so a key differing only in its Region part
    /// names nothing.
    const OTHER_REGION: &str = "eu-west-1";

    let first = preferences("123456789012", "first-bucket", KEY_ARN);
    let second = preferences("210987654321", "second-bucket", KEY_ARN);

    for (case, op) in [
        ("an Update's outcome", IdempotentOperation::Update),
        ("a Delete's outcome", IdempotentOperation::Delete),
    ] {
        let other_op = match op {
            IdempotentOperation::Update => IdempotentOperation::Delete,
            IdempotentOperation::Delete => IdempotentOperation::Update,
        };
        let (router, engine) = enrolled_router_with_engine().await;

        // The call whose outcome is recorded under TOKEN. A Delete has something to
        // remove only once the singleton is configured, which its setup Update does
        // under a token of its own.
        match op {
            IdempotentOperation::Update => {
                let (status, body) = call(
                    &router,
                    "UpdateConnectionRecordingPreferences",
                    serde_json::json!({
                        "ConnectionRecordingPreferences": first.clone(),
                        "ClientToken": TOKEN,
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the recording call must answer 200, got {status:?} with \
                     {body:?}"
                );
            }
            IdempotentOperation::Delete => {
                let (status, body) = call(
                    &router,
                    "UpdateConnectionRecordingPreferences",
                    serde_json::json!({
                        "ConnectionRecordingPreferences": first.clone(),
                        "ClientToken": SETUP,
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup Update must answer 200, got {status:?} with \
                     {body:?}"
                );
                let (status, body) = call(
                    &router,
                    "DeleteConnectionRecordingPreferences",
                    serde_json::json!({ "ClientToken": TOKEN }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the recording call must answer 200, got {status:?} with \
                     {body:?}"
                );
            }
        }

        // The WRITE side: the outcome is retained under exactly the shared derivation.
        // The store guard lives in a block of its own, so no call below is made while it
        // is held.
        let held_before = {
            let state = engine.state::<ComAmazonawsSsmguiconnectSsmguiconnectState>();
            let store = state.lock();
            let recorded = store.recorded_outcome(&idempotency_key(op, REGION, TOKEN));
            let recorded_as_expected = match op {
                IdempotentOperation::Update => matches!(recorded, Some(StoredResponse::Updated(_))),
                IdempotentOperation::Delete => matches!(recorded, Some(StoredResponse::Deleted)),
            };
            assert!(
                recorded_as_expected,
                "case {case:?}: the call's outcome must be retained under \
                 idempotency_key({op:?}, {REGION:?}, {TOKEN:?}), got {recorded:?}"
            );

            // …and under no key that differs in one of the key's parts, so none of the
            // three is dropped from the derivation.
            let perturbed = [
                ("the operation", idempotency_key(other_op, REGION, TOKEN)),
                ("the Region", idempotency_key(op, OTHER_REGION, TOKEN)),
                (
                    "the token",
                    idempotency_key(op, REGION, &format!("{TOKEN}-other")),
                ),
            ];
            for (part, key) in perturbed {
                let held = store.recorded_outcome(&key);
                assert!(
                    held.is_none(),
                    "case {case:?}: no outcome must be retained under a key differing in \
                     {part}, got {held:?}"
                );
            }
            store.preferences(REGION).cloned()
        };

        // The READ side: the handler finds that outcome under the key it derives for
        // itself, so the replay is answered from the FIRST call and writes nothing.
        match op {
            IdempotentOperation::Update => {
                let (status, replay) = call(
                    &router,
                    "UpdateConnectionRecordingPreferences",
                    serde_json::json!({
                        "ConnectionRecordingPreferences": second.clone(),
                        "ClientToken": TOKEN,
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the replay must answer 200, got {status:?} with {replay:?}"
                );
                assert_eq!(
                    replay["ConnectionRecordingPreferences"]["RecordingDestinations"]
                        ["S3Buckets"][0]["BucketName"]
                        .as_str(),
                    Some("first-bucket"),
                    "case {case:?}: the replay must answer with the first outcome, got {replay:?}"
                );
            }
            IdempotentOperation::Delete => {
                let (status, replay) = call(
                    &router,
                    "DeleteConnectionRecordingPreferences",
                    serde_json::json!({ "ClientToken": TOKEN }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the replay must answer 200, got {status:?} with {replay:?}"
                );
                assert_eq!(
                    replay["ClientToken"].as_str(),
                    Some(TOKEN),
                    "case {case:?}: the replay must report the token it was made with, got \
                     {replay:?}"
                );
            }
        }
        let held_after = engine
            .state::<ComAmazonawsSsmguiconnectSsmguiconnectState>()
            .lock()
            .preferences(REGION)
            .cloned();
        assert_eq!(
            held_after, held_before,
            "case {case:?}: a replay must leave the stored record exactly as the first call \
             left it, got {held_after:?}"
        );

        // The same token under the OTHER `@idempotent` operation is a different key — the
        // operation is part of it — so that operation runs for real rather than answering
        // from the outcome above.
        match other_op {
            IdempotentOperation::Update => {
                let (status, body) = call(
                    &router,
                    "UpdateConnectionRecordingPreferences",
                    serde_json::json!({
                        "ConnectionRecordingPreferences": second.clone(),
                        "ClientToken": TOKEN,
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the other operation under the same token must answer 200, \
                     got {status:?} with {body:?}"
                );
                let (status, got) = call(
                    &router,
                    "GetConnectionRecordingPreferences",
                    serde_json::json!({}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the Update under the same token must have run, got \
                     {status:?} with {got:?}"
                );
                assert_eq!(
                    got["ConnectionRecordingPreferences"]["RecordingDestinations"]["S3Buckets"]
                        [0]["BucketName"]
                        .as_str(),
                    Some("second-bucket"),
                    "case {case:?}: the Update under the same token must have written its own \
                     request, got {got:?}"
                );
            }
            IdempotentOperation::Delete => {
                let (status, body) = call(
                    &router,
                    "DeleteConnectionRecordingPreferences",
                    serde_json::json!({ "ClientToken": TOKEN }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the other operation under the same token must answer 200, \
                     got {status:?} with {body:?}"
                );
                let (status, got) = call(
                    &router,
                    "GetConnectionRecordingPreferences",
                    serde_json::json!({}),
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: the Delete under the same token must have removed the \
                     record, got {status:?} with {got:?}"
                );
            }
        }
    }
}

/// `ClientToken` is `@idempotencyToken`, so a request that omits it still gets one
/// back: within the modeled `@length(1, 64)`, distinct per call, and the value the
/// resource records. Asserted for Update and, over the same store, for Delete.
#[tokio::test]
async fn an_omitted_client_token_is_service_provided() {
    let router = enrolled_router().await;
    let mut minted: Vec<String> = Vec::new();

    for case in ["first write", "second write"] {
        let (status, updated) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 without a ClientToken, got {status:?} with {updated:?}"
        );
        let token = updated["ClientToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a minted ClientToken, got {updated:?}"))
            .to_string();
        assert!(
            (1..=64).contains(&token.chars().count()),
            "case {case:?}: a minted ClientToken must satisfy @length(1, 64), got {token:?}"
        );

        let (_, got) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            got["ClientToken"].as_str(),
            Some(token.as_str()),
            "case {case:?}: the minted token must be the one recorded, got {got:?}"
        );
        minted.push(token);
    }

    assert_ne!(
        minted[0], minted[1],
        "case \"two writes\": each minted ClientToken must be distinct, got {minted:?}"
    );

    let (status, deleted) = call(
        &router,
        "DeleteConnectionRecordingPreferences",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete without a token\": expected 200, got {status:?} with {deleted:?}"
    );
    let token = deleted["ClientToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"delete without a token\": expected a minted ClientToken, got {deleted:?}"));
    assert!(
        (1..=64).contains(&token.chars().count()),
        "case \"delete without a token\": a minted ClientToken must satisfy @length(1, 64), got {token:?}"
    );
}

/// `GetConnectionRecordingPreferences` is `@readonly`: reading does not change what a
/// later caller sees. Read many times and assert every response is identical to the
/// first — and that a Delete still finds the resource afterwards, so no read consumed
/// it.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = enrolled_router().await;
    let (status, _) = call(
        &router,
        "UpdateConnectionRecordingPreferences",
        serde_json::json!({
            "ConnectionRecordingPreferences":
                preferences("123456789012", "rdp-recordings", KEY_ARN),
            "ClientToken": "stable",
        }),
    )
    .await;
    assert_eq!(status, 200, "case \"setup\": expected Update to answer 200");

    let (_, first) = call(
        &router,
        "GetConnectionRecordingPreferences",
        serde_json::json!({}),
    )
    .await;
    for read in 1..=5 {
        let (status, again) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, 200, "case \"read {read}\": expected 200, got {status:?}");
        assert_eq!(
            again, first,
            "case \"read {read}\": a read must not change the resource, got {again:?}"
        );
    }

    let (status, deleted) = call(
        &router,
        "DeleteConnectionRecordingPreferences",
        serde_json::json!({ "ClientToken": "cleanup" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete after reads\": the reads must not have consumed the resource, got {status:?} with {deleted:?}"
    );
}

/// An idempotent replay is only answered from the record of the first call while the
/// store is still in the state that call left it in. Once a later call changes the
/// resource, the token describes a state no reader can see, so the replay must run
/// for real rather than report it — asserted in both directions.
#[tokio::test]
async fn a_replay_whose_resource_changed_is_not_answered_from_the_first_outcome() {
    // An Update token replayed after the singleton was DELETED must write again, not
    // report a record that is gone.
    let rewritten = enrolled_router().await;
    for case in ["first write", "replay after delete"] {
        let (status, updated) = call(
            &rewritten,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": "reused-update-token",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Update to answer 200, got {status:?} with {updated:?}"
        );
        let (status, got) = call(
            &rewritten,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an Update that answered 200 must leave a readable resource, got {status:?} with {got:?}"
        );

        if case == "first write" {
            let (status, _) = call(
                &rewritten,
                "DeleteConnectionRecordingPreferences",
                serde_json::json!({ "ClientToken": "interleaved-delete" }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: expected the interleaved Delete to answer 200");
        }
    }

    // A Delete token replayed after the singleton was RECONFIGURED must remove the new
    // record, so its 200 still means "there are no preferences now".
    let reconfigured = enrolled_router().await;
    for case in ["first delete", "replay after reconfigure"] {
        let (status, _) = call(
            &reconfigured,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": format!("setup-{case}").replace(' ', "-"),
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected the setup Update to answer 200");

        let (status, deleted) = call(
            &reconfigured,
            "DeleteConnectionRecordingPreferences",
            serde_json::json!({ "ClientToken": "reused-delete-token" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Delete to answer 200, got {status:?} with {deleted:?}"
        );

        let (status, got) = call(
            &reconfigured,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a Delete that answered 200 must leave nothing readable, got {status:?} with {got:?}"
        );
    }
}

/// The idempotency store is BOUNDED: the key holds a caller-supplied `ClientToken`, so
/// a client sending a fresh token per request must not grow it without end. Drive far
/// more distinct tokens than the cap and assert the retained count stops rising while
/// the service keeps answering, and that the newest token is still replayable.
#[tokio::test]
async fn the_idempotency_store_stays_bounded() {
    let (router, engine) = enrolled_router_with_engine().await;
    // Comfortably past the crate's cap, so eviction has to have happened for the count
    // to stay put. Asserted against the exported constant, not a literal, so raising
    // the cap cannot silently loosen this check.
    let distinct_tokens: u32 = u32::try_from(MAX_IDEMPOTENCY_OUTCOMES).unwrap_or(u32::MAX / 4) * 2
        + 1;

    let mut counts: Vec<usize> = Vec::new();
    for n in 0..distinct_tokens {
        let (status, updated) = call(
            &router,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": format!("token-{n}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"token {n}\": expected 200, got {status:?} with {updated:?}"
        );
        if n % 500 == 0 || n == distinct_tokens - 1 {
            counts.push(
                engine
                    .state::<ComAmazonawsSsmguiconnectSsmguiconnectState>()
                    .lock()
                    .retained_outcome_count(),
            );
        }
    }

    let highest = counts.iter().copied().max().unwrap_or_default();
    assert!(
        highest <= MAX_IDEMPOTENCY_OUTCOMES,
        "case \"{distinct_tokens} distinct tokens\": the idempotency store must stay inside \
         {MAX_IDEMPOTENCY_OUTCOMES} entries, observed counts {counts:?}"
    );
    assert!(
        highest > 0,
        "case \"{distinct_tokens} distinct tokens\": the store must still record outcomes, \
         observed counts {counts:?}"
    );

    // Eviction is FIFO, so the most recent token is still replayable and answers with
    // the record it wrote rather than writing a second time.
    let (status, replay) = call(
        &router,
        "UpdateConnectionRecordingPreferences",
        serde_json::json!({
            "ConnectionRecordingPreferences": preferences("210987654321", "other-bucket", "k"),
            "ClientToken": format!("token-{}", distinct_tokens - 1),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"replay the newest token\": expected 200, got {status:?} with {replay:?}"
    );
    assert_eq!(
        replay["ConnectionRecordingPreferences"]["RecordingDestinations"]["S3Buckets"][0]
            ["BucketName"]
            .as_str(),
        Some("rdp-recordings"),
        "case \"replay the newest token\": the newest outcome must survive eviction, got {replay:?}"
    );
}

/// Re-recording an outcome under a `ClientToken` the store already holds makes that
/// outcome the NEWEST entry of the bounded store: eviction drops the outcomes in the
/// order they were RECORDED, so it never drops a newer outcome to keep an older one,
/// not even when the newer one reuses a token recorded long before.
///
/// Both mutating operations take that re-record path whenever the token's recorded
/// outcome is no longer current (the resource changed out from under it), so the table
/// has a row per operation that records an outcome and per eviction depth — one entry
/// past the cap, and several. Each row drives real calls through the production router
/// and then reads the retained set off the store, because WHICH entry eviction chose is
/// not observable on the wire.
#[tokio::test]
async fn a_re_recorded_outcome_outlives_the_outcomes_recorded_before_it() {
    /// Outcomes recorded under fresh tokens between the reused token's first recording
    /// and its re-recording: the entries eviction has to consume before it could reach
    /// the re-recorded one.
    const EARLIER: usize = 4;
    /// The token recorded twice — first, and then again after [`EARLIER`] others.
    const REUSED: &str = "reused-token";

    // (case, the operation whose token is reused, outcomes recorded past the cap)
    let cases: [(&str, IdempotentOperation, usize); 4] = [
        (
            "an Update token re-recorded, one eviction",
            IdempotentOperation::Update,
            1,
        ),
        (
            "an Update token re-recorded, several evictions",
            IdempotentOperation::Update,
            3,
        ),
        (
            "a Delete token re-recorded, one eviction",
            IdempotentOperation::Delete,
            1,
        ),
        (
            "a Delete token re-recorded, several evictions",
            IdempotentOperation::Delete,
            3,
        ),
    ];

    for (case, reused_op, overflow) in cases {
        assert!(
            overflow <= EARLIER,
            "case {case:?}: the table must not ask for more evictions ({overflow}) than there \
             are earlier outcomes ({EARLIER})"
        );
        let (router, engine) = enrolled_router_with_engine().await;

        // The request each of the two recording operations is driven with. The OTHER
        // operation is what every fresh-token call below uses, so each of them also
        // changes the resource out from under the reused token — which is what makes the
        // reused token's second call re-record instead of replay.
        let body_for = |op: IdempotentOperation, token: &str| -> (&'static str, serde_json::Value) {
            match op {
                IdempotentOperation::Update => (
                    "UpdateConnectionRecordingPreferences",
                    serde_json::json!({
                        "ConnectionRecordingPreferences":
                            preferences("123456789012", "rdp-recordings", KEY_ARN),
                        "ClientToken": token,
                    }),
                ),
                IdempotentOperation::Delete => (
                    "DeleteConnectionRecordingPreferences",
                    serde_json::json!({ "ClientToken": token }),
                ),
            }
        };
        let other_op = match reused_op {
            IdempotentOperation::Update => IdempotentOperation::Delete,
            IdempotentOperation::Delete => IdempotentOperation::Update,
        };

        // The reused token's FIRST recording.
        let (op, body) = body_for(reused_op, REUSED);
        let (status, response) = call(&router, op, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first call under {REUSED:?} to answer 200, got \
             {status:?} with {response:?}"
        );

        // Outcomes recorded strictly LATER than that one, under fresh tokens.
        for n in 0..EARLIER {
            let (op, body) = body_for(other_op, &format!("earlier-{n}"));
            let (status, response) = call(&router, op, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the earlier-{n} call to answer 200, got {status:?} \
                 with {response:?}"
            );
        }

        // The RE-RECORD: the same token, whose recorded outcome no longer describes the
        // stored resource, so the call runs for real and records a fresh outcome under
        // the key the store already holds.
        let (op, body) = body_for(reused_op, REUSED);
        let (status, response) = call(&router, op, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second call under {REUSED:?} to answer 200, got \
             {status:?} with {response:?}"
        );

        // It RAN rather than replayed: an Update left a readable record written under
        // the reused token, a Delete left nothing readable.
        let (status, got) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        match reused_op {
            IdempotentOperation::Update => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the re-recording Update must leave a readable resource, got \
                     {status:?} with {got:?}"
                );
                assert_eq!(
                    got["ClientToken"].as_str(),
                    Some(REUSED),
                    "case {case:?}: the readable resource must be the one the re-recording \
                     Update wrote, got {got:?}"
                );
            }
            IdempotentOperation::Delete => assert_eq!(
                status, 404,
                "case {case:?}: the re-recording Delete must leave nothing readable, got \
                 {status:?} with {got:?}"
            ),
        }
        let retained = engine
            .state::<ComAmazonawsSsmguiconnectSsmguiconnectState>()
            .lock()
            .retained_outcome_count();
        assert_eq!(
            retained,
            EARLIER + 1,
            "case {case:?}: the re-record must REPLACE the reused token's entry rather than add \
             a second one, got {retained} retained outcomes"
        );

        // Drive the store exactly `overflow` outcomes past its cap, all under fresh
        // tokens, so eviction has to choose `overflow` entries to drop.
        let later = MAX_IDEMPOTENCY_OUTCOMES + overflow - EARLIER - 1;
        for n in 0..later {
            let (op, body) = body_for(other_op, &format!("later-{n}"));
            let (status, response) = call(&router, op, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the later-{n} call to answer 200, got {status:?} with \
                 {response:?}"
            );
        }

        let state = engine.state::<ComAmazonawsSsmguiconnectSsmguiconnectState>();
        let store = state.lock();
        let retained = store.retained_outcome_count();
        assert_eq!(
            retained, MAX_IDEMPOTENCY_OUTCOMES,
            "case {case:?}: the store must stay full at its cap once passed, got {retained}"
        );
        // The key an outcome is recorded under carries the Region the call was made in,
        // which for every call above is the one `request` signs for.
        let reused_key = idempotency_key(reused_op, REGION, REUSED);
        assert!(
            store.recorded_outcome(&reused_key).is_some(),
            "case {case:?}: the outcome re-recorded under {REUSED:?} was recorded after every \
             earlier-* one, so {overflow} eviction(s) must not reach it; the store holds \
             {:?} for it",
            store.recorded_outcome(&reused_key)
        );
        for n in 0..EARLIER {
            let expected_retained = n >= overflow;
            let actual_retained = store
                .recorded_outcome(&idempotency_key(other_op, REGION, &format!("earlier-{n}")))
                .is_some();
            assert_eq!(
                actual_retained, expected_retained,
                "case {case:?}: with {overflow} eviction(s) the least recently recorded outcomes \
                 go first, so earlier-{n} retained should be {expected_retained}, got \
                 {actual_retained}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — a value outside a modeled constraint is ValidationException (400)
// ---------------------------------------------------------------------------

/// Every modeled constraint on `UpdateConnectionRecordingPreferences`' input, one row
/// per constraint and per side of each bound, refused as 400
/// `ValidationException` — and the store left untouched, so a refused request
/// persists nothing.
///
/// The constraints come from `smithy_query(query="operation",
/// operation="UpdateConnectionRecordingPreferences")` and
/// `smithy_query(query="shape", shape="ConnectionRecordingPreferences,RecordingDestinations,S3Bucket")`
/// on this service's model: `ConnectionRecordingPreferences` `@required`,
/// `RecordingDestinations`/`S3Buckets`/`BucketOwner`/`BucketName`/`KMSKeyArn`
/// `@required`, `S3Buckets` `@length(1, 1)`, `BucketOwner` `^[0-9]{12}$`,
/// `BucketName`'s pattern, `KMSKeyArn` `@length(1, 2048)` and `ClientToken`
/// `@length(1, 64)`.
#[tokio::test]
async fn an_input_outside_a_modeled_constraint_is_refused() {
    let long_key: String = "k".repeat(2049);
    let long_token: String = "t".repeat(65);
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "ConnectionRecordingPreferences absent (@required)",
            serde_json::json!({ "ClientToken": "tok" }),
        ),
        (
            "RecordingDestinations absent (@required)",
            serde_json::json!({
                "ConnectionRecordingPreferences": { "KMSKeyArn": KEY_ARN },
                "ClientToken": "tok",
            }),
        ),
        (
            "KMSKeyArn absent (@required)",
            serde_json::json!({
                "ConnectionRecordingPreferences": {
                    "RecordingDestinations": {
                        "S3Buckets": [{ "BucketOwner": "123456789012", "BucketName": "a-bucket" }]
                    }
                },
                "ClientToken": "tok",
            }),
        ),
        (
            "S3Buckets empty (below @length(1, 1))",
            serde_json::json!({
                "ConnectionRecordingPreferences": {
                    "RecordingDestinations": { "S3Buckets": [] },
                    "KMSKeyArn": KEY_ARN,
                },
                "ClientToken": "tok",
            }),
        ),
        (
            "S3Buckets with two entries (above @length(1, 1))",
            serde_json::json!({
                "ConnectionRecordingPreferences": {
                    "RecordingDestinations": {
                        "S3Buckets": [
                            { "BucketOwner": "123456789012", "BucketName": "one-bucket" },
                            { "BucketOwner": "123456789012", "BucketName": "two-bucket" },
                        ]
                    },
                    "KMSKeyArn": KEY_ARN,
                },
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketOwner not twelve digits",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("abc", "a-bucket", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketOwner too short",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("12345678901", "a-bucket", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketOwner too long",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("1234567890123", "a-bucket", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketName too short for its pattern",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "ab", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketName too long for its pattern",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences(
                    "123456789012",
                    "a2345678901234567890123456789012345678901234567890123456789012bc",
                    KEY_ARN,
                ),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketName uppercase",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "My-Bucket", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketName all numeric labels",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "192.168.1.1", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketName with a leading hyphen",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "-bucket", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "BucketName with an empty label",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "a..b", KEY_ARN),
                "ClientToken": "tok",
            }),
        ),
        (
            "KMSKeyArn empty (below @length(1, 2048))",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "a-bucket", ""),
                "ClientToken": "tok",
            }),
        ),
        (
            "KMSKeyArn over @length(1, 2048)",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "a-bucket", &long_key),
                "ClientToken": "tok",
            }),
        ),
        (
            "ClientToken empty (below @length(1, 64))",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "a-bucket", KEY_ARN),
                "ClientToken": "",
            }),
        ),
        (
            "ClientToken over @length(1, 64)",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "a-bucket", KEY_ARN),
                "ClientToken": long_token,
            }),
        ),
    ];

    for (case, body) in cases {
        let router = enrolled_router().await;
        let (status, response) = call(&router, "UpdateConnectionRecordingPreferences", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status:?} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );

        let (status, got) = call(
            &router,
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused Update must persist nothing, got {status:?} with {got:?}"
        );
    }
}

/// A `ClientToken` outside `@length(1, 64)` is refused on Delete too, which carries
/// no other member.
#[tokio::test]
async fn a_delete_client_token_outside_its_length_is_refused() {
    let long_token: String = "t".repeat(65);
    let cases: [(&str, &str); 2] = [("empty", ""), ("over 64", long_token.as_str())];

    for (case, token) in cases {
        // Not enrolled, and it makes no difference: Delete is not gated, and a value
        // outside a modeled bound is refused before anything else either way.
        let router = router();
        let (status, response) = call(
            &router,
            "DeleteConnectionRecordingPreferences",
            serde_json::json!({ "ClientToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status:?} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }
}

/// The bucket-name rule itself, as a decision table over the modeled pattern's three
/// parts: the 3..63 length lookahead, the all-numeric-labels rejection, and the label
/// body. Exercised directly because it is the one modeled `@pattern` the framework's
/// boundary validator cannot compile (it uses look-around), so the handler owns it.
#[test]
fn the_bucket_name_rule_matches_the_modeled_pattern() {
    assert!(
        BUCKET_NAME_PATTERN.contains("(?=^.{3,63}$)"),
        "the recorded pattern must still carry the length lookahead, got {BUCKET_NAME_PATTERN:?}"
    );

    // (case, bucket name, accepted)
    let cases: [(&str, &str, bool); 16] = [
        ("single short label", "abc", true),
        ("hyphenated", "my-rdp-recordings", true),
        ("dotted labels", "logs.rdp.recordings", true),
        ("digit label beside a letter label", "1.rdp", true),
        ("all digits, one label", "123", true),
        (
            "exactly 63 characters",
            "a234567890123456789012345678901234567890123456789012345678901b",
            true,
        ),
        ("two characters: under the lower bound", "ab", false),
        ("one character", "a", false),
        ("empty", "", false),
        (
            "64 characters: over the upper bound",
            "a2345678901234567890123456789012345678901234567890123456789012bc",
            false,
        ),
        ("uppercase", "My-Bucket", false),
        ("underscore", "my_bucket", false),
        ("leading hyphen", "-bucket", false),
        ("trailing hyphen", "bucket-", false),
        ("empty label", "a..b", false),
        ("all numeric labels", "192.168.1.1", false),
    ];

    for (case, name, accepted) in cases {
        let actual = bucket_name_matches_model_pattern(name);
        assert_eq!(
            actual, accepted,
            "case {case:?} ({name:?}): expected accepted={accepted}, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4 — every call leaves a structured, attributable access record
// ---------------------------------------------------------------------------

/// A `tracing` subscriber that renders each event as one `target field=value …` line and
/// keeps the lines so a test can read them back.
///
/// Written against `tracing`'s own `Subscriber` trait rather than pulling in a formatting
/// subscriber crate: the test needs one line per event with its target and fields, which
/// is a dozen lines of `Visit`, and a new dev-dependency would put an entry in the
/// workspace lockfile that this change deliberately leaves untouched.
#[derive(Clone, Default)]
struct CapturedRecords(Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// Every record captured since the last drain, clearing the buffer.
    fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("the audit capture lock"))
    }
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The access key id the audit case signs with, so it can pick its own records out of a
/// process-wide log that sibling tests also write to.
const AUDIT_AKID: &str = "AKIDAUDITCASE";

/// The capture for audit records, installed ONCE as this test binary's global `tracing`
/// subscriber.
///
/// Global rather than scoped on purpose: `tracing` caches per callsite whether any
/// subscriber is interested, the cache is process-wide, and sibling tests run in
/// parallel on other threads — so a thread-local subscriber is raced away as soon as
/// another thread re-evaluates the handler's callsite with no subscriber of its own.
/// A global default is always interested, which makes the capture deterministic;
/// [`AUDIT_AKID`] is what keeps a sibling test's records out of this one's assertions.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// Every call — above all the two STATE-CHANGING ones — emits one structured record on
/// the `emulator::audit` target naming the operation, the identity the request
/// presented, the request target and the outcome.
///
/// A TABLE over the whole lifecycle, so each distinct outcome the service can report is
/// a row: the write, the idempotent replay, the read, the delete, the read that finds
/// nothing, and the refusal. Driven through the production router with the capturing
/// subscriber installed, so what is asserted is the record a real run would emit.
#[tokio::test]
async fn every_call_emits_a_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing audit
    // record: the capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_AKID, "probe");
    let probe = audit_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = enrolled_router().await;
    let good = serde_json::json!({
        "ConnectionRecordingPreferences": preferences("123456789012", "rdp-recordings", KEY_ARN),
        "ClientToken": "audited-token",
    });

    // (case, operation, request body, expected outcome)
    let cases: [(&str, &str, serde_json::Value, &str); 7] = [
        (
            "the write",
            "UpdateConnectionRecordingPreferences",
            good.clone(),
            "preferences-written",
        ),
        (
            "the idempotent replay",
            "UpdateConnectionRecordingPreferences",
            good.clone(),
            "idempotent-replay",
        ),
        (
            "the read",
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
            "preferences-read",
        ),
        (
            "the delete",
            "DeleteConnectionRecordingPreferences",
            serde_json::json!({ "ClientToken": "audited-delete" }),
            "preferences-deleted",
        ),
        (
            "the read that finds nothing",
            "GetConnectionRecordingPreferences",
            serde_json::json!({}),
            "not-found",
        ),
        (
            "the delete that finds nothing configured",
            "DeleteConnectionRecordingPreferences",
            serde_json::json!({ "ClientToken": "audited-delete-again" }),
            "preferences-already-absent",
        ),
        (
            "the refusal",
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "Not-A-Bucket", KEY_ARN),
                "ClientToken": "audited-refusal",
            }),
            "validation-refused",
        ),
    ];

    for (case, operation, body, outcome) in cases {
        let _ = captured.drain();
        let (status, response) = call_as(&router, operation, body, AUDIT_AKID).await;
        let records = audit_records(&captured.drain());

        // One call, one record: a second would mean a branch audited twice.
        assert_eq!(
            records.len(),
            1,
            "case {case:?} (HTTP {status}, body {response:?}): expected exactly one audit \
             record, got {records:?}"
        );
        let record = &records[0];

        for (field, value) in [
            ("service", "ssm-guiconnect"),
            ("operation", operation),
            // The identity the request PRESENTED, which is what the record attributes
            // the call to.
            ("access_key_id", AUDIT_AKID),
            ("request_target", &format!("POST /{operation}")[..]),
            ("outcome", outcome),
        ] {
            // Asserted as the `field=value` pair, not as a bare substring, so a value
            // cannot satisfy the check from the wrong field.
            let pair = format!("{field}={value}");
            assert!(
                record.contains(&pair),
                "case {case:?}: the audit record must carry {pair:?}, got {record:?}"
            );
        }
        // The caller's own configuration must never reach the log.
        for withheld in ["audited-token", "audited-delete", "audited-refusal", KEY_ARN] {
            assert!(
                !record.contains(withheld),
                "case {case:?}: the audit record must stay metadata-only, but it carries \
                 {withheld:?}: {record:?}"
            );
        }
    }

    // The last outcome the service can report needs the OTHER account state: an
    // account that is not enrolled in just-in-time node access, whose Update the gate
    // refuses.
    enroll(&router, false).await;
    let _ = captured.drain();
    let (status, response) = call_as(
        &router,
        "UpdateConnectionRecordingPreferences",
        good,
        AUDIT_AKID,
    )
    .await;
    let records = audit_records(&captured.drain());
    assert_eq!(
        records.len(),
        1,
        "case \"the gate's denial\" (HTTP {status}, body {response:?}): expected exactly one \
         audit record, got {records:?}"
    );
    assert!(
        records[0].contains("outcome=access-denied"),
        "case \"the gate's denial\": the audit record must carry \"outcome=access-denied\", got \
         {:?}",
        records[0]
    );
}

/// The captured records that are THIS case's: on the `emulator::audit` target and
/// presenting [`AUDIT_AKID`]. A sibling test running in parallel writes to the same
/// global capture, so selecting by the presented identity is what keeps the assertions
/// about this case's own calls.
fn audit_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with("emulator::audit ")
                && record.contains(&format!("access_key_id={AUDIT_AKID}"))
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 4 — the resource exists per Region
// ---------------------------------------------------------------------------

/// One call to `op` signed for `region`, built with the same request builder the
/// authorization suite routes every Region's call with — so the only thing that differs
/// between two calls here is the Region of the credential scope.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    op: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    reply_of(
        router
            .dispatch(signed_request(region, DEFAULT_AKID, op, &body))
            .await,
    )
}

/// Enroll the account in just-in-time node access in `region`, over Systems Manager's own
/// API — the precondition every Update below has to satisfy in the Region it is made in.
async fn enroll_in(router: &MultiServiceRouter, region: &str) {
    enroll_region(router, region, true).await;
}

/// A well-formed Update in `region` storing `bucket` under `token`.
async fn update_in(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    token: &str,
) -> (u16, serde_json::Value) {
    call_in(
        router,
        region,
        "UpdateConnectionRecordingPreferences",
        serde_json::json!({
            "ConnectionRecordingPreferences": preferences("123456789012", bucket, KEY_ARN),
            "ClientToken": token,
        }),
    )
    .await
}

/// A Delete in `region` carrying `token`.
async fn delete_in(
    router: &MultiServiceRouter,
    region: &str,
    token: &str,
) -> (u16, serde_json::Value) {
    call_in(
        router,
        region,
        "DeleteConnectionRecordingPreferences",
        serde_json::json!({ "ClientToken": token }),
    )
    .await
}

/// A Get in `region`.
async fn get_in(router: &MultiServiceRouter, region: &str) -> (u16, serde_json::Value) {
    call_in(
        router,
        region,
        "GetConnectionRecordingPreferences",
        serde_json::json!({}),
    )
    .await
}

/// The bucket name a Get (or an Update) response reports, or `None` when it carries no
/// preferences at all.
fn reported_bucket(body: &serde_json::Value) -> Option<&str> {
    body["ConnectionRecordingPreferences"]["RecordingDestinations"]["S3Buckets"][0]["BucketName"]
        .as_str()
}

/// Assert that `region` reports exactly the record `bucket` and `token` describe.
async fn assert_reports(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    token: &str,
    case: &str,
) {
    let (status, got) = get_in(router, region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {region} to report its own record, got {status} with {got:?}"
    );
    assert_eq!(
        reported_bucket(&got),
        Some(bucket),
        "case {case:?}: expected {region} to report {bucket:?}, got {got:?}"
    );
    assert_eq!(
        got["ClientToken"].as_str(),
        Some(token),
        "case {case:?}: expected {region} to report the token of its own last write \
         ({token:?}), got {got:?}"
    );
}

/// Assert that `region` has no configured resource: 404 `ResourceNotFoundException`.
async fn assert_reports_nothing(router: &MultiServiceRouter, region: &str, case: &str) {
    let (status, response) = get_in(router, region).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected {region} to have nothing configured, got {status} with \
         {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException in {region}, got {response:?}"
    );
}

/// Region pairs the mutation cases below run over: the Region the sibling cases use,
/// another one of the same partition, and a pair in which neither is the Region an
/// unsigned request would default to.
const REGION_PAIRS: [(&str, &str, &str); 3] = [
    ("the Region the sibling cases use", REGION, "eu-west-1"),
    ("another Region", "eu-west-1", REGION),
    ("neither Region the default one", "ap-south-1", "eu-central-1"),
];

/// A read reports the record of the Region its own credential scope names: each
/// configured Region answers with ITS record, and a Region nothing configured has no
/// resource to report at all — whatever another Region holds.
///
/// The table varies the Region together with every stored member, so a row answered from
/// another Region's record could not pass; the unconfigured rows are the other side of
/// the same rule.
#[tokio::test]
async fn each_region_reports_its_own_record_and_no_other() {
    // (case, region, bucket owner, bucket name, kms key arn)
    let configured: [(&str, &str, &str, &str, &str); 3] = [
        (
            "the Region the sibling cases use",
            REGION,
            "123456789012",
            "home-rdp-recordings",
            KEY_ARN,
        ),
        (
            "another Region of the same partition",
            "eu-west-1",
            "210987654321",
            "eu.rdp.recordings",
            "k",
        ),
        (
            "a third Region",
            "ap-south-1",
            "999999999999",
            "ap-recordings-1",
            KEY_ARN,
        ),
    ];
    // Regions no call configured.
    let unconfigured = ["eu-central-1", "us-west-2"];

    let router = router();

    for (case, region, owner, bucket, key_arn) in configured {
        enroll_in(&router, region).await;
        let (status, written) = call_in(
            &router,
            region,
            "UpdateConnectionRecordingPreferences",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences(owner, bucket, key_arn),
                "ClientToken": format!("token-in-{region}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write in {region} to answer 200, got {status} with \
             {written:?}"
        );
    }

    // Every write is in, so each Region's read has to pick out its own record.
    for (case, region, owner, bucket, key_arn) in configured {
        let (status, got) = get_in(&router, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {region} to report its record, got {status} with {got:?}"
        );
        let stored = &got["ConnectionRecordingPreferences"];
        assert_eq!(
            stored["KMSKeyArn"].as_str(),
            Some(key_arn),
            "case {case:?}: expected the KMSKeyArn written in {region}, got {got:?}"
        );
        assert_eq!(
            reported_bucket(&got),
            Some(bucket),
            "case {case:?}: expected the BucketName written in {region}, got {got:?}"
        );
        assert_eq!(
            stored["RecordingDestinations"]["S3Buckets"][0]["BucketOwner"].as_str(),
            Some(owner),
            "case {case:?}: expected the BucketOwner written in {region}, got {got:?}"
        );
        assert_eq!(
            got["ClientToken"].as_str(),
            Some(format!("token-in-{region}").as_str()),
            "case {case:?}: expected the ClientToken of {region}'s own write, got {got:?}"
        );
    }

    for region in unconfigured {
        assert_reports_nothing(&router, region, &format!("a read in unconfigured {region}")).await;
    }
}

/// A write and a delete reach the record of the Region the request was signed for and no
/// other: replacing one Region's record leaves another's as it was, and removing one
/// Region's record leaves another's readable.
#[tokio::test]
async fn a_write_or_a_delete_reaches_only_the_requesting_region() {
    for (case, mutated, untouched) in REGION_PAIRS {
        let router = router();
        for region in [mutated, untouched] {
            enroll_in(&router, region).await;
        }

        // Each Region starts with a record of its own, told apart by its bucket name and
        // by the token its write carried.
        for region in [mutated, untouched] {
            let bucket = format!("{region}-first-bucket");
            let token = format!("first-in-{region}");
            let (status, written) = update_in(&router, region, &bucket, &token).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the setup write in {region} to answer 200, got \
                 {status} with {written:?}"
            );
        }
        let kept_bucket = format!("{untouched}-first-bucket");
        let kept_token = format!("first-in-{untouched}");

        // A second write in `mutated` replaces ITS record only.
        let (status, replaced) = update_in(&router, mutated, "replacement-bucket", "replacement")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replacing write in {mutated} to answer 200, got \
             {status} with {replaced:?}"
        );
        assert_reports(&router, mutated, "replacement-bucket", "replacement", case).await;
        assert_reports(&router, untouched, &kept_bucket, &kept_token, case).await;

        // A delete in `mutated` removes ITS record only.
        let (status, removed) = delete_in(&router, mutated, "removal").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete in {mutated} to answer 200, got {status} with \
             {removed:?}"
        );
        assert_reports_nothing(&router, mutated, case).await;
        assert_reports(&router, untouched, &kept_bucket, &kept_token, case).await;
    }
}

/// An `@idempotencyToken` is scoped to the Region it was used in, because the resource it
/// describes is: the same token in another Region is a different call and runs there for
/// real, while a replay within ONE Region still answers that Region's first outcome.
#[tokio::test]
async fn an_idempotency_token_is_scoped_to_its_own_region() {
    /// The one token every call of a case carries, so the only thing telling two calls
    /// apart is the Region they were made in.
    const SHARED: &str = "shared-client-token";

    for (case, first, second) in REGION_PAIRS {
        let router = router();
        for region in [first, second] {
            enroll_in(&router, region).await;
        }

        let (status, written) = update_in(&router, first, "first-region-bucket", SHARED).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write in {first} to answer 200, got {status} with \
             {written:?}"
        );

        // The same token in the other Region: a different call, which stores that
        // Region's own values rather than replaying the first Region's record.
        let (status, elsewhere) = update_in(&router, second, "second-region-bucket", SHARED).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write in {second} to answer 200, got {status} with \
             {elsewhere:?}"
        );
        assert_eq!(
            reported_bucket(&elsewhere),
            Some("second-region-bucket"),
            "case {case:?}: expected the call in {second} to store its own values rather than \
             replay {first}'s, got {elsewhere:?}"
        );
        assert_reports(&router, first, "first-region-bucket", SHARED, case).await;
        assert_reports(&router, second, "second-region-bucket", SHARED, case).await;

        // Within ONE Region the token still replays: the same token answers the record
        // the first call wrote, and writes nothing.
        let (status, replayed) = update_in(&router, first, "would-be-a-replacement", SHARED).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replay in {first} to answer 200, got {status} with \
             {replayed:?}"
        );
        assert_eq!(
            reported_bucket(&replayed),
            Some("first-region-bucket"),
            "case {case:?}: expected the replay in {first} to answer the first call's record, \
             got {replayed:?}"
        );
        assert_reports(&router, first, "first-region-bucket", SHARED, case).await;

        // A Delete token is scoped the same way: the delete in `first` is not the
        // outcome the same token names in `second`, which still has a record to remove.
        let (status, removed) = delete_in(&router, first, SHARED).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete in {first} to answer 200, got {status} with \
             {removed:?}"
        );
        assert_reports_nothing(&router, first, case).await;
        assert_reports(&router, second, "second-region-bucket", SHARED, case).await;

        let (status, removed) = delete_in(&router, second, SHARED).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete in {second} to answer 200, got {status} with \
             {removed:?}"
        );
        assert_reports_nothing(&router, second, case).await;
    }
}

/// The Region a record is filed under comes from the caller's own credential scope, so
/// the records this store holds are capped: an Update that would configure a FURTHER
/// Region past the ceiling is refused with the modelled `ServiceQuotaExceededException`
/// (402) and writes nothing, while a Region already configured keeps serving Update,
/// Get and Delete, and a Delete returns the room it frees.
///
/// The ceiling itself is tested, plus one Region past it and the three operations at it,
/// so each side of the bound is a case.
#[tokio::test]
async fn the_regions_holding_a_record_are_capped() {
    let (router, engine) = router_with_engine();
    // Region names of the store's own making, one per entry up to the ceiling. They are
    // not AWS Region names: what is under test is that the Region of a credential scope
    // cannot grow the store without bound, whatever text it carries.
    let filled: Vec<String> = (0..MAX_CONFIGURED_REGIONS)
        .map(|n| format!("emu-test-{n}"))
        .collect();

    for region in &filled {
        enroll_in(&router, region).await;
        let (status, written) = update_in(&router, region, "capped-bucket", "at-the-cap").await;
        assert_eq!(
            status, 200,
            "case \"filling up to the ceiling\": expected the write in {region} to answer 200, \
             got {status} with {written:?}"
        );
    }

    // One Region past the ceiling: refused, and the refusal says whose ceiling it is.
    let past = "emu-test-one-too-many";
    enroll_in(&router, past).await;
    let (status, refused) = update_in(&router, past, "over-the-cap-bucket", "past-the-cap").await;
    assert_eq!(
        status, 402,
        "case \"one Region past the ceiling\": expected the modelled quota refusal, got {status} \
         with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case \"one Region past the ceiling\": expected ServiceQuotaExceededException, got \
         {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        region_ceiling_message(),
        "case \"one Region past the ceiling\": expected the ceiling's own sentence, got {refused:?}"
    );
    assert_reports_nothing(&router, past, "one Region past the ceiling").await;
    // Nothing was written: the store still holds exactly the Regions it was full with,
    // which the wire cannot show (the refused Region reads 404 whether or not an entry
    // was added for it).
    let configured = engine
        .state::<ComAmazonawsSsmguiconnectSsmguiconnectState>()
        .lock()
        .configured_region_count();
    assert_eq!(
        configured, MAX_CONFIGURED_REGIONS,
        "case \"one Region past the ceiling\": the refused write must leave the store at its \
         ceiling, got {configured} Regions holding a record"
    );

    // At the ceiling the Regions that DO hold a record are served as ever: a replacing
    // Update needs no further room.
    let held = &filled[0];
    let (status, replaced) = update_in(&router, held, "replacement-at-the-cap", "replacement").await;
    assert_eq!(
        status, 200,
        "case \"replacing a record at the ceiling\": expected 200, got {status} with {replaced:?}"
    );
    assert_reports(
        &router,
        held,
        "replacement-at-the-cap",
        "replacement",
        "replacing a record at the ceiling",
    )
    .await;
    let configured = engine
        .state::<ComAmazonawsSsmguiconnectSsmguiconnectState>()
        .lock()
        .configured_region_count();
    assert_eq!(
        configured, MAX_CONFIGURED_REGIONS,
        "case \"replacing a record at the ceiling\": replacing a record must not add a Region, \
         got {configured} Regions holding a record"
    );

    // A Delete frees the room it took, so the Region that was refused can be configured.
    let (status, removed) = delete_in(&router, held, "freeing-room").await;
    assert_eq!(
        status, 200,
        "case \"freeing room\": expected the delete to answer 200, got {status} with {removed:?}"
    );
    let (status, written) = update_in(&router, past, "over-the-cap-bucket", "after-room-freed").await;
    assert_eq!(
        status, 200,
        "case \"freeing room\": expected the write in {past} to be served once a Region's \
         record was removed, got {status} with {written:?}"
    );
    assert_reports(
        &router,
        past,
        "over-the-cap-bucket",
        "after-room-freed",
        "freeing room",
    )
    .await;
    assert_reports_nothing(&router, held, "freeing room").await;
}
