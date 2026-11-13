// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dataexchange-dataexchange service, plus the
//! helpers every handler shares: identifier minting, ARN building, the opaque
//! pagination cursor the `@paginated` list operations hand out, and the upload a job's
//! minted URL names ([`parse_upload_path`], [`ComAmazonawsDataexchangeDataexchangeState::record_upload`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;
// `Digest` is the one `digest` trait both hashes implement, so it is imported once and
// carries `Sha256::digest` (a minted id) and `Md5::digest` (an uploaded payload's
// `Content-MD5`) alike.
use md5::Md5;
use sha2::{Digest, Sha256};

use ::aws_smithy_schema_server::{envelope, Violation, ViolationKind};

use crate::ops::types::{
    Action, ApiGatewayApiAsset, AssetConfiguration, AssetDetails, ConflictException, Event,
    InternalServerException, JobError, OriginDetails, ResourceNotFoundException, ResponseDetails,
    S3DataAccessAsset, S3SnapshotAsset, ServiceLimitExceededException, ValidationException,
};

/// The service's SigV4 signing name, and so the ARN service segment.
pub const SERVICE: &str = "dataexchange";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's generated shared config sets.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `Origin` for a data set this account created. One of the model's `Origin`
/// values, enumerated by `crate::ops::types::ORIGIN_VALUES` (the test suite pins
/// that this constant is still a member of that set, so the two cannot drift);
/// `ENTITLED`, the other, describes a subscriber's copy of someone else's data
/// set, which only a data grant or a Marketplace subscription mints.
pub const ORIGIN_OWNED: &str = "OWNED";

/// `AssetType` for an asset that IS a data object — one of the model's `AssetType`
/// values, enumerated by `crate::ops::types::ASSET_TYPE_VALUES` (the test suite pins
/// that this constant is still a member of that set, so the two cannot drift). Its
/// details are the object's own ([`AssetDetails::s3_snapshot_asset`]).
pub const ASSET_TYPE_S3_SNAPSHOT: &str = "S3_SNAPSHOT";

/// `AssetType` for an asset that IS an Amazon API Gateway API — another of the
/// model's `AssetType` values (see [`ASSET_TYPE_S3_SNAPSHOT`]). Its details are that
/// API's ([`AssetDetails::api_gateway_api_asset`]).
pub const ASSET_TYPE_API_GATEWAY_API: &str = "API_GATEWAY_API";

/// `AssetType` for an asset that IS an Amazon S3 data access — another of the model's
/// `AssetType` values (see [`ASSET_TYPE_S3_SNAPSHOT`]). Its details are the data
/// access's own ([`AssetDetails::s3_data_access_asset`]): the bucket whose data it
/// shares and the keys and key prefixes of that bucket it shares.
pub const ASSET_TYPE_S3_DATA_ACCESS: &str = "S3_DATA_ACCESS";

/// `Origin` for a subscriber's copy of someone else's data set — the other of the
/// model's `Origin` values (see [`ORIGIN_OWNED`]), which a data grant or an AWS
/// Marketplace subscription mints. An event action watches a data set this account
/// subscribes to, so this is the only `Origin` one can be created for.
pub const ORIGIN_ENTITLED: &str = "ENTITLED";

/// The modelled maximum of `MaxResults` (`@range(min: 1, max: 200)` on
/// `com.amazonaws.dataexchange#MaxResults`), which is also the page bound when the
/// caller names no smaller one.
pub const MAX_PAGE_SIZE: i32 = 200;

/// The modelled minimum of `MaxResults`, from the same `@range`.
pub const MIN_PAGE_SIZE: i32 = 1;

/// The query parameter every `MaxResults` member of this service is bound to
/// (`smithy_query(query="operation", operation="ListDataSets")` reports
/// `"httpQuery": "maxResults"` on it), and so also the member path a refusal of it
/// states — the boundary walk spells a member path in lowerCamel.
pub const MAX_RESULTS_QUERY: &str = "maxResults";

/// The query parameter the `Origin` member of `ListDataSets` is bound to
/// (`smithy_query(query="operation", operation="ListDataSets")` reports
/// `"httpQuery": "origin"`).
pub const ORIGIN_QUERY: &str = "origin";

/// The model's `Origin` values IN THE ORDER THE MODEL DECLARES THEM
/// (`smithy_query(query="shape", shape="Origin")` -> `["OWNED", "ENTITLED"]`).
///
/// `crate::ops::types::ORIGIN_VALUES` is the generated catalog and is the set this
/// service CHECKS against; it is sorted, so it cannot state the set in the model's
/// own order, which is the order a refusal quotes it in. The test suite pins that
/// the two hold the same values, so this cannot drift from the model.
pub const ORIGIN_VALUES_IN_MODEL_ORDER: &[&str] = &["OWNED", "ENTITLED"];

/// One stored data set. `revisions` holds the revision records; `revision_order`
/// is their creation order, so a list page is stable across calls.
#[derive(Debug, Clone)]
pub struct DataSet {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub asset_type: String,
    pub origin: String,
    pub origin_details: Option<OriginDetails>,
    pub source_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub revisions: HashMap<String, Revision>,
    pub revision_order: Vec<String>,
    /// The notifications this data set has accepted, oldest first; see
    /// [`SentNotification`].
    pub notifications: Vec<SentNotification>,
}

/// One notification `SendDataSetNotification` accepted for a data set.
///
/// AWS delivers a notification to the subscribers of the data set's AWS Marketplace
/// product and models no operation that reads one back, so this record is what the
/// acceptance left behind: WHAT kind of event was notified, the provider's comment,
/// when it was accepted, and the idempotency token it was accepted under, which is
/// what lets a repeat under the same `ClientToken` be the same acceptance rather
/// than a second notification.
///
/// The notification's `Details` and `Scope` are deliberately NOT kept: they are
/// caller-sized nested collections the model bounds only in prose, nothing reads
/// them back, and the store must stay finite.
#[derive(Debug, Clone)]
pub struct SentNotification {
    /// One of the model's `NotificationType` values, enumerated by
    /// `crate::ops::types::NOTIFICATION_TYPE_VALUES`.
    pub r#type: String,
    /// The provider's free-form comment, bounded by the model's own
    /// `@length(0..4096)` on `__stringMin0Max4096`.
    pub comment: Option<String>,
    /// The caller's `ClientToken`, bounded by the model's own `@length(1..64)`.
    pub client_token: Option<String>,
    pub accepted_at: DateTime,
}

