// SPDX-License-Identifier: CC0-1.0

//! What the OBJECT a recipe job run writes carries, beyond its bytes.
//!
//! Driven end to end with the emulator's own Amazon S3 mounted beside this service on one
//! engine — what `core/router` assembles — so every case reads the written object back out
//! of the service that owns objects, over Amazon S3's own wire protocol.
//!
//! THE RULE: an object an emulated service writes on a caller's behalf carries the
//! properties that SERVICE sets and no others — the properties the transport it writes
//! through chose for itself are not the object's. For a recipe job's output that means:
//!
//! * the provenance of the work that produced it travels as the object's USER METADATA,
//!   which Amazon S3 serves back on every read as `x-amz-meta-<key>`: the job's `Name` and
//!   the `RunId` `StartJobRun` answered;
//! * the job sets no media type, so the object carries the object store's own default for an
//!   upload that names none — `binary/octet-stream`, never the media type an SDK picks for a
//!   binary payload;
//! * the job names no checksum algorithm, so the object carries the one Amazon S3 adds to
//!   such an upload — "if it was uploaded without a checksum (and Amazon S3 added the default
//!   checksum, CRC64NVME, to the uploaded object)"
//!   (`PutObjectOutput$ChecksumCRC64NVME`) — over the object's own bytes, never the
//!   algorithm an SDK computed on the way out.
//!
//! The rows differ along what the rule generalises over: the output's FORMAT (CSV and JSON,
//! so the bytes and the extension differ), how many PART files the output is split into (one
//! and several, so a row asserts every object a run wrote rather than one), how the output
//! is ADDRESSED (a bare prefix, a nested one, none at all) and the JOB's own name (which is
//! part of every part file's name and of the metadata under test).
//!
//! The expected checksum is computed through the object store's own published digest
//! function (`state::full_object_checksum`, whose value is pinned against the AWS SDK's
//! checksum crate by that service's own suite) rather than restated as a literal here: one
//! run's digest is of that run's bytes.

use serde_json::json;

use crate::databrew_harness::{Emulator, ObjectAnswer};

/// The bucket every case stages its dataset in and writes its output to.
const BUCKET: &str = "brew-headers";

/// The key the staged dataset object lives under.
const INPUT_KEY: &str = "in/people.csv";

/// The dataset every run below reads: a header and three rows.
const INPUT_CSV: &str = "name,status,size\nhalyard,Active,2\nboom,Inactive,5\ngaff,Active,7\n";

/// The media type an object written with no `Content-Type` of its own carries (Amazon S3's
/// documented default for an upload that specifies none).
const DEFAULT_MEDIA_TYPE: &str = "binary/octet-stream";

/// The checksum algorithm Amazon S3 adds to an object uploaded without one, and the header
/// its value travels on.
const DEFAULT_CHECKSUM_ALGORITHM: &str = "CRC64NVME";
/// See [`DEFAULT_CHECKSUM_ALGORITHM`].
const DEFAULT_CHECKSUM_HEADER: &str = "x-amz-checksum-crc64nvme";

/// The `ChecksumType` a single whole-object write settles: there are no parts to compose a
/// checksum from.
const FULL_OBJECT: &str = "FULL_OBJECT";

/// One row: what the job's output selects and what the run must then have written.
struct Case {
    /// The job's own name, which every part file's name and the provenance metadata carry.
    job: &'static str,
    /// The output location's `Key` — a prefix the part names hang off — or `None` for an
    /// output that names none.
    key: Option<&'static str>,
    /// The prefix the written objects are listed under.
    list_prefix: &'static str,
    format: &'static str,
    max_output_files: i32,
    /// How many objects the run must have written.
    parts: usize,
}

/// Read one object back with the checksum mode enabled, which is what asks Amazon S3 for the
/// checksum it stored ("to retrieve the checksum, this mode must be enabled",
/// `GetObjectRequest$ChecksumMode`).
async fn read_object(emulator: &Emulator, key: &str) -> ObjectAnswer {
    emulator
        .s3_with(
            "GET",
            &format!("/{BUCKET}/{key}"),
            &[("x-amz-checksum-mode", "ENABLED")],
            &[],
        )
        .await
}

