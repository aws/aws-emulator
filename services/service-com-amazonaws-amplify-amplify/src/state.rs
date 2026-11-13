// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplify-amplify service.
//!
//! The persisted record for a resource IS the modelled structure this service hands
//! back for it: [`App`] for an app, [`Branch`] for a branch, [`Webhook`] for a
//! webhook, [`Job`] for a job. Keeping one copy of each resource — rather than a
//! parallel struct with the same ~30 members plus a conversion — is what makes
//! `GetApp` after `CreateApp` return the created record field-for-field: a reader
//! clones what the writer stored, so the two cannot drift.
//!
//! Branches, webhooks, domain associations and backend environments live INSIDE their
//! owning app, so `DeleteApp` removes them transitively and `ListBranches` /
//! `ListWebhooks` / `ListDomainAssociations` / `ListBackendEnvironments` can never
//! reach another app's records. Jobs live INSIDE their owning branch for the same reason: `DeleteBranch`
//! takes that branch's jobs with it, and `GetJob` can never reach a job of another
//! branch. A deployment reserved on a branch and not started yet lives there too, so
//! it shares its branch's lifetime and its `jobId` cannot collide with another
//! branch's.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::ops::types::{
    App, BackendEnvironment, Branch, CustomRule, DomainAssociation, Job, JobSummary,
    JobSummaryProjection, Step, Webhook,
};

/// The access control an app configures on its branches: what a configured
/// basic-authorization credential is kept and reported as, and how the hosting edge
/// verifies the one a visitor presents.
pub mod basic_auth;
/// The custom HTTP headers an app configures: the document a caller submits, the rules it
/// declares, and the headers those rules add to a response the hosting edge serves.
pub mod custom_headers;
/// The address an app's hosting is announced at, and the host one branch's site answers
/// on.
pub mod hosting_address;

pub use basic_auth::{
    ingest_auto_branch_basic_auth, ingest_basic_auth, presented_credentials, BasicAuthCredentials,
};
pub use custom_headers::{
    custom_header_rules, custom_header_rules_limit_message, custom_headers_document,
    ingest_custom_headers, matched_headers, CustomHeaderRule, IngestedCustomHeaders,
    MAX_CUSTOM_HEADER_RULES_PER_APP,
};
pub use hosting_address::{amplify_zone_default_domain, hosted_branch_host, hosted_site_host};

/// One Amplify app as this service persists it: the modelled record it hands back,
/// plus the branches, domain associations, webhooks and backend environments created
/// on it.
///
/// The maps are `BTreeMap`s, not `HashMap`s, because the list operations are
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `nextToken` a page returns is the key of the first record it did NOT return, so
/// the next call resumes at that key.
#[derive(Debug, Clone)]
pub struct AmplifyApp {
    /// The app record, exactly as every reader returns it.
    pub app: App,
    /// The branches of this app, keyed by `branchName`.
    pub branches: BTreeMap<String, AmplifyBranch>,
    /// The domain associations of this app, keyed by `domainName`.
    ///
    /// A domain association is created under an app (`POST /apps/{appId}/domains`)
    /// and addressed afterwards by that app and its domain name
    /// (`GET /apps/{appId}/domains/{domainName}`), so it is held by the app rather
    /// than in a store-wide map: the same custom domain name associated with two
    /// apps is two records, and neither app's reader can reach the other's.
    pub domains: BTreeMap<String, AmplifyDomain>,
    /// The webhooks of this app, keyed by `webhookId`.
    ///
    /// A webhook is created under an app (`POST /apps/{appId}/webhooks`) and is
    /// addressed afterwards by its own id alone (`GET /webhooks/{webhookId}`), which
    /// [`ComAmazonawsAmplifyAmplifyState::webhook`] resolves across the apps.
    pub webhooks: BTreeMap<String, Webhook>,
    /// The backend environments of this app, keyed by `environmentName`.
    ///
    /// A backend environment is created under an app
    /// (`POST /apps/{appId}/backendenvironments`) and addressed afterwards by that app
    /// and its environment name
    /// (`GET /apps/{appId}/backendenvironments/{environmentName}`), so it is held by
    /// the app rather than in a store-wide map: the same environment name created on
    /// two apps is two records, and neither app's reader can reach the other's.
    pub backend_environments: BTreeMap<String, BackendEnvironment>,
    /// The basic-authorization credential configured for this app's branches
    /// (`App$basicAuthCredentials`), as the only thing this service keeps of one: a
    /// verifier, never the credential itself (see [`basic_auth`]). `App$basicAuthCredentials`
    /// itself reports the reference every reader is answered with.
    pub basic_auth: Option<BasicAuthCredentials>,
    /// The custom-header rules this app's `customHeaders` document declares, parsed at
    /// the write that supplied the document (see [`custom_headers`]).
    ///
    /// Held beside the record because the record reports the DOCUMENT while the hosting
    /// edge resolves a request against the RULES: parsing once at the write keeps the two
    /// in step and keeps a visit from re-reading a document per request. SHARED, so a
    /// visit can take the list out of the store's critical section without copying a rule
    /// and match it against the request path with the lock released.
    pub custom_header_rules: Arc<[CustomHeaderRule]>,
}

impl AmplifyApp {
    /// A newly created app with no branches, webhooks, domains or backend environments.
    pub fn new(app: App) -> Self {
        Self::created(app, None, Vec::new())
    }

    /// A newly created app carrying what its create INGESTED: the verifier for the
    /// basic-authorization credentials the request supplied, and the rules its
    /// custom-header document declared.
    ///
    /// The record and the two parsed halves are set in ONE place, so an app cannot end up
    /// reporting a credential or a header rule it does not hold.
    pub fn created(
        app: App,
        basic_auth: Option<BasicAuthCredentials>,
        custom_header_rules: Vec<CustomHeaderRule>,
    ) -> Self {
        Self {
            app,
            branches: BTreeMap::new(),
            domains: BTreeMap::new(),
            webhooks: BTreeMap::new(),
            backend_environments: BTreeMap::new(),
            basic_auth,
            custom_header_rules: custom_header_rules.into(),
        }
    }
}

/// One branch as this service persists it: the modelled record every reader returns,
/// plus the jobs started on it, keyed by the `jobId` this branch minted for each.
#[derive(Debug, Clone)]
pub struct AmplifyBranch {
    /// The branch record, exactly as every reader returns it.
    pub branch: Branch,
    /// The deployments `CreateDeployment` has reserved on this branch and nobody has
    /// started yet, keyed by the `jobId` reserved for each.
    ///
    /// A deployment is reserved by one call and started by another, so what the first
    /// call minted — the job id and the upload destinations — has to outlive it; this
    /// is where it lives until `StartDeployment` names that job id. One entry per
    /// deployment this branch actually handed destinations to: a `CreateDeployment` the
    /// serialization rule refuses hands out none and so stores none, and each of the
    /// three ways a reservation ends spends it — `StartDeployment` takes it, `StopJob`
    /// cancels it, and the closing of its window drops it
    /// ([`AmplifyBranch::expire_deployments`]).
    pub deployments: BTreeMap<String, PendingDeployment>,
    /// The jobs of this branch, keyed by `jobId`: one per call that started a job on it,
    /// and one per call that spent a job number reserving a deployment.
    ///
    /// BOUNDED, with an explicit retention policy: the map holds at most
    /// [`MAX_JOBS_PER_BRANCH`] jobs, and starting one more RETIRES the oldest job
    /// that has reached a terminal status (see
    /// [`AmplifyBranch::make_room_for_a_job`]). A branch's job history is therefore
    /// finite however many jobs are started on it, and a job that was retired is
    /// `NotFoundException` to every reader. `Branch$totalNumberOfJobs` counts the
    /// jobs the branch has STARTED ([`AmplifyBranch::jobs_started`]), not the
    /// retained ones, so retiring history does not rewrite that count.
    pub jobs: BTreeMap<String, BranchJob>,
    /// Monotonic counter feeding every `jobId` this branch hands out —
    /// [`AmplifyBranch::mint_job_id`] for a job it is starting and
    /// [`AmplifyBranch::reserve_job_id`] for a deployment it is reserving.
    ///
    /// ONE sequence for both, so a reserved id is never handed to a later job: the
    /// deployment that reserved it is started by its own call, under that id.
    pub next_job_seq: u64,
    /// How many jobs this branch has STARTED, which is what
    /// [`AmplifyBranch::jobs_started`] reports.
    ///
    /// Not the same as [`Self::next_job_seq`]: reserving a deployment takes an id out
    /// of that sequence without starting anything, and a job exists only once a
    /// start call creates it.
    pub started_jobs: u64,
    /// The files this branch's hosting edge serves now, keyed by the path each is
    /// served at under the site root — what the branch's LATEST deployment to have
    /// succeeded published, and empty for a branch nothing has deployed to yet.
    ///
    /// Held on the BRANCH, not on the job that deployed it, because this is the live
    /// site: one branch serves one set of files whatever its job history holds, and a
    /// later successful deployment REPLACES it (see [`Self::stage_site`]).
    pub site: BTreeMap<String, Vec<u8>>,
    /// The deployment that has been started and whose job the clock has not settled
    /// yet, with the files it publishes when it succeeds.
    ///
    /// A deployment's content goes live when its JOB reaches its terminal status, not
    /// when the call that started it answers, so the files wait here until the clock
    /// has settled that job — which [`Self::settle`] materialises.
    pub staged_site: Option<StagedSite>,
    /// The basic-authorization credential configured on this branch
    /// (`Branch$basicAuthCredentials`), as the only thing this service keeps of one: a
    /// verifier, never the credential itself (see [`basic_auth`]).
    ///
    /// This is what the hosting edge checks an `Authorization: Basic` header against;
    /// `Branch$basicAuthCredentials` reports the reference every reader is answered with.
    pub basic_auth: Option<BasicAuthCredentials>,
}

/// The files one started deployment publishes, and the job whose settling publishes
/// them.
#[derive(Debug, Clone)]
pub struct StagedSite {
    /// The `jobId` of the deployment that will publish these files.
    pub job_id: String,
    /// The files it publishes, keyed by the path each is served at.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl AmplifyBranch {
    /// A newly created branch, which has never been built and so holds no jobs.
    pub fn new(branch: Branch) -> Self {
        Self::created(branch, None)
    }

    /// A newly created branch carrying the access control its create configured: the
    /// verifier for the basic-authorization credentials the request supplied, if any.
    ///
    /// The record and the verifier are set in ONE place, so a branch cannot end up
    /// reporting a credential it does not hold (or holding one it does not report).
    pub fn created(branch: Branch, basic_auth: Option<BasicAuthCredentials>) -> Self {
        Self {
            branch,
            deployments: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_job_seq: 0,
            started_jobs: 0,
            site: BTreeMap::new(),
            staged_site: None,
            basic_auth,
        }
    }

    /// Record the files the deployment running under `job_id` publishes WHEN that job
    /// succeeds.
    ///
    /// Only one deployment of a branch can be waiting to go live: a deployment started
    /// while another is still settling replaces it, exactly as it replaces it as the
    /// branch's active job.
    pub fn stage_site(&mut self, job_id: &str, files: BTreeMap<String, Vec<u8>>) {
        self.staged_site = Some(StagedSite {
            job_id: job_id.to_string(),
            files,
        });
    }

    /// Materialise everything the clock has ALREADY made due on this branch: every
    /// job's own status change, and the site a started deployment publishes once its
    /// job has settled.
    ///
    /// Both are owed at an instant that passes whether or not anybody calls, so any
    /// operation that reports on this branch applies them before answering. Before that
    /// instant this is a no-op, so a caller that reads straight after a start call still
    /// observes the non-terminal job AND the site as it was.
    ///
    /// A staged deployment whose job FAILED publishes nothing: a deployment that could
    /// not be carried out does not take the live site down with it.
    pub fn settle(&mut self) {
        for job in self.jobs.values_mut() {
            job.settle();
        }
        let Some(staged) = self.staged_site.as_ref() else {
            return;
        };
        // A job the branch no longer holds — retired out of its bounded history, or
        // deleted — settles nothing: there is no status left to read, so the staged
        // files are dropped rather than published on no evidence.
        let Some(job) = self.jobs.get(&staged.job_id) else {
            self.staged_site = None;
            return;
        };
        if job.settles_at.is_some() {
            return;
        }
        let succeeded = job.job.summary.status == JOB_STATUS_SUCCEED;
        let staged = self.staged_site.take();
        if let (true, Some(staged)) = (succeeded, staged) {
            self.site = staged.files;
        }
    }

    /// Mint a `jobId` for a job this branch is STARTING, and count that job as started.
    ///
    /// `JobId` is modelled `length 0..255, pattern ^[0-9]+$`
    /// (`smithy_query(query="shape",shape="JobSummary")`), so a job id is decimal
    /// digits — not an opaque token — and a job is addressed within its branch
    /// (`/apps/{appId}/branches/{branchName}/jobs/{jobId}`). The ids are therefore a
    /// per-branch count: the first job a branch runs is `1`, the next `2`, which is
    /// also what `Branch$activeJobId` and `Branch$totalNumberOfJobs` report about.
    pub fn mint_job_id(&mut self) -> String {
        let job_id = self.reserve_job_id();
        self.started_a_job();
        job_id
    }

