// SPDX-License-Identifier: CC0-1.0

//! The job side of this service's state: the stored record of every job family
//! (transcription, medical transcription, Medical Scribe and Call Analytics), together
//! with the media those starts resolve and the output objects their results are served
//! from.
//!
//! Split out of [`super`] so the module root keeps the store itself and the records of
//! the custom-resource families (vocabularies, medical vocabularies, filters, language
//! models and Call Analytics categories); every name here is re-exported from
//! `crate::state`, so a handler reaches it exactly as before.

use super::*;

/// One stored transcription job: exactly what the request asked for, plus the
/// lifecycle facts the service itself owns (status, timestamps, transcript location).
///
/// This family has job QUEUEING, and the model names the request member that opts into
/// it: "Makes it possible to enable job queuing when your concurrent request limit is
/// exceeded. When AllowDeferredExecution is set to true, transcription job requests are
/// placed in a queue until the number of jobs falls below the concurrent request limit.
/// If AllowDeferredExecution is set to false and the number of transcription job
/// requests exceed the concurrent request limit, you get a LimitExceededException
/// error" (`smithy_query(query="shape", shape="JobExecutionSettings")`). So a start
/// that sets that member while the family's concurrency ceiling is already reached
/// ([`MAX_CONCURRENT_TRANSCRIPTION_JOBS`]) is ACCEPTED in [`STATUS_QUEUED`] with no
/// start time and has two clock-owed transitions, `QUEUED` -> `IN_PROGRESS` ->
/// `COMPLETED`, exactly as a queued Call Analytics job does ([`CallAnalyticsJobRecord`]);
/// a start at that ceiling that deferred nothing is refused, as is one that would make
/// the queue deeper than this emulator keeps it
/// ([`MAX_QUEUED_TRANSCRIPTION_JOBS`]), and every accepted job below the ceiling is
/// processing from the moment it is started ([`admit_start`]).
#[derive(Debug, Clone)]
pub struct TranscriptionJobRecord {
    /// `TranscriptionJobName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListTranscriptionJobs`
    /// pages with.
    pub sequence: u64,
    /// `TranscriptionJobStatus`: one of [`STATUS_QUEUED`], [`STATUS_IN_PROGRESS`],
    /// [`STATUS_COMPLETED`], [`STATUS_FAILED`].
    pub status: String,
    pub language_code: Option<String>,
    pub media: Media,
    /// `MediaFormat`, the format of this job's input media. What the start request
    /// declared it to be, if anything, until the job FINISHES, when the service's own
    /// determination fills it in where the request declared nothing
    /// ([`determined_media_format`]).
    pub media_format: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    pub output_bucket_name: Option<String>,
    pub output_key: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the transcript would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
    /// `OutputLocationType`, decided once at start from `OutputBucketName`.
    pub output_location_type: String,
    /// The region the start request was signed for; the service-managed transcript
    /// URI is in that region.
    pub region: String,
    pub settings: Option<Settings>,
    pub model_settings: Option<ModelSettings>,
    pub job_execution_settings: Option<JobExecutionSettings>,
    pub content_redaction: Option<ContentRedaction>,
    pub identify_language: Option<bool>,
    pub identify_multiple_languages: Option<bool>,
    pub language_options: Option<Vec<String>>,
    /// The language the media this job was pointed at EVIDENCES, established from the
    /// stored object when the job was started ([`media_language_evidence`]); `None`
    /// where that object evidenced none, or where no object store answered for it.
    /// What automatic language identification reads
    /// ([`Self::materialise_identified_languages`]).
    pub media_language: Option<String>,
    /// `IdentifiedLanguageScore`, the confidence the single language this job
    /// identified was reported with; `None` until a job that asked for
    /// single-language identification completes, and for a job that named its own
    /// language, which the service identified nothing for
    /// ([`Self::materialise_identified_languages`]).
    pub identified_language_score: Option<f32>,
    /// `LanguageCodes`, one entry per language a MULTI-language identification job
    /// identified; `None` until such a job completes, and for every job that did not
    /// ask for it ([`identified_languages`]).
    pub language_codes: Option<Vec<LanguageCodeItem>>,
    pub language_id_settings: Option<std::collections::HashMap<String, LanguageIdSettings>>,
    pub toxicity_detection: Option<Vec<ToxicityDetectionSettings>>,
    /// The subtitle request, echoed back as `SubtitlesOutput`; its
    /// `SubtitleFileUris` are filled in when the job completes.
    pub subtitles: Option<SubtitlesOutput>,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<Transcript>,
    /// Whether the documents the locations this job reports serve have already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
    pub failure_reason: Option<String>,
    /// The `FailureReason` the clock owes this job INSTEAD of a transcript, established
    /// from the media the start was pointed at ([`media_failure_reason`]); `None` for a
    /// job the service found nothing wrong with. Processing is what fails, so the job is
    /// accepted and reports this only once the clock has finished with it
    /// ([`Self::advance_clock`]).
    pub failure_due: Option<String>,
    pub creation_time: SystemTime,
    /// When the job began processing; `None` while it is still `QUEUED`.
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `QUEUED` -> `IN_PROGRESS` falls due; `None` for a job that started
    /// processing immediately.
    pub queued_until: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl TranscriptionJobRecord {
    /// Materialise the status transitions the CLOCK already owes this job: each one
    /// was due at its stored instant whether or not anybody called. A reporting
    /// operation (`Get`, `List`) runs this before projecting the record, and that
    /// is the only write it performs.
    ///
    /// Completing a job records WHERE each output it asked for is written. Storing the
    /// document at those locations is the one step this cannot take — it is a call into
    /// object storage, which only an operation can make — so the operation that finds
    /// the job finished makes it ([`due_output_objects`], [`store_job_outputs`]).
    ///
    /// It also records what the service determined while it processed the media: the
    /// language identification the request asked for
    /// ([`Self::materialise_identified_languages`]) and the format of the input audio
    /// ([`determined_media_format`]). Both are a function of the stored record, so a job
    /// reports the same ones at every later read.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status == STATUS_QUEUED {
            match self.queued_until {
                Some(due) if now >= due => {
                    self.status = STATUS_IN_PROGRESS.to_string();
                    self.start_time = Some(due);
                }
                _ => return,
            }
        }
        if self.status == STATUS_IN_PROGRESS && now >= self.completion_due {
            self.completion_time = Some(self.completion_due);
            // A job whose media the service found wrong ends FAILED, reporting why and
            // no transcript: there is no output to name for a job that produced none.
            if let Some(reason) = self.failure_due.clone() {
                self.status = STATUS_FAILED.to_string();
                self.failure_reason = Some(reason);
                return;
            }
            self.status = STATUS_COMPLETED.to_string();
            self.materialise_identified_languages();
            self.media_format = determined_media_format(self.media_format.take(), &self.media);
            let transcript_uri = self.output_uri("json");
            // Which transcripts the request asked the service to produce decides
            // which of the two locations the job reports ([`transcript_outputs`]).
            // `Media$RedactedMediaFileUri` cannot ask this family for a redacted
            // output: it "is only supported for Call Analytics
            // (StartCallAnalyticsJob) transcription requests" (model doc for
            // `Media$RedactedMediaFileUri`), so `ContentRedaction` decides alone.
            let outputs = transcript_outputs(self.content_redaction.as_ref(), false);
            self.transcript = Some(Transcript {
                transcript_file_uri: outputs.unredacted.then(|| transcript_uri.clone()),
                redacted_transcript_file_uri: outputs
                    .redacted
                    .then(|| redacted_transcript_uri(&transcript_uri)),
            });
            // The subtitle files a completed job produced sit beside the
            // transcript, one per requested format.
            let subtitle_uris: Option<Vec<String>> = self.subtitles.as_ref().map(|subtitles| {
                subtitles
                    .formats
                    .iter()
                    .flatten()
                    .map(|format| self.output_uri(format))
                    .collect()
            });
            if let (Some(subtitles), Some(uris)) = (self.subtitles.as_mut(), subtitle_uris) {
                subtitles.subtitle_file_uris = Some(uris);
            }
        }
    }

    /// Record what the automatic language identification this job asked for
    /// determined, as part of completing it.
    ///
    /// Identifying the language is part of processing the media, so a job reports the
    /// result only once it has finished, and only when the request asked the service
    /// to identify the language instead of naming one: a job started with an explicit
    /// `LanguageCode` had nothing identified for it and reports neither a score nor a
    /// per-language breakdown.
    ///
    /// WHICH members a finished job carries follows from which of the two
    /// identifications it asked for: `LanguageCode` "is used with single-language
    /// identification. For multi-language identification requests, refer to the plural
    /// version of this parameter, `LanguageCodes`", and `IdentifiedLanguageScore` is
    /// "the confidence score associated with the language identified in your media
    /// file" — one language, so it accompanies the singular member
    /// (`smithy_query(query="shape", shape="TranscriptionJob")`).
    ///
    /// WHICH language each determination answers with follows from the MEDIA the job
    /// was pointed at, narrowed to the options the request allowed (see
    /// [`identified_language`] and [`identified_languages`]): the evidence was taken
    /// from the stored object when the job was started ([`Self::media_language`]), so
    /// both are a function of the stored record and every later read of the job reports
    /// the same ones.
    pub(super) fn materialise_identified_languages(&mut self) {
        let evidence = self.media_language.as_deref();
        if self.identify_multiple_languages == Some(true) {
            let codes = identified_languages(self.language_options.as_deref(), evidence);
            self.language_codes = Some(codes);
        } else if self.identify_language == Some(true) {
            let code = identified_language(self.language_options.as_deref(), evidence);
            self.language_code = Some(code);
            self.identified_language_score = Some(IDENTIFIED_LANGUAGE_SCORE);
        }
    }

    /// The URI of one of this job's output files.
    pub(super) fn output_uri(&self, extension: &str) -> String {
        output_uri(
            self.output_bucket_name.as_deref(),
            self.output_key.as_deref(),
            &self.name,
            &self.region,
            extension,
        )
    }

    /// This record as the modelled `TranscriptionJob` shape.
    pub fn to_job(&self) -> TranscriptionJob {
        TranscriptionJob {
            transcription_job_name: Some(self.name.clone()),
            transcription_job_status: Some(self.status.clone()),
            language_code: self.language_code.clone(),
            media_sample_rate_hertz: self.media_sample_rate_hertz,
            media_format: self.media_format.clone(),
            media: Some(self.media.clone()),
            transcript: self.transcript.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            settings: self.settings.clone(),
            model_settings: self.model_settings.clone(),
            job_execution_settings: self.job_execution_settings.clone(),
            content_redaction: self.content_redaction.clone(),
            identify_language: self.identify_language,
            identify_multiple_languages: self.identify_multiple_languages,
            language_options: self.language_options.clone(),
            identified_language_score: self.identified_language_score,
            language_codes: self.language_codes.clone(),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
            subtitles: self.subtitles.clone(),
            language_id_settings: self.language_id_settings.clone(),
            toxicity_detection: self.toxicity_detection.clone(),
        }
    }

    /// This record as the modelled `TranscriptionJobSummary` shape.
    pub fn to_summary(&self) -> TranscriptionJobSummary {
        TranscriptionJobSummary {
            transcription_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: self.language_code.clone(),
            transcription_job_status: Some(self.status.clone()),
            failure_reason: self.failure_reason.clone(),
            output_location_type: Some(self.output_location_type.clone()),
            content_redaction: self.content_redaction.clone(),
            model_settings: self.model_settings.clone(),
            identify_language: self.identify_language,
            identify_multiple_languages: self.identify_multiple_languages,
            identified_language_score: self.identified_language_score,
            language_codes: self.language_codes.clone(),
            toxicity_detection: self.toxicity_detection.clone(),
        }
    }
}

