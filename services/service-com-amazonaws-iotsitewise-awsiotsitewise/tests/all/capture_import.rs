// SPDX-License-Identifier: CC0-1.0

//! What PUTS the video on a workspace's capture source, driven end to end over
//! HTTP with the emulator's own Amazon S3 mounted beside this service on ONE
//! engine — exactly as the assembled router mounts them — so the video an import
//! brings in is the video Amazon S3 really holds.
//!
//! The rule these suites pin: a bulk import names "the files in the specified
//! Amazon S3 bucket that contain your data" and the format each one is in (`csv`,
//! `parquet`, `mp4`, `annotation`), so an accepted import into a workspace
//! ingests every `mp4` file it names as video of that workspace's capture source
//! the file's `alias` identifies ("the alias associated with the file's time
//! series") — the bytes the object really holds, over the HALF-OPEN span from the
//! instant the file states ("the nanosecond-precision start time for the file
//! data") to that instant plus the duration the MEDIA declares — which
//! `GetCaptureData` then reads back and the workspace-scoped `ListTimeSeries`
//! reports as a `VIDEO` time series. A file that is not video, names no alias or
//! no start time, or whose object the store does not serve ingests nothing: the
//! capture source stays absent rather than being invented.
//!
//! The rows differ along what the rule generalises over: the timescale and
//! duration the media declares (whole seconds, sub-second, and a duration that is
//! not a whole number of milliseconds), how long the file is, whether the FILE or
//! the JOB states the format, each of the four modelled formats and a file
//! stating none, a file naming no alias, a file stating no start time, an object
//! the store does not hold, bytes that are no container at all, and a container
//! declaring no duration.
//!
//! The HTTP harness (`call`, `error_code`, `REGION`) is the one `asset_lifecycle`
//! already defines; the workspace and dataset helpers are `support`'s and the
//! data-segment suite's. Only the router differs, because this suite needs the
//! object store mounted beside the service.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    MAX_CAPTURE_FILE_BYTES, MAX_IMPORTED_CAPTURE_FILES,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

use crate::asset_lifecycle::{call, error_code, REGION};

/// The workspace every import below ingests into.
const WORKSPACE: &str = "press-floor";

/// The bucket the video files are staged in.
const BUCKET: &str = "sitewise-video";

/// The `jobRoleArn` every create names: an ARN of the form the modelled `ARN`
/// pattern admits (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`).
const JOB_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SiteWiseBulkImport";

/// The `VideoDataType` a read of imported `mp4` video reports. Source:
/// `smithy_query(query="shape", shape="VideoDataType")` — the enum holds exactly
/// this one value.
const VIDEO_DATA_TYPE: &str = "VIDEO-MP4";

/// The `PropertyDataType` a workspace's video time series reports. Source:
/// `smithy_query(query="shape", shape="PropertyDataType")` — STRING | INTEGER |
/// DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON.
const VIDEO_PROPERTY_TYPE: &str = "VIDEO";

/// How many nanoseconds one second holds — the conversion this suite does its own
/// span arithmetic with.
const NANOS_PER_SECOND: i64 = 1_000_000_000;

// ---------------------------------------------------------------------------
// Writing the MP4 files an import reads
//
// Hand-written from the box layout ISO/IEC 14496-12 defines, as an INDEPENDENT
// oracle: the service reads the container with a maintained MP4 reader, so
// building the bytes with that same reader's writer would let one reading of the
// format agree with itself and pass.
// ---------------------------------------------------------------------------

/// One ISO base media box: its length, its four-character name, then its body.
fn media_box(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + body.len());
    let size = u32::try_from(8 + body.len()).expect("a short box");
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(body);
    bytes
}

/// The movie header of a file whose media runs for `duration` units of
/// `timescale` units per second — version 0, so both are 32-bit.
fn movie_header(timescale: u32, duration: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4]; // version 0 and no flags
    body.extend_from_slice(&0u32.to_be_bytes()); // creation time
    body.extend_from_slice(&0u32.to_be_bytes()); // modification time
    body.extend_from_slice(&timescale.to_be_bytes());
    body.extend_from_slice(&duration.to_be_bytes());
    body.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    body.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    body.extend_from_slice(&0u16.to_be_bytes()); // reserved
    body.extend_from_slice(&[0u8; 8]); // reserved
    for value in [
        0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000,
    ] {
        body.extend_from_slice(&value.to_be_bytes()); // the unity matrix
    }
    body.extend_from_slice(&[0u8; 24]); // pre-defined
    body.extend_from_slice(&1u32.to_be_bytes()); // next track id
    media_box(b"mvhd", &body)
}