    /// Count one more job as STARTED on this branch without taking an id out of its
    /// sequence.
    ///
    /// What starting a deployment whose id was already reserved does: the id came out of
    /// the sequence when `CreateDeployment` reserved it ([`Self::reserve_job_id`]), and
    /// the job under it exists only once a start call creates it — which is the moment
    /// the branch has started one more job.
    pub fn started_a_job(&mut self) {
        self.started_jobs = self.started_jobs.wrapping_add(1);
    }

    /// Take the next `jobId` of this branch's id sequence without starting a job on it.
    ///
    /// What `CreateDeployment` reserves: it hands the caller the id its later
    /// `StartDeployment` will run under, so the id is spent — no job this branch
    /// starts afterwards may be handed the same one — while the branch has still
    /// started no job, since nothing runs until that start call.
    pub fn reserve_job_id(&mut self) -> String {
        loop {
            self.next_job_seq = self.next_job_seq.wrapping_add(1);
            let candidate = self.next_job_seq.to_string();
            if !self.jobs.contains_key(&candidate) && !self.deployments.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// How many jobs this branch has STARTED, which is what
    /// `Branch$totalNumberOfJobs` reports.
    ///
    /// Not the number of jobs the branch still holds: the history is bounded (see
    /// [`AmplifyBranch::jobs`]), and a branch that has run a thousand jobs has run a
    /// thousand jobs whether or not the oldest are still readable. Not the number of
    /// ids it has handed out either: a reserved deployment nobody started is not a job.
    pub fn jobs_started(&self) -> u64 {
        self.started_jobs
    }

    /// Drop every reservation whose deployment window the clock has already closed.
    ///
    /// The window closes at [`PendingDeployment::expires_at`] whether or not anybody
    /// calls, so a call that reaches this branch materialises it rather than keeping a
    /// reservation that can no longer be started. Before that instant this is a
    /// no-op, so a caller that starts its deployment inside the window still finds it.
    pub fn expire_deployments(&mut self) {
        let now = now_millis();
        self.deployments
            .retain(|_, pending| now < pending.expires_at);
    }

    /// How many bytes of DEPLOYMENT CONTENT this branch retains: what has been uploaded
    /// to its live reservations, plus the files of the site it serves and of the
    /// deployment waiting to replace it.
    ///
    /// All of it is caller content held in this process, which is why the store counts
    /// it (see [`MAX_STORED_DEPLOYMENT_BYTES`]). `token` is the one upload NOT counted —
    /// the slot a write is about to replace — and is the empty string for a caller that
    /// is replacing none.
    pub fn retained_bytes_apart_from_upload(&self, token: &str) -> usize {
        let reserved = self
            .deployments
            .values()
            .map(|pending| pending.uploaded_bytes_apart_from(token));
        let published = self.site.values().map(Vec::len);
        let staged = self
            .staged_site
            .iter()
            .flat_map(|staged| staged.files.values().map(Vec::len));
        reserved
            .chain(published)
            .chain(staged)
            .fold(0usize, usize::saturating_add)
    }

    /// The `jobId` of this branch's LAST job when that job has not reached a terminal
    /// status, or `None` when the branch has no job or its last one is over.
    ///
    /// Deployments of one branch are SERIALIZED on this: the last job is the one with the
    /// highest id — a branch's ids are a decimal sequence ([`Self::reserve_job_id`]), so
    /// numeric order is the order they were handed out — and while it is still in flight
    /// a further deployment cannot be ordered against it.
    ///
    /// The clock-owed work is materialised first, because a job the clock has already
    /// finished is a finished job here too even if nobody has read it yet: a reservation
    /// whose window has closed is dropped, and the job that window failed settles.
    pub fn last_unfinished_job(&mut self) -> Option<String> {
        self.expire_deployments();
        self.settle();
        let (job_id, last) = self
            .jobs
            .iter()
            .max_by_key(|(job_id, _)| job_id.parse::<u128>().unwrap_or(0))?;
        (!is_terminal_job_status(&last.job.summary.status)).then(|| job_id.clone())
    }

    /// Retire old jobs until this branch can hold one more, and report whether it can.
    ///
    /// The retention policy of [`AmplifyBranch::jobs`], applied: the map never exceeds
    /// [`MAX_JOBS_PER_BRANCH`] entries, and the entry that makes room is the oldest
    /// (lowest-numbered, i.e. first-started) job that has reached a TERMINAL status — a
    /// job that is still running is never dropped out from under the caller watching
    /// it. The clock-owed settling is materialised first, because a job the clock has
    /// already finished is retirable even if nobody has read it yet.
    ///
    /// `false` means the branch is at its cap and every job it holds is still running,
    /// so there is nothing to retire: the caller refuses the start with the modelled
    /// `LimitExceededException` rather than growing the map.
    pub fn make_room_for_a_job(&mut self) -> bool {
        if self.jobs.len() < MAX_JOBS_PER_BRANCH {
            return true;
        }
        for stored in self.jobs.values_mut() {
            stored.settle();
        }
        while self.jobs.len() >= MAX_JOBS_PER_BRANCH {
            // A minted id is decimal, so numeric order IS the order the jobs started;
            // an id that does not parse sorts first and is retired first.
            let Some(oldest) = self
                .jobs
                .iter()
                .filter(|(_, stored)| is_terminal_job_status(&stored.job.summary.status))
                .min_by_key(|(job_id, _)| job_id.parse::<u128>().unwrap_or(0))
                .map(|(job_id, _)| job_id.clone())
            else {
                return false;
            };
            self.jobs.remove(&oldest);
        }
        true
    }
}

/// How many jobs one branch's history holds at once.
///
/// CHOSEN, not documented: AWS publishes no per-branch job-history quota, so this is
/// the emulator's own retention bound — a branch's job map cannot grow without limit
/// however many jobs are started on it. It is the page maximum `ListJobs` models
/// (`maxResults` `range 0..50`, `smithy_query(query="operation",operation="ListJobs")`),
/// so a branch's whole retained history is reportable in one page. Reaching it retires
/// the oldest finished job rather than refusing the caller; see
/// [`AmplifyBranch::make_room_for_a_job`].
pub const MAX_JOBS_PER_BRANCH: usize = 50;

/// The `message` an Amplify `LimitExceededException` carries when a branch cannot take
/// one more job: it is at [`MAX_JOBS_PER_BRANCH`] and every job it holds is still
/// running, so there is nothing to retire (see [`AmplifyBranch::make_room_for_a_job`]).
///
/// Built FROM THE REQUEST and from the limit, and formatted HERE rather than in each
/// handler, so `StartJob` and `StartDeployment` — the two calls that start a job on a
/// branch — cannot drift on the sentence they refuse with.
pub fn job_limit_message(branch_name: &str) -> String {
    format!("Number of running jobs exceeds the limit of {MAX_JOBS_PER_BRANCH} for branch {branch_name}")
}

/// The `jobArn` of one job of a branch: the branch's own ARN, the `jobs` collection, and
/// the job's NUMBER padded with leading zeroes to [`JOB_ARN_NUMBER_DIGITS`] digits.
///
/// The ARN and the `jobId` of one summary are NOT the same string: `jobId` is modelled
/// `pattern ^[0-9]+$` and reports the branch's bare job number, while recorded behaviour
/// shows the last segment of the ARN as that number in a fixed-width field — the form
/// every job ARN Amplify emits has, and so the form an IAM resource pattern or a log
/// correlation built from one matches against. Formatted HERE rather than in each
/// handler, so the call that reserves a job, the two that start one and the readers that
/// report them cannot drift on it.
///
/// A number too long for the field is written as it stands: padding only ever adds
/// digits, so an id outside the width is still addressable.
pub fn job_arn(branch_arn: &str, job_id: &str) -> String {
    format!(
        "{branch_arn}/jobs/{job_id:0>width$}",
        width = JOB_ARN_NUMBER_DIGITS
    )
}

/// How wide the job-number field of a [`job_arn`] is.
pub const JOB_ARN_NUMBER_DIGITS: usize = 10;

/// One deployment reserved on a branch and not started yet: the `jobId` it reserved,
/// where its content is uploaded, and the window it has to be started in.
///
/// A manual deployment is TWO calls — one reserves the job id and the upload
/// destinations, a later one starts the job that deploys what was uploaded — so
/// everything the first call minted is persisted here for the second to resolve from
/// the `jobId` the caller was handed.
#[derive(Debug, Clone)]
pub struct PendingDeployment {
    /// The `jobId` this reservation took out of its branch's id sequence, and so the
    /// id the deployment's job runs under once it is started.
    pub job_id: String,
    /// The files the reservation named, keyed by file name: empty when the caller
    /// reserved a single upload of the whole zipped deployment.
    pub files: BTreeMap<String, DeploymentFile>,
    /// The single upload of the zipped deployment, or `None` when the reservation
    /// named its files individually and so has no zip upload.
    pub zip: Option<DeploymentUpload>,
    /// Epoch milliseconds at which this reservation was created.
    pub created_at: i64,
    /// Epoch milliseconds past which this reservation can no longer be started:
    /// `created_at` plus [`DEPLOYMENT_WINDOW_MILLIS`].
    pub expires_at: i64,
}

impl PendingDeployment {
    /// The value `CreateDeployment` reports as `zipUploadUrl`: the destination of the
    /// zipped deployment, or the empty string for a reservation that has none.
    ///
    /// `zipUploadUrl` is returned "when the fileMap argument is not provided in the
    /// request", so a reservation that named its files has none — and the member is
    /// then left OUT of the answer rather than sent empty (see
    /// `crate::ops::types::CreateDeploymentResponse`).
    pub fn zip_upload_url(&self) -> String {
        self.zip
            .as_ref()
            .map(|zip| zip.url.clone())
            .unwrap_or_default()
    }

    /// Has anything been uploaded to this reservation's destinations yet?
    ///
    /// What `StartDeployment` deploys from a reservation is the content AT those
    /// destinations, so this is the precondition of starting it: a reservation nothing
    /// has been uploaded to has no deployment file to deploy.
    pub fn holds_an_upload(&self) -> bool {
        self.zip.as_ref().is_some_and(|zip| zip.content.is_some())
            || self
                .files
                .values()
                .any(|file| file.upload.content.is_some())
    }

    /// Whether `token` authorises one of this reservation's uploads.
    pub fn holds_upload(&self, token: &str) -> bool {
        self.zip.as_ref().is_some_and(|zip| zip.token == token)
            || self
                .files
                .values()
                .any(|file| file.upload.token == token)
    }

    /// The upload `token` authorises, to WRITE: the zipped deployment's, or the one
    /// minted for a file this reservation named.
    pub fn upload_mut(&mut self, token: &str) -> Option<&mut DeploymentUpload> {
        if self.zip.as_ref().is_some_and(|zip| zip.token == token) {
            return self.zip.as_mut();
        }
        self.files
            .values_mut()
            .map(|file| &mut file.upload)
            .find(|upload| upload.token == token)
    }

    /// How many bytes this reservation already holds, NOT counting the upload `token`
    /// names — which is what a write to that one upload would replace.
    ///
    /// A reservation's content is caller content held in this process, and how many
    /// files a reservation names is the caller's own choice, so what a reservation may
    /// hold is bounded as a WHOLE rather than only per upload.
    pub fn uploaded_bytes_apart_from(&self, token: &str) -> usize {
        let zip = self
            .zip
            .iter()
            .filter(|zip| zip.token != token)
            .filter_map(|zip| zip.content.as_ref().map(Vec::len));
        let files = self
            .files
            .values()
            .filter(|file| file.upload.token != token)
            .filter_map(|file| file.upload.content.as_ref().map(Vec::len));
        zip.chain(files).fold(0usize, usize::saturating_add)
    }

    /// Whether the clock has already closed this reservation's window — the 8 hours
    /// [`DEPLOYMENT_WINDOW_MILLIS`] carries — at `now`.
    pub fn has_expired(&self, now: i64) -> bool {
        now >= self.expires_at
    }
}

/// One file of a [`PendingDeployment`]: the content hash its caller declared for it,
/// and the upload minted for that one file.
#[derive(Debug, Clone)]
pub struct DeploymentFile {
    /// The hash of the file's content, as the reservation's caller declared it
    /// (`FileMap` is modelled file name -> `MD5Hash`,
    /// `smithy_query(query="shape",shape="FileMap")`).
    pub content_md5: String,
    /// Where this one file's content is uploaded, and the content that arrived there.
    pub upload: DeploymentUpload,
}

/// ONE upload of a reserved deployment: the destination the reservation handed its
/// caller, the token that authorises writing to it, and the bytes that were written.
///
/// The content lives HERE because the destination is served by this emulator (see
/// [`deployment_upload_path`]): what a deployment deploys is what was actually
/// uploaded to the destinations its reservation handed out, so the bytes have to
/// outlive the request that wrote them and be readable by the later call that starts
/// the deployment.
#[derive(Debug, Clone)]
pub struct DeploymentUpload {
    /// The opaque token that authorises this one upload ([`ComAmazonawsAmplifyAmplifyState::mint_upload_token`]).
    pub token: String,
    /// The URL the reservation handed its caller for this upload.
    pub url: String,
    /// The bytes uploaded to it, or `None` while nothing has been uploaded.
    pub content: Option<Vec<u8>>,
}

/// What a request to a deployment-upload URL names: the app it was reserved under, the
/// reservation's `jobId`, and the token authorising that one upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentUploadTarget {
    pub app_id: String,
    pub job_id: String,
    pub token: String,
}

/// The path the deployment-upload plane serves ONE upload of a reservation under.
///
/// It sits under the app the deployment was reserved on — the same `/apps` space the
/// modelled routes use — and names no modelled operation: a deployment is reserved at
/// `POST /apps/{appId}/branches/{branchName}/deployments` and started at
/// `/apps/{appId}/branches/{branchName}/deployments/start`
/// (`smithy_query(query="operation",operation="CreateDeployment,StartDeployment")`),
/// neither of which this path shape can collide with. That is what lets the service
/// serve it from its routing-error seam.
///
/// The BRANCH is not part of the path: a branch name is free text
/// (`BranchName` `pattern ^(?s).+$`), which would have to be escaped into a path
/// segment, and the `jobId` and the token already locate the one reservation this
/// upload belongs to.
pub fn deployment_upload_path(app_id: &str, job_id: &str, token: &str) -> String {
    format!("/apps/{app_id}/deployments/{job_id}/uploads/{token}")
}

/// The upload a request path names, or `None` when the path is not one this service
/// publishes.
pub fn parse_deployment_upload_path(path: &str) -> Option<DeploymentUploadTarget> {
    let rest = path.strip_prefix("/apps/")?;
    let mut segments = rest.split('/');
    let (Some(app_id), Some("deployments"), Some(job_id), Some("uploads"), Some(token), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if app_id.is_empty() || job_id.is_empty() || token.is_empty() {
        return None;
    }
    Some(DeploymentUploadTarget {
        app_id: app_id.to_string(),
        job_id: job_id.to_string(),
        token: token.to_string(),
    })
}

/// The URL one upload of a reserved deployment is written to: the endpoint the caller
/// reached, plus that upload's own path.
///
/// NO SigV4 signature is attached — this emulator does not sign URLs, and a fabricated
/// signature would claim a computation nothing performed. The token in the path is the
/// receipt that authorises the write: it is minted per upload, unguessable, and stops
/// being usable when the reservation's window closes.
pub fn deployment_upload_url(host: &str, app_id: &str, job_id: &str, token: &str) -> String {
    format!(
        "https://{host}{}",
        deployment_upload_path(app_id, job_id, token)
    )
}

/// How long a reserved deployment may wait to be started.
///
/// DOCUMENTED, not chosen: "The maximum duration between the CreateDeployment call
/// and the StartDeployment call cannot exceed 8 hours"
/// (`smithy_query(query="doc",shape="CreateDeployment")`), so the window a
/// reservation carries is exactly those 8 hours.
pub const DEPLOYMENT_WINDOW_MILLIS: i64 = 8 * 60 * 60 * 1_000;

/// How many bytes of DEPLOYMENT CONTENT this service's store retains AT ONCE, across
/// every reservation's uploads and every branch's published and staged site.
///
/// CHOSEN, not a published quota: the content is caller content kept in this process,
/// and the per-deployment clamps
/// ([`crate::ops::start_deployment::MAX_ARCHIVE_BYTES`]) bound ONE item while the number
/// of reservations, branches and apps a caller may create bounds nothing — so without
/// this ceiling the total the process holds would grow with the number of items. It is
/// twice that per-deployment bound: enough for one deployment's content being uploaded
/// beside one published site of the same size, which is the most a single branch needs
/// to go from one site to the next, and it is the order the sibling in-memory payload
/// stores in this workspace use.
///
/// Reaching it refuses the WRITE that would pass it — the upload plane's 413, and a
/// deployment that fails rather than retaining its files — so nothing already published
/// is dropped out from under a visitor. Content is released when a reservation is
/// started (its uploads are spent), when a later deployment replaces a branch's site,
/// and when the branch or the app holding it is deleted.
pub const MAX_STORED_DEPLOYMENT_BYTES: usize = 2 * crate::ops::start_deployment::MAX_ARCHIVE_BYTES;

/// What the upload plane and a starting deployment are refused with when the write
/// would take this store past [`MAX_STORED_DEPLOYMENT_BYTES`].
pub const STORED_DEPLOYMENT_BYTES_EXCEEDED: &str =
    "The deployment content this endpoint holds is at its limit";

/// One job as this service persists it: the modelled [`Job`] record, the instant at
/// which the clock owes that job its next status change, and what that change is.
///
/// Starting a job is ASYNCHRONOUS — the call that starts it answers with a non-terminal
/// status and the job settles later — so the record carries when that settling is due,
/// and what it settles to, instead of blocking the call that started it.
/// [`BranchJob::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct BranchJob {
    /// The job record, exactly as every reader returns it.
    pub job: Job,
    /// Epoch milliseconds at which this job's non-terminal status is due to settle,
    /// or `None` once it has settled and the status is terminal.
    pub settles_at: Option<i64>,
    /// The terminal status, step and reason the clock settles this job to, DECIDED when
    /// the job was started rather than when it is read: what the job's work found is
    /// what it found at the instant it ran, so two reads of one settled job cannot
    /// disagree about how it ended.
    pub conclusion: JobConclusion,
    /// The asset paths this job's deployment published, relative to the root of the
    /// hosted branch — empty for a job that deployed nothing (every job that failed,
    /// and every job that is not a deployment at all).
    ///
    /// Held on the JOB rather than on the branch because it is what THIS job deployed:
    /// a later deployment of the same branch publishes its own set, and a reader of an
    /// older job still sees the one that job published.
    pub deployed_assets: BTreeSet<String>,
    /// Whether this job's work has BEGUN — whether a start call is running it.
    ///
    /// `false` for the job a `CreateDeployment` reserved and nothing has started: that
    /// job holds a number and a reservation, and no work. The distinction is what
    /// [`Self::cancel`] turns on, because there is nothing to wind down for work that
    /// never began.
    pub work_has_begun: bool,
}

impl BranchJob {
    /// A job that has just been started: [`JOB_STATUS_PENDING`] now, settled by the
    /// clock [`JOB_SETTLE_MILLIS`] from now to `conclusion`, having published `assets`.
    pub fn settling(
        summary: JobSummary,
        conclusion: JobConclusion,
        deployed_assets: BTreeSet<String>,
    ) -> Self {
        Self {
            job: Job {
                // The job is RUNNING, so the service determined every member of its
                // summary when it started it.
                summary: JobSummaryProjection::executed(summary),
                // The steps of a job are what its execution produced, and it has
                // produced none yet; the settling the clock owes records the one step
                // (see [`Self::settle`]).
                steps: Vec::new(),
            },
            settles_at: Some(now_millis().saturating_add(JOB_SETTLE_MILLIS)),
            conclusion,
            deployed_assets,
            work_has_begun: true,
        }
    }

