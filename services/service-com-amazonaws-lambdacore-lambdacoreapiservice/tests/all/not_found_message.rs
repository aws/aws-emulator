// SPDX-License-Identifier: CC0-1.0

//! How this service REFUSES a connector it does not hold.
//!
//! THE RULE: every operation addressed by a `NetworkConnectorIdentifier` — "a flexible
//! identifier that accepts a network connector ID, name, or ARN"
//! (`smithy_query(query="shape",shape="NetworkConnectorIdentifier")`) — refuses an
//! identifier that names no connector, and an identifier that qualifies a version the
//! connector does not hold, with `HTTP 404` / `ResourceNotFoundException` whose `Message`
//! is BUILT FROM THE REQUEST: `Network connector not found: <the resource the identifier
//! determines>`. Which form that resource takes follows from the IDENTIFIER and not from
//! the operation: an ARN is already one, and an `Id` IS an ARN's resource part
//! (`NetworkConnectorArn` is `…:network-connector:[a-zA-Z0-9-_]+`), so an id-addressed
//! request is refused naming the ARN it determines, while a NAME determines no ARN and is
//! stated exactly as supplied. A version-qualified identifier keeps its suffix. Nothing is
//! read off a record — the record is gone, or never existed.
//!
//! The rows span what the rule generalises over:
//!
//! * which OPERATION refuses (`GetNetworkConnector`, `UpdateNetworkConnector`,
//!   `DeleteNetworkConnector`, `ListNetworkConnectorVersions` — every operation this model
//!   binds under `/{Identifier}`), since the message is the service's and not one
//!   handler's, and every one of them states the same resource for the same identifier;
//! * which identifier FORM was supplied (a name, a connector id, an unqualified ARN, a
//!   version-qualified ARN), and the length of it (one character, and the 140 characters
//!   `NetworkConnectorIdentifier`'s `@length` admits at most);
//! * WHY the lookup found nothing: a connector that never existed, one that existed and was
//!   deleted (so the record the handler would have read is gone), and a version-qualified
//!   identifier naming a version a live connector does not hold.
//!
//! `DeleteNetworkConnector`'s `idempotent: true` success covers a connector this account
//! still HAS, whatever state its own deletion left it in; once the deletion has COMPLETED
//! the identifier names no connector of this account and Region, so that operation refuses
//! it under this rule like every other one (`crate::delete_idempotence` states the window
//! the idempotent success covers).
//!
//! The harness (router, request helpers) is the lifecycle suite's; it is `pub(crate)` there
//! so every suite drives the emulator the same way.

use serde_json::{json, Value};
use service_com_amazonaws_lambdacore_lambdacoreapiservice::state::names_a_connector_id;
use smithy_server_runtime::MultiServiceRouter;

use super::network_connector_lifecycle::{
    create, delete, emulator, expected_arn, get, list_versions, minimal_configuration,
    minimal_create, text, update, wait_for_the_clock, Answer,
};

/// The error code every one of these refusals carries
/// (`ResourceNotFoundException`, `@httpError 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The longest identifier the model admits (`NetworkConnectorIdentifier` is
/// `@length(min: 1, max: 140)`), so a row can state the bound rather than a value near it.
const MAX_IDENTIFIER_LENGTH: usize = 140;

/// Each operation this model binds under `/{Identifier}`.
#[derive(Clone, Copy, Debug)]
enum Addressed {
    Get,
    Update,
    Delete,
    ListVersions,
}

impl Addressed {
    /// Every one of them, so a row never covers a subset by accident.
    const ALL: [Self; 4] = [Self::Get, Self::Update, Self::Delete, Self::ListVersions];

