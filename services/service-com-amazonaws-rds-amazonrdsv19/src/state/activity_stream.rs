// SPDX-License-Identifier: CC0-1.0

//! The DATABASE ACTIVITY STREAM of a DB cluster or DB instance: the stream
//! `StartActivityStream` starts and `StopActivityStream` ends, the AUDIT POLICY STATE
//! `ModifyActivityStream` writes, and the preconditions each of the three holds a request to.
//!
//! All three address their resource by `ResourceArn` and report the same stream back, so the
//! record and the rules live here once rather than in three handlers: a stream started
//! through one operation is the one the next call stops and the one the resource's own
//! describe reports (`DBCluster$ActivityStream*`, `DBInstance$ActivityStream*`).
//!
//! `ModifyActivityStream` "changes the audit policy state of a database activity stream to
//! either locked (default) or unlocked", addressing the instance by `ResourceArn` — "the
//! Amazon Resource Name (ARN) of the RDS for Oracle or Microsoft SQL Server DB instance" —
//! and reports the state of that policy back as `ActivityStreamPolicyStatus`. So the policy
//! state is a property OF THE INSTANCE, stored on the instance's own record
//! ([`DbInstanceRecord::activity_stream_policy`]) rather than in a map beside it: it is the
//! same value `DBInstance$ActivityStreamPolicyStatus` reports, it goes when the instance
//! goes, and it follows the instance through a rename.
//!
//! The policy state does not change the instant it is asked for. `ActivityStreamPolicyStatus`
//! models two TRANSITIONAL members beside the two settled ones (`locked | unlocked |
//! locking-policy | unlocking-policy`), so a change of state is reported as the transition it
//! is until this service's clock has made it due ([`STATUS_TRANSITION_DELAY`]), and as the new
//! state afterwards — nothing sleeps for it: the due instant is stored on the record and
//! [`ActivityStreamPolicy::settle`] applies it from [`DbInstanceRecord::settle`].
//!
//! Asking for the state the policy is already in, or already moving to, is no transition at
//! all: the state is a SETTING rather than a step, so the call is taken and reports the policy
//! as it stands, the way every other idempotent write of this service does.
//!
//! A `ResourceArn` that names no DB instance of this service at all is refused on the MEMBER,
//! with the one sentence this front door gives every operation that addresses its resource by
//! ARN ([`tags::invalid_resource_name_message`]), so this operation cannot drift from the tag
//! writes and the Data API toggles in how it words an unusable reference. A `ResourceArn` that
//! IS a DB instance ARN but names another REGION is refused on the member too, and before any
//! lookup: a regional endpoint serves its own Region's resources only, so such a value names
//! nothing it could act on whatever this store holds ([`check_arn_parameter_region`]).
//!
//! The STREAM itself ([`ActivityStream`]) is stored on the same records, for the same reasons:
//! "Starts a database activity stream to monitor activity on the database" is a write on the
//! resource the `ResourceArn` names, and what it records — the mode, the key its messages are
//! encrypted under, whether engine-native audit fields travel with them and the status of the
//! delivery — is exactly what that resource's own describe reports. The Kinesis data stream
//! the activity is delivered to is NOT stored: Amazon RDS names it after the resource's
//! immutable resource id ([`activity_stream_kinesis_stream_name`]), so it is derived from the
//! record every time it is reported.
//!
//! A stream does not reach its new status the instant it is asked for either:
//! `ActivityStreamStatus` models `starting` and `stopping` beside `started` and `stopped`, so
//! a start is reported as `starting` and a stop as `stopping` until the change is due — at the
//! span this service owes a transition ([`STATUS_TRANSITION_DELAY`]) for a call that asked to
//! apply it "as soon as possible, regardless of the maintenance window for the database", and
//! in the resource's next maintenance window for one that did not, which is what
//! `ApplyImmediately` decides ([`maintenance_window::change_is_due`], the rule a deferred
//! modification of a DB instance waits on too). Nothing sleeps for either: the due instant is
//! stored and [`ActivityStream::settle`] applies it from the record's own `settle`.

use super::*;

/// The `locked` member of `AuditPolicyState` and of `ActivityStreamPolicyStatus`: "when it is
/// locked, it is read-only".
pub const AUDIT_POLICY_STATE_LOCKED: &str = "locked";

/// The `unlocked` member of both enums: "when a policy is unlocked, it is read/write".
pub const AUDIT_POLICY_STATE_UNLOCKED: &str = "unlocked";

/// The state an activity stream's audit policy is in until a modification moves it:
/// `ModifyActivityStream` changes it "to either locked (default) or unlocked", so `locked` is
/// the state a policy nothing has modified is reported in, and the state an absent
/// `AuditPolicyState` asks for.
pub const DEFAULT_AUDIT_POLICY_STATE: &str = AUDIT_POLICY_STATE_LOCKED;

/// The `locking-policy` member of `ActivityStreamPolicyStatus`: the policy is on its way to
/// [`AUDIT_POLICY_STATE_LOCKED`].
pub const ACTIVITY_STREAM_POLICY_LOCKING: &str = "locking-policy";

/// The `unlocking-policy` member of `ActivityStreamPolicyStatus`: the policy is on its way to
/// [`AUDIT_POLICY_STATE_UNLOCKED`].
pub const ACTIVITY_STREAM_POLICY_UNLOCKING: &str = "unlocking-policy";

