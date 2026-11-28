// SPDX-License-Identifier: CC0-1.0

//! The `Dataset` records of this service: what a `CreateDataset` persists, the lifecycle
//! the clock moves one through, and the projections its three readers answer with.
//!
//! A dataset is held UNDER the `Domain` it was created in (`Domain::datasets`), which is
//! what scopes it: one domain's dataset is not reachable through another's, and a domain's
//! datasets go away with the domain. Every member a response carries is read back out of
//! the stored record.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CreateDatasetOutput, DatasetArtifacts, DatasetConfig, DatasetSummary, GetDatasetOutput,
};

use super::{fingerprint_of, to_timestamp};

/// How long a `Dataset` sits in ONE non-terminal `DatasetStatus` before the transition the
/// clock owes it is materialised (`CREATING` -> `ACTIVE`, and `DELETING` -> gone).
///
/// AWS documents no duration for building or deleting a dataset, so the span is CHOSEN, and
/// chosen inside the same transient window as the rest of this service's lifecycles
/// ([`super::DOMAIN_DELETION_SPAN`], [`super::PATIENT_INSIGHTS_JOB_STEP_SPAN`]): a caller
/// that reads straight after `CreateDataset` still observes `CREATING` rather than a
/// dataset that was already built by the time the create returned.
pub const DATASET_STEP_SPAN: Duration = Duration::from_millis(400);

/// The most `Dataset` records ONE `Domain` holds at a time.
///
/// The EMULATOR's bound on this store's growth, not a published quota: `CreateDataset` is
/// the one write that adds a dataset, and a caller looping it would otherwise grow the
/// shared store without limit. Past this count a create is refused with the
/// `ServiceQuotaExceededException` the operation models, and a delete frees a slot again
/// (the record is dropped when its deletion falls due).
pub const MAX_DATASETS_PER_DOMAIN: usize = 10;

/// A dataset's lifecycle, as the model's `DatasetStatus` enum declares it
/// (`smithy_query(query="shape",shape="DatasetStatus",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`:
/// `CREATING`, `ACTIVE`, `CREATE_FAILED`, `DELETING`, `DELETE_FAILED`, `DELETED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetStatus {
    Creating,
    Active,
    CreateFailed,
    Deleting,
    DeleteFailed,
    Deleted,
}

impl DatasetStatus {
    /// The wire value — one of `types::DATASET_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::CreateFailed => "CREATE_FAILED",
            Self::Deleting => "DELETING",
            Self::DeleteFailed => "DELETE_FAILED",
            Self::Deleted => "DELETED",
        }
    }

    /// The status a wire value names, or `None` when it names none.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "CREATING" => Some(Self::Creating),
            "ACTIVE" => Some(Self::Active),
            "CREATE_FAILED" => Some(Self::CreateFailed),
            "DELETING" => Some(Self::Deleting),
            "DELETE_FAILED" => Some(Self::DeleteFailed),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }

    /// Whether `DeleteDataset` may start deleting a dataset in this status.
    ///
    /// The model's own documentation of that operation states the set: "You can delete a
    /// dataset whose status is ACTIVE, CREATE_FAILED, or DELETE_FAILED"
    /// (`smithy_query(query="doc",shape="DeleteDataset")`). A dataset still being built or
    /// already being deleted is not in it, which is what the operation's
    /// `ConflictException` answers.
    pub fn deletable(self) -> bool {
        matches!(self, Self::Active | Self::CreateFailed | Self::DeleteFailed)
    }
}

/// The one `DatasetType` value the model declares
/// (`smithy_query(query="shape",shape="DatasetType")`: `MEDICAL_CODING`), which is the type
/// a `medicalCoding` configuration gives a dataset.
pub const MEDICAL_CODING_DATASET_TYPE: &str = "MEDICAL_CODING";

