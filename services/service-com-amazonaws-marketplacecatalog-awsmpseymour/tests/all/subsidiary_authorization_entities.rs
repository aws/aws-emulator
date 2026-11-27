// SPDX-License-Identifier: CC0-1.0

//! The `SubsidiaryAuthorization` entity type, and the offer member that names one, end to
//! end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization. Nothing seeds the
//! store: the entities read back are the ones the change sets started below created.
//!
//! The rule under test: an entity type the model declares per-type shapes for is served
//! through all of them — its own `EntitySummary` member reports the details members that
//! summary names, its own `*Filters` member selects on each of those attributes, and its
//! own `*SortBy` enum is the set of attributes a list of that type orders by — and a
//! details value a summary member cannot hold fails its change set rather than being
//! reported.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The attributes a list of `SubsidiaryAuthorization` orders by: the values of
/// `SubsidiaryAuthorizationSortBy`, written out here from the model rather than read from
/// the crate (`smithy_query(query="shape", shape="SubsidiaryAuthorizationSortBy")`).
const SORT_ATTRIBUTES: &[&str] = &[
    "EntityId",
    "Status",
    "ResourceId",
    "SubsidiaryAccountId",
    "CreatedDate",
    "LastModifiedDate",
];

/// The statuses a subsidiary authorization publishes, from
/// `SubsidiaryAuthorizationSummary$Status`'s own doc ("valid values are: `Draft` …
/// `Active` … `Restricted`") and the enum its filter selects through
/// (`smithy_query(query="shape", shape="SubsidiaryAuthorizationStatusString")`).
const PUBLISHED_STATUSES: &[&str] = &["Draft", "Active", "Restricted"];

/// The `ErrorDetail$ErrorCode` the catalog reports for a details value it cannot read.
const INVALID_VALUE: &str = "INVALID_VALUE";

/// How long a poll waits for a condition the clock owes before giving up: far longer than
/// any span this service schedules, so a loaded host waits rather than failing.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll sleeps between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The array at `pointer`, or an empty one when the member is absent.
    fn list(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
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

/// Start one change set per row, each creating a new entity of `entity_type` from the
/// row's details, and answer the change set ids in order.
async fn started_change_sets(
    router: &MultiServiceRouter,
    entity_type: &str,
    rows: &[Value],
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::with_capacity(rows.len());
    for details in rows {
        let started = call(
            router,
            "POST",
            "/StartChangeSet",
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": format!("{entity_type}@1.0")},
                    "DetailsDocument": details,
                }],
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "a change set creating a {entity_type} must be accepted, got {} with {:?}",
            started.status, started.body
        );
        ids.push(started.text("/ChangeSetId"));
    }
    ids
}

/// `DescribeChangeSet` of `change_set_id` once its run has ended.
async fn ended_change_set(router: &MultiServiceRouter, change_set_id: &str) -> Answer {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let described = call(
            router,
            "GET",
            &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={change_set_id}"),
            None,
        )
        .await;
        let status = described.text("/Status");
        if status == "SUCCEEDED" || status == "FAILED" {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "the change set {change_set_id} must end its run, got {status:?} with {:?}",
            described.body
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The same, asserting the change set succeeded, and answer the entity it created.
async fn created_entity(router: &MultiServiceRouter, change_set_id: &str) -> String {
    let described = ended_change_set(router, change_set_id).await;
    assert_eq!(
        described.text("/Status"),
        "SUCCEEDED",
        "the fixture's change set {change_set_id} must succeed, got {:?}",
        described.body
    );
    let listed = call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG, "MaxResults": 20})),
    )
    .await;
    let summary = listed
        .list("/ChangeSetSummaryList")
        .into_iter()
        .find(|item| item["ChangeSetId"].as_str() == Some(change_set_id))
        .unwrap_or_else(|| {
            panic!(
                "the fixture's change set {change_set_id} must be listed, got {:?}",
                listed.body
            )
        });
    summary["EntityIdList"][0]
        .as_str()
        .unwrap_or_else(|| panic!("the change set must name its entity, got {summary:?}"))
        .to_string()
}