/// The sentence `ModifyActivityStream` refuses a DB instance whose engine has no database
/// activity stream to modify with.
///
/// It is the operation's OWN published precondition — "This operation is supported for RDS
/// for Oracle and Microsoft SQL Server" — and has no variable part: the refusal states which
/// engines the operation serves, which is what tells a caller that no amount of waiting will
/// change the answer.
pub const ACTIVITY_STREAM_ENGINE_MESSAGE: &str =
    "This operation is supported for RDS for Oracle and Microsoft SQL Server.";

/// How the ARN member all three activity-stream operations address their resource by is named
/// in a refusal of the VALUE it carried: `ResourceArn`, the member each of them declares
/// (`smithy_query(query="operation", operation="ModifyActivityStream")`), spelled as the model
/// spells it — which is what a caller matching the refusal against the parameter it sent looks
/// for.
pub const ACTIVITY_STREAM_RESOURCE_ARN_PARAMETER: &str = "ResourceArn";

/// Whether a DB instance running `engine` has an audit policy this operation modifies.
///
/// The operation publishes the engines it serves — "This operation is supported for RDS for
/// Oracle and Microsoft SQL Server" — and its `ResourceArn` names "the RDS for Oracle or
/// Microsoft SQL Server DB instance", so the engine families are exactly those two, each
/// matched by the prefix every one of their engine names carries (`oracle-ee`,
/// `sqlserver-se`, …). The comparison is case-INSENSITIVE, the way every per-engine rule of
/// this service compares an engine name ([`switchover_is_supported_for`]), because `Engine`
/// is accepted case-insensitively on a create and stored as the request spelled it.
///
/// The RDS CUSTOM families (`custom-oracle-*`, `custom-sqlserver-*`) are deliberately NOT
/// among them: database activity streams are one of the features RDS Custom does not support
/// ("RDS Custom unsupported features", Amazon RDS User Guide), so an instance of one has no
/// activity stream and no audit policy for this call to change.
///
/// This is a different question from the one
/// `OrderableDBInstanceOption$SupportedActivityStreamModes` answers
/// ([`crate::ops::describe_orderable_db_instance_options`]), which is why the two rules do
/// not list the same engines: that member reports the MODES a stream can be STARTED in and
/// its own documentation names only Aurora PostgreSQL, Aurora MySQL and RDS for Oracle,
/// while the audit policy this operation writes is the one Amazon RDS keeps for the two
/// engines whose streams have an audit policy at all — "This operation is supported for RDS
/// for Oracle and Microsoft SQL Server". Each rule follows the published text of the member
/// or operation it belongs to.
pub fn engine_has_audit_policy(engine: &str) -> bool {
    let lowered = engine.to_ascii_lowercase();
    lowered.starts_with("oracle") || lowered.starts_with("sqlserver")
}

/// The audit policy state of one DB instance's database activity stream: the state it is in,
/// and the state a modification has asked for but the clock has not made due yet.
#[derive(Debug, Clone)]
pub struct ActivityStreamPolicy {
    /// The settled state: [`AUDIT_POLICY_STATE_LOCKED`] or [`AUDIT_POLICY_STATE_UNLOCKED`].
    state: String,
    /// Where the policy is headed and when it gets there, for a change still in flight;
    /// `None` for a policy that is where it was asked to be.
    pending: Option<PendingAuditPolicyState>,
}

/// A change of audit policy state this service owes the clock.
#[derive(Debug, Clone)]
struct PendingAuditPolicyState {
    /// The state the policy reaches when the change is due.
    state: String,
    /// When that is.
    due_at: Instant,
}

impl ActivityStreamPolicy {
    /// The policy of an instance nothing has modified: the documented default state, with
    /// nothing in flight.
    fn unmodified() -> Self {
        Self {
            state: DEFAULT_AUDIT_POLICY_STATE.to_string(),
            pending: None,
        }
    }

    /// The state the policy is in or is on its way to — what a request asking for the same
    /// state again is compared against.
    fn target(&self) -> &str {
        self.pending
            .as_ref()
            .map_or(self.state.as_str(), |pending| pending.state.as_str())
    }

    /// `ActivityStreamPolicyStatus` as it stands: the settled state, or the transitional
    /// member naming where a change in flight is going.
    pub fn status(&self) -> &str {
        match self.pending.as_ref() {
            Some(pending) if pending.state == AUDIT_POLICY_STATE_UNLOCKED => {
                ACTIVITY_STREAM_POLICY_UNLOCKING
            }
            Some(_) => ACTIVITY_STREAM_POLICY_LOCKING,
            None => self.state.as_str(),
        }
    }

    /// Ask the policy for `state`.
    ///
    /// A state the policy is already in, or already moving to, leaves it exactly as it is —
    /// the call is idempotent in the value — and any other starts the transition the clock
    /// then settles.
    fn request(&mut self, state: &str) {
        if self.target() == state {
            return;
        }
        self.pending = Some(PendingAuditPolicyState {
            state: state.to_string(),
            due_at: Instant::now() + STATUS_TRANSITION_DELAY,
        });
    }

    /// Apply a change of state whose due instant has passed; see
    /// [`DbInstanceRecord::settle`], which is what calls this.
    pub(super) fn settle(&mut self, now: Instant) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at)
        {
            if let Some(pending) = self.pending.take() {
                self.state = pending.state;
            }
        }
    }
}

