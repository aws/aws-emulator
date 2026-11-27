// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRealtimeContactAnalysisSegments`.
//!
//! Every case drives a real HTTP request through the `MultiServiceRouter` — the same
//! path a socket-served request takes — so what is asserted is the wire behaviour, not
//! an internal function call. Segments are put into state through the seeding API on
//! `crate::state` (Contact Lens has no public write API: Amazon Connect's voice
//! analysis is the producer), and then read back through the operation.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connectcontactlens_amazonconnectcontactlens as contact_lens;

use contact_lens::ops::types::{
    Categories, CategoryDetails, CharacterOffsets, ExtractedInformation, ExtractedInformationValue,
    IssueDetected, PointOfInterest, PostContactSummary, RealtimeContactAnalysisSegment, Transcript,
    EXTRACTED_INFORMATION_FAILURE_CODE_VALUES, POST_CONTACT_SUMMARY_FAILURE_CODE_VALUES,
    POST_CONTACT_SUMMARY_STATUS_VALUES, SENTIMENT_VALUE_VALUES,
};
use contact_lens::state::{
    service_state, AnalysisSession, SessionKey, MAX_PAGE_SIZE, VOICE_DATA_RETENTION,
};

/// The `(InstanceId, ContactId)` pair every seeded session is stored under.
///
/// `InstanceId` is a GUID because that is what an Amazon Connect instance is identified by:
/// an identifier of any other form resolves to no instance and is refused at the front door
/// (`src/service/instance_id.rs`, pinned in `tests/all/refusal_stages.rs`). `ContactId` is a
/// GUID too so the swapped-pair row below addresses a resolvable instance and still misses.
const INSTANCE: &str = "4cd1aa0d-1f2e-4b3c-8d4e-5f60718293a4";
const CONTACT: &str = "b5f87632-2a3b-4c5d-9e8f-0a1b2c3d4e5f";
/// A second GUID-shaped instance identifier, for the rows that miss on the instance half.
const OTHER_INSTANCE: &str = "7d9e1c3a-4b5c-4d6e-8f90-1a2b3c4d5e6f";
const URI: &str = "/realtime-contact-analysis/analysis-segments";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`connect`), which is the routing signal the router reads first. The signature
/// itself is never checked here: the test router runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/connect/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// What one dispatched request answered: the status, the restJson1 error discriminator,
/// and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn segments(&self) -> Vec<Value> {
        self.body
            .get("Segments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The modeled `Message` member of an error body (the only member the error shapes
    /// in this service carry).
    fn message(&self) -> Option<String> {
        self.body
            .get("Message")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

fn build_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(contact_lens::EmulatedService::new(Arc::clone(engine)));
    router
}

/// The seeding path: register an analysis session on the shared engine the way the
/// producing side (Amazon Connect's voice analysis) would.
fn seed(engine: &Arc<engine::InMemoryEngine>, session: AnalysisSession) {
    service_state(&**engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .put_session(session);
}

fn complete(engine: &Arc<engine::InMemoryEngine>, key: &SessionKey) {
    let marked = service_state(&**engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .complete_session(key);
    assert!(marked, "complete_session must find the seeded session {key:?}");
}

async fn list(router: &MultiServiceRouter, body: &Value) -> Reply {
    let request = http::Request::builder()
        .method("POST")
        .uri(URI)
        .header("host", "contact-lens.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body: parsed,
    }
}

fn request(instance: &str, contact: &str, max_results: Option<i32>, token: Option<&str>) -> Value {
    let mut body = json!({ "InstanceId": instance, "ContactId": contact });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    body
}

/// Segment `index` of a synthetic transcript, with ascending offsets.
fn transcript_segment(index: usize) -> RealtimeContactAnalysisSegment {
    let begin = i32::try_from(index).expect("test index fits in i32") * 1_000;
    RealtimeContactAnalysisSegment {
        transcript: Some(Transcript {
            id: format!("t-{index}"),
            participant_id: "participant-1".to_string(),
            participant_role: if index.is_multiple_of(2) {
                "CUSTOMER"
            } else {
                "AGENT"
            }
            .to_string(),
            content: format!("utterance {index}"),
            begin_offset_millis: begin,
            end_offset_millis: begin + 900,
            sentiment: None,
            issues_detected: None,
            language_code: None,
        }),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Pagination over the stored segments
// ---------------------------------------------------------------------------

/// Paging a session returns every stored segment exactly once, in stored order, with no
/// page larger than the effective limit — and the response omits `NextToken` exactly
/// when analysis has completed and every segment has been handed out.
///
/// The rows vary what the rule generalises over: how many segments are stored, the page
/// size (absent, each published bound, and sizes that divide the list evenly and
/// unevenly) and whether analysis is still running.
#[tokio::test]
async fn pagination_returns_every_stored_segment_exactly_once() {
    let rows: [(usize, Option<i32>, bool); 12] = [
        (0, None, true),
        (0, Some(1), false),
        (1, Some(1), true),
        (1, None, false),
        (3, Some(2), false),
        (3, Some(2), true),
        (5, Some(1), true),
        (7, Some(3), false),
        (10, None, true),
        (10, Some(MAX_PAGE_SIZE), true),
        (100, Some(MAX_PAGE_SIZE), true),
        (100, Some(7), true),
    ];

    for (count, max_results, completed) in rows {
        let case = format!("count={count} max_results={max_results:?} completed={completed}");
        let engine = build_engine();
        let router = build_router(&engine);
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..count).map(transcript_segment))
                .completed(completed),
        );

        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let page_limit = usize::try_from(max_results.unwrap_or(MAX_PAGE_SIZE))
            .expect("the page limit is positive");

        for page in 0..=count + 1 {
            let reply = list(
                &router,
                &request(INSTANCE, CONTACT, max_results, token.as_deref()),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case}: page {page} expected HTTP 200, got {} {:?}",
                reply.status, reply.body
            );
            let segments = reply.segments();
            assert!(
                segments.len() <= page_limit,
                "case {case}: page {page} expected at most {page_limit} segments, got {}",
                segments.len()
            );
            let drained = segments.is_empty();
            seen.extend(segments);
            token = reply.next_token();
            if token.is_none() {
                break;
            }
            if drained {
                // A drained but still-running session keeps handing back a token so the
                // client can poll for segments that have not been produced yet.
                assert!(
                    !completed,
                    "case {case}: a completed, drained session must omit NextToken, got {token:?}"
                );
                break;
            }
        }

        assert_eq!(
            seen.len(),
            count,
            "case {case}: expected {count} segments across all pages, got {}",
            seen.len()
        );
        for (index, segment) in seen.iter().enumerate() {
            assert_eq!(
                segment["Transcript"]["Id"],
                json!(format!("t-{index}")),
                "case {case}: position {index} expected the stored segment t-{index}, got {segment:?}"
            );
        }
        assert_eq!(
            token.is_none(),
            completed,
            "case {case}: expected NextToken absent={completed}, got token {token:?}"
        );
    }
}

/// A `NextToken` resumes EXACTLY after the last returned segment: the page a token
/// yields is the stored slice starting at that boundary, whatever the boundary is.
#[tokio::test]
async fn a_next_token_resumes_exactly_after_the_last_returned_segment() {
    const COUNT: usize = 9;
    for first_page in 1..=COUNT {
        let case = format!("first_page={first_page}");
        let engine = build_engine();
        let router = build_router(&engine);
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..COUNT).map(transcript_segment))
                .completed(true),
        );

        let first = list(
            &router,
            &request(
                INSTANCE,
                CONTACT,
                Some(i32::try_from(first_page).expect("page size fits in i32")),
                None,
            ),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected HTTP 200, got {} {:?}",
            first.status, first.body
        );
        let token = first.next_token();
        assert_eq!(
            token.is_some(),
            first_page < COUNT,
            "case {case}: expected a NextToken only when segments remain, got {token:?}"
        );
        let Some(token) = token else { continue };

        let second = list(
            &router,
            &request(INSTANCE, CONTACT, Some(MAX_PAGE_SIZE), Some(&token)),
        )
        .await;
        assert_eq!(
            second.status, 200,
            "case {case}: resumed page expected HTTP 200, got {} {:?}",
            second.status, second.body
        );
        let resumed: Vec<String> = second
            .segments()
            .iter()
            .map(|segment| segment["Transcript"]["Id"].to_string())
            .collect();
        let expected: Vec<String> = (first_page..COUNT)
            .map(|index| json!(format!("t-{index}")).to_string())
            .collect();
        assert_eq!(
            resumed, expected,
            "case {case}: expected the resumed page to be {expected:?}, got {resumed:?}"
        );
    }
}

