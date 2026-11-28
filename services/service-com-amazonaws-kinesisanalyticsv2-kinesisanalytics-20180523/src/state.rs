// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523 service.
//!
//! The store is the single source of truth for every response this service computes:
//! `CreateApplication` writes an [`Application`] here, and `DescribeApplication`,
//! `ListApplications`, `StartApplication`, `StopApplication`, `UpdateApplication` and
//! `DeleteApplication` all read or mutate THIS struct — nothing is echoed from a request
//! or canned. `CreateApplicationSnapshot` writes a [`Snapshot`] onto the application it
//! names, `DescribeApplicationSnapshot` and `ListApplicationSnapshots` read it back from
//! there and `DeleteApplicationSnapshot` removes it. An ENHANCED snapshot is a resource of
//! the account rather than of one application, so it lives in [`enhanced_snapshots`] beside
//! these records and not on an [`Application`].
//!
//! `AddApplicationCloudWatchLoggingOption` and `DeleteApplicationCloudWatchLoggingOption`
//! attach and detach the log streams an application is monitored through
//! ([`ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::add_logging_option`],
//! [`ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::remove_logging_option`]).
//!
//! Beside the records themselves this module owns the two projections every reader
//! needs ([`Application::detail`], [`Application::summary`]), the per-version records a
//! version reader reports ([`Application::version_detail`],
//! [`Application::version_summaries`]), the tag list the tagging operations read and write
//! ([`Application::tag_list`]), the resolution of a `ResourceARN` to the application it
//! names ([`ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::application_by_arn`]),
//! the merge an update applies
//! to a stored configuration ([`Application::apply_configuration_update`]), the version a
//! change is addressed to ([`addressed_version_refusal`]), the mode an
//! application runs in when its create named none ([`default_application_mode`]) and whether
//! the mode a create named is one its runtime runs ([`application_mode_refusal`]), whether
//! the runtime generation a request names is one this service still puts an application on
//! ([`deprecated_runtime_refusal`]), the
//! maintenance window every application carries
//! ([`Application::maintenance_configuration`]), the resolution of a Studio notebook's
//! catalog against the Glue Data Catalog ([`catalog_database_refusal`]), the resolution of
//! the service execution role an application runs as
//! ([`service_execution_role_is_unassumable`]) and the
//! clock-owed status settlement
//! ([`ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::settle`]),
//! so an operation handler never re-derives them.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod enhanced_snapshots;
pub use enhanced_snapshots::*;

mod service_execution_role;
pub use service_execution_role::*;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ApplicationCodeConfigurationDescription, ApplicationConfiguration,
    ApplicationConfigurationDescription, ApplicationConfigurationUpdate, ApplicationDetail,
    ApplicationEncryptionConfigurationDescription,
    ApplicationMaintenanceConfigurationDescription, ApplicationSnapshotConfigurationDescription,
    ApplicationSummary, ApplicationSystemRollbackConfigurationDescription,
    ApplicationVersionSummary,
    CatalogConfigurationDescription, CheckpointConfigurationDescription, CodeContentDescription,
    CustomArtifactConfigurationDescription, DeployAsApplicationConfigurationDescription,
    EnvironmentPropertyDescriptions, FlinkApplicationConfigurationDescription,
    GlueDataCatalogConfigurationDescription, Input, InputDescription, InputParallelism,
    InputProcessingConfigurationDescription, InputLambdaProcessorDescription,
    KinesisFirehoseInputDescription, KinesisFirehoseOutputDescription,
    KinesisStreamsInputDescription, KinesisStreamsOutputDescription, LambdaOutputDescription,
    MonitoringConfigurationDescription, Output, OutputDescription,
    ParallelismConfigurationDescription,
    ReferenceDataSourceDescription, RunConfigurationDescription, RunConfigurationUpdate,
    S3ApplicationCodeLocationDescription, S3ContentBaseLocationDescription,
    S3ReferenceDataSourceDescription, SnapshotDetails, SqlApplicationConfigurationDescription, Tag,
    VpcConfigurationDescription, ZeppelinApplicationConfigurationDescription,
    ZeppelinMonitoringConfigurationDescription,
};
use crate::ops::types::{
    CloudWatchLoggingOption, CloudWatchLoggingOptionDescription, CloudWatchLoggingOptionUpdate,
};

/// `ApplicationStatus` values this slice drives, spelled exactly as the model's
/// `ApplicationStatus` enum does (`smithy_query(query="shape",shape="ApplicationStatus")`).
pub const STATUS_READY: &str = "READY";
pub const STATUS_STARTING: &str = "STARTING";
pub const STATUS_RUNNING: &str = "RUNNING";
pub const STATUS_STOPPING: &str = "STOPPING";
pub const STATUS_UPDATING: &str = "UPDATING";

/// `OperationStatus` values, from the model's `OperationStatus` enum.
pub const OPERATION_IN_PROGRESS: &str = "IN_PROGRESS";
pub const OPERATION_SUCCESSFUL: &str = "SUCCESSFUL";

/// `SnapshotStatus` values this slice drives, spelled exactly as the model's
/// `SnapshotStatus` enum does (`smithy_query(query="shape",shape="SnapshotStatus")`).
pub const SNAPSHOT_CREATING: &str = "CREATING";
pub const SNAPSHOT_READY: &str = "READY";

/// How long a status transition the clock owes stays pending.
///
/// `StartApplication` and `StopApplication` are asynchronous: they answer while the
/// application is still `STARTING`/`STOPPING` and the terminal status becomes due
/// without any further call. AWS documents no duration for that settle, so this span is
/// CHOSEN — inside the emulator's 250–750 ms window — and is deliberately non-zero so a
/// caller that reads straight back still observes the transient status.
const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The status change the clock owes an application, and the operation record it completes.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The `ApplicationStatus` the application takes once `due_at` has passed.
    pub to_status: String,
    /// The `AppOperation` this transition completes.
    pub operation_id: String,
    /// When the transition becomes due, on the monotonic clock.
    pub due_at: Instant,
    /// The same instant on the wall clock, so the completion timestamps a reader sees
    /// are the instant the change was OWED and do not drift with when it is observed.
    pub due_wall: DateTime,
}

/// One asynchronous application operation (`StartApplication`, `StopApplication`, …).
///
/// The `OperationId` those operations return is the id of a record held HERE, so the
/// value handed to a client is backed by state rather than minted and forgotten.
#[derive(Debug, Clone)]
pub struct AppOperation {
    pub operation_id: String,
    /// The API operation that created this record, e.g. `"StartApplication"`.
    pub operation: String,
    /// An `OperationStatus` value: `IN_PROGRESS` | `SUCCESSFUL` | `FAILED` | `CANCELLED`.
    pub status: String,
    pub start_time: DateTime,
    pub end_time: Option<DateTime>,
}

/// One stored snapshot of an application's state.
///
/// `CreateApplicationSnapshot` writes one of these and `DescribeApplicationSnapshot`
/// reports it, so the `SnapshotDetails` a reader sees is the record the writer stored.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub name: String,
    /// A `SnapshotStatus` enum value.
    pub status: String,
    /// The application version the snapshot was taken of.
    pub application_version_id: i64,
    pub creation_timestamp: DateTime,
    /// The application's `RuntimeEnvironment` when the snapshot was taken.
    pub runtime_environment: String,
    /// When `CREATING` becomes `READY` on the monotonic clock, while that change is
    /// outstanding.
    pub ready_at: Option<Instant>,
}

impl Snapshot {
    /// Apply the status change the clock owes this snapshot, if it is due at `now`.
    ///
    /// The same rule as [`Application::settle`]: the change was owed at `ready_at`
    /// whether or not anybody called, so materialising it on a read is not a read-driven
    /// mutation.
    fn settle(&mut self, now: Instant) {
        if self.ready_at.is_some_and(|due| now >= due) {
            self.status = SNAPSHOT_READY.to_string();
            self.ready_at = None;
        }
    }

    /// The `SnapshotDetails` `DescribeApplicationSnapshot` and `ListApplicationSnapshots`
    /// report.
    pub fn details(&self) -> SnapshotDetails {
        SnapshotDetails {
            snapshot_name: self.name.clone(),
            snapshot_status: self.status.clone(),
            application_version_id: self.application_version_id,
            snapshot_creation_timestamp: Some(self.creation_timestamp),
            runtime_environment: Some(self.runtime_environment.clone()),
            // The emulator stores no customer-managed key for an application, so a
            // snapshot carries no encryption description of its own.
            application_encryption_configuration_description: None,
        }
    }
}

/// One stored Managed Service for Apache Flink application.
#[derive(Debug, Clone)]
pub struct Application {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// A `RuntimeEnvironment` enum value, as supplied at create.
    pub runtime_environment: String,
    pub service_execution_role: String,
    /// The `ApplicationMode` enum value this application runs in: the one the creator
    /// supplied, else the one its runtime implies ([`default_application_mode`]). Every
    /// application runs in one of the two modes, so this is not optional.
    pub application_mode: String,
    /// An `ApplicationStatus` enum value.
    pub status: String,
    /// The status change the clock owes, while one is outstanding.
    pub pending: Option<PendingTransition>,
    pub version_id: i64,
    /// The version this application was on before the latest configuration update, while
    /// one has landed: `ApplicationVersionUpdatedFrom`. `None` for an application still on
    /// the version its create wrote.
    pub version_updated_from: Option<i64>,
    pub create_timestamp: DateTime,
    /// When the current application version was created; equal to `create_timestamp`
    /// until an operation that mints a new version lands.
    pub version_create_timestamp: DateTime,
    /// When this application last changed. The create is itself the first such change,
    /// so an application that has never been updated since reports the instant it was
    /// created and a reader never sees this member absent.
    pub last_update_timestamp: DateTime,
    pub configuration: Option<ApplicationConfigurationDescription>,
    /// The start of the window maintenance on this application runs in, as an
    /// `ApplicationMaintenanceWindowStartTime` (`HH:MM`): the one the account is
    /// assigned ([`DEFAULT_MAINTENANCE_WINDOW_START`]) until
    /// `UpdateApplicationMaintenanceConfiguration` replaces it. The window's END is
    /// derived from it ([`maintenance_window_end`]) rather than stored beside it, so the
    /// two can never disagree about how long a maintenance window is.
    pub maintenance_window_start: String,
    pub cloud_watch_logging_options: Vec<CloudWatchLoggingOptionDescription>,
    /// The optimistic-concurrency token for the current version.
    pub conditional_token: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub operations: Vec<AppOperation>,
    /// The snapshots taken of this application, by `SnapshotName`, so a reader walks a
    /// stable name-ordered sequence. Bounded by [`MAX_SNAPSHOTS_PER_APPLICATION`].
    pub snapshots: BTreeMap<String, Snapshot>,
    /// The record of every version this application has LEFT, by `ApplicationVersionId`,
    /// frozen as the detail that version ran under at the instant it was superseded. The
    /// version in force is NOT held here — it is the live record above, so a reader of it
    /// sees the status and the configuration in force now — which is why
    /// [`Application::version_detail`] and [`Application::version_summaries`] are the one
    /// way a reader reaches a version at all. Ordered by id, so a version walk is
    /// ascending on every call and a pagination token can resume it by naming an id.
    /// Bounded by [`MAX_RETAINED_VERSIONS`].
    pub versions: BTreeMap<i64, ApplicationDetail>,
}

impl Application {
    /// Apply the status change the clock owes, if it is due at `now`.
    ///
    /// This is the ONLY write a reporting operation may make: the change was owed at
    /// `due_at` whether or not anybody called, so materialising it here is not a
    /// read-driven mutation. Everything derived from it (the completion timestamps) is
    /// stamped at `due_wall`, so repeated reads report identical values.
    fn settle(&mut self, now: Instant) {
        for snapshot in self.snapshots.values_mut() {
            snapshot.settle(now);
        }
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.status = pending.to_status;
        self.last_update_timestamp = pending.due_wall;
        if let Some(op) = self
            .operations
            .iter_mut()
            .find(|op| op.operation_id == pending.operation_id)
        {
            op.status = OPERATION_SUCCESSFUL.to_string();
            op.end_time = Some(pending.due_wall);
        }
        self.pending = None;
    }

    /// Whether this application's stored configuration carries the code its runtime would
    /// run: `ApplicationCodeConfigurationDescription.CodeContentDescription` naming
    /// non-empty inline text, an S3 object, or a non-empty zip the creator uploaded (which
    /// the description reports as a code size, since AWS does not echo the bytes back).
    ///
    /// Read off the STORED description rather than off a request, so what decides is the
    /// configuration a create or an update actually persisted. An EMPTY text content or an
    /// empty zip is no more code than an absent one, so neither counts.
    pub fn has_code_content(&self) -> bool {
        self.configuration
            .as_ref()
            .and_then(|cfg| cfg.application_code_configuration_description.as_ref())
            .and_then(|code| code.code_content_description.as_ref())
            .is_some_and(|content| {
                content
                    .text_content
                    .as_deref()
                    .is_some_and(|text| !text.is_empty())
                    || content.s3_application_code_location_description.is_some()
                    || content.code_size.is_some_and(|size| size > 0)
            })
    }

    /// How many external destinations this application's stored SQL configuration already
    /// describes — the count `AddApplicationOutput` holds to
    /// [`MAX_OUTPUTS_PER_APPLICATION`], and 0 for an application carrying no SQL
    /// configuration at all.
    ///
    /// Read off the STORED description rather than off a request, so what decides is the
    /// configuration a create or an add actually persisted.
    pub fn described_outputs(&self) -> usize {
        self.configuration
            .as_ref()
            .and_then(|cfg| cfg.sql_application_configuration_description.as_ref())
            .and_then(|sql| sql.output_descriptions.as_ref())
            .map_or(0, Vec::len)
    }

    /// The `ApplicationDetail` `CreateApplication` and `DescribeApplication` return.
    pub fn detail(&self) -> ApplicationDetail {
        ApplicationDetail {
            application_arn: self.arn.clone(),
            application_description: self.description.clone(),
            application_name: self.name.clone(),
            runtime_environment: self.runtime_environment.clone(),
            service_execution_role: Some(self.service_execution_role.clone()),
            application_status: self.status.clone(),
            application_version_id: self.version_id,
            create_timestamp: Some(self.create_timestamp),
            last_update_timestamp: Some(self.last_update_timestamp),
            application_configuration_description: self.configuration.clone(),
            cloud_watch_logging_option_descriptions: (!self.cloud_watch_logging_options.is_empty())
                .then(|| self.cloud_watch_logging_options.clone()),
            // Maintenance runs on every application in a window the service keeps for
            // it, so the window is reported from the moment the application exists —
            // the one the account is assigned until
            // `UpdateApplicationMaintenanceConfiguration` replaces its start time.
            application_maintenance_configuration_description: Some(
                self.maintenance_configuration(),
            ),
            // The version an accepted configuration update moved this application on
            // from, as [`ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State::advance_version`]
            // recorded it.
            application_version_updated_from: self.version_updated_from,
            // Only a rollback fills this, and RollbackApplication is not in this slice.
            application_version_rolled_back_from: None,
            application_version_create_timestamp: Some(self.version_create_timestamp),
            conditional_token: self.conditional_token.clone(),
            application_version_rolled_back_to: None,
            application_mode: Some(self.application_mode.clone()),
        }
    }

    /// The `ApplicationMaintenanceConfigurationDescription` every reader of this
    /// application reports: the stored start time and the end time
    /// [`maintenance_window_end`] derives from it.
    pub fn maintenance_configuration(&self) -> ApplicationMaintenanceConfigurationDescription {
        ApplicationMaintenanceConfigurationDescription {
            application_maintenance_window_start_time: self.maintenance_window_start.clone(),
            application_maintenance_window_end_time: maintenance_window_end(
                &self.maintenance_window_start,
            ),
        }
    }

