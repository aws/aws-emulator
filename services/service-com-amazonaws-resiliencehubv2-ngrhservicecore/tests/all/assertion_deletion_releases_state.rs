// SPDX-License-Identifier: CC0-1.0

//! What `DeleteAssertion` does to the STORE, end to end over HTTP.
//!
//! The rule under test: deleting a child record removes it from its parent's record, so
//! every later read answers from a store that no longer holds it and the room it occupied
//! against its parent's ceiling is released — the records the delete did not name, in the
//! same parent and in every other one, keep projecting exactly what was stored, in every
//! later listing of them (unfiltered, filtered by `source`, and page by page).
//!
//! These suites complement `assertion_lifecycle`, which pins what a single delete answers
//! and that the surviving ids stay in creation order: here the SURVIVORS are compared
//! member by member against what their creates answered, the listing is walked across
//! pages whose `nextToken` was cut either side of a removed record, and the per-service
//! ceiling is driven to its bound to show the delete freed real state rather than hiding a
//! record a later create still pays for.
//!
//! Each suite is a TABLE: the rows differ along which positions of the parent's children
//! the delete removed and how many of them, not along one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    ASSERTION_SOURCE_USER, MAX_ASSERTIONS_PER_SERVICE,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes these suites drive.
///
/// SOURCE: `smithy_query(query="operation", operation="CreateAssertion,DeleteAssertion,
/// ListAssertions")` — `POST /v2/create-assertion`, `POST /v2/delete-assertion` and
/// `GET /v2/list-assertions`.
const CREATE_SERVICE: &str = "/v2/create-service";
const CREATE_ASSERTION: &str = "/v2/create-assertion";
const DELETE_ASSERTION: &str = "/v2/delete-assertion";
const LIST_ASSERTIONS: &str = "/v2/list-assertions";

/// A SigV4 `Authorization` header whose credential scope names this service's signing name.
/// Signature verification is off for these tests, so the value only has to parse.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY00000000/20260101/us-east-1/resiliencehub/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", SIGNED)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create a service named `name` and answer its ARN.
async fn create_service(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        CREATE_SERVICE,
        Some(json!({
            "name": name,
            "regions": ["us-east-1"],
            "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
        })),
    )
    .await;
    assert_eq!(status, 200, "creating service {name}: got {status} {body:?}");
    body["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {body:?}"))
        .to_string()
}

/// Create one assertion carrying `text` and answer the `Assertion` the create projected.
async fn create_assertion(
    router: &MultiServiceRouter,
    case: &str,
    service_arn: &str,
    text: &str,
) -> Value {
    let (status, body) = call(
        router,
        "POST",
        CREATE_ASSERTION,
        Some(json!({"serviceArn": service_arn, "text": text})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAssertion, got {status} {body:?}"
    );
    let assertion = body["assertion"].clone();
    assert!(
        !assertion.is_null(),
        "case {case:?}: the @required assertion member is absent from {body:?}"
    );
    assertion
}

async fn delete_assertion(
    router: &MultiServiceRouter,
    service_arn: &str,
    assertion_id: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        DELETE_ASSERTION,
        Some(json!({"serviceArn": service_arn, "assertionId": assertion_id})),
    )
    .await
}

/// The modelled error code of a refusal, from the restJson1 `__type` discriminator.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// One page of `ListAssertions`, as `(assertions, nextToken)`.
async fn list_page(
    router: &MultiServiceRouter,
    case: &str,
    query: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, body) = call(router, "GET", &format!("{LIST_ASSERTIONS}?{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListAssertions, got {status} {body:?}"
    );
    let assertions = body["assertions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the @required assertions member is absent from {body:?}")
        })
        .clone();
    let token = body["nextToken"].as_str().map(str::to_string);
    (assertions, token)
}

/// Every assertion of `service_arn`, read in pages of `page_size` and concatenated in the
/// order the pages reported them. `page_size` of `None` asks for the modelled default page.
async fn list_every_assertion(
    router: &MultiServiceRouter,
    case: &str,
    service_arn: &str,
    source: Option<&str>,
    page_size: Option<usize>,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    // One iteration per page, bounded by one page per record plus the terminating read.
    for _ in 0..=MAX_ASSERTIONS_PER_SERVICE {
        let mut query = format!("serviceArn={service_arn}");
        if let Some(source) = source {
            query.push_str(&format!("&source={source}"));
        }
        if let Some(size) = page_size {
            query.push_str(&format!("&maxResults={size}"));
        }
        if let Some(token) = &next {
            query.push_str(&format!("&nextToken={token}"));
        }
        let (page, token) = list_page(router, case, &query).await;
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case:?}: a page of at most {size} was asked for, got {} entries: \
                 {page:?}",
                page.len()
            );
        }
        collected.extend(page);
        match token {
            Some(token) => next = Some(token),
            None => return collected,
        }
    }
    panic!("case {case:?}: the listing never reported a last page: {collected:?}")
}

