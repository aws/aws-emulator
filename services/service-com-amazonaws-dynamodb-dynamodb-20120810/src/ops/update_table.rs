// SPDX-License-Identifier: CC0-1.0

//! @op UpdateTable
//! IMPLEMENTED: moves the stored table's provisioned throughput, its billing mode, its
//! warm throughput, its global secondary indexes (create, delete and throughput
//! update), its table class and its deletion protection, and reports the table back as
//! a `TableDescription` read from the stored table.
//!
//! The part of an update whose work OUTLIVES the call is asynchronous — a request that
//! moves capacity (the table's capacity mode, its provisioned throughput, its warm
//! throughput, its table class or one of its secondary indexes) and one that starts or
//! stops the table's change feed (a `StreamSpecification`). Such a request leaves the
//! table `UPDATING` ("while it's executing, the table status changes from ACTIVE to
//! UPDATING" — the doc below) and the clock owes it the move back to `ACTIVE`
//! [`crate::state::STATUS_TRANSITION`] later; a second `UpdateTable` inside that span is
//! refused with `ResourceInUseException`, while item operations keep being served. A
//! request that only flips a flag the service applies with the call — deletion
//! protection — leaves nothing for the clock to finish and the table is reported
//! `ACTIVE`, which is what the recorded service answers and what lets consecutive
//! control-plane calls through. Either way the NEW configuration is in effect from the
//! moment the call returns, which is what `DescribeTable` and the capacity accounting
//! report.
//!
//! The `TableDescription` this call ANSWERS with is a description of the table, so every
//! capacity figure it carries is the one the table is serving at the instant the call
//! returns: an asynchronous move has not happened yet, so the response reports the
//! `ProvisionedThroughput` and the warm pair still in force — the table's own and each
//! index's — and a `NumberOfDecreasesToday` that does not yet count the decrease just
//! accepted ([`crate::state::CapacityInForce`]). The pair's `Status` beside those figures
//! is the move's own `UPDATING`, and a later read is what reports the figures the move
//! left. An update that moves the table's capacity MODE is the exception: the mode is in
//! force from the moment the call returns, and the provisioned pair belongs to the mode, so
//! the pair that description carries is the NEW mode's own — none for a table moved onto
//! on-demand capacity, and the pair the update set for one moved onto provisioned capacity —
//! while the warm pair beside it is still the one in force.
//!
//! A request that names NO updatable member at all is refused as a `ValidationException`
//! before any of that: it is permanently invalid rather than a transient conflict. So is
//! an on-demand capacity ceiling named for a table the update leaves in the PROVISIONED
//! capacity mode — the two capacity models have no setting in common, so that
//! contradiction is refused ahead of the scaffold refusal below, and the stored table is
//! left as it was. Every capacity figure the request states is held to what the service can
//! hold ([`crate::state::check_capacity_bounds`]) before the table is even looked up, so a
//! figure past it leaves the table settled at the capacity it had rather than `UPDATING`
//! towards one AWS cannot report.
//!
//! A `StreamSpecification` enables or disables the table's DynamoDB Streams stream,
//! which lives in the streams service's own store: enabling registers a stream and
//! the table starts reporting its `LatestStreamArn`/`LatestStreamLabel`, disabling
//! closes it. Both contradictions the model names ("you try to enable a stream on a
//! table that already has a stream, or ... disable a stream on a table that doesn't
//! have a stream") are `ResourceInUseException`.
//!
//! A `GlobalSecondaryIndexUpdates` entry addresses one of the table's GLOBAL secondary
//! indexes. A `Create` adds the index it describes, `CREATING` until its build is done
//! ([`crate::state::IndexBackfill`]); a `Delete` puts the index on its way out — the
//! description the call answers still carries it, reported `DELETING`
//! ([`crate::state::GlobalSecondaryIndex::begin_deletion`]), and the index disappears when
//! the clock makes the removal due, taking with it every attribute definition no remaining
//! key schema uses. A `Delete` is served even for an index that is still being BUILT:
//! cancelling a build is how a caller undoes an index it mis-specified, so that entry stops
//! the build rather than asking for a second one, while any OTHER index change asked of a
//! table that is still building one is past the one-online-index limit
//! ([`index_build_refusal`]). An `Update` moves one index's provisioned throughput.
//!
//! A `VectorIndexUpdates` entry addresses one of the table's VECTOR indexes as the
//! resource it changes. A `Delete` removes the index it names, so a name the table
//! carries no vector index for is refused with `ResourceNotFoundException` — the modelled
//! answer for "the operation tried to access a nonexistent table or index" — before
//! anything is written, which leaves every stored index where it was; a name it does
//! carry is removed, its siblings keep serving `SearchVectors`. A `Create` adds the index
//! it describes, held to the same rules as one declared with the table — the capacity mode
//! the update leaves the table in included
//! ([`crate::state::check_vector_index_capacity_mode`]) — and it becomes
//! searchable when it reaches `ACTIVE`. An index ADDED this way has the items the table
//! already holds to read, so its description reports `Backfilling` for that span
//! ([`crate::state::IndexBackfill`]) — true while those items are being indexed, false
//! once they have been, which is also what an index added to an empty table reports from
//! the start. A `Delete` of an index that is still being BUILT is held to the phase the
//! build is in: while its resources are being allocated there is nothing allocated to take
//! away, so the delete is refused `ResourceInUseException` naming the phase to retry in and
//! the index keeps building ([`index_build_refusal`]); from the backfill on it is served,
//! cancelling the build. Either way the change is index lifecycle work, so
//! the table is left `UPDATING` for the span below like any other — and a call asking for
//! more than one create or one delete of a vector index exceeds what one `UpdateTable`
//! carries, which is the control plane's `LimitExceededException`.
//!
//! A `ReplicaUpdates` entry addresses one REGION of the table's replication group. A
//! `Create` makes the table a global table: the replica is a table of its own in the
//! Region it names, holding this table's data and reported by every regional table of the
//! group until a `Delete` removes it again; a `Disassociate` keeps the local replica as a
//! standalone regional table. Each override the action carries is applied to that regional
//! table — its read capacity, its on-demand read ceiling, its table class, the KMS key it
//! is encrypted with (resolved by KMS, like the table's own `SSESpecification`) and the
//! per-index settings of its `GlobalSecondaryIndexes`. An action that names the Region the
//! table itself is in, a Region the group does not hold, or an index the group's table does
//! not carry is refused with the 400 `ValidationException` it is
//! ([`crate::state::validated_replica_updates`]).
//!
//! An `SSESpecification` moves the table's server-side encryption: the key it names is
//! resolved by KMS, and the table reports the resolved key ARN under `SSEDescription`
//! ([`crate::state::requested_encryption`]). An `OnDemandThroughput` sets and clears the
//! table's on-demand capacity ceilings, each through the same member
//! ([`crate::state::OnDemandCeilings`]).
//!
//! `MultiRegionConsistency` and `GlobalTableWitnessUpdates` configure the replication GROUP
//! the request's `Create` actions establish: the mode and the witness Regions are recorded
//! on every regional table of the group, which is what each one's description then reports
//! under `MultiRegionConsistency` and `GlobalTableWitnesses`. A configuration multi-Region
//! strong consistency is not offered for — a group that would not span three Regions, a
//! witness in a replica's Region, or a table carrying a local secondary index — is refused
//! with the 400 `ValidationException` it is, before anything is provisioned
//! ([`crate::state::unsupported_consistency_configuration`]).
//!
//! Members this slice does not store (a PER-INDEX on-demand or warm throughput, the
//! settings replication mode and disassociation protection) are refused with the scaffold's
//! 501 and are their own gap issues.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateTable::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `UpdateTable`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! Modifies the provisioned throughput settings, global secondary indexes, DynamoDB Streams
//! settings, or global table replica settings for a given table. You can only perform one
//! of the following operations at once: Modify the provisioned throughput settings of the
//! table. Remove a global secondary index from the table. Create a new global secondary
//! index on the table. After the index begins backfilling, you can use UpdateTable to
//! perform other operations. Enable or disable disassociation protection for the local
//! global table replica. Disassociate the local global table replica and keep it as a
//! standalone regional table. The Disassociate action must be the only replica update in
//! the request. If the request includes any other update, UpdateTable fails with a
//! ValidationException and the disassociation does not start. UpdateTable is an
//! asynchronous operation; while it's executing, the table status changes from ACTIVE to
//! UPDATING. While it's UPDATING, you can't issue another UpdateTable request. Use
//! DescribeTable to monitor the table's status. To use this operation to disassociate a
//! local replica, you must have the dynamodb:UpdateTable and
//! dynamodb:DisassociateTableReplica IAM permissions on the local replica. Send
//! disassociation and disassociation protection requests to the replica's Region using
//! credentials for the account that owns the replica. For prerequisites and the procedure,
//! see Disassociating a global table replica in the Amazon DynamoDB Developer Guide. To
//! confirm successful disassociation, call DescribeTable in the replica's Region and verify
//! that TableStatus is ACTIVE and Replicas is absent or empty. If TableStatus returns to
//! ACTIVE but the response still includes a Replicas list, the disassociation did not
//! complete. Check ReplicaStatus for a replication-permission or encryption-access problem,
//! resolve it, and retry.
//!
//! ## Input
//!
//! * `AttributeDefinitions` (AttributeDefinitions) — An array of attributes that describe the
//!   key schema for the table and indexes. If you are adding a new global secondary index to
//!   the table, AttributeDefinitions must include the key element(s) of the new index.
//! * `BillingMode` (BillingMode, one of: PAY_PER_REQUEST | PROVISIONED) — Controls how you
//!   are charged for read and write throughput and how you manage capacity. When switching
//!   from pay-per-request to provisioned capacity, initial provisioned capacity values must
//!   be set. The initial provisioned capacity values are estimated based on the consumed read
//!   and write capacity of your table and global secondary indexes over the past 30 minutes.
//!   PAY_PER_REQUEST - We recommend using PAY_PER_REQUEST for most DynamoDB workloads.
//!   PAY_PER_REQUEST sets the billing mode to On-demand capacity mode. PROVISIONED - We
//!   recommend using PROVISIONED for steady workloads with predictable growth where capacity
//!   requirements can be reliably forecasted. PROVISIONED sets the billing mode to …
//! * `DeletionProtectionEnabled` (DeletionProtectionEnabled) — Indicates whether deletion
//!   protection is to be enabled (true) or disabled (false) on the table.
//! * `DisassociationProtectionEnabled` (DisassociationProtectionEnabled) — Specifies whether
//!   disassociation protection is enabled (true) or disabled (false) for the local replica of
//!   a multi-Region eventual consistency (MREC) global table using Version 2019.11.21.
//!   Disassociation protection is enabled by default. If you omit this parameter, the
//!   existing setting is unchanged. This setting is independent of DeletionProtectionEnabled.
//!   You can't combine this change with any other change in the same UpdateTable request.
//! * `GlobalSecondaryIndexUpdates` (GlobalSecondaryIndexUpdateList) — An array of one or more
//!   global secondary indexes for the table. For each index in the array, you can request one
//!   action: Create - add a new global secondary index to the table. Update - modify the
//!   provisioned throughput settings of an existing global secondary index. Delete - remove a
//!   global secondary index from the table. You can create or delete only one global
//!   secondary index per UpdateTable operation. For more information, see Managing Global
//!   Secondary Indexes in the Amazon DynamoDB Developer Guide.
//! * `GlobalTableSettingsReplicationMode` (GlobalTableSettingsReplicationMode, one of:
//!   DISABLED | ENABLED | ENABLED_WITH_OVERRIDES) — Controls the settings replication mode
//!   for a global table replica. This attribute can be defined using UpdateTable operation
//!   only on a regional table with values: ENABLED: Defines settings replication on a
//!   regional table to be used as a source table for creating Multi-Account Global Table.
//!   DISABLED: Remove settings replication on a regional table. Settings replication needs to
//!   be defined to ENABLED again in order to create a Multi-Account Global Table using this
//!   table.
//! * `GlobalTableWitnessUpdates` (GlobalTableWitnessGroupUpdateList, len 1..1) — A list of
//!   witness updates for a MRSC global table. A witness provides a cost-effective alternative
//!   to a full replica in a MRSC global table by maintaining replicated change data written
//!   to global table replicas. You cannot perform read or write operations on a witness. For
//!   each witness, you can request one action: Create - add a new witness to the global
//!   table. Delete - remove a witness from the global table. You can create or delete only
//!   one witness per UpdateTable operation. For more information, see Multi-Region strong
//!   consistency (MRSC) in the Amazon DynamoDB Developer Guide
//! * `MultiRegionConsistency` (MultiRegionConsistency, one of: EVENTUAL | STRONG) — Specifies
//!   the consistency mode for a new global table. This parameter is only valid when you
//!   create a global table by specifying one or more Create actions in the ReplicaUpdates
//!   action list. You can specify one of the following consistency modes: EVENTUAL:
//!   Configures a new global table for multi-Region eventual consistency (MREC). This is the
//!   default consistency mode for global tables. STRONG: Configures a new global table for
//!   multi-Region strong consistency (MRSC). If you don't specify this field, the global
//!   table consistency mode defaults to EVENTUAL. For more information about global tables
//!   consistency modes, see Consistency modes in DynamoDB developer guide.
//! * `OnDemandThroughput` (OnDemandThroughput) — Updates the maximum number of read and write
//!   units for the specified table in on-demand capacity mode. If you use this parameter, you
//!   must specify MaxReadRequestUnits, MaxWriteRequestUnits, or both.
//! * `ProvisionedThroughput` (ProvisionedThroughput) — The new provisioned throughput
//!   settings for the specified table or index.
//! * `ReplicaUpdates` (ReplicationGroupUpdateList, len 1..) — A list of replica update
//!   actions (create, delete, update, or disassociate) for the table.
//! * `SSESpecification` (SSESpecification) — The new server-side encryption settings for the
//!   specified table.
//! * `StreamSpecification` (StreamSpecification) — Represents the DynamoDB Streams
//!   configuration for the table. You receive a ValidationException if you try to enable a
//!   stream on a table that already has a stream, or if you try to disable a stream on a
//!   table that doesn't have a stream.
//! * `TableClass` (TableClass, one of: STANDARD | STANDARD_INFREQUENT_ACCESS) — The table
//!   class of the table to be updated. Valid values are STANDARD and
//!   STANDARD_INFREQUENT_ACCESS.
//! * `TableName` (TableArn, len 1..1024) **required** — The name of the table to be updated.
//!   You can also provide the Amazon Resource Name (ARN) of the table in this parameter.
//! * `VectorIndexUpdates` (VectorIndexUpdateList) — A list of vector indexes to be added to
//!   or removed from the table. You can add or remove one vector index for each UpdateTable
//!   operation. To add a vector index, specify IndexName, VectorAttribute, Dimensions,
//!   DistanceFunction, and Projection. To remove a vector index, specify only the IndexName.
//! * `WarmThroughput` (WarmThroughput) — Represents the warm throughput (in read units per
//!   second and write units per second) for updating a table.
//!
//! ## Output
//!
//! * `TableDescription` (TableDescription) — Represents the properties of the table.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidEndpointException` (HTTP 421)
//! * `LimitExceededException` (HTTP 400) — There is no limit to the number of daily on-demand
//!   backups that can be taken. For most purposes, up to 500 simultaneous table operations
//!   are allowed per account. These operations include CreateTable, UpdateTable,
//!   DeleteTable,UpdateTimeToLive, RestoreTableFromBackup, and RestoreTableToPointInTime.
//!   When you are creating a table with one or more secondary indexes, you can have up to 250
//!   such requests running at a time. However, if the table or index specifications are
//!   complex, then DynamoDB might temporarily reduce the number of concurrent operations.
//!   When importing into DynamoDB, up to 50 simultaneous import table operations are allowed
//!   per account. There is a soft account quota of 2,500 …
//! * `ResourceInUseException` (HTTP 400) — The operation conflicts with the resource's
//!   availability. For example: You attempted to recreate an existing table. You tried to
//!   delete a table currently in the CREATING state. You tried to update a resource that was
//!   already being updated. When appropriate, wait for the ongoing update to complete and
//!   attempt the request again.
//! * `ResourceNotFoundException` (HTTP 400) — The operation tried to access a nonexistent
//!   table or index. The resource might not be specified correctly, or its status might not
//!   be ACTIVE.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateTableError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateTableError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#UpdateTable")]
#[derive(Debug)]
pub enum UpdateTableErrorModeled {
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_table::UpdateTableError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateTableError = crate::scaffolded::Scaffolded<UpdateTableErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateTableErrorModeled> for UpdateTableError {
    fn from(modeled: UpdateTableErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateTable` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateTable<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateTable<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateTable<Eng>
{
    type Input = UpdateTableInput;
    type Output = UpdateTableOutput;
    type Error = UpdateTableError;

    const NAME: &'static str = "UpdateTable";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_TABLE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_TABLE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        let region = crate::state::request_region(ctx.parts());
        let name = crate::state::resolve_table_name_in(&input.table_name, &region).map_err(lift)?;

        // A request that asks for no change is permanently invalid, whatever the
        // table's status, so it is refused before the table is even looked up: a
        // conflict answer would tell a client to retry a request that can never
        // succeed.
        if !names_an_update(&input) {
            return Err(crate::scaffolded::Scaffolded::validation(
                EMPTY_UPDATE_MESSAGE.to_string(),
            ));
        }
        let index_updates = validated_index_updates(&input).map_err(lift)?;
        // Every capacity figure the request states, held to what the service can hold — the
        // same check the create applies, for the same reason
        // ([`crate::state::check_capacity_bounds`]). It runs before the table is looked up,
        // so a figure the service cannot hold leaves the table untouched whatever state it
        // is in, and the index list it walks is the clamped one above.
        crate::state::check_capacity_bounds(requested_capacities(&input)).map_err(lift)?;
        let vector_index_updates = validated_vector_index_updates(&input).map_err(lift)?;
        // "You can create or delete only one global secondary index per UpdateTable
        // operation" (`UpdateTable$GlobalSecondaryIndexUpdates`). That is a LIMIT on
        // the work one call may ask for rather than a malformed request, so it is
        // answered with the control plane's modelled limit error — the code a client
        // backs off and retries one index at a time on.
        // A CREATE and a DELETE in one call are two lifecycle actions, so they count
        // together against that one.
        if let Some(message) = index_lifecycle_actions_over_limit(&index_updates) {
            return Err(UpdateTableErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(message),
                },
            )
            .into());
        }
        // The same limit over the table's VECTOR indexes: a call that asks for more
        // vector-index lifecycle work than one `UpdateTable` carries is answered the
        // control plane's limit error, counted per action the way the secondary-index
        // limit above is.
        if let Some(message) = vector_index_actions_over_limit(&vector_index_updates) {
            return Err(UpdateTableErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(message),
                },
            )
            .into());
        }
        // What the request's `StreamSpecification` asks for, resolved before the
        // store lock is taken: a specification that cannot hold together refuses the
        // whole update.
        let stream_request = requested_stream_change(&input).map_err(lift)?;
        // What the request's `ReplicaUpdates` ask for, read before the store lock is taken
        // and held to every rule that is about the REQUEST: one action per entry, a Region
        // named once, and an action that addresses the local table's own Region rather
        // than a replica of it ([`crate::state::validated_replica_updates`]).
        let mut replica_actions = crate::state::validated_replica_updates(
            input.replica_updates.as_ref(),
            &name,
            &region,
        )
        .map_err(lift)?;
        // Which KMS key each replica action named, resolved by KMS in the replica's own
        // Region before anything is written: a key identifier KMS does not resolve refuses
        // the whole update, the way the same identifier on an `SSESpecification` does.
        crate::state::resolve_replica_keys(&*self.engine, &mut replica_actions)
            .map_err(sse_unresolved)?;
        // What the request's `GlobalTableWitnessUpdates` ask for, read here for the same
        // reason: the entry count and the form of each entry are rules about the REQUEST, so
        // they are decided before the store lock is taken and no loop whose length the
        // request chooses runs inside the critical section
        // ([`crate::state::validated_witness_updates`]).
        let witness_actions =
            crate::state::validated_witness_updates(input.global_table_witness_updates.as_ref())
                .map_err(lift)?;
        // Each on-demand ceiling the request names is held to the values that member takes:
        // 1 or more SETS a maximum, -1 removes one.
        crate::state::check_on_demand_ceiling_values(input.on_demand_throughput.as_ref())
            .map_err(lift)?;
        // Every other caller-sized list is clamped here too, BEFORE the store lock is
        // taken, so no loop whose length the request chooses runs inside the critical
        // section.
        let declared_definitions = clamped_definitions(&input).map_err(lift)?;
        // An index this call CREATES must have the attributes of its key schema declared in
        // the request's OWN `AttributeDefinitions` — "AttributeDefinitions must include the
        // key element(s) of the new index" (`UpdateTable$AttributeDefinitions`). Nothing the
        // store holds decides that, so it is answered here, ahead of resolving the table the
        // request names: the request is invalid as sent whatever the store holds, and a
        // client handed the not-found class for it would retry a call that can never succeed
        // ([`check_requested_index_definitions`]).
        check_requested_index_definitions(&index_updates, &declared_definitions).map_err(lift)?;

        // WHO this request speaks for, resolved before this service's store is locked: a
        // control-plane call on a table is authorized exactly as a read or a write of its
        // items is ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);
        // The table has to exist and be settled before anything is changed, so a
        // refused update leaves the stored table exactly as it was.
        let stored = store.table(&region, &name).map_err(lift)?;
        // A caller the policies in force do not let change this table is refused before
        // anything is written, so a refused update changes nothing.
        if let Some(denial) = crate::state::authorization::control_denial(
            &caller,
            stored,
            "dynamodb:UpdateTable",
            now,
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }
        let stored_billing_mode = stored.billing_mode.clone();
        let stored_encryption = stored.encryption.clone();
        // The capacity the table is SERVING at the instant this call arrives, kept for the
        // description the call answers with: a capacity move is asynchronous, so what that
        // description reports is the capacity in force rather than the one the request
        // asks for ([`crate::state::CapacityInForce`]).
        let capacity_in_force = crate::state::CapacityInForce::of(&stored.describe(now));
        match stored.status(now) {
            crate::state::TableStatus::Active => {}
            crate::state::TableStatus::Creating => {
                return Err(UpdateTableErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        message: Some(format!(
                            "Attempt to change a resource which is still in use: Table is being \
                             created: {name}"
                        )),
                    },
                )
                .into())
            }
            crate::state::TableStatus::Updating => {}
            crate::state::TableStatus::Deleting => {
                return Err(UpdateTableErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "Requested resource not found: Table: {name} not found"
                        )),
                    },
                )
                .into())
            }
        }
        // What this request asks of an index that is still being BUILT, and of a table
        // whose own update has not settled: "While it's UPDATING, you can't issue another
        // UpdateTable request" (the doc above), and an index change asked for while one
        // index is being built is past the limit on how many may be built at once
        // ([`index_build_refusal`] decides which of the three answers each request earns).
        if let Some(refusal) = index_build_refusal(
            stored,
            &name,
            stored.status(now) == crate::state::TableStatus::Updating,
            &index_updates,
            &vector_index_updates,
            now,
        ) {
            return Err(refusal);
        }

        // A table-class CHANGE is held to the quota on how many of them one table may be
        // given in the trailing window ([`crate::state::TableClassChanges`]). Past it the
        // answer is the control plane's modelled limit error — the code a client waits the
        // window out on — and nothing is written, so the table keeps the class it has. A
        // request that re-states the class the table is already in changes nothing and is
        // not held to the quota at all.
        if let Some(requested) = input.table_class.as_deref() {
            if stored.table_class.as_deref() != Some(requested)
                && stored.table_class_changes.exhausted(now)
            {
                return Err(UpdateTableErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(crate::state::table_class_change_limit_message()),
                    },
                )
                .into());
            }
        }
        // What `MultiRegionConsistency` and `GlobalTableWitnessUpdates` ask for, held to
        // the configurations multi-Region strong consistency is offered for: a combination
        // AWS does not serve is a bad REQUEST and is refused as one, with AWS's own sentence
        // for that refusal ([`crate::state::unsupported_consistency_configuration`]).
        if let Some(refusal) = crate::state::unsupported_consistency_configuration(
            input.multi_region_consistency.as_deref(),
            &witness_actions,
            &replica_actions,
            stored,
        ) {
            return Err(lift(refusal));
        }

        // What the update resolves to is computed against the STORED table, so a
        // request that only names an index update leaves the table's own mode and
        // throughput alone.
        let requested_mode = crate::state::requested_throughput_mode(
            input.billing_mode.as_deref(),
            input.table_throughput_mode.as_deref(),
        );
        // Whether this update moves the table's capacity MODE, which decides which
        // generation of the capacity figures the description below carries
        // ([`crate::state::CapacityInForce`]).
        let capacity_mode_moved = requested_mode.is_some_and(|mode| mode != stored_billing_mode);
        let billing_mode = requested_mode.map_or(stored_billing_mode, str::to_string);
        // Each mode member was held to the enum's value set before this handler ran, and a
        // stored mode is one this service wrote, so what the update resolves to is always
        // one of the two modes.
        // An on-demand capacity ceiling cannot hold on a table the update leaves in the
        // PROVISIONED capacity mode, whichever resource it was named for
        // ([`crate::state::check_on_demand_ceilings`]). That contradiction is in the
        // REQUEST, so it is refused here — ahead of the scaffold's 501 for the members
        // this slice does not store, and before anything is written, so the stored
        // throughput is left exactly as it was.
        crate::state::check_on_demand_ceilings(&billing_mode, requested_on_demand_ceilings(&input))
            .map_err(lift)?;
        if let Some(member) = unsupported_member(&input) {
            return Err(crate::scaffolded::Scaffolded::not_implemented(&format!(
                "UpdateTable with {member}"
            )));
        }
        // What the replica actions ask of the STORED replication group: a Region the group
        // does not hold cannot be updated or deleted, and one it already holds cannot be
        // created again. Checked here, before anything is written, so a refused update
        // leaves every regional table of the group exactly as it was.
        store
            .check_replica_actions(&region, &name, &replica_actions, now)
            .map_err(lift)?;
        // What those actions would COST: a created replica is a table of its own in
        // another Region holding a copy of the group's data, so it is counted against the
        // same quotas a create is. Past one of them the modelled limit error is the
        // answer — the code a client backs off and retries fewer replicas on.
        if let Some(message) = store.replica_creates_over_limit(&region, &name, &replica_actions) {
            return Err(UpdateTableErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(message),
                },
            )
            .into());
        }
        // A `TableClassOverride` moves the class of the replica's OWN regional table, so it
        // is held to the same table-class change quota a class change made against this
        // table is ([`crate::state::TableClassChanges`]) — the one quota, asked of whichever
        // table the change lands on.
        if let Some(message) =
            store.replica_class_changes_over_limit(&name, &replica_actions, now)
        {
            return Err(UpdateTableErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(message),
                },
            )
            .into());
        }
        // The encryption this update leaves in force, with the key ARN KMS resolved for
        // the identifier the request named ([`crate::state::requested_encryption`]). A
        // change to a table that already exists is reported `UPDATING` for the span the
        // rest of the update takes, so the instant it becomes effective is that span's.
        let encryption = crate::state::requested_encryption(
            &*self.engine,
            &region,
            input.sse_specification.as_ref(),
            stored_encryption.as_ref(),
            now + crate::state::STATUS_TRANSITION,
        )
        .map_err(sse_unresolved)?;
        let table = store.table_mut(&region, &name).map_err(lift)?;
        let table_throughput = resolve_table_throughput(table, &billing_mode, &input)
            .map_err(lift)?;
        // A warm-throughput change is reported `UPDATING` for the same span as the rest
        // of the update, and becomes `ACTIVE` when the clock makes that span due; a
        // request that names none leaves the pair in force exactly as it is.
        //
        // The pair an unnamed setting keeps warm follows the capacity this update SETS,
        // and survives a capacity this update WITHDRAWS (a switch to on-demand): what the
        // table was keeping warm stays warm, which is what the recorded description of a
        // table switched to `PAY_PER_REQUEST` reports.
        let warm_throughput = crate::state::resolved_warm_throughput(
            input.warm_throughput.as_ref(),
            &table
                .warm_throughput
                .tracking_capacity(table_throughput.as_ref()),
            now + crate::state::STATUS_TRANSITION,
        )
        .map_err(lift)?;
        let declared = merged_definitions(table, &declared_definitions);
        // The table's vector indexes after the update, resolved against the STORED ones:
        // a `Delete` naming an index the table does not carry, and a `Create` the table
        // cannot take, refuse the whole update here, before anything is written. The
        // definitions it is given are the MERGED ones, so a create may name a search-schema
        // attribute this same request declares.
        let vector_indexes = resolve_vector_indexes(
            table,
            &billing_mode,
            &vector_index_updates,
            &declared,
            &region,
            now,
        )
        .map_err(lift)?;
        let indexes = resolve_indexes(table, &billing_mode, &declared, &index_updates, now)
            .map_err(lift)?;
        // The capacity the table would be provisioned for after this update is held to the
        // per-table throughput quota, exactly as a declaration of a NEW table's capacity is:
        // the quota bounds the sum of the table's own capacity and its indexes', whichever
        // call asks for it ([`crate::state::check_table_capacity_quota`]).
        if let Some(refusal) = crate::state::check_table_capacity_quota(
            &region,
            table_throughput.iter().chain(
                indexes
                    .iter()
                    .filter(|index| !index.being_deleted())
                    .filter_map(|index| index.provisioned_throughput.as_ref()),
            ),
        ) {
            return Err(UpdateTableErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(refusal),
                },
            )
            .into());
        }
        // The projection budget is summed across EVERY index the table would carry after
        // this update, the table's local and VECTOR indexes included: "the total count of
        // NonKeyAttributes summed across all of the secondary indexes, must not exceed 100"
        // (`Projection$NonKeyAttributes`), so an index added here spends from the same
        // budget as the ones created with the table — which is also what bounds the
        // projection list a `Create` stores.
        crate::state::check_projection_budget(
            indexes
                .iter()
                .filter(|index| !index.being_deleted())
                .map(|index| crate::state::projected_non_key_attributes(&index.projection))
                .chain(table.local_secondary_indexes.iter().map(|index| {
                    crate::state::projected_non_key_attributes(&index.projection)
                }))
                .chain(
                    vector_indexes
                        .iter()
                        .filter(|index| !index.being_deleted())
                        .map(crate::state::vector_projected_non_key_attributes),
                ),
        )
        .map_err(lift)?;
        // Only the attribute definitions some key schema still uses are kept, so a
        // repeated UpdateTable cannot grow the stored list: the stored list tracks the
        // LIVE keys of the table and of the indexes it carries after this update.
        let definitions = retained_definitions(declared, table, &indexes, &vector_indexes);
        // What this request asks of the table's stream, decided against the STORED
        // stream: "You receive a ResourceInUseException if you try to enable a stream
        // on a table that already has a stream, or if you try to disable a stream on
        // a table that doesn't have a stream" (`UpdateTable$StreamSpecification`), so
        // both contradictions are refused here — the last step that can fail, which
        // is what lets the writes below apply whole.
        let stream = match (&stream_request, table.stream.clone()) {
            (StreamRequest::Unchanged, stored) => stored,
            (StreamRequest::Enable(_), Some(stored)) if stored.enabled => {
                return Err(stream_specification_invalid(&name, STREAM_ALREADY_ENABLED))
            }
            // A table with no stream, or one whose stream was disabled, gets a NEW
            // stream: a stream's identity is its ARN and label, and the disabled one
            // keeps the records it captured, so re-enabling cannot reuse it.
            (StreamRequest::Enable(view_type), _) => Some(
                crate::state::register_table_stream(
                    &*self.engine,
                    &name,
                    &region,
                    &table.key_schema,
                    view_type,
                    now,
                )
                .map_err(stream_not_registered)?,
            ),
            (StreamRequest::Disable, Some(stored)) if stored.enabled => {
                // The table's enabled stream is always in the registry (the clock
                // reclaims a stream only 24 hours after it is DISABLED), so a
                // registry that does not hold it is a broken invariant rather than a
                // state the caller can reach — reported as such.
                if !crate::state::disable_table_stream(&*self.engine, &stored.arn, now) {
                    return Err(stream_not_registered(crate::state::StreamFault::Internal(
                        "the table's enabled stream is not in the DynamoDB Streams registry",
                    )));
                }
                // The table keeps reporting the ARN and label: they name its LATEST
                // stream, which outlives the disable.
                Some(crate::state::TableStream {
                    enabled: false,
                    ..stored
                })
            }
            (StreamRequest::Disable, _) => {
                return Err(stream_specification_invalid(&name, STREAM_NOT_ENABLED))
            }
        };
        // A replica this request CREATES is a table of its OWN Region, and a global table
        // replicates over "DynamoDB Streams enabled, with the stream containing both the
        // new and the old images of the item" (the `CreateGlobalTable` doc), so each new
        // replica of a table that publishes its changes gets a stream of its own Region —
        // registered HERE, while nothing has been written yet, so a registration this
        // emulator refuses refuses the whole update rather than leaving a replica
        // reporting a stream nothing holds.
        let replica_streams = registered_replica_streams(
            &*self.engine,
            &name,
            stream.as_ref(),
            &table.key_schema,
            &replica_actions,
            now,
        )
        .map_err(stream_not_registered)?;

        // Nothing above wrote to the table, so from here on the update applies whole.
        //
        // What a capacity move is measured against is what a description REPORTS, where
        // an on-demand table's pair is 0/0 ("on-demand mode does not use provisioned
        // throughput" — `ProvisionedThroughputDescription$ReadCapacityUnits`). So going
        // on-demand WITHDRAWS the capacity: the reported pair drops to zero, which is the
        // last time it went down, without spending one of the day's counted decreases.
        // Coming BACK from on-demand raises the reported pair from that zero, which is an
        // INCREASE of the capacity the table reports — the instant the recorded
        // description of such a switch carries under `LastIncreaseDateTime`.
        match (&table.provisioned_throughput, &table_throughput) {
            (Some(_), None) => table.throughput_history.record_capacity_withdrawal(now),
            (before, after) => {
                let before = crate::state::reported_capacity(before.as_ref());
                let after = crate::state::reported_capacity(after.as_ref());
                table.throughput_history.record(now, &before, &after);
            }
        }
        // "Represents the time when PAY_PER_REQUEST was last set as the read/write
        // capacity mode" is recorded below, once the instant the switch takes effect is
        // known.
        // An update whose work outlives the call finishes later — the clock owes the
        // move back to ACTIVE; one the service applies with the call is finished when it
        // returns, so it leaves the table settled and the next control-plane call is
        // served.
        let settles_later = asks_for_asynchronous_work(&input);
        let settles_at = if settles_later {
            now + crate::state::STATUS_TRANSITION
        } else {
            now
        };
        table.updated_at = settles_later.then_some(settles_at);
        // Whether the work still running is a CAPACITY MOVE, which is what the refusal a
        // second change earns while it runs states ([`moves_provisioned_capacity`]).
        table.capacity_moving_until =
            (settles_later && moves_provisioned_capacity(&input)).then_some(settles_at);
        // "The time when PAY_PER_REQUEST was last set as the read/write capacity mode"
        // (`BillingModeSummary$LastUpdateToPayPerRequestDateTime`) is the instant of the
        // CALL that set it, which is this one, so a description taken while the capacity
        // move is still settling already reports it — and a request that re-states the
        // mode the table is already in records nothing new.
        if billing_mode == "PAY_PER_REQUEST" && table.billing_mode != "PAY_PER_REQUEST" {
            table.pay_per_request_at = Some(now);
        }
        table.billing_mode = billing_mode;
        table.provisioned_throughput = table_throughput;
        table.warm_throughput = warm_throughput;
        table.attribute_definitions = definitions;
        table.global_secondary_indexes = indexes;
        table.vector_indexes = vector_indexes;
        table.stream = stream;
        if let Some(table_class) = input.table_class.clone() {
            // "The date and time at which the table class was last updated"
            // (`TableClassSummary$LastUpdateDateTime`): this call is that update, so it
            // is stamped at the instant the call was made — a request that re-states the
            // class the table is already in updates nothing, stamps nothing and spends
            // none of the quota's changes ([`crate::state::TableClassChanges`]).
            if table.table_class.as_deref() != Some(table_class.as_str()) {
                table.table_class_changes.record(now);
            }
            table.table_class = Some(table_class);
        }
        if let Some(protection) = input.deletion_protection_enabled {
            table.deletion_protection_enabled = protection;
        }
        table.encryption = encryption;
        // Each on-demand ceiling the request names is set or cleared, and one it leaves
        // out stays as it was ([`crate::state::OnDemandCeilings::apply`]).
        table.on_demand_throughput = table
            .on_demand_throughput
            .apply(input.on_demand_throughput.as_ref());

        // The replica actions are applied LAST, because each one reaches tables of other
        // Regions as well as this one: a created replica is a table of its own in its own
        // Region, and every regional table of the group records it
        // ([`crate::state::ComAmazonawsDynamodbDynamodb20120810State::apply_replica_actions`]).
        store.apply_replica_actions(&region, &name, &replica_actions, &replica_streams, now);
        // ...and the multi-Region consistency configuration beside them, which is a
        // configuration of the whole GROUP: it is recorded on the local table and on every
        // replica of the group, the ones this request just created included
        // ([`crate::state::ComAmazonawsDynamodbDynamodb20120810State::apply_consistency_configuration`]).
        store.apply_consistency_configuration(
            &region,
            &name,
            input.multi_region_consistency.as_deref(),
            &witness_actions,
            now,
        );

        // The description answers from the stored table, read back after every write this
        // update made — including the ones the replica actions made to it.
        let mut description = store.table(&region, &name).map_err(lift)?.describe(now);
        // Every capacity figure in it is the one the table is SERVING: the move this call
        // started outlives the call, so its figures are what the reads after it report and
        // this response carries the generation still in force
        // ([`crate::state::CapacityInForce::restate`]).
        //
        // A move of the capacity MODE is the exception for the PROVISIONED pair: the mode
        // itself is in force from the moment the call returns (the response's
        // `BillingModeSummary` and `TableThroughputModeSummary` report the mode the update
        // resolved to), and that pair belongs to the mode — an on-demand table provisions no
        // capacity at all, and a table moved onto provisioned capacity serves the pair the
        // update set — so the figures beside the new mode are the new mode's own. What the
        // table keeps WARM is a setting of its own, so that half is the generation in force
        // either way.
        if capacity_mode_moved {
            capacity_in_force.restate_warm(&mut description);
        } else {
            capacity_in_force.restate(&mut description);
        }
        // The audit record for a state-changing call: who asked, what the table carries
        // after the change and that it succeeded. Never a credential — only the access
        // key id the caller presented.
        ::tracing::info!(
            operation = Self::NAME,
            akid = %::emulator_server::extract_akid(&ctx.parts().headers),
            table_name = %name,
            table_status = %description.table_status.clone().unwrap_or_default(),
            global_secondary_indexes = description
                .global_secondary_indexes
                .as_ref()
                .map_or(0, Vec::len),
            vector_indexes = description.vector_indexes.as_ref().map_or(0, Vec::len),
            outcome = "updated",
            "table updated"
        );

        Ok(UpdateTableOutput {
            table_description: Some(description),
        })
    }
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> UpdateTableError {
    UpdateTableErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// The modelled refusal for an update whose `SSESpecification` could not be served: a key
/// this Region's KMS endpoint does not serve, or a type the service does not offer, is the
/// 400 the bad request it is, and a KMS store this process cannot reach at all is a server
/// fault — never a table reported as encrypted with a key ARN nothing minted.
fn sse_unresolved(fault: crate::state::SseFault) -> UpdateTableError {
    match fault {
        crate::state::SseFault::Invalid(error) => lift(error),
        crate::state::SseFault::StoreUnavailable => {
            UpdateTableErrorModeled::InternalServerError(InternalServerError {
                message: Some("the KMS key store is unavailable".to_string()),
            })
            .into()
        }
    }
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> UpdateTableError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            UpdateTableErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}

/// The modelled refusal for an update whose stream change the streams registry did
/// not take: its own quota is the control plane's `LimitExceededException`, and a
/// store this process cannot reach at all is a server fault. Refusing either way is
/// the only honest answer — a table reported with a stream nothing registered would
/// be invisible to every streams consumer.
fn stream_not_registered(fault: crate::state::StreamFault) -> UpdateTableError {
    match fault {
        crate::state::StreamFault::Rejected(reason) => {
            UpdateTableErrorModeled::LimitExceededException(LimitExceededException {
                message: Some(format!("Subscriber limit exceeded: {reason}")),
            })
            .into()
        }
        crate::state::StreamFault::StoreUnavailable => {
            UpdateTableErrorModeled::InternalServerError(InternalServerError {
                message: Some("the DynamoDB Streams store is unavailable".to_string()),
            })
            .into()
        }
        crate::state::StreamFault::Internal(detail) => {
            UpdateTableErrorModeled::InternalServerError(InternalServerError {
                message: Some(detail.to_string()),
            })
            .into()
        }
    }
}

/// The stream REGISTERED for each replica this request creates, by Region: a global table's
/// replicas are added over "DynamoDB Streams enabled, with the stream containing both the
/// new and the old images of the item", and a stream is a resource of ONE Region and of the
/// streams service, so each new replica gets a stream registered in its own Region,
/// capturing what the group's table captures.
///
/// Empty for a table with no enabled stream — a replica of such a table has no change feed
/// to mirror — and for a request that creates no replica. Made before anything is written,
/// so a registration the streams service refuses refuses the whole update
/// ([`stream_not_registered`]).
fn registered_replica_streams<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    name: &str,
    stream: Option<&crate::state::TableStream>,
    key_schema: &[KeySchemaElement],
    actions: &[crate::state::ReplicaAction],
    now: ::std::time::SystemTime,
) -> Result<Vec<(String, crate::state::TableStream)>, crate::state::StreamFault> {
    let Some(stream) = stream.filter(|stream| stream.enabled) else {
        return Ok(Vec::new());
    };
    let mut registered: Vec<(String, crate::state::TableStream)> = Vec::new();
    for action in actions {
        let crate::state::ReplicaAction::Create(settings) = action else {
            continue;
        };
        registered.push((
            settings.region.clone(),
            crate::state::register_table_stream(
                engine,
                name,
                &settings.region,
                key_schema,
                &stream.view_type,
                now,
            )?,
        ));
    }
    Ok(registered)
}

/// The refusal a `StreamSpecification` that contradicts the table's stored stream gets:
/// a request that asks to enable a stream the table already has, that asks to change the
/// view type of a stream in place (which is the same request), or that asks to disable
/// one the table does not have.
///
/// Classified as a `ValidationException` (HTTP 400) rather than as a resource-state
/// conflict: neither is a state the table will leave on its own, so the request can
/// never succeed as sent and the caller has to change it. The model's prose for
/// `StreamSpecification` names `ResourceInUseException`, but the service answers the
/// parameter error — and the CODE is what a client branches on, so a
/// transient-conflict code would have an SDK retry an input that cannot succeed.
///
/// The sentence states what the table's stream is and the table it belongs to, both
/// facts of the request and of the stored table.
fn stream_specification_invalid(name: &str, state: &str) -> UpdateTableError {
    crate::scaffolded::Scaffolded::validation(format!("{state}: TableName: {name}"))
}

/// What the table's stream already is, when a request asks to enable one.
const STREAM_ALREADY_ENABLED: &str = "Table already has an enabled stream";

/// What it is not, when a request asks to disable one.
const STREAM_NOT_ENABLED: &str = "Table has no stream to disable";

/// What a request's `StreamSpecification` asks of the table's stream.
#[derive(Debug)]
enum StreamRequest {
    /// The request names no `StreamSpecification`, so the table's stream — whether it
    /// has one or not — is left exactly as it is.
    Unchanged,
    /// Turn a stream ON, capturing records in this `StreamViewType`.
    Enable(String),
    /// Turn the table's stream OFF.
    Disable,
}

/// Read the request's `StreamSpecification` as one of the three things it can ask
/// for, refusing a specification that cannot hold together
/// ([`crate::state::stream_view_type`]).
fn requested_stream_change(
    input: &UpdateTableInput,
) -> Result<StreamRequest, crate::state::RequestError> {
    let Some(specification) = input.stream_specification.as_ref() else {
        return Ok(StreamRequest::Unchanged);
    };
    Ok(
        match crate::state::stream_view_type(
            specification,
            crate::state::STREAM_VIEW_TYPE_REQUIRED_ON_ENABLE,
            crate::state::STREAM_VIEW_TYPE_FORBIDDEN_ON_UPDATE,
        )? {
            Some(view_type) => StreamRequest::Enable(view_type.to_string()),
            None => StreamRequest::Disable,
        },
    )
}

/// One input member that can ASK FOR A CHANGE: its name, and the test for whether this
/// request names it.
type UpdatableMember = (&'static str, fn(&UpdateTableInput) -> bool);

/// Every input member that asks for a change.
///
/// These are the members of the operation's input shape
/// (`smithy_query(query="operation",operation="UpdateTable")`) except `TableName`,
/// which names the table to change, and `AttributeDefinitions`, which only declares the
/// attributes another member's key schema uses and changes nothing on its own. A
/// request naming none of these asks for nothing and is refused.
///
/// A list member asks for a change only when it carries an entry: an empty list requests
/// no index, no replica and no witness update.
///
/// This list drives the EMPTINESS TEST only. The sentence such a request is refused with
/// is fixed text ([`EMPTY_UPDATE_MESSAGE`]) and is not assembled from these names.
const UPDATABLE_MEMBERS: &[UpdatableMember] = &[
    ("ProvisionedThroughput", |input| {
        input.provisioned_throughput.is_some()
    }),
    ("BillingMode", |input| input.billing_mode.is_some()),
    // The throughput-mode spelling of the same setting, which the model declares beside it
    // ([`crate::state::requested_throughput_mode`]): a request that states the mode through
    // it asks for the same change, so it belongs in the one list that decides whether a
    // request asks for anything.
    ("TableThroughputMode", |input| {
        input.table_throughput_mode.is_some()
    }),
    ("GlobalSecondaryIndexUpdates", |input| {
        list_named(input.global_secondary_index_updates.as_ref())
    }),
    ("StreamSpecification", |input| {
        input.stream_specification.is_some()
    }),
    ("SSESpecification", |input| {
        input.sse_specification.is_some()
    }),
    ("ReplicaUpdates", |input| {
        list_named(input.replica_updates.as_ref())
    }),
    ("GlobalTableWitnessUpdates", |input| {
        list_named(input.global_table_witness_updates.as_ref())
    }),
    ("VectorIndexUpdates", |input| {
        list_named(input.vector_index_updates.as_ref())
    }),
    ("OnDemandThroughput", |input| {
        input.on_demand_throughput.is_some()
    }),
    ("WarmThroughput", |input| input.warm_throughput.is_some()),
    ("MultiRegionConsistency", |input| {
        input.multi_region_consistency.is_some()
    }),
    ("GlobalTableSettingsReplicationMode", |input| {
        input.global_table_settings_replication_mode.is_some()
    }),
    ("DisassociationProtectionEnabled", |input| {
        input.disassociation_protection_enabled.is_some()
    }),
    ("DeletionProtectionEnabled", |input| {
        input.deletion_protection_enabled.is_some()
    }),
    ("TableClass", |input| input.table_class.is_some()),
];

/// Whether a list member carries an entry at all.
fn list_named<T>(list: Option<&Vec<T>>) -> bool {
    list.is_some_and(|entries| !entries.is_empty())
}

/// Whether this request names any member that asks for a change.
fn names_an_update(input: &UpdateTableInput) -> bool {
    UPDATABLE_MEMBERS.iter().any(|(_, named)| named(input))
}

/// The refusal for a request that names no updatable member: the FIXED sentence the
/// service serves for that case, listing the members it accepts in the order it lists
/// them, so the caller can fix the request from the message alone.
///
/// The sentence carries nothing of the request, and its list is the SERVICE's rather
/// than this model's: it names members the model does not declare on this input
/// (`UpdateStreamEnabled` for the stream specification, `MultiAccountReplicaReady`,
/// `ReplicaTransitRoleArn`) and leaves out ones it does
/// (`smithy_query(query="operation",operation="UpdateTable")` also declares
/// `TableThroughputMode`, `GlobalSecondaryIndexUpdates`' siblings
/// `GlobalTableWitnessUpdates` and `VectorIndexUpdates`,
/// `GlobalTableSettingsReplicationMode` and `DisassociationProtectionEnabled`). Nothing
/// in the model derives that list, so the sentence is held here as the one text the
/// service states, and the members a request may actually ask a change through stay
/// [`UPDATABLE_MEMBERS`] — a caller reading the message gets the service's own
/// documentation of the alternatives, which is what a client matching on it reads from
/// DynamoDB.
const EMPTY_UPDATE_MESSAGE: &str = "At least one of ProvisionedThroughput, BillingMode, \
     UpdateStreamEnabled, GlobalSecondaryIndexUpdates, SSESpecification, ReplicaUpdates, \
     MultiAccountReplicaReady, ReplicaTransitRoleArn, MultiRegionConsistency, \
     DeletionProtectionEnabled, OnDemandThroughput, WarmThroughput or TableClass is required";

/// Whether this request asks DynamoDB for work that OUTLIVES the call, which is the
/// part of an `UpdateTable` the clock has to finish ("while it's executing, the table
/// status changes from ACTIVE to UPDATING" — the doc above).
///
/// Two kinds of change are that work. One MOVES CAPACITY: the table's capacity mode and
/// provisioned throughput, the warm throughput it keeps ready ("the minimum provisioned
/// capacity maintained for immediate data access" — the `TableWarmThroughputDescription`
/// doc), its table class and its indexes — secondary and vector alike — are all backed
/// by provisioned capacity or stored data, and an index removed from the table releases
/// what it was holding. The other RE-PLUMBS THE TABLE'S CHANGE FEED: a
/// `StreamSpecification` starts or stops a DynamoDB Streams stream, a resource with its
/// own lifecycle in the streams service (a stream being turned off is `DISABLING` before
/// it is `DISABLED`), so that too is settled after the call returns.
///
/// Two more changes outlive the call for the same reason. A `ReplicaUpdates` entry acts on
/// a table in ANOTHER Region — "this request invokes the `CreateTableReplica` action in
/// the destination Region" — whose own `CREATING`/`DELETING` span is the replica's; and an
/// `SSESpecification` re-encrypts the table, which is why `SSEDescription` has an
/// `UPDATING` status of its own. A `GlobalTableWitnessUpdates` entry is the same kind of
/// work in a Region of its own, which is why a witness has a `CREATING`/`DELETING` span too
/// (`WitnessStatus`).
///
/// Deletion protection and the on-demand ceilings are settings with nothing behind them:
/// the service applies each with the call and answers with the table still `ACTIVE`, so
/// nothing is left for the clock to finish and the next control-plane call is served.
fn asks_for_asynchronous_work(input: &UpdateTableInput) -> bool {
    input.billing_mode.is_some()
        || input.table_throughput_mode.is_some()
        || input.provisioned_throughput.is_some()
        || input.warm_throughput.is_some()
        || input.table_class.is_some()
        || input.stream_specification.is_some()
        || input.sse_specification.is_some()
        || list_named(input.global_secondary_index_updates.as_ref())
        || list_named(input.vector_index_updates.as_ref())
        || list_named(input.replica_updates.as_ref())
        || list_named(input.global_table_witness_updates.as_ref())
}

/// Whether this request moves PROVISIONED CAPACITY — the read and write units the table
/// or one of its indexes serves — rather than only changing some other setting of the
/// table.
///
/// "UpdateTable is an asynchronous operation" and the capacity it moves is what the
/// service is still working on while the table reports `UPDATING`, so this is the fact a
/// refusal of a second change to that table states
/// ([`crate::state::Table::throughput_update_in_flight`]).
///
/// Three members ask for such a move: the table's own `ProvisionedThroughput`, the pair an
/// index `Update` carries, and a change of the capacity MODE — a switch to
/// `PAY_PER_REQUEST` withdraws the provisioned pair and a switch back sets one, which is
/// the same move read the other way. A warm-throughput change, a table class, a stream, an
/// SSE specification, an index create or delete and a replica update are all asynchronous
/// work on the table that moves no capacity figure, so none of them is one.
fn moves_provisioned_capacity(input: &UpdateTableInput) -> bool {
    input.provisioned_throughput.is_some()
        || input.billing_mode.is_some()
        || input.table_throughput_mode.is_some()
        || input
            .global_secondary_index_updates
            .iter()
            .flatten()
            .any(|update| {
                update
                    .update
                    .as_ref()
                    .is_some_and(|change| change.provisioned_throughput.is_some())
            })
}

/// Every on-demand capacity ceiling this request names: the table's own, and the one
/// each index create or index update carries — an index's capacity mode is the table's,
/// so the same rule holds for all of them.
fn requested_on_demand_ceilings(
    input: &UpdateTableInput,
) -> impl Iterator<Item = &OnDemandThroughput> {
    ::std::iter::once(input.on_demand_throughput.as_ref())
        .chain(
            input
                .global_secondary_index_updates
                .iter()
                .flatten()
                .flat_map(|update| {
                    [
                        update
                            .create
                            .as_ref()
                            .and_then(|create| create.on_demand_throughput.as_ref()),
                        update
                            .update
                            .as_ref()
                            .and_then(|change| change.on_demand_throughput.as_ref()),
                    ]
                }),
        )
        .flatten()
}

/// Every capacity pair this request states: the table's own, and the one each index
/// create or index update carries — the bound is on the figure, so the same rule holds for
/// all of them ([`crate::state::check_capacity_bounds`]).
fn requested_capacities(input: &UpdateTableInput) -> impl Iterator<Item = &ProvisionedThroughput> {
    ::std::iter::once(input.provisioned_throughput.as_ref())
        .chain(
            input
                .global_secondary_index_updates
                .iter()
                .flatten()
                .flat_map(|update| {
                    [
                        update
                            .create
                            .as_ref()
                            .and_then(|create| create.provisioned_throughput.as_ref()),
                        update
                            .update
                            .as_ref()
                            .and_then(|change| change.provisioned_throughput.as_ref()),
                    ]
                }),
        )
        .flatten()
}

/// The first input member present that this slice does not store, if any.
fn unsupported_member(input: &UpdateTableInput) -> Option<&'static str> {
    if input.global_table_settings_replication_mode.is_some() {
        return Some("GlobalTableSettingsReplicationMode");
    }
    if input.disassociation_protection_enabled.is_some() {
        return Some("DisassociationProtectionEnabled");
    }
    // An index's ON-DEMAND throughput is the one member this slice does not store, so an
    // on-demand ceiling asked for per index is "not written yet" rather than accepted and
    // dropped. Checked here so it answers the scaffold's 501, like the table-level members
    // above, instead of a bad-request refusal. An index's WARM throughput is stored and
    // reported, by the same rule the table's own is.
    for update in input.global_secondary_index_updates.iter().flatten() {
        let on_demand = update
            .create
            .as_ref()
            .is_some_and(|create| create.on_demand_throughput.is_some())
            || update
                .update
                .as_ref()
                .is_some_and(|change| change.on_demand_throughput.is_some());
        if on_demand {
            return Some("GlobalSecondaryIndexUpdates carrying OnDemandThroughput");
        }
    }
    None
}

/// The structure one `GlobalSecondaryIndexUpdates` entry is, as the refusals of an entry
/// that states no single action name it.
const INDEX_UPDATE_SHAPE: &str = "GlobalSecondaryIndexUpdate";

/// Its action members, in the order the model declares them
/// (`smithy_query(query="shape", shape="GlobalSecondaryIndexUpdate")`), which is the order
/// a refusal naming them follows.
const INDEX_UPDATE_ACTIONS: [&str; 3] = ["Update", "Create", "Delete"];

/// The same two facts for one `VectorIndexUpdates` entry, which models two actions rather
/// than three (`smithy_query(query="shape", shape="VectorIndexUpdate")`).
const VECTOR_INDEX_UPDATE_SHAPE: &str = "VectorIndexUpdate";

/// Its action members, in the order the model declares them.
const VECTOR_INDEX_UPDATE_ACTIONS: [&str; 2] = ["Create", "Delete"];

/// The index actions one request asks for, each named at most once, with at most one
/// create and at most one delete: "You can create or delete only one global secondary
/// index per UpdateTable operation" (`UpdateTable$GlobalSecondaryIndexUpdates`).
///
/// The caller-chosen list is CLAMPED before anything walks it: a table carries at most
/// [`crate::state::MAX_GLOBAL_SECONDARY_INDEXES`] indexes, so a longer list names an
/// index twice or an index that cannot exist and is already invalid. Duplicate
/// detection then costs one hash per entry rather than a scan, and the one-create /
/// one-delete rule short-circuits on the second of either.
fn validated_index_updates(
    input: &UpdateTableInput,
) -> Result<Vec<GlobalSecondaryIndexUpdate>, crate::state::RequestError> {
    let updates = input
        .global_secondary_index_updates
        .clone()
        .unwrap_or_default();
    if updates.len() > crate::state::MAX_GLOBAL_SECONDARY_INDEXES {
        return Err(crate::state::RequestError::Validation(format!(
            "Invalid GlobalSecondaryIndexUpdates: a table carries at most {} global secondary \
             indexes, so a request cannot name {} of them",
            crate::state::MAX_GLOBAL_SECONDARY_INDEXES,
            updates.len()
        )));
    }
    let mut named: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
    for update in &updates {
        // The index the entry acts on, which exists only when the entry asks for exactly
        // one of the three actions the model gives it: an entry asking for none describes
        // no change, and one asking for several no single change.
        let index_name = match (&update.update, &update.create, &update.delete) {
            (Some(change), None, None) => change.index_name.as_str(),
            (None, Some(create), None) => create.index_name.as_str(),
            (None, None, Some(delete)) => delete.index_name.as_str(),
            (None, None, None) => {
                return Err(crate::state::RequestError::Validation(
                    crate::state::no_index_action_message(
                        INDEX_UPDATE_SHAPE,
                        &INDEX_UPDATE_ACTIONS,
                    ),
                ))
            }
            _ => {
                return Err(crate::state::RequestError::Validation(
                    crate::state::one_index_action_message(
                        INDEX_UPDATE_SHAPE,
                        &INDEX_UPDATE_ACTIONS,
                    ),
                ))
            }
        };
        if !named.insert(index_name) {
            // An UPDATE list naming one index twice asks for two changes to one resource,
            // which is a defect of the list rather than of a declaration: AWS states the
            // rule the list broke — one update per index at a time — and names the index
            // it was broken over, which is not the sentence a request DECLARING two
            // indexes of a name earns ([`crate::state::duplicate_index_name`]).
            return Err(crate::state::RequestError::Validation(
                one_update_per_index_message(index_name),
            ));
        }
    }
    Ok(updates)
}

/// AWS's text for a `GlobalSecondaryIndexUpdates` list that names one index in more than
/// one entry: the rule the list broke, with the index it was broken over as its one
/// variable part.
///
/// Fixed text around that name: the sentence states how many updates of ONE index a single
/// call carries and nothing of how many entries named it or which actions they asked for,
/// so it reads the same for two deletes of a name, two creates of it, and an update beside
/// either.
fn one_update_per_index_message(index_name: &str) -> String {
    format!(
        "One or more parameter values were invalid: Only one global secondary index update per \
         index is allowed simultaneously. Index: {index_name}"
    )
}

/// Why one request asks for more index lifecycle work than a single `UpdateTable` may
/// carry, or `None` when it stays inside the limit: "You can create or delete only one
/// global secondary index per UpdateTable operation"
/// (`UpdateTable$GlobalSecondaryIndexUpdates`).
///
/// A CREATE and a DELETE in one call are two lifecycle actions on one table's indexes,
/// which is one more than a single call may carry, so the two are counted TOGETHER; the
/// sentence names the limit and nothing of which index or how many the request named.
fn index_lifecycle_actions_over_limit(updates: &[GlobalSecondaryIndexUpdate]) -> Option<String> {
    let lifecycle = updates
        .iter()
        .filter(|update| update.create.is_some() || update.delete.is_some())
        .count();
    if lifecycle <= ONLINE_INDEX_ACTIONS_AT_ONCE {
        return None;
    }
    // AWS's text for this limit is FIXED: it states the limit and nothing of which
    // index or how many the request named, so it reads the same whichever side of the
    // lifecycle the request asked twice for — and the same sentence answers for a vector
    // index ([`vector_index_actions_over_limit`]).
    Some(online_index_limit_message(ONLINE_INDEX_ACTIONS_AT_ONCE))
}

/// The build an index ADDED to `table` at `now` has ahead of it, whichever kind of index
/// it is: the storage its entries need is laid down first, and then the items the table
/// already holds are read ([`crate::state::IndexBackfill::added`] states the phases over
/// the span the table's own update takes).
fn added_index_backfill(
    table: &crate::state::Table,
    now: ::std::time::SystemTime,
) -> crate::state::IndexBackfill {
    crate::state::IndexBackfill::added(
        !table.items.is_empty(),
        now,
        crate::state::STATUS_TRANSITION,
    )
}

/// The sentence AWS states a per-call index lifecycle limit with: the limit, and nothing
/// of which index or how many the request named.
fn online_index_limit_message(per_call: usize) -> String {
    format!(
        "Subscriber limit exceeded: Only {per_call} online index can be created or deleted \
         simultaneously per table"
    )
}

/// How many of a table's indexes may be created or deleted AT ONCE: "Only 1 online index
/// can be created or deleted simultaneously per table"
/// ([`online_index_limit_message`]) — the limit each call is held to
/// ([`index_lifecycle_actions_over_limit`], [`vector_index_actions_over_limit`]) and the
/// limit the TABLE is held to across calls ([`index_build_refusal`]), which is one
/// figure: a second index change cannot be started while the first is still being built,
/// whether both were asked for in one request or in two.
const ONLINE_INDEX_ACTIONS_AT_ONCE: usize = 1;

/// The refusal an `UpdateTable` earns from the index BUILDS already running on the table
/// it addresses, and from the table's own unsettled update, or `None` when the request is
/// one such a table still serves.
///
/// "While it's UPDATING, you can't issue another UpdateTable request" (the operation's
/// doc), and `ResourceInUseException` is the modelled answer for "you tried to update a
/// resource that was already being updated" — but a table whose update has settled keeps
/// BUILDING an index it was given over items it already held, and what the caller asked of
/// that build decides the answer:
///
/// * a second index change, while one index is still being built, is past the limit on how
///   many of a table's indexes may be built at once ([`ONLINE_INDEX_ACTIONS_AT_ONCE`]), so
///   it is answered the control plane's `LimitExceededException` — the code a client backs
///   off and retries one index at a time on, rather than the in-use code it would abort on;
/// * a DELETE of the very index that is being built CANCELS that build rather than asking
///   for a second one, so it is served — cancelling a build is the escape hatch a caller
///   that mis-specified an index has, and the index moves to `DELETING` from there;
/// * except while a VECTOR index is still having its resources allocated, the phase every
///   build starts in ([`crate::state::VectorIndex::in_resource_allocation`]): there is
///   nothing allocated to take away yet, so that delete is the in-use refusal naming the
///   phase to retry in, and the index keeps building;
/// * everything else asked of a table that is still UPDATING is the plain in-use refusal,
///   over the table this request addressed.
///
/// Every answer is decided over the indexes of BOTH kinds, because each of them is about
/// an index being built and none of them is about the kind of index it is.
fn index_build_refusal(
    stored: &crate::state::Table,
    name: &str,
    table_updating: bool,
    index_updates: &[GlobalSecondaryIndexUpdate],
    vector_index_updates: &[VectorIndexUpdate],
    now: ::std::time::SystemTime,
) -> Option<UpdateTableError> {
    let in_use = |message: String| -> UpdateTableError {
        UpdateTableErrorModeled::ResourceInUseException(ResourceInUseException {
            message: Some(message),
        })
        .into()
    };
    // A vector index whose build is still having its RESOURCES ALLOCATED is not yet a
    // resource a delete can take away ([`crate::state::VectorIndex::in_resource_allocation`]),
    // so the delete is refused with the in-use class — "you tried to update a resource that
    // was already being updated" — and the phase the caller must wait for, before anything
    // is staged, leaving the index exactly as it was. Once the build is reading the items
    // the table already held, or has settled, the same delete is served below.
    for delete in vector_index_updates
        .iter()
        .filter_map(|update| update.delete.as_ref())
    {
        if stored
            .vector_indexes
            .iter()
            .any(|index| index.name == delete.index_name && index.in_resource_allocation(now))
        {
            return Some(in_use(format!(
                "Attempt to change a resource which is still in use: Index creation is in \
                 resource allocation phase. Retry deletion during backfilling phase or when \
                 the index is active. Table: {name} Index: {}",
                delete.index_name
            )));
        }
    }
    // Every index of this table the clock still owes its build, whichever KIND it is, as
    // the name it answers to. One list, because both the per-table limit and the
    // cancellation below are about an index being BUILT and neither is about its kind.
    let building: Vec<&str> = stored
        .vector_indexes
        .iter()
        .filter(|index| index.status(now) == crate::state::IndexStatus::Creating)
        .map(|index| index.name.as_str())
        .chain(
            stored
                .global_secondary_indexes
                .iter()
                .filter(|index| index.status(now) == crate::state::IndexStatus::Creating)
                .map(|index| index.name.as_str()),
        )
        .collect();
    // How many of the lifecycle actions this request asks for CANCEL a build that is still
    // running, which is work on the build already in flight rather than a second one — a
    // delete naming an index of either kind that is being built.
    let deleted = index_updates
        .iter()
        .filter_map(|update| update.delete.as_ref())
        .map(|delete| delete.index_name.as_str())
        .chain(
            vector_index_updates
                .iter()
                .filter_map(|update| update.delete.as_ref())
                .map(|delete| delete.index_name.as_str()),
        );
    let cancels = deleted
        .filter(|index_name| building.contains(index_name))
        .count();
    let lifecycle = index_updates
        .iter()
        .filter(|update| update.create.is_some() || update.delete.is_some())
        .count()
        + vector_index_updates
            .iter()
            .filter(|update| update.create.is_some() || update.delete.is_some())
            .count();
    if lifecycle > cancels {
        if !building.is_empty() {
            return Some(
                UpdateTableErrorModeled::LimitExceededException(LimitExceededException {
                    message: Some(online_index_limit_message(ONLINE_INDEX_ACTIONS_AT_ONCE)),
                })
                .into(),
            );
        }
    } else if cancels > 0 {
        // Nothing but the cancellation of a build already running, which is served even
        // while the table's own update is settling: it is that very update it stops.
        return None;
    }
    table_updating.then(|| {
        // WHAT is still running is what the refusal states: a capacity move is reported
        // as the table's IOPS being updated, naming the table after the fact, and every
        // other unsettled update as the table being updated
        // ([`crate::state::Table::throughput_update_in_flight`]).
        let detail = if stored.throughput_update_in_flight(now) {
            format!("Table IOPS are currently being updated. Table: {name}")
        } else {
            format!("Table is being updated: {name}")
        };
        in_use(format!(
            "Attempt to change a resource which is still in use: {detail}"
        ))
    })
}

/// Why one request asks for more VECTOR-index lifecycle work than a single `UpdateTable`
/// may carry, or `None` when it stays inside the limit.
///
/// `UpdateTableInput$VectorIndexUpdates` is a list, but a create or a delete of a vector
/// index is asynchronous work on the table itself — the table reports `UPDATING` until it
/// settles — so one call carries ONE such action in all: a create and a delete in one call
/// are two, and are refused like two creates. (The secondary-index limit beside it counts
/// its create and its delete separately, which is the rule stated for that member.) AWS
/// states both limits with the same sentence ([`online_index_limit_message`]), and it is
/// answered with the control plane's modelled `LimitExceededException` — the code a client
/// backs off on and retries one index at a time.
fn vector_index_actions_over_limit(updates: &[VectorIndexUpdate]) -> Option<String> {
    let actions = updates
        .iter()
        .filter(|update| update.create.is_some() || update.delete.is_some())
        .count();
    (actions > ONLINE_INDEX_ACTIONS_AT_ONCE)
        .then(|| online_index_limit_message(ONLINE_INDEX_ACTIONS_AT_ONCE))
}

/// The vector-index actions one request asks for, each index named at most once.
///
/// The caller-chosen list is CLAMPED before anything walks it: a table carries at most
/// [`crate::state::MAX_VECTOR_INDEXES`] vector indexes, so a longer list names an index
/// twice or an index that cannot exist and is already invalid. Duplicate detection then
/// costs one hash per entry rather than a scan, and each entry has to ask for exactly
/// one of the two actions the model gives it — an entry that asks for both, or for
/// neither, names no single change the table could apply.
///
/// A `Create`'s own caller-chosen list is clamped here too: a vector index's search schema
/// carries at most [`crate::state::MAX_VECTOR_SEARCH_SCHEMA_ELEMENTS`] elements, the bound
/// `CreateTable` holds a vector index declared WITH the table to, so neither the walk below
/// nor the stored index can be grown by one request.
fn validated_vector_index_updates(
    input: &UpdateTableInput,
) -> Result<Vec<VectorIndexUpdate>, crate::state::RequestError> {
    let updates = input.vector_index_updates.clone().unwrap_or_default();
    if updates.len() > crate::state::MAX_VECTOR_INDEXES {
        return Err(crate::state::RequestError::Validation(format!(
            "Invalid VectorIndexUpdates: a table carries at most {} vector indexes, so a request \
             cannot name {} of them",
            crate::state::MAX_VECTOR_INDEXES,
            updates.len()
        )));
    }
    let mut named: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
    for update in &updates {
        // The name the entry changes, which exists only when the entry asks for exactly
        // one of the two actions; an entry that asks for both, or for neither, names no
        // single change and is refused over the two action members it had to pick one of
        // ([`crate::state::no_index_action_message`]). Matched rather than
        // counted-then-unwrapped, so this stays total for every entry a request can send.
        let index_name = match (&update.create, &update.delete) {
            (Some(create), None) => create.index_name.as_str(),
            (None, Some(delete)) => delete.index_name.as_str(),
            (None, None) => {
                return Err(crate::state::RequestError::Validation(
                    crate::state::no_index_action_message(
                        VECTOR_INDEX_UPDATE_SHAPE,
                        &VECTOR_INDEX_UPDATE_ACTIONS,
                    ),
                ))
            }
            (Some(_), Some(_)) => {
                return Err(crate::state::RequestError::Validation(
                    crate::state::one_index_action_message(
                        VECTOR_INDEX_UPDATE_SHAPE,
                        &VECTOR_INDEX_UPDATE_ACTIONS,
                    ),
                ))
            }
        };
        if !named.insert(index_name) {
            // One index named twice in one update list, which keeps that member's own
            // sentence for the reason the secondary-index list above does.
            return Err(crate::state::RequestError::Validation(format!(
                "Invalid VectorIndexUpdates: index {index_name} is named more than once"
            )));
        }
        // The search schema a `Create` carries is request-sized, and it is both walked (once
        // per element, against the table's attribute definitions) and STORED, so it is held
        // to the per-index element bound before either happens — by the one statement of
        // that rule, which `CreateTable` holds a declared index to as well.
        if let Some(create) = &update.create {
            crate::ops::create_table::clamped_search_schema(
                &create.index_name,
                create.search_schema.as_ref().map_or(0, Vec::len),
            )?;
        }
    }
    Ok(updates)
}

/// The table's provisioned throughput after the update: "When switching from
/// pay-per-request to provisioned capacity, initial provisioned capacity values must be
/// set" (`UpdateTable$BillingMode`), and a `PAY_PER_REQUEST` table carries none.
///
/// A pair the table ALREADY has is no move at all, and is refused rather than applied
/// ([`crate::state::throughput_unchanged_message`]).
fn resolve_table_throughput(
    table: &crate::state::Table,
    billing_mode: &str,
    input: &UpdateTableInput,
) -> Result<Option<ProvisionedThroughput>, crate::state::RequestError> {
    if billing_mode == "PAY_PER_REQUEST" {
        if input.provisioned_throughput.is_some() {
            // WHICH sentence states it follows what the REQUEST said: a request that
            // names the billing mode is told the two members contradict each other,
            // while one that names only the throughput is told what the mode in force
            // accepts — the capacity members cannot be set on a table that is
            // pay-per-request.
            return Err(crate::state::RequestError::Validation(
                match input.billing_mode.is_some() || input.table_throughput_mode.is_some() {
                    true => super::create_table::THROUGHPUT_WITH_PAY_PER_REQUEST_MESSAGE
                        .to_string(),
                    false => "One or more parameter values were invalid: Neither \
                              ReadCapacityUnits nor WriteCapacityUnits can be specified when \
                              BillingMode is PAY_PER_REQUEST"
                        .to_string(),
                },
            ));
        }
        return Ok(None);
    }
    // A request that re-states the pair the table is already provisioned for asks for no
    // change, whatever else the call carries: refused before anything is written, so the
    // table is not transitioned for work there is none of. Only a pair the REQUEST named
    // is held to this: a call that leaves the member out keeps the table's own pair, and
    // keeping a value is not requesting it.
    if let (Some(requested), Some(current)) =
        (&input.provisioned_throughput, &table.provisioned_throughput)
    {
        if requested.read_capacity_units == current.read_capacity_units
            && requested.write_capacity_units == current.write_capacity_units
        {
            return Err(crate::state::RequestError::Validation(
                crate::state::throughput_unchanged_message(
                    "the table",
                    "the table",
                    current,
                    requested,
                ),
            ));
        }
    }
    match input
        .provisioned_throughput
        .clone()
        .or_else(|| table.provisioned_throughput.clone())
    {
        Some(throughput) => {
            if throughput.read_capacity_units < 1 || throughput.write_capacity_units < 1 {
                return Err(crate::state::RequestError::Validation(format!(
                    "Invalid ProvisionedThroughput: ReadCapacityUnits and WriteCapacityUnits \
                     must be at least 1, got {} and {}",
                    throughput.read_capacity_units, throughput.write_capacity_units
                )));
            }
            Ok(Some(throughput))
        }
        None => Err(crate::state::RequestError::Validation(
            THROUGHPUT_WITHOUT_PROVISIONED_ON_UPDATE_MESSAGE.to_string(),
        )),
    }
}

/// AWS's fixed text for an UPDATE that puts a table on `PROVISIONED` without stating a
/// `ProvisionedThroughput` — and whose table carries none either, since a table already
/// provisioned keeps its own pair.
///
/// "When switching from pay-per-request to provisioned capacity, initial provisioned
/// capacity values must be set" (`UpdateTable$BillingMode`): the sentence names the BLOCK
/// those values travel in rather than the two capacity members inside it, which is where it
/// differs from the create's sentence for the same rule
/// ([`super::create_table::THROUGHPUT_WITHOUT_PROVISIONED_MESSAGE`]) — a create has no
/// stored pair to fall back on, so it complains about the members it needed, while an update
/// complains about the member the request had to carry. It interpolates nothing.
pub(crate) const THROUGHPUT_WITHOUT_PROVISIONED_ON_UPDATE_MESSAGE: &str =
    "One or more parameter values were invalid: ProvisionedThroughput must be specified when \
     BillingMode is PROVISIONED";

/// The attribute definitions this request declares, refused when the list is longer
/// than any table could keep.
///
/// A table's definitions can never outnumber the key attributes of its own key schema
/// plus those of its indexes (see [`crate::state::MAX_ATTRIBUTE_DEFINITIONS`]), so a
/// longer list is already invalid — and clamping it HERE, before the store lock is
/// taken, is what bounds the merge below: both the stored list and the declared one
/// would otherwise be request-sized.
fn clamped_definitions(
    input: &UpdateTableInput,
) -> Result<Vec<AttributeDefinition>, crate::state::RequestError> {
    let declared = input.attribute_definitions.clone().unwrap_or_default();
    if declared.len() > crate::state::MAX_ATTRIBUTE_DEFINITIONS {
        return Err(crate::state::RequestError::Validation(format!(
            "Invalid AttributeDefinitions: a table may declare at most {} attribute \
             definitions, got {}",
            crate::state::MAX_ATTRIBUTE_DEFINITIONS,
            declared.len()
        )));
    }
    // One attribute defined twice states two types for one attribute of the item, which is
    // the same defect of the same declared list whichever call carries it
    // ([`super::create_table::check_distinct_definitions`]).
    super::create_table::check_distinct_definitions(&declared)?;
    Ok(declared)
}

/// The table's attribute definitions after the update: the stored ones plus the ones
/// this request declares — "If you are adding a new global secondary index to the
/// table, AttributeDefinitions must include the key element(s) of the new index"
/// (`UpdateTable$AttributeDefinitions`). The list [`retained_definitions`] keeps is a
/// subset of this one.
///
/// The merge is done through a map of name -> position, so it costs one hash per
/// declared definition rather than a scan of the accumulated list per declared
/// definition.
fn merged_definitions(
    table: &crate::state::Table,
    declared: &[AttributeDefinition],
) -> Vec<AttributeDefinition> {
    let mut definitions = table.attribute_definitions.clone();
    let mut positions: ::std::collections::HashMap<String, usize> = definitions
        .iter()
        .enumerate()
        .map(|(position, definition)| (definition.attribute_name.clone(), position))
        .collect();
    for declared in declared {
        match positions.get(&declared.attribute_name) {
            Some(position) => {
                definitions[*position].attribute_type = declared.attribute_type.clone();
            }
            None => {
                positions.insert(declared.attribute_name.clone(), definitions.len());
                definitions.push(declared.clone());
            }
        }
    }
    definitions
}

/// The definitions the updated table keeps: exactly those a schema of the table still
/// uses — its own key, the key of one of the indexes it carries after the update, or the
/// vector attribute or search schema of one of the VECTOR indexes it carries after the
/// update.
///
/// This is what bounds the stored list: it can never hold more entries than those
/// schemas name, however many `UpdateTable` calls declare definitions. A declaration no
/// schema uses is DROPPED rather than refused, which is where this parts from
/// `CreateTable`: there the member describes the whole key surface of the table being
/// created, so an unused entry contradicts the request ("Some AttributeDefinitions are
/// not used in any KeySchema"), while here it only has to "include the key element(s) of
/// the new index" (`UpdateTable$AttributeDefinitions`) — a declaration beyond that asks
/// for nothing, so the update carries on and the stored list tracks the live keys. A
/// definition that was stored and whose index this same request deleted is dropped by
/// the same rule, since removing an index removes its keys — and that holds from the
/// instant the delete is ACCEPTED, so an index on its way out
/// ([`crate::state::GlobalSecondaryIndex::being_deleted`],
/// [`crate::state::VectorIndex::being_deleted`]) keeps no definition alive even while the
/// table still reports it `DELETING`.
///
/// The used attributes are collected ONCE into a set, so the filter costs one hash per
/// definition rather than a walk over every schema.
fn retained_definitions(
    declared: Vec<AttributeDefinition>,
    table: &crate::state::Table,
    indexes: &[crate::state::GlobalSecondaryIndex],
    vector_indexes: &[crate::state::VectorIndex],
) -> Vec<AttributeDefinition> {
    let used: ::std::collections::HashSet<&str> = table
        .key_schema
        .iter()
        .chain(
            indexes
                .iter()
                .filter(|index| !index.being_deleted())
                .flat_map(|index| index.key_schema.iter()),
        )
        .chain(
            table
                .local_secondary_indexes
                .iter()
                .flat_map(|index| index.key_schema.iter()),
        )
        .map(|element| element.attribute_name.as_str())
        .chain(
            vector_indexes
                .iter()
                .filter(|index| !index.being_deleted())
                .flat_map(crate::state::VectorIndex::consumed_attributes),
        )
        .collect();
    declared
        .into_iter()
        .filter(|definition| used.contains(definition.attribute_name.as_str()))
        .collect()
}

/// Every attribute an index this request CREATES keys on must be declared BY THIS
/// REQUEST: "If you are adding a new global secondary index to the table,
/// AttributeDefinitions must include the key element(s) of the new index"
/// (`UpdateTable$AttributeDefinitions`).
///
/// The list the rule is about is the one the REQUEST carries, not the one the table
/// already holds: the sentence is that `AttributeDefinitions` must include the new
/// index's key elements, so an update that leaves the member out — or leaves one of the
/// new key attributes out of it — has not declared the index's key, whatever the table
/// happens to define already.
///
/// WHICH refusal it earns follows from what the request declared. A request that declares
/// no definition at all states nothing for the two lists to be compared over, so AWS
/// names the MEMBER that is missing and the index that needed it
/// ([`definitions_required_message`]) — the wording every member an index of this service
/// requires is stated with ([`crate::state::index_capacity_required_message`] for its
/// capacity pair). A request that declares some, but not the index's key attributes, earns
/// the sentence a key attribute with no definition earns wherever a schema declares one,
/// chosen by the same count ([`super::create_table::undefined_key_refusal`]): both lists
/// where the request declared at least as many definitions as the index's key has
/// elements, and the element left without a definition where it declared fewer.
///
/// Both lists are the REQUEST's own, so this is answered before the table the request
/// names is resolved: an undeclared key attribute makes the call permanently invalid
/// whether or not that table exists, and the not-found class would have a client retry
/// it.
fn check_requested_index_definitions(
    updates: &[GlobalSecondaryIndexUpdate],
    requested_definitions: &[AttributeDefinition],
) -> Result<(), crate::state::RequestError> {
    for update in updates {
        if let Some(create) = &update.create {
            check_index_definitions(
                &create.index_name,
                &create.key_schema,
                requested_definitions,
            )?;
        }
    }
    Ok(())
}

/// Hold ONE index a request creates to having its key attributes declared in the
/// `AttributeDefinitions` that request carried.
fn check_index_definitions(
    index_name: &str,
    key_schema: &[KeySchemaElement],
    requested_definitions: &[AttributeDefinition],
) -> Result<(), crate::state::RequestError> {
    let declared = |name: &str| {
        requested_definitions
            .iter()
            .any(|definition| definition.attribute_name == name)
    };
    if key_schema
        .iter()
        .all(|element| declared(&element.attribute_name))
    {
        return Ok(());
    }
    if requested_definitions.is_empty() {
        return Err(crate::state::RequestError::Validation(
            definitions_required_message(index_name),
        ));
    }
    Err(super::create_table::undefined_key_refusal(
        key_schema,
        requested_definitions,
    ))
}

/// AWS's fixed text for an index this request CREATES whose key attributes the request
/// declares no `AttributeDefinitions` for at all: the sentence names the member the
/// request has to carry and the index that needs it, and nothing of the key schema — with
/// no definition declared there is no list to report.
fn definitions_required_message(index_name: &str) -> String {
    format!(
        "One or more parameter values were invalid: AttributeDefinitions is not specified for \
         index: {index_name}"
    )
}

/// AWS's fixed text for a `Create` naming an index the table already carries: the whole
/// sentence states the work that cannot be done and interpolates nothing — neither the
/// index nor the table — so it reads the same whichever index of whichever table a request
/// asks to create twice.
const INDEX_ALREADY_EXISTS_MESSAGE: &str = "Attempting to create an index which already exists";

/// The table's global secondary indexes after the update.
///
/// A `Create` adds one (reported `CREATING` until the clock makes it `ACTIVE`), a
/// `Delete` puts one on its way out — the index is KEPT in the list and reported
/// `DELETING` ([`crate::state::GlobalSecondaryIndex::begin_deletion`]) until the clock
/// makes the removal due ([`crate::state::Table::apply_due_index_removals`]) — and an
/// `Update` moves one's provisioned throughput and leaves
/// it `UPDATING` for the same span. A switch of the table's billing mode carries its
/// indexes with it, because an index's capacity mode is the table's.
///
/// Which is also why the rules about what the TABLE may carry are asked of the indexes this
/// update leaves it KEEPING rather than of the whole list: an index on its way out takes no
/// slot of the per-table quota, keeps no attribute definition alive and needs no capacity
/// of its own — the same rule a VECTOR index on its way out is held by
/// ([`resolve_vector_indexes`]).
///
/// `definitions` are the ones in force for the updated table — the stored ones plus the
/// ones this request declares ([`merged_definitions`]). What the REQUEST itself declared
/// is not read here: an index this call creates is held to having its key attributes in
/// that list before the table is resolved at all
/// ([`check_requested_index_definitions`]).
fn resolve_indexes(
    table: &crate::state::Table,
    billing_mode: &str,
    definitions: &[AttributeDefinition],
    updates: &[GlobalSecondaryIndexUpdate],
    now: ::std::time::SystemTime,
) -> Result<Vec<crate::state::GlobalSecondaryIndex>, crate::state::RequestError> {
    let mut indexes = table.global_secondary_indexes.clone();
    // An index of a PAY_PER_REQUEST table has no provisioned throughput, and one of a
    // PROVISIONED table must have some; a mode switch resolves each stored index the
    // same way the table's own throughput was resolved.
    if billing_mode == "PAY_PER_REQUEST" {
        for index in &mut indexes {
            // The index's reported capacity is withdrawn with the table's, by the same
            // rule the table-level move in `handle` follows.
            if index.provisioned_throughput.is_some() {
                index.throughput_history.record_capacity_withdrawal(now);
            }
            index.provisioned_throughput = None;
            // Its warm throughput is NOT withdrawn with it: the capacity the index was
            // keeping warm stays warm, by the same rule the table-level move follows.
        }
    }
    for update in updates {
        if let Some(create) = &update.create {
            if indexes.iter().any(|index| index.name == create.index_name) {
                // AWS states the fact about the work asked for and names neither the
                // index nor the table: the create cannot be served because that index is
                // already there, which reads the same for every index of every table.
                return Err(crate::state::RequestError::Validation(
                    INDEX_ALREADY_EXISTS_MESSAGE.to_string(),
                ));
            }
            if indexes
                .iter()
                .filter(|index| !index.being_deleted())
                .count()
                >= crate::state::MAX_GLOBAL_SECONDARY_INDEXES
            {
                return Err(crate::state::RequestError::Validation(format!(
                    "Invalid GlobalSecondaryIndexUpdates: table {} already carries the maximum \
                     of {} global secondary indexes",
                    table.name,
                    crate::state::MAX_GLOBAL_SECONDARY_INDEXES
                )));
            }
            super::create_table::validated_index_key_schema(
                &create.index_name,
                &create.key_schema,
                definitions,
                // `GlobalSecondaryIndexUpdates` creates a GLOBAL secondary index — a local
                // index can only be defined with the table — so its key schema is held to
                // that kind's shape.
                &super::create_table::IndexKeyShape::GLOBAL,
            )?;
            super::create_table::validated_projection(&create.projection)?;
            let throughput = super::create_table::validated_index_throughput(
                &create.index_name,
                billing_mode,
                create.provisioned_throughput.clone(),
            )?;
            // An index the request names no warm throughput for keeps warm the capacity
            // it is provisioned for; one that names it is held to the same rule the
            // table-level member is ([`crate::state::resolved_warm_throughput`]).
            let warm_throughput = crate::state::resolved_warm_throughput(
                create.warm_throughput.as_ref(),
                &crate::state::WarmThroughputSettings::tracking(throughput.as_ref(), now),
                now,
            )?;
            indexes.push(crate::state::GlobalSecondaryIndex {
                arn: crate::state::index_arn(&table.region, &table.name, &create.index_name),
                name: create.index_name.clone(),
                key_schema: create.key_schema.clone(),
                projection: create.projection.clone(),
                provisioned_throughput: throughput,
                transient_status: crate::state::IndexStatus::Creating,
                // An index is built when it has read what it was given to read and the
                // table's own update has settled, so one added to a table that already
                // holds items keeps building after that update
                // ([`crate::state::IndexBackfill::settled_at`]).
                active_at: added_index_backfill(table, now)
                    .settled_at(now + crate::state::STATUS_TRANSITION),
                throughput_history: crate::state::ThroughputHistory::default(),
                warm_throughput,
                // An index added to a table that already holds items is metered on the
                // same schedule as every other one, starting from zero.
                reported_counters: crate::state::MeteredCounters::new(now),
                // Contributor insights are off on a new index, as they are on a new
                // table: only `UpdateContributorInsights` turns them on.
                contributor_insights: crate::state::ContributorInsights::default(),
                // An index ADDED to a table has the items that table already holds to
                // read, which is the backfill its description reports until those items
                // have been processed ([`crate::state::IndexBackfill`]).
                backfill: added_index_backfill(table, now),
            });
        }
        if let Some(delete) = &update.delete {
            // A LOCAL secondary index is on the table but is not a global one, so this
            // member cannot delete it: the index exists and the work does not
            // ([`crate::state::cannot_delete_non_global_secondary_index`]), which is a
            // different refusal from one that names nothing on the table at all.
            if table
                .local_secondary_indexes
                .iter()
                .any(|index| index.name == delete.index_name)
            {
                return Err(crate::state::cannot_delete_non_global_secondary_index(
                    &delete.index_name,
                ));
            }
            let Some(index) = indexes
                .iter_mut()
                .find(|index| index.name == delete.index_name && !index.being_deleted())
            else {
                // An entry naming an index the table does not carry — or one whose delete
                // has already been accepted — is a lookup miss on that index
                // ([`crate::state::lifecycle_index_not_found`]). The stored indexes are left
                // exactly as they were, because `handle` writes nothing until this whole
                // list resolves.
                return Err(crate::state::lifecycle_index_not_found(
                    &table.name,
                    &delete.index_name,
                ));
            };
            // The removal falls due when the table's own update settles, which is the span
            // this very call reports the table `UPDATING` for: a caller that waits for the
            // table and then describes it sees the index gone, and one that acts straight
            // after the call sees it `DELETING`
            // ([`crate::state::GlobalSecondaryIndex::begin_deletion`]).
            index.begin_deletion(now + crate::state::STATUS_TRANSITION);
        }
        if let Some(change) = &update.update {
            let Some(index) = indexes
                .iter_mut()
                .find(|index| index.name == change.index_name && !index.being_deleted())
            else {
                // An `Update` names the index the same way a `Delete` does, so an index
                // the table does not carry — or one already on its way out — is reported
                // with the same sentence: one index-lifecycle surface, one wording.
                return Err(crate::state::lifecycle_index_not_found(
                    &table.name,
                    &change.index_name,
                ));
            };
            if change.on_demand_throughput.is_some() {
                // Refused up front by `unsupported_member`, which answers the
                // scaffold's 501 for it; reaching here would mean that check was
                // bypassed, so it is an internal inconsistency rather than a bad
                // request.
                return Err(crate::state::RequestError::Validation(format!(
                    "Invalid GlobalSecondaryIndexUpdates: index {} can only be updated through \
                     ProvisionedThroughput and WarmThroughput in this emulator",
                    change.index_name
                )));
            }
            let throughput = super::create_table::validated_index_throughput(
                &change.index_name,
                billing_mode,
                change
                    .provisioned_throughput
                    .clone()
                    .or_else(|| index.provisioned_throughput.clone()),
            )?;
            // The same rule the table's own capacity is held to: a pair the index is
            // already provisioned for is no move, so the entry that re-states it is
            // refused instead of leaving the index UPDATING for work there is none of.
            if let (Some(requested), Some(current)) =
                (&change.provisioned_throughput, &index.provisioned_throughput)
            {
                if requested.read_capacity_units == current.read_capacity_units
                    && requested.write_capacity_units == current.write_capacity_units
                {
                    return Err(crate::state::RequestError::Validation(
                        crate::state::throughput_unchanged_message(
                            &format!("the index {}", change.index_name),
                            "the index",
                            current,
                            requested,
                        ),
                    ));
                }
            }
            // One rule for both levels: the move is measured against the capacity the
            // description REPORTS (0/0 for an index with none), so an index given
            // capacity it did not have reports that as an increase, exactly as the
            // table's own move does.
            let before = crate::state::reported_capacity(index.provisioned_throughput.as_ref());
            let after = crate::state::reported_capacity(throughput.as_ref());
            index.throughput_history.record(now, &before, &after);
            index.provisioned_throughput = throughput;
            // An index nobody has named a warm throughput for keeps warm the capacity it
            // is now provisioned for; one the request names it for takes what it asked.
            index.warm_throughput = crate::state::resolved_warm_throughput(
                change.warm_throughput.as_ref(),
                &index
                    .warm_throughput
                    .tracking_capacity(index.provisioned_throughput.as_ref()),
                now,
            )?;
            index.transient_status = crate::state::IndexStatus::Updating;
            index.active_at = now + crate::state::STATUS_TRANSITION;
        }
    }
    // "When switching from pay-per-request to provisioned capacity, initial provisioned
    // capacity values must be set" (`UpdateTable$BillingMode`) — and an index's capacity
    // mode is the table's, so every index needs one too. Refusing names the index rather
    // than leaving it reporting a capacity of zero, which a PROVISIONED index never has.
    if billing_mode == "PROVISIONED" {
        for index in indexes.iter().filter(|index| !index.being_deleted()) {
            if index.provisioned_throughput.is_none() {
                return Err(crate::state::RequestError::Validation(
                    crate::state::index_capacity_required_message(&index.name),
                ));
            }
        }
    }
    Ok(indexes)
}

/// The table's vector indexes after the update: every one a `Delete` names is on its way
/// out, and every one a `Create` names is added, `CREATING` until the clock settles it
/// `ACTIVE` and backfilling the items the table already holds for that span
/// ([`crate::state::IndexBackfill`]).
///
/// A `Delete` naming a vector index the table does not carry names a resource that is not
/// there, so it is refused with the index-lifecycle surface's own sentence
/// ([`crate::state::lifecycle_index_not_found`]), and because this resolves the whole list
/// before `handle` writes anything, the stored indexes are left exactly as they were.
///
/// An index a `Delete` names is KEPT in the list and marked as being removed
/// ([`crate::state::VectorIndex::begin_deletion`]): removing it is asynchronous work on the
/// table, so every description taken before that work falls due still reports the index,
/// with "`DELETING` - The index is being deleted" (`VectorIndexDescription$IndexStatus`),
/// and the entry disappears when the clock makes the removal due
/// ([`crate::state::Table::apply_due_index_removals`]). A caller reading the response to
/// learn the new index state can therefore tell a delete it requested from one that has
/// finished, and from a name the table never carried.
///
/// Which is also why the rules about what the TABLE may carry are asked of the indexes this
/// update leaves it KEEPING rather than of the whole list: an index on its way out takes no
/// slot of the per-table quota, holds no dimension count against a future create and does
/// not hold the table in the capacity mode a vector index needs.
///
/// A `Create` is held to what the table can carry, exactly as `CreateTable` holds a vector
/// index declared WITH the table: the table's capacity mode is the on-demand one
/// ([`crate::state::check_vector_index_capacity_mode`]), a name no other index of the table
/// already has, a `Projection` that holds together, a search-schema attribute the table has
/// an `AttributeDefinition` for (its type is what the index partitions and filters by), the
/// per-table index count ([`crate::state::check_vector_index_count`]) and the one dimension
/// count every index over a vector attribute shares
/// ([`crate::state::check_vector_dimensions_agree`]). One call carries ONE index action
/// ([`vector_index_actions_over_limit`]), so the two loops below never both do work for the
/// same request; the deletes are walked first all the same, which is the order the table
/// would be left in if the limit ever let both through.
fn resolve_vector_indexes(
    table: &crate::state::Table,
    billing_mode: &str,
    updates: &[VectorIndexUpdate],
    definitions: &[AttributeDefinition],
    region: &str,
    now: ::std::time::SystemTime,
) -> Result<Vec<crate::state::VectorIndex>, crate::state::RequestError> {
    let mut indexes = table.vector_indexes.clone();
    for delete in updates.iter().filter_map(|update| update.delete.as_ref()) {
        let Some(index) = indexes
            .iter_mut()
            .find(|index| index.name == delete.index_name && !index.being_deleted())
        else {
            return Err(crate::state::lifecycle_index_not_found(
                &table.name,
                &delete.index_name,
            ));
        };
        // The removal falls due when the table's own update settles, which is the span
        // this very call reports the table `UPDATING` for: a caller that waits for the
        // table and then describes it sees the index gone, and one that acts straight
        // after the call sees it `DELETING`.
        index.begin_deletion(now + crate::state::STATUS_TRANSITION);
    }
    for create in updates.iter().filter_map(|update| update.create.as_ref()) {
        let taken = indexes.iter().any(|stored| stored.name == create.index_name)
            || table
                .global_secondary_indexes
                .iter()
                .any(|stored| stored.name == create.index_name)
            || table
                .local_secondary_indexes
                .iter()
                .any(|stored| stored.name == create.index_name);
        if taken {
            return Err(crate::state::RequestError::Validation(format!(
                "Invalid VectorIndex: duplicate index name {}",
                create.index_name
            )));
        }
        // What the declaration has to satisfy, and the index it stores, are `CreateTable`'s
        // to state: the two members describe the same resource, so a vector index added
        // here is held to exactly the rules one declared with the table is
        // ([`crate::ops::create_table::DeclaredVectorIndex`]).
        indexes.push(
            crate::ops::create_table::DeclaredVectorIndex {
                index_name: &create.index_name,
                vector_attribute: &create.vector_attribute,
                search_schema: create.search_schema.as_deref(),
                projection: &create.projection,
                dimensions: create.dimensions,
                distance_function: &create.distance_function,
            }
            .validated(
                definitions,
                region,
                &table.name,
                // What this index has to read is what the table ALREADY holds: an index
                // added to a populated table backfills those items, and one added to an
                // empty table has nothing to read
                // ([`crate::state::IndexBackfill`]). Its resources are allocated first,
                // which takes the span the table's own update takes, and reading the items
                // takes one more — so the table settles while the index is still being
                // built, which is what a client that waits for the table and then looks at
                // the index sees.
                added_index_backfill(table, now),
                now,
            )?,
        );
    }
    // The per-table vector-index quota and the one dimension count each vector attribute
    // may have are the TABLE's rules, so both are asked of the indexes this update leaves
    // the table KEEPING ([`crate::state::check_vector_index_count`],
    // [`crate::state::check_vector_dimensions_agree`]): an index added here is held to
    // exactly what one declared with the table is, and one on its way out is held against
    // nothing.
    let kept: Vec<crate::state::VectorIndex> = indexes
        .iter()
        .filter(|index| !index.being_deleted())
        .cloned()
        .collect();
    crate::state::check_vector_index_count(kept.len())?;
    crate::state::check_vector_dimensions_agree(&kept)?;
    // A vector index exists only on an ON-DEMAND table, so the mode this very update
    // leaves the table in is what every index it will still CARRY has to hold on
    // ([`crate::state::check_vector_index_capacity_mode`]): an update that adds one to a
    // provisioned table is refused, and so is one that switches a table carrying vector
    // indexes to the provisioned mode — while one that removes the last of them in the
    // same call may switch it, because the table is left carrying none. Asked of the
    // resolved list, after the deletes and the creates above, so one rule answers all
    // three.
    crate::state::check_vector_index_capacity_mode(billing_mode, kept.len())?;
    Ok(indexes)
}