    /// The operations an identifier of this FORM can reach the not-found path through.
    ///
    /// A version-qualified identifier is refused by `UpdateNetworkConnector` and by
    /// `ListNetworkConnectorVersions` BEFORE any lookup happens — "each configuration
    /// update creates a new immutable version", so a request to update one names something
    /// that cannot be updated, and a request to list the versions OF one version names
    /// something that has none. Both are a different refusal (400
    /// `InvalidParameterValueException`, stated by those handlers) and not this rule's.
    /// `GetNetworkConnector` and `DeleteNetworkConnector` do resolve a qualified
    /// identifier — "a specific historical version" — so both refuse it here.
    fn reaching(identifier: &str) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|operation| {
                !matches!(operation, Self::Update | Self::ListVersions)
                    || !names_a_version(identifier)
            })
            .collect()
    }

    /// The operations of [`Addressed::reaching`] that answer for a connector whose record is
    /// gone — all of them, including `DeleteNetworkConnector`: the model marks that one
    /// `idempotent: true`, which makes a delete of a connector this account still HAS
    /// succeed whatever state its own deletion left it in, while a deletion that has
    /// COMPLETED leaves an identifier that names no connector of this account and Region at
    /// all, which is what `ResourceNotFoundException` is documented for.
    fn reporting(identifier: &str) -> Vec<Self> {
        Self::reaching(identifier)
    }
}

/// Whether `identifier` qualifies a version: its last colon-separated segment is a positive
/// integer (`<name-or-arn>:<version>`).
fn names_a_version(identifier: &str) -> bool {
    identifier
        .rsplit_once(':')
        .is_some_and(|(head, suffix)| !head.is_empty() && suffix.parse::<u32>().is_ok_and(|version| version >= 1))
}

/// Address `identifier` through `operation`.
async fn send(
    router: &MultiServiceRouter,
    operation: Addressed,
    identifier: &str,
) -> Answer {
    match operation {
        Addressed::Get => get(router, identifier).await,
        // Every member of the update request but `Identifier` is optional, so the body
        // carries only a configuration — the refusal under test is about the identifier.
        Addressed::Update => {
            update(
                router,
                identifier,
                json!({ "Configuration": minimal_configuration() }),
            )
            .await
        }
        Addressed::Delete => delete(router, identifier).await,
        Addressed::ListVersions => list_versions(router, identifier, "").await,
    }
}

/// The message the rule requires for `identifier`, whichever operation refuses it: the ARN
/// of the connector the identifier determines (an `Id` determines one, an ARN is one
/// already and is stated as supplied), and the identifier as supplied for a name, which
/// determines no ARN.
pub(crate) fn expected_message(identifier: &str) -> String {
    let (addressed, version) = match identifier.rsplit_once(':') {
        Some((head, suffix)) if names_a_version(identifier) => (head, Some(suffix)),
        _ => (identifier, None),
    };
    let names_the_resource =
        !identifier.contains(":network-connector:") && names_a_connector_id(addressed);
    if !names_the_resource {
        return format!("Network connector not found: {identifier}");
    }
    let arn = expected_arn(addressed);
    match version {
        Some(version) => format!("Network connector not found: {arn}:{version}"),
        None => format!("Network connector not found: {arn}"),
    }
}

/// Assert the answer is the modelled 404 naming the resource `identifier` determines.
fn assert_not_found(case: &str, identifier: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: an identifier that names no connector is refused with HTTP 404, \
         got {} with {:?}",
        answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with(RESOURCE_NOT_FOUND),
        "case {case:?}: expected the error code {RESOURCE_NOT_FOUND}, got {code:?}"
    );
    let message = text(&answer.body, "/Message");
    let expected = expected_message(identifier);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
    // Whichever form the caller used, the resource it named is in the message.
    let named = identifier
        .rsplit_once(":network-connector:")
        .map_or(identifier, |(_, tail)| tail);
    assert!(
        message.contains(named),
        "case {case:?}: the refusal must name the resource the request addressed \
         ({named:?}), got {message:?}"
    );
}

/// RULE: a connector that was never created is refused by every identifier-addressed
/// operation, each naming the resource the identifier determines.
#[tokio::test]
async fn an_identifier_that_names_no_connector_is_refused_naming_it() {
    let router = emulator();
    // One identifier per FORM the model admits, plus the two length bounds.
    let identifiers = [
        ("a plain name", "absent-connector".to_string()),
        ("a name with underscores", "absent_connector_01".to_string()),
        (
            "the connector-id form the service mints",
            "nc-0000000000000001".to_string(),
        ),
        (
            "an unqualified ARN",
            expected_arn("absent-connector"),
        ),
        (
            "a version-qualified ARN",
            format!("{}:7", expected_arn("absent-connector")),
        ),
        ("the shortest identifier", "a".to_string()),
        (
            "the longest identifier the model admits",
            "n".repeat(MAX_IDENTIFIER_LENGTH),
        ),
    ];

    for (case, identifier) in &identifiers {
        for operation in Addressed::reaching(identifier) {
            let answer = send(&router, operation, identifier).await;
            assert_not_found(
                &format!("{case} through {operation:?}"),
                identifier,
                &answer,
            );
        }
    }
}

