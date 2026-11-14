// SPDX-License-Identifier: CC0-1.0

//! What a call that would CHANGE a parameter in the hierarchy AWS reserves for its own
//! published parameters is answered with, end to end over the front door.
//!
//! RULE under test: **the `/aws` hierarchy holds AWS's own published parameters and no
//! parameter of this account, so a call that would change a name in it is refused as
//! DENIED — HTTP 400 `AccessDeniedException` with the fixed sentence "Cannot
//! access/update data for the restricted parameter." — and never as the name merely being
//! absent.** The two refusals say different things to a client: "absent" invites a retry
//! or a recreate under the same name, which can never succeed here, while "denied" tells
//! the caller to stop.
//!
//! A READ of that hierarchy is not a change, so it is not denied for being one: a name AWS
//! publishes resolves against the published catalog (`all/public_parameter_catalog.rs`) and
//! a name it publishes nothing under is still the modelled miss
//! (`all/parameter_not_found_payload.rs`). A HIERARCHY read of it does carry its own
//! restriction, checked here too: the reserved hierarchy is the namespaces AWS publishes in
//! (plus the reserved reference namespace this service serves another service's secrets
//! through), so a path in it that is no namespace of it is denied — "No access to \"/aws/\"
//! namespace: &lt;path&gt; is not a valid namespace" — rather than answered with an empty
//! page a caller would read as a namespace that exists and holds nothing.
//!
//! The table's rows differ along what the rule generalises over: the OPERATION that would
//! make the change (the delete, and both label calls), the NAME form inside the reserved
//! hierarchy (its root, a namespace level, a leaf AWS really publishes, a leaf AWS
//! publishes nothing under, the reserved reference namespace of another service, a name
//! addressed without its leading separator) and its depth. Each operation also makes the
//! same change to a parameter of the ACCOUNT's own in the same pass, so no row passes
//! against a handler that denies everything.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, error_message};
use crate::public_parameter_catalog::{emulator, AMAZON_LINUX_NAMESPACE};

/// The sentence the denial carries. Wholly fixed: it names neither the parameter nor the
/// operation, because what it says is that the hierarchy is not the account's to change.
const RESTRICTED: &str = "Cannot access/update data for the restricted parameter.";

/// The modelled class the denial is reported as
/// (`smithy_query(query="shape", shape="AccessDeniedException")`: "The requester doesn't
/// have permissions to perform the requested operation", httpError 400).
const DENIED: &str = "AccessDeniedException";

/// One change a caller can ask for, as the request body that asks for it.
fn change(operation: &str, name: &str) -> Value {
    match operation {
        "LabelParameterVersion" => json!({ "Name": name, "Labels": ["release"] }),
        "UnlabelParameterVersion" => {
            json!({ "Name": name, "ParameterVersion": 1, "Labels": ["release"] })
        }
        _ => json!({ "Name": name }),
    }
}

/// Every call that would change a parameter addressed by name and that this slice serves.
/// Source: each operation's modelled input (`smithy_query(query="operation",
/// operation="DeleteParameter,LabelParameterVersion,UnlabelParameterVersion")`), all three
/// of which take the parameter's `Name`.
const CHANGES: &[&str] = &[
    "DeleteParameter",
    "LabelParameterVersion",
    "UnlabelParameterVersion",
];

/// A name in the reserved hierarchy, whatever its form, is denied to every call that would
/// change it — and the same call changes the account's own parameter.
#[tokio::test]
async fn a_change_to_the_reserved_hierarchy_is_denied() {
    let router = emulator();
    let published = a_published_name(&router).await;

    // (case, the name the change addresses)
    let mut rows: Vec<(String, String)> = vec![
        (
            "the reserved root itself".to_string(),
            "/aws".to_string(),
        ),
        (
            "a namespace level of the reserved hierarchy".to_string(),
            AMAZON_LINUX_NAMESPACE.to_string(),
        ),
        (
            "a leaf AWS publishes nothing under".to_string(),
            format!("{AMAZON_LINUX_NAMESPACE}/no-such-published-alias"),
        ),
        (
            "the reserved reference namespace of another service".to_string(),
            "/aws/reference/secretsmanager/probe-secret".to_string(),
        ),
        (
            "a reserved name addressed without its leading separator".to_string(),
            "aws/service/pcs/sample".to_string(),
        ),
        (
            "a deep name under the reserved root".to_string(),
            "/aws/a/b/c/d/e/f".to_string(),
        ),
    ];
    // A leaf AWS really publishes, discovered through the catalog read itself rather than
    // written here, so the table covers a reserved name that DOES resolve for a reader.
    rows.push(("a leaf AWS really publishes".to_string(), published.clone()));

    for (case, name) in &rows {
        for operation in CHANGES {
            let (status, body) = call(&router, operation, &change(operation, name)).await;
            assert_eq!(
                (status, error_code(&body), error_message(&body)),
                (400, DENIED.to_string(), RESTRICTED.to_string()),
                "case {case:?} ({name}): expected {operation} to be denied, got {status} {body}"
            );
            assert!(
                !error_message(&body).contains(name.trim_start_matches('/')),
                "case {case:?}: the denial names no parameter, got {}",
                error_message(&body)
            );
        }
    }

    // The account's own parameter takes every one of those changes, so the denial is about
    // the hierarchy and not about the operations.
    let own = "/own/changeable";
    put(&router, own, "value").await;
    let (status, labelled) = call(
        &router,
        "LabelParameterVersion",
        &change("LabelParameterVersion", own),
    )
    .await;
    assert_eq!(
        (status, labelled["ParameterVersion"].as_i64()),
        (200, Some(1)),
        "expected the account's own parameter to be labelled, got {status} {labelled}"
    );
    let (status, unlabelled) = call(
        &router,
        "UnlabelParameterVersion",
        &change("UnlabelParameterVersion", own),
    )
    .await;
    assert_eq!(
        (
            status,
            unlabelled["RemovedLabels"].as_array().map(Vec::len)
        ),
        (200, Some(1)),
        "expected the label to be removed from the account's own parameter, got {status} \
         {unlabelled}"
    );
    let (status, deleted) = call(&router, "DeleteParameter", &change("DeleteParameter", own)).await;
    assert_eq!(
        status, 200,
        "expected the account's own parameter to be deleted, got {status} {deleted}"
    );
}

