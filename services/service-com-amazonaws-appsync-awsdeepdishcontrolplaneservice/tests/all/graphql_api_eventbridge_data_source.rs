// SPDX-License-Identifier: CC0-1.0

//! A resolver whose data source is of the `AMAZON_EVENTBRIDGE` type, driven over the API's own
//! published `GRAPHQL` endpoint, with Amazon EventBridge, Amazon SQS and IAM mounted on the
//! same engine — so what the publication came to is read back where a client reads it: on the
//! queue the matched rule targets, and in IAM's own documents.
//!
//! The rule these cases assert, one level above the recorded case (one mutation publishing one
//! event onto one bus):
//!
//! > A step whose data source is of the `AMAZON_EVENTBRIDGE` type PUBLISHES the events its
//! > request half returned onto the bus that data source was configured with
//! > (`eventBridgeConfig.eventBusArn`), under the permissions of the role it names
//! > (`serviceRoleArn`): every event of the request is published, in the order the request
//! > states them, so each is routed by the bus's own rules independently of the rest, and the
//! > result the response half is handed is EventBridge's own answer — `Entries`, one per event
//! > with the `EventId` that service minted, and `FailedEntryCount`. A request this data source
//! > cannot act on — one whose event carries a `detail` that is not a JSON object among them —
//! > publishes NONE of its events and is reported as an error of the request's own member, and
//! > a role whose identity-based documents withhold `events:PutEvents` on that bus publishes
//! > none either.
//!
//! The rows span what the rule generalises over: how many events one request carries (one,
//! several, the most a publication carries and one past it), which of them the bus's rules
//! match, the members an event states (`resources` and `time` beside the three required ones),
//! the member that makes a request one this data source cannot act on, what the role's
//! documents say (an allow for this bus, an allow for another bus, an allow for another action,
//! an explicit deny, and no document at all), and which KIND of step resolves the field — a
//! `UNIT` resolver on the data source, and a FUNCTION of a `PIPELINE` resolver on it, alone in
//! its pipeline or after a step that resolves against no data source at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{try_post_answer, GRAPHQL_PATH};
use crate::identity_authorization::{form_encode, xml_element};

/// The Region every call of this suite is signed for, which is the Region the API's data plane
/// publishes into: the control-plane helper this suite shares signs for it
/// ([`crate::event_api_data_plane::appsync`]).
const REGION: &str = "us-west-2";

/// How long a case waits for a delivery to show up on the queue, and how often it looks again.
/// Generous for a loaded machine, and bounded so a delivery that will never be made fails the
/// case instead of hanging it.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);
const POLL_SPACING: Duration = Duration::from_millis(50);

/// How long a case waits for the clock to perform a schema creation, far past the service's own
/// span (`state::SCHEMA_CREATION_SPAN`), so a loaded machine still reaches it.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);

/// The `runtime` the resolver of this suite is defined under. Source: `AppSyncRuntime`'s own
/// values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The schema every API of this suite is created with: one mutation that publishes the events
/// it is handed, answering what EventBridge reported about them, the two further mutations the
/// case about a data source this endpoint does not call attaches its resolvers to, and the two
/// the pipeline case attaches its own to.
const SCHEMA: &str = "schema { query: Query mutation: Mutation }\n\
                      \n\
                      type Query {\n  ping: String\n}\n\
                      \n\
                      type Mutation {\n  publish(events: AWSJSON!): Published\n  \
                      publishCoded(events: AWSJSON!): Published\n  \
                      publishTemplated(events: AWSJSON!): Published\n  \
                      publishPiped(events: AWSJSON!): Published\n  \
                      publishStashedPiped(events: AWSJSON!): Published\n}\n\
                      \n\
                      type Published {\n  failedCount: Int\n  eventIds: [String]\n}\n";

/// The resolver attached to that mutation: its request half returns the `PutEvents` request the
/// data source is called with, and its response half re-raises what went wrong with the
/// publication (`ctx.error`) or reports what EventBridge answered about it.
const RESOLVER_CODE: &str = "export function request(ctx) {\n\
                             return {operation: 'PutEvents', events: JSON.parse(ctx.args.events)};\n\
                             }\n\
                             export function response(ctx) {\n\
                             if (ctx.error) { util.error(ctx.error.message, ctx.error.type); }\n\
                             return {\n\
                             failedCount: ctx.result.FailedEntryCount,\n\
                             eventIds: ctx.result.Entries.map(function (e) { return e.EventId; }),\n\
                             };\n\
                             }\n";

/// The name of the data source every resolver of this suite resolves against. `DataSource$name`
/// admits only identifier characters (`^[_A-Za-z][_0-9A-Za-z]*$`).
const DATA_SOURCE: &str = "ordersBus";

/// The name of the data source of a type this endpoint does NOT call, which one case attaches
/// its own resolvers to.
const UNCALLED_DATA_SOURCE: &str = "ordersTable";

/// The data source the step that publishes NOTHING resolves against: `NONE` is "There is no
/// data source", so the step of a two-step pipeline that only stashes the events is bound to
/// one of that type.
const STASH_DATA_SOURCE: &str = "localSteps";

/// The names the pipelines of the pipeline case invoke their functions by. `ResourceName`
/// admits only identifier characters.
const PUBLISHER_FUNCTION: &str = "publisher";
const STASHER_FUNCTION: &str = "stasher";