    /// The `ApplicationDetail` of ONE version of this application: the live detail for the
    /// version in force, and the frozen record for a version this application has left.
    ///
    /// `None` when this application was never on that version, which is what every reader
    /// of a version by id reports as a miss.
    pub fn version_detail(&self, version_id: i64) -> Option<ApplicationDetail> {
        if version_id == self.version_id {
            return Some(self.detail());
        }
        self.versions.get(&version_id).cloned()
    }

    /// One `ApplicationVersionSummary` per version this application has, oldest first:
    /// every retained record and then the version in force.
    ///
    /// Each summary is projected from that version's own detail, so the status a summary
    /// carries is the status of the version it summarises — the live one for the version in
    /// force and the frozen one for a version already left. The retained details are read by
    /// REFERENCE: a summary is two scalars, so listing the history copies two scalars per
    /// version rather than a configuration record per version.
    pub fn version_summaries(&self) -> Vec<ApplicationVersionSummary> {
        let summarise = |version_id: i64, status: &str| ApplicationVersionSummary {
            application_version_id: version_id,
            application_status: status.to_string(),
        };
        self.versions
            .values()
            .map(|detail| summarise(detail.application_version_id, &detail.application_status))
            .chain(::std::iter::once(summarise(self.version_id, &self.status)))
            .collect()
    }

    /// This application's tags as the API reports them, in key order so two reads of one
    /// application report them in the same order.
    ///
    /// A tag assigned without a value carries the empty value the model admits
    /// (`Tag$Value` is `@length 0..256`), rather than being reported as a tag with no
    /// value member at all: the key IS assigned.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: Some(value.clone()),
            })
            .collect()
    }

    /// The `ApplicationSummary` `ListApplications` returns.
    pub fn summary(&self) -> ApplicationSummary {
        ApplicationSummary {
            application_name: self.name.clone(),
            application_arn: self.arn.clone(),
            application_status: self.status.clone(),
            application_version_id: self.version_id,
            runtime_environment: self.runtime_environment.clone(),
            application_mode: Some(self.application_mode.clone()),
        }
    }

    /// The `DatabaseARN` of the Glue Data Catalog database this application's stored
    /// Studio-notebook configuration points at, or `None` for one configured with no
    /// catalog at all.
    ///
    /// Read off the STORED description, so what a later call resolves is the catalog the
    /// create or the last accepted update persisted rather than a value a request carried.
    pub fn catalog_database_arn(&self) -> Option<&str> {
        self.configuration
            .as_ref()
            .and_then(|cfg| cfg.zeppelin_application_configuration_description.as_ref())
            .and_then(|zeppelin| zeppelin.catalog_configuration_description.as_ref())
            .map(|catalog| {
                catalog
                    .glue_data_catalog_configuration_description
                    .database_arn
                    .as_str()
            })
    }

    /// Apply an `ApplicationConfigurationUpdate` to this application's stored
    /// configuration description.
    ///
    /// Every `…Update` member REPLACES the one value it names and leaves the rest of the
    /// stored description as it stands, which is what makes a configuration-only update
    /// of one member (a log level, say) not a reset of the others. An update that names
    /// an element by id — an input, an output, a reference data source, a VPC
    /// configuration — the application holds no such element under is refused instead of
    /// silently doing nothing: `Err` carries that refusal's sentence.
    ///
    /// The merged description is then completed the same way a create's is
    /// ([`complete_with_service_defaults`]), so one rule decides what an application RUNS
    /// under however its configuration was written: a sub-configuration left or put back
    /// at `DEFAULT` reports the service's values, and only a `CUSTOM` one reports the
    /// caller's.
    pub fn apply_configuration_update(
        &mut self,
        update: &ApplicationConfigurationUpdate,
    ) -> Result<(), String> {
        let name = self.name.clone();
        let runtime_environment = self.runtime_environment.clone();
        let application_mode = self.application_mode.clone();
        let cfg = self.configuration.get_or_insert_with(Default::default);

        if let Some(code) = update.application_code_configuration_update.as_ref() {
            let stored = cfg
                .application_code_configuration_description
                .get_or_insert_with(Default::default);
            if let Some(content_type) = code.code_content_type_update.clone() {
                stored.code_content_type = content_type;
            }
            if let Some(content) = code.code_content_update.as_ref() {
                let described = stored
                    .code_content_description
                    .get_or_insert_with(Default::default);
                if let Some(text) = content.text_content_update.clone() {
                    described.code_size = Some(text.len() as i64);
                    described.text_content = Some(text);
                }
                if let Some(zip) = content.zip_file_content_update.as_ref() {
                    // AWS does not echo uploaded bytes back, so a zip is described by
                    // its size, exactly as a create describes one.
                    described.code_size = Some(zip.len() as i64);
                    described.text_content = None;
                }
                if let Some(s3) = content.s3_content_location_update.as_ref() {
                    let location = described
                        .s3_application_code_location_description
                        .get_or_insert_with(Default::default);
                    if let Some(bucket) = s3.bucket_arn_update.clone() {
                        location.bucket_arn = bucket;
                    }
                    if let Some(key) = s3.file_key_update.clone() {
                        location.file_key = key;
                    }
                    if let Some(version) = s3.object_version_update.clone() {
                        location.object_version = Some(version);
                    }
                }
            }
        }

        if let Some(flink) = update.flink_application_configuration_update.as_ref() {
            let stored = cfg
                .flink_application_configuration_description
                .get_or_insert_with(Default::default);
            if let Some(checkpoint) = flink.checkpoint_configuration_update.as_ref() {
                let described = stored
                    .checkpoint_configuration_description
                    .get_or_insert_with(Default::default);
                if let Some(kind) = checkpoint.configuration_type_update.clone() {
                    described.configuration_type = Some(kind);
                }
                if let Some(enabled) = checkpoint.checkpointing_enabled_update {
                    described.checkpointing_enabled = Some(enabled);
                }
                if let Some(interval) = checkpoint.checkpoint_interval_update {
                    described.checkpoint_interval = Some(interval);
                }
                if let Some(pause) = checkpoint.min_pause_between_checkpoints_update {
                    described.min_pause_between_checkpoints = Some(pause);
                }
            }
            if let Some(monitoring) = flink.monitoring_configuration_update.as_ref() {
                let described = stored
                    .monitoring_configuration_description
                    .get_or_insert_with(Default::default);
                if let Some(kind) = monitoring.configuration_type_update.clone() {
                    described.configuration_type = Some(kind);
                }
                if let Some(level) = monitoring.metrics_level_update.clone() {
                    described.metrics_level = Some(level);
                }
                if let Some(level) = monitoring.log_level_update.clone() {
                    described.log_level = Some(level);
                }
            }
            if let Some(parallelism) = flink.parallelism_configuration_update.as_ref() {
                let described = stored
                    .parallelism_configuration_description
                    .get_or_insert_with(Default::default);
                if let Some(kind) = parallelism.configuration_type_update.clone() {
                    described.configuration_type = Some(kind);
                }
                if let Some(count) = parallelism.parallelism_update {
                    described.parallelism = Some(count);
                    // The parallelism in force right now follows the configured one:
                    // autoscaling, which is what else would move it, is not modelled.
                    described.current_parallelism = Some(count);
                }
                if let Some(per_kpu) = parallelism.parallelism_per_kpu_update {
                    described.parallelism_per_kpu = Some(per_kpu);
                }
                if let Some(enabled) = parallelism.auto_scaling_enabled_update {
                    described.auto_scaling_enabled = Some(enabled);
                }
            }
        }

        if let Some(environment) = update.environment_property_updates.as_ref() {
            cfg.environment_property_descriptions = Some(EnvironmentPropertyDescriptions {
                property_group_descriptions: Some(environment.property_groups.clone()),
            });
        }

        if let Some(snapshots) = update.application_snapshot_configuration_update.as_ref() {
            // The update replaces the values it NAMES and leaves the rest of the description
            // as it stands, so an update that names only whether snapshots are taken carries
            // the described snapshot MODE over rather than dropping it.
            let described = cfg
                .application_snapshot_configuration_description
                .get_or_insert_with(Default::default);
            described.snapshots_enabled = snapshots.snapshots_enabled_update;
            if let Some(mode) = snapshots.snapshot_mode_update.clone() {
                described.snapshot_mode = Some(mode);
            }
        }

        if let Some(rollback) = update
            .application_system_rollback_configuration_update
            .as_ref()
        {
            cfg.application_system_rollback_configuration_description =
                Some(ApplicationSystemRollbackConfigurationDescription {
                    rollback_enabled: rollback.rollback_enabled_update,
                });
        }

        for vpc in update.vpc_configuration_updates.iter().flatten() {
            let described = cfg
                .vpc_configuration_descriptions
                .iter_mut()
                .flatten()
                .find(|described| described.vpc_configuration_id == vpc.vpc_configuration_id)
                .ok_or_else(|| {
                    missing_update_target_message(
                        "VPC configuration",
                        &vpc.vpc_configuration_id,
                        &name,
                    )
                })?;
            if let Some(subnets) = vpc.subnet_id_updates.clone() {
                // The VPC a set of subnets belongs to is derived from the subnets, so it
                // follows them when they change.
                described.vpc_id = vpc_id_for(&subnets);
                described.subnet_ids = subnets;
            }
            if let Some(groups) = vpc.security_group_id_updates.clone() {
                described.security_group_ids = groups;
            }
        }

        if let Some(zeppelin) = update.zeppelin_application_configuration_update.as_ref() {
            let stored = cfg
                .zeppelin_application_configuration_description
                .get_or_insert_with(|| ZeppelinApplicationConfigurationDescription {
                    // A description this update brings into existence still reports the
                    // verbosity in force, which for a notebook that has configured no
                    // monitoring is the service default.
                    monitoring_configuration_description:
                        ZeppelinMonitoringConfigurationDescription {
                            log_level: Some(DEFAULT_LOG_LEVEL.to_string()),
                        },
                    ..Default::default()
                });
            if let Some(monitoring) = zeppelin.monitoring_configuration_update.as_ref() {
                stored.monitoring_configuration_description.log_level =
                    Some(monitoring.log_level_update.clone());
            }
            if let Some(catalog) = zeppelin.catalog_configuration_update.as_ref() {
                stored.catalog_configuration_description = Some(CatalogConfigurationDescription {
                    glue_data_catalog_configuration_description:
                        GlueDataCatalogConfigurationDescription {
                            database_arn: catalog
                                .glue_data_catalog_configuration_update
                                .database_arn_update
                                .clone(),
                        },
                });
            }
            if let Some(deploy) = zeppelin.deploy_as_application_configuration_update.as_ref() {
                let described = stored
                    .deploy_as_application_configuration_description
                    .get_or_insert_with(Default::default);
                if let Some(s3) = deploy.s3_content_location_update.as_ref() {
                    if let Some(bucket) = s3.bucket_arn_update.clone() {
                        described.s3_content_location_description.bucket_arn = bucket;
                    }
                    if let Some(path) = s3.base_path_update.clone() {
                        described.s3_content_location_description.base_path = Some(path);
                    }
                }
            }
            if let Some(artifacts) = zeppelin.custom_artifacts_configuration_update.as_ref() {
                stored.custom_artifacts_configuration_description = Some(
                    artifacts
                        .iter()
                        .map(|artifact| CustomArtifactConfigurationDescription {
                            artifact_type: Some(artifact.artifact_type.clone()),
                            s3_content_location_description: artifact.s3_content_location.clone(),
                            maven_reference_description: artifact.maven_reference.clone(),
                        })
                        .collect(),
                );
            }
        }

        if let Some(encryption) = update.application_encryption_configuration_update.as_ref() {
            let described = cfg
                .application_encryption_configuration_description
                .get_or_insert_with(Default::default);
            if let Some(key) = encryption.key_id_update.clone() {
                described.key_id = Some(key);
            }
            described.key_type = encryption.key_type_update.clone();
        }

        if let Some(sql) = update.sql_application_configuration_update.as_ref() {
            let stored = cfg
                .sql_application_configuration_description
                .get_or_insert_with(Default::default);
            for input in sql.input_updates.iter().flatten() {
                let described = stored
                    .input_descriptions
                    .iter_mut()
                    .flatten()
                    .find(|described| described.input_id.as_deref() == Some(&input.input_id))
                    .ok_or_else(|| {
                        missing_update_target_message("Input", &input.input_id, &name)
                    })?;
                if let Some(prefix) = input.name_prefix_update.clone() {
                    described.name_prefix = Some(prefix);
                }
                if let Some(parallelism) = input.input_parallelism_update.as_ref() {
                    described.input_parallelism = Some(InputParallelism {
                        count: Some(parallelism.count_update),
                    });
                }
                // The in-app stream names are one per unit of input parallelism, named
                // after the prefix, so they follow both wherever either moves. The count
                // reaches here from the request, so it is bounded to the range the model
                // admits BEFORE it sizes the list it is about to derive.
                let stream_count = bounded_input_parallelism(
                    described
                        .input_parallelism
                        .as_ref()
                        .and_then(|parallelism| parallelism.count),
                );
                described.in_app_stream_names = described
                    .name_prefix
                    .as_deref()
                    .and_then(|prefix| in_app_stream_names(prefix, stream_count));
                if let Some(processing) = input.input_processing_configuration_update.as_ref() {
                    described.input_processing_configuration_description =
                        Some(InputProcessingConfigurationDescription {
                            input_lambda_processor_description: Some(
                                InputLambdaProcessorDescription {
                                    resource_arn: processing
                                        .input_lambda_processor_update
                                        .resource_arn_update
                                        .clone(),
                                    role_arn: None,
                                },
                            ),
                        });
                }
                if let Some(kinesis) = input.kinesis_streams_input_update.as_ref() {
                    described.kinesis_streams_input_description =
                        Some(KinesisStreamsInputDescription {
                            resource_arn: kinesis.resource_arn_update.clone(),
                            role_arn: None,
                        });
                }
                if let Some(firehose) = input.kinesis_firehose_input_update.as_ref() {
                    described.kinesis_firehose_input_description =
                        Some(KinesisFirehoseInputDescription {
                            resource_arn: firehose.resource_arn_update.clone(),
                            role_arn: None,
                        });
                }
                if let Some(schema) = input.input_schema_update.as_ref() {
                    let described = described.input_schema.get_or_insert_with(Default::default);
                    if let Some(format) = schema.record_format_update.clone() {
                        described.record_format = format;
                    }
                    if let Some(encoding) = schema.record_encoding_update.clone() {
                        described.record_encoding = Some(encoding);
                    }
                    if let Some(columns) = schema.record_column_updates.clone() {
                        described.record_columns = columns;
                    }
                }
            }
            for output in sql.output_updates.iter().flatten() {
                let described = stored
                    .output_descriptions
                    .iter_mut()
                    .flatten()
                    .find(|described| described.output_id.as_deref() == Some(&output.output_id))
                    .ok_or_else(|| {
                        missing_update_target_message("Output", &output.output_id, &name)
                    })?;
                if let Some(stream) = output.name_update.clone() {
                    described.name = Some(stream);
                }
                if let Some(kinesis) = output.kinesis_streams_output_update.as_ref() {
                    described.kinesis_streams_output_description =
                        Some(KinesisStreamsOutputDescription {
                            resource_arn: kinesis.resource_arn_update.clone(),
                            role_arn: None,
                        });
                }
                if let Some(firehose) = output.kinesis_firehose_output_update.as_ref() {
                    described.kinesis_firehose_output_description =
                        Some(KinesisFirehoseOutputDescription {
                            resource_arn: firehose.resource_arn_update.clone(),
                            role_arn: None,
                        });
                }
                if let Some(lambda) = output.lambda_output_update.as_ref() {
                    described.lambda_output_description = Some(LambdaOutputDescription {
                        resource_arn: lambda.resource_arn_update.clone(),
                        role_arn: None,
                    });
                }
                if let Some(schema) = output.destination_schema_update.clone() {
                    described.destination_schema = Some(schema);
                }
            }
            for reference in sql.reference_data_source_updates.iter().flatten() {
                let described = stored
                    .reference_data_source_descriptions
                    .iter_mut()
                    .flatten()
                    .find(|described| described.reference_id == reference.reference_id)
                    .ok_or_else(|| {
                        missing_update_target_message(
                            "Reference data source",
                            &reference.reference_id,
                            &name,
                        )
                    })?;
                if let Some(table) = reference.table_name_update.clone() {
                    described.table_name = table;
                }
                if let Some(s3) = reference.s3_reference_data_source_update.as_ref() {
                    if let Some(bucket) = s3.bucket_arn_update.clone() {
                        described.s3_reference_data_source_description.bucket_arn = bucket;
                    }
                    if let Some(key) = s3.file_key_update.clone() {
                        described.s3_reference_data_source_description.file_key = key;
                    }
                }
                if let Some(schema) = reference.reference_schema_update.clone() {
                    described.reference_schema = Some(schema);
                }
            }
        }

        complete_with_service_defaults(cfg, &runtime_environment, &application_mode);
        Ok(())
    }

    /// Apply the `CloudWatchLoggingOptionUpdates` of one update request.
    ///
    /// "You can only update existing CloudWatch logging options with this action" — the
    /// `UpdateApplication$CloudWatchLoggingOptionUpdates` doc — so an update naming an
    /// id this application holds no logging option under is refused, with the sentence
    /// `Err` carries.
    pub fn apply_logging_option_updates(
        &mut self,
        updates: &[CloudWatchLoggingOptionUpdate],
    ) -> Result<(), String> {
        let name = self.name.clone();
        for update in updates {
            let described = self
                .cloud_watch_logging_options
                .iter_mut()
                .find(|described| {
                    described.cloud_watch_logging_option_id.as_deref()
                        == Some(&update.cloud_watch_logging_option_id)
                })
                .ok_or_else(|| {
                    missing_update_target_message(
                        "CloudWatch logging option",
                        &update.cloud_watch_logging_option_id,
                        &name,
                    )
                })?;
            if let Some(arn) = update.log_stream_arn_update.clone() {
                described.log_stream_arn = arn;
            }
        }
        Ok(())
    }

    /// Apply a `RunConfigurationUpdate`: the parameters the application's next run
    /// starts with, which `DescribeApplication` reports back as its
    /// `RunConfigurationDescription`.
    pub fn apply_run_configuration_update(&mut self, update: &RunConfigurationUpdate) {
        let cfg = self.configuration.get_or_insert_with(Default::default);
        let described = cfg
            .run_configuration_description
            .get_or_insert_with(RunConfigurationDescription::default);
        if let Some(flink) = update.flink_run_configuration.clone() {
            described.flink_run_configuration_description = Some(flink);
        }
        if let Some(restore) = update.application_restore_configuration.clone() {
            described.application_restore_configuration_description = Some(restore);
        }
    }
}

