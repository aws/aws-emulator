// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Amazon Textract's DOCUMENT INSIGHTS jobs, driven with the
//! emulator's own Amazon S3 mounted beside textract on one engine — the way the assembled
//! emulator runs them, and the only way a job can be run at all: `StartDocumentInsights`
//! takes its document from an Amazon S3 URI and writes its results back to one.
//!
//! THE RULE UNDER TEST: a job is real work with a real artefact. The document named by the
//! request's `InputConfiguration` is read out of the object store, the content is extracted
//! from THAT text under the project's `ExtractionConfiguration` and the blueprints in play,
//! and the result is stored as an object under the `OutputConfiguration`'s location that a
//! caller can fetch with `GetObject`. `GetDocumentInsights` then reports the job's status —
//! derived from the instant its completion became due, so a caller that polls immediately
//! still sees `IN_PROGRESS` — the location the results were written to, and, for a job that
//! failed, the error that stopped it.
//!
//! Every page here is rendered in the test from known text with the same public-domain
//! 8x8 font the recogniser holds templates for, encoded in a raster format Textract
//! accepts and stored with `PutObject`, so each assertion is a ROUND TRIP through real
//! object bytes and real pixels rather than a comparison against a recorded answer.
//!
//! The tables span what the rules generalise over: which granularity levels the project
//! asks for, whether it asks for geometry, which text formats it asks for, whether an
//! additional file is produced, whether the configuration comes from a project or from
//! blueprints alone, and which part of the request names something that does not exist.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use image::ImageFormat;
use serde_json::{json, Map, Value};
use service_com_amazonaws_textract_textract::state;
use service_com_amazonaws_textract_textract::state::intelligence;
use smithy_server_runtime::MultiServiceRouter;

use crate::raster_documents::{encode, render, Paper};
use crate::s3_sourced_documents::{call, emulator, given_bucket, given_object, BUCKET};

/// The Region both services are addressed in, as `crate::s3_sourced_documents` addresses
/// them: Amazon S3 places a bucket created with no `LocationConstraint` in us-east-1, which
/// is also the Region textract serves a request carrying no credential scope in.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming Amazon S3, which is what routes a raw request to the
/// object store. No authenticator is registered, so the signature is never checked.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// How long a poll loop waits for a job's completion to be reported before calling it a
/// defect.
///
/// Generous on purpose: the job's own span is a fraction of a second, and this bound exists
/// only so a test that will never pass fails instead of hanging. Nothing here asserts on
/// how long anything took.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the loop waits between looks. The loop waits for the CONDITION; this is only
/// how often it looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The paper every staged page is drawn on: large enough for the recogniser to read the
/// characters back exactly, which is what makes the round trip an assertion about the bytes
/// that were stored.
fn paper() -> Paper {
    Paper {
        scale: 4,
        ..Paper::plain()
    }
}

/// The wire error code (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn string_member(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One raw path-style request to the emulator's Amazon S3 front door, with the response's
/// own bytes: this is how a caller fetches the object a job wrote.
async fn s3_get(router: &MultiServiceRouter, key: &str) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://s3.{REGION}.amazonaws.com/{BUCKET}/{key}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::new())
        .expect("the request builds");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.body().to_vec())
}

/// An `ExtractionConfiguration` naming the granularity levels, geometry and text formats
/// given, and whether an additional file is produced.
fn extraction(
    bounding_box: &str,
    levels: &[&str],
    formats: &[&str],
    additional_file: &str,
) -> Value {
    json!({
        "BoundingBox": { "State": bounding_box },
        "Granularity": { "Types": levels },
        "OutputFormat": {
            "AdditionalFileFormat": { "State": additional_file },
            "TextFormat": { "Types": formats },
        },
    })
}

/// A blueprint schema naming `fields`: the JSON object whose `properties` are the fields to
/// extract, which is the form `CreateBlueprint` accepts.
fn schema(fields: &[&str]) -> String {
    let properties: Map<String, Value> = fields
        .iter()
        .map(|field| ((*field).to_string(), json!({ "type": "string" })))
        .collect();
    json!({ "class": "document", "properties": Value::Object(properties) }).to_string()
}