/// An MP4 file: the brand box, a movie box carrying only the movie header, and a
/// media-data box holding `payload` — so two files of one duration still differ in
/// their bytes.
fn mp4_file(timescale: u32, duration: u32, payload: &[u8]) -> Vec<u8> {
    let mut brands = Vec::new();
    brands.extend_from_slice(b"isom"); // major brand
    brands.extend_from_slice(&0u32.to_be_bytes()); // minor version
    brands.extend_from_slice(b"isom");
    brands.extend_from_slice(b"mp41");
    let mut file = media_box(b"ftyp", &brands);
    file.extend_from_slice(&media_box(b"moov", &movie_header(timescale, duration)));
    file.extend_from_slice(&media_box(b"mdat", payload));
    file
}

/// A payload of `length` bytes whose contents name the file it belongs to, so an
/// answer that served another file's video cannot pass.
fn payload(marker: &str, length: usize) -> Vec<u8> {
    marker
        .bytes()
        .cycle()
        .take(length.max(1))
        .collect::<Vec<u8>>()
}

// ---------------------------------------------------------------------------
// The wire harness
// ---------------------------------------------------------------------------

/// A router with this service AND the emulator's Amazon S3 mounted on one shared
/// engine, the way the assembled router mounts every service.
fn router_with_object_store() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pin authentication off so the suite is independent of the process-wide switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam an
/// import reads its files through, so an object staged with this client is the
/// object an import reads.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Create the bucket the files are staged in, through Amazon S3's own
/// `CreateBucket`.
///
/// A bucket outside `us-east-1` is created with the location constraint naming
/// its region, which is what Amazon S3 requires of a request sent to a
/// region-specific endpoint.
async fn create_bucket(router: &MultiServiceRouter) {
    let created = s3(router)
        .create_bucket()
        .bucket(BUCKET)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await;
    assert!(
        created.is_ok(),
        "setup: Amazon S3 must create the staging bucket, got {created:?}"
    );
}

/// Stage one object, through Amazon S3's own `PutObject`.
async fn stage(router: &MultiServiceRouter, key: &str, body: &[u8]) {
    let put = s3(router)
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body.to_vec()))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "setup: Amazon S3 must hold the object {key}, got {put:?}"
    );
}

/// A router carrying the staging bucket, [`WORKSPACE`] and a session dataset of
/// it: the destination an import that ingests video names.
async fn fixture() -> (MultiServiceRouter, String) {
    let router = router_with_object_store();
    create_bucket(&router).await;
    let dataset = crate::dataset_data_segments::create_dataset(
        &router,
        "Video session",
        Some("SESSION"),
        Some(WORKSPACE),
    )
    .await;
    (router, dataset)
}

/// A modelled `TimeInNanos` on the wire.
fn instant(seconds: i64, nanos: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": nanos })
}

/// `POST /jobs`: one bulk import of `files` into the session dataset `dataset` of
/// [`WORKSPACE`], with the job-level `jobConfiguration` `configuration`.
///
/// `job_name` is the import's own `jobName`, which the member documents as "the
/// unique name that helps identify the job request": one name is held by at most
/// one job, so every import this suite starts on one router names its own.
async fn import(
    router: &MultiServiceRouter,
    dataset: &str,
    job_name: &str,
    files: Vec<Value>,
    configuration: Option<Value>,
) -> (u16, Value) {
    let mut body = json!({
        "jobName": job_name,
        "jobRoleArn": JOB_ROLE_ARN,
        "files": files,
        "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
        "workspaceName": WORKSPACE,
        "datasetId": dataset,
    });
    if let Some(configuration) = configuration {
        body.as_object_mut()
            .expect("an object body")
            .insert("jobConfiguration".to_string(), configuration);
    }
    call(router, "POST", "/jobs", Some(body)).await
}

/// `POST /workspaces/{workspaceName}/get-capture-data`.
async fn get_capture_data(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/workspaces/{WORKSPACE}/get-capture-data"),
        Some(body),
    )
    .await
}

