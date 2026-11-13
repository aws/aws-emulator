// SPDX-License-Identifier: CC0-1.0

//! What an asynchronous job's RUN does, and what it reports back to the stored record.
//!
//! A batch detection job is "an asynchronous job to detect" something in documents: the
//! run reads every document under the submitted `InputDataConfig`, applies the family's
//! own analysis to each one's text, and writes the analysed output back under the
//! submitted `OutputDataConfig` prefix. That is the work the job's terminal status
//! reports, so it is performed here, against this account's own object storage, over the
//! emulator's in-process callback ([`smithy_server_runtime::OperationContext::callback`])
//! — no socket is opened and nothing outside this process is reached.
//!
//! A family whose per-document analysis this emulator performs for no document runs
//! nothing, which is [`JobRun::NotPerformed`]: its jobs reach the modelled failure state
//! saying so rather than reporting results nothing produced.

use super::*;

/// The most input objects one run reads, and the most bytes it reads from one object and
/// in total.
///
/// AWS publishes its own per-job document and size quotas; these are the EMULATOR's
/// bounds on the work one request performs, so no submission can make this process read
/// an unbounded amount of object storage or hold the service for an unbounded time. A
/// submission that points past them is not silently truncated: the run ends in the
/// modelled failure state naming the bound it hit.
pub const MAX_INPUT_OBJECTS: usize = 200;
/// The most bytes read from one input document (see [`MAX_INPUT_OBJECTS`]).
pub const MAX_INPUT_OBJECT_BYTES: usize = 1024 * 1024;
/// The most bytes read across one run's whole input (see [`MAX_INPUT_OBJECTS`]).
pub const MAX_TOTAL_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// The most bytes one run writes back as output (see [`MAX_INPUT_OBJECTS`]). The input
/// bounds do not bound this: an analysed document costs an output entry per entity
/// placed in it.
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// The media type of the analysed output and of the file describing it: each is a JSON
/// document.
const OUTPUT_CONTENT_TYPE: &str = "application/json";

/// The suffix the analysed output of one input document is written under: Amazon
/// Comprehend Medical writes one output file per input document, named for that document.
const OUTPUT_OBJECT_SUFFIX: &str = ".out";

/// The name of the file describing a finished run's results, which the modelled
/// `ManifestFilePath` reports ("the path to the file that describes the results of a
/// batch job"). It is written inside the run's own output directory.
const MANIFEST_OBJECT_NAME: &str = "Manifest.json";

/// What is known about one job's run, which is what the clock reveals as its terminal
/// status (see [`AsyncJob::advance`]).
#[derive(Debug, Clone)]
pub enum JobRun {
    /// No run is performed for this job at all, because this emulator performs its
    /// family's per-document analysis for no document.
    NotPerformed,
    /// A run is in flight: what it did is not known yet.
    InFlight,
    /// The run reported this.
    Reported(JobRunOutcome),
}

/// What one run reported: the terminal status it earned, the description of that status
/// the modelled `Message` carries, and the file describing its results.
#[derive(Debug, Clone)]
pub struct JobRunOutcome {
    pub job_status: JobStatus,
    pub message: Option<String>,
    pub manifest_file_path: Option<String>,
}

impl JobRunOutcome {
    /// A run that read its input documents, analysed them and wrote its output back:
    /// `COMPLETED`, carrying the path of the file describing those results.
    ///
    /// No `Message`: the modelled member is "a description of the status of a job", and
    /// the model documents a description for the failure case ("if the status is
    /// `FAILED`, the `Message` field shows the reason for the failure"), so a run that
    /// did what was asked explains nothing.
    pub fn completed(manifest_file_path: String) -> Self {
        Self {
            job_status: JobStatus::Completed,
            message: None,
            manifest_file_path: Some(manifest_file_path),
        }
    }

    /// A run that could not be carried out, and the reason it reports.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            job_status: JobStatus::Failed,
            message: Some(reason.into()),
            manifest_file_path: None,
        }
    }
}

/// Which family a job belongs to: each family keeps its own store, so a report has to
/// name the store the job was recorded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFamily {
    EntitiesDetectionV2,
    PhiDetection,
}