/// The mutations the pipeline case drives: one resolved by a pipeline over the publishing
/// function alone, one by a pipeline that stashes the events in an earlier step.
const ONE_STEP_FIELD: &str = "publishPiped";
const TWO_STEP_FIELD: &str = "publishStashedPiped";

/// The function that PUBLISHES, as one step of a pipeline: the same request half the suite's
/// `UNIT` resolver has, reading the events an earlier step stashed where one did, and the
/// mutation's own argument otherwise.
const PUBLISHER_CODE: &str = "export function request(ctx) {\n\
                              let events = ctx.stash.events;\n\
                              if (!events) { events = JSON.parse(ctx.args.events); }\n\
                              return {operation: 'PutEvents', events: events};\n\
                              }\n\
                              export function response(ctx) {\n\
                              if (ctx.error) { util.error(ctx.error.message, ctx.error.type); }\n\
                              return {\n\
                              failedCount: ctx.result.FailedEntryCount,\n\
                              eventIds: ctx.result.Entries.map(function (e) { return e.EventId; }),\n\
                              };\n\
                              }\n";

/// The step BEFORE it in the two-step pipeline: it stashes the events the mutation carries and
/// publishes nothing of its own.
const STASHER_CODE: &str = "export function request(ctx) {\n\
                            ctx.stash.events = JSON.parse(ctx.args.events);\n\
                            return {payload: null};\n\
                            }\n\
                            export function response(ctx) { return ctx.result; }\n";

/// The pipeline resolver's own halves, which surround its functions: the before step asks
/// nothing of its own and the after step answers what the last function produced.
const PIPELINE_CODE: &str = "export function request(ctx) { return {}; }\n\
                             export function response(ctx) { return ctx.prev.result; }\n";

/// The source the bus's rule matches on, and one no rule of these cases matches.
const MATCHED_SOURCE: &str = "com.demo.orders";
const UNMATCHED_SOURCE: &str = "com.demo.elsewhere";

/// The detail type every event of this suite carries.
const DETAIL_TYPE: &str = "Order Placed";

/// The most events one publication carries. Source: `PutEventsRequestEntryList`'s own length
/// (`smithy_query(query="shape",shape="PutEventsRequestEntryList")`, 1..10).
const MAX_EVENTS: usize = 10;

/// The `errorType` of a step whose own request the data source cannot act on, and of one the
/// owning service refused.
const REQUEST_ERROR_TYPE: &str = "Code";
const ACCESS_DENIED_ERROR_TYPE: &str = "EventBridge:AccessDeniedException";

/// A router with AppSync, EventBridge, Amazon SQS and IAM over ONE engine: the bus the data
/// source publishes onto, the queue its rule targets and the documents the publication is
/// authorized against all belong to those services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One created GraphQL API of this suite: the address its `GRAPHQL` endpoint is served on, and
/// a live API key to present to it.
struct Api {
    api_id: String,
    graphql: String,
    api_key: String,
}

#[tokio::test]
async fn every_event_of_a_request_is_published_onto_the_data_sources_bus() {
    let router = emulator();
    let bus = given_bus(&router, "orders").await;
    let queue = given_queue(&router, "orders-queue").await;
    given_rule(&router, &bus, "orders-rule", &queue, MATCHED_SOURCE).await;
    let role = given_role(&router, "appsync-orders", Some(&allowing(&bus))).await;
    let api = create_api(&router, "published", &bus, &role).await;

    /// One row: the events the mutation is handed, the ids it must report, and the details the
    /// rule's queue must receive.
    struct Row<'a> {
        case: &'a str,
        events: Vec<Value>,
        published: usize,
        delivered: Vec<Value>,
    }
    let order = |id: &str| json!({"orderId": id});
    let event = |id: &str, source: &str| {
        json!({"source": source, "detailType": DETAIL_TYPE, "detail": order(id)})
    };
    let rows: Vec<Row> = vec![
        Row {
            case: "one event the rule matches",
            events: vec![event("A-1", MATCHED_SOURCE)],
            published: 1,
            delivered: vec![order("A-1")],
        },
        Row {
            case: "three events, each routed on its own, in the order the request states them",
            events: vec![
                event("B-1", MATCHED_SOURCE),
                event("B-2", MATCHED_SOURCE),
                event("B-3", MATCHED_SOURCE),
            ],
            published: 3,
            delivered: vec![order("B-1"), order("B-2"), order("B-3")],
        },
        Row {
            case: "an event no rule of this bus matches, which is published and routed nowhere",
            events: vec![event("C-1", UNMATCHED_SOURCE)],
            published: 1,
            delivered: Vec::new(),
        },
        Row {
            case: "a batch mixing a matched event with one nothing matches",
            events: vec![
                event("D-1", UNMATCHED_SOURCE),
                event("D-2", MATCHED_SOURCE),
                event("D-3", UNMATCHED_SOURCE),
            ],
            published: 3,
            delivered: vec![order("D-2")],
        },
        Row {
            case: "an event stating the resources and the time it concerns",
            events: vec![json!({
                "source": MATCHED_SOURCE,
                "detailType": DETAIL_TYPE,
                "detail": order("E-1"),
                "resources": ["arn:aws:s3:::a-bucket"],
                "time": "2026-01-02T03:04:05Z",
            })],
            published: 1,
            delivered: vec![order("E-1")],
        },
        Row {
            case: "the most events one publication carries",
            events: (0..MAX_EVENTS)
                .map(|index| event(&format!("F-{index}"), MATCHED_SOURCE))
                .collect(),
            published: MAX_EVENTS,
            delivered: (0..MAX_EVENTS).map(|index| order(&format!("F-{index}"))).collect(),
        },
    ];
    for row in &rows {
        let case = row.case;
        let answered = publish(case, &api, &row.events).await;
        let published = &answered["data"]["publish"];
        assert_eq!(
            answered["errors"].as_array(),
            None,
            "case {case:?}: a publication the data source can make reports no error, got \
             {answered}"
        );
        assert_eq!(
            published["failedCount"],
            json!(0),
            "case {case:?}: expected no failed entry, got {answered}"
        );
        let ids: Vec<String> = published["eventIds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids.len(),
            row.published,
            "case {case:?}: expected one event id per event published, got {answered}"
        );
        assert!(
            ids.iter().all(|id| !id.is_empty()),
            "case {case:?}: every published event carries the id EventBridge minted, got \
             {answered}"
        );
        let delivered = delivered_before_sentinel(case, &router, &bus, &queue, case).await;
        assert_eq!(
            delivered, row.delivered,
            "case {case:?}: expected the rule's queue to receive {:?}, got {delivered:?}",
            row.delivered
        );
    }
}