    /// The job a `CreateDeployment` call creates: the deployment's number is spent and
    /// nothing has started it, which is what [`JOB_STATUS_PENDING`] says.
    ///
    /// The clock already owes this job one status change: "the maximum duration between
    /// the CreateDeployment call and the StartDeployment call cannot exceed 8 hours. If
    /// the duration exceeds 8 hours, the StartDeployment call and the associated Job will
    /// fail" (`smithy_query(query="doc",shape="CreateDeployment")`), so the job settles
    /// to `FAILED` when the reservation's window closes — `window_closes_at`, the same
    /// instant [`PendingDeployment::expires_at`] drops the reservation. A deployment that
    /// IS started replaces this record with the started job's own
    /// ([`Self::settling`]).
    pub fn reserved(summary: JobSummary, window_closes_at: i64) -> Self {
        Self {
            job: Job {
                // Nothing has run under this job, so most of its summary has no value to
                // report (see [`JobSummaryProjection::reserved`]).
                summary: JobSummaryProjection::reserved(summary),
                steps: Vec::new(),
            },
            settles_at: Some(window_closes_at),
            conclusion: JobConclusion::not_deployed(DEPLOYMENT_WINDOW_CLOSED_REASON),
            deployed_assets: BTreeSet::new(),
            work_has_begun: false,
        }
    }

    /// A job `StartJob` has just started: a build of the branch's repository, which
    /// this emulator has no environment to run (see
    /// [`JobConclusion::build_without_an_environment`]), so it publishes no assets.
    pub fn started(summary: JobSummary) -> Self {
        Self::settling(
            summary,
            JobConclusion::build_without_an_environment(),
            BTreeSet::new(),
        )
    }

    /// Materialise the status change the clock has ALREADY made due, and nothing else.
    ///
    /// The change is owed at `settles_at` whether or not anybody calls, so a reporting
    /// operation applies it before answering rather than reporting a `PENDING` job that
    /// the clock settled seconds ago. Before that instant this is a no-op, so a caller
    /// that reads straight after the start call still observes the non-terminal status.
    ///
    /// What it settles TO is the job's own [`JobConclusion`], recorded when the job was
    /// started: the reason is carried on the one modelled member that can hold it —
    /// `Step$statusReason` — because `JobSummary` models no reason of its own.
    pub fn settle(&mut self) {
        let Some(due) = self.settles_at else {
            return;
        };
        if now_millis() < due {
            return;
        }
        self.settles_at = None;
        let ended = ::aws_smithy_types::DateTime::from_millis(due);
        self.job.summary.status = self.conclusion.status.clone();
        self.job.summary.end_time = Some(ended);
        self.job.steps = vec![Step {
            step_name: self.conclusion.step_name.clone(),
            start_time: self.job.summary.start_time,
            status: self.conclusion.status.clone(),
            end_time: ended,
            status_reason: self.conclusion.status_reason.clone(),
            // Every URL below addresses an artifact a build produced, and no build
            // ran; `screenshots` and `context` likewise only exist for a step that
            // executed.
            log_url: None,
            artifacts_url: None,
            test_artifacts_url: None,
            test_config_url: None,
            screenshots: None,
            context: None,
        }];
    }

    /// Stop this job: it reaches [`JOB_STATUS_CANCELLED`] now if no work had begun under
    /// it, and otherwise leaves the status it is in for [`JOB_STATUS_CANCELLING`] while
    /// the clock settles the stop to [`JOB_STATUS_CANCELLED`].
    ///
    /// Winding work down takes time, which is why `JobStatus` models both halves of
    /// stopping (`CANCELLING` and `CANCELLED`,
    /// `smithy_query(query="shape",shape="JobStatus")`): "stops a job that is in
    /// progress" names work that is already running, so a stop of one is answered with
    /// the non-terminal status and the terminal one is owed [`JOB_SETTLE_MILLIS`] later.
    /// A job NOTHING had begun has no work to wind down, so its cancellation is complete
    /// when the call answers — the terminal status is in that call's own response, and no
    /// settling is owed. The step the job is cancelled IN is the one its recorded
    /// conclusion names, because that is the phase the job was stopped in; only the
    /// status and the reason are the cancellation's.
    pub fn cancel(&mut self) {
        self.conclusion = JobConclusion::cancelled(&self.conclusion.step_name);
        if !self.work_has_begun {
            self.job.summary.status = JOB_STATUS_CANCELLED.to_string();
            // A job that never ran produced no step and has no execution that ended, so
            // it reports neither steps nor an end instant.
            self.job.summary.end_time = None;
            self.job.steps = Vec::new();
            self.settles_at = None;
            return;
        }
        self.job.summary.status = JOB_STATUS_CANCELLING.to_string();
        self.job.summary.end_time = None;
        self.job.steps = Vec::new();
        self.settles_at = Some(now_millis().saturating_add(JOB_SETTLE_MILLIS));
    }
}

/// How one job ends: the terminal `JobStatus` it reaches, the execution step that
/// carries that status, and the reason for it when there is one to give.
///
/// `JobStatus` is modelled `CREATED | PENDING | PROVISIONING | RUNNING | FAILED |
/// SUCCEED | CANCELLING | CANCELLED` (`smithy_query(query="shape",shape="JobStatus")`),
/// of which `FAILED` and `SUCCEED` are the two a job of this emulator settles to.
#[derive(Debug, Clone)]
pub struct JobConclusion {
    /// The terminal `JobStatus` the job settles to.
    pub status: String,
    /// The `Step$stepName` of the one execution step the settled job carries.
    pub step_name: String,
    /// `Step$statusReason`, the reason for that status — `None` where there is nothing
    /// to explain, which is what a step that did what it was asked carries.
    pub status_reason: Option<String>,
}

impl JobConclusion {
    /// A build of a branch's repository, which this emulator has no environment to run:
    /// a build nothing executed cannot be reported as having succeeded, so the job ends
    /// [`JOB_STATUS_FAILED`] in its `BUILD` step. See [`NO_BUILD_ENVIRONMENT_REASON`].
    pub fn build_without_an_environment() -> Self {
        Self {
            status: JOB_STATUS_FAILED.to_string(),
            step_name: BUILD_STEP_NAME.to_string(),
            status_reason: Some(NO_BUILD_ENVIRONMENT_REASON.to_string()),
        }
    }