/// `CreateBlueprint`, and the `BlueprintArn` it minted.
async fn create_blueprint(router: &MultiServiceRouter, name: &str, fields: &[&str]) -> String {
    let (status, body) = call(
        router,
        "CreateBlueprint",
        &json!({ "BlueprintName": name, "Schema": schema(fields) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateBlueprint must succeed, got {status} with {body}"
    );
    string_member(&body, "BlueprintArn")
}

/// `CreateProject`, and the `ProjectArn` it minted.
async fn create_project(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, body) = call(router, "CreateProject", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateProject must succeed, got {status} with {body}"
    );
    string_member(&body, "ProjectArn")
}

/// `StartDocumentInsights`, and the `JobArn` it minted.
async fn start_job(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, body) = call(router, "StartDocumentInsights", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartDocumentInsights must succeed, got {status} with {body}"
    );
    let job_arn = string_member(&body, "JobArn");
    // `JobArn`'s modelled pattern is
    // `^arn:aws(...):textract:[a-zA-Z0-9-]*:[0-9]{12}:job/[a-zA-Z0-9-_]+$`.
    let expected_prefix = format!(
        "arn:aws:textract:{REGION}:{}:job/",
        engine::emulator_account_id()
    );
    assert!(
        job_arn.starts_with(&expected_prefix) && job_arn.len() > expected_prefix.len(),
        "case {case:?}: expected a JobArn under {expected_prefix:?}, got {job_arn:?}"
    );
    job_arn
}