/// Why a modification of an activity stream's audit policy was refused, as the CLASS of
/// refusal rather than as one operation's error enum, so the handler lifts each into its own
/// modelled errors (`smithy_query(query="operation", operation="ModifyActivityStream")`).
#[derive(Debug, Clone)]
pub enum ActivityStreamRefusal {
    /// `ResourceArn` carries no Amazon RDS DB instance ARN this endpoint can act on — a value
    /// that is no ARN, an ARN of another of this service's resource types, or a well-formed
    /// ARN naming another Region than the one the request's endpoint serves. The value itself
    /// is unusable, so this is refused on the MEMBER that carried it rather than reported as a
    /// resource that might yet appear.
    UnusableArn(String),
    /// A DB instance ARN that names no DB instance this account holds in the request's
    /// region.
    NotFound(String),
    /// The instance is there, but is not one this operation can be performed on: its engine
    /// has no database activity stream, or it is not in a state a modification can be taken
    /// in.
    InvalidState(String),
}

/// What an accepted modification reports about the instance's activity stream: the status of
/// the stream itself, and the status of its audit policy.
#[derive(Debug, Clone)]
pub struct ActivityStreamReport {
    /// `ActivityStreamStatus` for the instance's stream, from the instance's own record —
    /// the same value `DBInstance$ActivityStreamStatus` reports.
    pub status: String,
    /// `ActivityStreamPolicyStatus` as the modification left it: the state asked for, or the
    /// transitional member while the change is in flight.
    pub policy_status: String,
    /// The STREAM the policy belongs to, for an instance whose stream is running: the
    /// modification reports the Kinesis stream the activity is delivered to, the key its
    /// messages are encrypted under and its mode beside the two statuses. `None` for an
    /// instance no stream was started for, which has none of that configuration to report.
    pub stream: Option<ActivityStreamDelivery>,
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Write the audit policy state of the activity stream of the DB instance `resource_arn`
    /// names in `region`, and report the stream and its policy as they now stand.
    ///
    /// Resolution is by the ARN each stored instance RENDERS ([`DbInstanceRecord::arn`]), so
    /// an ARN of another account or partition resolves to nothing here: the only instance this
    /// request can act on is one whose own reports carry the ARN it named. An ARN naming
    /// another REGION is refused on the member before that lookup, because a regional endpoint
    /// could not act on it whatever it holds ([`check_arn_parameter_region`]).
    pub fn set_activity_stream_audit_policy(
        &mut self,
        region: &str,
        resource_arn: &str,
        audit_policy_state: Option<&str>,
    ) -> Result<ActivityStreamReport, ActivityStreamRefusal> {
        // `ResourceArn` names "the RDS for Oracle or Microsoft SQL Server DB INSTANCE", so a
        // value that is no DB instance ARN of this service — one that is no ARN, or an ARN
        // of another of its resource types — names no DB instance for this call to resolve
        // at all, and is refused on the member that carried it as a resource this service
        // does not know.
        let addressed = parse_rds_resource_arn(resource_arn).filter(|parsed| {
            tags::TaggedKind::from_arn_resource_type(parsed.resource_type)
                == Some(tags::TaggedKind::DbInstance)
        });
        let Some(parsed) = addressed else {
            return Err(ActivityStreamRefusal::UnusableArn(
                tags::invalid_resource_name_message(resource_arn),
            ));
        };
        // And the ARN has to name the Region this endpoint serves: Amazon RDS is regional, so
        // an ARN of another Region names no DB instance this call could ever act on, whichever
        // Region holds one of that identifier. That is a bad value rather than a missing
        // instance, so it is answered on the member BEFORE the lookup
        // ([`check_arn_parameter_region`]).
        if let Err(message) = check_arn_parameter_region(
            ACTIVITY_STREAM_RESOURCE_ARN_PARAMETER,
            resource_arn,
            region,
        ) {
            return Err(ActivityStreamRefusal::UnusableArn(message));
        }

        // The DB instance the ARN names is its resource-name segment — the identifier —
        // which is also the handle the refusal names the instance by.
        let named = parsed.identifier.to_string();
        // The stored key of the instance the ARN names, folded the way every DB instance key
        // is ("stored as a lowercase string").
        let key = (region.to_string(), named.to_ascii_lowercase());
        // The wording this front door answers a `DBInstanceNotFoundFault` with on a call that
        // ACTS on the instance: the fixed phrase, then the instance the request named.
        let not_found =
            || ActivityStreamRefusal::NotFound(format!("DBInstance not found: {named}"));
        let Some(record) = self.db_instances.get_mut(&key) else {
            return Err(not_found());
        };
        // An account acts on its own resources: the only instance this request can act on is
        // one whose own reports carry the ARN it named, so an ARN of another account or
        // partition resolves to nothing here.
        if record.arn() != resource_arn {
            return Err(not_found());
        }
        // "This operation is supported for RDS for Oracle and Microsoft SQL Server": an
        // instance of any other engine has no audit policy for this call to change, which no
        // waiting will alter.
        if !engine_has_audit_policy(&record.engine) {
            return Err(ActivityStreamRefusal::InvalidState(
                ACTIVITY_STREAM_ENGINE_MESSAGE.to_string(),
            ));
        }
        // A modification of an instance's configuration needs the same available instance
        // every other modification of one does, and is refused with the sentence RDS words
        // for a modification an instance's status is not in a state for.
        if record.status != DB_INSTANCE_STATUS_AVAILABLE {
            return Err(ActivityStreamRefusal::InvalidState(
                modify_db_instance_invalid_state_message(&record.status),
            ));
        }

        // "to either locked (default) or unlocked": the state the request named, else the
        // default the operation documents for a call that names none.
        let requested = audit_policy_state.unwrap_or(DEFAULT_AUDIT_POLICY_STATE);
        let policy = record
            .activity_stream_policy
            .get_or_insert_with(ActivityStreamPolicy::unmodified);
        policy.request(requested);
        let policy_status = policy.status().to_string();
        // The instance's own log records what this service does to it, and a change of its
        // activity stream's audit policy is one of those things.
        record
            .engine_log
            .record(format!("database activity stream audit policy {policy_status}"));
        Ok(ActivityStreamReport {
            // The stream's own status, as the instance reports it
            // (`DBInstance$ActivityStreamStatus`): changing the audit policy neither starts
            // nor stops the stream — "restarting the activity stream isn't required".
            status: record.activity_stream_status().to_string(),
            policy_status,
            // And the stream's own configuration, for an instance that HAS one running: the
            // modification reports the stream it was performed on, and an instance no stream
            // was started for has none of it to report.
            stream: record.activity_stream_delivery(),
        })
    }
}