#[tokio::test]
async fn a_request_this_data_source_cannot_act_on_publishes_none_of_its_events() {
    let router = emulator();
    let bus = given_bus(&router, "refused").await;
    let queue = given_queue(&router, "refused-queue").await;
    given_rule(&router, &bus, "refused-rule", &queue, MATCHED_SOURCE).await;
    let role = given_role(&router, "appsync-refused", Some(&allowing(&bus))).await;
    let api = create_api(&router, "refused", &bus, &role).await;

    let good = json!({
        "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE, "detail": {"orderId": "G-1"},
    });
    let rows: Vec<(&str, Vec<Value>, &str)> = vec![
        (
            "a detail that is not an object, in the second event of the request",
            vec![
                good.clone(),
                json!({
                    "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE,
                    "detail": "not-an-object",
                }),
            ],
            "[events][1][detail]",
        ),
        (
            "a detail that is a list rather than an object",
            vec![json!({
                "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE,
                "detail": [{"orderId": "G-2"}],
            })],
            "[events][0][detail]",
        ),
        (
            "an event that states no detailType",
            vec![json!({"source": MATCHED_SOURCE, "detail": {"orderId": "G-3"}})],
            "[events][0][detailType]",
        ),
        (
            "an event that states no source",
            vec![json!({"detailType": DETAIL_TYPE, "detail": {"orderId": "G-4"}})],
            "[events][0][source]",
        ),
        (
            "a time that is no timestamp",
            vec![json!({
                "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE,
                "detail": {"orderId": "G-5"}, "time": "half past two",
            })],
            "[events][0][time]",
        ),
        (
            "one event more than a publication carries",
            (0..=MAX_EVENTS).map(|_| good.clone()).collect(),
            "10",
        ),
    ];
    for (case, events, located) in &rows {
        let answered = publish(case, &api, events).await;
        let error = &answered["errors"][0];
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(located),
            "case {case:?}: expected the refusal to state {located:?}, got {answered}"
        );
        assert_eq!(
            error["errorType"].as_str(),
            Some(REQUEST_ERROR_TYPE),
            "case {case:?}: expected the error type of a request the data source cannot act \
             on, got {answered}"
        );
        assert_eq!(
            answered["data"]["publish"],
            Value::Null,
            "case {case:?}: a field that did not resolve carries no value, got {answered}"
        );
        let delivered = delivered_before_sentinel(case, &router, &bus, &queue, case).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: a refused request publishes none of its events, got {delivered:?}"
        );
    }
}