/// One stored MEDICAL transcription job (Amazon Transcribe Medical): exactly what the
/// request asked for, plus the lifecycle facts the service itself owns (status,
/// timestamps and the location its transcript is written to).
///
/// NOTHING IS TRANSCRIBED, exactly as for the non-medical family (see this module's
/// header): the `MedicalTranscript.TranscriptFileUri` a completed job reports names a
/// location holding a transcript with no words in it, and no PHI is identified. The job
/// RECORD is what this emulates.
///
/// `StartMedicalTranscriptionJob` has no deferred-execution member of any kind
/// (`smithy_query(query="operation", operation="StartMedicalTranscriptionJob")` lists
/// no `JobExecutionSettings`), so a job of this family is processing from the moment
/// it is started and has exactly one clock-owed transition, `IN_PROGRESS` ->
/// `COMPLETED`.
#[derive(Debug, Clone)]
pub struct MedicalTranscriptionJobRecord {
    /// `MedicalTranscriptionJobName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor a listing of this family
    /// would page with.
    pub sequence: u64,
    /// `TranscriptionJobStatus`: [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`] — or [`STATUS_FAILED`], for a job the service found
    /// something wrong with in the media it was pointed at ([`Self::failure_due`]).
    /// [`STATUS_QUEUED`] is modelled but unreached: nothing queues a job of this
    /// family.
    pub status: String,
    /// `LanguageCode`, a REQUIRED member the operation narrows to one value: "US
    /// English (en-US) is the only valid value for medical transcription jobs"
    /// ([`MEDICAL_LANGUAGE_CODE`]).
    pub language_code: String,
    pub media: Media,
    /// `MediaFormat`, the format of this job's input media, determined as
    /// [`TranscriptionJobRecord::media_format`] is: the two families' start requests
    /// carry the member alike, so one determination answers for both
    /// ([`determined_media_format`]).
    pub media_format: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    /// `OutputBucketName`, a REQUIRED member of the start request: the bucket the
    /// reported transcript URI names, so this family has no service-managed branch as
    /// [`output_uri`] has.
    pub output_bucket_name: String,
    pub output_key: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the transcript would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
    pub settings: Option<MedicalTranscriptionSetting>,
    pub content_identification_type: Option<String>,
    pub specialty: String,
    /// `Type`: whether the media is a `CONVERSATION` or a `DICTATION`. Named
    /// `job_type` here because `type` is a Rust keyword.
    pub job_type: String,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<MedicalTranscript>,
    /// Whether the document the location this job reports serves has already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
    /// `FailureReason`, set only on a [`STATUS_FAILED`] job — the reason the clock
    /// materialised from [`Self::failure_due`].
    pub failure_reason: Option<String>,
    /// The `FailureReason` the clock owes this job INSTEAD of a transcript, established
    /// from the media the start was pointed at ([`media_failure_reason`]); `None` for a
    /// job the service found nothing wrong with. Processing is what fails, so the job is
    /// accepted and reports this only once the clock has finished with it
    /// ([`Self::advance_clock`]).
    pub failure_due: Option<String>,
    pub creation_time: SystemTime,
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl MedicalTranscriptionJobRecord {
    /// Materialise the status transition the CLOCK already owes this job: it was due
    /// at the stored instant whether or not anybody called. A reporting operation
    /// (`Get`) runs this before projecting the record, and that is the only write it
    /// performs.
    ///
    /// Completing a job records WHERE its transcript is written; the operation that
    /// finds the job finished stores the document there, for the reason
    /// [`TranscriptionJobRecord::advance_clock`] gives. It records the format of the
    /// input audio the service determined too, exactly as that family's does
    /// ([`determined_media_format`]).
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != STATUS_IN_PROGRESS || now < self.completion_due {
            return;
        }
        self.completion_time = Some(self.completion_due);
        // A job whose media the service found wrong ends FAILED, reporting why and no
        // transcript: there is no output to name for a job that produced none.
        if let Some(reason) = self.failure_due.clone() {
            self.status = STATUS_FAILED.to_string();
            self.failure_reason = Some(reason);
            return;
        }
        self.status = STATUS_COMPLETED.to_string();
        self.media_format = determined_media_format(self.media_format.take(), &self.media);
        self.transcript = Some(MedicalTranscript {
            // The same `OutputKey` rule the non-medical family applies, which this
            // operation documents in the same words — so one implementation
            // ([`output_uri`]) answers for both. `OutputBucketName` is REQUIRED for
            // this family, so the transcript is always in the caller's bucket and the
            // `region` argument (which only the service-managed branch reads) cannot
            // reach the URI.
            transcript_file_uri: Some(output_uri(
                Some(&self.output_bucket_name),
                self.output_key.as_deref(),
                &self.name,
                DEFAULT_REGION,
                "json",
            )),
        });
    }

    /// This record as the modelled `MedicalTranscriptionJob` shape, which is the whole
    /// answer of `StartMedicalTranscriptionJob` and `GetMedicalTranscriptionJob`.
    ///
    /// `Tags` is left ABSENT for a job carrying none rather than reported as an empty
    /// list, for the reason [`CallAnalyticsCategoryRecord::to_properties`] gives:
    /// `TagList` is modelled `length {min 1, max 200}`, so an empty list is not a
    /// value of that shape.
    pub fn to_job(&self) -> MedicalTranscriptionJob {
        MedicalTranscriptionJob {
            medical_transcription_job_name: Some(self.name.clone()),
            transcription_job_status: Some(self.status.clone()),
            language_code: Some(self.language_code.clone()),
            media_sample_rate_hertz: self.media_sample_rate_hertz,
            media_format: self.media_format.clone(),
            media: Some(self.media.clone()),
            transcript: self.transcript.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            settings: self.settings.clone(),
            content_identification_type: self.content_identification_type.clone(),
            specialty: Some(self.specialty.clone()),
            r#type: Some(self.job_type.clone()),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
        }
    }

    /// This record as the modelled `MedicalTranscriptionJobSummary` shape, which is
    /// what `ListMedicalTranscriptionJobs` reports one stored job as.
    ///
    /// `OutputLocationType` is always [`OUTPUT_LOCATION_CUSTOMER_BUCKET`] for this
    /// family: `OutputBucketName` is a REQUIRED member of the start request
    /// (`smithy_query(query="operation", operation="StartMedicalTranscriptionJob")`),
    /// so a job of this family never writes to a service-managed bucket. The summary
    /// carries no `Media`, `Transcript` or `Tags` member at all — those are read
    /// through `GetMedicalTranscriptionJob`.
    pub fn to_summary(&self) -> MedicalTranscriptionJobSummary {
        MedicalTranscriptionJobSummary {
            medical_transcription_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: Some(self.language_code.clone()),
            transcription_job_status: Some(self.status.clone()),
            failure_reason: self.failure_reason.clone(),
            output_location_type: Some(OUTPUT_LOCATION_CUSTOMER_BUCKET.to_string()),
            specialty: Some(self.specialty.clone()),
            content_identification_type: self.content_identification_type.clone(),
            r#type: Some(self.job_type.clone()),
        }
    }
}