/// The `starting` member of `ActivityStreamStatus` (`stopped | starting | started |
/// stopping`): a stream whose start this service owes but has not made due yet.
pub const ACTIVITY_STREAM_STARTING: &str = "starting";

/// The `started` member: the stream is delivering the resource's activity.
pub const ACTIVITY_STREAM_STARTED: &str = "started";

/// The `stopping` member: a stream whose stop this service owes but has not made due yet.
pub const ACTIVITY_STREAM_STOPPING: &str = "stopping";

/// The prefix of the name of the Amazon Kinesis data stream a database activity stream is
/// delivered to: Amazon RDS names that stream `aws-rds-das-` followed by the immutable
/// resource id of the resource whose activity it carries (Amazon Aurora User Guide /
/// Amazon RDS User Guide, "Monitoring … with Database Activity Streams": the activity stream
/// is delivered to a Kinesis data stream named `aws-rds-das-<resource id>`).
pub const ACTIVITY_STREAM_KINESIS_NAME_PREFIX: &str = "aws-rds-das-";

/// `KinesisStreamName` for the resource whose immutable resource id is `resource_id` — a DB
/// cluster's `DbClusterResourceId` or a DB instance's `DbiResourceId`.
///
/// DERIVED from the record rather than stored as text, so the name a start answers, the name
/// the resource's describe reports and the name its stop answers are always the same one, and
/// a stream cannot be left naming a delivery target of a resource it does not belong to.
pub fn activity_stream_kinesis_stream_name(resource_id: &str) -> String {
    format!("{ACTIVITY_STREAM_KINESIS_NAME_PREFIX}{resource_id}")
}

/// Provision the Kinesis data stream a database activity stream is DELIVERED to, in the
/// account's own Kinesis.
///
/// Amazon RDS creates that stream itself as part of starting the activity stream — a caller
/// names no delivery target and creates none — so the start asks the service that OWNS data
/// streams for it, over the in-process callback through the one shared client construction
/// ([`::service_common::kinesis_client`]): no socket is opened and nothing leaves the process.
/// The stream is created in the ON-DEMAND capacity mode, because its capacity is Amazon RDS's
/// to manage rather than a number the caller was asked for.
///
/// A stream Kinesis already holds under this name is the target this stream delivers to
/// already — a resource whose stream is started again after a stop keeps the one name — so an
/// answer that the name is in use is no failure here. Nothing else about the answer is acted
/// on: where no Kinesis answers at all (a process this service was mounted into without it)
/// there is no target to provision and nothing to report about one.
pub async fn provision_activity_stream_delivery_target(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kinesis_stream_name: &str,
) {
    let Ok(on_demand) = ::aws_sdk_kinesis::types::StreamModeDetails::builder()
        .stream_mode(::aws_sdk_kinesis::types::StreamMode::OnDemand)
        .build()
    else {
        return;
    };
    let _ = ::service_common::kinesis_client::kinesis_client(ctx, region)
        .create_stream()
        .stream_name(kinesis_stream_name)
        .stream_mode_details(on_demand)
        .send()
        .await;
}

/// And take it away again, which is the other half of what Amazon RDS manages for the caller:
/// the Kinesis data stream of a stopped activity stream is deleted, so a resource whose stream
/// was stopped leaves no delivery target behind for a caller to pay for or read from.
///
/// Asked of the same owner over the same in-process hop, and the answer is not acted on for
/// the same reason: a stream Kinesis no longer holds is already gone.
pub async fn release_activity_stream_delivery_target(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kinesis_stream_name: &str,
) {
    let _ = ::service_common::kinesis_client::kinesis_client(ctx, region)
        .delete_stream()
        .stream_name(kinesis_stream_name)
        .send()
        .await;
}

