// SPDX-License-Identifier: CC0-1.0

//! What a finished data-automation job WRITES into the output location its submission
//! named, read back out of the object store that owns objects.
//!
//! Amazon S3 is mounted beside this service on one engine — what `core/router` assembles —
//! because the whole rule is about objects: the asset a job reads is one, and the output it
//! writes is one too.
//!
//! THE RULE: a successful asynchronous data-automation job materialises its extraction
//! output into the `outputConfiguration.s3Uri` the submission named, under a segment of its
//! own `invocationId` — the per-asset standard output at
//! `<prefix>/<invocationId>/<assetIndex>/standard_output/<subdocumentIndex>/result.json`,
//! carrying that asset's `metadata` and the `representation` of what was extracted from it,
//! plus the `job_metadata.json` index at the top of the segment whose location the status
//! response reports — and both objects are in the store before any read reports the job
//! `Success`, because a client polls to the terminal status and then READS them. What is
//! written is what this process really extracted: an asset it extracts no text from gets its
//! metadata and no representation, and a job that failed writes nothing at all.
//!
//! The rows differ along what the rule generalises over: the asset's FORMAT and so its
//! modality and file type (a PDF, a PDF of several pages, UTF-8 text, a PNG image), how many
//! PAGES it holds (one and two, which is what the page members report), and the shape of the
//! submitted output prefix (the bucket root, one segment, a deep prefix, one written with a
//! trailing separator).
//!
//! The objects are read back over Amazon S3's own wire protocol, so the stored-object headers
//! a client sees — its media type, its checksum, its encryption — are the object store's own
//! answer rather than anything this service states.

