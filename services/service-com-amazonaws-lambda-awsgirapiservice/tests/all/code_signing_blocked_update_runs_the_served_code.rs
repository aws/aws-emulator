// SPDX-License-Identifier: CC0-1.0

//! What a deployment BLOCKED by code signing leaves behind: the code the function was
//! already running.
//!
//! RULE under test: a deployment to a function attached to a code-signing configuration is
//! verified against that configuration's `AllowedPublishers` ("A signing profile defines a
//! trusted user who can sign a code package"), and under
//! `CodeSigningPolicies$UntrustedArtifactOnDeployment` = `Enforce` — "Lambda blocks the
//! deployment request if signature validation checks fail" — a package this emulator cannot
//! attribute to one of those signing profile versions is refused with the modelled
//! `CodeVerificationFailedException` (HTTP 400) and CHANGES NOTHING: the function keeps
//! reporting the digest and the publisher of the package it already held, and an invocation
//! made after the refusal still RUNS that package's handler rather than the rejected one. The
//! refusal is attributable to the signer alone, so the very same entries signed by an allowed
//! publisher then deploy and run.
//!
//! The sibling suite [`crate::code_signing_enforcement`] pins which packages pass the checks
//! and which the policy blocks; what is pinned HERE is the other half of a blocked
//! deployment — that the running code, and not only the reported configuration, is left as it
//! was — which only an invocation can show. Each row therefore drives real HTTP requests
//! through the router for the whole sequence: a signed deployment that lands and runs, a
//! refusal, the state and the run after it, and the same code re-signed by the allowed
//! publisher landing and running.
//!
//! The rows differ along what the rule generalises over: HOW the submitted package fails to
//! be one an allowed publisher signed — signed by another publisher, an envelope naming the
//! allowed publisher but made under another publisher's key, and no signature at all.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::code_signing_enforcement::{archive_of, publisher, SigningKey, JOB_ID};
use crate::function_lifecycle::{
    create_active_function, expected_code_sha256, function_arn, json_call, make_update_due, send,
    zip_file, Answer,
};

/// Where `CreateCodeSigningConfig` is bound (`POST /2020-04-22/code-signing-configs`).
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// The `CodeSigningPolicy` value that BLOCKS a deployment whose signature checks failed,
/// from `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`).
const POLICY_ENFORCE: &str = "Enforce";

/// The modelled error a blocked deployment answers, and its status, from
/// `smithy_query(query="shape", shape="CodeVerificationFailedException")` (`httpError` 400).
const CODE_VERIFICATION_FAILED: &str = "CodeVerificationFailedException";
const BLOCKED_STATUS: u16 = 400;

/// The `LastUpdateStatus` an update settles to once the clock has made its completion due.
const UPDATE_SUCCESSFUL: &str = "Successful";

/// The entry a package carries its handler in, named by the `index.handler` handler
/// [`create_active_function`] creates every function with.
const HANDLER_ENTRY: &str = "index.py";

/// Lambda and the SIGNING service on ONE engine: a deployment package's signature is verified
/// against the key pair that service holds for the signing profile version it names.
fn emulator(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_signer_wallabyservice::EmulatedService::new(Arc::clone(engine)),
    );
    router
}

/// A handler whose answer NAMES the generation of code it came from and echoes the event it
/// was called with, so an invocation says WHICH deployment ran it.
fn handler_entries(generation: &str) -> Vec<(&'static str, Vec<u8>)> {
    let source = format!(
        "def handler(event, context):\n    \
         return {{'generation': '{generation}', 'echo': event.get('probe')}}\n"
    );
    vec![(HANDLER_ENTRY, source.into_bytes())]
}

/// A deployment package holding `entries`, signed by `key` as coming from `attributed_to`:
/// the envelope the signing service writes into an artifact it signed, over the digest of the
/// contents that artifact travels with
/// (`service_common::code_signing_signature::content_digest`).
fn signed(key: &SigningKey, attributed_to: &str, entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let contents = archive_of(entries);
    let digest = service_common::code_signing_signature::content_digest(&contents)
        .expect("the package's contents digest");
    let (signature, public_key) = key
        .signature_and_public_key(&service_common::code_signing_signature::signed_material(
            attributed_to,
            JOB_ID,
            &digest,
        ))
        .expect("the signing profile version signs");
    let envelope = service_common::code_signing_signature::render_envelope(
        &digest,
        &signature,
        &public_key,
        attributed_to,
        JOB_ID,
    );
    let mut all: Vec<(&str, Vec<u8>)> = entries.to_vec();
    all.push((
        service_common::code_signing_signature::SIGNATURE_ENTRY_NAME,
        envelope,
    ));
    archive_of(&all)
}

