// SPDX-License-Identifier: CC0-1.0

//! How Lambda WORDS the refusal of a mapping whose function's execution role may not make
//! one of the calls its poller makes on the event source the request names.
//!
//! RULE: where the reads of an event source are plain calls on the named resource, a call
//! that wires a function to such a source and finds the policies in force on that function's
//! execution role withholding one of them is refused with the modelled
//! `InvalidParameterValueException` (HTTP 400), persists nothing and takes nothing out of the
//! source, and the refusal is AWS's fixed sentence for that class — "The function execution
//! role does not have permissions to call `<Call>` on `<Service>`" — whose only varying parts
//! are the call the poller may not make and the service it would have been made to; the
//! sentence names neither the role, nor the function, nor the source.
//!
//! The rows differ along what the rule generalises over: WHICH source service the mapping
//! would read (a queue, a cluster, a broker, so the sentence's service and call both change),
//! WHICH of that source's reads is the one withheld (each of the three a queue takes in turn),
//! and HOW it comes to be withheld (granted to nothing, granted short of the set, granted on
//! another resource of the same kind, granted and then explicitly denied). The served rows
//! beside them are the control: the sentence is the answer to a withheld read and not to the
//! wiring call, so a role granted what its poller makes gets its mapping.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! mounted beside Lambda on ONE engine, so the gate reads the documents IAM itself persisted,
//! and reads the outcome back through `GetEventSourceMapping` and `ListEventSourceMappings`.
//! The last test adds Amazon SQS to the same engine and sends a message to a real queue, so
//! what a refused wiring left in the source is read out of the service that owns it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{
    allow, assert_refused, call_refusal, create_function_as, given_role, logs_only,
    router_with_iam, QUEUE_READS,
};
use crate::event_source_mappings::{
    create_mapping, create_queue, create_request, get_mapping, list_mappings, queue_held,
    router_on, send_message, MSK_SOURCE, QUEUE_SOURCE,
};

/// Another queue of the same account, for the row whose grant names the wrong resource.
const OTHER_QUEUE_SOURCE: &str = "arn:aws:sqs:us-east-1:000000000000:order-overflow";

/// An Amazon MQ broker, the third kind of source whose reads are plain calls on the named
/// resource.
const MQ_SOURCE: &str = "arn:aws:mq:us-east-1:000000000000:broker:order-broker:b-1111";

/// The call a reader of an Amazon MSK cluster makes on the cluster itself before it can
/// connect at all. Source: the cluster read AWS's own managed execution-role policy for that
/// event source (`AWSLambdaMSKExecutionRole`) grants.
const MSK_READ: &str = "DescribeClusterV2";

/// The same, for an Amazon MQ broker: Lambda reads the broker's endpoints first.
const MQ_READ: &str = "DescribeBroker";

/// Which kind of source a row's mapping would poll: the service whose name the sentence
/// carries, the IAM namespace its reads are named in, and the members a mapping onto one
/// needs beside `EventSourceArn`.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// An Amazon SQS queue, read by `ReceiveMessage`, `DeleteMessage` and
    /// `GetQueueAttributes`.
    Queue,
    /// An Amazon MSK cluster, whose topics a mapping onto one names.
    Cluster,
    /// An Amazon MQ broker, whose queues a mapping onto one names.
    Broker,
}

impl Source {
    /// The ARN of this row's source.
    fn arn(self) -> &'static str {
        match self {
            Self::Queue => QUEUE_SOURCE,
            Self::Cluster => MSK_SOURCE,
            Self::Broker => MQ_SOURCE,
        }
    }

    /// The members a `CreateEventSourceMapping` onto this kind of source needs beside the
    /// source itself.
    fn extras(self) -> Value {
        match self {
            Self::Queue => json!({}),
            Self::Cluster => json!({ "Topics": ["order-events"] }),
            Self::Broker => json!({ "Queues": ["order-jobs"] }),
        }
    }
}

/// One row: the source a mapping would poll, the statements of the role's one inline policy,
/// and the `(call, service)` the refusal has to name — `None` for a row that must be served.
struct Row {
    case: &'static str,
    source: Source,
    statements: Value,
    withheld: Option<(&'static str, &'static str)>,
}

