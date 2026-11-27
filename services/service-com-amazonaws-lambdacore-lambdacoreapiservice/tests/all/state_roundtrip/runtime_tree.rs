// SPDX-License-Identifier: CC0-1.0

//! The customer-managed RUNTIME tree, end to end over HTTP.
//!
//! THE RULE pinned here: a runtime, its runtime versions and its runtime aliases are the
//! records this control plane keeps, and every operation computes its answer from them —
//! the create stores what the request asked for, each `Get…`/`List…` reports that record,
//! each `Delete…` "schedules" its resource into `PENDING_DELETION` for the
//! `RecoveryWindowInDays` the request named, `RestoreRuntime` "cancels a pending scheduled
//! deletion, returning the resource to the ACTIVE state", and a resource's state is what
//! decides which further calls it permits.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The harness is the sibling connector suite's
//! ([`crate::network_connector_lifecycle`]), so the two families of this service are driven
//! one way rather than through two copies of a request helper; the one thing it does not
//! carry is a request HEADER, which the recovery window of every `Delete…` is bound to.
//!
//! Each case varies what the rule generalises over — which of the three resources, which
//! identifier form, which release state, which recovery window, how many items a page
//! carries — rather than repeating one example.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::network_connector_lifecycle::{
    at, call, emulator, poll_until, signed_for, text, Answer, REGION,
};