/// One asset stored in a revision: the file, table or API the revision publishes.
///
/// An asset is never created directly — it is what an import job puts into a
/// revision (`StartJob`), and what `GetAsset` / `ListRevisionAssets` report — so the
/// record carries the members those readers answer with, in the form they answer
/// them in.
#[derive(Debug, Clone)]
pub struct Asset {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// One of the model's `AssetType` values, enumerated by
    /// `crate::ops::types::ASSET_TYPE_VALUES`; a revision's assets all carry the
    /// `AssetType` of the data set they belong to.
    pub asset_type: String,
    /// The CONTENT of the asset: the bytes the import that created it read, which for
    /// an import from an upload URL are the bytes a client uploaded to it. Empty for
    /// an asset this emulator holds no bytes for.
    pub content: Vec<u8>,
    /// The API this asset fulfils, as the import that created it recorded it — set
    /// only for an asset imported from an Amazon API Gateway API, whose content is
    /// that API's specification rather than a data file.
    ///
    /// The members a READER mints per call (the API's endpoint, and the download URL
    /// of its specification, which names the endpoint the read arrived at) are left
    /// absent here: they are not part of what was imported.
    pub api: Option<ApiGatewayApiAsset>,
    /// The Amazon S3 data access this asset IS, as the job that created it recorded
    /// it — set only for an asset whose content is a view of a bucket the caller owns
    /// rather than bytes this store holds.
    ///
    /// The access-point members of the modelled `S3DataAccessAsset` are left absent:
    /// this emulator mints no Amazon S3 access point, so there is no alias or ARN of
    /// one to report, and a made-up one would name an endpoint nothing serves.
    pub s3_data_access: Option<S3DataAccessAsset>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

impl Asset {
    /// The modelled `AssetDetails` of this asset: the ONE detail member its
    /// `AssetType` names, filled from what the import that created it recorded.
    ///
    /// The model pairs each `AssetType` with the `AssetDetails` member that describes
    /// an asset of that type (`smithy_query(query="shape",
    /// shape="AssetType,AssetDetails")`), so the type decides which member an asset's
    /// details carry: a data object is described by its size, and an API asset by the
    /// API it fulfils. A type whose source this emulator cannot read \u2014 so no import
    /// of it ever completes, and no asset of it is ever stored \u2014 has nothing recorded
    /// to describe, and its details are empty rather than invented.
    pub fn details(&self) -> AssetDetails {
        match self.asset_type.as_str() {
            ASSET_TYPE_S3_SNAPSHOT => AssetDetails {
                s3_snapshot_asset: Some(S3SnapshotAsset {
                    // The size of the object the asset is, which is how many bytes of
                    // it this store holds.
                    size: self.content.len() as f64,
                }),
                ..AssetDetails::default()
            },
            ASSET_TYPE_API_GATEWAY_API => AssetDetails {
                api_gateway_api_asset: self.api.clone(),
                ..AssetDetails::default()
            },
            ASSET_TYPE_S3_DATA_ACCESS => AssetDetails {
                s3_data_access_asset: self.s3_data_access.clone(),
                ..AssetDetails::default()
            },
            _ => AssetDetails::default(),
        }
    }
}

/// One stored revision of a data set.
#[derive(Debug, Clone)]
pub struct Revision {
    pub id: String,
    pub arn: String,
    pub data_set_id: String,
    pub comment: Option<String>,
    pub finalized: bool,
    /// The assets this revision holds, in the order they were imported. A revision
    /// is created empty: assets arrive through an import job.
    pub assets: Vec<Asset>,
    pub revoked: bool,
    pub revocation_comment: Option<String>,
    pub revoked_at: Option<DateTime>,
    pub source_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

/// One stored job: what the caller asked for, in the response form every job
/// operation reports it in, plus where it is in its lifecycle.
///
/// `details` is kept as the modelled `ResponseDetails` rather than as the request
/// form, because that is what `CreateJob`, `GetJob` and `ListJobs` all answer with;
/// keeping one representation is what makes those three agree. `data_set_id` and
/// `revision_id` are lifted out of those details so a reader can filter on them
/// without re-matching on which detail member the job carries.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub arn: String,
    /// One of the model's `Type` values, enumerated by
    /// `crate::ops::types::TYPE_VALUES`.
    pub r#type: String,
    /// One of the model's `State` values, enumerated by
    /// `crate::ops::types::STATE_VALUES`.
    pub state: String,
    pub asset_configuration: Option<AssetConfiguration>,
    pub details: ResponseDetails,
    /// The errors the job collected while it ran; empty until it runs.
    pub errors: Vec<JobError>,
    /// The data set the job's details name (`ListJobs` filters on it).
    pub data_set_id: String,
    /// The revision the job's details name, where the job type names exactly one.
    pub revision_id: Option<String>,
    /// Where the execution a start call set going will leave this job, and when the
    /// clock owes it. `None` on a job nothing has started and on one that has already
    /// reached a terminal `State`.
    pub pending_settlement: Option<JobSettlement>,
    /// The upload the URL this job's creation handed out names, for a job that takes
    /// its content from one; `None` for a job that minted no URL.
    pub upload: Option<JobUpload>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

/// The upload the URL `CreateJob` minted for a job names: which of this service's
/// published upload paths it is, until when it accepts the bytes, the digest the
/// request declared for them, and the bytes a client has actually uploaded.
///
/// A URL a service hands out as an upload target is a promise that whoever holds it
/// can WRITE there, so the record exists from the moment the URL is announced: it is
/// what the data plane serving that path resolves the PUT against, and what the job's
/// own execution then reads its content from.
#[derive(Debug, Clone)]
pub struct JobUpload {
    /// The last segment of the published path, which is also WHICH content of the job
    /// this upload carries: one of [`UPLOAD_LEAF_ASSET`] /
    /// [`UPLOAD_LEAF_API_SPECIFICATION`].
    pub leaf: &'static str,
    /// The instant the announced URL stops accepting bytes — the expiry the create
    /// call published alongside it.
    pub expires_at: DateTime,
    /// The Base64 MD5 the request declared for the content, which the model makes a
    /// required member of both details that name an upload. It is what the uploaded
    /// bytes are checked against.
    pub md5_hash: String,
    /// The bytes a client has uploaded, once it has. Taken out of here by the
    /// execution that imports them, so one upload is imported exactly once.
    pub content: Option<Vec<u8>>,
}

/// The terminal state a started job's execution reaches, and the instant it comes
/// due.
///
/// What the execution of a job does is fixed by the job's own `Type` and `Details`,
/// so the outcome is resolved once, by the call that starts the job, and kept here
/// until the clock makes it due. Nothing sleeps: the due instant is stored, and
/// [`Job::settle`] materialises the outcome on the first call at or after it.
#[derive(Debug, Clone)]
pub struct JobSettlement {
    /// When the execution finishes, so when the terminal `State` below is owed.
    pub due_at: DateTime,
    /// The terminal `State` the execution reaches: one of the model's `State`
    /// values, enumerated by `crate::ops::types::STATE_VALUES`.
    pub state: String,
    /// What the execution reports about why it ended there — the `Errors` list a
    /// reader answers with, empty for an execution that carried its work out.
    pub errors: Vec<JobError>,
    /// The asset the execution puts into the revision it names when it finishes, for
    /// an import that has content to read; `None` for an execution that adds none.
    pub import: Option<PendingImport>,
}

/// The asset an execution adds to the revision its details name — the one an import
/// produces, and the one a published Amazon S3 data access IS.
///
/// What an execution produces is fixed by the job's own `Details`, so it is resolved
/// once, by the call that starts the job, and kept until the clock makes the execution
/// due; the bytes stay in the job's [`JobUpload`] until then rather than being copied
/// here.
#[derive(Debug, Clone)]
pub struct PendingImport {
    /// The Region the asset's ARN carries: the one the call that started the job was
    /// signed for.
    pub region: String,
    pub data_set_id: String,
    pub revision_id: String,
    /// The `Name` the asset carries — the name the job's details give the content.
    pub asset_name: String,
    /// The API the asset fulfils, for an import of an API specification; `None` for
    /// an import of a data file. See [`Asset::api`].
    pub api: Option<ApiGatewayApiAsset>,
    /// The Amazon S3 data access the asset IS, for a job that publishes one; `None`
    /// for a job that produces an asset of any other kind. See
    /// [`Asset::s3_data_access`].
    pub s3_data_access: Option<S3DataAccessAsset>,
}

/// An import whose execution has just finished and whose revision has yet to receive
/// the asset — what [`Job::settle`] hands back so the store can place it.
#[derive(Debug)]
pub struct DueImport {
    /// What the asset is, as the start call resolved it.
    pub plan: PendingImport,
    /// The content the asset carries: the bytes the client uploaded, moved out of the
    /// job's upload.
    pub content: Vec<u8>,
    /// The instant the execution came due, which is what the asset was created at.
    pub at: DateTime,
}

impl Job {
    /// Materialise the terminal state this job's execution was already owed at `now`,
    /// handing back the import it finished, for the store to place in its revision.
    ///
    /// This is the ONE write a reporting operation may make: the transition was due
    /// at the stored instant whether or not anybody called, so `updated_at` moves to
    /// that instant rather than to `now`.
    pub fn settle(&mut self, now: DateTime) -> Option<DueImport> {
        let due = self
            .pending_settlement
            .as_ref()
            .is_some_and(|settlement| settlement.due_at.as_secs_f64() <= now.as_secs_f64());
        if !due {
            return None;
        }
        let settlement = self.pending_settlement.take()?;
        self.state = settlement.state;
        self.errors = settlement.errors;
        if settlement.due_at.as_secs_f64() > self.updated_at.as_secs_f64() {
            self.updated_at = settlement.due_at;
        }
        let plan = settlement.import?;
        // The staged bytes BECOME the asset's content, so they are moved out of the
        // upload: a job imports one upload exactly once.
        let content = self
            .upload
            .as_mut()
            .and_then(|upload| upload.content.take())
            .unwrap_or_default();
        Some(DueImport {
            plan,
            content,
            at: settlement.due_at,
        })
    }

    /// Whether this job is still waiting to be started — the one `State` from which a
    /// start or a cancel is admitted, as both operations' documentation says.
    pub fn is_waiting(&self) -> bool {
        self.state == JOB_STATE_WAITING
    }
}

/// One stored event action: which event fires it, what it does when it fires, and
/// the identity and tags it carries.
///
/// `event_source_id` is lifted out of `event` so a listing can filter on the source
/// without re-matching on which event member the action watches.
#[derive(Debug, Clone)]
pub struct EventAction {
    pub id: String,
    pub arn: String,
    pub event: Event,
    pub action: Action,
    /// The id of the resource whose events fire this action — the data set a
    /// `RevisionPublished` event names.
    pub event_source_id: String,
    pub tags: HashMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

/// One stored data grant: who granted what to whom, and where the grant is in its
/// acceptance lifecycle.
///
/// `accepted_at` is the instant the receiver accepted, so it is absent for as long as
/// `acceptance_state` is [`DATA_GRANT_PENDING_RECEIVER_ACCEPTANCE`] — the two are one
/// fact, written together by the accept.
#[derive(Debug, Clone)]
pub struct DataGrant {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The account that created the grant: the caller of the create.
    pub sender_principal: String,
    /// The account the grant is for, and so the only one that can accept it.
    pub receiver_principal: String,
    /// One of the model's `DataGrantAcceptanceState` values, enumerated by
    /// `crate::ops::types::DATA_GRANT_ACCEPTANCE_STATE_VALUES`.
    pub acceptance_state: String,
    pub accepted_at: Option<DateTime>,
    pub ends_at: Option<DateTime>,
    /// One of the model's `GrantDistributionScope` values, enumerated by
    /// `crate::ops::types::GRANT_DISTRIBUTION_SCOPE_VALUES`.
    pub grant_distribution_scope: String,
    /// The data set the grant gives access to. This emulator mints no second,
    /// entitled copy of the data set the grant was created from, so the data set
    /// associated to the grant IS that source data set and both members report it;
    /// `source_data_set_id` is kept beside it because the two are distinct members of
    /// every data-grant response.
    pub data_set_id: String,
    pub source_data_set_id: String,
    pub tags: HashMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

/// Per-service state for com-amazonaws-dataexchange-dataexchange. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDataexchangeDataexchangeState {
    /// Every data set this emulator holds, keyed by its `DataSetId`.
    pub data_sets: HashMap<String, DataSet>,
    /// Creation order of `data_sets`, so `ListDataSets` pages deterministically.
    pub data_set_order: Vec<String>,
    /// Every job this emulator holds, keyed by its `JobId`.
    pub jobs: HashMap<String, Job>,
    /// Creation order of `jobs`, so `ListJobs` pages deterministically.
    pub job_order: Vec<String>,
    /// Every event action this emulator holds, keyed by its `EventActionId`.
    pub event_actions: HashMap<String, EventAction>,
    /// Creation order of `event_actions`, so `ListEventActions` pages
    /// deterministically.
    pub event_action_order: Vec<String>,
    /// Every data grant this emulator holds, keyed by its `Id`.
    pub data_grants: HashMap<String, DataGrant>,
    /// Creation order of `data_grants`, so `ListDataGrants` pages deterministically.
    pub data_grant_order: Vec<String>,
}

impl ComAmazonawsDataexchangeDataexchangeState {
    /// Materialise the terminal state every started job was already owed at `now`,
    /// and place the asset each import that finished produced.
    ///
    /// Called at the top of each job operation, the reporting ones included: a job
    /// that was started finishes at an instant the start call fixed, so the move out
    /// of `IN_PROGRESS` is owed at that instant whether or not anybody polls. The
    /// scan is bounded by the store, which holds at most [`MAX_JOBS`] jobs.
    pub fn settle_jobs(&mut self, now: DateTime) {
        // The jobs are settled first and their imports placed after, because placing
        // one writes into a data set while the jobs are being walked.
        let mut due: Vec<(String, DueImport)> = Vec::new();
        for job in self.jobs.values_mut() {
            if let Some(import) = job.settle(now) {
                due.push((job.id.clone(), import));
            }
        }
        for (job_id, import) in due {
            if let Some(error) = self.receive_imported_asset(import) {
                // The execution finished, but the revision it was importing into no
                // longer admits the asset, so that — and not a completion — is what
                // this job ended in.
                if let Some(job) = self.jobs.get_mut(&job_id) {
                    job.state = JOB_STATE_ERROR.to_string();
                    job.errors = vec![error];
                }
            }
        }
    }