/// The summaries the workspace-scoped `ListTimeSeries` reports, keyed by alias.
async fn workspace_series(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/timeseries?workspaceName={WORKSPACE}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListTimeSeries: got {status} {body}");
    body["TimeSeriesSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a TimeSeriesSummaries list, got {body}"))
        .clone()
}

/// Standard base64 with padding — the encoding restJson1 binds a blob member to,
/// through the runtime's own encoder, as the sibling capture suite sends blobs.
fn base64(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
}

// ---------------------------------------------------------------------------
// The import table
// ---------------------------------------------------------------------------

/// What one row stages at the key its file names.
#[derive(Debug, Clone, Copy)]
enum Object {
    /// An MP4 declaring `duration` units of a `timescale`-per-second clock, whose
    /// media-data box holds `length` bytes.
    Video {
        timescale: u32,
        duration: u32,
        length: usize,
    },
    /// Bytes that are no container at all.
    NotMedia,
    /// A container cut off part-way through, so its own boxes run past its end.
    Truncated,
    /// A container one byte past [`MAX_CAPTURE_FILE_BYTES`], the ceiling on how
    /// much of one file this emulator reads.
    Oversize,
    /// Nothing staged: the object store serves no such object.
    Absent,
}

/// Which `FileFormat` member a row declares, for the file or for the job.
#[derive(Debug, Clone, Copy)]
enum Format {
    Mp4,
    Csv,
    Parquet,
    Annotation,
}

impl Format {
    /// The modelled `FileFormat` naming this member. Source:
    /// `smithy_query(query="shape", shape="FileFormat")` — csv | parquet | mp4 |
    /// annotation.
    fn wire(self) -> Value {
        match self {
            Format::Mp4 => json!({ "mp4": {} }),
            // `csv` is the one member with a required body of its own.
            Format::Csv => json!({ "csv": { "columnNames": ["ALIAS", "VALUE"] } }),
            Format::Parquet => json!({ "parquet": {} }),
            Format::Annotation => json!({ "annotation": {} }),
        }
    }
}

/// One row of the import table.
#[derive(Debug, Clone, Copy)]
struct Case {
    case: &'static str,
    /// What the object store holds at the key this row's file names.
    object: Object,
    /// The `alias` the file carries, when it carries one.
    alias: Option<&'static str>,
    /// Whether the file states a `startTime`.
    states_start: bool,
    /// The format the FILE declares, when it declares one.
    file_format: Option<Format>,
    /// The format the JOB declares for the files that declare none.
    job_format: Option<Format>,
    /// Whether this row's file must bring video in.
    ingests: bool,
}

