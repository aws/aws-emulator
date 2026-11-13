// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `InvokeAssistant` (`POST /assistant/invocation`), driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire a real client's event-stream decoder sees.
//!
//! THE RULES UNDER TEST:
//!
//! * one turn is SERVED as an `application/vnd.amazon.eventstream` body of the modelled
//!   `ResponseStream` events — the `output` event carrying the answer, preceded by a `trace`
//!   event per step only when `enableTrace` asked for them — with the conversation the turn
//!   was recorded under in the modelled response header;
//! * the conversation is STATE: a request naming none starts one under a minted id, a
//!   request naming one continues it, and the turn the answer reports is the count that
//!   conversation has taken;
//! * the answer is RETRIEVED from the datasets this account holds: a dataset whose id, name
//!   or description carries a term the message names is cited — the datasets carrying more
//!   of those terms first — with the dataset's ARN, the source its text was read through and
//!   the text itself, and a message naming nothing the account holds cites nothing. No
//!   language model runs in this emulator, so the answer is never the submitted message
//!   handed back;
//! * a request that breaks one of the model's own rules is refused with the modelled
//!   `InvalidRequestException` (HTTP 400) before any of the stream is framed.
//!
//! The tables span what those rules generalise over: whether the request names a
//! conversation, whether it asks for the trace, the length of the message (the shortest and
//! the longest the model admits), which stored field of a dataset the message matches
//! (its name or its description), how many datasets match and under which source they are
//! cited (a Kendra knowledge base or the dataset's own ARN), and which member of the request
//! is out of bounds.
//!
//! The HTTP harness is this crate's own (`crate::asset_lifecycle`), except for the one
//! dispatch below: the shared `call` parses every response body as JSON, which an
//! event-stream body is not, and the shared dataset helper creates no description and no
//! Kendra source, which is what this suite's retrieval rows differ along.

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, MAX_ASSISTANT_CITATIONS, MAX_ASSISTANT_QUERY_TERMS,
};

use crate::asset_lifecycle::{call, is_modelled_id, REGION};

/// The content type of an event-stream body, in both directions.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The route the model binds this operation to.
const INVOCATION: &str = "/assistant/invocation";

/// The response header the model binds `conversationId` to.
const CONVERSATION_HEADER: &str = "x-amz-iotsitewise-assistant-conversation-id";

/// The `ResponseStream` members a served turn carries.
const TRACE_EVENT: &str = "trace";
const OUTPUT_EVENT: &str = "output";

/// The error code IoT SiteWise refuses a request that breaks the model's rules with.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// One answered call: what the wire carried back.
struct Served {
    status: u16,
    content_type: String,
    /// The modelled conversation header, or `""` when the response carried none.
    conversation_id: String,
    body: Bytes,
}

impl Served {
    /// The response body as JSON — for a REFUSED call, whose body is the error envelope.
    fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected a JSON error body, got {:?} ({error})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code of a refused call.
    fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The refusal sentence of a refused call.
    fn message(&self, case: &str) -> String {
        self.json(case)["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// One `InvokeAssistant` call, with the raw body and headers the response carried.
async fn invoke(router: &MultiServiceRouter, body: Value) -> Served {
    let payload = Bytes::from(serde_json::to_vec(&body).expect("the test body serializes"));
    let request = http::Request::builder()
        .method("POST")
        .uri(INVOCATION)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iotsitewise/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    Served {
        status: response.status().as_u16(),
        content_type: header("content-type"),
        conversation_id: header(CONVERSATION_HEADER),
        body: response.into_body(),
    }
}

/// A request body for `message`, optionally continuing `conversation` and optionally asking
/// for the trace.
fn turn(message: &str, conversation: Option<&str>, enable_trace: bool) -> Value {
    let mut body = json!({ "message": message });
    let object = body.as_object_mut().expect("an object body");
    if let Some(conversation) = conversation {
        object.insert("conversationId".to_string(), json!(conversation));
    }
    if enable_trace {
        object.insert("enableTrace".to_string(), json!(true));
    }
    body
}

/// Decode an event-stream body, asserting the frame headers an SDK decoder requires. Each
/// entry is `(event type, payload)`.
fn decode_frames(case: &str, body: &[u8]) -> Vec<(String, Value)> {
    let mut buffer = Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            header(":message-type").as_deref(),
            Some("event"),
            "case {case:?}: a response frame must be an event, got {:?}",
            header(":message-type")
        );
        let event_type = header(":event-type")
            .unwrap_or_else(|| panic!("case {case:?}: a response frame named no :event-type"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case:?}: the {event_type} payload must be JSON: {error}")
        });
        frames.push((event_type, payload));
    }
    frames
}

