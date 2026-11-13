// SPDX-License-Identifier: CC0-1.0

//! Cover for WHEN `UpdateWebhook` resolves the webhook it rewrites, driven over the wire
//! the router serves (`POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1) through the harness in `project_build_lifecycle`.
//!
//! RULE under test: `UpdateWebhook` has to RESOLVE the webhook it rewrites before it can
//! judge the request against it, so a project this account holds that carries no webhook
//! is refused on the WEBHOOK — HTTP 400, `ResourceNotFoundException`, CodeBuild's fixed
//! absent-webhook sentence — ahead of every precondition that is about the project's
//! stored source (the build type that source takes, the provider access token CodeBuild
//! would carry the provider-side call out with), and nothing is written. The sentence is
//! reserved for that absence: once the project carries a webhook, the same request is
//! judged against it instead.
//!
//! `UpdateWebhookInput` models `projectName` plus the configuration members
//! `branchFilter`, `rotateSecret`, `filterGroups`, `buildType` (`WebhookBuildType`:
//! `BUILD`, `BUILD_BATCH`, `RUNNER_BUILDKITE_BUILD`) and `pullRequestBuildPolicy`, and
//! models `ResourceNotFoundException` at HTTP 400
//! (`smithy_query(query="operation",operation="UpdateWebhook")`,
//! `smithy_query(query="shape",shape="WebhookBuildType")`).
//!
//! The rows span what the rule generalises over: every `SourceType` the model declares
//! (`crate::ops::types::SOURCE_TYPE_VALUES`) — including the ones whose projects cannot
//! carry a webhook at all, and the one whose only webhook is a runner one — crossed with
//! what the request carries (nothing, each configuration member, and both a build type
//! the project's source takes and one it does not) and with whether an access token for
//! that source's provider is held. `webhook_lifecycle` owns the configuration an update
//! applies once a webhook IS there; what is pinned here is the ORDER, which is what keeps
//! the two separately actionable absences — and the request-level refusals — apart.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::SOURCE_TYPE_VALUES;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project_body, emulator_on, error_type, source_of,
};

/// Whether a project whose primary source is `source_type` can carry a webhook at all:
/// the providers that send repository events (`state::WEBHOOK_SOURCE_TYPES`) plus the
/// source type the runner webhook belongs to (`state::RUNNER_WEBHOOK_SOURCE_TYPE`).
fn takes_a_webhook(source_type: &str) -> bool {
    state::WEBHOOK_SOURCE_TYPES.contains(&source_type)
        || source_type == state::RUNNER_WEBHOOK_SOURCE_TYPE
}

/// The `buildType` a webhook of a project whose source is `source_type` carries: a
/// `NO_SOURCE` project's only webhook is the runner one, every other is a
/// repository-event webhook, which the model's default build type covers.
fn build_type_for(source_type: &str) -> &'static str {
    if source_type == state::RUNNER_WEBHOOK_SOURCE_TYPE {
        state::RUNNER_WEBHOOK_BUILD_TYPE
    } else {
        DEFAULT_WEBHOOK_BUILD_TYPE
    }
}

/// The `WebhookBuildType` of a webhook that builds a repository's events
/// (`smithy_query(query="shape", shape="WebhookBuildType")` declares `BUILD`,
/// `BUILD_BATCH` and `RUNNER_BUILDKITE_BUILD`; the last one is the runner build type
/// `state::RUNNER_WEBHOOK_BUILD_TYPE` names).
const DEFAULT_WEBHOOK_BUILD_TYPE: &str = "BUILD";

/// Create `name` with a primary source of `source_type`, asserting it succeeded.
async fn create_project(router: &MultiServiceRouter, name: &str, source_type: &str) {
    let (status, body) = call(
        router,
        "CreateProject",
        create_project_body(name, json!({"source": source_of(source_type)})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?} ({source_type}): expected 200, got {status} with {body}"
    );
}

/// Import an access token for `server_type`, if that source type has a provider to
/// authenticate to at all.
async fn import_credential(router: &MultiServiceRouter, server_type: &str) {
    if !state::WEBHOOK_SOURCE_TYPES.contains(&server_type) {
        return;
    }
    let (status, body) = call(
        router,
        "ImportSourceCredentials",
        json!({
            "serverType": server_type,
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "an-access-token",
            "shouldOverwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ImportSourceCredentials {server_type}: expected 200, got {status} with {body}"
    );
}

/// The project `BatchGetProjects` reports, asserting it is still held.
async fn read_project(router: &MultiServiceRouter, case: &str, project: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [project]})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchGetProjects must be served, got {status} with {body}"
    );
    assert_eq!(
        body["projectsNotFound"],
        json!([]),
        "case {case:?}: the project must still be held, got {body}"
    );
    body["projects"][0].clone()
}

/// The stored `lastModified` of `project`, read out of the very store the handlers write,
/// so "nothing was written" is asserted against the record rather than against a
/// response.
fn stored_stamp(engine: &Arc<engine::InMemoryEngine>, project: &str) -> Option<i64> {
    engine
        .state::<CodeBuildState>()
        .lock()
        .projects
        .get(project)
        .map(|stored| stored.last_modified_millis)
}

/// One `UpdateWebhook` request body for `project`, with `extra`'s members merged in.
fn request(project: &str, extra: &Value) -> Value {
    let mut body = json!({"projectName": project});
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra.clone()) {
        target.extend(members);
    }
    body
}