/// Listing is a read: repeating one request answers byte-identically, including the
/// opaque `NextToken`, so no caller's later read is changed by an earlier one.
#[tokio::test]
async fn repeating_a_request_answers_identically() {
    let rows: [(usize, Option<i32>, bool); 4] = [
        (0, None, false),
        (4, Some(2), false),
        (4, Some(2), true),
        (4, None, true),
    ];
    for (count, max_results, completed) in rows {
        let case = format!("count={count} max_results={max_results:?} completed={completed}");
        let engine = build_engine();
        let router = build_router(&engine);
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..count).map(transcript_segment))
                .completed(completed),
        );

        let body = request(INSTANCE, CONTACT, max_results, None);
        let first = list(&router, &body).await;
        let second = list(&router, &body).await;
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {case}: expected the second identical read to answer {:?}, got {:?}",
            first.body,
            second.body
        );
    }
}

// ---------------------------------------------------------------------------
// Segment fidelity
// ---------------------------------------------------------------------------

/// Every stored segment comes back as the value that was stored, for each shape the
/// model lets a `RealtimeContactAnalysisSegment` carry.
///
/// The enum catalogs iterated here are the model's own, re-exported by the service
/// crate from the Smithy model (`SentimentValue`, `PostContactSummaryStatus`,
/// `PostContactSummaryFailureCode`, `ExtractedInformationFailureCode`), so values the
/// issue never cited — `INVALID_ANALYSIS_CONFIGURATION`, `MAX_PACKAGE_FEATURE_ONLY` —
/// are covered too.
#[tokio::test]
async fn stored_segments_round_trip_in_their_modeled_shape() {
    let mut cases: Vec<(String, RealtimeContactAnalysisSegment, Value)> = Vec::new();

    cases.push((
        "transcript without sentiment".to_string(),
        RealtimeContactAnalysisSegment {
            transcript: Some(Transcript {
                id: "seg-plain".to_string(),
                participant_id: "p-1".to_string(),
                participant_role: "CUSTOMER".to_string(),
                content: "hello there".to_string(),
                begin_offset_millis: 0,
                end_offset_millis: 1_200,
                sentiment: None,
                issues_detected: None,
                language_code: None,
            }),
            ..Default::default()
        },
        json!({
            "Transcript": {
                "Id": "seg-plain",
                "ParticipantId": "p-1",
                "ParticipantRole": "CUSTOMER",
                "Content": "hello there",
                "BeginOffsetMillis": 0,
                "EndOffsetMillis": 1_200,
            }
        }),
    ));

    for sentiment in SENTIMENT_VALUE_VALUES {
        cases.push((
            format!("transcript with sentiment {sentiment}"),
            RealtimeContactAnalysisSegment {
                transcript: Some(Transcript {
                    id: format!("seg-{sentiment}"),
                    participant_id: "p-2".to_string(),
                    participant_role: "AGENT".to_string(),
                    content: format!("feeling {sentiment}"),
                    begin_offset_millis: 2_000,
                    end_offset_millis: 2_500,
                    sentiment: Some((*sentiment).to_string()),
                    issues_detected: None,
                    language_code: None,
                }),
                ..Default::default()
            },
            json!({
                "Transcript": {
                    "Id": format!("seg-{sentiment}"),
                    "ParticipantId": "p-2",
                    "ParticipantRole": "AGENT",
                    "Content": format!("feeling {sentiment}"),
                    "BeginOffsetMillis": 2_000,
                    "EndOffsetMillis": 2_500,
                    "Sentiment": sentiment,
                }
            }),
        ));
    }

    cases.push((
        "transcript with issues detected".to_string(),
        RealtimeContactAnalysisSegment {
            transcript: Some(Transcript {
                id: "seg-issues".to_string(),
                participant_id: "p-3".to_string(),
                participant_role: "CUSTOMER".to_string(),
                content: "this is broken and I want a refund".to_string(),
                begin_offset_millis: 3_000,
                end_offset_millis: 4_000,
                sentiment: Some("NEGATIVE".to_string()),
                issues_detected: Some(vec![
                    IssueDetected {
                        character_offsets: CharacterOffsets {
                            begin_offset_char: 0,
                            end_offset_char: 14,
                        },
                    },
                    IssueDetected {
                        character_offsets: CharacterOffsets {
                            begin_offset_char: 19,
                            end_offset_char: 33,
                        },
                    },
                ]),
                language_code: None,
            }),
            ..Default::default()
        },
        json!({
            "Transcript": {
                "Id": "seg-issues",
                "ParticipantId": "p-3",
                "ParticipantRole": "CUSTOMER",
                "Content": "this is broken and I want a refund",
                "BeginOffsetMillis": 3_000,
                "EndOffsetMillis": 4_000,
                "Sentiment": "NEGATIVE",
                "IssuesDetected": [
                    { "CharacterOffsets": { "BeginOffsetChar": 0, "EndOffsetChar": 14 } },
                    { "CharacterOffsets": { "BeginOffsetChar": 19, "EndOffsetChar": 33 } },
                ],
            }
        }),
    ));

    cases.push((
        "categories with matched details".to_string(),
        RealtimeContactAnalysisSegment {
            categories: Some(Categories {
                matched_categories: vec!["Cancellation".to_string(), "Escalation".to_string()],
                matched_details: [
                    (
                        "Cancellation".to_string(),
                        CategoryDetails {
                            points_of_interest: vec![PointOfInterest {
                                begin_offset_millis: 1_000,
                                end_offset_millis: 2_000,
                            }],
                            rule_id: None,
                        },
                    ),
                    (
                        "Escalation".to_string(),
                        CategoryDetails {
                            points_of_interest: vec![
                                PointOfInterest {
                                    begin_offset_millis: 5_000,
                                    end_offset_millis: 6_000,
                                },
                                PointOfInterest {
                                    begin_offset_millis: 7_000,
                                    end_offset_millis: 7_500,
                                },
                            ],
                            rule_id: None,
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            }),
            ..Default::default()
        },
        json!({
            "Categories": {
                "MatchedCategories": ["Cancellation", "Escalation"],
                "MatchedDetails": {
                    "Cancellation": {
                        "PointsOfInterest": [{ "BeginOffsetMillis": 1_000, "EndOffsetMillis": 2_000 }]
                    },
                    "Escalation": {
                        "PointsOfInterest": [
                            { "BeginOffsetMillis": 5_000, "EndOffsetMillis": 6_000 },
                            { "BeginOffsetMillis": 7_000, "EndOffsetMillis": 7_500 },
                        ]
                    },
                },
            }
        }),
    ));

    cases.push((
        "categories with no match".to_string(),
        RealtimeContactAnalysisSegment {
            categories: Some(Categories {
                matched_categories: Vec::new(),
                matched_details: std::collections::HashMap::new(),
            }),
            ..Default::default()
        },
        json!({ "Categories": { "MatchedCategories": [], "MatchedDetails": {} } }),
    ));

    for status in POST_CONTACT_SUMMARY_STATUS_VALUES {
        // A COMPLETED summary carries its content and no failure code; a FAILED one is
        // enumerated against every failure code the model declares.
        if *status == "COMPLETED" {
            cases.push((
                "post contact summary COMPLETED".to_string(),
                RealtimeContactAnalysisSegment {
                    post_contact_summary: Some(PostContactSummary {
                        content: Some("The customer asked to cancel.".to_string()),
                        status: (*status).to_string(),
                        failure_code: None,
                    }),
                    ..Default::default()
                },
                json!({
                    "PostContactSummary": {
                        "Content": "The customer asked to cancel.",
                        "Status": status,
                    }
                }),
            ));
            continue;
        }
        for failure_code in POST_CONTACT_SUMMARY_FAILURE_CODE_VALUES {
            cases.push((
                format!("post contact summary {status}/{failure_code}"),
                RealtimeContactAnalysisSegment {
                    post_contact_summary: Some(PostContactSummary {
                        content: None,
                        status: (*status).to_string(),
                        failure_code: Some((*failure_code).to_string()),
                    }),
                    ..Default::default()
                },
                json!({
                    "PostContactSummary": { "Status": status, "FailureCode": failure_code }
                }),
            ));
        }
    }

    for failure_code in EXTRACTED_INFORMATION_FAILURE_CODE_VALUES {
        cases.push((
            format!("extracted information {failure_code}"),
            RealtimeContactAnalysisSegment {
                extracted_information: Some(ExtractedInformation {
                    extraction_definition_id: format!("def-{failure_code}"),
                    extraction_definition_name: "OrderNumber".to_string(),
                    extraction_definition_display_label: Some("Order no.".to_string()),
                    extracted_values: Some(vec![ExtractedInformationValue {
                        content: "A-1234".to_string(),
                        points_of_interest: vec![PointOfInterest {
                            begin_offset_millis: 8_000,
                            end_offset_millis: 8_400,
                        }],
                    }]),
                    failure_code: Some((*failure_code).to_string()),
                }),
                ..Default::default()
            },
            json!({
                "ExtractedInformation": {
                    "ExtractionDefinitionId": format!("def-{failure_code}"),
                    "ExtractionDefinitionName": "OrderNumber",
                    "ExtractionDefinitionDisplayLabel": "Order no.",
                    "ExtractedValues": [{
                        "Content": "A-1234",
                        "PointsOfInterest": [{ "BeginOffsetMillis": 8_000, "EndOffsetMillis": 8_400 }],
                    }],
                    "FailureCode": failure_code,
                }
            }),
        ));
    }

    // `LanguageCode` is a plain string (len 1..256, pattern \S) rather than an enum
    // (`smithy_query(query="shape",shape="Transcript")`), so these rows span the
    // published length bounds beside the code the member's own doc names.
    let longest_language_code = "l".repeat(256);
    for (index, language_code) in ["en-US", "es-ES", "x", longest_language_code.as_str()]
        .into_iter()
        .enumerate()
    {
        cases.push((
            format!("transcript with language code {language_code:?}"),
            RealtimeContactAnalysisSegment {
                transcript: Some(Transcript {
                    id: format!("seg-lang-{index}"),
                    participant_id: "p-5".to_string(),
                    participant_role: "CUSTOMER".to_string(),
                    content: "where is my order".to_string(),
                    begin_offset_millis: 10_000,
                    end_offset_millis: 10_500,
                    sentiment: None,
                    issues_detected: None,
                    language_code: Some(language_code.to_string()),
                }),
                ..Default::default()
            },
            json!({
                "Transcript": {
                    "Id": format!("seg-lang-{index}"),
                    "ParticipantId": "p-5",
                    "ParticipantRole": "CUSTOMER",
                    "Content": "where is my order",
                    "BeginOffsetMillis": 10_000,
                    "EndOffsetMillis": 10_500,
                    "LanguageCode": language_code,
                }
            }),
        ));
    }

    cases.push((
        "category details naming the rule that created the match".to_string(),
        RealtimeContactAnalysisSegment {
            categories: Some(Categories {
                matched_categories: vec!["Cancellation".to_string()],
                matched_details: [(
                    "Cancellation".to_string(),
                    CategoryDetails {
                        points_of_interest: vec![PointOfInterest {
                            begin_offset_millis: 11_000,
                            end_offset_millis: 11_800,
                        }],
                        rule_id: Some("2f3c4d5e-6f70-4812-9a3b-4c5d6e7f8091".to_string()),
                    },
                )]
                .into_iter()
                .collect(),
            }),
            ..Default::default()
        },
        json!({
            "Categories": {
                "MatchedCategories": ["Cancellation"],
                "MatchedDetails": {
                    "Cancellation": {
                        "PointsOfInterest": [
                            { "BeginOffsetMillis": 11_000, "EndOffsetMillis": 11_800 }
                        ],
                        "RuleId": "2f3c4d5e-6f70-4812-9a3b-4c5d6e7f8091",
                    }
                },
            }
        }),
    ));

    cases.push((
        "transcript and categories in one segment".to_string(),
        RealtimeContactAnalysisSegment {
            transcript: Some(Transcript {
                id: "seg-both".to_string(),
                participant_id: "p-4".to_string(),
                participant_role: "AGENT".to_string(),
                content: "let me escalate that".to_string(),
                begin_offset_millis: 9_000,
                end_offset_millis: 9_900,
                sentiment: Some("NEUTRAL".to_string()),
                issues_detected: None,
                language_code: None,
            }),
            categories: Some(Categories {
                matched_categories: vec!["Escalation".to_string()],
                matched_details: [(
                    "Escalation".to_string(),
                    CategoryDetails {
                        points_of_interest: vec![PointOfInterest {
                            begin_offset_millis: 9_000,
                            end_offset_millis: 9_900,
                        }],
                        rule_id: None,
                    },
                )]
                .into_iter()
                .collect(),
            }),
            ..Default::default()
        },
        json!({
            "Transcript": {
                "Id": "seg-both",
                "ParticipantId": "p-4",
                "ParticipantRole": "AGENT",
                "Content": "let me escalate that",
                "BeginOffsetMillis": 9_000,
                "EndOffsetMillis": 9_900,
                "Sentiment": "NEUTRAL",
            },
            "Categories": {
                "MatchedCategories": ["Escalation"],
                "MatchedDetails": {
                    "Escalation": {
                        "PointsOfInterest": [{ "BeginOffsetMillis": 9_000, "EndOffsetMillis": 9_900 }]
                    }
                },
            },
        }),
    ));

    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments(cases.iter().map(|(_, segment, _)| segment.clone()))
            .completed(true),
    );

    let reply = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        reply.status, 200,
        "expected HTTP 200, got {} {:?}",
        reply.status, reply.body
    );
    let returned = reply.segments();
    assert_eq!(
        returned.len(),
        cases.len(),
        "expected {} segments, got {}",
        cases.len(),
        returned.len()
    );
    for (index, (case, _, expected)) in cases.iter().enumerate() {
        assert_eq!(
            &returned[index], expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            returned[index]
        );
    }
}

// ---------------------------------------------------------------------------
// Unknown session, bad token, refused page size, retention
// ---------------------------------------------------------------------------

/// No stored session for the `(InstanceId, ContactId)` pair is `ResourceNotFoundException`
/// (HTTP 404) — never an empty HTTP 200. Both halves of the key are load-bearing, so a
/// miss on either (or a swapped pair) is a miss.
#[tokio::test]
async fn an_unknown_session_is_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments((0..2).map(transcript_segment))
            .completed(true),
    );

    let misses = [
        ("unknown contact", INSTANCE, "contact-does-not-exist"),
        ("unknown instance", OTHER_INSTANCE, CONTACT),
        ("both unknown", OTHER_INSTANCE, "contact-9"),
        ("swapped pair", CONTACT, INSTANCE),
    ];
    for (case, instance, contact) in misses {
        let reply = list(&router, &request(instance, contact, None, None)).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.body.get("__type").and_then(Value::as_str),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the body's __type to name the error, got {:?}",
            reply.body
        );
    }

    // The control row: the seeded pair itself is served.
    let hit = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        hit.status, 200,
        "case \"seeded pair\": expected HTTP 200, got {} {:?}",
        hit.status, hit.body
    );
}