/// The sentence a start is refused a DB CLUSTER whose engine has no database activity stream
/// with.
///
/// Amazon RDS serves database activity streams for the AURORA engines at the cluster level —
/// `StartActivityStream$ResourceArn` names "the DB cluster" of an Aurora database activity
/// stream, and the feature is documented in the Amazon Aurora User Guide for Aurora MySQL and
/// Aurora PostgreSQL — so a cluster of any other engine this front door serves has no stream
/// to start. Like the Data API toggle's own precondition
/// ([`DATA_API_ENDPOINT_CLUSTER_TYPE_MESSAGE`]) it states which clusters the operation serves
/// and has no variable part, which is what tells a caller that no amount of waiting will
/// change the answer.
pub const ACTIVITY_STREAM_CLUSTER_ENGINE_MESSAGE: &str =
    "Database activity streams are supported only for Aurora DB clusters.";

/// The sentence a start is refused with where the resource's stream is already running.
///
/// A stream is started once: its mode and its key are the configuration of the stream that is
/// running, and changing either means stopping the stream and starting it again ("you must
/// stop the activity stream before you can start it with a different configuration"), so a
/// start that arrives while one is started or starting is refused rather than taken as a
/// second start of the same stream. It states the condition and nothing about the request,
/// which is what tells a caller to stop the stream first.
pub const ACTIVITY_STREAM_ALREADY_STARTED_MESSAGE: &str =
    "The database activity stream is already started.";

/// And the sentence a stop is refused with where the resource has no stream running: there is
/// nothing to stop, which no waiting will change.
pub const ACTIVITY_STREAM_NOT_STARTED_MESSAGE: &str =
    "The database activity stream isn't started.";

/// Whether a DB INSTANCE running `engine` has a database activity stream to start at all.
///
/// Amazon RDS serves database activity streams at the INSTANCE level for RDS for Oracle and
/// RDS for Microsoft SQL Server — the same two engine families whose streams carry the audit
/// policy `ModifyActivityStream` writes, which is why the predicate is read from
/// [`engine_has_audit_policy`] rather than listed a second time: an instance that has a
/// stream has a policy on it, and one that has neither is refused by the one rule. The
/// clusters' rule is the Aurora one above ([`ACTIVITY_STREAM_CLUSTER_ENGINE_MESSAGE`]).
pub fn instance_engine_has_activity_stream(engine: &str) -> bool {
    engine_has_audit_policy(engine)
}

/// The database activity stream of one DB cluster or DB instance: the configuration the start
/// recorded, and where the delivery is in the lifecycle `ActivityStreamStatus` models.
///
/// A record exists for a resource whose stream is running or on its way to or from running;
/// a resource with no record reports the `stopped` member, which is also what a stream whose
/// stop has come due leaves behind ([`Self::settle`] takes the record away).
#[derive(Debug, Clone)]
pub struct ActivityStream {
    /// `ActivityStreamMode`: "the mode of the database activity stream" (`async | sync`), as
    /// the start asked for it.
    pub mode: String,
    /// `ActivityStreamKmsKeyId`: the KMS key "for encrypting messages in the database
    /// activity stream", as the start named it.
    pub kms_key_id: String,
    /// `ActivityStreamEngineNativeAuditFieldsIncluded`: whether engine-native audit fields
    /// travel with the stream's messages.
    ///
    /// `None` for a stream on a resource the option does not apply to: the member "applies to
    /// an Oracle or Microsoft SQL Server DB instance", and a DB cluster reports no such
    /// member at all, so nothing is invented for one.
    pub engine_native_audit_fields_included: Option<bool>,
    /// The SETTLED status: [`ACTIVITY_STREAM_STARTED`] or [`ACTIVITY_STREAM_STOPPED`].
    state: String,
    /// Where the stream is headed and when it gets there, for a change still in flight;
    /// `None` for a stream that is where it was asked to be.
    pending: Option<PendingActivityStreamState>,
}

/// A change of a stream's status this service owes the clock.
#[derive(Debug, Clone)]
struct PendingActivityStreamState {
    /// The status the stream reaches when the change is due.
    state: String,
    /// When that is, for a change the caller asked to apply "as soon as possible, regardless
    /// of the maintenance window"; `None` for one that waits for the resource's next
    /// maintenance window, which is what `ApplyImmediately` disabled asks for.
    due_at: Option<Instant>,
}

impl ActivityStream {
    /// The stream a start records: the configuration the request named, on its way to
    /// [`ACTIVITY_STREAM_STARTED`].
    fn starting(
        mode: &str,
        kms_key_id: &str,
        engine_native_audit_fields_included: Option<bool>,
        apply_immediately: bool,
    ) -> Self {
        Self {
            mode: mode.to_string(),
            kms_key_id: kms_key_id.to_string(),
            engine_native_audit_fields_included,
            // A stream is stopped until the start this call accepted is due: the status is
            // reported as the transition it is, and the resource's describe reports the same
            // thing, so a caller that reads either sees the stream coming up.
            state: ACTIVITY_STREAM_STOPPED.to_string(),
            pending: Some(PendingActivityStreamState::reaching(
                ACTIVITY_STREAM_STARTED,
                apply_immediately,
            )),
        }
    }

    /// Ask this stream to stop: the delivery keeps running — and keeps being reported — until
    /// the stop is due, which is what the `stopping` member says.
    fn stop(&mut self, apply_immediately: bool) {
        self.pending = Some(PendingActivityStreamState::reaching(
            ACTIVITY_STREAM_STOPPED,
            apply_immediately,
        ));
    }