/// The identifier a `JobArn` carries, which is what names the objects the job wrote.
fn job_id_of(job_arn: &str) -> String {
    job_arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// Look again at `job_arn` until its completion is reported, and hand back that answer.
///
/// The loop waits for the CONDITION — a status that is no longer `IN_PROGRESS` — rather
/// than for a fixed time, so it passes on a loaded host and on a fast one alike.
async fn settled(router: &MultiServiceRouter, case: &str, job_arn: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = call(
            router,
            "GetDocumentInsights",
            &json!({ "JobArn": job_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetDocumentInsights must succeed, got {status} with {body}"
        );
        if body.get("Status").and_then(Value::as_str) != Some(intelligence::JOB_STATUS_IN_PROGRESS)
        {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reported its completion, last answer {body}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Collapse runs of whitespace to one space.
///
/// A raster carries no characters, only ink: what separates two words on a page is a GAP,
/// which the recogniser reports as the separator it measured.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The entries of an extraction output at one level.
fn elements_of(document: &Value, level: &str) -> Vec<Value> {
    document
        .get("Elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|element| string_member(element, "Type") == level)
        .collect()
}

/// The object a job wrote, parsed as the JSON document it is.
async fn output_document(router: &MultiServiceRouter, case: &str, job_arn: &str) -> Value {
    let key = format!("insights/{}.json", job_id_of(job_arn));
    let (status, body) = s3_get(router, &key).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the job's output object {key:?} must be fetchable, got HTTP {status}"
    );
    serde_json::from_slice::<Value>(&body).unwrap_or_else(|error| {
        panic!("case {case:?}: the job's output must be the JSON document it claims: {error}")
    })
}

/// The lines every staged page below is drawn from: labelled fields, so a blueprint has
/// something to extract.
const PAGE_LINES: &[&str] = &["INVOICE NUMBER: 7781", "TOTAL AMOUNT: 1250.00"];

/// Stage the bucket and one page as the object `key`, and hand back the emulator.
async fn staged(key: &str) -> MultiServiceRouter {
    let router = emulator();
    given_bucket(&router).await;
    let page = render(PAGE_LINES, paper());
    given_object(&router, key, &encode(&page, ImageFormat::Png)).await;
    router
}

/// RULE: a job reads the document its `InputConfiguration` names, extracts it, and writes
/// the result to the location its `OutputConfiguration` named — so the object is really
/// there, and what it carries is the text the document really holds.
#[tokio::test]
async fn a_job_writes_the_extraction_it_derived_from_the_document() {
    let router = staged("insights/invoice.png").await;
    let blueprint = create_blueprint(
        &router,
        "invoice-fields",
        // The schema names a field the page carries, one it carries under a two-word
        // label, and one it does not carry at all.
        &["invoice_number", "total_amount", "vendor_name"],
    )
    .await;
    let project = create_project(
        &router,
        "the extracting project",
        &json!({
            "ProjectName": "insights-project",
            "ExtractionConfiguration": extraction(
                "ENABLED",
                &["DOCUMENT", "PAGE", "LINE", "WORD"],
                &["PLAIN_TEXT", "MARKDOWN", "HTML", "CSV"],
                "ENABLED",
            ),
            "CustomConfiguration": { "Blueprints": [{ "BlueprintArn": blueprint }] },
        }),
    )
    .await;

    let job_arn = start_job(
        &router,
        "the first job",
        &json!({
            "ProjectArn": project,
            "InputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights/invoice.png") },
            "OutputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights") },
        }),
    )
    .await;

    // A job is not done the instant it starts: a caller that polls immediately sees the
    // transient state.
    let (status, polled) = call(
        &router,
        "GetDocumentInsights",
        &json!({ "JobArn": job_arn.clone() }),
    )
    .await;
    assert_eq!(status, 200, "the first poll must succeed, got {polled}");
    assert_eq!(
        polled.get("Status").and_then(Value::as_str),
        Some(intelligence::JOB_STATUS_IN_PROGRESS),
        "a job polled immediately must still be IN_PROGRESS, got {polled}"
    );

    let answer = settled(&router, "the first job", &job_arn).await;
    assert_eq!(
        answer.get("Status").and_then(Value::as_str),
        Some(intelligence::JOB_STATUS_SUCCEEDED),
        "the job must report the completion it reached, got {answer}"
    );
    assert_eq!(
        answer
            .get("OutputConfiguration")
            .map(|output| string_member(output, "S3Uri")),
        Some(format!("s3://{BUCKET}/insights")),
        "the job must report where its results were written, got {answer}"
    );
    assert!(
        answer.get("ErrorType").is_none() && answer.get("ErrorMessage").is_none(),
        "a job that succeeded carries no error, got {answer}"
    );

    let document = output_document(&router, "the first job", &job_arn).await;
    assert_eq!(
        document
            .get("DocumentMetadata")
            .and_then(|metadata| metadata.get("Pages"))
            .and_then(Value::as_i64),
        Some(1),
        "the output must report the page count the document really has, got {document}"
    );

    // The lines the page was drawn from come back as the lines the output reports.
    let lines: Vec<String> = elements_of(&document, "LINE")
        .iter()
        .map(|element| collapse(&string_member(element, "Text")))
        .collect();
    let drawn: Vec<String> = PAGE_LINES.iter().map(|line| collapse(line)).collect();
    assert_eq!(
        lines, drawn,
        "the output must report the lines the page carries, got {lines:?} from {document}"
    );
    for element in elements_of(&document, "LINE") {
        assert!(
            element
                .get("Geometry")
                .and_then(|geometry| geometry.get("BoundingBox"))
                .is_some(),
            "a project asking for geometry gets it on every element, got {element}"
        );
    }
    // Every level the project asked for is present, and the words are the words of those
    // lines.
    for level in ["DOCUMENT", "PAGE", "LINE", "WORD"] {
        assert!(
            !elements_of(&document, level).is_empty(),
            "the output must carry the {level} level the project asked for, got {document}"
        );
    }
    let words: BTreeSet<String> = elements_of(&document, "WORD")
        .iter()
        .map(|element| string_member(element, "Text"))
        .collect();
    assert!(
        words.contains("7781"),
        "the words of the page must be the words it was drawn with, got {words:?}"
    );

    // Each text format the project asked for is a rendering of that same text.
    let renderings = document
        .get("Text")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let asked: BTreeSet<String> = ["PLAIN_TEXT", "MARKDOWN", "HTML", "CSV"]
        .iter()
        .map(|format| (*format).to_string())
        .collect();
    let rendered: BTreeSet<String> = renderings.keys().cloned().collect();
    assert_eq!(
        rendered, asked,
        "the output must carry exactly the text formats the project asked for, got {rendered:?}"
    );
    for (format, rendering) in &renderings {
        let text = rendering.as_str().unwrap_or_default();
        assert!(
            text.contains("7781"),
            "case {format:?}: a rendering must carry the document's own text, got {text:?}"
        );
    }
    assert!(
        renderings
            .get("HTML")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("<p>"),
        "the HTML rendering must be HTML, got {renderings:?}"
    );
    assert!(
        renderings
            .get("MARKDOWN")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("## Page 1"),
        "the Markdown rendering must be Markdown, got {renderings:?}"
    );

    // The blueprint's fields are extracted from the document's own labelled lines: a field
    // the page carries reports the value the page carries for it, and one it does not
    // carry reports none.
    let fields = document
        .get("CustomOutput")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first().cloned())
        .and_then(|entry| entry.get("Fields").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let named = |name: &str| -> Value {
        fields
            .iter()
            .find(|field| string_member(field, "Name") == name)
            .cloned()
            .unwrap_or(Value::Null)
    };
    // The value a field must carry is the part of its own line after the label, as the
    // recogniser read it — taken from the output's own lines, so this compares the custom
    // extraction against the document rather than against a recorded answer.
    let value_after_label = |label: &str| -> String {
        lines
            .iter()
            .find(|line| line.starts_with(label))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default()
    };
    for (name, label) in [
        ("invoice_number", "INVOICE NUMBER"),
        ("total_amount", "TOTAL AMOUNT"),
    ] {
        let field = named(name);
        let expected = value_after_label(label);
        assert!(
            !expected.is_empty(),
            "case {name:?}: the page must carry a value for {label:?}, got {lines:?}"
        );
        assert_eq!(
            field.get("Value").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {name:?}: expected the value the page carries, got {field}"
        );
        let confidence = field
            .get("Confidence")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            confidence > 0.0 && confidence <= 100.0,
            "case {name:?}: a value read off the page carries the confidence it was read \
             with, got {field}"
        );
    }
    let absent = named("vendor_name");
    assert_eq!(
        absent.get("Value"),
        Some(&Value::Null),
        "a field the document does not carry must be reported as absent, got {absent}"
    );

    // `AdditionalFileFormat` ENABLED, so the plain text was written beside the JSON.
    let (status, additional) = s3_get(&router, &format!("insights/{}.txt", job_id_of(&job_arn)))
        .await;
    assert_eq!(
        status, 200,
        "an additional file the project asked for must be written, got HTTP {status}"
    );
    let additional = String::from_utf8(additional).unwrap_or_default();
    assert!(
        additional.contains("7781"),
        "the additional file must carry the document's own text, got {additional:?}"
    );
}