/// The ceiling on how many applications this store holds, and on how many operation
/// records one application retains: the two caps that bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create is the only thing that adds one, so the store needs a maximum entry count —
/// neither a TTL nor an eviction policy is available to it, because `DescribeApplication`
/// must still answer what `CreateApplication` created. `CreateApplication` refuses past
/// [`MAX_APPLICATIONS`] with the service's own modelled `LimitExceededException`
/// (HTTP 400, "The number of allowed resources has been exceeded"), which the model
/// declares on `CreateApplication` and on no read. The operation log, which no caller can
/// drain, is trimmed oldest-first at [`MAX_OPERATIONS_PER_APPLICATION`] instead: losing
/// the oldest completed `StartApplication`/`StopApplication` record is not a refusal a
/// real client would see, and `Start`/`Stop` declare no quota error.
///
/// Both numbers are the emulator's own bound, not a published AWS default: AWS publishes
/// the application-per-region quota as adjustable, so no single value is the contract,
/// and the caps are set high enough that no ordinary workflow meets one.
pub const MAX_APPLICATIONS: usize = 5_000;

/// See [`MAX_APPLICATIONS`].
pub const MAX_OPERATIONS_PER_APPLICATION: usize = 200;

/// The ceiling on how many snapshots one application retains. Same reasoning as
/// [`MAX_APPLICATIONS`]: a snapshot is held for the life of the process and only a caller
/// adds one, so the count needs a maximum — and `CreateApplicationSnapshot` declares the
/// modelled `LimitExceededException` (HTTP 400) for a caller that reaches it, so the
/// refusal is the service's own rather than a trim a reader could not explain.
pub const MAX_SNAPSHOTS_PER_APPLICATION: usize = 1_000;

/// The ceiling on how many SUPERSEDED versions one application retains, beside the version
/// in force. Same reasoning as [`MAX_OPERATIONS_PER_APPLICATION`]: every accepted update
/// adds a record no caller can drain, so the history is trimmed oldest-first rather than
/// growing for the life of the process — and no operation that mints a version declares a
/// quota error, so a refusal would be one no real client sees. The number is the
/// emulator's own.
pub const MAX_RETAINED_VERSIONS: usize = 1_000;

/// How many external destinations ONE SQL-based application writes to.
///
/// `AddApplicationOutput` documents that "you can configure one or more outputs for your
/// application" and neither it nor the `Outputs` list states a ceiling
/// (`smithy_query(query="shape",shape="Outputs")`), so the NUMBER here is the emulator's
/// own: every accepted add appends one description to the stored application, so without a
/// ceiling a caller repeating the operation grows one application's configuration — and the
/// answer that reports it — without limit. Same reasoning as [`MAX_SNAPSHOTS_PER_APPLICATION`]
/// and [`MAX_OPERATIONS_PER_APPLICATION`]; unlike those it is REFUSED rather than trimmed,
/// because a destination dropped to make room would be one the caller believes its
/// application still writes to.
pub const MAX_OUTPUTS_PER_APPLICATION: usize = 3;

/// The ceiling on user-defined application tags, from the `CreateApplication$Tags`,/// `TagResource` and `TooManyTagsException` docs: "the maximum number of application tags
/// includes system tags. The maximum number of user-defined application tags is 50". The
/// modelled `Tags` bound (`@length 1..200`) counts the system tags too, so this ceiling is
/// the service's own rule and every operation that assigns a tag holds to it.
pub const MAX_USER_TAGS: usize = 50;

/// Per-service state for com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523. A handler reaches it through [`service_state`].
///
/// `applications` is a `BTreeMap`, so `ListApplications` walks a stable, name-ordered
/// sequence and a pagination token can resume from a name rather than an index. Its entry
/// count is bounded by [`MAX_APPLICATIONS`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State {
    pub applications: BTreeMap<String, Application>,
    /// The account's ENHANCED snapshots, by `SnapshotName`. An enhanced snapshot is not
    /// scoped to an application (`crate::state::enhanced_snapshots`), so it is held here
    /// rather than on one — and the name is unique across the account, which is why the map
    /// is keyed by it. Bounded by [`MAX_ENHANCED_SNAPSHOTS`].
    pub snapshots_v2: BTreeMap<String, EnhancedSnapshot>,
    /// Bumped for every server-assigned id, so two ids minted in the same instant differ.
    id_sequence: u64,
}

impl ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State {
    /// Apply the status change the clock owes `name`, if there is one.
    ///
    /// Call this FIRST in a handler that names a single application, so the operation
    /// decides against the status that application has now. It touches only that entry,
    /// so the work a request does under the state lock does not grow with the store.
    pub fn settle_named(&mut self, name: &str) {
        let now = Instant::now();
        if let Some(app) = self.applications.get_mut(name) {
            app.settle(now);
        }
    }

    /// Apply every status change the clock owes. For `ListApplications`, which reports on
    /// every stored application at once; the walk is bounded by [`MAX_APPLICATIONS`].
    pub fn settle(&mut self) {
        let now = Instant::now();
        for app in self.applications.values_mut() {
            app.settle(now);
        }
    }

    /// A fresh server-assigned id in the 8-4-4-4-12 hex form AWS uses for the ids this
    /// API hands out (`OperationId`, `InputId`, …).
    ///
    /// Unique by construction: the digest covers a monotonically increasing sequence
    /// number, so no two ids from one store collide, and the form satisfies `Id`
    /// (`^[a-zA-Z0-9_.-]+$`, 1..50).
    pub fn mint_id(&mut self, kind: &str) -> String {
        self.id_sequence += 1;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        hex_id(&format!("{kind}:{}:{nanos}", self.id_sequence))
    }

    /// A fresh `ConditionalToken`: "a value you use to implement strong concurrency for
    /// application updates" (the member's doc), minted for the version in force.
    ///
    /// A token is OPAQUE — a caller only ever hands the value it was given back — so it
    /// carries none of the structure an id does and is NOT written in the 8-4-4-4-12 form
    /// [`Self::mint_id`] gives a server-assigned id: a value wearing that punctuation
    /// invites a client to read it as a UUID, which is not what this value is. It is
    /// unique by the same construction (the digest covers a monotonically increasing
    /// sequence number, so no two tokens from one store collide, and no two applications
    /// share one), and satisfies `ConditionalToken`'s `^[a-zA-Z0-9-_+/=]+$` pattern and
    /// 1..512 length (`smithy_query(query="shape",shape="ConditionalToken")`).
    pub fn mint_conditional_token(&mut self) -> String {
        self.id_sequence += 1;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        opaque_token(&format!("ConditionalToken:{}:{nanos}", self.id_sequence))
    }

    /// Replace `name`'s maintenance window start time and answer the window as it now
    /// stands, or `None` when no application is stored under that name.
    ///
    /// The end time is not stored: it follows from the start
    /// ([`maintenance_window_end`]), so one rule decides how long every application's
    /// window is, however the start was set.
    pub fn set_maintenance_window(
        &mut self,
        name: &str,
        start_time: String,
        now: DateTime,
    ) -> Option<ApplicationMaintenanceConfigurationDescription> {
        let app = self.applications.get_mut(name)?;
        app.maintenance_window_start = start_time;
        // The stored application changed, and `LastUpdateTimestamp` reports when it last
        // did. The application's VERSION does not move: a version is the configuration
        // the application runs under, and when maintenance runs on it is not part of that.
        app.last_update_timestamp = now;
        Some(app.maintenance_configuration())
    }

    /// Record a new operation on `name` and return its `OperationId`.
    ///
    /// The id is the primary key of the stored [`AppOperation`], never a loose value. The
    /// log is capped at [`MAX_OPERATIONS_PER_APPLICATION`] and trimmed oldest-first, so
    /// repeated Start/Stop calls cannot grow one application's record without limit.
    pub fn record_operation(&mut self, name: &str, operation: &str, now: DateTime) -> String {
        self.push_operation(name, operation, OPERATION_IN_PROGRESS, now, None)
    }

    /// Record an operation on `name` that is ALREADY complete: one whose whole effect
    /// landed inside the call that made it, so the clock owes it nothing and a reader must
    /// never see it outstanding.
    pub fn record_completed_operation(
        &mut self,
        name: &str,
        operation: &str,
        now: DateTime,
    ) -> String {
        self.push_operation(name, operation, OPERATION_SUCCESSFUL, now, Some(now))
    }

    /// Append one operation record to `name`'s log and answer its `OperationId`.
    fn push_operation(
        &mut self,
        name: &str,
        operation: &str,
        status: &str,
        start_time: DateTime,
        end_time: Option<DateTime>,
    ) -> String {
        let operation_id = self.mint_id(operation);
        if let Some(app) = self.applications.get_mut(name) {
            app.operations.push(AppOperation {
                operation_id: operation_id.clone(),
                operation: operation.to_string(),
                status: status.to_string(),
                start_time,
                end_time,
            });
            if app.operations.len() > MAX_OPERATIONS_PER_APPLICATION {
                let excess = app.operations.len() - MAX_OPERATIONS_PER_APPLICATION;
                app.operations.drain(..excess);
            }
        }
        operation_id
    }

    /// Move `name` into `transient_status` now and owe `settled_status` to the clock.
    pub fn begin_transition(
        &mut self,
        name: &str,
        transient_status: &str,
        settled_status: &str,
        operation_id: String,
        now: DateTime,
    ) {
        // The change becomes due `TRANSITION_DELAY` after this call; it is stamped as
        // the next whole second, the resolution this service reports timestamps at.
        let due_wall = DateTime::from_secs(now.secs() + 1);
        if let Some(app) = self.applications.get_mut(name) {
            app.status = transient_status.to_string();
            app.pending = Some(PendingTransition {
                to_status: settled_status.to_string(),
                operation_id,
                due_at: Instant::now() + TRANSITION_DELAY,
                due_wall,
            });
        }
    }

    /// Take a snapshot of `application`'s current version: stored `CREATING`, with `READY`
    /// owed to the clock [`TRANSITION_DELAY`] from now, so a caller reading straight back
    /// still sees the transient status.
    ///
    /// The span is this crate's ONE clock-owed span — the same one `StartApplication` and
    /// `StopApplication` already settle on — rather than a second one of its own, so the
    /// emulator has a single answer to "how long does a transition this service starts stay
    /// pending", and the wire fixtures that poll one status poll them all the same way.
    ///
    /// The version and the runtime recorded are the application's as it stands now, so the
    /// `SnapshotDetails` a reader gets is about the application that was snapshotted and
    /// not about whatever it is later updated to.
    pub fn begin_snapshot(&mut self, application: &str, snapshot_name: &str, now: DateTime) {
        if let Some(app) = self.applications.get_mut(application) {
            app.snapshots.insert(
                snapshot_name.to_string(),
                Snapshot {
                    name: snapshot_name.to_string(),
                    status: SNAPSHOT_CREATING.to_string(),
                    application_version_id: app.version_id,
                    creation_timestamp: now,
                    runtime_environment: app.runtime_environment.clone(),
                    ready_at: Some(Instant::now() + TRANSITION_DELAY),
                },
            );
        }
    }

    /// Record a new version of `application` and return it, retaining `superseded` as the
    /// record of the version it leaves.
    ///
    /// Every accepted configuration update results in a new version of the application
    /// (the `AddApplicationInput` doc says so for adding a streaming source, and
    /// `ApplicationDetail$ApplicationVersionId` states it for the API as a whole), so the
    /// version the caller's next optimistic-concurrency token must carry advances by one,
    /// the version it moved on from is remembered as `ApplicationVersionUpdatedFrom`, and
    /// the `ConditionalToken` — which names the CURRENT version — is minted afresh.
    ///
    /// A version the application has left stays READABLE — `ListApplicationVersions`
    /// "lists all the versions for the specified application" and
    /// `DescribeApplicationVersion` describes one of them — so `superseded` is the detail
    /// the outgoing version ran under, taken BEFORE the caller's changes were merged, and
    /// is kept under the id it carries. The retained history is bounded by
    /// [`MAX_RETAINED_VERSIONS`] and trimmed oldest-first.
    ///
    /// Answers `None` when no application is stored under `application`, so a caller
    /// cannot read a version that was never recorded.
    pub fn advance_version(
        &mut self,
        application: &str,
        now: DateTime,
        superseded: ApplicationDetail,
    ) -> Option<i64> {
        let conditional_token = self.mint_conditional_token();
        let app = self.applications.get_mut(application)?;
        app.versions.insert(superseded.application_version_id, superseded);
        while app.versions.len() > MAX_RETAINED_VERSIONS {
            let Some(oldest) = app.versions.keys().next().copied() else {
                break;
            };
            app.versions.remove(&oldest);
        }
        app.version_updated_from = Some(app.version_id);
        app.version_id += 1;
        app.version_create_timestamp = now;
        app.last_update_timestamp = now;
        app.conditional_token = Some(conditional_token);
        Some(app.version_id)
    }