/// The URI prefix every customer-managed runtime operation is bound under.
const PREFIX: &str = "/2026-10-01/runtimes";

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`), spelled out here
/// so a wrong account in the implementation fails.
const ACCOUNT: &str = "000000000000";

/// The runtime ARN the service must build for `name`: the resource part of a runtime's ARN
/// is `runtime/<RuntimeName>`, so `Arn` and `Name` of one answer always name the same
/// resource and a client can build either from the other.
fn runtime_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:runtime/{name}")
}

/// The ARN of version `version` of the runtime `name`, which `RuntimeArn` writes as the
/// runtime's own ARN followed by `/version/<name>`.
fn version_arn(name: &str, version: &str) -> String {
    format!("{}/version/{version}", runtime_arn(name))
}

/// The same for an alias.
fn alias_arn(name: &str, alias: &str) -> String {
    format!("{}/alias/{alias}", runtime_arn(name))
}

/// `value` as a client puts it in a non-greedy `@httpLabel`: the reserved characters an ARN
/// carries percent-encoded, which is what an SDK sends for a label of this shape.
fn label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// [`call`] carrying request headers — the recovery window every `Delete…` binds to
/// `x-amzn-runtime-recovery-window-in-days`, which the sibling harness has no connector
/// operation to carry.
async fn call_with(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
) -> Answer {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    for (name, value) in headers {
        builder = builder.header(*name, value.as_str());
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The sentence a refusal carries, whichever casing its modelled member uses: the parameter
/// and conflict classes of this model declare `message`, the not-found class `Message`.
fn refusal_message(answer: &Answer) -> String {
    let lower = text(&answer.body, "/message");
    if lower.is_empty() {
        text(&answer.body, "/Message")
    } else {
        lower
    }
}

/// Assert `answer` is the modelled refusal `code` with status `status`, carrying exactly
/// `message`.
fn assert_refusal(case: &str, answer: &Answer, status: u16, code: &str, message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    let actual = text(&answer.body, "/__type");
    assert!(
        actual.ends_with(code),
        "case {case:?}: expected {code}, got {actual:?} with {:?}",
        answer.body
    );
    assert_eq!(
        refusal_message(answer),
        message,
        "case {case:?}: expected the refusal to state {message:?}, got {:?}",
        answer.body
    );
}

/// `CreateRuntime` for `name`, with `extra` merged into the request body.
async fn create_runtime(router: &MultiServiceRouter, name: &str, extra: Value) -> Answer {
    let mut request = json!({ "Name": name });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        request[key] = value;
    }
    call(router, "POST", PREFIX, Some(request)).await
}

/// Create the runtime `name` and fail the test if it was not created.
async fn runtime(router: &MultiServiceRouter, name: &str) {
    let created = create_runtime(router, name, json!({})).await;
    assert_eq!(
        created.status, 202,
        "setup: CreateRuntime({name:?}) must succeed, got {} with {:?}",
        created.status, created.body
    );
}

/// `CreateRuntimeVersion` for `version` of `name`, referencing `image`.
async fn create_version(
    router: &MultiServiceRouter,
    name: &str,
    version: &str,
    image: &str,
    extra: Value,
) -> Answer {
    let mut request = json!({ "Name": version, "ImageUri": image });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        request[key] = value;
    }
    call(
        router,
        "POST",
        &format!("{PREFIX}/{name}/versions"),
        Some(request),
    )
    .await
}

/// `CreateRuntimeAlias` for `alias` of `name`, pointing at `version`.
async fn create_alias(
    router: &MultiServiceRouter,
    name: &str,
    alias: &str,
    version: &str,
    extra: Value,
) -> Answer {
    let mut request = json!({ "Name": alias, "RuntimeVersion": version });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        request[key] = value;
    }
    call(
        router,
        "POST",
        &format!("{PREFIX}/{name}/aliases"),
        Some(request),
    )
    .await
}

/// The runtime version `version` of `name` once its creation has finished: a create leaves
/// it `CREATING`, "being created", and the clock owes it `ACTIVE`. Waits for the CONDITION,
/// never for a span of the wall clock.
async fn active_version(router: &MultiServiceRouter, name: &str, version: &str) -> Answer {
    let uri = format!("{PREFIX}/{name}/versions/{version}");
    poll_until(
        &format!("the runtime version {version:?} of {name:?} reaches ACTIVE"),
        || call(router, "GET", &uri, None),
        |answer| answer.status == 200 && text(&answer.body, "/State") == "ACTIVE",
    )
    .await
}

/// The same for an alias, which a create leaves `CREATING` and an update that moves it
/// leaves `UPDATING`.
async fn active_alias(router: &MultiServiceRouter, name: &str, alias: &str) -> Answer {
    let uri = format!("{PREFIX}/{name}/aliases/{alias}");
    poll_until(
        &format!("the runtime alias {alias:?} of {name:?} reaches ACTIVE"),
        || call(router, "GET", &uri, None),
        |answer| answer.status == 200 && text(&answer.body, "/State") == "ACTIVE",
    )
    .await
}

/// The `Name`s a listing reported, in the order it reported them.
fn listed_names(what: &str, answer: &Answer) -> Vec<String> {
    answer.body["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("{what} must return an Items list: {:?}", answer.body))
        .iter()
        .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: a create stores what the request asked for and every read of the tree reports that
/// record, under the ARN the service builds for it.
///
/// The rows vary what the rule generalises over: a runtime carrying every optional member
/// and one carrying none, versions referencing two different forms of image reference, an
/// alias with and without a description — each read back through its own `Get…` and through
/// the listing that reports it.
#[tokio::test]
async fn every_answer_about_the_runtime_tree_is_read_off_the_stored_record() {
    let router = emulator();

    let rows = [
        (
            "every optional member given",
            "python3.14",
            json!({
                "Description": "the house runtime",
                "LicenseInfo": "Apache-2.0",
                "Tags": { "team": "core" },
                "ReleaseLifecycle": { "ReleaseState": "GA", "DeprecationDate": "2099-01-01" },
            }),
        ),
        ("no optional member at all", "bare.runtime-1", json!({})),
    ];

    for (case, name, extra) in &rows {
        let created = create_runtime(&router, name, extra.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateRuntime, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            text(&created.body, "/Arn"),
            runtime_arn(name),
            "case {case:?}: the Arn names the runtime of this account and Region, got {:?}",
            created.body
        );

        let read = call(&router, "GET", &format!("{PREFIX}/{name}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetRuntime, got {} with {:?}",
            read.status, read.body
        );
        for member in ["Arn", "Name", "Description", "LicenseInfo", "ReleaseLifecycle"] {
            assert_eq!(
                at(&read.body, &format!("/{member}")),
                at(&created.body, &format!("/{member}")),
                "case {case:?}: GetRuntime must report the {member} the create stored, got {:?}",
                read.body
            );
        }
        assert_eq!(
            text(&read.body, "/State"),
            "ACTIVE",
            "case {case:?}: a created runtime is active and available for use, got {:?}",
            read.body
        );
        assert_eq!(
            at(&read.body, "/PendingDeletionAt"),
            Value::Null,
            "case {case:?}: PendingDeletionAt is present only when State is PENDING_DELETION, \
             got {:?}",
            read.body
        );
        assert!(
            text(&read.body, "/Id").starts_with("rt-"),
            "case {case:?}: GetRuntime reports the Id the service assigned, got {:?}",
            read.body
        );
        assert_eq!(
            text(&read.body, "/Created"),
            text(&created.body, "/Created"),
            "case {case:?}: Created is the instant the create recorded, got {:?}",
            read.body
        );
    }

    let listed = call(&router, "GET", PREFIX, None).await;
    assert_eq!(
        listed_names("ListRuntimes", &listed),
        vec!["bare.runtime-1".to_string(), "python3.14".to_string()],
        "ListRuntimes reports every runtime of the account, got {:?}",
        listed.body
    );

    let versions = [
        (
            "a tag reference",
            "v1",
            "123456789012.dkr.ecr.eu-west-1.amazonaws.com/runtimes:v1",
        ),
        (
            "a digest reference",
            "v2_0",
            "123456789012.dkr.ecr.eu-west-1.amazonaws.com/runtimes@sha256:0123456789abcdef",
        ),
    ];
    for (case, version, image) in &versions {
        let created = create_version(&router, "python3.14", version, image, json!({})).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateRuntimeVersion, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            text(&created.body, "/Arn"),
            version_arn("python3.14", version),
            "case {case:?}: a version's Arn is its runtime's followed by /version/<Name>, got \
             {:?}",
            created.body
        );
        assert_eq!(
            text(&created.body, "/ResolvedUri"),
            *image,
            "case {case:?}: the version resolves to the image the create referenced, got {:?}",
            created.body
        );
        assert_eq!(
            text(&created.body, "/State"),
            "CREATING",
            "case {case:?}: a runtime version starts being created, got {:?}",
            created.body
        );

        let settled = active_version(&router, "python3.14", version).await;
        assert_eq!(
            text(&settled.body, "/ResolvedUri"),
            *image,
            "case {case:?}: GetRuntimeVersion reports the image the create stored, got {:?}",
            settled.body
        );
        assert_eq!(
            text(&settled.body, "/Created"),
            text(&created.body, "/Created"),
            "case {case:?}: Created is the instant the create recorded, got {:?}",
            settled.body
        );
    }

    let listed = call(&router, "GET", &format!("{PREFIX}/python3.14/versions"), None).await;
    assert_eq!(
        listed_names("ListRuntimeVersions", &listed),
        vec!["v1".to_string(), "v2_0".to_string()],
        "ListRuntimeVersions reports every version of the runtime, got {:?}",
        listed.body
    );

    let aliases = [
        (
            "with a description",
            "current",
            "v1",
            json!({ "Description": "what functions use" }),
        ),
        ("with none", "next", "v2_0", json!({})),
    ];
    for (case, alias, version, extra) in &aliases {
        let created = create_alias(&router, "python3.14", alias, version, extra.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateRuntimeAlias, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            text(&created.body, "/Arn"),
            alias_arn("python3.14", alias),
            "case {case:?}: an alias's Arn is its runtime's followed by /alias/<Name>, got {:?}",
            created.body
        );
        assert_eq!(
            text(&created.body, "/RuntimeVersionArn"),
            version_arn("python3.14", version),
            "case {case:?}: the alias names the ARN of the version it points at, got {:?}",
            created.body
        );
        assert_eq!(
            text(&created.body, "/State"),
            "CREATING",
            "case {case:?}: a runtime alias starts being created, got {:?}",
            created.body
        );

        let settled = active_alias(&router, "python3.14", alias).await;
        assert_eq!(
            at(&settled.body, "/Description"),
            at(&created.body, "/Description"),
            "case {case:?}: GetRuntimeAlias reports the description the create stored, got {:?}",
            settled.body
        );
        assert_eq!(
            text(&settled.body, "/RuntimeVersion"),
            *version,
            "case {case:?}: the alias still points at the version it was created for, got {:?}",
            settled.body
        );
    }

    let listed = call(&router, "GET", &format!("{PREFIX}/python3.14/aliases"), None).await;
    assert_eq!(
        listed_names("ListRuntimeAliases", &listed),
        vec!["current".to_string(), "next".to_string()],
        "ListRuntimeAliases reports every alias of the runtime, got {:?}",
        listed.body
    );

    // One runtime's children are its own: the second runtime, which nothing was created
    // under, reports none.
    let empty = call(
        &router,
        "GET",
        &format!("{PREFIX}/bare.runtime-1/versions"),
        None,
    )
    .await;
    assert_eq!(
        at(&empty.body, "/Items"),
        json!([]),
        "a runtime nothing was created under reports no versions, got {:?}",
        empty.body
    );
}

/// RULE: a `RuntimeIdentifier` is "either the runtime name or the fully qualified runtime
/// ARN", and both address the same runtime; an ARN of another account addresses a resource
/// the caller may not reach.
#[tokio::test]
async fn a_runtime_is_addressed_by_its_name_or_by_its_arn() {
    let router = emulator();
    runtime(&router, "addressable").await;

    for (case, identifier) in [
        ("the bare name", "addressable".to_string()),
        ("this account's ARN", label(&runtime_arn("addressable"))),
    ] {
        let read = call(&router, "GET", &format!("{PREFIX}/{identifier}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetRuntime, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            text(&read.body, "/Arn"),
            runtime_arn("addressable"),
            "case {case:?}: both identifier forms address the one runtime, got {:?}",
            read.body
        );
    }

    let theirs = label(&format!(
        "arn:aws:lambda:{REGION}:111122223333:runtime/addressable"
    ));
    let refused = call(&router, "GET", &format!("{PREFIX}/{theirs}"), None).await;
    assert_eq!(
        refused.status, 403,
        "an ARN of another account is an authorization refusal, got {} with {:?}",
        refused.status, refused.body
    );
}

/// RULE: a resource the request names and this account does not hold is reported absent, and
/// the refusal names the resource the request addressed by the ARN that names it — the same
/// way whichever operation addressed it.
#[tokio::test]
async fn a_resource_this_account_does_not_hold_is_named_by_its_own_arn() {
    let router = emulator();
    runtime(&router, "held").await;
    assert_eq!(
        create_version(&router, "held", "v1", "registry/image:v1", json!({}))
            .await
            .status,
        201,
        "setup: the version must be created"
    );

    let rows = [
        (
            "a read of a runtime this account does not hold",
            "GET",
            format!("{PREFIX}/absent.runtime"),
            format!("Runtime not found: {}", runtime_arn("absent.runtime")),
        ),
        (
            "an update of a runtime this account does not hold",
            "PUT",
            format!("{PREFIX}/absent.runtime"),
            format!("Runtime not found: {}", runtime_arn("absent.runtime")),
        ),
        (
            "a delete of a runtime this account does not hold",
            "DELETE",
            format!("{PREFIX}/absent.runtime"),
            format!("Runtime not found: {}", runtime_arn("absent.runtime")),
        ),
        (
            "a read of a version its runtime does not hold",
            "GET",
            format!("{PREFIX}/held/versions/absent"),
            format!("Runtime version not found: {}", version_arn("held", "absent")),
        ),
        (
            "a delete of a version its runtime does not hold",
            "DELETE",
            format!("{PREFIX}/held/versions/absent"),
            format!("Runtime version not found: {}", version_arn("held", "absent")),
        ),
        (
            "a read of an alias its runtime does not hold",
            "GET",
            format!("{PREFIX}/held/aliases/absent"),
            format!("Runtime alias not found: {}", alias_arn("held", "absent")),
        ),
        (
            "a child of a runtime that is not there either",
            "GET",
            format!("{PREFIX}/absent.runtime/versions/v1"),
            format!("Runtime not found: {}", runtime_arn("absent.runtime")),
        ),
        (
            "a listing over a runtime that is not there",
            "GET",
            format!("{PREFIX}/absent.runtime/aliases"),
            format!("Runtime not found: {}", runtime_arn("absent.runtime")),
        ),
    ];
    for (case, method, uri, message) in &rows {
        let body = (*method == "PUT").then(|| json!({ "Description": "nothing to update" }));
        let refused = call(&router, method, uri, body).await;
        assert_refusal(case, &refused, 404, "ResourceNotFoundException", message);
    }

    // An alias can only point at a version its runtime holds.
    let refused = create_alias(&router, "held", "dangling", "absent", json!({})).await;
    assert_refusal(
        "an alias pointing at a version that is not there",
        &refused,
        404,
        "ResourceNotFoundException",
        &format!("Runtime version not found: {}", version_arn("held", "absent")),
    );
}

/// RULE: each `Delete…` SCHEDULES its resource — it enters `PENDING_DELETION`, reports the
/// instant the deletion will execute, and `RestoreRuntime` returns it to `ACTIVE` until
/// then; restoring a resource that is already `ACTIVE` is a no-op success, and repeating a
/// delete succeeds without moving the schedule the first one set.
///
/// The rows are the three resources of the tree, each with its own recovery window: the
/// shortest the parameter admits, the longest, and the default when the request names none.
#[tokio::test]
async fn a_scheduled_deletion_is_restorable_until_it_executes() {
    let router = emulator();
    runtime(&router, "scheduled").await;
    assert_eq!(
        create_version(&router, "scheduled", "v1", "registry/image:v1", json!({}))
            .await
            .status,
        201,
        "setup: the version must be created"
    );
    active_version(&router, "scheduled", "v1").await;
    assert_eq!(
        create_alias(&router, "scheduled", "live", "v1", json!({}))
            .await
            .status,
        201,
        "setup: the alias must be created"
    );
    active_alias(&router, "scheduled", "live").await;
    // The runtime row needs a runtime of its own: a runtime that still parents a live child
    // cannot be scheduled at all, which is the rule the state-gate case below pins.
    runtime(&router, "scheduled.alone").await;

    let rows = [
        (
            "the alias",
            "scheduled",
            format!("{PREFIX}/scheduled/aliases/live"),
            Some("7"),
            "?RuntimeAlias=live",
        ),
        (
            "the version",
            "scheduled",
            format!("{PREFIX}/scheduled/versions/v1"),
            Some("30"),
            "?RuntimeVersion=v1",
        ),
        (
            "the runtime itself",
            "scheduled.alone",
            format!("{PREFIX}/scheduled.alone"),
            None,
            "",
        ),
    ];

    for (case, owner, uri, window, scope) in &rows {
        let headers: Vec<(&str, String)> = window
            .map(|days| vec![("x-amzn-runtime-recovery-window-in-days", days.to_string())])
            .unwrap_or_default();
        let scheduled = call_with(&router, "DELETE", uri, &headers).await;
        assert_eq!(
            scheduled.status, 202,
            "case {case:?}: expected HTTP 202 from the delete, got {} with {:?}",
            scheduled.status, scheduled.body
        );

        let read = call(&router, "GET", uri, None).await;
        assert_eq!(
            text(&read.body, "/State"),
            "PENDING_DELETION",
            "case {case:?}: a scheduled resource is PENDING_DELETION, got {:?}",
            read.body
        );
        let pending_at = text(&read.body, "/PendingDeletionAt");
        assert!(
            !pending_at.is_empty(),
            "case {case:?}: a scheduled resource reports when it will be deleted, got {:?}",
            read.body
        );

        // The schedule is the first delete's: repeating it succeeds and moves nothing.
        let repeated = call_with(&router, "DELETE", uri, &headers).await;
        assert_eq!(
            repeated.status, 202,
            "case {case:?}: a repeated delete succeeds, got {} with {:?}",
            repeated.status, repeated.body
        );
        let reread = call(&router, "GET", uri, None).await;
        assert_eq!(
            text(&reread.body, "/PendingDeletionAt"),
            pending_at,
            "case {case:?}: a repeated delete leaves the schedule the first one set, got {:?}",
            reread.body
        );

        let restore = format!("{PREFIX}/{owner}/restore{scope}");
        let restored = call(&router, "POST", &restore, None).await;
        assert_eq!(
            restored.status, 200,
            "case {case:?}: expected HTTP 200 from RestoreRuntime, got {} with {:?}",
            restored.status, restored.body
        );
        assert_eq!(
            text(&restored.body, "/State"),
            "ACTIVE",
            "case {case:?}: a restore returns the resource to the ACTIVE state, got {:?}",
            restored.body
        );

        let read = call(&router, "GET", uri, None).await;
        assert_eq!(
            text(&read.body, "/State"),
            "ACTIVE",
            "case {case:?}: the restored resource stays active, got {:?}",
            read.body
        );
        assert_eq!(
            at(&read.body, "/PendingDeletionAt"),
            Value::Null,
            "case {case:?}: a restored resource reports no scheduled deletion, got {:?}",
            read.body
        );

        let again = call(&router, "POST", &restore, None).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: restoring a resource that is already ACTIVE is a no-op success, \
             got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            text(&again.body, "/LastModified"),
            text(&restored.body, "/LastModified"),
            "case {case:?}: a no-op restore changes nothing, not even LastModified, got {:?}",
            again.body
        );
    }

    // The two scope parameters are mutually exclusive.
    let ambiguous = call(
        &router,
        "POST",
        &format!("{PREFIX}/scheduled/restore?RuntimeVersion=v1&RuntimeAlias=live"),
        None,
    )
    .await;
    assert_refusal(
        "both scope parameters",
        &ambiguous,
        400,
        "InvalidParameterValueException",
        "RuntimeVersion and RuntimeAlias are mutually exclusive: specify at most one",
    );
}

/// RULE: a resource's state decides which calls it permits — "runtime versions and aliases
/// must be deleted or scheduled for deletion before deleting the runtime", a runtime version
/// "must be in the ACTIVE state" to be updated, a resource scheduled for deletion cannot be
/// modified or given new children, and a restore "only works while the target is
/// PENDING_DELETION".
#[tokio::test]
async fn a_state_that_does_not_permit_the_call_refuses_it() {
    let router = emulator();
    runtime(&router, "gated").await;
    assert_eq!(
        create_version(&router, "gated", "v1", "registry/image:v1", json!({}))
            .await
            .status,
        201,
        "setup: the version must be created"
    );

    // A version still being created is not in the ACTIVE state the update requires, and is
    // in no state to be restored either.
    let too_early = call(
        &router,
        "PATCH",
        &format!("{PREFIX}/gated/versions/v1"),
        Some(json!({ "ReleaseNotes": "too early" })),
    )
    .await;
    assert_refusal(
        "an update of a version still being created",
        &too_early,
        409,
        "ResourceConflictException",
        "Runtime version v1 is in state Creating and cannot be updated. Only ACTIVE runtime \
         versions can be updated.",
    );
    let not_scheduled = call(
        &router,
        "POST",
        &format!("{PREFIX}/gated/restore?RuntimeVersion=v1"),
        None,
    )
    .await;
    assert_refusal(
        "a restore of a version that is not scheduled for deletion",
        &not_scheduled,
        409,
        "ResourceConflictException",
        "Runtime version v1 is in state Creating and cannot be restored. Only PENDING_DELETION \
         runtime versions can be restored.",
    );

    active_version(&router, "gated", "v1").await;

    // The runtime still parents a live version.
    let blocked = call(&router, "DELETE", &format!("{PREFIX}/gated"), None).await;
    assert_refusal(
        "a runtime whose version is still live",
        &blocked,
        409,
        "ResourceConflictException",
        "Runtime gated cannot be deleted. Runtime versions and aliases must be deleted or \
         scheduled for deletion before deleting the runtime.",
    );

    // Once the version is scheduled, the runtime's own delete is accepted.
    assert_eq!(
        call(&router, "DELETE", &format!("{PREFIX}/gated/versions/v1"), None)
            .await
            .status,
        202,
        "setup: the version must be scheduled for deletion"
    );
    let accepted = call(&router, "DELETE", &format!("{PREFIX}/gated"), None).await;
    assert_eq!(
        accepted.status, 202,
        "a runtime whose children are all released is scheduled, got {} with {:?}",
        accepted.status, accepted.body
    );

    // A released runtime permits neither an update nor a new child.
    let refused = call(
        &router,
        "PUT",
        &format!("{PREFIX}/gated"),
        Some(json!({ "Description": "no" })),
    )
    .await;
    assert_refusal(
        "an update of a runtime scheduled for deletion",
        &refused,
        409,
        "ResourceConflictException",
        "Runtime gated is in state PendingDeletion and cannot be updated. Only ACTIVE runtimes \
         can be updated.",
    );
    let refused = create_version(&router, "gated", "v2", "registry/image:v2", json!({})).await;
    assert_refusal(
        "a new version under a runtime scheduled for deletion",
        &refused,
        409,
        "ResourceConflictException",
        "Runtime gated is in state PendingDeletion and cannot be given new runtime versions. \
         Only ACTIVE runtimes can be given new runtime versions.",
    );
    let refused = create_alias(&router, "gated", "any", "v1", json!({})).await;
    assert_refusal(
        "a new alias under a runtime scheduled for deletion",
        &refused,
        409,
        "ResourceConflictException",
        "Runtime gated is in state PendingDeletion and cannot be given new runtime aliases. \
         Only ACTIVE runtimes can be given new runtime aliases.",
    );
}

/// RULE: `RuntimeReleaseLifecycle$ReleaseState` — "`DEPRECATED` requires a `DeprecationDate`
/// on or before today (UTC); `PREVIEW` and `GA` require a later one, or none. Reads report
/// `DEPRECATED` once `DeprecationDate` is reached."
///
/// The rows span every release state the model declares
/// (`smithy_query(query="shape",shape="RuntimeReleaseState")` -> `PREVIEW | GA |
/// DEPRECATED`), against a date already past and a later one; the read half of the rule is
/// asserted on both sides of that boundary.
#[tokio::test]
async fn a_release_lifecycle_is_refused_against_the_deprecation_date_it_needs() {
    let router = emulator();

    let rows = [
        ("deprecated with a past date", "DEPRECATED", "2020-01-01", true),
        ("deprecated with a later date", "DEPRECATED", "2099-01-01", false),
        ("generally available, a later date", "GA", "2099-01-01", true),
        ("generally available, a past date", "GA", "2020-01-01", false),
        ("in preview, a later date", "PREVIEW", "2099-01-01", true),
        ("in preview, a past date", "PREVIEW", "2020-01-01", false),
    ];
    for (index, (case, release_state, date, accepted)) in rows.iter().enumerate() {
        let name = format!("lifecycle-{index}");
        let created = create_runtime(
            &router,
            &name,
            json!({
                "ReleaseLifecycle": { "ReleaseState": release_state, "DeprecationDate": date }
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                created.status, 202,
                "case {case:?}: expected the create to be accepted, got {} with {:?}",
                created.status, created.body
            );
        } else {
            let requirement = if *release_state == "DEPRECATED" {
                "ReleaseLifecycle release state DEPRECATED requires a DeprecationDate on or \
                 before today"
                    .to_string()
            } else {
                format!(
                    "ReleaseLifecycle release state {release_state} requires a DeprecationDate \
                     later than today"
                )
            };
            assert_refusal(case, &created, 400, "InvalidParameterValueException", &requirement);
        }
    }

    // The read half, on both sides of the boundary.
    let read = call(&router, "GET", &format!("{PREFIX}/lifecycle-0"), None).await;
    assert_eq!(
        text(&read.body, "/ReleaseLifecycle/ReleaseState"),
        "DEPRECATED",
        "a lifecycle whose DeprecationDate is reached reports DEPRECATED, got {:?}",
        read.body
    );
    let read = call(&router, "GET", &format!("{PREFIX}/lifecycle-2"), None).await;
    assert_eq!(
        text(&read.body, "/ReleaseLifecycle/ReleaseState"),
        "GA",
        "a lifecycle whose DeprecationDate is not reached reports the state it stores, got {:?}",
        read.body
    );

    // An update replaces the members in force, and "omitting or providing an empty object
    // removes the existing lifecycle".
    let updated = call(
        &router,
        "PUT",
        &format!("{PREFIX}/lifecycle-2"),
        Some(json!({ "LicenseInfo": "MIT" })),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "an update with no lifecycle must be served, got {} with {:?}",
        updated.status, updated.body
    );
    let read = call(&router, "GET", &format!("{PREFIX}/lifecycle-2"), None).await;
    assert_eq!(
        at(&read.body, "/ReleaseLifecycle"),
        Value::Null,
        "omitting the lifecycle removes the existing one, got {:?}",
        read.body
    );
    assert_eq!(
        text(&read.body, "/LicenseInfo"),
        "MIT",
        "the update stores what it carried, got {:?}",
        read.body
    );
    assert_eq!(
        at(&read.body, "/Description"),
        Value::Null,
        "a member the update omitted is cleared, got {:?}",
        read.body
    );
}

/// RULE: a runtime version is immutable but for its release notes, and an alias is "a named
/// pointer to a runtime version" that an update moves to another version of the same
/// runtime.
#[tokio::test]
async fn a_version_changes_only_its_notes_and_an_alias_moves_between_versions() {
    let router = emulator();
    runtime(&router, "mutable").await;
    for version in ["v1", "v2"] {
        assert_eq!(
            create_version(
                &router,
                "mutable",
                version,
                &format!("registry/image:{version}"),
                json!({ "ReleaseNotes": "first cut" }),
            )
            .await
            .status,
            201,
            "setup: the version {version} must be created"
        );
    }
    active_version(&router, "mutable", "v1").await;
    active_version(&router, "mutable", "v2").await;

    let updated = call(
        &router,
        "PATCH",
        &format!("{PREFIX}/mutable/versions/v1"),
        Some(json!({ "ReleaseNotes": "second cut" })),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "UpdateRuntimeVersion must be served, got {} with {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        text(&updated.body, "/ReleaseNotes"),
        "second cut",
        "the update stores the notes it carried, got {:?}",
        updated.body
    );
    assert_eq!(
        text(&updated.body, "/ResolvedUri"),
        "registry/image:v1",
        "a runtime version is immutable: the image it references is untouched, got {:?}",
        updated.body
    );
    let read = call(&router, "GET", &format!("{PREFIX}/mutable/versions/v1"), None).await;
    assert_eq!(
        text(&read.body, "/ReleaseNotes"),
        "second cut",
        "the read reports the notes the update stored, got {:?}",
        read.body
    );

    assert_eq!(
        create_alias(&router, "mutable", "current", "v1", json!({}))
            .await
            .status,
        201,
        "setup: the alias must be created"
    );
    active_alias(&router, "mutable", "current").await;

    let moved = call(
        &router,
        "PUT",
        &format!("{PREFIX}/mutable/aliases/current"),
        Some(json!({ "RuntimeVersion": "v2", "Description": "migrated" })),
    )
    .await;
    assert_eq!(
        moved.status, 200,
        "UpdateRuntimeAlias must be served, got {} with {:?}",
        moved.status, moved.body
    );
    assert_eq!(
        text(&moved.body, "/State"),
        "UPDATING",
        "an alias being moved to a new version is UPDATING, got {:?}",
        moved.body
    );
    assert_eq!(
        text(&moved.body, "/RuntimeVersionArn"),
        version_arn("mutable", "v2"),
        "the alias now names the ARN of the version it was moved to, got {:?}",
        moved.body
    );
    assert_eq!(
        text(&moved.body, "/RuntimeName"),
        "mutable",
        "the answer names the runtime the alias belongs to, got {:?}",
        moved.body
    );

    let settled = active_alias(&router, "mutable", "current").await;
    assert_eq!(
        text(&settled.body, "/RuntimeVersion"),
        "v2",
        "the moved alias points at its new version once the move settles, got {:?}",
        settled.body
    );
    assert_eq!(
        text(&settled.body, "/Description"),
        "migrated",
        "the update stores the description it carried, got {:?}",
        settled.body
    );

    // An alias can only be moved to a version its own runtime holds.
    let refused = call(
        &router,
        "PUT",
        &format!("{PREFIX}/mutable/aliases/current"),
        Some(json!({ "RuntimeVersion": "v9" })),
    )
    .await;
    assert_refusal(
        "an alias moved to a version that is not there",
        &refused,
        404,
        "ResourceNotFoundException",
        &format!("Runtime version not found: {}", version_arn("mutable", "v9")),
    );
}

/// RULE: the three creates are `@idempotent` — a repeat that asks for the resource the name
/// already holds is a retry and answers with it, and a repeat that asks for something else
/// under that name conflicts. `CreateRuntimeVersion`'s `DryRun` "validat[es] without
/// creating a new runtime version. ... No runtime version is created and no changes are
/// made."
#[tokio::test]
async fn a_repeated_create_is_a_retry_and_a_dry_run_creates_nothing() {
    let router = emulator();

    let first = create_runtime(&router, "idem", json!({ "Description": "the one" })).await;
    assert_eq!(first.status, 202, "setup: the runtime must be created");
    let retry = create_runtime(&router, "idem", json!({ "Description": "the one" })).await;
    assert_eq!(
        retry.status, 202,
        "a retry of a create that completed succeeds, got {} with {:?}",
        retry.status, retry.body
    );
    assert_eq!(
        text(&retry.body, "/Created"),
        text(&first.body, "/Created"),
        "a retry answers with the runtime the first call created, got {:?}",
        retry.body
    );
    let conflict = create_runtime(&router, "idem", json!({ "Description": "another" })).await;
    assert_refusal(
        "a different runtime under a name already held",
        &conflict,
        409,
        "ResourceConflictException",
        "A runtime with name 'idem' already exists in this account",
    );

    let first = create_version(&router, "idem", "v1", "registry/image:v1", json!({})).await;
    assert_eq!(first.status, 201, "setup: the version must be created");
    let retry = create_version(&router, "idem", "v1", "registry/image:v1", json!({})).await;
    assert_eq!(
        text(&retry.body, "/Created"),
        text(&first.body, "/Created"),
        "a retry answers with the version the first call created, got {:?}",
        retry.body
    );
    let conflict = create_version(&router, "idem", "v1", "registry/image:other", json!({})).await;
    assert_refusal(
        "a different image under a version name already held",
        &conflict,
        409,
        "ResourceConflictException",
        "A runtime version with name 'v1' already exists for runtime 'idem'",
    );

    active_version(&router, "idem", "v1").await;
    let first = create_alias(&router, "idem", "current", "v1", json!({})).await;
    assert_eq!(first.status, 201, "setup: the alias must be created");
    let retry = create_alias(&router, "idem", "current", "v1", json!({})).await;
    assert_eq!(
        text(&retry.body, "/Created"),
        text(&first.body, "/Created"),
        "a retry answers with the alias the first call created, got {:?}",
        retry.body
    );
    let conflict = create_alias(
        &router,
        "idem",
        "current",
        "v1",
        json!({ "Description": "something else" }),
    )
    .await;
    assert_refusal(
        "a different alias under a name already held",
        &conflict,
        409,
        "ResourceConflictException",
        "A runtime alias with name 'current' already exists for runtime 'idem'",
    );

    // A dry run validates and writes nothing: the version it describes is not there
    // afterwards, and a real create of the same name then succeeds.
    let dry = create_version(
        &router,
        "idem",
        "v2",
        "registry/image:v2",
        json!({ "DryRun": true }),
    )
    .await;
    assert_eq!(
        dry.status, 201,
        "a dry run is served, got {} with {:?}",
        dry.status, dry.body
    );
    let absent = call(&router, "GET", &format!("{PREFIX}/idem/versions/v2"), None).await;
    assert_refusal(
        "the version a dry run described",
        &absent,
        404,
        "ResourceNotFoundException",
        &format!("Runtime version not found: {}", version_arn("idem", "v2")),
    );
    let listed = call(&router, "GET", &format!("{PREFIX}/idem/versions"), None).await;
    assert_eq!(
        listed_names("ListRuntimeVersions", &listed),
        vec!["v1".to_string()],
        "a dry run leaves the runtime's versions as they were, got {:?}",
        listed.body
    );
    let real = create_version(&router, "idem", "v2", "registry/image:v2", json!({})).await;
    assert_eq!(
        real.status, 201,
        "the name a dry run described is still free, got {} with {:?}",
        real.status, real.body
    );

    // A dry run is still validated against the live tree.
    let refused = create_version(
        &router,
        "absent.runtime",
        "v1",
        "registry/image:v1",
        json!({ "DryRun": true }),
    )
    .await;
    assert_refusal(
        "a dry run under a runtime this account does not hold",
        &refused,
        404,
        "ResourceNotFoundException",
        &format!("Runtime not found: {}", runtime_arn("absent.runtime")),
    );
}

/// RULE: each listing is `@paginated` over one stable total order, serving at most the
/// `MaxResults` the request asked for and a `NextToken` only while items are left.
///
/// The rows are two page sizes over a set created out of order, so what is asserted is the
/// service's order and not the order of creation.
#[tokio::test]
async fn a_listing_pages_over_one_stable_order() {
    let router = emulator();
    for name in ["cherry", "apple", "banana"] {
        runtime(&router, name).await;
    }

    for page_size in [1_usize, 2] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..8 {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let page = call(&router, "GET", &format!("{PREFIX}{query}"), None).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: ListRuntimes must be served, got {} with {:?}",
                page.status, page.body
            );
            let names = listed_names("ListRuntimes", &page);
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: a page carries at most what the request asked \
                 for, got {:?}",
                page.body
            );
            seen.extend(names);
            token = at(&page.body, "/NextToken").as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            vec![
                "apple".to_string(),
                "banana".to_string(),
                "cherry".to_string()
            ],
            "case page size {page_size}: paging to exhaustion reports every runtime once, in \
             one order"
        );
    }
}