/// What a row sends beside `projectName`: nothing, each configuration member on its own,
/// and both a build type a repository-event webhook carries and the runner one — so for
/// every source type the table holds a request whose build type that source does NOT take
/// and one it does.
fn variations() -> Vec<(&'static str, Value)> {
    vec![
        ("minimal", json!({})),
        (
            "a branch filter",
            json!({"branchFilter": "^refs/heads/main$"}),
        ),
        ("rotateSecret true", json!({"rotateSecret": true})),
        (
            "an EVENT filter group",
            json!({"filterGroups": [[{"type": "EVENT", "pattern": "PUSH"}]]}),
        ),
        (
            "a pull request build policy",
            json!({"pullRequestBuildPolicy": {"requiresCommentApproval": "DISABLED"}}),
        ),
        (
            "the repository-event build type",
            json!({"buildType": DEFAULT_WEBHOOK_BUILD_TYPE}),
        ),
        (
            "the runner build type",
            json!({"buildType": state::RUNNER_WEBHOOK_BUILD_TYPE}),
        ),
    ]
}

/// THE RULE: for every source type, and whatever the request carries, a held project with
/// no webhook is refused on the webhook before anything about its source is judged — and
/// the refusal writes nothing.
#[tokio::test]
async fn an_update_resolves_the_webhook_before_it_judges_the_projects_source() {
    // How many rows the same request is SERVED for once the project carries a webhook: the
    // sentence under test is reserved for the absence, so it cannot be this operation's
    // only answer.
    let mut served = 0usize;

    for (nth, (variation, extra)) in variations().iter().enumerate() {
        for source_type in SOURCE_TYPE_VALUES {
            for token_held in [false, true] {
                let case = format!(
                    "UpdateWebhook with {variation} on a held {source_type} project with no \
                     webhook (access token held: {token_held})"
                );
                // A fresh emulator per row: no row's answer may depend on the one before.
                let engine = Arc::new(engine::InMemoryEngine::new());
                let router = emulator_on(Arc::clone(&engine));
                let project = format!("uwa-{}-{nth}", source_type.to_ascii_lowercase());
                create_project(&router, &project, source_type).await;
                if token_held {
                    import_credential(&router, source_type).await;
                }

                let before = read_project(&router, &case, &project).await;
                assert!(
                    before["webhook"].is_null(),
                    "case {case:?}: the row must start with no webhook on the project, got \
                     {before}"
                );
                let stamp_before = stored_stamp(&engine, &project);

                let (status, body) =
                    call(&router, "UpdateWebhook", request(&project, extra)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} with {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(state::WEBHOOK_NOT_FOUND_MESSAGE),
                    "case {case:?}: expected CodeBuild's absent-webhook sentence, got {body}"
                );
                // The project RESOLVED, so neither the absent-project sentence nor a
                // judgement about the source — the build type it takes, the token its
                // provider needs — can be what this call answered with.
                assert_ne!(
                    body["message"],
                    json!(state::PROJECT_NOT_FOUND_MESSAGE),
                    "case {case:?}: a project that resolves is never reported absent, got \
                     {body}"
                );
                let stated = body["message"].as_str().unwrap_or_default().to_string();
                assert!(
                    !stated.contains(source_type) && !stated.contains("build type"),
                    "case {case:?}: the absent webhook is what the refusal states, not a \
                     rule about the project's source, got {body}"
                );

                // Nothing was written: the project is still held, still carries no
                // webhook, and its stored lastModified has not moved.
                let after = read_project(&router, &case, &project).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refusal must leave the project untouched, got {after} \
                     where the project read {before}"
                );
                assert_eq!(
                    stored_stamp(&engine, &project),
                    stamp_before,
                    "case {case:?}: nothing changed, so the stored lastModified must stay \
                     at {stamp_before:?}"
                );

                // The other half: the sentence is about the ABSENCE. Where this source
                // type takes a webhook at all, connect the project — the token is what
                // the connecting call needs, so it is imported here for the rows that
                // held none, after the assertion above — and the SAME request is then
                // judged against the webhook it resolves, never refused as absent again.
                if !takes_a_webhook(source_type) {
                    continue;
                }
                import_credential(&router, source_type).await;
                let (status, connected) = call(
                    &router,
                    "CreateWebhook",
                    json!({
                        "projectName": project,
                        "buildType": build_type_for(source_type),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateWebhook must connect the project, got {status} \
                     with {connected}"
                );

                let (status, answered) =
                    call(&router, "UpdateWebhook", request(&project, extra)).await;
                assert_ne!(
                    answered["message"],
                    json!(state::WEBHOOK_NOT_FOUND_MESSAGE),
                    "case {case:?}: a webhook that IS there is never reported absent, got \
                     {status} with {answered}"
                );
                if status == 200 {
                    served += 1;
                    assert!(
                        answered["webhook"]["payloadUrl"].is_string(),
                        "case {case:?}: a served update reports the webhook, got {answered}"
                    );
                } else {
                    // The only other answer left is about the REQUEST: a build type the
                    // project's source does not take.
                    assert_eq!(
                        (status, error_type(&answered)),
                        (400, "InvalidInputException".to_string()),
                        "case {case:?}: the request is what is left to refuse, got {status} \
                         with {answered}"
                    );
                }
            }
        }
    }

    assert!(
        served > 0,
        "the absent-webhook refusal cannot be this operation's only answer: no row was \
         served once its project carried a webhook"
    );
}