/// One row of the configuration table: what the project asks for, and what the output must
/// then carry.
struct ConfigurationCase {
    name: &'static str,
    bounding_box: &'static str,
    levels: &'static [&'static str],
    formats: &'static [&'static str],
    additional_file: &'static str,
    /// The element `Type`s the output must carry, and no others.
    expect_types: &'static [&'static str],
}

/// RULE: the project's `ExtractionConfiguration` decides what the output carries — the
/// levels content is extracted at, whether each element carries the geometry it was read
/// at, which renderings of the text are produced, and whether an additional file is written
/// beside the primary output.
#[tokio::test]
async fn the_extraction_configuration_decides_what_the_output_carries() {
    let router = staged("insights/configured.png").await;

    // The rows differ along every knob the configuration has, and together they cover each
    // granularity level and each text format the model declares.
    let rows = [
        ConfigurationCase {
            name: "lines alone, with geometry and plain text",
            bounding_box: "ENABLED",
            levels: &["LINE"],
            formats: &["PLAIN_TEXT"],
            additional_file: "DISABLED",
            expect_types: &["LINE"],
        },
        ConfigurationCase {
            name: "words alone, without geometry, as Markdown",
            bounding_box: "DISABLED",
            levels: &["WORD"],
            formats: &["MARKDOWN"],
            additional_file: "DISABLED",
            expect_types: &["WORD"],
        },
        ConfigurationCase {
            name: "the document alone, as plain text and CSV",
            bounding_box: "ENABLED",
            levels: &["DOCUMENT"],
            formats: &["PLAIN_TEXT", "CSV"],
            additional_file: "ENABLED",
            expect_types: &["DOCUMENT"],
        },
        ConfigurationCase {
            name: "pages and structural elements, as HTML",
            bounding_box: "DISABLED",
            levels: &["PAGE", "ELEMENT"],
            formats: &["HTML"],
            additional_file: "DISABLED",
            // An `ELEMENT` level reports the structural items of the page: the labelled
            // lines of this page are each one key-value pair.
            expect_types: &["PAGE", "KEY_VALUE_SET"],
        },
    ];

    let mut covered_levels: BTreeSet<&str> = BTreeSet::new();
    let mut covered_formats: BTreeSet<&str> = BTreeSet::new();
    for (index, case) in rows.iter().enumerate() {
        covered_levels.extend(case.levels.iter().copied());
        covered_formats.extend(case.formats.iter().copied());
        let project = create_project(
            &router,
            case.name,
            &json!({
                "ProjectName": format!("configured-{index}"),
                "ExtractionConfiguration": extraction(
                    case.bounding_box,
                    case.levels,
                    case.formats,
                    case.additional_file,
                ),
            }),
        )
        .await;
        let job_arn = start_job(
            &router,
            case.name,
            &json!({
                "ProjectArn": project,
                "InputConfiguration": {
                    "S3Uri": format!("s3://{BUCKET}/insights/configured.png"),
                },
                "OutputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights") },
            }),
        )
        .await;
        let answer = settled(&router, case.name, &job_arn).await;
        assert_eq!(
            answer.get("Status").and_then(Value::as_str),
            Some(intelligence::JOB_STATUS_SUCCEEDED),
            "case {:?}: the job must succeed, got {answer}",
            case.name
        );

        let document = output_document(&router, case.name, &job_arn).await;
        let types: BTreeSet<String> = document
            .get("Elements")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|element| string_member(element, "Type"))
            .collect();
        let wanted: BTreeSet<String> = case
            .expect_types
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        assert_eq!(
            types, wanted,
            "case {:?}: expected exactly the levels asked for, got {types:?} from {document}",
            case.name
        );
        for element in document
            .get("Elements")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            // The DOCUMENT level is the whole document rather than a region of a page, so
            // it carries no geometry either way; every other element carries it exactly
            // when the project asked for it.
            if string_member(&element, "Type") == "DOCUMENT" {
                continue;
            }
            let has_geometry = element.get("Geometry").is_some();
            assert_eq!(
                has_geometry,
                case.bounding_box == "ENABLED",
                "case {:?}: geometry must follow BoundingBox {:?}, got {element}",
                case.name,
                case.bounding_box
            );
        }
        let rendered: BTreeSet<String> = document
            .get("Text")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
            .keys()
            .cloned()
            .collect();
        let asked: BTreeSet<String> = case
            .formats
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        assert_eq!(
            rendered, asked,
            "case {:?}: expected exactly the text formats asked for, got {rendered:?}",
            case.name
        );
        // A project asking for no blueprint gets no custom output: nothing is invented for
        // a schema nobody supplied.
        assert_eq!(
            document.get("CustomOutput").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {:?}: a project associating no blueprint extracts no custom fields, got \
             {document}",
            case.name
        );

        let (status, _) = s3_get(&router, &format!("insights/{}.txt", job_id_of(&job_arn))).await;
        let expected_status = if case.additional_file == "ENABLED" { 200 } else { 404 };
        assert_eq!(
            status, expected_status,
            "case {:?}: an additional file is written exactly when AdditionalFileFormat is \
             ENABLED, got HTTP {status}",
            case.name
        );
    }

    // The table covers every granularity level and every text format the model declares,
    // so a new one shows up here rather than silently going untested.
    let modelled_levels: BTreeSet<&str> =
        service_com_amazonaws_textract_textract::ops::types::EXTRACTION_GRANULARITY_TYPE_VALUES
            .iter()
            .copied()
            .collect();
    assert_eq!(
        covered_levels, modelled_levels,
        "the table must cover every modelled granularity level, missing {:?}",
        modelled_levels.difference(&covered_levels).collect::<Vec<_>>()
    );
    let modelled_formats: BTreeSet<&str> =
        service_com_amazonaws_textract_textract::ops::types::OUTPUT_TEXT_FORMAT_TYPE_VALUES
            .iter()
            .copied()
            .collect();
    assert_eq!(
        covered_formats, modelled_formats,
        "the table must cover every modelled text format, missing {:?}",
        modelled_formats.difference(&covered_formats).collect::<Vec<_>>()
    );
}

