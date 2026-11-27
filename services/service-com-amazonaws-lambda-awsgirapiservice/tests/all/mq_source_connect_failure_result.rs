// SPDX-License-Identifier: CC0-1.0

//! What an event source mapping on an Amazon MQ broker REPORTS when it cannot consume its
//! source: "the result of the event source mapping's last processing attempt"
//! (`EventSourceMappingConfiguration$LastProcessingResult`).
//!
//! RULE: a mapping whose Amazon MQ source cannot be consumed stays `Enabled` and reports, as
//! its `LastProcessingResult`, the ONE fixed sentence Lambda words a broker source's failure
//! to be connected to in — the same text however the source fails, with nothing of the
//! request or of the emulator's state written into it — and it keeps reporting exactly that
//! for as long as the fault lasts, through both `GetEventSourceMapping` and
//! `ListEventSourceMappings`. A mapping whose polling is paused has made no pass at all, so it
//! reports no result.
//!
//! The rows differ along what the rule generalises over: HOW the source fails to be consumable
//! (a running broker holding no queue of that name, a broker still being provisioned, a broker
//! that has been deleted under the mapping), the queue name the mapping consumes, the virtual
//! host it consumes in, the broker it names, and whether the mapping polls at all. The readings
//! of every failing row are compared with each other as well as with the expected sentence,
//! which is what holds the text to carrying NO variable part: three rows differing in queue,
//! virtual host and broker report one identical string.
//!
//! Every row drives real HTTP requests through the router: the brokers are created and deleted
//! through Amazon MQ's own `CreateBroker`/`DeleteBroker`, and the mappings read back through
//! Lambda's own `GetEventSourceMapping` and `ListEventSourceMappings`. Nothing here asserts on
//! elapsed time: each wait looks again until the condition holds or a generous deadline passes.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED, POLL_INTERVAL,
};
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::Instant;

use crate::event_source_mappings::{
    await_function_active, await_mapping_state, create_function, create_mapping, create_request,
    get_mapping, list_mappings, mq_call, router_on, LOOK_AGAIN, STATE_DISABLED, STATE_ENABLED,
};

/// How long a row waits for work no caller controls — a broker reaching `RUNNING`, a function
/// going `Active`, a mapping settling, a poller making its first pass. Generous: a row fails
/// only when the work did not happen at all, so a loaded host cannot decide the outcome.
const BUDGET: Duration = Duration::from_secs(60);

/// How many further looks a row takes at a reading it has already seen, to hold it to being
/// the reading the mapping KEEPS reporting rather than one pass's answer. Each look is a span
/// of one poller pass apart, so the mapping is read across several passes of it.
const FURTHER_LOOKS: usize = 4;

/// The user every broker below is created with, and that user's password.
const BROKER_USER: &str = "mq-connect-user";
/// See [`BROKER_USER`].
const BROKER_PASSWORD: &str = "Str0ng-Connect-Pass";

/// How the source of a row fails to be consumable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// A broker Amazon MQ reports `RUNNING`, which holds no queue of the name the mapping
    /// consumes: nothing ever declared it.
    RunningWithoutTheQueue,
    /// A broker that is still being provisioned when the mapping is created, so it answers no
    /// client on its endpoints yet.
    StillProvisioning,
    /// A broker that was deleted after the mapping was created, so there is nothing left to
    /// connect to.
    Deleted,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The name Amazon MQ creates this row's broker under, which is one of the values the
    /// reading must carry no part of.
    broker: &'static str,
    /// How this row's source fails to be consumable.
    source: Source,
    /// The one `Queues` entry the mapping consumes, which no broker below declares.
    queue: &'static str,
    /// The virtual host the mapping consumes in, stated through its
    /// `SourceAccessConfigurations`, and `None` for a mapping that states none.
    virtual_host: Option<&'static str>,
    /// `Enabled`, as the create states it.
    enabled: bool,
    /// The `LastProcessingResult` the mapping must settle on, and `None` for a mapping that
    /// must report none at all.
    expected: Option<&'static str>,
}