    /// A deployment whose assets were read and published: the job ends
    /// [`JOB_STATUS_SUCCEED`] in its `DEPLOY` step, with nothing to explain.
    pub fn deployed() -> Self {
        Self {
            status: JOB_STATUS_SUCCEED.to_string(),
            step_name: DEPLOY_STEP_NAME.to_string(),
            status_reason: None,
        }
    }

    /// A deployment whose assets could NOT be read and published: the job ends
    /// [`JOB_STATUS_FAILED`] in its `DEPLOY` step, carrying `reason`.
    pub fn not_deployed(reason: impl Into<String>) -> Self {
        Self {
            status: JOB_STATUS_FAILED.to_string(),
            step_name: DEPLOY_STEP_NAME.to_string(),
            status_reason: Some(reason.into()),
        }
    }

    /// A job a `StopJob` request stopped: it ends [`JOB_STATUS_CANCELLED`] in the step
    /// it was stopped in, carrying [`JOB_CANCELLED_REASON`].
    pub fn cancelled(step_name: &str) -> Self {
        Self {
            status: JOB_STATUS_CANCELLED.to_string(),
            step_name: step_name.to_string(),
            status_reason: Some(JOB_CANCELLED_REASON.to_string()),
        }
    }
}

/// How long after `StartJob` the clock settles a job out of its non-terminal status.
///
/// CHOSEN, not documented: a real Amplify build takes minutes, which no caller of an
/// emulator can wait for. The span is non-zero so a caller that reads the job straight
/// after starting it still sees the `PENDING` status a real client has to poll through.
pub const JOB_SETTLE_MILLIS: i64 = 500;

/// Why the job of a deployment nobody started ends in [`JOB_STATUS_FAILED`].
///
/// The duration is the documented one, not a chosen one: "the maximum duration between
/// the CreateDeployment call and the StartDeployment call cannot exceed 8 hours. If the
/// duration exceeds 8 hours, the StartDeployment call and the associated Job will fail"
/// (`smithy_query(query="doc",shape="CreateDeployment")`) — see
/// [`DEPLOYMENT_WINDOW_MILLIS`].
pub const DEPLOYMENT_WINDOW_CLOSED_REASON: &str =
    "The deployment was not started before its upload window closed.";

/// The status a job carries until the clock settles it: the one `StartJob` and
/// `StartDeployment` answer with, and the one the job of a reserved deployment carries
/// from `CreateDeployment` until something starts it, stops it, or its window closes.
///
/// `JobStatus` is modelled `CREATED | PENDING | PROVISIONING | RUNNING | FAILED |
/// SUCCEED | CANCELLING | CANCELLED` (`smithy_query(query="shape",shape="JobStatus")`)
/// and `StartJob` "starts a new job": the job exists and has not run yet, which is
/// what `PENDING` says — and which is also what a job holding a reserved deployment's
/// number is, since a client polls a deployment through the documented PENDING ->
/// RUNNING -> SUCCEED progression.
pub const JOB_STATUS_PENDING: &str = "PENDING";

/// The terminal status a job settles to when its work could not be done; see
/// [`JobConclusion`].
pub const JOB_STATUS_FAILED: &str = "FAILED";

/// The terminal status a job settles to when its work WAS done — the status Amplify
/// spells `SUCCEED` (`smithy_query(query="shape",shape="JobStatus")`), not `SUCCEEDED`.
pub const JOB_STATUS_SUCCEED: &str = "SUCCEED";

/// The status a job carries from `StopJob` until the clock settles the cancellation.
///
/// `JobStatus` models both halves of stopping a job — `CANCELLING` and `CANCELLED`
/// (`smithy_query(query="shape",shape="JobStatus")`) — so a stop that has been accepted
/// and not finished yet has its own status, which is the one `StopJob` answers.
pub const JOB_STATUS_CANCELLING: &str = "CANCELLING";

/// The terminal status a stopped job settles to.
pub const JOB_STATUS_CANCELLED: &str = "CANCELLED";

/// The statuses a job has reached the END of its life in: no further status is owed and
/// there is no progress left to stop.
///
/// `JobStatus` is modelled `CREATED | PENDING | PROVISIONING | RUNNING | FAILED |
/// SUCCEED | CANCELLING | CANCELLED` (`smithy_query(query="shape",shape="JobStatus")`);
/// of those, a job that has FAILED, SUCCEEDed or been CANCELLED is over, while
/// `CANCELLING` is a stop still in flight.
pub fn is_terminal_job_status(status: &str) -> bool {
    matches!(
        status,
        JOB_STATUS_FAILED | JOB_STATUS_SUCCEED | JOB_STATUS_CANCELLED
    )
}

/// Why a stopped job ends in [`JOB_STATUS_CANCELLED`], carried on the one modelled
/// member that can hold a reason (`Step$statusReason`).
pub const JOB_CANCELLED_REASON: &str = "The job was stopped by a StopJob request.";

/// The `message` a `StopJob` `BadRequestException` carries for a job that has already
/// reached a terminal status.
///
/// The operation "stops a job that is in progress"
/// (`smithy_query(query="doc",shape="StopJob")`), and this job has no progress left.
/// Unlike [`not_found_message`], which answers a request that NAMED something absent and
/// so quotes the identifier back, this refusal is about the addressed job's STATE: the
/// text Amplify answers it with is fixed for the whole error class and carries no
/// identifier, status or branch from the request or from state, so a client can match on
/// it. Hence a constant and not a template.
pub const JOB_ALREADY_TERMINAL_MESSAGE: &str =
    "Unable to cancel job. The given job is already in a terminal state.";

/// The `message` a `CreateDeployment` `BadRequestException` carries for a branch whose
/// last job has not finished.
///
/// Deployments of one branch are SERIALIZED: a new one cannot be created while the
/// branch's last job is still in flight, because the two could not be ordered against
/// each other. The sentence is fixed text apart from `job_id`, which is the number this
/// call took out of the branch's sequence — a call that is refused spends one too, so the
/// job it names is the branch's last — and it states the two ways out AWS offers: wait
/// for that job, or `StopJob` it.
pub fn unfinished_job_message(job_id: &str) -> String {
    format!(
        "The last job(deployment) {job_id} was not finished, please wait until it finish \
         or use StopJob operation to cancel it before creating a new deployment."
    )
}

/// The name of the execution step a settled job carries its failure reason on.
///
/// `Step$stepName` is modelled as free text (`smithy_query(query="shape",shape="Step")`)
/// and the phase a job fails in here is the build, which Amplify names `BUILD`.
pub const BUILD_STEP_NAME: &str = "BUILD";

/// The name of the execution step a MANUAL deployment's job carries: a manual
/// deployment runs no build — its content is already built — so the one step of its job
/// is the deploy of that content, which Amplify names `DEPLOY`.
pub const DEPLOY_STEP_NAME: &str = "DEPLOY";

/// Why a job this emulator started ends in [`JOB_STATUS_FAILED`].
pub const NO_BUILD_ENVIRONMENT_REASON: &str =
    "This emulator has no build environment, so the job's build was never executed.";

/// One domain association as this service persists it: the modelled
/// [`DomainAssociation`] record every reader returns, and the instant at which the
/// clock owes the association its next status.
///
/// Associating a custom domain is ASYNCHRONOUS — the create call answers with a
/// non-terminal `domainStatus` and the association moves on afterwards — so the
/// record carries when that move is due instead of blocking the call that started
/// it. [`AmplifyDomain::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct AmplifyDomain {
    /// The domain-association record, exactly as every reader returns it.
    pub association: DomainAssociation,
    /// Epoch milliseconds at which this association's status is due to move on, or
    /// `None` once it has reached the TERMINAL status of
    /// [`DOMAIN_STATUS_SEQUENCE`], after which nothing more is owed.
    pub settles_at: Option<i64>,
}

impl AmplifyDomain {
    /// A newly created association: the first status of [`DOMAIN_STATUS_SEQUENCE`],
    /// with the next one due [`DOMAIN_STATUS_STEP_MILLIS`] from now.
    pub fn created(association: DomainAssociation) -> Self {
        Self {
            association,
            settles_at: Some(now_millis().saturating_add(DOMAIN_STATUS_STEP_MILLIS)),
        }
    }

    /// Materialise every status change the clock has ALREADY made due, and nothing
    /// else.
    ///
    /// Each change is owed at `settles_at` whether or not anybody calls, so a
    /// reporting operation applies it before answering rather than reporting a
    /// status the clock left behind seconds ago. The loop walks as many steps as the
    /// elapsed time owes, so a reader that arrives late sees the status the
    /// association would have reached while nobody was watching — up to the TERMINAL
    /// status, where the clock owes nothing and every later read answers the same
    /// record. Before the next due instant this is a no-op, so a caller that reads
    /// straight after the create still observes the first, non-terminal status.
    pub fn settle(&mut self) {
        while let Some(due) = self.settles_at {
            if now_millis() < due {
                return;
            }
            let Some(next) = next_domain_status(&self.association.domain_status) else {
                self.settles_at = None;
                return;
            };
            self.association.domain_status = next.to_string();
            // Each status carries the reason the service gives for being in it, so
            // the reason a read reports can never belong to a status the
            // association has left.
            self.association.status_reason = domain_status_reason(next).to_string();
            self.settles_at =
                next_domain_status(next).map(|_| due.saturating_add(DOMAIN_STATUS_STEP_MILLIS));
        }
    }
}

/// The statuses the clock walks a new domain association through, in order, ending in
/// the TERMINAL one a polling client stops at.
///
/// `DomainStatus` is modelled `PENDING_VERIFICATION | IN_PROGRESS | AVAILABLE |
/// IMPORTING_CUSTOM_CERTIFICATE | PENDING_DEPLOYMENT | AWAITING_APP_CNAME | FAILED |
/// CREATING | REQUESTING_CERTIFICATE | UPDATING`
/// (`smithy_query(query="shape",shape="DomainStatus")`), and associating a custom
/// domain is created, then has its certificate requested, and then waits for the
/// domain's DNS records to prove the caller controls the domain. That proof never
/// arrives here — nothing in this emulator resolves DNS, so no record of the
/// caller's is ever observed — so the certificate cannot be issued and the
/// association ends in [`DOMAIN_STATUS_FAILED`], carrying the reason the certificate
/// service gives for that class of failure (see [`domain_status_reason`]).
pub const DOMAIN_STATUS_SEQUENCE: &[&str] = &[
    DOMAIN_STATUS_CREATING,
    DOMAIN_STATUS_REQUESTING_CERTIFICATE,
    DOMAIN_STATUS_PENDING_VERIFICATION,
    DOMAIN_STATUS_FAILED,
];

/// The status a domain association carries from its create call until the clock moves
/// it on: the association exists and nothing has been provisioned for it yet.
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";

/// The status at which the association's certificate has been requested of the
/// certificate service and not issued yet.
pub const DOMAIN_STATUS_REQUESTING_CERTIFICATE: &str = "REQUESTING_CERTIFICATE";

/// The status at which the association waits for the domain's DNS records to prove
/// the caller controls the domain.
pub const DOMAIN_STATUS_PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The TERMINAL status of [`DOMAIN_STATUS_SEQUENCE`]: the domain's certificate could
/// not be issued, so the association never serves the domain and the clock owes it
/// nothing further.
pub const DOMAIN_STATUS_FAILED: &str = "FAILED";

/// The status after `current` in [`DOMAIN_STATUS_SEQUENCE`], or `None` when `current`
/// is the terminal one (or a status the sequence does not name, which the clock owes
/// nothing).
pub fn next_domain_status(current: &str) -> Option<&'static str> {
    let at = DOMAIN_STATUS_SEQUENCE
        .iter()
        .position(|status| *status == current)?;
    DOMAIN_STATUS_SEQUENCE.get(at + 1).copied()
}

/// The `statusReason` an association carries while its `domainStatus` is `status`.
///
/// `statusReason` is "additional information that describes why the domain
/// association is in the current state"
/// (`smithy_query(query="shape",shape="DomainAssociation")`): a status the service
/// has nothing to add about carries NO reason — the empty value, which the wire
/// projection leaves out of the body — and the terminal failure carries the message
/// the certificate service gives when the domain names a certificate was requested
/// for cannot be verified. That message is fixed text; it names no value of the
/// request or of the association.
pub fn domain_status_reason(status: &str) -> &'static str {
    if status == DOMAIN_STATUS_FAILED {
        return CERTIFICATE_VERIFICATION_FAILED_REASON;
    }
    ""
}

/// Why a certificate request that cannot be verified fails, worded as the
/// certificate service words it — including the page it points the caller at, which
/// is part of the message.
pub const CERTIFICATE_VERIFICATION_FAILED_REASON: &str =
    "We require additional verification to request a certificate for one or more \
     domain names in this request. The following page describes this in more detail: \
     https://docs.aws.amazon.com/acm/latest/userguide/troubleshooting-failed.html#failed-additional-verification-required";

