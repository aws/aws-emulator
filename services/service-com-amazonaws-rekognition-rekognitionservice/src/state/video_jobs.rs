// SPDX-License-Identifier: CC0-1.0

//! The stored-video analysis JOBS this service has started: what a `Start…` operation
//! records, and how the clock moves a job it recorded to its terminal status.
//!
//! Every stored-video operation of this API works the same way — a `Start…` call "starts
//! asynchronous detection … in a stored video" and "returns a job identifier (`JobId`) that
//! you use to get the results of the operation", and the matching `Get…` call reports the
//! job under that identifier. The job is therefore a RESOURCE of this service, with one
//! record per start, and the rule is one rule for the whole family, so it lives here once
//! rather than once per operation: a job knows which analysis started it, the video it runs
//! on, the request it was started with and when it was started, and its status is a function
//! of the clock and nothing else.
//!
//! WHAT THIS EMULATOR DOES NOT COMPUTE: no video decoder runs in this process, so nothing
//! here measures a property of the media or finds anything IN it. A job records what the
//! request asked for and when, which is exactly what a start operation answers for; the
//! RESULTS of an analysis belong to the `Get…` operation that reports them, and the
//! operations of this family that report results are unimplemented 501s until one is written
//! (`GetFaceDetection`, …).
//!
//! The store lives in this process's memory, so it is BOUNDED twice: by how many jobs may be
//! running at one instant ([`MAX_CONCURRENT_JOBS`], the condition the modelled
//! `LimitExceededException` describes) and by how many it remembers at all
//! ([`MAX_REMEMBERED_JOBS`]).

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{NotificationChannel, S3Object};

use super::video_source::VideoLocation;

/// How long after the call that started it a video analysis job is finished.
///
/// A start operation answers while the analysis is still running — that is what makes the
/// job's status worth reporting — so the span is never zero: a caller that reads the job
/// straight after starting it sees it IN PROGRESS. AWS takes as long as the video is long;
/// this emulator owes the transition at a span of its own choosing, and it is this one for
/// every analysis of the family.
pub const COMPLETION_SPAN: Duration = Duration::from_secs(3);

/// How many jobs of this service may be running at one instant.
///
/// `LimitExceededException` is modelled for exactly this: "if you start too many jobs
/// concurrently, subsequent calls to start operations … will raise a LimitExceededException
/// exception … until the number of concurrently running jobs is below the Amazon Rekognition
/// service limit"
/// (`smithy_query(query="doc",shape="LimitExceededException")`). The published default for
/// concurrently running stored-video analysis jobs is 20 per Region.
pub const MAX_CONCURRENT_JOBS: usize = 20;

/// How many jobs the store remembers before the oldest is forgotten.
///
/// The store is this process's memory, so a caller that keeps starting jobs is bounded
/// rather than allowed to grow the process without limit. A forgotten job's identifier
/// names nothing, which is the same answer as an identifier this service never minted.
pub const MAX_REMEMBERED_JOBS: usize = 1_024;

/// Where a job is in its life: the two `VideoJobStatus` values this emulator's jobs take
/// (`smithy_query(query="shape",shape="VideoJobStatus")` models a third, `FAILED`, for an
/// analysis that broke — nothing here breaks one, so no job is ever reported in it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// The analysis is still running: the clock has not reached the job's completion yet.
    InProgress,
    /// The analysis has finished.
    Succeeded,
}

impl JobStatus {
    /// This status as the modelled `VideoJobStatus` value.
    pub fn modeled(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
        }
    }
}

/// One stored-video analysis job, as the `Start…` call that minted it recorded it.
#[derive(Debug, Clone)]
pub struct VideoJob {
    /// The `JobId` this service minted for the job; the store's key for it.
    pub job_id: String,
    /// The operation that started the job (`StartFaceDetection`, …), so a `Get…` call
    /// reports only the jobs of its own analysis and a reused idempotency token is one
    /// token OF ONE OPERATION.
    pub analysis: String,
    /// The video the job runs on, as the request named it.
    pub video: VideoLocation,
    /// `ClientRequestToken` — the token the start committed, when it carried one, so the
    /// same token sent again answers with this same job rather than starting a second one.
    pub client_request_token: Option<String>,
    /// `JobTag` — "an identifier you specify that's returned in the completion
    /// notification", carried through unchanged.
    pub job_tag: Option<String>,
    /// `NotificationChannel` — where the completion status of this job is published.
    pub notification_channel: Option<NotificationChannel>,
    /// The members that configure the analysis itself, by their modelled member names, for
    /// whichever `Start…` operation minted the job (`FaceAttributes` for a face detection,
    /// …). Kept as the request stated them, so the operation that reports the job's results
    /// answers for what was asked for.
    pub parameters: Vec<(String, String)>,
    /// When the start call recorded the job.
    pub started_at: SystemTime,
    /// When the analysis is finished — [`COMPLETION_SPAN`] after [`Self::started_at`]. The
    /// instant is STORED rather than waited for: nothing sleeps, and the status below is
    /// computed from the clock whenever it is asked for.
    pub completes_at: SystemTime,
}