    /// Put the asset an import produced into the revision the job's details named,
    /// answering the job error that replaces the completion when it cannot be placed.
    ///
    /// The target is judged at the instant the execution came due rather than at the
    /// start: a data set or revision that has gone since cannot receive an asset, and
    /// neither can one that was finalized while the import ran — the model's
    /// `Finalized` documentation calls that "this read-only state".
    fn receive_imported_asset(&mut self, import: DueImport) -> Option<JobError> {
        let plan = &import.plan;
        let Some(data_set) = self.data_sets.get_mut(&plan.data_set_id) else {
            return Some(import_target_not_found(
                &plan.data_set_id,
                JOB_ERROR_RESOURCE_TYPE_DATA_SET,
            ));
        };
        // Every asset of a revision carries the `AssetType` of the data set it belongs
        // to, which is where that type is recorded.
        let asset_type = data_set.asset_type.clone();
        let Some(revision) = data_set.revisions.get_mut(&plan.revision_id) else {
            return Some(import_target_not_found(
                &plan.revision_id,
                JOB_ERROR_RESOURCE_TYPE_REVISION,
            ));
        };
        if revision.finalized {
            return Some(JobError {
                code: JOB_ERROR_CODE_VALIDATION.to_string(),
                message: revision_is_finalized_message(&plan.revision_id),
                resource_id: Some(plan.revision_id.clone()),
                resource_type: Some(JOB_ERROR_RESOURCE_TYPE_REVISION.to_string()),
                ..JobError::default()
            });
        }
        let asset_id = mint_id();
        revision.assets.push(Asset {
            arn: asset_arn(
                &plan.region,
                &plan.data_set_id,
                &plan.revision_id,
                &asset_id,
            ),
            id: asset_id,
            name: plan.asset_name.clone(),
            asset_type,
            content: import.content,
            api: plan.api.clone(),
            s3_data_access: plan.s3_data_access.clone(),
            created_at: import.at,
            updated_at: import.at,
        });
        // The revision changed when the asset landed in it, which was the instant the
        // execution came due.
        if import.at.as_secs_f64() > revision.updated_at.as_secs_f64() {
            revision.updated_at = import.at;
        }
        None
    }

    /// Record the bytes one PUT to a published upload URL carried.
    ///
    /// `digest` is the Base64 MD5 of `body`, computed by the caller ([`content_md5`]).
    /// The upload the path addresses decides the answer, in this order: a path naming
    /// no job of this emulator, or a job that published no upload at that path, is no
    /// upload at all; a window that has closed is an expired request, which is a
    /// different answer from one that never existed; bytes whose digest is not the one
    /// the job declared are refused, so the content a job imports is the content its
    /// creation described; and the store keeps what it retains bounded.
    ///
    /// A PUT is idempotent: a second one to the same URL replaces the bytes of the
    /// first, and only the bytes that end up retained count against the ceiling.
    pub fn record_upload(
        &mut self,
        target: &UploadTarget,
        digest: &str,
        body: Vec<u8>,
        now: DateTime,
    ) -> UploadOutcome {
        let retained = self.retained_content_bytes();
        let Some(job) = self.jobs.get_mut(&target.job_id) else {
            return UploadOutcome::NoSuchUpload;
        };
        let Some(upload) = job
            .upload
            .as_mut()
            .filter(|upload| upload.leaf == target.leaf)
        else {
            return UploadOutcome::NoSuchUpload;
        };
        if upload.expires_at.as_secs_f64() <= now.as_secs_f64() {
            return UploadOutcome::Expired;
        }
        if digest != upload.md5_hash {
            return UploadOutcome::DigestMismatch;
        }
        let replaced = upload.content.as_ref().map_or(0, Vec::len);
        if retained.saturating_sub(replaced) + body.len() > MAX_CONTENT_BYTES {
            return UploadOutcome::StoreFull;
        }
        upload.content = Some(body);
        UploadOutcome::Stored
    }

    /// How many content bytes this store is holding: every upload staged against a job
    /// plus every imported asset's own content.
    ///
    /// The scan is bounded by the store — at most [`MAX_JOBS`] jobs, and an asset
    /// arrives only through a job's one import — and is what keeps
    /// [`MAX_CONTENT_BYTES`] a ceiling on the whole store rather than on one upload.
    fn retained_content_bytes(&self) -> usize {
        let staged: usize = self
            .jobs
            .values()
            .map(|job| {
                job.upload
                    .as_ref()
                    .and_then(|upload| upload.content.as_ref())
                    .map_or(0, Vec::len)
            })
            .sum();
        let imported: usize = self
            .data_sets
            .values()
            .flat_map(|data_set| data_set.revisions.values())
            .flat_map(|revision| revision.assets.iter())
            .map(|asset| asset.content.len())
            .sum();
        staged + imported
    }

    /// The tags stored on the resource the tagging `ResourceArn` addresses, or `None`
    /// when this emulator holds no resource with that ARN.
    ///
    /// The target is located by the ARN each resource was MINTED with
    /// ([`data_set_arn`], [`revision_arn`], [`event_action_arn`]), so the whole ARN —
    /// partition, vendor,
    /// region, account and resource path — is the identity being compared, and this
    /// crate holds no second account of what an ARN looks like: an ARN naming the
    /// same ids under another partition, region or account addresses nothing,
    /// because those fields are part of a resource's identity.
    ///
    /// The scan is bounded by the store: at most [`MAX_EVENT_ACTIONS`] event actions
    /// and at most [`MAX_DATA_SETS`] data sets, each holding at most
    /// [`MAX_REVISIONS_PER_DATA_SET`] revisions.
    ///
    /// A data set, its revisions, the event actions and the data grants this emulator
    /// stores are the taggable resources it holds, which is every resource of this
    /// service whose create carries `Tags`.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&HashMap<String, String>> {
        if let Some(event_action) = self
            .event_actions
            .values()
            .find(|event_action| event_action.arn == resource_arn)
        {
            return Some(&event_action.tags);
        }
        if let Some(data_grant) = self
            .data_grants
            .values()
            .find(|data_grant| data_grant.arn == resource_arn)
        {
            return Some(&data_grant.tags);
        }
        self.data_sets.values().find_map(|data_set| {
            if data_set.arn == resource_arn {
                return Some(&data_set.tags);
            }
            data_set
                .revisions
                .values()
                .find(|revision| revision.arn == resource_arn)
                .map(|revision| &revision.tags)
        })
    }

    /// Whether any data grant this emulator holds gives access to `data_set_id`.
    ///
    /// A revision can only be revoked while the data it belongs to is being shared —
    /// included in a data product or a data grant — and a data grant is the one such
    /// inclusion this service models, so this is what that precondition reads.
    ///
    /// The scan is bounded by the store, which holds at most [`MAX_DATA_GRANTS`]
    /// grants.
    pub fn data_set_is_granted(&self, data_set_id: &str) -> bool {
        self.data_grants.values().any(|data_grant| {
            data_grant.data_set_id == data_set_id || data_grant.source_data_set_id == data_set_id
        })
    }