impl JobFamily {
    /// This family's store within the service's state.
    pub fn store_mut(
        self,
        state: &mut ComAmazonawsComprehendmedicalComprehendmedical20181030State,
    ) -> &mut JobFamilyStore {
        match self {
            Self::EntitiesDetectionV2 => &mut state.entities_detection_v2_jobs,
            Self::PhiDetection => &mut state.phi_detection_jobs,
        }
    }

    /// How this family's analysis is named in the directory a run writes its output
    /// into.
    pub fn analysis_type(self) -> &'static str {
        match self {
            Self::EntitiesDetectionV2 => "EntitiesDetection",
            Self::PhiDetection => "PHI",
        }
    }
}

/// Records against the stored job what its run reported — and, if it is dropped without
/// a report, the modelled failure for a run that will never report one.
///
/// A job's terminal status is only revealed once its run has reported, so a run whose
/// handler never gets to report (the future dropped because the client went away, a
/// panic the server contains) would otherwise leave the job non-terminal for the life of
/// the process. `Drop` runs on EVERY exit from the handler, which is what makes that
/// unreachable.
///
/// `Drop` takes the state lock, so a recorder must not be dropped while a
/// [`::engine::ServiceState::lock`] guard is alive in the same scope.
pub struct RunOutcomeRecorder {
    store: ::engine::ServiceState<ComAmazonawsComprehendmedicalComprehendmedical20181030State>,
    family: JobFamily,
    job_id: String,
    reported: bool,
}

impl RunOutcomeRecorder {
    /// Watch the run of the job `job_id` of `family`.
    pub fn new(
        store: ::engine::ServiceState<ComAmazonawsComprehendmedicalComprehendmedical20181030State>,
        family: JobFamily,
        job_id: String,
    ) -> Self {
        Self {
            store,
            family,
            job_id,
            reported: false,
        }
    }

    /// Record `outcome` as what this job's run reported. The clock reveals it once the
    /// job's running span is over, never before.
    pub fn report(&mut self, outcome: JobRunOutcome) {
        self.reported = true;
        let mut state = self.store.lock();
        self.family
            .store_mut(&mut state)
            .record_run_outcome(&self.job_id, outcome);
    }
}

impl Drop for RunOutcomeRecorder {
    fn drop(&mut self) {
        if self.reported {
            return;
        }
        let mut state = self.store.lock();
        let family = self.family.store_mut(&mut state);
        if let Some(job) = family.jobs.get_mut(&self.job_id) {
            if matches!(job.run, JobRun::InFlight) {
                job.run = JobRun::Reported(JobRunOutcome::failed(
                    "The analysis run did not complete: the request that started it ended \
                     before the run reported an outcome.",
                ));
            }
        }
    }
}

/// Run one job of `family`: read every document under `input_data_config`, analyse each
/// one with `analyse`, and write what `analyse` produced back under
/// `output_data_config`.
///
/// `analyse` is the only per-family part — the JSON the family's own analysis produces
/// for one document's text — so how an input is listed, bounded, read and reported is
/// identical for every family that runs.
///
/// Answers a [`JobRunOutcome`] rather than an error channel: the run is the job's own
/// work, so what it could not do is the job's modelled failure state and its `Message`,
/// never a failure of the `Start*Job` request that submitted it.
pub async fn run_detection_job<Analyse>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    family: JobFamily,
    job_id: &str,
    input_data_config: &InputDataConfig,
    output_data_config: &OutputDataConfig,
    analyse: Analyse,
) -> JobRunOutcome
where
    Analyse: FnMut(&str) -> ::serde_json::Value,
{
    // Built by the one shared constructor every emulated service reaches object storage
    // through, so this hop is configured exactly as theirs are. The transport is
    // in-process: the endpoint it carries is addressing for the emulator's own router.
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    match analyse_documents(
        &client,
        family,
        job_id,
        input_data_config,
        output_data_config,
        analyse,
    )
    .await
    {
        Ok(manifest_file_path) => JobRunOutcome::completed(manifest_file_path),
        Err(reason) => JobRunOutcome::failed(reason),
    }
}