/// RULE, the case the recording was taken from: a connector that EXISTED and was deleted is
/// refused naming the resource the identifier determines — never the id of the record that
/// is gone when the caller did not name it.
#[tokio::test]
async fn a_deleted_connector_is_refused_naming_the_identifier_that_was_asked_for() {
    let router = emulator();
    let name = "deleted-connector";
    let created = create(&router, minimal_create(name)).await;
    assert_eq!(
        created.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        created.status, created.body
    );
    let id = text(&created.body, "/Id");
    let arn = text(&created.body, "/Arn");
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "a created connector reports its Id and Arn, got {:?}",
        created.body
    );
    // A connector is created PENDING and the clock owes it ACTIVE; a deletion is refused
    // until then, so the row waits rather than racing the transition.
    wait_for_the_clock();

    let removed = delete(&router, name).await;
    assert!(
        removed.status < 300,
        "expected DeleteNetworkConnector to accept the deletion, got {} with {:?}",
        removed.status, removed.body
    );
    // The deletion the call started is owed by the clock; once it falls due the record is
    // forgotten and every form of the identifier names nothing.
    wait_for_the_clock();

    // Every form the connector could be addressed by, including the two the handler could
    // have read off the record it no longer holds.
    let forms = [
        ("its name", name.to_string()),
        ("its connector id", id.clone()),
        ("its unqualified ARN", arn.clone()),
        ("its version-qualified ARN", format!("{arn}:1")),
    ];
    for (case, identifier) in &forms {
        for operation in Addressed::reporting(identifier) {
            let answer = send(&router, operation, identifier).await;
            assert_not_found(
                &format!("a deleted connector addressed by {case} through {operation:?}"),
                identifier,
                &answer,
            );
            let message = text(&answer.body, "/Message");
            assert!(
                // A connector's ARN carries its Id in the resource segment, and the
                // reporting operation names that ARN for an id, so an identifier that
                // names the id itself is the only one whose refusal may carry it.
                identifier.contains(&id) || !message.contains(&id),
                "case {case:?} through {operation:?}: the refusal must not name the \
                 connector id {id:?} the caller never used, got {message:?}"
            );
        }
    }
}

/// RULE: a version-qualified identifier naming a version a LIVE connector does not hold is
/// the same refusal, keeping the version suffix on the resource it names.
#[tokio::test]
async fn a_version_the_connector_does_not_hold_is_refused_naming_the_qualified_identifier() {
    let router = emulator();
    let name = "versioned-connector";
    let created = create(&router, minimal_create(name)).await;
    assert_eq!(
        created.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        created.status, created.body
    );
    let arn = text(&created.body, "/Arn");
    wait_for_the_clock();

    // Version 1 is the only one a freshly created connector holds, so every higher version
    // is absent — through the ARN form and through the name form of the same qualifier.
    let absent_versions = [
        ("the next version, by ARN", format!("{arn}:2")),
        ("a far higher version, by ARN", format!("{arn}:4096")),
        ("the next version, by name", format!("{name}:2")),
    ];
    for (case, identifier) in &absent_versions {
        let answer = get(&router, identifier).await;
        assert_not_found(case, identifier, &answer);
    }

    // And the version it DOES hold is still served, so the refusal is about the version
    // asked for rather than about the qualified form.
    let held = get(&router, &format!("{arn}:1")).await;
    assert_eq!(
        held.status, 200,
        "case the version the connector holds: expected HTTP 200, got {} with {:?}",
        held.status, held.body
    );
    assert_eq!(
        held.body["Version"],
        Value::from(1),
        "case the version the connector holds: expected version 1, got {:?}",
        held.body["Version"]
    );
}