/// RULE: a job can be submitted with blueprints alone, and then it extracts under the
/// default configuration and the schemas those blueprints carry.
#[tokio::test]
async fn a_job_can_be_submitted_with_blueprints_and_no_project() {
    let router = staged("insights/blueprints-only.png").await;
    let blueprint = create_blueprint(&router, "standalone", &["invoice_number"]).await;

    let job_arn = start_job(
        &router,
        "blueprints with no project",
        &json!({
            "Blueprints": [{ "BlueprintArn": blueprint }],
            "InputConfiguration": {
                "S3Uri": format!("s3://{BUCKET}/insights/blueprints-only.png"),
            },
            "OutputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights") },
        }),
    )
    .await;
    let answer = settled(&router, "blueprints with no project", &job_arn).await;
    assert_eq!(
        answer.get("Status").and_then(Value::as_str),
        Some(intelligence::JOB_STATUS_SUCCEEDED),
        "the job must succeed, got {answer}"
    );

    let document = output_document(&router, "blueprints with no project", &job_arn).await;
    // The default configuration extracts at every level the model declares.
    let types: BTreeSet<String> = document
        .get("Elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|element| string_member(element, "Type"))
        .collect();
    for level in ["DOCUMENT", "PAGE", "LINE", "WORD"] {
        assert!(
            types.contains(level),
            "the default configuration extracts the {level} level, got {types:?}"
        );
    }
    // And the blueprint's own field is extracted from the page.
    let value = document
        .get("CustomOutput")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first().cloned())
        .and_then(|entry| entry.get("Fields").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .first()
        .and_then(|field| field.get("Value").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    assert_eq!(
        value, "7781",
        "the blueprint's field must carry the value the page carries, got {value:?} from \
         {document}"
    );
}

/// RULE: a document the service takes on and cannot read ends the job in the modelled
/// failure state, carrying the error that stopped it — rather than an empty success.
#[tokio::test]
async fn a_document_that_cannot_be_read_fails_the_job_with_its_reason() {
    let router = emulator();
    given_bucket(&router).await;
    // Rows differ along WHY the bytes carry no document: bytes in no format at all, and
    // bytes that begin like an accepted raster and are truncated.
    let page = render(PAGE_LINES, paper());
    let truncated = {
        let mut bytes = encode(&page, ImageFormat::Png);
        bytes.truncate(bytes.len() / 3);
        bytes
    };
    let rows: Vec<(&str, &str, Vec<u8>)> = vec![
        (
            "bytes in no document format at all",
            "insights/not-a-document.bin",
            b"this is not a document".to_vec(),
        ),
        (
            "a raster truncated part way through",
            "insights/truncated.png",
            truncated,
        ),
    ];
    let blueprint = create_blueprint(&router, "unreadable", &["invoice_number"]).await;

    for (case, key, body) in &rows {
        given_object(&router, key, body).await;
        let job_arn = start_job(
            &router,
            case,
            &json!({
                "Blueprints": [{ "BlueprintArn": blueprint.clone() }],
                "InputConfiguration": { "S3Uri": format!("s3://{BUCKET}/{key}") },
                "OutputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights") },
            }),
        )
        .await;
        let answer = settled(&router, case, &job_arn).await;
        assert_eq!(
            answer.get("Status").and_then(Value::as_str),
            Some(intelligence::JOB_STATUS_FAILED),
            "case {case:?}: a document that cannot be read fails the job, got {answer}"
        );
        assert_eq!(
            answer.get("ErrorType").and_then(Value::as_str),
            Some("UnsupportedDocumentException"),
            "case {case:?}: the error must name the modelled reason, got {answer}"
        );
        assert_eq!(
            answer.get("ErrorMessage").and_then(Value::as_str),
            Some(state::UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE),
            "case {case:?}: expected the sentence Amazon Textract states this with, got {answer}"
        );
        // A failed job wrote no output, so nothing is reported as having been written.
        let (status, _) = s3_get(&router, &format!("insights/{}.json", job_id_of(&job_arn))).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a failed job must leave no output object, got HTTP {status}"
        );
    }
}

