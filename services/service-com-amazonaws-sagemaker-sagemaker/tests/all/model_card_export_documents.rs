// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for what a MODEL CARD EXPORT produces: the PDF document
//! `CreateModelCardExportJob` writes under its `OutputConfig$S3OutputPath`, and the
//! `Completed` status and `ExportArtifacts` `DescribeModelCardExportJob` then reports.
//!
//! Driven with the emulator's own Amazon S3 mounted beside SageMaker on ONE shared engine
//! (the harness `training_job_execution` owns), so the object the export writes is read
//! back out of Amazon S3 itself: what these tests assert about the artifact is what a
//! client with a bucket would see.
//!
//! The RULE, asserted over a TABLE of inputs rather than one recorded call: an accepted
//! export renders the card version it resolved as a PDF, writes it as ONE object under the
//! output path the request named — in either spelling the modelled `S3Uri` admits — and the
//! terminal status the clock owes the job is `Completed` with
//! `ExportArtifacts.S3ExportArtifacts` naming exactly that object; while an output path
//! with no bucket behind it writes nothing and the job ends `Failed` with the reason, never
//! a `Completed` for a document nobody wrote. The rows differ along the output path's
//! spelling and prefix shape, which version of the card is exported, how many versions the
//! card holds, and the content the card carries.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::model_card_governance::{content_with_description, create_body};
use crate::training_job_execution::{Emulator, BUCKET};

/// The account every ARN below is minted under, as the emulator mints it.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the harness signs every call for.
const REGION: &str = "us-east-1";

/// A deadline far longer than the span the state gives an export job's terminal status, so
/// a loaded runner does not decide whether a poll loop terminates.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The first bytes of every PDF document: "%PDF-" is the header a PDF file begins with,
/// so a document a reader opens is distinguishable from bytes shaped like one.
const PDF_HEADER: &[u8] = b"%PDF-";

/// Create `card` with `versions` versions, each carrying its own content, through the
/// wire: the create mints version 1 and every `UpdateModelCard` appends the next.
async fn with_card(emulator: &Emulator, card: &str, versions: i32) {
    emulator
        .sagemaker_ok(
            "CreateModelCard",
            &create_body(card, "Draft", &content_with_description("version 1")),
        )
        .await;
    for version in 2..=versions {
        emulator
            .sagemaker_ok(
                "UpdateModelCard",
                &json!({
                    "ModelCardName": card,
                    "Content": content_with_description(&format!("version {version}")),
                }),
            )
            .await;
    }
}

/// The ARN a job named `job` exporting `card` carries.
fn job_arn(card: &str, job: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-card/{card}/export-job/{job}")
}