/// The message Contact Lens returns with `ResourceNotFoundException`. Fixed text: it is
/// pinned here as a literal, not read from the crate, so a handler that starts
/// interpolating the request into it fails this test.
const NOT_FOUND_MESSAGE: &str = "Real-time contact analysis not found.";

/// The not-found refusal carries the service's FIXED message: the same sentence for every
/// missing session, with nothing from the request (or from stored state) interpolated into
/// it, and no member besides `Message` in the body.
///
/// The rows vary what the rule generalises over — which half of the key misses, the form
/// and length of the `ContactId` (len 1..256, pattern `\S`, per
/// `smithy_query(query="operation",operation="ListRealtimeContactAnalysisSegments")`), and
/// whether the session never existed at all or has aged out of the retention window. Every
/// row names a resolvable instance, because an `InstanceId` that resolves to none never
/// reaches this stage (`tests/all/refusal_stages.rs` pins that one).
#[tokio::test]
async fn the_not_found_message_is_the_services_fixed_text() {
    let longest = "l".repeat(256);
    let rows: [(&str, &str, &str, bool); 9] = [
        ("unknown contact", INSTANCE, "contact-does-not-exist", false),
        ("unknown instance", OTHER_INSTANCE, CONTACT, false),
        ("both unknown", OTHER_INSTANCE, "contact-9", false),
        ("swapped pair", CONTACT, INSTANCE, false),
        (
            "an instance arn naming the instance",
            "arn:aws:connect:us-east-1:000000000000:instance/7d9e1c3a-4b5c-4d6e-8f90-1a2b3c4d5e6f",
            CONTACT,
            false,
        ),
        ("single-character contact id", INSTANCE, "c", false),
        ("maximum-length contact id", INSTANCE, &longest, false),
        ("punctuated contact id", INSTANCE, "contact%9#d", false),
        ("session aged out of retention", INSTANCE, CONTACT, true),
    ];

    for (case, instance, contact, aged_out) in rows {
        let engine = build_engine();
        let router = build_router(&engine);
        // The control session always exists; for the aged-out row it is the requested
        // pair itself, placed past the retention window.
        let started_at = if aged_out {
            SystemTime::now()
                .checked_sub(VOICE_DATA_RETENTION + Duration::from_secs(60))
                .expect("a representable start time")
        } else {
            SystemTime::now()
        };
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..2).map(transcript_segment))
                .completed(true)
                .started_at(started_at),
        );

        let reply = list(&router, &request(instance, contact, None, None)).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.message().as_deref(),
            Some(NOT_FOUND_MESSAGE),
            "case {case:?}: expected the fixed message {NOT_FOUND_MESSAGE:?}, got {:?}",
            reply.message()
        );
        assert!(
            reply.body.get("Segments").is_none() && reply.body.get("NextToken").is_none(),
            "case {case:?}: the error body must carry no output member, got {:?}",
            reply.body
        );
    }

    // The control row: a session inside the window is served, so the refusal above is
    // the missing session and not a blanket 404.
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments((0..2).map(transcript_segment))
            .completed(true),
    );
    let hit = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        hit.status, 200,
        "case \"seeded pair\": expected HTTP 200, got {} {:?}",
        hit.status, hit.body
    );
    assert_eq!(
        hit.message(),
        None,
        "case \"seeded pair\": a served page carries no Message, got {:?}",
        hit.body
    );
}