/// One stored Medical Scribe job (AWS HealthScribe): exactly what the request asked
/// for, plus the lifecycle facts the service itself owns (status, timestamps and the
/// location its output is written to).
///
/// `StartMedicalScribeJob` has no deferred-execution member of any kind
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`), so a job
/// of this family is processing from the moment it is started and has exactly one
/// clock-owed transition, `IN_PROGRESS` -> `COMPLETED`.
#[derive(Debug, Clone)]
pub struct MedicalScribeJobRecord {
    /// `MedicalScribeJobName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor a listing of this family
    /// would page with.
    pub sequence: u64,
    /// `MedicalScribeJobStatus`: [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`]. The enum's other two values are modelled but unreached —
    /// nothing queues such a job and nothing this emulator does can fail one.
    pub status: String,
    pub media: Media,
    /// `OutputBucketName`, a REQUIRED member of the start request: the bucket both
    /// reported output URIs name.
    pub output_bucket_name: String,
    /// `OutputEncryptionKMSKeyId`: the key the output would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
    pub data_access_role_arn: String,
    /// `Settings`, as [`recorded_medical_scribe_settings`] records them: what the start
    /// asked for, with the documented clinical-note template default materialised.
    pub settings: MedicalScribeSettings,
    pub channel_definitions: Option<Vec<MedicalScribeChannelDefinition>>,
    /// Whether the start request carried a `MedicalScribeContext`, which is the ONLY
    /// thing the model reports about it (`MedicalScribeJob$MedicalScribeContextProvided`
    /// "Indicates whether the MedicalScribeContext object was provided"). The context
    /// itself is not kept: no operation of this service reports it, and its one member
    /// (`MedicalScribePatientContext$Pronouns`) is modelled `@sensitive`.
    pub context_provided: bool,
    /// Where the transcript and the clinical note were written; `None` until the
    /// job completes. See [`medical_scribe_output_uri`].
    pub output: Option<MedicalScribeOutput>,
    /// Whether the documents the locations this job reports serve have already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
    /// `FailureReason`, set only on a `FAILED` job — which none of this emulator's are.
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl MedicalScribeJobRecord {
    /// Materialise the status transition the CLOCK already owes this job: it was due
    /// at the stored instant whether or not anybody called. A reporting operation
    /// (`Get`) runs this before projecting the record, and that is the only write it
    /// performs.
    ///
    /// Completing a job records WHERE its output is written; the operation that finds
    /// the job finished stores the documents there, for the reason
    /// [`TranscriptionJobRecord::advance_clock`] gives.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status != STATUS_IN_PROGRESS || now < self.completion_due {
            return;
        }
        self.status = STATUS_COMPLETED.to_string();
        self.completion_time = Some(self.completion_due);
        self.output = Some(MedicalScribeOutput {
            transcript_file_uri: medical_scribe_output_uri(
                &self.output_bucket_name,
                &self.name,
                MEDICAL_SCRIBE_TRANSCRIPT_FILE,
            ),
            clinical_document_uri: medical_scribe_output_uri(
                &self.output_bucket_name,
                &self.name,
                MEDICAL_SCRIBE_CLINICAL_DOCUMENT_FILE,
            ),
        });
    }

    /// This record as the modelled `MedicalScribeJob` shape, which is the whole answer
    /// of `StartMedicalScribeJob` and `GetMedicalScribeJob`.
    ///
    /// `Tags` is left ABSENT for a job carrying none rather than reported as an empty
    /// list, for the reason [`CallAnalyticsCategoryRecord::to_properties`] gives:
    /// `TagList` is modelled `length {min 1, max 200}`, so an empty list is not a
    /// value of that shape.
    pub fn to_job(&self) -> MedicalScribeJob {
        MedicalScribeJob {
            medical_scribe_job_name: Some(self.name.clone()),
            medical_scribe_job_status: Some(self.status.clone()),
            language_code: Some(MEDICAL_SCRIBE_LANGUAGE_CODE.to_string()),
            media: Some(self.media.clone()),
            medical_scribe_output: self.output.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            settings: Some(self.settings.clone()),
            data_access_role_arn: Some(self.data_access_role_arn.clone()),
            channel_definitions: self.channel_definitions.clone(),
            medical_scribe_context_provided: Some(self.context_provided),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
        }
    }

    /// This record as the modelled `MedicalScribeJobSummary` shape, which is what
    /// `ListMedicalScribeJobs` reports one stored job as.
    ///
    /// The summary is a strict projection of the same record `GetMedicalScribeJob`
    /// reports: it carries only the seven members the shape declares
    /// (`smithy_query(query="shape", shape="MedicalScribeJobSummary")`), so the
    /// media, settings, channel definitions, output locations and tags of a job are
    /// read through `GetMedicalScribeJob` rather than from a listing. `LanguageCode`
    /// is [`MEDICAL_SCRIBE_LANGUAGE_CODE`] for every job of this family, for the
    /// reason that constant gives.
    pub fn to_summary(&self) -> MedicalScribeJobSummary {
        MedicalScribeJobSummary {
            medical_scribe_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: Some(MEDICAL_SCRIBE_LANGUAGE_CODE.to_string()),
            medical_scribe_job_status: Some(self.status.clone()),
            failure_reason: self.failure_reason.clone(),
        }
    }
}

/// `RedactionOutput` values, from the model's enum
/// (`smithy_query(query="shape", shape="RedactionOutput")`).
pub const REDACTION_OUTPUT_REDACTED: &str = REDACTION_OUTPUT_VALUES[0];
/// See [`REDACTION_OUTPUT_REDACTED`].
pub const REDACTION_OUTPUT_REDACTED_AND_UNREDACTED: &str = REDACTION_OUTPUT_VALUES[1];

/// Which of the two transcripts the modelled `Transcript` shape can report a finished
/// job produced, and so which of its two locations that job reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptOutputs {
    /// Whether an unredacted transcript was produced (`Transcript$TranscriptFileUri`).
    pub unredacted: bool,
    /// Whether a redacted one was (`Transcript$RedactedTranscriptFileUri`).
    pub redacted: bool,
}

/// What a job asked the service to produce, read from the two members that ask for
/// redaction.
///
/// `ContentRedaction$RedactionOutput` is a required member wherever
/// `ContentRedaction` is given, and it decides this: "When you choose `redacted`
/// Amazon Transcribe creates only a redacted transcript. When you choose
/// `redacted_and_unredacted` Amazon Transcribe creates a redacted and an unredacted
/// transcript (as two separate files)"
/// (`smithy_query(query="doc", shape="ContentRedaction", member="RedactionOutput")`).
/// `Media$RedactedMediaFileUri` asks for a redacted transcript by another route — it
/// "produces a redacted audio file in addition to a redacted transcript" — and
/// documents nothing about dropping the unredacted one, so a job that named it and no
/// `ContentRedaction` produces both.
///
/// A job that asked for neither produces the unredacted transcript alone, which is
/// why `RedactedTranscriptFileUri` is not reported for every job.
pub fn transcript_outputs(
    content_redaction: Option<&ContentRedaction>,
    redacts_media: bool,
) -> TranscriptOutputs {
    match content_redaction.map(|redaction| redaction.redaction_output.as_str()) {
        Some(REDACTION_OUTPUT_REDACTED) => TranscriptOutputs {
            unredacted: false,
            redacted: true,
        },
        Some(_) => TranscriptOutputs {
            unredacted: true,
            redacted: true,
        },
        None => TranscriptOutputs {
            unredacted: true,
            redacted: redacts_media,
        },
    }
}

/// What distinguishes the redacted transcript's location from the unredacted one's.
///
/// CHOSEN: the model documents the two as "two separate files" at two locations
/// (`Transcript$TranscriptFileUri` / `Transcript$RedactedTranscriptFileUri`) but names
/// no key for either, so the redacted one is the unredacted file name under this
/// prefix — in whatever folder or bucket the request directed the output to.
pub const REDACTED_OUTPUT_PREFIX: &str = "redacted-";

/// Where the REDACTED transcript of a job that asked for redaction is reported to
/// live, beside the unredacted one at `transcript_uri`.
///
/// Reported AND served, exactly as the unredacted location is (see this module's
/// header): nothing redacts a transcript nothing produced, so the document stored
/// there is the same contentless transcript — at a location distinct from the
/// unredacted one, as the model's two separate members require.
pub fn redacted_transcript_uri(transcript_uri: &str) -> String {
    match transcript_uri.rsplit_once('/') {
        Some((parent, file)) => format!("{parent}/{REDACTED_OUTPUT_PREFIX}{file}"),
        None => format!("{REDACTED_OUTPUT_PREFIX}{transcript_uri}"),
    }
}

/// The `MediaFormat` of the audio a job was given, as the media URI it was given
/// evidences it — `None` for a URI that evidences no modelled format.
///
/// `MediaFormat` is "The format of the input media file"
/// (`smithy_query(query="shape", shape="CallAnalyticsJob")`) and no member of
/// `StartCallAnalyticsJob` carries it, so the service reports what it found in the
/// file. Nothing here opens that file, and the one thing the request says about it is
/// the name it is stored under: a URI whose last path segment ends in one of the
/// modelled `MediaFormat` extensions ([`MEDIA_FORMAT_VALUES`]) is reported as that
/// format — `Media`'s own documented examples are spelled that way
/// (`s3://DOC-EXAMPLE-BUCKET/my-media-file.flac`) — and a URI that ends in no such
/// extension leaves the member absent rather than guessed.
///
/// `MediaFileUri` is the input audio; a job that named only `RedactedMediaFileUri`
/// gave its input audio there instead, so that URI answers for it.
pub fn media_format_of(media: &Media) -> Option<String> {
    let uri = [
        media.media_file_uri.as_deref(),
        media.redacted_media_file_uri.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|uri| !uri.is_empty())?;
    // The path, without the query or fragment an http(s) URI may carry.
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let extension = path.rsplit('/').next()?.rsplit_once('.')?.1.to_ascii_lowercase();
    MEDIA_FORMAT_VALUES
        .iter()
        .find(|format| **format == extension)
        .map(|format| (*format).to_string())
}

/// The `MediaFormat` a job that has just FINISHED reports for the media it processed:
/// the format the request declared, or — where it declared none — the one the service
/// determined from the media it was given ([`media_format_of`]).
///
/// `MediaFormat` is "The format of the input media file" on every job shape that reports
/// one (`smithy_query(query="doc", shape="TranscriptionJob", member="MediaFormat")`, and
/// the same sentence on `MedicalTranscriptionJob` and `CallAnalyticsJob`), so it is the
/// format of the FILE rather than an echo of what a request guessed: a finished job
/// reports what the service established about its input audio whether or not the request
/// said anything about it. The two can never disagree on a job that FINISHED — a declared
/// format the stored media contradicts ends the job `FAILED` instead
/// ([`media_failure_reason`]) — so a declared format is kept as the start reported it and
/// the determination fills the member in only where the request left it blank. Media that
/// evidences no modelled format leaves it absent rather than guessed, as
/// [`media_format_of`] does.
///
/// One determination for all three families that report the member: the two
/// transcription families, whose start request carries a `MediaFormat` of its own, and
/// the Call Analytics family, whose start carries none at all.
pub fn determined_media_format(declared: Option<String>, media: &Media) -> Option<String> {
    declared.or_else(|| media_format_of(media))
}

/// The Amazon S3 bucket an `s3://` URI addresses, together with everything after it —
/// the key, or the key PREFIX, which is empty for a URI that names the bucket alone.
///
/// The ONE `s3://` parse this service makes: the training and tuning locations of a
/// custom language model ([`read_training_location`]) are prefixes, so they may name a
/// bucket with nothing after it, while a URI naming one OBJECT layers the
/// non-empty-key requirement on top of this ([`s3_object_address`]) — so the two
/// cannot disagree about what a URI addresses.
///
/// `None` for a value that addresses no bucket, including the `http(s)://` form the
/// `Uri` pattern also admits, which addresses a HOST, and this emulator resolves no
/// hosts, so such a URI names nothing it can ask an object store about.
pub(super) fn s3_bucket_and_path(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.trim().strip_prefix("s3://")?;
    let (bucket, path) = rest.split_once('/').unwrap_or((rest, ""));
    (!bucket.is_empty()).then_some((bucket, path))
}