/// `ListEntities` of `entity_type` with `extra` members beside the catalog.
async fn list_entities(router: &MultiServiceRouter, entity_type: &str, extra: Value) -> Answer {
    let mut request = json!({
        "Catalog": CATALOG,
        "EntityType": entity_type,
        "MaxResults": 50,
    });
    if let (Value::Object(into), Value::Object(members)) = (&mut request, extra) {
        for (key, value) in members {
            into.insert(key, value);
        }
    }
    call(router, "POST", "/ListEntities", Some(request)).await
}

/// The `EntityId`s a `ListEntities` answer reported, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer
        .list("/EntitySummaryList")
        .iter()
        .map(|summary| {
            summary["EntityId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The details of a subsidiary authorization: every member its own summary reports.
///
/// The values are chosen so that every attribute the type's own sort enum names orders
/// the rows the same way: `Active` < `Draft` < `Restricted` is the alphabetical order of
/// the statuses the set publishes, and the resources, accounts and instants rise with
/// them.
fn authorization(
    name: &str,
    status: &str,
    resource_id: &str,
    account_id: &str,
    created: &str,
) -> Value {
    json!({
        "Name": name,
        "Status": status,
        "ResourceId": resource_id,
        "ResourceName": format!("{resource_id} product"),
        "SubsidiaryAccountId": account_id,
        "CreatedDate": created,
    })
}

/// `details` with `member` carrying `value` instead of what it held.
fn with(details: &Value, member: &str, value: Value) -> Value {
    let mut members = details.as_object().cloned().unwrap_or_default();
    members.insert(member.to_string(), value);
    Value::Object(members)
}

/// A subsidiary authorization is reported through its own summary member, selected
/// through its own filters, and ordered by the attributes its own sort enum names.
///
/// The rows differ along the status, the authorized resource, the subsidiary account and
/// the creation instant, which is what the filters and sorts below are read over.
#[tokio::test]
async fn a_subsidiary_authorization_is_served_through_its_own_shapes() {
    let router = emulator();
    let rows = vec![
        authorization(
            "first authorization",
            "Active",
            "prod-aaa111",
            "111122223333",
            "2026-01-01T00:00:00Z",
        ),
        authorization(
            "second authorization",
            "Draft",
            "prod-bbb222",
            "222233334444",
            "2026-02-01T00:00:00Z",
        ),
        authorization(
            "third authorization",
            "Restricted",
            "prod-ccc333",
            "333344445555",
            "2026-03-01T00:00:00Z",
        ),
    ];
    let change_sets = started_change_sets(&router, "SubsidiaryAuthorization", &rows).await;
    let mut entities: Vec<String> = Vec::with_capacity(rows.len());
    for change_set_id in &change_sets {
        entities.push(created_entity(&router, change_set_id).await);
    }

    // Every member the per-type summary names is reported from the details the change
    // submitted, and no other type's summary is reported beside it.
    let served = list_entities(&router, "SubsidiaryAuthorization", json!({})).await;
    assert_eq!(
        served.status, 200,
        "listing the type must answer a page, got {} with {:?}",
        served.status, served.body
    );
    for (details, entity_id) in rows.iter().zip(entities.iter()) {
        let summary = served
            .list("/EntitySummaryList")
            .into_iter()
            .find(|item| item["EntityId"].as_str() == Some(entity_id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "the created authorization {entity_id} must be listed, got {:?}",
                    served.body
                )
            });
        assert_eq!(
            (
                summary["EntityType"].as_str().unwrap_or_default(),
                summary["Name"].as_str().unwrap_or_default(),
            ),
            (
                "SubsidiaryAuthorization",
                details["Name"].as_str().unwrap_or_default()
            ),
            "case {entity_id:?}: the common members must report the persisted entity, got \
             {summary:?}"
        );
        for member in [
            "Name",
            "Status",
            "ResourceId",
            "ResourceName",
            "SubsidiaryAccountId",
            "CreatedDate",
        ] {
            assert_eq!(
                summary["SubsidiaryAuthorizationSummary"][member],
                details[member],
                "case {entity_id:?}: the per-type summary must report {member} from the \
                 details the change submitted, got {summary:?}"
            );
        }
        for other in ["AmiProductSummary", "OfferSummary", "OfferSetSummary"] {
            assert!(
                summary.get(other).is_none_or(Value::is_null),
                "case {entity_id:?}: {other} belongs to another type, got {summary:?}"
            );
        }

        let described = call(
            &router,
            "GET",
            &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
            None,
        )
        .await;
        assert_eq!(
            (described.status, described.text("/EntityType")),
            (200, "SubsidiaryAuthorization@1.0".to_string()),
            "case {entity_id:?}: the entity must be described under its own type, got {} \
             with {:?}",
            described.status,
            described.body
        );
    }

    // (case, the filters asked for, the entities they select)
    let filters: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "one published status",
            json!({"Status": {"ValueList": ["Draft"]}}),
            vec![entities[1].clone()],
        ),
        (
            "two published statuses",
            json!({"Status": {"ValueList": ["Active", "Restricted"]}}),
            vec![entities[0].clone(), entities[2].clone()],
        ),
        (
            "the authorized resource",
            json!({"ResourceId": {"ValueList": ["prod-ccc333"]}}),
            vec![entities[2].clone()],
        ),
        (
            "the subsidiary account",
            json!({"SubsidiaryAccountId": {"ValueList": ["111122223333"]}}),
            vec![entities[0].clone()],
        ),
        (
            "a partial resource name",
            json!({"ResourceName": {"WildCardValue": "bbb222"}}),
            vec![entities[1].clone()],
        ),
        (
            "a partial name",
            json!({"Name": {"WildCardValue": "third"}}),
            vec![entities[2].clone()],
        ),
        (
            "a creation window",
            json!({"CreatedDate": {"DateRange": {
                "AfterValue": "2026-01-15T00:00:00Z",
                "BeforeValue": "2026-02-15T00:00:00Z",
            }}}),
            vec![entities[1].clone()],
        ),
        (
            "an entity id",
            json!({"EntityId": {"ValueList": [entities[0].clone()]}}),
            vec![entities[0].clone()],
        ),
        (
            "a status no authorization carries",
            json!({"Status": {"ValueList": ["Draft"]}, "ResourceId": {"ValueList": ["prod-ccc333"]}}),
            Vec::new(),
        ),
    ];
    for (case, filter, expected) in filters {
        let answer = list_entities(
            &router,
            "SubsidiaryAuthorization",
            json!({"EntityTypeFilters": {"SubsidiaryAuthorizationFilters": filter}}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a filter of the type being listed must be served, got {} \
             with {:?}",
            answer.status, answer.body
        );
        let mut served = listed_ids(&answer);
        served.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            served, expected,
            "case {case:?}: the filter must select on what the entities hold, got {:?}",
            answer.body
        );
    }

    // Every attribute the type's own sort enum names orders the page, in both directions,
    // and an attribute belonging to another type orders nothing on this one.
    for sort_by in SORT_ATTRIBUTES {
        for order in ["ASCENDING", "DESCENDING"] {
            let answer = list_entities(
                &router,
                "SubsidiaryAuthorization",
                json!({"EntityTypeSort": {"SubsidiaryAuthorizationSort": {
                    "SortBy": sort_by,
                    "SortOrder": order,
                }}}),
            )
            .await;
            let served = listed_ids(&answer);
            assert_eq!(
                (answer.status, served.len()),
                (200, entities.len()),
                "case {sort_by:?}/{order:?}: the whole page must be served, got {} with {:?}",
                answer.status,
                answer.body
            );
            // The rows differ along every attribute but `LastModifiedDate`, which they all
            // took from the one instant their change sets were applied at, so only the
            // others say anything about the ORDER.
            if *sort_by == "LastModifiedDate" {
                continue;
            }
            let mut expected = entities.clone();
            if order == "DESCENDING" {
                expected.reverse();
            }
            assert_eq!(
                served, expected,
                "case {sort_by:?}/{order:?}: the page must be ordered by the attribute \
                 asked for, got {:?}",
                answer.body
            );
        }
    }
    for sort_by in ["ProductTitle", "BuyerAccounts"] {
        let refused = list_entities(
            &router,
            "SubsidiaryAuthorization",
            json!({"Sort": {"SortBy": sort_by}}),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {sort_by:?}: an attribute this type publishes no sort for must be \
             refused, got {} with {:?}",
            refused.status,
            refused.body
        );
    }
}