    /// The application the `ResourceARN` of a tagging call names, for a caller that reads
    /// its tags.
    ///
    /// The ARN is resolved through [`application_name_from_arn`] and the stored ARN then
    /// has to EQUAL the one supplied, so an ARN naming this account's application in
    /// another Region resolves to nothing rather than to the like-named application of the
    /// Region the caller happens to be signing for.
    pub fn application_by_arn(&self, arn: &str) -> Option<&Application> {
        self.applications
            .get(application_name_from_arn(arn)?)
            .filter(|application| application.arn == arn)
    }

    /// The same lookup as [`Self::application_by_arn`], for a caller that WRITES the tags
    /// of the application the ARN names.
    pub fn application_by_arn_mut(&mut self, arn: &str) -> Option<&mut Application> {
        self.applications
            .get_mut(application_name_from_arn(arn)?)
            .filter(|application| application.arn == arn)
    }

    /// Store `input` as `application`'s streaming source and move the application on to a
    /// new version; answers that version and the input descriptions it now carries.
    ///
    /// The whole accepted mutation of `AddApplicationInput`, in one place beside the other
    /// writers of this store: the description (with the id and in-app stream names the
    /// service assigns), the version advance, and the answer read back out of what was
    /// stored. `None` when no application is stored under `application`.
    pub fn add_input(
        &mut self,
        application: &str,
        input: &Input,
        now: DateTime,
    ) -> Option<(i64, Vec<InputDescription>)> {
        let description = self.describe_input(input);
        // The detail of the version this add LEAVES: read before the input is stored, so
        // the record kept for it describes the application as that version ran.
        let superseded = self.applications.get(application)?.detail();
        let version_id = self.advance_version(application, now, superseded)?;
        let app = self.applications.get_mut(application)?;
        let descriptions = app
            .configuration
            .get_or_insert_with(ApplicationConfigurationDescription::default)
            .sql_application_configuration_description
            .get_or_insert_with(SqlApplicationConfigurationDescription::default)
            .input_descriptions
            .get_or_insert_with(Vec::new);
        descriptions.push(description);
        Some((version_id, descriptions.clone()))
    }

    /// The `ApplicationConfigurationDescription` an application RUNS under: every member
    /// the creator set, plus the ids and derived values the service assigns, completed
    /// with the service defaults for everything the request left out
    /// ([`complete_with_service_defaults`]).
    ///
    /// `cfg` is `None` when the request carried no `ApplicationConfiguration` at all,
    /// which is not an application with no configuration: the service still runs it under
    /// its defaults, so the description is materialised either way.
    pub fn describe_configuration(
        &mut self,
        cfg: Option<&ApplicationConfiguration>,
        runtime_environment: &str,
        application_mode: &str,
    ) -> ApplicationConfigurationDescription {
        let mut description = match cfg {
            Some(cfg) => self.describe_supplied_configuration(cfg),
            None => ApplicationConfigurationDescription::default(),
        };
        complete_with_service_defaults(&mut description, runtime_environment, application_mode);
        description
    }

    /// The description of one streaming source, with the id and the in-app stream names
    /// the service assigns it.
    ///
    /// Shared by `CreateApplication`'s `SqlApplicationConfiguration` and by
    /// `AddApplicationInput`, so an input added after the fact is described exactly like
    /// one supplied at create time.
    pub fn describe_input(&mut self, input: &Input) -> InputDescription {
        InputDescription {
            input_id: Some(self.mint_id("Input")),
            name_prefix: Some(input.name_prefix.clone()),
            // The count comes from the request, so it is bounded to the modelled range
            // before it sizes the name list derived from it.
            in_app_stream_names: in_app_stream_names(
                &input.name_prefix,
                bounded_input_parallelism(
                    input.input_parallelism.as_ref().and_then(|p| p.count),
                ),
            ),
            input_processing_configuration_description: input
                .input_processing_configuration
                .as_ref()
                .map(|ipc| InputProcessingConfigurationDescription {
                    input_lambda_processor_description: Some(InputLambdaProcessorDescription {
                        resource_arn: ipc.input_lambda_processor.resource_arn.clone(),
                        role_arn: None,
                    }),
                }),
            kinesis_streams_input_description: input.kinesis_streams_input.as_ref().map(|k| {
                KinesisStreamsInputDescription {
                    resource_arn: k.resource_arn.clone(),
                    role_arn: None,
                }
            }),
            kinesis_firehose_input_description: input.kinesis_firehose_input.as_ref().map(|k| {
                KinesisFirehoseInputDescription {
                    resource_arn: k.resource_arn.clone(),
                    role_arn: None,
                }
            }),
            input_schema: Some(input.input_schema.clone()),
            input_parallelism: input.input_parallelism.clone(),
            // Set by the RunConfiguration a StartApplication call carries.
            input_starting_position_configuration: None,
        }
    }

    /// The description of one output configuration — one in-application stream and the
    /// destination its data is written to — with the id the service assigns it.
    ///
    /// Shared by `CreateApplication`'s `SqlApplicationConfiguration` and by
    /// `AddApplicationOutput`, so a destination added after the fact is described exactly
    /// like one supplied at create time.
    pub fn describe_output(&mut self, output: &Output) -> OutputDescription {
        OutputDescription {
            output_id: Some(self.mint_id("Output")),
            name: Some(output.name.clone()),
            kinesis_streams_output_description: output.kinesis_streams_output.as_ref().map(|k| {
                KinesisStreamsOutputDescription {
                    resource_arn: k.resource_arn.clone(),
                    role_arn: None,
                }
            }),
            kinesis_firehose_output_description: output.kinesis_firehose_output.as_ref().map(|k| {
                KinesisFirehoseOutputDescription {
                    resource_arn: k.resource_arn.clone(),
                    role_arn: None,
                }
            }),
            lambda_output_description: output.lambda_output.as_ref().map(|l| {
                LambdaOutputDescription {
                    resource_arn: l.resource_arn.clone(),
                    role_arn: None,
                }
            }),
            destination_schema: Some(output.destination_schema.clone()),
        }
    }

    /// Store `output` as one of `application`'s destinations and move the application on to
    /// a new version; answers that version and the output descriptions it now carries.
    ///
    /// The whole accepted mutation of `AddApplicationOutput`, in one place beside the other
    /// writers of this store: the description (with the id the service assigns it), the
    /// version advance, and the answer read back out of what was stored. The list GROWS —
    /// "You can configure one or more outputs for your application" (the operation's doc) —
    /// so a second destination joins the first rather than replacing it, up to
    /// [`MAX_OUTPUTS_PER_APPLICATION`].
    ///
    /// `None` when no application is stored under `application`, and when the one stored is
    /// already configured up to that ceiling — the second line of defence behind the
    /// handler's own check, so no writer can grow one application's configuration past it.
    /// Nothing is written and no version is consumed on either.
    pub fn add_output(
        &mut self,
        application: &str,
        output: &Output,
        now: DateTime,
    ) -> Option<(i64, Vec<OutputDescription>)> {
        let stored = self.applications.get(application)?;
        if stored.described_outputs() >= MAX_OUTPUTS_PER_APPLICATION {
            return None;
        }
        let description = self.describe_output(output);
        // The detail of the version this add LEAVES: read before the output is stored, so
        // the record kept for it describes the application as that version ran.
        let superseded = self.applications.get(application)?.detail();
        let version_id = self.advance_version(application, now, superseded)?;
        let app = self.applications.get_mut(application)?;
        let descriptions = app
            .configuration
            .get_or_insert_with(ApplicationConfigurationDescription::default)
            .sql_application_configuration_description
            .get_or_insert_with(SqlApplicationConfigurationDescription::default)
            .output_descriptions
            .get_or_insert_with(Vec::new);
        descriptions.push(description);
        Some((version_id, descriptions.clone()))
    }

    /// Drop the output configuration `application` carries under `output_id` and move the
    /// application on to a new version; answers that version and the descriptions that
    /// REMAIN.
    ///
    /// `None` when no application is stored under that name, or when the one stored carries
    /// no output under that id — so nothing is written and no version is consumed for a
    /// request naming a destination that was never configured.
    pub fn remove_output(
        &mut self,
        application: &str,
        output_id: &str,
        now: DateTime,
    ) -> Option<(i64, Vec<OutputDescription>)> {
        let app = self.applications.get(application)?;
        let described = app
            .configuration
            .as_ref()?
            .sql_application_configuration_description
            .as_ref()?
            .output_descriptions
            .as_ref()?;
        let position = described
            .iter()
            .position(|output| output.output_id.as_deref() == Some(output_id))?;
        let superseded = app.detail();
        let version_id = self.advance_version(application, now, superseded)?;
        let descriptions = self
            .applications
            .get_mut(application)?
            .configuration
            .as_mut()?
            .sql_application_configuration_description
            .as_mut()?
            .output_descriptions
            .as_mut()?;
        descriptions.remove(position);
        Some((version_id, descriptions.clone()))
    }

    /// The description of the members a request actually supplied, before the service
    /// defaults complete it.
    fn describe_supplied_configuration(
        &mut self,
        cfg: &ApplicationConfiguration,
    ) -> ApplicationConfigurationDescription {
        let sql = cfg.sql_application_configuration.as_ref().map(|sql| {
            SqlApplicationConfigurationDescription {
                input_descriptions: sql.inputs.as_ref().map(|inputs| {
                    inputs
                        .iter()
                        .map(|input| self.describe_input(input))
                        .collect()
                }),
                output_descriptions: sql.outputs.as_ref().map(|outputs| {
                    outputs
                        .iter()
                        .map(|output| self.describe_output(output))
                        .collect()
                }),
                reference_data_source_descriptions: sql.reference_data_sources.as_ref().map(
                    |sources| {
                        sources
                            .iter()
                            // A reference data source is described through its S3
                            // location, whose `BucketARN` (`^arn:`) and `FileKey`
                            // (len 1..1024) the description REQUIRES. A source that
                            // supplied neither cannot be described, and empty strings
                            // would break both constraints, so it is left out rather
                            // than reported with placeholder members.
                            .filter_map(|source| {
                                let s3 = source.s3_reference_data_source.as_ref()?;
                                Some(ReferenceDataSourceDescription {
                                    reference_id: self.mint_id("Reference"),
                                    table_name: source.table_name.clone(),
                                    s3_reference_data_source_description:
                                        S3ReferenceDataSourceDescription {
                                            bucket_arn: s3.bucket_arn.clone()?,
                                            file_key: s3.file_key.clone()?,
                                            reference_role_arn: None,
                                        },
                                    reference_schema: Some(source.reference_schema.clone()),
                                })
                            })
                            .collect()
                    },
                ),
            }
        });

        let vpc_configuration_descriptions = cfg.vpc_configurations.as_ref().map(|vpcs| {
            vpcs.iter()
                .map(|vpc| VpcConfigurationDescription {
                    vpc_configuration_id: self.mint_id("VpcConfiguration"),
                    // The VPC a set of subnets belongs to. This emulator holds no VPC
                    // inventory, so the id is derived from the subnet set itself: the
                    // same subnets always report the same VPC, which is the invariant a
                    // caller can rely on.
                    vpc_id: vpc_id_for(&vpc.subnet_ids),
                    subnet_ids: vpc.subnet_ids.clone(),
                    security_group_ids: vpc.security_group_ids.clone(),
                })
                .collect()
        });

        ApplicationConfigurationDescription {
            sql_application_configuration_description: sql,
            application_code_configuration_description: cfg
                .application_code_configuration
                .as_ref()
                .map(|code| ApplicationCodeConfigurationDescription {
                    code_content_type: code.code_content_type.clone(),
                    code_content_description: code.code_content.as_ref().map(|content| {
                        CodeContentDescription {
                            text_content: content.text_content.clone(),
                            // AWS reports the code's MD5 digest. Inline code is in hand,
                            // but S3-hosted code is not, and this emulator has no MD5
                            // implementation to report either with, so the member is
                            // left absent rather than filled with another digest.
                            code_md5: None,
                            code_size: content
                                .text_content
                                .as_ref()
                                .map(|t| t.len() as i64)
                                .or_else(|| content.zip_file_content.as_ref().map(|z| z.len() as i64)),
                            s3_application_code_location_description: content
                                .s3_content_location
                                .as_ref()
                                .map(|s3| S3ApplicationCodeLocationDescription {
                                    bucket_arn: s3.bucket_arn.clone(),
                                    file_key: s3.file_key.clone(),
                                    object_version: s3.object_version.clone(),
                                }),
                        }
                    }),
                }),
            // Filled by StartApplication's RunConfiguration, not at create time.
            run_configuration_description: None,
            flink_application_configuration_description: cfg
                .flink_application_configuration
                .as_ref()
                .map(|flink| FlinkApplicationConfigurationDescription {
                    checkpoint_configuration_description: flink
                        .checkpoint_configuration
                        .as_ref()
                        .map(|c| CheckpointConfigurationDescription {
                            configuration_type: Some(c.configuration_type.clone()),
                            checkpointing_enabled: c.checkpointing_enabled,
                            checkpoint_interval: c.checkpoint_interval,
                            min_pause_between_checkpoints: c.min_pause_between_checkpoints,
                        }),
                    monitoring_configuration_description: flink
                        .monitoring_configuration
                        .as_ref()
                        .map(|m| MonitoringConfigurationDescription {
                            configuration_type: Some(m.configuration_type.clone()),
                            metrics_level: m.metrics_level.clone(),
                            log_level: m.log_level.clone(),
                        }),
                    parallelism_configuration_description: flink
                        .parallelism_configuration
                        .as_ref()
                        .map(|p| ParallelismConfigurationDescription {
                            configuration_type: Some(p.configuration_type.clone()),
                            parallelism: p.parallelism,
                            parallelism_per_kpu: p.parallelism_per_kpu,
                            // The parallelism in force right now: the configured value
                            // until autoscaling moves it, and autoscaling is not modelled.
                            current_parallelism: p.parallelism,
                            auto_scaling_enabled: p.auto_scaling_enabled,
                        }),
                    // The Flink job plan exists only once a job has been submitted to a
                    // running cluster; this emulator runs no Flink job.
                    job_plan_description: None,
                }),
            environment_property_descriptions: cfg.environment_properties.as_ref().map(|env| {
                EnvironmentPropertyDescriptions {
                    property_group_descriptions: Some(env.property_groups.clone()),
                }
            }),
            application_snapshot_configuration_description: cfg
                .application_snapshot_configuration
                .as_ref()
                .map(|s| ApplicationSnapshotConfigurationDescription {
                    snapshots_enabled: s.snapshots_enabled,
                    // The mode the configuration SELECTED, which is what the application's
                    // snapshots are then taken under; a configuration that named none is
                    // completed with the service's own value
                    // ([`complete_with_service_defaults`]).
                    snapshot_mode: s.snapshot_mode.clone(),
                }),
            application_system_rollback_configuration_description: cfg
                .application_system_rollback_configuration
                .as_ref()
                .map(|s| ApplicationSystemRollbackConfigurationDescription {
                    rollback_enabled: s.rollback_enabled,
                }),
            vpc_configuration_descriptions,
            zeppelin_application_configuration_description: cfg
                .zeppelin_application_configuration
                .as_ref()
                .map(|z| ZeppelinApplicationConfigurationDescription {
                    monitoring_configuration_description: ZeppelinMonitoringConfigurationDescription {
                        // The verbosity the notebook's CloudWatch Logs are written at.
                        // `ZeppelinApplicationConfigurationDescription` declares this
                        // description REQUIRED, so it reports the level in force — the
                        // one the create configured, else the service default — rather
                        // than nothing at all for a notebook that configured no
                        // monitoring.
                        log_level: Some(z.monitoring_configuration.as_ref().map_or_else(
                            || DEFAULT_LOG_LEVEL.to_string(),
                            |m| m.log_level.clone(),
                        )),
                    },
                    catalog_configuration_description: z.catalog_configuration.as_ref().map(|c| {
                        CatalogConfigurationDescription {
                            glue_data_catalog_configuration_description:
                                GlueDataCatalogConfigurationDescription {
                                    database_arn: c
                                        .glue_data_catalog_configuration
                                        .database_arn
                                        .clone(),
                                },
                        }
                    }),
                    deploy_as_application_configuration_description: z
                        .deploy_as_application_configuration
                        .as_ref()
                        .map(|d| DeployAsApplicationConfigurationDescription {
                            s3_content_location_description: S3ContentBaseLocationDescription {
                                bucket_arn: d.s3_content_location.bucket_arn.clone(),
                                base_path: d.s3_content_location.base_path.clone(),
                            },
                        }),
                    custom_artifacts_configuration_description: z
                        .custom_artifacts_configuration
                        .as_ref()
                        .map(|artifacts| {
                            artifacts
                                .iter()
                                .map(|a| CustomArtifactConfigurationDescription {
                                    artifact_type: Some(a.artifact_type.clone()),
                                    s3_content_location_description: a
                                        .s3_content_location
                                        .clone(),
                                    maven_reference_description: a.maven_reference.clone(),
                                })
                                .collect()
                        }),
                }),
            application_encryption_configuration_description: cfg
                .application_encryption_configuration
                .as_ref()
                .map(|e| ApplicationEncryptionConfigurationDescription {
                    key_id: e.key_id.clone(),
                    key_type: e.key_type.clone(),
                }),
        }
    }