    /// The same lookup as [`Self::resource_tags`], for a tag write.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut HashMap<String, String>> {
        if let Some(event_action) = self
            .event_actions
            .values_mut()
            .find(|event_action| event_action.arn == resource_arn)
        {
            return Some(&mut event_action.tags);
        }
        if let Some(data_grant) = self
            .data_grants
            .values_mut()
            .find(|data_grant| data_grant.arn == resource_arn)
        {
            return Some(&mut data_grant.tags);
        }
        self.data_sets.values_mut().find_map(|data_set| {
            if data_set.arn == resource_arn {
                return Some(&mut data_set.tags);
            }
            data_set
                .revisions
                .values_mut()
                .find(|revision| revision.arn == resource_arn)
                .map(|revision| &mut revision.tags)
        })
    }
}

/// What a value must start with to be an Amazon Resource Name.
///
/// This is the whole of the ARN grammar this crate states. WHICH resource a tagging
/// `ResourceArn` addresses is decided by comparing it with the ARNs this service
/// minted ([`ComAmazonawsDataexchangeDataexchangeState::resource_tags`]), so the ARN
/// a resource carries is read in exactly one place — the builders
/// [`data_set_arn`] / [`revision_arn`] — and no second parse can disagree with them.
const ARN_PREFIX: &str = "arn:";

/// Whether `value` is an Amazon Resource Name at all, as opposed to an identifier, a
/// name or any other text a caller may have sent in a `ResourceArn`.
///
/// A value that is not one is a bad REQUEST (the member cannot name a resource of any
/// service), which is a different answer from an ARN that addresses no resource this
/// emulator holds — that one is the modelled 404 ([`tagged_resource_not_found`]).
pub fn is_resource_arn(value: &str) -> bool {
    value.starts_with(ARN_PREFIX)
}

/// The 400 for a `ResourceArn` that is not an Amazon Resource Name, naming the member
/// and what it has to carry so the caller can fix the request from the text alone.
pub fn resource_arn_not_an_arn(value: &str) -> ValidationException {
    invalid_request(format!(
        "ResourceArn must be an Amazon Resource Name of the form \
         arn:<partition>:{SERVICE}:<region>:<account>:<resource>, got {value:?}"
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDataexchangeDataexchangeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDataexchangeDataexchangeState>)
}

/// Monotonic sequence folded into every minted identifier, so two ids minted in
/// the same nanosecond still differ.
static MINT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// `ResourceType` for a data set, from the model's `ResourceType` enum
/// (`ASSET | DATA_GRANT | DATA_SET | EVENT_ACTION | JOB | REVISION`).
pub const RESOURCE_TYPE_DATA_SET: &str = "DATA_SET";

/// `ResourceType` for a revision, from the same `ResourceType` enum.
pub const RESOURCE_TYPE_REVISION: &str = "REVISION";

/// `ResourceType` for an asset, from the same `ResourceType` enum.
pub const RESOURCE_TYPE_ASSET: &str = "ASSET";

/// `ResourceType` for a job, from the same `ResourceType` enum.
pub const RESOURCE_TYPE_JOB: &str = "JOB";

/// `ResourceType` for an event action, from the same `ResourceType` enum.
pub const RESOURCE_TYPE_EVENT_ACTION: &str = "EVENT_ACTION";

/// `ResourceType` for a data grant, from the same `ResourceType` enum.
pub const RESOURCE_TYPE_DATA_GRANT: &str = "DATA_GRANT";

/// The `AcceptanceState` a data grant carries until its receiver accepts it: one of
/// the model's `DataGrantAcceptanceState` values, enumerated by
/// `crate::ops::types::DATA_GRANT_ACCEPTANCE_STATE_VALUES`. A created grant is in it,
/// because only the receiver's own call can move it on.
pub const DATA_GRANT_PENDING_RECEIVER_ACCEPTANCE: &str = "PENDING_RECEIVER_ACCEPTANCE";

/// The `AcceptanceState` of a data grant its receiver has accepted, from the same
/// `DataGrantAcceptanceState` value set.
pub const DATA_GRANT_ACCEPTED: &str = "ACCEPTED";

/// The `State` a job is in before anything has started it: one of the model's
/// `State` values, enumerated by `crate::ops::types::STATE_VALUES`. A job does no
/// work until `StartJob` moves it on, so this is the state a created job carries.
pub const JOB_STATE_WAITING: &str = "WAITING";

/// The `State` a job carries while its execution runs: the one non-terminal state a
/// started job passes through, from the same `State` value set.
pub const JOB_STATE_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal `State` of an execution that could not carry its work out, from the
/// same `State` value set. A job in it reports why in its `Errors`.
pub const JOB_STATE_ERROR: &str = "ERROR";

/// The terminal `State` of a job a caller cancelled before it was started, from the
/// same `State` value set.
pub const JOB_STATE_CANCELLED: &str = "CANCELLED";

/// The terminal `State` of an execution that carried its work out, from the same
/// `State` value set. A job in it reports no `Errors`.
pub const JOB_STATE_COMPLETED: &str = "COMPLETED";

/// The `Code` of the job error an execution reports when a resource it had to work on
/// is not there: one of the model's `Code` values, enumerated by
/// `crate::ops::types::CODE_VALUES`.
pub const JOB_ERROR_CODE_RESOURCE_NOT_FOUND: &str = "RESOURCE_NOT_FOUND_EXCEPTION";

/// The `Code` of the job error an execution reports when the work it was to do does
/// not hold against the state of the resource it names, from the same `Code` value set.
pub const JOB_ERROR_CODE_VALIDATION: &str = "VALIDATION_EXCEPTION";

/// `ResourceType` for a revision ON A JOB ERROR: a value of the model's
/// `JobErrorResourceTypes` enum (`REVISION | ASSET | DATA_SET`), which is a different
/// member — and a different value set — from the `ResourceType` the modelled
/// exceptions carry, even where the two spell a kind the same way.
pub const JOB_ERROR_RESOURCE_TYPE_REVISION: &str = "REVISION";

/// `ResourceType` for a data set on a job error, from the same
/// `JobErrorResourceTypes` value set.
pub const JOB_ERROR_RESOURCE_TYPE_DATA_SET: &str = "DATA_SET";

/// The job error an execution reports when the resource it was importing into is not
/// there any more: the same not-found sentence this service's modelled 404 carries
/// ([`RESOURCE_NOT_FOUND_MESSAGE`]), with the identity in the error's own members.
///
/// `resource_type` is a value of the model's `JobErrorResourceTypes` enum
/// ([`JOB_ERROR_RESOURCE_TYPE_DATA_SET`] / [`JOB_ERROR_RESOURCE_TYPE_REVISION`]).
fn import_target_not_found(resource_id: &str, resource_type: &str) -> JobError {
    JobError {
        code: JOB_ERROR_CODE_RESOURCE_NOT_FOUND.to_string(),
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
        ..JobError::default()
    }
}

/// Why a write into `revision_id` cannot hold: the revision is in the state the
/// model's `Finalized` documentation calls "this read-only state".
///
/// ONE sentence for every refusal of such a write, so the modelled 409 a request is
/// refused with and the job error an execution reports state the same thing.
pub fn revision_is_finalized_message(revision_id: &str) -> String {
    format!("The revision {revision_id} is finalized and can't be modified.")
}

/// Why nothing more can be done to a revision whose subscribers' access has been
/// revoked.
///
/// A revision is revocable only while it is finalized — the state the model's
/// `Finalized` documentation calls read-only — so a revoked revision is in that state
/// for good, and every later call that would change it is refused with this sentence.
/// It is entirely fixed text: the identity of the revision travels in the exception's
/// own `ResourceId`/`ResourceType` members, as it does in every other refusal this
/// service composes.
pub const REVISION_ACTION_REFUSED_MESSAGE: &str = "Cannot take action on a Finalized Revision.";

/// Why a revision nothing is sharing cannot be revoked: revoking withdraws
/// subscribers' access, so there has to be access to withdraw — the revision's data
/// has to be included in a data product or a data grant. Fixed text, for the reason
/// [`REVISION_ACTION_REFUSED_MESSAGE`] states.
pub const REVISION_NOT_SHARED_MESSAGE: &str =
    "Can only revoke a Revision that is included in a data product or data grant.";

/// How long a started job's execution runs before the clock owes it a terminal
/// `State`, in seconds.
///
/// The model publishes no duration for a job's execution, so the span is this
/// emulator's choice. It is deliberately non-zero: a caller that reads the job
/// straight after starting it sees `IN_PROGRESS`, as it does against the real
/// service, and the state errors of a running job still apply to it.
pub const JOB_EXECUTION_SECONDS: f64 = 3.0;

/// `instant` advanced by `seconds`, as a modelled `Timestamp`.
pub fn later(instant: DateTime, seconds: f64) -> DateTime {
    DateTime::from_secs_f64(instant.as_secs_f64() + seconds)
}

/// The path space every upload URL this service mints is served at:
/// `/v1/jobs/{JobId}/{leaf}`.
///
/// It is the `/v1/jobs` space the modelled job operations live in, one segment deeper
/// than any of them (`/v1/jobs`, `/v1/jobs/{JobId}`), so a published upload path names
/// no modelled operation and collides with none.
const UPLOAD_PATH_ROOT: &str = "/v1/jobs/";

/// The leaf of the URL a job's ASSET content is uploaded to — the target
/// `ImportAssetFromSignedUrlResponseDetails.SignedUrl` announces.
pub const UPLOAD_LEAF_ASSET: &str = "asset";

/// The leaf of the URL a job's API SPECIFICATION is uploaded to — the target
/// `ImportAssetFromApiGatewayApiResponseDetails.ApiSpecificationUploadUrl` announces.
pub const UPLOAD_LEAF_API_SPECIFICATION: &str = "api-specification";

/// Every upload leaf this service publishes, which is the whole of the path space the
/// upload plane claims.
const UPLOAD_LEAVES: &[&str] = &[UPLOAD_LEAF_ASSET, UPLOAD_LEAF_API_SPECIFICATION];

/// The URL a client uploads the content a job will import to, or `None` when the
/// request named no endpoint to serve it at.
///
/// AWS answers an Amazon S3 presigned URL here. This emulator has no object store, so
/// the URL names THIS job's upload on the endpoint THIS REQUEST ARRIVED AT, under the
/// path space [`parse_upload_path`] reads back and the upload plane serves: a client
/// that follows it writes the bytes into this emulator, and the job's own execution
/// reads them from there. The two sides are built and read here so they cannot drift.
///
/// When the request names no host there is no endpoint to point at, so `None` comes
/// back — a baked-in AWS endpoint here would send a following client's bytes to the
/// real service.
pub fn upload_url(endpoint: Option<&str>, job_id: &str, leaf: &str) -> Option<String> {
    endpoint.map(|host| format!("https://{host}{UPLOAD_PATH_ROOT}{job_id}/{leaf}"))
}

/// Which upload a request's path addresses, or `None` for a path that is not one of
/// the upload URLs this service mints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTarget {
    /// The job whose upload this is.
    pub job_id: String,
    /// Which of the job's uploads, as [`JobUpload::leaf`] records it.
    pub leaf: &'static str,
}

/// Read a request path as one of the upload URLs [`upload_url`] mints.
///
/// The job id has to be a modelled `Id` and the leaf one this service publishes, so a
/// path a client assembled itself addresses no upload — and the leaf handed back is
/// this crate's own constant, never the caller's text.
pub fn parse_upload_path(path: &str) -> Option<UploadTarget> {
    let (job_id, leaf) = path.strip_prefix(UPLOAD_PATH_ROOT)?.split_once('/')?;
    if !is_modelled_id(job_id) {
        return None;
    }
    let leaf = UPLOAD_LEAVES
        .iter()
        .find(|published| **published == leaf)?;
    Some(UploadTarget {
        job_id: job_id.to_string(),
        leaf,
    })
}

/// Whether `value` is a `com.amazonaws.dataexchange#Id` — the model's
/// `^[a-zA-Z0-9]{30,40}$`, which every identifier this service mints satisfies.
fn is_modelled_id(value: &str) -> bool {
    (30..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// The Base64 MD5 of `body`, in the encoding the model's `Md5Hash` /
/// `ApiSpecificationMd5Hash` members carry (`@length(24, 24)` with a base64
/// `@pattern`) and HTTP's own `Content-MD5` header uses.
pub fn content_md5(body: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(Md5::digest(body))
}

/// The entity tag of stored content: its MD5 as quoted hex, which is the `ETag` an
/// Amazon S3 presigned upload of a whole object answers with.
pub fn content_etag(body: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Md5::digest(body)))
}

/// What one PUT to a published upload URL did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    /// The bytes are now the job's staged content.
    Stored,
    /// The path addresses no upload this service minted.
    NoSuchUpload,
    /// The window the create call published for the URL has closed.
    Expired,
    /// The bytes are not the ones the job's declared digest describes.
    DigestMismatch,
    /// Retaining the bytes would push the store past [`MAX_CONTENT_BYTES`].
    StoreFull,
}

