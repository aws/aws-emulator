// SPDX-License-Identifier: CC0-1.0

//! What a response carries for a `@required` member the service has NO VALUE for.
//!
//! RULE under test: **a response carries no key for a member that holds no value — a string
//! member whose own `@length` minimum is 1 or more admits no empty value at all, so "unset"
//! for such a member is reported by leaving the member out, never as the empty string.**
//! The rule and the walk that applies it at every nesting level are stated once, for every
//! service of that class, in `service_common::unset_string_members`; a service takes them at
//! its own wire protocol.
//!
//! The rows drive that one shared path through the two kinds of member it covers, each
//! reached by a different operation of the IoT SiteWise surface:
//!
//! * a PAGINATION TOKEN the model marks `@required` while documenting it as null on the
//!   last page — `ListActionsResponse$nextToken`, targeting `NextToken` (`@length` min 1,
//!   `@pattern ^[A-Za-z0-9+/=]+$`, so no value of it means "no next page"). The walk is
//!   repeated at several page sizes, because what the rule generalises over here is how many
//!   pages there are and therefore which page has no token to report.
//! * a DESCRIPTION a resource was created without — `DescribeAssetModelResponse`'s, also
//!   `@required` on the read side while the member that writes it is optional on the create,
//!   targeting `Description` (`@length` min 1, max 2048).
//!
//! Both members are `@required`, so the derived shapes hold a plain `String` for them and
//! their serializers write it unconditionally: without the shared projection the bodies
//! below would carry `""` — a value the member's own shape rejects, and one a paginator
//! would feed straight back into a request the request boundary refuses.
//!
//! Why it lives beside the router's suites rather than in the service crate: the invariant
//! spans two crates — the `service-common` module that owns the rule and the service whose
//! protocol takes it — and this is the test target that may observe both at once.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{ActionPayload, TargetResource};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, ActionRecord};
use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

/// The Region every request below signs for; the credential scope is what routes it.
const REGION: &str = "us-west-2";

/// Where `ListActions` is bound: `GET /actions`
/// (`smithy_query(query="operation", operation="ListActions")`).
const ACTIONS: &str = "/actions";

/// The ids of the actions this suite records, in the order the listing reports them —
/// oldest first, which is the order `ListActions` keeps.
const ACTION_IDS: [&str; 5] = [
    "aaaaaaaa-1111-4111-8111-111111111111",
    "aaaaaaaa-2222-4222-8222-222222222222",
    "aaaaaaaa-3333-4333-8333-333333333333",
    "aaaaaaaa-4444-4444-8444-444444444444",
    "aaaaaaaa-5555-4555-8555-555555555555",
];

/// The action definition every recorded action runs; one is enough, since nothing here is
/// about which definition an action names.
const DEFINITION_ID: &str = "deadbeef-1111-4111-8111-111111111111";

/// The maximum `@length` of `Description`
/// (`smithy_query(query="shape", shape="Description")` — min 1, max 2048).
const DESCRIPTION_MAX_LENGTH: usize = 2048;

/// A router with just this service mounted, keeping the shared engine so a test can record
/// an action the way the operation that takes one records it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Authentication off, so the suite is independent of the process-wide switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request and answer `(status, parsed body)`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iotsitewise/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(raw.as_ref()).into_owned()))
    };
    (status, parsed)
}

/// `GET uri`, which must answer 200.
async fn read(router: &MultiServiceRouter, uri: &str) -> Value {
    let (status, body) = call(router, "GET", uri, None).await;
    assert_eq!(status, 200, "GET {uri}: expected 200, got {status} {body}");
    body
}

/// Create an asset model, with `description` when the row supplies one, and answer its id.
async fn create_model(
    router: &MultiServiceRouter,
    name: &str,
    description: Option<&str>,
) -> String {
    let mut body = json!({ "assetModelName": name });
    if let (Some(object), Some(description)) = (body.as_object_mut(), description) {
        object.insert("assetModelDescription".to_string(), json!(description));
    }
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(
        status, 202,
        "create model {name}: expected 202, got {status} {created}"
    );
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create model {name}: no assetModelId in {created}"))
        .to_string()
}

/// Create an asset of `model_id` and answer its id — the resource the recorded actions are
/// taken on, which `ListActions` resolves before it answers.
async fn create_asset(router: &MultiServiceRouter, name: &str, model_id: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/assets",
        Some(json!({ "assetName": name, "assetModelId": model_id })),
    )
    .await;
    assert_eq!(
        status, 202,
        "create asset {name}: expected 202, got {status} {created}"
    );
    created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create asset {name}: no assetId in {created}"))
        .to_string()
}