    /// The stored descriptions for the CloudWatch log streams a create call attached.
    pub fn describe_logging_options(
        &mut self,
        options: &[CloudWatchLoggingOption],
    ) -> Vec<CloudWatchLoggingOptionDescription> {
        options
            .iter()
            .map(|option| CloudWatchLoggingOptionDescription {
                cloud_watch_logging_option_id: Some(self.mint_id("CloudWatchLoggingOption")),
                log_stream_arn: option.log_stream_arn.clone(),
                role_arn: None,
            })
            .collect()
    }

    /// Attach `option` as one of `application`'s CloudWatch log streams and move the
    /// application on to a new version; answers that version and the logging option
    /// descriptions it now carries.
    ///
    /// The whole accepted mutation of `AddApplicationCloudWatchLoggingOption`, in one place
    /// beside the other writers of this store: the description (with the id the service
    /// assigns it), the version advance, and the answer read back out of what was stored.
    /// `None` when no application is stored under `application`.
    pub fn add_logging_option(
        &mut self,
        application: &str,
        option: &CloudWatchLoggingOption,
        now: DateTime,
    ) -> Option<(i64, Vec<CloudWatchLoggingOptionDescription>)> {
        // Described exactly as a create describes the log streams it attaches, so one
        // added after the fact carries the same id form and the same members.
        let described = self.describe_logging_options(std::slice::from_ref(option));
        // The detail of the version this add LEAVES: read before the option is stored, so
        // the record kept for it describes the application as that version ran.
        let superseded = self.applications.get(application)?.detail();
        let version_id = self.advance_version(application, now, superseded)?;
        let app = self.applications.get_mut(application)?;
        app.cloud_watch_logging_options.extend(described);
        Some((version_id, app.cloud_watch_logging_options.clone()))
    }

    /// Detach the CloudWatch log stream `application` holds under `option_id` and move the
    /// application on to a new version; answers that version and the descriptions that
    /// REMAIN.
    ///
    /// `None` when no application is stored under that name, or when the one stored holds
    /// no logging option under that id — so nothing is written and no version is consumed
    /// for a request naming an option that was never attached.
    pub fn remove_logging_option(
        &mut self,
        application: &str,
        option_id: &str,
        now: DateTime,
    ) -> Option<(i64, Vec<CloudWatchLoggingOptionDescription>)> {
        let app = self.applications.get(application)?;
        let position = app.cloud_watch_logging_options.iter().position(|described| {
            described.cloud_watch_logging_option_id.as_deref() == Some(option_id)
        })?;
        let superseded = app.detail();
        let version_id = self.advance_version(application, now, superseded)?;
        let app = self.applications.get_mut(application)?;
        app.cloud_watch_logging_options.remove(position);
        Some((version_id, app.cloud_watch_logging_options.clone()))
    }
}

/// The two `ConfigurationType` values, spelled as the model's enum spells them
/// (`smithy_query(query="shape",shape="ConfigurationType")`).
const CONFIGURATION_DEFAULT: &str = "DEFAULT";
const CONFIGURATION_CUSTOM: &str = "CUSTOM";

/// The checkpointing intervals a `DEFAULT` checkpoint configuration runs with, from the
/// `CheckpointConfiguration$ConfigurationType` doc: "If this value is set to `DEFAULT`,
/// the application will use the following values, even if they are set to other values
/// using APIs or application code: CheckpointingEnabled true, CheckpointInterval 60000,
/// MinPauseBetweenCheckpoints 5000". Whether checkpointing itself is on is the one value
/// of that table the application's KIND decides ([`ApplicationDefaults`]); milliseconds,
/// as both members are documented.
const DEFAULT_CHECKPOINT_INTERVAL: i64 = 60_000;
/// See [`DEFAULT_CHECKPOINT_INTERVAL`].
const DEFAULT_MIN_PAUSE_BETWEEN_CHECKPOINTS: i64 = 5_000;

/// The default values that are NOT the same for every application this service runs,
/// because the three features they switch — checkpointing, snapshots and the automatic
/// scaling of the parallelism — are features of a Managed Service for Apache Flink
/// application: `CheckpointConfiguration$CheckpointingEnabled` "describes whether
/// checkpointing is enabled for a Managed Service for Apache Flink application",
/// `ApplicationSnapshotConfiguration$SnapshotsEnabled` "describes whether snapshots are
/// enabled for a Managed Service for Apache Flink application" and
/// `ParallelismConfiguration$AutoScalingEnabled` "describes whether the Managed Service for
/// Apache Flink service can increase the parallelism of the application in response to
/// increased throughput"
/// (`smithy_query(query="doc",shape="CheckpointConfiguration",member="CheckpointingEnabled")`
/// / `shape="ApplicationSnapshotConfiguration",member="SnapshotsEnabled"` /
/// `shape="ParallelismConfiguration",member="AutoScalingEnabled"`). An application the
/// service runs INTERACTIVELY is a Studio notebook rather than such an application
/// (`CreateApplication$ApplicationMode`: "Use the STREAMING mode to create a Managed Service
/// for Apache Flink application. To create a Managed Service for Apache Flink Studio
/// notebook, use the INTERACTIVE mode"), so none of them runs for it unless its own
/// configuration switched it on, and its defaults are the off ones.
#[derive(Debug, Clone, Copy)]
struct ApplicationDefaults {
    /// The `CheckpointingEnabled` of a checkpoint configuration the caller left to the
    /// service.
    checkpointing_enabled: bool,
    /// The `SnapshotsEnabled` of an application created with no snapshot configuration.
    snapshots_enabled: bool,
    /// The `AutoScalingEnabled` of a parallelism configuration the caller left to the
    /// service.
    auto_scaling_enabled: bool,
}

/// The defaults of a Managed Service for Apache Flink application — one that STREAMS:
/// checkpointing runs, as the `DEFAULT` checkpoint-configuration table states, snapshots
/// are taken for it, and the service is free to scale its parallelism out in response to
/// load.
const STREAMING_DEFAULTS: ApplicationDefaults = ApplicationDefaults {
    checkpointing_enabled: true,
    snapshots_enabled: true,
    auto_scaling_enabled: true,
};

/// The defaults of an application the service runs INTERACTIVELY (a Studio notebook):
/// none of the three Managed Service for Apache Flink features above runs for it on its
/// own. See [`ApplicationDefaults`].
const INTERACTIVE_DEFAULTS: ApplicationDefaults = ApplicationDefaults {
    checkpointing_enabled: false,
    snapshots_enabled: false,
    auto_scaling_enabled: false,
};

/// Whether an application of this runtime and mode is a Studio notebook rather than a
/// Managed Service for Apache Flink application — the two kinds of application this API
/// creates (`CreateApplication$ApplicationMode`: "Use the STREAMING mode to create a
/// Managed Service for Apache Flink application. To create a Managed Service for Apache
/// Flink Studio notebook, use the INTERACTIVE mode").
///
/// What decides is the KIND of application rather than one spelled-out runtime value: the
/// mode the application runs in, which for an application whose creator named none is the
/// one its runtime implies ([`default_application_mode`]), so every Zeppelin runtime —
/// present and future — is read as the notebook it is.
fn is_studio_notebook(runtime_environment: &str, application_mode: &str) -> bool {
    application_mode == APPLICATION_MODE_INTERACTIVE || is_notebook_runtime(runtime_environment)
}

/// The defaults the service runs an application of this runtime and mode under. WHICH
/// defaults apply follows from the KIND of application ([`is_studio_notebook`]).
fn defaults_for(runtime_environment: &str, application_mode: &str) -> ApplicationDefaults {
    if is_studio_notebook(runtime_environment, application_mode) {
        INTERACTIVE_DEFAULTS
    } else {
        STREAMING_DEFAULTS
    }
}

/// The CloudWatch logging a `DEFAULT` monitoring configuration runs with: the service's
/// own published defaults for the two members `CUSTOM` is required to set
/// (`MonitoringConfiguration$ConfigurationType`), each a value of its modelled enum
/// (`smithy_query(query="shape",shape="MetricsLevel")` / `shape="LogLevel"`).
const DEFAULT_METRICS_LEVEL: &str = "APPLICATION";
/// See [`DEFAULT_METRICS_LEVEL`].
const DEFAULT_LOG_LEVEL: &str = "INFO";

/// The parallelism a `DEFAULT` parallelism configuration runs with: the service's own
/// published defaults for the two members `CUSTOM` is required to set that are the same for
/// every application (`ParallelismConfiguration$ConfigurationType`) — one parallel task and
/// one task per KPU. Whether the service may scale the application OUT is the one value of
/// that table the application's KIND decides ([`ApplicationDefaults`]).
const DEFAULT_PARALLELISM: i32 = 1;
/// See [`DEFAULT_PARALLELISM`].
const DEFAULT_PARALLELISM_PER_KPU: i32 = 1;

/// The service does not roll an application back on its own unless its creator asked for
/// it: the default for `ApplicationSystemRollbackConfiguration$RollbackEnabled`.
const DEFAULT_ROLLBACK_ENABLED: bool = false;

/// The snapshot mode an application whose mode was never selected runs under: the legacy
/// marker, which is what the service reports for an application whose create or update
/// named no other mode.
///
/// It is the mode stored ON the application's configuration description
/// ([`complete_with_service_defaults`]) rather than a value the serializer writes, so a
/// description that was stored with a mode keeps reporting that one.
pub const DEFAULT_SNAPSHOT_MODE: &str = "APPLICATION_SNAPSHOTS_LEGACY";

/// An application whose creator named no KMS key is encrypted at rest under the key AWS
/// owns, which is the `KeyType` value the model declares for it
/// (`smithy_query(query="shape",shape="KeyType")`). No key id is reported with it,
/// because the caller has no key of their own to name.
const DEFAULT_KEY_TYPE: &str = "AWS_OWNED_KEY";

/// Complete `description` with the configuration the service runs an application under
/// when the request said nothing about it.
///
/// The rule: an application is always RUNNING under a full configuration, so every
/// sub-configuration the service APPLIES to this kind of application is described — with
/// `ConfigurationType` `DEFAULT` and the documented default values where the request
/// supplied none — and a sub-configuration the request marked `DEFAULT` is described with
/// those same defaults whatever else it carried, which is what `ConfigurationType`
/// `DEFAULT` MEANS. Only a `CUSTOM` sub-configuration reports the caller's values, and the
/// members it left unset still report the defaults the service applies to them. A
/// sub-configuration the service does NOT apply to this kind of application is described
/// only when the application has one of its own, so a description reports the
/// sub-configurations the application HAS rather than one entry per sub-configuration the
/// model declares.
///
/// WHICH defaults those are is the application's own: the two members that switch a
/// Managed Service for Apache Flink feature on are defaulted per kind of application
/// ([`defaults_for`]), so a Studio notebook is not described as running the streaming
/// features it does not run.
fn complete_with_service_defaults(
    description: &mut ApplicationConfigurationDescription,
    runtime_environment: &str,
    application_mode: &str,
) {
    let defaults = defaults_for(runtime_environment, application_mode);
    // Every runtime family this API still creates applications for runs on Apache Flink
    // (`FLINK-` directly, `ZEPPELIN-FLINK-` as a Studio notebook), so all of them run
    // under a Flink configuration. A SQL application does not: its processing is
    // configured by its SQL configuration, so it is described with whatever Flink
    // configuration it was given and nothing is invented for it.
    if !is_sql_runtime(runtime_environment)
        || description
            .flink_application_configuration_description
            .is_some()
    {
        let flink = description
            .flink_application_configuration_description
            .get_or_insert_with(FlinkApplicationConfigurationDescription::default);
        complete_checkpoint_configuration(
            flink
                .checkpoint_configuration_description
                .get_or_insert_with(CheckpointConfigurationDescription::default),
            defaults,
        );
        complete_monitoring_configuration(
            flink
                .monitoring_configuration_description
                .get_or_insert_with(MonitoringConfigurationDescription::default),
        );
        complete_parallelism_configuration(
            flink
                .parallelism_configuration_description
                .get_or_insert_with(ParallelismConfigurationDescription::default),
            defaults,
        );
    }

    let snapshots = description
        .application_snapshot_configuration_description
        .get_or_insert(ApplicationSnapshotConfigurationDescription {
            snapshots_enabled: defaults.snapshots_enabled,
            snapshot_mode: None,
        });
    // Every application's snapshots are taken under one snapshot mode, so a description
    // that carries no mode — because the configuration selected none — reports the one an
    // application that never selected a mode runs under
    // is what `ApplicationSnapshotConfiguration$SnapshotMode` documents: "If you don't
    // specify this parameter, the application uses APPLICATION_SNAPSHOTS_LEGACY"
    // ([`DEFAULT_SNAPSHOT_MODE`]). Stored on the description, so a later read — of the
    // version in force or of a version this application has left — reports the mode that
    // configuration was described with rather than recomputing it.
    if snapshots.snapshot_mode.is_none() {
        snapshots.snapshot_mode = Some(DEFAULT_SNAPSHOT_MODE.to_string());
    }
    // The two application-level features left — rolling an application back on its own
    // and encrypting it at rest — are features of a Managed Service for Apache Flink
    // application (`ApplicationSystemRollbackConfiguration`: "Describes the system
    // rollback configuration for a Managed Service for Apache Flink application"), so the
    // service applies them, and therefore describes them, only for an application of that
    // kind ([`is_studio_notebook`]). A Studio notebook is described with the
    // sub-configurations it actually HAS: one its own configuration supplied is reported
    // as given, and nothing is invented for one it never configured.
    if !is_studio_notebook(runtime_environment, application_mode) {
        if description
            .application_system_rollback_configuration_description
            .is_none()
        {
            description.application_system_rollback_configuration_description =
                Some(ApplicationSystemRollbackConfigurationDescription {
                    rollback_enabled: DEFAULT_ROLLBACK_ENABLED,
                });
        }
        if description
            .application_encryption_configuration_description
            .is_none()
        {
            description.application_encryption_configuration_description =
                Some(ApplicationEncryptionConfigurationDescription {
                    key_id: None,
                    key_type: DEFAULT_KEY_TYPE.to_string(),
                });
        }
    }
}