/// Polling an unknown session repeatedly answers the SAME refusal every time: the message
/// is state-independent, so it cannot drift between calls.
#[tokio::test]
async fn repeated_polls_of_an_unknown_session_answer_identically() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT).with_segments((0..2).map(transcript_segment)),
    );

    for poll in 0..4 {
        let reply = list(&router, &request(INSTANCE, "never-a-contact", None, None)).await;
        assert_eq!(
            (reply.status, reply.message().as_deref()),
            (404, Some(NOT_FOUND_MESSAGE)),
            "case \"poll {poll}\": expected 404 with the fixed message, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// A `NextToken` the service did not mint for THIS contact is `InvalidRequestException`
/// (HTTP 400): the token is resolved against stored cursors, never parsed.
#[tokio::test]
async fn a_next_token_with_no_live_cursor_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let other: SessionKey = (INSTANCE.to_string(), "contact-2".to_string());
    for contact in [CONTACT, "contact-2"] {
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, contact).with_segments((0..3).map(transcript_segment)),
        );
    }

    // A token minted for the OTHER contact, and one for a session that was replaced.
    let foreign = list(&router, &request(INSTANCE, &other.1, Some(1), None))
        .await
        .next_token()
        .expect("an open session hands back a NextToken");
    let replaced = list(&router, &request(INSTANCE, CONTACT, Some(1), None))
        .await
        .next_token()
        .expect("an open session hands back a NextToken");
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT).with_segments((0..3).map(transcript_segment)),
    );

    let long_token = "x".repeat(131_070);
    let rejected: [(&str, &str); 5] = [
        ("unstructured garbage", "garbage"),
        ("never minted", "ffffffffffffffffffffffffffffffff"),
        ("minted for another contact", &foreign),
        ("cursor dropped with its session", &replaced),
        ("maximum-length token", &long_token),
    ];
    for (case, token) in rejected {
        let reply = list(&router, &request(INSTANCE, CONTACT, None, Some(token))).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            reply.error_type
        );
    }

    // The control row: a token this contact's own cursor backs is accepted.
    let fresh = list(&router, &request(INSTANCE, CONTACT, Some(1), None))
        .await
        .next_token()
        .expect("an open session hands back a NextToken");
    let reply = list(&router, &request(INSTANCE, CONTACT, Some(1), Some(&fresh))).await;
    assert_eq!(
        reply.status, 200,
        "case \"live cursor\": expected HTTP 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.segments()
            .first()
            .map(|segment| segment["Transcript"]["Id"].clone()),
        Some(json!("t-1")),
        "case \"live cursor\": expected the page to resume at t-1, got {:?}",
        reply.segments()
    );
}