/// The DNS record a subdomain of an association reports while the service has
/// published none for it.
///
/// A subdomain's `dnsRecord` is "the DNS record for the subdomain"
/// (`smithy_query(query="shape",shape="SubDomain")`) — the record the CALLER
/// publishes at its DNS provider — and it cannot exist before the association's
/// certificate and the address it would point at do. Until then the service reports
/// the UNRESOLVED form: the name the subdomain is served under, which is its
/// `prefix` ([`APEX_SUBDOMAIN_PREFIX`] for the empty prefix, the domain itself),
/// pointing at [`PENDING_DNS_TARGET`] rather than at a target no client could use.
/// An association of this emulator never serves the domain (see
/// [`DOMAIN_STATUS_SEQUENCE`]), so no record is ever published for it.
pub fn pending_sub_domain_dns_record(prefix: &str) -> String {
    let name = if prefix.is_empty() {
        APEX_SUBDOMAIN_PREFIX
    } else {
        prefix
    };
    format!("{name} CNAME {PENDING_DNS_TARGET}")
}

/// The name a DNS record reports for the subdomain whose prefix is empty — the apex,
/// which is the associated domain itself.
pub const APEX_SUBDOMAIN_PREFIX: &str = "*";

/// The target a subdomain's DNS record carries until the record itself exists: the
/// marker that tells a caller to poll rather than publish.
pub const PENDING_DNS_TARGET: &str = "<pending>";

/// How long after a status the clock owes the next one.
///
/// CHOSEN, not documented: AWS publishes no duration for a domain association's
/// provisioning, and a real one takes minutes, which no caller of an emulator can
/// wait for. The span is non-zero so a caller that reads the association straight
/// after creating it still sees the first status a real client has to poll through.
pub const DOMAIN_STATUS_STEP_MILLIS: i64 = 500;

/// The certificate type an association created without `certificateSettings`
/// carries.
///
/// `CertificateType` is modelled `AMPLIFY_MANAGED | CUSTOM`
/// (`smithy_query(query="shape",shape="CertificateType")`) and the model states that
/// "If a certificate type isn't specified, Amplify uses the default AMPLIFY_MANAGED
/// certificate" (`smithy_query(query="shape",shape="CertificateSettings")`).
pub const CERTIFICATE_TYPE_AMPLIFY_MANAGED: &str = "AMPLIFY_MANAGED";

/// The certificate type that names a certificate the CALLER already holds, and so
/// the one whose ARN the request has to carry.
pub const CERTIFICATE_TYPE_CUSTOM: &str = "CUSTOM";

/// How many domain associations one app may hold at once, past which creating
/// another answers the modelled `LimitExceededException` (HTTP 429, "A resource
/// could not be created because service quotas were exceeded").
///
/// CHOSEN, not a published quota: without a bound one app's domain map could grow
/// without limit. It is the page maximum `ListDomainAssociations` models
/// (`maxResults` `range 0..50`,
/// `smithy_query(query="operation",operation="ListDomainAssociations")`), so an app's
/// whole set of associations is reportable in one page.
pub const MAX_DOMAIN_ASSOCIATIONS_PER_APP: usize = 50;

/// `DomainName`'s modelled `@pattern`, verbatim from the model
/// (`smithy_query(query="shape",shape="DomainAssociation")`).
pub const DOMAIN_NAME_PATTERN: &str =
    r"^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])(\.)?$";

/// Does one DNS label satisfy the label half of [`DOMAIN_NAME_PATTERN`]:
/// `(?!-)[A-Za-z0-9-]{min_len-1,62}[A-Za-z0-9]`?
///
/// Letters, digits and hyphens only, never opening with a hyphen, always closing on
/// a letter or a digit, and at most 63 characters. `min_len` is 1 for a label the
/// pattern writes `{0,62}` and 2 for the final label it writes `{1,62}`.
fn is_domain_label(label: &str, min_len: usize) -> bool {
    let bytes = label.as_bytes();
    bytes.len() >= min_len
        && bytes.len() <= 63
        && bytes.first() != Some(&b'-')
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// Does `name` satisfy [`DOMAIN_NAME_PATTERN`]: two or more DNS labels, optionally
/// closed by the root dot?
///
/// Checked here rather than left to the boundary validator because that pattern uses
/// negative look-ahead, which the framework's regex engine cannot compile, and a
/// `@pattern` it cannot compile is treated as matching every value. The
/// `@length(0, 64)` half of the constraint IS enforced upstream, so it is not
/// repeated here.
pub fn is_domain_name(name: &str) -> bool {
    // `(\.)?$` — one optional trailing dot, which is the DNS root and not a label.
    let name = name.strip_suffix('.').unwrap_or(name);
    let mut labels: Vec<&str> = name.split('.').collect();
    // `((…)\.)+(…)`: the final label, preceded by at least one dotted label.
    let Some(last) = labels.pop() else {
        return false;
    };
    !labels.is_empty()
        && labels.iter().all(|label| is_domain_label(label, 1))
        && is_domain_label(last, 2)
}

/// The refusal a `domainName` outside its modelled `@pattern` takes, or `None` when
/// the name satisfies it.
///
/// A value outside a MODELLED constraint is refused at request validation, before
/// the operation runs — the same class of refusal as an `appId` outside its pattern
/// or a `maxResults` outside its range, which the boundary validator answers with
/// HTTP 400 `ValidationException` naming the failing member path and the constraint
/// it violated. This pattern reaches a handler only because the framework's regex
/// engine cannot compile a negative look-ahead, so the refusal states the same two
/// facts in the clause shape that renderer uses
/// (`N validation error(s) detected: Value '…' at '<path>' failed to satisfy
/// constraint: <requirement>` — [`::aws_smithy_schema_server::Violation::message`]),
/// with the member path and the pattern the model declares.
///
/// `member` is the Smithy member name the value arrived on, lowerCamel-ed by the
/// framework's own path builder, so every operation that takes a `domainName`
/// refuses a malformed one in one wording.
pub fn domain_name_violation(member: &str, name: &str) -> Option<String> {
    if is_domain_name(name) {
        return None;
    }
    let path = ::aws_smithy_schema_server::constraints::path::member(member);
    Some(format!(
        "1 validation error detected: Value '{name}' at '{path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {DOMAIN_NAME_PATTERN}"
    ))
}

/// The symbols a branch name may carry beside letters and digits.
///
/// Source: the refusal AWS answers a name outside the set with, which states the set as
/// "letters, numbers, and the symbols /_.-" ([`BRANCH_NAME_CHARACTERS_MESSAGE`]).
const BRANCH_NAME_SYMBOLS: &str = "/_.-";

/// Whether `name` is a branch name this service accepts: letters, digits and
/// [`BRANCH_NAME_SYMBOLS`], and nothing else.
///
/// The letters and digits are ASCII because a branch name becomes the branch's default
/// domain PREFIX (`Branch$displayName` "is used as the default domain prefix", and the
/// site of a branch answers at `<displayName>.<defaultDomain>`), and a host label is
/// ASCII: a name outside this set would name a branch no visitor could address.
///
/// The `@length` half of `BranchName`'s modelled constraint (`length 1..255`) is enforced
/// upstream by the boundary validator, so it is not repeated here.
pub fn is_branch_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || BRANCH_NAME_SYMBOLS.contains(c))
}

/// The sentence a branch name outside [`is_branch_name`] is refused with.
///
/// `BranchName` is modelled `@length 1..255, pattern ^(?s).+$`
/// (`smithy_query(query="operation",operation="CreateBranch")`), which admits any
/// non-empty value, so the character set is a rule the service enforces and the model
/// does not express: the refusal is the service's own, and its text is fixed apart from
/// the name the request sent.
pub fn branch_name_not_supported_message(branch_name: &str) -> String {
    format!(
        "The requested branch name is not supported. Failed to create branch. The branch \
         name {branch_name} is invalid. {BRANCH_NAME_CHARACTERS_MESSAGE}"
    )
}

/// The clause [`branch_name_not_supported_message`] closes with: the character set a
/// branch name is drawn from, as AWS states it.
const BRANCH_NAME_CHARACTERS_MESSAGE: &str =
    "The name can only contain letters, numbers, and the symbols /_.-";