/// RULE: a submit that names no configuration to run under, or names a resource this
/// account does not hold, or a document the object store does not hold, is refused — and
/// creates no job.
#[tokio::test]
async fn a_submit_that_names_nothing_usable_is_refused() {
    let router = staged("insights/refusals.png").await;
    let blueprint = create_blueprint(&router, "refusal-blueprint", &["field_one"]).await;
    let project = create_project(
        &router,
        "a project for the refusal table",
        &json!({
            "ProjectName": "refusal-project",
            "ExtractionConfiguration": extraction(
                "ENABLED",
                &["LINE"],
                &["PLAIN_TEXT"],
                "DISABLED",
            ),
        }),
    )
    .await;
    let account = engine::emulator_account_id();
    let input = json!({ "S3Uri": format!("s3://{BUCKET}/insights/refusals.png") });
    let output = json!({ "S3Uri": format!("s3://{BUCKET}/insights") });

    // Rows differ along WHICH part of the request names nothing usable.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "neither a project nor a blueprint",
            json!({ "InputConfiguration": input.clone(), "OutputConfiguration": output.clone() }),
            "ValidationException",
        ),
        (
            "an empty blueprint list and no project",
            json!({
                "Blueprints": [],
                "InputConfiguration": input.clone(),
                "OutputConfiguration": output.clone(),
            }),
            "ValidationException",
        ),
        (
            "a project this account does not hold",
            json!({
                "ProjectArn": format!(
                    "arn:aws:textract:{REGION}:{account}:project/{}",
                    "a".repeat(12)
                ),
                "InputConfiguration": input.clone(),
                "OutputConfiguration": output.clone(),
            }),
            "ResourceNotFoundException",
        ),
        (
            "a blueprint this account does not hold",
            json!({
                "Blueprints": [{
                    "BlueprintArn": format!(
                        "arn:aws:textract:{REGION}:{account}:blueprint/{}",
                        "b".repeat(12)
                    ),
                }],
                "InputConfiguration": input.clone(),
                "OutputConfiguration": output.clone(),
            }),
            "ResourceNotFoundException",
        ),
        (
            "a held blueprint pinned to a version it never had",
            json!({
                "Blueprints": [{ "BlueprintArn": blueprint.clone(), "BlueprintVersion": "4" }],
                "InputConfiguration": input.clone(),
                "OutputConfiguration": output.clone(),
            }),
            "ResourceNotFoundException",
        ),
        (
            "a document the object store does not hold",
            json!({
                "ProjectArn": project.clone(),
                "InputConfiguration": {
                    "S3Uri": format!("s3://{BUCKET}/insights/never-staged.png"),
                },
                "OutputConfiguration": output.clone(),
            }),
            "InvalidParameterException",
        ),
        (
            "a bucket the object store does not hold",
            json!({
                "ProjectArn": project.clone(),
                "InputConfiguration": { "S3Uri": "s3://textract-no-such-bucket/page.png" },
                "OutputConfiguration": output.clone(),
            }),
            "InvalidParameterException",
        ),
    ];

    for (case, request, expected) in &rows {
        let (status, body) = call(&router, "StartDocumentInsights", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert!(
            body.get("JobArn").is_none(),
            "case {case:?}: a refused submit must create no job, got {body}"
        );
    }
    // The sentence AWS's own documentation grounds the missing-configuration refusal in.
    let (_, body) = call(
        &router,
        "StartDocumentInsights",
        &json!({ "InputConfiguration": input, "OutputConfiguration": output }),
    )
    .await;
    assert_eq!(
        body.get("Message").and_then(Value::as_str),
        Some(intelligence::NO_CONFIGURATION_SOURCE_MESSAGE),
        "expected the missing-configuration wording, got {body}"
    );
}