/// `MaxResults` outside the modeled 1..100 range is refused with HTTP 400 before the
/// handler runs; each published bound is served.
#[tokio::test]
async fn max_results_outside_the_modeled_range_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments((0..3).map(transcript_segment))
            .completed(true),
    );

    // The published bounds (1 and 100), and one value past each, plus the extremes.
    let rows: [(i32, u16); 6] = [
        (1, 200),
        (MAX_PAGE_SIZE, 200),
        (0, 400),
        (MAX_PAGE_SIZE + 1, 400),
        (-1, 400),
        (i32::MAX, 400),
    ];
    for (max_results, expected) in rows {
        let reply = list(&router, &request(INSTANCE, CONTACT, Some(max_results), None)).await;
        assert_eq!(
            reply.status, expected,
            "case MaxResults={max_results}: expected HTTP {expected}, got {} {:?}",
            reply.status, reply.body
        );
    }
}

/// Voice data is retained for 24 hours (the modeled operation doc), so a session whose
/// analysis started at or before that bound behaves as absent.
#[tokio::test]
async fn a_session_past_the_retention_window_behaves_as_absent() {
    let rows: [(&str, Duration, bool, u16); 5] = [
        ("just started", Duration::ZERO, false, 200),
        (
            "one minute inside the window",
            VOICE_DATA_RETENTION - Duration::from_secs(60),
            false,
            200,
        ),
        ("exactly at the window", VOICE_DATA_RETENTION, false, 404),
        (
            "one minute past the window",
            VOICE_DATA_RETENTION + Duration::from_secs(60),
            false,
            404,
        ),
        ("started in the future", Duration::from_secs(600), true, 200),
    ];

    for (case, offset, in_future, expected) in rows {
        let engine = build_engine();
        let router = build_router(&engine);
        let now = SystemTime::now();
        let started_at = if in_future {
            now.checked_add(offset).expect("a representable start time")
        } else {
            now.checked_sub(offset).expect("a representable start time")
        };
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..2).map(transcript_segment))
                .completed(true)
                .started_at(started_at),
        );

        let reply = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            reply.status, reply.body
        );
    }
}

/// Segments produced after a page was read show up on the next call, and completing the
/// session is what finally drops `NextToken` — the round trip through stored state.
#[tokio::test]
async fn segments_appended_after_a_read_appear_on_the_next_call() {
    let engine = build_engine();
    let router = build_router(&engine);
    let key: SessionKey = (INSTANCE.to_string(), CONTACT.to_string());
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT).with_segments((0..2).map(transcript_segment)),
    );

    let first = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
    assert_eq!(
        first.segments().len(),
        2,
        "case \"before append\": expected 2 segments, got {:?}",
        first.segments()
    );
    let token = first
        .next_token()
        .expect("case \"before append\": an open session must hand back a NextToken");

    let appended = service_state(&*engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .append_segments(&key, (2..5).map(transcript_segment));
    assert!(appended, "append_segments must find the seeded session");

    let second = list(&router, &request(INSTANCE, CONTACT, None, Some(&token))).await;
    let resumed: Vec<Value> = second
        .segments()
        .iter()
        .map(|segment| segment["Transcript"]["Id"].clone())
        .collect();
    assert_eq!(
        resumed,
        vec![json!("t-2"), json!("t-3"), json!("t-4")],
        "case \"after append\": expected the newly produced segments, got {resumed:?}"
    );
    assert!(
        second.next_token().is_some(),
        "case \"after append\": a running session must still hand back a NextToken, got {:?}",
        second.body
    );

    complete(&engine, &key);
    let third = list(
        &router,
        &request(
            INSTANCE,
            CONTACT,
            None,
            second.next_token().as_deref(),
        ),
    )
    .await;
    assert!(
        third.segments().is_empty(),
        "case \"after completion\": expected no further segments, got {:?}",
        third.segments()
    );
    assert_eq!(
        third.next_token(),
        None,
        "case \"after completion\": a completed, drained session must omit NextToken, got {:?}",
        third.body
    );
}

