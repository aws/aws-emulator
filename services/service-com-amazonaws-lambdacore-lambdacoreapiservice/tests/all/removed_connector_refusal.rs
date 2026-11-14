// SPDX-License-Identifier: CC0-1.0

//! What this service answers once a connector's deletion has COMPLETED.
//!
//! THE RULE: a `DeleteNetworkConnector` whose identifier names no connector of this
//! account and Region is refused with the modelled `ResourceNotFoundException` — HTTP 404,
//! "the specified network connector does not exist" — and the refusal names the RESOURCE
//! the identifier determines: `Network connector not found: ` followed by the connector's
//! ARN for an identifier that is an ARN or an `Id` (an ARN already carries the resource
//! segment, and an `Id` IS that segment, `NetworkConnectorArn` being
//! `…:network-connector:[a-zA-Z0-9-_]+`), and by the value as supplied for a NAME, which
//! determines no ARN. Which form it is follows from the identifier and not from the
//! operation, so every operation bound under `/{Identifier}` answers one identifier with
//! one message. `DeleteNetworkConnector`'s documented idempotency ("calling delete on a
//! connector that is already deleting or has been deleted succeeds without error") covers a
//! connector this account still HAS — the window in which the record is there in `DELETING`
//! state — and ends the instant the record is gone, which is the same instant
//! `GetNetworkConnector` starts refusing the identifier.
//!
//! The rows span what the rule generalises over: which identifier FORM repeats the delete
//! (name, connector id, unqualified ARN, version-qualified ARN of a historical version and
//! of the latest one), how many versions the connector held, which OPERATION the same
//! identifier is addressed through afterwards, how many times the refused call is repeated,
//! and how many other connectors were deleted in between.
//!
//! The harness (router, request helpers) is the lifecycle suite's, and the message the rule
//! requires of an identifier is `crate::not_found_message`'s, so the two suites state one
//! rule. Each wait here POLLS for the condition it waits on rather than for a span.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::network_connector_lifecycle::{
    at, create, delete, emulator, get, list_versions, minimal_configuration, minimal_create, text,
    update, vpc_egress, Answer,
};
use super::not_found_message::expected_message;

/// How long a poll loop keeps looking before it gives up. Generous: the condition is owed by
/// the emulator's clock, and nothing here measures how long it took.
const DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll loop waits between two looks.
const BETWEEN_LOOKS: Duration = Duration::from_millis(25);

/// Look again until `condition` holds of the answer `GetNetworkConnector` gives for
/// `identifier`, and answer that answer. Panics with `what` when the deadline passes.
async fn poll_get(
    router: &MultiServiceRouter,
    identifier: &str,
    what: &str,
    condition: impl Fn(&Answer) -> bool,
) -> Answer {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let answer = get(router, identifier).await;
        if condition(&answer) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "{identifier:?} never {what}: last answer {} with {:?}",
            answer.status,
            answer.body
        );
        std::thread::sleep(BETWEEN_LOOKS);
    }
}

/// Create `name` and look until it has reached `ACTIVE`, the state a connector must be in to
/// be deletable. Answers the connector's `Id`.
async fn active_connector(router: &MultiServiceRouter, name: &str) -> String {
    let created = create(router, minimal_create(name)).await;
    assert_eq!(
        created.status, 202,
        "creating {name:?} must succeed, got {} with {:?}",
        created.status, created.body
    );
    let settled = poll_get(router, name, "reached ACTIVE", |answer| {
        answer.status == 200 && text(&answer.body, "/State") == "ACTIVE"
    })
    .await;
    text(&settled.body, "/Id")
}

/// Append `count` versions to `name`, looking until each update has finished, so the
/// connector is `ACTIVE` with historical versions to address.
async fn append_versions(router: &MultiServiceRouter, name: &str, count: usize) {
    // Each row is a configuration the connector does not already hold: only "a successful
    // configuration change creates a new immutable version".
    let subnets = [
        "subnet-0fedcba9876543210",
        "subnet-00112233445566778",
        "subnet-0a1b2c3d4e5f60718",
    ];
    for index in 0..count {
        let updated = update(
            router,
            name,
            json!({ "Configuration": vpc_egress(&subnets[index % subnets.len()..][..1], &[], None, None) }),
        )
        .await;
        assert_eq!(
            updated.status, 202,
            "appending a version to {name:?} must succeed, got {} with {:?}",
            updated.status, updated.body
        );
        poll_get(router, name, "finished its update", |answer| {
            answer.status == 200 && text(&answer.body, "/LastUpdateStatus") == "Successful"
        })
        .await;
    }
}

