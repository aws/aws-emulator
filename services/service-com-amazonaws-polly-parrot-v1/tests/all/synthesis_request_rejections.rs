// SPDX-License-Identifier: CC0-1.0

//! The rejections a synthesis request earns BEFORE anything is synthesized, checked over
//! the class each rule covers rather than over the one request a fixture quoted.
//!
//! Both synthesis paths take the same synthesis request — the synchronous
//! `SynthesizeSpeech` and the asynchronous `StartSpeechSynthesisTask`, which adds only a
//! destination for the output — so each rule below is driven through BOTH, and the
//! asynchronous one is checked to have recorded NO task for a request it refused.
//!
//! RULE 1: a request naming a lexicon the SIGNING Region does not store is refused with
//! HTTP 404 `LexiconNotFoundException`, whatever that name's position in `LexiconNames`
//! and however many of the other names are stored; a request whose every name is stored
//! in that Region is accepted.
//!
//! RULE 2: `SpeechMarkTypes` is accepted only with `OutputFormat` `json` — every other
//! modelled output format earns HTTP 400 `MarksNotSupportedForFormatException`, for every
//! speech-mark type — while a request asking for no marks is accepted with any format.
//!
//! RULE 3: each synthesis API enforces ITS OWN documented text bounds, a total and a
//! billed one: one character past either is refused with HTTP 400
//! `TextLengthExceededException`, the bound itself is accepted, and SSML markup counts
//! towards the total while billing nothing.

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::start_speech_synthesis_task::{
    MAX_BILLED_CHARACTERS as TASK_BILLED_BOUND, MAX_TOTAL_CHARACTERS as TASK_TOTAL_BOUND,
};
use service_com_amazonaws_polly_parrot_v1::ops::synthesize_speech::{
    MAX_BILLED_CHARACTERS as SPEECH_BILLED_BOUND, MAX_TOTAL_CHARACTERS as SPEECH_TOTAL_BOUND,
};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, header, json, pls, Polly};
use crate::synthesis_tasks::{list, task_ids};

/// The Region every row below is signed for, unless it is about another one.
const REGION: &str = "us-east-1";

/// Another Region, for the lexicon stored outside the one the call names.
const OTHER_REGION: &str = "eu-west-1";

/// The bucket the task API writes its output to.
const OUTPUT_BUCKET: &str = "polly-output";

/// A voice every row synthesizes with.
const VOICE: &str = "Joanna";

/// Every value of the model's `OutputFormat` enum
/// (`smithy_query(query="shape", shape="OutputFormat")`), so RULE 2 is checked on the
/// whole domain rather than on the one audio format an issue quoted.
const OUTPUT_FORMATS: &[&str] = &[
    "json",
    "mp3",
    "ogg_opus",
    "ogg_vorbis",
    "pcm",
    "mulaw",
    "alaw",
];

/// Every value of the model's `SpeechMarkType` enum
/// (`smithy_query(query="shape", shape="SpeechMarkType")`).
const SPEECH_MARK_TYPES: &[&str] = &["sentence", "ssml", "viseme", "word"];

/// The `SpeechMarkTypeList` length bound the model declares (`@length 0..4`).
const MAX_SPEECH_MARK_TYPES: usize = 4;

/// The `LexiconNameList` length bound the model declares (`@length 0..5`).
const MAX_LEXICON_NAMES: usize = 5;

/// The opening of an SSML document, which bills nothing.
const SPEAK_OPEN: &str = "<speak>";

/// Its closing, which bills nothing either.
const SPEAK_CLOSE: &str = "</speak>";

/// A piece of SSML markup that carries no text content at all, so it lengthens a document
/// without billing a character. `break` is one of the elements Amazon Polly's
/// supported-SSML-tags documentation publishes, and both of its attributes are optional.
const FILLER: &str = "<break/>";

/// The two paths one synthesis request reaches. They take the same synthesis members, so
/// every rule below holds on both; only the text bounds are each API's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    SynthesizeSpeech,
    StartSpeechSynthesisTask,
}

impl Api {
    /// Both, so every row is driven through each.
    const ALL: [Api; 2] = [Api::SynthesizeSpeech, Api::StartSpeechSynthesisTask];