/// The rows. Every one of them is imported as its own job into the same session
/// dataset, under an alias of its own, so one row's video cannot stand in for
/// another's.
const CASES: &[Case] = &[
    Case {
        case: "a video file of a whole number of seconds",
        object: Object::Video {
            timescale: 1_000,
            duration: 4_000,
            length: 64,
        },
        alias: Some("/plant/line/1/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: true,
    },
    Case {
        case: "a video file of a sub-second duration in a video timescale",
        object: Object::Video {
            timescale: 90_000,
            duration: 45_000,
            length: 7,
        },
        alias: Some("/plant/line/2/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: true,
    },
    Case {
        case: "a video file whose duration is no whole number of milliseconds",
        object: Object::Video {
            timescale: 1_000_000_000,
            duration: 1_500_000_123,
            length: 1,
        },
        alias: Some("/plant/line/3/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: true,
    },
    Case {
        case: "a video file whose format the job states rather than the file",
        object: Object::Video {
            timescale: 30_000,
            duration: 60_000,
            length: 256,
        },
        alias: Some("/plant/line/4/camera"),
        states_start: true,
        file_format: None,
        job_format: Some(Format::Mp4),
        ingests: true,
    },
    Case {
        case: "a csv file",
        object: Object::Video {
            timescale: 1_000,
            duration: 2_000,
            length: 32,
        },
        alias: Some("/plant/line/5/camera"),
        states_start: true,
        file_format: Some(Format::Csv),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a parquet file",
        object: Object::Video {
            timescale: 1_000,
            duration: 2_000,
            length: 32,
        },
        alias: Some("/plant/line/6/camera"),
        states_start: true,
        file_format: Some(Format::Parquet),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "an annotation file",
        object: Object::Video {
            timescale: 1_000,
            duration: 2_000,
            length: 32,
        },
        alias: Some("/plant/line/7/camera"),
        states_start: true,
        file_format: Some(Format::Annotation),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a file whose format neither it nor the job states",
        object: Object::Video {
            timescale: 1_000,
            duration: 2_000,
            length: 32,
        },
        alias: Some("/plant/line/8/camera"),
        states_start: true,
        file_format: None,
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a video file whose format the job states as another",
        object: Object::Video {
            timescale: 1_000,
            duration: 2_000,
            length: 32,
        },
        alias: Some("/plant/line/9/camera"),
        states_start: true,
        file_format: None,
        job_format: Some(Format::Parquet),
        ingests: false,
    },
    Case {
        case: "a video file naming no alias",
        object: Object::Video {
            timescale: 1_000,
            duration: 2_000,
            length: 32,
        },
        alias: None,
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a video file stating no start time",
        object: Object::Video {
            timescale: 1_000,
            duration: 2_000,
            length: 32,
        },
        alias: Some("/plant/line/11/camera"),
        states_start: false,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a video file the object store does not hold",
        object: Object::Absent,
        alias: Some("/plant/line/12/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a file whose bytes are no container at all",
        object: Object::NotMedia,
        alias: Some("/plant/line/13/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a container declaring no duration",
        object: Object::Video {
            timescale: 1_000,
            duration: 0,
            length: 32,
        },
        alias: Some("/plant/line/14/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a container cut off part-way through",
        object: Object::Truncated,
        alias: Some("/plant/line/15/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: false,
    },
    Case {
        case: "a video file one byte past the ceiling this emulator reads",
        object: Object::Oversize,
        alias: Some("/plant/line/16/camera"),
        states_start: true,
        file_format: Some(Format::Mp4),
        job_format: None,
        ingests: false,
    },
];

/// Where one row's video begins: a distinct second per row, inside the modelled
/// `TimeInNanos` range, with a nanosecond offset on every third row so a span
/// crossing a second boundary is covered too.
fn start_of(row: usize) -> (i64, i64) {
    (
        1_700_000_000 + (row as i64) * 10_000,
        if row.is_multiple_of(3) { 0 } else { 750_000_000 },
    )
}

/// The object key one row's file names.
fn key_of(row: usize) -> String {
    format!("video/{row}/clip.mp4")
}

/// The bytes one row stages, or `None` for the row that stages nothing.
fn object_of(case: &Case, row: usize) -> Option<Vec<u8>> {
    match case.object {
        Object::Video {
            timescale,
            duration,
            length,
        } => Some(mp4_file(
            timescale,
            duration,
            &payload(&format!("row-{row}-"), length),
        )),
        Object::NotMedia => Some(payload(&format!("not-a-container-{row}-"), 48)),
        // Enough to carry the brand box and the start of the movie box, so what
        // the reader meets is a container whose own boxes run past its end.
        Object::Truncated => {
            let mut whole = mp4_file(1_000, 2_000, &payload(&format!("cut-{row}-"), 32));
            whole.truncate(32);
            Some(whole)
        }
        // A file whose padding alone takes it past the ceiling: the ceiling is on
        // the whole object, so the boxes in front of the padding count too.
        Object::Oversize => Some(mp4_file(
            1_000,
            2_000,
            &payload(&format!("big-{row}-"), MAX_CAPTURE_FILE_BYTES),
        )),
        Object::Absent => None,
    }
}

/// The half-open span this suite expects one row's video to cover: the instant the
/// file states, to that instant plus the duration the media declares — computed
/// here from the row's own numbers rather than from anything the service answered.
fn span_of(case: &Case, row: usize) -> ((i64, i64), (i64, i64)) {
    let (start_seconds, start_nanos) = start_of(row);
    let Object::Video {
        timescale,
        duration,
        ..
    } = case.object
    else {
        panic!("case {:?}: only a video row has a span", case.case);
    };
    let nanos = i128::from(duration) * i128::from(NANOS_PER_SECOND) / i128::from(timescale);
    let total = i128::from(start_nanos) + nanos;
    let end_seconds = start_seconds + i64::try_from(total / i128::from(NANOS_PER_SECOND)).unwrap();
    let end_nanos = i64::try_from(total % i128::from(NANOS_PER_SECOND)).unwrap();
    ((start_seconds, start_nanos), (end_seconds, end_nanos))
}

/// A request naming the capture source `alias` holds over a window far wider than
/// any row's span.
fn by_alias(alias: &str, row: usize) -> Value {
    let (seconds, _) = start_of(row);
    json!({
        "propertyAlias": alias,
        "startTime": instant(seconds - 1_000, 0),
        "endTime": instant(seconds + 1_000, 0),
    })
}

#[tokio::test]
async fn an_imported_video_file_becomes_the_capture_source_its_alias_names() {
    let (router, dataset) = fixture().await;

    for (row, case) in CASES.iter().enumerate() {
        let label = case.case;
        let key = key_of(row);
        if let Some(bytes) = object_of(case, row) {
            stage(&router, &key, &bytes).await;
        }

        let (start_seconds, start_nanos) = start_of(row);
        let mut file = json!({ "bucket": BUCKET, "key": key });
        let object = file.as_object_mut().expect("an object file");
        if let Some(alias) = case.alias {
            object.insert("alias".to_string(), json!(alias));
        }
        if case.states_start {
            object.insert(
                "startTime".to_string(),
                instant(start_seconds, start_nanos),
            );
        }
        if let Some(format) = case.file_format {
            object.insert("fileFormat".to_string(), format.wire());
        }
        let configuration = case
            .job_format
            .map(|format| json!({ "fileFormat": format.wire() }));

        let (status, started) =
            import(&router, &dataset, &format!("Video import {row}"), vec![file], configuration)
                .await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected the import to be accepted, got {status} {started}"
        );

        // A row that names no alias has no identifier to read back by; the
        // listing below is what pins that it created no series at all.
        let Some(alias) = case.alias else {
            continue;
        };
        let (status, body) = get_capture_data(&router, by_alias(alias, row)).await;
        if !case.ingests {
            assert_eq!(
                status, 404,
                "case {label:?}: expected no capture source, got {status} {body}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {label:?}: expected ResourceNotFoundException, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {label:?}: expected the imported video to be served, got {status} {body}"
        );
        let expected = object_of(case, row).expect("a staged video object");
        assert_eq!(
            body["data"].as_str().unwrap_or_default(),
            base64(&expected),
            "case {label:?}: expected the bytes the object holds ({} of them), got {body}",
            expected.len()
        );
        let ((start_seconds, start_nanos), (end_seconds, end_nanos)) = span_of(case, row);
        assert_eq!(
            (
                body["startTime"]["timeInSeconds"].as_i64(),
                body["startTime"]["offsetInNanos"].as_i64().unwrap_or(0),
            ),
            (Some(start_seconds), start_nanos),
            "case {label:?}: expected the span to start where the file states, got {body}"
        );
        assert_eq!(
            (
                body["endTime"]["timeInSeconds"].as_i64(),
                body["endTime"]["offsetInNanos"].as_i64().unwrap_or(0),
            ),
            (Some(end_seconds), end_nanos),
            "case {label:?}: expected the span to end at the media's own duration, got {body}"
        );
        assert_eq!(
            body["dataType"].as_str(),
            Some(VIDEO_DATA_TYPE),
            "case {label:?}: expected the modelled video data type, got {body}"
        );
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_none(),
            "case {label:?}: one file is one segment, so there is no next chunk, got {body}"
        );
    }

    // The workspace holds EXACTLY the series the ingesting rows brought in: a row
    // that ingested nothing created no time series either.
    let listed = workspace_series(&router).await;
    let mut aliases: Vec<String> = listed
        .iter()
        .map(|summary| summary["alias"].as_str().unwrap_or_default().to_string())
        .collect();
    aliases.sort();
    let mut expected: Vec<String> = CASES
        .iter()
        .filter(|case| case.ingests)
        .filter_map(|case| case.alias.map(str::to_string))
        .collect();
    expected.sort();
    assert_eq!(
        aliases, expected,
        "the workspace must hold one time series per imported video file, got {listed:?}"
    );
    for summary in &listed {
        assert_eq!(
            summary["dataType"].as_str(),
            Some(VIDEO_PROPERTY_TYPE),
            "an imported video stream is a VIDEO time series, got {summary}"
        );
    }
}

#[tokio::test]
async fn imported_video_reads_back_by_either_identifier_and_pages_one_file_per_answer() {
    let (router, dataset) = fixture().await;
    let alias = "/plant/line/paging/camera";

    // Three files of ONE stream, at consecutive spans of different lengths, each
    // carrying bytes of its own.
    let clips: Vec<(String, i64, Vec<u8>)> = vec![
        (
            "video/paging/one.mp4".to_string(),
            1_700_000_000,
            mp4_file(1_000, 5_000, &payload("first-", 40)),
        ),
        (
            "video/paging/two.mp4".to_string(),
            1_700_000_005,
            mp4_file(90_000, 450_000, &payload("second-", 11)),
        ),
        (
            "video/paging/three.mp4".to_string(),
            1_700_000_010,
            mp4_file(1_000, 1_250, &payload("third-", 300)),
        ),
    ];
    let mut files = Vec::new();
    for (key, start, bytes) in &clips {
        stage(&router, key, bytes).await;
        files.push(json!({
            "bucket": BUCKET,
            "key": key,
            "alias": alias,
            "startTime": instant(*start, 0),
            "fileFormat": { "mp4": {} },
        }));
    }
    let (status, started) = import(&router, &dataset, "Paging import", files.clone(), None).await;
    assert_eq!(
        status, 202,
        "the import of three clips must be accepted, got {status} {started}"
    );

    // ONE time series, which the workspace-scoped listing reports under the alias
    // every file named, and whose id is the second identifier a read may use.
    let listed = workspace_series(&router).await;
    assert_eq!(
        listed.len(),
        1,
        "three files of one stream are one time series, got {listed:?}"
    );
    let time_series_id = listed[0]["timeSeriesId"]
        .as_str()
        .unwrap_or_else(|| panic!("a summary carries a timeSeriesId, got {:?}", listed[0]))
        .to_string();
    assert_eq!(
        listed[0]["alias"].as_str(),
        Some(alias),
        "the series answers to the alias the files named, got {:?}",
        listed[0]
    );

    // A window covering every clip: one answer per stored segment, oldest first,
    // each reporting its own bytes and span, with the token absent on the last.
    for (label, identifier) in [
        ("alias", json!({ "propertyAlias": alias })),
        ("id", json!({ "timeSeriesId": time_series_id })),
    ] {
        let mut token: Option<String> = None;
        for (at, (_, start, bytes)) in clips.iter().enumerate() {
            let mut body = json!({
                "startTime": instant(1_699_999_000, 0),
                "endTime": instant(1_700_001_000, 0),
            });
            let object = body.as_object_mut().expect("an object body");
            for (member, value) in identifier.as_object().expect("an identifier") {
                object.insert(member.clone(), value.clone());
            }
            if let Some(token) = &token {
                object.insert("nextToken".to_string(), json!(token));
            }
            let (status, answer) = get_capture_data(&router, body).await;
            assert_eq!(
                status, 200,
                "case {label:?}/chunk {at}: expected 200, got {status} {answer}"
            );
            assert_eq!(
                answer["data"].as_str().unwrap_or_default(),
                base64(bytes),
                "case {label:?}/chunk {at}: expected the {at}th clip's own bytes, got {answer}"
            );
            assert_eq!(
                answer["startTime"]["timeInSeconds"].as_i64(),
                Some(*start),
                "case {label:?}/chunk {at}: expected the clip's own start, got {answer}"
            );
            token = answer
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let last = at + 1 == clips.len();
            assert_eq!(
                token.is_none(),
                last,
                "case {label:?}/chunk {at}: the token is absent only on the last chunk, \
                 got {answer}"
            );
        }
    }

    // Importing the same files again stores the same spans again rather than a
    // second copy of each: a segment is single-valued at its start time.
    let (status, again) = import(&router, &dataset, "Paging import again", files, None).await;
    assert_eq!(
        status, 202,
        "the repeated import must be accepted, got {status} {again}"
    );
    let mut chunks = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "propertyAlias": alias,
            "startTime": instant(1_699_999_000, 0),
            "endTime": instant(1_700_001_000, 0),
        });
        if let Some(token) = &token {
            body.as_object_mut()
                .expect("an object body")
                .insert("nextToken".to_string(), json!(token));
        }
        let (status, answer) = get_capture_data(&router, body).await;
        assert_eq!(status, 200, "re-import walk: got {status} {answer}");
        chunks += 1;
        token = answer
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() || chunks > clips.len() {
            break;
        }
    }
    assert_eq!(
        chunks,
        clips.len(),
        "a repeated import of the same files adds no second segment"
    );
}

#[tokio::test]
async fn an_import_naming_more_video_files_than_this_emulator_reads_is_refused() {
    let (router, dataset) = fixture().await;

    // One file past the ceiling, every one of them video: the import is REFUSED
    // rather than accepted with some of the video it named left unread.
    let mut files = Vec::new();
    for at in 0..=MAX_IMPORTED_CAPTURE_FILES {
        let key = format!("video/quota/{at}.mp4");
        stage(
            &router,
            &key,
            &mp4_file(1_000, 2_000, &payload(&format!("quota-{at}-"), 16)),
        )
        .await;
        files.push(json!({
            "bucket": BUCKET,
            "key": key,
            "alias": format!("/plant/quota/{at}/camera"),
            "startTime": instant(1_700_000_000 + (at as i64) * 100, 0),
            "fileFormat": { "mp4": {} },
        }));
    }

    let (status, refused) = import(&router, &dataset, "Quota import", files.clone(), None).await;
    assert_eq!(
        status, 410,
        "one file past the ceiling must be the modelled quota refusal, got {status} {refused}"
    );
    assert!(
        error_code(&refused).contains("LimitExceededException"),
        "expected LimitExceededException, got {refused}"
    );
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains(&MAX_IMPORTED_CAPTURE_FILES.to_string()),
        "the refusal must name the bound it enforces, got {refused}"
    );
    assert!(
        workspace_series(&router).await.is_empty(),
        "a refused import brings in no video at all"
    );

    // At the ceiling exactly, the import is accepted and EVERY file's video lands.
    files.pop();
    let (status, started) = import(&router, &dataset, "Quota import at the ceiling", files, None)
        .await;
    assert_eq!(
        status, 202,
        "an import at the ceiling must be accepted, got {status} {started}"
    );
    let listed = workspace_series(&router).await;
    assert_eq!(
        listed.len(),
        MAX_IMPORTED_CAPTURE_FILES,
        "every file of an accepted import brings its video in, got {listed:?}"
    );
}

#[tokio::test]
async fn an_import_outside_a_workspace_ingests_no_video_anywhere() {
    let router = router_with_object_store();
    create_bucket(&router).await;
    crate::support::ensure_workspace(&router, WORKSPACE).await;

    // A job naming no workspace ingests into "time series outside of a
    // workspace", which the warm tier is the documented precondition for; video
    // belongs to a workspace's capture source, so such a job brings none in.
    let (status, configured) = call(
        &router,
        "POST",
        "/configuration/account/storage",
        Some(json!({ "storageType": "SITEWISE_DEFAULT_STORAGE", "warmTier": "ENABLED" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the warm tier must be enabled, got {status} {configured}"
    );

    let alias = "/plant/line/outside/camera";
    let key = "video/outside/clip.mp4";
    stage(&router, key, &mp4_file(1_000, 3_000, &payload("outside-", 24))).await;
    let (status, started) = call(
        &router,
        "POST",
        "/jobs",
        Some(json!({
            "jobName": "Outside import",
            "jobRoleArn": JOB_ROLE_ARN,
            "files": [{
                "bucket": BUCKET,
                "key": key,
                "alias": alias,
                "startTime": instant(1_700_000_000, 0),
                "fileFormat": { "mp4": {} },
            }],
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the import outside a workspace must be accepted, got {status} {started}"
    );

    let (status, body) = get_capture_data(
        &router,
        json!({
            "propertyAlias": alias,
            "startTime": instant(1_699_999_000, 0),
            "endTime": instant(1_700_001_000, 0),
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a job outside a workspace puts no video in one, got {status} {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {body}"
    );
    assert!(
        workspace_series(&router).await.is_empty(),
        "a job outside a workspace creates no workspace time series"
    );
}