    /// `ActivityStreamStatus` as it stands: the settled status, or the transitional member
    /// naming where a change in flight is going.
    pub fn status(&self) -> &str {
        match self.pending.as_ref() {
            Some(pending) if pending.state == ACTIVITY_STREAM_STARTED => ACTIVITY_STREAM_STARTING,
            Some(_) => ACTIVITY_STREAM_STOPPING,
            None => self.state.as_str(),
        }
    }

    /// Whether this stream is RUNNING — started, or on its way to started.
    ///
    /// The question a second start and a stop are both decided by: a running stream cannot be
    /// started again, and only a running one can be stopped.
    pub fn is_running(&self) -> bool {
        self.pending
            .as_ref()
            .map_or(self.state.as_str(), |pending| pending.state.as_str())
            == ACTIVITY_STREAM_STARTED
    }

    /// This stream as a call on it, or the resource's own describe, reports it, with the
    /// delivery target derived from the resource's immutable `resource_id`.
    pub fn delivery(&self, resource_id: &str) -> ActivityStreamDelivery {
        ActivityStreamDelivery {
            status: self.status().to_string(),
            mode: self.mode.clone(),
            kms_key_id: self.kms_key_id.clone(),
            kinesis_stream_name: activity_stream_kinesis_stream_name(resource_id),
            engine_native_audit_fields_included: self.engine_native_audit_fields_included,
        }
    }

    /// Apply a change of status whose due instant has passed, or whose resource's maintenance
    /// `window` has come round. Returns whether the stream is now GONE — a stop that has run
    /// its course leaves no stream on the resource, which is what reporting the `stopped`
    /// member and none of a stream's configuration says — so the record's own `settle` takes
    /// it away.
    pub(super) fn settle(&mut self, now: Instant, window: Option<&str>) -> bool {
        let due = self
            .pending
            .as_ref()
            .is_some_and(|pending| maintenance_window::change_is_due(pending.due_at, window, now));
        if due {
            if let Some(pending) = self.pending.take() {
                self.state = pending.state;
            }
        }
        self.pending.is_none() && self.state == ACTIVITY_STREAM_STOPPED
    }
}

impl PendingActivityStreamState {
    /// The change that takes a stream to `state`, due at the span this service owes a
    /// transition where the caller asked for it "as soon as possible, regardless of the
    /// maintenance window for the database", and in the resource's next maintenance window
    /// otherwise.
    fn reaching(state: &str, apply_immediately: bool) -> Self {
        Self {
            state: state.to_string(),
            due_at: apply_immediately.then(|| Instant::now() + STATUS_TRANSITION_DELAY),
        }
    }
}

/// What a call on a database activity stream, and the resource's own describe, report about
/// it: the four members both carry under their own names.
#[derive(Debug, Clone)]
pub struct ActivityStreamDelivery {
    /// `Status` / `ActivityStreamStatus`.
    pub status: String,
    /// `Mode` / `ActivityStreamMode`.
    pub mode: String,
    /// `KmsKeyId` / `ActivityStreamKmsKeyId`.
    pub kms_key_id: String,
    /// `KinesisStreamName` / `ActivityStreamKinesisStreamName`.
    pub kinesis_stream_name: String,
    /// `EngineNativeAuditFieldsIncluded` /
    /// `ActivityStreamEngineNativeAuditFieldsIncluded`, for a resource the option applies to
    /// ([`ActivityStream::engine_native_audit_fields_included`]).
    pub engine_native_audit_fields_included: Option<bool>,
}

/// Why a start or a stop of a database activity stream was refused, as the CLASS of refusal
/// rather than as one operation's error enum.
///
/// A stream belongs either to a DB cluster or to a DB instance, and both operations model a
/// not-found and an invalid-state fault for EACH
/// (`smithy_query(query="operation", operation="StartActivityStream")`), so the class carries
/// which of the two the `ResourceArn` named and each handler lifts it into its own modelled
/// errors.
#[derive(Debug, Clone)]
pub enum ActivityStreamDeliveryRefusal {
    /// `ResourceArn` carries no Amazon RDS DB cluster or DB instance ARN this endpoint can
    /// act on — a value that is no ARN, an ARN of another of this service's resource types,
    /// or a well-formed ARN naming another Region than the one the request's endpoint serves.
    /// The value itself is unusable, so this is refused on the MEMBER that carried it rather
    /// than reported as a resource that might yet appear.
    UnusableArn(String),
    /// A DB cluster ARN that names no cluster this account holds in the request's region.
    ClusterNotFound(String),
    /// A DB instance ARN that names no instance this account holds in the request's region.
    InstanceNotFound(String),
    /// The cluster is there, but is not one the call can be performed on: its engine has no
    /// database activity stream, it is not in a state the call can be taken in, or its stream
    /// is already in the state the call asks for.
    InvalidClusterState(String),
    /// The same, for a DB instance.
    InvalidInstanceState(String),
}