    fn name(self) -> &'static str {
        match self {
            Api::SynthesizeSpeech => "SynthesizeSpeech",
            Api::StartSpeechSynthesisTask => "StartSpeechSynthesisTask",
        }
    }

    /// The text bounds this API documents: the total and the billed one.
    fn bounds(self) -> (usize, usize) {
        match self {
            Api::SynthesizeSpeech => (SPEECH_TOTAL_BOUND, SPEECH_BILLED_BOUND),
            Api::StartSpeechSynthesisTask => (TASK_TOTAL_BOUND, TASK_BILLED_BOUND),
        }
    }

    /// One call with this synthesis request, signed for `region`. The task API is handed a
    /// destination bucket, the one member of its request shape the synchronous path does
    /// not take.
    async fn call(
        self,
        polly: &Polly,
        region: &str,
        request: &serde_json::Value,
    ) -> http::Response<Bytes> {
        match self {
            Api::SynthesizeSpeech => polly.synthesize_in(region, request).await,
            Api::StartSpeechSynthesisTask => {
                let mut body = request.clone();
                body["OutputS3BucketName"] = serde_json::json!(OUTPUT_BUCKET);
                polly
                    .call_in_region(region, "POST", "/v1/synthesisTasks", &body.to_string())
                    .await
            }
        }
    }

    /// The billed character count this API reports for an accepted request: a response
    /// header on the synchronous path, the task record's own member on the task path.
    fn reported_billed(self, response: &http::Response<Bytes>) -> Option<i64> {
        match self {
            Api::SynthesizeSpeech => header(response, "x-amzn-requestcharacters").parse().ok(),
            Api::StartSpeechSynthesisTask => {
                json(response)["SynthesisTask"]["RequestCharacters"].as_i64()
            }
        }
    }
}

/// The `message` an error response carries.
fn error_message(response: &http::Response<Bytes>) -> Option<String> {
    let body = json(response);
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(|m| m.as_str()) {
            return Some(message.to_string());
        }
    }
    None
}

/// The first bytes of a response body, for a failure report that must not paste a whole
/// audio stream into the output.
fn body_head(response: &http::Response<Bytes>) -> String {
    let body = response.body();
    let head = &body[..body.len().min(200)];
    format!("{} bytes, starting {:?}", body.len(), String::from_utf8_lossy(head))
}

/// Assert one refusal: its status, its error code and the fixed message of that code.
fn assert_refused(
    case: &str,
    response: &http::Response<Bytes>,
    status: u16,
    code: &str,
    message: &str,
) {
    assert_eq!(
        (response.status().as_u16(), error_code(response)),
        (status, code.to_string()),
        "case {case:?}: expected the modelled refusal, got {}",
        body_head(response)
    );
    let actual = error_message(response);
    assert_eq!(
        actual.as_deref(),
        Some(message),
        "case {case:?}: expected the fixed message of {code}, got {actual:?}"
    );
}

/// Assert one acceptance.
fn assert_accepted(case: &str, response: &http::Response<Bytes>) {
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?}: expected the request to be accepted, got {}",
        body_head(response)
    );
}

/// How many tasks the service holds — zero after a run whose task-path rows were all
/// refused, and exactly the accepted ones otherwise.
async fn held_tasks(polly: &Polly) -> Vec<String> {
    task_ids(&json(&list(polly, "").await))
}

/// A synthesis request naming `lexicons`.
fn request_with_lexicons(lexicons: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "Text": "hello world",
        "VoiceId": VOICE,
        "OutputFormat": "mp3",
        "LexiconNames": lexicons,
    })
}