/// A details value the per-type summary member cannot hold fails its change set, so no
/// `ListEntities` answer reports it.
///
/// The rows span the forms the model declares for those members: a status outside the
/// published set, an account id outside its fixed length, a created date outside the
/// instant format, and a resource name past its published length.
#[tokio::test]
async fn a_value_a_subsidiary_authorization_summary_cannot_hold_fails_its_change_set() {
    let router = emulator();
    let sound = authorization(
        "an authorization",
        "Active",
        "prod-aaa111",
        "111122223333",
        "2026-01-01T00:00:00Z",
    );
    // (case, the details one change carries, the member the reported defect names)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a status outside the published set",
            with(&sound, "Status", json!("Revoked")),
            "Status",
        ),
        (
            "an account id of eleven digits",
            with(&sound, "SubsidiaryAccountId", json!("11112222333")),
            "SubsidiaryAccountId",
        ),
        (
            "a created date that is not an instant",
            with(&sound, "CreatedDate", json!("2026-01-01")),
            "CreatedDate",
        ),
        (
            "a resource name one character past @length(max: 255)",
            with(&sound, "ResourceName", json!("r".repeat(256))),
            "ResourceName",
        ),
    ];

    let details: Vec<Value> = rows.iter().map(|(_, details, _)| details.clone()).collect();
    let change_sets = started_change_sets(&router, "SubsidiaryAuthorization", &details).await;

    for ((case, _, member), change_set_id) in rows.iter().zip(change_sets.iter()) {
        let described = ended_change_set(&router, change_set_id).await;
        assert_eq!(
            (described.text("/Status"), described.text("/FailureCode")),
            ("FAILED".to_string(), "CLIENT_ERROR".to_string()),
            "case {case:?}: a value the summary member cannot hold must fail the change \
             set, got {:?}",
            described.body
        );
        let errors = described
            .list("/ChangeSet")
            .first()
            .map(|change| {
                change["ErrorDetailList"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: the change must report the one defect it carries, got {:?}",
            described.body
        );
        assert_eq!(
            errors[0]["ErrorCode"].as_str().unwrap_or_default(),
            INVALID_VALUE,
            "case {case:?}: a value outside a member's contract is an {INVALID_VALUE}, \
             got {:?}",
            errors[0]
        );
        assert!(
            errors[0]["ErrorMessage"]
                .as_str()
                .unwrap_or_default()
                .starts_with(&format!("Details.{member} accepts ")),
            "case {case:?}: the error must name the member it is about and what that \
             member accepts, got {:?}",
            errors[0]
        );
    }

    // Nothing was created: a failed change set applies nothing.
    let served = list_entities(&router, "SubsidiaryAuthorization", json!({})).await;
    assert!(
        listed_ids(&served).is_empty(),
        "a failed change set must leave no entity behind, got {:?}",
        served.body
    );

    // And the statuses the set publishes are all accepted, so the refusals above are
    // about the value and not about the member.
    let accepted: Vec<Value> = PUBLISHED_STATUSES
        .iter()
        .map(|status| {
            authorization(
                "an authorization",
                status,
                "prod-aaa111",
                "111122223333",
                "2026-01-01T00:00:00Z",
            )
        })
        .collect();
    let change_sets = started_change_sets(&router, "SubsidiaryAuthorization", &accepted).await;
    for (status, change_set_id) in PUBLISHED_STATUSES.iter().zip(change_sets.iter()) {
        let entity_id = created_entity(&router, change_set_id).await;
        let served = list_entities(
            &router,
            "SubsidiaryAuthorization",
            json!({"EntityTypeFilters": {"SubsidiaryAuthorizationFilters": {
                "EntityId": {"ValueList": [entity_id.clone()]},
            }}}),
        )
        .await;
        let summary = served
            .list("/EntitySummaryList")
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!("case {status:?}: the authorization must be listed, got {:?}", served.body)
            });
        assert_eq!(
            summary["SubsidiaryAuthorizationSummary"]["Status"],
            json!(status),
            "case {status:?}: a published status must be reported as submitted, got \
             {summary:?}"
        );
    }
}