/// The run itself: the location of the file describing its results, or the reason it
/// could not be carried out.
async fn analyse_documents<Analyse>(
    client: &::aws_sdk_s3::Client,
    family: JobFamily,
    job_id: &str,
    input_data_config: &InputDataConfig,
    output_data_config: &OutputDataConfig,
    mut analyse: Analyse,
) -> Result<String, String>
where
    Analyse: FnMut(&str) -> ::serde_json::Value,
{
    let input_bucket = input_data_config.s3_bucket.as_str();
    let input_prefix = input_data_config.s3_key.as_deref().unwrap_or("");
    let output_bucket = output_data_config.s3_bucket.as_str();
    let output_directory = output_directory(
        output_data_config.s3_key.as_deref().unwrap_or(""),
        family,
        job_id,
    );

    let documents = input_documents(client, input_bucket, input_prefix).await?;
    let mut remaining_input = MAX_TOTAL_INPUT_BYTES;
    let mut remaining_output = MAX_OUTPUT_BYTES;
    let mut written: Vec<::serde_json::Value> = Vec::with_capacity(documents.len());

    for (key, _) in &documents {
        let text = read_document(client, input_bucket, key, &mut remaining_input).await?;
        let analysed = analyse(&text).to_string();
        if analysed.len() > remaining_output {
            return Err(format!(
                "The output for the documents at s3://{input_bucket}/{input_prefix} is larger \
                 than the {MAX_OUTPUT_BYTES} bytes this emulator writes in one job."
            ));
        }
        remaining_output -= analysed.len();
        let output_key = format!(
            "{output_directory}{}{OUTPUT_OBJECT_SUFFIX}",
            document_name(input_prefix, key),
        );
        write_object(client, output_bucket, &output_key, analysed).await?;
        written.push(::serde_json::json!({
            "InputFile": format!("s3://{input_bucket}/{key}"),
            "OutputFile": format!("s3://{output_bucket}/{output_key}"),
        }));
    }

    // The file describing the results is written LAST, so a manifest exists only for a
    // run whose every output file was written.
    let manifest_key = format!("{output_directory}{MANIFEST_OBJECT_NAME}");
    let manifest = ::serde_json::json!({
        "JobId": job_id,
        "AnalysisType": family.analysis_type(),
        "InputDataConfig": {"S3Bucket": input_bucket, "S3Key": input_prefix},
        "OutputDataConfig": {"S3Bucket": output_bucket, "S3Key": output_directory},
        "DocumentCount": written.len(),
        "Documents": written,
    })
    .to_string();
    write_object(client, output_bucket, &manifest_key, manifest).await?;
    Ok(format!("s3://{output_bucket}/{manifest_key}"))
}

/// The directory one run writes its output into.
///
/// `OutputDataConfig$S3Key` documents the layout: the submitted location "is used as the
/// prefix for the actual location of the output" and the service "creates an output
/// directory using the job ID so that the output from one job does not overwrite the
/// output of another". The directory is named for the account, the analysis and that job
/// id.
pub fn output_directory(prefix: &str, family: JobFamily, job_id: &str) -> String {
    let prefix = prefix.trim_start_matches('/');
    let prefix = match prefix.trim_end_matches('/') {
        "" => String::new(),
        trimmed => format!("{trimmed}/"),
    };
    format!(
        "{prefix}{}-{}-{job_id}/",
        ::emulator_server::emulator_account_id(),
        family.analysis_type(),
    )
}

/// The name the output file of the document at `key` is written under: the key with the
/// submitted input prefix removed, so an output file points back at the document it was
/// produced from.
///
/// An input prefix that names ONE object leaves nothing under it, so the output file is
/// named for that object itself.
pub fn document_name<'a>(input_prefix: &str, key: &'a str) -> &'a str {
    let relative = key
        .strip_prefix(input_prefix)
        .unwrap_or(key)
        .trim_start_matches('/');
    if relative.is_empty() {
        key.split('/').next_back().unwrap_or(key)
    } else {
        relative
    }
}

