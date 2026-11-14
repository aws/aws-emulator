// SPDX-License-Identifier: CC0-1.0

//! @op CreateReplaceRootVolumeTask
//! Implemented: starts one root volume replacement task for the instance the request
//! names, stores it, and answers it as it then stands — this is the ONLY file to touch for
//! this operation.
//!
//! ## What it does
//!
//! "Replaces the EBS-backed root volume for a running instance with a new volume that is
//! restored to the original root volume's launch state, that is restored to a specific
//! snapshot taken from the original root volume, that is restored from an AMI that has the
//! same key characteristics as that of the instance, or that is replaced by a specified
//! volume." Those four forms are what the request's `SnapshotId`, `ImageId` and `VolumeId`
//! members select between, and each of the three says the other two must be omitted ("If
//! you want to restore the replacement root volume from a specific snapshot, an AMI, or to
//! its launch state, omit this parameter", `VolumeId`), so naming more than one is
//! refused with `InvalidParameterCombination` rather than silently preferring one.
//!
//! The task is a resource of the instance, so the instance is resolved first — and by
//! THIS operation's own validator: an instance id it cannot resolve is answered
//! `InvalidInstanceID.Malformed`, whether or not the value reads like the long id form,
//! which is what the recorded unit `ec2-instance-dependent-ops-error-paths#task#4` pins
//! for this operation (see [`require_replaceable_instance`], which also records why the
//! shared [`crate::state::require_known_instance`] — the malformed-before-not-found
//! helper other operations use — is NOT what answers here). An instance that is on its way
//! to `terminated` has no root volume left to replace, so it is refused with
//! `IncorrectInstanceState`. The restore source is resolved the same way: an `ImageId` the
//! account cannot see is refused by [`crate::state::require_visible_image`], a `VolumeId`
//! by [`crate::state::require_volume`] plus the three conditions the member
//! documents ("must be in the same Availability Zone as the instance, must be in the
//! `available` state, and must not be attached to an instance"), and a `SnapshotId` by
//! [`crate::state::unresolvable_snapshot_id`] — this emulator stores no EBS snapshots at
//! all, since `CreateSnapshot` is still a scaffold, so no snapshot id can resolve and
//! `InvalidSnapshot.NotFound` is the faithful answer rather than a task that claims to
//! restore from one.
//!
//! `VolumeInitializationRate` carries a published bound the model declares no `@range`
//! for — "Valid range: 100 - 300 MiB/s" — so it is enforced here
//! ([`crate::state::MIN_VOLUME_INITIALIZATION_RATE`]).
//!
//! `SnapshotId` on the task it answers is "the ID of the snapshot used to create the
//! replacement root volume", which for a restore FROM AN AMI is the root device snapshot of
//! the image the volume is restored from, read back off that stored image. A restore to the
//! original root volume's launch state restores the AMI the instance was launched from, so
//! that is the image read for it; a replacement BY a specified volume creates no volume from
//! a snapshot and names none, and neither does a restore whose AMI this account holds no
//! image for. A restore from a snapshot the REQUEST named never gets that far: it is refused
//! above, so no stored task ever reports an id for a snapshot that does not exist. That id
//! is one AWS mints per run — this store mints it the same way, in the
//! published `snap-` plus 17 lowercase hexadecimal digit form, when the image is stored
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::insert_image`]) — so it is the snapshot
//! of THIS run's image rather than the one some recording happened to see; it is reported
//! identically on every read of the task and of the image it came from, which is the
//! invariant such an id carries.
//!
//! `DryRun` sits after every parameter has been validated and before the first write, so
//! it reports whether the request WOULD have succeeded, and one `ClientToken` names the
//! one task it started: a replay answers that task and starts nothing. The request's
//! `TagSpecifications` are read by the crate's one reader of a create's tag specifications
//! ([`crate::state::tags_from_specifications`]), so the resource type it may name, the
//! reserved key namespace, duplicate keys and the per-resource tag bounds are enforced the
//! same way here as in every other create.
//!
//! The task is stored as [`crate::state::ReplaceRootVolumeTaskRecord`] and read back out
//! of the store, so the caller sees what a later `DescribeReplaceRootVolumeTasks` will
//! see. A fresh task is `pending` — "the replacement volume is being created" — and the
//! clock owes it `in-progress` and then `succeeded`; see that record for the spans.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateReplaceRootVolumeTask::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateReplaceRootVolumeTask`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Replaces the EBS-backed root volume for a running instance with a new volume that is
//! restored to the original root volume's launch state, that is restored to a specific
//! snapshot taken from the original root volume, that is restored from an AMI that has the
//! same key characteristics as that of the instance, or that is replaced by a specified
//! volume. For more information, see Replace a root volume in the Amazon EC2 User Guide.
//!
//! ## Input
//!
//! * `ClientToken` (String, @idempotencyToken) — Unique, case-sensitive identifier you
//!   provide to ensure the idempotency of the request. If you do not specify a client token,
//!   a randomly generated token is used for the request to ensure idempotency. For more
//!   information, see Ensuring idempotency.
//! * `DeleteReplacedRootVolume` (Boolean) — Indicates whether to automatically delete the
//!   original root volume after the root volume replacement task completes. To delete the
//!   original root volume, specify true. If you choose to keep the original root volume after
//!   the replacement task completes, you must manually delete it when you no longer need it.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `ImageId` (ImageId) — The ID of the AMI to use to restore the root volume. The specified
//!   AMI must have the same product code, billing information, architecture type, and
//!   virtualization type as that of the instance. If you want to restore the replacement
//!   volume from a specific snapshot, if you want to restore it to its launch state, or if
//!   you want to replace the root volume with a specified volume, omit this parameter.
//! * `InstanceId` (InstanceId) **required** — The ID of the instance for which to replace the
//!   root volume.
//! * `SnapshotId` (SnapshotId) — The ID of the snapshot from which to restore the replacement
//!   root volume. The specified snapshot must be a snapshot that you previously created from
//!   the original root volume. If you want to restore the replacement root volume to the
//!   initial launch state, if you want to restore the replacement root volume from an AMI, or
//!   if you want to replace the root volume with a specified volume, omit this parameter.
//! * `TagSpecifications` (TagSpecificationList) — The tags to apply to the root volume
//!   replacement task.
//! * `VolumeId` (VolumeId) — The ID of the volume to use as the replacement root volume. The
//!   specified volume must be in the same Availability Zone as the instance, must be in the
//!   available state, and must not be attached to an instance. If the original root volume is
//!   encrypted, the specified volume must also be encrypted. If you want to restore the
//!   replacement root volume from a specific snapshot, an AMI, or to its launch state, omit
//!   this parameter.
//! * `VolumeInitializationRate` (Long) — Specifies the Amazon EBS Provisioned Rate for Volume
//!   Initialization (volume initialization rate), in MiB/s, at which to download the snapshot
//!   blocks from Amazon S3 to the replacement root volume. This is also known as volume
//!   initialization. Specifying a volume initialization rate ensures that the volume is
//!   initialized at a predictable and consistent rate after creation. Omit this parameter if:
//!   You want to create the volume using fast snapshot restore. You must specify a snapshot
//!   that is enabled for fast snapshot restore. In this case, the volume is fully initialized
//!   at creation. If you specify a snapshot that is enabled for fast snapshot restore and a
//!   volume initialization rate, the volume …
//!
//! ## Output
//!
//! * `ReplaceRootVolumeTask` (ReplaceRootVolumeTask) — Information about the root volume
//!   replacement task.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateReplaceRootVolumeTaskError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateReplaceRootVolumeTaskError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#CreateReplaceRootVolumeTask")]
#[derive(Debug)]
pub enum CreateReplaceRootVolumeTaskErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_replace_root_volume_task::CreateReplaceRootVolumeTaskError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateReplaceRootVolumeTaskError = crate::scaffolded::Scaffolded<CreateReplaceRootVolumeTaskErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateReplaceRootVolumeTaskErrorModeled> for CreateReplaceRootVolumeTaskError {
    fn from(modeled: CreateReplaceRootVolumeTaskErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateReplaceRootVolumeTask` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateReplaceRootVolumeTask<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateReplaceRootVolumeTask<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateReplaceRootVolumeTask<Eng>
{
    type Input = CreateReplaceRootVolumeTaskInput;
    type Output = CreateReplaceRootVolumeTaskOutput;
    type Error = CreateReplaceRootVolumeTaskError;

    const NAME: &'static str = "CreateReplaceRootVolumeTask";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_REPLACE_ROOT_VOLUME_TASK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_REPLACE_ROOT_VOLUME_TASK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state::{
            MAX_CLIENT_TOKEN_CHARS, MAX_STORED_BYTES, MAX_STORED_RECORDS_PER_STORE,
            MAX_VOLUME_INITIALIZATION_RATE, MIN_VOLUME_INITIALIZATION_RATE,
            REPLACE_ROOT_VOLUME_TASK_TAGGABLE_RESOURCE, ReplaceRootVolumeTaskRecord,
            VOLUME_STATE_AVAILABLE, iso8601_utc, replace_root_volume_task_payload_bytes,
            require_service_state, require_visible_image, require_volume,
            tags_from_specifications, unresolvable_snapshot_id,
        };

        // `ctx` carries request metadata only, and nothing this record holds names a
        // Region: the modelled `ReplaceRootVolumeTask` carries no ARN and no zone, and the
        // zone the volume check compares against is read off the instance.
        let _ = ctx;

        // The four restore forms are mutually exclusive: each of the three members that
        // selects one says the other two must be omitted ("If you want to restore the
        // replacement root volume from a specific snapshot, an AMI, or to its launch state,
        // omit this parameter", `VolumeId`), and naming none of them is the fourth form —
        // the original root volume's launch state.
        let sources: [(&str, Option<&str>); 3] = [
            ("snapshotId", input.snapshot_id.as_deref()),
            ("imageId", input.image_id.as_deref()),
            ("volumeId", input.volume_id.as_deref()),
        ];
        let named: Vec<&str> = sources
            .iter()
            .filter(|(_, value)| value.is_some())
            .map(|(member, _)| *member)
            .collect();
        if named.len() > 1 {
            return Err(Scaffolded::invalid_parameter_combination(format!(
                "A replacement root volume is restored from exactly one source, so {} may \
                 not be given together",
                named.join(" and ")
            )));
        }

        // "Valid range: 100 - 300 MiB/s"
        // (`CreateReplaceRootVolumeTaskRequest$VolumeInitializationRate`). The model
        // declares no `@range` for the member, so this handler is where the published bound
        // is enforced.
        if let Some(rate) = input.volume_initialization_rate {
            if !(MIN_VOLUME_INITIALIZATION_RATE..=MAX_VOLUME_INITIALIZATION_RATE).contains(&rate) {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "Invalid value '{rate}' for volumeInitializationRate. The volume \
                     initialization rate is between {MIN_VOLUME_INITIALIZATION_RATE} and \
                     {MAX_VOLUME_INITIALIZATION_RATE} MiB/s"
                )));
            }
        }

        if input
            .client_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > MAX_CLIENT_TOKEN_CHARS)
        {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"
            )));
        }

        // "The tags to apply to the root volume replacement task"
        // (`TagSpecifications`): the task is the only resource this call creates, so the ONE
        // reader of a create's tag specifications is what reads them — a specification
        // naming another resource type or none at all is refused, a duplicate or reserved
        // key is refused, and the per-resource tag count and each key and value length are
        // bounded there rather than a second way here.
        let tags = tags_from_specifications(
            input.tag_specifications.as_deref(),
            REPLACE_ROOT_VOLUME_TASK_TAGGABLE_RESOURCE,
            Self::NAME,
        )?;

        let state = require_service_state(&*self.engine)?;
        let mut store = state.lock();
        // This call writes, so the moves the clock already owed the instances are applied
        // before their states are read: the refusal below is about the state the instance
        // is in NOW, not the one it was left in by an earlier call.
        store.materialize_due_transitions();
        let now = ::std::time::Instant::now();

        // The instance this operation's own validator resolves — see
        // [`require_replaceable_instance`] for why an id it cannot resolve is answered
        // malformed rather than not-found. ONE lookup: the validator hands back the record
        // it resolved.
        let instance = require_replaceable_instance(&store, &input.instance_id)?;
        // "Replaces the EBS-backed root volume for a `running` instance"
        // (`CreateReplaceRootVolumeTask`): the replacement is attached to the instance in
        // place, which is work only a running instance is given — an instance in any other
        // state (one still coming up, one stopping or stopped, one on its way to
        // `terminated`) has no running root volume for a replacement to take over from, so
        // the task is refused rather than started against a state that could not carry it.
        if instance.state != crate::state::InstanceStateName::Running {
            return Err(Scaffolded::service_error(
                "IncorrectInstanceState",
                400,
                // The sentence a recorded refusal of THIS operation words that code with: it
                // names the state the operation needs and nothing of the request, which is
                // why it is built here rather than through the shared constructor
                // ([`crate::scaffolded::Scaffolded::incorrect_instance_state`], whose
                // sentence other operations are recorded answering).
                "Instance is not in the running state.",
            ));
        }
        let instance_zone = instance.placement_az.clone();
        // The AMI the replacement root volume is restored from: the one the request named,
        // or — for a restore "to the original root volume's launch state" — the AMI the
        // instance itself was launched from, which is where that launch state comes from.
        let restore_image = input.image_id.clone().or_else(|| instance.image_id.clone());

        // The restore source, resolved against what the account actually holds, so a task
        // is never stored naming a resource that does not exist.
        if let Some(image_id) = input.image_id.as_deref() {
            require_visible_image(&store, image_id)?;
        }
        if let Some(snapshot_id) = input.snapshot_id.as_deref() {
            // This emulator stores no EBS snapshots at all (`CreateSnapshot` is still a
            // scaffold), so no snapshot id can resolve: the request named a snapshot that
            // does not exist, which is what it is told.
            return Err(unresolvable_snapshot_id(snapshot_id));
        }
        if let Some(volume_id) = input.volume_id.as_deref() {
            // ONE lookup: the reader hands back the record it resolved
            // ([`crate::state::require_volume`]'s own contract), so there is no second
            // lookup and no unreachable branch for an id that resolved and then could not
            // be read.
            let volume = require_volume(&store, volume_id)?;
            // "The specified volume must be in the same Availability Zone as the instance,
            // must be in the `available` state, and must not be attached to an instance"
            // (`VolumeId`) — the `available` state IS "attached to nothing", so one check
            // answers the last two.
            let volume_zone = volume.view.availability_zone.clone().unwrap_or_default();
            if volume_zone != instance_zone {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "Invalid value '{volume_id}' for volumeId. The volume is in \
                     Availability Zone '{volume_zone}' and the instance is in \
                     '{instance_zone}'"
                )));
            }
            let volume_state = volume.state_at(now);
            if volume_state != VOLUME_STATE_AVAILABLE {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "Invalid value '{volume_id}' for volumeId. The volume is in state \
                     '{volume_state}', not '{VOLUME_STATE_AVAILABLE}'"
                )));
            }
        }

        // Idempotency: "Unique, case-sensitive identifier you provide to ensure the
        // idempotency of the request" (`ClientToken`) — one token names the one task it
        // started, so a replay reports that task and starts nothing. Answered below the
        // ceilings and the DryRun check, because a replay stores nothing and so passes the
        // ceilings a fresh create has to clear.
        let replayed = input
            .client_token
            .as_deref()
            .and_then(|token| store.replace_root_volume_task_client_tokens.get(token).cloned());

        let started_at = ::std::time::SystemTime::now();
        // "The ID of the snapshot used to create the replacement root volume"
        // (`ReplaceRootVolumeTask$SnapshotId`): a replacement volume RESTORED FROM AN AMI —
        // the AMI the request named, or, for a restore to the original root volume's launch
        // state, the AMI the instance was launched from — is created from that image's root
        // device snapshot, read back off the stored image
        // ([`crate::state::ComAmazonawsEc2Amazonec2State::image_root_snapshot_id`]). The two
        // forms that name no snapshot are a replacement BY a specified volume (nothing is
        // created from a snapshot) and a restore whose AMI this account holds no image for
        // (this emulator's state then holds no snapshot the task could have used). The third
        // form, a restore FROM a snapshot the request named, never reaches here: it is
        // refused above, because this emulator stores no EBS snapshots for an id to resolve
        // against.
        let snapshot_id = if input.volume_id.is_some() {
            None
        } else {
            restore_image
                .as_deref()
                .and_then(|image_id| store.image_root_snapshot_id(image_id))
        };
        let view = ReplaceRootVolumeTask {
            // Minted by the store at the insert, so an id is never chosen twice.
            replace_root_volume_task_id: None,
            instance_id: Some(input.instance_id.clone()),
            // Derived by the record from the clock, not stored on the view.
            task_state: None,
            start_time: Some(iso8601_utc(started_at)),
            complete_time: None,
            tags: Some(tags),
            // "The ID of the AMI used to create the replacement root volume" and "the ID
            // of the snapshot used to create the replacement root volume": the AMI is the
            // one the request named, having been resolved above, and the snapshot is the
            // one the replacement volume was created from.
            image_id: input.image_id.clone(),
            snapshot_id,
            // "Indicates whether the original root volume is to be deleted after the root
            // volume replacement task completes" — a request that asked for nothing keeps
            // the original volume, which the member reports as `false`.
            delete_replaced_root_volume: Some(
                input.delete_replaced_root_volume.unwrap_or(false),
            ),
        };
        let payload_bytes =
            replace_root_volume_task_payload_bytes(&view, input.client_token.as_deref());

        if replayed.is_none() && !store.has_replace_root_volume_task_capacity() {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "This emulator holds {MAX_STORED_RECORDS_PER_STORE} root volume replacement \
                 tasks, which is as many as it stores"
            )));
        }
        if replayed.is_none() && store.would_exceed_stored_bytes(payload_bytes) {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                 caller-supplied data, so it cannot store {payload_bytes} more",
                store.stored_bytes()
            )));
        }

        // DryRun sits AFTER every parameter has been validated and BEFORE the first write:
        // the flag asks whether the request WOULD have worked, so one that would have been
        // refused is refused with that reason instead, and one that would have succeeded
        // leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        let task_id = match replayed {
            Some(task_id) => task_id,
            None => store.insert_replace_root_volume_task(
                ReplaceRootVolumeTaskRecord::starting_now(view, started_at),
                input.client_token.as_deref(),
            ),
        };

        // Read the response back OUT of the store, so the caller sees what a later
        // DescribeReplaceRootVolumeTasks will see rather than the request object.
        store
            .replace_root_volume_tasks
            .get(&task_id)
            .map(|record| CreateReplaceRootVolumeTaskOutput {
                replace_root_volume_task: Some(record.view_at(::std::time::Instant::now())),
            })
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the newly created root volume replacement task '{task_id}' could not be \
                     read back"
                ))
            })
    }
}

/// Resolve the instance `CreateReplaceRootVolumeTask` is asked to replace the root volume
/// of, or refuse the id: HTTP 400 `InvalidInstanceID.Malformed`.
///
/// EC2's per-operation validators do NOT agree about an instance id they cannot resolve,
/// and this operation's is the strict one: it checks the id against the instance id space
/// and answers `InvalidInstanceID.Malformed` for anything it does not recognise as an id
/// the service issued — including a value that reads like the long `i-xxxxxxxxxxxxxxxxx`
/// form but names no instance. The recorded unit
/// `ec2-instance-dependent-ops-error-paths#task#4` probes a 17-hexadecimal-digit id of
/// exactly that shape and `CreateReplaceRootVolumeTask` answers
/// `InvalidInstanceID.Malformed`, "The instance ID '<id>' is malformed", while
/// `CreateInstanceExportTask` answers the SAME id "does not exist" (recorded in
/// `issues/.../20261023T102725Z-ec2-answers-parameter-rejections-with-generic-validationerror-and.md`).
/// So the choice belongs to the OPERATION rather than to the id, which is why this lives
/// here and not in [`crate::state::require_known_instance`] — the shared helper that other
/// operations, which do draw the malformed/not-found distinction, keep using.
///
/// The refusal itself is [`crate::scaffolded::Scaffolded::instance_id_is_malformed`], a
/// named constructor beside the syntax-only one, so both wordings of this code live in
/// `crate::scaffolded` rather than one of them in a handler. The message is built from the
/// request, naming the member and the value the caller gave.
///
/// The resolved record is handed back, so the handler reads the instance ONCE rather than
/// looking it up again behind an unreachable branch.
fn require_replaceable_instance<'a, E>(
    store: &'a crate::state::ComAmazonawsEc2Amazonec2State,
    instance_id: &str,
) -> Result<&'a crate::state::InstanceRecord, crate::scaffolded::Scaffolded<E>> {
    store
        .instances
        .get(instance_id)
        .ok_or_else(|| crate::scaffolded::Scaffolded::instance_id_is_malformed(instance_id))
}