// ---------------------------------------------------------------------------
// The `ListFilter`: which categories a caller may view
// ---------------------------------------------------------------------------

/// The rules whose matches the seeded category segments carry. A rule identifier is
/// `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="shape",shape="CategoryDetails")`), so these are legal
/// identifiers; what matters to a filter is the TAGS each one carries.
const RULE_PUBLIC: &str = "rule-public";
const RULE_PRIVATE: &str = "rule-private";
/// A rule no tags were ever registered for: a rule with no tags, which is the same
/// absence as a category naming no rule at all.
const RULE_UNTAGGED: &str = "rule-untagged";

/// The message the service answers a `CategoriesAttributeFilter` that does not name
/// exactly one of its three conditions with. Pinned here as a literal, not read from the
/// crate, so a handler that starts interpolating the request into it fails this test.
const ONE_CONDITION_MESSAGE: &str = "Exactly one of OrConditions, AndCondition, or \
     TagCondition must be provided in CategoriesAttributeFilter.";

/// The seeding path for a rule's tags: Amazon Connect owns the rules, so the tags on one
/// are registered on the shared engine the way its producing side would register them.
fn seed_rule_tags(
    engine: &Arc<engine::InMemoryEngine>,
    instance: &str,
    rule_id: &str,
    tags: &[(&str, &str)],
) {
    service_state(&**engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .put_rule_tags(
            instance,
            rule_id,
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
        );
}

/// A category segment: one matched category, created by `rule_id` where it names a rule.
fn category_segment(name: &str, rule_id: Option<&str>) -> RealtimeContactAnalysisSegment {
    RealtimeContactAnalysisSegment {
        categories: Some(Categories {
            matched_categories: vec![name.to_string()],
            matched_details: [(
                name.to_string(),
                CategoryDetails {
                    points_of_interest: vec![PointOfInterest {
                        begin_offset_millis: 1_000,
                        end_offset_millis: 2_000,
                    }],
                    rule_id: rule_id.map(str::to_string),
                },
            )]
            .into_iter()
            .collect(),
        }),
        ..Default::default()
    }
}

/// What one returned segment is, for an assertion over a whole page: its transcript's
/// identifier and the matched categories it carries, so a page is compared as a sequence
/// of identities rather than of whole bodies.
fn identity(segment: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(id) = segment["Transcript"]["Id"].as_str() {
        parts.push(format!("transcript:{id}"));
    }
    if let Some(names) = segment["Categories"]["MatchedCategories"].as_array() {
        let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
        parts.push(format!("categories:[{}]", names.join(",")));
    }
    parts.join("+")
}

/// The identities of every segment one reply returned, in the order it returned them.
fn identities(reply: &Reply) -> Vec<String> {
    reply.segments().iter().map(identity).collect()
}

/// A request carrying `ListFilter`.
fn filtered_request(instance: &str, contact: &str, max_results: Option<i32>, filter: &Value) -> Value {
    let mut body = request(instance, contact, max_results, None);
    body["ListFilter"] = filter.clone();
    body
}

/// A session whose segments are one category per rule — tagged public, tagged private,
/// one naming a rule with no tags, one naming no rule — plus a transcript segment, which
/// is about no category at all.
fn seed_tagged_session(engine: &Arc<engine::InMemoryEngine>, instance: &str) {
    seed_rule_tags(
        engine,
        instance,
        RULE_PUBLIC,
        &[("Visibility", "PUBLIC"), ("Team", "SALES")],
    );
    seed_rule_tags(
        engine,
        instance,
        RULE_PRIVATE,
        &[("Visibility", "PRIVATE"), ("Team", "SUPPORT")],
    );
    seed(
        engine,
        AnalysisSession::new(instance, CONTACT)
            .with_segments([
                category_segment("Cancellation", Some(RULE_PUBLIC)),
                category_segment("Escalation", Some(RULE_PRIVATE)),
                category_segment("Compliance", Some(RULE_UNTAGGED)),
                category_segment("Legacy", None),
                transcript_segment(0),
            ])
            .completed(true),
    );
}

/// A category is returned only when the tags on the rule that created it meet the one
/// condition the request's `CategoriesAttributeFilter` names; a segment about no category
/// is returned whatever the filter says.
///
/// The rows vary what the rule generalises over: which of the three conditions the filter
/// names, the comparison type (`EQUALS`, `NOT_EQUALS`, and a condition naming none), how
/// many tag conditions one condition carries (none, one, several), and whether the rule
/// carries the tag, carries it with another value, carries no tags, or does not exist.
#[tokio::test]
async fn a_list_filter_admits_only_the_categories_its_rule_tags_meet() {
    let public = "categories:[Cancellation]";
    let private = "categories:[Escalation]";
    let untagged = "categories:[Compliance]";
    let no_rule = "categories:[Legacy]";
    let transcript = "transcript:t-0";

    let rows: [(&str, Option<Value>, Vec<&str>); 13] = [
        ("no filter at all", None, vec![public, private, untagged, no_rule, transcript]),
        (
            "a filter naming no categories attribute filter",
            Some(json!({})),
            vec![public, private, untagged, no_rule, transcript],
        ),
        (
            "one tag condition the public rule carries",
            Some(json!({ "CategoriesAttributeFilter": { "TagCondition": {
                "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS"
            } } })),
            vec![public, transcript],
        ),
        (
            "one tag condition with no comparison type",
            Some(json!({ "CategoriesAttributeFilter": { "TagCondition": {
                "TagKey": "Visibility", "TagValue": "PRIVATE"
            } } })),
            vec![private, transcript],
        ),
        (
            "one NOT_EQUALS tag condition",
            Some(json!({ "CategoriesAttributeFilter": { "TagCondition": {
                "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "NOT_EQUALS"
            } } })),
            vec![private, untagged, no_rule, transcript],
        ),
        (
            "one tag condition on a tag key no rule carries",
            Some(json!({ "CategoriesAttributeFilter": { "TagCondition": {
                "TagKey": "Department", "TagValue": "SALES", "ComparisonType": "EQUALS"
            } } })),
            vec![transcript],
        ),
        (
            "one tag condition on a value no rule carries for that key",
            Some(json!({ "CategoriesAttributeFilter": { "TagCondition": {
                "TagKey": "Team", "TagValue": "BILLING", "ComparisonType": "EQUALS"
            } } })),
            vec![transcript],
        ),
        (
            "an and condition every tag of the public rule meets",
            Some(json!({ "CategoriesAttributeFilter": { "AndCondition": { "TagConditions": [
                { "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS" },
                { "TagKey": "Team", "TagValue": "SALES", "ComparisonType": "EQUALS" }
            ] } } })),
            vec![public, transcript],
        ),
        (
            "an and condition no single rule meets",
            Some(json!({ "CategoriesAttributeFilter": { "AndCondition": { "TagConditions": [
                { "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS" },
                { "TagKey": "Team", "TagValue": "SUPPORT", "ComparisonType": "EQUALS" }
            ] } } })),
            vec![transcript],
        ),
        (
            "an and condition carrying no tag condition",
            Some(json!({ "CategoriesAttributeFilter": {
                "AndCondition": { "TagConditions": [] }
            } })),
            vec![public, private, untagged, no_rule, transcript],
        ),
        (
            "or conditions one of which is met",
            Some(json!({ "CategoriesAttributeFilter": { "OrConditions": [
                { "TagConditions": [
                    { "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS" }
                ] },
                { "TagConditions": [
                    { "TagKey": "Team", "TagValue": "SUPPORT", "ComparisonType": "EQUALS" }
                ] }
            ] } })),
            vec![public, private, transcript],
        ),
        (
            "or conditions none of which is met",
            Some(json!({ "CategoriesAttributeFilter": { "OrConditions": [
                { "TagConditions": [
                    { "TagKey": "Visibility", "TagValue": "SECRET", "ComparisonType": "EQUALS" }
                ] }
            ] } })),
            vec![transcript],
        ),
        (
            "an empty list of or conditions",
            Some(json!({ "CategoriesAttributeFilter": { "OrConditions": [] } })),
            vec![transcript],
        ),
    ];

    for (case, filter, expected) in rows {
        let engine = build_engine();
        let router = build_router(&engine);
        seed_tagged_session(&engine, INSTANCE);

        let body = match &filter {
            None => request(INSTANCE, CONTACT, None, None),
            Some(filter) => filtered_request(INSTANCE, CONTACT, None, filter),
        };
        let reply = list(&router, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            reply.status, reply.body
        );
        let returned = identities(&reply);
        assert_eq!(
            returned, expected,
            "case {case:?}: expected {expected:?}, got {returned:?}"
        );
    }
}

/// A category segment carrying several matched categories comes back carrying only the
/// ones the filter admits — the matched names and their details together — and one
/// carrying no admitted category is not returned at all.
#[tokio::test]
async fn a_filter_prunes_the_categories_of_one_segment() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed_rule_tags(&engine, INSTANCE, RULE_PUBLIC, &[("Visibility", "PUBLIC")]);
    seed_rule_tags(&engine, INSTANCE, RULE_PRIVATE, &[("Visibility", "PRIVATE")]);

    let mixed = RealtimeContactAnalysisSegment {
        categories: Some(Categories {
            matched_categories: vec!["Cancellation".to_string(), "Escalation".to_string()],
            matched_details: [
                (
                    "Cancellation".to_string(),
                    CategoryDetails {
                        points_of_interest: vec![PointOfInterest {
                            begin_offset_millis: 1_000,
                            end_offset_millis: 2_000,
                        }],
                        rule_id: Some(RULE_PUBLIC.to_string()),
                    },
                ),
                (
                    "Escalation".to_string(),
                    CategoryDetails {
                        points_of_interest: vec![PointOfInterest {
                            begin_offset_millis: 3_000,
                            end_offset_millis: 4_000,
                        }],
                        rule_id: Some(RULE_PRIVATE.to_string()),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        }),
        ..Default::default()
    };
    // A segment whose only matched category the filter hides, and which carries nothing
    // else: the filter leaves nothing of it.
    let hidden = category_segment("Escalation", Some(RULE_PRIVATE));
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments([mixed, hidden])
            .completed(true),
    );

    let filter = json!({ "CategoriesAttributeFilter": { "TagCondition": {
        "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS"
    } } });
    let reply = list(&router, &filtered_request(INSTANCE, CONTACT, None, &filter)).await;
    assert_eq!(
        reply.status, 200,
        "case \"pruned segment\": expected HTTP 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.segments(),
        vec![json!({
            "Categories": {
                "MatchedCategories": ["Cancellation"],
                "MatchedDetails": {
                    "Cancellation": {
                        "PointsOfInterest": [
                            { "BeginOffsetMillis": 1_000, "EndOffsetMillis": 2_000 }
                        ],
                        "RuleId": RULE_PUBLIC,
                    }
                },
            }
        })],
        "case \"pruned segment\": expected only the admitted category, got {:?}",
        reply.segments()
    );
}

/// A filter is about categories, so it leaves the rest of a segment alone: a segment that
/// also carries a transcript keeps it even when every category of that segment is hidden.
#[tokio::test]
async fn a_filter_leaves_everything_but_the_categories_of_a_segment() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed_rule_tags(&engine, INSTANCE, RULE_PRIVATE, &[("Visibility", "PRIVATE")]);

    let mut combined = transcript_segment(7);
    combined.categories = category_segment("Escalation", Some(RULE_PRIVATE)).categories;
    seed(
        &engine,
        AnalysisSession::new(INSTANCE, CONTACT)
            .with_segments([combined])
            .completed(true),
    );

    let filter = json!({ "CategoriesAttributeFilter": { "TagCondition": {
        "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS"
    } } });
    let reply = list(&router, &filtered_request(INSTANCE, CONTACT, None, &filter)).await;
    let returned = identities(&reply);
    assert_eq!(
        returned,
        vec!["transcript:t-7".to_string()],
        "case \"hidden category beside a transcript\": expected the transcript alone, got \
         {returned:?} in {:?}",
        reply.body
    );
}

/// A rule identifier is resolved within the instance the request names: the same
/// identifier tagged differently in another instance decides nothing for this one.
#[tokio::test]
async fn rule_tags_are_resolved_within_the_requested_instance() {
    let engine = build_engine();
    let router = build_router(&engine);
    // ONE rule identifier, tagged `PUBLIC` in one instance and `PRIVATE` in the other.
    seed_rule_tags(&engine, INSTANCE, RULE_PUBLIC, &[("Visibility", "PUBLIC")]);
    seed_rule_tags(
        &engine,
        OTHER_INSTANCE,
        RULE_PUBLIC,
        &[("Visibility", "PRIVATE")],
    );
    for instance in [INSTANCE, OTHER_INSTANCE] {
        seed(
            &engine,
            AnalysisSession::new(instance, CONTACT)
                .with_segments([category_segment("Cancellation", Some(RULE_PUBLIC))])
                .completed(true),
        );
    }

    let filter = json!({ "CategoriesAttributeFilter": { "TagCondition": {
        "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS"
    } } });
    for (case, instance, expected) in [
        ("the instance that tagged the rule PUBLIC", INSTANCE, vec!["categories:[Cancellation]"]),
        ("the instance that tagged it PRIVATE", OTHER_INSTANCE, Vec::new()),
    ] {
        let reply = list(&router, &filtered_request(instance, CONTACT, None, &filter)).await;
        let returned = identities(&reply);
        assert_eq!(
            returned, expected,
            "case {case:?}: expected {expected:?}, got {returned:?}"
        );
    }
}

/// Paging a filtered read hands out every admitted segment exactly once, in stored order,
/// with no page larger than the effective limit — and the stored segments a filter hides
/// are still there for the next caller, whose own filter decides afresh.
#[tokio::test]
async fn a_filtered_read_pages_over_the_admitted_segments_and_changes_nothing() {
    const COUNT: usize = 10;
    for max_results in [Some(1), Some(2), Some(3), Some(MAX_PAGE_SIZE), None] {
        let case = format!("max_results={max_results:?}");
        let engine = build_engine();
        let router = build_router(&engine);
        seed_rule_tags(&engine, INSTANCE, RULE_PUBLIC, &[("Visibility", "PUBLIC")]);
        seed_rule_tags(&engine, INSTANCE, RULE_PRIVATE, &[("Visibility", "PRIVATE")]);
        seed(
            &engine,
            AnalysisSession::new(INSTANCE, CONTACT)
                .with_segments((0..COUNT).map(|index| {
                    let rule = if index.is_multiple_of(2) {
                        RULE_PUBLIC
                    } else {
                        RULE_PRIVATE
                    };
                    category_segment(&format!("Category-{index}"), Some(rule))
                }))
                .completed(true),
        );

        let filter = json!({ "CategoriesAttributeFilter": { "TagCondition": {
            "TagKey": "Visibility", "TagValue": "PUBLIC", "ComparisonType": "EQUALS"
        } } });
        let page_limit = usize::try_from(max_results.unwrap_or(MAX_PAGE_SIZE))
            .expect("the page limit is positive");

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..=COUNT {
            let mut body = filtered_request(INSTANCE, CONTACT, max_results, &filter);
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let reply = list(&router, &body).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: page {page} expected HTTP 200, got {} {:?}",
                reply.status, reply.body
            );
            let returned = identities(&reply);
            assert!(
                returned.len() <= page_limit,
                "case {case}: page {page} expected at most {page_limit} segments, got \
                 {returned:?}"
            );
            seen.extend(returned);
            token = reply.next_token();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case}: a completed, drained session must omit NextToken, got {token:?}"
        );
        let expected: Vec<String> = (0..COUNT)
            .filter(|index| index.is_multiple_of(2))
            .map(|index| format!("categories:[Category-{index}]"))
            .collect();
        assert_eq!(
            seen, expected,
            "case {case}: expected every admitted segment exactly once, got {seen:?}"
        );

        // The read changed nothing: an unfiltered caller still sees every stored segment.
        let unfiltered = list(&router, &request(INSTANCE, CONTACT, None, None)).await;
        let all: Vec<String> = (0..COUNT)
            .map(|index| format!("categories:[Category-{index}]"))
            .collect();
        assert_eq!(
            identities(&unfiltered),
            all,
            "case {case}: a filtered read must not drop a stored segment, got {:?}",
            unfiltered.body
        );
    }
}