/// The catalog: for each source service, which of its reads the role is granted, how the
/// grant names them, and the sentence that follows.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: a role carrying only the log writes, so the FIRST read the
            // poller makes is the one the sentence names.
            case: "a queue, with no queue call granted at all",
            source: Source::Queue,
            statements: json!([logs_only()]),
            withheld: Some((QUEUE_READS[0], "SQS")),
        },
        Row {
            case: "a queue, with only the receive granted",
            source: Source::Queue,
            statements: json!([logs_only(), allow("sqs", &[QUEUE_READS[0]], QUEUE_SOURCE)]),
            withheld: Some((QUEUE_READS[1], "SQS")),
        },
        Row {
            case: "a queue, with the receive and the delete granted",
            source: Source::Queue,
            statements: json!([
                logs_only(),
                allow("sqs", &[QUEUE_READS[0], QUEUE_READS[1]], QUEUE_SOURCE)
            ]),
            withheld: Some((QUEUE_READS[2], "SQS")),
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over the grant beside it, and
            // the sentence names the call it took away.
            case: "a queue, with every call granted and the delete denied",
            source: Source::Queue,
            statements: json!([
                allow("sqs", &QUEUE_READS, QUEUE_SOURCE),
                {
                    "Effect": "Deny",
                    "Action": "sqs:DeleteMessage",
                    "Resource": QUEUE_SOURCE,
                },
            ]),
            withheld: Some((QUEUE_READS[1], "SQS")),
        },
        Row {
            // The grant has to name the source the mapping polls: another queue's ARN says
            // nothing about this one.
            case: "a queue, with every call granted on another queue",
            source: Source::Queue,
            statements: json!([allow("sqs", &QUEUE_READS, OTHER_QUEUE_SOURCE)]),
            withheld: Some((QUEUE_READS[0], "SQS")),
        },
        Row {
            case: "a queue, with every call granted on it",
            source: Source::Queue,
            statements: json!([logs_only(), allow("sqs", &QUEUE_READS, QUEUE_SOURCE)]),
            withheld: None,
        },
        Row {
            case: "an msk cluster, with no cluster call granted at all",
            source: Source::Cluster,
            statements: json!([logs_only()]),
            withheld: Some((MSK_READ, "MSK")),
        },
        Row {
            case: "an msk cluster, with the cluster describe granted on it",
            source: Source::Cluster,
            statements: json!([logs_only(), allow("kafka", &[MSK_READ], MSK_SOURCE)]),
            withheld: None,
        },
        Row {
            case: "an mq broker, with no broker call granted at all",
            source: Source::Broker,
            statements: json!([logs_only()]),
            withheld: Some((MQ_READ, "MQ")),
        },
        Row {
            case: "an mq broker, with the broker describe granted on it",
            source: Source::Broker,
            statements: json!([logs_only(), allow("mq", &[MQ_READ], MQ_SOURCE)]),
            withheld: None,
        },
    ]
}

/// Assert `message` is fixed text: it carries nothing the request named, because the only
/// parts of this sentence that vary are the call and the service.
fn assert_names_nothing_of_the_request(case: &str, message: &str, named: &[&str]) {
    for value in named {
        assert!(
            !message.contains(value),
            "case {case:?}: the sentence is fixed text and must not name {value:?}, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_withheld_source_read_is_refused_with_awss_fixed_sentence() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_iam();
        let role = given_role(
            &router,
            case,
            &format!("esm-sentence-{index}"),
            Some(row.statements.clone()),
        )
        .await;
        let function = format!("esm-sentence-fn-{index}");
        create_function_as(&router, case, &function, &role).await;

        let source = row.source.arn();
        let answer = create_mapping(
            &router,
            &create_request(&function, Some(source), &row.source.extras()),
        )
        .await;

        let Some((call, service)) = row.withheld else {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: expected HTTP 202, got {} {}",
                answer.status,
                answer.text()
            );
            let uuid = answer
                .string("UUID")
                .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", answer.text()));
            let reading = get_mapping(&router, &uuid).await;
            assert_eq!(
                (reading.status, reading.string("EventSourceArn")),
                (200, Some(source.to_string())),
                "case {case:?}: the mapping must read back on {source}, got {} {}",
                reading.status,
                reading.text()
            );
            continue;
        };

        let expected = call_refusal(call, service);
        assert_refused(case, &answer, &expected);
        let message = answer
            .string("message")
            .or_else(|| answer.string("Message"))
            .unwrap_or_default();
        assert_names_nothing_of_the_request(case, &message, &[&role, &function, source]);
        // The refusal is instead of the mapping, not beside it.
        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        assert_eq!(
            (listed.status, listed.listed_uuids()),
            (200, Vec::new()),
            "case {case:?}: a refused create must leave no mapping behind, got {} {}",
            listed.status,
            listed.text()
        );
    }
}

/// Lambda and IAM over the same engine as Amazon SQS, so the source a refused wiring named is
/// a real queue read back out of the service that owns it.
fn router_with_iam_and_queues() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

#[tokio::test]
async fn a_refused_wiring_leaves_the_queues_messages_where_they_were() {
    let case = "a real queue whose message nothing was wired to consume";
    let router = router_with_iam_and_queues();
    let (url, arn) = create_queue(&router, case, "esm-sentence-untouched", &json!({})).await;
    send_message(&router, case, &url, "order-1", None).await;

    let role = given_role(
        &router,
        case,
        "esm-sentence-blind",
        Some(json!([logs_only()])),
    )
    .await;
    create_function_as(&router, case, "esm-sentence-blind-fn", &role).await;

    let answer = create_mapping(
        &router,
        &create_request("esm-sentence-blind-fn", Some(&arn), &json!({})),
    )
    .await;
    assert_refused(case, &answer, &call_refusal(QUEUE_READS[0], "SQS"));

    let listed = list_mappings(&router, "FunctionName=esm-sentence-blind-fn").await;
    assert_eq!(
        (listed.status, listed.listed_uuids()),
        (200, Vec::new()),
        "case {case:?}: a refused create must leave no mapping behind, got {} {}",
        listed.status,
        listed.text()
    );
    let held = queue_held(&router, case, &url).await;
    assert_eq!(
        held, 1,
        "case {case:?}: no mapping exists to consume the message, so the queue must still \
         hold it, got {held}"
    );
}