/// The type a dataset's own CONFIGURATION gives it.
///
/// `CreateDatasetOutput$type` is "The type of the dataset, determined by the configuration
/// that you provided" (`smithy_query(query="shape",shape="CreateDatasetOutput")`), so the
/// type is DERIVED from the union member the request carried rather than being a second
/// value a request supplies — which is why no request member names it.
pub fn dataset_type_of(config: &DatasetConfig) -> &'static str {
    match config {
        DatasetConfig::medicalCoding(_) => MEDICAL_CODING_DATASET_TYPE,
    }
}

/// The identity of a `CreateDataset` REQUEST, beside the `clientToken` that names it: a
/// digest of the parameters the operation accepts — `name` and the `config` union — so a
/// retry carrying one token can be told from a DIFFERENT request carrying it.
///
/// The model's own documentation states both halves of that rule: "if you retry a request
/// with the same clientToken, it returns the dataset that the first request created", and
/// (on `clientToken` itself) "Use a new clientToken for a request with different
/// parameters", which is what the operation's `ConflictException` answers. Built through
/// [`fingerprint_of`], the one place this crate turns a request's parameters into its
/// identity, with the union member folded in ahead of its values so two configurations
/// cannot be read into one another.
pub fn dataset_creation_fingerprint(name: &str, config: &DatasetConfig) -> String {
    let mut parts: Vec<String> = vec![name.to_string()];
    match config {
        DatasetConfig::medicalCoding(medical_coding) => {
            parts.push("medicalCoding".to_string());
            parts.push(medical_coding.s3_input_path.clone());
            parts.push(medical_coding.s3_output_path.clone());
        }
    }
    fingerprint_of(parts.iter().map(String::as_str))
}

/// A persisted `Dataset`.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub dataset_id: String,
    pub domain_id: String,
    pub arn: String,
    pub name: String,
    /// The modelled `DatasetType` this dataset's configuration gives it
    /// ([`dataset_type_of`]).
    pub dataset_type: String,
    /// The configuration the create persisted, reported back by `GetDataset` as the
    /// configuration "that you provided when you created the dataset".
    pub config: DatasetConfig,
    pub status: DatasetStatus,
    /// What is reported about the CURRENT status; `None` while there is nothing to say
    /// about it — which is every status this emulator's own lifecycle reaches, none of
    /// which is a failure.
    pub status_details: Option<String>,
    /// What building the dataset produced. `None` here, always, and deliberately: the
    /// member is "the results of building the dataset, such as medical coding evaluation
    /// scores" (`smithy_query(query="shape",shape="GetDatasetOutput")`), and this emulator
    /// evaluates no coded examples — so a built dataset reports no scores rather than
    /// claiming an evaluation nothing ran. Held on the record so a build that can report
    /// them has one place to write them.
    pub artifacts: Option<DatasetArtifacts>,
    pub created_at: SystemTime,
    /// When the record last changed — the instant of its last status transition, which is
    /// the creation instant for a dataset that has not moved yet.
    pub updated_at: SystemTime,
    /// When the next transition the clock owes becomes due; `None` in a status that owes
    /// none.
    pub next_transition_at: Option<SystemTime>,
    /// The `clientToken` the create carried, and the identity of that request
    /// ([`dataset_creation_fingerprint`]) — what tells a RETRY of one create from a
    /// different create presenting the same token. `None` for a create that carried no
    /// token, which binds nothing and so is never a retry.
    pub client_token: Option<String>,
    pub creation_fingerprint: String,
}

impl Dataset {
    /// Whether this record is one a reader of a dataset reports.
    ///
    /// The model's own documentation of `GetDataset` states the rule: "A dataset whose
    /// status is DELETED returns ResourceNotFoundException"
    /// (`smithy_query(query="doc",shape="GetDataset")`), and `ListDatasets` "doesn't return
    /// datasets whose status is DELETED". The ONE place that rule is stated: every reader
    /// goes through it, so a dataset a caller can enumerate is one it can also read.
    pub fn is_reportable(&self) -> bool {
        self.status != DatasetStatus::Deleted
    }