/// Per-service state for com-amazonaws-amplify-amplify. A handler reaches it through [`service_state`].
///
/// The apps map is NOT partitioned by region: an app's ARN records the region of the
/// request that created it, but any later call reaches every app in the store
/// whatever region it is signed for. Partitioning the control plane by region is a
/// separate change; nothing here claims it.
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifyAmplifyState {
    /// Every app that exists, keyed by its minted `appId`.
    pub apps: BTreeMap<String, AmplifyApp>,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_app_id`].
    pub next_app_id_seq: u64,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_webhook_id`].
    pub next_webhook_id_seq: u64,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_upload_token`].
    pub next_upload_token_seq: u64,
}

impl ComAmazonawsAmplifyAmplifyState {
    /// Mint an `appId` no app in state is using.
    ///
    /// `AppId` is modelled `length 1..20, pattern ^d[a-z0-9]+$`
    /// (`smithy_query(query="shape",shape="App")`), so the id is a literal `d`
    /// followed by 13 base-36 digits — 14 characters, inside the bound, and inside
    /// the pattern's alphabet.
    ///
    /// The 13 digits come from a monotonic counter mixed with the wall clock through
    /// the SplitMix64 finaliser, so ids do not expose a guessable sequence, and the
    /// loop re-mints on the (astronomically unlikely) collision, so two `CreateApp`
    /// calls can never be handed the same id.
    pub fn mint_app_id(&mut self) -> String {
        loop {
            self.next_app_id_seq = self.next_app_id_seq.wrapping_add(1);
            let candidate = format!("d{}", base36_digits(mixed(self.next_app_id_seq)));
            if !self.apps.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `webhookId` no webhook in state is using, and the opaque token that
    /// authorises the webhook's own URL.
    ///
    /// `WebhookId` is modelled `length 0..255, pattern ^(?s)`
    /// (`smithy_query(query="shape",shape="Webhook")`), so the format is the
    /// service's rather than the model's: Amplify hands back a UUID, which is what
    /// [`uuid_v4`] renders — 36 characters, inside the modelled bound.
    ///
    /// The token is minted in the same call because it is part of the same secret:
    /// knowing a webhook's id is not enough to trigger it, so the URL carries a
    /// second unguessable value. Both come from the same mixer that mints an app id,
    /// so neither exposes the counter behind it, and the loop re-mints on a
    /// collision, so two creates can never be handed the same id.
    pub fn mint_webhook_id(&mut self) -> (String, String) {
        loop {
            self.next_webhook_id_seq = self.next_webhook_id_seq.wrapping_add(1);
            let (high, low) = mixed_pair(self.next_webhook_id_seq);
            let candidate = uuid_v4(high, low);
            if self.webhook(&candidate).is_none() {
                let (token_high, token_low) = mixed_pair(low ^ high);
                return (candidate, format!("{token_high:016x}{token_low:016x}"));
            }
        }
    }

    /// Mint the opaque token that authorises ONE upload of a reserved deployment.
    ///
    /// A deployment's upload destination is handed to the caller and to nobody else,
    /// so what authorises writing to it cannot be the app id, the branch name or the
    /// job id the caller already knows: it is 128 bits from the same mixer that mints
    /// an app id, rendered as hexadecimal, minted once per upload so no two uploads of
    /// a deployment — and no two deployments — share a destination.
    pub fn mint_upload_token(&mut self) -> String {
        self.next_upload_token_seq = self.next_upload_token_seq.wrapping_add(1);
        let (high, low) = mixed_pair(self.next_upload_token_seq);
        format!("{high:016x}{low:016x}")
    }

    /// The webhook `webhook_id` names, whichever app holds it.
    ///
    /// `GetWebhook`, `UpdateWebhook` and `DeleteWebhook` address a webhook by its id
    /// alone, and an id is unique across the store (see [`Self::mint_webhook_id`]), so
    /// the lookup is a scan of the apps for the one holding that id.
    pub fn webhook(&self, webhook_id: &str) -> Option<&Webhook> {
        self.apps
            .values()
            .find_map(|app| app.webhooks.get(webhook_id))
    }

    /// The webhook `webhook_id` names, whichever app holds it, to MUTATE.
    ///
    /// The same resolution as [`Self::webhook`], for the operation that writes the
    /// record rather than reading it: a webhook is addressed by its id alone, so the
    /// app holding it is found rather than named by the request. One lookup for both
    /// sides keeps `UpdateWebhook` and `GetWebhook` from disagreeing about which
    /// webhook an id resolves to.
    pub fn webhook_mut(&mut self, webhook_id: &str) -> Option<&mut Webhook> {
        self.apps
            .values_mut()
            .find_map(|app| app.webhooks.get_mut(webhook_id))
    }

    /// The tag map of the resource `resource_arn` names, to READ.
    ///
    /// Answers the map the resource carries — the empty map when it carries none —
    /// or, for an ARN that names no resource this store holds, the `message` a 404
    /// carries for it (see [`not_found_message`]). A clone rather than a borrow:
    /// reading tags must leave the record untouched, and a resource with no tags yet
    /// has no map to borrow.
    pub fn resource_tags(&self, resource_arn: &str) -> Result<HashMap<String, String>, String> {
        let tags = match tagged_resource(resource_arn) {
            Some(TaggedResource::App { app_id }) => &self.app_for_tagging(app_id)?.app.tags,
            Some(TaggedResource::Branch {
                app_id,
                branch_name,
            }) => {
                let app = self.app_for_tagging(app_id)?;
                let branch = app
                    .branches
                    .get(branch_name)
                    .ok_or_else(|| tag_not_found_message(BRANCH_ENTITY, branch_name))?;
                &branch.branch.tags
            }
            None => return Err(tag_not_found_message(RESOURCE_ENTITY, resource_arn)),
        };
        Ok(tags.clone().unwrap_or_default())
    }

    /// The tag map of the resource `resource_arn` names, to WRITE.
    ///
    /// The map lives ON the resource's own record — `App$tags` for an app,
    /// `Branch$tags` for a branch — so a tag write is reported by every operation that
    /// reports that record (`GetApp`, `ListApps`, `GetBranch`, `ListBranches`) as well
    /// as by `ListTagsForResource`, and the two cannot drift. A resource that carries
    /// no tags yet gets its (empty) map here, which is what a write then fills — and
    /// what every reader already reports for such a resource, so materialising it is
    /// not a change a caller can see.
    ///
    /// The error is the `message` a 404 carries for an ARN naming no such resource.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut HashMap<String, String>, String> {
        match tagged_resource(resource_arn) {
            Some(TaggedResource::App { app_id }) => {
                let app = self
                    .apps
                    .get_mut(app_id)
                    .ok_or_else(|| tag_not_found_message(APP_ENTITY, app_id))?;
                Ok(app.app.tags.get_or_insert_with(HashMap::new))
            }
            Some(TaggedResource::Branch {
                app_id,
                branch_name,
            }) => {
                let app = self
                    .apps
                    .get_mut(app_id)
                    .ok_or_else(|| tag_not_found_message(APP_ENTITY, app_id))?;
                let branch = app
                    .branches
                    .get_mut(branch_name)
                    .ok_or_else(|| tag_not_found_message(BRANCH_ENTITY, branch_name))?;
                Ok(branch.branch.tags.get_or_insert_with(HashMap::new))
            }
            None => Err(tag_not_found_message(RESOURCE_ENTITY, resource_arn)),
        }
    }

    /// Whether this store holds the reservation one deployment-upload URL names.
    ///
    /// The token in the path is the receipt: it was minted by this store, is
    /// unguessable, and stops being usable when the reservation's window closes. That
    /// is what makes the service's claim on such a request unambiguous — a URL this
    /// emulator never handed out is claimed by nobody.
    pub fn holds_deployment_upload(&self, target: &DeploymentUploadTarget) -> bool {
        self.reservation_of_upload(target).is_some()
    }

    /// The reservation a deployment-upload URL names, to READ.
    pub fn reservation_of_upload(
        &self,
        target: &DeploymentUploadTarget,
    ) -> Option<&PendingDeployment> {
        self.apps
            .get(&target.app_id)?
            .branches
            .values()
            .filter_map(|branch| branch.deployments.get(&target.job_id))
            .find(|pending| pending.holds_upload(&target.token))
    }

    /// The reservation a deployment-upload URL names, to WRITE.
    ///
    /// A `jobId` is minted per BRANCH, so the branches of the app the path names are
    /// walked for the one holding a reservation under that id AND the token — both of
    /// which the reservation itself minted, so at most one reservation matches.
    pub fn reservation_of_upload_mut(
        &mut self,
        target: &DeploymentUploadTarget,
    ) -> Option<&mut PendingDeployment> {
        self.apps
            .get_mut(&target.app_id)?
            .branches
            .values_mut()
            .filter_map(|branch| branch.deployments.get_mut(&target.job_id))
            .find(|pending| pending.holds_upload(&target.token))
    }

    /// How many bytes of DEPLOYMENT CONTENT this whole store retains — every live
    /// reservation's uploads and every branch's published and staged site — NOT counting
    /// the one upload `token` names, which is the slot a write is about to replace (the
    /// empty string counts everything).
    ///
    /// This is what [`MAX_STORED_DEPLOYMENT_BYTES`] bounds: the per-deployment clamps
    /// cap one item, and this caps the TOTAL the process holds, so no number of
    /// reservations, branches or apps can grow it without limit.
    pub fn retained_deployment_bytes_apart_from_upload(&self, token: &str) -> usize {
        self.apps
            .values()
            .flat_map(|app| app.branches.values())
            .map(|branch| branch.retained_bytes_apart_from_upload(token))
            .fold(0usize, usize::saturating_add)
    }

    /// Whether some branch of some app is HOSTED at `host` — the question
    /// `Service::can_handle` asks of a request addressed to a branch's default domain.
    pub fn hosts_a_branch(&self, host: &str) -> bool {
        let host = hosted_site_host(host);
        self.apps.values().any(|app| {
            let domain = &app.app.default_domain;
            app.branches
                .values()
                .any(|branch| hosted_branch_host(&branch.branch.display_name, domain) == host)
        })
    }

    /// Resolve one hosting-edge visit: the branch `host` names, settled, with `path`
    /// resolved against its app's rules and its own published files.
    ///
    /// A branch's site is served at `<displayName>.<app defaultDomain>`:
    /// `Branch$displayName` "is used as the default domain prefix"
    /// (`smithy_query(query="doc",shape="Branch",member="displayName")`) and
    /// `App$defaultDomain` is "the default domain for the Amplify app"
    /// (`smithy_query(query="doc",shape="App",member="defaultDomain")`). The match is
    /// case-insensitive and ignores any port, because a host is. `None` when no branch
    /// of any app is hosted there.
    ///
    /// `&mut` on the branch because serving a request REPORTS on it, and reporting
    /// materialises what the clock already owes it ([`AmplifyBranch::settle`]) — never
    /// anything else. The resolution happens HERE, where the access control, the rules
    /// and the files all live, so a visit copies none of them: both the access control
    /// (`App$enableBasicAuth` "enables basic authorization for the Amplify app's
    /// branches") and the rules are configured on the APP as well as on the branch, and
    /// applied by the edge of each of its branches.
    ///
    /// `presented` is the `user:password` pair the request's `Authorization: Basic`
    /// header carried, which is what the branch's access control is verified against.
    pub fn hosted_visit(
        &mut self,
        host: &str,
        path: &str,
        presented: Option<&[u8]>,
    ) -> Option<HostedVisit> {
        let host = hosted_site_host(host);
        self.apps.values_mut().find_map(|app| {
            let domain = app.app.default_domain.clone();
            let hosted = app.branches.iter().find_map(|(name, branch)| {
                (hosted_branch_host(&branch.branch.display_name, &domain) == host)
                    .then(|| name.clone())
            })?;
            // What the clock already owes this branch — its jobs' status changes, and
            // the site a finished deployment publishes — is materialised before anything
            // is read, so a visitor sees the deployment that has finished rather than
            // the one before it.
            app.branches.get_mut(&hosted)?.settle();
            let branch = app.branches.get(&hosted)?;
            // Access control comes FIRST: a protected site tells an unauthenticated
            // visitor nothing about what it carries, not even whether a path exists.
            if !site_is_open_to(app, branch, presented) {
                return Some(HostedVisit::unauthorized());
            }
            let rules = app.app.custom_rules.as_deref().unwrap_or_default();
            Some(HostedVisit {
                answer: resolve_hosted_request(rules, &branch.site, path),
                // The header rules are handed BACK rather than resolved here: matching
                // them against the path is bounded work on caller text
                // ([`matched_headers`]), and it happens after this lock is released, so
                // no other caller of this service waits on it. The list is shared, so
                // handing it over copies no rule.
                header_rules: Arc::clone(&app.custom_header_rules),
            })
        })
    }

    /// The app an ARN addressed for tagging, or the `message` its 404 carries.
    fn app_for_tagging(&self, app_id: &str) -> Result<&AmplifyApp, String> {
        self.apps
            .get(app_id)
            .ok_or_else(|| tag_not_found_message(APP_ENTITY, app_id))
    }
}

/// The `message` a tagging 404 carries: the crate's one not-found construction, inside
/// the length its `message` member is modelled to carry.
fn tag_not_found_message(entity: &str, identifier: &str) -> String {
    within_error_message_bound(not_found_message(entity, identifier))
}

/// The resource an Amplify ARN addresses for TAGGING, and the identifiers that locate
/// it in this store.
///
/// The taggable resources are the ones this service MODELS a tag map on: `App$tags`
/// and `Branch$tags` are the only `TagMap` members on a resource shape, and `tags` is
/// accepted only by `CreateApp` and `CreateBranch`
/// (`smithy_query(query="operation",operation="CreateApp,CreateBranch")`). An ARN
/// naming anything else carries no tags to report, so tagging answers it as a resource
/// it does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    /// `arn:aws:amplify:<region>:<account>:apps/<appId>`.
    App { app_id: &'a str },
    /// `arn:aws:amplify:<region>:<account>:apps/<appId>/branches/<branchName>`.
    Branch {
        app_id: &'a str,
        branch_name: &'a str,
    },
}

/// The resource `resource_arn` addresses, read out of the ARN's own resource part, or
/// `None` when the ARN addresses no taggable resource of this service.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>` — everything
/// after the fifth colon is the resource part, which Amplify builds as
/// `apps/<appId>` and extends per nested resource (an app's branch ARN is the app's
/// ARN plus `/branches/<branchName>`, see `CreateBranch`). The branch name is taken as
/// the WHOLE remainder, because `BranchName` is free text and a branch is commonly
/// named with a slash in it (`release/1.0`).
///
/// The ARN's region and account are NOT matched against the request: this store is not
/// partitioned by region (see [`ComAmazonawsAmplifyAmplifyState`]), so an ARN locates
/// a resource by the same identifiers `GetApp` and `GetBranch` take.
pub fn tagged_resource(resource_arn: &str) -> Option<TaggedResource<'_>> {
    let resource = resource_arn.splitn(6, ':').nth(5)?;
    let under_apps = resource.strip_prefix("apps/")?;
    match under_apps.split_once('/') {
        None => (!under_apps.is_empty()).then_some(TaggedResource::App {
            app_id: under_apps,
        }),
        Some((app_id, nested)) => {
            let branch_name = nested.strip_prefix("branches/")?;
            (!app_id.is_empty() && !branch_name.is_empty()).then_some(TaggedResource::Branch {
                app_id,
                branch_name,
            })
        }
    }
}

/// How many tags one resource may carry at once.
///
/// `TagMap` is modelled `length 0..50`
/// (`smithy_query(query="operation",operation="TagResource")`), and the map a resource
/// carries IS a `TagMap` — it is what `ListTagsForResource` answers with — so the bound
/// is on the MERGED map a tag write leaves behind, not only on one request's map.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `message` a `BadRequestException` carries when a tag write would leave the
/// resource carrying more than [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// Like [`not_found_message`] and [`already_exists_message`], the text is built FROM
/// THE REQUEST and from the state it would produce — the ARN the request named, the
/// limit, and the number of tags the merged map would hold — and follows the same
/// `Failed to <verb> …` construction those refusals use, because the model spells no
/// message out for this exception beyond "A request contains unexpected data".
pub fn tag_limit_message(resource_arn: &str, merged: usize) -> String {
    within_error_message_bound(format!(
        "Failed to tag resource {resource_arn}. A resource can have a maximum of \
         {MAX_TAGS_PER_RESOURCE} tags, and these tags would leave it with {merged}."
    ))
}

/// The status a successful Amplify response answers with when it reports NO document
/// and the request that asked for it REMOVED something: `204 No Content`.
///
/// A response body is only ever omitted where the operation's modelled output declares
/// no members, and of this service's operations exactly two do: `TagResource`, bound
/// `POST /tags/{resourceArn}`, and `UntagResource`, bound `DELETE /tags/{resourceArn}`
/// (`smithy_query(query="operation",operation="TagResource,UntagResource")`). The
/// DELETE of the pair is the one that answers No Content: there is nothing to report
/// and nothing left where the request pointed. The model pins no success code for
/// either, which leaves the restJson1 default of 200 — and a 200 promises a document
/// the response does not carry, so a client that parses a body only when there is one,
/// or that asserts the status it was told, takes the wrong branch on it.
///
/// Every other `DELETE` this service binds (`DeleteApp`, `DeleteBranch`,
/// `DeleteBackendEnvironment`, `DeleteDomainAssociation`, `DeleteJob`, `DeleteWebhook`,
/// `StopJob`) reports the record it acted on, so each of those keeps the 200 it answers
/// a document with, as does the `POST` half of the tagging pair.
///
/// The status itself is the cross-service one, and the FRAMING of an answer carrying it
/// is `service_common`'s too (see `crate::service`): what belongs to this service is only
/// which of its operations answers it.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = ::service_common::no_content_responses::NO_CONTENT_STATUS;