/// The Amazon S3 object a URI addresses: its bucket and its key.
///
/// Every member of this service that names ONE object spells it the same way —
/// `Media$MediaFileUri` is "The Amazon S3 location of the media file you want to
/// transcribe", spelled `s3://DOC-EXAMPLE-BUCKET/media-files/my-media-file.flac` in the
/// member's own examples, and `CreateMedicalVocabularyRequest$VocabularyFileUri` is "The
/// Amazon S3 location (URI) of the text file that contains your custom medical
/// vocabulary", spelled `s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt` — so the first path
/// segment is the bucket and everything after it is the key, and one parse answers for
/// the media a job is pointed at and the file a vocabulary is built from alike.
/// [`s3_bucket_and_path`] with the requirement those members add: such a URI names an
/// OBJECT, so a URI naming only a bucket addresses none.
pub(super) fn s3_object_address(uri: &str) -> Option<(&str, &str)> {
    let (bucket, key) = s3_bucket_and_path(uri)?;
    (!key.is_empty()).then_some((bucket, key))
}

/// Whether `uri` is of the Amazon S3 scheme and yet addresses NO object of it
/// ([`s3_object_address`]): `s3://` with no bucket, or a bucket with no key under it.
///
/// Such a URI needs no object store to judge: the media member is "The Amazon S3
/// location of the media file you want to transcribe", and every example of it names a
/// bucket AND a key (`s3://DOC-EXAMPLE-BUCKET/my-media-file.flac`), so an `s3://` URI
/// carrying no key addresses a bucket rather than a media file and names no object the
/// service could ever read — unlike the `http(s)://` form, which addresses a host this
/// emulator cannot resolve and so leaves the media simply unknown.
pub(super) fn s3_uri_addresses_no_object(uri: &str) -> bool {
    uri.trim().starts_with("s3://") && s3_object_address(uri).is_none()
}

/// What asking this emulator's own Amazon S3 about the media a start request named
/// established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaVerdict {
    /// No object of the object store answers for the media the request named — either
    /// Amazon S3 reports that an object it named is not there, or the URI is of the S3
    /// scheme and addresses no object of it at all
    /// ([`s3_uri_addresses_no_object`]): the start is refused and nothing is stored.
    NoSuchObject,
    /// Every object the request addressed resolved, so the service has seen the stored
    /// media it is to process.
    Resolved,
    /// Nothing is known about the media: there was no object store to ask, the URI
    /// addresses no object this emulator can resolve ([`s3_object_address`]), or only
    /// SOME of the objects the request addressed resolved. Nothing is refused and no job
    /// is failed on an assumption.
    Unknown,
}

/// The refusal a request that names an Amazon S3 object the store holds none of gets:
/// "To make a StartMedicalTranscriptionJob request, you must first upload your media
/// file into an Amazon S3 bucket; you can then specify the Amazon S3 location of the
/// file using the Media parameter" — the same precondition every job family's start
/// documents, and the same one a custom vocabulary's table file carries ("you must
/// first upload a text file that contains your vocabulary table into an Amazon S3
/// bucket"), so one wording answers for every member of this service that names an
/// object: the file a request points at is not there.
pub const URI_NAMES_NO_OBJECT: &str =
    "The URI that you provided doesn't point to an S3 object. Make sure that the object \
     exists and try your request again.";

/// The Amazon S3 error codes in which S3 reports that an object a request names is NOT
/// THERE: a bucket it does not hold (`NoSuchBucket`), a key that bucket does not hold
/// (`NoSuchKey`), a version it holds none of (`NoSuchVersion`), and the code `HeadObject`
/// models for either (`NotFound`, which a HEAD carries no body to spell out).
///
/// The code is required TOGETHER WITH the 404 status ([`object_is_absent`]), and never
/// the status alone, because a status is not evidence that S3 answered at all: a request
/// that reached no S3 is a 404 too, and that is this emulator failing to ask rather than
/// a verdict about the object. The list is deliberately CLOSED, and holds only the codes
/// that say the object is absent — the refusal this wording states
/// ([`URI_NAMES_NO_OBJECT`]) is about an object that is not there, so a code
/// reporting something else about it (a read that is not permitted, a refusal S3 adds
/// later) answers [`MediaVerdict::Unknown`] and refuses nothing.
pub(super) const ABSENT_OBJECT_CODES: &[&str] = &["NoSuchBucket", "NoSuchKey", "NoSuchVersion", "NotFound"];

/// Whether S3's answer to a read is S3 ITSELF saying it holds nothing at that
/// address: HTTP 404 AND one of the codes it sends for a missing one
/// ([`ABSENT_OBJECT_CODES`]).
///
/// One judgement for every read this service makes of a caller-named object, whichever
/// operation of the object store asked: the `HeadObject` a job's media is resolved with
/// ([`resolve_media_objects`]) and the `GetObject` a vocabulary's file is read with
/// ([`resolve_vocabulary_file`]) answer an absent object the same way, so what "the
/// object is not there" means cannot differ between them.
pub(super) fn object_is_absent<E>(
    error: &::aws_sdk_s3::error::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool
where
    E: ::aws_sdk_s3::error::ProvideErrorMetadata,
{
    use ::aws_sdk_s3::error::ProvideErrorMetadata;

    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 404)
        && error
            .code()
            .is_some_and(|code| ABSENT_OBJECT_CODES.contains(&code))
}

/// Resolve every media object `uris` names against this emulator's own Amazon S3 and
/// report what that established.
///
/// A job's media is resolved when the job is STARTED: "you must first upload your media
/// file into an Amazon S3 bucket; you can then specify the Amazon S3 location of the
/// file using the Media parameter", so a start naming an object the store does not hold
/// is refused with the operation's modelled `BadRequestException`
/// ([`URI_NAMES_NO_OBJECT`]) and nothing is persisted. S3 is reached the way any
/// emulated service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`], see [`s3_client`] — no
/// socket, no outbound request, no host resolution).
///
/// The verdict is read from S3's own absent-object answer and never from an HTTP status
/// alone ([`object_is_absent`]): with no S3 to ask (a router with only this service
/// mounted, say) nothing is known about the media, so the start is accepted — a refusal
/// has to be grounded in what the object store answered, never assumed. The one refusal
/// that needs no answer is a URI of the S3 scheme that addresses no object of it
/// ([`s3_uri_addresses_no_object`]): it names no media file for any store to hold.
///
/// The read is a `HeadObject`, "the same options as a GET operation on an object"
/// without its body: what a start has to establish is that the media it was pointed at
/// IS there, and nothing in this service transcribes audio, so no media file's bytes are
/// ever fetched or held. What that answer says ABOUT the object is what the language the
/// media evidences is read from ([`media_language_evidence`]), so a start that has to
/// identify a language takes its evidence from the same read.
pub async fn resolve_media_objects(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uris: &[&str],
) -> MediaResolution {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    // One client for the whole set: a family that names its input audio in two members
    // resolves both through it.
    let client = s3_client(ctx, region);
    let mut asked = 0usize;
    let mut resolved = 0usize;
    let mut language: Option<String> = None;
    for uri in uris {
        let Some((bucket, key)) = s3_object_address(uri) else {
            // A URI of the S3 scheme that addresses no object of it names no media file
            // at all ([`s3_uri_addresses_no_object`]), so the start is refused without
            // asking the store: there is no object for it to answer about.
            if s3_uri_addresses_no_object(uri) {
                return MediaResolution::verdict(MediaVerdict::NoSuchObject);
            }
            continue;
        };
        asked += 1;
        match client.head_object().bucket(bucket).key(key).send().await {
            // The object is there. Only its METADATA was asked for, never its bytes:
            // nothing in this service transcribes audio, so what the start needs to
            // know is that the media it was pointed at exists — and what that metadata
            // says about the object is what identification reads.
            Ok(head) => {
                resolved += 1;
                // The input audio is the FIRST media member the request named (the
                // families that name two give the same audio in either), so the first
                // object that resolved is the one a language is identified in.
                if language.is_none() {
                    language = media_language_evidence(head.content_language(), uri);
                }
            }
            Err(error) => {
                if object_is_absent(&error) {
                    // S3 says the object is not there: the refusal is grounded.
                    return MediaResolution::verdict(MediaVerdict::NoSuchObject);
                }
                // Any other answer is not S3 reporting an absent object, so it refuses
                // nothing — and it is worth a warning, because a deployment with no S3
                // mounted to ask is one where this check does not run. The caller's own
                // values are rendered with their escaped `Debug` form, so a key carrying
                // a newline cannot forge a log line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    code = ?error.code(),
                    "the media object could not be resolved against this emulator's Amazon S3"
                );
            }
        }
    }
    // `Resolved` says the service has seen EVERY object the request addressed, which is
    // what judging the media it was given rests on: one object that resolved beside one
    // the store could not be asked about leaves the set unknown, whatever order they
    // were asked in.
    MediaResolution {
        verdict: if asked > 0 && resolved == asked {
            MediaVerdict::Resolved
        } else {
            MediaVerdict::Unknown
        },
        // Evidence stands on the object it was read from: it was taken from an object
        // the store ANSWERED for, so it says what that media is whatever the set's
        // verdict is.
        language,
    }
}

/// What the object store's answer about a stored media object says about the LANGUAGE
/// of the audio in it — `None` for media that evidences none.
///
/// Nothing decodes the audio (see this module's header: nothing is transcribed), so
/// identification reads what the stored object itself states about its language, most
/// authoritative first: the `Content-Language` the store holds for it
/// ([`declared_language_tag`]), which is where stored content declares its natural
/// language, and failing that the language tag its own address carries
/// ([`address_language_tag`]) — the same kind of evidence this service reads the FORMAT
/// of a stored media object from ([`media_format_of`]).
///
/// The result is a language TAG rather than a `LanguageCode`: which code a job reports
/// for it also depends on what that job's request allowed identification to answer with
/// ([`narrowed_identified_language`]), which is settled when the job finishes.
pub fn media_language_evidence(content_language: Option<&str>, uri: &str) -> Option<String> {
    declared_language_tag(content_language).or_else(|| address_language_tag(uri))
}