/// The most bytes one upload to a published URL retains.
///
/// AWS's own presigned target takes an object far larger; this emulator holds the
/// content in memory, so it keeps what one upload may leave behind bounded, and says
/// so rather than truncating.
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;

/// The most content bytes the whole store retains: every upload staged against a job
/// plus every imported asset's content, which is one ceiling over both because an
/// upload's bytes BECOME an asset's.
pub const MAX_CONTENT_BYTES: usize = 64 * 1024 * 1024;

/// The 500 a handler answers with when the shared engine is not the one this
/// service's store lives in. Never a fresh engine: that would drop the write.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the dataexchange store is unavailable".to_string(),
    }
}

/// The whole `Message` of a `ResourceNotFoundException`, for every resource kind and
/// every lookup site.
///
/// The sentence has no variable part: the model gives the exception its own
/// `ResourceId` ("The unique identifier for the resource that couldn't be found.")
/// and `ResourceType` ("The type of resource that couldn't be found.") members, so
/// the identity of the missing resource travels in those members and never in the
/// text. Every constructor below composes the same sentence and differs only in the
/// identity it reports.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// The modelled 404 for an identifier that addresses no stored resource of
/// `resource_type`: the fixed [`RESOURCE_NOT_FOUND_MESSAGE`], with the identifier the
/// caller wrote in `ResourceId` and the kind the route addresses in `ResourceType`.
///
/// `resource_type` is a value of the model's `ResourceType` enum
/// (`ASSET | DATA_GRANT | DATA_SET | EVENT_ACTION | JOB | REVISION`), or `None` where
/// the request named no kind this service can state.
fn resource_not_found(resource_id: &str, resource_type: Option<&str>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: Some(resource_id.to_string()),
        resource_type: resource_type.map(str::to_string),
    }
}

/// The modelled 404 for a well-formed `DataSetId` that names no stored data set.
pub fn data_set_not_found(data_set_id: &str) -> ResourceNotFoundException {
    resource_not_found(data_set_id, Some(RESOURCE_TYPE_DATA_SET))
}

/// The modelled 404 for a well-formed `RevisionId` that names no revision of
/// `data_set_id`.
pub fn revision_not_found(revision_id: &str) -> ResourceNotFoundException {
    resource_not_found(revision_id, Some(RESOURCE_TYPE_REVISION))
}

/// The modelled 404 for a well-formed `AssetId` that names no asset of the revision
/// the route addresses.
pub fn asset_not_found(asset_id: &str) -> ResourceNotFoundException {
    resource_not_found(asset_id, Some(RESOURCE_TYPE_ASSET))
}

/// The modelled 404 for an `EventActionId` that names no stored event action.
///
/// `EventActionId` is modelled as a plain `__string` (it carries no pattern, unlike
/// `Id`), so there is no malformed-id refusal in front of this one: any id that
/// addresses no stored event action is this 404.
pub fn event_action_not_found(event_action_id: &str) -> ResourceNotFoundException {
    resource_not_found(event_action_id, Some(RESOURCE_TYPE_EVENT_ACTION))
}

/// The modelled 404 for a well-formed `JobId` that names no stored job.
pub fn job_not_found(job_id: &str) -> ResourceNotFoundException {
    resource_not_found(job_id, Some(RESOURCE_TYPE_JOB))
}

/// The modelled 404 for a `DataGrantId`/`DataGrantArn` that addresses no data grant
/// this caller can reach: one no create minted, and one whose receiver the caller is
/// not where the route is the receiver's.
///
/// `identifier` is the value the caller wrote, which is the only identifier of the
/// grant the request named — an id or the ARN, whichever the member carries.
pub fn data_grant_not_found(identifier: &str) -> ResourceNotFoundException {
    resource_not_found(identifier, Some(RESOURCE_TYPE_DATA_GRANT))
}

/// The modelled 404 for a tagging `ResourceArn` that is an ARN but addresses no
/// resource this emulator holds.
///
/// `ResourceId` is the ARN the caller wrote — the only identifier of the resource
/// this call named — and `ResourceType` is left absent, because an ARN that
/// addresses nothing stored names no resource whose kind this service can state. The
/// id-addressed 404s above report both, since there the kind is the route's.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    resource_not_found(resource_arn, None)
}

/// The modelled 409 for a request that cannot hold against the current state of
/// `revision_id` — a write into a revision the model's `Finalized` documentation
/// calls "this read-only state".
pub fn revision_conflict(revision_id: &str, message: impl Into<String>) -> ConflictException {
    ConflictException {
        message: message.into(),
        resource_id: Some(revision_id.to_string()),
        resource_type: Some(RESOURCE_TYPE_REVISION.to_string()),
    }
}

/// The modelled 409 for a job operation the job's current `State` does not admit.
///
/// The sentence names only what was asked of the job (`verb`, the operation's own
/// verb — "cancel", "start") and the `State` a job has to be in for that to be
/// admitted: both `CancelJob` and `StartJob` act on a job that has not started yet,
/// and the model's `CancelJob` documentation says "Jobs can be cancelled only when
/// they are in the WAITING state". The identity of the job travels in the
/// exception's own `ResourceId` and `ResourceType` members, as it does in every
/// other refusal this service composes, so neither the job's id nor the state it is
/// actually in appears in the text.
pub fn job_state_conflict(job_id: &str, verb: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "Cannot {verb} the requested Job. \
             To {verb} a Job, it must be in the {JOB_STATE_WAITING} state."
        ),
        resource_id: Some(job_id.to_string()),
        resource_type: Some(RESOURCE_TYPE_JOB.to_string()),
    }
}

/// The modelled 400 for a `NextToken` that is not one this caller was issued.
///
/// Every token [`Cursor::encode`] hands out carries the account it was issued to, so
/// this is the refusal for a token naming another account AND for anything that is
/// not a token this service issued at all: neither carries the caller's account id.
/// The wording is the one AWS answers with, recorded in
/// `dataexchange.list-data-set-revisions-invalid-next-token-rejected`.
///
/// `ExceptionCause` is left unset: the model's only causes are
/// `InsufficientS3BucketPolicy` and `S3AccessDenied`, neither of which describes a
/// bad continuation token.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: "Invalid NextToken. Account Id does not match Caller's Account Id.".to_string(),
        exception_cause: None,
    }
}