/// `CreateCodeSigningConfig` under the `Enforce` policy, allowing exactly `allowed`.
async fn enforcing_config(router: &MultiServiceRouter, case: &str, allowed: &str) -> String {
    let answer = json_call(
        router,
        "POST",
        CODE_SIGNING_CONFIGS_URI,
        &json!({
            "AllowedPublishers": { "SigningProfileVersionArns": [allowed] },
            "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateCodeSigningConfig must answer 201, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()["CodeSigningConfig"]["CodeSigningConfigArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: CreateCodeSigningConfig must answer an ARN, got {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
        .to_string()
}

/// `PutFunctionCodeSigningConfig`, which must attach the configuration to the function.
async fn attach_config(router: &MultiServiceRouter, case: &str, function: &str, arn: &str) {
    let answer = json_call(
        router,
        "PUT",
        &format!("/2020-06-30/functions/{function}/code-signing-config"),
        &json!({ "CodeSigningConfigArn": arn }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: PutFunctionCodeSigningConfig must answer 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `UpdateFunctionCode` deploying `package` inline.
async fn update_code(router: &MultiServiceRouter, function: &str, package: &[u8]) -> Answer {
    json_call(
        router,
        "PUT",
        &format!("/2015-03-31/functions/{function}/code"),
        &json!({ "ZipFile": zip_file(package) }),
    )
    .await
}

/// `GetFunctionConfiguration` on `function`.
async fn configuration(router: &MultiServiceRouter, function: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/2015-03-31/functions/{function}/configuration"),
        &Value::Null,
    )
    .await
}

/// Bring the update's completion — which the CLOCK owes — forward, and read the function
/// back, which is what materialises it. The package a deployment submits takes over from the
/// one that was serving when the window closes, so this is what waiting the window out would
/// do, without waiting for it.
async fn settled(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
) {
    make_update_due(engine);
    let answer = configuration(router, function).await;
    assert_eq!(
        answer.string("LastUpdateStatus"),
        Some(UPDATE_SUCCESSFUL.to_string()),
        "case {case:?}: expected the deployment to settle to {UPDATE_SUCCESSFUL}, got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// `Invoke` the function and assert the handler that RAN is the one of the generation
/// `generation` — a run that answered the event this call carried, so the answer is this
/// invocation's own.
async fn assert_running_generation(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    generation: &str,
) {
    let probe = format!("{case}/{generation}");
    let answer = send(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/invocations"),
        "application/json",
        serde_json::to_vec(&json!({ "probe": probe })).expect("the event serializes"),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: Invoke must answer 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.header("x-amz-function-error"),
        None,
        "case {case:?}: the invocation must have run the handler, got the error signal {:?} and \
         {}",
        answer.header("x-amz-function-error"),
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.json(),
        json!({ "generation": generation, "echo": probe }),
        "case {case:?}: expected the invocation to run the {generation:?} handler on its own \
         event, got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// The refusal Lambda words for a deployment its code-signing checks rejected under the
/// `Enforce` policy: three sentences whose only variable part is the function the deployment
/// was of.
fn blocked_message(function: &str) -> String {
    format!(
        "Lambda cannot deploy the function. The function or layer might be signed using a \
         signature that the client is not configured to accept. Check the provided signature \
         for {}.",
        function_arn(function)
    )
}

/// How a row's rejected package fails to be one the configuration's allowed publisher signed.
#[derive(Debug, Clone, Copy)]
enum NotSignedBy {
    /// Signed, as it claims, by a profile the configuration does not allow.
    AnotherPublisher,
    /// An envelope naming the allowed publisher, made under another profile's key.
    AnotherPublishersKey,
    /// No signature at all.
    Nothing,
}

/// RULE: a deployment an `Enforce` code-signing configuration blocks changes nothing — the
/// function reports the package it already held and still RUNS that package's handler — and
/// the same code signed by an allowed publisher deploys and runs.
#[tokio::test]
async fn a_blocked_update_leaves_the_function_running_the_package_it_already_served() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let release = publisher(&engine, &router, "the allowed publisher", "Release_Profile").await;
    let other = publisher(&engine, &router, "another publisher", "Candidate_Profile").await;

    // (case, the function, how the rejected package fails to be signed by an allowed publisher)
    let rows: Vec<(&str, &str, NotSignedBy)> = vec![
        (
            "signed by a publisher the configuration does not allow",
            "csc-run-other-publisher",
            NotSignedBy::AnotherPublisher,
        ),
        (
            "an envelope naming the allowed publisher, signed under another key",
            "csc-run-another-key",
            NotSignedBy::AnotherPublishersKey,
        ),
        (
            "no signature at all",
            "csc-run-unsigned",
            NotSignedBy::Nothing,
        ),
    ];

    for (case, function, failure) in rows {
        let config = enforcing_config(&router, case, &release.version_arn).await;
        // The function starts from an UNSIGNED package, which is why the enforcing
        // configuration is attached after the create: that create's own deployment is verified
        // the same way.
        create_active_function(
            &router,
            &engine,
            function,
            &archive_of(&handler_entries("created")),
        )
        .await;
        attach_config(&router, case, function, &config).await;

        // THE POSITIVE CONTROL: a package the allowed publisher signed is deployed, and the
        // handler it carries is the one that runs.
        let first_entries = handler_entries("first");
        let first = signed(&release.key, &release.version_arn, &first_entries);
        let deployed = update_code(&router, function, &first).await;
        assert_eq!(
            deployed.status,
            200,
            "case {case:?}: expected 200 from UpdateFunctionCode for a package the allowed \
             publisher signed, got {} {}",
            deployed.status,
            String::from_utf8_lossy(&deployed.body)
        );
        assert_eq!(
            deployed.string("CodeSha256"),
            Some(expected_code_sha256(&first)),
            "case {case:?}: expected the deployed package's own digest, got {}",
            String::from_utf8_lossy(&deployed.body)
        );
        settled(&engine, &router, case, function).await;
        assert_running_generation(&router, case, function, "first").await;

        // THE BLOCKED DEPLOYMENT: the next generation of the same handler, in a package no
        // allowed publisher signed.
        let second_entries = handler_entries("second");
        let rejected = match failure {
            NotSignedBy::AnotherPublisher => {
                signed(&other.key, &other.version_arn, &second_entries)
            }
            NotSignedBy::AnotherPublishersKey => {
                signed(&other.key, &release.version_arn, &second_entries)
            }
            NotSignedBy::Nothing => archive_of(&second_entries),
        };
        assert_ne!(
            expected_code_sha256(&rejected),
            expected_code_sha256(&first),
            "case {case:?}: the rejected package must be other bytes than the deployed one"
        );
        let refused = update_code(&router, function, &rejected).await;
        assert_eq!(
            refused.status,
            BLOCKED_STATUS,
            "case {case:?}: expected {BLOCKED_STATUS} from UpdateFunctionCode for a package no \
             allowed publisher signed, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            CODE_VERIFICATION_FAILED,
            "case {case:?}: expected {CODE_VERIFICATION_FAILED}, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.string("Message"),
            Some(blocked_message(function)),
            "case {case:?}: expected the refusal to name the function whose deployment was \
             blocked, got {}",
            String::from_utf8_lossy(&refused.body)
        );

        // Nothing of it landed: the reported package, the publisher it is attributed to, and
        // the update status are the ones the admitted deployment left...
        let after = configuration(&router, function).await;
        assert_eq!(
            after.string("CodeSha256"),
            Some(expected_code_sha256(&first)),
            "case {case:?}: a blocked deployment must leave the stored package alone, got {}",
            String::from_utf8_lossy(&after.body)
        );
        assert_eq!(
            after.string("SigningProfileVersionArn"),
            Some(release.version_arn.clone()),
            "case {case:?}: a blocked deployment must leave the reported publisher alone, got {}",
            String::from_utf8_lossy(&after.body)
        );
        assert_eq!(
            after.string("LastUpdateStatus"),
            Some(UPDATE_SUCCESSFUL.to_string()),
            "case {case:?}: a blocked deployment must open no update window, got {}",
            String::from_utf8_lossy(&after.body)
        );
        // ... and so is the code that RUNS: the rejected handler never reached the function.
        assert_running_generation(&router, case, function, "first").await;

        // The refusal is attributable to the SIGNER alone: the very same entries, signed by
        // the allowed publisher, deploy and run.
        let resigned = signed(&release.key, &release.version_arn, &second_entries);
        let landed = update_code(&router, function, &resigned).await;
        assert_eq!(
            landed.status,
            200,
            "case {case:?}: expected 200 from UpdateFunctionCode once the same code is signed \
             by the allowed publisher, got {} {}",
            landed.status,
            String::from_utf8_lossy(&landed.body)
        );
        assert_eq!(
            landed.string("CodeSha256"),
            Some(expected_code_sha256(&resigned)),
            "case {case:?}: expected the re-signed package's own digest, got {}",
            String::from_utf8_lossy(&landed.body)
        );
        settled(&engine, &router, case, function).await;
        assert_running_generation(&router, case, function, "second").await;
    }
}