use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use service_com_amazonaws_s3_amazons3 as s3;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`).
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for: the handler reads the region out of the SigV4
/// credential scope and addresses the object store in it, so every call here agrees on one.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// The data-automation project every submission below names — the member that says WHAT to
/// extract. Arranged in the owning (build-time) service's store, because a submission naming
/// a project nobody created is refused before any rule here could be reached.
const PROJECT_ARN: &str = "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices";

/// The bucket the input assets are staged in.
const ASSET_BUCKET: &str = "bda-input-assets";

/// The bucket every job below writes its output into.
const OUTPUT_BUCKET: &str = "bda-job-output";

/// The media type an object written with no `Content-Type` of its own carries (Amazon S3's
/// documented default for an upload that specifies none) — what a client reads a job's
/// output document with.
const DEFAULT_MEDIA_TYPE: &str = "binary/octet-stream";

/// The `ChecksumType` a single whole-object write settles: there are no parts to compose a
/// checksum from.
const FULL_OBJECT: &str = "FULL_OBJECT";

/// The header the CRC32 checksum of a stored object travels on.
const CRC32_HEADER: &str = "x-amz-checksum-crc32";

/// The server-side encryption Amazon S3 applies to a new object by default (SSE-S3).
const SSE_S3: &str = "AES256";

/// How long a poll may wait for a job to reach a terminal status.
///
/// GENEROUS on purpose: the emulated lifecycle spans are a fraction of a second, and this is
/// only the deadline that stops a broken build from hanging — the loop below waits for the
/// CONDITION, not for a duration, so a loaded machine takes longer and still passes.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll sleeps before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One emulator: this service and Amazon S3 on a single shared engine behind the production
/// router, with the referenced project arranged. The engine is handed back as well, so a case
/// that has to ARRANGE a record reaches the very store the handlers serve from.
async fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "data-automation-invoices");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(s3::EmulatedService::new(Arc::clone(&engine)));
    for bucket in [ASSET_BUCKET, OUTPUT_BUCKET] {
        // A region-specific endpoint needs the location constraint stated: "the unspecified
        // location constraint is incompatible for the region specific endpoint this request
        // was sent to" is what the object store answers a create without one.
        let configuration = format!(
            "<CreateBucketConfiguration><LocationConstraint>{REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        );
        let answer = s3_call(
            &router,
            "PUT",
            &format!("/{bucket}"),
            &[],
            configuration.as_bytes(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "the bucket {bucket:?} must be created, got {} / {}",
            answer.status,
            answer.text()
        );
    }
    (router, engine)
}

/// One raw Amazon S3 answer: the status, the response headers and the body.
struct ObjectAnswer {
    status: u16,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

impl ObjectAnswer {
    /// One response header's value, or `None` when the answer carries no such header.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
    }

    /// The body as text, for an answer a case reports in an assertion message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, or `Value::Null` when it is not a JSON document.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// One path-style request to the emulator's own Amazon S3 front door: the bucket and the key
/// live in the URI, the body is the object's own bytes. The credential scope names `s3`,
/// which is what claims it.
async fn s3_call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> ObjectAnswer {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    ObjectAnswer {
        status: response.status().as_u16(),
        headers: response.headers().clone(),
        body: response.body().as_ref().to_vec(),
    }
}

/// Stage one input asset at `key` in [`ASSET_BUCKET`], the way a caller uploads it before
/// submitting.
async fn stage_asset(router: &MultiServiceRouter, case: &str, key: &str, bytes: &[u8]) {
    let answer = s3_call(
        router,
        "PUT",
        &format!("/{ASSET_BUCKET}/{key}"),
        &[],
        bytes,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the asset {key:?} must be staged, got {} / {}",
        answer.status,
        answer.text()
    );
}

/// Read one object back with the checksum mode enabled, which is what asks Amazon S3 for the
/// checksum it stored ("to retrieve the checksum, this mode must be enabled",
/// `GetObjectRequest$ChecksumMode`).
async fn read_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> ObjectAnswer {
    s3_call(
        router,
        "GET",
        &format!("/{bucket}/{key}"),
        &[("x-amz-checksum-mode", "ENABLED")],
        &[],
    )
    .await
}

/// One answer from this service: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

/// Call one operation of this service the way an SDK would (awsJson1_1: `POST /` with
/// `X-Amz-Target`).
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Submit one invocation reading `input_s3_uri` and writing under `output_s3_uri`, and return
/// its `invocationArn`.
async fn submit(
    router: &MultiServiceRouter,
    case: &str,
    input_s3_uri: &str,
    output_s3_uri: &str,
) -> String {
    let answer = call(
        router,
        "InvokeDataAutomationAsync",
        json!({
            "inputConfiguration": {"s3Uri": input_s3_uri},
            "outputConfiguration": {"s3Uri": output_s3_uri},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
        .to_string()
}

/// Poll `GetDataAutomationStatus` until the job reaches a terminal status, and answer the
/// FIRST body that reported it — so every assertion about the job's output is made against
/// the very read a polling client would have acted on.
async fn poll_until_terminal(router: &MultiServiceRouter, case: &str, invocation_arn: &str) -> Value {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let answer = call(
            router,
            "GetDataAutomationStatus",
            json!({"invocationArn": invocation_arn}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
            answer.status, answer.body
        );
        let status = answer.body["status"].as_str().unwrap_or_default();
        if matches!(status, "Success" | "ClientError" | "ServiceError") {
            return answer.body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reached a terminal status, last read {}",
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `invocationId` inside a minted invocation ARN.
fn invocation_id(invocation_arn: &str) -> &str {
    crate::support::invocation_id(invocation_arn)
}

/// `text` with every whitespace character removed, so an assertion about WHAT a document says
/// does not depend on how the extraction spaced it.
fn squeezed(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// A PDF document of `pages`, each page showing its own lines in the Base-14 Helvetica face
/// every PDF reader carries (nothing is embedded, so the document stands alone).
///
/// Written by the `pdf-writer` crate, so the asset a job reads is a document a PDF parser
/// opens rather than bytes shaped like one.
fn pdf_document(pages: &[&[&str]]) -> Vec<u8> {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

    /// The ISO 216 A4 page box in PDF points (1/72 inch).
    const A4: [f32; 4] = [0.0, 0.0, 595.28, 841.89];
    /// Point size of the body text, and the baseline-to-baseline distance with it.
    const FONT_SIZE: f32 = 12.0;
    /// See [`FONT_SIZE`].
    const LEADING: f32 = 16.0;

    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let font_id = Ref::new(3);
    let font_name = Name(b"F1");

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    // Two object ids per page: the page and its content stream, after the three above.
    let page_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(4 + 2 * i32::try_from(index).expect("a small page count")))
        .collect();
    let content_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(5 + 2 * i32::try_from(index).expect("a small page count")))
        .collect();
    pdf.pages(page_tree_id)
        .kids(page_ids.iter().copied())
        .count(i32::try_from(pages.len()).expect("a small page count"));
    for ((lines, page_id), content_id) in pages.iter().zip(&page_ids).zip(&content_ids) {
        let mut content = Content::new();
        content.begin_text();
        content.set_font(font_name, FONT_SIZE);
        content.set_leading(LEADING);
        content.next_line(56.0, 785.0);
        for line in lines.iter() {
            content.show(Str(line.as_bytes()));
            content.next_line(0.0, -LEADING);
        }
        content.end_text();
        let content = content.finish();

        let mut page = pdf.page(*page_id);
        page.parent(page_tree_id);
        page.media_box(Rect::new(A4[0], A4[1], A4[2], A4[3]));
        page.contents(*content_id);
        page.resources().fonts().pair(font_name, font_id);
        page.finish();
        pdf.stream(*content_id, &content);
    }
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    pdf.finish()
}

/// A PNG image `side` pixels square, encoded by the `image` crate — an asset of the IMAGE
/// modality, inside the dimensions data automation processes.
fn png_image(side: u32) -> Vec<u8> {
    let image = ::image::RgbImage::from_fn(side, side, |x, y| {
        ::image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut bytes: Vec<u8> = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ::image::ImageFormat::Png)
        .expect("the test image encodes as PNG");
    bytes
}

/// One row: the asset a job reads, and what its standard output must then report.
struct Case {
    /// The key the asset is staged under.
    key: &'static str,
    /// The asset's bytes.
    bytes: Vec<u8>,
    /// The `outputConfiguration.s3Uri` the submission names.
    output_s3_uri: String,
    /// The prefix the job's own segment hangs off inside [`OUTPUT_BUCKET`] — empty for a
    /// submission that named the bucket root.
    output_prefix: &'static str,
    /// The `semantic_modality` the metadata must report.
    modality: &'static str,
    /// The `file_type` the metadata must report.
    file_type: &'static str,
    /// How many pages the metadata must report, or `None` for an asset whose pages this
    /// process does not count because it extracts no text from it.
    pages: Option<usize>,
    /// Text the extracted representation must carry, ignoring how it was spaced.
    carries: &'static [&'static str],
}

#[tokio::test]
async fn a_finished_jobs_standard_output_is_readable_under_the_submitted_prefix() {
    let (router, _engine) = emulator().await;

    let rows: &[(&str, Case)] = &[
        (
            "a one-page PDF under a deep prefix",
            Case {
                key: "input/invoice.pdf",
                bytes: pdf_document(&[&["Invoice 4821 for Pelagic Shipping", "Total due 1200 USD"]]),
                output_s3_uri: format!("s3://{OUTPUT_BUCKET}/team/invoices/2026"),
                output_prefix: "team/invoices/2026",
                modality: "DOCUMENT",
                file_type: "PDF",
                pages: Some(1),
                carries: &["Invoice4821forPelagicShipping", "Totaldue1200USD"],
            },
        ),
        (
            "a two-page PDF under one prefix segment with a trailing separator",
            Case {
                key: "input/statement.pdf",
                bytes: pdf_document(&[
                    &["Statement page one of two", "Opening balance 40 USD"],
                    &["Statement page two of two", "Closing balance 95 USD"],
                ]),
                output_s3_uri: format!("s3://{OUTPUT_BUCKET}/results/"),
                output_prefix: "results",
                modality: "DOCUMENT",
                file_type: "PDF",
                pages: Some(2),
                carries: &["Openingbalance40USD", "Closingbalance95USD"],
            },
        ),
        (
            "a UTF-8 text asset at the bucket root",
            Case {
                key: "notes.txt",
                bytes: "Delivery note 77\nSigned by Ada Lovelace\n".as_bytes().to_vec(),
                output_s3_uri: format!("s3://{OUTPUT_BUCKET}"),
                output_prefix: "",
                modality: "DOCUMENT",
                file_type: "TXT",
                pages: Some(1),
                carries: &["Deliverynote77", "SignedbyAdaLovelace"],
            },
        ),
        (
            "a PNG image, a modality this emulator extracts no text from",
            Case {
                key: "input/scan.png",
                bytes: png_image(96),
                output_s3_uri: format!("s3://{OUTPUT_BUCKET}/images"),
                output_prefix: "images",
                modality: "IMAGE",
                file_type: "PNG",
                pages: None,
                carries: &[],
            },
        ),
    ];

    for (case, row) in rows {
        stage_asset(&router, case, row.key, &row.bytes).await;
        let input_s3_uri = format!("s3://{ASSET_BUCKET}/{}", row.key);
        let arn = submit(&router, case, &input_s3_uri, &row.output_s3_uri).await;
        let id = invocation_id(&arn).to_string();

        let status = poll_until_terminal(&router, case, &arn).await;
        assert_eq!(
            status["status"].as_str(),
            Some("Success"),
            "case {case:?}: a job whose asset the store holds must succeed, got {status}"
        );

        // The keys the layout puts this job's output at: the asset segment's standard
        // output, and the index at the top of the job's own segment.
        let segment = if row.output_prefix.is_empty() {
            id.clone()
        } else {
            format!("{}/{id}", row.output_prefix)
        };
        let result_key = format!("{segment}/0/standard_output/0/result.json");
        let index_key = format!("{segment}/job_metadata.json");

        // The status answer's reported location IS the index object — and it is readable,
        // from the very read that first reported the job finished.
        assert_eq!(
            status["outputConfiguration"]["s3Uri"].as_str(),
            Some(format!("s3://{OUTPUT_BUCKET}/{index_key}").as_str()),
            "case {case:?}: the reported output location must name this job's index object, \
             got {status}"
        );

        let result = read_object(&router, OUTPUT_BUCKET, &result_key).await;
        assert_eq!(
            result.status, 200,
            "case {case:?}: the standard output at {result_key:?} must be readable as soon as \
             the job reports Success, got {} / {}",
            result.status,
            result.text()
        );

        // The ordinary headers of a stored object, which is what a client reading a job's
        // output gets: the store's own default media type (the job names none), the checksum
        // the write asked for over the whole object, the account's default encryption, and
        // the identity and range members every stored object carries.
        for (header, want) in [
            ("content-type", DEFAULT_MEDIA_TYPE),
            ("accept-ranges", "bytes"),
            ("x-amz-checksum-type", FULL_OBJECT),
            ("x-amz-server-side-encryption", SSE_S3),
        ] {
            assert_eq!(
                result.header(header).as_deref(),
                Some(want),
                "case {case:?}: expected {header} {want:?} on the standard output, got {:?}",
                result.header(header)
            );
        }
        for header in [CRC32_HEADER, "etag", "last-modified"] {
            assert!(
                result.header(header).is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected {header} present on the standard output, got {:?}",
                result.header(header)
            );
        }

        let body = result.json();
        let metadata = &body["metadata"];
        for (member, want) in [
            ("asset_id", json!("0")),
            ("logical_subdocument_id", json!("0")),
            ("semantic_modality", json!(row.modality)),
            ("s3_bucket", json!(ASSET_BUCKET)),
            ("s3_key", json!(row.key)),
            ("file_type", json!(row.file_type)),
        ] {
            assert_eq!(
                metadata[member], want,
                "case {case:?}: expected metadata.{member} {want}, got {}",
                metadata[member]
            );
        }
        match row.pages {
            Some(pages) => {
                for (member, want) in [
                    ("number_of_pages", json!(pages)),
                    ("start_page_index", json!(0)),
                    ("end_page_index", json!(pages - 1)),
                ] {
                    assert_eq!(
                        metadata[member], want,
                        "case {case:?}: expected metadata.{member} {want}, got {}",
                        metadata[member]
                    );
                }
                let text = body[row.modality.to_lowercase()]["representation"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                for wanted in row.carries {
                    assert!(
                        squeezed(&text).contains(*wanted),
                        "case {case:?}: expected the extracted text to carry {wanted:?}, got {text:?}"
                    );
                }
                assert_eq!(
                    body[row.modality.to_lowercase()]["statistics"],
                    json!({}),
                    "case {case:?}: the statistics member must state no count this emulator did \
                     not compute, got {body}"
                );
            }
            None => {
                // Nothing was extracted, so nothing is claimed: no page range and no
                // representation, while the metadata that IS a fact is still there.
                for member in ["number_of_pages", "start_page_index", "end_page_index"] {
                    assert!(
                        metadata[member].is_null(),
                        "case {case:?}: an asset whose pages nothing counted must claim no \
                         {member}, got {body}"
                    );
                }
                assert!(
                    body[row.modality.to_lowercase()].is_null(),
                    "case {case:?}: an asset nothing was extracted from must carry no \
                     representation, got {body}"
                );
            }
        }

        // The index at the top of the job's segment names the standard output that is there.
        let index = read_object(&router, OUTPUT_BUCKET, &index_key).await;
        assert_eq!(
            index.status, 200,
            "case {case:?}: the job index at {index_key:?} must be readable, got {} / {}",
            index.status,
            index.text()
        );
        let index_body = index.json();
        assert_eq!(
            index_body["job_id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the index must name the job it belongs to, got {index_body}"
        );
        assert_eq!(
            index_body["output_metadata"][0]["segment_metadata"][0]["standard_output_path"].as_str(),
            Some(format!("s3://{OUTPUT_BUCKET}/{result_key}").as_str()),
            "case {case:?}: the index must name the standard output of this job's asset, got \
             {index_body}"
        );

        // A read changes nothing a later caller sees: the object and the reported location
        // stay as they were.
        let again = poll_until_terminal(&router, case, &arn).await;
        assert_eq!(
            again["outputConfiguration"], status["outputConfiguration"],
            "case {case:?}: a later read must report the same location"
        );
        let reread = read_object(&router, OUTPUT_BUCKET, &result_key).await;
        assert_eq!(
            reread.json(),
            body,
            "case {case:?}: the standard output must not change between reads"
        );
    }
}

#[tokio::test]
async fn two_jobs_under_one_prefix_write_their_own_output() {
    // The prefix a submission names is shared; the segment a job's output lives under is its
    // own, so one job's output can never overwrite another's.
    let (router, _engine) = emulator().await;
    let case = "two jobs, one prefix";
    let key = "input/shared.pdf";
    stage_asset(
        &router,
        case,
        key,
        &pdf_document(&[&["Shared prefix document"]]),
    )
    .await;
    let input = format!("s3://{ASSET_BUCKET}/{key}");
    let prefix = format!("s3://{OUTPUT_BUCKET}/shared/");

    let first = submit(&router, case, &input, &prefix).await;
    let second = submit(&router, case, &input, &prefix).await;
    assert_ne!(
        invocation_id(&first),
        invocation_id(&second),
        "case {case:?}: two submissions must mint two jobs"
    );

    for arn in [&first, &second] {
        let status = poll_until_terminal(&router, case, arn).await;
        assert_eq!(
            status["status"].as_str(),
            Some("Success"),
            "case {case:?}: both jobs must succeed, got {status}"
        );
        let result_key = format!(
            "shared/{}/0/standard_output/0/result.json",
            invocation_id(arn)
        );
        let answer = read_object(&router, OUTPUT_BUCKET, &result_key).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: each job must write its own standard output at {result_key:?}, got {} / {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.json()["metadata"]["s3_key"].as_str(),
            Some(key),
            "case {case:?}: each job's output must name the asset it processed, got {}",
            answer.text()
        );
    }
}

#[tokio::test]
async fn a_job_that_failed_writes_no_output_at_all() {
    // The other half of the rule: output is written for work that was done. A job whose
    // asset the store holds none of fails, and nothing is written under its segment — a
    // client that reads it gets the object store's own absent-object answer, not an empty
    // document.
    let (router, _engine) = emulator().await;
    let case = "an asset the store holds none of";
    let arn = submit(
        &router,
        case,
        &format!("s3://{ASSET_BUCKET}/input/never-staged.pdf"),
        &format!("s3://{OUTPUT_BUCKET}/failed"),
    )
    .await;
    let status = poll_until_terminal(&router, case, &arn).await;
    assert_eq!(
        status["status"].as_str(),
        Some("ClientError"),
        "case {case:?}: a job that cannot read its asset must fail, got {status}"
    );

    let id = invocation_id(&arn);
    for key in [
        format!("failed/{id}/0/standard_output/0/result.json"),
        format!("failed/{id}/job_metadata.json"),
    ] {
        let answer = read_object(&router, OUTPUT_BUCKET, &key).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a failed job must leave {key:?} unwritten, got {} / {}",
            answer.status,
            answer.text()
        );
    }
}

#[tokio::test]
async fn an_asset_nothing_could_be_read_out_of_still_leaves_the_reported_index_readable() {
    // The location a status answer reports must be readable once the job finished, whatever
    // the asset turned out to be: an asset nothing recognised yields an index that names no
    // standard output — the honest index of no output — rather than a reported location
    // nothing answers for.
    let (router, _engine) = emulator().await;
    let case = "bytes nothing recognises";
    let key = "input/opaque.bin";
    // Neither a PDF header, nor any image format's magic numbers, nor UTF-8 text.
    stage_asset(&router, case, key, &[0x00, 0xFF, 0xFE, 0x01, 0x80, 0x7F]).await;
    let arn = submit(
        &router,
        case,
        &format!("s3://{ASSET_BUCKET}/{key}"),
        &format!("s3://{OUTPUT_BUCKET}/opaque"),
    )
    .await;
    let status = poll_until_terminal(&router, case, &arn).await;
    assert_eq!(
        status["status"].as_str(),
        Some("Success"),
        "case {case:?}: an asset the store holds lets the job finish, got {status}"
    );

    let id = invocation_id(&arn).to_string();
    let index = read_object(
        &router,
        OUTPUT_BUCKET,
        &format!("opaque/{id}/job_metadata.json"),
    )
    .await;
    assert_eq!(
        index.status, 200,
        "case {case:?}: the location the status answer reports must be readable, got {} / {}",
        index.status,
        index.text()
    );
    let body = index.json();
    assert_eq!(
        body["job_id"].as_str(),
        Some(id.as_str()),
        "case {case:?}: the index must name the job it belongs to, got {body}"
    );
    assert_eq!(
        body["output_metadata"],
        json!([]),
        "case {case:?}: an index must name no standard output for an asset nothing was read \
         out of, got {body}"
    );
    let result = read_object(
        &router,
        OUTPUT_BUCKET,
        &format!("opaque/{id}/0/standard_output/0/result.json"),
    )
    .await;
    assert_eq!(
        result.status, 404,
        "case {case:?}: no standard output may be written for an asset nothing was read out \
         of, got {} / {}",
        result.status,
        result.text()
    );
}

#[tokio::test]
async fn a_read_before_the_job_finishes_reports_in_progress_and_no_output_yet() {
    // The output is written by the TERMINAL transition and by nothing else, so a job the
    // clock has only moved off `Created` reports the modelled non-terminal status and has
    // written nothing yet. Which transition the clock owes is driven by the record's OWN
    // submission instant — the service stamped it, and the arrangement below moves it back by
    // one `Created` span — rather than by waiting, so the case does not depend on how fast
    // this host runs.
    let (router, engine) = emulator().await;
    let case = "a read inside the in-progress window";
    let key = "input/pending.txt";
    stage_asset(&router, case, key, b"Pending delivery note 12\n").await;

    let arn = submit(
        &router,
        case,
        &format!("s3://{ASSET_BUCKET}/{key}"),
        &format!("s3://{OUTPUT_BUCKET}/pending"),
    )
    .await;
    let id = invocation_id(&arn).to_string();
    {
        let state = bda::state::service_state(&*engine).expect("this service's store");
        let mut store = state.lock();
        let invocation = store
            .invocation_mut(&arn)
            .unwrap_or_else(|| panic!("case {case:?}: the submitted job must be stored"));
        // The `Created` span has now elapsed for this job; its terminal transition is not due
        // for another whole `IN_PROGRESS_FOR`.
        invocation.job_submission_time -= bda::state::CREATED_FOR;
    }

    let answer = call(
        &router,
        "GetDataAutomationStatus",
        json!({"invocationArn": arn}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["status"].as_str(),
        Some("InProgress"),
        "case {case:?}: a job the clock has started but not finished must report the modelled \
         non-terminal status, got {}",
        answer.body
    );
    assert!(
        answer.body["jobCompletionTime"].is_null(),
        "case {case:?}: a job that has not finished reports no completion time, got {}",
        answer.body
    );
    for key in [
        format!("pending/{id}/0/standard_output/0/result.json"),
        format!("pending/{id}/job_metadata.json"),
    ] {
        let read = read_object(&router, OUTPUT_BUCKET, &key).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: nothing may be written at {key:?} before the job finishes, got {} / {}",
            read.status,
            read.text()
        );
    }

    // And once the clock owes the terminal transition, the same reader finishes the job and
    // its output is there.
    let status = poll_until_terminal(&router, case, &arn).await;
    assert_eq!(
        status["status"].as_str(),
        Some("Success"),
        "case {case:?}: the job must finish, got {status}"
    );
    let result = read_object(
        &router,
        OUTPUT_BUCKET,
        &format!("pending/{id}/0/standard_output/0/result.json"),
    )
    .await;
    assert_eq!(
        result.status, 200,
        "case {case:?}: the standard output must be there once the job reports Success, got {} / {}",
        result.status,
        result.text()
    );
    assert_eq!(
        result.json()["document"]["representation"]["text"]
            .as_str()
            .map(squeezed)
            .as_deref(),
        Some("Pendingdeliverynote12"),
        "case {case:?}: the output must carry the text of the asset that was processed, got {}",
        result.text()
    );
}