/// What a refusal about the REQUEST'S OWN PARAMETERS opens with, whichever operation
/// composes it and whatever is wrong with them.
///
/// The faults of one request are stated as this fixed prefix followed by one clause
/// per fault, comma-separated, so a caller reads every fault from the one sentence.
/// Nothing but the clauses varies: the prefix carries no member name, no value and no
/// count.
const PARAMETERS_NOT_VALID_PREFIX: &str = "The following parameter(s) are not valid: ";

/// The clause stating that the request has to carry a parameter and does not.
///
/// `path` is the parameter's path from the operation's own input — the nesting written
/// with `.`, as in a `Details` member — and it is quoted in the sentence, which is the
/// one variable part of the clause.
pub fn required_parameter(path: &str) -> String {
    format!("{path:?} is required")
}

/// The clause stating that the request carries parameters it may not carry here. It
/// names none of them: the clause is the whole of what is said about them, so it is
/// fixed text. Private, so the clause reaches a `Message` only through the one
/// envelope constructor below.
const UNSUPPORTED_PARAMETERS: &str = "Unsupported request parameter(s) were found.";

/// The whole `Message` of a refusal about the request's parameters: the fixed
/// [`PARAMETERS_NOT_VALID_PREFIX`] followed by `clauses`, one per fault, in the order
/// the faults were found.
pub fn parameters_not_valid_message(clauses: &[String]) -> String {
    format!("{PARAMETERS_NOT_VALID_PREFIX}{}", clauses.join(", "))
}

/// The modelled 400 for a request whose own parameters are at fault; the `Message` is
/// [`parameters_not_valid_message`].
pub fn parameters_not_valid(clauses: &[String]) -> ValidationException {
    invalid_request(parameters_not_valid_message(clauses))
}

/// The `Message` refusing a request whose set of nested members is not the set the
/// request's own discriminator (its `Type`, its `Event`, its `Action`) admits:
/// `missing` is every parameter path the request has to carry and omits, and
/// `unsupported` says whether it carries a member that is not admitted here.
///
/// `None` when nothing is at fault — no path is missing and no member is unsupported —
/// so one call decides both whether to refuse and what the refusal says.
pub fn member_set_message(missing: &[&str], unsupported: bool) -> Option<String> {
    let mut clauses: Vec<String> = missing.iter().copied().map(required_parameter).collect();
    if unsupported {
        clauses.push(UNSUPPORTED_PARAMETERS.to_string());
    }
    (!clauses.is_empty()).then(|| parameters_not_valid_message(&clauses))
}

/// The whole `Message` of the refusal of a parameter whose value is outside the set
/// the model admits for it; see [`parameter_not_in_value_set`].
pub fn parameter_not_in_value_set_message(parameter: &str, admitted: &[&str]) -> String {
    parameters_not_valid_message(&[format!(
        "{parameter:?} must be one of [{}].",
        admitted.join(", ")
    )])
}

/// The modelled 400 for a request parameter whose value is outside the set the model
/// admits for it: `The following parameter(s) are not valid: "origin" must be one of
/// [OWNED, ENTITLED].`
///
/// `admitted` is the model's own value set, in the model's own order, so the sentence
/// states the contract rather than a remembered example.
pub fn parameter_not_in_value_set(parameter: &str, admitted: &[&str]) -> ValidationException {
    invalid_request(parameter_not_in_value_set_message(parameter, admitted))
}

/// Whether the request carried `name` as a query parameter at all - whatever value
/// it gave it.
///
/// This is the one fact a deserialized input cannot report for a member the model
/// gives a `@default`: such a member is a bare (non-`Option`) field, so an absent
/// query parameter and one sent as the default value arrive identically. The raw
/// query string is therefore where presence is read from. A parameter name is
/// matched verbatim, which is what a client sends: the names this service binds are
/// all unreserved ASCII, so no client percent-encodes them.
pub fn carries_query_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    name: &str,
) -> bool {
    ctx.parts().uri.query().is_some_and(|query| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(name))
    })
}

/// The page bound the request asked for: `Some(n)` for a caller-supplied
/// `maxResults`, `None` when the request carried none.
///
/// `MaxResults` is optional, and the model gives it `@default: 0` while its own
/// `@range` starts at 1 - a default outside the member's own bound. An absent
/// optional parameter is therefore ABSENT, not that default: it names no page size,
/// so the service serves its own page, and the `@range` is checked only against a
/// value the request actually carried. That is why the member carries
/// `skip_validation` on every list input of this service (see `src/ops/types/`):
/// the boundary walk would otherwise refuse the materialised default and so refuse
/// every call that legally omits the parameter.
///
/// `deserialized` is the member as the framework built it, which is the caller's
/// value whenever the parameter was present.
///
/// The structural fix lives in the vendored codegen
/// (`vendor/smithy-macros/src/lib.rs`: `usable_default` decides which members are
/// generated non-`Option` and `default_member_fallback_expr` materialises the
/// declared default) - a `@default` outside its own member's `@range` is not a
/// usable default, so the member should be generated `Option<T>` and left absent.
/// `vendor/` is read-only from inside the emulator, so the per-service seam the
/// framework offers for exactly this (`skip_validation`, plus the handler check
/// here) takes its place.
pub fn requested_max_results(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    deserialized: i32,
) -> Result<Option<i32>, ValidationException> {
    if !carries_query_parameter(ctx, MAX_RESULTS_QUERY) {
        return Ok(None);
    }
    if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&deserialized) {
        return Err(max_results_out_of_range(deserialized));
    }
    Ok(Some(deserialized))
}

/// The `@range` refusal of a caller-supplied `maxResults`, worded exactly as the
/// framework's own boundary walk words a violated numeric bound - through that
/// crate's own [`envelope`] over its own [`Violation`] - so moving this one member's
/// check into the handler cannot change the sentence a client reads.
fn max_results_out_of_range(value: i32) -> ValidationException {
    let kind = if value < MIN_PAGE_SIZE {
        ViolationKind::MinValue(MIN_PAGE_SIZE.to_string())
    } else {
        ViolationKind::MaxValue(MAX_PAGE_SIZE.to_string())
    };
    let violation = Violation {
        path: MAX_RESULTS_QUERY.to_string(),
        kind,
        value: Some(value.to_string()),
        sensitive: false,
    };
    // One violated bound is one violation, so the envelope always renders.
    invalid_request(envelope(&[violation]).unwrap_or_default())
}

/// The modelled 400 for a notification about a data set that exists and is owned by
/// the caller but is not part of an AWS Marketplace data product: there is nobody to
/// notify, because a notification goes to that product's subscribers.
///
/// The wording is the one AWS answers with, recorded in
/// `dataexchange.send-data-set-notification-error-semantics`.
pub fn data_set_not_configured_for_marketplace() -> ValidationException {
    invalid_request("Data set not configured for AWS Marketplace Data Product.")
}

/// The modelled 400 for finalizing a revision that holds no assets: a revision is
/// published to subscribers, so there has to be something in it to publish.
///
/// `assets` is what the revision holds at the moment of the refusal, which is what
/// makes the sentence state the record rather than a remembered example. The wording
/// is the one AWS answers with, recorded in
/// `dataexchange.finalize-revision-without-assets-rejected`.
pub fn revision_has_no_assets_to_finalize(assets: usize) -> ValidationException {
    invalid_request(format!(
        "Cannot set Finalized to true for a Revision which has {assets} Assets."
    ))
}

/// The modelled 400 for an event action whose event names a data set this account
/// owns: an event action exports a revision of a data set this account SUBSCRIBES
/// to, so the data set it watches must carry `Origin` [`ORIGIN_ENTITLED`].
///
/// The wording is the one AWS answers with, recorded in
/// `dataexchange.event-action-requires-entitled-dataset-rejected`.
pub fn event_action_requires_entitled_data_set() -> ValidationException {
    invalid_request(format!(
        "Event actions are only compatible with Data Sets where the value for Origin is \
         {ORIGIN_ENTITLED}."
    ))
}