/// What resolving the media a start was pointed at against this emulator's own Amazon
/// S3 established: whether those objects are there, and what the stored media evidences
/// about the language of its audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaResolution {
    /// Whether the object store answers for every object the request addressed
    /// ([`MediaVerdict`]).
    pub verdict: MediaVerdict,
    /// The language tag the stored media evidences, for the families that IDENTIFY a
    /// language rather than being told one ([`media_language_evidence`]); `None` where
    /// the media evidenced none, or where no object of it resolved.
    pub language: Option<String>,
}

impl MediaResolution {
    /// A resolution carrying `verdict` and no language evidence: what a set holding an
    /// object the store says is not there established, since nothing about that media
    /// is to be read.
    fn verdict(verdict: MediaVerdict) -> Self {
        Self {
            verdict,
            language: None,
        }
    }
}

/// The `FailureReason` of a job whose declared `MediaFormat` is not the format of the
/// media the service found at the location the request named: "The media format
/// specified in `MediaFormat` doesn't match the format of the input file" — one of the
/// values the model enumerates for that member (`smithy_query(query="doc",
/// shape="MedicalTranscriptionJob", member="FailureReason")`), reported verbatim as the
/// doc spells it.
pub const MEDIA_FORMAT_MISMATCH_FAILURE_REASON: &str =
    "The media format provided does not match the detected media format";

/// The `FailureReason` the clock owes a job started with `declared` as its
/// `MediaFormat` over the media `media` names, or `None` for a job the service found
/// nothing wrong with.
///
/// A format MISMATCH is the one per-media failure this emulator can establish: the
/// format of the stored media is what its name evidences ([`media_format_of`]), and the
/// request states what the caller believes it to be, so the two can disagree. Judged
/// only for a job whose media actually RESOLVED ([`MediaVerdict::Resolved`]) — the
/// service has to have seen the file to have detected its format — and never for the
/// members only the audio itself could answer (the sample rate, the file size), which
/// stay unjudged rather than guessed.
pub fn media_failure_reason(
    verdict: MediaVerdict,
    declared: Option<&str>,
    media: &Media,
) -> Option<String> {
    if verdict != MediaVerdict::Resolved {
        return None;
    }
    let declared = declared?;
    let detected = media_format_of(media)?;
    (detected != declared).then(|| MEDIA_FORMAT_MISMATCH_FAILURE_REASON.to_string())
}

/// The `MediaSampleRateHertz` a finished Call Analytics job reports: "The sample rate,
/// in hertz, of the audio track in your input media file".
///
/// No member of `StartCallAnalyticsJob` carries it either, and unlike the format the
/// request says nothing at all about it, so this is a CHOSEN figure: the lowest rate
/// the member itself models (`@range(min: 8000, max: 48000)`), which is the rate the
/// telephone call audio this family analyses is sampled at. One value for every job of
/// the family, fixed when the job finishes, so what a job reports never changes
/// between reads.
pub const CALL_AUDIO_SAMPLE_RATE_HERTZ: i32 = 8_000;

/// The language a job reports when the request narrowed identification to nothing.
///
/// A CHOSEN value of the modelled `LanguageCode` enum
/// ([`crate::ops::types::LANGUAGE_CODE_VALUES`]), used only where
/// [`identified_language`] and [`identified_languages`] have no option list to read.
pub const DEFAULT_IDENTIFIED_LANGUAGE: &str = "en-US";

/// The `IdentifiedLanguageScore` that accompanies an identified language:
/// "Confidence scores are values between 0 and 1; a larger value indicates a higher
/// probability" — the same sentence on both shapes that report one
/// (`smithy_query(query="doc", shape="CallAnalyticsJob", member="IdentifiedLanguageScore")`,
/// `smithy_query(query="doc", shape="TranscriptionJob", member="IdentifiedLanguageScore")`).
///
/// This emulator's identification is not probabilistic — it reads the language the
/// stored media EVIDENCES rather than weighing audio (see
/// [`media_language_evidence`]) — so what it reports is the top of that documented
/// range, the certainty of a determination that weighed nothing, rather than an
/// invented probability.
pub const IDENTIFIED_LANGUAGE_SCORE: f32 = 1.0;

/// The single language a job whose language the service IDENTIFIED was transcribed
/// in: the language its MEDIA evidences, narrowed to the options the request allowed.
///
/// Two families ask the service to identify one language: a Call Analytics job, which
/// has no language member to name one with, and a transcription job started with
/// `IdentifyLanguage` (`IdentifyMultipleLanguages` asks for the plural determination
/// [`identified_languages`] makes instead). Both narrow the choice with a
/// `LanguageOptions` list and both report the result as the finished job's
/// `LanguageCode`, so one determination answers for both rather than each family
/// making its own.
///
/// `StartCallAnalyticsJob` carries NO language member
/// (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`): Amazon
/// Transcribe identifies the language of the call itself, and
/// `CallAnalyticsJobSettings$LanguageOptions` only narrows what identification may
/// choose between ("You can specify two or more language codes that represent the
/// languages you think may be present in your media"). So the language is the
/// service's own determination, and a job that finished has one — "The language code
/// used to create your Call Analytics job"
/// (`smithy_query(query="shape", shape="CallAnalyticsJob")`) — however many options
/// the request listed, or none.
///
/// That determination is about the MEDIA: `media_language` is the language the stored
/// object the job was pointed at evidenced when the job was started
/// ([`media_language_evidence`]), and the answer is that language in the spelling
/// identification was allowed to report it in ([`narrowed_identified_language`]), so
/// two jobs differing only in their media can identify different languages. Media that
/// evidenced nothing leaves identification with nothing to go on, and the answer is
/// then drawn from the request alone: the FIRST code `LanguageOptions` lists, since
/// identification may only answer with one of them, and
/// [`DEFAULT_IDENTIFIED_LANGUAGE`] where the request narrowed the choice to nothing.
/// Either way it is a function of the stored record, so every read of a job reports
/// the same language.
pub fn identified_language(
    language_options: Option<&[String]>,
    media_language: Option<&str>,
) -> String {
    media_language
        .and_then(|tag| narrowed_identified_language(tag, language_options))
        .or_else(|| language_options.and_then(<[String]>::first).cloned())
        .unwrap_or_else(|| DEFAULT_IDENTIFIED_LANGUAGE.to_string())
}

/// Every language a MULTI-language identification job identified, narrowed to the
/// options the request allowed.
///
/// `IdentifyMultipleLanguages` "enables automatic multi-language identification in
/// your transcription job request ... you can optionally include a list of language
/// codes, using LanguageOptions ... Including LanguageOptions restricts
/// [identification] to only the language options that you specify"
/// (`smithy_query(query="operation", operation="StartTranscriptionJob")`), and the
/// finished job reports one `LanguageCodeItem` per language identified in the media
/// (`smithy_query(query="shape", shape="LanguageCodeItem")`).
///
/// One entry per language the MEDIA evidences, as the singular determination
/// [`identified_language`] makes is: media evidencing one language was identified as
/// holding that one language, in the spelling identification was allowed to report it
/// in ([`narrowed_identified_language`]), so two jobs differing only in their media can
/// identify different languages. Where the stored media evidenced nothing, the
/// determination falls back to the request: every code the options allowed, in the
/// order the request listed them, since identification may answer with no other and
/// the options are the caller's own statement of the "language codes that represent the
/// languages you think may be present in your media" — reporting a SUBSET of them with
/// nothing evidencing which would take the audio that says what is actually spoken.
/// Where the request narrowed the choice to nothing either, the one
/// [`DEFAULT_IDENTIFIED_LANGUAGE`] is the whole answer. Every form of the answer is a
/// function of the stored record, so every read of a job reports the same languages in
/// the same order.
///
/// `DurationInSeconds` is "the total time, in seconds, each identified language is
/// spoken in your media" — a measurement of the media itself, which nothing here
/// reads (see the module header: nothing is transcribed) — so each entry leaves that
/// optional member ABSENT rather than reporting a duration nothing timed.
pub fn identified_languages(
    language_options: Option<&[String]>,
    media_language: Option<&str>,
) -> Vec<LanguageCodeItem> {
    let evidenced =
        media_language.and_then(|tag| narrowed_identified_language(tag, language_options));
    let default = [DEFAULT_IDENTIFIED_LANGUAGE.to_string()];
    let codes = match (evidenced.as_ref(), language_options) {
        (Some(code), _) => std::slice::from_ref(code),
        (None, Some(options)) if !options.is_empty() => options,
        _ => default.as_slice(),
    };
    codes
        .iter()
        .map(|code| LanguageCodeItem {
            language_code: Some(code.clone()),
            duration_in_seconds: None,
        })
        .collect()
}