/// Whether a sub-configuration described with `configuration_type` is the caller's own:
/// `CUSTOM` is, and anything else — `DEFAULT`, or no type at all because the request
/// supplied no such sub-configuration — runs on the service's defaults.
fn is_custom(configuration_type: Option<&String>) -> bool {
    configuration_type.map(String::as_str) == Some(CONFIGURATION_CUSTOM)
}

/// See [`complete_with_service_defaults`]. Whether checkpointing runs for an application
/// that left the decision to the service is the application's own default
/// ([`ApplicationDefaults`]); the two intervals are the same for every application.
fn complete_checkpoint_configuration(
    description: &mut CheckpointConfigurationDescription,
    defaults: ApplicationDefaults,
) {
    if !is_custom(description.configuration_type.as_ref()) {
        *description = CheckpointConfigurationDescription {
            configuration_type: Some(CONFIGURATION_DEFAULT.to_string()),
            checkpointing_enabled: Some(defaults.checkpointing_enabled),
            checkpoint_interval: Some(DEFAULT_CHECKPOINT_INTERVAL),
            min_pause_between_checkpoints: Some(DEFAULT_MIN_PAUSE_BETWEEN_CHECKPOINTS),
        };
        return;
    }
    description.checkpointing_enabled = description
        .checkpointing_enabled
        .or(Some(defaults.checkpointing_enabled));
    description.checkpoint_interval = description
        .checkpoint_interval
        .or(Some(DEFAULT_CHECKPOINT_INTERVAL));
    description.min_pause_between_checkpoints = description
        .min_pause_between_checkpoints
        .or(Some(DEFAULT_MIN_PAUSE_BETWEEN_CHECKPOINTS));
}

/// See [`complete_with_service_defaults`].
fn complete_monitoring_configuration(description: &mut MonitoringConfigurationDescription) {
    if !is_custom(description.configuration_type.as_ref()) {
        *description = MonitoringConfigurationDescription {
            configuration_type: Some(CONFIGURATION_DEFAULT.to_string()),
            metrics_level: Some(DEFAULT_METRICS_LEVEL.to_string()),
            log_level: Some(DEFAULT_LOG_LEVEL.to_string()),
        };
        return;
    }
    description.metrics_level = description
        .metrics_level
        .take()
        .or_else(|| Some(DEFAULT_METRICS_LEVEL.to_string()));
    description.log_level = description
        .log_level
        .take()
        .or_else(|| Some(DEFAULT_LOG_LEVEL.to_string()));
}

/// See [`complete_with_service_defaults`]. Whether the service may scale an application
/// that left the decision to it is the application's own default ([`ApplicationDefaults`]);
/// the parallelism itself is the same for every application. `CurrentParallelism` is the
/// parallelism in force right now, which is the configured one until the service scales the
/// application, and that scaling is not modelled.
fn complete_parallelism_configuration(
    description: &mut ParallelismConfigurationDescription,
    defaults: ApplicationDefaults,
) {
    if !is_custom(description.configuration_type.as_ref()) {
        *description = ParallelismConfigurationDescription {
            configuration_type: Some(CONFIGURATION_DEFAULT.to_string()),
            parallelism: Some(DEFAULT_PARALLELISM),
            parallelism_per_kpu: Some(DEFAULT_PARALLELISM_PER_KPU),
            current_parallelism: Some(DEFAULT_PARALLELISM),
            auto_scaling_enabled: Some(defaults.auto_scaling_enabled),
        };
        return;
    }
    description.parallelism = description.parallelism.or(Some(DEFAULT_PARALLELISM));
    description.parallelism_per_kpu = description
        .parallelism_per_kpu
        .or(Some(DEFAULT_PARALLELISM_PER_KPU));
    description.current_parallelism = description.current_parallelism.or(description.parallelism);
    description.auto_scaling_enabled = description
        .auto_scaling_enabled
        .or(Some(defaults.auto_scaling_enabled));
}

/// The `RuntimeEnvironment` family that names a SQL application, as the enum spells its
/// members (`smithy_query(query="shape",shape="RuntimeEnvironment")`: the SQL family is
/// `SQL-<major>_<minor>`, today `SQL-1_0`, beside the `FLINK-` and `ZEPPELIN-FLINK-`
/// families).
const SQL_RUNTIME_FAMILY: &str = "SQL-";

/// Whether `runtime` names a SQL application's runtime — any version of that family, not
/// one spelled-out value, so a later enum member of the same family is read the same way.
///
/// The value reaching here is either one the enum admits (the boundary validator refuses
/// anything else on the way in) or one read back off a stored application, and the enum's
/// members are the exact strings a client sends, so the comparison is on those bytes.
pub fn is_sql_runtime(runtime: &str) -> bool {
    runtime.starts_with(SQL_RUNTIME_FAMILY)
}

/// How this service words an add that would leave an application writing to more
/// destinations than [`MAX_OUTPUTS_PER_APPLICATION`] admits: the application it was asked
/// of, and the ceiling.
pub fn too_many_outputs_message(application: &str) -> String {
    format!(
        "Application {application} already has {MAX_OUTPUTS_PER_APPLICATION} outputs. A \
         SQL-based application supports at most {MAX_OUTPUTS_PER_APPLICATION} output \
         destinations."
    )
}

/// How this service words a delete that names an output configuration the application's
/// current version does not carry: the `OutputId` the request named.
///
/// An output is addressed by the id the service assigned it, so this is what
/// `DeleteApplicationOutput` answers for an id the stored configuration does not hold —
/// including on an application that carries no SQL configuration at all, which is the
/// answer the behavioural unit
/// `kinesisanalyticsv2.add-application-output-kinesis-stream-on-flink-application-rejected`
/// records for a delete on an application of another runtime family. The sentence is the
/// recorded one, which ends without a full stop — as this service's other refusals about
/// what an application cannot do do ([`ineligible_status_message`]).
pub fn missing_output_message(output_id: &str) -> String {
    format!("There are no outputs with OutputId {output_id} that you would like to delete")
}

/// How this service words a SQL configuration offered to an application that runs no SQL.
///
/// One sentence for every operation that extends a SQL application's configuration — the
/// streaming source, the destination, the reference data table — so two of them cannot
/// report the same refusal differently. `configuration` names the SQL configuration the
/// REQUEST carries, spelled as the model spells the member that carries it
/// ([`OUTPUT_CONFIGURATION`]); it is `None` for `AddApplicationInput`, whose recorded
/// sentence names none (the behavioural unit
/// `kinesis-analytics-v2.add-application-input-on-flink-application-rejected` expects
/// "You cannot add sql configuration to a Flink application.").
///
/// The refusal is about the runtime the STORED application declares rather than about
/// anything in the request, which is why no caller passes the runtime in: an application
/// this service did not create as a SQL one runs its records through the code it runs, and
/// every such runtime family this API knows is a Flink one ([`is_flink_runtime`],
/// [`is_notebook_runtime`] — a Studio notebook runs Zeppelin over Flink).
pub fn sql_configuration_on_flink_message(configuration: Option<&str>) -> String {
    match configuration {
        Some(configuration) => {
            format!("You cannot add sql configuration {configuration} to a Flink application.")
        }
        None => "You cannot add sql configuration to a Flink application.".to_string(),
    }
}

/// The SQL configuration `AddApplicationOutput` offers, as [`sql_configuration_on_flink_message`]
/// names it: the model's `AddApplicationOutput$Output` member
/// (`smithy_query(query="operation",operation="AddApplicationOutput")`), which is the
/// configuration that request carries.
pub const OUTPUT_CONFIGURATION: &str = "Output";

/// The `RuntimeEnvironment` family a Studio notebook runs on: Apache Zeppelin over
/// Flink, `ZEPPELIN-FLINK-<major>_<minor>` (`ZEPPELIN-FLINK-1_0` … `ZEPPELIN-FLINK-3_0`
/// today), beside the plain `FLINK-` family a streaming application runs on
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")`).
const NOTEBOOK_RUNTIME_FAMILY: &str = "ZEPPELIN-";

/// Whether `runtime` names a Studio notebook's runtime — any version of that family, so
/// a later enum member of it is read the same way.
pub fn is_notebook_runtime(runtime: &str) -> bool {
    runtime.starts_with(NOTEBOOK_RUNTIME_FAMILY)
}

/// The `ApplicationMode` values, spelled exactly as the model's `ApplicationMode` enum
/// does (`smithy_query(query="shape",shape="ApplicationMode")`).
pub const APPLICATION_MODE_STREAMING: &str = "STREAMING";
pub const APPLICATION_MODE_INTERACTIVE: &str = "INTERACTIVE";

/// The mode an application runs in when its create named none.
///
/// `ApplicationSummary$ApplicationMode` states the rule: for a Managed Service for
/// Apache Flink application the mode is `STREAMING`, and for a Studio notebook it is
/// `INTERACTIVE` — and what makes an application a notebook is the Zeppelin runtime it
/// runs on ([`is_notebook_runtime`]). So the mode is a property of the application
/// rather than of the request that created it, and it is resolved for every runtime
/// family rather than for the ones a caller happens to name explicitly.
pub fn default_application_mode(runtime_environment: &str) -> &'static str {
    if is_notebook_runtime(runtime_environment) {
        APPLICATION_MODE_INTERACTIVE
    } else {
        APPLICATION_MODE_STREAMING
    }
}

/// How an `ApplicationMode` the request did not carry is QUOTED BACK by a refusal about it:
/// the absent value itself, not the mode the application would otherwise have run in.
const ABSENT_APPLICATION_MODE: &str = "null";

/// How this service words a create whose `ApplicationMode` is not the mode its
/// `RuntimeEnvironment` runs applications in: the mode the REQUEST carried and the runtime it
/// named, both echoed from the request.
pub fn inapplicable_application_mode_message(mode: Option<&str>, runtime: &str) -> String {
    format!(
        "Invalid configuration provided. ApplicationMode '{}' is not applicable to runtime \
         environment : {runtime}",
        mode.unwrap_or(ABSENT_APPLICATION_MODE)
    )
}

/// The refusal the `RuntimeEnvironment`/`ApplicationMode` pair a create names earns, or
/// `None` when the mode is the one that runtime runs its applications in.
///
/// An application is either a Managed Service for Apache Flink application or a Studio
/// notebook, and WHICH it is follows from its runtime — the Zeppelin family is the notebook
/// one ([`default_application_mode`]). The two modes are not interchangeable (`ApplicationMode`:
/// "Use the STREAMING mode to create a Managed Service for Apache Flink application. To create
/// a Managed Service for Apache Flink Studio notebook, use the INTERACTIVE mode"), so a create
/// naming the mode of the other kind of application is refused instead of stored, whichever of
/// the two it named.
///
/// A create that names NO mode is judged on the mode it would then run in — `STREAMING`, the
/// mode of a streaming application — so it is taken on a streaming runtime and refused on a
/// notebook one, where the interactive mode has to be asked for; that refusal quotes the
/// absent value the request sent rather than the mode it was read as.
pub fn application_mode_refusal(runtime_environment: &str, mode: Option<&str>) -> Option<String> {
    let applicable = default_application_mode(runtime_environment);
    let running_in = mode.unwrap_or(APPLICATION_MODE_STREAMING);
    (running_in != applicable)
        .then(|| inapplicable_application_mode_message(mode, runtime_environment))
}

/// The page this service points a caller at when it refuses the runtime a request named:
/// `CreateApplication`'s `RuntimeEnvironment` in the API reference, the list of the versions
/// the API still creates applications on.
const RUNTIME_ENVIRONMENT_REFERENCE: &str = "https://docs.aws.amazon.com/managed-flink/latest\
                                             /apiv2/API_CreateApplication.html\
                                             #APIReference-CreateApplication-request-\
                                             RuntimeEnvironment";

/// How this service words a request that would put an application on a runtime generation it
/// no longer supports: the runtime identifier is the `RuntimeEnvironment` the REQUEST named,
/// and the rest — the sentence and the page it points at — is fixed text.
pub fn deprecated_runtime_message(runtime: &str) -> String {
    format!(
        "Runtime {runtime} is deprecated. Please choose a valid version from: \
         {RUNTIME_ENVIRONMENT_REFERENCE}"
    )
}