/// Delete `name` and look until the removal the clock owes has completed, i.e. until
/// `GetNetworkConnector` refuses the connector — "after deletion completes, subsequent calls
/// to GetNetworkConnector return ResourceNotFoundException".
async fn delete_and_wait_until_removed(router: &MultiServiceRouter, name: &str) {
    let accepted = delete(router, name).await;
    assert_eq!(
        accepted.status, 202,
        "deleting {name:?} must be accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
    poll_get(router, name, "became absent", |answer| answer.status == 404).await;
}

/// Assert `answer` is the modelled 404 naming the resource `identifier` determines.
fn assert_refused(case: &str, answer: &Answer, identifier: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: an identifier that names no connector is refused with HTTP 404, \
         got {} with {:?}",
        answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with("ResourceNotFoundException"),
        "case {case:?}: expected the error code ResourceNotFoundException, got {code:?}"
    );
    let message = text(&answer.body, "/Message");
    let expected = expected_message(identifier);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

/// Whether `identifier` carries a version suffix, which `UpdateNetworkConnector` and
/// `ListNetworkConnectorVersions` refuse as a bad parameter before any lookup happens.
fn names_a_version(identifier: &str) -> bool {
    identifier.rsplit_once(':').is_some_and(|(head, suffix)| {
        !head.is_empty() && suffix.parse::<u32>().is_ok_and(|version| version >= 1)
    })
}

/// RULE: once the deletion has completed, a repeat delete is refused naming the resource the
/// identifier determines — whichever form addresses it, however many versions the connector
/// held, and however often the call is repeated. `GetNetworkConnector` (and, for an
/// unqualified identifier, the other operations addressed by one) answers the SAME message,
/// so no identifier is absent to one operation and present to another.
#[tokio::test]
async fn a_delete_after_the_removal_is_refused_naming_the_resource_the_identifier_determines() {
    // Rows differ along the identifier form the repeat uses and how many versions the
    // connector held when it was deleted.
    let cases: [(&str, usize, &str); 6] = [
        ("by name, one version", 0, "name"),
        ("by connector id, one version", 0, "id"),
        ("by unqualified ARN, one version", 0, "arn"),
        ("by connector id, three versions", 2, "id"),
        ("by the ARN of a historical version", 2, "version-1"),
        ("by the ARN of the latest version", 2, "latest-version"),
    ];

    for (index, (case, extra_versions, form)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("removed-{index}");
        let id = active_connector(&router, &name).await;
        let arn = text(&get(&router, &name).await.body, "/Arn");
        append_versions(&router, &name, *extra_versions).await;
        let latest = at(&get(&router, &name).await.body, "/LatestVersion")
            .as_i64()
            .unwrap_or_default();
        assert_eq!(
            latest,
            *extra_versions as i64 + 1,
            "case {case:?}: the connector must hold the versions the row asked for, got \
             {latest}"
        );

        delete_and_wait_until_removed(&router, &name).await;

        let identifier = match *form {
            "name" => name.clone(),
            "id" => id.clone(),
            "arn" => arn.clone(),
            "version-1" => format!("{arn}:1"),
            _ => format!("{arn}:{latest}"),
        };

        // The repeat, and a second repeat: the answer is about whether the connector is
        // still there, not about how many times it was asked for.
        for attempt in 1..=2 {
            let repeated = delete(&router, &identifier).await;
            assert_refused(
                &format!("{case}: the delete repeated (attempt {attempt})"),
                &repeated,
                &identifier,
            );
        }

        // And the same identifier through every other operation bound under it: one
        // identifier, one message.
        let reported = get(&router, &identifier).await;
        assert_refused(&format!("{case}: through Get"), &reported, &identifier);
        if !names_a_version(&identifier) {
            let updated = update(
                &router,
                &identifier,
                json!({ "Configuration": minimal_configuration() }),
            )
            .await;
            assert_refused(&format!("{case}: through Update"), &updated, &identifier);
            let versions = list_versions(&router, &identifier, "").await;
            assert_refused(
                &format!("{case}: through ListVersions"),
                &versions,
                &identifier,
            );
        }
    }
}

/// RULE: the refusal depends only on whether the connector is still there, so it is the same
/// answer after other connectors have been created and deleted — nothing the service
/// remembers about an earlier deletion turns a refused call back into a success.
#[tokio::test]
async fn the_refusal_does_not_depend_on_what_was_deleted_since() {
    let router = emulator();
    let name = "first-gone";
    let id = active_connector(&router, name).await;
    delete_and_wait_until_removed(&router, name).await;

    let before = delete(&router, &id).await;
    assert_refused("the removed connector, by id", &before, &id);

    // Two more connectors created and deleted after it, so the answer for the first one is
    // asked for again with other deletions behind it.
    for other in ["second-gone", "third-gone"] {
        active_connector(&router, other).await;
        delete_and_wait_until_removed(&router, other).await;
    }

    let after = delete(&router, &id).await;
    assert_refused("the same removed connector, two deletions later", &after, &id);
    let by_name = delete(&router, name).await;
    assert_refused(
        "the same removed connector, addressed by the name it held",
        &by_name,
        name,
    );
}

/// RULE, the window the idempotent success covers: while the connector still EXISTS in
/// `DELETING` state, a repeat delete is accepted with HTTP 202 and the record's identity —
/// only the repeat taken after the removal completed is refused.
#[tokio::test]
async fn a_repeat_while_the_record_is_still_there_is_accepted() {
    for form in ["name", "id", "arn"] {
        let router = emulator();
        let name = format!("still-deleting-{form}");
        let id = active_connector(&router, &name).await;
        let arn = text(&get(&router, &name).await.body, "/Arn");

        let first = delete(&router, &name).await;
        assert_eq!(
            first.status, 202,
            "case {form:?}: the first delete is accepted, got {} with {:?}",
            first.status, first.body
        );

        let identifier = match form {
            "id" => id.clone(),
            "arn" => arn.clone(),
            _ => name.clone(),
        };
        let repeated = delete(&router, &identifier).await;
        assert_eq!(
            repeated.status, 202,
            "case {form:?}: a repeat while the connector is still DELETING is accepted, \
             got {} with {:?}",
            repeated.status, repeated.body
        );
        for (member, expected) in [("/Arn", arn.as_str()), ("/Name", name.as_str()), ("/Id", id.as_str())] {
            assert_eq!(
                text(&repeated.body, member),
                expected,
                "case {form:?}: the repeat reports the stored {member} {expected:?}, got {:?}",
                at(&repeated.body, member)
            );
        }
        assert_eq!(
            text(&repeated.body, "/State"),
            "DELETING",
            "case {form:?}: expected State \"DELETING\", got {:?}",
            at(&repeated.body, "/State")
        );
    }
}