/// The one `output` event of a served turn, asserting the frames around it: the traces come
/// first, and exactly `traced` of them, and the stream ends with the answer.
fn output_event(case: &str, served: &Served, traced: bool) -> Value {
    assert_eq!(
        served.status,
        200,
        "case {case:?}: expected 200, got {} {:?}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    assert_eq!(
        served.content_type, EVENT_STREAM,
        "case {case:?}: expected an event-stream body, got {:?}",
        served.content_type
    );
    let frames = decode_frames(case, &served.body);
    let order: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
    let traces = order.iter().filter(|name| **name == TRACE_EVENT).count();
    assert_eq!(
        order.last(),
        Some(&OUTPUT_EVENT),
        "case {case:?}: the stream must end with the answer, got {order:?}"
    );
    assert_eq!(
        order.len(),
        traces + 1,
        "case {case:?}: a stream carries the traces and one answer, got {order:?}"
    );
    assert_eq!(
        traces > 0,
        traced,
        "case {case:?}: expected traced={traced}, got {order:?}"
    );
    if traced {
        for (name, payload) in frames.iter().take(traces) {
            assert_eq!(
                name, TRACE_EVENT,
                "case {case:?}: every frame before the answer is a trace, got {order:?}"
            );
            let text = payload["text"].as_str().unwrap_or_default();
            assert!(
                !text.is_empty(),
                "case {case:?}: a trace must say what the assistant did, got {payload:?}"
            );
        }
    }
    frames
        .into_iter()
        .next_back()
        .map(|(_, payload)| payload)
        .unwrap_or(Value::Null)
}

/// The `message` of an `output` event.
fn answered(case: &str, output: &Value) -> String {
    output["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry a message, got {output:?}"))
        .to_string()
}

/// One created dataset, as this suite needs to read it back.
#[derive(Debug, Clone)]
struct Dataset {
    id: String,
    arn: String,
}