/// The modelled 400 for a request the model's own constraints cannot express.
/// `message` names the member and the bound it broke, so the caller can fix the
/// request from it alone.
pub fn invalid_request(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        exception_cause: None,
    }
}

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteDataSet`, so each growth site carries a cap and nothing caller-sized
// reaches the store unchecked. A cap is named with the model's own `LimitName`
// value where the service names the limit; the VALUE is this emulator's resource
// guard, set well above any realistic use, because the model publishes none.
//
// The product is therefore finite: at most MAX_DATA_SETS data sets, each holding a
// Name of at most MAX_NAME_CHARS characters, a Description of at most
// MAX_DESCRIPTION_CHARS, at most MAX_TAGS tags of at most
// MAX_TAG_KEY_CHARS/MAX_TAG_VALUE_CHARS characters, and at most
// MAX_REVISIONS_PER_DATA_SET revisions, each itself holding a Comment the model
// already bounds at 16384 characters plus the same bounded tag map.
//
// Jobs are bounded the same way: at most MAX_JOBS of them, each holding at most
// MAX_JOB_DETAIL_ENTRIES entries in any one caller-sized detail list and at most
// MAX_TAGS asset-configuration tags (the model's own `@length(max: 50)` on
// `ListOfTag`) of at most MAX_TAG_KEY_CHARS/MAX_TAG_VALUE_CHARS characters.
//
// Event actions too: at most MAX_EVENT_ACTIONS of them, each holding one action
// whose caller-sized text is bounded by MAX_BUCKET_CHARS, MAX_KEY_PATTERN_CHARS and
// MAX_KMS_KEY_ARN_CHARS, plus the same bounded tag map.
//
// Data grants too: at most MAX_DATA_GRANTS of them, each holding a Name the model
// already bounds at 256 characters, a Description bounded by MAX_DESCRIPTION_CHARS
// and the same bounded tag map.
// ---------------------------------------------------------------------------

/// Emulator resource guard, NOT an AWS-published value: how many data sets this
/// emulator stores. The limit is the one the model's `LimitName` enum calls
/// [`LIMIT_DATA_SETS_PER_ACCOUNT`]; the model carries no number for it.
pub const MAX_DATA_SETS: usize = 250;

/// The model's `LimitName` value for the data-set count limit, reported on the
/// modelled `ServiceLimitExceededException`.
pub const LIMIT_DATA_SETS_PER_ACCOUNT: &str = "Data sets per account";

/// Emulator resource guard, NOT an AWS-published value: how many revisions this
/// emulator stores per data set. See [`LIMIT_REVISIONS_PER_DATA_SET`].
pub const MAX_REVISIONS_PER_DATA_SET: usize = 250;

/// Emulator resource guard, NOT an AWS-published value: how many jobs this emulator
/// stores. A job is reclaimed by nothing — the model has no DeleteJob — so the
/// store needs its own ceiling. The model's `LimitName` enum publishes only
/// CONCURRENCY limits for jobs, and no number for any of them.
pub const MAX_JOBS: usize = 500;

/// Emulator resource guard, NOT an AWS-published value: how many entries a single
/// job's caller-sized detail list (asset sources, asset destinations, revision
/// destinations) may carry. The model's `LimitName` enum names these limits
/// ("Assets per import job from Amazon S3", "Asset per export job from Amazon S3",
/// "Amazon Redshift datashare assets per import job from Redshift") but carries no
/// value for any of them.
pub const MAX_JOB_DETAIL_ENTRIES: usize = 100;

/// Emulator resource guard, NOT an AWS-published value: how many event actions this
/// emulator stores. The limit is the one the model's `LimitName` enum calls
/// [`LIMIT_EVENT_ACTIONS_PER_ACCOUNT`]; the model carries no number for it.
pub const MAX_EVENT_ACTIONS: usize = 250;

/// The model's `LimitName` value for the event-action count limit, reported on the
/// modelled `ServiceLimitExceededException`.
pub const LIMIT_EVENT_ACTIONS_PER_ACCOUNT: &str = "Event actions per account";

/// Emulator resource guard, NOT an AWS-published value: how many data grants this
/// emulator stores. The limit is the one the model's `LimitName` enum calls
/// [`LIMIT_ACTIVE_AND_PENDING_DATA_GRANTS`]; the model carries no number for it.
pub const MAX_DATA_GRANTS: usize = 250;

/// The model's `LimitName` value for the data-grant count limit, reported on the
/// modelled `ServiceLimitExceededException`.
pub const LIMIT_ACTIVE_AND_PENDING_DATA_GRANTS: &str = "Active and pending data grants";

/// Amazon S3's own bucket-naming rule: a bucket name is 3-63 characters long. The
/// destination bucket of an auto-export action is stored, so it is bounded by the
/// longest name S3 itself admits.
pub const MAX_BUCKET_CHARS: usize = 63;

/// Amazon S3's own object-key limit: a key is at most 1024 characters. An auto-export
/// `KeyPattern` names the keys the export writes, so it is bounded by the same.
pub const MAX_KEY_PATTERN_CHARS: usize = 1_024;

/// Emulator resource guard, NOT an AWS quota: the longest `KmsKeyArn` this emulator
/// stores. The model constrains it only as a string.
pub const MAX_KMS_KEY_ARN_CHARS: usize = 2_048;

/// The model's `LimitName` value for the per-data-set revision limit.
pub const LIMIT_REVISIONS_PER_DATA_SET: &str = "Revisions per data set";

/// From the `SchemaChangeDetails` shape's own documentation: "a notification can
/// have up to 100 of these".
pub const MAX_SCHEMA_CHANGES: usize = 100;

/// Emulator resource guard, NOT an AWS-published value: how many accepted
/// notifications this emulator keeps per data set. A notification is reclaimed only
/// with its data set, so the record needs its own ceiling; the model's `LimitName`
/// enum publishes no limit for notifications. The NEWEST are kept: the record is an
/// append-only log nothing reads back, so a bounded tail of it keeps the store
/// finite without refusing a call AWS accepts.
pub const MAX_NOTIFICATIONS_PER_DATA_SET: usize = 500;

/// From the `Description` member's own documentation: "A description for the data
/// set. This value can be up to 16,348 characters long."
pub const MAX_DESCRIPTION_CHARS: usize = 16_348;

/// Emulator resource guard, NOT an AWS quota: the longest `Name` this emulator
/// stores. The model constrains `Name` only as a string.
pub const MAX_NAME_CHARS: usize = 256;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// AWS's tagging limit on a tag key: up to 128 characters.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// AWS's tagging limit on a tag value: up to 256 characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Refuse a caller-sized collection whose entry count would grow the store past
/// `max`. `Err` carries the message naming the member and the bound it broke.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!(
            "{member} may carry at most {max} entries, got {count}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} may be at most {max} characters, got {length}"
        ));
    }
    Ok(())
}

/// Refuse a tag map the store will not hold: too many entries, or a key or value
/// past AWS's tagging limits.
pub fn bound_tags(tags: Option<&HashMap<String, String>>) -> Result<(), String> {
    let Some(tags) = tags else {
        return Ok(());
    };
    bound_entries("Tags", tags.len(), MAX_TAGS)?;
    for (key, value) in tags {
        bound_chars(&format!("Tags key {key:?}"), key, MAX_TAG_KEY_CHARS)?;
        bound_chars(&format!("Tags value for key {key:?}"), value, MAX_TAG_VALUE_CHARS)?;
    }
    Ok(())
}

/// The modelled 402 for a create that would take the store past [`MAX_DATA_SETS`].
pub fn data_set_limit_exceeded() -> ServiceLimitExceededException {
    ServiceLimitExceededException {
        limit_name: Some(LIMIT_DATA_SETS_PER_ACCOUNT.to_string()),
        limit_value: MAX_DATA_SETS as f64,
        message: format!(
            "The {LIMIT_DATA_SETS_PER_ACCOUNT} limit of {MAX_DATA_SETS} has been reached."
        ),
    }
}

/// The modelled 402 for a create that would take the store past [`MAX_EVENT_ACTIONS`].
pub fn event_action_limit_exceeded() -> ServiceLimitExceededException {
    ServiceLimitExceededException {
        limit_name: Some(LIMIT_EVENT_ACTIONS_PER_ACCOUNT.to_string()),
        limit_value: MAX_EVENT_ACTIONS as f64,
        message: format!(
            "The {LIMIT_EVENT_ACTIONS_PER_ACCOUNT} limit of {MAX_EVENT_ACTIONS} has been reached."
        ),
    }
}

/// The modelled 402 for a create that would take the store past [`MAX_DATA_GRANTS`].
pub fn data_grant_limit_exceeded() -> ServiceLimitExceededException {
    ServiceLimitExceededException {
        limit_name: Some(LIMIT_ACTIVE_AND_PENDING_DATA_GRANTS.to_string()),
        limit_value: MAX_DATA_GRANTS as f64,
        message: format!(
            "The {LIMIT_ACTIVE_AND_PENDING_DATA_GRANTS} limit of {MAX_DATA_GRANTS} has been reached."
        ),
    }
}

/// The parameter path of the one member an `Action` names — the action it performs
/// (`smithy_query(query="shape", shape="Action")`).
const ACTION_MEMBER_PATH: &str = "Action.ExportRevisionToS3";

/// The parameter path an `Action` has to carry and does not, or `None` when it names
/// the action it performs.
///
/// `Action`'s single member is optional in the model, so the boundary walk admits an
/// empty one; an operation that carries an `Action` beside other members reports this
/// fault together with theirs, in one refusal.
pub fn missing_action_member(action: &Action) -> Option<&'static str> {
    action
        .export_revision_to_s3
        .is_none()
        .then_some(ACTION_MEMBER_PATH)
}

/// Refuse an event action's `Action` the store will not hold, and one that describes
/// nothing to do.
///
/// `Action` and the details under it are modelled structures, so the boundary walk
/// has already refused a missing `@required` member; what it cannot state is that an
/// action has to name WHICH action it performs (the model's `Action` carries one
/// optional member, `ExportRevisionToS3`), nor how much caller-sized text the stored
/// record may carry. Both are checked here, so every operation that stores an action
/// — the create and the update — refuses the same values with the same sentence.
pub fn bound_action(action: &Action) -> Result<(), String> {
    let Some(export) = action.export_revision_to_s3.as_ref() else {
        return Err(parameters_not_valid_message(&[required_parameter(
            ACTION_MEMBER_PATH,
        )]));
    };
    bound_chars(
        "Action.ExportRevisionToS3.RevisionDestination.Bucket",
        &export.revision_destination.bucket,
        MAX_BUCKET_CHARS,
    )?;
    if let Some(key_pattern) = export.revision_destination.key_pattern.as_deref() {
        bound_chars(
            "Action.ExportRevisionToS3.RevisionDestination.KeyPattern",
            key_pattern,
            MAX_KEY_PATTERN_CHARS,
        )?;
    }
    if let Some(kms_key_arn) = export
        .encryption
        .as_ref()
        .and_then(|encryption| encryption.kms_key_arn.as_deref())
    {
        bound_chars(
            "Action.ExportRevisionToS3.Encryption.KmsKeyArn",
            kms_key_arn,
            MAX_KMS_KEY_ARN_CHARS,
        )?;
    }
    Ok(())
}

/// Mint a fresh identifier for a data set or a revision.
///
/// The model's `com.amazonaws.dataexchange#Id` is `^[a-zA-Z0-9]{30,40}$`, so the
/// value must be 30-40 alphanumerics: this returns the first 16 bytes of a SHA-256
/// digest as 32 lowercase hex characters, which satisfies it.
///
/// The digest covers a process-monotonic sequence number, the wall clock and
/// `RandomState`'s OS-seeded key, so ids are unique within a run and unguessable
/// between runs. Nothing about the request feeds it: an identifier AWS generates
/// must not be derivable from the caller's input.
pub fn mint_id() -> String {
    use std::hash::{BuildHasher, RandomState};

    let sequence = MINT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u128, |since| since.as_nanos());
    let entropy = RandomState::new().hash_one(sequence);

    let mut digest = Sha256::new();
    digest.update(sequence.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(entropy.to_be_bytes());
    hex::encode(&digest.finalize()[..16])
}