impl VideoJob {
    /// The job `analysis` starts at `started_at` on `video`, due to finish one
    /// [`COMPLETION_SPAN`] later.
    pub fn new(
        job_id: String,
        analysis: &str,
        video: VideoLocation,
        started_at: SystemTime,
    ) -> Self {
        Self {
            job_id,
            analysis: analysis.to_string(),
            video,
            client_request_token: None,
            job_tag: None,
            notification_channel: None,
            parameters: Vec::new(),
            started_at,
            completes_at: started_at + COMPLETION_SPAN,
        }
    }

    /// The job's status at `now`: still running until the instant its analysis is due, and
    /// finished from that instant on.
    pub fn status_at(&self, now: SystemTime) -> JobStatus {
        if now < self.completes_at {
            JobStatus::InProgress
        } else {
            JobStatus::Succeeded
        }
    }
}

/// Every video analysis job this service remembers, keyed by its `JobId`.
///
/// A `BTreeMap` so iteration order is deterministic, with the start order kept beside it so
/// the OLDEST job is the one forgotten when the store is full.
#[derive(Debug, Default)]
pub struct VideoJobs {
    jobs: BTreeMap<String, VideoJob>,
    order: VecDeque<String>,
}

impl VideoJobs {
    /// The job `job_id` names, or `None` when this store holds none.
    pub fn get(&self, job_id: &str) -> Option<&VideoJob> {
        self.jobs.get(job_id)
    }

    /// The job `analysis` started under `token`, when it still holds one: what makes a
    /// retried start answer with the job identifier it already handed out.
    pub fn started_under_token(&self, analysis: &str, token: &str) -> Option<&VideoJob> {
        self.jobs.values().find(|job| {
            job.analysis == analysis && job.client_request_token.as_deref() == Some(token)
        })
    }

    /// How many jobs are running at `now` — the count the concurrency limit is about.
    pub fn running_at(&self, now: SystemTime) -> usize {
        self.jobs
            .values()
            .filter(|job| job.status_at(now) == JobStatus::InProgress)
            .count()
    }

    /// Whether a job started at `now` would be one job too many; see
    /// [`MAX_CONCURRENT_JOBS`].
    pub fn at_concurrency_limit(&self, now: SystemTime) -> bool {
        self.running_at(now) >= MAX_CONCURRENT_JOBS
    }

    /// Record `job`, forgetting the oldest remembered job once the store is full.
    pub fn record(&mut self, job: VideoJob) {
        while self.order.len() >= MAX_REMEMBERED_JOBS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.jobs.remove(&oldest);
                }
                None => break,
            }
        }
        let job_id = job.job_id.clone();
        if self.jobs.insert(job_id.clone(), job).is_none() {
            self.order.push_back(job_id);
        }
    }
}

/// The request members a `ClientRequestToken` commits for a stored-video analysis, by their
/// modelled member names: the video the job runs on, the channel its completion is published
/// to, the tag it carries and the analysis's own parameters.
///
/// `IdempotentParameterMismatchException` is modelled for "a `ClientRequestToken` input
/// parameter … reused with an operation, but at least one of the other input parameters …
/// different from the previous call", so "the other input parameters" is every input member
/// beside the token itself — one list for the whole family, so two `Start…` operations
/// cannot disagree on what a token committed. A member the request left out is compared as
/// absent rather than ignored, which is what makes adding one a different request.
pub fn committed_parameters(
    video: Option<&S3Object>,
    notification_channel: Option<&NotificationChannel>,
    job_tag: Option<&str>,
    parameters: &[(&str, &str)],
) -> Vec<(String, String)> {
    let mut committed = vec![
        member("Bucket", video.and_then(|object| object.bucket.as_deref())),
        member("Name", video.and_then(|object| object.name.as_deref())),
        member("Version", video.and_then(|object| object.version.as_deref())),
        member(
            "SNSTopicArn",
            notification_channel.map(|channel| channel.sns_topic_arn.as_str()),
        ),
        member(
            "RoleArn",
            notification_channel.map(|channel| channel.role_arn.as_str()),
        ),
        member("JobTag", job_tag),
    ];
    committed.extend(
        parameters
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    committed
}

/// One compared member: its modelled name and the value the request carried, with an absent
/// value compared as the empty string.
fn member(name: &str, value: Option<&str>) -> (String, String) {
    (name.to_string(), value.unwrap_or_default().to_string())
}