/// The rows: one per way an Amazon MQ source fails to be consumable, plus the paused control.
const ROWS: &[Row] = &[
    // The recorded shape: a broker that is RUNNING, whose queue was never declared.
    Row {
        case: "a-running-broker-holding-no-queue-of-that-name",
        broker: "mq-connect-running",
        source: Source::RunningWithoutTheQueue,
        queue: "esm-never-declared",
        virtual_host: None,
        enabled: true,
        expected: Some(LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED),
    },
    // The same failure reached another way, and with another queue and virtual host stated: a
    // broker that answers no client yet.
    Row {
        case: "a-broker-that-is-still-being-provisioned",
        broker: "mq_connect_provisioning",
        source: Source::StillProvisioning,
        queue: "orders-inbound",
        virtual_host: Some("/"),
        enabled: true,
        expected: Some(LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED),
    },
    // And the source going away under a mapping that was wired to it.
    Row {
        case: "a-broker-deleted-under-the-mapping",
        broker: "mq-connect-deleted-42",
        source: Source::Deleted,
        queue: "payments.settled",
        virtual_host: Some("/orders"),
        enabled: true,
        expected: Some(LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED),
    },
    // The control: "Lambda pauses polling and invocation", so this mapping makes no pass over
    // the same unconsumable source and has no result of one to report — which is what tells a
    // reported failure apart from a mapping nothing ever polled.
    Row {
        case: "polling-paused-on-the-same-unconsumable-source",
        broker: "mq-connect-paused",
        source: Source::RunningWithoutTheQueue,
        queue: "esm-never-declared",
        virtual_host: None,
        enabled: false,
        expected: None,
    },
];

/// RULE: a mapping whose Amazon MQ source cannot be consumed stays `Enabled` and reports the
/// one fixed sentence Lambda words a broker source's connect failure in — identical across
/// every way the source fails and whatever queue, virtual host and broker the mapping names —
/// for as long as the fault lasts, through the get and the listing alike; a paused mapping
/// reports no result at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unconsumable_broker_source_reports_the_connect_failure() {
    const FUNCTION: &str = "mq-connect-failure-consumer";

    // The readings the FAILING rows settled on, which must be one and the same string even
    // though those rows differ in queue, virtual host, broker and the way the source fails.
    let mut readings: Vec<(&str, String)> = Vec::new();

    for row in ROWS {
        let case = row.case;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, FUNCTION).await;
        // "Lambda reads items from the event source and invokes the function": a pass is made
        // only once the function it would invoke is there to invoke.
        await_function_active(&router, case, FUNCTION).await;

        let (broker_arn, broker_id) = rabbitmq_broker(&router, case, row.broker).await;
        if row.source != Source::StillProvisioning {
            await_broker_running(&router, case, &broker_id).await;
        }

        let mut extras = json!({ "Queues": [row.queue], "BatchSize": 1, "Enabled": row.enabled });
        if let Some(host) = row.virtual_host {
            extras["SourceAccessConfigurations"] =
                json!([{ "Type": "VIRTUAL_HOST", "URI": host }]);
        }
        let created = create_mapping(
            &router,
            &create_request(FUNCTION, Some(&broker_arn), &extras),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created.string("UUID").unwrap_or_else(|| {
            panic!(
                "case {case:?}: CreateEventSourceMapping must answer a UUID, got {}",
                created.text()
            )
        });

        if row.source == Source::Deleted {
            delete_broker(&router, case, &broker_id).await;
        }

        let settled = if row.enabled {
            STATE_ENABLED
        } else {
            STATE_DISABLED
        };
        await_mapping_state(&router, case, &uuid, settled).await;

        let Some(expected) = row.expected else {
            // The paused control: no pass is made, so no result is reported — asserted across
            // several spans of a poller pass, not at one instant of this test's choosing.
            for look in 0..=FURTHER_LOOKS {
                let reading = get_mapping(&router, &uuid).await;
                let reported = reading.string("LastProcessingResult");
                assert_eq!(
                    reported, None,
                    "case {case:?}: look {look}: a paused mapping makes no pass, so it must \
                     report no LastProcessingResult, got {reported:?}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            continue;
        };

        let reported = result_until_reported(&router, case, &uuid).await;
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: a mapping that cannot consume its broker source must report the \
             connect failure, got {reported:?}"
        );

        // It is the reading the mapping KEEPS reporting while the fault lasts, and the mapping
        // stays the one the create accepted: looked at again across several poller passes.
        for look in 0..FURTHER_LOOKS {
            tokio::time::sleep(POLL_INTERVAL).await;
            let reading = get_mapping(&router, &uuid).await;
            let again = reading.string("LastProcessingResult");
            assert_eq!(
                again.as_deref(),
                Some(expected),
                "case {case:?}: look {look}: the reading must be unchanged while the fault \
                 lasts, got {again:?}"
            );
            let state = reading.string("State");
            assert_eq!(
                state.as_deref(),
                Some(STATE_ENABLED),
                "case {case:?}: look {look}: the mapping keeps polling while it reports the \
                 failure, got {state:?}"
            );
        }

        // The listing reports the same reading as the get: both read the one record.
        let listed = list_mappings(&router, &format!("EventSourceArn={broker_arn}")).await;
        let listed_result = listed
            .mappings()
            .iter()
            .find(|mapping| mapping.get("UUID").and_then(Value::as_str) == Some(uuid.as_str()))
            .and_then(|mapping| mapping.get("LastProcessingResult"))
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            listed_result.as_deref(),
            Some(expected),
            "case {case:?}: ListEventSourceMappings must report the same reading as the get, \
             got {}",
            listed.text()
        );

        // Nothing of the request or of the emulator's state is written into the sentence.
        let text = reported.unwrap_or_default();
        for value in [
            row.queue,
            row.broker,
            broker_arn.as_str(),
            broker_id.as_str(),
            FUNCTION,
        ] {
            assert!(
                !text.contains(value),
                "case {case:?}: the reading is fixed text, so it must not carry {value:?}, got \
                 {text:?}"
            );
        }
        readings.push((case, text));
    }

    // One sentence for every way the source failed: the rows differ in queue, virtual host,
    // broker and failure, and their readings are byte for byte the same.
    let Some((first_case, first)) = readings.first() else {
        panic!("the table must drive at least one failing row");
    };
    for (case, text) in &readings {
        assert_eq!(
            text, first,
            "case {case:?}: every unconsumable broker source reports the same sentence as \
             {first_case:?} ({first:?}), got {text:?}"
        );
    }
}