/// A `CategoriesAttributeFilter` names exactly one of `OrConditions`, `AndCondition` and
/// `TagCondition` (its modelled doc), so one naming none of them, or several, is
/// `InvalidRequestException` (HTTP 400) — and the refusal carries the service's fixed
/// sentence.
#[tokio::test]
async fn a_filter_that_does_not_name_exactly_one_condition_is_refused() {
    let tag_condition = json!({ "TagKey": "Visibility", "TagValue": "PUBLIC" });
    let and_condition = json!({ "TagConditions": [tag_condition.clone()] });
    let rows: [(&str, Value, u16); 7] = [
        ("no condition at all", json!({}), 400),
        (
            "a tag condition and an and condition",
            json!({ "TagCondition": tag_condition, "AndCondition": and_condition }),
            400,
        ),
        (
            "or conditions and a tag condition",
            json!({ "OrConditions": [and_condition], "TagCondition": tag_condition }),
            400,
        ),
        (
            "or conditions and an and condition",
            json!({ "OrConditions": [and_condition], "AndCondition": and_condition }),
            400,
        ),
        (
            "all three conditions",
            json!({
                "OrConditions": [and_condition],
                "AndCondition": and_condition,
                "TagCondition": tag_condition,
            }),
            400,
        ),
        // The control rows: exactly one condition is served, whichever one it is.
        ("one tag condition", json!({ "TagCondition": tag_condition }), 200),
        ("one and condition", json!({ "AndCondition": and_condition }), 200),
    ];

    for (case, categories_filter, expected) in rows {
        let engine = build_engine();
        let router = build_router(&engine);
        seed_tagged_session(&engine, INSTANCE);

        let filter = json!({ "CategoriesAttributeFilter": categories_filter });
        let reply = list(&router, &filtered_request(INSTANCE, CONTACT, None, &filter)).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            reply.status, reply.body
        );
        if expected != 400 {
            continue;
        }
        assert_eq!(
            reply.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.message().as_deref(),
            Some(ONE_CONDITION_MESSAGE),
            "case {case:?}: expected the fixed message {ONE_CONDITION_MESSAGE:?}, got {:?}",
            reply.message()
        );
        assert!(
            reply.body.get("Segments").is_none(),
            "case {case:?}: the error body must carry no output member, got {:?}",
            reply.body
        );
    }
}

/// A filter is read before any stored state is: a request whose filter names no condition
/// is refused HTTP 400 whether or not the contact it names has an analysis session, so the
/// refusal says nothing about stored state.
#[tokio::test]
async fn a_refused_filter_is_refused_before_the_session_is_looked_up() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed_tagged_session(&engine, INSTANCE);

    let filter = json!({ "CategoriesAttributeFilter": {} });
    for (case, contact) in [
        ("a contact with a session", CONTACT),
        ("a contact with none", "0b1c2d3e-4f50-4617-8829-9a0b1c2d3e4f"),
    ] {
        let reply = list(&router, &filtered_request(INSTANCE, contact, None, &filter)).await;
        assert_eq!(
            (reply.status, reply.message().as_deref()),
            (400, Some(ONE_CONDITION_MESSAGE)),
            "case {case:?}: expected the filter refusal, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}