/// Which resource a `ResourceArn` of a start or a stop named, as the key of the stored
/// record in the store that holds it.
#[derive(Debug, Clone)]
enum ActivityStreamSubject {
    /// A DB cluster, keyed in [`ComAmazonawsRdsAmazonrdsv19State::db_clusters`].
    Cluster(RegionalKey),
    /// A DB instance, keyed in [`ComAmazonawsRdsAmazonrdsv19State::db_instances`].
    Instance(RegionalKey),
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The resource `resource_arn` names in `region`, for a call on its database activity
    /// stream.
    ///
    /// Resolution is by the ARN each stored record RENDERS ([`DbClusterRecord::arn`],
    /// [`DbInstanceRecord::arn`]), so an ARN of another account or partition resolves to
    /// nothing here: the only resource this request can act on is one whose own reports carry
    /// the ARN it named. An ARN naming another REGION is refused on the member before that
    /// lookup, because a regional endpoint could not act on it whatever it holds
    /// ([`check_arn_parameter_region`]).
    ///
    /// Each miss is worded the way this front door words the FAULT the call raises for that
    /// resource: the cluster's `DBClusterNotFoundFault` in the
    /// `<resource type> not found: <named>` form its association and snapshot siblings answer
    /// with ([`db_cluster_not_found_colon_message`]), carrying the ARN the request wrote, and
    /// the instance's by the handle the ARN names, as the audit policy write above words it —
    /// so the two activity-stream calls cannot drift from their siblings. The Data API
    /// toggles raise a DIFFERENT fault for a cluster they cannot resolve
    /// (`ResourceNotFoundFault`), which Amazon RDS words its own way
    /// ([`data_api_cluster_not_found_message`]).
    fn activity_stream_subject(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Result<ActivityStreamSubject, ActivityStreamDeliveryRefusal> {
        let unusable = || {
            ActivityStreamDeliveryRefusal::UnusableArn(tags::invalid_resource_name_message(
                resource_arn,
            ))
        };
        let Some(parsed) = parse_rds_resource_arn(resource_arn) else {
            return Err(unusable());
        };
        // The stored key of the resource the ARN names, folded the way every key of both
        // stores is ("stored as a lowercase string").
        let key = (region.to_string(), parsed.identifier.to_ascii_lowercase());
        // The two resource kinds a database activity stream belongs to; every other resource
        // type this service keeps records of — and every value that is no ARN at all — names
        // no resource one belongs to.
        let kind = match tags::TaggedKind::from_arn_resource_type(parsed.resource_type) {
            Some(kind @ (tags::TaggedKind::DbCluster | tags::TaggedKind::DbInstance)) => kind,
            _ => return Err(unusable()),
        };
        // Amazon RDS is regional: whichever of the two kinds the ARN names, an ARN of another
        // Region names nothing this endpoint can act on, which is a bad value rather than a
        // missing resource and is answered on the member before the lookup
        // ([`check_arn_parameter_region`]).
        if let Err(message) = check_arn_parameter_region(
            ACTIVITY_STREAM_RESOURCE_ARN_PARAMETER,
            resource_arn,
            region,
        ) {
            return Err(ActivityStreamDeliveryRefusal::UnusableArn(message));
        }
        match kind {
            tags::TaggedKind::DbCluster => {
                if self
                    .db_clusters
                    .get(&key)
                    .is_none_or(|record| record.arn() != resource_arn)
                {
                    return Err(ActivityStreamDeliveryRefusal::ClusterNotFound(
                        db_cluster_not_found_colon_message(resource_arn),
                    ));
                }
                Ok(ActivityStreamSubject::Cluster(key))
            }
            tags::TaggedKind::DbInstance => {
                if self
                    .db_instances
                    .get(&key)
                    .is_none_or(|record| record.arn() != resource_arn)
                {
                    return Err(ActivityStreamDeliveryRefusal::InstanceNotFound(
                        db_instance_not_found_colon_message(parsed.identifier),
                    ));
                }
                Ok(ActivityStreamSubject::Instance(key))
            }
            // The kind was narrowed to the two above before the lookup.
            _ => Err(unusable()),
        }
    }

    /// Start the database activity stream of the resource `resource_arn` names in `region`,
    /// and report it as it now stands.
    ///
    /// "Starts a database activity stream to monitor activity on the database": the stream's
    /// configuration is written onto the resource's own record, so the resource's describe
    /// reports the same stream from this call on, and the delivery reaches `started` on this
    /// service's clock — at once for a call that asked for it, else in the resource's next
    /// maintenance window ([`ActivityStream`]).
    pub fn start_activity_stream(
        &mut self,
        region: &str,
        resource_arn: &str,
        mode: &str,
        kms_key_id: &str,
        engine_native_audit_fields_included: Option<bool>,
        apply_immediately: bool,
    ) -> Result<ActivityStreamDelivery, ActivityStreamDeliveryRefusal> {
        match self.activity_stream_subject(region, resource_arn)? {
            ActivityStreamSubject::Cluster(key) => {
                let Some(record) = self.db_clusters.get_mut(&key) else {
                    return Err(ActivityStreamDeliveryRefusal::ClusterNotFound(
                        db_cluster_not_found_colon_message(resource_arn),
                    ));
                };
                // The engine has to have a stream to start, and the cluster has to be in a
                // state a change of its configuration can be taken in — the same available
                // cluster every other modification of one needs, refused with the sentence
                // RDS words for a modification a cluster's status is not in a state for.
                if !is_aurora(&record.engine) {
                    return Err(ActivityStreamDeliveryRefusal::InvalidClusterState(
                        ACTIVITY_STREAM_CLUSTER_ENGINE_MESSAGE.to_string(),
                    ));
                }
                if record.status != DB_CLUSTER_STATUS_AVAILABLE {
                    return Err(ActivityStreamDeliveryRefusal::InvalidClusterState(
                        db_cluster_not_available_for_modification_message(&record.status),
                    ));
                }
                if record
                    .activity_stream
                    .as_ref()
                    .is_some_and(ActivityStream::is_running)
                {
                    return Err(ActivityStreamDeliveryRefusal::InvalidClusterState(
                        ACTIVITY_STREAM_ALREADY_STARTED_MESSAGE.to_string(),
                    ));
                }
                // `EngineNativeAuditFieldsIncluded` "applies to an Oracle or Microsoft SQL
                // Server DB instance", and `DBCluster` carries no member for it, so a
                // cluster's stream records none of it rather than reporting a setting its
                // own describe has nowhere to carry.
                let stream = ActivityStream::starting(mode, kms_key_id, None, apply_immediately);
                let started = stream.delivery(&record.resource_id);
                record.activity_stream = Some(stream);
                Ok(started)
            }
            ActivityStreamSubject::Instance(key) => {
                let Some(record) = self.db_instances.get_mut(&key) else {
                    return Err(ActivityStreamDeliveryRefusal::InstanceNotFound(
                        db_instance_not_found_colon_message(resource_arn),
                    ));
                };
                if !instance_engine_has_activity_stream(&record.engine) {
                    return Err(ActivityStreamDeliveryRefusal::InvalidInstanceState(
                        ACTIVITY_STREAM_ENGINE_MESSAGE.to_string(),
                    ));
                }
                if record.status != DB_INSTANCE_STATUS_AVAILABLE {
                    return Err(ActivityStreamDeliveryRefusal::InvalidInstanceState(
                        modify_db_instance_invalid_state_message(&record.status),
                    ));
                }
                if record
                    .activity_stream
                    .as_ref()
                    .is_some_and(ActivityStream::is_running)
                {
                    return Err(ActivityStreamDeliveryRefusal::InvalidInstanceState(
                        ACTIVITY_STREAM_ALREADY_STARTED_MESSAGE.to_string(),
                    ));
                }
                // "Specifies whether the database activity stream includes engine-native
                // audit fields … By default, no engine-native audit fields are included":
                // the option applies to these engines, so the stream records what the
                // request asked for, else the documented default.
                let stream = ActivityStream::starting(
                    mode,
                    kms_key_id,
                    Some(engine_native_audit_fields_included.unwrap_or(false)),
                    apply_immediately,
                );
                let started = stream.delivery(&record.dbi_resource_id);
                record.activity_stream = Some(stream);
                // The instance's own log records what this service does to it, and starting
                // its database activity stream is one of those things.
                record
                    .engine_log
                    .record("database activity stream starting".to_string());
                Ok(started)
            }
        }
    }

    /// Stop the database activity stream of the resource `resource_arn` names in `region`,
    /// and report it as it now stands.
    ///
    /// "Stops a database activity stream that was started …": the delivery is still the one
    /// the start recorded — the stop reports its Kinesis stream and its key — and reaches
    /// `stopped` on this service's clock, after which the resource reports no stream at all.
    pub fn stop_activity_stream(
        &mut self,
        region: &str,
        resource_arn: &str,
        apply_immediately: bool,
    ) -> Result<ActivityStreamDelivery, ActivityStreamDeliveryRefusal> {
        match self.activity_stream_subject(region, resource_arn)? {
            ActivityStreamSubject::Cluster(key) => {
                let Some(record) = self.db_clusters.get_mut(&key) else {
                    return Err(ActivityStreamDeliveryRefusal::ClusterNotFound(
                        db_cluster_not_found_colon_message(resource_arn),
                    ));
                };
                if record.status != DB_CLUSTER_STATUS_AVAILABLE {
                    return Err(ActivityStreamDeliveryRefusal::InvalidClusterState(
                        db_cluster_not_available_for_modification_message(&record.status),
                    ));
                }
                let resource_id = record.resource_id.clone();
                let Some(stream) = record
                    .activity_stream
                    .as_mut()
                    .filter(|stream| stream.is_running())
                else {
                    return Err(ActivityStreamDeliveryRefusal::InvalidClusterState(
                        ACTIVITY_STREAM_NOT_STARTED_MESSAGE.to_string(),
                    ));
                };
                stream.stop(apply_immediately);
                Ok(stream.delivery(&resource_id))
            }
            ActivityStreamSubject::Instance(key) => {
                let Some(record) = self.db_instances.get_mut(&key) else {
                    return Err(ActivityStreamDeliveryRefusal::InstanceNotFound(
                        db_instance_not_found_colon_message(resource_arn),
                    ));
                };
                if record.status != DB_INSTANCE_STATUS_AVAILABLE {
                    return Err(ActivityStreamDeliveryRefusal::InvalidInstanceState(
                        modify_db_instance_invalid_state_message(&record.status),
                    ));
                }
                let resource_id = record.dbi_resource_id.clone();
                let Some(stream) = record
                    .activity_stream
                    .as_mut()
                    .filter(|stream| stream.is_running())
                else {
                    return Err(ActivityStreamDeliveryRefusal::InvalidInstanceState(
                        ACTIVITY_STREAM_NOT_STARTED_MESSAGE.to_string(),
                    ));
                };
                stream.stop(apply_immediately);
                let stopped = stream.delivery(&resource_id);
                record
                    .engine_log
                    .record("database activity stream stopping".to_string());
                Ok(stopped)
            }
        }
    }
}