/// The denial is about CHANGING the hierarchy, so a read of it is unchanged: a name AWS
/// publishes still resolves, and a name it publishes nothing under is still the miss.
#[tokio::test]
async fn a_read_of_the_reserved_hierarchy_is_not_denied() {
    let router = emulator();
    let published = a_published_name(&router).await;

    let (status, read) = call(&router, "GetParameter", &json!({ "Name": published })).await;
    assert_eq!(
        (status, read.pointer("/Parameter/Name").and_then(Value::as_str)),
        (200, Some(published.as_str())),
        "expected the published parameter to read back, got {status} {read}"
    );

    // (case, the Name read, the code it answers with)
    let misses = [
        (
            "a leaf AWS publishes nothing under",
            format!("{AMAZON_LINUX_NAMESPACE}/no-such-published-alias"),
        ),
        ("a namespace AWS publishes nothing in", "/aws/no/such/space".to_string()),
    ];
    for (case, name) in misses {
        for operation in ["GetParameter", "GetParameterHistory"] {
            let (status, body) = call(&router, operation, &json!({ "Name": name })).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "ParameterNotFound".to_string()),
                "case {case:?}: expected {operation} to report a miss rather than a denial, \
                 got {status} {body}"
            );
        }
    }
}

/// A hierarchy read inside the reserved hierarchy reaches the NAMESPACES of it this
/// endpoint serves names in; a path that is no namespace of it is denied, naming the path
/// and the hierarchy, rather than answered with an empty page a caller would read as "this
/// namespace exists and is empty".
#[tokio::test]
async fn a_hierarchy_read_outside_the_reserved_namespaces_is_denied() {
    let router = emulator();
    let published = a_published_name(&router).await;

    // (case, the path read)
    let denied: Vec<(&str, String)> = vec![
        (
            "a level of the reserved hierarchy AWS has none",
            "/aws/no-such-level".to_string(),
        ),
        (
            "a service namespace AWS publishes nothing in",
            "/aws/service/no-such-published-namespace".to_string(),
        ),
        (
            "a leaf nothing is published at or under",
            format!("{AMAZON_LINUX_NAMESPACE}/no-such-published-alias"),
        ),
        (
            "a published name read as if it were a hierarchy below itself",
            format!("{published}/deeper"),
        ),
        (
            "a reference namespace of no service",
            "/aws/reference/no-such-source".to_string(),
        ),
    ];
    for (case, path) in &denied {
        let (status, refused) = call(
            &router,
            "GetParametersByPath",
            &json!({ "Path": path, "Recursive": true }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused), error_message(&refused)),
            (
                400,
                DENIED.to_string(),
                format!(
                    "No access to \"/aws/\" namespace: {} is not a valid namespace",
                    path.trim_start_matches('/')
                )
            ),
            "case {case:?} ({path}): expected the namespace refusal, got {status} {refused}"
        );
    }

    // Every namespace this endpoint DOES serve names at or under is read, so the refusals
    // above are about the paths and not about the hierarchy.
    for (case, path) in [
        ("the reserved root", "/aws".to_string()),
        ("the service level", "/aws/service".to_string()),
        (
            "a namespace AWS publishes in",
            AMAZON_LINUX_NAMESPACE.to_string(),
        ),
        // The reserved hierarchy through which this service serves another's secrets: a
        // namespace it serves names in, though it publishes no parameter there.
        (
            "the reserved reference namespace",
            "/aws/reference/secretsmanager".to_string(),
        ),
        ("the level above it", "/aws/reference".to_string()),
        ("a published name itself", published.clone()),
    ] {
        let (status, served) = call(
            &router,
            "GetParametersByPath",
            &json!({ "Path": path, "Recursive": true, "MaxResults": 10 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} ({path}): a namespace of the reserved hierarchy is read, got \
             {status} {served}"
        );
    }
}

/// One parameter written through the front door.
async fn put(router: &MultiServiceRouter, name: &str, value: &str) {
    let (status, written) = call(
        router,
        "PutParameter",
        &json!({ "Name": name, "Type": "String", "Value": value }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected {name} to be written, got {status} {written}"
    );
}

/// A name AWS publishes at this endpoint, discovered through the hierarchy read rather than
/// written here.
async fn a_published_name(router: &MultiServiceRouter) -> String {
    let (status, page) = call(
        router,
        "GetParametersByPath",
        &json!({ "Path": AMAZON_LINUX_NAMESPACE, "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the published namespace to be readable, got {status} {page}"
    );
    page["Parameters"][0]["Name"]
        .as_str()
        .unwrap_or_else(|| panic!("the published namespace reports a parameter, got {page}"))
        .to_string()
}