#[tokio::test]
async fn the_documents_in_force_on_the_data_sources_role_decide_the_publication() {
    let router = emulator();
    let bus = given_bus(&router, "guarded").await;
    let other = given_bus(&router, "elsewhere").await;
    let queue = given_queue(&router, "guarded-queue").await;
    given_rule(&router, &bus, "guarded-rule", &queue, MATCHED_SOURCE).await;

    /// One row: the document the data source's role carries, and whether the publication is
    /// made under it.
    struct Row<'a> {
        case: &'a str,
        role: &'a str,
        policy: Option<String>,
        published: bool,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a role allowed events:PutEvents on this bus",
            role: "allowed",
            policy: Some(allowing(&bus)),
            published: true,
        },
        Row {
            case: "a role allowed it on every resource",
            role: "wildcard",
            policy: Some(statement("Allow", "events:PutEvents", "*")),
            published: true,
        },
        Row {
            case: "a role whose allow names another bus",
            role: "other-bus",
            policy: Some(allowing(&other)),
            published: false,
        },
        Row {
            case: "a role whose allow names another action",
            role: "other-action",
            policy: Some(statement("Allow", "events:DescribeEventBus", &bus.arn)),
            published: false,
        },
        Row {
            case: "a role whose documents deny it explicitly beside a wildcard allow",
            role: "denied",
            policy: Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Effect\":\"Allow\",\"Action\":\"events:*\",\"Resource\":\"*\"}},\
                 {{\"Effect\":\"Deny\",\"Action\":\"events:PutEvents\",\"Resource\":\"{}\"}}]}}",
                bus.arn
            )),
            published: false,
        },
        Row {
            // A role IAM HOLDS is the authority on what may be done under it, so an action
            // nothing in force on it allows is refused — an EMPTY set of documents allows
            // nothing. (A role IAM holds nothing under is the case that says nothing, since
            // a service role is routinely provisioned out of band.)
            case: "a role carrying no identity-based document at all",
            role: "undocumented",
            policy: None,
            published: false,
        },
    ];
    for row in &rows {
        let case = row.case;
        let role = given_role(&router, &format!("appsync-{}", row.role), row.policy.as_deref()).await;
        let api = create_api(&router, row.role, &bus, &role).await;
        let events = vec![json!({
            "source": MATCHED_SOURCE,
            "detailType": DETAIL_TYPE,
            "detail": {"orderId": row.role},
        })];
        let answered = publish(case, &api, &events).await;
        if row.published {
            assert_eq!(
                answered["data"]["publish"]["failedCount"],
                json!(0),
                "case {case:?}: expected the publication to be made, got {answered}"
            );
            let delivered = delivered_before_sentinel(case, &router, &bus, &queue, case).await;
            assert_eq!(
                delivered,
                vec![json!({"orderId": row.role})],
                "case {case:?}: expected the event on the rule's queue, got {delivered:?}"
            );
            continue;
        }
        let error = &answered["errors"][0];
        let message = error["message"].as_str().unwrap_or_default();
        assert_eq!(
            error["errorType"].as_str(),
            Some(ACCESS_DENIED_ERROR_TYPE),
            "case {case:?}: expected the error type of a refused call, got {answered}"
        );
        // The refusal names the SESSION of the role the data source names, the action it
        // asked for and the bus it asked for.
        let session = format!(
            "arn:aws:sts::{}:assumed-role/appsync-{}/APPSYNC_ASSUME_ROLE",
            ::engine::emulator_account_id(),
            row.role
        );
        for named in [session.as_str(), "events:PutEvents", bus.arn.as_str()] {
            assert!(
                message.contains(named),
                "case {case:?}: expected the refusal to name {named:?}, got {answered}"
            );
        }
        let delivered = delivered_before_sentinel(case, &router, &bus, &queue, case).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: a withheld publication delivers nothing, got {delivered:?}"
        );
    }
}