/// RULE 1: every name in `LexiconNames` must be stored in the Region the call is signed
/// for; one that is not refuses the whole request with 404 `LexiconNotFoundException`,
/// and a refused request records no task.
#[tokio::test]
async fn a_lexicon_the_signing_region_does_not_store_refuses_the_whole_request() {
    let polly = Polly::new();
    let document = pls("ipa", "en-US", 1);
    // Enough stored names to fill the list to its modelled maximum.
    let stored: Vec<String> = (0..MAX_LEXICON_NAMES)
        .map(|index| format!("Stored{index}"))
        .collect();
    for name in &stored {
        let response = polly.put_lexicon(name, &document).await;
        assert_accepted(&format!("storing lexicon {name:?}"), &response);
    }
    // ... and one stored in ANOTHER Region, which is the "lexicon that is in a different
    // region" the error's own documentation names.
    let elsewhere = "Elsewhere";
    let response = polly
        .put_lexicon_in(OTHER_REGION, elsewhere, &document)
        .await;
    assert_accepted(
        &format!("storing lexicon {elsewhere:?} in {OTHER_REGION}"),
        &response,
    );

    let absent = "Absent";
    let full_with_gap: Vec<&str> = stored
        .iter()
        .map(String::as_str)
        .enumerate()
        .map(|(index, name)| if index == 2 { absent } else { name })
        .collect();
    let all_stored: Vec<&str> = stored.iter().map(String::as_str).collect();
    let rows: Vec<(&str, Vec<&str>, bool)> = vec![
        ("the empty list", vec![], false),
        ("one stored name", vec![all_stored[0]], false),
        ("the list's maximum of stored names", all_stored.clone(), false),
        ("one name nothing stored", vec![absent], true),
        ("the missing name last", vec![all_stored[0], absent], true),
        ("the missing name first", vec![absent, all_stored[1]], true),
        (
            "the missing name inside a full list",
            full_with_gap.clone(),
            true,
        ),
        ("a name stored only in another Region", vec![elsewhere], true),
        ("no name stored at all", vec![absent, "Missing"], true),
    ];

    let mut accepted_tasks = 0usize;
    for api in Api::ALL {
        for (label, lexicons, refused) in &rows {
            let case = format!("{label} on {}", api.name());
            let response = api
                .call(&polly, REGION, &request_with_lexicons(lexicons))
                .await;
            if *refused {
                assert_refused(
                    &case,
                    &response,
                    404,
                    "LexiconNotFoundException",
                    errors::LEXICON_NOT_FOUND,
                );
            } else {
                assert_accepted(&case, &response);
                if api == Api::StartSpeechSynthesisTask {
                    accepted_tasks += 1;
                }
            }
        }
    }

    // The refusals landed before any task was recorded: only the accepted starts are held.
    let held = held_tasks(&polly).await;
    assert_eq!(
        held.len(),
        accepted_tasks,
        "a refused request must record no task: expected {accepted_tasks} held tasks, \
         got {held:?}"
    );
}

/// The mark sets RULE 2 is checked over, each with the text type its marks need: `ssml`
/// marks describe an SSML document, so a set carrying them is driven with one.
fn mark_sets() -> Vec<(String, Vec<&'static str>, &'static str, String)> {
    let plain = "Hello world. Bye.".to_string();
    let ssml = format!("{SPEAK_OPEN}Hello world. Bye.{SPEAK_CLOSE}");
    let mut sets: Vec<(String, Vec<&'static str>, &'static str, String)> = SPEECH_MARK_TYPES
        .iter()
        .map(|mark| {
            if *mark == "ssml" {
                (format!("the {mark} mark"), vec![*mark], "ssml", ssml.clone())
            } else {
                (
                    format!("the {mark} mark"),
                    vec![*mark],
                    "text",
                    plain.clone(),
                )
            }
        })
        .collect();
    // The list at its modelled maximum length, which is every type at once.
    assert_eq!(
        SPEECH_MARK_TYPES.len(),
        MAX_SPEECH_MARK_TYPES,
        "the mark catalog must be the list's maximum length, got {SPEECH_MARK_TYPES:?}"
    );
    sets.push((
        "every mark type at once".to_string(),
        SPEECH_MARK_TYPES.to_vec(),
        "ssml",
        ssml.clone(),
    ));
    sets
}

