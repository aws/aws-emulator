// SPDX-License-Identifier: CC0-1.0

//! Cover for what `DeleteProject` reports about the deleted project's WEBHOOK, driven
//! over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.DeleteProject` on awsJson1_1.
//!
//! RULE: a project is connected to its repository's events by the webhook record
//! CodeBuild keeps against it, and deleting the project disconnects it — so the delete
//! answers the disposition of that webhook in `webhookDeletedStatus`, computed from the
//! project's webhook state: `state::WEBHOOK_DELETED_STATUS` when the call removed such a
//! record and `state::WEBHOOK_NOT_DELETED_STATUS` when there was none to remove. The
//! member is not in the published `DeleteProjectOutput`; it is declared in this
//! emulator's copy of the model from the operation's recorded 200 body, which carries it.
//!
//! The table spans the ways a project comes to have a webhook or not: every source type
//! the model allows a webhook for, a source type that allows none, a project that never
//! had one, a project whose webhook was deleted on its own beforehand, a name this
//! account holds no project for, and the second delete of the same project.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project, emulator, source_of};

/// Delete `name`, asserting the call succeeded, and hand back the disposition it reported.
async fn delete_project(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = call(router, "DeleteProject", json!({"name": name})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteProject {name:?} expected 200, got {status} with {body}"
    );
    body["webhookDeletedStatus"].clone()
}

/// Hold an access token for `server_type`, which a webhook CodeBuild registers with a
/// repository provider needs (`webhook_access_token` owns that rule).
async fn import_source_credentials(router: &MultiServiceRouter, server_type: &str) {
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
        "ImportSourceCredentials ({server_type}): expected 200, got {status} with {body}"
    );
}

/// Create `project` with a `source_type` source and, when `with_webhook`, connect a
/// webhook to it.
async fn project_with(
    router: &MultiServiceRouter,
    project: &str,
    source_type: &str,
    with_webhook: bool,
) {
    create_project(router, project, json!({"source": source_of(source_type)})).await;
    if !with_webhook {
        return;
    }
    import_source_credentials(router, source_type).await;
    let (status, body) = call(router, "CreateWebhook", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "CreateWebhook {project:?} ({source_type}): expected 200, got {status} with {body}"
    );
}

/// RULE: the disposition reports whether this delete removed a webhook record.
///
/// The rows differ along the source type the project was created with (every type
/// `state::WEBHOOK_SOURCE_TYPES` admits a webhook for, plus one that admits none) and
/// along whether a webhook was connected at all.
#[tokio::test]
async fn deleting_a_project_reports_what_became_of_its_webhook() {
    // (case, the project's source type, whether a webhook is connected, the disposition)
    let mut rows: Vec<(String, &str, bool)> = Vec::new();
    for source_type in state::WEBHOOK_SOURCE_TYPES {
        rows.push((
            format!("a {source_type} project carrying a webhook"),
            source_type,
            true,
        ));
        rows.push((
            format!("a {source_type} project carrying none"),
            source_type,
            false,
        ));
    }
    // A source type no webhook can be created for: the project can never carry one.
    rows.push((
        "a NO_SOURCE project, which no webhook can be created for".to_string(),
        state::NO_SOURCE_TYPE,
        false,
    ));

    for (index, (case, source_type, with_webhook)) in rows.into_iter().enumerate() {
        // A fresh emulator per row: a row's state must not depend on the one before.
        let router = emulator();
        let project = format!("webhook-disposition-{index}");
        project_with(&router, &project, source_type, with_webhook).await;

        let expected = if with_webhook {
            state::WEBHOOK_DELETED_STATUS
        } else {
            state::WEBHOOK_NOT_DELETED_STATUS
        };
        let reported = delete_project(&router, &case, &project).await;
        assert_eq!(
            reported,
            json!(expected),
            "case {case:?}: expected the disposition {expected:?}, got {reported}"
        );

        // The delete really happened: the project is gone from the account's reads, so
        // the disposition is about state this call changed rather than a fixed answer.
        let (status, body) = call(&router, "BatchGetProjects", json!({"names": [&project]})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetProjects expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["projectsNotFound"],
            json!([project.clone()]),
            "case {case:?}: the deleted project must be gone, got {body}"
        );

        // And the webhook went with it: the SECOND delete of the same name has no
        // webhook to remove, whatever the first one found.
        let again = delete_project(&router, &case, &project).await;
        assert_eq!(
            again,
            json!(state::WEBHOOK_NOT_DELETED_STATUS),
            "case {case:?}: a repeated delete removes no webhook, got {again}"
        );
    }
}

/// RULE, at the edges: a name this account holds no project for, and a project whose
/// webhook was deleted on its own beforehand, both report that there was no webhook to
/// remove — the disposition follows the state the call found, not the request.
#[tokio::test]
async fn a_delete_that_finds_no_webhook_reports_that_there_was_none() {
    let router = emulator();

    // (case, the name to delete, how the state is set up beforehand)
    let absent = delete_project(&router, "a name no project exists under", "never-created").await;
    assert_eq!(
        absent,
        json!(state::WEBHOOK_NOT_DELETED_STATUS),
        "a name no project exists under carries no webhook either, got {absent}"
    );

    let project = "webhook-deleted-first";
    project_with(&router, project, "GITHUB", true).await;
    let (status, body) = call(&router, "DeleteWebhook", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "DeleteWebhook {project:?}: expected 200, got {status} with {body}"
    );
    let reported = delete_project(&router, "a project whose webhook was deleted first", project)
        .await;
    assert_eq!(
        reported,
        json!(state::WEBHOOK_NOT_DELETED_STATUS),
        "a project whose webhook was already deleted has none left to remove, got {reported}"
    );
}

/// The disposition is a VALUE a client branches on, so the two tokens must differ and
/// neither may be empty: a single token would make the answer carry no information.
#[test]
fn the_two_dispositions_are_distinct_tokens() {
    let deleted = state::WEBHOOK_DELETED_STATUS;
    let none = state::WEBHOOK_NOT_DELETED_STATUS;
    assert_ne!(
        deleted, none,
        "the two webhook dispositions must be distinguishable, got {deleted:?} and {none:?}"
    );
    assert!(
        !deleted.is_empty() && !none.is_empty(),
        "neither disposition may be empty, got {deleted:?} and {none:?}"
    );
    assert_eq!(
        state::webhook_deleted_status(true),
        deleted,
        "a delete that removed a webhook reports {deleted:?}"
    );
    assert_eq!(
        state::webhook_deleted_status(false),
        none,
        "a delete that found none reports {none:?}"
    );
}