/// One stored Call Analytics job: exactly what the request asked for, plus the
/// lifecycle facts the service itself owns (status, timestamps and the location its
/// transcript is written to).
///
/// This family has job QUEUEING, and the operation names the request member that opts
/// into it: "Job queuing is available for Call Analytics jobs. If you pass a
/// DataAccessRoleArn in your request and you exceed your Concurrent Job Limit, your job
/// will automatically be added to a queue to be processed once your concurrent job
/// count is below the limit"
/// (`smithy_query(query="doc", shape="StartCallAnalyticsJob")`). So a start that passes
/// that member while the family's concurrency ceiling is already reached
/// ([`MAX_CONCURRENT_CALL_ANALYTICS_JOBS`]) is ACCEPTED in [`STATUS_QUEUED`] with no
/// start time and has two clock-owed transitions, `QUEUED` -> `IN_PROGRESS` ->
/// `COMPLETED`, exactly as a deferred batch transcription job does
/// ([`TranscriptionJobRecord`]); a start at that ceiling that passes no role has opted
/// into nothing and is refused, as is one that would make the queue deeper than this
/// emulator keeps it ([`MAX_QUEUED_CALL_ANALYTICS_JOBS`]), and every accepted job below
/// the ceiling is processing from the moment it is started.
///
/// WHAT IT EMULATES is the job RECORD (see the module header): no call is
/// transcribed, no sentiment or call characteristic is derived and no category is
/// matched, so the `Transcript.TranscriptFileUri` a completed job reports names a
/// location holding a transcript with none of that in it.
#[derive(Debug, Clone)]
pub struct CallAnalyticsJobRecord {
    /// `CallAnalyticsJobName`, with the Region, this record's key in the store.
    pub name: String,
    /// Creation order, newest = largest. Also the cursor `ListCallAnalyticsJobs`
    /// pages with.
    pub sequence: u64,
    /// `CallAnalyticsJobStatus`: [`STATUS_QUEUED`] while the job waits for a
    /// concurrency slot, then [`STATUS_IN_PROGRESS`] until the clock owes it
    /// [`STATUS_COMPLETED`]. [`STATUS_FAILED`] is modelled but unreached — nothing
    /// this emulator does can fail a job it accepted.
    pub status: String,
    pub media: Media,
    /// `OutputLocation`, the one output member this operation takes; `None` when the
    /// request named none, in which case the transcript is reported in a
    /// service-managed location (see [`call_analytics_output_uri`]).
    pub output_location: Option<String>,
    /// `OutputEncryptionKMSKeyId`: the key the output would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because this emulator holds
    /// no key to encrypt the document it writes with.
    pub output_encryption_kms_key_id: Option<String>,
    /// `DataAccessRoleArn`, "The Amazon Resource Name (ARN) you included in your
    /// request" (model doc for `CallAnalyticsJob$DataAccessRoleArn`) — optional on
    /// this operation, so `None` when the request named none.
    pub data_access_role_arn: Option<String>,
    /// The region the start request was signed for; the service-managed transcript
    /// URI is in that region.
    pub region: String,
    pub settings: Option<CallAnalyticsJobSettings>,
    pub channel_definitions: Option<Vec<ChannelDefinition>>,
    /// `LanguageCode`, the language the service identified for this call; `None`
    /// until the job completes, because identification is part of processing it
    /// ([`identified_language`]).
    pub language_code: Option<String>,
    /// The language the media this job was pointed at EVIDENCES, established from the
    /// stored object when the job was started ([`media_language_evidence`]); what
    /// identification reads when the job completes.
    pub media_language: Option<String>,
    /// `IdentifiedLanguageScore`, set with [`Self::language_code`]
    /// ([`IDENTIFIED_LANGUAGE_SCORE`]).
    pub identified_language_score: Option<f32>,
    /// `MediaFormat`, the format the service determined the input audio is in; `None`
    /// until the job completes, and for a media URI that evidences no modelled format
    /// ([`determined_media_format`], which this family reaches with nothing declared:
    /// its start request carries no `MediaFormat` member at all).
    pub media_format: Option<String>,
    /// `MediaSampleRateHertz`, the rate the service determined the input audio is
    /// sampled at; `None` until the job completes
    /// ([`CALL_AUDIO_SAMPLE_RATE_HERTZ`]).
    pub media_sample_rate_hertz: Option<i32>,
    /// Where the transcript was written; `None` until the job completes.
    pub transcript: Option<Transcript>,
    /// Whether the documents the locations this job reports serve have already been
    /// stored there ([`due_output_objects`]).
    pub outputs_stored: bool,
    /// `FailureReason`, set only on a `FAILED` job — which none of this emulator's
    /// are: nothing it does can fail a job it accepted.
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    /// When the job began processing; `None` while it is still `QUEUED`.
    pub start_time: Option<SystemTime>,
    pub completion_time: Option<SystemTime>,
    /// When `QUEUED` -> `IN_PROGRESS` falls due; `None` for a job that started
    /// processing immediately.
    pub queued_until: Option<SystemTime>,
    /// When `IN_PROGRESS` -> `COMPLETED` falls due.
    pub completion_due: SystemTime,
    pub tags: Vec<Tag>,
}

impl CallAnalyticsJobRecord {
    /// Materialise every status transition the CLOCK already owes this job — leaving
    /// the queue it was accepted into, and finishing — each of which was due at its
    /// stored instant whether or not anybody called. A reporting operation (`Get`,
    /// `List`) runs this before projecting the record, and that is the only write it
    /// performs.
    ///
    /// Completing a job records what the service determined while it processed the
    /// call — the language it identified, the format and sample rate of the audio it
    /// was given, and WHERE each transcript it produced is written (the operation that
    /// finds the job finished stores the document there, for the reason
    /// [`TranscriptionJobRecord::advance_clock`] gives). Every one of those
    /// values is a function of the stored record, so a job reports the same ones at
    /// every later read.
    pub fn advance_clock(&mut self, now: SystemTime) {
        if self.status == STATUS_QUEUED {
            match self.queued_until {
                Some(due) if now >= due => {
                    self.status = STATUS_IN_PROGRESS.to_string();
                    self.start_time = Some(due);
                }
                _ => return,
            }
        }
        if self.status != STATUS_IN_PROGRESS || now < self.completion_due {
            return;
        }
        self.status = STATUS_COMPLETED.to_string();
        self.completion_time = Some(self.completion_due);
        let settings = self.settings.as_ref();
        self.language_code = Some(identified_language(
            settings.and_then(|settings| settings.language_options.as_deref()),
            self.media_language.as_deref(),
        ));
        self.identified_language_score = Some(IDENTIFIED_LANGUAGE_SCORE);
        self.media_format = determined_media_format(self.media_format.take(), &self.media);
        self.media_sample_rate_hertz = Some(CALL_AUDIO_SAMPLE_RATE_HERTZ);
        let transcript_uri =
            call_analytics_output_uri(self.output_location.as_deref(), &self.name, &self.region);
        // Which transcripts the request asked the service to produce decides which
        // of the two locations the job reports ([`transcript_outputs`]).
        let outputs = transcript_outputs(
            settings.and_then(|settings| settings.content_redaction.as_ref()),
            self.media
                .redacted_media_file_uri
                .as_deref()
                .is_some_and(|uri| !uri.trim().is_empty()),
        );
        self.transcript = Some(Transcript {
            transcript_file_uri: outputs.unredacted.then(|| transcript_uri.clone()),
            redacted_transcript_file_uri: outputs
                .redacted
                .then(|| redacted_transcript_uri(&transcript_uri)),
        });
    }

    /// This record as the modelled `CallAnalyticsJob` shape, which is the whole answer
    /// of `StartCallAnalyticsJob` and `GetCallAnalyticsJob`.
    ///
    /// `Tags` is left ABSENT for a job carrying none rather than reported as an empty
    /// list, for the reason [`CallAnalyticsCategoryRecord::to_properties`] gives:
    /// `TagList` is modelled `length {min 1, max 200}`, so an empty list is not a
    /// value of that shape.
    pub fn to_job(&self) -> CallAnalyticsJob {
        CallAnalyticsJob {
            call_analytics_job_name: Some(self.name.clone()),
            call_analytics_job_status: Some(self.status.clone()),
            // Nothing here skips an analytics feature: the one feature the model
            // names a skip for is generative summarization, and both reason codes it
            // models turn on the CONTENT of the call, which nothing evaluates. A job
            // that skipped nothing reports no details at all, rather than an empty
            // list of skipped features.
            call_analytics_job_details: None,
            language_code: self.language_code.clone(),
            media_sample_rate_hertz: self.media_sample_rate_hertz,
            media_format: self.media_format.clone(),
            media: Some(self.media.clone()),
            transcript: self.transcript.clone(),
            start_time: self.start_time.map(to_date_time),
            creation_time: Some(to_date_time(self.creation_time)),
            completion_time: self.completion_time.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            data_access_role_arn: self.data_access_role_arn.clone(),
            identified_language_score: self.identified_language_score,
            settings: self.settings.clone(),
            channel_definitions: self.channel_definitions.clone(),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
        }
    }

    /// This record as the modelled `CallAnalyticsJobSummary` shape, which is what
    /// `ListCallAnalyticsJobs` reports for each result.
    ///
    /// `LanguageCode` is the same determination the job itself reports ("The language
    /// code used to create your Call Analytics transcription",
    /// `smithy_query(query="shape", shape="CallAnalyticsJobSummary")`), read off the
    /// same record rather than recomputed here.
    pub fn to_summary(&self) -> CallAnalyticsJobSummary {
        CallAnalyticsJobSummary {
            call_analytics_job_name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            start_time: self.start_time.map(to_date_time),
            completion_time: self.completion_time.map(to_date_time),
            language_code: self.language_code.clone(),
            call_analytics_job_status: Some(self.status.clone()),
            call_analytics_job_details: None,
            failure_reason: self.failure_reason.clone(),
        }
    }
}

/// Where a Call Analytics job's transcript is reported to live.
///
/// `StartCallAnalyticsJob` takes ONE `OutputLocation` URI rather than the
/// bucket-and-key pair a transcription job takes, and documents three forms —
/// `s3://BUCKET`, `s3://BUCKET/my-output-folder/` and
/// `s3://BUCKET/my-output-folder/my-call-analytics-job.json`: "Unless you specify a
/// file name (option 3), the name of your output file has a default value that
/// matches the name you specified for your transcription job using the
/// CallAnalyticsJobName parameter" (model doc for
/// `StartCallAnalyticsJobRequest$OutputLocation`). A request that names no output
/// location gets the service-managed location every job family reports
/// ([`output_uri`]).
///
/// Reported AND served: the document a finished job owes this location is stored at it
/// ([`store_job_outputs`]).
///
/// [`output_object_key`] is NOT reused: that rule names an object KEY relative to a
/// separate `OutputBucketName`, while this member is a whole URI whose bucket is part
/// of the value, so the two share the documented wording but not an input.
pub fn call_analytics_output_uri(
    output_location: Option<&str>,
    job_name: &str,
    region: &str,
) -> String {
    let Some(location) = output_location else {
        return output_uri(None, None, job_name, region, CALL_ANALYTICS_OUTPUT_EXTENSION);
    };
    match location.strip_suffix(&format!(".{CALL_ANALYTICS_OUTPUT_EXTENSION}")) {
        // The third documented form: a location naming a file is used as given.
        Some(_) => location.to_string(),
        None => format!(
            "{folder}/{job_name}.{CALL_ANALYTICS_OUTPUT_EXTENSION}",
            folder = location.trim_end_matches('/')
        ),
    }
}

/// The file extension of a Call Analytics job's transcript, as the `OutputLocation`
/// doc's own example file name (`my-call-analytics-job.json`) spells it.
pub const CALL_ANALYTICS_OUTPUT_EXTENSION: &str = "json";