#[tokio::test]
async fn a_run_s_output_object_carries_its_provenance_and_the_store_s_own_defaults() {
    let cases = [
        (
            "one CSV part under a bare prefix",
            Case {
                job: "headers-job",
                key: Some("out/"),
                list_prefix: "out/",
                format: "CSV",
                max_output_files: 1,
                parts: 1,
            },
        ),
        (
            "three CSV parts under a nested prefix",
            Case {
                job: "split-headers-job",
                key: Some("out/nested/dir/"),
                list_prefix: "out/nested/dir/",
                format: "CSV",
                max_output_files: 3,
                parts: 3,
            },
        ),
        (
            "one JSON part under no prefix at all",
            Case {
                job: "json-headers-job",
                key: None,
                list_prefix: "json-headers-job",
                format: "JSON",
                max_output_files: 1,
                parts: 1,
            },
        ),
        (
            "two JSON parts under a bare prefix",
            Case {
                job: "json-split-job",
                key: Some("out/"),
                list_prefix: "out/",
                format: "JSON",
                max_output_files: 2,
                parts: 2,
            },
        ),
    ];

    for (case, expected) in cases {
        let Case {
            job,
            key,
            list_prefix,
            format,
            max_output_files,
            parts,
        } = expected;

        let emulator = Emulator::with_object_store_and_iam();
        emulator.given_bucket(BUCKET).await;
        emulator.given_object(BUCKET, INPUT_KEY, INPUT_CSV).await;
        let dataset = format!("{job}-ds");
        let created_dataset = emulator
            .call(
                "POST",
                "/datasets",
                Some(json!({
                    "Name": dataset,
                    "Format": "CSV",
                    "Input": {"S3InputDefinition": {"Bucket": BUCKET, "Key": INPUT_KEY}},
                })),
            )
            .await;
        assert_eq!(
            created_dataset.status, 200,
            "case {case:?}: the dataset must be created, got {} {:?}",
            created_dataset.status, created_dataset.body
        );

        let mut location = json!({"Bucket": BUCKET});
        if let Some(key) = key {
            location["Key"] = json!(key);
        }
        let created_job = emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": job,
                    "DatasetName": dataset,
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "Outputs": [{
                        "Format": format,
                        "Location": location,
                        "Overwrite": true,
                        "MaxOutputFiles": max_output_files,
                    }],
                })),
            )
            .await;
        assert_eq!(
            created_job.status, 200,
            "case {case:?}: the job must be created, got {} {:?}",
            created_job.status, created_job.body
        );

        let started = emulator
            .call("POST", &format!("/jobs/{job}/startJobRun"), None)
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartJobRun must be served, got {} {:?}",
            started.status, started.body
        );
        let run_id = started.text("RunId");
        assert!(
            !run_id.is_empty(),
            "case {case:?}: StartJobRun must answer the run's id, got {:?}",
            started.body
        );

        let written = emulator.keys_under(BUCKET, list_prefix).await;
        assert_eq!(
            written.len(),
            parts,
            "case {case:?}: expected {parts} part file(s) under {list_prefix:?}, got {written:?}"
        );

        for key in &written {
            let object = read_object(&emulator, key).await;
            assert_eq!(
                object.status, 200,
                "case {case:?}: the written object {key:?} must be readable, got {} {:?}",
                object.status,
                object.text()
            );
            // The provenance of the work that wrote it, served back as user metadata.
            assert_eq!(
                object.header("x-amz-meta-job_name").as_deref(),
                Some(job),
                "case {case:?}: {key:?} must name the job that wrote it, got {:?}",
                object.header("x-amz-meta-job_name")
            );
            assert_eq!(
                object.header("x-amz-meta-job_run_id").as_deref(),
                Some(run_id.as_str()),
                "case {case:?}: {key:?} must name the run that wrote it ({run_id}), got {:?}",
                object.header("x-amz-meta-job_run_id")
            );
            // The media type the job set: none, so the object store's own default.
            assert_eq!(
                object.header("content-type").as_deref(),
                Some(DEFAULT_MEDIA_TYPE),
                "case {case:?}: {key:?} must carry the store's default media type, got {:?}",
                object.header("content-type")
            );
            // And the checksum the store added, over the bytes it holds.
            assert_eq!(
                object.header(DEFAULT_CHECKSUM_HEADER),
                service_com_amazonaws_s3_amazons3::state::full_object_checksum(
                    DEFAULT_CHECKSUM_ALGORITHM,
                    &object.body,
                ),
                "case {case:?}: {key:?} must carry the default checksum of its own {} bytes, \
                 got {:?}",
                object.body.len(),
                object.header(DEFAULT_CHECKSUM_HEADER)
            );
            assert_eq!(
                object.header("x-amz-checksum-type").as_deref(),
                Some(FULL_OBJECT),
                "case {case:?}: {key:?} is one whole-object write, got {:?}",
                object.header("x-amz-checksum-type")
            );
            for other in [
                "x-amz-checksum-crc32",
                "x-amz-checksum-crc32c",
                "x-amz-checksum-sha1",
                "x-amz-checksum-sha256",
            ] {
                assert_eq!(
                    object.header(other),
                    None,
                    "case {case:?}: {key:?} carries the checksum of ONE algorithm, so \
                     {other} must be absent, got {:?}",
                    object.header(other)
                );
            }
            assert!(
                !object.body.is_empty(),
                "case {case:?}: {key:?} must hold the transformed rows"
            );
        }
    }
}