/// Poll `DescribeModelCardExportJob` until the job leaves `InProgress` — the move the
/// clock owes it — or a generous deadline passes.
async fn poll_until_terminal(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let described = emulator
            .sagemaker_ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": arn }),
            )
            .await;
        if described.get("Status").and_then(Value::as_str) != Some("InProgress") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the export job must leave InProgress, last read {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: an accepted export writes the card version's PDF as one object under the output
/// path the request named, and the job's terminal status is `Completed` with
/// `ExportArtifacts.S3ExportArtifacts` naming that object — which Amazon S3 itself holds,
/// as a document whose bytes are a PDF carrying what the exported version says.
#[tokio::test]
async fn a_completed_export_writes_the_card_version_as_a_pdf_object() {
    // Rows differ along what the rule generalises over: the spelling of the output path
    // (`s3://`, the virtual-hosted `https://` form of an Amazon S3 endpoint, and its
    // path-style form — both spellings the modelled `S3Uri` pattern
    // `^(https|s3)://([^/]+)/?(.*)$` admits), its prefix shape (the bucket alone, a prefix
    // with and without the trailing delimiter, a nested prefix), how many versions the
    // card holds, and which version the request pins.
    type Row<'case> = (&'case str, String, &'case str, i32, Option<i32>, i32);
    // (case, S3OutputPath, the key prefix it names, versions held, requested, exported)
    let rows: [Row<'_>; 7] = [
        ("the bucket alone", format!("s3://{BUCKET}"), "", 1, None, 1),
        (
            "a prefix with a delimiter",
            format!("s3://{BUCKET}/cards/"),
            "cards/",
            1,
            Some(1),
            1,
        ),
        (
            "a prefix with no delimiter",
            format!("s3://{BUCKET}/exports"),
            "exports/",
            3,
            None,
            3,
        ),
        (
            "a nested prefix",
            format!("s3://{BUCKET}/deeply/nested/prefix/"),
            "deeply/nested/prefix/",
            3,
            Some(1),
            1,
        ),
        (
            "a pinned middle version",
            format!("s3://{BUCKET}/middle/"),
            "middle/",
            3,
            Some(2),
            2,
        ),
        (
            "the virtual-hosted https spelling",
            format!("https://{BUCKET}.s3.{REGION}.amazonaws.com/hosted/"),
            "hosted/",
            5,
            None,
            5,
        ),
        (
            "the path-style https spelling",
            format!("https://s3.{REGION}.amazonaws.com/{BUCKET}/styled"),
            "styled/",
            2,
            Some(2),
            2,
        ),
    ];

    for (index, (case, path, prefix, held, requested, exported)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        emulator.given_bucket(BUCKET).await;
        let card = format!("exported-card-{index}");
        let job = format!("export-{index}");
        with_card(&emulator, &card, held).await;

        let mut body = json!({
            "ModelCardName": card,
            "ModelCardExportJobName": job,
            "OutputConfig": { "S3OutputPath": path },
        });
        if let Some(version) = requested {
            body["ModelCardVersion"] = json!(version);
        }
        let created = emulator.sagemaker_ok("CreateModelCardExportJob", &body).await;
        let arn = job_arn(&card, &job);
        assert_eq!(
            created.get("ModelCardExportJobArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the job's ARN {arn}, got {created}"
        );

        let settled = poll_until_terminal(&emulator, &arn, case).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some("Completed"),
            "case {case:?}: an export that wrote its document reaches Completed, got \
             {settled}"
        );
        assert_eq!(
            settled.get("FailureReason"),
            None,
            "case {case:?}: a completed export carries no FailureReason, got {settled}"
        );
        // The artifact is the object the export wrote: the prefix the output path names,
        // then the card, the version that was exported and the job that exported it — and
        // the `https://` spellings reach the same bucket and prefix the `s3://` one does.
        let key = format!("{prefix}{card}-{exported}-{job}.pdf");
        let expected_uri = format!("s3://{BUCKET}/{key}");
        assert_eq!(
            settled
                .pointer("/ExportArtifacts/S3ExportArtifacts")
                .and_then(Value::as_str),
            Some(expected_uri.as_str()),
            "case {case:?}: expected the artifact {expected_uri}, got {settled}"
        );

        // And Amazon S3 itself holds it: a PDF document carrying what the exported
        // version of the card says.
        let document = emulator
            .object_bytes(BUCKET, &key)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: Amazon S3 must hold the object {key}"));
        assert!(
            document.starts_with(PDF_HEADER),
            "case {case:?}: the object must be a PDF document, got {} byte(s) starting \
             {:?}",
            document.len(),
            String::from_utf8_lossy(&document[..document.len().min(8)])
        );
        let text = String::from_utf8_lossy(&document).to_string();
        for shown in [card.clone(), format!("version {exported}")] {
            assert!(
                text.contains(&shown),
                "case {case:?}: the document must show {shown:?}, got {} byte(s)",
                document.len()
            );
        }
        // Nothing else was written: the export leaves ONE object behind.
        let other_version = if exported == 1 { 2 } else { 1 };
        let absent = format!("{prefix}{card}-{other_version}-{job}.pdf");
        assert!(
            emulator.object_bytes(BUCKET, &absent).await.is_none(),
            "case {case:?}: the export must write only its own version's document, but \
             Amazon S3 holds {absent}"
        );
    }
}

/// RULE: the document is a function of the card version it exports — the content the
/// version carries is what the PDF shows, whatever that content is — and two jobs
/// exporting one version each write their own object, with the same bytes.
#[tokio::test]
async fn the_document_carries_the_exported_versions_own_content() {
    // Rows differ along the content the card carries: a minimal document, a nested one, a
    // list, non-ASCII text, and the JSON scalar kinds (number, boolean, null) — every one
    // of them a `Content` the create admits, since the model bounds only its length and
    // its documentation requires a JSON object.
    let rows: [(&str, &str); 5] = [
        ("an empty document", "{}"),
        (
            "a nested document",
            r#"{"model_overview":{"model_description":"a ranker","model_owner":"ml"}}"#,
        ),
        (
            "a document with a list",
            r#"{"intended_uses":["ranking","recall"],"risk_rating":"low"}"#,
        ),
        (
            "non-ASCII text",
            r#"{"model_overview":{"model_description":"モデルの説明"}}"#,
        ),
        (
            "scalars of every JSON kind",
            r#"{"accuracy":0.93,"approved":true,"owner":null}"#,
        ),
    ];

    for (index, (case, content)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        emulator.given_bucket(BUCKET).await;
        let card = format!("content-card-{index}");
        let job = "content-export";
        emulator
            .sagemaker_ok("CreateModelCard", &create_body(&card, "Draft", content))
            .await;
        emulator
            .sagemaker_ok(
                "CreateModelCardExportJob",
                &json!({
                    "ModelCardName": card,
                    "ModelCardExportJobName": job,
                    "OutputConfig": { "S3OutputPath": format!("s3://{BUCKET}/docs/") },
                }),
            )
            .await;
        let settled = poll_until_terminal(&emulator, &job_arn(&card, job), case).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some("Completed"),
            "case {case:?}: expected Completed, got {settled}"
        );

        let key = format!("docs/{card}-1-{job}.pdf");
        let first = emulator
            .object_bytes(BUCKET, &key)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: Amazon S3 must hold {key}"));
        assert!(
            first.starts_with(PDF_HEADER),
            "case {case:?}: the object must be a PDF document, got {} byte(s)",
            first.len()
        );

        // A second export job over the same version writes the same document: the bytes
        // are a function of the card version, not of the moment the export ran or of which
        // job ran it.
        emulator
            .sagemaker_ok(
                "CreateModelCardExportJob",
                &json!({
                    "ModelCardName": card,
                    "ModelCardExportJobName": "second-export",
                    "OutputConfig": { "S3OutputPath": format!("s3://{BUCKET}/docs/") },
                }),
            )
            .await;
        let settled = poll_until_terminal(&emulator, &job_arn(&card, "second-export"), case).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some("Completed"),
            "case {case:?}: the second export must complete too, got {settled}"
        );
        // Each job leaves its OWN object, and the first job's document is untouched.
        let second_key = format!("docs/{card}-1-second-export.pdf");
        let again = emulator
            .object_bytes(BUCKET, &second_key)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: Amazon S3 must hold {second_key}"));
        assert_eq!(
            again, first,
            "case {case:?}: exporting one version twice must write the same {} byte(s)",
            first.len()
        );
        assert_eq!(
            emulator.object_bytes(BUCKET, &key).await.as_deref(),
            Some(first.as_slice()),
            "case {case:?}: the first job's document must be untouched"
        );
    }
}