/// Every input document under `prefix` with its listed size, in listing order.
///
/// The size is carried so a document, or a whole input, past this run's byte bounds is
/// refused BEFORE any of it is fetched.
async fn input_documents(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<(String, usize)>, String> {
    let mut documents: Vec<(String, usize)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                format!(
                    "Could not list the input documents at s3://{bucket}/{prefix}: {}",
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                )
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key ending at the delimiter is a directory marker rather than a
            // document. An EMPTY document is a document: an analysis of text that holds
            // no entity is an empty list, which is the honest result for an empty file.
            if key.ends_with('/') {
                continue;
            }
            if documents.len() >= MAX_INPUT_OBJECTS {
                return Err(format!(
                    "The input at s3://{bucket}/{prefix} holds more than the \
                     {MAX_INPUT_OBJECTS} documents this emulator analyzes in one job."
                ));
            }
            documents.push((
                key.to_string(),
                usize::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(usize::MAX),
            ));
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    if documents.is_empty() {
        return Err(format!(
            "No input documents were found at s3://{bucket}/{prefix}."
        ));
    }
    // The bounds are decided from the LISTED sizes, before a single document is fetched,
    // so an input past them costs no buffering at all.
    if let Some((key, size)) = documents
        .iter()
        .find(|(_, size)| *size > MAX_INPUT_OBJECT_BYTES)
    {
        return Err(format!(
            "The input document s3://{bucket}/{key} is {size} bytes, more than the \
             {MAX_INPUT_OBJECT_BYTES} bytes this emulator reads from one document."
        ));
    }
    let listed: usize = documents.iter().map(|(_, size)| *size).sum();
    if listed > MAX_TOTAL_INPUT_BYTES {
        return Err(format!(
            "The input at s3://{bucket}/{prefix} is {listed} bytes, more than the \
             {MAX_TOTAL_INPUT_BYTES} bytes this emulator reads in one job."
        ));
    }
    Ok(documents)
}

/// One input document as the text the analysis runs over, charged against the bytes this
/// run may still read.
async fn read_document(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    remaining_input: &mut usize,
) -> Result<String, String> {
    let object = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|error| {
            format!(
                "Could not read the input document s3://{bucket}/{key}: {}",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error),
            )
        })?;
    let body = object
        .body
        .collect()
        .await
        .map_err(|error| format!("Could not read the input document s3://{bucket}/{key}: {error}"))?
        .into_bytes();
    // The listed size is what the bounds above were decided from; these are the backstop
    // for a document whose fetched bytes disagree with its listing.
    if body.len() > MAX_INPUT_OBJECT_BYTES || body.len() > *remaining_input {
        return Err(format!(
            "The input document s3://{bucket}/{key} is {} bytes, more than this emulator reads \
             from one document ({MAX_INPUT_OBJECT_BYTES} bytes) or in one job \
             ({MAX_TOTAL_INPUT_BYTES} bytes).",
            body.len(),
        ));
    }
    *remaining_input -= body.len();
    String::from_utf8(body.to_vec()).map_err(|_| {
        format!(
            "The input document s3://{bucket}/{key} is not UTF-8 text; this emulator analyzes \
             plain-text clinical documents only."
        )
    })
}

/// Write one JSON object of a run's output.
async fn write_object(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    body: String,
) -> Result<(), String> {
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(OUTPUT_CONTENT_TYPE)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            body.into_bytes(),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "Could not write the output to s3://{bucket}/{key}: {}",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error),
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a run writes its output into a directory of the submitted prefix named for
    /// the account, the analysis and the job id, however the caller wrote that prefix.
    #[test]
    fn the_output_directory_is_under_the_submitted_prefix() {
        let rows: &[(&str, &str, &str)] = &[
            ("a prefix with a trailing delimiter", "output/", "output/"),
            ("a prefix without one", "output", "output/"),
            ("a nested prefix", "a/b/c/", "a/b/c/"),
            ("no prefix at all", "", ""),
            ("a prefix written from the root", "/output/", "output/"),
        ];
        for (case, prefix, expected_prefix) in rows {
            let actual = output_directory(prefix, JobFamily::PhiDetection, "job-1");
            let expected = format!(
                "{expected_prefix}{}-PHI-job-1/",
                ::emulator_server::emulator_account_id()
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: an output file is named for the input document it was produced from, with
    /// the submitted input prefix removed.
    #[test]
    fn an_output_file_is_named_for_its_input_document() {
        let rows: &[(&str, &str, &str, &str)] = &[
            ("a document under a prefix", "input/", "input/doc.txt", "doc.txt"),
            (
                "a document under a nested prefix",
                "input/",
                "input/deeper/doc.txt",
                "deeper/doc.txt",
            ),
            ("a prefix naming one document", "input/doc.txt", "input/doc.txt", "doc.txt"),
            ("no prefix at all", "", "doc.txt", "doc.txt"),
            (
                "a prefix that is not a directory",
                "input/d",
                "input/doc.txt",
                "oc.txt",
            ),
        ];
        for (case, prefix, key, expected) in rows {
            let actual = document_name(prefix, key);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