/// The documented `OutputKey` rule, as a free function so `StartTranscriptionJob`
/// and the completion path agree on one implementation.
///
/// From the model doc for `StartTranscriptionJobRequest$OutputKey`: "The default
/// name for your transcription output is the same as the name you specified for your
/// transcription job", `OutputKey` `my-transcript.json` yields
/// `my-transcript.json`, and `OutputKey` `my-transcript` with job
/// `my-first-transcription` yields `my-transcript/my-first-transcription.json`.
pub fn output_object_key(output_key: Option<&str>, job_name: &str, extension: &str) -> String {
    match output_key {
        None => format!("{job_name}.{extension}"),
        // A key naming a file is used as given — with this output's own
        // extension, so the subtitle files sit beside the transcript.
        Some(key) => match key.strip_suffix(".json") {
            Some(stem) => format!("{stem}.{extension}"),
            None => format!("{folder}/{job_name}.{extension}", folder = key.trim_end_matches('/')),
        },
    }
}

/// The URI of one output file of a job, in the caller's bucket when the request
/// named one and in a service-managed bucket otherwise (model doc for
/// `Transcript$TranscriptFileUri`).
///
/// The service-managed branch names [`service_managed_bucket`], the one bucket this
/// service's own files live in — the same one a custom vocabulary's `DownloadUri`
/// names ([`vocabulary_download_uri`]).
pub fn output_uri(
    output_bucket_name: Option<&str>,
    output_key: Option<&str>,
    job_name: &str,
    region: &str,
    extension: &str,
) -> String {
    let key = output_object_key(output_key, job_name, extension);
    match output_bucket_name {
        Some(bucket) => format!("s3://{bucket}/{key}"),
        None => format!(
            "https://s3.{region}.amazonaws.com/{bucket}/{account}/{key}",
            bucket = service_managed_bucket(region),
            account = ::engine::emulator_account_id(),
        ),
    }
}

/// One object a finished job's reported output location names: the address it is
/// served from and the document stored there.
///
/// The location a reader REPORTS and the object a client FETCHES are built from one
/// value — the URI the record projects ([`output_object_address`]) — so the two cannot
/// drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputObject {
    /// The bucket the reported URI addresses.
    pub bucket: String,
    /// The key it addresses under that bucket.
    pub key: String,
    /// The document stored there.
    pub body: String,
    /// The `Content-Type` it is stored with.
    pub content_type: String,
}

/// The bucket and key a reported output URI addresses, in either of the two forms this
/// service reports one in: the `s3://BUCKET/KEY` of an output location the request
/// named ([`output_uri`], [`call_analytics_output_uri`],
/// [`medical_scribe_output_uri`]) and the path-style
/// `https://s3.REGION.amazonaws.com/BUCKET/KEY` of a service-managed one — both of
/// which the modelled `Uri` pattern `^(s3://|http(s*)://).+$` admits.
///
/// `None` for a value that addresses no object of this emulator's store, which there is
/// nothing to write at: one naming a bucket alone, and one of the `http(s)` form whose
/// authority is not the path-style Amazon S3 endpoint ([`is_path_style_s3_host`]). That
/// second case is why the two forms are not parsed alike: an output location is
/// caller-supplied, and a URI of that form naming any other authority addresses a HOST,
/// which this emulator resolves none of — reading its first path segment as a bucket
/// would write an object into a bucket the caller never named and this service never
/// reports.
pub(super) fn output_object_address(uri: &str) -> Option<(&str, &str)> {
    let uri = uri.trim();
    if let Some(address) = s3_object_address(uri) {
        return Some(address);
    }
    let (host, path) = uri.split_once("://")?.1.split_once('/')?;
    if !is_path_style_s3_host(host) {
        return None;
    }
    let (bucket, key) = path.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key))
}

/// Whether `host` is the path-style Amazon S3 endpoint this service reports a
/// service-managed location at: `s3.REGION.amazonaws.com` for a Region-shaped REGION
/// ([`is_region_like`]), which is the authority [`output_uri`] and
/// [`vocabulary_download_uri`] build such a URI with.
pub(super) fn is_path_style_s3_host(host: &str) -> bool {
    host.strip_prefix("s3.")
        .and_then(|rest| rest.strip_suffix(".amazonaws.com"))
        .is_some_and(is_region_like)
}

/// The `Content-Type` a JSON output document is stored with.
pub(super) const OUTPUT_JSON_CONTENT_TYPE: &str = "application/json";

/// `SubtitleFormat` values, from the model's enum
/// (`smithy_query(query="shape", shape="SubtitleFormat")`).
pub const SUBTITLE_FORMAT_SUBRIP: &str = SUBTITLE_FORMAT_VALUES[0];
/// See [`SUBTITLE_FORMAT_SUBRIP`].
pub const SUBTITLE_FORMAT_WEBVTT: &str = SUBTITLE_FORMAT_VALUES[1];

/// The first line of every WebVTT file, which is what identifies the format.
pub(super) const WEBVTT_SIGNATURE: &str = "WEBVTT";

/// The `Content-Type` a WebVTT subtitle file is stored with: the media type registered
/// for that format.
pub(super) const SUBTITLE_CONTENT_TYPE_WEBVTT: &str = "text/vtt";

/// The `Content-Type` a SubRip subtitle file is stored with. CHOSEN: SubRip has no
/// registered media type of its own and the file is plain text.
pub(super) const SUBTITLE_CONTENT_TYPE_SUBRIP: &str = "text/plain";

/// The transcript document a finished job serves at the location it reports.
///
/// There IS a document to fetch there: `Transcript$TranscriptFileUri` is "The Amazon S3
/// location of your transcript. You can use this URI to access or download your
/// transcript" (`smithy_query(query="doc", shape="Transcript",
/// member="TranscriptFileUri")`), and `MedicalTranscript$TranscriptFileUri` says the
/// same of its own. It is JSON, as every output key this service names ends in `.json`.
///
/// Its members are the facts the service itself owns: the job this is the output of,
/// the account that ran it and the status it finished in. The transcribed text is EMPTY
/// and the item list holds nothing, because no engine read the audio (see the module
/// header) — the document is the true envelope of a transcription that produced no
/// words, not invented speech.
pub fn transcript_document(job_name: &str, status: &str) -> String {
    ::serde_json::json!({
        "jobName": job_name,
        "accountId": ::engine::emulator_account_id(),
        "status": status,
        "results": {"transcripts": [{"transcript": ""}], "items": []},
    })
    .to_string()
}

/// The clinical note a finished Medical Scribe job serves at the location it reports
/// ([`MEDICAL_SCRIBE_CLINICAL_DOCUMENT_FILE`]).
///
/// `MedicalScribeOutput$ClinicalDocumentUri` "Holds the Amazon S3 URI for the Clinical
/// Document" (`smithy_query(query="shape", shape="MedicalScribeOutput")`), so a
/// document is fetchable there. It carries NO section: a clinical note is written from
/// the conversation, and nothing here transcribes one, so what a client reads is a note
/// with no content rather than an invented summary of a patient.
pub fn clinical_document() -> String {
    ::serde_json::json!({"ClinicalDocumentation": {"Sections": []}}).to_string()
}

/// The subtitle file a finished job serves for one requested format, with the
/// `Content-Type` it is stored with.
///
/// `SubtitleFormat` is `vtt | srt` ([`SUBTITLE_FORMAT_VALUES`]). A WebVTT file is
/// identified by its own first line, so the one served carries that line and no cue; a
/// SubRip file is a sequence of cues and nothing else, so one with no cue is empty.
/// Either way it holds no caption, for the reason [`transcript_document`] gives.
pub fn subtitle_document(format: &str) -> (String, &'static str) {
    if format.eq_ignore_ascii_case(SUBTITLE_FORMAT_SUBRIP) {
        return (String::new(), SUBTITLE_CONTENT_TYPE_SUBRIP);
    }
    // The other of the two modelled formats, [`SUBTITLE_FORMAT_WEBVTT`].
    (
        format!("{WEBVTT_SIGNATURE}\n\n"),
        SUBTITLE_CONTENT_TYPE_WEBVTT,
    )
}

/// One output object at the address `uri` names, or `None` when that URI addresses no
/// object an object store could hold ([`output_object_address`]).
pub(super) fn output_object(uri: &str, body: String, content_type: &str) -> Option<OutputObject> {
    let (bucket, key) = output_object_address(uri)?;
    Some(OutputObject {
        bucket: bucket.to_string(),
        key: key.to_string(),
        body,
        content_type: content_type.to_string(),
    })
}

/// The object a reported transcript location serves: the transcript document of the job
/// named `job_name` at the address `uri` names.
pub(super) fn transcript_object(uri: &str, job_name: &str, status: &str) -> Option<OutputObject> {
    output_object(
        uri,
        transcript_document(job_name, status),
        OUTPUT_JSON_CONTENT_TYPE,
    )
}

/// What a job family's record owes the output locations it REPORTS: the documents a
/// client that follows one of those URIs fetches.
///
/// One trait rather than one spelling per family, so "a finished job's reported
/// location serves the document it names" is written once over any family's record
/// ([`due_output_objects`], [`store_job_outputs`]): the job families differ in WHICH
/// locations a finished record reports, not in how the documents reach the object
/// store. A family added later reaches those helpers only by implementing this.
pub trait JobOutputs {
    /// The objects this record's reported output locations serve — empty for a job the
    /// clock has not finished yet, and for one that finished with no output to report
    /// (a `FAILED` job produced none).
    fn output_objects(&self) -> Vec<OutputObject>;

    /// Whether those objects have already been stored.
    fn outputs_stored_mut(&mut self) -> &mut bool;
}