#[tokio::test]
async fn two_runs_of_one_job_stamp_their_own_run_ids_on_what_each_wrote() {
    let emulator = Emulator::with_object_store_and_iam();
    emulator.given_bucket(BUCKET).await;
    emulator.given_object(BUCKET, INPUT_KEY, INPUT_CSV).await;
    let created_dataset = emulator
        .call(
            "POST",
            "/datasets",
            Some(json!({
                "Name": "rerun-ds",
                "Format": "CSV",
                "Input": {"S3InputDefinition": {"Bucket": BUCKET, "Key": INPUT_KEY}},
            })),
        )
        .await;
    assert_eq!(
        created_dataset.status, 200,
        "the dataset must be created, got {} {:?}",
        created_dataset.status, created_dataset.body
    );
    let created_job = emulator
        .call(
            "POST",
            "/recipeJobs",
            Some(json!({
                "Name": "rerun-job",
                "DatasetName": "rerun-ds",
                "RoleArn": "arn:aws:iam::000000000000:role/brew",
                "Outputs": [{
                    "Format": "CSV",
                    "Location": {"Bucket": BUCKET, "Key": "rerun/"},
                    "Overwrite": true,
                    "MaxOutputFiles": 1,
                }],
            })),
        )
        .await;
    assert_eq!(
        created_job.status, 200,
        "the job must be created, got {} {:?}",
        created_job.status, created_job.body
    );

    // The object records the run that LAST wrote it, which is this run and not the one
    // before it: the metadata a write stores is the metadata a read then serves.
    let mut seen: Vec<String> = Vec::new();
    for case in ["the first run", "a second run of the same job"] {
        let started = emulator
            .call("POST", "/jobs/rerun-job/startJobRun", None)
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartJobRun must be served, got {} {:?}",
            started.status, started.body
        );
        let run_id = started.text("RunId");
        assert!(
            !seen.contains(&run_id),
            "case {case:?}: each run carries an id of its own, got {run_id:?} again"
        );
        seen.push(run_id.clone());

        let written = emulator.keys_under(BUCKET, "rerun/").await;
        assert_eq!(
            written.len(),
            1,
            "case {case:?}: the output holds one part file, got {written:?}"
        );
        let object = read_object(&emulator, &written[0]).await;
        assert_eq!(
            object.header("x-amz-meta-job_run_id").as_deref(),
            Some(run_id.as_str()),
            "case {case:?}: the object must name the run that wrote it ({run_id}), got {:?}",
            object.header("x-amz-meta-job_run_id")
        );
        assert_eq!(
            object.header("x-amz-meta-job_name").as_deref(),
            Some("rerun-job"),
            "case {case:?}: the object must name the job that wrote it, got {:?}",
            object.header("x-amz-meta-job_name")
        );
    }
}