/// The region to stamp into an ARN: the one the request's SigV4 credential scope
/// names, else [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:dataexchange:<region>:<account>:data-sets/<data_set_id>`.
pub fn data_set_arn(region: &str, data_set_id: &str) -> String {
    ::engine::emulator_arn(SERVICE, region, &format!("data-sets/{data_set_id}"))
}

/// `arn:aws:dataexchange:<region>:<account>:jobs/<job_id>`.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(SERVICE, region, &format!("jobs/{job_id}"))
}

/// `arn:aws:dataexchange:<region>:<account>:data-sets/<data_set_id>/revisions/<revision_id>`.
pub fn revision_arn(region: &str, data_set_id: &str, revision_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE,
        region,
        &format!("data-sets/{data_set_id}/revisions/{revision_id}"),
    )
}

/// `arn:aws:dataexchange:<region>:<account>:event-actions/<event_action_id>`.
pub fn event_action_arn(region: &str, event_action_id: &str) -> String {
    ::engine::emulator_arn(SERVICE, region, &format!("event-actions/{event_action_id}"))
}

/// The resource segment every data grant ARN carries, which is also what tells an
/// ARN apart from a bare id in the model's `DataGrantId`
/// (`^[a-zA-Z0-9]{30,40}$|^arn:aws:dataexchange:[\-a-z0-9]*:(\d{12}):data-grants\/[a-zA-Z0-9]{30,40}$`).
const DATA_GRANT_RESOURCE_PREFIX: &str = "data-grants/";

/// `arn:aws:dataexchange:<region>:<account>:data-grants/<data_grant_id>`.
pub fn data_grant_arn(region: &str, data_grant_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE,
        region,
        &format!("{DATA_GRANT_RESOURCE_PREFIX}{data_grant_id}"),
    )
}

/// The data grant a `DataGrantId` or a `DataGrantArn` member addresses, as the store
/// keys it.
///
/// The model lets `DataGrantId` carry either the bare id or the grant's ARN, and
/// `DataGrantArn` carries the ARN, so both forms are read here: the id is the segment
/// after the ARN's own resource prefix, and a value carrying no ARN IS the id. The
/// boundary walk has already refused anything outside those two patterns.
pub fn data_grant_key(identifier: &str) -> &str {
    identifier
        .rsplit_once(DATA_GRANT_RESOURCE_PREFIX)
        .map_or(identifier, |(_, id)| id)
}

/// `arn:aws:dataexchange:<region>:<account>:data-sets/<data_set_id>/revisions/<revision_id>/assets/<asset_id>`.
pub fn asset_arn(region: &str, data_set_id: &str, revision_id: &str, asset_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE,
        region,
        &format!("data-sets/{data_set_id}/revisions/{revision_id}/assets/{asset_id}"),
    )
}

/// Now, as the modelled `Timestamp` members carry it.
pub fn now() -> DateTime {
    DateTime::from(SystemTime::now())
}

/// A decoded pagination cursor: where the next page starts.
///
/// Both halves are kept because either alone is wrong when the collection changed
/// between pages: `anchor` is the id of the last item already returned, so an
/// insertion or deletion earlier in the list cannot make a page repeat or skip an
/// item, and `offset` is the fallback for when that anchor itself was deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub offset: usize,
    pub anchor: String,
}

/// Version prefix of an encoded cursor, so a token from a different scheme is
/// recognised as unparsable rather than silently misread.
const CURSOR_VERSION: &str = "1";

/// The account a `NextToken` is issued to, and so the only account one can be
/// resumed by. This emulator serves exactly one account
/// ([`::engine::EMULATOR_ACCOUNT_ID`], the account every ARN it mints carries), so
/// that is the caller's account on every request.
pub fn caller_account() -> &'static str {
    ::engine::EMULATOR_ACCOUNT_ID
}

/// The longest token [`Cursor::encode`] can produce, as hex characters: two per
/// byte of `"<version>|<account>|<offset>|<anchor>"`, where the version is one
/// character, an account id is 12 digits, a `usize` offset is at most 20 digits, the
/// anchor is an `Id` of at most 40 characters, and the three separators are one
/// each. A longer token is not one this service issued, so [`Cursor::decode`]
/// refuses it before decoding anything.
const MAX_CURSOR_CHARS: usize = 2 * (1 + 1 + 12 + 1 + 20 + 1 + 40);

impl Cursor {
    /// The cursor that resumes after `offset` items, the last of which is `anchor`.
    pub fn new(offset: usize, anchor: impl Into<String>) -> Self {
        Self {
            offset,
            anchor: anchor.into(),
        }
    }

    /// Render the cursor as the opaque `NextToken` a caller passes back.
    ///
    /// Hex of the internal encoding: the token is alphanumeric and carries no
    /// readable structure, so a client cannot build one by hand and depend on a
    /// layout the service is free to change.
    ///
    /// The account the token is issued to travels inside it, because a page is a
    /// position in ONE account's collection: resuming it from another account would
    /// name a position in a collection the token was never minted against.
    pub fn encode(&self) -> String {
        hex::encode(format!(
            "{CURSOR_VERSION}|{}|{}|{}",
            caller_account(),
            self.offset,
            self.anchor
        ))
    }

    /// Parse a `NextToken` this caller was issued. `None` for anything else - the
    /// caller gets a `ValidationException`, never a silently different page.
    ///
    /// A token whose account is not the caller's is refused exactly like one this
    /// service never issued: see [`invalid_next_token`] for the wording both get.
    ///
    /// A token longer than one this encoder can produce is rejected before it is
    /// decoded, so the work a caller can ask of this parser is bounded here rather
    /// than only by the request-size limit.
    pub fn decode(token: &str) -> Option<Self> {
        if token.len() > MAX_CURSOR_CHARS {
            return None;
        }
        let bytes = hex::decode(token).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let mut fields = text.splitn(4, '|');
        let version = fields.next()?;
        let account = fields.next()?;
        let offset = fields.next()?.parse::<usize>().ok()?;
        let anchor = fields.next()?;
        if version != CURSOR_VERSION || account != caller_account() {
            return None;
        }
        Some(Self::new(offset, anchor))
    }
}

/// The half-open window `start..end` of `ordered` that the cursor `token` and the
/// caller's `maxResults` select.
///
/// `None` means the token is not one this caller was issued.
///
/// The returned window always satisfies `start <= end <= ordered.len()`, so a
/// caller can slice `ordered` with it: a cursor carries a caller-controlled
/// offset, and one naming a position past the end (its anchor was deleted, or it
/// was minted when the collection held more) must come back as an empty final page
/// rather than panicking at the slice. That clamp lives here so every operation
/// paging through this helper inherits it.
///
/// `max_results` is the page size the CALLER asked for, as
/// [`requested_max_results`] read it off the request: `None` means the request
/// named none, and the page is bounded by the largest the model admits
/// ([`MAX_PAGE_SIZE`]) instead. A value is clamped into the modelled range, which is
/// belt and braces - the caller's value was refused outside it before this is
/// reached.
pub fn page_window(
    ordered: &[String],
    token: Option<&str>,
    max_results: Option<i32>,
) -> Option<(usize, usize)> {
    let requested_start = match token {
        None => 0,
        Some(token) => {
            let cursor = Cursor::decode(token)?;
            match ordered.iter().position(|id| *id == cursor.anchor) {
                // The anchor is still there: resume immediately after it, whatever
                // moved around it.
                Some(index) => index.saturating_add(1),
                // The anchor was deleted between pages; fall back to the recorded
                // offset.
                None => cursor.offset,
            }
        }
    };
    let start = requested_start.min(ordered.len());
    let bound = max_results
        .unwrap_or(MAX_PAGE_SIZE)
        .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE) as usize;
    let end = ordered.len().min(start.saturating_add(bound));
    Some((start, end))
}

/// The `NextToken` to return after serving `ordered[start..end]`, or `None` when
/// that page exhausted the collection.
pub fn next_token(ordered: &[String], start: usize, end: usize) -> Option<String> {
    if end >= ordered.len() || end == start {
        return None;
    }
    Some(Cursor::new(end, ordered[end - 1].clone()).encode())
}