/// The distinct texts a row's assertions are created with: each one names its position, so
/// a survivor compared member by member also proves no OTHER record took its place.
fn text_for(case: &str, position: usize) -> String {
    format!("{case}: the service survives failure mode {position}.")
}

#[tokio::test]
async fn deleting_an_assertion_releases_its_room_against_the_per_service_ceiling() {
    let router = emulator();

    // The rows differ along how many of the parent's children the deletes removed: one, a
    // handful, and every one of them.
    for (case, freed) in [
        ("one deleted", 1usize),
        ("three deleted", 3),
        ("every one deleted", MAX_ASSERTIONS_PER_SERVICE),
    ] {
        let service_arn = create_service(&router, &case.replace(' ', "-")).await;
        let mut ids: Vec<String> = Vec::new();
        for position in 0..MAX_ASSERTIONS_PER_SERVICE {
            let created =
                create_assertion(&router, case, &service_arn, &text_for(case, position)).await;
            ids.push(
                created["assertionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        // At the ceiling the create is refused, so the room a delete frees is observable.
        let (status, body) = call(
            &router,
            "POST",
            CREATE_ASSERTION,
            Some(json!({"serviceArn": service_arn, "text": text_for(case, usize::MAX)})),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: expected HTTP 402 creating past the ceiling of \
             {MAX_ASSERTIONS_PER_SERVICE}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {body:?}"
        );

        // Free `freed` slots, taking them from alternating ends so the removals are not all
        // at one position in creation order.
        let mut removed: Vec<String> = Vec::new();
        for step in 0..freed {
            let id = if step % 2 == 0 {
                ids.remove(0)
            } else {
                ids.pop().expect("a remaining assertion")
            };
            let (status, body) = delete_assertion(&router, &service_arn, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DeleteAssertion, got {status} {body:?}"
            );
            assert_eq!(
                body["assertionId"].as_str(),
                Some(id.as_str()),
                "case {case:?}: expected the deleted assertion's id, got {body:?}"
            );
            removed.push(id);
        }

        // Exactly the freed room is available again: `freed` creates succeed and the next
        // one is refused, so the delete released state rather than hiding a record a later
        // create still pays for.
        for step in 0..freed {
            let created = create_assertion(
                &router,
                case,
                &service_arn,
                &text_for(case, MAX_ASSERTIONS_PER_SERVICE + step),
            )
            .await;
            let fresh = created["assertionId"].as_str().unwrap_or_default();
            assert!(
                !removed.iter().any(|gone| gone == fresh),
                "case {case:?}: a create after a delete must mint a fresh id, got {fresh:?} \
                 which names a removed assertion"
            );
        }
        let (status, body) = call(
            &router,
            "POST",
            CREATE_ASSERTION,
            Some(json!({"serviceArn": service_arn, "text": text_for(case, usize::MAX)})),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: the freed room is used up, so expected HTTP 402, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {body:?}"
        );

        // And the store holds exactly the ceiling: nothing the deletes removed is reported.
        let page = Some(MAX_ASSERTIONS_PER_SERVICE);
        let listed = list_every_assertion(&router, case, &service_arn, None, page).await;
        assert_eq!(
            listed.len(),
            MAX_ASSERTIONS_PER_SERVICE,
            "case {case:?}: expected {MAX_ASSERTIONS_PER_SERVICE} stored assertions, got {}",
            listed.len()
        );
        let reported: Vec<&str> = listed
            .iter()
            .map(|assertion| assertion["assertionId"].as_str().unwrap_or_default())
            .collect();
        for gone in &removed {
            assert!(
                !reported.iter().any(|id| id == gone),
                "case {case:?}: the removed assertion {gone:?} is still reported: {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_leaves_every_survivor_whole_in_every_later_listing() {
    let router = emulator();

    // A neighbouring parent, created first and read last: a delete reaches only the record
    // it names, inside the parent it names.
    let neighbour = create_service(&router, "Neighbour").await;
    let mut neighbour_created: Vec<Value> = Vec::new();
    for position in 0..3 {
        let text = text_for("neighbour", position);
        neighbour_created
            .push(create_assertion(&router, "the neighbour", &neighbour, &text).await);
    }

    // Five children per row; the rows differ along WHICH positions in creation order the
    // deletes removed — a leading one, a trailing one, an interior run, alternating ones,
    // all but one, and all of them.
    const CHILDREN: usize = 5;
    for (case, doomed) in [
        ("the first", vec![0usize]),
        ("the last", vec![CHILDREN - 1]),
        ("an interior run", vec![1, 2]),
        ("alternating positions", vec![0, 2, 4]),
        ("all but one", vec![0, 1, 2, 3]),
        ("every one", vec![0, 1, 2, 3, 4]),
    ] {
        let service_arn = create_service(&router, &case.replace(' ', "-")).await;
        let mut created: Vec<Value> = Vec::new();
        for position in 0..CHILDREN {
            let text = text_for(case, position);
            created.push(create_assertion(&router, case, &service_arn, &text).await);
        }

        for position in &doomed {
            let id = created[*position]["assertionId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (status, body) = delete_assertion(&router, &service_arn, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 deleting position {position}, got {status} \
                 {body:?}"
            );
            assert_eq!(
                body["assertionId"].as_str(),
                Some(id.as_str()),
                "case {case:?}: expected the deleted assertion's id, got {body:?}"
            );
        }

        // The survivors, as their creates projected them, in creation order.
        let survivors: Vec<Value> = created
            .iter()
            .enumerate()
            .filter(|(position, _)| !doomed.contains(position))
            .map(|(_, assertion)| assertion.clone())
            .collect();

        // Every later read of the parent answers exactly those records, whole: the default
        // page, the `source` filter the model declares, and the listing walked one and two
        // records at a time, so a `nextToken` cut either side of a removed record resumes
        // the same scan.
        for (read, source, page_size) in [
            ("the default page", None, None),
            ("the USER source filter", Some(ASSERTION_SOURCE_USER), None),
            ("pages of one", None, Some(1usize)),
            ("pages of two", None, Some(2)),
        ] {
            let listed = list_every_assertion(&router, case, &service_arn, source, page_size).await;
            assert_eq!(
                listed, survivors,
                "case {case:?} read through {read:?}: expected the surviving assertions \
                 exactly as created, got {listed:?}"
            );
        }
    }

    // The neighbouring parent kept every one of its children, member for member.
    let listed = list_every_assertion(&router, "the neighbour", &neighbour, None, Some(2)).await;
    assert_eq!(
        listed, neighbour_created,
        "another service's assertions must survive every delete aimed elsewhere, got {listed:?}"
    );
}

/// RULE: a delete releases EVERY piece of state the create wrote, the index of consumed
/// idempotency tokens included, and the per-service ceiling is held against that index as
/// well — so a caller repeating "create under a token, delete what it created" keeps
/// creating however many times it repeats, and the token the delete dropped names no
/// removed record afterwards.
///
/// The rows differ along how the churn uses tokens: a fresh token every cycle, and one
/// token reused every cycle. Both run one cycle more than the ceiling, so an index that
/// grew with every token consumed rather than with the records stored would refuse before
/// the last cycle.
#[tokio::test]
async fn a_delete_releases_the_token_index_so_token_churn_stays_bounded() {
    let router = emulator();

    for (case, reuse_one_token) in [
        ("a fresh token each cycle", false),
        ("one token reused", true),
    ] {
        let service_arn = create_service(&router, &case.replace(' ', "-")).await;
        for cycle in 0..=MAX_ASSERTIONS_PER_SERVICE {
            let token = if reuse_one_token {
                "churned-token".to_string()
            } else {
                format!("churned-token-{cycle}")
            };
            // The text differs every cycle, so a token still held by the index would be
            // refused as a conflict rather than answered as a retry.
            let (status, body) = call(
                &router,
                "POST",
                CREATE_ASSERTION,
                Some(json!({
                    "serviceArn": service_arn,
                    "text": text_for(case, cycle),
                    "clientToken": token,
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: cycle {cycle} under token {token:?} expected HTTP 200 from \
                 CreateAssertion, got {status} {body:?}"
            );
            let assertion_id = body["assertion"]["assertionId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: cycle {cycle} answered no assertionId: {body:?}")
                })
                .to_string();

            let (status, deleted) = delete_assertion(&router, &service_arn, &assertion_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: cycle {cycle} expected HTTP 200 from DeleteAssertion, got \
                 {status} {deleted:?}"
            );
        }

        // Every cycle removed what it created, so the parent holds nothing at the end.
        let listed = list_every_assertion(&router, case, &service_arn, None, None).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: every cycle deleted what it created, so the service must hold no \
             assertion, got {listed:?}"
        );
    }
}