/// The GENERATION a `RuntimeEnvironment` value names, as `(major, minor)` — the version that
/// closes the value (`FLINK-1_20` → `(1, 20)`, `ZEPPELIN-FLINK-3_0` → `(3, 0)`) — or `None`
/// for a value that carries none.
///
/// Every member of the enum spells its version as `<family>-<major>_<minor>`
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")`), so the generation is read off
/// the value rather than matched against a list of spelled-out members: a member the enum
/// gains later is read the same way.
pub fn runtime_generation(runtime: &str) -> Option<(u32, u32)> {
    let version = runtime.rsplit('-').next()?;
    let (major, minor) = version.split_once('_')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The generation of the Studio-notebook runtime family this API creates notebooks on: the
/// NEWEST generation of that family the `RuntimeEnvironment` enum carries.
///
/// A Studio notebook runs the Apache Zeppelin and Apache Flink pair its runtime generation
/// names, and this API offers one such pair at a time — the current one — so the generations
/// before it are withdrawn as each new one ships ([`deprecated_runtime_refusal`]). Derived
/// from the enum, so the current generation follows the model rather than being spelled out
/// here beside it.
fn current_notebook_generation() -> Option<(u32, u32)> {
    crate::ops::types::RUNTIME_ENVIRONMENT_VALUES
        .iter()
        .filter(|runtime| is_notebook_runtime(runtime))
        .filter_map(|runtime| runtime_generation(runtime))
        .max()
}

/// The refusal a request naming `runtime` earns because the service no longer puts an
/// application on that runtime generation, or `None` when the generation is one it still does.
///
/// A Studio notebook is created on the CURRENT generation of its runtime family only
/// ([`current_notebook_generation`]); an earlier generation of that family is deprecated, and a
/// request naming one is refused with [`deprecated_runtime_message`] instead of being served —
/// whatever else the request carries, since what is withdrawn is the runtime and not the
/// configuration around it. Every generation of the streaming Flink family the enum carries is
/// one this API still creates applications on, so nothing in that family is refused here, and
/// the retired SQL family has a refusal of its own ([`is_sql_runtime`]).
///
/// One rule for the create that puts an application on a runtime and the update that moves it
/// onto another, so neither route lands an application on a withdrawn generation. An
/// application ALREADY stored on one keeps being read, described and run: the rule is about the
/// runtime a request NAMES.
pub fn deprecated_runtime_refusal(runtime: &str) -> Option<String> {
    let generation = runtime_generation(runtime).filter(|_| is_notebook_runtime(runtime))?;
    let current = current_notebook_generation()?;
    (generation < current).then(|| deprecated_runtime_message(runtime))
}

/// The two members a Zeppelin note is identified by, as the note object spells them: Apache
/// Zeppelin holds a note as a JSON object carrying the note's `id` and its `name`.
const ZEPPELIN_NOTE_ID: &str = "id";
/// See [`ZEPPELIN_NOTE_ID`].
const ZEPPELIN_NOTE_NAME: &str = "name";

/// How this service words inline Studio code that is not a note.
///
/// Entirely fixed text: what the refusal is about is the FORM the value has to take, so it
/// quotes no value the request carried.
pub const INVALID_ZEPPELIN_NOTE: &str =
    "Zeppelin application code provided in TextContent must be a valid Zeppelin note JSON \
     object containing 'id' and 'name' fields";

/// The refusal the inline application code of an application on `runtime` earns, or `None`
/// when it is code that runtime runs.
///
/// A Studio notebook's code is a NOTE of the Apache Zeppelin the notebook runtime runs
/// ([`is_notebook_runtime`]), and a note is a JSON object carrying the note's id and its
/// name — so `CodeContent$TextContent` supplied for such an application has to be one, and
/// anything else is refused rather than stored as code no notebook could open. Every other
/// runtime of this service runs inline code as the program text it is, so nothing about its
/// form is judged here.
///
/// One rule for the create that supplies the code and the update that replaces it, decided
/// on the runtime the application RUNS on once the call lands, so a notebook cannot be given
/// code of another form by either route. The value is read with `serde_json`, the crate every
/// reader of a JSON document in this crate already parses one with, rather than by looking
/// for the member names in the text: it answers an error for anything that is not a JSON
/// document instead of failing, and the text reaching it is already bounded by `TextContent`'s
/// modelled length, which the boundary validator holds every request to.
pub fn zeppelin_note_refusal(runtime: &str, text_content: Option<&str>) -> Option<String> {
    let text = text_content.filter(|_| is_notebook_runtime(runtime))?;
    let is_note = ::serde_json::from_str::<::serde_json::Value>(text)
        .ok()
        .as_ref()
        .and_then(::serde_json::Value::as_object)
        .is_some_and(|note| {
            [ZEPPELIN_NOTE_ID, ZEPPELIN_NOTE_NAME]
                .iter()
                .all(|member| note.get(*member).is_some_and(|value| !value.is_null()))
        });
    (!is_note).then(|| INVALID_ZEPPELIN_NOTE.to_string())
}

/// The inline code a configuration supplied to a create asks the application to run, or
/// `None` when it supplies none.
pub fn configured_text_content(cfg: &ApplicationConfiguration) -> Option<&str> {
    cfg.application_code_configuration
        .as_ref()
        .and_then(|code| code.code_content.as_ref())
        .and_then(|content| content.text_content.as_deref())
}

/// The inline code an UPDATE writes, or `None` when it writes none — the update-shape
/// sibling of [`configured_text_content`], so a caller of the note rule reaches the right
/// member whichever shape it has in hand.
pub fn updated_text_content(update: &ApplicationConfigurationUpdate) -> Option<&str> {
    update
        .application_code_configuration_update
        .as_ref()
        .and_then(|code| code.code_content_update.as_ref())
        .and_then(|content| content.text_content_update.as_deref())
}

/// The `ApplicationMaintenanceWindowStartTime` an application is created with.
///
/// Maintenance runs on every application in a window the service keeps for it, so a
/// window exists from the moment the application does and
/// `UpdateApplicationMaintenanceConfiguration` only moves its start. Which start a
/// freshly created application gets is per-account rather than part of the API
/// contract, so this is the emulator's account-wide one, the start the API reference's
/// `UpdateApplicationMaintenanceConfiguration` example answers with.
pub const DEFAULT_MAINTENANCE_WINDOW_START: &str = "06:00";

/// How long a maintenance window lasts: the API reference's
/// `UpdateApplicationMaintenanceConfiguration` example answers an
/// `ApplicationMaintenanceWindowStartTime` of `06:00` with an
/// `ApplicationMaintenanceWindowEndTime` of `14:00` — eight hours.
const MAINTENANCE_WINDOW_HOURS: u32 = 8;

/// A day's length in hours, the modulus a maintenance window that runs past midnight
/// wraps its end time by: `ApplicationMaintenanceWindowEndTime` is a time of day
/// (`^([01][0-9]|2[0-3]):[0-5][0-9]$`), not an offset from the start.
const HOURS_PER_DAY: u32 = 24;

/// The `ApplicationMaintenanceWindowEndTime` that goes with `start`:
/// [`MAINTENANCE_WINDOW_HOURS`] later, wrapped into the same day's clock, with the
/// minutes the start carries.
///
/// `start` is an `ApplicationMaintenanceWindowStartTime` — `HH:MM`, refused at the
/// boundary when it is anything else — so a value that does not parse can only be one
/// this store never wrote, and is answered unchanged rather than guessed at.
pub fn maintenance_window_end(start: &str) -> String {
    let Some((hours, minutes)) = start.split_once(':') else {
        return start.to_string();
    };
    match (hours.parse::<u32>(), minutes.parse::<u32>()) {
        (Ok(hours), Ok(minutes)) if hours < HOURS_PER_DAY && minutes < 60 => {
            let end = (hours + MAINTENANCE_WINDOW_HOURS) % HOURS_PER_DAY;
            format!("{end:02}:{minutes:02}")
        }
        _ => start.to_string(),
    }
}

/// How this service words a change addressed to a version other than the one in force.
///
/// One wording for every operation that carries that optimistic-concurrency token, so two
/// operations cannot report the same stale address differently. The MEMBER the request
/// addressed the version by is the sentence's only variable part: naming the version the
/// caller sent, or the one the application is on, would hand a client a way to guess its
/// way past the check.
pub fn stale_version_message(member: &str) -> String {
    format!("The provided {member} does not match the most recent version for the application.")
}

/// The two members a change can address a version by, as [`stale_version_message`] names
/// them.
pub const VERSION_ID_MEMBER: &str = "application version ID";
/// See [`VERSION_ID_MEMBER`].
pub const CONDITIONAL_TOKEN_MEMBER: &str = "ConditionalToken";

/// What AWS answers a caller that names neither of the two members a change is made
/// against, as both members' docs state the requirement ("You must provide the
/// CurrentApplicationVersionId or the ConditionalToken").
pub const VERSION_OR_TOKEN_REQUIRED: &str =
    "You must provide the CurrentApplicationVersionId or the ConditionalToken.";

/// What AWS answers a change made against an application whose version a `ConditionalToken`
/// does not address, so the version ID is the only member that addresses it: the runtime the
/// application runs on.
///
/// Read off the STORED application rather than off the request, because what decides the
/// requirement is the application the change is made against.
pub fn version_id_required_message(runtime: &str) -> String {
    format!(
        "CurrentApplicationVersionId parameter should be provided for application with \
         RuntimeEnvironment: {runtime}"
    )
}

/// Why the version a change is addressed to is refused.
///
/// The two cases are different error CLASSES, which is why they are distinguished here and
/// not just worded differently: a request that addresses no version at all carries a
/// parameter value that is not valid, while one that addresses a consumed version lost a
/// race with another change.
#[derive(Debug, Clone)]
pub enum AddressedVersionRefusal {
    /// The request named neither member, so it addresses no version at all.
    NotAddressed(String),
    /// The member it named no longer addresses the version in force, so something else has
    /// modified the application since the caller read it.
    Stale(String),
}

/// The refusal the version a change is addressed to earns, or `None` when it addresses the
/// version in force.
///
/// "You must provide the CurrentApplicationVersionId or the ConditionalToken" (both
/// members' docs): every operation that changes an application's configuration is made
/// against a NAMED version, and whichever of the two the caller names has to be the version
/// in force. One rule for all of them, so two such operations cannot decide the same
/// address differently; each maps the refusal onto its OWN modelled error shapes.
pub fn addressed_version_refusal(
    supplied_version_id: Option<i64>,
    supplied_token: Option<&str>,
    version_in_force: i64,
    token_in_force: Option<&str>,
    runtime_environment: &str,
) -> Option<AddressedVersionRefusal> {
    // The either-or holds for an application a `ConditionalToken` addresses a version of; a
    // Studio notebook is not one, so on the Zeppelin runtime family
    // ([`is_notebook_runtime`]) the version is addressed by `CurrentApplicationVersionId`
    // ALONE and a change naming no ID is refused naming that runtime, whether or not it
    // named a token. Decided on the runtime of the STORED application, so it answers for
    // every operation of this service that is made against a named version.
    if supplied_version_id.is_none() && is_notebook_runtime(runtime_environment) {
        return Some(AddressedVersionRefusal::NotAddressed(
            version_id_required_message(runtime_environment),
        ));
    }
    match (supplied_version_id, supplied_token) {
        (None, None) => Some(AddressedVersionRefusal::NotAddressed(
            VERSION_OR_TOKEN_REQUIRED.to_string(),
        )),
        (Some(supplied), _) if supplied != version_in_force => Some(
            AddressedVersionRefusal::Stale(stale_version_message(VERSION_ID_MEMBER)),
        ),
        (_, Some(token)) if Some(token) != token_in_force => Some(
            AddressedVersionRefusal::Stale(stale_version_message(CONDITIONAL_TOKEN_MEMBER)),
        ),
        _ => None,
    }
}

/// The current wall-clock instant as a modelled timestamp, truncated to whole seconds.
///
/// Whole seconds keep the awsJson1_1 encoding an integer, so a `CreateTimestamp` a
/// client reads back and passes to `DeleteApplication` is bit-identical to the stored
/// one rather than a float that lost precision in transit.
pub fn now_timestamp() -> DateTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    DateTime::from_secs(secs as i64)
}

/// How the operations that END the sentence in a period word a lookup miss on an
/// application: the name the caller asked for, quoted, and the fact that no such
/// application exists.
///
/// This API words a not-found refusal per operation, so this is not one sentence for the
/// whole service: `StopApplication` answers the same miss with the same sentence stopped
/// at its last word, and carries its own wording
/// (`crate::ops::stop_application::unknown_application_message`). The error CODE stays
/// each operation's own too — the model does not declare the same error on all of them,
/// and `CreateApplicationSnapshot` reports the miss as `InvalidArgumentException` with its
/// own wording.
pub fn missing_application_message(name: &str) -> String {
    format!("Application '{name}' does not exist.")
}

/// How this service words a `CreateTimestamp` that does not identify the application the
/// caller addressed it to: the name the request named, and the reader that answers the value
/// in force.
///
/// The stamp is the caller's proof that it is acting on the application it last read, so a
/// value that does not match belongs to a read something else has since superseded — which is
/// why the sentence sends the caller back to `DescribeApplication` rather than telling it the
/// value is malformed. Neither stamp is stated: naming the one in force would hand a client a
/// way past the check.
pub fn stale_create_timestamp_message(name: &str) -> String {
    format!(
        "The application with name: '{name}' has a different create timestamp than the one \
         you provided. Please call DescribeApplication to fetch valid create timestamp."
    )
}

/// How the SNAPSHOT operations word the same lookup miss: the name the caller asked for and
/// the account the lookup was made in, which is the emulator's own.
///
/// These operations do not report an application miss the way the application readers do —
/// they refuse it as a parameter that is not a valid argument, and state the account the
/// name was resolved in, because a snapshot request is resolved against the snapshots of
/// one application in one account. One wording for every snapshot operation that looks an
/// application up by name, so two of them cannot report the same absent application
/// differently.
pub fn missing_application_in_account_message(name: &str) -> String {
    format!(
        "Application '{name}' does not exist under account '{}'.",
        ::engine::emulator_account_id()
    )
}

/// How this service words a create refused because an application of that name is already
/// stored: the SAME sentence shape a lookup miss uses
/// ([`missing_application_message`]) — the resource the request named, quoted, then what
/// is true of it, as a sentence ending in a period.
pub fn existing_application_message(name: &str) -> String {
    format!("Application '{name}' already exists.")
}

/// How this service words a lifecycle action refused because the application is not in a
/// status that action is accepted from: the fixed sentence the API answers for that error
/// class, naming the attempted transition and the application's CURRENT status, quoted.
///
/// `transition` is the past participle of the action as the API spells it
/// ([`TRANSITION_STOPPED`] for `StopApplication`, [`TRANSITION_STARTED`] for
/// `StartApplication`), and `status` is read off the STORED application, so one sentence
/// answers every status a caller can catch an application in. The application's NAME is
/// NOT part of it: the refusal states which status the action is not accepted from, not
/// which application the caller named — the request already carries that. One wording for
/// every operation that refuses a lifecycle action on a status, so two of them cannot
/// report the same ineligible application differently.
pub fn ineligible_status_message(transition: &str, status: &str) -> String {
    format!("Application cannot be {transition} in '{status}' state")
}

/// The transition `StopApplication` names in [`ineligible_status_message`].
pub const TRANSITION_STOPPED: &str = "Stopped";

/// The transition `StartApplication` names in [`ineligible_status_message`].
pub const TRANSITION_STARTED: &str = "Started";

/// The transition `DeleteApplication` names in [`ineligible_status_message`].
pub const TRANSITION_DELETED: &str = "Deleted";

/// The transition the updates — `UpdateApplication` and
/// `UpdateApplicationMaintenanceConfiguration` — name in [`ineligible_status_message`].
pub const TRANSITION_UPDATED: &str = "Updated";

/// How this service words a lookup miss on one VERSION of an application: the id the
/// caller asked for and the application it was looked for in, in the sentence shape
/// [`missing_application_message`] uses for the application itself.
pub fn missing_application_version_message(version_id: i64, application: &str) -> String {
    format!("Application version '{version_id}' does not exist for application {application}.")
}

/// How this service words a call refused because the operation it names is supported only
/// for Managed Service for Apache Flink applications — which `DescribeApplicationVersion`
/// and `ListApplicationVersions` both document ("This operation is supported only for
/// Managed Service for Apache Flink") — and the application it was called on runs on
/// another runtime family.
///
/// The sentence is the RECORDED one: the behavioural unit
/// `kinesisanalyticsv2.studio-notebook-version-history-reads-rejected-as-unsupported`
/// expects `UnsupportedOperationException` carrying
/// "DescribeApplicationVersion is not supported for applications with runtime environment
/// ZEPPELIN-FLINK-3_0." The operation and the application's `RuntimeEnvironment` are its
/// variable parts, taken from the request and from the stored application, so one wording
/// answers every such call whatever runtime the application runs on.
pub fn unsupported_for_runtime_message(operation: &str, runtime: &str) -> String {
    format!("{operation} is not supported for applications with runtime environment {runtime}.")
}

/// How this service words a call refused because the SNAPSHOT the operation would take is
/// not something the application's runtime keeps: the runtime, read off the stored
/// application.
///
/// The sentence names no operation of its own ("This operation"), which is how AWS words
/// this refusal — a different wording from the version readers'
/// ([`unsupported_for_runtime_message`]) and from the rollback's
/// ([`rollback_unsupported_for_runtime_message`]), each of which states its own.
pub fn unsupported_for_runtime_environment_message(runtime: &str) -> String {
    format!("This operation is not supported for RuntimeEnvironment {runtime}")
}

/// How this service words a rollback refused because the application's runtime keeps no
/// previous running version to load: the runtime, read off the stored application.
///
/// AWS words this one in the singular — about THE application the call named — so it is its
/// own sentence rather than the version readers'
/// ([`unsupported_for_runtime_message`]).
pub fn rollback_unsupported_for_runtime_message(runtime: &str) -> String {
    format!(
        "RollbackApplication is not supported for application with runtime environment \
         {runtime}."
    )
}

/// Whether `runtime` names the runtime family of a Managed Service for Apache Flink
/// application: the plain `FLINK-` family, as opposed to a SQL application
/// ([`is_sql_runtime`]) or a Studio notebook ([`is_notebook_runtime`]), which run the same
/// service's other two kinds of application.
///
/// Read off a STORED application, so what decides is the runtime the application runs on
/// rather than a value a request carried.
pub fn is_flink_runtime(runtime: &str) -> bool {
    !is_sql_runtime(runtime) && !is_notebook_runtime(runtime)
}

/// How this service words a call that would leave an application carrying more
/// user-defined tags than [`MAX_USER_TAGS`] admits: how many tags it asked the application
/// to carry, and the ceiling.
///
/// One sentence for the create that supplies tags and for `TagResource`, so the two cannot
/// report the same ceiling differently.
pub fn too_many_tags_message(count: usize) -> String {
    format!(
        "{count} tags were supplied; at most {MAX_USER_TAGS} user-defined application tags \
         are allowed"
    )
}