/// RULE 2: speech marks ride the `json` output, so a request asking for any of them with
/// any other modelled output format is refused with 400
/// `MarksNotSupportedForFormatException` — and a request asking for none is accepted with
/// every format.
#[tokio::test]
async fn speech_marks_are_accepted_only_with_the_json_output_format() {
    let polly = Polly::new();
    let mut accepted_tasks = 0usize;

    for (label, marks, text_type, text) in mark_sets() {
        for format in OUTPUT_FORMATS {
            for api in Api::ALL {
                let case = format!("{label} with OutputFormat {format} on {}", api.name());
                let request = serde_json::json!({
                    "Text": text,
                    "TextType": text_type,
                    "VoiceId": VOICE,
                    "OutputFormat": format,
                    "SpeechMarkTypes": marks,
                });
                let response = api.call(&polly, REGION, &request).await;
                if *format == "json" {
                    assert_accepted(&case, &response);
                    if api == Api::StartSpeechSynthesisTask {
                        accepted_tasks += 1;
                    }
                } else {
                    assert_refused(
                        &case,
                        &response,
                        400,
                        "MarksNotSupportedForFormatException",
                        errors::MARKS_NOT_SUPPORTED_FOR_FORMAT,
                    );
                }
            }
        }
    }

    // The other side of the rule: asking for no marks — the member absent, and the list in
    // its modelled empty form — is accepted with every format, so the refusals above are
    // the marks' and not the formats'.
    for format in OUTPUT_FORMATS {
        for api in Api::ALL {
            for (shape, marks) in [
                ("no SpeechMarkTypes member", None),
                ("the empty SpeechMarkTypes list", Some(Vec::<&str>::new())),
            ] {
                let case = format!("{shape} with OutputFormat {format} on {}", api.name());
                let mut request = serde_json::json!({
                    "Text": "Hello world. Bye.",
                    "VoiceId": VOICE,
                    "OutputFormat": format,
                });
                if let Some(marks) = marks {
                    request["SpeechMarkTypes"] = serde_json::json!(marks);
                }
                let response = api.call(&polly, REGION, &request).await;
                assert_accepted(&case, &response);
                if api == Api::StartSpeechSynthesisTask {
                    accepted_tasks += 1;
                }
            }
        }
    }

    let held = held_tasks(&polly).await;
    assert_eq!(
        held.len(),
        accepted_tasks,
        "a refused request must record no task: expected {accepted_tasks} held tasks, \
         got {held:?}"
    );
}

/// An SSML document that bills `billed` characters and carries `fillers` pieces of markup
/// that bill none.
fn ssml(billed: usize, fillers: usize) -> String {
    format!(
        "{SPEAK_OPEN}{}{}{SPEAK_CLOSE}",
        "a".repeat(billed),
        FILLER.repeat(fillers)
    )
}

/// The length of [`ssml`]'s document, text content and markup together.
fn ssml_length(billed: usize, fillers: usize) -> usize {
    SPEAK_OPEN.len() + SPEAK_CLOSE.len() + billed + fillers * FILLER.len()
}

/// An SSML document of EXACTLY `total` characters, of which `fillers` pieces of markup
/// bill nothing: the billed text is whatever length the markup leaves.
fn ssml_of_length(total: usize, fillers: usize) -> (String, usize) {
    let billed = total - ssml_length(0, fillers);
    (ssml(billed, fillers), billed)
}

/// One text, and what the API under test must answer for it.
struct TextRow {
    label: String,
    text: String,
    text_type: &'static str,
    /// The billed characters the API must report when it accepts the text, or `None` when
    /// it must refuse it.
    accepted_billed: Option<usize>,
}

/// The rows RULE 3 is checked over for one API: each of its two published bounds, and one
/// character past each.
fn text_rows(api: Api) -> Vec<TextRow> {
    let (total, billed_bound) = api.bounds();
    // Markup fills half the document, so its billed characters stay well under the billed
    // bound while its total length sits exactly on the total one.
    let fillers = total / (2 * FILLER.len());
    let (at_total, at_total_billed) = ssml_of_length(total, fillers);
    let (past_total, past_total_billed) = ssml_of_length(total + 1, fillers);
    assert!(
        at_total_billed <= billed_bound && past_total_billed <= billed_bound,
        "the total-bound rows of {} must stay inside its billed bound of \
         {billed_bound}, got {at_total_billed} and {past_total_billed}",
        api.name()
    );
    vec![
        TextRow {
            label: "plain text at the billed bound".to_string(),
            text: "a".repeat(billed_bound),
            text_type: "text",
            accepted_billed: Some(billed_bound),
        },
        TextRow {
            label: "plain text one character past the billed bound".to_string(),
            text: "a".repeat(billed_bound + 1),
            text_type: "text",
            accepted_billed: None,
        },
        TextRow {
            label: "an ssml document at the billed bound, its markup on top".to_string(),
            text: ssml(billed_bound, 0),
            text_type: "ssml",
            accepted_billed: Some(billed_bound),
        },
        TextRow {
            label: "an ssml document one character past the billed bound".to_string(),
            text: ssml(billed_bound + 1, 0),
            text_type: "ssml",
            accepted_billed: None,
        },
        TextRow {
            label: "an ssml document at the total bound, billed under it".to_string(),
            text: at_total,
            text_type: "ssml",
            accepted_billed: Some(at_total_billed),
        },
        TextRow {
            label: "an ssml document one character past the total bound".to_string(),
            text: past_total,
            text_type: "ssml",
            accepted_billed: None,
        },
    ]
}