/// The `code` an Amplify `ResourceNotFoundException` carries.
///
/// `ResourceNotFoundException$code` is `@required` and modelled as plain text
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`): it is the error's
/// own code, which is the name the protocol reports for it in `x-amzn-errortype`.
pub const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// `message` cut to the length an Amplify error message is modelled to carry.
///
/// `ErrorMessage` is modelled `length 0..255` while `ResourceArn` is `length 0..2048`
/// (`smithy_query(query="shape",shape="ErrorMessage")`), so a refusal that quotes the
/// ARN the request carried can be longer than the member reporting it: the text is cut
/// to the modelled bound, on a character boundary, rather than sent past it.
pub fn within_error_message_bound(message: String) -> String {
    const MAX_ERROR_MESSAGE_CHARS: usize = 255;
    if message.chars().count() <= MAX_ERROR_MESSAGE_CHARS {
        return message;
    }
    message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

/// A counter value mixed with the wall clock through the SplitMix64 finaliser: the
/// entropy every minted identifier in this service is built from, so an id does not
/// expose a guessable sequence.
fn mixed(seq: u64) -> u64 {
    let clock = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    splitmix64(seq ^ clock.rotate_left(17))
}

/// 128 bits of [`mixed`] entropy, for an identifier wider than a `u64`.
fn mixed_pair(seq: u64) -> (u64, u64) {
    let high = mixed(seq);
    let low = splitmix64(high.rotate_left(32) ^ seq);
    (high, low)
}

/// `high`/`low` rendered as a version-4 UUID: 32 hexadecimal digits in the 8-4-4-4-12
/// grouping, with RFC 4122's version nibble (`4`) and variant bits (`10`) set.
fn uuid_v4(high: u64, low: u64) -> String {
    let high = (high & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let low = (low & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xFFFF,
        high & 0xFFFF,
        low >> 48,
        low & 0xFFFF_FFFF_FFFF,
    )
}

/// The SplitMix64 finaliser: an invertible bit mixer, so distinct inputs give
/// distinct outputs and a counter does not show through the minted id.
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly 13 base-36 digits over `[0-9a-z]`, zero-padded on the left.
///
/// 36^13 > `u64::MAX`, so 13 digits represent every `u64` without truncation.
fn base36_digits(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = [b'0'; 13];
    for slot in out.iter_mut().rev() {
        let digit = usize::try_from(value % 36).unwrap_or(0);
        *slot = DIGITS[digit];
        value /= 36;
    }
    out.iter().map(|byte| char::from(*byte)).collect()
}

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own shared
/// AWS config names, so an unsigned call and a default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The document a hosted branch serves for a request that names a directory rather than
/// a file — the root of the site, or a path ending in the separator.
///
/// It is the same document a deployment has to carry at its root to be servable, which
/// `StartDeployment` owns ([`crate::ops::start_deployment::ROOT_DOCUMENT`]).
pub fn site_root_document() -> &'static str {
    crate::ops::start_deployment::ROOT_DOCUMENT
}

/// The path a hosted-site request asks for, as a key of a branch's published files: the
/// request path without its leading separator, with the root document appended when it
/// names a directory.
///
/// `None` for a path that would leave the site — a `..` segment — which names no file
/// the deployment published.
pub fn hosted_site_key(path: &str) -> Option<String> {
    let path = path.strip_prefix('/').unwrap_or(path);
    if path.split('/').any(|segment| segment == ".." || segment == ".") {
        return None;
    }
    if path.is_empty() || path.ends_with('/') {
        return Some(format!("{path}{}", site_root_document()));
    }
    Some(path.to_string())
}

/// The media type a hosted file is served with, from the extension of the path it is
/// served at.
///
/// A hosting edge answers the type of the FILE, which is what a browser needs to render
/// a page rather than download it. The reading is the workspace's one
/// (`::service_common::web_media_type`), so this plane and the next one to serve deployed
/// files cannot answer one extension differently.
pub fn hosted_file_media_type(path: &str) -> &'static str {
    ::service_common::web_media_type::media_type_of_path(path)
}

/// The placeholder a rule's `source` captures a part of the request path with, and that
/// its `target` substitutes that capture back into.
///
/// `CustomRule$source` is "The source pattern for a URL rewrite or redirect rule" and
/// `target` "The target pattern" (`smithy_query(query="shape",shape="CustomRule")`): a
/// PATTERN, not a path, and this is the one placeholder those patterns are written with.
/// A source carrying none of it is a literal path, matched whole.
pub const CUSTOM_RULE_PLACEHOLDER: &str = "<*>";

/// How many rewrite and redirect rules one app may carry at once, past which a writer
/// refuses the list.
///
/// CHOSEN, not a published quota, exactly as [`MAX_DOMAIN_ASSOCIATIONS_PER_APP`] is:
/// `CustomRules` is modelled as a list with no length
/// (`smithy_query(query="shape",shape="CustomRules")`), and the stored list is walked
/// for EVERY request the hosting edge serves, so without a bound one app's
/// configuration — and the work one visit does — would grow with nothing to cap it. It
/// is far above the handful of rules a site's redirect configuration carries, and with
/// each rule's `source` modelled at most 2048 characters it bounds what resolving one
/// visit can cost.
pub const MAX_CUSTOM_RULES_PER_APP: usize = 1000;

/// The `message` a writer's refusal carries when a list would leave an app holding more
/// than [`MAX_CUSTOM_RULES_PER_APP`] rules.
///
/// Built FROM THE REQUEST — the limit, the number of rules the list carries, and the
/// identifier the request names the app by (its `appId` on an update; its `name` on a
/// create, which is all a create has) — in the same `Number of … exceeds the limit of …`
/// construction this crate's other quota refusal uses, because the model spells no
/// message out for a bound it does not state.
pub fn custom_rules_limit_message(app: &str, supplied: usize) -> String {
    within_error_message_bound(format!(
        "Number of custom rules ({supplied}) exceeds the limit of \
         {MAX_CUSTOM_RULES_PER_APP} for app {app}"
    ))
}

/// What one of an app's custom rules does to a request its `source` matches.
///
/// The five values are the ones `CustomRule$status` documents
/// (`smithy_query(query="doc",shape="CustomRule",member="status")`): `200` "Represents a
/// 200 rewrite rule", `301` "a 301 (moved permanently) redirect rule. This and all
/// future requests should be directed to the target URL", `302` "a 302 temporary
/// redirect rule", `404` "a 404 redirect rule" and `404-200` "a 404 rewrite rule".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomRuleStatus {
    /// `200`: the request is answered, with status 200, from the object the target names.
    Rewrite,
    /// `301`: the visitor is sent to the target permanently.
    MovedPermanently,
    /// `302`: the visitor is sent to the target temporarily.
    Found,
    /// `404`: the request is answered as a request for a path the site does not carry.
    NotFound,
    /// `404-200`: a request the site carries NO file for is answered, with status 200,
    /// from the object the target names — the rule a single-page app's catch-all is.
    NotFoundRewrite,
}

/// What `status` makes a rule do, or `None` when it names nothing the member documents —
/// including the rule that carries no status at all, which asks for none of the five
/// behaviours and so leaves the request to be resolved as if the rule were not there.
pub fn custom_rule_status(status: Option<&str>) -> Option<CustomRuleStatus> {
    match status?.trim() {
        "200" => Some(CustomRuleStatus::Rewrite),
        "301" => Some(CustomRuleStatus::MovedPermanently),
        "302" => Some(CustomRuleStatus::Found),
        "404" => Some(CustomRuleStatus::NotFound),
        "404-200" => Some(CustomRuleStatus::NotFoundRewrite),
        _ => None,
    }
}

/// What a hosting edge answers one request with, once the branch's access control, the
/// app's rules and the branch's published files have all been consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedVisit {
    /// What answers the request.
    pub answer: HostedAnswer,
    /// The app's custom-header rules, which the edge resolves against the request path
    /// ([`matched_headers`]) once the store's lock is released.
    pub header_rules: Arc<[CustomHeaderRule]>,
}

impl HostedVisit {
    /// The visit of a site whose access control this request did not satisfy: nothing
    /// about the site is answered, and none of its configured headers is served either —
    /// a challenge is not one of the site's own responses.
    fn unauthorized() -> Self {
        Self {
            answer: HostedAnswer::Unauthorized,
            header_rules: Vec::new().into(),
        }
    }
}

/// Whether the site of `branch` is served to a request presenting `presented` — the
/// `user:password` pair its `Authorization: Basic` header carried, if any.
///
/// Basic authorization is required when the BRANCH has it enabled, or when its app does:
/// `App$enableBasicAuth` "enables basic authorization for the Amplify app's branches"
/// (`smithy_query(query="doc",shape="App",member="enableBasicAuth")`). The credential
/// verified against is the branch's own where it configured one, and otherwise the app's
/// — `App$basicAuthCredentials` is "the basic authorization credentials for branches for
/// the Amplify app". A site with access control enabled and no credential configured
/// anywhere admits NOBODY: there is nothing a visitor could present, and a site that was
/// asked to be protected is not served openly.
fn site_is_open_to(
    app: &AmplifyApp,
    branch: &AmplifyBranch,
    presented: Option<&[u8]>,
) -> bool {
    if !app.app.enable_basic_auth && !branch.branch.enable_basic_auth {
        return true;
    }
    let Some(required) = branch.basic_auth.as_ref().or(app.basic_auth.as_ref()) else {
        return false;
    };
    presented.is_some_and(|presented| required.accepts(presented))
}

/// What a hosting edge answers one request with, once the app's rules and the branch's
/// published files have both been consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostedAnswer {
    /// Serve these bytes with status 200, as the file published at `key` — which is also
    /// the path the media type is read from, so a rewritten request carries the type of
    /// the object that ANSWERED it.
    File {
        /// The site key the bytes are published at.
        key: String,
        /// The bytes to serve.
        content: Vec<u8>,
    },
    /// Send the visitor to `location` with this redirect's status.
    Redirect {
        /// Which of the two documented redirects this is.
        redirect: HostedRedirect,
        /// The substituted target, as the `Location` the visitor follows.
        location: String,
    },
    /// Neither a rule nor a published file answers this path.
    NotFound,
    /// The branch's site is under basic authorization and this request presented no
    /// credential, or not the configured one: the visitor is challenged for one and told
    /// nothing else.
    Unauthorized,
}

/// Which redirect a rule answers with — the two [`CustomRuleStatus`] carries.
///
/// A kind rather than a number so the status an answer is built with can only ever be
/// one of the two, whatever a rule's own `status` text said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedRedirect {
    /// `301`: "This and all future requests should be directed to the target URL".
    Permanent,
    /// `302`: a temporary redirect.
    Temporary,
}

impl HostedRedirect {
    /// The HTTP status this redirect answers with.
    pub fn status(self) -> u16 {
        match self {
            Self::Permanent => 301,
            Self::Temporary => 302,
        }
    }
}

/// Resolve one request path against an app's custom rules and a branch's published
/// files.
///
/// The rules are walked IN ORDER and the FIRST one whose `source` matches the request
/// path decides the answer, per the behaviour its `status` names
/// ([`CustomRuleStatus`]); a rewrite's answer is the object the substituted `target`
/// names, so the capture the `<*>` placeholder took out of the path is put back into the
/// target. A `404-200` rule is considered only for a request the site carries no file
/// for, which is what makes it the catch-all. When no rule decides, the request is
/// answered from the file the path itself names, and is a 404 when the site carries
/// none.
///
/// Two kinds of rule this edge cannot carry out are passed over rather than guessed at:
/// one whose `condition` is set, because the condition names an attribute of the VISITOR
/// ("such as a country code",
/// `smithy_query(query="doc",shape="CustomRule",member="condition")`) that no request to
/// this emulator carries, and one whose status names none of the documented behaviours.
/// A rewrite whose target names another ORIGIN (an absolute URL) would have to be
/// fetched from there, which this edge does not do, so it answers as a path with no
/// object behind it; a REDIRECT to one is just a `Location`, and is sent as written
/// unless the substituted target cannot be sent as one at all ([`redirect_to`]).
pub fn resolve_hosted_request(
    rules: &[CustomRule],
    site: &BTreeMap<String, Vec<u8>>,
    path: &str,
) -> HostedAnswer {
    let published = hosted_site_key(path).filter(|key| site.contains_key(key));
    for rule in rules {
        if rule.condition.as_deref().is_some_and(|value| !value.is_empty()) {
            continue;
        }
        let Some(status) = custom_rule_status(rule.status.as_deref()) else {
            continue;
        };
        if status == CustomRuleStatus::NotFoundRewrite && published.is_some() {
            continue;
        }
        let Some(captured) = custom_rule_match(&rule.source, path) else {
            continue;
        };
        let Some(target) = substituted_target(&rule.target, &captured) else {
            continue;
        };
        return match status {
            CustomRuleStatus::Rewrite | CustomRuleStatus::NotFoundRewrite => {
                rewritten_file(site, &target)
            }
            CustomRuleStatus::MovedPermanently => redirect_to(HostedRedirect::Permanent, target),
            CustomRuleStatus::Found => redirect_to(HostedRedirect::Temporary, target),
            CustomRuleStatus::NotFound => HostedAnswer::NotFound,
        };
    }
    match published {
        Some(key) => published_file(site, &key),
        None => HostedAnswer::NotFound,
    }
}

/// The answer a redirect rule to `location` produces.
///
/// A target is caller text whose characters nothing constrains (`CustomRule$target` is
/// modelled by its length alone), and a redirect carries it as a header value — so a
/// target that cannot BE one names a destination no answer can send, and the rule is
/// left uncarried-out rather than half-applied.
fn redirect_to(redirect: HostedRedirect, location: String) -> HostedAnswer {
    if !sendable_as_a_header_value(&location) {
        return HostedAnswer::NotFound;
    }
    HostedAnswer::Redirect { redirect, location }
}

/// Whether `value` can be sent as a header value: non-empty, and every byte visible
/// ASCII or a tab.
pub fn sendable_as_a_header_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte == b'\t' || (0x20..0x7f).contains(&byte))
}

/// The answer a 200 (or `404-200`) rewrite to `target` produces: the object the target
/// names within the site, or nothing when the site publishes no such object — including
/// every target naming another origin, which this edge does not fetch from.
fn rewritten_file(site: &BTreeMap<String, Vec<u8>>, target: &str) -> HostedAnswer {
    if target.contains("://") {
        return HostedAnswer::NotFound;
    }
    // A target is a pattern of the same shape as a request path, so it is read as one:
    // its query string dropped, and a directory answered by the site's root document.
    match hosted_site_key(::service_common::request_path(target)) {
        Some(key) => published_file(site, &key),
        None => HostedAnswer::NotFound,
    }
}

/// The answer the file published at `key` produces, or [`HostedAnswer::NotFound`] when
/// the site publishes none.
fn published_file(site: &BTreeMap<String, Vec<u8>>, key: &str) -> HostedAnswer {
    match site.get(key) {
        Some(content) => HostedAnswer::File {
            key: key.to_string(),
            content: content.clone(),
        },
        None => HostedAnswer::NotFound,
    }
}

/// What a rule's `source` pattern captures out of `path`, or `None` when it does not
/// match it.
///
/// The pattern is the literal path around each [`CUSTOM_RULE_PLACEHOLDER`], and each
/// placeholder captures a NON-EMPTY run of the request path: `/assets/<*>` matches
/// `/assets/main.css` capturing `main.css`, and does not match `/assets/` — which names
/// a directory, not something to substitute. Both the pattern and the path are compared
/// from the site root, so a pattern written without the leading separator matches the
/// same requests as one written with it.
fn custom_rule_match(source: &str, path: &str) -> Option<Vec<String>> {
    let source = from_site_root(source);
    let path = from_site_root(path);
    let mut parts = source.split(CUSTOM_RULE_PLACEHOLDER);
    let leading = parts.next().unwrap_or_default();
    let literals: Vec<&str> = parts.collect();
    let mut remaining = path.strip_prefix(leading)?;
    if literals.is_empty() {
        return remaining.is_empty().then(Vec::new);
    }
    let mut captured = Vec::with_capacity(literals.len());
    for (index, literal) in literals.iter().enumerate() {
        if index + 1 == literals.len() {
            // The last literal is the pattern's tail, so what the final placeholder
            // captured is everything before it.
            let value = if literal.is_empty() {
                remaining
            } else {
                remaining.strip_suffix(literal)?
            };
            if value.is_empty() {
                return None;
            }
            captured.push(value.to_string());
        } else {
            let at = first_occurrence_after_a_capture(remaining, literal)?;
            captured.push(remaining[..at].to_string());
            remaining = &remaining[at + literal.len()..];
        }
    }
    Some(captured)
}

/// The index of the first occurrence of `literal` in `remaining` that leaves a non-empty
/// capture before it, so a placeholder always stands for something.
fn first_occurrence_after_a_capture(remaining: &str, literal: &str) -> Option<usize> {
    remaining
        .char_indices()
        .skip(1)
        .find(|(at, _)| remaining[*at..].starts_with(literal))
        .map(|(at, _)| at)
}

/// A rule's `target` with each [`CUSTOM_RULE_PLACEHOLDER`] replaced by what the source's
/// placeholders captured, in order, or `None` when the target carries more placeholders
/// than the source captured.
///
/// A target asking for a capture the match did not produce cannot be substituted, so the
/// rule decides nothing rather than being completed with a guess. A capture the target
/// does not use is simply unused: a target is as specific as it is written.
fn substituted_target(target: &str, captured: &[String]) -> Option<String> {
    let mut out = String::with_capacity(target.len());
    let mut values = captured.iter();
    let mut parts = target.split(CUSTOM_RULE_PLACEHOLDER).peekable();
    while let Some(part) = parts.next() {
        out.push_str(part);
        if parts.peek().is_some() {
            out.push_str(values.next()?);
        }
    }
    Some(out)
}

/// `value` as a path from the site root: exactly one leading separator, however it was
/// written.
fn from_site_root(value: &str) -> ::std::borrow::Cow<'_, str> {
    if value.starts_with('/') {
        ::std::borrow::Cow::Borrowed(value)
    } else {
        ::std::borrow::Cow::Owned(format!("/{value}"))
    }
}

/// This service's own endpoint prefix (`smithy_query(query="service_info")`:
/// endpointPrefix `amplify`), which its published endpoint host is built from.
pub const ENDPOINT_PREFIX: &str = "amplify";

/// The endpoint host this request reached the emulator on, so a URL this service hands
/// out resolves back to where the caller is already talking rather than naming a host
/// nothing serves.
///
/// The reading is the workspace's one
/// (`::service_common::data_plane_addressing::reached_endpoint_host`): the host the
/// request addressed — its `Host` header, or the authority of an absolute-form URI for a
/// request that carries its endpoint there — when it is shaped like a host, and this
/// service's published endpoint for the region it was signed for otherwise.
pub fn request_endpoint_host(parts: &::http::request::Parts, region: &str) -> String {
    ::service_common::data_plane_addressing::reached_endpoint_host(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        ENDPOINT_PREFIX,
        region,
    )
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the one
/// in its SigV4 credential scope, so that is what an ARN minted while serving it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Now, as epoch milliseconds.
///
/// The one clock this service reads, so a stored timestamp and a stored due instant
/// (see [`BranchJob::settles_at`]) are measured against the same thing.
pub fn now_millis() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Now, truncated to milliseconds.
///
/// restJson1 writes a timestamp as epoch seconds with a fractional part, i.e. as a
/// JSON number: an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond, so a nanosecond-precision instant would not survive the round trip
/// and `createTime` would differ between the create response and a later read.
/// Milliseconds round-trip exactly, which keeps a stored timestamp stable across reads.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(now_millis())
}

/// The page size to serve for a `@paginated` operation whose `maxResults` member is
/// `max_results` and whose modelled `range` maximum is `modelled_max`.
///
/// `maxResults` is modelled with `default: 0`, so an absent member arrives as `0`:
/// that is "the caller set no bound", which the operation serves with its modelled
/// maximum rather than with an empty page. A caller-supplied bound is already inside
/// the modelled range by the time a handler runs (the boundary validator refuses
/// anything else); clamping again costs nothing and keeps this total.
pub fn page_size(max_results: i32, modelled_max: usize) -> usize {
    match usize::try_from(max_results) {
        Ok(0) | Err(_) => modelled_max,
        Ok(bound) => bound.min(modelled_max),
    }
}

/// One page of an ordered `(key, record)` sequence, plus the `nextToken` to continue
/// from — `None` when the page reached the end.
///
/// The token is the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts
/// at the first key at or after the token. Resuming is therefore well defined even
/// when the record the token names was deleted between the two calls: the scan simply
/// continues at the next key.
pub fn page<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    // The keys of these collections are names and ids compared as text, so "before the
    // token" is text order.
    page_in_order(ordered, next_token, page_size, |key, token| key < token)
}

/// One page of a JOB history, which is ordered NEWEST JOB FIRST.
///
/// The same contract as [`page`] — the token is the key of the first record not returned,
/// and a resumed call starts at the first key at or after the token IN THE REPORTED
/// ORDER — read in that order instead of in text order, so a resumed call carries on
/// downwards through the history rather than restarting at its newest job. `ordered` is
/// the history already in reported order (see [`newest_job_first`]).
pub fn page_jobs<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    page_in_order(ordered, next_token, page_size, |key, token| {
        newest_job_first(key, token) == ::std::cmp::Ordering::Less
    })
}

/// The walk both pagers perform, over a sequence already in its reported order:
/// `before` is what "earlier in that order than the token" means for these keys.
fn page_in_order<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
    before: impl Fn(&str, &str) -> bool,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if next_token.is_some_and(|token| before(key.as_str(), token)) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(key));
        }
        records.push(record);
    }
    (records, None)
}

/// Which of two `jobId`s a job listing reports FIRST: the newer job, which is the one
/// whose id is numerically greater.
///
/// Every `jobId` is a decimal run this branch minted from its own counter
/// ([`AmplifyBranch::mint_job_id`]), so the greater id is the later job — and for digits
/// carrying no leading zero, the numerically greater id is the LONGER one, or the
/// lexicographically greater one among ids of equal length. Comparing the ids as plain
/// text instead would order job 10 before job 2.
///
/// The comparison reads the DIGITS rather than a parsed integer for a reason: one side of
/// it can be a `nextToken`, which is a `JobId` and so may carry up to 255 digits
/// (`smithy_query(query="shape",shape="JobSummary")`) — a decimal run no integer type
/// holds, which still has a well-defined place in this order.
pub fn newest_job_first(left: &str, right: &str) -> ::std::cmp::Ordering {
    right
        .len()
        .cmp(&left.len())
        .then_with(|| right.cmp(left))
}

/// Replace a stored @required member when the request supplied one.
///
/// A partial update is "every member the request carries, and nothing else": an
/// absent member is not a value, so it leaves the stored one as it was. Shared by
/// `UpdateApp` and `UpdateBranch` so the two cannot drift on that rule.
pub fn apply<T>(stored: &mut T, supplied: Option<T>) {
    if let Some(value) = supplied {
        *stored = value;
    }
}

/// Replace a stored OPTIONAL member when the request supplied one.
///
/// The same rule as [`apply`], for a member that may legitimately be unset: an
/// omitted member leaves the stored value alone rather than clearing it. Clearing a
/// member is therefore NOT expressible by omission — a caller that wants a member
/// emptied sends its empty value, which [`apply`] and this function store as given.
pub fn apply_optional<T>(stored: &mut Option<T>, supplied: Option<T>) {
    if supplied.is_some() {
        *stored = supplied;
    }
}

/// The `message` an Amplify `NotFoundException` carries for a resource the request
/// addressed and this store does not hold.
///
/// The text is built FROM THE REQUEST and is a complete sentence: the kind of entity,
/// the identifier the request used for it, and `not found` closed by the
/// sentence-terminating period. Every 404 in this crate is formatted through this one
/// function, so `GetApp`, `DeleteApp`, `GetBranch`, `DeleteBranch`, `CreateBranch`,
/// `ListBranches`, `UpdateApp`, `UpdateBranch`, the webhook operations, `StartJob` and
/// `GetJob` cannot drift from one another — and
/// an identifier is NEVER inspected here, so the message is the same construction
/// whatever the request named.
pub fn not_found_message(entity: &str, identifier: &str) -> String {
    format!("{entity} {identifier} not found.")
}

/// The `message` an Amplify `BadRequestException` carries when a create is refused
/// because the owning app ALREADY HOLDS a resource of that kind under the name the
/// request asked for.
///
/// Like [`not_found_message`], the text is built FROM THE REQUEST and from nothing
/// else: Amplify names the kind of resource it failed to create, then the name the
/// request used for it, then the app it already exists under —
/// `Failed to create <entity>. The <entity> <name> already exists for the app <appId>`,
/// with the entity in lower case, as recorded behaviour shows (the model does not
/// spell the message out). Every duplicate-name refusal in this crate is formatted
/// through this one function, so the creates cannot drift from one another, and an
/// identifier is NEVER inspected here: the message is the same construction whatever
/// the request named.
pub fn already_exists_message(entity: &str, identifier: &str, app_id: &str) -> String {
    let entity = entity.to_lowercase();
    format!(
        "Failed to create {entity}. The {entity} {identifier} already exists for the app {app_id}"
    )
}

/// The entity name [`not_found_message`] uses for an app addressed by `appId`.
pub const APP_ENTITY: &str = "App";

/// The entity name [`not_found_message`] and [`already_exists_message`] use for a
/// branch addressed by `branchName`.
pub const BRANCH_ENTITY: &str = "Branch";

/// The entity name [`not_found_message`] uses for a job addressed by `jobId`.
pub const JOB_ENTITY: &str = "Job";

/// The entity name [`not_found_message`] uses for a webhook addressed by `webhookId`.
pub const WEBHOOK_ENTITY: &str = "Webhook";

/// The entity name [`not_found_message`] and [`already_exists_message`] use for a
/// domain association addressed by `domainName`.
pub const DOMAIN_ENTITY: &str = "Domain";

/// The entity name [`not_found_message`] and [`already_exists_message`] use for a
/// backend environment addressed by `environmentName`.
pub const BACKEND_ENVIRONMENT_ENTITY: &str = "Backend environment";

/// The entity name [`not_found_message`] uses for a resource addressed by its ARN
/// alone — what the tagging operations take, where the ARN itself is the only
/// identifier the request carries and it names no resource this store holds.
pub const RESOURCE_ENTITY: &str = "Resource";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifyAmplifyState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifyAmplifyState>)
}