#[tokio::test]
async fn a_data_source_repointed_to_another_bus_publishes_onto_that_bus() {
    let router = emulator();
    let first = given_bus(&router, "first").await;
    let second = given_bus(&router, "second").await;
    let first_queue = given_queue(&router, "first-queue").await;
    let second_queue = given_queue(&router, "second-queue").await;
    given_rule(&router, &first, "first-rule", &first_queue, MATCHED_SOURCE).await;
    given_rule(&router, &second, "second-rule", &second_queue, MATCHED_SOURCE).await;
    let role = given_role(
        &router,
        "appsync-repointed",
        Some(&statement("Allow", "events:PutEvents", "*")),
    )
    .await;
    let api = create_api(&router, "repointed", &first, &role).await;

    let case = "a mutation before the data source is repointed";
    let events = vec![json!({
        "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE, "detail": {"orderId": "H-1"},
    })];
    let answered = publish(case, &api, &events).await;
    assert_eq!(
        answered["data"]["publish"]["failedCount"],
        json!(0),
        "case {case:?}: expected the publication to be made, got {answered}"
    );
    let delivered = delivered_before_sentinel(case, &router, &first, &first_queue, case).await;
    assert_eq!(
        delivered,
        vec![json!({"orderId": "H-1"})],
        "case {case:?}: expected the first bus's queue to receive it, got {delivered:?}"
    );

    // The data source is REPOINTED at the second bus, which is the only thing that changes.
    let (status, updated) = appsync(
        &router,
        "POST",
        &format!("/v1/apis/{}/datasources/{DATA_SOURCE}", api.api_id),
        Some(json!({
            "type": "AMAZON_EVENTBRIDGE",
            "serviceRoleArn": role,
            "eventBridgeConfig": {"eventBusArn": second.arn},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateDataSource must answer 200, got {updated}"
    );

    let case = "a mutation after the data source is repointed";
    let events = vec![json!({
        "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE, "detail": {"orderId": "H-2"},
    })];
    let answered = publish(case, &api, &events).await;
    assert_eq!(
        answered["data"]["publish"]["failedCount"],
        json!(0),
        "case {case:?}: expected the publication to be made, got {answered}"
    );
    let delivered = delivered_before_sentinel(case, &router, &second, &second_queue, case).await;
    assert_eq!(
        delivered,
        vec![json!({"orderId": "H-2"})],
        "case {case:?}: expected the second bus's queue to receive it, got {delivered:?}"
    );
    let left = delivered_before_sentinel(case, &router, &first, &first_queue, "left").await;
    assert!(
        left.is_empty(),
        "case {case:?}: the bus the data source no longer names receives nothing, got {left:?}"
    );
}

#[tokio::test]
async fn a_data_source_this_endpoint_does_not_call_is_reported_to_the_caller() {
    let router = emulator();
    let bus = given_bus(&router, "uncalled").await;
    let role = given_role(&router, "appsync-uncalled", Some(&allowing(&bus))).await;
    let api = create_api(&router, "uncalled", &bus, &role).await;

    // A data source of a type this endpoint does NOT call, named by two further resolvers —
    // one defined by `code`, one by mapping templates — NEITHER of which reads `ctx.error`.
    // The refusal is not a failure of the data source, because no data source was called at
    // all, so it must reach the client rather than leave the field silently null.
    let (status, created) = appsync(
        &router,
        "POST",
        &format!("/v1/apis/{}/datasources", api.api_id),
        Some(json!({
            "name": UNCALLED_DATA_SOURCE,
            "type": "AMAZON_BEDROCK_RUNTIME",
            "serviceRoleArn": role,
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {created}");
    let definitions: &[(&str, &str, Value)] = &[
        (
            "a resolver defined by code whose response returns the result",
            "publishCoded",
            json!({
                "fieldName": "publishCoded",
                "kind": "UNIT",
                "dataSourceName": UNCALLED_DATA_SOURCE,
                "code": "export function request(ctx) { return {operation: 'PutItem'}; }\n\
                         export function response(ctx) { return ctx.result; }",
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            }),
        ),
        (
            "a resolver defined by templates whose response renders the result",
            "publishTemplated",
            json!({
                "fieldName": "publishTemplated",
                "kind": "UNIT",
                "dataSourceName": UNCALLED_DATA_SOURCE,
                "requestMappingTemplate": "{\"version\": \"2017-02-28\", \"operation\": \"PutItem\"}",
                "responseMappingTemplate": "$util.toJson($ctx.result)",
            }),
        ),
    ];
    for (case, field, resolver) in definitions {
        let (status, created) = appsync(
            &router,
            "POST",
            &format!("/v1/apis/{}/types/Mutation/resolvers", api.api_id),
            Some(resolver.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateResolver must answer 200, got {created}"
        );
        let events = vec![json!({
            "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE, "detail": {"orderId": "I-1"},
        })];
        let answered = published_through(case, &api, field, &events).await;
        let message = answered["errors"][0]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(UNCALLED_DATA_SOURCE),
            "case {case:?}: expected the field to report the data source nothing called, got \
             {answered}"
        );
        assert_eq!(
            answered["data"][field],
            Value::Null,
            "case {case:?}: a field that did not resolve carries no value, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_call_that_failed_is_reported_even_where_the_response_half_ignores_the_error() {
    let router = emulator();
    let bus = given_bus(&router, "unhandled").await;
    // A role whose documents do NOT allow the publication, so the call the data source makes
    // is one the permissions in force refuse: the step's data source WAS called and did not
    // answer, which is the outcome its response half runs over.
    let role = given_role(
        &router,
        "appsync-unhandled",
        Some(&statement("Allow", "events:DescribeEventBus", &bus.arn)),
    )
    .await;
    let api = create_api(&router, "unhandled", &bus, &role).await;

    // Two further resolvers on the SAME data source, neither of whose response halves reads
    // `ctx.error`: one written in code, one in mapping templates. A half that neither raises
    // an error of its own nor appends one leaves the failure unhandled, so the field reports
    // it rather than the value a call that failed never produced.
    let definitions: &[(&str, &str, Value)] = &[
        (
            "a response half written in code that returns the result",
            "publishCoded",
            json!({
                "fieldName": "publishCoded",
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": "export function request(ctx) {\n\
                         return {operation: 'PutEvents', events: JSON.parse(ctx.args.events)};\n\
                         }\n\
                         export function response(ctx) { return ctx.result; }",
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            }),
        ),
        (
            "a response half written in templates that renders the result",
            "publishTemplated",
            json!({
                "fieldName": "publishTemplated",
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate":
                    "{\"version\": \"2018-05-29\", \"operation\": \"PutEvents\", \
                      \"events\": $ctx.args.events}",
                "responseMappingTemplate": "$util.toJson($ctx.result)",
            }),
        ),
    ];
    for (case, field, resolver) in definitions {
        let (status, created) = appsync(
            &router,
            "POST",
            &format!("/v1/apis/{}/types/Mutation/resolvers", api.api_id),
            Some(resolver.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateResolver must answer 200, got {created}"
        );
        let events = vec![json!({
            "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE, "detail": {"orderId": "I-9"},
        })];
        let answered = published_through(case, &api, field, &events).await;
        assert_eq!(
            answered["errors"][0]["errorType"].as_str(),
            Some(ACCESS_DENIED_ERROR_TYPE),
            "case {case:?}: expected the refused call to be reported to the caller, got \
             {answered}"
        );
        assert_eq!(
            answered["data"][field],
            Value::Null,
            "case {case:?}: a field whose call failed carries no value, got {answered}"
        );
    }
}

/// RULE: it is the data source a STEP resolves against that decides what that step does, so a
/// FUNCTION of a `PIPELINE` resolver bound to an `AMAZON_EVENTBRIDGE` data source publishes
/// exactly as a `UNIT` resolver bound to one does — onto that data source's configured bus,
/// under the role it names — whatever the pipeline runs around it.
#[tokio::test]
async fn a_function_of_a_pipeline_publishes_onto_the_data_sources_bus_too() {
    let router = emulator();
    let bus = given_bus(&router, "piped").await;
    let elsewhere = given_bus(&router, "piped-elsewhere").await;
    let queue = given_queue(&router, "piped-queue").await;
    given_rule(&router, &bus, "piped-rule", &queue, MATCHED_SOURCE).await;
    // Two APIs on the SAME bus, differing only in what the data source's role is allowed:
    // the publication onto this bus, and the publication onto another one.
    let allowed = given_role(&router, "appsync-piped-allowed", Some(&allowing(&bus))).await;
    let withheld = given_role(
        &router,
        "appsync-piped-withheld",
        Some(&allowing(&elsewhere)),
    )
    .await;
    let allowed = create_api(&router, "piped-allowed", &bus, &allowed).await;
    let withheld = create_api(&router, "piped-withheld", &bus, &withheld).await;
    for api in [&allowed, &withheld] {
        given_pipelines(&router, &api.api_id).await;
    }

    /// What one row expects of the pipeline it drives.
    enum Outcome {
        /// The publication is made: the details the rule's queue must receive, in order.
        Published(Vec<Value>),
        /// It is not: what the error of the field must state, and the `errorType` it is of.
        Refused(&'static str, &'static str),
    }
    /// One row: the API whose data-source role decides the publication, the field whose
    /// pipeline publishes, the events the mutation is handed and what must come of them.
    struct Row<'a> {
        case: &'a str,
        api: &'a Api,
        field: &'a str,
        events: Vec<Value>,
        outcome: Outcome,
    }
    let order = |id: &str| json!({"orderId": id});
    let event = |id: &str, source: &str| {
        json!({"source": source, "detailType": DETAIL_TYPE, "detail": order(id)})
    };
    let rows: Vec<Row> = vec![
        Row {
            case: "a pipeline whose only function publishes one event",
            api: &allowed,
            field: ONE_STEP_FIELD,
            events: vec![event("P-1", MATCHED_SOURCE)],
            outcome: Outcome::Published(vec![order("P-1")]),
        },
        Row {
            case: "a pipeline whose only function publishes a batch, each event routed on its \
                   own",
            api: &allowed,
            field: ONE_STEP_FIELD,
            events: vec![
                event("P-2", UNMATCHED_SOURCE),
                event("P-3", MATCHED_SOURCE),
                event("P-4", MATCHED_SOURCE),
            ],
            outcome: Outcome::Published(vec![order("P-3"), order("P-4")]),
        },
        Row {
            case: "a publishing function that is the second step of its pipeline",
            api: &allowed,
            field: TWO_STEP_FIELD,
            events: vec![event("P-5", MATCHED_SOURCE)],
            outcome: Outcome::Published(vec![order("P-5")]),
        },
        Row {
            case: "a publishing function the earlier step of its pipeline handed a batch",
            api: &allowed,
            field: TWO_STEP_FIELD,
            events: vec![event("P-6", MATCHED_SOURCE), event("P-7", MATCHED_SOURCE)],
            outcome: Outcome::Published(vec![order("P-6"), order("P-7")]),
        },
        Row {
            case: "a request this data source cannot act on, reached through a pipeline",
            api: &allowed,
            field: ONE_STEP_FIELD,
            events: vec![
                event("P-8", MATCHED_SOURCE),
                json!({
                    "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE,
                    "detail": "not-an-object",
                }),
            ],
            outcome: Outcome::Refused("[events][1][detail]", REQUEST_ERROR_TYPE),
        },
        Row {
            case: "the same, in the second step of its pipeline",
            api: &allowed,
            field: TWO_STEP_FIELD,
            events: vec![json!({
                "source": MATCHED_SOURCE, "detailType": DETAIL_TYPE, "detail": [1, 2],
            })],
            outcome: Outcome::Refused("[events][0][detail]", REQUEST_ERROR_TYPE),
        },
        Row {
            case: "a pipeline function whose data-source role is allowed another bus only",
            api: &withheld,
            field: ONE_STEP_FIELD,
            events: vec![event("P-9", MATCHED_SOURCE)],
            outcome: Outcome::Refused("events:PutEvents", ACCESS_DENIED_ERROR_TYPE),
        },
        Row {
            case: "the same, in the second step of its pipeline",
            api: &withheld,
            field: TWO_STEP_FIELD,
            events: vec![event("P-10", MATCHED_SOURCE)],
            outcome: Outcome::Refused("events:PutEvents", ACCESS_DENIED_ERROR_TYPE),
        },
    ];
    for row in &rows {
        let case = row.case;
        let answered = published_through(case, row.api, row.field, &row.events).await;
        let delivered = delivered_before_sentinel(case, &router, &bus, &queue, case).await;
        match &row.outcome {
            Outcome::Published(expected) => {
                assert_eq!(
                    answered["errors"].as_array(),
                    None,
                    "case {case:?}: a publication the data source can make reports no error, \
                     got {answered}"
                );
                let published = &answered["data"][row.field];
                assert_eq!(
                    published["failedCount"],
                    json!(0),
                    "case {case:?}: expected no failed entry, got {answered}"
                );
                let ids: Vec<String> = published["eventIds"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|id| id.as_str().unwrap_or_default().to_string())
                    .collect();
                assert_eq!(
                    ids.len(),
                    row.events.len(),
                    "case {case:?}: expected one event id per event published, got {answered}"
                );
                assert!(
                    ids.iter().all(|id| !id.is_empty()),
                    "case {case:?}: every published event carries the id EventBridge minted, \
                     got {answered}"
                );
                assert_eq!(
                    &delivered, expected,
                    "case {case:?}: expected the rule's queue to receive {expected:?}, got \
                     {delivered:?}"
                );
            }
            Outcome::Refused(states, error_type) => {
                let error = &answered["errors"][0];
                let message = error["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(states),
                    "case {case:?}: expected the refusal to state {states:?}, got {answered}"
                );
                // The step that did not resolve is the FUNCTION of the pipeline, which the
                // error of the field names.
                assert!(
                    message.contains(PUBLISHER_FUNCTION),
                    "case {case:?}: expected the error to name the step that did not resolve, \
                     got {answered}"
                );
                assert_eq!(
                    error["errorType"].as_str(),
                    Some(*error_type),
                    "case {case:?}: expected the error type of this refusal, got {answered}"
                );
                assert_eq!(
                    answered["data"][row.field],
                    Value::Null,
                    "case {case:?}: a field that did not resolve carries no value, got \
                     {answered}"
                );
                assert!(
                    delivered.is_empty(),
                    "case {case:?}: a publication that was not made delivers nothing, got \
                     {delivered:?}"
                );
            }
        }
    }
}

/// Attach the two `PIPELINE` resolvers the pipeline case drives to `api_id`: one over the
/// publishing function alone, one over a step that stashes the events followed by it.
///
/// The step that stashes resolves against a data source of the `NONE` type ("There is no data
/// source"), so the only step of either pipeline that CALLS anything is the publishing one.
async fn given_pipelines(router: &MultiServiceRouter, api_id: &str) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": STASH_DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateDataSource for the local steps must answer 200, got {created}"
    );
    let publisher = given_function(router, api_id, PUBLISHER_FUNCTION, DATA_SOURCE, PUBLISHER_CODE)
        .await;
    let stasher =
        given_function(router, api_id, STASHER_FUNCTION, STASH_DATA_SOURCE, STASHER_CODE).await;
    for (field, functions) in [
        (ONE_STEP_FIELD, vec![publisher.clone()]),
        (TWO_STEP_FIELD, vec![stasher, publisher.clone()]),
    ] {
        let (status, resolver) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "PIPELINE",
                "pipelineConfig": {"functions": functions},
                "code": PIPELINE_CODE,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {resolver}"
        );
    }
}

/// Create one pipeline function on `data_source`, defined by `code`, and answer the id a
/// pipeline invokes it by.
async fn given_function(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    data_source: &str,
    code: &str,
) -> String {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(json!({
            "name": name,
            "dataSourceName": data_source,
            "code": code,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateFunction for {name} must answer 200, got {created}"
    );
    created["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction answers the function's id, got {created}"))
        .to_string()
}

/// POST one mutation publishing `events` to the API's own endpoint, as a client does, and
/// answer the GraphQL document it was served.
async fn publish(case: &str, api: &Api, events: &[Value]) -> Value {
    published_through(case, api, "publish", events).await
}

/// The same, naming the mutation to publish THROUGH — the seam the case about a data source
/// this endpoint does not call varies.
async fn published_through(case: &str, api: &Api, field: &str, events: &[Value]) -> Value {
    let document = json!({
        "query": format!(
            "mutation Publish($events: AWSJSON!) {{ \
             {field}(events: $events) {{ failedCount eventIds }} }}"
        ),
        "variables": {"events": Value::Array(events.to_vec()).to_string()},
    });
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &document,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    assert_eq!(
        status, 200,
        "case {case:?}: a GraphQL request is answered 200, got {status} and {answered}"
    );
    answered
}

/// One event bus of this suite: its name and the ARN a data source names it by.
struct Bus {
    name: String,
    arn: String,
}

/// Create an event bus called `name` through EventBridge's own `CreateEventBus`.
async fn given_bus(router: &MultiServiceRouter, name: &str) -> Bus {
    let (status, created) = events(router, "CreateEventBus", &json!({"Name": name})).await;
    assert_eq!(
        status, 200,
        "CreateEventBus for {name} must answer 200, got {created}"
    );
    Bus {
        name: name.to_string(),
        arn: created["EventBusArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateEventBus answers the bus ARN, got {created}"))
            .to_string(),
    }
}

/// One queue of this suite: the url a reader receives from, and the ARN a rule targets.
struct Queue {
    url: String,
    arn: String,
}

/// Create an Amazon SQS queue called `name`, carrying the policy a rule's target needs: "for
/// Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
/// resource-based policies" (`PutTargets`), so a queue that granted the service nothing
/// receives nothing.
async fn given_queue(router: &MultiServiceRouter, name: &str) -> Queue {
    let (status, created) = sqs(router, "CreateQueue", &json!({"QueueName": name})).await;
    assert_eq!(
        status, 200,
        "CreateQueue for {name} must answer 200, got {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue answers a QueueUrl, got {created}"))
        .to_string();
    let (status, attributes) = sqs(
        router,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetQueueAttributes for {name} must answer 200, got {attributes}"
    );
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes answers the QueueArn, got {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "events.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    })
    .to_string();
    let (status, set) = sqs(
        router,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "SetQueueAttributes for {name} must answer 200, got {set}"
    );
    Queue { url, arn }
}

/// Put a rule on `bus` matching `source`, with `queue` as its only target.
async fn given_rule(
    router: &MultiServiceRouter,
    bus: &Bus,
    name: &str,
    queue: &Queue,
    source: &str,
) {
    let (status, put) = events(
        router,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus.name,
            "State": "ENABLED",
            "EventPattern": json!({"source": [source]}).to_string(),
        }),
    )
    .await;
    assert_eq!(status, 200, "PutRule for {name} must answer 200, got {put}");
    let (status, targeted) = events(
        router,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": bus.name,
            "Targets": [{"Id": "queue", "Arn": queue.arn}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutTargets for {name} must answer 200, got {targeted}"
    );
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "PutTargets for {name} must accept the target, got {targeted}"
    );
}

/// Create an IAM role called `role_name` carrying `policy` as its inline policy (when one is
/// given), through IAM's OWN operations, and answer its ARN.
async fn given_role(
    router: &MultiServiceRouter,
    role_name: &str,
    policy: Option<&str>,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "appsync.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role's Arn, got {created}"));
    if let Some(policy) = policy {
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }
    arn
}

/// The document allowing `events:PutEvents` on one bus and nothing else.
fn allowing(bus: &Bus) -> String {
    statement("Allow", "events:PutEvents", &bus.arn)
}

/// One-statement identity-based document.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": action, "Resource": resource}],
    })
    .to_string()
}