    /// Apply every status transition the CLOCK has already made due.
    ///
    /// A dataset owes at most one transition at a time, one [`DATASET_STEP_SPAN`] after the
    /// call that started it: `CREATING` -> `ACTIVE`, the status a built dataset reaches, and
    /// `DELETING` -> `DELETED`, the terminal status the model documents a delete as reaching
    /// ("The dataset moves to DELETING, and then to DELETED"). Each transition is stamped
    /// with the instant it FELL DUE rather than the instant somebody happened to look, so
    /// the record reads the same however late the first read comes.
    ///
    /// A record that reaches `DELETED` is dropped by the store that holds it
    /// ([`super::ComAmazonawsConnecthealthConnecthealthState::materialise_due`]) rather than
    /// kept: no reader of this service reports a dataset in that status, so a kept record
    /// would be observable nowhere and would grow the store for every create-and-delete a
    /// caller makes.
    pub(super) fn apply_due_transitions(&mut self, now: SystemTime) {
        while let Some(due) = self.next_transition_at.filter(|due| *due <= now) {
            match self.status {
                DatasetStatus::Creating => {
                    self.status = DatasetStatus::Active;
                    self.next_transition_at = None;
                }
                DatasetStatus::Deleting => {
                    self.status = DatasetStatus::Deleted;
                    self.next_transition_at = None;
                }
                // A status that owes nothing; a due instant left on one is dropped rather
                // than acted on.
                DatasetStatus::Active
                | DatasetStatus::CreateFailed
                | DatasetStatus::DeleteFailed
                | DatasetStatus::Deleted => self.next_transition_at = None,
            }
            self.updated_at = due;
        }
    }

    /// Start deleting this dataset, as of `now`: the status the delete moves it to, and the
    /// instant the removal the clock then owes becomes due.
    ///
    /// The ONE status step a dataset deletion takes, held on the record beside the
    /// transitions the clock applies rather than written out in the deleting handler.
    pub fn start_deleting(&mut self, now: SystemTime) {
        self.status = DatasetStatus::Deleting;
        self.updated_at = now;
        self.next_transition_at = Some(now + DATASET_STEP_SPAN);
    }

    /// This record as the `CreateDatasetOutput` the create answers with.
    pub fn create_output(&self) -> CreateDatasetOutput {
        CreateDatasetOutput {
            arn: self.arn.clone(),
            dataset_id: self.dataset_id.clone(),
            r#type: self.dataset_type.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_timestamp(self.created_at),
        }
    }

    /// This record as the `GetDatasetOutput` its reader answers with: every member read
    /// back out of what the create persisted and what the clock has since written.
    pub fn get_output(&self) -> GetDatasetOutput {
        GetDatasetOutput {
            arn: self.arn.clone(),
            dataset_id: self.dataset_id.clone(),
            name: self.name.clone(),
            r#type: self.dataset_type.clone(),
            config: self.config.clone(),
            status: self.status.as_wire().to_string(),
            status_details: self.status_details.clone(),
            artifacts: self.artifacts.clone(),
            created_at: to_timestamp(self.created_at),
            updated_at: to_timestamp(self.updated_at),
        }
    }

    /// This record as the `DatasetSummary` the listing reports.
    ///
    /// The SAME projection as [`Self::get_output`] minus the two members the summary shape
    /// does not declare (`config` and `artifacts`), so the listing and the reader cannot
    /// describe one stored record differently.
    pub fn summary(&self) -> DatasetSummary {
        let GetDatasetOutput {
            arn,
            dataset_id,
            name,
            r#type,
            config: _,
            status,
            status_details,
            artifacts: _,
            created_at,
            updated_at,
        } = self.get_output();
        DatasetSummary {
            arn,
            dataset_id,
            name,
            r#type,
            status,
            status_details,
            created_at,
            updated_at,
        }
    }
}

/// The datasets of one domain that a reader reports, in `datasetId` order.
///
/// Held beside the record type rather than in a handler, so the listing, the reader and the
/// job that references a dataset all resolve the same set.
pub fn reportable_datasets(datasets: &BTreeMap<String, Dataset>) -> impl Iterator<Item = &Dataset> {
    datasets.values().filter(|dataset| dataset.is_reportable())
}