/// Look at the mapping `uuid` names until it reports a `LastProcessingResult`, answering what
/// it reported, or `None` when it never reported one inside [`BUDGET`].
///
/// Waits for the CONDITION: what a pass came to is recorded once that pass has been made,
/// which is a span no caller controls.
async fn result_until_reported(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
) -> Option<String> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let reading = get_mapping(router, uuid).await;
        assert_eq!(
            reading.status, 200,
            "case {case:?}: expected GetEventSourceMapping to answer 200, got {} {}",
            reading.status,
            reading.text()
        );
        if let Some(result) = reading.string("LastProcessingResult") {
            return Some(result);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create one RabbitMQ broker with one user through Amazon MQ's own `CreateBroker`, and answer
/// `(brokerArn, brokerId)`.
async fn rabbitmq_broker(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    let body = json!({
        "brokerName": name,
        "engineType": "RABBITMQ",
        "engineVersion": "3.13",
        "hostInstanceType": "mq.m5.large",
        "deploymentMode": "SINGLE_INSTANCE",
        "publiclyAccessible": false,
        "users": [{ "username": BROKER_USER, "password": BROKER_PASSWORD }],
    });
    let created = mq_call(router, "POST", "/v1/brokers", Some(&body)).await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: expected CreateBroker to answer 200, got {} {}",
        created.status,
        created.text()
    );
    let arn = created
        .string("brokerArn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateBroker answered no brokerArn"));
    let id = created
        .string("brokerId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateBroker answered no brokerId"));
    (arn, id)
}

/// Look at the broker `broker_id` names through Amazon MQ's own `DescribeBroker` until it
/// reports `RUNNING`, which is the state in which it answers clients on its endpoints.
async fn await_broker_running(router: &MultiServiceRouter, case: &str, broker_id: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = mq_call(router, "GET", &format!("/v1/brokers/{broker_id}"), None).await;
        if described.string("brokerState").as_deref() == Some("RUNNING") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the broker never reached RUNNING, last answer {}",
            described.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Delete the broker `broker_id` names through Amazon MQ's own `DeleteBroker`, and hold it to
/// being gone from that service's own read of it.
async fn delete_broker(router: &MultiServiceRouter, case: &str, broker_id: &str) {
    let deleted = mq_call(router, "DELETE", &format!("/v1/brokers/{broker_id}"), None).await;
    assert_eq!(
        deleted.status,
        200,
        "case {case:?}: expected DeleteBroker to answer 200, got {} {}",
        deleted.status,
        deleted.text()
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = mq_call(router, "GET", &format!("/v1/brokers/{broker_id}"), None).await;
        if described.status == 404 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: Amazon MQ must stop reporting the deleted broker, last answer {} {}",
            described.status,
            described.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