/// Create one GraphQL API with an API key, [`SCHEMA`], an `AMAZON_EVENTBRIDGE` data source on
/// `bus` under `role`, and the resolver attached to the mutation; answer the address its
/// `GRAPHQL` endpoint is served on.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    bus: &Bus,
    role_arn: &str,
) -> Api {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("eventbridge-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    create_schema(router, &api_id).await;
    let (status, data_source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "AMAZON_EVENTBRIDGE",
            "serviceRoleArn": role_arn,
            "eventBridgeConfig": {"eventBusArn": bus.arn},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateDataSource must answer 200, got {data_source}"
    );
    let (status, resolver) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
        Some(json!({
            "fieldName": "publish",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": RESOLVER_CODE,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateResolver must answer 200, got {resolver}");
    Api {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Install [`SCHEMA`] on `api_id`, waiting for the clock to perform the creation.
async fn create_schema(router: &MultiServiceRouter, api_id: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(SCHEMA);
    let (status, started) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": definition})),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSchemaCreation must answer 200, got {started}"
    );
    let deadline = tokio::time::Instant::now() + CREATION_DEADLINE;
    loop {
        let (status, settled) = appsync(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSchemaCreationStatus must answer 200, got {settled}"
        );
        if settled["status"].as_str() == Some("SUCCESS") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the schema creation must be performed, got {settled}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Everything the queue at `queue` received, read only once a SENTINEL event published
/// straight through EventBridge onto `bus` has arrived on it: a negative read anchored on an
/// observed positive, so "nothing was delivered" cannot pass by outrunning a delivery that was
/// in fact made. The sentinel itself is the last entry and is dropped, so what is answered is
/// everything else the queue received.
async fn delivered_before_sentinel(
    case: &str,
    router: &MultiServiceRouter,
    bus: &Bus,
    queue: &Queue,
    label: &str,
) -> Vec<Value> {
    let sentinel = json!({"sentinel": label});
    let (status, put) = events(
        router,
        "PutEvents",
        &json!({"Entries": [{
            "Source": MATCHED_SOURCE,
            "DetailType": DETAIL_TYPE,
            "Detail": sentinel.to_string(),
            "EventBusName": bus.name,
        }]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the sentinel's PutEvents must answer 200, got {put}"
    );
    assert_eq!(
        put["FailedEntryCount"],
        json!(0),
        "case {case:?}: the sentinel must be published, got {put}"
    );
    let mut delivered: Vec<Value> = Vec::new();
    let deadline = tokio::time::Instant::now() + DELIVERY_DEADLINE;
    loop {
        delivered.extend(received_details(case, router, &queue.url).await);
        if let Some(at) = delivered.iter().position(|detail| detail == &sentinel) {
            delivered.remove(at);
            return delivered;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the sentinel published onto this bus must reach the rule's queue, \
             got {delivered:?}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// The `detail` of every event ONE read of the queue at `url` reports.
async fn received_details(case: &str, router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let (status, received) = sqs(
        router,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage must answer 200, got {received}"
    );
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body: Value = message["Body"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            body["detail"].clone()
        })
        .collect()
}

/// One EventBridge call, over the wire that service speaks (`awsJson1_1`, `POST /` with
/// `X-Amz-Target: AWSEvents.<Operation>`).
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "events", "AWSEvents.1.1", operation, body).await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`).
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "sqs", "AmazonSQS.1.0", operation, body).await
}

/// One `awsJson` RPC call against the service whose signing name is `signing_name` — the
/// credential scope is what routes it. `target` is the `<prefix>.<json version>` the service
/// speaks.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (prefix, version) = target
        .split_once('.')
        .expect("the target names a prefix and a JSON version");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{version}"))
        .header("x-amz-target", format!("{prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One awsQuery call to IAM, signed with this suite's own credential; answers the XML body,
/// having insisted the call succeeded.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "the IAM {action} call of this suite must succeed, got {status} with {text}"
    );
    text
}