/// How this service words a `ResourceARN` that is not the ARN of one of its applications at
/// all — a value that names no application to tag or read tags from, as opposed to one
/// naming an application this store does not hold ([`missing_application_message`]). The
/// same split, and the same sentence shape, as [`invalid_catalog_database_arn_message`].
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    format!(
        "The ResourceARN '{resource_arn}' is not the ARN of a Managed Service for Apache Flink \
         application"
    )
}

/// The name of the application a `KinesisAnalyticsARN` names, or `None` when the value is
/// not the ARN of an application of this service.
///
/// Read through the shared ARN parse, so a caller-supplied ARN is read exactly as every
/// other service reads one rather than by splitting the value again. The vendor field has
/// to be this service's `arnNamespace` and the resource an `application` with a non-empty
/// id, which is the form [`application_arn`] mints.
pub fn application_name_from_arn(resource_arn: &str) -> Option<&str> {
    if !::service_common::arn::is_resource_arn_of(resource_arn, ARN_NAMESPACE) {
        return None;
    }
    let name =
        ::service_common::arn::resource_id_of_type(resource_arn, APPLICATION_RESOURCE_TYPE)?;
    (!name.is_empty()).then_some(name)
}

/// How this service words a `NextToken` that is not a pagination token it issued: the
/// value the caller sent, quoted, and the fact that it names no page to resume from.
pub fn invalid_next_token_message(next_token: &str) -> String {
    format!("The NextToken '{next_token}' is not valid.")
}

/// The type of resource an application's ARN names, the `application` of
/// `application/<name>`.
const APPLICATION_RESOURCE_TYPE: &str = "application";

/// How this service words a sentence about one application's SNAPSHOT: the snapshot the
/// request named is quoted, because it is what the sentence is about, and the application
/// it belongs to is named plainly as the scope it was looked up in.
///
/// `fact` is what is true of that snapshot (`does not exist`, `already exists`), so every
/// operation that reports on a snapshot by name states it the same way.
fn snapshot_message(snapshot: &str, fact: &str, application: &str) -> String {
    format!("Snapshot '{snapshot}' {fact} for application {application}.")
}

/// A snapshot the application holds no record of, for the operations that read one by name.
pub fn missing_snapshot_message(snapshot: &str, application: &str) -> String {
    snapshot_message(snapshot, "does not exist", application)
}

/// How this service words a snapshot refused because the application is not RUNNING.
///
/// A snapshot captures the state a RUNNING application holds, so in any other status there
/// is no running state to capture. The sentence states the status the application is in and
/// the status the operation requires, both read from the store and the model's
/// `ApplicationStatus` enum, so it reads the same for every status a caller can catch the
/// application in — and one wording serves `CreateApplicationSnapshot` and `CreateSnapshot`,
/// which take the two kinds of snapshot of the same running application.
pub fn snapshot_requires_running_message(application: &str, status: &str) -> String {
    format!(
        "Application {application} is in {status} status. Snapshot creation is only allowed \
         when application is in {STATUS_RUNNING} status"
    )
}

/// A snapshot name the application already holds a snapshot under, for the operations that
/// create one.
pub fn existing_snapshot_message(snapshot: &str, application: &str) -> String {
    snapshot_message(snapshot, "already exists", application)
}

/// How this service words an update that names an element of an application's
/// configuration — an input, an output, a reference data source, a VPC configuration, a
/// CloudWatch logging option — by an id the application holds no such element under.
///
/// `kind` names the element as the API spells it, so every update that resolves one by id
/// reports a miss the same way.
pub fn missing_update_target_message(kind: &str, id: &str, application: &str) -> String {
    format!("{kind} '{id}' does not exist for application {application}.")
}

/// How the service words a Studio notebook's catalog reference that the Glue Data Catalog
/// holds no database for: the database the request's `DatabaseARN` names, quoted, and the
/// catalog it was looked for in.
pub fn missing_catalog_database_message(database: &str) -> String {
    format!("Cannot find database with name '{database}' in Glue Data Catalog")
}

/// How the service words a `DatabaseARN` naming a database of ANOTHER Region: the Region the
/// request was made in, which is the only one this application can read a catalog in, and
/// the Region the value carried.
pub fn cross_region_catalog_database_message(expected: &str, found: &str) -> String {
    format!("Incorrect region in DatabaseArn, expecting '{expected}' but found '{found}'.")
}

/// How the service words a `DatabaseARN` that is not the ARN of a Glue Data Catalog
/// database at all — a value that names no database for the notebook to read, as opposed
/// to one naming a database the catalog does not hold
/// ([`missing_catalog_database_message`]). The value the request carried is the sentence's
/// only variable part.
pub fn invalid_catalog_database_arn_message(database_arn: &str) -> String {
    format!("The DatabaseARN '{database_arn}' is not the ARN of a Glue Data Catalog database")
}

/// The `arnNamespace` of the service whose resource a `DatabaseARN` names, and the type of
/// resource it has to name: a Glue database, written
/// `arn:aws:glue:<region>:<account>:database/<name>`.
const GLUE_ARN_NAMESPACE: &str = "glue";
const GLUE_DATABASE_RESOURCE_TYPE: &str = "database";

/// The Data Catalog, the Region and the database name a
/// `GlueDataCatalogConfiguration$DatabaseARN` points at, or `None` when the value is not the
/// ARN of a Glue database.
///
/// A Glue database ARN carries the catalog it lives in as its ACCOUNT field (Glue keys a
/// Data Catalog by the account id, which is what its own operations default `CatalogId`
/// to), the Region that catalog is in as its REGION field, and the database as the ID of a
/// `database` resource. Read through the shared ARN parse ONCE, so this service reads a
/// caller-supplied ARN exactly as every other one does rather than splitting the value again
/// — and so every reader of one value agrees on what it names.
fn glue_database_reference(database_arn: &str) -> Option<(String, String, String)> {
    let arn = ::service_common::arn::parse(database_arn)?;
    if arn.vendor != GLUE_ARN_NAMESPACE {
        return None;
    }
    let database =
        ::service_common::arn::resource_id_of_type(database_arn, GLUE_DATABASE_RESOURCE_TYPE)?;
    if database.is_empty() {
        return None;
    }
    let catalog_id = if arn.account.is_empty() {
        ::engine::emulator_account_id()
    } else {
        arn.account
    };
    Some((
        catalog_id.to_string(),
        arn.region.to_string(),
        database.to_string(),
    ))
}

/// The refusal a Studio notebook's catalog configuration earns when the `DatabaseARN` it
/// names is not a Glue database ARN, names a database of another Region, or points at a name
/// the Glue Data Catalog holds no database under — `None` when that database resolves.
///
/// `region` is the Region the request was made in, which is the Region whose Data Catalog
/// the application reads: a Data Catalog is Regional, so a value naming another Region is
/// refused on the ARN alone ([`cross_region_catalog_database_message`]) before any catalog is
/// asked.
///
/// Read from the Glue service's OWN store through the one shared engine, so the answer is
/// LIVE: the same ARN resolves once `CreateDatabase` has written the database and stops
/// resolving once `DeleteDatabase` has dropped it, and the name is folded exactly as
/// Glue's own `GetDatabase` folds it (the catalog keys a database by its folded name).
/// It takes the Glue store's lock and holds no other while it does, so it is called with
/// NO guard of this service's store held.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one): an
/// emulator with no Glue to ask shows nothing about the database, and refusing on that
/// would refuse every notebook. A value that is no database ARN needs nothing asked.
pub fn catalog_database_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    database_arn: &str,
) -> Option<String> {
    let Some((catalog_id, named_region, database)) = glue_database_reference(database_arn) else {
        return Some(invalid_catalog_database_arn_message(database_arn));
    };
    // A Data Catalog is Regional, and an application reads the one of the Region it runs in,
    // so a `DatabaseARN` whose Region field is another one names a database this application
    // could never read — refused on the ARN alone, before any catalog is asked, naming the
    // Region expected and the Region found. A value carrying no Region at all states no
    // other Region, so it is left to the lookup below.
    if !named_region.is_empty() && named_region != region {
        return Some(cross_region_catalog_database_message(region, &named_region));
    }
    let state = ::service_com_amazonaws_glue_awsglue::state::service_state(engine)?;
    let held = state.lock().database(&catalog_id, &database).is_some();
    (!held).then(|| missing_catalog_database_message(&database))
}

/// The `DatabaseARN` a configuration supplied to a create points its Studio notebook at,
/// or `None` when it configures no catalog.
pub fn configured_catalog_database_arn(cfg: &ApplicationConfiguration) -> Option<&str> {
    cfg.zeppelin_application_configuration
        .as_ref()
        .and_then(|zeppelin| zeppelin.catalog_configuration.as_ref())
        .map(|catalog| catalog.glue_data_catalog_configuration.database_arn.as_str())
}

/// The greatest `ParallelismPerKPU` this service runs an application at.
///
/// NOT a modelled bound: `ParallelismPerKPU` declares only `@range` min 1
/// (`smithy_query(query="shape",shape="ParallelismPerKPU")`), so a value past the one the
/// service supports satisfies the model and is refused by the handler, with the modelled
/// `InvalidArgumentException`, rather than at the boundary.
pub const MAX_PARALLELISM_PER_KPU: i32 = 8;

/// How this service words a `ParallelismPerKPU` outside the values it runs an application at:
/// the application the configuration was supplied for, and what the value had to be.
pub fn parallelism_per_kpu_message(application: &str) -> String {
    format!(
        "FlinkApplicationParallelismPerKPU provided for the application {application} should \
         be a positive integer and should not be larger than the supported limit \
         {MAX_PARALLELISM_PER_KPU}"
    )
}

/// The refusal a `ParallelismPerKPU` a request configures earns, or `None` when it is one the
/// service runs an application at — and `None` for a request that configures none, which
/// leaves the application on the service's own default.
///
/// One rule for the create that supplies the configuration and for the update that replaces
/// it, so the two cannot bound the same member differently.
pub fn parallelism_per_kpu_refusal(application: &str, configured: Option<i32>) -> Option<String> {
    configured
        .filter(|per_kpu| *per_kpu < 1 || *per_kpu > MAX_PARALLELISM_PER_KPU)
        .map(|_| parallelism_per_kpu_message(application))
}

/// The `ParallelismPerKPU` a configuration supplied to a create asks the application to run
/// at, or `None` when it configures no parallelism.
pub fn configured_parallelism_per_kpu(cfg: &ApplicationConfiguration) -> Option<i32> {
    cfg.flink_application_configuration
        .as_ref()
        .and_then(|flink| flink.parallelism_configuration.as_ref())
        .and_then(|parallelism| parallelism.parallelism_per_kpu)
}

/// The `SnapshotMode` a configuration supplied to a create selects, or `None` when it
/// configures no snapshot mode.
pub fn configured_snapshot_mode(cfg: &ApplicationConfiguration) -> Option<&String> {
    cfg.application_snapshot_configuration
        .as_ref()
        .and_then(|snapshots| snapshots.snapshot_mode.as_ref())
}

/// The region every ARN this service mints carries: the one the caller signed for.
///
/// Falls back to the emulator's default region when the request carries no SigV4
/// credential scope to read it from.
pub fn region_from_headers(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request is answered in, matching the region the emulator
/// writes into the AWS config it hands clients (`vendor/server/src/lib.rs`).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of an application: `arn:aws:kinesisanalytics:<region>:<account>:application/<name>`.
///
/// `kinesisanalytics` is this service's `arnNamespace`
/// (`smithy_query(query="service_info")`), and the account segment is the emulator's
/// canonical one, which [`::engine::emulator_arn`] owns.
pub fn application_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APPLICATION_RESOURCE_TYPE}/{name}"),
    )
}

/// This service's `arnNamespace` (`smithy_query(query="service_info")`): the vendor field
/// of every ARN it mints, and the one a `ResourceARN` it is handed has to carry.
const ARN_NAMESPACE: &str = "kinesisanalytics";

/// This service's SigV4 signing name (`sigv4Name` in `smithy_query(query="service_info")`):
/// the service a URL this emulator signs for a caller to fetch later is signed as, so the
/// emulator verifies that URL as a request to THIS service.
pub const SIGNING_NAME: &str = "kinesisanalytics";

/// A digest-derived id in the 8-4-4-4-12 hex form.
fn hex_id(seed: &str) -> String {
    let hex = digest_hex(seed);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A digest-derived OPAQUE token: the same digest as [`hex_id`], with no punctuation at
/// all, so the value carries no structure a client could read a meaning out of.
fn opaque_token(seed: &str) -> String {
    digest_hex(seed)
}

/// The first 16 bytes of `seed`'s SHA-256, hex encoded: 32 characters, which both the id
/// form and the token form above are written from.
fn digest_hex(seed: &str) -> String {
    use sha2::Digest;
    hex::encode(&sha2::Sha256::digest(seed.as_bytes())[..16])
}

/// The VPC id reported for a subnet set, derived from the subnets themselves so the
/// same subnets always report the same VPC.
fn vpc_id_for(subnet_ids: &[String]) -> String {
    let mut sorted: Vec<&str> = subnet_ids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let digest = hex_id(&format!("vpc:{}", sorted.join(",")));
    format!("vpc-{}", &digest[0..8])
}

/// The in-app stream names an SQL input is exposed under: one per unit of input
/// parallelism, named `<NamePrefix>_NNN`.
///
/// `count` reaches here already bounded by [`bounded_input_parallelism`], and SIZES A
/// LOOP, so it is clamped again to [`MAX_INPUT_PARALLELISM`] before anything is
/// allocated: no request-supplied integer can drive an unbounded allocation loop even if
/// the boundary range check that refuses it, or a caller of this function, ever stops
/// bounding it.
///
/// `InAppStreamName` is bounded at 32 characters (`smithy_query(query="shape",
/// shape="InAppStreamName")`), so a prefix long enough to overflow that bound yields no
/// names at all rather than a value the model would reject.
fn in_app_stream_names(name_prefix: &str, count: i32) -> Option<Vec<String>> {
    let count = count.clamp(MIN_INPUT_PARALLELISM, MAX_INPUT_PARALLELISM) as usize;
    let mut names: Vec<String> = Vec::with_capacity(count);
    names.extend(
        (1..=count)
            .map(|n| format!("{name_prefix}_{n:03}"))
            .filter(|name| name.len() <= 32),
    );
    (!names.is_empty()).then_some(names)
}

/// The largest `InputParallelism$Count` the model admits, from its `@range` of 1..64
/// (`smithy_query(query="shape",shape="InputParallelismCount")`). The boundary validator
/// already refuses a request outside that range; this is the defence-in-depth bound on
/// the loop the value sizes.
const MAX_INPUT_PARALLELISM: i32 = 64;

/// The smallest `InputParallelism$Count` the model admits, from the same `@range` of
/// 1..64, and the count an input described without one carries: one in-application stream
/// for the streaming source.
const MIN_INPUT_PARALLELISM: i32 = 1;

/// The `InputParallelism$Count` a request asked for, bounded to the range the model admits
/// before anything is sized from it.
///
/// Every derivation that counts in-application streams reads the value through here, so a
/// request-supplied integer is bounded where it is READ and not only inside the loop it
/// sizes. The boundary validator already refuses a count outside the modelled range, so
/// this changes no response a caller can obtain.
fn bounded_input_parallelism(count: Option<i32>) -> i32 {
    count
        .unwrap_or(MIN_INPUT_PARALLELISM)
        .clamp(MIN_INPUT_PARALLELISM, MAX_INPUT_PARALLELISM)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisanalyticsv2Kinesisanalytics20180523State>)
}