/// RULE: a `JobArn` no `StartDocumentInsights` of this service issued names no job, which is
/// the modelled `InvalidJobIdException` — the same refusal, in the same words, every other
/// `Get*` of this service answers an unknown job with.
#[tokio::test]
async fn a_job_arn_that_names_nothing_is_refused() {
    let router = staged("insights/known.png").await;
    let blueprint = create_blueprint(&router, "known-blueprint", &["invoice_number"]).await;
    let live = start_job(
        &router,
        "the one real job",
        &json!({
            "Blueprints": [{ "BlueprintArn": blueprint }],
            "InputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights/known.png") },
            "OutputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights") },
        }),
    )
    .await;

    // Rows span the identifier forms `JobArn`'s modelled pattern allows
    // (`job/[a-zA-Z0-9-_]+`), including a real one with its last character changed.
    let account = engine::emulator_account_id();
    let mut flipped = live.clone();
    flipped.pop();
    flipped.push(if live.ends_with('0') { '1' } else { '0' });
    let rows: Vec<(&str, String)> = vec![
        (
            "a one-character identifier",
            format!("arn:aws:textract:{REGION}:{account}:job/x"),
        ),
        (
            "a plausible 32-character identifier",
            format!("arn:aws:textract:{REGION}:{account}:job/{}", "c".repeat(32)),
        ),
        (
            "an identifier using the pattern's - and _ characters",
            format!("arn:aws:textract:{REGION}:{account}:job/a-job_id"),
        ),
        ("a real identifier with its last character changed", flipped),
    ];
    for (case, job_arn) in &rows {
        assert_ne!(
            job_arn, &live,
            "case {case:?}: the table must not name the job that exists"
        );
        let (status, body) =
            call(&router, "GetDocumentInsights", &json!({ "JobArn": job_arn })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidJobIdException",
            "case {case:?}: expected InvalidJobIdException, got {body}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(state::INVALID_JOB_ID_MESSAGE),
            "case {case:?}: expected the sentence this service states this with, got {body}"
        );
    }
}