/// The objects `record` still owes the locations it reports, taken ONCE.
///
/// The write a finished job owes is materialised exactly like the status transition
/// that finished it: it fell due when the job completed, whether or not anybody called,
/// so the first operation that finds the job finished takes the documents and stores
/// them ([`store_job_outputs`]) — and no later call takes them again, so a reader
/// reports the same locations without touching what is stored there and what a client
/// fetched once stays what it fetched.
pub fn due_output_objects<R: JobOutputs>(record: &mut R) -> Vec<OutputObject> {
    if *record.outputs_stored_mut() {
        return Vec::new();
    }
    let mut objects = record.output_objects();
    // ONE write per location, however many times the record reports it. A request's
    // subtitle formats are a list the model gives no length (`SubtitleFormats`), so a
    // request may name one format over and over — and each naming is the same file:
    // "if you select both WebVTT (vtt) and SubRip (srt) formats, two output files are
    // generated" (model doc for `Subtitles$Formats`), so there are as many files as
    // there are distinct formats. Deduplicating here is what keeps the work one read
    // does bounded by how many distinct locations a job HAS rather than by how long a
    // list the request sent.
    let mut seen = ::std::collections::BTreeSet::new();
    objects.retain(|object| seen.insert((object.bucket.clone(), object.key.clone())));
    if !objects.is_empty() {
        *record.outputs_stored_mut() = true;
    }
    objects
}

/// Store the documents a finished job's reported output locations serve.
///
/// The addresses come from the record's own projection, so a client that follows the
/// URI a reader reported fetches the document stored here — which is what lets the
/// canonical workflow (start a job, poll it until it is finished, then fetch its
/// output) complete. The object store is this emulator's own Amazon S3, reached the way
/// any emulated service reaches another: over the in-process callback ([`s3_client`] —
/// no socket, no outbound request, no host resolution).
///
/// The service-managed bucket is this service's own, so a write into it stands it up
/// first ([`create_service_managed_bucket`]); a bucket the REQUEST named belongs to the
/// caller, who created it before starting the job, so nothing is created there.
///
/// Nothing is ENCRYPTED: `OutputEncryptionKMSKeyId` is recorded as asked for and the
/// document is stored as written, because this emulator holds no key to encrypt it
/// with.
///
/// Object storage answering nothing says nothing about this service's record — the
/// control plane is real whether or not the emulator was assembled with Amazon S3 — so
/// a failed write is logged and the operation still answers; it never turns a
/// successful read into a refusal. It is REPORTED, though: the answer is whether every
/// document was stored, and a caller that hears otherwise gives the documents back to
/// the record ([`return_due_output_objects`]), because a write the object store did not
/// take is one the job still owes.
pub async fn store_job_outputs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    objects: &[OutputObject],
) -> bool {
    if objects.is_empty() {
        return true;
    }
    let client = s3_client(ctx, region);
    let service_managed = service_managed_bucket(region);
    let mut stood_up = false;
    let mut stored_every_one = true;
    for object in objects {
        if object.bucket == service_managed && !stood_up {
            create_service_managed_bucket(&client, &service_managed, region).await;
            stood_up = true;
        }
        match client
            .put_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(
                object.body.clone().into_bytes(),
            ))
            .content_type(object.content_type.as_str())
            .send()
            .await
        {
            Ok(_) => ::tracing::info!(
                bucket = ?object.bucket,
                key = ?object.key,
                "the document this job's reported output location names was stored"
            ),
            Err(error) => {
                stored_every_one = false;
                ::tracing::warn!(
                    bucket = ?object.bucket,
                    key = ?object.key,
                    detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the document this job's reported output location names was not stored"
                );
            }
        }
    }
    stored_every_one
}

/// The objects a job's reported output locations name in THIS SERVICE'S OWN bucket
/// ([`service_managed_bucket`]), out of every location `record` reports
/// ([`JobOutputs::output_objects`]).
///
/// The service-managed branch of a reported location is the only one this service owns:
/// a bucket the REQUEST named belongs to the caller, who created it before starting the
/// job ([`store_job_outputs`]), so nothing of the caller's is ever named here. The
/// addresses are the record's own projection — the very one the write addresses — so a
/// caller can never be told about one location and have another touched.
///
/// Taken while the record is still held, since it is the record that knows where its
/// files are; what is DONE with them is awaited outside the store lock
/// ([`withdraw_job_outputs`]).
pub fn service_managed_outputs<R: JobOutputs>(record: &R, region: &str) -> Vec<OutputObject> {
    let service_managed = service_managed_bucket(region);
    let mut objects = record.output_objects();
    objects.retain(|object| object.bucket == service_managed);
    // ONE withdrawal per address, however many times the record reports it, for the
    // reason [`due_output_objects`] deduplicates the writes.
    let mut seen = ::std::collections::BTreeSet::new();
    objects.retain(|object| seen.insert(object.key.clone()));
    objects
}

/// Withdraw the documents this service's own bucket holds for a job whose record has
/// just been removed.
///
/// The other direction of [`store_job_outputs`], and the same rule the vocabulary
/// family's delete closes with ([`store_vocabulary_content`] called with no terms): the
/// record is gone, so this service reports those locations to nobody, and the files it
/// wrote into its OWN bucket for that record are withdrawn with it rather than left
/// behind for a job nothing holds. Only the service-managed objects are passed in
/// ([`service_managed_outputs`]) — an object in a bucket the request named is the
/// caller's, who created that bucket and keeps whatever is in it.
///
/// Object storage answering nothing says nothing about this service's record — the
/// control plane is real whether or not the emulator was assembled with Amazon S3 — so a
/// withdrawal that is not taken is logged and the delete still answers. An answer saying
/// the bucket or the object is not there already means nothing is served there, which is
/// why that is not a warning.
pub async fn withdraw_job_outputs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    objects: &[OutputObject],
) {
    if objects.is_empty() {
        return;
    }
    let client = s3_client(ctx, region);
    for object in objects {
        match client
            .delete_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .send()
            .await
        {
            Ok(_) => ::tracing::info!(
                bucket = ?object.bucket,
                key = ?object.key,
                "the document this job's reported output location named was withdrawn"
            ),
            Err(error) => ::tracing::debug!(
                bucket = ?object.bucket,
                key = ?object.key,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "no document was withdrawn for this job; nothing is served there"
            ),
        }
    }
}

/// Give the documents a write did NOT store back to `record`, so the next operation
/// that reads the job takes them again ([`due_output_objects`]).
///
/// What the job owes its reported locations does not stop being owed because one
/// attempt to store it failed — the bucket the request named may not have existed yet,
/// or the emulator may have been assembled with no object store at that moment — so the
/// write stays due rather than being consumed by the attempt. `None` is the job having
/// been deleted while the write was in flight, which owes nothing.
pub fn return_due_output_objects<R: JobOutputs>(record: Option<&mut R>) {
    if let Some(record) = record {
        *record.outputs_stored_mut() = false;
    }
}

// The four job families' output sets, together so what each one owes its reported
// locations can be read beside the others. Each is a projection of the SAME members the
// family's reader reports, so no location is written that a client was not told about,
// and none is reported that holds nothing.

impl JobOutputs for TranscriptionJobRecord {
    /// The transcript this job produced, at each location it reports one at
    /// ([`transcript_outputs`]), plus one subtitle file per requested format.
    fn output_objects(&self) -> Vec<OutputObject> {
        let Some(transcript) = self.transcript.as_ref() else {
            return Vec::new();
        };
        let mut objects: Vec<OutputObject> = [
            transcript.transcript_file_uri.as_deref(),
            transcript.redacted_transcript_file_uri.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|uri| transcript_object(uri, &self.name, &self.status))
        .collect();
        // "Your subtitle file is stored in the same location as your transcript. If you
        // specified both WebVTT and SubRip subtitle formats, two URIs are provided"
        // (model doc for `SubtitlesOutput$SubtitleFileUris`): one file per requested
        // format, each at the location reported for that format — the two lists are
        // built in one order ([`Self::advance_clock`]), so they pair up.
        if let Some(subtitles) = self.subtitles.as_ref() {
            let formats = subtitles.formats.iter().flatten();
            let uris = subtitles.subtitle_file_uris.iter().flatten();
            objects.extend(formats.zip(uris).filter_map(|(format, uri)| {
                let (body, content_type) = subtitle_document(format);
                output_object(uri, body, content_type)
            }));
        }
        objects
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
    }
}

impl JobOutputs for MedicalTranscriptionJobRecord {
    /// The one transcript this family reports, in the bucket the start named.
    fn output_objects(&self) -> Vec<OutputObject> {
        self.transcript
            .as_ref()
            .and_then(|transcript| transcript.transcript_file_uri.as_deref())
            .and_then(|uri| transcript_object(uri, &self.name, &self.status))
            .into_iter()
            .collect()
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
    }
}

impl JobOutputs for CallAnalyticsJobRecord {
    /// The transcript of the call, at each location this job reports one at
    /// ([`transcript_outputs`]). No analysis of the call is in it, for the reason
    /// [`transcript_document`] gives: nothing listened to it.
    fn output_objects(&self) -> Vec<OutputObject> {
        let Some(transcript) = self.transcript.as_ref() else {
            return Vec::new();
        };
        [
            transcript.transcript_file_uri.as_deref(),
            transcript.redacted_transcript_file_uri.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|uri| transcript_object(uri, &self.name, &self.status))
        .collect()
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
    }
}

impl JobOutputs for MedicalScribeJobRecord {
    /// Both files this family reports: the transcript and the clinical note.
    fn output_objects(&self) -> Vec<OutputObject> {
        let Some(output) = self.output.as_ref() else {
            return Vec::new();
        };
        [
            transcript_object(&output.transcript_file_uri, &self.name, &self.status),
            output_object(
                &output.clinical_document_uri,
                clinical_document(),
                OUTPUT_JSON_CONTENT_TYPE,
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn outputs_stored_mut(&mut self) -> &mut bool {
        &mut self.outputs_stored
    }
}

/// A `SystemTime` as the `DateTime` the modelled timestamp members carry.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTranscribeTranscribeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTranscribeTranscribeState>)
}

/// The region the request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope or one that does not name a region.
///
/// The scope is caller-supplied and the region reaches the host part of a
/// service-managed transcript URI, so a value that is not shaped like a Region
/// identifier is not reflected: the real service would not have served the request
/// under a region it does not have.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like an AWS Region identifier: two or more
/// hyphen-separated lowercase-letter parts followed by a digit group
/// (`us-east-1`, `ap-southeast-3`, `us-gov-west-1`, `cn-north-1`).
pub(super) fn is_region_like(region: &str) -> bool {
    let Some((parts, index)) = region.rsplit_once('-') else {
        return false;
    };
    region.len() <= 32
        && !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && parts.split('-').count() >= 2
        && parts
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase()))
}