/// RULE 3: each API enforces its own documented bounds — the bound itself is accepted, one
/// character past it is refused with 400 `TextLengthExceededException`, and markup counts
/// towards the total while billing nothing.
#[tokio::test]
async fn each_synthesis_api_enforces_its_own_documented_text_bounds() {
    // The published figures, which the rows are built from:
    // "For the SynthesizeSpeech API, the maximum is 6000 characters total, of which no
    // more than 3000 can be billed characters" and "For the StartSpeechSynthesisTask API,
    // the maximum is 200,000 characters, of which no more than 100,000 can be billed
    // characters" (`TextLengthExceededException`'s documentation).
    assert_eq!(
        (SPEECH_TOTAL_BOUND, SPEECH_BILLED_BOUND),
        (6_000, 3_000),
        "the synchronous bounds must be the documented ones, got {SPEECH_TOTAL_BOUND} \
         total and {SPEECH_BILLED_BOUND} billed"
    );
    assert_eq!(
        (TASK_TOTAL_BOUND, TASK_BILLED_BOUND),
        (200_000, 100_000),
        "the task bounds must be the documented ones, got {TASK_TOTAL_BOUND} total and \
         {TASK_BILLED_BOUND} billed"
    );

    let polly = Polly::new();
    let mut accepted_tasks = 0usize;
    for api in Api::ALL {
        for row in text_rows(api) {
            let case = format!("{} on {}", row.label, api.name());
            // `pcm` is the cheapest of the formats to render and the bound under test is
            // the text's, not the format's.
            let request = serde_json::json!({
                "Text": row.text,
                "TextType": row.text_type,
                "VoiceId": VOICE,
                "OutputFormat": "pcm",
            });
            let response = api.call(&polly, REGION, &request).await;
            match row.accepted_billed {
                None => assert_refused(
                    &case,
                    &response,
                    400,
                    "TextLengthExceededException",
                    errors::TEXT_LENGTH_EXCEEDED,
                ),
                Some(billed) => {
                    assert_accepted(&case, &response);
                    let reported = api.reported_billed(&response);
                    assert_eq!(
                        reported,
                        Some(i64::try_from(billed).expect("a bound fits an i64")),
                        "case {case:?}: markup bills nothing, so the billed count must be \
                         {billed}, got {reported:?}"
                    );
                    if api == Api::StartSpeechSynthesisTask {
                        accepted_tasks += 1;
                    }
                }
            }
        }
    }

    // And the two APIs' bounds are distinct: a text inside the task API's billed bound and
    // past the synchronous one is accepted by the one and refused by the other.
    let between = "a".repeat(SPEECH_BILLED_BOUND + 1);
    for (api, expected) in [
        (Api::SynthesizeSpeech, None),
        (Api::StartSpeechSynthesisTask, Some(SPEECH_BILLED_BOUND + 1)),
    ] {
        let case = format!(
            "{} characters, between the two billed bounds, on {}",
            between.len(),
            api.name()
        );
        let request = serde_json::json!({
            "Text": between,
            "VoiceId": VOICE,
            "OutputFormat": "pcm",
        });
        let response = api.call(&polly, REGION, &request).await;
        match expected {
            None => assert_refused(
                &case,
                &response,
                400,
                "TextLengthExceededException",
                errors::TEXT_LENGTH_EXCEEDED,
            ),
            Some(billed) => {
                assert_accepted(&case, &response);
                let reported = api.reported_billed(&response);
                assert_eq!(
                    reported,
                    Some(i64::try_from(billed).expect("a bound fits an i64")),
                    "case {case:?}: the billed count must be {billed}, got {reported:?}"
                );
                accepted_tasks += 1;
            }
        }
    }

    let held = held_tasks(&polly).await;
    assert_eq!(
        held.len(),
        accepted_tasks,
        "a refused request must record no task: expected {accepted_tasks} held tasks, \
         got {held:?}"
    );
}