/// Record the actions of [`ACTION_IDS`] against `asset_id` in the shared store, as the
/// operation that takes an action records them: ascending execution times, so the listing's
/// order is the order they are declared in.
fn record_actions(engine: &Arc<engine::InMemoryEngine>, asset_id: &str) {
    let state = service_state(&**engine).expect("the iotsitewise store");
    let mut store = state.lock();
    for (at, id) in ACTION_IDS.iter().enumerate() {
        let ran_ago = Duration::from_secs((ACTION_IDS.len() - at) as u64 * 100);
        store.actions.insert(
            (*id).to_string(),
            ActionRecord {
                action_id: (*id).to_string(),
                action_definition_id: DEFINITION_ID.to_string(),
                target_resource: TargetResource {
                    asset_id: Some(asset_id.to_string()),
                    computation_model_id: None,
                },
                action_payload: ActionPayload {
                    string_value: "{}".to_string(),
                },
                resolve_to: None,
                execution_time: SystemTime::now() - ran_ago,
                client_token: None,
            },
        );
    }
}

/// `true` when `token` lies inside the modelled `NextToken` value space: 1..=4096 characters
/// of `^[A-Za-z0-9+/=]+$`
/// (`smithy_query(query="operation", operation="ListActions")`). Spelled out here rather
/// than taken from a regex crate, so the check is an independent oracle.
fn in_next_token_value_space(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '/' || ch == '=')
}

/// The action ids one page reports, in the order it reports them.
fn page_ids(case: &str, body: &Value) -> Vec<String> {
    body["actionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a page carries actionSummaries, got {body}"))
        .iter()
        .map(|summary| {
            summary["actionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a summary names its action, got {body}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_last_page_of_a_listing_reports_no_pagination_token_member() {
    let (router, engine) = emulator();
    let model_id = create_model(&router, "Boiler", None).await;
    let asset_id = create_asset(&router, "Boiler-A", &model_id).await;
    record_actions(&engine, &asset_id);

    // One row per page size: every page size that splits the recorded run (so some page is
    // followed by another), one that holds it exactly, and the published maximum, which
    // holds it whole. What the rule generalises over here is which page has no token left to
    // report — the last one, whichever page size produced it.
    for page_size in [1_usize, 2, 3, 5, 250] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            let mut uri = format!(
                "{ACTIONS}?targetResourceType=ASSET&targetResourceId={asset_id}\
                 &maxResults={page_size}"
            );
            if let Some(resume) = token.as_deref() {
                uri.push_str(&format!("&nextToken={resume}"));
            }
            let body = read(&router, &uri).await;
            pages += 1;
            seen.extend(page_ids(&case, &body));

            // The rule: the page that leaves nothing to resume from carries no `nextToken`
            // MEMBER at all — not the empty string, and not a JSON null.
            let reported = body.get("nextToken");
            if seen.len() == ACTION_IDS.len() {
                assert!(
                    reported.is_none(),
                    "case {case:?}: the last page must report no nextToken member, got \
                     {reported:?} in {body}"
                );
                break;
            }

            let resume = reported
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a page with {} of {} actions reported so far must carry a \
                         nextToken, got {reported:?} in {body}",
                        seen.len(),
                        ACTION_IDS.len()
                    )
                })
                .to_string();
            assert!(
                in_next_token_value_space(&resume),
                "case {case:?}: a reported nextToken must lie inside the modelled NextToken \
                 value space, got {resume:?} in {body}"
            );
            token = Some(resume);

            assert!(
                pages <= ACTION_IDS.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages for {} actions",
                ACTION_IDS.len()
            );
        }

        assert_eq!(
            seen,
            ACTION_IDS.map(str::to_string).to_vec(),
            "case {case:?}: the pages joined must be the recorded run, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_read_reports_a_description_only_for_a_resource_that_has_one() {
    let (router, _engine) = emulator();

    // One row per form the description may be created in: absent, both published `@length`
    // bounds, and an ordinary sentence. The rule is the same one the token rows drive, over
    // a member of a different operation's response.
    let rows: [(&str, Option<String>); 4] = [
        ("no description at all", None),
        ("the shortest legal description", Some("d".to_string())),
        (
            "an ordinary description",
            Some("the boiler on the north line".to_string()),
        ),
        (
            "a description of the maximum length",
            Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
        ),
    ];

    for (at, (case, description)) in rows.into_iter().enumerate() {
        // A name of its own per row: an asset model's name is unique per account.
        let name = format!("Boiler-{at}");
        let model_id = create_model(&router, &name, description.as_deref()).await;
        let body = read(&router, &format!("/asset-models/{model_id}")).await;
        match description.as_deref() {
            None => assert!(
                body.get("assetModelDescription").is_none(),
                "case {case:?}: a model created without a description must report no \
                 assetModelDescription member, got {body}"
            ),
            Some(text) => assert_eq!(
                body["assetModelDescription"].as_str(),
                Some(text),
                "case {case:?}: expected the description back, got {:?}",
                body.get("assetModelDescription")
            ),
        }
    }
}