/// An offer reports the subsidiary authorization it was created from, is selected by it,
/// and is refused an id outside the form that member publishes.
#[tokio::test]
async fn an_offer_reports_the_subsidiary_authorization_it_was_created_from() {
    let router = emulator();
    // `OfferSubsidiaryAuthorizationIdString` is `@length(min: 1, max: 22)`,
    // `^subauthz-[a-zA-Z0-9]{13}$`: the prefix and exactly thirteen alphanumerics.
    let authorized = "subauthz-0123456789abc";
    let rows: Vec<Value> = vec![
        json!({
            "Name": "an offer from a subsidiary authorization",
            "ProductId": "prod-aaa111",
            "SubsidiaryAuthorizationId": authorized,
        }),
        json!({"Name": "an offer from no authorization", "ProductId": "prod-bbb222"}),
    ];
    let change_sets = started_change_sets(&router, "Offer", &rows).await;
    let mut entities: Vec<String> = Vec::with_capacity(rows.len());
    for change_set_id in &change_sets {
        entities.push(created_entity(&router, change_set_id).await);
    }

    let served = list_entities(&router, "Offer", json!({})).await;
    let summaries = served.list("/EntitySummaryList");
    let from_authorization = summaries
        .iter()
        .find(|summary| summary["EntityId"].as_str() == Some(entities[0].as_str()))
        .unwrap_or_else(|| panic!("the first offer must be listed, got {:?}", served.body));
    assert_eq!(
        from_authorization["OfferSummary"]["SubsidiaryAuthorizationId"],
        json!(authorized),
        "the offer must report the authorization it was created from, got \
         {from_authorization:?}"
    );
    let from_nothing = summaries
        .iter()
        .find(|summary| summary["EntityId"].as_str() == Some(entities[1].as_str()))
        .unwrap_or_else(|| panic!("the second offer must be listed, got {:?}", served.body));
    assert!(
        from_nothing["OfferSummary"]["SubsidiaryAuthorizationId"].is_null(),
        "an offer from no authorization must report none, got {from_nothing:?}"
    );

    // The offer filter of that member selects the offer that carries it, and the member is
    // one the offer list orders by.
    let selected = list_entities(
        &router,
        "Offer",
        json!({"EntityTypeFilters": {"OfferFilters": {
            "SubsidiaryAuthorizationId": {"ValueList": [authorized]},
        }}}),
    )
    .await;
    assert_eq!(
        listed_ids(&selected),
        vec![entities[0].clone()],
        "the filter must select the offer created from that authorization, got {:?}",
        selected.body
    );
    let ordered = list_entities(
        &router,
        "Offer",
        json!({"EntityTypeSort": {"OfferSort": {"SortBy": "SubsidiaryAuthorizationId"}}}),
    )
    .await;
    assert_eq!(
        (ordered.status, listed_ids(&ordered).len()),
        (200, 2),
        "the offer list must order by the attribute its own sort enum names, got {} \
         with {:?}",
        ordered.status,
        ordered.body
    );

    // An id outside the form the member publishes fails its change set.
    let refused = started_change_sets(
        &router,
        "Offer",
        &[json!({
            "Name": "an offer naming a malformed authorization",
            "SubsidiaryAuthorizationId": "subauthz-short",
        })],
    )
    .await;
    let described = ended_change_set(&router, &refused[0]).await;
    assert_eq!(
        described.text("/Status"),
        "FAILED",
        "an id outside the member's published form must fail the change set, got {:?}",
        described.body
    );
}