/// `POST /datasets`: create a dataset carrying `description`, and `kendra` as the knowledge
/// base its source names.
///
/// The shared helper (`crate::dataset_data_segments::create_dataset`) creates neither, and
/// both are what this suite's rows differ along, so the body is built here.
async fn create_dataset(
    router: &MultiServiceRouter,
    name: &str,
    description: Option<&str>,
    kendra: Option<&str>,
) -> Dataset {
    let source = match kendra {
        Some(knowledge_base) => json!({
            "sourceType": "KENDRA",
            "sourceFormat": "KNOWLEDGE_BASE",
            "sourceDetail": {
                "kendra": {
                    "knowledgeBaseArn": knowledge_base,
                    "roleArn": format!("arn:aws:iam::000000000000:role/assistant-{}", "reader"),
                },
            },
        }),
        None => json!({ "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" }),
    };
    let mut body = json!({ "datasetName": name, "datasetSource": source });
    if let Some(description) = description {
        body.as_object_mut()
            .expect("an object body")
            .insert("datasetDescription".to_string(), json!(description));
    }
    let (status, created) = call(router, "POST", "/datasets", Some(body)).await;
    assert_eq!(status, 202, "create dataset {name}: got {status} {created}");
    let id = created["datasetId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&id),
        "create dataset {name}: expected a modelled datasetId, got {created}"
    );
    Dataset {
        id,
        arn: created["datasetArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// The Kendra knowledge base the one Kendra-sourced dataset of this suite names.
fn knowledge_base() -> String {
    format!("arn:aws:kendra:{REGION}:000000000000:index/4f0b5f7a9c")
}

/// RULE: a turn is served as an event stream whose last frame is the modelled `output`
/// event, the traces before it present exactly when `enableTrace` asked for them, and the
/// conversation the turn was recorded under travels in the modelled response header.
#[tokio::test]
async fn a_turn_is_served_as_an_output_event_naming_its_conversation() {
    /// One request, and whether the stream must carry the trace.
    struct Row {
        case: &'static str,
        message: String,
        /// `Some` when the request names its own conversation.
        conversation: Option<String>,
        enable_trace: bool,
    }

    /// A conversation id of the form the model admits that no call minted.
    const OWN_CONVERSATION: &str = "11111111-2222-4333-8444-555555555555";

    let rows = [
        Row {
            case: "a first turn, naming no conversation and no trace",
            message: "which boiler readings do you hold?".to_string(),
            conversation: None,
            enable_trace: false,
        },
        Row {
            case: "a first turn asking for the trace",
            message: "which boiler readings do you hold?".to_string(),
            conversation: None,
            enable_trace: true,
        },
        Row {
            case: "a turn naming the caller's own conversation id",
            message: "and the compressor runbook?".to_string(),
            conversation: Some(OWN_CONVERSATION.to_string()),
            enable_trace: false,
        },
        Row {
            case: "the shortest message the model admits",
            message: "?".to_string(),
            conversation: None,
            enable_trace: true,
        },
        Row {
            case: "the longest message the model admits",
            message: "pressure ".repeat(1_111) + "p",
            conversation: None,
            enable_trace: false,
        },
    ];

    let router = crate::asset_lifecycle::router();
    create_dataset(
        &router,
        "Boiler pressure",
        Some("Hourly pressure readings from the main boiler."),
        None,
    )
    .await;

    for row in rows {
        assert!(
            (1..=10_000).contains(&row.message.chars().count()),
            "case {:?}: the row's own message must be inside the modelled length",
            row.case
        );
        let served = invoke(
            &router,
            turn(&row.message, row.conversation.as_deref(), row.enable_trace),
        )
        .await;
        let output = output_event(row.case, &served, row.enable_trace);
        let message = answered(row.case, &output);

        assert!(
            is_modelled_id(&served.conversation_id),
            "case {:?}: expected a modelled conversation id in {CONVERSATION_HEADER}, got {:?}",
            row.case,
            served.conversation_id
        );
        if let Some(named) = &row.conversation {
            assert_eq!(
                &served.conversation_id, named,
                "case {:?}: a turn naming its own conversation is recorded under it",
                row.case
            );
        }
        assert!(
            message.contains(&format!("conversation {}", served.conversation_id)),
            "case {:?}: the answer must name the conversation it was recorded under, got \
             {message:?}",
            row.case
        );
        assert!(
            !message.contains(row.message.trim()),
            "case {:?}: the answer must not be the submitted message handed back, got \
             {message:?}",
            row.case
        );
        assert!(
            output["citations"].is_array(),
            "case {:?}: the answer must carry its citations, got {output:?}",
            row.case
        );
    }
}

/// RULE: the conversation is state — a turn naming none starts one, a turn naming one
/// continues it, and the turn the answer reports is the count that conversation has taken.
#[tokio::test]
async fn a_conversation_continues_across_turns_and_each_one_is_independent() {
    /// A conversation id of the form the model admits that no call minted.
    const OWN_CONVERSATION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    let (router, engine) = crate::asset_lifecycle::router_with_engine();

    // A first turn naming no conversation starts one under a minted id.
    let first = invoke(&router, turn("hello", None, false)).await;
    let started = first.conversation_id.clone();
    let message = answered("a first turn", &output_event("a first turn", &first, false));
    assert!(
        message.contains("turn 1 of conversation"),
        "case \"a first turn\": expected the first turn of a fresh conversation, got {message:?}"
    );

    // Naming it again CONTINUES it: the same id, one turn further on.
    for expected in [2u32, 3] {
        let case = format!("turn {expected} of the minted conversation");
        let served = invoke(&router, turn("and again", Some(&started), false)).await;
        assert_eq!(
            served.conversation_id, started,
            "case {case:?}: a continued turn is recorded under the conversation it named"
        );
        let message = answered(&case, &output_event(&case, &served, false));
        assert!(
            message.contains(&format!("turn {expected} of conversation {started}")),
            "case {case:?}: expected turn {expected}, got {message:?}"
        );
    }

    // A conversation the CALLER names is independent of it, and starts at its first turn.
    let own = invoke(&router, turn("hello", Some(OWN_CONVERSATION), false)).await;
    let case = "the caller's own conversation";
    let message = answered(case, &output_event(case, &own, false));
    assert_eq!(
        own.conversation_id, OWN_CONVERSATION,
        "case {case:?}: expected the caller's own id in the response header"
    );
    assert!(
        message.contains(&format!("turn 1 of conversation {OWN_CONVERSATION}")),
        "case {case:?}: a conversation nothing has used starts at its first turn, got {message:?}"
    );

    // The store is what those answers were read off: the turns each conversation has taken.
    let state = service_state(&*engine).expect("the IoT SiteWise store");
    let store = state.lock();
    for (conversation, expected) in [(started.as_str(), 3u32), (OWN_CONVERSATION, 1)] {
        let held = store
            .assistant_conversation(conversation)
            .unwrap_or_else(|| panic!("case {conversation:?}: the store must hold the conversation"));
        assert_eq!(
            held.turns, expected,
            "case {conversation:?}: expected {expected} recorded turns, got {}",
            held.turns
        );
        assert_eq!(
            held.id, conversation,
            "case {conversation:?}: the record must carry the id it is addressed by"
        );
        assert!(
            held.last_turn_at >= held.started_at,
            "case {conversation:?}: the last turn cannot predate the conversation"
        );
    }
}

/// RULE: the answer cites the datasets whose stored text carries a term the message names,
/// the ones carrying more of those terms first, each with the dataset's ARN, the source its
/// text was read through and the text itself; a message naming nothing the account holds
/// cites nothing.
#[tokio::test]
async fn the_answer_cites_the_datasets_whose_stored_text_the_message_names() {
    /// One message, and the datasets the answer must cite for it, in order.
    struct Row {
        case: &'static str,
        message: String,
        cites: Vec<&'static str>,
    }

    let router = crate::asset_lifecycle::router();
    let boiler = create_dataset(
        &router,
        "Boiler pressure",
        Some("Hourly pressure readings from the main boiler."),
        None,
    )
    .await;
    // No description: a match on a dataset's NAME is cited with the name, which is the text
    // the match was found in.
    let turbine = create_dataset(&router, "Turbine vibration", None, Some(&knowledge_base())).await;
    let runbook = create_dataset(
        &router,
        "Compressor runbook",
        Some("Maintenance runbook for the compressor hall."),
        None,
    )
    .await;
    let held = [
        ("Boiler pressure", &boiler),
        ("Turbine vibration", &turbine),
        ("Compressor runbook", &runbook),
    ];

    let rows = [
        Row {
            case: "a message naming one dataset's name and its description",
            message: "boiler pressure".to_string(),
            cites: vec!["Boiler pressure"],
        },
        Row {
            case: "a message naming a dataset that carries no description",
            message: "vibration".to_string(),
            cites: vec!["Turbine vibration"],
        },
        Row {
            case: "a message naming only what a description carries",
            message: "maintenance hall".to_string(),
            cites: vec!["Compressor runbook"],
        },
        Row {
            case: "a message naming a term of two datasets, ranked by name when tied",
            message: "boiler runbook".to_string(),
            cites: vec!["Boiler pressure", "Compressor runbook"],
        },
        Row {
            case: "a message whose terms one dataset carries more of than another",
            message: "compressor maintenance boiler".to_string(),
            cites: vec!["Compressor runbook", "Boiler pressure"],
        },
        Row {
            case: "a message naming the dataset by its minted id",
            message: boiler.id.clone(),
            cites: vec!["Boiler pressure"],
        },
        Row {
            case: "a message naming nothing this account holds",
            message: "zzzz quux".to_string(),
            cites: Vec::new(),
        },
        Row {
            case: "a message naming no term at all",
            message: "?!".to_string(),
            cites: Vec::new(),
        },
    ];

    for row in rows {
        let served = invoke(&router, turn(&row.message, None, true)).await;
        let output = output_event(row.case, &served, true);
        let message = answered(row.case, &output);
        let citations = output["citations"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: expected citations, got {output:?}", row.case));
        let cited: Vec<String> = citations
            .iter()
            .map(|citation| {
                citation["reference"]["dataset"]["datasetArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = row
            .cites
            .iter()
            .map(|name| {
                held.iter()
                    .find(|(held_name, _)| held_name == name)
                    .map(|(_, dataset)| dataset.arn.clone())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            cited, expected,
            "case {:?}: expected the datasets {:?} cited in order, got {message:?}",
            row.case, row.cites
        );

        for (citation, name) in citations.iter().zip(&row.cites) {
            let dataset = held
                .iter()
                .find(|(held_name, _)| held_name == name)
                .map(|(_, dataset)| (*dataset).clone())
                .expect("a dataset this suite created");
            // The source a citation is read through: the Kendra knowledge base of a dataset
            // whose source names one, and the dataset's own ARN otherwise.
            let expected_source = if *name == "Turbine vibration" {
                knowledge_base()
            } else {
                dataset.arn.clone()
            };
            let source = citation["reference"]["dataset"]["source"]["arn"]
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                source, expected_source,
                "case {:?}: expected {name} cited through {expected_source}, got {citation:?}",
                row.case
            );
            let text = citation["content"]["text"].as_str().unwrap_or_default();
            assert!(
                !text.is_empty() && message.contains(text),
                "case {:?}: a citation must quote text the answer carries, got {citation:?}",
                row.case
            );
            assert!(
                message.contains(&dataset.id),
                "case {:?}: the answer must name the dataset it cites, got {message:?}",
                row.case
            );
        }
        if row.cites.is_empty() {
            assert!(
                message.contains("cites no dataset"),
                "case {:?}: an answer citing nothing must say so, got {message:?}",
                row.case
            );
        }
    }
}

/// RULE: one answer cites at most the datasets this emulator bounds a turn at, however many
/// of them the message names.
#[tokio::test]
async fn an_answer_cites_at_most_the_bounded_number_of_datasets() {
    let router = crate::asset_lifecycle::router();
    // One past the bound, so the cap is what the answer stops at rather than the account.
    let held = MAX_ASSISTANT_CITATIONS + 1;
    for index in 0..held {
        create_dataset(
            &router,
            &format!("Pressure log {index}"),
            Some("Pressure samples of one line."),
            None,
        )
        .await;
    }

    let case = "every dataset of the account carries the term";
    let served = invoke(&router, turn("pressure", None, false)).await;
    let output = output_event(case, &served, false);
    let message = answered(case, &output);
    let citations = output["citations"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        citations.len(),
        MAX_ASSISTANT_CITATIONS,
        "case {case:?}: expected {MAX_ASSISTANT_CITATIONS} citations, got {}",
        citations.len()
    );
    assert!(
        message.contains(&format!("{held} of the {held} datasets")),
        "case {case:?}: the answer must state how much of the account matched, got {message:?}"
    );
}

/// RULE: one turn searches for at most the number of terms this emulator bounds a turn at,
/// in the order the message states them, however many distinct terms the message names.
#[tokio::test]
async fn a_turn_searches_at_most_the_bounded_number_of_terms() {
    /// Terms no dataset of this suite carries, and that a minted hexadecimal id cannot
    /// either, so the only term that can match is the one the rows place.
    fn filler(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("qa{index}")).collect()
    }

    let router = crate::asset_lifecycle::router();
    create_dataset(&router, "Turbine vibration", None, None).await;

    // The matching term inside the bound is searched for; the same term pushed past it is
    // not, so the bound is what stopped the search rather than the message.
    let mut inside = vec!["vibration".to_string()];
    inside.extend(filler(MAX_ASSISTANT_QUERY_TERMS * 2));
    let mut outside = filler(MAX_ASSISTANT_QUERY_TERMS);
    outside.push("vibration".to_string());

    for (case, terms, cited) in [
        ("the matching term inside the bound", inside, 1usize),
        ("the matching term one past the bound", outside, 0),
    ] {
        let served = invoke(&router, turn(&terms.join(" "), None, true)).await;
        let output = output_event(case, &served, true);
        let message = answered(case, &output);
        let citations = output["citations"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            citations.len(),
            cited,
            "case {case:?}: expected {cited} citations, got {message:?}"
        );
        assert!(
            message.contains(&format!("{MAX_ASSISTANT_QUERY_TERMS} terms searched for")),
            "case {case:?}: the answer must state how many terms it searched for, got \
             {message:?}"
        );
    }
}

/// RULE: a request that breaks one of the model's own rules is refused with the modelled
/// `InvalidRequestException` (HTTP 400), naming the member it broke, and answers no stream.
#[tokio::test]
async fn a_request_that_breaks_the_model_is_refused_before_the_stream() {
    /// One malformed request, and the member the refusal has to name.
    struct Row {
        case: &'static str,
        body: Value,
        names: &'static str,
    }

    let rows = [
        Row {
            case: "no message at all",
            body: json!({}),
            names: "message",
        },
        Row {
            case: "an empty message, below the modelled minimum length",
            body: turn("", None, false),
            names: "message",
        },
        Row {
            case: "a message one character past the modelled maximum length",
            body: turn(&"p".repeat(10_001), None, false),
            names: "message",
        },
        Row {
            case: "a conversation id shorter than the modelled length",
            body: turn("hello", Some("not-a-conversation"), false),
            names: "conversationId",
        },
        Row {
            case: "a conversation id of the right length outside the modelled pattern",
            body: turn("hello", Some("AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE"), false),
            names: "conversationId",
        },
        Row {
            case: "a conversation id with a non-hexadecimal group",
            body: turn("hello", Some("gggggggg-hhhh-4iii-8jjj-kkkkkkkkkkkk"), false),
            names: "conversationId",
        },
    ];

    let router = crate::asset_lifecycle::router();
    for row in rows {
        let served = invoke(&router, row.body.clone()).await;
        assert_eq!(
            served.status,
            400,
            "case {:?}: expected 400, got {} {:?}",
            row.case,
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.error_code(row.case),
            INVALID_REQUEST,
            "case {:?}: expected {INVALID_REQUEST}, got {:?}",
            row.case,
            served.json(row.case)
        );
        assert!(
            served.message(row.case).contains(row.names),
            "case {:?}: the refusal must name {:?}, got {:?}",
            row.case,
            row.names,
            served.message(row.case)
        );
        assert_ne!(
            served.content_type, EVENT_STREAM,
            "case {:?}: a refused request must not answer an event stream",
            row.case
        );
        assert!(
            served.conversation_id.is_empty(),
            "case {:?}: a refused request records no conversation, got {:?}",
            row.case,
            served.conversation_id
        );
    }
}