/// RULE: an export that could not write its document reports the modelled `Failed` with a
/// `FailureReason` naming the location it was writing to, and no `ExportArtifacts` — never
/// a `Completed` for a document nobody wrote.
#[tokio::test]
async fn an_export_that_writes_nothing_fails_and_names_no_artifact() {
    // Rows differ along WHY nothing could be written: a bucket this account does not hold,
    // in both spellings of an output path, and an `https://` host that is no Amazon S3
    // endpoint, which names no bucket at all. The third column is what the reason must
    // name: the object the export was writing, or the path itself where none resolved.
    let rows: [(&str, &str, &str); 4] = [
        (
            "a bucket nobody created",
            "s3://no-such-export-bucket/cards/",
            "s3://no-such-export-bucket/cards/",
        ),
        (
            "another bucket under a nested prefix",
            "s3://absent-bucket/deeply/nested/",
            "s3://absent-bucket/deeply/nested/",
        ),
        (
            "the virtual-hosted https spelling of a bucket nobody created",
            "https://absent-bucket.s3.us-east-1.amazonaws.com/hosted/",
            "s3://absent-bucket/hosted/",
        ),
        (
            "an https host that is no Amazon S3 endpoint",
            "https://example.com/cards/",
            "https://example.com/cards/",
        ),
    ];

    for (index, (case, path, named)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        emulator.given_bucket(BUCKET).await;
        let card = format!("unwritable-card-{index}");
        let job = "unwritable-export";
        with_card(&emulator, &card, 1).await;
        emulator
            .sagemaker_ok(
                "CreateModelCardExportJob",
                &json!({
                    "ModelCardName": card,
                    "ModelCardExportJobName": job,
                    "OutputConfig": { "S3OutputPath": path },
                }),
            )
            .await;

        let settled = poll_until_terminal(&emulator, &job_arn(&card, job), case).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some("Failed"),
            "case {case:?}: an export that wrote nothing must fail, got {settled}"
        );
        let reason = settled
            .get("FailureReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reason.starts_with("Model card export failed") && reason.contains(named),
            "case {case:?}: the FailureReason must say the export failed and name {named}, \
             got {settled}"
        );
        assert_eq!(
            settled.get("ExportArtifacts"),
            None,
            "case {case:?}: a job that wrote no document names no artifact, got {settled}"
        );
        // And the bucket that does exist is untouched by the failed export.
        assert!(
            emulator
                .object_bytes(BUCKET, &format!("cards/{card}-1-{job}.pdf"))
                .await
                .is_none(),
            "case {case:?}: a failed export must write nothing anywhere"
        );
    }
}