/// RULE: a project asking for a capability this emulator does not have still gets the
/// extraction it CAN derive, and the job says so: the modelled `PARTIAL_SUCCESS`, with
/// `ErrorType` and `ErrorMessage` naming exactly the members that asked — never an
/// unqualified success for work nothing did.
#[tokio::test]
async fn a_configuration_this_emulator_does_not_carry_out_is_reported_not_claimed() {
    let router = staged("insights/partial.png").await;

    // Rows differ along WHICH capability the project asks for, including one row asking for
    // several at once and one asking for none of them.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a generative summary",
            json!({ "GenerativeConfiguration": { "Summarization": { "State": "ENABLED" } } }),
            vec!["GenerativeConfiguration.Summarization"],
        ),
        (
            "generative figure captions",
            json!({ "GenerativeConfiguration": { "FigureCaption": { "State": "ENABLED" } } }),
            vec!["GenerativeConfiguration.FigureCaption"],
        ),
        (
            "PII detection alone",
            json!({ "SensitiveDataConfiguration": { "DetectionMode": "DETECTION" } }),
            vec!["SensitiveDataConfiguration"],
        ),
        (
            "PII detection and redaction",
            json!({
                "SensitiveDataConfiguration": { "DetectionMode": "DETECTION_AND_REDACTION" },
            }),
            vec!["SensitiveDataConfiguration"],
        ),
        (
            "splitting a multi-document packet",
            json!({ "SplitterConfiguration": { "State": "ENABLED" } }),
            vec!["SplitterConfiguration"],
        ),
        (
            "every one of them at once",
            json!({
                "GenerativeConfiguration": {
                    "Summarization": { "State": "ENABLED" },
                    "FigureCaption": { "State": "ENABLED" },
                },
                "SensitiveDataConfiguration": { "DetectionMode": "DETECTION_AND_REDACTION" },
                "SplitterConfiguration": { "State": "ENABLED" },
            }),
            vec![
                "GenerativeConfiguration.Summarization",
                "GenerativeConfiguration.FigureCaption",
                "SensitiveDataConfiguration",
                "SplitterConfiguration",
            ],
        ),
        (
            "the same configurations all turned off, which ask for nothing",
            json!({
                "GenerativeConfiguration": {
                    "Summarization": { "State": "DISABLED" },
                    "FigureCaption": { "State": "DISABLED" },
                },
                "SensitiveDataConfiguration": { "DetectionMode": "DISABLED" },
                "SplitterConfiguration": { "State": "DISABLED" },
            }),
            vec![],
        ),
    ];

    for (index, (case, configuration, expected)) in rows.iter().enumerate() {
        let mut request = json!({
            "ProjectName": format!("partial-{index}"),
            "ExtractionConfiguration": extraction("ENABLED", &["LINE"], &["PLAIN_TEXT"], "DISABLED"),
        });
        for (member, value) in configuration.as_object().cloned().unwrap_or_default() {
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert(member, value);
        }
        let project = create_project(&router, case, &request).await;
        let job_arn = start_job(
            &router,
            case,
            &json!({
                "ProjectArn": project,
                "InputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights/partial.png") },
                "OutputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights") },
            }),
        )
        .await;
        let answer = settled(&router, case, &job_arn).await;

        // Either way the extraction that COULD be derived is written: the job is partial,
        // not failed.
        let document = output_document(&router, case, &job_arn).await;
        assert!(
            !elements_of(&document, "LINE").is_empty(),
            "case {case:?}: the extraction that could be derived must still be written, got \
             {document}"
        );

        if expected.is_empty() {
            assert_eq!(
                answer.get("Status").and_then(Value::as_str),
                Some(intelligence::JOB_STATUS_SUCCEEDED),
                "case {case:?}: a project asking for nothing unsupported succeeds outright, \
                 got {answer}"
            );
            assert!(
                answer.get("ErrorType").is_none() && answer.get("ErrorMessage").is_none(),
                "case {case:?}: an unqualified success carries no reason, got {answer}"
            );
            continue;
        }
        assert_eq!(
            answer.get("Status").and_then(Value::as_str),
            Some(intelligence::JOB_STATUS_PARTIAL_SUCCESS),
            "case {case:?}: expected the modelled PARTIAL_SUCCESS, got {answer}"
        );
        assert_eq!(
            answer.get("ErrorType").and_then(Value::as_str),
            Some("UnsupportedConfiguration"),
            "case {case:?}: the reason must name the class, got {answer}"
        );
        // The message names exactly the members that asked, taken from the project's own
        // configuration.
        assert_eq!(
            answer.get("ErrorMessage").and_then(Value::as_str),
            Some(
                format!(
                    "The extraction output was written. Amazon Textract did not apply: {}.",
                    expected.join(", ")
                )
                .as_str()
            ),
            "case {case:?}: expected the members the project asked for, got {answer}"
        );
    }
}

/// RULE: `GetDocumentInsights` is a read — repeating it, and starting other jobs in
/// between, never changes what it answers.
#[tokio::test]
async fn get_document_insights_does_not_change_what_a_later_caller_sees() {
    let router = staged("insights/stable.png").await;
    let blueprint = create_blueprint(&router, "stable-blueprint", &["invoice_number"]).await;
    let request = json!({
        "Blueprints": [{ "BlueprintArn": blueprint }],
        "InputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights/stable.png") },
        "OutputConfiguration": { "S3Uri": format!("s3://{BUCKET}/insights") },
    });
    let job_arn = start_job(&router, "the job under test", &request).await;
    let first = settled(&router, "the job under test", &job_arn).await;

    for index in 0..3u32 {
        let case = format!("after {index} intervening jobs");
        let (status, repeat) = call(
            &router,
            "GetDocumentInsights",
            &json!({ "JobArn": job_arn.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated read must succeed, got {status} with {repeat}"
        );
        assert_eq!(
            repeat, first,
            "case {case:?}: a read must answer identically, got {repeat} after {first}"
        );
        let other = start_job(&router, &case, &request).await;
        assert_ne!(
            other, job_arn,
            "case {case:?}: every job needs its own JobArn"
        );
    }
}
